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
use sprag_input::{Modifiers, MouseInput};
use sprag_terminal::{
    CommandBuilder, LayoutSnapshot, LayoutWire, Pane, PaneId, PanePtyError, PanePtyHandle,
    PaneRebirth, SessionInfo, SessionRegistry, Snapshot, SnapshotError, SplitDir, SplitSide,
    WindowInfo, Workspace,
};
use sprag_vt::{ClipboardTarget, ClipboardTargets, Image, MouseProtocol, Screen, osc52_reply};

use crate::external::lock;
use crate::scope::SessionScope;

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
}

impl PaneScrollFacts {
    /// Read the non-cell facts from a live `screen` — the SINGLE population site,
    /// shared by [`Host::pane_scroll_facts`](HostClient::pane_scroll_facts) and the
    /// wire `cells` action, so the two never disagree on how a fact is derived
    /// (adding a fact edits only here + the struct).
    pub(crate) fn from_screen(screen: &Screen) -> Self {
        Self {
            scrollback_len: screen.scrollback_len(),
            visible_rows: screen.rows(),
        }
    }
}

/// One find-in-scrollback match, as a client reads it off the wire — the serde projection of
/// [`sprag_vt::FindMatch`], whose coordinate this carries unchanged: `line` counts logical lines
/// from the pane's OLDEST retained line (the scroll `offset_y` axis, so a client jumps to a match
/// with the offset it already speaks) and `col`/`cols` are CELL columns, ready to overlay.
///
/// The VT layer stays serde-free by design ("the VT layer owns no wire shape"), so the wire shape
/// lives here, beside [`PaneScrollFacts`] and for the same reason: ONE definition both the host's
/// `find.<needle>` query and every client deserialize, so the JSON keys cannot drift.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneMatch {
    /// Logical line index from the pane's oldest retained line.
    pub line: usize,
    /// Starting cell column within that line.
    pub col: u16,
    /// Width in cell columns (a wide cluster counts two).
    pub cols: u16,
}

/// One line carrying at least one match, with its text — the serde projection of
/// [`sprag_vt::FindLine`], and the DISPLAY view of a search beside [`PaneFind::matches`]'s
/// coordinate view.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneFindLine {
    /// The logical line index — the join key back to [`PaneMatch::line`].
    pub line: usize,
    /// The line's text, trailing blanks trimmed.
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
                    col: m.col,
                    cols: m.cols,
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
pub trait HostClient {
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

    /// Install `tree` as the current window's arrangement, returning the CANONICAL result
    /// — the write half of the arc (see [`sprag_terminal::layout`]).
    ///
    /// `expected` is the revision this gesture was authored against; the write is REFUSED if
    /// the arrangement has moved on (another client, a plugin's spawn), because a gesture
    /// means "in the layout I am looking at". The answer is the tree as the host stores it,
    /// with every divider the client minted now named, so the caller adopts it directly
    /// rather than re-reading — and a refused write answers with the arrangement actually in
    /// force, so a client always learns the truth it must project.
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

    /// Make the window named `name` the scoped session's current window (tmux `select-window`).
    /// Every attached client's next read then projects that window. A no-op for an unknown name.
    fn select_window(&self, name: &str);

    /// Create a window in the scoped session, born with a shell, and select it (tmux
    /// `new-window`), returning its name.
    fn new_window(&self) -> String;

    /// Kill the window named `name` of the scoped session (tmux `kill-window`); the session's
    /// LAST window ends the session. A no-op for an unknown name.
    fn kill_window(&self, name: &str);

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
    fn new_pane(&self) -> Option<PaneId> {
        None
    }

    /// Close pane `id` — kill its child and drop it from the window (tmux `kill-pane`), returning
    /// whether a pane was actually removed (`false` for an absent id).
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
    /// The window emptied by the last pane is not closed either: the arrangement reconciles to an
    /// empty tree and the window remains, exactly as a window whose panes all ran `exit` does.
    ///
    /// Defaulted to `false`, like [`new_pane`](Self::new_pane).
    fn kill_pane(&self, id: PaneId) -> bool {
        let _ = id;
        false
    }

    /// Break the pane `id` out of its window into a NEW window of the scoped session (tmux
    /// `break-pane`), returning the new window's name — or `None` if the move was refused (the
    /// pane's window has only that pane, an explicit `name` is already taken, or no window holds
    /// `id`). The pane is MOVED whole (no re-spawn); the new window is selected.
    ///
    /// Defaulted to `None` — a display client that never breaks panes (and the test doubles) need
    /// not implement it; the in-process [`Host`] and the wire client override it.
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

    /// Move the pane `id` into the window named `dst` of the scoped session (tmux `join-pane`),
    /// returning whether the source window was CLOSED (a join that emptied it) — or `None` if the
    /// move was refused (`id` already lives in `dst`, no window holds `id`, or `dst` names no
    /// window). Defaulted to `None`, like [`break_pane`](Self::break_pane).
    fn join_pane(&self, id: PaneId, dst: &str) -> Option<bool> {
        let _ = (id, dst);
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

    /// Create a fresh session on the host (born with a shell, tmux `new-session`) and switch this
    /// client to it, returning its name. The "+" of a session sidebar.
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
    fn kill_session(&self, name: &str);

    /// Reconcile a session this client lost OUT OF BAND — killed by ANOTHER client or the `sprag`
    /// CLI while we were attached — against the `detach-on-destroy` policy: switch to a neighbouring
    /// session or detach. Called every frame from the pre-view reconcile, because the out-of-band
    /// destroy is detected on the wire client's background poll thread, which cannot itself perform a
    /// switch (a UI-thread operation) — it flags the condition and this resolves it on the UI thread.
    ///
    /// The in-process arm renders one default session it can never lose out of band (it has no daemon
    /// and no second client), so the default is a NO-OP; only the wire client overrides it.
    fn reconcile_lost_session(&self) {}

    /// Switch this client to the LAST session — the most-recently-used OTHER session it visited that
    /// is still live (tmux `switch-client -l`), a keyboard `Ctrl+Shift+L` affordance. A no-op with no
    /// such session (never switched, or the prior sessions are gone). The in-process arm renders only
    /// the default session and has no visit history, so the default is a NO-OP; only the wire client
    /// (which keeps the MRU stack) overrides it.
    fn switch_to_last_session(&self) {}

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

    /// The pane's most recent attention [`PaneNotification`] (`OSC 9` / `OSC 777;notify` /
    /// `OSC 99`), or `None` if it raised none. Like [`pane_title`](Self::pane_title) this is
    /// LIVE, CHILD-CONTROLLED display state — a display client surfaces it as "this pane wants
    /// attention" and detects a NEW one via the [`seq`](PaneNotification::seq) growing past the
    /// last it acknowledged. An absent pane and a pane that raised nothing both flatten to
    /// `None`. Defaulted to `None` so an older [`HostClient`] impl need not implement it.
    fn pane_notification(&self, _id: PaneId) -> Option<PaneNotification> {
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
    /// How a pane born through [`HostClient::new_pane`] is wired to its client — see
    /// [`with_pane_hooks`](Self::with_pane_hooks). `None` leaves such a pane unwired.
    pane_hooks: Option<PaneHooks>,
}

/// The `on_dirty` FACTORY a [`Host`] wires each client-created pane with: a fresh hook per pane,
/// because a `Box<dyn Fn>` cannot be reused. The same shape [`Host::restore`] takes per call — the
/// difference being that a restore's caller is present to supply one and
/// [`HostClient::new_pane`]'s is not, so this is held instead of passed.
type PaneHooks = Arc<dyn Fn() -> Option<Box<dyn Fn() + Send>> + Send + Sync>;

impl Host {
    /// A new host over a registry with one empty session / window whose dimension-less
    /// spawns adopt `default_size`. Boot panes are added with [`spawn`](Self::spawn).
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        Self {
            registry: Arc::new(Mutex::new(SessionRegistry::new(default_size))),
            pane_hooks: None,
        }
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

    /// Spawn a boot pane running `command` (labelled `label`) at `cols x rows`,
    /// returning its id. `on_dirty` is the pinion-free wake hook a windowed client
    /// passes (`Some(Box::new(move || sink.request_repaint()))`, the R999
    /// `RepaintSink` seam) so this pane's output repaints the window; the headless
    /// server passes `bump_on_dirty`. `on_exit` is the "this child is gone" hook the daemon
    /// feeds to its reaper to end its own process when the last live pane dies (the headless
    /// server passes [`pane_exit_hook`](crate::pane_exit_hook); a windowed / test caller passes
    /// `None`). Both are `Box<dyn Fn() + Send>` (not pinion types), so the display and
    /// lifetime concerns live above while the spawn lives here.
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
        on_dirty: Option<Box<dyn Fn() + Send>>,
        on_exit: Option<Box<dyn Fn() + Send>>,
    ) -> Result<PaneId, PanePtyError> {
        let workspace = self.workspace();
        lock(&workspace).spawn_with_dirty(command, label, cols, rows, on_dirty, on_exit)
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
        history: impl Fn(PaneId) -> Vec<u8>,
    ) -> Result<usize, SnapshotError> {
        // Build the new shape FIRST (fallible), so a bad snapshot leaves the boot registry intact.
        let (registry, plan) = SessionRegistry::from_snapshot(snapshot)?;
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
            let (command, label) = match &pane.remote {
                Some(remote) => crate::reconnect_command(remote),
                None => crate::restore_command(&pane.argv, pane.cwd.as_deref(), allowlist),
            };
            // Bind the spawn result so the pool lock RELEASES at the `;` — a `match` scrutinee's
            // temporary lock would live across the arms, and the `Ok` arm re-locks to mark the pane
            // remote, which on a non-reentrant `Mutex` would self-deadlock.
            let spawned = lock(&pool).spawn_restored(PaneRebirth {
                id: pane.id,
                command,
                label,
                size: (pane.cols, pane.rows),
                on_dirty: on_dirty(&pane.session),
                on_exit: on_exit(),
                history: history(pane.id),
            });
            match spawned {
                Ok(()) => {
                    // Keep the restored pane marked remote so a CHAINED restore reconnects it again.
                    if let Some(remote) = pane.remote {
                        lock(&pool).set_pane_remote(pane.id, remote);
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
    })
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

/// Divide `target`'s cell in the scoped window and put `pane` in the half on `side` — the ONE
/// place a directional split lands (see [`crate::wire::SPLIT_ACTION`]).
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
        .is_some_and(|window| window.split_pane(pane, target, side, dir, &panes))
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

impl HostClient for Host {
    fn pane_ids(&self) -> Vec<PaneId> {
        let workspace = self.workspace();
        lock(&workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_cells(&self, id: PaneId, offset_lines: usize) -> GridBuffer {
        self.with_pane_id(id, |pane| crate::pane_cells(pane.pty(), offset_lines))
            .unwrap_or_else(|| GridBuffer::new(1, 1))
    }

    fn pane_scroll_facts(&self, id: PaneId) -> PaneScrollFacts {
        self.with_pane_id(id, |pane| {
            pane.pty().with_screen(PaneScrollFacts::from_screen)
        })
        .unwrap_or(PaneScrollFacts {
            scrollback_len: 0,
            visible_rows: 1,
        })
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
    fn send_key(&self, id: PaneId, key: &str, mods: Modifiers) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::send_key(&handle, key, mods))
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

    fn send_text(&self, id: PaneId, text: &str) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::send_text(&handle, text))
    }

    /// Brackets the paste (and filters an embedded end marker) at the PTY boundary when the pane's
    /// child enabled DEC private mode 2004 — the mode is read live from the emulator here, so the
    /// bracketing cannot disagree with what the child asked for. `false` for an absent id.
    fn paste(&self, id: PaneId, text: &str) -> bool {
        self.with_pane_id(id, Pane::handle)
            .is_some_and(|handle| crate::paste(&handle, text))
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

    /// The DEFAULT session's windows (this arm scopes there; see [`Host::workspace`]). Total: the
    /// default always resolves.
    fn windows(&self) -> Vec<WindowInfo> {
        lock(&self.registry).default_session().window_infos()
    }

    /// Select a window of the default session; an unknown name is a no-op (logged by the registry
    /// through the `Result` this arm discards — the in-process caller has no wire to refuse on).
    fn select_window(&self, name: &str) {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        let _ = registry.select_window(&session, name);
    }

    /// Create + select a window in the default session, birthing a shell into it — the in-process
    /// arm's OWN spawn path (no wake / reaper hooks; the daemon births at its `WorkspaceExternal`).
    /// `self.workspace()` is the new window's pool because `new_window` selected it.
    fn new_window(&self) -> String {
        let created = {
            let mut registry = lock(&self.registry);
            let session = registry.default_session().name().to_owned();
            registry.new_window(&session, None)
        };
        let Ok(name) = created else {
            // The default session always resolves and the allocated name is free by construction,
            // so an in-process create cannot fail — but never panic on the arm that unwraps least.
            return String::new();
        };
        let (command, label) = sprag_terminal::default_shell_command();
        let (cols, rows) = lock(&self.workspace()).default_size();
        let _ = self.spawn(command, label, cols, rows, None, None);
        name
    }

    /// Kill a window of the default session; the last window ends the session. An unknown name is a
    /// no-op. The in-process arm has no daemon to exit, so the reaped panes just drop here —
    /// OFF the registry lock (the outcome is bound after the lock guard falls at the `;`).
    fn kill_window(&self, name: &str) {
        let session = lock(&self.registry).default_session().name().to_owned();
        let _outcome = lock(&self.registry).kill_window(&session, name);
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
    /// `$SHELL` through [`default_shell_command`](sprag_terminal::default_shell_command) — the same
    /// SSOT the `spawn` wire action's `cmd`-less default resolves, so an in-process client and a
    /// wire client are born with the same program rather than two ideas of "a shell".
    ///
    /// The pane adopts the workspace's default size: a client-created pane has no geometry of its
    /// own until the first reflow gives it its tile, exactly as a boot pane does not.
    fn new_pane(&self) -> Option<PaneId> {
        let (command, label) = sprag_terminal::default_shell_command();
        let on_dirty = self.pane_hooks.as_ref().and_then(|hooks| hooks());
        let workspace = self.workspace();
        let mut workspace = lock(&workspace);
        let (cols, rows) = workspace.default_size();
        workspace
            .spawn_with_dirty(command, label, cols, rows, on_dirty, None)
            .ok()
    }

    /// Remove the pane, bound so the pool guard drops FIRST and the reaped `Pane`'s blocking `Drop`
    /// (kill / wait / join the reader) runs outside the lock — the discipline the `close` wire
    /// action keeps for the same reason.
    fn kill_pane(&self, id: PaneId) -> bool {
        let removed = lock(&self.workspace()).close(id);
        removed.is_some()
    }

    /// Break the pane `id` out into a new window of the default session (tmux `break-pane`). The
    /// pane is MOVED (already spawned — no birth here, unlike [`new_window`](Self::new_window)) and
    /// the new window selected. `None` if the move was refused.
    fn break_pane(&self, id: PaneId, name: Option<&str>) -> Option<String> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        registry.break_pane(&session, id, name).ok()
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

    /// Move the pane `id` into the window named `dst` of the default session (tmux `join-pane`),
    /// returning whether the emptied source window was closed. `None` if refused.
    fn join_pane(&self, id: PaneId, dst: &str) -> Option<bool> {
        let mut registry = lock(&self.registry);
        let session = registry.default_session().name().to_owned();
        registry.join_pane(&session, id, dst).ok()
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
    /// (both built by [`SessionRegistry::session_infos_live`], the ONE enriched builder), marking
    /// the default and carrying each session's live cwd + git branch. Not narrowed to the default
    /// even though this arm only renders that one: the list's whole purpose is to enumerate the
    /// scopes a switcher could name.
    fn sessions(&self) -> Vec<SessionInfo> {
        let mut infos = SessionRegistry::session_infos_live(&self.registry);
        // Same human-facing filter the wire `sessions` slot applies (the SSOT rule), so the
        // in-process arm and the daemon cannot disagree on whether the resting anchor lists. An
        // in-process host has no attachment map, so `attached` stays 0 and a session lists on its
        // pane count alone — the empty anchor drops, a working session stays.
        infos.retain(SessionInfo::is_listable);
        infos
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
    fn kill_session(&self, _name: &str) {}
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

    /// The in-process arm reads a pane's project from that pane's OWN live cwd.
    ///
    /// REVERT-PROOF: point `Host::project` at the daemon's cwd instead of the pane's and this fails
    /// (the test process does not run inside the temporary project).
    #[test]
    fn a_panes_project_is_read_from_that_panes_working_directory() {
        let root = temp_project("local");
        let host = Host::new((40, 6));
        let id = host
            .spawn(cat_in(&root), "cat".to_owned(), 40, 6, None, None)
            .expect("spawn a pane inside the project");

        let project = host
            .project(id)
            .expect("the pane sits in a project")
            .expect("its config parses");
        assert_eq!(project.root, root);
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
            .spawn(cat_in(&root), "cat".to_owned(), 40, 6, None, None)
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
        host.spawn(cat(), "cat".to_owned(), 40, 6, None, None)
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
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
            .unwrap();
        assert_eq!(host.pane_ids(), vec![id]);
        assert_eq!(host.pane_grid_size(id), (40, 6));
    }

    /// The client-protocol pane pair: `new_pane` grows the CURRENT window's set with a shell, and
    /// `kill_pane` removes exactly the pane named — answering `false` for one that is not there.
    ///
    /// REVERT-PROOF: have `new_pane` return `None` without spawning and the set never grows; have
    /// `kill_pane` ignore its answer and the absent-id assertion fails, which is the difference
    /// between "I closed it" and "there was nothing to close".
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

        assert!(host.kill_pane(first), "the named pane is removed");
        assert_eq!(host.pane_ids(), vec![second], "and only that one");
        assert!(
            !host.kill_pane(first),
            "closing it again reports that there was nothing to close"
        );
        assert!(host.kill_pane(second), "the window's LAST pane closes too");
        assert!(
            host.pane_ids().is_empty(),
            "leaving an empty window rather than refusing"
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
            .spawn(command, "sh".to_owned(), 40, 6, None, None)
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
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
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
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
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
            .spawn(cat(), "sh".to_owned(), 80, 24, None, None)
            .unwrap();
        let snap = sprag_terminal::snapshot(live.registry());

        let restored = Host::new((80, 24));
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
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
        live.spawn(cat(), "sh".to_owned(), 80, 24, None, None)
            .unwrap();
        live.spawn(cat(), "sh".to_owned(), 80, 24, None, None)
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
            .spawn(cat(), "sh".to_owned(), 80, 24, None, None)
            .unwrap();
        let snap = sprag_terminal::snapshot(live.registry());

        let restored = Host::new((80, 24));
        let n = restored
            .restore(
                snap,
                &std::collections::HashSet::new(),
                |_| None,
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
            .spawn(cat(), "sh".to_owned(), 80, 24, None, None)
            .unwrap();
        let b = live
            .spawn(cat(), "sh".to_owned(), 80, 24, None, None)
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
            .spawn(cat(), "sh".to_owned(), 80, 24, None, None)
            .unwrap();
        assert_eq!(
            next,
            PaneId(3),
            "the counter resumed above the restored ids"
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
                            remote: None,
                            cols: 80,
                            rows: 24,
                        },
                        PaneSnapshot {
                            id: PaneId(1),
                            cwd: None, // no recorded cwd -> falls back to the daemon's
                            command_label: "sh".to_owned(),
                            argv: vec!["sh".to_owned()],
                            remote: None,
                            cols: 80,
                            rows: 24,
                        },
                    ],
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
                        remote: None,
                        cols: 80,
                        rows: 24,
                    }],
                }],
            }],
        };

        let host = Host::new((80, 24));
        assert_eq!(
            host.restore(snap, &allow, |_| None, || None, |_| Vec::new())
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
                            remote: Some(SshRemote {
                                user: None,
                                host: "srv".to_owned(),
                                port: None,
                            }),
                            cols: 80,
                            rows: 24,
                        },
                        PaneSnapshot {
                            id: PaneId(1),
                            cwd: None,
                            command_label: "ssh".to_owned(),
                            // A shell that merely had `ssh` in its argv — NOT a sanctioned workspace.
                            argv: vec!["ssh".to_owned(), "host".to_owned(), "danger".to_owned()],
                            remote: None,
                            cols: 80,
                            rows: 24,
                        },
                    ],
                }],
            }],
        };

        let host = Host::new((80, 24));
        assert_eq!(
            host.restore(snap, &allow, |_| None, || None, |_| Vec::new())
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
            .spawn(cat(), "cat".to_owned(), 40, 6, None, None)
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
