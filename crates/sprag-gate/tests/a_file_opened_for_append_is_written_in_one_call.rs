//! **A FILE OPENED FOR APPEND IS WRITTEN IN ONE CALL** — register items 817 and 818, and the CLASS
//! that those two are instances of.
//!
//! # ⛔⛔⛔⛔⛔ What `O_APPEND` promises, and the half everybody reads into it
//!
//! It makes **one** write atomic against other writers: the kernel takes the offset and performs
//! the write in a single step, so two writers appending never land on top of each other. It
//! promises nothing whatever about **two** writes — and `write!`/`writeln!` are two or more.
//! `Write::write_fmt` calls `write_all` once per FORMAT PIECE, and a `File` buffers nothing, so a
//! formatted line leaves through one syscall per piece with another writer free to land between
//! them.
//!
//! Measured on this repository, twice, four hours apart:
//!
//!   * `sprag_terminal::pty` (item 817) — five pieces, 32 threads: **188 of 827 lines shredded**,
//!     and one wreck, `opened=opened=3432 live=opened= live=193317`, parses as the plausible
//!     number 193317. A gate reading that file would have reported it as a process's demand.
//!   * `sprag_plugin::review` (item 818) — two pieces. One ledger file per DAEMON
//!     (`durability::state_dir()` plus one bare `ledger_into`) and one THREAD per run, so two runs
//!     whose reviews end together are two writers on one file.
//!
//! Both were written by somebody who knew about `O_APPEND` and had reasoned about atomicity in a
//! comment. The reasoning was right; the spelling did not do it. That is why this gate is over the
//! SPELLING and not over anybody's argument (this workspace's rule 10: prose nobody measures).
//!
//! # ⚠⚠ What this scan can and cannot claim, said plainly so a green run is not misread
//!
//! It is a line scan over `crates/**/*.rs` with comments and `#[cfg(test)]` items already dropped
//! by [`sprag_gate::sources`]. It cannot tell WHICH file handle a macro writes to, so the question
//! it answers is deliberately wider than the defect: **a file that opens something for append does
//! not format through a write macro anywhere in its product code.** A green run means that shape is
//! absent — not that every append in this workspace is atomic.
//!
//! ⚠ There is no exemption list, and that is a measurement rather than a preference: all three
//! append sites in this workspace satisfy the rule outright today, so an exemption would be a door
//! nothing needs. This workspace's rule is that an escape hatch which cannot be empty is a gate
//! already routed around, and the moment a legitimate multi-write appender exists, the RED is what
//! brings somebody to decide — which is the point.

use sprag_gate::sources::{Source, outside_strings, rust_sources};

/// The spelling that opens a file for append.
///
/// ⚠ Matched against the file's SQUEEZED product code rather than a line, because rustfmt breaks
/// an `OpenOptions` chain across lines by width: `.append(true)` sits on its own line in
/// `sprag_plugin::review` and inside a longer chain elsewhere, and a line scan would see one and
/// not the other.
const APPEND: &str = ".append(true)";

/// The macros that are not one write.
const FORMATTING: [&str; 2] = ["writeln!(", "write!("];

/// How many appending files this workspace has today — a FLOOR, so a scan that found its way to
/// the wrong tree cannot read as clean.
///
/// ⚠ A floor and not an equality on purpose: a new appending file is not an offence, and this gate
/// has nothing to say about one until it also formats. The ratchet that counts sites belongs to
/// whoever wants that question; this one is about the spelling.
const APPENDING_FILES: usize = 3;

/// Every product line of `source` that formats through a write macro, as `path:line: text`.
///
/// ⚠ [`outside_strings`] for the reason that function exists: this file names both macros in its
/// own [`FORMATTING`], and a scan that read a quoted mention as a call would report itself — which
/// it did, on its first run, before this was here.
fn formats_in(source: &Source) -> Vec<String> {
    source
        .product
        .iter()
        .filter(|(_, line)| {
            let code = outside_strings(line);
            FORMATTING.iter().any(|macro_| code.contains(macro_))
        })
        .map(|(number, line)| format!("{}:{number}: {line}", source.file))
        .collect()
}

/// That file's product code, strings blanked and whitespace gone — see [`APPEND`] for the second
/// half and [`formats_in`] for the first.
fn squeezed_product(source: &Source) -> String {
    source
        .product
        .iter()
        .flat_map(|(_, line)| {
            outside_strings(line)
                .chars()
                .filter(|char| !char.is_whitespace())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// ⛔ **THE GATE.**
#[test]
fn no_file_that_appends_formats_through_a_write_macro() {
    let mut appending = Vec::new();
    let mut offences = Vec::new();
    for source in rust_sources() {
        if !squeezed_product(&source).contains(APPEND) {
            continue;
        }
        appending.push(source.file.clone());
        offences.extend(formats_in(&source));
    }

    assert!(
        appending.len() >= APPENDING_FILES,
        "this scan found {} file(s) opening for append and this workspace has at least \
         {APPENDING_FILES} — it is pointed at the wrong tree, and a probe pointed at nothing must \
         never read as clean. Found: {appending:?}",
        appending.len(),
    );
    assert!(
        offences.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEMS 817 AND 818: {} of these {} appending file(s) format through a \
         write macro. `O_APPEND` makes ONE write atomic and makes nothing else atomic, and \
         `write!`/`writeln!` are one `write_all` per format piece — so every one of these is a \
         window another writer's line can land in. Build the line first and hand it over in a \
         single `write_all(format!(..).as_bytes())`.\n  {}",
        offences.len(),
        appending.len(),
        offences.join("\n  "),
    );
}
