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

use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use pinion_core::GridBuffer;
use serde_json::{Value, json};
use sprag_host::{HostClient, PaneScrollFacts};
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

/// One pane's live cell FRAME (offset 0), maintained by the poll thread and read by
/// the pure `view` — the producer-authoritative buffer (the GUI's `hello-live-data`
/// analog).
struct Frame {
    cells: GridBuffer,
    facts: PaneScrollFacts,
}

/// The wire-frame shape the host's `cells` action returns
/// (`{cells, scrollback_len, visible_rows}`) — the client deserialize twin of the
/// host's serialize-only `CellFrame`, sharing [`PaneScrollFacts`] as the flattened
/// non-cell field set (its field names ARE the wire keys), so the two ends cannot
/// drift.
#[derive(serde::Deserialize)]
struct WireFrame {
    cells: GridBuffer,
    #[serde(flatten)]
    facts: PaneScrollFacts,
}

/// The GUI's wire client of a `sprag-term` host. See the module docs.
pub(crate) struct WireHost {
    /// The spawned host child (`None` when attached to an operator-run host).
    child: Option<Child>,
    /// Tile index -> pane identity (immutable after boot).
    panes: Vec<PaneMeta>,
    /// Per-pane current grid `(cols, rows)`. The GUI is the sole resizer of its
    /// panes, so this tracks its own `resize` calls (updated there) — the reflow
    /// no-op guard reads it immediately, with no round-trip and no resize loop.
    dims: Mutex<Vec<(u16, u16)>>,
    /// The UI thread's request connection (reads / input / resize). Behind a `Mutex`
    /// only for the `&self` trait signature; one caller (the UI thread) touches it.
    conn: Mutex<HostConn>,
    /// The live per-pane frames (offset 0), swapped in by the poll thread on each
    /// host change and read by `view` under a brief lock.
    cache: Arc<Mutex<Vec<Frame>>>,
    /// Set on Drop to stop the poll loop (a killed child also unblocks its read).
    stop: Arc<AtomicBool>,
    /// The background change-notification -> repaint thread (detached on Drop).
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
    /// [`CONNECT_TIMEOUT`], or boot the panes over RPC.
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

        let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let panes = boot_panes(&mut conn, argv.as_deref(), cols, rows, n_panes)?;
        let dims = vec![(cols, rows); panes.len()];
        let cache = Arc::new(Mutex::new(fetch_all(&mut conn, &panes)?));

        // The poll thread's own connection — a parked `scene/waitFor` on it never
        // blocks the request connection above (separate host handler threads).
        let poll_conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let stop = Arc::new(AtomicBool::new(false));
        let poll = spawn_poll(
            poll_conn,
            panes.iter().map(|p| p.id).collect(),
            Arc::clone(&cache),
            on_change,
            Arc::clone(&stop),
        );

        Ok(Self {
            child,
            panes,
            dims: Mutex::new(dims),
            conn: Mutex::new(conn),
            cache,
            stop,
            poll: Some(poll),
        })
    }

    /// Run `f` over the request connection (poison-tolerant — the UI thread is the
    /// sole caller, so a poisoned lock only means a prior panic, not contention).
    fn with_conn<R>(&self, f: impl FnOnce(&mut HostConn) -> R) -> R {
        let mut conn = self.conn.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut conn)
    }

    /// This pane's `/pane_<id>/sprag_input/external/<action>` invoke path.
    fn pane_path(&self, index: usize, action: &str) -> String {
        format!(
            "/pane_{}/sprag_input/external/{action}",
            self.panes[index].id
        )
    }
}

impl HostClient for WireHost {
    fn pane_count(&self) -> usize {
        self.panes.len()
    }

    fn pane_cells(&self, index: usize, offset_lines: usize) -> GridBuffer {
        if offset_lines == 0 {
            // Live view: the poll-thread-maintained cache — no socket call in `view`.
            return self
                .cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(index)
                .map(|f| f.cells.clone())
                .unwrap_or_else(|| GridBuffer::new(1, 1));
        }
        // Scrolled history: one synchronous fetch (an interactive gesture, off the
        // per-frame hot path). On error, fall back to the cached live buffer.
        let path = self.pane_path(index, "cells");
        self.with_conn(|conn| {
            conn.call(
                "scene/invoke",
                invoke(&path, json!({ "offset": offset_lines })),
            )
        })
        .ok()
        .and_then(|v| serde_json::from_value::<WireFrame>(v).ok())
        .map(|f| f.cells)
        .unwrap_or_else(|| {
            self.cache
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(index)
                .map(|f| f.cells.clone())
                .unwrap_or_else(|| GridBuffer::new(1, 1))
        })
    }

    fn pane_scroll_facts(&self, index: usize) -> PaneScrollFacts {
        self.cache
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
            .map(|f| f.facts.clone())
            .unwrap_or(PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            })
    }

    fn pane_grid_size(&self, index: usize) -> (u16, u16) {
        self.dims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(index)
            .copied()
            .unwrap_or((1, 1))
    }

    fn resize(&self, index: usize, cols: u16, rows: u16) {
        // Track the target locally FIRST (the GUI is the resize authority), so the
        // reflow no-op guard sees the new size immediately — no resize loop while the
        // host's reflowed cells propagate back through the poll.
        if let Some(slot) = self
            .dims
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_mut(index)
        {
            *slot = (cols, rows);
        }
        let id = self.panes[index].id;
        let params = invoke(
            "/sprag_mux/external/resize",
            json!({ "id": id, "cols": cols, "rows": rows }),
        );
        if let Err(error) = self.with_conn(|conn| conn.call("scene/invoke", params)) {
            tracing::debug!(target: "sprag_gui::wire", pane = index, %error, "resize over the wire failed");
        }
    }

    fn send_key(&self, index: usize, key: &str, mods: Modifiers) -> bool {
        let path = self.pane_path(index, "key");
        let args = json!({
            "key": key,
            "ctrl": mods.ctrl,
            "alt": mods.alt,
            "shift": mods.shift,
            "super": mods.sup,
        });
        self.with_conn(|conn| conn.call("scene/invoke", invoke(&path, args)))
            .is_ok()
    }

    fn send_text(&self, index: usize, text: &str) -> bool {
        let path = self.pane_path(index, "text");
        self.with_conn(|conn| conn.call("scene/invoke", invoke(&path, json!({ "text": text }))))
            .is_ok()
    }

    fn pane_full_text(&self, index: usize) -> String {
        let path = self.pane_path(index, "full_text");
        self.with_conn(|conn| conn.call("scene/query", json!({ "path": path })))
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn pane_command_label(&self, index: usize) -> String {
        self.panes[index].label.clone()
    }
}

impl Drop for WireHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Reap a spawned child gracefully-ish: SIGKILL closes its PTY masters (the
        // pane shells get SIGHUP) and its sockets, which unblocks the poll thread's
        // parked read so it exits on its own. PR_SET_PDEATHSIG already covers an
        // ungraceful GUI exit; this is the graceful path.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
            if let Some(handle) = self.poll.take() {
                let _ = handle.join();
            }
        }
        // Attach mode: the host stays up and the poll read stays parked, so don't
        // join (it would hang). The detached thread exits when the process does.
    }
}

/// `scene/invoke` params: the addressed `path` + its `args`.
fn invoke(path: &str, args: Value) -> Value {
    json!({ "path": path, "args": args })
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
/// reaps it if the GUI dies ungracefully.
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
        // The socket is the transport; a null stdin ends the host's stdin reader at
        // once (the always-on socket keeps the server alive).
        .stdin(Stdio::null());
    // SAFETY: `pre_exec` runs in the forked child before exec; `prctl` is
    // async-signal-safe. PR_SET_PDEATHSIG(SIGKILL) makes the kernel kill the child
    // when THIS (the parent GUI) thread dies — the crash-safety net over kill-on-Drop.
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
        json!({ "path": "/sprag_mux/external/panes" }),
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

/// Ensure the host has exactly `n_panes` panes (spawn extras running `argv` at
/// `cols x rows` to reach it; adopt only the first `n_panes` if more exist), then
/// return their identities in tile order. The GUI's fixed-slot model wants a stable
/// count, so this normalizes both the spawn (1 boot pane -> N) and attach (adopt N)
/// paths to `n_panes`.
fn boot_panes(
    conn: &mut HostConn,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
    n_panes: usize,
) -> io::Result<Vec<PaneMeta>> {
    let mut have = query_panes(conn)?.len();
    while have < n_panes {
        let mut args = json!({ "cols": cols, "rows": rows });
        if let Some(argv) = argv {
            args["cmd"] = json!(argv);
        }
        conn.call("scene/invoke", invoke("/sprag_mux/external/spawn", args))?;
        have += 1;
    }
    let mut panes: Vec<PaneMeta> = query_panes(conn)?.into_iter().map(|(m, _)| m).collect();
    panes.truncate(n_panes.max(1));
    Ok(panes)
}

/// Fetch every pane's live (offset 0) frame — the initial cache fill before the
/// first paint.
fn fetch_all(conn: &mut HostConn, panes: &[PaneMeta]) -> io::Result<Vec<Frame>> {
    panes.iter().map(|p| fetch_frame(conn, p.id, 0)).collect()
}

/// Fetch one pane's cell frame at `offset` over the `cells` action.
fn fetch_frame(conn: &mut HostConn, id: u64, offset: usize) -> io::Result<Frame> {
    let path = format!("/pane_{id}/sprag_input/external/cells");
    let value = conn.call("scene/invoke", invoke(&path, json!({ "offset": offset })))?;
    let frame: WireFrame = serde_json::from_value(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    Ok(Frame {
        cells: frame.cells,
        facts: frame.facts,
    })
}

/// Start the background poll: block on `scene/waitFor {since}`, refetch every pane's
/// live cells into `cache` on each wake, and call `on_change` (repaint). Exits when
/// `stop` is set (a killed child unblocks the parked read) or the host closes.
fn spawn_poll(
    mut conn: HostConn,
    ids: Vec<u64>,
    cache: Arc<Mutex<Vec<Frame>>>,
    on_change: Box<dyn Fn() + Send>,
    stop: Arc<AtomicBool>,
) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name("sprag-gui-wire-poll".to_owned())
        .spawn(move || {
            let mut since = match conn.call("scene/revision", json!({})) {
                Ok(value) => value["revision"].as_u64().unwrap_or(0),
                Err(error) => {
                    tracing::debug!(target: "sprag_gui::wire", %error, "revision bootstrap failed; poll ends");
                    return;
                }
            };
            while !stop.load(Ordering::Relaxed) {
                let response = match conn.call("scene/waitFor", json!({ "since": since })) {
                    Ok(value) => value,
                    Err(_) => break, // host gone (or killed on Drop)
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
                        Err(_) => {
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
        .expect("spawn the sprag-gui wire poll thread")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_frame_deserializes_the_host_cell_frame() {
        // The client's `WireFrame` must read the EXACT JSON the host's `cells` action
        // emits ({cells: <GridBuffer>, scrollback_len, visible_rows} — the flattened
        // PaneScrollFacts). Build that shape from a real GridBuffer and round it back:
        // a field rename on either end (they live in different crates) fails HERE.
        let buffer = GridBuffer::new(3, 2);
        let frame = json!({
            "cells": serde_json::to_value(&buffer).expect("GridBuffer serializes (PR-49)"),
            "scrollback_len": 7,
            "visible_rows": 2,
        });
        let wire: WireFrame = serde_json::from_value(frame).expect("host frame deserializes");
        assert_eq!((wire.cells.cols(), wire.cells.rows()), (3, 2));
        assert_eq!(wire.facts.scrollback_len, 7);
        assert_eq!(wire.facts.visible_rows, 2);
    }

    #[test]
    fn invoke_params_carry_the_path_and_args() {
        let params = invoke("/pane_0/sprag_input/external/key", json!({ "key": "a" }));
        assert_eq!(params["path"], "/pane_0/sprag_input/external/key");
        assert_eq!(params["args"]["key"], "a");
    }
}
