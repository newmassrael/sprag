//! Whether an event the loop's document reads `_event.data` off is ever handed on WITHOUT it —
//! register item 507.
//!
//! # ⚠⚠⚠⚠⚠ The silence this exists to end, measured
//!
//! `judging`'s `<onentry>` reads three keys off `_event.data` and every guard in that state reads
//! one, so `turn.done` and `judge` are DATA-CARRYING events. Fifteen fixture sites in
//! `ai_loop.rs` raised them through `process_event`, **which carries no `_event.data` at all** — so
//! the datamodel was asked to index nil, W3C SCXML 3.8 abandoned the rest of the entry block after
//! its first assignment, and W3C 3.12.2 dropped the error nothing matched. Those gates were GREEN
//! on a half-executed `judging` for as long as they had existed.
//!
//! They were found only when item 505 gave the document an edge that answers its own errors and
//! seven of them went red at once. ⚠⚠ That edge is a real detector and it is a BEHAVIOURAL one: a
//! bare raise now ends the run `failed`, so the next fixture written this way fails on the state it
//! lands in. What was missing is the STATIC half — nothing pairs *what the document reads* with
//! *what the Rust sends*, and nothing ever said the two had drifted.
//!
//! ⚠ `reflected`'s own doc had written the rule down — *"a fixture must reach a state by the door
//! the product uses"* — one screen above the sites that broke it. A recorded lesson is not an
//! applied one, which is why this is a gate and not a comment.
//!
//! # ⚠⚠⚠⚠⚠ Both halves are derived, which is what keeps the needle from going blind
//!
//! Item 453's finding, in one sentence: *a blind ratchet is green forever, in exactly the voice of
//! a working one*. So nothing here is a list:
//!
//! * the EVENTS come from `ai_loop.scxml` — every event whose own transition reads `_event.data`,
//!   plus every event that ENTERS a state whose `<onentry>` reads one
//!   ([`data_carrying`](crate::payload::data_carrying)). A tenth data-carrying event is discovered
//!   by the edit that adds it;
//! * the KEYS come from the same read, so the day `judging` reads a fourth number the fixtures owe
//!   it;
//! * the RAISERS come from this workspace's own Rust ([`Rust::of`](crate::payload::Rust::of)) — the
//!   engine's two doors and every function that forwards an event to one — so a sixteenth fixture
//!   helper is seen rather than walked past;
//! * the SITES are read through the names each file reaches the event type by
//!   ([`crate::loop_shape::reaching`]), so an alias or a glob import teaches the needle instead of
//!   blinding it.
//!
//! # ⚠⚠ What a text scan can and cannot claim here
//!
//! This crate takes no dependencies by charter ([`crate::sources`]), so nothing here understands
//! Rust: every claim below is about a site where the event is SPELLED. A driver that computes an
//! event into a variable and hands it on somewhere else is outside this gate's reach — `watch`
//! answers `AiLoopEvent::TurnDone` as a value and its caller decides what to attach — and that
//! residue is stated rather than implied. The fifteen sites the item was filed for, and every
//! payload this workspace writes down, are inside it.

use std::collections::{BTreeMap, BTreeSet};

use crate::loop_shape::{self, STATE_TYPE};
use crate::sources::Source;

/// The generated event type the driver and its fixtures name events through.
///
/// ⚠ GENERATED from the document by SCE, like [`STATE_TYPE`] beside it: the names are the
/// compiler's business and adding an event breaks every exhaustive match until a person has looked.
/// What no compiler checks is whether a raise carries the data the document reads — that is this
/// module's whole subject.
pub const EVENT_TYPE: &str = "AiLoopEvent";

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The DOCUMENT half: which events carry data, and which keys.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// Every event `scxml` reads `_event.data` off, with the keys it reads.
///
/// # The two shapes, and why the second one needs a second pass
///
/// | where the read is | whose event it is |
/// |---|---|
/// | on a transition — its `cond`, or executable content inside it | that transition's own `event` |
/// | in a state's `<onentry>` | the event of EVERY transition that targets that state |
///
/// The second is `judging`: three `<assign>`s reading `_event.data.context`, `cold` and `floor`,
/// and the only way in is `turn.done`. A reader that looked at transitions alone would call
/// `turn.done` data-free — which is exactly the reading fifteen fixtures were written under.
///
/// # ⚠⚠ What is deliberately NOT guessed
///
/// Entry through an ancestor's `initial`, or through a transition targeting a descendant, would
/// also run the block. Neither shape exists in this document and inventing an answer for them would
/// be a claim nothing measured, so the pairing is `target="…"` only.
///
/// # Panics
///
/// When an `<onexit>` reads `_event.data`. That read belongs to whichever event LEFT the state, and
/// this reader does not know which — a probe that cannot tell must never read as clean
/// ([`crate::sources`]'s own rule), so it says so rather than dropping the read on the floor.
#[must_use]
pub fn data_carrying(scxml: &str) -> BTreeMap<String, BTreeSet<String>> {
    walked(scxml).reads
}

/// The `(state, event)` pairs where a BARE raise of a data-carrying event is HARMLESS — the state
/// declares its own transition for it, that transition reads nothing, and what it enters reads
/// nothing either.
///
/// # ⚠⚠⚠⚠⚠ Why a gate needs this, and why the ledger's plan was wrong without it
///
/// Whether a payload is owed is **state-dependent**. `turn.blocked` owes three keys in `working` —
/// two `cond`s route on them — and owes NOTHING in `reflecting`, which answers it with one
/// unconditional edge to `awaiting_human`. So `OuterLoop::reflect` handing a bare `turn.blocked` on
/// through `ended.into()` is CORRECT, and item 515's original plan — *make `advance` refuse every
/// data-carrying event delivered with `data: None`* — would have refused what the document permits.
///
/// ⚠⚠ The static gate over SPELLED sites is deliberately stricter than this: always carrying is
/// harmless there and simpler to keep. This is for the other half — the indirect sites a text scan
/// cannot follow — where the driver is right BECAUSE it knows the state, and where the thing worth
/// watching is the day the document stops tolerating it.
#[must_use]
pub fn tolerant(scxml: &str) -> BTreeSet<(String, String)> {
    let read = walked(scxml);
    // ⚠⚠⚠⚠⚠ AGGREGATED PER `(state, event)`, NOT PER TRANSITION — and the first version of this
    // was per transition, which called `judging`/`judge` and `working`/`turn.blocked` TOLERANT.
    // They are the opposite: each declares several transitions for that event and the conditional
    // ones are evaluated in document order until one is taken, so a bare raise indexes nil on the
    // FIRST guard whatever the last unconditional fall-through would have done. One safe edge among
    // several is not a safe state.
    let mut safe: BTreeMap<(String, String), bool> = BTreeMap::new();
    for row in &read.declared {
        if !read.reads.contains_key(&row.event) {
            continue;
        }
        let clean = !row.reads
            && read
                .on_entry
                .get(&row.target)
                .is_none_or(BTreeSet::is_empty);
        let held = safe
            .entry((row.state.clone(), row.event.clone()))
            .or_insert(true);
        *held = *held && clean;
    }
    safe.into_iter()
        .filter(|(_, clean)| *clean)
        .map(|(pair, _)| pair)
        .collect()
}

/// One transition a state declares for an event, and what taking it reads.
struct Declared {
    /// The state that declares it.
    state: String,
    /// Its `event` descriptor.
    event: String,
    /// Where it goes, so entering that state can be asked about too.
    target: String,
    /// Whether the transition itself reads `_event.data` — its `cond` or its own content.
    reads: bool,
}

/// What one pass over the document found, so the two questions above cannot drift apart.
struct Walked {
    /// Every event that reads `_event.data`, with the keys — entry reads already folded in.
    reads: BTreeMap<String, BTreeSet<String>>,
    /// Per state, what its `<onentry>` reads.
    on_entry: BTreeMap<String, BTreeSet<String>>,
    /// Every transition, by the state that declares it.
    declared: Vec<Declared>,
}

/// The single pass. See [`data_carrying`] for what it is reading for.
fn walked(scxml: &str) -> Walked {
    /// One element still open above the reader, for attributing a read to an event.
    enum Open {
        /// A `<state>`, `<parallel>` or `<final>`, by its id.
        State(String),
        /// An `<onentry>` — what runs when an event ARRIVES at the state below.
        Entry,
        /// An `<onexit>`, whose event this reader cannot name.
        Exit,
        /// A `<transition>`, by its `event` descriptor when it declares one.
        Transition(Option<String>),
    }

    let text = without_comments(scxml);
    let mut reads: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut on_entry: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut into: Vec<(String, String)> = Vec::new();
    let mut declared: Vec<Declared> = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut rest = text.as_str();

    while let Some(open) = rest.find('<') {
        let tail = &rest[open + 1..];
        let Some(end) = tail.find('>') else { break };
        let body = &tail[..end];
        rest = &tail[end..];
        let closing = body.starts_with('/');
        let name = body
            .trim_start_matches('/')
            .split([' ', '\t', '\n', '/', '>'])
            .next()
            .unwrap_or("");
        if closing {
            if matches!(
                name,
                "state" | "parallel" | "final" | "onentry" | "onexit" | "transition"
            ) {
                stack.pop();
            }
            continue;
        }

        let keys = data_keys(body);
        if name == "transition" {
            if let Some(event) = attribute(body, "event") {
                if !keys.is_empty() {
                    reads.entry(event.clone()).or_default().extend(keys.clone());
                }
                // ⚠ WHICH STATE DECLARES IT, and where it goes — the two facts that decide whether
                // a BARE raise of this event is safe while the machine sits in that state.
                declared.push(Declared {
                    state: stack
                        .iter()
                        .rev()
                        .find_map(|above| match above {
                            Open::State(id) => Some(id.clone()),
                            _ => None,
                        })
                        .unwrap_or_default(),
                    event: event.clone(),
                    target: attribute(body, "target").unwrap_or_default(),
                    reads: !keys.is_empty(),
                });
                if let Some(target) = attribute(body, "target") {
                    into.push((target, event));
                }
            }
        } else if !keys.is_empty() {
            let mut entering = false;
            for above in stack.iter().rev() {
                match above {
                    Open::Transition(Some(event)) => {
                        reads.entry(event.clone()).or_default().extend(keys.clone());
                        // ⚠ A READ IN THE TRANSITION'S CONTENT counts as the transition reading —
                        // `<assign expr="_event.data.text"/>` owes a payload exactly as a `cond`
                        // does. Transitions do not nest, so the one still open is the last pushed.
                        if let Some(row) = declared.last_mut() {
                            row.reads = true;
                        }
                        break;
                    }
                    Open::Transition(None) => break,
                    Open::Entry => entering = true,
                    Open::Exit => panic!(
                        "⚠⚠⚠⚠⚠ an `<onexit>` reads `_event.data` ({keys:?}). That is the event \
                         that LEFT the state, and this reader pairs reads with the events that \
                         ARRIVE — so it cannot say which raise owes this payload. Teach it the \
                         shape rather than letting the read go uncounted",
                    ),
                    Open::State(id) => {
                        if entering {
                            on_entry.entry(id.clone()).or_default().extend(keys.clone());
                        }
                        break;
                    }
                }
            }
        }

        if !body.trim_end().ends_with('/') {
            match name {
                "state" | "parallel" | "final" => {
                    stack.push(Open::State(attribute(body, "id").unwrap_or_default()));
                }
                "onentry" => stack.push(Open::Entry),
                "onexit" => stack.push(Open::Exit),
                "transition" => stack.push(Open::Transition(attribute(body, "event"))),
                _ => {}
            }
        }
    }

    // ⚠ THE SECOND PASS, and the reason it cannot be folded into the first: a state's `<onentry>`
    // is written above the transitions that reach it as often as below, so a single walk arriving
    // at `judging`'s assignments does not yet know that `turn.done` is what runs them.
    for (target, event) in into {
        if let Some(keys) = on_entry.get(&target) {
            reads.entry(event).or_default().extend(keys.iter().cloned());
        }
    }
    Walked {
        reads,
        on_entry,
        declared,
    }
}

/// The variant the code generator spells a document EVENT name as — `turn.done` becomes `TurnDone`.
///
/// ⚠ Both separators, because this document uses both: `turn.done` and `reflect.applied` are dotted
/// and a `<data>`-style name would be underscored. [`crate::loop_shape::variant_of`] is the same
/// rule for state ids, which have only the one separator.
#[must_use]
pub fn variant_of_event(event: &str) -> String {
    event
        .split(['.', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect()
}

/// Every `_event.data.<key>` named in one element's attributes.
fn data_keys(body: &str) -> BTreeSet<String> {
    const NEEDLE: &str = "_event.data.";
    body.match_indices(NEEDLE)
        .map(|(at, _)| {
            body[at + NEEDLE.len()..]
                .chars()
                .take_while(|char| char.is_alphanumeric() || *char == '_')
                .collect::<String>()
        })
        .filter(|key| !key.is_empty())
        .collect()
}

/// The value of `name="…"` in one element's body.
fn attribute(body: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let at = body.find(&key)?;
    let rest = &body[at + key.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

/// `scxml` with every XML comment gone.
///
/// ⚠ This is the difference between a gate and a word count. `ai_loop.scxml` is mostly comment, and
/// its prose discusses `_event.data.rule` and `cond="…"` far more often than the markup uses them —
/// so a reader that took comments as content would invent events nobody raises. `sprag-plugin` keeps
/// its own copy for its own crate's gates; this one is here because that one lives inside a
/// `#[cfg(test)]` module and cannot cross a crate boundary.
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

// ─────────────────────────────────────────────────────────────────────────────────────────────
// The RUST half: which calls hand an event to a machine, and what they do with a payload.
// ─────────────────────────────────────────────────────────────────────────────────────────────

/// What a raiser does with whatever stands beside the event.
///
/// ⚠⚠ The three are not decoration: they decide whether an EMPTY thing beside the event is a bare
/// raise. `carried(&mut engine, event, "")` hands `""` straight to the machine and is the defect;
/// `reflected(&mut engine, event, "")` builds `{"standing": ""}` around it and is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Handing {
    /// It cannot carry a payload at all — `process_event`, and the envelope's `From<AiLoopEvent>`.
    /// Every data-carrying event handed this way is bare.
    Nothing,
    /// Whatever stands beside the event reaches the machine unchanged, so an empty one is an empty
    /// `_event.data`.
    Forwards,
    /// What stands beside the event is built INTO a payload, so an empty one still carries its key.
    Composes,
}

/// One callee that hands an event to a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Raiser {
    /// What it does with whatever stands beside the event.
    pub handing: Handing,
    /// Which of its arguments IS the event, zero-indexed — so the one after it is the payload.
    ///
    /// ⚠⚠ Read from the callee's own signature rather than assumed, because this workspace's
    /// helpers do not agree about it: the engine takes the event FIRST (`raise_external(event,
    /// data, "")`) and every fixture helper takes the machine first (`carried(engine, event,
    /// data)`). A reader that assumed either would call the other's payload the event.
    pub event_at: usize,
}

/// One place this workspace spells a data-carrying event where it is handed on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spelled {
    /// The file, relative to the workspace root.
    pub file: String,
    /// One-indexed line of the event itself.
    pub line: usize,
    /// The document's own name for the event — `turn.done`, not `TurnDone`, so a refusal talks in
    /// the words the document uses.
    pub event: String,
    /// How it is handed on, as written — `engine.process_event`, `Raise::carrying`, `(a pair)`.
    pub through: String,
    /// What stands beside it, as written, or [`None`] when nothing does.
    pub payload: Option<String>,
    /// Whether that amounts to a payload at all. `false` is the defect item 507 was filed for.
    pub carries: bool,
    /// Whether the site is in [`Source::product`] — the driver — rather than in a fixture.
    pub shipping: bool,
}

/// What this workspace's Rust says about raising, read once so every claim below shares it.
#[derive(Debug)]
pub struct Rust {
    /// Every callee that hands an event to a machine, and what it does with a payload.
    raisers: BTreeMap<String, Raiser>,
    /// The type the driver wraps an event in when it has a payload to attach — discovered as the
    /// `X` of `impl From<AiLoopEvent> for X`.
    envelope: Option<String>,
    /// Every `const NAME: &str = "…";` in the workspace, for resolving a payload or a key spelled
    /// as a name — as name to the DISTINCT VALUES that name has in this workspace.
    ///
    /// # ⚠⚠⚠⚠⚠ A set, not a value, because the same name is eight different keys here
    ///
    /// A payload writes its keys as `Readiness::WIRE_KEY`, and a reader that resolved that by the
    /// LAST PATH SEGMENT would be choosing between **eight distinct `WIRE_KEY` constants** —
    /// `may_answer`, `hand`, `screen_rules`, `handback_still_ms`, `await_person_ms`,
    /// `ready_timeout_ms`, `turn_within_ms`, `match` — three of which live in `readiness.rs` alone.
    /// Whichever file was read last would win, and the claim built on it would be a confident lie.
    ///
    /// ⚠⚠ So an ambiguous name resolves to NOTHING ([`Rust::one`]) and the token is left standing,
    /// which makes a claim about it go RED naming the token rather than pass on a guess. Measured
    /// 2026-08-21: no constant the gate resolves TODAY is ambiguous, so this is a hazard being
    /// closed before item 516 walks into it, not a defect being repaired.
    strings: BTreeMap<String, BTreeSet<String>>,
    /// Function bodies by name, for a payload spelled as a call.
    bodies: BTreeMap<String, Vec<String>>,
}

impl Rust {
    /// Read `sources` for everything the claims need.
    ///
    /// # ⚠⚠⚠⚠⚠ The raiser set is a CLOSURE, not a list
    ///
    /// It starts at the engine's two doors — `process_event`, which cannot carry data, and
    /// `raise_external`, which is the one that can — and then grows: any function taking the event
    /// type whose body reaches a raiser is itself one. That is what finds `carried`, `reflected`
    /// and the two table walkers without any of them being named here, and it is what will find the
    /// sixteenth helper somebody writes.
    ///
    /// ⚠ The two seeds are the RUNTIME's API and are spelled rather than derived — this crate takes
    /// no dependencies, so the engine's source is not on the road. They are asserted PRESENT by the
    /// gate, so a rename upstream is announced rather than absorbed.
    #[must_use]
    pub fn of(sources: &[Source]) -> Self {
        let mut strings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for source in sources {
            // ⚠⚠⚠ A CONSTANT IS RECORDED UNDER BOTH ITS NAMES — `WIRE_KEY` and
            // `Readiness::WIRE_KEY` — because the bare one is not enough to tell eight of them
            // apart, and the qualified one is how every payload in this workspace spells it.
            let mut depth: i32 = 0;
            let mut owner: Vec<(String, i32)> = Vec::new();
            for (_, line) in &source.code {
                if let Some(name) = impl_type(line) {
                    owner.push((name, depth));
                }
                if let Some((name, value)) = string_const(line) {
                    if let Some((declaring, _)) = owner.last() {
                        strings
                            .entry(format!("{declaring}::{name}"))
                            .or_default()
                            .insert(value.clone());
                    }
                    strings.entry(name).or_default().insert(value);
                }
                depth += i32::try_from(line.matches('{').count()).unwrap_or_default();
                depth -= i32::try_from(line.matches('}').count()).unwrap_or_default();
                while owner.last().is_some_and(|(_, opened)| depth <= *opened) {
                    owner.pop();
                }
            }
        }

        let mut bodies: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut defined: Vec<(String, String, String)> = Vec::new();
        let mut envelope = None;
        for source in sources {
            if !source
                .code
                .iter()
                .any(|(_, line)| line.contains(EVENT_TYPE))
            {
                continue;
            }
            let text = Squeezed::of(source);
            if envelope.is_none() {
                envelope = text.after(&format!("implFrom<{EVENT_TYPE}>for"));
            }
            for (name, params, body) in text.functions() {
                bodies.entry(name.clone()).or_default().push(body.clone());
                if params.contains(EVENT_TYPE) {
                    defined.push((name, params, body));
                }
            }
        }

        let mut raisers = BTreeMap::from([
            (
                "process_event".to_owned(),
                Raiser {
                    handing: Handing::Nothing,
                    event_at: 0,
                },
            ),
            (
                "raise_external".to_owned(),
                Raiser {
                    handing: Handing::Forwards,
                    event_at: 0,
                },
            ),
        ]);
        loop {
            let mut grew = false;
            for (name, params, body) in &defined {
                if raisers.contains_key(name) {
                    continue;
                }
                let Some(reached) = raisers
                    .iter()
                    .filter(|(called, _)| calls(body, called))
                    .map(|(called, raiser)| (called.clone(), *raiser))
                    .max_by_key(|(_, raiser)| raiser.handing)
                else {
                    continue;
                };
                // ⚠⚠ THE CLASS COMES FROM THE CALL IT MAKES, not from its own shape: a helper that
                // hands its argument through unchanged makes an empty argument an empty payload,
                // and one that wraps it does not. Which argument to look at comes from the callee's
                // own signature, so a helper that takes the machine first is read correctly.
                let handing = if reached.1.handing == Handing::Nothing {
                    Handing::Nothing
                } else if forwarded(body, &reached.0, reached.1.event_at) {
                    Handing::Forwards
                } else {
                    Handing::Composes
                };
                raisers.insert(
                    name.clone(),
                    Raiser {
                        handing,
                        event_at: event_at(params),
                    },
                );
                grew = true;
            }
            if !grew {
                break;
            }
        }

        Self {
            raisers,
            envelope,
            strings,
            bodies,
        }
    }

    /// Every callee that hands an event to a machine, and what it does with a payload.
    #[must_use]
    pub fn raisers(&self) -> &BTreeMap<String, Raiser> {
        &self.raisers
    }

    /// The type an event is wrapped in when a payload travels with it, when this workspace has one.
    #[must_use]
    pub fn envelope(&self) -> Option<&str> {
        self.envelope.as_deref()
    }

    /// The keys a payload names, or [`None`] when it is not one this reader can read.
    ///
    /// # The four shapes this workspace writes, and the one it cannot follow
    ///
    /// A `json!({…})` object, a JSON string literal, a name that resolves to one, and a call whose
    /// body builds one. What it cannot follow is a payload assembled elsewhere and passed in a
    /// variable — `brief`'s is built a screen away — and [`None`] is how it says so, so a claim
    /// built on this SKIPS what it cannot read rather than calling it empty.
    #[must_use]
    pub fn keys_of(&self, payload: &str) -> Option<BTreeSet<String>> {
        let expr = payload
            .trim_start_matches('&')
            .trim_end_matches(".to_string()")
            .trim_start_matches('&');
        if let Some(object) = expr
            .strip_prefix("serde_json::json!(")
            .or_else(|| expr.strip_prefix("json!("))
            .and_then(|rest| rest.strip_suffix(')'))
        {
            return Some(self.object_keys(object));
        }
        if let Some(json) = literal(expr) {
            return Some(self.object_keys(&json));
        }
        if let Some(named) = self.one(expr) {
            return Some(self.object_keys(&named));
        }
        if let Some(bound) = self.bound(expr) {
            return Some(bound);
        }
        let called = expr
            .strip_prefix("self.")
            .unwrap_or(expr)
            .split('(')
            .next()
            .unwrap_or_default();
        let body = self
            .bodies
            .get(called)?
            .iter()
            .find(|body| body.contains("json!({"))?;
        let at = body.find("json!({")? + "json!(".len();
        Some(self.object_keys(&balanced(body, at)))
    }

    /// The keys of a payload handed over as a LOCAL BINDING — `let payload = json!({…})` a few
    /// lines above the raise, which is how the widest payload in this document is written.
    ///
    /// # ⚠⚠⚠⚠ Why this refuses a name bound more than once
    ///
    /// `brief` is raised as `raise_external(AiLoopEvent::Brief, &payload.to_string(), "")` and the
    /// object is built just above it — the eighteen `<data>` names the document reads. Following
    /// the binding is the only way a gate ever sees them.
    ///
    /// ⚠ But `payload` is an ordinary name: measured 2026-08-21, **one** binding of it exists in the
    /// files this reader scans and **fourteen** exist workspace-wide. A follower that took the first
    /// one it found would be the same confident lie the constants were — so two bindings that do
    /// not agree answer [`None`], and the claim goes unread rather than wrong.
    fn bound(&self, name: &str) -> Option<BTreeSet<String>> {
        if name.is_empty() || !name.chars().all(is_ident) {
            return None;
        }
        let mut found: Option<BTreeSet<String>> = None;
        for bodies in self.bodies.values() {
            for body in bodies {
                for lead in [format!("let{name}="), format!("letmut{name}=")] {
                    let Some(at) = body.find(&lead) else { continue };
                    let value = &body[at + lead.len()..];
                    let opens = ["serde_json::json!({", "json!({"]
                        .into_iter()
                        .find(|start| value.starts_with(start))?;
                    let object = balanced(
                        body,
                        at + lead.len() + opens.len() - 1, // at the `{`
                    );
                    let keys = self.object_keys(&object);
                    match &found {
                        Some(already) if *already != keys => return None,
                        Some(_) => {}
                        None => found = Some(keys),
                    }
                }
            }
        }
        found
    }

    /// Whether `payload` is a name this workspace SHARES rather than one site's own literal.
    ///
    /// ⚠ That distinction is item 507's residue in one predicate: an inline literal is one probe
    /// asking one guard a narrow question, while a name reused across fifteen sites is a second
    /// spelling of the driver's own payload — and two spellings in two files with nothing holding
    /// them together is what the repayment itself introduced.
    #[must_use]
    pub fn shared(&self, payload: &str) -> bool {
        self.strings.contains_key(payload)
    }

    /// What `name` spells, when this workspace agrees about it — [`None`] when it is declared
    /// nowhere, and [`None`] when it is declared TWICE WITH DIFFERENT VALUES.
    ///
    /// ⚠⚠⚠ The second case is the whole point. *A probe that cannot tell must never read as clean*
    /// ([`crate::sources`]'s rule), and the alternative here is worse than unclean: picking one of
    /// two values would put a key nobody wrote into a claim about what the driver sends.
    fn one(&self, name: &str) -> Option<String> {
        let values = self.strings.get(name)?;
        match values.len() {
            1 => values.iter().next().cloned(),
            _ => None,
        }
    }

    /// Every constant name this workspace declares more than once with DIFFERENT values.
    ///
    /// ⚠⚠ Exposed so a gate can refuse a payload that spells one, rather than this reader silently
    /// declining to resolve it — a key left unresolved is a key that cannot match, and a claim that
    /// went red without saying WHY would send the next reader hunting the wrong thing.
    #[must_use]
    pub fn ambiguous(&self) -> BTreeMap<String, BTreeSet<String>> {
        self.strings
            .iter()
            .filter(|(_, values)| values.len() > 1)
            .map(|(name, values)| (name.clone(), values.clone()))
            .collect()
    }

    /// The top-level keys of a squeezed `{…}` object, names resolved through the workspace's
    /// constants.
    ///
    /// ⚠ Top level only, and on purpose: `"checked": match … { Some(word) => … }` holds a `:` of
    /// its own inside braces, and a reader that counted those would report keys nobody wrote.
    fn object_keys(&self, object: &str) -> BTreeSet<String> {
        let chars: Vec<char> = object.chars().collect();
        let mut keys = BTreeSet::new();
        let mut depth: i32 = 0;
        let mut at = 0;
        while at < chars.len() {
            match chars[at] {
                '"' => {
                    let mut end = at + 1;
                    while end < chars.len() && chars[end] != '"' {
                        end += if chars[end] == '\\' { 2 } else { 1 };
                    }
                    if depth == 1
                        && chars.get(end + 1) == Some(&':')
                        && chars.get(end + 2) != Some(&':')
                    {
                        keys.insert(chars[at + 1..end.min(chars.len())].iter().collect());
                    }
                    at = end + 1;
                    continue;
                }
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                ':' if depth == 1
                    && chars.get(at + 1) != Some(&':')
                    && (at == 0 || chars[at - 1] != ':') =>
                {
                    let mut start = at;
                    while start > 0 && (is_ident(chars[start - 1]) || chars[start - 1] == ':') {
                        start -= 1;
                    }
                    let token: String = chars[start..at].iter().collect();
                    // ⚠⚠⚠⚠ THE DECLARING TYPE IS THE LAST TWO SEGMENTS, not the whole path: this
                    // payload spells `crate::readiness::Readiness::WIRE_KEY` while the declaration
                    // is registered as `Readiness::WIRE_KEY`. Measured — the first version of this
                    // reader resolved fourteen of `brief`'s eighteen keys and left these four
                    // standing as their own paths, which is what a `use`-shortened spelling and a
                    // fully-qualified one look like side by side in one object.
                    let segments: Vec<&str> = token.split("::").collect();
                    let declared = if segments.len() >= 2 {
                        segments[segments.len() - 2..].join("::")
                    } else {
                        String::new()
                    };
                    if let Some(name) = token.rsplit("::").next().filter(|name| !name.is_empty()) {
                        // ⚠⚠⚠ THE TOKEN STANDS when the workspace does not agree what the name
                        // means — see [`Rust::one`]. An unresolved `WIRE_KEY` matches no key the
                        // document reads, so the claim goes RED naming it; a guess would go GREEN
                        // on a key nobody wrote.
                        // ⚠⚠⚠⚠ BOTH NAMES ARE ASKED AND THE ORDER IS NOT WHAT SAVES THIS — measured
                        // by flipping it, which changed nothing. What saves it is that the BARE
                        // name accumulates every value it has, so a contested `WIRE_KEY` answers
                        // `None` and the qualified `Readiness::WIRE_KEY` is what resolves. Deleting
                        // the qualified registration in [`Rust::of`] is the mutation that reds.
                        keys.insert(
                            self.one(&token)
                                .or_else(|| self.one(&declared))
                                .or_else(|| self.one(name))
                                .unwrap_or_else(|| token.clone()),
                        );
                    }
                }
                _ => {}
            }
            at += 1;
        }
        keys
    }
}

/// Every place a data-carrying event is spelled where something is done with it.
///
/// # ⚠⚠⚠⚠ What is DECLINED, and why each declension is not a hole
///
/// A mention is only a subject when the event stands as an argument. A match arm
/// (`AiLoopEvent::Judge => …`), a comparison (`raised == AiLoopEvent::Judge`) and a list of events
/// (`[AiLoopEvent::ReviewDone, AiLoopEvent::ReviewNone]`) each name an event without handing it
/// anywhere, and a gate that read them as raises would be red on every file that reasons about the
/// vocabulary.
///
/// ⚠ For the same reason, a neighbour that is itself a word of the machine's vocabulary
/// (`AiLoopEvent::…`, `AiLoopState::…`) is not a payload: that is a table pairing two facts, not an
/// event beside its data. `AiLoop::walked(AiLoopState::Judging, AiLoopEvent::Judge,
/// AiLoopState::Working, …)` renders a transition and raises nothing — and it shares its name with
/// a fixture helper that does raise, which is exactly the collision a text scan cannot resolve.
#[must_use]
pub fn spelled(
    sources: &[Source],
    carrying: &BTreeMap<String, BTreeSet<String>>,
    rust: &Rust,
) -> Vec<Spelled> {
    let variants: Vec<(String, String)> = carrying
        .keys()
        .map(|event| (variant_of_event(event), event.clone()))
        .collect();
    let mut found = Vec::new();
    for source in sources {
        let reaching = loop_shape::reaching(&source.code, EVENT_TYPE);
        if reaching.paths.is_empty() {
            continue;
        }
        let text = Squeezed::of(source);
        let shipping: BTreeSet<usize> = source.product.iter().map(|(line, _)| *line).collect();
        for (variant, event) in &variants {
            for at in text.variants(variant, &reaching) {
                if let Some(site) = text.handed(at, variant, rust) {
                    found.push(Spelled {
                        file: source.file.clone(),
                        line: text.line(at),
                        event: event.clone(),
                        shipping: shipping.contains(&text.line(at)),
                        ..site
                    });
                }
            }
        }
    }
    found.sort_by(|left, right| {
        (&left.file, left.line, &left.event).cmp(&(&right.file, right.line, &right.event))
    });
    found
}

/// Every place the DRIVER hands an event on through a VARIABLE — `(function, the name converted)`.
///
/// # ⚠⚠⚠⚠⚠ Why these sites need finding rather than remembering
///
/// A raise this gate can read spells its event: `Raise::carrying(AiLoopEvent::TurnDone, …)`. These
/// do not — `ended.into()` converts whatever `watch` answered, and no text scan can say which event
/// that is. They are safe today because of what the DOCUMENT does in the states they run in, which
/// [`tolerant`] pins.
///
/// ⚠⚠ But that pin names its driver site in PROSE, so deleting the site would leave the pin green
/// and guarding nobody — item 453's shape once more. This is the other end of the tie: the sites
/// are DISCOVERED, so one that stops existing is announced by the same gate that pins what it needs.
///
/// Only [`Source::product`] is read: a fixture converting something is not a driver hand-off.
/// Measured 2026-08-21 — the whole workspace has exactly two, both in the loop's driver.
#[must_use]
pub fn indirect(sources: &[Source]) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for source in sources {
        if !source.product.iter().any(|(_, l)| l.contains(EVENT_TYPE)) {
            continue;
        }
        let text = Squeezed::of_lines(&source.product);
        for (name, _, body) in text.functions() {
            let mut at = 0;
            while let Some(hit) = body[at..].find(".into()") {
                let end = at + hit;
                let mut start = end;
                while start > 0 && is_ident(body[..start].chars().next_back().unwrap_or(' ')) {
                    start -= 1;
                }
                let receiver = &body[start..end];
                // ⚠ A BARE LOWERCASE NAME is a value; `AiLoopEvent::Null.into()` is a spelled raise
                // and belongs to the claim that CAN read it, not to this one.
                let qualified = body[..start].ends_with(':') || body[..start].ends_with('.');
                if !qualified
                    && !receiver.is_empty()
                    && receiver.starts_with(|c: char| c.is_lowercase() || c == '_')
                {
                    found.insert((name.clone(), receiver.to_owned()));
                }
                at = end + ".into()".len();
            }
        }
    }
    found
}

/// One source file with every space gone, and the line each character came from.
///
/// ⚠⚠ Squeezed because rustfmt decides by line width whether a call is one line or five, so a
/// reader that worked line by line would see `carried(&mut engine, EVENT, DATA)` and miss the same
/// call broken across four. ⚠ Held as `char`s rather than a `String` because these lines carry
/// non-ASCII — every refusal message in this workspace does — and byte arithmetic on them is a
/// panic waiting for the first assertion that says `⚠`.
struct Squeezed {
    chars: Vec<char>,
    lines: Vec<usize>,
}

impl Squeezed {
    /// `source`'s code with the whitespace out and a line number kept per character.
    fn of(source: &Source) -> Self {
        Self::of_lines(&source.code)
    }

    /// The same, over whichever half of a source the caller means — [`Source::code`] for the
    /// fixtures too, [`Source::product`] for what SHIPS.
    fn of_lines(lines_of: &[(usize, String)]) -> Self {
        let mut chars = Vec::new();
        let mut lines = Vec::new();
        for (line, text) in lines_of {
            for char in text.chars().filter(|char| !char.is_whitespace()) {
                chars.push(char);
                lines.push(*line);
            }
        }
        Self { chars, lines }
    }

    /// The line character `at` came from.
    fn line(&self, at: usize) -> usize {
        self.lines.get(at).copied().unwrap_or_default()
    }

    /// Whether `needle` starts at `at`.
    fn starts(&self, at: usize, needle: &str) -> bool {
        needle
            .chars()
            .enumerate()
            .all(|(step, char)| self.chars.get(at + step) == Some(&char))
    }

    /// The text between two character positions.
    fn text(&self, from: usize, to: usize) -> String {
        self.chars[from.min(self.chars.len())..to.min(self.chars.len())]
            .iter()
            .collect()
    }

    /// The identifier written immediately after `needle`, the first time it occurs.
    fn after(&self, needle: &str) -> Option<String> {
        let at =
            (0..self.chars.len()).find(|at| self.starts(*at, needle))? + needle.chars().count();
        let name: String = self.chars[at..]
            .iter()
            .take_while(|char| is_ident(**char))
            .collect();
        (!name.is_empty()).then_some(name)
    }

    /// Where `variant` is named, through any of the names this file reaches the event type by.
    fn variants(&self, variant: &str, reaching: &loop_shape::Reaching) -> Vec<usize> {
        let mut found = Vec::new();
        for at in 0..self.chars.len() {
            if !self.starts(at, variant) {
                continue;
            }
            let after = at + variant.chars().count();
            if self.chars.get(after).copied().is_some_and(is_ident) {
                continue;
            }
            let qualified = at >= 2
                && self.chars[at - 2] == ':'
                && self.chars[at - 1] == ':'
                && reaching.paths.iter().any(|path| self.ends_on(at - 2, path));
            let bare = reaching.glob && (at == 0 || !is_ident(self.chars[at - 1]));
            if qualified || bare {
                found.push(at);
            }
        }
        found
    }

    /// Whether the path segment ending just before `at` is `word`.
    fn ends_on(&self, at: usize, word: &str) -> bool {
        let len = word.chars().count();
        at >= len
            && self.starts(at - len, word)
            && (at == len || !is_ident(self.chars[at - len - 1]))
    }

    /// What is done with the event named at `at`, or [`None`] when nothing is.
    fn handed(&self, at: usize, variant: &str, rust: &Rust) -> Option<Spelled> {
        let after = at + variant.chars().count();
        // ⚠ THE ENVELOPE'S OWN CONVERSION, which carries no data by construction — `impl
        // From<AiLoopEvent> for Raise` sets `data: None`. It is the driver's way of committing the
        // same defect the fifteen fixtures committed with `process_event`.
        if self.starts(after, ".into()") {
            return Some(self.site(at, ".into()".to_owned(), None, false));
        }
        let (opener, callee) = self.enclosing(at)?;
        if self.chars.get(opener) != Some(&'(') {
            return None;
        }
        let envelope_road = rust
            .envelope()
            .is_some_and(|envelope| callee.starts_with(&format!("{envelope}::")));
        let short = callee.rsplit(['.', ':']).next().unwrap_or_default();
        let handing = if envelope_road {
            Some(Handing::Composes)
        } else if callee.is_empty() {
            // A pair is a pairing: whatever stands beside the event in a table IS its payload, and
            // the walkers that consume these tables hand it straight to the machine.
            Some(Handing::Forwards)
        } else {
            rust.raisers().get(short).map(|raiser| raiser.handing)
        }?;

        let (through, payload) = (
            if callee.is_empty() {
                "(a pair)".to_owned()
            } else {
                callee
            },
            match self.chars.get(after) {
                Some(',') => Some(self.argument(after + 1)),
                _ => None,
            },
        );
        // A neighbour that is another word of the machine's vocabulary is a second FACT, not this
        // event's data — see this function's own doc.
        if payload.as_deref().is_some_and(|beside| {
            beside.contains(&format!("{EVENT_TYPE}::"))
                || beside.contains(&format!("{STATE_TYPE}::"))
        }) {
            return None;
        }
        if payload.is_none() && !matches!(self.chars.get(after), Some(')')) {
            return None;
        }
        let carries = match (handing, payload.as_deref()) {
            (_, None) | (Handing::Nothing, _) => false,
            (Handing::Forwards, Some(beside)) => !beside.is_empty() && beside != "\"\"",
            (Handing::Composes, Some(_)) => true,
        };
        Some(self.site(at, through, payload, carries))
    }

    /// One finding, with the fields only [`spelled`] can fill left at their defaults.
    fn site(&self, at: usize, through: String, payload: Option<String>, carries: bool) -> Spelled {
        Spelled {
            file: String::new(),
            line: self.line(at),
            event: String::new(),
            through,
            payload,
            carries,
            shipping: false,
        }
    }

    /// The innermost unclosed opener above `at`, and the callee written before it.
    fn enclosing(&self, at: usize) -> Option<(usize, String)> {
        let mut depth = 0usize;
        let mut walk = at;
        while walk > 0 {
            walk -= 1;
            match self.chars[walk] {
                ')' | ']' | '}' => depth += 1,
                '(' | '[' | '{' => {
                    if depth == 0 {
                        let mut start = walk;
                        while start > 0
                            && (is_ident(self.chars[start - 1])
                                || matches!(self.chars[start - 1], ':' | '.'))
                        {
                            start -= 1;
                        }
                        return Some((walk, self.text(start, walk)));
                    }
                    depth -= 1;
                }
                _ => {}
            }
        }
        None
    }

    /// The argument beginning at `at`, up to the comma or closing delimiter that ends it.
    fn argument(&self, at: usize) -> String {
        let mut depth = 0usize;
        let mut walk = at;
        while walk < self.chars.len() {
            match self.chars[walk] {
                '"' => {
                    walk += 1;
                    while walk < self.chars.len() && self.chars[walk] != '"' {
                        walk += if self.chars[walk] == '\\' { 2 } else { 1 };
                    }
                }
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                ',' if depth == 0 => break,
                _ => {}
            }
            walk += 1;
        }
        self.text(at, walk)
    }

    /// Every `fn NAME(params) … { body }` in this file, as `(name, params, body)`.
    fn functions(&self) -> Vec<(String, String, String)> {
        /// What may stand immediately before `fn` once the spaces are gone.
        const BEFORE: [&str; 6] = ["pub", "unsafe", "async", "const", "extern", "default"];
        let mut found = Vec::new();
        for at in 0..self.chars.len() {
            if !self.starts(at, "fn") {
                continue;
            }
            let leads = at == 0
                || !is_ident(self.chars[at - 1])
                || BEFORE.iter().any(|word| self.ends_on(at, word));
            if !leads {
                continue;
            }
            let mut name_end = at + 2;
            while self.chars.get(name_end).copied().is_some_and(is_ident) {
                name_end += 1;
            }
            let name = self.text(at + 2, name_end);
            if name.is_empty() {
                continue;
            }
            // Past a generic parameter list, so `fn then<'a>(…)` is read like its neighbours.
            let mut open = name_end;
            if self.chars.get(open) == Some(&'<') {
                let Some(close) = self.closed(open, '<', '>') else {
                    continue;
                };
                open = close + 1;
            }
            if self.chars.get(open) != Some(&'(') {
                continue;
            }
            let Some(params_end) = self.closed(open, '(', ')') else {
                continue;
            };
            let Some(body_start) = (params_end + 1..self.chars.len())
                .find(|walk| matches!(self.chars[*walk], '{' | ';'))
            else {
                continue;
            };
            if self.chars[body_start] == ';' {
                continue;
            }
            let Some(body_end) = self.closed(body_start, '{', '}') else {
                continue;
            };
            found.push((
                name,
                self.text(open + 1, params_end),
                self.text(body_start, body_end + 1),
            ));
        }
        found
    }

    /// Where the delimiter opened at `at` closes.
    fn closed(&self, at: usize, open: char, close: char) -> Option<usize> {
        let mut depth = 0usize;
        for walk in at..self.chars.len() {
            if self.chars[walk] == open {
                depth += 1;
            } else if self.chars[walk] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(walk);
                }
            }
        }
        None
    }
}

/// Whether `char` can be part of an identifier.
fn is_ident(char: char) -> bool {
    char.is_alphanumeric() || char == '_'
}

/// Whether `body` calls `name`, as a whole word rather than as the tail of a longer one.
fn calls(body: &str, name: &str) -> bool {
    let needle = format!("{name}(");
    body.match_indices(&needle)
        .any(|(at, _)| at == 0 || !is_ident(body[..at].chars().next_back().unwrap_or(' ')))
}

/// Whether `body`'s call to `callee` hands a NAME on as the payload rather than building one.
///
/// `true` for a bare identifier — `raise_external(event, data, "")`, where an empty `data` is an
/// empty `_event.data`; `false` for anything built on the spot —
/// `&serde_json::json!({…}).to_string()`, where an empty argument still leaves a key behind. That
/// is the whole difference between `carried(&mut engine, event, "")`, which is the defect, and
/// `reflected(&mut engine, event, "")`, which is not.
fn forwarded(body: &str, callee: &str, event_at: usize) -> bool {
    let needle = format!("{callee}(");
    let Some(at) = body
        .match_indices(&needle)
        .map(|(at, _)| at)
        .find(|at| *at == 0 || !is_ident(body[..*at].chars().next_back().unwrap_or(' ')))
    else {
        return false;
    };
    let call = balanced(body, at + needle.len() - 1);
    let Some(payload) = arguments(call.trim_start_matches('(').trim_end_matches(')'))
        .into_iter()
        .nth(event_at + 1)
    else {
        return false;
    };
    let name = payload.trim_start_matches(['&', '*']);
    !name.is_empty() && name.chars().all(is_ident)
}

/// One argument list split at the commas that separate ITS OWN arguments.
///
/// ⚠ `json!({"a": 1, "b": 2})` holds commas that belong to something else, so a plain `split(',')`
/// would call the second half of one argument the next one.
fn arguments(list: &str) -> Vec<String> {
    let mut found = vec![String::new()];
    let mut depth = 0i32;
    let mut chars = list.chars();
    while let Some(char) = chars.next() {
        match char {
            '"' => {
                found
                    .last_mut()
                    .expect("one argument is always open")
                    .push(char);
                for quoted in chars.by_ref() {
                    found
                        .last_mut()
                        .expect("one argument is always open")
                        .push(quoted);
                    if quoted == '"' {
                        break;
                    }
                }
                continue;
            }
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                found.push(String::new());
                continue;
            }
            _ => {}
        }
        found
            .last_mut()
            .expect("one argument is always open")
            .push(char);
    }
    found.into_iter().map(|arg| arg.trim().to_owned()).collect()
}

/// Which of a function's parameters IS the event, zero-indexed.
///
/// ⚠ Read from the signature rather than assumed: the engine takes the event first and every
/// fixture helper in this workspace takes the machine first, so there is no convention to assume.
fn event_at(params: &str) -> usize {
    arguments(params)
        .iter()
        .position(|param| param.contains(EVENT_TYPE))
        .unwrap_or_default()
}

/// The `(…)` or `{…}` group opening at `at`, delimiters included.
fn balanced(text: &str, at: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let start = text[..at].chars().count();
    let (open, close) = match chars.get(start) {
        Some('{') => ('{', '}'),
        _ => ('(', ')'),
    };
    let mut depth = 0usize;
    for walk in start..chars.len() {
        if chars[walk] == open {
            depth += 1;
        } else if chars[walk] == close {
            depth -= 1;
            if depth == 0 {
                return chars[start..=walk].iter().collect();
            }
        }
    }
    chars[start..].iter().collect()
}

/// The content of a `"…"` or `r#"…"#` literal, unescaped.
fn literal(expr: &str) -> Option<String> {
    if let Some(raw) = expr
        .strip_prefix("r#\"")
        .and_then(|s| s.strip_suffix("\"#"))
    {
        return Some(raw.to_owned());
    }
    let quoted = expr.strip_prefix('"')?.strip_suffix('"')?;
    if quoted.contains('"') && !quoted.contains("\\\"") {
        return None;
    }
    Some(quoted.replace("\\\"", "\""))
}

/// The TYPE an `impl` block is about, so a constant inside it can be named the way payloads name
/// it — `Raise` for both `impl Raise {` and `impl From<AiLoopEvent> for Raise {`.
///
/// ⚠ The type is what a payload spells (`ScreenRule::TEXT_KEY`), never the trait, which is why the
/// text after ` for ` wins when there is one.
fn impl_type(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("impl ")
        .or_else(|| line.strip_prefix("impl<"))?;
    let rest = if line.starts_with("impl<") {
        rest.split_once('>')?.1
    } else {
        rest
    };
    let head = rest.split('{').next()?.trim();
    let target = head.rsplit(" for ").next()?.trim();
    let name = target
        .split(['<', ' ', '\''])
        .next()?
        .rsplit("::")
        .next()?
        .trim();
    (!name.is_empty() && name.starts_with(char::is_uppercase)).then(|| name.to_owned())
}

/// `const NAME: &str = "…";` as `(name, value)`, for a line that is one.
fn string_const(line: &str) -> Option<(String, String)> {
    let rest = line
        .strip_prefix("pub const ")
        .or_else(|| line.strip_prefix("const "))?;
    let (name, tail) = rest.split_once(american_colon())?;
    if !name.chars().all(is_ident) || name.is_empty() {
        return None;
    }
    let (kind, value) = tail.split_once('=')?;
    if !kind.contains("str") {
        return None;
    }
    let value = value.trim().trim_end_matches(';').trim();
    literal(value).map(|value| (name.to_owned(), value))
}

/// The separator between a constant's name and its type, spelled once so the two readers above
/// cannot disagree about it.
const fn american_colon() -> char {
    ':'
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str, text: &str) -> Source {
        let code: Vec<_> = text
            .lines()
            .enumerate()
            .map(|(index, line)| (index + 1, line.trim().to_owned()))
            .filter(|(_, line)| !line.starts_with("//") && !line.starts_with('#'))
            .collect();
        Source {
            file: file.to_owned(),
            product: code.clone(),
            code,
        }
    }

    /// ⚠⚠⚠ **BOTH SHAPES THE DOCUMENT USES**, and the one it must not be read as using.
    #[test]
    fn an_event_carries_data_when_its_own_transition_reads_one_or_the_state_it_enters_does() {
        let doc = r#"<scxml>
  <state id="working">
    <transition event="turn.blocked" cond="_event.data.service" target="service_down"/>
    <transition event="turn.done" target="judging"/>
  </state>
  <state id="judging">
    <onentry>
      <assign location="context" expr="_event.data.context"/>
    </onentry>
    <transition event="judge" cond="_event.data.done" target="idle"/>
  </state>
  <state id="idle">
    <transition event="resume" target="working"/>
  </state>
</scxml>"#;
        let carrying = data_carrying(doc);
        assert_eq!(
            carrying
                .get("turn.done")
                .map(|keys| keys.iter().cloned().collect::<Vec<_>>()),
            Some(vec!["context".to_owned()]),
            "⚠⚠⚠⚠⚠ `turn.done` reads nothing on its OWN transition and is the event that runs \
             `judging`'s entry block. Reading transitions alone is the reading fifteen fixtures \
             were written under: {carrying:?}",
        );
        assert!(carrying.contains_key("turn.blocked") && carrying.contains_key("judge"));
        assert!(
            !carrying.contains_key("resume"),
            "an event nothing reads data off is not a data-carrying event: {carrying:?}",
        );
    }

    /// ⚠⚠ A gate that read comments as content would invent events nobody raises.
    #[test]
    fn a_read_written_in_prose_is_not_a_read() {
        let doc = "<scxml><state id=\"a\">\
                   <!-- <transition event=\"ghost\" cond=\"_event.data.nothing\"/> -->\
                   </state></scxml>";
        assert!(data_carrying(doc).is_empty());
    }

    #[test]
    fn a_document_event_is_spelled_as_the_generator_spells_it() {
        assert_eq!(variant_of_event("turn.done"), "TurnDone");
        assert_eq!(variant_of_event("reflect.applied"), "ReflectApplied");
        assert_eq!(variant_of_event("judge"), "Judge");
        assert_eq!(variant_of_event("some_authored_name"), "SomeAuthoredName");
    }

    /// ⚠⚠⚠⚠⚠ **BOTH DIRECTIONS.** Each row is Rust as a file could really carry it, plus whether
    /// the site owes a payload and whether it has one. Item 453's lesson is that a needle written
    /// for one spelling is green through the others without ever saying so — so the ordinary ways
    /// of committing this defect are all here, including the two nobody has committed yet.
    #[test]
    fn every_way_of_handing_an_event_on_is_seen_and_the_rest_declined() {
        // (the file's Rust, sites found, of which carrying)
        let table: &[(&str, usize, usize)] = &[
            // ⚠ THE DEFECT, as all fifteen were written: the door that cannot carry data.
            (
                "use x::AiLoopEvent;\nfn go(e: &mut E) { e.process_event(AiLoopEvent::TurnDone); }",
                1,
                0,
            ),
            // The repair beside it.
            (
                "use x::AiLoopEvent;\nfn go(e: &mut E) { e.raise_external(AiLoopEvent::TurnDone, TURN, \"\"); }",
                1,
                1,
            ),
            // ⚠ AND THE SAME DEFECT THROUGH THE DOOR THAT CAN: an empty payload forwarded is an
            // empty `_event.data`.
            (
                "use x::AiLoopEvent;\nfn go(e: &mut E) { e.raise_external(AiLoopEvent::TurnDone, \"\", \"\"); }",
                1,
                0,
            ),
            // A helper that forwards its argument — discovered, not named.
            (
                "use x::AiLoopEvent;\nfn carried(e: &mut E, event: AiLoopEvent, data: &str) { e.raise_external(event, data, \"\"); }\nfn go(e: &mut E) { carried(e, AiLoopEvent::Judge, \"\"); }",
                1,
                0,
            ),
            // ⚠⚠ A helper that COMPOSES one, where the same empty argument is not the defect.
            (
                "use x::AiLoopEvent;\nfn reflected(e: &mut E, event: AiLoopEvent, standing: &str) { e.raise_external(event, &json!({\"standing\": standing}).to_string(), \"\"); }\nfn go(e: &mut E) { reflected(e, AiLoopEvent::ReflectNone, \"\"); }",
                1,
                1,
            ),
            // A table pairing an event with its payload, and the same table with none.
            (
                "use x::AiLoopEvent;\nconst W: [(AiLoopEvent, &str); 1] = [(AiLoopEvent::TurnDone, TURN)];",
                1,
                1,
            ),
            (
                "use x::AiLoopEvent;\nconst W: [(AiLoopEvent, &str); 1] = [(AiLoopEvent::TurnDone, \"\")];",
                1,
                0,
            ),
            // ⚠ DECLINED — naming an event is not handing it on.
            (
                "use x::AiLoopEvent;\nmatch raised { AiLoopEvent::Judge => go(), }\nif raised == AiLoopEvent::TurnDone { go(); }",
                0,
                0,
            ),
            // ⚠ DECLINED — a list of events pairs each with the next, and neither is the other's data.
            (
                "use x::AiLoopEvent;\nfor e in [AiLoopEvent::TurnDone, AiLoopEvent::Judge] { go(e); }",
                0,
                0,
            ),
            // ⚠ DECLINED — a call that is not a raiser, however many arguments stand beside it.
            (
                "use x::AiLoopEvent;\nassert!(ingress.contains(&AiLoopEvent::Judge), \"published\");",
                0,
                0,
            ),
            // ⚠⚠ GLOB-IMPORTED, so the variant stands bare — a spelling no literal needle sees,
            // and item 453's whole finding is that such a needle is green through it in silence.
            (
                "use x::AiLoopEvent::*;\nfn go(e: &mut E) { e.process_event(TurnDone); }",
                1,
                0,
            ),
            // ⚠ DECLINED — the same bare word in a file that never reaches the event type. A
            // `TurnDone` nothing imported is somebody else's word.
            ("fn go(e: &mut E) { e.process_event(TurnDone); }", 0, 0),
        ];

        let carrying: BTreeMap<String, BTreeSet<String>> = ["turn.done", "judge", "reflect.none"]
            .into_iter()
            .map(|event| (event.to_owned(), BTreeSet::from(["done".to_owned()])))
            .collect();

        let mut wrong = Vec::new();
        for (rust, owed, owed_carrying) in table {
            let sources = [source("a.rs", rust)];
            let sites = spelled(&sources, &carrying, &Rust::of(&sources));
            let carried = sites.iter().filter(|site| site.carries).count();
            if sites.len() != *owed || carried != *owed_carrying {
                wrong.push(format!(
                    "owed {owed} sites ({owed_carrying} carrying), read {} ({carried}) for \
                     {rust:?} => {sites:#?}",
                    sites.len(),
                ));
            }
        }
        assert!(
            wrong.is_empty(),
            "a ratchet that cannot see the ordinary way of committing the defect is green forever \
             in the voice of a working one: {wrong:#?}",
        );
    }

    /// ⚠⚠⚠ **THE DRIVER'S OWN WAY OF COMMITTING IT**, which no fixture has: the envelope carries
    /// `data: None` through `From`, so `.into()` and `Envelope::from` are bare raises.
    #[test]
    fn the_envelopes_own_conversion_carries_nothing_and_is_read_as_a_bare_raise() {
        let rust = "use x::AiLoopEvent;\n\
                    impl From<AiLoopEvent> for Raise { fn from(event: AiLoopEvent) -> Self { Self { event, data: None } } }\n\
                    fn one() -> Raise { AiLoopEvent::TurnDone.into() }\n\
                    fn two() -> Raise { Raise::from(AiLoopEvent::Judge) }\n\
                    fn three() -> Raise { Raise::carrying(AiLoopEvent::Judge, json!({\"done\": true})) }";
        let sources = [source("a.rs", rust)];
        let read = Rust::of(&sources);
        assert_eq!(
            read.envelope(),
            Some("Raise"),
            "the envelope is DISCOVERED from `impl From<AiLoopEvent> for …`, so renaming it teaches \
             this reader instead of blinding it",
        );
        let carrying: BTreeMap<String, BTreeSet<String>> = ["turn.done", "judge"]
            .into_iter()
            .map(|event| (event.to_owned(), BTreeSet::from(["done".to_owned()])))
            .collect();
        let sites = spelled(&sources, &carrying, &read);
        assert_eq!(sites.len(), 3, "{sites:#?}");
        assert!(
            sites.iter().filter(|site| site.carries).count() == 1,
            "⚠⚠⚠ only the one built through the envelope's CARRYING constructor has data — the \
             other two are the driver's version of the fifteen: {sites:#?}",
        );
    }

    /// ⚠⚠ The keys of a payload, in each of the four shapes this workspace writes one.
    #[test]
    fn a_payload_names_its_keys_however_it_is_written() {
        let rust = "use x::AiLoopEvent;\n\
                    const TURN: &str = r#\"{\"context\": 0, \"cold\": 0}\"#;\n\
                    const STANDING: &str = \"standing\";\n\
                    fn costs(&self) -> Value { json!({\"context\": 1, \"unreadable\": false}) }\n\
                    fn go() { let _ = json!({STANDING: 1}); }";
        let sources = [source("a.rs", rust)];
        let read = Rust::of(&sources);
        assert_eq!(
            read.keys_of("TURN"),
            Some(BTreeSet::from(["context".to_owned(), "cold".to_owned()])),
        );
        assert_eq!(
            read.keys_of("serde_json::json!({STANDING: learned})"),
            Some(BTreeSet::from(["standing".to_owned()])),
            "⚠⚠ a key spelled as a CONSTANT is resolved, because the driver spells most of them \
             that way and a reader that took the name would compare `STANDING` with `standing`",
        );
        assert_eq!(
            read.keys_of("self.costs(panes)"),
            Some(BTreeSet::from([
                "context".to_owned(),
                "unreadable".to_owned()
            ])),
        );
        assert_eq!(
            read.keys_of("&payload.to_string()"),
            None,
            "⚠⚠⚠ a payload assembled elsewhere is one this reader CANNOT read, and `None` is how \
             it says so — a claim that read it as empty would be a red about nothing",
        );
        assert!(read.shared("TURN") && !read.shared("json!({})"));
    }

    /// ⚠⚠⚠⚠⚠ **A CONSTANT RESOLVES THROUGH THE `impl` THAT DECLARES IT**, or eight `WIRE_KEY`s are
    /// one question with eight answers — item 516's prerequisite, measured in this workspace.
    #[test]
    fn a_constant_is_told_apart_by_the_type_that_declares_it() {
        let rust = "impl Readiness {\n\
                    pub const WIRE_KEY: &'static str = \"await_person_ms\";\n\
                    }\n\
                    impl Turn {\n\
                    pub const WIRE_KEY: &'static str = \"turn_within_ms\";\n\
                    }\n\
                    impl From<AiLoopEvent> for Raise {\n\
                    pub const WIRE_KEY: &'static str = \"raised\";\n\
                    }";
        let sources = [source("a.rs", rust)];
        let read = Rust::of(&sources);

        assert_eq!(
            read.keys_of("json!({Readiness::WIRE_KEY: 1, Turn::WIRE_KEY: 2})"),
            Some(BTreeSet::from([
                "await_person_ms".to_owned(),
                "turn_within_ms".to_owned()
            ])),
            "⚠⚠⚠⚠⚠ two types declare `WIRE_KEY` and they are DIFFERENT keys — a reader that took \
             the last path segment would report one of them twice and the claim built on it would \
             be a confident lie",
        );
        assert_eq!(
            read.keys_of("json!({WIRE_KEY: 1})"),
            Some(BTreeSet::from(["WIRE_KEY".to_owned()])),
            "⚠⚠⚠ and the BARE name stays unresolved, because this workspace does not agree what it \
             means — the token standing is what makes a claim go red naming it rather than pass",
        );
        assert!(
            read.ambiguous().contains_key("WIRE_KEY"),
            "the bare name is contested and must be reportable as such: {:?}",
            read.ambiguous(),
        );
        assert!(
            !read.ambiguous().contains_key("Readiness::WIRE_KEY"),
            "⚠⚠ but the QUALIFIED name is not contested — that is the whole repair",
        );
    }

    /// ⚠ `impl Trait for Type` names the TYPE, which is what a payload spells.
    #[test]
    fn an_impl_names_the_type_and_not_the_trait_or_its_lifetimes() {
        assert_eq!(impl_type("impl Raise {"), Some("Raise".to_owned()));
        assert_eq!(
            impl_type("impl From<AiLoopEvent> for Raise {"),
            Some("Raise".to_owned()),
        );
        assert_eq!(
            impl_type("impl<'a> Screened<'a> {"),
            Some("Screened".to_owned())
        );
        assert_eq!(impl_type("let x = 1;"), None);
    }

    /// ⚠⚠ `match … { Some(word) => Value::from(word) }` inside a payload holds colons of its own.
    #[test]
    fn a_key_is_only_a_key_at_the_top_of_the_object() {
        let sources = [source("a.rs", "")];
        let read = Rust::of(&sources);
        let keys = read
            .keys_of(
                "json!({\"done\":heard.said(),\"checked\":match c.wire_str(){Some(w)=>serde_json::Value::from(w),None=>serde_json::Value::Bool(false),},})",
            )
            .expect("an object written out is one this reader can read");
        assert_eq!(
            keys,
            BTreeSet::from(["done".to_owned(), "checked".to_owned()]),
            "a reader that counted the colons inside the arms would report keys nobody wrote",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A PROBE POINTED AT NOTHING MUST NEVER READ AS CLEAN**, applied to the closure: the
    /// two engine doors are the seed and everything else is discovered from them.
    #[test]
    fn the_raiser_set_starts_at_the_engines_doors_and_grows_by_what_reaches_them() {
        let rust = "use x::AiLoopEvent;\n\
                    fn carried(e: &mut E, event: AiLoopEvent, data: &str) { e.raise_external(event, data, \"\"); }\n\
                    fn twice(e: &mut E, event: AiLoopEvent, data: &str) { carried(e, event, data); }\n\
                    fn wraps(e: &mut E, event: AiLoopEvent, said: &str) { carried(e, event, &json!({\"text\": said}).to_string()); }\n\
                    fn bare(e: &mut E, event: AiLoopEvent) { e.process_event(event); }\n\
                    fn renders(raised: AiLoopEvent) -> String { format!(\"{raised:?}\") }";
        let sources = [source("a.rs", rust)];
        let raisers = Rust::of(&sources).raisers().clone();
        let handing = |name: &str| raisers.get(name).map(|raiser| raiser.handing);
        assert_eq!(handing("process_event"), Some(Handing::Nothing));
        assert_eq!(handing("raise_external"), Some(Handing::Forwards));
        assert_eq!(handing("carried"), Some(Handing::Forwards));
        assert_eq!(
            handing("twice"),
            Some(Handing::Forwards),
            "⚠⚠ the closure is TRANSITIVE, or a helper one layer further out is invisible — and \
             this one hands its argument to a helper that takes the MACHINE first, so the payload \
             is the third thing rather than the second: {raisers:?}",
        );
        assert_eq!(
            handing("wraps"),
            Some(Handing::Composes),
            "⚠⚠⚠ one layer out and BUILDING a payload, where an empty argument is not the defect",
        );
        assert_eq!(handing("bare"), Some(Handing::Nothing));
        assert_eq!(
            raisers.get("carried").map(|raiser| raiser.event_at),
            Some(1),
            "⚠⚠ which argument is the event comes from the signature: this workspace's helpers put \
             the machine first and the engine puts the event first",
        );
        assert!(
            !raisers.contains_key("renders"),
            "⚠ a function that takes an event and raises nothing is not a raiser, or every reader \
             of the vocabulary would owe a payload: {raisers:?}",
        );
    }
}
