//! The QUESTION a client asks before it can carry out a keystroke — one definition, two frontends.
//!
//! Some verbs cannot be performed by a key alone. `rename-window` needs a string a keystroke does
//! not carry; `kill-window` needs the user to mean it. Until R306 sprag had neither: the rename
//! verbs were reachable from the CLI and the MCP surface only, and `kill-window` shipped BINDABLE
//! AND UNBOUND because tmux guards its own `prefix &` with a `confirm-before` this tree did not
//! have. One missing surface, two registered gaps.
//!
//! # The ask is a VALUE
//!
//! An [`Ask`] carries exactly what its own arm needs and nothing else, and closing the prompt drops
//! it. That is the whole design, and it is worth stating against the alternative because the rival
//! took it: herdr's prompt is a MODE (`Mode::RenameWorkspace` / `RenameTab` / `RenamePane`) plus
//! five sidecar fields on the whole application state — `name_input`,
//! `name_input_replace_on_type`, `creating_new_tab`, `requested_new_tab_name`,
//! `pending_workspace_create_cwd`, `rename_pane_target` — which four separate openers each set by
//! hand (`src/app/input/modal.rs`, herdr `9a4ce5e1`). One forgotten reset there is a rename that
//! lands on the wrong subject, and nothing in the types prevents it. Here there is nothing to
//! reset.
//!
//! # Three rules this project already paid for, and what each one decides
//!
//! 1. **The client never validates a name and never trims one.** The daemon owns the grammar
//!    ([`WindowName`] and its two siblings); the client sends what was
//!    typed and paints what came BACK. [`Subject::check`] is not an exception — it calls the
//!    daemon's own function so a mistake can be named while the user is still holding the keyboard,
//!    and it decides nothing: a name it lets through is still parsed at the other end.
//! 2. **The target is never a name read out of a mirror.** R304 measured a `switch-client -l`
//!    landing on an IMPOSTOR that had taken a freed name. So a window rename carries NO target (the
//!    daemon renames the scope's current window, resolved under its own lock at commit), a session
//!    rename carries none either (the scope IS the target, R303's attachment), and a pane rename
//!    carries the registry-unique [`PaneId`] — an identity, not an address.
//! 3. **The SEED is a mirror read, and that is safe because a seed is not an address.** A stale
//!    seed is a wrong default TEXT, visible on screen, that the user edits or replaces before
//!    pressing Enter. It cannot rename the wrong thing.
//!
//! # What is deliberately NOT shared
//!
//! The SURFACE. `sprag-tui` paints a one-row overlay and edits with [`Line`]; `sprag-gui` opens a
//! modal whose field is pinion's, with the mouse, the clipboard and an IME behind it. Forcing one
//! editor on both would take those away from the GUI to buy a uniformity no user experiences —
//! and it is the split [`crate::config`]'s catalog cousin already draws in the GUI: the thing that
//! must not differ between surfaces belongs to the command, the thing that must differ belongs to
//! the surface. What must not differ is here: which actions ask ([`Ask::of`]), what they ask, what
//! an answer MEANS, and what happens to it ([`Subject::commit`]).
//!
//! # Not modeled in SCE, on purpose
//!
//! The project reaches for SCE/SCXML before hand-rolling a state machine. Not here: this machine is
//! three states whose whole content is the PAYLOAD — a buffer, a cursor, a typed hole — which a
//! `datamodel="null"` chart cannot hold, and it joins [`PrefixMode`](crate::keymap::PrefixMode),
//! the routing machine already in this crate, which is a hand-written enum. A chart would model the
//! three names and leave every fact in Rust, splitting one definition across two files.

use sprag_input::Modifiers;
use sprag_terminal::{PaneId, PaneName, SessionName, WindowName};
use unicode_segmentation::UnicodeSegmentation;

use crate::HostClient;
use crate::chooser::Pick;
use crate::keymap::BoundAction;

/// What a client is asking, and what it will do with the answer.
///
/// Built by [`Ask::of`] from the action a key was bound to — never by a frontend, so neither of
/// them can decide on its own that a verb needs no question.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Ask {
    /// A LINE of text: the answer names something.
    Line {
        /// What the answer will name.
        subject: Subject,
        /// The text the editor starts on — the subject's current name, so the common edit (fix a
        /// typo, add a suffix) starts from what is there. tmux spells this `command-prompt -I`
        /// with a `#W` format; here it is READ at the moment the key is pressed, which is why
        /// sprag needs no format language to put the right name in front of the user.
        seed: String,
    },
    /// A LIST: the answer is a row a person PICKED, not a name they typed (R315).
    ///
    /// The third kind of question, and the one this vocabulary was missing: `rename-window` needs a
    /// string, `kill-window` needs a yes, and `choose-tree` needs *that one, there*. It is a
    /// separate arm rather than a [`Line`](Self::Line) whose subject happens to offer completions,
    /// because what comes back is not text at all — see [`Pick::commit`].
    Choose {
        /// The open chooser: every row, the query narrowing them, and the picked one.
        pick: Box<Pick>,
    },
    /// A YES/NO: the answer decides whether a destructive verb runs at all.
    Confirm {
        /// The question, naming exactly what is about to be destroyed — captured when the prompt is
        /// armed, because the user agrees to what they READ.
        question: String,
        /// The consequence the question does not already imply (killing a session's LAST window
        /// ends the session). [`None`] when the question says everything.
        ///
        /// A second field rather than a longer sentence, because the two are read differently: one
        /// names the target, the other names what else happens. `sprag-gui`'s catalog already
        /// splits its own prompts this way, and this ask feeds that same surface.
        consequence: Option<String>,
        /// The affirmative act in the imperative — the word a button wears, never a bare "OK". A
        /// property of the ACT and not of the surface, which is why it is decided here.
        verb: &'static str,
        /// What to do if they say yes. Performed by the FRONTEND, not here: carrying out a bound
        /// action is where the two clients legitimately differ (one repaints on the spot, the
        /// other lets the host's announcement carry it), and this module has no opinion about
        /// that. It only decides whether the verb runs.
        action: Box<BoundAction>,
    },
}

/// What a line ask will name.
///
/// One variant per LINE-asking verb, rather than a generic "verb with a hole" carrying a command
/// template. Three of the five are renames and two are not
/// ([`MoveBefore`](Self::MoveBefore) and [`SwitchTo`](Self::SwitchTo)): what the arms share is that
/// the answer is a NAME the daemon resolves, not that the act is a rename. The two that are not
/// renames are the two whose answer is an ADDRESS of something ELSE, and that is exactly the line
/// this vocabulary lets a binding leave blank — see [`BoundAction::SwitchClient`].
/// tmux's `command-prompt "rename-window '%%'"` substitutes the answer into TEXT and
/// re-parses it, which is why it has to quote; here the answer fills a TYPED slot, so an answer
/// beginning with `-` cannot become a flag. The wrong thing is unrepresentable rather than escaped.
///
/// Serde-derived because `sprag-gui` holds the armed subject in a reactive `Signal`, whose value
/// type carries pinion's serialization bound — the same reason that client's own `Choice` derives
/// it. It is a two-field enum over a [`PaneId`], so this costs nothing and keeps the GUI from having
/// to hold a stringly-typed copy of a decision this type already makes.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Subject {
    /// The session's CURRENT window — tmux `prefix ,`.
    Window,
    /// The session this client is attached to — tmux `prefix $`.
    Session,
    /// One pane, BY ID. The only subject that carries a target, and the only one that has an
    /// identity to carry: a [`PaneId`] is registry-unique and does not move.
    Pane(PaneId),
    /// The window the CURRENT window is to be moved in front of — tmux `prefix .` (R310).
    ///
    /// The one arm whose answer is not the new name of the thing being asked about: it is an
    /// ANCHOR, an address of a DIFFERENT window, and the act is a move. It shares this type
    /// because everything the surface needs is the same — a line of text, checked against
    /// [`WindowName`]'s grammar, committed through one function — and splitting it out would give
    /// both frontends a second prompt to route.
    MoveBefore,
    /// The SESSION this client is to be moved to — `switch-client -t` with the name left off
    /// (R314).
    ///
    /// [`MoveBefore`](Self::MoveBefore)'s sibling one level up: an ADDRESS the user supplies per
    /// press, not a new name for the subject. It is what this product has instead of tmux's
    /// `choose-tree` — a name rather than a list, which is why `prefix s` is deliberately left
    /// unbound (see [`BoundAction::SwitchClient`]).
    SwitchTo,
}

impl Subject {
    /// The question, in tmux's own shape: the verb in parentheses.
    ///
    /// Derived from the subject rather than configured, so it cannot describe a different verb
    /// than the one that will run. That is the whole reason `-p` is not in the binding grammar.
    #[must_use]
    pub fn question(self) -> &'static str {
        match self {
            Self::Window => "(rename-window)",
            Self::Session => "(rename-session)",
            Self::Pane(_) => "(rename-pane)",
            // tmux asks `(move-window)` here too, and then wants an INDEX. The verb is the same
            // and the vocabulary is not, which is the whole of R310 in one line of a prompt.
            Self::MoveBefore => "(move-window --before)",
            Self::SwitchTo => "(switch-client)",
        }
    }

    /// The name this subject has NOW, off the client's mirror — the editor's starting text.
    ///
    /// A mirror read, deliberately, and rule 3 in the module docs is why it is sound: a seed is
    /// text on a screen, not an address. A pane with no name seeds EMPTY rather than with its
    /// command label, because the label is not a name and offering it would invite a user to
    /// "keep" a name the pane does not have.
    #[must_use]
    pub fn seed(self, host: &dyn HostClient) -> String {
        match self {
            Self::Window => host
                .windows()
                .into_iter()
                .find(|window| window.current)
                .map(|window| window.name)
                .unwrap_or_default(),
            Self::Session => host.current_session(),
            Self::Pane(id) => host
                .pane_name(id)
                .map(|name| name.as_str().to_owned())
                .unwrap_or_default(),
            // EMPTY, and that is the honest seed rather than a missing feature. Every other arm
            // seeds with the subject's CURRENT name, because the common edit is to fix a typo in
            // it. There is no current answer to "which window shall this go in front of" — seeding
            // the neighbour would make the commonest press a no-op the user had to notice and
            // clear.
            //
            // The same holds one level up: seeding the session the client is ALREADY on would make
            // pressing Enter a switch to where it already is.
            Self::MoveBefore | Self::SwitchTo => String::new(),
        }
    }

    /// Check `answer` against the grammar the DAEMON will apply, rendering the rule it breaks.
    ///
    /// Not a second authority — each arm calls the same parse the daemon calls, and nothing here
    /// decides anything: an answer this lets through is parsed again at the other end and can still
    /// be refused. What it buys is the sentence. `InvokeError::Rejected` carries no payload while
    /// upstream PINION-PR82 is unlanded, so a refusal that crossed the wire could only ever be a
    /// disjunction of every rule; checking the grammar HERE leaves the wire refusal with one cause
    /// (the name is taken), which is one sentence a user can act on.
    ///
    /// # Errors
    ///
    /// The rendered rule, ready to paint under the prompt.
    pub fn check(self, answer: &str) -> Result<(), String> {
        match self {
            Self::Window => WindowName::parse(answer)
                .map(drop)
                .map_err(|e| e.to_string()),
            Self::Session => SessionName::parse(answer)
                .map(drop)
                .map_err(|e| e.to_string()),
            Self::Pane(_) => PaneName::parse(answer).map(drop).map_err(|e| e.to_string()),
            // The ANCHOR is a window name, so it is checked against a window's grammar — which
            // catches a malformed answer here and leaves the wire refusal with one cause (no
            // window is called that), exactly as the rename arms do.
            Self::MoveBefore => WindowName::parse(answer)
                .map(drop)
                .map_err(|e| e.to_string()),
            // A SESSION address, so a session's grammar — the same trade the anchor makes, one
            // level up: the refusal that survives to the wire has one cause (no session is called
            // that) rather than being a disjunction of every rule.
            Self::SwitchTo => SessionName::parse(answer)
                .map(drop)
                .map_err(|e| e.to_string()),
        }
    }

    /// Carry the answer to the daemon, and report the name it RECORDED.
    ///
    /// **The one implementation of what an answered prompt does**, called by both frontends. herdr
    /// writes this twice — `apply_rename_action` is `#[cfg(test)]` and mutates state directly,
    /// while production takes `save_rename_modal_via_api` — so their tests exercise a path their
    /// users never run (herdr `9a4ce5e1`, `src/app/input/modal.rs`). One function here, driven live
    /// by both clients' own tests.
    ///
    /// # Errors
    ///
    /// The sentence to paint when the daemon refused. Its causes are enumerated per arm rather
    /// than pooled, because [`check`](Self::check) has already removed the grammar from the list.
    pub fn commit(self, host: &dyn HostClient, answer: &str) -> Result<String, String> {
        match self {
            Self::Window => host
                .rename_window(answer)
                .ok_or_else(|| format!("a window is already called {answer:?}")),
            Self::Session => host
                .rename_session(answer)
                .ok_or_else(|| format!("a session is already called {answer:?}")),
            // TWO causes here where the others have one, and the second is real: a pane can exit
            // while its name is being typed. Said as a disjunction rather than guessed.
            Self::Pane(id) => host.rename_pane(id, answer).ok_or_else(|| {
                format!(
                    "a pane is already called {answer:?}, or pane {} is gone",
                    id.0
                )
            }),
            // The one arm that reports something other than a name: the daemon's own
            // [`PlaceHow`](sprag_terminal::PlaceHow) word, because three of its four outcomes leave
            // the order untouched and a user who pressed a key and saw nothing move is owed the
            // reason. `Itself` is reachable from here — a user can name the window they are on.
            Self::MoveBefore => {
                let place = sprag_terminal::WindowPlace::Before(answer.to_owned());
                match host.move_window(None, &place) {
                    None => Err(format!("no window is called {answer:?}")),
                    Some((window, how)) => Ok(match how {
                        sprag_terminal::PlaceHow::Moved => format!("moved {window}"),
                        sprag_terminal::PlaceHow::AlreadyThere => {
                            format!("{window} is already there")
                        }
                        sprag_terminal::PlaceHow::Alone => {
                            format!("{window} is this session's only window")
                        }
                        sprag_terminal::PlaceHow::Itself => {
                            format!("{window} cannot be anchored to itself")
                        }
                    }),
                }
            }
            // ONE cause, which is what `check` above buys: the grammar is already satisfied, so a
            // refusal here means the daemon has no session by that name. It reports the name the
            // daemon LANDED on rather than the one typed — a session may have been renamed while
            // the user was typing, and echoing the argument would tell them they are somewhere
            // they are not (R295's rule).
            Self::SwitchTo => host
                .switch_session_named(answer)
                .map(|landed| format!("switched to {landed}"))
                .ok_or_else(|| format!("no session is called {answer:?}")),
        }
    }
}

impl Ask {
    /// The question `action` has to ask before it can act, or [`None`] for one that acts at once.
    ///
    /// **The one place that decides which actions ask.** Both frontends route every bound action
    /// through here, so neither can perform a verb the other guards — the shape
    /// [`Routed::next`](crate::keymap::Routed::next) already uses for the prefix mode, and for the
    /// same reason: two clients with more ways to lose a keystroke than to route one cannot each
    /// hold half a rule.
    ///
    /// `pane` is the pane the key was pressed on, which is the only pane a keystroke can mean. An
    /// action needing one when there is none asks nothing and does nothing — a `rename-pane`
    /// pressed with the focus outside a pane has no subject, and inventing one would rename a pane
    /// the user was not looking at.
    #[must_use]
    pub fn of(action: &BoundAction, host: &dyn HostClient, pane: Option<PaneId>) -> Option<Self> {
        // NO SUBJECT, NO QUESTION — asked once here rather than per arm, and it SEES THROUGH THE
        // GUARD, which is the half a per-arm check kept getting wrong: `confirm-before kill-pane`
        // pressed with the focus outside a pane would otherwise ask "Kill this pane?", take the
        // user's yes, and kill nothing, because both frontends' `perform` needs the same pane this
        // question does not have.
        if action.needs_pane() && pane.is_none() {
            return None;
        }
        let line = |subject: Subject| Self::Line {
            seed: subject.seed(host),
            subject,
        };
        match action {
            BoundAction::RenameWindow => Some(line(Subject::Window)),
            BoundAction::RenameSession => Some(line(Subject::Session)),
            BoundAction::RenamePane => pane.map(|id| line(Subject::Pane(id))),
            BoundAction::MoveWindowBefore => Some(line(Subject::MoveBefore)),
            // Only the arm with no name in it asks; `switch-client -t work` acts at once, and
            // `-n`/`-p`/`-l` carry their whole content. `BoundAction::asks` decides that, and this
            // reads it through the pattern rather than restating the rule.
            BoundAction::SwitchClient {
                ask: crate::keymap::SwitchClientAsk::Ask,
            } => Some(line(Subject::SwitchTo)),
            // The tree is read HERE, at the moment the key is pressed, exactly as a seed is and for
            // the same reason: what the user reads is what was true when they asked. A chooser with
            // nothing in it asks NOTHING rather than opening an empty box — `RenamePane`'s rule
            // ("an action needing a subject when there is none asks nothing and does nothing"),
            // reached by a different route.
            BoundAction::ChooseTree => {
                Pick::new(&host.tree(), &host.current_session()).map(|pick| Self::Choose {
                    pick: Box::new(pick),
                })
            }
            BoundAction::ConfirmBefore { action } => {
                let (question, consequence, verb) = confirm_question(action, host, pane);
                Some(Self::Confirm {
                    question,
                    consequence,
                    verb,
                    action: action.clone(),
                })
            }
            _ => None,
        }
    }
}

/// The sentence a `confirm-before` shows, naming what is about to be destroyed.
///
/// Read off the mirror at ARM time and captured, which is [`crate::HostClient`]-shaped for the same
/// reason a seed is: it is text the user READS, and they agree to what it says. The GUI's own
/// destructive prompt captures its sentence the same way and states the same bound — a window that
/// became the session's last one while the prompt was up is still described as it was, and killing
/// it still ends the session.
///
/// tmux writes this into the binding (`confirm-before -p "kill-window #W? (y/n)"`), which needs a
/// format language to name the window and lets a config say something the verb does not do. Derived
/// here, it cannot.
fn confirm_question(
    action: &BoundAction,
    host: &dyn HostClient,
    pane: Option<PaneId>,
) -> (String, Option<String>, &'static str) {
    match action {
        // `kill-pane` is the one guarded verb whose blast radius its own NAME does not carry: a
        // window's last pane takes the window, and that window's session takes the session
        // (`Ended`). So the consequence line walks the same chain the daemon will, off the live
        // mirror, and says the FURTHEST thing that will happen — which is the fact a user needs
        // before answering, and the one tmux's `confirm-before -p "kill-pane #P? (y/n)"` cannot
        // state because its prompt is a fixed string in a config file.
        BoundAction::KillPane => (
            // It names the PROGRAM, not the id — the same sentence `sprag-gui`'s catalog has always
            // shown for the same act, word for word. Two surfaces asking one question two ways is
            // the exact thing this module exists to prevent (*"what must not differ is here: which
            // actions ask, WHAT THEY ASK"*), and the divergence was mine: the first version of this
            // arm asked `Kill pane 3?`, which tells a user a number they would have to go and look
            // up, where `Kill pane running 'vim'?` tells them what they are about to lose.
            //
            // The id is the FALLBACK rather than the answer, because a pane always has one and a
            // label can be empty; `Kill this pane?` is what is left when there is neither.
            pane.map_or_else(
                || "Kill this pane?".to_owned(),
                |id| match host.pane_command_label(id) {
                    label if label.is_empty() => format!("Kill pane {}?", id.0),
                    label => format!("Kill pane running '{label}'?"),
                },
            ),
            // `pane_ids` is the panes this client can RENDER — its own contract allows briefly
            // omitting one it cannot draw yet, so this can over-state the escalation and never
            // under-state it. That asymmetry is the right way round for a warning: a user told
            // "this ends your session" about a kill that only ends a pane loses nothing, and the
            // reverse loses their session. The GUI's catalog line reads the same fact the same way.
            match (host.pane_ids().len() <= 1, host.windows().len() <= 1) {
                (true, true) => Some(
                    "It is this window's last pane and this session's last window, so the session \
                     ends with it."
                        .to_owned(),
                ),
                (true, false) => {
                    Some("It is this window's last pane, so the window ends with it.".to_owned())
                }
                (false, _) => None,
            },
            "Kill",
        ),
        BoundAction::KillWindow => {
            let windows = host.windows();
            let current = windows
                .iter()
                .find(|window| window.current)
                .map(|window| window.name.clone())
                .unwrap_or_default();
            (
                format!("Kill window {current:?}?"),
                // The ESCALATION, said BEFORE it happens rather than discovered afterwards: a
                // session's last window takes the SESSION with it (`WindowKillOutcome::Session`),
                // and that is not something the word "window" implies.
                (windows.len() <= 1).then(|| {
                    "It is this session's last window, so the session ends with it.".to_owned()
                }),
                "Kill",
            )
        }
        // Every other action is describable by its own SPELLING, which is the vocabulary the user
        // wrote in their config — so the question quotes their own binding back at them rather than
        // inventing prose for a verb this function has not been taught. `Run` for the same reason:
        // a guard a user asked for on a verb this function does not know is still a guard, and
        // naming the act "OK" would be the one thing the catalog's own rule forbids.
        other => (format!("{other}?"), None, "Run"),
    }
}

/// What a keystroke did to a [`Line`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Typed {
    /// The text or the cursor moved — repaint the prompt.
    Edited,
    /// Nothing here understood it. Swallowed anyway: a prompt owns the keyboard while it is up, so
    /// an unhandled key must not fall through to the pane behind it.
    Ignored,
    /// `Enter` — take the answer.
    Commit,
    /// `Escape` (or `C-c` / `C-g`) — drop the whole ask.
    Cancel,
}

/// The one-line editor behind a [`Ask::Line`] — the surface for a client with no text widget.
///
/// # It moves by GRAPHEME CLUSTER
///
/// A Backspace over `한` composed from two codepoints, or over an emoji ZWJ sequence, takes the
/// whole cluster. Editing by codepoint would leave a user staring at half a syllable they cannot
/// type back, and this is a TERMINAL: CJK and emoji are the ordinary case, not the edge. The rival
/// edits by codepoint and cannot move at all — `insert_rename_input_text` is a `push_str` and
/// `delete_rename_input_char` is a `pop`, so their cursor is always at the end and a typo in the
/// middle of a name can only be backspaced to (herdr `9a4ce5e1`).
///
/// [`cursor`](Self::cursor) is where a client paints the caret, in the one unit that is not a
/// guess about somebody else's screen.
///
/// Serde-derived for [`Subject`]'s reason: `sprag-gui` holds an open question in a reactive cell,
/// whose value type carries pinion's serialization bound. **It is not a wire type and must not
/// become one** — [`cursor`](Self::cursor) is a byte offset this type's own methods keep on a
/// grapheme boundary, and a value decoded from somewhere else could arrive with one that is not.
/// Every value that round-trips through that cell was built by these methods in this process.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub struct Line {
    /// What has been typed.
    text: String,
    /// Where the cursor is, as a BYTE index into [`text`](Self::text) — always on a grapheme
    /// boundary, which every method here preserves and the tests pin.
    cursor: usize,
}

impl Line {
    /// An editor holding `seed`, with the cursor at the END of it.
    ///
    /// At the end rather than selecting the whole seed, which is the other reasonable choice and is
    /// what the rival does (`name_input_replace_on_type` clears the buffer on the first keystroke).
    /// The reason is that a seed here is the name the thing ALREADY HAS: the common edit is to
    /// amend it, and a first keystroke that silently destroyed it would make the seed a trap. A
    /// user who wants to start over presses `C-u`, which is one key and says so.
    #[must_use]
    pub fn new(seed: &str) -> Self {
        Self {
            cursor: seed.len(),
            text: seed.to_owned(),
        }
    }

    /// What has been typed so far.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Where the cursor is, as a byte offset into [`text`](Self::text) — always on a grapheme
    /// boundary.
    ///
    /// A byte offset and not a column count, deliberately. How wide a cluster is DEPENDS ON THE
    /// SURFACE — `sprag-tui` measures with its painter's own `unicode_column_width`, and the GUI's
    /// field is laid out by pinion — so a column number computed here would be a second opinion
    /// about somebody else's screen, right up until the two disagreed by one cell in front of the
    /// users this editor's clustering exists for. The buffer owns the TEXT and the OFFSET; the
    /// surface owns the WIDTH.
    #[must_use]
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Feed one keystroke, in the wire's own spelling — the same `(name, mods)` pair
    /// [`Keymap::route`](crate::keymap::Keymap::route) takes, which is what both frontends already
    /// hold by the time they ask.
    ///
    /// A printable key arrives as its own character, so the editor needs no second decoding of what
    /// a key means. Control chords are read as chords (`C-a`, `C-e`, `C-u`, `C-k`, `C-w`) — the
    /// readline vocabulary every shell user's fingers already carry, which is also what the pane
    /// behind this prompt would have done with them.
    ///
    /// **`C-c` and `C-g` cancel as well as `Escape`, and in a TERMINAL client that is not a
    /// convenience.** A lone `ESC` byte is the start of an escape SEQUENCE as far as any parser can
    /// tell, so a key typed straight after it arrives as `Alt+<that key>` rather than as two
    /// keystrokes — Escape cancels for a user who pauses, which is what cancelling looks like, but
    /// it is the one gesture here that depends on a timeout somebody else owns. The two chords have
    /// no such ambiguity, and a GUI's Escape has none either.
    pub fn typed(&mut self, name: &str, mods: Modifiers) -> Typed {
        if mods.ctrl {
            return match name {
                "a" => self.move_to(0),
                "e" => self.move_to(self.text.len()),
                "u" => self.cut_to_start(),
                "k" => self.cut_to_end(),
                "w" => self.delete_word(),
                // `C-c` and `C-g` cancel, which is what both of them mean at a shell prompt.
                "c" | "g" => Typed::Cancel,
                _ => Typed::Ignored,
            };
        }
        match name {
            "Enter" => Typed::Commit,
            "Escape" => Typed::Cancel,
            "Backspace" => self.delete_back(),
            "Delete" => self.delete_forward(),
            "ArrowLeft" => self.step(false),
            "ArrowRight" => self.step(true),
            "Home" => self.move_to(0),
            "End" => self.move_to(self.text.len()),
            // A printable key is spelled as itself. `Tab` and the rest of the named keys are not
            // insertable: a control character in a name is refused by every grammar this prompt
            // feeds, so accepting one here would only defer the refusal.
            _ if name.chars().count() == 1 && !name.chars().any(char::is_control) => {
                self.text.insert_str(self.cursor, name);
                self.cursor += name.len();
                Typed::Edited
            }
            _ => Typed::Ignored,
        }
    }

    /// Insert PASTED text at the cursor — one line of it, with the control characters dropped.
    ///
    /// A paste is not a run of keystrokes and must not be treated as one: a pasted newline at a
    /// shell prompt RUNS what is above it, and the same bytes here would commit a name the user has
    /// not read. So the first line is taken and the rest is discarded, which is what every
    /// single-line field does — and the controls go with it, since every grammar this prompt feeds
    /// refuses them anyway.
    ///
    /// It exists because a terminal delivers a paste as its OWN event: `sprag-tui` was forwarding
    /// that event to the focused PANE while the prompt held the keyboard, so a pasted name went
    /// into the shell behind the question. The keystroke path had been closed; this had not.
    pub fn pasted(&mut self, text: &str) -> Typed {
        let line: String = text
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        if line.is_empty() {
            return Typed::Ignored;
        }
        self.text.insert_str(self.cursor, &line);
        self.cursor += line.len();
        Typed::Edited
    }

    /// Put the cursor at byte offset `at` (a boundary the caller already knows is one).
    fn move_to(&mut self, at: usize) -> Typed {
        if self.cursor == at {
            return Typed::Ignored;
        }
        self.cursor = at;
        Typed::Edited
    }

    /// The byte range of the grapheme cluster before / after the cursor, or [`None`] at the end.
    fn neighbour(&self, forward: bool) -> Option<(usize, usize)> {
        if forward {
            self.text[self.cursor..]
                .grapheme_indices(true)
                .next()
                .map(|(offset, cluster)| {
                    (self.cursor + offset, self.cursor + offset + cluster.len())
                })
        } else {
            self.text[..self.cursor]
                .grapheme_indices(true)
                .next_back()
                .map(|(offset, cluster)| (offset, offset + cluster.len()))
        }
    }

    /// Move one cluster left or right.
    fn step(&mut self, forward: bool) -> Typed {
        match self.neighbour(forward) {
            Some((start, end)) => self.move_to(if forward { end } else { start }),
            None => Typed::Ignored,
        }
    }

    /// Delete the cluster before the cursor.
    fn delete_back(&mut self) -> Typed {
        match self.neighbour(false) {
            Some((start, end)) => {
                self.text.replace_range(start..end, "");
                self.cursor = start;
                Typed::Edited
            }
            None => Typed::Ignored,
        }
    }

    /// Delete the cluster after the cursor — the cursor does not move.
    fn delete_forward(&mut self) -> Typed {
        match self.neighbour(true) {
            Some((start, end)) => {
                self.text.replace_range(start..end, "");
                Typed::Edited
            }
            None => Typed::Ignored,
        }
    }

    /// `C-u`: everything before the cursor.
    fn cut_to_start(&mut self) -> Typed {
        if self.cursor == 0 {
            return Typed::Ignored;
        }
        self.text.replace_range(..self.cursor, "");
        self.cursor = 0;
        Typed::Edited
    }

    /// `C-k`: everything after it.
    fn cut_to_end(&mut self) -> Typed {
        if self.cursor == self.text.len() {
            return Typed::Ignored;
        }
        self.text.truncate(self.cursor);
        Typed::Edited
    }

    /// `C-w`: the word before the cursor, plus the whitespace it was sitting on.
    ///
    /// Whitespace-delimited, which is `C-w`'s own meaning at a shell prompt rather than an editor's
    /// word class — the rival splits alphanumerics from separators here, which is a different (and
    /// also defensible) verb, but it is not the one this key means to the fingers pressing it.
    fn delete_word(&mut self) -> Typed {
        let head = &self.text[..self.cursor];
        let trimmed = head.trim_end();
        let start = trimmed.rfind(char::is_whitespace).map_or(0, |at| {
            at + head[at..].chars().next().map_or(1, char::len_utf8)
        });
        if start == self.cursor {
            return Typed::Ignored;
        }
        self.text.replace_range(start..self.cursor, "");
        self.cursor = start;
        Typed::Edited
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Type `name` with no modifiers.
    fn key(line: &mut Line, name: &str) -> Typed {
        line.typed(name, Modifiers::default())
    }

    /// Type a control chord.
    fn ctrl(line: &mut Line, name: &str) -> Typed {
        line.typed(
            name,
            Modifiers {
                ctrl: true,
                ..Modifiers::default()
            },
        )
    }

    /// **`switch-client -t` with the name LEFT OFF asks which session; every other arm acts at
    /// once.**
    ///
    /// The one arm R314 added that needs a surface, and nothing drove it until this existed.
    ///
    /// Driven against a real [`Host`](crate::Host), like the kill test below and for its reason:
    /// what is checked here is the SHAPE of the question and which actions ask at all, and a double
    /// would be re-stating the decision [`Ask::of`] is supposed to make. What an ANSWER does is a
    /// different claim and is driven end to end instead — `sprag-tui`'s
    /// `a_bound_switch_client_asks_which_session_and_the_answer_moves_the_client`, on a real
    /// pseudoterminal against a real daemon, where a landing can actually be observed.
    ///
    /// REVERT-PROOF: give the ASKING arm a seed and the empty-seed assertion fails; make any
    /// carried arm ask and the last loop fails, because a binding that names its target must act.
    #[test]
    fn switch_client_with_no_name_asks_for_one_and_every_other_arm_acts() {
        use crate::keymap::SwitchClientAsk;
        let host = crate::Host::new((40, 6));
        let asking = BoundAction::SwitchClient {
            ask: SwitchClientAsk::Ask,
        };
        let Some(Ask::Line { subject, seed }) = Ask::of(&asking, &host, None) else {
            panic!("`switch-client -t` with no name must open a line ask");
        };
        assert_eq!(subject, Subject::SwitchTo);
        assert_eq!(
            seed, "",
            "EMPTY: seeding the session the client is already on would make Enter a no-op",
        );
        assert_eq!(subject.question(), "(switch-client)");
        // The grammar is the DAEMON's; this calls the same parse so the sentence can arrive while
        // the user is still holding the keyboard.
        assert!(subject.check("work").is_ok());
        assert!(
            subject.check("").is_err(),
            "an empty session name is refused by the same parse the daemon applies",
        );
        // THE CONTROL, and the whole reason `-t` may be left blank at all: an arm that CARRIES its
        // target acts at once, so a config that fixed a name does not stop to ask for one. Written
        // as a loop over the other three, because the claim is about all of them.
        for carried in [
            SwitchClientAsk::Named("work".to_owned()),
            SwitchClientAsk::LastViewed,
            SwitchClientAsk::Step(sprag_terminal::OrderStep::Next),
        ] {
            let action = BoundAction::SwitchClient {
                ask: carried.clone(),
            };
            assert!(
                Ask::of(&action, &host, None).is_none(),
                "{carried:?} carries its whole content and must not ask",
            );
            assert!(
                !action.asks(),
                "...and `asks` agrees, which is what stops a confirm-before wrapping it",
            );
        }
    }

    /// **The guarded kill names the escalation it actually has**, read off the live arrangement at
    /// the moment the key is pressed — R309.
    ///
    /// This is what a binding cannot do. tmux writes the question into the config
    /// (`confirm-before -p "kill-pane #P? (y/n)"`), so its prompt says the same words whether the
    /// pane has ten siblings or is the last thing keeping the user's session alive. Here the
    /// sentence is DERIVED, so the one press that ends a session says so.
    ///
    /// Driven against a real [`Host`](crate::Host) rather than a double: the fixture IS the claim
    /// (how many panes, how many windows), and a double would be re-stating the arrangement this
    /// function is supposed to read.
    ///
    /// REVERT-PROOF: drop the `pane_ids` condition and the two-pane case grows a consequence; drop
    /// the `windows` condition and the last-pane case stops mentioning the session — the same pair
    /// `sprag-gui`'s catalog line is proved with, because they are two readings of one chain.
    #[test]
    fn a_guarded_pane_kill_states_only_the_escalation_it_actually_has() {
        let guarded = BoundAction::ConfirmBefore {
            action: Box::new(BoundAction::KillPane),
        };
        let consequence =
            |host: &crate::Host, pane: PaneId| match Ask::of(&guarded, host, Some(pane)) {
                Some(Ask::Confirm { consequence, .. }) => consequence,
                other => panic!("a guarded kill asks a yes/no: {other:?}"),
            };

        // TWO PANES: nothing else goes, whatever the window count.
        let host = crate::Host::new((40, 6));
        let first = host.new_pane().expect("a shell is born");
        host.new_pane().expect("and a second");
        assert_eq!(
            consequence(&host, first),
            None,
            "a pane with a sibling takes nothing else down with it",
        );
        // ...AND THE QUESTION NAMES THE PROGRAM, which is the same sentence `sprag-gui`'s catalog
        // shows for this act. Two surfaces asking one question two ways is what this module exists
        // to prevent, and it is checked here rather than assumed: the id is a number a user would
        // have to go and look up.
        let question = match Ask::of(&guarded, &host, Some(first)) {
            Some(Ask::Confirm { question, .. }) => question,
            other => panic!("a guarded kill asks a yes/no: {other:?}"),
        };
        let label = host.pane_command_label(first);
        assert!(!label.is_empty(), "the fixture has a label to name");
        assert_eq!(question, format!("Kill pane running '{label}'?"));

        // ONE PANE, TWO WINDOWS: the window goes and the session does not.
        let host = crate::Host::new((40, 6));
        let alone = host.new_pane().expect("a shell is born");
        host.new_window();
        // `new_window` selects the window it made, so come back to the one holding the pane.
        host.select_window("0");
        let said = consequence(&host, alone).expect("the escalation is said");
        assert!(
            said.contains("last pane"),
            "it names the pane escalation: {said}"
        );
        assert!(
            !said.contains("session"),
            "and does NOT claim the session ends while another window survives: {said}",
        );

        // ONE PANE, ONE WINDOW: both escalations, in one sentence.
        let host = crate::Host::new((40, 6));
        let only = host.new_pane().expect("a shell is born");
        let said = consequence(&host, only).expect("the escalation is said");
        assert!(
            said.contains("last pane") && said.contains("last window") && said.contains("session"),
            "the two escalations compose and name what actually ends: {said}",
        );
    }

    /// A guarded kill with the focus OUTSIDE a pane asks NOTHING.
    ///
    /// The hole this closes is specific: both frontends need a [`PaneId`] to perform
    /// `kill-pane`, so without one the question would be answered and nothing would happen — a
    /// prompt that takes a user's "yes" and drops it. `RenamePane` has always refused this; the
    /// GUARD had to be taught to see through itself to the verb, which is what
    /// [`BoundAction::needs_pane`] does.
    ///
    /// REVERT-PROOF: make `needs_pane` answer `false` for the wrapper and this asks anyway.
    #[test]
    fn a_guarded_pane_kill_with_no_pane_focused_asks_nothing() {
        let host = crate::Host::new((40, 6));
        host.new_pane()
            .expect("a pane exists, so this is not vacuous");
        let guarded = BoundAction::ConfirmBefore {
            action: Box::new(BoundAction::KillPane),
        };
        assert_eq!(Ask::of(&guarded, &host, None), None);
        assert_eq!(
            Ask::of(&BoundAction::RenamePane, &host, None),
            None,
            "the rename it borrows the rule from behaves the same",
        );
        // THE CONTROL: a verb that needs no pane still asks with the focus outside one, so the gate
        // above is about the SUBJECT and not about refusing every keystroke.
        assert!(
            Ask::of(&BoundAction::RenameWindow, &host, None).is_some(),
            "a window rename has its subject without a pane",
        );
    }

    /// The cursor is a real cursor: text goes in where it is, and it moves both ways.
    ///
    /// The discriminator against an append-only buffer (which is what the rival has) is the INSERT
    /// IN THE MIDDLE — an editor that ignored the arrows would still pass every "typing appends"
    /// assertion.
    #[test]
    fn the_editor_inserts_where_the_cursor_is_rather_than_at_the_end() {
        let mut line = Line::new("main");
        assert_eq!(line.text(), "main");
        assert_eq!(
            line.cursor(),
            4,
            "the seed starts with the cursor at its end"
        );

        assert_eq!(key(&mut line, "ArrowLeft"), Typed::Edited);
        assert_eq!(key(&mut line, "x"), Typed::Edited);
        assert_eq!(line.text(), "maixn");
        assert_eq!(line.cursor(), 4);

        assert_eq!(key(&mut line, "Backspace"), Typed::Edited);
        assert_eq!(
            line.text(),
            "main",
            "backspace takes what is BEFORE the cursor"
        );
        assert_eq!(key(&mut line, "Delete"), Typed::Edited);
        assert_eq!(line.text(), "mai", "and delete takes what is after it");

        assert_eq!(key(&mut line, "Home"), Typed::Edited);
        assert_eq!(line.cursor(), 0);
        assert_eq!(
            key(&mut line, "ArrowLeft"),
            Typed::Ignored,
            "there is nothing before the start, and it does not wrap",
        );
        assert_eq!(key(&mut line, "End"), Typed::Edited);
        assert_eq!(key(&mut line, "Delete"), Typed::Ignored);
    }

    /// Editing is by GRAPHEME CLUSTER — the fact a codepoint editor gets wrong, and the one a
    /// terminal whose users type CJK and emoji cannot get wrong.
    #[test]
    fn a_cluster_is_one_step_however_many_codepoints_it_holds() {
        // A decomposed Hangul syllable: two codepoints, ONE thing on screen.
        let mut line = Line::new("\u{1100}\u{1161}");
        assert_eq!(line.text().chars().count(), 2, "two codepoints,");
        assert_eq!(key(&mut line, "Backspace"), Typed::Edited);
        assert_eq!(
            line.text(),
            "",
            "and one Backspace takes the whole cluster — by codepoint this leaves a half-syllable",
        );

        // A step is a CLUSTER, which for Hangul is three bytes and one glyph.
        let mut line = Line::new("한글");
        assert_eq!(line.cursor(), 6, "two syllables, three bytes each");
        assert_eq!(key(&mut line, "ArrowLeft"), Typed::Edited);
        assert_eq!(line.cursor(), 3, "and one step back is one whole syllable");
        assert_eq!(key(&mut line, "a"), Typed::Edited);
        assert_eq!(line.text(), "한a글");

        // A family emoji is one cluster of many codepoints.
        let mut line = Line::new("x\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}");
        assert_eq!(key(&mut line, "Backspace"), Typed::Edited);
        assert_eq!(
            line.text(),
            "x",
            "the whole ZWJ sequence, not one link of it"
        );
    }

    /// The readline chords, and the two keys that end the ask.
    #[test]
    fn the_shell_chords_do_what_a_shell_user_expects_and_escape_drops_the_ask() {
        let mut line = Line::new("release build");
        assert_eq!(ctrl(&mut line, "w"), Typed::Edited);
        assert_eq!(
            line.text(),
            "release ",
            "C-w takes the word before the cursor"
        );
        assert_eq!(ctrl(&mut line, "w"), Typed::Edited);
        assert_eq!(line.text(), "", "including the space it was sitting on");
        assert_eq!(ctrl(&mut line, "w"), Typed::Ignored);

        let mut line = Line::new("main");
        assert_eq!(ctrl(&mut line, "a"), Typed::Edited);
        assert_eq!(line.cursor(), 0);
        assert_eq!(ctrl(&mut line, "k"), Typed::Edited);
        assert_eq!(line.text(), "", "C-k from the start clears the line");

        let mut line = Line::new("main");
        assert_eq!(ctrl(&mut line, "u"), Typed::Edited);
        assert_eq!(line.text(), "", "C-u is how a user replaces the seed");

        let mut line = Line::new("x");
        assert_eq!(key(&mut line, "Enter"), Typed::Commit);
        assert_eq!(key(&mut line, "Escape"), Typed::Cancel);
        assert_eq!(ctrl(&mut line, "c"), Typed::Cancel);
        assert_eq!(
            key(&mut line, "F5"),
            Typed::Ignored,
            "a named key is not text — and Ignored is still SWALLOWED by the caller",
        );
        assert_eq!(line.text(), "x", "none of which typed anything");
    }

    /// A PASTE is one line, with the controls dropped — never a commit.
    ///
    /// The newline is the assertion that matters: the same bytes at the shell prompt behind this
    /// one would RUN what was pasted, and a prompt that treated a pasted `\n` as `Enter` would
    /// commit a name the user has not read yet.
    #[test]
    fn a_paste_is_one_line_of_text_and_never_an_answer() {
        let mut line = Line::new("build");
        assert_eq!(line.pasted("-x"), Typed::Edited);
        assert_eq!(line.text(), "build-x");

        let mut line = Line::new("");
        assert_eq!(line.pasted("one\ntwo\nthree"), Typed::Edited);
        assert_eq!(line.text(), "one", "the tail is dropped, not run");

        let mut line = Line::new("");
        assert_eq!(line.pasted("a\u{1b}[31mb\tc"), Typed::Edited);
        assert_eq!(line.text(), "a[31mbc", "the controls go, the text stays");

        let mut line = Line::new("keep");
        assert_eq!(line.pasted("\n"), Typed::Ignored);
        assert_eq!(
            line.text(),
            "keep",
            "and a paste with nothing in it types nothing"
        );
    }

    /// The grammar check is the daemon's own, one rule at a time — the sentence a user acts on.
    #[test]
    fn a_refusal_names_the_rule_the_daemon_would_have_applied() {
        assert_eq!(
            Subject::Window.check("  "),
            Err("a window name cannot be blank".to_owned()),
        );
        assert!(
            Subject::Session
                .check("a\nb")
                .is_err_and(|why| why.contains("control characters")),
        );
        assert!(
            Subject::Pane(PaneId(1))
                .check("7")
                .is_err_and(|why| why.contains("number")),
            "a pane name is refused where a window's is not: a pane has an ordinal to collide with",
        );
        assert!(Subject::Window.check("7").is_ok());
    }
}
