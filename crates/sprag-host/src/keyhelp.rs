//! What the keys do, as a value — one answer, three surfaces.
//!
//! Until R308 the question *"what can I press?"* had no answer inside sprag at all. Neither
//! frontend showed a binding: a user had to leave the terminal they were in and run
//! `sprag list-keys` in a shell, which is the one moment they cannot, and the single place in the
//! product that displayed a chord — the GUI palette's hint column — held five hand-typed strings
//! whose own doc said *"there is no chord table to derive them from, so a renamed binding must be
//! renamed here too."* Three consecutive rounds added keys (R305's window ring, R306's three
//! renames, R307's eight `resize-pane` chords) and every one of them widened the hole.
//!
//! # The view is DERIVED, and that is the whole design
//!
//! Every row here comes out of a keymap in force, through the two authorities that already exist:
//! [`BoundAction`]'s [`Display`](std::fmt::Display) impl — the canonical spelling `list-keys` prints
//! and [`BoundAction::parse`] reads back — and [`BoundAction::VOCABULARY`], *"the ONE enumeration"*
//! of what a binding may name. Nothing in this module writes down the name of an action, a flag or
//! a key. **A binding added to the vocabulary appears here the day it exists, and one that is
//! renamed is renamed here, because there is no second copy to update.**
//!
//! That is the property to hold against the rival, and the rival is the argument for it: herdr
//! (`9a4ce5e1`) has the help surface sprag lacked — `src/ui/keybind_help.rs`, a modal bound to
//! `kb.help` — and it is a HAND-WRITTEN list of `help_entry(keybind_label(&kb.<field>), "label")`
//! calls. Their `Keybinds` struct carries 47 bindable actions; that file names **43**. The four
//! `swap_pane_left` / `right` / `up` / `down` bindings are in the config, are bound by default, and
//! **a herdr user pressing their help key is never told they exist** (verified by diffing the
//! struct's fields against every `kb.` reference in the file). A second list is a list that drifts;
//! this module is what having no second list looks like.
//!
//! # What it answers, and why that is two questions
//!
//! 1. **What do my keys do?** Every binding in force, grouped by [`ActionSubject`] and rendered as
//!    the chord a user actually presses ([`Keymap::chord`]).
//! 2. **What else could I bind?** The whole of [`BoundAction::VOCABULARY`], each form marked when
//!    no key reaches its verb. herdr can answer the first for the actions somebody remembered to
//!    list, and cannot answer the second at all — their vocabulary is a struct, so "everything you
//!    could bind" exists only as fields nobody enumerates.
//!
//! # What is shared and what is not
//!
//! [`prompt`](crate::prompt)'s split, applied again: what must not differ between surfaces belongs
//! here — which rows, in what ORDER, with what TEXT, and how wide the chord column is. How they are
//! painted does not, and is each frontend's. [`Scroll`] sits on this side of that line even though
//! scrolling looks like a surface concern: a view whose `End` key lands on the last ROW in one
//! frontend and the last PAGE in the other is two answers to one question, and the arithmetic is
//! the same in both.

use std::fmt;

use serde::{Deserialize, Serialize};
use sprag_input::Modifiers;

use crate::keymap::{ActionSubject, BoundAction, Keymap, verb_of};

/// What one keystroke does to an open help view.
///
/// Two answers and no third, because there is no third thing a key may do here: the view owns the
/// keyboard while it is up, so a key it does not recognise is SWALLOWED rather than passed on. That
/// is R306's rule on the prompt applied to the surface beside it — a keystroke aimed at a reader's
/// own screen must never turn up in the shell behind it — and it is why an unrecognised key comes
/// back as [`Open`](Self::Open) with the position unchanged rather than as a variant of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Pressed {
    /// Still up, at this position.
    Open(Scroll),
    /// The reader is done; give the panes back.
    Closed,
}

/// One line of the view.
///
/// A row rather than a pre-formatted string because a surface lays out its own columns — but every
/// row's TEXT is decided here, headings included, so the two frontends cannot come to say different
/// things about the same table.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Row {
    /// A section heading, in the words the view uses.
    Heading(String),
    /// A blank line between sections.
    Blank,
    /// A key in force and what it does.
    Bind {
        /// What to press — [`Keymap::chord`], so a rebound prefix shows the user's own key.
        chord: String,
        /// The canonical spelling, which is also what a user could type back at `bind-key`.
        action: String,
        /// tmux's `-r`: this one repeats while the prefix table stays armed.
        repeat: bool,
    },
    /// One form a binding may name, and whether any key reaches its verb.
    Vocabulary {
        /// The form as [`BoundAction::VOCABULARY`] spells it, flag grammar and all.
        form: String,
        /// Whether some binding in force names this verb.
        bound: bool,
    },
}

/// The whole view: every row, in reading order, plus what the surfaces need to align it.
///
/// Built by [`KeyHelp::of`] from a keymap and never mutated — a help view is a photograph of a
/// table, and a client that re-read the config while one was open would be showing rows from two
/// different files.
///
/// # Why THIS has serde where the vocabulary deliberately does not
///
/// `sprag-gui` holds what it is showing in a reactive `Signal`, which must serialize — the same
/// constraint that made [`crate::keymap::BoundAction`] reach that client as a STRING, because it
/// carries types from two crates with no serde and no reason to have any. Nothing of that applies
/// here: a rendered view is `String`s, `bool`s and a `usize`, it is DATA about a table rather than
/// the table itself, and a round trip through it cannot lose an action's identity because it never
/// held one. So the frontend keeps the value instead of a spelling it has to parse back.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct KeyHelp {
    rows: Vec<Row>,
    chord_width: usize,
}

impl KeyHelp {
    /// The word a surface puts against a form no key reaches.
    ///
    /// Shared for one word because it is the answer to a question, not decoration: two surfaces
    /// disagreeing about whether the mark means *unbound* or *unavailable* would be two answers.
    pub const UNBOUND: &'static str = "unbound";

    /// The heading over the vocabulary section.
    pub const VOCABULARY_HEADING: &'static str = "actions a binding can name";

    /// tmux's mark for a binding that repeats while the prefix table stays armed.
    ///
    /// Shared for two characters for [`UNBOUND`](Self::UNBOUND)'s reason: three surfaces print it,
    /// and a reader who learns what it means on one must not meet a different spelling on another.
    /// It is tmux's own `-r` so a tmux user learns nothing new.
    pub const REPEAT: &'static str = "-r";

    /// Render `keymap` as a view.
    #[must_use]
    pub fn of(keymap: &Keymap) -> Self {
        let mut rows = vec![Row::Heading(format!("prefix {}", keymap.prefix()))];
        // Grouped by subject, and WITHIN a group in the keymap's own order rather than sorted:
        // that order is the default table's, which was written to be read (the splits together,
        // the arrows in tmux's `Up Down Left Right`), and sorting by chord would scatter the four
        // resize keys across the alphabet. A user's own additions land after the defaults they
        // extend, which is where they wrote them.
        for subject in ActionSubject::ALL {
            let group: Vec<&crate::keymap::Bind> = keymap
                .binds()
                .filter(|bind| bind.action().subject() == subject)
                .collect();
            // A subject nothing is bound to gets no heading at all. An empty section under a
            // heading reads as a bug in the view rather than as a fact about the table, and the
            // fact is already carried — precisely — by the vocabulary section below.
            if group.is_empty() {
                continue;
            }
            rows.push(Row::Blank);
            rows.push(Row::Heading(subject.heading().to_owned()));
            for bind in group {
                rows.push(Row::Bind {
                    chord: keymap.chord(bind),
                    action: bind.action().to_string(),
                    repeat: bind.repeats(),
                });
            }
        }
        rows.push(Row::Blank);
        rows.push(Row::Heading(Self::VOCABULARY_HEADING.to_owned()));
        // The join is on the VERB, cut the same way on both sides by `verb_of` — `resize-pane -L 5`
        // in the table meets `resize-pane -L|-R|-U|-D [N]` in the vocabulary. Verb-level and not
        // form-level on purpose: a user who has bound one direction has found the verb, and the
        // form beside it is what tells them the other three flags exist.
        //
        // Through `reaches` rather than `verb`, because `prefix &` is `confirm-before kill-window`
        // and reaches BOTH: the first version of this counted outer verbs and reported `kill-window`
        // as bound to nothing, which is a lie about a key three lines above it in the same view.
        let bound: Vec<String> = keymap
            .binds()
            .flat_map(|bind| bind.action().reaches())
            .collect();
        for form in BoundAction::VOCABULARY {
            rows.push(Row::Vocabulary {
                form: form.to_owned(),
                bound: bound.iter().any(|verb| verb == verb_of(form)),
            });
        }
        let chord_width = rows
            .iter()
            .filter_map(|row| match row {
                Row::Bind { chord, .. } => Some(chord.chars().count()),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        Self { rows, chord_width }
    }

    /// Every row, in reading order.
    pub fn rows(&self) -> impl ExactSizeIterator<Item = &Row> {
        self.rows.iter()
    }

    /// How many rows there are — what a surface sizes its scroll against.
    #[must_use]
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether there is nothing to show. Never true for a real keymap, which always has a prefix
    /// heading and a vocabulary; kept because `len` without it is a clippy lint and a caller
    /// holding a view has no other way to ask.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// What `name`+`mods` does to this view, shown `viewport` rows at a time.
    ///
    /// # Why the GESTURE is shared and the painting is not
    ///
    /// [`prompt`](crate::prompt)'s line applied again, and this side of it is bigger than it looks:
    /// two frontends that each decided what `PageDown` meant would drift the moment one of them
    /// gained a key. The vocabulary is deliberately small and entirely borrowed — arrows and
    /// `j`/`k`, `PageUp`/`PageDown` with less's `b`/`f` and a spacebar, `Home`/`End`, and `q` /
    /// `Escape` / `Enter` to leave — so that nothing here is a gesture a reader has to be taught.
    ///
    /// # The modifier rule, and the shipped defect it comes from
    ///
    /// `Shift` is IGNORED on a character key, because R306 measured `prefix %` doing nothing in
    /// `sprag-gui` on a real keyboard: winit reports `Shift+5` as `"%"` WITH the shift flag and a
    /// pty reports the same character with none, so an exact comparison matched only one of them.
    /// `Q` must close this view on both. `Ctrl` is read, and only for the two cancels a terminal
    /// user already has in their fingers.
    #[must_use]
    pub fn pressed(&self, scroll: Scroll, name: &str, mods: Modifiers, viewport: usize) -> Pressed {
        let len = self.len();
        let plain = !mods.ctrl && !mods.alt && !mods.sup;
        if mods.ctrl && matches!(name, "c" | "C" | "g" | "G") {
            return Pressed::Closed;
        }
        if !plain {
            return Pressed::Open(scroll);
        }
        match name {
            "Escape" | "Enter" | "q" | "Q" => Pressed::Closed,
            "ArrowUp" | "k" | "K" => Pressed::Open(scroll.by(-1, len, viewport)),
            "ArrowDown" | "j" | "J" => Pressed::Open(scroll.by(1, len, viewport)),
            "PageUp" | "b" | "B" => Pressed::Open(scroll.page(-1, len, viewport)),
            // The spacebar reaches this under both of its spellings: a character key from a
            // terminal, and the `code`-style name an IME and a config file use
            // ([`sprag_input::NAMED_KEYS`]).
            "PageDown" | "Space" | " " | "f" | "F" => Pressed::Open(scroll.page(1, len, viewport)),
            "Home" => Pressed::Open(Scroll::home()),
            "End" => Pressed::Open(Scroll::end(len, viewport)),
            _ => Pressed::Open(scroll),
        }
    }

    /// The widest chord, in CHARACTERS, so the actions line up as a column.
    ///
    /// Measured here rather than by each surface because the two would then align the same table to
    /// different widths — and characters rather than bytes for `list-keys`' own reason: a chord is a
    /// user's string and `%` is not the only thing anyone binds.
    #[must_use]
    pub fn chord_width(&self) -> usize {
        self.chord_width
    }
}

/// Where a viewport sits in a list longer than itself.
///
/// Shared arithmetic, for the reason the module docs give: `End` must mean the same thing in both
/// frontends. Everything is clamped on the way out rather than on the way in, so a viewport that
/// changes size — a window resize while the view is open — cannot leave an offset stranded past the
/// end of a list it no longer fits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Scroll {
    offset: usize,
}

impl Scroll {
    /// The first row to paint, given how many rows there are and how many fit.
    ///
    /// Clamped so the last screenful is always full: scrolling to the end of a 40-row list in a
    /// 10-row viewport shows rows 30..40, never row 39 alone above nine blanks.
    #[must_use]
    pub fn offset(self, len: usize, viewport: usize) -> usize {
        self.offset.min(len.saturating_sub(viewport))
    }

    /// Move by `delta` rows, up for a negative one.
    #[must_use]
    pub fn by(self, delta: isize, len: usize, viewport: usize) -> Self {
        let from = self.offset(len, viewport);
        let offset = if delta < 0 {
            from.saturating_sub(delta.unsigned_abs())
        } else {
            from.saturating_add(delta.unsigned_abs())
        };
        Self {
            offset: offset.min(len.saturating_sub(viewport)),
        }
    }

    /// Move by one screenful, up for a negative one.
    ///
    /// A screenful is the viewport LESS ONE ROW, which is what every pager does: the line that was
    /// at the bottom is at the top afterwards, so a reader has an anchor and nothing can fall
    /// between two pages.
    #[must_use]
    pub fn page(self, pages: isize, len: usize, viewport: usize) -> Self {
        let step = isize::try_from(viewport.saturating_sub(1).max(1)).unwrap_or(isize::MAX);
        self.by(pages.saturating_mul(step), len, viewport)
    }

    /// Back to the first row.
    #[must_use]
    pub fn home() -> Self {
        Self { offset: 0 }
    }

    /// To the last screenful.
    #[must_use]
    pub fn end(len: usize, viewport: usize) -> Self {
        Self {
            offset: len.saturating_sub(viewport),
        }
    }

    /// Whether there is anything above the viewport — what a surface draws a scroll mark from.
    #[must_use]
    pub fn more_above(self, len: usize, viewport: usize) -> bool {
        self.offset(len, viewport) > 0
    }

    /// Whether there is anything below it.
    #[must_use]
    pub fn more_below(self, len: usize, viewport: usize) -> bool {
        self.offset(len, viewport) + viewport < len
    }
}

impl fmt::Display for Row {
    /// One row as a single line, for a surface with no columns to give it — the CLI's notes form.
    ///
    /// Not what the frontends use: they have a chord column and align it with
    /// [`KeyHelp::chord_width`]. This is the fallback shape, and it exists so that a row's text is
    /// still decided in one place when there is only one column to put it in.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Heading(text) => f.write_str(text),
            Self::Blank => Ok(()),
            Self::Bind {
                chord,
                action,
                repeat,
            } => {
                let mark = if *repeat {
                    format!("{} ", KeyHelp::REPEAT)
                } else {
                    String::new()
                };
                write!(f, "{chord}  {mark}{action}")
            }
            Self::Vocabulary { form, bound } => {
                if *bound {
                    f.write_str(form)
                } else {
                    write!(f, "{form}  ({})", KeyHelp::UNBOUND)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::KeyTable;

    /// Every form the vocabulary names gets a row, and none is invented.
    ///
    /// The join this module is built on runs the other way round — binds looked up in the
    /// vocabulary — so this is the assertion that the SECOND question ("what else could I bind?")
    /// is answered completely rather than for the verbs that happen to be bound.
    #[test]
    fn the_view_offers_every_form_a_binding_can_name() {
        let view = KeyHelp::of(&Keymap::default());
        let offered: Vec<&str> = view
            .rows()
            .filter_map(|row| match row {
                Row::Vocabulary { form, .. } => Some(form.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(offered, BoundAction::VOCABULARY.to_vec());
    }

    /// Every binding in force reaches the view — none is filtered out by the grouping.
    ///
    /// The failure this catches is the one herdr HAS: a vocabulary that grows past the surface that
    /// lists it. There a new field is simply absent from `keybind_help.rs` and nothing complains;
    /// here a subject with no group, or a group dropped by the filter, moves this count.
    #[test]
    fn every_binding_in_force_is_shown() {
        let keymap = Keymap::default();
        let view = KeyHelp::of(&keymap);
        let shown = view
            .rows()
            .filter(|row| matches!(row, Row::Bind { .. }))
            .count();
        assert_eq!(shown, keymap.binds().count());
        assert!(
            shown >= 29,
            "the default table shrank unexpectedly: {shown}"
        );
    }

    /// The default table reaches every verb the vocabulary has — and the mark MOVES when one stops
    /// being bound.
    ///
    /// Two assertions in one test on purpose. The first is the claim (sprag ships a complete
    /// vocabulary); the second is its CONTROL, because a `bound` flag that was hardwired to `true`
    /// would satisfy the first and say nothing. Unbinding `?` is what tells them apart.
    #[test]
    fn the_default_table_binds_every_verb_and_the_mark_moves_when_one_stops() {
        let unbound = |keymap: &Keymap| -> Vec<String> {
            KeyHelp::of(keymap)
                .rows()
                .filter_map(|row| match row {
                    Row::Vocabulary { form, bound: false } => Some(form.clone()),
                    _ => None,
                })
                .collect()
        };
        let mut keymap = Keymap::default();
        assert_eq!(unbound(&keymap), Vec::<String>::new());
        keymap.unbind(KeyTable::Prefix, "?").expect("? is bound");
        assert_eq!(unbound(&keymap), vec!["list-keys".to_owned()]);
        // The GUARD's own control, and the reason `BoundAction::reaches` exists. There are TWO
        // guarded keys now (`prefix &` is `confirm-before kill-window`, `prefix x` is
        // `confirm-before kill-pane`), and taking them one at a time is a sharper control than the
        // single unbind was: dropping the first must take its INNER verb out of reach while
        // leaving the WRAPPER reachable by the other, which no `bound` flag computed from outer
        // verbs alone could produce.
        keymap.unbind(KeyTable::Prefix, "&").expect("& is bound");
        assert_eq!(
            unbound(&keymap),
            vec!["list-keys".to_owned(), "kill-window".to_owned()],
            "the inner verb goes; `confirm-before` stays reachable through `prefix x`"
        );
        keymap.unbind(KeyTable::Prefix, "x").expect("x is bound");
        assert_eq!(
            unbound(&keymap),
            vec![
                "list-keys".to_owned(),
                "kill-pane".to_owned(),
                "kill-window".to_owned(),
                "confirm-before <action>".to_owned(),
            ],
            "and with the last guard gone the wrapper itself is out of reach"
        );
    }

    /// A chord shows the user's OWN prefix, not the word `prefix` and not the default key — and a
    /// ROOT binding shows no prefix, because a user presses none.
    ///
    /// ⚠ The first assertion read *every* chord until R314, which was only an assertion at all
    /// while the root table shipped EMPTY. It is split now: the prefix rows are exactly the
    /// defaults minus the two session chords, and those two are named as the reason. Written this
    /// way it still fails if `chord` stopped consulting the prefix in force, and it no longer fails
    /// for a root binding being added.
    #[test]
    fn a_rebound_prefix_moves_every_chord() {
        let mut keymap = Keymap::default();
        keymap.set_prefix("C-a").expect("C-a is a key");
        let chords: Vec<String> = KeyHelp::of(&keymap)
            .rows()
            .filter_map(|row| match row {
                Row::Bind { chord, .. } => Some(chord.clone()),
                _ => None,
            })
            .collect();
        let (rooted, prefixed): (Vec<&String>, Vec<&String>) =
            chords.iter().partition(|chord| !chord.contains(' '));
        assert!(
            prefixed.iter().all(|chord| chord.starts_with("C-a ")),
            "a prefix binding must be shown under the prefix in force: {prefixed:?}"
        );
        assert_eq!(
            rooted,
            vec!["C-S-PageDown", "C-S-PageUp"],
            "and the ROOT bindings carry no prefix, because a user presses none",
        );
        assert!(
            chords.contains(&"C-a ?".to_owned()),
            "the key that opens this view must appear in it: {chords:?}"
        );
    }

    /// A ROOT binding is shown as the bare key, because that is what a user presses.
    #[test]
    fn a_root_binding_carries_no_prefix() {
        let mut keymap = Keymap::default();
        keymap
            .bind(KeyTable::Root, "F1", "list-keys", false)
            .expect("F1 takes a binding");
        let chords: Vec<String> = KeyHelp::of(&keymap)
            .rows()
            .filter_map(|row| match row {
                Row::Bind { chord, .. } => Some(chord.clone()),
                _ => None,
            })
            .collect();
        assert!(chords.contains(&"F1".to_owned()), "{chords:?}");
    }

    /// The guarded kill is filed with the WINDOW keys, not with the client's.
    ///
    /// `confirm-before kill-window` is the only binding whose subject is not its own first word, so
    /// it is the one that says whether the recursion in `BoundAction::subject` is wired up.
    #[test]
    fn a_guarded_action_is_grouped_by_what_it_guards() {
        let view = KeyHelp::of(&Keymap::default());
        let mut heading = None;
        for row in view.rows() {
            match row {
                Row::Heading(text) => heading = Some(text.clone()),
                Row::Bind { action, .. } if action == "confirm-before kill-window" => {
                    assert_eq!(heading.as_deref(), Some("window"));
                    return;
                }
                _ => {}
            }
        }
        panic!("the guarded kill is not in the view at all");
    }

    /// Groups appear in the declared reading order and each appears once.
    #[test]
    fn the_groups_read_in_the_declared_order() {
        let view = KeyHelp::of(&Keymap::default());
        let headings: Vec<String> = view
            .rows()
            .filter_map(|row| match row {
                Row::Heading(text) => Some(text.clone()),
                _ => None,
            })
            .collect();
        let subjects: Vec<String> = headings
            .iter()
            .filter(|text| {
                ActionSubject::ALL
                    .iter()
                    .any(|s| s.heading() == text.as_str())
            })
            .cloned()
            .collect();
        let expected: Vec<String> = ActionSubject::ALL
            .iter()
            .map(|subject| subject.heading().to_owned())
            .collect();
        assert_eq!(subjects, expected);
    }

    /// The chord column is as wide as the widest chord and no wider.
    #[test]
    fn the_chord_column_fits_the_widest_chord() {
        let view = KeyHelp::of(&Keymap::default());
        let widest = view
            .rows()
            .filter_map(|row| match row {
                Row::Bind { chord, .. } => Some(chord.chars().count()),
                _ => None,
            })
            .max()
            .expect("the default table binds something");
        assert_eq!(view.chord_width(), widest);
    }

    /// A row renders its own text, including the mark on a form nothing reaches.
    #[test]
    fn a_row_says_what_it_is() {
        assert_eq!(
            Row::Bind {
                chord: "C-b Left".to_owned(),
                action: "resize-pane -L 1".to_owned(),
                repeat: true,
            }
            .to_string(),
            "C-b Left  -r resize-pane -L 1"
        );
        assert_eq!(
            Row::Vocabulary {
                form: "list-keys".to_owned(),
                bound: false,
            }
            .to_string(),
            "list-keys  (unbound)"
        );
        assert_eq!(
            Row::Vocabulary {
                form: "list-keys".to_owned(),
                bound: true,
            }
            .to_string(),
            "list-keys"
        );
    }

    /// The last screenful is full, and `End` and scrolling past the end agree about where it is.
    #[test]
    fn the_end_of_a_scroll_is_a_full_screenful() {
        let (len, viewport) = (40, 10);
        assert_eq!(Scroll::end(len, viewport).offset(len, viewport), 30);
        assert_eq!(
            Scroll::default()
                .by(9_999, len, viewport)
                .offset(len, viewport),
            30
        );
        assert_eq!(Scroll::home().offset(len, viewport), 0);
    }

    /// A page overlaps the previous one by one row, so nothing falls between two pages.
    #[test]
    fn a_page_keeps_one_row_of_context() {
        let (len, viewport) = (40, 10);
        let down = Scroll::default().page(1, len, viewport);
        assert_eq!(down.offset(len, viewport), 9);
        assert_eq!(down.page(-1, len, viewport).offset(len, viewport), 0);
    }

    /// A viewport bigger than the list never scrolls, however hard it is asked to.
    #[test]
    fn a_short_list_does_not_scroll() {
        let (len, viewport) = (3, 10);
        assert_eq!(Scroll::end(len, viewport).offset(len, viewport), 0);
        assert_eq!(
            Scroll::default()
                .page(5, len, viewport)
                .offset(len, viewport),
            0
        );
        assert!(!Scroll::default().more_below(len, viewport));
        assert!(!Scroll::default().more_above(len, viewport));
    }

    /// The scroll marks say what is off screen in each direction.
    #[test]
    fn the_marks_report_what_is_off_screen() {
        let (len, viewport) = (40, 10);
        let top = Scroll::home();
        assert!(!top.more_above(len, viewport));
        assert!(top.more_below(len, viewport));
        let bottom = Scroll::end(len, viewport);
        assert!(bottom.more_above(len, viewport));
        assert!(!bottom.more_below(len, viewport));
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use sprag_input::Modifiers;

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        sup: false,
    };

    fn view() -> KeyHelp {
        KeyHelp::of(&Keymap::default())
    }

    #[test]
    fn the_leaving_keys_all_leave() {
        let view = view();
        for name in ["Escape", "Enter", "q", "Q"] {
            assert_eq!(
                view.pressed(Scroll::default(), name, NONE, 10),
                Pressed::Closed,
                "{name} must close the view",
            );
        }
        let ctrl = Modifiers { ctrl: true, ..NONE };
        for name in ["c", "g"] {
            assert_eq!(
                view.pressed(Scroll::default(), name, ctrl, 10),
                Pressed::Closed,
                "C-{name} must close the view",
            );
        }
    }

    /// `Shift` on a character is the character, which is the shipped defect R306 measured.
    ///
    /// REVERT-PROOF: make the modifier test exact (`mods == NONE`) and this fails on `Q`, which is
    /// exactly how `prefix %` reached users broken in `sprag-gui` and worked in a pty.
    #[test]
    fn a_shifted_letter_is_still_that_letter() {
        let shift = Modifiers {
            shift: true,
            ..NONE
        };
        assert_eq!(
            view().pressed(Scroll::default(), "Q", shift, 10),
            Pressed::Closed
        );
    }

    /// A key nobody bound is SWALLOWED at the position it was pressed, never passed on and never a
    /// close.
    #[test]
    fn an_unknown_key_changes_nothing_and_closes_nothing() {
        let view = view();
        let somewhere = Scroll::default().by(4, view.len(), 10);
        assert_eq!(
            view.pressed(somewhere, "x", NONE, 10),
            Pressed::Open(somewhere)
        );
        let alt = Modifiers { alt: true, ..NONE };
        assert_eq!(
            view.pressed(somewhere, "ArrowDown", alt, 10),
            Pressed::Open(somewhere),
            "a modified arrow is not this view's gesture",
        );
    }

    /// The scroll keys move, and the two spellings of each move the same way.
    #[test]
    fn the_scroll_keys_agree_with_their_aliases() {
        let view = view();
        let at = |name: &str| match view.pressed(Scroll::default(), name, NONE, 10) {
            Pressed::Open(scroll) => scroll.offset(view.len(), 10),
            Pressed::Closed => panic!("{name} closed the view"),
        };
        assert_eq!(at("ArrowDown"), 1);
        assert_eq!(at("j"), 1);
        assert_eq!(at("PageDown"), 9);
        assert_eq!(at("Space"), 9);
        assert_eq!(at(" "), 9);
        assert_eq!(at("f"), 9);
        assert_eq!(at("End"), view.len() - 10);
        assert_eq!(at("Home"), 0);
        assert_eq!(at("ArrowUp"), 0, "the top does not scroll past itself");
    }

    /// The view is longer than a default terminal, which is why it scrolls at all.
    ///
    /// Stated as an assertion rather than in prose because it is the premise of every test above:
    /// a view that fitted in 24 rows would make all of them vacuous.
    #[test]
    fn the_view_does_not_fit_a_default_terminal() {
        assert!(
            view().len() > 24,
            "the view fits on one screen, so the scroll tests measure nothing",
        );
    }
}
