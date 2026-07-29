//! `WireHost` — a display client as a pure wire client of a `sprag-term` host process
//! (topology B).
//!
//! [`WireHost`] implements the same [`HostClient`] protocol the in-process
//! [`Host`](sprag_host::Host) does — addressing panes by their host [`PaneId`] — but
//! reaches them over an RPC socket to a `sprag-term` host PROCESS instead of an
//! in-process `Workspace`. A frontend wraps it in its own slot↔`PaneId` adapter (the GUI's
//! `SlotView`) that maps that frontend's display slots onto host ids, so both this wire client
//! and the in-process `Host` stay pure identity clients and the "slot" concept lives in ONE
//! per-frontend place — the R109 seam across the process boundary.
//!
//! Written for the GUI and now shared: it says "a client" rather than "the GUI" wherever the
//! statement is about the wire, and names the GUI only where the fact really is the GUI's.
//!
//! ## What runs where
//!
//! * A `sprag-term` DAEMON owns the `Workspace` + PTYs. A client CONNECT-OR-SPAWNS it on the
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
//! naming none ALLOCATES a fresh session and spawns this client's panes into it, so by DEFAULT two
//! GUIs against one host start on different sessions. (A running client can later
//! [`switch_session`](WireHost::switch_session) to any session — the session sidebar — so two
//! clients CAN come to mirror one session; the host serves that fine, tmux-style multi-attach.) A
//! spawned daemon boots with no STRAY pane
//! (`--daemon`) — every pane lives in some client's session; it may RESTORE prior sessions from
//! its durability snapshot after a reboot, but those are named sessions a client attaches to, not
//! a default-session pane that would leak into this client.
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

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pinion_core::{GridBuffer, QuitSink};
use serde_json::{Value, json};
use sprag_grid::ProjectionToken;
use sprag_host::wire::{
    BREAK_PANE_ACTION, CLIPBOARD_ANSWER_ACTION, CLIPBOARD_WRITE_SLOT, CLOSE_ACTION,
    DROP_FILE_ACTION, FOCUS_ACTION, FULL_TEXT_SLOT, GLOBAL_COMMANDS_SLOT, JOIN_PANE_ACTION,
    KEY_ACTION, KILL_SESSION_ACTION, KILL_WINDOW_ACTION, LAYOUT_SLOT, MOUSE_ACTION,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT, PASTE_ACTION, PROMPT_MARKS_SLOT,
    RESIZE_ACTION, SELECT_WINDOW_ACTION, SESSIONS_SLOT, SET_FLOATING_ACTION, SET_LAYOUT_ACTION,
    SPAWN_ACTION, SPLIT_ACTION, TEXT_ACTION, WINDOWS_SLOT, cells_slot_at, find_slot_for,
    project_slot_for, regex_slot_for,
};
use sprag_host::{
    CellFrame, HostClient, PaneClipboardQuery, PaneClipboardWrite, PaneFind, PaneNotification,
    PaneScrollFacts, Project, UserConfig, mux_action_path, pane_input_path,
};
use sprag_input::{Modifiers, MouseButton, MouseEventKind, MouseInput};
use sprag_rpc::{
    CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM, HostConn, new_gui_client_id,
    runtime_path,
};
use sprag_terminal::{
    LayoutSnapshot, LayoutWire, PaneExit, PaneId, SessionInfo, SplitDir, WindowInfo,
};
use sprag_vt::{ClipboardTarget, ClipboardTargets, Image, MouseProtocol};

/// How long to wait for a just-spawned daemon's socket to accept — covers its bind race.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a UI-thread request may wait for the daemon's reply before this client gives up on
/// the connection ([`HostConn::set_read_deadline`]).
///
/// These calls run on the reducer, so the window paints nothing while one is outstanding, and the
/// daemon is a local process answering from memory — a reply that has not arrived in seconds is
/// not slow, it is not coming. Without a bound a daemon that accepts and then stops answering
/// (wedged, stopped, mid-crash) freezes the GUI for as long as it stays that way, with no way out
/// but killing the window. Generous enough that a loaded machine mid-`switch-client` — the
/// heaviest of these, a connect plus a read per pane — never trips it.
///
/// Emphatically NOT applied to the long-poll connection: `scene/waitFor` parks until a pane
/// produces output, so waiting forever is its contract rather than a hazard.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);

/// Env override: the host socket path to connect-or-spawn on, instead of the well-known
/// `sprag-host.sock` (a test's private socket, or an operator-run host).
const HOST_SOCK_ENV: &str = "SPRAG_GUI_HOST_SOCK";
/// Env override: the `sprag-term` binary to spawn (else the sibling of the GUI exe,
/// else `sprag-term` on `PATH`).
const HOST_BIN_ENV: &str = "SPRAG_GUI_HOST_BIN";
/// Env: the SESSION to attach to (adopt its live panes) — the reattach gesture. Absent, the
/// client allocates a fresh session and spawns its own panes into it, so by DEFAULT each launch
/// starts on its own session (the owner's several-windows workflow) — though a running client can
/// [`switch_session`](WireHost::switch_session) to any other from the sidebar. `sprag attach` sets
/// this env; it is the established GUI-config channel (`SPRAG_GUI_PANES`/`_HOST_SOCK`/…).
const SESSION_ENV: &str = "SPRAG_GUI_SESSION";

/// Env: how this client reacts when its OWN attached session is DESTROYED — the tmux
/// `detach-on-destroy` session option. `on` (or unset / an unrecognized value) DETACHES this client
/// (the shipped default, and tmux's own); `off` / `next` / `previous` SWITCH it to a neighbouring
/// session instead (tmux's switch-to-next), detaching only when there is no other session to move
/// to; `no-detached` switches only to a session NO OTHER client is viewing, detaching rather than
/// pile a second client onto one another client already holds. Read
/// ONCE at boot (the codebase's config convention, alongside [`SESSION_ENV`]) into a
/// [`DetachOnDestroy`] held on the [`WireHost`]; a future runtime `set-option` would write the SAME
/// enum, so the policy — not this env — is the durable seam.
const DETACH_ON_DESTROY_ENV: &str = "SPRAG_DETACH_ON_DESTROY";

/// One pane the wire client mirrors, in HOST order (no holes — "slots" and their holes
/// are the GUI `SlotView`'s concern, not this data client's). Holds the pane's host
/// identity ([`PaneId`] + command label), its live (offset 0) [`CellFrame`] (refreshed
/// by the poll thread on each host change), and the GUI-tracked grid size.
struct WirePane {
    id: PaneId,
    /// The projection token that was current when [`Self::frame`] was taken — the value the next
    /// wake compares against to decide whether re-fetching would tell it anything new. `None`
    /// keeps this pane on the unconditional-fetch path.
    projection: Option<ProjectionToken>,
    label: String,
    /// The child's live `OSC 0`/`OSC 2` window title, `None` until it sets one.
    /// Host-authoritative like [`Self::label`] (re-read on every poll re-query, since a
    /// shell rewrites it each prompt). A DISPLAY name only — never identity.
    title: Option<String>,
    /// The pane's most recent attention notification (`OSC 9` / `OSC 777;notify` / `OSC 99`),
    /// or `None`. Host-authoritative + dynamic like [`Self::title`] — re-adopted every wake, so
    /// its `seq` grows as the child raises more (the GUI's attention badge reads it).
    notification: Option<PaneNotification>,
    /// The pane's tmux monitor-bell count (`\a`), `0` if none. Host-authoritative + dynamic like
    /// [`Self::notification`] — re-adopted each wake, kept SEPARATE from it (a bell carries no
    /// text) so the attention marker can combine the two.
    bell_seq: u64,
    /// Whether the pane's child has EXITED, `false` while it runs. Host-authoritative and re-adopted
    /// each wake like the rest, but ONE-WAY: a pane never comes back to life, so the only staleness
    /// this can hold is a just-exited pane still reading live for one poll interval.
    dead: bool,
    /// HOW the child ended, `None` until the host has reaped it — and possibly never, for a child
    /// whose pty outlives it. Host-authoritative and one-way like [`Self::dead`], but it REFINES
    /// rather than duplicates: `dead` alone cannot say whether the command worked.
    child_exit: Option<PaneExit>,
    /// The pane's OSC 52 clipboard WRITE count, `0` if none. Host-authoritative + dynamic — the
    /// CHEAP detection counter (no payload); a frontend's clipboard policy fetches the actual
    /// write via [`WireHost::pane_clipboard_write`] only when this grows past the ack.
    clipboard_write_seq: u64,
    /// The pane's pending OSC 52 clipboard READ query (selection + seq), `None` if none. Host-
    /// authoritative + dynamic — a frontend answers it (subject to its policy) when the seq grows.
    clipboard_query: Option<PaneClipboardQuery>,
    /// The pane's inline images (Kitty graphics, R1404), empty if none. Host-authoritative +
    /// dynamic — re-adopted each wake, composited over the grid by whichever frontend can show
    /// one (the GUI does; a terminal client does not).
    images: Vec<Image>,
    /// The pane's live mouse-tracking protocol level (None / Click / ButtonEvent / AnyEvent — the
    /// wire `mouse` key carries the level token, present only while tracking). Host-authoritative +
    /// dynamic — re-adopted each wake; the pane pointer oracle reads it to decide whether to CAPTURE
    /// a press AND, from the level, whether to forward drag / bare motion.
    mouse_protocol: MouseProtocol,
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

/// The host arrangement this client PROJECTS, tagged with the WINDOW it belongs to.
///
/// Bundling the window NAME with the snapshot is what makes a cross-window SWITCH
/// distinguishable from a within-window UPDATE. The per-window `layout_revision` is monotonic
/// only WITHIN a window, so switching to a window whose revision happens to be LOWER must RESET
/// the mirror — NOT be dropped by [`store_layout`]'s staleness guard (which exists to stop a
/// racing poll read from clobbering a UI write on the SAME window — the R154 lost-layout scar).
/// Without this the client would keep projecting the OLD window's tree over the NEW window's
/// panes (whose ids the stale tree does not name), collapsing the dock to nothing. The window
/// tag also lets a layout WRITE name the window its gesture was drawn on (bound d — see
/// [`WireHost::set_layout`]).
#[derive(Default)]
struct Mirrored {
    /// The window the `layout` belongs to (its `windows`-slot name).
    window: String,
    layout: LayoutSnapshot,
}

/// The host's current arrangement + which window it is, mirrored. Shared between the UI thread
/// (which projects it, replaces it with its own writes' answers, and RESETS it on a window
/// switch) and the poll thread (which re-reads it whenever the host says the scene moved), under
/// one lock.
///
/// Mirrored rather than fetched on demand for the same reason the pane frames are: the paint
/// path must never make a socket call. A client reads this every frame to notice its projection
/// is stale, so a round trip there would put the wire on the UI thread's hot path.
type LayoutMirror = Arc<Mutex<Mirrored>>;

/// Lock the mirrored arrangement, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_layout(layout: &Mutex<Mirrored>) -> MutexGuard<'_, Mirrored> {
    layout.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Store `snapshot` (the arrangement of the window named `current`) in the mirror — the ONE place
/// it is written, shared by the poll thread, the UI thread's writes, and the switch refresh.
///
/// A store for a DIFFERENT window than the mirror holds is a SWITCH: reset unconditionally,
/// because the incoming per-window revision does not compare with the outgoing window's. A store
/// for the SAME window is revision-GUARDED: two threads race here (the poll reads the layout off
/// the lock and stores it after, so a UI-thread write can land first), and storing an older
/// same-window revision would move the mirror BACKWARD — the client would re-project a pre-gesture
/// tree and visibly snap the user's just-settled divider back until a later bump healed it (R154).
fn store_layout(layout: &Mutex<Mirrored>, current: &str, snapshot: LayoutSnapshot) {
    let mut mirror = lock_layout(layout);
    if mirror.window != current {
        mirror.window = current.to_owned();
        mirror.layout = snapshot;
    } else if snapshot.revision >= mirror.layout.revision {
        mirror.layout = snapshot;
    } else {
        tracing::trace!(
            target: "sprag_gui::wire",
            stale = snapshot.revision,
            held = mirror.layout.revision,
            "dropped a same-window layout read overtaken by a newer one",
        );
    }
}

/// The name of the current window in `list`, or `None` if none is marked current (an empty or
/// malformed list). The SSOT for "which window this client is on", read by the boot, the poll,
/// and the switch refresh to tag the layout mirror.
fn current_window_name(list: &[WindowInfo]) -> Option<String> {
    list.iter()
        .find(|window| window.current)
        .map(|window| window.name.clone())
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

/// The scoped session's window LIST, mirrored — what a tabbed client draws. Shared between the
/// UI thread (which reads it, and refreshes it right after its own window write for immediate
/// feedback) and the poll thread (which re-reads it whenever the scene moves), under one lock.
/// Mirrored, not fetched on demand, for the same reason the layout is: the paint path must make
/// no socket call.
type WindowsMirror = Arc<Mutex<Vec<WindowInfo>>>;

/// Lock the mirrored window list, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_windows(windows: &Mutex<Vec<WindowInfo>>) -> MutexGuard<'_, Vec<WindowInfo>> {
    windows.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replace the mirrored window list — the ONE place it is written, shared by the poll thread and
/// a window write's own follow-up read. Unconditional (unlike [`store_layout`]): the list carries
/// no revision, so there is nothing to compare, and any brief backward move heals on the next
/// wake — the tab set is not a live gesture the way a dragged divider is.
fn store_windows(windows: &Mutex<Vec<WindowInfo>>, list: Vec<WindowInfo>) {
    *lock_windows(windows) = list;
}

/// Read the scoped session's window list off the wire — the ONE place the `windows` slot is
/// queried, shared by the boot read and the poll thread's refresh.
fn query_windows(conn: &mut HostConn) -> io::Result<Vec<WindowInfo>> {
    let value = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(WINDOWS_SLOT) }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Every session on the host, mirrored — what a session SWITCHER draws (a vertical rail of every
/// session, the current one highlighted). Registry-WIDE, not scoped: it is the `sessions` slot,
/// whose subject is the SET of sessions, so it is read the same over any client's scoped conn.
/// Shared between the UI thread (which reads it to paint the sidebar) and the poll thread (which
/// re-reads it whenever the scene moves — a new / killed session bumps the revision), under one
/// lock. Mirrored, not fetched on demand, for the same reason the windows list is: the paint path
/// must make no socket call.
type SessionsMirror = Arc<Mutex<Vec<SessionInfo>>>;

/// Lock the mirrored session list, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_sessions(sessions: &Mutex<Vec<SessionInfo>>) -> MutexGuard<'_, Vec<SessionInfo>> {
    sessions.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replace the mirrored session list — the ONE place it is written, shared by the poll thread and
/// a switch's own re-boot. Unconditional (like [`store_windows`]): the list carries no revision,
/// so any brief backward move heals on the next wake.
fn store_sessions(sessions: &Mutex<Vec<SessionInfo>>, list: Vec<SessionInfo>) {
    *lock_sessions(sessions) = list;
}

/// Read every session off the wire (the registry-wide `sessions` slot) — the ONE place it is
/// queried, shared by the boot read, the poll thread's refresh, and a switch's re-boot.
fn query_sessions(conn: &mut HostConn) -> io::Result<Vec<SessionInfo>> {
    let value = conn.call(
        "scene/query",
        json!({ "path": mux_action_path(SESSIONS_SLOT) }),
    )?;
    serde_json::from_value(value).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Announce `conn`'s CLIENT id to the daemon (`client/hello`, R-PR67) — the group key a client's
/// several connections share, so the daemon counts one GUI as one attached client, not one per
/// connection. BEST-EFFORT: attachment drives only the sidebar viewer badge, and a daemon that does
/// not know the method (older than R-PR67) simply leaves the badge empty, so a failure is logged and
/// swallowed, never fatal to displaying panes.
fn send_hello(conn: &mut HostConn, client_id: &str) {
    if let Err(error) = conn.call(CLIENT_HELLO_METHOD, json!({ CLIENT_PARAM: client_id })) {
        tracing::debug!(target: "sprag_gui::wire", %error, "client/hello failed; viewer badge disabled");
    }
}

/// Declare (or switch — tmux `switch-client`) this client's ATTACHED session to the one `conn` is
/// scoped to (`client/attach`, R-PR67). The session rides the connection's scope, so no arg is
/// needed; the daemon attributes the attach to the connection's client via its prior [`send_hello`].
/// BEST-EFFORT, like [`send_hello`].
fn send_attach(conn: &mut HostConn) {
    if let Err(error) = conn.call(CLIENT_ATTACH_METHOD, json!({})) {
        tracing::debug!(target: "sprag_gui::wire", %error, "client/attach failed; viewer badge disabled");
    }
}

/// What this client does when its attached session is DESTROYED — by its own sidebar kill OR out of
/// band (another client / the `sprag` CLI killing it, its last pane exiting) — the tmux
/// `detach-on-destroy` policy. The ONE decision "my session is gone, now what": the default
/// [`Detach`](Self::Detach) reproduces tmux's default (and the pre-policy shipped behavior)
/// byte-for-byte, so the switch policy is purely additive. Held on the [`WireHost`] and consulted at
/// BOTH destroy triggers, so the switch-vs-detach DECISION is the same however its session died
/// (never switch on one trigger and detach on the other). Only the LATENCY differs: the sidebar kill
/// switches inline, while an out-of-band destroy applies its switch on the next paint (the poll flags
/// it and the UI-thread reconcile performs it), a beat later.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum DetachOnDestroy {
    /// tmux `on` (default): DETACH — the client leaves (asks the shell to quit).
    #[default]
    Detach,
    /// tmux `off`: switch to the MOST-RECENTLY-USED other session (this client's visit history), the
    /// most useful "go back to where I just was" target. Falls back to the [`Next`](Self::Next) list
    /// neighbour when no visited session survives, so it still SWITCHES whenever any other session
    /// exists; detaches only when this is the last session.
    Off,
    /// tmux `no-detached`: switch to the most-recently-used OTHER session that NO OTHER client is
    /// attached to, so two clients never pile onto one session; DETACH when every other session is
    /// already being viewed by another client (or this is the last session). The [`Off`](Self::Off)
    /// cousin that RESPECTS other clients — where `off` switches to any surviving session,
    /// `no-detached` refuses an occupied one and leaves instead. Needs the per-session viewer count
    /// ([`SessionInfo::attached`], R-PR67); the switching client is never on the candidate sessions
    /// (one client attaches one session), so a candidate's non-zero count is always ANOTHER client —
    /// self-exclusion is automatic, no `client_id` needed at this seam.
    NoDetached,
    /// tmux `next`: switch to the NEXT session in list order (wrapping), detaching only if this is
    /// the last session.
    Next,
    /// tmux `previous`: switch to the PREVIOUS session in list order (wrapping), detaching only if
    /// this is the last session.
    Previous,
}

impl DetachOnDestroy {
    /// Whether this policy DEFERS the destroy decision to [`destroy_successor`] rather than detaching
    /// immediately on the poll thread — every variant EXCEPT [`Detach`](Self::Detach). For the switch
    /// policies it resolves to a switch; for [`NoDetached`](Self::NoDetached) it resolves to a switch
    /// OR, when every other session is occupied, a detach — so the poll thread must defer to the UI
    /// reconcile (which runs [`destroy_successor`] against the freshest mirror) either way, never
    /// pre-empt it with its own detach. The one predicate both destroy triggers gate on, so a policy
    /// added later is deferred everywhere at once (the poll thread's out-of-band arm and the sidebar
    /// kill) and cannot be forgotten in one place — the bug a variant list invites.
    fn is_switch(self) -> bool {
        !matches!(self, DetachOnDestroy::Detach)
    }
}

/// Parse the [`DETACH_ON_DESTROY_ENV`] value into a [`DetachOnDestroy`] — a pure decision over its
/// input (the env read stays in [`WireHost::spawn_or_attach`], matching `resolve_session` /
/// `parse_allowlist`). Whitespace- and case-insensitive; an ABSENT or UNRECOGNIZED value is
/// [`Detach`](DetachOnDestroy::Detach), the safe tmux default — a typo detaches (what an unset env
/// does) rather than silently switching a client somewhere it never asked to go.
fn parse_detach_on_destroy(raw: Option<&str>) -> DetachOnDestroy {
    match raw
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("off") => DetachOnDestroy::Off,
        Some("no-detached") => DetachOnDestroy::NoDetached,
        Some("next") => DetachOnDestroy::Next,
        Some("previous") => DetachOnDestroy::Previous,
        _ => DetachOnDestroy::Detach,
    }
}

/// The session this client should SWITCH to when its own attached session `killed` is destroyed
/// under `policy`, or `None` to DETACH instead — the tmux `detach-on-destroy off`/`next`/`previous`
/// target. `None` whenever the policy is [`Detach`](DetachOnDestroy::Detach), or `killed` is the
/// only session (nothing to move to), or `killed` is not in `list` (already gone, so no neighbour to
/// anchor on — a detach is the honest answer).
///
/// `off` walks `mru` — this client's visit history, most-recent-first ([`push_mru`]) — for the
/// most-recent OTHER session still present in `list`; that is tmux's "switch to the last session"
/// intent. Because sprag's history is CLIENT-LOCAL (not tmux's global last-activity), a client that
/// only ever saw `killed` has no MRU other, so `off` FALLS BACK to the `next` list neighbour rather
/// than detach — "off" means "don't leave if there is somewhere to go", so it detaches only when
/// `killed` is truly the last session. `next`/`previous` ignore `mru`.
///
/// The neighbour (for `next`/`previous`, and `off`'s fallback) is by LIST ORDER — the order the
/// sidebar draws it (session creation order, a stable `Vec`), so `next` moves to the row visually
/// below and `previous` above, WRAPPING at the ends. tmux orders by session NAME (it has no visible
/// list); sprag has a sidebar, so its visible order is the more intuitive, honest analog. `killed` is
/// present in `list` when this runs (the successor is picked BEFORE the kill removes it), and with
/// `len >= 2` the ±1 wrap can never land back on it — so the returned name is always a DIFFERENT,
/// live session.
/// The MOST-RECENT session in `mru` (this client's visit history, most-recent-first) that is NOT
/// `current` and is still present in `list`, or `None` — the tmux "last session" target
/// (`switch-client -l`) AND the preference a [`DetachOnDestroy::Off`] switch walks. Skips a `mru`
/// entry that has since died (not in `list`); `None` when the client has visited no other surviving
/// session (a fresh client that never switched, or all its prior sessions are gone).
fn mru_live_other(mru: &[String], list: &[SessionInfo], current: &str) -> Option<String> {
    mru.iter()
        .find(|name| name.as_str() != current && list.iter().any(|session| session.name == **name))
        .map(|name| name.to_string())
}

/// The session tmux `no-detached` switches to when `killed` is destroyed: the most-recent OTHER
/// session that NO OTHER client is attached to ([`SessionInfo::attached`] `== 0`), or the first such
/// session in list order when this client's `mru` history offers none, or `None` to DETACH when
/// every other session is occupied by another client (or `killed` is the last session). The
/// `attached == 0` filter is the whole difference from [`DetachOnDestroy::Off`]: `off` switches to
/// any surviving session, `no-detached` refuses one another client is already viewing and leaves
/// instead — so a destroyed shared workspace never dumps its client onto a colleague's session. The
/// switching client is attached to `killed`, never to a candidate (one client, one session), so a
/// candidate's non-zero count is always ANOTHER client — the count needs no self-exclusion here.
///
/// The counts are as fresh as this client's last sessions poll (the [`SessionsMirror`] the poll
/// refreshes on each attach/detach revision bump); a client that joined an otherwise-free candidate
/// in the beat between that poll and this destroy is not yet reflected, so two clients could
/// MOMENTARILY share — a bounded staleness the daemon's next poll corrects, not a lasting split.
fn no_detached_successor(list: &[SessionInfo], killed: &str, mru: &[String]) -> Option<String> {
    let is_free_other = |name: &str| -> bool {
        name != killed
            && list
                .iter()
                .any(|session| session.name == name && session.attached == 0)
    };
    // MRU-preferred (tmux picks the newest DETACHED session; sprag's client-local analog is visit
    // recency), skipping any visited session another client has since joined.
    if let Some(name) = mru.iter().find(|visited| is_free_other(visited.as_str())) {
        return Some(name.clone());
    }
    // No visited session qualifies — the first UNATTACHED other session in list (creation) order.
    list.iter()
        .find(|session| session.name != killed && session.attached == 0)
        .map(|session| session.name.clone())
}

fn destroy_successor(
    policy: DetachOnDestroy,
    list: &[SessionInfo],
    killed: &str,
    mru: &[String],
) -> Option<String> {
    let step: isize = match policy {
        DetachOnDestroy::Detach => return None,
        DetachOnDestroy::Off => {
            // MRU-preferred: the most-recent OTHER visited session still live.
            if let Some(last) = mru_live_other(mru, list, killed) {
                return Some(last);
            }
            // No visited session survives — fall back to the `next` list neighbour, so `off` still
            // SWITCHES whenever another session exists (detaching only when `killed` is the last).
            1
        }
        // `no-detached` never falls through to a blind list neighbour (which could be occupied) — it
        // picks only an UNATTACHED session or detaches, fully resolved by its own helper.
        DetachOnDestroy::NoDetached => return no_detached_successor(list, killed, mru),
        DetachOnDestroy::Next => 1,
        DetachOnDestroy::Previous => -1,
    };
    if list.len() < 2 {
        return None; // only `killed` (or empty): nothing to switch to.
    }
    let here = list.iter().position(|session| session.name == killed)? as isize;
    let len = list.len() as isize;
    let neighbour = (here + step).rem_euclid(len) as usize;
    Some(list[neighbour].name.clone())
}

/// Record `name` as the MOST-RECENTLY-used session in `stack` (most-recent-first, deduplicated):
/// drop any existing entry, then push to the front. The client-local visit history a
/// [`DetachOnDestroy::Off`] switch walks ([`destroy_successor`]) to pick the most-recent OTHER live
/// session — and the natural home for a future "switch to the last session" hotkey (tmux
/// `switch-client -l`). Bounded by the session count (the dedup keeps at most one entry per session).
fn push_mru(stack: &mut Vec<String>, name: &str) {
    stack.retain(|visited| visited != name);
    stack.insert(0, name.to_owned());
}

/// The GUI's wire client of a `sprag-term` host. See the module docs.
pub struct WireHost {
    /// The pane data cache ([`Cache`]) in host order: identity + live frame + tracked
    /// dims per pane. The UI thread reads it under a brief lock; the poll thread refreshes
    /// each pane's frame under the same lock. Addressed by [`PaneId`] — the GUI's
    /// `SlotView` maps display slots onto these ids.
    cache: Cache,
    /// The host's arrangement, mirrored ([`LayoutMirror`]) — what this client PROJECTS.
    /// The poll thread re-reads it on every scene change; a write on the UI thread replaces
    /// it with the host's canonical answer.
    layout: LayoutMirror,
    /// The scoped session's window list, mirrored ([`WindowsMirror`]) — what a tabbed client
    /// draws. The poll thread re-reads it on every scene change; a window write on the UI thread
    /// refreshes it right after, so the tab strip updates without waiting a poll wake.
    windows: WindowsMirror,
    /// EVERY session on the host, mirrored ([`SessionsMirror`]) — what a session switcher draws.
    /// Registry-wide (not this client's own scope); the poll thread re-reads it on every scene
    /// change, and a session switch / create re-boots it for immediate feedback.
    sessions: SessionsMirror,
    /// The UI thread's request connection (reads / input / resize). `RefCell`, not
    /// `Mutex`: `WireHost` is UI-thread-only (see the module docs), and the poll thread
    /// owns a SEPARATE connection. A session SWITCH re-scopes this connection in place.
    conn: RefCell<HostConn>,
    /// This GUI's opaque CLIENT id (R-PR67), shared by its request + poll connections so the daemon
    /// counts one window as ONE attached client, not one per connection. Announced on every
    /// connection ([`send_hello`]) and used to attach ([`send_attach`]) on boot and each switch;
    /// minted once per process ([`new_gui_client_id`]). A lifecycle token, not identity.
    client_id: String,
    /// The session this client is CURRENTLY attached to — a client-local fact (the wire's
    /// per-session `attached` COUNT is a different thing: how many clients view each session, not
    /// which one THIS client is on). Read to highlight the switcher's current row and to no-op a switch to
    /// the same session; re-pointed by [`switch_session`](WireHost::switch_session). `RefCell`
    /// because `WireHost` is UI-thread-only.
    session: RefCell<String>,
    /// The host socket this client connect-or-spawned on — kept so a session switch can open a
    /// FRESH poll connection to the same daemon (the request conn is re-scoped in place; the poll
    /// thread is torn down and a new one spawned on a new connection).
    sock: PathBuf,
    /// The pane grid `(cols, rows)` this client booted at — the birth size a sidebar "+" gives a
    /// new session (it reflows to this window on first paint, like every boot pane).
    boot_dims: (u16, u16),
    /// How this client reacts when its OWN attached session is destroyed — the tmux
    /// `detach-on-destroy` policy ([`DetachOnDestroy`]), read once at boot from
    /// [`DETACH_ON_DESTROY_ENV`]. `Copy`, so a `&self` method reads it with no borrow. Consulted at
    /// BOTH destroy triggers: [`kill_session`](HostClient::kill_session) (this client's own sidebar
    /// kill) and [`reconcile_lost_session`](HostClient::reconcile_lost_session) (an out-of-band kill).
    detach_on_destroy: DetachOnDestroy,
    /// Set by the poll thread when this client's attached session is destroyed OUT OF BAND under a
    /// SWITCH policy (another client / the `sprag` CLI killed it): the poll cannot switch (a UI-thread
    /// op), so it flags this + repaints, and the UI-thread
    /// [`reconcile_lost_session`](HostClient::reconcile_lost_session) does the switch. Shared
    /// `Arc<AtomicBool>` (the poll thread is off-thread); swap-cleared by the reconcile and by any
    /// successful [`attach_in_place`](WireHost::attach_in_place), so a manual switch that pre-empts
    /// the reconcile can't leave
    /// a stale flag to fire a spurious second switch. Never set under the `Detach` policy — that path
    /// stays the poll thread's own immediate detach, unchanged.
    lost_session: Arc<AtomicBool>,
    /// This client's session VISIT history, most-recent-first + deduplicated ([`push_mru`]) — the MRU
    /// stack a [`DetachOnDestroy::Off`] switch walks ([`destroy_successor`]) for the most-recent OTHER
    /// live session. Seeded with the boot session and pushed on every
    /// [`attach_in_place`](WireHost::attach_in_place). `RefCell` because `WireHost` is UI-thread-only,
    /// like [`session`](Self::session).
    mru: RefCell<Vec<String>>,
    /// The repaint sink, kept as a reusable `Arc` so a session switch can hand a FRESH poll thread
    /// the same `on_change` (a `Box<dyn Fn>` could only be moved into the first thread). Send+Sync
    /// because the underlying [`RepaintSink`](pinion_core::RepaintSink) is, so it is shared across
    /// each poll incarnation (only one runs at a time — the old is joined before the new spawns).
    on_change: Arc<dyn Fn() + Send + Sync>,
    /// The shell's quit edge, kept for the same reason as [`Self::on_change`]: each poll thread
    /// (boot's and every switch's) is handed a clone so a dead daemon ends the client.
    quit: Arc<dyn QuitSink>,
    /// The running poll thread + its cancellation handles, as ONE swappable unit: a session switch
    /// stops-and-joins the current thread, then installs a fresh one scoped to the new session.
    /// `RefCell` (UI-thread-only) so a `&self` switch can replace it; `Option` is the between-state
    /// (`None` only transiently mid-swap and after Drop).
    poll: RefCell<Option<PollThread>>,
}

/// The background change-notification -> repaint poll thread and the two handles that stop it —
/// swapped as a unit when this client switches sessions (the new session needs a poll scoped to
/// IT), and torn down on Drop. Bundling the three means a switch and the Drop share one stop path
/// ([`PollThread::stop`]), so neither can forget the shutdown-then-join order.
struct PollThread {
    /// Set to stop the poll loop (its `while !stop` guard + the post-`waitFor` re-check).
    stop: Arc<AtomicBool>,
    /// A shutdown handle onto the poll connection: `shutdown(Both)` cancels the thread's parked
    /// `scene/waitFor` so the join is deterministic. The host is a daemon we never kill, so this is
    /// the ONLY thing that unblocks the parked read on teardown.
    shutdown: UnixStream,
    /// The thread handle, joined by [`stop`](Self::stop) (taken once).
    handle: Option<JoinHandle<()>>,
}

impl PollThread {
    /// Stop the poll thread and join it: flag `stop`, cancel its parked read, then join. Ordered
    /// so the parked `waitFor` is unblocked BEFORE the join, never a deadlock. Idempotent — the
    /// handle is taken once, so a second call (Drop after a switch already stopped it) is a no-op.
    ///
    /// The `stop` store is `Release` and the poll thread's loads are `Acquire` (not `Relaxed`)
    /// because this runs from a LIVE `switch_session`, not only Drop: the shutdown wakes the poll
    /// thread's `waitFor` with an error, and its error arm calls `request_detach`, which asks the
    /// shell to QUIT unless it observes `stop == true`. A stale `false` there would exit the whole
    /// GUI mid-switch. (In Drop a stale read was harmless — already quitting — which is why the
    /// pre-switch code got away with `Relaxed`.) `Release`/`Acquire` documents the ordering the
    /// store-before-`shutdown` / read-error-before-load chain already relies on.
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.shutdown.shutdown(Shutdown::Both);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
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
    /// dies. Both `on_change` and `quit` are kept as shared `Arc` handles so each poll
    /// incarnation a session SWITCH spawns is handed the same pair.
    ///
    /// # Errors
    ///
    /// Any failure to spawn the daemon, connect to its socket within `CONNECT_TIMEOUT`, or
    /// resolve the session / boot the panes over RPC. The daemon is NOT reaped on failure —
    /// it is a detached process this GUI does not own.
    pub fn spawn_or_attach(
        argv: Option<Vec<String>>,
        cols: u16,
        rows: u16,
        n_panes: usize,
        on_change: Arc<dyn Fn() + Send + Sync>,
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
        // Bound every UI-thread reply from here on ([`REQUEST_DEADLINE`]). This is the ONE request
        // connection for the client's whole life — `attach_in_place` re-scopes it rather than
        // replacing it — so setting the deadline once here covers every later request, including
        // the switch-client path that named this hazard. A socket that refuses the option is not a
        // reason to abandon the boot: the client is then exactly as exposed as it was before, which
        // is worse than the bound but no worse than shipping without it.
        if let Err(error) = conn.set_read_deadline(Some(REQUEST_DEADLINE)) {
            tracing::warn!(
                target: "sprag_gui::wire",
                %error,
                "could not bound the request connection's reads; a wedged daemon can stall the UI",
            );
        }

        // Resolve WHICH session this client acts on before booting panes, and scope every
        // request to it (both this connection and the poll one below), so a request can never
        // silently land in another session. Naming one ATTACHES (adopt its panes); naming none
        // ALLOCATES a fresh one (spawn our own panes) — the "each launch is its own session"
        // model. `boot_panes` branches on `created`, replacing the old "did we spawn the host"
        // key with "did we create the session".
        // Read the requested-session env HERE (not inside `resolve_session`, which stays a pure
        // decision over its inputs): [`SESSION_ENV`] names a session to ATTACH to, absent creates.
        let requested = std::env::var_os(SESSION_ENV)
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string_lossy().into_owned());
        // The destroy policy is a boot-time config read here (kept out of the pure helpers, like the
        // session env above), held on the client so a session kill can consult it with no env re-read.
        let detach_on_destroy =
            parse_detach_on_destroy(std::env::var(DETACH_ON_DESTROY_ENV).ok().as_deref());
        let (session, created) =
            resolve_session(&mut conn, requested.as_deref(), argv.as_deref(), cols, rows)?;
        conn.scope_to(session.clone());
        // R-PR67: this GUI is one attached CLIENT across its two connections. Announce the shared id
        // on the request conn and attach it to its session, so the daemon counts this window as a
        // viewer (the sidebar badge). Done before the `since0` baseline below so the attach's own
        // scene bump is folded into the baseline, not a spurious first poll wake.
        let client_id = new_gui_client_id();
        send_hello(&mut conn, &client_id);
        send_attach(&mut conn);
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
        // The window list is booted the same way and for the same reason (a tabbed client draws
        // it and must never fetch it from the paint path) — and FIRST, because it names WHICH
        // window the layout mirror reflects: that tag is what makes a later switch a RESET rather
        // than a dropped-as-stale read (see [`Mirrored`]).
        let window_list = query_windows(&mut conn)?;
        let current = current_window_name(&window_list).unwrap_or_default();
        let layout: LayoutMirror = Arc::new(Mutex::new(Mirrored {
            window: current,
            layout: query_layout(&mut conn)?,
        }));
        let windows: WindowsMirror = Arc::new(Mutex::new(window_list));
        // EVERY session, mirrored for the switcher sidebar — booted like the window list and for the
        // same reason (a switcher draws it and must never fetch it from the paint path).
        let sessions: SessionsMirror = Arc::new(Mutex::new(query_sessions(&mut conn)?));

        // Construct the client with NO poll thread yet, then spawn the initial one through the SAME
        // path a session switch re-spawns through ([`spawn_poll_for`]) — one poll-spawn SSOT for
        // boot and switch, so neither can drift from the other.
        let host = Self {
            cache,
            layout,
            windows,
            sessions,
            conn: RefCell::new(conn),
            client_id: client_id.clone(),
            session: RefCell::new(session.clone()),
            sock: sock.clone(),
            boot_dims: (cols, rows),
            detach_on_destroy,
            lost_session: Arc::new(AtomicBool::new(false)),
            mru: RefCell::new(vec![session.clone()]),
            on_change,
            quit,
            poll: RefCell::new(None),
        };
        // The poll thread's own connection — a parked `scene/waitFor` on it never blocks the
        // request connection above (separate host handler threads). Scoped to the SAME session, so
        // its `waitFor`/`revision`/re-queries watch the client's own session and never another's.
        let mut poll_conn = HostConn::connect(&sock, CONNECT_TIMEOUT)?;
        poll_conn.scope_to(session);
        // The poll connection is a SECOND connection of the SAME client: announce the same id so the
        // daemon groups both under one attached client (not two). Only the request conn attaches.
        send_hello(&mut poll_conn, &client_id);
        host.spawn_poll_for(poll_conn, since0)?;
        Ok(host)
    }

    /// Install a fresh poll thread on `poll_conn` (already scoped to the target session), watching
    /// from scene revision `since0` — the ONE poll-spawn site, shared by boot and every session
    /// switch. It hands the thread the shared mirrors + the kept `on_change`/`quit` and stores its
    /// stop/shutdown/handle as the swappable [`PollThread`]. Any previous `PollThread` MUST already
    /// be stopped (boot has none; a switch stops-and-joins the old one first).
    ///
    /// # Errors
    /// Fails if the poll thread cannot be spawned or its shutdown handle taken.
    fn spawn_poll_for(&self, poll_conn: HostConn, since0: u64) -> io::Result<()> {
        let shutdown = poll_conn.shutdown_handle()?;
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn_poll(
            poll_conn,
            Arc::clone(&self.cache),
            Arc::clone(&self.layout),
            Arc::clone(&self.windows),
            Arc::clone(&self.sessions),
            Arc::clone(&self.on_change),
            Arc::clone(&self.quit),
            self.detach_on_destroy,
            Arc::clone(&self.lost_session),
            Arc::clone(&stop),
            since0,
        )?;
        *self.poll.borrow_mut() = Some(PollThread {
            stop,
            shutdown,
            handle: Some(handle),
        });
        Ok(())
    }

    /// Re-point this client at the session named `session` IN PLACE (tmux `switch-client`): re-scope
    /// the request connection, read the target's WHOLE view (windows, layout, panes + frames,
    /// sessions) + a fresh poll baseline over it, then — only if EVERY read succeeds — swap the
    /// mirror CONTENTS (same `Arc`s, so the paint path and the new poll thread keep sharing them),
    /// set the current session, and start a fresh poll scoped to it. The caller MUST have stopped
    /// the previous poll thread first, so nothing refreshes a mirror out from under the swap.
    ///
    /// Ordering vs failure: every READ and the poll-CONNECT happen BEFORE any mirror is written, so
    /// the COMMON failure (the target session is gone, or the daemon won't give a poll connection)
    /// leaves the mirrors untouched. Only the poll-thread START runs after the commit; if THAT rare
    /// step fails the mirrors are already swapped to the target — but the caller
    /// ([`switch_session`](HostClient::switch_session)) recovers by re-attaching to the PREVIOUS
    /// session, which re-reads from the host and never trusts the possibly-swapped mirrors, so the
    /// end state is coherent on either path. (So: reads-then-commit, not a strict all-or-nothing
    /// transaction — the recovery is what makes a post-commit failure safe.)
    ///
    /// # Errors
    /// Any read / connect against the target failing — the session is gone or the daemon will not
    /// give a poll connection — or, rarely, the poll thread failing to spawn after the commit.
    fn attach_in_place(&self, session: &str) -> io::Result<()> {
        // Re-scope the request conn and gather the FULL view + poll baseline over it, all inside one
        // borrow and BEFORE mutating any mirror (so a failed read is a clean abort). Order mirrors
        // boot: revision baseline first (subscribe-then-snapshot), then the frames, then windows /
        // layout / sessions.
        let (fetched, seeds, window_list, current, layout_snapshot, session_list, since0) = {
            let mut conn = self.conn.borrow_mut();
            conn.scope_to(session.to_owned());
            // R-PR67: re-attach this client to the session it just switched to (tmux
            // `switch-client`), moving its viewer count off the old session and onto this one. Before
            // the `since0` baseline, so the attach's scene bump is in the new poll's baseline rather
            // than a spurious self-wake. The old poll conn was already stopped by the caller, so its
            // `on_disconnect` fired; the request conn kept this client present across the switch.
            send_attach(&mut conn);
            let since0 = read_revision(&mut conn)?;
            let seeds = query_panes(&mut conn)?;
            let fetched = fetch_frames(&mut conn, &pane_ids_of(&seeds));
            let window_list = query_windows(&mut conn)?;
            let current = current_window_name(&window_list).unwrap_or_default();
            let layout_snapshot = query_layout(&mut conn)?;
            let session_list = query_sessions(&mut conn)?;
            (
                fetched,
                seeds,
                window_list,
                current,
                layout_snapshot,
                session_list,
                since0,
            )
        };
        // A fresh poll connection scoped to the target (its own host handler thread) — connected
        // BEFORE the commit so a daemon that will not answer aborts the switch rather than leaving
        // the client with mirrors swapped but no live updates.
        let mut poll_conn = HostConn::connect(&self.sock, CONNECT_TIMEOUT)?;
        poll_conn.scope_to(session.to_owned());
        // The fresh poll conn is a new connection of the SAME client (the old one was torn down by
        // the switch): re-announce the shared id so the daemon keeps grouping both under one client.
        send_hello(&mut poll_conn, &self.client_id);

        // COMMIT: swap every mirror's CONTENTS (the `Arc`s themselves stay — shared with the paint
        // path and the poll thread), set the attached session, then start the poll. `merge_panes`
        // with an empty `existing` is the boot case (all newcomers, each taking its fetched frame).
        *lock_cache(&self.cache) = merge_panes(&[], &seeds, &fetched);
        *lock_layout(&self.layout) = Mirrored {
            window: current,
            layout: layout_snapshot,
        };
        store_windows(&self.windows, window_list);
        store_sessions(&self.sessions, session_list);
        *self.session.borrow_mut() = session.to_owned();
        // Record the just-attached session as most-recently-used, for a `detach-on-destroy off`
        // switch to walk (the current session lands at the MRU front, its predecessor next).
        push_mru(&mut self.mru.borrow_mut(), session);
        // A successful attach RESOLVES any "lost session" the poll flagged (the caller joined that
        // poll before this commit, so its flag is now settled): clear it, so a manual switch that
        // pre-empted the reconcile cannot leave a stale flag to fire a spurious second switch.
        self.lost_session.store(false, Ordering::Release);
        self.spawn_poll_for(poll_conn, since0)?;
        (self.on_change)(); // repaint the just-attached session at once, no poll-wake lag
        Ok(())
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
                // A write is on the window this client is already showing — store the answer
                // tagged with THAT window (the mirror's own), so it is revision-guarded (the
                // answer's higher revision lands), not treated as a switch.
                let current = lock_layout(&self.layout).window.clone();
                store_layout(&self.layout, &current, snapshot.clone());
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

    /// Re-read this client's WHOLE view — windows, panes, layout — on the UI-thread connection and
    /// adopt it, the immediate-feedback follow-up to this client's OWN window op (select / new /
    /// kill window), which changes WHICH window is current.
    ///
    /// All three together, because a window op switches the current window and so the panes AND
    /// the arrangement, not just the tab set: refreshing only the windows list would leave the
    /// dock projecting the OLD window until the next poll wake. Windows FIRST, so the current
    /// window names what the layout store is tagged with — a switch RESETS the layout mirror
    /// (the new window's revision does not compare with the old's; see [`store_layout`]). The
    /// poll thread also re-reads on the revision bump this op caused; this just spares the view a
    /// wake's lag and keeps the windows / panes / layout mirrors consistent for the next write.
    fn refresh_view(&self) {
        let mut conn = self.conn.borrow_mut();
        let current = match query_windows(&mut conn) {
            Ok(list) => {
                let current = current_window_name(&list).unwrap_or_default();
                store_windows(&self.windows, list);
                current
            }
            Err(error) => {
                tracing::debug!(target: "sprag_gui::wire", %error, "refresh_view: windows re-read failed");
                return;
            }
        };
        if let Ok(seeds) = query_panes(&mut conn) {
            refresh_to_set(&mut conn, &self.cache, &seeds);
        }
        if let Ok(snapshot) = query_layout(&mut conn) {
            store_layout(&self.layout, &current, snapshot);
        }
    }

    /// Re-read every session on the UI-thread connection and store it — the immediate-feedback
    /// follow-up to this client's OWN kill of ANOTHER session, so the killed row leaves the sidebar
    /// without waiting a poll wake. Registry-wide (like the poll thread's own sessions re-read), so
    /// it does NOT detach on a scope refusal the way the scoped window/pane reads do — a transient
    /// failure just keeps the last-known list, which the poll thread's revision-bump re-read heals.
    /// Not used for the own-session kill: that detaches, so the sidebar it would refresh is going.
    fn refresh_sessions(&self) {
        let mut conn = self.conn.borrow_mut();
        match query_sessions(&mut conn) {
            Ok(list) => store_sessions(&self.sessions, list),
            Err(error) => tracing::debug!(
                target: "sprag_gui::wire",
                %error,
                "refresh_sessions: sessions re-read failed; keeping the last-known list",
            ),
        }
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
    /// path can read it every frame to notice its projection is stale (see `LayoutMirror`).
    ///
    /// Booted from a real read and kept current by the poll thread; a transient wire failure
    /// leaves the LAST KNOWN arrangement standing rather than reporting an empty one, since
    /// "the host did not answer" and "this window tiles nothing" are opposite facts that
    /// must never arrive as the same value.
    fn layout(&self) -> LayoutSnapshot {
        lock_layout(&self.layout).layout.clone()
    }

    fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot {
        // Name the WINDOW this gesture was drawn on — the one the mirror (and so the client)
        // currently projects. If the host's current window has since switched out of band
        // (another client attached to the same session), `scope.window()` won't match and the
        // host REFUSES the write rather than mis-applying this window's tree to that one (bound
        // d — the belt to the revision compare-and-set's suspenders).
        let expected_window = lock_layout(&self.layout).window.clone();
        self.write_layout(
            json!({
                "path": mux_action_path(SET_LAYOUT_ACTION),
                "args": {
                    "tree": tree,
                    "expected_revision": expected,
                    "expected_window": expected_window,
                },
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

    /// The mirrored window list — a lock and a clone, never a socket call, so the paint path can
    /// draw the tab strip every frame (see `WindowsMirror`).
    fn windows(&self) -> Vec<WindowInfo> {
        lock_windows(&self.windows).clone()
    }

    fn select_window(&self, name: &str) {
        let params = invoke(
            &mux_action_path(SELECT_WINDOW_ACTION),
            json!({ "window": name }),
        );
        if self
            .request("scene/invoke", params, "select_window")
            .is_some()
        {
            self.refresh_view();
        }
    }

    fn new_window(&self) -> String {
        let params = invoke(&mux_action_path(NEW_WINDOW_ACTION), json!({}));
        let name = self
            .request("scene/invoke", params, "new_window")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.refresh_view();
        name
    }

    fn kill_window(&self, name: &str) {
        let params = invoke(
            &mux_action_path(KILL_WINDOW_ACTION),
            json!({ "window": name }),
        );
        if self
            .request("scene/invoke", params, "kill_window")
            .is_some()
        {
            self.refresh_view();
        }
    }

    /// Create a pane in the scoped session's current window over the wire, returning its host id.
    ///
    /// Args are EMPTY on purpose: `cmd` absent is what makes the daemon apply its own `$SHELL`
    /// default, so the program a client-created pane runs is decided in one place (the host) rather
    /// than being a string this client also has an opinion about.
    fn new_pane(&self) -> Option<PaneId> {
        let params = invoke(&mux_action_path(SPAWN_ACTION), json!({}));
        let born = self
            .request("scene/invoke", params, "new_pane")
            .and_then(|value| value.as_u64())
            .map(PaneId);
        if born.is_some() {
            self.refresh_view();
        }
        born
    }

    /// Divide `target` and spawn into the half it opens, over the wire.
    ///
    /// `cmd` is absent for the same reason [`new_pane`](HostClient::new_pane)'s args are empty: the
    /// program a client-created pane runs is the host's `$SHELL` default, decided in one place.
    /// `before` is sent only when it is true, so the common request carries the two facts the
    /// daemon needs and nothing it would have defaulted anyway.
    ///
    /// Both refusals the daemon can make — an unreachable target and a child that would not start —
    /// arrive as the same absent result, which is the conflation every write on this client accepts.
    /// The caller's recourse is identical either way: the arrangement is unchanged, so re-reading it
    /// costs nothing and shows the truth.
    fn split(&self, target: PaneId, dir: SplitDir, before: bool) -> Option<PaneId> {
        let mut args = json!({
            "pane": target.0,
            "dir": match dir {
                SplitDir::Horizontal => "horizontal",
                SplitDir::Vertical => "vertical",
            },
        });
        if before {
            args["before"] = json!(true);
        }
        let born = self
            .request(
                "scene/invoke",
                invoke(&mux_action_path(SPLIT_ACTION), args),
                "split",
            )
            .and_then(|value| value.as_u64())
            .map(PaneId);
        if born.is_some() {
            // Both the pane SET and the ARRANGEMENT moved, and this client mirrors each separately
            // — so a repaint that re-read only the panes would tile a new pane the layout mirror
            // has never heard of, and drop it.
            self.refresh_view();
        }
        born
    }

    /// Close pane `id` over the wire. The daemon answers `Rejected` for an absent pane, which
    /// arrives here as the absent request result — so "no such pane" and "the socket failed" are
    /// both `false`, which is the same conflation every other write on this client accepts.
    fn kill_pane(&self, id: PaneId) -> bool {
        let params = invoke(&mux_action_path(CLOSE_ACTION), json!({ "id": id.0 }));
        let killed = self.request("scene/invoke", params, "kill_pane").is_some();
        if killed {
            self.refresh_view();
        }
        killed
    }

    /// Break the pane `id` out into a new window (tmux `break-pane`) over the wire, returning the
    /// new window's name — or `None` if the daemon refused. The scoped session rides the connection,
    /// so the args carry only the pane and an optional name.
    fn break_pane(&self, id: PaneId, name: Option<&str>) -> Option<String> {
        let mut args = json!({ "pane": id.0 });
        if let Some(name) = name {
            args["name"] = json!(name);
        }
        let params = invoke(&mux_action_path(BREAK_PANE_ACTION), args);
        let created = self
            .request("scene/invoke", params, "break_pane")
            .and_then(|value| value.as_str().map(str::to_owned));
        if created.is_some() {
            self.refresh_view();
        }
        created
    }

    /// Move the pane `id` into the window named `dst` (tmux `join-pane`) over the wire, returning
    /// whether the source window was closed — or `None` if the daemon refused.
    fn join_pane(&self, id: PaneId, dst: &str) -> Option<bool> {
        let params = invoke(
            &mux_action_path(JOIN_PANE_ACTION),
            json!({ "pane": id.0, "window": dst }),
        );
        let answer = self
            .request("scene/invoke", params, "join_pane")
            .and_then(|value| value.get("closed_source").and_then(Value::as_bool));
        if answer.is_some() {
            self.refresh_view();
        }
        answer
    }

    /// Hand a file dropped on this window to pane `id` over the wire, returning the path the pane
    /// was given — or `None` if the daemon refused it.
    ///
    /// No `refresh_view`: nothing in the pane SET changed. The pasted path
    /// arrives as ordinary pane output (and, for an upload, only when the transfer finishes), so the
    /// pane's own change notification is what repaints it — the same path a keystroke's echo takes.
    fn drop_file(&self, id: PaneId, path: &str) -> Option<String> {
        let params = invoke(
            &mux_action_path(DROP_FILE_ACTION),
            json!({ "pane": id.0, "path": path }),
        );
        self.request("scene/invoke", params, "drop_file")
            .and_then(|value| value.get("path").and_then(Value::as_str).map(str::to_owned))
    }

    /// The mirrored session list — a lock and a clone, never a socket call, so the paint path can
    /// draw the switcher every frame (see `SessionsMirror`).
    fn sessions(&self) -> Vec<SessionInfo> {
        lock_sessions(&self.sessions).clone()
    }

    /// The session this client is attached to — a client-local fact (the wire carries no
    /// "attached" marker), re-pointed by [`switch_session`](HostClient::switch_session).
    fn current_session(&self) -> String {
        self.session.borrow().clone()
    }

    /// Switch this client to the session named `name` IN PLACE (tmux `switch-client`): stop the
    /// running poll thread — joined FIRST, so it can never refresh a mirror out from under the swap
    /// — then re-attach to `name` (`attach_in_place`). A no-op for the
    /// already-current session. On failure, fall back to the session we were on so the window keeps
    /// serving; if THAT is gone too (killed while we tried to switch), detach — the tmux rule when a
    /// client can serve no session.
    ///
    /// TRACKED BOUND (responsiveness): this runs SYNCHRONOUSLY on the UI thread (the reducer) and
    /// does a thread join plus several blocking RPCs (connect + a read per pane), and `HostConn` has
    /// no read timeout — so a daemon that accepts but never answers freezes the GUI for the duration.
    /// A per-click gesture, not a per-frame path, and the daemon is local; the broader fix (a
    /// `HostConn` read deadline) is a `WireHost`-wide concern, not this seam's.
    fn switch_session(&self, name: &str) {
        if name == self.session.borrow().as_str() {
            return;
        }
        let previous = self.session.borrow().clone();
        // Tear the current poll thread down BEFORE re-pointing anything: joined first, it cannot
        // race the mirror swap. `spawn_poll_for` (inside `attach_in_place`) installs the replacement.
        // Bind `take()` to a local FIRST so the `self.poll` borrow is released before the blocking
        // `stop()`/join: a `borrow_mut()` temporary inside the `if let` would live across the whole
        // body. Sound today (the joined thread never re-borrows `self.poll`), but this removes a
        // needless `already borrowed` hazard should a future join path touch it.
        let running = self.poll.borrow_mut().take();
        if let Some(mut poll) = running {
            poll.stop();
        }
        if let Err(error) = self.attach_in_place(name) {
            tracing::warn!(
                target: "sprag_gui::wire",
                session = name,
                %error,
                "session switch failed; staying on the previous session",
            );
            if let Err(error) = self.attach_in_place(&previous) {
                tracing::error!(
                    target: "sprag_gui::wire",
                    %error,
                    "could not re-attach to the previous session either; detaching",
                );
                self.quit.request_quit();
            }
        }
    }

    /// Create a fresh session on the host (born with a shell at the boot size — it reflows to this
    /// window on first paint) and switch to it, returning its name. NO argv: a sidebar "+" carries
    /// no command, so the session births the host's default `$SHELL`. On a create failure the
    /// client stays put (the current session name is returned).
    fn new_session(&self) -> String {
        let (cols, rows) = self.boot_dims;
        let created = self.request(
            "scene/invoke",
            invoke(
                &mux_action_path(NEW_SESSION_ACTION),
                json!({ "cols": cols, "rows": rows }),
            ),
            "new_session",
        );
        match created.as_ref().and_then(Value::as_str) {
            Some(name) => {
                let name = name.to_owned();
                self.switch_session(&name);
                name
            }
            None => self.current_session(),
        }
    }

    /// Kill the session named `name` on the host (tmux `kill-session`). Three outcomes, split on
    /// whether it is THIS client's own attached session and — if so — the
    /// `detach_on_destroy` policy:
    ///
    /// * **Own session, a successor exists** → SWITCH: under a `next`/`previous` policy, re-point this
    ///   client at the neighbouring session ([`switch_session`](HostClient::switch_session)) instead
    ///   of leaving — tmux `detach-on-destroy next`. The successor is picked from the CURRENT list
    ///   BEFORE the kill (which removes `name`); the kill of a NON-last session leaves the daemon
    ///   alive, so its reply returns and the following `switch_session`'s scoped reads succeed.
    /// * **Own session, nothing to switch to** → DETACH: the `on` default, or `name` was the last
    ///   session — ask the shell to quit, the immediate form of the tmux rule that a client whose
    ///   session is destroyed leaves (the poll thread's `detach_reason` is the backstop and the
    ///   out-of-band path). We detach whether the reply came back or was severed: killing the LAST
    ///   session ends the daemon, so the reply can be cut off (EOF/reset), indistinguishable from
    ///   success here — and either way this client is leaving.
    /// * **Another session** → keep serving ours; drop the killed row from the sidebar at once with a
    ///   `refresh_sessions` (the poll thread's revision-bump re-read is
    ///   the backstop for the same change arriving out of band).
    ///
    /// The invoke's answer is intentionally ignored (see the detach note); a genuine refusal — only
    /// an unknown name for this action — leaves every session as it was, and the sidebar the next
    /// re-read paints is unchanged.
    fn kill_session(&self, name: &str) {
        let params = invoke(
            &mux_action_path(KILL_SESSION_ACTION),
            json!({ "name": name }),
        );
        let is_own = name == self.session.borrow().as_str();
        // For an OWN kill under a switch policy, pick the successor NOW — BEFORE the kill removes
        // `name` from the list, so `next`/`previous` resolve against the list the user sees. `None`
        // means detach (the `on` default, or `name` is the last session). A kill of ANOTHER session
        // never switches this client.
        let successor = is_own
            .then(|| {
                destroy_successor(
                    self.detach_on_destroy,
                    &self.sessions(),
                    name,
                    &self.mru.borrow(),
                )
            })
            .flatten();
        if let Some(next) = successor {
            // switch-to-next. STOP the poll thread BEFORE the kill so the own-kill switch is
            // DETERMINISTIC and self-contained. Killing `name` bumps the scene revision, waking the
            // poll (still scoped to the dying session) into a re-query the host now REFUSES; under a
            // switch policy its error arm takes the OUT-OF-BAND path — it flags `lost_session` and
            // repaints (NOT `request_quit`; that is only `HostGone`) — whose reconcile would then
            // RACE the switch we are about to do (a flag `attach_in_place` would have to clear again).
            // Joining the poll first means no flag is ever raised for our OWN kill, and no wasted
            // re-query/repaint on the dead scope. With the poll gone, kill, then switch:
            // [`switch_session`] attaches to `next` and, if that fails (the successor died in the
            // gap), falls back to the now-dead `name` and so detaches — the correct end state when
            // there is nothing left to serve.
            let running = self.poll.borrow_mut().take();
            if let Some(mut poll) = running {
                poll.stop();
            }
            let _ = self.request("scene/invoke", params, "kill_session");
            self.switch_session(&next);
            return;
        }
        let _ = self.request("scene/invoke", params, "kill_session");
        if is_own {
            // Own kill with nothing to switch to → DETACH.
            self.quit.request_quit();
        } else {
            // Another session killed → keep serving ours; drop the killed row now.
            self.refresh_sessions();
        }
    }

    /// Resolve a session lost OUT OF BAND (another client / the `sprag` CLI killed THIS client's
    /// attached session) under the `detach_on_destroy` policy — the second
    /// destroy trigger, sharing the same switch-vs-detach decision as
    /// [`kill_session`](HostClient::kill_session)'s own-kill handling. The poll thread cannot switch
    /// (a UI-thread op), so it sets `lost_session` + repaints; this runs on the
    /// UI thread each frame and, when the flag is set, switches-to-next or detaches.
    ///
    /// Swap-claim the flag so it fires ONCE. The session mirror still lists the just-lost session —
    /// the poll broke on the scoped-read refusal BEFORE its next registry-wide sessions re-read — so
    /// `destroy_successor` finds it and returns a live neighbour; `None` (it was the last session,
    /// or already gone from the mirror) detaches. `switch_session` joins the now-broken poll and
    /// attaches to the neighbour (whose own commit re-clears the flag).
    ///
    /// COVERAGE: the end-to-end switch here and in [`kill_session`](HostClient::kill_session)'s own
    /// branch is NOT unit-tested — both drive [`switch_session`](HostClient::switch_session) /
    /// `request`, which need a live daemon. The testable pieces ARE covered (the pure
    /// `destroy_successor` pick and the poll thread's flag+repaint), and
    /// [`switch_session`](HostClient::switch_session) itself is live-smoke-proven (R170); this is the
    /// same accepted live-smoke gap the session-sidebar rounds carry.
    fn reconcile_lost_session(&self) {
        if self.lost_session.swap(false, Ordering::AcqRel) {
            let me = self.session.borrow().clone();
            let successor = destroy_successor(
                self.detach_on_destroy,
                &self.sessions(),
                &me,
                &self.mru.borrow(),
            );
            match successor {
                Some(next) => self.switch_session(&next),
                None => self.quit.request_quit(),
            }
        }
    }

    /// Switch to the LAST session — the most-recent OTHER session this client visited that is still
    /// live (tmux `switch-client -l`), walking the MRU stack (`mru_live_other`). A no-op when the
    /// client has visited no other surviving session (a fresh client that never switched, or all its
    /// prior sessions are gone) — matching tmux, which also no-ops with no last session.
    fn switch_to_last_session(&self) {
        let current = self.session.borrow().clone();
        // Resolve into a local so the `mru` borrow is released before `switch_session` (which
        // re-borrows `mru` mutably via `attach_in_place`'s `push_mru`).
        let last = mru_live_other(&self.mru.borrow(), &self.sessions(), &current);
        if let Some(last) = last {
            self.switch_session(&last);
        }
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

    fn resize(&self, id: PaneId, cols: u16, rows: u16, cell_px: (u16, u16)) {
        let params = invoke(
            &mux_action_path(RESIZE_ACTION),
            resize_args(id, cols, rows, cell_px),
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

    fn paste(&self, id: PaneId, text: &str) -> bool {
        // Forward the raw text; the host brackets it (and filters an embedded end marker) if the
        // pane's child has enabled DEC private mode 2004. This client cannot see the pane's input
        // modes, so the bracketing decision stays at the PTY boundary.
        let params = invoke(
            &pane_input_path(id.0, PASTE_ACTION),
            json!({ "text": text }),
        );
        self.request("scene/invoke", params, "paste").is_some()
    }

    /// REPORT a mouse event: forward the SEMANTIC event (cell + button edge + mods) to the host,
    /// which gates it against the pane's LIVE tracking mode and encodes the X10 / SGR report at the
    /// PTY boundary — the same mode-authority-at-the-boundary split as [`Self::paste`] / send_key
    /// (this client cannot see the pane's input modes, so it never encodes). The wire arg shape is
    /// the `mouse` action's `{button, kind, col, row, ctrl, alt, shift}` (host `parse_mouse_args`).
    fn mouse(&self, id: PaneId, event: MouseInput) -> bool {
        self.request(
            "scene/invoke",
            invoke(&pane_input_path(id.0, MOUSE_ACTION), mouse_wire_args(event)),
            "mouse",
        )
        .is_some()
    }

    /// REPORT a pane FOCUS change: forward `{focused}` to the host, which sends `ESC [ I` / `ESC [ O`
    /// when the pane's child has enabled DEC 1004 (a no-op otherwise) — the same
    /// mode-authority-at-the-boundary split as [`Self::mouse`] (this client never encodes).
    fn focus(&self, id: PaneId, focused: bool) -> bool {
        self.request(
            "scene/invoke",
            invoke(
                &pane_input_path(id.0, FOCUS_ACTION),
                json!({ "focused": focused }),
            ),
            "focus",
        )
        .is_some()
    }

    fn pane_full_text(&self, id: PaneId) -> String {
        let params = json!({ "path": pane_input_path(id.0, FULL_TEXT_SLOT) });
        self.request("scene/query", params, "pane_full_text")
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// The pane's search matches, over the `find.<needle>` query family. On demand (a find bar
    /// keystroke), never per frame — the needle rides the PATH, so this stays a READ and a client
    /// typing in the bar wakes no other client's parked `waitFor`. Deserialized into the SAME
    /// [`PaneFind`] the host serialized, so the two ends cannot drift on a field name.
    fn pane_find(&self, id: PaneId, needle: &str) -> PaneFind {
        if needle.is_empty() {
            return PaneFind::default(); // the host answers Null for an empty member; do not ask
        }
        let params = json!({ "path": pane_input_path(id.0, &find_slot_for(needle)) });
        self.request("scene/query", params, "pane_find")
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// The pane's REGEX matches, over the `regex.<pattern>` query family — a DIFFERENT address from
    /// `pane_find`'s, not the same one with a flag, so the wire never has to guess which language the
    /// characters are in. An empty pattern is a malformed member the host answers `Null` for, so it is
    /// not asked; a REFUSED pattern is a well-formed address whose value the engine rejected, and its
    /// answer deserializes into the same [`PaneFind`] carrying `error`.
    fn pane_find_regex(&self, id: PaneId, pattern: &str) -> PaneFind {
        if pattern.is_empty() {
            return PaneFind::default();
        }
        let params = json!({ "path": pane_input_path(id.0, &regex_slot_for(pattern)) });
        self.request("scene/query", params, "pane_find_regex")
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// The project governing pane `id`, over the mux `project.<pane>` slot. On demand (a palette
    /// opening), never per frame — the answer costs a filesystem walk host-side.
    ///
    /// The host distinguishes three answers and so does this: `Null` is "no project here" (`None`),
    /// an `{error}` object is a project whose config is unusable, and anything else deserialises
    /// into the SAME [`Project`] the host serialised, so the two ends cannot drift on a field name.
    /// An unparseable payload is reported as a malformed config rather than silently dropped, since
    /// the alternative is a client that shows an empty command list for a project that has one.
    ///
    /// The error travels ALREADY RENDERED and is passed through verbatim, exactly like
    /// [`Self::global_commands`]'s: the host is the end that knows the file is `.sprag.toml`, so
    /// re-wrapping it in a `ProjectError` here only re-prefixed a name the sentence already had.
    fn project(&self, id: PaneId) -> Option<Result<Project, String>> {
        let params = json!({ "path": mux_action_path(&project_slot_for(id.0)) });
        let value = self.request("scene/query", params, "project")?;
        if value.is_null() {
            return None;
        }
        if let Some(message) = value.get("error").and_then(Value::as_str) {
            return Some(Err(message.to_owned()));
        }
        Some(serde_json::from_value::<Project>(value).map_err(|error| error.to_string()))
    }

    /// The user's declared commands, over the mux `commands` slot. On demand (a palette opening),
    /// never per frame — the answer costs a file read host-side.
    ///
    /// Three answers, like the project slot's: `Null` is "no config written", an `{error}` object is
    /// one that cannot be used, and anything else deserialises into the SAME [`UserConfig`] the host
    /// serialised. The error travels ALREADY RENDERED and is passed through verbatim — the host is
    /// the end that knows which file it is about, so this one does not re-word it.
    fn global_commands(&self) -> Option<Result<UserConfig, String>> {
        let params = json!({ "path": mux_action_path(GLOBAL_COMMANDS_SLOT) });
        let value = self.request("scene/query", params, "global_commands")?;
        if value.is_null() {
            return None;
        }
        if let Some(message) = value.get("error").and_then(Value::as_str) {
            return Some(Err(message.to_owned()));
        }
        Some(serde_json::from_value::<UserConfig>(value).map_err(|error| error.to_string()))
    }

    fn pane_prompt_positions(&self, id: PaneId) -> Vec<usize> {
        // On demand (a jump-to-prompt keypress), not per frame — so it does not ride the
        // cached cells frame. The host serves a JSON array of logical line indices.
        let params = json!({ "path": pane_input_path(id.0, PROMPT_MARKS_SLOT) });
        self.request("scene/query", params, "pane_prompt_positions")
            .and_then(|value| {
                value.as_array().map(|arr| {
                    arr.iter()
                        .filter_map(|v| usize::try_from(v.as_u64()?).ok())
                        .collect()
                })
            })
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

    /// Served from the same poll-refreshed mirror as [`Self::pane_title`], re-adopted each wake,
    /// so the `seq` reflects the host's latest.
    fn pane_notification(&self, id: PaneId) -> Option<PaneNotification> {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.notification.clone())
    }

    /// Served from the same poll-refreshed mirror as [`Self::pane_notification`], re-adopted each
    /// wake, so the bell count reflects the host's latest.
    fn pane_bell_seq(&self, id: PaneId) -> u64 {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map_or(0, |pane| pane.bell_seq)
    }

    /// Whether the child has exited, served from the same poll-refreshed mirror as
    /// [`Self::pane_bell_seq`].
    ///
    /// A wake-stale answer here is benign in the one direction it can be wrong: liveness is
    /// ONE-WAY, so the worst case is a just-exited pane still reading live for a poll interval —
    /// never a live pane declared dead.
    fn pane_is_dead(&self, id: PaneId) -> bool {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .is_some_and(|pane| pane.dead)
    }

    /// HOW the child ended, from the same mirror as [`Self::pane_is_dead`].
    ///
    /// Wake-stale in one benign direction too, and a NARROWER one: the status is published after
    /// the liveness bit, so the worst case is a dead pane reading "(exited)" for a poll interval
    /// before it names its code — never a code attributed to a pane that is still running.
    fn pane_child_exit(&self, id: PaneId) -> Option<PaneExit> {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.child_exit.clone())
    }

    /// The child's mouse-tracking bit, served from the same poll-refreshed mirror as
    /// [`Self::pane_bell_seq`] (re-adopted each wake). The pane pointer oracle reads it per frame to
    /// gate pointer capture + decide drag / motion forwarding; the authoritative encode still
    /// re-reads the live mode host-side in [`Self::mouse`], so a one-wake-stale level can at most
    /// mis-gate a single event. `pane_mouse_active` is the trait's derived `.is_active()`.
    fn pane_mouse_protocol(&self, id: PaneId) -> MouseProtocol {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map_or(MouseProtocol::None, |pane| pane.mouse_protocol)
    }

    /// The image SUMMARIES (`{id,width,height,anchor,seq}`, RGBA empty), served from the same
    /// poll-refreshed mirror as [`Self::pane_bell_seq`], re-adopted each wake, so the composited
    /// images reflect the host's latest transmit / clear. The RGBA is fetched separately via
    /// [`Self::pane_image_rgba`] (R1404 Stage 5 on-demand).
    fn pane_images(&self, id: PaneId) -> Vec<Image> {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map(|pane| pane.images.clone())
            .unwrap_or_default()
    }

    /// One image's RGBA, fetched ON DEMAND over the wire (`image_data.<id>`, R1404 Stage 5) when the
    /// compositor sees a new / changed `(id, seq)` — not per poll, since the raster is up to a MiB.
    /// `None` if the host returns `Null` (the pane no longer shows that id) or a decode fails.
    fn pane_image_rgba(&self, id: PaneId, image_id: u32) -> Option<Vec<u8>> {
        let params = json!({ "path": pane_input_path(id.0, &format!("image_data.{image_id}")) });
        let value = self.request("scene/query", params, "pane_image_rgba")?;
        STANDARD.decode(value.as_str()?).ok()
    }

    /// The CHEAP clipboard-write count, served from the poll-refreshed mirror (no round-trip) —
    /// `clipboard_osc` polls it each frame and fetches the payload only when it grows.
    fn pane_clipboard_write_seq(&self, id: PaneId) -> u64 {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .map_or(0, |pane| pane.clipboard_write_seq)
    }

    /// The pending read query (selection + seq), served from the mirror (no round-trip).
    fn pane_clipboard_query(&self, id: PaneId) -> Option<PaneClipboardQuery> {
        self.lock_cache()
            .iter()
            .find(|pane| pane.id == id)
            .and_then(|pane| pane.clipboard_query)
    }

    /// The actual clipboard WRITE payload — an ON-DEMAND `scene/query` (like
    /// [`Self::pane_full_text`]), issued only when the mirrored write seq grows, so the
    /// (potentially large) paste never rides the per-frame path.
    fn pane_clipboard_write(&self, id: PaneId) -> Option<PaneClipboardWrite> {
        let params = json!({ "path": pane_input_path(id.0, CLIPBOARD_WRITE_SLOT) });
        let value = self.request("scene/query", params, "pane_clipboard_write")?;
        let object = value.as_object()?;
        let targets = object.get("targets").and_then(Value::as_object);
        Some(PaneClipboardWrite {
            targets: ClipboardTargets {
                clipboard: targets
                    .and_then(|t| t.get("clipboard"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                primary: targets
                    .and_then(|t| t.get("primary"))
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
            },
            text: object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            seq: object.get("seq").and_then(Value::as_u64).unwrap_or(0),
        })
    }

    /// Answer a read query — an ON-DEMAND `scene/invoke` of the `clipboard_answer` action. The
    /// host arbitrates exactly-once across clients and replies `{wrote}`; `true` here means THIS
    /// client's answer reached the PTY. A selection char (`c`/`p`) from the [`ClipboardTarget`].
    fn answer_clipboard_query(
        &self,
        id: PaneId,
        seq: u64,
        target: ClipboardTarget,
        text: &str,
    ) -> bool {
        let args = json!({ "seq": seq, "sel": target.osc_char().to_string(), "text": text });
        let params = invoke(&pane_input_path(id.0, CLIPBOARD_ANSWER_ACTION), args);
        self.request("scene/invoke", params, "answer_clipboard_query")
            .and_then(|value| value.get("wrote").and_then(Value::as_bool))
            .unwrap_or(false)
    }
}

impl Drop for WireHost {
    fn drop(&mut self) {
        // The host is a DAEMON we do not own — closing this client leaves it (and the user's
        // shells) running, which is the whole detach/reattach point. So there is nothing to kill
        // here; we only stop OUR poll thread. [`PollThread::stop`] flags `stop`, cancels the parked
        // `scene/waitFor` read (so the join is deterministic — the daemon never closes the socket
        // for us), and joins. A `None` means the client never finished booting, or a switch is
        // mid-swap (unreachable — Drop and switch are both the one UI thread). When the last live
        // pane across every session finally exits, the daemon self-cleans (its own reaper).
        // Bind `take()` to a local FIRST so the `self.poll` borrow is released before the blocking
        // `stop()`/join: a `borrow_mut()` temporary inside the `if let` would live across the whole
        // body. Sound today (the joined thread never re-borrows `self.poll`), but this removes a
        // needless `already borrowed` hazard should a future join path touch it.
        let running = self.poll.borrow_mut().take();
        if let Some(mut poll) = running {
            poll.stop();
        }
    }
}

/// `scene/invoke` params: the addressed `path` + its `args`.
fn invoke(path: &str, args: Value) -> Value {
    json!({ "path": path, "args": args })
}

/// The `resize` action's arguments — the pane, the new character grid, and OPTIONALLY the
/// display's cell pixel geometry.
///
/// **An unknown cell metric is spelled by OMITTING the two keys, never by sending a zero**, and
/// that is a hard requirement rather than a tidiness preference. The action reads them with the
/// host's `opt_dim`, which accepts only a POSITIVE dimension and rejects a present zero outright —
/// so a client that helpfully sent `(0, 0)` had its WHOLE resize refused, `cols` and `rows`
/// included, and got back an invoke error naming nothing about a cell.
///
/// It was not hypothetical. It is how the terminal client's every resize failed on its first run,
/// silently, with the pane left at the size whoever created it chose. The GUI could never see it —
/// a font metric is never zero — and `sprag resize-pane` had always omitted the keys, so the wire
/// client was the one caller spelling "unknown" the way the host refuses. `(0, 0)` remains the
/// TRAIT's spelling ([`HostClient::resize`]); this is where it is translated into the wire's.
///
/// A HALF-known metric is treated as unknown for the same reason the trait carries a pair: a cell
/// has two dimensions, and a width with no height describes nothing the emulator could use.
fn resize_args(id: PaneId, cols: u16, rows: u16, cell_px: (u16, u16)) -> Value {
    let mut args = json!({ "id": id.0, "cols": cols, "rows": rows });
    if cell_px.0 > 0 && cell_px.1 > 0 {
        args["cell_width"] = json!(cell_px.0);
        args["cell_height"] = json!(cell_px.1);
    }
    args
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
/// pre-fork instant (no output there); we null them. It boots with no STRAY pane (`--daemon`) —
/// this client's panes are spawned into its own session afterwards; any sessions it RESTORES from
/// a durability snapshot are named sessions a client attaches to, never a boot pane of ours.
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
    /// The pane's most recent attention notification, `None` when the wire omits the key
    /// (the child raised none — the additive `skip`-when-absent shape).
    notification: Option<PaneNotification>,
    /// The pane's tmux monitor-bell count, `0` when the wire omits the key (the child rang none,
    /// or an older daemon).
    bell_seq: u64,
    /// Whether the pane's child has EXITED, `false` when the wire omits the key (it is live, or an
    /// older daemon). One-way: a pane never comes back to life.
    dead: bool,
    /// HOW the child ended, `None` when the wire omits the key — which covers a live pane, a dead
    /// one the host has not reaped yet, and an older daemon alike. A client cannot tell those apart
    /// and does not need to: all three mean "no status to show".
    child_exit: Option<PaneExit>,
    /// The pane's OSC 52 clipboard-write count, `0` when the wire omits the key (no write, or an
    /// older daemon).
    clipboard_write_seq: u64,
    /// The pane's pending OSC 52 read query (selection + seq), `None` when the wire omits the key.
    clipboard_query: Option<PaneClipboardQuery>,
    /// The pane's inline images (Kitty graphics, R1404), empty when the wire omits the key.
    images: Vec<Image>,
    /// The pane's live mouse-tracking protocol level, parsed from the additive `mouse` wire token
    /// ([`MouseProtocol::from_wire_str`]); `None` when the key is omitted (no tracking / older daemon).
    mouse_protocol: MouseProtocol,
    dims: (u16, u16),
    /// What a fetch of this pane's CELLS would depend on, as the host reported it
    /// ([`sprag_grid::ProjectionToken`]). `None` when the wire omits the key — an older daemon, or
    /// a token the host could not serialize — which means "fetch anyway".
    projection: Option<ProjectionToken>,
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
            let notification = parse_notification(&pane["notification"]);
            let bell_seq = pane["bell_seq"].as_u64().unwrap_or(0);
            // ADDITIVE: present only once the child has exited, so absent means live.
            let dead = pane["dead"].as_bool().unwrap_or(false);
            // ADDITIVE and LATER than `dead`: present only once the host reaped the child.
            let child_exit = parse_child_exit(&pane["child_exit"]);
            let clipboard_write_seq = pane["clipboard_write_seq"].as_u64().unwrap_or(0);
            let clipboard_query = parse_clipboard_query(&pane["clipboard_query"]);
            let images = parse_images(&pane["images"]);
            // ADDITIVE: the `mouse` key is a protocol-level token present only while the child is
            // tracking; parse it back to the level (absent / unknown -> None) via the vt SSOT.
            let mouse_protocol = MouseProtocol::from_wire_str(pane["mouse"].as_str());
            let cols = u16::try_from(pane["cols"].as_u64().unwrap_or(1)).unwrap_or(1);
            let rows = u16::try_from(pane["rows"].as_u64().unwrap_or(1)).unwrap_or(1);
            // ADDITIVE: absent (or unparseable) reads as `None`, which makes this pane fetch
            // unconditionally — the safe direction, since a skipped fetch is what freezes a pane.
            let projection =
                serde_json::from_value::<ProjectionToken>(pane["projection"].clone()).ok();
            Ok(PaneSeed {
                id: PaneId(id),
                label,
                title,
                notification,
                bell_seq,
                dead,
                child_exit,
                clipboard_write_seq,
                clipboard_query,
                images,
                mouse_protocol,
                dims: (cols, rows),
                projection,
            })
        })
        .collect()
}

/// Parse the additive `child_exit` object (`{code, signal?}`) back into a [`PaneExit`].
///
/// Absent, `null` or codeless is `None` — "no status to show" — rather than a defaulted `code: 0`,
/// which would tell a user their still-running command had succeeded. The one field that may be
/// missing from a WELL-FORMED object is `signal`, which rides only a signalled death.
fn parse_child_exit(value: &Value) -> Option<PaneExit> {
    Some(PaneExit {
        code: u32::try_from(value["code"].as_u64()?).ok()?,
        signal: value["signal"].as_str().map(str::to_owned),
    })
}

/// Build the `mouse` action's wire args from a semantic [`MouseInput`] — the object shape the
/// host's `parse_mouse_args` decodes (`{button, kind, col, row, ctrl, alt, shift}`). Extracted from
/// [`WireHost::mouse`] so the wire grammar is unit-testable without a live host. `super` is not
/// carried: a mouse report has no encoding for the logo key (host-side `parse_mouse_args` fixes it
/// to `false`).
fn mouse_wire_args(event: MouseInput) -> Value {
    json!({
        "button": mouse_button_wire(event.button),
        "kind": mouse_kind_wire(event.kind),
        "col": event.col,
        "row": event.row,
        "ctrl": event.mods.ctrl,
        "alt": event.mods.alt,
        "shift": event.mods.shift,
    })
}

/// The `button` wire token for a [`MouseButton`] — the vocabulary the host's `parse_mouse_args`
/// decodes back. The producer twin of that parser, kept beside [`WireHost::mouse`] so the wire
/// grammar has one emitter.
fn mouse_button_wire(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Middle => "middle",
        MouseButton::Right => "right",
        MouseButton::WheelUp => "wheelup",
        MouseButton::WheelDown => "wheeldown",
        MouseButton::WheelLeft => "wheelleft",
        MouseButton::WheelRight => "wheelright",
        MouseButton::None => "none",
    }
}

/// The `kind` wire token for a [`MouseEventKind`] — the edge vocabulary the host's
/// `parse_mouse_args` decodes back.
fn mouse_kind_wire(kind: MouseEventKind) -> &'static str {
    match kind {
        MouseEventKind::Press => "press",
        MouseEventKind::Release => "release",
        MouseEventKind::Drag => "drag",
        MouseEventKind::Motion => "motion",
    }
}

/// Parse a pane's `notification` wire value (`{title, body, seq}`, or absent/`null`) into a
/// [`PaneNotification`]. A missing key or a non-object is `None` (the additive shape: a pane that
/// raised none, or an older daemon that never sends it). A malformed `seq` clamps to `0` so a
/// present-but-garbled object never claims to be a fresh notification.
fn parse_notification(value: &Value) -> Option<PaneNotification> {
    let object = value.as_object()?;
    Some(PaneNotification {
        title: object
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
        body: object
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        seq: object.get("seq").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// Parse a pane's `images` wire value — a JSON array of `{id, width, height, anchor:[col,row],
/// rgba_b64}` (Kitty graphics, R1404), or absent/`null` — into [`Image`]s. A missing key, a
/// non-array, or a malformed entry (bad base64, or a byte count that is not `width*height*4`)
/// yields an empty list / drops the entry, so a garbled payload never paints a torn image.
fn parse_images(value: &Value) -> Vec<Image> {
    let Some(array) = value.as_array() else {
        return Vec::new();
    };
    array
        .iter()
        .filter_map(|entry| {
            let id = u32::try_from(entry["id"].as_u64()?).ok()?;
            let width = u32::try_from(entry["width"].as_u64()?).ok()?;
            let height = u32::try_from(entry["height"].as_u64()?).ok()?;
            let anchor = entry["anchor"].as_array()?;
            let col = u16::try_from(anchor.first()?.as_u64()?).ok()?;
            let row = u16::try_from(anchor.get(1)?.as_u64()?).ok()?;
            let seq = entry["seq"].as_u64()?;
            Some(Image {
                id,
                width,
                height,
                // A SUMMARY: the RGBA is fetched on demand via `image_data.<id>` (R1404 Stage 5),
                // keyed on `(id, seq)` — it does not ride the panes slot.
                rgba: Vec::new(),
                anchor: (col, row),
                seq,
            })
        })
        .collect()
}

/// Parse a pane's `clipboard_query` wire value (`{sel, seq}`, or absent/`null`) into a
/// [`PaneClipboardQuery`]. A missing key, a non-object, or an unknown `sel` is `None` (the
/// additive shape: a pane that issued no read, or a daemon that never sends it). `c` -> clipboard,
/// `p` -> primary; a malformed `seq` clamps to `0`.
fn parse_clipboard_query(value: &Value) -> Option<PaneClipboardQuery> {
    let object = value.as_object()?;
    let target = match object.get("sel").and_then(Value::as_str) {
        Some("c") => ClipboardTarget::Clipboard,
        Some("p") => ClipboardTarget::Primary,
        _ => return None,
    };
    Some(PaneClipboardQuery {
        target,
        seq: object.get("seq").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// Resolve WHICH session this client acts on, over `conn` (before it is scoped).
///
/// `requested` is the caller's [`SESSION_ENV`] (resolved by `spawn_or_attach` so this function
/// stays a pure, testable decision over its inputs, not a reader of process-global env).
/// Returns the session name and whether this client CREATED it:
/// * `requested` names one → ATTACH (`created = false`): the session must already exist
///   on the reached host; its live panes are adopted (tmux reattach). A name no session
///   carries makes the first scoped read fail, which fails the boot honestly rather than
///   silently opening an empty window. NO `new_session` is sent — attach never births a pane.
/// * `None` → ALLOCATE a fresh session (`created = true`) via the registry's own auto-naming
///   ([`NEW_SESSION_ACTION`] with no name), so two clients never invent one name and race.
///   This call is deliberately made BEFORE the connection is scoped — creating a session is a
///   registry-wide act, not one scoped to a session that does not exist yet.
///
/// The allocate call passes THIS client's first pane (`cols`/`rows`/`argv`) to `new_session`,
/// which births the session with exactly that pane — tmux's `new-session -x -y [command]`. So
/// [`boot_panes`] tops up from a pane that already matches the configured layout (no default-shell
/// first pane to reconcile). This narrows the old create→spawn window (a fresh session with no
/// live pane for one whole RPC, vulnerable to the daemon self-cleaning out from under it): the
/// birth pane makes the session live before `new_session` even answers, so from this client's
/// vantage it is never observably empty. A narrower in-handler race survives host-side (an
/// unrelated last pane dying between the create and the birth spawn) — inherent, fail-safe, and
/// documented at [`new_session`]'s birth site; it is not fully eliminated here.
///
/// [`new_session`]: sprag_host::wire::NEW_SESSION_ACTION
fn resolve_session(
    conn: &mut HostConn,
    requested: Option<&str>,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
) -> io::Result<(String, bool)> {
    if let Some(name) = requested {
        return Ok((name.to_owned(), false));
    }
    let mut args = json!({ "cols": cols, "rows": rows });
    if let Some(argv) = argv {
        args["cmd"] = json!(argv);
    }
    let answer = conn.call(
        "scene/invoke",
        invoke(&mux_action_path(NEW_SESSION_ACTION), args),
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
///   `n_panes` (the GUI is the operator asking for its configured layout). The session is born
///   with one pane ([`resolve_session`] passed its `cols`/`rows`/`argv` to `new_session`), so
///   this tops up FROM that birth pane — spawning `n_panes - 1` more running `argv` at
///   `cols x rows` — then takes the first `n_panes`. (If the birth pane failed to spawn, the
///   count starts at 0 and this still reaches `n_panes`, so it doubles as the recovery path.)
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
///
/// A frame the host answered but this client could not READ is reported at a different level,
/// because it is a different fact. The tolerated case is transient and self-correcting: the pane
/// closed, the next wake will not ask about it. A payload that does not deserialize means the two
/// ends disagree about the frame's wire shape — which sprag has no protocol version handshake to
/// catch — and it is neither transient nor self-correcting: every wake will fail the same way and
/// the window will show nothing at all. Logging both at `debug` made the second one look like the
/// first, so the shape change in R222 that made the skew reachable is the reason for the split.
fn fetch_frames(conn: &mut HostConn, ids: &[PaneId]) -> Vec<(PaneId, CellFrame)> {
    let mut fetched = Vec::with_capacity(ids.len());
    for &id in ids {
        match fetch_frame(conn, id.0) {
            Ok(frame) => fetched.push((id, frame)),
            Err(error) if error.kind() == io::ErrorKind::InvalidData => tracing::error!(
                target: "sprag_gui::wire",
                pane = id.0,
                %error,
                "pane frame did not deserialize; this client and the running daemon disagree \
                 about the frame's wire shape (a daemon older than this build will not be \
                 readable — restart it), so no pane will be mirrored",
            ),
            Err(error) => tracing::debug!(
                target: "sprag_gui::wire",
                pane = id.0,
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
    let fetched = fetch_frames(conn, &pane_ids_of(&seeds));
    merge_panes(&[], &seeds, &fetched)
}

/// Every seed's id — the BOOT / re-attach case of the fetch set, where there is no cache to
/// compare a token against and so nothing to skip.
fn pane_ids_of(seeds: &[PaneSeed]) -> Vec<PaneId> {
    seeds.iter().map(|seed| seed.id).collect()
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
    // Decide WHAT to fetch under a short lock, then fetch off it (never a socket call while
    // locked). Deciding first is the whole point: a pane nothing has happened in costs one
    // comparison instead of a whole-screen projection on the host and a grid on the wire.
    let stale = {
        let guard = lock_cache(cache);
        stale_panes(&guard, seeds)
    };
    let fetched = fetch_frames(conn, &stale);
    // Rebuild the cache in host order under one lock (the pure merge is `merge_panes`).
    let mut guard = lock_cache(cache);
    let rebuilt = merge_panes(&guard, seeds, &fetched);
    *guard = rebuilt;
}

/// Which panes this wake must actually re-fetch the cells of — PURE, so the policy that decides
/// whether a client can go on painting a frame it already holds is unit-tested without a socket.
///
/// Three reasons to fetch, and the residue is the win:
///
/// * a **newcomer** has no frame at all, so there is nothing to keep;
/// * a pane with **no token** on either side — an older daemon, a token the host could not
///   serialize, or a frame taken before this client learned to record one — is fetched
///   unconditionally, because "I cannot tell" must never resolve to "assume unchanged";
/// * a pane whose token **moved** since the frame we hold: by
///   [`ProjectionToken`]'s contract an unequal token is the only thing that can mean the frame
///   differs, and an equal one guarantees it does not.
///
/// The asymmetry is deliberate and is the reason this is safe: the token can be stale-but-equal
/// only if it was read BEFORE the frame it labels, which the host and [`merge_panes`] between them
/// rule out — the host reads it under the same lock as the pane list, and the merge stores it only
/// beside a frame this wake fetched. Every other imprecision costs a redundant fetch.
fn stale_panes(existing: &[WirePane], seeds: &[PaneSeed]) -> Vec<PaneId> {
    seeds
        .iter()
        .filter(|seed| {
            let Some(prior) = existing.iter().find(|pane| pane.id == seed.id) else {
                return true; // newcomer: no frame to keep
            };
            match (&prior.projection, &seed.projection) {
                (Some(held), Some(current)) => held != current,
                _ => true, // cannot tell: fetch
            }
        })
        .map(|seed| seed.id)
        .collect()
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
        let fresh = fetched
            .iter()
            .find(|(id, _)| *id == seed.id)
            .map(|(_, frame)| frame.clone());
        let refetched = fresh.is_some();
        let frame = fresh.or_else(|| prior.map(|pane| pane.frame.clone()));
        let Some(frame) = frame else {
            continue; // a brand-new pane whose first frame is not here yet — next wake
        };
        rebuilt.push(WirePane {
            id: seed.id,
            label: seed.label.clone(), // host-authoritative — always the query's label
            title: seed.title.clone(), // host-authoritative + dynamic — re-adopt every wake
            // host-authoritative + dynamic like the title: re-adopt the query's, so the seq
            // grows as the child raises more (and clears to None if the host ever drops it).
            notification: seed.notification.clone(),
            bell_seq: seed.bell_seq, // host-authoritative + dynamic, like the notification
            dead: seed.dead,         // host-authoritative, and one-way once true
            // ...and the status, on the same terms. Re-adopted rather than kept, because unlike
            // `dead` this one CHANGES after the pane dies: the reap lands a wake or two after the
            // exit, and a kept value would freeze the pane at "(exited)" for good.
            child_exit: seed.child_exit.clone(),
            // host-authoritative + dynamic like the notification: re-adopt the query's, so the
            // clipboard write count / read query reflect the child's latest for `clipboard_osc`.
            clipboard_write_seq: seed.clipboard_write_seq,
            clipboard_query: seed.clipboard_query,
            // host-authoritative + dynamic: re-adopt the query's images each wake.
            images: seed.images.clone(),
            // host-authoritative + dynamic: re-adopt the query's mouse-tracking level each wake, so
            // the capture gate + drag/motion forwarding track the child enabling / disabling reporting.
            mouse_protocol: seed.mouse_protocol,
            // The token is stored ONLY beside a frame this wake actually fetched. A survivor whose
            // fetch was skipped keeps the token its frame was taken under, and a survivor whose
            // fetch was ATTEMPTED and missed keeps it too — adopting the query's token beside an
            // older frame is the one move that would freeze the pane. The token is therefore never
            // newer than the frame it labels; at worst it is older, which costs a redundant fetch.
            projection: if refetched {
                seed.projection.clone()
            } else {
                prior.and_then(|pane| pane.projection.clone())
            },
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
// The poll thread's inputs: its own connection, the FOUR shared mirrors it refreshes (cache /
// layout / windows / sessions), the repaint + quit sinks, the destroy `policy` + the `lost_session`
// flag it raises for an out-of-band session kill under a switch policy, the stop flag, and the
// subscribe baseline. Well over clippy's default limit — bundling the four `Arc<Mutex<_>>` mirrors
// into a struct would ripple through every `WireHost` method that reads one, a churn out of
// proportion to this seam.
#[allow(clippy::too_many_arguments)]
fn spawn_poll(
    mut conn: HostConn,
    cache: Cache,
    layout: LayoutMirror,
    windows: WindowsMirror,
    sessions: SessionsMirror,
    on_change: Arc<dyn Fn() + Send + Sync>,
    quit: Arc<dyn QuitSink>,
    policy: DetachOnDestroy,
    lost_session: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    mut since: u64,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("sprag-gui-wire-poll".to_owned())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                let response = match conn.call("scene/waitFor", json!({ "since": since })) {
                    Ok(value) => value,
                    Err(error) => {
                        // A DEFINITIVE end detaches — or, for an out-of-band session kill under a
                        // switch policy, flags the UI thread to switch-to-next (decided inside
                        // [`handle_poll_error`]); a transient hiccup re-parks the long-poll rather
                        // than ending the client. A stop-initiated error is our own graceful Drop,
                        // which the helper's `stopped` check leaves silent.
                        if handle_poll_error(
                            &error,
                            &stop,
                            policy,
                            &lost_session,
                            &quit,
                            &on_change,
                        ) {
                            break;
                        }
                        continue;
                    }
                };
                if stop.load(Ordering::Acquire) {
                    break;
                }
                since = response["revision"].as_u64().unwrap_or(since);
                // Re-read the window list FIRST each wake, so a new / killed / renamed / selected
                // window (this client's or another attached one's) reaches the tab strip — AND so
                // the current window is known before the layout store, which it tags: a switch
                // (this session's current window moved, e.g. another client's select_window) RESETS
                // the layout mirror rather than dropping the new window's read as stale (see
                // [`store_layout`]). A definitive failure detaches (as below); a transient one keeps
                // the last-known list and tags the layout store with the mirror's OWN window (a
                // hiccup is not a switch), so that store stays revision-guarded.
                let current: String = match query_windows(&mut conn) {
                    Ok(list) => {
                        let current = current_window_name(&list).unwrap_or_default();
                        store_windows(&windows, list);
                        current
                    }
                    Err(error) => {
                        if handle_poll_error(
                            &error,
                            &stop,
                            policy,
                            &lost_session,
                            &quit,
                            &on_change,
                        ) {
                            break;
                        }
                        tracing::debug!(
                            target: "sprag_gui::wire",
                            %error,
                            "windows re-read failed this wake; keeping the last-known list",
                        );
                        lock_layout(&layout).window.clone()
                    }
                };
                // Re-query the live pane set so a host-side spawn/close is MIRRORED (cache
                // add/remove), not just existing panes refreshed. A DEFINITIVE failure (our session
                // was killed) detaches at once — no stale repaint first; a transient one refreshes
                // the known set instead so liveness holds (the change is caught on a later wake).
                match query_panes(&mut conn) {
                    Ok(seeds) => refresh_to_set(&mut conn, &cache, &seeds),
                    Err(error) => {
                        if handle_poll_error(
                            &error,
                            &stop,
                            policy,
                            &lost_session,
                            &quit,
                            &on_change,
                        ) {
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
                                // Likewise keep the last-known notification (and its seq) rather
                                // than dropping the attention badge on a transient query miss.
                                notification: pane.notification.clone(),
                                bell_seq: pane.bell_seq, // keep the last-known bell count too
                                // Liveness is ONE-WAY, so a stale `true` can never become a lie —
                                // and a stale `false` is only the pre-liveness reading, corrected
                                // on the next successful query.
                                dead: pane.dead,
                                // and the last-known status with it: a transient miss must not
                                // retract a code the user has already been shown.
                                child_exit: pane.child_exit.clone(),
                                // keep the last-known clipboard signals across a transient miss
                                clipboard_write_seq: pane.clipboard_write_seq,
                                clipboard_query: pane.clipboard_query,
                                images: pane.images.clone(), // keep last-known images too
                                mouse_protocol: pane.mouse_protocol, // keep last-known tracking level too
                                dims: pane.dims,
                                // Keep the token the held frame was taken under: the re-query
                                // failed, so the host's current one is unknown, and inventing one
                                // either way would either freeze the pane or force a full refetch.
                                projection: pane.projection.clone(),
                            })
                            .collect();
                        refresh_to_set(&mut conn, &cache, &seeds);
                    }
                }
                // Re-read the arrangement each wake too, so a host-side change — another
                // attached client's gesture, a plugin's spawn, a float, a WINDOW SWITCH — reaches
                // this client's projection. Tagged with `current`, so a switch resets the mirror.
                // A definitive failure detaches (as above); on a transient one the last-known
                // arrangement stands (a hiccup means "no news", never "your layout is gone").
                match query_layout(&mut conn) {
                    Ok(snapshot) => store_layout(&layout, &current, snapshot),
                    Err(error) => {
                        if handle_poll_error(
                            &error,
                            &stop,
                            policy,
                            &lost_session,
                            &quit,
                            &on_change,
                        ) {
                            break;
                        }
                        tracing::debug!(
                            target: "sprag_gui::wire",
                            %error,
                            "layout re-read failed this wake; keeping the last-known arrangement",
                        );
                    }
                }
                // Re-read EVERY session each wake so a session created / killed anywhere on the host
                // (this client's `new_session`, another client's, the `sprag` CLI) reaches the
                // switcher sidebar. Registry-wide, so it does not detach on a scope refusal the way
                // the scoped reads above do — a transient failure just keeps the last-known list.
                match query_sessions(&mut conn) {
                    Ok(list) => store_sessions(&sessions, list),
                    Err(error) => tracing::debug!(
                        target: "sprag_gui::wire",
                        %error,
                        "sessions re-read failed this wake; keeping the last-known list",
                    ),
                }
                on_change();
            }
        })
}

/// A DEFINITIVE poll-thread failure, classified by whether the DAEMON survives it — which is what
/// decides whether a switch-to-next is even possible when this client's session is destroyed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PollEnd {
    /// A scoped request the host ANSWERED with a refusal ([`io::ErrorKind::Other`] — [`HostConn::call`]
    /// maps a JSON-RPC error object to it). For a client that scopes every request to its session,
    /// the only such refusal is that session being killed while the DAEMON serves on for others — so
    /// the daemon lives, and under a switch policy this client can move to a neighbour instead of
    /// leaving.
    SessionClosed,
    /// The connection is DEAD — the daemon exited, or a killed LAST session took the whole daemon
    /// with it. Nothing survives to switch to, so this is always a detach whatever the policy.
    HostGone,
}

impl PollEnd {
    /// The human-readable cause for the detach log line.
    fn reason(self) -> &'static str {
        match self {
            PollEnd::SessionClosed => "this client's session was closed",
            PollEnd::HostGone => "the host exited",
        }
    }
}

/// Why a poll-thread request failed, if the failure is DEFINITIVE — `None` only for a genuinely
/// transient hiccup to tolerate (re-park / keep the last frame).
///
/// Definitiveness is the DEFAULT: a broken pipe, a reset, an EOF — any dead connection — ends this
/// client's poll, tmux's rule that a client leaves when it can no longer serve its session. Only the
/// handful of retryable kinds are tolerated; classifying a dead-socket write error (`BrokenPipe`) as
/// transient would spin the long-poll forever, never ending. The [`PollEnd`] variant separates the
/// two definitive causes (session-gone vs host-gone), which the caller needs to decide detach-vs-switch.
fn detach_reason(error: &io::Error) -> Option<PollEnd> {
    match error.kind() {
        // Retryable: a signal interrupted the syscall, a non-blocking op would block, or a read
        // timed out. Re-park and try again; the connection itself is fine. This arm is a
        // DEFENSIVE guard, not a live path today: [`HostConn`] is a blocking socket with no read
        // timeout, so `WouldBlock`/`TimedOut` never arise and `Interrupted` is absorbed inside
        // `read_line`'s retry — every error the poll actually meets is definitive. It stays so a
        // future non-blocking connection re-parks a hiccup rather than ending on it.
        io::ErrorKind::Interrupted | io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut => None,
        // A refusal the host actually answered with — for a scoped client, its session is gone.
        io::ErrorKind::Other => Some(PollEnd::SessionClosed),
        // Any other error means the connection is dead — the host is gone.
        _ => Some(PollEnd::HostGone),
    }
}

/// React to a poll-thread request `error` under the destroy `policy`, returning whether the caller
/// should BREAK the poll loop (a definitive end) or keep polling (a transient hiccup). The tmux
/// `detach-on-destroy` split, on the ONE path an out-of-band destroy reaches this client:
/// * transient ([`detach_reason`] `None`) → keep polling (`false`).
/// * [`HostGone`](PollEnd::HostGone), or [`SessionClosed`](PollEnd::SessionClosed) under the `Detach`
///   policy → DETACH now ([`request_detach`], byte-identical to before the policy existed), so the
///   default path stays the poll thread's own immediate quit — nothing depends on the UI reconcile.
/// * `SessionClosed` under a SWITCH policy → the daemon lives and we have somewhere to go, but a
///   switch is a UI-thread op: FLAG [`lost_session`](WireHost::lost_session) and repaint, so the
///   UI-thread [`reconcile_lost_session`](WireHost::reconcile_lost_session) performs it. Skipped
///   while WE are tearing down (`stop`) — that is our own graceful teardown, not a lost session, and
///   falls through to the `request_detach` that then no-ops on `stopped`.
///
/// Returns `true` for any definitive end (the loop breaks either way — by a quit or a flagged switch).
fn handle_poll_error(
    error: &io::Error,
    stop: &AtomicBool,
    policy: DetachOnDestroy,
    lost_session: &AtomicBool,
    quit: &Arc<dyn QuitSink>,
    on_change: &Arc<dyn Fn() + Send + Sync>,
) -> bool {
    let Some(end) = detach_reason(error) else {
        return false; // transient — keep polling
    };
    let stopped = stop.load(Ordering::Acquire);
    match end {
        PollEnd::SessionClosed if policy.is_switch() && !stopped => {
            // Our session died out of band and a switch policy (any but Detach) wants us to MOVE, not
            // leave — but the switch is a UI-thread operation. Flag it and wake the UI thread; its
            // reconcile owns the switch. (While `stopped`, this is our own teardown, handled by the
            // detach arm below.)
            lost_session.store(true, Ordering::Release);
            on_change();
        }
        // HostGone (any policy), or a Detach-policy session close, or our own teardown → detach now.
        _ => request_detach(quit, stopped, end.reason(), error),
    }
    true // definitive — break the loop
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

    /// The panes-slot `images` wire value round-trips to [`Image`] SUMMARIES (R1404 Stage 5): each
    /// carries `{id,width,height,anchor,seq}` and an EMPTY rgba (fetched on demand), and an entry
    /// missing a field is dropped. NO rgba rides the panes slot.
    #[test]
    fn parse_images_round_trips_summaries_and_drops_malformed() {
        let value = json!([
            { "id": 7, "width": 2, "height": 1, "anchor": [3, 4], "seq": 5 },
            // missing seq → dropped (the revert-proof guard: the summary must carry the generation).
            { "id": 8, "width": 2, "height": 2, "anchor": [0, 0] },
            // missing anchor → dropped.
            { "id": 9, "width": 1, "height": 1, "seq": 1 },
        ]);
        let images = parse_images(&value);
        assert_eq!(images.len(), 1, "only the well-formed summary survives");
        assert_eq!(images[0].id, 7);
        assert_eq!((images[0].width, images[0].height), (2, 1));
        assert_eq!(images[0].anchor, (3, 4));
        assert_eq!(images[0].seq, 5);
        assert!(images[0].rgba.is_empty(), "the summary carries NO rgba");
        assert!(parse_images(&Value::Null).is_empty(), "absent ⇒ empty");
    }

    /// [`WireHost::mouse`] serializes a semantic event into the exact object shape the host's
    /// `parse_mouse_args` decodes (`{button, kind, col, row, ctrl, alt, shift}`) — a token typo here
    /// would make the host silently drop the report. Pins the vocabulary + the 0-based coordinates.
    #[test]
    fn mouse_wire_args_matches_the_host_parse_shape() {
        let event = MouseInput {
            button: MouseButton::Left,
            kind: MouseEventKind::Press,
            col: 4,
            row: 2,
            mods: Modifiers {
                ctrl: true,
                alt: false,
                shift: true,
                sup: true, // never travels — a mouse report has no logo bit
            },
        };
        assert_eq!(
            mouse_wire_args(event),
            json!({
                "button": "left",
                "kind": "press",
                "col": 4,
                "row": 2,
                "ctrl": true,
                "alt": false,
                "shift": true,
            }),
        );
        // The release edge + the other buttons/edges use the tokens the parser matches.
        assert_eq!(mouse_button_wire(MouseButton::Right), "right");
        assert_eq!(mouse_button_wire(MouseButton::WheelUp), "wheelup");
        assert_eq!(mouse_kind_wire(MouseEventKind::Release), "release");
        assert_eq!(mouse_kind_wire(MouseEventKind::Motion), "motion");
    }

    /// A structural session list in creation order, the shape [`destroy_successor`] reads — only the
    /// name matters to the neighbour pick, so the live fields are empty.
    fn session_list(names: &[&str]) -> Vec<SessionInfo> {
        names
            .iter()
            .map(|name| SessionInfo {
                name: (*name).to_owned(),
                windows: 1,
                panes: 1,
                default: false,
                cwd: None,
                branch: None,
                ports: Vec::new(),
                attached: 0,
            })
            .collect()
    }

    /// The policy env parses to the tmux values, DEFAULTING to detach for anything unrecognized —
    /// so a client only ever switches away on an EXPLICIT `off`/`no-detached`/`next`/`previous`,
    /// never on a typo or an unset env. REVERT-PROOF: drop the trim/lowercase and `"  NEXT "` stops
    /// matching; map the wildcard to `Next` and the `"on"`/unset/bogus cases start switching; the
    /// hyphenless `"nodetached"` proves the match is EXACT (a near-miss detaches, never silently
    /// picks a switch policy).
    #[test]
    fn the_detach_policy_env_defaults_to_detach_and_reads_off_no_detached_next_previous() {
        assert_eq!(parse_detach_on_destroy(None), DetachOnDestroy::Detach);
        assert_eq!(parse_detach_on_destroy(Some("on")), DetachOnDestroy::Detach);
        assert_eq!(parse_detach_on_destroy(Some("")), DetachOnDestroy::Detach);
        assert_eq!(
            parse_detach_on_destroy(Some("sideways")),
            DetachOnDestroy::Detach
        );
        assert_eq!(parse_detach_on_destroy(Some("off")), DetachOnDestroy::Off);
        assert_eq!(parse_detach_on_destroy(Some(" OFF ")), DetachOnDestroy::Off);
        assert_eq!(
            parse_detach_on_destroy(Some("no-detached")),
            DetachOnDestroy::NoDetached
        );
        assert_eq!(
            parse_detach_on_destroy(Some(" No-Detached ")),
            DetachOnDestroy::NoDetached
        );
        // A near-miss (hyphen dropped) is NOT no-detached — it falls to the safe detach default.
        assert_eq!(
            parse_detach_on_destroy(Some("nodetached")),
            DetachOnDestroy::Detach
        );
        assert_eq!(parse_detach_on_destroy(Some("next")), DetachOnDestroy::Next);
        assert_eq!(
            parse_detach_on_destroy(Some("  NEXT ")),
            DetachOnDestroy::Next
        );
        assert_eq!(
            parse_detach_on_destroy(Some("Previous")),
            DetachOnDestroy::Previous,
        );
    }

    /// The `next`/`previous` target is the LIST NEIGHBOUR (wrapping), or `None` (detach) for the
    /// detach policy, the last remaining session, or a name already gone from the list. `mru` is
    /// ignored by these policies (passed empty). REVERT-PROOF: a step of 0 or a missing wrap breaks
    /// the neighbour/wrap rows; returning `Some` for the `Detach`, single-session, or absent-name
    /// cases fails a `None` assertion — each of which would turn a safe detach into a wrong switch.
    #[test]
    fn destroy_successor_next_previous_is_the_wrapping_list_neighbour_or_a_detach() {
        let list = session_list(&["a", "b", "c"]);
        // Detach policy never switches.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Detach, &list, "b", &[]),
            None
        );
        // Next: the row below, wrapping the last back to the first.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &list, "a", &[]).as_deref(),
            Some("b"),
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &list, "c", &[]).as_deref(),
            Some("a"),
        );
        // Previous: the row above, wrapping the first back to the last.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &list, "a", &[]).as_deref(),
            Some("c"),
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &list, "b", &[]).as_deref(),
            Some("a"),
        );
        // Nothing to switch to → detach: the last session, or a name already off the list.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &session_list(&["only"]), "only", &[]),
            None,
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &list, "gone", &[]),
            None
        );
    }

    /// `off` prefers the MOST-RECENT OTHER visited session that is still live, then FALLS BACK to the
    /// `next` list neighbour when none survives (so it still switches rather than detaching whenever
    /// another session exists), then detaches only when `killed` is the last session. REVERT-PROOF:
    /// remove the MRU walk and the first two rows drop to the list neighbour (b, not the MRU pick);
    /// remove the `1` fallback (e.g. return None) and the "never visited another" row wrongly
    /// detaches; keep the `len < 2` guard and the last-session row stays a detach.
    #[test]
    fn destroy_successor_off_prefers_the_mru_then_falls_back_to_the_neighbour() {
        let list = session_list(&["a", "b", "c"]);
        let mru = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        // MRU front is the current (killed) session; the next entry is the most-recent OTHER — pick
        // it even though the LIST neighbour of "a" would be "b".
        assert_eq!(
            destroy_successor(DetachOnDestroy::Off, &list, "a", &mru(&["a", "c", "b"])).as_deref(),
            Some("c"),
            "off takes the most-recent other, not the list neighbour",
        );
        // The most-recent other ("c") is dead (not in the list) → skip it, take the next live MRU
        // entry ("b").
        let live = session_list(&["a", "b"]);
        assert_eq!(
            destroy_successor(DetachOnDestroy::Off, &live, "a", &mru(&["a", "c", "b"])).as_deref(),
            Some("b"),
            "a dead MRU entry is skipped",
        );
        // Never visited another session (MRU holds only the killed one), but others exist → FALL
        // BACK to the list neighbour rather than detach.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Off, &list, "a", &mru(&["a"])).as_deref(),
            Some("b"),
            "off falls back to the neighbour when no visited session survives",
        );
        // Truly the last session → detach even under off.
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::Off,
                &session_list(&["only"]),
                "only",
                &mru(&["only"])
            ),
            None,
            "off detaches only when there is no other session",
        );
    }

    /// A session list carrying explicit per-session viewer counts — the extra fact `no-detached`
    /// reads over the name-only [`session_list`].
    fn attached_list(entries: &[(&str, usize)]) -> Vec<SessionInfo> {
        entries
            .iter()
            .map(|(name, attached)| SessionInfo {
                name: (*name).to_owned(),
                windows: 1,
                panes: 1,
                default: false,
                cwd: None,
                branch: None,
                ports: Vec::new(),
                attached: *attached,
            })
            .collect()
    }

    /// `no-detached` switches ONLY to a session no other client is on (`attached == 0`),
    /// MRU-preferred, and DETACHES rather than pile onto an occupied one — the whole point that
    /// distinguishes it from `off`. REVERT-PROOF: drop the `attached == 0` filter and it degrades to
    /// `off` (the all-watched row switches to "b" instead of detaching, and the occupied "b" is no
    /// longer skipped); drop the MRU preference and the two-free row takes the list-order "b" not the
    /// most-recent "c"; drop the list-order fallback and the empty-MRU row wrongly detaches. The
    /// paired `off` assertions in the SAME worlds pin that only `no-detached` consults the count.
    #[test]
    fn destroy_successor_no_detached_switches_only_to_a_free_session_else_detaches() {
        let mru = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        // killed "a"; "b" held by another client, "c" free. MRU walks a (killed) → b (occupied,
        // skipped) → c (free, picked) — the count overrides the more-recent-but-occupied "b".
        let one_free = attached_list(&[("a", 1), ("b", 1), ("c", 0)]);
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::NoDetached,
                &one_free,
                "a",
                &mru(&["a", "b", "c"])
            )
            .as_deref(),
            Some("c"),
            "no-detached skips the session another client is on and takes the free one",
        );
        // No MRU hint → still finds the free session by scanning list order.
        assert_eq!(
            destroy_successor(DetachOnDestroy::NoDetached, &one_free, "a", &[]).as_deref(),
            Some("c"),
            "no-detached falls back to the first free session in list order",
        );
        // Every OTHER session is watched by another client → DETACH (never share).
        let all_watched = attached_list(&[("a", 1), ("b", 2), ("c", 1)]);
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::NoDetached,
                &all_watched,
                "a",
                &mru(&["a", "b", "c"])
            ),
            None,
            "no-detached leaves rather than pile onto an occupied session",
        );
        // Contrast in the SAME world: `off` ignores the counts and switches onto the occupied "b".
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::Off,
                &all_watched,
                "a",
                &mru(&["a", "b", "c"])
            )
            .as_deref(),
            Some("b"),
            "off ignores viewer counts; only no-detached respects them",
        );
        // Two free sessions → the most-recent (MRU) free one wins over the list-order-earlier free.
        let two_free = attached_list(&[("a", 1), ("b", 0), ("c", 0)]);
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::NoDetached,
                &two_free,
                "a",
                &mru(&["a", "c", "b"])
            )
            .as_deref(),
            Some("c"),
            "the most-recent free session wins over the list-order-earlier free one",
        );
        // The last session → detach.
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::NoDetached,
                &attached_list(&[("only", 0)]),
                "only",
                &[]
            ),
            None,
            "no-detached detaches when there is no other session",
        );
    }

    /// `mru_live_other` — the tmux "last session" pick (and `off`'s MRU preference) — is the
    /// most-recent OTHER visited session still live, skipping a since-dead entry, `None` when none
    /// survives (the last-session no-op). REVERT-PROOF: drop the `!= current` guard and it returns
    /// `current`; drop the liveness filter and it returns a dead session.
    #[test]
    fn mru_live_other_is_the_most_recent_live_other_session() {
        let list = session_list(&["a", "b", "c"]);
        let mru = |names: &[&str]| names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>();
        // Most-recent OTHER (the MRU front is the current session) that is live.
        assert_eq!(
            mru_live_other(&mru(&["a", "c", "b"]), &list, "a").as_deref(),
            Some("c"),
        );
        // The most-recent other ("c") is dead → skip to the next live MRU entry ("b").
        assert_eq!(
            mru_live_other(&mru(&["a", "c", "b"]), &session_list(&["a", "b"]), "a").as_deref(),
            Some("b"),
        );
        // Never visited another (only the current session in the MRU) → None (last-session no-op).
        assert_eq!(mru_live_other(&mru(&["a"]), &list, "a"), None);
        // Every prior session has since died → None.
        assert_eq!(
            mru_live_other(&mru(&["a", "z"]), &session_list(&["a"]), "a"),
            None,
        );
    }

    /// [`push_mru`] keeps a most-recent-first, deduplicated visit stack: a fresh name goes to the
    /// front; a repeat MOVES to the front (never a duplicate). REVERT-PROOF: drop the `retain` and
    /// the repeat leaves a duplicate + a stale earlier entry, failing the dedup/order assertions.
    #[test]
    fn push_mru_dedups_and_moves_to_front() {
        let mut stack = Vec::new();
        push_mru(&mut stack, "a");
        push_mru(&mut stack, "b");
        push_mru(&mut stack, "c");
        assert_eq!(stack, vec!["c", "b", "a"], "most-recent-first");
        // Re-visiting "a" moves it to the front and leaves no duplicate.
        push_mru(&mut stack, "a");
        assert_eq!(
            stack,
            vec!["a", "c", "b"],
            "a repeat moves to front, no dupe"
        );
    }

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
    /// REVERT-PROOF: delete the `quit.request_quit()` call in `request_detach` and this reads 0 —
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
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            DetachOnDestroy::Detach,
            Arc::new(AtomicBool::new(false)),
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
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            DetachOnDestroy::Detach,
            Arc::new(AtomicBool::new(false)),
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

    /// A connected [`HostConn`] whose server answers the FIRST request `Ok` (a revision bump —
    /// what the kill's own bump does to wake a parked poll) and then REFUSES every later request
    /// with `-32602`. It never closes; the client's own detach ends it.
    fn a_wake_then_refuse_host_conn(tag: &str) -> (HostConn, JoinHandle<()>, SockGuard) {
        use std::io::Write;
        let path = sock_path(tag);
        let listener = UnixListener::bind(&path).expect("bind the throwaway host socket");
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect to it");
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept the client");
            let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
            let mut writer = stream;
            let mut line = String::new();
            let mut first = true;
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                let reply = if std::mem::take(&mut first) {
                    // The kill bumped the shared revision; the parked waitFor wakes Ok.
                    json!({ "jsonrpc": "2.0", "id": request["id"], "result": { "revision": 1 } })
                } else {
                    // Every scoped request after that names the killed session → refused.
                    json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "error": { "code": -32602, "message": "no session named \"1\"" },
                    })
                };
                let _ = writeln!(writer, "{reply}");
                let _ = writer.flush();
                line.clear();
            }
        });
        (conn, server, SockGuard(path))
    }

    /// The DOCUMENTED production sequence for a killed NON-last session: the kill bumps the shared
    /// revision, so the parked `waitFor` wakes `Ok`, and the refusal lands on the RE-QUERY —
    /// where the client detaches BEFORE repainting one stale frame. The `on_change` counter is
    /// the non-vacuous check: a detach that fell through to `on_change` would repaint the dead
    /// layout once. REVERT-PROOF: delete the `detach_reason` `break` from BOTH re-query arms and
    /// `repaints` reads 1 (the client repaints stale, then detaches on the next waitFor's refusal).
    #[test]
    fn a_killed_session_detaches_on_the_re_query_without_a_stale_repaint() {
        let (conn, server, _guard) = a_wake_then_refuse_host_conn("requery");
        let quit = Arc::new(RecordingQuit::default());
        let repaints = Arc::new(AtomicUsize::new(0));
        let on_change: Arc<dyn Fn() + Send + Sync> = {
            let repaints = Arc::clone(&repaints);
            Arc::new(move || {
                repaints.fetch_add(1, Ordering::SeqCst);
            })
        };
        let stop = Arc::new(AtomicBool::new(false));
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            on_change,
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            DetachOnDestroy::Detach,
            Arc::new(AtomicBool::new(false)),
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");
        poll.join().expect("the poll thread exited");
        server.join().expect("the server thread exited");

        assert_eq!(
            quit.0.load(Ordering::SeqCst),
            1,
            "the re-query refusal detaches the client",
        );
        assert_eq!(
            repaints.load(Ordering::SeqCst),
            0,
            "and it detaches BEFORE repainting a stale frame",
        );
    }

    /// EVERY switch policy's out-of-band trigger: a client whose session is killed while the daemon
    /// serves on does NOT detach under `off`/`next`/`previous` — the poll thread FLAGS the loss and
    /// repaints, so the UI-thread reconcile can switch instead. This is what distinguishes
    /// [`handle_poll_error`]'s switch arm (gated on [`DetachOnDestroy::is_switch`]) from the default
    /// detach — and looping ALL THREE is what pins the invariant: `off` was once missing from a
    /// hand-listed `Next | Previous` guard and silently detached (a live smoke caught it); the
    /// `is_switch` predicate + this loop mean a policy added later cannot be forgotten here.
    ///
    /// REVERT-PROOF: delete the switch arm (so every definitive end calls `request_detach`) and each
    /// iteration reads `quit == 1` / `lost == false`; drop a policy from `is_switch` and its row here
    /// fails. Keep them and the client leaves the poll WITHOUT quitting, flag raised, repaint fired.
    #[test]
    fn every_switch_policy_flags_a_lost_session_instead_of_detaching() {
        for (policy, tag) in [
            (DetachOnDestroy::Off, "switch-lost-off"),
            (DetachOnDestroy::Next, "switch-lost-next"),
            (DetachOnDestroy::Previous, "switch-lost-prev"),
        ] {
            let (conn, server, _guard, _seen) = a_session_killed_host_conn(tag);
            let quit = Arc::new(RecordingQuit::default());
            let lost = Arc::new(AtomicBool::new(false));
            let repaints = Arc::new(AtomicUsize::new(0));
            let on_change: Arc<dyn Fn() + Send + Sync> = {
                let repaints = Arc::clone(&repaints);
                Arc::new(move || {
                    repaints.fetch_add(1, Ordering::SeqCst);
                })
            };
            let stop = Arc::new(AtomicBool::new(false)); // NOT our teardown: the session was killed
            let poll = spawn_poll(
                conn,
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(Mirrored::default())),
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(Vec::new())),
                on_change,
                Arc::clone(&quit) as Arc<dyn QuitSink>,
                policy,
                Arc::clone(&lost),
                Arc::clone(&stop),
                0,
            )
            .expect("spawn the poll thread");
            poll.join().expect("the poll thread exited");
            server.join().expect("the server thread exited");

            assert_eq!(
                quit.0.load(Ordering::SeqCst),
                0,
                "{policy:?}: a switch policy must NOT detach on an out-of-band session kill",
            );
            assert!(
                lost.load(Ordering::SeqCst),
                "{policy:?}: it flags the lost session for the UI reconcile to switch",
            );
            assert!(
                repaints.load(Ordering::SeqCst) >= 1,
                "{policy:?}: and repaints so the UI thread wakes to run that reconcile",
            );
        }
    }

    /// Under a SWITCH policy, a DEAD HOST (the whole daemon gone, not merely our session) still
    /// DETACHES — nothing survives to switch to. Guards the `HostGone` half of [`handle_poll_error`]'s
    /// catch-all against a regression that widened the switch arm to ANY `Next`/`Previous`.
    /// REVERT-PROOF: change the switch-arm guard from `(PollEnd::SessionClosed, Next | Previous)` to
    /// `(_, Next | Previous)` and this reads `quit == 0` / `lost == true` — the client wrongly tries
    /// to switch off a daemon that is gone.
    #[test]
    fn a_switch_policy_still_detaches_when_the_whole_host_is_gone() {
        let (conn, _listener, _guard) = a_dead_host_conn("switch-hostgone");
        let quit = Arc::new(RecordingQuit::default());
        let lost = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false)); // NOT our teardown: the host died
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            DetachOnDestroy::Next,
            Arc::clone(&lost),
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");
        poll.join().expect("the poll thread exited");

        assert_eq!(
            quit.0.load(Ordering::SeqCst),
            1,
            "a dead HOST detaches even under a switch policy — nothing survives to switch to",
        );
        assert!(
            !lost.load(Ordering::SeqCst),
            "and it does NOT flag a switch (that is only a session lost while the daemon lives)",
        );
    }

    /// Under a SWITCH policy, a session-close refusal that arrives DURING OUR OWN teardown must
    /// neither flag a switch NOR quit: a graceful Drop / switch teardown is not a lost session. This
    /// guards the switch arm's `if !stopped` — a distinct guard from the one
    /// `our_own_teardown_does_not_ask_the_shell_to_quit` covers (`request_detach`'s). Driven the same
    /// way: the thread is parked in `conn.call` when `stop` flips, then the server answers the parked
    /// request with the `-32602` a killed-session scope yields. REVERT-PROOF: drop the `if !stopped`
    /// from the switch arm and this reads `lost == true` (teardown spuriously flags a switch).
    #[test]
    fn our_own_teardown_under_a_switch_policy_does_not_flag_a_switch() {
        use std::io::Write;
        let path = sock_path("teardown-switch");
        let listener = UnixListener::bind(&path).expect("bind");
        let _guard = SockGuard(path.clone());
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect");
        let (server, _) = listener.accept().expect("accept");

        let quit = Arc::new(RecordingQuit::default());
        let lost = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false)); // FALSE, so the loop enters and parks
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            DetachOnDestroy::Next,
            Arc::clone(&lost),
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");

        // Read the parked `waitFor` request: the thread is now blocked in `conn.call`, exactly where
        // a teardown finds it.
        let mut reader = BufReader::new(server.try_clone().expect("clone server"));
        let mut writer = server;
        let mut line = String::new();
        reader.read_line(&mut line).expect("read the request");
        assert!(line.contains("scene/waitFor"), "the parked request: {line}");
        let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);

        // WE tear down (set stop, ordered before the write), THEN answer the parked request with the
        // killed-session refusal. The thread wakes to a `SessionClosed` error but sees `stopped`, so
        // under the switch policy it must neither flag nor quit.
        stop.store(true, Ordering::Release);
        let reply = json!({
            "jsonrpc": "2.0",
            "id": request["id"],
            "error": { "code": -32602, "message": "no session named \"1\"" },
        });
        writeln!(writer, "{reply}").expect("write the refusal");
        writer.flush().expect("flush the refusal");
        poll.join().expect("the poll thread exited");

        assert_eq!(
            quit.0.load(Ordering::SeqCst),
            0,
            "a refusal during our own teardown is not a lost session; it must not quit",
        );
        assert!(
            !lost.load(Ordering::SeqCst),
            "and it must not spuriously flag a switch during teardown",
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
    /// REVERT-PROOF: drop the `if !stopped` guard in `request_detach` (so it quits regardless)
    /// and this reads 1 — proving it exercises that exact guard, which the preset-`true` version
    /// did not.
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
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            DetachOnDestroy::Detach,
            Arc::new(AtomicBool::new(false)),
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
        stop.store(true, Ordering::Release);
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

    /// A client with a real font metric sends it, and a client with none OMITS the keys rather than
    /// zeroing them — the distinction the host's `opt_dim` makes between "absent" and "invalid".
    ///
    /// REVERT-PROOF: write the two keys unconditionally (the shape this carried until the terminal
    /// client found it) and the second half fails. The end-to-end consequence — a resize that is
    /// refused whole, leaving the pane at its old size — is `sprag-tui`'s PTY gate, because that is
    /// the only place a REFUSAL is observable; from in here a wrong argument still looks like JSON.
    #[test]
    fn an_unknown_cell_metric_is_absent_from_a_resize_not_zero() {
        let with_metric = resize_args(PaneId(3), 100, 30, (9, 18));
        assert_eq!(with_metric["id"], 3);
        assert_eq!(with_metric["cols"], 100);
        assert_eq!(with_metric["rows"], 30);
        assert_eq!(with_metric["cell_width"], 9);
        assert_eq!(with_metric["cell_height"], 18);

        let without = resize_args(PaneId(3), 100, 30, (0, 0));
        assert_eq!(without["cols"], 100, "the grid is still sent");
        assert_eq!(without["rows"], 30);
        assert!(
            without.get("cell_width").is_none() && without.get("cell_height").is_none(),
            "an unknown metric is absent, not zero: {without}",
        );

        // A half-known metric is no metric: neither key is sent rather than one.
        for half in [(9, 0), (0, 18)] {
            let args = resize_args(PaneId(3), 100, 30, half);
            assert!(
                args.get("cell_width").is_none() && args.get("cell_height").is_none(),
                "{half:?} describes no cell: {args}",
            );
        }
    }

    /// The layout mirror is WINDOW-AWARE: a store for a DIFFERENT window RESETS unconditionally
    /// (the per-window revision does not compare across windows), while a store for the SAME window
    /// is revision-GUARDED (an older read overtaken by a newer write is dropped — the R154 scar).
    ///
    /// REVERT-PROOF of the fix: if `store_layout` guarded across windows too (the pre-fix
    /// per-window-only assumption), the switch to a LOWER-revision window below would be dropped as
    /// "stale" and the mirror would keep projecting the old window's tree over the new window's
    /// panes — a broken dock. The `revision == 3` and `window == "1"` assertions catch exactly that.
    #[test]
    fn the_layout_mirror_resets_on_a_window_switch_but_guards_within_a_window() {
        let mirror: LayoutMirror = Arc::new(Mutex::new(Mirrored::default()));
        let snap = |rev| LayoutSnapshot {
            revision: rev,
            ..Default::default()
        };

        // Boot window "0" at revision 5.
        store_layout(&mirror, "0", snap(5));
        assert_eq!(lock_layout(&mirror).layout.revision, 5);

        // Within window "0": a LOWER revision (a stale poll read overtaken by a UI write) is
        // dropped — the mirror must not move backward on the same window.
        store_layout(&mirror, "0", snap(3));
        assert_eq!(
            lock_layout(&mirror).layout.revision,
            5,
            "same window: an older read is dropped (R154)",
        );

        // A SWITCH to window "1" whose revision is LOWER (3) RESETS the mirror — NOT dropped as
        // stale, because the per-window revision does not compare across windows.
        store_layout(&mirror, "1", snap(3));
        assert_eq!(
            lock_layout(&mirror).window,
            "1",
            "the mirror moved to the new window"
        );
        assert_eq!(
            lock_layout(&mirror).layout.revision,
            3,
            "a switch resets the mirror even to a lower revision",
        );

        // Within window "1": a higher revision lands as usual.
        store_layout(&mirror, "1", snap(7));
        assert_eq!(lock_layout(&mirror).layout.revision, 7);
    }

    /// A projection token distinguishable by its single row generation, so a test can say
    /// "the same screen" or "a changed one" without building an emulator.
    fn token(generation: u64) -> ProjectionToken {
        ProjectionToken {
            row_generations: vec![generation],
            cursor: pinion_core::GridCursor::default(),
            screen: pinion_core::ScreenKind::Main,
            cols: 80,
            scrollback_len: 0,
        }
    }

    /// A cached pane holding `frame(3)`, taken under `held`.
    fn cached(id: u64, held: Option<ProjectionToken>) -> WirePane {
        WirePane {
            id: PaneId(id),
            projection: held,
            label: "bash".to_owned(),
            title: None,
            notification: None,
            bell_seq: 0,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            frame: frame(3),
            dims: (80, 24),
        }
    }

    /// A host pane-list entry for `id`, reporting `current`.
    fn seeded(id: u64, current: Option<ProjectionToken>) -> PaneSeed {
        PaneSeed {
            id: PaneId(id),
            label: "bash".to_owned(),
            title: None,
            notification: None,
            bell_seq: 0,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            dims: (80, 24),
            projection: current,
        }
    }

    /// THE fetch gate: a pane whose projection token has not moved is not re-fetched, and every
    /// other case is. The skip is the whole point of the token; the three fetches are why it is
    /// safe, since each is a case where "unchanged" cannot be established rather than one where it
    /// is known to be false.
    #[test]
    fn only_a_pane_whose_projection_moved_is_refetched() {
        let cache = vec![
            cached(10, Some(token(7))), // unchanged since its frame
            cached(11, Some(token(7))), // moved on
            cached(12, None),           // frame predates the token
        ];
        let seeds = vec![
            seeded(10, Some(token(7))),
            seeded(11, Some(token(8))),
            seeded(12, Some(token(7))),
            seeded(13, Some(token(7))), // a newcomer, with no frame at all
            seeded(14, None),           // the host reported no token
        ];
        assert_eq!(
            stale_panes(&cache, &seeds),
            vec![PaneId(11), PaneId(12), PaneId(13), PaneId(14)],
            "only the pane whose token still matches its frame is skipped",
        );
    }

    /// A pane the wake did NOT re-fetch keeps the token its held frame was taken under — it must
    /// NOT adopt the query's. Adopting it would label an old frame with a new token, and the next
    /// wake would compare equal and skip again, forever: the exact shape of a frozen pane.
    #[test]
    fn a_skipped_pane_keeps_the_token_its_frame_was_taken_under() {
        let existing = vec![cached(10, Some(token(7)))];
        // The host has moved on, but this wake fetched nothing for pane 10.
        let seeds = vec![seeded(10, Some(token(9)))];
        let merged = merge_panes(&existing, &seeds, &[]);
        assert_eq!(
            merged[0].projection,
            Some(token(7)),
            "an unfetched pane keeps the token its frame belongs to",
        );
        // ...so the very next wake still sees it as stale and fetches. Without that, the missed
        // fetch above would be permanent.
        assert_eq!(stale_panes(&merged, &seeds), vec![PaneId(10)]);
    }

    /// A pane the wake DID re-fetch adopts the query's token, which is what lets the next wake
    /// skip it. The pair with the test above is the whole invariant: the stored token is never
    /// newer than the frame it labels.
    #[test]
    fn a_refetched_pane_adopts_the_token_that_came_with_the_query() {
        let existing = vec![cached(10, Some(token(7)))];
        let seeds = vec![seeded(10, Some(token(9)))];
        let merged = merge_panes(&existing, &seeds, &[(PaneId(10), frame(9))]);
        assert_eq!(merged[0].projection, Some(token(9)));
        assert_eq!(
            merged[0].frame.cells.cols(),
            9,
            "and the fetched frame with it"
        );
        assert!(
            stale_panes(&merged, &seeds).is_empty(),
            "so the next wake skips it",
        );
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
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
                frame: frame(3),
                dims: (80, 24),
            },
            WirePane {
                id: PaneId(11),
                label: "cat".to_owned(),
                title: None,
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
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
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
                dims: (100, 30),
            },
            PaneSeed {
                id: PaneId(12),
                label: "vim".to_owned(),
                title: None,
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
                dims: (80, 24),
            },
            PaneSeed {
                id: PaneId(13),
                label: "top".to_owned(),
                title: None,
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
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
            notification: None,
            bell_seq: 0,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            frame: frame(3),
            projection: None,
            dims: (80, 24),
        }];
        let seeds = vec![PaneSeed {
            id: PaneId(10),
            label: "bash".to_owned(),
            title: None,
            notification: None,
            bell_seq: 0,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            projection: None,
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

    /// LIVENESS is host-authoritative and re-adopted each wake, like the title beside it: the
    /// mirror must take the host's answer, not keep the `false` it was born with. Without this a
    /// pane's child could exit and the client would never notice — the exited marker would never
    /// appear, however long the daemon reported it.
    ///
    /// The STATUS rides the same rule and needs it more: it arrives strictly after the liveness bit
    /// (the host reaps only once the child's output has ended), so a mirror that kept its
    /// first-seen value would freeze every pane at "(exited)" and never show a single exit code.
    ///
    /// REVERT-PROOF: pin `dead: false` in `merge_panes` and the exited assertion fails; keep
    /// `existing`'s `child_exit` instead of the seed's and the code assertion fails.
    #[test]
    fn merge_panes_readopts_the_hosts_liveness() {
        let existing = vec![WirePane {
            id: PaneId(10),
            label: "cargo".to_owned(),
            title: None,
            notification: None,
            bell_seq: 0,
            dead: false,      // last wake it was still running
            child_exit: None, // ...so of course nothing had reaped it
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            frame: frame(3),
            projection: None,
            dims: (80, 24),
        }];
        let seeds = vec![PaneSeed {
            id: PaneId(10),
            label: "cargo".to_owned(),
            title: None,
            notification: None,
            bell_seq: 0,
            dead: true, // ...and the host now says the child has exited
            child_exit: Some(PaneExit {
                code: 101, // ...having reaped it, with cargo's own failure code
                signal: None,
            }),
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            projection: None,
            dims: (80, 24),
        }];

        let merged = merge_panes(&existing, &seeds, &[]);
        assert!(
            merged[0].dead,
            "the mirror adopts the host's liveness rather than keeping its own stale view"
        );
        assert_eq!(
            merged[0].child_exit.as_ref().map(|exit| exit.code),
            Some(101),
            "and the status the host learned AFTER that, which is the only way a code ever arrives",
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
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
                frame: frame(3),
                dims: (80, 24),
            },
            WirePane {
                id: PaneId(11),
                label: "bash".to_owned(),
                title: Some("about to be cleared".to_owned()),
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
                frame: frame(3),
                dims: (80, 24),
            },
        ];
        let seeds = vec![
            PaneSeed {
                id: PaneId(10),
                label: "bash".to_owned(),
                title: Some("coin@host:~".to_owned()), // child retitled at the new prompt
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
                dims: (80, 24),
            },
            PaneSeed {
                id: PaneId(11),
                label: "bash".to_owned(),
                title: None, // child cleared its title
                notification: None,
                bell_seq: 0,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                projection: None,
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

    /// [`parse_notification`] maps the `panes` slot's `notification` object to a
    /// [`PaneNotification`], degrading safely: an absent key / `null` / non-object is `None`
    /// (the additive shape — a pane that raised none, or an older daemon), and a garbled `seq`
    /// clamps to `0` so a present-but-broken object never claims to be a fresh notification.
    #[test]
    fn parse_notification_maps_the_wire_object_and_degrades_safely() {
        // A full object.
        let n = parse_notification(&json!({ "title": "Build", "body": "done", "seq": 3 }))
            .expect("a well-formed object parses");
        assert_eq!(n.title.as_deref(), Some("Build"));
        assert_eq!(n.body, "done");
        assert_eq!(n.seq, 3);

        // A body-only (OSC 9) shape: title null.
        let n = parse_notification(&json!({ "title": null, "body": "ping", "seq": 1 }))
            .expect("a body-only object parses");
        assert_eq!(n.title, None);
        assert_eq!(n.body, "ping");

        // Absent / null / non-object ⇒ None (the additive "no notification" shape).
        assert!(parse_notification(&Value::Null).is_none(), "null ⇒ None");
        assert!(
            parse_notification(&json!("nope")).is_none(),
            "a string ⇒ None"
        );

        // A garbled seq clamps to 0 rather than inheriting a stale-looking number.
        let n = parse_notification(&json!({ "body": "x", "seq": "oops" }))
            .expect("a present object still parses");
        assert_eq!(n.seq, 0, "a non-numeric seq clamps to 0");
        assert_eq!(n.title, None, "an absent title is None");
    }

    /// A connected [`HostConn`] whose server RECORDS every request it receives and answers each
    /// with `reply` as the JSON-RPC `result` — used to prove what [`resolve_session`] sends over
    /// the wire (and, on the attach path, that it sends NOTHING). Reads until the client hangs up.
    fn a_recording_host_conn(
        tag: &str,
        reply: &'static str,
    ) -> (HostConn, JoinHandle<()>, SockGuard, Arc<Mutex<Vec<Value>>>) {
        use std::io::Write;
        let path = sock_path(tag);
        let listener = UnixListener::bind(&path).expect("bind the throwaway host socket");
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect to it");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_srv = Arc::clone(&seen);
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept the client");
            let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
            let mut writer = stream;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                let id = request["id"].clone();
                seen_srv.lock().expect("record lock").push(request);
                let response = json!({ "jsonrpc": "2.0", "id": id, "result": reply });
                let _ = writeln!(writer, "{response}");
                let _ = writer.flush();
                line.clear();
            }
        });
        (conn, server, SockGuard(path), seen)
    }

    /// The CREATE path of [`resolve_session`]: with no requested session it sends ONE
    /// `new_session` carrying THIS client's first pane (`cmd`/`cols`/`rows` — tmux's
    /// `new-session -x -y command`), so the birth pane matches and [`boot_panes`] tops up from it.
    /// Proves the GUI actually EMITS the birth spec, which the host-side test can only prove it
    /// accepts.
    #[test]
    fn resolve_session_creates_with_the_clients_first_pane() {
        let (mut conn, server, _guard, seen) = a_recording_host_conn("create", "7");
        let argv = ["vim".to_owned(), "README".to_owned()];
        let (name, created) = resolve_session(&mut conn, None, Some(&argv), 100, 40)
            .expect("resolve_session creates");
        drop(conn); // let the server thread see EOF and exit
        server.join().expect("server thread exited");

        assert_eq!(
            (name.as_str(), created),
            ("7", true),
            "it adopts the allocated name",
        );
        let seen = seen.lock().expect("record lock");
        assert_eq!(seen.len(), 1, "exactly one request — the create — was sent");
        let req = &seen[0];
        assert_eq!(req["method"], "scene/invoke");
        assert_eq!(
            req["params"]["path"],
            json!(mux_action_path(NEW_SESSION_ACTION)),
        );
        assert_eq!(
            req["params"]["args"],
            json!({ "cols": 100, "rows": 40, "cmd": ["vim", "README"] }),
            "the birth spec carries this client's first pane",
        );
    }

    /// The ATTACH path of [`resolve_session`]: a requested session name returns
    /// `(name, created=false)` and sends NOTHING — attach adopts an existing session's live panes
    /// and must NEVER birth a pane (the tmux distinction reattach rests on). REVERT-PROOF: delete
    /// the `if let Some(name) = requested` early return and this fails (a `new_session` is sent,
    /// so `seen` is non-empty).
    #[test]
    fn resolve_session_attaches_without_sending_new_session() {
        let (mut conn, server, _guard, seen) = a_recording_host_conn("attach", "unused");
        let (name, created) = resolve_session(&mut conn, Some("mysession"), None, 80, 24)
            .expect("resolve_session attaches");
        drop(conn);
        server.join().expect("server thread exited");

        assert_eq!(
            (name.as_str(), created),
            ("mysession", false),
            "attach adopts the named session",
        );
        assert!(
            seen.lock().expect("record lock").is_empty(),
            "attach sends no new_session — it must never birth a pane",
        );
    }
}
