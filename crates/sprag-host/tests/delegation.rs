//! The daemon asking systemd for a subtree of its own, against a real user manager.
//!
//! `sprag_terminal::share`'s own gate proves the tree enforces a share ONCE it has a root it may
//! write. This proves the half that gets it one: without delegation the root arrives owned by
//! systemd, `mkdir` inside it is denied, and every pane opens unplaced no matter how correct the
//! tree code is.
//!
//! The discriminator is `Delegate=true`. Drop that one property and systemd still creates the
//! scope, still moves the process into it, and still answers success — and the tree then cannot be
//! built in it at all. So this test asserts the thing the property buys, not that the call
//! returned.
//!
//! # Why it delegates a CHILD and not the test runner
//!
//! Moving this process into a delegated scope is a one-way trip: coming back out needs write access
//! to the cgroup it came from, which nobody delegated. A child can be delegated, measured and
//! killed, leaving the runner where it started.
//!
//! Skips — saying which precondition was missing — on a host with no systemd user manager.

#![cfg(target_os = "linux")]

use std::process::{Child, Command, Stdio};

use sprag_host::delegation::{self, DelegationError};
use sprag_terminal::registry::{SessionId, WindowId};
use sprag_terminal::share::{PaneLineage, Share, Tree};
use sprag_terminal::workspace::PaneId;

#[test]
fn a_delegated_scope_is_a_subtree_this_daemon_can_build_a_pane_tree_in() {
    let Some(mut holder) = Holder::spawn() else {
        return;
    };

    let delegated = match delegation::acquire(holder.pid()) {
        Ok(delegated) => delegated,
        Err(error) => {
            // A machine that cannot delegate is not a failing crate, but the reason has to reach a
            // reader — a silent skip and a pass are the same colour.
            eprintln!("SKIP: {error}");
            return;
        }
    };
    // Handed over BEFORE the first assertion. Every assertion below can panic, and a panic that
    // skipped the teardown would leave a live systemd unit behind on the developer's machine —
    // which is exactly what happened the first time this gate went red.
    holder.owns(delegated.unit());

    let root = delegated.root().to_path_buf();
    assert!(
        root.is_dir(),
        "systemd reported a scope whose cgroup is not there: {}",
        root.display()
    );
    // The controller the whole feature rests on. A scope under a slice that never got `cpu` would
    // arrive here looking fine and weight nothing.
    let controllers = std::fs::read_to_string(root.join("cgroup.controllers"))
        .expect("a delegated scope has a controller list");
    assert!(
        controllers
            .split_ascii_whitespace()
            .any(|name| name == "cpu"),
        "the delegated scope offers {controllers:?}, without the cpu controller"
    );

    // The actual claim: a tree can be built here. This is what `Delegate=true` buys, and what its
    // absence takes away while every call still reports success.
    let tree = Tree::adopt(root.clone()).expect("adopt the delegated root");
    let lineage = PaneLineage {
        session: SessionId(1),
        window: WindowId(1),
        pane: PaneId(1),
    };
    let placed = tree.place(lineage, Share::EVEN).expect("place a pane");
    let weight = std::fs::read_to_string(placed.path().join("cpu.weight"))
        .expect("a placed pane carries a weight the kernel accepted");
    assert_eq!(weight.trim(), "100");

    tree.release(lineage).expect("release the pane");
}

#[test]
fn a_daemon_with_no_user_manager_is_told_which_precondition_was_missing() {
    // Not a mock: an unreachable bus address is the real shape of "started without a session bus",
    // which is how a daemon under a bare init or in a container meets this.
    let restore = std::env::var("DBUS_SESSION_BUS_ADDRESS").ok();
    // SAFETY: single-threaded at this point in the test binary's own process; restored below.
    unsafe {
        std::env::set_var(
            "DBUS_SESSION_BUS_ADDRESS",
            "unix:path=/nonexistent/sprag-bus",
        );
    }

    let failed = delegation::acquire(std::process::id());

    unsafe {
        match restore {
            Some(value) => std::env::set_var("DBUS_SESSION_BUS_ADDRESS", value),
            None => std::env::remove_var("DBUS_SESSION_BUS_ADDRESS"),
        }
    }

    let Err(error) = failed else {
        panic!("a bus that is not there answered a delegation request");
    };
    assert!(
        matches!(error, DelegationError::NoSessionBus { .. }),
        "expected the missing-bus precondition, got {error}"
    );
    // The message is the product here: it is what a person reads when their panes open unplaced.
    assert!(error.to_string().contains("no systemd user manager"));
}

/// The process this test delegates, and the unit it ends up in — both torn down on the way out,
/// panic or not.
struct Holder {
    /// The process systemd is asked to move.
    child: Child,
    /// The scope it was moved into, once there is one.
    unit: Option<String>,
}

impl Holder {
    /// Start a process to delegate. `None`, with a reason on stderr, where one cannot be started.
    fn spawn() -> Option<Self> {
        match Command::new("sleep")
            .arg("600")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => Some(Self { child, unit: None }),
            Err(source) => {
                eprintln!("SKIP: cannot start a process to delegate: {source}");
                None
            }
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Take responsibility for stopping `unit` however this test ends.
    fn owns(&mut self, unit: &str) {
        self.unit = Some(unit.to_owned());
    }
}

impl Drop for Holder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(unit) = &self.unit {
            let _ = Command::new("systemctl")
                .args(["--user", "stop", unit])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}
