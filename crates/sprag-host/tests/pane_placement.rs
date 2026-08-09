//! A pane's cgroup is where the pane IS — by every door it can arrive through, and after it moves.
//!
//! The two halves are gated apart already — `sprag-terminal` proves the tree enforces a share, and
//! `tests/delegation.rs` proves the daemon can get a subtree to build one in. Neither notices if
//! nothing ever calls `place` at pane birth, which is the state this product was in between them:
//! a daemon holding a perfectly good tree with nothing in it.
//!
//! So this asserts the join, and it asserts it by the only evidence that cannot be faked from the
//! sprag side — **the child's pid, read out of the kernel's own `cgroup.procs`** for the leaf the
//! session, window and pane ids spell.
//!
//! # Why there is a test per DOOR
//!
//! R336 wired exactly one of them ([`Host::spawn`]) and said so. A pane also arrives from the
//! daemon's wire (`spawn`/`split`), from a durability restore, from an in-process client's
//! `new_pane`, and from another WINDOW when a person breaks it out. Four doors onto one action is
//! the shape that produces a feature which works in the test that was written for it and nowhere a
//! person actually goes — so each door is a test, and they assert the same thing about the same
//! kernel file.
//!
//! The move is not a fifth door but the claim itself: the resource tree is a PROJECTION of the
//! identity tree, and a projection that only holds at birth is not one. A pane broken into a new
//! window has a new identity, so its processes belong under the new window's cgroup — otherwise the
//! window a person is looking at is charged for work it does not hold.
//!
//! Skips, saying so, where systemd will not delegate.

#![cfg(target_os = "linux")]

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Once};

use pinion_core::external::{ExternalIntrospect, IntrospectValue};
use sprag_host::host::{Host, HostClient};
use sprag_host::notify::ChannelRegistry;
use sprag_host::scope::SessionScope;
use sprag_host::wire::SPAWN_ACTION;
use sprag_host::{DaemonShared, WorkspaceExternal, delegation};
use sprag_terminal::{CommandBuilder, PaneBirthHooks, PaneId, Tree};

#[test]
fn a_pane_born_under_a_host_with_a_tree_has_its_child_in_its_own_cgroup() {
    let Some(mut holder) = Holder::spawn() else {
        return;
    };
    let delegated = match delegation::acquire(holder.pid()) {
        Ok(delegated) => delegated,
        Err(error) => {
            eprintln!("SKIP: {error}");
            return;
        }
    };
    holder.owns(delegated.unit());

    let tree = Arc::new(Tree::adopt(delegated.root().to_path_buf()).expect("adopt"));
    let host = Host::new((80, 24)).with_shares(Arc::clone(&tree));

    let mut command = CommandBuilder::new("/bin/sleep");
    command.arg("30");
    let pane = host
        .spawn(
            command,
            "sleep".to_owned(),
            80,
            24,
            PaneBirthHooks::default(),
        )
        .expect("spawn a pane");

    // A SECOND window, with its own pane. Without it this gate cannot tell a host that resolves the
    // pool it actually spawned into from one that always answers "the first window" — measured: that
    // mutation was GREEN against a single-window host, which is a gate that passes and does not
    // discriminate.
    host.new_window();
    let mut second = CommandBuilder::new("/bin/sleep");
    second.arg("30");
    let elsewhere = host
        .spawn(
            second,
            "sleep".to_owned(),
            80,
            24,
            PaneBirthHooks::default(),
        )
        .expect("spawn a pane in the second window");

    // The leaves are found by WALKING the tree rather than by spelling paths here: what the ids are
    // is the registry's business, and a test that hard-coded `session-0/window-1` would be asserting
    // this run's numbering instead of the thing that matters. The COUNT is not asserted either —
    // opening a window gives it a pane of its own, so how many there are is that verb's business.
    let leaves = pane_leaves(tree.root());
    let leaf = leaf_of(&leaves, pane);
    let far = leaf_of(&leaves, elsewhere);

    // THE DISCRIMINATOR: two panes opened in two windows are under two window cgroups. Resolving
    // the pool by `Arc` identity gives that; answering "the first window" every time does not, and
    // a single-window host cannot tell the two apart.
    assert_ne!(
        leaf.parent(),
        far.parent(),
        "panes of two different windows landed under one window cgroup"
    );

    let weight = std::fs::read_to_string(leaf.join("cpu.weight")).expect("the leaf is weighted");
    assert_eq!(weight.trim(), "100");

    // The claim, in the kernel's own words: this pane's child is IN this pane's cgroup.
    let members: Vec<u32> = std::fs::read_to_string(leaf.join("cgroup.procs"))
        .expect("cgroup.procs")
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect();
    assert!(
        !members.is_empty(),
        "pane {} was placed in {} and nothing was moved into it",
        pane.0,
        leaf.display()
    );
    for pid in &members {
        let argv = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        assert!(
            argv.contains("sleep"),
            "pid {pid} in the pane's cgroup is not the pane's child: {argv:?}"
        );
    }
}

/// A pane whose program forks IMMEDIATELY keeps its grandchildren too.
///
/// This is the gate for the thing that made sprag own its pseudoterminal. When the child was moved
/// into its cgroup AFTER `exec`, a shell that forked in that window put its children in the
/// DAEMON's cgroup — measured, 2 of 2 — so work a person started in their pane was charged to the
/// daemon and escaped its window's share entirely. Joining before `exec` closes it by construction:
/// nothing the child forks can be born outside a cgroup the child is already in.
#[test]
fn a_pane_whose_program_forks_at_once_keeps_its_grandchildren() {
    let Some(mut holder) = Holder::spawn() else {
        return;
    };
    let delegated = match delegation::acquire(holder.pid()) {
        Ok(delegated) => delegated,
        Err(error) => {
            eprintln!("SKIP: {error}");
            return;
        }
    };
    holder.owns(delegated.unit());

    let tree = Arc::new(Tree::adopt(delegated.root().to_path_buf()).expect("adopt"));
    let host = Host::new((80, 24)).with_shares(Arc::clone(&tree));

    // A shell that forks before it has done anything else — the shape of a `.bashrc` that starts
    // something, which is how a person meets this without trying to.
    let mut command = CommandBuilder::new("/bin/sh");
    command.args(["-c", "sleep 60 & sleep 60"]);
    let pane = host
        .spawn(command, "sh".to_owned(), 80, 24, PaneBirthHooks::default())
        .expect("spawn a forking pane");

    let leaf = leaf_of(&pane_leaves(tree.root()), pane).to_path_buf();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut members = 0;
    while std::time::Instant::now() < deadline {
        members = std::fs::read_to_string(leaf.join("cgroup.procs"))
            .expect("cgroup.procs")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count();
        if members >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    // The shell and BOTH of the things it forked. One means only the shell arrived and its children
    // were born somewhere else, which is exactly the defect.
    assert!(
        members >= 3,
        "the pane's cgroup holds {members} process(es); the shell's children were born outside it"
    );
}

/// A pane born over the DAEMON'S WIRE — the door a `split` or a `new-pane` comes through.
///
/// This is the door a person actually uses: `Host::spawn` is the BOOT pane and nothing else. The
/// wire's dispatch is built from the registry alone and never sees the host, so a design that hangs
/// the tree off the host leaves every pane a person opens unweighted while the one the daemon
/// started with is placed perfectly — measured, and the reason placement moved onto the pool.
#[test]
fn a_pane_born_over_the_daemon_wire_lands_in_its_own_cgroup() {
    let Some((_holder, tree)) = delegated() else {
        return;
    };
    let host = Host::new((80, 24)).with_shares(Arc::clone(&tree));
    let registry = Arc::clone(host.registry());
    let mut wire = WorkspaceExternal::new(
        Arc::clone(&registry),
        SessionScope::unscoped(&registry),
        Arc::new(ChannelRegistry::default()),
        DaemonShared::none(),
    );

    let answer = wire
        .invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(serde_json::json!({"cmd": ["/bin/sleep", "30"]})),
        )
        .expect("the wire spawns a pane");
    let IntrospectValue::Int(id) = answer else {
        panic!("the spawn action answers with the new pane's id, got {answer:?}");
    };
    let pane = PaneId(u64::try_from(id).expect("a pane id is not negative"));

    assert_child_in_cgroup(&tree, pane, "sleep");
}

/// A pane that COMES BACK from a durability snapshot.
///
/// A restore replaces every pool in the registry, so it is where an installed-at-construction
/// source is silently lost — the exact asymmetry `set_pane_env_source` is re-installed here to
/// avoid, and the one a share source would fall into if it were carried anywhere but the pool.
/// A pane placed on every path EXCEPT after a reboot is a pane nobody would ever catch being
/// unplaced.
#[test]
fn a_pane_that_comes_back_from_a_snapshot_lands_in_its_own_cgroup() {
    let Some((_holder, tree)) = delegated() else {
        return;
    };

    // The pre-reboot daemon needs no tree: what is under test is where the RESTORED pane lands.
    let live = Host::new((80, 24));
    let mut command = CommandBuilder::new("/bin/sleep");
    command.arg("30");
    let pane = live
        .spawn(
            command,
            "sleep".to_owned(),
            80,
            24,
            PaneBirthHooks::default(),
        )
        .expect("spawn the pane that will be snapshotted");
    let snapshot = sprag_terminal::snapshot(live.registry());

    // Allowlisted so the pane re-runs `sleep` EXACTLY: a non-allowlisted argv comes back as a
    // shell, and then the pid in the cgroup could not be told from the daemon's own.
    let allowlist = std::collections::HashSet::from(["sleep".to_owned()]);
    let rebooted = Host::new((80, 24)).with_shares(Arc::clone(&tree));
    let back = rebooted
        .restore(
            snapshot,
            &allowlist,
            |_| None,
            || None,
            || None,
            |_| Vec::new(),
        )
        .expect("a valid snapshot restores");
    assert_eq!(back, 1, "the pane came back");

    assert_child_in_cgroup(&tree, pane, "sleep");
}

/// A pane an IN-PROCESS client asks for — [`HostClient::new_pane`], which a `prefix c` reaches and
/// which every `split` in that mode is built on.
#[test]
fn a_pane_an_in_process_client_asks_for_lands_in_its_own_cgroup() {
    hermetic_config();
    let Some((_holder, tree)) = delegated() else {
        return;
    };
    let host = Host::new((80, 24)).with_shares(Arc::clone(&tree));

    let pane = host.new_pane().expect("the client opens a pane");

    // The program is the user's `$SHELL` (hermetic here, so: whatever this machine's is) rather
    // than a name this test chose, so the claim is only that SOMETHING of this pane is in its
    // cgroup — which is the whole claim anyway.
    let leaf = leaf_of(&pane_leaves(tree.root()), pane).to_path_buf();
    assert!(
        !members_of(&leaf).is_empty(),
        "pane {} was placed in {} and nothing was moved into it",
        pane.0,
        leaf.display()
    );
}

/// A pane BROKEN OUT into a new window takes its cgroup with it.
///
/// The claim R336 made is that the resource tree is a projection of the identity tree. A pane's
/// identity changes when it moves — `break-pane`, `join-pane`, `move-pane` all do it — and a
/// projection that is only computed at birth stops being one at the first move. What a person sees
/// otherwise: they pull a runaway build out into its own window to contain it, and it goes on
/// eating the share of the window they pulled it out of.
#[test]
fn a_pane_broken_into_a_new_window_takes_its_cgroup_with_it() {
    let Some((_holder, tree)) = delegated() else {
        return;
    };
    let host = Host::new((80, 24)).with_shares(Arc::clone(&tree));

    // Two panes in ONE window: `break-pane` refuses a window's last pane, as tmux does.
    let stay = spawn_sleeper(&host, "stay");
    let moved = spawn_sleeper(&host, "moved");
    let before = leaf_of(&pane_leaves(tree.root()), moved).to_path_buf();
    assert_eq!(
        before.parent(),
        leaf_of(&pane_leaves(tree.root()), stay).parent(),
        "the two panes start under one window's cgroup"
    );

    host.break_pane(moved, None)
        .expect("the pane breaks out into a window of its own");

    // Exactly ONE leaf carries the moved pane's name. This is the release half, and it is asserted
    // here rather than in `share`'s unit tests because only a real cgroupfs lets a cgroup holding
    // the kernel's interface files be removed. Without it the pane would own two cgroups, and the
    // sweep would collect the empty one only at the next birth — leaving the window it left charged
    // for a directory in the meantime.
    let named: Vec<_> = pane_leaves(tree.root())
        .into_iter()
        .filter(|leaf| {
            leaf.file_name()
                .is_some_and(|name| name == format!("pane-{}", moved.0).as_str())
        })
        .collect();
    assert_eq!(
        named.len(),
        1,
        "the moved pane has {} cgroups: {named:?}",
        named.len()
    );

    let after = leaf_of(&pane_leaves(tree.root()), moved).to_path_buf();
    assert_ne!(
        after.parent(),
        leaf_of(&pane_leaves(tree.root()), stay).parent(),
        "the broken-out pane's cgroup is still under the window it left: {}",
        after.display()
    );

    // And it is the PROCESSES that moved, not just a directory: an empty new leaf beside a full old
    // one would pass the assertion above and change nothing about who is charged for the work.
    let members = members_of(&after);
    assert!(
        !members.is_empty(),
        "the pane's new cgroup {} is empty — the directory moved and the child did not",
        after.display()
    );
    for pid in &members {
        let argv = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        assert!(
            argv.contains("sleep"),
            "pid {pid} in the moved pane's cgroup is not its child: {argv:?}"
        );
    }
}

/// Spawn a long-lived, identifiable child into `host`'s current window.
fn spawn_sleeper(host: &Host, label: &str) -> PaneId {
    let mut command = CommandBuilder::new("/bin/sleep");
    command.arg("30");
    host.spawn(command, label.to_owned(), 80, 24, PaneBirthHooks::default())
        .expect("spawn a pane")
}

/// The pids the kernel says are in `cgroup`.
fn members_of(cgroup: &Path) -> Vec<u32> {
    std::fs::read_to_string(cgroup.join("cgroup.procs"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .collect()
}

/// Assert `pane` has a leaf in `tree` and that the kernel holds its child there, by argv.
fn assert_child_in_cgroup(tree: &Tree, pane: PaneId, program: &str) {
    let leaf = leaf_of(&pane_leaves(tree.root()), pane).to_path_buf();
    let members = members_of(&leaf);
    assert!(
        !members.is_empty(),
        "pane {} was placed in {} and nothing was moved into it",
        pane.0,
        leaf.display()
    );
    for pid in &members {
        let argv = std::fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        assert!(
            argv.contains(program),
            "pid {pid} in pane {}'s cgroup is not its child: {argv:?}",
            pane.0
        );
    }
}

/// A delegated subtree and the process holding it, or `None` with the missing precondition stated.
///
/// The holder comes back with the tree because dropping it tears the scope down: a helper that
/// returned the tree alone would hand every caller a root systemd had already reclaimed.
fn delegated() -> Option<(Holder, Arc<Tree>)> {
    let mut holder = Holder::spawn()?;
    let delegated = match delegation::acquire(holder.pid()) {
        Ok(delegated) => delegated,
        Err(error) => {
            eprintln!("SKIP: {error}");
            return None;
        }
    };
    holder.owns(delegated.unit());
    let tree = Tree::adopt(delegated.root().to_path_buf()).expect("adopt");
    Some((holder, Arc::new(tree)))
}

/// Point this test binary's config and state at a directory of its own, ONCE.
///
/// [`HostClient::new_pane`] runs the user's `default-command`, so without this the pane a test
/// opens is whatever the developer running the suite has in `config.toml` — the rule R318/R319/R331
/// each re-learned. Binary-wide rather than per test because the environment is process-global and
/// these tests run in parallel: setting it per test would be one thread writing what another reads.
fn hermetic_config() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pane-placement-xdg");
        for var in ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME"] {
            let dir = base.join(var.to_ascii_lowercase());
            std::fs::create_dir_all(&dir).expect("a directory for this binary's XDG roots");
            // SAFETY: the only write to the environment in this binary, behind a `Once`, and it
            // runs before any test has read these variables (nothing else in this file touches
            // them).
            unsafe { std::env::set_var(var, &dir) };
        }
    });
}

/// The cgroup named for `pane`, out of every leaf the tree holds.
fn leaf_of(leaves: &[std::path::PathBuf], pane: sprag_terminal::PaneId) -> &std::path::Path {
    let wanted = format!("pane-{}", pane.0);
    leaves
        .iter()
        .find(|leaf| leaf.file_name().is_some_and(|name| name == wanted.as_str()))
        .unwrap_or_else(|| panic!("pane {} has no cgroup among {leaves:?}", pane.0))
}

/// Every `session-*/window-*/pane-*` directory under `root`.
fn pane_leaves(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let level = |parent: &std::path::Path, prefix: &str| -> Vec<std::path::PathBuf> {
        std::fs::read_dir(parent)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_dir()
                    && path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with(prefix))
            })
            .collect()
    };
    level(root, "session-")
        .iter()
        .flat_map(|session| level(session, "window-"))
        .flat_map(|window| level(&window, "pane-"))
        .collect()
}

/// The process this test delegates, and the unit it ends up in — both torn down on the way out,
/// panic or not.
struct Holder {
    child: Child,
    unit: Option<String>,
}

impl Holder {
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
