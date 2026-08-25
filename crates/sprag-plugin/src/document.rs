//! **WHAT A DOCUMENT DOES WITH AN ERROR OF ITS OWN, AND WHAT A HOST DOES WITH ONE THE DOCUMENT
//! CANNOT ANSWER** — register item 505.
//!
//! # ⚠⚠⚠⚠⚠ The silence this exists to end
//!
//! W3C SCXML 3.12.2: the processor raises `error.*` onto the internal queue and the events **are
//! ignored if no transition matches them**. So a document whose own executable content fails keeps
//! running, in exactly the voice of one that worked — and register item 483 measured the other half:
//! an error ABANDONS THE REST OF ITS BLOCK. Together those two clauses produce the failure this
//! module exists for: a `priming` whose `onentry` composed half a prompt and never sent it looks,
//! from every reading a host takes, like an agent that is thinking slowly.
//!
//! Measured 2026-08-20, before any of this was built: `ai_loop.scxml` and `debt_loop.scxml` carried
//! **zero** `error.*` transitions between them, and a mutation — one `<send>` naming a type nobody
//! serves, in `priming` — made a real run walk `Priming --PromptSent--> Working` and then
//! `Working --Null--> Working` **eleven times**, going nowhere, with every other gate in this crate
//! green.
//!
//! # The two answers, and why a document needs both
//!
//! | who | what they can answer | how a person hears it |
//! |---|---|---|
//! | the DOCUMENT, where it has states | its own `error.execution`, by the `fail` edge it already owns | the run ends `failed` and the sentence names the error |
//! | the HOST, at the door | an error raised where NOTHING can match — a datamodel-only kind, an error during initialisation | the door refuses, and the refusal names the count |
//!
//! A document with transitions answers for itself, because *what to do about a failure* is a
//! decision and this repository's decisions live in `.scxml`. A **kind** document has one state and
//! it is final on entry ([`crate::kind`]) — there is nowhere to put a transition, so its errors can
//! only ever be answered out here. Neither half covers the other: [`opened`] cannot know what a
//! failure MEANS to a loop, and a document cannot answer an error raised while it is being built.
//!
//! # ⚠⚠⚠⚠ And a handler is a new way to fail, which is why this reads THREE numbers
//!
//! The engine's own doc says it plainly: an error handler that fails the same way every time
//! answers its own error for ever, and *"that is not a hang: it is a core at 100% forever"* — a
//! reading an unattended supervisor takes as healthy. The engine stops feeding such a chain and
//! counts what it refused. So the round that gives these documents error handlers is the round that
//! must read that counter, or it would have traded a silence for a spin.
//!
//! ⚠ [`Faulted`] therefore carries `cascaded` beside `unanswered`, and [`faults`] answers `Some`
//! for either. One is *nobody answered*; the other is *somebody answered and could not*.

use core::fmt;

use sce_rust_runtime::{Engine, StatePolicy};

/// **AN ERROR THE DOCUMENT ITSELF RAISED AND NOBODY DEALT WITH** — the fact W3C SCXML 3.12.2 makes
/// invisible, in the shape a host can act on.
///
/// ⚠ Both counts are cumulative readings of one machine, so a `Faulted` is *what this document has
/// swallowed so far* rather than an event. [`opened`] takes it at initialisation, where the machine
/// is one macrostep old and the count can only be about the document's own start-up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Faulted {
    /// How many `error.*` this document raised with no transition to match them — W3C SCXML
    /// 3.12.2's *ignored*, counted.
    pub unanswered: u32,
    /// **WHICH ERROR THE LAST ONE WAS**, in the document's own event vocabulary
    /// (`"error.execution"`, `"error.communication"`), or [`None`] when the fault is a cascade
    /// rather than an unanswered event.
    ///
    /// ⚠⚠ The CLASS is the whole diagnosis and a count cannot carry it: `error.execution` is the
    /// document's own content failing — a repair in the `.scxml` — and `error.communication` is a
    /// `<send>` that could not be delivered, which is a repair in the HOST that did not serve the
    /// type. Two different people fix those.
    pub error: Option<&'static str>,
    /// How many `error.*` the engine REFUSED to queue because the handler answering them had itself
    /// been failing for a hundred links running.
    ///
    /// ⚠⚠⚠ The opposite failure from [`unanswered`](Self::unanswered) and the worse one: the
    /// document DOES match the error, the handler fails the same way, and the drain never empties.
    /// A run in that state is not idle and not stopped — it is a core at full tilt with a
    /// configuration that never moves.
    pub cascaded: u32,
}

impl fmt::Display for Faulted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.cascaded > 0 {
            return write!(
                f,
                "its own error handling failed {} time(s) answering the error it raised, so the \
                 engine stopped feeding the chain — the handler is what to look at, not the first \
                 failure",
                self.cascaded,
            );
        }
        match self.error {
            Some(error) => write!(
                f,
                "it raised {} and answers no error at all, so W3C SCXML 3.12.2 dropped it ({} in \
                 total) and the rest of that block never ran",
                error, self.unanswered,
            ),
            None => write!(
                f,
                "it raised {} error(s) nothing answered, and the engine could not say which",
                self.unanswered,
            ),
        }
    }
}

/// **OPEN A DOCUMENT** — build its machine, initialise it, and REFUSE one whose start-up failed in
/// silence.
///
/// # ⚠⚠⚠⚠⚠ Why every driven document in this crate is opened here
///
/// This is the one road, and that is the whole mechanism: a check a caller has to remember is a
/// check the next caller will not. `crate::access`'s barrier is on the client *"so that it cannot be
/// spelled without one"*, and the same argument reaches a statechart — a document initialised
/// through `Engine::new` + `initialize` by hand is a document nothing asked about, and the ratchet
/// `every_document_this_crate_drives_is_opened_through_one_door` is what says so when a site is
/// written that way.
///
/// ⚠⚠ A document that CANNOT raise an error needs no door and is not forced through one: measured
/// off the generated machines, `session.scxml` and `orchestration.scxml` are `datamodel="null"` with
/// no `cond`, `<send>` or `<invoke>`, and their event enums carry no error variant at all. That
/// exclusion is DERIVED rather than listed — see the ratchet, which reads the generated artefact —
/// so the day one of them grows a guard, the gate asks for this road.
///
/// # ⚠⚠⚠⚠⚠ Why the door also REGISTERS what the host serves
///
/// Register item 470 stage 2: a document declares an act with `<send type="x-sprag-host">` and this
/// crate performs it. SCE's contract has two halves and BOTH are required — a type declared to the
/// build with no handler registered raises `error.execution` exactly as an undeclared one does — and
/// the registration has to happen **before `initialize`**, because a document whose initial state
/// declares an act would dispatch it during initialisation, to a handler that did not exist yet.
///
/// That ordering is a thing a caller would have to remember, which is this door's whole argument.
/// So `serving` comes in here and is registered on the line above `initialize`, and a document that
/// declares no host act pays one map insertion for it.
///
/// # Errors
///
/// [`Faulted`] when initialising raised an `error.*` this document answered nowhere, or when its own
/// error handling failed in a chain the engine had to cut. Both mean the values a caller is about to
/// read are values nobody can vouch for.
pub fn opened<P: StatePolicy>(
    policy: P,
    serving: &crate::act::Serving,
) -> Result<Engine<P>, Faulted> {
    let mut machine = Engine::new(policy);
    serving.on(&mut machine);
    machine.initialize();
    match faults(&machine) {
        Some(faulted) => Err(faulted),
        None => Ok(machine),
    }
}

/// **WHAT THIS MACHINE HAS SWALLOWED SO FAR** — [`None`] for a machine that has swallowed nothing,
/// which is what every healthy run reads.
///
/// ⚠ Read mid-run as well as at the door: a document answers the errors its own states can match,
/// and an error raised once the machine has left them — in a `<final>`, or after the run is over —
/// has nobody left to answer it. [`crate::outer::OuterLoop::errors_nobody_answered`] is the loop's
/// own window onto the same fact, and the run's closing note is where it reaches a person.
#[must_use]
pub fn faults<P: StatePolicy>(machine: &Engine<P>) -> Option<Faulted> {
    let unanswered = machine.unhandled_error_events();
    let cascaded = machine.error_cascade_events();
    if unanswered == 0 && cascaded == 0 {
        return None;
    }
    Some(Faulted {
        unanswered,
        error: machine.last_unhandled_error().map(P::get_event_name),
        cascaded,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::sync::Arc;

    use sce_rust_runtime::IScriptEngine;

    use super::{faults, opened};

    /// Where this crate's sources are, for the two gates that read the tree they are compiled from.
    const SRC: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/src");

    /// Where SCE wrote the machines this crate compiled — the GENERATED half of every claim below.
    ///
    /// ⚠ `env!` rather than a path built by hand: this is the same directory `lib.rs` `include!`s
    /// the machines from, so a gate reading it is reading the artefact the crate is actually built
    /// out of. A hand-spelled `target/debug/build/...` would drift the first time a second build
    /// directory existed — and this workspace has eight of them.
    const GENERATED: &str = env!("OUT_DIR");

    /// ⛔⛔⛔⛔ **THE DOCUMENT FINGERPRINT IS THE DOCUMENTS, RECOMPUTED RATHER THAN TRUSTED** —
    /// register items 543 and 544.
    ///
    /// # Why this gate is the one holding the whole skew guard up
    ///
    /// A run that outlived its daemon records where it stopped, and a state name means something
    /// only against the document it came from. `PersistedRun::resumable_here` therefore hands the
    /// position back only when the recorded fingerprint equals this build's — which is item 544's
    /// *a changed document is a NEW run* expressed as data. **All of that is a comparison of one
    /// constant with another, and if the constant does not MOVE when a `.scxml` does, it reports
    /// every document unchanged for ever** — the exact skew it is named after, wearing its own
    /// name, with every gate around it green.
    ///
    /// ⚠⚠⚠ So this recomputes the number from the files, deliberately duplicating `build.rs`'s
    /// arithmetic — this module's own rule one gate down, that *"the runtime's copy is what the
    /// ENGINE uses; a gate that called it would be asking the subject to mark its own work."* The
    /// two spellings must agree, and a `build.rs` that stopped hashing content — emitting a
    /// version, a constant, a build stamp — disagrees at once.
    ///
    /// ⚠⚠ The source LIST comes from `build.rs` too, so the gate cannot pass by hashing a
    /// different set of files than the one the binary was compiled from.
    #[test]
    fn the_statechart_fingerprint_is_the_documents_it_names() {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;

        let sources: Vec<&str> = env!("SPRAG_STATECHART_SOURCES").split(',').collect();
        assert!(
            sources.len() > 1 && sources.iter().any(|path| path.ends_with("ai_loop.scxml")),
            "⚠⚠ the gate must be hashing the shipped documents, `ai_loop.scxml` among them: \
             {sources:?}",
        );

        let mut hash = OFFSET;
        for path in &sources {
            let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
            let body = std::fs::read(&full).unwrap_or_else(|why| panic!("read {full}: {why}"));
            for byte in path.as_bytes().iter().chain(body.iter()) {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(PRIME);
            }
        }

        assert_eq!(
            format!("{hash:016x}"),
            crate::STATECHARTS_FINGERPRINT,
            "⛔⛔⛔⛔ REGISTER ITEM 544: the baked fingerprint must be the CONTENT of the documents \
             this binary compiled. A number that does not follow the files cannot tell a changed \
             `ai_loop.scxml` from an unchanged one — so every interrupted run's recorded position \
             would be read back as though it were in this build's vocabulary, which is the version \
             skew the pair exists to make structurally impossible",
        );

        // ⚠⚠⚠ AND IT IS SENSITIVE TO A ONE-BYTE EDIT, proved rather than assumed: the assertion
        // above is satisfied by any function of the bytes, INCLUDING a bad one that collides
        // everything to the same value. Re-running the same arithmetic over the same files with a
        // single byte flipped must land somewhere else.
        let mut nudged = OFFSET;
        for (index, path) in sources.iter().enumerate() {
            let full = format!("{}/{path}", env!("CARGO_MANIFEST_DIR"));
            let mut body = std::fs::read(&full).unwrap_or_else(|why| panic!("read {full}: {why}"));
            if index == 0
                && let Some(first) = body.first_mut()
            {
                *first = first.wrapping_add(1);
            }
            for byte in path.as_bytes().iter().chain(body.iter()) {
                nudged ^= u64::from(*byte);
                nudged = nudged.wrapping_mul(PRIME);
            }
        }
        assert_ne!(
            format!("{nudged:016x}"),
            crate::STATECHARTS_FINGERPRINT,
            "⚠⚠⚠ a one-byte change to a document must change the fingerprint, or the comparison \
             above is satisfied by a constant and nothing is being tracked",
        );
    }

    /// **HOW A DOCUMENT'S MACHINE IS INITIALISED**, as the shipping code does it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    enum Road {
        /// Through [`opened`], which refuses a start-up that swallowed an error.
        Door,
        /// `Engine::new` + `initialize` by hand, so nothing asked.
        Bare,
    }

    /// W3C SCXML 5.9.3, the part these documents can reach: whether `name` matches `descriptor`.
    ///
    /// ⚠ Written here rather than borrowed from the runtime because the runtime's copy is what the
    /// ENGINE uses; a gate that called it would be asking the subject to mark its own work. The
    /// rules are the specification's: space-separated tokens, `*`, a `foo.*` suffix, an exact match,
    /// and a prefix that ends on a dot (`error` matches `error.execution`, `err` does not).
    fn descriptor_matches(name: &str, descriptor: &str) -> bool {
        descriptor.split_whitespace().any(|token| {
            token == "*"
                || token
                    .strip_suffix(".*")
                    .is_some_and(|prefix| name.starts_with(&format!("{prefix}.")))
                || token == name
                || name
                    .strip_prefix(token)
                    .is_some_and(|rest| rest.starts_with('.'))
        })
    }

    /// XML comments out, because every needle below (`<send>`, `cond=`, `error.execution`) is
    /// written about in prose in these files far more often than it is written as markup.
    ///
    /// ⚠ This is the difference between a gate and a word count. `ai_loop.scxml` is 3,800 lines of
    /// which the great majority is comment; reading them as content would have every state look
    /// covered and every rule look kept.
    fn without_comments(scxml: &str) -> String {
        let mut kept = String::with_capacity(scxml.len());
        let mut rest = scxml;
        while let Some(open) = rest.find("<!--") {
            kept.push_str(&rest[..open]);
            match rest[open..].find("-->") {
                Some(close) => rest = &rest[open + close + 3..],
                None => return kept,
            }
        }
        kept.push_str(rest);
        kept
    }

    /// One element of a document's state tree, as the coverage claim needs it.
    #[derive(Debug)]
    struct Element {
        /// Its `id`, for the message a red has to carry.
        id: String,
        /// Its parent's index in the tree, or [`None`] for a child of `<scxml>`.
        parent: Option<usize>,
        /// Whether anything in it can RAISE — `<onentry>`/`<onexit>` content, or a guard.
        runs_content: bool,
        /// The event descriptors of the transitions declared ON it.
        answers: Vec<String>,
        /// Whether it is a `<final>`, which takes no transitions and runs no guard.
        ending: bool,
    }

    /// The document's state tree, flattened, with what each state runs and what it answers.
    ///
    /// A two-pass shape is unavoidable and the reason is in `ai_loop.scxml` itself: the region's own
    /// rules sit at the FOOT of `work`, after every child — so a single pass reaching a child's
    /// content does not yet know whether an ancestor answers for it.
    fn tree(scxml: &str) -> Vec<Element> {
        let text = without_comments(scxml);
        let mut elements: Vec<Element> = Vec::new();
        let mut stack: Vec<usize> = Vec::new();
        let mut rest = text.as_str();
        while let Some(at) = rest.find('<') {
            let tail = &rest[at + 1..];
            let (tag, body) = match tail.find('>') {
                Some(end) => (&tail[..end], &tail[..end]),
                None => break,
            };
            // ⚠ A CLOSING TAG IS NOT A TAG WITH AN EMPTY NAME, which is what splitting on `/` makes
            // of `</state>` — the first draft of this parser popped nothing and read the whole
            // document as one flat level.
            let closing = tag.starts_with('/');
            let name = tag
                .trim_start_matches('/')
                .split([' ', '\t', '\n', '/', '>'])
                .next()
                .unwrap_or("");
            let self_closing = body.trim_end().ends_with('/');
            if closing {
                if matches!(name, "state" | "parallel" | "final") {
                    stack.pop();
                }
                rest = &tail[body.len()..];
                continue;
            }
            match name {
                "state" | "parallel" | "final" => {
                    let id = attribute(body, "id").unwrap_or_default();
                    let parent = stack.last().copied();
                    elements.push(Element {
                        id,
                        parent,
                        runs_content: attribute(body, "cond").is_some(),
                        answers: Vec::new(),
                        ending: name == "final",
                    });
                    if !self_closing {
                        stack.push(elements.len() - 1);
                    }
                }
                "transition" => {
                    if let Some(&owner) = stack.last() {
                        if let Some(event) = attribute(body, "event") {
                            elements[owner].answers.push(event);
                        }
                        // ⚠ A GUARD IS CONTENT, and it is the reachable failure in
                        // `context_review.scxml`: `cond="_event.data.records"` on an event carrying
                        // no data asks the datamodel to index nil, which raises where no `<assign>`
                        // is involved at all.
                        if attribute(body, "cond").is_some() {
                            elements[owner].runs_content = true;
                        }
                    }
                }
                "onentry" | "onexit" | "assign" | "send" | "raise" | "if" | "log" | "script"
                | "cancel" | "invoke" | "foreach" => {
                    if let Some(&owner) = stack.last() {
                        elements[owner].runs_content = true;
                    }
                }
                _ => {}
            }
            rest = &tail[body.len()..];
        }
        elements
    }

    /// One attribute of an element, as written.
    fn attribute(body: &str, name: &str) -> Option<String> {
        let key = format!("{name}=\"");
        let at = body.find(&key)?;
        let rest = &body[at + key.len()..];
        let end = rest.find('"')?;
        Some(rest[..end].to_owned())
    }

    /// **WHICH ERROR CLASSES THIS DOCUMENT'S GENERATED MACHINE CAN RAISE** — read off the machine's
    /// own event vocabulary, so the answer is the generator's and not a guess about it.
    fn raisable(stem: &str) -> BTreeSet<String> {
        let machine = std::fs::read_to_string(format!("{GENERATED}/{stem}_sm.rs"))
            .unwrap_or_else(|why| panic!("the machine this crate compiled for {stem:?}: {why}"));
        machine
            .lines()
            .filter_map(|line| line.split_once("=> \"error."))
            .filter_map(|(_, tail)| tail.split_once('"'))
            .map(|(name, _)| format!("error.{name}"))
            .collect()
    }

    /// **WHICH DOCUMENTS THIS CRATE DRIVES, FOUND BY THE ROAD ITS SHIPPING CODE TAKES** — a glob
    /// over the crate's own sources rather than a list, which is item 470's rule and item 498's
    /// mechanism.
    ///
    /// A construction site is a policy type being built: `<Camel>Policy::new`, whose document is its
    /// name in snake case. What separates SHIPPING from a fixture is the `#[cfg(test)] mod tests`
    /// boundary in each file — everything past it belongs to the gates, including every probe.
    fn driven() -> BTreeMap<String, Road> {
        let mut found = BTreeMap::new();
        let mut files: Vec<_> = std::fs::read_dir(SRC)
            .expect("this crate's own source directory")
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .collect();
        files.sort();
        for path in files {
            let source = std::fs::read_to_string(&path).expect("a source file this crate compiles");
            let shipping = match source.find("#[cfg(test)]\nmod tests {") {
                Some(at) => &source[..at],
                None => &source[..],
            };
            for (at, _) in shipping.match_indices("Policy::new") {
                let opens = shipping[..at].rfind(|c: char| !c.is_alphanumeric() && c != '_');
                let camel = &shipping[opens.map_or(0, |at| at + 1)..at];
                if camel.is_empty() {
                    continue;
                }
                let mut stem = String::new();
                for (index, letter) in camel.char_indices() {
                    if letter.is_uppercase() && index > 0 {
                        stem.push('_');
                    }
                    stem.extend(letter.to_lowercase());
                }
                // ⚠⚠ THE ROAD IS READ FROM THE WHOLE STATEMENT, and the first draft read the
                // characters immediately before the policy instead — which answered `Bare` for
                // every site in this crate, because a policy is reached through its module
                // (`crate::sm::debt_loop::DebtLoopPolicy`) and the door is several tokens further
                // left. The statement is the unit that either goes through the door or does not.
                let statement = shipping[..at]
                    .rfind([';', '{', '}'])
                    .map_or(0, |end| end + 1);
                let road = if shipping[statement..at].contains("opened(") {
                    Road::Door
                } else {
                    Road::Bare
                };
                found
                    .entry(stem)
                    .and_modify(|held| {
                        if road == Road::Door {
                            *held = Road::Door;
                        }
                    })
                    .or_insert(road);
            }
        }
        found
    }

    /// ⚠⚠⚠⚠⚠ **THE DOOR REFUSES A DOCUMENT THAT SWALLOWED ITS OWN ERROR, AND ADMITS THE ONE THAT
    /// ANSWERED IT** — one axis, two documents, neither of them mutated.
    ///
    /// # ⚠⚠ Why the subject is a pair and not an assertion about one document
    ///
    /// A refusal on its own is consistent with a door that refuses everything, and this door is
    /// about to stand in front of every machine this crate drives. `probe_unanswered.scxml` raises
    /// `error.execution` (a `<send>` naming a type nobody serves) and answers it NOWHERE;
    /// `probe_send_type.scxml` raises the same error from the same construct and has a transition
    /// for it. The two differ in exactly one thing — whether the document answers — so what this
    /// gate reads is attributable to the document rather than to the engine's mood.
    ///
    /// ⚠⚠⚠ **AND IT MEASURES WHEN THE FACT BECOMES VISIBLE**, which is the premise [`opened`] rests
    /// on: `Engine::initialize` runs the main event loop, so an error raised while entering the
    /// initial state is already counted before anybody steps the machine. If that were not true this
    /// door would be a door onto an empty reading, and the refusal would have to live at the first
    /// pump instead.
    #[test]
    fn a_document_that_swallows_an_error_while_it_starts_is_refused_at_the_door() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());

        let refused = opened(
            crate::sm::probe_unanswered_sm::ProbeUnansweredPolicy::new(Arc::clone(&lua)),
            // ⚠ THE PRODUCT'S OWN HOST, and it changes nothing here on purpose: this document's
            // subject is `x-nobody-serves-this`, a type the BUILD never declared, so the engine
            // refuses it before any registry is consulted. Passing the real host keeps this gate on
            // the road every driven document takes.
            &crate::act::Serving::new(),
        )
        .err()
        .expect(
            "⚠⚠⚠⚠⚠ `probe_unanswered.scxml` raises `error.execution` on its way in and answers no \
             error at all, so this door must refuse it. An `Ok` means the count is not yet visible \
             when `initialize` returns — and then every caller below is guarding with a reading \
             taken too early, which is the shape of item 497",
        );
        assert_eq!(
            refused.unanswered, 1,
            "exactly one error, so this is the document's start-up and not a cascade: {refused:?}",
        );
        assert_eq!(
            refused.error,
            Some("error.execution"),
            "⚠⚠⚠ and WHICH error, because the class names who repairs it — the document's own \
             content failed, which is not the same fault as a `<send>` nobody served: {refused:?}",
        );
        assert_eq!(
            refused.cascaded, 0,
            "⚠ nothing answered, so nothing can have answered badly: {refused:?}",
        );
        assert!(
            refused.to_string().contains("error.execution")
                && refused.to_string().contains("never ran"),
            "⚠⚠ the sentence a person reads must name the error AND what it cost — an error \
             abandons the rest of its block, which is why a half-run `onentry` looks like a slow \
             peer: {}",
            refused,
        );

        let admitted = opened(
            crate::sm::probe_send_type_sm::ProbeSendTypePolicy::new(lua),
            // ⚠⚠ AND HERE THE HOST IS PART OF THE SUBJECT. This document's send names
            // `x-sprag-host`, which the build DOES declare, so the refusal is now the host's own —
            // `reached.host` is not an act [`crate::act::Act`] serves — rather than the engine's.
            // It is still `error.execution`, which is [`crate::act`]'s claim: an act nobody
            // performs and a type nobody supports are one fact to the document.
            &crate::act::Serving::new(),
        )
        .expect(
            "⚠⚠⚠⚠⚠ THE CONTROL: `probe_send_type.scxml` raises the SAME `error.execution` from the \
             same construct and ANSWERS it, so it must come through. A refusal here would mean this \
             door turns away every document that ever raised an error, which would make the \
             assertion above about the engine instead of about the document",
        );
        assert!(
            faults(&admitted).is_none(),
            "and it must read clean afterwards, or the reading counts errors RAISED rather than \
             errors nobody answered",
        );
    }

    /// ⚠⚠⚠⚠⚠ **EVERY DOCUMENT THIS CRATE DRIVES ANSWERS EVERY ERROR ITS MACHINE CAN RAISE, OR IS
    /// OPENED THROUGH THE DOOR THAT REFUSES ONE** — register item 505's ratchet, and the reason the
    /// item is closed rather than patched.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a ratchet and not four edges and a sigh of relief
    ///
    /// Item 505 was filed because `ai_loop.scxml` and `debt_loop.scxml` carried ZERO `error.*`
    /// transitions between them for as long as they had existed, while eight documents in this
    /// crate could raise one. Adding the edges fixes today. What stops the SIXTH document from
    /// arriving without one is this — and item 453's finding is why it is written the way it is: *a
    /// blind ratchet is green forever, in exactly the voice of a working one*.
    ///
    /// So nothing here is spelled by hand:
    ///
    /// * the DOCUMENTS come from the ROAD this crate's shipping code takes to build a machine
    ///   ([`driven`]), not from a list — a policy constructed outside a `mod tests` is a document
    ///   this crate drives, and a new one is discovered by the compile that adds it;
    /// * the ERROR CLASSES come from the GENERATED machine's own event vocabulary ([`raisable`]),
    ///   so the day SCE mints `error.communication` for one of these files, that class is required
    ///   and a wildcard nobody measured is not what answers it;
    /// * the COVERAGE comes from the document's own tree, so a state added tomorrow is checked
    ///   tomorrow, and `<final>`-only documents — a kind is one — are answered by the DOOR instead,
    ///   which the same claim requires of them.
    ///
    /// ⚠⚠ AND THE SET IS PINNED, for item 498's reason exactly: a claim over a discovered set is
    /// green whether it discovered five documents or one, so the FIRST thing asserted is what was
    /// discovered. A second driven document is then ANNOUNCED rather than absorbed.
    #[test]
    fn every_document_this_crate_drives_answers_its_own_errors_or_is_refused_at_the_door() {
        /// What the road-glob must find. `Door` is [`opened`]; `Bare` is a machine built by hand,
        /// which is only allowed for a document whose generated event enum has no error at all.
        const PINNED: &[(&str, Road)] = &[
            ("ai_loop", Road::Door),
            ("context_review", Road::Door),
            ("debt_loop", Road::Door),
            // ⚠⚠ `datamodel="null"`, no guard, no `<send>`, no `<invoke>` — so the generator mints
            // no error variant for either, and there is nothing for a door to refuse. The exclusion
            // is DERIVED below rather than granted here: the day one of them grows a `cond`, its
            // machine gains the variant and this gate asks for the road.
            ("orchestration", Road::Bare),
            ("session", Road::Bare),
        ];

        let found = driven();
        assert_eq!(
            found,
            PINNED
                .iter()
                .map(|(stem, road)| ((*stem).to_owned(), *road))
                .collect::<BTreeMap<_, _>>(),
            "⚠⚠⚠⚠⚠ THE SUBJECTS MOVED. Either a document joined the ones this crate drives — in \
             which case decide, here, whether it answers its own errors or is opened through the \
             door — or one stopped being driven, or a construction site changed road. A union that \
             quietly grew would leave the new document unchecked, which is the state every document \
             in this crate was in until item 505",
        );
        assert!(
            !found.is_empty(),
            "⚠⚠⚠ and an EMPTY discovery is a refusal, not a pass: a glob that finds nothing is how \
             a ratchet goes blind while reading green",
        );

        for (stem, road) in &found {
            let errors = raisable(stem);
            if errors.is_empty() {
                assert_eq!(
                    *road,
                    Road::Bare,
                    "⚠ {stem} cannot raise an error at all, so the door has nothing to refuse — \
                     this is a note rather than a rule, and it fires only if the pin above and the \
                     generated machine disagree",
                );
                continue;
            }
            assert_eq!(
                *road,
                Road::Door,
                "⚠⚠⚠⚠⚠ {stem} CAN raise {errors:?} and its machine is initialised by hand, so an \
                 error raised before any state of it could answer is dropped by W3C SCXML 3.12.2 \
                 and the caller reads a document that came up fine. Open it through \
                 `document::opened`",
            );

            let scxml = std::fs::read_to_string(format!("{SRC}/{stem}.scxml"))
                .unwrap_or_else(|why| panic!("the document {stem:?} is compiled from: {why}"));
            let elements = tree(&scxml);
            let answered = |element: &Element, error: &str| {
                let mut at = Some(element);
                let mut walk = element.parent;
                while let Some(here) = at {
                    if here
                        .answers
                        .iter()
                        .any(|descriptor| descriptor_matches(error, descriptor))
                    {
                        return true;
                    }
                    at = walk.map(|index| &elements[index]);
                    walk = at.and_then(|here| here.parent);
                }
                false
            };
            for error in &errors {
                for element in &elements {
                    if element.ending || !element.runs_content {
                        continue;
                    }
                    assert!(
                        answered(element, error),
                        "⚠⚠⚠⚠⚠ {stem}'s `{}` RUNS EXECUTABLE CONTENT AND ANSWERS NO {error}. W3C \
                         SCXML 3.12.2 drops what nothing matches and W3C 3.8 abandons the rest of \
                         the block that raised it, so a failure there leaves this machine exactly \
                         where it was — measured on this very document: with its region edge \
                         deleted, a run whose `max_turns` guard could not be evaluated came back \
                         CONVERGED. Put the edge on this state or on an ancestor of it",
                        element.id,
                    );
                }
            }
        }
    }
}
