//! `WireHost` — the GUI as a pure wire client of a `sprag-term` host process
//! (topology B).
//!
//! [`WireHost`] implements the same [`HostClient`] protocol the in-process
//! [`Host`](sprag_host::Host) does — addressing panes by their host [`PaneId`] — but
//! reaches them over an RPC socket to a `sprag-term` host PROCESS instead of an
//! in-process `Workspace`. The GUI wraps it in a [`SlotView`](crate::slotview::SlotView),
//! the GUI-side slot↔`PaneId` adapter that maps the consumers' display slots onto host
//! ids, so both this wire client and the in-process `Host` stay pure identity clients and
//! the "slot" concept lives in ONE GUI place — the R109 seam across the process boundary.
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
//! a pane produces output OR the pane set changes — the R118 rail bumps the revision on
//! a spawn/close too), then RE-QUERIES the pane list and mirrors it into the shared
//! [`Cache`] — adding new panes, dropping closed ones, and refreshing every survivor's
//! LIVE frame (Round 2b live delta) — and calls `on_change` (the shell's
//! `RepaintSink::request_repaint`). The pure `view` reads that cache — never a
//! blocking socket call — exactly the off-thread-producer -> repaint shape
//! `examples/hello-live-data` proves; the GUI's `SlotView` maps the mirrored set onto
//! display slots on the UI thread (its own `reconcile` in the pinion `reconcile_frame`
//! pre-view hook). A scrolled-history read (`offset > 0`) is the one synchronous fetch,
//! off `view`'s hot path (an interactive gesture, not per-frame).
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
//! mutability). Only the pane [`Cache`] is genuinely shared with the poll thread — it
//! holds the pane identities, live frames, AND tracked dims under one `Arc<Mutex<_>>`,
//! so a poll-side frame refresh is atomic vs a UI-thread read / resize. Slot mapping
//! (and its membership) is the GUI's `SlotView`, on the UI thread only.

use std::cell::RefCell;
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
use sprag_terminal::PaneId;

/// How long to wait for the host socket to accept — covers the child's bind race.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Env override: attach to an already-running host at this socket path instead of
/// spawning a child (the tmux-attach mode).
const HOST_SOCK_ENV: &str = "SPRAG_GUI_HOST_SOCK";
/// Env override: the `sprag-term` binary to spawn (else the sibling of the GUI exe,
/// else `sprag-term` on `PATH`).
const HOST_BIN_ENV: &str = "SPRAG_GUI_HOST_BIN";

/// One pane the wire client mirrors, in HOST order (no holes — "slots" and their holes
/// are the GUI `SlotView`'s concern, not this data client's). Holds the pane's host
/// identity ([`PaneId`] + command label), its live (offset 0) [`CellFrame`] (refreshed
/// by the poll thread on each host change), and the GUI-tracked grid size.
struct WirePane {
    id: PaneId,
    label: String,
    /// The child's live `OSC 0`/`OSC 2` window title, `None` until it sets one.
    /// Host-authoritative like [`Self::label`] (re-read on every poll re-query, since a
    /// shell rewrites it each prompt). A DISPLAY name only — never identity.
    title: Option<String>,
    frame: CellFrame,
    /// The GUI's tracked grid `(cols, rows)`: seeded from the host at boot, advanced only
    /// when a `resize` RPC SUCCEEDS (so the reflow no-op guard reads it with no
    /// round-trip and a failed resize is retried, not latched).
    dims: (u16, u16),
}

/// The wire client's pane data cache, in host order. Shared between the UI thread
/// (reads / input / resize) and the poll thread (frame refresh) under one lock. Addressed
/// by [`PaneId`] identity (a linear scan over the small pane set), NOT by display slot —
/// this client speaks the host's language; the GUI's `SlotView` owns slot mapping.
type Cache = Arc<Mutex<Vec<WirePane>>>;

/// Lock the shared pane cache, poison-tolerant — the ONE definition of the cache's lock
/// discipline, shared by the UI thread ([`WireHost::lock_cache`]) and the poll thread.
fn lock_cache(cache: &Mutex<Vec<WirePane>>) -> MutexGuard<'_, Vec<WirePane>> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
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
    /// The pane data cache ([`Cache`]) in host order: identity + live frame + tracked
    /// dims per pane. The UI thread reads it under a brief lock; the poll thread refreshes
    /// each pane's frame under the same lock. Addressed by [`PaneId`] — the GUI's
    /// `SlotView` maps display slots onto these ids.
    cache: Cache,
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
        let seeds = boot_panes(&mut conn, argv.as_deref(), cols, rows, n_panes, attach)?;

        // Subscribe-then-snapshot: read the change-notification baseline BEFORE the
        // initial frame fetches, so output landing during boot makes the first `waitFor`
        // fire immediately (catch-up) instead of being lost against a stale first frame.
        let since0 = read_revision(&mut conn)?;
        let cache: Cache = Arc::new(Mutex::new(build_cache(&mut conn, seeds)));

        // The poll thread's own connection — a parked `scene/waitFor` on it never
        // blocks the request connection above (separate host handler threads).
        let poll_conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        let poll_shutdown = poll_conn.shutdown_handle()?;
        let stop = Arc::new(AtomicBool::new(false));
        let poll = spawn_poll(
            poll_conn,
            Arc::clone(&cache),
            on_change,
            Arc::clone(&stop),
            since0,
        )?;

        Ok(Self {
            child: guard.disarm(),
            cache,
            conn: RefCell::new(conn),
            stop,
            poll_shutdown,
            poll: Some(poll),
        })
    }

    /// Issue one request over the UI-thread connection, tracing (not swallowing
    /// silently) any wire failure — the ONE place a `WireHost` read/write RPC error is
    /// handled, so every method's error policy is consistent (the "swallow is honest,
    /// not silent" bar the in-process [`Host`](sprag_host::Host) holds). Returns `None`
    /// on failure.
    fn request(&self, method: &str, params: Value, ctx: &str) -> Option<Value> {
        match self.conn.borrow_mut().call(method, params) {
            Ok(value) => Some(value),
            Err(error) => {
                tracing::debug!(target: "sprag_gui::wire", ctx, %error, "wire request failed");
                None
            }
        }
    }

    /// Lock the shared pane cache (poison-tolerant, matching the rest of the wire
    /// client's lock discipline). The ONE place the cache lock is taken on the UI thread.
    fn lock_cache(&self) -> MutexGuard<'_, Vec<WirePane>> {
        lock_cache(&self.cache)
    }

    /// The cached live (offset 0) cell buffer for pane `id`, or a `1x1` placeholder for
    /// an absent id / before the first frame. Absent-id tolerance keeps every method's
    /// contract graceful, matching the in-process [`Host`](sprag_host::Host).
    fn live_cells(&self, id: PaneId) -> GridBuffer {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.frame.cells.clone())
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }
}

impl HostClient for WireHost {
    /// The mirrored cache's ids, in host order. Honors the trait's "renderable now"
    /// contract by construction: `merge_panes` only admits a pane once its first frame is
    /// fetched (a frameless newcomer is dropped and retried next wake), so a just-spawned
    /// host pane appears here at most one poll-wake after the host gained it — normally the
    /// very next wake (its first `cells` fetch usually succeeds), later only if that fetch
    /// keeps failing.
    fn pane_ids(&self) -> Vec<PaneId> {
        self.lock_cache().iter().map(|pane| pane.id).collect()
    }

    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer {
        if offset_lines == 0 {
            // Live view: the poll-thread-maintained cache — no socket call in `view`.
            return self.live_cells(id);
        }
        // Scrolled history: one synchronous fetch (an interactive gesture, off the
        // per-frame hot path). On any failure, fall back to the cached live buffer.
        let params = invoke(
            &pane_input_path(id.0, CELLS_ACTION),
            json!({ "offset": offset_lines }),
        );
        self.request("scene/invoke", params, "pane_cells")
            .and_then(|value| serde_json::from_value::<CellFrame>(value).ok())
            .map_or_else(|| self.live_cells(id), |frame| frame.cells)
    }

    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.frame.facts.clone())
            .unwrap_or(PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            })
    }

    fn pane_grid_size(&self, id: PaneId) -> (u16, u16) {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.dims)
            .unwrap_or((1, 1))
    }

    fn resize(&self, id: PaneId, cols: u16, rows: u16) {
        let params = invoke(
            &mux_action_path(RESIZE_ACTION),
            json!({ "id": id.0, "cols": cols, "rows": rows }),
        );
        // Advance the tracked size only on a SUCCESSFUL resize (else it is retried, not
        // latched). Addressed by IDENTITY: the write-back finds the pane by `id`, so a
        // freed/reused entry can never latch a resize onto a different pane (F8 dissolves).
        if self.request("scene/invoke", params, "resize").is_some()
            && let Some(pane) = self.lock_cache().iter_mut().find(|pane| pane.id == id)
        {
            pane.dims = (cols, rows);
        }
    }

    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool {
        let args = json!({
            "key": key,
            "ctrl": mods.ctrl,
            "alt": mods.alt,
            "shift": mods.shift,
            "super": mods.sup,
        });
        self.request(
            "scene/invoke",
            invoke(&pane_input_path(id.0, KEY_ACTION), args),
            "send_key",
        )
        .is_some()
    }

    fn send_text(&self, id: PaneId, text: &str) -> bool {
        let params = invoke(&pane_input_path(id.0, TEXT_ACTION), json!({ "text": text }));
        self.request("scene/invoke", params, "send_text").is_some()
    }

    fn pane_full_text(&self, id: PaneId) -> String {
        let params = json!({ "path": pane_input_path(id.0, FULL_TEXT_SLOT) });
        self.request("scene/query", params, "pane_full_text")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn pane_command_label(&self, id: PaneId) -> String {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.label.clone())
            .unwrap_or_default()
    }

    /// Served from the mirror the poll thread refreshes (no socket round-trip on the
    /// paint path); the title re-adopts the host's on every wake, so it tracks a shell
    /// rewriting it each prompt.
    fn pane_title(&self, id: PaneId) -> Option<String> {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.title.clone())
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

/// A host pane as the pane-list query reports it, before its frame is fetched.
struct PaneSeed {
    id: PaneId,
    label: String,
    /// The child's live OSC window title, `None` if it has set none (the wire sends
    /// `null`).
    title: Option<String>,
    dims: (u16, u16),
}

/// Query the host's pane list (`/sprag_mux/external/panes`), returning a [`PaneSeed`]
/// per pane in host order.
fn query_panes(conn: &mut HostConn) -> io::Result<Vec<PaneSeed>> {
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
            // `null` (child set no title) and a missing key both mean "no title".
            let title = pane["title"].as_str().map(str::to_owned);
            let cols = u16::try_from(pane["cols"].as_u64().unwrap_or(1)).unwrap_or(1);
            let rows = u16::try_from(pane["rows"].as_u64().unwrap_or(1)).unwrap_or(1);
            Ok(PaneSeed {
                id: PaneId(id),
                label,
                title,
                dims: (cols, rows),
            })
        })
        .collect()
}

/// The pane set the wire client boots with, in host order, each with its identity +
/// host-reported dims. This client mirrors ALL the host's panes; "slots" and the display
/// cap are the GUI `SlotView`'s concern, so there is NO cap here.
///
/// * **Attach mode** (the GUI reached an operator-run host) ADOPTS the host's live panes
///   as-is — the tmux-attach semantics (no spawn / truncate to a GUI-chosen count).
/// * **Spawn mode** (the GUI owns the host child) ensures exactly `n_panes` (the GUI is
///   the operator asking for its configured layout), spawning extras running `argv` at
///   `cols x rows` to reach it, then takes the first `n_panes`.
fn boot_panes(
    conn: &mut HostConn,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
    n_panes: usize,
    attach: bool,
) -> io::Result<Vec<PaneSeed>> {
    if attach {
        return query_panes(conn);
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

/// Fetch each seeded pane's live cell frame off the connection, in host order — the shared
/// fetch loop behind BOTH boot ([`build_cache`]) and each poll wake ([`refresh_to_set`]). A
/// pane whose fetch fails (it closed between the pane-list query and here — a real attach
/// race, since the host set is operator-controlled) is SKIPPED + logged rather than
/// aborting; the caller's [`merge_panes`] then drops a frameless NEWCOMER (retried next
/// wake) or keeps a SURVIVOR's last frame, so the client never mirrors a frameless pane and
/// [`pane_ids`](HostClient::pane_ids) omits it until it has one.
fn fetch_frames(conn: &mut HostConn, seeds: &[PaneSeed]) -> Vec<(PaneId, CellFrame)> {
    let mut fetched = Vec::with_capacity(seeds.len());
    for seed in seeds {
        match fetch_frame(conn, seed.id.0, 0) {
            Ok(frame) => fetched.push((seed.id, frame)),
            Err(error) => tracing::debug!(
                target: "sprag_gui::wire",
                pane = seed.id.0,
                %error,
                "pane frame fetch failed; not mirrored this wake (retried next)",
            ),
        }
    }
    fetched
}

/// The pane cache the wire client boots with — the BOOT case of the same
/// fetch+[`merge_panes`] path each poll wake runs, with an empty `existing` (every seed is a
/// newcomer, taking its fetched frame or dropped if frameless). One merge SSOT for boot and
/// steady-state, so they can never diverge (the R120 attach-race tolerance falls out of the
/// shared drop-if-frameless rule; the dock topology projects from `SlotView`'s occupied
/// slots, not a count, so a mid-boot close orphans nothing).
fn build_cache(conn: &mut HostConn, seeds: Vec<PaneSeed>) -> Vec<WirePane> {
    let fetched = fetch_frames(conn, &seeds);
    merge_panes(&[], &seeds, &fetched)
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

/// Refresh the cache to the host's live pane set (Round 2b live delta): mirror `seeds`
/// (the re-queried pane list, host order) — DROP panes no longer present, ADD new panes
/// (seeding their tracked dims from the query + fetching an initial frame), and REFRESH
/// every survivor's live frame — while PRESERVING each survivor's GUI-tracked dims (those
/// advance only on a successful `resize`, never clobbered by the query's momentary size)
/// and label. Frames are fetched OFF the cache lock (never a socket call while locked);
/// the rebuilt Vec swaps in under ONE lock, so a concurrent UI-thread read/resize sees an
/// atomic set. A frame fetch that fails is tolerated: a SURVIVOR keeps its last frame
/// (transient), a NEWCOMER is skipped this wake (retried next), so the GUI never mirrors a
/// frameless pane, and [`pane_ids`](HostClient::pane_ids) omits it until it has a frame.
fn refresh_to_set(conn: &mut HostConn, cache: &Cache, seeds: &[PaneSeed]) {
    let fetched = fetch_frames(conn, seeds); // off the lock (never a socket call while locked)
    // Rebuild the cache in host order under one lock (the pure merge is `merge_panes`).
    let mut guard = lock_cache(cache);
    let rebuilt = merge_panes(&guard, seeds, &fetched);
    *guard = rebuilt;
}

/// Merge the re-queried host pane list into the cache — PURE, so the dims/label authority +
/// newcomer-skip policy is unit-tested without a socket. Produces the new cache Vec in host
/// (`seeds`) order. The two per-pane fields split by AUTHORITY:
///
/// * **dims** are GUI-authoritative (advanced only on a successful `resize`, may lag the
///   query's momentary size), so a SURVIVOR keeps its tracked `prior.dims`; a NEWCOMER seeds
///   from the query.
/// * **label** is HOST-authoritative (set at spawn, the GUI never mutates it), so it ALWAYS
///   comes from the query (`seed.label`) — a survivor adopts any host relabel rather than
///   freezing the first-seen name.
/// * **title** (the child's live `OSC 0`/`OSC 2` window title, R128) is HOST-authoritative
///   and genuinely DYNAMIC — a shell rewrites it on every prompt — so a survivor ALWAYS
///   re-adopts `seed.title`, including back to `None` if the child clears it. This is the
///   dynamic-title case the label rule above was written to be forward-correct for.
///
/// The frame is the freshly-`fetched` one, else a survivor's last frame if this wake's fetch
/// missed; a NEWCOMER with no frame yet is DROPPED (retried next wake, so no frameless pane
/// is ever mirrored — [`pane_ids`](HostClient::pane_ids) omits it). A pane absent from
/// `seeds` is gone (not carried over). Boot is the `existing == &[]` case (all newcomers).
fn merge_panes(
    existing: &[WirePane],
    seeds: &[PaneSeed],
    fetched: &[(PaneId, CellFrame)],
) -> Vec<WirePane> {
    let mut rebuilt = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let prior = existing.iter().find(|pane| pane.id == seed.id);
        let frame = fetched
            .iter()
            .find(|(id, _)| *id == seed.id)
            .map(|(_, frame)| frame.clone())
            .or_else(|| prior.map(|pane| pane.frame.clone()));
        let Some(frame) = frame else {
            continue; // a brand-new pane whose first frame is not here yet — next wake
        };
        rebuilt.push(WirePane {
            id: seed.id,
            label: seed.label.clone(), // host-authoritative — always the query's label
            title: seed.title.clone(), // host-authoritative + dynamic — re-adopt every wake
            frame,
            dims: prior.map_or(seed.dims, |pane| pane.dims), // GUI-authoritative — keep tracked
        });
    }
    rebuilt
}

/// Start the background poll: block on `scene/waitFor {since}`, then MIRROR the host's
/// live pane set into the [`Cache`] on each wake — re-query the pane list and add/remove
/// panes ([`refresh_to_set`]) so a host-side spawn/close is reflected, not just existing
/// panes refreshed — and call `on_change` (repaint). The R118 notification rail bumps the
/// scene revision on a set change too, so a spawn/close wakes this parked `waitFor` just
/// like output does.
///
/// Exits ONLY when `stop` is set (Drop cancels the parked read via a shutdown handle) or
/// the parked `scene/waitFor` itself errors (the host connection was lost — logged at
/// `warn`, since live updates then stop and the GUI would otherwise silently freeze). A
/// pane-list re-query failing a single wake is NOT fatal — it falls back to refreshing the
/// current cache ids (a cache-derived seed snapshot through the same [`refresh_to_set`]
/// path, so no adds/removes) and the set change is picked up on a later wake.
///
/// # Errors
///
/// Fails if the poll thread cannot be spawned (matching `spawn_or_attach`'s contract
/// rather than panicking inside it).
fn spawn_poll(
    mut conn: HostConn,
    cache: Cache,
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
                // Re-query the live pane set each wake so a host-side spawn/close is
                // MIRRORED (cache add/remove), not just existing panes refreshed. On a
                // transient query failure, refresh the known set instead so liveness holds
                // (the set change is caught on a later wake).
                match query_panes(&mut conn) {
                    Ok(seeds) => refresh_to_set(&mut conn, &cache, &seeds),
                    Err(error) => {
                        tracing::debug!(
                            target: "sprag_gui::wire",
                            %error,
                            "pane-list re-query failed this wake; refreshing the known set",
                        );
                        // Fall back to the cache's current ids as the seed set (no
                        // adds/removes), through the SAME refresh_to_set path, so a transient
                        // query error still refreshes live output; the real set change is
                        // caught on a later wake.
                        let seeds: Vec<PaneSeed> = lock_cache(&cache)
                            .iter()
                            .map(|pane| PaneSeed {
                                id: pane.id,
                                label: pane.label.clone(),
                                // Re-query failed, so the host's current title is unknown —
                                // KEEP the last-known one rather than blanking the display.
                                title: pane.title.clone(),
                                dims: pane.dims,
                            })
                            .collect();
                        refresh_to_set(&mut conn, &cache, &seeds);
                    }
                }
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

    /// A cell frame `n` cols wide, so a test can tell frames apart by `cells.cols()`.
    fn frame(cols: u16) -> CellFrame {
        CellFrame {
            cells: GridBuffer::new(cols, 1),
            facts: PaneScrollFacts {
                scrollback_len: 0,
                visible_rows: 1,
            },
        }
    }

    #[test]
    fn merge_panes_keeps_survivor_dims_adds_newcomers_and_drops_gone_or_frameless() {
        // Existing cache: pane 10 (dims tracked at 80x24 by a prior resize), pane 11.
        let existing = vec![
            WirePane {
                id: PaneId(10),
                label: "bash".to_owned(),
                title: None,
                frame: frame(3),
                dims: (80, 24),
            },
            WirePane {
                id: PaneId(11),
                label: "cat".to_owned(),
                title: None,
                frame: frame(3),
                dims: (40, 12),
            },
        ];
        // Host now (host order): pane 10 (survivor, query reports a DIFFERENT momentary
        // size 100x30 + a relabel), pane 12 (newcomer), pane 13 (newcomer, no frame yet).
        // Pane 11 vanished.
        let seeds = vec![
            PaneSeed {
                id: PaneId(10),
                label: "bash-relabeled".to_owned(),
                title: None,
                dims: (100, 30),
            },
            PaneSeed {
                id: PaneId(12),
                label: "vim".to_owned(),
                title: None,
                dims: (80, 24),
            },
            PaneSeed {
                id: PaneId(13),
                label: "top".to_owned(),
                title: None,
                dims: (80, 24),
            },
        ];
        let fetched = vec![(PaneId(10), frame(5)), (PaneId(12), frame(7))]; // 13 not fetched

        let merged = merge_panes(&existing, &seeds, &fetched);

        // Host order; pane 11 gone, pane 13 dropped (no frame yet).
        assert_eq!(
            merged.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![PaneId(10), PaneId(12)],
        );
        // Survivor 10 splits by authority: KEEPS its GUI-tracked dims (not the query's
        // momentary size) but ADOPTS the query's label (host-authoritative), takes the fresh
        // frame.
        assert_eq!(
            merged[0].dims,
            (80, 24),
            "survivor keeps its GUI-tracked dims, not the query's momentary size"
        );
        assert_eq!(
            merged[0].label, "bash-relabeled",
            "survivor adopts the query's label (host-authoritative), not the stale first-seen one"
        );
        assert_eq!(
            merged[0].frame.cells.cols(),
            5,
            "survivor took the fresh frame"
        );
        // Newcomer 12 seeds dims + label from the query, takes its fetched frame.
        assert_eq!(merged[1].dims, (80, 24));
        assert_eq!(merged[1].label, "vim");
        assert_eq!(merged[1].frame.cells.cols(), 7);
    }

    #[test]
    fn merge_panes_survivor_keeps_its_last_frame_when_the_refetch_missed() {
        let existing = vec![WirePane {
            id: PaneId(10),
            label: "bash".to_owned(),
            title: None,
            frame: frame(3),
            dims: (80, 24),
        }];
        let seeds = vec![PaneSeed {
            id: PaneId(10),
            label: "bash".to_owned(),
            title: None,
            dims: (80, 24),
        }];
        let merged = merge_panes(&existing, &seeds, &[]); // fetch missed this wake
        assert_eq!(merged.len(), 1, "the survivor is still mirrored");
        assert_eq!(
            merged[0].frame.cells.cols(),
            3,
            "kept its last frame when the refetch missed (not dropped)"
        );
    }

    /// The OSC title (R128) is HOST-authoritative AND dynamic — a shell rewrites it on
    /// every prompt. So a survivor must RE-ADOPT the query's title each wake, including
    /// back to `None` when the child clears it; freezing the first-seen title would pin a
    /// stale name (`vim README` long after vim exited).
    #[test]
    fn merge_panes_survivor_readopts_the_hosts_live_title_including_clearing_it() {
        let existing = vec![
            WirePane {
                id: PaneId(10),
                label: "bash".to_owned(),
                title: Some("stale: vim README".to_owned()),
                frame: frame(3),
                dims: (80, 24),
            },
            WirePane {
                id: PaneId(11),
                label: "bash".to_owned(),
                title: Some("about to be cleared".to_owned()),
                frame: frame(3),
                dims: (80, 24),
            },
        ];
        let seeds = vec![
            PaneSeed {
                id: PaneId(10),
                label: "bash".to_owned(),
                title: Some("coin@host:~".to_owned()), // child retitled at the new prompt
                dims: (80, 24),
            },
            PaneSeed {
                id: PaneId(11),
                label: "bash".to_owned(),
                title: None, // child cleared its title
                dims: (80, 24),
            },
        ];
        let fetched = vec![(PaneId(10), frame(5)), (PaneId(11), frame(5))];

        let merged = merge_panes(&existing, &seeds, &fetched);

        assert_eq!(
            merged[0].title.as_deref(),
            Some("coin@host:~"),
            "survivor re-adopts the host's live title, never freezing the first-seen one",
        );
        assert_eq!(
            merged[1].title, None,
            "a cleared title clears the mirror too (host is authoritative both ways)",
        );
    }
}
