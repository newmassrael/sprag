//! Integration test: the `sprag` management CLI drives a real `sprag-term` over the socket.
//!
//! Both binaries are the built artifacts (`CARGO_BIN_EXE_*`), so a break in the wire vocabulary
//! the CLI shares with the daemon — or in the CLI's own output — fails in CI, not by hand.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use sprag_host::mux_action_path;
use sprag_host::wire::{
    BREAK_PANE_ACTION, BUILD, CLOSE_ACTION, DISPLAY_MESSAGE_ACTION, JOIN_PANE_ACTION, KEY_ACTION,
    KILL_SESSION_ACTION, KILL_WINDOW_ACTION, MOVE_PANE_ACTION, MOVE_WINDOW_ACTION,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT, RELEASE_AGENT_ACTION, RENAME_PANE_ACTION,
    RENAME_SESSION_ACTION, RENAME_WINDOW_ACTION, REPORT_AGENT_ACTION, RESIZE_ACTION,
    RESIZE_PANE_ACTION, RESIZE_WINDOW_ACTION, SELECT_PANE_ACTION, SELECT_WINDOW_ACTION,
    SET_LAYOUT_ACTION, SPAWN_ACTION, SPLIT_ACTION, SWAP_PANE_ACTION, WINDOWS_SLOT,
    ZOOM_PANE_ACTION, pane_input_path,
};
use sprag_rpc::{CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM, HOST_SILENT, HostConn};

/// Reaps the spawned host process and its socket file on drop — including on a panicked
/// assertion, so a failed run leaks neither.
struct HostChild(Child, PathBuf);
impl Drop for HostChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_file(&self.1);
        // ⚠ And the state home this daemon was given — see [`isolated_state_home`]. Removed on the
        // socket's terms: a guard that cleaned up one and leaked the other would trade a stray
        // socket for a stray directory tree.
        let _ = std::fs::remove_dir_all(isolated_state_home(&self.1));
    }
}

/// The state home a spawned daemon is given, derived from the socket it already owns.
///
/// # ⚠⚠⚠⚠⚠ Why every spawned daemon needs one, and what it cost not to have it
///
/// A daemon writes its snapshot, its run registry and its pane history under
/// `$XDG_STATE_HOME/sprag`. A test that spawns one WITHOUT saying where that is has just pointed a
/// real daemon at the ambient state home — **which on a developer's machine is their
/// `~/.local/state`** — and `sprag-gate`'s whole reason for existing is that no test can be the
/// guard for that, because the variable is process-global.
///
/// ⚠⚠ MEASURED 2026-08-19, by bisecting the suite against a scratch XDG home rather than by
/// reading: `-p sprag-host` left `$XDG_STATE_HOME/sprag` behind while `--lib` and the
/// `wire_client` target did not, so THIS file's daemons are the ones that write. It is the last
/// thing CI's `ambient-home-guard` was still failing on once register item 464 removed the ledger
/// file — an EMPTY directory is still a write, and the guard walks recursively for exactly that
/// reason (*"a walk that only listed the entries of `home` itself … could not tell a directory
/// somebody made from a file somebody wrote"*).
///
/// ⚠ Keyed on the SOCKET, which [`socket_path`] already mints unique per call, so parallel threads
/// in one binary cannot share a state home any more than they can share a socket — the R152/R153
/// race lesson applied to the second thing a daemon owns.
fn isolated_state_home(sock: &Path) -> PathBuf {
    sock.with_extension("state")
}

/// A state home unique to this CALL, for a CLI run that has no socket to derive one from.
///
/// ⚠ Its own counter rather than [`socket_path`]'s, for that function's stated reason: parallel
/// test threads share one binary, so a name keyed only on the pid is the same string in every
/// thread that asks — and here that would have two CLI runs removing each other's directory.
fn scratch_state_home() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-cli-it-{}-state-{n}", std::process::id()))
}

/// A socket path unique to this CALL (pid + a per-binary counter), so parallel test threads in
/// one binary never unlink each other's sockets — the `wire_client` R152/R153 race lesson.
fn socket_path() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("sprag-cli-it-{}-{n}.sock", std::process::id()))
}

/// Spawn a NON-daemon `sprag-term` serving `sock`, its boot pane running `cat` (which blocks on
/// its PTY, so the session stays live for the test's duration).
fn spawn_host() -> (HostChild, PathBuf) {
    spawn_host_env(&[])
}

/// [`spawn_host`] with EXTRA env vars on the daemon — the ssh test prepends a stand-in `ssh` to the
/// daemon's `PATH`, because the daemon (not the CLI) is what spawns the pane and resolves the program.
fn spawn_host_env(envs: &[(&str, &str)]) -> (HostChild, PathBuf) {
    spawn_host_with(&["cat"], envs)
}

/// [`spawn_host`] whose boot pane runs `program [args…]` instead of `cat` — for a test that needs
/// the pane to have PRINTED something (the search tests), not just to echo.
fn spawn_host_running(program_and_args: &[&str]) -> (HostChild, PathBuf) {
    spawn_host_with(program_and_args, &[])
}

/// [`spawn_host_running`] on a pane of a chosen WIDTH — for a claim about what happens at the right
/// margin, which the default size is too wide to reach.
fn spawn_host_sized(cols: u16, rows: u16, program_and_args: &[&str]) -> (HostChild, PathBuf) {
    spawn_host_argv(
        &[
            "--size".to_owned(),
            format!("{cols}x{rows}"),
            "--".to_owned(),
        ],
        program_and_args,
        &[],
    )
}

/// The one spawn: the boot command plus any daemon env overrides.
fn spawn_host_with(program_and_args: &[&str], envs: &[(&str, &str)]) -> (HostChild, PathBuf) {
    spawn_host_argv(&["--".to_owned()], program_and_args, envs)
}

/// The one spawn under both entry points: `leading` is whatever the daemon needs before the boot
/// command (always ending in `--`), so a sized host and a default one cannot drift in how they
/// wait for the bind.
fn spawn_host_argv(
    leading: &[String],
    program_and_args: &[&str],
    envs: &[(&str, &str)],
) -> (HostChild, PathBuf) {
    let sock = socket_path();
    let _ = std::fs::remove_file(&sock);
    // ⚠⚠⚠ BEFORE `.envs`, so a caller that wants to CHOOSE the state home (the restore tests, which
    // hand the same one to two daemon lifetimes) still overrides this. What it replaces is the
    // ambient default, which is somebody's real `~/.local/state` — see [`isolated_state_home`].
    let state = isolated_state_home(&sock);
    let child = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .args(leading)
        .args(program_and_args)
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .env("SPRAG_HOST_RPC", "1")
        .env("XDG_STATE_HOME", &state)
        .envs(envs.iter().copied())
        .stdin(Stdio::null())
        .spawn()
        .expect("spawn the sprag-term host binary");
    // ⚠ WAIT FOR THE BIND, and this is a flake fix rather than tidiness (R331). The `sprag` CLI
    // gives a connect 500ms and refuses after it — deliberately, because a management command talks
    // to an ALREADY-RUNNING daemon and has no spawn race to wait out. This harness has exactly that
    // race: it returned the moment `spawn` did, so every test here was betting the daemon would
    // bind within the first command's budget. Under a full-workspace run that bet is lost, and it
    // was: `the_cli_lists_attached_clients_and_shows_the_attached_count` failed one run with *"no
    // server running"* and passed alone. The product's budget is right; the harness owed the wait.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if HostConn::connect(&sock, Duration::from_millis(200)).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the daemon never bound {}",
            sock.display(),
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    (HostChild(child, sock.clone()), sock)
}

/// The result of running the `sprag` CLI: its stdout, its stderr, and whether it exited 0.
struct CliRun {
    stdout: String,
    stderr: String,
    ok: bool,
}

/// Run the `sprag` CLI against `sock`.
fn sprag(sock: &Path, args: &[&str]) -> CliRun {
    sprag_env(sock, args, &[])
}

/// Run the `sprag` CLI against `sock` with EXTRA env vars — the attach test points `SPRAG_GUI_BIN`
/// at a harmless stand-in (`/usr/bin/env`) for the real GUI window, so the launch + env
/// propagation are provable headlessly.
fn sprag_env(sock: &Path, args: &[&str], envs: &[(&str, &str)]) -> CliRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag"));
    cmd.args(args).env("SPRAG_HOST_RPC_SOCK", sock);
    // ⚠⚠⚠⚠⚠ **THE CLI WRITES STATE TOO, AND THAT IS WHAT WAS ACTUALLY LEAKING.** `sprag mute-hook`
    // files `$XDG_STATE_HOME/sprag/hook-mute.<pane>` (`hooks.rs`), so pointing only the SOCKET at
    // this test's daemon left the CLI writing the runner's real `~/.local/state`. Bisected against
    // a scratch XDG home 2026-08-19: the residue was `state/sprag/hook-mute.0`, and it is the last
    // thing CI's `ambient-home-guard` failed on after register item 464 took the review ledger out.
    //
    // ⚠⚠ IT IS THE SAME STATE HOME THE DAEMON ON THIS SOCKET WAS GIVEN — see
    // [`isolated_state_home`] — because these two processes are meant to share one machine's state:
    // a CLI that muted a hook somewhere the daemon does not read would be a gate proving nothing.
    // ⚠ Before `envs` below, so a caller that names its own still wins.
    cmd.env("XDG_STATE_HOME", isolated_state_home(sock));
    // ⚠⚠⚠⚠ **THE PANE THIS SUITE'S RUNNER IS ITSELF IN MUST NOT LEAK INTO THE CLI IT DRIVES** —
    // register item 226, which named ONE gate and had two. Run from a shell inside a sprag pane,
    // `sprag report-agent` picked up the RUNNER's `SPRAG_PANE` and asked a test daemon about a pane
    // it has never heard of: *"sprag: no pane 49 on this host"*, where the gate demanded the
    // refusal that names the variable. **The debt-repayment loop's own agent runs in a pane**, so
    // this is a red it meets every time and never causes — which is why every command in this tree
    // had grown an `env -u SPRAG_PANE` prefix.
    //
    // ⚠⚠ REMOVED BEFORE `envs` IS APPLIED, so a gate that WANTS a pane still sets one and wins.
    // That is the whole shape: the harness stops leaking, and every use of the variable below is a
    // stated intention rather than an inheritance.
    cmd.env_remove(sprag_host::PANE_ENV_VAR);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("run the sprag CLI");
    CliRun {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        ok: output.status.success(),
    }
}

/// A daemon OLDER than this build, serving NO address — [`sprag_peer`]'s, since R324.
///
/// It was written out in this file until then, and so was the proxy below, and so were two more in
/// `sprag-mcp` and `sprag-tui`: four stand-ins with three different ideas of what *"older"* means.
/// The crate holds the policy as data and every front picks the shape it needs; what the peer
/// answers is the WIRE's own fault string rather than one this file spelled.
fn stale_host() -> sprag_peer::OldDaemon {
    sprag_peer::OldDaemon::serving_nothing(&socket_path())
}

/// A daemon that serves every READ a live one serves and knows NO ACTION — a real `sprag-term`
/// behind a proxy that refuses `scene/invoke`.
///
/// Answers the peer, the daemon behind it, and that daemon's own socket: a test PREPARES state (a
/// second pane, a second window) through the daemon directly and then drives the swept verbs
/// through the aged front. The daemon is handed back so its lifetime is the test's — dropping it
/// ends the process.
/// A daemon that HAS every verb and refuses one WITHOUT SAYING WHY — every pinion before
/// PINION-PR82, and the degradation R325's deleted guesses used to cover.
///
/// A PROXY for [`aged_host`]'s reason: an acting path is reached only after its pre-flight reads
/// succeed, so a peer that serves nothing never gets there.
fn refusing_peer(upstream: &Path) -> sprag_peer::OldDaemon {
    sprag_peer::OldDaemon::proxying(
        &socket_path(),
        upstream,
        sprag_peer::Missing::refusing_without_reason(),
    )
}

fn aged_host() -> (sprag_peer::OldDaemon, HostChild, PathBuf) {
    let (daemon, upstream) = spawn_host();
    let peer =
        sprag_peer::OldDaemon::proxying(&socket_path(), &upstream, sprag_peer::Missing::actions());
    (peer, daemon, upstream)
}

/// Every way `grant`'s argument list can be wrong is REFUSED and NAMED, never quietly taken.
///
/// ⚠ Written when the debt question asked what R340 had left untested. Each of these is a way a
/// person's intent silently becomes a different command, and the duplicate is the worst of them:
/// `--share 10 --share 1000` under a "last one wins" rule is a pane granted the opposite of what
/// the first flag asked for, with nothing on screen to say so.
#[test]
fn every_malformed_grant_is_refused_and_says_which_argument() {
    let (_host, sock) = spawn_host();

    let twice = sprag(&sock, &["grant", "0", "--share", "10", "--share", "1000"]);
    assert!(!twice.ok, "a flag given twice is refused: {}", twice.stdout);
    assert!(
        twice.stderr.contains("--share") && twice.stderr.contains("twice"),
        "and the refusal names the flag: {}",
        twice.stderr,
    );

    let wordy = sprag(&sock, &["grant", "0", "--memory", "lots"]);
    assert!(!wordy.ok, "a non-number is refused: {}", wordy.stdout);
    assert!(
        wordy.stderr.contains("--memory") && wordy.stderr.contains("lots"),
        "and the refusal quotes what was typed: {}",
        wordy.stderr,
    );

    let dangling = sprag(&sock, &["grant", "0", "--processes"]);
    assert!(!dangling.ok, "a flag with no value is refused");
    assert!(
        dangling.stderr.contains("--processes"),
        "named: {}",
        dangling.stderr,
    );

    let unknown = sprag(&sock, &["grant", "0", "--cores", "4"]);
    assert!(
        !unknown.ok,
        "an unknown flag is refused rather than ignored"
    );
    assert!(
        unknown.stderr.contains("--cores"),
        "and it is quoted back, so a typo is visible: {}",
        unknown.stderr,
    );

    // THE CONTROL: the same shape, spelled right, is not refused for its spelling — so the four
    // above are about the spelling and not about `grant` refusing everything. See
    // `enforces_or_refuses_for_the_host` for why it cannot demand success.
    let ok = sprag(
        &sock,
        &["grant", "0", "--share", "100", "--processes", "64"],
    );
    assert!(
        enforces_or_refuses_for_the_host(&ok),
        "a well-formed grant is not refused for its spelling: {} / {}",
        ok.stdout,
        ok.stderr
    );
}

/// `grant` refuses a request that sets NOTHING, and it refuses at both layers that could accept it.
///
/// # Why this is a refusal and not a no-op
///
/// A `grant` with no settings is somebody who meant something and typed it wrong. Answering with
/// the grant they already had would print three plausible numbers and look exactly like success —
/// the failure mode a person cannot detect, because the output of "I did nothing" and the output of
/// "I set it to what it already was" are the same three numbers.
///
/// This drives the CLI's half, which refuses without a round trip so the message can name the
/// flags. The ACTION refuses it too, and that is the load-bearing one — it holds for whatever
/// client asked, `sprag-mcp` included — so it has its own gate one crate over
/// (`sprag_host::workspace`'s `a_grant_that_sets_nothing_is_refused_at_the_action`). Two tests
/// because a CLI-only gate would go green against a daemon that accepted an empty grant from
/// anybody else.
#[test]
fn a_grant_that_sets_nothing_is_refused_rather_than_reported_as_done() {
    let (_host, sock) = spawn_host();

    // Pane 0 throughout — the boot pane, which EXISTS. A refusal about a pane that was not there
    // would be the wrong refusal and would pass this test just as well.
    let run = sprag(&sock, &["grant", "0"]);
    assert!(!run.ok, "a grant with no settings failed: {}", run.stdout);
    assert!(
        run.stderr.contains("--share") && run.stderr.contains("--memory"),
        "the refusal names the flags rather than saying 'invalid': {}",
        run.stderr,
    );

    // A grant that DOES set something is not refused for THAT reason — the control this pair needs.
    //
    // ⚠ It cannot demand SUCCESS. A grant writes to a cgroup, and a host with no delegated subtree
    // has none, so demanding success would make this a gate on the developer's systemd
    // configuration — the R318/R319/R331 class, one layer out. **Measured**: it passes on this box
    // and FAILS under `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/bus`, which is what macOS
    // is permanently (`with_shares` is `cfg`-ed out there).
    let ok = sprag(&sock, &["grant", "0", "--share", "100"]);
    assert!(
        enforces_or_refuses_for_the_host(&ok),
        "a grant that sets a share is not refused for setting nothing: {} / {}",
        ok.stdout,
        ok.stderr,
    );
}

/// Whether a `grant` run either worked or failed ONLY because this host enforces nothing.
///
/// The gates that use it are about the ARGUMENT LIST, and every machine can get an argument list
/// wrong; only a machine with a delegated cgroup subtree can get the write right. Without this
/// split they are gates on the developer's systemd configuration rather than on `grant`.
fn enforces_or_refuses_for_the_host(run: &CliRun) -> bool {
    run.ok || run.stderr.contains("no cgroup subtree")
}

#[test]
fn the_cli_lists_creates_and_kills_sessions_over_the_socket() {
    let (_host, sock) = spawn_host();

    // ls: the boot session "0" is the one an unscoped request lands in. (The CLI's own connect
    // window absorbs the host's bind race.)
    let run = sprag(&sock, &["ls"]);
    assert!(run.ok, "ls succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("0:") && run.stdout.contains("(default)"),
        "ls shows the boot session as the default: {}",
        run.stdout,
    );

    // new work: creates it and prints the name a client would scope to.
    let run = sprag(&sock, &["new", "work"]);
    assert!(run.ok, "new succeeded: {}", run.stderr);
    assert_eq!(run.stdout.trim(), "work", "new prints the created name");

    // ls now lists it, and "0" is still the default.
    let run = sprag(&sock, &["ls"]);
    assert!(run.stdout.contains("work"), "ls shows it: {}", run.stdout);
    assert!(run.stdout.contains("(default)"), "default unmoved");

    // kill-session work: a non-last kill removes it.
    let run = sprag(&sock, &["kill-session", "work"]);
    assert!(run.ok, "kill-session succeeded: {}", run.stderr);
    assert!(run.stdout.contains("killed work"), "kill-session confirms");

    // ...and it is gone, while the default survives (it was not the last).
    let run = sprag(&sock, &["ls"]);
    assert!(!run.stdout.contains("work"), "the killed session is gone");
    assert!(
        run.stdout.contains("0:"),
        "a non-last kill leaves the default"
    );

    // Killing an unknown session fails cleanly — non-zero exit AND a clean message, not the raw
    // wire error the mapping at `sprag.rs` replaces.
    let run = sprag(&sock, &["kill-session", "ghost"]);
    assert!(!run.ok, "killing an unknown session fails");
    assert!(
        run.stderr.contains("no session named") && !run.stderr.contains("host rpc error"),
        "the refusal is a clean message, not the raw wire error: {}",
        run.stderr,
    );
}

/// `kill-server` ends the daemon: it succeeds, and a follow-up command then finds no server.
#[test]
fn the_cli_kill_server_ends_the_daemon() {
    let (_host, sock) = spawn_host();

    // The server is up.
    assert!(sprag(&sock, &["ls"]).ok, "server up before kill-server");

    // kill-server stops it.
    let run = sprag(&sock, &["kill-server"]);
    assert!(run.ok, "kill-server succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("server stopped"),
        "kill-server confirms"
    );

    // The daemon exits asynchronously after the last kill's reply, so poll `ls` until it can no
    // longer reach a server (a bounded wait, not a fixed sleep — the "a flake is a bug" rule).
    let gone = (0..40).any(|_| {
        if sprag(&sock, &["ls"]).ok {
            std::thread::sleep(std::time::Duration::from_millis(50));
            false
        } else {
            true
        }
    });
    assert!(
        gone,
        "kill-server ended the daemon: ls no longer finds a server"
    );
}

/// `kill-server --purge` also ends the daemon and confirms the workspace was purged. (The snapshot
/// deletion itself needs a `--daemon` with a persistent state dir — it is smoke-tested; here we
/// prove the flag is accepted and the end-the-daemon path still works.)
#[test]
fn the_cli_kill_server_purge_ends_the_daemon() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["ls"]).ok, "server up before kill-server");

    let run = sprag(&sock, &["kill-server", "--purge"]);
    assert!(run.ok, "kill-server --purge succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("purged"),
        "kill-server --purge confirms the purge: {}",
        run.stdout,
    );
    let gone = (0..40).any(|_| {
        if sprag(&sock, &["ls"]).ok {
            std::thread::sleep(std::time::Duration::from_millis(50));
            false
        } else {
            true
        }
    });
    assert!(gone, "kill-server --purge ended the daemon");
}

/// An unknown `kill-server` argument is refused BEFORE any kill, so a typo cannot end the daemon —
/// the server is still reachable afterwards.
#[test]
fn kill_server_refuses_an_unknown_arg() {
    let (_host, sock) = spawn_host();
    let run = sprag(&sock, &["kill-server", "--force"]);
    assert!(!run.ok, "an unknown kill-server arg is refused");
    assert!(
        run.stderr.contains("--purge"),
        "the error names the only accepted flag: {}",
        run.stderr,
    );
    assert!(
        sprag(&sock, &["ls"]).ok,
        "the refused kill-server did NOT end the daemon",
    );
}

/// The window subcommands drive the SCOPED mux window actions over the socket: list, create +
/// select, select, rename, and kill a window — each `-t SESSION`.
#[test]
fn the_cli_manages_windows_over_the_socket() {
    let (_host, sock) = spawn_host();

    // windows -t 0: the boot session has one window "0", current.
    let run = sprag(&sock, &["windows", "-t", "0"]);
    assert!(run.ok, "windows succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("0 (current)"),
        "one boot window, current: {}",
        run.stdout,
    );

    // new-window -t 0 logs: creates + selects it, printing its name.
    let run = sprag(&sock, &["new-window", "-t", "0", "logs"]);
    assert!(run.ok, "new-window succeeded: {}", run.stderr);
    assert_eq!(
        run.stdout.trim(),
        "logs",
        "new-window prints the created name"
    );
    let run = sprag(&sock, &["windows", "-t", "0"]);
    assert!(
        run.stdout.contains("logs (current)"),
        "the new window is listed and current: {}",
        run.stdout,
    );

    // select-window -t 0 0: moves current back to "0".
    assert!(sprag(&sock, &["select-window", "-t", "0", "0"]).ok);
    assert!(
        sprag(&sock, &["windows", "-t", "0"])
            .stdout
            .contains("0 (current)"),
        "the current window moved back to 0",
    );

    // select-window -n / -p: tmux's `next-window` / `previous-window`, on the verb sprag already
    // had (R305). Two windows here, so `-n` from the first lands on the second and `-p` WRAPS back
    // — the wrap is the half a walk that clamped would get wrong while looking right in the middle.
    //
    // The printed line is asserted too, because the daemon answers the window it LANDED on and a
    // step cannot name one: a CLI that echoed its argument would print `-n` here.
    let run = sprag(&sock, &["select-window", "-t", "0", "-n"]);
    assert!(run.ok, "select-window -n succeeded: {}", run.stderr);
    assert_eq!(
        run.stdout.trim(),
        "selected logs",
        "the step prints the window the DAEMON landed on",
    );
    assert!(
        sprag(&sock, &["windows", "-t", "0"])
            .stdout
            .contains("logs (current)"),
        "and the session really moved",
    );
    let run = sprag(&sock, &["select-window", "-t", "0", "-n"]);
    assert_eq!(
        run.stdout.trim(),
        "selected 0",
        "the ring WRAPS past the last window onto the first",
    );
    let run = sprag(&sock, &["select-window", "-t", "0", "-p"]);
    assert_eq!(
        run.stdout.trim(),
        "selected logs",
        "...and the other way, past the first onto the last",
    );
    // Back where the checks below expect it.
    assert!(sprag(&sock, &["select-window", "-t", "0", "0"]).ok);

    // rename-window -t 0 main: renames the CURRENT window ("0") to "main".
    assert!(sprag(&sock, &["rename-window", "-t", "0", "main"]).ok);
    let run = sprag(&sock, &["windows", "-t", "0"]);
    assert!(
        run.stdout.contains("main (current)"),
        "the current window was renamed: {}",
        run.stdout,
    );

    // kill-window -t 0 logs: removes a non-last window.
    assert!(sprag(&sock, &["kill-window", "-t", "0", "logs"]).ok);
    assert!(
        !sprag(&sock, &["windows", "-t", "0"])
            .stdout
            .contains("logs"),
        "the killed window is gone",
    );

    // An unknown session is a clean pre-flight error; a missing -t is an argument error.
    let ghost = sprag(&sock, &["windows", "-t", "ghost"]);
    assert!(!ghost.ok, "windows on a ghost session fails");
    assert!(
        ghost.stderr.contains("no session named"),
        "clean error: {}",
        ghost.stderr,
    );
    let noarg = sprag(&sock, &["windows"]);
    assert!(!noarg.ok, "windows without -t fails");
    assert!(
        noarg.stderr.contains("target session is required"),
        "arg error: {}",
        noarg.stderr,
    );
}

/// `move-window` reorders a session's windows over the socket, and its FOUR SENTENCES are pinned —
/// including the three that mean nothing moved, which is the discrimination the rival's `bool`
/// cannot make (`Workspace::move_tab`, herdr `9a4ce5e1` `src/workspace.rs:619`, whose handler then
/// reports success and emits no event for all three).
///
/// The listing is read back after every move rather than trusting the sentence: the printed word is
/// the DAEMON's own outcome, but "the order really changed" is a different claim from "the daemon
/// said moved", and only the second is a claim about the CLI.
///
/// REVERT-PROOF: drop the `if anchor > from` frame correction in `Session::move_window` and the
/// `--after` row lands one window early; make the CLI print `moved` for every outcome and the three
/// no-op rows fail.
#[test]
fn the_cli_moves_a_window_and_says_which_nothing_happened() {
    let (_host, sock) = spawn_host();
    let order = |sock: &std::path::Path| {
        sprag(sock, &["windows", "-t", "0"])
            .stdout
            .lines()
            .map(|line| line.replace(" (current)", ""))
            .collect::<Vec<_>>()
    };
    for name in ["a", "b", "c"] {
        assert!(sprag(&sock, &["new-window", "-t", "0", name]).ok);
    }
    assert!(sprag(&sock, &["select-window", "-t", "0", "0"]).ok);
    assert_eq!(order(&sock), ["0", "a", "b", "c"], "the order as created");

    // NAMED, to the front.
    let run = sprag(&sock, &["move-window", "-t", "0", "c", "--first"]);
    assert!(run.ok, "move-window succeeded: {}", run.stderr);
    assert_eq!(run.stdout.trim(), "moved c");
    assert_eq!(order(&sock), ["c", "0", "a", "b"]);

    // ANCHORED, forward — the direction that exercises the frame correction.
    let run = sprag(&sock, &["move-window", "-t", "0", "c", "--after", "a"]);
    assert_eq!(run.stdout.trim(), "moved c");
    assert_eq!(order(&sock), ["0", "a", "c", "b"]);

    // UNNAMED: the window the session is ON, resolved by the daemon and NAMED BACK — a caller that
    // omitted it learns which one it meant, which is the half a bare "ok" could not carry.
    let run = sprag(&sock, &["move-window", "-t", "0", "--last"]);
    assert_eq!(run.stdout.trim(), "moved 0");
    assert_eq!(order(&sock), ["a", "c", "b", "0"]);

    // THE THREE THAT MOVED NOTHING, each with its own sentence.
    let run = sprag(&sock, &["move-window", "-t", "0", "--last"]);
    assert!(
        run.ok,
        "a no-op is a SUCCESS, not a failure: {}",
        run.stderr
    );
    assert_eq!(run.stdout.trim(), "0 is already there");
    let run = sprag(&sock, &["move-window", "-t", "0", "a", "--before", "a"]);
    assert_eq!(run.stdout.trim(), "a cannot be anchored to itself");
    assert_eq!(order(&sock), ["a", "c", "b", "0"], "and nothing moved");
    // `Alone` needs a session holding ONE window, which the boot session no longer does.
    assert!(sprag(&sock, &["new", "solo"]).ok);
    let run = sprag(&sock, &["move-window", "-t", "solo", "-p"]);
    assert_eq!(run.stdout.trim(), "0 is this session's only window");

    // A REFUSAL names the ANCHOR, not the window that was to move — the two are different mistakes
    // and only one of them is in the user's hand.
    let run = sprag(&sock, &["move-window", "-t", "0", "a", "--after", "nosuch"]);
    assert!(!run.ok, "an absent anchor fails");
    assert!(
        run.stderr
            .contains("no window named \"nosuch\" to anchor to"),
        "the refusal names the anchor: {}",
        run.stderr,
    );

    // The ARGUMENT errors, which never reach the wire.
    let run = sprag(&sock, &["move-window", "-t", "0", "a"]);
    assert!(!run.ok, "a move with no place fails");
    assert!(
        run.stderr.contains("move-window needs a place"),
        "and says what is missing: {}",
        run.stderr,
    );
    let run = sprag(&sock, &["move-window", "-t", "0", "--first", "--last"]);
    assert!(
        run.stderr.contains("exactly one place"),
        "two places is one mistake with one sentence: {}",
        run.stderr,
    );
    let run = sprag(&sock, &["move-window", "-t", "0", "--before"]);
    assert!(
        run.stderr
            .contains("needs the name of a window to anchor to"),
        "an anchorless --before is an error at the CLI, where it ASKS at a keybinding: {}",
        run.stderr,
    );
}

/// ⚠⚠⚠⚠⚠ **THE PANE LISTING SAYS WHEN A PANE'S CHILD IS GONE, AND IN THE WORDS THE OTHER SURFACE
/// USES** — register item 418.
///
/// # What its absence cost, which is the reason this is a gate and not a nicety
///
/// A person pressed `Esc` at a dialog in a restored pane, saw nothing happen, pressed it again, and
/// reported that **the key was broken**. It was not: the first `Esc` had been honoured and the
/// program had EXITED, the screen kept the last frame it painted, and the second `Esc` reached a
/// terminal with no program on it — so the tty echoed it as `^[`. Every piece of that was already
/// knowable: `dead` rides the pane row (additive, one-way), `sprag processes` says `no child`, and
/// the GUI's title has carried an `(exited)` suffix all along. **This listing — the one a person
/// greps — printed a dead pane byte-identically to a live one.**
///
/// ⚠⚠⚠ **The three arms are the vocabulary, not the flag.** A gate that only checked "some marker
/// appears" would let a signalled death read as `exited 1`, which is the specific wrong answer a
/// second hand-written spelling produces — so each ending is driven for real and read back.
///
/// ⚠ Linux-gated with its neighbours: it reads the process table to know when the child is actually
/// reaped, rather than sleeping and hoping.
#[test]
#[cfg(target_os = "linux")]
fn the_pane_listing_says_a_child_is_gone_and_how_it_went() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-dead-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );

    // ⚠⚠⚠⚠ A LIVE PANE IS CREATED FIRST, AND IT IS LOAD-BEARING RATHER THAN TIDY. A session does
    // not outlive its last pane and the last session ends the daemon (R309), so a fixture whose
    // FIRST pane is one of the dying arms below kills the very daemon it is about to read — which
    // is exactly what this gate did on its first run: `sprag panes` answered an empty listing
    // because there was no longer a server. This pane holds the session open for all four arms and
    // doubles as the live control at the end.
    let anchor = sprag(&sock, &["split-window", "--", "sleep", "300"]);
    assert!(anchor.ok, "the anchor pane: {}", anchor.stderr);
    let anchor_pane = anchor.stdout.trim().to_owned();

    // The row for `pane`, once the listing has admitted the child is gone. The wait is on the
    // CONDITION rather than on a clock: `dead` is published when the output stream ends and
    // `child_exit` lands later, so a sleep would race the second fact and read the first.
    let row_when_dead = |pane: &str, expect: &str| -> String {
        assert!(
            wait_for(Duration::from_secs(10), || {
                sprag(&sock, &["panes"])
                    .stdout
                    .lines()
                    .any(|line| line.starts_with(&format!("{pane}:")) && line.contains(expect))
            }),
            "pane {pane} never came to read {expect:?}: {}",
            sprag(&sock, &["panes"]).stdout,
        );
        sprag(&sock, &["panes"])
            .stdout
            .lines()
            .find(|line| line.starts_with(&format!("{pane}:")))
            .expect("the pane is listed")
            .to_owned()
    };

    // ── ARM 1: a CLEAN exit. The plain word, with no number attached. ──
    let clean = sprag(&sock, &["split-window", "--", "sh", "-c", "exit 0"]);
    assert!(clean.ok, "{}", clean.stderr);
    let clean_row = row_when_dead(clean.stdout.trim(), "(exited)");
    assert!(
        !clean_row.contains("exited 0"),
        "⚠⚠ A CLEAN EXIT SAYS `(exited)` AND NAMES NO CODE. `exited 0` reads as a fault report \
         about a command that succeeded, which is the whole reason these words are not tmux's \
         `dead`: {clean_row}",
    );

    // ── ARM 2: a FAILING exit. The code is the fact a person is looking for. ──
    let failed = sprag(&sock, &["split-window", "--", "sh", "-c", "exit 3"]);
    assert!(failed.ok, "{}", failed.stderr);
    let failed_row = row_when_dead(failed.stdout.trim(), "(exited 3)");
    assert!(
        failed_row.contains("(exited 3)"),
        "a non-zero exit carries its code: {failed_row}",
    );

    // ── ARM 3: a SIGNALLED death — the arm a second spelling gets wrong. ──
    //
    // ⚠ The child kills ITSELF rather than being killed from here: `kill-pane` removes the pane, so
    // there would be no row left to read, and signalling the daemon's grandchild from a test is a
    // race against the reaper. `PaneExit`'s own doc is why this arm exists at all — a signalled
    // death carries the platform's stand-in code `1`, so a renderer that consulted the code first
    // would print `(exited 1)` and lose the difference between a failed build and the OOM killer.
    let killed = sprag(&sock, &["split-window", "--", "sh", "-c", "kill -TERM $$"]);
    assert!(killed.ok, "{}", killed.stderr);
    let killed_row = row_when_dead(killed.stdout.trim(), "killed");
    assert!(
        !killed_row.contains("exited 1"),
        "⚠⚠⚠⚠ A SIGNALLED DEATH MUST NOT READ AS AN EXIT CODE. The signal is consulted first \
         precisely because the `1` beside it is the platform's stand-in and not the process's \
         choice — see `sprag_terminal::exit_phrase`: {killed_row}",
    );

    // ── AND THE CONTROL: the anchor's child is still running, so its row says nothing at all and
    //    the listing a script parses is unchanged for every live pane. ──
    let alive_row = sprag(&sock, &["panes"])
        .stdout
        .lines()
        .find(|line| line.starts_with(&format!("{anchor_pane}:")))
        .expect("the live pane is listed")
        .to_owned();
    assert!(
        !alive_row.contains("exited") && !alive_row.contains("killed"),
        "⚠⚠⚠ A LIVE PANE MUST BE BYTE-IDENTICAL TO THE PRE-418 SHAPE — the marker is additive, and \
         a row that always carried it would be a second thing for every script here to parse: \
         {alive_row}",
    );
    drop(guard);
}

/// ⚠⚠⚠⚠⚠ **A CALLER CAN SAY WHERE A PANE STARTS, AND WITHOUT SAYING IT THE ANSWER IS `$HOME` —
/// NOT THE DAEMON'S OWN DIRECTORY** (register item 417).
///
/// # Why this is a control PAIR and neither half means anything alone
///
/// The claim is a DIFFERENCE, so both arms run against one daemon in one test. The daemon is
/// deliberately started from a directory that is neither `$HOME` nor the target, which is what makes
/// the two answers distinguishable at all: a gate that spawned its daemon in `$HOME` could not tell
/// *"absent means home"* from *"absent means the daemon's directory"*, and those were the two
/// sentences this product held at the same time — `wire.rs` published one and `start_dir` did the
/// other.
///
/// ⚠⚠⚠ **What the wrong sentence cost, measured on the owner's own machine 2026-08-18**: a daemon
/// whose cwd was the repository spawned its panes into `$HOME`, so a restored `claude` came
/// back asking to trust the home directory instead of the project, and the run never started.
/// Nothing in the product could state where an agent's pane ought to be, because no CLI verb could
/// send the `cwd` the wire had taken all along.
///
/// ⚠ The directory is read from the pane's own child through `/proc`, not from anything the daemon
/// reports about itself: what is under test is where the process ACTUALLY IS. Linux-gated for that
/// reason, like its neighbours that read the process table.
#[test]
#[cfg(target_os = "linux")]
fn a_caller_can_say_where_a_pane_starts_and_silence_means_home() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-cwd-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );

    // Where this test's daemon actually is — the third directory, and the reason the pair separates.
    let daemon_dir = std::fs::read_link(format!(
        "/proc/{}/cwd",
        daemon_pid(&sock).expect("the daemon is findable")
    ))
    .expect("the daemon's own cwd is readable");
    let home = std::env::var("HOME").expect("a home directory");
    assert_ne!(
        daemon_dir,
        Path::new(&home),
        "⚠ this gate is only meaningful when the daemon is NOT sitting in $HOME — otherwise the two \
         candidate rules give the same answer and neither arm below decides anything",
    );

    let cwd_of = |pane: &str| -> PathBuf {
        let listed = sprag(&sock, &["processes", pane]);
        assert!(listed.ok, "processes {pane}: {}", listed.stderr);
        // ⚠ The pid is taken from the word after `child`, not from the first number in the output —
        // the listing opens with `sampled 0 ms ago`, so a bare "first integer" scan reads that `0`
        // and asks about `/proc/0`. Found by running it.
        let mut words = listed.stdout.split_whitespace();
        let pid: u32 = words
            .by_ref()
            .find(|word| *word == "child")
            .and_then(|_| words.next())
            .and_then(|word| word.parse().ok())
            .unwrap_or_else(|| panic!("the pane's child pid is in the listing: {}", listed.stdout));
        std::fs::read_link(format!("/proc/{pid}/cwd")).expect("the child's cwd is readable")
    };

    // ── ARM 1: SILENCE. Not the daemon's directory — $HOME. ──
    let bare = sprag(&sock, &["split-window"]);
    assert!(bare.ok, "a bare split: {}", bare.stderr);
    let bare_pane = bare.stdout.trim().to_owned();
    assert_eq!(
        cwd_of(&bare_pane),
        Path::new(&home),
        "⚠⚠⚠⚠⚠ ABSENT `cwd` IS `$HOME`. `wire.rs` published *the DAEMON's own directory* until item \
         417 measured this, and `start_dir` had always done the other thing — deliberately, with \
         its reasoning. This arm is what makes the published sentence answerable instead of \
         plausible (the daemon is in {daemon_dir:?})",
    );

    // ── ARM 2: SAID. The caller's directory, which nothing could express before. ──
    let named = sprag(&sock, &["split-window", "-c", "/tmp"]);
    assert!(named.ok, "a split with -c: {}", named.stderr);
    let named_pane = named.stdout.trim().to_owned();
    assert_eq!(
        cwd_of(&named_pane),
        Path::new("/tmp"),
        "⚠⚠⚠⚠ AND A CALLER CAN NAME IT. This is item 417's repair: `SPAWN_CWD_KEY` had existed on \
         the wire the whole time and NO CLI verb sent it, so the debt loop's own skill carried a \
         JSON-RPC helper for this single field. Deleting the `-c` arm in `split_window` reddens here",
    );

    // ⚠ A directory that does not exist is refused by the DAEMON before a pane is built — the pane
    // that would otherwise be born already dead. Asserted here because it is the same argument's
    // other edge, and because a client that silently opened $HOME instead would be the wrong answer
    // that decodes cleanly.
    let missing = sprag(&sock, &["split-window", "-c", "/nonexistent-sprag-417"]);
    assert!(
        !missing.ok,
        "a cwd that names no directory must be refused, not quietly replaced: {:?}",
        missing.stdout,
    );
    drop(guard);
}

/// `--version` answers off a socket with NO daemon behind it — R281.
///
/// The point is what it does NOT do. Every other command connects first, so a version that needed
/// a server could not answer the one question asked of a misbehaving install: which build is this.
/// The socket here is a path nothing is listening on, which is why the assertion is about the exit
/// code and stdout rather than about the string alone — a command that failed to reach a daemon
/// also prints nothing on stdout, and the two look identical if only stderr is read.
///
/// ⚠⚠⚠⚠ **AND IT NAMES THE COMMIT, WHICH THIS GATE HELD THE ABSENCE OF FOR A WHILE.** Item 438
/// made `print_version` say `sprag <version> (<commit>)` precisely because `CARGO_PKG_VERSION` is
/// `0.0.1` and has never moved — the bare string was the same sentence for every build this
/// repository has ever produced, so it could not answer the question the command exists for. This
/// gate was not updated with it and pinned the pre-438 answer; nothing went red because the
/// pre-push hook does not run this suite. Found 2026-08-18 by a workspace sweep taken for an
/// unrelated pin bump.
///
/// ⚠⚠⚠ **The expected string is ASKED OF THE PRODUCT, never written down here.** A commit hash
/// copied into a fixture is stale the moment the next commit lands, and this register has paid for
/// written-down numbers before. `wire::BUILD` is the same constant the binary printed, so the pair
/// cannot drift — while dropping the clause entirely (the mutation that matters) still reddens.
#[test]
fn version_answers_without_a_daemon() {
    let sock = socket_path();
    let expected = format!("sprag {} ({})", env!("CARGO_PKG_VERSION"), BUILD);
    for flag in ["--version", "-V", "version"] {
        let run = sprag(&sock, &[flag]);
        assert!(run.ok, "{flag} succeeded with no server: {}", run.stderr);
        assert_eq!(
            run.stdout.trim(),
            expected,
            "{flag} prints the build on stdout — the VERSION alone cannot distinguish two images",
        );
    }
    // The control: the same absent daemon refuses a command that needs one, so the success above
    // is `--version` not contacting it — not this socket happening to work.
    let needs_one = sprag(&sock, &["ls"]);
    assert!(!needs_one.ok, "ls has no server to answer it");
    assert!(
        needs_one.stderr.contains("no server running"),
        "and says so: {}",
        needs_one.stderr,
    );
}

/// A session does NOT outlive its last pane, and `kill-pane` says so — R309.
///
/// # What this test used to say, and why it says the opposite now
///
/// It was `a_session_the_listing_hides_is_still_addressable` (R281), and it built exactly this
/// state: two sessions, then `kill-pane` on the boot anchor's only pane. The anchor was then a
/// session holding a window holding nothing — `SessionInfo::is_listable` hid it (no panes, nobody
/// attached) while the daemon went on serving it and refusing to let anyone re-create the name. The
/// bug it pinned the fix for was that the CLI's pre-flight scanned the HUMAN listing for an
/// ADDRESS, so `panes -t 0` answered `no session named "0"` while `new 0` answered `a session named
/// "0" already exists` — both true, both about the same daemon.
///
/// R309 removes the state rather than the disagreement: closing a window's last pane ends the
/// WINDOW, and a session's last window ends the SESSION. So the anchor is GONE, and the two answers
/// can no longer disagree because there is nothing left for them to disagree about. The scope
/// resolver is untouched — it is still not the listing — it simply has no hidden session to reach.
///
/// **`new 0` succeeding is the assertion that carries the round**, and it is the one the old test
/// could not make: it tells a hidden session apart from an absent one. A cascade that merely
/// stopped listing the anchor would pass every other line here.
#[test]
fn a_session_does_not_outlive_its_last_pane() {
    let (_host, sock) = spawn_host();

    // A second session, so ending the anchor cannot drain the last one and stop the daemon this
    // test still has to talk to.
    assert!(sprag(&sock, &["new", "work"]).ok, "a second session");

    let killed = sprag(&sock, &["kill-pane", "0", "-t", "0"]);
    assert!(
        killed.ok,
        "the anchor's only pane is closed: {}",
        killed.stderr
    );
    // The SENTENCE, not just the exit code: the whole point is that a user who typed a PANE verb is
    // told their session went. Before R309 this printed `killed pane 0` and the session survived.
    assert_eq!(
        killed.stdout.trim(),
        "killed pane 0 — the window went with it, and the session",
        "the kill names every level its cascade reached",
    );

    let listed = sprag(&sock, &["ls"]);
    assert!(listed.ok, "ls succeeded: {}", listed.stderr);
    assert!(
        !listed.stdout.contains("0:"),
        "the anchor is not listed: {}",
        listed.stdout,
    );
    assert!(
        listed.stdout.contains("work"),
        "the guard is vacuous unless ls answered at all: {}",
        listed.stdout,
    );

    // ...and it is not there to be addressed either, which is the half that changed.
    let scoped = sprag(&sock, &["panes", "-t", "0"]);
    assert!(
        !scoped.ok,
        "the ended session is not addressable: {}",
        scoped.stdout
    );
    assert!(
        scoped.stderr.contains("no session named"),
        "and it is refused as absent rather than as empty: {}",
        scoped.stderr,
    );

    // THE DISCRIMINATOR. A session merely hidden still holds its name; one that ENDED gives it
    // back. This is the exact contradiction the old defect was made of, asserted from the other
    // side: the two answers now agree.
    let reborn = sprag(&sock, &["new", "0"]);
    assert!(
        reborn.ok,
        "the name is free again, so the session really ended: {}",
        reborn.stderr,
    );

    // The refusal a real unknown name gets is unchanged, so nothing here bought its answers by
    // refusing everything.
    let ghost = sprag(&sock, &["panes", "-t", "ghost"]);
    assert!(!ghost.ok, "an unknown session still fails");
    assert!(
        ghost.stderr.contains("no session named"),
        "clean error: {}",
        ghost.stderr,
    );
}

/// A refused agent report reaches the operator as a sentence about PANES, never as the wire's own
/// vocabulary (R283).
///
/// These two verbs were the only CLI commands with no mapping at all: `sprag report-agent --pane
/// 999` printed `scene/invoke /sprag_mux/external/report_agent: host rpc error: InvokeRejected` —
/// a scene path and a Rust enum variant, neither of which an operator can act on.
///
/// The WHOLE sentence is pinned, not three fragments of it, because that is what a person reads
/// (R279). Both verbs are checked: they share [`agent_refusal`], and a test of one would pass while
/// the other still leaked. The CONTROL is the happy path in the same test — a refusal message
/// cannot be produced by a command that is simply broken.
#[test]
fn a_refused_agent_report_names_the_pane_and_not_the_wire() {
    let (_host, sock) = spawn_host();

    for command in [
        vec!["report-agent", "working", "--pane", "999"],
        vec!["release-agent", "--pane", "999"],
    ] {
        let verb = command[0];
        let run = sprag(&sock, &command);
        assert!(!run.ok, "{verb} for a pane that is not there fails");
        assert_eq!(
            run.stderr.trim(),
            // The refusal is the RESOLVER's now, and it is exact: before R312 the pane half of
            // this reached the daemon, came back as a payload-less `Rejected`, and had to be
            // rendered as a two-way guess (upstream PINION-PR82). Resolving the pane first
            // answers one half locally and leaves the other half to say only what it means.
            format!("sprag: {verb}: no pane 999 in the default session (panes: [0])"),
            "the refusal is the sentence a person reads",
        );
    }

    // The control: the same two verbs against the pane that IS there. Without it, every assertion
    // above would also pass against a build where both commands simply failed.
    let reported = sprag(&sock, &["report-agent", "working", "--pane", "0"]);
    assert!(
        reported.ok,
        "the boot pane accepts a report: {}",
        reported.stderr
    );
    let released = sprag(&sock, &["release-agent", "--pane", "0"]);
    assert!(released.ok, "and hands it back: {}", released.stderr);
}

/// `ls` still prints where each session is working, now that the fact comes from a SECOND read —
/// and it joins the two by NAME, not by position (R282).
///
/// The two answers are different lengths ON PURPOSE and this test builds the state where that
/// bites: `sessions` is the human listing and hides a resting empty anchor, while the activity
/// reading describes EVERY session the registry holds, hidden ones included. So with the anchor
/// emptied, `work` is row 0 of the listing and row 1 of the reading.
///
/// That makes the assertion below a real discriminator rather than a smoke check. A positional join
/// would hand `work` the anchor's row — a session with no pane, hence no cwd — and the line would
/// come out bare. It is bare too if the second read is dropped altogether, which is the other way
/// this can break. One assertion, both failure modes.
#[test]
fn ls_joins_the_activity_sample_onto_the_session_it_belongs_to() {
    let (_host, sock) = spawn_host();

    assert!(sprag(&sock, &["new", "work"]).ok, "a second session");
    // Empty the boot anchor, so the listing hides it and the two answers stop lining up.
    assert!(
        sprag(&sock, &["kill-pane", "0", "-t", "0"]).ok,
        "the anchor's only pane is closed",
    );

    let listed = sprag(&sock, &["ls"]);
    assert!(listed.ok, "ls succeeded: {}", listed.stderr);
    let work = listed
        .stdout
        .lines()
        .find(|line| line.starts_with("work:"))
        .unwrap_or_else(|| panic!("ls lists the working session: {}", listed.stdout));
    // The pane inherits the daemon's working directory, which is wherever this harness ran it — so
    // WHICH path it is is not a fact worth pinning. That there is one is.
    let (_, after_windows) = work
        .split_once("window(s)")
        .expect("the line states its window count");
    assert!(
        after_windows.contains('/'),
        "the working session's line carries the cwd its own pane reports: {work:?}",
    );
}

/// break-pane and join-pane MOVE a pane between windows over the CLI (plus the refusal paths). The
/// pane set-up (spawn a second pane, read ids) goes over the wire — the CLI has no pane-spawn verb —
/// while the moves themselves go through the `sprag` binary, so its arg parsing, dispatch, and
/// output are what is under test; the deep behaviour is the registry + `wire_client` tests' job.
#[test]
fn the_cli_breaks_and_joins_panes_over_the_socket() {
    let (_host, sock) = spawn_host();
    let mut c = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");

    // The boot window "0" has one pane; break-pane on the ONLY pane is refused, cleanly.
    let refused = sprag(&sock, &["break-pane", "-t", "0", "0"]);
    assert!(!refused.ok, "breaking the only pane is refused");
    assert_eq!(
        refused.stderr.trim(),
        "sprag: cannot break the only pane in a window",
        "the refusal is `PaneMoveError::LastPane`'s own sentence — it used to be a three-cause \
         guess (*\"is its window's only pane, no window holds it, or the name is taken\"*)",
    );
    // A missing pane id is an argument error.
    let noarg = sprag(&sock, &["break-pane", "-t", "0"]);
    assert!(
        !noarg.ok && noarg.stderr.contains("needs a pane id"),
        "arg error: {}",
        noarg.stderr,
    );

    // Spawn a second pane into window "0" (over the wire) so it has one to break out.
    let extra = spawn_pane(&mut c);
    assert_eq!(pane_count(&mut c), 2, "window 0 now has two panes");

    // break-pane PANE: move it out into a NEW window (born current); the new name prints.
    let broke = sprag(&sock, &["break-pane", "-t", "0", &extra.to_string()]);
    assert!(broke.ok, "break-pane succeeded: {}", broke.stderr);
    let new_window = broke.stdout.trim().to_owned();
    assert!(
        window_names(&mut c).contains(&new_window),
        "the new window {new_window:?} exists: {:?}",
        window_names(&mut c),
    );
    assert_eq!(
        pane_count(&mut c),
        1,
        "the new (current) window holds only the moved pane",
    );

    // join-pane PANE WINDOW: move it back into "0"; the emptied source (the new window) closes.
    let joined = sprag(&sock, &["join-pane", "-t", "0", &extra.to_string(), "0"]);
    assert!(joined.ok, "join-pane succeeded: {}", joined.stderr);
    assert!(
        joined.stdout.contains("source window closed"),
        "the emptied source closed: {}",
        joined.stdout,
    );
    assert!(
        !window_names(&mut c).contains(&new_window),
        "the emptied source window is gone: {:?}",
        window_names(&mut c),
    );
    assert_eq!(pane_count(&mut c), 2, "both panes back in window 0");

    // join-pane to a non-existent window is a clean refusal — and it names the DESTINATION, which
    // is the one of this verb's three arguments that was wrong. It used to name all three
    // (*"no window named x, no pane N, or it already lives there"*), two of which were false here.
    let ghost = sprag(&sock, &["join-pane", "-t", "0", &extra.to_string(), "nope"]);
    assert!(!ghost.ok, "an absent destination is refused");
    assert_eq!(
        ghost.stderr.trim(),
        "sprag: no window named \"nope\"",
        "clean refusal, naming only what was observed",
    );
    assert!(
        !ghost.stderr.contains(&extra.to_string()),
        "and it does NOT cast doubt on the pane, which resolved: {}",
        ghost.stderr,
    );
}

/// Spawn a `cat` pane into the current window of session "0" over the wire, returning its id.
fn spawn_pane(conn: &mut HostConn) -> u64 {
    conn.call(
        "scene/invoke",
        json!({ "session": "0", "path": mux_action_path(SPAWN_ACTION), "args": { "cmd": ["cat"] } }),
    )
    .expect("spawn a pane")
    .as_u64()
    .expect("spawn returns the pane id")
}

/// How many panes the current window of session "0" holds, over the mux `panes` slot.
fn pane_count(conn: &mut HostConn) -> usize {
    conn.call(
        "scene/query",
        json!({ "session": "0", "path": mux_action_path(PANES_SLOT) }),
    )
    .ok()
    .and_then(|v| v.as_array().map(Vec::len))
    .unwrap_or(0)
}

/// The window names of session "0", over the mux `windows` slot.
fn window_names(conn: &mut HostConn) -> Vec<String> {
    conn.call(
        "scene/query",
        json!({ "session": "0", "path": mux_action_path(WINDOWS_SLOT) }),
    )
    .ok()
    .and_then(|v| {
        v.as_array().map(|arr| {
            arr.iter()
                .filter_map(|w| w["name"].as_str().map(str::to_owned))
                .collect()
        })
    })
    .unwrap_or_default()
}

/// Killing a session's LAST window ends the SESSION (tmux) — driven over the CLI: `kill-window`
/// with no target kills the current (only) window, and the session is then gone.
#[test]
fn the_cli_kill_window_on_the_last_window_ends_the_session() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "created a session");

    // kill-window with no window ⇒ the current (and only) one; its removal ends the session.
    let run = sprag(&sock, &["kill-window", "-t", "work"]);
    assert!(
        run.ok,
        "kill-window on the last window succeeded: {}",
        run.stderr
    );
    // ...AND SAYS SO (R309). Until then this printed `killed the current window` whether it had
    // ended a window or the session the caller was attached to — the same words for the two
    // outcomes a person most needs told apart. The escalation is the daemon's word, rendered by the
    // renderer `kill-pane` and `kill-session` share.
    assert_eq!(
        run.stdout.trim(),
        "killed the current window — the session went with it",
        "the kill names the level past the one that was asked for",
    );

    let ls = sprag(&sock, &["ls"]);
    assert!(
        !ls.stdout.contains("work"),
        "the session went with its last window: {}",
        ls.stdout,
    );
    assert!(
        ls.stdout.contains("0:"),
        "the default survives (work was not the last session)",
    );
}

/// `attach` PRE-FLIGHTS (does the session exist?) over the same connect-only path, then launches
/// the GUI scoped to that session and pinned to THIS daemon's socket. `/usr/bin/env` stands in for
/// the real GUI window (prints its env, exits 0), so the launch + env propagation are provable
/// without a display.
#[test]
fn the_cli_attach_preflights_then_launches_the_gui_scoped_to_the_session() {
    let (_host, sock) = spawn_host();
    assert!(
        sprag(&sock, &["new", "work"]).ok,
        "created a session to attach to"
    );

    // A missing session is a CLEAN pre-flight error — the "no session" message, NOT a
    // "could not launch sprag-gui" one, which proves no GUI was launched on the bad name.
    let bad = sprag_env(
        &sock,
        &["attach", "ghost"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env")],
    );
    assert!(!bad.ok, "attach to a missing session fails");
    assert!(
        bad.stderr.contains("no session named"),
        "clean pre-flight error, no gui launch: {}",
        bad.stderr,
    );

    // No name is an argument error.
    let noarg = sprag(&sock, &["attach"]);
    assert!(!noarg.ok, "attach with no name fails");
    assert!(
        noarg.stderr.contains("needs a session name"),
        "arg error: {}",
        noarg.stderr,
    );

    // A real session: the pre-flight passes and the GUI is launched with the session and THIS
    // socket in its env (the stand-in inherits the CLI's stdout, so its env dump is captured here).
    // ⚠ `SPRAG_GUI_NEW` is EXPORTED here on purpose, and the assertion below is worthless without
    // it: a check that the word is absent from the child's environment passes for free when the
    // parent never had it, which is the vacuous shape a mutation caught in this very file. The run
    // stands in for a person whose shell has been through `sprag new -a`.
    let ok = sprag_env(
        &sock,
        &["attach", "work"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env"), ("SPRAG_GUI_NEW", "1")],
    );
    assert!(
        ok.ok,
        "attach to a real session launches the gui and succeeds: {}",
        ok.stderr
    );
    assert!(
        ok.stdout.contains("SPRAG_GUI_SESSION=work"),
        "the gui is handed the session to adopt: {}",
        ok.stdout,
    );
    assert!(
        ok.stdout
            .contains(&format!("SPRAG_GUI_HOST_SOCK={}", sock.display())),
        "the gui is pinned to THIS daemon's socket, not a default: {}",
        ok.stdout,
    );
    // ⚠ THE COMPLEMENT, and it is not decoration: an inherited *new* word must be REMOVED, or
    // `sprag attach work` run from a shell that `sprag new -a` exported into would create a session
    // instead of joining the named one — an explicit instruction silently reversed by an
    // inheritance. The two words are read by one client and only one of them may be present.
    assert!(
        !ok.stdout.contains("SPRAG_GUI_NEW="),
        "an inherited `new` word does not survive an explicit attach: {}",
        ok.stdout,
    );
}

/// `new -a` launches a WINDOW told to make a session of its OWN — the explicit *new* that a bare
/// launch stopped being when adoption became the default (register items 284 and 368).
///
/// Both halves are asserted because a client is on the other end of exactly two words, and the
/// failure that matters is not the flag going missing — it is the flag arriving BESIDE an inherited
/// `SPRAG_GUI_SESSION`, which the client reads first. So this run deliberately exports the attach
/// word and demands the launcher clear it: a person who ran `sprag attach` in this shell an hour ago
/// must still be able to type `sprag new -a` and get a new session.
///
/// `/usr/bin/env` stands in for the window (prints its env, exits 0) exactly as the attach gate
/// above uses it. The command therefore FAILS — the stand-in never attaches, and `new -a` waits for
/// the daemon to witness a window, which is the `--no-wait` discipline. What is under test is the
/// environment the launch was handed, and that is on stdout either way.
#[test]
fn the_cli_new_attach_launches_a_window_told_to_make_its_own_session() {
    let (_host, sock) = spawn_host();
    // Something to adopt, so that "it was told to create" and "it would have adopted" are
    // distinguishable facts rather than the same empty daemon.
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to adopt");

    let run = sprag_env(
        &sock,
        &["new", "-a"],
        &[
            ("SPRAG_GUI_BIN", "/usr/bin/env"),
            ("SPRAG_GUI_SESSION", "work"),
        ],
    );
    assert!(
        run.stdout.contains("SPRAG_GUI_NEW=1"),
        "the window is told to make a session of its own: {}",
        run.stdout,
    );
    assert!(
        !run.stdout.contains("SPRAG_GUI_SESSION="),
        "an inherited attach word does not survive an explicit `new -a`: {}",
        run.stdout,
    );
    assert!(
        run.stdout
            .contains(&format!("SPRAG_GUI_HOST_SOCK={}", sock.display())),
        "the window is pinned to THIS daemon's socket, not a default: {}",
        run.stdout,
    );

    // A NAME with `-a` is refused rather than accepted and dropped: the window creates the session,
    // so this command has nowhere to put the name. The message must say what to run instead.
    let named = sprag_env(
        &sock,
        &["new", "spare", "-a"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env")],
    );
    assert!(!named.ok, "a named `new -a` is refused");
    assert!(
        named.stderr.contains("sprag new spare") && named.stderr.contains("sprag attach spare"),
        "the refusal names the two commands that do it: {}",
        named.stderr,
    );
    // And the refusal came BEFORE anything was launched — no env dump on stdout at all.
    assert!(
        !named.stdout.contains("SPRAG_GUI_NEW"),
        "nothing was launched for the refused form: {}",
        named.stdout,
    );
}

/// `--no-wait` returns on the window ATTACHING, not on merely having been spawned — so a window
/// that dies on startup is still this command's failure. That is the whole reason the flag waits
/// for the daemon to witness the client rather than exiting 0 the moment `spawn` succeeds.
///
/// `/usr/bin/env` is the stand-in for exactly that failure: it starts, prints, and exits without
/// ever speaking to the daemon — indistinguishable, from the CLI's side, from a real window that
/// cannot open a display. The pass condition is that the CLI NOTICES.
///
/// The success path needs a window that really attaches, so it is proven live rather than here
/// (a stand-in that speaks the client handshake would be asserting against a mock of the thing
/// under test); measured 4/4 attaching in 0.13-0.22s.
#[test]
fn attach_no_wait_reports_a_window_that_exits_before_attaching() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to attach to");

    let run = sprag_env(
        &sock,
        &["attach", "work", "--no-wait"],
        &[("SPRAG_GUI_BIN", "/usr/bin/env")],
    );
    assert!(
        !run.ok,
        "a window that never attached is a FAILURE, not a silent exit 0: {}",
        run.stdout,
    );
    assert!(
        run.stderr.contains("before its window attached"),
        "and it is diagnosed as such, not as a timeout: {}",
        run.stderr,
    );
}

/// An unknown flag is refused rather than swallowed as the session name — otherwise a typo
/// (`--nowait`) would attach to a session by that name, or fail with a confusing "no session".
#[test]
fn attach_refuses_an_unknown_argument() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to attach to");

    let run = sprag(&sock, &["attach", "work", "--nowait"]);
    assert!(!run.ok, "a misspelled flag fails");
    assert!(
        run.stderr.contains("unexpected argument"),
        "named as the argument error it is: {}",
        run.stderr,
    );
}

/// `attach --tui` PRE-FLIGHTS like the window client, then launches the TERMINAL client with the
/// same session + socket env. `/usr/bin/env` stands in for `sprag-tui` (prints its env, exits 0),
/// which is what makes the launch provable without a terminal to take.
///
/// `SPRAG_GUI_BIN` is pointed at the system's `false` throughout — reached by name through
/// [`sprag_gate::doubles::system`], because macOS keeps it in `/usr/bin` and has no `/bin/false`.
/// That is the half of this test that distinguishes "the flag chose the terminal client" from "a
/// client was launched" — a pass here cannot be produced by launching the wrong one. MEASURED, by
/// making the launch resolve `SPRAG_GUI_BIN` whatever the flag says: exit 1 with NOTHING on either
/// stream, because `exec` leaves no CLI behind to say what it launched. The silence is the point —
/// the flag is not something a diagnostic could recover from being wrong about.
#[test]
fn the_cli_attach_tui_launches_the_terminal_client_scoped_to_the_session() {
    let (_host, sock) = spawn_host();
    assert!(
        sprag(&sock, &["new", "work"]).ok,
        "created a session to attach to"
    );
    let never = sprag_gate::doubles::system("false");
    let never = never
        .to_str()
        .expect("the system's `false` has a utf-8 path");
    let clients = [("SPRAG_TUI_BIN", "/usr/bin/env"), ("SPRAG_GUI_BIN", never)];

    // The pre-flight is the terminal client's too: a missing session is the "no session" error,
    // NOT a launch failure — which is what proves nothing was exec'd on a bad name.
    let bad = sprag_env(&sock, &["attach", "ghost", "--tui"], &clients);
    assert!(!bad.ok, "attach --tui to a missing session fails");
    assert!(
        bad.stderr.contains("no session named"),
        "clean pre-flight error, no client launched: {}",
        bad.stderr,
    );

    let ok = sprag_env(&sock, &["attach", "work", "--tui"], &clients);
    assert!(
        ok.ok,
        "attach --tui to a real session launches the terminal client: {} / {}",
        ok.stdout, ok.stderr,
    );
    assert!(
        ok.stdout.contains("SPRAG_GUI_SESSION=work"),
        "the terminal client is handed the session to adopt: {}",
        ok.stdout,
    );
    assert!(
        ok.stdout
            .contains(&format!("SPRAG_GUI_HOST_SOCK={}", sock.display())),
        "and pinned to THIS daemon's socket, not a default: {}",
        ok.stdout,
    );
}

/// `--no-wait` is refused with `--tui`, by NAME, rather than accepted and ignored.
///
/// The flag exists to hand a shell back once a window is up. A terminal client holds this terminal
/// until it detaches, so there is no such moment — and accepting it silently would promise a
/// prompt that never returns.
#[test]
fn attach_tui_refuses_no_wait() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a session to attach to");

    let run = sprag_env(
        &sock,
        &["attach", "work", "--tui", "--no-wait"],
        &[("SPRAG_TUI_BIN", "/usr/bin/env")],
    );
    assert!(!run.ok, "--no-wait with a terminal client fails");
    assert!(
        run.stderr
            .contains("--no-wait belongs to the window client"),
        "refused by name, with the reason: {}",
        run.stderr,
    );
}

/// `attach --remote HOST NAME` runs the terminal client ON HOST: it execs
/// `ssh -t HOST sprag attach --tui NAME` and never opens a local connection.
///
/// Both halves are asserted by ONE arrangement — no daemon is spawned at all, and the session
/// named exists nowhere. If the local pre-flight ran, this could only fail (there is no daemon on
/// that socket to answer, and no session by that name if there were), so reaching the recorded
/// argv IS the proof that `--remote` is answered before any of it.
///
/// The argv is compared as an exact SEQUENCE rather than a set of tokens: `sprag attach --tui
/// ghost` and `sprag --tui attach ghost` carry the same tokens and only one of them is a command.
/// It needs no polling either — `exec` makes the stand-in this process, so the CLI run returning
/// IS the stand-in having finished writing.
#[test]
fn the_cli_attach_remote_execs_ssh_and_never_touches_a_local_daemon() {
    let (_tmp, dir, argv_file) = stub_ssh("attach-remote", |_| "exit 0".to_owned());
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    // A socket nothing serves: the local daemon is not merely unused here, it does not exist.
    let sock = socket_path();

    let run = sprag_env(
        &sock,
        &["attach", "ghost", "--remote", "me@server"],
        &[("PATH", path.as_str())],
    );
    assert!(
        run.ok,
        "attach --remote needs no local daemon and no local session: {}",
        run.stderr,
    );

    let recorded: Vec<String> = std::fs::read_to_string(&argv_file)
        .expect("the stand-in ssh recorded its argv")
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(
        recorded,
        vec!["-t", "me@server", "sprag", "attach", "--tui", "ghost"],
        "the remote command is the terminal client, named, under a forced pty",
    );
    assert!(
        !sock.exists(),
        "no daemon was connected to or spawned on the local socket",
    );
}

/// `--remote` with no host is a clean argument error, not a host silently read off the next thing
/// in the line (which would be the session name, producing an ssh to a machine named for it).
#[test]
fn attach_remote_needs_a_host() {
    let (_host, sock) = spawn_host();

    let run = sprag(&sock, &["attach", "work", "--remote"]);
    assert!(!run.ok, "a valueless --remote fails");
    assert!(
        run.stderr.contains("--remote needs a host"),
        "with a message naming the missing value: {}",
        run.stderr,
    );
}

/// `list-clients` + the `ls` attached count, END TO END over the real socket (R-PR67): the CLI
/// reads the daemon's live per-client attachment state, so this pins the CLI's parse + wire read +
/// formatting against a REAL attached client — not a mocked slot.
///
/// The test itself opens a `HostConn`, announces a client id (`client/hello`) and attaches to the
/// default session (`client/attach`), then holds that connection open across the CLI runs. While
/// it is held: `sprag ls` shows `(1 attached)` on that session, `sprag list-clients` lists the
/// client -> session line, and `-t` filters by session (a match keeps it, an unknown session is a
/// clean pre-flight error). When the connection DROPS, the daemon releases the attachment
/// (`on_disconnect`, crash-safe), and `list-clients` empties + `ls` loses the badge — polled,
/// because the release is delivered asynchronously on the daemon's reader thread.
#[test]
fn the_cli_lists_attached_clients_and_shows_the_attached_count() {
    let (_host, sock) = spawn_host();

    // With no attached client, list-clients is empty (exit 0) and ls carries no badge.
    let empty = sprag(&sock, &["list-clients"]);
    assert!(
        empty.ok,
        "list-clients with no clients succeeds: {}",
        empty.stderr
    );
    assert!(
        empty.stdout.trim().is_empty(),
        "no clients ⇒ no lines: {:?}",
        empty.stdout,
    );
    assert!(
        !sprag(&sock, &["ls"]).stdout.contains("attached"),
        "an unviewed session shows no attached badge",
    );

    {
        // Attach a real client to the default session "0" and HOLD the connection open.
        let mut attacher =
            HostConn::connect(&sock, Duration::from_secs(5)).expect("attacher connects");
        attacher
            .call(
                CLIENT_HELLO_METHOD,
                json!({ CLIENT_PARAM: "cli-test-client" }),
            )
            .expect("client/hello accepted");
        attacher
            .call(CLIENT_ATTACH_METHOD, json!({}))
            .expect("client/attach accepted");

        // The attach is delivered on the daemon's dispatch thread; poll the CLI until it shows.
        assert!(
            wait_for(Duration::from_secs(5), || {
                sprag(&sock, &["list-clients"])
                    .stdout
                    .contains("cli-test-client: 0")
            }),
            "list-clients lists the attached client and its session",
        );

        let ls = sprag(&sock, &["ls"]);
        assert!(
            ls.stdout.contains("(1 attached)"),
            "ls shows the attached count on the viewed session: {}",
            ls.stdout,
        );

        // -t filters by session: the default "0" keeps the client...
        let matched = sprag(&sock, &["list-clients", "-t", "0"]);
        assert!(matched.ok, "list-clients -t 0 succeeds: {}", matched.stderr);
        assert!(
            matched.stdout.contains("cli-test-client: 0"),
            "the client attached to 0 survives the -t 0 filter: {}",
            matched.stdout,
        );
        // ...and an unknown session is a clean pre-flight error, not an empty success.
        let unknown = sprag(&sock, &["list-clients", "-t", "ghost"]);
        assert!(!unknown.ok, "list-clients -t <missing> fails");
        assert!(
            unknown.stderr.contains("no session named"),
            "a clean pre-flight error: {}",
            unknown.stderr,
        );

        // attacher drops here: its socket closes with no explicit detach.
    }

    // The released attachment must empty the listing and drop the badge (crash-safe on_disconnect).
    assert!(
        wait_for(Duration::from_secs(5), || {
            sprag(&sock, &["list-clients"]).stdout.trim().is_empty()
                && !sprag(&sock, &["ls"]).stdout.contains("attached")
        }),
        "closing the connection releases the attachment, not leaks it",
    );
}

/// KILLING a session releases its viewers, as `sprag list-clients` and `sprag ls` report them —
/// the operator-facing half of the same fact, over the real socket and the real CLI.
///
/// **This was measured WRONG at `2402e62`, and both surfaces lied**: after `sprag kill-session
/// alpha`, `list-clients` went on printing `probe-kill: alpha` — a viewer of a session the registry
/// no longer held — and once a NEW session took the freed name, `sprag ls` credited it with
/// `(1 attached)` for a client that had never seen it. Nothing about the client was at fault; the
/// attachment simply outlived the thing it named.
///
/// The connection is deliberately HELD open across the kill, because the release under test is the
/// SESSION's death and not the connection's: letting the socket drop would release the attachment
/// through `on_disconnect` and this test would pass with the kill path doing nothing at all.
#[test]
fn killing_a_session_releases_its_viewers_and_a_new_session_of_that_name_inherits_none() {
    let (_host, sock) = spawn_host();
    // A second session, so killing the first does not end the daemon.
    assert!(sprag(&sock, &["new", "alpha"]).ok, "create alpha");
    assert!(sprag(&sock, &["new", "keeper"]).ok, "create keeper");

    let mut attacher = HostConn::connect(&sock, Duration::from_secs(5)).expect("attacher connects");
    attacher
        .call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: "viewer" }))
        .expect("client/hello accepted");
    attacher
        .call(
            CLIENT_ATTACH_METHOD,
            json!({ sprag_rpc::SESSION_PARAM: "alpha" }),
        )
        .expect("client/attach accepted");
    assert!(
        wait_for(Duration::from_secs(5), || {
            sprag(&sock, &["list-clients"])
                .stdout
                .contains("viewer: alpha")
        }),
        "the control: while alpha lives, its viewer is listed",
    );

    assert!(sprag(&sock, &["kill-session", "alpha"]).ok, "kill alpha");
    assert!(
        wait_for(Duration::from_secs(5), || {
            sprag(&sock, &["list-clients"]).stdout.trim().is_empty()
        }),
        "the killed session's viewer is released, not left naming a session that is gone: {}",
        sprag(&sock, &["list-clients"]).stdout,
    );

    // The inheritance: a new session takes the freed name and must be credited with nobody.
    assert!(sprag(&sock, &["new", "alpha"]).ok, "re-create alpha");
    let ls = sprag(&sock, &["ls"]);
    assert!(
        !ls.stdout.contains("attached"),
        "a fresh session of the retired name inherits no viewer: {}",
        ls.stdout,
    );
    drop(attacher);
}

/// Poll `predicate` until it holds or `timeout` elapses. The CLI's per-client attachment reads
/// depend on the daemon's async `on_disconnect`, so a populated/emptied assertion is polled, not
/// asserted once.
fn wait_for(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    predicate()
}

/// `sprag find` end to end: the CLI sweeps the session's panes through the host's `find.<needle>`
/// query and prints each matching line as `PANE:LINE: text`.
///
/// The boot pane prints two lines, one of which matches twice — so this pins the two properties a
/// grep-shaped output has to get right: the matching line appears ONCE (deduped, not once per
/// match), and the non-matching line does not appear at all. The needle carries a SPACE, which also
/// proves the path-carried argument survives the wire verbatim from a shell argument.
#[test]
fn the_cli_find_prints_matching_lines_from_the_session() {
    let (_host, sock) =
        spawn_host_running(&["sh", "-c", "printf 'a hit and a hit\\nquiet\\n'; exec cat"]);

    // The pane's output is asynchronous, so poll the search itself until it sees the line.
    let mut run = sprag(&sock, &["find", "a hit"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !run.stdout.contains("a hit and a hit") && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        run = sprag(&sock, &["find", "a hit"]);
    }
    assert!(run.ok, "sprag find succeeded: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(
        lines,
        vec!["0:0: a hit and a hit"],
        "one line per MATCHING line — deduped, and the quiet line is absent: {:?}",
        run.stdout,
    );

    // A needle nothing matches is not an error: it prints nothing and exits 0, so a script can
    // tell "the search ran" from "something broke".
    let empty = sprag(&sock, &["find", "zzz-no-such-text"]);
    assert!(empty.ok, "a search with no matches still succeeds");
    assert!(
        empty.stdout.is_empty(),
        "and prints nothing: {:?}",
        empty.stdout
    );

    // A missing needle is a clean local error, before any request.
    let bare = sprag(&sock, &["find"]);
    assert!(!bare.ok, "find with no needle fails");
    assert!(
        bare.stderr.contains("needle is required"),
        "with a clear message: {}",
        bare.stderr,
    );
}

/// `sprag find` at the SHELL mouth, for a line the pane broke at its right edge — and what it
/// prints for one.
///
/// The three mouths answer from ONE search, so this is not a re-test of the traversal (that is
/// `sprag-vt`'s) but of what a person at a terminal gets: one `PANE:LINE: text` row carrying the
/// whole logical line, keyed on the row the LINE starts at. Printing the pane's rows instead would
/// hand a script the word broken in half — the blindness one layer up from the search's own.
#[test]
fn the_cli_find_reads_a_line_the_pane_broke_at_its_edge() {
    // Twenty columns and a 24-character marker: one logical line over two rows.
    let (_host, sock) = spawn_host_sized(
        20,
        6,
        &["sh", "-c", "printf 'the-build-is-done-now-ok\\n'; exec cat"],
    );

    let mut run = sprag(&sock, &["find", "done-now"]);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while run.stdout.is_empty() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        run = sprag(&sock, &["find", "done-now"]);
    }
    assert!(run.ok, "sprag find succeeded: {}", run.stderr);
    assert_eq!(
        run.stdout.lines().collect::<Vec<_>>(),
        vec!["0:0: the-build-is-done-now-ok"],
        "one row of output carrying the WHOLE line, not the twenty columns that fit on a row: {:?}",
        run.stdout,
    );
}

/// Wait until the stand-in's recorded argv carries EVERY token in `expected`, panicking with what it
/// did record if it never does.
///
/// Polling `argv_file.exists()` and then reading it is a race, not a wait: the stub's
/// `printf … > file` CREATES (truncates) the file at the redirect, before it writes a single byte, so
/// an existence poll can hand the assertion an empty file. The window is narrow enough to pass alone
/// and wide enough to fail under a loaded parallel run — which is how it surfaced. Waiting on the
/// condition the assertion actually reads removes the race instead of widening the timeout.
fn wait_for_recorded_argv(argv_file: &Path, expected: &[&str]) {
    let mut recorded = String::new();
    let complete = wait_for(Duration::from_secs(5), || {
        recorded = std::fs::read_to_string(argv_file).unwrap_or_default();
        expected
            .iter()
            .all(|token| recorded.lines().any(|line| line == *token))
    });
    assert!(
        complete,
        "the stand-in never recorded the argv {expected:?}; it recorded {recorded:?}",
    );
}

/// A temp directory removed on drop (including on a panicked assertion) — holds an ssh test's
/// stand-in `ssh` and the argv it records, so a failed run leaves nothing behind.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Create a temp dir holding an executable stand-in `ssh` that records its exec argv (one token per
/// line) to `<dir>/argv.txt`, then runs the shell fragment `tail(dir)` — `exec cat` to block like a
/// live pane, or a listener to simulate `ssh -L`. `tail` receives the dir so it can reference sibling
/// files by absolute path (the pane is exec'd via PATH, so `$0`-relative paths are unreliable).
/// Returns the drop guard, the dir (prepend to the daemon PATH), and the argv-record path.
///
/// ⚠⚠⚠⚠ **The stub is LINKED from a tracked file and the per-case tail is a DATA file beside it** —
/// register item 467. It used to be composed here and the DAEMON then exec'd it: a file any process
/// holds open for writing cannot be executed (`ETXTBSY`), and this harness runs its cases on THREADS
/// of one process, so a sibling forking to spawn a program inherits the write handle and holds it
/// until its own exec. Item 465 measured that shape on `sprag-gate` at 10 failures in 30 runs.
/// `tail.sh` is SOURCED by the stub rather than executed, so it carries none of the window.
fn stub_ssh(label: &str, tail: impl FnOnce(&Path) -> String) -> (TempDir, PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("sprag-ssh-it-{}-{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the stand-in ssh dir");
    let argv_file = dir.join("argv.txt");
    std::fs::write(dir.join("tail.sh"), format!("{}\n", tail(&dir)))
        .expect("what the stand-in ssh is to do after it has recorded its argv");
    sprag_gate::doubles::Doubles::of(env!("CARGO_MANIFEST_DIR"))
        .set("cli")
        .link("ssh", &dir.join("ssh"));
    (TempDir(dir.clone()), dir, argv_file)
}

/// `sprag ssh` end to end over the socket. The CLI parses `me@server -p 2222`, builds the `ssh -t …`
/// argv ([`sprag_host::SshTarget`]), and the daemon spawns it as the birth pane of a FRESH session —
/// no ssh-awareness anywhere on the wire, it rides the existing `new_session {cmd}` action. The
/// stand-in `ssh` records the exact argv it is exec'd with, then blocks (`exec cat`) so the pane
/// stays live and the assertion is deterministic (not racing a real ssh's connect-and-die). This
/// pins the WHOLE chain — parse → `ssh_argv` → `new_session {cmd}` → `build_command` → PTY exec —
/// reaching exec with the real arguments; the argv *shape* itself is the [`sprag_host::ssh`] unit
/// tests' job.
#[test]
fn the_cli_ssh_launches_a_remote_pane_with_the_ssh_argv() {
    let (_tmp, dir, argv_file) = stub_ssh("launch", |_| "exec cat".to_owned());

    // The DAEMON spawns the pane, so the stand-in must be first on ITS PATH.
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path)]);

    // ssh me@server -p 2222: creates a session, printing the allocated name to scope a client to.
    let run = sprag(&sock, &["ssh", "me@server", "-p", "2222"]);
    assert!(run.ok, "sprag ssh succeeded: {}", run.stderr);
    let name = run.stdout.trim().to_owned();
    assert!(
        !name.is_empty() && name != "0",
        "ssh prints a fresh allocated session name: {name:?}",
    );

    // The birth pane exec'd the stand-in ssh with the real argv (async spawn — poll for the record).
    wait_for_recorded_argv(&argv_file, &["-t", "-p", "2222", "me@server"]);

    // The live remote pane keeps the session listable (panes > 0), like any other workspace.
    assert!(
        sprag(&sock, &["ls"]).stdout.contains(&name),
        "the ssh session lists as a live workspace",
    );
}

/// `sprag ssh -L` reaches exec with the forwards ssh's SSOT rendered — the one-field shorthand
/// `3000` EXPANDED to `3000:localhost:3000`, the three-field spec verbatim. Deterministic (`exec cat`
/// stand-in, no listener), so it pins the CLI→argv→exec path for forwards; the live "the forward
/// surfaces in the sidebar" composition is the next test.
#[test]
fn the_cli_ssh_passes_local_forwards_to_exec() {
    let (_tmp, dir, argv_file) = stub_ssh("fwd", |_| "exec cat".to_owned());
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path)]);

    let run = sprag(&sock, &["ssh", "host", "-L", "3000", "-L", "8080:db:5432"]);
    assert!(run.ok, "sprag ssh -L succeeded: {}", run.stderr);

    wait_for_recorded_argv(&argv_file, &["-L", "3000:localhost:3000", "8080:db:5432"]);
}

/// The headline of Slice 2, LIVE: a remote workspace's forwarded local port shows up in the session
/// sidebar (`sprag ls`). Real `ssh -L PORT:…` opens a local listener on the ssh process; here a
/// python stand-in binds that same local port and holds it, so the daemon's per-pane `/proc` port
/// scan attributes it to the ssh session — no ssh-specific code in the scan, an ssh pane is just a
/// pane. Proves the composition end to end (`/proc` attribution itself is unit-covered in
/// `sprag_terminal::ports`). Linux-only, like the port scan; needs python3 to hold the socket.
// ⚠ LINUX ONLY, and unlike its neighbours this one has not moved: what it drives is the LISTENING
// PORTS reader — `/proc/net/tcp` plus a walk of `/proc/<pid>/fd`, with no cheap macOS counterpart
// (`proc_pidfdinfo`, per descriptor). R343 ungated it by SWEEPING FOR THE PATTERN rather than
// reading each site's subject — **the doc comment right above says "Linux-only, like the port
// scan"** — and the macOS runner said so within the hour.
#[cfg(target_os = "linux")]
#[test]
fn the_cli_ssh_forward_surfaces_the_local_port_in_the_sidebar() {
    let has_python = Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !has_python {
        eprintln!(
            "skipping ssh-forward-surfaces: python3 is needed to hold the forwarded listener"
        );
        return;
    }

    // A free local port: bind :0 to learn a number, then release it (SO_REUSEADDR lets python rebind).
    let free_port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind an ephemeral loopback port")
        .local_addr()
        .unwrap()
        .port();

    // The stand-in ssh execs a python listener on $LISTEN_PORT (what real `ssh -L` would bind).
    let (_tmp, dir, argv_file) = stub_ssh("surface", |dir| {
        format!(
            "exec python3 '{}' \"$LISTEN_PORT\"",
            dir.join("listener.py").display(),
        )
    });
    std::fs::write(
        dir.join("listener.py"),
        "import socket, sys, signal\n\
         s = socket.socket()\n\
         s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)\n\
         s.bind((\"127.0.0.1\", int(sys.argv[1])))\n\
         s.listen()\n\
         signal.pause()\n",
    )
    .expect("write the python listener");

    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path), ("LISTEN_PORT", &free_port.to_string())]);

    // Forward the free local port to the remote's :22 (the remote target is irrelevant to the LOCAL
    // listen this test observes).
    let spec = format!("{free_port}:localhost:22");
    let run = sprag(&sock, &["ssh", "host", "-L", &spec]);
    assert!(run.ok, "sprag ssh -L succeeded: {}", run.stderr);
    let name = run.stdout.trim().to_owned();

    // The -L reached exec (deterministic argv record).
    wait_for_recorded_argv(&argv_file, &[&spec]);

    // The live composition: the forwarded port, held by the ssh (python) process in the pane's
    // subtree, surfaces on THIS session's `sprag ls` line. Polled — the listener binds asynchronously
    // and the port scan is a live per-read.
    let badge = format!(":{free_port}");
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["ls"])
                .stdout
                .lines()
                .any(|line| line.starts_with(&format!("{name}:")) && line.contains(&badge))
        }),
        "the forwarded port {free_port} surfaces on the ssh session in `sprag ls`",
    );
}

/// The `--tmux` preset reaches exec as `tmux new-session -A -s NAME` (attach-or-create), and clashing
/// it with a `--` remote command is a clean LOCAL error surfaced before anything is sent — the parse
/// runs ahead of the connect.
#[test]
fn the_cli_ssh_tmux_preset_reaches_exec_and_rejects_a_conflict() {
    let (_tmp, dir, argv_file) = stub_ssh("tmux", |_| "exec cat".to_owned());
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_host, sock) = spawn_host_env(&[("PATH", &path)]);

    // --tmux=work: the birth pane execs the attach-or-create tmux command on the remote.
    let run = sprag(&sock, &["ssh", "host", "--tmux=work"]);
    assert!(run.ok, "sprag ssh --tmux succeeded: {}", run.stderr);
    wait_for_recorded_argv(&argv_file, &["tmux", "new-session", "-A", "-s", "work"]);

    // --tmux together with a -- command is a clean, non-zero local error (nothing spawned).
    let clash = sprag(&sock, &["ssh", "host", "--tmux", "--", "vim"]);
    assert!(!clash.ok, "combining --tmux and a -- command fails");
    assert!(
        clash.stderr.contains("--tmux") && clash.stderr.contains("not both"),
        "a clean conflict message: {}",
        clash.stderr,
    );
}

/// `--pane` narrows the sweep to ONE pane, and naming a pane that is nowhere in the session is a
/// clean ERROR rather than an empty result.
///
/// The asymmetry is the point: finding no matches for a needle IS the answer, but finding no
/// matches for a pane that is not there answers a question the caller did not ask. Two panes print
/// the SAME needle so the filter has something to exclude — a one-pane fixture would pass whether
/// the filter worked or not.
///
/// ⚠ It said *"a pane the WINDOW does not hold"* until 2026-08-17, and every pane it names is one
/// that exists NOWHERE (`9999`, `abc`) — so the sentence made a claim about the one case it never
/// ran. A pane one window over resolves and IS searched;
/// `find_narrowed_to_a_pane_reaches_a_window_the_sweep_does_not` is where that lives now.
#[test]
fn the_cli_find_narrows_to_one_pane_and_rejects_an_absent_one() {
    let printer = "printf 'shared marker\\n'; exec cat";
    let (_host, sock) = spawn_host_running(&["sh", "-c", printer]);
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    let second: u64 = conn
        .call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "cmd": ["sh", "-c", printer] },
            }),
        )
        .expect("spawn a second pane")
        .as_u64()
        .expect("spawn returns the pane id");

    // Both panes match, once the second one has printed (its output is asynchronous).
    let mut all = sprag(&sock, &["find", "shared marker"]);
    let both = wait_for(Duration::from_secs(5), || {
        all = sprag(&sock, &["find", "shared marker"]);
        all.stdout.lines().count() == 2
    });
    assert!(both, "both panes matched: {:?}", all.stdout);

    // --pane keeps only that pane's lines.
    let only = sprag(
        &sock,
        &["find", "shared marker", "--pane", &second.to_string()],
    );
    assert!(only.ok, "find --pane succeeded: {}", only.stderr);
    assert_eq!(
        only.stdout.trim_end(),
        format!("{second}:0: shared marker"),
        "only the named pane's line: {:?}",
        only.stdout,
    );

    // An absent pane is an error that says what IS there, not an empty success.
    let absent = sprag(&sock, &["find", "shared marker", "--pane", "9999"]);
    assert!(!absent.ok, "an absent pane fails");
    assert!(
        absent.stderr.contains("no pane 9999"),
        "and names it: {}",
        absent.stderr,
    );

    // A non-numeric `--pane` is a NAME now, so it is resolved rather than rejected by shape — and
    // the refusal names what IS in the session, which a local shape check could never do.
    let bad = sprag(&sock, &["find", "shared marker", "--pane", "abc"]);
    assert!(!bad.ok, "a name no pane carries fails");
    assert!(
        bad.stderr.contains("no pane is called \"abc\""),
        "with a clear message: {}",
        bad.stderr,
    );
}

/// **NARROWING `find` TO A PANE MAKES IT FIND MORE, NOT LESS** — the sweep stops at the current
/// window and `--pane` reaches any window of the session.
///
/// # Why this is pinned rather than fixed
///
/// It is a DECISION, and the code says so where it is made: *"the narrowed form resolves
/// session-wide (a name reaches any window), and the sweep does not: searching every window would
/// change what an unnarrowed `sprag find` means for every caller that has one."* R312 widened pane
/// RESOLUTION to the whole session for every verb; the sweep was deliberately left where it was.
///
/// What was NOT deliberate is that nothing measured it. The neighbouring test above says in its own
/// doc that *"naming a pane the window does not hold is a clean ERROR"* and then only ever names
/// panes that exist nowhere at all (`9999`, `abc`) — so the one case that separates the two reaches
/// was never run, and `find`'s doc drifted into claiming both answers at once (item 429).
///
/// # ⚠ If this test goes RED because the sweep now reaches every window
///
/// That is item 429 being PAID, not a regression. Delete the first assertion, keep the rest, and
/// close the item — the whole point of pinning an asymmetry is that changing it must be a decision
/// somebody makes on purpose rather than a side effect nobody noticed.
#[test]
fn find_narrowed_to_a_pane_reaches_a_window_the_sweep_does_not() {
    let marker = "MARKER-IN-WINDOW-ZERO";
    let (_host, sock) =
        spawn_host_running(&["sh", "-c", &format!("printf '{marker}\\n'; exec cat")]);

    // The marker's pane, named, while it is still the current window's.
    let far = sprag(&sock, &["panes", "-t", "0"])
        .stdout
        .lines()
        .next()
        .and_then(|row| row.split(':').next().map(str::to_owned))
        .expect("the boot pane");
    assert!(sprag(&sock, &["rename-pane", &far, "marked", "-t", "0"]).ok);
    // It has to be FOUND before the window moves, or a later empty answer would be the pane never
    // having printed rather than the sweep not reaching it — the control this whole test rests on.
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["find", marker, "-t", "0"])
                .stdout
                .contains(marker)
        }),
        "the sweep finds the marker while its pane is in the CURRENT window: {}",
        sprag(&sock, &["find", marker, "-t", "0"]).stdout,
    );

    // A second window, which `new-window` selects — so the marker's pane is now elsewhere.
    assert!(sprag(&sock, &["new-window", "-t", "0", "elsewhere"]).ok);
    let here = sprag(&sock, &["panes", "-t", "0"]).stdout;
    assert!(
        !here.lines().any(|row| row.starts_with(&format!("{far}:"))),
        "the marked pane must be OUT of the current window or nothing here discriminates: {here}",
    );

    // THE SWEEP STOPS AT THE WINDOW. Read the ⚠ section above before changing this.
    let swept = sprag(&sock, &["find", marker, "-t", "0"]);
    assert!(swept.ok, "the sweep still succeeds: {}", swept.stderr);
    assert!(
        !swept.stdout.contains(marker),
        "an unnarrowed sweep does not cross into another window — if this went red because the \
         sweep now reaches every window, that is register item 429 being PAID: {}",
        swept.stdout,
    );

    // ...AND `--pane` REACHES PAST IT, by id and by the name R312 made addressable. This is the
    // half that makes the line above an asymmetry rather than just a scope.
    for spelling in [far.as_str(), "marked"] {
        let narrowed = sprag(&sock, &["find", marker, "--pane", spelling, "-t", "0"]);
        assert!(
            narrowed.ok,
            "find --pane {spelling} succeeded: {}",
            narrowed.stderr,
        );
        assert_eq!(
            narrowed.stdout.trim_end(),
            format!("{far}:0: {marker}"),
            "--pane {spelling} reaches a window the sweep did not: {}",
            narrowed.stdout,
        );
    }
}

/// `--regex` sends a DIFFERENT query, not the same one with a flag: the same argument matches
/// different things under the two languages, and an invalid pattern is an error rather than a
/// silently empty result.
///
/// The fixture prints `axb` and `a.b` on one line so the literal and pattern readings of `a.b` are
/// distinguishable — one match versus two. A fixture where both readings agree would pass whether
/// the flag reached the wire or not.
#[test]
fn the_cli_find_regex_reads_the_needle_as_a_pattern() {
    let (_host, sock) = spawn_host_running(&["sh", "-c", "printf 'axb a.b\\n'; exec cat"]);

    // Literal: only the real dot matches, so ONE line, printed once.
    let mut literal = sprag(&sock, &["find", "a.b"]);
    let printed = wait_for(Duration::from_secs(5), || {
        literal = sprag(&sock, &["find", "a.b"]);
        literal.stdout.contains("axb a.b")
    });
    assert!(printed, "the pane printed: {:?}", literal.stdout);
    assert_eq!(literal.stdout.trim_end(), "0:0: axb a.b");

    // As a pattern the dot matches any character — the same LINE, so the grep-shaped output is
    // still one line. What proves the language changed is a pattern that is not literal text.
    let pattern = sprag(&sock, &["find", "--regex", "^a.b a"]);
    assert!(pattern.ok, "find --regex succeeded: {}", pattern.stderr);
    assert_eq!(
        pattern.stdout.trim_end(),
        "0:0: axb a.b",
        "an anchored pattern matched: {:?}",
        pattern.stdout,
    );
    // …and the same string as a LITERAL matches nothing, so the flag really did change the query.
    let as_literal = sprag(&sock, &["find", "^a.b a"]);
    assert!(
        as_literal.ok && as_literal.stdout.is_empty(),
        "literally absent"
    );

    // An invalid pattern is an error naming the reason, not an empty success.
    let bad = sprag(&sock, &["find", "--regex", "a(b"]);
    assert!(!bad.ok, "an invalid pattern fails");
    assert!(
        bad.stderr.contains("invalid pattern"),
        "with the engine's reason: {}",
        bad.stderr,
    );
    // The SAME string is a perfectly good literal needle, which is exactly the point.
    let fine = sprag(&sock, &["find", "a(b"]);
    assert!(
        fine.ok,
        "the same string is a valid literal needle: {}",
        fine.stderr
    );
}

/// **THE live gate for `wait-for-output`, and the round that added the verb did not write it.**
///
/// The owner's "did you register all the debt?" is what found the gap: 172 tests here cover the
/// other verbs and this one had none, so the whole CLI half rested on hand-driven runs that left
/// nothing standing. R291's rule is to build the missing assertion rather than register it.
///
/// Three properties, each of which is a property of the PATH and not of the parser: the verb really
/// BLOCKS and is released by the pane's own output; the search reads what the pane KEPT (so a line
/// scrolled far past any recent window still answers); and a refused pattern exits non-zero with
/// the engine's reason rather than succeeding emptily.
#[test]
fn the_cli_waits_for_output_a_pane_has_not_printed_yet() {
    // A pane that says nothing for a moment, then says it — so the wait cannot pass by finding
    // something that was already there when it started.
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "sleep 1; printf 'the-build-is-done\\n'; seq 1 60; exec cat",
    ]);

    let started = Instant::now();
    let waited = sprag(
        &sock,
        &["wait-for-output", "--pane", "0", "the-build-is-done"],
    );
    assert!(waited.ok, "the wait succeeded: {}", waited.stderr);
    assert_eq!(
        waited.stdout.trim_end(),
        "0:0: the-build-is-done",
        "and printed the matching line in find's own format: {:?}",
        waited.stdout,
    );
    // It BLOCKED rather than returning empty: the pane needed a second to get there, and a verb
    // that answered without waiting would have come back at once with nothing.
    assert!(
        started.elapsed() >= Duration::from_millis(500),
        "the verb blocked until the pane produced ({:?})",
        started.elapsed(),
    );

    // THE DISCRIMINATOR. The marker is line 0 of sixty-one and the pane is 24 rows, so a reader
    // that re-read the visible screen — which is what a polling implementation does — would find
    // nothing. Asked again, it answers immediately from the pane's RETAINED output.
    let again = sprag(
        &sock,
        &["wait-for-output", "--pane", "0", "the-build-is-done"],
    );
    assert!(again.ok, "asked again: {}", again.stderr);
    assert_eq!(
        again.stdout.trim_end(),
        "0:0: the-build-is-done",
        "still matched from scrollback: {:?}",
        again.stdout,
    );
    // The control, expressed the only way this SURFACE can express it: the match is line 0 of a
    // capture with sixty-one lines, on a 24-row pane — so it sits at least 37 lines above anything
    // a reader of the recent window could see. `capture-pane` reads `full_text` (scrollback AND
    // visible) and the CLI has no visible-only read at all, so a screen-only comparison is not
    // available here; the assertion that reads the six rows directly is the unit test in
    // `sprag_host::rpc`, and this one measures the DISTANCE instead.
    //
    // ⚠ AND IT HAS TO WAIT FOR THE TAIL FIRST. The boot program is
    // `sleep 1; printf marker; seq 1 60; exec cat`, so the wait above is released by the FIRST line
    // and everything measured below is printed AFTER it — a check that reads the last line having
    // waited for the first one races its own fixture. Seen failing once in a full-suite run
    // (`the pane kept 1 lines`) and passing three times in isolation, which is the signature of a
    // check that is unsound under load rather than of a defect in the verb. Waiting with the same
    // verb the test already trusts is what makes the distance a fact rather than a hope.
    let tail = sprag(&sock, &["wait-for-output", "--pane", "0", "60"]);
    assert!(
        tail.ok,
        "the pane finished printing before the distance is measured: {}",
        tail.stderr,
    );
    let captured = sprag(&sock, &["capture-pane", "0", "-p"]);
    assert!(captured.ok, "capture-pane succeeded: {}", captured.stderr);
    let lines = captured.stdout.lines().count();
    assert!(
        lines >= 60,
        "the pane kept {lines} lines, so line 0 is far outside a 24-row view",
    );
    assert!(
        captured.stdout.starts_with("the-build-is-done"),
        "and line 0 is the marker itself: {:?}",
        &captured.stdout[..captured.stdout.len().min(40)],
    );

    // A refused pattern is an ERROR with the engine's reason, never an empty success — the
    // difference between "your pattern is wrong" and "it has not happened yet".
    let bad = sprag(&sock, &["wait-for-output", "--pane", "0", "--regex", "a(b"]);
    assert!(!bad.ok, "an invalid pattern fails rather than parking");
    assert!(
        bad.stderr.contains("invalid pattern"),
        "with the reason: {}",
        bad.stderr,
    );

    // And the argument checks the caller can act on, each named.
    let no_pane = sprag(&sock, &["wait-for-output", "anything"]);
    assert!(!no_pane.ok && no_pane.stderr.contains("--pane is required"));
    let no_needle = sprag(&sock, &["wait-for-output", "--pane", "0"]);
    assert!(!no_needle.ok && no_needle.stderr.contains("a search needle is required"));
}

// ---------------------------------------------------------------------------
// The durability ring, end to end: a daemon that dies gives its panes back WITH
// their scrollback.
// ---------------------------------------------------------------------------

/// Kills whatever daemon is serving `sock` and removes the socket and the state directory —
/// including on a panicked assertion, so a failed run leaks neither a process nor a temp tree.
struct DaemonGuard {
    sock: PathBuf,
    state: PathBuf,
}

impl Drop for DaemonGuard {
    fn drop(&mut self) {
        if let Some(pid) = daemon_pid(&self.sock) {
            kill_daemon(pid);
        }
        let _ = std::fs::remove_file(&self.sock);
        let _ = std::fs::remove_dir_all(&self.state);
    }
}

/// The pid of the `sprag-term` daemon serving `sock`.
///
/// It has to be FOUND rather than remembered: a `--daemon` forks and the parent exits, so the
/// process the test spawned is a short-lived intermediate, not the daemon. Matching on the
/// environment rather than the command line because the socket travels as `SPRAG_HOST_RPC_SOCK`,
/// and that value is unique per test call — so this cannot pick up a sibling test's daemon the way
/// a `pkill -f sprag-term` would.
fn daemon_pid(sock: &Path) -> Option<u32> {
    let want = format!("SPRAG_HOST_RPC_SOCK={}", sock.display());
    let me = std::process::id();
    // ⚠ THROUGH `sprag_terminal::procfs`, which walks whatever process table this OS has. This used
    // to read `/proc` directly — the directory, each entry's `comm`, each entry's `environ` — so the
    // three tests that need it were `#[cfg(target_os = "linux")]` and the restore-after-SIGKILL path
    // was never once exercised off Linux. Nothing about the QUESTION is Linux-shaped: it is "which
    // process is named sprag-term and was started beside THIS socket", and both halves are portable
    // now (R343).
    //
    // The socket is matched from the ENVIRONMENT rather than the name, because a box can be running
    // the developer's own daemon: a probe that took the first `sprag-term` it found would SIGKILL
    // somebody's terminal (R278, and this test kills what it finds).
    let holding: Vec<u32> = sprag_terminal::procfs::pids_named("sprag-term")
        .into_iter()
        .filter(|&pid| {
            pid != me
                && sprag_terminal::procfs::environ(pid).is_some_and(|environ| {
                    environ
                        .split(|byte| *byte == 0)
                        .any(|value| value == want.as_bytes())
                })
        })
        .collect();
    // ⚠⚠⚠⚠⚠ **THE ONE THAT IS NOBODY'S CHILD, AND THAT STOPPED BEING A FORMALITY ON 2026-08-24.**
    // Until `run-driver-process` defaulted to `on`, exactly one process held this socket in its
    // environment and the first match WAS the daemon. A driver is spawned by the daemon with the
    // same endpoint in its environment (`crate::driver_spawn`) and is the same binary, so it
    // matches every clause above — and the first match became a coin toss between the daemon and
    // whichever of its runs happened to be listed first. Every caller here means the DAEMON: they
    // SIGKILL it to stage a reboot, and killing a driver instead leaves the socket held by a daemon
    // the test then waits for a successor to replace.
    //
    // ⚠ Parentage rather than argv, because `procfs` publishes `parent` and no command line — and
    // it answers the question exactly: a driver's parent is the daemon that spawned it, and the
    // daemon's own parent is the init process that adopted it when its intermediate forked away.
    holding
        .iter()
        .copied()
        .find(|&pid| {
            !sprag_terminal::procfs::parent(pid).is_some_and(|parent| holding.contains(&parent))
        })
        // ⚠ A daemon whose only run's driver outlived it is still the answer nobody's-child gives,
        // so this fallback is for the shape where the walk saw a torn process table, not for a
        // shape the product produces.
        .or_else(|| holding.first().copied())
}

/// SIGKILL `pid` and wait for it to be gone — the reboot analogue. Deliberately not a graceful
/// `kill-server`: the ring exists for the case where the daemon gets NO chance to tidy up, and a
/// clean shutdown would also race its own teardown against the save loop.
fn kill_daemon(pid: u32) {
    // SAFETY: `pid` was just read from /proc for a process this test spawned.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
    wait_for(Duration::from_secs(5), || {
        !Path::new(&format!("/proc/{pid}")).exists()
    });
}

/// Launch a `--daemon` on `sock` keeping its durable state under `state`. Returns once the
/// intermediate parent has forked and exited; the caller waits for the socket to answer.
fn spawn_daemon(sock: &Path, state: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_sprag-term"))
        .arg("--daemon")
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("SPRAG_HOST_RPC", "1")
        .env("XDG_STATE_HOME", state)
        // ⚠⚠⚠⚠⚠ **AND ITS OPTIONS COME FROM A DIRECTORY THIS TEST OWNS, WHICH IS THE SAME RULE THE
        // STATE HOME ABOVE IS HERE FOR.** A daemon reads `$XDG_CONFIG_HOME/sprag/config.toml`, so
        // without this line every daemon in this file inherits **the developer's own options** —
        // and the moment a test's subject IS an option (`run-driver-process`, register item 544)
        // the suite would be measuring whatever is set on the box it happens to run on. Pointed at
        // the test's own state root, which is empty unless a test writes one, so what is measured
        // is the DEFAULT.
        .env("XDG_CONFIG_HOME", state.join("config"))
        .stdin(Stdio::null())
        .status()
        .expect("spawn the sprag-term daemon");
    assert!(status.success(), "the daemon's parent forked cleanly");
}

/// Set one option for the daemon [`spawn_daemon`] will start under `state`, by writing the config
/// file that daemon reads.
///
/// # ⚠⚠⚠⚠⚠ A test that depends on a path rather than on a word must SAY the word
///
/// [`spawn_daemon`] points a daemon at the test's own config root precisely so that what it gets is
/// the shipped DEFAULT — which is the right answer for almost every gate in this file, and the
/// wrong one for a gate whose argument names a path. On 2026-08-25
/// [`sprag_host::options::RUN_DRIVER_PROCESS`]'s default moved from `off` to `on` (register item
/// 544) and two gates that had never mentioned the option **silently changed which path they
/// measure**: both are the in-process half of a pair, and both went on passing, so the sweep could
/// not say so. Their partners had pinned `on` from the first line; the halves that assumed a
/// default had nothing written down at all.
///
/// ⚠ So the rule this exists to make cheap: **if a gate's own doc says which side of a switch it is
/// on, it pins the switch.** A default is a fact about what ships, not a fixture.
fn daemon_told(state: &Path, options: &[(&str, &str)]) {
    if options.is_empty() {
        return;
    }
    let config = state.join("config").join("sprag");
    std::fs::create_dir_all(&config).expect("this test's own config directory");
    let mut written = String::from("[options]\n");
    for (name, value) in options {
        written.push_str(&format!("{name} = \"{value}\"\n"));
    }
    std::fs::write(config.join(sprag_host::CONFIG_FILE), written)
        .expect("a config a daemon will read");
}

/// Every `sprag-term` process holding `sock` in its environment — the daemon, plus one per run it
/// is driving in a process of its own ([`sprag_host::options::RUN_DRIVER_PROCESS`]).
///
/// # ⚠⚠⚠⚠ Why the process table is the only honest observer here
///
/// A row deliberately cannot say which kind of driver filled it in — `RUN_DRIVER_PROCESS`'s own doc
/// makes that a promise (*"the same request must mean the same thing either way, or the wire has
/// grown a second answer"*). So *the driver is a process of its own* has no wire answer by design,
/// and the only place it is true or false is the operating system.
///
/// ⚠ Matched on the ENVIRONMENT and not the name, [`daemon_pid`]'s rule and for its reason: a box
/// can be running the developer's own daemon, and a probe that counted every `sprag-term` would
/// count theirs.
fn sprag_term_processes(sock: &Path) -> usize {
    sprag_term_pids(sock).len()
}

/// The pids behind [`sprag_term_processes`], because a COUNT cannot say whether the two processes
/// you are looking at are the two you were looking at before.
///
/// Register item 526 needs the difference: a daemon replaced under a live loop must leave that
/// loop's driver ALIVE — the same process, not a fresh one with the same tally — and a boot that
/// put the run back on a second driver would keep the count right while driving one pane twice.
fn sprag_term_pids(sock: &Path) -> Vec<u32> {
    let want = format!("SPRAG_HOST_RPC_SOCK={}", sock.display());
    let me = std::process::id();
    sprag_terminal::procfs::pids_named("sprag-term")
        .into_iter()
        .filter(|&pid| pid != me)
        .filter(|&pid| {
            sprag_terminal::procfs::environ(pid).is_some_and(|environ| {
                environ
                    .split(|byte| *byte == 0)
                    .any(|value| value == want.as_bytes())
            })
        })
        .collect()
}

/// The pids of the DRIVER processes against `sock` — every `sprag-term` holding this endpoint
/// except the daemon itself.
///
/// ⚠ Only honest while the daemon is ALIVE: [`daemon_pid`] finds the daemon by being nobody's
/// child, and once it is killed its drivers are re-parented to init and answer that description
/// too. So a caller reads this BEFORE a kill and checks the pids it got are still there after.
fn driver_pids(sock: &Path) -> Vec<u32> {
    let daemon = daemon_pid(sock);
    sprag_term_pids(sock)
        .into_iter()
        .filter(|pid| Some(*pid) != daemon)
        .collect()
}

/// Whether `pid` is still a live `sprag-term`.
///
/// ⚠ Through `procfs::pids_named` rather than `/proc/<pid>`, which is what [`kill_daemon`] reads —
/// the question is portable and the answer should be too, and a pid that has been REUSED by some
/// other program is not this driver coming back from the dead.
fn still_running(pid: u32) -> bool {
    sprag_terminal::procfs::pids_named("sprag-term").contains(&pid)
}

/// Open a session on `sock` and submit a run over its pane that **cannot finish while anybody is
/// looking** — the fixture the [`RUN_DRIVER_PROCESS`](sprag_host::options::RUN_DRIVER_PROCESS)
/// gates count processes against.
///
/// # ⚠⚠⚠ Every part of it is load-bearing, so it is spelled once rather than per gate
///
/// The pane runs `cat` with its echo turned off and the sentinel is a line `cat` can never produce,
/// so the run is CERTAINLY still going while the process table is read — a run that converged would
/// take its driver process down with it and the count would be a race rather than a claim. The
/// guardrails are far past anything a gate waits out for the same reason.
///
/// ⚠ Its callers ask OPPOSITE questions of it (a daemon told `on`, one told nothing, and one that
/// kills a driver to see what the run becomes), which is exactly why one spelling: a fixture that
/// drifted between them would make those answers incomparable while all of them stayed green.
///
/// ⚠⚠ `session` is a PARAMETER because one caller needs TWO of these against one daemon — a subject
/// and a control — and a session name is unique per daemon. Every caller that wants only one passes
/// `work`, which is the name this fixture used to hard-code.
fn start_a_run_that_cannot_converge(sock: &Path, session: &str) {
    let mut conn = HostConn::connect(sock, Duration::from_secs(5)).expect("connect");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": session, "cmd": ["sh", "-c", "stty -echo; exec cat"] },
        }),
    )
    .expect("new_session answers");
    let pane = conn
        .call(
            "scene/query",
            json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the session's pane");
    conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "orchestrator",
                "pane": pane,
                "stimulus": "x",
                // ⚠ Never printed by a `cat`, so the run is certainly still going while the
                // process table is read — the same shape the restart gates in this file use.
                "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the run is submitted");
}

/// A session whose pane runs a stand-in agent that announces itself and echoes, so a loop over it
/// gets past readiness and takes real steps — and **a run that never stepped records no place**,
/// which is the difference between a run that can be put back and one that cannot.
///
/// ⚠ Hoisted out of the promotion gate when a second gate needed the same fixture (register item
/// 671): two spellings of *what a loop's pane is* would let the two gates measure different things
/// while both stayed green, which is [`start_a_run_that_cannot_converge`]'s own argument.
fn loop_session(conn: &mut HostConn, name: &str) -> u64 {
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": name,
                "cmd": ["sh", "-c",
                        "stty -echo; printf 'AGENT-READY\\n'; while read l; do printf '%s\\n' \"$l\"; done"],
            },
        }),
    )
    .expect("new_session answers");
    conn.call(
        "scene/query",
        json!({ "session": name, "path": mux_action_path(PANES_SLOT) }),
    )
    .expect("the pane list answers")
    .as_array()
    .and_then(|panes| panes.first().cloned())
    .and_then(|pane| pane["id"].as_u64())
    .expect("the session's pane")
}

/// Submit an `ai_loop` over `pane` in `session`, effectively unbounded so it is certainly still
/// going when whatever is driving it is taken away.
fn start_loop(conn: &mut HostConn, session: &str, pane: u64, star: &str) {
    conn.call(
        "scene/invoke",
        json!({
            "session": session,
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "ai_loop",
                "pane": pane,
                "agent": "claude",
                "north_star": star,
                "milestone": "still be running after the thing driving it was replaced",
                "reference": "register items 526 and 671",
                "ready_when": { "match": "shows", "marker": "AGENT-READY" },
                // ⚠ The stand-in paints only whole lines, so a delivery cannot be confirmed on
                // screen before the newline that submits it.
                "shows_prompt": false,
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the loop is submitted");
}

/// Whether the run log under `state` holds at least `want` unfinished runs that carry BOTH halves
/// of what a resume needs — the place a machine was at and the request that built it.
///
/// ⚠⚠ THE FILE IS FOUND BY SCANNING THE TEST'S OWN STATE DIR, because `runs_path` resolves
/// `XDG_STATE_HOME` in the CALLING process and this process's is the developer's.
fn resumable_runs(state: &Path, want: usize) -> bool {
    std::fs::read_dir(state.join("sprag"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".runs.json"))
        .any(|entry| {
            sprag_host::load_runs(&entry.path()).is_some_and(|log| {
                log.runs
                    .iter()
                    .filter(|run| {
                        !run.finished
                            && run.resumable_place().is_some()
                            && run.resumable_request().is_some()
                    })
                    .count()
                    >= want
            })
        })
}

/// Whether any DURABLE saved pane history under `state` contains `needle`.
///
/// "Durable" is the whole point, and it is why the file filter is
/// [`sprag_host::history_file_pane`] rather than a hand-rolled one: the atomic write leaves
/// `<id>.hist.tmp` in the same directory while it is in flight, and an unfiltered scan matches THAT.
/// A caller waiting on this to decide the history is safe would then kill the daemon between the
/// temp write and the rename — the pane comes back with no history at all, and the successor's own
/// save loop overwrites the file with its fresh prompt, erasing the evidence.
///
/// That was a real ~15% flake in
/// `a_killed_daemon_gives_its_panes_back_with_their_scrollback`, found by dumping the bytes: the
/// needle was sitting in `0.hist.tmp`. Waiting on the file a RESTORE would actually read is the
/// condition the assertion depends on — the rule this helper had stated in its caller's comment and
/// then not implemented.
fn saved_history_contains(state: &Path, needle: &str) -> bool {
    let Ok(dirs) = std::fs::read_dir(state.join("sprag")) else {
        return false;
    };
    dirs.flatten()
        .filter(|dir| dir.path().extension().is_some_and(|e| e == "history"))
        .flat_map(|dir| {
            std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
        })
        .filter(|file| sprag_host::history_file_pane(&file.path()).is_some())
        .any(|file| {
            std::fs::read(file.path())
                .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
        })
}

/// THE reboot payoff, end to end: a pane prints something, the daemon is KILLED outright, and the
/// next daemon on the same socket brings the pane back with its scrollback — provable because
/// `sprag find` still finds text the pane printed before the crash.
///
/// This is the one test that exercises the whole ring at once: the save loop's timer, the encoding,
/// the per-pane file, the restore's replay-before-the-reader seam, and the search that reads the
/// result. The pane comes back as a plain SHELL (a recorded `sh -c` is never re-run), so the text
/// it finds can only have come from the restored history, never from re-running the command.
///
/// Linux-gated: it finds the forked daemon through `/proc`.
#[test]
// Linux AND macOS: the restore-after-SIGKILL path stopped being Linux-shaped when `daemon_pid`
// went through the portable process table (R343). A gate left on a test after its subject
// became portable is a claim that the subject is not — and its HELPERS come with it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_killed_daemon_gives_its_panes_back_with_their_scrollback() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-durability-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    // Unique per run, so a needle can never be found in another test's leftovers.
    let needle = format!("PERSISTED-SCROLLBACK-{}", std::process::id());

    // Daemon A, and a session whose pane PRINTS the needle then blocks on its pty.
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "work",
                "cmd": ["sh", "-c", format!("printf '{needle}\\n'; exec cat")],
            },
        }),
    )
    .expect("new_session answers");
    drop(conn);

    // Wait on the CONDITION the assertion reads: the needle is actually ON DISK. The save loop is
    // on a timer, so polling anything else here would be a race dressed as a wait.
    assert!(
        wait_for(Duration::from_secs(30), || saved_history_contains(
            &state, &needle
        )),
        "the daemon never persisted the pane's scrollback under {}",
        state.display(),
    );

    // The reboot: kill the daemon outright, then start its successor on the same socket + state.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );

    // The payoff: the restored pane is searchable over output its predecessor produced.
    let mut run = sprag(&sock, &["find", "-t", "work", &needle]);
    let found = wait_for(Duration::from_secs(15), || {
        run = sprag(&sock, &["find", "-t", "work", &needle]);
        run.stdout.contains(&needle)
    });
    assert!(
        found,
        "the restored pane lost its scrollback (stdout {:?}, stderr {:?})",
        run.stdout, run.stderr,
    );
    drop(guard);
}

/// ⚠⚠⚠ **STOPPING THE SERVER LEAVES THE SAVED WORKSPACE STANDING** — `kill-server`'s own promise,
/// which nothing asked of it until it had already been broken.
///
/// # The defect, in the shape it was measured
///
/// `kill-server` used to be *"kill every session; the last kill drains the daemon"*. Sessions die
/// ONE AT A TIME while the durability saver runs on its five-second tick, so **the snapshot
/// converges toward empty as the kill proceeds** — the file the next launch reads is written DURING
/// the demolition. Measured on the owner's own daemon (2026-08-16): six sessions in, five out.
/// Session `1` — the first killed, holding the loop's outer and inner panes — was simply gone.
///
/// Its doc had said the opposite the whole time: *"By DEFAULT the durability snapshot is PRESERVED
/// … the next launch restores it."* **The promise and the implementation had never been asked to
/// agree**, because every existing gate asked only whether the daemon ENDED.
///
/// # ⚠⚠ Why the assertion is the RESTORE and not the file
///
/// Reading the snapshot would assert the mechanism this round happens to use. What the owner was
/// promised is that the workspace comes back, so the gate stops the server and starts a successor
/// on the same socket and state — the same reboot shape as
/// [`a_killed_daemon_gives_its_panes_back_with_their_scrollback`], with the ONE difference under
/// test: a graceful `kill-server` where that one uses SIGKILL.
///
/// ⚠ TWO sessions, not one, and that is the defect's own shape: the loss was in the sessions killed
/// FIRST, and a fixture with a single session cannot express *"first"*.
///
/// # ⚠⚠⚠ WHAT THIS GATE DOES NOT CATCH, MEASURED RATHER THAN ASSUMED
///
/// Reverting `kill_server` to the old kill-every-session shape leaves this **GREEN**, and that was
/// checked rather than hoped. The loss needed a saver TICK to land between the first kill and the
/// daemon's exit; two sessions die in milliseconds and the five-second timer never fires, so the
/// snapshot survives a demolition it only happens to outrun. **The defect is timing-dependent, and
/// this is a forward ratchet on the promise rather than a reproducer of it.**
///
/// What DOES catch the mechanism, deterministically, is
/// [`every_slot_reader_explains_a_daemon_that_does_not_serve_it`]: the old shape reads
/// `/sprag_mux/external/sessions` first, and that sweep now asserts `kill-server` fails for no such
/// address. Under the revert it goes red, printing the absurdity whole — a failing `sprag
/// kill-server` whose own message advises *"Restart it: `sprag kill-server`"*.
///
/// **The two are kept apart on purpose**: one holds the mechanism (cheap, deterministic, and about
/// the wire), the other holds the PROMISE end to end (a real daemon, a real restart, a real
/// restore), which is the thing an operator was actually told and the thing no gate asked about.
#[test]
fn kill_server_leaves_every_session_in_the_saved_workspace() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-killserver-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );

    // Two named sessions, each holding a pane that stays put.
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    for name in ["first", "second"] {
        conn.call(
            "scene/invoke",
            json!({
                "path": mux_action_path(NEW_SESSION_ACTION),
                "args": { "name": name, "cmd": ["cat"] },
            }),
        )
        .expect("new_session answers");
    }
    drop(conn);

    // ⚠ Wait on the CONDITION the assertion depends on — that the saver has actually written a file
    // naming both. The loop is on a timer, so anything else here is a race dressed as a wait.
    let saved = |state: &Path| -> String {
        std::fs::read_dir(state.join("sprag"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|file| file.path().extension().is_some_and(|kind| kind == "json"))
            .filter_map(|file| std::fs::read_to_string(file.path()).ok())
            .collect()
    };
    assert!(
        wait_for(Duration::from_secs(30), || {
            let text = saved(&state);
            text.contains("\"first\"") && text.contains("\"second\"")
        }),
        "the control: the saver must have written BOTH sessions before the server is stopped, or \
         this gate would pass on a daemon that never saved anything",
    );

    // The graceful stop — the verb whose promise is under test.
    let stopped = sprag(&sock, &["kill-server"]);
    assert!(stopped.ok, "kill-server succeeded: {}", stopped.stderr);
    assert!(
        !sprag(&sock, &["ls"]).ok,
        "and the server really is gone, or nothing below is about a restart",
    );

    // The successor, on the same socket and the same state.
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );
    let listed = sprag(&sock, &["ls"]);
    for name in ["first", "second"] {
        assert!(
            listed.stdout.contains(name),
            "⚠⚠⚠ THE SAVED WORKSPACE LOST {name:?} WHEN THE SERVER WAS STOPPED. `kill-server` \
             promises the opposite in its own doc, and the way it broke that was killing sessions \
             one at a time while the durability saver kept writing the shrinking shape. Got: {:?}",
            listed.stdout,
        );
    }
    drop(guard);
}

/// **A pane that came back from a REBOOT can still ask for a person** (R319, closing R318's item
/// 44) — the `on_attention` hook the RESTORE path wires, driven for the first time.
///
/// R318 wired it (`host.restore(.., || Some(pane_attention_hook(&attention)), ..)`) and drove the
/// PanePty half only: a restored pane's replayed `OSC 9` does not re-fire, which is a claim about the
/// emulator's latch and not about the hook. Nothing anywhere asked whether a pane that survived a
/// crash can still reach a person — and the wiring is one closure argument, in a call whose other
/// five arguments a passing test would not distinguish it from.
///
/// The message is read at the DAEMON's mailbox (`client/messages`) rather than off a screen, because
/// this file has no display: what is under test is the routing, and R318's own gate already proves a
/// routed message reaches a terminal front's row.
///
/// **THE CONTROL GOES FIRST and is watched all the way through** — R318's lesson, where a control
/// sent second had already replaced the row the claim was then read from. `sprag display-message`
/// takes the same mailbox to the same client on the same daemon, so it establishes that the
/// attachment, the delivery and the collect all work here; it is then COLLECTED, leaving the mailbox
/// provably empty, so the sentence the claim reads cannot be the control's.
///
/// Linux-gated: it finds the forked daemon through `/proc`, like its two siblings above.
#[test]
// Linux AND macOS: the restore-after-SIGKILL path stopped being Linux-shaped when `daemon_pid`
// went through the portable process table (R343). A gate left on a test after its subject
// became portable is a claim that the subject is not — and its HELPERS come with it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_run_whose_daemon_died_is_reported_as_interrupted_and_belongs_to_nobody() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-runlog-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    // Daemon A, a pane, and a LONG run against it — long enough that it is certainly still going
    // when the daemon is killed under it. Its provenance is stamped, so the drop below is a
    // measurable act and not a property of a run nobody claimed.
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": "work", "cmd": ["sh", "-c", "stty -echo; exec cat"] },
        }),
    )
    .expect("new_session answers");
    // `new_session` does not answer a pane id, so the pane is read off the slot that lists them.
    let pane = conn
        .call(
            "scene/query",
            json!({ "session": "work", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the session's pane");
    conn.call(
        "scene/invoke",
        json!({
            "session": "work",
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "orchestrator",
                "pane": pane,
                "stimulus": "x",
                "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                "opened_by": pane,
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the run is submitted");
    drop(conn);

    // Wait on the CONDITION the assertion reads: the run is ON DISK and still running. The save
    // loop is on a timer, so anything else here would be a race dressed as a wait.
    //
    // ⚠⚠ THE FILE IS FOUND UNDER THIS TEST'S OWN STATE DIR, by scanning it. `runs_path` resolves
    // `XDG_STATE_HOME` in the CALLING process, and this process's is the developer's — so asking it
    // for the path pointed the wait at a file some other daemon on this machine had written. The
    // gate passed with the daemon's own write DELETED, which is what a fixture reading somebody
    // else's file looks like from the inside (R318/R319/R331's rule, one directory along).
    let runs_dir = state.join("sprag");
    let live_run_on_disk = || {
        std::fs::read_dir(&runs_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".runs.json"))
            .any(|entry| {
                sprag_host::load_runs(&entry.path()).is_some_and(|log| {
                    log.runs
                        .iter()
                        .any(|run| !run.finished && run.iterations > 0)
                })
            })
    };
    assert!(
        wait_for(Duration::from_secs(30), live_run_on_disk),
        "the daemon never persisted a live run under {}",
        runs_dir.display(),
    );

    // THE KILL: outright, so nothing gets to write a tidy terminal state on the way out.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );

    // ⚠⚠ THE PAYOFF: the successor says the run was INTERRUPTED, where before it said nothing at
    // all — and "no runs" is the same answer a daemon nobody ever asked for a loop gives.
    let listed = sprag(&sock, &["runs", "-t", "work"]);
    assert!(listed.ok, "{}", listed.stderr);
    assert!(
        listed.stdout.contains("interrupted"),
        "a run whose daemon died must be reported as interrupted, not forgotten: {:?}",
        listed.stdout,
    );
    assert!(
        listed.stdout.contains("run 0"),
        "and it keeps the id it had, so a person can match it against what they started: {:?}",
        listed.stdout,
    );

    // ⚠⚠⚠⚠ AND IT BELONGS TO NOBODY — BECAUSE THIS RUN WAS ASKED FOR BY A SHELL, which is the
    // reason after 2026-08-18 and not the one this comment used to give. It said the restored pane's
    // occupant is "a plain shell — never the agent that asked", as though that were true of every
    // restore; measured false, an allowlisted agent comes back `--resume`d in the same conversation.
    // What is true is narrower and is what this fixture actually stages: the pane here runs
    // `sh -c "stty -echo; exec cat"`, so `Pane::agent_session` is `None`, so the run recorded NO
    // conversation, so a successor has nothing to re-derive a seat from. The conservative answer is
    // kept for a real reason instead of a false one — see `RunRegistry::restore`'s rule 1.
    assert!(
        !listed.stdout.contains("asked for by pane"),
        "a run a SHELL asked for must claim no opener after a restart — there is no conversation to \
         match it to anybody, and inventing a seat would hand it to whoever boots in next: {:?}",
        listed.stdout,
    );
    drop(guard);
}

/// ⛔⛔⛔⛔⛔ **A DAEMON TOLD TO DRIVE ITS RUNS IN PROCESSES OF THEIR OWN DOES, AND ONE TOLD NOT TO
/// DOES NOT** — [`sprag_host::options::RUN_DRIVER_PROCESS`]'s contract, in both directions.
///
/// # ⚠⚠⚠⚠⚠ Why this exists, and why it is NOT the gate on the default
///
/// It was written as one. Item 544's end state is the opposite default, and the measurements its
/// own doc said that decision was waiting for had arrived (items 543, 662, 663, and this item's
/// stage 1). So the switch was flipped — and **the workspace sweep answered with eighteen
/// failures**, then nine, then two, which became register items 664 and 665 and sent the word back
/// to `off` for a day. Both are paid; the word is `on` since 2026-08-25.
///
/// What survives all of that is this: **the option must work in both directions, whichever way the
/// default points.** A switch nobody measures is a promise nobody is keeping — and it is the WAY
/// BACK, which is the half that matters most on the day somebody reaches for it.
///
/// ⚠⚠⚠ **AND BOTH ARMS HERE WRITE A CONFIG FILE, SO THIS GATE IS BLIND TO THE DEFAULT BY
/// CONSTRUCTION** — neither of its daemons ever reads one. That is deliberate and it is also why it
/// cannot be the only gate: the day the shipped word moves, the thing that moved would have nothing
/// watching it. [`a_daemon_nobody_configured_drives_its_runs_in_processes_of_their_own`] is that
/// one, and the mutation says the two are different sentences — put the default back to `off` and
/// it goes red while this stays green.
///
/// ⚠⚠ **THE PROCESS TABLE IS THE ONLY HONEST OBSERVER.** A row deliberately cannot say which kind
/// of driver filled it in — that is the option's own promise (*"the same request must mean the same
/// thing either way, or the wire has grown a second answer"*) — so *the driver is a process of its
/// own* has no wire answer by design, and is true or false only in the operating system.
///
/// ⚠⚠ **THE CONFIG DIRECTORY IS THE TEST'S OWN** ([`spawn_daemon`]): the subject here IS an option,
/// so a daemon reading the developer's `config.toml` would make this suite answer differently on
/// different boxes — and answer WRONGLY on the one box where somebody had set it.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_daemon_told_to_drive_runs_in_processes_of_their_own_does_and_one_told_not_to_does_not() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-driver-on-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    // ── A DAEMON TOLD `on` ───────────────────────────────────────────────────────────────────
    daemon_told(&state, &[(sprag_host::options::RUN_DRIVER_PROCESS, "on")]);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    assert_eq!(
        sprag_term_processes(&sock),
        1,
        "⚠⚠ THE PREMISE: before any run there is exactly one `sprag-term` against this socket — \
         the daemon. If this is not 1 the count below says nothing about drivers.",
    );

    start_a_run_that_cannot_converge(&sock, "work");

    // ── THE CLAIM: the run got a process of its own ──────────────────────────────────────────
    assert!(
        wait_for(Duration::from_secs(20), || sprag_term_processes(&sock) == 2),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 544: a daemon told `run-driver-process = on` drove this run on a \
         THREAD of its own. Everything items 543, 662 and 663 built — a run's place and datamodel \
         crossing the log, its counters, position and walk reaching the daemon, a boot putting it \
         back on a driver — serves the out-of-process path, and a switch that does not reach it is \
         a path nothing can be pointed at. Found {} `sprag-term` process(es) against this socket, \
         and a driven run makes two.",
        sprag_term_processes(&sock),
    );

    // ── THE CONTROL: a daemon told `off` still drives on a thread ────────────────────────────
    //
    // ⚠⚠⚠⚠ WITHOUT THIS, *the switch was honoured* and *this test cannot count* are the same
    // green. And it is the arm that matters most on the day somebody reaches for it: `off` is the
    // WAY BACK, so a switch that only ever worked in one direction is half a switch.
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-driver-off-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let off = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    daemon_told(&state, &[(sprag_host::options::RUN_DRIVER_PROCESS, "off")]);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the configured daemon never started serving",
    );
    start_a_run_that_cannot_converge(&sock, "work");
    // ⚠ Given TIME to be wrong: the claim above waits up to twenty seconds for a second process, so
    // a control that looked once could pass simply by reading before a child had been spawned.
    assert!(
        !wait_for(Duration::from_secs(5), || sprag_term_processes(&sock) > 1),
        "⚠⚠⚠ THE CONTROL FAILED: a daemon told `run-driver-process = off` started a driver \
         PROCESS anyway. That switch is the way back from the new default, and a switch that does \
         nothing is worse than no switch — it is a promise nobody is keeping.",
    );
    drop(off);
    drop(guard);
}

/// ⛔⛔⛔⛔⛔ **A DAEMON NOBODY CONFIGURED DRIVES ITS RUNS IN PROCESSES OF THEIR OWN** — register
/// item 544's destination, held as the DEFAULT rather than as a switch somebody threw.
///
/// # ⚠⚠⚠⚠⚠ Why the switch's own gate cannot cover this, and it is not a matter of taste
///
/// [`a_daemon_told_to_drive_runs_in_processes_of_their_own_does_and_one_told_not_to_does_not`]
/// writes a config file in BOTH of its arms, deliberately: it is the OPTION's contract, and it must
/// hold whichever way the default points. That makes it blind to this question **by construction** —
/// change [`sprag_host::options::RUN_DRIVER_PROCESS`]'s default word and neither of its daemons
/// notices, because neither of them ever reads a default. So the day the word moved, the thing that
/// moved had no gate at all. **Two gates, two sentences**, and the mutation says so: with the
/// default back at `off` that gate passes and this one fails.
///
/// ⚠⚠ **THE PREMISE IS THAT THERE WAS NOTHING TO READ, AND IT IS ASSERTED RATHER THAN ARRANGED.**
/// [`spawn_daemon`] points the daemon's `XDG_CONFIG_HOME` at this test's own state root for exactly
/// this reason; this gate then checks the file is absent before the daemon starts. Without that
/// check a developer with `run-driver-process` set in their own `config.toml` would have this pass
/// on their box and answer about their setting, not about the shipped word.
///
/// ⚠⚠ **THE COUNT IS A BEFORE AND AN AFTER ON ONE DAEMON**, which is what makes it about drivers:
/// one process while nothing is running, two once a run is going. A single reading could be a box
/// with somebody else's daemon on it — [`sprag_term_processes`] filters by this socket for that
/// reason, and the pair closes what is left.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_daemon_nobody_configured_drives_its_runs_in_processes_of_their_own() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-driver-default-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    let configured = state
        .join("config")
        .join("sprag")
        .join(sprag_host::CONFIG_FILE);
    assert!(
        !configured.exists(),
        "⚠⚠ THE PREMISE: this daemon must have NOTHING to read, or the count below is about \
         somebody's setting rather than about the word this product ships. Found a config at \
         {configured:?}",
    );

    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    assert_eq!(
        sprag_term_processes(&sock),
        1,
        "⚠⚠ THE PREMISE: before any run there is exactly one `sprag-term` against this socket — \
         the daemon. If this is not 1 the count below says nothing about drivers.",
    );

    start_a_run_that_cannot_converge(&sock, "work");

    assert!(
        wait_for(Duration::from_secs(20), || sprag_term_processes(&sock) == 2),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 544: A DAEMON NOBODY CONFIGURED DROVE THIS RUN ON A THREAD INSIDE \
         ITSELF. That is the fusion this item exists to end — a run's supervisor sharing a process \
         with the thing that holds the PTYs, so changing how a loop reflects means restarting \
         somebody's terminals, and a panic in a driver takes the panes down with it. The switch \
         has worked in both directions since 2026-08-24; what this gate holds is that a person who \
         sets nothing gets the unfused one. Found {} `sprag-term` process(es) against this socket, \
         and a driven run makes two.",
        sprag_term_processes(&sock),
    );
    drop(guard);
}

/// ⛔⛔⛔⛔⛔ **A DAEMON RESTARTED UNDER A LIVE LOOP BRINGS THAT LOOP BACK RUNNING** — register item
/// 543's own «done when», end to end through two real daemons.
///
/// # ⚠⚠⚠⚠⚠ What it ends, and why the bill was never only about runs
///
/// *"A run's machine is never persisted, so every restart kills every run."* Because a restart kills
/// runs, promoting this daemon's own build is a DESTRUCTIVE act; four supervisors share one daemon
/// (register item 196), so promoting sprag's build killed three other repositories' loops; and
/// because of that, item 526's cheap route was to split daemons — buying the split at the price of a
/// second GUI (item 285), which the owner has refused once. A restart that resumes runs is a
/// promotion nobody has to schedule around, which is why this gate is the item's payoff rather than
/// one more assertion about serialization.
///
/// # ⚠⚠⚠⚠ The control is the OTHER run, under the same kill, and it is what makes this a claim
///
/// A gate that only watched the loop could be passed by a daemon that brought EVERYTHING back
/// running — which would be a lie about every plugin that walks no statechart and has no place to
/// be put back at. So a second run is started beside it, on its own pane, with a plugin that has no
/// machine, and it must still come back **interrupted**. One restart, two runs, opposite answers:
/// that is the difference between *resuming* and *forgetting to mark things dead*.
///
/// ⚠⚠ **THE WAIT BEFORE THE KILL IS ON WHAT THE ASSERTIONS READ**, both halves of it: the loop is on
/// disk with a place AND a request, and the control is on disk with neither. The save loop is on a
/// timer, so anything else here would be a race dressed as a wait. The kill is outright, so nothing
/// gets to write a tidy terminal state on the way out.
///
/// ⚠ The loop's pane comes back as a plain SHELL — `sh` is not on the restore allowlist — so the
/// resumed run drives a peer that will never answer it. That is fine and is deliberate: what is
/// under test is whether the run is DRIVEN again, not whether it converges. Its id and its seat are
/// the same; only the occupant is not.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_daemon_restarted_under_a_live_loop_brings_that_loop_back_running() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-resumed-run-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
    let pane_of = |conn: &mut HostConn, session: &str| {
        conn.call(
            "scene/query",
            json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the session's pane")
    };

    // The loop's peer: a stand-in agent that announces itself and echoes, so the loop gets past
    // readiness and takes real steps — a run that never stepped records no place.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "work",
                "cmd": ["sh", "-c",
                        "stty -echo; printf 'AGENT-READY\\n'; while read l; do printf '%s\\n' \"$l\"; done"],
            },
        }),
    )
    .expect("new_session answers");
    let peer = pane_of(&mut conn, "work");
    // And the CONTROL's own pane, so the two runs never type into each other's peer.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": "control", "cmd": ["sh", "-c", "stty -echo; exec cat"] },
        }),
    )
    .expect("new_session answers");
    let quiet = pane_of(&mut conn, "control");

    conn.call(
        "scene/invoke",
        json!({
            "session": "work",
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "ai_loop",
                "pane": peer,
                "agent": "claude",
                "north_star": "a run outlives the daemon that started it",
                "milestone": "come back running where the log said",
                "reference": "register item 543",
                "ready_when": { "match": "shows", "marker": "AGENT-READY" },
                // ⚠ The stand-in paints only whole lines, so a delivery cannot be confirmed on
                // screen before the newline that submits it.
                "shows_prompt": false,
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the loop is submitted");
    conn.call(
        "scene/invoke",
        json!({
            "session": "control",
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "orchestrator",
                "pane": quiet,
                "stimulus": "x",
                "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the control run is submitted");
    drop(conn);

    // ⚠⚠ THE FILE IS FOUND BY SCANNING THIS TEST'S OWN STATE DIR — `runs_path` resolves
    // `XDG_STATE_HOME` in the CALLING process, and this process's is the developer's, so asking it
    // for the path would point the wait at some other daemon's file (the sibling gate above paid
    // for that once).
    let runs_dir = state.join("sprag");
    let both_on_disk = || {
        std::fs::read_dir(&runs_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".runs.json"))
            .any(|entry| {
                sprag_host::load_runs(&entry.path()).is_some_and(|log| {
                    log.runs.iter().any(|run| {
                        !run.finished
                            && run.resumable_place().is_some()
                            && run.resumable_request().is_some()
                    }) && log
                        .runs
                        .iter()
                        .any(|run| !run.finished && run.place.is_none())
                })
            })
    };
    assert!(
        wait_for(Duration::from_secs(60), both_on_disk),
        "⚠⚠ THE PREMISE FAILED: the daemon never persisted BOTH a live loop carrying a place and a \
         request AND a live run carrying neither, under {}. Without both, the pair of answers below \
         would be a pair of accidents.",
        runs_dir.display(),
    );
    // And the panes have to survive too, or the loop comes back to no pool and nothing could put it
    // anywhere. The shape is saved by the same loop on the same timer, one file along.
    assert!(
        wait_for(Duration::from_secs(60), || std::fs::read_dir(&runs_dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .ends_with(".snapshot.json"))),
        "⚠⚠ THE PREMISE FAILED: the daemon never wrote a workspace snapshot, so its successor \
         would boot with no panes and no run could be put back over one",
    );

    // THE KILL: outright, so nothing writes a tidy terminal state on the way out.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
    let listed = conn
        .call(
            "scene/query",
            json!({
                "session": "work",
                "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
            }),
        )
        .expect("the runs slot answers");
    let rows = listed.as_array().expect("a list of runs").clone();
    let row_of = |id: u64| {
        rows.iter()
            .find(|run| run["id"] == json!(id))
            .unwrap_or_else(|| panic!("run {id} survived the restart as a row: {rows:?}"))
            .clone()
    };

    // ── THE CONTROL: a run with no machine still comes back dead ─────────────────────────────
    assert_eq!(
        row_of(1)["state"]["status"],
        json!("interrupted"),
        "⚠⚠⚠ THE CONTROL FAILED: a run whose plugin walks no statechart came back alive. Nothing \
         recorded where it was, so there was nowhere to put it — a daemon that started it again \
         would be starting a SECOND run under the first one's id. Rows: {rows:?}",
    );

    // ── THE CLAIM: the loop is driven again ──────────────────────────────────────────────────
    assert_ne!(
        row_of(0)["state"]["status"],
        json!("interrupted"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a daemon was restarted under a live loop whose place AND \
         request were both on disk, and the loop came back dead. Every brick before this one is a \
         capability nothing calls: the place crossed the log, the words its entry actions wrote \
         crossed beside them, and the boot did not pick them up. Rows: {rows:?}",
    );
    drop(conn);
    drop(guard);
}

/// ⛔⛔⛔⛔⛔ **A DAEMON REPLACED UNDER SEVERAL INDEPENDENT LOOPS BRINGS THEM ALL BACK, AND NONE OF
/// THEM TWICE** — register item 526, which is the *other people's work* half of item 543.
///
/// # ⚠⚠⚠⚠⚠ What this is about, in the owner's own words
///
/// *"넷이 돌면 데몬·세션도 넷이어야 승격이 되는 것 아닌가"*. Four repositories' loops share one
/// daemon on this machine, and only ONE of them (sprag's) ever needs the binary swapped. Because a
/// promotion is a DAEMON act, promoting sprag's build used to kill three other repositories' work —
/// so item 526's cheap route was a second daemon, paid for with a second GUI (item 285) and a second
/// binary copy (item 412), and the owner refused that once.
///
/// Item 526's own «done when» named the larger way out: *"408's residue is paid (persist the
/// machine, not the summary) so a restart stops being a run's death and the whole reason to split
/// disappears."* It was paid — item 543 — and item 544 then took the driver out of the daemon
/// altogether. **This gate is what says the reason to split is actually gone**, and it is a
/// different sentence from 543's: that one asks whether A loop comes back, this one asks whether
/// SOMEBODY ELSE'S does when the promotion was not theirs.
///
/// # ⚠⚠⚠⚠ Two mechanisms could each carry it, and the gate holds BOTH ends on purpose
///
/// Since item 544's default moved, a loop's driver is a process of its own, so a daemon that dies
/// does not take it with it — the driver latches and re-adopts. And independently, the boot reads
/// the runs log and puts an unfinished run back on a driver. **Either alone would make the rows
/// below say `running`.** Run together carelessly they are worse than either: a survivor plus a
/// freshly spawned one is ONE PANE WITH TWO DRIVERS, which no row can show because a row
/// deliberately cannot say which kind of driver filled it in.
///
/// **Measured before any of this was decided: five `sprag-term` against one socket for two loops,
/// where three is right.** The row said `running` throughout, both times.
///
/// # ⚠⚠⚠⚠⚠ Which of the two the product keeps, and the fact that settles it
///
/// The leftover is the one that goes, and the argument is not tidiness — it is the ANSWER CHANNEL.
/// A driver reports its outcome on the stdout pipe of the process that spawned it (`crate::drive`'s
/// module doc: *"the parent spawned this process, so the pipe is already theirs"*), so a driver
/// whose daemon is gone can finish hours of work that no successor will ever be able to read, while
/// the run log goes on saying it is running. The LOG is what survives a restart, so the run comes
/// back through the log and `put_back_inherited_runs` ends the leftover first.
///
/// ⚠ The residue, stated rather than hidden: whatever the loop did since its last persist is lost.
/// That is the cost item 543 already accepted for a restart, paid here for the same reason.
///
/// So the process table is read on both sides of the kill:
/// * afterwards there is exactly ONE driver per loop plus the replacement daemon — fewer means
///   somebody's work stopped, which is the whole of item 526, and more means a pane with two;
/// * and no pid from before is still alive — the leftovers were ENDED, not left typing.
///
/// ⚠ `driver_pids` is read while the first daemon is alive for the reason its own doc gives.
///
/// # ⚠⚠ The control is a run with no machine, and it must still come back dead
///
/// Without it this gate would pass against a daemon that marked everything `running` on boot —
/// which is the failure mode 543's own control exists for, and it is not less likely here.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_promotion_brings_every_loop_back_on_exactly_one_driver() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-promoted-under-guests-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");

    // ── OURS: the loop whose repository is the one being promoted ────────────────────────────
    let ours = loop_session(&mut conn, "ours");
    start_loop(
        &mut conn,
        "ours",
        ours,
        "the repository whose build is being swapped",
    );
    // ── THE GUEST: somebody else's work, which did not ask for any of this ───────────────────
    let guest = loop_session(&mut conn, "guest");
    start_loop(&mut conn, "guest", guest, "another repository entirely");

    // ⚠⚠⚠⚠⚠ **THE LOOPS' OWN DRIVERS ARE IDENTIFIED HERE, BEFORE ANYTHING ELSE IS STARTED, AND
    // THE FIRST FORM OF THIS GATE GOT IT WRONG.** Every run gets a driver process now, the control
    // below included — and the control's driver is SUPPOSED to die with its daemon, because a
    // plugin that walks no statechart has nowhere to be put back. Reading the pids after the
    // control was submitted made this gate report that death as *a promotion killed somebody
    // else's loop*, which is a true sentence about the wrong process. The process table does not
    // say which run a driver belongs to (`procfs` publishes parentage, not a command line), so
    // WHEN they are read is what names them.
    assert!(
        wait_for(Duration::from_secs(30), || driver_pids(&sock).len() == 2),
        "⚠⚠ THE PREMISE FAILED: this daemon ships `run-driver-process` on (register item 544), so \
         the two loops above should be two processes of their own. Found {:?}.",
        driver_pids(&sock),
    );
    let loops = driver_pids(&sock);

    // ── THE CONTROL: a run whose plugin walks no statechart ──────────────────────────────────
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": "control", "cmd": ["sh", "-c", "stty -echo; exec cat"] },
        }),
    )
    .expect("new_session answers");
    let quiet = conn
        .call(
            "scene/query",
            json!({ "session": "control", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the control session's pane");
    conn.call(
        "scene/invoke",
        json!({
            "session": "control",
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "orchestrator",
                "pane": quiet,
                "stimulus": "x",
                "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the control run is submitted");
    drop(conn);

    // ⚠⚠ THE FILE IS FOUND BY SCANNING THIS TEST'S OWN STATE DIR — `runs_path` resolves
    // `XDG_STATE_HOME` in the CALLING process, and this process's is the developer's.
    let runs_dir = state.join("sprag");
    assert!(
        wait_for(Duration::from_secs(90), || resumable_runs(&state, 2)),
        "⚠⚠ THE PREMISE FAILED: the daemon never persisted TWO live loops each carrying a place \
         and a request under {}. With fewer, the answers below are about one loop and this gate is \
         item 543 wearing a second name.",
        runs_dir.display(),
    );
    assert!(
        wait_for(Duration::from_secs(60), || std::fs::read_dir(&runs_dir)
            .into_iter()
            .flatten()
            .flatten()
            .any(|entry| entry
                .file_name()
                .to_string_lossy()
                .ends_with(".snapshot.json"))),
        "⚠⚠ THE PREMISE FAILED: the daemon never wrote a workspace snapshot, so its successor \
         would boot with no panes and no run could be put back over one",
    );

    // ⚠⚠⚠ READ WHILE THE DAEMON IS ALIVE, which is the only time `driver_pids` can tell a driver
    // from the daemon. This is every driver, the control's included — it is the CEILING the count
    // below is compared against, while `loops` above is the set that has to survive.
    let before = driver_pids(&sock);
    assert!(
        before.len() > loops.len(),
        "⚠⚠ THE PREMISE FAILED: the control run got no driver of its own, so the ceiling below is \
         not the one this gate reasoned about. Loops {loops:?}, all {before:?}.",
    );

    // THE PROMOTION: outright, so nothing writes a tidy terminal state on the way out — a build
    // swap is not a courtesy shutdown.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the replacement daemon never started serving",
    );

    // ── THE CLAIM, PART ONE: every loop is driven again, and by EXACTLY ONE process each ─────
    //
    // ⚠⚠⚠⚠⚠ COUNTED AS A TOTAL, NOT THROUGH `driver_pids` — the first form of this assertion used
    // that helper and was WRONG about which process was missing. Once the first daemon is killed
    // its leftover drivers are re-parented to init, so `daemon_pid`'s *nobody's child* rule can
    // pick a DRIVER as the daemon; that helper's own doc says it is honest only while the daemon
    // is alive. A total needs no such judgement: one replacement daemon, one driver per loop, and
    // no driver for the control because a run with no machine has nowhere to be put back.
    let after = sprag_term_pids(&sock);
    let want = 1 + loops.len();
    assert_eq!(
        after.len(),
        want,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 526: A PROMOTION LEFT THE WRONG NUMBER OF PROCESSES DRIVING. \
         Wanted the replacement daemon and one driver for each of the {} loops, which is {want}; \
         found {:?}. MORE means a pane with two drivers — the leftover kept typing and the boot \
         started another beside it, which no ROW can show because a row deliberately cannot say \
         which kind of driver filled it in. FEWER means somebody's loop is not being driven at \
         all, which is the promotion killing other people's work that this item was filed for.",
        loops.len(),
        after,
    );

    // ── THE CLAIM, PART TWO: and the leftovers are GONE rather than still typing ─────────────
    //
    // ⚠⚠⚠⚠⚠ **THIS IS A DECISION, NOT AN ACCIDENT, AND IT COST THIS ROUND A RED TO TAKE.** The
    // first form of this gate asserted the opposite — that the pids seen before the kill were all
    // still alive after — because item 544's stage 1 made a driver outlive its daemon on purpose.
    // What settles it is the ANSWER CHANNEL: a driver reports its outcome on the stdout pipe of
    // the process that spawned it, so a leftover can finish hours of work that no successor can
    // ever read, while the run log — which does survive — says the run is still going. So the run
    // comes back through the log and the leftover process is ended by the boot.
    //
    // ⚠⚠ The residue, stated rather than hidden: whatever the loop did since its last persist is
    // lost. That is the same cost register item 543 already accepted for a restart, and it is
    // paid here for the same reason.
    let alive: Vec<u32> = loops
        .iter()
        .copied()
        .filter(|pid| still_running(*pid))
        .collect();
    assert!(
        alive.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 526: A DRIVER THE PREDECESSOR LEFT IS STILL TYPING. Driver(s) \
         {alive:?} of {loops:?} outlived the daemon that spawned them and were not ended by the \
         boot, so the agent they are driving now has two processes talking to it and the older \
         one's outcome goes to a pipe nobody holds. `put_back_inherited_runs` is where this is \
         supposed to be decided.",
    );

    // ── AND THE ROWS AGREE, in both directions ───────────────────────────────────────────────
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
    let rows_of = |conn: &mut HostConn, session: &str| {
        conn.call(
            "scene/query",
            json!({
                "session": session,
                "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
            }),
        )
        .expect("the runs slot answers")
        .as_array()
        .expect("a list of runs")
        .clone()
    };
    let rows = rows_of(&mut conn, "guest");
    let interrupted = |rows: &[Value], id: u64| {
        rows.iter()
            .find(|run| run["id"] == json!(id))
            .unwrap_or_else(|| panic!("run {id} survived the promotion as a row: {rows:?}"))["state"]
            ["status"]
            == json!("interrupted")
    };
    assert!(
        !interrupted(&rows, 1),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 526: the GUEST's loop came back dead from a promotion it had no \
         part in. Rows: {rows:?}",
    );
    assert!(
        !interrupted(&rows, 0),
        "and so did ours, which means this is item 543 regressing rather than 526: {rows:?}",
    );
    assert!(
        interrupted(&rows, 2),
        "⚠⚠⚠ THE CONTROL FAILED: a run whose plugin walks no statechart came back alive, so these \
         rows would say `running` whatever had happened to the work. Rows: {rows:?}",
    );
    drop(conn);
    drop(guard);
}

/// ⚠⚠⚠⚠⚠ **A RUN WHOSE DRIVER PROCESS DIES UNDER A LIVING DAEMON IS PUT BACK ON A NEW ONE, AND A
/// RUN THAT CANNOT BE IS TOLD SO** — register item 671, the first of the two residues item 544 left
/// behind when it moved a driver out of the daemon.
///
/// # ⚠⚠⚠ What the fused design got for free, and what a process boundary took away
///
/// While a run was driven on a THREAD of this daemon's own, a driver that died was a thread that
/// panicked, and `RunRegistry::sweep` turned that into the run's outcome without anybody having to
/// decide it should. A driver that is a PROCESS can be killed by an OOM killer, by a person reading
/// `ps`, or by a bug in the image it runs, and item 544's own text filed *who notices* as a residue.
/// Since the default moved on 2026-08-25 (register item 544) that is where every daemon nobody has
/// configured now stands.
///
/// # ⚠⚠⚠⚠⚠ The decision this gate holds, and the fact that took it
///
/// A BOOT already puts back every run a dead daemon left (register item 543,
/// `put_back_inherited_runs`). Leaving a live daemon to do nothing gives one run two different
/// fates for the same fact — *nothing is driving it* — decided by whether the daemon happened to
/// restart, which is an accident and not an answer. So the live daemon answers it the same way,
/// through the same door, at the same price: what the run did since its last report is lost, which
/// is the cost item 543 already accepted.
///
/// ⚠ The gate's first form measured the OLD answer — the row reaching a reader as `panicked` — and
/// it was green. That is not what makes this one right; what makes it right is that the two
/// answers to one fact were being decided by an accident. The word in the row moved because the
/// product changed its mind on purpose.
///
/// # ⚠⚠⚠⚠ The second arm is not a control, it is the BOUNDARY, and it is race-free
///
/// A daemon that put every dead driver's run back would spin on a run that cannot come back at all,
/// so the refusal is half the decision and is measured beside it: a plugin that walks no statechart
/// records no place, so there is nowhere to put it back, and the row has to SAY that rather than
/// leaving a person watching a stopped run for a rescue that was already declined.
///
/// ⚠⚠ And the refusal is what makes the *no new driver was started* assertion an observation rather
/// than a guess: `RunRegistry::revival` writes that sentence into the row BEFORE the collector
/// announces anything, so by the time a reader can see the sentence the decision is already taken.
/// A gate reading a count after a plain `panicked` would be racing the daemon.
///
/// ⚠⚠ The subject's driver is named BEFORE the second run is submitted, which is the rule the
/// promotion gate above paid a red to learn: a process table publishes parentage, not which run a
/// driver belongs to, so WHEN a pid is read is what names it.
///
/// ⚠ And the daemon's own pid is checked across the kill, because a gate that let the daemon die
/// with its driver would be register item 543's restart question wearing a second name.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_run_whose_driver_process_dies_is_put_back_on_a_new_one() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-driver-lost-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    let daemon = daemon_pid(&sock).expect("the daemon is running");
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");

    // ── THE SUBJECT: a LOOP, because only a run with a machine records a place ───────────────
    let pane = loop_session(&mut conn, "subject");
    start_loop(
        &mut conn,
        "subject",
        pane,
        "a loop that outlives its driver",
    );
    assert!(
        wait_for(Duration::from_secs(30), || driver_pids(&sock).len() == 1),
        "⚠⚠ THE PREMISE FAILED: this daemon ships `run-driver-process` on (register item 544), so \
         the loop above should be driven by a process of its own and there is nothing here to \
         kill. Found {:?}.",
        driver_pids(&sock),
    );
    let subject = driver_pids(&sock)[0];

    // ── THE BOUNDARY: a run whose plugin walks no statechart, so it records no place ─────────
    start_a_run_that_cannot_converge(&sock, "nowhere");
    assert!(
        wait_for(Duration::from_secs(30), || driver_pids(&sock).len() == 2),
        "⚠⚠ THE PREMISE FAILED: the second run got no driver of its own, so there is nothing to \
         kill for the refusal arm. Found {:?}.",
        driver_pids(&sock),
    );
    let nowhere = driver_pids(&sock)
        .into_iter()
        .find(|pid| *pid != subject)
        .expect("the second run's driver");

    // ⚠⚠⚠ THE PREMISE THE WHOLE SUBJECT ARM RESTS ON: the loop has taken a step and written a
    // place down. A run with no place IS the refusal arm, so without this the gate could be
    // measuring the same answer twice and calling one of them a rescue.
    assert!(
        wait_for(Duration::from_secs(90), || resumable_runs(&state, 1)),
        "⚠⚠ THE PREMISE FAILED: the daemon never persisted a live run carrying BOTH a place and a \
         request under {}, so the subject is not a run that can be put back at all.",
        state.join("sprag").display(),
    );

    // THE KILL: outright and from outside, which is what an OOM killer and a person with `ps` both
    // do — nothing gets to write a tidy outcome on the way out.
    //
    // SAFETY: `subject` was read from the process table as a driver of THIS test's own daemon,
    // started beside a socket this test made.
    unsafe { libc::kill(subject as libc::pid_t, libc::SIGKILL) };
    assert!(
        wait_for(Duration::from_secs(10), || !still_running(subject)),
        "the driver process {subject} did not die",
    );
    assert_eq!(
        daemon_pid(&sock),
        Some(daemon),
        "⚠⚠ THE PREMISE FAILED: the daemon did not survive its driver, so what is measured below \
         is a restart (register item 543) and not a driver's death",
    );

    // ⚠ THE SESSION SCOPES THE CONNECTION, NOT THE ANSWER: this slot lists every run this daemon
    // holds, which is why both rows are read out of one call.
    let rows_of = |conn: &mut HostConn| {
        conn.call(
            "scene/query",
            json!({
                "session": "subject",
                "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
            }),
        )
        .expect("the runs slot answers")
        .as_array()
        .expect("a list of runs")
        .clone()
    };
    let status_of = |rows: &[Value], id: u64| -> Value {
        rows.iter()
            .find(|run| run["id"] == json!(id))
            .unwrap_or_else(|| panic!("run {id} is not in the rows at all: {rows:?}"))["state"]
            .clone()
    };

    // ── THE CLAIM, PART ONE: a NEW process is driving the same run ───────────────────────────
    //
    // ⚠ Counted as *a pid that is neither of the two this test started*, because the process table
    // cannot say which run a driver belongs to — the promotion gate above records the round that
    // learned it.
    let replacement = || {
        driver_pids(&sock)
            .into_iter()
            .find(|pid| *pid != subject && *pid != nowhere)
    };
    assert!(
        wait_for(Duration::from_secs(30), || replacement().is_some()),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 671: A RUN WHOSE DRIVER PROCESS DIED WAS NEVER PUT BACK ON A NEW \
         ONE. The daemon is alive and answering, the loop has a place and a request in its own run \
         log, and a BOOT would have put this exact run back (register item 543) — so the same run \
         gets two different fates depending on whether the daemon happened to restart. Drivers \
         now: {:?}, killed {subject}.",
        driver_pids(&sock),
    );
    let replacement = replacement().expect("the replacement driver");
    let rows = rows_of(&mut conn);
    assert_eq!(
        status_of(&rows, 0)["status"],
        json!("running"),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 671: a replacement driver is running but the ROW does not say the \
         run is. A reader watching this loop is told it is dead while a process types at its \
         agent, which is worse than either answer alone. Rows: {rows:?}",
    );

    // ── THE CLAIM, PART TWO: and the run that CANNOT come back is told so ────────────────────
    //
    // SAFETY: as above — a driver of this test's own daemon, read from the process table.
    unsafe { libc::kill(nowhere as libc::pid_t, libc::SIGKILL) };
    assert!(
        wait_for(Duration::from_secs(10), || !still_running(nowhere)),
        "the driver process {nowhere} did not die",
    );
    let told = wait_for(Duration::from_secs(30), || {
        status_of(&rows_of(&mut conn), 1)["error"]
            .as_str()
            .is_some_and(|why| why.contains("did not put it back on a new driver"))
    });
    let rows = rows_of(&mut conn);
    assert!(
        told,
        "⛔⛔⛔⛔⛔ REGISTER ITEM 671: a run whose driver died and which this daemon is NOT going to \
         put back says only that it failed. The person reading it cannot tell *a rescue is coming* \
         from *nothing is coming*, and the only difference that matters to them is whether to \
         start it again themselves. `RunRegistry::revival` is where the reason is supposed to be \
         written into the row. Rows: {rows:?}",
    );
    assert_eq!(
        status_of(&rows, 1)["status"],
        json!("panicked"),
        "the refused run left the row in a word nobody decided on: {rows:?}",
    );
    assert_eq!(
        driver_pids(&sock),
        vec![replacement],
        "⛔⛔⛔⛔⛔ REGISTER ITEM 671: this daemon started a driver for a run it had just told the \
         person it would not put back. A run with no place has nowhere to be put back to, so \
         whatever that process is doing, it is not resuming anything — and it will die and be \
         replaced forever. Killed {nowhere}, expected only the subject's replacement.",
    );
    drop(conn);
    drop(guard);
}

/// ⚠⚠ **A RUN WHOSE DAEMON DIED IS ACCOUNTED FOR** — the record that used to vanish with the
/// process that was keeping it.
///
/// Before this, a restart left `runs` answering *"no runs"*, which is the SAME answer a daemon
/// nobody has ever asked for a loop gives. A person who started a bounded loop, walked away, and
/// came back to a restarted daemon could not tell *it finished and the record is gone* from *it
/// never ran*.
///
/// ⚠ The run is deliberately LONG (a hundred thousand iterations against a pane that never prints
/// its sentinel), so it is certainly still going when the daemon is SIGKILLed under it — an outright
/// kill, so nothing gets to write a tidy terminal state on the way out. The wait before the kill is
/// on the CONDITION the assertion reads — the run is on disk AND unfinished — because the save loop
/// is on a timer.
///
/// ⚠⚠ The second half is the authority one and is why this is not just serialization: `opened_by`
/// must NOT come back for THIS run. Panes survive a restart and so do their ids, so the seat is not
/// what identifies an asker — and the pane staged here runs a plain shell, which holds no
/// conversation, so there is nothing a successor could honestly match the run to.
///
/// ⚠⚠⚠ **The sentence this used to give was false and is worth naming**: it said a restored pane's
/// occupant *is* a plain shell and never the agent that asked. Measured 2026-08-18 — an allowlisted
/// agent comes back `--resume`d, holding the same conversation. See `RunRegistry::restore`'s rule 1
/// for the decision that was re-taken on the truth, and the lib gate
/// `the_run_a_shutdown_left_behind_comes_back_interrupted_and_keeps_the_conversation_that_asked`
/// for the arm where a conversation DOES survive.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_pane_that_survived_a_reboot_can_still_ask_for_a_person() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-attention-durability-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    // Unique per run, so a sentence found below can only have come from THIS test's child.
    let needle = format!("REBOOTED-PANE-ASKS-{}", std::process::id());

    // Daemon A, and a session whose pane prints a marker then blocks on its pty — the marker is
    // what makes the durable wait below a wait on the CONDITION rather than on a timer.
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "work",
                "cmd": ["sh", "-c", format!("printf 'MARKER-{needle}\\n'; exec cat")],
            },
        }),
    )
    .expect("new_session answers");
    drop(conn);
    assert!(
        wait_for(Duration::from_secs(30), || saved_history_contains(
            &state,
            &format!("MARKER-{needle}")
        )),
        "the daemon never persisted the pane under {}",
        state.display(),
    );

    // The reboot: killed outright, then its successor on the same socket and state.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );
    assert!(
        wait_for(Duration::from_secs(15), || sprag(&sock, &["ls"])
            .stdout
            .contains("work")),
        "the restore never brought the session back",
    );

    // A client the message can be addressed TO. Held open for the rest of the test: the router
    // walks the attachment map at the moment the child speaks, so a connection that had dropped
    // would make the claim below fail for the wrong reason.
    let mut viewer = HostConn::connect(&sock, Duration::from_secs(5)).expect("the viewer connects");
    viewer
        .call(
            CLIENT_HELLO_METHOD,
            json!({ CLIENT_PARAM: "reboot-viewer" }),
        )
        .expect("client/hello accepted");
    viewer
        .call(
            CLIENT_ATTACH_METHOD,
            json!({ sprag_rpc::SESSION_PARAM: "work" }),
        )
        .expect("client/attach accepted");
    assert!(
        wait_for(Duration::from_secs(5), || sprag(&sock, &["list-clients"])
            .stdout
            .contains("reboot-viewer: work")),
        "the daemon never counted the viewer, so nothing could be addressed to it",
    );

    let collect = |conn: &mut HostConn| -> Value {
        conn.call(sprag_rpc::CLIENT_MESSAGES_METHOD, json!({}))
            .expect("collecting a message is a well-formed call")[sprag_rpc::MESSAGE_FIELD]
            .clone()
    };

    // THE CONTROL, FIRST: a person's own message takes this mailbox to this client on this daemon.
    let control = format!("CONTROL-{needle}");
    let said = sprag(&sock, &["display-message", "-t", "work", &control]);
    assert!(said.ok, "display-message succeeded: {}", said.stderr);
    let mut saw = Value::Null;
    assert!(
        wait_for(Duration::from_secs(10), || {
            saw = collect(&mut viewer);
            saw["text"].as_str().is_some_and(|text| text == control)
        }),
        "the control never arrived, so the mailbox is not what this test thinks it is: {saw}",
    );
    // ...and it is COLLECTED: the mailbox is empty now, so the claim's sentence cannot be this one.
    assert_eq!(
        collect(&mut viewer),
        Value::Null,
        "a collected message must leave the mailbox empty",
    );

    // THE CLAIM: the RESTORED pane's own child raises a notification. The pane came back as a plain
    // shell (a recorded `sh -c` is never re-run), so `send-keys` gives it a command line to run —
    // and `printf` in the pane is the only thing here that can produce an `ESC` the emulator parses.
    let raise = format!("printf '\\033]9;{needle}\\007'");
    let typed = sprag(&sock, &["send-keys", "-t", "work", "0", "-l", &raise]);
    assert!(typed.ok, "send-keys -l succeeded: {}", typed.stderr);
    let entered = sprag(&sock, &["send-keys", "-t", "work", "0", "Enter"]);
    assert!(entered.ok, "send-keys Enter succeeded: {}", entered.stderr);

    let mut got = Value::Null;
    assert!(
        wait_for(Duration::from_secs(20), || {
            let next = collect(&mut viewer);
            if !next.is_null() {
                got = next;
            }
            got["text"]
                .as_str()
                .is_some_and(|text| text.contains(&needle))
        }),
        "a pane that survived the reboot asked for a person and nobody was told: {got}",
    );
    let text = got["text"].as_str().expect("the message carries text");
    assert!(
        text.starts_with("pane "),
        "the sentence must name the pane the way a person types it back: {text:?}",
    );
    drop(guard);
}

/// THE ghostty-parity payoff: a killed daemon gives a pane back WITH ITS INLINE IMAGE — same id, same
/// extent, same anchor cell — not merely with the text around it.
///
/// This is the axis no other multiplexer covers. tmux and cmux persist no image; ghostty renders
/// images but has no session persistence at all, so it has nothing to restore them into. sprag stores
/// the image as Kitty transmit bytes in the same `.hist` stream as the text, which is why it costs no
/// second file, no second lifecycle and — the load-bearing part — no second decoder: the emulator that
/// replays the text replays the image.
///
/// The child PRINTS the sequence itself rather than having it written to the pty: a raw `ESC` written as
/// input would be echoed back in caret notation by the line discipline and never reach the parser.
///
/// Linux-gated: it finds the forked daemon through `/proc`, like its text sibling.
#[test]
// Linux AND macOS: the restore-after-SIGKILL path stopped being Linux-shaped when `daemon_pid`
// went through the portable process table (R343). A gate left on a test after its subject
// became portable is a claim that the subject is not — and its HELPERS come with it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_killed_daemon_gives_its_panes_back_with_their_inline_images() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-image-durability-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };

    // A 2x2 RGBA raster with distinctive pixels, transmitted at cell (0,0) under a distinctive id.
    let pixels: Vec<u8> = (1..=16u8).collect();
    let b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(&pixels)
    };
    let printf = format!("\\033_Ga=T,f=32,s=2,v=2,i=42;{b64}\\033\\\\");

    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the first daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "art",
                "cmd": ["sh", "-c", format!("printf '{printf}'; exec cat")],
            },
        }),
    )
    .expect("new_session answers");

    // The pane really is showing the image before anything is killed — otherwise a restore of nothing
    // would pass vacuously.
    assert!(
        wait_for(Duration::from_secs(10), || {
            image_summaries(&mut conn, "art")
                .iter()
                .any(|img| img["id"] == 42 && img["anchor"] == json!([0, 0]) && img["width"] == 2)
        }),
        "the child's image never reached the pane",
    );
    drop(conn);

    // Wait on the DURABLE condition: the transmit is in a committed `.hist`, not in the atomic
    // write's temp (see `saved_history_contains`).
    assert!(
        wait_for(Duration::from_secs(30), || saved_history_contains(
            &state, "_Ga=T"
        )),
        "the daemon never persisted the pane's image under {}",
        state.display(),
    );

    // The reboot.
    let pid = daemon_pid(&sock).expect("the daemon is running");
    kill_daemon(pid);
    let _ = std::fs::remove_file(&sock);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the second daemon never started serving",
    );

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("reconnect");
    let mut seen = Vec::new();
    let restored = wait_for(Duration::from_secs(15), || {
        seen = image_summaries(&mut conn, "art");
        !seen.is_empty()
    });
    assert!(
        restored,
        "the restored pane came back with no image at all (summaries: {seen:?})",
    );
    assert_eq!(seen.len(), 1, "exactly the one image: {seen:?}");
    assert_eq!(seen[0]["id"], 42, "the image kept its OWN id: {seen:?}");
    assert_eq!(
        (seen[0]["width"].as_u64(), seen[0]["height"].as_u64()),
        (Some(2), Some(2)),
        "and its extent: {seen:?}",
    );
    assert_eq!(
        seen[0]["anchor"],
        json!([0, 0]),
        "and the CELL it was anchored at: {seen:?}",
    );
    drop(guard);
}

/// Every pane image summary in session `session`, flattened across its panes — `{id,width,height,
/// anchor,seq}` as the panes slot reports it. Empty when no pane is showing one (the field is additive,
/// so an image-less pane simply omits it).
// Linux AND macOS: the restore-after-SIGKILL path stopped being Linux-shaped when `daemon_pid`
// went through the portable process table (R343). A gate left on a test after its subject
// became portable is a claim that the subject is not — and its HELPERS come with it.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn image_summaries(conn: &mut HostConn, session: &str) -> Vec<serde_json::Value> {
    let listed: serde_json::Value = match conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
    ) {
        Ok(value) => value,
        Err(_) => return Vec::new(),
    };
    listed
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|pane| pane["images"].as_array())
        .flatten()
        .cloned()
        .collect()
}

/// `sprag run` LISTS a project's declared commands, and TYPES one at the pane's prompt without
/// running it — the whole vertical, over the real socket, with the daemon's `HOME` pointed at a
/// temporary project so its boot pane sits inside one.
///
/// The pane runs `cat`, so what is "typed" at it comes straight back as its own output: that is how
/// this proves the paste ARRIVED without needing a shell. The absence of a trailing newline is the
/// load-bearing assertion — a project's config names the command, but the user presses Enter.
///
/// REVERT-PROOF: append a `\n` to the pasted line and the "no newline" assertion fails; drop the
/// `run` verb's paste and the pane echoes nothing.
#[test]
fn the_cli_run_lists_a_projects_commands_and_types_one_at_the_prompt() {
    let project = std::env::temp_dir().join(format!("sprag-cli-project-{}", std::process::id()));
    std::fs::create_dir_all(&project).expect("create the temp project");
    std::fs::write(
        project.join(sprag_host::PROJECT_FILE),
        "[[command]]\nname = \"greet\"\nrun = [\"echo\", \"two words\"]\n",
    )
    .expect("write the project config");

    let (_host, sock) = spawn_host_env(&[("HOME", &project.display().to_string())]);

    // The listing: `name<TAB>command line`, with the multi-word argument quoted back into one word.
    let listed = sprag(&sock, &["run"]);
    assert!(listed.ok, "run listed: {}", listed.stderr);
    assert_eq!(
        listed.stdout.trim(),
        "greet\techo 'two words'",
        "one line per command, name then the line it would run: {:?}",
        listed.stdout
    );

    // An unknown name is a clean error naming what IS declared, never a silent no-op.
    let unknown = sprag(&sock, &["run", "nope"]);
    assert!(!unknown.ok, "an unknown command fails");
    assert!(
        unknown.stderr.contains("nope") && unknown.stderr.contains("greet"),
        "the error names the miss and the alternatives: {}",
        unknown.stderr
    );

    // The delivery: typed at the pane, which is `cat`, so it echoes back.
    let typed = sprag(&sock, &["run", "greet"]);
    assert!(typed.ok, "run greet succeeded: {}", typed.stderr);
    let echoed = wait_for_pane_text(&sock, "echo 'two words'");
    // The line appears EXACTLY ONCE: that one copy is the terminal's echo of what was typed. A
    // trailing newline would have completed `cat`'s line-buffered read, so `cat` would write the
    // line back and it would appear TWICE — which is precisely the "it ran" signal that must not
    // happen here. (Measured both ways before being written down.)
    assert_eq!(
        echoed.matches("echo 'two words'").count(),
        1,
        "the line was typed but NOT executed — the Enter is the user's: {echoed:?}"
    );

    std::fs::remove_dir_all(&project).ok();
}

/// Poll pane 0's full text until it contains `needle`, returning it. Waits on the CONDITION the
/// assertion reads rather than on a timer.
fn wait_for_pane_text(sock: &Path, needle: &str) -> String {
    let mut conn = HostConn::connect(sock, Duration::from_secs(5)).expect("connect");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = String::new();
    while Instant::now() < deadline {
        let answer = conn
            .call(
                "scene/query",
                json!({ "path": sprag_host::pane_input_path(0, sprag_host::wire::FULL_TEXT_SLOT) }),
            )
            .expect("full_text query");
        last = answer.as_str().unwrap_or_default().to_owned();
        if last.contains(needle) {
            return last;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("pane 0 never showed {needle:?}; last text was {last:?}");
}

/// `sprag agent` against a REAL daemon (H3 slice 5): the pane whose screen says an agent is waiting
/// for an answer is reported, the shell beside it is not, and naming a pane also says WHICH RULE
/// decided and how to correct it.
///
/// # What only a live run can prove here
///
/// The verb reads three wire field names (`state`, `name`, `rule`) out of a key (`agent`) that a
/// different crate writes, and a unit test over hand-written JSON would agree with itself about all
/// four. This is the same seam R253 was bitten at, and the CLI is where it is cheapest to hold: the
/// daemon under test is the shipped binary, the screen is painted by a real child, and the detector
/// runs where it runs in production.
///
/// The pane is agent-SHAPED rather than a credentialed agent — H3's discipline throughout, and a gate
/// that needed an API key is a gate nobody runs. `Blocked` is the state used because it rests on
/// evidence PRESENT on the screen (a bottom-anchored choice list), so it publishes on sight with no
/// settle window and this test needs no sleep.
///
/// REVERT-PROOF: have the verb print every pane rather than only the claimed ones and the two-line
/// assertion fails; drop the `require_pane` pre-flight and the unknown-pane assertion fails.
#[test]
fn the_cli_reports_which_pane_an_agent_is_waiting_in() {
    // `claude`'s resting glyph in the title (OSC 2) is the fingerprint; the numbered choice list in
    // the bottom rows is what the `dialog-choice-list` rule reads. Then `cat`, so the pane goes quiet.
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\
         \\342\\235\\257 1. Yes\\n  2. No\\n'; cat",
    ]);
    wait_for_pane_text(&sock, "2. No");
    // A second pane running a plain shell — the population that must stay silent.
    let split = sprag(&sock, &["split-window", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let shell = split.stdout.trim().to_owned();

    let listed = sprag(&sock, &["agent"]);
    assert!(listed.ok, "agent succeeded: {}", listed.stderr);
    let lines: Vec<&str> = listed.stdout.lines().collect();
    assert_eq!(
        lines.len(),
        1,
        "only the pane a manifest CLAIMS is listed — a shell is not an agent at rest: {:?}",
        listed.stdout,
    );
    assert!(
        lines[0].starts_with("0: blocked  claude  rule=dialog-choice-list  seq="),
        "ID: STATE  NAME  rule=RULE  seq=N, from the daemon's own verdict: {:?}",
        lines[0],
    );
    assert!(
        !listed.stdout.contains(&format!("{shell}: ")),
        "the shell pane contributes nothing: {:?}",
        listed.stdout,
    );

    // Naming the pane turns the reading into a diagnosis: the same line, plus the remedy.
    let explained = sprag(&sock, &["agent", "0"]);
    assert!(explained.ok, "agent 0 succeeded: {}", explained.stderr);
    assert!(
        explained.stdout.contains("rule=dialog-choice-list")
            && explained.stdout.contains("[[agent]] block in config.toml"),
        "a named pane says which rule fired and what to edit: {:?}",
        explained.stdout,
    );
    // ⚠⚠⚠ **AND WHAT IT IS ASKING.** This gate asserted the rule and the remedy and NOT the
    // question, so the shell mouth published `blocked` and threw away a menu the daemon had
    // already parsed — for a whole round after R367 put it on this very surface for the agent
    // mouth. A gate that checks the diagnosis and not the content is how a silence survives.
    assert!(
        explained.stdout.contains("it is asking:")
            && explained.stdout.contains("1. Yes")
            && explained.stdout.contains("2. No"),
        "⚠⚠⚠ a person told their agent is WAITING must be told what for, or they go and read the \
         pane themselves — which is the re-derivation this key exists to end: {:?}",
        explained.stdout,
    );
    assert!(
        explained
            .stdout
            .lines()
            .any(|line| line.contains("->") && line.contains("1. Yes")),
        "and WHICH option a bare Enter would take, marked — on a permission dialog that is the \
         difference between confirming a command and declining it: {:?}",
        explained.stdout,
    );
    assert!(
        explained.stdout.contains("sprag answer-pane 0"),
        "⚠⚠ ...and the verb that answers it, naming this pane. A surface that shows a question and \
         no way to answer it sends the reader to `send-keys`, which is the one act the consent \
         contract exists to stop: {:?}",
        explained.stdout,
    );

    // ...and the pane with no agent, when NAMED, says so — the answer D3 requires to be
    // distinguishable from `idle`, rather than the silence the list gives it.
    let quiet = sprag(&sock, &["agent", &shell]);
    assert!(
        quiet.ok,
        "agent on a shell pane succeeded: {}",
        quiet.stderr
    );
    assert!(
        quiet.stdout.contains("no agent") && quiet.stdout.contains("not the same as idle"),
        "a named shell pane is answered, not ignored: {:?}",
        quiet.stdout,
    );

    // An absent pane is an ERROR, not an empty answer: the caller named a specific pane.
    let missing = sprag(&sock, &["agent", "99"]);
    assert!(
        !missing.ok,
        "an unknown pane is refused: {:?}",
        missing.stdout
    );
    assert!(
        missing.stderr.contains("no pane 99"),
        "and says which: {:?}",
        missing.stderr,
    );
}

/// `sprag agent` says a broken `config.toml` FIRST, and says it on stderr — the case where the
/// remedy the verb prints is otherwise a trap.
///
/// # The defect this closes
///
/// The verb tells a user with a wrong verdict to edit an `[[agent]]` block. When that file will not
/// parse, the daemon keeps the last list that worked and reports it to a `tracing::warn` nobody
/// reads — so a user who has ALREADY written the block is told to write it, sees nothing change, and
/// has no way from here to learn the file was refused. Worse for the pane the block was meant to
/// claim: `no agent  (no manifest claims this pane)` is what an unparsed claim looks like from here,
/// which reads as a detection problem rather than a syntax one.
///
/// # Why stderr, and why the assertion is about the STREAM
///
/// The listing is `ID: STATE  NAME  rule=RULE  seq=N`, a shape a script slices. A caveat on stdout
/// would make every such script skip a line it never had to skip. So the split is the claim: the
/// sentence is on stderr, and stdout carries the rows and only the rows.
///
/// REVERT-PROOF, both measured, and the FIRST attempt at the pair was wrong in a way worth keeping:
/// swapping `eprintln!` for `println!` does NOT fail the stdout assertion, because the sentence
/// leaves the stream the earlier assertion reads and that one fails first. The mutation that isolates
/// the stdout claim is printing to BOTH streams — measured red, on that assertion, naming the row it
/// polluted. Dropping the query is what fails the stderr assertion.
#[test]
fn the_cli_says_a_refused_manifest_file_before_it_reports_any_verdict() {
    let dir = std::env::temp_dir().join(format!("sprag-cli-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sprag")).expect("create the temp config dir");
    // Valid TOML, invalid MANIFEST: a `disable` naming a rule that does not exist. Nothing else in
    // the file stops working, which is what makes the failure silent.
    std::fs::write(
        dir.join("sprag").join(sprag_host::config::CONFIG_FILE),
        "[[agent]]\nname = \"claude\"\ndisable = [\"nope\"]\n",
    )
    .expect("write the broken config");

    let (_host, sock) =
        spawn_host_with(&["cat"], &[("XDG_CONFIG_HOME", &dir.display().to_string())]);

    let run = sprag(&sock, &["agent"]);
    assert!(run.ok, "agent still answers: {}", run.stderr);
    assert!(
        run.stderr.contains("nope") && run.stderr.contains("last worked"),
        "the refusal is reported, with what the daemon did about it: {:?}",
        run.stderr,
    );
    assert!(
        run.stdout.is_empty(),
        "and the rows stream carries rows only — a script slicing it skips nothing: {:?}",
        run.stdout,
    );

    // The diagnosing form is where the trap was: it names the file to edit, so it must also say the
    // file is currently being refused.
    let named = sprag(&sock, &["agent", "0"]);
    assert!(named.ok, "agent 0 still answers: {}", named.stderr);
    assert!(
        named.stdout.contains("no agent"),
        "the pane still reports as unclaimed — the reading a broken manifest produces: {:?}",
        named.stdout,
    );
    assert!(
        named.stderr.contains("nope"),
        "and the caveat is on this call too, which is the one that says to edit that file: {:?}",
        named.stderr,
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The pane lifecycle over the CLI: list, split, list again, kill, and the refusals on either side
/// of it. Drives the real daemon, so a break in the wire vocabulary these verbs share with it —
/// `spawn`, `close`, the `panes` slot — fails here rather than in someone's shell.
///
/// The listing is checked for the SHAPE a script slices (`ID: COLSxROWS  COMMAND`), not just for
/// the id being present: `cut -d: -f1` is the documented way to feed these ids to the other verbs,
/// so the colon is part of the contract.
#[test]
fn the_cli_splits_lists_and_kills_panes_over_the_socket() {
    let (_host, sock) = spawn_host();

    // The boot window holds exactly its one pane, listed id-first.
    let listed = sprag(&sock, &["panes"]);
    assert!(listed.ok, "panes succeeded: {}", listed.stderr);
    let first: Vec<&str> = listed.stdout.lines().collect();
    assert_eq!(first.len(), 1, "one boot pane: {:?}", listed.stdout);
    assert!(
        first[0].starts_with("0: ") && first[0].contains('x') && first[0].contains("cat"),
        "ID: COLSxROWS  COMMAND: {:?}",
        first[0],
    );
    // The scope is OPTIONAL for a pane command, and naming it explicitly means the same thing.
    assert_eq!(
        sprag(&sock, &["panes", "-t", "0"]).stdout,
        listed.stdout,
        "-t 0 and the default scope are the same session here",
    );

    // split-window: a new pane in the same window, its id printed for a script to capture.
    let split = sprag(&sock, &["split-window", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let new_pane = split.stdout.trim().to_owned();
    assert!(
        new_pane.parse::<u64>().is_ok(),
        "it prints the new pane id: {new_pane:?}",
    );
    let listed = sprag(&sock, &["panes"]);
    assert_eq!(
        listed.stdout.lines().count(),
        2,
        "the window now holds two panes: {:?}",
        listed.stdout,
    );
    assert!(
        listed
            .stdout
            .lines()
            .any(|line| line.starts_with(&format!("{new_pane}: "))),
        "including the one just made: {:?}",
        listed.stdout,
    );

    // kill-pane: it goes, and the boot pane stays.
    let killed = sprag(&sock, &["kill-pane", &new_pane]);
    assert!(killed.ok, "kill-pane succeeded: {}", killed.stderr);
    assert!(
        killed.stdout.contains(&format!("killed pane {new_pane}")),
        "it confirms which: {:?}",
        killed.stdout,
    );
    let listed = sprag(&sock, &["panes"]);
    assert_eq!(
        listed.stdout.lines().count(),
        1,
        "back to the boot pane: {:?}",
        listed.stdout,
    );

    // Killing it again is a clean "no such pane", not a silent success.
    let again = sprag(&sock, &["kill-pane", &new_pane]);
    assert!(!again.ok, "a second kill fails");
    assert!(
        again.stderr.contains(&format!("pane {new_pane}")),
        "and names the miss: {}",
        again.stderr,
    );

    // No id at all is tmux's form: kill the pane the session is ON. The boot pane is the only
    // one left, so this empties the window — and the refusal above proves the id path still runs.
    let bare = sprag(&sock, &["kill-pane"]);
    assert!(bare.ok, "kill-pane with no target: {}", bare.stderr);
    assert!(
        bare.stdout.contains("the active pane"),
        "and says which pane it meant: {:?}",
        bare.stdout,
    );
    assert_eq!(
        sprag(&sock, &["panes"]).stdout.trim(),
        "",
        "the window is empty, so the bare form really acted on the pane the session was on",
    );

    // SHAPE errors are still local, before any request goes out. Note what is NOT one any more:
    // `kill-pane nope` used to fail here as "pane id must be a number", and `nope` is a NAME now —
    // which can only be told apart from a real pane BY ASKING, so that check is necessarily remote.
    let junk = sprag(&sock, &["kill-pane", "1", "2"]);
    assert!(
        !junk.ok && junk.stderr.contains("unexpected argument"),
        "arg error: {}",
        junk.stderr,
    );
}

/// `sprag panes` says WHO ASKED for a pane, so an operator can see what an agent opened.
///
/// The provenance is put on the wire the way the agent surface puts it there — an invoke carrying
/// `opened_by` — because no CLI verb stamps one (a decision, recorded in the design: the CLI has no
/// pane identity of its own to claim). That makes this the reading half's only end-to-end coverage,
/// so it pins the RENDERED LINE rather than the presence of the fact.
#[test]
fn the_pane_listing_says_which_pane_asked_for_a_pane() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");
    let opened = conn
        .call(
            "scene/invoke",
            json!({
                "session": "0",
                "path": mux_action_path(SPAWN_ACTION),
                "args": { "cmd": ["cat"], "opened_by": 0 },
            }),
        )
        .expect("a spawn naming the boot pane")
        .as_u64()
        .expect("the new pane's id");

    let listed = sprag(&sock, &["panes"]);
    assert!(listed.ok, "panes succeeded: {}", listed.stderr);
    let lines: Vec<&str> = listed.stdout.lines().collect();
    assert_eq!(lines.len(), 2, "two panes: {:?}", listed.stdout);
    let opened_line = lines
        .iter()
        .find(|line| line.starts_with(&format!("{opened}: ")))
        .expect("the opened pane is listed");
    assert!(
        opened_line.contains("opened by pane 0"),
        "the listing names the pane that asked: {opened_line:?}",
    );
    assert!(
        !lines[0].contains("opened by"),
        "and says nothing about a pane nobody claims — the boot pane is the person's: {:?}",
        lines[0],
    );
}

/// `sprag rename-session` moves the session's ADDRESS, and everything addressed by it moves too —
/// end to end against a real daemon, because that is the only place the three halves meet.
///
/// The JOURNAL assertion is the load-bearing one. A rename that minted a fresh channel would leave
/// this reading empty and every parked client asleep, and nothing in the registry or the CLI would
/// show it: the session would be alive under its new name with its clients waiting on a key nothing
/// reaches again.
#[test]
fn a_session_can_be_renamed_and_its_address_takes_its_clients_with_it() {
    let (_host, sock) = spawn_host();

    // Something for the journal to hold, so "the journal survived" is a claim with a witness.
    assert!(sprag(&sock, &["new-window", "-t", "0", "alpha"]).ok);
    let before = sprag(&sock, &["events", "-t", "0", "--since", "0"]);
    assert!(
        before.stdout.contains("window_created\talpha"),
        "the journal holds the window's birth before the rename: {:?}",
        before.stdout,
    );

    let renamed = sprag(&sock, &["rename-session", "-t", "0", "prod"]);
    assert!(renamed.ok, "rename-session succeeded: {}", renamed.stderr);
    assert_eq!(renamed.stdout.trim(), "renamed to prod");

    // The new address resolves and the old one does not — that is what makes a name an address.
    let listed = sprag(&sock, &["ls"]);
    assert!(
        listed.stdout.contains("prod:"),
        "the listing names the session by its new name: {:?}",
        listed.stdout,
    );
    let stale = sprag(&sock, &["panes", "-t", "0"]);
    assert!(!stale.ok, "the retired name resolves to nothing");
    assert!(
        stale.stderr.contains("no session named \"0\""),
        "and says so as a sentence: {:?}",
        stale.stderr,
    );

    // THE ONE THAT MATTERS: the journal came across, and the rename is IN it — as one rename,
    // naming the address the client held and the one it answers to now.
    let after = sprag(&sock, &["events", "-t", "prod", "--since", "0"]);
    assert!(
        after.stdout.contains("window_created\talpha"),
        "the journal moved with the session rather than being minted fresh: {:?}",
        after.stdout,
    );
    assert!(
        after.stdout.contains("session_renamed\t0\tprod"),
        "and the change reads as ONE rename carrying both names: {:?}",
        after.stdout,
    );
    assert!(
        !after.stdout.contains("session_closed"),
        "never as a death: {:?}",
        after.stdout,
    );

    // A rename onto a name another session holds is refused as a sentence, and changes nothing.
    assert!(sprag(&sock, &["new", "spare"]).ok);
    let taken = sprag(&sock, &["rename-session", "-t", "prod", "spare"]);
    assert!(!taken.ok, "a duplicate address is refused");
    assert_eq!(
        taken.stderr.trim(),
        "sprag: a session named \"spare\" already exists",
        "the refusal is the registry's own sentence, not a list of four the CLI imagined",
    );
    assert!(
        sprag(&sock, &["panes", "-t", "prod"]).ok,
        "and the refused rename moved nothing",
    );
}

/// A session NAME is an address, so the grammar is enforced at both places one can enter — and
/// the listing this protects is asserted by reading it back.
///
/// MEASURED before it was written, against a live daemon at `9bca338`: `rename-session ""`
/// answered `renamed to `, a name holding a newline printed as TWO rows of `sprag ls`, and
/// `x\ty\x1b[31m` put an escape sequence into the reader's terminal. R295 settled this rule for a
/// PANE name; a rename is what made the same hole reachable for the widest address the daemon has.
#[test]
fn a_session_name_is_an_address_and_the_grammar_holds_at_both_ends() {
    let (_host, sock) = spawn_host();

    // A name is TRIMMED and the verb reports what the DAEMON recorded, not the argument sent.
    let renamed = sprag(&sock, &["rename-session", "-t", "0", "  work  "]);
    assert!(renamed.ok, "a padded name is trimmed: {}", renamed.stderr);
    assert_eq!(renamed.stdout.trim(), "renamed to work");
    assert!(
        sprag(&sock, &["panes", "-t", "work"]).ok,
        "and `work` is the address that resolves",
    );

    // Every rule, at the RENAME end — and each one is answered with the rule IT broke. Until R325
    // every line here read the same sentence, a list of all four rules, because the daemon's
    // `SessionNameError` could not cross the wire; the pairs below are what it had known all along.
    for (hostile, rule) in [
        ("", "blank"),
        ("   ", "blank"),
        ("a\nb", "control character"),
        ("x\u{1b}[31m", "control character"),
    ] {
        let refused = sprag(&sock, &["rename-session", "-t", "work", hostile]);
        assert!(
            !refused.ok,
            "a session cannot be renamed to {hostile:?}: {}",
            refused.stdout,
        );
        assert!(
            refused.stderr.contains(rule),
            "the refusal names the rule {hostile:?} broke, not every rule there is: {:?}",
            refused.stderr,
        );
    }
    let long = "z".repeat(81);
    let refused = sprag(&sock, &["rename-session", "-t", "work", &long]);
    assert!(!refused.ok, "an over-long name is refused");
    assert!(
        !refused.stderr.contains("control character"),
        "and an over-long name is NOT told about control characters — the discriminator this \
         round bought, and the assertion that fails if the guess comes back: {:?}",
        refused.stderr,
    );
    assert!(
        sprag(&sock, &["panes", "-t", "work"]).ok,
        "and every refusal left the address alone",
    );

    // The same grammar at the CREATE end, which is the other way a name enters.
    for hostile in ["", "a\nb"] {
        let refused = sprag(&sock, &["new", hostile]);
        assert!(!refused.ok, "a session cannot be CREATED as {hostile:?}");
    }

    // THE CONTROL — all digits is allowed here where a pane name refuses it, because a session has
    // no ordinal to be confused with and this registry ALLOCATES exactly those names.
    assert!(
        sprag(&sock, &["rename-session", "-t", "work", "7"]).ok,
        "an all-digit session name is legal: the daemon's own boot session is called `0`",
    );
    let listed = sprag(&sock, &["ls"]);
    assert_eq!(
        listed.stdout.lines().count(),
        1,
        "and the listing is still ONE line for one session — the contract the control rule \
         protects: {:?}",
        listed.stdout,
    );
}

/// The same grammar one level down (R306): a WINDOW name is an address too, and until this round it
/// was the only one of the three with no rules at all.
///
/// The sibling above is the model, and the two differences are the point. The RECORDED name is
/// asserted from the printed line — a verb that echoed its argument would print `  main  ` — which
/// is the half `rename-window` could not report before, since the action answered `null`
/// (`WIRE_PROTOCOL` 8 → 9). And the refusal is asserted to name the RULE rather than the wire's
/// two-way disjunction, because the CLI parses with the daemon's own function before it sends.
#[test]
fn a_window_name_is_an_address_and_the_grammar_holds_at_both_ends() {
    let (_host, sock) = spawn_host();

    let renamed = sprag(&sock, &["rename-window", "-t", "0", "  main  "]);
    assert!(renamed.ok, "a padded name is trimmed: {}", renamed.stderr);
    assert_eq!(
        renamed.stdout.trim(),
        "renamed to main",
        "the DAEMON's recorded name, not the argument this verb was handed",
    );

    for hostile in ["", "   ", "a\nb", "x\u{1b}[31m", &"z".repeat(81)] {
        let refused = sprag(&sock, &["rename-window", "-t", "0", "main", hostile]);
        assert!(
            !refused.ok,
            "a window cannot be renamed to {hostile:?}: {}",
            refused.stdout,
        );
        assert!(
            refused.stderr.contains("a window name"),
            "the refusal names the rule that was broken: {:?}",
            refused.stderr,
        );
    }

    // The same grammar at the CREATE end, which is the other way a name enters over this CLI.
    for hostile in ["", "a\nb"] {
        let refused = sprag(&sock, &["new-window", "-t", "0", hostile]);
        assert!(
            !refused.ok,
            "a window cannot be CREATED as {hostile:?}: {}",
            refused.stdout,
        );
    }

    // THE CONTROL — all digits is allowed, for the session name's reason one level down: the
    // registry MINTS `0`, `1`, `2`, so a grammar that refused digits would refuse its own windows.
    assert!(
        sprag(&sock, &["rename-window", "-t", "0", "main", "7"]).ok,
        "an all-digit window name is legal: this session's boot window is called `0`",
    );
    let listed = sprag(&sock, &["windows", "-t", "0"]);
    assert_eq!(
        listed.stdout.lines().count(),
        1,
        "and the listing is still ONE line for one window: {:?}",
        listed.stdout,
    );
}

/// A window RENAME and a pane MOVE reach a reader as what they are — one event each, carrying the
/// fact no later read could recover.
///
/// Live, because the derivation is only half the path: the printer has to render the DETAIL beside
/// the subject, and a test on the diff alone would pass on a build whose `sprag events` prints the
/// subject and drops the rest.
///
/// Each assertion has its NEGATIVE beside it, because the defect this replaced was not a missing
/// event but a pair of wrong ones — `window_created beta` + `window_closed alpha` for a rename, and
/// `pane_closed 1` + `pane_created 1` in one batch for a move.
#[test]
fn a_renamed_window_and_a_moved_pane_are_not_reported_as_deaths() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new-window", "-t", "0", "alpha"]).ok);

    assert!(sprag(&sock, &["rename-window", "-t", "0", "alpha", "beta"]).ok);
    let events = sprag(&sock, &["events", "-t", "0", "--since", "0"]);
    assert!(
        events.stdout.contains("window_renamed\talpha\tbeta"),
        "one rename, naming the address a client held and the one it answers to now: {:?}",
        events.stdout,
    );
    assert!(
        !events.stdout.contains("window_closed"),
        "and no death: {:?}",
        events.stdout,
    );

    // A pane moved between windows: born in the first, joined into the second.
    assert!(sprag(&sock, &["select-window", "-t", "0", "0"]).ok);
    let born = sprag(&sock, &["split-window", "-t", "0"]);
    assert!(born.ok, "split-window succeeded: {}", born.stderr);
    let pane = born.stdout.trim().to_owned();
    assert!(sprag(&sock, &["join-pane", "-t", "0", &pane, "beta"]).ok);

    let events = sprag(&sock, &["events", "-t", "0", "--since", "0"]);
    assert!(
        events.stdout.contains(&format!("pane_moved\t{pane}\tbeta")),
        "the move names the pane AND the window it went to — which no slot serves, because \
         `panes` and `layout` answer for the current window only: {:?}",
        events.stdout,
    );
    assert!(
        !events.stdout.contains(&format!("pane_closed\t{pane}")),
        "the pane did not die: {:?}",
        events.stdout,
    );
    assert_eq!(
        events
            .stdout
            .lines()
            .filter(|line| *line == format!("pane_created\t{pane}"))
            .count(),
        1,
        "and was born exactly ONCE — at the split that made it, never again on arriving \
         somewhere: {:?}",
        events.stdout,
    );
}

/// `sprag rename-pane` names a pane, the listing says so, and every refusal is read as a SENTENCE.
///
/// End to end against a real daemon, because the whole loop is CLI-reachable here — unlike the
/// provenance above, which no CLI verb stamps. Every assertion is on the RENDERED line: a test that
/// checked the wire would pass on a build whose listing prints nothing.
#[test]
fn a_pane_can_be_named_from_the_command_line_and_the_listing_says_so() {
    let (_host, sock) = spawn_host();

    // The name is sent with surrounding whitespace, so the echo below is a claim about what the
    // DAEMON recorded rather than about the argument: the two differ, and a verb that echoed its
    // own argument would name a pane something it is not called.
    let named = sprag(&sock, &["rename-pane", "0", "  build  "]);
    assert!(named.ok, "rename-pane succeeded: {}", named.stderr);
    assert_eq!(
        named.stdout.trim(),
        "pane 0 is now \"build\"",
        "the verb reports the TRIMMED name the daemon stored, quoted",
    );
    let listed = sprag(&sock, &["panes"]);
    assert_eq!(
        listed.stdout.trim(),
        "0: 80x24  cat  name=\"build\"  (active)",
        "the whole rendered line, not just the presence of the fact",
    );

    // A name with a SPACE round-trips through the listing readably, which is why it is quoted at
    // all: `name=my build` could not be read back by anything, and this listing's fields are read
    // positionally by whoever pipes it.
    assert!(sprag(&sock, &["rename-pane", "0", "the build"]).ok);
    assert!(
        sprag(&sock, &["panes"])
            .stdout
            .contains("name=\"the build\""),
        "a name holding a space is delimited: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );
    assert!(sprag(&sock, &["rename-pane", "0", "build"]).ok);

    // A SECOND pane cannot take it. The sentence is pinned whole (R279/R283's rule) and it is the
    // DAEMON'S — since R325 consumed PINION-PR82, a refused action carries the producer's own
    // reason. This line used to read *"no pane N, or \"build\" is already taken, blank, over 80
    // bytes, all digits, or contains a control character"*: SIX causes, of which the daemon knew
    // exactly one and had no way to say which.
    let second = sprag(&sock, &["split-window"]);
    assert!(second.ok, "split-window: {}", second.stderr);
    let taken = sprag(&sock, &["rename-pane", second.stdout.trim(), "build"]);
    assert!(!taken.ok, "a name in use is refused: {:?}", taken.stdout);
    assert_eq!(
        taken.stderr.trim(),
        "sprag: pane 0 is already called \"build\"",
        "the refusal is the ONE fact the daemon observed, naming the pane that holds the name",
    );
    assert!(
        !taken.stderr.contains(" or "),
        "and it is not a disjunction — that is the whole point of the round: {:?}",
        taken.stderr,
    );

    // A name that is all digits is refused too — the rule that keeps a name and a pane NUMBER from
    // ever meaning each other where they share an argument.
    assert!(
        !sprag(&sock, &["rename-pane", "0", "42"]).ok,
        "an all-digit name is refused",
    );

    let cleared = sprag(&sock, &["rename-pane", "0", "--clear"]);
    assert!(cleared.ok, "--clear succeeded: {}", cleared.stderr);
    assert_eq!(cleared.stdout.trim(), "pane 0 has no name");
    assert!(
        !sprag(&sock, &["panes"]).stdout.contains("name="),
        "and the listing goes back to saying nothing about a name",
    );

    // A missing NAME is a local argument error, before any request goes out.
    let bare = sprag(&sock, &["rename-pane", "0"]);
    assert!(
        !bare.ok && bare.stderr.contains("--clear"),
        "and the arg error names the way to clear it: {}",
        bare.stderr,
    );
}

/// The ids `sprag panes` lists, in the order it lists them — the reading a placement verb leaves
/// UNCHANGED, which is the gap `sprag layout` exists to fill.
fn listed_pane_ids(sock: &Path) -> Vec<String> {
    sprag(sock, &["panes"])
        .stdout
        .lines()
        .filter_map(|line| line.split(':').next().map(str::to_owned))
        .collect()
}

/// `sprag layout`'s drawing, minus its `revision` header — the part a test can pin, since the
/// revision counts every change the daemon has ever made to this window.
fn drawn_layout(sock: &Path) -> String {
    let run = sprag(sock, &["layout"]);
    assert!(run.ok, "layout succeeded: {}", run.stderr);
    let (head, rest) = run.stdout.split_once('\n').expect("a header and a body");
    assert!(
        head.starts_with("revision "),
        "the arrangement's own version leads: {head:?}",
    );
    rest.to_owned()
}

/// `resize-pane -L|-R|-U|-D` over the socket — **the first gesture other than a pointer drag in
/// `sprag-gui` that has ever moved a split's share**, driven end to end against a real daemon.
///
/// The window is PINNED rather than reported by an attached client, which is what makes this
/// runnable from a shell at all: a cell has no length until somebody has measured the window, so
/// this test also pins the refusal that says so. `sprag layout` renders the share as a percentage,
/// which is the observable — and it is the DAEMON's own reading of its own tree, not this end's.
///
/// Three claims, each of which a plausible wrong implementation passes only one of:
///
/// * a direction moves the BOUNDARY, so `-R` from the LEFT pane and `-L` from the RIGHT pane move
///   it opposite ways. A verb that grew whichever pane asked would pass the first and fail here.
/// * the distance is in CELLS, so the same flag over a different window moves a different
///   percentage — the property `amount: f32` cannot have.
/// * an outcome that is not a move says WHICH nothing happened, in its own sentence.
#[test]
fn the_cli_moves_the_boundary_beside_a_pane() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let (_host, sock) = spawn_host_env(&env);
    // Nothing is watching yet: the verb refuses, and the sentence names both remedies.
    let unmeasured = sprag(&sock, &["resize-pane", "0", "-R", "3"]);
    assert!(!unmeasured.ok, "an unmeasured window refuses the move");
    assert!(
        unmeasured
            .stderr
            .contains("nothing is watching that window"),
        "and says why: {:?}",
        unmeasured.stderr,
    );

    let pinned = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "101", "-y", "30"],
        &env,
    );
    assert!(pinned.ok, "resize-window succeeded: {}", pinned.stderr);
    let split = sprag(&sock, &["split-window", "-h", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let right = split.stdout.trim().to_owned();
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane 0\n└─ pane {right}\n"),
        "an even share is where every division opens",
    );

    // 100 usable columns (one is the divider), so the boundary opens at 50 and ten cells right of
    // that is 60 — a percentage this arithmetic predicts rather than one read back off the answer.
    let grow = sprag(&sock, &["resize-pane", "0", "-R", "10"]);
    assert!(grow.ok, "resize-pane -R succeeded: {}", grow.stderr);
    assert_eq!(grow.stdout.trim(), "moved pane 0's right boundary 10 cells");
    assert_eq!(
        drawn_layout(&sock),
        format!("60% left|right\n├─ pane 0\n└─ pane {right}\n"),
        "the boundary moved right, so the pane it moved away from grew",
    );

    // THE DISCRIMINATOR for "the direction moves the boundary": the same flag family from the pane
    // on the OTHER side of it moves the share the other way, and grows the asker.
    let from_the_right = sprag(&sock, &["resize-pane", &right, "-L", "20"]);
    assert!(
        from_the_right.ok,
        "resize-pane -L succeeded: {}",
        from_the_right.stderr,
    );
    assert_eq!(
        from_the_right.stdout.trim(),
        format!("moved pane {right}'s left boundary 20 cells"),
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("40% left|right\n├─ pane 0\n└─ pane {right}\n"),
        "60 - 20 = 40: the RIGHT pane grew by moving the same boundary left",
    );

    // No boundary on the other axis at all — a different fact from a boundary at its limit, and a
    // sentence of its own. It SUCCEEDS: a key at the edge of a layout must not report a failure.
    let across = sprag(&sock, &["resize-pane", "0", "-U", "2"]);
    assert!(across.ok, "an edge is not an error: {}", across.stderr);
    assert_eq!(
        across.stdout.trim(),
        "pane 0 not resized: the pane spans the window that way, so there is no boundary to move up",
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("40% left|right\n├─ pane 0\n└─ pane {right}\n"),
        "and nothing moved",
    );

    // A distance past the wall reports how far it ACTUALLY got — the fact no outcome word carries.
    let clamped = sprag(&sock, &["resize-pane", "0", "-L", "500"]);
    assert!(clamped.ok, "a clamped move succeeded: {}", clamped.stderr);
    assert_eq!(
        clamped.stdout.trim(),
        "moved pane 0's left boundary 39 cells of the 500 asked for; it stopped at the last cell \
         the far side may keep",
    );
    let at_the_wall = sprag(&sock, &["resize-pane", "0", "-L", "1"]);
    assert!(at_the_wall.ok, "and asking again is not an error");
    assert_eq!(
        at_the_wall.stdout.trim(),
        "pane 0 not resized: the boundary is already as far left as it goes",
    );

    // A MOVED BOUNDARY IS A CHANGE AN AGENT CAN WAIT FOR — asserted here, against a live daemon,
    // because the derivation is three hops from the action (the registry bumps a window's layout
    // revision, the change funnel diffs it, the journal records it) and each hop could drop it.
    //
    // It is a LIVE assertion and not a unit one because the funnel is the DAEMON's: an in-process
    // external produces no events at all, so a unit test would have measured its own harness. That
    // is not hypothetical — it is what the first version of this check did, and it failed for a
    // reason that had nothing to do with the verb.
    let journal = sprag(&sock, &["events", "-t", "0", "--since", "0"]);
    assert!(journal.ok, "events succeeded: {}", journal.stderr);
    assert!(
        journal.stdout.contains("layout_updated"),
        "a moved boundary reached the journal: {:?}",
        journal.stdout,
    );
    // THE CONTROL, and it is what makes the line above mean anything: an edge moves nothing, so the
    // journal must not grow for it. Without this, a funnel that recorded a layout change on every
    // invoke would pass.
    let lines = journal.stdout.lines().count();
    let edge_again = sprag(&sock, &["resize-pane", "0", "-U", "2"]);
    assert!(
        edge_again.ok,
        "an edge is not an error: {}",
        edge_again.stderr
    );
    assert_eq!(
        sprag(&sock, &["events", "-t", "0", "--since", "0"])
            .stdout
            .lines()
            .count(),
        lines,
        "a boundary that did not move gives a parked agent nothing to re-read",
    );

    // The two forms are two different actions, and naming both is this end's mistake to report.
    let both = sprag(
        &sock,
        &["resize-pane", "0", "-R", "2", "-x", "40", "-y", "10"],
    );
    assert!(!both.ok, "a size AND a direction is refused");
    assert!(
        both.stderr.contains("not both"),
        "and says so: {:?}",
        both.stderr,
    );
}

/// `swap-pane -L|-R` over the socket — the DIRECTIONAL half of the verb, which had **no live
/// coverage at all** until R299 touched its flag parse.
///
/// It is its own test rather than a step in `the_cli_shows_where_the_placement_verbs_put_a_pane`,
/// whose every assertion inherits the state the one before it left: appending there broke a later
/// zoom reading, which is R272's *"read what a CONTAINER asserts before appending"* met head on.
///
/// Two claims. The flags are two DIRECTIONS (a verb that mapped every flag to one word would pass a
/// single-direction test — this one held its own `"-R" => "right"` table, a copy of the one
/// `select-pane` shed, checked by nothing), and **the EDGE succeeds**: a key bound to this must not
/// log a failure every time a user reaches the side of their own layout.
#[test]
fn the_cli_swaps_a_pane_with_the_one_in_a_direction() {
    let (_host, sock) = spawn_host();
    let split = sprag(&sock, &["split-window", "-h", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let new_pane = split.stdout.trim().to_owned();
    // ASSERTED, not assumed: the split leaves the session on the pane it opened, on the RIGHT — so
    // the first press below has somewhere to go and the second is at an edge.
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane 0\n└─ pane {new_pane}\n"),
    );

    let left = sprag(&sock, &["swap-pane", "-t", "0", "-L"]);
    assert!(left.ok, "swap-pane -L succeeded: {}", left.stderr);
    assert_eq!(
        left.stdout.trim(),
        format!("swapped pane {new_pane} with 0"),
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane {new_pane}\n└─ pane 0\n"),
        "the active pane traded with the one to ITS LEFT",
    );

    // AT THE EDGE: the same flag, from the pane it just moved to the left of the window.
    let edge = sprag(&sock, &["swap-pane", "-t", "0", "-L"]);
    assert!(
        edge.ok,
        "walking into the edge is not an error: {}",
        edge.stderr
    );
    assert_eq!(
        edge.stdout.trim(),
        format!("nothing to the left of {new_pane} to trade with"),
        "the sentence names the DIRECTION the caller asked and the pane it asked about — where \
         before R301 it said only 'that way', which is the same sentence a FLOATING pane got",
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane {new_pane}\n└─ pane 0\n"),
        "and nothing moved",
    );

    // ...and the OTHER flag is the other direction, which is what tells a direction from a toggle.
    let right = sprag(&sock, &["swap-pane", "-t", "0", "-R"]);
    assert!(right.ok, "swap-pane -R succeeded: {}", right.stderr);
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane 0\n└─ pane {new_pane}\n"),
        "back where it started, so the two flags are not one direction spelled twice",
    );

    // The SCOPE is optional now, which is what makes this verb `select-pane`'s twin at the one
    // surface a person types: every other pane verb already took `-t` or nothing, and this one
    // required it. Same request, same answer, no `-t`.
    let bare = sprag(&sock, &["swap-pane", "-L"]);
    assert!(bare.ok, "swap-pane with no -t succeeded: {}", bare.stderr);
    assert_eq!(
        bare.stdout.trim(),
        format!("swapped pane {new_pane} with 0"),
    );

    // And the ORIGIN is the leading positional — the thing `select-pane` spells `--from`. Sending
    // pane 0 back to the right names a pane the session is NOT on, so this cannot be the default
    // arm answering.
    let origin = sprag(&sock, &["swap-pane", "0", "-L"]);
    assert!(origin.ok, "swap-pane 0 -L succeeded: {}", origin.stderr);
    assert_eq!(
        origin.stdout.trim(),
        format!("swapped pane 0 with {new_pane}")
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane 0\n└─ pane {new_pane}\n"),
    );

    // ...and the ORIGIN THROUGH `-t`, which is a combination and not a repetition: `scope_and_rest`
    // runs BEFORE the verb's own parse and consumes a value-taking flag, so a second positional is
    // exactly what it could swallow. R300 found this hole one verb over — `--from` had never been
    // driven through `-t` and was "read and reasoned safe", which is not the same as run.
    let scoped_origin = sprag(&sock, &["swap-pane", "-t", "0", "0", "-R"]);
    assert!(
        scoped_origin.ok,
        "swap-pane -t 0 0 -R succeeded: {}",
        scoped_origin.stderr
    );
    assert_eq!(
        scoped_origin.stdout.trim(),
        format!("swapped pane 0 with {new_pane}"),
        "the scope parse did not eat the origin",
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane {new_pane}\n└─ pane 0\n"),
        "and it really moved the pane the positional named",
    );
}

/// What the three placement verbs DID, as the CLI can now show it — debt-register item 11.
///
/// The point of the test is the pair of readings taken at every step: `sprag panes` lists the same
/// ids in the same order across a swap and a zoom (it answers WHO, and pool order is not position),
/// while `sprag layout` shows both. Before this verb existed the first column was the only one the
/// CLI had, so a caller running the verbs written for the draws-nothing audience had to open raw
/// JSON-RPC to see whether they had worked.
///
/// It drives the real daemon, so the arrangement here is the one the daemon serves — a rendering
/// test over a hand-built snapshot cannot catch a slot read that addresses the wrong path.
#[test]
fn the_cli_shows_where_the_placement_verbs_put_a_pane() {
    let (_host, sock) = spawn_host();

    // A one-pane window roots at a leaf, so it draws with no guides at all.
    assert_eq!(drawn_layout(&sock), "pane 0\n");

    let split = sprag(&sock, &["split-window", "-h", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let new_pane = split.stdout.trim().to_owned();
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane 0\n└─ pane {new_pane}\n"),
        "the division, its ratio, and which pane is on which side",
    );

    // The SWAP: invisible in the pane listing, which is the whole complaint, and plain here.
    let before = listed_pane_ids(&sock);
    let swapped = sprag(&sock, &["swap-pane", "-t", "0", "0", &new_pane]);
    assert!(swapped.ok, "swap-pane succeeded: {}", swapped.stderr);
    assert_eq!(
        listed_pane_ids(&sock),
        before,
        "`panes` answers WHO, and a swap changes nobody — this is the reading that could not \
         observe the verb",
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane {new_pane}\n└─ pane 0\n"),
        "and the two panes have exchanged sides",
    );

    // The ZOOM: the arrangement stays READABLE while one pane fills the window, on purpose — a
    // filtered tree would blind exactly the callers these verbs exist for.
    let zoomed = sprag(&sock, &["zoom-pane", "-t", "0", "0"]);
    assert!(zoomed.ok, "zoom-pane succeeded: {}", zoomed.stderr);
    assert_eq!(
        listed_pane_ids(&sock),
        before,
        "`panes` cannot observe a zoom either",
    );
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane {new_pane}\n└─ pane 0  (fills the window)\n"),
        "both panes still placed, and the one covering them named",
    );

    // Un-zooming takes the mark away rather than leaving a reading nothing can clear.
    let unzoomed = sprag(&sock, &["zoom-pane", "-t", "0", "0", "-u"]);
    assert!(unzoomed.ok, "zoom-pane -u succeeded: {}", unzoomed.stderr);
    assert_eq!(
        drawn_layout(&sock),
        format!("50% left|right\n├─ pane {new_pane}\n└─ pane 0\n"),
    );

    // The window this verb reports is the session's CURRENT one, which `break-pane` moves (tmux's
    // behaviour) — so the reading follows the session rather than the window it was last asked about.
    let broken = sprag(&sock, &["break-pane", "-t", "0", &new_pane]);
    assert!(broken.ok, "break-pane succeeded: {}", broken.stderr);
    let born = broken.stdout.trim().to_owned();
    assert_eq!(
        drawn_layout(&sock),
        format!("pane {new_pane}\n"),
        "the new window holds the broken-out pane alone",
    );
    let back = sprag(&sock, &["select-window", "-t", "0", "0"]);
    assert!(back.ok, "select-window succeeded: {}", back.stderr);
    assert_ne!(
        born, "0",
        "break-pane made a window that is not the boot one"
    );
    assert_eq!(
        drawn_layout(&sock),
        "pane 0\n",
        "and the source window's division collapsed into its survivor",
    );

    // The scope is OPTIONAL here exactly as it is for `panes`, and means the same thing.
    assert_eq!(
        sprag(&sock, &["layout", "-t", "0"]).stdout,
        sprag(&sock, &["layout"]).stdout,
    );

    // An argument this verb does not take is refused locally, naming what it does take.
    let junk = sprag(&sock, &["layout", "0"]);
    assert!(
        !junk.ok && junk.stderr.contains("-t SESSION"),
        "arg error: {}",
        junk.stderr,
    );
}

/// `split-window -h` / `-v` divide the pane the caller names, from the shell.
///
/// The verb's FOUR forms are exercised because they are four different requests and only running
/// each proves the dispatch: bare (append — the `spawn` action), `-h PANE` and `-v PANE` (divide —
/// the `split` action), and `-b` (the other side).
///
/// Each one's ARRANGEMENT is then read off the daemon, which is the assertion that could not be
/// skipped: a CLI that mapped `-v` to `"horizontal"`, or dropped `-b`, would spawn a pane and
/// print an id exactly like a correct one. Counting panes proves the request arrived; only the
/// layout proves it arrived meaning what the user typed.
#[test]
fn split_window_divides_the_pane_it_is_given_from_the_shell() {
    let (_host, sock) = spawn_host();
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the host");

    // Bare: no direction, no target — the append tmux's bare `split-window` gives.
    let appended = split_id(&sock, &["split-window", "--", "cat"]);
    assert_eq!(
        tiled(&mut conn),
        vec![0, appended],
        "a bare split appends, as it always has",
    );

    // Each directional form, against the SAME target, read back through the daemon's own layout.
    for (args, side, dir) in [
        (
            vec!["split-window", "-h", "0", "--", "cat"],
            sprag_terminal::SplitSide::Second,
            sprag_terminal::SplitDir::Horizontal,
        ),
        (
            vec!["split-window", "-v", "0", "--", "cat"],
            sprag_terminal::SplitSide::Second,
            sprag_terminal::SplitDir::Vertical,
        ),
        (
            vec!["split-window", "-v", "-b", "0", "--", "cat"],
            sprag_terminal::SplitSide::First,
            sprag_terminal::SplitDir::Vertical,
        ),
    ] {
        let fresh = split_id(&sock, &args);
        assert_eq!(
            layout_of(&mut conn).leaf_home(sprag_terminal::PaneId(fresh)),
            Some(sprag_terminal::LeafHome::beside(
                sprag_terminal::PaneId(0),
                side,
                dir
            )),
            "{args:?} put pane {fresh} beside pane 0 on the axis and side it named",
        );
    }
}

/// Run a `split-window` form and return the pane id it printed.
fn split_id(sock: &Path, args: &[&str]) -> u64 {
    let run = sprag(sock, args);
    assert!(run.ok, "{args:?} succeeded: {}", run.stderr);
    run.stdout
        .trim()
        .parse::<u64>()
        .unwrap_or_else(|_| panic!("{args:?} prints the new pane id: {:?}", run.stdout))
}

/// `select-pane` from a shell, against a real daemon over the socket — the verb, the direction
/// walk, and the two facts that make it session state rather than a client's private idea.
///
/// The listing is the READ half: `sprag panes` marks exactly one row `(active)`, and it moves when
/// the select does. That pairing is what a live test adds over the unit ones — the action and the
/// slot are different code paths on either side of a socket, and a select that moved nothing a
/// reader could see would pass every test on one side alone.
#[test]
fn the_cli_selects_a_pane_by_id_and_by_direction_over_the_socket() {
    let (_host, sock) = spawn_host();
    // The boot pane, then two more to its right: `0 | 1 | 2` — a row wide enough that left and
    // right are different answers and neither is the whole window.
    let one = split_id(&sock, &["split-window", "-h", "0", "--", "cat"]);
    let two = split_id(
        &sock,
        &["split-window", "-h", &one.to_string(), "--", "cat"],
    );

    let active_row = |marker: &str| {
        let listed = sprag(&sock, &["panes"]);
        assert!(listed.ok, "panes succeeded: {}", listed.stderr);
        let marked: Vec<String> = listed
            .stdout
            .lines()
            .filter(|line| line.contains("(active)"))
            .map(str::to_owned)
            .collect();
        assert_eq!(marked.len(), 1, "exactly one row is active {marker}");
        marked[0].clone()
    };
    assert!(
        active_row("after the splits").starts_with(&format!("{two}:")),
        "a split leaves the session on the pane it opened — tmux's rule, and here it reaches a \
         caller that is a shell script",
    );

    // By id, back to the pane the splits started from.
    let picked = sprag(&sock, &["select-pane", "0"]);
    assert!(picked.ok, "select-pane by id: {}", picked.stderr);
    assert_eq!(picked.stdout.trim(), "selected 0");
    assert!(active_row("after a select").starts_with("0:"));

    // By direction — tmux's -R, which walks the ARRANGEMENT rather than the pane list.
    let right = sprag(&sock, &["select-pane", "-R"]);
    assert!(right.ok, "select-pane -R: {}", right.stderr);
    assert_eq!(right.stdout.trim(), format!("selected {one}"));

    // At the EDGE: well-formed, honest, and not a failure — the case a keybinding hits constantly.
    // The sentence names the DIRECTION the caller asked for, because "already on 0" — what this
    // printed until R299, and what this test asserted — answers a question nobody asked: the caller
    // did not say "put me on 0", it said "go left", and the honest answer is that there is no left.
    sprag(&sock, &["select-pane", "-L"]);
    let edge = sprag(&sock, &["select-pane", "-L"]);
    assert!(
        edge.ok,
        "walking into the edge is not an error: {}",
        edge.stderr
    );
    assert_eq!(edge.stdout.trim(), "nothing to the left of 0");
    assert!(active_row("at the edge").starts_with("0:"));

    // ...and the SAME "nothing moved" from a request that named a pane keeps the other sentence, so
    // the two are distinguishable at a shell rather than only over the wire.
    let again = sprag(&sock, &["select-pane", "0"]);
    assert!(again.ok, "a re-select is not an error: {}", again.stderr);
    assert_eq!(again.stdout.trim(), "already on 0");

    // A pane the window does not hold is the daemon's refusal, and it names the miss.
    let ghost = sprag(&sock, &["select-pane", "9999"]);
    assert!(!ghost.ok, "an unknown pane is refused");
    assert!(
        ghost.stderr.contains("9999"),
        "and names it: {}",
        ghost.stderr,
    );
    // Both namings at once is the parser's refusal, before any request goes out.
    let both = sprag(&sock, &["select-pane", "0", "-R"]);
    assert!(
        !both.ok && both.stderr.contains("give one"),
        "{}",
        both.stderr
    );
    assert!(
        active_row("after two refusals").starts_with("0:"),
        "every refusal left the session where it was",
    );
}

/// `select-pane -L|-R --from PANE` over the socket: the step is measured from the pane the CALLER
/// names, and where it goes nowhere the user stays where THEY were.
///
/// Every case is paired with the same flag asked WITHOUT `--from`, and the two answer differently.
/// A fixture where they agree cannot tell an origin that is honoured from one that is dropped —
/// which is exactly what a daemon from before this argument does with it, and the reason the wire
/// protocol number moved rather than the argument simply being added.
#[test]
fn the_cli_steps_a_direction_from_the_pane_it_names() {
    let (_host, sock) = spawn_host();
    // `0 | 1 | 2`, a row wide enough that an origin in the middle has a different left and right
    // from either end.
    let one = split_id(&sock, &["split-window", "-h", "0", "--", "cat"]);
    let two = split_id(
        &sock,
        &["split-window", "-h", &one.to_string(), "--", "cat"],
    );

    let selected = sprag(&sock, &["select-pane", &two.to_string()]);
    assert!(selected.ok, "start on the rightmost: {}", selected.stderr);

    // THE CONTROL: from where the user is (2), right is the edge.
    let control = sprag(&sock, &["select-pane", "-R"]);
    assert!(control.ok, "an edge is not an error: {}", control.stderr);
    assert_eq!(
        control.stdout.trim(),
        format!("nothing to the right of {two}")
    );

    // ...and from pane 0 the same flag crosses into 1. Same daemon, same instant, same flag: the
    // only thing that differs is the origin. Through `-t` as well, because the scope parse runs
    // BEFORE this verb's own and a flag that takes a value is exactly what it could swallow.
    let stepped = sprag(&sock, &["select-pane", "-t", "0", "-R", "--from", "0"]);
    assert!(stepped.ok, "--from: {}", stepped.stderr);
    assert_eq!(stepped.stdout.trim(), format!("selected {one}"));

    // An origin at the window's edge: NOTHING moves, and the sentence names the ORIGIN rather than
    // the pane the user is on — two different panes, which is a distinction no request without an
    // origin can even express.
    let edge = sprag(&sock, &["select-pane", "-L", "--from", "0"]);
    assert!(
        edge.ok,
        "an origin at the edge is not an error: {}",
        edge.stderr
    );
    assert_eq!(edge.stdout.trim(), "nothing to the left of 0");
    let listed = sprag(&sock, &["panes"]);
    let active: Vec<&str> = listed
        .stdout
        .lines()
        .filter(|line| line.contains("(active)"))
        .collect();
    assert_eq!(active.len(), 1, "one active row: {}", listed.stdout);
    assert!(
        active[0].starts_with(&format!("{one}:")),
        "the user stayed on {one} — a step that goes nowhere must not move them onto the ORIGIN: \
         {}",
        listed.stdout,
    );

    // An origin the window does not hold is the daemon's refusal, and this end says which argument
    // named it — the daemon cannot, because a refusal carries no payload.
    let ghost = sprag(&sock, &["select-pane", "-L", "--from", "9999"]);
    assert!(!ghost.ok, "an unknown origin is refused");
    assert!(
        ghost.stderr.contains("9999") && ghost.stderr.contains("--from"),
        "and names it as the ORIGIN, not as the target: {}",
        ghost.stderr,
    );
    // An origin with no direction to be the origin OF, refused by the parser before a request goes
    // out — the same rule the daemon applies, said in this surface's own words.
    let no_dir = sprag(&sock, &["select-pane", "0", "--from", "1"]);
    assert!(
        !no_dir.ok && no_dir.stderr.contains("--from"),
        "{}",
        no_dir.stderr
    );
    let dangling = sprag(&sock, &["select-pane", "-L", "--from"]);
    assert!(
        !dangling.ok && dangling.stderr.contains("needs a pane id"),
        "{}",
        dangling.stderr
    );
}

/// The scoped window's arrangement, as the daemon serves it.
fn layout_of(conn: &mut HostConn) -> sprag_terminal::LayoutTree {
    let value = conn
        .call(
            "scene/query",
            json!({ "path": mux_action_path(sprag_host::wire::LAYOUT_SLOT) }),
        )
        .expect("the layout query answers");
    let snapshot: sprag_terminal::LayoutSnapshot =
        serde_json::from_value(value).expect("the layout deserialises off the wire");
    let mut tree = sprag_terminal::LayoutTree::new();
    tree.set_from_wire(snapshot.tree)
        .expect("a served arrangement is well-formed");
    tree
}

/// The tiled pane ids, in paint order.
fn tiled(conn: &mut HostConn) -> Vec<u64> {
    layout_of(conn)
        .panes()
        .into_iter()
        .map(|pane| pane.0)
        .collect()
}

/// The verb's two halves arrive together or not at all, and each refusal NAMES what is missing.
///
/// This is the honesty guard the old direction-flag refusal became. sprag's daemon has no current
/// A placement is a direction AND a pane, and the pane may now be left to the daemon — so the
/// refusals left are the half-stated ones: a pane with no axis (nothing to ask for), `-b` with no
/// side to be before, two axes, and a word that is neither. A refused request must also cost
/// nothing, so each case re-counts the panes; the bare `-h` at the end is the form that used to be
/// on this list and is now honoured.
#[test]
fn split_window_refuses_a_half_stated_placement_and_honours_the_bare_form() {
    let (_host, sock) = spawn_host();

    for (args, expected) in [
        (vec!["split-window", "0"], "needs an axis"),
        (vec!["split-window", "-b"], "needs -h or -v"),
        (vec!["split-window", "-h", "-v", "0"], "only one"),
        // `nope` is a NAME now, so the missing thing is the AXIS — the same answer `0` gets.
        (vec!["split-window", "nope"], "needs an axis"),
    ] {
        let run = sprag(&sock, &args);
        assert!(!run.ok, "{args:?} is refused, not guessed at");
        assert!(
            run.stderr.contains(expected),
            "{args:?} names what is missing (want {expected:?}): {}",
            run.stderr,
        );
        assert_eq!(
            sprag(&sock, &["panes"]).stdout.lines().count(),
            1,
            "{args:?} spawned nothing",
        );
    }

    // A pane the window does not hold is the daemon's refusal, not the parser's — and it too
    // must leave nothing behind.
    // A pane the SESSION does not hold is refused by the resolver, naming what it does hold.
    let missing = sprag(&sock, &["split-window", "-v", "9999"]);
    assert!(!missing.ok, "an unreachable target is refused");
    assert!(
        missing.stderr.contains("9999") && missing.stderr.contains("panes: [0]"),
        "and the refusal names the pane and what IS there: {}",
        missing.stderr,
    );
    assert_eq!(
        sprag(&sock, &["panes"]).stdout.lines().count(),
        1,
        "a refused split spawns nothing",
    );

    // tmux's BARE `-h`: no pane, because the daemon holds the active one. This form was refused
    // ("sprag has no current pane") until `select-pane` gave it a "here" to mean.
    let here = sprag(&sock, &["split-window", "-h", "--", "cat"]);
    assert!(here.ok, "the bare directional form: {}", here.stderr);
    assert_eq!(
        sprag(&sock, &["panes"]).stdout.lines().count(),
        2,
        "and it really divided the window",
    );
}

/// `resize-pane -x -y` reaches the pane's PTY: the daemon reports the new geometry back through
/// the same `panes` slot the listing reads.
///
/// Both dimensions are required, and a zero is refused — the two argument rules that keep this
/// verb from sending a size no terminal can hold.
#[test]
fn the_cli_resizes_a_pane_and_the_daemon_reports_the_new_size() {
    let (_host, sock) = spawn_host();

    let resized = sprag(&sock, &["resize-pane", "0", "-x", "100", "-y", "30"]);
    assert!(resized.ok, "resize-pane succeeded: {}", resized.stderr);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("100x30"),
        "the daemon reports the new size: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // One dimension is not enough: there is no honest "the other one, unchanged" to send.
    let half = sprag(&sock, &["resize-pane", "0", "-x", "80"]);
    assert!(!half.ok, "one dimension is refused");
    assert!(
        half.stderr.contains("both dimensions"),
        "and says so: {}",
        half.stderr,
    );

    // A zero column count is not a resize; it is rejected locally.
    let zero = sprag(&sock, &["resize-pane", "0", "-x", "0", "-y", "30"]);
    assert!(!zero.ok, "a zero dimension is refused");
    assert!(
        zero.stderr.contains("positive"),
        "with a clear reason: {}",
        zero.stderr,
    );

    // An absent pane names what IS there rather than failing obscurely.
    let ghost = sprag(&sock, &["resize-pane", "9999", "-x", "80", "-y", "24"]);
    assert!(
        !ghost.ok && ghost.stderr.contains("9999"),
        "an absent pane fails, naming it: {}",
        ghost.stderr,
    );
}

/// `resize-window` PINS a window's size, `-u` un-pins it, and the size takes effect only while
/// `window-size` is `manual` — with the gap REPORTED rather than left to be discovered.
///
/// The daemon here has no attached client, which is what makes the first claim discriminating: every
/// other policy answers "no window" in that state, so a pane that moves at all proves the pin is a
/// rule the daemon performs. The `largest` half then proves the other direction — the size is stored
/// but inert — and that a user is TOLD, which is the whole reason storing an inert value is allowed.
#[test]
fn the_cli_pins_a_window_size_and_reports_when_the_policy_ignores_it() {
    // The DAEMON reads `window-size` from the user's file itself — no option crosses the wire — so
    // the config home has to reach the daemon as well as the CLI. Giving it to the CLI alone is a
    // real mistake this gate made first: the verb stored the pin and printed no note (its own view of
    // the file said `manual`) while the daemon went on arbitrating under the default.
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let (_host, sock) = spawn_host_env(&env);

    let pinned = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "111", "-y", "33"],
        &env,
    );
    assert!(pinned.ok, "resize-window succeeded: {}", pinned.stderr);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("111x33"),
        "the pin reached the pane with nobody attached: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );
    assert!(
        pinned.stderr.is_empty(),
        "a pin the policy USES needs no note: {}",
        pinned.stderr,
    );

    // A pane forced elsewhere is pulled back by the next action boundary — the pin is the authority
    // for as long as it stands, not a one-shot write.
    sprag(&sock, &["resize-pane", "0", "-x", "40", "-y", "10"]);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("111x33"),
        "a pinned window re-derives over a direct pane resize: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // -u hands it back: with nothing pinned and no client reporting, there is no window, so the
    // panes hold where they are rather than reflowing to a number nobody chose.
    let freed = sprag_env(&sock, &["resize-window", "-t", "0", "-u"], &env);
    assert!(freed.ok, "-u succeeded: {}", freed.stderr);
    sprag(&sock, &["resize-pane", "0", "-x", "40", "-y", "10"]);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("40x10"),
        "an un-pinned window stops overriding: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // The same verb under a policy that does not read the pin: it STORES and says so. The policy is
    // flipped through the real `set-option`, which edits the one file both processes read — so this
    // also shows the daemon picking the change up with nothing restarted.
    let flipped = sprag_env(&sock, &["set-option", "window-size", "largest"], &env);
    assert!(flipped.ok, "set-option succeeded: {}", flipped.stderr);
    let noted = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "100", "-y", "30"],
        &env,
    );
    assert!(
        noted.ok,
        "a pin is stored whatever the policy: {}",
        noted.stderr
    );
    assert!(
        noted.stderr.contains("largest") && noted.stderr.contains("manual"),
        "the note names the policy in force AND the way out: {:?}",
        noted.stderr,
    );
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("40x10"),
        "and it stayed inert: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // ⚠⚠⚠⚠⚠ AND `doctor` MUST BE QUIET ABOUT AN INERT PIN — register item 482, and THE arm its
    // first draft got wrong. A size is stored on this window right now and the policy does not read
    // it, so the window still follows its clients — a report keyed on the pin alone would tell a
    // person their terminal is ignoring them while it is doing exactly what they asked. **That is
    // worse than silence: it is a surface actively saying the wrong thing.**
    //
    // ⚠⚠⚠ THIS ASSERTION IS WHY THE GATE IS HERE AND NOT ONLY BELOW. The arms after the policy goes
    // back to `manual` pass with the conjunction removed — measured: deleting the policy half left
    // them green — because there the pin and the policy agree. Only this state tells them apart.
    let inert = sprag_env(&sock, &["doctor"], &env);
    assert!(inert.ok, "doctor answers: {}", inert.stderr);
    assert!(
        !inert.stdout.contains("FOLLOW NO CLIENT"),
        "⚠⚠⚠⚠⚠ A STORED PIN IS NOT A WINDOW THAT FOLLOWS NOBODY. The product says so itself when \
         the pin is made — *stored, but window-size is largest so the panes still follow the \
         clients* — and a report that disagrees with the verb that produced the state is the one a \
         reader will believe: {:?}",
        inert.stdout,
    );
    assert!(
        inert.stdout.contains("no window is pinned"),
        "⚠⚠ and it still says it LOOKED, rather than falling silent and reading as unchecked: {:?}",
        inert.stdout,
    );

    // ...and the SAME stored size becomes live the moment the policy names it. Nothing is re-sent.
    let back = sprag_env(&sock, &["set-option", "window-size", "manual"], &env);
    assert!(back.ok, "set-option succeeded: {}", back.stderr);
    sprag_env(&sock, &["resize-pane", "0", "-x", "50", "-y", "12"], &env);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("100x30"),
        "a size stored under `largest` is what `manual` then uses: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // ⚠⚠⚠⚠⚠ AND `doctor` SAYS WHICH WINDOWS FOLLOW NOBODY — register item 482, driven here rather
    // than in a unit because the fact is a CONJUNCTION of two authorities: a size pinned on a
    // window, and a policy that reads pins. Only a real daemon holds both.
    //
    // ⚠⚠⚠⚠ THE FIRST VERSION OF THIS REPORT WAS BUILT ON THE PIN ALONE, and driving it is what
    // refuted it — under `largest` above the same pin is INERT and the product says so itself
    // (*"stored, but window-size is largest so the panes still follow the clients"*), so a report
    // keyed on the pin tells a person their terminal is ignoring them while it is doing exactly
    // what they asked. **A surface that says the wrong thing is worse than one that says nothing.**
    //
    // The policy is `manual` and the pin is in force at this point, which is the arm a reader acts
    // on; the inert arm is driven immediately after by handing the window back.
    let named = sprag_env(&sock, &["doctor"], &env);
    assert!(
        named.ok,
        "doctor answers on a live daemon: {}",
        named.stderr
    );
    assert!(
        named.stdout.contains("FOLLOW NO CLIENT") && named.stdout.contains("resize-window -u"),
        "⚠⚠⚠⚠⚠ A WINDOW THAT FOLLOWS NOBODY MUST BE NAMED WITH ITS REMEDY. Until this, the window \
         simply stopped following and no surface anywhere said why — the owner read a working \
         product as a broken one and asked whether the code had been hardcoded, which was a \
         reasonable reading of total silence: {:?}",
        named.stdout,
    );

    // ── AND THE INERT ARM IS SILENT, which is the half the first draft got wrong ──
    let freed = sprag_env(&sock, &["resize-window", "-t", "0", "-u"], &env);
    assert!(freed.ok, "-u succeeded: {}", freed.stderr);
    let quiet = sprag_env(&sock, &["doctor"], &env);
    assert!(
        quiet.stdout.contains("no window is pinned"),
        "⚠⚠⚠ AND IT MUST STILL SAY THAT IT LOOKED. Silence here reads as *nobody checked* to the \
         one reader who consults this — somebody whose terminal is already behaving oddly: {:?}",
        quiet.stdout,
    );
    assert!(
        !quiet.stdout.contains("FOLLOW NO CLIENT"),
        "⚠⚠⚠⚠ and a window that was handed back must stop being reported, or the sentence becomes \
         one a reader learns to skip: {:?}",
        quiet.stdout,
    );
}

/// **THE NOTE IS THE DAEMON'S FACT, not this process's reading of a file** (R331) — driven by
/// giving the two processes DIFFERENT config homes, which is the only fixture in which the two
/// authorities can be told apart at all.
///
/// The verb printed that note by calling `sprag_host::config::window_size()` in the CLI's own
/// process, with a comment saying *"the daemon was never asked what it thinks the policy is"*. Both
/// directions were wrong and only one of them is loud:
///
/// * the daemon on `largest` and this process's file on `manual` — the pin is INERT and the user is
///   told NOTHING, which is the "I resized and nothing moved" discovery the note exists to prevent;
/// * the daemon on `manual` and this process's file on anything else — the pin is IN FORCE and the
///   user is told it is not, about a resize they can see on their own screen.
///
/// Both are asserted here, in that order, because the first is the one that fails silently and the
/// second is the one a fix could reintroduce by over-reporting. The PANES are read between them: a
/// note is a claim about whether the panes moved, so a gate that checked only the words could pass
/// while the sentence was about the wrong window.
#[test]
fn the_policy_note_comes_from_the_daemon_and_not_from_the_callers_own_config() {
    // THREE homes, because an option verb edits the file the process running it resolves: the
    // daemon's own (which `set-option` below is pointed at deliberately), and one per DIRECTION for
    // the caller. A gate that shared a home between the two processes could not fail.
    let served = ConfigHome::new("[options]\nwindow-size = \"largest\"\n");
    let quiet = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let loud = ConfigHome::new("[options]\nwindow-size = \"largest\"\n");
    let daemon = [("XDG_CONFIG_HOME", served.as_str())];
    let (_host, sock) = spawn_host_env(&daemon);

    let before = sprag(&sock, &["panes"]).stdout;
    let pinned = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "77", "-y", "21"],
        &[("XDG_CONFIG_HOME", quiet.as_str())],
    );
    assert!(
        pinned.ok,
        "the pin is stored whatever the policy: {}",
        pinned.stderr
    );
    assert_eq!(
        sprag(&sock, &["panes"]).stdout,
        before,
        "the fixture's whole point: under `largest` with nobody attached the pin moves nothing",
    );
    assert!(
        pinned.stderr.contains("largest"),
        "a pin that did nothing must say so, naming the policy the DAEMON is under — this \
         process's own file says `manual` and would have said nothing at all: {:?}",
        pinned.stderr,
    );

    // ...and the other direction, which is the one an over-eager note would get wrong: the DAEMON's
    // file moves to `manual`, so the same pin is now IN FORCE and the panes say so, while the caller
    // reads a file that still says `largest`. A note here would be telling a user their resize did
    // nothing while they watch it happen.
    let flipped = sprag_env(&sock, &["set-option", "window-size", "manual"], &daemon);
    assert!(flipped.ok, "set-option succeeded: {}", flipped.stderr);
    let in_force = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "78", "-y", "22"],
        &[("XDG_CONFIG_HOME", loud.as_str())],
    );
    assert!(in_force.ok, "the pin succeeded: {}", in_force.stderr);
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("78x22"),
        "the pin is in force at the daemon: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );
    assert!(
        in_force.stderr.is_empty(),
        "a pin the daemon USES needs no note, whatever this process's own file says: {:?}",
        in_force.stderr,
    );
}

/// A RELATIVE resize reads the window's current size on the DAEMON side and answers the rectangle it
/// produced — the one form a CLI could almost have computed for itself, and deliberately does not.
///
/// The direction convention is what a reader gets wrong, so it is asserted rather than described:
/// each flag names an EDGE and pushes it, so `-R` widens and `-U` SHORTENS. A gate written to "up
/// means taller" fails here.
#[test]
fn the_cli_resizes_a_window_relative_to_the_size_it_has() {
    let config = ConfigHome::new("[options]\nwindow-size = \"manual\"\n");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let (_host, sock) = spawn_host_env(&env);

    sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-x", "100", "-y", "30"],
        &env,
    );
    let moved = sprag_env(
        &sock,
        &["resize-window", "-t", "0", "-R", "20", "-U", "5"],
        &env,
    );
    assert!(moved.ok, "{}", moved.stderr);
    assert!(
        moved.stdout.contains("120x25"),
        "the daemon answered the rectangle it worked out: {:?}",
        moved.stdout,
    );
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("120x25"),
        "and the panes took it: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // The clamp: past the bottom is the smallest window, never a wrapped one.
    let floored = sprag_env(&sock, &["resize-window", "-t", "0", "-L", "9999"], &env);
    assert!(
        floored.ok && floored.stdout.contains("1x25"),
        "a huge narrowing saturates: {:?} {}",
        floored.stdout,
        floored.stderr,
    );
}

/// `resize-window`'s argument rules: both dimensions or neither, no zero, the five spellings mutually
/// exclusive, opposing edges on one axis refused, a relative count required, an unresolvable fold
/// refused, and an unknown window named. Each refusal happens with nothing written.
#[test]
fn the_cli_refuses_a_half_window_a_zero_and_a_contradiction() {
    let (_host, sock) = spawn_host();

    let half = sprag(&sock, &["resize-window", "-t", "0", "-x", "80"]);
    assert!(
        !half.ok && half.stderr.contains("both dimensions"),
        "{}",
        half.stderr
    );

    let zero = sprag(&sock, &["resize-window", "-t", "0", "-x", "0", "-y", "30"]);
    assert!(
        !zero.ok && zero.stderr.contains("positive"),
        "{}",
        zero.stderr
    );

    let ghost = sprag(
        &sock,
        &["resize-window", "-t", "0", "ghost", "-x", "80", "-y", "24"],
    );
    assert!(
        !ghost.ok && ghost.stderr.contains("ghost"),
        "{}",
        ghost.stderr
    );

    // The five spellings name ONE rectangle, so two of them is a caller who has not decided. Every
    // pair is refused rather than ordered by a precedence rule the user never stated.
    for pair in [
        vec!["-x", "80", "-y", "24", "-A"],
        vec!["-x", "80", "-y", "24", "-u"],
        vec!["-A", "-R", "4"],
        vec!["-u", "-R", "4"],
    ] {
        let mut args = vec!["resize-window", "-t", "0"];
        args.extend(pair.iter().copied());
        let mixed = sprag(&sock, &args);
        assert!(
            !mixed.ok && mixed.stderr.contains("five ways to name one size"),
            "{pair:?}: {}",
            mixed.stderr,
        );
    }

    // `-a -A` is the one contradiction the mode count CANNOT see, because both spellings land in one
    // slot — and this assertion is why it is caught at all: it first failed with the daemon's
    // no-basis refusal, which is the shape of `-A` having silently won.
    let folds = sprag(&sock, &["resize-window", "-t", "0", "-a", "-A"]);
    assert!(
        !folds.ok && folds.stderr.contains("opposite folds"),
        "{}",
        folds.stderr,
    );

    // Both directions on ONE axis is a contradiction, not a net movement: `-L 5 -R 3` could only be
    // read as "2 narrower" by arithmetic nobody asked for.
    for axis in [vec!["-L", "5", "-R", "3"], vec!["-U", "2", "-D", "9"]] {
        let mut args = vec!["resize-window", "-t", "0"];
        args.extend(axis.iter().copied());
        let opposed = sprag(&sock, &args);
        assert!(
            !opposed.ok && opposed.stderr.contains("opposite ways"),
            "{axis:?}: {}",
            opposed.stderr,
        );
    }

    // A call naming NOTHING is refused rather than read as `-u`: an empty command line has stated no
    // intent, and treating it as an un-pin would throw a decision away.
    let silent = sprag(&sock, &["resize-window", "-t", "0"]);
    assert!(
        !silent.ok && silent.stderr.contains("needs a size"),
        "{}",
        silent.stderr,
    );

    // A relative flag's count is REQUIRED and positive — the direction carries the sign, so there is
    // no negative adjustment to spell and no default to guess (see the verb's docs on why sprag
    // cannot copy tmux's bare `-U`).
    let bare = sprag(&sock, &["resize-window", "-t", "0", "-R"]);
    assert!(
        !bare.ok && bare.stderr.contains("needs a column count"),
        "{}",
        bare.stderr
    );
    let nought = sprag(&sock, &["resize-window", "-t", "0", "-D", "0"]);
    assert!(
        !nought.ok && nought.stderr.contains("positive"),
        "{}",
        nought.stderr
    );

    // `-A` with no client that has reported an area names no rectangle. The refusal must NOT be read
    // as "no size", which would un-pin the window the user was trying to resize.
    //
    // ⚠ **AND IT MUST NOT MENTION THE PIN** (R331) — proved at the END of this test, where a
    // rectangle can be stored without breaking the nothing-was-written claim below.
    let unfoldable = sprag(&sock, &["resize-window", "-t", "0", "-A"]);
    assert!(
        !unfoldable.ok && unfoldable.stderr.contains("-a/-A"),
        "the refusal names the flags that needed a client: {}",
        unfoldable.stderr,
    );

    // A window command's -t is required, so a missing one is refused before any connection.
    let unscoped = sprag(&sock, &["resize-window", "-x", "80", "-y", "24"]);
    assert!(
        !unscoped.ok && unscoped.stderr.contains("-t SESSION"),
        "{}",
        unscoped.stderr,
    );

    // Nothing above wrote anything: the boot pane is untouched.
    assert!(
        sprag(&sock, &["panes"]).stdout.contains("80x24"),
        "a refused resize-window pins nothing: {:?}",
        sprag(&sock, &["panes"]).stdout,
    );

    // ⚠ **THE FOLD'S REFUSAL WITH A PIN IN PLACE** (R331), last because it is the one call here
    // that WRITES. This refusal used to carry one sentence for two causes — *"nothing is pinned and
    // no client has reported an area"* — and with a rectangle stored its first clause is MEASURABLY
    // FALSE. Driven at the shipped daemon before the fix, that is exactly what an operator who had
    // just typed `-x 100 -y 30` was told about their own window.
    //
    // The pin is the FIXTURE and the discriminator both: without it the old sentence and the new one
    // are equally true here, and this assertion would pass on the defect.
    let stored = sprag(
        &sock,
        &["resize-window", "-t", "0", "-x", "100", "-y", "30"],
    );
    assert!(stored.ok, "the fixture pins a rectangle: {}", stored.stderr);
    let folded = sprag(&sock, &["resize-window", "-t", "0", "-a"]);
    assert!(
        !folded.ok,
        "a fold of no clients was accepted: {}",
        folded.stdout
    );
    assert!(
        !folded.stderr.contains("pinned"),
        "a fold's refusal claimed something about a pin it never read, and this session HAS one: {}",
        folded.stderr,
    );
}

/// The strongest chain the CLI can prove without a display: `send-keys` reaches the pane's CHILD
/// through the real PTY, and `capture-pane` reads back what the child did with it.
///
/// The fixture is `cat`, which makes the two languages distinguishable. Literal text is ECHOED by
/// the terminal once, and stays one copy while the line is unfinished; the `Enter` KEY completes
/// `cat`'s line-buffered read, so it writes the line back and a SECOND copy appears. One assertion
/// therefore separates "the text arrived" from "the keystroke arrived", which a single combined
/// send could not. (The same mechanism `sprag run`'s test relies on, read in the other direction.)
/// ⚠⚠⚠ **BOTH MOUTHS OFFER THE SAME CHOICE, FROM THE SAME VOCABULARY.**
///
/// `capture-pane`'s doc promised the shell and the `read_pane` tool see *"one definition of what a
/// pane's output IS rather than two"* — and that promise went false the moment the tool grew a
/// `line_breaks` argument the shell did not have. A grammar change is every mouth, and the one that
/// gets left behind is the one nobody is holding a test on.
///
/// Five columns, so `TOOL UP` breaks at its SPACE: `screen` reports where the terminal broke it and
/// `program` reports the line the child wrote. Both in one test, because either alone is a claim
/// about a reading rather than about the choice.
#[test]
fn capture_pane_offers_the_screens_line_breaks_or_the_programs() {
    let (_host, sock) = spawn_host_sized(5, 4, &["sh", "-c", "printf 'TOOL UP\\n'; exec cat"]);
    wait_for_pane_text(&sock, "TOOL");

    let rendered = sprag(&sock, &["capture-pane", "0"]);
    assert!(rendered.ok, "capture-pane succeeded: {}", rendered.stderr);
    assert!(
        !rendered.stdout.contains("TOOL UP"),
        "THE FIXTURE CHECK AND THE CONTROL: five columns really broke the line, and the default \
         still describes the screen: {:?}",
        rendered.stdout,
    );

    let written = sprag(&sock, &["capture-pane", "0", "--line-breaks", "program"]);
    assert!(written.ok, "--line-breaks program: {}", written.stderr);
    assert!(
        written.stdout.contains("TOOL UP"),
        "⚠⚠ THE LINE THE CHILD WROTE, at the shell mouth too — a script piping this into a \
         matcher was matching against the width of whoever attached a client: {:?}",
        written.stdout,
    );
    assert_eq!(
        sprag(&sock, &["capture-pane", "0", "--line-breaks", "screen"]).stdout,
        rendered.stdout,
        "and naming the default explicitly is the default — the arm nothing else drives",
    );

    let refused = sprag(&sock, &["capture-pane", "0", "--line-breaks", "sideways"]);
    assert!(
        !refused.ok && refused.stderr.contains("sideways"),
        "a word the vocabulary does not publish is refused NAMING what was sent: {:?}",
        refused.stderr,
    );
    let bare = sprag(&sock, &["capture-pane", "0", "--line-breaks"]);
    assert!(
        !bare.ok && bare.stderr.contains("needs a value"),
        "and the flag with nothing after it is refused rather than silently defaulting: {:?}",
        bare.stderr,
    );
}

#[test]
fn send_keys_reaches_the_child_and_capture_pane_reads_it_back() {
    let (_host, sock) = spawn_host();

    // -l: literal text, typed, no Enter appended.
    let typed = sprag(&sock, &["send-keys", "0", "-l", "marker-one"]);
    assert!(typed.ok, "send-keys -l succeeded: {}", typed.stderr);
    let echoed = wait_for_pane_text(&sock, "marker-one");
    assert_eq!(
        echoed.matches("marker-one").count(),
        1,
        "the terminal echoed it once and `cat` has not seen a line yet: {echoed:?}",
    );

    // capture-pane reads the same text the daemon holds, through the CLI.
    let captured = sprag(&sock, &["capture-pane", "0"]);
    assert!(captured.ok, "capture-pane succeeded: {}", captured.stderr);
    assert!(
        captured.stdout.contains("marker-one"),
        "capture-pane prints the pane's output: {:?}",
        captured.stdout,
    );
    // tmux's `-p` says "to stdout", which is the only thing this can do — same output.
    assert_eq!(
        sprag(&sock, &["capture-pane", "0", "-p"]).stdout,
        captured.stdout,
        "-p is accepted and means what it says",
    );

    // The KEY language: Enter finishes the line, so `cat` writes it back and a second copy lands.
    let entered = sprag(&sock, &["send-keys", "0", "Enter"]);
    assert!(entered.ok, "send-keys Enter succeeded: {}", entered.stderr);
    let doubled = wait_for(Duration::from_secs(5), || {
        sprag(&sock, &["capture-pane", "0"])
            .stdout
            .matches("marker-one")
            .count()
            == 2
    });
    assert!(
        doubled,
        "the Enter reached the child, which echoed the line back: {:?}",
        sprag(&sock, &["capture-pane", "0"]).stdout,
    );

    // A key name the encoder does not know is a clean refusal naming the vocabulary — never a
    // keystroke that silently vanished, which is the one outcome a script cannot detect.
    let unknown = sprag(&sock, &["send-keys", "0", "NotAKey"]);
    assert!(!unknown.ok, "an unknown key name fails");
    assert!(
        unknown.stderr.contains("W3C key name"),
        "and names the vocabulary: {}",
        unknown.stderr,
    );
    // ...and the sentence is the ENCODER'S, not this client's guess at what the encoder knows.
    // `send_key` answered one `bool` for "unencodable" and "the child is gone" until R325, so the
    // CLI wrote both possibilities itself; now the end that owns the vocabulary states it.
    assert!(
        !unknown.stderr.contains("PTY refused"),
        "a key the encoder does not know is not reported as a dead child: {}",
        unknown.stderr,
    );

    // Both pane-addressed verbs pre-flight the id, so a wrong one is about PANES, not addresses.
    for args in [
        vec!["send-keys", "9999", "Enter"],
        vec!["capture-pane", "9999"],
    ] {
        let ghost = sprag(&sock, &args);
        assert!(!ghost.ok, "{args:?} fails on an absent pane");
        assert!(
            ghost.stderr.contains("no pane 9999"),
            "naming it: {}",
            ghost.stderr,
        );
    }
}

/// A temporary `$XDG_CONFIG_HOME` holding `text` as the user config, cleaned up on drop —
/// including on a panicked assertion, so a failed run leaks no directory.
struct ConfigHome(PathBuf);
impl Drop for ConfigHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl ConfigHome {
    /// Unique per CALL, like [`socket_path`]: these tests run in parallel threads of one binary and
    /// a shared directory would have them reading each other's config.
    fn new(text: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sprag-cli-cfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join("sprag")).expect("temp config dir");
        std::fs::write(dir.join("sprag").join("config.toml"), text).expect("write config");
        Self(dir)
    }

    fn as_str(&self) -> &str {
        self.0.to_str().expect("a utf-8 temp path")
    }
}

/// `list-keys` answers from the user's config with **NO DAEMON RUNNING**, and a file's declarations
/// reach the table a client would use.
///
/// The no-daemon half is the point of the verb, not a convenience: a keybinding is what a CLIENT
/// does with a keyboard, so it lives in the config file rather than in the server — which is why
/// this test names a socket that was never bound and still expects success. tmux's `list-keys`
/// starts a server to answer the same question.
///
/// REVERT-PROOF for the no-daemon claim: route this through `connect()` like every other verb and it
/// fails with "no server running", i.e. a user could not read their own keymap while editing it.
#[test]
fn list_keys_reads_the_users_config_with_no_daemon() {
    let config = ConfigHome::new(
        "[options]\nprefix = \"C-a\"\n\n\
         [[bind]]\nkey = \"|\"\naction = \"split-window -h\"\n\n\
         [[unbind]]\nkey = \"%\"\n",
    );
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let run = sprag_env(
        &absent,
        &["list-keys"],
        &[("XDG_CONFIG_HOME", config.as_str())],
    );
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines.first().copied(), Some("prefix C-a"));
    let binds: Vec<String> = lines[1..]
        .iter()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    assert!(
        binds.contains(&"bind-key -T prefix | split-window -h".to_owned()),
        "the declared bind is there: {binds:?}",
    );
    assert!(
        !binds.iter().any(|line| line.contains(" % ")),
        "the unbound default is gone: {binds:?}",
    );
    assert!(
        binds.contains(&"bind-key -T prefix d detach-client".to_owned()),
        "and every default the file did not mention survives: {binds:?}",
    );
    // The self-send followed the prefix, so `prefix prefix` still types it.
    assert!(
        binds.contains(&"bind-key -T prefix C-a send-prefix".to_owned()),
        "the self-send follows the prefix: {binds:?}",
    );
    // THE FOUR DIRECTIONAL DEFAULTS, INCLUDING THE `-r` COLUMN — the rows R297 shipped and read by
    // hand, which left nothing standing: this test mentioned `ArrowUp` and `select-pane -U` zero
    // times, and the keymap's own unit test asserts a string IT formats rather than this rendering.
    // R299 then rewrote the flag table underneath both, which is the round that owed the assertion.
    for (key, flag) in [
        ("ArrowUp", "-U"),
        ("ArrowDown", "-D"),
        ("ArrowLeft", "-L"),
        ("ArrowRight", "-R"),
    ] {
        assert!(
            binds.contains(&format!("bind-key -r -T prefix {key} select-pane {flag}")),
            "tmux repeats exactly these four, and the -r is part of the row: {binds:?}",
        );
    }
}

/// A broken config is a clean refusal that names the file and what is wrong — never a silently
/// default table, which would leave a user believing their config was accepted.
#[test]
fn list_keys_refuses_a_broken_config_and_names_it() {
    // A REAL VERB a keystroke cannot mean — the likeliest broken line in anybody's config,
    // because it is a word they read in `sprag --help`. Since R323 the refusal says which RULE
    // stops it rather than claiming the verb does not exist.
    let config = ConfigHome::new("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n");
    let run = sprag_env(
        &socket_path(),
        &["list-keys"],
        &[("XDG_CONFIG_HOME", config.as_str())],
    );
    assert!(!run.ok, "a broken config fails");
    assert!(
        run.stderr.contains("config.toml") && run.stderr.contains("is a command, not a binding"),
        "naming the file and the fault: {}",
        run.stderr,
    );
    // AND A WORD THAT IS NO VERB AT ALL, which must still read as a typo — the control that keeps
    // the sentence above from being the only thing this binary can say about a bad action.
    let typo = ConfigHome::new("[[bind]]\nkey = \"x\"\naction = \"kill-serverr\"\n");
    let run = sprag_env(
        &socket_path(),
        &["list-keys"],
        &[("XDG_CONFIG_HOME", typo.as_str())],
    );
    assert!(!run.ok, "a broken config fails");
    assert!(
        run.stderr.contains("config.toml") && run.stderr.contains("is not an action"),
        "naming the file and the fault: {}",
        run.stderr,
    );
}

/// The text `config` currently holds, for the editing verbs' tests.
fn config_text(config: &ConfigHome) -> String {
    std::fs::read_to_string(std::path::Path::new(config.as_str()).join("sprag/config.toml"))
        .expect("the config file")
}

/// **`bind-key` and `unbind-key` need NO DAEMON either, and they WRITE the file.**
///
/// The write is the whole of slice 2's design and the one thing tmux's `bind-key` does not do:
/// tmux's config is an imperative script a runtime fact cannot be written back into, so its binds
/// are transient and the user has to remember to record them. sprag's is declarative TOML, so the
/// file simply IS the live table — which it also has to be, because `list-keys` reads that file
/// with no server and a binding living anywhere else would make it print a table nobody uses.
///
/// Asserted through `list-keys` rather than by reading the file, because the claim is about what a
/// CLIENT would do, and `list-keys` is the same reader a client uses.
///
/// REVERT-PROOF for the no-daemon claim: route either verb through `connect()` and it fails against
/// this never-bound socket — a user could not bind a key while no session was running, which is
/// exactly when they are setting one up.
#[test]
fn bind_key_writes_the_users_config_with_no_daemon() {
    let config = ConfigHome::new("# mine\n[[command]]\nname = \"top\"\nrun = [\"htop\"]\n");
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    // tmux's own unquoted spelling: the action is the rest of the line.
    let run = sprag_env(&absent, &["bind-key", "c", "split-window", "-h"], &env);
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "stdout stays clean for a script: {:?}",
        run.stdout
    );

    let run = sprag_env(&absent, &["unbind-key", "o"], &env);
    assert!(run.ok, "{}", run.stderr);

    let listed = sprag_env(&absent, &["list-keys"], &env);
    assert!(listed.ok, "{}", listed.stderr);
    let binds: Vec<String> = listed
        .stdout
        .lines()
        .skip(1)
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect();
    assert!(
        binds.contains(&"bind-key -T prefix c split-window -h".to_owned()),
        "the bound key is in the table a client would read: {binds:?}",
    );
    assert!(
        !binds.iter().any(|line| line.contains(" o ")),
        "and the unbound default is gone: {binds:?}",
    );
    // The user's own file is still theirs.
    let text = config_text(&config);
    assert!(text.contains("# mine"), "the comment survived: {text:?}");
    assert!(text.contains("[[command]]"), "and the commands: {text:?}");
}

/// A key or action the USER TYPED is reported as an argument, naming NO file — while a broken FILE
/// still names `config.toml`. Both messages exist to send someone to the right place, and a
/// `config.toml:` prefix on a command-line typo sends them to a file that is fine.
///
/// REVERT-PROOF: parse the argument inside `config::bind_key` and render the failure through
/// `ConfigError`, and the first assertion's `config.toml` check fires.
#[test]
fn a_mistyped_argument_names_no_file_and_a_broken_file_still_does() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let run = sprag_env(&socket_path(), &["bind-key", "Up", "detach-client"], &env);
    assert!(!run.ok, "a key nothing can produce is refused");
    assert!(
        run.stderr.contains("is not a key") && !run.stderr.contains("config.toml"),
        "the argument is the fault, not the file: {}",
        run.stderr,
    );
    assert_eq!(config_text(&config), "", "and nothing was written");

    let broken = ConfigHome::new("[[bind]]\nkey = \"x\"\naction = \"kill-server\"\n");
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "c", "detach-client"],
        &[("XDG_CONFIG_HOME", broken.as_str())],
    );
    assert!(!run.ok, "an unusable file is refused");
    assert!(
        run.stderr.contains("config.toml"),
        "the file IS the fault here: {}",
        run.stderr,
    );
}

/// Both of sprag's key tables are accepted under tmux's own spellings, and a THIRD is refused by
/// name.
///
/// `-n` and `-T root` have to reach the same place, because tmux's manual defines the first as an
/// alias for the second — a user whose fingers produce one and whose config was written with the
/// other must not end up with two bindings.
///
/// Refusing an unknown table by NAME rather than defaulting it is what keeps `switch-client -T`'s
/// custom tables open: a silent fallback to the prefix table would put a binding somewhere the user
/// did not ask for and print it back to them as though they had.
#[test]
fn both_key_tables_are_accepted_and_a_third_is_refused() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-T", "prefix", "c", "detach-client"],
        &env,
    );
    assert!(run.ok, "tmux's own spelling works: {}", run.stderr);

    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-n", "F5", "detach-client"],
        &env,
    );
    assert!(run.ok, "-n binds in the root table: {}", run.stderr);
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-T", "root", "F6", "detach-client"],
        &env,
    );
    assert!(run.ok, "and so does its long form: {}", run.stderr);

    let listed = sprag_env(&socket_path(), &["list-keys"], &env);
    let root_lines: Vec<&str> = listed
        .stdout
        .lines()
        .filter(|line| line.contains("-T root"))
        .collect();
    // The two just bound, PLUS the two session chords the root table ships with (R314) — asserted
    // as the SET rather than as a count, so the claim survives a default being added and still
    // fails if one spelling produced two lines.
    let bound_here: Vec<&&str> = root_lines
        .iter()
        .filter(|line| line.contains("F5") || line.contains("F6"))
        .collect();
    assert_eq!(
        bound_here.len(),
        2,
        "one line each, not one per spelling:\n{}",
        listed.stdout
    );
    assert_eq!(
        root_lines.len(),
        4,
        "and nothing else appeared in root beyond the two shipped chords:\n{}",
        listed.stdout
    );

    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-T", "copy-mode", "c", "detach-client"],
        &env,
    );
    assert!(!run.ok, "sprag has two tables, not three");
    assert!(
        run.stderr.contains("copy-mode")
            && run.stderr.contains("root")
            && run.stderr.contains("prefix"),
        "naming what was asked for and what exists: {}",
        run.stderr,
    );
}

/// `list-keys` lines its columns up ACROSS the two tables, so the actions read as one column.
///
/// Pinned because the defect it caught is invisible to every other assertion here and was found by
/// reading the output: `KeyTable`'s `Display` wrote its string directly instead of going through
/// `Formatter::pad`, so the `{:width$}` the printer asks for was silently ignored and every `root`
/// line put its key two characters to the left. A manual `Display` that does not call `pad` honours
/// no formatting flag it is given.
#[test]
fn list_keys_lines_up_its_columns_across_both_tables() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    assert!(
        sprag_env(
            &socket_path(),
            &["bind-key", "-n", "F5", "detach-client"],
            &env
        )
        .ok
    );
    let listed = sprag_env(&socket_path(), &["list-keys"], &env).stdout;
    let widest = listed
        .lines()
        .filter(|line| line.starts_with("bind-key"))
        .filter_map(|line| line.find(" -T ").map(|at| &line[at + 4..]))
        .filter_map(|rest| rest.find(' ').map(|at| &rest[..at]))
        .map(str::len)
        .max()
        .expect("some bindings are listed");
    assert!(
        listed
            .lines()
            .filter(|line| line.contains("-T root"))
            .all(|line| line.contains(&format!("-T {:widest$} ", "root"))),
        "the shorter table name is padded to the longer one:\n{listed}",
    );
}

/// `-n` and `-T prefix` together are REFUSED rather than resolved.
///
/// They are two contradictory statements about one binding — tmux documents `-n` AS `-T root` — and
/// honouring either would be inventing a precedence rule a user cannot see in their own command
/// line. The same reasoning slice 1 applied to a key that is both bound and unbound.
#[test]
fn the_two_spellings_of_the_root_table_may_not_contradict() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-n", "-T", "prefix", "c", "detach-client"],
        &env,
    );
    assert!(!run.ok, "-n and -T prefix cannot both be true");
    assert!(
        run.stderr.contains("-n") && run.stderr.contains("root"),
        "the message says why: {}",
        run.stderr,
    );
}

/// `-r` is `bind-key`'s and REFUSED in the root table, where it could not mean anything.
///
/// Measured against `tmux 3.2a`: it accepts `bind -n -r` and stores it, and the binding never
/// repeats — because repeat holds the PREFIX table open and a root binding is reached without the
/// prefix. sprag refuses it instead, the same divergence it already takes for a bare `split-window`
/// in a binding: a declaration with no effect is what this whole surface exists to prevent.
#[test]
fn repeat_is_refused_where_it_could_not_act() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-r", "o", "select-pane -t :.+"],
        &env,
    );
    assert!(
        run.ok,
        "-r in the prefix table is the whole point: {}",
        run.stderr
    );
    let listed = sprag_env(&socket_path(), &["list-keys"], &env);
    assert!(
        listed
            .stdout
            .lines()
            .any(|line| line.contains("-r") && line.contains("-T prefix") && line.contains(" o ")),
        "list-keys shows the flag back:\n{}",
        listed.stdout,
    );

    let run = sprag_env(
        &socket_path(),
        &["bind-key", "-n", "-r", "F5", "detach-client"],
        &env,
    );
    assert!(!run.ok, "a root binding has no prefix table to hold open");
    assert!(
        run.stderr.contains("repeat") && run.stderr.contains("prefix"),
        "the message names the mechanism: {}",
        run.stderr,
    );

    let run = sprag_env(&socket_path(), &["unbind-key", "-r", "o"], &env);
    assert!(!run.ok, "-r is not unbind-key's flag");
}

/// The two tables hold one key SEPARATELY, all the way through the file and back out of
/// `list-keys`.
///
/// This is the property that makes the table half of a binding's identity rather than a property of
/// one: binding `%` in the root table must not disturb the `%` sprag ships in the prefix table, and
/// unbinding one must not remove the other. An editor matching on the key alone would corrupt
/// exactly the config of a user who took the trouble to bind both.
#[test]
fn one_key_in_two_tables_is_two_bindings() {
    let config = ConfigHome::new("");
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    assert!(
        sprag_env(
            &socket_path(),
            &["bind-key", "-n", "%", "detach-client"],
            &env
        )
        .ok,
        "bound in the root table",
    );
    let listed = sprag_env(&socket_path(), &["list-keys"], &env).stdout;
    assert!(
        listed.contains("-T prefix") && listed.contains("-T root"),
        "both survive:\n{listed}",
    );
    assert!(
        listed.lines().any(|line| line.contains("-T prefix")
            && line.contains('%')
            && line.contains("split-window -h")),
        "the shipped default is untouched:\n{listed}",
    );

    assert!(
        sprag_env(&socket_path(), &["unbind-key", "-n", "%"], &env).ok,
        "unbound in the root table",
    );
    let listed = sprag_env(&socket_path(), &["list-keys"], &env).stdout;
    assert!(
        listed.lines().any(|line| line.contains("-T prefix")
            && line.contains('%')
            && line.contains("split-window -h")),
        "and the prefix table's % is STILL there:\n{listed}",
    );
    // Back to what the root table SHIPS with (R314: the two session chords), not to empty — and
    // asserted as exactly that rather than as an absence, so the unbind is still what is being
    // measured. `%` is what must be gone; the defaults must not be.
    assert!(
        !listed
            .lines()
            .any(|line| line.contains("-T root") && line.contains('%')),
        "while the root % is gone again:\n{listed}",
    );
    assert_eq!(
        listed
            .lines()
            .filter(|line| line.contains("-T root"))
            .count(),
        2,
        "and the two shipped session chords are untouched by any of it:\n{listed}",
    );
}

/// `show-options` answers from the user's config with **NO DAEMON RUNNING**, and prints EVERY option
/// — the one the file sets and the one it does not.
///
/// The no-daemon half is the point of the verb, for `list-keys`'s reason: every option here is what
/// one CLIENT does with one attachment, so it lives in the user's file rather than in the server.
///
/// Printing an option the file never mentions is the other half, and it is not padding: a user who
/// does not already know an option's name cannot discover it in a file that does not name it, which
/// is the whole reason this table exists rather than a struct. tmux answers the same way.
#[test]
fn show_options_prints_every_option_with_no_daemon() {
    let config = ConfigHome::new("[options]\ndetach-on-destroy = \"off\"\n");
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let run = sprag_env(
        &absent,
        &["show-options"],
        &[("XDG_CONFIG_HOME", config.as_str())],
    );
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert!(
        lines.contains(&"detach-on-destroy off"),
        "the option the file set: {lines:?}",
    );
    assert!(
        lines.contains(&"prefix C-b"),
        "and the one it did not, at its default: {lines:?}",
    );
    let mut sorted = lines.clone();
    sorted.sort_unstable();
    assert_eq!(lines, sorted, "sorted by name, like tmux's: {lines:?}");
}

/// `set-option` WRITES the user's config, needs no daemon, and canonicalises the value.
///
/// It writes for `bind-key`'s reason: the file IS the live table, so `show-options`, an attached
/// client and the next attach cannot give three different answers. Asserted THROUGH `show-options`
/// rather than by reading the file, because the claim is about what a client would read.
///
/// The canonical half is what keeps one setting one string: `^a` and `C-a` are one keystroke, so a
/// file that recorded the spelling it was handed would make `show-options` and `list-keys` disagree
/// about a prefix they both read from it.
#[test]
fn set_option_writes_the_users_config_with_no_daemon() {
    let config = ConfigHome::new("# mine\n[[command]]\nname = \"top\"\nrun = [\"htop\"]\n");
    let absent = socket_path();
    assert!(!absent.exists(), "the socket was never bound");
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let run = sprag_env(&absent, &["set-option", "prefix", "^a"], &env);
    assert!(run.ok, "no daemon is not an error: {}", run.stderr);
    assert!(
        run.stdout.is_empty(),
        "stdout stays clean for a script: {:?}",
        run.stdout
    );

    let listed = sprag_env(&absent, &["show-options"], &env);
    assert!(listed.ok, "{}", listed.stderr);
    assert!(
        listed.stdout.lines().any(|line| line == "prefix C-a"),
        "stored as the keymap spells it: {:?}",
        listed.stdout,
    );

    // The user's own file is still theirs.
    let text = config_text(&config);
    assert!(
        text.contains("# mine") && text.contains("htop"),
        "the comment and the unrelated table survive an option edit: {text:?}",
    );
}

/// `set-option -u` puts an option back to its default by REMOVING it, so the file says only what the
/// user chose — and it is idempotent, like `unbind-key`.
#[test]
fn set_option_u_removes_the_option_and_restores_the_default() {
    let config = ConfigHome::new("[options]\nprefix = \"C-a\"\n");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let run = sprag_env(&absent, &["set-option", "-u", "prefix"], &env);
    assert!(run.ok, "{}", run.stderr);
    assert!(
        !config_text(&config).contains("C-a"),
        "the value is gone from the file: {:?}",
        config_text(&config),
    );
    let listed = sprag_env(&absent, &["show-options"], &env);
    assert!(
        listed.stdout.lines().any(|line| line == "prefix C-b"),
        "and the default is in force: {:?}",
        listed.stdout,
    );

    // Unsetting what is already unset rewrites the same file rather than failing.
    let before = config_text(&config);
    let again = sprag_env(&absent, &["set-option", "-u", "prefix"], &env);
    assert!(again.ok, "{}", again.stderr);
    assert_eq!(before, config_text(&config), "idempotent");
}

/// A mistyped option NAME or VALUE is reported as an ARGUMENT — it must never name `config.toml`.
///
/// The rule this pins is the one `ConfigError` exists for, one level in: a user who typed
/// `set-option prefixx` has a fine config file, and a message prefixed with `config.toml` would send
/// them to read it. Both messages instead say what the alternatives are, which is the only thing that
/// makes a name-keyed table usable.
#[test]
fn set_option_reports_a_bad_argument_without_naming_the_file() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let unknown = sprag_env(&absent, &["set-option", "prefixx", "C-a"], &env);
    assert!(!unknown.ok, "a mistyped option is refused");
    assert!(
        !unknown.stderr.contains("config.toml"),
        "an argument mistake names no file: {}",
        unknown.stderr,
    );
    assert!(
        unknown.stderr.contains("prefix") && unknown.stderr.contains("detach-on-destroy"),
        "it lists the real options: {}",
        unknown.stderr,
    );

    let bad = sprag_env(&absent, &["set-option", "detach-on-destroy", "maybe"], &env);
    assert!(!bad.ok, "a value outside the vocabulary is refused");
    assert!(
        !bad.stderr.contains("config.toml"),
        "also an argument mistake: {}",
        bad.stderr,
    );
    assert!(
        bad.stderr.contains("no-detached"),
        "and it lists the values: {}",
        bad.stderr,
    );
    assert!(
        config_text(&config).is_empty(),
        "neither refusal touched the file: {:?}",
        config_text(&config),
    );
}

/// tmux's `-g` is accepted and a per-window / per-pane scope is refused BY NAME.
///
/// `-g` because every sprag option is global, so the flag a tmux user's fingers produce carries no
/// information and must simply work. `-w` / `-p` because there is no per-window or per-pane table:
/// accepting them would promise an overlay nothing holds, and silently ignoring them would leave a
/// user believing they had set something narrower than they had. The `-T root` treatment, one verb
/// over.
#[test]
fn an_option_scope_with_no_members_is_refused_by_name() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let global = sprag_env(&absent, &["set-option", "-g", "prefix", "C-a"], &env);
    assert!(global.ok, "tmux's own flag works: {}", global.stderr);
    assert!(sprag_env(&absent, &["show-options", "-g"], &env).ok);

    for scope in ["-w", "-p"] {
        let run = sprag_env(&absent, &["set-option", scope, "prefix", "C-a"], &env);
        assert!(!run.ok, "{scope} is refused rather than ignored");
        assert!(
            run.stderr.contains(scope) && run.stderr.contains("-g"),
            "naming what was asked for and what exists: {}",
            run.stderr,
        );
    }
}

/// `show-options` and `list-keys` can never disagree about the PREFIX.
///
/// Two verbs print it, which is safe only because they read ONE fact: the keymap's prefix is built
/// FROM the option rather than beside it. This is the drift guard that claim needs — a second home
/// for the prefix would show up here as two answers, and R235's defect was exactly a keymap and its
/// prefix disagreeing.
#[test]
fn the_prefix_reads_the_same_through_both_verbs() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    for spelling in ["C-a", "^o", "F1"] {
        assert!(sprag_env(&absent, &["set-option", "prefix", spelling], &env).ok);
        let shown = sprag_env(&absent, &["show-options"], &env);
        let listed = sprag_env(&absent, &["list-keys"], &env);
        let from_options = shown
            .stdout
            .lines()
            .find_map(|line| line.strip_prefix("prefix "))
            .map(str::to_owned);
        let from_keys = listed
            .stdout
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("prefix "))
            .map(str::to_owned);
        assert_eq!(
            from_options, from_keys,
            "{spelling:?}: show-options and list-keys must name one prefix",
        );
        assert!(from_options.is_some(), "and both printed one");
    }
}

/// `show-options NAME` prints one option, and `-v` prints its value ALONE — tmux's `show-options -v`,
/// and the singular read herdr spells `show-option`.
///
/// `-v` is what a script wants: `$(sprag show-options -v prefix)` needs no `cut`, and a value on a
/// line of its own cannot be mis-split by one.
#[test]
fn show_options_reads_one_option_by_name() {
    let config = ConfigHome::new("[options]\nprefix = \"C-a\"\n");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    let named = sprag_env(&absent, &["show-options", "prefix"], &env);
    assert!(named.ok, "{}", named.stderr);
    assert_eq!(named.stdout.trim(), "prefix C-a", "the tmux shape");

    let bare = sprag_env(&absent, &["show-options", "-v", "prefix"], &env);
    assert!(bare.ok, "{}", bare.stderr);
    assert_eq!(bare.stdout.trim(), "C-a", "the value alone");

    // An option the file never mentions still answers, with the default in force — the whole reason
    // the registry holds a default rather than the file.
    let unset = sprag_env(&absent, &["show-options", "-v", "detach-on-destroy"], &env);
    assert_eq!(unset.stdout.trim(), "on");

    // A mistyped name is refused with the list, like `set-option`'s.
    let unknown = sprag_env(&absent, &["show-options", "prefixx"], &env);
    assert!(!unknown.ok, "a mistyped name is refused");
    assert!(
        unknown.stderr.contains("gui-font") && !unknown.stderr.contains("config.toml"),
        "listing the real options and naming no file: {}",
        unknown.stderr,
    );

    // `-v` with nothing to read is refused rather than answered with every value: the flag exists so a
    // caller can read ONE without parsing, and a list would hand back the ambiguity it removes.
    let vague = sprag_env(&absent, &["show-options", "-v"], &env);
    assert!(!vague.ok, "-v needs a name");
    assert!(vague.stderr.contains("needs a name"), "{}", vague.stderr);
}

/// A NUMBER option is written unquoted and read back from either spelling.
///
/// `gui-font = 20` is how a person writes a size in a file they maintain by hand; demanding `"20"`
/// would be the parser's convenience imposed on the user. Both spellings are accepted BECAUSE the
/// writer emits one of them — a file this CLI edited and a file a human wrote must not differ.
#[test]
fn a_number_option_is_written_as_a_number() {
    let config = ConfigHome::new("");
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];

    assert!(sprag_env(&absent, &["set-option", "gui-font", "28"], &env).ok);
    let text = config_text(&config);
    assert!(
        text.contains("gui-font = 28") && !text.contains("\"28\""),
        "written unquoted, as a person writes a size: {text:?}",
    );

    // The QUOTED spelling a user may have written by hand reads back the same.
    let quoted = ConfigHome::new("[options]\ngui-font = \"28\"\n");
    let read = sprag_env(
        &absent,
        &["show-options", "-v", "gui-font"],
        &[("XDG_CONFIG_HOME", quoted.as_str())],
    );
    assert_eq!(read.stdout.trim(), "28", "either spelling is one value");

    // A TOML type no option takes names the option — not just the type serde wanted.
    let wrong = ConfigHome::new("[options]\ngui-font = 1.5\n");
    let refused = sprag_env(
        &absent,
        &["show-options"],
        &[("XDG_CONFIG_HOME", wrong.as_str())],
    );
    assert!(!refused.ok, "a float is not a value an option takes");
    assert!(
        refused.stderr.contains("gui-font") && refused.stderr.contains("config.toml"),
        "naming the option AND the file: {}",
        refused.stderr,
    );
}

/// **THE GATE for the first DAEMON-side option**: a pane the daemon births with no command runs the
/// user's `default-command`.
///
/// Every other option in this table is one client's, and the daemon reads none of them. This one it
/// must, because a pane is born THERE — so the claim can only be made against a real daemon reading a
/// real `config.toml`, which is what this does. It covers TWO birth paths, and they are separate code:
/// the standalone boot pane (`sprag-term` with no `--` command) and a pane created through the wire
/// `spawn` action (`split-window`, which every other daemon-side birth shares a spec parser with).
///
/// The claim discriminates: with `default-command = "cat"` the label is `cat`, where a daemon ignoring
/// the option labels the pane with `$SHELL`'s own name. Read through `sprag panes`, so it is the label
/// the daemon actually recorded.
///
/// REVERT-PROOF: put `default_shell_command()` back in `parse_spawn` and the split's line fails while
/// the boot pane's still passes — the two paths are asserted separately for exactly that reason.
#[test]
fn a_daemon_born_pane_runs_the_users_default_command() {
    let config = ConfigHome::new("[options]\ndefault-command = \"cat\"\n");
    // NO `--` command, so the boot pane has none of its own to prefer. `cat` blocks on its PTY, which
    // is what keeps the session alive for the rest of the test.
    let (_host, sock) = spawn_host_with(&[], &[("XDG_CONFIG_HOME", config.as_str())]);

    // Polled, because the boot pane is spawned after the bind: an unpopulated first read would be a
    // false failure rather than a claim about the option.
    let mut boot = String::new();
    let born = wait_for(Duration::from_secs(5), || {
        boot = sprag(&sock, &["panes"])
            .stdout
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned();
        !boot.is_empty()
    });
    assert!(born, "the boot pane appears");
    assert!(
        // The `(active)` marker trails the command on this row (a fresh window is on its only
        // pane), so the command is what the line ends with once that is taken off.
        boot.trim_end_matches("  (active)").ends_with("cat"),
        "the boot pane runs the user's default-command, not a shell: {boot:?}",
    );

    // ...and so does a pane born through the wire spawn action.
    let split = sprag(&sock, &["split-window", "-h", "0"]);
    assert!(split.ok, "split: {}", split.stderr);
    let listed = sprag(&sock, &["panes"]);
    assert!(listed.ok, "{}", listed.stderr);
    let labels: Vec<&str> = listed
        .stdout
        .lines()
        // The active pane's row trails its marker, so the LABEL is the last word before it.
        .filter_map(|line| {
            line.trim_end_matches("  (active)")
                .split_whitespace()
                .last()
        })
        .collect();
    assert_eq!(
        labels,
        vec!["cat", "cat"],
        "both the boot pane and the split's run it: {:?}",
        listed.stdout,
    );
}

/// A pane the DAEMON births retains the user's `history-limit`, not the emulator's default.
///
/// The gate for the whole seam, and it has to run against a real daemon: the limit is read from
/// `config.toml` at the birth, inside the process that owns the emulator, so a test that drove
/// `Workspace` directly would prove the pool honours a source without proving anything installs one.
/// That install lives on the path a restore also takes, and it is exactly the asymmetry R237 named —
/// a setting honoured on one birth path and silently ignored on another.
///
/// The numbers are chosen so no fallback can pass: `12` is not the 1000-line default and not the
/// screen height, and the output fed is longer than both. A pane still on the default retains ~1000
/// here, a pane whose limit never reached the emulator retains ~1000 too, and only a pane born with
/// the user's value retains 12.
#[test]
fn a_daemon_born_pane_retains_the_users_history_limit() {
    let config = ConfigHome::new("[options]\nhistory-limit = 12\n");
    let (_host, sock) = spawn_host_with(&[], &[("XDG_CONFIG_HOME", config.as_str())]);

    let mut born = false;
    wait_for(Duration::from_secs(5), || {
        born = !sprag(&sock, &["panes"]).stdout.trim().is_empty();
        born
    });
    assert!(born, "the boot pane appears");

    // 200 numbered lines: far past the configured 12 and past a screenful, so the retained count
    // can only be the limit.
    let typed = sprag(&sock, &["send-keys", "0", "-l", "seq 1 200 | sed 's/^/L/'"]);
    assert!(typed.ok, "send-keys: {}", typed.stderr);
    let entered = sprag(&sock, &["send-keys", "0", "Enter"]);
    assert!(entered.ok, "send-keys Enter: {}", entered.stderr);

    // Wait on the CONTENT the assertion reads — the last line arriving — rather than on a timer.
    let mut captured = String::new();
    let finished = wait_for(Duration::from_secs(20), || {
        captured = sprag(&sock, &["capture-pane", "0", "-p"]).stdout;
        captured.contains("L200")
    });
    assert!(finished, "the 200 lines were echoed: {captured:?}");

    let retained: Vec<&str> = captured
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.strip_prefix('L')
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()))
        })
        .collect();
    // Scrollback is the limit; the VISIBLE screen is on top of it and is not bounded by it, so the
    // total is "12 plus a screenful" rather than exactly 12. The discriminating fact is that it is
    // nowhere near the 1000 an unconfigured pane keeps.
    assert!(
        retained.len() < 200,
        "a limit of 12 must evict most of 200 lines, kept {}: {captured:?}",
        retained.len(),
    );
    assert!(
        retained.last() == Some(&"L200"),
        "the NEWEST line is the one kept — eviction takes from the oldest end: {retained:?}",
    );
}

/// `show-options` prints a COMMAND shell-quoted, and `-v` prints it raw.
///
/// Quoted because it is the only way an empty one can be seen at all — `default-command ''` says
/// something, a line ending in a space says nothing — and because a command with spaces on an
/// unquoted line is ambiguous to a reader. tmux prints a string option the same way. `-v` stays raw:
/// a script asked for the value, not a rendering of it.
#[test]
fn a_command_option_is_printed_quoted_and_read_raw() {
    let absent = socket_path();
    let empty = ConfigHome::new("");
    let shown = sprag_env(
        &absent,
        &["show-options"],
        &[("XDG_CONFIG_HOME", empty.as_str())],
    );
    assert!(shown.ok, "{}", shown.stderr);
    assert!(
        shown
            .stdout
            .lines()
            .any(|line| line == "default-command ''"),
        "an unset command is VISIBLE as empty: {:?}",
        shown.stdout,
    );

    let set = ConfigHome::new("[options]\ndefault-command = \"exec top -d 2\"\n");
    let env = [("XDG_CONFIG_HOME", set.as_str())];
    let shown = sprag_env(&absent, &["show-options"], &env);
    assert!(
        shown
            .stdout
            .lines()
            .any(|line| line == "default-command 'exec top -d 2'"),
        "a command with spaces is one quoted word: {:?}",
        shown.stdout,
    );
    let bare = sprag_env(&absent, &["show-options", "-v", "default-command"], &env);
    assert_eq!(
        bare.stdout.trim(),
        "exec top -d 2",
        "and -v hands a script the value itself",
    );
}

/// **The slice-5 claim, and H4's lesson applied**: a wire fact no shell can reach is not a delivered
/// capability. This drives the real CLI against a real daemon.
///
/// Also the shape a caller actually wants: `--since 0` for the backlog, and a cursor that advances
/// so the same change is not delivered twice.
#[test]
fn the_cli_reads_what_changed_by_cursor() {
    let (_host, sock) = spawn_host();

    // A bare `events` starts at NOW, not at zero: `events -f` means "tell me what happens", and a
    // daemon's whole history is not that.
    let now = sprag(&sock, &["events"]);
    assert!(now.ok, "events succeeded: {}", now.stderr);
    assert!(
        now.stdout.is_empty(),
        "the default cursor is the present, so nothing is replayed: {:?}",
        now.stdout,
    );

    let split = sprag(&sock, &["split-window", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let pane = split.stdout.trim().to_owned();

    let backlog = sprag(&sock, &["events", "--since", "0"]);
    assert!(backlog.ok, "events --since 0 succeeded: {}", backlog.stderr);
    assert!(
        backlog
            .stdout
            .lines()
            .any(|line| line == format!("pane_created\t{pane}")),
        "the change is readable as TYPE<TAB>SUBJECT, a shape a script can cut: {:?}",
        backlog.stdout,
    );
    assert!(
        backlog.stderr.is_empty(),
        "nothing was lost, so nothing is said about it: {:?}",
        backlog.stderr,
    );
}

/// **The blocking form, which is the whole reason the verb exists** — and the regression test for a
/// bug that a manual drive did NOT catch.
///
/// `sprag events -f` parks until something happens and then says what. The loop is `read at cursor`
/// then `waitFor at cursor`, and the wait's answer is a SIGNAL rather than a cursor: adopting it
/// would skip whatever was recorded AT that revision, which is never offered again because the read
/// is strictly greater than the cursor.
///
/// The first version did adopt it, and driving it by hand looked fine — a spawn bumps the revision
/// TWICE, so the record lands above the wait's answer and survives by luck. This test uses the
/// AGENT transition instead, which `ChannelRegistry::announce` publishes with a SINGLE bump and a
/// record at that exact revision. Under the bug it prints nothing at all.
#[test]
fn following_delivers_a_single_bump_change_the_wait_answers_with() {
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "printf '\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[H\
         \\342\\235\\257\\n  \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; cat",
    ]);

    let mut follow = Command::new(env!("CARGO_BIN_EXE_sprag"))
        .args(["events", "-f"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start `sprag events -f`");

    // Read ONE line, blocking. The settle window has to close before the waker publishes, so this
    // is what the verb is for: no polling, no sleep chosen by the caller.
    let stdout = follow.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut line = String::new();
        let read = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(read.map(|_| line));
    });

    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("`events -f` printed a line before the timeout")
        .expect("reading the line succeeded");
    let _ = follow.kill();
    let _ = follow.wait();

    assert_eq!(
        line.trim(),
        "pane_agent_state_changed\t0",
        "the settle waker's transition reaches a shell, at the revision the wait answers with",
    );
}

/// **R298, at the operator's surface: ONE request, many answers.** `sprag events -f` keeps printing
/// across SEVERAL separate changes, and it sends no request between them.
///
/// The test before this one reads a single line, so it would pass over either mechanism. This one
/// reads THREE lines produced by TWO later mutations, which is what a stream has to do and is where
/// a follow that lost its place after the first batch would stop.
///
/// The request COUNT is not observable from here — it was measured with `strace` against a
/// parent-commit control (4 requests for 1 change, 9 for 6; flat at 3 after) and that number lives in
/// the ledger. What is standing coverage is the delivery: `events/subscribe` answers once and the
/// daemon then speaks unprompted, twice, on a connection this process never writes to again.
#[test]
fn following_keeps_delivering_across_several_changes_on_one_request() {
    let (_host, sock) = spawn_host_running(&["cat"]);

    let mut follow = Command::new(env!("CARGO_BIN_EXE_sprag"))
        .args(["events", "-f", "--kind", "pane_created"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start `sprag events -f --kind pane_created`");

    // Filtered to ONE kind, so the lines are countable: a spawn also records a selection and a
    // layout change, and a test that counted those would be asserting the daemon's whole vocabulary
    // rather than the stream's continuity.
    let stdout = follow.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                return;
            }
        }
    });

    // A bare `events -f` starts at NOW rather than at zero, so there is no backlog line to wait on —
    // and READING THAT SILENCE is how this test knows the subscription is open before it makes a
    // change. Without it the first split could land before the subscribe and the cursor would start
    // past it, which reads as "the stream lost a record".
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_err(),
        "nothing has happened yet, so a follower says nothing",
    );

    let mut seen = Vec::new();
    for _ in 0..2 {
        assert!(sprag(&sock, &["split-window", "-h"]).ok, "the split landed",);
        let line = rx
            .recv_timeout(Duration::from_secs(30))
            .expect("a notification arrived for this change")
            .expect("reading it succeeded");
        seen.push(line.trim().to_owned());
    }
    let _ = follow.kill();
    let _ = follow.wait();

    assert_eq!(
        seen,
        vec!["pane_created\t1", "pane_created\t2"],
        "two changes after the one request, each delivered once and in order — a stream whose \
         cursor did not advance would repeat pane 1 here",
    );
}

/// **R292, at the operator's surface**: `sprag events -f` sleeps through a pane that is producing
/// output, and a `--pane`/`--kind` filter narrows what wakes it.
///
/// Two halves, and the first is the one that used to be broken. Against a pane writing continuously
/// the follow loop parked on `scene/waitFor`, which OUTPUT releases — so it read the slot, printed
/// nothing, and parked again, at socket speed (measured: 22 431 rounds a second). The verb looked
/// idle to a human while spinning — `sprag-latency`'s poll-pair row reproduces the rate and shows
/// the same loop costing 16x less when its cursor matches what it waits on. A `--pane` filter could
/// not have helped, because there was no
/// record to filter.
///
/// The second half is that it still delivers: the change the caller named arrives, from the same
/// flooding daemon.
#[test]
fn following_sleeps_through_output_and_wakes_for_the_named_change() {
    let (_host, sock) = spawn_host_running(&[
        "bash",
        "-c",
        "while :; do echo building a thing; sleep 0.02; done",
    ]);

    // A filter needs -f, because the daemon is the only matcher and a non-blocking read cannot ask
    // it anything. Refused with the sentence that says so, before any connection is made.
    let refused = sprag(&sock, &["events", "--pane", "0"]);
    assert!(!refused.ok, "a filter without -f is refused");
    assert!(
        refused
            .stderr
            .contains("narrow what to WAIT for, so they need -f"),
        "and says why: {:?}",
        refused.stderr,
    );

    // An unknown kind is refused BY THE DAEMON, whose vocabulary it is, and reaches the operator as a
    // sentence rather than behind a transport's phrase for an unanticipated fault.
    let unknown = sprag(&sock, &["events", "-f", "--kind", "pane_output"]);
    assert!(!unknown.ok, "an unknown kind is refused");
    assert!(
        unknown
            .stderr
            .contains("is not a change this terminal reports"),
        "with the daemon's own sentence: {:?}",
        unknown.stderr,
    );
    assert!(
        unknown.stderr.contains("pane_job_changed"),
        "offering the vocabulary it could have asked for: {:?}",
        unknown.stderr,
    );
    assert!(
        !unknown.stderr.contains("host rpc error"),
        "and not dressed as an unanticipated transport fault: {:?}",
        unknown.stderr,
    );

    let mut follow = Command::new(env!("CARGO_BIN_EXE_sprag"))
        .args(["events", "-f", "--kind", "pane_created"])
        .env("SPRAG_HOST_RPC_SOCK", &sock)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start `sprag events -f --kind pane_created`");

    let stdout = follow.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use std::io::BufRead;
        let mut line = String::new();
        let read = std::io::BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send(read.map(|_| line));
    });

    // HALF ONE: the pane is writing the whole time, and nothing arrives. Under the old loop this
    // window would have completed thousands of read-and-park rounds; a line here would mean output
    // had become an event.
    assert!(
        rx.recv_timeout(Duration::from_secs(2)).is_err(),
        "output must not reach a follower — it is not a change",
    );

    // HALF TWO: the change it asked for does.
    let split = sprag(&sock, &["split-window", "--", "cat"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let pane = split.stdout.trim().to_owned();

    let line = rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the named change reaches the follower")
        .expect("reading the line succeeded");
    let _ = follow.kill();
    let _ = follow.wait();

    assert_eq!(
        line.trim(),
        format!("pane_created\t{pane}"),
        "exactly the change named, with the session's output nowhere in it",
    );
}

/// `report-agent` / `release-agent` from a shell, with the pane taken from `$SPRAG_PANE` — the form a
/// hook uses, and the whole point of publishing that variable at a pane's birth.
///
/// The CLI is a surface of its own: every assertion here is about what a person or a hook can actually
/// type, which no wire-level test reaches. Three things are proven — the env default, the vocabulary's
/// refusals, and that a report typed at a shell reaches the pane list the daemon serves.
#[test]
fn the_cli_reports_and_releases_an_agent_for_the_pane_it_is_running_in() {
    let (_host, sock) = spawn_host();

    // No `$SPRAG_PANE` and no `--pane`: refused, naming the variable rather than suggesting a guess.
    let run = sprag(&sock, &["report-agent", "working"]);
    assert!(!run.ok, "a report with no pane cannot be honoured");
    assert!(
        run.stderr.contains("SPRAG_PANE"),
        "the error names the variable a pane would have: {}",
        run.stderr,
    );

    // The vocabulary, refused CLIENT-SIDE so a typo does not become a round trip — and `unknown`
    // pointed at the verb that actually means it.
    let run = sprag_env(&sock, &["report-agent", "unknown"], &[("SPRAG_PANE", "0")]);
    assert!(!run.ok);
    assert!(
        run.stderr.contains("release-agent"),
        "`unknown` is not a state; it is a release: {}",
        run.stderr,
    );
    let run = sprag_env(&sock, &["report-agent", "busy"], &[("SPRAG_PANE", "0")]);
    assert!(!run.ok);
    assert!(
        run.stderr.contains("working | blocked | idle"),
        "a spelling outside the vocabulary names the vocabulary: {}",
        run.stderr,
    );

    // The report itself, with the pane coming from the environment exactly as it does inside a pane.
    let run = sprag_env(
        &sock,
        &["report-agent", "blocked", "--name", "claude", "--seq", "5"],
        &[("SPRAG_PANE", "0")],
    );
    assert!(run.ok, "the report succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("accepted") && run.stdout.contains("(state changed)"),
        "and says what the daemon did with it: {}",
        run.stdout,
    );

    // It reached the daemon's own answer: `sprag agent` reads the pane list, so this is the same
    // publication a display client sees.
    let run = sprag(&sock, &["agent", "0"]);
    assert!(run.ok, "agent succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("0: blocked") && run.stdout.contains("claude"),
        "a `cat` pane no manifest claims is published because a report said so: {}",
        run.stdout,
    );

    // A replay of the same sequence number is REFUSED, and the CLI says so rather than exiting 0 on a
    // report that vanished.
    let run = sprag_env(
        &sock,
        &["report-agent", "idle", "--seq", "5"],
        &[("SPRAG_PANE", "0")],
    );
    assert!(
        run.stdout.contains("REFUSED"),
        "a replay is reported as refused: {} {}",
        run.stdout,
        run.stderr,
    );

    // The release, and its answer for a pane nobody is reporting any more.
    let run = sprag_env(&sock, &["release-agent"], &[("SPRAG_PANE", "0")]);
    assert!(run.ok, "release succeeded: {}", run.stderr);
    assert!(
        run.stdout.contains("released"),
        "the release says a report was dropped: {}",
        run.stdout,
    );
    let run = sprag_env(&sock, &["release-agent"], &[("SPRAG_PANE", "0")]);
    assert!(
        run.stdout.contains("nothing to release"),
        "and the second one says there was nothing left: {}",
        run.stdout,
    );
}

/// A hook's report does not outlive the agent it speaks for, even when that agent is a GRANDCHILD
/// of the pane — which is how an agent normally runs.
///
/// The pane's child here is an interactive shell, and the "agent" is a job started at its prompt,
/// exactly as a user typing `claude` produces. Killing that job leaves the shell alive, so the rule
/// slice 2 shipped — a report dies when the pane's own child reaches EOF — never fires for it. What
/// this proves is the whole chain a real crash goes through: `sprag hook` binds the report to
/// whatever owns the pane's terminal, the daemon samples that itself, and a sweep retires the report
/// once that job is gone.
///
/// Two controls, and the test is worth little without either. The report is read back WHILE the job
/// runs, so a rule that retired every bound report on sight could not pass; and the shell is
/// asserted alive at the end, so a pane that simply died cannot be what produced the recovery.
///
/// What this does NOT prove is which pass does the retiring — `sprag agent` reads the pane list, and
/// a pane-list read observes every pane it describes (R271). The sweep being the actor is proven
/// where it can be: `sweep_once` is called directly in `sweep.rs`'s own tests.
#[test]
fn a_hooks_report_does_not_outlive_an_agent_the_pane_did_not_spawn() {
    let (_host, sock) = spawn_host_running(&["bash", "--norc", "-i"]);
    let env = [("SPRAG_PANE", "0")];

    // The agent: a job at the shell's prompt, so it is one level below the pane's own child.
    assert!(sprag(&sock, &["send-keys", "0", "-l", "sleep 300"]).ok);
    assert!(sprag(&sock, &["send-keys", "0", "Enter"]).ok);
    // ⚠⚠ WAIT FOR THE JOB, NOT FOR THE ECHO. This waited until `sleep 300` appeared on the SCREEN,
    // and a pty echoes what was typed before the shell has forked anything — the same distinction
    // the readiness barrier draws between `ReadyWhen::Prints` and `ReadyWhen::Runs`, and the one
    // this project has paid for repeatedly. `sprag processes` reads the PROCESS TABLE, so this
    // waits for the thing the rest of the test is about.
    //
    // ⚠⚠⚠ THIS DID NOT FIX THE FAILURE IT WAS WRITTEN FOR, and saying so is the point. Under
    // whole-workspace load on another machine this test still fails, and instrumenting it there
    // showed why: `send-keys C-c` puts the byte on the tty — the screen shows the `^C` echo — and
    // NO SIGINT follows. The job is still alive, in its own process group, still foreground
    // (`pid == pgid`, state `S+`), and the daemon still names it as the pane's job. So the agent
    // report under test is CORRECT and it is this fixture's premise that fails. See the debt
    // register: what `send-keys C-c` guarantees is an open product question, not a flake.
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["processes"]).stdout.contains("sleep")
        }),
        "the shell FORKED the job, so there is something for the hook to bind to: {}",
        sprag(&sock, &["processes"]).stdout,
    );

    // The hook fires the way the agent's own config makes it fire: a payload on stdin, the pane from
    // the environment. Nothing here names a process — the daemon reads which one owns the terminal.
    let run = sprag_stdin(
        &sock,
        &["hook", "claude"],
        &env,
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#,
    );
    assert!(
        run.ok && run.stdout.is_empty(),
        "the hook is silent: {} {}",
        run.stdout,
        run.stderr,
    );
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["agent", "0"]).stdout.contains("0: working")
        }),
        "CONTROL: while the agent runs, its report stands: {}",
        sprag(&sock, &["agent", "0"]).stdout,
    );

    // The agent dies without running a hook, which is what a crash, a SIGKILL and an OOM all look
    // like from here.
    //
    // ⚠⚠⚠ **`stop-job` AND NOT `send-keys C-c`, AND THAT IS THE FIX FOR THIS TEST'S OWN FLAKE.**
    // A `C-c` is a BYTE on the pty, and whether a `SIGINT` follows is the line discipline's
    // decision at the instant it processes it — not the caller's. Under whole-workspace load on
    // another machine this fixture failed exactly there, and instrumenting it showed the byte
    // arriving (the screen carried its `^C` echo) with no signal behind it: the job alive, in its
    // own group, still foreground, and the daemon still naming it. The report under test was
    // CORRECT and the fixture's premise was not.
    //
    // `stop-job` signals the group itself, so what this fixture needs — the job GONE — is what it
    // now asks for, and the answer names what received it. The `C-c` path is measured on its own,
    // beside its control, in `sprag_terminal::stop`.
    let stopped = sprag(&sock, &["stop-job", "0"]);
    assert!(stopped.ok, "the pane's job is stopped: {}", stopped.stderr);
    assert!(
        stopped.stdout.contains("sleep"),
        "and the stop NAMES the job it reached, which is how this fixture knows it stopped the \
         agent rather than the shell: {}",
        stopped.stdout,
    );
    assert!(
        wait_for(Duration::from_secs(20), || {
            !sprag(&sock, &["agent", "0"]).stdout.contains("0: working")
        }),
        "the agent is gone, so its report is: {}",
        sprag(&sock, &["agent", "0"]).stdout,
    );

    // CONTROL: the pane's own child never died, so the EOF rule cannot be what did this.
    assert!(sprag(&sock, &["send-keys", "0", "-l", "echo still-here"]).ok);
    assert!(sprag(&sock, &["send-keys", "0", "Enter"]).ok);
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["capture-pane", "0"])
                .stdout
                .contains("still-here")
        }),
        "the shell is still running and answering",
    );
}

/// Run the CLI with `input` on its stdin.
///
/// The only way to exercise `hook`, which takes its payload there — and the only way to be sure the
/// installer's refusal is about a stdin that is not a TERMINAL rather than about a stdin that is
/// missing.
fn sprag_stdin(sock: &Path, args: &[&str], envs: &[(&str, &str)], input: &str) -> CliRun {
    let mut child = Command::new(env!("CARGO_BIN_EXE_sprag"))
        .args(args)
        .env("SPRAG_HOST_RPC_SOCK", sock)
        // ⚠⚠⚠⚠⚠ **THIS IS THE HELPER THE `hook` VERB RUNS THROUGH, AND `hook` IS THE ONE THAT
        // WRITES.** `note_hook_trouble` files `$XDG_STATE_HOME/sprag/hook-mute.<pane>` whenever a
        // report could not be delivered — *"the daemon is by definition unreachable when this is
        // written"* — so the refusal gates below leave a breadcrumb by construction. Without a
        // state home of its own it landed in the runner's real `~/.local/state`, and it was the
        // last residue CI's `ambient-home-guard` reported after register item 464 removed the
        // review ledger. ⚠ Bisected rather than reasoned, and THREE call sites were wrongly
        // accused first (the daemon spawner, the plain CLI runner, the socket-less one): the
        // artifact was `state/sprag/hook-mute.0` and only `hook` ever writes that name.
        //
        // ⚠ The same home the daemon on this socket has, for [`isolated_state_home`]'s reason —
        // these two are meant to share one machine's state. Before `envs`, so a caller still wins.
        .env("XDG_STATE_HOME", isolated_state_home(sock))
        .envs(envs.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run the sprag CLI");
    // ⚠⚠⚠⚠ Through [`sprag_gate::feeding`] — register item 471. The CLI can REFUSE before it reads
    // its payload (an unparseable argument, a socket that is not there), and a fixture that treated
    // the resulting `EPIPE` as fatal would report a write failure where the refusal it came for is
    // sitting in the exit status.
    sprag_gate::feeding::feed(&mut child, input.as_bytes());
    let output = child.wait_with_output().expect("wait for the sprag CLI");
    CliRun {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        ok: output.status.success(),
    }
}

/// A temporary `$HOME` holding an agent's own config directory, cleaned up on drop.
///
/// Every installer test runs against one of these. A test that reached the developer's real
/// `~/.claude/settings.json` would be a defect whatever it went on to assert.
struct AgentHome(PathBuf);
impl Drop for AgentHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl AgentHome {
    /// Unique per CALL, like [`socket_path`] and for the same reason.
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("sprag-cli-home-{}-{n}", std::process::id()));
        std::fs::create_dir_all(dir.join(".claude")).expect("a temp agent config dir");
        Self(dir)
    }

    fn as_str(&self) -> &str {
        self.0.to_str().expect("a utf-8 temp path")
    }

    /// What is in the agent's settings file, or `None` when there is no such file.
    fn settings(&self) -> Option<String> {
        std::fs::read_to_string(self.0.join(".claude").join("settings.json")).ok()
    }
}

/// The installer ASKS before it writes under a user's HOME, and an unanswerable question is not a
/// yes.
///
/// This is the decision that made the slice an owner's call rather than a technical one, so it is
/// asserted at the surface a person actually types. No daemon is spawned on purpose: installing a
/// hook edits a file and needs no server, exactly as `bind-key` does, and a socket that leads
/// nowhere proves it.
#[test]
fn the_installer_asks_before_writing_and_takes_back_exactly_what_it_wrote() {
    let sock = socket_path();
    let home = AgentHome::new();
    let env = [("HOME", home.as_str())];

    // --dry-run shows the edit and writes nothing.
    let run = sprag_env(&sock, &["install-hooks", "claude", "--dry-run"], &env);
    assert!(run.ok, "a dry run succeeds: {}", run.stderr);
    assert!(
        run.stdout.contains("settings.json") && run.stdout.contains("hook claude"),
        "it shows the file and the command it would add: {}",
        run.stdout,
    );
    assert_eq!(home.settings(), None, "--dry-run wrote nothing");

    // Nothing to ask on and no --yes: REFUSED. Not assumed yes (the promise was to ask) and not
    // assumed no (exiting 0 having silently done nothing is the failure mode being avoided).
    let run = sprag_env(&sock, &["install-hooks", "claude"], &env);
    assert!(
        !run.ok,
        "an unanswerable question must not be read as consent: {}",
        run.stdout,
    );
    assert!(
        run.stderr.contains("--yes"),
        "and it names the flag that answers in advance: {}",
        run.stderr,
    );
    assert_eq!(home.settings(), None, "still nothing on disk");

    // --yes answers it.
    let run = sprag_env(&sock, &["install-hooks", "claude", "--yes"], &env);
    assert!(run.ok, "the install succeeded: {}", run.stderr);
    let installed = home.settings().expect("a settings file");
    assert!(
        installed.contains("hook claude") && installed.contains("UserPromptSubmit"),
        "the command is wired to the events: {installed}",
    );

    // `list-hooks` reads back what is actually in the file.
    let run = sprag_env(&sock, &["list-hooks"], &env);
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("claude") && run.stdout.contains("installed"),
        "{}",
        run.stdout,
    );

    // And the uninstall takes back exactly what the install put in, leaving the file it created
    // empty rather than deleting a path the user may since have adopted.
    let run = sprag_env(&sock, &["uninstall-hooks", "claude", "--yes"], &env);
    assert!(run.ok, "the uninstall succeeded: {}", run.stderr);
    assert_eq!(home.settings().as_deref(), Some("{}\n"));
    let run = sprag_env(&sock, &["list-hooks"], &env);
    assert!(run.stdout.contains("available"), "{}", run.stdout);
}

/// The installed hook, end to end: it moves the pane it runs in, and says NOTHING anywhere else.
///
/// The silence is not politeness. An installed hook runs in every session of that agent, and most
/// of them are not sprag's — a multiplexer that makes somebody's agent print errors because it is
/// not in a pane, or because its own daemon is down, is not shippable. So the first assertion is
/// the negative one, and the pane that follows is its CONTROL: without it, this would pass on a
/// binary that does nothing at all.
///
/// ON THE INSTRUMENT (R271's lesson): reading the pane list DOES observe the pane it describes, so
/// it cannot prove the daemon woke by itself — that is not the claim here, and it is already proven
/// where it belongs. What a pane-list read cannot do is invent `working  claude` for a pane running
/// `cat`: no manifest claims that screen, so the only thing that can put an agent verdict on it is
/// the report this hook sent.
#[test]
fn an_installed_hook_moves_its_own_pane_and_stays_silent_outside_one() {
    let (_host, sock) = spawn_host();
    let prompt = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#;

    // Outside a pane: silent, and successful.
    let run = sprag_stdin(&sock, &["hook", "claude"], &[], prompt);
    assert!(
        run.ok,
        "a hook outside a sprag pane must succeed: {}",
        run.stderr,
    );
    assert!(
        run.stdout.is_empty() && run.stderr.is_empty(),
        "and print nothing at all: {:?} {:?}",
        run.stdout,
        run.stderr,
    );
    assert!(
        !sprag(&sock, &["agent"]).stdout.contains("claude"),
        "nothing was reported",
    );

    // The control: the same payload, with the pane the daemon publishes at a pane's birth.
    let run = sprag_stdin(&sock, &["hook", "claude"], &[("SPRAG_PANE", "0")], prompt);
    assert!(run.ok, "{}", run.stderr);
    let listed = sprag(&sock, &["agent"]).stdout;
    assert!(
        listed.contains("0: working  claude"),
        "the pane reports what its agent said: {listed}",
    );
    // WHERE the verdict came from, on the same line. A reported verdict carries a source and no
    // rule, and a reader has to be able to tell an authority from an inference — only one of the
    // two is corrected by editing a manifest.
    assert!(
        listed.contains("source=hook:claude") && !listed.contains("rule="),
        "and says who said so: {listed}",
    );
    // The advice follows the evidence: naming a manifest rule here would name one that never fired.
    let explained = sprag(&sock, &["agent", "0"]).stdout;
    assert!(
        explained.contains("release-agent") && !explained.contains("config.toml"),
        "a reported pane is corrected by a release, not by editing a rule: {explained}",
    );

    // A SUBAGENT's completion must not move the pane — a report outranks the screen, so a wrong one
    // would stand until something released it.
    let run = sprag_stdin(
        &sock,
        &["hook", "claude"],
        &[("SPRAG_PANE", "0")],
        r#"{"hook_event_name":"Stop","agent_id":"sub-1"}"#,
    );
    assert!(run.ok, "{}", run.stderr);
    assert!(
        sprag(&sock, &["agent"]).stdout.contains("working"),
        "a subagent's Stop left the pane working: {}",
        sprag(&sock, &["agent"]).stdout,
    );

    // …while the pane's own does.
    let run = sprag_stdin(
        &sock,
        &["hook", "claude"],
        &[("SPRAG_PANE", "0")],
        r#"{"hook_event_name":"Stop"}"#,
    );
    assert!(run.ok, "{}", run.stderr);
    assert!(
        sprag(&sock, &["agent"]).stdout.contains("0: idle  claude"),
        "{}",
        sprag(&sock, &["agent"]).stdout,
    );

    // SessionEnd is not a state: the agent is gone, and the pane goes back to the screen — which
    // claims nothing for a `cat`.
    //
    // POLLED rather than asserted once, and the reason is worth stating because nothing else in the
    // tree does: a released pane does NOT drop its verdict at once. `idle` -> `unknown` is a resting
    // transition, asserted by the ABSENCE of a working signal, so the settle window that guards
    // every such transition guards this one too — the report skipped hysteresis on the way in
    // (a report is not a sample) and the screen's answer does not skip it on the way out. An
    // assertion the instant after the release would be asserting that it does, and would pass only
    // by racing.
    let run = sprag_stdin(
        &sock,
        &["hook", "claude"],
        &[("SPRAG_PANE", "0")],
        r#"{"hook_event_name":"SessionEnd"}"#,
    );
    assert!(run.ok, "{}", run.stderr);
    assert!(
        wait_for(Duration::from_secs(10), || {
            !sprag(&sock, &["agent"]).stdout.contains("claude")
        }),
        "the release handed the pane back: {}",
        sprag(&sock, &["agent"]).stdout,
    );
}

/// ⚠⚠⚠⚠⚠ **A TURN THAT ENDED WITHOUT A `Stop` DOES NOT LEAVE ITS PANE `working`** — register item
/// 458, driven through the same door a real agent's hook uses.
///
/// Every row of [`sprag_host::hooks::CLAUDE`]'s table but one is a TURN BOUNDARY, so a turn whose
/// boundary is LOST raises nothing further and the last thing the pane said stands for ever — item
/// 344's ordinary case, where a rebuilt or refused reporter leaves a `working` that nothing can
/// correct. **Measured 2026-08-19**: a fresh run over such a pane sat at `0 iterations, 0 steps` for
/// six minutes until a person released it.
///
/// The agent's idle nag is raised by its own idleness timer rather than by anything about a turn, so
/// it is the one report that arrives in exactly that case. **Measured end to end the same day** with
/// a live agent whose `Stop` was dropped on its way to the daemon: the pane read `working` with the
/// turn already over, and the nag moved it to `idle` 60.02 s later.
///
/// ⚠⚠ **THE STEPS BELOW ARE THAT SEQUENCE AND NOT THE OTHER HALF OF 458.** A turn a person
/// INTERRUPTS emits no payload at all — Escape restores the prompt into the composer and the nag is
/// suppressed while it holds text — so nothing this file can send reproduces it, and no gate here
/// should pretend to. That half belongs to the wait, not to this table.
///
/// # ⚠⚠⚠⚠ The control is the DIALOG's own notice, and it is what stops this passing vacuously
///
/// A gate that only walked *notice → rest* would pass on a build that stopped reading
/// `notification_type` and called every notice rest — and that build lets a run type its next prompt
/// into a numbered menu, where the keystroke SELECTS. So `permission_prompt` reaches the same pane in
/// the same state and must leave it `blocked`. The five steps are one claim each, and the pane is
/// re-armed between them so no arm inherits its predecessor's answer.
#[test]
fn a_turn_that_ended_without_a_stop_does_not_leave_its_pane_working() {
    let (_host, sock) = spawn_host();
    let pane = [("SPRAG_PANE", "0")];
    let submit = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#;
    // Verbatim from the live capture but for the ids — see `hooks::captured_idle_notice`.
    let nag = r#"{"hook_event_name":"Notification","message":"Claude is waiting for your input","notification_type":"idle_prompt"}"#;
    // The kind a permission dialog carries, captured live 2026-08-19 on an Edit approval.
    let dialog = r#"{"hook_event_name":"Notification","message":"Claude needs your permission","notification_type":"permission_prompt"}"#;
    let hook = |payload: &str| {
        let run = sprag_stdin(&sock, &["hook", "claude"], &pane, payload);
        assert!(run.ok, "the hook must succeed: {}", run.stderr);
        sprag(&sock, &["agent"]).stdout
    };

    // ── 1. The turn OPENS, and nothing ends it. This is what a 529 and an Escape both look like
    //       from here: one boundary in, and then silence.
    assert!(
        hook(submit).contains("0: working  claude"),
        "the submit opened a turn",
    );

    // ── 2. THE FIX. The nag arrives with no turn boundary behind it, and the pane is at rest.
    let listed = hook(nag);
    assert!(
        listed.contains("0: idle  claude"),
        "⚠⚠⚠⚠⚠ THE PANE IS AT REST AND THIS IS THE ONLY THING THAT SAYS SO. Dropping this notice \
         leaves the dead turn's `working` standing until a person releases the pane — which is what \
         happened, twice, on the day this was written: {listed}",
    );
    assert!(
        listed.contains("source=hook:claude"),
        "and it is the agent's own account, not a rule reading the screen: {listed}",
    );

    // ── 3. RE-ARMED, so the arms below cannot inherit the answer above.
    assert!(hook(submit).contains("0: working  claude"), "working again");

    // ── 4. THE CONTROL. A DIALOG's notice reaches the same pane in the same state, and rest is the
    //       one thing it must not be read as.
    let listed = hook(dialog);
    assert!(
        listed.contains("0: blocked  claude"),
        "⚠⚠⚠⚠⚠ THE DIALOG MUST STILL BLOCK. A build that stopped reading the notice's KIND would \
         answer `idle` here, and a run that believes it types its prompt at a numbered menu where \
         the keystroke SELECTS: {listed}",
    );

    // ── 5. AND A STALE `blocked` IS CORRECTED TOO, which is item 458's other face exactly: the
    //       readiness barrier a fresh run waits on wants `idle`, and `blocked` is not it.
    assert!(
        hook(nag).contains("0: idle  claude"),
        "a report nobody is behind any more gives way to the agent's own statement of rest",
    );
}

/// ⚠⚠⚠⚠⚠ **A PANE BLOCKED ON SOMETHING NOBODY CAN PARSE STILL SAYS WHAT THE PEER ASKED FOR** —
/// register item 452, driven through the door a real agent's hook uses.
///
/// # What was being thrown away, and where
///
/// The daemon's answer for a blocked pane whose screen carries no readable menu was *"look at the
/// pane yourself"* — and the pane is a screen this build has just failed to read. Every round spent
/// on that failure was spent on the wrong layer: **the agent had already said what it wanted**, in
/// the `message` of the very payload that produced the word `blocked`, and `deliver_hook` dropped it
/// on the floor. This walks the whole distance — real hook process, real socket, real daemon, real
/// CLI — because a fixture that assembles the observation by hand proves the plumbing nobody built.
///
/// # ⚠⚠⚠ The retirement is asserted here too, and it is the sharp half
///
/// The tracker REPLACES this field instead of carrying it, so a request that has been dealt with
/// cannot be re-quoted at a later block. That decision is invisible in one payload and load-bearing
/// in three: the last step drives the peer back to work and demands the sentence be gone. A
/// supervisor quoting a stale notice would be telling somebody, in the peer's own voice, to go and
/// answer a question that no longer exists.
#[test]
fn a_blocked_pane_says_what_its_agent_asked_a_person_for() {
    let (_host, sock) = spawn_host();
    let pane = [("SPRAG_PANE", "0")];
    // The kind a permission dialog carries, captured live 2026-08-19 — the arm whose menu this
    // daemon may or may not be able to read, and the one a person is handed either way.
    let dialog = r#"{"hook_event_name":"Notification","message":"Claude needs your permission to use Bash","notification_type":"permission_prompt"}"#;
    let submit = r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#;
    let hook = |payload: &str| {
        let run = sprag_stdin(&sock, &["hook", "claude"], &pane, payload);
        assert!(run.ok, "the hook must succeed: {}", run.stderr);
        sprag(&sock, &["agent", "0"]).stdout
    };

    // ── 1. THE FIX, end to end. Nothing about the screen changed: a `cat` shows no menu at all, so
    //       the daemon's own parse answers nothing here and the sentence can only be the agent's.
    let explained = hook(dialog);
    assert!(
        explained.contains("0: blocked  claude"),
        "⚠ THE STAGING: the notice has to reach the daemon as a block before its words matter: \
         {explained}",
    );
    assert!(
        explained.contains("Claude needs your permission to use Bash"),
        "⚠⚠⚠⚠⚠ AND THE PERSON IS TOLD WHAT FOR. Without it the whole account is *look at the pane \
         yourself* — pointing them at a screen this daemon has just failed to read, while the agent \
         stated its business in the payload that produced the word `blocked`. Drop `noticed_in` \
         from `deliver_hook` and every other test in this file stays green: {explained}",
    );
    assert!(
        explained.contains("could not read as a menu"),
        "⚠⚠⚠ AND THE REMEDY IS UNCHANGED. The quotation is beside the instruction, never instead of \
         it: quoting a sentence is not parsing it into options, and a daemon that acted on prose it \
         could not read as a menu would be doing what that line exists to refuse: {explained}",
    );

    // ── 2. SOMEBODY DEALT WITH IT and the peer went back to work.
    assert!(
        hook(submit).contains("0: working  claude"),
        "⚠ THE STAGING for the arm below: the request is now answered",
    );

    // ── 3. THE RETIREMENT, ASKED WHERE IT IS OBSERVABLE. The peer blocks AGAIN, on something it did
    //       not describe — a notice with no `message`, which this daemon reads as *it did not say*.
    //
    //       ⚠⚠⚠⚠⚠ THE `working` STEP ABOVE CANNOT CARRY THIS CLAIM AND WAS FIRST WRITTEN AS THOUGH
    //       IT COULD. The person-facing line is printed only for a BLOCKED pane, so its absence at a
    //       working one is a fact about the renderer and not about the tracker — the assertion
    //       passed under the very mutation it named, which is this register's *a control can be
    //       vacuous* (items 441, 447) arriving by the front door. A second BLOCK is where a carried
    //       sentence would actually be spoken, and it is the hazard in its real shape: a peer that
    //       stops for a reason it did not state must not be given the last reason it did.
    let silent_block =
        r#"{"hook_event_name":"Notification","notification_type":"permission_prompt"}"#;
    let explained = hook(silent_block);
    assert!(
        explained.contains("0: blocked  claude"),
        "⚠ THE STAGING: this arm is about a SECOND block, so it has to be one: {explained}",
    );
    assert!(
        !explained.contains("Claude needs your permission"),
        "⚠⚠⚠⚠⚠ A REQUEST DOES NOT OUTLIVE THE REPORT THAT ANSWERED IT. Carry this field the way its \
         two neighbours are carried — one `or_else` in `Tracker::report` — and a person stopping at \
         this second, undescribed block is handed the FIRST one's question, in the peer's own voice, \
         which is the sort of evidence nobody re-checks: {explained}",
    );
    assert!(
        explained.contains("could not read as a menu"),
        "and the honest account of an undescribed block is the one that was always there: \
         {explained}",
    );
}

/// A state home two daemon GENERATIONS share, removed when the test ends.
///
/// [`isolated_state_home`] derives one from a socket, which is right for every gate that has a
/// single daemon and wrong for the one below: its whole subject is a file that OUTLIVES the daemon
/// that wrote it, so both generations must look in the same directory or there is nothing to
/// outlive. Kept as a guard rather than removed at the end because a failed assertion unwinds, and a
/// gate that leaks a directory tree on the way out is what `sprag-gate`'s ambient-home check exists
/// to catch.
struct SharedStateHome(PathBuf);
impl Drop for SharedStateHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Which generation the daemon on `sock` says it is, off its own handshake — the READER's half of
/// the comparison item 711 is about.
fn handshake_generation(sock: &Path) -> Option<String> {
    let mut conn = HostConn::connect(sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.handshake("cli-it-generation")
        .expect("the daemon answers the handshake");
    conn.daemon_generation().map(str::to_owned)
}

/// The value of `key=` in a `BORN gen=… pane=…` line a pane's own child printed.
fn born_field(printed: &str, key: &str) -> String {
    printed
        .split_whitespace()
        .find_map(|word| word.strip_prefix(key))
        .unwrap_or_else(|| panic!("the pane never printed {key}…: {printed:?}"))
        .to_owned()
}

/// ⛔⛔⛔⛔⛔ **A PANE THAT INHERITS A DEAD PANE'S NUMBER IS NOT TAKEN OFF ITS HOOK** — register item
/// 711, driven end to end through the doors the product uses: a real hook process leaves the word, a
/// real purge takes the panes, a real second daemon reissues the number, and a real `sprag agent`
/// reads it.
///
/// # ⛔⛔⛔ The failure this reproduces, measured before it was written
///
/// A mute breadcrumb is filed under a pane NUMBER, and a pane number is unique and never reused —
/// WITHIN one daemon. The counter starts over with the process, and nothing removes the file when a
/// pane dies or when a generation changes. On 2026-08-26 this host held fifteen of them with mtimes
/// from 12:39 to 22:45; `hook-mute.4` (14:02) and `hook-mute.6` (13:37) named numbers that a daemon
/// born at 22:47 had given to LIVE panes whose children started at 22:57. A watcher read one as a
/// live mute and called `release-agent` on a healthy reporter — **the number was right and its
/// subject was gone**.
///
/// # ⚠⚠⚠⚠⚠ Why this gate needs TWO GENERATIONS, and why the two gates that already read this file
/// could not have caught it
///
/// `a_reporter_that_left_word_is_flagged_mute` and its neighbours write a breadcrumb under a fixture
/// id and read it back in the same breath. **A fixture id is never reused**, so the whole hazard —
/// one number, two occupants, separated in time — is outside anything a single-lifetime fixture can
/// express. It is register item 686's shape (*"a scope that cannot disagree with itself is always
/// green"*) on the TIME axis, and the second axis is genuinely required here: a fixture with one
/// generation asserts nothing about attribution, because the only generation there is is the right
/// one.
///
/// So both premises are asserted INSIDE the gate rather than assumed:
///
/// * the breadcrumb is really on disk, with the generation that left it, and it really survives the
///   purge — which is the residue item 700 named and did not fix;
/// * the number is really reissued, read off the SECOND generation's pane printing its own id;
/// * and the two generations really differ.
///
/// # ⚠⚠⚠ The control is generation ONE reading the same file, and it is what stops this passing
/// vacuously
///
/// A build whose reader never says `mute` at all passes the fix half of this gate perfectly. So the
/// first generation — the one that actually left the word — must be told its reporter is mute off the
/// very same file, one command before the purge.
///
/// ⚠ The generation reaches the hook process in this test's `envs` because a test cannot be a CHILD
/// of the pane, which is how a real reporter inherits it. The VALUE is not invented: it is read off
/// the pane's own child, which printed what the daemon published to it, and it is asserted equal to
/// what the daemon says on its handshake — so the environment half and the wire half are pinned to
/// one fact here, and `pane_env_source`'s own gate covers the publication.
#[test]
fn a_pane_that_inherits_a_dead_panes_number_is_not_taken_off_its_hook() {
    let shared = SharedStateHome(scratch_state_home());
    let home = shared.0.display().to_string();
    let xdg = [("XDG_STATE_HOME", home.as_str())];
    // A boot pane whose child SAYS what it was born with, so the generation the hook stamps and the
    // number it is filed under both come from the product rather than from this file.
    let announce = [
        "sh",
        "-c",
        "printf 'BORN gen=%s pane=%s\\n' \"$SPRAG_PANE_GENERATION\" \"$SPRAG_PANE\"; exec cat",
    ];

    // ── 1. GENERATION ONE, and the two halves of its identity are ONE fact.
    let (first, first_sock) = spawn_host_with(&announce, &xdg);
    let born = wait_for_pane_text(&first_sock, "BORN ");
    let gen_one = born_field(&born, "gen=");
    assert_eq!(
        born_field(&born, "pane="),
        "0",
        "the boot pane is number 0, which is the number this gate reissues: {born:?}",
    );
    assert_eq!(
        handshake_generation(&first_sock).as_deref(),
        Some(gen_one.as_str()),
        "⚠⚠⚠⚠⚠ THE WRITER'S HALF AND THE READER'S HALF MUST BE THE SAME FACT. The hook reads the \
         generation out of its pane's ENVIRONMENT (it is written when the daemon cannot be reached, \
         so it cannot ask) and a reader learns it from the HANDSHAKE. Two values that merely look \
         alike would make every comparison below answer `Inherited` for a live pane — the inverse of \
         the bug, and just as wrong",
    );

    // ── 2. THE HOOK CANNOT DELIVER AND LEAVES WORD, under the number it was born with. Cause (ii)
    //       of the census — `hook-mute.47` was exactly this, written in the same second as the purge.
    let nowhere = shared.0.join("nobody-serves-this.sock");
    let left_word = sprag_stdin(
        &first_sock,
        &["hook", "claude"],
        &[
            ("SPRAG_PANE", "0"),
            ("SPRAG_PANE_GENERATION", gen_one.as_str()),
            ("SPRAG_HOST_RPC_SOCK", nowhere.to_str().expect("utf-8 path")),
            ("XDG_STATE_HOME", home.as_str()),
        ],
        r#"{"hook_event_name":"UserPromptSubmit","session_id":"s1"}"#,
    );
    assert!(
        left_word.ok,
        "a hook always exits 0, whatever it could not do: {}",
        left_word.stderr,
    );
    let breadcrumb = shared.0.join("sprag").join("hook-mute.0");
    let written = std::fs::read_to_string(&breadcrumb).unwrap_or_else(|why| {
        panic!(
            "⚠ THE PREMISE: the hook must actually have left word at {} — without a file on disk \
             every assertion below is vacuous ({why})",
            breadcrumb.display(),
        )
    });
    assert!(
        written.starts_with(&format!("generation {gen_one}\n")),
        "⚠⚠⚠ AND IT NAMES ITS OWN SUBJECT. A breadcrumb that says only what went wrong is the defect: \
         it is filed under a number the next counter reissues, so there is nothing to compare and \
         the reader answers about whoever holds the number next. Got {written:?}",
    );
    assert!(
        written.contains("could not reach the daemon"),
        "and the hook's own account of the failure is still in it: {written:?}",
    );

    // ── 3. THE CONTROL. Generation ONE reads the very same file and IS told its reporter is mute.
    //       Without this arm a build whose reader never says `mute` passes step 6 perfectly.
    assert!(
        sprag_env(
            &first_sock,
            &["report-agent", "working", "--pane", "0"],
            &xdg
        )
        .ok,
        "a reported verdict is what makes the reporter's health worth printing",
    );
    let live = sprag_env(&first_sock, &["agent", "0"], &xdg).stdout;
    assert!(
        live.contains("THAT REPORTER IS MUTE"),
        "⚠⚠⚠⚠⚠ THE CONTROL: this breadcrumb IS this generation's, so it must be acted on. A gate \
         whose reader is silent about every breadcrumb would pass the fix below while reporting \
         nothing at all: {live}",
    );

    // ── 4. THE PANES GO, exactly as the measured night's `kill-server --purge` took them — and the
    //       breadcrumb does NOT go with them, which is the residue that makes this reachable.
    assert!(
        sprag_env(&first_sock, &["kill-server", "--purge"], &xdg).ok,
        "the first generation ends the way the measured one did",
    );
    drop(first);
    assert!(
        breadcrumb.exists(),
        "⚠⚠ THE PREMISE THAT MAKES THIS BUG REACHABLE AT ALL: a purge destroys the snapshot and \
         every pane's history and leaves this file standing. Nothing prunes it — item 700 says so in \
         its own residue — so the next generation meets it under a number it has just reissued",
    );

    // ── 5. GENERATION TWO, in the same state home, reissuing the number from one.
    let (_second, second_sock) = spawn_host_with(&announce, &xdg);
    let reborn = wait_for_pane_text(&second_sock, "BORN ");
    let gen_two = born_field(&reborn, "gen=");
    assert_ne!(
        gen_one, gen_two,
        "⚠ THE PREMISE: two generations. One process cannot exhibit this hazard, which is why a \
         single-lifetime fixture is vacuous here: {reborn:?}",
    );
    assert_eq!(
        born_field(&reborn, "pane="),
        "0",
        "⚠⚠⚠ THE OTHER PREMISE, AND THE ONE A FIXTURE ID CANNOT SUPPLY: the number really is \
         reissued. `0` here and `0` above are two different panes, eight hours apart on the disk that \
         produced this item: {reborn:?}",
    );

    // ── 6. THE FIX. The new pane's reporter is healthy, and the word under its number is not its.
    assert!(
        sprag_env(
            &second_sock,
            &["report-agent", "working", "--pane", "0"],
            &xdg
        )
        .ok,
        "the new occupant reports too",
    );
    let inherited = sprag_env(&second_sock, &["agent", "0"], &xdg).stdout;
    assert!(
        !inherited.contains("THAT REPORTER IS MUTE"),
        "⛔⛔⛔⛔⛔ THE WHOLE ITEM. This pane's reporter has never failed once; the breadcrumb under \
         its number belongs to a pane that died eight hours ago in the measured case. Key the read on \
         the number alone — which is what shipped — and a healthy reporter is declared mute, which is \
         what sent a watcher to `release-agent` against it: {inherited}",
    );
    assert!(
        inherited.contains(&gen_one) && inherited.contains(&gen_two),
        "⚠⚠ AND IT SAYS WHOSE WORD IT WAS RATHER THAN SAYING NOTHING. Nothing prunes these files, so \
         a reader told only *not mute* meets the file itself later and reads it the way the watcher \
         did — naming both generations is what makes the thirty minutes item 712 measures cost \
         nothing: {inherited}",
    );
}

/// A daemon that is UP but wedged cannot stall a REQUEST VERB either — it says so and exits.
///
/// The hook path below has been bounded since R273 because an agent waits for it. Every other verb
/// waited forever, on a rationale that said a person can interrupt their own command. **Measured on
/// the first macOS CI run this repository ever completed**: nothing here is a person. Two `sprag`
/// processes started by this very file waited **3 h 38 min** against a peer that had dropped their
/// connections, the job died without reporting, and five rounds had already recorded macOS as
/// *unmeasured* because a hang produces no log at all.
///
/// So the claim has two halves and both are asserted: it GIVES UP, and it SAYS WHICH SILENCE THIS
/// IS. A bare `TimedOut` reaches an operator as the OS's `Resource temporarily unavailable`, which
/// reads like a socket that is not there — and this one is there, it accepted, and it is quiet.
/// [`sprag_rpc::HOST_SILENT`] is asserted from the wire's own constant rather than a copy, so a
/// reworded sentence fails here rather than drifting.
///
/// `ls` is the verb because it is the plainest read on the wire; the deadline is set in `connect`,
/// so any of the twenty-six would do.
#[test]
fn a_wedged_daemon_cannot_stall_a_request_verb() {
    let sock = socket_path();
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("a stand-in daemon");
    std::thread::spawn(move || {
        // HELD, not dropped, for the reason the hook's stand-in holds it: a closed stream is an EOF,
        // and an EOF is an answer. Being ignored is the case under test. The sleep outlasts the
        // CLI's own deadline by enough that a pass cannot come from this thread ending early.
        let _held = listener.accept();
        std::thread::sleep(Duration::from_secs(60));
    });

    let start = Instant::now();
    let run = sprag(&sock, &["ls"]);
    let waited = start.elapsed();
    let _ = std::fs::remove_file(&sock);

    assert!(
        !run.ok,
        "a verb that got no answer must not report success: {:?}",
        run.stdout,
    );
    // The two halves are asserted in this order so that each names its own defect: drop the
    // deadline and it is THIS one that fires (measured: 60 s, the stand-in's own sleep, because
    // nothing else was going to end the wait); keep the deadline and hand the OS's message
    // straight up, and only the one below does.
    assert!(
        waited < Duration::from_secs(30),
        "it gave up on the daemon rather than on the person: waited {waited:?}",
    );
    assert!(
        run.stderr.contains(HOST_SILENT),
        "it must name WHICH silence this is, not hand over an errno: {:?}",
        run.stderr,
    );
}

/// A daemon that is UP but wedged cannot stall the agent that ran the hook.
///
/// This is the failure a connect timeout cannot see: the socket accepts, so the connection succeeds,
/// and the call then waits for an answer that never comes. It matters because an agent WAITS for its
/// hooks — a multiplexer whose own daemon is stuck must not take somebody's editing session down
/// with it. The stand-in daemon accepts and answers nothing, which is exactly that shape.
///
/// REVERT-PROOF: drop `set_read_deadline` from `deliver_hook` and this hangs until the harness
/// gives up, while every other test in this file stays green.
#[test]
fn a_wedged_daemon_cannot_stall_the_agents_hook() {
    let sock = socket_path();
    let listener = std::os::unix::net::UnixListener::bind(&sock).expect("a stand-in daemon");
    std::thread::spawn(move || {
        // HELD, not dropped: closing the stream would give the client an EOF, which is an answer of
        // a kind. Being ignored is the case under test.
        let _held = listener.accept();
        std::thread::sleep(Duration::from_secs(20));
    });

    let start = Instant::now();
    let run = sprag_stdin(
        &sock,
        &["hook", "claude"],
        &[("SPRAG_PANE", "0")],
        r#"{"hook_event_name":"UserPromptSubmit"}"#,
    );
    let waited = start.elapsed();
    let _ = std::fs::remove_file(&sock);

    assert!(run.ok, "it still exits 0: {}", run.stderr);
    assert!(
        run.stdout.is_empty() && run.stderr.is_empty(),
        "and still says nothing: {:?} {:?}",
        run.stdout,
        run.stderr,
    );
    assert!(
        waited < Duration::from_secs(8),
        "it gave up on the daemon rather than on the agent: waited {waited:?}",
    );
}

/// `list-hooks` tells an install that still WORKS from one that merely still parses.
///
/// The settings file is written by hand rather than by an install, because the case being described
/// is a `sprag` that has MOVED since — which no install this test could run would produce. Without
/// the check this prints `installed` for six hooks that all fail.
#[test]
fn list_hooks_says_broken_when_the_binary_its_hooks_run_is_gone() {
    let sock = socket_path();
    let home = AgentHome::new();
    let gone = format!("{}/gone/sprag", home.as_str());
    std::fs::write(
        PathBuf::from(home.as_str()).join(".claude").join("settings.json"),
        json!({
            "hooks": {
                "Stop": [ { "hooks": [ { "type": "command", "command": format!("{gone} hook claude") } ] } ]
            }
        })
        .to_string(),
    )
    .expect("write the settings file");

    let run = sprag_env(&sock, &["list-hooks"], &[("HOME", home.as_str())]);
    assert!(run.ok, "{}", run.stderr);
    assert!(
        run.stdout.contains("BROKEN") && run.stdout.contains(&gone),
        "it names the binary that is gone: {}",
        run.stdout,
    );
    assert!(
        run.stdout.contains("install-hooks claude"),
        "and the command that repairs it: {}",
        run.stdout,
    );
}

/// `sprag processes` end to end against the real daemon: WHAT each pane is running, which the pane
/// listing cannot say.
///
/// The pair of readings IS the test, the way `sprag layout`'s is. The boot pane is spawned as `cat`
/// and `sprag panes` prints that label forever; this verb has to name what actually owns the
/// terminal, and here that is the same `cat` — so a second pane is opened running `sleep` and the
/// two rows are compared. A verb that simply re-printed the spawn label would pass on the first
/// pane and fail on nothing, which is why the assertions are about the DIFFERENCE.
///
/// Every line is pinned as text rather than probed by `contains`, because the rendering is the
/// product here: the id column feeds every other verb, and the argv column is the one place a
/// vector becomes a line.
#[test]
fn the_cli_says_what_each_pane_is_running() {
    let (_host, sock) = spawn_host();

    let split = sprag(&sock, &["split-window", "-h", "--", "sleep", "600"]);
    assert!(split.ok, "split-window succeeded: {}", split.stderr);
    let second = split.stdout.trim().to_owned();

    // A real shell takes a moment to become its own foreground job, so the reading is polled.
    let ran = |args: &[&str]| {
        let run = sprag(&sock, args);
        assert!(run.ok, "processes succeeded: {}", run.stderr);
        let (head, body) = run
            .stdout
            .split_once('\n')
            .expect("an age header and a body");
        assert!(
            head.starts_with("sampled ") && head.ends_with(" ms ago"),
            "the reading states its own age first: {head:?}",
        );
        body.to_owned()
    };
    assert!(
        wait_for(Duration::from_secs(10), || {
            ran(&["processes"]).contains(" sleep  sleep 600")
        }),
        "the second pane's job reaches the reading: {}",
        ran(&["processes"]),
    );

    // ONE pane, named — herdr's whole `pane process-info` surface, and here it is the narrow case
    // of a verb that answers about all of them at once.
    let one = ran(&["processes", &second]);
    let mut lines = one.lines();
    let head = lines.next().expect("the pane's own line");
    // `/dev/` rather than `/dev/pts/`: the SPELLING of a pty's slave device is the platform's, not
    // this product's — Linux says `/dev/pts/7` and macOS says `/dev/ttys007`. Pinning the Linux
    // spelling made this assertion a claim about a kernel's naming convention while reading as a
    // claim about sprag's output, and the first macOS run of this suite is what separated the two.
    assert!(
        head.starts_with(&format!("{second}: /dev/")) && head.contains("  child "),
        "the pane names its id, its terminal DEVICE and the child the daemon spawned: {head:?}",
    );
    let job = lines.next().expect("the job's line");
    let (pid, rest) = job
        .trim_start()
        .split_once(' ')
        .expect("a pid then the process");
    assert!(pid.parse::<u32>().is_ok(), "a real pid leads: {job:?}");
    assert_eq!(
        rest, "sleep  sleep 600",
        "then the kernel's name for it and its argv, quoted per argument",
    );
    assert_eq!(lines.next(), None, "and a one-process job is one line");

    // The pane listing, taken at the same daemon, carries the pane's spawn LABEL — and the label is
    // not the command line. Written from the failure this assertion produced when it was first
    // guessed at: `sprag panes` prints `sleep`, this verb prints `sleep 600`. The two verbs answer
    // different questions, and the arguments a process is actually running are only here.
    let listed = sprag(&sock, &["panes"]);
    let row = listed
        .stdout
        .lines()
        .find(|line| line.starts_with(&format!("{second}:")))
        .expect("the split pane lists");
    assert!(
        row.contains("  sleep") && !row.contains("600"),
        "the pane list stops at the program name: {row:?}",
    );

    // A pane id nobody has is an ERROR, not silence: the caller asked about that pane.
    let missing = sprag(&sock, &["processes", "4242"]);
    assert!(
        !missing.ok && missing.stderr.contains("no pane 4242"),
        "an absent pane is refused with the ids that do exist: {}",
        missing.stderr,
    );
}

/// The pane ids a `processes` / `resources` listing NAMED, in the order it printed them.
///
/// A row's own line starts at column 0 with `ID:`; a process line is indented, and the age header
/// is neither — so this reads the listing exactly as the rendering defines it rather than by
/// counting lines.
fn pane_ids_in(listing: &str) -> Vec<u64> {
    listing
        .lines()
        .filter(|line| !line.starts_with(' '))
        .filter_map(|line| line.split_once(':'))
        .filter_map(|(id, _)| id.parse().ok())
        .collect()
}

/// Sorted [`pane_ids_in`] — for comparing a listing against the set of panes a session HOLDS,
/// which has no order the daemon promises.
fn pane_id_set_in(listing: &str) -> Vec<u64> {
    let mut ids = pane_ids_in(listing);
    ids.sort_unstable();
    ids
}

/// **`processes` and `resources` ANSWER ABOUT THE SESSION THEY WERE SCOPED TO** — and `-a` is the
/// word for the machine-wide answer they used to give whatever you asked for.
///
/// # The defect, measured
///
/// Both verbs took `-t SESSION`, published it in `--help`, and then dropped it: the reading is
/// registry-wide, the narrowing was by PANE only, so with no pane named the scope reached nothing.
/// Against a live daemon on 2026-08-17, one session holding panes 0 and 2 and another holding 1
/// and 3:
///
/// ```text
/// sprag panes     -t 0       -> 0                    sprag panes     -t work -> 1  3
/// sprag processes -t 0       -> 0  2  1  3           sprag processes -t work -> 0  2  1  3
/// sprag processes -t nosuch  -> 0  2  1  3           sprag panes     -t nosuch -> no session named
/// ```
///
/// — the same four panes for every session, and a session name nobody has accepted in silence
/// where every other `-t` verb refuses it.
///
/// # What is asserted, and why each half is here
///
/// The two scoped listings are compared to the sessions' OWN pane sets and to EACH OTHER: an answer
/// that narrowed to something wrong would satisfy the first alone, and the defect's own sentence is
/// that the two answers are identical, so the discriminator is asserted as itself.
///
/// Each session holds panes in TWO WINDOWS, because the narrowing must be SESSION-wide — the same
/// reach [`resolve_pane`] gives a pane argument. A fixture with one window each would pass a
/// narrowing that stopped at the current window, which is a different (and smaller) answer.
///
/// `-a` is asserted to still see both sessions. Losing the machine-wide reading would be the wrong
/// repair: the pane eating the CPU may well be in a session the caller is not scoped to, which is
/// why that answer needs a WORD rather than an accident.
///
/// Both verbs, in one test, for the reason `pane_and_scope` is one function: they make the same
/// claim in `--help`, so a round that fixed one and left the other is the drift that helper exists
/// to prevent.
#[test]
fn a_scoped_process_listing_answers_about_that_session_and_not_the_machine() {
    let (_host, sock) = spawn_host();

    // The session this daemon booted with, and a second one — the two subjects.
    let created = sprag(&sock, &["new", "work"]);
    assert!(created.ok, "the second session: {}", created.stderr);

    // A SECOND WINDOW in each, holding a pane of its own. `new-window` selects what it makes, so
    // the pane it was born with is what `panes` then lists — the fixture shape the pane ratchet
    // next door already uses.
    let sole_pane_of_current_window = |session: &str| -> u64 {
        let listed = sprag(&sock, &["panes", "-t", session]);
        assert!(listed.ok, "panes -t {session}: {}", listed.stderr);
        let ids = pane_ids_in(&listed.stdout);
        assert_eq!(
            ids.len(),
            1,
            "a freshly made window holds exactly one pane, or this fixture is reading the wrong \
             window: {}",
            listed.stdout,
        );
        ids[0]
    };
    let first_of_boot = sole_pane_of_current_window("0");
    let first_of_work = sole_pane_of_current_window("work");
    assert!(sprag(&sock, &["new-window", "-t", "0", "spare"]).ok);
    let second_of_boot = sole_pane_of_current_window("0");
    assert!(sprag(&sock, &["new-window", "-t", "work", "extra"]).ok);
    let second_of_work = sole_pane_of_current_window("work");

    let mut boot: Vec<u64> = vec![first_of_boot, second_of_boot];
    boot.sort_unstable();
    let mut work: Vec<u64> = vec![first_of_work, second_of_work];
    work.sort_unstable();
    let mut every: Vec<u64> = boot.iter().chain(&work).copied().collect();
    every.sort_unstable();
    assert_eq!(every.len(), 4, "four distinct panes: {every:?}");

    // NOTHING IS POLLED HERE, and that is a decision rather than an omission. The reading is a
    // fresh walk (tolerance zero) of the very registry `sprag panes` was just read out of, so a
    // pane this fixture has already listed is in it — and the assertions below read pane IDS, never
    // a job, so they cannot race the moment a child takes its terminal. A `wait_for` would also
    // have had to name a listing to wait ON, and every listing here is one this round changes.
    let ran = |args: &[&str]| -> CliRun { sprag(&sock, args) };
    // The CLI's own published claim about these two verbs — a flag nothing names is a flag nobody
    // can find, and the usage is the SECOND list this binary keeps of what it accepts.
    let usage = ran(&["--nonsense"]).stderr;
    // Which session an UNSCOPED request lands in is the daemon's to say, so it is read rather than
    // assumed — the assertion about a bare invocation below is a claim about that session.
    let listed = ran(&["ls"]);
    assert!(listed.ok, "ls answered: {}", listed.stderr);
    assert!(
        listed
            .stdout
            .lines()
            .any(|row| row.starts_with("0:") && row.contains("(default)")),
        "the boot session is the daemon's default, or the bare invocation below means \
         something else: {}",
        listed.stdout,
    );

    for verb in ["processes", "resources"] {
        assert!(
            usage.contains(&format!("{verb} [PANE] [-t SESSION] [-a]")),
            "the usage spells {verb}'s pane, its scope and its -a: {usage}",
        );

        // THE BARE INVOCATION — no pane, no scope, no `-a` — which is the commonest one and the one
        // whose meaning this round moved furthest. It lands where `sprag panes` lands with nothing
        // named: the daemon's DEFAULT session, not the machine. Asserted separately from `-t 0`
        // because a fix that narrowed only an EXPLICIT scope would pass every other line here.
        let bare = ran(&[verb]);
        assert!(
            bare.ok,
            "{verb} with nothing named answered: {}",
            bare.stderr
        );
        assert_eq!(
            pane_id_set_in(&bare.stdout),
            boot,
            "{verb} with nothing named is the default session's panes: {}",
            bare.stdout,
        );

        let scoped_to_boot = ran(&[verb, "-t", "0"]);
        assert!(
            scoped_to_boot.ok,
            "{verb} -t 0 answered: {}",
            scoped_to_boot.stderr
        );
        let scoped_to_work = ran(&[verb, "-t", "work"]);
        assert!(
            scoped_to_work.ok,
            "{verb} -t work answered: {}",
            scoped_to_work.stderr
        );
        assert_eq!(
            pane_id_set_in(&scoped_to_boot.stdout),
            boot,
            "{verb} -t 0 lists the boot session's panes, both windows of it: {}",
            scoped_to_boot.stdout,
        );
        assert_eq!(
            pane_id_set_in(&scoped_to_work.stdout),
            work,
            "{verb} -t work lists the other session's panes, both windows of it: {}",
            scoped_to_work.stdout,
        );
        // THE DISCRIMINATOR, as the defect's own sentence: the two scopes must not print the same
        // panes. Asserted directly, because both equalities above would hold for a narrowing that
        // was right by accident on one fixture and this cannot.
        assert_ne!(
            pane_id_set_in(&scoped_to_boot.stdout),
            pane_id_set_in(&scoped_to_work.stdout),
            "the two sessions get DIFFERENT answers: {} vs {}",
            scoped_to_boot.stdout,
            scoped_to_work.stdout,
        );

        // The machine-wide reading is still reachable, and now it has a word.
        let everywhere = ran(&[verb, "-a"]);
        assert!(everywhere.ok, "{verb} -a answered: {}", everywhere.stderr);
        assert_eq!(
            pane_id_set_in(&everywhere.stdout),
            every,
            "{verb} -a crosses every session: {}",
            everywhere.stdout,
        );

        // A session nobody has is REFUSED, the way every other `-t` verb refuses it — it used to be
        // accepted in silence and answered with the whole machine. BOTH SPELLINGS, and the second
        // one is why the refusal is a PRE-FLIGHT rather than a by-product: a narrowed listing
        // refuses a bad scope on its way to reading the session's panes, but `-a` reads nothing
        // scoped at all, so without the pre-flight `sprag processes -a -t nosuch` would answer
        // happily about a session that does not exist. A green mutation is what asked this
        // question — dropping `connect_scoped` left every other assertion here passing.
        for ghosted in [vec![verb, "-t", "nosuch"], vec![verb, "-a", "-t", "nosuch"]] {
            let ghost = ran(&ghosted);
            assert!(
                !ghost.ok,
                "`sprag {}` is refused rather than answered: {}",
                ghosted.join(" "),
                ghost.stdout,
            );
            assert!(
                ghost.stderr.contains("no session named"),
                "`sprag {}` names the missing session: {}",
                ghosted.join(" "),
                ghost.stderr,
            );
        }

        // A PANE still wins over the scope, and still reaches a window over — `second_of_boot` is
        // in the `spare` window, and the caller is scoped to the session, not to that window.
        let one = ran(&[verb, &second_of_boot.to_string(), "-t", "0"]);
        assert!(one.ok, "{verb} PANE answered: {}", one.stderr);
        assert_eq!(
            pane_id_set_in(&one.stdout),
            vec![second_of_boot],
            "{verb} PANE narrows to that pane alone: {}",
            one.stdout,
        );

        // `-a` and a PANE are a contradiction — every pane, and this one — so it is refused rather
        // than silently resolved one way, which is the failure this whole test is about.
        let both = ran(&[verb, &second_of_boot.to_string(), "-a"]);
        assert!(
            !both.ok,
            "{verb} PANE -a is refused as a contradiction: {}",
            both.stdout,
        );

        // A CALLER STANDING IN A PANE has one resolved for it with nothing named, so `-a` must be
        // read BEFORE that — a flag tested after the ambient pane is a flag accepted and dropped,
        // which is this test's own subject one level down. The env is set explicitly because the
        // harness strips it (item 226), so both lines below are a stated intention.
        let standing_in = second_of_boot.to_string();
        let inside = [(sprag_host::PANE_ENV_VAR, standing_in.as_str())];
        // THE CONTROL. Without it the `-a` arm proves nothing: a build that never resolved an
        // ambient pane at all would satisfy the assertion under it for the wrong reason.
        let ambient = sprag_env(&sock, &[verb], &inside);
        assert!(
            ambient.ok,
            "{verb} inside a pane answered: {}",
            ambient.stderr
        );
        assert_eq!(
            pane_id_set_in(&ambient.stdout),
            vec![second_of_boot],
            "{verb} run inside a pane answers about that pane: {}",
            ambient.stdout,
        );
        let ambient_all = sprag_env(&sock, &[verb, "-a"], &inside);
        assert!(
            ambient_all.ok,
            "{verb} -a inside a pane answered: {}",
            ambient_all.stderr,
        );
        assert_eq!(
            pane_id_set_in(&ambient_all.stdout),
            every,
            "{verb} -a asks past the pane it is standing in: {}",
            ambient_all.stdout,
        );
    }
}

/// **`agent` AND `run` ANSWER ABOUT THE SESSION THEY WERE SCOPED TO** — the last two `-t` verbs a
/// fixture can be built for, and the two nothing measured.
///
/// # Why these two, and why now
///
/// This closes the sweep item 425 started. Every other `-t` verb has been measured on three
/// questions — an unknown scope is refused (425, 427), no verb ACTS on the wrong session (17 probed,
/// all clean), and a pane address does not cross a session (18 probed, all clean). The third
/// question, *does `-t` actually SELECT*, is the milestone's own defect, and for `agent` and `run`
/// it was answered by nothing: **every existing test of both verbs uses ONE session and no `-t`
/// at all.** A regression of exactly 425's shape — the flag validated and then dropped — would have
/// been invisible here.
///
/// # The fixture, and why each half is built the way it is
///
/// A fact is placed in ONE session and the other must not report it. For `agent` that is a reported
/// state, which is why `report-agent` names its pane. For `run` it is a PROJECT, which the daemon
/// discovers from a pane's LIVE working directory — so the boot pane `cd`s into one and the second
/// session's pane, born wherever the daemon runs, does not. Pointing the daemon's `HOME` at the
/// project (the way the older `run` test does) would put BOTH sessions' panes inside it and
/// discriminate nothing.
///
/// ⚠ The `run` assertions are about which project's command APPEARS, never about the second session
/// failing: whether a pane outside the fixture has some other project is a fact about the machine
/// this suite runs on, and an assertion resting on that would be a claim about the checkout.
///
/// The project directory is a [`TempDir`] — the ssh test's guard, reused rather than copied — so a
/// panicking assertion leaves nothing under `/tmp`.
#[test]
fn agent_and_run_answer_about_the_session_they_were_scoped_to() {
    let project = TempDir(
        std::env::temp_dir().join(format!("sprag-cli-scoped-project-{}", std::process::id())),
    );
    std::fs::create_dir_all(&project.0).expect("create the temp project");
    std::fs::write(
        project.0.join(sprag_host::PROJECT_FILE),
        "[[command]]\nname = \"only-in-zero\"\nrun = [\"true\"]\n",
    )
    .expect("write the project config");

    // The boot pane sits INSIDE the project; `sprag new`'s pane will not.
    let (_host, sock) =
        spawn_host_running(&["sh", "-c", &format!("cd {}; exec cat", project.0.display())]);
    assert!(sprag(&sock, &["new", "work"]).ok, "the second session");

    let boot_pane = sprag(&sock, &["panes", "-t", "0"])
        .stdout
        .lines()
        .next()
        .and_then(|row| row.split(':').next().map(str::to_owned))
        .expect("the boot pane");

    // ── `agent`: a state reported into session 0's pane, and only there.
    let reported = sprag(
        &sock,
        &["report-agent", "working", "--pane", &boot_pane, "-t", "0"],
    );
    assert!(reported.ok, "report-agent accepted: {}", reported.stderr);
    assert!(
        wait_for(Duration::from_secs(10), || {
            sprag(&sock, &["agent", "-t", "0"])
                .stdout
                .contains("working")
        }),
        "the boot session reports the agent: {}",
        sprag(&sock, &["agent", "-t", "0"]).stdout,
    );
    let elsewhere = sprag(&sock, &["agent", "-t", "work"]);
    assert!(elsewhere.ok, "agent -t work answered: {}", elsewhere.stderr);
    assert!(
        !elsewhere.stdout.contains("working"),
        "the OTHER session does not report a pane it does not hold — if this went red, `-t` \
         stopped selecting and every session sees every agent: {}",
        elsewhere.stdout,
    );

    // ── `run`: the project of the scoped session's pane.
    let here = sprag(&sock, &["run", "-t", "0"]);
    assert!(here.ok, "run -t 0 listed: {}", here.stderr);
    assert!(
        here.stdout.contains("only-in-zero"),
        "the boot session's pane is in the fixture's project: {}",
        here.stdout,
    );
    let there = sprag(&sock, &["run", "-t", "work"]);
    assert!(
        !there.stdout.contains("only-in-zero"),
        "the OTHER session's pane is not, so its commands are not this project's: {} / {}",
        there.stdout,
        there.stderr,
    );
    // ⚠ AND IT MUST BE TALKING ABOUT A PANE OF `work`. The line above is NOT enough on its own, and
    // a green mutation is what proved it: with the pane pick unscoped, `run -t work` chose the boot
    // session's pane and the project query — still scoped to `work` — answered null, so it printed
    // *"pane 0 is in no project"* and satisfied the assertion above for entirely the wrong reason.
    // Naming the pane is what separates "answered about work" from "answered about 0 and missed".
    let said = format!("{}{}", there.stdout, there.stderr);
    assert!(
        !said.contains(&format!("pane {boot_pane} ")),
        "run -t work must not be answering about the BOOT session's pane {boot_pane}: {said}",
    );

    // And neither verb reaches ACROSS: the pane is session 0's, named while scoped to `work`.
    for argv in [
        vec!["agent", boot_pane.as_str(), "-t", "work"],
        vec!["run", "--pane", boot_pane.as_str(), "-t", "work"],
    ] {
        let crossed = sprag(&sock, &argv);
        assert!(
            !crossed.ok,
            "`sprag {}` does not reach a pane of another session: {}",
            argv.join(" "),
            crossed.stdout,
        );
        assert!(
            crossed
                .stderr
                .contains(&format!("no pane {boot_pane} in work")),
            "`sprag {}` says which session it looked in: {}",
            argv.join(" "),
            crossed.stderr,
        );
    }
}

/// `list-keys -N` is the same table in the form a PERSON reads — and the paste-back form is
/// untouched, which is the contract this flag exists to protect.
///
/// Both halves matter and only together. tmux's own `-N` is a second view of one table, and the
/// reason sprag copies that rather than changing the default output is that every line of the
/// default after the first is a `bind-key` command a user can paste back; a script filtering on that
/// prefix must not have the ground moved under it by a round about readability. So this asserts what
/// the notes form ADDS and, in the same run, that the old form still says exactly what it said.
///
/// The third assertion is the one that could not exist before R308: the view names the actions NO
/// key reaches. `%` is unbound here, and `split-window` still reaches the vocabulary because the
/// file binds `|` to it — so the mark is about the VERB, and the row for `zoom-pane` is the one that
/// moves when its only key goes away.
#[test]
fn list_keys_notes_form_reads_as_a_table_and_leaves_the_paste_back_form_alone() {
    let config = ConfigHome::new(
        "[options]\nprefix = \"C-a\"\n\n\
         [[bind]]\nkey = \"|\"\naction = \"split-window -h\"\n\n\
         [[unbind]]\nkey = \"%\"\n\n\
         [[unbind]]\nkey = \"z\"\n",
    );
    let absent = socket_path();
    let env = [("XDG_CONFIG_HOME", config.as_str())];
    let notes = sprag_env(&absent, &["list-keys", "-N"], &env);
    assert!(
        notes.ok,
        "no daemon is not an error here either: {}",
        notes.stderr
    );
    let text = notes.stdout;

    // THE CHORD IN FORCE, so a reader with a rebound prefix is not sent to look it up elsewhere.
    assert!(
        text.contains("C-a |"),
        "the notes form shows the chord a user presses: {text}",
    );
    assert!(
        !text.contains("C-b "),
        "and never the default prefix this file moved: {text}",
    );
    // GROUPED, by what each verb acts on.
    for heading in ["client", "pane", "window", "session"] {
        assert!(
            text.lines().any(|line| line == heading),
            "the {heading} group has a heading of its own: {text}",
        );
    }
    // AND WHAT IS NOT BOUND, which is the question the paste-back form cannot answer at all.
    assert!(
        text.contains(&format!(
            "zoom-pane [-Z|-u]  ({})",
            sprag_host::keyhelp::KeyHelp::UNBOUND
        )),
        "the verb whose only key the file removed is marked: {text}",
    );
    assert!(
        !text
            .lines()
            .any(|line| line.trim_start().starts_with("split-window")
                && line.contains(sprag_host::keyhelp::KeyHelp::UNBOUND)),
        "and a verb the file gave a NEW key to is not: {text}",
    );

    // THE CONTRACT: the default form is byte-for-byte what it was, flag or no flag.
    let plain = sprag_env(&absent, &["list-keys"], &env);
    assert!(plain.ok, "{}", plain.stderr);
    assert!(
        plain
            .stdout
            .lines()
            .skip(1)
            .all(|line| line.starts_with("bind-key")),
        "every line after the prefix is still a command a user can paste back: {}",
        plain.stdout,
    );
    assert_ne!(
        plain.stdout, text,
        "the two forms are different views, or one of them is pointless",
    );

    // A FLAG THIS VERB DOES NOT HAVE is refused by name rather than ignored.
    let bad = sprag_env(&absent, &["list-keys", "-Q"], &env);
    assert!(!bad.ok, "an unknown flag is refused: {}", bad.stdout);
    assert!(
        bad.stderr.contains("-Q") && bad.stderr.contains("[-N]"),
        "and the refusal names both what was given and what is taken: {}",
        bad.stderr,
    );
}

/// The usage line names the flag `list-keys` grew, because nothing else makes it.
///
/// `USAGE`'s own doc says why this test exists at all: it is a SECOND list of what this binary does,
/// and *"a second list is exactly what nothing checks — `sprag bind-key` held one that was stale for
/// eight rounds"*. R308 added `-N` and did not update it, which the round's own audit caught. This
/// is the assertion that would have caught it instead, and it is deliberately narrow: it pins the
/// flag this round added rather than claiming to police the whole list, which would need a verb
/// enumeration this binary does not have.
#[test]
fn the_usage_line_names_the_flag_list_keys_takes() {
    let absent = socket_path();
    let run = sprag_env(&absent, &["--nonsense"], &[]);
    assert!(!run.ok, "an unknown verb is refused: {}", run.stdout);
    assert!(
        run.stderr.contains("list-keys [-N]"),
        "the usage names the notes form, or a user cannot find it: {}",
        run.stderr,
    );
}

/// **THE CLI RATCHET: every verb the USAGE says takes a PANE accepts a NAME, and reaches a pane one
/// window over — and the list of verbs is DERIVED FROM THE USAGE, not written here.**
///
/// # Why the usage is the source
///
/// The usage text is the CLI's own published claim about which verbs take a pane. Before R312 that
/// claim was false for every one of them: measured against a live two-window daemon at `e7be5eb`,
/// **no CLI verb accepted a pane's NAME at all**, and the refusals came in SIX different sentences
/// (`pane id "x" must be a number` / `"x" is not a pane id` / `"x" is neither a direction flag nor
/// a pane id` / `"x" is neither a flag nor a pane id` / `"x" is neither -t nor a pane id` /
/// `--pane "x" is not a pane id (a number)`). Worse, the CLI contradicted itself about whether a
/// pane existed: `zoom-pane 1` / `rename-pane 1` / `swap-pane 1 0` succeeded against a pane one
/// window over while `capture-pane 1` / `agent 1` / `select-pane 1` refused it, on the same daemon
/// at the same instant.
///
/// Deriving the list means a verb ADDED to the usage with a PANE is checked the day it is added,
/// and a verb that resolves its pane against one window fails here by name.
///
/// # What it asserts
///
/// That the ADDRESS RESOLVES — never that the verb succeeds. `resize-pane` legitimately refuses a
/// window nothing is watching (measured with a control: it refuses the caller's OWN window
/// identically), and `split-window` refuses a target with no axis. What must not happen is a
/// refusal about the SPELLING, which is what all six sentences above were.
#[test]
fn every_verb_the_usage_says_takes_a_pane_reaches_one_a_window_over() {
    let (_host, sock) = spawn_host();

    // A fixture per verb, ALL BUILT UP FRONT, and that is not tidiness: these verbs REACH, so
    // `kill-pane` ends the far pane's window, `join-pane` and `swap-pane` move panes into the
    // caller's own, and any of those would leave a later check measuring nothing. Each verb gets
    // its own window and its own name, so no verb can disturb another's.
    let build = |sock: &Path, name: &str| {
        assert!(sprag(sock, &["new-window", "-t", "0"]).ok);
        let listed = sprag(sock, &["panes", "-t", "0"]).stdout;
        let rows: Vec<&str> = listed.lines().collect();
        assert_eq!(
            rows.len(),
            1,
            "a new window is born with exactly one pane, or this fixture is looking at the wrong \
             window: {listed}",
        );
        let far = rows[0].split(':').next().expect("a pane id").to_owned();
        assert!(sprag(sock, &["rename-pane", &far, name, "-t", "0"]).ok);
        // The pane PRINTS, so `wait-for-output` has something to see: it parks with no deadline by
        // design, and a ratchet that hung on it would be measuring nothing at all.
        assert!(sprag(sock, &["send-keys", name, "-l", "marker", "-t", "0"]).ok);
        assert!(sprag(sock, &["send-keys", name, "Enter", "-t", "0"]).ok);
        far
    };

    // A sample per ARGUMENT SHAPE the usage spells, so a verb built from shapes already here needs
    // no edit — and one that introduces a new shape fails loudly naming it. Failing closed is the
    // point: a silently skipped verb is a verb this ratchet believes it covered.
    let extra = |verb: &str| -> Vec<&'static str> {
        match verb {
            "find" | "wait-for-output" => vec!["marker"],
            "join-pane" => vec!["0"],
            "move-pane" => vec!["-h", "0"],
            "select-pane" | "swap-pane" | "resize-pane" => vec!["-L"],
            "split-window" => vec!["-h"],
            "rename-pane" => vec!["renamed"],
            "send-keys" => vec!["Escape"],
            "report-agent" => vec!["working"],
            // `grant` refuses a request that sets nothing, deliberately — so it is given
            // the weight every pane is born with. That leaves the pane exactly as it was,
            // which matters here because this ratchet drives a LIVE pane one window over.
            "grant" => vec!["--share", "100"],
            // `run` with NO name LISTS the pane's project commands, which resolves the pane and
            // prints — everything this ratchet needs. It entered the sweep at R323, when the usage
            // stopped being a hand-written list and started naming every verb the binary
            // dispatches: this verb had been dispatched and undocumented, so nothing derived from
            // the usage could see it.
            // `stop-job` needs nothing else — the pane one window over is at its own prompt, so
            // the stop reaches its shell, which is what a `Ctrl-C` at a prompt reaches and what a
            // shell answers by redrawing it. That leaves the pane exactly as it was, which is what
            // this ratchet needs of a verb it drives against a LIVE pane.
            // ⚠ `answer-pane` against a pane that is NOT asking, deliberately: no agent manifest
            // claims a shell one window over, so the run converges having typed nothing and the
            // pane is left exactly as it was — which is what this ratchet needs of a verb it
            // drives against a LIVE pane. What it measures here is the ADDRESSING: that a pane
            // NAME reaches a window over, on the newest verb that takes one.
            "answer-pane" => vec!["--asked", "marker", "--answer", "marker"],
            "run" | "break-pane" | "processes" | "resources" | "kill-pane" | "zoom-pane"
            | "capture-pane" | "agent" | "release-agent" | "events" | "stop-job" => vec![],
            other => panic!(
                "the usage says {other:?} takes a PANE and this ratchet has no other arguments for \
                 it. Add them — a skipped verb is a verb this test believes it covered."
            ),
        }
    };

    // Every verb the usage spells with a PANE, and HOW it takes one (positionally or behind
    // `--pane`). Read out of the usage rather than listed here, so the two cannot drift — and the
    // usage is itself DERIVED from `sprag_host::vocabulary` since R323, so what this walks is the
    // vocabulary's own claim about which verbs take a pane.
    //
    // ⚠ THE PARSE USED TO BE THE HARD PART and is now four lines. The text it read was a packed
    // `sprag <A | B | C> [-t SESSION]` block, so telling a run of VERBS from one verb's own
    // alternatives needed a bracket-depth walk. One verb per line needs none of that: a verb line
    // is indented four spaces and begins with its own name.
    let usage = sprag(&sock, &["--help"]).stderr;
    let mut verbs: Vec<(String, bool)> = Vec::new();
    for line in usage.lines() {
        let Some(form) = line.strip_prefix("    ") else {
            continue;
        };
        let Some(verb) = form.split_whitespace().next() else {
            continue;
        };
        // The trailing note is indented like a verb line and is not one; a line whose first word
        // is not this vocabulary's verb is skipped rather than guessed at.
        if sprag_host::vocabulary::Verb::parse(verb).is_none() {
            continue;
        }
        if form.contains("PANE") && !verbs.iter().any(|(known, _)| known == verb) {
            verbs.push((verb.to_owned(), form.contains("--pane PANE")));
        }
    }
    assert!(
        !verbs.is_empty(),
        "the usage parse found no pane-taking verb at all: {usage}",
    );

    // Build them all, then step back to window 0 — and prove the fixtures really are ELSEWHERE,
    // because "reaches another window" is trivially true on a one-window daemon, which is the
    // mistake R311's first skew probe made.
    let names: Vec<String> = (0..verbs.len()).map(|n| format!("far-{n}")).collect();
    let ids: Vec<String> = names.iter().map(|name| build(&sock, name)).collect();
    assert!(sprag(&sock, &["select-window", "0", "-t", "0"]).ok);
    let here = sprag(&sock, &["panes", "-t", "0"]).stdout;
    for id in &ids {
        assert!(
            !here.lines().any(|row| row.starts_with(&format!("{id}:"))),
            "window 0 must hold none of the fixtures, or nothing below discriminates: {here}",
        );
    }

    let mut checked: Vec<String> = Vec::new();
    for ((verb, flagged), name) in verbs.iter().zip(&names) {
        let mut args: Vec<&str> = vec![verb];
        if *flagged {
            args.extend(extra(verb));
            args.extend(["--pane", name]);
        } else {
            args.push(name);
            args.extend(extra(verb));
        }
        args.extend(["-t", "0"]);
        let run = sprag(&sock, &args);
        assert!(
            !run.stderr.contains("no pane is called")
                && !run.stderr.contains("must be a number")
                && !run.stderr.contains("is not a pane id")
                && !run.stderr.contains("neither"),
            "`sprag {}` cannot resolve a pane NAME one window over: {}",
            args.join(" "),
            run.stderr,
        );
        // ⚠ **AND IT MUST NOT REFUSE THE ARGUMENTS ITS OWN USAGE SPELLS.** This line appended
        // `-t 0` from the day it was written and asserted only that the NAME resolved — so a verb
        // that rejected the whole command line before resolving anything passed it. `processes`
        // did exactly that from R290: its usage promised `[PANE] [-t SESSION]` and its parser
        // answered `unexpected argument "-t"`, measured against a live daemon at R338 when the new
        // `resources` verb copied the parser along with the usage. A gate that passes on the defect
        // it exists to catch is worse than no gate.
        assert!(
            !run.stderr.contains("unexpected argument"),
            "`sprag {}` refuses an argument its OWN usage line spells: {}",
            args.join(" "),
            run.stderr,
        );
        checked.push(verb.clone());
    }

    // The ratchet is worth nothing if it walked an empty usage, and naming the verbs the
    // measurement found refusing is what makes it a regression pin rather than a shape test.
    for wanted in [
        "capture-pane",
        "send-keys",
        "agent",
        "select-pane",
        "zoom-pane",
        "rename-pane",
        "swap-pane",
        "resize-pane",
        "processes",
        "resources",
        "find",
        "wait-for-output",
    ] {
        assert!(
            checked.iter().any(|verb| verb == wanted),
            "the usage no longer spells a PANE for {wanted}, so this ratchet stopped covering it: \
             {checked:?}",
        );
    }

    // And the usage's own SENTENCE about what a PANE is, checked against what was just measured.
    assert!(
        usage.contains("PANE is a pane's id")
            && usage.contains("Either spelling reaches any WINDOW of the session"),
        "the usage must claim exactly the reach the verbs were just measured to have: {usage}",
    );
}

/// The round's claim end to end at the CLI: an operator NAMES a pane in one window, then works on
/// it from another — writing to it, reading it back, searching it and asking what it runs.
///
/// The middle step is the whole of it. A test that named a pane and read it in the SAME window
/// would pass on the build this round replaces, so the fixture puts the pane one window over and
/// asserts it is not among the caller's own before anything is claimed.
#[test]
fn an_operator_works_on_a_named_pane_in_another_window() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new-window", "-t", "0"]).ok);
    let far = sprag(&sock, &["panes", "-t", "0"])
        .stdout
        .lines()
        .next()
        .and_then(|line| line.split(':').next().map(str::to_owned))
        .expect("the new window's birth pane");
    assert!(sprag(&sock, &["rename-pane", &far, "buildout", "-t", "0"]).ok);
    assert!(sprag(&sock, &["select-window", "0", "-t", "0"]).ok);

    // ⚠ THE FIXTURE MUST DISCRIMINATE — see the ratchet above.
    let here = sprag(&sock, &["panes", "-t", "0"]).stdout;
    assert!(
        !here.lines().any(|row| row.starts_with(&format!("{far}:"))),
        "`buildout` must be in ANOTHER window or nothing below discriminates: {here}",
    );

    // WRITE to it by name, from a window that does not hold it.
    assert!(
        sprag(
            &sock,
            &["send-keys", "buildout", "-l", "R312-MARKER", "-t", "0"]
        )
        .ok
    );
    assert!(sprag(&sock, &["send-keys", "buildout", "Enter", "-t", "0"]).ok);

    // READ it back by name. Polled rather than slept on: the pane's child echoes when it echoes.
    let mut screen = String::new();
    for _ in 0..200 {
        screen = sprag(&sock, &["capture-pane", "buildout", "-t", "0"]).stdout;
        if screen.contains("R312-MARKER") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(
        screen.contains("R312-MARKER"),
        "the write and the read both reached a pane one window over: {screen:?}",
    );

    // SEARCH it by name, and the answer names the pane the caller asked about.
    let found = sprag(
        &sock,
        &["find", "R312-MARKER", "--pane", "buildout", "-t", "0"],
    );
    assert!(found.ok, "find by name: {}", found.stderr);
    assert!(
        found.stdout.contains("R312-MARKER"),
        "and it found the line: {}",
        found.stdout,
    );

    // Ask WHAT IT RUNS by name — a registry-wide reading, narrowed client-side.
    let running = sprag(&sock, &["processes", "buildout"]);
    assert!(running.ok, "processes by name: {}", running.stderr);
    assert!(
        running.stdout.contains(&format!("{far}: ")),
        "and it narrowed to that pane: {}",
        running.stdout,
    );

    // And the CONTROL that says the narrowing is real rather than the whole listing: the caller's
    // OWN pane is not in it. Without this the assertion above passes on a verb that ignores `pane`.
    assert!(
        !running.stdout.contains("\n0: "),
        "the narrowing is real — pane 0 is the caller's own and must not be here: {}",
        running.stdout,
    );

    // A NUMBER still never leaves the caller's window... except that a CLI number is a registry
    // id, which never moves, so it reaches too — and that is the self-contradiction this round
    // removed: `zoom-pane <far>` used to succeed while `capture-pane <far>` refused the same pane.
    let by_id = sprag(&sock, &["capture-pane", &far, "-t", "0"]);
    assert!(by_id.ok, "an id reaches as far as a name: {}", by_id.stderr);
    assert!(by_id.stdout.contains("R312-MARKER"), "{}", by_id.stdout);
    assert!(
        sprag(&sock, &["zoom-pane", &far, "-t", "0"]).ok,
        "and the verb that ALWAYS reached still does — the two agree now",
    );
}

/// Every slot-reading verb explains a daemon that does not serve its address, instead of printing
/// the name of a Rust enum variant at an operator.
///
/// # What this is, and why it took three rounds to exist
///
/// The debt register carried this as *"every other slot-reading CLI verb still leaks a Rust variant
/// name"* from R290, with the note that only the pure fault-to-sentence mapping could be pinned
/// because the suite spawns the CURRENT daemon. [`StaleHost`] is the refutation: the peer is the
/// thing under test's counterpart, not the thing under test, and it can be anything the protocol
/// permits.
///
/// The state is REACHABLE, which is what makes the sentence worth having. A slot is additive, so
/// `WIRE_PROTOCOL` deliberately does NOT rise when one is added (R320's ratchet says so in its own
/// assertion message) — and a client that gained an address therefore meets same-numbered daemons
/// that never had it. `sprag processes` is the lived example: R290 added `pane_processes` under an
/// unchanged number, and this is the sentence its author wrote for exactly this run.
///
/// The verbs are named one by one rather than swept, because a list is a claim a reader can check.
#[test]
fn every_slot_reader_explains_a_daemon_that_does_not_serve_it() {
    let host = stale_host();
    let sock = host.sock().to_path_buf();

    // (argv, the address the verb's FIRST read asks for). The address is asserted too: a sentence
    // that named the wrong slot would still pass a "no variant name" check while sending the
    // reader to the wrong place.
    let readers: &[(&[&str], &str)] = &[
        (&["ls"], "/sprag_mux/external/sessions"),
        (&["panes"], "/sprag_mux/external/panes"),
        (&["layout"], "/sprag_mux/external/layout"),
        (&["windows", "-t", "0"], "/sprag_mux/external/windows"),
        (&["list-clients"], "/sprag_mux/external/clients"),
        (&["agent"], "/sprag_mux/external/agent_manifests"),
        (&["events"], "/sprag_mux/external/events.0"),
        (&["capture-pane", "1"], "/sprag_mux/external/panes"),
        // ⚠⚠⚠ `kill-server` USED TO BE ON THIS LIST, reading `/sprag_mux/external/sessions`, and it
        // is gone because the verb stopped reading anything — see the assertion below this loop,
        // which is what it turned into. Leaving it here would have been asserting that the REMEDY
        // every sentence in this sweep carries is itself unreachable.
        // BOTH SPELLINGS OF `processes`, because the scope decides which read comes first and the
        // sweep is about the FIRST one. A scoped listing has to learn which panes the session holds
        // before it can narrow the reading, so its first read is the window list — `capture-pane`'s
        // shape exactly, and for the same reason. `-a` narrows to nothing, so it goes straight at
        // the verb's own address, which is the pairing this entry was written for: R290 added
        // `pane_processes` under an unchanged `WIRE_PROTOCOL`, and against a daemon that predates
        // only THAT address (rather than this fixture's daemon, which serves nothing) both
        // spellings still name it, because the window list is served.
        (&["processes"], "/sprag_mux/external/windows"),
        (&["processes", "-a"], "/sprag_mux/external/pane_processes.0"),
        // Its sibling, which shares the parse and the narrowing and was never on this list.
        (&["resources"], "/sprag_mux/external/windows"),
        (&["resources", "-a"], "/sprag_mux/external/pane_resources.0"),
        (&["find", "x"], "/sprag_mux/external/panes"),
        (&["select-pane", "1"], "/sprag_mux/external/panes"),
    ];

    for (argv, address) in readers {
        let run = sprag(&sock, argv);
        assert!(!run.ok, "{argv:?} fails against a daemon serving nothing");
        assert!(
            !run.stderr.contains("UnknownIntrospectPath"),
            "{argv:?} must not print a Rust variant name at an operator: {}",
            run.stderr,
        );
        assert!(
            run.stderr
                .contains(&format!("this daemon does not serve {address}")),
            "{argv:?} names the address it could not read: {}",
            run.stderr,
        );
        assert!(
            run.stderr.contains("sprag kill-server"),
            "{argv:?} carries the remedy: {}",
            run.stderr,
        );
    }

    // ⚠⚠⚠ THE REMEDY EVERY SENTENCE ABOVE CARRIES MUST NOT BE BEHIND THE SKEW IT REMEDIES.
    //
    // Each message ends `— sprag kill-server`, and for as long as that verb read a slot it was in
    // the list above: **the advice was one of the things that could not get through.** Measured on
    // the owner's live daemon (2026-08-16) at the harder version of this skew, a PROTOCOL mismatch,
    // where every request including `kill-server` was refused at `client/hello` and the way out was
    // a hand-written script speaking the daemon's older wire.
    //
    // So the claim here is negative and precise: whatever `kill-server` fails for against a peer
    // serving nothing, it is NOT a missing address. It reaches the process instead — and this peer
    // is served by the TEST process, so what it says is the daemon guard, which is the proof that it
    // got past the wire entirely.
    //
    // ⚠ It cannot be asserted positively here: succeeding would mean this test SIGTERM-ing its own
    // harness, which is exactly what the guard exists to refuse and exactly what the first draft of
    // that code did.
    let remedy = sprag(&sock, &["kill-server"]);
    assert!(
        !remedy.stderr.contains("does not serve"),
        "⚠⚠⚠ `kill-server` is the remedy every other sentence in this sweep names. If it fails for a \
         missing ADDRESS then the advice is behind the skew it advises about, which is the defect \
         this assertion exists for: {}",
        remedy.stderr,
    );
    assert!(
        remedy.stderr.contains("not a `sprag-term` daemon"),
        "and what stops it here is the DAEMON guard, not the wire — this peer is served by the test \
         process itself, and a `kill-server` that signalled it would end this harness: {}",
        remedy.stderr,
    );

    // THE ANSWER THAT WAS WRONG RATHER THAN UGLY. Every scoped verb pre-flights through
    // `session_exists`, which read the JSON-RPC code alone — and an unknown ADDRESS arrives under
    // the same `INVALID_PARAMS` a refused SCOPE does. So this verb used to report `no session
    // named "0"` about a session the daemon was holding: a wrong answer that parses, from the
    // check that exists to make addresses trustworthy.
    let scoped = sprag(&sock, &["windows", "-t", "0"]);
    assert!(
        !scoped.stderr.contains("no session named"),
        "an unknown address is not a missing session: {}",
        scoped.stderr,
    );

    // A CALLER INSIDE A PANE gets the same sentence. Working out which session it is standing in
    // needs a slot this peer does not serve either, and that question is one this CLI asked on its
    // own behalf — so its failure must stay invisible and leave the verb's own skew sentence
    // untouched. The arm is otherwise reachable only from a daemon too old to serve the tree.
    let in_a_pane = sprag_env(&sock, &["panes"], &[("SPRAG_PANE", "1")]);
    assert!(!in_a_pane.ok, "it still fails, for the verb's own reason");
    assert!(
        in_a_pane
            .stderr
            .contains("this daemon does not serve /sprag_mux/external/panes"),
        "a scope this CLI could not work out is silent, not an error of its own: {}",
        in_a_pane.stderr,
    );

    // CONTROL 1 — the sentence is not blanket-applied. A verb that fails for its OWN reason keeps
    // its own words, so the assertions above are about the skew path and not about "any failure".
    let own = sprag(&sock, &["send-keys", "-t", "0", "x"]);
    assert!(!own.ok, "the control fails too");
    assert!(
        own.stderr.contains("send-keys needs at least one key name")
            && !own.stderr.contains("this daemon does not serve"),
        "a verb's own refusal is untouched: {}",
        own.stderr,
    );

    // CONTROL 2 — the verbs are not simply broken. The SAME argv against a REAL daemon succeeds,
    // which is what makes the failures above attributable to the peer.
    let (_real_host, real_sock) = spawn_host();
    for argv in [
        &["ls"][..],
        &["panes"][..],
        &["layout"][..],
        &["list-clients"][..],
        &["processes"][..],
    ] {
        let run = sprag(&real_sock, argv);
        assert!(
            run.ok,
            "{argv:?} works against a daemon that serves it: {}",
            run.stderr,
        );
    }
    assert!(
        sprag(&real_sock, &["windows", "-t", "0"]).ok,
        "and the scoped pre-flight still finds a session that IS there",
    );
}

/// A command run INSIDE a pane acts on that pane's session, not on the daemon's default one.
///
/// # The wrong answer, as it was measured
///
/// The daemon tells every pane's child which pane it is (`$SPRAG_PANE`) and nothing read it back.
/// So from a pane of session `work`, on a daemon whose default session was `0`, `sprag panes`
/// listed session 0's panes, `sprag layout` drew session 0, and `sprag split-window` put the new
/// pane in session 0 — reporting success. A person sees their command act on somebody else's
/// session; an AGENT, which is the caller that runs inside a pane and has no other way to know
/// where it is, has no way to see it at all.
#[test]
fn a_command_run_inside_a_pane_acts_on_that_panes_session() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["new", "work"]).ok, "a second session");
    assert!(
        sprag(&sock, &["split-window", "-t", "work"]).ok,
        "so work has two panes and the default session has one",
    );

    // The panes of each session, and the fixture's whole point: they are DIFFERENT panes, so an
    // answer about the wrong session cannot be mistaken for a right one.
    let mine = pane_ids_of(&sock, "work");
    let theirs = pane_ids_of(&sock, "0");
    assert_eq!(theirs.len(), 1, "the default session holds one pane");
    assert_eq!(mine.len(), 2, "work holds two: {mine:?}");
    let (mine_first, mine_second) = (mine[0].clone(), mine[1].clone());

    // A pane of `work` is what this process claims to be running in.
    let inside = [("SPRAG_PANE", mine_first.as_str())];

    // READING: the listing is work's.
    let listed = sprag_env(&sock, &["panes"], &inside);
    assert!(listed.ok, "panes from inside a pane: {}", listed.stderr);
    assert!(
        listed.stdout.contains(&format!("{mine_second}: ")),
        "an unscoped read is about the session the caller is in: {}",
        listed.stdout,
    );
    assert!(
        !listed.stdout.contains(&format!("{}: ", theirs[0])),
        "and NOT about the default session's pane: {}",
        listed.stdout,
    );

    // ACTING: the new pane lands in work.
    let split = sprag_env(&sock, &["split-window"], &inside);
    assert!(split.ok, "split from inside a pane: {}", split.stderr);
    assert_eq!(
        pane_ids_of(&sock, "0").len(),
        1,
        "an unscoped act does not reach into the session the caller is NOT in",
    );
    assert_eq!(
        pane_ids_of(&sock, "work").len(),
        3,
        "it acts where the caller is",
    );

    // CONTROL 1 — the old behaviour is intact for a caller that is NOT in a pane. Without this the
    // test could pass on a CLI that had simply stopped honouring the daemon's default.
    let outside = sprag(&sock, &["panes"]);
    assert!(outside.ok, "panes from a shell: {}", outside.stderr);
    assert!(
        outside.stdout.contains(&format!("{}: ", theirs[0]))
            && !outside.stdout.contains(&format!("{mine_second}: ")),
        "a caller outside any pane still lands in the daemon's default session: {}",
        outside.stdout,
    );

    // CONTROL 2 — an explicit `-t` still wins from inside a pane, in BOTH directions.
    let scoped = sprag_env(&sock, &["panes", "-t", "0"], &inside);
    assert!(
        scoped.stdout.contains(&format!("{}: ", theirs[0]))
            && !scoped.stdout.contains(&format!("{mine_second}: ")),
        "-t names the session, wherever the caller is standing: {}",
        scoped.stdout,
    );

    // CONTROL 3 — a `$SPRAG_PANE` this daemon does not hold is IGNORED, not an error. A pane id
    // outlives the daemon that issued it (ids restart with the process), and the caller of a
    // command has no business being told about somebody else's stale environment.
    let stale = sprag_env(&sock, &["panes"], &[("SPRAG_PANE", "99999")]);
    assert!(
        stale.ok,
        "a stale pane id is not an error: {}",
        stale.stderr
    );
    assert!(
        stale.stdout.contains(&format!("{}: ", theirs[0])),
        "it falls back to the daemon's default: {}",
        stale.stdout,
    );

    // A REGISTRY-WIDE read is not narrowed by the scope it now carries. `ls` answers about every
    // session, so a scope reaching it must change nothing — the arm that would break if the ambient
    // session were applied as a FILTER rather than as a scope.
    let listing = sprag_env(&sock, &["ls"], &inside);
    assert!(listing.ok, "ls from inside a pane: {}", listing.stderr);
    assert!(
        listing.stdout.contains("work") && listing.stdout.contains("0:"),
        "every session is still listed: {}",
        listing.stdout,
    );

    // A SESSION-LEVEL verb still means what it says from inside a pane. `new` reaches an action
    // that creates a session rather than acting within one, and it now travels with a scope it
    // never used to carry — so this drives the arm rather than assuming the daemon ignores it.
    let born = sprag_env(&sock, &["new", "third"], &inside);
    assert!(born.ok, "new from inside a pane: {}", born.stderr);
    assert_eq!(born.stdout.trim(), "third", "and it created what was asked");

    // CONTROL 4 — an id with NO ADDRESS beside it is not ours. Pane ids are per-daemon and start
    // at zero, so a box running two sprag terminals has two pane `1`s: an id inherited without the
    // socket it was published beside names a real, plausible pane of whichever daemon is being
    // asked, and the session it resolved to would be wrong in the one way nobody can see.
    //
    // Driven by reaching this daemon the OTHER way — through `XDG_RUNTIME_DIR`, where the endpoint
    // falls back when the variable is absent — so the CLI still talks to the test's own daemon and
    // never to the machine's (the R278 rule: a probe reaches the endpoint its resolver names).
    let runtime = default_named_runtime_dir(&sock);
    let orphaned = sprag_no_sock(
        &["panes"],
        &[
            (
                "XDG_RUNTIME_DIR",
                runtime.to_str().expect("a utf-8 temp dir"),
            ),
            ("SPRAG_PANE", mine_first.as_str()),
        ],
    );
    assert!(
        orphaned.ok,
        "the CLI still reaches this test's daemon: {}",
        orphaned.stderr,
    );
    assert!(
        orphaned.stdout.contains(&format!("{}: ", theirs[0]))
            && !orphaned.stdout.contains(&format!("{mine_second}: ")),
        "an id with no address beside it is ignored, not trusted: {}",
        orphaned.stdout,
    );

    // CONTROL 5 — the caller's own pane does NOT travel into a session it names. `-t 0` from a pane
    // of `work` must act on session 0's own active pane; substituting the ambient one would address
    // a pane that is not in the named scope at all, which is the same class of wrong answer as the
    // default this test exists for. Driven because it is a BRANCH (the scope filter in
    // `resolve_optional_pane`), and a branch nothing drives is a branch nothing checks.
    let elsewhere = sprag_env(&sock, &["zoom-pane", "-t", "0"], &inside);
    assert!(
        elsewhere.ok,
        "a scoped verb from inside another session's pane: {}",
        elsewhere.stderr,
    );
    assert!(
        elsewhere.stdout.contains(&format!("pane {} ", theirs[0])),
        "it acted on the NAMED session's pane, not on the caller's: {}",
        elsewhere.stdout,
    );
}

/// A verb that takes an OPTIONAL pane and is given none acts on the caller's OWN pane — not on
/// whichever pane of that session somebody else is looking at.
///
/// # The fixture is built so the two readings disagree
///
/// The caller's pane and the session's ACTIVE pane are deliberately different panes here. With one
/// pane, or with the caller sitting on the active one, every assertion below would pass against a
/// CLI that ignored `$SPRAG_PANE` entirely — the vacuous-fixture shape this project keeps catching.
///
/// This is also the discriminator against the rival: `herdr`'s `--current` sends no pane and its
/// daemon falls back to `state.active` + `focused_pane_id()` (`src/app/api/panes.rs`), so two
/// agents in two panes are both answered about whichever pane a human is watching, and neither can
/// address itself. Only `pane current` and `pane split` read the caller's own `HERDR_PANE_ID`.
#[test]
fn a_verb_given_no_pane_acts_on_the_callers_own_pane() {
    let (_host, sock) = spawn_host();
    assert!(sprag(&sock, &["split-window", "-t", "0"]).ok, "two panes");
    let panes = pane_ids_of(&sock, "0");
    assert_eq!(panes.len(), 2, "two panes to tell apart: {panes:?}");
    let (caller, active) = (panes[0].clone(), panes[1].clone());

    // The ACTIVE pane is the other one — the answer a CLI that ignored the caller would give.
    assert!(
        sprag(&sock, &["select-pane", "-t", "0", &active]).ok,
        "the session is left focused on the pane the caller is NOT in",
    );

    // THE CONTROL RUNS FIRST, because it MOVES what it measures: a zoom selects the pane it
    // zoomed, so a control run afterwards would be reading the state the probe had just left.
    // (Measured, not reasoned: written the other way round this read `pane 0` for the shell case
    // and looked like a product defect.)
    let outside = sprag(&sock, &["zoom-pane", "-t", "0"]);
    assert!(outside.ok, "zoom from a shell: {}", outside.stderr);
    assert!(
        outside.stdout.contains(&format!("pane {active} ")),
        "a caller in no pane gets the daemon's own choice — the ACTIVE pane: {}",
        outside.stdout,
    );
    assert!(sprag(&sock, &["zoom-pane", "-u", "-t", "0"]).ok, "unzoom");
    assert!(
        sprag(&sock, &["select-pane", "-t", "0", &active]).ok,
        "and the fixture is put back: the active pane is again the one the caller is NOT in",
    );

    // THE PROBE: the same verb, with no pane named, run from inside the OTHER pane.
    let zoomed = sprag_env(&sock, &["zoom-pane"], &[("SPRAG_PANE", caller.as_str())]);
    assert!(zoomed.ok, "zoom from inside a pane: {}", zoomed.stderr);
    assert!(
        zoomed.stdout.contains(&format!("pane {caller} ")),
        "the verb acted on the caller's own pane, not the focused one: {}",
        zoomed.stdout,
    );

    // CONTROL 2 — and that caller must still SAY which session, because nothing else can place it.
    // The sentence is the one it has always been.
    let unplaceable = sprag(&sock, &["zoom-pane"]);
    assert!(!unplaceable.ok, "a shell with no -t is still refused");
    assert!(
        unplaceable
            .stderr
            .contains("zoom-pane: a target session is required (-t SESSION)"),
        "and refused in the same words as before: {}",
        unplaceable.stderr,
    );
}

/// Run the CLI WITHOUT `SPRAG_HOST_RPC_SOCK`, so the endpoint resolves the other way.
///
/// Every other run here names the socket, which is the guard that keeps this suite off the author's
/// live daemon. The one claim that cannot be made that way is what happens to a pane id inherited
/// with no address beside it — so this reaches the same daemon through `XDG_RUNTIME_DIR` instead,
/// which is where the endpoint falls back. `envs` MUST point that variable at a directory holding
/// this test's own socket.
fn sprag_no_sock(args: &[&str], envs: &[(&str, &str)]) -> CliRun {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_sprag"));
    cmd.args(args).env_remove("SPRAG_HOST_RPC_SOCK");
    // ⚠⚠⚠⚠⚠ **THIS HELPER IS THE ONE THAT WRITES, PRECISELY BECAUSE IT REACHES NO DAEMON.** A
    // reporter that cannot reach one leaves a breadcrumb at
    // `$XDG_STATE_HOME/sprag/hook-mute.<pane>` (`hooks::note_mute`) — *"the daemon is by
    // definition unreachable when this is written"* — so the very condition these gates construct
    // is the condition that files something. With no state home of its own that landed in the
    // runner's real `~/.local/state`, and it was the last residue CI's `ambient-home-guard`
    // reported once register item 464 removed the review ledger: bisected to
    // `state/sprag/hook-mute.0` on 2026-08-19.
    //
    // ⚠ Its own directory, removed once the CLI has exited — `output()` has already waited, so
    // nothing is still writing here. Before `envs`, so a caller naming its own still wins.
    let state = scratch_state_home();
    cmd.env("XDG_STATE_HOME", &state);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    let output = cmd.output().expect("run the sprag CLI");
    let _ = std::fs::remove_dir_all(&state);
    CliRun {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        ok: output.status.success(),
    }
}

/// A directory holding a link to `sock` under the WELL-KNOWN name, for [`sprag_no_sock`] to point
/// `XDG_RUNTIME_DIR` at. A hard link rather than a copy: a socket is not a file whose bytes mean
/// anything, and the link names the same inode the daemon is listening on.
fn default_named_runtime_dir(sock: &Path) -> PathBuf {
    // ⚠ SHORT ON PURPOSE — a unix socket's path has a HARD platform ceiling (`sun_path`: 104 bytes
    // on macOS, 108 on Linux) and what goes under this directory is a socket. This used to embed the
    // whole socket FILE NAME for uniqueness, 44 characters of it, which is invisible under Linux's
    // `/tmp/` and fatal under macOS's `/var/folders/<two opaque components>/T/`: **measured at 109
    // bytes, five over.** The connect then failed and the CLI reported *"no server running"*, which
    // is true and useless — it sends a person looking for a daemon that is running.
    //
    // A per-CALL counter gives the same uniqueness in three characters. The pid keeps two test
    // BINARIES apart, the counter keeps two threads of one binary apart.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("sprag-rt-{}-{n}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    let link = dir.join(sprag_rpc::HOST_SOCKET_NAME);
    let _ = std::fs::remove_file(&link);
    std::fs::hard_link(sock, &link).expect("link the test daemon's socket under the default name");
    dir
}

/// The pane ids a session holds, in the order `sprag panes` lists them.
fn pane_ids_of(sock: &Path, session: &str) -> Vec<String> {
    let run = sprag(sock, &["panes", "-t", session]);
    assert!(run.ok, "panes -t {session}: {}", run.stderr);
    run.stdout
        .lines()
        .filter_map(|line| line.split(':').next())
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect()
}

/// Every ACTING verb explains a daemon that does not know its verb, instead of blaming the
/// arguments the user typed.
///
/// # Why this is the sharper half of the skew
///
/// The reading half ([`every_slot_reader_explains_a_daemon_that_does_not_serve_it`]) printed a Rust
/// variant name at an operator: ugly, and honest about having failed. The acting half is worse,
/// because a refused INVOKE and a refused REQUEST arrive under one JSON-RPC code — so a verb that
/// maps every fault to its own disjunction tells the user their WINDOW NAME is taken, or their pane
/// is not there, when the truth is that their daemon predates the verb. R317 measured exactly that
/// against a parent-commit daemon: `sprag rename-session` answered *"prod" is already another
/// session's name* about a name no session held.
///
/// The state is REACHABLE for the same reason the reading one is: an action is additive, so
/// `WIRE_PROTOCOL` does not rise when one is added, and a `sprag` that gained a verb meets
/// same-numbered daemons that lack it.
#[test]
fn every_acting_verb_explains_a_daemon_that_does_not_know_its_verb() {
    let (host, _daemon, real) = aged_host();
    let sock = host.sock().to_path_buf();

    // PREPARE through the daemon itself: a second pane and a second window, so the verbs that
    // address one are refused by the PEER rather than by their own client-side reads.

    assert!(
        sprag(&real, &["split-window", "-t", "0"]).ok,
        "the fixture's second pane",
    );
    assert!(
        sprag(&real, &["new-window", "-t", "0", "-d", "spare"]).ok,
        "the fixture's second window",
    );
    let panes = sprag(&real, &["panes", "-t", "0"]);
    assert!(panes.ok, "the fixture reads back: {}", panes.stderr);
    let ids: Vec<String> = panes
        .stdout
        .lines()
        .filter_map(|line| line.split(':').next())
        .map(str::trim)
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
        .collect();
    assert!(ids.len() >= 2, "two panes to address: {}", panes.stdout);
    let (first, second) = (ids[0].as_str(), ids[1].as_str());

    // (argv, the address the verb's act asks for). The address is asserted too, for the reading
    // sweep's reason: a sentence naming the WRONG act would pass a "no variant name" check while
    // sending the reader somewhere else. Every one is DERIVED from the wire vocabulary rather than
    // spelled, so a verb that is re-pointed at another action moves this table with it.
    let mux = |action: &str| mux_action_path(action);
    let acting: Vec<(Vec<&str>, String)> = vec![
        (vec!["new", "work"], mux(NEW_SESSION_ACTION)),
        (
            vec!["new-window", "-t", "0", "extra"],
            mux(NEW_WINDOW_ACTION),
        ),
        (
            vec!["select-window", "-t", "0", "-n"],
            mux(SELECT_WINDOW_ACTION),
        ),
        (
            vec!["rename-window", "-t", "0", "renamed"],
            mux(RENAME_WINDOW_ACTION),
        ),
        (
            vec!["move-window", "-t", "0", "--first"],
            mux(MOVE_WINDOW_ACTION),
        ),
        (vec!["kill-window", "-t", "0"], mux(KILL_WINDOW_ACTION)),
        (
            vec!["resize-window", "-t", "0", "-x", "80", "-y", "24"],
            mux(RESIZE_WINDOW_ACTION),
        ),
        (vec!["split-window", "-t", "0"], mux(SPAWN_ACTION)),
        (vec!["kill-pane", "-t", "0", second], mux(CLOSE_ACTION)),
        // BOTH of `resize-pane`'s arms, and they reach DIFFERENT actions — the exact size is the
        // pane's own `resize`, the boundary walk is the mux's `resize_pane`. The boundary one was
        // the only acting arm in the tree that already told the skew apart (R297 wrote it after
        // measuring this exact wrong answer), so it is this sweep's own positive case: an assertion
        // nothing could satisfy would pass the list by being impossible rather than by being met.
        (
            vec!["resize-pane", "-t", "0", second, "-x", "40", "-y", "10"],
            mux(RESIZE_ACTION),
        ),
        (
            vec!["resize-pane", "-t", "0", second, "-L", "3"],
            mux(RESIZE_PANE_ACTION),
        ),
        (
            vec!["select-pane", "-t", "0", second],
            mux(SELECT_PANE_ACTION),
        ),
        (
            vec!["swap-pane", "-t", "0", first, second],
            mux(SWAP_PANE_ACTION),
        ),
        (
            vec!["move-pane", "-t", "0", second, "-h", first],
            mux(MOVE_PANE_ACTION),
        ),
        (
            vec!["break-pane", "-t", "0", second],
            mux(BREAK_PANE_ACTION),
        ),
        (
            vec!["join-pane", "-t", "0", second, "spare"],
            mux(JOIN_PANE_ACTION),
        ),
        (vec!["zoom-pane", "-t", "0", first], mux(ZOOM_PANE_ACTION)),
        (
            vec!["rename-pane", "-t", "0", first, "named"],
            mux(RENAME_PANE_ACTION),
        ),
        // The one act that is not a MUX action: a key goes to the pane's own input surface.
        (
            vec!["send-keys", "-t", "0", first, "x"],
            pane_input_path(first.parse().expect("a pane id"), KEY_ACTION),
        ),
        (
            vec!["rename-session", "-t", "0", "prod"],
            mux(RENAME_SESSION_ACTION),
        ),
        (
            vec!["display-message", "-t", "0", "hello"],
            mux(DISPLAY_MESSAGE_ACTION),
        ),
        (
            vec!["report-agent", "blocked", "-t", "0", "--pane", first],
            mux(REPORT_AGENT_ACTION),
        ),
        (
            vec!["release-agent", "-t", "0", "--pane", first],
            mux(RELEASE_AGENT_ACTION),
        ),
        (vec!["kill-session", "0"], mux(KILL_SESSION_ACTION)),
    ];

    // Collected rather than asserted one at a time: the first failure is not the finding, the LIST
    // is, and a sweep that stops at the first leak cannot say how wide the class is. Measured
    // before the seam existed, this list came back TWENTY-ONE of twenty-four.
    let mut wrong = Vec::new();
    for (argv, address) in &acting {
        let run = sprag(&sock, argv);
        if run.ok {
            wrong.push(format!(
                "{argv:?} SUCCEEDED against a daemon that refused the action"
            ));
            continue;
        }
        if run.stderr.contains("UnknownInvokePath") {
            wrong.push(format!(
                "{argv:?} printed the variant name: {}",
                run.stderr.trim()
            ));
        } else if !run.stderr.contains(&format!("does not perform {address}")) {
            wrong.push(format!(
                "{argv:?} did not name {address} — it said: {}",
                run.stderr.trim()
            ));
        } else if !run.stderr.contains("sprag kill-server") {
            wrong.push(format!("{argv:?} carries no remedy: {}", run.stderr.trim()));
        }
    }
    assert!(
        wrong.is_empty(),
        "an acting verb must say the daemon is older than this `sprag`:\n  {}",
        wrong.join("\n  "),
    );

    // NOT IN THE LIST, and the reason is a property worth pinning rather than an omission:
    // `set-option` writes the user's `config.toml` and never asks the daemon at all, so it is the
    // one verb that reads as an act and cannot meet this skew. It succeeds here, which is correct.
    //
    // ⚠ IT GETS A CONFIG HOME OF ITS OWN, and that is not tidiness. This line ran against the
    // AMBIENT one, so `cargo test` wrote `window-size = "manual"` into the config file of whoever
    // ran the suite — measured: one run of this test alone creates `$XDG_CONFIG_HOME/sprag/
    // config.toml`, and with the variable unset that is `~/.config/sprag/config.toml`. It is how
    // this developer's own file came to exist, which is in turn how a unit test of `resize_window`
    // came to be gated against a policy that is not the default (R341). A test that WRITES the
    // developer's config is the other half of the rule about tests that read it.
    let own = ConfigHome::new("");
    assert!(
        sprag_env(
            &sock,
            &["set-option", "window-size", "manual"],
            &[("XDG_CONFIG_HOME", own.as_str())],
        )
        .ok,
        "a verb that acts on the CONFIG FILE is not a verb the daemon can be too old for",
    );

    // CONTROL 1 — the sentence is not blanket-applied: a verb that fails for its OWN reason keeps
    // its own words, so the sweep is about the skew path and not about "any failure".
    let own = sprag(&sock, &["send-keys", "-t", "0", first]);
    assert!(!own.ok, "the control fails too");
    assert!(
        own.stderr.contains("send-keys needs at least one key name")
            && !own.stderr.contains("does not know this verb"),
        "a verb's own refusal is untouched: {}",
        own.stderr,
    );

    // CONTROL 2 — the verbs are not simply broken. The same argv against the daemon BEHIND the
    // proxy succeeds, which is what makes every failure above attributable to the peer.
    for argv in [
        &["rename-window", "-t", "0", "renamed"][..],
        &["display-message", "-t", "0", "hello"][..],
        &["zoom-pane", "-t", "0", first][..],
    ] {
        let run = sprag(&real, argv);
        assert!(
            run.ok,
            "{argv:?} works against the daemon that knows it: {}",
            run.stderr,
        );
    }
}

/// **THE RATCHET: every verb the vocabulary has, driven at the SHIPPED BINARY.**
///
/// Register item 48 asked for a sweep that a newly added verb enters by itself, and this is the
/// half no library test can make: `sprag_host::vocabulary` is a table, and a table can say anything.
/// What the table CLAIMS is that this binary dispatches 49 words and refuses five others by name —
/// so every one of them is run, and the answers are held against the claim.
///
/// # The three answers, and the control
///
/// * a verb the table says the shell RUNS must not come back `unknown command` — it may fail for
///   any reason of its own (there is no daemon on this socket, which most of them need);
/// * a verb the table says is only ever a KEYSTROKE must be refused by NAME, with the line that
///   would bind it — where until R323 `sprag switch-client -n` answered `unknown command
///   "switch-client"` about a verb this product has had since R314;
/// * a verb the table says the shell has NO FORM FOR YET must be refused by NAME too, and must name
///   the mouth that does have it — R335's three, which reach an AI agent and no shell. This is the
///   answer `Option<Shell>` could not spell: they are not keystrokes and they are not typos;
/// * a word that is in no vocabulary is `unknown command`, which is the CONTROL: without it, a
///   binary that answered every word the same way would satisfy the first rule completely.
///
/// The EXIT CODES discriminate too, and they are deliberately different: an unknown word exits 2
/// (tmux's usage exit), an argument error exits 1. A refusal that merged them would tell a script
/// that a real verb was a typo.
#[test]
fn every_verb_the_vocabulary_names_is_one_this_binary_answers_for() {
    use sprag_host::vocabulary::{Shell, Verb};
    let absent = socket_path();
    let mut ran = 0_usize;
    let mut refused = 0_usize;
    let mut unbuilt = 0_usize;
    for verb in Verb::ALL {
        let name = verb.name();
        let run = sprag(&absent, &[name]);
        match verb.entry().shell {
            Shell::Runs(_) => {
                ran += 1;
                assert!(
                    !run.stderr.contains("unknown command"),
                    "{name:?} is a verb this binary dispatches and it answered: {}",
                    run.stderr,
                );
            }
            Shell::Cannot(_) => {
                refused += 1;
                assert!(
                    run.stderr.contains(name)
                        && run.stderr.contains("is a key binding, not a command")
                        && run.stderr.contains("bind-key"),
                    "{name:?} must be refused as the keystroke it is: {}",
                    run.stderr,
                );
            }
            Shell::NotBuilt => {
                unbuilt += 1;
                // It says the act EXISTS and where to reach it — the difference between this and
                // `unknown command`, and the whole reason the third answer was added.
                assert!(
                    run.stderr.contains(name)
                        && run.stderr.contains("has no shell command yet")
                        && verb.tools().iter().all(|tool| run.stderr.contains(tool)),
                    "{name:?} must name itself and the tool that has it: {}",
                    run.stderr,
                );
                assert!(
                    !run.stderr.contains("unknown command"),
                    "{name:?} is a verb this product HAS: {}",
                    run.stderr,
                );
            }
        }
    }
    assert_eq!(
        (ran, refused, unbuilt),
        // R353: `show-grammar` is the 53rd — the CLI door onto the wire's own call grammar.
        // R355: three more, and they are the door onto the LOOP the README leads with. Each was
        // DRIVEN here before the count moved: the binary answers for `orchestrate`, `runs` and
        // `cancel-run` against a live daemon, which is what this sweep is for.
        // ⚠ R369: `answer-pane` is the 58th, and it was DRIVEN here before the count moved —
        // against a live daemon, addressing a pane one window over by NAME, in the sweep above.
        // ⚠ `stand-down` is the 59th, and it is DRIVEN here before the count moves — the sweep
        // above runs it against a live daemon, which is what makes this a claim about the binary
        // rather than about the table.
        // R(item 9): `hold-run` and `resume-run` — the third thing a person may say to a run, and
        // the only one they can take back. Both are shell verbs, so both land in the first column.
        (61, 5, 3),
        "the shell half, the keyboard-only half, and the acts no shell spells yet",
    );

    // THE CONTROL — a word in no vocabulary, and the exit code that says so. Without it every
    // assertion above is satisfied by a binary that never says `unknown command` at all.
    let nonsense = sprag(&absent, &["kill-serverr"]);
    assert!(
        nonsense.stderr.contains("unknown command \"kill-serverr\""),
        "a word that is no verb is a typo: {}",
        nonsense.stderr,
    );
    assert!(!nonsense.ok, "and it fails");

    // ...and the two failures are told apart by their exit code, not only by their words.
    let keystroke = std::process::Command::new(env!("CARGO_BIN_EXE_sprag"))
        .arg("switch-client")
        .env("SPRAG_HOST_RPC_SOCK", &absent)
        .output()
        .expect("run the sprag CLI");
    let typo = std::process::Command::new(env!("CARGO_BIN_EXE_sprag"))
        .arg("kill-serverr")
        .env("SPRAG_HOST_RPC_SOCK", &absent)
        .output()
        .expect("run the sprag CLI");
    assert_eq!(
        (keystroke.status.code(), typo.status.code()),
        (Some(1), Some(2)),
        "a real verb refused is an argument error; a word nobody has is the usage exit",
    );
}

/// **THE HELP NAMES EVERY VERB THIS BINARY DISPATCHES** — the drift that opened R323, measured.
///
/// The usage text was a hand-written `const` whose OWN DOC said *"a second list is exactly what
/// nothing checks"*. It was right, and nobody had checked: `run` and `hook` were dispatched by the
/// shipped binary and named in it nowhere, so two of the CLI's verbs could be found only by reading
/// its source.
///
/// **This is deliberately not a comparison against the table** — the help is BUILT from the table
/// now, so that assertion would be circular and would pass on a table missing a verb. It reads the
/// help the binary PRINTS, and drives every word it finds there back at the binary.
#[test]
fn the_help_names_the_verbs_and_every_word_it_names_is_dispatched() {
    let absent = socket_path();
    let help = sprag(&absent, &["--help"]).stderr;
    let named: Vec<String> = help
        .lines()
        .filter_map(|line| line.strip_prefix("    "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|word| word.chars().all(|c| c.is_ascii_lowercase() || c == '-'))
        .map(str::to_owned)
        .collect();
    // The two the const had lost, named here rather than counted: this test exists because of
    // them, and a regression that dropped one again must fail by name.
    for lost in ["run", "hook"] {
        assert!(
            named.iter().any(|word| word == lost),
            "{lost:?} is dispatched and the help must name it: {help}",
        );
    }
    assert!(
        named.len() >= 49,
        "the help lists {} verbs, which is fewer than this binary had at R323: {help}",
        named.len(),
    );
    for verb in &named {
        let run = sprag(&absent, &[verb]);
        assert!(
            !run.stderr.contains("unknown command"),
            "the help names {verb:?} and the binary does not have it: {}",
            run.stderr,
        );
    }
}

/// **`bind-key` ANSWERS FOR EVERY VERB THE PRODUCT HAS** — the head of R323, at the surface a user
/// meets it.
///
/// R322 measured this by hand and wrote the number down: of the 47 verbs the CLI dispatched,
/// `sprag bind-key F9 <verb>` took 8 outright, 6 more with flags, and told the other 33 that the
/// verb *"is not an action"* — the sentence a TYPO gets, about words the same binary runs. This
/// asserts the whole sweep instead, so the number is derived and a new verb enters it by existing.
///
/// Each of the four answers is a different sentence, and which one a verb gets is the table's claim:
/// bound, refused for its FLAGS (a real verb, wrong grammar), refused with a RULE, or named as a
/// gap sprag has not closed. The CONTROL is a word that is no verb at all — it must still read as
/// a typo, which is what stops "every refusal now has a reason" from meaning "every refusal now
/// reads alike".
#[test]
fn bind_key_answers_for_every_verb_in_the_words_the_table_promises() {
    use sprag_host::vocabulary::{Keystroke, Verb};
    let absent = socket_path();
    let config = ConfigHome::new("");
    let bind = |name: &str| {
        sprag_env(
            &absent,
            &["bind-key", "F9", name],
            &[("XDG_CONFIG_HOME", config.as_str())],
        )
    };
    let mut counts = (0_usize, 0_usize, 0_usize, 0_usize);
    for verb in Verb::ALL {
        let name = verb.name();
        let run = bind(name);
        match verb.keystroke() {
            // A form is a GRAMMAR: `split-window` alone is a real verb with an incomplete flag
            // list, and its refusal must be about the FLAGS rather than about the verb.
            Keystroke::Means(_) => {
                if run.ok {
                    counts.0 += 1;
                } else {
                    counts.1 += 1;
                    assert!(
                        run.stderr.contains(name)
                            && !run.stderr.contains("is not an action")
                            && !run.stderr.contains("not a binding"),
                        "{name:?} is bindable, so a refusal is about its flags: {}",
                        run.stderr,
                    );
                }
            }
            Keystroke::Cannot(why) => {
                counts.2 += 1;
                assert!(!run.ok, "{name:?} must be refused");
                assert!(
                    run.stderr.contains("is a command, not a binding")
                        && run.stderr.contains(why.why())
                        && run.stderr.contains(&format!("`sprag {name}`")),
                    "{name:?} must be refused with its own rule and the way to run it: {}",
                    run.stderr,
                );
            }
            Keystroke::NotBuilt => {
                counts.3 += 1;
                assert!(!run.ok, "{name:?} must be refused");
                assert!(
                    run.stderr.contains("does not bind it yet"),
                    "{name:?} is a gap of sprag's, and the sentence must say so: {}",
                    run.stderr,
                );
            }
        }
    }
    // MEASURED, not predicted: the author's arithmetic said 16/6 and the run said 14/8, because
    // `switch-client` and `confirm-before` are bindable verbs whose BARE form is incomplete — the
    // same shape as `split-window`. The numbers are here so a verb changing category has to change
    // one.
    // R328 moved `move-pane` from the fourth column to the SECOND, which is the shape of closing
    // one of these: a verb sprag had not built becomes a verb whose BARE form is incomplete, like
    // `split-window` beside it. Bare `move-pane` needs `-h` or `-v`.
    // R329 moved `join-pane` from the fourth to the FIRST, which is the other shape: a join names
    // no axis, so its BARE form is the whole verb and `sprag bind-key F9 join-pane` is accepted
    // outright. The two together are what the four columns are for — closing a gap can land in
    // either of the bindable ones and only the count says which.
    // R331 moved `resize-window` from the fourth to the SECOND, `move-pane`'s shape: a bare resize
    // has named no rectangle, and reading it as the un-pin would throw a decision away on an empty
    // config line — so the verb is bindable and its BARE form is refused with the sizes it takes.
    // R335 added THREE to the third column at once, and none of them moved: `read-last-command`,
    // `pane-links` and `pane-images` are acts the product performs for an AI agent, and they enter
    // this sweep as refusals with a rule (`it answers with text`) the moment the vocabulary names
    // them — which is the whole reason a new verb is added to the TABLE rather than to a surface.
    assert_eq!(
        counts,
        // R353: `show-grammar` ANSWERS something, so it is refused with a rule like `doctor`.
        // R355 lands in BOTH remaining columns at once, which is what the four are for:
        // `orchestrate` (its whole content is the words) and `runs` (it answers) join the third,
        // and `cancel-run` is the SECOND entry in the fourth — a key could mean "cancel the runs I
        // started" and nobody has built that verb, which is a gap rather than a refusal.
        // ⚠ R369: `answer-pane` joins the THIRD column — refused with a rule. Its whole content
        // is the two needles a caller quotes off a dialog they just read, so a binding would fix
        // one question and one option forever, which is the one shape a consent must never take.
        // ⚠ `stand-down` is the FOURTH in the last column, beside `cancel-run` and for its reason:
        // "stand down every run I started" is an act a key could mean and nobody has built it.
        // R(item 9): `hold-run` and `resume-run` join the fourth column. A key naming no run cannot
        // give an order about one — and `hold-run` is the best candidate of the three for a binding,
        // because *stop so I can read this* is the order somebody wants to give without leaving the
        // pane they are looking at.
        (15, 10, 38, 6),
        "bound outright / refused for flags / refused with a rule / not built yet",
    );

    // THE CONTROL: a word that is in no vocabulary still reads as a typo, and the list it is
    // offered is the DERIVED one — `break-pane` is in it because the table says so.
    let typo = bind("kill-serverr");
    assert!(!typo.ok);
    assert!(
        typo.stderr.contains("is not an action") && typo.stderr.contains("break-pane"),
        "a word nobody has is a typo, answered with what exists: {}",
        typo.stderr,
    );
}

/// **THE GATE for R325: a refused act is answered with the ONE fact the daemon observed, and no
/// client anywhere writes a list of causes.**
///
/// # What this replaces, measured at `87cde88` before a line was written
///
/// Every acting verb answered a refusal with a client-side DISJUNCTION, because
/// `InvokeError::Rejected` was a payload-free variant and the reason the daemon already held could
/// not cross the wire (register item 9, filed upstream as PINION-PR82). Driven at this binary
/// against an isolated daemon, four survived to the end and each named more causes than were true:
///
/// | verb | what an operator read | causes | true |
/// |---|---|---|---|
/// | `rename-session` | *"is already another session's name, or is blank, over 80 bytes, or contains a control character"* | 4 | 1 |
/// | `break-pane` | *"is its window's only pane, no window holds it, or the name is taken"* | 3 | 1 |
/// | `join-pane` | *"no window named x, no pane N, or it already lives there"* | 3 | 1 |
/// | `rename-window` | *"window not found, or \"0\" is already taken"* | 2 | 1 |
///
/// PINION-PR82 landed upstream (pinion R1564, `a refused invoke states why`); R325 bumped the pin
/// onto it, made every one of this daemon's ninety-odd refusal sites state its fact, and deleted
/// the guesses.
///
/// # Why the disjunction check is the load-bearing half
///
/// Pinning the four sentences alone would pass on a build that printed the right words AND kept a
/// guess beside them. What cannot survive the return of a client-side list is the second assertion
/// in each block: the refusal contains no `" or "`. That is the property the round is about, and it
/// is checked on the STDERR a person reads rather than on any function's return type.
///
/// # And one of these sentences was WRONG when it first became visible
///
/// `rename-window` onto a taken name answered *a **session** named "0" already exists* — the
/// registry used `SessionError::Duplicate` for every window-level clash, the exact collapse
/// `SessionError::MalformedWindow`'s own doc forbids two variants over. It had been invisible for
/// as long as the CLI was overwriting it. `DuplicateWindow` / `UnknownWindow` / `UnknownAnchor`
/// exist because making a sentence reach a person is what tests it.
#[test]
fn a_refusal_is_the_one_fact_the_daemon_observed() {
    let (_host, sock) = spawn_host();
    let session = "0";

    // Two windows and a spare session, so every refusal below is reached with its pre-flight
    // PASSED — the state that could only ever be described by the daemon.
    assert!(sprag(&sock, &["new-window", "-t", session]).ok);
    assert!(sprag(&sock, &["new", "beta"]).ok);
    let pane = sprag(&sock, &["panes", "-t", session]).stdout;
    let pane = pane
        .lines()
        .next()
        .and_then(|row| row.split(':').next())
        .expect("a pane row")
        .to_owned();

    // (the command, the ONE fact, what the guess used to also say)
    let cases: [(&[&str], &str, &str); 4] = [
        (
            &["rename-window", "-t", "0", "1", "0"],
            "a window named \"0\" already exists",
            // The guess said "window not found" too — and, once visible, the sentence itself said
            // SESSION about a window.
            "not found",
        ),
        (
            &["rename-session", "-t", "0", "beta"],
            "a session named \"beta\" already exists",
            "over 80 bytes",
        ),
        (
            &["break-pane", "-t", "0"],
            "cannot break the only pane in a window",
            "the name is taken",
        ),
        (
            &["join-pane", "-t", "0", "ghostwin"],
            "no window named \"ghostwin\"",
            "already lives there",
        ),
    ];
    for (args, fact, retired) in cases {
        // `break-pane` / `join-pane` take the pane id this daemon happened to mint, so it is read
        // rather than assumed — R295's rule about positions applies to ids a fixture guesses too.
        let mut argv: Vec<&str> = args.to_vec();
        if matches!(args[0], "break-pane" | "join-pane") {
            argv.insert(1, &pane);
        }
        let run = sprag(&sock, &argv);
        assert!(!run.ok, "{argv:?} must be refused: {}", run.stdout);
        assert_eq!(
            run.stderr.trim(),
            format!("sprag: {fact}"),
            "{argv:?} is answered with the fact the daemon observed",
        );
        // THE LOAD-BEARING HALF: no list of causes, and none of the retired ones.
        assert!(
            !run.stderr.contains(" or "),
            "{argv:?} must not offer a disjunction: {:?}",
            run.stderr,
        );
        assert!(
            !run.stderr.contains(retired),
            "{argv:?} must not name {retired:?}, which was never true here: {:?}",
            run.stderr,
        );
    }

    // THE CONTROL, on the same daemon at the same instant: a well-formed act still WORKS, so the
    // four refusals above are about their arguments rather than about a build that refuses
    // everything. Without it, a `sprag` that failed every invoke would pass every line above.
    let worked = sprag(&sock, &["rename-window", "-t", "0", "1", "spare"]);
    assert!(worked.ok, "the control must succeed: {}", worked.stderr);
}

/// **A daemon that refuses and says NOTHING is reported as the SKEW it is** — the degradation the
/// gate above leaves, and the reason the ten guesses could be deleted rather than kept as a
/// fallback.
///
/// On this build a refusal cannot be anonymous (`InvokeError::rejected` requires the sentence), so
/// a bare one means a daemon older than the build that made it mandatory. Driven against
/// [`sprag_peer`]'s proxy, which relays every read to a real daemon and answers the invoke itself —
/// the only shape that reaches an acting path at all (R322/R324).
///
/// What must NOT appear is `InvokeRejected`, a Rust variant name: that was register item 9's
/// original leak, and deleting the guesses re-opened the hole for exactly this peer until
/// `wire::unstated_refusal` closed it.
#[test]
fn a_refusal_that_states_nothing_is_reported_as_an_old_daemon() {
    let (_host, upstream) = spawn_host();
    let peer = refusing_peer(&upstream);
    let run = sprag(peer.sock(), &["kill-window", "-t", "0"]);
    assert!(!run.ok, "a refusal is a failure: {}", run.stdout);
    assert!(
        !run.stderr.contains("InvokeRejected"),
        "a Rust variant name must not reach a person: {}",
        run.stderr,
    );
    assert!(
        run.stderr.contains("did not say why") && run.stderr.contains("Restart"),
        "it is told as the skew it is, with the remedy: {}",
        run.stderr,
    );
}

/// ⚠⚠ **THE DOOR ONTO THE WIRE'S GRAMMAR ASKS THE DAEMON** — `sprag show-grammar`, against a real one.
///
/// # What is worth asserting here, and what is not
///
/// The TABLE's honesty is held by four property gates in the crate and by an end-to-end drive in
/// `wire_client`. This is about the DOOR: that the verb reaches the daemon, that it prints the two
/// surfaces separately, and that it names what it publishes when asked for something it does not.
///
/// ⚠ The last one is the difference between this and the closest rival's `herdr api schema`, which
/// prints a JSON Schema a test wrote into its docs and the binary `include_str!`'d — a document about
/// the build the CLI came from, with no method among its ninety-one returning it. The proof that this
/// one asks the DAEMON is [`the_cli_reports_a_stale_daemons_grammar_as_skew`](self) below: point it at
/// a daemon that serves nothing and it fails, where a compiled-in document could not have noticed.
#[test]
fn the_cli_prints_how_to_call_the_verbs_the_daemon_publishes() {
    let (_host, sock) = spawn_host();

    // THE MULTIPLEXER's surface, narrowed to one verb — the question a person actually has.
    let run = sprag(&sock, &["show-grammar", SELECT_PANE_ACTION]);
    assert!(run.ok, "show-grammar failed: {}", run.stderr);
    let lines: Vec<&str> = run.stdout.lines().collect();
    assert_eq!(lines[0], SELECT_PANE_ACTION);
    assert!(
        lines
            .iter()
            .filter(|line| line.trim() == "form object")
            .count()
            == 2,
        "`select_pane` takes a pane OR a direction, and the two forms print as two: {}",
        run.stdout,
    );
    assert!(
        run.stdout.contains("dir") && run.stdout.contains("one of: left, right, up, down"),
        "an argument with a closed vocabulary prints the words a caller may send: {}",
        run.stdout,
    );

    // THE PANE's surface — a different table at a different address, which is the whole reason the
    // flag exists rather than one merged answer.
    let pane = sprag(&sock, &["show-grammar", KEY_ACTION, "--pane"]);
    assert!(pane.ok, "show-grammar --pane failed: {}", pane.stderr);
    assert!(
        pane.stdout.contains("form scalar") && pane.stdout.contains("form object"),
        "`key` takes a bare string or an object, and the shapes print: {}",
        pane.stdout,
    );
    assert!(
        pane.stdout.contains("one of: down, up"),
        "the key edge's vocabulary is published: {}",
        pane.stdout,
    );
    // ...and the two surfaces are NOT each other: the multiplexer knows nothing about `key`.
    let wrong_surface = sprag(&sock, &["show-grammar", KEY_ACTION]);
    assert!(
        !wrong_surface.ok && wrong_surface.stderr.contains("publishes no grammar"),
        "a pane verb is not on the multiplexer's surface, and the refusal says what is: {}",
        wrong_surface.stderr,
    );

    // A verb the daemon serves and deliberately does not describe — `set_layout` takes an arrangement
    // TREE, which a flat argument grammar cannot state. The refusal NAMES what is published, because
    // the commonest reason to be here is not knowing the spelling.
    let nested = sprag(&sock, &["show-grammar", SET_LAYOUT_ACTION]);
    assert!(!nested.ok);
    assert!(
        nested.stderr.contains("publishes no grammar") && nested.stderr.contains(SPLIT_ACTION),
        "the refusal lists the verbs that DO publish: {}",
        nested.stderr,
    );

    // The whole surface, unnarrowed: every verb the daemon publishes, and nothing else.
    let all = sprag(&sock, &["show-grammar"]);
    assert!(all.ok, "show-grammar failed: {}", all.stderr);
    let verbs: Vec<&str> = all
        .stdout
        .lines()
        .filter(|line| !line.starts_with(' '))
        .collect();
    assert_eq!(
        verbs.len(),
        29,
        "the multiplexer publishes all but ONE of its verbs — `resize` and `grant_pane` were \
         exempted as NESTED values and are flat (R355b), leaving `set_layout`'s arrangement tree \
         as the only one whose reason survived being re-derived. ⚠ The newest is `respawn` (item \
         557), and it publishing here is the point: a driver outside the daemon learns to spell \
         the verb that rolls its session from the daemon rather than from folklore: {verbs:?}",
    );
}

/// ⚠ **A DAEMON THAT DOES NOT PUBLISH ITS GRAMMAR IS REPORTED AS SKEW, NOT ANSWERED FROM THIS BUILD.**
///
/// This is the claim that makes `show-grammar` a wire verb rather than a document: the answer comes
/// from the daemon on the other end of the socket, so a daemon too old to serve the slot cannot be
/// papered over by the CLI's own copy of the table. A compiled-in schema — the rival's shape — would
/// print happily here and be wrong about the thing the operator is debugging.
#[test]
fn the_cli_reports_a_stale_daemons_grammar_as_skew() {
    let stale = stale_host();
    let run = sprag(stale.sock(), &["show-grammar"]);
    assert!(
        !run.ok,
        "a daemon serving no grammar must not be answered out of this binary: {}",
        run.stdout,
    );
    assert!(
        run.stderr.contains("action_grammar") || run.stderr.contains("older"),
        "the refusal names the address or the skew: {}",
        run.stderr,
    );
}

/// `show-grammar`'s own ARGUMENT parsing — the arms a test has to build because a person's typo is
/// what reaches them.
///
/// ⚠ Written because the round's debt sweep asked which branches of its own new code nothing drives
/// (R340's rule): the option parser had four refusal arms and one scope arm, and the tests above drove
/// none of them. A refusal nobody has run is a sentence that is wrong the first time somebody sees it.
#[test]
fn show_grammar_says_what_it_takes_when_a_caller_gets_it_wrong() {
    let (_host, sock) = spawn_host();

    // The SCOPE arm: a named session, which is what `--pane` needs to find a pane at all.
    // The boot session is "0" — the name `ls` prints and an unscoped request lands in.
    let scoped = sprag(&sock, &["show-grammar", "--pane", "-t", "0"]);
    assert!(
        scoped.ok && scoped.stdout.contains(KEY_ACTION),
        "a scoped ask reaches the pane surface: {} {}",
        scoped.stdout,
        scoped.stderr,
    );

    // `-t` with nothing after it.
    let dangling = sprag(&sock, &["show-grammar", "-t"]);
    assert!(
        !dangling.ok && dangling.stderr.contains("takes a session name"),
        "a dangling -t says what it wanted: {}",
        dangling.stderr,
    );

    // An option this verb does not have — and the sentence names the two it does.
    let bogus = sprag(&sock, &["show-grammar", "--json"]);
    assert!(
        !bogus.ok && bogus.stderr.contains("--pane"),
        "an unknown option names what the verb takes: {}",
        bogus.stderr,
    );

    // Two verbs at once: this narrows to ONE, and says so rather than printing the first.
    let two = sprag(&sock, &["show-grammar", SPLIT_ACTION, SPAWN_ACTION]);
    assert!(
        !two.ok && two.stderr.contains("one verb at a time"),
        "a second verb is a caller's mistake, not a silent narrowing: {}",
        two.stderr,
    );
}

// ----- the orchestration loop's CLI door (R355) -----

/// A daemon of this test's own, with one session holding one pane — the fixture the three
/// orchestration gates drive.
///
/// Returns the guard (which kills the daemon and clears its state on drop), the socket, and the
/// host id of the pane a run can name. The ID rather than a number, because `run` takes the id the
/// daemon knows and `sprag panes` is what a person reads.
///
/// ⚠ This one takes the shipped defaults. A gate whose argument names a SIDE of a switch wants
/// [`daemon_with_one_pane_told`] instead — see [`daemon_told`] for what that distinction cost.
fn daemon_with_one_pane(label: &str) -> (DaemonGuard, PathBuf, u64) {
    daemon_with_one_pane_told(label, &[])
}

/// [`daemon_with_one_pane`], with `options` written into the config that daemon reads before it
/// starts — for a gate that is about one side of a switch rather than about what ships.
fn daemon_with_one_pane_told(label: &str, options: &[(&str, &str)]) -> (DaemonGuard, PathBuf, u64) {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    daemon_told(&state, options);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": "work", "cmd": ["sh", "-c", "exec cat"] },
        }),
    )
    .expect("new_session answers");
    // The pane's HOST ID, read from the session's own pane list — `new_session` answers with the
    // name it took, and the id is what a `run` names.
    let listed = conn
        .call(
            "scene/query",
            json!({ "session": "work", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the session's panes");
    let pane = listed
        .as_array()
        .and_then(|panes| panes.first())
        .and_then(|pane| pane["id"].as_u64())
        .unwrap_or_else(|| panic!("the session's first pane id: {listed}"));
    (guard, sock, pane)
}

/// ⚠⚠ **A PERSON CAN START A BOUNDED LOOP FROM A SHELL, AND THE BOUND IS THE ONE THEY ASKED FOR.**
///
/// The README's first line names the AI↔AI orchestration loop as what sprag is FOR, and until R355
/// there was no way to start one that did not involve hand-writing a `scene/invoke` body. This is
/// that verb, driven as a person drives it: the shipped binary, a real daemon, a real pane.
///
/// The number is the claim. `--max-iterations 2` against a daemon whose own default is 100 must
/// come back `exhausted (iterations) after 2 iterations` — a run that ignored the guardrail would
/// report a different number and fail here with it.
///
/// ⚠ And the WORD IN THE BRACKET is a second claim, on the daemon's other two ceilings: this run
/// was stopped by the one the person named, not by the wall-clock deadline or the cost ceiling it
/// silently inherited. Before the outcome carried which ceiling, those three endings were one
/// word and a person could not tell them apart.
/// ⚠⚠⚠ **THE SHELL CAN SAY A PERSON IS WATCHING** — the argument reaches the CLI's flag surface,
/// and a misspelling of it is refused.
///
/// # ⚠⚠ Why an argument the daemon reads is not yet an argument a person can send
///
/// This mouth derives its flags from the grammar the daemon publishes, so a new argument SHOULD
/// arrive for free — and *should* is exactly the word R369 measured being wrong at this surface,
/// where a question the daemon had already parsed stayed invisible to the shell for a whole round.
/// The two halves are gated separately here because they fail separately: the grammar can publish
/// what the flag parser never offers, and the flag parser can accept a name nothing serves.
///
/// ⚠ The CONTROL is the second half and it is what stops the first from being vacuous: a flag this
/// surface does not know must be REFUSED. Without it the acceptance below would pass against a CLI
/// that shrugs at anything it is handed — which is the failure mode a derived surface actually has.
#[test]
fn the_shell_offers_the_argument_that_says_somebody_is_watching() {
    let (_guard, sock, pane) = daemon_with_one_pane("attend-flag");
    let pane = pane.to_string();

    let published = sprag(&sock, &["show-grammar", "run", "--plugins", "-t", "work"]);
    assert!(published.ok, "show-grammar failed: {}", published.stderr);
    assert!(
        published
            .stdout
            .contains(sprag_plugin::Attended::WIRE_KEY.replace('_', "-").as_str())
            || published.stdout.contains(sprag_plugin::Attended::WIRE_KEY),
        "⚠⚠ a person reading the grammar must be able to SEE that a run can be told to wait for \
         them — an argument served and unpublished is one nobody can find: {}",
        published.stdout,
    );

    // The pane here is a plain shell that is not asking anything, so the patience is never spent:
    // what is under test is that the FLAG is accepted at all, and the run's own ending is the
    // bounded-loop gate's business.
    let accepted = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane,
            "--stimulus",
            "echo watched",
            "--await-person-ms",
            "5000",
            "--max-iterations",
            "1",
            "--wait",
        ],
    );
    assert!(
        accepted.ok,
        "⚠⚠⚠ the shell must accept the argument the daemon serves. A refusal here is the flag \
         surface and the grammar disagreeing about what this wire takes: {}",
        accepted.stderr,
    );

    // ⚠ THE CONTROL: the same call with the key's own name mangled. Refused, or the acceptance
    // above says nothing about this argument in particular.
    let misspelled = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane,
            "--stimulus",
            "echo watched",
            "--await-person",
            "5000",
            "--max-iterations",
            "1",
            "--wait",
        ],
    );
    assert!(
        !misspelled.ok,
        "⚠⚠⚠ a flag this surface does not serve must be REFUSED, or the acceptance above would \
         pass for a CLI that swallows anything: {} / {}",
        misspelled.stdout, misspelled.stderr,
    );
}

#[test]
fn a_person_starts_a_bounded_loop_and_waits_for_how_it_ended() {
    let (_guard, sock, pane) = daemon_with_one_pane("orchestrate");
    let pane = pane.to_string();

    let started = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane,
            "--stimulus",
            "echo bounded",
            "--max-iterations",
            "2",
            "--wait",
        ],
    );
    assert!(started.ok, "the run was refused: {}", started.stderr);
    assert!(
        started
            .stdout
            .contains("exhausted (iterations) after 2 iterations"),
        "THE GUARDRAIL BOUND IT, at the number the person asked for rather than this daemon's \
         default of {}: {}",
        sprag_host::plugins::DEFAULT_MAX_ITERATIONS,
        started.stdout,
    );
    assert!(
        started.stdout.contains("bytes"),
        "and the cost is in the run's OWN unit: {}",
        started.stdout,
    );

    // ...and it is still readable afterwards, which is what makes a run an outcome rather than an
    // event somebody had to be watching for.
    let listed = sprag(&sock, &["runs", "-t", "work"]);
    assert!(listed.ok, "{}", listed.stderr);
    assert!(
        listed.stdout.contains("run 0") && listed.stdout.contains("exhausted"),
        "runs reports the finished run: {}",
        listed.stdout,
    );
}

/// **A DAEMON HOLDING A LIVE RUN IS GONE SOON AFTER SIGTERM** — `install_shutdown`, which is the
/// path `RunRegistry::JOIN_DEADLINE` exists for, driven for the first time.
///
/// ⚠⚠⚠⚠⚠ **THE RUN HERE IS DRIVEN ON A THREAD, AND SINCE 2026-08-25 THAT HAS TO BE ASKED FOR.**
/// This is the IN-PROCESS half of a pair whose other half is
/// [`a_signalled_daemon_whose_run_is_driven_elsewhere_is_gone_promptly`] — the two shapes
/// differ in whether the order can reach the driver through shared memory, which is the whole of
/// register item 664. The partner pinned `run-driver-process = on` from its first line; this half
/// said nothing and took the default, so when that default MOVED (item 544) this gate quietly
/// started measuring its partner's path and went on passing. Pinned now, and [`daemon_told`] says
/// why that is a rule rather than a patch.
///
/// Register item 305's repair is gated eight ways at the registry, and every one of those gates
/// builds its own `RunRegistry` and drops it. **None of them asks the question a person asks**: I
/// signalled a daemon that was in the middle of something — is it gone? The handler is three lines
/// in a binary, and the two of them that matter are an ORDER (`cancel_all`, then the bounded join)
/// that nothing outside this file can observe.
///
/// ⚠⚠⚠ THE BOUND SEPARATES THE TWO SHAPES rather than reporting a reading. A run that HEARS the
/// cancel is over in milliseconds and the process follows; a run merely waited out costs the whole
/// `JOIN_DEADLINE` before the handler reaches its `exit`. Two seconds is far past the first and far
/// short of the second, so a handler that forgot to ask fails here — dropping its `cancel_all` was
/// measured at **5.03 s**, the deadline exactly.
///
/// # ⚠⚠ What this gate does NOT catch, written rather than assumed
///
/// **The join going unbounded again.** The run here is healthy and hears its cancel, so an
/// unbounded join returns just as fast as a bounded one and this stays green. Staging a worker that
/// will not come back needs a pane whose device has stopped and some eighty kilobytes pushed at it,
/// which is a different fixture altogether — the boundedness itself is held at the registry, where
/// a parked worker is three lines. What lives HERE is the pairing no unit test can see: the
/// handler asks first, and it asks the registry a person's daemon actually holds.
#[test]
// Linux AND macOS, for `daemon_pid`'s reason: the question is which process was started beside this
// socket, and the process table it reads is portable.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_signalled_daemon_holding_a_live_run_is_gone_promptly() {
    let (_guard, sock, pane) = daemon_with_one_pane_told(
        "sigterm",
        &[(sprag_host::options::RUN_DRIVER_PROCESS, "off")],
    );
    let started = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane.to_string(),
            "--stimulus",
            "echo still going",
            // Effectively unbounded: this run must still be GOING when the signal lands, and a step
            // that ends on the turn constant makes a million of them longer than any test.
            "--max-iterations",
            "1000000",
            "--max-bytes",
            "1073741824",
        ],
    );
    assert!(started.ok, "the run was refused: {}", started.stderr);

    // ⚠⚠ THE FIXTURE STATES ITS HAZARD. A run that had already finished would make the timing below
    // a measurement of an empty registry — which is the trap the `rpc` gate beside this repair fell
    // into (register item 384).
    assert!(
        wait_for(Duration::from_secs(10), || sprag(
            &sock,
            &["runs", "-t", "work"]
        )
        .stdout
        .contains("running")),
        "the run never came up, so nothing was holding the shutdown: {}",
        sprag(&sock, &["runs", "-t", "work"]).stdout,
    );
    // ⚠⚠⚠⚠⚠ AND THE PIN IS A CLAIM RATHER THAN A COMMENT. This gate's whole argument is that the
    // order reaches its driver through SHARED MEMORY; take the pin away and the daemon spawns a
    // driver process, which is its partner's path, and this would go on passing while measuring
    // the other half. One process against this socket is what *driven on a thread* looks like from
    // outside — see `sprag_term_processes`, and register item 544 for the day the default moved.
    assert_eq!(
        sprag_term_processes(&sock),
        1,
        "⚠⚠ THE PREMISE: a daemon told `run-driver-process = off` drives its runs on threads of \
         its own, so exactly one `sprag-term` holds this socket while the run is live. More than \
         one means this gate is measuring the out-of-process path its partner exists for.",
    );

    let pid = daemon_pid(&sock).expect("the daemon this test spawned");
    let signalled = Instant::now();
    // SAFETY: `pid` was just read from the process table for a daemon this test spawned, matched by
    // its own socket in the environment — `daemon_pid`'s doc says why that matters.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let gone = wait_for(Duration::from_secs(10), || daemon_pid(&sock).is_none());
    let took = signalled.elapsed();
    assert!(gone, "the signalled daemon was still there after {took:?}");
    // Measured at 58.6 ms (2026-08-17), which is one `wait_for` poll plus the exit itself; the
    // handler that forgot to ask costs `JOIN_DEADLINE`, five seconds. Two lies between them and is
    // a requirement rather than a reading — a person who signalled a daemon must not wait on it.
    assert!(
        took < Duration::from_secs(2),
        "the signalled daemon took {took:?} to go: it is waiting its runs out rather than asking \
         them to stop",
    );
}

/// ⛔⛔⛔⛔⛔ **AND SO IS ONE WHOSE RUN IS DRIVEN IN A PROCESS OF ITS OWN** — register item 664.
///
/// # ⚠⚠⚠⚠⚠ Why its neighbour above cannot see this, when it drives the very same handler
///
/// That gate submits a run to a daemon PINNED to `run-driver-process = off`, so the run
/// it holds is driven by a THREAD sharing the registry's flags: `cancel_all` stores into the same
/// `AtomicBool` the worker is reading, the worker sees it at its next loop top, and the bounded
/// join returns in milliseconds. **Nothing is published, and nothing needs to be.**
///
/// A driver in another process shares no memory with this daemon. Its orders are the run's ROW and
/// it is WOKEN to re-read that row (`Event::RunOrdered`, register item 648) — and the shutdown
/// sweep calls [`sprag_host::runs::RunRegistry::cancel_all`] on the registry DIRECTLY, past the
/// three doors of `PluginsExternal` that announce on their accepted arms. So the flags
/// `ProcessRun` holds are, in its own words, *pure record*: nobody wakes, nobody re-reads, the
/// collector thread goes on waiting for a child that was never told, and the handler pays
/// `JOIN_DEADLINE` in full before its `exit`.
///
/// ⚠⚠ **THE OPTION IS TURNED ON HERE RATHER THAN WAITED FOR.** Item 544's flip is what makes this
/// the ordinary path, and item 664 is one of the two things holding that word at `off` — a gate
/// that waited for the default would be a gate for the day after the repair.
///
/// ⚠⚠⚠ **THE STAGING CONTROL IS THE PROCESS TABLE**, and without it *the sweep reaches its
/// drivers* and *this daemon never had one* are the same green: a config that failed to take, a
/// run refused, or a driver that had not been spawned yet all produce a daemon that exits fast
/// because there was nothing to wait for. Two `sprag-term` against this socket is what says the
/// run really is being driven somewhere else.
///
/// ⚠ The pid is captured BEFORE the signal and asked about by number afterwards. [`daemon_pid`]
/// answers *the holder that is nobody's child*, and the moment the daemon goes its orphaned driver
/// becomes exactly that — so re-asking the question would answer the driver's pid and this gate
/// would hang on a daemon that had already gone.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_signalled_daemon_whose_run_is_driven_elsewhere_is_gone_promptly() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-sigterm-driven-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let _guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    let config = state.join("config").join("sprag");
    std::fs::create_dir_all(&config).expect("this test's own config directory");
    std::fs::write(
        config.join(sprag_host::CONFIG_FILE),
        format!(
            "[options]\n{} = \"on\"\n",
            sprag_host::options::RUN_DRIVER_PROCESS
        ),
    )
    .expect("a config a daemon will read");
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );

    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": "work", "cmd": ["sh", "-c", "stty -echo; exec cat"] },
        }),
    )
    .expect("new_session answers");
    let pane = conn
        .call(
            "scene/query",
            json!({ "session": "work", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the session's pane");
    conn.call(
        "scene/invoke",
        json!({
            "session": "work",
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "orchestrator",
                "pane": pane,
                "stimulus": "x",
                // Never printed by a `cat`, and bounded far past any shutdown: the run must still
                // be GOING when the signal lands or the timing below measures an empty registry.
                "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the run is submitted");

    assert!(
        wait_for(Duration::from_secs(20), || sprag_term_processes(&sock) == 2),
        "⚠⚠ THE PREMISE: this run is driven in a process of its own, so there are two `sprag-term` \
         against this socket. Found {}, and the timing below would then be a measurement of a \
         daemon holding nothing.",
        sprag_term_processes(&sock),
    );

    let pid = daemon_pid(&sock).expect("the daemon this test spawned");
    let signalled = Instant::now();
    // SAFETY: `pid` was just read from the process table for a daemon this test spawned, matched by
    // its own socket in the environment — `daemon_pid`'s doc says why that matters.
    unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
    let gone = wait_for(Duration::from_secs(10), || {
        !sprag_terminal::procfs::pids_named("sprag-term").contains(&pid)
    });
    let took = signalled.elapsed();
    assert!(gone, "the signalled daemon was still there after {took:?}");
    assert!(
        took < Duration::from_secs(2),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 664: the signalled daemon took {took:?} to go, which is \
         `JOIN_DEADLINE` ({:?}) rather than the milliseconds its thread-driven neighbour costs — \
         so the collector thread waited out a child that never stopped, and a person who \
         signalled this daemon waited with it. ⚠⚠ TWO SEAMS CAN PRODUCE THIS, and a reader must \
         not stop at the first: (1) THE SWEEP TOLD NOBODY — a driver in another process reads its \
         orders off the row and is woken to re-read it, so a `cancel_all` that publishes nothing \
         is a flag written for a reader that shares no memory with this daemon (`Orders::deliver` \
         is where that announcement is raised, and it is raised for every order because the sweep \
         reaches the registry directly, past the plugin surface's doors); (2) THE DAEMON COULD \
         NOT ANSWER — what the woken driver does is CALL BACK for its row, so a shutdown holding \
         the registry lock across its join blocks the one question it has just asked for, and the \
         wake buys nothing (`RunRegistry::stop_all_within` takes that lock per pass for exactly \
         this reason). Both were measured red on 2026-08-25, separately, at 5.03 s and 5.02 s.",
        sprag_host::runs::RunRegistry::JOIN_DEADLINE,
    );
}

/// ⚠⚠ **A FLAG NOBODY WROTE IN THIS BINARY** — the publication surface paying out for an argument
/// that did not exist when the door was built.
///
/// R355 proved the CLI FOLLOWS the daemon's grammar by RENAMING an argument in the daemon and
/// watching the untouched CLI refuse the old spelling. This is the other direction, and the one
/// that matters more: a guardrail ADDED to the daemon's grammar is reachable from the command line
/// with **no edit to any CLI source file at all**. `sprag orchestrate` contains no string
/// `max_seconds`; it asks the daemon what a run takes and offers what it is told.
///
/// The claim would be hollow without the run actually being bounded by it, so all three legs are
/// here: the flag is ACCEPTED, the run ends at the CLOCK (its iteration ceiling is a hundred
/// thousand steps this pane will never take), and the person's own renderer NAMES which ceiling
/// stopped it.
#[test]
fn a_person_bounds_a_loop_in_time_with_a_flag_this_binary_never_spells() {
    let (_guard, sock, pane) = daemon_with_one_pane("clockwork");
    let pane = pane.to_string();

    let started = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane,
            "--stimulus",
            "echo timed",
            "--sentinel",
            "A SENTINEL THIS PANE NEVER PRINTS",
            "--max-iterations",
            "100000",
            "--max-seconds",
            "1",
            "--wait",
        ],
    );
    assert!(
        started.ok,
        "the daemon publishes max_seconds, so the CLI offers --max-seconds: {}",
        started.stderr,
    );
    assert!(
        started.stdout.contains("exhausted (duration)"),
        "THE CLOCK BOUND IT, and the person is told so: the iteration ceiling was a hundred \
         thousand and nothing else could have ended this run. {}",
        started.stdout,
    );
}

/// ⚠⚠ **THE CLI BUILDS ITS CALL OUT OF WHAT THE DAEMON PUBLISHED**, so its refusals are the
/// daemon's grammar speaking and not a second list of argument names in this binary.
///
/// This is the first consumer in the workspace that ACTS on the published grammar rather than
/// printing it. Three refusals, each of which can only be produced by having read the daemon's
/// answer:
///
/// * no plugin word — names every word the daemon publishes as selecting a form;
/// * a missing required argument — names it, out of the SELECTED form;
/// * a cost bound the chosen plugin cannot take — `--max-tokens` is not an argument of a byte-relay
///   form at all, which is the mutual exclusion made UNREPRESENTABLE by publishing guardrails per
///   form rather than as one flat object.
#[test]
fn the_orchestrate_refusals_are_the_daemons_own_grammar() {
    let (_guard, sock, pane) = daemon_with_one_pane("grammar");
    let pane = pane.to_string();

    let no_word = sprag(&sock, &["orchestrate", "-t", "work", "--pane", &pane]);
    assert!(!no_word.ok);
    for word in ["orchestrator", "pipe", "agent", "dialogue"] {
        assert!(
            no_word.stderr.contains(word),
            "the refusal names every plugin this daemon serves, {word} included: {}",
            no_word.stderr,
        );
    }

    let missing = sprag(
        &sock,
        &["orchestrate", "agent", "-t", "work", "--pane", &pane],
    );
    assert!(!missing.ok);
    assert!(
        missing.stderr.contains("needs prompt"),
        "it names the argument the SELECTED form requires: {}",
        missing.stderr,
    );

    let wrong_unit = sprag(
        &sock,
        &[
            "orchestrate",
            "agent",
            "-t",
            "work",
            "--pane",
            &pane,
            "--prompt",
            "hi",
            "--max-tokens",
            "5",
        ],
    );
    assert!(!wrong_unit.ok);
    assert!(
        wrong_unit
            .stderr
            .contains("--max-tokens is not an argument")
            && wrong_unit.stderr.contains("max_bytes"),
        "a token bound is not offered on a plugin that spends bytes, and the refusal says what IS: \
         {}",
        wrong_unit.stderr,
    );
}

/// ⛔⛔⛔⛔ **`sprag hold-run` AND `sprag stand-down` STOP PROMISING A PERSON SOMETHING THEY WILL
/// NOT GET** — register items 539 and 597, at the surface a person actually types.
///
/// # What was printed, and what happened
///
/// `hold-run` printed `sprag_plugin::HOLD_TAKES_EFFECT` — *"it parks at its next pass … nothing
/// further is typed at the pane while it waits"* — and `stand-down` printed
/// `sprag_plugin::STAND_DOWN_TAKES_EFFECT`. Both sentences are true of exactly ONE plugin, and both
/// were printed for a run of ANY of the six. So a person holding an `orchestrator` to read its pane
/// was told the pane had gone still, and it had not: the run drove on and kept typing under them.
///
/// ⚠⚠⚠ **THIS GATE IS AT THE CLI ON PURPOSE.** The host's own gate proves the wire door refuses;
/// nothing proved the COMMAND stops printing the promise, and *a fact that reaches the wire and
/// dies at the mouth* is this repository's most repeated defect. Here the promise is a string
/// owned one crate over, so the assertion holds the printed output to that constant rather than to
/// words retyped here.
///
/// ⚠⚠ **AND THE CONTROL IS THE SAME COMMAND AGAINST A LOOP**, which is what keeps this from
/// passing on a build where both verbs simply refuse everything.
#[test]
fn an_order_only_the_loop_reads_is_refused_at_the_command_a_person_types() {
    let (_guard, sock, pane) = daemon_with_one_pane("orders");
    let pane = pane.to_string();

    let started = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane,
            "--stimulus",
            "sleep 1",
            "--max-iterations",
            "1000000",
        ],
    );
    assert!(started.ok, "{}", started.stderr);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(
            &sock,
            &["runs", "-t", "work"]
        )
        .stdout
        .contains("running")),
        "the run never reached the state this test is about",
    );

    for (argv, promise) in [
        (
            vec!["stand-down", "0", "-t", "work"],
            sprag_plugin::STAND_DOWN_TAKES_EFFECT,
        ),
        (
            vec!["hold-run", "0", "-t", "work"],
            sprag_plugin::HOLD_TAKES_EFFECT,
        ),
    ] {
        let said = sprag(&sock, &argv);
        let printed = format!("{}{}", said.stdout, said.stderr);
        assert!(
            !said.ok,
            "⛔⛔⛔ ITEMS 539/597: `sprag {}` SUCCEEDED against an orchestrator run. No plugin but \
             the loop reads that order, so the run drove straight on while the caller was told it \
             had not: {printed}",
            argv.join(" "),
        );
        assert!(
            !printed.contains(promise),
            "⛔⛔⛔⛔ AND THE PROMISE WAS PRINTED ANYWAY, which is the whole cost of these two \
             items. A person who reads this goes and types in a pane an agent is still driving. \
             Got: {printed}",
        );
        assert!(
            printed.contains("orchestrator"),
            "⚠⚠⚠ the refusal must name WHICH KIND of run cannot take the order — *refused* alone \
             sends a person to check whether they typed the wrong id: {printed}",
        );
        assert!(
            printed.contains("cancel-run"),
            "⚠⚠ and what to reach for instead. A refusal that leaves somebody with no way to stop \
             a long unattended run has told them half of what they need: {printed}",
        );
    }

    // ⚠⚠⚠⚠ THE CONTROL: the same two verbs against a run whose plugin DOES read them. Without it
    // this gate passes on a build where both verbs refuse every run, which is the opposite defect
    // and would take the two orders away from the one plugin that can obey them.
    let loop_run = sprag(
        &sock,
        &[
            "orchestrate",
            "ai_loop",
            "-t",
            "work",
            "--pane",
            &pane,
            "--north-star",
            "SPRAG-ORDERS-CONTROL",
            "--milestone",
            "say the marker",
            "--reference",
            "this gate",
            "--max-turns",
            "1000000",
            // ⚠ The three the form requires, named by the refusal this gate's first run got: an
            // `ai_loop` carries a readiness barrier because it INJECTS, and the barrier is what
            // stops it typing into a shell. It never has to be satisfied here — the orders below
            // are given to a run that EXISTS, and readiness is about what it may do to the pane.
            "--agent",
            "claude",
            "--match",
            "shows",
            "--marker",
            "SPRAG-ORDERS-CONTROL-READY",
        ],
    );
    assert!(loop_run.ok, "{}", loop_run.stderr);
    for (argv, promise) in [
        (
            vec!["stand-down", "1", "-t", "work"],
            sprag_plugin::STAND_DOWN_TAKES_EFFECT,
        ),
        (
            vec!["hold-run", "1", "-t", "work"],
            sprag_plugin::HOLD_TAKES_EFFECT,
        ),
    ] {
        let said = sprag(&sock, &argv);
        assert!(
            said.ok && said.stdout.contains(promise),
            "⚠⚠⚠⚠ THE CONTROL: `sprag {}` against an `ai_loop` run must still be ACCEPTED and \
             still print the promise — that plugin reads the order, and a refusal here would mean \
             the fix took the order away from the one run that can obey it: {} / {}",
            argv.join(" "),
            said.stdout,
            said.stderr,
        );
    }
}

/// ⚠⚠ **A RUNNING LOOP CAN BE STOPPED, AND STOPPING ONE THAT IS NOT THERE IS A DIFFERENT ANSWER.**
///
/// The cancel flag is polled by every wait inside the driver, so a cancel lands BETWEEN steps
/// rather than killing a thread mid-write — which is what leaves the pane readable. The fixture
/// starts a run whose ceiling it will not reach in the life of this test, so what ends it is
/// provably the cancel and not exhaustion.
#[test]
fn a_running_loop_is_cancelled_and_an_absent_one_is_told_apart() {
    let (_guard, sock, pane) = daemon_with_one_pane("cancel");
    let pane = pane.to_string();

    let started = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &pane,
            "--stimulus",
            "sleep 1",
            "--max-iterations",
            "1000000",
        ],
    );
    assert!(started.ok, "{}", started.stderr);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(
            &sock,
            &["runs", "-t", "work"]
        )
        .stdout
        .contains("running")),
        "the run never reached the state this test is about",
    );

    let cancelled = sprag(&sock, &["cancel-run", "0", "-t", "work"]);
    assert!(cancelled.ok, "{}", cancelled.stderr);
    assert!(
        wait_for(Duration::from_secs(20), || sprag(
            &sock,
            &["runs", "-t", "work"]
        )
        .stdout
        .contains("cancelled")),
        "the run never ended cancelled: {}",
        sprag(&sock, &["runs", "-t", "work"]).stdout,
    );
    // ⚠⚠ **AND WHAT BECAME OF THE WORK IS ON THE SAME LISTING.** `cancelled` alone is consistent
    // with two opposite states of the world — the peer stopped, or it is still going and still
    // spending — and the one a person acts on is the second. This pane's own program IS the peer
    // (`exec cat`), so the run is REFUSED the reach that would have closed the pane, and the
    // listing has to say so rather than leave `cancelled` to be read as *it is over*.
    let listed = sprag(&sock, &["runs", "-t", "work"]).stdout;
    assert!(
        listed.contains("still running"),
        "a cancelled run must say what became of its work, or the answer reaches the wire and \
         dies at the mouth a person reads: {listed}",
    );

    // A run nobody has is a REFUSAL with its own sentence, not a silent success — the difference
    // between "stopped" and "there was nothing to stop" is the whole of what a caller needs next.
    let absent = sprag(&sock, &["cancel-run", "999", "-t", "work"]);
    assert!(!absent.ok);
    assert!(
        absent.stderr.contains("no run 999 is in flight"),
        "{}",
        absent.stderr,
    );
}

/// **A MISTYPED SESSION IS A MISTYPED SESSION AT EVERY VERB — not a Rust variant name, and above
/// all not an order to kill the daemon.**
///
/// # The defect, measured
///
/// Found by the sweep that item 425 (`processes` / `resources` ignoring `-t`) ended with: the same
/// question asked of every other verb that publishes `-t SESSION`. Nine readers were probed and
/// eight refused an unknown scope cleanly. The run family did not. Measured 2026-08-17 against a
/// daemon built from HEAD — which serves every one of these paths, and the `-t 0` column is the
/// control that PROVES it serves them:
///
/// ```text
///                       -t 0 (control)              -t nosuch
/// orchestrate           names the plugins           host rpc error: NoExternalAtPath
/// runs                  no runs (start one ...)     host rpc error: NoExternalAtPath
/// cancel-run 999        no run 999 is in flight     "... is older than this build of sprag.
/// stand-down 999        no run 999 is in flight      Restart it: `sprag kill-server`"
/// display-message hi    shown to nobody: ...        (the same kill-server sentence)
/// ```
///
/// ⚠⚠⚠⚠ **THE SKEW SENTENCE IS THE SHARP ONE AND IT IS WORSE THAN A LEAKED VARIANT NAME.** A leaked
/// variant is ugly and admits it failed. This one is CONFIDENT AND WRONG: it diagnoses version skew
/// that is not there and prescribes `sprag kill-server` — so the answer to a typo'd session name is
/// an instruction to end every session on the machine. On the box that runs the debt loop, a person
/// following it would kill live runs.
///
/// ⚠⚠⚠ **`display-message` IS NOT A RUN VERB AND WAS FOUND BY A SECOND SWEEP.** The first pass
/// asked only the readers that take no argument; this one asked the seventeen verbs that ACT, where
/// a dropped scope would not merely answer wrong but act on the WRONG SESSION. Sixteen refused and
/// left the workspace byte-identical — the hazard is not there — and the seventeenth carried this
/// same false remedy. **A fix applied to a FAMILY is a fix applied to a list somebody wrote.**
///
/// # Why it lands here and not on the daemon
///
/// The verbs pass `-t` through as the request's out-of-band scope and never pre-flight the name, so
/// the daemon's refusal of an unknown scope arrives as *this path is not served* — which the client
/// then reads as skew, because for every OTHER cause that is what it means. [`connect_scoped`] is
/// the pre-flight every window and pane verb already makes, and 425 gave it to `processes` and
/// `resources` for exactly this half of the same defect.
///
/// # What is asserted
///
/// Every verb, each with its own required arguments, and the CONTROL beside it: the same command
/// scoped to a session that DOES exist must still reach the daemon and answer. Without that column
/// this test would pass on a build where `-t` refused everything, which is the opposite defect.
#[test]
fn a_mistyped_session_at_the_run_verbs_is_not_an_order_to_kill_the_daemon() {
    let (_guard, sock, _pane) = daemon_with_one_pane("scoped-run-verbs");

    // (argv without the scope, a phrase the REAL session's answer carries). The control phrase is
    // what proves this daemon serves the path, so a refusal below can only be about the name.
    let family: &[(&[&str], &str)] = &[
        (&["orchestrate"], "orchestrate"),
        (&["runs"], "no runs"),
        (&["cancel-run", "999"], "no run 999 is in flight"),
        (&["stand-down", "999"], "no run 999 is in flight"),
        // NOT a run verb. It is here because it carried the identical sentence and because a list
        // is what this defect hides behind: the round that fixed the family would have left it.
        (&["display-message", "hello"], "shown to"),
    ];

    for (argv, reached) in family {
        // THE CONTROL FIRST. `work` exists, so whatever comes back is the verb's own answer and
        // this daemon demonstrably serves the path the refusal below would otherwise be blamed on.
        let mut real: Vec<&str> = argv.to_vec();
        real.extend(["-t", "work"]);
        let run = sprag(&sock, &real);
        assert!(
            run.stdout.contains(reached) || run.stderr.contains(reached),
            "`sprag {}` reaches the run registry, or nothing below discriminates: {} / {}",
            real.join(" "),
            run.stdout,
            run.stderr,
        );

        let mut ghosted: Vec<&str> = argv.to_vec();
        ghosted.extend(["-t", "nosuch"]);
        let ghost = sprag(&sock, &ghosted);
        assert!(
            !ghost.ok,
            "`sprag {}` is refused: {}",
            ghosted.join(" "),
            ghost.stdout,
        );
        assert!(
            ghost.stderr.contains("no session named"),
            "`sprag {}` names the session the caller got wrong: {}",
            ghosted.join(" "),
            ghost.stderr,
        );
        // The two wrong answers this exists to remove, named as themselves so a future rewrite
        // cannot reintroduce either while still saying something plausible.
        assert!(
            !ghost.stderr.contains("NoExternalAtPath"),
            "`sprag {}` must not print a Rust variant name at an operator: {}",
            ghosted.join(" "),
            ghost.stderr,
        );
        assert!(
            !ghost.stderr.contains("kill-server"),
            "`sprag {}` must not answer a typo with an order to end every session: {}",
            ghosted.join(" "),
            ghost.stderr,
        );
    }
}

/// ⚠⚠ **THE DOOR ONTO THE WIRE'S GRAMMAR CAN BE POINTED AT THE LOOP** — and what it says there
/// includes the arguments INSIDE `guardrails`.
///
/// Two defects in one gate, both found by the round that built the loop's door:
///
/// * `show-grammar` knew two surfaces and this daemon serves three. The plugin host has published
///   its own `action_grammar` since a derived audit found it (R353), and the verb whose whole job
///   is *"ask the daemon how to call this"* could not be pointed at it — so the loop the README
///   leads with was undiscoverable from the discovery verb.
/// * the printer walked the answer's JSON by hand and stopped at the top level, so a nested
///   argument printed as `guardrails object optional` and its fields were never named. That is the
///   silence the nested grammar was added to end, printed by the verb that exists to end it.
///
/// The assertion is on the FIELDS, because the parent alone is what the defect looked like.
#[test]
fn show_grammar_points_at_the_plugin_host_and_names_what_is_inside_guardrails() {
    let (_guard, sock, _pane) = daemon_with_one_pane("grammar-surface");

    let run = sprag(&sock, &["show-grammar", "run", "--plugins", "-t", "work"]);
    assert!(run.ok, "show-grammar --plugins failed: {}", run.stderr);
    assert!(
        run.stdout.contains("guardrails") && run.stdout.contains("max_iterations"),
        "the iteration ceiling is NAMED, not hidden behind its parent's type: {}",
        run.stdout,
    );
    assert!(
        run.stdout.contains("max_bytes") && run.stdout.contains("max_tokens"),
        "and each form publishes the cost key ITS plugin admits, which is what removed the \
         mutual exclusion from the grammar: {}",
        run.stdout,
    );
    assert!(
        run.stdout.contains("one of: orchestrator"),
        "each form still says the one word that selects it: {}",
        run.stdout,
    );
    // ⚠⚠ AND A LIST SAYS THAT IT IS ONE. `may_answer` and a `dialogue` endpoint's argv are both
    // `array`, and indented under that word their fields print identically — so a reader could not
    // tell "send these keys once" from "send this many times, each with these keys". That is the
    // same silence this gate's own doc is about, one shape out: a nested grammar the printer
    // publishes without saying whose fields these are.
    let listed = run
        .stdout
        .lines()
        .skip_while(|line| !line.contains(sprag_plugin::Consents::WIRE_KEY))
        .take(4)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        listed.contains("array") && listed.contains("each entry:") && listed.contains("asked"),
        "⚠⚠ the consent prints as an ARRAY, says its fields belong to ONE ENTRY, and names them: \
         {listed:?} — from {}",
        run.stdout,
    );

    // ...and the surfaces are still NOT each other: a plugin verb is not on the multiplexer, and
    // the refusal says which surface was asked and how to name another.
    let wrong = sprag(&sock, &["show-grammar", "run", "-t", "work"]);
    assert!(!wrong.ok);
    assert!(
        wrong.stderr.contains("sprag_mux") && wrong.stderr.contains("--plugins"),
        "the refusal names the surface it asked and the flags that name the others: {}",
        wrong.stderr,
    );
}

/// ⚠⚠⚠ **THE BYTE IS THE CONTROL AND THE VERB IS THE SUBJECT, THROUGH THE SHIPPED CLI.**
///
/// The same pair `sprag_terminal::stop` measures at the substrate, driven here through the two
/// commands a person actually types — because what a caller has is the CLI, and a mechanism that is
/// right one layer down and unreachable from the outside is not a fix.
///
/// The pane runs `stty -isig; exec sleep 300`, so its terminal has been told to make no signals out
/// of input. Then:
///
/// 1. `send-keys PANE C-c` succeeds and the screen shows `^C` — the byte was PROCESSED, so what
///    follows is not a race a longer wait would win. **The job lives.**
/// 2. `stop-job PANE` ends it, and its line names the program and the process group.
///
/// ⚠ The `^C` assertion is what makes step 1 an OBSERVATION rather than a timer — the load-marginal
/// shape this suite has paid for repeatedly.
#[test]
fn a_ctrl_c_is_a_byte_the_pane_may_ignore_and_stop_job_is_a_signal_it_cannot() {
    let (_host, sock) = spawn_host();
    let split = sprag(
        &sock,
        &[
            "split-window",
            "--",
            "/bin/sh",
            "-c",
            "stty -isig; exec sleep 300",
        ],
    );
    assert!(split.ok, "split-window succeeded: {}", split.stderr);

    assert!(
        wait_for(Duration::from_secs(15), || {
            sprag(&sock, &["processes", "1"]).stdout.contains("sleep")
        }),
        "the fixture reached its job, or nothing below measures anything: {}",
        sprag(&sock, &["processes", "1"]).stdout,
    );

    assert!(sprag(&sock, &["send-keys", "1", "C-c"]).ok);
    assert!(
        wait_for(Duration::from_secs(15), || {
            sprag(&sock, &["capture-pane", "1", "-p"])
                .stdout
                .contains("^C")
        }),
        "⚠ THE CONTROL'S PREMISE: the terminal ECHOED the byte, so it was processed: {}",
        sprag(&sock, &["capture-pane", "1", "-p"]).stdout,
    );
    assert!(
        sprag(&sock, &["processes", "1"]).stdout.contains("sleep"),
        "⚠⚠ THE CONTROL: `send-keys C-c` was written, echoed, and stopped nothing — which is what \
         it guarantees, and it is nothing: {}",
        sprag(&sock, &["processes", "1"]).stdout,
    );

    let stopped = sprag(&sock, &["stop-job", "1"]);
    assert!(stopped.ok, "the job is signalled: {}", stopped.stderr);
    assert!(
        stopped.stdout.contains("sleep") && stopped.stdout.contains("interrupted"),
        "⚠ and the line NAMES what received it and what was delivered — a write of 0x03 can \
         report neither: {}",
        stopped.stdout,
    );
    assert!(
        wait_for(Duration::from_secs(15), || {
            !sprag(&sock, &["processes", "1"]).stdout.contains("sleep")
        }),
        "⚠⚠ THE SUBJECT: the signal ended the job the byte could not: {}",
        sprag(&sock, &["processes", "1"]).stdout,
    );

    // ⚠⚠ AND A PANE WHOSE PROGRAM HAS ALREADY FINISHED SAYS SO. The pane outlives its child, so
    // this is a real state a caller reaches — and *your program is over* sends them to their
    // scrollback where *there is no such pane* would send them to their pane list. The daemon's own
    // refusal path for it had no gate at this mouth until now; it is free here because the stop
    // above is what ended the program.
    let finished = sprag(&sock, &["stop-job", "1"]);
    assert!(!finished.ok);
    assert!(
        finished.stderr.contains("already exited"),
        "a pane whose child is gone is refused about the PROGRAM, not about the pane: {}",
        finished.stderr,
    );

    // ⚠ AND A WORD THIS VERB DOES NOT SEND IS REFUSED WITH THE LIST. A caller who mistyped is owed
    // the vocabulary, and the wire's own type refusal has nowhere to carry one.
    let wrong = sprag(&sock, &["stop-job", "0", "--signal", "maim"]);
    assert!(!wrong.ok);
    assert!(
        wrong.stderr.contains("interrupt")
            && wrong.stderr.contains("terminate")
            && wrong.stderr.contains("kill"),
        "the refusal lists every word the verb takes: {}",
        wrong.stderr,
    );
}

/// A pane the DAEMON'S OWN DETECTOR reads as `claude`, blocked on a numbered menu — and which takes
/// the digit `1` and then prints a sentinel.
///
/// ⚠ Its shape is `workspace::tests::BLOCKED_CLAUDE`'s, with a question line added above the list:
/// a consent names the QUESTION, and that fixture's screen has nothing above its first option, so
/// there is no sentence for one to be about. Everything else — the marker glyph, the footer that
/// makes the built-in manifest claim the pane — is unchanged, because what this gate needs is a
/// dialog the SHIPPING detector recognises rather than one this test invented.
///
/// ⚠ It reads bytes in a LOOP and ignores everything that is not the digit, so a stray keystroke
/// cannot end the fixture early and turn a real failure into a green pass.
const ASKING_CLAUDE: &str = "stty -icanon -echo 2>/dev/null; \
     printf '\\033[2J\\033[HDo you want to proceed?\\n\\342\\235\\257 1. Yes\\n  2. No\\n  \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n'; \
     while :; do \
       k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'); \
       [ -n \"$k\" ] || exit 0; \
       [ \"$k\" = 49 ] && break; \
     done; \
     printf '\\033[2J\\033[HANSWERED-OK\\n'; exec cat";

/// ⚠⚠⚠ **THE SAME PEER, ASKING TWICE IN ONE TURN** — the shape a real agent turn has, and the one
/// a single-clause consent could not survive.
///
/// An agent that runs a command and then edits a file asks about both, in its own different words.
/// Everything else here is [`ASKING_CLAUDE`]'s — the same footer, so the daemon's own detector
/// claims the pane the same way, and the same `ANSWERED-OK` sentinel — so what differs between the
/// two fixtures is the number of questions and nothing else.
const ASKING_CLAUDE_TWICE: &str = "stty -icanon -echo 2>/dev/null; \
     ask() { \
       printf '\\033[2J\\033[H%s\\n\\342\\235\\257 1. Yes\\n  2. No\\n  \\342\\217\\270 manual mode on \\302\\267 ? for shortcuts\\n' \"$1\"; \
       while :; do \
         k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'); \
         [ -n \"$k\" ] || exit 0; \
         [ \"$k\" = 49 ] && break; \
       done; \
     }; \
     ask 'Do you want to proceed?'; \
     ask 'Do you want to make this edit?'; \
     printf '\\033[2J\\033[HANSWERED-OK\\n'; exec cat";

/// ⚠⚠⚠ **THE ANSWERING CONTRACT, END TO END THROUGH A REAL DAEMON** — the wire, the shipping
/// detector, a real pseudoterminal, and a run that answers its peer and goes on to converge.
///
/// # What this proves that the unit gates cannot
///
/// `sprag-plugin`'s gates drive the barrier directly and `sprag-host`'s drive the parser and the
/// renderer, each with the other side supplied. Nothing joined them: **no test had ever sent
/// `may_answer` over the wire.** That matters more here than for an ordinary argument, because this
/// wire is measured to SWALLOW an undeclared key and report success
/// (`an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused`) — so a
/// mis-spelled key, a parser never reached, or a spec field never threaded would all look exactly
/// like a run that simply chose not to answer.
///
/// The peer here is claimed by the daemon's OWN agent detector, and the question is parsed by the
/// shipping `sprag_detect::question` off the pane's real screen. Nothing in the path is a double.
///
/// ⚠ THE RUN IS SUBMITTED ONLY ONCE THE DAEMON REPORTS THE PANE BLOCKED. The detector has a settle
/// window, so a run started before it has answered would inject its stimulus into a pane the daemon
/// does not yet call blocked — a race in the FIXTURE that would read as the product failing to
/// answer.
///
/// ⚠ CONTROL: the same run, same pane, same dialog, with NO `may_answer` — it must end `blocked`
/// having answered nothing. Without it this gate would pass against a product that answers every
/// dialog it meets, which is the behaviour the whole contract exists to prevent.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_run_given_consent_answers_its_peer_over_the_wire_and_one_without_it_does_not() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-consent-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    // ⚠⚠⚠ PINNED, and this is the IN-PROCESS half of register item 665's pair — see the partner's
    // doc and [`daemon_told`]. It used to take the default and say so in prose; the day that
    // default moved (item 544) this gate would have started measuring the partner's path instead,
    // still green, with nothing anywhere to say the pair had collapsed into one arm.
    daemon_told(&state, &[(sprag_host::options::RUN_DRIVER_PROCESS, "off")]);
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");

    // Each half gets its OWN session and pane, so the control cannot be answered by the subject's
    // run and the two dialogs cannot be confused for one another.
    /// A fresh session whose pane runs `script`, returned once the DAEMON'S OWN detector calls it
    /// blocked.
    fn blocked_pane(conn: &mut HostConn, session: &str, script: &str) -> u64 {
        conn.call(
            "scene/invoke",
            json!({
                "path": mux_action_path(NEW_SESSION_ACTION),
                "args": { "name": session, "cmd": ["sh", "-c", script] },
            }),
        )
        .expect("new_session answers");
        let pane = conn
            .call(
                "scene/query",
                json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
            )
            .expect("the pane list answers")
            .as_array()
            .and_then(|panes| panes.first().cloned())
            .and_then(|pane| pane["id"].as_u64())
            .expect("the session's pane");
        // ⚠ THE BEHAVIOURAL TRIGGER: wait for the DAEMON to say the pane is blocked, not for a
        // clock. The detector settles, and a run submitted before it has would be racing.
        let ready = wait_for(Duration::from_secs(20), || {
            conn.call(
                "scene/query",
                json!({ "session": session, "path": mux_action_path(PANES_SLOT) }),
            )
            .ok()
            .and_then(|panes| panes.as_array().and_then(|list| list.first().cloned()))
            .is_some_and(|entry| entry["agent"]["state"] == "blocked")
        });
        assert!(
            ready,
            "the daemon's own detector must call this pane blocked, or the gate is about nothing",
        );
        pane
    }

    /// One orchestrator run over the wire, with or without a consent, waited out and answered as
    /// its `runs` entry. `awaits` names a person watching the pane, for the supervised half.
    fn drive(
        conn: &mut HostConn,
        sock: &Path,
        session: &str,
        pane: u64,
        consent: Option<Value>,
        awaits: Option<u64>,
    ) -> Value {
        let mut args = json!({
            "plugin": "orchestrator",
            "pane": pane,
            "stimulus": "ping",
            "sentinel": "ANSWERED-OK",
            // ⚠ SIX iterations, not four: a supervised run spends one of them on the WAIT itself
            // (the barrier hands the step back so the next one meets the pane afresh), so the
            // budget that fits the unattended halves would end this one `exhausted` — a red about
            // arithmetic wearing the shape of a red about the feature.
            "guardrails": { "max_iterations": 6, "max_seconds": 120 },
        });
        if let Some(consent) = consent {
            args[sprag_plugin::Consents::WIRE_KEY] = consent;
        }
        if let Some(patience) = awaits {
            args[sprag_plugin::Attended::WIRE_KEY] = patience.into();
        }
        conn.call(
            "scene/invoke",
            json!({
                "session": session,
                "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
                "args": args,
            }),
        )
        .expect("the run is submitted");
        let mut last = Value::Null;
        // ⚠⚠⚠ SAMPLED WHILE THE RUN IS ALIVE, because that is the only window in which *driven on
        // a thread* is observable at all: the pin above is this gate's premise, and a premise
        // nothing reads is a comment. The MAXIMUM over the whole wait, not one reading.
        let mut most = 0;
        let finished = wait_for(Duration::from_secs(90), || {
            most = most.max(sprag_term_processes(sock));
            last = conn
                .call(
                    "scene/query",
                    json!({
                        "session": session,
                        "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
                    }),
                )
                .expect("the runs slot answers");
            last.as_array()
                .and_then(|runs| runs.last().cloned())
                .is_some_and(|run| run["state"]["status"] == "done")
        });
        assert!(finished, "the run never finished: {last}");
        assert_eq!(
            most, 1,
            "⚠⚠ THE PREMISE: a daemon told `run-driver-process = off` drives on threads of its \
             own, so one `sprag-term` holds this socket for the whole of a run. {most} means this \
             gate has drifted onto the out-of-process path its partner exists for (item 544).",
        );
        last.as_array()
            .and_then(|runs| runs.last().cloned())
            .expect("the run's entry")
    }

    // ── THE SUBJECT: a consent naming this question and this option.
    let pane = blocked_pane(&mut conn, "answered", ASKING_CLAUDE);
    let outcome = drive(
        &mut conn,
        &sock,
        "answered",
        pane,
        Some(json!([{
            sprag_plugin::Consent::ASKED_KEY: "Do you want to proceed?",
            sprag_plugin::Consent::ANSWER_KEY: "Yes",
        }])),
        None,
    )["state"]["outcome"]
        .clone();
    assert_eq!(
        outcome["state"], "converged",
        "⚠⚠⚠ the run answered its peer's dialog and went on to reach its sentinel — a
         `may_answer` the daemon swallowed would leave this `blocked`: {outcome}",
    );
    assert_eq!(
        outcome[sprag_host::plugins::RUN_ANSWERED_KEY],
        1,
        "and the outcome says a decision was taken on the caller's behalf: {outcome}",
    );
    assert!(
        outcome.get(sprag_host::plugins::RUN_ASKING_KEY).is_none(),
        "a converged run has no unanswered question: {outcome}",
    );

    // ── THE TURN THAT ASKS TWICE: two clauses, both of them the caller's, one run.
    //
    // ⚠⚠⚠ THE CLAIM THE UNIT GATES CANNOT MAKE. `sprag-plugin` measures the list against its own
    // fixture and `sprag-host` measures the parser; neither sends a SECOND CLAUSE over the wire, and
    // this surface is measured to swallow what it does not read. A daemon that kept only the first
    // clause would answer question one, meet question two, and end `blocked` — which is precisely
    // the pre-R370 behaviour, indistinguishable from the feature working unless somebody drives a
    // turn that asks twice.
    let pane = blocked_pane(&mut conn, "twice", ASKING_CLAUDE_TWICE);
    let outcome = drive(
        &mut conn,
        &sock,
        "twice",
        pane,
        Some(json!([
            {
                sprag_plugin::Consent::ASKED_KEY: "Do you want to proceed?",
                sprag_plugin::Consent::ANSWER_KEY: "Yes",
            },
            {
                sprag_plugin::Consent::ASKED_KEY: "Do you want to make this edit?",
                sprag_plugin::Consent::ANSWER_KEY: "Yes",
            },
        ])),
        None,
    )["state"]["outcome"]
        .clone();
    assert_eq!(
        outcome["state"], "converged",
        "⚠⚠⚠ one turn asked TWO different questions and the caller had decided about both — a \
         daemon that read one clause out of the list leaves this `blocked` on the second: \
         {outcome}",
    );
    assert_eq!(
        outcome[sprag_host::plugins::RUN_ANSWERED_KEY],
        2,
        "and BOTH decisions are counted, which is what a reader of a long run has instead of a \
         journal that reaches back that far: {outcome}",
    );

    // ── THE SUPERVISED RUN: one clause, and a PERSON for the question it does not cover.
    //
    // ⚠⚠⚠ THE CLAIM NO UNIT GATE CAN MAKE, for this argument more than any other. `await_person_ms`
    // buys a run the right to WAIT, and a wait that never happens is invisible in every other
    // signal: this surface swallows a key it does not declare, so a daemon that ignored the
    // argument would report exactly what the pre-R371 daemon reports — `blocked`, on the second
    // question, having answered the first. The only thing that tells the two apart is that
    // somebody answered the dialog afterwards and the run WENT ON.
    //
    // ⚠ The person is a SEPARATE CLIENT holding its own connection, typing through the same
    // `send-keys` a human at the keyboard uses. Nothing in their path belongs to the run.
    let pane = blocked_pane(&mut conn, "supervised", ASKING_CLAUDE_TWICE);
    let outcome = std::thread::scope(|watching| {
        watching.spawn(|| {
            let mut theirs = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
            // ⚠ THEY WAIT FOR THE SECOND QUESTION, not for a clock. The first is the RUN's to
            // answer, and a person who typed during it would be answering it for them — which
            // would make this gate pass with the wait never happening at all.
            let showed = wait_for(Duration::from_secs(60), || {
                theirs
                    .call(
                        "scene/query",
                        json!({
                            "session": "supervised",
                            "path": sprag_host::pane_input_path(
                                pane,
                                sprag_host::wire::FULL_TEXT_SLOT,
                            ),
                        }),
                    )
                    .ok()
                    .and_then(|text| text.as_str().map(str::to_owned))
                    .is_some_and(|text| text.contains("make this edit"))
            });
            assert!(showed, "the peer never reached its second question");
            let typed = sprag(
                &sock,
                &[
                    "send-keys",
                    "-t",
                    "supervised",
                    &pane.to_string(),
                    "-l",
                    "1",
                ],
            );
            assert!(typed.ok, "the person's keystroke: {}", typed.stderr);
        });
        drive(
            &mut conn,
            &sock,
            "supervised",
            pane,
            Some(json!([{
                sprag_plugin::Consent::ASKED_KEY: "Do you want to proceed?",
                sprag_plugin::Consent::ANSWER_KEY: "Yes",
            }])),
            Some(60_000),
        )
    })["state"]["outcome"]
        .clone();
    assert_eq!(
        outcome["state"], "converged",
        "⚠⚠⚠ the run answered the question it had a clause for, WAITED for the person on the one \
         it did not, and reached its sentinel after they answered. A daemon that swallowed \
         `await_person_ms` reports `blocked` here — the pre-R371 answer, and indistinguishable \
         from this one without a person who actually comes: {outcome}",
    );
    assert_eq!(
        outcome[sprag_host::plugins::RUN_ANSWERED_KEY],
        1,
        "⚠⚠⚠ and the tally counts what THE RUN answered — ONE. The person's answer is not the \
         machine's, and a run that claimed it has lost the distinction that makes every approval \
         on this wire traceable to whoever made it: {outcome}",
    );

    // ── THE CONTROL: the same everything, minus the consent.
    let pane = blocked_pane(&mut conn, "unanswered", ASKING_CLAUDE);
    let outcome =
        drive(&mut conn, &sock, "unanswered", pane, None, None)["state"]["outcome"].clone();
    assert_eq!(
        outcome["state"], "blocked",
        "⚠⚠⚠ WITHOUT a consent the run must answer nothing at all. This is the control that stops \
         the gate above from passing against a product that answers every dialog it meets: \
         {outcome}",
    );
    assert_eq!(
        outcome[sprag_host::plugins::RUN_ANSWERED_KEY],
        0,
        "and it says so: {outcome}",
    );
    let asking = &outcome[sprag_host::plugins::RUN_ASKING_KEY];
    assert_eq!(
        asking[sprag_host::plugins::RUN_WHY_KEY],
        "no_consent",
        "with the reason a caller can act on: {outcome}",
    );
    assert_eq!(
        asking[sprag_host::plugins::RUN_ASKED_KEY][0],
        "Do you want to proceed?",
        "and the question the shipping parser read off the pane's own screen: {outcome}",
    );
    assert_eq!(
        asking[sprag_host::plugins::RUN_CHOICES_KEY][0]["selected"],
        true,
        "and where a bare Enter would land: {outcome}",
    );
    drop(conn);
    drop(guard);
}

/// ⛔⛔⛔⛔⛔ **AND THE SUPERVISED HALF OF THAT CONTRACT WHEN THE RUN IS DRIVEN IN A PROCESS OF ITS
/// OWN** — register item 665.
///
/// # ⚠⚠⚠⚠⚠ Why its neighbour above cannot see this, when it drives the very same code
///
/// That gate submits to a daemon PINNED to `run-driver-process = off`, so its runs are
/// driven by a THREAD inside the daemon: the wait for a person parks on a `PaneChanges` backed by
/// the daemon's own pane pool, and the question is re-read out of the same process that owns the
/// pseudoterminal. Nothing crosses a socket, so nothing can be stale, absent or scoped wrong.
///
/// A driver in another process asks every one of those questions down a wire. Measured on
/// 2026-08-24 with the option flipped: this half — and only this half — came back `blocked`, with
/// `asking.why = "unattended"` and `answered = 1`. The run answered the question it had a clause
/// for, met the one it did not, waited out its whole patience with a person standing at the pane
/// typing, and concluded that nobody was there. **That is a person losing the ability to answer
/// their own loop**, which is the AI loop's central act and was the heavier of the two reasons item
/// 544's default stayed `off` for a day longer. Both were paid on 2026-08-25 and the word moved.
///
/// ⚠⚠ **THE OPTION IS TURNED ON HERE RATHER THAN LEFT TO THE DEFAULT**, and it was pinned before
/// there was a default to lean on. Pinning is what keeps this gate meaning the same thing on the
/// day the shipped word moves again — [`daemon_told`] is that rule, and this gate's partner is
/// where the round learned it.
///
/// ⚠⚠⚠ **THE STAGING CONTROL IS THE PROCESS TABLE** — register item 664's reason, and it bites
/// harder here: *the wait reached its person* and *this run was never driven anywhere else* are
/// otherwise the same green, and a config that failed to take would produce the second while
/// reading as the first.
///
/// ⚠ The person is a SEPARATE CLIENT typing through the same `send-keys` a human at the keyboard
/// uses, and they wait for the SECOND question rather than for a clock: the first is the run's to
/// answer, and a person who typed during it would answer it for them — which would make this gate
/// pass with the wait never happening at all.
#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn a_supervised_run_driven_elsewhere_waits_for_its_person_and_goes_on() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-consent-driven-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    let config = state.join("config").join("sprag");
    std::fs::create_dir_all(&config).expect("this test's own config directory");
    std::fs::write(
        config.join(sprag_host::CONFIG_FILE),
        format!(
            "[options]\n{} = \"on\"\n",
            sprag_host::options::RUN_DRIVER_PROCESS
        ),
    )
    .expect("a config a daemon will read");
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");

    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": { "name": "watched", "cmd": ["sh", "-c", ASKING_CLAUDE_TWICE] },
        }),
    )
    .expect("new_session answers");
    let pane = conn
        .call(
            "scene/query",
            json!({ "session": "watched", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the session's pane");
    // ⚠ THE BEHAVIOURAL TRIGGER, its neighbour's verbatim: wait for the DAEMON to say the pane is
    // blocked, not for a clock. A run submitted before the detector settled would be racing.
    assert!(
        wait_for(Duration::from_secs(20), || conn
            .call(
                "scene/query",
                json!({ "session": "watched", "path": mux_action_path(PANES_SLOT) }),
            )
            .ok()
            .and_then(|panes| panes.as_array().and_then(|list| list.first().cloned()))
            .is_some_and(|entry| entry["agent"]["state"] == "blocked")),
        "the daemon's own detector must call this pane blocked, or the gate is about nothing",
    );

    conn.call(
        "scene/invoke",
        json!({
            "session": "watched",
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "orchestrator",
                "pane": pane,
                "stimulus": "ping",
                "sentinel": "ANSWERED-OK",
                // Six for its neighbour's reason: a supervised run spends one iteration on the WAIT
                // itself, so the budget that fits an unattended half would end this one `exhausted`.
                "guardrails": { "max_iterations": 6, "max_seconds": 120 },
                sprag_plugin::Consents::WIRE_KEY: [{
                    sprag_plugin::Consent::ASKED_KEY: "Do you want to proceed?",
                    sprag_plugin::Consent::ANSWER_KEY: "Yes",
                }],
                sprag_plugin::Attended::WIRE_KEY: 60_000,
            },
        }),
    )
    .expect("the run is submitted");

    assert!(
        wait_for(Duration::from_secs(20), || sprag_term_processes(&sock) == 2),
        "⚠⚠ THE PREMISE: this run is driven in a process of its own, so there are two `sprag-term` \
         against this socket. Found {}, and everything below would then be a measurement of the \
         in-process driver its neighbour already gates.",
        sprag_term_processes(&sock),
    );

    // ⛔⛔⛔⛔⛔ **THE INSTRUMENT, AND WHY IT IS READ AFTER THE RUN HAS ENDED — register item 665.**
    // A red below has two possible seams and the outcome cannot tell them apart: the run's wait
    // parks on the pane moving and never hears, or it hears, re-asks, and the DAEMON goes on
    // answering with the same question. The address that separates them is the one the driver
    // itself re-reads through, `agent.<pane>`.
    //
    // ⚠⚠⚠⚠⚠ **IT MUST NOT BE READ WHILE THE RUN IS STILL WAITING, AND THAT IS A MEASUREMENT.**
    // That address is served by `AgentClock::observe`, which advances the tracker and publishes —
    // and a supervisor that publishes BUMPS THE PANE'S REVISION (register item 646), which is
    // precisely the signal the run's wait is parked on. Sampled inside the wait, this instrument
    // wakes the run it is supposed to be observing: measured 2026-08-25, the same gate went from
    // four reds in five to twenty-three greens in twenty-four the moment this call was added
    // inside the person's thread, on two different machines. **An instrument that supplies the
    // wake is not measuring the product, it is being it.**
    //
    // ⚠⚠ R58 named the mechanism, and it is worth keeping because it is not the obvious one: this
    // read raises a settle CANDIDATE, the daemon's own waker publishes it at that deadline, and a
    // publish announces on the session THE REGISTRY says holds the pane. While the pane's own
    // output was bumping the requesting scope's token instead, that announcement was the only
    // thing this wait's park was ever armed by — so the instrument was not merely early, it was
    // the entire wake path.
    let after = std::sync::Mutex::new(Value::Null);
    let outcome = std::thread::scope(|watching| {
        watching.spawn(|| {
            let mut theirs = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
            let showed = wait_for(Duration::from_secs(60), || {
                theirs
                    .call(
                        "scene/query",
                        json!({
                            "session": "watched",
                            "path": sprag_host::pane_input_path(
                                pane,
                                sprag_host::wire::FULL_TEXT_SLOT,
                            ),
                        }),
                    )
                    .ok()
                    .and_then(|text| text.as_str().map(str::to_owned))
                    .is_some_and(|text| text.contains("make this edit"))
            });
            assert!(showed, "the peer never reached its second question");
            // ⚠⚠⚠⚠⚠ **NOTHING GOES BETWEEN THE SIGHTING AND THE KEYSTROKE — register item 665, and
            // it is a MEASUREMENT rather than a style.** This person types the instant the question
            // is on the pane, which is what its thread-driven neighbour does and what a person at a
            // keyboard does. A single extra round trip here — one `runs` read, asserting the run
            // had not already ended — moved the gate from 4-red-in-5 to 6-green-in-6 on
            // 2026-08-25, twice. The window this gate is about is that narrow, so anything added
            // here is not a check: it is the repair.
            let typed = sprag(
                &sock,
                &["send-keys", "-t", "watched", &pane.to_string(), "-l", "1"],
            );
            assert!(typed.ok, "the person's keystroke: {}", typed.stderr);
            // ⛔⛔⛔⛔⛔ **THE DISCRIMINATOR — register item 665, and without it the red below names
            // TWO defects at once.** *The person's key never reached the peer* and *the peer took
            // it and the RUN never noticed* produce the same `unattended`, and only the first is
            // about the daemon's input path. The peer prints its sentinel once BOTH questions are
            // answered, so this pane going to `ANSWERED-OK` is the pane's own statement that the
            // person's half of the work is done and everything left is the run's reading of it.
            let took = wait_for(Duration::from_secs(30), || {
                theirs
                    .call(
                        "scene/query",
                        json!({
                            "session": "watched",
                            "path": sprag_host::pane_input_path(
                                pane,
                                sprag_host::wire::FULL_TEXT_SLOT,
                            ),
                        }),
                    )
                    .ok()
                    .and_then(|text| text.as_str().map(str::to_owned))
                    .is_some_and(|text| text.contains("ANSWERED-OK"))
            });
            assert!(
                took,
                "⚠⚠ THE PREMISE OF THE WHOLE GATE: the person typed and the peer did NOT move off \
                 its dialog, so the daemon's input path is what is broken and the run's reading of \
                 the pane has not been measured at all",
            );
        });
        let mut last = Value::Null;
        let finished = wait_for(Duration::from_secs(120), || {
            last = conn
                .call(
                    "scene/query",
                    json!({
                        "session": "watched",
                        "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUNS_SLOT),
                    }),
                )
                .expect("the runs slot answers");
            last.as_array()
                .and_then(|runs| runs.last().cloned())
                .is_some_and(|run| run["state"]["status"] == "done")
        });
        assert!(finished, "the run never finished: {last}");
        last.as_array()
            .and_then(|runs| runs.last().cloned())
            .expect("the run's entry")
    })["state"]["outcome"]
        .clone();

    // Sampled now, with the run over and nothing left for a wake to reach — see this instrument's
    // own note. What it answers is whether the DAEMON was still calling this pane blocked on the
    // question the person had already answered.
    *after
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = conn
        .call(
            "scene/query",
            json!({
                "session": "watched",
                "path": mux_action_path(&sprag_host::wire::agent_slot_for(pane)),
            }),
        )
        .unwrap_or(Value::Null);

    assert_eq!(
        outcome["state"],
        "converged",
        "⛔⛔⛔⛔⛔ REGISTER ITEM 665: a run driven in a process of its own answered the question \
         it had a clause for, and then could not be answered by the PERSON on the one it did not. \
         A `blocked` with `{}` = `unattended` says it waited out its whole patience while somebody \
         was typing at the pane.\n\
         ⛔ READ `a_pane_born_with_its_session_wakes_a_wait_parked_on_that_session` FIRST. That is \
         the deterministic gate for the seam this turned out to be in R58 — a pane born with its \
         session wiring its output to the REQUESTING scope's revision token, so the wait parked on \
         the session that holds it was woken only by unrelated traffic — and it answers in three \
         seconds where this one takes sixty. Its thread-driven neighbour \
         (`a_run_given_consent_answers_its_peer_over_the_wire_and_one_without_it_does_not`) makes \
         this exact claim and is green: {outcome}\n\
         ⛔ THE INSTRUMENT — what `agent.{pane}` said once the peer had provably left the dialog, \
         which is the address the run's own wait re-reads through: {}",
        sprag_host::plugins::RUN_WHY_KEY,
        after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    assert_eq!(
        outcome[sprag_host::plugins::RUN_ANSWERED_KEY],
        1,
        "⚠⚠⚠ and the tally counts what THE RUN answered — ONE. The person's answer is not the \
         machine's, and a run that claimed it has lost the distinction that makes every approval \
         on this wire traceable to whoever made it: {outcome}",
    );
    drop(conn);
    drop(guard);
}

/// ⛔⛔⛔⛔⛔ **A PANE'S OWN OUTPUT MUST WAKE THE WAITS PARKED ON THE SESSION THAT HOLDS IT** —
/// register item 665, and the seam its whole 60-second symptom turned out to be.
///
/// # ⚠⚠⚠⚠⚠ What was actually broken, measured in the daemon's own log
///
/// [`sprag_host::bump_on_dirty`]'s doc has always said `revision` *"must be the token of the
/// session the pane is being spawned INTO"*. `new_session` births its first pane through the
/// spawn door of the REQUESTING connection, whose scope is that client's default session (`"0"`
/// for a client that named none) and not the session just created. So every pane born with its
/// session bumped a token nothing about that session was parked on:
///
/// ```text
/// PROBE bump  at=6 session=watched     <- a mutation on the session (the person typing)
/// PROBE evaluate session="watched" pane=0 since=2 now=2
/// PROBE bump  at=5 session=0           <- the PANE'S OWN OUTPUT, one millisecond later
///   (nothing whatever for sixty seconds)
/// ```
///
/// A wait parked on `watched` was therefore woken only by UNRELATED traffic on that session, and
/// answered only if such traffic happened to land after the pane had moved. That is why item 665
/// read as a race: a supervised run's wait for its person was woken by the person's own keystroke
/// (a mutation) and then never again by the peer's reply to it.
///
/// # ⚠⚠⚠ Why this gate and not the run-level one alone
///
/// The run-level gate above measures the same defect through a driver, a peer, a detector and a
/// person, and it is a coin toss which of two events lands first — so it can only ever say
/// *something in this stack is late*. This one holds nothing but the claim: **a pane speaks, and
/// what is parked on its session hears it.** No run, no agent, no keystroke, and nothing that
/// mutates the session at all between the park and the answer — which is what makes a failure here
/// name one seam.
///
/// ⚠⚠ **THE STAGING CONTROL IS THE PANE'S OWN COUNTER, READ AFTER THE WAIT.** *The wake was lost*
/// and *the pane never spoke* are otherwise the same red, and only the first is about this seam.
/// `pane.<id>.revision` is a pure counter read — it observes no agent and publishes nothing — so
/// asking it afterwards cannot be the thing that supplies the answer.
#[test]
fn a_pane_born_with_its_session_wakes_a_wait_parked_on_that_session() {
    let sock = socket_path();
    let state = isolated_state_home(&sock);
    let _ = std::fs::remove_dir_all(&state);
    let guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");

    // ⚠ THE FIXTURE SPEAKS TWICE, ON ITS OWN CLOCK AND NOBODY ELSE'S. The first line says the pane
    // is alive; the second lands well after the park below, with no client having asked for it —
    // which is the only kind of output that can prove the hook rather than the traffic around it.
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "spoken",
                "cmd": ["sh", "-c", "printf 'FIRST-LINE\\n'; sleep 3; printf 'LATE-LINE\\n'; exec cat"],
            },
        }),
    )
    .expect("new_session answers");
    let pane = conn
        .call(
            "scene/query",
            json!({ "session": "spoken", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the pane list answers")
        .as_array()
        .and_then(|panes| panes.first().cloned())
        .and_then(|pane| pane["id"].as_u64())
        .expect("the session's pane");
    let text = |conn: &mut HostConn| {
        conn.call(
            "scene/query",
            json!({
                "session": "spoken",
                "path": pane_input_path(pane, sprag_host::wire::FULL_TEXT_SLOT),
            }),
        )
        .ok()
        .and_then(|text| text.as_str().map(str::to_owned))
        .unwrap_or_default()
    };
    assert!(
        wait_for(Duration::from_secs(10), || text(&mut conn)
            .contains("FIRST-LINE")),
        "the fixture never started, so nothing below would be about a pane that speaks",
    );
    let revision = |conn: &mut HostConn| {
        conn.call(
            "scene/query",
            json!({
                "session": "spoken",
                "path": pane_input_path(pane, sprag_host::wire::PANE_REVISION_SLOT),
            }),
        )
        .expect("the pane's revision answers")
        .as_u64()
        .expect("the revision is a number")
    };
    let since = revision(&mut conn);
    assert!(
        !text(&mut conn).contains("LATE-LINE"),
        "⚠⚠ THE PREMISE: the second line must still be TO COME when the wait is parked, or the \
         park is asking about a move that already happened and would answer without any hook at \
         all",
    );

    // ⚠⚠⚠⚠⚠ **A SECOND CONNECTION, AND NOTHING IS ASKED OF THE DAEMON WHILE THIS IS OUTSTANDING.**
    // A `HostConn` carries one question at a time (`sprag_host::remote_access::RemotePaneAccess`
    // opens a park connection of its own for exactly this reason), and — more to the point here —
    // every INVOKE on this session bumps its revision through the dispatch funnel. A gate that
    // chattered while parked would be supplying the very wake it is measuring.
    let mut parking = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect");
    let parked = parking
        .begin(
            sprag_host::wire::PANE_WAIT_REVISION_METHOD,
            json!({
                "session": "spoken",
                sprag_host::wire::PANE_PARAM: pane,
                sprag_host::wire::SINCE_PARAM: since,
            }),
        )
        .expect("the park is accepted");
    let answer = parking
        .settle(&parked, Duration::from_secs(15))
        .expect("the park did not fault");

    // THE STAGING CONTROL, read now that the wait is over: the pane really did speak, and its own
    // counter really did move past what the wait named. Both are pure reads.
    let said = text(&mut conn);
    let now = revision(&mut conn);
    assert!(
        said.contains("LATE-LINE"),
        "⚠⚠ THE CONTROL: the fixture never printed its second line, so this measured a silent \
         pane and not a lost wake. What it showed: {said:?}",
    );
    assert!(
        now > since,
        "⚠⚠ THE CONTROL: `{}` never moved past {since}, so the pane's own counter is what is \
         broken and the wake path has not been measured at all",
        sprag_host::wire::PANE_REVISION_SLOT,
    );
    assert_eq!(
        answer
            .as_ref()
            .and_then(|answer| answer[sprag_host::wire::PANE_REVISION_FIELD].as_u64()),
        Some(now),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 665: this pane printed a line entirely of its own accord, its \
         own revision moved from {since} to {now}, and the `{}` parked on the session that HOLDS \
         it was never woken. A pane born with its session must bump that session's token — \
         `sprag_host::bump_on_dirty`'s own contract — or every wait parked there sleeps through \
         the pane it is about and is answered only by unrelated traffic. The park answered: \
         {answer:?}",
        sprag_host::wire::PANE_WAIT_REVISION_METHOD,
    );
    drop(parking);
    drop(conn);
    drop(guard);
}

/// ⚠⚠⚠ **A PERSON ANSWERS A BLOCKED AGENT FROM THE COMMAND LINE, IN THE AGENT'S OWN WORDS** —
/// R369's claim at the shell, where `sprag agent` had been able to SHOW the dialog for a round and
/// the only thing to do about it was `sprag send-keys`.
///
/// # ⚠⚠ Why the peer must report which BYTE moved it
///
/// The claim is about which keystrokes the daemon sent, so a gate reading only the outcome would
/// pass for a run that typed a digit it did not need. This peer prints `took <option> via <byte>`,
/// and it prints nothing at all if no key reaches it — so the pane is the witness for both halves.
///
/// ⚠ The words are built by `printf`'s FORMAT rather than written out: this script is the pane's
/// argv, and a literal `took 3 via 51` in it would sit in the pane's own text where an assertion
/// could read it instead of the behaviour.
///
/// # ⚠⚠⚠ The safety claim
///
/// The peer's marker starts on option 1 (`Yes`). The person authorises option 3. A machine that
/// answered by pressing the one key with a known landing place — Enter — would APPROVE what they
/// declined. `via 51` is the digit `3`, and `via 10` would be that approval.
#[test]
fn the_cli_answers_a_blocked_agent_in_the_agents_own_words() {
    let (_host, sock) = spawn_host_running(&[
        "sh",
        "-c",
        "stty -icanon -echo; printf '\\033]2;\\342\\234\\263 Claude Code\\007'; sel=1; \
         d() { printf '\\033[2J\\033[H'; printf 'Do you want to proceed?\\r\\n'; i=1; \
         for l in 'Yes' 'Yes, and do not ask again' 'No, and tell me why'; do \
         if [ \"$i\" = \"$sel\" ]; then printf '\\342\\235\\257 '; else printf '  '; fi; \
         printf '%s. %s\\r\\n' \"$i\" \"$l\"; i=$((i+1)); done; }; d; \
         while :; do k=$(dd bs=1 count=1 2>/dev/null | od -An -tu1 | tr -d ' \\n'); \
         [ -n \"$k\" ] || exit 0; case \"$k\" in 49|50|51) sel=$((k-48));; esac; \
         printf '\\033[2J\\033[H'; printf 'took %s via %s\\r\\n' \"$sel\" \"$k\"; exec cat; done",
    ]);
    wait_for_pane_text(&sock, "3. No, and tell me why");

    // ⚠ THE READING FIRST, because that is the state a person is in when they reach for this verb:
    // `agent` says the pane is blocked and shows the menu, and until R369 that was the end of what
    // the shell could do about it.
    let seen = sprag(&sock, &["agent", "0"]);
    assert!(
        seen.ok && seen.stdout.contains("Do you want to proceed?"),
        "the verb that reports a blocked agent shows the question: {:?}",
        seen.stdout,
    );

    // ⚠⚠⚠ THE REFUSAL GOES FIRST because it types NOTHING, which leaves the dialog up for the
    // answer below. `and` is carried by `Yes, and do not ask again` AND by `No, and tell me why` —
    // a grant and a refusal — so a first-match policy would pick between opposites for the person.
    let ambiguous = sprag(
        &sock,
        &["answer-pane", "0", "--asked", "proceed", "--answer", "and"],
    );
    assert!(
        ambiguous.ok,
        "a consent that authorises nothing is a REPORT, not a command-line error: {}",
        ambiguous.stderr,
    );
    assert!(
        ambiguous
            .stdout
            .contains("more than one option carries the authorised answer"),
        "⚠⚠⚠ and it says WHICH reason it was, as the sentence that names the remedy — \
         a person who cannot tell `my words matched nothing` from `they matched twice` cannot fix \
         either: {:?}",
        ambiguous.stdout,
    );
    let untouched = sprag(&sock, &["capture-pane", "0", "-p"]);
    assert!(
        !untouched.stdout.contains("via 4")
            && !untouched.stdout.contains("via 5")
            && !untouched.stdout.contains("via 1"),
        "⚠⚠⚠ AND NOT ONE KEY REACHED THE PANE. The peer prints `took <option> via <byte>` for \
         anything it receives, and a refusal that typed first would be the product deciding and \
         then apologising: {:?}",
        untouched.stdout,
    );

    // ...AND THE ANSWER, which is what the verb exists for.
    let answered = sprag(
        &sock,
        &[
            "answer-pane",
            "0",
            "--asked",
            "Do you want to proceed?",
            "--answer",
            "No, and tell me why",
        ],
    );
    assert!(answered.ok, "answer-pane succeeded: {}", answered.stderr);
    assert!(
        answered.stdout.contains("converged") && answered.stdout.contains("No, and tell me why"),
        "⚠⚠ the run is over in one answer and the record names the option in WORDS — a number \
         cannot be audited once the dialog is gone: {:?}",
        answered.stdout,
    );

    wait_for_pane_text(&sock, "took 3 via 51");
    let witness = sprag(&sock, &["capture-pane", "0", "-p"]);
    assert!(
        witness.stdout.contains("took 3 via 51"),
        "⚠⚠⚠ THE PEER TOOK OPTION 3, MOVED BY ITS DIGIT. Its own marker was on `Yes`, so a machine \
         that pressed the key with the known landing place would have approved the command this \
         person declined: {:?}",
        witness.stdout,
    );
    assert!(
        !witness.stdout.contains("via 10"),
        "⚠⚠⚠ AND NO ENTER FOLLOWED IT — the peer left the question on the digit alone, so an Enter \
         sent anyway would land on whatever it shows next: {:?}",
        witness.stdout,
    );

    // ⚠ BOTH NEEDLES ARE THE SHELL'S TO DEMAND TOO, and each refusal says what the needle is FOR
    // rather than only that it is missing.
    let no_question = sprag(&sock, &["answer-pane", "0", "--answer", "Yes"]);
    assert!(!no_question.ok);
    assert!(
        no_question.stderr.contains("WHICH QUESTION"),
        "a consent with no question answers whatever the pane happens to be showing: {}",
        no_question.stderr,
    );
    let no_option = sprag(&sock, &["answer-pane", "0", "--asked", "proceed"]);
    assert!(!no_option.ok);
    assert!(
        no_option.stderr.contains("WHICH OPTION"),
        "and one with no option makes every real menu ambiguous: {}",
        no_option.stderr,
    );
}

/// ⚠⚠ **A PANE BLOCKED ON SOMETHING THIS DAEMON CANNOT READ AS A MENU STILL SAYS SO, AND SAYS THE
/// REMEDY IS A PERSON** — the branch beside the one above, and the one an absence would hide.
///
/// # ⚠⚠⚠ Why silence here would be the worst answer available
///
/// `blocked` with no question is a REAL state with a real remedy: an agent can stop on a free-text
/// prompt, a paged view, or a confirmation drawn as prose, and no consent can name an option a
/// screen does not offer. Said nothing about, it is indistinguishable from a daemon too old to look
/// — which is exactly the reading `WIRE_PROTOCOL` 28 exists to make impossible, and the sentence is
/// how this mouth keeps its side of that.
///
/// ⚠ The state is REPORTED rather than scraped, because that is the only way to be blocked with no
/// menu on purpose: the screen rule that produces `blocked` is the choice-list one, so a scraped
/// verdict always has a question behind it. A report outranks the screen, which is what lets this
/// gate build the state at all.
#[test]
fn the_cli_says_a_blocked_pane_it_cannot_read_needs_a_person() {
    let (_host, sock) = spawn_host_running(&["cat"]);
    let reported = sprag(
        &sock,
        &[
            "report-agent",
            "blocked",
            "--pane",
            "0",
            "--source",
            "hook",
            "--name",
            "claude",
        ],
    );
    assert!(reported.ok, "report-agent succeeded: {}", reported.stderr);

    let said = sprag(&sock, &["agent", "0"]);
    assert!(said.ok, "agent 0 succeeded: {}", said.stderr);
    assert!(
        said.stdout.contains("blocked"),
        "the verdict is the reported one: {:?}",
        said.stdout,
    );
    assert!(
        said.stdout.contains("could not read as a menu")
            && said.stdout.contains("look at the pane yourself"),
        "⚠⚠⚠ a blocked pane with no readable question must SAY that the daemon looked and could \
         not read one, and that the remedy is a person. Silence would read as `nothing more is \
         known`, which is what an older daemon says: {:?}",
        said.stdout,
    );
    assert!(
        !said.stdout.contains("answer-pane"),
        "⚠⚠ and it must NOT offer the answering verb: there is no option for a consent to name, \
         so the only thing that advice could produce is a caller writing needles that cannot \
         match: {:?}",
        said.stdout,
    );

    // ⚠⚠ AND ANSWERING IT ANYWAY IS REFUSED WITH THAT SAME REMEDY — `Refusal::Unreadable`, the one
    // arm of the six that is not about the consent at all. A caller who ignores the advice above
    // must meet the same sentence from the act itself rather than a match failure, because
    // *"your needles were wrong"* would send them to rewrite words that were never the problem.
    //
    // ⚠ AND IT IS DRIVEN IN THE `--flag=value` SPELLING, R350's rule: a flag has TWO spellings and
    // the joined one is the only way to pass a needle that begins with a dash — which an agent's
    // options frequently do. Everything above uses the separated form, so without this the second
    // spelling is a branch no test builds.
    let anyway = sprag(
        &sock,
        &["answer-pane", "0", "--asked=proceed", "--answer=Yes"],
    );
    assert!(
        anyway.ok,
        "an unanswerable pane is a REPORT, not a command-line error: {}",
        anyway.stderr,
    );
    assert!(
        anyway.stdout.contains("blocked")
            && anyway.stdout.contains("cannot read as a numbered menu")
            && anyway.stdout.contains("hand the pane to a person"),
        "⚠⚠⚠ the act's own refusal names the remedy, and the remedy is a PERSON — not a rewritten \
         needle: {:?}",
        anyway.stdout,
    );
}

/// ⛔⛔⛔⛔ **`orchestrate --pane` TAKES A NAME, LIKE EVERY OTHER PANE ARGUMENT ON THIS CLI** —
/// register item 542.
///
/// # ⚠⚠⚠⚠⚠ Why the one verb that refused a name is the worst one to refuse it
///
/// Every other pane-taking verb resolves through `resolve_pane`, and so does the MCP surface. This
/// one handed its flags straight to `sprag_rpc::build_call`, whose grammar declares `pane` as an
/// `int` — so `--pane wz-inner` was a TYPE ERROR. It is the verb whose target a person types least
/// often and remembers least well, and an operator paid for that live.
///
/// ⚠⚠ **And a number is not merely inconvenient here, it is wrong more often**: `--pane` used to be
/// read against the CURRENT window, so an operator standing on another window is told `no pane 8 in
/// this workspace` about a number that is correct somewhere else. A NAME does not have that failure
/// mode, which is the argument for fixing this rather than documenting it.
///
/// # ⚠⚠⚠ The pair is the claim
///
/// A CLI that shrugged at anything would pass the first half. So the second half hands it a name no
/// pane has, and requires a refusal that says so — the acceptance means nothing without it.
#[test]
fn orchestrate_starts_a_run_on_a_pane_named_rather_than_numbered() {
    let (_guard, sock, pane) = daemon_with_one_pane("orchestrate-by-name");
    let named = sprag(
        &sock,
        &["rename-pane", &pane.to_string(), "driven", "-t", "work"],
    );
    assert!(named.ok, "naming the pane failed: {}", named.stderr);

    // ── THE CLAIM: the NAME is accepted where the number always was ──────────────────────────
    let started = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            "driven",
            "--stimulus",
            "echo named",
            "--max-iterations",
            "1",
            "--wait",
        ],
    );
    assert!(
        started.ok,
        "⛔⛔⛔⛔ REGISTER ITEM 542: `orchestrate` refused a pane NAME that every other verb on \
         this CLI takes. The grammar calls this argument an `int`, so a name is a type error before \
         the daemon is ever asked — and this is the one verb whose target a person types least \
         often and remembers least well. Refused with: {}",
        started.stderr,
    );

    // ── THE CONTROL: a name no pane has must be REFUSED, and say so ─────────────────────────
    let nobody = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            "no-such-pane-here",
            "--stimulus",
            "echo named",
            "--max-iterations",
            "1",
        ],
    );
    assert!(
        !nobody.ok,
        "⚠⚠⚠ THE CONTROL: a name nothing holds must be refused. A CLI that accepted it would make \
         the claim above a statement about a shell that shrugs at whatever it is handed, which is \
         exactly the failure mode a derived surface has. Answered: {} {}",
        nobody.stdout, nobody.stderr,
    );
}

/// ⛔⛔⛔⛔ **`orchestrate` STARTS A RUN ON A PANE OF A WINDOW THAT IS NOT THE CURRENT ONE** —
/// register item 686, and the line AFTER item 542's.
///
/// # ⚠⚠⚠⚠⚠ What 542 paid for, and what it left standing
///
/// Item 542 made this verb take a pane NAME, and that done-when is true: `resolve_pane` answers a
/// name from anywhere in the scoped SESSION. What it left standing is the step AFTER the answer —
/// `orchestrate` kept `site.id` and threw `site.window` away, then sent the request with the SCOPE
/// alone. The daemon's `require_pane_in` reads `PluginWorld::has_pane`, which is ONE WINDOW's pane
/// pool, so a request that does not say which window is answered against the CURRENT one, and a
/// correctly-resolved pane came back `no pane 13 in this workspace`.
///
/// That is why the refusal was diagnostic rather than merely wrong: called BY NAME, it named a
/// NUMBER. The resolver had done its job; the sentence came from a mouth one layer further in.
///
/// # ⚠⚠⚠⚠ Why four hundred gates did not see it, and what this fixture does differently
///
/// Every fixture that had ever driven this verb held ONE window, and an operator standing in the
/// only window there is is always standing in the current one. `plugins.rs` asserts this refusal
/// twice, and both times about a case where refusing is RIGHT (a pane that does not exist; a pane
/// of somebody else's workspace) — nobody drove the other side.
///
/// ⇒ the sister of what item 684 taught: **two scopes that can diverge cannot be told apart from
/// inside one of them.** So this fixture stands up TWO windows, leaves the target in the one that
/// is not current, and asserts that premise before making any claim that rests on it.
///
/// # ⚠⚠⚠ Both spellings, because they died here for two different reasons
///
/// A NAME reached `resolve_pane`, resolved, and died at the window that was dropped. A NUMBER never
/// reached the resolver at all — the loop guarded on `raw.parse::<u64>().is_err()` — so it went out
/// as typed and was read against the current window. Two spellings, one place, two causes; a gate
/// that drove only the name would leave the number free to regrow.
#[test]
fn orchestrate_starts_a_run_on_a_pane_of_a_window_that_is_not_the_current_one() {
    let (_guard, sock, pane) = daemon_with_one_pane("orchestrate-elsewhere");
    let numbered = pane.to_string();
    let named = sprag(&sock, &["rename-pane", &numbered, "driven", "-t", "work"]);
    assert!(named.ok, "naming the pane failed: {}", named.stderr);

    // ── THE FIXTURE'S PREMISE, MADE AND THEN ASSERTED ───────────────────────────────────────
    // `new-window` selects what it makes, so after this the session's current window is `spare`
    // and `driven` is one window over. Every claim below is vacuous without that, which is why it
    // is measured here rather than assumed from the call that was supposed to cause it.
    assert!(
        sprag(&sock, &["new-window", "-t", "work", "spare"]).ok,
        "the second window is the whole fixture",
    );
    let here = sprag(&sock, &["panes", "-t", "work"]);
    assert!(here.ok, "panes -t work: {}", here.stderr);
    assert!(
        !pane_ids_in(&here.stdout).contains(&pane),
        "⛔ THE PREMISE: `panes` lists the CURRENT window, and this gate is only about a pane that \
         is NOT in it. Pane {pane} still listed here means the current window never moved, and \
         every claim below would then be passing in a one-window world: {}",
        here.stdout,
    );

    // ── THE CLAIM, SPELLED BY NAME ──────────────────────────────────────────────────────────
    let by_name = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            "driven",
            "--stimulus",
            "echo elsewhere",
            "--max-iterations",
            "1",
        ],
    );
    assert!(
        by_name.ok,
        "⛔⛔⛔⛔ REGISTER ITEM 686: `orchestrate` resolved this pane by NAME and then threw away \
         WHICH WINDOW it had been found in, so the daemon read the id against the current window \
         and refused a pane that exists. A refusal naming a NUMBER when the request named a NAME \
         is the signature. Answered: {} / {}",
        by_name.stdout, by_name.stderr,
    );

    // ── THE SAME CLAIM, SPELLED AS A NUMBER ─────────────────────────────────────────────────
    // A number never reached the resolver at all, so it failed for a DIFFERENT reason than the
    // name did. One fix, two spellings, and neither is evidence for the other.
    let by_number = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            &numbered,
            "--stimulus",
            "echo elsewhere",
            "--max-iterations",
            "1",
        ],
    );
    assert!(
        by_number.ok,
        "⛔⛔⛔⛔ REGISTER ITEM 686: a pane's NUMBER must reach the daemon by the same road its \
         NAME does. This one was passed through as typed and read against the current window. \
         Answered: {} / {}",
        by_number.stdout, by_number.stderr,
    );

    // ── AND THE RUNS LANDED ON THAT PANE, RATHER THAN MERELY BEING ACCEPTED ─────────────────
    let runs = sprag(&sock, &["runs", "-t", "work"]);
    assert!(runs.ok, "runs -t work: {}", runs.stderr);
    assert_eq!(
        runs.stdout.matches(&format!("pane={pane}")).count(),
        2,
        "⚠⚠⚠ AN ACCEPTED REQUEST IS NOT A STARTED RUN. Both spellings must have put a run on pane \
         {pane} ITSELF — an exit code says the shell was happy, and this says the daemon acted on \
         the pane the operator named: {}",
        runs.stdout,
    );

    // ── THE CONTROL: a pane no window holds is still refused ────────────────────────────────
    // Carrying the window must WIDEN what resolves, not stop the verb checking. Without this the
    // two claims above would pass just as well for a CLI that had simply stopped looking.
    let nowhere = sprag(
        &sock,
        &[
            "orchestrate",
            "orchestrator",
            "-t",
            "work",
            "--pane",
            "9999",
            "--stimulus",
            "echo elsewhere",
            "--max-iterations",
            "1",
        ],
    );
    assert!(
        !nowhere.ok,
        "⚠⚠⚠ THE CONTROL: reaching past the current WINDOW must not become reaching past the \
         SESSION. A pane nothing holds is still a refusal, or the claims above are about a verb \
         that stopped looking: {} / {}",
        nowhere.stdout, nowhere.stderr,
    );
}

/// ⚠⚠⚠⚠⚠ **AND SO DOES AN ANSWER** — register item 687's CLI half, the verb next door to the one
/// above.
///
/// # ⚠⚠ There IS a ratchet over this verb already, and it could not see this
///
/// `every_verb_the_usage_says_takes_a_pane_reaches_one_a_window_over` drives `answer-pane` against
/// a live pane one window over and has since R369. It asserts that the refusal is not one of FOUR
/// named spelling sentences — which is the right assertion for the defect R312 paid off, and blind
/// to a verb that never called the resolver at all: `answer-pane` handed what was typed straight to
/// `build_call`, whose grammar declares this argument an `int`, and the sentence that came back was
/// none of those four. **Measured, not reasoned — see the mutation record in register item 687:
/// reverting this fix leaves that ratchet GREEN and turns this gate red.**
///
/// ⇒ the rule item 682 wrote down, arriving from the other side: a gate that names the sentences it
/// knows about cannot see a failure that speaks a new one. This one reads the OUTCOME instead.
#[test]
fn answer_pane_reaches_a_pane_of_a_window_that_is_not_the_current_one() {
    let (_guard, sock, pane) = daemon_with_one_pane("answer-elsewhere");
    let numbered = pane.to_string();
    assert!(
        sprag(&sock, &["rename-pane", &numbered, "asked", "-t", "work"]).ok,
        "naming the pane is what makes the first spelling below a NAME",
    );

    // ── THE FIXTURE'S PREMISE, MADE AND THEN ASSERTED ───────────────────────────────────────────
    assert!(
        sprag(&sock, &["new-window", "-t", "work", "spare"]).ok,
        "the second window is the whole fixture",
    );
    let here = sprag(&sock, &["panes", "-t", "work"]);
    assert!(here.ok, "panes -t work: {}", here.stderr);
    assert!(
        !pane_ids_in(&here.stdout).contains(&pane),
        "⛔ THE PREMISE: `panes` lists the CURRENT window, and pane {pane} still being in it would \
         make every claim below pass in a one-window world: {}",
        here.stdout,
    );

    // ── THE CLAIM, IN BOTH SPELLINGS ────────────────────────────────────────────────────────────
    // The pane is a shell and is not asking anything, deliberately: no agent manifest claims it, so
    // the run types NOTHING and the pane is left as it was. What is under test is the ADDRESSING,
    // and an answer that had to find a dialog first would be measuring the detector.
    let answered = |spelling: &str| {
        sprag(
            &sock,
            &[
                "answer-pane",
                spelling,
                "--asked",
                "marker",
                "--answer",
                "marker",
                "-t",
                "work",
            ],
        )
    };
    let by_name = answered("asked");
    assert!(
        by_name.ok,
        "⛔⛔⛔⛔ REGISTER ITEM 687: `answer-pane` must resolve a pane by NAME and say which window \
         it was found in — the surface that SHOWS a person the dialog reaches every window of the \
         session, so the verb that acts on what they read cannot be the one that does not: {} / {}",
        by_name.stdout, by_name.stderr,
    );
    let by_number = answered(&numbered);
    assert!(
        by_number.ok,
        "⛔⛔⛔⛔ REGISTER ITEM 687: and a NUMBER must reach the daemon by the same road its NAME \
         does, rather than being passed through as typed and read against the current window: \
         {} / {}",
        by_number.stdout, by_number.stderr,
    );

    // ── AND BOTH RUNS LANDED ON THAT PANE, rather than merely being accepted ─────────────────────
    // The daemon labels an answer `answer pane=<id>`, so this reads the pane the DAEMON acted on.
    let runs = sprag(&sock, &["runs", "-t", "work"]);
    assert!(runs.ok, "runs -t work: {}", runs.stderr);
    assert_eq!(
        runs.stdout.matches(&format!("answer pane={pane}")).count(),
        2,
        "⚠⚠⚠ AN ACCEPTED REQUEST IS NOT A STARTED RUN. Both spellings must have put an answer run \
         on pane {pane} ITSELF: {}",
        runs.stdout,
    );

    // ── THE CONTROL: a pane no window holds is still refused ────────────────────────────────────
    let nowhere = answered("9999");
    assert!(
        !nowhere.ok,
        "⚠⚠⚠ THE CONTROL: reaching past the current WINDOW must not become reaching past the \
         SESSION: {} / {}",
        nowhere.stdout, nowhere.stderr,
    );
}

/// The name of the window `session` is CURRENTLY showing — the fact this gate pins and then keeps
/// asserting, read from the daemon rather than assumed from the call that was supposed to set it.
fn current_window(sock: &Path, session: &str) -> String {
    let mut conn = HostConn::connect(sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/query",
        json!({ "session": session, "path": mux_action_path(WINDOWS_SLOT) }),
    )
    .expect("the window list")
    .as_array()
    .into_iter()
    .flatten()
    .find(|window| window["current"].as_bool().unwrap_or(false))
    .and_then(|window| window["name"].as_str())
    .expect("a session always has a current window")
    .to_owned()
}

/// ⚠⚠⚠⚠⚠ **A RUN DRIVES ITS OWN PANE WHILE THE SESSION IS LOOKING SOMEWHERE ELSE** — register
/// item 690, the third and deepest layer of the family items 686 and 687 paid off at the mouths.
///
/// # What was wrong: the mouth learned the window and the HAND never did
///
/// Item 686 gave `orchestrate` the window, item 687 gave it to the MCP tools and to `answer-pane`,
/// and a run then STARTED correctly on a pane one window over. It still could not be DRIVEN there.
/// A run's driver is a process of its own (`run-driver-process` defaults to `on`); it was handed
/// `--drive <id>`, `-t <session>` and the request map, and **`drive.rs` contained no window at
/// all** — so every pane read, every keystroke and every progress report it made resolved against
/// whichever window the session happened to be showing.
///
/// Measured on a live loop, 2026-08-25: run 23 died `failed at reflecting: there is no pane 23`
/// while pane 23 was `idle seq=12` and the session's current window was `pinion`. Nothing was
/// wrong with the pane, the run, or the request that started it.
///
/// # ⚠⚠⚠⚠ Why TWO WINDOWS is not enough, and what this fixture pins
///
/// Which window is CURRENT is what decides the answer, so a fixture that does not fix it lets the
/// RUNNER fix it — and a gate whose premise is set by whatever ran last is measuring the runner
/// (the rule item 666 wrote down). So the current window is pinned to `spare`, which is NOT the
/// run's window, and the pin is asserted **twice**: before the run is submitted, and again after
/// the driver has demonstrably typed into the pane. The second assertion is the one that matters —
/// it says the pin was still holding at the moment the driving happened.
#[test]
fn a_run_drives_its_pane_while_the_session_is_looking_at_another_window() {
    let sock = socket_path();
    let state = std::env::temp_dir().join(format!(
        "sprag-drive-elsewhere-{}-{:?}",
        std::process::id(),
        std::thread::current().id(),
    ));
    let _ = std::fs::remove_dir_all(&state);
    let _guard = DaemonGuard {
        sock: sock.clone(),
        state: state.clone(),
    };
    spawn_daemon(&sock, &state);
    assert!(
        wait_for(Duration::from_secs(10), || sprag(&sock, &["ls"]).ok),
        "the daemon never started serving",
    );

    // The loop's peer: a stand-in agent that announces itself and echoes, so the run gets PAST its
    // readiness barrier and injects — which is the first thing a driver does that touches the pane.
    let mut conn = HostConn::connect(&sock, Duration::from_secs(5)).expect("connect to the daemon");
    conn.call(
        "scene/invoke",
        json!({
            "path": mux_action_path(NEW_SESSION_ACTION),
            "args": {
                "name": "work",
                "cmd": ["sh", "-c",
                        "stty -echo; printf 'AGENT-READY\\n'; while read l; do printf '%s\\n' \"$l\"; done"],
            },
        }),
    )
    .expect("new_session answers");
    let listed = conn
        .call(
            "scene/query",
            json!({ "session": "work", "path": mux_action_path(PANES_SLOT) }),
        )
        .expect("the session's panes");
    let pane = listed
        .as_array()
        .and_then(|panes| panes.first())
        .and_then(|pane| pane["id"].as_u64())
        .unwrap_or_else(|| panic!("the session's first pane id: {listed}"));
    let home = current_window(&sock, "work");

    // ── THE PIN, MADE AND THEN ASSERTED ─────────────────────────────────────────────────────────
    // `new-window` selects what it makes, so after this the session is showing `spare` and the
    // run's pane is one window over. Both halves are measured, because a fixture that only ARRANGED
    // this would be trusting the very call whose effect is the premise.
    assert!(
        sprag(&sock, &["new-window", "-t", "work", "spare"]).ok,
        "the second window is the whole fixture",
    );
    assert_eq!(
        current_window(&sock, "work"),
        "spare",
        "⛔ THE PIN: the session must be looking at `spare`, which is NOT the run's window — \
         everything below passes trivially if the session is still on {home}",
    );
    let here = sprag(&sock, &["panes", "-t", "work"]);
    assert!(here.ok, "panes -t work: {}", here.stderr);
    assert!(
        !pane_ids_in(&here.stdout).contains(&pane),
        "⛔ THE PIN, the other way round: `panes` lists the CURRENT window, and pane {pane} being \
         in it would mean the run and the person are looking at the same place: {}",
        here.stdout,
    );

    // ── THE RUN, submitted against the window that HOLDS the pane ───────────────────────────────
    // Over the wire rather than through `sprag orchestrate`, deliberately: the mouths already have
    // gates of their own (items 686 and 687), and what is under test here is what happens AFTER a
    // correctly addressed request is accepted — the driver process the daemon then spawns.
    conn.call(
        "scene/invoke",
        json!({
            "session": "work",
            sprag_rpc::WINDOW_PARAM: home,
            "path": sprag_host::wire::plugins_path(sprag_host::plugins::RUN_ACTION),
            "args": {
                "plugin": "ai_loop",
                "pane": pane,
                "agent": "claude",
                "north_star": "a run drives the pane it was given, wherever the person is looking",
                "milestone": "get past readiness and inject while the session is elsewhere",
                "reference": "register item 690",
                "ready_when": { "match": "shows", "marker": "AGENT-READY" },
                // ⚠ The stand-in paints only whole lines, so a delivery cannot be confirmed on
                // screen before the newline that submits it.
                "shows_prompt": false,
                "guardrails": { "max_iterations": 100000, "max_seconds": 3000 },
            },
        }),
    )
    .expect("the loop is submitted");
    drop(conn);

    // ── THE CLAIM: the driver READ the pane and TYPED into it, one window over ───────────────────
    // A delivery is the product's own proof that the whole chain worked: the driver resolved the
    // pane, read its screen until the readiness marker was there, and wrote a prompt into it. Under
    // the defect none of that is reachable — the first read answers `there is no pane N`.
    let mut last = String::new();
    let delivered = wait_for(Duration::from_secs(60), || {
        last = sprag(&sock, &["runs", "-t", "work"]).stdout;
        last.contains("prompt(s) delivered")
    });
    assert!(
        delivered,
        "⛔⛔⛔⛔ REGISTER ITEM 690: the run's driver never typed into pane {pane}. It is a process \
         of its own and was told the session but not the WINDOW, so every pane request it made was \
         read against `spare` — which does not hold this pane. The row: {last}",
    );
    assert!(
        !last.contains("there is no pane"),
        "⛔⛔⛔⛔ REGISTER ITEM 690, in the words the live failure used: a run must not be told its \
         own pane does not exist while it is alive one window over: {last}",
    );

    // ── AND THE PIN WAS STILL HOLDING WHILE THAT HAPPENED ───────────────────────────────────────
    // ⚠ This is the assertion the fixture exists for. Read AFTER the driving rather than before it:
    // a pin asserted only at the start would pass on a run that succeeded because something moved
    // the session back onto the run's window in between, which is the coincidence item 684 warned
    // about — two sources that happen to agree cannot say which one was used.
    assert_eq!(
        current_window(&sock, "work"),
        "spare",
        "⛔ THE PIN MUST HAVE HELD THROUGHOUT: if the session moved onto the run's window while the \
         driver was working, the claim above is about a one-window world after all",
    );
    let still = sprag(&sock, &["panes", "-t", "work"]);
    assert!(
        !pane_ids_in(&still.stdout).contains(&pane),
        "⛔ AND THE PANE WAS NEVER IN THE CURRENT WINDOW: {}",
        still.stdout,
    );
}
