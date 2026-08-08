//! Asking systemd for a cgroup subtree this daemon may build panes into.
//!
//! [`sprag_terminal::share::Tree`] can build the whole session/window/pane tree and weight every
//! level of it — given a root it is allowed to write. Nothing hands it one. A multiplexer launched
//! from a desktop terminal lands in that terminal's own transient scope, which systemd owns and
//! which was never given the CPU controller in the first place (measured: `memory pids`, no `cpu`).
//! This module is the one call that changes that.
//!
//! # Why D-Bus, and not the `systemd-run` binary
//!
//! The daemon is ALREADY RUNNING when it needs a subtree. `systemd-run` starts a *command* in a new
//! unit; it has no spelling for "take this pid and put it in a new scope". `StartTransientUnit`'s
//! `PIDs` property does exactly that, and it is reachable only over the bus. So the choice is not
//! between a library and a subprocess — it is between this and not being able to express the thing.
//!
//! It also avoids what wrapping would cost: no rewritten argv, no extra process in the tree, and no
//! changed process group. That last one is not hypothetical — `systemd-run --scope` puts the
//! command it runs in a NEW process group (measured: caller 2261215, child 2261224), and a
//! multiplexer resolves a pane's foreground job by comparing process groups.
//!
//! # Once, at startup, for the daemon itself
//!
//! One unit per daemon, not one per pane. Everything below it is plain `mkdir` in a subtree the
//! kernel has handed over, so the bus is never on the path of opening a pane.
//!
//! # What a host without systemd gets
//!
//! An error naming which precondition was missing, and a daemon that carries on. Resource control
//! is the thing that fails, never the terminal: `Enforcement::probe` then answers `Unenforceable`
//! and the panes open unplaced, which is the designed outcome rather than a degraded one.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;
use zbus::zvariant::Value;

/// systemd's own bus name, object and interface — the user manager answers on the session bus.
const SYSTEMD_SERVICE: &str = "org.freedesktop.systemd1";
/// The manager object every unit operation goes through.
const SYSTEMD_PATH: &str = "/org/freedesktop/systemd1";
/// The interface `StartTransientUnit` lives on.
const SYSTEMD_MANAGER: &str = "org.freedesktop.systemd1.Manager";

/// How long to wait for the kernel to actually move the process into the new scope.
///
/// `StartTransientUnit` returns when the job is ENQUEUED, not when it is done, so the move is
/// observed rather than assumed. Generous, because the cost of being wrong is a daemon that runs
/// unplaced for its whole life; a busy user manager taking a moment is not a reason to give up.
const MOVE_TIMEOUT: Duration = Duration::from_secs(5);

/// How often to look while waiting for that move.
const POLL: Duration = Duration::from_millis(20);

/// A cgroup subtree systemd has handed to this daemon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Delegated {
    /// The transient unit that owns it, for a person reading `systemctl --user status`.
    unit: String,
    /// The subtree's root — what [`sprag_terminal::share::Tree::adopt`] takes.
    root: PathBuf,
}

impl Delegated {
    /// The scope unit systemd created.
    #[must_use]
    pub fn unit(&self) -> &str {
        &self.unit
    }

    /// The delegated root, ready to be adopted.
    #[must_use]
    pub fn root(&self) -> &std::path::Path {
        &self.root
    }
}

/// Ask the systemd user manager to put `pid` in a new delegated scope, and wait until it is there.
///
/// `pid` is normally this process — a daemon asking for its own subtree. It is a parameter rather
/// than `std::process::id()` so the act can be TESTED: delegating the test runner itself is a
/// one-way trip, because moving back out afterwards would need write access to a cgroup nobody
/// delegated. Handing in a child makes the whole thing reversible.
///
/// # Errors
///
/// Returns [`DelegationError`] if there is no session bus to ask, if systemd refuses, or if the
/// process never arrives in the new scope. None of these is fatal to a daemon: the caller reports
/// the reason and opens panes unplaced.
pub fn acquire(pid: u32) -> Result<Delegated, DelegationError> {
    let unit = format!("sprag-{pid}.scope");
    let connection = Connection::session().map_err(|source| DelegationError::NoSessionBus {
        source: Box::new(source),
    })?;

    let properties: Vec<(&str, Value<'_>)> = vec![
        // The whole point: without it systemd keeps ownership of the subtree and every `mkdir`
        // below is denied.
        ("Delegate", Value::Bool(true)),
        // Move THIS process rather than start a new one — the reason this is a bus call at all.
        ("PIDs", Value::from(vec![pid])),
        // Let a dead scope be collected instead of lingering as a failed unit somebody has to
        // reset by hand before the daemon can start again under the same name.
        ("CollectMode", Value::from("inactive-or-failed")),
        ("Description", Value::from("sprag pane resource tree")),
    ];
    // No auxiliary units.
    let aux: Vec<(&str, Vec<(&str, Value<'_>)>)> = Vec::new();

    connection
        .call_method(
            Some(SYSTEMD_SERVICE),
            SYSTEMD_PATH,
            Some(SYSTEMD_MANAGER),
            "StartTransientUnit",
            // "fail" rather than "replace": if something already holds this name, that is a fact
            // worth reporting, not one to overwrite.
            &(unit.as_str(), "fail", properties, aux),
        )
        .map_err(|source| DelegationError::Refused {
            unit: unit.clone(),
            source: Box::new(source),
        })?;

    // The reply means "job accepted", so the move is WATCHED rather than assumed. Reading the
    // process's own cgroup is the same question `Enforcement::probe` asks, answered about a pid.
    let deadline = Instant::now() + MOVE_TIMEOUT;
    while Instant::now() < deadline {
        if let Some(root) = sprag_terminal::share::cgroup_of(pid)
            && root.file_name().is_some_and(|name| name == unit.as_str())
        {
            return Ok(Delegated { unit, root });
        }
        std::thread::sleep(POLL);
    }

    Err(DelegationError::NeverArrived { unit })
}

/// Why this daemon has no subtree of its own.
///
/// Every arm names a precondition rather than an error code, because the only useful thing to do
/// with one is tell a person which part of their machine did not answer.
#[derive(Debug)]
pub enum DelegationError {
    /// No session bus to ask — no systemd user manager, or a daemon started without one in reach.
    NoSessionBus {
        /// What the bus client said.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The manager was reachable and said no.
    Refused {
        /// The unit that was asked for.
        unit: String,
        /// What systemd said.
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// The job was accepted, but the process never appeared in the new scope.
    NeverArrived {
        /// The unit that was asked for.
        unit: String,
    },
}

impl std::fmt::Display for DelegationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSessionBus { source } => {
                write!(f, "no systemd user manager to ask for a cgroup: {source}")
            }
            Self::Refused { unit, source } => {
                write!(f, "systemd refused the scope {unit}: {source}")
            }
            Self::NeverArrived { unit } => write!(
                f,
                "systemd accepted the scope {unit} but the process never moved into it"
            ),
        }
    }
}

impl std::error::Error for DelegationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NoSessionBus { source } | Self::Refused { source, .. } => Some(&**source),
            Self::NeverArrived { .. } => None,
        }
    }
}
