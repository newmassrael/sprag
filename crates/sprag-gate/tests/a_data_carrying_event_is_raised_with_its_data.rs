//! An event a document reads `_event.data` off must never be raised without it — item 507.
//!
//! # ⚠⚠⚠⚠⚠ What was green for months
//!
//! `judging`'s `<onentry>` reads three keys off `_event.data`, and the only way into `judging` is
//! `turn.done`. Fifteen fixture sites raised it — and `judge` beside it — through `process_event`,
//! which carries no `_event.data` at all. The datamodel was asked to index nil; W3C SCXML 3.8
//! abandoned the rest of the entry block; W3C SCXML 3.12.2 dropped the error because nothing
//! matched it. **Every one of those gates passed on a half-executed state**, and nothing anywhere
//! said so.
//!
//! Item 505 gave the document an edge that answers its own errors and seven of them went red at
//! once. That edge is a real detector and it is BEHAVIOURAL — the run ends `failed`, on the state
//! the fixture lands in, some steps later. This is the static half: the pairing itself, said out
//! loud, so the sixteenth site is refused where it is written rather than where it lands.
//!
//! # ⚠⚠⚠⚠⚠ AND FOR MONTHS IT ASKED ALL OF THAT OF ONE DOCUMENT — register item 1025
//!
//! Every claim below used to be aimed at `ai_loop.scxml` through a pair of constants: the event
//! type was `"AiLoopEvent"` and the document was `loop_shape::DOCUMENT`, joined by nothing but both
//! being spelled here. `context_review.scxml` reads `_event.data` off three of its events and
//! `review.rs` drives it — and none of that was PASSING these rules. It was outside their
//! POPULATION, which is the quietest exemption a gate can have: no refusal names it, no pin counts
//! it, and item 1024's coverage number was measured over the first machine alone without saying so.
//!
//! The population is now walked for ([`sprag_gate::payload::driven`]) and every pin below is held
//! PER MACHINE, in [`MACHINES`]. What a document must do to be judged here is a predicate — some
//! shipping line spells the event type SCE generates from its stem — and a document that starts
//! meeting it arrives as a red on [`every_document_this_workspace_drives_is_one_this_gate_pins`],
//! rather than as somebody remembering.
//!
//! # ⚠⚠ Why the claim is about SPELLED sites, stated rather than implied
//!
//! [`sprag_gate`] takes no dependencies by charter, so nothing here parses Rust. A driver that
//! computes an event into a variable and attaches a payload three functions later is outside this
//! gate's reach — `OuterLoop::watch` answers `AiLoopEvent::TurnDone` as a value and its caller
//! decides what to carry. What IS inside it: every place this workspace writes an event's name
//! where the event is handed on, which is where all fifteen lived and where every payload it writes
//! down is decided.

use std::collections::{BTreeMap, BTreeSet};

use sprag_gate::payload::{
    Driven, Rust, Spelled, data_carrying, driven, indirect, named, spelled, tolerant,
    variant_of_event,
};
use sprag_gate::sources::{Source, Statechart, rust_sources, statecharts, workspace_root};

/// One machine this gate is about, and everything a claim below needs pinned about it.
///
/// # ⚠⚠⚠⚠⚠ Why every number here is PER MACHINE rather than per gate
///
/// Item 498's rule: a claim over a DISCOVERED set is green whether it discovered nine events or
/// none, so the first thing asserted is what was discovered. That rule is what these fields are —
/// and a single set of them, shared across machines, would be the same defect one level up: a
/// second document could arrive, discover nothing, and satisfy the first document's numbers by
/// standing still.
struct Machine {
    /// The document, relative to the workspace root — the half of the pair a person can open.
    document: &'static str,
    /// The events it reads `_event.data` off, and the pin every claim below rests on.
    carrying: &'static [&'static str],
    /// The keys it NAMES beside `_event.data` and reads nowhere — item 1024's exemption.
    affordances: &'static [&'static str],
    /// How many of this machine's shipping payloads the scan can read, and how many it cannot.
    coverage: (usize, usize),
    /// At least this many sites spell one of its data-carrying events.
    ///
    /// ⚠⚠⚠⚠⚠ A RATCHET AT THE MEASUREMENT, and it was a slack floor until 2026-09-11. It read
    /// `sites.len() > 30` while the walk found **144** — a floor a hundred and fourteen sites below
    /// the fact it was guarding, which is a number that can only go red after the reader has gone
    /// almost entirely blind. Held here at what was counted, so LOSING one is announced; a round
    /// that adds a raise raises this line in the same edit, which is the ratchet working.
    sites: usize,
    /// Whether its driver wraps an event in an envelope that carries no data.
    envelope: bool,
    /// The files the walk must reach, or the needle has gone blind and every claim is vacuous.
    raised_in: &'static [&'static str],
    /// `(state, event)` pairs where a BARE raise of a data-carrying event is harmless.
    tolerant: &'static [(&'static str, &'static str)],
    /// The driver's indirect hand-offs — `(function, the name converted)`.
    indirect: &'static [(&'static str, &'static str)],
}

/// Every machine this workspace drives, with what was measured about each.
///
/// ⚠⚠ THE SET ITSELF IS A PIN, and [`every_document_this_workspace_drives_is_one_this_gate_pins`]
/// is what holds it against the walk. Thirteen `.scxml` live under `crates/`; seven are `probe_*`
/// driven only from `#[cfg(test)]`, and `debt_loop.scxml` and `unclaimed_loop.scxml` are compiled
/// machines no Rust raises a single event into — they are read as DOCUMENTS, for the decisions
/// their `<data>` holds. Four remain, and all four are here.
const MACHINES: &[Machine] = &[
    Machine {
        document: "crates/sprag-plugin/src/ai_loop.scxml",
        // ⚠⚠⚠⚠⚠ `peer.silent` JOINED THIS SET on 2026-08-27, and this gate is how it was found.
        // Register item 715 gave it a guard — `cond="_event.data.service"` — because a peer that
        // goes quiet having PRINTED why is waiting out an outage rather than falling silent. Its
        // payload is `{service}`, published by the same `watch` pass that publishes
        // `turn.blocked`'s. **And announcing it caught two fixtures raising it bare**, both in
        // `ai_loop.rs`: a guard evaluated against a nil `_event.data` is W3C SCXML 3.8's abandoned
        // block, which is a state half-entered in the voice of one that worked — and both of those
        // gates were GREEN. That is the whole reason this list is written down rather than
        // discovered.
        //
        // ⚠⚠⚠⚠⚠ `prompt.unasked` JOINED IT the same day and it is the same shape. Register item
        // 719 gave it `cond="_event.data.retyped"`, because a prompt the peer refused is not one
        // fact but two: text this run has never delivered before, and text a replacement has
        // ALREADY been spent on. Announcing it named the same defect as last time — every fixture
        // in `ai_loop.rs` raised it bare, and each ended its run `failed` on an `error.execution`
        // nobody meant.
        carrying: &[
            "brief",
            "judge",
            "peer.silent",
            "prompt.unasked",
            "reflect.applied",
            "reflect.done",
            "reflect.none",
            "review.done",
            "screen.matched",
            "turn.blocked",
            "turn.done",
        ],
        // ⚠⚠ IT HELD TWO ON ITS FIRST RUN, and the second one is why the pin exists: `design` was a
        // name this document's prose gave to a verdict that has always travelled as `judged`. No
        // driver ever sent such a key and no edge ever read one — so a sentence naming nothing was
        // quietly widening what a payload may carry unread. The sentence was corrected, not the pin.
        affordances: &["rule"],
        // ⚠⚠ TEN AND TWO on the day item 1024 was paid, and the refusal NAMES the two rather than
        // leaving them to be re-derived:
        //
        // * `outer.rs` raises `brief` with `&payload.to_string()` — assembled a screen away, which
        //   is why item 1023 built `sprag_gate::briefing` to ask this same question of that one
        //   edge. Covered, elsewhere.
        // * `outer.rs` raises `prompt.unasked` with `&retyped.wire()` — a METHOD on a receiver, and
        //   `Rust::keys_of` follows a plain call's body but not this. Not covered anywhere, and the
        //   document does read `retyped`, so nothing is wrong there today.
        coverage: (10, 2),
        sites: 144,
        envelope: true,
        raised_in: &[
            "crates/sprag-plugin/src/outer.rs",
            "crates/sprag-plugin/src/ai_loop.rs",
        ],
        // ⚠ `reflecting`/`turn.blocked` is the load-bearing one: `OuterLoop::reflect` hands that
        // event on with no payload. The rest are pinned because a set that quietly grew or shrank
        // would say nothing, which is how this class hides.
        //
        // ⚠⚠ THE TWO `prompt.unasked` PAIRS ARRIVED ON 2026-08-27 (item 719) AND ARE THE HARMLESS
        // DIRECTION. `closing` and `stopping` answer that event with one unconditional edge each —
        // a run that is already ending must not buy a session and must not be failed by one — so
        // neither indexes `_event.data`, and the region's rule underneath them is where the guard
        // lives. The driver carries a payload on every raise of it regardless (`Retyped::wire`).
        tolerant: &[
            ("closing", "prompt.unasked"),
            ("closing", "turn.blocked"),
            ("closing", "turn.done"),
            ("reflecting", "turn.blocked"),
            ("stopping", "prompt.unasked"),
            ("stopping", "turn.blocked"),
            ("stopping", "turn.done"),
        ],
        indirect: &[("pumping", "other"), ("reflect", "ended")],
    },
    Machine {
        // ⚠⚠⚠⚠⚠ THE SECOND MACHINE, and the whole of register item 1025. `review.rs` drives it:
        // seven raise sites, every payload written down where the event is spelled, and three of
        // its events read `_event.data`. None of that was judged by anything until the pair below
        // stopped being a constant.
        document: "crates/sprag-plugin/src/context_review.scxml",
        carrying: &["ask.done", "count.done", "read.done"],
        affordances: &[],
        coverage: (3, 0),
        sites: 11,
        // ⚠ NO ENVELOPE: this driver's own `raise` takes the payload as an argument and renders it,
        // so there is no `impl From<ContextReviewEvent> for …` to commit the driver's version of
        // the defect through. `false` is a claim, and it moves the day one is written.
        envelope: false,
        raised_in: &["crates/sprag-plugin/src/review.rs"],
        // ⚠ EMPTY, and measured: all three of this document's data-carrying events are answered by
        // a transition that reads `_event.data` itself, so no state tolerates a bare one.
        tolerant: &[],
        indirect: &[],
    },
    Machine {
        // ⚠⚠ `datamodel="null"`: this machine's events cannot carry data at all, so every pin here
        // is empty BY MEASUREMENT rather than by exemption. The day it grows a datamodel and reads
        // a key, `carrying` moves and the round that did it decides the rest.
        document: "crates/sprag-plugin/src/orchestration.scxml",
        carrying: &[],
        affordances: &[],
        coverage: (0, 0),
        sites: 0,
        envelope: false,
        raised_in: &[],
        tolerant: &[],
        indirect: &[],
    },
    Machine {
        // ⚠⚠ `datamodel="null"` as well — see `orchestration.scxml` one entry up.
        document: "crates/sprag-plugin/src/session.scxml",
        carrying: &[],
        affordances: &[],
        coverage: (0, 0),
        sites: 0,
        envelope: false,
        raised_in: &[],
        tolerant: &[],
        indirect: &[],
    },
];

/// The files this gate knowingly does not read, each with the reason.
///
/// ⚠⚠⚠ An entry is an exemption for a WHOLE file, which is as coarse as a text scan can honestly
/// be, and [`every_exemption_is_still_load_bearing`] re-measures each one — an exemption that has
/// stopped mattering is a dead rule, and a dead rule in a gate reads exactly like a live one.
const EXEMPT: [(&str, &str); 2] = [
    (
        "crates/sprag-gate/src/payload.rs",
        "this gate's own reader, whose both-directions table has to SPELL the defect in order to \
         prove it can see it — measured, on the first run: its \
         `e.process_event(AiLoopEvent::TurnDone)` fixture is read as a bare raise, because it is \
         one, written down. Splitting the needles so they do not match themselves is the \
         alternative and it is worse: a trick that quietly stops matching is the silent failure \
         this whole module exists to prevent",
    ),
    (
        "crates/sprag-gate/tests/a_data_carrying_event_is_raised_with_its_data.rs",
        "this file, for the same reason one line up and measured the same way: the exemption above \
         QUOTES the offending call, so the gate read its own prose as the sixteenth site. ⚠ That \
         makes the two entries load-bearing on each other's wording, which is not an accident — \
         `every_exemption_is_still_load_bearing` goes red the day either stops tripping, and a \
         reword that silences it is told to delete it",
    ),
];

/// This workspace's Rust, minus the files [`EXEMPT`] names.
fn subject() -> Vec<Source> {
    rust_sources()
        .into_iter()
        .filter(|source| !EXEMPT.iter().any(|(file, _)| *file == source.file))
        .collect()
}

/// One machine's document, its reads, this workspace's raising vocabulary for it, and every site
/// the two meet at.
struct Reading {
    /// The pair, as the walk made it.
    machine: Driven,
    /// What was measured about it before, held beside what is measured now.
    pinned: &'static Machine,
    /// The document, verbatim.
    document: String,
    /// Every event it reads `_event.data` off, with the keys.
    carrying: BTreeMap<String, BTreeSet<String>>,
    /// What this workspace's Rust says about raising THIS machine's events.
    rust: Rust,
    /// Every place one of those events is spelled where it is handed on.
    sites: Vec<Spelled>,
}

/// The set of statecharts this workspace carries, read once.
fn charts() -> Vec<Statechart> {
    let found = statecharts();
    // ⚠ The walk answers about the tree it was pointed at, and `workspace_root` is what decides
    // that — named here so a reader of a refusal below knows which tree said it.
    assert!(
        found
            .iter()
            .any(|chart| chart.file.ends_with("/ai_loop.scxml")),
        "⚠⚠⚠⚠⚠ the loop's own document is not in the statecharts {} holds, so the walk is pointed \
         at some other tree and every population claim below is about that one: {:?}",
        workspace_root().display(),
        found.iter().map(|chart| &chart.file).collect::<Vec<_>>(),
    );
    found
}

/// Every driven machine, read.
///
/// # Panics
///
/// When the walk finds a driven document [`MACHINES`] does not pin. That is a population change,
/// and this is the one place it cannot be silent —
/// [`every_document_this_workspace_drives_is_one_this_gate_pins`] says it in full, and this says it
/// wherever a claim is reached first.
fn readings() -> Vec<Reading> {
    let sources = subject();
    let charts = charts();
    driven(&sources, &charts)
        .into_iter()
        .map(|machine| {
            let pinned = MACHINES
                .iter()
                .find(|pinned| pinned.document == machine.document())
                .unwrap_or_else(|| {
                    panic!(
                        "⚠⚠⚠⚠⚠ REGISTER ITEM 1025: `{}` is driven — shipping Rust spells `{}` — \
                         and nothing in `MACHINES` pins it, so every claim in this file would \
                         simply not be about it. Pin it, with the numbers measured, the way the \
                         four before it are",
                        machine.document(),
                        machine.event_type(),
                    )
                });
            let document = charts
                .iter()
                .find(|chart| chart.file == machine.document())
                .map(|chart| chart.text.clone())
                .expect("the machine came from these charts");
            let carrying = data_carrying(&document);
            let rust = Rust::of(&sources, &machine);
            let sites = spelled(&sources, &carrying, &rust);
            Reading {
                machine,
                pinned,
                document,
                carrying,
                rust,
                sites,
            }
        })
        .collect()
}

/// Whether `text` names `name` as a WHOLE identifier rather than as part of a longer one.
///
/// ⚠⚠⚠⚠ Measured on this gate's own first run: a plain `contains` reported `KEY` inside `TEXT_KEY`
/// and `MARKER` inside `REFERENCE_MARKER`, so it accused two payloads that spell nothing contested.
/// Item 498's rule — *the subject is a glob, the boundary is punctuation* — and a needle without
/// boundaries decides alone.
fn spells(text: &str, name: &str) -> bool {
    let ident = |char: char| char.is_alphanumeric() || char == '_';
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(ident) && !after.is_some_and(ident)
    })
}

/// Every key the DRIVER puts on each event, taken from the payloads its shipping code writes down.
fn drivers_keys(reading: &Reading) -> BTreeMap<String, BTreeSet<String>> {
    let mut keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for site in reading.sites.iter().filter(|site| site.shipping) {
        if let Some(read) = site
            .payload
            .as_deref()
            .and_then(|beside| reading.rust.keys_of(beside))
        {
            keys.entry(site.event.clone()).or_default().extend(read);
        }
    }
    keys
}

/// ⚠⚠⚠⚠⚠ **WHICH DOCUMENTS THIS GATE IS ABOUT IS WALKED FOR, AND THE WALK IS PINNED** — register
/// item 1025, and the claim every other one in this file rests on.
///
/// # Why the population is the thing that had to be measured
///
/// A rule aimed at one document is not a rule with an exemption; it is a rule with a POPULATION OF
/// ONE, and nothing about it says so. `context_review.scxml` was not passing these claims and was
/// not failing them — it was not in them, and neither the coverage pin nor the affordance pin nor
/// the tolerant pin was any narrower for it. That is rule 6's shape: what is not classified reads
/// exactly like what is clean.
///
/// ⚠⚠ MORE than the pin: a document started being driven. Decide its numbers and pin it — the
/// round that drives a new machine is the round that knows what its payloads are. FEWER: a driver
/// stopped spelling a machine's events, so every claim about it below is now vacuous and the pin is
/// what says so instead of the silence.
#[test]
fn every_document_this_workspace_drives_is_one_this_gate_pins() {
    let sources = subject();
    let charts = charts();
    let walked: Vec<String> = driven(&sources, &charts)
        .iter()
        .map(|machine| machine.document().to_owned())
        .collect();
    let mut expected: Vec<String> = MACHINES
        .iter()
        .map(|machine| machine.document.to_owned())
        .collect();
    expected.sort();

    assert_eq!(
        walked,
        expected,
        "⚠⚠⚠⚠⚠ THE SET OF MACHINES THIS WORKSPACE DRIVES HAS MOVED, and this gate's every other \
         claim is about the old set.\n\
         What the walk asks of each of the {} statechart(s) under `crates/`: does any SHIPPING \
         line spell the event type SCE generates from its stem? Nothing else — a `probe_*` driven \
         from `#[cfg(test)]` is not driven, and a compiled machine no Rust raises into is a \
         document.\n\
         The statecharts, and what each would be named: {:?}",
        charts.len(),
        charts
            .iter()
            .map(|chart| (
                chart.file.clone(),
                Driven::of(chart).event_type().to_owned()
            ))
            .collect::<Vec<_>>(),
    );
}

/// ⚠⚠⚠⚠⚠ **A PROBE POINTED AT NOTHING MUST NEVER READ AS CLEAN.** Every claim below is satisfied
/// by a measurement that found nothing, so this is the one that says the walk reached each loop.
#[test]
fn the_measurement_reaches_each_document_and_the_rust_that_raises_into_it() {
    for reading in readings() {
        let machine = reading.machine.event_type();
        let document = reading.machine.document();

        assert!(
            reading.sites.len() >= reading.pinned.sites,
            "`{document}` is raised into in at least {} places and this walk found {}: the gate is \
             measuring less than it did and would be green on what it stopped seeing",
            reading.pinned.sites,
            reading.sites.len(),
        );

        // ⚠⚠ THE SEEDS ARE THE RUNTIME'S API AND ARE THE ONE THING HERE THAT IS SPELLED. If the
        // engine ever renames a door, this is where it is announced — rather than the raiser
        // closure quietly finding nothing and every claim below passing on an empty set.
        for door in ["process_event", "raise_external"] {
            assert!(
                reading.rust.raisers().contains_key(door),
                "⚠⚠⚠⚠⚠ `{door}` is one of the engine's two doors and the seed the raiser set for \
                 `{machine}` grows from. Without it the closure finds no helpers, no site is a \
                 raise, and this gate goes green in the voice of a working one: {:?}",
                reading.rust.raisers().keys().collect::<Vec<_>>(),
            );
        }
        assert_eq!(
            reading.rust.envelope().is_some(),
            reading.pinned.envelope,
            "⚠⚠⚠ whether `{machine}`'s driver wraps an event in an envelope (`impl From<{machine}> \
             for …`) is a fact about how it can commit this defect WITHOUT a fixture: that \
             conversion carries nothing. Gone, and the reader can no longer see the driver's own \
             way of committing it; arrived, and the round that wrote it owes this pin a look",
        );

        // ⚠ AND THE SPELLING RULE IS MEASURED, not assumed: the generator's variant for each of
        // these events must be a name this workspace actually writes, or the needle is looking for
        // a word nobody uses and every site is invisible.
        let text: String = subject()
            .iter()
            .flat_map(|source| source.code.iter().map(|(_, line)| line.clone()))
            .collect();
        for event in reading.carrying.keys() {
            let variant = variant_of_event(event);
            assert!(
                text.contains(&format!("::{variant}")),
                "⚠⚠⚠⚠ `{document}` carries `{event}` and nothing in this workspace spells \
                 `::{variant}`. Either the generator's naming changed — in which case this needle \
                 is blind and every claim below is vacuous — or an event the document reads data \
                 off is raised by nobody at all",
            );
        }

        let files: BTreeSet<&str> = reading
            .sites
            .iter()
            .map(|site| site.file.as_str())
            .collect();
        for owner in reading.pinned.raised_in {
            assert!(
                files.contains(owner),
                "{owner} is where `{document}`'s driver and fixtures live, and the walk must reach \
                 it: {files:?}",
            );
        }
    }
}

/// ⚠⚠ **WHICH EVENTS CARRY DATA IS A FACT ABOUT THE DOCUMENT, AND A NEW ONE IS ANNOUNCED.**
#[test]
fn the_events_each_document_reads_data_off_are_the_ones_this_gate_was_written_for() {
    for reading in readings() {
        assert_eq!(
            reading
                .carrying
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            reading.pinned.carrying,
            "⚠⚠⚠⚠⚠ THE SUBJECTS OF `{}` MOVED. Either the document started reading `_event.data` \
             off another event — in which case decide, here, what its payload is and check that \
             every raise of it carries one — or it stopped reading one. A set that quietly grew \
             would leave the new event's raises unchecked, which is the state `turn.done` and \
             `judge` were in for months",
            reading.machine.document(),
        );

        // ⚠ The keys are NOT pinned, and the reason is stated at the reader: a `<data>` this loop
        // invites a caller to author is item 494's subject, and a second pin on the same names here
        // would go red on every round that adds one while saying nothing this gate is about.
        for (event, keys) in &reading.carrying {
            assert!(
                !keys.is_empty(),
                "{event} is in `{}`'s carrying set and reads no key at all, which cannot happen \
                 unless the reader is attributing reads to the wrong event",
                reading.machine.document(),
            );
        }
    }
}

/// ⚠⚠⚠⚠⚠ **THE CLAIM: NOTHING HANDS ONE OF THESE EVENTS ON WITH NOTHING.** Item 507's ratchet.
///
/// A red here names a site, and the repair is always the same shape: give the raise the payload the
/// document reads. `process_event` cannot carry one at all — it is `raise_external(event, "", "")`
/// followed by a macrostep — so a site written that way has to change door, which is exactly what
/// the fifteen did.
#[test]
fn no_data_carrying_event_is_handed_on_without_its_data() {
    let mut bare = Vec::new();
    let mut reads: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for reading in readings() {
        for site in reading.sites.iter().filter(|site| !site.carries) {
            bare.push(format!(
                "  {}:{} raises `{}` into {} through `{}` carrying {}",
                site.file,
                site.line,
                site.event,
                reading.machine.document(),
                site.through,
                site.payload.as_deref().unwrap_or("nothing at all"),
            ));
            reads.insert(
                site.event.clone(),
                reading
                    .carrying
                    .get(&site.event)
                    .cloned()
                    .unwrap_or_default(),
            );
        }
    }
    assert!(
        bare.is_empty(),
        "⚠⚠⚠⚠⚠ {} SITE(S) HAND A DATA-CARRYING EVENT ON WITH NO `_event.data`. The document reads \
         keys off each of these events, so the datamodel is asked to index nil: W3C SCXML 3.8 \
         abandons the rest of the block that raised the error and W3C SCXML 3.12.2 drops the error \
         itself unless something answers it. That is not a failure a reader sees — it is a state \
         half-entered, in the voice of one that worked.\n\n{}\n\nwhat the document reads: {reads:?}",
        bare.len(),
        bare.join("\n"),
    );
}

/// ⚠⚠⚠⚠ **AND WHAT THE DRIVER PUTS ON ONE IS WHAT THE DOCUMENT ASKED FOR** — the other direction,
/// which a rule about *whether* there is a payload cannot reach.
///
/// ⚠ Only payloads written DOWN are read: `brief`'s is assembled a screen away and handed over in a
/// variable, and [`Rust::keys_of`] answers [`None`] rather than guessing. A claim that read an
/// unreadable payload as empty would be a red about nothing.
#[test]
fn every_payload_the_driver_writes_down_carries_the_keys_the_document_reads() {
    for reading in readings() {
        let mut short = Vec::new();
        let mut read = 0usize;
        for site in reading.sites.iter().filter(|site| site.shipping) {
            let Some(keys) = site
                .payload
                .as_deref()
                .and_then(|beside| reading.rust.keys_of(beside))
            else {
                continue;
            };
            read += 1;
            let owed = reading
                .carrying
                .get(&site.event)
                .cloned()
                .unwrap_or_default();
            let missing: Vec<&String> = owed.difference(&keys).collect();
            if !missing.is_empty() {
                short.push(format!(
                    "  {}:{} puts {:?} on `{}`, and the document reads {missing:?} off it",
                    site.file, site.line, keys, site.event,
                ));
            }
        }
        assert_eq!(
            read,
            reading.pinned.coverage.0,
            "⚠⚠⚠ {read} of `{}`'s shipping payloads could be read, and the pin says {}: below it, \
             the reader has gone blind to a shape the driver writes and this claim is about less \
             than it was",
            reading.machine.document(),
            reading.pinned.coverage.0,
        );
        assert!(
            short.is_empty(),
            "⚠⚠⚠⚠ THE DRIVER SENDS LESS THAN `{}` READS. A missing key is `nil` to the datamodel, \
             so a guard on it is silently false and an `<assign>` of it writes nothing — which is \
             item 477's shape, a decision nothing carries:\n{}",
            reading.machine.document(),
            short.join("\n"),
        );
    }
}

/// ⛔⛔⛔⛔⛔ **AND A KEY THE DRIVER PUTS ON AN EVENT THAT THE DOCUMENT NEVER READS** — register
/// item 1024, the direction the gate above computes both halves of and only asks one of.
///
/// # Why this is a separate claim rather than a stricter one
///
/// `owed - keys` is *the driver sends less than the document reads*, and a missing key is `nil`.
/// `keys - owed` is the opposite defect and it costs nothing at run time, which is exactly why
/// nobody notices it: a value computed, put on an event, and dropped by the machine that was handed
/// it. This repository has now paid that shape three times — item 1018 (`PANE_DRIVEN_KEY`, a key
/// that could not be true), item 1021 (`PANE_BORNE_BY_KEY`, three weeks unread), item 1023's own
/// first run — and **every one was found by a person, not a gate**.
///
/// ⚠⚠ IT IS ASKED PER EVENT AND NOT PER STATE, deliberately. Whether a payload is OWED is
/// state-dependent — [`tolerant`] measures that, and `turn.blocked` owes three keys in `working`
/// and nothing in `reflecting` — but whether a key is EVER read is not: the document either names
/// it somewhere for that event or names it nowhere. So this direction needs no state and admits no
/// exemption, which is what makes it safe to ask where the other direction needed care.
///
/// ⚠ Only payloads written DOWN, on the gate above's terms: [`Rust::keys_of`] answers [`None`] for
/// one assembled a screen away rather than guessing, and the brief's is exactly that — which is why
/// item 1023 built `sprag_gate::briefing` to ask this same question of that one edge.
#[test]
fn every_key_the_driver_puts_on_an_event_is_one_the_document_reads() {
    for reading in readings() {
        let admitted = named(&reading.document);
        let mut dead = Vec::new();
        for site in reading.sites.iter().filter(|site| site.shipping) {
            let Some(keys) = site
                .payload
                .as_deref()
                .and_then(|beside| reading.rust.keys_of(beside))
            else {
                continue;
            };
            let owed = reading
                .carrying
                .get(&site.event)
                .cloned()
                .unwrap_or_default();
            let spare: Vec<&String> = keys
                .difference(&owed)
                .filter(|key| !admitted.contains(*key))
                .collect();
            if !spare.is_empty() {
                dead.push(format!(
                    "  {}:{} puts {spare:?} on `{}`, and nothing in {} reads {} off it",
                    site.file,
                    site.line,
                    site.event,
                    reading.machine.document(),
                    if spare.len() == 1 { "it" } else { "them" },
                ));
            }
        }
        assert!(
            dead.is_empty(),
            "⛔⛔⛔⛔ REGISTER ITEM 1024: the driver computes these keys, puts them on an event, \
             and the document never looks at them. Nothing fails, nothing is logged, and the cost \
             is paid on every raise — which is why the three earlier instances of this shape each \
             took a PERSON counting by hand to find.\n\
             Two answers, and the first one to check is whether the DOCUMENT should be reading it: \
             a key nobody reads is either a decision that never arrives or a computation to \
             delete.\n{}",
            dead.join("\n"),
        );
    }
}

/// ⚠⚠⚠⚠⚠ **AND HOW MUCH OF EACH DRIVER THESE CLAIMS REACH IS PINNED** — register item 1024's
/// residue, held per machine as [`Machine::coverage`].
///
/// # ⚠⚠⚠⚠⚠ Why the uncoverable half is COUNTED rather than described
///
/// [`Rust::keys_of`] answers [`None`] for a payload assembled elsewhere and handed over in a
/// variable, and it is right to: guessing would put a red on a shape nobody wrote down. But every
/// claim in this file is then *about the readable ones*, and the size of the other half decides how
/// much that is worth. Left in prose it is a sentence nobody re-measures — this repository's own
/// rule — so it is a pin: **a new raise the scan cannot follow moves the second number and says
/// so**, which is exactly the shape item 1024 was opened about.
///
/// ⚠ The FIRST number falling is the other alarm: sites went away, or the reader went blind to a
/// shape it used to follow, and those two are indistinguishable from here.
#[test]
fn how_much_of_each_driver_these_claims_can_read_is_what_they_could_read_before() {
    for reading in readings() {
        let mut read = 0usize;
        let mut opaque = Vec::new();
        for site in reading.sites.iter().filter(|site| site.shipping) {
            if site
                .payload
                .as_deref()
                .and_then(|beside| reading.rust.keys_of(beside))
                .is_some()
            {
                read += 1;
            } else {
                // ⚠ NAMED, NOT COUNTED — the number alone would leave the next reader re-deriving
                // WHICH sites are dark, which is the prose-nobody-re-measures this pin exists to
                // replace. The refusal prints them, so they are a fact the gate states rather than
                // one a person has to go and find.
                opaque.push(format!(
                    "  {}:{} raises `{}` with {}",
                    site.file,
                    site.line,
                    site.event,
                    site.payload.as_deref().unwrap_or("nothing beside it"),
                ));
            }
        }

        assert_eq!(
            (read, opaque.len()),
            reading.pinned.coverage,
            "⚠⚠⚠⚠⚠ WHAT THIS FILE'S CLAIMS REACH IN `{}` HAS MOVED.\n\
             The second number UP: a raise arrived whose payload this scan cannot follow, so every \
             claim here is silent about it — which is the state register item 1024 was opened on. \
             Either write the payload down where the event is spelled, or say here why this one \
             cannot be.\n\
             The first number DOWN: sites went away, or the reader stopped following a shape it \
             used to. Those two look identical from here, which is why this is a pin and not a \
             floor.\n\
             The sites this scan cannot read, as it found them:\n{}",
            reading.machine.document(),
            opaque.join("\n"),
        );
    }
}

/// ⚠⚠⚠⚠⚠ **AND WHICH KEYS EACH DOCUMENT NAMES WITHOUT READING IS PINNED** — register item 1024's
/// exemption, asserted rather than trusted.
///
/// # Why an equality, and why the exemption needs one at all
///
/// The gate above admits a key the document NAMES, because `ai_loop.scxml` plans in prose: `rule`
/// is published *"so a fork per decision is one more line above this one"*, with the reason the
/// line is not yet written. That is the document deciding, which is right — and it is also,
/// exactly, a way to silence a red by typing a sentence. Item 903's rule is that an exemption is
/// written down and its SIZE asserted; this is that, with the names, because there are few enough
/// to name.
///
/// ⚠ Measured ABOVE the pin: a key was named in commentary and is read by nothing. Either the
/// document means to route on it — then the guard belongs in the same commit — or somebody quieted
/// this gate. Measured BELOW: an affordance was taken up (good, and the key now appears in
/// [`data_carrying`]) or the commentary that admitted it is gone, which makes the driver's payload
/// key dead in the way item 1021 was for three weeks.
#[test]
fn every_key_a_document_names_without_reading_is_one_it_says_why_about() {
    for reading in readings() {
        let read: BTreeSet<String> = reading.carrying.values().flatten().cloned().collect();
        let unread: Vec<String> = named(&reading.document)
            .difference(&read)
            .cloned()
            .collect();

        assert_eq!(
            unread,
            reading
                .pinned
                .affordances
                .iter()
                .map(|key| (*key).to_owned())
                .collect::<Vec<String>>(),
            "⚠⚠⚠⚠⚠ THE SET OF KEYS `{}` NAMES BUT DOES NOT READ HAS MOVED, and the gate that \
             admits them is only as narrow as this line.\n\
             MORE than the pin: check the commentary that names the new one. A document that plans \
             to route on a key says so and says why it has not; a sentence added to quiet a red \
             says neither.\n\
             FEWER than the pin: an affordance was taken up — then it is read now and this is the \
             happy direction — or the sentence that admitted it was deleted, which makes the \
             driver's key dead publication with nothing left to say otherwise.",
            reading.machine.document(),
        );
    }
}

/// ⚠⚠⚠⚠⚠ **A PAYLOAD A FIXTURE SHARES UNDER A NAME IS THE DRIVER'S OWN** — item 507's residue, and
/// the half the repayment itself created.
///
/// The repair that fixed the fifteen introduced `TURN` and `ORDINARY`: constants whose own doc says
/// *what the driver puts on `turn.done`* and *on `judge`*, spelling the same keys `Raise::carrying`
/// spells, **in another file, with nothing holding them together**. So they are held here, from
/// both sides:
///
/// * every key the DOCUMENT reads must be in them, or a fixture is walking a state the product
///   never walks;
/// * no key the DRIVER does not send may be in them, or a fixture is proving the machine against a
///   payload nobody will ever raise.
///
/// ⚠ An inline literal is deliberately NOT held to this. A fixture asking one guard one narrow
/// question (`"{\"done\": true}"`) is asking about that guard, and a missing key is `nil`, which is
/// the answer it wants. A NAME reused across fifteen sites is a second spelling of a shared fact,
/// and that is the thing that drifts.
///
/// ⚠⚠ ASKED OF EVERY MACHINE, and the floor is asked ONCE across all of them: a second driver whose
/// fixtures share no constant is not a defect, and a workspace where NONE do is the reader having
/// stopped resolving a name to its literal.
#[test]
fn a_payload_a_fixture_shares_under_a_name_is_the_drivers_own() {
    let mut wrong = Vec::new();
    let mut sharing = 0usize;
    for reading in readings() {
        let driver = drivers_keys(&reading);
        let mut shared = BTreeMap::new();
        for site in reading.sites.iter().filter(|site| !site.shipping) {
            let Some(name) = site
                .payload
                .as_deref()
                .filter(|beside| reading.rust.shared(beside))
            else {
                continue;
            };
            shared.insert((name.to_owned(), site.event.clone()), site.line);
        }
        sharing += shared.len();

        for ((name, event), line) in &shared {
            let keys = reading
                .rust
                .keys_of(name)
                .unwrap_or_else(|| panic!("{name} resolved to a payload once and must again"));
            let owed = reading.carrying.get(event).cloned().unwrap_or_default();
            let missing: Vec<&String> = owed.difference(&keys).collect();
            if !missing.is_empty() {
                wrong.push(format!(
                    "  `{name}` stands in for `{event}` (line {line}) and lacks {missing:?}, which \
                     the document READS — so a fixture using it walks a state the product never \
                     walks",
                ));
            }
            let Some(sends) = driver.get(event) else {
                continue;
            };
            let invented: Vec<&String> = keys.difference(sends).collect();
            if !invented.is_empty() {
                wrong.push(format!(
                    "  `{name}` stands in for `{event}` (line {line}) and carries {invented:?}, \
                     which the driver never sends — the fixture is proving the machine against a \
                     payload no run will ever raise. The driver sends {sends:?}",
                ));
            }
        }
    }
    assert!(
        sharing >= 2,
        "⚠⚠⚠ this claim is about the constants the fixtures share for a driver payload, and it \
         found {sharing} across every machine: either they stopped being shared — in which case \
         delete this — or the reader has stopped resolving a name to its literal and the claim is \
         vacuous",
    );
    assert!(
        wrong.is_empty(),
        "⚠⚠⚠⚠⚠ A SHARED FIXTURE PAYLOAD HAS DRIFTED FROM THE DRIVER'S. Two spellings of one \
         payload in two files is what item 507's repayment left behind, and this is what holds \
         them together:\n{}",
        wrong.join("\n"),
    );
}

/// ⚠⚠⚠⚠⚠ **NO KEY IN A PAYLOAD THIS GATE READS IS SPELLED BY A NAME THE WORKSPACE DISAGREES
/// ABOUT** — the hazard item 516 would otherwise walk straight into, measured 2026-08-21.
///
/// # Why this exists before the payload that needs it
///
/// The driver writes a payload's keys as constants: `{MILESTONE: …, STANDING: …}`,
/// `{ScreenRule::TEXT_KEY: &said}`. [`Rust::keys_of`] resolves those, and a resolver keyed on the
/// LAST PATH SEGMENT is choosing blind whenever two types declare the same constant name — this
/// workspace has **eight distinct `WIRE_KEY`s** (`may_answer`, `hand`, `screen_rules`,
/// `handback_still_ms`, `await_person_ms`, `ready_timeout_ms`, `turn_within_ms`, `match`), three of
/// them in one file, and `brief`'s payload spells three of them.
///
/// ⚠⚠ Measured: nothing the gate resolves TODAY is ambiguous — every one is either unique or has
/// the same value at both declarations (`TEXT_KEY` is `"text"` in `screen.rs` and in `judge.rs`).
/// So this is a hazard closed BEFORE it bites rather than a defect repaired after. The day a
/// payload starts spelling a contested name, this says so by name instead of the claim above
/// passing on a key nobody wrote.
#[test]
fn no_payload_key_is_spelled_by_a_name_this_workspace_disagrees_about() {
    let mut guessed = Vec::new();
    for reading in readings() {
        let contested = reading.rust.ambiguous();
        assert!(
            !contested.is_empty(),
            "⚠⚠⚠ this workspace declares the same constant name with two different values in \
             several places, and finding NONE while reading for `{}` means the reader stopped \
             seeing constants at all — which would make this gate, and every key claim above it, \
             vacuous",
            reading.machine.event_type(),
        );

        for site in &reading.sites {
            let Some(payload) = site.payload.as_deref() else {
                continue;
            };
            for (name, values) in &contested {
                if spells(payload, name) {
                    guessed.push(format!(
                        "  {}:{} spells `{name}` in `{}`'s payload, and this workspace declares it \
                         as {values:?} — the gate cannot tell which, so the key it reports would \
                         be a guess. Resolve it through the `impl` that declares it, or rename one",
                        site.file, site.line, site.event,
                    ));
                }
            }
        }
    }
    assert!(
        guessed.is_empty(),
        "⚠⚠⚠⚠⚠ A PAYLOAD KEY IS SPELLED BY A CONTESTED NAME. A claim about what the driver sends \
         would be built on whichever declaration was read last:\n{}",
        guessed.join("\n"),
    );
}

/// ⚠⚠⚠⚠⚠ **THE STATES THAT TOLERATE A BARE RAISE ARE PINNED, BECAUSE A DRIVER RELIES ON ONE** —
/// item 515, turned from a note into a mechanism.
///
/// # What the driver relies on, measured
///
/// Two sites hand an event value on INDIRECTLY, past what a text scan can follow. `pump`'s
/// `other => other.into()` cannot be data-carrying at all — `TurnDone` and `TurnBlocked` have
/// explicit arms above it. `OuterLoop::reflect`'s `ended.into()` CAN be `turn.blocked`, which IS
/// data-carrying, and it is still correct: `reflecting` answers `turn.blocked` with ONE
/// unconditional edge to `awaiting_human`, so nothing indexes `_event.data` there.
///
/// ⚠⚠⚠ **THAT IS A FACT ABOUT THE DOCUMENT, AND THE DOCUMENT CAN CHANGE.** Put a `cond` on
/// `reflecting`'s `turn.blocked` and the driver's bare raise becomes the exact defect item 507 was
/// filed for — silently, because no spelled site moved and every other claim here stays green. The
/// ledger's answer was a sentence saying *reopen 515 if that happens*, which nothing enforces. This
/// is the enforcement: the tolerant set is pinned, and shrinking it is announced.
///
/// ⚠⚠ THE INDIRECT PIN IS ASKED ONLY OF A MACHINE THAT HAS DATA-CARRYING EVENTS, and that is a
/// measurement rather than an exemption: it exists to keep the tolerant pin's subject alive, and
/// [`tolerant`] is EMPTY for a machine no event of which carries data — asserted here, right below,
/// rather than assumed. `.into()` in a file that merely mentions such a machine's type is an
/// ordinary conversion, and pinning a list of those would be a pin about nothing.
#[test]
fn the_states_that_tolerate_a_bare_raise_are_the_ones_the_driver_was_written_against() {
    for reading in readings() {
        let document = reading.machine.document();
        let measured = tolerant(&reading.document);
        assert_eq!(
            measured,
            reading
                .pinned
                .tolerant
                .iter()
                .map(|(state, event)| ((*state).to_owned(), (*event).to_owned()))
                .collect::<BTreeSet<_>>(),
            "⚠⚠⚠⚠⚠ WHICH STATES OF `{document}` TOLERATE A BARE RAISE HAS MOVED. A pair LEAVING \
             this set is the dangerous direction: the document started reading `_event.data` where \
             it did not, and any driver site that hands that event on without a payload — \
             `OuterLoop::reflect` does, for `turn.blocked` — is now asking the datamodel to index \
             nil, which W3C SCXML 3.12.2 drops in silence. A pair ARRIVING is harmless but still \
             worth a person's eye",
        );

        if reading.carrying.is_empty() {
            assert!(
                measured.is_empty(),
                "⚠⚠⚠ `{document}` reads `_event.data` off nothing, so no state of it can tolerate \
                 a bare raise ANY differently from any other — a non-empty set here means \
                 `tolerant` and `data_carrying` disagree about the same document: {measured:?}",
            );
            continue;
        }

        // ⚠⚠⚠⚠ AND THE SITE THAT PIN PROTECTS IS DISCOVERED, NOT REMEMBERED — item 518. A pin
        // whose subject has been deleted is green forever, guarding nobody, in the voice of a live
        // rule.
        let sites = indirect(&subject(), &reading.machine);
        assert_eq!(
            sites,
            reading
                .pinned
                .indirect
                .iter()
                .map(|(func, name)| ((*func).to_owned(), (*name).to_owned()))
                .collect::<BTreeSet<_>>(),
            "⚠⚠⚠⚠⚠ THE INDIRECT HAND-OFFS INTO `{document}` MOVED. These are the raises no static \
             claim above can read — the event arrives in a variable — so each one depends on the \
             tolerant set pinned in this very test. A site GONE means a pin here may now be \
             guarding nobody; a site ADDED means a new raise nothing checks, and the states it can \
             run in must be tolerant",
        );
    }

    let loops: Vec<&Machine> = MACHINES
        .iter()
        .filter(|machine| !machine.tolerant.is_empty())
        .collect();
    assert!(
        !loops.is_empty(),
        "⚠⚠⚠ no machine in this workspace has a state that tolerates a bare raise, which would \
         make `OuterLoop::reflect`'s `ended.into()` a live defect rather than a permitted one. \
         Either the pins were emptied or `tolerant` stopped reading documents",
    );
}

/// ⚠⚠⚠ **AN EXEMPTION THAT HAS STOPPED MATTERING IS A DEAD RULE**, and a dead rule in a gate reads
/// exactly like a live one.
///
/// ⚠ Asked across EVERY machine: a file is exempt from this gate as a whole, so it earns its
/// exemption by tripping any one of them.
#[test]
fn every_exemption_is_still_load_bearing() {
    let all = rust_sources();
    let charts = charts();
    let mut tripped: BTreeMap<&str, usize> = EXEMPT.iter().map(|(file, _)| (*file, 0)).collect();
    for machine in driven(&all, &charts) {
        let document = charts
            .iter()
            .find(|chart| chart.file == machine.document())
            .map(|chart| chart.text.clone())
            .expect("the machine came from these charts");
        let carrying = data_carrying(&document);
        let rust = Rust::of(&all, &machine);
        for site in spelled(&all, &carrying, &rust) {
            if let Some(hits) = tripped.get_mut(site.file.as_str()) {
                *hits += usize::from(!site.carries);
            }
        }
    }

    for (file, why) in EXEMPT {
        assert!(
            tripped[file] > 0,
            "⚠⚠⚠ `{file}` is exempted because {why} — and it no longer trips this gate at all, \
             for any of the {} machine(s) this workspace drives. Delete the exemption: it is now a \
             hole with a reason attached",
            MACHINES.len(),
        );
    }
}
