//! ⛔⛔⛔⛔⛔ **A `Source` IS DERIVED FROM A FILE AND NEVER ASSEMBLED BY HAND** — register item 1046.
//!
//! # ⚠⚠⚠ The compiler shouts once, and is then silent for ever
//!
//! Adding a field to `Source` broke every hand-built literal at once — **eight** of them on
//! 2026-09-11 — and the round that added it filled each one in. After that the type checks out
//! whatever is in there, so a fixture carrying `attributes: Vec::new()` when its text HAS
//! attributes reads exactly like one whose text has none. Nothing distinguishes *this file has no
//! such line* from *this fixture was never updated*.
//!
//! That is not a hypothetical failure. Item 1044's gate was written against a field chosen by the
//! same reasoning and **counted 0 in a workspace holding 33** — it compiled, it ran, and it
//! measured nothing. Four comments saying *no attribute in this case* were then written, and a
//! comment forbids nothing; item 1046's own `Done when` rules them out as the answer.
//!
//! # What replaced them
//!
//! `Source` carries a private field, so the struct literal is unavailable outside its module and
//! `Source::of(file, text)` is the only way to make one. Every field of a given `Source` now comes
//! from one text, and *forgot to fill this in* has no spelling. The six fixtures outside the module
//! were converted; the two inside it were too.
//!
//! ⚠ A fixture that needs specific line numbers writes the blank lines that put them there — see
//! `vocabulary`'s `a_file_with`. That is a real file, and a `Source` no file could produce is a
//! control proving something about an input the gate will never be handed. One of them was exactly
//! that before this item: `code` empty while `product` held three lines.
//!
//! # ⛔ What the compiler still cannot see, and this file does
//!
//! The private field stops assembly from OTHER modules. Inside `sources` itself the literal is
//! legal, so a second construction path can be added there without a word — which is how the first
//! one came to exist. Hence the first arm below. The second asks the other question: are the
//! fields of a derived `Source` actually consistent with one another, the way one text forces them
//! to be?

use sprag_gate::sources::rust_sources;

/// How many lines in this workspace name `Source`'s private marker: its DECLARATION and the one
/// CONSTRUCTOR, and nothing else.
///
/// ⚠⚠ A floor above the count is that many construction paths admitted in silence — item 926's
/// rule, the same one `TESTS_SWITCHED_OFF` carries. So it is an equality, and the refusal names
/// the number to write.
const DERIVATION_SITES: usize = 2;

/// ⛔⛔⛔ **ONE WAY TO MAKE A `Source`, COUNTED** — register item 1046.
///
/// The marker field is private, so every mention of it is either the declaration or a construction;
/// outside its module neither is possible at all. Two means *declared once, built in one place*. A
/// third is a second construction path, which is where a hand-assembled `Source` would come back.
///
/// ⚠ The needle is built rather than written so this file does not match itself.
#[test]
fn a_source_has_one_construction_path() {
    let marker = format!("_{}: ()", "derived");
    let sources = rust_sources();
    // ⚠⚠ ONE FILE, and that is the whole population rather than a narrowing. Outside this module
    // the private field makes the literal a COMPILE error, so no gate is needed there and a scan
    // that looked would answer about nothing. Inside it the literal is legal, which is the only
    // place a second construction path can appear.
    //
    // ⚠ Measured 2026-09-11: scanning the workspace for the bare marker found **13** lines, none of
    // them constructions — `_derived` is a common enough fragment that the needle has to be the
    // field as it is WRITTEN, and the file has to be named.
    let home = "crates/sprag-gate/src/sources.rs";
    let module = sources
        .iter()
        .find(|source| source.file == home)
        .unwrap_or_else(|| {
            panic!("⛔ ITEM 1046: {home} is where `Source` lives and it was not read")
        });
    let sites: Vec<(String, usize)> = module
        .code
        .iter()
        .filter(|(_, line)| line.contains(marker.as_str()))
        .map(|(line, _)| (module.file.clone(), *line))
        .collect();
    let rendered = || {
        sites
            .iter()
            .map(|(file, line)| format!("  {file}:{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    assert!(
        sites.len() <= DERIVATION_SITES,
        "⛔ ITEM 1046: {} site(s) name the marker, against the {DERIVATION_SITES} that are the \
         declaration and the single constructor. A third is a second way to build a `Source`, and \
         a hand-built one is how a fixture comes to hold a field nobody derived — item 1044's gate \
         counted 0 in a workspace holding 33 for exactly that reason.\n{}",
        sites.len(),
        rendered(),
    );
    assert!(
        sites.len() >= DERIVATION_SITES,
        "⚠⚠⚠ ITEM 1046: only {} site(s) name the marker and this gate admits {DERIVATION_SITES}. \
         Either the private field is gone — in which case any module can assemble a `Source` again \
         — or the constructor is. Write the smaller number only with the reason beside it.\n{}",
        sites.len(),
        rendered(),
    );
    // ⛔⛔⛔ AND THE MARKER IS STILL PRIVATE, which the count above cannot see: publishing the field
    // leaves both sites exactly where they are and quietly reopens the struct literal to every
    // module. Rule 6 — the escape hatch must not be able to disable its own gate.
    let published: Vec<&(usize, String)> = module
        .code
        .iter()
        .filter(|(_, line)| line.contains(marker.as_str()) && line.starts_with("pub "))
        .collect();
    assert!(
        published.is_empty(),
        "⛔ ITEM 1046: the marker is published ({published:?}), so `Source` can be assembled by \
         hand again from anywhere. Its whole job is to be unreachable outside its module.",
    );
}

/// ⛔⛔⛔⛔⛔ **AND THE FIELDS OF A DERIVED `Source` AGREE, BECAUSE ONE TEXT FORCED THEM** — register
/// item 1046.
///
/// Counting construction paths says a `Source` came from `Source::of`. It does not say that
/// constructor is coherent. These two facts are what "derived from one text" MEANS, and each is a
/// line the same splitter drew:
///
/// * what ships is a subset of what is code — `product` is `code` with the proving lines removed,
///   so a line in `product` and not in `code` is a `Source` no file can produce, which is what one
///   fixture held before this item (`code` empty, `product` three lines);
/// * an attribute line is never a code line — `code_lines` drops every line starting with `#` and
///   `attribute_lines` keeps exactly those, so the two are complements and cannot overlap. An
///   overlap means one of them has stopped being the complement of the other, which is the drift
///   item 213 names and the reason they are written beside each other.
#[test]
fn the_fields_of_a_source_agree_with_one_another() {
    for source in rust_sources() {
        let code: std::collections::BTreeSet<usize> =
            source.code.iter().map(|(line, _)| *line).collect();
        let stray: Vec<usize> = source
            .product
            .iter()
            .map(|(line, _)| *line)
            .filter(|line| !code.contains(line))
            .collect();
        assert!(
            stray.is_empty(),
            "⛔ ITEM 1046: {} ships line(s) {stray:?} that are not in its code at all. `product` is \
             `code` less the proving lines, so this `Source` is one no file could produce.",
            source.file,
        );
        let both: Vec<usize> = source
            .attributes
            .iter()
            .map(|(line, _)| *line)
            .filter(|line| code.contains(line))
            .collect();
        assert!(
            both.is_empty(),
            "⚠⚠⚠ ITEM 1046: in {} line(s) {both:?} are counted as BOTH code and attribute. Those \
             two readings are complements of one filter; an overlap means one of them has stopped \
             being the other's complement, and a caller cannot then be told which field to ask.",
            source.file,
        );
    }
}
