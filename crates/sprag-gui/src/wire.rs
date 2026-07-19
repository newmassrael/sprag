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
//! * A `sprag-term` DAEMON owns the `Workspace` + PTYs. The GUI CONNECT-OR-SPAWNS it on the
//!   well-known socket: join the one already running, or spawn a detached `--daemon` (which
//!   the GUI does NOT own — no kill, no `PR_SET_PDEATHSIG`) and connect. Closing the GUI
//!   leaves the daemon and the user's shells running; a fresh GUI reattaches. The tmux/mosh
//!   split, now with tmux's detach: the server outlives the client. `SPRAG_GUI_HOST_SOCK`
//!   overrides the socket path (a test, or an operator-run host).
//! * The GUI reads each pane's cells + input over the wire. Two connections avoid
//!   head-of-line blocking: a background POLL connection parks on the async
//!   `scene/waitFor` change-notification, and a REQUEST connection serves the UI
//!   thread's reads / input / resize.
//!
//! ## Which session
//!
//! One host holds many sessions; a client acts on exactly one. At boot the client resolves
//! its session ([`resolve_session`]) and scopes BOTH connections to it
//! ([`HostConn::scope_to`]), so every request names it and none can leak into another. Naming
//! a session ([`SESSION_ENV`]) ATTACHES to it (adopt its live panes — the tmux reattach);
//! naming none ALLOCATES a fresh session and spawns this client's panes into it, so two GUIs
//! against one host never mirror the same session. A spawned daemon boots EMPTY (`--daemon`),
//! so there is no stray default-session pane — every pane lives in some client's session.
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
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use pinion_core::{GridBuffer, QuitSink};
use serde_json::{Value, json};
use sprag_host::wire::{
    FULL_TEXT_SLOT, KEY_ACTION, LAYOUT_SLOT, NEW_SESSION_ACTION, PANES_SLOT, RESIZE_ACTION,
    SET_FLOATING_ACTION, SET_LAYOUT_ACTION, SPAWN_ACTION, TEXT_ACTION, cells_slot_at,
};
use sprag_host::{CellFrame, HostClient, PaneScrollFacts, mux_action_path, pane_input_path};
use sprag_input::Modifiers;
use sprag_rpc::{HostConn, runtime_path};
use sprag_terminal::{LayoutSnapshot, LayoutWire, PaneId};

/// How long to wait for a just-spawned daemon's socket to accept — covers its bind race.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Env override: the host socket path to connect-or-spawn on, instead of the well-known
/// `sprag-host.sock` (a test's private socket, or an operator-run host).
const HOST_SOCK_ENV: &str = "SPRAG_GUI_HOST_SOCK";
/// Env override: the `sprag-term` binary to spawn (else the sibling of the GUI exe,
/// else `sprag-term` on `PATH`).
const HOST_BIN_ENV: &str = "SPRAG_GUI_HOST_BIN";
/// Env: the SESSION to attach to (adopt its live panes) — the reattach gesture. Absent, the
/// client allocates a fresh session and spawns its own panes into it, so two GUIs never
/// mirror one session (the owner's several-windows workflow). A `sprag attach` CLI is a later
/// increment; env is the established GUI-config channel (`SPRAG_GUI_PANES`/`_HOST_SOCK`/…).
const SESSION_ENV: &str = "SPRAG_GUI_SESSION";

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

/// The host's current arrangement, mirrored. Shared between the UI thread (which projects
/// it, and replaces it with the answer to its own writes) and the poll thread (which
/// re-reads it whenever the host says the scene moved), under one lock.
///
/// Mirrored rather than fetched on demand for the same reason the pane frames are: the
/// paint path must never make a socket call. A client reads this every frame to notice its
/// projection is stale, so a round trip there would put the wire on the UI thread's hot
/// path — and a client whose arrangement is a projection reads it a great deal.
type LayoutMirror = Arc<Mutex<LayoutSnapshot>>;

/// Lock the mirrored arrangement, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_layout(layout: &Mutex<LayoutSnapshot>) -> MutexGuard<'_, LayoutSnapshot> {
    layout.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Store `snapshot` in the mirror unless it is OLDER than what is already there — the ONE
/// place the mirror is written, shared by the poll thread and the UI thread's writes.
///
/// The revision is monotonic per window, so an older snapshot is stale by definition. Two
/// threads race here: the poll thread reads the layout OFF the lock and stores it after, so
/// its read can be overtaken by a UI-thread write that lands first. Storing unconditionally
/// let the mirror move BACKWARD — the client then saw a revision it had already passed,
/// re-projected the pre-gesture tree, and visibly snapped the user's just-settled divider
/// back until an unrelated later bump healed it.
fn store_layout(layout: &Mutex<LayoutSnapshot>, snapshot: LayoutSnapshot) {
    let mut mirror = lock_layout(layout);
    if snapshot.revision >= mirror.revision {
        *mirror = snapshot;
    } else {
        tracing::trace!(
            target: "sprag_gui::wire",
            stale = snapshot.revision,
            held = mirror.revision,
            "dropped a layout read overtaken by a newer one",
        );
    }
}

/// Read the host's arrangement off the wire — the ONE place the `layout` slot is queried,
/// shared by the boot read and the poll thread's refresh.
fn query_layout(conn: &mut HostConn) -> io::Result<LayoutSnapshot> {
    let value = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(LAYOUT_SLOT) }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// The GUI's wire client of a `sprag-term` host. See the module docs.
pub(crate) struct WireHost {
    /// The pane data cache ([`Cache`]) in host order: identity + live frame + tracked
    /// dims per pane. The UI thread reads it under a brief lock; the poll thread refreshes
    /// each pane's frame under the same lock. Addressed by [`PaneId`] — the GUI's
    /// `SlotView` maps display slots onto these ids.
    cache: Cache,
    /// The host's arrangement, mirrored ([`LayoutMirror`]) — what this client PROJECTS.
    /// The poll thread re-reads it on every scene change; a write on the UI thread replaces
    /// it with the host's canonical answer.
    layout: LayoutMirror,
    /// The UI thread's request connection (reads / input / resize). `RefCell`, not
    /// `Mutex`: `WireHost` is UI-thread-only (see the module docs), and the poll thread
    /// owns a SEPARATE connection.
    conn: RefCell<HostConn>,
    /// Set on Drop to stop the poll loop.
    stop: Arc<AtomicBool>,
    /// A shutdown handle onto the poll connection: Drop calls `shutdown(Both)` on it to
    /// cancel the poll thread's parked `scene/waitFor` read so the join is deterministic.
    /// The host is a daemon we never kill, so this is the ONLY thing that unblocks the parked
    /// read on teardown — there is no child-exit to close the socket for us.
    poll_shutdown: UnixStream,
    /// The background change-notification -> repaint thread, joined on Drop.
    poll: Option<JoinHandle<()>>,
}

impl WireHost {
    /// Connect-or-spawn a `sprag-term` daemon, resolve this client's session, and boot
    /// `n_panes` panes running `argv` (`None` = the host's default `$SHELL`) at `cols x rows`
    /// into it (or adopt an attached session's panes), then start the background
    /// change-notification -> `on_change` repaint poll.
    ///
    /// `quit` is the shell's [`QuitSink`]: when the poll thread's parked
    /// `scene/waitFor` returns an error while the host is NOT being torn down by us
    /// (a definitive host-gone, the daemon exited under a detached client), it asks
    /// the shell to end — the tmux convention that a client detaches when its server
    /// dies. It rides here as a `Send` handle exactly as `on_change` does.
    ///
    /// # Errors
    ///
    /// Any failure to spawn the daemon, connect to its socket within [`CONNECT_TIMEOUT`], or
    /// resolve the session / boot the panes over RPC. The daemon is NOT reaped on failure —
    /// it is a detached process this GUI does not own.
    pub(crate) fn spawn_or_attach(
        argv: Option<Vec<String>>,
        cols: u16,
        rows: u16,
        n_panes: usize,
        on_change: Box<dyn Fn() + Send>,
        quit: Arc<dyn QuitSink>,
    ) -> io::Result<Self> {
        // Connect-or-spawn on the well-known socket. A daemon there outlives every client, so
        // first try to JOIN one; only if none answers do we spawn a detached `--daemon` and
        // connect through the bind-race retry. We do NOT own its lifetime — no kill, no
        // PDEATHSIG — which is the whole point: the session survives this GUI. A spawn RACE is
        // safe, because the daemon's single-instance flock leaves exactly one alive and every
        // client connects to it.
        let sock = host_socket();
        let mut conn = match HostConn::connect(&sock, Duration::ZERO) {
            Ok(conn) => {
                tracing::info!(target: "sprag_gui::wire", path = %sock.display(), "joined a running host");
                conn
            }
            Err(_) => {
                spawn_daemon(&sock)?;
                tracing::info!(target: "sprag_gui::wire", path = %sock.display(), "spawned a daemon host");
                HostConn::connect(&sock, CONNECT_TIMEOUT)?
            }
        };

        // Resolve WHICH session this client acts on before booting panes, and scope every
        // request to it (both this connection and the poll one below), so a request can never
        // silently land in another session. Naming one ATTACHES (adopt its panes); naming none
        // ALLOCATES a fresh one (spawn our own panes) — the "each launch is its own session"
        // model. `boot_panes` branches on `created`, replacing the old "did we spawn the host"
        // key with "did we create the session".
        let (session, created) = resolve_session(&mut conn)?;
        conn.scope_to(session.clone());
        let seeds = boot_panes(&mut conn, argv.as_deref(), cols, rows, n_panes, created)?;

        // Subscribe-then-snapshot: read the change-notification baseline BEFORE the
        // initial frame fetches, so output landing during boot makes the first `waitFor`
        // fire immediately (catch-up) instead of being lost against a stale first frame.
        let since0 = read_revision(&mut conn)?;
        let cache: Cache = Arc::new(Mutex::new(build_cache(&mut conn, seeds)));
        // The arrangement is part of BOOTING, not a best-effort read: this client renders a
        // projection of it, so failing to read it means there is nothing honest to paint.
        // Failing the attach says exactly that, where a silent empty tree would leave a
        // blank window over live PTYs and blame nothing.
        let layout: LayoutMirror = Arc::new(Mutex::new(query_layout(&mut conn)?));

        // The poll thread's own connection — a parked `scene/waitFor` on it never
        // blocks the request connection above (separate host handler threads). Scoped to the
        // SAME session, so its `waitFor`/`revision`/re-queries watch the client's own session
        // and never another's.
        let mut poll_conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        poll_conn.scope_to(session);
        let poll_shutdown = poll_conn.shutdown_handle()?;
        let stop = Arc::new(AtomicBool::new(false));
        let poll = spawn_poll(
            poll_conn,
            Arc::clone(&cache),
            Arc::clone(&layout),
            on_change,
            quit,
            Arc::clone(&stop),
            since0,
        )?;

        Ok(Self {
            cache,
            layout,
            conn: RefCell::new(conn),
            stop,
            poll_shutdown,
            poll: Some(poll),
        })
    }

    /// Send an arrangement write and adopt the host's canonical answer — the ONE place this
    /// client's mirror is replaced by a write's result, shared by
    /// [`set_layout`](HostClient::set_layout) and [`set_floating`](HostClient::set_floating).
    ///
    /// The answer is authoritative: it carries the tree as the host stores it, with any
    /// divider this client minted now NAMED, so adopting it (rather than what we sent)
    /// is what keeps this client a projection. A write that does not land leaves the
    /// mirror alone and answers with it — a failed write must report the arrangement that
    /// is actually in force, never the one we hoped for.
    fn write_layout(&self, params: Value, ctx: &str) -> LayoutSnapshot {
        match self
            .request("scene/invoke", params, ctx)
            .and_then(|value| serde_json::from_value::<LayoutSnapshot>(value).ok())
        {
            Some(snapshot) => {
                store_layout(&self.layout, snapshot.clone());
                snapshot
            }
            None => self.layout(),
        }
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
        //
        // A `scene/query` against the same `cells.<offset>` family the live view reads, with
        // the offset riding the path. It was an invoke until PR-61 landed, which made a wheel
        // tick a `Mutate`: it bumped the scene revision and woke every OTHER attached client's
        // parked `waitFor` into a full re-fetch. Bounded and terminating, so never the R152
        // livelock — but the same defect, and a read has no business waking anyone.
        let params = json!({ "path": pane_input_path(id.0, &cells_slot_at(offset_lines)) });
        self.request("scene/query", params, "pane_cells")
            .and_then(|value| serde_json::from_value::<CellFrame>(value).ok())
            .map_or_else(|| self.live_cells(id), |frame| frame.cells)
    }

    /// The mirrored arrangement — a lock and a clone, never a socket call, so the paint
    /// path can read it every frame to notice its projection is stale (see [`LayoutMirror`]).
    ///
    /// Booted from a real read and kept current by the poll thread; a transient wire failure
    /// leaves the LAST KNOWN arrangement standing rather than reporting an empty one, since
    /// "the host did not answer" and "this window tiles nothing" are opposite facts that
    /// must never arrive as the same value.
    fn layout(&self) -> LayoutSnapshot {
        lock_layout(&self.layout).clone()
    }

    fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot {
        self.write_layout(
            json!({
                "path": mux_action_path(SET_LAYOUT_ACTION),
                "args": { "tree": tree, "expected_revision": expected },
            }),
            "set_layout",
        )
    }

    fn set_floating(&self, id: PaneId, floating: bool) -> LayoutSnapshot {
        self.write_layout(
            json!({
                "path": mux_action_path(SET_FLOATING_ACTION),
                "args": { "id": id.0, "floating": floating },
            }),
            "set_floating",
        )
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
        // deterministic. All stream clones name one OS socket, so this reaches the reader.
        let _ = self.poll_shutdown.shutdown(Shutdown::Both);
        // The host is a DAEMON we do not own — closing this client leaves it (and the user's
        // shells) running, which is the whole detach/reattach point. So there is nothing to
        // kill here; we only tear down OUR connections and poll thread. When the last live pane
        // across every session finally exits, the daemon self-cleans (its own reaper).
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

/// The host socket the GUI connects to: `SPRAG_GUI_HOST_SOCK` if set (a test or an
/// operator-run host), else the WELL-KNOWN `sprag-host.sock` under the runtime dir — the same
/// default a spawned `sprag-term --daemon` binds, so a spawner and its daemon agree with no
/// per-instance keying. One endpoint, whether we join an existing daemon or spawn one.
fn host_socket() -> PathBuf {
    match std::env::var_os(HOST_SOCK_ENV) {
        Some(path) => PathBuf::from(path),
        None => runtime_path(sprag_rpc::HOST_SOCKET_NAME),
    }
}

/// Spawn a detached `sprag-term --daemon` bound to `sock`.
///
/// The daemon self-daemonizes: the process we spawn here is a short-lived intermediate that
/// forks the real daemon and exits, so we WAIT on it (reaping the intermediate, not a zombie)
/// and never track the daemon — it is reparented to init and outlives us by design. Its own
/// stdio is redirected to a log after the fork, so the pipes we hand it only cover the
/// pre-fork instant (no output there); we null them. It boots EMPTY (`--daemon`) — this
/// client's panes are spawned into its own session afterwards, not by the daemon's boot.
fn spawn_daemon(sock: &Path) -> io::Result<()> {
    let mut intermediate = Command::new(host_bin())
        .arg("--daemon")
        .env("SPRAG_HOST_RPC_SOCK", sock)
        .env("SPRAG_HOST_RPC", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    // The intermediate exits the instant it has forked the daemon, so this returns at once.
    let _ = intermediate.wait();
    Ok(())
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

/// Resolve WHICH session this client acts on, over `conn` (before it is scoped).
///
/// Returns the session name and whether this client CREATED it:
/// * [`SESSION_ENV`] names one → ATTACH (`created = false`): the session must already exist
///   on the reached host; its live panes are adopted (tmux reattach). A name no session
///   carries makes the first scoped read fail, which fails the boot honestly rather than
///   silently opening an empty window.
/// * absent → ALLOCATE a fresh session (`created = true`) via the registry's own auto-naming
///   ([`NEW_SESSION_ACTION`] with no name), so two clients never invent one name and race.
///   This call is deliberately made BEFORE the connection is scoped — creating a session is a
///   registry-wide act, not one scoped to a session that does not exist yet.
///
/// **Bound (allocate path, joining an existing daemon):** the freshly-allocated session is
/// empty until [`boot_panes`] spawns its first pane one round trip later, and the daemon's
/// self-cleaning counts an empty session as having no live panes. So if the daemon's last
/// OTHER pane exits in that create→spawn window, the daemon can self-exit and this boot fails
/// with `UnexpectedEof` — an honest error, never corruption, and unreachable for the FIRST
/// client of a fresh daemon (no pane exists to die). It resolves when session-close semantics
/// let a just-created session pin liveness (a later increment); until then the window is one
/// RPC wide, so the spawn follows the allocate promptly.
fn resolve_session(conn: &mut HostConn) -> io::Result<(String, bool)> {
    if let Some(name) = std::env::var_os(SESSION_ENV).filter(|name| !name.is_empty()) {
        return Ok((name.to_string_lossy().into_owned(), false));
    }
    let answer = conn.call(
        "scene/invoke",
        invoke(&mux_action_path(NEW_SESSION_ACTION), json!({})),
    )?;
    let name = answer
        .as_str()
        .ok_or_else(|| io::Error::other("new_session did not answer with a name"))?
        .to_owned();
    Ok((name, true))
}

/// The pane set the wire client boots with, in host order, each with its identity +
/// host-reported dims. This client mirrors ALL the host's panes; "slots" and the display
/// cap are the GUI `SlotView`'s concern, so there is NO cap here.
///
/// * **Attached** (`created == false` — the GUI named an existing session) ADOPTS that
///   session's live panes as-is — the tmux-attach semantics (no spawn / truncate to a
///   GUI-chosen count).
/// * **Created** (`created == true` — the GUI allocated a fresh session) ensures exactly
///   `n_panes` (the GUI is the operator asking for its configured layout), spawning extras
///   running `argv` at `cols x rows` to reach it, then takes the first `n_panes`.
fn boot_panes(
    conn: &mut HostConn,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
    n_panes: usize,
    created: bool,
) -> io::Result<Vec<PaneSeed>> {
    if !created {
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
        match fetch_frame(conn, seed.id.0) {
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

/// Fetch one pane's LIVE cell frame — [`cells_slot_at(0)`](cells_slot_at), the live member
/// of the host's `cells.<offset>` query family — as the shared [`CellFrame`] the host
/// serializes, deserialized on this end (one wire type).
///
/// A `scene/query` (READ) because this runs on every poll-thread wake, and an invoke is a
/// `MethodOcc::Mutate` that bumps the scene revision — so fetching the frame over an action
/// woke the very `scene/waitFor` that dispatched the wake, a ~30Hz idle livelock. A query
/// bumps nothing, so the loop parks until real output moves the revision.
fn fetch_frame(conn: &mut HostConn, id: u64) -> io::Result<CellFrame> {
    let value = conn.call(
        "scene/query",
        json!({ "path": pane_input_path(id, &cells_slot_at(0)) }),
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
/// Exits ONLY when `stop` is set (Drop cancels the parked read via a shutdown handle) or a
/// request fails DEFINITIVELY ([`detach_reason`]). A stop-initiated error is our own graceful
/// teardown and is silent; a definitive failure while `stop` is CLEAR detaches the client
/// (asks the shell to quit via `quit`) — the tmux rule that a client leaves when it can no
/// longer serve its session. There are two such failures, told apart only for the log: the
/// HOST exited (the socket closed — also how a killed LAST session ends, the whole daemon
/// going), or THIS client's SESSION was killed while the daemon serves on for others (a scoped
/// request is refused). The session-kill case is caught on the RE-QUERY, so the client detaches
/// at once rather than repainting one stale frame first. A TRANSIENT hiccup is not fatal — the
/// long-poll re-parks and a pane-list re-query falls back to refreshing the known cache ids
/// (through the same [`refresh_to_set`] path, no adds/removes), the change picked up on a later
/// wake.
///
/// # Errors
///
/// Fails if the poll thread cannot be spawned (matching `spawn_or_attach`'s contract
/// rather than panicking inside it).
fn spawn_poll(
    mut conn: HostConn,
    cache: Cache,
    layout: LayoutMirror,
    on_change: Box<dyn Fn() + Send>,
    quit: Arc<dyn QuitSink>,
    stop: Arc<AtomicBool>,
    mut since: u64,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("sprag-gui-wire-poll".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let response = match conn.call("scene/waitFor", json!({ "since": since })) {
                    Ok(value) => value,
                    Err(error) => match detach_reason(&error) {
                        // A DEFINITIVE end (host gone or our session killed) — detach, unless
                        // WE initiated the teardown (a stop-initiated error is the graceful Drop
                        // that shut this socket down, and quitting then would be redundant).
                        Some(reason) => {
                            request_detach(&quit, stop.load(Ordering::Relaxed), reason, &error);
                            break;
                        }
                        // A transient hiccup — re-park the long-poll rather than end the client.
                        None => continue,
                    },
                };
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                since = response["revision"].as_u64().unwrap_or(since);
                // Re-query the live pane set each wake so a host-side spawn/close is MIRRORED
                // (cache add/remove), not just existing panes refreshed. A DEFINITIVE failure
                // (our session was killed) detaches at once — no stale repaint first; a transient
                // one refreshes the known set instead so liveness holds (the change is caught on
                // a later wake).
                match query_panes(&mut conn) {
                    Ok(seeds) => refresh_to_set(&mut conn, &cache, &seeds),
                    Err(error) => {
                        if let Some(reason) = detach_reason(&error) {
                            request_detach(&quit, stop.load(Ordering::Relaxed), reason, &error);
                            break;
                        }
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
                // Re-read the arrangement each wake too, so a host-side change — another
                // attached client's gesture, a plugin's spawn, a float — reaches this
                // client's projection. A definitive failure detaches (as above); on a transient
                // one the last-known arrangement stands (a hiccup means "no news", never "your
                // layout is gone").
                match query_layout(&mut conn) {
                    Ok(snapshot) => store_layout(&layout, snapshot),
                    Err(error) => {
                        if let Some(reason) = detach_reason(&error) {
                            request_detach(&quit, stop.load(Ordering::Relaxed), reason, &error);
                            break;
                        }
                        tracing::debug!(
                            target: "sprag_gui::wire",
                            %error,
                            "layout re-read failed this wake; keeping the last-known arrangement",
                        );
                    }
                }
                on_change();
            }
        })
}

/// Why a poll-thread request failed, if the failure is DEFINITIVE and the client should detach
/// — `None` only for a genuinely transient hiccup to tolerate (re-park / keep the last frame).
///
/// Definitiveness is the DEFAULT: a broken pipe, a reset, an EOF — any dead connection — ends
/// this client, tmux's rule that a client leaves when it can no longer serve its session. Only
/// the handful of retryable kinds are tolerated; classifying a dead-socket write error
/// (`BrokenPipe`) as transient would spin the long-poll forever, never detaching. The message
/// separates the two REPORTABLE causes:
/// * [`Other`](io::ErrorKind::Other) — [`HostConn::call`] maps a JSON-RPC error object to this;
///   for a client that scopes every request to its session, the only such refusal is that
///   session being killed while the daemon lives on for other sessions.
/// * everything else definitive — the host socket is gone (the daemon exited, or a killed LAST
///   session took the whole daemon with it).
fn detach_reason(error: &io::Error) -> Option<&'static str> {
    match error.kind() {
        // Retryable: a signal interrupted the syscall, a non-blocking op would block, or a read
        // timed out. Re-park and try again; the connection itself is fine.
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => None,
        // A refusal the host actually answered with — for a scoped client, its session is gone.
        io::ErrorKind::Other => Some("this client's session was closed"),
        // Any other error means the connection is dead — the host is gone.
        _ => Some("the host exited"),
    }
}

/// Ask the shell to end — the tmux detach — unless WE initiated the teardown (`stopped`), in
/// which case the error is our own graceful `Drop` and quitting would be redundant.
fn request_detach(quit: &Arc<dyn QuitSink>, stopped: bool, reason: &str, error: &io::Error) {
    if !stopped {
        tracing::info!(target: "sprag_gui::wire", %error, "{reason}; requesting client exit");
        quit.request_quit();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::sync::atomic::AtomicUsize;

    /// A [`QuitSink`] that counts requests, so a test can assert the poll thread asked
    /// the shell to end (and did so across the thread boundary).
    #[derive(Default)]
    struct RecordingQuit(AtomicUsize);
    impl QuitSink for RecordingQuit {
        fn request_quit(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Unlinks a test socket file on scope exit — INCLUDING a panic (an assertion failure or
    /// a revert-proof run), which a bare end-of-test `remove_file` skips, leaking one socket
    /// per panicked run under the temp dir. Mirrors `wire_client.rs`'s `HostChild` discipline.
    struct SockGuard(PathBuf);
    impl Drop for SockGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// A throwaway host socket path unique to this CALL (pid + `tag`, so parallel test threads
    /// in one binary never collide — the R152/R153 socket-race lesson), pre-cleared.
    fn sock_path(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("sprag-wire-quit-{}-{tag}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    /// A connected [`HostConn`] whose server end is already CLOSED. The next `call` on the
    /// conn reads EOF — the exact wire condition a daemon exiting under a detached client
    /// produces. The listener + [`SockGuard`] are returned so the caller keeps them alive and
    /// the socket file is unlinked even on panic.
    fn a_dead_host_conn(tag: &str) -> (HostConn, UnixListener, SockGuard) {
        let path = sock_path(tag);
        let listener = UnixListener::bind(&path).expect("bind the throwaway host socket");
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect to it");
        // Accept then drop the server side: the client's next read returns EOF, which is
        // what `HostConn::call` maps to `UnexpectedEof` — the host is gone.
        let (server, _) = listener.accept().expect("accept the client");
        drop(server);
        (conn, listener, SockGuard(path))
    }

    /// The tmux convention, deterministically: when the poll thread's parked `scene/waitFor`
    /// fails and we are NOT tearing the host down ourselves, it asks the shell to quit. Driven
    /// over a REAL closed socket (no `sprag-term`, no global env, no `WireHost` env branch), so
    /// it exercises the actual `spawn_poll` error arm rather than a stand-in.
    ///
    /// REVERT-PROOF: delete the `quit.request_quit()` call in the error arm and this reads 0 —
    /// the guard is not vacuous, it pins the one line that turns a dead daemon into a client
    /// that exits instead of a window frozen over dead content.
    #[test]
    fn a_dead_host_asks_the_shell_to_quit() {
        let (conn, _listener, _guard) = a_dead_host_conn("gone");
        let quit = Arc::new(RecordingQuit::default());
        let stop = Arc::new(AtomicBool::new(false)); // NOT our teardown: the host died
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(LayoutSnapshot::default())),
            Box::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");
        poll.join().expect("the poll thread exited");

        assert_eq!(
            quit.0.load(Ordering::SeqCst),
            1,
            "a host that vanished under us must ask the client to exit exactly once",
        );
    }

    /// A connected [`HostConn`] whose server REFUSES every request with a JSON-RPC `-32602` —
    /// the wire condition a client meets when its SESSION is killed while the daemon serves on
    /// for others (`HostConn::call` maps the error object to [`io::ErrorKind::Other`], which
    /// [`detach_reason`] reads as "session gone"). It counts the requests it received and stops
    /// answering after two, so a client that FAILS to detach terminates the test (via EOF) rather
    /// than spinning forever. The count is what proves the client left on the FIRST refusal.
    fn a_session_killed_host_conn(
        tag: &str,
    ) -> (HostConn, JoinHandle<()>, SockGuard, Arc<AtomicUsize>) {
        use std::io::Write;
        let path = sock_path(tag);
        let listener = UnixListener::bind(&path).expect("bind the throwaway host socket");
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect to it");
        let seen = Arc::new(AtomicUsize::new(0));
        let seen_srv = Arc::clone(&seen);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept the client");
            let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
            let mut writer = stream;
            let mut line = String::new();
            // Refuse up to two requests with the same `-32602` the host raises for a scope naming
            // a killed session; stop after two so a non-detaching client hits EOF and the test
            // ends instead of the server answering a spin forever.
            while seen_srv.load(Ordering::SeqCst) < 2
                && reader.read_line(&mut line).is_ok_and(|n| n > 0)
            {
                seen_srv.fetch_add(1, Ordering::SeqCst);
                let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                let reply = json!({
                    "jsonrpc": "2.0",
                    "id": request["id"],
                    "error": { "code": -32602, "message": "no session named \"1\"" },
                });
                let _ = writeln!(writer, "{reply}");
                let _ = writer.flush();
                line.clear();
            }
        });
        (conn, server, SockGuard(path), seen)
    }

    /// The other definitive detach: a client whose SESSION was killed (the daemon still serves
    /// other sessions) meets a scoped-request REFUSAL, and leaves at once — tmux's "the session
    /// is gone, so the client detaches". Distinct from a dead host (which closes the socket);
    /// here the socket is alive and answers with an ERROR, and [`detach_reason`] tells them apart.
    ///
    /// The `seen == 1` assertion is what makes this NON-VACUOUS: it proves the client detached on
    /// the FIRST refusal, not after re-polling. REVERT-PROOF: change `detach_reason`'s `Other`
    /// arm to `None` and the client tolerates the refusal, re-polls (seen becomes 2), then
    /// detaches only on the EOF that follows — so `quit` still reads 1 (masking the defect) but
    /// `seen` reads 2 and this fails.
    #[test]
    fn a_killed_session_asks_the_shell_to_quit_on_the_first_refusal() {
        let (conn, server, _guard, seen) = a_session_killed_host_conn("killed");
        let quit = Arc::new(RecordingQuit::default());
        let stop = Arc::new(AtomicBool::new(false)); // NOT our teardown: the session was killed
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(LayoutSnapshot::default())),
            Box::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");
        poll.join().expect("the poll thread exited");
        server.join().expect("the server thread exited");

        assert_eq!(
            quit.0.load(Ordering::SeqCst),
            1,
            "a client whose session was killed must detach exactly once",
        );
        assert_eq!(
            seen.load(Ordering::SeqCst),
            1,
            "and it must leave on the FIRST refusal, not repaint stale and re-poll",
        );
    }

    /// The other half, and NOT vacuously: when WE are the one tearing down, the socket error
    /// must NOT quit — even though the poll thread was already RUNNING (past its `while !stop`
    /// entry guard) when `stop` flipped. This is the real `Drop` race: the thread is parked in
    /// `conn.call`, and `WireHost::drop` sets `stop` THEN shuts the socket. The old version of
    /// this test preset `stop = true`, so the loop never entered and the error-arm `!stop`
    /// guard it claimed to protect was never reached — it passed even with `request_quit`
    /// hoisted out of that guard. This drives the thread INTO the parked read first.
    ///
    /// REVERT-PROOF: hoist `quit.request_quit()` out of the error arm's `if !stop` and this
    /// reads 1 — proving it exercises that exact guard, which the preset-`true` version did not.
    #[test]
    fn our_own_teardown_does_not_ask_the_shell_to_quit() {
        let path = sock_path("teardown");
        let listener = UnixListener::bind(&path).expect("bind");
        let _guard = SockGuard(path.clone());
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect");
        let (server, _) = listener.accept().expect("accept");

        let quit = Arc::new(RecordingQuit::default());
        let stop = Arc::new(AtomicBool::new(false)); // FALSE, so the loop actually enters
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(LayoutSnapshot::default())),
            Box::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");

        // Synchronize: read the `scene/waitFor` request the thread wrote. Once it arrives, the
        // thread is past `while !stop` and BLOCKED in `conn.call` reading the (never-coming)
        // reply — exactly where `WireHost::drop` finds it.
        let mut reader = BufReader::new(server.try_clone().expect("clone server"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("read the request");
        assert!(line.contains("scene/waitFor"), "the parked request: {line}");

        // Now WE tear down, in Drop's order: set stop, THEN close the socket. The blocked read
        // returns EOF, the error arm sees stop=true, and must NOT quit.
        stop.store(true, Ordering::Relaxed);
        drop(reader);
        drop(server);
        poll.join().expect("the poll thread exited");

        assert_eq!(
            quit.0.load(Ordering::SeqCst),
            0,
            "a socket error during our own teardown is not a host death; it must not quit",
        );
    }

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
