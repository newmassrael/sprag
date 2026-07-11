//! `WireHost` — the GUI as a pure wire client of a `sprag-term` host process
//! (topology B).
//!
//! [`WireHost`] implements the same [`HostClient`] protocol the in-process
//! [`Host`](sprag_host::Host) does, but reaches the panes over an RPC socket to a
//! `sprag-term` host PROCESS instead of an in-process `Workspace`. The GUI holds it
//! as a `Box<dyn HostClient>` (`terminal::use_terminal`), so every pane call site is
//! byte-identical to the in-process path — the R109 seam paying off across the
//! process boundary.
//!
//! ## What runs where
//!
//! * A `sprag-term` host owns the `Workspace` + PTYs. The GUI either SPAWNS one as a
//!   child (the default — a per-instance socket, reaped on exit / GUI death) or
//!   ATTACHES to one an operator launched (`SPRAG_GUI_HOST_SOCK=<path>`). The
//!   tmux/mosh split: server owns the terminals, client draws them.
//! * The GUI reads each pane's cells + input over the wire. Two connections avoid
//!   head-of-line blocking: a background POLL connection parks on the async
//!   `scene/waitFor` change-notification, and a REQUEST connection serves the UI
//!   thread's reads / input / resize.
//!
//! ## The repaint loop (producer-authoritative, off-thread — R999 / R1270)
//!
//! The poll thread blocks on `scene/waitFor {since}` (cheap — parked host-side until
//! a pane produces output), then refetches every pane's LIVE cells into a shared
//! cache and calls `on_change` (the shell's `RepaintSink::request_repaint`). The
//! pure `view` reads that cache — never a blocking socket call — exactly the
//! off-thread-producer -> repaint shape `examples/hello-live-data` proves. A
//! scrolled-history read (`offset > 0`) is the one synchronous fetch, off `view`'s
//! hot path (an interactive gesture, not per-frame).
//!
//! The `since` baseline is read BEFORE the initial cell fetch (subscribe-then-
//! snapshot), so output landing during boot is caught by the first `waitFor`
//! (answered immediately when the scene already advanced) rather than lost against a
//! stale first frame.
//!
//! ## Threading
//!
//! `WireHost` lives on the single UI thread (`Owner::cache`, never `Send`/`Sync`
//! bound), so its request connection + tracked dims use [`RefCell`] (single-threaded
//! interior mutability). Only the `cache` is genuinely shared with the poll thread
//! and so is an `Arc<Mutex<_>>`.

use std::cell::RefCell;
use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use pinion_core::GridBuffer;
use serde_json::{Value, json};
use sprag_host::wire::{
    CELLS_ACTION, FULL_TEXT_SLOT, KEY_ACTION, PANES_SLOT, RESIZE_ACTION, SPAWN_ACTION, TEXT_ACTION,
};
use sprag_host::{CellFrame, HostClient, PaneScrollFacts, mux_action_path, pane_input_path};
use sprag_input::Modifiers;
use sprag_rpc::{HostConn, runtime_path};

/// How long to wait for the host socket to accept — covers the child's bind race.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Env override: attach to an already-running host at this socket path instead of
/// spawning a child (the tmux-attach mode).
const HOST_SOCK_ENV: &str = "SPRAG_GUI_HOST_SOCK";
/// Env override: the `sprag-term` binary to spawn (else the sibling of the GUI exe,
/// else `sprag-term` on `PATH`).
const HOST_BIN_ENV: &str = "SPRAG_GUI_HOST_BIN";

/// One pane's immutable identity on the wire: the host [`PaneId`](sprag_terminal::PaneId)
/// (the `/pane_<id>/…` address) and its command label (the a11y node name). Both are
/// read once at boot; panes never close this increment.
struct PaneMeta {
    id: u64,
    label: String,
}

/// Kills + reaps a spawned host child if [`spawn_or_attach`](WireHost::spawn_or_attach)
/// fails after the spawn — `std::process::Child`'s own `Drop` neither kills nor waits,
/// so an error `?`-returned after `spawn_host` would otherwise leak the child until GUI
/// exit. Disarmed with [`disarm`](Self::disarm) once boot succeeds (the child moves into
/// the live `WireHost`). Holds `None` in attach mode (nothing to reap).
struct ChildGuard(Option<Child>);

impl ChildGuard {
    /// Take the child out, disarming the guard — called on boot success so the reap
    /// does NOT run (the child is now owned by the live `WireHost`).
    fn disarm(mut self) -> Option<Child> {
        self.0.take()
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// The GUI's wire client of a `sprag-term` host. See the module docs.
pub(crate) struct WireHost {
    /// The spawned host child (`None` when attached to an operator-run host).
    child: Option<Child>,
    /// Tile index -> pane identity (immutable after boot).
    panes: Vec<PaneMeta>,
    /// Per-pane current grid `(cols, rows)`. The GUI is the sole resizer of its
    /// panes, so this tracks the host's true size: seeded from the host's pane list at
    /// boot and advanced only when a `resize` RPC SUCCEEDS, so the reflow no-op guard
    /// reads it immediately with no round-trip and no resize loop, and a failed resize
    /// is retried rather than latched.
    dims: RefCell<Vec<(u16, u16)>>,
    /// The UI thread's request connection (reads / input / resize). `RefCell`, not
    /// `Mutex`: `WireHost` is UI-thread-only (see the module docs), and the poll thread
    /// owns a SEPARATE connection.
    conn: RefCell<HostConn>,
    /// The live per-pane frames (offset 0), swapped in by the poll thread on each host
    /// change and read by `view` under a brief lock. The one genuinely-shared field.
    cache: Arc<Mutex<Vec<CellFrame>>>,
    /// Set on Drop to stop the poll loop.
    stop: Arc<AtomicBool>,
    /// A shutdown handle onto the poll connection: Drop calls `shutdown(Both)` on it to
    /// cancel the poll thread's parked `scene/waitFor` read so the join is deterministic
    /// in BOTH spawn and attach modes (in attach mode there is no child kill to close the
    /// socket for us).
    poll_shutdown: UnixStream,
    /// The background change-notification -> repaint thread, joined on Drop.
    poll: Option<JoinHandle<()>>,
}

impl WireHost {
    /// Spawn (or attach to) a `sprag-term` host and boot `n_panes` panes running
    /// `argv` (`None` = the host's default `$SHELL`), each at `cols x rows`, then
    /// start the background change-notification -> `on_change` repaint poll.
    ///
    /// # Errors
    ///
    /// Any failure to spawn the child, connect to its socket within
    /// [`CONNECT_TIMEOUT`], or boot the panes over RPC. A spawned child is reaped on
    /// any such failure ([`ChildGuard`]).
    pub(crate) fn spawn_or_attach(
        argv: Option<Vec<String>>,
        cols: u16,
        rows: u16,
        n_panes: usize,
        on_change: Box<dyn Fn() + Send>,
    ) -> io::Result<Self> {
        let (child, sock) = match std::env::var_os(HOST_SOCK_ENV) {
            Some(path) => {
                tracing::info!(target: "sprag_gui::wire", path = ?path, "attaching to a running host");
                (None, PathBuf::from(path))
            }
            None => {
                let sock = runtime_path(&format!("sprag-gui-host-{}.sock", std::process::id()));
                // Clear a stale socket file from a previous same-pid run (self-heal).
                let _ = std::fs::remove_file(&sock);
                let child = spawn_host(argv.as_deref(), cols, rows, &sock)?;
                tracing::info!(target: "sprag_gui::wire", path = %sock.display(), pid = child.id(), "spawned host child");
                (Some(child), sock)
            }
        };
        // Reap the spawned child on any boot error below (PR_SET_PDEATHSIG only covers
        // GUI-process death, not an error return here). Disarmed on success.
        let guard = ChildGuard(child);

        let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let booted = boot_panes(&mut conn, argv.as_deref(), cols, rows, n_panes)?;
        // Seed dims from the host's OWN pane list (its truth), not the GUI's window-
        // derived guess — correct in attach mode where the host's panes may differ.
        let dims: Vec<(u16, u16)> = booted.iter().map(|(_, d)| *d).collect();
        let panes: Vec<PaneMeta> = booted.into_iter().map(|(m, _)| m).collect();

        // Subscribe-then-snapshot: read the change-notification baseline BEFORE the
        // initial cell fetch, so output landing during boot makes the first `waitFor`
        // fire immediately (catch-up) instead of being lost against a stale first frame.
        let since0 = read_revision(&mut conn)?;
        let cache = Arc::new(Mutex::new(fetch_all(&mut conn, &panes)?));

        // The poll thread's own connection — a parked `scene/waitFor` on it never
        // blocks the request connection above (separate host handler threads).
        let poll_conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let poll_shutdown = poll_conn.shutdown_handle()?;
        let stop = Arc::new(AtomicBool::new(false));
        let poll = spawn_poll(
            poll_conn,
            panes.iter().map(|p| p.id).collect(),
            Arc::clone(&cache),
            on_change,
            Arc::clone(&stop),
            since0,
        )?;

        Ok(Self {
            child: guard.disarm(),
            panes,
            dims: RefCell::new(dims),
            conn: RefCell::new(conn),
            cache,
            stop,
            poll_shutdown,
            poll: Some(poll),
        })
    }

    /// Issue one request over the UI-thread connection, tracing (not swallowing
    /// silently) any wire failure — the ONE place a `WireHost` read/write RPC error is
    /// handled, so every method's error policy is consistent (the "swallow is honest,
    /// not silent" bar the in-process `Host` holds). Returns `None` on failure.
    fn request(&self, method: &str, params: Value, ctx: &str) -> Option<Value> {
        match self.conn.borrow_mut().call(method, params) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::debug!(target: "sprag_gui::wire", ctx, %error, "wire request failed");
                None
            }
        }
    }

    /// The host id of the pane at tile `index`, or `None` if out of range. The write /
    /// input methods guard on this (returning the trait's no-op / `false`) exactly like
    /// the read methods guard on the cache, so the whole impl handles an absent index
    /// uniformly rather than half-panicking.
    fn pane_id(&self, index: usize) -> Option<u64> {
        self.panes.get(index).map(|p| p.id)
    }

    /// The cached live (offset 0) cell buffer for tile `index`, or a `1x1` placeholder
    /// before the first frame / out of range.
    fn live_cells(&self, index: usize) -> GridBuffer {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
            .map(|frame| frame.cells.clone())
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }
}

impl HostClient for WireHost {
    fn pane_count(&self) -> usize {
        self.panes.len()
    }

    fn pane_cells(&self, index: usize, offset_lines: usize) -> GridBuffer {
        if offset_lines == 0 {
            // Live view: the poll-thread-maintained cache — no socket call in `view`.
            return self.live_cells(index);
        }
        // Scrolled history: one synchronous fetch (an interactive gesture, off the
        // per-frame hot path). On any failure, fall back to the cached live buffer.
        let Some(id) = self.pane_id(index) else {
            return self.live_cells(index);
        };
        let params = invoke(
            &pane_input_path(id, CELLS_ACTION),
            json!({ "offset": offset_lines }),
        );
        self.request("scene/invoke", params, "pane_cells")
            .and_then(|value| serde_json::from_value::<CellFrame>(value).ok())
            .map_or_else(|| self.live_cells(index), |frame| frame.cells)
    }

    fn pane_scroll_facts(&self, index: usize) -> PaneScrollFacts {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
            .map(|frame| frame.facts.clone())
            .unwrap_or(PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            })
    }

    fn pane_grid_size(&self, index: usize) -> (u16, u16) {
        self.dims.borrow().get(index).copied().unwrap_or((1, 1))
    }

    fn resize(&self, index: usize, cols: u16, rows: u16) {
        let Some(id) = self.pane_id(index) else {
            return;
        };
        let params = invoke(
            &mux_action_path(RESIZE_ACTION),
            json!({ "id": id, "cols": cols, "rows": rows }),
        );
        // Advance the tracked size only on a SUCCESSFUL resize, so a failed resize is
        // retried on the next reflow rather than latched into the no-op guard.
        if self.request("scene/invoke", params, "resize").is_some()
            && let Some(slot) = self.dims.borrow_mut().get_mut(index)
        {
            *slot = (cols, rows);
        }
    }

    fn send_key(&self, index: usize, key: &str, mods: Modifiers) -> bool {
        let Some(id) = self.pane_id(index) else {
            return false;
        };
        let args = json!({
            "key": key,
            "ctrl": mods.ctrl,
            "alt": mods.alt,
            "shift": mods.shift,
            "super": mods.sup,
        });
        self.request(
            "scene/invoke",
            invoke(&pane_input_path(id, KEY_ACTION), args),
            "send_key",
        )
        .is_some()
    }

    fn send_text(&self, index: usize, text: &str) -> bool {
        let Some(id) = self.pane_id(index) else {
            return false;
        };
        let params = invoke(&pane_input_path(id, TEXT_ACTION), json!({ "text": text }));
        self.request("scene/invoke", params, "send_text").is_some()
    }

    fn pane_full_text(&self, index: usize) -> String {
        let Some(id) = self.pane_id(index) else {
            return String::new();
        };
        let params = json!({ "path": pane_input_path(id, FULL_TEXT_SLOT) });
        self.request("scene/query", params, "pane_full_text")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn pane_command_label(&self, index: usize) -> String {
        self.panes
            .get(index)
            .map(|pane| pane.label.clone())
            .unwrap_or_default()
    }
}

impl Drop for WireHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Cancel the poll thread's parked `scene/waitFor` read so the join below is
        // deterministic in BOTH modes (attach mode has no child kill to close the
        // socket). All stream clones name one OS socket, so this reaches the reader.
        let _ = self.poll_shutdown.shutdown(Shutdown::Both);
        // Reap a spawned child (SIGKILL closes its PTY masters -> the pane shells get
        // SIGHUP, and its sockets close). PR_SET_PDEATHSIG covers an ungraceful GUI
        // exit; this is the graceful path.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(handle) = self.poll.take() {
            let _ = handle.join();
        }
    }
}

/// `scene/invoke` params: the addressed `path` + its `args`.
fn invoke(path: &str, args: Value) -> Value {
    json!({ "path": path, "args": args })
}

/// Read the host's current scene revision (the async `scene/waitFor` baseline).
fn read_revision(conn: &mut HostConn) -> io::Result<u64> {
    let value = conn.call("scene/revision", json!({}))?;
    Ok(value["revision"].as_u64().unwrap_or(0))
}

/// The `sprag-term` binary: `SPRAG_GUI_HOST_BIN`, else the sibling of the running
/// GUI executable (the cargo/target co-location), else `sprag-term` on `PATH`.
fn host_bin() -> PathBuf {
    if let Some(path) = std::env::var_os(HOST_BIN_ENV) {
        return PathBuf::from(path);
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(sibling) = exe.parent().map(|dir| dir.join("sprag-term"))
        && sibling.exists()
    {
        return sibling;
    }
    PathBuf::from("sprag-term")
}

/// Spawn a `sprag-term` child bound to `sock`, its initial pane running `argv`
/// (`None` = the host's default `$SHELL`) at `cols x rows`. `PR_SET_PDEATHSIG`
/// reaps it if the GUI dies ungracefully. stdout/stderr are inherited (the host's
/// tracing shows beside the GUI's); stdin is null (the socket is the transport, so
/// the host's stdin reader ends at once and only the socket keeps it alive).
fn spawn_host(
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
    sock: &std::path::Path,
) -> io::Result<Child> {
    let mut command = Command::new(host_bin());
    command.arg("--size").arg(format!("{cols}x{rows}"));
    if let Some(argv) = argv {
        command.arg("--");
        command.args(argv);
    }
    command
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null());
    // SAFETY: `pre_exec` runs in the forked child before exec; `prctl` is
    // async-signal-safe. PR_SET_PDEATHSIG(SIGKILL) makes the kernel kill the child
    // when the SPAWNING THREAD dies. The GUI spawns this on its long-lived
    // winit/main thread (use_terminal boot), so "spawning-thread death" == GUI
    // process exit — the crash-safety net over kill-on-Drop.
    unsafe {
        command.pre_exec(|| {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    command.spawn()
}

/// Query the host's pane list (`/sprag_mux/external/panes`), returning `(meta, dims)`
/// per pane in host order.
fn query_panes(conn: &mut HostConn) -> io::Result<Vec<(PaneMeta, (u16, u16))>> {
    let value = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(PANES_SLOT) }),
    )?;
    let array = value
        .as_array()
        .ok_or_else(|| io::Error::other("panes query did not return an array"))?;
    array
        .iter()
        .map(|pane| {
            let id = pane["id"]
                .as_u64()
                .ok_or_else(|| io::Error::other("pane entry missing a numeric id"))?;
            let label = pane["command"].as_str().unwrap_or_default().to_owned();
            let cols = u16::try_from(pane["cols"].as_u64().unwrap_or(1)).unwrap_or(1);
            let rows = u16::try_from(pane["rows"].as_u64().unwrap_or(1)).unwrap_or(1);
            Ok((PaneMeta { id, label }, (cols, rows)))
        })
        .collect()
}

/// Ensure the host has at least `n_panes` panes (spawn extras running `argv` at
/// `cols x rows` to reach it), then return the first `n_panes` with their identities
/// AND their host-reported dims, in tile order. The GUI's fixed-slot model wants a
/// stable count; this normalizes both the spawn (1 boot pane -> N) and attach (adopt
/// the first N) paths to `n_panes`.
///
/// NOTE (attach mode): forcing the count onto a pre-existing host is a bridge until
/// dynamic panes let the GUI mirror the host's live set; see the terminal module.
fn boot_panes(
    conn: &mut HostConn,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
    n_panes: usize,
) -> io::Result<Vec<(PaneMeta, (u16, u16))>> {
    let mut have = query_panes(conn)?.len();
    while have < n_panes {
        let mut args = json!({ "cols": cols, "rows": rows });
        if let Some(argv) = argv {
            args["cmd"] = json!(argv);
        }
        conn.call("scene/invoke", invoke(&mux_action_path(SPAWN_ACTION), args))?;
        have += 1;
    }
    let mut panes = query_panes(conn)?;
    panes.truncate(n_panes);
    Ok(panes)
}

/// Fetch every pane's live (offset 0) frame — the initial cache fill before the
/// first paint.
fn fetch_all(conn: &mut HostConn, panes: &[PaneMeta]) -> io::Result<Vec<CellFrame>> {
    panes
        .iter()
        .map(|pane| fetch_frame(conn, pane.id, 0))
        .collect()
}

/// Fetch one pane's cell frame at `offset` over the `cells` action — the shared
/// [`CellFrame`] the host serializes, deserialized on this end (one wire type).
fn fetch_frame(conn: &mut HostConn, id: u64, offset: usize) -> io::Result<CellFrame> {
    let value = conn.call(
        "scene/invoke",
        invoke(
            &pane_input_path(id, CELLS_ACTION),
            json!({ "offset": offset }),
        ),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Start the background poll: block on `scene/waitFor {since}`, refetch every pane's
/// live cells into `cache` on each wake, and call `on_change` (repaint). Exits when
/// `stop` is set (Drop cancels the parked read via a shutdown handle) or the host
/// closes. A poll exit that is NOT a requested stop is logged at `warn` — the host
/// connection was lost, so live updates have stopped and the GUI would otherwise
/// silently freeze.
///
/// # Errors
///
/// Fails if the poll thread cannot be spawned (matching `spawn_or_attach`'s contract
/// rather than panicking inside it).
fn spawn_poll(
    mut conn: HostConn,
    ids: Vec<u64>,
    cache: Arc<Mutex<Vec<CellFrame>>>,
    on_change: Box<dyn Fn() + Send>,
    stop: Arc<AtomicBool>,
    mut since: u64,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("sprag-gui-wire-poll".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let response = match conn.call("scene/waitFor", json!({ "since": since })) {
                    Ok(value) => value,
                    Err(error) => {
                        if !stop.load(Ordering::Relaxed) {
                            tracing::warn!(target: "sprag_gui::wire", %error, "host connection lost; live updates stopped");
                        }
                        break;
                    }
                };
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                since = response["revision"].as_u64().unwrap_or(since);
                let mut frames = Vec::with_capacity(ids.len());
                let mut ok = true;
                for &id in &ids {
                    match fetch_frame(&mut conn, id, 0) {
                        Ok(frame) => frames.push(frame),
                        Err(error) => {
                            tracing::warn!(target: "sprag_gui::wire", %error, "pane cell refetch failed; live updates stopped");
                            ok = false;
                            break;
                        }
                    }
                }
                if !ok {
                    break;
                }
                *cache.lock().unwrap_or_else(PoisonError::into_inner) = frames;
                on_change();
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_frame_deserializes_the_shared_host_cell_frame() {
        // The client and host share ONE `CellFrame` type (sprag-host), so a frame the
        // host serializes deserializes back byte-for-byte here — the envelope (the
        // `cells` key + GridBuffer) AND the flattened facts. Build a real CellFrame,
        // serialize it as the host does, and read it back exactly as `fetch_frame` does.
        let frame = CellFrame {
            cells: GridBuffer::new(3, 2),
            facts: PaneScrollFacts {
                scrollback_len: 7,
                visible_rows: 2,
            },
        };
        let json = serde_json::to_value(&frame).expect("host serializes CellFrame");
        // The wire keys are flat: cells + the flattened facts.
        assert!(json.get("cells").is_some());
        assert_eq!(json["scrollback_len"], 7);
        assert_eq!(json["visible_rows"], 2);
        let back: CellFrame = serde_json::from_value(json).expect("client deserializes CellFrame");
        assert_eq!((back.cells.cols(), back.cells.rows()), (3, 2));
        assert_eq!(back.facts.scrollback_len, 7);
        assert_eq!(back.facts.visible_rows, 2);
    }

    #[test]
    fn invoke_params_carry_the_path_and_args() {
        let params = invoke(&pane_input_path(0, KEY_ACTION), json!({ "key": "a" }));
        assert_eq!(params["path"], "/pane_0/sprag_input/external/key");
        assert_eq!(params["args"]["key"], "a");
    }
}
