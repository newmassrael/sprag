//! WHAT THE DOCK'S LAUNCHER DOES — register item 825, held as a claim instead of a habit.
//!
//! # ⛔⛔⛔⛔⛔ The defect this gate exists for, measured
//!
//! The owner pressed the dock icon six times over six days and no window ever opened. Each press
//! ran `crates/sprag-gui/desktop/sprag-gui-launch`, which asked the sibling client with no socket
//! named — so the client resolved the WELL-KNOWN DEFAULT, found the file a daemon had left behind
//! on 2026-08-25, and put *"no server running at `/run/user/1000/sprag-host.sock`"* on screen while
//! a daemon served six windows on `/run/user/1000/sprag-loop.sock`. **Every word of it was true.**
//!
//! # ⚠⚠⚠⚠⚠ Why the claim lives HERE and not in a test beside the GUI
//!
//! This crate's charter is *the gates a test cannot be*, and the launcher is the shape that charter
//! was written for one turn further out: **it is not compiled by anything.** No `cargo build`
//! reaches it, no crate imports it, and for two weeks the only other artifact in its family —
//! `sprag (loop).desktop`, which register item 285 recorded as the workaround that *"only gets to
//! the door"* — **disappeared from this machine with nothing anywhere going red.** A file no
//! compiler reads and no test runs is a file whose deletion is silent, and that silence is half of
//! why item 825 took six days to be noticed.
//!
//! So the script became a TRACKED artifact of the repository and this module runs it. A launcher
//! that stops asking which daemons are running now fails a gate rather than a person.
//!
//! # ⚠⚠⚠ What is staged, and what is deliberately NOT
//!
//! The stand-ins play back a survey; they do not compute one. That split is the honest one:
//!
//! * **This module's claim** is *the launcher asks its build's own client which daemons are
//!   serving, points the GUI at the answer, and shows the survey verbatim when there is none.*
//! * **Whether the survey is CORRECT** is `sprag_rpc::survey`'s claim, gated in that module's own
//!   tests and, against a real daemon on a socket nobody named, by
//!   `a_daemon_on_a_socket_nobody_named_is_found_by_the_survey` in `sprag-host`'s CLI suite.
//!
//! A gate here that booted a real daemon would be re-asserting the second claim and would still not
//! have made the first — the launcher could pass it while ignoring the answer entirely.
//!
//! ⚠ Those two are named in prose rather than linked because this crate declares NO dependencies —
//! deliberately, so a gate cannot fail to build when the product does.

use std::path::PathBuf;

/// Where the tracked launcher lives, relative to the workspace root.
///
/// ⚠ Beside the GUI it launches rather than in a `scripts/` directory of its own: the one job it
/// has that the product cannot do for itself is choosing among **this tree's builds of
/// `sprag-gui`**, so it belongs to that crate the way a `build.rs` does.
pub const LAUNCHER: &str = "crates/sprag-gui/desktop/sprag-gui-launch";

/// The launcher, as the running tree holds it.
///
/// # Panics
///
/// When the file is missing or carries no execute bit. Both are the failure this module was
/// written for — an artifact nothing compiles, gone with nobody told — so they are LOUD rather
/// than a skipped case.
#[must_use]
pub fn launcher_path() -> PathBuf {
    let path = crate::sources::workspace_root().join(LAUNCHER);
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(&path)
            .unwrap_or_else(|why| {
                panic!(
                    "⛔ REGISTER ITEM 825: the dock's launcher must be in this tree — {} — {why}. \
                     It is the artifact nothing compiles, and the last one in its family left \
                     without a word (item 285's `sprag (loop).desktop`).",
                    path.display(),
                )
            })
            .permissions()
            .mode()
    };
    assert!(
        mode & 0o111 != 0,
        "⚠⚠⚠ THE LAUNCHER MUST BE EXECUTABLE ({mode:o}): {}. A dock entry pointing at a file \
         without the bit is the window that never appears with no message anywhere the clicker \
         will look — the exact face item 825 wears.",
        path.display(),
    );
    path
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crate::doubles::Doubles;

    /// One staged machine: a repository whose `target/` holds builds, and the files a run leaves.
    struct Fixture {
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    impl Fixture {
        /// A scratch tree of this CASE's own — named for the case, so a leak says which.
        ///
        /// ⚠ `sprag_scratch::scratch_root()` rather than `std::env::temp_dir()` — register item
        /// 794: the bare call answers a RELATIVE path when `TMPDIR` is set-and-empty, and a
        /// fixture that staged a whole `target/` tree there would build it inside this crate's own
        /// directory in the repository. This workspace ratchets the number of sites that still
        /// bypass it, so the import is what keeps a new fixture from adding one.
        fn new(case: &str) -> Self {
            let root = sprag_scratch::scratch_root().join(format!(
                "sprag-gate-gui-launch-{}-{case}",
                std::process::id(),
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a scratch tree");
            Self { root }
        }

        /// Stage one build of the GUI and its sibling client under `target/<profile>/`.
        ///
        /// ⚠⚠ THE PROGRAMS ARE SYMLINKS to the tracked doubles, never copies and never written
        /// here — register item 467: a file this process wrote is a file `execve` can refuse with
        /// `ETXTBSY` while a sibling case happens to hold the handle, and that race reads as a
        /// flake for months.
        ///
        /// `serving` and `survey` are the TAPE, and staging them is not writing a program.
        fn build(&self, profile: &str, tape: &[(&str, &str)]) -> PathBuf {
            let dir = self.root.join("target").join(profile);
            std::fs::create_dir_all(&dir).expect("a staged build directory");
            let doubles = Doubles::of(env!("CARGO_MANIFEST_DIR")).set("gui-launch");
            for program in ["sprag", "sprag-gui"] {
                std::os::unix::fs::symlink(doubles.program(program), dir.join(program))
                    .expect("link the tracked double into the staged build");
            }
            for (name, body) in tape {
                std::fs::write(dir.join(name), body).expect("stage the tape");
            }
            dir
        }

        /// Run the launcher against this staged machine, with `env` on top.
        fn launch(&self, env: &[(&str, &str)]) -> Run {
            let record = self.root.join("gui-record");
            let asked = self.root.join("client-record");
            let shown = self.root.join("shown");
            let log = self.root.join("gui-launch.log");
            let status = Command::new(launcher_path())
                .env("SPRAG_REPO", &self.root)
                .env("SPRAG_GUI_LAUNCH_LOG", &log)
                // ⚠ NO `notify-send`: the notification is the product of the failing path, so it
                // is captured as a file rather than fired at whatever daemon the runner happens to
                // have. A gate that needed a desktop session would be green on nobody's machine.
                .env("SPRAG_GUI_LAUNCH_NOTIFY", &shown)
                .env("SPRAG_GATE_GUI_RECORD", &record)
                .env("SPRAG_GATE_CLIENT_RECORD", &asked)
                // ⚠⚠ THE RUNNER'S OWN SESSION MUST NOT LEAK IN. This suite is run from a shell
                // that may itself be inside a sprag pane, and a pane exports `SPRAG_HOST_RPC_SOCK`
                // — which is one of the two variables the launcher treats as *somebody named an
                // endpoint*. Left alone, every case here would take the named branch by
                // inheritance and none would test the survey at all.
                .env_remove("SPRAG_HOST_RPC_SOCK")
                .env_remove("SPRAG_GUI_HOST_SOCK")
                .envs(env.iter().copied())
                .status()
                .expect("run the launcher");
            Run {
                ok: status.success(),
                gui: read(&record),
                asked: read(&asked),
                shown: read(&shown),
            }
        }
    }

    /// A file that may not exist — an absent record is *the program never ran*, which is a claim
    /// several cases make, so it is an empty string rather than an error.
    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap_or_default()
    }

    /// What one launcher run did.
    struct Run {
        ok: bool,
        /// What the GUI double recorded — empty when no GUI ran.
        gui: String,
        /// Every question the launcher put to a client, one per line, prefixed by the build.
        asked: String,
        /// What would have been on screen.
        shown: String,
    }

    impl Run {
        /// The endpoint the GUI was launched with, by variable name.
        fn endpoint(&self) -> BTreeMap<&str, &str> {
            self.gui
                .lines()
                .filter_map(|line| line.split_once('='))
                .collect()
        }
    }

    /// The survey a client prints when one daemon is alive on a socket nobody named — this
    /// machine's own answer on 2026-09-02, kept verbatim so the tape is a measurement.
    const LIVE: &str = "\
/run/user/1000/sprag-gui.sock  silent — nothing is listening; the file is what a daemon left behind
/run/user/1000/sprag-host.sock  silent — nothing is listening; the file is what a daemon left behind
/run/user/1000/sprag-loop-gui.sock  refused — something is listening and would not talk: client/hello: host rpc error: client/hello
/run/user/1000/sprag-loop.sock  serving — a daemon answered and speaks this build's wire
asked 4 socket(s) matching sprag*.sock under /run/user/1000
";

    /// The same machine with the daemon gone: three sockets and not one of them serving.
    const DEAD: &str = "\
/run/user/1000/sprag-gui.sock  silent — nothing is listening; the file is what a daemon left behind
/run/user/1000/sprag-host.sock  silent — nothing is listening; the file is what a daemon left behind
/run/user/1000/sprag-loop-gui.sock  refused — something is listening and would not talk: client/hello: host rpc error: client/hello
asked 3 socket(s) matching sprag*.sock under /run/user/1000
";

    /// ⛔⛔⛔⛔⛔ **THE DAEMON THAT IS RUNNING IS THE ONE THE GUI IS POINTED AT** — register item
    /// 825's whole sentence, and the case the owner pressed six times.
    ///
    /// The staged machine is the one that was measured: the well-known socket is a file a dead
    /// daemon left behind, and the live daemon is on `sprag-loop.sock`, which no variable names and
    /// no default reaches. A launcher that still resolved by default would run the GUI with **no**
    /// endpoint named and it would open against the dead file — which is the failure, one step
    /// later and with a panic instead of a notification.
    #[test]
    fn the_gui_is_pointed_at_the_daemon_that_is_running_and_not_at_the_default_socket() {
        let fixture = Fixture::new("running");
        fixture.build(
            "debug",
            &[
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );

        let run = fixture.launch(&[]);

        assert!(run.ok, "the launcher refused a machine with a daemon on it");
        assert_eq!(
            run.endpoint().get("SPRAG_GUI_HOST_SOCK").copied(),
            Some("/run/user/1000/sprag-loop.sock"),
            "⛔ ITEM 825: the GUI must be launched AT the socket the survey said was serving. \
             {:?} is what it actually got, and an unset endpoint here is not a smaller failure \
             than a wrong one — it is the original defect: the display client falls back to the \
             well-known default, which on the measured machine was a file with no daemon behind \
             it since 2026-08-25",
            run.gui,
        );
        assert!(
            run.asked.contains("daemons --serving"),
            "⚠⚠ AND IT MUST HAVE ASKED. A launcher that reached the right socket without putting \
             the question would be one hardcoded path away from the same defect — the register \
             item is explicit that a second path is not the fix. Asked: {:?}",
            run.asked,
        );
    }

    /// ⛔⛔⛔⛔ **A BUILD THAT CANNOT REACH A DAEMON IS PASSED OVER FOR ONE THAT CAN** — the
    /// launcher's ORIGINAL purpose, which the new question must not have replaced.
    ///
    /// The two claims are one act: `daemons` connects and handshakes on every socket, so a build
    /// whose wire the daemon refuses reports every socket `refused` and serves nothing. That is why
    /// there is no second probe here for *is this build too old* — the survey already answers it,
    /// on the same connect, and a separate check would be a second authority on one fact.
    #[test]
    fn a_build_whose_wire_no_daemon_speaks_is_passed_over_for_one_that_serves() {
        let fixture = Fixture::new("skew");
        // ⚠ NEWEST FIRST is by mtime, and the launcher's pick is `ls -t`. `newer` is staged second
        // so it really is the newer file — the property being claimed is *the newest that fits*, so
        // the fixture must MAKE "newest" true rather than assume it.
        //
        // ⚠⚠ THE PAUSE IS MEASURED, NOT SUPERSTITION. `ls -t` reads the SYMLINK's own mtime
        // (measured 2026-09-02: staging order decides, in both orders), and the kernel stamps it
        // from a coarse clock — two links made back to back tied 5 times out of 5 and `ls` then
        // ordered them by name, which would have made this case pass or fail on the alphabet.
        // 50 ms separated them 5 times out of 5.
        let older = fixture.build(
            "release",
            &[
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        let newer = fixture.build("debug", &[("survey", DEAD)]);

        let run = fixture.launch(&[]);

        assert!(
            run.ok,
            "a fitting build was staged and the launcher refused"
        );
        let endpoint = run.endpoint();
        assert_eq!(
            endpoint.get("SPRAG_GUI_HOST_SOCK").copied(),
            Some("/run/user/1000/sprag-loop.sock"),
        );
        assert!(
            run.gui
                .contains(&format!("ran {}", older.join("sprag-gui").display())),
            "⛔ the OLDER build is the one that serves, so it is the one that must run — {:?}",
            run.gui,
        );
        assert!(
            !run.gui
                .contains(&format!("ran {}", newer.join("sprag-gui").display())),
            "⛔⛔ and the newer one must NOT have been launched: a GUI whose wire is refused \
             panics on `client/hello` with no window and no message, which is the failure this \
             script was written for before item 825 widened it",
        );
        assert!(
            run.asked.starts_with(&format!("{}", newer.display())),
            "⚠ the newest build is asked FIRST — the mtime pick is what keeps a rebuild current \
             with no edit anywhere. Asked: {:?}",
            run.asked,
        );
    }

    /// ⛔⛔⛔⛔⛔ **WHAT A PERSON IS TOLD WHEN NOTHING SERVES IS THE SURVEY, NOT THE DEFAULT
    /// SOCKET** — item 825's second requirement, and the one a fix that only changed the
    /// connection would have left standing.
    ///
    /// > ⚠ 그리고 거절 문장이 「기본 소켓에 없다」로 끝나면 안 된다 — a daemon living elsewhere
    /// > makes that sentence true and its reader wrong.
    ///
    /// So this asserts the three WORDS are on screen. Each is a different repair, and the old
    /// notification could spell only one of them.
    #[test]
    fn a_machine_with_no_daemon_is_shown_every_sockets_word_and_where_it_looked() {
        let fixture = Fixture::new("none");
        fixture.build("debug", &[("survey", DEAD)]);

        let run = fixture.launch(&[]);

        assert!(!run.ok, "no daemon serves, so the launcher must refuse");
        assert!(
            run.gui.is_empty(),
            "⛔ no GUI may be launched at a machine with no daemon on it — {:?}",
            run.gui,
        );
        for word in ["silent", "refused"] {
            assert!(
                run.shown.contains(word),
                "⛔ ITEM 825: {word:?} must be on screen. The sentence it replaces named ONE \
                 socket and one problem; the owner's machine had a file a dead daemon left behind \
                 AND a socket another program owns, and those are two different afternoons. \
                 Shown: {:?}",
                run.shown,
            );
        }
        assert!(
            run.shown
                .contains("asked 3 socket(s) matching sprag*.sock under /run/user/1000"),
            "⚠⚠ AND WHERE IT LOOKED. A reader told *no daemon* cannot otherwise tell that from \
             *it did not look where mine is* — an operator who pointed the socket outside this \
             product's naming is not asked about at all, and the population line is the only \
             thing that says so. Shown: {:?}",
            run.shown,
        );
        assert!(
            !run.shown.contains("the well-known default"),
            "⛔⛔⛔ AND THE OLD SENTENCE MUST BE GONE. *no server running at the well-known \
             default* was TRUE on the measured machine and sent its reader to the wrong socket six \
             times — a refusal that is accurate and misdirecting is the defect, not a smaller one. \
             Shown: {:?}",
            run.shown,
        );
    }

    /// ⚠⚠⚠ **AN ENDPOINT SOMEBODY NAMED IS NOT REPLACED BY A SURVEY.**
    ///
    /// A GUI launched from inside a pane belongs to the daemon that owns the pane — `$TMUX`
    /// semantics, which `sprag_rpc::endpoint` spells as the client's precedence. A launcher that
    /// surveyed anyway could hand the GUI a DIFFERENT daemon than the one its own resolution would
    /// have chosen, and `endpoint.rs` records what that costs: a probe whose client and daemon
    /// disagreed drove the machine's live daemon for an afternoon with nothing able to say so.
    #[test]
    fn an_endpoint_the_environment_named_is_left_alone() {
        let fixture = Fixture::new("named");
        fixture.build(
            "debug",
            &[
                ("ls-ok", ""),
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );

        let run = fixture.launch(&[("SPRAG_HOST_RPC_SOCK", "/run/user/1000/somebody-said.sock")]);

        assert!(
            run.ok,
            "the named endpoint answered and the launcher refused"
        );
        let endpoint = run.endpoint();
        assert_eq!(
            endpoint.get("SPRAG_HOST_RPC_SOCK").copied(),
            Some("/run/user/1000/somebody-said.sock"),
            "the endpoint the environment named must survive into the GUI",
        );
        assert_eq!(
            endpoint.get("SPRAG_GUI_HOST_SOCK").copied(),
            Some("<unset>"),
            "⛔⛔ AND THE LAUNCHER MUST NOT OVERRIDE IT. `SPRAG_GUI_HOST_SOCK` wins over the \
             variable a pane exports, so writing one here would silently move a pane's own GUI to \
             whichever daemon a survey happened to list first — {:?}",
            run.gui,
        );
        assert!(
            !run.asked.contains("daemons"),
            "⚠ and the survey is not even PUT when the question is already answered: asking and \
             discarding would leave the next reader unable to tell which answer the launcher \
             acted on. Asked: {:?}",
            run.asked,
        );
    }

    /// ⚠⚠⚠⚠ **TWO DAEMONS IS A CHOICE, AND A DOCK CLICK CANNOT ASK** — so the pick is stated
    /// rather than silent.
    ///
    /// This machine holds a second daemon by design (the debt-repayment loop runs its own), and the
    /// register's own measurement found four candidate sockets. Opening the first in path order is
    /// the only answer a click can give — but a person who gets a window they did not expect must
    /// be able to find out why, and be told how to reach the other. An arbitrary pick made in
    /// silence is the same class of defect as a true sentence pointing at the wrong socket.
    #[test]
    fn more_than_one_serving_daemon_is_named_on_screen_rather_than_chosen_in_silence() {
        let fixture = Fixture::new("two");
        fixture.build(
            "debug",
            &[
                ("survey", LIVE),
                (
                    "serving",
                    "/run/user/1000/sprag-host.sock\n/run/user/1000/sprag-loop.sock\n",
                ),
            ],
        );

        let run = fixture.launch(&[]);

        assert!(run.ok, "a window must still open");
        assert_eq!(
            run.endpoint().get("SPRAG_GUI_HOST_SOCK").copied(),
            Some("/run/user/1000/sprag-host.sock"),
            "the FIRST in path order, so two clicks open the same daemon",
        );
        assert!(
            run.shown.contains("/run/user/1000/sprag-loop.sock")
                && run.shown.contains("SPRAG_GUI_HOST_SOCK=<socket>"),
            "⚠⚠ the other daemon must be named, and so must the way to open it — {:?}",
            run.shown,
        );
    }

    /// ⚠⚠ **A TREE WITH NO GUI BUILT SAYS SO, AND SAYS NOTHING ABOUT DAEMONS.**
    ///
    /// The control on every case above: they all prove the launcher reached a client, so one of
    /// them passing because the script fell over early would look the same. Here nothing is staged
    /// at all, and the message has to be about the BUILD — the one problem in this script's family
    /// that a survey cannot diagnose, because there is no client to put the question to.
    #[test]
    fn a_tree_with_no_build_is_told_to_build_rather_than_told_about_sockets() {
        let fixture = Fixture::new("nobuild");
        std::fs::create_dir_all(fixture.root.join("target")).expect("an empty target directory");

        let run = fixture.launch(&[]);

        assert!(!run.ok);
        assert!(
            run.shown.contains("cargo build -p sprag-gui"),
            "the repair is a build, and it is the one this script can name — {:?}",
            run.shown,
        );
        assert!(
            !run.shown.contains("socket"),
            "⚠ and a tree with no client must not be reported as a machine with no daemon: \
             those are two repairs, and guessing between them is what item 825 is about — {:?}",
            run.shown,
        );
    }
}
