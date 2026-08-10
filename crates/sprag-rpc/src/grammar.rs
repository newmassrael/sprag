//! THE SHAPES A PUBLISHED CALL GRAMMAR IS MADE OF — one argument, one form, one verb, one surface.
//!
//! # Why these live beside the protocol number and not with the tables
//!
//! `sprag_host::wire` owns the TABLES: which verbs the multiplexer serves, what a pane's `key` takes,
//! which four forms a plugin `run` has. Those name sprag's actions and belong with them, and that
//! module goes on re-exporting every type here so a reader of the grammar still finds one place.
//!
//! What moved is the VOCABULARY OF SHAPES, and the reason is a compiler error rather than taste. The
//! conformance harness that drives these declarations against a live surface has to be reachable from
//! every crate that serves one — `sprag-host` for the multiplexer, a pane and the plugin host,
//! `sprag-gui` for the palette, the confirmation and the hyperlink oracle. As a `#[cfg(test)]` module
//! it was reachable from neither; as a dev-dependency CYCLE on `sprag-host` it compiled and then
//! **linked two different `sprag_host` crates, so `ActionGrammar` was not `ActionGrammar`** (cargo
//! says so: *"there are multiple different versions of crate `sprag_host` in the dependency graph"*).
//!
//! Putting the shapes in the crate every front already depends on removes the cycle instead of
//! relying on it: the harness depends on the WIRE's vocabulary, which is what it is about, and not on
//! the daemon that happens to serve most of it.
//!
//! ⚠ An inherent `impl` may only live in the defining crate, so the three tables became free consts
//! (`sprag_host::wire::MUX_GRAMMAR` and its siblings) rather than `ActionGrammar::MUX`. That is
//! arguably where they belonged: a table is not a member of a shape.

use serde_json::{Map, Value};

/// ONE ARGUMENT OF ONE VERB, as a client discovers it — the name to send it under, its JSON type,
/// whether a well-formed call may omit it, and the CLOSED VOCABULARY it admits when it has one.
///
/// # Why sprag declares this and does not use pinion's
///
/// pinion's `SchemaArg` is the same idea and is the right long-term home: after R1638 it carries
/// an `ArgDomain`, an argument form, and optionality, and
/// `SchemaField::action_with` attaches the lot to a verb inside `$schema`. **None of that exists at
/// the rev sprag pins.** Measured at `0de77429`: `ArgForm`, `action_with` and `ArgDomain::OneOf`
/// are all absent, and the three commits that add them are unpushed in the sibling checkout, so
/// there is no rev sprag can name to get them.
///
/// The honest options were to publish nothing until the upstream lands, or to publish the same
/// facts on a surface sprag owns. Publishing nothing is what the wire has been doing since it
/// existed, and it is the whole of the gap this pays: a client that cannot ask what a word may be
/// has to know it out of band, which for an AI client means guessing. So the facts go out now, in
/// sprag's own vocabulary, shaped deliberately like the upstream's — a `name`/`type`/`optional`
/// triple plus a `one_of` — so the day the pin moves this becomes a second PROJECTION of one table
/// rather than a second table.
///
/// ⚠ The residue, stated rather than hidden: until then a client learns the ARGUMENTS from
/// `ACTION_GRAMMAR_SLOT` and the ADDRESSES from `$schema`, which is two reads where the upstream
/// design has one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArgGrammar {
    /// The key this argument is sent under, inside the action's `args` object.
    pub name: &'static str,
    /// The JSON type of the value, in `$schema`'s vocabulary (`"int"`, `"string"`, `"bool"`).
    pub ty: &'static str,
    /// Whether a WELL-FORMED call may leave this key out.
    ///
    /// True does not mean "unimportant": for several of these verbs the absence has its own
    /// meaning — no pane names the active one, no rectangle means the un-pin — and the verb's own
    /// documentation is where that is said.
    pub optional: bool,
    /// The CLOSED VOCABULARY this argument admits, or [`None`] when the value is the caller's own
    /// (a name, a needle, a number).
    ///
    /// ⚠ **Never a literal**: every one of these is a
    /// `WIRE_WORDS` array projected from the closed set the parser reads
    /// through, so what a client is told it may send is what the daemon admits, by construction and
    /// not by a test that could be forgotten. The one assembled by hand
    /// (`MoveWindowAsk::PLACE_WORDS`) says so at its declaration and is held to the parser by
    /// `every_published_word_is_a_word_the_daemon_accepts`.
    pub words: Option<&'static [&'static str]>,
    /// THE ARGUMENTS INSIDE THIS ONE, when its value is an object with a known shape — empty for
    /// every scalar argument.
    ///
    /// # This is the answer to "is a nested value a recursive grammar, or an address of its own?"
    ///
    /// It is a RECURSIVE GRAMMAR, and the reason is that a nested argument has no independent
    /// existence: `guardrails` cannot be invoked, cannot be queried, and lives exactly as long as
    /// the call it rides on. An address of its own would have promised all three. The alternative
    /// was considered because `set_layout`'s arrangement TREE really is addressable — and that case
    /// is still undescribed, deliberately: this answers the FINITE nesting a call carries, not an
    /// unbounded recursive value. See [`WireSurface::undescribed`].
    ///
    /// The publication cost is one additive key ([`FIELDS_KEY`](Self::FIELDS_KEY)), so a reader that
    /// does not know it sees the shape it always saw — the same argument [`to_answer`](Self::to_answer)
    /// makes for omitting an absent `one_of`.
    ///
    /// ⚠ **What it buys is not documentation, it is a MOUTH.** `guardrails` was declared
    /// `object` with its inner keys unnamed, so the loop's iteration ceiling and its typed cost
    /// ceiling — the whole reason a bounded run is safe — could not be reached by any client
    /// building a call from the publication. A door derived from a grammar that cannot say
    /// `max_iterations` is a door with no lock on it.
    pub fields: &'static [ArgGrammar],
}

impl ArgGrammar {
    /// The answer key naming the argument.
    pub const NAME_KEY: &'static str = "name";
    /// The answer key naming its JSON type.
    pub const TYPE_KEY: &'static str = "type";
    /// The answer key saying a well-formed call may omit it.
    pub const OPTIONAL_KEY: &'static str = "optional";
    /// The answer key carrying the closed vocabulary, absent when there is none.
    pub const ONE_OF_KEY: &'static str = "one_of";
    /// The answer key carrying the arguments INSIDE this one, absent when it carries none.
    pub const FIELDS_KEY: &'static str = "fields";

    /// An argument whose value is the caller's own, and which the call must carry.
    #[must_use]
    pub const fn open(name: &'static str, ty: &'static str) -> Self {
        Self {
            name,
            ty,
            optional: false,
            words: None,
            fields: &[],
        }
    }

    /// An argument drawn from a CLOSED VOCABULARY, and which the call must carry.
    ///
    /// Hand a `WIRE_WORDS` array, never a literal — see [`words`](Self::words).
    #[must_use]
    pub const fn one_of(
        name: &'static str,
        ty: &'static str,
        words: &'static [&'static str],
    ) -> Self {
        Self {
            name,
            ty,
            optional: false,
            words: Some(words),
            fields: &[],
        }
    }

    /// An argument whose value is an OBJECT with these arguments inside it.
    ///
    /// The type is fixed at `"object"` rather than taken as a parameter: a value with named fields
    /// inside it is an object, and letting a caller declare `nested("guardrails", "int", …)` would
    /// spell a shape no reader could act on. [`fields`](Self::fields) says why nesting is a
    /// recursion here and not an address.
    #[must_use]
    pub const fn nested(name: &'static str, fields: &'static [ArgGrammar]) -> Self {
        Self {
            name,
            ty: "object",
            optional: false,
            words: None,
            fields,
        }
    }

    /// The same argument, marked as one this form REQUIRES.
    ///
    /// The inverse of [`optional`](Self::optional), and it exists because the two shared window
    /// arguments (`WindowRef::NAMED_ARG`) are omittable on most verbs and not on all: a
    /// `join_pane` with no window names nowhere to put the pane. One declaration, adjusted at the
    /// form that differs, beats a second copy of the same name and type.
    #[must_use]
    pub const fn required(self) -> Self {
        Self {
            optional: false,
            ..self
        }
    }

    /// The same argument, marked as one a well-formed call may leave out.
    ///
    /// A builder rather than a second pair of constructors, for the reason pinion's own
    /// `SchemaArg` gives its optional builder: optionality is orthogonal to where the values come
    /// from, and four constructors would be the product of two axes spelled out.
    #[must_use]
    pub const fn optional(self) -> Self {
        Self {
            optional: true,
            ..self
        }
    }

    /// This argument as the client reads it.
    ///
    /// `one_of` and `fields` are OMITTED rather than sent as `null` when there is no vocabulary and
    /// no nesting, for the reason `ArgForm::Undeclared` publishes nothing: a reader that
    /// does not know the key sees the shape it always saw, and a reader that does tells silence
    /// from a claim by the key's absence.
    #[must_use]
    pub fn to_answer(&self) -> Value {
        let mut map = Map::new();
        map.insert(Self::NAME_KEY.to_owned(), Value::from(self.name));
        map.insert(Self::TYPE_KEY.to_owned(), Value::from(self.ty));
        map.insert(Self::OPTIONAL_KEY.to_owned(), Value::from(self.optional));
        if let Some(words) = self.words {
            map.insert(
                Self::ONE_OF_KEY.to_owned(),
                Value::from(words.iter().map(|w| Value::from(*w)).collect::<Vec<_>>()),
            );
        }
        if !self.fields.is_empty() {
            map.insert(
                Self::FIELDS_KEY.to_owned(),
                Value::from(
                    self.fields
                        .iter()
                        .map(ArgGrammar::to_answer)
                        .collect::<Vec<_>>(),
                ),
            );
        }
        Value::Object(map)
    }
}

sprag_vt::closed_set! {
    /// WHERE A CALL'S ARGUMENTS LIVE — the shape of the `args` value itself, before any key is read.
    ///
    /// # The defect this exists for
    ///
    /// Every verb the multiplexer serves takes an OBJECT, so the first version of this table had no
    /// need to say so. The pane's input verbs do not: `key`, `text` and `paste` each accept a bare
    /// string as well (`invoke("text", "한")`), which is the seam an IME commit and a paste reach,
    /// and there is no key in that call at all. Publishing those as objects would have been an
    /// affirmative false statement about half of what the daemon accepts, and publishing nothing
    /// would have left the pane surface — the surface an agent uses most — undiscoverable for another
    /// round.
    ///
    /// # Why these two words and not pinion's six
    ///
    /// pinion HEAD's `ArgForm` is the same concept with a
    /// wider vocabulary (`Undeclared`, `Path`, `Nullary`, `Scalar`, `Object`, `Delimited(char)`), and
    /// it is the right long-term home for exactly the reason [`ArgGrammar`] gives about `SchemaArg`:
    /// **none of it exists at the rev sprag pins.** So this is deliberately a SUBSET of the
    /// upstream's, spelled with the upstream's words, and it publishes only the shapes sprag
    /// genuinely serves. The day the pin moves, each arm maps onto the upstream one of the same name.
    ///
    /// ⚠⚠ **R353 WROTE HERE THAT "a verb that took no arguments at all would be `Nullary` and sprag
    /// has none", AND THAT WAS FALSE WHEN IT WAS WRITTEN.** The GUI's three surfaces have **five** of
    /// them — `palette.open`, `palette.execute`, `confirm.accept`, `confirm.dismiss` and
    /// `hyperlink.activate`, every one of which ignores its `args` entirely. The claim held only for
    /// the surfaces the round had in front of it, which is the whole reason R354's coverage audit is
    /// derived from a SCENE rather than from the surfaces somebody remembered.
    ///
    /// ⚠ It is per FORM rather than per verb, which the upstream's is not, because `key` is BOTH: a
    /// bare string or an object. A verb-level form could not describe it without calling one of the
    /// two shapes a lie.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum FormKind {
        /// The arguments are the members of a JSON OBJECT, keyed by [`ArgGrammar::name`](ArgGrammar::name).
        Object,
        /// The single declared argument IS the whole `args` value — `invoke("text", "한")`. A form
        /// this shape carries exactly one argument, which the conformance harness holds it to.
        Scalar,
        /// The verb takes NO argument. `scene/invoke` may still be handed `null` — pinion's own
        /// wording, and its wire requires the `args` key to be present — so what this publishes is
        /// that there is nothing to fill in.
        ///
        /// ⚠ A form this shape declares ZERO arguments, so the three gates that walk arguments drive
        /// nothing for it. The claim that holds it is its own:
        /// `a_nullary_form_is_a_verb_that_needs_nothing` calls it with `null` and requires the verb to
        /// answer, which a verb that secretly reads a key cannot do.
        Nullary,
    }
}

impl FormKind {
    /// This shape's word in a published form's [`FORM_KEY`](CallForm::FORM_KEY).
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Scalar => "scalar",
            Self::Nullary => "nullary",
        }
    }
}

sprag_vt::wire_words!(FormKind: wire_str);

/// ONE WAY A VERB MAY BE CALLED — the shape of its `args` and the arguments that shape carries.
///
/// A verb publishes a LIST of these ([`ActionGrammar::forms`]), one per arm of its request grammar,
/// and a client picks one and fills it. The alternation is the part pinion's `SchemaArg` cannot
/// express and the part a flat argument list gets wrong: see [`ActionGrammar::forms`] for the
/// `move_window` call that a flat list said was well-formed and the daemon refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CallForm {
    /// Where this form's arguments live — an object's members, or the whole `args` value.
    pub form: FormKind,
    /// The arguments, in declared order. A [`FormKind::Scalar`] form has exactly one.
    pub args: &'static [ArgGrammar],
}

impl CallForm {
    /// The answer key naming the form's shape.
    pub const FORM_KEY: &'static str = "form";
    /// The answer key carrying the form's arguments.
    pub const ARGS_KEY: &'static str = "args";

    /// A form whose arguments are the members of a JSON object — every mux verb, and half the pane's.
    #[must_use]
    pub const fn object(args: &'static [ArgGrammar]) -> Self {
        Self {
            form: FormKind::Object,
            args,
        }
    }

    /// A form whose ONE argument is the whole `args` value.
    ///
    /// Takes the argument by value rather than as a slice so the "exactly one" is in the TYPE at the
    /// declaration, not only in the gate that checks it.
    #[must_use]
    pub const fn scalar(arg: &'static ArgGrammar) -> Self {
        Self {
            form: FormKind::Scalar,
            args: std::slice::from_ref(arg),
        }
    }

    /// A form with nothing to fill in — the shape of a verb whose subject is the surface's own state.
    ///
    /// The empty slice is in the constructor rather than at each declaration, so "nullary" and "no
    /// arguments" cannot come apart: there is no way to spell a nullary form that carries one.
    #[must_use]
    pub const fn nullary() -> Self {
        Self {
            form: FormKind::Nullary,
            args: &[],
        }
    }

    /// This form as the client reads it: its shape, and its arguments in declared order.
    ///
    /// ⚠ An OBJECT, where the first version of this surface published a bare array of arguments.
    /// That array could not say which shape it described, and the day a verb with a scalar form was
    /// published it would have had to — so the shape moved into the answer and
    /// [`WIRE_PROTOCOL`](crate::WIRE_PROTOCOL) rose with it. An added key is additive; a value that was an
    /// ARRAY and is now an OBJECT is not, and the number exists to say so (R342).
    #[must_use]
    pub fn to_answer(&self) -> Value {
        let mut map = Map::new();
        map.insert(Self::FORM_KEY.to_owned(), Value::from(self.form.wire_str()));
        map.insert(
            Self::ARGS_KEY.to_owned(),
            Value::from(
                self.args
                    .iter()
                    .map(ArgGrammar::to_answer)
                    .collect::<Vec<_>>(),
            ),
        );
        Value::Object(map)
    }
}

/// ONE VERB'S CALL GRAMMAR — the action's address and every argument it takes.
///
/// # What publishing this is for
///
/// An agent that can read the pane grid, the layout tree and the session list still could not work
/// out how to CALL anything: `$schema` published 34 verb names and not one argument, so every
/// client on this wire has had to carry sprag's request grammar as folklore. This is the table that
/// stops that, and each surface's table (`MUX_GRAMMAR`, `PANE_GRAMMAR`) is what that
/// surface's `ACTION_GRAMMAR_SLOT` serves.
///
/// # Which verbs are here, and why the others are not
///
/// Exactly the verbs whose request grammar is modelled by an ASK TYPE — the `to_args` / `parse`
/// pair that already spells the keys once for the daemon, the CLI verb, the MCP tool and the
/// keybinding. Those types are the source; this is a fourth face of the same grammar, and
/// `the_published_grammar_is_the_ask_types_own` holds each declaration to the keys its ask
/// actually emits.
///
/// A verb whose arguments are read inline out of the request map has no such source, and publishing
/// a hand-transcribed list for it would be the affirmative false statement this project keeps
/// paying for — worse than silence, because it carries a schema's authority. **So the rule is
/// mechanical: give a verb an ask type and it publishes itself.** The count of each is asserted, so
/// the gap is a number rather than an impression.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ActionGrammar {
    /// The action's address on the multiplexer surface — one of `MUX_SCHEMA`'s verbs.
    pub action: &'static str,
    /// Whether [`forms`](Self::forms) came from an ASK TYPE or was declared inline.
    ///
    /// Not a wire fact — it is never published — but the one thing that decides how much can be
    /// asserted about an entry. An ask-backed grammar is held to its request type key for key by
    /// `the_published_grammar_is_the_ask_types_own`, so an argument the parser reads and
    /// the declaration omits fails there. An inline one has no such source and is held only by what
    /// can be driven: every published word accepted, every open string argument genuinely open, and
    /// every declared argument refused at the wrong type.
    ///
    /// ⚠ **A `bool` and not a comment**, because the ask gate has to know which entries it is
    /// responsible for. Written as a comment it would have been a list in a test — the shape this
    /// whole feature exists to avoid — and adding an inline verb would have failed a gate that had
    /// nothing to do with it.
    pub from_ask: bool,
    /// The WAYS this verb may be spelled, one per arm of its ask type — each a complete list of
    /// the arguments THAT spelling takes.
    ///
    /// ⚠⚠ **A FLAT ARGUMENT LIST CANNOT SAY "EXACTLY ONE OF THESE"**, and this table's first draft
    /// was flat. Every sprag ask that is an ENUM is an alternation — `swap_pane` takes `with` or
    /// `dir` and refuses both, `move_window` takes `place` or `before` or `after` — so marking each
    /// alternative "optional" said, truthfully but uselessly, that a call could omit every one of
    /// them. An agent following that builds `{"window": "build"}` and is refused as malformed,
    /// which is exactly what
    /// `an_argument_the_daemon_constrains_publishes_what_it_admits` reported the first time
    /// it ran.
    ///
    /// pinion's `SchemaArg` cannot express the alternation either (its `optional` is the same flat
    /// idea), so this is not a shape sprag is declining to adopt — it is one the upstream has not
    /// met yet, because its surfaces take arguments and sprag's take REQUESTS.
    ///
    /// The forms are not invented here: there is one per arm of the ask, so an arm added to one of
    /// those enums is a form the round that adds it has to decide about, and
    /// `the_published_grammar_is_the_ask_types_own` compares the two lists arm by arm.
    pub forms: &'static [CallForm],
}

impl ActionGrammar {
    /// ONE SURFACE'S table as its `ACTION_GRAMMAR_SLOT` serves it: an object keyed by the action's
    /// address on that surface, each holding its forms in declared order.
    ///
    /// Keyed by address rather than sent as a list because the client's question is always *"how do
    /// I call THIS one"*, and an object answers it without a scan. Each value is a list of FORMS —
    /// a client picks one and fills it, which is the shape that makes an enumerated call correct
    /// rather than merely well-typed.
    ///
    /// # Why the table is a PARAMETER and not `Self::MUX`
    ///
    /// ⚠ **Each surface publishes the verbs IT serves, on its own copy of the slot.** The addresses
    /// are per surface — the multiplexer's `key`-less verbs hang off `/sprag_mux/external/…` while a
    /// pane's hang off `/pane_<id>/sprag_input/external/…` — so one flat table would have had to
    /// carry a surface dimension in every key, and two verbs of the same name on different surfaces
    /// could not both be described. Answering on the surface the client already holds a path to needs
    /// no such encoding: the address it asked is the address it invokes.
    #[must_use]
    pub fn answer(table: &'static [Self]) -> Value {
        let mut map = Map::new();
        for verb in table {
            map.insert(
                verb.action.to_owned(),
                Value::from(
                    verb.forms
                        .iter()
                        .map(CallForm::to_answer)
                        .collect::<Vec<_>>(),
                ),
            );
        }
        Value::Object(map)
    }
}

sprag_vt::closed_set! {
    /// WHOSE REQUEST SHAPE a surface's verbs are — the one thing that decides whether sprag may
    /// publish a grammar for them.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum SurfaceAuthor {
        /// sprag wrote this external, so its request grammar is sprag's to publish and every verb it
        /// declares must be described or exempted.
        Sprag,
        /// A pinion WIDGET sprag registers (a button, a text field, a scrollbar, the dock
        /// reorganizer). Its verbs are real and a client can call them; describing them is upstream's
        /// to do. Listed so the surface set stays complete and a new widget still forces a decision.
        Upstream,
    }
}

/// ONE SURFACE — what it is called, what it declares, what it publishes about
/// calling those verbs, and which of them it deliberately does not describe.
///
/// A named struct rather than the four-tuple this was first written as: the fourth column is an
/// EXEMPTION LIST and a reader has no way to know that from its position. (Clippy's
/// `type_complexity` said the same thing about the tuple, which is the cheap version of the same
/// argument.)
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WireSurface {
    /// What a gate's failure message calls this surface, in a reader's terms.
    pub name: &'static str,
    /// WHO WROTE this surface — whether its grammar is sprag's to publish.
    ///
    /// ⚠⚠ **A WINDOW SERVES FAR MORE VERBS THAN ITS OWNER WROTE.** sprag's GUI registers about sixty
    /// surfaces that are pinion WIDGET instances — a `ButtonExternal` per window tab, a
    /// `TextFieldExternal` per text field, a `ScrollbarExternal` per pane, the context menu, the dock
    /// reorganizer — each serving verbs of its own. R352 counted "eight verbs in the GUI" and meant
    /// the three surfaces sprag WROTE; the audit derived from the real registration list found
    /// sixty-eight verb addresses. Publishing a grammar for an upstream widget would be sprag
    /// describing somebody else's request shape from the outside, which is the affirmative false
    /// statement this whole surface avoids.
    ///
    /// So an upstream surface is LISTED and not described: the list stays complete, a widget added
    /// tomorrow still forces a decision, and nothing here claims to speak for pinion.
    pub author: SurfaceAuthor,
    /// The scene TAG its external hangs under — what a walk of the served scene finds it by.
    ///
    /// ⚠ A surface registered ONCE PER INSTANCE spells its index with the schema's own placeholder
    /// idiom (`sprag_gui.pane.<i>`, the shape `cells.<offset>` established), and the audit matches the
    /// stem plus a numeric tail. **Matching by bare prefix was the first draft and it was wrong**:
    /// `sprag_palette` silently swallowed the `sprag_palette_field` beside it — a pinion text field —
    /// so that widget's verbs were read as the palette's and the palette's own table appeared to
    /// describe them.
    ///
    /// ⚠ This was the surface's declared SCHEMA for one afternoon, and **nothing read it**: the gates
    /// walk the SERVED scene rather than the consts (R320), so the field was a claim with no reader —
    /// shape (1) of the debt sweep, in the round's own new code. The tag is what a gate genuinely
    /// needs, and carrying it here deleted a hand-written prose-name-to-tag `match` whose default arm
    /// was a panic.
    pub tag: &'static str,
    /// How to call the verbs it serves.
    pub grammar: &'static [ActionGrammar],
    /// The verbs it serves and publishes NOTHING about, each a decision with a reason beside it here.
    ///
    /// An omission that is not on this list fails
    /// `every_verb_a_surface_declares_publishes_its_grammar`, and an entry that has since been
    /// described fails it too — a list nothing prunes is how a stale decision survives.
    pub undescribed: &'static [&'static str],
}
