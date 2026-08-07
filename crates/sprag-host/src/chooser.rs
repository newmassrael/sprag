//! The LIST a client puts in front of a person so they can go somewhere they cannot name — one
//! definition, two frontends (R315).
//!
//! tmux calls it `choose-tree`; the rival calls it the Navigator. sprag had neither, and what stood
//! in for it was [`Subject::SwitchTo`](crate::prompt::Subject::SwitchTo) — a prompt that asks a user
//! to TYPE a session name. That is the right gesture for somebody who knows the name and no gesture
//! at all for somebody who does not, which is why `switch-client -t` shipped BINDABLE AND UNBOUND:
//! tmux's key for this is `prefix s` and answering a chooser's key with a name prompt would be
//! answering a gesture with a different one.
//!
//! # A PICK IS AN IDENTITY
//!
//! Everything here follows from one sentence, and it is not this module's — it is R304's, written
//! when a `switch-client -l` was measured landing on a stranger that had taken a freed name:
//!
//! > a fact about the PRESENT can be kept true by a hook where the change is published; a fact
//! > about the PAST cannot, because its subject may no longer exist to be updated.
//!
//! A chooser's list is a fact about the past by construction. It is painted, and then a PERSON
//! reads it — that pause is the whole feature. So:
//!
//! * **What is committed is a [`Target`]**, a path of identities the daemon resolves again
//!   ([`crate::wire::AttachAsk::Goto`]). A picked NAME lands on whatever holds it now; a picked
//!   POSITION lands on whatever sits there now; a picked IDENTITY lands on that same thing or on
//!   NOTHING, and nothing is an answer.
//! * **The CURSOR is a [`Target`] too**, not an index into the rows. When the list changes under an
//!   open chooser — another client makes a session, a pane exits — the cursor stays on the thing
//!   the person was looking at. The rival's `NavigatorState::selected` is a `usize` into rows
//!   rebuilt on every render (herdr `9a4ce5e1`), so a session closing above the selection moves it
//!   silently onto a different row. This is the same defect one level in from the one their
//!   `NavigatorTarget::Workspace { ws_idx }` has, and it is why the rule is applied to both here.
//!
//! # What is REUSED rather than rebuilt
//!
//! The query is a [`Line`] — the grapheme-clustered editor R306 built for the name prompts, with
//! its `C-u` / `C-w` / `C-a` chords and its paste rule already decided and already tested. A
//! chooser's filter is a one-line text field, and a second one in this product would be a second
//! set of answers about what Backspace does to a Hangul syllable.
//!
//! # What is deliberately NOT here
//!
//! The SURFACE, on [`crate::prompt`]'s own split: `sprag-tui` paints an overlay and `sprag-gui`
//! paints a modal, and what must not differ — which rows exist, which one is picked, what a pick
//! MEANS — is decided here.

use sprag_terminal::{PaneId, SessionId, SplitDir, TreeSession, WindowId};

use crate::HostClient;
use crate::prompt::{Line, Typed};
use sprag_input::Modifiers;

/// What a chooser is FOR — what picking a row DOES, and which rows may be picked at all.
///
/// # Why the chooser had to learn this, and why it is one type rather than two views
///
/// Until R328 a chooser had exactly one errand and it was implicit: [`Pick::commit`] called
/// [`HostClient::goto`] and nothing else. That is why `move-pane` was one of the four verbs a
/// keystroke could mean and sprag did not bind — not because the surface was missing (the tree
/// chooser has shown every pane since R315) but because a pick could only ever mean *"go there"*.
///
/// Making it a value rather than a second chooser is what keeps the two honest about each other:
/// the rows, the filtering, the identity discipline and the live refresh are ONE implementation, and
/// what differs is stated here — in one closed set that [`Pick::commit`] matches exhaustively, so a
/// third errand cannot be added without deciding what it does AND what it may be done to.
///
/// # An errand decides which rows are PICKABLE, and that is half its value
///
/// A chooser opened to move a pane must not let a person commit a SESSION row: there is no act on
/// the other side of it. So [`accepts`](Self::accepts) gates the cursor as well as the commit —
/// non-pickable rows stay VISIBLE (a pane row means nothing without the session and window above
/// it) and the cursor steps over them. A surface cannot get this wrong by forgetting to check,
/// because the check is not on the surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Errand {
    /// GO to the row picked — tmux `choose-tree`, and every row is a place, so every row is
    /// pickable.
    Goto,
    /// Put the pane the key was pressed on BESIDE the pane picked — tmux `move-pane`.
    ///
    /// The moving pane travels in the errand because a keystroke can only ever mean the pane the
    /// person is on (`BoundAction`'s rule for every pane verb), and the TARGET is the thing a
    /// person picks — which is exactly the split that made this a chooser errand rather than a
    /// binding with a pane argument.
    MovePane {
        /// The pane that MOVES: the focused one, resolved when the key was pressed.
        pane: PaneId,
        /// The axis the target pane is divided on — tmux's `-h` (side by side) / `-v` (stacked).
        dir: SplitDir,
        /// tmux's `-b`: the near side of the target rather than the far one.
        before: bool,
    },
}

impl Errand {
    /// Whether `row` may be PICKED for this errand — the cursor lands only on rows this admits, and
    /// [`Pick::commit`] can only ever be reached from one.
    ///
    /// A pane cannot be moved beside ITSELF, and the daemon says so
    /// ([`PaneMoveError::SamePane`](sprag_terminal::PaneMoveError)); refusing the row is not a
    /// second authority on that rule but the surface declining to offer a press whose only possible
    /// answer is a refusal.
    ///
    /// **A target in ANOTHER session is admitted here and refused by the daemon.** The `move_pane`
    /// action resolves both panes inside the request's own scope, so a cross-session pick cannot be
    /// honoured — but the reason belongs to the daemon, which states it (R325), and a client that
    /// hid the rows would be deciding a scope question from a mirror. The rows are visible either
    /// way; what this refuses is only what is CERTAINLY not an act.
    #[must_use]
    pub const fn accepts(&self, row: &Row) -> bool {
        match (self, row.target) {
            (Self::Goto, _) => true,
            (Self::MovePane { pane, .. }, Target::Pane(_, _, target)) => target.0 != pane.0,
            (Self::MovePane { .. }, _) => false,
        }
    }

    /// What this chooser is asking, in words — the line a frontend paints above the list.
    ///
    /// Built HERE for [`Row`]'s stated reason one level up: two surfaces describing one question two
    /// ways is the drift these shared types exist to prevent, and a chooser that looked identical
    /// for two errands would be a list a person cannot act on. The DECORATION is still the
    /// surface's.
    /// It is the CANONICAL SPELLING, matching what `list-keys` prints and what a user could type
    /// back at `bind-key` — the discipline every other label in this product follows, so the word in
    /// front of a person is the word they would write. `choose-tree` for the errand that had no
    /// name because it was the only one.
    #[must_use]
    pub const fn asking(&self) -> &'static str {
        match self {
            Self::Goto => "choose-tree",
            Self::MovePane {
                dir: SplitDir::Horizontal,
                before: false,
                ..
            } => "move-pane -h",
            Self::MovePane {
                dir: SplitDir::Horizontal,
                before: true,
                ..
            } => "move-pane -h -b",
            Self::MovePane {
                dir: SplitDir::Vertical,
                before: false,
                ..
            } => "move-pane -v",
            Self::MovePane {
                dir: SplitDir::Vertical,
                before: true,
                ..
            } => "move-pane -v -b",
        }
    }
}

/// WHERE a picked row goes — a path of identities down the tree, exactly as far as the row is deep.
///
/// # Why the path and not just the leaf
///
/// A [`PaneId`] is registry-unique, so `Pane` could carry it alone and the daemon could find it.
/// It carries the whole path because the daemon checks the path WHOLE: a pick that resolves only
/// because its leaf happens to still exist somewhere else is a pick that landed on something the
/// person did not choose. It is also what lets the three arms be one grammar on the wire, with the
/// deeper members simply absent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Target {
    /// A session row — go there and look at whatever it is looking at.
    Session(SessionId),
    /// A window row — go to the session and make that window current.
    Window(SessionId, WindowId),
    /// A pane row — the above, and make that pane active.
    Pane(SessionId, WindowId, PaneId),
}

impl Target {
    /// The session every arm names.
    #[must_use]
    pub const fn session(self) -> SessionId {
        match self {
            Self::Session(session) | Self::Window(session, _) | Self::Pane(session, _, _) => {
                session
            }
        }
    }

    /// The window, for the two arms that have one.
    #[must_use]
    pub const fn window(self) -> Option<WindowId> {
        match self {
            Self::Session(_) => None,
            Self::Window(_, window) | Self::Pane(_, window, _) => Some(window),
        }
    }

    /// The pane, for the one arm that has one.
    #[must_use]
    pub const fn pane(self) -> Option<PaneId> {
        match self {
            Self::Session(_) | Self::Window(_, _) => None,
            Self::Pane(_, _, pane) => Some(pane),
        }
    }
}

/// One line of the chooser, ready to paint.
///
/// The strings are BUILT here rather than left to each frontend, for [`crate::prompt`]'s stated
/// reason: two surfaces describing one session two ways is the drift these shared types exist to
/// prevent. How a row is DECORATED — colour, the indent character, where the detail sits — is the
/// surface's, and none of that is here.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Row {
    /// Where picking it goes.
    pub target: Target,
    /// How deep it sits: `0` a session, `1` a window, `2` a pane. A number rather than a rendered
    /// indent, because how wide an indent is belongs to whoever is painting.
    pub depth: u8,
    /// What it is CALLED — a session or window name, a pane's name, or (for a pane nobody has
    /// named) its command label. The one place a label stands in for a name, and it is honest here
    /// where it would not be in an address: a row is text to read, not something to type.
    pub label: String,
    /// What else a person needs to tell this row from its neighbours — how many windows and panes,
    /// who else is viewing, what a pane is running.
    pub detail: String,
    /// Whether this row is WHERE THE CLIENT ALREADY IS. Derived from the tree's own `current` /
    /// `active` flags plus the client's session, so a frontend never computes it twice.
    pub here: bool,
}

/// An open chooser: every row, what has been typed to narrow them, and which one is picked.
///
/// Held by the frontend for as long as the list is up, refreshed from the daemon
/// ([`refresh`](Self::refresh)) so the list is LIVE while a person reads it.
///
/// Serde-derived for [`Line`]'s reason, and under its bound: `sprag-gui` holds the open chooser in
/// a reactive cell. It is not a wire type — what crosses the wire is the [`Target`] a pick commits.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Pick {
    /// Every row, unfiltered, in tree order.
    rows: Vec<Row>,
    /// What has been typed to narrow them — a [`Line`], so the editing is the prompts'.
    query: Line,
    /// The picked row, BY IDENTITY. See the module docs for why this is not a `usize`.
    cursor: Target,
    /// What this chooser is FOR — what a pick DOES and which rows it may be made on.
    errand: Errand,
}

impl Pick {
    /// Build a chooser over `tree`, with the cursor on the session the client is on.
    ///
    /// `here` is the client's own session NAME — the one fact the tree cannot carry, because a
    /// session has no idea who is looking at it (the same split [`sprag_terminal::SessionInfo`]
    /// states for its `attached` count). Everything else a row needs is in the tree.
    ///
    /// [`None`] for an empty tree: a chooser with nothing to choose is not a question, and opening
    /// one would take the keyboard to show a person an empty box. That cannot happen against a live
    /// daemon (the client asking is attached, and an attached session lists), which is exactly why
    /// it is an [`Option`] rather than an assertion.
    /// A chooser whose pick GOES to the row — the errand every chooser had before R328.
    #[must_use]
    pub fn new(tree: &[TreeSession], here: &str) -> Option<Self> {
        Self::for_errand(tree, here, Errand::Goto)
    }

    /// Build a chooser over `tree` for `errand`, with the cursor on the first row that errand can be
    /// done to.
    ///
    /// `here` is the client's own session NAME — the one fact the tree cannot carry, because a
    /// session has no idea who is looking at it (the same split [`sprag_terminal::SessionInfo`]
    /// states for its `attached` count). Everything else a row needs is in the tree.
    ///
    /// [`None`] for a tree with nothing this errand can be done TO, which is a strictly wider rule
    /// than the empty tree it replaces and is the same judgement: a chooser with nothing to choose
    /// is not a question, and opening one would take the keyboard to show a person a list they
    /// cannot act on. A `move-pane` errand in a session holding one pane is exactly that.
    #[must_use]
    pub fn for_errand(tree: &[TreeSession], here: &str, errand: Errand) -> Option<Self> {
        let rows = rows_of(tree, here);
        let cursor = rows
            .iter()
            .filter(|row| errand.accepts(row))
            // The session the client is on, so the list opens showing a person their own
            // surroundings. `find` over depth 0 rather than the first `here` row of any depth: a
            // chooser that opened on the ACTIVE PANE's row would put the cursor two levels in, and
            // the first thing a person does with a chooser is look UP the list. An errand that
            // admits no session row falls through to its own first pickable row.
            .find(|row| row.depth == 0 && row.here)
            .or_else(|| rows.iter().find(|row| errand.accepts(row)))?
            .target;
        Some(Self {
            rows,
            query: Line::new(""),
            cursor,
            errand,
        })
    }

    /// What this chooser is FOR — read by a frontend to paint the question above the list.
    #[must_use]
    pub const fn errand(&self) -> Errand {
        self.errand
    }

    /// Re-read the tree, keeping the cursor on the row the person is looking at.
    ///
    /// **The identity claim, applied to the client's own state.** The cursor survives anything that
    /// happens to the rest of the list; it moves only when its OWN row goes, and then to the next
    /// row that is still there — never silently onto a different session because one above it
    /// closed.
    pub fn refresh(&mut self, tree: &[TreeSession], here: &str) {
        // Where the cursor sits in the CURRENT visible order, before anything changes. It is the
        // fallback and only the fallback: if the picked row survives, this is not used at all.
        let was = self
            .pickable_rows()
            .position(|row| row.target == self.cursor)
            .unwrap_or(0);
        self.rows = rows_of(tree, here);
        if self.pickable_rows().any(|row| row.target == self.cursor) {
            return;
        }
        let visible: Vec<Target> = self.pickable_rows().map(|row| row.target).collect();
        // The row that took its PLACE, or the last one if the list got shorter. A person whose
        // pane exited under the cursor is left looking at what is now in that spot, which is what
        // a list does; what they are NOT left with is a cursor that moved while their row lived.
        if let Some(target) = visible.get(was).or_else(|| visible.last()) {
            self.cursor = *target;
        }
    }

    /// The rows to PAINT — those matching the query, with their ancestors and descendants.
    ///
    /// A row is shown when it matches, when an ANCESTOR matches (so narrowing to a session shows
    /// what is in it), or when a DESCENDANT matches (so narrowing to a pane's command still tells
    /// you which session it is in). A filter that showed only exact matches would answer *"where is
    /// this?"* with a row that cannot say.
    pub fn visible(&self) -> Vec<&Row> {
        self.visible_rows().collect()
    }

    /// The picked row's target — what [`commit`](Self::commit) will send.
    #[must_use]
    pub const fn cursor(&self) -> Target {
        self.cursor
    }

    /// Which VISIBLE row is picked, for a surface that has to scroll to it. [`None`] only when the
    /// query matches nothing, which is the one state with no picked row at all.
    #[must_use]
    pub fn cursor_at(&self) -> Option<usize> {
        self.visible_rows()
            .position(|row| row.target == self.cursor)
    }

    /// What has been typed to narrow the list.
    #[must_use]
    pub fn query(&self) -> &Line {
        &self.query
    }

    /// Feed one keystroke, in the wire's own `(name, mods)` spelling — the same pair
    /// [`Line::typed`] and [`Keymap::route`](crate::keymap::Keymap::route) take.
    ///
    /// The row keys are read FIRST and the rest goes to the query, which is what makes typing the
    /// obvious thing: every printable character narrows, and the two arrow keys that would
    /// otherwise be dead in a one-line field move the selection. `C-n` / `C-p` are there as well,
    /// because the fingers that reach for them in a shell's history are reaching for this.
    ///
    /// `ArrowLeft` / `ArrowRight` stay the QUERY's, so a typo in the middle of a filter is fixable
    /// — the editor's whole reason for existing, and it would be thrown away by spending those two
    /// keys on collapsing a tree this chooser does not fold.
    pub fn typed(&mut self, name: &str, mods: Modifiers) -> Typed {
        if mods.ctrl {
            match name {
                "n" => return self.step(1),
                "p" => return self.step(-1),
                _ => {}
            }
        } else {
            match name {
                "ArrowDown" => return self.step(1),
                "ArrowUp" => return self.step(-1),
                _ => {}
            }
        }
        let typed = self.query.typed(name, mods);
        if typed == Typed::Edited {
            // The filter just moved, so the picked row may no longer be in the list. Landing on the
            // first VISIBLE row is what makes type-then-Enter work: a person who types enough to
            // leave one row expects that row to be the one they get.
            let first = self
                .pickable_rows()
                .find(|row| row.target == self.cursor)
                .map_or_else(
                    || self.pickable_rows().next().map(|row| row.target),
                    |_| None,
                );
            if let Some(first) = first {
                self.cursor = first;
            }
        }
        typed
    }

    /// Carry the pick to the daemon, and report where the client LANDED.
    ///
    /// **The one implementation of what an answered chooser does**, called by both frontends —
    /// [`Subject::commit`](crate::prompt::Subject::commit)'s counterpart, and it reports the
    /// daemon's own answer for that method's stated reason: a session may have been renamed while
    /// the person was reading, and echoing the label off the row would tell them they are somewhere
    /// they are not.
    ///
    /// # Errors
    ///
    /// The sentence to paint. It has ONE cause, which is the whole value of committing an identity:
    /// the row named something that is no longer there.
    ///
    /// ⚠ **The SUCCESS carries nothing, and that is a decision the audit made rather than an
    /// oversight.** The first version answered `Ok("went to beta")` and both frontends discarded
    /// it — R314 registered exactly that shape one verb over (*"the landing nobody reads"*), and a
    /// round that reproduced it while citing it would be this project's own recorded failure. There
    /// is nowhere to paint a success: the chooser CLOSES on one, and a person who picked a row they
    /// were looking at has already been told where they went. The daemon's landed name is still
    /// answered by [`HostClient::goto`], which is where a status line would read it.
    /// EXHAUSTIVE over the errand, which is the whole reason [`Errand`] is a closed set: an errand
    /// added later cannot compile until somebody decides what committing it DOES.
    ///
    /// Both arms fail the same way and say the same thing, and that is not laziness — a pick is an
    /// IDENTITY, so the one cause either can have is that the row named something no longer there.
    /// A move can also be refused by the daemon for a reason of its own (the target is in another
    /// session, the pane is the window's only one); that sentence is the DAEMON's, it reaches the
    /// client through R325's funnel, and `sprag-gui`'s `message::preferred` gives it precedence over
    /// this one — so the generic word here never covers a stated reason.
    pub fn commit(&self, host: &dyn HostClient) -> Result<(), String> {
        let done = match self.errand {
            Errand::Goto => host.goto(self.cursor).map(drop),
            Errand::MovePane { pane, dir, before } => match self.cursor {
                Target::Pane(_, _, target) => host.move_pane(pane, target, dir, before).map(drop),
                // Unreachable through the cursor, which `Errand::accepts` keeps on pane rows — and
                // answered rather than asserted, because "the surface cannot produce this" is a
                // claim about two modules and a refusal costs nothing.
                Target::Session(_) | Target::Window(_, _) => None,
            },
        };
        done.ok_or_else(|| {
            let row = self
                .visible_rows()
                .find(|row| row.target == self.cursor)
                .map(|row| row.label.clone())
                .unwrap_or_default();
            format!("{row:?} is gone")
        })
    }

    /// Insert PASTED text into the query — [`Line::pasted`]'s rule, reached through the chooser so
    /// a paste narrows the list instead of landing in the shell behind it.
    ///
    /// It exists because a terminal delivers a paste as its OWN event, which is the leak R306 found
    /// on the name prompt and closed there. A surface that owns the keyboard owns the clipboard
    /// events too, and a new one gets that guarantee only by asking for it.
    pub fn pasted(&mut self, text: &str) -> Typed {
        let typed = self.query.pasted(text);
        if typed == Typed::Edited {
            let first = self
                .pickable_rows()
                .find(|row| row.target == self.cursor)
                .map_or_else(
                    || self.pickable_rows().next().map(|row| row.target),
                    |_| None,
                );
            if let Some(first) = first {
                self.cursor = first;
            }
        }
        typed
    }

    /// The visible rows this chooser's ERRAND can be done to — what the arrows walk and what a
    /// commit can land on.
    ///
    /// A subset of [`visible_rows`](Self::visible_rows) rather than a different list, because the
    /// two answer different questions and both are right: a `move-pane` chooser SHOWS its sessions
    /// and windows (a pane row means nothing without them) and lets the cursor stop only on panes.
    /// One place decides it, so the cursor, the commit and the type-then-Enter landing cannot come
    /// to admit different rows.
    fn pickable_rows(&self) -> impl Iterator<Item = &Row> {
        self.visible_rows().filter(|row| self.errand.accepts(row))
    }

    /// The visible rows, as an iterator — [`visible`](Self::visible)'s engine, shared with the
    /// cursor arithmetic so *what is on screen* and *what the arrows walk* cannot come apart.
    fn visible_rows(&self) -> impl Iterator<Item = &Row> {
        let query = self.query.text().to_lowercase();
        let matches = move |row: &Row| {
            query.is_empty()
                || row.label.to_lowercase().contains(&query)
                || row.detail.to_lowercase().contains(&query)
        };
        // A row's ANCESTORS are the nearest preceding rows of smaller depth, and its DESCENDANTS
        // are the following rows of greater depth up to the next row at its own — both readable
        // straight off a depth-ordered list, which is why the rows are flat with a depth rather
        // than nested. Computed once per call into a keep-set, so the whole filter is two passes.
        let mut keep = vec![false; self.rows.len()];
        for (at, row) in self.rows.iter().enumerate() {
            if !matches(row) {
                continue;
            }
            keep[at] = true;
            // Up: the chain of nearer-to-the-root rows above it.
            let mut depth = row.depth;
            for (above, ancestor) in self.rows[..at].iter().enumerate().rev() {
                if ancestor.depth < depth {
                    keep[above] = true;
                    depth = ancestor.depth;
                }
            }
            // Down: everything nested under it, until the tree comes back to its level.
            for (below, descendant) in self.rows.iter().enumerate().skip(at + 1) {
                if descendant.depth <= row.depth {
                    break;
                }
                keep[below] = true;
            }
        }
        self.rows
            .iter()
            .zip(keep)
            .filter_map(|(row, keep)| keep.then_some(row))
    }

    /// Move the cursor `by` rows through the VISIBLE list, stopping at the ends.
    ///
    /// It STOPS rather than wrapping, unlike the session ring `switch-client -n` walks, and the two
    /// are different for a reason this project has already written down once: a ring is walked
    /// blind and must not dead-end, while a LIST is being looked at and its ends are on the screen.
    /// [`WindowPlace::Step`](sprag_terminal::WindowPlace) draws the same line one level down.
    fn step(&mut self, by: isize) -> Typed {
        let visible: Vec<Target> = self.pickable_rows().map(|row| row.target).collect();
        let Some(at) = visible.iter().position(|target| *target == self.cursor) else {
            return Typed::Ignored;
        };
        let next = (at as isize + by).clamp(0, visible.len() as isize - 1) as usize;
        if next == at {
            return Typed::Ignored;
        }
        self.cursor = visible[next];
        Typed::Edited
    }
}

/// Flatten `tree` into rows — the ONE place a tree becomes a list, so both frontends and every test
/// see the same one.
///
/// `here` is the client's session name; a row is [`Row::here`] when it is that session, or the
/// current window of it, or the active pane of that window. Derived from the tree's own flags, so
/// "where I am" is one answer rather than three each surface computes.
fn rows_of(tree: &[TreeSession], here: &str) -> Vec<Row> {
    let mut rows = Vec::new();
    for session in tree {
        let on_it = session.name == here;
        rows.push(Row {
            target: Target::Session(session.id),
            depth: 0,
            label: session.name.clone(),
            detail: session_detail(session),
            here: on_it,
        });
        for window in &session.windows {
            rows.push(Row {
                target: Target::Window(session.id, window.id),
                depth: 1,
                label: window.name.clone(),
                detail: format!("{} pane{}", window.panes.len(), plural(window.panes.len())),
                here: on_it && window.current,
            });
            for pane in &window.panes {
                rows.push(Row {
                    target: Target::Pane(session.id, window.id, pane.id),
                    depth: 2,
                    // A pane's NAME when it has one, and its command label when it does not — the
                    // fallback `sprag_host::prompt`'s guarded kill already uses to name its
                    // subject, and for the same reason: an id is a number a person would have to go
                    // and look up.
                    label: pane.name.clone().unwrap_or_else(|| pane.command.clone()),
                    // The id is the DETAIL rather than the label, so a pane with neither a name nor
                    // a command still has something to tell it from its sibling.
                    detail: format!("pane {}", pane.id.0),
                    here: on_it && window.current && pane.active,
                });
            }
        }
    }
    rows
}

/// A session row's detail: its size, and who else is looking at it.
///
/// The viewer count is the column the rival cannot have — herdr is one process with no
/// display-client seam, so there is never anybody else viewing a workspace. It is omitted when it
/// is zero rather than printed as "0 viewing", because a row should carry facts a person acts on.
fn session_detail(session: &TreeSession) -> String {
    let panes: usize = session
        .windows
        .iter()
        .map(|window| window.panes.len())
        .sum();
    let mut detail = format!(
        "{} window{}, {panes} pane{}",
        session.windows.len(),
        plural(session.windows.len()),
        plural(panes),
    );
    if session.attached > 0 {
        detail.push_str(&format!(" · {} viewing", session.attached));
    }
    if session.default {
        detail.push_str(" · default");
    }
    detail
}

/// The `s` on a count — spelled once, because a row that says "1 windows" is a row somebody stops
/// trusting.
const fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::{TreePane, TreeWindow};

    /// A session of `windows` windows, each holding one pane, with ids counted off `from`.
    ///
    /// Built by hand rather than from a live [`Host`](crate::Host), and that is the point: what is
    /// under test here is what a chooser does with a tree, so the tree is the FIXTURE. Every test
    /// below that needs two things to disagree gets to make them disagree.
    fn session(id: u64, name: &str, windows: u64) -> TreeSession {
        TreeSession {
            id: SessionId(id),
            name: name.to_owned(),
            default: false,
            attached: 0,
            windows: (0..windows)
                .map(|w| TreeWindow {
                    id: WindowId(id * 100 + w),
                    name: format!("w{w}"),
                    current: w == 0,
                    panes: vec![TreePane {
                        id: PaneId(id * 1000 + w),
                        name: None,
                        command: "bash".to_owned(),
                        active: true,
                    }],
                })
                .collect(),
        }
    }

    /// Every key that reaches a chooser, as the wire spells it.
    fn key(pick: &mut Pick, name: &str) -> Typed {
        pick.typed(name, Modifiers::default())
    }

    /// **AN ERRAND DECIDES WHAT A PICK MEANS *AND* WHAT IT MAY BE MADE ON** — R328's whole thesis,
    /// and the reason `move-pane` stopped being a verb a keystroke could mean and sprag had not
    /// built.
    ///
    /// The two errands are driven over the SAME tree so nothing can be attributed to the fixture:
    /// what differs is the errand and only the errand.
    ///
    /// The `move-pane` half makes three claims a build could fail separately:
    ///
    /// 1. it OPENS ON A PANE ROW, not on the session row `Goto` opens on — a cursor two levels in
    ///    is right here and wrong there, and the same constructor decides both;
    /// 2. the arrows STEP OVER sessions and windows, so every position the cursor can reach is a
    ///    row the commit can act on. `Goto` over the identical tree walks all of them, which is what
    ///    makes this a claim about the errand rather than about the walk;
    /// 3. the MOVING PANE IS NOT OFFERED as its own target — the one row whose only possible answer
    ///    is a refusal.
    ///
    /// REVERT-PROOF: make `accepts` answer `true` for every row and claims 1-3 fail together; drop
    /// the `target != pane` term and claim 3 alone fails; point `step` back at `visible_rows` and
    /// claim 2 fails while 1 and 3 still pass.
    #[test]
    fn an_errand_confines_the_cursor_to_the_rows_it_can_act_on() {
        let tree = vec![session(1, "alpha", 2), session(2, "beta", 1)];
        let mover = PaneId(1000); // alpha's first pane — the one the key was pressed on.

        let goto = Pick::new(&tree, "alpha").expect("a tree with two sessions");
        assert!(
            matches!(goto.cursor(), Target::Session(_)),
            "a `choose-tree` opens where the person IS, which is their session's row",
        );

        let mut moving = Pick::for_errand(
            &tree,
            "alpha",
            Errand::MovePane {
                pane: mover,
                dir: SplitDir::Horizontal,
                before: false,
            },
        )
        .expect("a tree holding a pane other than the mover");
        assert!(
            matches!(moving.cursor(), Target::Pane(..)),
            "a `move-pane` opens on something it can be done TO: {:?}",
            moving.cursor(),
        );

        // WALK THE WHOLE LIST with the arrows and collect every position the cursor rests on.
        let mut landed = vec![moving.cursor()];
        while key(&mut moving, "ArrowDown") == Typed::Edited {
            landed.push(moving.cursor());
        }
        assert!(
            landed
                .iter()
                .all(|target| matches!(target, Target::Pane(..))),
            "every row the arrows can reach must be one a pick can act on: {landed:?}",
        );
        assert!(
            !landed.contains(&Target::Pane(SessionId(1), WindowId(100), mover)),
            "...and a pane cannot be moved beside itself, so it is not offered: {landed:?}",
        );
        // The CONTROL that makes the confinement mean something: the same walk under `Goto` reaches
        // rows of every depth, so the list really does hold what the errand is stepping over.
        let mut going = Pick::new(&tree, "alpha").expect("the same tree");
        let mut all = vec![going.cursor()];
        while key(&mut going, "ArrowDown") == Typed::Edited {
            all.push(going.cursor());
        }
        assert!(
            all.iter().any(|t| matches!(t, Target::Session(_)))
                && all.iter().any(|t| matches!(t, Target::Window(..)))
                && all.len() > landed.len(),
            "the tree holds sessions and windows the move errand stepped over: {all:?}",
        );
    }

    /// **THE CURSOR IS AN IDENTITY, AND A ROW THAT CLOSES ABOVE IT DOES NOT MOVE IT.**
    ///
    /// This is the round's thesis applied to the client's own state, and **the fixture is built so
    /// an INDEX and an IDENTITY land on different sessions rather than merely disagreeing**: the
    /// cursor sits on `gamma`'s row, which is row 8 of fourteen, and then `alpha` — five rows,
    /// entirely ABOVE it — ends. Position 8 of what is left still EXISTS; it is a pane of `delta`.
    /// So a `usize` cursor does not clamp or reset, it silently points at another session's pane,
    /// which is the failure this rule exists to remove and the one a shorter fixture could not tell
    /// from a clamp.
    ///
    /// That is not a hypothetical shape. It is what the rival's `NavigatorState::selected` is (a
    /// `usize` into rows rebuilt on every render, herdr `9a4ce5e1`), and a chooser is precisely the
    /// surface where it matters, because a person is reading the list while the daemon changes it.
    ///
    /// REVERT-PROOF: make `refresh` keep a POSITION instead of the target — replace the early
    /// return with `self.cursor = visible[was]` — and the first assertion fails naming a pane of
    /// `delta`, while every other test in this module still passes.
    #[test]
    fn a_row_closing_above_the_cursor_leaves_the_cursor_where_the_person_left_it() {
        let full = vec![
            session(1, "alpha", 2),
            session(2, "beta", 1),
            session(3, "gamma", 1),
            session(4, "delta", 1),
        ];
        let mut pick = Pick::new(&full, "beta").expect("a tree with four sessions in it");

        // Onto `gamma`: down from `beta`'s row, past its window and its pane.
        for _ in 0..3 {
            assert_eq!(key(&mut pick, "ArrowDown"), Typed::Edited);
        }
        assert_eq!(pick.cursor(), Target::Session(SessionId(3)));
        let was = pick.cursor_at().expect("the cursor is on a visible row");
        assert_eq!(was, 8, "...and its POSITION is eight rows down");

        // `alpha` ends — five rows above the cursor go with it.
        let shorter: Vec<TreeSession> = full.iter().skip(1).cloned().collect();
        pick.refresh(&shorter, "beta");

        assert_eq!(
            pick.cursor(),
            Target::Session(SessionId(3)),
            "the person is still looking at gamma",
        );
        assert_eq!(
            pick.cursor_at(),
            Some(3),
            "...at a different POSITION, which is exactly what an index cursor would have kept",
        );
        // THE CONTROL: position 8 is still a real row, and it belongs to ANOTHER SESSION. Without
        // this the test could not tell a cursor that followed its identity from one that clamped.
        let taken = pick.visible()[was].target;
        assert_eq!(
            taken.session(),
            SessionId(4),
            "row 8 of the shorter list is delta's, so the two rules had different answers to give",
        );
    }

    /// **A row that goes UNDER the cursor moves it, and to the row that took its place.**
    ///
    /// The other half of the same rule: the cursor follows its own identity while that identity
    /// exists, and only then falls back to a position. A chooser that simply reset to the top here
    /// would throw away a person's place every time a pane in another session exited.
    ///
    /// REVERT-PROOF: reset the cursor to the first row on any change and the second assertion
    /// fails, naming `alpha`.
    #[test]
    fn a_cursor_whose_own_row_goes_lands_on_what_took_its_place() {
        let full = vec![session(1, "alpha", 1), session(2, "beta", 1)];
        let mut pick = Pick::new(&full, "alpha").expect("two sessions");
        for _ in 0..3 {
            key(&mut pick, "ArrowDown");
        }
        assert_eq!(pick.cursor(), Target::Session(SessionId(2)));
        let was = pick.cursor_at().expect("visible");

        // `beta` itself ends. Its row is gone, so there is nothing to follow.
        pick.refresh(&full[..1], "alpha");
        assert_eq!(
            pick.cursor_at(),
            Some(pick.visible().len() - 1),
            "the cursor fell to the last surviving row rather than to the top",
        );
        assert_ne!(
            pick.cursor(),
            Target::Session(SessionId(1)),
            "and NOT to alpha's own row, which is where a reset-to-top would have put it",
        );
        assert!(was >= pick.visible().len(), "the fixture really did shrink");
    }

    /// **Typing narrows the list, and a match brings its ANCESTORS and its DESCENDANTS with it.**
    ///
    /// The ancestor half is what makes a filtered pane row usable — a row saying `vim` with no
    /// session above it cannot answer *where is this?* The descendant half is what makes typing a
    /// session name show what is in it.
    ///
    /// REVERT-PROOF: drop the ancestor walk and the `beta` row disappears from the first
    /// assertion; drop the descendant walk and the second returns one row instead of three.
    #[test]
    fn the_query_keeps_a_match_together_with_what_it_is_inside() {
        let mut tree = vec![session(1, "alpha", 1), session(2, "beta", 1)];
        tree[1].windows[0].panes[0].name = Some("deploy".to_owned());
        let mut pick = Pick::new(&tree, "alpha").expect("two sessions");
        assert_eq!(
            pick.visible().len(),
            6,
            "three rows per session, unfiltered"
        );

        for ch in "deploy".chars() {
            key(&mut pick, &ch.to_string());
        }
        let labels: Vec<&str> = pick
            .visible()
            .iter()
            .map(|row| row.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["beta", "w0", "deploy"],
            "the pane that matched, and the window and session it is INSIDE",
        );

        // ...and a SESSION match brings its subtree.
        let mut pick = Pick::new(&tree, "alpha").expect("two sessions");
        for ch in "beta".chars() {
            key(&mut pick, &ch.to_string());
        }
        assert_eq!(
            pick.visible().len(),
            3,
            "the session, its window and its pane",
        );
        assert_eq!(
            pick.cursor(),
            Target::Session(SessionId(2)),
            "and the cursor followed the filter onto the only thing left to pick",
        );
    }

    /// **The rows say where the person already IS, at every level they are.**
    ///
    /// Three flags rather than one, and each is a different fact: the session is the client's own
    /// (which the tree cannot know), the window is that session's current one, and the pane is that
    /// window's active one. A chooser that marked only the session would leave a person with four
    /// windows unable to tell which one they are looking at.
    ///
    /// REVERT-PROOF: drop the `on_it` conjunct from the window or pane arm and the LAST assertion
    /// fails — a window of the session the client is NOT on starts claiming to be here.
    #[test]
    fn the_rows_mark_the_session_window_and_pane_the_client_is_on() {
        let tree = vec![session(1, "alpha", 2), session(2, "beta", 2)];
        let pick = Pick::new(&tree, "beta").expect("two sessions");
        let here: Vec<(&str, u8)> = pick
            .visible()
            .iter()
            .filter(|row| row.here)
            .map(|row| (row.label.as_str(), row.depth))
            .collect();
        assert_eq!(
            here,
            vec![("beta", 0), ("w0", 1), ("bash", 2)],
            "the client's session, its current window, and that window's active pane",
        );
        assert_eq!(
            pick.cursor(),
            Target::Session(SessionId(2)),
            "and the list opens on the session the client is on, not on the first row",
        );
    }

    /// **A pane's row is its NAME when it has one and its COMMAND when it does not** — and the id
    /// is in the detail either way, so two `bash` panes are still two rows a person can tell apart.
    #[test]
    fn a_pane_row_falls_back_to_what_it_is_running_and_still_carries_its_id() {
        let mut tree = vec![session(1, "alpha", 2)];
        tree[0].windows[0].panes[0].name = Some("logs".to_owned());
        let pick = Pick::new(&tree, "alpha").expect("one session");
        let panes: Vec<(&str, &str)> = pick
            .visible()
            .iter()
            .filter(|row| row.depth == 2)
            .map(|row| (row.label.as_str(), row.detail.as_str()))
            .collect();
        assert_eq!(panes, vec![("logs", "pane 1000"), ("bash", "pane 1001")]);
    }

    /// **A session row says how big it is and who ELSE is looking at it.**
    ///
    /// The viewer count is the column the rival has no way to have: herdr is one process with no
    /// display-client seam, so nobody else is ever viewing a workspace (`9a4ce5e1`). It is omitted
    /// at zero rather than printed as "0 viewing", which is what keeps a row's detail a list of
    /// facts a person acts on.
    #[test]
    fn a_session_row_says_its_size_and_who_else_is_watching() {
        let mut tree = vec![session(1, "alpha", 2), session(2, "beta", 1)];
        tree[0].attached = 2;
        tree[1].default = true;
        let pick = Pick::new(&tree, "alpha").expect("two sessions");
        let details: Vec<&str> = pick
            .visible()
            .iter()
            .filter(|row| row.depth == 0)
            .map(|row| row.detail.as_str())
            .collect();
        assert_eq!(
            details,
            vec![
                "2 windows, 2 panes · 2 viewing",
                "1 window, 1 pane · default",
            ],
            "...and the counts are singular where they should be",
        );
    }

    /// **The arrows STOP at the ends of the list**, where `switch-client -n`'s ring wraps.
    ///
    /// Two different verbs over two different things: a ring is walked blind, and a LIST is being
    /// looked at with both its ends on the screen. `Ignored` is what says so — the surface repaints
    /// nothing, which is what a person pressing Down at the bottom expects to see.
    #[test]
    fn the_cursor_stops_at_the_ends_rather_than_wrapping() {
        let tree = vec![session(1, "alpha", 1), session(2, "beta", 1)];
        let mut pick = Pick::new(&tree, "alpha").expect("two sessions");
        assert_eq!(
            key(&mut pick, "ArrowUp"),
            Typed::Ignored,
            "already at the top"
        );
        for _ in 0..5 {
            key(&mut pick, "ArrowDown");
        }
        assert_eq!(pick.cursor_at(), Some(5), "at the last row");
        assert_eq!(
            key(&mut pick, "ArrowDown"),
            Typed::Ignored,
            "and it does not wrap round to the first",
        );
        // The shell's own chords reach the same movement, for the fingers that already have them.
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::default()
        };
        assert_eq!(pick.typed("p", ctrl), Typed::Edited);
        assert_eq!(pick.cursor_at(), Some(4));
        assert_eq!(pick.typed("n", ctrl), Typed::Edited);
        assert_eq!(pick.cursor_at(), Some(5));
    }

    /// **The query is a real editor**, and the two horizontal arrows stay ITS keys.
    ///
    /// A chooser that spent `ArrowLeft` on collapsing a tree would take away the fix-a-typo the
    /// shared editor exists for. The claim that discriminates is the INSERT IN THE MIDDLE, exactly
    /// as it is for the name prompt.
    #[test]
    fn the_query_edits_where_the_cursor_is_and_the_row_keys_do_not_take_its_arrows() {
        let tree = vec![session(1, "alpha", 1)];
        let mut pick = Pick::new(&tree, "alpha").expect("one session");
        for ch in "aph".chars() {
            key(&mut pick, &ch.to_string());
        }
        assert_eq!(pick.query().text(), "aph");
        assert_eq!(key(&mut pick, "ArrowLeft"), Typed::Edited);
        assert_eq!(key(&mut pick, "l"), Typed::Edited);
        assert_eq!(
            pick.query().text(),
            "aplh",
            "the horizontal arrows moved the TEXT cursor, not the row cursor",
        );
        assert_eq!(
            pick.typed(
                "u",
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            ),
            Typed::Edited,
        );
        assert_eq!(
            pick.query().text(),
            "h",
            "and `C-u` cut to the CURSOR, which is what it means at a shell prompt — a chooser \
             that had spent the arrows on the rows could not have left a cursor to cut to",
        );

        // A PASTE goes into the query too — the leak R306 closed on the name prompt, closed here
        // before it could be re-opened.
        assert_eq!(pick.pasted("alpha\nbeta"), Typed::Edited);
        assert_eq!(
            pick.query().text(),
            "alphah",
            "one line of it, never two — inserted at the cursor, where the cut above left it",
        );

        assert_eq!(key(&mut pick, "Enter"), Typed::Commit);
        assert_eq!(key(&mut pick, "Escape"), Typed::Cancel);
    }

    /// **An empty tree is not a question** — [`Pick::new`] answers [`None`] rather than opening a
    /// box with nothing in it.
    #[test]
    fn a_daemon_with_nothing_to_choose_from_opens_no_chooser() {
        assert!(Pick::new(&[], "alpha").is_none());
        assert!(
            Pick::new(&[session(1, "alpha", 1)], "somewhere else").is_some(),
            "...where a tree the client is not IN still opens, on its first row",
        );
    }

    /// **A pick carries the whole path down to the row's depth, and no further.**
    ///
    /// It is what the wire grammar's optional members are built from, and it is checked here rather
    /// than only at the wire because this is where the depth is decided.
    #[test]
    fn a_target_carries_exactly_as_much_of_the_path_as_its_row_is_deep() {
        let tree = vec![session(1, "alpha", 1)];
        let mut pick = Pick::new(&tree, "alpha").expect("one session");
        assert_eq!(pick.cursor().window(), None);
        assert_eq!(pick.cursor().pane(), None);

        key(&mut pick, "ArrowDown");
        assert_eq!(pick.cursor(), Target::Window(SessionId(1), WindowId(100)));
        assert_eq!(pick.cursor().session(), SessionId(1));
        assert_eq!(pick.cursor().pane(), None);

        key(&mut pick, "ArrowDown");
        assert_eq!(
            pick.cursor(),
            Target::Pane(SessionId(1), WindowId(100), PaneId(1000)),
        );
        assert_eq!(pick.cursor().window(), Some(WindowId(100)));
        assert_eq!(pick.cursor().pane(), Some(PaneId(1000)));
    }
}
