//! The host — the single [`Workspace`] owner, used two ways.
//!
//! [`Host`] owns the one live [`Workspace`] (and thus the PTYs) and serves the
//! typed [`HostClient`] protocol over it: cell DATA, per-frame scroll facts,
//! resize control, INPUT (`send_key` / `send_text`), input handles, and pane
//! text. This is the single home for "who owns the panes", shared by both
//! frontends (the north-star's two-frontend platform, [DESIGN.md §5]):
//!
//! * the **GUI** (`sprag-gui`) reaches every pane through a `Box<dyn HostClient>`
//!   — a wire client (`WireHost`) attached to a `sprag-term` host PROCESS — so the
//!   display client is a structurally-separate client of this host (topology B);
//! * the **headless server** (`sprag-term`) boots its pane through a `Host`
//!   in-process and wraps it in [`HostState`](crate::HostState) to serve the
//!   scene-as-data RPC surface an AI peer (and the GUI) drives.
//!
//! ## The protocol (shaped like the wire)
//!
//! [`HostClient`] is that protocol as a Rust trait, with two impls: the
//! in-process [`Host`] (below) and the GUI's `WireHost` (the same surface over an
//! RPC socket). Its methods are:
//!
//! * cell DATA ([`pane_cells`](HostClient::pane_cells)) + the non-cell per-frame
//!   facts that ride alongside it ([`pane_scroll_facts`](HostClient::pane_scroll_facts));
//! * resize control ([`resize`](HostClient::resize)) + grid geometry
//!   ([`pane_grid_size`](HostClient::pane_grid_size));
//! * INPUT — the display client's keyboard / IME are client SENDs
//!   ([`send_key`](HostClient::send_key) / [`send_text`](HostClient::send_text)),
//!   encoded by the shared [`crate::send_key`] / [`crate::send_text`] SSOT (the
//!   same encoder the RPC `scene/invoke` path uses); the wire client's
//!   implementation sends them as an RPC `scene/invoke` to the host's pane input
//!   surface, the in-process `Host` writes the PTY directly;
//! * pane text ([`pane_full_text`](HostClient::pane_full_text) /
//!   [`pane_command_label`](HostClient::pane_command_label)) for the a11y tree.
//!
//! The ONE method NOT on the trait is [`pane_handle`](Host::pane_handle) — it
//! hands out a live [`PanePtyHandle`] that cannot cross a wire; it stays an
//! inherent [`Host`] method used only by in-process input surfaces, and retires as
//! input clients attach to the host.
//!
//! ## Ownership
//!
//! The `Workspace` lives behind `Arc<Mutex<_>>` — the shape [`HostState`](crate::HostState) and the
//! plugin/control externals already share (a background plugin run reads a pane
//! from a worker thread, so the pool is genuinely shared). Presentation (cell
//! metric, font size) is NOT here — that is the display client's own state.

use std::sync::{Arc, Mutex};

use pinion_core::GridBuffer;
use sprag_grid::{ProjectionToken, RowShares};
use sprag_input::{Modifiers, MouseInput};
use sprag_terminal::{
    ActivityReading, Attention, CommandBuilder, DividerStep, Ended, Hand, HistoryLimitSource,
    LayoutSnapshot, LayoutWire, OrderStep, Pane, PaneArgsSource, PaneBirthHooks, PaneDir,
    PaneEnvSource, PaneHomes, PaneId, PaneIdentitySource, PanePtyError, PanePtyHandle, PaneRebirth,
    PaneStep, PlaceHow, Projection, Rect, SessionId, SessionInfo, SessionRegistry, Snapshot,
    SnapshotError, SplitDir, SplitSide, Tree, WindowId, WindowInfo, WindowPlace, Workspace,
    ZoomOutcome, tile, with_ratio,
};
use sprag_vt::{ClipboardTarget, ClipboardTargets, Image, MouseProtocol, Screen, osc52_reply};

use crate::attach::{AttachmentRegistry, ClientSize};
use crate::external::lock;
use crate::scope::SessionScope;
use crate::wire::{ResizeAsk, ResizeHow, SelectAsk, SelectHow, SwapAsk, SwapHow};

/// Per-pane facts the client reads each frame that are NOT carried in the cell
/// buffer but ride ALONGSIDE it in one pane-frame: the scrollback depth (the
/// scrollbar extent + the top-anchored offset math) and the visible row count
/// (one scrollback page). Host-owned; over the wire these travel WITH the
/// [`pane_cells`](HostClient::pane_cells) buffer as one message (not a separate
/// round-trip). Named "facts", not "dims", so it is never confused with the grid
/// geometry ([`pane_grid_size`](HostClient::pane_grid_size)) — `scrollback_len` is
/// a history depth, not a dimension.
///
/// This is the ONE definition of the frame's non-cell field set: the in-process
/// client reads it via [`Host::pane_scroll_facts`](HostClient::pane_scroll_facts),
/// and the wire's `cells.<offset>` query family ([`CELLS_FIELD`](crate::wire::CELLS_FIELD))
/// flattens the SAME type into its JSON frame (serde-derived), so the field
/// names + wire keys cannot drift between the two clients. `Serialize` /
/// `Deserialize` for the wire; `Eq` so a test can compare two reads.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneScrollFacts {
    pub scrollback_len: usize,
    pub visible_rows: u16,
    /// Where each of the frame's rows ends its logical LINE, and which of them run on — what a
    /// client narrower than this pane needs to re-wrap it ([`sprag_grid::rewrap`]) instead of
    /// showing a sixty-column slice of a hundred-column line.
    ///
    /// A fact and not a rendering: the producer owns where a line ends (`sprag_vt`'s `line_cells`
    /// is the one place that is decided) and a rectangle of cells cannot carry it. R344 recorded
    /// what three readers guessing it cost, so it is sent rather than inferred.
    ///
    /// ADDITIVE, and absent-not-wrong to a reader that has never heard of it: a client that
    /// ignores this key draws the pane exactly as every client did before it existed. Skipped when
    /// empty so a host answering a frame nobody derived shares for costs a reader nothing.
    #[serde(default, skip_serializing_if = "RowShares::is_empty")]
    pub shares: RowShares,
}

impl PaneScrollFacts {
    /// Read the non-cell facts from a live `screen` — the SINGLE population site,
    /// shared by [`Host::pane_scroll_facts`](HostClient::pane_scroll_facts) and the
    /// wire `cells` action, so the two never disagree on how a fact is derived
    /// (adding a fact edits only here + the struct).
    ///
    /// `offset_lines` is the scrollback offset the CELLS beside these facts were projected at, and
    /// it is a parameter rather than an assumption because [`shares`](Self::shares) describes
    /// those rows: a frame windowed into history whose shares described the live rows would tell a
    /// client to cut lines that are not on its screen.
    ///
    /// Public so the WIRE-COST instrument (`sprag-latency`) can build the frame the daemon
    /// actually sends. It used to spell the fields by hand, which measured a frame no client ever
    /// receives — and would have priced this fact at zero on the day it was added.
    #[must_use]
    pub fn of(screen: &Screen, offset_lines: usize) -> Self {
        Self {
            scrollback_len: screen.scrollback_len(),
            visible_rows: screen.rows(),
            shares: sprag_grid::shares(screen, offset_lines),
        }
    }

    /// [`Self::of`] for the LIVE view — the offset every in-process reader of these facts uses.
    pub(crate) fn from_screen(screen: &Screen) -> Self {
        Self::of(screen, 0)
    }

    /// The facts for a pane that is NOT THERE — a zero-depth, one-row frame nobody can scroll.
    ///
    /// Spelled once because five surfaces answer it (the host, the wire client, and three test
    /// doubles), and the round that added [`shares`](Self::shares) is the round that found all
    /// five by breaking them. A default nobody can spell wrongly is the point.
    #[must_use]
    pub fn absent() -> Self {
        Self {
            scrollback_len: 0,
            visible_rows: 1,
            shares: RowShares::default(),
        }
    }
}

/// What ONE read of a pane produced: its cells, where each of their rows ends its logical line,
/// and the token they arrived under.
///
/// The distinction from [`CellFrame`](crate::CellFrame) is which side of the wire is speaking.
/// `CellFrame` is a pane frame as the HOST ANSWERS it — cells plus the facts that ride with them.
/// This is the same frame as a CLIENT READ it, and the difference is the [`ProjectionToken`]: a
/// cache key the client derives from its own last paint, which never crosses the wire at all.
///
/// The three travel together because each of the other two describes the cells: separating them
/// is separating a frame from what it means. See [`HostClient::pane_frame`].
#[derive(Debug, Clone)]
pub struct PaneFrame {
    /// The paint-authoritative cells.
    pub cells: GridBuffer,
    /// Where each of those rows ends its logical line, and which run on. EMPTY means the host
    /// could not say, which a caller reads as "draw the pane as it stands".
    pub shares: RowShares,
    /// The projection those cells arrived under, or `None` for "cannot say" — rebuild everything.
    pub token: Option<ProjectionToken>,
}

/// One find-in-scrollback match, as a client reads it off the wire — the serde projection of
/// [`sprag_vt::FindMatch`], whose coordinate this carries unchanged.
///
/// A pane holds ROWS and a person reads LINES, and a match knows both: `line` names the LOGICAL
/// line by the retained row it begins on (the scroll `offset_y` axis, so a client jumps to it with
/// the offset it already speaks, and the join key to [`PaneFindLine`]), while `row`/`col`/`cols`
/// say where the match's first cell actually sits and `wrapped` carries the rest of it, one width
/// per row it runs on to. `col`/`cols`/`wrapped` are CELL columns, ready to overlay.
///
/// The VT layer stays serde-free by design ("the VT layer owns no wire shape"), so the wire shape
/// lives here, beside [`PaneScrollFacts`] and for the same reason: ONE definition both the host's
/// `find.<needle>` query and every client deserialize, so the JSON keys cannot drift.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneMatch {
    /// The logical line, named by the retained row it begins on.
    pub line: usize,
    /// The retained row the match's first cell sits on (`line` unless it starts past a wrap).
    pub row: usize,
    /// Starting cell column within `row`.
    pub col: u16,
    /// Width in cell columns on `row` (a wide cluster counts two).
    pub cols: u16,
    /// Width in cell columns on each row after `row`, each starting at column 0. Absent for a
    /// match that lies within one row, which is nearly all of them — a find bar re-queries on
    /// every keystroke and carries up to [`sprag_vt::FIND_MATCH_CAP`] of these, so the ordinary
    /// answer does not pay for the extraordinary one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wrapped: Vec<u16>,
}

/// One LOGICAL line carrying at least one match, with its text — the serde projection of
/// [`sprag_vt::FindLine`], and the DISPLAY view of a search beside [`PaneFind::matches`]'s
/// coordinate view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneFindLine {
    /// The logical line, named by the retained row it begins on — the join key back to
    /// [`PaneMatch::line`].
    pub line: usize,
    /// The line's text: every row it occupies, joined, trailing blanks trimmed. A line that
    /// wrapped over three rows is ONE entry here, reading as the person reads it.
    pub text: String,
}

/// The answer to a pane search: the matches, the matching lines, and whether the scan hit its cap
/// ([`sprag_vt::FIND_MATCH_CAP`]). The serde projection of [`sprag_vt::FindResult`].
///
/// Two views of ONE search, and each consumer reads exactly one: a find bar navigates
/// [`matches`](Self::matches) (coordinates, no text), a grep-like CLI or agent prints
/// [`lines`](Self::lines) (deduped text, no columns). Carrying the text per MATCH instead would
/// repeat a line once per match on it, for a field the interactive consumer never reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneFind {
    /// Every match, in reading order (oldest line first, then by column).
    pub matches: Vec<PaneMatch>,
    /// Every line that carries a match, in order and each ONCE.
    #[serde(default)]
    pub lines: Vec<PaneFindLine>,
    /// `true` when the search stopped at the cap — there may be more past the last match.
    pub truncated: bool,
    /// Why the search could not run, when it could not: the regex engine's own explanation of a
    /// pattern it refused ("unclosed group", "exceeds size limit"). `None` for every answer that
    /// actually searched.
    ///
    /// Only a `regex.<pattern>` query can set it — a literal needle has no syntax to get wrong, so
    /// on the `find.<needle>` path this is structurally always `None`. It is carried rather than
    /// reported as an absent answer because an invalid pattern is a WELL-FORMED address whose value
    /// the engine rejected: answering `Null` would make "your pattern is wrong here" indistinguish-
    /// able from "no such pane", and the caller needs to be told which.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl PaneFind {
    /// Project a VT [`FindResult`](sprag_vt::FindResult) onto the wire shape — the SINGLE
    /// conversion site, so the host's query and a client's deserialize share one field mapping.
    pub(crate) fn from_screen_result(found: &sprag_vt::FindResult) -> Self {
        Self {
            matches: found
                .matches
                .iter()
                .map(|m| PaneMatch {
                    line: m.line,
                    row: m.row,
                    col: m.col,
                    cols: m.cols,
                    wrapped: m.wrapped.clone(),
                })
                .collect(),
            lines: found
                .lines
                .iter()
                .map(|l| PaneFindLine {
                    line: l.line,
                    text: l.text.clone(),
                })
                .collect(),
            truncated: found.truncated,
            error: None,
        }
    }

    /// The answer to a REFUSED regex pattern: no matches, plus the engine's explanation.
    ///
    /// A constructor rather than a caller-built literal, so "a refusal is an empty result carrying
    /// a message" is stated once and every producer of one agrees.
    pub(crate) fn refused(error: &sprag_vt::BadPattern) -> Self {
        Self {
            error: Some(error.message().to_owned()),
            ..Self::default()
        }
    }
}

/// A pane's most recent ATTENTION notification (`OSC 9` / `OSC 777;notify` / `OSC 99`), as a
/// display client reads it off [`HostClient::pane_notification`] — the payload plus the monotonic
/// `seq` that lets a client tell a NEW one from a re-read of the same latched notification (the
/// "unseen attention" badge is this `seq` past the last a viewer acknowledged). A DISPLAY signal,
/// never identity — the child raises it freely, exactly like the window title.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaneNotification {
    /// The notification's short heading, or `None` when the source carried only a body
    /// (`OSC 9`).
    pub title: Option<String>,
    /// The notification's message text (may be empty for a kitty title-only chunk).
    pub body: String,
    /// The monotonic count of notifications this pane's child has raised — always `>= 1` here
    /// (this type exists only when there IS a notification).
    pub seq: u64,
}

/// What the AGENT running in a pane is doing, as a display client reads it off
/// [`HostClient::pane_agent`] — H3's verdict as it arrived, not a re-derivation.
///
/// Present only for a pane some manifest CLAIMS and some rule answered for: `Unknown` is carried as
/// the ABSENCE of this value, which is the same additive discipline the `agent` key follows on the
/// wire (D8) and the reason a workspace of shells produces nothing here. D3 requires that absence to
/// stay distinguishable from [`state`](Self::state) `"idle"`, because "this is not an agent" and
/// "this agent wants you" are opposite instructions to a person reading a pane list.
///
/// [`state`](Self::state) is a `String` rather than [`sprag_detect::AgentState`] deliberately, and
/// the reason is which side of the wire this type lives on: a client parses it from a daemon that
/// may be NEWER than itself, so an unrecognised token has to survive as itself and reach a surface
/// rather than collapse into an enum's fallback. It is the same rule `sprag-mcp`'s mouse token
/// already follows. The vocabulary a daemon of this build can produce is
/// [`sprag_detect::AgentState::wire_str`]'s.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaneAgent {
    /// The state token — `"working"` / `"blocked"` / `"idle"` from
    /// [`sprag_detect::AgentState::wire_str`], never a spelling invented client-side.
    pub state: String,
    /// Which manifest claims the pane (`"claude"`, `"codex"`), or `None` when a rule fired and no
    /// manifest is identified — a modal covering the fingerprint is the measured case (R251).
    pub name: Option<String>,
    /// Which RULE produced the state. D7: a gate that cannot say what it saw cannot be diagnosed,
    /// and this is the whole content of `explain` — a read of the value the detector already
    /// produced, so a surface can never disagree with the verdict it explains.
    pub rule: Option<String>,
    /// Increments on a PUBLISHED change, so a client tells "still blocked" from "blocked again"
    /// without diffing strings — [`PaneNotification::seq`]'s treatment exactly.
    pub seq: u64,
}

/// A pane's most recent OSC 52 clipboard WRITE, as a display client fetches it ON DEMAND off
/// [`HostClient::pane_clipboard_write`] once the write `seq` in the pane list grows past the last
/// it applied. The payload is potentially large (a whole paste), which is why it is fetched on
/// demand rather than carried in the per-poll pane list. A client applies [`text`](Self::text) to
/// each selection [`targets`](Self::targets) names — subject to its clipboard policy — on its own
/// system clipboard.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PaneClipboardWrite {
    /// The selections to set (a single OSC 52 may name both the clipboard and the primary).
    pub targets: ClipboardTargets,
    /// The text to place on each target selection (empty = a clear).
    pub text: String,
    /// The monotonic count of clipboard writes this pane's child has requested — `>= 1` here
    /// (this type exists only when there IS a write).
    pub seq: u64,
}

/// A pane's pending OSC 52 clipboard READ query, as a display client reads it off
/// [`HostClient::pane_clipboard_query`] — the single selection the child asked to read back, plus
/// the monotonic `seq` that lets a client answer each query once. Tiny (a selection + a counter),
/// so — unlike the write payload — it rides the per-poll pane list. A client answers by reading
/// that selection off ITS clipboard (if policy permits) and calling
/// [`HostClient::answer_clipboard_query`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PaneClipboardQuery {
    /// The selection the child wants read back (`c` clipboard / `p` primary).
    pub target: ClipboardTarget,
    /// The monotonic count of clipboard reads this pane's child has requested — `>= 1` here.
    pub seq: u64,
}

/// The typed client protocol a display client reaches the host's panes through —
/// the topology-B wire contract expressed as a trait, with two impls:
///
/// * the in-process [`Host`] (this crate) — resolves the DEFAULT session's current-window
///   [`Workspace`] out of its [`SessionRegistry`] (it has no request to name another);
/// * the GUI's wire client (`sprag-gui`'s `WireHost`) — the SAME method surface
///   over an RPC socket to a `sprag-term` host process.
///
/// The GUI holds a `Box<dyn HostClient>` and reaches every pane ONLY through these
/// methods, so the frontend code is identical whether the `Workspace` lives in its
/// own process (in-process) or another (wire) — that structural equivalence is the
/// point of topology B. Each method addresses a pane by its host [`PaneId`] — the
/// host's OWN stable identity (monotonic, never reused), NOT a display slot: "slots"
/// are a GUI display concept the display client maps onto these ids ITSELF (see
/// `sprag-gui`'s `SlotView`), and the host has no notion of them. [`pane_ids`](HostClient::pane_ids)
/// is the membership source; an absent id returns each method's graceful default.
///
/// [`Host::pane_handle`] is deliberately NOT on this trait: a live [`PanePtyHandle`]
/// cannot cross a wire, so it stays an inherent `Host` method used only to build
/// in-process input surfaces (retired as input clients attach to the host).
pub trait HostClient: crate::wake::WakeSource {
    /// The host's live pane identities, in host order — the ONE membership source a
    /// display client reads (it maps these to its own display slots). Replaces the
    /// former `pane_count` / `occupied_slots` (slot concepts that moved to the GUI's
    /// `SlotView`).
    ///
    /// CONTRACT: yields exactly the panes this client can RENDER right now — membership is
    /// "renderable now", not merely "exists". An impl MAY briefly omit a pane the host has
    /// but it cannot yet render (e.g. a frame not fetched), so a consumer never maps a
    /// frameless pane; the omitted pane appears once it becomes renderable. An impl that
    /// renders the host's state directly reports the live set with no lag; a
    /// transport-mediated impl may lag by however long it takes a new pane to become
    /// renderable. (Each impl's own `pane_ids` documents how it honors this.)
    fn pane_ids(&self) -> Vec<PaneId>;

    /// Pane `id`'s cell DATA scrolled `offset_lines` rows up — the paint buffer a
    /// client renders. `offset_lines == 0` is the live view; a larger offset windows
    /// into scrollback (self-clamped to the retained depth). A `1x1` placeholder if
    /// `id` is absent.
    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer;

    /// [`pane_cells`](Self::pane_cells) TOGETHER WITH everything that describes them — the token
    /// they were projected under, and where their rows end their logical lines.
    ///
    /// **One call rather than three, and that is the whole reason it exists.** A client that read
    /// the cells and then the token would be reading two facts a frame apart: a fetch landing
    /// between them pairs OLD cells with a NEW token, the client stores the new token beside the
    /// old cells, and every row those cells still owe goes unpainted for as long as nothing else
    /// stamps it. The row shares join the pair for the same reason and a second one — they say
    /// where to CUT those cells, so a share read off a later frame re-wraps a line at a column
    /// nothing printed.
    ///
    /// An EMPTY [`PaneFrame::shares`] and a `None` token are the same kind of answer — "this host
    /// cannot say" — and both have one safe reading a caller must take: draw the pane whole, the
    /// way every client did before either fact existed. They are the default here, so an impl need
    /// not implement this at all; they are also the honest answer for a NON-ZERO `offset_lines` on
    /// a host that only knows the live screen.
    ///
    /// See [`ProjectionToken`] for the one direction it promises: equal token ⇒ equal projection,
    /// never the converse. A row whose stamp is unchanged is a row a painter may leave alone — the
    /// same invariant pinion's `TextGrid` already rests on, and the reason the emulator stamps
    /// EVERY row on a palette change.
    fn pane_frame(&self, id: PaneId, offset_lines: usize) -> PaneFrame {
        PaneFrame {
            cells: self.pane_cells(id, offset_lines),
            shares: RowShares::default(),
            token: None,
        }
    }

    /// Pane `id`'s non-cell per-frame facts ([`PaneScrollFacts`]): scrollback depth +
    /// visible rows. A zero-depth / one-row default if `id` is absent.
    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts;

    /// Pane `id`'s OSC 133 prompt-mark positions
    /// ([`prompt_positions`](sprag_vt::Screen::prompt_positions)) — the logical line indices
    /// (from the oldest retained line, the scroll `offset_y` unit) a jump-to-prompt scrolls to.
    /// Queried ON DEMAND (a keyboard jump), NOT per frame, so it never rides the hot
    /// [`pane_cells`](HostClient::pane_cells) path. Empty if `id` is absent or the shell emits no
    /// OSC 133 marks.
    fn pane_prompt_positions(&self, id: PaneId) -> Vec<usize>;

    /// Every literal match of `needle` in pane `id`'s retained output (scrollback + visible), in the
    /// pane's logical line + cell-column coordinate — the find-in-scrollback read.
    ///
    /// Queried ON DEMAND (a find bar's keystroke), NEVER per frame, like
    /// [`pane_prompt_positions`](HostClient::pane_prompt_positions): the search runs where the cells
    /// are, so a client asks for the handful of matches instead of pulling a whole scrollback across
    /// a socket to search it itself. Empty for an absent pane or an empty needle.
    ///
    /// Defaulted to empty — a client that cannot reach a host search (and the test doubles) need not
    /// implement it; [`Host`] and the wire client override it.
    fn pane_find(&self, id: PaneId, needle: &str) -> PaneFind {
        let _ = (id, needle);
        PaneFind::default()
    }

    /// The same read over a REGULAR EXPRESSION rather than literal text — a SEPARATE method for the
    /// same reason it is a separate wire address (`regex.<pattern>` beside `find.<needle>`): a needle
    /// and a pattern are different languages, and the same string means different things in each, so
    /// one entry point taking a mode argument would make what a search MEANS depend on something the
    /// call does not carry.
    ///
    /// A pattern the engine REFUSES answers the normal shape with [`PaneFind::error`] set, never an
    /// empty match list — a caller that cannot tell "your pattern is wrong" from "nothing matched"
    /// retries forever. Case is the pattern language's to decide, so this is case-SENSITIVE and
    /// `(?i)` is the caller's switch (the literal search folds ASCII case instead).
    ///
    /// Defaulted to empty like [`pane_find`](HostClient::pane_find), so a client that reaches no host
    /// search — and the test doubles — need not implement it.
    fn pane_find_regex(&self, id: PaneId, pattern: &str) -> PaneFind {
        let _ = (id, pattern);
        PaneFind::default()
    }

    /// Pane `id`'s current grid `(cols, rows)` — the emulator screen size, which tracks
    /// the last reflow target (the reflow no-op guard + an undock window's intrinsic
    /// open size read it). `(1, 1)` if `id` is absent.
    fn pane_grid_size(&self, id: PaneId) -> (u16, u16);

    /// Resize pane `id`'s PTY (`TIOCSWINSZ`) + emulator — the reflow control path. A
    /// no-op for an absent id. `cell_px` is the display's `(cell_width, cell_height)` in logical
    /// pixels (the caller's font metric), so the PTY winsize carries real `ws_xpixel` / `ws_ypixel`
    /// and XTWINOPS pixel reports answer truthfully; `(0, 0)` means "unknown" and leaves the pane's
    /// last-known cell geometry untouched.
    fn resize(&self, id: PaneId, cols: u16, rows: u16, cell_px: (u16, u16));

    /// Tell the host how many CELLS this client has to give a window
    /// ([`crate::wire::CLIENT_SIZE_METHOD`]) — at attach, and again on every window change.
    ///
    /// The input to tmux's `window-size`: the host cannot measure a client's surface, so a session
    /// with several viewers can only be arbitrated over what each one reports. The default is a
    /// no-op, which is the right answer for an in-process host — it has exactly one surface, so
    /// there is nothing to arbitrate between and the client's own size IS the window.
    fn report_client_size(&self, cols: u16, rows: u16) {
        let _ = (cols, rows);
    }

    /// The session's arbitrated window in cells — the area [`sprag_terminal::tile`] lays the
    /// arrangement out over, and the reason two clients of one session give a pane one size.
    ///
    /// `None` means no arbitration applies: an in-process host (one surface, so the caller's own is
    /// the window) or a daemon no client has reported a size to yet. A caller that gets `None` uses
    /// its own surface, which is both the honest answer and the behaviour that predates this.
    fn window_size(&self) -> Option<(u16, u16)> {
        None
    }

    /// Send a W3C `key` + `mods` to pane `id` — the CLIENT input path. `true` if it
    /// reached the PTY; `false` if `id` is absent, the key is unencodable, or the send
    /// failed.
    #[must_use]
    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool;

    /// REPORT a mouse `event` to pane `id` — the CLIENT pointer path. The host gates it against pane
    /// `id`'s live mouse-tracking mode and encodes an X10 / SGR report at the PTY boundary (an event
    /// the mode does not want is a no-op success). The default is a no-op `false` — a client that
    /// cannot reach the authoritative mode reports nothing; [`Host`] and the wire client override it.
    /// `true` if the event reached the PTY or was legitimately dropped by the mode; `false` if `id`
    /// is absent or the write failed.
    #[must_use]
    fn mouse(&self, _id: PaneId, _event: MouseInput) -> bool {
        false
    }

    /// REPORT a pane FOCUS change to pane `id` — the CLIENT focus path. The host sends `ESC [ I` /
    /// `ESC [ O` when pane `id`'s child has enabled focus reporting (DEC private mode 1004), a no-op
    /// otherwise. The default is a no-op `false`; [`Host`] and the wire client override it. `true` if
    /// the edge reached the PTY or was legitimately dropped (1004 off); `false` if `id` is absent or
    /// the write failed.
    #[must_use]
    fn focus(&self, _id: PaneId, _focused: bool) -> bool {
        false
    }

    /// Write literal committed `text` to pane `id` — the IME-commit client path (typed text,
    /// never bracketed). Empty is a no-op success. `true` if it reached the PTY.
    #[must_use]
    fn send_text(&self, id: PaneId, text: &str) -> bool;

    /// PASTE literal `text` into pane `id` — the clipboard-paste client path, distinct from
    /// [`Self::send_text`]: the host wraps it in the bracketed-paste markers (and filters an
    /// embedded end marker) when pane `id`'s child has enabled DEC private mode 2004. The default
    /// forwards to [`Self::send_text`] (raw, unbracketed) — a safe legacy fallback for a client
    /// that cannot reach the authoritative mode; [`Host`] and the wire client override it to do
    /// the mode-aware bracketing at the PTY boundary. Empty is a no-op success.
    #[must_use]
    fn paste(&self, id: PaneId, text: &str) -> bool {
        self.send_text(id, text)
    }

    /// Whether pane `id`'s CHILD has exited — the pane is still there, but nothing is running in
    /// it. `false` for an absent id (nothing that is not there is dead).
    ///
    /// The pane survives its child ([`kill_pane`](Self::kill_pane) is what removes one), which is
    /// what lets a command's output be read after it finishes — and is exactly why a client needs
    /// this: a frozen screen means "done" or "hung", and only the host knows which.
    ///
    /// Defaulted to `false` — a client that cannot reach the authoritative liveness (and the test
    /// doubles) reports every pane live, the pre-liveness behaviour.
    fn pane_is_dead(&self, id: PaneId) -> bool {
        let _ = id;
        false
    }

    /// Pane `id`'s full text (scrollback + visible) — the a11y text SSOT. Empty if
    /// `id` is absent.
    fn pane_full_text(&self, id: PaneId) -> String;

    /// Pane `id`'s command label (the a11y node name). Empty if `id` is absent.
    fn pane_command_label(&self, id: PaneId) -> String;

    /// The current window's LOGICAL arrangement of its TILED panes — which panes are
    /// split, in what order, at what proportion — reconciled against the live pane set,
    /// with the revision it is at.
    ///
    /// Logical ONLY — it carries no rect, because pixel geometry belongs to whichever
    /// client is rendering (a TUI and a GUI at different sizes project the same tree
    /// differently). A client PROJECTS this into its own surface; it never receives
    /// pixels here.
    ///
    /// A client re-reads exactly when [`revision`](sprag_terminal::LayoutSnapshot::revision)
    /// changes, which is what keeps its tree a projection rather than a fork.
    ///
    /// **Scope note:** this and its two writes are WINDOW state on an otherwise
    /// pane-addressed trait. They live here because both impls and the client's one
    /// `Box<dyn HostClient>` already existed; when the window surface grows (window list,
    /// select-window) they should move to their own mux/window client trait.
    fn layout(&self) -> LayoutSnapshot;

    /// The pane the current window is ON — the daemon's active pane, which a display client's own
    /// focus is a PROJECTION of (see [`Self::select_pane`]).
    ///
    /// `None` for a window holding no panes, and for an impl with no daemon behind it. Read from
    /// whatever the impl already mirrors — this is called on every re-tile, so an impl that made a
    /// round trip here would put one on the keystroke path.
    fn active_pane(&self) -> Option<PaneId> {
        None
    }

    /// Tell the daemon the user moved to pane `id` — tmux `select-pane`, from the client side.
    ///
    /// The other half of [`Self::active_pane`], and the reason a client PUBLISHES rather than
    /// merely remembering: which pane the user is on is session state (every attached client
    /// follows it, a reattaching one inherits it, and a pane verb given no target acts on it), so a
    /// client that kept its focus to itself would be a second authority on one fact.
    ///
    /// Sent for USER INTENT only. A client that cannot SHOW the active pane — a terminal client
    /// while the daemon's active pane is floating — moves its own focus ring locally and says
    /// nothing, because "I cannot display that" is not the user choosing something else, and
    /// publishing it would fight the client that can.
    ///
    /// The default is a no-op `false`; the wire client overrides it. `true` if the daemon accepted.
    #[must_use]
    fn select_pane(&self, _id: PaneId) -> bool {
        false
    }

    /// Move the session's active pane to its NEIGHBOUR in `dir` — tmux `select-pane -L/-R/-U/-D`,
    /// and the wire's `{dir}` form of [`crate::wire::SELECT_PANE_ACTION`].
    ///
    /// Answers whether the focus MOVED — the same shape [`swap_toward`](Self::swap_toward) and
    /// [`resize_toward`](Self::resize_toward) already have, so the three directional questions are
    /// one question asked three times.
    ///
    /// **It answered the landed `PaneId` until R316, and no caller ever read it** — both frontends
    /// re-derive the focus through their own reconcile, which is the authority split stated below.
    /// What they could not get was the EDGE: `select-pane -L` at the left edge answered
    /// `Some(the pane you were already on)`, indistinguishable from a move, so a key pressed against
    /// a wall was silent. This method's own doc named that gap — *"the day one wants \"you are at
    /// the edge\", this signature is where the fact stops"* — and the fact does not stop here now.
    ///
    /// Reaching the edge is still not a REFUSAL: `select-pane -L` at the left edge is a well-formed
    /// request whose honest answer is "nothing that way", which is what `false` says.
    ///
    /// # The one write on this trait that names no pane, and why
    ///
    /// Every other write here states its target ([`split`](Self::split), [`zoom_pane`](Self::zoom_pane),
    /// [`kill_pane`](Self::kill_pane), [`select_pane`](Self::select_pane)) because a gesture happened
    /// ON a pane the user picked out. A directional move is the other kind of request: it names no
    /// pane at either end, and both ends are facts the daemon already holds — the ORIGIN is the
    /// active pane, which this client only [projects](Self::active_pane), and the DESTINATION is a
    /// property of the arrangement, which this client only mirrors.
    ///
    /// So the direction travels and the daemon resolves it, for two reasons that a client-side
    /// resolve gives up:
    ///
    /// * **Atomicity.** `crate::host::select_pane` walks the arrangement and selects under ONE
    ///   registry lock. A client that read the neighbour and then selected it by id would be
    ///   reading a MIRROR — staler than a released lock — and could land the user on a pane that had
    ///   exited in between.
    /// * **One authority.** `sprag select-pane -L` from a shell already sends the direction. A
    ///   client that resolved its own would be a second answer to the same question, which is the
    ///   fork [`LayoutWire::neighbor`](sprag_terminal::LayoutWire::neighbor) exists to prevent one
    ///   level down — and the rival's is exactly that fork, computed from its last composed frame's
    ///   rectangles rather than from the arrangement.
    ///
    /// **The honest cost**: a client showing a focus ring that is NOT the session's active pane —
    /// a terminal client while the active pane is floating, the one case
    /// [`active_pane`](Self::active_pane) documents — moves from the session's pane rather than
    /// from its ring. A floating pane is in no arrangement, so that call answers unmoved and the
    /// ring stays where it was.
    ///
    /// Defaulted to `false`, like [`swap_toward`](Self::swap_toward) — the wire client overrides it.
    #[must_use = "`false` is the EDGE, which no repaint can show because nothing moved"]
    fn select_toward(&self, dir: PaneDir) -> bool {
        let _ = dir;
        false
    }

    /// Trade the session's active pane with its NEIGHBOUR in `dir` — tmux `swap-pane`, and the
    /// wire's `{dir}` form of [`crate::wire::SWAP_PANE_ACTION`].
    ///
    /// [`select_toward`](Self::select_toward)'s twin, and everything that method's own section says
    /// about naming no pane at either end holds here unchanged: the ORIGIN is the active pane, which
    /// this client only projects, and the PARTNER is a property of the arrangement, which this
    /// client only mirrors. So the direction travels and the daemon resolves it, under one lock,
    /// with `sprag swap-pane -L` from a shell reaching the same code path.
    ///
    /// Answers whether the arrangement MOVED. That is less than the wire carries — the answer names
    /// both panes and says WHY in [`crate::wire::SwapHow`]'s four words — and the reduction is
    /// deliberate for [`select_toward`](Self::select_toward)'s reason: a client that draws the
    /// arrangement re-reads it, and nothing here has anything to SAY about an edge. The day one
    /// wants "you are at the edge", this signature is where the fact stops.
    ///
    /// **The user's cursor does not follow, and does not need to**: the active pane is a PANE, not a
    /// cell, so the pane a user is typing into is still theirs after it has moved. That is a
    /// property of the daemon's swap rather than something a client arranges.
    ///
    /// Defaulted to `false`, like [`new_pane`](Self::new_pane) — the wire client overrides it.
    #[must_use = "`false` is the EDGE, which no repaint can show because nothing moved"]
    fn swap_toward(&self, dir: PaneDir) -> bool {
        let _ = dir;
        false
    }

    /// Move the boundary that bounds the session's active pane on `dir`'s axis, by `cells` — tmux
    /// `resize-pane -L|-R|-U|-D`, and the wire's [`crate::wire::RESIZE_PANE_ACTION`].
    ///
    /// [`swap_toward`](Self::swap_toward)'s sibling, and what that method's own section says about
    /// naming no pane holds unchanged. It has a STRONGER version of the same argument besides:
    /// converting `cells` into a share needs the split's region, which needs the arbitrated window
    /// — a fact a client does not have even in principle, since it is derived from every OTHER
    /// attached client's report too. A client that did the conversion from its own rectangle would
    /// move the boundary a different distance than the user asked for whenever it was not the
    /// largest client.
    ///
    /// Answers whether the arrangement MOVED, dropping the wire's `cells` and
    /// [`crate::wire::ResizeHow`] for [`select_toward`](Self::select_toward)'s reason: a client
    /// that draws the arrangement re-reads it, and nothing here has anything to SAY about a
    /// boundary that stopped short. The day a frontend wants to tell the user, this signature is
    /// where the fact stops.
    ///
    /// Defaulted to `false`, like [`new_pane`](Self::new_pane) — the wire client overrides it.
    #[must_use = "`false` is a boundary that had nowhere to go, which no repaint can show"]
    fn resize_toward(&self, dir: PaneDir, cells: u16) -> bool {
        let _ = (dir, cells);
        false
    }

    /// Install `tree` as the current window's arrangement, returning the CANONICAL result
    /// — the write half of the arc (see [`sprag_terminal::layout`]).
    ///
    /// `expected` is the revision this gesture was authored against; the write is REFUSED if
    /// the arrangement has moved on (another client, a plugin's spawn), because a gesture
    /// means "in the layout I am looking at". The answer is the tree as the host stores it,
    /// with every divider the client minted now named, so the caller adopts it directly
    /// rather than re-reading — and a refused write answers with the arrangement actually in
    /// force, so a client always learns the truth it must project.
    #[must_use = "the CANONICAL arrangement, including a write the daemon REFUSED because the layout moved"]
    fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot;

    /// Take pane `id` out of the tiling (`floating == true`) or put it back, returning the
    /// resulting arrangement.
    ///
    /// Floating collapses the pane's leaf host-side, so the siblings reclaim its space. A
    /// pane docked back with no gesture to place it returns to the place its float captured
    /// (`sprag_terminal::LeafHome`): the pane SEQUENCE comes home, and the shares come home
    /// too when its sibling was a bare leaf. It falls back to the arrangement's END only when
    /// the home cannot be honored — its sibling has since exited, or been floated out itself.
    /// WHERE a floating pane's window then sits on screen is the client's own business.
    ///
    /// REFUSED if it would leave the window tiling nothing (a terminal window always shows a
    /// terminal). The answer then carries the arrangement still in force, so a client that
    /// asked anyway learns the truth rather than being trusted to have checked first.
    #[must_use = "the resulting arrangement, which the caller adopts rather than re-reads"]
    fn set_floating(&self, id: PaneId, floating: bool) -> LayoutSnapshot;

    /// The scoped session's windows, in order — each window's name and whether it is CURRENT
    /// (tmux "windows"). The list a tabbed client draws; it re-reads when the scene revision
    /// moves (a new / killed / renamed / selected window bumps it).
    ///
    /// **Scope note (extended):** the window LIST + the three window WRITES below join `layout`
    /// and its writes as window-level members of an otherwise pane-addressed trait. They live
    /// here for the same reason ([`layout`](Self::layout)'s note): both impls and the client's one
    /// `Box<dyn HostClient>` already exist. A future extraction of this whole group into a
    /// dedicated window/mux client trait is tracked, not done — so this increment stays focused.
    fn windows(&self) -> Vec<WindowInfo>;

    /// Make the window named `name` the scoped session's current window (tmux `select-window`), and
    /// answer the name the daemon RECORDED. Every attached client's next read then projects that
    /// window. [`None`] when no window carries that name, or the host could not be asked.
    ///
    /// **It answered `()` until R316, which made a key bound to `select-window -t <a name that is
    /// not there>` a silent no-op with nothing for a client to report.** The daemon has always
    /// refused an unknown name; what was missing was the fact crossing back. The name comes back
    /// rather than being echoed for [`switch_session_named`](Self::switch_session_named)'s reason —
    /// the daemon's recorded spelling, never the caller's argument.
    /// # It takes a REFERENCE, so the caller says which address it holds
    ///
    /// A keystroke bound to `select-window -t build` holds a name a person TYPED, and means whatever
    /// carries it when the key is pressed. A pointer surface holds a row it PAINTED, and means the
    /// window that was on it. Both are honest and they land on different windows across a rename —
    /// which is why the two are one parameter with two arms rather than two methods: a client picks
    /// its address by naming it, and no caller can reach for the wrong door by accident.
    ///
    /// Unlike [`kill_window`](Self::kill_window) the NAME arm survives here, because the two verbs
    /// differ in what being wrong costs: a select that lands on a stranger is undone by one
    /// keystroke, and refusing to offer a name-addressed select would take the verb away from every
    /// keybinding in every config file.
    #[must_use = "a select that did not land is the only way a client learns the name is not there, \
                  and a client that drops it is the silent key R316 removed"]
    fn select_window(&self, window: &crate::wire::WindowRef) -> Option<String>;

    /// Walk the scoped session's windows one step, WRAPPING, and answer the window it landed on —
    /// tmux `next-window` / `previous-window` (`select-window -n` / `-p`).
    ///
    /// A SECOND method beside [`select_window`](Self::select_window) rather than one taking a
    /// grammar, which is the shape [`select_toward`](Self::select_toward) already has beside
    /// [`select_pane`](Self::select_pane): the wire grammar
    /// ([`SelectWindowAsk`](crate::wire::SelectWindowAsk)) is what the CLI and the keybinding BUILD,
    /// and the trait offers the two questions a client actually asks.
    ///
    /// **The step is resolved by the DAEMON, never here.** A client walking its own `windows` mirror
    /// would be a second answer to this question, derived from a list that can be one revision
    /// behind the session it is naming — the argument R299/R300 settled for the pane walk, and the
    /// reason that one carries no client-side geometry either.
    ///
    /// [`None`] when the host could not be asked. It cannot mean "nowhere to go": a session always
    /// holds a window, so the walk always lands.
    #[must_use = "the window a walk landed on is what a client paints; dropping it is the silent \
                  key R316 removed"]
    fn select_window_toward(&self, step: OrderStep) -> Option<String>;

    /// Move a window's PLACE in the scoped session's order — tmux `move-window`. Answers the window
    /// that was placed and WHAT happened, or [`None`] if the host could not be asked or refused.
    ///
    /// `window` [`None`] means the session's CURRENT window, which is what a keypress means. It is
    /// resolved by the DAEMON and not here, for [`select_window_toward`](Self::select_window_toward)'s
    /// reason: a client reading its own `windows` mirror for "the current one" would be naming a
    /// window off a list that can be a revision behind the session it is naming.
    ///
    /// The ANSWER is a [`PlaceHow`] and not a `bool`, because three of its four words mean the order
    /// did not move and each has a different remedy — the discrimination R301 gave the swap, and the
    /// one the rival's `move_tab` collapses into `false`.
    #[must_use = "three of `PlaceHow`'s four words mean the order did NOT move, and a client that \
                  drops them tells the user nothing about a key that did nothing"]
    fn move_window(&self, window: Option<&str>, place: &WindowPlace) -> Option<(String, PlaceHow)>;

    /// Create a window in the scoped session, born with a shell, and select it (tmux
    /// `new-window`), returning its name.
    fn new_window(&self) -> String;

    /// Kill the window `window` IDENTIFIES, in the scoped session (tmux `kill-window`), answering
    /// HOW FAR the kill cascaded — or [`None`] if nothing was killed (a window already gone, a
    /// refusal, a failed request).
    ///
    /// # Why the one destructive window verb takes an identity and no name
    ///
    /// Because every client that kills a window decided WHICH window at some earlier instant — a
    /// GUI row painted into a menu, a TUI prompt armed on the current window — and a confirmation
    /// dialog sits between that instant and the act. A name committed across that gap lands on
    /// whatever holds it by the time the person says yes, which was MEASURED at the registry
    /// destroying a window nobody pointed at while the one on the row survived
    /// (`a_kill_lands_on_the_window_pointed_at_and_a_name_lands_on_whatever_holds_it`).
    ///
    /// The name-addressed door that used to be here is DELETED rather than kept beside this one —
    /// `HostClient::join_pane`'s treatment at R329, on the verb where being wrong cannot be undone.
    /// The wire action keeps its `window` arm for `sprag kill-window -t s build`, where a person
    /// TYPED the name and means whatever holds it when they press Enter.
    ///
    /// [`Ended`] rather than `()`, which is what this answered until R325 and the last acting method
    /// in this trait to be widened (R316's shape, met a final time). The word is not decoration: a
    /// session's LAST window takes the SESSION with it, and measured on a live client with
    /// `detach-on-destroy next`, `prefix &` destroyed the person's session, moved them silently to a
    /// neighbouring one, and **left the status row naming the session that had just died**. Nothing
    /// in the product could say what had happened because nothing was told.
    ///
    /// The word is the DAEMON's ([`crate::wire::ENDED_KEY`]) for
    /// [`kill_pane`](Self::kill_pane)'s reason: whether a session survived its last window is a fact
    /// only the process that performed the kill holds, and a client counting its own rows would
    /// answer from a snapshot taken before the kill.
    #[must_use = "the [`Ended`] word says HOW FAR the cascade went — a window kill that took the \
                  SESSION is not something a re-read tells the person who pressed the key"]
    fn kill_window(&self, window: sprag_terminal::WindowId) -> Option<Ended>;

    /// PIN the size of the scoped session's CURRENT window, or un-pin it, answering what the daemon
    /// stored AND the policy it is arbitrating under — [`None`] if it refused (tmux
    /// `resize-window`).
    ///
    /// # It names no window, for [`rename_window`](Self::rename_window)'s reason
    ///
    /// The CLI verb takes an optional window; this takes none. A keystroke can only ever mean *the
    /// window I am on*, and a client that read its mirror for the current window's NAME and then
    /// resized by that name would be addressing a fact about the past — the impostor shape R304
    /// measured. There is no target here to go stale.
    ///
    /// # Why the ANSWER is a [`WindowPin`](crate::wire::WindowPin) and not a rectangle
    ///
    /// Three of the four spellings a caller may send are DESCRIPTIONS the daemon resolves, so the
    /// rectangle is news. The policy is the other half and it is the half a display client cannot
    /// get anywhere else: a pin stored under a policy that does not read it moves nothing, so a key
    /// that pinned would look exactly like a key that is not bound. Reading the config file HERE to
    /// find that out would be the CLI's old mistake rebuilt in a client that re-reads its config per
    /// keystroke (R319) — see the type's own doc.
    #[must_use = "a resize the daemon REFUSED, and a size it stored under a policy that ignores it, \
                  are both facts no repaint carries"]
    fn resize_window(&self, size: crate::window::SizeRequest) -> Option<crate::wire::WindowPin>;

    /// Rename the scoped session's CURRENT window, answering the name the daemon RECORDED — or
    /// [`None`] if it refused (tmux `rename-window`).
    ///
    /// # It names no window, and that is the whole design
    ///
    /// The CLI verb takes an optional window; this takes none, so the daemon resolves *the current
    /// one* under its own lock at the moment the rename happens. A client that read its mirror for
    /// the current window's name and then renamed BY that name would be addressing a fact about the
    /// past — the shape R304 measured landing on an impostor that had taken a freed name. There is
    /// no target here to go stale.
    ///
    /// The ANSWER is the recorded name because a name is trimmed and validated on the way in
    /// ([`WindowName`](sprag_terminal::WindowName)), so a caller that echoed its own argument would
    /// paint a name the window does not have. [`None`] has one cause once a caller has checked the
    /// grammar with that same type: the name is already another window's.
    #[must_use = "a rename the daemon REFUSED — the name is taken, or unusable — is a fact no repaint carries"]
    fn rename_window(&self, name: &str) -> Option<String>;

    /// Rename the session this connection is scoped to, answering the name the daemon RECORDED — or
    /// [`None`] if it refused (tmux `rename-session`).
    ///
    /// The scope IS the target, so like [`rename_window`](Self::rename_window) there is no name to
    /// go stale — and for a display client the scope is its ATTACHMENT (R303), so this renames the
    /// session the user is looking at even if somebody renamed it a moment ago. The daemon carries
    /// the session's change channel and every attachment across with the name (R302), which is why
    /// a client can ask for this without knowing who else is watching.
    #[must_use = "a rename the daemon REFUSED is a fact no repaint carries"]
    fn rename_session(&self, name: &str) -> Option<String>;

    /// Give the pane with `id` the name `name`, answering the name the daemon RECORDED — or
    /// [`None`] if it refused.
    ///
    /// The one rename that carries a TARGET, because a [`PaneId`] is an identity: registry-unique,
    /// stable, and not the thing being changed. A pane NAME is an address (R295), which is what
    /// makes this worth a gesture rather than a decoration.
    #[must_use = "a rename the daemon REFUSED is a fact no repaint carries"]
    fn rename_pane(&self, id: PaneId, name: &str) -> Option<String>;

    /// Create a pane in the scoped session's CURRENT window, born with a shell (tmux
    /// `split-window`), returning its id — or `None` if the child could not be started.
    ///
    /// Named for its EFFECT in this trait's own vocabulary (`new_window` / `new_session` above),
    /// not for tmux's verb: what "splits" is the ARRANGEMENT, and that happens by itself. A window's
    /// [`LayoutTree`](sprag_terminal::LayoutTree) reconciles against the live pane set, so a pane
    /// born here is appended to the arrangement on the next read with nothing said about it —
    /// which is why this takes no position and returns no layout.
    ///
    /// Takes no argv: the pane is born with `$SHELL`, the same default the `spawn` wire action
    /// applies to a request that names no `cmd`. A client that wants a specific program is asking
    /// for something else (a run-in-a-new-pane target), which needs remain-on-exit first.
    ///
    /// Defaulted to `None` — a display client that never creates panes (and the test doubles) need
    /// not implement it; the in-process [`Host`] and the wire client override it.
    #[must_use = "[`None`] is a pane that was NOT born, which an arrangement re-read cannot distinguish from one that was"]
    fn new_pane(&self) -> Option<PaneId> {
        None
    }

    /// Divide `target`'s cell in the scoped session's current window and put a new shell in the
    /// half it opens (tmux `split-window -h` / `-v`), returning the new pane's id — or `None` if
    /// the split was refused.
    ///
    /// [`new_pane`](Self::new_pane) with a PLACE, and the distinction is the whole reason this
    /// exists. An append states where only by CONVENTION (the rightmost spine), which is all a
    /// client with a pointer needs — it can rearrange afterwards by writing a whole tree back
    /// through [`set_layout`](Self::set_layout). A client that draws in character cells has no such
    /// gesture, so "put a shell below this one" has to be sayable in one request or it is not
    /// sayable at all.
    ///
    /// `dir` names how the two halves are LAID OUT, not which way the line between them is drawn:
    /// `Horizontal` puts the new pane to the RIGHT of `target`, `Vertical` BELOW it — the host's
    /// [`SplitDir`] vocabulary and tmux's `-h`/`-v`, so one word means one thing from the keystroke
    /// to the tree. `before` puts it on the other side instead (left of, or above), which is tmux's
    /// `-b`.
    ///
    /// `target` is stated explicitly here even though the daemon now HOLDS an active pane
    /// ([`crate::wire::SELECT_PANE_ACTION`]), and the two are not in tension: this is the
    /// in-process trait, whose caller is code that already knows which pane it means. Defaulting a
    /// Rust argument to session state would hide that choice inside a method signature; the WIRE
    /// action, whose caller may be a person or an agent with no pane in hand, is where the default
    /// belongs and is where it lives.
    ///
    /// **`None` means the split did not happen** — the target holds no leaf in the current window
    /// (it exited, it is floating, or it belongs to another window), or the child could not be
    /// started. It never silently appends: a direction the user spelled is a request, and appending
    /// instead would be the same lie as accepting `-h` and ignoring it.
    ///
    /// Defaulted to `None`, like [`new_pane`](Self::new_pane) — the wire client overrides it.
    #[must_use = "[`None`] is a split that did not happen, and the arrangement looks the same either way"]
    fn split(&self, target: PaneId, dir: SplitDir, before: bool) -> Option<PaneId> {
        let (_, _, _) = (target, dir, before);
        None
    }

    /// Fill `target`'s window with `target` alone, or give the arrangement back — tmux
    /// `resize-pane -Z`, and the gesture half of [`crate::wire::ZOOM_PANE_ACTION`].
    ///
    /// `on` absent TOGGLES, so one binding is a switch whichever pane it is aimed at; `Some(true)` /
    /// `Some(false)` are the explicit forms. The whole tri-state travels rather than being collapsed
    /// here, because it is one vocabulary from the CLI flag (`-Z` / `-u`) through the bound action to
    /// the wire argument, and a method that only toggled would make an explicit binding
    /// unexpressible.
    ///
    /// `target` is stated explicitly for [`split`](Self::split)'s reason and one more: a gesture
    /// happened ON a pane. The wire action would resolve an absent `pane` to the session's active
    /// one, and a client that leaned on that could zoom somewhere else entirely if the active pane
    /// moved between the click and the call — the tear
    /// [`select_pane`](Self::select_pane) sends an id rather than a direction to avoid.
    ///
    /// [`ZoomOutcome`] rather than a bool, because `{zoomed, changed}` is total over four distinct
    /// cases (now filling / already filling / arrangement back / arrangement already showing) and a
    /// caller handed one flag would have to guess which it had.
    ///
    /// **`None` means the zoom did not happen**: `target` names no pane of the scoped session, or
    /// one its window does not TILE because a client floated it out. Both are refusals rather than
    /// quiet no-ops, which is the rule the whole placement family shares.
    ///
    /// Defaulted to `None`, like [`new_pane`](Self::new_pane) — the wire client overrides it.
    #[must_use = "[`None`] is a zoom that was refused (no such pane, or it is floating)"]
    fn zoom_pane(&self, target: PaneId, on: Option<bool>) -> Option<ZoomOutcome> {
        let (_, _) = (target, on);
        None
    }

    /// Close pane `id` — kill its child and drop it from the window (tmux `kill-pane`), answering
    /// HOW FAR the kill cascaded, or [`None`] if nothing was killed (an absent id, a refusal, a
    /// failed request).
    ///
    /// DESTRUCTIVE and unconditional: it ends a running program and takes that pane's scrollback
    /// with it. Asking first is a CLIENT's job — this is the performer, exactly as
    /// [`kill_window`](Self::kill_window) is.
    ///
    /// This is the ONE way a pane leaves a live window, and it is worth saying what it is NOT: a
    /// child that exits on its own does **not** remove its pane. Nothing reaps an EOF pane — the
    /// daemon's reaper reads
    /// [`is_eof`](sprag_terminal::PanePty::is_eof) only to decide whether ANY pane is still live —
    /// so an exited pane stays in the pool showing its last screen, and this is what removes it.
    ///
    /// **The window emptied by the last pane IS closed** (R309), which is what
    /// [`Ended`] exists to report: the window, then its session, then the
    /// server. Until R309 it was not, and the sentence here said so — arguing that an emptied
    /// window is *"exactly as a window whose panes all ran `exit`"*. It is not: those panes are
    /// still there showing their exit statuses, which is why nothing reaps them, whereas an
    /// emptied window tiles nothing and both frontends draw it as a void.
    ///
    /// [`Ended`] rather than a bool, for the reason
    /// [`zoom_pane`](Self::zoom_pane) takes a [`ZoomOutcome`]: the answer is total over four
    /// distinct cases and a caller handed one flag would have to guess which it had — and here the
    /// cases differ by whether the caller's own session still exists.
    ///
    /// Defaulted to `None`, like [`new_pane`](Self::new_pane).
    #[must_use = "the [`Ended`] word says HOW FAR the cascade went, which nothing else states"]
    fn kill_pane(&self, id: PaneId) -> Option<Ended> {
        let _ = id;
        None
    }

    /// Break the pane `id` out of its window into a NEW window of the scoped session (tmux
    /// `break-pane`), returning the new window's name — or `None` if the move was refused (the
    /// pane's window has only that pane, an explicit `name` is already taken, or no window holds
    /// `id`). The pane is MOVED whole (no re-spawn); the new window is selected.
    ///
    /// Defaulted to `None` — a display client that never breaks panes (and the test doubles) need
    /// not implement it; the in-process [`Host`] and the wire client override it.
    #[must_use = "[`None`] is a refusal — the pane is its window's only one"]
    fn break_pane(&self, id: PaneId, name: Option<&str>) -> Option<String> {
        let _ = (id, name);
        None
    }

    /// HOW pane `id`'s child ended — its exit code, or the signal that killed it — and `None` while
    /// it runs, before it has been reaped, or when no pane holds `id`.
    ///
    /// The refinement of [`pane_is_dead`](Self::pane_is_dead), and deliberately NOT a replacement
    /// for it. `pane_is_dead` answers "is this finished?", which is what makes a stopped screen
    /// readable; this answers "and did it work?", which only a reaped status can and which is
    /// therefore sometimes unavailable ([`PaneExit`](sprag_terminal::PaneExit) has the full
    /// distinction). Folding the two into one `Option` would have made "dead, status unknown"
    /// unsayable — the state every dead pane passes through.
    ///
    /// Defaulted to `None`, like [`project`](Self::project): a test double, and a client that only
    /// needs liveness, need not implement it.
    fn pane_child_exit(&self, id: PaneId) -> Option<sprag_terminal::PaneExit> {
        let _ = id;
        None
    }

    /// The PROJECT governing pane `id` — the commands its `.sprag.toml` declares — or `None` when
    /// the pane is in no project, its working directory is not local (a remote workspace), or no
    /// window holds `id`.
    ///
    /// `Some(Err(_))` reports a project whose config is UNUSABLE, which a client must show rather
    /// than treat as an empty list: a typo in a committed config is something its author needs to
    /// hear about ([`crate::project`] has the whole rationale, including why nothing here runs).
    ///
    /// The error is the RENDERED report, a `String`, for the reason
    /// [`global_commands`](Self::global_commands)'s is: the only consumer paints the sentence, and
    /// only THIS end knows which file it is about. Carrying [`crate::ProjectError`] across the wire
    /// meant re-rendering at the far end from a message that had already been rendered here — and
    /// its `Display` prefixes the filename, so a wire client's report named `.sprag.toml` twice.
    /// The type is still the right shape where it is BUILT (`project::load` distinguishes
    /// unreadable / malformed / invalid, and its tests match on that); it is only the crossing that
    /// wants a sentence.
    ///
    /// A READ that touches the filesystem, so it is asked ON DEMAND (a palette opening, a `sprag
    /// run`) and never per frame. Defaulted to `None`, like [`break_pane`](Self::break_pane) — a
    /// test double need not implement it.
    fn project(&self, id: PaneId) -> Option<Result<crate::Project, String>> {
        let _ = id;
        None
    }

    /// The USER's own declared commands — the ones available in every pane — or `None` when no
    /// config has been written.
    ///
    /// Takes no pane, which is the whole difference from [`project`](Self::project): these do not
    /// depend on where any shell happens to be, so they are offered even in a pane that is in no
    /// project at all. A client shows BOTH lists; which one a name belongs to is the client's
    /// ordering decision, not a fact this hides.
    ///
    /// `Some(Err(_))` is a config that EXISTS and is unusable, reported rather than swallowed for
    /// the reason a project's is — and its message names `config.toml`, so a user reading it in a
    /// palette beside a project's error knows which file to open.
    ///
    /// The error is the RENDERED report, a `String`, not a typed error — deliberately. Only one
    /// consumer exists and it paints the sentence; carrying a type would mean re-rendering it at the
    /// far end of the wire, and the side that receives it does not know which file it is about. (The
    /// project slot beside this one does re-render, and consequently names its file twice in a
    /// wire-client's report — the defect this shape exists to not repeat.)
    ///
    /// A READ that touches the filesystem, so it is asked ON DEMAND (a palette opening), never per
    /// frame. Defaulted to `None` — a test double need not implement it.
    fn global_commands(&self) -> Option<Result<crate::UserConfig, String>> {
        None
    }

    /// Why the agent manifests IN FORCE are not the ones the user's `config.toml` declares — `None`
    /// when they are, when the user declares none, or when this host detects no agents at all.
    ///
    /// A rendered sentence, like [`global_commands`](Self::global_commands)'s error and for the same
    /// reason. It is not `Result`-shaped, though, because there is no success value to carry: the
    /// ruleset itself never crosses the wire (a client evaluates nothing — D2), so the only thing a
    /// client can be told is what went wrong.
    ///
    /// GLOBAL, where every other H3 answer is per-pane. A typo in one `[[agent]]` block does not
    /// mis-detect one pane; it drops the whole daemon back to whatever list last worked, so the
    /// report belongs to the workspace rather than to any pane in it. That is also why it cannot be
    /// derived from [`pane_agents`](Self::pane_agents): a workspace of shells reports no agents
    /// whether the file is broken or perfect.
    ///
    /// Defaulted to `None`, and the DEFAULT is the in-process arm's real answer, exactly as
    /// [`pane_agent`](Self::pane_agent)'s is: the manifests are read by the daemon's waker, so a host
    /// without one is not being modest about a fact it holds — it holds none.
    ///
    /// A READ that touches NO disk (the daemon rendered this when it last read the file), but it is
    /// still asked on demand — a palette opening, a `sprag agent` — because it is a report a person
    /// reads, not a frame input.
    fn agent_manifest_report(&self) -> Option<String> {
        None
    }

    /// Move the pane `id` into the window `dst` IDENTIFIES (tmux `join-pane`), returning whether
    /// the source window was CLOSED (a join that emptied it) — or `None` if the move was refused
    /// (`id` already lives there, no window holds `id`, or `dst` names a window that is gone).
    ///
    /// # The ONLY join a display client can perform, and that is the design
    ///
    /// There was a name-addressed twin here until R329 and it is deliberately gone. A name is the
    /// right address for a caller who TYPED one and means whatever holds it at the instant they
    /// press Enter — which is the `sprag join-pane` verb, and the wire action still has the arm for
    /// it ([`crate::wire::WindowRef::Named`]). It is the wrong address for every caller this trait
    /// has: a display client commits a join from a ROW, painted at one instant and clicked at
    /// another, so the name it holds is a fact about the past.
    ///
    /// That was MEASURED, not reasoned about — at the registry, a rename of the destination away
    /// and of a sibling onto the freed name lands the join in a window nobody chose
    /// (`a_join_lands_on_the_window_picked_and_a_name_lands_on_whatever_holds_it`) — and every
    /// surface in this product was committing a name: the GUI's `Move pane to window …` row painted
    /// one, and `join-pane` could not be bound to a key at all, because a chooser pick carries a
    /// [`sprag_terminal::WindowId`] and there was nothing to hand it to. Removing the door is what
    /// keeps the second half of that fix from being re-opened by the next client.
    ///
    /// Defaulted to [`None`] so an impl that cannot move panes need not say so twice; it collapses
    /// the daemon's stated reason exactly as its neighbours do, which is filed rather than
    /// pre-solved (a refused `scene/invoke` stores that sentence through R325's funnel and a display
    /// client paints it in preference to its own word).
    #[must_use = "[`None`] is a refusal and `Some(false)` is a move that changed nothing"]
    fn join_pane_into(&self, id: PaneId, dst: sprag_terminal::WindowId) -> Option<bool> {
        let _ = (id, dst);
        None
    }

    /// Put pane `id` BESIDE pane `target`, dividing the target on `dir` — tmux `move-pane`.
    /// Answers whether the move emptied and closed the source window, or [`None`] for a refusal.
    ///
    /// # Why this did not exist until R328, and what it cost
    ///
    /// The wire action has taken TWO PANE IDS and no name since it was built, which is exactly the
    /// shape a chooser's [`Target::Pane`](crate::chooser::Target) carries — and yet `move-pane` was
    /// one of the four verbs a keystroke could mean and sprag did not bind. The register said the
    /// chooser was the missing half; measuring said the chooser was ready and THIS was missing, so
    /// no client could reach the action a keystroke needed.
    ///
    /// The neighbour it sits beside answers `Option<bool>` too, and both collapse the daemon's
    /// stated reason into [`None`] — which costs nothing today, because a refused `scene/invoke`
    /// stores that sentence through the funnel R325 built and a display client paints it in
    /// preference to its own word. It would cost something the day an in-process host had to word
    /// a refusal, and that is filed rather than pre-solved.
    ///
    /// Defaulted to [`None`] so an impl that cannot move panes need not say so twice.
    fn move_pane(
        &self,
        id: PaneId,
        target: PaneId,
        dir: sprag_terminal::SplitDir,
        before: bool,
    ) -> Option<bool> {
        let _ = (id, target, dir, before);
        None
    }

    /// Deliver a file DROPPED on this client (a drag-and-drop of the LOCAL absolute `path`) to pane
    /// `id`, returning the path the pane is handed — or `None` if the delivery was refused (no such
    /// pane, or `path` resolves to nothing).
    ///
    /// The host decides what a drop MEANS, because only it knows whether the pane is local or a
    /// remote workspace: an ordinary pane is pasted the local path; a `sprag ssh` pane has the file
    /// `scp`-uploaded and is pasted the REMOTE path once it lands, so the answer for a remote pane is
    /// a path that is still in flight (see the `drop_file` action in [`crate::wire`]). Keeping the
    /// decision here rather than in the display client is what stops a second, divergent drop policy
    /// from growing in every frontend.
    ///
    /// Defaulted to `None`, like [`break_pane`](Self::break_pane) — a client that cannot spawn
    /// processes (and the test doubles) need not implement it.
    #[must_use = "the answer is the delivery's own error text, which nobody else holds"]
    fn drop_file(&self, id: PaneId, path: &str) -> Option<String> {
        let _ = (id, path);
        None
    }

    /// Every session on the host — registry-WIDE, NOT scoped to this client's own: each session's
    /// name, window count, and whether it is the registry default. The list a session-switcher
    /// sidebar draws; a client re-reads it when the scene revision moves (a new / killed session
    /// bumps it, just like a window change does the window list).
    ///
    /// **Scope note (extended, again):** a SESSION-level member of an otherwise pane-addressed
    /// trait, joining `layout` / `windows` and their writes here for the same reason (both impls
    /// and the client's one `Box<dyn HostClient>` already exist). The window+session group has now
    /// grown enough that its extraction into a dedicated mux/window/session client trait is the
    /// natural next refactor — tracked, still deliberately NOT done, so this increment stays the
    /// focused feature it set out to be.
    fn sessions(&self) -> Vec<SessionInfo>;

    /// Every session's live ACTIVITY — where it is working, on what branch, what it is serving — as
    /// fresh as this host has it, and carrying the AGE it actually has (R282).
    ///
    /// A SEPARATE call from [`sessions`](Self::sessions), and the separation is the design: that one
    /// answers the registry, which moves when this host performs an event, while this samples the
    /// operating system, which moves with nothing the host can see. Serving both from one call meant
    /// the cheapest question cost a `/proc` walk of the whole box on every poll wake — see
    /// [`sprag_terminal::ActivitySampler`].
    ///
    /// # Why this takes no tolerance, when the wire address does
    ///
    /// A tolerance is a promise to SAMPLE if the held answer is too old, and one implementer of this
    /// trait cannot keep it: a wire client's paint path must make no socket call, so it answers from
    /// a mirror its poll thread fills. A parameter one arm silently ignored would be worse than no
    /// parameter at all. So this trait asks the question every arm can answer honestly — "what have
    /// you got, and how old is it" — and the tolerance lives where the sampling decision is really
    /// made: on the wire address for a caller that can wait, and in
    /// [`SESSION_ACTIVITY_DISPLAY_MAX_AGE`](crate::wire::SESSION_ACTIVITY_DISPLAY_MAX_AGE) for the
    /// display path both arms serve.
    fn session_activity(&self) -> ActivityReading;

    /// The name of the session THIS client is currently attached to — a CLIENT-LOCAL fact. The
    /// wire carries no "attached" marker ([`sessions`](Self::sessions)'s `default` answers a
    /// DIFFERENT question — where an unscoped request lands), so a switcher reads this to highlight
    /// its own row.
    fn current_session(&self) -> String;

    /// Attach this client to the session named `name` IN PLACE — tmux `switch-client -t`: re-point
    /// every read at that session and adopt its live panes + windows + arrangement, WITHOUT
    /// relaunching the client. A no-op for the already-current session; a switch to a session that
    /// cannot be attached leaves the client on the one it was already showing (never a blank
    /// window over a failed switch).
    ///
    /// This is fundamentally a CLIENT operation (it re-points this client's projection, changing no
    /// host state), unlike the window writes above — so the in-process arm, which renders the
    /// default session directly with no re-pointable projection, implements it as a documented
    /// no-op; the wire client carries the real switch.
    fn switch_session(&self, name: &str);

    /// Move this client one step along the DAEMON's session order and answer where it LANDED —
    /// tmux `switch-client -n` / `-p` (R314). [`None`] if there was nowhere to go or the switch
    /// failed.
    ///
    /// # Why it takes a direction and answers a name
    ///
    /// [`switch_session`](Self::switch_session)'s twin for a caller that cannot name its target,
    /// and it must not be able to: the ring is walked by the daemon over the list a user SEES, so
    /// a client resolving it against its own `sessions` mirror would be a second answer derived
    /// from a poll that can be a revision behind — the authority split
    /// [`select_window_toward`](Self::select_window_toward) states one level down. The answer is
    /// the name the daemon LANDED on, which is the only way the caller learns where it went.
    ///
    /// A one-session ring answers that same session: the ring wrapped, and that is not a failure.
    #[must_use = "where this client landed is the only evidence the step worked; dropping it is \
                  the silent key R316 removed"]
    fn switch_session_toward(&self, step: OrderStep) -> Option<String>;

    /// Move this client back to the session it was viewing BEFORE this one and answer where it
    /// LANDED — tmux `switch-client -l` (R304's ask, given a driver by R314). [`None`] when this
    /// client has viewed nothing else that is still alive.
    ///
    /// The history is the DAEMON's and is keyed by session IDENTITY, which is why a client cannot
    /// answer this itself: a remembered NAME resolves to nothing after a rename and to A STRANGER
    /// once a new session takes the freed one. See [`crate::wire::AttachAsk::LastViewed`].
    #[must_use = "`None` here means there was nowhere to go back to, which no repaint can say"]
    fn switch_session_last(&self) -> Option<String>;

    /// Attach this client to the session named `name` and answer the name the daemon RECORDED —
    /// tmux `switch-client -t <session>`. [`None`] if no session carries that name, or the switch
    /// failed.
    ///
    /// [`switch_session`](Self::switch_session)'s answering form, and the one a PROMPT needs: a
    /// user who typed a name is owed either the session they asked for or the sentence saying no
    /// session is called that. The name comes back rather than being echoed for R295's rule — the
    /// daemon's recorded spelling, never the caller's argument.
    #[must_use = "`None` here means no session carries that name — the measured defect R316 was \
                  opened by, where a mistyped binding was indistinguishable from a broken build"]
    fn switch_session_named(&self, name: &str) -> Option<String>;

    /// The whole registry as a NAVIGABLE TREE — what a chooser draws its rows from (R315).
    ///
    /// [`sessions`](Self::sessions) one question wider, and a SEPARATE call for the reason
    /// [`crate::wire::TREE_SLOT`] states: the flat list is polled by every attached client and this
    /// is built when somebody presses a key.
    ///
    /// A MIRROR READ, deliberately, and it is sound for exactly [`crate::prompt`]'s rule 3: a row
    /// is text on a screen, and what a pick commits is the IDENTITY the daemon resolves again. A
    /// stale row cannot send anybody anywhere — it can only be refused.
    ///
    /// Defaulted to EMPTY rather than to the in-process host's one session, and that is not
    /// laziness: an in-process host has one session with one window, so a chooser over it would be
    /// a list with one useful row. The frontends that have one are the wire clients, which
    /// override this.
    fn tree(&self) -> Vec<sprag_terminal::TreeSession> {
        Vec::new()
    }

    /// Go where a chooser's row points — attach to its session, and select its window and pane if
    /// it named them. Answers the session name the client LANDED on (R315).
    ///
    /// [`switch_session_named`](Self::switch_session_named)'s counterpart for a pick rather than a
    /// typed name, and the difference is the whole point: [`None`] here means *that row is gone*,
    /// an answer a name cannot give because a name that something else has taken RESOLVES. See
    /// [`crate::wire::AttachAsk::Goto`].
    ///
    /// The daemon carries all three levels out under its own locks after checking the path whole,
    /// so a client never half-lands — and never resolves a level itself, which would be the second
    /// answer to `select-window` this project removes wherever it appears.
    ///
    /// Defaulted to [`None`] for the in-process arm, which has no chooser to answer.
    #[must_use = "[`None`] means THAT ROW IS GONE — the one answer a name cannot give, and the \
                  refusal the chooser paints in place"]
    fn goto(&self, target: crate::chooser::Target) -> Option<String> {
        let _ = target;
        None
    }

    /// Create a fresh session on the host (born with a shell, tmux `new-session`) and switch this
    /// client to it, returning its name. The "+" of a session sidebar.
    #[must_use = "the NAME the daemon minted, which the caller cannot guess"]
    fn new_session(&self) -> String;

    /// Kill the session named `name` (tmux `kill-session -t`) — a session sidebar row's close
    /// affordance. Killing a session OTHER than this client's own removes it and the daemon keeps
    /// serving the rest. Killing this client's OWN attached session ends the session it was serving,
    /// so the client must leave it: the tmux `detach-on-destroy` policy decides HOW — DETACH (the
    /// default) or SWITCH to a neighbouring session (`next`/`previous`), detaching only when there is
    /// no other session to move to. Killing the LAST session ends the daemon. A no-op for an unknown
    /// name.
    ///
    /// A CLIENT-adjacent SESSION op, like [`switch_session`](Self::switch_session): the in-process
    /// arm renders only the default session and owns no client to detach, so it implements this as a
    /// documented no-op; the wire client carries the real kill + detach/switch.
    ///
    /// Answers HOW FAR the kill cascaded, or [`None`] for a name the host does not hold and for the
    /// in-process no-op — [`kill_window`](Self::kill_window)'s widening one level up, and the same
    /// reason: [`Ended::Server`] means the DAEMON went with the session, which is the one outcome a
    /// client cannot discover by re-reading (there is nothing left to read).
    #[must_use = "[`Ended::Server`] means the daemon went too, which no re-read can report"]
    fn kill_session(&self, name: &str) -> Option<Ended>;

    /// Pane `id`'s child-reported window TITLE (`OSC 0` / `OSC 2`), or `None` if the
    /// child never set one (or `id` is absent).
    ///
    /// This is LIVE, CHILD-CONTROLLED state — a shell's `PROMPT_COMMAND` rewrites it on
    /// every prompt, vim sets the edited file, ssh the remote host — so it is strictly a
    /// DISPLAY name: a surface prefers it and falls back to a stable one
    /// ([`Self::pane_command_label`] / the client's own panel id). Pane IDENTITY (ids,
    /// tags, panel ids) must NEVER derive from it, because the child sets it freely.
    /// Distinct from `pane_command_label`, which names what was LAUNCHED and never
    /// changes — conflating the two would silently rewrite the a11y node name too.
    fn pane_title(&self, id: PaneId) -> Option<String>;

    /// Pane `id`'s operator-given NAME ([`sprag_terminal::Pane::name`]), or `None` if nobody named
    /// it (or `id` is absent).
    ///
    /// The OPPOSITE kind of fact from [`pane_title`](Self::pane_title), which is why they are two
    /// methods and not one. A title is chosen by the CHILD, rewritten on every prompt, and is
    /// display only. A name is chosen by a PERSON (or by the pane's opener), changes only when
    /// somebody says so, and IS identity — unique across the registry and resolvable back to this
    /// pane. So a display surface prefers this over the title, and a stable name over both.
    ///
    /// Defaulted to `None` so an older [`HostClient`] impl need not implement it.
    fn pane_name(&self, id: PaneId) -> Option<String> {
        let _ = id;
        None
    }

    /// The pane's most recent attention [`PaneNotification`] (`OSC 9` / `OSC 777;notify` /
    /// `OSC 99`), or `None` if it raised none. Like [`pane_title`](Self::pane_title) this is
    /// LIVE, CHILD-CONTROLLED display state — a display client surfaces it as "this pane wants
    /// attention" and detects a NEW one via the [`seq`](PaneNotification::seq) growing past the
    /// last it acknowledged. An absent pane and a pane that raised nothing both flatten to
    /// `None`. Defaulted to `None` so an older [`HostClient`] impl need not implement it.
    fn pane_notification(&self, _id: PaneId) -> Option<PaneNotification> {
        None
    }

    /// WHY this client's own act did not happen, taken — the answer a person's gesture earned.
    ///
    /// # Two kinds, one mailbox, and the name says which
    ///
    /// R324 built this for a SKEW alone (a daemon that cannot perform the action at all) and called
    /// it `take_skew`. R325 put the daemon's own STATED refusal in it too — *"cannot break the only
    /// pane in a window"* rather than the client's generic *"break-pane: nowhere to go"* — so the
    /// name moved with the contents. From a surface's point of view they are one fact (the gesture
    /// did not happen and there is a sentence for it); they differ in the REMEDY, which the sentence
    /// carries and the client does not have to know.
    ///
    /// The client keeps its own report as the fallback, and that is not redundancy: a refusal with
    /// no reason (a pre-PINION-PR82 daemon), a reason too long for a row, and an outcome that is not
    /// a refusal at all (`select-pane` at an edge) all land there.
    ///
    /// # Why it is not [`take_message`](crate::wake::WakeSource::take_message)
    ///
    /// That mailbox holds what the DAEMON routed to this client, and two things follow from that
    /// which are wrong here: the terminal front copies its contents OUT to the desktop notifier
    /// (R319 — *"only what the daemon routed is copied out"*), and it is drained on a WAKE. A
    /// skewed daemon performs nothing, so it bumps no channel and no wake ever comes — measured,
    /// which is how this method came to exist rather than the mailbox being reused.
    ///
    /// It is an EDGE, like a message: taken exactly once, by the path that made it. A client that
    /// read it twice would report one refusal to two keystrokes.
    ///
    /// Defaulted to [`None`] for the in-process arm, which cannot be a version behind itself.
    #[must_use = "a person's gesture that the daemon could not perform is the one thing this \
                  answers, and dropping it is the swallow it was written to end"]
    fn take_gesture_refusal(&self) -> Option<crate::report::Announcement> {
        None
    }

    /// The pane's monotonic BELL count (`\a`) — the tmux `monitor-bell` signal, `0` if it has
    /// rung none (or the pane is absent). LIVE, child-controlled, kept SEPARATE from
    /// [`pane_notification`](Self::pane_notification) (a bell carries no text) so the two
    /// attention sources stay individually addressable; a viewer's "unseen attention" combines
    /// both. Defaulted to `0` so an older [`HostClient`] impl need not implement it.
    fn pane_bell_seq(&self, _id: PaneId) -> u64 {
        0
    }

    /// What the AGENT running in pane `id` is doing ([`PaneAgent`]), or `None` for a pane no
    /// manifest claims — H3's verdict, read by a display client so its pane list can say which pane
    /// is waiting on the user.
    ///
    /// Defaulted to `None`, and here the default is the IN-PROCESS arm's real answer rather than a
    /// courtesy to an older impl. H3's D2 puts the detector daemon-side and gives its reason — three
    /// consumers computing the same fact independently is three authorities that can disagree — so
    /// the memory the verdict comes out of ([`crate::AgentRegistry`]) lives in `sprag-term`'s process
    /// and reaches a client on the pane list. An in-process [`Host`] has no such memory, and giving
    /// it one would BE the second evaluation site D2 refuses. So it says so, exactly as
    /// `SessionInfo::attached` answers `None` off a daemon rather than guessing.
    fn pane_agent(&self, _id: PaneId) -> Option<PaneAgent> {
        None
    }

    /// EVERY pane an agent claims, with its verdict, in host order — the whole answer at once.
    ///
    /// **One call rather than N+1, and that is the whole reason it exists**, on the same terms as
    /// [`pane_frame`](Self::pane_frame). A caller that read
    /// [`pane_ids`](Self::pane_ids) and then [`pane_agent`](Self::pane_agent) per id would be
    /// reading the MEMBERSHIP and the VERDICTS at different moments: for an impl that mirrors both
    /// in a cache another thread replaces, a refresh landing mid-walk pairs one generation's pane
    /// list with another's states, so a pane that went away is silently dropped from the answer and
    /// one that arrived is never asked about. Answering under one lock is what makes the result
    /// describe one moment.
    ///
    /// The default COMPOSES exactly that walk, and is correct only where the two reads cannot
    /// disagree — an in-process impl reading one authority, or a stub. **An impl whose membership
    /// and verdicts live in shared mutable state must override it**; `WireHost` does.
    fn pane_agents(&self) -> Vec<(PaneId, PaneAgent)> {
        self.pane_ids()
            .into_iter()
            .filter_map(|id| self.pane_agent(id).map(|agent| (id, agent)))
            .collect()
    }

    /// A token that changes whenever [`pane_agents`](Self::pane_agents) could answer differently —
    /// or `None` from an impl that cannot promise one.
    ///
    /// `None` is the SAFE default and the reason this is an `Option` rather than a counter every
    /// impl has to fake: it means "ask again", so an impl that says nothing costs a caller a
    /// recomputation, and only an impl that actively claims a token can make one skip. The same
    /// treatment `WirePane::projection` gives a frame's projection token in `sprag-client`, for the
    /// same reason — an absent token belongs on the unconditional path.
    ///
    /// A caller memoises whatever it derives from the pane list beside this value and recomputes
    /// when it moves. `sprag-tui` does exactly that for the window title, which it otherwise
    /// rebuilds and discards on every keystroke.
    ///
    /// CONTRACT: an impl that answers `Some` must move the token on every change a caller could
    /// observe through [`pane_agents`](Self::pane_agents) — including a pane appearing or leaving.
    /// Moving it when nothing observable changed is permitted and costs only a recomputation;
    /// FAILING to move it is a stale answer with no way to notice, so an impl that cannot promise
    /// the first should answer `None`.
    fn pane_agents_token(&self) -> Option<u64> {
        None
    }

    /// Pane `id`'s live mouse-tracking protocol level (None / Click / ButtonEvent / AnyEvent, DECSET
    /// 1000 / 1002 / 1003) — the ONE mouse-report authority fact a display client reads to decide
    /// whether to CAPTURE the pointer AND, from the level, which edges to forward (press/release,
    /// drag while a button is held, bare motion). The report ENCODING stays at the PTY boundary
    /// ([`Self::mouse`], which re-reads the authoritative live mode); this is only the client's
    /// capture gate, so a one-frame-stale value at most mis-gates one event. Defaulted to `None` so
    /// an older [`HostClient`] impl need not implement it.
    #[must_use]
    fn pane_mouse_protocol(&self, _id: PaneId) -> MouseProtocol {
        MouseProtocol::None
    }

    /// Whether pane `id`'s child has ANY tracking active — DERIVED from
    /// [`Self::pane_mouse_protocol`] (not a separate fact), the boolean the pointer oracle's
    /// capture gate reads. Not overridden: the protocol level is the single source.
    #[must_use]
    fn pane_mouse_active(&self, id: PaneId) -> bool {
        self.pane_mouse_protocol(id).is_active()
    }

    /// The pane's inline images (Kitty graphics / Sixel, R1404) as SUMMARIES — `{id, width,
    /// height, anchor, seq}`, the [`Image::rgba`](sprag_vt::Image) EMPTY over the wire. A display
    /// client reads the summary each poll, composites each over the grid at its anchor cell × the
    /// cell metric, and fetches the RGBA bytes ON DEMAND via [`Self::pane_image_rgba`] keyed on
    /// `(id, seq)` (R1404 Stage 5 — the raster is up to a MiB, so it does not ride the panes slot).
    /// Empty if the pane is absent or its child transmitted none. Defaulted to empty so an older
    /// [`HostClient`] impl need not implement it.
    fn pane_images(&self, _id: PaneId) -> Vec<Image> {
        Vec::new()
    }

    /// One inline image's RGBA bytes, fetched ON DEMAND by [`Image::id`] when a display client sees
    /// a new / changed image in [`Self::pane_images`] (R1404 Stage 5). `None` if the pane is absent
    /// or shows no image with that id. Defaulted to `None` so an older [`HostClient`] impl need not
    /// implement it.
    fn pane_image_rgba(&self, _id: PaneId, _image_id: u32) -> Option<Vec<u8>> {
        None
    }

    /// The pane's most recent OSC 52 clipboard WRITE ([`PaneClipboardWrite`]) — fetched ON
    /// DEMAND when the write `seq` in the pane list grows, NOT carried per poll, because the
    /// payload can be large (a whole paste). `None` if the pane is absent or its child has
    /// written no clipboard. A client applies it — subject to its clipboard policy — to its own
    /// system clipboard. Defaulted to `None` so an older [`HostClient`] impl need not implement it.
    fn pane_clipboard_write(&self, _id: PaneId) -> Option<PaneClipboardWrite> {
        None
    }

    /// The CHEAP monotonic count of OSC 52 clipboard WRITES this pane's child has requested (`0`
    /// before the first, or an absent pane) — no payload. A display client polls this every frame
    /// and fetches the (potentially large) payload via [`pane_clipboard_write`](Self::pane_clipboard_write)
    /// only when it grows past the last write it applied. Defaulted to `0`.
    fn pane_clipboard_write_seq(&self, _id: PaneId) -> u64 {
        0
    }

    /// The pane's pending OSC 52 clipboard READ query ([`PaneClipboardQuery`]) — the selection the
    /// child asked to read back plus its `seq` — or `None` if the pane is absent or issued no read.
    /// Tiny, so it rides the per-poll pane list (a display client answers when the `seq` grows past
    /// the last it handled). Defaulted to `None`.
    fn pane_clipboard_query(&self, _id: PaneId) -> Option<PaneClipboardQuery> {
        None
    }

    /// Answer pane `id`'s pending OSC 52 clipboard READ query `seq` by writing `text` for
    /// `target` back to its PTY as an `OSC 52` reply. Returns whether THIS call wrote: the host
    /// admits EXACTLY ONE reply per query across all attached clients (each has its own system
    /// clipboard), so a later or duplicate offer for the same `seq` returns `false` without
    /// writing. A client calls this only after its clipboard policy PERMITS the read; if every
    /// client's policy denies, the query goes unanswered (no one shares their clipboard).
    /// Defaulted to `false` so an older [`HostClient`] impl need not implement it.
    #[must_use]
    fn answer_clipboard_query(
        &self,
        _id: PaneId,
        _seq: u64,
        _target: ClipboardTarget,
        _text: &str,
    ) -> bool {
        false
    }
}

/// The owner of the session / window tree (a [`SessionRegistry`]), and the
/// **in-process** arm of the [`HostClient`] protocol. See the module docs for the
/// wire-shape + ownership notes. Constructed with one empty session / window
/// ([`new`](Host::new)) and populated with [`spawn`](Host::spawn); the headless server
/// boots its panes this way and serves them, while the GUI reaches an out-of-process
/// `Host` through a wire client.
///
/// The registry is the durable authority the detach/reattach arc rests on; `Host`
/// resolves the CURRENT window's [`Workspace`] out of it per operation, so the scene
/// assembly and the control / plugin externals keep speaking a single
/// `Arc<Mutex<Workspace>>` and never learn about the tree above them.
pub struct Host {
    registry: Arc<Mutex<SessionRegistry>>,
    /// The one place this host's SAMPLED facts are sampled and held between reads — session
    /// activity (R282) and each pane's processes (R290). See [`crate::Samplers`].
    ///
    /// They belong to the HOST rather than to the dispatch state because every arm that answers such
    /// a question reaches them from here — the wire slots, through [`crate::DaemonShared`], and this
    /// type's own [`HostClient`] arm — so one sample serves every reader and no two arms can drift
    /// about what a field means. A per-arm cache would have multiplied the `/proc` walk by the
    /// number of readers, which is the cost the split exists to remove.
    samplers: crate::Samplers,
    /// How a pane born through [`HostClient::new_pane`] is wired to its client — see
    /// [`with_pane_hooks`](Self::with_pane_hooks). `None` leaves such a pane unwired.
    pane_hooks: Option<PaneHooks>,
    /// What every pane born here tells its child about itself — see
    /// [`with_pane_env`](Self::with_pane_env). HELD as well as installed on the registry because a
    /// [`restore`](Self::restore) replaces the registry's pools wholesale and must re-install it.
    pane_env: Option<PaneEnvSource>,
    /// What every pane born here adds to its launch — see [`with_pane_args`](Self::with_pane_args).
    /// HELD for [`pane_env`](Self::pane_env)'s reason: after a [`restore`](Self::restore) an agent
    /// brought back from a snapshot would otherwise be the one agent in the daemon that cannot
    /// report.
    pane_args: Option<PaneArgsSource>,
    /// Which conversation every agent born here is in — see
    /// [`with_pane_identity`](Self::with_pane_identity). HELD for [`pane_args`](Self::pane_args)'s
    /// reason, at its sharpest: a restore is the one moment this is read for its own sake.
    pane_identity: Option<PaneIdentitySource>,
    /// Where every pane of this host lives in the machine — the daemon's delegated cgroup subtree
    /// (R336), or [`PaneHomes::none`] for a host with nothing to enforce.
    ///
    /// Installed on the registry immediately by [`with_shares`](Self::with_shares) AND held, for the
    /// reason [`pane_env`](Self::pane_env) is held: a [`restore`](Self::restore) replaces the pools
    /// wholesale and must re-install it, or a pane would be placed on every path except after a
    /// reboot.
    homes: Arc<PaneHomes>,
}

/// The `on_dirty` FACTORY a [`Host`] wires each client-created pane with: a fresh hook per pane,
/// because a `Box<dyn Fn>` cannot be reused. The same shape [`Host::restore`] takes per call — the
/// difference being that a restore's caller is present to supply one and
/// [`HostClient::new_pane`]'s is not, so this is held instead of passed.
type PaneHooks = Arc<dyn Fn() -> Option<Box<dyn Fn() + Send>> + Send + Sync>;

/// The [`HistoryLimitSource`] every registry this crate builds is seeded with: the user's
/// `history-limit`, re-read from `config.toml` at each pane's birth.
///
/// One function rather than the closure spelled at each construction site, so the daemon's boot
/// registry and a restore's replacement registry cannot come to answer differently — the asymmetry
/// R237 named, where a setting is honoured on one path and silently ignored on another.
fn history_limit_source() -> HistoryLimitSource {
    Arc::new(crate::config::history_limit_lines)
}

/// The variable naming the pane a process is running IN — tmux's `TMUX_PANE`, and the identity half
/// of what [`pane_env_source`] publishes.
///
/// A public constant because both ends of this rendezvous must name ONE fact once: the daemon
/// writes it at each pane's birth and anything reporting back reads it, the way
/// [`HOST_SOCKET`](sprag_rpc::HOST_SOCKET) already does for the address half. The value is the
/// [`PaneId`] in decimal, which is what every wire method addressing a pane takes as `id`.
///
/// Singular, and distinct from the GUI's `SPRAG_GUI_PANES` (a pane COUNT read at start-up).
pub const PANE_ENV_VAR: &str = "SPRAG_PANE";

/// **THE VARIABLES THAT TELL AN AGENT IT IS NESTED INSIDE ANOTHER ONE**, blanked at every pane's
/// birth — see [`pane_env_source`].
///
/// # ⚠⚠⚠ Why the product carries this, and what it cost that only a harness did
///
/// A daemon started from inside an agent session inherits that session's markers, and every pane it
/// opens inherits them again. The child agent then believes it is a SUB-TASK of the one that
/// launched the daemon: measured live, a `claude` opened in a sprag pane came up saying
/// `⚠ Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker`, so it wrote **no
/// transcript at all**. Nothing looked broken; the pane worked. What died silently was every reader
/// of that transcript — `sprag_plugin::spend`, which is how a loop knows what its session has been
/// charged to read, so an `ai_loop`'s `context` was `0` for ever and the one signal a restart policy
/// could ever be argued from was gone.
///
/// ⚠⚠⚠ **THIS EXACT LIST ALREADY EXISTED IN THE LIVE-AGENT HARNESS**, with a comment saying the
/// child *"must be the thing a person gets from a terminal"*. That is R379's rule for the third
/// time in this workspace: **the harness was clearing a barrier the product was not**, so every gate
/// measured a correctly-launched agent and no user ever got one. The harness now reads this.
///
/// ⚠⚠ **IT REACHES THE CASE `pane_args_source` CANNOT** — an agent a PERSON types at a shell prompt
/// inside a pane. sprag never sees that argv, so nothing can be appended to it; the ENVIRONMENT is
/// the one thing both births share.
///
/// ⚠ **BLANKED, NOT UNSET**, because `CommandBuilder` adds to the inherited environment and has no
/// unset. Every reader of these treats an empty value as absent.
///
/// ⚠⚠ **A LIST WITH NO GLOB DECIDES ALONE**, and this one is deliberately NOT a `CLAUDE_CODE_*`
/// prefix sweep: variables in that space also carry a user's own configuration (which model, which
/// endpoint, how many retries), and blanking those would break the agent this exists to make work.
/// The residue, stated: a nesting marker added by a future release is not covered until it is added
/// here, and the symptom is the one measured above.
pub const NESTED_AGENT_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_PID",
    "AI_AGENT",
];

/// The [`PaneEnvSource`] a DAEMON installs: every pane it births is told which pane it is
/// ([`PANE_ENV_VAR`]) and where the daemon serving it listens
/// (`SPRAG_HOST_RPC_SOCK`, named by [`HOST_SOCKET`](sprag_rpc::HOST_SOCKET) rather than respelled
/// here, so a client's override and this publication cannot drift apart).
///
/// `socket` is PASSED rather than resolved here, and by the caller that mounts the endpoint: a
/// process that serves no host socket must publish no address, and only the site that decides to
/// serve knows which of those it is. That is why a GUI's in-process host installs nothing and its
/// panes are spawned exactly as before.
///
/// **Publishing the address is what makes the identity usable**: a child holding only an id has
/// nothing to send it to, and a child that had to rediscover the endpoint would be a second copy of
/// [`sprag_rpc::socket_path`]'s precedence rules written in shell.
///
/// The pair is a BIRTH-TIME snapshot, as any environment is. Two consequences worth naming: a
/// process that outlives its pane keeps an id the daemon will answer as unknown (ids are never
/// reused, so it can never come to mean a DIFFERENT pane), and a pane whose child re-execs keeps
/// what it was born with.
#[must_use]
pub fn pane_env_source(socket: &std::path::Path) -> PaneEnvSource {
    // Resolved to a `String` ONCE here rather than per birth: a non-UTF-8 socket path cannot travel
    // as an env value on the wire this publishes into, and finding that out at each spawn would make
    // every pane pay for a question whose answer cannot change.
    let socket = socket.to_string_lossy().into_owned();
    let address_var = sprag_rpc::HOST_SOCKET.path_env;
    Arc::new(move |id: PaneId| {
        let mut env = vec![
            (PANE_ENV_VAR.to_owned(), id.0.to_string()),
            (address_var.to_owned(), socket.clone()),
        ];
        // ⚠⚠⚠ A PANE IS A FRESH TERMINAL, NOT A SUB-TASK OF WHOEVER STARTED THE DAEMON — see
        // [`NESTED_AGENT_MARKERS`], and the live measurement in its doc.
        env.extend(
            NESTED_AGENT_MARKERS
                .iter()
                .map(|marker| ((*marker).to_owned(), String::new())),
        );
        env
    })
}

/// The [`PaneArgsSource`] a daemon installs: an AGENT this daemon starts is launched already
/// instrumented to report its own turn boundaries, and everything else is launched untouched.
///
/// This is the other half of what [`pane_env_source`] began. That one told a pane's child where to
/// report; a child still has to be CONFIGURED to report, and until this existed the only way to
/// configure one was `sprag install-hooks`, which edits the user's own file and so instruments every
/// copy of that agent on the machine — including the ones that have nothing to do with sprag. A
/// user who never ran it got an agent supervised by SCRAPING its screen, which is a sample of an
/// animation and loses any turn that starts and ends between two samples.
///
/// The whole decision is [`crate::hooks::launch_args`], including the case that is nearly every
/// pane: a shell gets nothing added to its argv.
///
/// # What it cannot reach, stated rather than discovered
///
/// An agent a PERSON types at a shell prompt inside a pane. sprag never sees that argv — the pane's
/// child is the shell, and the agent is the shell's child — so there is nothing to append to. That
/// user's door is `install-hooks`, which is why this does not replace it, and their agent is
/// reported through [`sprag_plugin::Authority::Scraped`] so a supervisor knows which it has.
#[must_use]
pub fn pane_args_source() -> PaneArgsSource {
    // Resolved ONCE, for the reason `pane_env_source` resolves its socket once: where this binary's
    // sibling is cannot change while the daemon runs, and asking per birth would make every pane pay
    // a filesystem probe for an answer that is fixed.
    let sprag = sprag_bin();
    Arc::new(move |argv: &[String]| crate::hooks::launch_args(argv, &sprag))
}

/// The [`PaneIdentitySource`] this daemon's pools consult — which part of a LAUNCHED argv names a
/// conversation that outlives the process.
///
/// Nothing is resolved once here, unlike [`pane_args_source`]: the answer is read entirely out of the
/// argv it is shown, so there is no machine fact to cache.
#[must_use]
pub fn pane_identity_source() -> PaneIdentitySource {
    Arc::new(|argv: &[String]| crate::hooks::launched_identity(argv))
}

/// The `sprag` binary this daemon's agents report THROUGH: the sibling of the running executable,
/// else `sprag` on `PATH`.
///
/// `sprag install-hooks` writes `std::env::current_exe()` because it IS that binary. A daemon is
/// `sprag-term`, so it has to name its sibling — the same discovery `sprag` itself uses to find the
/// client binary it launches, and the same reason: a build tree works uninstalled, where `PATH`
/// alone finds nothing or finds an installed sprag of another version.
fn sprag_bin() -> std::path::PathBuf {
    sprag_beside(std::env::current_exe().ok().as_deref())
}

/// [`sprag_bin`]'s DECISION, separated from the process it reads.
///
/// Split for the reason `hooks::Target::dir_from` is split from `dir_path`: the rule and the
/// environment it consults fail differently, and `current_exe` is process-global, so a test of the
/// rule would otherwise be a test nothing can drive. The fallback in particular — an executable with
/// no sibling of that name, which is every installed layout that separates the two — is a branch no
/// developer's build tree ever takes.
fn sprag_beside(exe: Option<&std::path::Path>) -> std::path::PathBuf {
    if let Some(sibling) = exe
        .and_then(std::path::Path::parent)
        .map(|dir| dir.join("sprag"))
        && sibling.exists()
    {
        return sibling;
    }
    std::path::PathBuf::from("sprag")
}

impl Host {
    /// A new host over a registry with one empty session / window whose dimension-less
    /// spawns adopt `default_size`. Boot panes are added with [`spawn`](Self::spawn).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        let registry = SessionRegistry::new(default_size);
        registry.set_history_limit_source(history_limit_source());
        Self {
            registry: Arc::new(Mutex::new(registry)),
            samplers: crate::Samplers::default(),
            pane_hooks: None,
            pane_env: None,
            pane_args: None,
            pane_identity: None,
            homes: Arc::new(PaneHomes::none()),
        }
    }

    /// This host's [samplers](crate::Samplers), shared with whatever else serves those questions —
    /// see the field for why there is exactly one set per host.
    #[must_use]
    pub fn samplers(&self) -> &crate::Samplers {
        &self.samplers
    }

    /// Install the `on_dirty` factory every pane born through [`HostClient::new_pane`] is wired
    /// with — the in-process equivalent of the hook a caller passes [`spawn`](Self::spawn) for a
    /// boot pane.
    ///
    /// It is HELD rather than passed because the trait method that needs it takes no arguments and
    /// must not: `new_pane` is a mux operation, and how a pane wakes its display is not something a
    /// palette row (or any other caller of the client protocol) knows or should have to say. So the
    /// one place that DOES know — whoever built this host beside its display — states it once here.
    ///
    /// Without it a client-created pane is born unwired: its output still reaches its emulator, but
    /// nothing asks the display to repaint, so it appears to stall until something else does. That
    /// is the right default for a test (which polls) and the wrong one for a window, which is
    /// exactly why the window's builder calls this and a test does not.
    #[must_use]
    pub fn with_pane_hooks(
        mut self,
        on_dirty: impl Fn() -> Option<Box<dyn Fn() + Send>> + Send + Sync + 'static,
    ) -> Self {
        self.pane_hooks = Some(Arc::new(on_dirty));
        self
    }

    /// Install the [`PaneEnvSource`] every pane born under this host publishes to its child — the
    /// daemon passes [`pane_env_source`], a GUI's in-process host passes nothing.
    ///
    /// Installed on the registry immediately (so the boot pane, which is spawned before anything
    /// else, already carries it) AND held, because [`restore`](Self::restore) replaces the pools and
    /// re-installs it there. A caller therefore states this once, at construction, and every later
    /// birth path inherits it — the asymmetry `history_limit_source` names, avoided the same way.
    #[must_use]
    pub fn with_pane_env(mut self, source: PaneEnvSource) -> Self {
        lock(&self.registry).set_pane_env_source(Arc::clone(&source));
        self.pane_env = Some(source);
        self
    }

    /// Instrument every AGENT this host launches so it reports its own turn boundaries — the daemon
    /// passes [`pane_args_source`], a GUI's in-process host and a test pass nothing and launch every
    /// argv exactly as written.
    ///
    /// Installed on the REGISTRY and held, exactly like [`with_pane_env`](Self::with_pane_env) and
    /// for both of its reasons: every birth door in the daemon goes through a pool, and a restore
    /// replaces the pools.
    #[must_use]
    pub fn with_pane_args(mut self, source: PaneArgsSource) -> Self {
        lock(&self.registry).set_pane_args_source(Arc::clone(&source));
        self.pane_args = Some(source);
        self
    }

    /// Record which conversation every AGENT this host launches is in, so a restore can re-enter it
    /// rather than name a fresh one — the daemon passes [`pane_identity_source`], a GUI's in-process
    /// host and a test pass nothing and record no names.
    ///
    /// Installed and held on [`with_pane_args`](Self::with_pane_args)'s terms, and a restore
    /// re-installs it for a sharper reason than any of its siblings: a restore is the ONE moment this
    /// source is read for its own sake, so a registry that got the args source and not this one would
    /// come back instrumented and anonymous — the exact defect, arriving through the door built for
    /// it.
    #[must_use]
    pub fn with_pane_identity(mut self, source: PaneIdentitySource) -> Self {
        lock(&self.registry).set_pane_identity_source(Arc::clone(&source));
        self.pane_identity = Some(source);
        self
    }

    /// Place every pane of this host in `tree` — the daemon's adopted cgroup subtree (R336). A
    /// caller with no tree installs none and its panes open exactly as before.
    ///
    /// Installed on the REGISTRY, exactly like [`with_pane_env`](Self::with_pane_env), and R337
    /// moved it there because the alternative had been measured: consulting the tree from this type
    /// covered `Host::spawn` and left the daemon's wire, a restore, an in-process `new_pane` and a
    /// cross-window move all placing nothing. A pool is what every one of those goes through.
    #[must_use]
    pub fn with_shares(mut self, tree: Arc<Tree>) -> Self {
        // Cloned out of the `Arc` rather than shared: `PaneHomes` owns the serialisation a tree has
        // none of, so two `PaneHomes` over one tree would be two locks over one subtree. Callers
        // pass an `Arc<Tree>` because a test also wants to walk the root, and a `Tree` is a path.
        self.homes = Arc::new(
            PaneHomes::over(Tree::clone(&tree))
                // The user's ceilings, asked at each birth — `history_limit_source`'s seam exactly,
                // and installed HERE rather than by the caller for its reason: a daemon that had to
                // remember to pass it is a daemon that would place panes and cap none of them.
                .limited_by(Arc::new(crate::config::pane_limits)),
        );
        lock(&self.registry).set_pane_homes(Arc::clone(&self.homes));
        self
    }

    /// Spawn a boot pane running `command` (labelled `label`) at `cols x rows`,
    /// returning its id. `on_dirty` is the pinion-free wake hook a windowed client
    /// passes (`Some(Box::new(move || sink.request_repaint()))`, the R999
    /// `RepaintSink` seam) so this pane's output repaints the window; the headless
    /// server passes `bump_on_dirty`. `on_exit` is the "this child is gone" hook the daemon
    /// feeds to its reaper to end its own process when the last live pane dies (the headless
    /// server passes [`pane_exit_hook`](crate::pane_exit_hook); a windowed / test caller passes
    /// `None`). `on_attention` is the "this child is asking for a person" hook the daemon feeds to
    /// its [attention router](crate::attention) (the headless server passes
    /// [`pane_attention_hook`](crate::pane_attention_hook); a windowed / test caller passes `None`).
    /// All three travel as one [`PaneBirthHooks`] and all three are pinion-free, so the display,
    /// lifetime and notification concerns live above while the spawn lives here.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be started.
    pub fn spawn(
        &self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
        hooks: PaneBirthHooks,
    ) -> Result<PaneId, PanePtyError> {
        // The pane's cgroup is the POOL's business now (R337) — this door says nothing about it, and
        // neither does any other, which is the whole of what that round changed.
        lock(&self.workspace()).spawn_with_dirty(command, label, cols, rows, hooks)
    }

    /// Rebuild this host's registry from a durability [`Snapshot`] and re-spawn its panes — the
    /// restore half of the cmux-parity ring (`sprag-terminal::snapshot`).
    ///
    /// Replaces the empty boot registry with the snapshot's SHAPE (sessions, windows, layout
    /// trees, float sets, the seeded id counter), then re-spawns every recorded pane IN ITS
    /// RECORDED CWD, under its OLD id so the trees resolve, with the daemon's own `on_dirty` /
    /// `on_exit` hooks — the D4 birth-at-host seam: a restored pane must feed the reaper exactly
    /// like a boot pane. The `Arc<Mutex<SessionRegistry>>` IDENTITY is preserved (only its contents
    /// swap), so the reaper wired to it BEFORE this call stays valid.
    ///
    /// `allowlist` decides what each pane re-runs (`crate::restore_command`): a recorded
    /// NON-shell program whose basename is in it re-runs EXACTLY; a shell, a non-allowlisted
    /// program, or a cwd-less/argv-less pane restores a plain shell in the cwd. Injected (not read
    /// from the environment here) so the caller owns the policy and a test is hermetic; the daemon
    /// passes [`crate::restore_allowlist`].
    ///
    /// Hooks are FACTORIES — a fresh `Box` per pane (a `Box<dyn Fn>` cannot be reused) — so the
    /// daemon passes `|session| Some(bump_on_dirty(&channels.revision(session)))` and
    /// `|| Some(pane_exit_hook(&on_pane_exit))`, the same hooks its boot pane gets.
    ///
    /// `on_dirty` is handed the SESSION each pane is being restored into, because change
    /// notification is per session: a pane restored into `work` must announce on `work`'s token or
    /// a client waiting there sleeps through its output. Only the plan knows which session each
    /// pane belongs to, so the name is passed rather than left for the caller to guess — a factory
    /// that ignored it would silently wire every restored pane to one session's channel.
    ///
    /// `history` supplies each pane's recorded scrollback as replayable terminal bytes, replayed
    /// into its fresh emulator before the child can write a byte. Injected for the same reason the
    /// allowlist is — the host names no state directory and reads no environment — so the daemon
    /// passes a closure over [`crate::load_pane_history`] and a test passes one returning whatever
    /// it wants. Returning empty restores the pane blank, the pre-history behaviour.
    ///
    /// A pane whose shell fails to spawn (a cwd removed since the snapshot, an unexecutable
    /// `$SHELL`) is LOGGED and skipped — best-effort, the way the boot pane's own spawn failure
    /// is non-fatal; the first [`reconcile_layout`](sprag_terminal::Window) drops its now-empty
    /// leaf. Returns how many panes actually came back.
    ///
    /// # Errors
    ///
    /// [`SnapshotError`] if the snapshot itself is unusable (bad version, malformed shape, or a
    /// malformed stored layout). The registry is then LEFT AS IT WAS (the empty boot), so a
    /// corrupt snapshot degrades to a clean empty daemon, never a half-restored one.
    pub fn restore(
        &self,
        snapshot: Snapshot,
        allowlist: &std::collections::HashSet<String>,
        mut on_dirty: impl FnMut(&str) -> Option<Box<dyn Fn() + Send>>,
        mut on_exit: impl FnMut() -> Option<Box<dyn Fn() + Send>>,
        mut on_attention: impl FnMut() -> Option<Box<dyn Fn(PaneId, Attention) + Send>>,
        history: impl Fn(PaneId) -> Vec<u8>,
    ) -> Result<usize, SnapshotError> {
        // Build the new shape FIRST (fallible), so a bad snapshot leaves the boot registry intact.
        let (registry, plan) = SessionRegistry::from_snapshot(snapshot)?;
        // A restore builds a whole new set of pane pools, so the source has to be installed on them
        // too — before the loop below spawns a single restored pane, or those panes would come back
        // at the default depth while every later one honoured the user's setting.
        registry.set_history_limit_source(history_limit_source());
        // And the pane environment, for that reason at that moment: a restored pane unable to name
        // itself would be the only such pane in the daemon, and the gap would surface only after a
        // reboot.
        if let Some(source) = &self.pane_env {
            registry.set_pane_env_source(Arc::clone(source));
        }
        // And what a restored AGENT's launch carries, at that moment for that reason. A restore
        // re-derives it rather than replaying what the snapshot recorded, which is the whole reason
        // `Workspace::spawn_restored` asks the source instead of trusting the stored argv: the
        // instrumentation names a daemon, and the daemon that recorded it is gone.
        if let Some(source) = &self.pane_args {
            registry.set_pane_args_source(Arc::clone(source));
        }
        // And what a restored agent's launch is CALLED, which the loop below is about to depend on:
        // it hands each restored pane a resume of the recorded name, and this is what reads that name
        // back off the reborn launch so a SECOND restart can resume it again.
        if let Some(source) = &self.pane_identity {
            registry.set_pane_identity_source(Arc::clone(source));
        }
        // And where its panes live in the machine, for exactly that reason at exactly that moment.
        // A restored pane placed nowhere would be the one unweighted pane in the daemon, and the gap
        // would surface only after a reboot — which is where R337 found it.
        registry.set_pane_homes(Arc::clone(&self.homes));
        // Swap the CONTENTS, preserving the Arc the reaper already holds a clone of, and claim the
        // birth in the same breath. The restored registry describes sessions whose panes are still
        // being spawned one at a time below, so it reads as "nothing live" for the whole loop: the
        // first restored pane to die instantly (a shell that execs and exits) would otherwise end
        // the daemon while the rest were still coming back. The claim is taken AFTER the swap
        // because the swap replaces the whole registry, claims included. Released when `pin` falls
        // at the end of this call, whatever the loop achieved.
        //
        // It releases WITHOUT a nudge, unlike `new_session`'s, and the asymmetry is the point. A
        // create happens on a daemon that already had live panes, so the claim held off an exit
        // that was genuinely due and the release must re-ask. A restore runs at BOOT, where zero
        // panes is the daemon's legitimate resting state — a daemon with no snapshot at all boots
        // exactly like this and waits for a client. Nudging here would make a restore that brought
        // nothing back exit instead, so the client that just spawned this daemon would find it
        // gone: the very failure the claim exists to prevent, moved one step earlier.
        let pin = {
            let mut held = lock(&self.registry);
            *held = registry;
            crate::BirthPin::taken(&self.registry, &mut held, None)
        };

        let mut restored = 0usize;
        for pane in plan.panes {
            // Resolve the target window's pool (cloned Arc), then release the registry lock before
            // taking the workspace lock — the workspace-then-registry order the host keeps.
            let Some(pool) = lock(&self.registry).window_workspace(&pane.session, &pane.window)
            else {
                // The window vanished between from_snapshot and here — unreachable on one thread,
                // but the resolve is fallible, so skip rather than unwrap a should-not-happen.
                continue;
            };
            // A SANCTIONED remote workspace (its `remote` endpoint is explicit `sprag ssh` intent)
            // RECONNECTS (`ssh -t user@host`, a login shell) — the argv allowlist is bypassed
            // because the endpoint is intent, not an argv that merely mentions ssh, and the original
            // remote command is dropped so a side-effecting `-- rm -rf` never re-runs on its own.
            // Every other pane takes the exact-command-or-shell path. Env is re-derived from the
            // daemon, not disk.
            // ⚠⚠⚠ AND A RESTORED AGENT RE-ENTERS THE CONVERSATION IT WAS IN, rather than being named
            // a fresh one — `restore_command`'s `session`, which is why the recorded name travels in
            // the plan at all. Without it a pane comes back in the right directory, correctly
            // instrumented, and remembering nothing.
            //
            // ⚠⚠ The name is kept OUT of `pane.argv` on purpose, and the same reason keeps it out of
            // `Pane::argv`: that argv is what a REPLACEMENT re-runs, and a replacement must be a
            // FRESH session — `ai_loop.scxml`'s `restarting` replaces its inner session precisely to
            // throw the accumulated context away. Restoring and replacing want opposite answers, so
            // they read different fields.
            //
            // ⚠ A remote reconnect takes none: its argv is an `ssh` login, not an agent this daemon
            // named, so there is nothing a resume would answer for — said here rather than found out.
            let (command, label) = match &pane.remote {
                Some(remote) => crate::reconnect_command(remote),
                None => crate::restore_command(
                    &pane.argv,
                    pane.cwd.as_deref(),
                    allowlist,
                    pane.agent_session.as_deref(),
                ),
            };
            // Bind the spawn result so the pool lock RELEASES at the `;` — a `match` scrutinee's
            // temporary lock would live across the arms, and the `Ok` arm re-locks to mark the pane
            // remote, which on a non-reentrant `Mutex` would self-deadlock.
            let spawned = lock(&pool).spawn_restored(PaneRebirth {
                id: pane.id,
                command,
                label,
                size: (pane.cols, pane.rows),
                // The attention hook is keyed by NOTHING, unlike the wake beside it: the router
                // asks the registry which session holds the pane. They arrive UNBOUND even though a
                // restore has the id in hand, because binding is also where a pane's cgroup is
                // opened and that is the pool's to know (R337).
                hooks: PaneBirthHooks {
                    on_dirty: on_dirty(&pane.session),
                    on_exit: on_exit(),
                    on_attention: on_attention(),
                },
                history: history(pane.id),
            });
            match spawned {
                Ok(()) => {
                    // Keep the restored pane marked remote so a CHAINED restore reconnects it again.
                    if let Some(remote) = pane.remote {
                        lock(&pool).set_pane_remote(pane.id, remote);
                    }
                    // And keep its PROVENANCE, for a sharper reason than the chaining above: pane
                    // ids come back exactly, so an agent that opened panes before the reboot still
                    // names them correctly afterwards — and the agent surface's "you may close only
                    // what you opened" gate reads this fact. Dropping it here would quietly convert
                    // every agent-opened pane into one nothing can clean up.
                    if let Some(opener) = pane.opened_by {
                        lock(&pool).set_pane_opened_by(pane.id, opener);
                    }
                    // And its NAME, which is the strongest of the three: it is an ADDRESS, so a
                    // script or an agent that says `--pane build` resolves to nothing after a
                    // reboot unless the name comes back with the pane. Its registry-wide
                    // uniqueness needs no check here — `RestorePlan` refuses a snapshot carrying a
                    // name twice, exactly as it refuses one carrying an id twice.
                    if let Some(name) = pane.name {
                        lock(&pool).set_pane_name(pane.id, Some(name));
                    }
                    restored += 1;
                }
                Err(e) => tracing::warn!(
                    target: "sprag_host::durability",
                    session = %pane.session,
                    window = %pane.window,
                    id = %pane.id,
                    "restore: pane shell failed to spawn ({e}); its leaf will reconcile away",
                ),
            }
        }
        // Explicit, for the same reason `new_session`'s is: the claim's job is to outlive the loop
        // above, and only a named drop says where it ends.
        drop(pin);
        Ok(restored)
    }

    /// The DEFAULT session's current window pane pool — this arm's panes.
    ///
    /// Resolved out of the [`SessionRegistry`] on each call (a cloned `Arc`, not a borrow),
    /// so once window switching lands the next call picks up the new current window with no
    /// re-plumbing. The one place the raw `Workspace` handle escapes; the [`HostClient`]
    /// methods are how a client reaches panes.
    ///
    /// The DEFAULT session, because an in-process caller has no request to name another and
    /// this is what an unscoped one gets ([`SessionScope::unscoped`]). A caller that wants a
    /// specific session comes over the wire and names it — see
    /// [`SESSION_PARAM`](crate::wire::SESSION_PARAM).
    #[must_use]
    pub fn workspace(&self) -> Arc<Mutex<Workspace>> {
        Arc::clone(self.scope().workspace())
    }

    /// This arm's scope: the default session (see [`workspace`](Self::workspace)). Total: this
    /// in-process arm is SINGLE-session, so its registry never shrinks below one. Its only removal
    /// path is [`kill_window`](HostClient::kill_window) on the last window, which escalates to
    /// `kill_session`; but for the last session that DRAINS rather than removes, keeping
    /// `default_session` total. (The daemon's registry can shrink via a non-last `kill_session`,
    /// but that resolves scopes over the fallible wire path, not through here.)
    fn scope(&self) -> SessionScope {
        SessionScope::unscoped(&self.registry)
    }

    /// The mux state tree, for the scene-as-data assembly
    /// ([`workspace_scene`](crate::workspace_scene)) and the mux control external. Each
    /// request's [`SessionScope`] is resolved once at the door and passed to the assembly;
    /// the registry travels alongside it for the mux external's registry-WIDE operations
    /// (resolving a scoped window by name, creating a session, listing sessions) — the pane
    /// pool a request acts on comes from the scope, not from a re-resolution here. The plugin
    /// host deliberately gets [`workspace`](Self::workspace) instead — a plugin operates on a
    /// pane pool, not a session tree (Interface Segregation).
    #[must_use]
    pub fn registry(&self) -> &Arc<Mutex<SessionRegistry>> {
        &self.registry
    }

    /// Pane `id`'s cloneable I/O handle — the ONE non-wire-shaped method (module
    /// docs), so it is NOT on [`HostClient`]. It hands out a live [`PanePtyHandle`]
    /// to build the headless host's own RPC input `SpragPaneExternal`s; a display
    /// client's OWN keyboard / IME go through [`HostClient::send_key`] /
    /// [`HostClient::send_text`], NOT this handle. `None` for an absent id.
    #[must_use]
    pub fn pane_handle(&self, id: PaneId) -> Option<PanePtyHandle> {
        self.with_pane_id(id, Pane::handle)
    }

    /// Run `f` over the pane with `id` under the workspace lock — the ONE place an id
    /// resolves to a pane. `None` if no live pane has that id (closed / never existed),
    /// so every [`HostClient`] method returns its graceful default for an absent id
    /// rather than panicking (the widened identity-addressed contract).
    fn with_pane_id<R>(&self, id: PaneId, f: impl FnOnce(&Pane) -> R) -> Option<R> {
        let workspace = self.workspace();
        lock(&workspace).pane(id).map(f)
    }
}

/// The arrangement of the window `scope` names, self-healed against its live pane set, plus
/// the revision it is at — the ONE place this sequence exists (the in-process
/// [`Host::layout`], both write paths below, and the mux control external's `layout` slot all
/// call it).
///
/// It is single-sourced because its CORRECTNESS IS ITS ORDERING: the pane ids are read
/// under the WORKSPACE lock and the reconcile runs under the REGISTRY lock, taken
/// SEQUENTIALLY. Fusing them (`lock(registry)…reconcile(&lock(pool)…)`) would nest the
/// workspace lock inside the registry lock and invert the order the rest of the host
/// holds them in. A rule that load-bearing must live in one function, not in a comment
/// copied to each caller.
///
/// A pane spawning / closing between the two steps leaves the arrangement one read
/// behind; the next read heals it (the tree is not the membership authority — the
/// workspace is).
///
/// `None` if no session carries `scope`'s name. Unreachable through a wire request, whose
/// scope resolved before the scene was assembled — but the registry is the authority on
/// which sessions exist, and asking it again at the moment of use is what keeps this honest
/// once a session can be killed.
pub(crate) fn reconciled_layout(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
) -> Option<LayoutSnapshot> {
    // The pool travels with the scope, so there is no lookup to fail here and no lock to
    // take — the fallible half is the WINDOW below, which only the registry can hand out.
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    let window = registry.window_mut(scope.session(), scope.window())?;
    let tree = LayoutWire::from(window.reconcile_layout(&panes));
    let mut floating: Vec<PaneId> = window.floating().iter().copied().collect();
    floating.sort_unstable(); // a HashSet's order is arbitrary; the wire must be stable or
    // a client watching for change would see one where there is none
    Some(LayoutSnapshot {
        revision: window.layout_revision(),
        tree,
        floating,
        // Read from the SAME window under the SAME lock as the tree it filters, which is what makes
        // `LayoutSnapshot::projection` a fact rather than a join of two readings.
        zoomed: window.zoomed(),
    })
}

/// Every session a HUMAN LIST shows, in the registry's own order, with each row's viewer count
/// filled in — the ONE place [`SessionInfo::is_listable`] is applied.
///
/// The rule needs two facts held by two different owners: the pane count is the registry's and the
/// attachment count is the dispatch layer's, so the filter can only run where both are known.
/// That sentence was already on `is_listable`, and three callers were each doing the sequence by
/// hand — the wire `sessions` slot, the in-process [`HostClient::sessions`] arm, and, from R314,
/// the `switch-client` ring walk. **The order this answers is the order a user SEES**, so a step
/// along it cannot land on a session no list would show; that is the whole reason the walk shares
/// this function rather than the registry's raw `sessions()`.
///
/// `attachments` is [`None`] for an in-process host, which owns no clients: every `attached` stays
/// at the structural `0` and a session lists on its pane count alone.
///
/// # Locking
///
/// The two locks are taken SEQUENTIALLY and never nested —
/// [`SessionRegistry::session_infos_live`] releases the registry before this takes the attachment
/// map. Fusing them would nest one inside the other and pick a side in an ordering the rest of the
/// host has no need to constrain.
///
/// Its DESCENDING twin is [`listable_tree`], which applies the same rule to the same sessions.
pub(crate) fn listable_sessions(
    registry: &Arc<Mutex<SessionRegistry>>,
    attachments: Option<&Mutex<AttachmentRegistry>>,
) -> Vec<SessionInfo> {
    let mut infos = SessionRegistry::session_infos_live(registry);
    if let Some(attachments) = attachments {
        let attachments = lock(attachments);
        for info in &mut infos {
            info.attached = attachments.attached_count(&info.name);
        }
    }
    infos.retain(SessionInfo::is_listable);
    infos
}

/// The NAVIGABLE TREE a human list shows — [`listable_sessions`] one question wider (R315).
///
/// Same rule, same two owners, same sequential locks. It exists beside that function rather than
/// replacing it because the two are read at different rates: the flat list is polled by every
/// attached client, and this is built when somebody presses a key.
pub(crate) fn listable_tree(
    registry: &Arc<Mutex<SessionRegistry>>,
    attachments: Option<&Mutex<AttachmentRegistry>>,
) -> Vec<sprag_terminal::TreeSession> {
    let mut tree = SessionRegistry::tree(registry);
    if let Some(attachments) = attachments {
        let attachments = lock(attachments);
        for session in &mut tree {
            session.attached = attachments.attached_count(&session.name);
        }
    }
    // THE SAME RULE THE LISTING APPLIES, read off the same predicate rather than re-stated: a
    // chooser that offered the resting anchor would send a user somewhere `sprag ls` denies exists.
    // The counterpart of `listable_sessions`'s `retain`, and it must stay one rule — the two
    // surfaces answer the same question about the same sessions.
    tree.retain(|session| {
        SessionInfo {
            name: session.name.clone(),
            windows: session.windows.len(),
            panes: session
                .windows
                .iter()
                .map(|window| window.panes.len())
                .sum(),
            default: session.default,
            attached: session.attached,
        }
        .is_listable()
    });
    tree
}

/// Resolve a chooser's PICK against the live registry, moving NOTHING (R315).
///
/// Two acquisitions, sequential and never nested: the registry names the session and the window
/// ([`SessionRegistry::locate`]), then the window's own pool is asked whether it still holds the
/// pane. [`None`] is *that place is gone* — the answer only an IDENTITY can give, and the reason
/// this is a separate step from [`land_goto`]: a path with a dead level must refuse the ATTACH too,
/// rather than leaving a client somewhere it did not pick.
pub(crate) fn resolve_goto(
    registry: &Arc<Mutex<SessionRegistry>>,
    session: SessionId,
    window: Option<WindowId>,
    pane: Option<PaneId>,
) -> Option<Landing> {
    let located = lock(registry).locate(session, window)?;
    let window = match located.window {
        None => None,
        Some(found) => {
            // The registry lock is RELEASED by now (`locate` borrowed it for the line above and the
            // guard is dropped with the temporary), so this pool lock is sequential with it.
            if let Some(pane) = pane
                && !lock(&found.pool)
                    .panes()
                    .iter()
                    .any(|held| held.id() == pane)
            {
                return None;
            }
            Some(found.name)
        }
    };
    Some(Landing {
        session: located.session,
        window,
        pane,
    })
}

/// Carry a resolved [`Landing`]'s window and pane out — the half that MOVES something.
///
/// Called only after the attach has succeeded, so a client that never said hello cannot select a
/// window for everybody else. Both steps go through the same name-addressed verbs `sprag
/// select-window` and `sprag select-pane` reach, which is what keeps a pick from being a second
/// answer to either.
///
/// Nothing here can fail in a way a caller acts on: the path was resolved a moment ago on THIS
/// dispatch thread, which is the only thread that mutates the registry (see `handle_attach`'s own
/// statement of it). A window that went in between leaves the client attached where it picked,
/// looking at whatever that session is looking at — the honest degradation, and one no answer shape
/// could improve on.
pub(crate) fn land_goto(registry: &Arc<Mutex<SessionRegistry>>, landing: &Landing) {
    let Some(window) = landing.window.as_deref() else {
        return;
    };
    if lock(registry)
        .select_window(&landing.session, window)
        .is_err()
    {
        return;
    }
    let Some(pane) = landing.pane else {
        return;
    };
    // POOL then REGISTRY, `select_pane`'s own order one screen up: the pane list is read under the
    // workspace lock and the selection is made under the registry's, never nested.
    let panes: Vec<PaneId> = {
        let Some(pool) = lock(registry)
            .window(&landing.session, window)
            .map(|window| Arc::clone(window.workspace()))
        else {
            return;
        };
        lock(&pool).panes().iter().map(Pane::id).collect()
    };
    let mut registry = lock(registry);
    if let Some(window) = registry.window_mut(&landing.session, window) {
        window.reconcile_layout(&panes);
        window.select_pane(pane, &panes);
    }
}

/// A chooser's pick, RESOLVED — the names to act through, with nothing moved yet.
///
/// It holds names rather than the ids it was built from, because what is left to do is expressed in
/// the name-addressed verbs every other caller uses. The ids did their whole job at the door: they
/// answered *is that still the thing you were looking at*, which is the one question a name cannot.
pub(crate) struct Landing {
    /// The picked session's name NOW — never the one a client might have painted.
    pub(crate) session: String,
    /// The picked window's name now, or [`None`] for a session row.
    pub(crate) window: Option<String>,
    /// The picked pane, checked against its window's live pool.
    pub(crate) pane: Option<PaneId>,
}

/// Install a client's settled arrangement, then answer with the canonical one — the ONE
/// place a write lands (the in-process [`Host::set_layout`] and the mux `set_layout` action
/// share it).
///
/// A malformed arrangement is TRACED and dropped, and the caller is answered with the
/// layout that is actually in force. Rejecting is the honest outcome: the alternative —
/// storing a tree that violates the type's invariants — would outlive the buggy client that
/// sent it and corrupt the session for every later one.
///
/// The registry lock is released before [`reconciled_layout`] takes the workspace lock, so
/// the two are sequential here as everywhere else.
pub(crate) fn set_layout(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    tree: LayoutWire,
    expected: u64,
    expected_window: Option<&str>,
) -> Option<LayoutSnapshot> {
    // A gesture authored against a window the client has since switched away from is stale in
    // exactly the way a stale revision is (the per-window-revision bound): the scope resolves to
    // the session's CURRENT window, so if the client names the window it drew on and that is no
    // longer current, the write is REFUSED rather than mis-applied to whatever is current now.
    // The revision compare-and-set alone could pass on a cross-window revision collision; naming
    // the window is the belt to its suspenders. `None` skips the check — a caller with no window
    // to be stale against (the in-process arm, which is single-client).
    if let Some(expected_window) = expected_window
        && expected_window != scope.window()
    {
        tracing::warn!(
            target: "sprag_host",
            session = scope.session(),
            expected_window,
            current_window = scope.window(),
            "a client's arrangement targeted a window that is no longer current; keeping the one in force",
        );
        return reconciled_layout(registry, scope);
    }
    {
        // The guard is bound to a NAMED local (not left an unbound temporary) so the `?`-extracted
        // `&mut Window` borrows something that outlives the statement, and scoped in this block so it
        // DROPS before `reconciled_layout` below re-locks the same registry (a std `Mutex` is not
        // reentrant — holding it across that call would deadlock). A missing window `?`-returns `None`
        // early, exactly as the prior `None => return None` arm did, skipping `reconciled_layout`.
        let mut guard = lock(registry);
        let window = guard.window_mut(scope.session(), scope.window())?;
        if let Err(error) = window.set_layout(tree, Some(expected)) {
            tracing::warn!(
                target: "sprag_host",
                %error,
                session = scope.session(),
                "a client's arrangement was rejected; keeping the one in force",
            );
        }
    }
    reconciled_layout(registry, scope)
}

/// The panes the scoped window currently TILES, reconciled first — what a caller needs to ask
/// "is this pane there to be divided?" and get an answer about the tiling as it is rather than as
/// it was when someone last read it.
///
/// An unknown window tiles nothing, which is the honest answer for a caller that is about to
/// refuse anyway.
pub(crate) fn tiled_panes(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
) -> Vec<PaneId> {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    registry
        .window_mut(scope.session(), scope.window())
        .map_or_else(Vec::new, |window| window.reconcile_layout(&panes).panes())
}

/// The pane the scoped window is ON, reconciled first — the daemon's answer to "here".
///
/// `None` for a window that holds no panes, and for a scope naming no window. Reconciled for
/// [`tiled_panes`]' reason: a caller asking which pane is active wants the window as it IS, not as
/// it was when someone last read it — a pane that has exited must not be answered.
pub(crate) fn active_pane(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
) -> Option<PaneId> {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    let window = registry.window_mut(scope.session(), scope.window())?;
    window.reconcile_layout(&panes);
    window.active_pane()
}

/// What a select DID — the pane the window is on afterwards, and why it is that one.
///
/// A named pair rather than `(PaneId, SelectHow)`, on [`sprag_terminal::ZoomOutcome`]'s argument:
/// at a call site a tuple says nothing about which half is which, and these two are not
/// interchangeable — one is a state a caller adopts, the other a claim about how it came about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Selection {
    /// The pane the scoped window is ON after the call, which a caller adopts either way.
    pub(crate) pane: PaneId,
    /// Why it is that pane — the answer's `outcome` word.
    pub(crate) how: SelectHow,
}

/// Make a pane active in the scoped window — the ONE place the active pane moves.
///
/// Answers the [`Selection`]. `None` — refused — for a window that holds no panes, a scope naming no
/// window, a [`SelectAsk::Pane`] the window does not hold, and a
/// [`from`](SelectAsk::Toward::from) origin it does not hold either.
///
/// A [`SelectAsk::Toward`] that goes nowhere is NOT a refusal: it answers the unmoved active pane,
/// because reaching the edge of a layout is a normal outcome of a keypress rather than a caller's
/// mistake (see the action's docs). WHICH nothing it was — an edge, or an origin the tiling does
/// not hold — is [`PaneStep`]'s three-way answer, taken here because it is free at this one lock and
/// unavailable to anybody else without a second read at a second instant.
///
/// **A step that goes nowhere leaves the user on the ACTIVE pane, never on the origin.** They are
/// the same pane for a request that named no origin, and the distinction is the whole of what an
/// origin means: "one left of pane 7" says where to MEASURE from, not where to go if there is
/// nothing there. Moving the user onto pane 7 because its left edge was empty would answer a
/// question nobody asked — R299's own finding, one argument later.
///
/// The resolve and the select happen under ONE registry lock, which is what makes them atomic: a
/// caller that read the neighbour, released the lock, and then selected it could land the user on a
/// pane that had exited in between. The pool ids are read under the workspace lock first and handed
/// down, so the two locks stay sequential exactly as everywhere else in this file.
pub(crate) fn select_pane(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    ask: SelectAsk,
) -> Option<Selection> {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    let window = registry.window_mut(scope.session(), scope.window())?;
    window.reconcile_layout(&panes);
    let was = window.active_pane()?;
    // Where the request points — and, when a direction has nowhere to go, why the window stays.
    let (wanted, nowhere) = match ask {
        SelectAsk::Pane(pane) => (pane, None),
        SelectAsk::Toward { dir, from } => {
            let origin = from.unwrap_or(was);
            // Checked against the POOL, exactly as the target is one line down in
            // `Window::select_pane`: a FLOATING pane is a pane of this window and a legal origin —
            // it simply has no adjacency, which is the `Untiled` answer rather than a refusal.
            // Without this check a pane of ANOTHER window would walk off the end of `step` and be
            // reported as floating, which is a true-sounding sentence about the wrong fact.
            if !panes.contains(&origin) {
                return None;
            }
            match window.layout().step(origin, dir) {
                PaneStep::To(next) => (next, None),
                PaneStep::Edge => (was, Some(SelectHow::AtEdge)),
                PaneStep::Untiled => (was, Some(SelectHow::Untiled)),
            }
        }
    };
    // Re-asserted even when nothing moved, deliberately: `Window::select_pane` is where the zoom
    // invariant is healed, so a request that answers "nowhere" still passes through the one place
    // that keeps the window's facts consistent.
    window.select_pane(wanted, &panes).then(|| Selection {
        pane: wanted,
        how: match nowhere {
            Some(how) => how,
            None if wanted == was => SelectHow::AlreadyActive,
            None => SelectHow::Moved,
        },
    })
}

/// What a swap DID — the two panes as resolved, and why they ended up that way.
///
/// [`Selection`]'s shape one verb over, and a named record for its reason: at a call site a
/// `(PaneId, Option<PaneId>, SwapHow)` says nothing about which half is which, and these are not
/// interchangeable — two are ids a caller reports, the third is a claim about what happened.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Swap {
    /// The pane the request placed — the origin it named, or the active pane.
    pub(crate) a: PaneId,
    /// The pane it traded with. [`None`] when a direction found nobody, which is the only way a
    /// well-formed request answers without a partner.
    pub(crate) b: Option<PaneId>,
    /// What became of them — the answer's `outcome` word.
    pub(crate) how: SwapHow,
}

/// Exchange two panes' places — the ONE place the arrangement trades two leaves.
///
/// Answers the [`Swap`]. `None` — refused — for a scope whose window holds no panes, an origin no
/// window of the session holds, and a partner that is not a TILED pane of it.
///
/// A [`SwapAsk::Toward`] that finds nobody is NOT a refusal: it answers the unmoved arrangement,
/// because reaching the edge of a layout is a normal outcome of a keypress rather than a caller's
/// mistake. WHICH nothing it was — an edge, or an origin the tiling does not hold — is
/// [`PaneStep`]'s three-way answer, taken here because it is free at this one lock.
///
/// **The resolve and the trade happen under ONE registry lock**, which is what makes them atomic and
/// is a fix rather than a restatement: the wire handler used to take the lock three times (once for
/// the active pane, once for the neighbour, once for the swap), so a pane could exit between the
/// walk that chose it and the trade that moved it. That is exactly the tear
/// [`select_pane`] holds one lock to avoid, in the verb beside it.
pub(crate) fn swap_pane(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    ask: SwapAsk,
) -> Option<Swap> {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    let a = match ask.origin() {
        Some(pane) => pane,
        None => {
            let window = registry.window_mut(scope.session(), scope.window())?;
            window.reconcile_layout(&panes);
            window.active_pane()?
        }
    };
    let b = match ask {
        SwapAsk::With { with, .. } => with,
        SwapAsk::Toward { dir, .. } => {
            // `None` here is an origin NO window of this session holds — a caller's mistake, and a
            // refusal, where the two steps inside are answers. Before R301 all three were one
            // `None` and all three answered "nothing to trade with".
            match registry.step_of(scope.session(), a, dir)? {
                PaneStep::To(partner) => partner,
                PaneStep::Edge => {
                    return Some(Swap {
                        a,
                        b: None,
                        how: SwapHow::AtEdge,
                    });
                }
                PaneStep::Untiled => {
                    return Some(Swap {
                        a,
                        b: None,
                        how: SwapHow::Untiled,
                    });
                }
            }
        }
    };
    let changed = registry.swap_panes(scope.session(), a, b).ok()?;
    Some(Swap {
        a,
        b: Some(b),
        // The registry compared the two ids: it answers `false` only for a pane traded with itself,
        // which is the one outcome besides the trade that a NAMED partner can reach.
        how: if changed {
            SwapHow::Swapped
        } else {
            SwapHow::SamePane
        },
    })
}

/// What a resize DID — the pane as resolved, how far the boundary went, and why it stopped.
///
/// [`Swap`]'s shape one verb over, and a named record for its reason: `(PaneId, u16, ResizeHow)`
/// says nothing at a call site about which number is a distance and which an id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Resize {
    /// The pane the request moved a boundary of — the one it named, or the active pane.
    pub(crate) pane: PaneId,
    /// How many cells the boundary ACTUALLY travelled. Zero for every outcome but
    /// [`ResizeHow::Resized`], and BELOW what was asked when the move ran into the last cell a
    /// side may keep.
    pub(crate) cells: u16,
    /// What became of the boundary — the answer's `outcome` word.
    pub(crate) how: ResizeHow,
}

/// Move the boundary that bounds `pane` on `ask.dir`'s axis — the ONE place a split's share moves
/// by a distance rather than by a whole tree being written back.
///
/// Answers the [`Resize`], or a [`ResizeRefusal`] naming WHICH of four things stopped it: a scope
/// whose window the registry does not hold, an origin no window of the session holds, a window with
/// NO SIZE (nobody has reported an area and nothing is pinned, so a cell has no length), and an
/// arrangement this daemon could not install.
///
/// **Those four shared ONE `None` until R325**, on the reasoning that `InvokeError` carried no
/// payload so the SURFACES had to say which — which meant each surface guessed, and the CLI's guess
/// named two of the four. PINION-PR82 landed; the fact is stated at the end that observes it.
///
/// # The conversion, and why it is here
///
/// A boundary's position is a share of the region its split divides, and a caller asks in CELLS.
/// The region comes from `tile(tree, window)` — the TREE this process owns and the WINDOW this
/// process arbitrates across every attached client — so this is the only place that can do the
/// conversion at all without re-deriving one of the two from a rectangle of its own. It is the same
/// argument the daemon-side reflow makes about a pane's size, one question further in.
///
/// The share itself comes from [`Divider::stepped`](sprag_terminal::tiling::Divider::stepped),
/// which the pointer drag's [`ratio_at`](sprag_terminal::tiling::Divider::ratio_at) shares — so the
/// key and the mouse round the same way and stop in the same place.
///
/// # Locks
///
/// The pool and the attachment reports are read FIRST, each lock taken and released in turn, and
/// then the registry — the order the daemon-side reflow already takes, so this cannot invert it.
/// Why a boundary move could not be EVALUATED — [`resize_pane`]'s refusal, distinct from the four
/// [`ResizeHow`] words that say a boundary was evaluated and did not move.
///
/// It exists because the four causes were one `None` until R325, and the CLI wrote a sentence
/// naming two of them and no others. `Unmeasured` in particular is nothing like the rest: a cell
/// has no length in a window nobody is looking at, so the request cannot be judged at all — and a
/// user told *"no such pane"* about it would go hunting for a pane that is right there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ResizeRefusal {
    /// The scope names no window — a session killed under a connection already scoped to it.
    NoWindow,
    /// No pane was named and the window has no active one to default to.
    NoActivePane,
    /// NOBODY IS WATCHING that window, so it has no measured area and a cell has no size.
    Unmeasured,
    /// The moved arrangement could not be rendered or installed — a daemon-side fault, and the one
    /// arm nothing the caller sends differently would avoid.
    Unrenderable,
}

impl std::fmt::Display for ResizeRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoWindow => f.write_str("that session has no current window"),
            Self::NoActivePane => {
                f.write_str("no pane was named and that window has no active one")
            }
            Self::Unmeasured => f.write_str(
                "nothing is watching that window, so a cell has no size to move a boundary by \
                 — attach a client, or pin a size with resize-window",
            ),
            Self::Unrenderable => {
                f.write_str("this daemon could not install the moved arrangement")
            }
        }
    }
}

pub(crate) fn resize_pane(
    registry: &Arc<Mutex<SessionRegistry>>,
    attachments: &Arc<Mutex<AttachmentRegistry>>,
    scope: &SessionScope,
    ask: ResizeAsk,
) -> Result<Resize, ResizeRefusal> {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let sizes = lock(attachments).sizes_for(scope.session(), scope.window_id());
    let mut registry = lock(registry);
    let window = registry
        .window_mut(scope.session(), scope.window())
        .ok_or(ResizeRefusal::NoWindow)?;
    window.reconcile_layout(&panes);
    let pane = match ask.pane {
        Some(pane) => pane,
        None => window.active_pane().ok_or(ResizeRefusal::NoActivePane)?,
    };
    // THE VERB'S PRECONDITION, and it comes FIRST for a reason a test found: a cell has no length
    // in a window nobody has measured, so the request cannot be evaluated at all. Checking it after
    // the arrangement would make the refusal depend on the LAYOUT — a one-pane window answering
    // `at_edge` where a two-pane one refuses — which is one verb with two contracts.
    let area = crate::window::arbitrate(
        crate::config::window_size(),
        &sizes,
        window
            .manual_size()
            .map(|(cols, rows)| ClientSize { cols, rows }),
    )
    .ok_or(ResizeRefusal::Unmeasured)?;
    // Then the zoom: a zoomed window's arrangement is not what is on screen, and R285 made the
    // zoom a PROJECTION exactly so that the arrangement is untouched by it.
    if window.zoomed().is_some() {
        return Ok(Resize {
            pane,
            cells: 0,
            how: ResizeHow::Zoomed,
        });
    }
    let step = window.layout().divider_on(pane, ask.dir.axis());
    let split = match step {
        DividerStep::At(split) => split,
        DividerStep::Edge => {
            return Ok(Resize {
                pane,
                cells: 0,
                how: ResizeHow::AtEdge,
            });
        }
        DividerStep::Untiled => {
            return Ok(Resize {
                pane,
                cells: 0,
                how: ResizeHow::Untiled,
            });
        }
    };
    let tree = LayoutWire::from(window.layout());
    // `Whole`, not the snapshot's projection: the zoom is already refused above, so the two agree,
    // and naming the arrangement is what says which of them a boundary is a fact of.
    let tiling = tile(
        &Projection::Whole(&tree),
        Rect::screen(area.cols, area.rows),
    );
    // A split the tiling does not draw a divider for is one the window was too small to show, which
    // `tile` states is a pane DROPPED rather than shrunk. There is no boundary on screen to move.
    let Some((ratio, moved)) = tiling
        .dividers
        .iter()
        .find(|divider| divider.id == Some(split))
        .and_then(|divider| divider.stepped(ask.dir, ask.cells))
    else {
        return Ok(Resize {
            pane,
            cells: 0,
            how: ResizeHow::AtMinimum,
        });
    };
    if moved == 0 {
        return Ok(Resize {
            pane,
            cells: 0,
            how: ResizeHow::AtMinimum,
        });
    }
    // `None` for the expected revision: this write is the daemon's own, derived from the tree it is
    // holding the lock over, so there is no earlier read of somebody else's for it to be stale
    // against — which is precisely the case that method's `expected` documents.
    let moved_tree = with_ratio(&tree, split, ratio).ok_or(ResizeRefusal::Unrenderable)?;
    window
        .set_layout(moved_tree, None)
        .map_err(|_| ResizeRefusal::Unrenderable)?;
    Ok(Resize {
        pane,
        cells: moved,
        how: ResizeHow::Resized,
    })
}

/// What is ADJACENT to `pane` in the scoped window, in every direction — see
/// [`crate::wire::NEIGHBORS_FIELD`].
///
/// Reconciled first, so a pane that has just exited is not reported as somebody's neighbour. Each
/// direction is independently `None` at an edge, and ALL are `None` for a pane the tiling does not
/// hold (floating, gone, or another window's).
pub(crate) fn neighbors(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    pane: PaneId,
) -> [(PaneDir, Option<PaneId>); 4] {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    let mut registry = lock(registry);
    let tree = registry
        .window_mut(scope.session(), scope.window())
        .map(|window| window.reconcile_layout(&panes).clone())
        .unwrap_or_default();
    PaneDir::ALL.map(|dir| (dir, tree.neighbor(pane, dir)))
}

/// Divide `target`'s cell in the scoped window and put `pane` in the half on `side` — the ONE
/// place a directional split lands (see [`crate::wire::SPLIT_ACTION`]).
///
/// SCOPED to the current window, which is what separates it from
/// [`SessionRegistry::move_pane`](sprag_terminal::SessionRegistry::move_pane): a split places a
/// pane the caller just spawned INTO the window it is looking at, so both panes are that window's
/// by construction, while a move names a target whose window is whatever holds it.
///
/// Returns whether the target was there to divide. The pool is read under the WORKSPACE lock and
/// handed down, so the two locks stay sequential as everywhere else.
pub(crate) fn split_pane(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    pane: PaneId,
    target: PaneId,
    side: SplitSide,
    dir: SplitDir,
) -> bool {
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    lock(registry)
        .window_mut(scope.session(), scope.window())
        .is_some_and(|window| window.place_pane(pane, target, side, dir, &panes))
}

/// Take a pane out of the tiling or put it back, then answer with the resulting
/// arrangement — the ONE place a float lands (see [`HostClient::set_floating`]).
pub(crate) fn set_floating(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    id: PaneId,
    floating: bool,
) -> Option<LayoutSnapshot> {
    // The window's invariant needs the live pane set to judge "would this untile the last
    // one?", so it is read under the WORKSPACE lock and handed down — the same sequential
    // order as everywhere else, never nested.
    let panes: Vec<PaneId> = lock(scope.workspace())
        .panes()
        .iter()
        .map(Pane::id)
        .collect();
    if !lock(registry)
        .window_mut(scope.session(), scope.window())?
        .set_floating(id, floating, &panes)
    {
        tracing::debug!(
            target: "sprag_host",
            %id,
            session = scope.session(),
            "refused to float the last tiled pane; the window keeps a terminal",
        );
    }
    reconciled_layout(registry, scope)
}

/// The in-process host plays the wake's role by answering NOTHING, and both defaults are right for
/// it rather than merely convenient: it has no daemon to route a person's message from, and one
/// default session it can never lose out of band — there is no second client and no `sprag` CLI
/// reaching it.
impl crate::wake::WakeSource for Host {}

impl HostClient for Host {
    fn pane_ids(&self) -> Vec<PaneId> {
        let workspace = self.workspace();
        lock(&workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer {
        self.with_pane_id(id, |pane| crate::pane_cells(pane.pty(), offset_lines))
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }

    /// The in-process frame, read under ONE screen lock — the same atomicity the wire client gets
    /// from one borrow of its mirror, and the reason this is overridden rather than defaulted: a
    /// host that HAS the screen answering "cannot say" would leave a client attached to a local
    /// daemon unable to re-wrap a pane it can see perfectly well.
    ///
    /// The token stays `None`, which is the default's answer and still the right one: nothing here
    /// has a place to remember a projection between frames, so a caller must rebuild — see
    /// [`HostClient::pane_frame`].
    fn pane_frame(&self, id: PaneId, offset_lines: usize) -> PaneFrame {
        self.with_pane_id(id, |pane| {
            pane.pty().with_screen_palette(|screen, palette| PaneFrame {
                cells: sprag_grid::project_scrolled(screen, offset_lines, palette),
                shares: sprag_grid::shares(screen, offset_lines),
                token: None,
            })
        })
        .unwrap_or_else(|| PaneFrame {
            cells: GridBuffer::new(1, 1),
            shares: RowShares::default(),
            token: None,
        })
    }

    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts {
        self.with_pane_id(id, |pane| {
            pane.pty().with_screen(PaneScrollFacts::from_screen)
        })
        .unwrap_or(PaneScrollFacts::absent())
    }

    fn pane_prompt_positions(&self, id: PaneId) -> Vec<usize> {
        self.with_pane_id(id, |pane| {
            pane.pty().with_screen(|screen| screen.prompt_positions())
        })
        .unwrap_or_default()
    }

    /// Runs the search on the pane's own [`Screen`] — the same
    /// [`Screen::find`](sprag_vt::Screen::find) the wire `find.<needle>` family serves, so the
    /// in-process client and a wire client cannot disagree about what matches.
    fn pane_find(&self, id: PaneId, needle: &str) -> PaneFind {
        self.with_pane_id(id, |pane| {
            PaneFind::from_screen_result(&pane.pty().with_screen(|screen| screen.find(needle)))
        })
        .unwrap_or_default()
    }

    /// Runs the REGEX search on the pane's own [`Screen`] — the same
    /// [`Screen::find_regex`](sprag_vt::Screen::find_regex) the wire `regex.<pattern>` family serves,
    /// including its refusal: a pattern the engine rejects comes back as a [`PaneFind`] carrying the
    /// engine's own message, exactly as the wire answers it.
    fn pane_find_regex(&self, id: PaneId, pattern: &str) -> PaneFind {
        self.with_pane_id(id, |pane| {
            match pane.pty().with_screen(|screen| screen.find_regex(pattern)) {
                Ok(found) => PaneFind::from_screen_result(&found),
                Err(bad) => PaneFind::refused(&bad),
            }
        })
        .unwrap_or_default()
    }

    fn pane_grid_size(&self, id: PaneId) -> (u16, u16) {
        self.with_pane_id(id, |pane| pane.pty().dimensions())
            .unwrap_or((1, 1))
    }

    /// A closed / absent pane is TRACED and ignored (the swallow is honest, not
    /// silent); so is a winsize-ioctl failure.
    fn resize(&self, id: PaneId, cols: u16, rows: u16, cell_px: (u16, u16)) {
        let ws = self.workspace();
        let workspace = lock(&ws);
        if workspace.pane(id).is_none() {
            tracing::trace!(target: "sprag_host", %id, "resize of a closed/absent pane ignored");
            return;
        }
        if let Err(error) = workspace.resize(id, cols, rows, cell_px) {
            tracing::trace!(target: "sprag_host", %id, ?error, "resize winsize ioctl failed; ignored");
        }
    }

    /// Encodes to PTY bytes and writes via the shared [`crate::send_key`] SSOT (the
    /// same encoder the RPC `scene/invoke` path uses); `false` for an absent id.
    ///
    /// # ⚠⚠⚠ THIS IS THE DOOR A PERSON'S HANDS COME THROUGH
    ///
    /// A [`HostClient`] IS a display client — the TUI and the GUI reach a pane's child here and
    /// nowhere else, and what arrives has been typed on a keyboard by somebody sitting in front of
    /// it. That is why [`Hand::APerson`] is stamped here and [`Hand::AProgram`] on the wire: the
    /// encoder below is deliberately the same for both, and the hand is the only thing that
    /// distinguishes them afterwards. See [`sprag_terminal::Hand`] for what it cost not to.
    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::send_key(&handle, key, mods, Hand::APerson).is_ok())
    }

    /// Gates + encodes the mouse report at the PTY boundary via the shared [`crate::mouse`] SSOT
    /// (reading the pane's live tracking mode from the emulator); `false` for an absent id.
    fn mouse(&self, id: PaneId, event: MouseInput) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::mouse(&handle, event))
    }

    /// Gates + encodes the focus report at the PTY boundary via the shared [`crate::focus`] SSOT
    /// (reading the pane's live DEC 1004 mode from the emulator); `false` for an absent id.
    fn focus(&self, id: PaneId, focused: bool) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::focus(&handle, focused))
    }

    /// [`Hand::APerson`], for [`send_key`](Self::send_key)'s reason: this is an IME commit or typed
    /// text from a display client, which is a person composing at a keyboard.
    fn send_text(&self, id: PaneId, text: &str) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::send_text(&handle, text, Hand::APerson))
    }

    /// Brackets the paste (and filters an embedded end marker) at the PTY boundary when the pane's
    /// child enabled DEC private mode 2004 — the mode is read live from the emulator here, so the
    /// bracketing cannot disagree with what the child asked for. `false` for an absent id.
    /// [`Hand::APerson`], for [`send_key`](Self::send_key)'s reason: somebody pressed paste.
    fn paste(&self, id: PaneId, text: &str) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::paste(&handle, text, Hand::APerson))
    }

    fn pane_full_text(&self, id: PaneId) -> String {
        self.with_pane_id(id, |pane| pane.pty().with_screen(Screen::full_text))
            .unwrap_or_default()
    }

    /// Owned (`String`, not `&str`) because the workspace lock is released before it
    /// returns.
    fn pane_command_label(&self, id: PaneId) -> String {
        self.with_pane_id(id, |pane| pane.command_label().to_owned())
            .unwrap_or_default()
    }

    /// Flattens "absent pane" and "pane set no title" to the same `None` — both mean
    /// "no title to display", and a caller that must distinguish them has `pane_ids`.
    fn pane_title(&self, id: PaneId) -> Option<String> {
        self.with_pane_id(id, Pane::title).flatten()
    }

    /// The pane's operator-given name, read off the pane under the workspace lock. Flattens
    /// "absent pane" and "nobody named it" to `None`, exactly as [`Self::pane_title`] does.
    fn pane_name(&self, id: PaneId) -> Option<String> {
        self.with_pane_id(id, |pane| pane.name().map(std::string::ToString::to_string))
            .flatten()
    }

    /// The pane's live attention notification, read off the emulator under the workspace
    /// lock (like [`Self::pane_title`]) and shaped into a [`PaneNotification`]. Flattens
    /// "absent pane" and "raised nothing" to `None`.
    fn pane_notification(&self, id: PaneId) -> Option<PaneNotification> {
        self.with_pane_id(id, Pane::notification)
            .and_then(|(note, seq)| {
                note.map(|n| PaneNotification {
                    title: n.title,
                    body: n.body,
                    seq,
                })
            })
    }

    /// The pane's live BELL count, read off the emulator under the workspace lock (like
    /// [`Self::pane_notification`]). An absent pane flattens to `0`.
    fn pane_bell_seq(&self, id: PaneId) -> u64 {
        self.with_pane_id(id, Pane::bell_seq).unwrap_or(0)
    }

    /// The pane's live mouse-tracking protocol level, read off the emulator under the workspace
    /// lock (like [`Self::pane_bell_seq`]). An absent pane flattens to `None`.
    fn pane_mouse_protocol(&self, id: PaneId) -> MouseProtocol {
        self.with_pane_id(id, |pane| pane.mouse_protocol())
            .unwrap_or_default()
    }

    /// The pane's live inline images, read off the emulator under the workspace lock (like
    /// [`Self::pane_bell_seq`]). An absent pane flattens to an empty list. (In-process the RGBA is
    /// present; a display client ignores it and fetches via [`Self::pane_image_rgba`], so both the
    /// in-process and wire paths compose identically.)
    fn pane_images(&self, id: PaneId) -> Vec<Image> {
        self.with_pane_id(id, Pane::images).unwrap_or_default()
    }

    /// One inline image's live RGBA bytes by id (R1404 Stage 5 on-demand), read off the emulator
    /// under the workspace lock. `None` if the pane is absent or shows no image with that id.
    fn pane_image_rgba(&self, id: PaneId, image_id: u32) -> Option<Vec<u8>> {
        self.with_pane_id(id, |p| {
            p.images()
                .into_iter()
                .find(|im| im.id == image_id)
                .map(|im| im.rgba)
        })
        .flatten()
    }

    /// The pane's live OSC 52 clipboard WRITE, read off the emulator under the workspace lock and
    /// shaped into a [`PaneClipboardWrite`]. Flattens "absent pane" and "wrote nothing" to `None`.
    fn pane_clipboard_write(&self, id: PaneId) -> Option<PaneClipboardWrite> {
        self.with_pane_id(id, Pane::clipboard_write)
            .and_then(|(write, seq)| {
                write.map(|w| PaneClipboardWrite {
                    targets: w.targets,
                    text: w.text,
                    seq,
                })
            })
    }

    /// The pane's cheap live clipboard-write count (no payload clone), read under the workspace
    /// lock. `0` for an absent pane.
    fn pane_clipboard_write_seq(&self, id: PaneId) -> u64 {
        self.with_pane_id(id, Pane::clipboard_write_seq)
            .unwrap_or(0)
    }

    /// The pane's live pending OSC 52 read query, read off the emulator under the workspace lock
    /// and shaped into a [`PaneClipboardQuery`]. Flattens "absent pane" and "issued no read" to
    /// `None`.
    fn pane_clipboard_query(&self, id: PaneId) -> Option<PaneClipboardQuery> {
        self.with_pane_id(id, Pane::clipboard_query)
            .and_then(|(query, seq)| {
                query.map(|q| PaneClipboardQuery {
                    target: q.target,
                    seq,
                })
            })
    }

    /// Formats the `OSC 52` reply ([`osc52_reply`]) and hands it to the pane's exactly-once
    /// arbiter through the pane's handle, which is extracted from under the workspace lock and
    /// written OUTSIDE it (like [`Self::send_key`]) so the pty write does not hold the registry.
    /// `false` for an absent pane (nothing to answer).
    fn answer_clipboard_query(
        &self,
        id: PaneId,
        seq: u64,
        target: ClipboardTarget,
        text: &str,
    ) -> bool {
        let reply = osc52_reply(target, text);
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| handle.answer_clipboard_query(seq, &reply).unwrap_or(false))
    }

    /// The DEFAULT session's window (see [`Host::workspace`]).
    fn layout(&self) -> LayoutSnapshot {
        reconciled_layout(&self.registry, &self.scope()).expect(DEFAULT_ALWAYS_RESOLVES)
    }

    fn set_layout(&self, tree: LayoutWire, expected: u64) -> LayoutSnapshot {
        // No window to be stale against — the in-process arm is single-client, always current.
        set_layout(&self.registry, &self.scope(), tree, expected, None)
            .expect(DEFAULT_ALWAYS_RESOLVES)
    }

    fn set_floating(&self, id: PaneId, floating: bool) -> LayoutSnapshot {
        set_floating(&self.registry, &self.scope(), id, floating).expect(DEFAULT_ALWAYS_RESOLVES)
    }

    /// Straight to the resolve-and-select the wire action calls, so the in-process arm and a wire
    /// client walk ONE arrangement with one lock — not two implementations that agree today.
    fn select_toward(&self, dir: PaneDir) -> bool {
        select_pane(
            &self.registry,
            &self.scope(),
            SelectAsk::Toward { dir, from: None },
        )
        .is_some_and(|selection| selection.how.changed())
    }

    /// ...and the same for the SWAP, through the same one function the wire action calls.
    fn swap_toward(&self, dir: PaneDir) -> bool {
        swap_pane(
            &self.registry,
            &self.scope(),
            SwapAsk::Toward { pane: None, dir },
        )
        .is_some_and(|swap| swap.how.changed())
    }

    /// The pane THIS registry's current window is on.
    ///
    /// The trait's default answers [`None`] for "an impl with no daemon behind it", which is the
    /// wrong reading for this one: an in-process host IS the registry, so it holds the fact a wire
    /// client has to mirror. Left defaulted until R297, where a directional move made it the only
    /// way to observe where a select had LANDED — and a client that cannot read the active pane
    /// cannot follow it either.
    fn active_pane(&self) -> Option<PaneId> {
        active_pane(&self.registry, &self.scope())
    }

    /// ...and the PUBLISH half, taken in the same breath and for the same reason.
    ///
    /// The two are one fact read and written, so implementing either alone leaves this arm able to
    /// follow a pane it can never move — which is a client whose focus is a projection of something
    /// it cannot address. `false` for a pane this registry's current window does not hold, exactly
    /// as the wire client reports a refused invoke.
    fn select_pane(&self, id: PaneId) -> bool {
        select_pane(&self.registry, &self.scope(), SelectAsk::Pane(id)).is_some()
    }

    /// The DEFAULT session's windows (this arm scopes there; see [`Host::workspace`]). Total: the
    /// default always resolves.
    fn windows(&self) -> Vec<WindowInfo> {
        lock(&self.registry).default_session().window_infos()
    }

    /// Select a window of the default session, answering the name that was selected.
    ///
    /// The registry's own `Result` IS the answer now — it used to be discarded here, which is what
    /// left an unknown name looking like a success to every caller. The name comes back from the
    /// argument only after the registry has accepted it, so a [`Some`] means the select happened.
    fn select_window(&self, window: &crate::wire::WindowRef) -> Option<String> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        match window {
            crate::wire::WindowRef::Named(name) => registry
                .select_window(&session, name)
                .ok()
                .map(|()| name.clone()),
            // The identity arm has to READ the name back: its caller holds a row whose label may
            // already be stale, and the answer is what a status line paints. The wire arm resolves
            // it the same way, one process over.
            crate::wire::WindowRef::Picked(window) => registry
                .select_window_id(&session, *window)
                .ok()
                .and_then(|()| {
                    registry
                        .session(&session)
                        .map(|s| s.current_window().name().to_owned())
                }),
        }
    }

    /// The walk, straight on the registry this arm owns — the same ring the wire arm asks the daemon
    /// to walk, resolved here because this host IS the daemon for its one in-process client.
    fn select_window_toward(&self, step: OrderStep) -> Option<String> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        registry.select_window_relative(&session, step).ok()
    }

    /// The move, straight on the registry this arm owns — and the CURRENT window resolved here for
    /// the same reason the wire arm resolves it at the daemon: it is a fact about the scope, read
    /// under the one lock that also performs the move, so nothing can slip between the two.
    fn move_window(&self, window: Option<&str>, place: &WindowPlace) -> Option<(String, PlaceHow)> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        let window = match window {
            Some(window) => window.to_owned(),
            None => registry
                .session(&session)?
                .current_window()
                .name()
                .to_owned(),
        };
        let how = registry.move_window(&session, &window, place).ok()?;
        Some((window, how))
    }

    /// Create + select a window in the default session, birthing a shell into it — the in-process
    /// arm's OWN spawn path (no wake / reaper hooks; the daemon births at its `WorkspaceExternal`).
    /// `self.workspace()` is the new window's pool because `new_window` selected it.
    fn new_window(&self) -> String {
        let created = {
            let mut registry = lock(&self.registry);
            let session = registry.default_session().name().to_owned();
            // Attached, and by nobody: this is the in-process arm's "the user pressed a key",
            // which is exactly the caller `WindowBirth::default` describes.
            registry.new_window(&session, None, sprag_terminal::WindowBirth::default())
        };
        let Ok(name) = created else {
            // The default session always resolves and the allocated name is free by construction,
            // so an in-process create cannot fail — but never panic on the arm that unwraps least.
            return String::new();
        };
        let (command, label) = crate::config::default_pane_command();
        let (cols, rows) = lock(&self.workspace()).default_size();
        let _ = self.spawn(command, label, cols, rows, PaneBirthHooks::default());
        name
    }

    /// Kill a window of the default session; the last window ends the session. A window already
    /// gone is a no-op. The in-process arm has no daemon to exit, so the reaped panes just drop
    /// here — OFF the registry lock (the outcome is bound after the lock guard falls at the `;`).
    fn kill_window(&self, window: sprag_terminal::WindowId) -> Option<Ended> {
        let session = lock(&self.registry).default_session().name().to_owned();
        // Bound off the lock so the reaped panes' blocking `Drop` runs outside it — the discipline
        // every kill in this tree keeps — and its word is the answer.
        let outcome = lock(&self.registry).kill_window_id(&session, window);
        outcome.ok().map(|outcome| outcome.ended())
    }

    /// The default session's current window, pinned under the registry lock.
    ///
    /// The in-process host tracks no wire clients, so it folds NO reported areas — which makes
    /// `-a`/`-A` refuse here rather than answer, exactly as they do at the daemon for a session
    /// nobody is attached to. That is [`crate::window::arbitrate`]'s rule and not a shortcut taken
    /// here: this host has one surface and never needed arbitrating, which is the same reason
    /// [`client_areas`](crate::workspace) is empty for it.
    ///
    /// The POLICY is this process's own — it IS the process that lays the panes out — so the answer
    /// carries it rather than [`None`].
    fn resize_window(&self, size: crate::window::SizeRequest) -> Option<crate::wire::WindowPin> {
        let policy = crate::config::window_size();
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        let window = registry.session(&session)?.current_window();
        let pinned = window
            .manual_size()
            .map(|(cols, rows)| crate::attach::ClientSize { cols, rows });
        let name = window.name().to_owned();
        // No client reports, so the arbitration answers only what is PINNED — and the fallback is
        // the daemon's own: a stored rectangle is the one thing anybody has declared for this
        // window, so a relative resize can still move it.
        let current = crate::window::arbitrate(policy, &[], pinned).or(pinned);
        let resolved = size.resolve(current, &[]).ok()?;
        registry
            .resize_window(&session, &name, resolved.map(|size| (size.cols, size.rows)))
            .ok()?;
        Some(crate::wire::WindowPin {
            size: resolved,
            policy: Some(policy),
        })
    }

    /// The default session's current window, renamed under the registry lock. The three renames
    /// below are the in-process arm of the same verbs the wire client sends — each answers the
    /// RECORDED name, which is the registry's own answer rather than a re-read.
    fn rename_window(&self, name: &str) -> Option<String> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        let current = registry
            .session(&session)?
            .current_window()
            .name()
            .to_owned();
        registry.rename_window(&session, &current, name).ok()
    }

    fn rename_session(&self, name: &str) -> Option<String> {
        let mut registry = lock(&self.registry);
        let from = registry.default_session().name().to_owned();
        registry.rename_session(&from, name).ok()
    }

    /// The pane's own pool, under the workspace lock.
    ///
    /// The UNIQUENESS check the wire action makes is scoped to this arm's one session because that
    /// is all this arm has: it is single-session by construction (its `scope` is always the default
    /// session), so "every pane this host holds" and "every pane of the session" are the same set.
    /// The daemon's arm is registry-wide for the same fact reached the other way.
    fn rename_pane(&self, id: PaneId, name: &str) -> Option<String> {
        let parsed = sprag_terminal::PaneName::parse(name).ok()?;
        let workspace = self.workspace();
        let mut pool = lock(&workspace);
        if pool
            .panes()
            .iter()
            .any(|pane| pane.id() != id && pane.name() == Some(&parsed))
        {
            return None;
        }
        pool.set_pane_name(id, Some(parsed.clone()))
            .then(|| parsed.as_str().to_owned())
    }

    /// Read straight off the pane's own pty, so this arm is the authority the wire client's
    /// poll-refreshed mirror is a copy of.
    fn pane_is_dead(&self, id: PaneId) -> bool {
        self.with_pane_id(id, |pane| pane.pty().is_eof())
            .unwrap_or(false)
    }

    /// Read off the same pty as [`pane_is_dead`](Self::pane_is_dead), and separately from it: the
    /// two are published at different moments by the pane's reader thread, so reading them together
    /// here would only invent a consistency the fact itself does not have.
    fn pane_child_exit(&self, id: PaneId) -> Option<sprag_terminal::PaneExit> {
        self.with_pane_id(id, |pane| pane.pty().exit_status())
            .flatten()
    }

    /// The user's config, read from disk on every call — the host holds no registry state for it,
    /// which is why this arm and the wire client's answer the same thing with no session in sight.
    fn global_commands(&self) -> Option<Result<crate::UserConfig, String>> {
        Some(crate::config::load()?.map_err(|error| error.to_string()))
    }

    /// Spawn a shell into the default session's CURRENT window, wired with whatever
    /// [`with_pane_hooks`](Self::with_pane_hooks) installed (nothing, for a test host).
    ///
    /// The user's [`default-command`](crate::options::DEFAULT_COMMAND) then `$SHELL`, through
    /// [`default_pane_command`](crate::config::default_pane_command) — the same SSOT the `spawn` wire
    /// action's `cmd`-less default resolves, so an in-process client and a wire client are born with
    /// the same program rather than two ideas of what a pane runs.
    ///
    /// The pane adopts the workspace's default size: a client-created pane has no geometry of its
    /// own until the first reflow gives it its tile, exactly as a boot pane does not.
    fn new_pane(&self) -> Option<PaneId> {
        let (command, label) = crate::config::default_pane_command();
        let on_dirty = self.pane_hooks.as_ref().and_then(|hooks| hooks());
        let workspace = self.workspace();
        let mut workspace = lock(&workspace);
        let (cols, rows) = workspace.default_size();
        workspace
            // NO exit and NO attention hook, and both are decisions rather than omissions: this is
            // the IN-PROCESS host's own new-pane path (a windowed shell that owns its registry),
            // which does not self-exit and has no wire clients for a routed message to reach. The
            // daemon's panes are born through `WorkspaceExternal`, which wires both.
            .spawn_with_dirty(
                command,
                label,
                cols,
                rows,
                PaneBirthHooks {
                    on_dirty,
                    ..PaneBirthHooks::default()
                },
            )
            .ok()
    }

    /// Divide `target`'s cell and put a new shell in the half it opens — [`new_pane`](Self::new_pane)
    /// with a PLACE.
    ///
    /// Ordered PRE-FLIGHT, spawn, place, which is the `split` wire action's order and for its reason:
    /// a target that holds no leaf here is refused with NO child forked, because the alternative is to
    /// fork the user's shell and then have to kill it. The placement is checked again where it happens
    /// — the spawn needs the workspace lock and the placement the registry's, and this codebase never
    /// nests them, so the two cannot be one atomic step. A target that leaves the tiling in between
    /// leaves the pane APPENDED and still returns its id: killing a shell the user just asked for
    /// would be the worse answer.
    ///
    /// The trait defaults this to `None` and until now only the wire client overrode it, because only
    /// a wire client had a keystroke to serve. `sprag-gui` under `SPRAG_GUI_HOST=inprocess` now has
    /// one too (a `prefix %` off the user's keymap), and a client whose split works over a socket and
    /// silently does nothing in-process is an asymmetry no test could see past.
    fn split(&self, target: PaneId, dir: SplitDir, before: bool) -> Option<PaneId> {
        let scope = self.scope();
        if !crate::host::tiled_panes(&self.registry, &scope).contains(&target) {
            return None;
        }
        let id = self.new_pane()?;
        let side = if before {
            SplitSide::First
        } else {
            SplitSide::Second
        };
        if !crate::host::split_pane(&self.registry, &scope, id, target, side, dir) {
            tracing::warn!(
                target: "sprag_host",
                %id,
                %target,
                "the split's target left the tiling while its pane was being born; appended it",
            );
        }
        Some(id)
    }

    /// Fill the target's window with it alone, or give the arrangement back.
    ///
    /// One registry call: the window is DERIVED from the pane, so this needs no scope beyond the id
    /// — and the registry's own `zoom_pane` reconciles that window before it decides, which is what
    /// makes a freshly split pane zoomable before any client has re-read the arrangement.
    ///
    /// The scope IS consulted first, and only to refuse a pane of another session: this trait's
    /// caller is a client acting on the session it is showing, and a `PaneId` is registry-unique, so
    /// without the check a stale id from a session this client has left would zoom a window nobody
    /// here is looking at.
    fn zoom_pane(&self, target: PaneId, on: Option<bool>) -> Option<ZoomOutcome> {
        let scope = self.scope();
        let session = scope.session();
        lock(&self.registry).zoom_pane(session, target, on)
    }

    /// Remove the pane, bound so the outcome's reaped owners' blocking `Drop` (kill / wait / join
    /// the reader) runs outside the registry lock — the discipline the `close` wire action keeps
    /// for the same reason.
    ///
    /// The CASCADE is the registry's, so this arm and the wire's cannot disagree about whether a
    /// window survived: both call
    /// [`close_pane`](sprag_terminal::SessionRegistry::close_pane). What this arm does NOT do is
    /// end the process — an in-process host has no daemon to exit, so `Ended::Server` is a word it
    /// reports and does not act on, exactly as its `kill_session` arm already behaves.
    fn kill_pane(&self, id: PaneId) -> Option<Ended> {
        let scope = self.scope();
        let outcome = lock(&self.registry).close_pane(scope.session(), scope.window(), id);
        outcome.ok().map(|outcome| outcome.ended())
    }

    /// Break the pane `id` out into a new window of the default session (tmux `break-pane`). The
    /// pane is MOVED (already spawned — no pane birth here, unlike
    /// [`new_window`](Self::new_window)) and the new window selected. `None` if the move was
    /// refused.
    ///
    /// **The WINDOW's birth is [`sprag_terminal::WindowBirth::default`]**, which R335 made a decision rather than
    /// the only possibility: this trait's callers are a PERSON's key, menu row and CLI verb, and a
    /// person who breaks a pane out is asking to look at it. The agent surface is the caller that
    /// wants the other answer, and it sends the keys itself
    /// ([`BREAK_PANE_ACTION`](crate::wire::BREAK_PANE_ACTION)) rather than widening a trait every
    /// person-facing caller would then have to answer.
    fn break_pane(&self, id: PaneId, name: Option<&str>) -> Option<String> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        registry
            .break_pane(&session, id, name, sprag_terminal::WindowBirth::default())
            .ok()
    }

    /// The project governing pane `id`, read from that pane's LIVE working directory.
    ///
    /// The lock scope ENDS before the config is read, like [`drop_file`](Self::drop_file)'s: a
    /// filesystem walk under the pool lock would stall every other caller behind a slow disk. A
    /// remote pane answers `None` — its cwd is on another machine, so a local walk would describe
    /// the wrong filesystem.
    ///
    /// Renders the error here, where `.sprag.toml` is known, exactly as the wire slot does — so an
    /// in-process client and a wire client read the same sentence rather than two spellings of it.
    fn project(&self, id: PaneId) -> Option<Result<crate::Project, String>> {
        let cwd = {
            let workspace = self.workspace();
            let workspace = lock(&workspace);
            let pane = workspace.pane(id)?;
            if pane.remote().is_some() {
                return None;
            }
            pane.pty().cwd()?
        };
        Some(crate::project::load(&cwd)?.map_err(|error| error.to_string()))
    }

    /// Move the pane `id` into the window `dst` identifies, in the default session — the in-process
    /// arm of the trait method, resolving the session the way `break_pane` above does.
    fn join_pane_into(&self, id: PaneId, dst: sprag_terminal::WindowId) -> Option<bool> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        registry.join_pane_into(&session, id, dst).ok()
    }

    /// Put pane `id` beside `target` in the default session (tmux `move-pane`) — the in-process arm
    /// of the trait method, resolving the session the way its neighbour above does.
    fn move_pane(
        &self,
        id: PaneId,
        target: PaneId,
        dir: sprag_terminal::SplitDir,
        before: bool,
    ) -> Option<bool> {
        let side = if before {
            sprag_terminal::SplitSide::First
        } else {
            sprag_terminal::SplitSide::Second
        };
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        registry.move_pane(&session, id, target, side, dir).ok()
    }

    /// Resolves the pane to its PTY handle + recorded remote endpoint, then hands both to the ONE
    /// delivery policy the wire `drop_file` action also calls. The workspace guard is bound to a
    /// scope that ENDS before the delivery runs — an upload spawns a thread and a local drop writes
    /// to the PTY; neither may happen under the pool lock.
    fn drop_file(&self, id: PaneId, path: &str) -> Option<String> {
        let target = {
            let workspace = self.workspace();
            let workspace = lock(&workspace);
            let pane = workspace.pane(id)?;
            (pane.handle(), pane.remote().cloned())
        };
        crate::upload::deliver(target.0, target.1, std::path::Path::new(path))
    }

    /// Every session in the registry — the SAME registry-wide list the wire `sessions` slot serves
    /// (both built by [`SessionRegistry::session_infos_live`], the ONE builder), marking the
    /// default. Not narrowed to the default even though this arm only renders that one: the list's
    /// whole purpose is to enumerate the scopes a switcher could name.
    fn sessions(&self) -> Vec<SessionInfo> {
        // The SAME builder the wire `sessions` slot and the `switch-client` ring walk use, so no
        // two of the three can disagree about whether the resting anchor lists. An in-process host
        // owns no clients, hence the `None`: a session lists on its pane count alone here.
        listable_sessions(&self.registry, None)
    }

    /// Read this host's own sampler — the SAME one the wire `session_activity` family serves from
    /// ([`Host::samplers`]), so the two arms share one sample and cannot drift about what a field
    /// means or pay twice for the walk that produced it.
    ///
    /// Unfiltered, unlike [`sessions`](HostClient::sessions): the listability rule is about which
    /// sessions a HUMAN LIST shows, and this answers rows a caller joins onto a list it already
    /// filtered. Filtering here as well would be the same rule in two places, which is how two
    /// places come to disagree.
    ///
    /// This arm CAN sample, so it does, at the display tolerance both arms serve
    /// ([`SESSION_ACTIVITY_DISPLAY_MAX_AGE`](crate::wire::SESSION_ACTIVITY_DISPLAY_MAX_AGE)) — the
    /// same window a wire client's poll thread asks the daemon for, so a sidebar drawn over this
    /// host and one drawn over a daemon show facts of the same age.
    fn session_activity(&self) -> ActivityReading {
        self.samplers.activity.read(
            &self.registry,
            crate::wire::SESSION_ACTIVITY_DISPLAY_MAX_AGE,
        )
    }

    /// The in-process arm renders the DEFAULT session (see [`Host::workspace`]), so that is the
    /// session it is "attached" to.
    fn current_session(&self) -> String {
        lock(&self.registry).default_session().name().to_owned()
    }

    /// No-op: the in-process debug host renders the default session directly and has no
    /// re-pointable client projection to switch (see the trait method's note). Switching sessions
    /// is a wire-client capability; a test / debug in-process host stays on its default.
    fn switch_session(&self, _name: &str) {}

    /// [`None`]: this arm has one session it can render and no attachment to move, so there is no
    /// ring to walk — the same reason [`switch_session`](HostClient::switch_session) is a no-op.
    /// Answering the current session instead would be the worse lie of the two: a key would report
    /// a move that never happened.
    fn switch_session_toward(&self, _step: OrderStep) -> Option<String> {
        None
    }

    /// [`None`]: an in-process host keeps no visit history because it has no client to keep one
    /// for — the history is the DAEMON's, keyed by identity
    /// ([`crate::wire::AttachAsk::LastViewed`]).
    fn switch_session_last(&self) -> Option<String> {
        None
    }

    /// [`None`]: [`switch_session`](HostClient::switch_session)'s answering form over an arm that
    /// cannot switch, so there is never a landing to report.
    fn switch_session_named(&self, _name: &str) -> Option<String> {
        None
    }

    /// No-op returning the current (default) session: the in-process debug host cannot show a
    /// freshly-created session (it renders only the default — see
    /// [`switch_session`](HostClient::switch_session)), so creating one it could never display
    /// would be a lie. Session creation is exercised through the wire client.
    fn new_session(&self) -> String {
        self.current_session()
    }

    /// No-op: the in-process debug host renders only the default session (see
    /// [`switch_session`](HostClient::switch_session)) and owns no client to detach nor a daemon to
    /// end. Reaching the registry kill from this single-session arm would still leave
    /// `default_session` total — the registry DRAINS (never removes) the last session, exactly the
    /// `DEFAULT_ALWAYS_RESOLVES` invariant — but it would CLOSE that one session's live panes, an
    /// observable change wrong for an arm meant only to render the default. The kill action is a
    /// wire-client capability, exercised through the daemon.
    fn kill_session(&self, _name: &str) -> Option<Ended> {
        None
    }
}

/// Why the in-process arm may unwrap a scoped layout read that a wire caller must handle.
///
/// The `Option` those three return is about a NAMED session having gone; this arm names none
/// — it scopes to the default, which [`SessionRegistry::default_session`] makes total by
/// construction (`sessions` is seeded non-empty and NEVER becomes empty: `kill_session` drains
/// rather than removes the last one). The wire path never unwraps: it answers a vanished scope
/// with a refusal, because there a name really can come from a client and really can be stale.
///
/// This is a panic guarding an invariant, not a shortcut around one. The daemon increment gave
/// `sessions` a way to SHRINK (a non-last kill), but not a way to EMPTY — so this arm, which is
/// single-session and cannot reach the kill action anyway, stays total.
const DEFAULT_ALWAYS_RESOLVES: &str = "the default session resolves by construction: it is the first of a never-empty \
     session list (SessionRegistry::default_session); kill_session never removes the last one";

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// A long-lived `cat` pane (echoes stdin, keeps the PTY open across assertions).
    fn cat() -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        command
    }

    /// A temporary project holding one declared command, and its root.
    fn temp_project(infix: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("sprag-host-project-{}-{infix}", std::process::id()));
        std::fs::create_dir_all(&root).expect("temp project");
        std::fs::write(
            root.join(crate::PROJECT_FILE),
            "[[command]]\nname = \"test\"\nrun = [\"cargo\", \"test\"]\n",
        )
        .expect("write the config");
        root
    }

    /// `cat`, started IN `cwd` — so the pane's live working directory is the project's root without
    /// driving a `cd` through the shell.
    fn cat_in(cwd: &std::path::Path) -> CommandBuilder {
        let mut command = cat();
        command.cwd(cwd);
        command
    }

    /// The daemon's source publishes BOTH halves of the rendezvous, and names the address variable
    /// the same way every client reads it.
    ///
    /// The address is asserted against `HOST_SOCKET.path_env` rather than a literal on purpose: a
    /// literal here would keep passing if the policy renamed its override, which is exactly the
    /// drift that would leave a pane's child pointed at a variable nobody honours.
    #[test]
    fn the_daemon_s_pane_environment_names_the_pane_and_its_endpoint() {
        let source = pane_env_source(std::path::Path::new("/run/sprag/host.sock"));
        let published = |id: u64| -> std::collections::HashMap<String, String> {
            source(PaneId(id)).into_iter().collect()
        };

        let pane_7 = published(7);
        assert_eq!(pane_7.get(PANE_ENV_VAR).map(String::as_str), Some("7"));
        assert_eq!(
            pane_7
                .get(sprag_rpc::HOST_SOCKET.path_env)
                .map(String::as_str),
            Some("/run/sprag/host.sock"),
            "the address travels under the variable a client already overrides",
        );
        assert_eq!(
            pane_7.len(),
            2 + NESTED_AGENT_MARKERS.len(),
            "and nothing else is published: the rendezvous pair, and the nesting markers this \
             birth BLANKS — see `NESTED_AGENT_MARKERS`. ⚠ This count used to be 2 and the third \
             thing a pane is born with is not decoration: a daemon started inside an agent session \
             handed every pane that session's markers, and the agent in it then wrote no \
             transcript at all",
        );

        // The identity moves with the pane while the address does not: one source serves every pane.
        assert_eq!(
            published(8).get(PANE_ENV_VAR).map(String::as_str),
            Some("8")
        );
        assert_eq!(
            published(8).get(sprag_rpc::HOST_SOCKET.path_env),
            pane_7.get(sprag_rpc::HOST_SOCKET.path_env),
        );
    }

    /// A host with no source installed spawns panes exactly as it did before the seam existed — the
    /// GUI's in-process host, which serves no host socket, is this case.
    /// ⚠⚠⚠ **A PANE'S CHILD IS NOT A SUB-TASK OF WHOEVER STARTED THE DAEMON** — measured through a
    /// real pseudoterminal, by asking the CHILD what it inherited.
    ///
    /// # ⚠⚠⚠ The live failure this exists for
    ///
    /// A daemon started from inside an agent session exports that session's nesting markers, and a
    /// pane it opens inherits them. A real `claude` opened that way came up saying
    /// `⚠ Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker` and wrote **no
    /// transcript**. Nothing was visibly broken — the agent worked, the pane worked — and every
    /// reader of that transcript was dead: `sprag_plugin::spend` is how an `ai_loop` learns what its
    /// session has been charged to read, so `context` was `0` for the life of the run.
    ///
    /// ⚠⚠⚠ **AND THE LIVE-AGENT HARNESS HAD BEEN BLANKING THESE ALL ALONG**, which is why no gate
    /// ever saw it: the harness cleared a barrier the product did not have. This one asks the
    /// product.
    ///
    /// ⚠⚠ **THE CHILD IS THE WITNESS, NOT THE SOURCE.** `pane_env_source` returning the right pairs
    /// proves the intent; only the process on the far side of the pty proves the pairs were
    /// APPLIED. The test process sets the marker for real, so what the child reports is an
    /// inheritance that genuinely happened.
    #[test]
    fn a_pane_is_born_without_the_markers_that_would_make_its_agent_a_nested_one() {
        /// The marker whose absence a real agent named in its own words.
        const MARKER: &str = "CLAUDE_CODE_CHILD_SESSION";
        /// What the child prints when it inherited one — distinctive, so a blank screen cannot pass
        /// for a blanked variable.
        const INHERITED: &str = "INHERITED";

        assert!(
            NESTED_AGENT_MARKERS.contains(&MARKER),
            "⚠ the control: this gate is about a marker the product actually claims to blank",
        );
        // ⚠ SET FOR REAL, and this is what makes the claim an inheritance rather than an
        // arrangement: `CommandBuilder` starts from THIS process's environment, so a variable that
        // is not here cannot be inherited and the gate would pass against a host that does nothing.
        // SAFETY: single-threaded at this point in the test; the child reads it through `fork`.
        unsafe { std::env::set_var(MARKER, INHERITED) };

        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf %s \"${{{MARKER}:-blanked}}\""));
        command.env("TERM", "dumb");

        let host = Host::new((40, 6))
            .with_pane_env(pane_env_source(std::path::Path::new("/run/sprag/h.sock")));
        let id = host
            .spawn(command, "sh".to_owned(), 40, 6, PaneBirthHooks::default())
            .expect("spawn a pane");
        let said = printed_row(&host, id);
        unsafe { std::env::remove_var(MARKER) };

        assert_eq!(
            said, "blanked",
            "⚠⚠⚠ the pane's child must NOT be told it is nested inside another agent session. It \
             read {said:?}, which is what this process was started with — so an agent opened in \
             this pane believes it is a sub-task, writes no transcript, and every reader of that \
             transcript is silently answering about nothing",
        );
    }

    #[test]
    fn a_host_without_a_pane_environment_publishes_nothing() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                echo_pane_var(),
                "sh".to_owned(),
                40,
                6,
                PaneBirthHooks::default(),
            )
            .expect("spawn a pane");
        assert_eq!(printed_row(&host, id), "unset");
    }

    /// The installed source reaches a pane born through the in-process host — the path the daemon's
    /// boot pane and every `new_pane` take.
    /// The `sprag` a launched agent reports THROUGH is the one beside this binary, and a bare name only
    /// when there is none.
    ///
    /// The fallback is the branch worth driving: a build tree always has the sibling, so the case
    /// `PATH` exists for is the one no developer's machine ever takes. Read twice with the input
    /// changed — the same directory, with and without the sibling in it.
    #[test]
    fn the_agent_reports_through_the_sprag_beside_this_binary() {
        let dir = std::env::temp_dir().join(format!("sprag-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp dir");
        let exe = dir.join("sprag-term");

        assert_eq!(
            sprag_beside(Some(&exe)),
            std::path::PathBuf::from("sprag"),
            "with no sibling there is nothing to name but the one on PATH",
        );

        std::fs::write(dir.join("sprag"), "").expect("a sibling");
        assert_eq!(
            sprag_beside(Some(&exe)),
            dir.join("sprag"),
            "a build tree's own sprag, which PATH would miss or answer with another version's",
        );

        assert_eq!(sprag_beside(None), std::path::PathBuf::from("sprag"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_host_with_a_pane_environment_tells_each_pane_its_own_id() {
        let host = Host::new((40, 6))
            .with_pane_env(pane_env_source(std::path::Path::new("/run/sprag/h.sock")));
        let first = host
            .spawn(
                echo_pane_var(),
                "sh".to_owned(),
                40,
                6,
                PaneBirthHooks::default(),
            )
            .expect("spawn a pane");
        let second = host
            .spawn(
                echo_pane_var(),
                "sh".to_owned(),
                40,
                6,
                PaneBirthHooks::default(),
            )
            .expect("spawn a second pane");
        assert_eq!(printed_row(&host, first), first.0.to_string());
        assert_eq!(
            printed_row(&host, second),
            second.0.to_string(),
            "the second pane is told its own id, not the first's",
        );
    }

    /// A RESTORED pane is told too — the case the held source exists for.
    ///
    /// A restore replaces every pool in the registry, so the source installed at construction is
    /// gone from the pools that come back. REVERT-PROOF: drop the `set_pane_env_source` call from
    /// `restore` and this fails with `unset` while every other restore test stays green — a pane
    /// unable to name itself only after a reboot is precisely the asymmetry nothing else would catch.
    #[test]
    fn a_restored_pane_is_told_which_pane_it_is() {
        let live = Host::new((80, 24));
        let id = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let snap = sprag_terminal::snapshot(live.registry());

        // The restored pane re-runs a shell (`cat` is not allowlisted), so the recorded argv is not
        // what prints the variable — the RESTORE's own env publication is, which is the point.
        let restored = Host::new((80, 24))
            .with_pane_env(Arc::new(|id: PaneId| {
                vec![(PANE_ENV_VAR.to_owned(), format!("restored{}", id.0))]
            }))
            .with_pane_hooks(|| None);
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");
        assert_eq!(n, 1, "the pane came back");

        // Ask the restored shell to print it, rather than reading a screen a `cat` never writes to.
        assert!(
            restored.send_text(id, &format!("printf %s \"${PANE_ENV_VAR}\"\n")),
            "the restored pane accepts input",
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let expected = format!("restored{id}");
        while std::time::Instant::now() < deadline
            && !restored.pane_full_text(id).contains(&expected)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            restored.pane_full_text(id).contains(&expected),
            "the restored pane's screen: {:?}",
            restored.pane_full_text(id),
        );
    }

    /// A RESTORED agent is instrumented by the daemon RESTORING it, not by the one that recorded it.
    ///
    /// The twin of the test above, and the branch that only exists after a reboot: `restore` builds
    /// a whole new set of pools, so a source installed at construction is gone from every one of
    /// them. REVERT-PROOF: drop the `set_pane_args_source` call from `restore` and this fails while
    /// every other restore test stays green.
    ///
    /// It matters because `claude` is in the DEFAULT restore allowlist — an agent pane really does
    /// come back running the agent — so without this the one pane in the daemon that could not
    /// report would be the agent somebody had open when the machine rebooted.
    #[test]
    fn a_restored_agent_is_instrumented_by_the_daemon_restoring_it() {
        use sprag_terminal::{PaneSnapshot, SessionSnapshot, WindowSnapshot};

        // `echo` stands in for the agent: allowlisted here so the restore re-runs it exactly, and it
        // PRINTS what it was launched with, which is the only way to read an argv from outside.
        let allow: std::collections::HashSet<String> = ["echo".to_owned()].into_iter().collect();
        let snap = Snapshot {
            version: sprag_terminal::SNAPSHOT_VERSION,
            next_id: 1,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(),
                    floating: vec![],
                    panes: vec![PaneSnapshot {
                        id: PaneId(0),
                        cwd: None,
                        command_label: "echo".to_owned(),
                        argv: vec!["echo".to_owned(), "recorded".to_owned()],
                        agent_session: None,
                        remote: None,
                        opened_by: None,
                        name: None,
                        cols: 80,
                        rows: 24,
                    }],
                    manual_size: None,
                    active: None,
                    zoomed: None,
                    opened_by: None,
                }],
            }],
        };

        let host = Host::new((80, 24)).with_pane_args(Arc::new(|argv: &[String]| {
            if argv.first().is_some_and(|first| first.ends_with("echo")) {
                vec!["--settings".to_owned(), "DOC".to_owned()]
            } else {
                Vec::new()
            }
        }));
        assert_eq!(
            host.restore(snap, &allow, |_| None, || None, || None, |_| Vec::new())
                .expect("restores"),
            1,
        );

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let wanted = "recorded --settings DOC";
        while std::time::Instant::now() < deadline
            && !host.pane_full_text(PaneId(0)).contains(wanted)
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            host.pane_full_text(PaneId(0)).contains(wanted),
            "the restored pane's screen: {:?}, and {}",
            host.pane_full_text(PaneId(0)),
            // R351: THE FACTS AT THE DEADLINE, because an empty screen alone cannot say which of
            // two very different things happened. This test failed on the macOS runner with `""`
            // and nothing else, and the two candidates — *the child never ran* and *the child ran
            // and what it wrote was lost* — are told apart by exactly these: a child that never
            // started leaves no exit status and a pane that is not at EOF, while a child that ran
            // and was reaped leaves code 0 with an empty capture.
            pane_liveness(&host, PaneId(0)),
        );
    }

    /// What a pane can say about its own child when its screen says nothing — see the one caller.
    fn pane_liveness(host: &Host, id: PaneId) -> String {
        let workspace = host.workspace();
        let guard = lock(&workspace);
        let Some(pane) = guard.pane(id) else {
            return "there is no such pane".to_owned();
        };
        let pty = pane.pty();
        format!(
            "the child: eof={}, exit={:?}, and it wrote {} raw bytes",
            pty.is_eof(),
            pty.exit_status(),
            pty.raw_output().bytes.len(),
        )
    }

    /// A pane's command that prints [`PANE_ENV_VAR`] and exits, so a test can read what the CHILD
    /// received rather than what the builder was handed.
    fn echo_pane_var() -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf %s \"${{{PANE_ENV_VAR}-unset}}\""));
        command.env("TERM", "dumb");
        command
    }

    /// Row 0 of `id`'s screen once its child has exited.
    fn printed_row(host: &Host, id: PaneId) -> String {
        let workspace = host.workspace();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            let eof = lock(&workspace)
                .pane(id)
                .is_some_and(|pane| pane.pty().is_eof());
            if eof {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let held = lock(&workspace);
        let pane = held.pane(id).expect("the pane is still in the pool");
        pane.pty()
            .with_screen(|screen| {
                (0..screen.cols())
                    .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.to_string()))
                    .collect::<String>()
            })
            .trim_end()
            .to_owned()
    }

    /// The in-process arm reads a pane's project from that pane's OWN live cwd.
    ///
    /// REVERT-PROOF: point `Host::project` at the daemon's cwd instead of the pane's and this fails
    /// (the test process does not run inside the temporary project).
    #[test]
    fn a_panes_project_is_read_from_that_panes_working_directory() {
        let root = temp_project("local");
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                cat_in(&root),
                "cat".to_owned(),
                40,
                6,
                PaneBirthHooks::default(),
            )
            .expect("spawn a pane inside the project");

        let project = host
            .project(id)
            .expect("the pane sits in a project")
            .expect("its config parses");
        // CANONICALISED on both sides. The root here came back through the pane's own cwd, which
        // the OS reports RESOLVED — and macOS's `TMPDIR` is `/var/folders/…`, a symlink to
        // `/private/var/folders/…`. Comparing the path this test HANDED OVER against the one the
        // kernel HANDED BACK is comparing two spellings of one directory, and it fails on the
        // platform where they differ. The same rule `a_spawn_opens_in_the_directory_it_was_given`
        // already follows.
        assert_eq!(
            project.root.canonicalize().ok(),
            root.canonicalize().ok(),
            "the project root is the directory the pane is working in",
        );
        assert_eq!(project.actions[0].run, vec!["cargo", "test"]);
        std::fs::remove_dir_all(&root).ok();
    }

    /// A REMOTE workspace has no LOCAL project, even when its pane's recorded cwd would resolve to
    /// one on this machine: the shell is on another host, so offering the local repository's
    /// commands would run them in the wrong place.
    ///
    /// REVERT-PROOF: drop the `remote().is_some()` guard from `Host::project` and this fails,
    /// because the surrounding directory DOES hold a config.
    #[test]
    fn a_remote_pane_has_no_local_project() {
        let root = temp_project("remote");
        let host = Host::new((40, 6));
        let id = host
            .spawn(
                cat_in(&root),
                "cat".to_owned(),
                40,
                6,
                PaneBirthHooks::default(),
            )
            .expect("spawn a pane inside the project");
        // Mark it the way `sprag ssh` does once its birth pane exists.
        lock(&host.workspace()).set_pane_remote(
            id,
            sprag_terminal::SshRemote {
                user: None,
                host: "elsewhere".to_owned(),
                port: None,
            },
        );

        assert!(
            host.project(id).is_none(),
            "a remote pane's cwd is on another machine, so no local project describes it"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The in-process `Host::sessions()` applies the SAME listability filter the wire `sessions`
    /// slot does (the SSOT rule), so the two arms cannot disagree on the resting anchor: a fresh
    /// host's empty anchor is hidden, and a session that holds a pane lists.
    #[test]
    fn sessions_hides_the_empty_anchor_and_lists_a_worked_session() {
        let host = Host::new((40, 6));
        assert!(
            host.sessions().is_empty(),
            "a fresh host's empty anchor holds no pane and is not listed",
        );
        host.spawn(cat(), "cat".to_owned(), 40, 6, PaneBirthHooks::default())
            .unwrap();
        let names: Vec<String> = host.sessions().into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            vec!["0".to_owned()],
            "a session holding a pane lists"
        );
    }

    #[test]
    fn spawn_grows_the_pane_set_and_exposes_geometry() {
        let host = Host::new((40, 6));
        assert!(host.pane_ids().is_empty());
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, PaneBirthHooks::default())
            .unwrap();
        assert_eq!(host.pane_ids(), vec![id]);
        assert_eq!(host.pane_grid_size(id), (40, 6));
    }

    /// The client-protocol pane pair: `new_pane` grows the CURRENT window's set with a shell, and
    /// `kill_pane` removes exactly the pane named — answering `None` for one that is not there, and
    /// saying how far the kill CASCADED for one that is.
    ///
    /// **The last assertion is the one that changed at R309, and it changed from the opposite
    /// claim.** It used to read *"leaving an empty window rather than refusing"* — this test PINNED
    /// the window surviving its own last pane. It does not survive it now: this host holds one
    /// session of one window, so closing that pane walks the whole chain in one call and the answer
    /// says so.
    ///
    /// REVERT-PROOF: have `new_pane` return `None` without spawning and the set never grows; have
    /// `kill_pane` ignore its answer and the absent-id assertion fails, which is the difference
    /// between "I closed it" and "there was nothing to close"; drop the escalation from
    /// `SessionRegistry::close_pane` and the last pane answers `Ended::Pane`.
    #[test]
    fn the_client_protocol_creates_and_closes_panes_in_the_current_window() {
        let host = Host::new((40, 6));
        assert!(host.pane_ids().is_empty());

        let first = host.new_pane().expect("a shell is born");
        let second = host.new_pane().expect("and a second");
        assert_eq!(
            host.pane_ids(),
            vec![first, second],
            "both join the current window's pane set, in birth order"
        );

        assert_eq!(
            host.kill_pane(first),
            Some(Ended::Pane),
            "the named pane is removed, and it took nothing else with it"
        );
        assert_eq!(host.pane_ids(), vec![second], "and only that one");
        assert_eq!(
            host.kill_pane(first),
            None,
            "closing it again reports that there was nothing to close"
        );
        assert_eq!(
            host.kill_pane(second),
            Some(Ended::Server),
            "the window's LAST pane takes the window, its session, and — this host holding only \
             that one — the server: one call, the whole chain, said in one word"
        );
        assert!(
            host.pane_ids().is_empty(),
            "and nothing is left tiled behind it"
        );
    }

    /// The trait's zoom fills a window with one pane, gives it back, and REFUSES a pane this
    /// session does not hold — the three outcomes a client's binding meets.
    ///
    /// The tri-state is asserted as a tri-state rather than as a toggle used twice: `on` absent has
    /// to flip whatever is in force, and `Some(false)` on an unzoomed window has to be the
    /// distinguishable "arrangement already showing" rather than a second toggle. Those two are the
    /// pair a caller cannot tell apart if `changed` is dropped.
    ///
    /// **REVERT-PROOF: return `Some(ZoomOutcome{..})` for an unknown pane and the last assertion
    /// fails** — a client would report a zoom that never happened, and its own layout mirror would
    /// then be re-read for nothing.
    #[test]
    fn the_trait_zoom_fills_a_window_gives_it_back_and_refuses_a_foreign_pane() {
        let host = Host::new((40, 6));
        let first = host.new_pane().expect("a shell is born");
        let second = host
            .split(first, SplitDir::Vertical, false)
            .expect("splits");

        let filled = host.zoom_pane(second, None).expect("a tiled pane zooms");
        assert_eq!(
            (filled.zoomed, filled.changed),
            (true, true),
            "the toggle filled the window with the pane it named",
        );
        assert_eq!(
            host.layout().zoomed,
            Some(second),
            "and the arrangement NAMES that pane, which is what every client projects through",
        );

        assert_eq!(
            host.zoom_pane(second, Some(true)).map(|o| o.changed),
            Some(false),
            "re-asserting the state in force moves nothing, and says so",
        );
        assert_eq!(
            host.zoom_pane(second, None).map(|o| (o.zoomed, o.changed)),
            Some((false, true)),
            "the same toggle gives the arrangement back",
        );
        assert_eq!(
            host.zoom_pane(second, Some(false))
                .map(|o| (o.zoomed, o.changed)),
            Some((false, false)),
            "...and asking for that again is the FOURTH case, not a second toggle",
        );
        assert_eq!(host.layout().zoomed, None);

        assert_eq!(
            host.zoom_pane(PaneId(999), None),
            None,
            "a pane this session does not hold is refused, not toggled",
        );
    }

    /// `split` is `new_pane` with a PLACE: the new shell lands where the direction asked, and a target
    /// that holds no leaf here is refused with NO child forked.
    ///
    /// The refusal is what the ordering is for. **REVERT-PROOF: drop the `tiled_panes` pre-flight and
    /// the last assertion fails** — an unknown target forks the user's shell, appends it because there
    /// was nothing to divide, and answers with an id, which is the outcome the wire action's own
    /// pre-flight exists to avoid.
    #[test]
    fn split_places_a_shell_beside_its_target_and_refuses_an_unknown_one() {
        let host = Host::new((40, 6));
        let first = host.new_pane().expect("a shell is born");

        let below = host
            .split(first, SplitDir::Vertical, false)
            .expect("the target was there to divide");
        assert_eq!(host.pane_ids(), vec![first, below]);
        assert!(
            matches!(
                host.layout().tree.root,
                Some(sprag_terminal::LayoutNodeWire::Split {
                    dir: SplitDir::Vertical,
                    ..
                })
            ),
            "the arrangement is the stack the direction named: {:?}",
            host.layout().tree,
        );

        // tmux's `-b`: the near side instead of the far one, which is a different tree and not a
        // different pane count — so the pair is asserted rather than just the growth.
        let above = host
            .split(below, SplitDir::Horizontal, true)
            .expect("splits again");
        assert_eq!(host.pane_ids(), vec![first, below, above]);

        assert_eq!(
            host.split(PaneId(999), SplitDir::Horizontal, false),
            None,
            "an unknown target is refused",
        );
        assert_eq!(
            host.pane_ids(),
            vec![first, below, above],
            "...with no child forked, which is the point of checking first",
        );
    }

    /// A pane born through the client protocol is wired with the hooks the host was BUILT with —
    /// the seam that keeps a client-created pane as live as a boot pane, since `new_pane` takes no
    /// arguments and so has nowhere to be handed one.
    ///
    /// Asserts the factory was CONSULTED (a pane cannot be asked whether it holds a hook), which is
    /// the whole of what this seam owes: `spawn_with_dirty` is what installs it, and its own tests
    /// cover the firing.
    ///
    /// REVERT-PROOF: pass `None` instead of `self.pane_hooks` in `new_pane` and the count stays 0.
    #[test]
    fn a_client_created_pane_is_wired_with_the_hosts_own_pane_hooks() {
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&asked);
        let host = Host::new((40, 6)).with_pane_hooks(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            None
        });

        assert_eq!(
            asked.load(Ordering::SeqCst),
            0,
            "nothing asked before a spawn"
        );
        host.new_pane().expect("a shell is born");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            1,
            "the factory is consulted once, per pane"
        );
        host.new_pane().expect("and again for the next");
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    /// A host built WITHOUT hooks still creates panes — the test / headless default, and the reason
    /// the field is an `Option` rather than a required constructor argument.
    #[test]
    fn a_host_with_no_pane_hooks_still_creates_panes() {
        let host = Host::new((40, 6));
        assert!(host.new_pane().is_some());
        assert_eq!(host.pane_ids().len(), 1);
    }

    /// A pane whose child EXITS stays in the window, showing what it printed — the property a
    /// run-in-a-new-pane target needs, asserted because the whole feature rests on it and nothing
    /// else pins it.
    #[test]
    fn a_pane_whose_child_exits_keeps_its_place_and_its_output() {
        let host = Host::new((40, 6));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("echo done-and-gone");
        command.env("TERM", "dumb");
        let id = host
            .spawn(command, "sh".to_owned(), 40, 6, PaneBirthHooks::default())
            .expect("spawn a short-lived child");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !lock(&host.workspace())
            .pane(id)
            .is_some_and(|pane| pane.pty().is_eof())
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the child never exited"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        assert_eq!(
            host.pane_ids(),
            vec![id],
            "the pane is still a member after its child is gone"
        );
        assert!(
            host.pane_full_text(id).contains("done-and-gone"),
            "and still holds what the child printed: {:?}",
            host.pane_full_text(id)
        );
        // ...and SAYS it is dead, which is the only way a client can tell this apart from a pane
        // whose child is merely quiet. REVERT-PROOF: return `false` unconditionally from
        // `pane_is_dead` and this fails.
        assert!(host.pane_is_dead(id), "the client protocol reports it dead");

        let live = host
            .spawn(cat(), "cat".to_owned(), 40, 6, PaneBirthHooks::default())
            .expect("spawn a long-lived child");
        assert!(!host.pane_is_dead(live), "a running child is not dead");
        assert!(
            !host.pane_is_dead(PaneId(999)),
            "and neither is a pane that was never there"
        );
    }

    /// The in-process arm serves the USER's config — the same three answers the wire client gets,
    /// over the same loader, so the two clients cannot disagree about what the user declared.
    ///
    /// REVERT-PROOF: return `None` unconditionally and both the declared and the broken cases fail.
    #[test]
    fn the_client_protocol_serves_the_users_own_commands() {
        let host = Host::new((40, 6));

        crate::config::with_config(None, || {
            assert!(
                host.global_commands().is_none(),
                "no config written is not an error"
            );
        });

        crate::config::with_config(
            Some("[[command]]\nname = \"top\"\nrun = [\"htop\"]\n"),
            || {
                let config = host
                    .global_commands()
                    .expect("a config exists")
                    .expect("it parses");
                assert_eq!(config.commands.len(), 1);
                assert_eq!(config.commands[0].name, "top");
            },
        );

        crate::config::with_config(Some("[[command]]\nname = \"a\"\nrun = []\n"), || {
            let report = host
                .global_commands()
                .expect("the file exists")
                .expect_err("and is refused");
            assert!(
                report.contains(crate::config::CONFIG_FILE),
                "the report reaches the client ALREADY naming its own file: {report:?}"
            );
        });
    }

    #[test]
    fn resize_updates_the_grid_geometry() {
        let host = Host::new((40, 6));
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, PaneBirthHooks::default())
            .unwrap();
        host.resize(id, 100, 30, (0, 0));
        assert_eq!(host.pane_grid_size(id), (100, 30));
    }

    /// The OTHER half of the reboot payoff: a restored pane comes back with its OUTPUT, not just
    /// its shape and directory.
    ///
    /// The recorded history is replayed into the fresh emulator before its child can write a byte,
    /// so the text is on the screen the MOMENT `restore` returns — this assertion needs no wait on
    /// the new shell, which is precisely the ordering guarantee the seam was built for.
    #[test]
    fn restore_replays_a_panes_recorded_history_onto_its_screen() {
        let live = Host::new((80, 24));
        let id = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let snap = sprag_terminal::snapshot(live.registry());

        let restored = Host::new((80, 24));
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |pane| format!("output of pane {pane}\r\n").into_bytes(),
            )
            .expect("a valid snapshot restores");

        assert_eq!(n, 1, "the pane came back");
        assert!(
            restored
                .pane_full_text(id)
                .contains(&format!("output of pane {id}")),
            "the restored pane's screen: {:?}",
            restored.pane_full_text(id),
        );
    }

    /// A restore CLAIMS the daemon's life for the whole re-spawn loop, and hands it back at the end.
    ///
    /// A restored registry describes sessions whose panes are being spawned one at a time, so for
    /// the length of that loop it reads as "nothing live" — and the first restored pane to exit
    /// instantly (a shell that execs and dies) would end the daemon while the rest were still
    /// coming back. Same blind spot as a `new_session`'s empty window, same claim closes it.
    ///
    /// Observed from INSIDE the loop through the injected `history` closure, which runs once per
    /// pane: that is deterministic, where racing a real early death against the remaining spawns
    /// would not be. A test that cannot fail on demand is not evidence.
    ///
    /// REVERT-PROOF: drop the `BirthPin` in `restore` and the in-loop assertion fails on the first
    /// pane.
    #[test]
    fn a_restore_claims_the_daemon_for_the_whole_respawn_loop() {
        let live = Host::new((80, 24));
        live.spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        live.spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let snap = sprag_terminal::snapshot(live.registry());

        let restored = Host::new((80, 24));
        let seen = std::cell::Cell::new(0usize);
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| {
                    seen.set(seen.get() + 1);
                    assert!(
                        lock(restored.registry()).birth_in_flight(),
                        "mid-loop, with panes still to come, the daemon is not finished",
                    );
                    Vec::new()
                },
            )
            .expect("a valid snapshot restores");

        assert_eq!(n, 2, "both panes came back");
        assert_eq!(
            seen.get(),
            2,
            "the claim was read on every pane, not just one"
        );
        assert!(
            !lock(restored.registry()).birth_in_flight(),
            "and the claim is handed back once the loop is done",
        );
    }

    /// A restore that brings NOTHING back leaves the daemon standing, waiting for a client.
    ///
    /// The other half of the claim, and the one that is easy to get backwards: zero panes at BOOT
    /// is the daemon's legitimate resting state — a daemon with no snapshot at all boots exactly
    /// like this. If releasing the claim re-asked the liveness question here, an empty restore
    /// would end the daemon that a client had just spawned, which is the same failure the claim
    /// exists to prevent moved one step earlier.
    ///
    /// REVERT-PROOF: pass `on_exit()` instead of `None` as the restore pin's signal and this fires.
    #[test]
    fn an_empty_restore_leaves_the_daemon_standing() {
        // A snapshot of a registry that has a session and a window but no panes — the shape a
        // daemon whose recorded panes all failed to come back is left holding.
        let empty = sprag_terminal::snapshot(Host::new((80, 24)).registry());

        let restored = Host::new((80, 24));
        let fired = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&fired);
        let signal = crate::spawn_reaper(
            Arc::clone(restored.registry()),
            Arc::new(move || {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
        );

        let n = restored
            .restore(
                empty,
                &std::collections::HashSet::new(),
                |_| None,
                || Some(crate::pane_exit_hook(&signal)),
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");
        assert_eq!(n, 0, "there was nothing to bring back");

        // Ample for the reaper thread to have scanned, had anything woken it.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(
            fired.load(Ordering::SeqCst),
            0,
            "an empty restore is a daemon waiting for a client, not one that has outlived its work",
        );
    }

    /// An absent history restores the pane BLANK rather than failing it — the pre-history
    /// behaviour, and what a disabled or unreadable history degrades to.
    #[test]
    fn restore_without_history_brings_the_pane_back_blank() {
        let live = Host::new((80, 24));
        let id = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let snap = sprag_terminal::snapshot(live.registry());

        let restored = Host::new((80, 24));
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");

        assert_eq!(n, 1, "the pane still came back");
        // Whatever the fresh shell has printed by now, nothing was REPLAYED into it.
        assert!(
            !restored.pane_full_text(id).contains("output of pane"),
            "no history was seeded",
        );
    }

    /// The reboot payoff at the host level: a live host is snapshotted, a FRESH host restores it,
    /// and every pane comes back under its OLD id in the SAME session — two in the default, one in
    /// a second session. Restore replaces the empty boot registry with the snapshot's shape and
    /// re-spawns each pane's shell; membership (which the plan carries) is independent of the tree.
    #[test]
    fn restore_rebuilds_the_shape_and_respawns_panes_under_their_old_ids() {
        // A live host: two panes in the default session…
        let live = Host::new((80, 24));
        let a = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let b = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        // …and a second session "work" with one pane of its own.
        lock(live.registry()).new_session(Some("work")).unwrap();
        let work_pool = lock(live.registry()).workspace_of("work").unwrap();
        let c = lock(&work_pool)
            .spawn(cat(), "sh".to_owned(), 80, 24)
            .unwrap();

        let snap = sprag_terminal::snapshot(live.registry());
        assert_eq!(snap.next_id, 3, "three panes minted across both sessions");

        // A FRESH host restores it, as a daemon boot would (no hooks needed for the mechanism).
        let restored = Host::new((80, 24));
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");
        assert_eq!(n, 3, "all three panes came back");

        // The default session's panes returned under their old ids.
        let ids = restored.pane_ids();
        assert!(
            ids.contains(&a) && ids.contains(&b),
            "default panes back under their old ids: {ids:?}",
        );
        // The second session and its pane returned — a restore is registry-wide, not just default.
        let work = lock(restored.registry()).workspace_of("work").unwrap();
        let work_ids: Vec<PaneId> = lock(&work).panes().iter().map(Pane::id).collect();
        assert_eq!(work_ids, vec![c], "work's pane came back under its old id");

        // A fresh spawn on the restored host mints ABOVE the restored ids — never reissuing one.
        let next = restored
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        assert_eq!(
            next,
            PaneId(3),
            "the counter resumed above the restored ids"
        );
    }

    /// A pane's PROVENANCE survives a reboot, driven through the real `Host::restore` loop rather
    /// than through the plan it consumes.
    ///
    /// The loop is what this pins, deliberately: the snapshot half and the plan half each have
    /// their own test in `sprag-terminal`, and both stayed green while this path dropped the fact on
    /// the floor. Ids come back exactly, so a provenance that did not would leave every
    /// agent-opened pane claimed by nobody — closable by no agent, and invisible to the person
    /// asking what opened it. See [`Pane::opened_by`](sprag_terminal::Pane::opened_by).
    #[test]
    fn a_restore_brings_back_who_asked_for_a_pane() {
        let live = Host::new((80, 24));
        let opener = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let opened = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        lock(&live.workspace()).set_pane_opened_by(opened, opener);

        let restored = Host::new((80, 24));
        restored
            .restore(
                sprag_terminal::snapshot(live.registry()),
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");

        let pool = restored.workspace();
        let pool = lock(&pool);
        assert_eq!(
            pool.pane(opened)
                .expect("the opened pane came back")
                .opened_by(),
            Some(opener),
            "the reborn pane still knows which pane asked for it",
        );
        assert_eq!(
            pool.pane(opener).expect("the opener came back").opened_by(),
            None,
            "and a pane nobody asked for is not handed an opener by the restore",
        );
    }

    /// A pane's NAME survives a reboot, driven through the real `Host::restore` LOOP.
    ///
    /// Same shape as the provenance test above and for a sharper reason: a name is an ADDRESS, so a
    /// script or an agent that says `--pane build` resolves to nothing after a restart unless the
    /// name comes back with the pane. `sprag-terminal` already pins the snapshot half and the plan
    /// half; **both would stay green while this loop dropped the fact**, which is exactly the hole
    /// R291 and R292 each shipped a round apart — a unit test on a method is not a test that the
    /// caller calls it.
    #[test]
    fn a_restore_brings_back_what_a_pane_is_called() {
        let live = Host::new((80, 24));
        let named = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        let anonymous = live
            .spawn(cat(), "sh".to_owned(), 80, 24, PaneBirthHooks::default())
            .unwrap();
        lock(&live.workspace()).set_pane_name(
            named,
            Some(sprag_terminal::PaneName::parse("build").unwrap()),
        );

        let restored = Host::new((80, 24));
        restored
            .restore(
                sprag_terminal::snapshot(live.registry()),
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");

        let pool = restored.workspace();
        let pool = lock(&pool);
        assert_eq!(
            pool.pane(named)
                .expect("the named pane came back")
                .name()
                .map(sprag_terminal::PaneName::as_str),
            Some("build"),
            "the reborn pane still answers to the name it was given",
        );
        assert_eq!(
            pool.pane(anonymous)
                .expect("the other pane came back")
                .name(),
            None,
            "and a pane nobody named is not handed a name by the restore",
        );
    }

    /// Restore honors a FLOATED pane (comes back in the float set, not the tiling) and a pane with
    /// NO recorded cwd (re-spawns anyway, falling back to the daemon's cwd). Driven from a
    /// hand-authored snapshot so both are exercised through the real `restore` path — the
    /// registry-level round-trip has a float but never re-spawns it here.
    #[test]
    fn restore_honors_a_floated_pane_and_a_cwdless_pane() {
        use sprag_terminal::{PaneSnapshot, SessionSnapshot, WindowSnapshot};

        let snap = Snapshot {
            version: sprag_terminal::SNAPSHOT_VERSION,
            next_id: 2,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(), // pane 0 gets appended by the first reconcile
                    floating: vec![PaneId(1)],
                    panes: vec![
                        PaneSnapshot {
                            id: PaneId(0),
                            cwd: Some("/tmp".into()),
                            command_label: "sh".to_owned(),
                            argv: vec!["sh".to_owned()],
                            agent_session: None,
                            remote: None,
                            opened_by: None,
                            name: None,
                            cols: 80,
                            rows: 24,
                        },
                        PaneSnapshot {
                            id: PaneId(1),
                            cwd: None, // no recorded cwd -> falls back to the daemon's
                            command_label: "sh".to_owned(),
                            argv: vec!["sh".to_owned()],
                            agent_session: None,
                            remote: None,
                            opened_by: None,
                            name: None,
                            cols: 80,
                            rows: 24,
                        },
                    ],
                    manual_size: None,
                    active: None,
                    zoomed: None,
                    opened_by: None,
                }],
            }],
        };

        let host = Host::new((80, 24));
        let n = host
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
                || None,
                || None,
                |_| Vec::new(),
            )
            .expect("a valid snapshot restores");
        assert_eq!(n, 2, "both the cwd and the cwd-less pane re-spawned");
        let ids = host.pane_ids();
        assert!(
            ids.contains(&PaneId(0)) && ids.contains(&PaneId(1)),
            "both panes are live: {ids:?}",
        );
        // Pane 1 came back FLOATED — the window's float set holds it.
        let registry = lock(host.registry());
        let window = registry.session("0").unwrap().current_window();
        assert!(
            window.floating().contains(&PaneId(1)),
            "the floated pane restored as floated, not tiled",
        );
    }

    /// The exact-command path through `Host::restore` end to end: a pane whose argv is an
    /// ALLOWLISTED non-shell program (`cat`, long-lived on a PTY) comes back RE-RUNNING that
    /// program — its `command_label` is `cat`, not a shell. The allowlist is passed explicitly
    /// (hermetic — no env), so this is the only test that drives the re-run wiring live.
    #[test]
    fn restore_reruns_an_allowlisted_program_end_to_end() {
        use sprag_terminal::{PaneSnapshot, SessionSnapshot, WindowSnapshot};

        let allow: std::collections::HashSet<String> = ["cat".to_owned()].into_iter().collect();
        let snap = Snapshot {
            version: sprag_terminal::SNAPSHOT_VERSION,
            next_id: 1,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(),
                    floating: vec![],
                    panes: vec![PaneSnapshot {
                        id: PaneId(0),
                        cwd: None,
                        command_label: "cat".to_owned(),
                        argv: vec!["cat".to_owned()], // allowlisted -> re-run exactly
                        agent_session: None,
                        remote: None,
                        opened_by: None,
                        name: None,
                        cols: 80,
                        rows: 24,
                    }],
                    manual_size: None,
                    active: None,
                    zoomed: None,
                    opened_by: None,
                }],
            }],
        };

        let host = Host::new((80, 24));
        assert_eq!(
            host.restore(snap, &allow, |_| None, || None, || None, |_| Vec::new())
                .expect("restores"),
            1,
        );
        let ws = host.workspace();
        let pool = lock(&ws);
        assert_eq!(
            pool.pane(PaneId(0)).unwrap().command_label(),
            "cat",
            "the allowlisted program re-ran exactly, not a shell fallback",
        );
    }

    /// ⚠⚠⚠ **A RESTORED AGENT COMES BACK INTO THE CONVERSATION IT WAS IN — AND A SHELL FALLBACK
    /// DOES NOT.** The wiring claim, which no unit test can reach.
    ///
    /// # What is being held, and what it cost to not hold it
    ///
    /// A daemon cannot adopt new code without restarting, and a restart re-spawns every pane. Before
    /// this, a restored agent came back in the right directory, correctly instrumented, and
    /// **remembering nothing**: the naming happens per birth, the recorded argv carries no name, so a
    /// fresh one was minted and the transcript the agent had been writing was orphaned on disk under
    /// a name nothing pointed at any more. Measured on the live daemon of 2026-08-16 — the loop's
    /// inner pane held a 3.5 MB record under `d8be3b14-…` and its snapshot row read `argv:
    /// ["claude"]`.
    ///
    /// # ⚠⚠ Why the second half is not the first with a word changed
    ///
    /// `restore_command` re-runs an argv EXACTLY only for an allowlisted program; everything else
    /// comes back as a plain shell in the cwd. The recorded name travels with the pane either way, so
    /// the resume decision has to be made against **what actually re-ran** rather than against what
    /// was recorded — a shell handed `--resume` is a shell handed an argument meant for something
    /// else, and it would land on the one path nobody watches, after a reboot.
    ///
    /// ⚠ Asserted through [`Pane::agent_session`], which `spawn_restored` fills by reading the BUILT
    /// command — so a green here is also the chained-restore claim: a pane that came back resuming
    /// `X` still says it is in `X`, and the SECOND restart can resume it again. A reader that knew
    /// only the minting flag would lose the name on that second restart, which looks fixed and is
    /// not.
    ///
    /// # ⚠⚠⚠ The fixture MANUFACTURED ITS OWN AGREEMENT once, and this is what it took to stop it
    ///
    /// The shell-fallback half first used a pane whose program was not an agent at all
    /// (`definitely-not-an-agent`). It passed, and so did the mutation it exists to catch — deciding
    /// the resume from the RECORDED argv instead of the built command — because for that pane the two
    /// readings agree: neither is an agent, so neither takes a resume.
    ///
    /// **The only case that separates them is an argv whose program IS an agent and which the
    /// allowlist does NOT admit**: recorded, it reads `claude`; built, it is a shell. So the same
    /// snapshot is restored a second time with an EMPTY allowlist, which is a shape a cautious
    /// operator really configures (`SPRAG_RESTORE_PROGRAMS=`). Without that second restore this gate
    /// is two assertions that hold nothing between them.
    #[test]
    fn a_restored_agent_resumes_its_conversation_and_a_shell_fallback_does_not() {
        use sprag_terminal::{PaneSnapshot, SessionSnapshot, WindowSnapshot};

        const NAME: &str = "d8be3b14-3f26-4220-96f5-c57a462ea383";

        // A program BASENAMED `claude`, which is what both the allowlist and `hooks::agent_of` read.
        // `cat` stands in for the agent: this gate is about which arguments reach the launch, and a
        // real agent would answer that identically while costing a model call.
        let dir = std::env::temp_dir().join(format!("sprag-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory for the stand-in agent");
        let agent = dir.join("claude");
        std::fs::copy("/bin/cat", &agent).expect("a stand-in agent to launch");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o755))
                .expect("the stand-in must be executable");
        }

        let pane = |id: u64, argv: Vec<String>| PaneSnapshot {
            id: PaneId(id),
            cwd: None,
            command_label: "claude".to_owned(),
            argv,
            // BOTH panes carry the name. The difference the gate is about is what re-ran, not what
            // was recorded — so recording differs here would prove nothing.
            agent_session: Some(NAME.to_owned()),
            remote: None,
            opened_by: None,
            name: None,
            cols: 80,
            rows: 24,
        };
        let snap = Snapshot {
            version: sprag_terminal::SNAPSHOT_VERSION,
            next_id: 2,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(),
                    floating: vec![],
                    panes: vec![
                        pane(0, vec![agent.to_string_lossy().into_owned()]),
                        // NOT allowlisted -> `restore_command` falls back to a plain shell.
                        pane(1, vec!["definitely-not-an-agent".to_owned()]),
                    ],
                    manual_size: None,
                    active: None,
                    zoomed: None,
                    opened_by: None,
                }],
            }],
        };

        let restored = |allow: std::collections::HashSet<String>| {
            let host = Host::new((80, 24)).with_pane_identity(pane_identity_source());
            assert_eq!(
                host.restore(
                    snap.clone(),
                    &allow,
                    |_| None,
                    || None,
                    || None,
                    |_| { Vec::new() }
                )
                .expect("restores"),
                2,
            );
            let ws = host.workspace();
            let pool = lock(&ws);
            [PaneId(0), PaneId(1)].map(|id| {
                pool.pane(id)
                    .and_then(Pane::agent_session)
                    .map(str::to_owned)
            })
        };

        // ── THE AGENT IS ADMITTED: it re-runs exactly, so it takes the resume ──
        let admitted = restored(["claude".to_owned()].into_iter().collect());
        assert_eq!(
            admitted[0].as_deref(),
            Some(NAME),
            "⚠⚠⚠ THE RESTORED AGENT WAS NAMED AFRESH INSTEAD OF RESUMED. Its transcript is still on \
             disk under {NAME:?} and nothing points at it any more — the agent comes up knowing \
             nothing, and every restart of this daemon costs a session's whole context. The resume \
             is appended in `Host::restore`; without it `identity_args` mints",
        );
        assert_eq!(
            admitted[1], None,
            "⚠⚠ a pane whose program is not an agent takes nothing, whatever its row records",
        );

        // ── THE SAME AGENT, NOT ADMITTED: it comes back a SHELL, so it must take nothing ──
        // ⚠ This is the half that separates *read the built command* from *read the recorded argv*.
        // Pane 0's row still says `claude`; only what re-ran differs.
        let refused = restored(std::collections::HashSet::new());
        assert_eq!(
            refused,
            [None, None],
            "⚠⚠⚠ A SHELL FALLBACK TOOK A RESUME. With an empty allowlist an agent comes back as a \
             plain shell in its cwd, and `--resume <uuid>` appended to that is an argument meant for \
             something else. The decision must be read from the BUILT command — the recorded argv \
             still names an agent here, which is exactly why reading it is wrong",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The Slice-5 security-defining restore path: a SANCTIONED remote workspace (its structured
    /// `remote` endpoint is `sprag ssh` intent) RECONNECTS on restore — `ssh` runs even though it is
    /// NOT in the allowlist (the bypass) — while a pane whose argv merely CONTAINS `ssh` but has no
    /// `remote` marker falls back to a shell, so an incidentally-typed `ssh host '<cmd>'` never
    /// re-runs itself. Also proves the reconnected pane STAYS marked remote (chained-restore safe).
    #[test]
    fn restore_reconnects_a_remote_workspace_but_not_a_bare_ssh_argv() {
        use sprag_terminal::{PaneSnapshot, SessionSnapshot, SshRemote, WindowSnapshot};

        // `ssh` is deliberately NOT allowlisted — so ONLY the structured `remote` reconnects.
        let allow: std::collections::HashSet<String> = std::collections::HashSet::new();
        let snap = Snapshot {
            version: sprag_terminal::SNAPSHOT_VERSION,
            next_id: 2,
            default_size: (80, 24),
            sessions: vec![SessionSnapshot {
                name: "0".to_owned(),
                current_window: "0".to_owned(),
                windows: vec![WindowSnapshot {
                    name: "0".to_owned(),
                    layout: LayoutWire::default(),
                    floating: vec![],
                    panes: vec![
                        PaneSnapshot {
                            id: PaneId(0),
                            cwd: None,
                            command_label: "ssh".to_owned(),
                            argv: vec!["ssh".to_owned(), "-t".to_owned(), "srv".to_owned()],
                            agent_session: None,
                            remote: Some(SshRemote {
                                user: None,
                                host: "srv".to_owned(),
                                port: None,
                            }),
                            opened_by: None,
                            name: None,
                            cols: 80,
                            rows: 24,
                        },
                        PaneSnapshot {
                            id: PaneId(1),
                            cwd: None,
                            command_label: "ssh".to_owned(),
                            // A shell that merely had `ssh` in its argv — NOT a sanctioned workspace.
                            argv: vec!["ssh".to_owned(), "host".to_owned(), "danger".to_owned()],
                            agent_session: None,
                            remote: None,
                            opened_by: None,
                            name: None,
                            cols: 80,
                            rows: 24,
                        },
                    ],
                    manual_size: None,
                    active: None,
                    zoomed: None,
                    opened_by: None,
                }],
            }],
        };

        let host = Host::new((80, 24));
        assert_eq!(
            host.restore(snap, &allow, |_| None, || None, || None, |_| Vec::new())
                .expect("restores"),
            2,
        );
        let ws = host.workspace();
        let pool = lock(&ws);

        // Pane 0 RECONNECTED (ssh ran despite the empty allowlist) and stays marked remote.
        let reconnected = pool.pane(PaneId(0)).unwrap();
        assert_eq!(
            reconnected.command_label(),
            "ssh",
            "the sanctioned remote workspace reconnected — the allowlist was bypassed",
        );
        assert!(
            reconnected.remote().is_some(),
            "the reconnected pane stays marked remote for a chained restore",
        );

        // Pane 1 did NOT reconnect: a bare ssh argv with no intent marker falls back to a shell.
        let shell = pool.pane(PaneId(1)).unwrap();
        assert_ne!(
            shell.command_label(),
            "ssh",
            "a bare ssh argv without the remote marker is NOT auto-reconnected — a shell",
        );
        assert!(shell.remote().is_none());
    }

    #[test]
    fn send_text_reaches_the_pane_pty() {
        use std::time::{Duration, Instant};
        let host = Host::new((40, 6));
        let id = host
            .spawn(cat(), "cat".to_owned(), 40, 6, PaneBirthHooks::default())
            .unwrap();
        assert!(host.send_text(id, "hello"));
        // The cooked-mode `cat` echoes it back into the pane's screen.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if host.pane_full_text(id).contains("hello") {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the sent text never echoed back through the host");
    }

    /// **The IN-PROCESS arm of `resize_window`** (R331) — a driver, because the register keeps
    /// filing this shape (53b, 54a) as *"reached only by a GUI running its own host"* and one unit
    /// test is cheaper than the note saying it is untested.
    ///
    /// Two arms and they differ in a way no wire test can show: this host tracks NO clients, so
    /// `-a`/`-A` have nothing to fold and are refused, while an exact rectangle is stored — the same
    /// distinction the daemon draws for a session nobody is attached to, arrived at through a
    /// completely different code path.
    ///
    /// The POLICY on the answer is `Some` and not `None`: this process IS the one that lays the
    /// panes out, so it always knows what it is arbitrating under. A `None` here would be a client
    /// telling itself it could not find out what it had just decided.
    #[test]
    fn the_in_process_host_pins_a_window_and_refuses_a_fold_with_no_clients() {
        use crate::window::SizeRequest;

        let host = Host::new((40, 6));
        let pinned = HostClient::resize_window(
            &host,
            SizeRequest::Exact(crate::attach::ClientSize {
                cols: 100,
                rows: 30,
            }),
        )
        .expect("an exact rectangle needs nothing from a client");
        assert_eq!(
            pinned.size,
            Some(crate::attach::ClientSize {
                cols: 100,
                rows: 30
            }),
        );
        assert!(
            pinned.policy.is_some(),
            "the process that lays the panes out always knows its own policy: {pinned:?}",
        );
        // ...and a RELATIVE request moves the pin that is now there, which is what says the store
        // above reached the registry rather than only the answer.
        let moved = HostClient::resize_window(&host, SizeRequest::Adjust { cols: -20, rows: 0 })
            .expect("a relative request has the pin to move");
        assert_eq!(
            moved.size,
            Some(crate::attach::ClientSize { cols: 80, rows: 30 }),
        );
        assert_eq!(
            HostClient::resize_window(&host, SizeRequest::Clients(crate::WindowSize::Largest)),
            None,
            "this host folds no clients, so a fold names no rectangle — refused, never an un-pin",
        );
        assert_eq!(
            HostClient::resize_window(&host, SizeRequest::Clear)
                .expect("an un-pin needs nothing at all")
                .size,
            None,
        );
    }

    #[test]
    fn an_absent_id_returns_graceful_defaults() {
        // The widened identity contract (R121): a `PaneId` with no live pane no-ops /
        // placeholders rather than panicking (was `with_pane`'s `.expect` before).
        let host = Host::new((40, 6));
        let ghost = PaneId(999);
        assert_eq!(host.pane_grid_size(ghost), (1, 1));
        assert_eq!(
            (
                host.pane_cells(ghost, 0).cols(),
                host.pane_cells(ghost, 0).rows()
            ),
            (1, 1)
        );
        assert!(!host.send_text(ghost, "x"));
        assert!(!host.send_key(ghost, "a", Modifiers::default()));
        assert!(host.pane_full_text(ghost).is_empty());
        assert!(host.pane_command_label(ghost).is_empty());
        assert!(host.pane_handle(ghost).is_none());
        host.resize(ghost, 10, 10, (0, 0)); // no panic
    }
}
