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
//! # ⚠⚠⚠⚠⚠ Why a ratchet and not a fix — and why it outlived the refutation it was written under
//!
//! ⚠ This module was written while stages 2 and 3 of item 470 were REFUTED at the pinned SCE (item
//! 483: a host could not register its own `<send>`/`<invoke>` type, so an act could not leave the
//! document). **That was a fact about a REV, and it did not survive one.** At `e0fdd46b` a host
//! declares the types it serves to the build and registers a handler at run time; on 2026-08-25 the
//! first act crossed, and `closing` and `stopping` stopped announcing a name to the machine and
//! started asking this host to perform `prompt.say`. Twenty-eight driver arms went with it.
//!
//! ⚠ **FOUR HAVE CROSSED AS OF 2026-08-26**, and the count is stated here rather than left to the
//! sentence above because a status is what ages fastest in the file that measures it (item 470's
//! own R74 finding, made on this module): `closing` and `stopping` first, then `priming` — which
//! proved the move is not about ENDINGS — then `reflecting`, which asks for a third thing and so
//! proved `asks` is a vocabulary rather than a flag with two spellings. ⚠⚠ Only the FIRST of the
//! four cost the driver a row here; the other three each traded one mention of their state for
//! another, which is why [`crate::loop_shape::served_acts`] exists and why a round paying this
//! item may see exactly one of these numbers move.
//!
//! The decisions still cannot all move in one round — and meanwhile the defect GREW: the register
//! recorded 153 state-keyed sites in the driver on 2026-08-19 and this module measured 157 on
//! 2026-08-20, four of them added by the very rounds that were paying the item down.
//!
//! A ratchet is what turns "it grew again" from something a person has to notice into something the
//! suite says — and, because the pin is an EQUALITY, what turns "it shrank" into a number the
//! paying round has to write down. [`crate::loop_shape::served_acts`] watches the other direction.
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
    // ⚠ The same door `uncommented` closes on the other two needles, closed here for the same
    // reason. A phantom state from a comment fails LOUDLY rather than quietly — the pin gate would
    // demand a row for a state that does not exist — but a walk that reads commentary is wrong
    // whichever way it fails, and this is the walk the other two are measured against.
    let scxml = uncommented(scxml);
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
///
/// ⚠ The tag's own `>` is the one OUTSIDE its attribute values — see [`tag_end`]. XML lets a value
/// hold a bare `>`, and this document compares numbers inside expressions.
fn attribute(tag: &str, name: &str) -> Option<String> {
    let end = tag_end(tag)?;
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
    uncommented(scxml).matches("<onentry>").count()
}

/// The document with its XML comments removed, so a needle cannot match what a comment QUOTES.
///
/// # ⚠⚠⚠⚠⚠ The two halves of this file disagreed about whether commentary counts
///
/// [`crate::sources::rust_sources`] has always dropped a line that starts `//` — a driver arm
/// discussed in a comment is not a driver arm, and a test asserts it. The document walk beside it
/// did not: every needle here was matched against the raw text, so a `<send>` or an `<onentry>`
/// **quoted** in a comment counted as one performed.
///
/// ⚠⚠ Measured 2026-08-25 R76 before this existed — `served_acts` answered **2** for one real act
/// and one quoted beside it. And [`DOCUMENT`]'s house style is to quote the send a state USED TO
/// carry on the round its act moves, with seven acts still to move: the hazard was one comment
/// away, on the ONE meter that can see item 470's stage 2. ⚠ Worse than a miscount, because the
/// gate over that meter answers a count above its pin with *raise `SERVED_ACTS` … the debt is
/// being PAID* — a quoted act would have handed the next round a ratchet onto an act nobody moved.
///
/// ⚠ An unterminated comment swallows the rest of the file, which is what a parser would do with
/// it; a document that shipped one would not open at all.
///
/// ⚠⚠ PUBLIC since register item 800, whose gate reads the document from a test rather than from
/// this module. A second copy of *what a comment is* is where two readers of the same file come to
/// disagree — and this file's own subject is a needle that matched what a comment quoted.
#[must_use]
pub fn uncommented(scxml: &str) -> String {
    let mut kept = String::with_capacity(scxml.len());
    let mut rest = scxml;
    while let Some(open) = rest.find("<!--") {
        kept.push_str(&rest[..open]);
        let after = &rest[open + "<!--".len()..];
        let Some(close) = after.find("-->") else {
            return kept;
        };
        rest = &after[close + "-->".len()..];
    }
    kept.push_str(rest);
    kept
}

/// One prompt the document COMPOSES — an `<assign>` whose `location` names a prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Composed {
    /// The `location`, which is the prompt's name in the datamodel.
    pub prompt: String,
    /// The expression it is composed from, whitespace squeezed to single spaces.
    ///
    /// ⚠ Squeezed because rustfmt has no say here and the document wraps an expression wherever
    /// the line ran out: a needle looking for `+ milestone +` would miss it across a wrap.
    pub expr: String,
}

/// Every prompt the document composes, in document order — register item 800.
///
/// # ⚠⚠⚠ Why the parse is of the ASSIGN and not of the datamodel declaration
///
/// `<data id="turn_prompt" expr="''"/>` declares an empty string; the text an agent actually reads
/// is put together later, in `priming`, out of parts. A reader that looked at the declarations
/// would find no prompt naming anything and report a clean document — the vacuous green register
/// item 799 measured.
///
/// ⚠⚠ A prompt is a `location` ENDING in `prompt`, which is this document's whole naming
/// convention for them (`start_prompt`, `turn_prompt`, `reflect_prompt`, `end_prompt`,
/// `stop_prompt`, `dispute_prompt`, `unverified_prompt`). A caller that wants a fixed list gets a
/// list that goes stale; what this returns is whatever the document has, so a NEW prompt shows up
/// unclassified rather than unnoticed.
///
/// # Panics
///
/// When an `<assign>` tag or one of its quoted attributes does not close. That is the reader having
/// lost the document, and a walk that has stopped understanding its subject must not answer as if
/// it had understood it.
#[must_use]
pub fn composed_prompts(scxml: &str) -> Vec<Composed> {
    let text = uncommented(scxml);
    let mut found = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("<assign") {
        let after = &rest[at + "<assign".len()..];
        let end = tag_end(after)
            .unwrap_or_else(|| panic!("an `<assign` at byte {at} of the document never closes"));
        rest = &after[end..];
        let Some(prompt) = attribute(after, "location") else {
            continue;
        };
        if !prompt.ends_with("prompt") {
            continue;
        }
        // ⛔ A PROMPT WHOSE TEXT THIS READER CANNOT SEE IS RED, NOT ABSENT. SCXML also lets an
        // `<assign>` carry its value as a child element; a prompt written that way would be
        // composed out of something no caller here can judge, and answering *nothing to see* about
        // it is how a gate goes green over the one prompt it was built for.
        let expr = attribute(after, "expr").unwrap_or_else(|| {
            panic!("`{prompt}` is assigned without an `expr` this reader can read")
        });
        found.push(Composed {
            prompt,
            expr: expr.split_whitespace().collect::<Vec<_>>().join(" "),
        });
    }
    found
}

/// Where an opening tag ends, counting only a `>` that is OUTSIDE an attribute value.
///
/// ⚠⚠ XML lets an attribute value hold a bare `>`, and this document's own expressions compare
/// numbers. It happens to write `&gt;` today, which is a HABIT and not a guarantee — and a parser
/// that ends the tag on the first `>` would silently return an expression cut in half, which reads
/// exactly like a prompt that stopped naming something.
fn tag_end(after: &str) -> Option<usize> {
    let mut quoted = false;
    for (at, char) in after.char_indices() {
        match char {
            '"' => quoted = !quoted,
            '>' if !quoted => return Some(at),
            _ => {}
        }
    }
    None
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
/// ⚠⚠ The needle is the OPENING TAG with its type attribute, so a `<send>` addressed to some other
/// host is not counted, and neither is one a comment merely QUOTES. It is deliberately not a parse:
/// this crate reads artefacts as text on purpose, and what makes the number trustworthy is the
/// gate beside it that refuses a count of zero.
///
/// ⚠⚠⚠⚠⚠ **THE COMMENT HALF OF THAT SENTENCE WAS PROSE AHEAD OF THE CODE UNTIL 2026-08-25 R76**,
/// and it is left standing rather than softened because it is now true: this counted the raw text,
/// so it answered **2** for one act with one quoted beside it. The private `uncommented` beside
/// this says why that was one comment away from happening in [`DOCUMENT`] — named rather than
/// linked, because a public doc that links a private item does not build. Register item 644:
/// *prose that runs ahead of the code is the thing auditing the code.*
#[must_use]
pub fn served_acts(scxml: &str) -> usize {
    uncommented(scxml)
        .matches(&format!("<send type=\"{HOST_TYPE}\""))
        .count()
}

/// **HOW MANY SENDS THE DOCUMENT ANNOUNCES TO A MACHINE THAT IS NOT LISTENING** — every `<send>`
/// carrying no `type`, which is W3C SCXML 6.2's external event to SELF.
///
/// # ⚠⚠⚠⚠⚠ Why this is a number to drive to ZERO rather than one to grow
///
/// [`served_acts`] counts acts a host PERFORMS and must climb. This counts its opposite: a name
/// raised onto the machine's own queue that no `<transition>` listens for and no driver reads. It
/// looks like an instruction and is not one — `sprag-plugin`'s own
/// `the_machine_instructs_its_driver_through_its_state_not_through_its_sends` established that the
/// event-driven driver those sends imply **cannot be written**, because the one handle that looks
/// like a subscription mints a fresh empty queue on every call.
///
/// ⚠⚠ **AND AN ANNOUNCEMENT IS EXACTLY THE SILENCE ITEM 470 EXISTS TO END.** A `<send>` nobody
/// carries out is indistinguishable from one that worked — the failure that module's own
/// documentation names — so the five this document carried were not documentation of intent, they
/// were five places a reader could believe an effect was declared where nothing declares it. Every
/// one of the five had already been replaced by a `pass.do` word the driver really performs.
///
/// ⚠ Counted on the UNCOMMENTED text, for [`served_acts`]'s measured reason: this file's house
/// style is to quote the send a state USED to carry on the round its act moves, and a counter that
/// read the raw text would find those quotations and report behaviour nobody declares.
#[must_use]
pub fn announced_sends(scxml: &str) -> usize {
    let text = uncommented(scxml);
    text.match_indices("<send")
        .filter(|(at, _)| {
            let rest = &text[*at..];
            let tag = rest.find('>').map_or(rest, |end| &rest[..end]);
            !tag.contains("type=")
        })
        .count()
}

/// The three elements of SCXML that can CONTAIN a state, and so the only ones whose nesting can
/// change what a state's parent is. Everything else — `<transition>`, `<onentry>`, `<data>`,
/// `<send>` — is tracked by nobody here because it holds no states.
const NESTING: [&str; 3] = ["parallel", "state", "final"];

/// **THE IDS A `<parallel>` HOLDS DIRECTLY** — the document's REGIONS, in document order.
///
/// # ⚠⚠⚠⚠⚠ Why a gate needs this, measured rather than argued
///
/// Item 470's floor rests on an exemption: two sites in the driver name a state and neither one
/// DECIDES anything, so they stay. The ground recorded for BOTH of them on 2026-08-26 was *it names
/// a region root — a parallel configuration holds several states at once and the region's id is the
/// only handle the generated policy offers*.
///
/// **Half of that was wrong, and only the document could say so.** `work` is a direct child of
/// `<parallel id="running">` and the sentence is true of it. `standing_down` is a LEAF inside the
/// `orders` region — its parent is `orders`, not the parallel — so whatever its exemption stands
/// on, it is not that sentence. Two exemptions were recorded as one because prose cannot tell a
/// region root from a state inside a region, and this can.
///
/// ⚠ A region root is not the same question as *is this state active*: it is about the ARRANGEMENT,
/// which is exactly why no act a document could declare answers it.
#[must_use]
pub fn region_roots(scxml: &str) -> Vec<String> {
    let scxml = uncommented(scxml);
    let mut roots: Vec<String> = Vec::new();
    // What this walk is currently inside, innermost last. Only [`NESTING`] pushes.
    let mut open: Vec<&str> = Vec::new();
    for (at, _) in scxml.match_indices('<') {
        let rest = &scxml[at + 1..];
        if let Some(shut) = rest.strip_prefix('/') {
            // ⚠ A close pops only for a tag this walk PUSHED. `</onentry>` closes nothing here, and
            // popping for it would hand the next state the wrong parent.
            if NESTING.into_iter().any(|kind| {
                shut.strip_prefix(kind)
                    .is_some_and(|after| after.starts_with('>'))
            }) {
                open.pop();
            }
            continue;
        }
        let Some(kind) = NESTING.into_iter().find(|kind| rest.starts_with(*kind)) else {
            continue;
        };
        let after = &rest[kind.len()..];
        // ⚠ The name has to END here, or `<send>` would be read as a `<state>` whose name ran on.
        if !after.starts_with(char::is_whitespace) && !after.starts_with('>') {
            continue;
        }
        let Some(end) = after.find('>') else {
            continue;
        };
        if open.last() == Some(&"parallel")
            && let Some(id) = attribute(after, "id")
            && !roots.contains(&id)
        {
            roots.push(id);
        }
        // ⚠ `<state id="standing_down"/>` opens nothing: a self-closing tag is its own close, and a
        // walk that pushed it would put every later sibling one level too deep.
        if !after[..end].ends_with('/') {
            open.push(kind);
        }
    }
    roots
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

    /// ⚠⚠⚠⚠⚠ **A REGION ROOT IS NOT ANY STATE INSIDE A REGION**, and the whole worth of this
    /// reader is telling those two apart — item 470's floor rests on the difference, and a round
    /// that recorded one exemption for both got it wrong in prose because nothing could check.
    ///
    /// Three shapes in one document, and only ONE of them is a region:
    ///
    /// * `work` and `orders` — direct children of the `<parallel>`. **Regions.**
    /// * `standing_down` — a self-closing LEAF inside `orders`. Not a region, and its self-closing
    ///   tag must not push, or `idle` below it would read one level too deep.
    /// * `idle` — inside `work`, and `converged` — outside the parallel entirely. Neither.
    #[test]
    fn a_region_root_is_a_parallels_own_child_and_nothing_deeper() {
        let doc = r#"<scxml initial="running">
  <state id="lonely"/>
  <parallel id="running">
    <state id="work" initial="idle">
      <onentry><send event="pass"/></onentry>
      <state id="idle"/>
    </state>
    <state id="orders" initial="standing">
      <state id="standing"/>
      <state id="standing_down"/>
    </state>
  </parallel>
  <final id="converged"/>
</scxml>"#;
        assert_eq!(
            region_roots(doc),
            ["work", "orders"],
            "only the parallel's OWN children are regions",
        );
        // ⚠ The control the claim above is worth nothing without: every one of these IS a state of
        // the document, so a reader that answered *every state* would satisfy the line above too.
        for inside in [
            "standing_down",
            "standing",
            "idle",
            "lonely",
            "converged",
            "running",
        ] {
            assert!(
                !region_roots(doc).iter().any(|root| root == inside),
                "`{inside}` is not a region root and this reader must not say it is",
            );
        }
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

    /// ⚠⚠⚠⚠⚠ **A COMMENT IS NOT A DECISION, AND THIS DOCUMENT'S COMMENTS QUOTE SENDS.**
    ///
    /// [`DOCUMENT`]'s house style is to say what a state USED TO send on the round its act moves —
    /// seven of its comment lines quote a `<send>` tag today, one of them a whole element with its
    /// attributes. **Seven acts are still to move, and each one will be written that way.**
    ///
    /// ⚠⚠ So this is not a tidiness assertion. [`served_acts`] is the ONLY meter that can see item
    /// 470's stage 2 — the other two are measured blind to it — and the gate over it does not just
    /// count: on a count ABOVE its pin it INSTRUCTS the next round to *raise `SERVED_ACTS` … the
    /// debt is being PAID*. A quoted act would therefore hand a future round a ratchet onto an act
    /// nobody moved, in the voice of progress. Register item 453's blind ratchet, arriving by the
    /// one door this file had left open.
    ///
    /// ⚠ The reading of a `<send>` addressed elsewhere is the control that this is about COMMENTS
    /// and not about the needle being loose in general.
    #[test]
    fn an_act_quoted_in_a_comment_is_not_an_act_this_document_asks_for() {
        let real = "<send type=\"x-sprag-host\" event=\"prompt.say\"/>";
        assert_eq!(served_acts(real), 1, "the control: a real act is counted");

        let quoted = format!("<!-- what stood here was {real} -->\n{real}");
        assert_eq!(
            served_acts(&quoted),
            1,
            "⚠⚠⚠⚠⚠ A `<send>` QUOTED IN A COMMENT IS NOT AN ACT. This document says what a state \
             used to send every time one moves, so the next act to move brings this shape with it \
             — and counting it would raise the one pin that can see item 470's stage 2 onto an act \
             nobody performed.",
        );

        let elsewhere = "<send type=\"x-somebody-else\" event=\"prompt.say\"/>";
        assert_eq!(
            served_acts(elsewhere),
            0,
            "the control for the control: an act addressed to another host is not this host's",
        );

        assert_eq!(
            declared_acts("<!-- <onentry> --><onentry></onentry>"),
            1,
            "⚠⚠ and the same door on the other meter: a quoted `<onentry>` is not a block. It has \
             not bitten yet — no comment in the document quotes one today — which is exactly when \
             it is cheap to close.",
        );

        assert_eq!(
            document_states("<!-- <state id=\"phantom\"/> --><state id=\"real\"/>"),
            ["real"],
            "⚠⚠ and on the walk the other two are measured against. A state invented by a comment \
             fails LOUDLY — the pin gate would demand a row for it — but a walk that reads \
             commentary is wrong whichever way it fails.",
        );

        assert_eq!(
            served_acts(&format!("<!-- unterminated {real}")),
            0,
            "⚠ an unterminated comment swallows the rest, which is what a parser would do with it: \
             a document that shipped one would not open at all, so counting its tail as acts would \
             be counting a file nothing can run.",
        );
    }

    /// ⛔⛔⛔⛔⛔ **EVERY SHAPE A COMPOSED PROMPT REALLY HAS** — register item 800, and the reader
    /// its gate stands on.
    ///
    /// # ⚠⚠⚠⚠ Why the cases are the awkward ones rather than the tidy one
    ///
    /// The assigns in this document wrap wherever the line ran out, sit beside assigns that are not
    /// prompts at all, and are QUOTED in the commentary — this file's house style is to show the
    /// send a state used to carry. Each of those is a way a reader answers confidently about
    /// something that is not there, and register item 803's lesson is that a branch written for
    /// *the next document* has to be driven now.
    #[test]
    fn the_prompt_reader_sees_a_wrapped_assign_and_declines_what_is_not_one() {
        let wrapped = "<assign location=\"turn_prompt\"\n        expr=\"'go on' +\n              \
                       standing\"/>";
        assert_eq!(
            composed_prompts(wrapped),
            [Composed {
                prompt: "turn_prompt".to_owned(),
                expr: "'go on' + standing".to_owned(),
            }],
            "an expression the document wrapped is one expression, or a needle misses it across \
             the wrap",
        );

        // ⚠ DECLINED, three ways, and each is a way of answering about nothing.
        assert!(
            composed_prompts("<!-- <assign location=\"turn_prompt\" expr=\"'quoted'\"/> -->")
                .is_empty(),
            "a prompt this file's commentary QUOTES is not a prompt the document composes",
        );
        assert!(
            composed_prompts("<data id=\"turn_prompt\" expr=\"''\"/>").is_empty(),
            "the datamodel declares an EMPTY prompt; what an agent reads is composed later, and a \
             reader that looked here would find no prompt naming anything and call that clean",
        );
        assert!(
            composed_prompts("<assign location=\"turns\" expr=\"turns + 1\"/>").is_empty(),
            "an assign that is not a prompt is not one",
        );

        // ⛔⛔⛔⛔⛔ A BARE `>` INSIDE AN ATTRIBUTE, WITH THE ATTRIBUTES IN THE OTHER ORDER. The
        //   document writes `&gt;` and puts `location` first — both HABITS, neither a guarantee.
        //
        // ⚠⚠ THE ORDER IS THE WHOLE CASE, and the first draft of it was a DEAD CONTROL: written
        //   with `location` first, a reader that ended the tag at the first `>` still answered
        //   correctly, because the name it needed had already gone by. The mutation that removes
        //   the quote-awareness was GREEN against it. Only when the `>` comes BEFORE the name does
        //   ending early lose the prompt — and losing a prompt is this gate reporting a clean
        //   document about one it never read.
        assert_eq!(
            composed_prompts("<assign expr=\"turns > 0 ? a : b\" location=\"end_prompt\"/>"),
            [Composed {
                prompt: "end_prompt".to_owned(),
                expr: "turns > 0 ? a : b".to_owned(),
            }],
            "the tag ends at the `>` that is outside the quotes, not at the first one",
        );

        // ⚠ And the real document is the population the gate above measures.
        let document = std::fs::read_to_string(crate::sources::workspace_root().join(DOCUMENT))
            .expect("the loop's document is part of this workspace");
        let composed = composed_prompts(&document);
        assert!(
            composed.iter().any(|one| one.prompt == "start_prompt")
                && composed.iter().any(|one| one.prompt == "turn_prompt"),
            "the two prompts item 800 is about must both be found, or the reader is pointed \
             somewhere else: {composed:?}",
        );
    }
}
