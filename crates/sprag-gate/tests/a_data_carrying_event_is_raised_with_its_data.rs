//! An event the loop's document reads `_event.data` off must never be raised without it — item 507.
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
//! # ⚠⚠ Why the claim is about SPELLED sites, stated rather than implied
//!
//! [`sprag_gate`] takes no dependencies by charter, so nothing here parses Rust. A driver that
//! computes an event into a variable and attaches a payload three functions later is outside this
//! gate's reach — `OuterLoop::watch` answers `AiLoopEvent::TurnDone` as a value and its caller
//! decides what to carry. What IS inside it: every place this workspace writes an event's name
//! where the event is handed on, which is where all fifteen lived and where every payload it writes
//! down is decided.

use std::collections::{BTreeMap, BTreeSet};

use sprag_gate::loop_shape::DOCUMENT;
use sprag_gate::payload::{Rust, Spelled, data_carrying, spelled, variant_of_event};
use sprag_gate::sources::{Source, rust_sources, workspace_root};

/// The events `ai_loop.scxml` reads `_event.data` off — measured 2026-08-20, and PINNED.
///
/// ⚠⚠ Pinned for item 498's reason exactly: a claim over a DISCOVERED set is green whether it
/// discovered nine events or none, so the first thing asserted is what was discovered. A tenth
/// data-carrying event is then ANNOUNCED — and announcing it is the point, because the round that
/// adds one has to decide what its payload is and which raises owe it.
const CARRYING: &[&str] = &[
    "brief",
    "judge",
    "reflect.applied",
    "reflect.done",
    "reflect.none",
    "review.done",
    "screen.matched",
    "turn.blocked",
    "turn.done",
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

fn document() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is this loop's document: {why}", path.display()))
}

/// This workspace's Rust, minus the files [`EXEMPT`] names.
fn subject() -> Vec<Source> {
    rust_sources()
        .into_iter()
        .filter(|source| !EXEMPT.iter().any(|(file, _)| *file == source.file))
        .collect()
}

/// The document's reads, this workspace's raising vocabulary, and every site the two meet at.
fn measured() -> (BTreeMap<String, BTreeSet<String>>, Rust, Vec<Spelled>) {
    let carrying = data_carrying(&document());
    let sources = subject();
    let rust = Rust::of(&sources);
    let sites = spelled(&sources, &carrying, &rust);
    (carrying, rust, sites)
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
fn drivers_keys(sites: &[Spelled], rust: &Rust) -> BTreeMap<String, BTreeSet<String>> {
    let mut keys: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for site in sites.iter().filter(|site| site.shipping) {
        if let Some(read) = site
            .payload
            .as_deref()
            .and_then(|beside| rust.keys_of(beside))
        {
            keys.entry(site.event.clone()).or_default().extend(read);
        }
    }
    keys
}

/// ⚠⚠⚠⚠⚠ **A PROBE POINTED AT NOTHING MUST NEVER READ AS CLEAN.** Every claim below is satisfied
/// by a measurement that found nothing, so this is the one that says the walk reached the loop.
#[test]
fn the_measurement_reaches_the_document_and_the_rust_that_raises_into_it() {
    let (carrying, rust, sites) = measured();

    assert!(
        carrying.len() > 5,
        "`{DOCUMENT}` is read for the events that carry data and this walk found only {}: a reader \
         pointed at the wrong file answers about the wrong file",
        carrying.len(),
    );
    assert!(
        sites.len() > 30,
        "this workspace raises those events in dozens of places and this walk found only {}: the \
         gate is measuring nothing and would be green forever",
        sites.len(),
    );

    // ⚠⚠ THE SEEDS ARE THE RUNTIME'S API AND ARE THE ONE THING HERE THAT IS SPELLED. If the engine
    // ever renames a door, this is where it is announced — rather than the raiser closure quietly
    // finding nothing and every claim below passing on an empty set.
    for door in ["process_event", "raise_external"] {
        assert!(
            rust.raisers().contains_key(door),
            "⚠⚠⚠⚠⚠ `{door}` is one of the engine's two doors and the seed the raiser set grows \
             from. Without it the closure finds no helpers, no site is a raise, and this gate goes \
             green in the voice of a working one: {:?}",
            rust.raisers().keys().collect::<Vec<_>>(),
        );
    }
    assert!(
        rust.envelope().is_some(),
        "⚠⚠⚠ the driver wraps an event in an envelope when it has a payload (`impl \
         From<AiLoopEvent> for …`), and that conversion carries NOTHING — so a reader that cannot \
         find the envelope cannot see the driver's own way of committing this defect",
    );

    // ⚠ AND THE SPELLING RULE IS MEASURED, not assumed: the generator's variant for each of these
    // events must be a name this workspace actually writes, or the needle is looking for a word
    // nobody uses and every site is invisible.
    let text: String = subject()
        .iter()
        .flat_map(|source| source.code.iter().map(|(_, line)| line.clone()))
        .collect();
    for event in carrying.keys() {
        let variant = variant_of_event(event);
        assert!(
            text.contains(&format!("::{variant}")),
            "⚠⚠⚠⚠ the document carries `{event}` and nothing in this workspace spells \
             `::{variant}`. Either the generator's naming changed — in which case this needle is \
             blind and every claim below is vacuous — or an event the document reads data off is \
             raised by nobody at all",
        );
    }

    let files: BTreeSet<&str> = sites.iter().map(|site| site.file.as_str()).collect();
    for owner in [
        "crates/sprag-plugin/src/outer.rs",
        "crates/sprag-plugin/src/ai_loop.rs",
    ] {
        assert!(
            files.contains(owner),
            "{owner} is where the driver and the fifteen fixtures live, and the walk must reach \
             it: {files:?}",
        );
    }
}

/// ⚠⚠ **WHICH EVENTS CARRY DATA IS A FACT ABOUT THE DOCUMENT, AND A NEW ONE IS ANNOUNCED.**
#[test]
fn the_events_this_document_reads_data_off_are_the_ones_this_gate_was_written_for() {
    let carrying = data_carrying(&document());
    assert_eq!(
        carrying.keys().map(String::as_str).collect::<Vec<_>>(),
        CARRYING,
        "⚠⚠⚠⚠⚠ THE SUBJECTS MOVED. Either the document started reading `_event.data` off another \
         event — in which case decide, here, what its payload is and check that every raise of it \
         carries one — or it stopped reading one. A set that quietly grew would leave the new \
         event's raises unchecked, which is the state `turn.done` and `judge` were in for months",
    );

    // ⚠ The keys are NOT pinned, and the reason is stated at the reader: a `<data>` this loop
    // invites a caller to author is item 494's subject, and a second pin on the same names here
    // would go red on every round that adds one while saying nothing this gate is about.
    for (event, keys) in &carrying {
        assert!(
            !keys.is_empty(),
            "{event} is in the carrying set and reads no key at all, which cannot happen unless \
             the reader is attributing reads to the wrong event",
        );
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
    let (carrying, _, sites) = measured();

    let bare: Vec<&Spelled> = sites.iter().filter(|site| !site.carries).collect();
    assert!(
        bare.is_empty(),
        "⚠⚠⚠⚠⚠ {} SITE(S) HAND A DATA-CARRYING EVENT ON WITH NO `_event.data`. The document reads \
         keys off each of these events, so the datamodel is asked to index nil: W3C SCXML 3.8 \
         abandons the rest of the block that raised the error and W3C SCXML 3.12.2 drops the error \
         itself unless something answers it. That is not a failure a reader sees — it is a state \
         half-entered, in the voice of one that worked.\n\n{}\n\nwhat the document reads: {:?}",
        bare.len(),
        bare.iter()
            .map(|site| format!(
                "  {}:{} raises `{}` through `{}` carrying {}",
                site.file,
                site.line,
                site.event,
                site.through,
                site.payload.as_deref().unwrap_or("nothing at all"),
            ))
            .collect::<Vec<_>>()
            .join("\n"),
        bare.iter()
            .map(|site| (&site.event, carrying.get(&site.event)))
            .collect::<BTreeMap<_, _>>(),
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
    let (carrying, rust, sites) = measured();

    let mut short = Vec::new();
    let mut read = 0usize;
    for site in sites.iter().filter(|site| site.shipping) {
        let Some(keys) = site
            .payload
            .as_deref()
            .and_then(|beside| rust.keys_of(beside))
        else {
            continue;
        };
        read += 1;
        let owed = carrying.get(&site.event).cloned().unwrap_or_default();
        let missing: Vec<&String> = owed.difference(&keys).collect();
        if !missing.is_empty() {
            short.push(format!(
                "  {}:{} puts {:?} on `{}`, and the document reads {missing:?} off it",
                site.file, site.line, keys, site.event,
            ));
        }
    }
    assert!(
        read > 5,
        "⚠⚠⚠ only {read} of the driver's payloads could be read at all, which is too few for this \
         claim to be about anything — the reader has gone blind to the shapes the driver writes",
    );
    assert!(
        short.is_empty(),
        "⚠⚠⚠⚠ THE DRIVER SENDS LESS THAN THE DOCUMENT READS. A missing key is `nil` to the \
         datamodel, so a guard on it is silently false and an `<assign>` of it writes nothing — \
         which is item 477's shape, a decision nothing carries:\n{}",
        short.join("\n"),
    );
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
#[test]
fn a_payload_a_fixture_shares_under_a_name_is_the_drivers_own() {
    let (carrying, rust, sites) = measured();
    let driver = drivers_keys(&sites, &rust);

    let mut shared = BTreeMap::new();
    for site in sites.iter().filter(|site| !site.shipping) {
        let Some(name) = site.payload.as_deref().filter(|beside| rust.shared(beside)) else {
            continue;
        };
        shared.insert((name.to_owned(), site.event.clone()), site.line);
    }
    assert!(
        shared.len() >= 2,
        "⚠⚠⚠ this claim is about the constants the fixtures share for a driver payload, and it \
         found {}: either they stopped being shared — in which case delete this — or the reader \
         has stopped resolving a name to its literal and the claim is vacuous",
        shared.len(),
    );

    let mut wrong = Vec::new();
    for ((name, event), line) in &shared {
        let keys = rust
            .keys_of(name)
            .unwrap_or_else(|| panic!("{name} resolved to a payload once and must again"));
        let owed = carrying.get(event).cloned().unwrap_or_default();
        let missing: Vec<&String> = owed.difference(&keys).collect();
        if !missing.is_empty() {
            wrong.push(format!(
                "  `{name}` stands in for `{event}` (line {line}) and lacks {missing:?}, which the \
                 document READS — so a fixture using it walks a state the product never walks",
            ));
        }
        let Some(sends) = driver.get(event) else {
            continue;
        };
        let invented: Vec<&String> = keys.difference(sends).collect();
        if !invented.is_empty() {
            wrong.push(format!(
                "  `{name}` stands in for `{event}` (line {line}) and carries {invented:?}, which \
                 the driver never sends — the fixture is proving the machine against a payload no \
                 run will ever raise. The driver sends {sends:?}",
            ));
        }
    }
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
    let (_, rust, sites) = measured();
    let contested = rust.ambiguous();
    assert!(
        !contested.is_empty(),
        "⚠⚠⚠ this workspace declares the same constant name with two different values in several \
         places, and finding NONE means the reader stopped seeing constants at all — which would \
         make this gate, and every key claim above it, vacuous",
    );

    let mut guessed = Vec::new();
    for site in &sites {
        let Some(payload) = site.payload.as_deref() else {
            continue;
        };
        for (name, values) in &contested {
            if spells(payload, name) {
                guessed.push(format!(
                    "  {}:{} spells `{name}` in `{}`'s payload, and this workspace declares it as \
                     {values:?} — the gate cannot tell which, so the key it reports would be a \
                     guess. Resolve it through the `impl` that declares it, or rename one",
                    site.file, site.line, site.event,
                ));
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

/// ⚠⚠⚠ **AN EXEMPTION THAT HAS STOPPED MATTERING IS A DEAD RULE**, and a dead rule in a gate reads
/// exactly like a live one.
#[test]
fn every_exemption_is_still_load_bearing() {
    let carrying = data_carrying(&document());
    let all = rust_sources();
    let rust = Rust::of(&all);
    let sites = spelled(&all, &carrying, &rust);

    for (file, why) in EXEMPT {
        let hits = sites
            .iter()
            .filter(|site| site.file == file && !site.carries)
            .count();
        assert!(
            hits > 0,
            "⚠⚠⚠ `{file}` is exempted because {why} — and it no longer trips this gate at all. \
             Delete the exemption: it is now a hole with a reason attached",
        );
    }
}
