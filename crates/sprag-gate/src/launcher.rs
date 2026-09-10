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
//! # ⛔⛔⛔⛔ And the same defect again, on the other noun — register item 963
//!
//! Six days later the icon stopped opening again, and the survey this gate had bought was **right
//! and still not enough**: `refused — this daemon speaks wire protocol 45 and the client speaks
//! 46`. Ordinary rounds bump the wire, so both builds under `target/` were 46; the daemon serving
//! four windows had been started from the PROMOTED copy and still spoke 45. A build that fitted
//! was on the machine, and the launcher's candidate population — `$REPO/target/*`, and nothing
//! else — could not see it.
//!
//! ⇒ **825 was *which socket*; 963 is *which builds*.** The lesson each cost is one sentence: a
//! launcher that hardcodes either half of its own question fails the day that half changes, and
//! fails as a window that never appears.
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
/// has that the product cannot do for itself is choosing among **this machine's builds of
/// `sprag-gui`**, so it belongs to that crate the way a `build.rs` does.
///
/// ⚠⚠ *This machine's* and no longer *this tree's* — register item 963 widened the population to
/// the promoted copy, because the build that fits a daemon this tree has moved past is the one
/// that was promoted to it. The sentence above said *tree* for six days while the script meant it,
/// and the day it stopped being true is the day the dock stopped opening.
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
            let root = sprag_scratch::scratch_for("sprag-gate-gui-launch", case);
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("a scratch tree");
            Self { root }
        }

        /// Stage one of THIS TREE's builds, under `target/<profile>/`.
        fn build(&self, profile: &str, tape: &[(&str, &str)]) -> PathBuf {
            self.stage(self.root.join("target").join(profile), tape)
        }

        /// Stage the PROMOTED build — the copy this repository puts in front of the loop.
        ///
        /// ⚠⚠⚠ THE DIRECTORY IS READ FROM THE TOOL THAT OWNS IT (`promoted_dir`) rather than
        /// retyped here. `promotion.rs` says why in its own words — *a second spelling is how a
        /// fifth binary gets built, promoted and never asked* — and a fixture that spelled the
        /// path itself would go on staging the old place after somebody moved it, staying green
        /// over a launcher that had stopped finding anything.
        fn promote(&self, tape: &[(&str, &str)]) -> PathBuf {
            self.stage(self.home().join(promoted_dir()), tape)
        }

        /// The `$HOME` the launcher runs with, which is this fixture's and never the runner's.
        fn home(&self) -> PathBuf {
            self.root.join("home")
        }

        /// Put the doubles and a tape in one directory and hand it back.
        ///
        /// ⚠⚠ THE PROGRAMS ARE SYMLINKS to the tracked doubles, never copies and never written
        /// here — register item 467: a file this process wrote is a file `execve` can refuse with
        /// `ETXTBSY` while a sibling case happens to hold the handle, and that race reads as a
        /// flake for months.
        ///
        /// `serving` and `survey` are the TAPE, and staging them is not writing a program.
        fn stage(&self, dir: PathBuf, tape: &[(&str, &str)]) -> PathBuf {
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
                // ⛔⛔⛔⛔⛔ AND NEITHER MAY THE RUNNER'S HOME. Register item 963 made `$HOME` part
                // of the launcher's build population — the promoted copy lives under it — so a
                // fixture that left this alone would put THIS MACHINE's promoted `sprag-gui` into
                // every case below, and the developer's own machine has one. Every case here
                // would then be answering about a build the case never staged, and the two that
                // claim *nothing fitted* would be green or red by whatever the loop last promoted.
                .env("HOME", self.home())
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

    /// ⛔⛔⛔ **THE SURVEY OF 2026-09-08T05:50Z, VERBATIM** — register item 963's own measurement,
    /// and the difference from [`DEAD`] is the whole point: a daemon IS there, it is serving four
    /// windows, and this tree simply cannot speak to it any more.
    const AHEAD: &str = "\
/run/user/1000/sprag-gui.sock  silent — nothing is listening; the file is what a daemon left behind
/run/user/1000/sprag-loop-gui.sock  silent — nothing is listening; the file is what a daemon left behind
/run/user/1000/sprag-loop.sock  refused — something is listening and would not talk: client/hello: host rpc error: this daemon speaks wire protocol 45 and the client speaks 46; they cannot understand each other. Rebuild the client, or restart this daemon to the client's build — `sprag kill-server` (sessions are restored from the durability snapshot)
asked 3 socket(s) matching sprag*.sock under /run/user/1000
";

    /// The file that owns the name of the directory a promotion writes into.
    const PROMOTABLE: &str = "crates/sprag-host/src/bin/sprag-promotable.rs";

    /// Where this repository promotes its images, READ from the tool that owns the name.
    ///
    /// ⚠ Read as TEXT rather than imported: this crate declares no dependencies, deliberately, so
    /// that a gate cannot fail to build when the product does.
    fn promoted_dir() -> String {
        let path = crate::sources::workspace_root().join(PROMOTABLE);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|why| {
            panic!(
                "⛔ REGISTER ITEM 963: {} is the one authority on where a promotion puts its \
                 images, and both this gate and the dock's launcher answer from it — {why}",
                path.display(),
            )
        });
        let line = text
            .lines()
            .find(|line| line.trim_start().starts_with("const PROMOTED:"))
            .unwrap_or_else(|| {
                panic!(
                    "⛔⛔ REGISTER ITEM 963: {PROMOTABLE} no longer declares `const PROMOTED`. \
                     Wherever that name went, the launcher's population has to follow it — a dock \
                     looking in a directory nothing promotes into is item 963 again"
                )
            });
        line.split('"')
            .nth(1)
            .unwrap_or_else(|| {
                panic!("⛔ `const PROMOTED` is no longer a plain string literal: {line:?}")
            })
            .to_string()
    }

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
        // ⚠ NEWEST FIRST is by mtime, and the launcher's pick is `ls -td`. `newer` is staged second
        // so it really is the newer file — the property being claimed is *the newest that fits*, so
        // the fixture must MAKE "newest" true rather than assume it.
        //
        // ⚠⚠ THE PAUSE IS MEASURED, NOT SUPERSTITION. `ls -td` reads the SYMLINK's own mtime
        // (measured 2026-09-02: staging order decides, in both orders), and the kernel stamps it
        // from a coarse clock — two links made back to back tied 5 times out of 5 and `ls` then
        // ordered them by name, which would have made this case pass or fail on the alphabet.
        // 50 ms separated them 5 times out of 5.
        //
        // ⛔⛔⛔⛔⛔ AND THAT 2026-09-02 SENTENCE WAS TRUE ONLY OF GNU — register item 970. BSD `ls`
        // follows a symlink named on its command line unless `-l`/`-P`/`-d`/`-F`/`-i` is given, so
        // WITHOUT the `-d` the script now passes, every build staged here resolves to the ONE
        // tracked double, they tie, and the order is `strcoll` over the full path. This case was
        // GREEN on macOS anyway — `debug` sorts before `release` and the fixture stages `debug`
        // second, so the alphabet agreed with the clock — while two of its neighbours were red.
        // A gate green because of the alphabet is a gate that will red the day somebody renames a
        // profile. `the_build_order_is_the_same_where_ls_follows_a_link_on_its_command_line` is
        // where that reading is held, and it stages the pair the other way round for exactly this
        // reason.
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

    /// ⛔⛔⛔⛔⛔ **THE BUILD THAT FITS MAY NOT BE ONE OF THIS TREE'S** — register item 963, and
    /// the afternoon of 2026-09-08.
    ///
    /// # 📊 What was measured
    ///
    /// The owner pressed the icon and no window opened. `sprag daemons` from this tree answered
    /// [`AHEAD`] — `refused — this daemon speaks wire protocol 45 and the client speaks 46` on the
    /// only live socket — and **both** of `target/debug` and `target/release` were 46, because
    /// ordinary rounds bump the wire. The daemon serving four windows had been started on
    /// 2026-09-05 from `~/.local/share/sprag-loop/bin/sprag-term`, and the GUI promoted beside it
    /// still spoke 45.
    ///
    /// **So a build that fitted was on the machine and the launcher could not see it**: its
    /// population was `$REPO/target/*` and nothing else. That is item 825's defect one noun over —
    /// the script had never asked which BUILDS were candidates, only which SOCKET — and it wears
    /// the same face: a window that never appears, with a true sentence for the reason.
    #[test]
    fn a_daemon_this_tree_has_moved_past_is_opened_by_the_build_that_was_promoted_to_it() {
        let fixture = Fixture::new("promoted");
        // ⚠ OLDEST, which is what it is in life: a promotion is a copy of a build this tree has
        // since replaced. The 50 ms is the same measured pause the skew case above explains.
        let promoted = fixture.promote(&[
            ("survey", LIVE),
            ("serving", "/run/user/1000/sprag-loop.sock\n"),
        ]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        fixture.build("release", &[("survey", AHEAD)]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let newest = fixture.build("debug", &[("survey", AHEAD)]);

        let run = fixture.launch(&[]);

        assert!(
            run.ok,
            "⛔ ITEM 963: a daemon is serving and a build on this machine can talk to it, so a \
             window must open. Shown instead: {:?}",
            run.shown,
        );
        assert_eq!(
            run.endpoint().get("SPRAG_GUI_HOST_SOCK").copied(),
            Some("/run/user/1000/sprag-loop.sock"),
            "and at the socket the fitting build's own survey named — {:?}",
            run.gui,
        );
        assert!(
            run.gui
                .contains(&format!("ran {}", promoted.join("sprag-gui").display())),
            "⛔⛔ THE PROMOTED BUILD IS THE ONE THAT RUNS, because it is the only one whose wire \
             the daemon speaks. A launcher that reached this line having run something else is \
             back at `client/hello` with a panic and no window — {:?}",
            run.gui,
        );
        assert!(
            run.asked.starts_with(&format!("{}", newest.display())),
            "⚠⚠ AND THIS TREE IS ASKED FIRST. The promoted copy is a FALLBACK: it is reached by \
             mtime order, so a rebuild that can talk still wins with no edit anywhere. Asked: {:?}",
            run.asked,
        );
        assert!(
            run.shown.contains(&format!("{}", promoted.display())),
            "⚠⚠⚠ AND THE PERSON IS TOLD WHICH BUILD THEY GOT. A window opened in silence from a \
             build this tree has moved past teaches nothing: the skew that produced the click \
             survives the click, and tomorrow's press is the same puzzlement. The multi-daemon \
             case below sets the precedent — open it, and NAME the choice. Shown: {:?}",
            run.shown,
        );
    }

    /// ⛔⛔⛔⛔⛔ **A PROMOTED COPY IS STILL A FALLBACK WHEN IT IS THE NEWER FILE** — register item
    /// 970, and the arm `a_build_of_this_tree_that_fits_is_preferred_to_the_promoted_copy` could
    /// not reach.
    ///
    /// # 📊 What was measured, on this machine, with one flat `ls -t`
    ///
    /// ```text
    /// target/debug/sprag-gui                    2026-09-10 20:24
    /// ~/.local/share/sprag-loop/bin/sprag-gui   2026-09-09 13:15   ← the promoted copy, in the MIDDLE
    /// target/release/sprag-gui                  2026-09-08 21:37
    /// ```
    ///
    /// The script's own comment claimed *"a promoted copy is older than the build that superseded
    /// it, so `ls -t` asks `target/` FIRST"*. **That was an accident dressed as a mechanism**, and
    /// on this machine it was already false: `target/release` is a build of this tree and it was
    /// asked AFTER the promoted copy. In the window between a promotion and the next build the
    /// promoted copy is newer than EVERY profile, so it is asked first — the fallback becomes the
    /// preference, silently, and the dock opens a build this tree has moved past while a build of
    /// this tree fits. That is item 963's own defect, produced by item 963's own fix.
    ///
    /// ⚠⚠ **THE OLDER CONTROL CANNOT SEE IT.** It stages the promoted copy first and the tree's
    /// build second, so the tree's build is the newer file and wins for the wrong reason: an mtime
    /// accident, not the requirement. This case makes the promoted copy the NEWER one — which is
    /// what a promotion actually leaves behind — and asks the same question.
    #[test]
    fn a_promoted_copy_newer_than_this_trees_build_is_still_only_a_fallback() {
        let fixture = Fixture::new("newer-promotion");
        // ⚠ THE TREE'S BUILD FIRST, so the promoted copy is the NEWER file — the order a promotion
        // really leaves. The 50 ms is the same measured pause the skew case explains.
        let mine = fixture.build(
            "debug",
            &[
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        let promoted = fixture.promote(&[
            ("survey", LIVE),
            ("serving", "/run/user/1000/sprag-loop.sock\n"),
        ]);

        let run = fixture.launch(&[]);

        assert!(run.ok, "both builds fit, so a window must open");
        assert!(
            run.asked.starts_with(&format!("{}", mine.display())),
            "⛔⛔⛔ REGISTER ITEM 970: THIS TREE IS ASKED FIRST WHATEVER THE CLOCK SAYS. The \
             promoted copy is the newer file here — which is what every promotion leaves until the \
             next build — and a launcher ordering one flat list by mtime asks it first. *This tree \
             first* is a requirement and cannot be left to a timestamp. Asked: {:?}",
            run.asked,
        );
        assert!(
            run.gui
                .contains(&format!("ran {}", mine.join("sprag-gui").display())),
            "⛔⛔ and this tree's build is the one that RUNS — a dock frozen at the last promotion \
             is a repository whose fixes never reach the person clicking it: {:?}",
            run.gui,
        );
        assert!(
            !run.gui
                .contains(&format!("ran {}", promoted.join("sprag-gui").display())),
            "⛔⛔⛔ AND THE PROMOTED COPY MUST NOT HAVE RUN. It fits, and it is the newer file, so \
             an order that reads the clock alone hands the dock a build this tree has moved past \
             on every press between a promotion and the next build: {:?}",
            run.gui,
        );
        assert!(
            run.shown.is_empty(),
            "⚠⚠ AND NOTHING IS SHOWN: the window is this tree's build, so the note item 963 adds \
             for the fallback must not fire. Shown: {:?}",
            run.shown,
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND THE ORDER IS THE SAME WHERE `ls` READS A LINK THE OTHER WAY** — register
    /// item 970, and the macOS runner reproduced on this machine.
    ///
    /// # 📊 The divergence, measured on both readings
    ///
    /// Read out of the implementation macOS actually ships — `apple-oss-distributions/file_cmds`,
    /// not remembered:
    ///
    /// ```text
    /// ls.c   if (!f_nofollow && !f_longform && !f_listdir && (!f_type || f_slash) && !f_inode …)
    ///                fts_options |= FTS_COMFOLLOW;        /* `case 'd': f_listdir = 1;` */
    /// cmp.c  modcmp(): … tv_sec, then tv_nsec, then `strcoll(a->fts_name, b->fts_name)`
    /// ```
    ///
    /// So `-t` alone sorts by the link TARGET's mtime while GNU `ls` uses the link's own, and a tie
    /// is broken by NAME — which for a root-level operand is the whole path as given. Every build
    /// these gates stage is a symlink to ONE tracked double (item 467), so under that reading they
    /// all resolve to one file and every candidate ties.
    ///
    /// Reproduced here, same three links, same instant:
    ///
    /// ```text
    /// ls -tL  →  home/.local/…/sprag-gui   target/debug/sprag-gui   target/release/sprag-gui
    /// ls -t   →  target/release/sprag-gui  target/debug/sprag-gui   home/.local/…/sprag-gui
    /// ```
    ///
    /// That is byte for byte what the macOS runner did on 2026-09-10 (run 34465283158), and it is
    /// why the two cases naming the promoted copy were red there and here green.
    ///
    /// # ⚠⚠⚠ Why this case stages `release` as the NEWEST
    ///
    /// Under the tie the order is the ALPHABET, and `debug` < `release`. Every other case in this
    /// file stages `debug` last, so the alphabet agrees with the clock and a tie is invisible —
    /// which is exactly why `a_build_whose_wire_no_daemon_speaks_is_passed_over_for_one_that_serves`
    /// was GREEN on macOS while two of its neighbours were red. Reversing the pair is what makes
    /// the two readings disagree, and it is the only arrangement in which this gate says anything.
    #[test]
    fn the_build_order_is_the_same_where_ls_follows_a_link_on_its_command_line() {
        let fixture = Fixture::new("bsd-ls");
        let promoted = fixture.promote(&[
            ("survey", LIVE),
            ("serving", "/run/user/1000/sprag-loop.sock\n"),
        ]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let older = fixture.build(
            "debug",
            &[
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
        let newest = fixture.build(
            "release",
            &[
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );

        // ⚠⚠ THE DOUBLE EMULATES BSD'S RULE, NOT ITS OUTCOME — see the file itself. One that always
        // dereferenced would defeat the repair it exists to measure, because `-d` is what suppresses
        // `FTS_COMFOLLOW` over there.
        let doubles = Doubles::of(env!("CARGO_MANIFEST_DIR")).set("gui-launch");
        let path = doubles.ahead_of_inherited();
        let run = fixture.launch(&[("PATH", &path.to_string_lossy())]);

        assert!(run.ok, "every build here fits, so a window must open");
        assert!(
            run.asked.starts_with(&format!("{}", newest.display())),
            "⛔⛔⛔ REGISTER ITEM 970: on a machine whose `ls` follows a link named on its command \
             line, every staged build resolves to ONE file, they tie, and the order becomes the \
             ALPHABET over full paths — under which `home/.local/…` beats `target/…` and `debug` \
             beats `release`. The launcher must not inherit that: its candidate order is a \
             requirement, not whichever `ls` the machine ships. Asked: {:?}",
            run.asked,
        );
        assert!(
            run.gui
                .contains(&format!("ran {}", newest.join("sprag-gui").display())),
            "⛔⛔ and the newest build of THIS TREE is the one that ran: {:?}",
            run.gui,
        );
        assert!(
            !run.asked.starts_with(&format!("{}", promoted.display())),
            "⛔⛔⛔⛔⛔ AND THE PROMOTED COPY IS NOT ASKED FIRST — this is the macOS red itself: \
             `home/.local/…` sorts ahead of `target/…`, so a tie handed the dock a build this tree \
             had moved past on every press. Asked: {:?}",
            run.asked,
        );
        assert!(
            !run.gui
                .contains(&format!("ran {}", older.join("sprag-gui").display())),
            "⚠⚠ and not the older profile either: the alphabet puts `debug` first and the clock \
             does not, which is the whole reason this case stages them this way round",
        );
    }

    /// ⚠⚠⚠⚠ **AND THE PROMOTED COPY IS A FALLBACK, NEVER A PREFERENCE** — the control on
    /// `a_daemon_this_tree_has_moved_past_is_opened_by_the_build_that_was_promoted_to_it`, which a
    /// widening that merely appended a path could pass while being wrong.
    ///
    /// Both fit here. The tree's build is newer, so it must be the one that runs — otherwise a
    /// promotion would freeze the dock at whatever was promoted, and every fix this repository
    /// makes would stop reaching the person who clicks. That is the launcher's ORIGINAL purpose,
    /// which item 963's widening must not have spent.
    ///
    /// ⚠⚠⚠ **AND IT WINS HERE FOR AN ACCIDENTAL REASON, WHICH IS WHY IT IS NOT THE WHOLE CLAIM** —
    /// register item 970. The tree's build is staged second, so it is the newer FILE, and a
    /// launcher ordering one flat list by mtime passes this while getting the requirement wrong.
    /// `a_promoted_copy_newer_than_this_trees_build_is_still_only_a_fallback` is the arm that makes
    /// the promoted copy the newer one — which is what a promotion actually leaves behind.
    ///
    /// ⚠ And nothing may be SHOWN. A note on an ordinary click is noise that teaches its reader to
    /// dismiss the one that matters, which is the note
    /// `a_daemon_this_tree_has_moved_past_is_opened_by_the_build_that_was_promoted_to_it` requires.
    ///
    /// ⚠⚠ **THIS CASE WAS GREEN BEFORE THE FIX AND THAT IS NOT A FAULT — it is what a control is.**
    /// Measured: with the population still `$REPO/target/*`, the promoted copy was not a candidate
    /// at all, so of course the tree's build won. It proves nothing about item 963's defect and
    /// everything about the repair not overshooting into *always use the promoted one*, which is
    /// the failure a widening reaches for next.
    #[test]
    fn a_build_of_this_tree_that_fits_is_preferred_to_the_promoted_copy() {
        let fixture = Fixture::new("prefers-tree");
        let promoted = fixture.promote(&[
            ("survey", LIVE),
            ("serving", "/run/user/1000/sprag-loop.sock\n"),
        ]);
        std::thread::sleep(std::time::Duration::from_millis(50));
        let mine = fixture.build(
            "debug",
            &[
                ("survey", LIVE),
                ("serving", "/run/user/1000/sprag-loop.sock\n"),
            ],
        );

        let run = fixture.launch(&[]);

        assert!(run.ok, "both builds fit, so a window must open");
        assert!(
            run.gui
                .contains(&format!("ran {}", mine.join("sprag-gui").display())),
            "⛔ THIS TREE'S BUILD MUST WIN WHEN IT CAN TALK — {:?}",
            run.gui,
        );
        assert!(
            !run.gui
                .contains(&format!("ran {}", promoted.join("sprag-gui").display())),
            "⛔⛔ and the promoted copy must not have run: a dock frozen at the last promotion is \
             a repository whose fixes never reach the person clicking it",
        );
        assert!(
            run.shown.is_empty(),
            "⚠⚠ NOTHING IS SHOWN ON AN ORDINARY CLICK. The note item 963 adds is for the case \
             where the window is NOT this tree's build; firing it every time would train its \
             reader to dismiss it. Shown: {:?}",
            run.shown,
        );
    }

    /// ⚠⚠⚠ **THE LAUNCHER AND THE TOOL THAT PROMOTES NAME ONE DIRECTORY** — register item 963,
    /// holding the rule `promotion.rs` states about its own list.
    ///
    /// > *a second spelling is how a fifth binary gets built, promoted and never asked*
    ///
    /// A shell script cannot import a Rust `const`, so the two can only be held together from
    /// outside — which is this crate's charter. Without this, moving `PROMOTED` would leave the
    /// dock looking in the old place with nothing anywhere going red, and the failure that
    /// produces is the silent one item 963 is about: no window, and no message either.
    ///
    /// ⚠ The COMMENTS are skipped, and that is load-bearing in the same way the bash-3 scan's
    /// skip is: the launcher's own paragraph names this directory while explaining it, so a scan
    /// that counted prose would pass a script that had stopped looking there.
    #[test]
    fn the_launcher_looks_where_this_repository_actually_promotes() {
        let promoted = promoted_dir();
        let text = std::fs::read_to_string(launcher_path()).expect("read the tracked launcher");
        let named = text
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .any(|line| line.contains(&promoted));

        assert!(
            named,
            "⛔⛔⛔ REGISTER ITEM 963: {PROMOTABLE} promotes this repository's images into \
             {promoted:?}, and the launcher's CODE must look there — a build the loop is actually \
             running is the one that fits a daemon this tree has moved past, and on 2026-09-08 it \
             was the only one on the machine that did. Prose does not count: a path that appears \
             only in a comment is a path nothing opens.",
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

    /// ⛔⛔⛔⛔⛔ **NO TRACKED SCRIPT USES A BUILTIN THE OLDEST BASH THIS PROJECT RUNS ON DOES NOT
    /// HAVE** — register item 947, and the gate the six cases above could not be.
    ///
    /// # ⛔⛔⛔⛔ What was measured, and why every case above was blind to it
    ///
    /// The macOS CI runner of 2026-09-07 printed
    /// `sprag-gui-launch: line 54: mapfile: command not found` **six times, once per case above**,
    /// and each case then failed on `Shown: ""`. Apple froze `/bin/bash` at **3.2** over its
    /// licence, `mapfile` arrived in bash 4, and `set -u` alone does not stop a script whose array
    /// simply never got filled — so the launcher ran on and showed nothing. Every assertion above
    /// is about what the script SAID, so all six reported the symptom and none could name the
    /// cause; the six had also been red on that runner with nobody reading it.
    ///
    /// # ⚠⚠⚠ Why a text scan is the right instrument here, and not a run
    ///
    /// The cases above run the launcher, and they run it under **this machine's** bash — which is
    /// 5.2 on the only host anybody develops on. A builtin that is missing somewhere else cannot be
    /// made missing here: `enable -n` acts on the CALLING shell, and the script is a fresh `bash`
    /// naming its own interpreter. So the fact worth holding is a fact about the FILE, and this
    /// crate's charter is *the gates a test cannot be*.
    ///
    /// ⚠⚠ **AND IT COVERS EVERY TRACKED SCRIPT, NOT THE ONE THAT BROKE.** The same disease had
    /// already been diagnosed and repaired in `.githooks/commit-msg` — whose own comment records
    /// that `set -e` made its `mapfile` line *"the END of this hook, so on macOS every rule below
    /// was unreachable"* and that four gates said so from 2026-08-14 with nobody reading them. One
    /// file was fixed, the disease was not, and it came back in a second file. A gate over one path
    /// would let it come back in a third.
    ///
    /// ⚠ A CLOSED SET rather than a denylist of everything ever added to bash: these are the
    /// constructs this project has actually used or reached for, each with the version that
    /// introduced it. A build that wants a fifth adds it here, which is a decision somebody takes
    /// rather than a drift.
    ///
    /// ⚠ `outside_strings` is DEFENSIVE and was not measured — no script here puts one of these
    /// words inside a literal today, so a mutation of it would be vacuously green. It is the shared
    /// reader (item 818: the second gate that needs one does not copy it) and it errs toward a red
    /// to read, which is why it is here rather than a `contains` on the raw line.
    #[test]
    fn no_tracked_script_uses_a_builtin_the_oldest_bash_this_project_runs_on_lacks() {
        /// `(needle, since, instead)` — what to look for, the bash it needs, and the repair.
        const YOUNGER_THAN_BASH_3: [(&str, &str, &str); 4] = [
            (
                "mapfile ",
                "4.0",
                "`GUIS=(); while IFS= read -r one; do GUIS+=(\"$one\"); done < <(…)`",
            ),
            ("readarray ", "4.0", "the same `while IFS= read -r` loop"),
            (
                "declare -A",
                "4.0",
                "two indexed arrays, or a `case` over the keys",
            ),
            (
                "${EPOCHSECONDS",
                "5.0",
                "`$(date +%s)`, which every one of them has",
            ),
        ];

        let root = crate::sources::workspace_root();
        // ⛔⛔⛔⛔⛔ **DISCOVERED AND NOT LISTED**, which is register item 945's own finding turned on
        // this gate: a hand-written list is *by definition the place that leaks* (items 80 and 762
        // are the same face), and the first draft of this very test named six paths while
        // `.githooks/` held **ten**. The population is every hook in that directory plus the
        // launcher — the scripts that run on a person's or a runner's machine rather than under
        // `cargo`.
        //
        // ⚠⚠ THE COUNT IS ASSERTED BELOW, because a `read_dir` that started answering nothing —
        // a rename, a move, a gate run from somewhere else — would make this test vacuously green,
        // which is item 924's shape exactly.
        let hooks = root.join(".githooks");
        let mut scripts = vec![root.join(LAUNCHER)];
        for entry in std::fs::read_dir(&hooks).unwrap_or_else(|why| {
            panic!(
                "⛔ REGISTER ITEM 947: {} is where this repository keeps the scripts that run \
                 outside `cargo` — {why}",
                hooks.display(),
            )
        }) {
            let path = entry.expect("a readable directory entry").path();
            if path.is_file() {
                scripts.push(path);
            }
        }
        assert!(
            scripts.len() > 6,
            "⚠⚠⚠ THE POPULATION COLLAPSED: this gate found {} script(s), and measured 2026-09-07 \
             there are eleven. A scan of nothing is green for the wrong reason — find out where \
             the hooks went before touching this number: {scripts:?}",
            scripts.len(),
        );

        let mut guilty = Vec::new();
        for path in &scripts {
            let name = path
                .strip_prefix(&root)
                .unwrap_or(path)
                .display()
                .to_string();
            let text = std::fs::read_to_string(path).unwrap_or_else(|why| {
                panic!("⛔ REGISTER ITEM 947: {name} could not be read — {why}")
            });
            for (line, body) in text.lines().enumerate() {
                // ⚠⚠ THE COMMENTS ARE SKIPPED AND THAT IS LOAD-BEARING: both files carry a
                // paragraph NAMING `mapfile` as the thing they must not use, and a scan that read
                // those would be permanently red at the very files it just repaired — the
                // instrument convicting its own documentation.
                let code = body.trim_start();
                if code.starts_with('#') {
                    continue;
                }
                for (needle, since, instead) in YOUNGER_THAN_BASH_3 {
                    if crate::sources::outside_strings(code).contains(needle) {
                        guilty.push(format!(
                            "  {name}:{} uses `{}`, which needs bash {since} — macOS ships 3.2 as \
                             `/bin/bash`. Use {instead}",
                            line + 1,
                            needle.trim(),
                        ));
                    }
                }
            }
        }
        assert!(
            guilty.is_empty(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 947: a tracked script needs a bash macOS does not have, and \
             the failure is SILENT in the worst way — `set -u` does not stop it, so the script runs \
             on with the value it never got. That is six red cases reporting `Shown: \"\"` in this \
             very file, and before that a commit hook whose every rule was unreachable on a Mac:\n{}",
            guilty.join("\n"),
        );
    }
}
