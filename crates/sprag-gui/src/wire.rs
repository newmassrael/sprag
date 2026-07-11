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
//! a pane produces output), then refreshes every occupied slot's LIVE frame in the
//! shared slot [`Mirror`] and calls `on_change` (the shell's
//! `RepaintSink::request_repaint`). The pure `view` reads that mirror — never a
//! blocking socket call — exactly the off-thread-producer -> repaint shape
//! `examples/hello-live-data` proves. A scrolled-history read (`offset > 0`) is the
//! one synchronous fetch, off `view`'s hot path (an interactive gesture, not
//! per-frame).
//!
//! The `since` baseline is read BEFORE the initial cell fetch (subscribe-then-
//! snapshot), so output landing during boot is caught by the first `waitFor`
//! (answered immediately when the scene already advanced) rather than lost against a
//! stale first frame.
//!
//! ## Threading
//!
//! `WireHost` lives on the single UI thread (`Owner::cache`, never `Send`/`Sync`
//! bound), so its request connection uses [`RefCell`] (single-threaded interior
//! mutability). Only the slot [`Mirror`] is genuinely shared with the poll thread —
//! it holds the pane identities, live frames, AND tracked dims under one
//! `Arc<Mutex<_>>`, so a poll-side frame refresh (and a Round 2b membership reconcile)
//! is atomic vs a UI-thread read / resize.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
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

use crate::terminal::MAX_PANES;

/// How long to wait for the host socket to accept — covers the child's bind race.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Env override: attach to an already-running host at this socket path instead of
/// spawning a child (the tmux-attach mode).
const HOST_SOCK_ENV: &str = "SPRAG_GUI_HOST_SOCK";
/// Env override: the `sprag-term` binary to spawn (else the sibling of the GUI exe,
/// else `sprag-term` on `PATH`).
const HOST_BIN_ENV: &str = "SPRAG_GUI_HOST_BIN";

/// One pane's identity on the wire: the host [`PaneId`](sprag_terminal::PaneId) (the
/// `/pane_<id>/…` address) and its command label (the a11y node name). Stable for the
/// pane's life; it is what a slot maps to, so a pane KEEPS its slot as long as this id
/// stays in the host's list ([`reconcile`]).
struct PaneMeta {
    id: u64,
    label: String,
}

/// One occupied mirror slot: the pane's [`PaneMeta`] identity, its live (offset 0)
/// cell [`CellFrame`], and the GUI-authored grid size. A `None` slot in the mirror is
/// a HOLE (a closed pane's freed slot, or a never-used slot beyond the current set).
struct PaneEntry {
    meta: PaneMeta,
    /// The live (offset 0) frame, refreshed by the poll thread on each host change.
    frame: CellFrame,
    /// The GUI's tracked grid `(cols, rows)`. The GUI is the sole resizer of its panes,
    /// so this tracks the host's true size: seeded from the host's pane list at boot /
    /// on appearance, advanced only when a `resize` RPC SUCCEEDS (so the reflow no-op
    /// guard reads it with no round-trip and a failed resize is retried, not latched).
    dims: (u16, u16),
}

/// The slot-indexed mirror of the host's pane set: a `Vec<Option<PaneEntry>>` of length
/// [`MAX_PANES`], `Some` = an occupied display slot, `None` = a hole. This is the ONE
/// abstraction the whole GUI addresses panes through (topology B): the `HostClient`
/// methods take a SLOT, mapped here to the pane's live `PaneId`. Shared between the UI
/// thread (reads, input, resize) and the poll thread (frame refresh + [`reconcile`]) —
/// one lock so a set change is atomic vs a UI read. Slots are stable per pane (a pane
/// keeps its slot for life), designed so Round 2b can add / remove entries live without
/// migrating any per-slot GUI state (scroll / preedit / focus, all keyed by slot).
type Mirror = Arc<Mutex<Vec<Option<PaneEntry>>>>;

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
    /// The slot-indexed mirror of the host's pane set (the ONE pane-state field:
    /// identity + live frame + tracked dims per slot, [`Mirror`]). The UI thread reads
    /// it under a brief lock; the poll thread refreshes frames (and, in Round 2b,
    /// reconciles membership) under the same lock. The one genuinely-shared pane state,
    /// replacing the former parallel `panes` / `dims` / `cache` fields.
    mirror: Mirror,
    /// The UI thread's request connection (reads / input / resize). `RefCell`, not
    /// `Mutex`: `WireHost` is UI-thread-only (see the module docs), and the poll thread
    /// owns a SEPARATE connection.
    conn: RefCell<HostConn>,
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
        //
        // Attach mode (no child) ADOPTS the host's live pane set (tmux-attach); spawn
        // mode reaches exactly `n_panes` (the GUI is the operator). `boot_panes` branches
        // on this — read BEFORE the child moves into the guard.
        let attach = child.is_none();
        let guard = ChildGuard(child);

        let mut conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let host_panes = boot_panes(&mut conn, argv.as_deref(), cols, rows, n_panes, attach)?;

        // Subscribe-then-snapshot: read the change-notification baseline BEFORE the
        // initial frame fetches (in `reconcile`), so output landing during boot makes the
        // first `waitFor` fire immediately (catch-up) instead of being lost against a
        // stale first frame.
        let since0 = read_revision(&mut conn)?;
        // The slot-indexed mirror, filled by the ONE `reconcile` SSOT (boot = its all-new
        // path: contiguous slots `0..N` in host display order).
        let mut slots: Vec<Option<PaneEntry>> = (0..MAX_PANES).map(|_| None).collect();
        reconcile(&mut slots, &mut conn, host_panes)?;
        let mirror: Mirror = Arc::new(Mutex::new(slots));

        // The poll thread's own connection — a parked `scene/waitFor` on it never
        // blocks the request connection above (separate host handler threads).
        let poll_conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let poll_shutdown = poll_conn.shutdown_handle()?;
        let stop = Arc::new(AtomicBool::new(false));
        let poll = spawn_poll(
            poll_conn,
            Arc::clone(&mirror),
            on_change,
            Arc::clone(&stop),
            since0,
        )?;

        Ok(Self {
            child: guard.disarm(),
            mirror,
            conn: RefCell::new(conn),
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

    /// Lock the shared mirror (poison-tolerant, matching the rest of the wire client's
    /// lock discipline). The ONE place the mirror lock is taken on the UI thread.
    fn lock_mirror(&self) -> MutexGuard<'_, Vec<Option<PaneEntry>>> {
        self.mirror.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host id of the pane at SLOT `index`, or `None` for a hole / out of range. The
    /// write / input methods guard on this (returning the trait's no-op / `false`)
    /// exactly like the read methods guard on the mirror, so the whole impl handles an
    /// empty slot uniformly rather than half-panicking.
    fn pane_id(&self, index: usize) -> Option<u64> {
        self.lock_mirror()
            .get(index)
            .and_then(|slot| slot.as_ref())
            .map(|entry| entry.meta.id)
    }

    /// The cached live (offset 0) cell buffer for SLOT `index`, or a `1x1` placeholder
    /// for a hole / out of range / before the first frame.
    fn live_cells(&self, index: usize) -> GridBuffer {
        self.lock_mirror()
            .get(index)
            .and_then(|slot| slot.as_ref())
            .map(|entry| entry.frame.cells.clone())
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }
}

impl HostClient for WireHost {
    fn pane_count(&self) -> usize {
        self.lock_mirror()
            .iter()
            .filter(|slot| slot.is_some())
            .count()
    }

    fn occupied_slots(&self) -> Vec<usize> {
        self.lock_mirror()
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| entry.as_ref().map(|_| slot))
            .collect()
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
        self.lock_mirror()
            .get(index)
            .and_then(|slot| slot.as_ref())
            .map(|entry| entry.frame.facts.clone())
            .unwrap_or(PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            })
    }

    fn pane_grid_size(&self, index: usize) -> (u16, u16) {
        self.lock_mirror()
            .get(index)
            .and_then(|slot| slot.as_ref())
            .map(|entry| entry.dims)
            .unwrap_or((1, 1))
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
            && let Some(Some(entry)) = self.lock_mirror().get_mut(index)
        {
            entry.dims = (cols, rows);
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
        self.lock_mirror()
            .get(index)
            .and_then(|slot| slot.as_ref())
            .map(|entry| entry.meta.label.clone())
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

/// The pane set the mirror boots with, in host display order, each with its identity +
/// host-reported dims.
///
/// * **Attach mode** (`attach`, the GUI reached an operator-run host) ADOPTS the host's
///   live panes — the tmux-attach semantics — clamped to the GUI's slot cap
///   [`MAX_PANES`]. No spawn / truncate to a GUI-chosen count: the GUI mirrors the host.
/// * **Spawn mode** (the GUI owns the host child) ensures exactly `n_panes` (already
///   clamped to `[1, MAX_PANES]` by [`pane_count`](crate::terminal::pane_count)),
///   spawning extras running `argv` at `cols x rows` to reach it, then takes the first
///   `n_panes` — the GUI is the operator and asks for its configured layout.
///
/// Forcing a count is now ONLY the spawn path (Round 1 of the tmux-mirror). Live add /
/// remove of the adopted set is Round 2b.
fn boot_panes(
    conn: &mut HostConn,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
    n_panes: usize,
    attach: bool,
) -> io::Result<Vec<(PaneMeta, (u16, u16))>> {
    if attach {
        let mut panes = query_panes(conn)?;
        panes.truncate(MAX_PANES);
        return Ok(panes);
    }
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

/// Reconcile the slot mirror to the host's current pane list — the ONE place slot
/// MEMBERSHIP changes. It frees the slot of every mapped pane no longer present, then
/// allocates the lowest free slot to each host pane not yet mapped (fetching its initial
/// frame). A pane thus KEEPS its slot for its whole life, so no per-slot GUI state
/// (scroll / preedit / focus) migrates onto a different pane.
///
/// Boot calls this once against an all-`None` mirror — the all-new path, which lays the
/// adopted panes in contiguous slots `0..N` in host display order. Round 2b calls it per
/// poll wake to apply live add / remove deltas (the reason it is written against the
/// delta case now, not a boot-only shortcut). Frame REFRESH of an already-mapped pane is
/// the poll's separate per-wake fetch; this owns only membership + a newly-appeared
/// pane's FIRST frame. A host set larger than [`MAX_PANES`] mirrors the first `MAX_PANES`
/// (the honest slot-cap bound) and logs the overflow.
fn reconcile(
    slots: &mut [Option<PaneEntry>],
    conn: &mut HostConn,
    host_panes: Vec<(PaneMeta, (u16, u16))>,
) -> io::Result<()> {
    let current: Vec<Option<u64>> = slots
        .iter()
        .map(|slot| slot.as_ref().map(|entry| entry.meta.id))
        .collect();
    let host_ids: Vec<u64> = host_panes.iter().map(|(meta, _)| meta.id).collect();
    let (frees, adds) = plan_slots(&current, &host_ids);

    for slot in frees {
        slots[slot] = None;
    }
    // Move each newly-placed pane's identity + dims into its slot, fetching its first
    // frame. Index the host panes by id so an add takes OWNERSHIP of its entry.
    let mut by_id: HashMap<u64, (PaneMeta, (u16, u16))> = host_panes
        .into_iter()
        .map(|(meta, dims)| (meta.id, (meta, dims)))
        .collect();
    for (slot, id) in adds {
        if let Some((meta, dims)) = by_id.remove(&id) {
            let frame = fetch_frame(conn, id, 0)?;
            slots[slot] = Some(PaneEntry { meta, frame, dims });
        }
    }
    // A host id in no slot after applying the plan overflowed the slot cap (survivors +
    // placed adds fill every slot they can) — the honest MAX_PANES bound, logged once.
    for id in host_ids {
        if !slots.iter().flatten().any(|entry| entry.meta.id == id) {
            tracing::warn!(
                target: "sprag_gui::wire",
                pane = id,
                "host pane set exceeds MAX_PANES; pane not mirrored",
            );
        }
    }
    Ok(())
}

/// The PURE slot-allocation plan behind [`reconcile`] (so the allocator is unit-tested
/// without a host): from each slot's current occupant id (`None` = a hole) and the
/// host's live id list (display order), compute the slots to FREE (occupant vanished)
/// and the `(slot, id)` ADDS (a host id with no slot yet, placed at the LOWEST free slot
/// — reusing a slot freed in this same plan, so slot usage stays compact). A survivor (an
/// id still present) keeps its existing slot and appears in neither list. A host id past
/// the [`MAX_PANES`] slot cap gets no slot — it is absent from `adds` (the caller logs
/// the overflow). This is the load-bearing Round 2b logic; boot exercises only its
/// all-new path (contiguous `0..N`).
fn plan_slots(current: &[Option<u64>], host_ids: &[u64]) -> (Vec<usize>, Vec<(usize, u64)>) {
    let live: HashSet<u64> = host_ids.iter().copied().collect();
    let mut taken: Vec<bool> = current.iter().map(Option::is_some).collect();
    let mut frees = Vec::new();
    for (slot, occupant) in current.iter().enumerate() {
        if let Some(id) = occupant
            && !live.contains(id)
        {
            frees.push(slot);
            taken[slot] = false; // available for an add below (hole reuse)
        }
    }
    let survivors: HashSet<u64> = current
        .iter()
        .flatten()
        .copied()
        .filter(|id| live.contains(id))
        .collect();
    let mut adds = Vec::new();
    for &id in host_ids {
        if survivors.contains(&id) {
            continue; // keeps its existing slot
        }
        if let Some(free) = taken.iter().position(|slot_taken| !slot_taken) {
            taken[free] = true;
            adds.push((free, id));
        }
        // else: no free slot (host set > MAX_PANES) — dropped; reconcile logs it.
    }
    (frees, adds)
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
    mirror: Mirror,
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
                // Snapshot the occupied (slot, id) targets under a brief lock, fetch each
                // frame OFF the lock (never a socket call while holding it), then write the
                // frames back to their slots. Round 2b inserts a `reconcile` (re-query the
                // pane list) HERE, before deriving targets; the set is fixed in Round 2a.
                let targets: Vec<(usize, u64)> = {
                    let guard = mirror.lock().unwrap_or_else(PoisonError::into_inner);
                    guard
                        .iter()
                        .enumerate()
                        .filter_map(|(slot, entry)| {
                            entry.as_ref().map(|entry| (slot, entry.meta.id))
                        })
                        .collect()
                };
                let mut fetched = Vec::with_capacity(targets.len());
                let mut ok = true;
                for (slot, id) in targets {
                    match fetch_frame(&mut conn, id, 0) {
                        Ok(frame) => fetched.push((slot, frame)),
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
                // Write frames back only to slots STILL occupied by the same fetch target
                // (a `get_mut(slot)` that is `Some(Some(entry))`); a slot freed meanwhile
                // (Round 2b) is skipped rather than resurrected.
                let mut guard = mirror.lock().unwrap_or_else(PoisonError::into_inner);
                for (slot, frame) in fetched {
                    if let Some(Some(entry)) = guard.get_mut(slot) {
                        entry.frame = frame;
                    }
                }
                drop(guard);
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

    #[test]
    fn plan_slots_boot_is_contiguous_from_empty() {
        // Boot = the all-new path: an empty mirror + host ids -> contiguous slots 0..N in
        // host display order, no frees.
        let (frees, adds) = plan_slots(&[None, None, None, None], &[10, 11, 12]);
        assert!(frees.is_empty());
        assert_eq!(adds, vec![(0, 10), (1, 11), (2, 12)]);
    }

    #[test]
    fn plan_slots_survivors_keep_their_slots() {
        // Ids already mapped and still live keep their slots (neither freed nor re-added),
        // so no per-slot GUI state migrates.
        let (frees, adds) = plan_slots(&[Some(10), Some(11), None, None], &[10, 11]);
        assert!(frees.is_empty());
        assert!(adds.is_empty());
    }

    #[test]
    fn plan_slots_frees_a_closed_pane_and_reuses_the_hole() {
        // Pane at slot 1 closed, a new pane (20) appeared: slot 1 frees, the survivors (10,
        // 12) keep slots 0 and 2, and the newcomer takes the LOWEST free slot — the reused
        // hole at slot 1 — so slot usage stays compact.
        let (frees, adds) = plan_slots(&[Some(10), Some(11), Some(12), None], &[10, 12, 20]);
        assert_eq!(frees, vec![1]);
        assert_eq!(adds, vec![(1, 20)]);
    }

    #[test]
    fn plan_slots_drops_ids_past_the_slot_cap() {
        // A full mirror (no holes) with an extra host id: the newcomer gets NO slot (absent
        // from adds) — the honest MAX_PANES bound the caller logs.
        let full: Vec<Option<u64>> = (0..MAX_PANES as u64).map(Some).collect();
        let mut host: Vec<u64> = (0..MAX_PANES as u64).collect();
        host.push(999);
        let (frees, adds) = plan_slots(&full, &host);
        assert!(frees.is_empty());
        assert!(adds.is_empty(), "no free slot -> the extra id is dropped");
    }
}
