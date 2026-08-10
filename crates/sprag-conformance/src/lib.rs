//! THE PROPERTY CLAIMS OVER A PUBLISHED CALL GRAMMAR, written once for every surface that publishes
//! one.
//!
//! # Why this is a crate and not a test module
//!
//! R352 wrote three of these against the multiplexer, inside that surface's own test module. R353
//! needed all three a second time for a pane's input surface and a third time for the plugin host, so
//! they moved into one `#[cfg(test)]` module of `sprag-host` — one definition, three readers, R347's
//! rule met before the copy existed.
//!
//! Then the front reached **the GUI's three surfaces**, which live in `sprag-gui`: a `#[cfg(test)]`
//! item is invisible across a crate boundary, so the choice was a fourth copy of every claim or a home
//! both crates can reach. A copy is what this whole feature exists to refuse — each copy passes on its
//! own surface, and the first divergence is invisible.
//!
//! ⚠ **DEV-ONLY, like `sprag-peer` and `sprag-gate`.** No binary depends on it, so the daemon does not
//! ship an audit harness.
//!
//! ⚠⚠ **AND IT DEPENDS ON `sprag-rpc`, NOT ON `sprag-host`** — the first draft did the latter while
//! being a dev-dependency of it, which is a cycle cargo permits and which **linked two different
//! `sprag_host` crates**, so the types it was handed were not the types it declared. The shapes moved
//! down into the crate every front already shares. A harness about the wire's vocabulary depends on
//! the wire's vocabulary.
//!
//! # Findings, not assertions
//!
//! Every claim here answers what it DROVE and what it FOUND, and the caller asserts. That is not
//! politeness about panics: the non-vacuity COUNT belongs to the surface — 31 published words on the
//! multiplexer, 16 on a pane, 8 on the plugin host — and a harness that asserted for the caller would
//! have to be told the number anyway. The claim is the finding's text; the count is the surface's.

use pinion_core::external::{IntrospectValue, InvokeError, SchemaChannel};
use pinion_core::scene::Scene;
use serde_json::{Map, Value};
use sprag_rpc::grammar::{
    ActionGrammar, ArgGrammar, CallForm, FormKind, SurfaceAuthor, WireSurface,
};

/// What one claim drove, and what it found — the shape every check here answers.
///
/// `findings` is empty when the claim holds. Each entry is a whole sentence naming the verb, the
/// argument and what the daemon did, because a conformance failure is read by somebody who was not
/// looking at the table when it broke.
#[derive(Debug, Default)]
pub struct Driven {
    /// How many calls the claim actually made — the caller's non-vacuity number.
    pub count: usize,
    /// One sentence per violation, empty when the claim holds.
    pub findings: Vec<String>,
}

impl Driven {
    /// The count, or a panic carrying every finding — the one-line form a test wants.
    ///
    /// # Panics
    ///
    /// When the claim found anything at all.
    #[must_use]
    pub fn count_or_panic(self) -> usize {
        assert!(
            self.findings.is_empty(),
            "{} conformance finding(s):\n  {}",
            self.findings.len(),
            self.findings.join("\n  "),
        );
        self.count
    }
}

/// A way to call one surface's verbs — the surface's own `invoke`, closed over its fixture.
///
/// ⚠ The trait object's own lifetime is SPELLED (`+ 'a`) rather than elided. Elided, the alias and a
/// `impl Fn(.., Invoke<'_>)` bound at a call site do not resolve to the same higher-ranked signature
/// across a crate boundary, and the mismatch reads as *"expected due to this / found signature defined
/// here"* with two lines that look identical.
pub type Invoke<'a> =
    &'a mut (dyn FnMut(&str, IntrospectValue) -> Result<IntrospectValue, InvokeError> + 'a);

/// A JSON value AS THE WIRE DELIVERS IT to a surface.
///
/// ⚠⚠ **AN INSTRUMENT IS A CLAIM** (R351). pinion's `json_to_introspect_value` maps a JSON string to
/// [`IntrospectValue::Text`], a number to `Int`, a bool to `Bool`, and an object or array to `Json` —
/// so a harness that wrapped everything in `Json` would be probing a shape no client can send, and
/// would report nothing about the SCALAR forms two of these surfaces publish (`text` matches `Text`
/// and refuses `Json`). This mirrors that mapping, and the mirror is not taken on trust:
/// `a_client_can_drive_a_pane_from_its_published_grammar` builds both forms of a call from the served
/// answer and sends them over a REAL socket, where the conversion is pinion's own.
#[must_use]
pub fn as_the_wire_delivers_it(value: &Value) -> IntrospectValue {
    match value {
        Value::Null => IntrospectValue::Null,
        Value::Bool(held) => IntrospectValue::Bool(*held),
        Value::Number(number) => number
            .as_i64()
            .map_or(IntrospectValue::Null, IntrospectValue::Int),
        Value::String(text) => IntrospectValue::Text(text.clone()),
        Value::Array(_) | Value::Object(_) => IntrospectValue::Json(value.clone()),
    }
}

/// The `args` value the PUBLISHED GRAMMAR alone says is well-formed, with one argument set to `probe`
/// — what an agent that has read the grammar slot and nothing else would send.
///
/// Every REQUIRED argument is filled, because a call missing one is malformed by the declaration's own
/// account and its refusal would say nothing about the value under test. Every OPTIONAL one is left
/// out, so exactly one argument is being varied. The filler for a required argument comes from the
/// declaration too: a vocabulary's first word, or `1` for the only other required scalar shape these
/// tables have (an id or a count, which the fixtures hold).
///
/// ⚠ A [`FormKind::Scalar`] form has no keys at all — the probe IS the call — which is the whole reason
/// the form kind is published. A harness that assumed an object here would have sent `{"text": …}` to
/// a verb whose scalar form takes the bare string, and reported about the object form twice.
#[must_use]
pub fn call_built_from_the_grammar(
    form: &CallForm,
    vary: &ArgGrammar,
    probe: Value,
) -> IntrospectValue {
    if form.form == FormKind::Scalar {
        return as_the_wire_delivers_it(&probe);
    }
    let mut map = Map::new();
    for arg in form.args {
        if arg.name == vary.name {
            map.insert(arg.name.to_owned(), probe.clone());
        } else if !arg.optional {
            map.insert(
                arg.name.to_owned(),
                match (arg.words, arg.ty) {
                    // A vocabulary supplies its own filler. The FIRST word, arbitrarily: this
                    // argument is not the one under test, and every word of it is driven by its own
                    // turn through the loop.
                    (Some(words), _) => Value::from(words[0]),
                    // ⚠ ONE, NOT ZERO, AND THE FILLER IS A CLAIM TOO. Zero is a legal pane id and an
                    // ILLEGAL dimension — `resize_window` refuses a zero-column rectangle as
                    // malformed — so a zero filler made this harness report `resize_window`'s
                    // `window` as constrained when what the daemon had rejected was the filler beside
                    // it. One is admissible in every int argument these verbs take.
                    (None, "int") => Value::from(1),
                    (None, "bool") => Value::from(false),
                    // ⚠ AN ARGV, and the program is chosen rather than invented: `/bin/echo` is on
                    // both platforms this project ships to (R352b swept them after a doctest spawned
                    // a `/bin/true` macOS does not have) and it exits at once. The first REQUIRED
                    // array on this wire is a dialogue's endpoint, and the filler table had no arm
                    // for one — the plugin surface's checks found that on their first run.
                    (None, "array") => Value::from(vec![Value::from("/bin/echo")]),
                    // ⚠ NO ARM FOR A REQUIRED OBJECT, deliberately: every object argument on this
                    // wire today is optional (`guardrails`), so an arm for one would be a fallback
                    // nothing drives — wrong the first time it ran, and R318's rule. The first
                    // required object falls to the string filler below and fails NAMING the argument,
                    // which is exactly how the missing array arm was found.
                    //
                    // A window NAME the fixture does not hold: it parses, which is all this filler
                    // has to do, and it cannot collide with a real window.
                    (None, _) => Value::from("filler-not-a-window"),
                },
            );
        }
    }
    IntrospectValue::Json(Value::Object(map))
}

/// ⚠⚠ **EVERY WORD THE WIRE PUBLISHES IS A WORD THE DAEMON ACCEPTS** — the write half of the discovery
/// pair, and the claim that makes a grammar slot worth reading.
///
/// A schema that publishes a vocabulary is making a promise about somebody else's code: it says an
/// agent may enumerate these words and send any of them. Nothing about a `const` array enforces that
/// — the words could be yesterday's spelling, or a set the parser narrowed — so the promise is driven,
/// one call per word, through the real `invoke`.
///
/// # Why `TypeMismatch` is the discriminator and the other refusals are not
///
/// These verbs answer `TypeMismatch` for exactly one thing: a request their GRAMMAR does not admit. A
/// `Rejected` means the grammar was read and the request could not be honoured — no pane that way, no
/// window by that name, no pending clipboard query — which is the action's business and not this
/// claim's. So a word that gets anything other than `TypeMismatch` was ACCEPTED as a word, which is
/// the whole claim.
#[must_use]
pub fn every_published_word_is_accepted(
    table: &'static [ActionGrammar],
    invoke: Invoke<'_>,
) -> Driven {
    let mut driven = Driven::default();
    for verb in table {
        for form in verb.forms {
            for arg in form.args {
                let Some(words) = arg.words else { continue };
                for word in words {
                    let call = call_built_from_the_grammar(form, arg, Value::from(*word));
                    let answer = invoke(verb.action, call.clone());
                    if matches!(answer, Err(InvokeError::TypeMismatch)) {
                        driven.findings.push(format!(
                            "THE WIRE PUBLISHES A WORD THE DAEMON REFUSES. `{}` says its `{}` may be \
                             {word:?}, and sending {call:?} came back TypeMismatch — an agent that \
                             enumerated the published vocabulary would have built a call this daemon \
                             cannot read.",
                            verb.action, arg.name,
                        ));
                    }
                    driven.count += 1;
                }
            }
        }
    }
    driven
}

/// ⚠⚠ **AN ARGUMENT THE PARSER CONSTRAINS MUST PUBLISH WHAT IT ADMITS** — the completeness half, and
/// the one that cannot be satisfied by remembering.
///
/// The claim above is soundness: nothing published is refused. It says nothing about an argument whose
/// vocabulary was simply left out — which is the failure this project keeps meeting, because a
/// hand-written list is the one a new thing is missing from. There is no way to DERIVE "this string
/// argument is closed" from the declaration, so it is derived from the PRODUCT instead: send a word
/// nobody could have declared, and see whether the parser takes it.
///
/// * it takes the nonsense ⇒ the argument really is open, and publishing no vocabulary is true;
/// * it refuses ⇒ the argument is drawn from some set, and the wire is keeping it a secret.
///
/// So an argument added tomorrow with a closed vocabulary and no `one_of` fails here, with nobody
/// having had to notice. **It found `clipboard_answer`'s `sel` and `key`'s `state`** (R353), two
/// vocabularies that had lived as string literals inside a pane's parsers.
///
/// ⚠⚠ **IT CAN ONLY SEE A VOCABULARY THE DAEMON REFUSES AS MALFORMED.** A parser that answers
/// `Rejected` with a friendly sentence for a bad word looks OPEN here and passes — which is what the
/// plugin host's `plugin` and `format_a` were doing, and why both now answer `TypeMismatch`.
#[must_use]
pub fn a_constrained_argument_publishes_what_it_admits(
    table: &'static [ActionGrammar],
    invoke: Invoke<'_>,
) -> Driven {
    // A value no vocabulary in this workspace can contain, and no window or pane is named.
    const NONSENSE: &str = "not-a-word-any-vocabulary-holds";

    let mut driven = Driven::default();
    for verb in table {
        for form in verb.forms {
            for arg in form.args {
                if arg.ty != "string" || arg.words.is_some() {
                    continue;
                }
                let call = call_built_from_the_grammar(form, arg, Value::from(NONSENSE));
                let answer = invoke(verb.action, call.clone());
                if matches!(answer, Err(InvokeError::TypeMismatch)) {
                    driven.findings.push(format!(
                        "`{}` REFUSES ITS OWN DECLARED `{}` AS MALFORMED, so that argument is drawn \
                         from a set the wire does not publish. Send {call:?} and the daemon answers \
                         TypeMismatch — declare the vocabulary with `ArgGrammar::one_of`, projected \
                         from the closed set the parser reads through.",
                        verb.action, arg.name,
                    ));
                }
                driven.count += 1;
            }
        }
    }
    driven
}

/// ⚠⚠ **A DECLARED ARGUMENT IS ONE THE DAEMON ACTUALLY READS** — the claim that lets a grammar be
/// written by hand at all.
///
/// The other two hold the VOCABULARIES to the parser. Neither can see a declared argument the parser
/// ignores completely: send it, nothing refuses, and the wire goes on advertising a key that does
/// nothing.
///
/// This closes it by TYPE. Send each declared argument with a value of the wrong JSON type — a string
/// where an int is declared, a number where a string is — and require the daemon to refuse the request
/// as malformed. A parser that reads the key cannot accept the wrong type for it; a parser that never
/// looks at the key cannot refuse anything. So:
///
/// * refused ⇒ the declaration names a key this verb genuinely reads, at the type it claims;
/// * accepted ⇒ **the wire is advertising an argument the daemon does not have.**
///
/// ⚠⚠ **IT FOUND FIVE COERCIONS ON THE PANE SURFACE** (R353): `ctrl`, `alt`, `shift`, `super` and
/// `state` were read with `and_then(…).unwrap_or(default)`, so `{"key":"a","ctrl":1}` was injected as
/// an unmodified `a` and answered success — in a parser whose own `col` and `row` refuse a malformed
/// value two lines away.
///
/// # What it still cannot see, said rather than implied
///
/// An argument the parser reads and the declaration OMITS. That direction is absent-not-wrong — a
/// client is told less rather than something false — and the only thing that catches it is deriving
/// the key set from the request type, which `the_published_grammar_is_the_ask_types_own` does for the
/// verbs that have one.
#[must_use]
pub fn a_declared_argument_is_one_the_daemon_reads(
    table: &'static [ActionGrammar],
    invoke: Invoke<'_>,
) -> Driven {
    let mut driven = Driven::default();
    for verb in table {
        for form in verb.forms {
            for arg in form.args {
                // The wrong type for what this argument says it is. A vocabulary argument is a
                // string, so a number is wrong for it too — and wrong in a way no vocabulary could
                // absorb.
                let wrong = match arg.ty {
                    "int" | "bool" => Value::from("not-of-the-declared-type"),
                    _ => Value::from(4242),
                };
                let call = call_built_from_the_grammar(form, arg, wrong);
                let answer = invoke(verb.action, call.clone());
                if !matches!(answer, Err(InvokeError::TypeMismatch)) {
                    driven.findings.push(format!(
                        "`{}` ACCEPTS A {} FOR ITS DECLARED `{}`, so either the daemon does not read \
                         that key at all or it does not read it as a {}. Sending {call:?} answered \
                         {answer:?}, and a wire that advertises an argument nothing reads is worse \
                         than one that says nothing.",
                        verb.action, arg.ty, arg.name, arg.ty,
                    ));
                }
                driven.count += 1;
            }
        }
    }
    driven
}

/// ⚠⚠ **A NULLARY FORM IS A VERB THAT NEEDS NOTHING** — the claim the other three cannot make, because
/// they walk arguments and this form has none.
///
/// A [`FormKind::Nullary`] declaration says *"there is nothing to fill in"*. Two ways that can be a
/// false statement, and this drives both:
///
/// * the verb refuses a call with no arguments ⇒ it needs something the wire does not mention, and a
///   client that believed the form is refused for a reason it cannot see;
/// * the verb answers DIFFERENTLY when handed an argument ⇒ it reads a key after all, and the
///   declaration is hiding it. Probed with an object no declaration mentions; a nullary verb must
///   treat it exactly as it treated `null`.
///
/// ⚠ The second half is why the GUI's five nullary verbs are safe to publish as nullary: each ignores
/// its `args` entirely, which is a fact about the parser rather than a promise in a comment.
#[must_use]
pub fn a_nullary_form_is_a_verb_that_needs_nothing(
    table: &'static [ActionGrammar],
    invoke: Invoke<'_>,
) -> Driven {
    let mut driven = Driven::default();
    for verb in table {
        for form in verb.forms {
            if form.form != FormKind::Nullary {
                continue;
            }
            let bare = invoke(verb.action, IntrospectValue::Null);
            if matches!(bare, Err(InvokeError::TypeMismatch)) {
                driven.findings.push(format!(
                    "`{}` PUBLISHES A NULLARY FORM AND REFUSES A CALL WITH NO ARGUMENTS, so the one \
                     thing the wire says about calling it is wrong.",
                    verb.action,
                ));
            }
            let ignored = invoke(
                verb.action,
                IntrospectValue::Json(Value::Object(
                    [("a-key-no-declaration-mentions".to_owned(), Value::from(1))]
                        .into_iter()
                        .collect(),
                )),
            );
            if bare.is_ok() != ignored.is_ok() {
                driven.findings.push(format!(
                    "`{}` PUBLISHES A NULLARY FORM AND ANSWERS DIFFERENTLY WHEN HANDED AN ARGUMENT \
                     ({bare:?} vs {ignored:?}), so it reads something the wire does not declare.",
                    verb.action,
                ));
            }
            driven.count += 2;
        }
    }
    driven
}

/// ⚠⚠ **EVERY VERB A SURFACE DECLARES PUBLISHES ITS GRAMMAR, OR IS A NAMED EXEMPTION** — the omission
/// direction, over a whole SCENE.
///
/// # What no other claim here can see
///
/// The four above walk a TABLE and drive what is in it. A verb the table LEAVES OUT declares nothing
/// for them to audit — the same blindness R352 paid for at the schema level, where `report_agent` was
/// dispatched and declared nowhere and the round's own new gate stayed green.
///
/// So the SCENE is the source and the tables are checked against it. Four findings, and the last two
/// are what keep the exemption list from becoming a place things go to be forgotten:
///
/// 1. a surface serving verbs that `surfaces` does not name — **this is what found the plugin host**,
///    one hour after the list it checks was hand-written (R353);
/// 2. a declared verb neither described nor exempted;
/// 3. a table entry for a verb the surface does not serve (a renamed action's leftover);
/// 4. an exemption for a verb that has since been described, or that no longer exists.
#[must_use]
pub fn every_verb_a_surface_declares_publishes_its_grammar(
    scene: &Scene,
    surfaces: &'static [WireSurface],
) -> Driven {
    let mut driven = Driven::default();
    let served = verbs_served(scene);

    // ⚠ NO TAG NAMES TWO SURFACES. With placeholder matching a collision needs two entries that
    // accept the same tag — a duplicate, or a stem that also matches a sibling's whole name — and
    // whichever comes second is an entry no claim here reaches.
    for (i, outer) in surfaces.iter().enumerate() {
        for inner in &surfaces[i + 1..] {
            if outer.tag == inner.tag || names(outer, inner.tag) || names(inner, outer.tag) {
                driven.findings.push(format!(
                    "`{}` AND `{}` BOTH ANSWER TO ONE TAG, so one of them is an entry this audit \
                     never reaches.",
                    outer.name, inner.name,
                ));
            }
        }
    }

    let mut unlisted: Vec<&str> = served
        .iter()
        .filter(|(under, _)| !surfaces.iter().any(|surface| names(surface, under)))
        .map(|(under, _)| under.as_str())
        .collect();
    unlisted.sort_unstable();
    unlisted.dedup();
    for under in unlisted {
        driven.findings.push(format!(
            "THIS SCENE SERVES VERBS ON A SURFACE THE LIST DOES NOT NAME (`{under}`), so nothing else \
             here is about them: no claim asks whether they publish a call grammar, and a client \
             walking `$schema` finds addresses it cannot learn to call.",
        ));
    }

    for surface in surfaces {
        let mut declared: Vec<&str> = served
            .iter()
            .filter(|(under, _)| names(surface, under))
            .map(|(_, verb)| verb.as_str())
            .collect();
        declared.sort_unstable();
        declared.dedup();
        if declared.is_empty() {
            driven.findings.push(format!(
                "{} IS IN THE SURFACE LIST AND SERVES NO VERB IN THIS SCENE, so every claim about it \
                 is vacuous.",
                surface.name,
            ));
            continue;
        }
        if surface.author == SurfaceAuthor::Upstream {
            // ⚠ LISTED, NOT DESCRIBED — and not silently skipped either: an upstream surface that came
            // to carry a grammar table would be sprag describing another project's request shape from
            // the outside, so that is the one thing checked about it.
            if !surface.grammar.is_empty() {
                driven.findings.push(format!(
                    "{} IS AN UPSTREAM WIDGET AND CARRIES A GRAMMAR TABLE — describing another \
                     project's request shape from the outside is the affirmative false statement this \
                     surface avoids.",
                    surface.name,
                ));
            }
            driven.count += declared.len();
            continue;
        }
        for verb in &declared {
            let described = surface.grammar.iter().any(|entry| entry.action == *verb);
            if !described && !surface.undescribed.contains(verb) {
                driven.findings.push(format!(
                    "{} DECLARES `{verb}` AND PUBLISHES NOTHING ABOUT CALLING IT. An agent that \
                     walks `$schema` learns the address and cannot learn one argument, so it has to \
                     know the request grammar out of band — which for an AI client means guessing. \
                     Add the verb to this surface's grammar table, or name it in the surface's \
                     `undescribed` list with the reason it cannot be described.",
                    surface.name,
                ));
            }
        }
        for orphan in surface
            .grammar
            .iter()
            .map(|entry| entry.action)
            .filter(|action| !declared.contains(action))
        {
            driven.findings.push(format!(
                "{} PUBLISHES A GRAMMAR FOR `{orphan}`, WHICH IT DOES NOT SERVE — a description of a \
                 call nobody can make.",
                surface.name,
            ));
        }
        for stale in surface.undescribed.iter().filter(|verb| {
            !declared.contains(*verb) || surface.grammar.iter().any(|e| e.action == **verb)
        }) {
            driven.findings.push(format!(
                "{} EXEMPTS `{stale}`, WHICH IT NO LONGER SERVES OR NOW PUBLISHES ANYWAY — a stale \
                 decision, and a list nothing prunes is how one survives.",
                surface.name,
            ));
        }
        driven.count += declared.len();
    }
    driven
}

/// Whether `under`'s own tag is this surface's.
///
/// The LAST segment of the chain is the external's own tag, and the match is a PREFIX because a
/// surface registered once per pane spells the index into its tag (`sprag_gui.pane.0`), so its stem is
/// the only name the list can carry. `every_verb_a_surface_declares_publishes_its_grammar` refuses two
/// tags where one is a prefix of the other, which is what keeps a prefix from meaning two things.
fn names(surface: &WireSurface, under: &str) -> bool {
    let Some(own) = under.rsplit('/').next() else {
        return false;
    };
    match surface.tag.find('<') {
        None => own == surface.tag,
        Some(at) => {
            let stem = &surface.tag[..at];
            own.strip_prefix(stem)
                .is_some_and(|tail| !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()))
        }
    }
}

/// Every `(surface tag chain, verb)` the scene SERVES, walked through `External::introspect` — the same
/// accessor `scene/invoke` resolves a path with, so a surface reachable by a client is one counted
/// here.
#[must_use]
pub fn verbs_served(scene: &Scene) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk(scene, "", &mut found);
    found
}

fn walk(scene: &Scene, under: &str, found: &mut Vec<(String, String)>) {
    let tagged = |tag: &Option<std::borrow::Cow<'static, str>>| match tag {
        Some(tag) if under.is_empty() => tag.to_string(),
        Some(tag) => format!("{under}/{tag}"),
        None => under.to_owned(),
    };
    match scene {
        Scene::External(node) => {
            if let Some(introspect) = node.handle.introspect() {
                let under = tagged(&node.tag);
                for field in introspect.schema().fields {
                    if field.channel == SchemaChannel::Invoke {
                        found.push((under.clone(), field.path.to_owned()));
                    }
                }
            }
        }
        Scene::Container(node) => {
            let under = tagged(&node.tag);
            for child in &node.children {
                walk(child, &under, found);
            }
        }
        _ => {}
    }
}
