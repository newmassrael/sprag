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
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pinion_core::{GridBuffer, QuitSink};
use serde_json::{Value, json};
use sprag_grid::ProjectionToken;
use sprag_host::ClientSize;
use sprag_host::chooser::Target;
use sprag_host::report::Announcement;
use sprag_host::wire::ActivityWire;
use sprag_host::wire::{
    AGENT_MANIFESTS_SLOT, AttachAsk, BREAK_PANE_ACTION, CLIPBOARD_ANSWER_ACTION,
    CLIPBOARD_WRITE_SLOT, CLOSE_ACTION, DROP_FILE_ACTION, ENDED_KEY, FOCUS_ACTION, FULL_TEXT_SLOT,
    GLOBAL_COMMANDS_SLOT, JOIN_PANE_ACTION, JoinAsk, KEY_ACTION, KILL_SESSION_ACTION,
    KILL_WINDOW_ACTION, LAYOUT_SLOT, MOUSE_ACTION, MOVE_PANE_ACTION, MOVE_WINDOW_ACTION,
    MoveWindowAsk, NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANES_SLOT, PASTE_ACTION,
    PROMPT_MARKS_SLOT, RENAME_PANE_ACTION, RENAME_SESSION_ACTION, RENAME_WINDOW_ACTION,
    RESIZE_ACTION, RESIZE_PANE_ACTION, RESIZE_WINDOW_ACTION, ResizeAsk, ResizeHow, ResizeWindowAsk,
    SELECT_PANE_ACTION, SELECT_WINDOW_ACTION, SESSION_ACTIVITY_DISPLAY_MAX_AGE, SESSION_SLOT,
    SESSIONS_SLOT, SET_FLOATING_ACTION, SET_LAYOUT_ACTION, SPAWN_ACTION, SPLIT_ACTION,
    SWAP_PANE_ACTION, SelectAsk, SelectWindowAsk, SwapAsk, SwapHow, TEXT_ACTION, TREE_SLOT,
    WINDOW_SIZE_SLOT, WINDOWS_SLOT, WindowPin, WindowRef, ZOOM_PANE_ACTION, cells_slot_at,
    find_slot_for, project_slot_for, regex_slot_for, session_activity_at,
};
use sprag_host::{
    CellFrame, HostClient, PaneAgent, PaneClipboardQuery, PaneClipboardWrite, PaneFind, PaneFrame,
    PaneNotification, PaneScrollFacts, Project, UserConfig, mux_action_path, pane_input_path,
};
use sprag_input::{Modifiers, MouseInput};
use sprag_rpc::{
    CLIENT_ATTACH_METHOD, CLIENT_MESSAGES_METHOD, CLIENT_SIZE_METHOD, COLS_PARAM, HostConn,
    HostEndpoint, MESSAGE_FIELD, ROWS_PARAM, new_gui_client_id,
};
use sprag_terminal::{
    Ended, LayoutSnapshot, LayoutWire, OrderStep, PaneDir, PaneExit, PaneId, PlaceHow, SessionInfo,
    SplitDir, WindowInfo, WindowPlace, ZoomOutcome,
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

/// Env override: the `sprag-term` binary to spawn (else the sibling of the GUI exe,
/// else `sprag-term` on `PATH`).
const HOST_BIN_ENV: &str = "SPRAG_GUI_HOST_BIN";
/// Env: the SESSION to attach to (adopt its live panes) — the reattach gesture. Absent, the
/// client allocates a fresh session and spawns its own panes into it, so by DEFAULT each launch
/// starts on its own session (the owner's several-windows workflow) — though a running client can
/// [`switch_session`](WireHost::switch_session) to any other from the sidebar. `sprag attach` sets
/// this env; it is the established GUI-config channel (`SPRAG_GUI_PANES`/`_HOST_SOCK`/…).
const SESSION_ENV: &str = "SPRAG_GUI_SESSION";

/// How this client reacts when its OWN attached session is DESTROYED — the tmux
/// `detach-on-destroy` option, now the user's own
/// [`options::DETACH_ON_DESTROY`](sprag_host::options::DETACH_ON_DESTROY). `on` DETACHES this client
/// (the default, and tmux's own); `off` / `next` / `previous` SWITCH it to a neighbouring session
/// instead (tmux's switch-to-next), detaching only when there is no other session to move to;
/// `no-detached` switches only to a session NO OTHER client is viewing, detaching rather than pile a
/// second client onto one another client already holds.
///
/// # ⚠⚠⚠ THE DEFAULT IS THE FRONTEND'S, AND IT USED TO BE THIS FILE'S
///
/// `unset` was `Detach` for every caller, because that is tmux's own default — and it is the WRONG
/// default for a window. Register item 282, measured by the owner with a mouse: closing the attached
/// session QUIT THE WHOLE APP with three other sessions alive. That is not a crash; it is the honest
/// consequence of detaching a client that has nowhere to put an empty screen. **A terminal client
/// detaches to a shell; a window detaches to nothing**, so one constant cannot serve both, and this
/// file is the wrong place to know which is asking.
///
/// The reference is herdr (read at `9a4ce5e1`, `src/app/actions.rs:1665`), whose own comment states
/// the rule this now defaults to: *"Keep focus on the previously focused workspace"* — and which,
/// with none left, keeps the app alive on an empty state rather than ending. ⚠ **sprag has no word
/// for that last part** and cannot get one here: a `WireHost` is scoped to a session at boot, so
/// *no session* is not a state it can hold. Registered rather than half-built.
///
/// # Why this is no longer `SPRAG_DETACH_ON_DESTROY`
///
/// It was an env var read once at boot, and its own note said a runtime `set-option` would write the
/// same enum. Keeping both would put TWO authorities behind one setting, and the CLI could not
/// reconcile them: `sprag show-options` runs in its own process and cannot see the environment of the
/// client it is describing, so it would print a policy that client is not using — the exact failure
/// this front keeps meeting. The env was the temporary channel; the option is the durable one.
fn detach_on_destroy(unset: DetachOnDestroy) -> DetachOnDestroy {
    match sprag_host::config::options() {
        Ok(options) => options
            .get(sprag_host::options::DETACH_ON_DESTROY)
            .map_or(unset, parse_detach_on_destroy),
        Err(error) => {
            tracing::warn!(
                target: "sprag_client::wire",
                %error,
                "using the default destroy policy",
            );
            unset
        }
    }
}

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
    /// The name a PERSON gave the pane, `None` for one nobody named. Host-authoritative and
    /// re-adopted each wake like [`Self::title`] — but the OPPOSITE kind of fact: a name is
    /// chosen by a person and is IDENTITY (unique across the daemon, resolvable back to this
    /// pane), where a title is chosen by the child and is display only. A display surface
    /// therefore prefers this OVER the title.
    name: Option<String>,
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
    /// Whether this is the pane the window is ON — the DAEMON's active pane, which this client's
    /// own focus is a projection of. Host-authoritative and re-adopted each wake like the title:
    /// another client's `select-pane`, or a close handing off, moves it without this client acting.
    /// Exactly one mirrored pane can carry it.
    active: bool,
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
    /// What the AGENT in this pane is doing (H3's `agent` key), `None` for a pane no manifest claims.
    /// Host-authoritative + dynamic like [`Self::notification`] — re-adopted every wake, INCLUDING
    /// back to `None`, because an agent that exits leaves a shell behind and a title still saying
    /// "working" would be the one failure this fact exists to prevent.
    agent: Option<PaneAgent>,
    frame: CellFrame,
    /// The GUI's tracked grid `(cols, rows)`: seeded from the host at boot, advanced only
    /// when a `resize` RPC SUCCEEDS (so the reflow no-op guard reads it with no
    /// round-trip and a failed resize is retried, not latched).
    dims: (u16, u16),
}

/// The wire client's pane data cache: the panes in HOST ORDER, plus the index that makes
/// [`PaneId`] an address rather than a search. Addressed by identity, NOT by display slot —
/// this client speaks the host's language; the GUI's `SlotView` owns slot mapping.
///
/// # Why the index, and why the two halves cannot drift
///
/// The order is load-bearing (`pane_ids` is a membership list a client maps to its own
/// slots), so this is a `Vec` with a side index rather than a map. It used to be the `Vec`
/// alone, and its own doc justified that with "a linear scan over the small pane set" —
/// a premise R264 removed when it lifted the 62-pane ceiling the wire used to impose. Every
/// per-pane accessor paid that scan, and so did the POLL thread's own rebuild
/// ([`merge_panes`], [`stale_panes`]), which is the worse half: that one runs on every scene
/// revision, not merely on every paint.
///
/// Both fields are PRIVATE and the only way to build one is [`PaneCache::new`], which
/// derives the index from the panes it is given. A cache whose index disagrees with its
/// panes is therefore not a bug to test for — it is a value that cannot be constructed.
struct PaneCache {
    panes: Vec<WirePane>,
    index: HashMap<PaneId, usize>,
    agents_generation: u64,
}

impl PaneCache {
    /// Adopt `panes` (already in host order) and derive the index from them.
    fn new(panes: Vec<WirePane>) -> Self {
        Self {
            index: Self::index_of(&panes),
            panes,
            agents_generation: 0,
        }
    }

    fn index_of(panes: &[WirePane]) -> HashMap<PaneId, usize> {
        panes
            .iter()
            .enumerate()
            .map(|(at, pane)| (pane.id, at))
            .collect()
    }

    /// Take `panes` as the new contents, re-derive the index, and move the agent generation if
    /// the agent projection moved with them.
    ///
    /// The ONE way the contents change, which is what lets [`Self::agents_generation`] mean
    /// anything at all. `merge_panes` builds the replacement; owning the swap here is what keeps a
    /// caller from installing contents without declaring what moved.
    fn replace(&mut self, panes: Vec<WirePane>) {
        if !Self::same_agents(&self.panes, &panes) {
            self.agents_generation += 1;
        }
        self.index = Self::index_of(&panes);
        self.panes = panes;
    }

    /// Whether two contents carry the SAME agent projection — the ids in order and each one's
    /// verdict.
    ///
    /// Deliberately the projection [`WireHost::pane_agents`] returns and not a whole-contents
    /// comparison. The first version of this counted every replacement, which is complete but
    /// USELESS: `refresh_to_set` replaces the contents on every wake, and a wake is what a pane
    /// echoing a keystroke causes, so during the typing this was meant to make cheap the token
    /// would have moved on every single paint. A mechanism that is unit-green and inert in the
    /// binary is the failure this project keeps recording, so the token tracks what its reader
    /// reads.
    fn same_agents(before: &[WirePane], after: &[WirePane]) -> bool {
        before.len() == after.len()
            && before
                .iter()
                .zip(after)
                .all(|(was, now)| was.id == now.id && was.agent == now.agent)
    }

    /// How many times the AGENT projection has changed — the token a reader of
    /// [`WireHost::pane_agents`] keys a derived answer on.
    ///
    /// It cannot go stale for that reader, because the comparison behind it is over exactly the
    /// fields that reader returns and the two are defined together: extend one and the other stops
    /// compiling honestly. What it can do is move when a reader would have been happy — a verdict
    /// changing `seq` and nothing else, say — which costs a recomputation and never a wrong answer.
    fn agents_generation(&self) -> u64 {
        self.agents_generation
    }

    /// Pane `id`, or `None` if this cache does not hold it.
    fn get(&self, id: PaneId) -> Option<&WirePane> {
        self.index.get(&id).map(|at| &self.panes[*at])
    }

    /// Latch pane `id`'s tracked dimensions — the one mutation a reader performs, and the reason
    /// this is not a general `get_mut`.
    ///
    /// Handing out `&mut WirePane` would let a caller write the `agent` field, and then
    /// [`Self::agents_generation`] would be a promise rather than a property. Narrowing the write
    /// to the field it is actually for makes that unrepresentable instead of forbidden. Answers
    /// whether the pane was there, since a resize can land after its pane closed.
    fn set_dims(&mut self, id: PaneId, dims: (u16, u16)) -> bool {
        match self.index.get(&id) {
            Some(at) => {
                self.panes[*at].dims = dims;
                true
            }
            None => false,
        }
    }

    /// Every pane, in host order.
    fn panes(&self) -> &[WirePane] {
        &self.panes
    }
}

impl Default for PaneCache {
    fn default() -> Self {
        Self::new(Vec::new())
    }
}

/// The shared handle: one cache between the UI thread (reads / input / resize) and the poll
/// thread (frame refresh), under one lock.
type Cache = Arc<Mutex<PaneCache>>;

/// Lock the shared pane cache, poison-tolerant — the ONE definition of the cache's lock
/// discipline, shared by the UI thread ([`WireHost::lock_cache`]) and the poll thread.
fn lock_cache(cache: &Mutex<PaneCache>) -> MutexGuard<'_, PaneCache> {
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
    /// The session's arbitrated window size (tmux `window-size`), or `None` while no attached
    /// client has reported an area.
    ///
    /// Kept HERE rather than in a mirror of its own because it is read WITH the arrangement and
    /// never without it: tiling is a function of both, so the poll stores them under one lock and a
    /// reader takes them from one place. Not revision-guarded like the layout — it carries no
    /// revision, being derived from every attached client's report rather than from a versioned
    /// document, so the newest read simply wins.
    window_size: Option<(u16, u16)>,
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

/// Read the slot at `path` and decode it, NAMING the slot when the decode fails.
///
/// `HostConn::call` already names a failed request. This is the other half, and the two failures
/// are opposite facts that used to read identically: `invalid type: integer 0, expected string or
/// map` says the same thing whether the daemon REFUSED the request or ANSWERED it with a shape
/// this client's types do not describe. The second is a version skew between a client and a
/// daemon — the failure a mixed-version machine actually has — and naming the slot is what turns
/// it from a bisect into a sentence.
///
/// One helper rather than the four hand-rolled `from_value(..).map_err(..)` copies it replaces:
/// they were identical except for the type, which is exactly the shape where the fifth copy
/// forgets the `map_err` and re-opens the hole.
/// # The THIRD failure it names: a daemon that does not have the slot at all
///
/// A slot is ADDITIVE, so `WIRE_PROTOCOL` does not rise when one is added and this client meets
/// same-numbered daemons that lack an address it reads. Measured by running, against a peer that
/// passes the handshake and serves nothing, `sprag-tui` exited at boot with
/// `scene/query /sprag_mux/external/panes: host rpc error: UnknownIntrospectPath` — a Rust enum
/// variant at an operator, which is the class R283/R290/R321/R322 have been removing verb by verb.
///
/// Exiting is the RIGHT shape (a display client with no panes to paint has nothing to do); only the
/// sentence was wrong. It is the daemon's own vocabulary now, shared with the `sprag` CLI and the
/// agent surface, so the three cannot describe one situation three ways.
fn read_slot<T: serde::de::DeserializeOwned>(conn: &mut HostConn, path: String) -> io::Result<T> {
    let value = query_slot(conn, &path)?;
    serde_json::from_value(value).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{path} answered a shape this client cannot read: {error}"),
        )
    })
}

/// One BOOT read of one slot, with the refusal a daemon that lacks the address sends turned into
/// the sentence it means.
///
/// The ONE place this crate spells the query method for a boot read, and it exists because there
/// were TWO: [`read_slot`] and `query_panes` each called `HostConn::call` directly, and the fix
/// applied to the first changed nothing a person sees, because the read that actually fails first
/// is the second one. [`read_slot`]'s own doc warned about exactly that shape — *"the fifth copy
/// forgets the `map_err` and re-opens the hole"* — one function above the copy that did it.
fn query_slot(conn: &mut HostConn, path: &str) -> io::Result<Value> {
    let params = json!({ "path": path });
    // Rendered exactly as `HostConn::call` would have, through the same function it uses, so every
    // other failure this boot has ever printed is byte-identical.
    let label = sprag_rpc::request_label("scene/query", &params);
    conn.try_call("scene/query", params)
        .map_err(|error| match error {
            sprag_rpc::CallError::Fault(fault) => sprag_host::wire::unknown_slot(path, &fault)
                .unwrap_or_else(|| io::Error::other(format!("{label}: {fault}"))),
            sprag_rpc::CallError::Transport(error) => error,
        })
}

/// Read the host's arrangement off the wire — the ONE place the `layout` slot is queried,
/// shared by the boot read and the poll thread's refresh.
fn query_layout(conn: &mut HostConn) -> io::Result<LayoutSnapshot> {
    read_slot(conn, mux_action_path(LAYOUT_SLOT))
}

/// Read the session's arbitrated window size off the wire — the ONE place the `window_size` slot is
/// queried, shared by the boot read, the poll thread's refresh, and a switch's re-boot.
///
/// A slot that answers `null` (no client has reported an area, or a host that tracks none) decodes
/// to `None`, which is a value rather than a failure: the caller falls back to its own surface.
fn query_window_size(conn: &mut HostConn) -> io::Result<Option<(u16, u16)>> {
    let size: Option<ClientSize> = read_slot(conn, mux_action_path(WINDOW_SIZE_SLOT))?;
    Ok(size.map(|size| (size.cols, size.rows)))
}

/// Store the arbitrated window in the mirror. Unguarded (see [`Mirrored::window_size`]).
fn store_window_size(layout: &Mutex<Mirrored>, size: Option<(u16, u16)>) {
    lock_layout(layout).window_size = size;
}

/// Report this client's own cell area to the daemon (`client/size`) — the input its `window-size`
/// policy arbitrates over.
///
/// BEST-EFFORT, and it stays that way now that [`shake_hands`] is not: a daemon that does not know
/// this method leaves this client out of the arbitration, which degrades to the behaviour that
/// predates it (the window is whatever the clients that DO report agree on, or nothing at all and
/// every client uses its own surface). Losing a vote in a size arbitration costs a size; reading a
/// daemon whose wire shape you do not share costs correctness, which is why only one of the two is
/// fatal.
fn send_size(conn: &mut HostConn, cols: u16, rows: u16) {
    if let Err(error) = conn.call(
        CLIENT_SIZE_METHOD,
        json!({ COLS_PARAM: cols, ROWS_PARAM: rows }),
    ) {
        tracing::debug!(target: "sprag_gui::wire", %error, "client/size failed; this client is not arbitrated over");
    }
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
    read_slot(conn, mux_action_path(WINDOWS_SLOT))
}

/// Every session on the host, mirrored — what a session SWITCHER draws (a vertical rail of every
/// session, the current one highlighted). Registry-WIDE, not scoped: it is the `sessions` slot,
/// whose subject is the SET of sessions, so it is read the same over any client's scoped conn.
/// Shared between the UI thread (which reads it to paint the sidebar) and the poll thread (which
/// re-reads it whenever the scene moves — a new / killed session bumps the revision), under one
/// lock. Mirrored, not fetched on demand, for the same reason the windows list is: the paint path
/// must make no socket call.
type SessionsMirror = Arc<Mutex<Sessions>>;

/// The mirrored session list, plus WHERE THIS CLIENT'S OWN ROW STOOD in it.
///
/// # ⚠⚠ Why a list alone could not answer the question a destroy asks it
///
/// [`list_neighbour`] counts from the row that died, and until R367 the only thing holding that row
/// was the list itself — so the walk worked exactly as long as the mirror had not been refreshed
/// past it. It is refreshed at the END of every wake, by a registry-wide read that does NOT fail
/// when this client's session dies (that is [`store_sessions`]' whole point, and it is deliberate).
/// So a kill landing between a wake's scoped reads and its sessions re-read leaves the mirror
/// holding the survivors and NOT the row the person was standing on, and the next wake's refusal
/// then asks for a neighbour of a row nothing remembers.
///
/// R345 measured the harm that answer causes and named it: **a client detaching past a live session
/// throws a person out of the multiplexer**. It fixed the mirror being too STALE to see the
/// survivor; this is the same harm through the opposite door — the mirror too FRESH to see the
/// anchor — and it reached CI as the same unattributable 45-second timeout.
///
/// The anchor is the fix because it is the one fact that cannot be re-derived: the survivors are
/// still readable from the daemon at any time, and where a destroyed row STOOD is readable from
/// nowhere once the list has moved on. A client knows it continuously while it is attached, which
/// is exactly when it costs nothing to record.
#[derive(Debug, Default)]
struct Sessions {
    /// The list as of the last read — what a switcher sidebar draws.
    list: Vec<SessionInfo>,
    /// The index THIS CLIENT'S OWN session held in the last list that still carried it, or `None`
    /// for a client that has never seen itself in one.
    ///
    /// Kept across a refresh that drops the row, and only across that: any list still holding this
    /// client's session re-derives it, so a switch moves it and a rename cannot strand it. `None`
    /// is left honest rather than defaulted to zero — *"this client never had a place"* and *"it
    /// stood at the top"* are different claims, and only the second one may be counted from.
    anchor: Option<usize>,
}

/// Lock the mirrored session list, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_sessions(sessions: &Mutex<Sessions>) -> MutexGuard<'_, Sessions> {
    sessions.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replace the mirrored session list — the ONE place it is written, shared by the poll thread and
/// a switch's own re-boot. Unconditional (like [`store_windows`]): the list carries no revision,
/// so any brief backward move heals on the next wake.
///
/// `viewing` is the session this client is attached to, and it is what makes the write more than an
/// assignment: a list that still holds that name re-derives the [`Sessions::anchor`], and a list
/// that has dropped it KEEPS the one already there. The asymmetry is the whole point — a
/// registry-wide read succeeds while this client's own session is being destroyed, so the refresh
/// that erases the row is exactly the refresh that must not erase the memory of where it was.
fn store_sessions(sessions: &Mutex<Sessions>, list: Vec<SessionInfo>, viewing: &str) {
    let mut held = lock_sessions(sessions);
    if let Some(at) = list.iter().position(|session| session.name == viewing) {
        held.anchor = Some(at);
    }
    held.list = list;
}

/// Read every session off the wire (the registry-wide `sessions` slot) — the ONE place it is
/// queried, shared by the boot read, the poll thread's refresh, and a switch's re-boot.
fn query_sessions(conn: &mut HostConn) -> io::Result<Vec<SessionInfo>> {
    read_slot(conn, mux_action_path(SESSIONS_SLOT))
}

/// The last activity reading this client has, and WHEN it landed here — the two halves of an honest
/// age (R282).
///
/// The daemon's `sampled_ms_ago` is how old the sample was when it was ANSWERED; it keeps ageing
/// while it sits in this mirror. Storing the arrival instant beside it is what lets
/// [`HostClient::session_activity`] add the difference back in, so a client that has been parked for
/// a minute reports a minute-old subtitle rather than the one-second-old one it was handed.
///
/// Dropping the arrival instant and reporting the daemon's number alone would be the quiet kind of
/// wrong: every reading would look fresh, and the age would be decoration rather than a fact.
struct ActivityMirrorEntry {
    reading: sprag_terminal::ActivityReading,
    arrived: Instant,
}

/// Every session's live activity, mirrored — the sidebar's subtitle line. Filled by the poll thread
/// and read by the UI thread, like the session list beside it, and for the same reason: the paint
/// path must make no socket call.
type ActivityMirror = Arc<Mutex<Option<ActivityMirrorEntry>>>;

/// Lock the mirrored activity, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_activity(
    activity: &Mutex<Option<ActivityMirrorEntry>>,
) -> MutexGuard<'_, Option<ActivityMirrorEntry>> {
    activity.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Replace the mirrored activity, stamping its arrival — the ONE place it is written.
fn store_activity(
    activity: &Mutex<Option<ActivityMirrorEntry>>,
    reading: sprag_terminal::ActivityReading,
) {
    *lock_activity(activity) = Some(ActivityMirrorEntry {
        reading,
        arrived: Instant::now(),
    });
}

/// Read every session's ACTIVITY off the wire, accepting an answer up to
/// [`SESSION_ACTIVITY_DISPLAY_MAX_AGE`] old — the ONE place the family is queried.
///
/// The tolerance is what keeps this call cheap on the wake it rides. The poll thread wakes on every
/// batch of PTY output, and the facts here move at human pace, so asking the daemon to re-walk
/// `/proc` for each of them would be paying a keystroke's worth of latency for an answer that has
/// not changed. The daemon answers from its held sample instead, and says how old it is.
fn query_activity(conn: &mut HostConn) -> io::Result<sprag_terminal::ActivityReading> {
    let max_age = u64::try_from(SESSION_ACTIVITY_DISPLAY_MAX_AGE.as_millis()).unwrap_or(u64::MAX);
    let wire: ActivityWire = read_slot(conn, mux_action_path(&session_activity_at(max_age)))?;
    Ok(sprag_terminal::ActivityReading {
        age: Duration::from_millis(wire.sampled_ms_ago),
        value: wire.sessions,
    })
}

/// The NAME of the session this client is viewing, mirrored — what it PAINTS where its scope is now
/// an attachment rather than a name. Shared between the poll thread (which refreshes it) and the
/// paint path (which reads it), under one lock, like every other mirror here.
type SessionMirror = Arc<Mutex<String>>;

/// Lock the mirrored session name, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_session(session: &Mutex<String>) -> MutexGuard<'_, String> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Read the name of the session this connection's requests are scoped to — the ONE place
/// [`SESSION_SLOT`] is queried, shared by the boot read, the poll thread's refresh and a switch's
/// re-boot.
fn query_session(conn: &mut HostConn) -> io::Result<String> {
    read_slot(conn, mux_action_path(SESSION_SLOT))
}

/// Replace the mirrored session name — the ONE place it is written.
///
/// Unconditional, like [`store_windows`]: the name carries no revision, so a brief backward move
/// (two wakes racing a rename) heals on the next one, and the alternative — guessing which of two
/// answers is newer — is how a mirror comes to hold a name the daemon never had.
fn store_session(session: &SessionMirror, name: String) {
    *lock_session(session) = name;
}

/// The one message the daemon has handed this client and its surface has not yet shown, mirrored —
/// filled by the poll thread's collection and EMPTIED by the paint path's
/// [`take_message`](sprag_host::wake::WakeSource::take_message).
///
/// The same shape every other mirror here has, and it holds ONE value for the reason the daemon's
/// own mailbox does: two undelivered messages are resolved by [`Announcement::over`], not queued, so
/// there is no capacity to exceed and nothing to discard by a rule nobody wrote down.
type MessageMirror = Arc<Mutex<Option<Announcement>>>;

/// Lock the mirrored message, poison-tolerant (see [`lock_cache`] for the discipline).
fn lock_message(message: &Mutex<Option<Announcement>>) -> MutexGuard<'_, Option<Announcement>> {
    message.lock().unwrap_or_else(PoisonError::into_inner)
}

/// COLLECT whatever the daemon is holding for this connection's client — the ONE place
/// [`CLIENT_MESSAGES_METHOD`] is called.
///
/// # Why it rides the poll thread's wake and not the paint path
///
/// A paint happens whenever a pane produces output, and a synchronous round trip on that path would
/// be a new cost class in the loop this client's whole idle cost rests on. The poll thread already
/// makes four reads per wake (the session name, the window list, the pane set, the layout) and it
/// runs on its own connection, parked host-side while the scene is quiet — so this is a fifth read
/// on a path that is already paid for, and it happens exactly when the daemon says something moved.
///
/// There is no timer here and none is wanted: a client watching a quiet session is not woken, and a
/// message to it arrives on the next wake it gets — measured at roughly 150 ms end to end. **Which
/// wake that is has NOT been attributed**: the daemon bumps the delivered-into session's channel,
/// and deleting that bump leaves a settled cross-session fixture green. Recorded here rather than
/// claimed, because a mechanism sentence in a durable comment is a claim like any other.
fn query_message(conn: &mut HostConn) -> io::Result<Option<Announcement>> {
    let answer = conn
        .call(CLIENT_MESSAGES_METHOD, json!({}))
        .map_err(io::Error::other)?;
    match answer.get(MESSAGE_FIELD) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(io::Error::other),
    }
}

/// Put `collected` in front of whatever this client has not yet shown — the ONE place the mirror is
/// written.
///
/// [`Announcement::over`] and not a plain replace: a note arriving while an alert is still waiting to
/// be painted must not displace it, which is the row's own rule asked one step earlier. The daemon
/// applies the same rule to its own slot, through the same function, so a message cannot win here
/// and lose there.
fn store_message(mirror: &MessageMirror, collected: Announcement) {
    let mut held = lock_message(mirror);
    let waiting = held.take();
    *held = Some(collected.over(waiting));
}

/// Announce `conn`'s CLIENT id to the daemon and agree on the wire's SHAPE
/// ([`HostConn::handshake`]) — the group key a client's several connections share, plus the check
/// that this build and that daemon speak the same wire.
///
/// It used to be best-effort, because attachment drove only the sidebar's viewer badge. It is
/// FATAL now, and the change of status is the point: the same call now carries a fact that decides
/// whether anything else this client reads means what it says. A daemon that cannot answer it is
/// either older than R-PR67 or older than the shape — and in both cases going on to paint from its
/// replies is how R278's boot came to die nine requests later with a message about an integer.
fn shake_hands(conn: &mut HostConn, client_id: &str) -> io::Result<()> {
    conn.handshake(client_id)
}

/// What a `client/attach` did — the answer, which is the session this client is now attached to.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Landed {
    /// Attached, to the session the daemon NAMED. For a [`Attaching::Named`] ask that is the name
    /// asked with; for [`Attaching::LastViewed`] it is the answer, and the only way to learn it.
    On(String),
    /// A history ask found nowhere to go back to. Nothing moved, on either side.
    Nowhere,
    /// The daemon refused the attach or could not be reached. Nothing moved.
    Refused,
}

/// Declare (or switch — tmux `switch-client`) this client's ATTACHED session (`client/attach`,
/// R-PR67), and report where it LANDED.
///
/// `ask` is the target grammar ([`AttachAsk`]): absent means the session `conn` is scoped to, which
/// is how every attach before R304 named one; [`AttachAsk::LastViewed`] asks the daemon to resolve
/// the session this client was viewing before. The daemon answers the session's CURRENT NAME either
/// way — so a caller reads where it went rather than assuming it, which is the only shape the
/// history ask can have and is a better one for the named ask too (R295's rule: the recorded name,
/// never the argument).
///
/// Still BEST-EFFORT — like [`send_size`] and unlike [`shake_hands`] — but the answer is no longer
/// discardable, and that is what the return value is for. A successful attach is what makes
/// [`HostConn::scope_to_attached`] legal on this client's connections; without one they must keep
/// scoping by NAME, which is a worse address ([`scope_to_view`]) but the only one that still works.
/// So a daemon that refuses the attach costs a viewer count and a rename this client can follow —
/// not a correct reading.
fn send_attach(conn: &mut HostConn, ask: AttachAsk) -> Landed {
    let mut params = serde_json::Map::new();
    ask.write_into(&mut params);
    match conn.call(CLIENT_ATTACH_METHOD, Value::Object(params)) {
        // A NAME is the daemon saying where this client now is. `null` is the history ask answering
        // that there is nowhere to go back to — a state, not a failure, so it is not logged as one.
        Ok(Value::String(session)) => Landed::On(session),
        Ok(Value::Null) => Landed::Nowhere,
        // A daemon that answered something else is one this client cannot follow. It is not a
        // refusal (the request was taken) but it is not a landing either, and guessing which
        // session it meant is the whole failure this round is about.
        Ok(other) => {
            tracing::debug!(
                target: "sprag_gui::wire",
                answer = %other,
                "client/attach answered no session name; treating it as a refusal",
            );
            Landed::Refused
        }
        Err(error) => {
            tracing::debug!(
                target: "sprag_gui::wire",
                %error,
                "client/attach failed; viewer badge disabled and this client's reads stay \
                 name-scoped, so a rename of its session will detach it",
            );
            Landed::Refused
        }
    }
}

/// Which session a client is asking to be moved to — the ways it can name one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Attaching<'a> {
    /// A session by NAME: the sidebar's row, a `-t` target, a session just created.
    Named(&'a str),
    /// One step along the daemon's session order from wherever this client is — tmux
    /// `switch-client -n` / `-p` (R314).
    ///
    /// Like [`LastViewed`](Self::LastViewed) and unlike [`Named`](Self::Named), the client sends a
    /// DIRECTION and reads back a name. It could resolve this itself off its `sessions` mirror, and
    /// that is exactly the second answer the daemon's walk exists to prevent: the mirror is a poll
    /// behind, so a client that stepped in it and then attached BY NAME would aim at a row that may
    /// have moved — R304's defect, reached by a different route. See
    /// [`sprag_host::wire::AttachAsk::Step`].
    Step(OrderStep),
    /// The row a CHOOSER picked, as a path of IDENTITIES — R315.
    ///
    /// The strongest form of the argument the two arms above make. There the client cannot name its
    /// target; here it CAN — the row it painted has a label on it — and must not, because the label
    /// was true when the list was drawn and a person has been reading it since. See
    /// [`AttachAsk::Goto`].
    Goto(Target),
    /// The session this client was viewing BEFORE this one, resolved by the daemon (tmux
    /// `switch-client -l`). The client cannot name it, and R304 measured what happens when it
    /// tries — see [`AttachAsk::LastViewed`].
    LastViewed {
        /// Restrict it to a session no OTHER client is viewing (tmux `no-detached`).
        unattached: bool,
    },
    /// Wherever this client ALREADY is — its own attachment, by identity.
    ///
    /// For resuming rather than switching: a gesture that turned out to have nowhere to go has
    /// stopped this client's poll thread and has to start one again over the session it never left.
    /// Naming that session would be this round's own defect in its recovery path — the name is a
    /// mirror, and a rename in the instant before the gesture would make the resume FAIL and take
    /// the client down with it. An attachment cannot go stale that way, and the daemon answers what
    /// the session is called now.
    ///
    /// It is deliberately NOT how a SWITCH recovers: after a failed switch the attachment may
    /// already have moved to the target, and the client wants the session it was on — which only a
    /// name can say.
    Attached,
}

/// Attach `conn`'s client where `to` says and then move `conn`'s own scope onto the ATTACHMENT —
/// the ONE sequence a display client's connection goes through, at boot and at every switch, so
/// neither can drift from the other. Answers where it landed.
///
/// The order is the whole content: you must AIM an attachment at something (a pointer has to point
/// somewhere), and from then on the name is the wrong address to keep sending. A `rename-session`
/// retires it — the daemon then refuses this client, which reads the refusal as "my session is
/// gone" and leaves a session that is alive; and once a NEW session takes the freed name, the same
/// read SUCCEEDS against a stranger's panes. Both measured at R303.
///
/// [`Attaching::Named`] scopes `conn` to the name first, because a named attach IS the scope's:
/// the daemon takes the connection's scope as the target when nothing else names one.
/// [`Attaching::LastViewed`] names its own target, so it needs no scope of its own and never
/// re-points one — a client asking to go back must not have to say where it currently is.
///
/// [`Landed::Refused`] leaves `conn` NAME-scoped on the named arm, deliberately: that is what this
/// client did before the attached scope existed, so a daemon that cannot attach degrades to the old
/// behaviour rather than to no behaviour.
fn attach_and_follow(conn: &mut HostConn, to: Attaching<'_>) -> Landed {
    let landed = match to {
        Attaching::Named(session) => {
            conn.scope_to(session);
            send_attach(conn, AttachAsk::Scoped)
        }
        Attaching::LastViewed { unattached } => {
            send_attach(conn, AttachAsk::LastViewed { unattached })
        }
        // Names its own target, so like the history arm it needs no scope of its own — a client
        // asking for "the next one" must not have to say where it currently is, and saying so
        // would be a name where the daemon already holds an identity.
        Attaching::Step(step) => send_attach(conn, AttachAsk::Step(step)),
        // The same, with a target the client DOES hold and deliberately does not send by name.
        Attaching::Goto(target) => send_attach(
            conn,
            AttachAsk::Goto {
                session: target.session(),
                window: target.window(),
                pane: target.pane(),
            },
        ),
        // The scope IS the target here: the daemon reads `{"attached": true}` as this client's own
        // attachment and re-attaches it to itself, which is a no-op that answers the one thing the
        // caller needs — what that session is called now.
        Attaching::Attached => {
            conn.scope_to_attached();
            send_attach(conn, AttachAsk::Scoped)
        }
    };
    if matches!(landed, Landed::On(_)) {
        conn.scope_to_attached();
    }
    landed
}

/// Scope `conn` the way this client's OTHER connections are scoped — to the attachment when this
/// client has one, else to `session` by name.
///
/// A client's several connections must address ONE session, and only the request connection sends
/// `client/attach`; the rest inherit the attachment through the `conn -> client -> session` map by
/// saying hello with the same client id. So the choice here is not "does THIS connection have an
/// attachment" but "did this CLIENT attach", which is what `attached` carries — and getting that
/// wrong in either direction is a poll thread reading a different session than the paint path.
fn scope_to_view(conn: &mut HostConn, session: &str, attached: bool) {
    if attached {
        conn.scope_to_attached();
    } else {
        conn.scope_to(session);
    }
}

/// **WHICH KIND OF FRONTEND IS BOOTING** — for the decisions whose right answer differs between a
/// window and a terminal, and for nothing else.
///
/// # ⚠⚠⚠ Why this exists rather than one constant
///
/// Register item 282: closing the attached session QUIT THE WHOLE APP while three other sessions
/// were alive. The policy behind it (`DetachOnDestroy`, private to this module) defaulted to tmux's
/// `on` for every caller,
/// and that default is right for exactly one of the two frontends this crate serves. **A terminal
/// client that detaches hands the person back their shell; a window that detaches has nothing to
/// draw and ends.** Same policy, same code, opposite outcome — so the default is a fact about the
/// caller, and a caller is the only party that can state it.
///
/// ⚠ It is deliberately NOT *"is there a GPU"* or *"is this sprag-gui"*. What the decision turns on
/// is whether detaching leaves the person somewhere, and a future frontend answers that about
/// itself rather than being recognised by name here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Frontend {
    /// A WINDOW. Detaching leaves an empty frame nobody asked for, so its destroy default prefers a
    /// surviving session. The reference is herdr (`src/app/actions.rs:1665` at `9a4ce5e1`): *"Keep
    /// focus on the previously focused workspace"*.
    Window,
    /// A TERMINAL client. Detaching returns the person to the shell they launched from, which is
    /// where tmux's own `on` default is correct and why it stays.
    Terminal,
}

impl Frontend {
    /// This frontend's `DetachOnDestroy` when the user has set none.
    ///
    /// ⚠ The type is spelled rather than LINKED: it is private to this module and this method is
    /// reachable from a public one, so an intra-doc link here is `private_intra_doc_links` under
    /// `-D warnings`. Measured, not styled — it refused a commit.
    ///
    /// ⚠ `Off` rather than `Next` for the window: `Off` prefers the session this client was LAST
    /// VIEWING and falls back to the list neighbour, which is herdr's rule in both halves — it
    /// re-finds the previously focused workspace by id, and only when that is gone does it take
    /// whatever occupies the index.
    const fn unset_destroy_policy(self) -> DetachOnDestroy {
        match self {
            Self::Window => DetachOnDestroy::Off,
            Self::Terminal => DetachOnDestroy::Detach,
        }
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
///
/// ⚠⚠⚠⚠ **THIS DOC BELONGS TO THIS ENUM AND WAS SEPARATED FROM IT ONCE**, on 2026-08-17, by a
/// supervising session that inserted [`Frontend`] between the two. Rust then read the whole block as
/// `Frontend`'s: `Self::Detach` stopped resolving and a public item was documenting a link to a
/// private one, which `-D warnings` refuses. It cost the loop sharing this tree a refused commit.
/// **An item goes above a doc block, never between one and the item it describes.**
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

/// Parse one [`options::DETACH_ON_DESTROY_VALUES`](sprag_host::options::DETACH_ON_DESTROY_VALUES)
/// name into a [`DetachOnDestroy`] — a pure decision over its input, kept out of
/// [`detach_on_destroy`]'s config read the way `resolve_session` is kept out of the env reads.
///
/// The option table has already refused anything outside that vocabulary, so the fallback arm is not
/// the validation — it is what keeps this total. It answers [`Detach`](DetachOnDestroy::Detach), the
/// safe default: an unrecognised policy detaches rather than silently switching a client somewhere it
/// never asked to go. A value the TABLE offers and this does not translate would be a setting nothing
/// performs, which is why `every_offered_policy_is_one_this_client_performs` exists.
fn parse_detach_on_destroy(value: &str) -> DetachOnDestroy {
    match value {
        "off" => DetachOnDestroy::Off,
        "no-detached" => DetachOnDestroy::NoDetached,
        "next" => DetachOnDestroy::Next,
        "previous" => DetachOnDestroy::Previous,
        _ => DetachOnDestroy::Detach,
    }
}

/// Where a client goes when its own attached session is destroyed — the resolved tmux
/// `detach-on-destroy` decision, as far as the CLIENT can resolve it.
///
/// [`LastViewed`](Self::LastViewed) is why this is a type rather than an `Option<String>`: the two
/// MRU-preferring policies ask a question only the daemon can answer, so the answer is a PLAN
/// (ask, and here is what to do if the answer is "nowhere") rather than a name.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Successor {
    /// LEAVE. tmux `detach-on-destroy on`, or a switch policy with nowhere to go.
    Detach,
    /// Attach to this session by NAME — the `next`/`previous` list neighbour.
    Named(String),
    /// Ask the daemon for the session this client was viewing before ([`AttachAsk::LastViewed`]),
    /// and fall back to `fallback` (or DETACH, when it is `None`) if it answers that there is none.
    ///
    /// The pick itself is deliberately NOT made here. A client that walked its own remembered names
    /// for it is exactly R304's defect: after a rename the entry resolves to nothing and the visit
    /// is lost, and once a new session takes the freed name it resolves to a STRANGER — so a
    /// destroyed session would dump its client onto somebody else's work.
    LastViewed {
        /// Restrict the answer to a session no OTHER client is viewing (tmux `no-detached`).
        unattached: bool,
        /// Where to go when the client has viewed nothing else that survives.
        fallback: Option<String>,
    },
}

/// The nearest session `step` places from `killed` in the order the person SAW that the daemon
/// still serves NOW — or `None` when there is no such session to name.
///
/// # Two lists, because the anchor and the candidates are different questions
///
/// The order is the SIDEBAR's — the registry's own creation order, a stable `Vec` — so `next` moves
/// to the row visually below and `previous` above. tmux orders by session NAME because it has no
/// visible list; sprag has one, so its visible order is the more intuitive, honest analog. Only
/// `seen` can supply that order, because it is the only list that still holds `killed`, the row the
/// walk counts FROM (R326: deciding on a fresh read alone leaves `next` with no anchor at all).
///
/// **But a row in `seen` is not a session that exists.** The mirror is refreshed by the poll thread
/// on a wake, and [`first_free_other`] has recorded since R327 that nothing bounds how stale it
/// gets; the counts were moved onto a fresh read then and the MEMBERSHIP was not. So the walk takes
/// its order from `seen` and its CANDIDATES from `now`, in both directions:
///
/// * a session `now` has and `seen` does not is appended to the order and can be landed on — it was
///   created since this client last looked, and detaching a person past a live session is the worse
///   answer by a distance (R345: measured doing exactly that, on `off`, `next` and `previous`);
/// * a name `seen` still holds and `now` does not is SKIPPED rather than named — following it is an
///   attach that fails, and a failed follow detaches, so a corpse in the mirror costs the person
///   the live session behind it.
///
/// ## ⚠⚠⚠ And a mirror refreshed PAST the row is not a mirror without one — R367
///
/// The paragraph above says only `seen` can supply the anchor, and that was read for two rounds as
/// *"the list either holds `killed` or nothing does"*. It does not follow. The mirror is rewritten
/// at the end of every wake by a read that keeps succeeding while this client's own session dies
/// ([`Sessions`] carries the measurement), so the list can lose the row while the client is still
/// standing on it — and the answer this function gave then was a DETACH, past whatever survivors
/// the daemon was serving. That is R345's harm exactly, reached from the other side.
///
/// So the anchor is taken from the mirror's [`Sessions::anchor`] when the row itself is gone, and
/// the row is SPLICED BACK where it stood before the walk runs. Putting it back rather than
/// special-casing the walk is what keeps the two cases one algorithm: `next` is still the row after
/// it and `previous` still the row before it, wrapping identically, and the `alive` filter still
/// refuses to name it because `now` does not hold it. A walk taught to step from a GAP would have to decide
/// what `next` means with nothing there, and that decision is exactly the one the splice makes
/// once, visibly.
///
/// `None` is left for the cases that genuinely have no answer: `killed` is in neither the order nor
/// the remembered anchor (a client that never saw itself in a list — nothing to count from), or the
/// walk finds no survivor. Both are honest detaches.
///
/// The step loop stops one short of a full lap, so it can never name `killed` itself — which also
/// means the same-list callers (planning a kill they are about to perform, where `now` still holds
/// `killed`) are unaffected.
fn list_neighbour(
    seen: &[SessionInfo],
    anchor: Option<usize>,
    now: &[SessionInfo],
    killed: &str,
    step: isize,
) -> Option<String> {
    let alive = |name: &str| now.iter().any(|session| session.name == name);
    // What the person saw, then whatever the daemon has gained since — one order, so the walk is a
    // single wrapping index and the appended rows are reachable at the end of a lap.
    let mut order: Vec<&str> = seen
        .iter()
        .map(|session| session.name.as_str())
        .chain(
            now.iter()
                .map(|session| session.name.as_str())
                .filter(|name| !seen.iter().any(|session| session.name == *name)),
        )
        .collect();
    let here = match order.iter().position(|name| *name == killed) {
        Some(at) => at,
        None => {
            // The mirror has been refreshed past this client's own row. Put it back where the
            // person last saw it, and the walk below is unchanged. The bound is `<= len` because an
            // anchor one past the end is a row that stood LAST — `insert` accepts exactly that, and
            // anything beyond it is a memory no order can host, which is a detach.
            let at = anchor.filter(|at| *at <= order.len())?;
            order.insert(at, killed);
            at
        }
    };
    let here = here as isize;
    let len = order.len() as isize;
    (1..len)
        .map(|lap| order[(here + step * lap).rem_euclid(len) as usize])
        .find(|name| alive(name))
        .map(str::to_owned)
}

/// The first session in list order that is NOT `killed` and that no client is viewing
/// ([`SessionInfo::attached`] `== 0`) — tmux `no-detached`'s fallback when this client has viewed
/// nothing else that survives, and `None` when every other session is occupied (which is a DETACH:
/// `no-detached` leaves rather than pile a second client onto a colleague's session).
///
/// ## The counts must come from a list read AT THE DECISION, and until R327 they could not
///
/// R326 measured this walking into an occupied session. The reason is not in this function: an
/// attach bumps only the channel of the session ATTACHED TO, so a client parked on its own session
/// is never woken to re-read the counts its policy depends on, and nothing bounds how stale the
/// mirror's are. The re-read that would fix it was itself refused — scope resolution gated every
/// method, including a read whose subject is the whole registry, so at the one moment this decision
/// is made the list could not be fetched at all.
///
/// R327 opened that door daemon-side ([`sprag_host::registry_scene`]), and
/// [`destroy_successor`] now hands this the list as of NOW rather than the mirror. The MRU-preferred
/// half of the policy never had the problem: it is answered inside the daemon, off the attachment
/// map itself.
/// **THE SESSION A LAUNCH SHOULD ADOPT**, or [`None`] when there is nothing to adopt and one must be
/// created — register item 284.
///
/// # ⚠⚠⚠ The order, and what each preference is protecting
///
/// 1. **A session nobody is viewing.** This is the several-windows workflow the old create-always
///    behaviour was protecting, kept — a second launch still gets a window of its own, it just stops
///    inventing a session to put in it.
/// 2. **Failing that, any session.** Every one is occupied, so piling on is the honest choice: the
///    host serves multi-attach and the person plainly has work open. Creating here would be the
///    original defect wearing a condition.
/// 3. **[`None`] only when there are none at all**, which is the one case a launch should create.
///
/// ⚠⚠ EMPTY SESSIONS ARE NOT SKIPPED, and that is deliberate. A session with no panes is where the
/// last one exited, which is exactly somewhere a person may be coming back to — and the alternative
/// rule (*adopt only sessions with work in them*) would create a fresh one beside every session that
/// had just been tidied, which is how seven of them accumulated.
fn adoptable(list: &[SessionInfo]) -> Option<String> {
    list.iter()
        .find(|session| session.attached == 0)
        .or_else(|| list.first())
        .map(|session| session.name.clone())
}

fn first_free_other(list: &[SessionInfo], killed: &str) -> Option<String> {
    list.iter()
        .find(|session| session.name != killed && session.attached == 0)
        .map(|session| session.name.clone())
}

/// What this client should do when its own attached session `killed` is destroyed under `policy` —
/// the tmux `detach-on-destroy` decision, resolved against TWO readings of the session list.
///
/// # Why two lists, and why neither one can answer both questions
///
/// * **`seen`** is the mirror the user's sidebar was drawn from, and it still holds `killed`. That
///   row is the ANCHOR `next` and `previous` count from — "the session after the one that died" is
///   not a question a list without it can answer, so a re-read is not merely unnecessary here, it
///   is unusable.
/// * **`now`** is the list re-read at the instant of the decision, where `killed` is already gone.
///   `no-detached` asks *"is anybody sitting in it"*, and that is a fact about OTHER CLIENTS which
///   the mirror has no way to have learnt — see [`first_free_other`] for the measurement.
///
/// So the split is by question, not by preference: the ORDER comes from what the person could see,
/// the OCCUPANCY from what is true now. The two are the same list for the callers that plan BEFORE
/// a kill they are performing themselves ([`HostClient::kill_session`], [`HostClient::kill_window`]),
/// and differ only on the out-of-band path — which is exactly the path the defect was on.
///
/// `off` and `no-detached` prefer the session this client was viewing before, which the DAEMON
/// resolves (see [`Successor::LastViewed`]); this names what to do when there is no such session.
/// `off` falls back to the `next` list neighbour rather than detaching — "off" means "don't leave
/// if there is somewhere to go", so it detaches only when `killed` is truly the last session —
/// while `no-detached` falls back only to an UNOCCUPIED session, and leaves rather than share one.
fn destroy_successor(
    policy: DetachOnDestroy,
    seen: &[SessionInfo],
    anchor: Option<usize>,
    now: &[SessionInfo],
    killed: &str,
) -> Successor {
    match policy {
        DetachOnDestroy::Detach => Successor::Detach,
        DetachOnDestroy::Off => Successor::LastViewed {
            unattached: false,
            fallback: list_neighbour(seen, anchor, now, killed, 1),
        },
        DetachOnDestroy::NoDetached => Successor::LastViewed {
            unattached: true,
            fallback: first_free_other(now, killed),
        },
        DetachOnDestroy::Next => {
            list_neighbour(seen, anchor, now, killed, 1).map_or(Successor::Detach, Successor::Named)
        }
        DetachOnDestroy::Previous => list_neighbour(seen, anchor, now, killed, -1)
            .map_or(Successor::Detach, Successor::Named),
    }
}

/// The GUI's wire client of a `sprag-term` host. See the module docs.
pub struct WireHost {
    /// WHICH KIND OF FRONTEND holds this client, kept for the whole of its life because the
    /// decision it feeds is taken on a DESTROY — long after the boot that knew. See [`Frontend`].
    frontend: Frontend,
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
    /// Every session's live ACTIVITY, mirrored ([`ActivityMirror`]) — the sidebar's subtitle line.
    /// Beside the session list rather than inside it because R282 made them separate questions: that
    /// one is the registry's structure, this is a SAMPLE of the operating system, and only one of
    /// the two has an age. `None` until the first read lands, which is an honest "this client has
    /// not been told yet" rather than a row of empty facts.
    activity: ActivityMirror,
    /// The UI thread's request connection (reads / input / resize). `RefCell`, not
    /// `Mutex`: `WireHost` is UI-thread-only (see the module docs), and the poll thread
    /// owns a SEPARATE connection. A session SWITCH re-scopes this connection in place.
    conn: RefCell<HostConn>,
    /// This GUI's opaque CLIENT id (R-PR67), shared by its request + poll connections so the daemon
    /// counts one window as ONE attached client, not one per connection. Announced on every
    /// connection ([`shake_hands`]) and used to attach ([`send_attach`]) on boot and each switch;
    /// minted once per process ([`new_gui_client_id`]). A lifecycle token, not identity.
    client_id: String,
    /// The NAME of the session this client is currently viewing — MIRRORED from the daemon
    /// ([`SESSION_SLOT`]), not remembered.
    ///
    /// Read to highlight the switcher's current row, to mark the palette's current session, to walk
    /// next/previous, and to title `sprag-tui`'s terminal. Re-pointed by
    /// [`switch_session`](WireHost::switch_session) — and REFRESHED BY THE POLL THREAD, which is the
    /// part that has to be a mirror: this client no longer addresses its session by name
    /// ([`HostConn::scope_to_attached`]), so a `rename-session` moves it underneath and nothing the
    /// client does would ever notice. **Measured at R303**: the terminal title stayed `sprag: alpha`
    /// for the whole life of a client the daemon was reporting on `production`.
    ///
    /// `Arc<Mutex<_>>` rather than the `RefCell` it was, for exactly that reason: the writer is the
    /// poll thread and the readers are the paint path, the same arrangement as every other mirror
    /// here.
    session: SessionMirror,
    /// The message the daemon has handed this client and its surface has not shown yet
    /// ([`MessageMirror`], R317) — `sprag display-message`, collected by the poll thread on the wake
    /// the delivery itself caused.
    ///
    /// Beside the session name and refreshed by the same thread, but the OPPOSITE kind of fact: a
    /// name is a level this client re-reads and can read twice harmlessly, and this is an EDGE that
    /// must be consumed exactly once. That is why the reader takes it
    /// ([`take_message`](sprag_host::wake::WakeSource::take_message)) rather than reading it, and why it survives a
    /// session SWITCH untouched: the message was addressed to this client, not to the session it
    /// happened to be watching when somebody sent it.
    message: MessageMirror,
    /// The SKEW this client's own act met and its surface has not shown yet — a daemon too old to
    /// perform the action a keystroke asked for (R324).
    ///
    /// Beside the message mirror and deliberately NOT it: that one holds what the DAEMON routed,
    /// which the terminal front copies out to the desktop and drains on a WAKE — and a daemon that
    /// performs nothing bumps no channel, so the wake never comes. Written by
    /// [`request`](WireHost::request), which is where the fault is seen, and taken by the key path
    /// that caused it.
    gesture_refused: MessageMirror,
    /// The host endpoint this client connect-or-spawned on — kept so a session switch can open a
    /// FRESH poll connection to the same daemon (the request conn is re-scoped in place; the poll
    /// thread is torn down and a new one spawned on a new connection).
    ///
    /// The [`HostEndpoint`] rather than its path, so a later failure on this daemon is reported
    /// the way the boot's was: naming WHICH daemon, and what pointed this client at it.
    endpoint: HostEndpoint,
    /// The pane grid `(cols, rows)` this client booted at — the birth size a sidebar "+" gives a
    /// new session (it reflows to this window on first paint, like every boot pane).
    boot_dims: (u16, u16),
    /// Set by the poll thread when this client's attached session is destroyed OUT OF BAND under a
    /// SWITCH policy (another client / the `sprag` CLI killed it): the poll cannot switch (a UI-thread
    /// op), so it flags this + repaints, and the UI-thread
    /// [`resolve_lost_session`](sprag_host::wake::WakeSource::resolve_lost_session) does the switch,
    /// on the one wake this client takes its duties on. Shared
    /// `Arc<AtomicBool>` (the poll thread is off-thread); swap-cleared by the reconcile and by any
    /// successful [`attach_in_place`](WireHost::attach_in_place), so a manual switch that pre-empts
    /// the reconcile can't leave
    /// a stale flag to fire a spurious second switch. Never set under the `Detach` policy — that path
    /// stays the poll thread's own immediate detach, unchanged.
    lost_session: Arc<AtomicBool>,
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
    /// The SAMPLED facts' own clock ([`ActivityThread`]) — spawned once at boot and never swapped,
    /// unlike [`Self::poll`]: the activity question is registry-WIDE, so a session switch changes
    /// nothing about what this thread asks or which sessions it hears about. `Option` is the
    /// after-Drop state, and the `None` a client that could not spawn it boots with — a rail whose
    /// subtitle goes stale is worse than one that never appears, but neither is worth refusing to
    /// start a terminal over.
    activity_thread: RefCell<Option<ActivityThread>>,
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

/// The thread that keeps the SAMPLED facts live — the session rail's subtitle (R282).
///
/// # Why the poll thread cannot do this
///
/// The poll thread parks on `scene/waitFor`, which wakes when the SCENE moves — a batch of PTY
/// output, a pane opening, a window changing. Where a session is working, on what branch, and what
/// it is serving move with none of those. They move when a shell chdirs, when a server binds a port,
/// when somebody checks out a branch — and while a `cd` does happen to print a prompt, the wake it
/// causes races the sample: the daemon can answer that wake from a sample taken a moment BEFORE the
/// chdir, and then nothing bumps the revision again, so the rail shows the old directory until the
/// next unrelated keystroke. Measured, not reasoned about: the pixel smoke's sidebar check timed out
/// exactly that way before this thread existed.
///
/// So a fact nothing announces needs a reader with its own clock. This is that clock, and it is the
/// CLIENT's rather than the daemon's on purpose: a daemon that sampled on a timer would walk `/proc`
/// forever on a box nobody is looking at, while this runs only while a client is attached and asks
/// for a tolerance the daemon serves from one held sample however many clients ask
/// ([`sprag_terminal::ActivitySampler`]).
struct ActivityThread {
    /// Set to stop the loop, with the condvar below signalled so a sleeping refresh wakes at once
    /// rather than after its remaining tolerance.
    stop: Arc<(Mutex<bool>, Condvar)>,
    /// The thread handle, joined by [`stop`](Self::stop) (taken once).
    handle: Option<JoinHandle<()>>,
}

impl ActivityThread {
    /// Stop the refresh thread and join it: flag, signal, join.
    ///
    /// No socket shutdown, unlike [`PollThread::stop`]: this thread never parks host-side. It is
    /// either inside a bounded request or asleep on the condvar, and the signal ends the sleep — so
    /// the join is deterministic without cancelling a read.
    fn stop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The activity thread's own connection: connected, **BOUNDED**, and shaken hands with.
///
/// # Why this is a function and not three lines at the call site
///
/// It was three lines at the call site, and the middle one was missing. [`ActivityThread::stop`]
/// already claimed what it needed — *"this thread never parks host-side: it is either inside a
/// BOUNDED REQUEST or asleep on the condvar, so the join is deterministic"* — while the connection
/// it was handed carried no deadline at all. Against a daemon that accepts and then answers nothing,
/// the refresh parks inside `query_activity`, never looks at the stop flag again, and `stop`'s join
/// waits forever: **a display client that cannot shut down**, for the sake of a subtitle.
///
/// R343 found it by sweeping one front over from the `sprag` CLI's own version of the same defect.
/// A seam rather than a fourth copy of `set_read_deadline`, so the next connection added here
/// cannot be the one that forgets — and so the claim above has something to be true OF.
///
/// # Errors
///
/// If the endpoint refuses the connection or the handshake fails. Best-effort at the call site: what
/// this drives is a subtitle, and the deadline is what keeps a failure that size.
fn activity_connection(endpoint: &HostEndpoint, client_id: &str) -> io::Result<HostConn> {
    let mut conn = HostConn::connect(endpoint.path(), CONNECT_TIMEOUT)?;
    conn.set_read_deadline(Some(REQUEST_DEADLINE))?;
    shake_hands(&mut conn, client_id)?;
    Ok(conn)
}

/// Re-read the sampled activity every [`SESSION_ACTIVITY_DISPLAY_MAX_AGE`] until stopped, repainting
/// only when it CHANGED.
///
/// The change check is what keeps an idle window idle: without it every client would repaint once a
/// second forever, which is a worse regression than the staleness this fixes. With it, a box where
/// nothing moves costs one round trip per second per client and not one frame.
fn spawn_activity_refresh(
    mut conn: HostConn,
    activity: ActivityMirror,
    on_change: Arc<dyn Fn() + Send + Sync>,
    stop: Arc<(Mutex<bool>, Condvar)>,
) -> io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("sprag-gui-wire-activity".to_owned())
        .spawn(move || {
            let (flag, wake) = &*stop;
            loop {
                {
                    let guard = flag.lock().unwrap_or_else(PoisonError::into_inner);
                    if *guard {
                        break;
                    }
                    // Sleep FIRST: the boot already took a reading, so refreshing immediately would
                    // spend a round trip re-asking a question just answered.
                    let (guard, _) = wake
                        .wait_timeout(guard, SESSION_ACTIVITY_DISPLAY_MAX_AGE)
                        .unwrap_or_else(PoisonError::into_inner);
                    if *guard {
                        break;
                    }
                }
                match query_activity(&mut conn) {
                    Ok(reading) => {
                        let moved = lock_activity(&activity)
                            .as_ref()
                            .is_none_or(|held| held.reading.value != reading.value);
                        store_activity(&activity, reading);
                        if moved {
                            on_change();
                        }
                    }
                    // Kept, not fatal: this thread drives a SUBTITLE. A daemon that will not answer
                    // it is reported by the poll thread, which is the one whose failure means the
                    // client can no longer paint.
                    Err(error) => tracing::debug!(
                        target: "sprag_gui::wire",
                        %error,
                        "activity refresh failed; keeping the last-known sample",
                    ),
                }
            }
        })
}

/// What a client is booting: WHICH daemon, WHICH session, and the panes to open in it.
///
/// A struct rather than six positional arguments because these are the boot's INPUTS, and naming
/// them is what lets [`WireHost::boot`] be a decision over its inputs instead of a reader of
/// process globals — the same discipline the session resolution follows. It is also the extension
/// point: a launcher that learns more about the session it wants (a working directory, an
/// environment) adds a field here rather than a seventh argument at three call sites.
pub struct BootSpec<'a> {
    /// The daemon to connect-or-spawn on, WITH its provenance — every failure this boot reports
    /// names it, so a client can never drive a daemon nobody named without saying so.
    pub endpoint: &'a HostEndpoint,
    /// The session to ATTACH to (adopting its live panes), or `None` to let this boot decide —
    /// which since item 284 means *adopt what is there*, and create only when there is nothing.
    pub session: Option<&'a str>,
    /// **ASK FOR A SESSION OF THIS BOOT'S OWN**, even where one could be adopted — the explicit
    /// *new* that the default stopped being.
    ///
    /// ⚠⚠⚠ It exists because making adoption the default removed the only way to say the other
    /// thing. The owner's specification is *"the existing sessions are just there, and a new one
    /// appears only when I press new"* — two verbs, and a launch that could only ever create was
    /// wrong in one direction while a launch that can only ever adopt is wrong in the other.
    ///
    /// ⚠ Ignored when [`session`](Self::session) names one: attaching to a named session and
    /// creating a fresh one are different requests, and honouring both would mean guessing which
    /// the caller meant.
    pub fresh: bool,
    /// The command each booted pane runs, or `None` for the host's own `$SHELL`.
    pub argv: Option<&'a [String]>,
    /// The pane grid this client boots at.
    pub cols: u16,
    /// The pane grid this client boots at.
    pub rows: u16,
    /// How many panes to ensure when this boot CREATES its session (an attach adopts what is
    /// there and ignores this).
    pub panes: usize,
    /// WHICH KIND OF FRONTEND this is, for the defaults whose right answer differs between a window
    /// and a terminal — today only the destroy policy. See [`Frontend`], and item 282 for what one
    /// shared default cost.
    pub frontend: Frontend,
}

/// A session THIS boot created — and the obligation that comes with having created it.
///
/// The daemon outlives its clients by design, so a session a client made and then abandoned lives
/// forever: it holds its name, it is restored from the durability snapshot, and nothing will ever
/// come back for it. Nine such sessions accumulated on a live daemon in one afternoon
/// (R278) before this type existed.
///
/// It is deliberately NOT a `Drop` guard. Drop cannot compose the error it must report into, and
/// the whole point of the rollback is that the failure SAYS what it left behind — so the undo is
/// an explicit consuming call with the cause in hand.
struct BornSession<'a> {
    /// Where the session was created, for the rollback's own connection and for the report.
    endpoint: &'a HostEndpoint,
    /// The name the daemon allocated.
    session: String,
}

impl BornSession<'_> {
    /// Undo the creation, then report `cause` with what the undo achieved.
    ///
    /// The kill goes over a **fresh** connection, not the boot's: the failure being guarded is
    /// usually that connection being unusable — a `HostConn` whose read deadline tripped is
    /// finished by design ([`HostConn::set_read_deadline`]) and would refuse the one request that
    /// matters. A fresh connect also re-tests the daemon, which is the thing being asked about.
    fn roll_back(self, cause: io::Error) -> BootError {
        match self.kill() {
            Ok(()) => {
                tracing::info!(
                    target: "sprag_gui::wire",
                    endpoint = %self.endpoint,
                    session = %self.session,
                    "boot failed; removed the session it had created",
                );
                BootError {
                    endpoint: self.endpoint.clone(),
                    residue: BootResidue::Removed(self.session),
                    cause,
                }
            }
            Err(failure) => {
                tracing::error!(
                    target: "sprag_gui::wire",
                    endpoint = %self.endpoint,
                    session = %self.session,
                    %failure,
                    "boot failed and the session it created could NOT be removed; it is orphaned",
                );
                BootError {
                    endpoint: self.endpoint.clone(),
                    residue: BootResidue::Orphan {
                        session: self.session,
                        failure,
                    },
                    cause,
                }
            }
        }
    }

    /// One `kill_session` over a connection opened for it, with its reply BOUNDED — a daemon that
    /// accepts and then stops answering must not hold a client that is already failing.
    ///
    /// The connect gets NO retry window, unlike the boot's. [`CONNECT_TIMEOUT`] exists for the
    /// spawn race — a daemon we just started has not bound yet — and this daemon answered a
    /// request moments ago, so the three ways it can behave now are: it accepts (instantly, a
    /// listening socket's backlog does not depend on the accept loop), it is gone (instantly
    /// refused), or it is wedged (accepts, then the read deadline below applies). Retrying for
    /// five seconds serves none of them, and it would spend those seconds with a failing client
    /// showing nothing.
    fn kill(&self) -> io::Result<()> {
        let mut conn = HostConn::connect(self.endpoint.path(), Duration::ZERO)?;
        conn.set_read_deadline(Some(REQUEST_DEADLINE))?;
        conn.call(
            "scene/invoke",
            invoke(
                &mux_action_path(KILL_SESSION_ACTION),
                json!({ "name": self.session }),
            ),
        )?;
        Ok(())
    }
}

/// What a failed boot LEFT on the daemon.
///
/// Three states, and the type makes the wrong ones unrepresentable: an orphan always carries both
/// the session's name and why removing it failed, because an operator told "something was left
/// behind" without either of those has been told nothing actionable.
#[derive(Debug)]
enum BootResidue {
    /// Nothing: the boot failed before it created a session, or it ATTACHED to one — which it
    /// must never remove, having not made it.
    None,
    /// The boot created this session and has removed it again.
    Removed(String),
    /// The boot created this session and it is STILL on the daemon, because the removal failed.
    Orphan { session: String, failure: io::Error },
}

/// Why a client's boot failed — including which daemon it was talking to and what it left there.
///
/// Returned inside an [`io::Error`] so every existing caller keeps its `io::Result`, and reachable
/// with [`get_ref`](io::Error::get_ref) + `downcast_ref` for one that needs the facts rather than
/// the prose. That is the point of it being a type: "did this boot leave a session behind?" is a
/// question a caller — or a test — must be able to ask without parsing a sentence.
#[derive(Debug)]
pub struct BootError {
    endpoint: HostEndpoint,
    residue: BootResidue,
    cause: io::Error,
}

impl BootError {
    /// A failure with nothing left behind: the boot never got as far as creating a session, or it
    /// ATTACHED to one — which is not this boot's to remove however badly the rest of it went.
    fn left_nothing(endpoint: HostEndpoint, cause: io::Error) -> Self {
        Self {
            endpoint,
            residue: BootResidue::None,
            cause,
        }
    }

    /// The daemon this boot was talking to, with the provenance of how it was chosen.
    #[must_use]
    pub fn endpoint(&self) -> &HostEndpoint {
        &self.endpoint
    }

    /// The session this boot CREATED, whether or not it survived — `None` when the boot created
    /// none (it failed earlier, or it attached to an existing session).
    #[must_use]
    pub fn created(&self) -> Option<&str> {
        match &self.residue {
            BootResidue::None => None,
            BootResidue::Removed(session) | BootResidue::Orphan { session, .. } => Some(session),
        }
    }

    /// The session this boot created and could NOT remove — `Some` only when it is still on the
    /// daemon, which is exactly when a caller has something to act on.
    #[must_use]
    pub fn orphan(&self) -> Option<&str> {
        match &self.residue {
            BootResidue::Orphan { session, .. } => Some(session),
            BootResidue::None | BootResidue::Removed(_) => None,
        }
    }
}

impl fmt::Display for BootError {
    /// The endpoint, the cause, then what became of a created session — in that order, because
    /// "which daemon" is the question the silent fallback made unanswerable and it belongs first.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.endpoint, self.cause)?;
        match &self.residue {
            BootResidue::None => Ok(()),
            BootResidue::Removed(session) => {
                write!(
                    f,
                    " (the session `{session}` this boot created was removed)"
                )
            }
            BootResidue::Orphan { session, failure } => write!(
                f,
                " (the session `{session}` this boot created is STILL on that daemon — removing \
                 it failed: {failure}; remove it with `sprag kill-session -t {session}`)",
            ),
        }
    }
}

impl std::error::Error for BootError {
    /// The underlying wire failure, so a caller walking the chain reaches the real cause rather
    /// than only this report of it.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

impl From<BootError> for io::Error {
    /// Carry the boot's own error KIND outward: a caller matching on `NotFound` or
    /// `ConnectionRefused` sees what the wire saw, with the boot's report attached as the payload.
    fn from(error: BootError) -> Self {
        Self::new(error.cause.kind(), error)
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
    /// it is a detached process this GUI does not own. Every failure carries a [`BootError`]
    /// (naming the endpoint, and what became of a session this boot created); see [`boot`](Self::boot).
    pub fn spawn_or_attach(
        frontend: Frontend,
        argv: Option<Vec<String>>,
        cols: u16,
        rows: u16,
        n_panes: usize,
        on_change: Arc<dyn Fn() + Send + Sync>,
        quit: Arc<dyn QuitSink>,
    ) -> io::Result<Self> {
        // The ONE place this boot reads the process environment. [`SESSION_ENV`] names a session
        // to ATTACH to (absent creates), and the endpoint resolves by the client precedence
        // ([`HostEndpoint::client`]). Everything below decides on its INPUTS, which is what makes
        // the boot testable against a private daemon without touching process globals.
        let requested = std::env::var_os(SESSION_ENV)
            .filter(|name| !name.is_empty())
            .map(|name| name.to_string_lossy().into_owned());
        Self::boot(
            &BootSpec {
                endpoint: &HostEndpoint::client(),
                session: requested.as_deref(),
                // ⚠⚠ A LAUNCH DOES NOT ASK FOR A NEW ONE — item 284. This is the entry point every
                // window and every `sprag attach` comes through, and *take me to my work* is what
                // naming nothing means here. The explicit `new` belongs to a caller that says so;
                // ⚠ TODAY THE ONLY ONE IS A TEST, so nothing a person can press reaches it yet, and
                // the sidebar's "+" is a running client's `new_session` rather than a boot.
                fresh: false,
                argv: argv.as_deref(),
                cols,
                rows,
                panes: n_panes,
                frontend,
            },
            on_change,
            quit,
        )
    }

    /// The boot, over an EXPLICIT [`BootSpec`] — the body [`spawn_or_attach`](Self::spawn_or_attach)
    /// wraps once it has read the environment, and the entry point for a caller that already knows
    /// which daemon it means (a launcher that resolved the endpoint itself, a test harness with a
    /// private socket).
    ///
    /// # The session this boot creates is this boot's to undo
    ///
    /// When the spec names no session, the session resolution CREATES one on a daemon that
    /// outlives this client. Everything after that can still fail — panes, the mirrors, the poll
    /// connection — and a client that just returned the error would leave that session behind
    /// forever. So the tail runs as ONE fallible expression, and its failure is handed to the
    /// rollback, which removes the session over a FRESH connection and reports what it managed to
    /// do (see this module's `BornSession`).
    ///
    /// An ATTACHED session is never rolled back: a client that failed to attach must not remove a
    /// session it did not make. That is structural here — the guard is only constructed on the
    /// create path.
    ///
    /// # Errors
    ///
    /// Any failure to reach the daemon or to boot against it, always as a [`BootError`]: the
    /// endpoint that was reached, the cause, and what became of a session this boot created.
    pub fn boot(
        spec: &BootSpec<'_>,
        on_change: Arc<dyn Fn() + Send + Sync>,
        quit: Arc<dyn QuitSink>,
    ) -> io::Result<Self> {
        // Connect-or-spawn on the resolved endpoint. A daemon there outlives every client, so
        // first try to JOIN one; only if none answers do we spawn a detached `--daemon` and
        // connect through the bind-race retry. We do NOT own its lifetime — no kill, no
        // PDEATHSIG — which is the whole point: the session survives this GUI. A spawn RACE is
        // safe, because the daemon's single-instance flock leaves exactly one alive and every
        // client connects to it.
        let endpoint = spec.endpoint;
        let mut conn = match Self::reach_daemon(endpoint) {
            Ok(conn) => conn,
            // Nothing has been created yet, so there is no residue to report — only WHICH daemon
            // could not be reached, which is the fact the silent fallback used to swallow.
            Err(cause) => return Err(BootError::left_nothing(endpoint.clone(), cause).into()),
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

        // ANNOUNCE AND AGREE, before anything is created. The hello carries this client's id (the
        // group key its connections share) and the wire's shape; a daemon this build cannot
        // understand is refused HERE, where the only thing lost is a connection — not after a
        // session has been made and a boot is nine requests deep, which is exactly how R278's
        // client came to abandon one.
        let client_id = new_gui_client_id();
        if let Err(cause) = shake_hands(&mut conn, &client_id) {
            return Err(BootError::left_nothing(endpoint.clone(), cause).into());
        }

        // Resolve WHICH session this client acts on before booting panes, and scope every
        // request to it (both this connection and the poll one below), so a request can never
        // silently land in another session. Naming one ATTACHES (adopt its panes); naming none
        // ALLOCATES a fresh one (spawn our own panes) — the "each launch is its own session"
        // model. `boot_panes` branches on `created`, replacing the old "did we spawn the host"
        // key with "did we create the session".
        let (session, created) = match resolve_session(
            &mut conn,
            spec.session,
            spec.fresh,
            spec.argv,
            spec.cols,
            spec.rows,
        ) {
            Ok(resolved) => resolved,
            Err(cause) => return Err(BootError::left_nothing(endpoint.clone(), cause).into()),
        };
        // From here on this boot OWNS the session if it made it. The guard is the only thing
        // that knows how to undo the creation, and the single `match` below is the only place
        // the tail's failure can reach — so no later edit can add an early return that skips it.
        let born = created.then(|| BornSession {
            endpoint,
            session: session.clone(),
        });
        let booted =
            Self::boot_into_session(conn, spec, client_id, session, created, on_change, quit);
        match (booted, born) {
            (Ok(host), _) => Ok(host),
            (Err(cause), Some(born)) => Err(born.roll_back(cause).into()),
            (Err(cause), None) => Err(BootError::left_nothing(endpoint.clone(), cause).into()),
        }
    }

    /// Join a running daemon on `endpoint`, else spawn one and connect through the bind-race
    /// retry. Both outcomes are logged WITH the endpoint's provenance, so even a successful boot
    /// records which daemon it chose and what pointed it there.
    fn reach_daemon(endpoint: &HostEndpoint) -> io::Result<HostConn> {
        match HostConn::connect(endpoint.path(), Duration::ZERO) {
            Ok(conn) => {
                tracing::info!(target: "sprag_gui::wire", %endpoint, "joined a running host");
                Ok(conn)
            }
            Err(_) => {
                spawn_daemon(endpoint.path())?;
                tracing::info!(target: "sprag_gui::wire", %endpoint, "spawned a daemon host");
                HostConn::connect(endpoint.path(), CONNECT_TIMEOUT)
            }
        }
    }

    /// Everything after the session exists: the panes, the mirrors, and the poll thread.
    ///
    /// Split out as ONE fallible expression so that [`boot`](Self::boot) has exactly one place to
    /// catch a failure that must undo a created session. Any `?` added here is covered by that
    /// rollback for free; a `?` added to `boot` itself after the creation would not be, which is
    /// why the tail lives in its own function rather than behind a comment asking for care.
    fn boot_into_session(
        mut conn: HostConn,
        spec: &BootSpec<'_>,
        client_id: String,
        session: String,
        created: bool,
        on_change: Arc<dyn Fn() + Send + Sync>,
        quit: Arc<dyn QuitSink>,
    ) -> io::Result<Self> {
        let endpoint = spec.endpoint;
        let (cols, rows) = (spec.cols, spec.rows);
        // R-PR67: this GUI is one attached CLIENT across its two connections — the id was minted
        // and announced by the handshake in `boot`, before anything was created. Attaching it to
        // the session makes the daemon count this window as a viewer (the sidebar badge), and is
        // done before the `since0` baseline below so the attach's own scene bump is folded into
        // the baseline rather than becoming a spurious first poll wake.
        //
        // R303: and it is what every read after it is scoped BY. `attach_and_follow` names the
        // session to attach and then drops the name — this client asks for "the session I am
        // viewing" from here on, so a rename of it moves this client with it instead of refusing it.
        let attached = matches!(
            attach_and_follow(&mut conn, Attaching::Named(&session)),
            Landed::On(_),
        );
        let seeds = boot_panes(&mut conn, spec.argv, cols, rows, spec.panes, created)?;

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
            // Best-effort, unlike the arrangement: a client that cannot learn the arbitrated window
            // falls back to its own surface, which is what it did before the slot existed. A daemon
            // too old to serve it must not stop this client from painting.
            window_size: query_window_size(&mut conn).unwrap_or_default(),
        }));
        let windows: WindowsMirror = Arc::new(Mutex::new(window_list));
        // EVERY session, mirrored for the switcher sidebar — booted like the window list and for the
        // same reason (a switcher draws it and must never fetch it from the paint path).
        // Booted through the same write the poll thread uses, so the ANCHOR is set from the boot
        // list rather than left `None` until the first wake — a client killed between attaching and
        // its first wake would otherwise have no place to count from.
        let sessions: SessionsMirror = Arc::new(Mutex::new(Sessions::default()));
        store_sessions(&sessions, query_sessions(&mut conn)?, &session);
        // The sidebar's SAMPLED half. Best-effort, unlike the list above: a client that cannot read
        // it draws every row without its subtitle, which is a poorer sidebar and a working one — the
        // same degradation the window size takes, and for the same reason (nothing a client paints
        // its panes from depends on it).
        let activity: ActivityMirror = Arc::new(Mutex::new(None));
        if let Ok(reading) = query_activity(&mut conn) {
            store_activity(&activity, reading);
        }

        // Construct the client with NO poll thread yet, then spawn the initial one through the SAME
        // path a session switch re-spawns through ([`spawn_poll_for`]) — one poll-spawn SSOT for
        // boot and switch, so neither can drift from the other.
        let host = Self {
            frontend: spec.frontend,
            cache,
            layout,
            windows,
            sessions,
            activity,
            conn: RefCell::new(conn),
            client_id: client_id.clone(),
            session: Arc::new(Mutex::new(session.clone())),
            // Empty at boot and NOT read here: a message is addressed to a client, and this one has
            // not attached yet — `client/attach` is what puts it in the set `display-message`
            // reaches. Anything sent between now and the first wake is collected by that wake, which
            // the delivery's own bump causes.
            message: Arc::new(Mutex::new(None)),
            // Empty at boot for a sharper reason than the mailbox's: nothing this client has done
            // can have met a skew yet, and the boot READS that could have are the poll's, which
            // this deliberately does not carry.
            gesture_refused: Arc::new(Mutex::new(None)),
            endpoint: endpoint.clone(),
            boot_dims: (cols, rows),
            lost_session: Arc::new(AtomicBool::new(false)),
            on_change,
            quit,
            poll: RefCell::new(None),
            activity_thread: RefCell::new(None),
        };
        // The poll thread's own connection — a parked `scene/waitFor` on it never blocks the
        // request connection above (separate host handler threads). Scoped to the SAME session, so
        // its `waitFor`/`revision`/re-queries watch the client's own session and never another's —
        // which after R303 means the same ATTACHMENT, not merely the same name. It attaches nothing
        // itself; saying hello with this client's id is what puts it on the same view.
        let mut poll_conn = HostConn::connect(endpoint.path(), CONNECT_TIMEOUT)?;
        scope_to_view(&mut poll_conn, &session, attached);
        // The poll connection is a SECOND connection of the SAME client: announce the same id so the
        // daemon groups both under one attached client (not two). Only the request conn attaches.
        shake_hands(&mut poll_conn, &client_id)?;
        host.spawn_poll_for(poll_conn, since0)?;
        // A THIRD connection, for the one fact no wake announces (see [`ActivityThread`]). Its own,
        // because the poll connection is parked host-side for as long as the scene is quiet — which
        // is exactly when this thread has work.
        //
        // Best-effort, unlike the two above: what it drives is a subtitle. A client that cannot get
        // a connection for it paints the rail without live facts rather than refusing to open a
        // terminal, which is the same call `send_attach` makes about the viewer badge.
        match activity_connection(endpoint, &client_id) {
            Ok(activity_conn) => host.spawn_activity_for(activity_conn),
            Err(error) => tracing::debug!(
                target: "sprag_gui::wire",
                %error,
                "no connection for the activity refresh; the session rail's subtitle will not update",
            ),
        }
        Ok(host)
    }

    /// Install the activity refresh thread on `conn` — the ONE spawn site, called once at boot.
    ///
    /// Best-effort like its connection: a thread that cannot be spawned leaves the rail's subtitle
    /// at whatever the boot read, which the poll thread still refreshes on every wake. What is lost
    /// is only the healing of a sample that went stale while nothing else happened.
    fn spawn_activity_for(&self, conn: HostConn) {
        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        match spawn_activity_refresh(
            conn,
            Arc::clone(&self.activity),
            Arc::clone(&self.on_change),
            Arc::clone(&stop),
        ) {
            Ok(handle) => {
                *self.activity_thread.borrow_mut() = Some(ActivityThread {
                    stop,
                    handle: Some(handle),
                });
            }
            Err(error) => tracing::debug!(
                target: "sprag_gui::wire",
                %error,
                "the activity refresh thread would not spawn; the session rail's subtitle will not update",
            ),
        }
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
            Arc::clone(&self.activity),
            Arc::clone(&self.session),
            Arc::clone(&self.message),
            Arc::clone(&self.on_change),
            Arc::clone(&self.quit),
            {
                // Resolved on every DESTROY, never captured as a value: the user may `set-option`
                // mid-run and the poll thread must read what is true then. Only the frontend's
                // fallback is captured, because that one cannot change.
                let unset = self.frontend.unset_destroy_policy();
                Arc::new(move || detach_on_destroy(unset))
            },
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
    /// # The history ask, and why `Ok(None)` is not a failure
    ///
    /// [`Attaching::LastViewed`] asks the daemon to resolve the target, and "there is nowhere to go
    /// back to" is one of its answers. It arrives at the ATTACH — the first thing this does, before
    /// any read and long before any commit — so it returns `Ok(None)` with every mirror exactly as
    /// it was, which is the same guarantee the reads-then-commit ordering already gives a failed
    /// read. The caller has stopped the poll thread by then and must restart it (by re-attaching to
    /// where it was), which is what [`switch_session`](HostClient::switch_session)'s own recovery
    /// path already does.
    ///
    /// # Errors
    /// Any read / connect against the target failing — the session is gone or the daemon will not
    /// give a poll connection — or, rarely, the poll thread failing to spawn after the commit.
    fn attach_in_place(&self, to: Attaching<'_>) -> io::Result<Option<String>> {
        // Re-scope the request conn and gather the FULL view + poll baseline over it, all inside one
        // borrow and BEFORE mutating any mirror (so a failed read is a clean abort). Order mirrors
        // boot: revision baseline first (subscribe-then-snapshot), then the frames, then windows /
        // layout / sessions.
        let (
            fetched,
            seeds,
            window_list,
            current,
            layout_snapshot,
            window_size,
            session_list,
            activity,
            since0,
            attached,
            session,
        ) = {
            let mut conn = self.conn.borrow_mut();
            // R-PR67: re-attach this client to the session it just switched to (tmux
            // `switch-client`), moving its viewer count off the old session and onto this one. Before
            // the `since0` baseline, so the attach's scene bump is in the new poll's baseline rather
            // than a spurious self-wake. The old poll conn was already stopped by the caller, so its
            // `on_disconnect` fired; the request conn kept this client present across the switch.
            //
            // R303: the same `attach_and_follow` boot uses, so a switched-to session is addressed
            // exactly as a booted-into one — every read below is already on the attachment. A switch
            // that left this connection name-scoped would be a client that follows a rename until
            // the user switches sessions and then quietly stops.
            let (session, attached) = match attach_and_follow(&mut conn, to) {
                // The daemon named where this client now is, and `conn` is on the attachment.
                Landed::On(session) => (session, true),
                // It says this client has viewed nothing else that survives. Nothing has been read
                // and nothing written, so this is a clean, complete answer.
                Landed::Nowhere => return Ok(None),
                // Refused: a NAMED target carries on name-scoped (the pre-R303 degradation, where
                // this client's reads work and a rename of its session detaches it); a history
                // target has nothing to carry on with, because the refusal WAS its answer.
                Landed::Refused => match to {
                    Attaching::Named(session) => (session.to_owned(), false),
                    // None of these has a name to carry on with: the refusal WAS the answer. The
                    // step arm belongs here rather than beside `Named` for the reason it exists —
                    // it never held a name, and inventing one off the mirror is the second answer
                    // the daemon's walk was built to prevent.
                    // The goto arm joins them: its refusal is the daemon saying the picked row is
                    // GONE, which is the one answer this whole design exists to be able to give.
                    // Falling back to a name here would be inventing the very address the pick
                    // refused to be.
                    Attaching::LastViewed { .. }
                    | Attaching::Step(_)
                    | Attaching::Goto(_)
                    | Attaching::Attached => {
                        return Ok(None);
                    }
                },
            };
            let since0 = read_revision(&mut conn)?;
            let seeds = query_panes(&mut conn)?;
            let fetched = fetch_frames(&mut conn, &pane_ids_of(&seeds));
            let window_list = query_windows(&mut conn)?;
            let current = current_window_name(&window_list).unwrap_or_default();
            let layout_snapshot = query_layout(&mut conn)?;
            // The window is arbitrated PER SESSION, so a switch re-reads it: the session being
            // joined has its own attached clients and its own answer.
            let window_size = query_window_size(&mut conn).unwrap_or_default();
            let session_list = query_sessions(&mut conn)?;
            // Best-effort like the window size, and re-read for the same reason the list is: the
            // subtitle belongs to the sidebar, which is registry-wide and survives the switch.
            let activity = query_activity(&mut conn).ok();
            (
                fetched,
                seeds,
                window_list,
                current,
                layout_snapshot,
                window_size,
                session_list,
                activity,
                since0,
                attached,
                session,
            )
        };
        // A fresh poll connection scoped to the target (its own host handler thread) — connected
        // BEFORE the commit so a daemon that will not answer aborts the switch rather than leaving
        // the client with mirrors swapped but no live updates.
        //
        // Reported through the endpoint: this is the one failure a user meets AFTER the boot, and
        // "Connection refused" naming no daemon is the same silence the boot's own failures were
        // taught out of ([`HostEndpoint::context`](sprag_rpc::HostEndpoint::context)).
        let mut poll_conn = HostConn::connect(self.endpoint.path(), CONNECT_TIMEOUT)
            .map_err(|error| self.endpoint.context(&error))?;
        scope_to_view(&mut poll_conn, &session, attached);
        // The fresh poll conn is a new connection of the SAME client (the old one was torn down by
        // the switch): re-announce the shared id so the daemon keeps grouping both under one client.
        shake_hands(&mut poll_conn, &self.client_id)?;

        // COMMIT: swap every mirror's CONTENTS (the `Arc`s themselves stay — shared with the paint
        // path and the poll thread), set the attached session, then start the poll. `merge_panes`
        // with an empty `existing` is the boot case (all newcomers, each taking its fetched frame).
        let rebuilt = merge_panes(&PaneCache::default(), &seeds, &fetched);
        lock_cache(&self.cache).replace(rebuilt);
        *lock_layout(&self.layout) = Mirrored {
            window: current,
            layout: layout_snapshot,
            window_size,
        };
        store_windows(&self.windows, window_list);
        // The session we have just landed in, so the anchor moves WITH the switch — the row this
        // client stands on is the new one from here, and a stale anchor would count a later destroy
        // from where the person used to be.
        store_sessions(&self.sessions, session_list, &session);
        if let Some(reading) = activity {
            store_activity(&self.activity, reading);
        }
        // The name the DAEMON answered, never the one we asked with. It is a label from here on
        // (the scope is the attachment), and the poll thread refreshes it from `SESSION_SLOT` — set
        // eagerly here only so the first paint after a switch is already right. The VISIT itself is
        // recorded by the daemon, at the attach, keyed by the session's identity: a client that
        // remembered names for that is R304's defect.
        store_session(&self.session, session.clone());
        // A successful attach RESOLVES any "lost session" the poll flagged (the caller joined that
        // poll before this commit, so its flag is now settled): clear it, so a manual switch that
        // pre-empted the reconcile cannot leave a stale flag to fire a spurious second switch.
        self.lost_session.store(false, Ordering::Release);
        self.spawn_poll_for(poll_conn, since0)?;
        (self.on_change)(); // repaint the just-attached session at once, no poll-wake lag
        Ok(Some(session))
    }

    /// Carry out a [`Successor`] — the ONE place a destroy decision becomes a switch, shared by the
    /// own-kill and out-of-band triggers so the two can never perform the same plan differently.
    ///
    /// The poll thread has already been stopped by the caller, and every path here either starts a
    /// fresh one (through [`attach_in_place`](Self::attach_in_place)) or quits.
    ///
    /// [`Successor::LastViewed`] is asked of the DAEMON, after the kill: it resolves by identity, so
    /// the session that just died cannot be its answer (a dead id resolves to nothing) and neither
    /// can an impostor of that name. `Ok(None)` — nowhere to go back to — takes the fallback the
    /// policy named, which is what makes `off` "don't leave if there is somewhere to go".
    /// Answers the session it LANDED on, as the daemon named it, or [`None`] when this client is
    /// leaving instead. R326: a plan is not a landing — every arm here has a fallback, and two of
    /// them can end in a detach the policy did not ask for — so the only honest answer to *"where
    /// did this client go"* is the one read back after the move, never the plan that aimed at it.
    ///
    /// `#[must_use]` because [`Option`] is not, which is R316's whole finding: both kill verbs below
    /// DISCARD this, each for a reason it states, and without the attribute those discards would
    /// look exactly like the eight that motivated [`sprag_host::report`].
    #[must_use = "where a destroy left this client is the one fact no re-read can recover"]
    fn follow(&self, successor: Successor) -> Option<String> {
        match successor {
            Successor::Detach => self.detached(),
            Successor::Named(next) => self.switch_session_named(&next),
            Successor::LastViewed {
                unattached,
                fallback,
            } => match self.attach_in_place(Attaching::LastViewed { unattached }) {
                Ok(landed @ Some(_)) => landed,
                Ok(None) => self.follow_fallback(fallback),
                Err(error) => {
                    tracing::warn!(
                        target: "sprag_gui::wire",
                        %error,
                        "could not go back to the last session; taking the fallback",
                    );
                    self.follow_fallback(fallback)
                }
            },
        }
    }

    /// **WHERE A CLIENT GOES WHEN THERE IS NOWHERE TO GO** — register items 282 and 359, and the
    /// half that `ab07598` left owed.
    ///
    /// # ⚠⚠⚠⚠ Why a window does not end here and a terminal does
    ///
    /// Making the window's destroy default `Off` fixed the case where another session survives. It
    /// left the LAST one: with nothing to switch to, the policy answers `Detach`, and a detached
    /// window has nothing to draw — so the app exited, which is what the owner reported as *"the
    /// program dies completely"*. A terminal client detaching hands the person back the shell they
    /// launched from; a window detaching hands them nothing.
    ///
    /// ⚠⚠⚠ **SO A WINDOW OPENS A SESSION RATHER THAN ENDING.** The reference is herdr
    /// (`src/app/actions.rs:1709` at `9a4ce5e1`): with nothing left it sets `active = None` and
    /// KEEPS DRAWING. sprag cannot hold that state — a `WireHost` is scoped to a session at boot and
    /// 56 reads depend on it — so the nearest true thing is a window showing a session with nothing
    /// in it yet. **Registered rather than pretended**: a real empty state is item 359's remainder.
    ///
    /// ⚠⚠ AND IT IS NOT ITEM 284's GARBAGE. That one is about a LAUNCH inventing a session nobody
    /// asked for and walking away; this is a person at the keyboard whose window would otherwise
    /// vanish, and the session it opens is the one they are looking at.
    ///
    /// ⚠ Quitting stays the answer when the daemon will not serve one, which is the honest end: a
    /// window that cannot reach a host has nothing left to be.
    fn detached(&self) -> Option<String> {
        if self.frontend == Frontend::Terminal {
            self.quit.request_quit();
            return None;
        }
        // ⚠⚠⚠ THE REQUEST IS MADE HERE RATHER THAN THROUGH `new_session`, and that is not a
        // duplication to tidy away: on failure that method answers `current_session()` — the name of
        // the session that was JUST DESTROYED — which is a perfectly non-empty string and would read
        // as success. The one thing this arm must be able to see is the daemon refusing.
        let (cols, rows) = self.boot_dims;
        let created = self.request(
            "scene/invoke",
            invoke(
                &mux_action_path(NEW_SESSION_ACTION),
                json!({ "cols": cols, "rows": rows }),
            ),
            "new_session",
        );
        let Some(opened) = created.as_ref().and_then(Value::as_str).map(str::to_owned) else {
            tracing::error!(
                target: "sprag_gui::wire",
                "the last session was destroyed and the daemon would not open another; ending",
            );
            self.quit.request_quit();
            return None;
        };
        self.switch_session(&opened);
        Some(opened)
    }

    /// A [`Successor::LastViewed`]'s fallback: the named session, or a DETACH when the policy named
    /// none. Both of [`follow`](Self::follow)'s error arms end here, so the two cannot come to treat
    /// "nowhere to go back to" differently.
    fn follow_fallback(&self, fallback: Option<String>) -> Option<String> {
        match fallback {
            Some(next) => self.switch_session_named(&next),
            None => {
                self.quit.request_quit();
                None
            }
        }
    }

    /// Re-attach to `session` after a SWITCH failed, and DETACH if even that will not work — the
    /// reason a failed switch never leaves a client with a stopped poll thread.
    ///
    /// By NAME, deliberately: a switch that failed may have moved the attachment to the target
    /// already, so "where I am" is the wrong question and only the name says which session the user
    /// was on. [`resume`](Self::resume) is the other half of that distinction.
    fn fall_back_to(&self, session: &str) {
        if let Err(error) = self.attach_in_place(Attaching::Named(session)) {
            tracing::error!(
                target: "sprag_gui::wire",
                %error,
                "could not re-attach to the previous session either; detaching",
            );
            self.quit.request_quit();
        }
    }

    /// Start serving again over the session this client never left — for a gesture that stopped the
    /// poll thread and then found it had nowhere to go.
    ///
    /// By ATTACHMENT first ([`Attaching::Attached`]), because nothing moved and the client is still
    /// exactly where it was; `previous` is only the degraded path's address, for a client whose
    /// attach was refused and which therefore has no attachment to resume ([`attach_and_follow`]).
    fn resume(&self, previous: &str) {
        match self.attach_in_place(Attaching::Attached) {
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => self.fall_back_to(previous),
        }
    }

    /// THE switch sequence — stop the poll thread, re-attach where `to` says, and answer where this
    /// client LANDED. Every session switch this client makes goes through here.
    ///
    /// The poll thread is stopped FIRST, joined, so it can never refresh a mirror out from under
    /// the swap; `spawn_poll_for` (inside [`attach_in_place`](Self::attach_in_place)) installs the
    /// replacement. `take()` is bound to a local so the `self.poll` borrow is released before the
    /// blocking join — sound today either way (the joined thread never re-borrows `self.poll`), but
    /// it removes an `already borrowed` hazard should a future join path touch it.
    ///
    /// **A gesture that turns out to have nowhere to go still pays that teardown**, which is why
    /// [`resume`](Self::resume) exists and why the `None` arm cannot simply return: the client is
    /// staying where it was, and where it was has no poll thread any more. Missing that is a client
    /// that stops updating after a key that "did nothing" — the reason this is ONE function rather
    /// than the sequence written at each of the four call sites.
    ///
    /// TRACKED BOUND (responsiveness): this runs SYNCHRONOUSLY on the UI thread (the reducer) and
    /// does a thread join plus several blocking RPCs (connect + a read per pane), on a connection
    /// carrying [`REQUEST_DEADLINE`] — so a daemon that accepts but never answers costs this gesture
    /// that bound and not the window. ⚠ This note SAID `HostConn` had no read timeout and called the
    /// deadline a broader concern; both request connections have carried one since, and R343 found
    /// the sentence still here. **A tracked bound is a claim, and it expires.**
    fn switch_to(&self, to: Attaching<'_>) -> Option<String> {
        let previous = lock_session(&self.session).clone();
        let running = self.poll.borrow_mut().take();
        if let Some(mut poll) = running {
            poll.stop();
        }
        match self.attach_in_place(to) {
            Ok(landed @ Some(_)) => landed,
            // The daemon answered that there is nowhere to go (a ring with nothing in it, or no
            // last session), or refused a target that has no name to fall back on. Either way this
            // client stays where it was and its poll thread has to come back.
            Ok(None) => {
                self.resume(&previous);
                None
            }
            Err(error) => {
                tracing::warn!(
                    target: "sprag_gui::wire",
                    %error,
                    "session switch failed; staying on the previous session",
                );
                // `fall_back_to` and not `resume`: the attach may already have moved this client's
                // attachment before the read failed, so "where I am" is the wrong question and only
                // the NAME says which session the user was on.
                self.fall_back_to(&previous);
                None
            }
        }
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
    ///
    /// # An ACT that the daemon cannot perform reaches the person (R324)
    ///
    /// The trace was the whole of the policy until this round, and the register carried the
    /// consequence as an open surface decision: *"a RUNNING display client still SWALLOWS a
    /// skew ... whether a repaint loop should say so is a question about how noisy a degraded
    /// client is allowed to be."* Measured against a daemon serving every read and knowing no
    /// verb, `prefix c` on a live client left the status row unchanged and created no window.
    ///
    /// **The answer taken is: a person's GESTURE gets an answer, and a poll does not shout.** A
    /// `scene/invoke` happens only because somebody acted; a `scene/query` happens on every wake,
    /// and a client that reported each would have nothing else on its row.
    ///
    /// ⚠ **The DISCRIMINATOR is the FAULT, not the method** — a correction the revert-proof made,
    /// not the design's own claim: `UnknownInvokePath` is what a daemon answers an ACTION it does
    /// not have, so `unknown_action` already implies an invoke, and mutating the method check alone
    /// left every assertion green. The method test stays as a second, explicit guard — it says what
    /// this branch is FOR, and it costs a string compare — but the control that can fail is the one
    /// that swaps the fault matcher.
    ///
    /// The [`sprag_host::wire::skew_announcement`] sentence is the daemon-facing one both fronts
    /// already show for the shell, so no client writes words of its own.
    ///
    /// Only a SKEW takes the row. A transport error is the poll thread's business (it owns the
    /// detach edge) and a refusal the daemon MEANT already reaches the caller as a value.
    fn request(&self, method: &str, params: Value, ctx: &str) -> Option<Value> {
        let path = params
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        match self.conn.borrow_mut().try_call(method, params) {
            Ok(value) => Some(value),
            Err(error) => {
                if let sprag_rpc::CallError::Fault(fault) = &error
                    && method == "scene/invoke"
                    // TWO kinds of "your gesture did not happen", asked in the order of how much
                    // they tell a person: the daemon cannot perform this AT ALL (restart it), or it
                    // performed nothing and SAID WHY (fix the workspace). R325 added the second.
                    // Before it, a live client answered `prefix !` on a lone pane with its own
                    // generic *"break-pane: nowhere to go"* while the daemon was saying *"cannot
                    // break the only pane in a window"* — the client's word for four different
                    // situations, one of which it was in.
                    && let Some(said) = sprag_host::wire::skew_announcement(&path)
                        .filter(|_| sprag_host::wire::unknown_action(&path, fault).is_some())
                        .or_else(|| {
                            fault
                                .refusal()
                                .and_then(sprag_host::wire::refusal_announcement)
                        })
                {
                    store_message(&self.gesture_refused, said);
                }
                // Rendered as `call` renders it, through the conversion that exists for exactly
                // this: a caller that opted into the typed error must not spell a second wording.
                let error = io::Error::from(error);
                tracing::debug!(target: "sprag_gui::wire", ctx, %error, "wire request failed");
                None
            }
        }
    }

    /// Lock the shared pane cache (poison-tolerant, matching the rest of the wire
    /// client's lock discipline). The ONE place the cache lock is taken on the UI thread.
    fn lock_cache(&self) -> MutexGuard<'_, PaneCache> {
        lock_cache(&self.cache)
    }

    /// The cached live (offset 0) cell buffer for pane `id`, or a `1x1` placeholder for
    /// an absent id / before the first frame. Absent-id tolerance keeps every method's
    /// contract graceful, matching the in-process [`Host`](sprag_host::Host).
    fn live_cells(&self, id: PaneId) -> GridBuffer {
        self.lock_cache()
            .get(id)
            .map(|pane| pane.frame.cells.clone())
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }

    /// Perform a join — the ONE place this client spells the `join_pane` action.
    ///
    /// The keys are [`JoinAsk`]'s, built by the grammar rather than by a `json!` here: a hand-built
    /// object is the fifth-spelling shape this project has removed from `select_pane` and
    /// `swap_pane` already, and a client that spelled `window_id` its own way would be a second
    /// authority on which address it is sending. It takes the whole ask rather than the id so the
    /// day a display client has an honest reason to send a NAME, the arm is added HERE and not as a
    /// second `json!` beside this one.
    fn join(&self, ask: &JoinAsk) -> Option<bool> {
        let params = invoke(&mux_action_path(JOIN_PANE_ACTION), ask.to_args());
        let answer = self
            .request("scene/invoke", params, "join_pane")
            .and_then(|value| value.get("closed_source").and_then(Value::as_bool));
        if answer.is_some() {
            self.refresh_view();
        }
        answer
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
        if let Ok(size) = query_window_size(&mut conn) {
            store_window_size(&self.layout, size);
        }
    }

    /// Re-read every session on the UI-thread connection and store it — the immediate-feedback
    /// follow-up to this client's OWN kill of ANOTHER session, so the killed row leaves the sidebar
    /// without waiting a poll wake. Registry-wide (like the poll thread's own sessions re-read), so
    /// it does NOT detach on a scope refusal the way the scoped window/pane reads do — a transient
    /// failure just keeps the last-known list, which the poll thread's revision-bump re-read heals.
    /// Not used for the own-session kill: that detaches, so the sidebar it would refresh is going.
    /// Plan where this client goes when `killed` is destroyed.
    ///
    /// # One door, because three sites spelled the same two steps
    ///
    /// `kill_session`'s own-kill branch, `kill_window`'s cascade and the out-of-band resolve each
    /// read the policy and the session list and called [`destroy_successor`]. That is the drift
    /// shape this tree keeps paying to remove.
    ///
    /// # THE MIRROR ANSWERS THE ORDER; A FRESH READ ANSWERS THE OCCUPANCY
    ///
    /// R326 measured `no-detached` walking into a session another client was sitting in — the
    /// loser's row reading `[beta] 0:0*` with `beta` holding two clients, twice in five
    /// full-workspace runs. Its fallback ([`first_free_other`]) turns on each session's ATTACHED
    /// count, an attach bumps only the channel of the session ATTACHED TO, and a client parked on
    /// its own session is therefore never woken to re-read the count its policy depends on.
    ///
    /// The re-read that answers it was itself refused: by the time this runs `killed` is gone, this
    /// connection is scoped to it, and scope resolution gated every method — including a read whose
    /// subject is the whole REGISTRY and which needs no live session at all. R327 opened that door
    /// where it belonged, in the daemon ([`sprag_host::registry_scene`]), so the list can now be
    /// fetched at the exact moment the decision is made.
    ///
    /// Both readings are passed on, because [`destroy_successor`] needs both and for opposite
    /// reasons: the mirror still holds `killed`, which is the anchor `next` / `previous` count from
    /// and which no post-destroy list can supply, while only the fresh one knows who is sitting
    /// where. A read that FAILS falls back to the mirror — the behaviour every build before this
    /// one had, so a transient failure is no worse than the old floor and never turns a switch
    /// policy into a detach.
    fn plan_successor(&self, killed: &str) -> Successor {
        // Both halves of the mirror under ONE lock: the list and the place this client's row held
        // in it are answers to the same instant, and reading them separately would let a wake land
        // between and pair an order with an anchor from a different one.
        let (seen, anchor) = {
            let held = lock_sessions(&self.sessions);
            (held.list.clone(), held.anchor)
        };
        let now = self.live_sessions().unwrap_or_else(|| seen.clone());
        destroy_successor(
            detach_on_destroy(self.frontend.unset_destroy_policy()),
            &seen,
            anchor,
            &now,
            killed,
        )
    }

    /// Every session as of NOW, read on this client's own connection — or [`None`] when the daemon
    /// will not answer.
    ///
    /// Distinct from [`refresh_sessions`](Self::refresh_sessions), which STORES what it reads: this
    /// is a decision input and must not overwrite the mirror the sidebar is drawn from, because at
    /// the moment it runs that mirror is holding the one row this client still needs — the session
    /// that just died.
    // `#[must_use]` because an `Option` is not: R316's whole finding was a client left BYTE-FOR-BYTE
    // UNCHANGED by an outcome nothing read, and a read made for a decision and then dropped is that
    // shape exactly — it would look like the fix while the decision still ran on the mirror.
    #[must_use]
    fn live_sessions(&self) -> Option<Vec<SessionInfo>> {
        let mut conn = self.conn.borrow_mut();
        match query_sessions(&mut conn) {
            Ok(list) => Some(list),
            Err(error) => {
                tracing::debug!(
                    target: "sprag_gui::wire",
                    %error,
                    "live_sessions: the destroy decision falls back to the mirror's counts",
                );
                None
            }
        }
    }

    fn refresh_sessions(&self) {
        let mut conn = self.conn.borrow_mut();
        let viewing = lock_session(&self.session).clone();
        match query_sessions(&mut conn) {
            Ok(list) => store_sessions(&self.sessions, list, &viewing),
            Err(error) => tracing::debug!(
                target: "sprag_gui::wire",
                %error,
                "refresh_sessions: sessions re-read failed; keeping the last-known list",
            ),
        }
    }
}

/// How far a kill CASCADED, read off the daemon's answer — the ONE reader the three destructive
/// verbs share.
///
/// It was `kill_pane`'s alone until R325 widened `kill_window` and `kill_session` off `()`; three
/// copies of a four-line extraction is exactly the drift this tree keeps paying to remove, and the
/// `None` case has a subtlety worth stating once rather than three times.
///
/// **[`None`] is not [`Ended::Pane`]**, and never guessed at as one. An answer with no
/// [`ENDED_KEY`] can only come from a daemon older than the cascade, which
/// [`client/hello`](sprag_rpc::CLIENT_HELLO_METHOD) refuses by number — so it is reported as a kill
/// this client cannot describe. Guessing the lowest level would tell a user their session survived
/// when it did not, which is the one answer worse than silence.
fn ended_of(answer: &Value, ctx: &str) -> Option<Ended> {
    let ended = answer
        .get(ENDED_KEY)
        .and_then(Value::as_str)
        .and_then(Ended::from_wire);
    if ended.is_none() {
        tracing::debug!(
            target: "sprag_gui::wire",
            ctx,
            "the daemon performed a kill without saying what it ended",
        );
    }
    ended
}

/// The wire client is the ONLY host that plays this role for real: it is the only one with a daemon
/// that can route a person's message to it, and the only one whose session another process can
/// destroy while somebody is sitting in it.
impl sprag_host::wake::WakeSource for WireHost {
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
    /// or already gone from the mirror) detaches. `switch_session_named` joins the now-broken poll
    /// and attaches to the neighbour (whose own commit re-clears the flag).
    ///
    /// COVERAGE: the end-to-end switch here and in [`kill_session`](HostClient::kill_session)'s own
    /// branch is driven at the SHIPPED BINARIES rather than here — `sprag-tui`'s pty gate
    /// (`a_destroyed_session_moves_the_terminal_client_and_says_so`) and `sprag-gui`'s pixel smoke
    /// both kill this client's session out of band under `detach-on-destroy = next` and read what
    /// the client does. It was an accepted unit-test gap until R326, and what the gap was hiding
    /// was not a wrong switch: it was that ONE OF THE TWO FRONTS NEVER CALLED THIS AT ALL.
    fn resolve_lost_session(&self) -> Option<sprag_host::wake::Lost> {
        if !self.lost_session.swap(false, Ordering::AcqRel) {
            return None;
        }
        // Read BEFORE the follow: the switch re-points `self.session`, so the name of the session
        // that died is only readable up to this line. It is the half no re-read can recover — the
        // session is gone from every list the daemon serves.
        let was = lock_session(&self.session).clone();
        let plan = self.plan_successor(&was);
        // The LANDING, not the plan: `follow`'s fallbacks can end in a detach the policy did not
        // ask for, and the daemon names where a switch actually arrived.
        match self.follow(plan) {
            Some(now) => Some(sprag_host::wake::Lost::Moved { was, now }),
            None => Some(sprag_host::wake::Lost::Detached { was }),
        }
    }

    /// TAKE the message the poll thread collected — the client-side half of the hand-off the daemon
    /// started, and the reason both halves REMOVE rather than read.
    ///
    /// Served from the mirror the poll thread fills, so a surface asks for it on the reconcile it
    /// already does and never touches the wire from the paint path.
    // No `#[must_use]` here and none is missing: the TRAIT declares it, and an impl inherits it —
    // the compiler rejects the attribute in this position, which is the check that the rule lives
    // in one place.
    fn take_message(&self) -> Option<Announcement> {
        lock_message(&self.message).take()
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
        self.lock_cache()
            .panes()
            .iter()
            .map(|pane| pane.id)
            .collect()
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

    /// The live cells and the token they arrived with, read under ONE lock.
    ///
    /// One `lock_cache` and not two, which is the whole point: the poll thread replaces a pane's
    /// frame and its token together, so a caller taking them in two calls can straddle that swap
    /// and pair last wake's cells with this wake's token. It would then believe it had painted rows
    /// it never received — see [`HostClient::pane_frame`].
    ///
    /// A non-zero offset answers no token: this client's own scrolled fetch re-projects the screen
    /// windowed into scrollback, and the token summarises the LIVE screen. Answering the live
    /// token beside a scrolled buffer would let a painter skip rows it has never drawn. The row
    /// SHARES do come back for a scrolled read, because that fetch answers a whole
    /// [`CellFrame`] and the shares in it describe the rows it carries.
    fn pane_frame(&self, id: PaneId, offset_lines: usize) -> PaneFrame {
        if offset_lines != 0 {
            let params = json!({ "path": pane_input_path(id.0, &cells_slot_at(offset_lines)) });
            return self
                .request("scene/query", params, "pane_cells")
                .and_then(|value| serde_json::from_value::<CellFrame>(value).ok())
                .map_or_else(
                    || PaneFrame {
                        cells: self.live_cells(id),
                        shares: sprag_grid::RowShares::default(),
                        token: None,
                    },
                    |frame| PaneFrame {
                        cells: frame.cells,
                        shares: frame.facts.shares,
                        token: None,
                    },
                );
        }
        live_frame(&self.lock_cache(), id)
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

    /// The pane the daemon says the window is ON, read from the PANE mirror — a lock, no socket
    /// call, for [`Self::layout`]'s reason: a client re-tiles on every wake, and a round trip here
    /// would put one on that path.
    ///
    /// `None` while the mirror holds no active pane, which covers three cases a client treats
    /// alike: an empty window, a mirror not yet booted, and a daemon old enough to have no active
    /// pane at all. In every one of them the client keeps its own focus, which is exactly what it
    /// did before this fact existed.
    fn active_pane(&self) -> Option<PaneId> {
        self.lock_cache()
            .panes()
            .iter()
            .find(|pane| pane.active)
            .map(|pane| pane.id)
    }

    /// Publish the user's move to the daemon (`select_pane`), so every attached client follows and a
    /// pane verb given no target acts where the user is.
    ///
    /// Sends the pane by ID because the caller PICKED THAT PANE OUT — a click, a focus ring the
    /// user cycled, the pane a split just opened. A DIRECTION is the other kind of request and
    /// takes the other arm of the same action: see [`Self::select_toward`], which names no pane at
    /// either end.
    ///
    /// The request is built by [`SelectAsk`], not by a `json!` here: this crate is the one both
    /// frontends share, so a key spelled by hand in it is a key that can drift away from the daemon
    /// that reads it while every test on both sides stays green.
    fn select_pane(&self, id: PaneId) -> bool {
        self.request(
            "scene/invoke",
            invoke(
                &mux_action_path(SELECT_PANE_ACTION),
                SelectAsk::Pane(id).to_args(),
            ),
            "select_pane",
        )
        .is_some()
    }

    /// Send the DIRECTION and let the daemon walk its own arrangement — the same action
    /// `sprag select-pane -L` invokes, so a keystroke and a shell command are one code path.
    ///
    /// The reply carries the pane the window is on afterwards, which is what a caller adopts; a
    /// direction with no neighbour answers the unmoved pane rather than a fault, so an arrow key
    /// held against the edge of a layout is quiet.
    ///
    /// **It also carries an `outcome` word this client DROPS** (`sprag_host::wire::SelectHow`: an
    /// edge and a floating active pane read differently there). Deliberate, and recorded here rather
    /// than left for a reader to wonder about: the trait answers where to put the focus ring, and
    /// nothing that draws one has anything to SAY about why it did not move. The day a client wants
    /// "you are at the edge", this signature is where the fact stops.
    fn select_toward(&self, dir: PaneDir) -> bool {
        let Some(answer) = self.request(
            "scene/invoke",
            invoke(
                &mux_action_path(SELECT_PANE_ACTION),
                // No ORIGIN, and that is the whole of what a display client means: this call comes
                // from a KEYPRESS, and a keypress is always "from where I am". The argument exists
                // for a caller that is not the person — an agent reasoning about the pane beside its
                // own — which is why it reaches the MCP tool and the CLI verb and stops there.
                SelectAsk::Toward { dir, from: None }.to_args(),
            ),
            "select_toward",
        ) else {
            return false;
        };
        // `changed` and not the pane id: the daemon answers the pane the window is on either way,
        // so the id says nothing about whether the key did anything. This key is the one the swap
        // and the resize beside it already read this way.
        answer["changed"].as_bool().unwrap_or(false)
    }

    /// Send the DIRECTION and let the daemon trade the two leaves — the same action
    /// `sprag swap-pane -L` invokes, so a keystroke and a shell command are one code path.
    ///
    /// [`Self::select_toward`]'s twin in every respect, including what it DROPS: the answer names
    /// both panes and carries an `outcome` word (`sprag_host::wire::SwapHow` — an edge, a floating
    /// origin and a pane traded with itself all read differently there), and this keeps only
    /// whether the arrangement moved, because nothing that draws one has anything to SAY about why
    /// it did not.
    ///
    /// The origin is absent for that method's reason: a keypress is always "the pane I am on".
    fn swap_toward(&self, dir: PaneDir) -> bool {
        self.request(
            "scene/invoke",
            invoke(
                &mux_action_path(SWAP_PANE_ACTION),
                SwapAsk::Toward { pane: None, dir }.to_args(),
            ),
            "swap_toward",
        )
        .is_some_and(|answer| SwapHow::read(&answer, Some(dir)).changed())
    }

    /// The boundary beside the active pane, moved `cells` cells `dir` — the client half of
    /// `resize-pane -L|-R|-U|-D`.
    ///
    /// The outcome word is read rather than the `changed` key, because this action has no such key:
    /// a resize has FIVE outcomes and only one of them moved anything, so a boolean beside them
    /// would be a second encoding of one fact. An answer this build cannot read is `false` — the
    /// honest reduction, since a client that cannot tell whether the arrangement moved must not
    /// claim it did.
    fn resize_toward(&self, dir: PaneDir, cells: u16) -> bool {
        self.request(
            "scene/invoke",
            invoke(
                &mux_action_path(RESIZE_PANE_ACTION),
                ResizeAsk {
                    pane: None,
                    dir,
                    cells,
                }
                .to_args(),
            ),
            "resize_toward",
        )
        .and_then(|answer| ResizeHow::from_wire(answer[sprag_host::wire::OUTCOME_KEY].as_str()?))
        .is_some_and(ResizeHow::changed)
    }

    /// The session's arbitrated window, read from the same mirror as the arrangement — a lock, no
    /// socket call, because this is on the paint path beside [`Self::layout`].
    fn window_size(&self) -> Option<(u16, u16)> {
        lock_layout(&self.layout).window_size
    }

    /// Report this client's cell area, and mirror the window it produces IMMEDIATELY.
    ///
    /// The immediate re-read is what makes a resize feel local rather than arriving a poll later:
    /// under the default `latest` policy this client's own report IS the new window, so waiting for
    /// the poll's wake would leave one frame tiled over the old rectangle. It is the same
    /// immediate-feedback shape the window and session writes already use.
    fn report_client_size(&self, cols: u16, rows: u16) {
        let mut conn = self.conn.borrow_mut();
        send_size(&mut conn, cols, rows);
        if let Ok(size) = query_window_size(&mut conn) {
            drop(conn);
            store_window_size(&self.layout, size);
        }
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

    /// The NAMED select, and it adopts the daemon's answer exactly as the step below does.
    ///
    /// **The daemon has always answered the window it landed on** — the handler's own comment says
    /// giving the named arm the step's answer "is what keeps one shape for one verb" — and this
    /// client discarded it until R316, which is what made `select-window -t <a name that is not
    /// there>` indistinguishable from a success at every surface above. Nothing on the wire
    /// changed; what changed is that the fact is now carried.
    ///
    /// ⚠ **The load-bearing half is the `?`, not the `as_str`**, and that was MEASURED rather than
    /// assumed: a revert-proof that replaced the answer with the caller's own argument left the
    /// live test GREEN, because an unknown name is REFUSED at the daemon and the refusal
    /// short-circuits here. Reading the answer is kept for the shape the step arm has — one verb,
    /// one reply — and for the day the daemon's recorded spelling stops being the argument.
    fn select_window(&self, window: &sprag_host::wire::WindowRef) -> Option<String> {
        let params = invoke(
            &mux_action_path(SELECT_WINDOW_ACTION),
            SelectWindowAsk::At(window.clone()).to_args(),
        );
        let landed = self.request("scene/invoke", params, "select_window")?;
        self.refresh_view();
        landed.as_str().map(str::to_owned)
    }

    /// The RING walk, asked of the daemon and adopted from its answer — never resolved against this
    /// client's `windows` mirror, which can be a revision behind the session it would be naming.
    ///
    /// The view is refreshed exactly as a named select refreshes it: the current window changed, so
    /// every mirror this client projects from is about a different window now.
    fn select_window_toward(&self, step: OrderStep) -> Option<String> {
        let params = invoke(
            &mux_action_path(SELECT_WINDOW_ACTION),
            SelectWindowAsk::Step(step).to_args(),
        );
        let landed = self.request("scene/invoke", params, "select_window_toward")?;
        self.refresh_view();
        landed.as_str().map(str::to_owned)
    }

    /// The move, sent with the client's own `window` argument or NONE at all — a client never
    /// resolves "the current window" itself, for [`HostClient::move_window`]'s stated reason.
    ///
    /// The view is refreshed on every outcome, not only on [`PlaceHow::Moved`]: the strip this
    /// client paints reads the window order, and a client deciding for itself when that order is
    /// worth re-reading is a second answer to a question the daemon already answered.
    ///
    /// An answer this build cannot READ (a daemon too old to serve the verb refuses it before this
    /// point; one that answered a word this build's [`PlaceHow`] lacks) is [`None`] — never a
    /// guessed `Moved`, which would tell a user their window went somewhere it did not.
    fn move_window(&self, window: Option<&str>, place: &WindowPlace) -> Option<(String, PlaceHow)> {
        let ask = MoveWindowAsk {
            window: window.map(str::to_owned),
            place: place.clone(),
        };
        let params = invoke(&mux_action_path(MOVE_WINDOW_ACTION), ask.to_args());
        let answer = self.request("scene/invoke", params, "move_window")?;
        self.refresh_view();
        MoveWindowAsk::read_answer(&answer)
    }

    /// Pin the CURRENT window's size over the wire, answering what the daemon stored and the policy
    /// it is under.
    ///
    /// The ask names NO window — [`HostClient::resize_window`]'s rule — so the daemon resolves *the
    /// one this connection is scoped to* under its own lock, where this client's mirror can be a
    /// revision behind. That is the same discipline `rename_window` and `move_window`'s bare form
    /// keep, and it is why the [`ResizeWindowAsk`] built here has `window: None` rather than a name
    /// read back off the layout mirror.
    ///
    /// The view is refreshed BEFORE the answer is read, so a caller that paints on the strength of
    /// the returned pin is painting over a mirror the resize has already reached — the shape every
    /// acting method here keeps.
    fn resize_window(&self, size: sprag_host::window::SizeRequest) -> Option<WindowPin> {
        let params = invoke(
            &mux_action_path(RESIZE_WINDOW_ACTION),
            ResizeWindowAsk { window: None, size }.to_args(),
        );
        let answer = self.request("scene/invoke", params, "resize_window")?;
        self.refresh_view();
        Some(WindowPin::read(&answer))
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

    /// Kill a window over the wire, answering how far the kill CASCADED —
    /// [`HostClient::kill_pane`]'s shape one level up, and read through the same reader: the
    /// word is the DAEMON's and is never re-derived from a mirror this client holds.
    ///
    /// [`HostClient::kill_window`] carries why this answers a value at all; what belongs here is
    /// that the answer arrives on the SAME reply as the act, which matters more than usual for this
    /// verb: killing a session's last window ends the session, and a client that asked afterwards
    /// would be asking a daemon that may have exited.
    fn kill_window(&self, window: sprag_terminal::WindowId) -> Option<Ended> {
        // Plan the successor BEFORE the kill, for the reason [`kill_session`](Self::kill_session)
        // states: `next` / `previous` and the MRU fallbacks must resolve against the session list
        // the PERSON can see, and a cascade takes our own session out of it. Pure over the mirror,
        // so a kill that stops at the window pays a clone and throws the plan away.
        let me = lock_session(&self.session).clone();
        let plan = self.plan_successor(&me);
        // The IDENTITY is what crosses, never a name this client read off a mirror at some earlier
        // instant — `WindowRef`'s whole reason, and the keys are the grammar's rather than a
        // `json!` here for `join`'s stated reason one method over.
        let mut args = serde_json::Map::new();
        WindowRef::Picked(window).write(&mut args);
        let params = invoke(&mux_action_path(KILL_WINDOW_ACTION), Value::Object(args));
        let answer = self.request("scene/invoke", params, "kill_window")?;
        let ended = ended_of(&answer, "kill_window");
        // OUR SESSION WENT WITH THE WINDOW — take the deterministic own-kill path here rather than
        // leaving the move to the poll thread's out-of-band flag (R326).
        //
        // It read as an ordinary cascade until this round put a SENTENCE on the out-of-band path,
        // and then said the wrong thing out loud: `prefix &` answered *"the session went with it"*
        // and, 150 ms later, *"session \"0\" was destroyed"* — a passive sentence about a destroy
        // this very keyboard had just asked for, replacing the gesture's own answer a fifth of the
        // way into its `display-time`. Two answers to one gesture, and the second blamed nobody
        // while implying somebody.
        //
        // Following HERE fixes both halves at once: the switch happens inside the dispatch that
        // caused it, so the status row this client paints on the very next frame already names
        // where it landed — which is why the gesture needs no second sentence and gets none
        // (`sprag_host::report`'s own rule that a LANDING is not a message). The poll may have
        // flagged the loss in the gap; `follow` joins that thread on its way through and the
        // attach clears the flag, so nothing fires twice.
        if matches!(ended, Some(Ended::Session | Ended::Server)) {
            // Discarded for [`kill_session`](HostClient::kill_session)'s reason, stated there.
            let _ = self.follow(plan);
            return ended;
        }
        self.refresh_view();
        ended
    }

    /// The CURRENT window's rename, sent with NO target so the daemon resolves it under its own
    /// lock — see [`HostClient::rename_window`] for why a client must not name it.
    ///
    /// The view is refreshed because the window strip and every window-named read this client
    /// paints are about to be wrong. The answer is the daemon's recorded name; `null` from a
    /// pre-R306 daemon reads as a refusal here, which is the honest degradation — a client one
    /// protocol number ahead cannot tell that apart from a rejection, and the number exists so it
    /// never has to.
    fn rename_window(&self, name: &str) -> Option<String> {
        let params = invoke(
            &mux_action_path(RENAME_WINDOW_ACTION),
            json!({ "name": name }),
        );
        let recorded = self.request("scene/invoke", params, "rename_window")?;
        self.refresh_view();
        recorded
            .get("name")
            .and_then(|name| name.as_str())
            .map(str::to_owned)
    }

    /// The SCOPE's rename — the session this connection is attached to (R303), never a name this
    /// client read out of its mirror.
    fn rename_session(&self, name: &str) -> Option<String> {
        let params = invoke(
            &mux_action_path(RENAME_SESSION_ACTION),
            json!({ "name": name }),
        );
        let recorded = self.request("scene/invoke", params, "rename_session")?;
        self.refresh_view();
        recorded.as_str().map(str::to_owned)
    }

    /// One pane's rename, addressed BY ID — the only one of the three with an identity to carry.
    fn rename_pane(&self, id: PaneId, name: &str) -> Option<String> {
        let params = invoke(
            &mux_action_path(RENAME_PANE_ACTION),
            json!({ "pane": id.0, "name": name }),
        );
        let recorded = self.request("scene/invoke", params, "rename_pane")?;
        self.refresh_view();
        recorded
            .get("name")
            .and_then(|name| name.as_str())
            .map(str::to_owned)
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

    /// Fill the target's window with it alone, or give the arrangement back, over the wire.
    ///
    /// The pane is NAMED even though the action would default an absent one to the session's active
    /// pane: this call serves a gesture that happened on a pane, and the daemon's active pane can
    /// move between the gesture and the request.
    ///
    /// # Why the arrangement is re-read here, and only when something moved
    ///
    /// A zoom changes what this client draws — the projection is the arrangement filtered by the
    /// zoomed pane — but `zoom_pane` answers `{pane, zoomed, changed}`, not a `LayoutSnapshot`, so
    /// there is nothing to install the way `set_layout` and `set_floating` install their answers.
    /// The mirror is therefore refreshed from the LAYOUT slot it already reads, rather than by
    /// teaching a second answer to carry an arrangement — one reader of that encoding, which is what
    /// keeps a client from coming to disagree with the daemon about it.
    ///
    /// The caller that NEEDS it is `sprag-tui`, not the GUI: its key arm calls this and then
    /// reconciles and paints in the same breath, off `HostClient::layout` — this mirror. Without the
    /// re-read it would paint the pre-zoom tiling and correct itself only on the next poll wake, a
    /// visible lag on the user's own keystroke. The GUI converges either way, through the wake the
    /// daemon sends when `changed`.
    ///
    /// **And that is why NO TEST PINS THIS.** Both frontends end up correct, so every assertion
    /// available — the pixel smoke's included, measured by removing this block and watching it stay
    /// green — is satisfied with or without it. It is kept because the TUI's synchronous reconcile
    /// is a design already in force, not because anything here proves it; recorded rather than
    /// claimed.
    ///
    /// ⚠ **The debt register predicted that a LIVE TUI ZOOM TEST would close this, and R319 built
    /// one and MEASURED the prediction wrong**: with `the_zoom_key_gives_the_focused_pane_the_whole_
    /// area_and_gives_it_back` driving a real `sprag-tui` through `prefix z`, deleting this block
    /// still leaves it green. What it protects is ONE FRAME of lag on the person's own keystroke,
    /// and a test that waits for a condition cannot see a frame that corrected itself. So this is a
    /// DECISION with a stated cost, not a gap waiting for a harness — which is the honest end of the
    /// story rather than a test somebody keeps expecting.
    ///
    /// A re-assertion that moved nothing skips the read entirely: `changed` is exactly the fact that
    /// says whether there is anything to re-read.
    fn zoom_pane(&self, target: PaneId, on: Option<bool>) -> Option<ZoomOutcome> {
        let mut args = json!({ "pane": target.0 });
        if let Some(on) = on {
            args["on"] = json!(on);
        }
        let answer = self.request(
            "scene/invoke",
            invoke(&mux_action_path(ZOOM_PANE_ACTION), args),
            "zoom_pane",
        )?;
        let outcome = ZoomOutcome {
            zoomed: answer.get("zoomed")?.as_bool()?,
            changed: answer.get("changed")?.as_bool()?,
        };
        if outcome.changed {
            let current = lock_layout(&self.layout).window.clone();
            let mut conn = self.conn.borrow_mut();
            match query_layout(&mut conn) {
                Ok(snapshot) => store_layout(&self.layout, &current, snapshot),
                Err(error) => {
                    tracing::debug!(target: "sprag_gui::wire", %error, "zoom_pane: layout re-read failed");
                }
            }
        }
        Some(outcome)
    }

    /// Close pane `id` over the wire, answering how far the kill CASCADED. The daemon answers
    /// `Rejected` for an absent pane, which arrives here as the absent request result — so "no such
    /// pane" and "the socket failed" are both [`None`], which is the same conflation every other
    /// write on this client accepts.
    ///
    /// The word is the DAEMON's ([`ENDED_KEY`]), never re-derived here from a mirror: whether a
    /// window survived its last pane is a fact only the process that performed the kill holds, and
    /// a client that counted its own tiles would answer from a snapshot taken before the kill.
    ///
    /// An answer with no such key can only come from a daemon older than the cascade, which
    /// [`client/hello`](sprag_rpc::CLIENT_HELLO_METHOD) refuses by number — so it is reported as a
    /// kill this client cannot describe rather than guessed at as [`Ended::Pane`], the guess that
    /// would tell a user their session survived when it did not.
    fn kill_pane(&self, id: PaneId) -> Option<Ended> {
        let params = invoke(&mux_action_path(CLOSE_ACTION), json!({ "id": id.0 }));
        let answer = self.request("scene/invoke", params, "kill_pane")?;
        self.refresh_view();
        ended_of(&answer, "kill_pane")
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

    /// Put pane `id` beside `target` over the wire — tmux `move-pane`, the action a chooser's pick
    /// commits to since R328.
    ///
    /// **Two pane IDENTITIES and no name**, which is why this verb was the one of the keyboard's
    /// four that needed no wire change: the action has always resolved both panes itself, so what a
    /// person picked is what is sent. `refresh_view` on success for the join arm below its reason —
    /// the pane SET of this client's window changed, and waiting a poll wake would paint the old
    /// one.
    fn move_pane(
        &self,
        id: PaneId,
        target: PaneId,
        dir: sprag_terminal::SplitDir,
        before: bool,
    ) -> Option<bool> {
        let params = invoke(
            &mux_action_path(MOVE_PANE_ACTION),
            json!({
                "pane": id.0,
                "target": target.0,
                "dir": match dir {
                    sprag_terminal::SplitDir::Horizontal => "horizontal",
                    sprag_terminal::SplitDir::Vertical => "vertical",
                },
                "before": before,
            }),
        );
        let answer = self
            .request("scene/invoke", params, "move_pane")
            .and_then(|value| value.get("closed_source").and_then(Value::as_bool));
        if answer.is_some() {
            self.refresh_view();
        }
        answer
    }

    fn join_pane_into(&self, id: PaneId, dst: sprag_terminal::WindowId) -> Option<bool> {
        self.join(&JoinAsk {
            pane: id,
            window: WindowRef::Picked(dst),
        })
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
        lock_sessions(&self.sessions).list.clone()
    }

    /// The mirrored activity, AGED FORWARD to now — a lock and a clone, never a socket call, for the
    /// same reason the list above is not fetched here.
    ///
    /// The age this returns is the daemon's `sampled_ms_ago` PLUS however long the reading has sat
    /// in this mirror, which is the only honest answer: a client parked for a minute holds a
    /// minute-old subtitle, whatever the daemon said when it handed it over. Reporting the daemon's
    /// number alone would make every reading look fresh and turn the age into decoration.
    ///
    /// Nothing mirrored yet — a boot whose best-effort read failed, or a daemon too old to serve the
    /// family — answers an empty reading of age zero. That is the honest shape for "no rows", not a
    /// claim of freshness: there is nothing here whose age could be wrong.
    fn session_activity(&self) -> sprag_terminal::ActivityReading {
        lock_activity(&self.activity).as_ref().map_or_else(
            || sprag_terminal::ActivityReading {
                age: Duration::ZERO,
                value: Vec::new(),
            },
            |entry| sprag_terminal::ActivityReading {
                age: entry.reading.age + entry.arrived.elapsed(),
                value: entry.reading.value.clone(),
            },
        )
    }

    /// The session this client is attached to — a client-local fact (the wire carries no
    /// "attached" marker), re-pointed by [`switch_session`](HostClient::switch_session).
    fn current_session(&self) -> String {
        lock_session(&self.session).clone()
    }

    /// Switch this client to the session named `name` IN PLACE (tmux `switch-client`): stop the
    /// running poll thread — joined FIRST, so it can never refresh a mirror out from under the swap
    /// — then re-attach to `name` (`attach_in_place`). A no-op for the
    /// already-current session. On failure, fall back to the session we were on so the window keeps
    /// serving; if THAT is gone too (killed while we tried to switch), detach — the tmux rule when a
    /// client can serve no session.
    ///
    /// TRACKED BOUND (responsiveness): this runs SYNCHRONOUSLY on the UI thread (the reducer) and
    /// does a thread join plus several blocking RPCs (connect + a read per pane), on a connection
    /// carrying this crate's request deadline, so the cost of a silent daemon is that bound rather
    /// than the window. See `switch_to` for why this note used to say otherwise.
    fn switch_session(&self, name: &str) {
        if name == lock_session(&self.session).as_str() {
            return;
        }
        self.switch_to(Attaching::Named(name));
    }

    /// The ring is the DAEMON's, so this sends a DIRECTION and reads back the name it landed on —
    /// see the trait method. It does not short-circuit the way
    /// [`switch_session`](HostClient::switch_session) does: a step cannot know in advance that it
    /// wraps onto the session it started from, and the daemon answering that name IS the answer.
    fn switch_session_toward(&self, step: OrderStep) -> Option<String> {
        self.switch_to(Attaching::Step(step))
    }

    /// The history is the DAEMON's ([`AttachAsk::LastViewed`]) and this client keeps none of its
    /// own. It used to: a `Vec<String>` of names maintained by nothing, which R304 measured walking
    /// straight into a stranger's session — the visited session had been renamed and a new one had
    /// taken its name, so "take me back where I was" attached this client to a session it had never
    /// seen, on the connection it types down.
    fn switch_session_last(&self) -> Option<String> {
        self.switch_to(Attaching::LastViewed { unattached: false })
    }

    /// [`switch_session`](HostClient::switch_session)'s answering form, for a caller that has to
    /// tell a user whether their typed name landed. It keeps that method's short-circuit, so
    /// asking for the session you are already on answers that session rather than paying a
    /// re-attach.
    fn switch_session_named(&self, name: &str) -> Option<String> {
        let here = lock_session(&self.session).clone();
        if name == here {
            return Some(here);
        }
        self.switch_to(Attaching::Named(name))
    }

    /// The registry-wide tree, read fresh from the daemon rather than off this client's mirror.
    ///
    /// A FRESH READ and not a poll, deliberately: nothing else in this client needs the other
    /// sessions' windows, so mirroring them would make every poll wake pay for a fact one keystroke
    /// a day asks for. See [`sprag_host::wire::TREE_SLOT`].
    fn tree(&self) -> Vec<sprag_terminal::TreeSession> {
        self.request(
            "scene/query",
            json!({ "path": mux_action_path(TREE_SLOT) }),
            "tree",
        )
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default()
    }

    /// Go where a chooser's row points, by IDENTITY.
    ///
    /// It takes the SWITCH path even when the pick names the session this client is already on,
    /// where [`switch_session_named`](HostClient::switch_session_named) short-circuits — because a
    /// pick can also name a window or a pane of that session, which is a real act with nothing to
    /// short-circuit. The daemon's own `Unchanged` outcome makes the attach half free.
    fn goto(&self, target: Target) -> Option<String> {
        self.switch_to(Attaching::Goto(target))
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
    /// The answer carries how far the kill CASCADED, and R325 stopped throwing it away. The note
    /// this replaces said it was *"intentionally ignored (see the detach note)"* — true of the
    /// SEVERED case and of nothing else: the two branches that get a reply get [`Ended`] out of it,
    /// and only the branch that is leaving anyway cannot. [`Ended::Server`] is the one this buys
    /// outright: killing the LAST session ends the daemon, which no re-read can report because
    /// there is nothing left to read.
    fn kill_session(&self, name: &str) -> Option<Ended> {
        let params = invoke(
            &mux_action_path(KILL_SESSION_ACTION),
            json!({ "name": name }),
        );
        let is_own = name == lock_session(&self.session).as_str();
        // For an OWN kill under a switch policy, plan the successor NOW — BEFORE the kill removes
        // `name` from the list, so `next`/`previous` and the fallbacks resolve against the list the
        // user can see. A kill of ANOTHER session never switches this client, so it plans nothing.
        let successor = is_own.then(|| self.plan_successor(name));
        if let Some(plan) = successor.filter(|plan| *plan != Successor::Detach) {
            // switch-to-next. STOP the poll thread BEFORE the kill so the own-kill switch is
            // DETERMINISTIC and self-contained. Killing `name` bumps the scene revision, waking the
            // poll (still scoped to the dying session) into a re-query the host now REFUSES; under a
            // switch policy its error arm takes the OUT-OF-BAND path — it flags `lost_session` and
            // repaints (NOT `request_quit`; that is only `HostGone`) — whose reconcile would then
            // RACE the switch we are about to do (a flag `attach_in_place` would have to clear again).
            // Joining the poll first means no flag is ever raised for our OWN kill, and no wasted
            // re-query/repaint on the dead scope. With the poll gone, kill, then switch:
            // [`switch_session`] attaches to the successor and, if that fails (it died in the gap),
            // falls back to the now-dead `name` and so detaches — the correct end state when there
            // is nothing left to serve.
            let running = self.poll.borrow_mut().take();
            if let Some(mut poll) = running {
                poll.stop();
            }
            let ended = self
                .request("scene/invoke", params, "kill_session")
                .and_then(|answer| ended_of(&answer, "kill_session"));
            // The landing is DISCARDED, and that is `sprag_host::report`'s rule rather than an
            // oversight: a person who asked to kill their own session gets `ended` for how far it
            // reached, and the status row they are looking at already names where they now are. The
            // sentence exists for the destroy NOBODY at this keyboard asked for.
            let _ = self.follow(plan);
            return ended;
        }
        let ended = self
            .request("scene/invoke", params, "kill_session")
            .and_then(|answer| ended_of(&answer, "kill_session"));
        if is_own {
            // Own kill with nothing to switch to → DETACH. The reply may have been SEVERED by the
            // daemon's own exit, which is success and is why `ended` is `None` here as often as not
            // — the client is leaving either way, so there is nobody left to tell.
            self.quit.request_quit();
        } else {
            // Another session killed → keep serving ours; drop the killed row now.
            self.refresh_sessions();
        }
        ended
    }

    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts {
        self.lock_cache()
            .get(id)
            .map(|pane| pane.frame.facts.clone())
            .unwrap_or(PaneScrollFacts::absent())
    }

    fn pane_grid_size(&self, id: PaneId) -> (u16, u16) {
        self.lock_cache()
            .get(id)
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
        if self.request("scene/invoke", params, "resize").is_some() {
            self.lock_cache().set_dims(id, (cols, rows));
        }
    }

    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool {
        let args = json!({
            "key": key,
            "ctrl": mods.ctrl,
            "alt": mods.alt,
            "shift": mods.shift,
            "super": mods.sup,
            sprag_terminal::Hand::WIRE_KEY: sprag_terminal::Hand::APerson.word(),
        });
        self.request(
            "scene/invoke",
            invoke(&pane_input_path(id.0, KEY_ACTION), args),
            "send_key",
        )
        .is_some()
    }

    fn send_text(&self, id: PaneId, text: &str) -> bool {
        let params = invoke(
            &pane_input_path(id.0, TEXT_ACTION),
            json!({
                "text": text,
                sprag_terminal::Hand::WIRE_KEY: sprag_terminal::Hand::APerson.word(),
            }),
        );
        self.request("scene/invoke", params, "send_text").is_some()
    }

    fn paste(&self, id: PaneId, text: &str) -> bool {
        // Forward the raw text; the host brackets it (and filters an embedded end marker) if the
        // pane's child has enabled DEC private mode 2004. This client cannot see the pane's input
        // modes, so the bracketing decision stays at the PTY boundary.
        let params = invoke(
            &pane_input_path(id.0, PASTE_ACTION),
            json!({
                "text": text,
                sprag_terminal::Hand::WIRE_KEY: sprag_terminal::Hand::APerson.word(),
            }),
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

    /// Why the daemon's agent manifests are not the user's, over the mux `agent_manifests` slot. On
    /// demand (a palette opening, a `sprag agent`), never per frame.
    ///
    /// TWO answers where its two neighbours have three, and the missing one is the success value:
    /// the ruleset never crosses this wire, so there is nothing to deserialise and `Null` and "no
    /// problem" are the same reading. The error travels ALREADY RENDERED and is passed through
    /// verbatim, for the reason [`Self::global_commands`] states.
    fn agent_manifest_report(&self) -> Option<String> {
        let params = json!({ "path": mux_action_path(AGENT_MANIFESTS_SLOT) });
        let value = self.request("scene/query", params, "agent_manifest_report")?;
        value
            .get("error")
            .and_then(Value::as_str)
            .map(str::to_owned)
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
            .get(id)
            .map(|pane| pane.label.clone())
            .unwrap_or_default()
    }

    /// Served from the mirror the poll thread refreshes (no socket round-trip on the
    /// paint path); the title re-adopts the host's on every wake, so it tracks a shell
    /// rewriting it each prompt.
    fn pane_title(&self, id: PaneId) -> Option<String> {
        self.lock_cache()
            .get(id)
            .and_then(|pane| pane.title.clone())
    }

    /// Served from the same poll-refreshed mirror as [`Self::pane_title`], re-adopted each wake, so
    /// another client's `rename-pane` reaches this one's header without it asking.
    fn pane_name(&self, id: PaneId) -> Option<String> {
        self.lock_cache().get(id).and_then(|pane| pane.name.clone())
    }

    /// Served from the same poll-refreshed mirror as [`Self::pane_title`], re-adopted each wake,
    /// so the `seq` reflects the host's latest.
    fn pane_notification(&self, id: PaneId) -> Option<PaneNotification> {
        self.lock_cache()
            .get(id)
            .and_then(|pane| pane.notification.clone())
    }

    /// Take the skew this client's own act met.
    ///
    /// Served from a mirror of its OWN, not from the message mailbox above: that one holds what the
    /// daemon routed, is copied out to the desktop notifier, and is drained on a wake — and a
    /// daemon too old to act bumps no channel, so no wake ever comes.
    fn take_gesture_refusal(&self) -> Option<Announcement> {
        lock_message(&self.gesture_refused).take()
    }

    /// The pane's agent verdict (H3), served from the same poll-refreshed mirror as
    /// [`Self::pane_notification`] and re-adopted each wake — so a state that MOVED reaches a title
    /// on the wake that carried it, and a pane whose agent exited stops claiming one.
    ///
    /// Wake-stale by exactly one poll, which is the same staleness every other fact on this mirror
    /// carries and is bounded by the same thing: the daemon bumps the scene revision when a verdict
    /// publishes (H3's D9 schedules a bump for the settle window's expiry precisely so an absence-based
    /// state does not wait for the next keystroke), so the wake this reads is the one the change
    /// caused.
    fn pane_agent(&self, id: PaneId) -> Option<PaneAgent> {
        self.lock_cache()
            .get(id)
            .and_then(|pane| pane.agent.clone())
    }

    /// One `lock_cache` and not N+1, which is the whole point — see the trait's
    /// [`pane_agents`](HostClient::pane_agents) for what the composed walk gets wrong here.
    ///
    /// The composed default takes this lock once for the id list and again for each id, and the
    /// POLL THREAD replaces this cache wholesale between any two of those acquisitions. So the
    /// walk could report a pane the current cache no longer holds, and miss one it does. Reading
    /// the membership and the verdicts off ONE guard is what makes the answer a single moment's.
    ///
    /// Measured R262/R263 and re-measured after this change (R265, `sprag-tui`'s
    /// `examples/title-cost.rs`, release, against a real daemon). The walk runs on every repaint —
    /// every KEYSTROKE for `sprag-tui` (R246) — because the equality skip is at the OSC and not
    /// here, so what it costs is worth knowing:
    ///
    /// | panes | walk, no pane claimed | walk, every pane claimed |
    /// |---|---|---|
    /// | 63 | 1.943 us → **0.100 us** | 10.851 us → **8.427 us** |
    /// | 100 | 3.794 us → **0.172 us** | 18.419 us → **14.200 us** |
    ///
    /// The two columns move by 20x and 1.3x for the same change, and the gap is the finding: the
    /// quadratic scan is ALL of the empty branch and a minority of the populated one, which is
    /// dominated by cloning each claimed pane's three `String`s. That clone is not a leftover to
    /// remove — handing owned data out from behind a lock is what this seam costs, and the
    /// alternative (running a caller's closure while holding the cache lock) is worse.
    ///
    /// What matters more than either number: the empty branch grew 1.95x for 1.59x the panes before
    /// and grows 1.72x now. R264 removed the wire's 62-pane cap, so nothing bounds the count any
    /// more — LINEAR is the property that makes the cost safe at a count nobody has measured.
    fn pane_agents(&self) -> Vec<(PaneId, PaneAgent)> {
        self.lock_cache()
            .panes()
            .iter()
            .filter_map(|pane| pane.agent.clone().map(|agent| (pane.id, agent)))
            .collect()
    }

    /// The cache's own generation — a token this client CAN promise, because every fact
    /// [`Self::pane_agents`] reads lives in that cache and the cache counts its own changes
    /// (this module's `PaneCache::agents_generation`).
    ///
    /// That is what makes the key complete without enumerating inputs: a verdict, a pane
    /// appearing, a pane leaving and a state moving all arrive the same way — as new contents —
    /// so none of them can slip past. It is deliberately not the SCENE revision, which would be
    /// a promise about a number this client does not own.
    fn pane_agents_token(&self) -> Option<u64> {
        Some(self.lock_cache().agents_generation())
    }

    /// Served from the same poll-refreshed mirror as [`Self::pane_notification`], re-adopted each
    /// wake, so the bell count reflects the host's latest.
    fn pane_bell_seq(&self, id: PaneId) -> u64 {
        self.lock_cache().get(id).map_or(0, |pane| pane.bell_seq)
    }

    /// Whether the child has exited, served from the same poll-refreshed mirror as
    /// [`Self::pane_bell_seq`].
    ///
    /// A wake-stale answer here is benign in the one direction it can be wrong: liveness is
    /// ONE-WAY, so the worst case is a just-exited pane still reading live for a poll interval —
    /// never a live pane declared dead.
    fn pane_is_dead(&self, id: PaneId) -> bool {
        self.lock_cache().get(id).is_some_and(|pane| pane.dead)
    }

    /// HOW the child ended, from the same mirror as [`Self::pane_is_dead`].
    ///
    /// Wake-stale in one benign direction too, and a NARROWER one: the status is published after
    /// the liveness bit, so the worst case is a dead pane reading "(exited)" for a poll interval
    /// before it names its code — never a code attributed to a pane that is still running.
    fn pane_child_exit(&self, id: PaneId) -> Option<PaneExit> {
        self.lock_cache()
            .get(id)
            .and_then(|pane| pane.child_exit.clone())
    }

    /// The child's mouse-tracking bit, served from the same poll-refreshed mirror as
    /// [`Self::pane_bell_seq`] (re-adopted each wake). The pane pointer oracle reads it per frame to
    /// gate pointer capture + decide drag / motion forwarding; the authoritative encode still
    /// re-reads the live mode host-side in [`Self::mouse`], so a one-wake-stale level can at most
    /// mis-gate a single event. `pane_mouse_active` is the trait's derived `.is_active()`.
    fn pane_mouse_protocol(&self, id: PaneId) -> MouseProtocol {
        self.lock_cache()
            .get(id)
            .map_or(MouseProtocol::None, |pane| pane.mouse_protocol)
    }

    /// The image SUMMARIES (`{id,width,height,anchor,seq}`, RGBA empty), served from the same
    /// poll-refreshed mirror as [`Self::pane_bell_seq`], re-adopted each wake, so the composited
    /// images reflect the host's latest transmit / clear. The RGBA is fetched separately via
    /// [`Self::pane_image_rgba`] (R1404 Stage 5 on-demand).
    fn pane_images(&self, id: PaneId) -> Vec<Image> {
        self.lock_cache()
            .get(id)
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
            .get(id)
            .map_or(0, |pane| pane.clipboard_write_seq)
    }

    /// The pending read query (selection + seq), served from the mirror (no round-trip).
    fn pane_clipboard_query(&self, id: PaneId) -> Option<PaneClipboardQuery> {
        self.lock_cache()
            .get(id)
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
        // Same take-then-stop ordering, and for the same borrow reason. After the poll: this one
        // only reads, so nothing depends on it stopping first.
        let refreshing = self.activity_thread.borrow_mut().take();
        if let Some(mut activity) = refreshing {
            activity.stop();
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
    /// The name a person gave the pane, `None` when the wire omits the key (nobody named it, or
    /// an older daemon).
    name: Option<String>,
    /// The child's live OSC window title, `None` if it has set none (the wire sends
    /// `null`).
    title: Option<String>,
    /// The pane's most recent attention notification, `None` when the wire omits the key
    /// (the child raised none — the additive `skip`-when-absent shape).
    notification: Option<PaneNotification>,
    /// The pane's tmux monitor-bell count, `0` when the wire omits the key (the child rang none,
    /// or an older daemon).
    bell_seq: u64,
    /// Whether the window is ON this pane, `false` when the wire omits the key (another pane is
    /// active, the window holds none, or an older daemon that has no active pane at all).
    active: bool,
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
    /// What the AGENT in this pane is doing (H3), `None` when the wire omits the `agent` key — a pane
    /// no manifest claims, or a pre-H3 daemon. Those flatten together on purpose: both mean "this
    /// host is telling me nothing about an agent here", which is what a surface must render as
    /// silence rather than as `idle`.
    agent: Option<PaneAgent>,
    dims: (u16, u16),
    /// What a fetch of this pane's CELLS would depend on, as the host reported it
    /// ([`sprag_grid::ProjectionToken`]). `None` when the wire omits the key — an older daemon, or
    /// a token the host could not serialize — which means "fetch anyway".
    projection: Option<ProjectionToken>,
}

/// Query the host's pane list (`/sprag_mux/external/panes`), returning a [`PaneSeed`]
/// per pane in host order.
fn query_panes(conn: &mut HostConn) -> io::Result<Vec<PaneSeed>> {
    let value = query_slot(conn, &mux_action_path(PANES_SLOT))?;
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
            // ADDITIVE: present only on a pane somebody named, so absent means "unnamed" — and
            // reads `None` for every row of a daemon that predates pane names.
            let name = pane["name"].as_str().map(str::to_owned);
            // `null` (child set no title) and a missing key both mean "no title".
            let title = pane["title"].as_str().map(str::to_owned);
            let notification = parse_notification(&pane["notification"]);
            let bell_seq = pane["bell_seq"].as_u64().unwrap_or(0);
            // ADDITIVE: present only on the ONE row the window is on, so absent means "not this
            // one" — and reads `false` for every row of a daemon that predates the active pane.
            let active = pane["active"].as_bool().unwrap_or(false);
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
            // ADDITIVE: the `agent` key rides only a pane with a KNOWN agent state (H3's D8), so an
            // absent key is the honest "no agent here" and never a defaulted state.
            let agent = parse_agent(&pane["agent"]);
            let cols = u16::try_from(pane["cols"].as_u64().unwrap_or(1)).unwrap_or(1);
            let rows = u16::try_from(pane["rows"].as_u64().unwrap_or(1)).unwrap_or(1);
            // ADDITIVE: absent (or unparseable) reads as `None`, which makes this pane fetch
            // unconditionally — the safe direction, since a skipped fetch is what freezes a pane.
            let projection =
                serde_json::from_value::<ProjectionToken>(pane["projection"].clone()).ok();
            Ok(PaneSeed {
                id: PaneId(id),
                label,
                name,
                title,
                notification,
                bell_seq,
                active,
                dead,
                child_exit,
                clipboard_write_seq,
                clipboard_query,
                images,
                mouse_protocol,
                agent,
                dims: (cols, rows),
                projection,
            })
        })
        .collect()
}

/// Parse the additive `agent` object (`{state, name?, rule?, seq}`) back into a [`PaneAgent`].
///
/// The `state` token is REQUIRED and is what makes the value exist: a missing key, `null`, a
/// non-object, or an object without a state string all read as `None` — "this host is saying nothing
/// about an agent in this pane". Nothing is defaulted, for [`parse_child_exit`]'s reason one state
/// further on: a defaulted `"idle"` would tell a user an agent was waiting for them in a pane that
/// holds a shell.
///
/// The token is carried VERBATIM rather than matched against this build's vocabulary, so a daemon
/// newer than its client can name a state this client has never heard of and have it reach a
/// surface. `seq` defaults to `0` because it is a change COUNTER — a client compares it with the last
/// one it saw, and 0 is the honest "no change seen yet" for a daemon that omitted it.
fn parse_agent(value: &Value) -> Option<PaneAgent> {
    Some(PaneAgent {
        state: value["state"].as_str()?.to_owned(),
        name: value["name"].as_str().map(str::to_owned),
        rule: value["rule"].as_str().map(str::to_owned),
        seq: value["seq"].as_u64().unwrap_or(0),
    })
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
        // ⚠ THE TYPE'S OWN SPELLING, not this crate's. Both words used to be emitted by a local
        // match here — documented as the "producer twin" of the host's parser — so one vocabulary
        // had two hand-written definitions in two crates and a rename on either side would have made
        // a button unsendable with both suites green. `MouseButton::wire_str` says the rest.
        "button": event.button.wire_str(),
        "kind": event.kind.wire_str(),
        "col": event.col,
        "row": event.row,
        "ctrl": event.mods.ctrl,
        "alt": event.mods.alt,
        "shift": event.mods.shift,
    })
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
    fresh: bool,
    argv: Option<&[String]>,
    cols: u16,
    rows: u16,
) -> io::Result<(String, bool)> {
    if let Some(name) = requested {
        return Ok((name.to_owned(), false));
    }
    // ⚠⚠⚠⚠ NAMING NONE MEANS *TAKE ME TO MY WORK*, NOT *MAKE ME A NEW ONE* — register item 284.
    //
    // This allocated unconditionally, and the prose beside it called that "the each-launch-is-its-own
    // session model". Measured on the owner's own daemon after an afternoon of clicking: **seven
    // sessions, two attached to live windows and the rest abandoned.** The verb was not merely
    // missing; its absence manufactured garbage on every launch, and the only route back to a
    // running session was a tab click that crashed the window (item 407).
    //
    // ⚠⚠⚠ THE REFERENCE IS herdr, READ AT `9a4ce5e1` (`src/app/mod.rs:398`): a non-empty snapshot
    // restores its workspaces AND the one that was active; an empty one gives an empty app. **It
    // creates nothing at launch.** tmux is the same shape — `attach` is the default act and `new` is
    // the explicit one — and this client had only the explicit one and performed it implicitly.
    //
    // ⚠⚠ WHERE herdr CANNOT ADVISE, BECAUSE IT IS ONE PROCESS: what a SECOND window should do. The
    // rule here prefers a session nobody is viewing, which keeps the several-windows workflow the
    // old prose was protecting while still never manufacturing one. Only when every session is
    // occupied does a launch pile onto one (the host serves multi-attach, tmux-style), and only when
    // there are none at all does it create.
    // ⚠ `fresh` is the caller saying *new*, which is the verb the default stopped being.
    if !fresh && let Some(free) = query_sessions(conn).ok().and_then(|live| adoptable(&live)) {
        return Ok((free, false));
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
/// Pane `id`'s live frame — cells, row shares and token — read out of ONE borrow of the mirror,
/// which is the body of [`HostClient::pane_frame`], split out so the pairing can be asserted
/// without a daemon.
///
/// A pane the mirror does not hold answers the `1x1` placeholder every absent-id read here
/// answers, with no shares and no token: nothing has been painted for it, so there is nothing a
/// later frame could skip and nothing whose lines could be cut.
fn live_frame(cache: &PaneCache, id: PaneId) -> PaneFrame {
    cache.get(id).map_or_else(
        || PaneFrame {
            cells: GridBuffer::new(1, 1),
            shares: sprag_grid::RowShares::default(),
            token: None,
        },
        |pane| PaneFrame {
            cells: pane.frame.cells.clone(),
            shares: pane.frame.facts.shares.clone(),
            token: pane.projection.clone(),
        },
    )
}

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
fn build_cache(conn: &mut HostConn, seeds: Vec<PaneSeed>) -> PaneCache {
    let fetched = fetch_frames(conn, &pane_ids_of(&seeds));
    PaneCache::new(merge_panes(&PaneCache::default(), &seeds, &fetched))
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
    guard.replace(rebuilt);
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
fn stale_panes(existing: &PaneCache, seeds: &[PaneSeed]) -> Vec<PaneId> {
    seeds
        .iter()
        .filter(|seed| {
            let Some(prior) = existing.get(seed.id) else {
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
    existing: &PaneCache,
    seeds: &[PaneSeed],
    fetched: &[(PaneId, CellFrame)],
) -> Vec<WirePane> {
    // Index the arrivals ONCE rather than searching them per seed. `fetched` holds only the
    // panes this wake re-fetched, so on a quiet wake it is empty and on a busy one it is the
    // whole set — which is exactly when a per-seed search would cost the most.
    let arrived: HashMap<PaneId, &CellFrame> =
        fetched.iter().map(|(id, frame)| (*id, frame)).collect();
    let mut rebuilt = Vec::with_capacity(seeds.len());
    for seed in seeds {
        let prior = existing.get(seed.id);
        let fresh = arrived.get(&seed.id).map(|frame| (*frame).clone());
        let refetched = fresh.is_some();
        let frame = fresh.or_else(|| prior.map(|pane| pane.frame.clone()));
        let Some(frame) = frame else {
            continue; // a brand-new pane whose first frame is not here yet — next wake
        };
        rebuilt.push(WirePane {
            id: seed.id,
            label: seed.label.clone(), // host-authoritative — always the query's label
            name: seed.name.clone(),   // host-authoritative + dynamic — a rename lands on a wake
            title: seed.title.clone(), // host-authoritative + dynamic — re-adopt every wake
            // host-authoritative + dynamic like the title: re-adopt the query's, so the seq
            // grows as the child raises more (and clears to None if the host ever drops it).
            notification: seed.notification.clone(),
            bell_seq: seed.bell_seq, // host-authoritative + dynamic, like the notification
            // host-authoritative + dynamic: another client's select, or a close handing off,
            // moves it with nothing local having happened.
            active: seed.active,
            dead: seed.dead, // host-authoritative, and one-way once true
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
            // host-authoritative + dynamic, and re-adopted for the reason `child_exit` is: the value
            // KEEPS CHANGING while the pane lives, and the change that matters most is back to
            // `None` — an agent that exits leaves its shell in the pane, and a kept verdict would
            // leave that shell wearing the agent's last state for the life of the client.
            agent: seed.agent.clone(),
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
// The poll thread's inputs: its own connection, the shared mirrors it refreshes (cache / layout /
// windows / sessions / activity / the session NAME), the repaint + quit sinks, the destroy `policy` + the `lost_session`
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
    activity: ActivityMirror,
    session: SessionMirror,
    message: MessageMirror,
    on_change: Arc<dyn Fn() + Send + Sync>,
    quit: Arc<dyn QuitSink>,
    policy: Arc<dyn Fn() -> DetachOnDestroy + Send + Sync>,
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
                            policy(),
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
                // What this client's session is CALLED, re-read before anything it labels. Its
                // scope is an attachment, so a `rename-session` moves this client silently and by
                // design; the name is the one thing that has to notice, because it is what the
                // sidebar highlights, the palette marks, the next/previous walk indexes, and
                // `sprag-tui` puts in the terminal's title bar. BEST-EFFORT, like the activity
                // sample and unlike the window list: a wake that cannot read it keeps the last-known
                // name, which is a stale label on a working client — the same degradation, not a
                // reason to stop painting. A definitive failure is caught by the window read below,
                // which is not best-effort.
                // WHAT SOMEBODY ASKED THIS CLIENT TO SAY, collected first — before the reads that
                // decide what the panes look like, because a message is the only thing on this path
                // that is an EDGE rather than a level: every read below can be missed and healed on
                // the next wake, and this one cannot (the daemon has already handed it over). Doing
                // it first means a wake that dies half way through still delivered it.
                //
                // BEST-EFFORT like the session name and unlike the window list: a daemon that cannot
                // answer it is a client that shows no message, which is the degradation this feature
                // had before it existed. A definitive failure is caught by the window read below.
                match query_message(&mut conn) {
                    Ok(None) => {}
                    Ok(Some(collected)) => store_message(&message, collected),
                    Err(error) => tracing::debug!(
                        target: "sprag_gui::wire",
                        %error,
                        "message collection failed this wake; nothing is shown for it",
                    ),
                }
                match query_session(&mut conn) {
                    Ok(name) => store_session(&session, name),
                    Err(error) => tracing::debug!(
                        target: "sprag_gui::wire",
                        %error,
                        "session-name re-read failed this wake; keeping the last-known name",
                    ),
                }
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
                            policy(),
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
                            policy(),
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
                            .panes()
                            .iter()
                            .map(|pane| PaneSeed {
                                id: pane.id,
                                label: pane.label.clone(),
                                // Keep the last-known NAME as well. It matters more than the
                                // title beside it: a display surface prefers the name, so
                                // blanking it on a hiccup would visibly re-title the pane.
                                name: pane.name.clone(),
                                // Re-query failed, so the host's current title is unknown —
                                // KEEP the last-known one rather than blanking the display.
                                title: pane.title.clone(),
                                // Likewise keep the last-known notification (and its seq) rather
                                // than dropping the attention badge on a transient query miss.
                                notification: pane.notification.clone(),
                                // Keep the last-known active pane too: a failed query says
                                // nothing about where the user is, and clearing it would move
                                // this client's focus ring on a hiccup.
                                active: pane.active,
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
                                // Keep the last-known verdict: a query that failed says nothing
                                // about the agent, and dropping it would blank a "blocked" title on
                                // a hiccup — the one moment a user most needs it to still be there.
                                agent: pane.agent.clone(),
                                dims: pane.dims,
                                // NO TOKEN, deliberately — the one field this fallback must NOT
                                // carry forward. The re-query failed, so the host's current token
                                // is unknown, and `stale_panes`' own rule for an unknown token is
                                // that "I cannot tell" must never resolve to "assume unchanged".
                                // Carrying the held one resolves it exactly that way: the compare
                                // finds `held == held`, nothing is fetched, and the pane FREEZES —
                                // not until a later wake, but until something else in the session
                                // moves, because the wake this change had was consumed here. A
                                // pane that has just printed and gone quiet has no later wake, so
                                // the freeze is permanent and silent. Measured: the pixel smoke's
                                // driven-line check failed ~1 in 8 with the daemon holding the
                                // line and the client never fetching it. `None` costs one
                                // redundant refresh of the live set on a hiccup, which is the
                                // price this module already states for every other imprecision.
                                projection: None,
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
                // The arbitrated window rides the SAME wake: it moves when any client of this
                // session resizes, and a client holding a stale one would tile over a rectangle
                // the others have already left.
                if let Ok(size) = query_window_size(&mut conn) {
                    store_window_size(&layout, size);
                }
                match query_layout(&mut conn) {
                    Ok(snapshot) => store_layout(&layout, &current, snapshot),
                    Err(error) => {
                        if handle_poll_error(
                            &error,
                            &stop,
                            policy(),
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
                //
                // ⚠⚠ THAT TOLERANCE IS ALSO WHY THIS WRITE CARRIES THE NAME. This read keeps
                // succeeding through the destruction of the session the scoped reads above are
                // about, so it is the one line in the loop that can erase the row this client is
                // standing on — and [`store_sessions`] holds the anchor precisely across it (R367).
                // The name is re-read here rather than taken from the wake's earlier `query_session`
                // because that one is best-effort: a wake that failed to refresh it must still
                // anchor against the last name this client actually believes it is on.
                let viewing = lock_session(&session).clone();
                match query_sessions(&mut conn) {
                    Ok(list) => store_sessions(&sessions, list, &viewing),
                    Err(error) => tracing::debug!(
                        target: "sprag_gui::wire",
                        %error,
                        "sessions re-read failed this wake; keeping the last-known list",
                    ),
                }
                // And the sidebar's SAMPLED half, at the display tolerance rather than at this
                // wake's cadence: a wake is a batch of PTY output, and nothing about a cwd, a branch
                // or a listening port follows from a character being printed. The daemon answers
                // from its held sample unless that sample is older than the tolerance, so this
                // request costs a round trip and not a `/proc` walk (R282).
                match query_activity(&mut conn) {
                    Ok(reading) => store_activity(&activity, reading),
                    Err(error) => tracing::debug!(
                        target: "sprag_gui::wire",
                        %error,
                        "activity re-read failed this wake; keeping the last-known sample",
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
///   UI-thread [`resolve_lost_session`](sprag_host::wake::WakeSource::resolve_lost_session)
///   performs it, through [`Woken::take`](sprag_host::wake::Woken::take). Skipped
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
    // The two mouse vocabularies are the TYPE's now, so nothing outside these tests names the enums
    // in this crate — the args builder goes through `MouseInput`'s own members.
    use sprag_input::{MouseButton, MouseEventKind};
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
    /// `parse_mouse_args` decodes (`{button, kind, col, row, ctrl, alt, shift}`) — a missing KEY here
    /// would make the host refuse the report. Pins the key set + the 0-based coordinates.
    ///
    /// ⚠ **The WORDS are no longer this test's business, and that is the point.** It used to assert
    /// four of the twelve tokens a local match in this crate emitted, which is what let one
    /// vocabulary live in two crates: nothing here could see the host's copy. The words come from
    /// [`MouseButton::wire_str`] now, so the only claim left to make is about the SHAPE — and the
    /// vocabulary is held to the daemon by the pane surface's own acceptance gate, which drives every
    /// published word through a live `invoke`.
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
        // A DIFFERENT event fills the same key set: the shape does not depend on which button or
        // edge it carries, which is the claim a single sample cannot make.
        let wheel = MouseInput {
            button: MouseButton::WheelUp,
            kind: MouseEventKind::Motion,
            col: 0,
            row: 0,
            mods: Modifiers::default(),
        };
        let sent = mouse_wire_args(wheel);
        let mut keys: Vec<&String> = sent
            .as_object()
            .expect("the args are an object")
            .keys()
            .collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            ["alt", "button", "col", "ctrl", "kind", "row", "shift"],
            "every well-formed report carries the same keys",
        );
    }

    /// [`destroy_successor`] over ONE world: the mirror and the fresh read AGREEING.
    ///
    /// That is what a client planning before a kill it is performing itself sees, and it is the
    /// right fixture for every claim about the POLICY — which session each value picks — because a
    /// disagreement would only obscure it. The claim about WHICH list answers which question needs
    /// the two to differ, so it calls the two-list form directly.
    ///
    /// NO ANCHOR, deliberately: every claim about the policy is made by a list that still HOLDS the
    /// row that died, which is the only state a client planning its own kill can be in. Passing one
    /// would let the remembered-place path stand in for the ordinary walk, and then a regression in
    /// the ordinary walk would read green here.
    fn plan(policy: DetachOnDestroy, list: &[SessionInfo], killed: &str) -> Successor {
        destroy_successor(policy, list, None, list, killed)
    }

    /// A structural session list in creation order, the shape [`destroy_successor`] reads — only the
    /// name matters to the neighbour pick, so the live fields are empty.
    /// ⚠⚠⚠⚠ **A LAUNCH ADOPTS WHAT IS THERE AND CREATES ONLY WHEN NOTHING IS** — register item 284.
    ///
    /// # What this replaces, measured rather than argued
    ///
    /// Naming no session used to ALLOCATE one, unconditionally, and the code called that a model.
    /// On the owner's own daemon it produced **seven sessions in one afternoon**, two attached to
    /// live windows and the rest abandoned — and the only route back to a running one was a tab
    /// click that crashed the window. The owner's comparison is the specification: *in herdr the
    /// existing sessions are just there, and a new one appears only when I press new*, which reading
    /// their source confirmed (`src/app/mod.rs:398` at `9a4ce5e1`: restore the snapshot's workspaces
    /// and the active one; create nothing).
    ///
    /// # ⚠⚠⚠ Why the preference is UNOCCUPIED first, which herdr cannot advise on
    ///
    /// They are one process, so *what should a second window do* does not arise there. It does here,
    /// and the old prose was protecting a real thing — two launches, two windows of work. Preferring
    /// a session nobody is viewing keeps that and still never invents one. The arms below are three
    /// separate claims and each has its own way of being wrong.
    #[test]
    fn a_launch_that_names_no_session_adopts_one_and_creates_only_when_there_are_none() {
        assert_eq!(
            adoptable(&[]),
            None,
            "⚠⚠⚠ THE ONE CASE THAT MAY CREATE: no sessions at all. If this ever answers a name, a \
             first launch on a fresh daemon has nothing to attach to and the boot fails instead of \
             opening a window",
        );

        let mut list = session_list(&["work", "notes"]);
        assert_eq!(
            adoptable(&list).as_deref(),
            Some("work"),
            "⚠⚠⚠⚠ THE DEFECT ITSELF: with sessions present a launch must take one. Answering `None` \
             here is the create-always behaviour that manufactured seven abandoned sessions",
        );

        // The first is being watched by another window; the second is free.
        list[0].attached = 1;
        assert_eq!(
            adoptable(&list).as_deref(),
            Some("notes"),
            "⚠⚠⚠ AND A SECOND WINDOW GETS A SESSION OF ITS OWN WHERE ONE IS FREE. This is the \
             several-windows workflow the old create-always prose was protecting, kept — without \
             it, two launches land on one session and the person's second window is a mirror",
        );

        // Every session is occupied: piling on beats inventing.
        list[1].attached = 2;
        assert_eq!(
            adoptable(&list).as_deref(),
            Some("work"),
            "⚠⚠⚠⚠ AND WHEN NOTHING IS FREE IT STILL MUST NOT CREATE. The host serves multi-attach \
             and the person plainly has work open; creating here would be the original defect \
             wearing a condition, and it is how the seventh session appeared",
        );
    }

    fn session_list(names: &[&str]) -> Vec<SessionInfo> {
        names
            .iter()
            .map(|name| SessionInfo {
                name: (*name).to_owned(),
                windows: 1,
                panes: 1,
                default: false,
                attached: 0,
            })
            .collect()
    }

    /// ⚠⚠⚠⚠ **A WINDOW THAT CLOSES ITS OWN SESSION GOES SOMEWHERE; A TERMINAL LEAVES** — register
    /// item 282, which the owner found with a mouse: the app quit with three other sessions alive.
    ///
    /// # Why this asserts the CONSEQUENCE and not the constant
    ///
    /// `Frontend::Window.unset_destroy_policy() == Off` is a restatement of the line it is meant to
    /// hold, and it would stay green if `Off` itself came to detach. What a person met was not a
    /// policy name, it was a window ending — so the claim is *the successor is not a detach*, read
    /// through the same function the live client calls.
    ///
    /// ⚠⚠⚠ **AND THE TERMINAL HALF IS NOT SYMMETRY, IT IS THE POINT.** One shared default is what
    /// caused this, so a fix that made BOTH frontends switch would have replaced one wrong-for-half
    /// constant with another: a `sprag-tui` client that switched sessions instead of detaching would
    /// leave the person's terminal painting a session they did not ask for, where tmux hands their
    /// shell back. The two arms must DIFFER, and either arm alone would pass while the other rotted.
    ///
    /// ⚠⚠ REVERT-PROOF: give `Frontend::Window` the terminal's answer and the first assertion goes
    /// red naming the item; give the terminal the window's and the second does.
    ///
    /// ⚠ The reference for the window's arm is herdr at `9a4ce5e1`
    /// (`src/app/actions.rs:1665 close_selected_workspace`), whose own comment is *"Keep focus on the
    /// previously focused workspace"* — `Off`'s two halves exactly. What herdr ALSO does and this
    /// cannot reach is stay alive on an empty state when nothing survives; a `WireHost` is scoped to
    /// a session at boot, so *no session* is not a state it can hold. That half is registered, not
    /// silently dropped.
    #[test]
    fn the_windows_unset_destroy_policy_switches_where_the_terminals_leaves() {
        let list = session_list(&["a", "b", "c"]);
        let window = plan(Frontend::Window.unset_destroy_policy(), &list, "b");
        assert_ne!(
            window,
            Successor::Detach,
            "⚠⚠⚠⚠ ITEM 282: a WINDOW that destroys its own session must land on another one while \
             any other session is alive. Detaching a window leaves nothing to draw and the app \
             ends — measured by the owner with sessions 1, 3 and 4 still there. Got {window:?}",
        );

        let terminal = plan(Frontend::Terminal.unset_destroy_policy(), &list, "b");
        assert_eq!(
            terminal,
            Successor::Detach,
            "⚠⚠⚠ AND THE TERMINAL MUST STILL LEAVE. Detaching hands the person back the shell they \
             launched from, which is tmux's own default and correct here — a `sprag-tui` that \
             switched instead would repaint their terminal with a session they never asked for. If \
             this arm goes, the fix has replaced one wrong-for-half default with another",
        );
    }

    /// The policy names parse to the tmux values, DEFAULTING to detach for anything unrecognized —
    /// so a client only ever switches away on an EXPLICIT `off`/`no-detached`/`next`/`previous`,
    /// never on a typo. REVERT-PROOF: map the wildcard to `Next` and the `on`/empty/bogus cases start
    /// switching; the hyphenless `"nodetached"` proves the match is EXACT (a near-miss detaches, never
    /// silently picks a switch policy).
    ///
    /// The trim/lowercase this used to assert is now the OPTION table's
    /// ([`OptionKind::canonicalise`](sprag_host::options::OptionKind::canonicalise)), which is why a
    /// value reaching here is already canonical — one folding site instead of one per consumer.
    #[test]
    fn the_detach_policy_defaults_to_detach_and_reads_off_no_detached_next_previous() {
        assert_eq!(parse_detach_on_destroy("on"), DetachOnDestroy::Detach);
        assert_eq!(parse_detach_on_destroy(""), DetachOnDestroy::Detach);
        assert_eq!(parse_detach_on_destroy("sideways"), DetachOnDestroy::Detach);
        assert_eq!(parse_detach_on_destroy("off"), DetachOnDestroy::Off);
        assert_eq!(
            parse_detach_on_destroy("no-detached"),
            DetachOnDestroy::NoDetached
        );
        // A near-miss (hyphen dropped) is NOT no-detached — it falls to the safe detach default.
        assert_eq!(
            parse_detach_on_destroy("nodetached"),
            DetachOnDestroy::Detach
        );
        assert_eq!(parse_detach_on_destroy("next"), DetachOnDestroy::Next);
        assert_eq!(
            parse_detach_on_destroy("previous"),
            DetachOnDestroy::Previous
        );
    }

    /// Every value the option table OFFERS is a policy this client actually performs.
    ///
    /// The drift guard the split vocabulary needs: the names live in `sprag-host`, which cannot see
    /// this enum, and the translation lives here — so nothing but a test holds them together. A name
    /// the table offered and this fell through on would be a setting a user can write and
    /// `show-options` will print, that behaves exactly like `on` and reports nothing. Distinctness is
    /// what detects it: a fall-through collides with `on`'s own policy.
    #[test]
    fn every_offered_policy_is_one_this_client_performs() {
        let offered = sprag_host::options::DETACH_ON_DESTROY_VALUES;
        let performed: Vec<DetachOnDestroy> = offered
            .iter()
            .map(|value| parse_detach_on_destroy(value))
            .collect();
        for (value, policy) in offered.iter().zip(&performed) {
            assert_eq!(
                performed.iter().filter(|other| *other == policy).count(),
                1,
                "{value:?} performs the same policy as another offered value",
            );
        }
    }

    /// The `next`/`previous` target is the LIST NEIGHBOUR (wrapping), or a DETACH for the detach
    /// policy, the last remaining session, or a name already gone from the list. REVERT-PROOF: a
    /// step of 0 or a missing wrap breaks the neighbour/wrap rows; returning a `Named` for the
    /// `Detach`, single-session, or absent-name cases fails a `Detach` assertion — each of which
    /// would turn a safe detach into a wrong switch.
    #[test]
    fn destroy_successor_next_previous_is_the_wrapping_list_neighbour_or_a_detach() {
        let list = session_list(&["a", "b", "c"]);
        let named = |name: &str| Successor::Named(name.to_owned());
        // Detach policy never switches.
        assert_eq!(plan(DetachOnDestroy::Detach, &list, "b"), Successor::Detach,);
        // Next: the row below, wrapping the last back to the first.
        assert_eq!(plan(DetachOnDestroy::Next, &list, "a"), named("b"),);
        assert_eq!(plan(DetachOnDestroy::Next, &list, "c"), named("a"),);
        // Previous: the row above, wrapping the first back to the last.
        assert_eq!(plan(DetachOnDestroy::Previous, &list, "a"), named("c"),);
        assert_eq!(plan(DetachOnDestroy::Previous, &list, "b"), named("a"),);
        // Nothing to switch to → detach: the last session, or a name already off the list.
        assert_eq!(
            plan(DetachOnDestroy::Next, &session_list(&["only"]), "only"),
            Successor::Detach,
        );
        assert_eq!(
            plan(DetachOnDestroy::Next, &list, "gone"),
            Successor::Detach,
        );
    }

    /// `off` asks the DAEMON for the session this client was viewing before, and names the `next`
    /// list neighbour as the fallback — so it still switches whenever another session exists, and
    /// detaches only when `killed` is truly the last one.
    ///
    /// The PICK itself is not tested here and cannot be: it is resolved by session identity inside
    /// the daemon (`AttachmentRegistry::last_viewed`, and end to end in `sprag-host`'s
    /// `a_client_goes_back_to_the_session_it_visited_not_to_the_name_it_wore`). That is the whole
    /// change: this used to walk a `Vec<String>` of remembered names, which is what a re-issued name
    /// captured. What is left here is the FALLBACK, which is a fact about the visible list.
    ///
    /// REVERT-PROOF: drop the `LastViewed` arm to a plain neighbour and the first row's ask is gone;
    /// drop the `1` step and the fallback is no longer the row below; drop the `len < 2` guard and
    /// the last-session row offers a fallback that cannot exist.
    #[test]
    fn destroy_successor_off_asks_for_the_last_viewed_session_with_the_neighbour_as_fallback() {
        let list = session_list(&["a", "b", "c"]);
        assert_eq!(
            plan(DetachOnDestroy::Off, &list, "a"),
            Successor::LastViewed {
                unattached: false,
                fallback: Some("b".to_owned()),
            },
            "off goes back where it was, and falls back to the list neighbour",
        );
        assert_eq!(
            plan(DetachOnDestroy::Off, &session_list(&["only"]), "only"),
            Successor::LastViewed {
                unattached: false,
                fallback: None,
            },
            "with no other session there is no fallback, so `off` detaches after asking",
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
                attached: *attached,
            })
            .collect()
    }

    /// `no-detached` switches ONLY to a session no other client is on (`attached == 0`) — it asks
    /// for the last viewed one NARROWED that way, and falls back to the first free session in list
    /// order, detaching rather than pile onto an occupied one. That last part is the whole point
    /// that distinguishes it from `off`, and it is pinned here by the paired `off` assertion in the
    /// SAME world.
    ///
    /// REVERT-PROOF: drop the `unattached` flag and the ask degrades to `off`'s; drop the
    /// `attached == 0` filter from the fallback and the all-watched row offers an occupied session
    /// instead of detaching.
    #[test]
    fn destroy_successor_no_detached_asks_only_for_a_free_session_else_detaches() {
        // killed "a"; "b" held by another client, "c" free.
        let one_free = attached_list(&[("a", 1), ("b", 1), ("c", 0)]);
        assert_eq!(
            plan(DetachOnDestroy::NoDetached, &one_free, "a"),
            Successor::LastViewed {
                unattached: true,
                fallback: Some("c".to_owned()),
            },
            "no-detached asks for a free session and falls back to the first free one",
        );
        // Every OTHER session is watched by another client → nowhere to fall back to, so this
        // DETACHES rather than share.
        let all_watched = attached_list(&[("a", 1), ("b", 2), ("c", 1)]);
        assert_eq!(
            plan(DetachOnDestroy::NoDetached, &all_watched, "a"),
            Successor::LastViewed {
                unattached: true,
                fallback: None,
            },
            "no-detached leaves rather than pile onto an occupied session",
        );
        // The CONTRAST, in the SAME world: `off` ignores the counts and falls back onto occupied
        // "b" — so a filter that leaked into both policies fails here.
        assert_eq!(
            plan(DetachOnDestroy::Off, &all_watched, "a"),
            Successor::LastViewed {
                unattached: false,
                fallback: Some("b".to_owned()),
            },
            "off ignores viewer counts; only no-detached respects them",
        );
        // The last session → nothing to fall back to.
        assert_eq!(
            plan(
                DetachOnDestroy::NoDetached,
                &attached_list(&[("only", 0)]),
                "only",
            ),
            Successor::LastViewed {
                unattached: true,
                fallback: None,
            },
        );
    }

    /// R327's head, in the one fixture where the two readings DISAGREE: the OCCUPANCY comes from
    /// the list as of now, and the ORDER from the mirror the person could see.
    ///
    /// This is the defect R326 measured, reduced to the decision that made it. Two clients, and the
    /// one whose session is destroyed must not walk into the session the other is sitting in. Its
    /// mirror says `beta` is free — truthfully, as of the last time anything woke this client to
    /// re-read, which an attach to ANOTHER session never does — and the daemon says `beta` now holds
    /// a client. A build that decided on the mirror joins `beta`; the answer is to LEAVE.
    ///
    /// The mirror is deliberately not merely stale but WRONG IN BOTH DIRECTIONS (it also calls
    /// `gamma` occupied where the fresh list has it free), so a reading that took the union, or that
    /// preferred whichever list said "free", is caught rather than passed.
    ///
    /// And the second half is the one an over-eager fix breaks: `next` must still name `beta`,
    /// which only the mirror can say, because the fresh list no longer holds the row `next` counts
    /// FROM. R326 measured exactly that failure — making the decision turn on a re-read alone
    /// turned the occupancy case green and BOTH switch-policy gates red.
    ///
    /// REVERT-PROOF: point `no-detached`'s fallback at `seen` and the first row joins the occupied
    /// session; point `next` at `now` and the last row detaches instead of naming the neighbour.
    #[test]
    fn the_occupancy_comes_from_now_and_the_order_comes_from_what_the_person_saw() {
        // The mirror: `alpha` is about to die, `beta` looks free, `gamma` looks taken.
        let seen = attached_list(&[("alpha", 1), ("beta", 0), ("gamma", 1)]);
        // The daemon, asked at the instant of the decision: `alpha` is gone, and the two survivors
        // are the other way round from what this client last heard.
        let now = attached_list(&[("beta", 1), ("gamma", 0)]);

        assert_eq!(
            destroy_successor(DetachOnDestroy::NoDetached, &seen, None, &now, "alpha"),
            Successor::LastViewed {
                unattached: true,
                fallback: Some("gamma".to_owned()),
            },
            "no-detached must not offer `beta`: somebody is sitting in it NOW, whatever the \
             sidebar was last told",
        );
        // The CONTROL that makes that mean something: on the mirror alone the answer is `beta` —
        // the join R326 measured. So the row above is about which list was read, not about the
        // filter existing.
        assert_eq!(
            plan(DetachOnDestroy::NoDetached, &seen, "alpha"),
            Successor::LastViewed {
                unattached: true,
                fallback: Some("beta".to_owned()),
            },
            "deciding on the mirror is what walked into an occupied session",
        );

        // ...and the ORDER still comes from the mirror, which is the only list that holds the
        // anchor. `next` from `alpha` is the row below it — a question the fresh list cannot answer
        // at all, since `alpha` is not in it.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, None, &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "next counts from the row that died, which only the mirror still has",
        );
        assert_eq!(
            plan(DetachOnDestroy::Next, &now, "alpha"),
            Successor::Detach,
            "the control: asked of the fresh list alone, `next` has no anchor and detaches — \
             which is the switch-policy breakage a re-read-everything fix causes",
        );
    }

    /// **The other half of R327's split, and the one that was throwing people out of the product**:
    /// a switch policy must land on a session that EXISTS, and the mirror is not a list of those.
    ///
    /// [`first_free_other`]'s own doc has said since R327 that *"nothing bounds how stale the
    /// mirror's counts are"*. The counts were fixed by reading `now`; the MEMBERSHIP was left on
    /// `seen`, so a session created after this client's last sessions re-read is a session
    /// `next` / `previous` / `off` cannot see — and with nowhere to go they DETACH. Measured end to
    /// end at R345 (`sprag-tui`'s `every_switch_policy_moves_the_terminal_client`, with the spare
    /// created and the kill performed back to back on one connection so no poll wake fits between
    /// them): the client left the multiplexer while a live session sat beside it, on `off`, `next`
    /// and `previous` alike. It had been reaching CI as an unattributable 45-second timeout since
    /// R343 — three occurrences, two platforms, three tests.
    ///
    /// The converse is the same defect read the other way and is fixed by the same line: a session
    /// the mirror still lists and the daemon has since destroyed is a name [`WireHost::follow`] will
    /// fail to attach to, and a failed follow detaches — so a corpse in the mirror silently costs
    /// the person the live session behind it.
    ///
    /// **The anchor still comes from `seen` and only from `seen`**, which is R326's finding and the
    /// half an over-eager fix breaks; what changed is that the ROW WALKED TO must be one the daemon
    /// says is there. The sibling test above holds the other direction — read the two together.
    ///
    /// REVERT-PROOF: resolve the neighbour over `seen` alone and the first two rows detach (the
    /// measured defect); resolve it over `now` alone and the third row loses its anchor and detaches
    /// too (R326's); drop the skip and the fourth names a session that is gone.
    #[test]
    fn a_switch_policy_lands_on_a_session_that_exists_now_counting_from_the_row_that_died() {
        // The mirror holds ONLY the dying session: this client was never woken to learn about the
        // one somebody else just made. That is the whole fixture — a `seen` shorter than `now`.
        let seen = session_list(&["alpha"]);
        let now = session_list(&["beta"]);

        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &seen, None, &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "`previous` must reach a live session the mirror had not heard of, not detach",
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, None, &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "...and so must `next`: one walk, one candidate list",
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Off, &seen, None, &now, "alpha"),
            Successor::LastViewed {
                unattached: false,
                fallback: Some("beta".to_owned()),
            },
            "`off` means do not leave if there is somewhere to go, and there is",
        );

        // THE ANCHOR IS STILL THE MIRROR'S — the fresh list cannot say what is "after alpha",
        // because alpha is not in it. Two survivors this time, so the ORDER is the claim.
        let seen = session_list(&["alpha", "beta", "gamma"]);
        let now = session_list(&["gamma", "beta"]);
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, None, &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "the row below alpha is beta, whatever order the daemon happens to list them in",
        );
        // ...and the row ABOVE it is the LAST one the person saw. This is the assertion that holds
        // the append to sessions the mirror does NOT have: `next` reads the same either way, and
        // only a wrapping step can tell an order of three from the same three with the daemon's two
        // stuck on the end.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &seen, None, &now, "alpha"),
            Successor::Named("gamma".to_owned()),
            "wrapping backwards lands on the last row the person saw, not on a repeat of it",
        );

        // A NAME THE MIRROR STILL HOLDS AND THE DAEMON NO LONGER SERVES IS SKIPPED, not followed:
        // attaching to it fails, and a failed follow is the detach this whole test is about.
        let seen = session_list(&["alpha", "dead", "beta"]);
        let now = session_list(&["beta"]);
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, None, &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "the neighbour is the next row that still EXISTS",
        );

        // ...and when nothing in either list is still there, a detach is the honest answer. The
        // control for the row above: the skip must run out rather than wrap forever.
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::Next,
                &seen,
                None,
                &session_list(&[]),
                "alpha"
            ),
            Successor::Detach,
            "no live session anywhere is the one case a switch policy has to leave on",
        );

        // A mirror this client never got a list into AND no remembered place cannot say where
        // `alpha` stood, so there is no row to count from and a detach is the honest answer — the
        // same arm as a name already off the list, reached from the other side.
        //
        // ⚠ It is the `None` that carries this now, not the empty list: R367 made a mirror WITHOUT
        // the row a case with an answer, so the claim here is about a client with no place at all.
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::Next,
                &session_list(&[]),
                None,
                &session_list(&["beta"]),
                "alpha",
            ),
            Successor::Detach,
            "no anchor is no neighbour, however much the daemon is serving",
        );
    }

    /// **THE GATE FOR R367: a mirror refreshed PAST the row that died still counts from where the
    /// person stood.**
    ///
    /// # The measurement this opened on
    ///
    /// CI run `31639410510`, `every_switch_policy_moves_the_terminal_client`, `detach-on-destroy =
    /// "previous"`: `client: EXITED ExitStatus(unix_wait_status(0))` with the status trail holding
    /// only `[0] 0:0*` — the client left the multiplexer without ever painting the survivor. That
    /// is R345's signature exactly, and R345's fix is intact: this is the same harm through the
    /// opposite door. There, the mirror was too STALE to see the session to land in; here it is too
    /// FRESH to hold the row to count from, because the sessions re-read that erases it keeps
    /// succeeding while the scoped reads that detect the loss are still failing.
    ///
    /// `a_wake_that_outlives_its_session_keeps_the_place_the_person_stood` drives the shipped poll
    /// loop into that state; this decides what the state MEANS.
    ///
    /// REVERT-PROOF: drop the splice (return `None` when the row is gone) and every row below
    /// detaches — which is the shipped defect. Splice at `0` instead of the anchor and the
    /// three-session rows name the wrong neighbour in both directions. Keep the splice but let the
    /// anchor default to `0` when it is `None` and the sibling test above stops distinguishing a
    /// client with no place from one that stood at the top.
    #[test]
    fn a_switch_policy_counts_from_where_the_person_stood_after_the_mirror_moved_past_it() {
        // THE CI FAILURE, in the smallest world that holds it: two sessions, the mirror already
        // refreshed to the survivor alone, and this client still standing on the one that died.
        let seen = session_list(&["beta"]);
        let now = session_list(&["beta"]);
        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &seen, Some(0), &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "`previous` must not leave the multiplexer past a live session",
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, Some(0), &now, "alpha"),
            Successor::Named("beta".to_owned()),
            "...and neither must `next`: one walk, one anchor",
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Off, &seen, Some(0), &now, "alpha"),
            Successor::LastViewed {
                unattached: false,
                fallback: Some("beta".to_owned()),
            },
            "`off` means do not leave if there is somewhere to go, and there is",
        );

        // THE PLACE IS WHAT IS REMEMBERED, NOT MERELY THAT THERE WAS ONE. Three sessions, the
        // person standing in the MIDDLE, and the mirror refreshed to the two survivors: `next` and
        // `previous` must still disagree, and disagree the way the person's own sidebar read.
        let seen = session_list(&["alpha", "beta", "gamma"]);
        let now = session_list(&["alpha", "gamma"]);
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, Some(1), &now, "beta"),
            Successor::Named("gamma".to_owned()),
            "the row below the gap beta left is gamma",
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &seen, Some(1), &now, "beta"),
            Successor::Named("alpha".to_owned()),
            "and the row above it is alpha — a policy that lost its place would pick either",
        );

        // THE ROW THAT STOOD LAST is the boundary case the splice has to accept: an anchor of
        // `len` is a person who was on the bottom row, and `previous` from there wraps to the row
        // above while `next` wraps to the top.
        let seen = session_list(&["alpha", "beta"]);
        let now = session_list(&["alpha", "beta"]);
        assert_eq!(
            destroy_successor(DetachOnDestroy::Previous, &seen, Some(2), &now, "gamma"),
            Successor::Named("beta".to_owned()),
            "the row above the bottom is the one before it",
        );
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, Some(2), &now, "gamma"),
            Successor::Named("alpha".to_owned()),
            "...and below the bottom wraps to the top",
        );

        // AN ANCHOR NO ORDER CAN HOST is a memory, not a place — a detach, like no anchor at all.
        // Without the bound this is a panic inside a client's reconcile.
        assert_eq!(
            destroy_successor(DetachOnDestroy::Next, &seen, Some(9), &now, "gamma"),
            Successor::Detach,
            "an anchor past the end of the order names no row a person could have been on",
        );

        // ...and a remembered place is still no reason to land on a corpse: the anchor says WHERE
        // to count from, never WHAT is alive. With no survivor anywhere the answer is a detach.
        assert_eq!(
            destroy_successor(
                DetachOnDestroy::Next,
                &session_list(&[]),
                Some(0),
                &session_list(&[]),
                "alpha",
            ),
            Successor::Detach,
            "a place to count from is not a session to go to",
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

    /// ⚠⚠ **A DAEMON THAT ACCEPTS AND SAYS NOTHING MUST NOT STOP THIS CLIENT FROM SHUTTING DOWN.**
    ///
    /// [`ActivityThread::stop`] flags, signals and JOINS, and its doc rests the whole thing on the
    /// thread being *"either inside a bounded request or asleep on the condvar"*. The connection it
    /// was handed carried no deadline at all — so against a silent daemon the refresh parked inside
    /// its read, never looked at the flag again, and the join never returned. Found by R343's debt
    /// sweep, one front over from the `sprag` CLI's own version of the same defect.
    ///
    /// Driven through [`activity_connection`], which is the seam the fix created: a test that built
    /// its own bounded connection would prove the MECHANISM and say nothing about the call site
    /// forgetting to use it, which is exactly what happened.
    ///
    /// The stand-in accepts and holds — never answering, never closing — because an EOF is an answer
    /// of a kind and would end the read on its own. The join is awaited on a channel, since
    /// `JoinHandle::join` has no deadline of its own: **a test that hangs when the code is wrong is a
    /// test whose failure nobody can read.**
    #[test]
    fn a_silent_daemon_cannot_keep_the_activity_thread_from_being_joined() {
        use std::io::{BufRead as _, Write as _};

        let path = sock_path("activity-wedged");
        let listener = UnixListener::bind(&path).expect("bind the wedged host socket");
        let guard = SockGuard(path.clone());
        let held = std::thread::spawn(move || {
            // It answers the HANDSHAKE and nothing after it — a daemon that WEDGES once it is
            // running, which is the only reachable shape: a client refused the handshake never
            // starts a thread to be joined. HELD, not dropped, for the rest of the test, because an
            // EOF is an answer of a kind and would end the read whether or not a deadline exists.
            let Ok((stream, _)) = listener.accept() else {
                return;
            };
            let mut reader = std::io::BufReader::new(
                stream.try_clone().expect("split the stand-in's connection"),
            );
            let mut hello = String::new();
            let _ = reader.read_line(&mut hello);
            let id: Value = serde_json::from_str::<Value>(hello.trim())
                .map(|frame| frame["id"].clone())
                .unwrap_or(Value::Null);
            let mut writer = &stream;
            let _ = writeln!(
                writer,
                "{}",
                json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": { sprag_rpc::PROTOCOL_FIELD: sprag_rpc::WIRE_PROTOCOL },
                })
            );
            let _ = writer.flush();
            std::thread::sleep(Duration::from_secs(60));
            drop(stream);
        });

        let endpoint = HostEndpoint::given("the activity gate", &path);
        let conn = activity_connection(&endpoint, "gui-activity-gate")
            .expect("the wedged host still accepts and shakes hands");

        let stop = Arc::new((Mutex::new(false), Condvar::new()));
        let handle = spawn_activity_refresh(
            conn,
            ActivityMirror::default(),
            Arc::new(|| {}),
            Arc::clone(&stop),
        )
        .expect("the refresh thread spawns");
        let mut thread = ActivityThread {
            stop,
            handle: Some(handle),
        };

        // Past one refresh interval, so the thread is INSIDE the request rather than on the condvar
        // — the state the claim is about, and the one an immediate stop would skip.
        std::thread::sleep(SESSION_ACTIVITY_DISPLAY_MAX_AGE * 2);

        let (done, joined) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            thread.stop();
            let _ = done.send(());
        });
        assert!(
            joined
                .recv_timeout(REQUEST_DEADLINE + Duration::from_secs(20))
                .is_ok(),
            "stop() must return: a request with no deadline parks this thread past every stop flag",
        );

        drop(guard);
        drop(held);
    }

    /// **The client's own mirror ranks two undelivered messages**, and this drives `store_message`
    /// rather than the `Announcement::over` it calls.
    ///
    /// The daemon's slot resolves two messages before either is collected, so this composition only
    /// runs when TWO wakes land between two of the surface's takes — a state no live fixture builds
    /// reliably. A unit test on `over` is not a test that `store_message` calls it (the rule this
    /// project keeps re-learning), so the caller is driven here directly.
    #[test]
    fn the_mirror_keeps_the_alert_when_a_note_lands_behind_it() {
        let say = |text: &str, severity: sprag_host::report::Severity| Announcement {
            text: sprag_host::report::MessageText::parse(text).expect("a plain sentence"),
            severity,
        };
        let mirror: MessageMirror = Arc::new(Mutex::new(None));

        store_message(
            &mirror,
            say("your turn", sprag_host::report::Severity::Alert),
        );
        store_message(&mirror, say("a note", sprag_host::report::Severity::Note));
        assert_eq!(
            lock_message(&mirror)
                .as_ref()
                .map(|held| held.text.as_str().to_owned()),
            Some("your turn".to_owned()),
            "a note landing behind an alert must not displace it in the mirror either",
        );

        // ...and the other way round, which is what stops this passing for a mirror that simply
        // ignored every message after the first.
        let mirror: MessageMirror = Arc::new(Mutex::new(None));
        store_message(&mirror, say("a note", sprag_host::report::Severity::Note));
        store_message(
            &mirror,
            say("your turn", sprag_host::report::Severity::Alert),
        );
        assert_eq!(
            lock_message(&mirror)
                .as_ref()
                .map(|held| held.text.as_str().to_owned()),
            Some("your turn".to_owned()),
        );
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
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Sessions::default())),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            Arc::new(Mutex::new(None)),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Detach),
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
    /// A connected [`HostConn`] whose server serves ONE COMPLETE WAKE whose session list has
    /// already dropped `viewing`, and refuses everything after it — the production window R367 was
    /// measured in.
    ///
    /// # What this reproduces, and why nothing cheaper does
    ///
    /// The kill lands between a wake's SCOPED reads and its registry-wide sessions re-read. The
    /// scoped reads of that wake all succeeded (they ran before the kill), and the sessions read is
    /// registry-wide, so it succeeds AFTER it and answers the survivors alone. The wake therefore
    /// completes normally and leaves the mirror holding a list this client is not in. Only the NEXT
    /// wake meets the refusal that flags the loss — by which time the row to count from is gone.
    ///
    /// Every earlier fixture in this file refuses from the first or second request, so the mirror
    /// never gets written at all and the anchor question cannot arise. That is why they were all
    /// green through the defect.
    ///
    /// The replies are the minimum shapes each read decodes: an empty window and pane list, a
    /// default arrangement, no arbitrated size, no message. `attached` is 1 on the survivor because
    /// a real one has a client in it — nothing here reads it, and a fixture that writes 0 into a
    /// field production never sees is a fixture drifting from the thing it stands for.
    fn a_wake_that_drops_us_then_refuses(
        tag: &str,
        viewing: &str,
        survivors: &[&str],
    ) -> (HostConn, JoinHandle<()>, SockGuard) {
        use std::io::Write;
        let path = sock_path(tag);
        let listener = UnixListener::bind(&path).expect("bind the throwaway host socket");
        let conn = HostConn::connect(&path, Duration::from_secs(2)).expect("connect to it");
        let viewing = viewing.to_owned();
        let list: Vec<SessionInfo> = survivors
            .iter()
            .map(|name| SessionInfo {
                name: (*name).to_owned(),
                windows: 1,
                panes: 1,
                default: false,
                attached: 1,
            })
            .collect();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept the client");
            let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
            let mut writer = stream;
            let mut line = String::new();
            // Flipped by the sessions read itself, which is the LAST thing the wake this fixture
            // serves does that matters. Everything after it is the killed session's world.
            let mut dropped_us = false;
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                let method = request["method"].as_str().unwrap_or_default().to_owned();
                let asked = request["params"]["path"].as_str().unwrap_or_default();
                let result = if dropped_us {
                    None
                } else if method == "scene/waitFor" {
                    Some(json!({ "revision": 1 }))
                } else if method == CLIENT_MESSAGES_METHOD {
                    Some(json!({}))
                } else if asked == mux_action_path(SESSION_SLOT) {
                    Some(Value::String(viewing.clone()))
                } else if asked == mux_action_path(WINDOWS_SLOT)
                    || asked == mux_action_path(PANES_SLOT)
                {
                    Some(json!([]))
                } else if asked == mux_action_path(WINDOW_SIZE_SLOT) {
                    Some(Value::Null)
                } else if asked == mux_action_path(LAYOUT_SLOT) {
                    Some(serde_json::to_value(LayoutSnapshot::default()).expect("a default tree"))
                } else if asked == mux_action_path(SESSIONS_SLOT) {
                    dropped_us = true;
                    Some(serde_json::to_value(&list).expect("a session list"))
                } else {
                    // The activity sample and anything else this wake asks for: best-effort on the
                    // client's side, so a refusal here changes nothing it does.
                    None
                };
                let reply = match result {
                    Some(value) => {
                        json!({ "jsonrpc": "2.0", "id": request["id"], "result": value })
                    }
                    None => json!({
                        "jsonrpc": "2.0",
                        "id": request["id"],
                        "error": { "code": -32602, "message": "no session named \"alpha\"" },
                    }),
                };
                let _ = writeln!(writer, "{reply}");
                let _ = writer.flush();
                line.clear();
            }
        });
        (conn, server, SockGuard(path))
    }

    /// **THE REACHABILITY HALF OF R367: the shipped poll loop erases the row this client stands on,
    /// and must not erase WHERE IT STOOD.**
    ///
    /// Two claims, and they are separate facts about the same wake:
    ///
    /// 1. **The mirror really does lose the row.** It holds the survivor alone afterwards — the
    ///    state `a_switch_policy_counts_from_where_the_person_stood_after_the_mirror_moved_past_it`
    ///    decides, reached here by the real loop rather than written by hand. This claim held BEFORE
    ///    R367 too, and that is the point: it is what made the decision-level defect live.
    /// 2. **The anchor survives it**, so the reconcile that follows still has a place to count from.
    ///
    /// The `lost` flag is asserted with them because a fixture that never got as far as the refusal
    /// would satisfy both claims vacuously — the loop simply would not have run.
    ///
    /// REVERT-PROOF: make [`store_sessions`] assign the anchor unconditionally (the natural way to
    /// write it) and the second claim reads `None` — which is the shipped detach. Serve the
    /// survivor list BEFORE the wake's scoped reads and the first claim still passes while the
    /// window this is about never opens, which is why the fixture flips on the sessions read itself.
    #[test]
    fn a_wake_that_outlives_its_session_keeps_the_place_the_person_stood() {
        let (conn, server, _guard) =
            a_wake_that_drops_us_then_refuses("anchor", "alpha", &["beta"]);
        // The mirror as it stands the instant before that wake: the person can see both rows and is
        // on the FIRST one, which is the place the assertions below are about.
        let sessions: SessionsMirror = Arc::new(Mutex::new(Sessions::default()));
        store_sessions(&sessions, session_list(&["alpha", "beta"]), "alpha");
        let session = Arc::new(Mutex::new("alpha".to_owned()));
        let lost = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(RecordingQuit::default());
        let stop = Arc::new(AtomicBool::new(false)); // NOT our teardown: the session was killed
        let poll = spawn_poll(
            conn,
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::clone(&sessions),
            Arc::new(Mutex::new(None)),
            Arc::clone(&session),
            Arc::new(Mutex::new(None)),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Previous),
            Arc::clone(&lost),
            Arc::clone(&stop),
            0,
        )
        .expect("spawn the poll thread");
        poll.join().expect("the poll thread exited");
        drop(server); // the client left; the server thread ends on the socket's EOF

        assert!(
            lost.load(Ordering::Acquire),
            "the wake after the one that dropped us must flag the loss, or nothing below ran",
        );
        let held = lock_sessions(&sessions);
        assert_eq!(
            held.list
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>(),
            ["beta"],
            "the registry-wide re-read succeeds through our own death and erases our row",
        );
        assert_eq!(
            held.anchor,
            Some(0),
            "...and the place that row held is what no later read can recover, so it is kept",
        );
    }

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
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Sessions::default())),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            Arc::new(Mutex::new(None)),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Detach),
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
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Sessions::default())),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            Arc::new(Mutex::new(None)),
            on_change,
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Detach),
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
                Arc::new(Mutex::new(PaneCache::default())),
                Arc::new(Mutex::new(Mirrored::default())),
                Arc::new(Mutex::new(Vec::new())),
                Arc::new(Mutex::new(Sessions::default())),
                Arc::new(Mutex::new(None)),
                Arc::new(Mutex::new(String::new())),
                Arc::new(Mutex::new(None)),
                on_change,
                Arc::clone(&quit) as Arc<dyn QuitSink>,
                Arc::new(move || policy),
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
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Sessions::default())),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            Arc::new(Mutex::new(None)),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Next),
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
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Sessions::default())),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            Arc::new(Mutex::new(None)),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Next),
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
            Arc::new(Mutex::new(PaneCache::default())),
            Arc::new(Mutex::new(Mirrored::default())),
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Sessions::default())),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(String::new())),
            Arc::new(Mutex::new(None)),
            Arc::new(|| {}),
            Arc::clone(&quit) as Arc<dyn QuitSink>,
            Arc::new(|| DetachOnDestroy::Detach),
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
                shares: sprag_grid::RowShares {
                    upto: vec![3, 1],
                    continues: vec![0],
                },
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
            name: None,
            title: None,
            notification: None,
            bell_seq: 0,
            active: false,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            agent: None,
            frame: frame(3),
            dims: (80, 24),
        }
    }

    /// A host pane-list entry for `id`, reporting `current`.
    fn seeded(id: u64, current: Option<ProjectionToken>) -> PaneSeed {
        PaneSeed {
            id: PaneId(id),
            label: "bash".to_owned(),
            name: None,
            title: None,
            notification: None,
            bell_seq: 0,
            active: false,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            agent: None,
            dims: (80, 24),
            projection: current,
        }
    }

    /// The agent walk answers off ONE cache: every claimed pane, in host order, nothing else.
    ///
    /// The host-order half is not decoration — the title digest a client builds from this reads
    /// left to right, so a walk that reordered panes would rewrite the user's window title every
    /// time the cache was rebuilt. And the ONE-lock half is held the way
    /// [`cells_and_token`]'s pairing is: structurally, by there being a single `lock_cache` in the
    /// method, with the reason in its doc. What is testable without a daemon is the answer, and
    /// that is what this pins.
    #[test]
    fn the_agent_walk_reports_every_claimed_pane_in_host_order() {
        let claimed = |id: u64, state: &str| {
            let mut pane = cached(id, None);
            pane.agent = Some(PaneAgent {
                state: state.to_owned(),
                name: Some("claude".to_owned()),
                rule: None,
                seq: 1,
            });
            pane
        };
        // Host order 7, 3, 9 — deliberately not sorted, so a walk that sorted would show.
        let cache = PaneCache::new(vec![
            claimed(7, "working"),
            cached(3, None), // a shell: claimed by nothing
            claimed(9, "blocked"),
        ]);

        let walked: Vec<(PaneId, String)> = cache
            .panes()
            .iter()
            .filter_map(|pane| pane.agent.clone().map(|agent| (pane.id, agent.state)))
            .collect();
        assert_eq!(
            walked,
            vec![
                (PaneId(7), "working".to_owned()),
                (PaneId(9), "blocked".to_owned()),
            ],
            "the claimed panes, in host order, and the shell contributes nothing",
        );
    }

    /// A rebuild re-addresses AND moves the generation — the two halves of what `replace` is for.
    ///
    /// The index and the panes cannot disagree, since `replace` derives one from the other, so what
    /// is worth pinning is that a rebuild goes through it. A swap that installed contents without it
    /// would answer with the wrong pane, or with a pane that is gone, and would leave every reader
    /// keyed on the generation showing a stale answer for ever.
    #[test]
    fn a_rebuilt_cache_re_addresses_and_moves_its_generation() {
        let mut cache = PaneCache::new(vec![cached(10, None), cached(11, None)]);
        assert!(cache.get(PaneId(11)).is_some());
        let before = cache.agents_generation();

        // Pane 11 closed, pane 12 was born — and 12 comes FIRST in host order, so a stale index
        // would not merely miss it, it would resolve 10 to the wrong slot.
        let seeds = vec![seeded(12, None), seeded(10, None)];
        let rebuilt = merge_panes(
            &cache,
            &seeds,
            &[(PaneId(12), frame(4)), (PaneId(10), frame(4))],
        );
        cache.replace(rebuilt);

        assert!(
            cache.get(PaneId(11)).is_none(),
            "a closed pane stops resolving"
        );
        assert_eq!(cache.get(PaneId(12)).map(|pane| pane.id), Some(PaneId(12)));
        assert_eq!(cache.get(PaneId(10)).map(|pane| pane.id), Some(PaneId(10)));
        assert_eq!(
            cache.panes().iter().map(|pane| pane.id).collect::<Vec<_>>(),
            vec![PaneId(12), PaneId(10)],
            "and host order is what the seeds said, not what the old cache held",
        );
        assert!(
            cache.agents_generation() > before,
            "the pane set moved, so anything keyed on the agent list must recompute",
        );
    }

    /// A wake that only moved FRAMES must NOT move the token — the test the first version of this
    /// mechanism would have failed, and the reason it exists at all.
    ///
    /// `refresh_to_set` replaces the contents on every wake, and a wake is exactly what a pane
    /// echoing a keystroke causes. A token that counted replacements would therefore move on every
    /// paint during typing — unit-green, and inert in the binary at precisely the moment it was
    /// built for. What a reader of `pane_agents` sees did not change here, so neither may the token.
    #[test]
    fn typing_into_a_pane_does_not_move_the_agent_token() {
        let claimed = |projection| {
            let mut seed = seeded(1, projection);
            seed.agent = Some(PaneAgent {
                state: "working".to_owned(),
                name: Some("claude".to_owned()),
                rule: None,
                seq: 4,
            });
            seed
        };
        let mut cache = PaneCache::new(merge_panes(
            &PaneCache::default(),
            &[claimed(Some(token(7)))],
            &[(PaneId(1), frame(3))],
        ));
        let before = cache.agents_generation();

        // The pane printed: a new projection token and a new frame arrive, the verdict does not.
        let rebuilt = merge_panes(&cache, &[claimed(Some(token(8)))], &[(PaneId(1), frame(9))]);
        cache.replace(rebuilt);

        assert_eq!(
            cache.get(PaneId(1)).map(|pane| pane.frame.cells.cols()),
            Some(frame(9).cells.cols()),
            "the frame really did move — this is a live wake, not a no-op",
        );
        assert_eq!(
            cache.agents_generation(),
            before,
            "but nothing a reader of pane_agents can see moved, so the token must not either",
        );
    }

    /// The generation moves for a change NO pane-set comparison could see — which is the whole
    /// reason it counts CONTENTS rather than being a key assembled from named inputs.
    ///
    /// A reader that keyed the window title on "which panes are there" would hold the same key
    /// across this and go on showing `working` for a pane that is now `blocked` — never
    /// recomputed, and invisible, because every other pane's title is right.
    #[test]
    fn the_generation_moves_when_only_a_verdict_moved() {
        let claimed = |state: &str| {
            let mut seed = seeded(1, None);
            seed.agent = Some(PaneAgent {
                state: state.to_owned(),
                name: Some("claude".to_owned()),
                rule: None,
                seq: 1,
            });
            seed
        };
        let mut cache = PaneCache::new(merge_panes(
            &PaneCache::default(),
            &[claimed("working")],
            &[(PaneId(1), frame(3))],
        ));
        let before = cache.agents_generation();
        let ids: Vec<PaneId> = cache.panes().iter().map(|pane| pane.id).collect();

        // Same pane, same id, same frame: only the verdict moved.
        let rebuilt = merge_panes(&cache, &[claimed("blocked")], &[]);
        cache.replace(rebuilt);

        assert_eq!(
            cache.panes().iter().map(|pane| pane.id).collect::<Vec<_>>(),
            ids,
            "the pane SET is identical — a key built from it would not have moved",
        );
        assert_eq!(
            cache
                .get(PaneId(1))
                .and_then(|pane| pane.agent.as_ref())
                .map(|agent| agent.state.as_str()),
            Some("blocked"),
        );
        assert!(
            cache.agents_generation() > before,
            "but the contents moved, and the generation counts THAT",
        );
    }

    /// The paint path's read: a pane's cells, the token they arrived under, and where their rows
    /// end their logical lines all come back TOGETHER.
    ///
    /// The pairing is the claim, not any one part. A client that answered the cells and dropped
    /// the token would paint correctly and rebuild every row of every pane on every frame —
    /// invisible from any screen, which is why it is asserted here rather than left to the live
    /// gate. The shares joined it for a louder reason: they say where to CUT those cells, so a
    /// share taken from a later frame re-wraps a line at a column nothing printed.
    ///
    /// REVERT-PROOF: answer `RowShares::default()` instead of the mirror's own and the shares come
    /// back empty, which a caller reads as "this host cannot say" and draws un-wrapped.
    #[test]
    fn a_panes_cells_come_back_with_the_token_and_the_shares_they_were_fetched_under() {
        let mirror = PaneCache::new(vec![cached(1, Some(token(9))), cached(2, None)]);

        let held = live_frame(&mirror, PaneId(1));
        assert_eq!(
            held.token,
            Some(token(9)),
            "the token must ride with the cells"
        );
        assert_eq!(held.cells.cols(), frame(3).cells.cols());
        assert_eq!(
            held.shares,
            frame(3).facts.shares,
            "and so must the fact that says where those cells' lines end",
        );
        assert!(!held.shares.is_empty(), "or this proves nothing");

        assert_eq!(
            live_frame(&mirror, PaneId(2)).token,
            None,
            "a pane the host could not vouch for must not be given a token",
        );
        let absent = live_frame(&mirror, PaneId(7));
        assert_eq!(
            absent.token, None,
            "and neither must a pane the mirror does not hold",
        );
        assert!(
            absent.shares.is_empty(),
            "which has no lines to describe either",
        );
    }

    /// THE fetch gate: a pane whose projection token has not moved is not re-fetched, and every
    /// other case is. The skip is the whole point of the token; the three fetches are why it is
    /// safe, since each is a case where "unchanged" cannot be established rather than one where it
    /// is known to be false.
    #[test]
    fn only_a_pane_whose_projection_moved_is_refetched() {
        let cache = PaneCache::new(vec![
            cached(10, Some(token(7))), // unchanged since its frame
            cached(11, Some(token(7))), // moved on
            cached(12, None),           // frame predates the token
        ]);
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
        let merged = merge_panes(&PaneCache::new(existing), &seeds, &[]);
        assert_eq!(
            merged[0].projection,
            Some(token(7)),
            "an unfetched pane keeps the token its frame belongs to",
        );
        // ...so the very next wake still sees it as stale and fetches. Without that, the missed
        // fetch above would be permanent.
        assert_eq!(
            stale_panes(&PaneCache::new(merged), &seeds),
            vec![PaneId(10)]
        );
    }

    /// **THE FREEZE, and the fallback that used to cause it.** When a wake's pane re-query fails,
    /// the poll thread rebuilds its seeds from the cache and refreshes through this same gate. That
    /// seed must carry NO token: the host's current one is unknown, and `stale_panes`' rule for an
    /// unknown token is that "I cannot tell" must never resolve to "assume unchanged".
    ///
    /// Carrying the HELD token instead resolves it exactly that way — and the consequence is not a
    /// delay but a permanent freeze, because the wake that would have caught the change was spent
    /// on the failed query. A pane that printed once and went quiet has no later wake to be caught
    /// on. Measured before the fix: the headless pixel smoke's driven-line check failed ~1 in 8
    /// with the DAEMON's own pane holding the line and the client never fetching it, for sixty
    /// seconds.
    ///
    /// Revert-proof: put `pane.projection.clone()` back in that fallback and the first assertion
    /// here reads an empty fetch list.
    #[test]
    fn a_fallback_seed_carries_no_token_so_a_failed_wake_cannot_freeze_a_pane() {
        // The client holds a frame taken at token 7. The host has moved to 8 (the driven line),
        // and this wake's pane re-query failed, so the seed can only be rebuilt from the cache.
        let cache = PaneCache::new(vec![cached(10, Some(token(7)))]);
        let fallback = vec![seeded(10, None)];
        assert_eq!(
            stale_panes(&cache, &fallback),
            vec![PaneId(10)],
            "a seed that cannot state the host's token is fetched, never assumed unchanged",
        );

        // And the merge that follows keeps the pane fetchable rather than labelling the old frame
        // with a token it does not have: an unfetched pane holds on to its own.
        let merged = merge_panes(&cache, &fallback, &[]);
        assert_eq!(merged[0].projection, Some(token(7)));
        assert_eq!(
            stale_panes(&PaneCache::new(merged), &fallback),
            vec![PaneId(10)],
            "so a second failed wake cannot settle into a skip either",
        );
    }

    /// A pane the wake DID re-fetch adopts the query's token, which is what lets the next wake
    /// skip it. The pair with the test above is the whole invariant: the stored token is never
    /// newer than the frame it labels.
    #[test]
    fn a_refetched_pane_adopts_the_token_that_came_with_the_query() {
        let existing = vec![cached(10, Some(token(7)))];
        let seeds = vec![seeded(10, Some(token(9)))];
        let merged = merge_panes(&PaneCache::new(existing), &seeds, &[(PaneId(10), frame(9))]);
        assert_eq!(merged[0].projection, Some(token(9)));
        assert_eq!(
            merged[0].frame.cells.cols(),
            9,
            "and the fetched frame with it"
        );
        assert!(
            stale_panes(&PaneCache::new(merged), &seeds).is_empty(),
            "so the next wake skips it",
        );
    }

    /// A cell frame `n` cols wide, so a test can tell frames apart by `cells.cols()`.
    ///
    /// It carries NON-EMPTY row shares, and that is load-bearing rather than decoration: the
    /// pairing gate asserts the shares ride with the cells, and a helper answering the empty
    /// default would make that assertion true of a client that had dropped them.
    fn frame(cols: u16) -> CellFrame {
        CellFrame {
            cells: GridBuffer::new(cols, 1),
            facts: PaneScrollFacts {
                shares: sprag_grid::RowShares {
                    upto: vec![cols],
                    continues: Vec::new(),
                },
                ..PaneScrollFacts::absent()
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
                name: None,
                title: None,
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                frame: frame(3),
                dims: (80, 24),
            },
            WirePane {
                id: PaneId(11),
                label: "cat".to_owned(),
                name: None,
                title: None,
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
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
                name: None,
                title: None,
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                dims: (100, 30),
            },
            PaneSeed {
                id: PaneId(12),
                label: "vim".to_owned(),
                name: None,
                title: None,
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                dims: (80, 24),
            },
            PaneSeed {
                id: PaneId(13),
                label: "top".to_owned(),
                name: None,
                title: None,
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                dims: (80, 24),
            },
        ];
        let fetched = vec![(PaneId(10), frame(5)), (PaneId(12), frame(7))]; // 13 not fetched

        let merged = merge_panes(&PaneCache::new(existing), &seeds, &fetched);

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
            name: None,
            title: None,
            notification: None,
            bell_seq: 0,
            active: false,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            agent: None,
            frame: frame(3),
            projection: None,
            dims: (80, 24),
        }];
        let seeds = vec![PaneSeed {
            id: PaneId(10),
            label: "bash".to_owned(),
            name: None,
            title: None,
            notification: None,
            bell_seq: 0,
            active: false,
            dead: false,
            child_exit: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            agent: None,
            projection: None,
            dims: (80, 24),
        }];
        let merged = merge_panes(&PaneCache::new(existing), &seeds, &[]); // fetch missed this wake
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
            name: None,
            title: None,
            notification: None,
            bell_seq: 0,
            active: false,
            dead: false,      // last wake it was still running
            child_exit: None, // ...so of course nothing had reaped it
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            agent: None,
            frame: frame(3),
            projection: None,
            dims: (80, 24),
        }];
        let seeds = vec![PaneSeed {
            id: PaneId(10),
            label: "cargo".to_owned(),
            name: None,
            title: None,
            notification: None,
            bell_seq: 0,
            active: false,
            dead: true, // ...and the host now says the child has exited
            child_exit: Some(PaneExit {
                code: 101, // ...having reaped it, with cargo's own failure code
                signal: None,
            }),
            clipboard_write_seq: 0,
            clipboard_query: None,
            images: Vec::new(),
            mouse_protocol: MouseProtocol::None,
            agent: None,
            projection: None,
            dims: (80, 24),
        }];

        let merged = merge_panes(&PaneCache::new(existing), &seeds, &[]);
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
                name: None,
                title: Some("stale: vim README".to_owned()),
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                frame: frame(3),
                dims: (80, 24),
            },
            WirePane {
                id: PaneId(11),
                label: "bash".to_owned(),
                name: None,
                title: Some("about to be cleared".to_owned()),
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                frame: frame(3),
                dims: (80, 24),
            },
        ];
        let seeds = vec![
            PaneSeed {
                id: PaneId(10),
                label: "bash".to_owned(),
                name: None,
                title: Some("coin@host:~".to_owned()), // child retitled at the new prompt
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                dims: (80, 24),
            },
            PaneSeed {
                id: PaneId(11),
                label: "bash".to_owned(),
                name: None,
                title: None, // child cleared its title
                notification: None,
                bell_seq: 0,
                active: false,
                dead: false,
                child_exit: None,
                clipboard_write_seq: 0,
                clipboard_query: None,
                images: Vec::new(),
                mouse_protocol: MouseProtocol::None,
                agent: None,
                projection: None,
                dims: (80, 24),
            },
        ];
        let fetched = vec![(PaneId(10), frame(5)), (PaneId(11), frame(5))];

        let merged = merge_panes(&PaneCache::new(existing), &seeds, &fetched);

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

    /// [`parse_agent`] maps the additive `agent` object and NEVER manufactures a state.
    ///
    /// The rejections are the assertions that matter. `idle` means "an agent is waiting for you", so a
    /// defaulted state would put that on every shell in the workspace — and D3 makes the distinction
    /// between "not an agent" and "an agent at rest" mandatory precisely because they are opposite
    /// instructions to the person reading a pane list.
    ///
    /// REVERT-PROOF: default the state (`unwrap_or("idle")`) and the three rejection assertions fail
    /// together.
    #[test]
    fn parse_agent_maps_the_wire_object_and_never_invents_a_state() {
        let a = parse_agent(&json!({
            "state": "blocked", "name": "claude", "rule": "dialog-choice-list", "seq": 4
        }))
        .expect("a well-formed verdict parses");
        assert_eq!(a.state, "blocked");
        assert_eq!(a.name.as_deref(), Some("claude"));
        assert_eq!(a.rule.as_deref(), Some("dialog-choice-list"));
        assert_eq!(a.seq, 4);

        // A pane no manifest claims, an older daemon, and a garbled object all read as silence.
        assert!(parse_agent(&Value::Null).is_none(), "null ⇒ None");
        assert!(parse_agent(&json!("nope")).is_none(), "a string ⇒ None");
        assert!(
            parse_agent(&json!({ "seq": 2 })).is_none(),
            "an object with no state is not a state",
        );

        // `name` and `rule` are optional ON THE WIRE (R251: a modal can cover the fingerprint), and a
        // state token this build has never heard of is carried rather than dropped — the daemon may be
        // newer than its client.
        let bare = parse_agent(&json!({ "state": "compacting" })).expect("a state alone is enough");
        assert_eq!(
            (bare.state.as_str(), bare.name, bare.rule, bare.seq),
            ("compacting", None, None, 0),
        );
    }

    /// A pane whose agent EXITED stops claiming one: the merge re-adopts the query's verdict,
    /// including back to `None`.
    ///
    /// This is the direction that would rot silently. A kept verdict still looks right on every pane
    /// that has one, and the pane it is wrong about is the shell an agent left behind — which would go
    /// on wearing "working" in every title surface for the life of the client.
    ///
    /// REVERT-PROOF: carry the prior value when the seed has none (`seed.agent.clone().or(prior…)`)
    /// and the second assertion fails while the first still passes.
    #[test]
    fn a_survivor_re_adopts_the_hosts_verdict_including_its_absence() {
        let verdict = PaneAgent {
            state: "working".to_owned(),
            name: Some("claude".to_owned()),
            rule: Some("spinner-glyph".to_owned()),
            seq: 2,
        };
        // The pane is cached with no verdict and the host now reports one: the survivor adopts it.
        let mut seed = seeded(1, None);
        seed.agent = Some(verdict.clone());
        let merged = merge_panes(
            &PaneCache::new(vec![cached(1, None)]),
            &[seed],
            &[(PaneId(1), frame(4))],
        );
        assert_eq!(
            merged[0].agent.as_ref().map(|a| a.state.as_str()),
            Some("working"),
            "a verdict that appeared reaches the cache on the wake that carried it",
        );

        // ...and now the agent has exited, so the host says nothing about this pane again.
        let mut held = cached(1, None);
        held.agent = Some(verdict);
        let merged = merge_panes(
            &PaneCache::new(vec![held]),
            &[seeded(1, None)],
            &[(PaneId(1), frame(5))],
        );
        assert!(
            merged[0].agent.is_none(),
            "the absence is adopted too — the shell left behind is not still an agent",
        );
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

    /// The CREATE path of [`resolve_session`]: carrying THIS client's first pane
    /// (`cmd`/`cols`/`rows` — tmux's `new-session -x -y command`), so the birth pane matches and
    /// [`boot_panes`] tops up from it. Proves the GUI actually EMITS the birth spec, which the
    /// host-side test can only prove it accepts.
    ///
    /// # ⚠⚠⚠⚠ It asserted ONE request and now asserts TWO, and the change is the point
    ///
    /// Naming no session used to create unconditionally. Register item 284: that manufactured seven
    /// abandoned sessions on the owner's daemon in an afternoon, because *name nothing* means *take
    /// me to my work* and not *make me another one*. So the path now ASKS first and creates only
    /// when the answer is empty — this fixture's recording host lists none, which is exactly the
    /// case that still creates.
    ///
    /// ⚠⚠⚠ **THIS COUNT IS WHAT HOLDS THE WIRING.** `adoptable`'s own gate is a pure decision over a
    /// list and would stay green if nobody ever called it; the request count here is the only thing
    /// that says `resolve_session` actually looks before it creates. Measured: deleting the lookup
    /// reds this assertion and no other in the crate.
    #[test]
    fn resolve_session_creates_with_the_clients_first_pane() {
        let (mut conn, server, _guard, seen) = a_recording_host_conn("create", "7");
        let argv = ["vim".to_owned(), "README".to_owned()];
        let (name, created) = resolve_session(&mut conn, None, false, Some(&argv), 100, 40)
            .expect("resolve_session creates");
        drop(conn); // let the server thread see EOF and exit
        server.join().expect("server thread exited");

        assert_eq!(
            (name.as_str(), created),
            ("7", true),
            "it adopts the allocated name",
        );
        let seen = seen.lock().expect("record lock");
        assert_eq!(
            seen.len(),
            2,
            "⚠⚠⚠⚠ TWO REQUESTS: the LOOK, then the create. One means the client created without \
             asking what was already there, which is item 284 — and no other assertion in this \
             crate can tell the difference. Sent: {seen:?}",
        );
        assert_eq!(
            seen[0]["params"]["path"],
            json!(mux_action_path(SESSIONS_SLOT)),
            "⚠⚠⚠ and the look comes FIRST — a client that created and then listed would have made \
             the garbage before reading the answer that says not to",
        );
        let req = &seen[1];
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
        let (name, created) = resolve_session(&mut conn, Some("mysession"), false, None, 80, 24)
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

    /// A host socket that accepts ONE connection, records every request and answers each with a
    /// null result — for the rollback, whose subject is the connection it opens for ITSELF rather
    /// than one it was handed.
    ///
    /// The accept is BOUNDED: a rollback that never connects (the state the revert-proof puts this
    /// in) must make the test fail on its `seen` assertion, not hang on the join. A test that hangs
    /// when the code is wrong is a test whose failure nobody can read.
    fn a_kill_recording_host(
        tag: &str,
    ) -> (PathBuf, JoinHandle<()>, SockGuard, Arc<Mutex<Vec<Value>>>) {
        use std::io::Write;
        let path = sock_path(tag);
        let listener = UnixListener::bind(&path).expect("bind the throwaway host socket");
        listener
            .set_nonblocking(true)
            .expect("poll the listener rather than park on it");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_srv = Arc::clone(&seen);
        let server = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break Some(stream),
                    Err(_) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break None,
                }
            };
            let Some(stream) = stream else { return };
            stream
                .set_nonblocking(false)
                .expect("serve the accepted connection blocking");
            let mut reader = BufReader::new(stream.try_clone().expect("clone the stream"));
            let mut writer = stream;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 0) {
                let request: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                let id = request["id"].clone();
                seen_srv.lock().expect("record lock").push(request);
                let response = json!({ "jsonrpc": "2.0", "id": id, "result": Value::Null });
                let _ = writeln!(writer, "{response}");
                let _ = writer.flush();
                line.clear();
            }
        });
        (path.clone(), server, SockGuard(path), seen)
    }

    /// The rollback sends exactly ONE `kill_session` naming the session this boot created, over a
    /// connection it opened itself, and reports the session as REMOVED.
    ///
    /// REVERT-PROOF: make [`BornSession::kill`] a no-op `Ok(())` and `seen` is empty — the daemon
    /// was never asked, and the "was removed" this reports would be a lie.
    #[test]
    fn the_rollback_asks_the_daemon_to_kill_the_session_it_created() {
        let (path, server, _guard, seen) = a_kill_recording_host("rollback");
        let endpoint = HostEndpoint::given("the rollback test", path);
        let born = BornSession {
            endpoint: &endpoint,
            session: "7".to_owned(),
        };

        let reported = born.roll_back(io::Error::other("the poll connection failed"));
        server.join().expect("server thread exited");

        let seen = seen.lock().expect("record lock");
        assert_eq!(seen.len(), 1, "exactly one request — the kill — was sent");
        assert_eq!(seen[0]["method"], "scene/invoke");
        assert_eq!(
            seen[0]["params"]["path"],
            json!(mux_action_path(KILL_SESSION_ACTION)),
        );
        assert_eq!(seen[0]["params"]["args"], json!({ "name": "7" }));
        assert_eq!(reported.created(), Some("7"));
        assert_eq!(
            reported.orphan(),
            None,
            "the daemon answered, so nothing is left behind",
        );
        assert!(
            reported
                .to_string()
                .contains("the session `7` this boot created was removed"),
            "the report says what it did about the session: {reported}",
        );
    }

    /// When the rollback itself cannot run, the failure NAMES the orphan and the exact command
    /// that removes it — the case R278 hit nine times, where the operator was told nothing.
    ///
    /// The whole SENTENCE is pinned rather than three fragments of it, because this string is the
    /// deliverable: an operator reads it once, at the worst moment, and its order (which daemon →
    /// what went wrong → what is still there → what to type) is what makes it usable. Only the
    /// OS's own errno text is left unpinned, being the one part this code does not author.
    ///
    /// REVERT-PROOF: collapse [`BootResidue::Orphan`] into `Removed` (or drop the name from the
    /// message) and both the `orphan()` fact and the remedy line go.
    #[test]
    fn an_unremovable_session_is_named_with_its_remedy() {
        // A path nobody serves, spelled literally so the rendering below is exact: the rollback's
        // own connect fails, which is precisely the state a boot that failed BECAUSE the daemon
        // went away leaves behind.
        let endpoint = HostEndpoint::given("the rollback test", "/nonexistent/dir/x.sock");
        let born = BornSession {
            endpoint: &endpoint,
            session: "3".to_owned(),
        };

        let reported = born.roll_back(io::Error::other("the layout read failed"));

        assert_eq!(reported.created(), Some("3"));
        assert_eq!(reported.orphan(), Some("3"), "it is still on the daemon");
        let rendered = reported.to_string();
        let opening = "/nonexistent/dir/x.sock (given by the rollback test): the layout read \
                       failed (the session `3` this boot created is STILL on that daemon — \
                       removing it failed: ";
        assert!(
            rendered.starts_with(opening),
            "the report reads: which daemon, what failed, what is still there — in that order.\n\
             expected to start with: {opening}\n                    rendered: {rendered}",
        );
        assert!(
            rendered.ends_with("; remove it with `sprag kill-session -t 3`)"),
            "and it ends with the command that removes the orphan: {rendered}",
        );
    }

    /// A daemon that answers a hello WITHOUT naming its wire shape is one from before the shape
    /// agreement, and the client says so instead of going on to read it.
    ///
    /// This is the direction the daemon's own check cannot cover — an old daemon serves a request
    /// carrying an unknown `protocol` param happily — and it is the ORDINARY skew here, because a
    /// sprag daemon outlives the clients that rebuild around it.
    ///
    /// REVERT-PROOF: make [`HostConn::handshake`]'s `None` arm `Ok(())` and this passes a boot
    /// through to a daemon whose shape nobody agreed on.
    #[test]
    fn a_daemon_that_names_no_protocol_is_refused_as_older() {
        // `true` is what the pre-handshake daemon answered a hello with.
        let (mut conn, server, _guard, _seen) = a_recording_host_conn("no-protocol", "unused");
        let refused = conn
            .handshake("gui-test")
            .expect_err("a daemon that names no shape cannot be agreed with");
        drop(conn);
        server.join().expect("server thread exited");

        let message = refused.to_string();
        assert!(
            message.contains("a daemon older than this check"),
            "the report says WHICH end is behind: {message}",
        );
        assert!(
            message.contains(&format!(
                "client speaks wire protocol {}",
                sprag_rpc::WIRE_PROTOCOL
            )),
            "and what this build speaks: {message}",
        );
        assert!(
            message.contains("sprag kill-server"),
            "and the action that resolves it: {message}",
        );
    }

    /// A boot that never created a session reports no residue at all — the ATTACH path must not
    /// claim to have made (or removed) anything.
    #[test]
    fn a_boot_that_created_nothing_reports_no_residue() {
        let endpoint = HostEndpoint::given("the rollback test", sock_path("unreached"));
        let reported = BootError::left_nothing(
            endpoint,
            io::Error::new(io::ErrorKind::ConnectionRefused, "Connection refused"),
        );

        assert_eq!(reported.created(), None);
        assert_eq!(reported.orphan(), None);
        let as_io: io::Error = reported.into();
        assert_eq!(
            as_io.kind(),
            io::ErrorKind::ConnectionRefused,
            "the wire's own kind survives the wrapping, for a caller that matches on it",
        );
        assert!(
            as_io.to_string().contains("(given by the rollback test)"),
            "even a boot that reached nothing names the endpoint it tried: {as_io}",
        );
    }
}
