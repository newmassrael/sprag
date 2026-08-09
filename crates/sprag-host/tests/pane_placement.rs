//! A pane born through the host lands in the cgroup its identities name.
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
//! Skips, saying so, where systemd will not delegate.

#![cfg(target_os = "linux")]

use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use sprag_host::delegation;
use sprag_host::host::{Host, HostClient};
use sprag_terminal::{CommandBuilder, PaneBirthHooks, Tree};

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
