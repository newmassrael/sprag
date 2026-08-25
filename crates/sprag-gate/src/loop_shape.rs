//! Where the debt-repayment loop's DECISIONS live — register item 470.
//!
//! # The claim, and why it is not "is there any Rust"
//!
//! SCXML is designed not to perform I/O, so the line was never *Rust versus document*. It is:
//!
//! > **DECISIONS in the document, EFFECTS in the host.** Can a reader say what this loop DOES from
//! > `ai_loop.scxml` alone?
//!
//! Writing bytes to a pty, parsing a screen, spawning a process are EFFECTS and belong in Rust.
//! *"a matched needle means the service is down"*, *"a held run is not unattended"* are DECISIONS,
//! and today most of them are in the driver. Item 470 measured the shape: a Rust table keyed by the
//! document's own states is BY DEFINITION a second copy of the topology.
//!
//! # ⚠⚠⚠⚠⚠ Why a ratchet and not a fix
//!
//! Stages 2 and 3 of item 470 are REFUTED at the pinned SCE (item 483: a host cannot register its
//! own `<send>`/`<invoke>` type, so the act cannot leave the document). The decisions therefore
//! cannot all move yet — and meanwhile the defect GREW: the register recorded 153 state-keyed sites
//! in the driver on 2026-08-19 and this module measured 157 on 2026-08-20, four of them added by
//! the very rounds that were paying the item down.
//!
//! A ratchet is what turns "it grew again" from something a person has to notice into something the
//! suite says.
//!
//! # ⚠⚠⚠⚠⚠ The needle is derived from BOTH artefacts, which is what keeps it from going blind
//!
//! Register item 453's finding, in one sentence: *a blind ratchet is green forever, in exactly the
//! voice of a working one*, and nothing tells a needle it has gone narrow. That gate's needles were
//! two spellings of a chmod and the third spelling — the one every Rust reference reaches for —
//! walked past it.
//!
//! So nothing here is spelled by hand:
//!
//! * the STATES come from `ai_loop.scxml`, so a state added to the document is watched for in the
//!   driver from the moment it exists;
//! * the NAMES the driver reaches the state type through come from the driver's own `use` lines, so
//!   an alias or a glob import is seen rather than walked past.
//!
//! A ratchet whose needle is a constant can only ever see the spelling its author thought of.

use crate::sources::Source;
use std::collections::{BTreeMap, BTreeSet};

/// The loop's document, relative to the workspace root.
pub const DOCUMENT: &str = "crates/sprag-plugin/src/ai_loop.scxml";

/// The generated state type the driver keys its arms on.
///
/// ⚠ It is GENERATED from [`DOCUMENT`] by SCE at build time, which is why the state NAMES are not
/// the defect — the compiler keeps those in step, and adding a state to the document breaks every
/// exhaustive match until a person has looked at each. The defect is the BEHAVIOUR in those arms.
pub const STATE_TYPE: &str = "AiLoopState";

/// One place in this workspace's Rust where behaviour is keyed by a state of the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateKeyed {
    /// The file, relative to the workspace root.
    pub file: String,
    /// One-indexed line.
    pub line: usize,
    /// The document's own id for the state — `service_down`, not `ServiceDown`, so a refusal talks
    /// in the words the document uses.
    pub state: String,
    /// The line itself, so a refusal shows the site rather than only counting it.
    pub text: String,
}

/// Every state, parallel region and final state the document declares, in document order.
///
/// ⚠ `<final>` states are included deliberately: `blocked` and `converged` are ENDINGS, and the
/// driver's arms that turn an ending into a sentence are exactly the behaviour item 470 is about.
#[must_use]
pub fn document_states(scxml: &str) -> Vec<String> {
    let mut states = Vec::new();
    for (open, _) in scxml.match_indices('<') {
        let rest = &scxml[open + 1..];
        let Some(kind) = ["state", "parallel", "final"]
            .into_iter()
            .find(|kind| rest.starts_with(kind))
        else {
            continue;
        };
        let after = &rest[kind.len()..];
        if !after.starts_with(char::is_whitespace) {
            continue;
        }
        let Some(id) = attribute(after, "id") else {
            continue;
        };
        if !states.contains(&id) {
            states.push(id);
        }
    }
    states
}

/// The value of `name="…"` in `tag`, up to the tag's own `>`.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let end = tag.find('>')?;
    let needle = format!("{name}=\"");
    let at = tag[..end].find(&needle)? + needle.len();
    let value = &tag[at..];
    let close = value.find('"')?;
    Some(value[..close].to_owned())
}

/// The variant the code generator spells a document id as — `service_down` becomes `ServiceDown`.
#[must_use]
pub fn variant_of(id: &str) -> String {
    id.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect()
}

/// The names ONE file can reach a generated type through, taken from that file's own `use` lines.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Reaching {
    /// Path segments that name the type — `AiLoopState` itself and any `as` alias of it.
    pub paths: BTreeSet<String>,
    /// Whether the file glob-imported the variants, so a bare `Working` is a site here.
    pub glob: bool,
}

/// How `product` reaches `generated` — [`STATE_TYPE`] here, and the event type for
/// [`crate::payload`].
///
/// # ⚠⚠⚠⚠⚠ This is the anti-blindness half, and it is the half a hand-written needle lacks
///
/// `AiLoopState::Working` is what the driver writes today. It is not the only way to write it, and
/// the alternatives are the ordinary ones a person reaches for when a match gets wide:
/// `use …::AiLoopState as S;` then `S::Working`, or `use …::AiLoopState::*;` then a bare `Working`.
/// A ratchet needled on the literal string `AiLoopState::` is green through both — silently, and in
/// the voice of a working gate.
///
/// Reading the file's own imports means the rename is what TEACHES the needle, rather than what
/// blinds it.
///
/// ⚠ The type is a PARAMETER rather than this module's constant because the same blindness reaches
/// every generated name a gate needles on: item 507's gate reads `AiLoopEvent` through this, and a
/// second copy of the import walk is where two copies of a rule drift apart.
#[must_use]
pub fn reaching(product: &[(usize, String)], generated: &str) -> Reaching {
    let mut found = Reaching::default();
    for (_, line) in product {
        if !line.contains(generated) {
            continue;
        }
        if !line.starts_with("use ") && !line.starts_with("pub use ") {
            // A path used in an expression tells us nothing new; only an import can rename.
            found.paths.insert(generated.to_owned());
            continue;
        }
        found.paths.insert(generated.to_owned());
        if line.contains(&format!("{generated}::*")) {
            found.glob = true;
        }
        if let Some(alias) = alias_after(line, generated) {
            found.paths.insert(alias);
        }
    }
    found
}

/// The identifier in `… AiLoopState as Alias …`, when the line renames the type.
fn alias_after(line: &str, generated: &str) -> Option<String> {
    let at = line.find(generated)? + generated.len();
    let rest = line[at..].trim_start();
    let named = rest.strip_prefix("as ")?.trim_start();
    let alias: String = named
        .chars()
        .take_while(|char| char.is_alphanumeric() || *char == '_')
        .collect();
    (!alias.is_empty()).then_some(alias)
}

/// Every state-keyed site in `sources`, over the states `document` declares.
///
/// Only [`Source::product`] is read. A gate is not the defect: register item 470 proposed counting
/// every `AiLoopState::` in the driver, and 46 of the 157 there are inside its own test module — so
/// that ratchet would have gone RED on the round that added a gate and GREEN on the round that
/// added an arm.
#[must_use]
pub fn state_keyed(sources: &[Source], document: &[String]) -> Vec<StateKeyed> {
    let variants: Vec<(String, String)> = document
        .iter()
        .map(|id| (variant_of(id), id.clone()))
        .collect();

    let mut found = Vec::new();
    for source in sources {
        let reaching = reaching(&source.product, STATE_TYPE);
        if reaching.paths.is_empty() {
            continue;
        }
        for (line, text) in &source.product {
            for (variant, id) in &variants {
                let hits = if reaching.glob {
                    mentions(text, variant, &reaching.paths, true)
                } else {
                    mentions(text, variant, &reaching.paths, false)
                };
                for _ in 0..hits {
                    found.push(StateKeyed {
                        file: source.file.clone(),
                        line: *line,
                        state: id.clone(),
                        text: text.clone(),
                    });
                }
            }
        }
    }
    found
}

/// How many times `text` names `variant` through one of `paths` — or bare, when the file globbed.
fn mentions(text: &str, variant: &str, paths: &BTreeSet<String>, glob: bool) -> usize {
    text.match_indices(variant)
        .filter(|(at, _)| {
            // `Working` is not `WorkingSet`, and `AiLoopStateWorking` is not a path to one.
            let after = &text[at + variant.len()..];
            if after.starts_with(is_ident) {
                return false;
            }
            let before = &text[..*at];
            match before.strip_suffix("::") {
                Some(head) => paths.iter().any(|path| ends_on_word(head, path)),
                // Bare, and only a file that glob-imported the variants can mean one that way.
                None => glob && !before.ends_with(is_ident),
            }
        })
        .count()
}

/// Whether `char` can be part of an identifier, so a longer name is not read as a shorter one.
fn is_ident(char: char) -> bool {
    char.is_alphanumeric() || char == '_'
}

/// Whether `head` ends with `word` as a whole path segment rather than as a suffix of a longer one.
fn ends_on_word(head: &str, word: &str) -> bool {
    head.strip_suffix(word)
        .is_some_and(|before| !before.ends_with(is_ident))
}

/// The sites of [`state_keyed`] tallied per document state, every state present even at zero.
///
/// ⚠ Zero-valued entries are kept on purpose: a pinned table that only lists what is non-zero
/// cannot say *this state used to have arms and no longer does*, and that direction is the one that
/// records the debt being PAID.
#[must_use]
pub fn tally(sites: &[StateKeyed], document: &[String]) -> BTreeMap<String, usize> {
    let mut counted: BTreeMap<String, usize> = document.iter().map(|id| (id.clone(), 0)).collect();
    for site in sites {
        *counted.entry(site.state.clone()).or_default() += 1;
    }
    counted
}

/// How many acts the document itself declares — one per `<onentry>` block.
///
/// ⚠ This is the side that must GROW. Every act the document takes over is one the driver stops
/// holding, so a ratchet with only a ceiling on the Rust could be satisfied by deleting behaviour
/// rather than by moving it.
///
/// ⚠⚠⚠⚠⚠ **AND IT IS BLIND TO THE MOVE ITEM 470 STAGE 2 ACTUALLY MAKES** — measured 2026-08-25,
/// on the first act to move. `closing` and `stopping` already HAD an `<onentry>`; what changed is
/// that its `<send>` stopped announcing a name to the machine and started asking the HOST to
/// perform an act, carrying what to say and what it asks for as `<param>`s. Twenty-eight driver
/// arms went with it and **this number did not move at all**. A meter that cannot see the one move
/// the item is about is exactly the blind ratchet this file's own header warns against, so
/// [`served_acts`] is the other half.
#[must_use]
pub fn declared_acts(scxml: &str) -> usize {
    scxml.matches("<onentry>").count()
}

/// The Event I/O Processor type this crate's host serves — `crate`'s side of W3C SCXML 6.2.5.
///
/// ⚠ Spelled here rather than read from `sprag-plugin`, because this crate deliberately has no
/// dependencies: a gate that stands outside the suite must not fail to compile because the product
/// did. The two spellings are held together by `an_act_this_host_serves_is_declared_to_the_build`,
/// which reads the product's `build.rs` and this document with the one needle.
pub const HOST_TYPE: &str = "x-sprag-host";

/// How many acts the document asks THIS HOST to perform — one per `<send type="x-sprag-host">`.
///
/// # ⚠⚠⚠⚠⚠ Why this is a different question from [`declared_acts`]
///
/// An `<onentry>` block is a place where a document does something. A host-served `<send>` is a
/// document telling a host WHAT TO DO and WITH WHAT — the only construct in SCXML by which a
/// decision can leave the document and still be the document's. Item 470's second stage is measured
/// in these and in nothing else: every one of them is a fact that used to be derived in Rust from
/// the name of the state it belonged to.
///
/// ⚠⚠ The needle is the OPENING TAG with its type attribute, so a `<send>` that names the type in a
/// comment, or one addressed to some other host, is not counted. It is deliberately not a parse:
/// this crate reads artefacts as text on purpose, and what makes the number trustworthy is the
/// gate beside it that refuses a count of zero.
#[must_use]
pub fn served_acts(scxml: &str) -> usize {
    scxml
        .matches(&format!("<send type=\"{HOST_TYPE}\""))
        .count()
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

    #[test]
    fn a_document_yields_its_states_and_the_generators_spelling_of_them() {
        let doc = r#"<scxml>
  <parallel id="running">
    <state id="work" initial="idle">
      <state id="awaiting_human"/>
    </state>
  </parallel>
  <final id="peer_gone"/>
</scxml>"#;
        assert_eq!(
            document_states(doc),
            ["running", "work", "awaiting_human", "peer_gone"],
        );
        assert_eq!(variant_of("awaiting_human"), "AwaitingHuman");
        assert_eq!(variant_of("work"), "Work");
    }

    /// ⚠⚠⚠ A tag that merely CONTAINS the word is not the tag — `<stateful id="x"/>` is not a
    /// state, and neither is an `id` written after the tag has closed.
    #[test]
    fn a_tag_that_only_looks_like_a_state_is_declined() {
        assert!(document_states("<stateful id=\"x\"/>").is_empty());
        assert!(document_states("<state/><data id=\"x\"/>").is_empty());
        assert_eq!(document_states("<state id=\"a\"/><state id=\"a\"/>"), ["a"]);
    }

    /// ⚠⚠⚠⚠⚠ **BOTH DIRECTIONS.** Each row is a line as a driver could really carry it, plus the
    /// number of state-keyed sites owed. The three qualified spellings are the ones a person
    /// reaches for as a match gets wide, and item 453's whole lesson is that a needle written for
    /// one of them is green through the others without ever saying so.
    #[test]
    fn every_spelling_that_reaches_the_state_type_is_seen_and_the_rest_declined() {
        // (the file's Rust, sites owed)
        let table: &[(&str, usize)] = &[
            // What the driver writes today.
            (
                "use crate::sm::ai_loop::AiLoopState;\nmatch from {\nAiLoopState::Working => go(),\n}",
                1,
            ),
            // Two on one line — a match arm that folds states together.
            (
                "use crate::sm::ai_loop::AiLoopState;\nAiLoopState::Closing | AiLoopState::Stopping => true,",
                2,
            ),
            // Renamed on import, which no literal needle can follow.
            (
                "use crate::sm::ai_loop::AiLoopState as S;\nS::Working => go(),",
                1,
            ),
            // Glob-imported, so the variant stands bare.
            (
                "use crate::sm::ai_loop::AiLoopState::*;\nWorking => go(),",
                1,
            ),
            // ⚠ DECLINED — a file that never reaches the type has no sites, whatever it spells.
            ("Working => go(),", 0),
            // ⚠ DECLINED — a longer identifier that merely starts with a variant.
            (
                "use crate::sm::ai_loop::AiLoopState;\nlet WorkingSet = 1;\nAiLoopStateWorking;",
                0,
            ),
            // ⚠ DECLINED — another type that happens to share a variant name.
            (
                "use crate::sm::ai_loop::AiLoopState;\nPaneError::PeerGone(_) => retry(),",
                0,
            ),
        ];

        let document: Vec<String> = ["working", "closing", "stopping", "peer_gone"]
            .iter()
            .map(|id| (*id).to_owned())
            .collect();

        let mut wrong = Vec::new();
        for (rust, owed) in table {
            let read = state_keyed(&[source("a.rs", rust)], &document).len();
            if read != *owed {
                wrong.push(format!("owed {owed}, read {read} for {rust:?}"));
            }
        }
        assert!(
            wrong.is_empty(),
            "a ratchet that cannot see the ordinary way of committing the defect is green \
             forever in the voice of a working one: {wrong:#?}",
        );
    }

    #[test]
    fn an_import_teaches_the_needle_its_own_names() {
        let renamed = source("a.rs", "use crate::sm::ai_loop::AiLoopState as S;");
        let found = reaching(&renamed.product, STATE_TYPE);
        assert!(found.paths.contains("S") && found.paths.contains(STATE_TYPE));
        assert!(!found.glob);

        let globbed = source("a.rs", "use crate::sm::ai_loop::AiLoopState::*;");
        assert!(reaching(&globbed.product, STATE_TYPE).glob);

        assert_eq!(
            reaching(&source("a.rs", "let x = 1;").product, STATE_TYPE),
            Reaching::default()
        );
    }

    /// ⚠⚠ A state with no arms must still be IN the tally, or the table can never record a debt
    /// being paid down to nothing.
    #[test]
    fn a_state_with_no_arms_is_still_counted_at_zero() {
        let document: Vec<String> = ["working", "idle"]
            .iter()
            .map(|id| (*id).to_owned())
            .collect();
        let sites = state_keyed(
            &[source(
                "a.rs",
                "use x::AiLoopState;\nAiLoopState::Working => go(),",
            )],
            &document,
        );
        let counted = tally(&sites, &document);
        assert_eq!(counted.get("working"), Some(&1));
        assert_eq!(counted.get("idle"), Some(&0));
    }

    #[test]
    fn the_acts_a_document_declares_are_its_onentry_blocks() {
        assert_eq!(declared_acts("<onentry><raise event=\"a\"/></onentry>"), 1);
        assert_eq!(declared_acts("<onexit/>"), 0);
    }
}
