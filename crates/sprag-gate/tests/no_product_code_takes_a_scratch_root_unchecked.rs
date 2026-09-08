//! ⛔⛔⛔⛔⛔ **PRODUCT CODE DOES NOT ASK THE OPERATING SYSTEM FOR A SCRATCH ROOT AND THEN TRUST
//! THE ANSWER** — register item 794.
//!
//! # What `std::env::temp_dir()` does, measured rather than assumed
//!
//! It answers a RELATIVE path when `TMPDIR` is set-and-empty. Measured 2026-08-31 with `rustc`:
//!
//! ```text
//! TMPDIR unset : temp_dir="/tmp"  joined="/tmp/sprag-probe"  absolute=true
//! TMPDIR=      : temp_dir=""      joined="sprag-probe"       absolute=false
//! ```
//!
//! Nothing downstream refuses the result. `git -C <repo> worktree add --detach -q
//! sprag-check-probe HEAD` exits **0** and leaves `?? sprag-check-probe/` INSIDE the repository,
//! because git resolves a relative path against its own `-C`. `create_dir_all` and `File::create`
//! are the same. So a socket, a checkout or a run directory silently lands in whatever directory
//! the process was launched from, and every reader that resolves the same name from somewhere else
//! looks in a different place.
//!
//! # ⚠⚠ WHY THIS IS A SECOND AXIS RATHER THAN A WIDER `hooks_cannot_pass_in_silence`
//!
//! Item 794's own done-when asks that question before answering it, so it was measured:
//!
//! | | that gate (shell) | this one (Rust) |
//! |---|---|---|
//! | population | `.githooks/`: 7 files, 12 `mktemp` lines | product `.rs`: `temp_dir()` calls |
//! | predicate | did the taker read the status | is the root usable where it is taken |
//!
//! The intersection is EMPTY, re-measured 2026-08-31: `.githooks/` holds **zero** `temp_dir` across
//! all 7 of its files, and every `mktemp` under `crates/` sits in a comment or a string literal —
//! that gate's own filter and refusal text, and this paragraph — so **no Rust code calls it**. Its
//! population comes from a directory WALK of `.githooks`, which cannot reach a crate. Widening its
//! name would have made both the file name (`hooks_…`) and the test name (`no_hook_…`) false.
//!
//! # ⛔⛔⛔⛔⛔ WHAT THE FIRST DRAFT OF THIS FILE GOT WRONG, AND HOW IT WAS FOUND
//!
//! It split product from harness on `#[cfg(test)]` alone, and wrote in its own doc that the split
//! came out **8 product / 163 test**. Re-derived before the gate had ever been executed — the run
//! that would have run it died earlier, at `cargo check --locked` — the same predicate answers
//! **77 product**, because `#[cfg(test)]` is a marker of an inline module in `src/`, and an
//! INTEGRATION test has no such marker: every one of `crates/*/tests/*.rs` is test code from its
//! first line and this file called all 74 of those call sites product. The gate would have been
//! red on arrival, at 77 sites of which 76 are not product at all — and three of the 77 were **this
//! file's own filter string**.
//!
//! ⭐ Two lessons, both already written in this repository's register and both re-earned here:
//! prose that nobody re-runs is not evidence (the `8 / 163` was prose), and a gate that has not
//! been EXECUTED has not been measured, however carefully it was read.
//!
//! ⇒ So the split is by CARGO TARGET, which is what actually decides whether a line ships:
//! `src/**` and `build.rs` are compiled into the crate; `tests/`, `benches/` and `examples/` are
//! built only by `--all-targets` and linked into nothing a user runs. Inside a product file,
//! `#[cfg(test)]` still separates the inline harness. **Anything that matches none of those shapes
//! is classified PRODUCT**, so a layout nobody anticipated arrives here as a red line to read
//! rather than as a silent pass.
//!
//! # ⚠⚠⚠ AND WHY THE POPULATION IS PRODUCT CODE RATHER THAN EVERY CALL
//!
//! Rule 5 — is there a path by which this reaches zero? The harness half cannot reach zero in one
//! round: it is 163 call sites whose remedy is a different one (they litter the repository under
//! `TMPDIR=` — a suite run that way on 2026-08-31 left **131** untracked entries under `crates/*/`
//! and failed **414** tests, against 0 and 0 for the same tree with a normal `TMPDIR`). A
//! population that cannot reach zero in one round makes a gate that is red forever and therefore
//! read by nobody, so that half is registered as its own item — and it is COUNTED here rather than
//! waved through, by [`the_harness_half_cannot_grow_without_being_read`], because an exemption
//! nobody measures is how the population grows back.

use std::path::{Path, PathBuf};

use sprag_gate::sources::outside_strings;

/// ⛔ **The harness half, as it stood when item 794's product half reached zero.**
///
/// ⛔⛔ **HELD EXACTLY, NOT AS A CEILING.** A ceiling rots: pay ten sites down and the slack it
/// leaves admits ten new ones with nothing going red — which is this repository's own lesson that a
/// blind ratchet is green forever. Equality makes a move in EITHER direction a line somebody edits
/// and a reason somebody reads. Item 795's done-when is to walk this to 0 and delete the constant
/// and its test along with it.
///
/// ⚠ **ASKED OF THE GATE, NOT DERIVED BY A SECOND SCRIPT.** A first draft of this line said 165,
/// from an `awk` that blanked quoted spans with `"[^"]*"` and so stopped at the first ESCAPED quote
/// in this file's own `"…\"env::temp_dir()\"…"`. The number here is what
/// [`the_harness_half_cannot_grow_without_being_read`] printed when the ceiling was set to zero on
/// purpose: a ceiling with slack in it is an exemption that can grow silently, which is the one
/// thing this constant exists to prevent.
///
/// # 163 → 161, 2026-09-01, register item 802 paying down item 795
///
/// `sprag-tui`'s PTY gate took both of its roots from the operating system, and one of them became
/// the `XDG_STATE_HOME` handed to every daemon, client and CLI run that file spawns. Under
/// `TMPDIR=` that home is relative — which the daemon must IGNORE — so the file's isolation was
/// undone silently and 67 daemons persisted into the tester's own state home. Converted to
/// `sprag_scratch::scratch_root()`, which refuses the root where it is taken.
///
/// ⚠ AND THE NUMBER MOVED BY ONE LESS THAN THE CONVERSION, WHICH IS WORTH THE SENTENCE: a new
/// assertion message in `sprag-host` spelled the call inside a `\`-continued string literal, and
/// [`outside_strings`] keeps a line that ENDS inside a string as code — deliberately, in the
/// direction of a red to read. The prose was rephrased rather than the filter widened; naming the
/// call in words costs nothing and an exemption for "it was only a message" costs the gate.
/// ⚠ AND BY ONE ON 2026-09-03: register item 871's gate spawns a daemon of its own, so it takes a
/// state root like every other daemon gate here. It wanted a SECOND root as well — a directory to
/// stand a fake agent binary in — and that one was folded under the first instead, so the gate
/// costs this population one site rather than two and `DaemonGuard` takes both away together. The
/// ratchet is what asked the question; the answer was to litter less, not to record more.
///
/// ⚠ AND BY ONE AGAIN THE SAME DAY, TWICE: register items 865's ⑸ and 870 each need a daemon, so
/// each needs a state root. Both went to ONE root apiece from the start, on the line above's answer
/// — the second and third times this ratchet's question was worth asking and the first two times it
/// was already answered before it was put.
/// ⭐⭐ **AND DOWN BY FOUR ON 2026-09-06 — 164 → 160, register item 927 paying 795 down.** The four
/// `sprag-promoted-*` cases in `sprag-host`'s CLI suite each took their own root from the operating
/// system to build a fake `bin/` in. They now go through `promoted_scratch`, which asks
/// `sprag_scratch::scratch_root()` once and — the reason item 927 touched them at all — reaps the
/// directories DEAD predecessors left, which their own `remove_dir_all` never could: it deleted
/// only the identical name, and the name carries the pid. Thirteen of them were standing, holding
/// 2,499.6 MB.
///
/// ⚠ Lowered here rather than left as slack, which is what this constant's own doc demands and what
/// register item 926 had to build into `north-star` for the same reason: a floor above the count is
/// exactly that many new sites admitted in silence.
///
/// ⭐⭐⭐ **AND DOWN BY TWENTY-FIVE ON 2026-09-06 — 160 → 135, register item 795 paid down against
/// the families that dominate the scratch root's COUNT.** `sprag-wire-it` (1,922), `sprag-gate`'s
/// five tags (520 apiece), `sprag-gate-bin`'s four (491 apiece), `sprag-cli-tree` (399),
/// `sprag-cli-it`, `sprag-rt`, `sprag-loop-tree`, `sprag-other-tree`, `sprag-no-tree`,
/// `sprag-not-a-tree`, `sprag-sweep-mute`, `sprag-checker`, `sprag-695`, `sprag-619` and
/// `sprag-mcp`'s six. Each now takes its name from `sprag_scratch::scratch_for`, which mints it and
/// sweeps the same prefix's dead owners in one call — see the block below this constant for why
/// the conversion alone would not have been the fix.
const HARNESS_SITES_REGISTERED: usize = 135;

// ⛔⛔⛔⛔⛔ THE SECOND AXIS: A NAME NOBODY CAN EVER COLLECT — register items 795 and 930, and the
// thing converting a site to `scratch_root()` does NOT do.
//
// ⚠ A BLOCK COMMENT AND NOT A DOC ONE, because what it used to document is gone. It stood on
// `PER_RUN_NAMES_REGISTERED` until item 930 took that population to zero; a doc comment with no
// item under it attaches itself to whatever comes next, which is how this file first learned the
// rule (`clippy::empty_line_after_doc_comments`, on the very edit that deleted the constant).
//
// # Why this is a second population and not a wider `HARNESS_SITES_REGISTERED`
//
// The file's header asks that question of the shell gate before answering it; the same question is
// owed here, and it was measured rather than argued:
//
// | | that ratchet | this one |
// |---|---|---|
// | population | lines calling `env::temp_dir()` | lines calling `scratch_root()` and minting a per-run name |
// | predicate | is the root usable where it is taken | can what is put there ever be collected |
//
// The intersection is empty by construction — a converted site LEAVES the first population and
// ENTERS this one — and that is precisely why both live in one file. Read alone, the first number
// falling looks like the litter being paid down. Measured 2026-09-06T14:24Z, in the same scratch
// root, it was not: `sprag-869-<pid>` had been converted for item 794 and stood at **124
// directories**, because a name carries the pid of the run that made it, so that run's own
// `remove_dir_all` matches only itself and its `Drop` never runs when it is killed.
//
// # ⚠ Rule 5 — is there a path by which this reaches zero?
//
// Yes, and it is why the population is *per-run* names rather than every `scratch_root()` caller.
// A caller that joins a FIXED name, or that wants the root itself to hand on, has no predecessor
// problem and could never be zero; including it would have made a gate that is red forever and
// therefore read by nobody. Every site in THIS population is one `scratch_for` call away, and on
// 2026-09-06 the last of them took it.
//
// # 26 -> 4 -> 0, 2026-09-06, register items 795 and 930
//
// The gate was written with a floor of 0 on purpose — the idiom `HARNESS_SITES_REGISTERED`'s own
// doc records — and printed **26**. Fourteen were harness and went the same day, leaving four that
// the round registered rather than moved, on the argument that `scratch_for` SWEEPS and a sweep
// inside a shipped process is a behaviour change owed its own measurement.
//
// ⛔⛔⛔ THAT ARGUMENT WAS HALF WRONG, AND MEASURING IT IS WHAT ITEM 930 PAID. Re-derived
// 2026-09-06T15:27Z:
//
//   * `live_agent.rs` is declared `#[cfg(test)] mod live_agent;` in `sprag-host/src/lib.rs`, so
//     the whole module is harness and nothing ships it. This file's classifier called it PRODUCT
//     because `test_module_starts_at` looks for `#[cfg(test)]` INSIDE the file and this one's gate
//     is on the DECLARATION. That is the conservative direction the classifier's own doc asks for
//     — a shape it does not recognise arrives as a line to read — and the line was read.
//   * `sprag-latency` is a benchmark a person runs by hand. Its three sites are all setup — two
//     spawn a daemon, one is a `OnceLock` — and nothing in this tree invokes the binary: across
//     the 338 tracked files it appears in no `*.sh`, `*.yml`, `*.toml`, `.githooks/` file or
//     `Makefile`, and in no `.rs` line that spawns it. Every other mention is prose quoting rows.
//     ⚠⚠ THE FIRST DRAFT OF THIS BULLET SAID *"nothing under `.githooks`, `scripts` or
//     `.github`"* — AND `scripts/` DOES NOT EXIST IN THIS REPOSITORY. A grep of a path that is
//     not there answers "none" for the same reason it answers "none" when there is genuinely
//     nothing, so a third of that evidence was *not looked*. The claim above names file TYPES
//     over the tracked set instead, which is a population anybody can count.
//
// ⚠⚠ The one real cost was in the `OnceLock`, and it was not the one the argument named: its first
// call sat two lines inside a timed row reporting **1.96-8.38 us**, while a sweep of this machine's
// 16,055-entry scratch root measures **20-40 ms**. Hoisting that initialisation out of the timed
// region — which also removed a `create_dir_all` that only one side of a printed ratio was paying
// — is what let the site take the seam. See `sprag-latency.rs`'s own block.
//
// # ⛔⛔⛔⛔⛔ AND THE TEST DID NOT GO WITH THE CONSTANT
//
// Item 930's done-when said to delete both when the number reached 0, by analogy with
// `HARNESS_SITES_REGISTERED`. THE ANALOGY DOES NOT HOLD AND THE PRESCRIPTION WAS WRONG — and the
// first draft of THIS paragraph got the reason half right, which is worth the four lines.
//
// It said the harness ratchet may be deleted at 0 *"because the gate above it still holds the
// population, so it never stops being watched"*. Asked of the file rather than remembered:
//
//   `grep -n 'call_sites()' <this file>`  ->  three hits, two of them assertions
//   line 450: `no_product_code_takes_a_scratch_root_unchecked`  filters `Where::Product` ONLY
//   line 481: `the_harness_half_cannot_grow_without_being_read` filters `Where::Harness` ONLY
//
// ⇒ the sibling watches the PRODUCT half and nothing else. Delete the harness ratchet at 0 and a
// new harness `env::temp_dir()` site arrives in silence — so NEITHER ratchet may be deleted with
// its test, and this axis, which has no sibling at all, certainly may not. The CONSTANT went — a
// floor of zero is not a floor, it is a number that can only rot upward — and the test became an
// emptiness assertion, the same shape as `no_product_code_takes_a_scratch_root_unchecked`, which
// reds on the FILE AND LINE of a new site rather than on a count somebody has to update.

/// The tree this ratchet counts — through the one door, register item 809.
///
/// ⚠⚠ IT MATTERS MOST HERE. This gate's whole verdict is an EQUALITY against
/// [`HARNESS_SITES_REGISTERED`], so a walk of the wrong tree does not merely mis-report: it is the
/// one artefact that could answer *which tree was walked* and it would be answering about a tree
/// nobody asked for. That equality is what proved, after the fact, that the 2026-09-01 rounds had
/// judged this workspace — the number 161 exists in no other tree — and the proof only works while
/// the walk goes through the door that checks.
fn repo_root() -> PathBuf {
    sprag_gate::sources::workspace_root()
}

/// Whether a file's lines are compiled into something a user runs.
///
/// ⛔⛔⛔⛔ **RULE 6 LIVES HERE.** This is the only thing that can excuse a call site, so it is
/// deliberately wrong in the direction that costs a false RED: every shape it does not recognise
/// is [`Where::Product`]. A crate laid out some way this workspace has never used arrives as a line
/// in the failure message — a person reading one path — instead of a call site riding through.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Where {
    /// Compiled into the crate: `src/**`, and `build.rs`, which runs on whoever is building.
    Product,
    /// Built only by `--all-targets` and linked into nothing shipped: `tests/`, `benches/`,
    /// `examples/`.
    Harness,
}

/// Classify a repo-relative `crates/…` path by the Cargo target it belongs to.
fn where_it_lives(rel: &str) -> Where {
    // `crates` / `<crate>` / `<target-dir-or-file>` / …
    match Path::new(rel).components().nth(2).map(|c| c.as_os_str()) {
        Some(dir) if dir == "tests" || dir == "benches" || dir == "examples" => Where::Harness,
        _ => Where::Product,
    }
}

/// Every tracked `.rs` file under `crates/`, as `(repo-relative path, text)`.
///
/// ⚠⚠ Found by WALKING, for the reason the shell gate walks `.githooks/`: a hardcoded list decides
/// alone which files are looked at, and the one it leaves out is exactly the one nobody is
/// watching. A crate added tomorrow is in this population without anyone remembering to add it.
fn rust_files() -> Vec<(String, String)> {
    let root = repo_root();
    let mut found = Vec::new();
    let mut stack = vec![root.join("crates")];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .unwrap_or_else(|why| panic!("{} must be readable: {why}", dir.display()));
        for entry in entries {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                // `target` is build output, not source anybody wrote.
                if path.file_name().is_some_and(|name| name == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|why| panic!("{} must be text: {why}", path.display()));
                let rel = path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .into_owned();
                found.push((rel, text));
            }
        }
    }
    assert!(
        !found.is_empty(),
        "crates/ held no Rust files — this gate would then be asserting nothing",
    );
    found.sort();
    found
}

/// The lines that are CODE. Comments carry the reasoning, and this repository's reasoning quotes
/// the very call being hunted — four doc lines name `std::env::temp_dir()` while explaining why it
/// is dangerous, and scanning those would red on the documentation that justifies this gate.
fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| {
            !line.is_empty()
                && !line.starts_with("//")
                && !line.starts_with("///")
                && !line.starts_with("//!")
                && !line.starts_with('*')
        })
}

// ⚠⚠ `outside_strings` — *a code line with its double-quoted strings blanked out* — was written
// here and now lives in [`sprag_gate::sources`], imported above. It moved the day a SECOND gate
// needed it (register item 818, the append-in-one-call scan), which is the moment a copy would have
// been made and the two would have begun to drift. Its own reason is on it, there.

/// Where a file's `#[cfg(test)]` module begins, if it has one.
///
/// ⚠ This separates the INLINE harness inside a product file. It is not what tells a test file from
/// a product one — [`where_it_lives`] is, and the first draft of this gate confusing the two is
/// what made it red at 77 sites. The answer here is conservative the same way: the FIRST
/// `#[cfg(test)]`, so a module spelled some other way leaves its calls classified as product.
fn test_module_starts_at(text: &str) -> Option<usize> {
    text.lines()
        .enumerate()
        .find(|(_, line)| line.trim().starts_with("#[cfg(test)]"))
        .map(|(index, _)| index + 1)
}

/// Every `env::temp_dir()` call site under `crates/`, as `(Where, "path:line: text")`.
///
/// The seam is not in here at all: `sprag-scratch` is the one crate that may make the call, because
/// it is the one that checks the answer.
fn call_sites() -> Vec<(Where, String)> {
    let mut sites = Vec::new();
    for (name, text) in rust_files() {
        if Path::new(&name).starts_with("crates/sprag-scratch") {
            continue;
        }
        let inline_harness_from = test_module_starts_at(&text);
        let file = where_it_lives(&name);
        for (number, line) in code_lines(&text) {
            let code = outside_strings(line);
            if !code.contains("env::temp_dir()") {
                continue;
            }
            let placed = match file {
                Where::Harness => Where::Harness,
                Where::Product if inline_harness_from.is_some_and(|start| number > start) => {
                    Where::Harness
                }
                Where::Product => Where::Product,
            };
            sites.push((placed, format!("{name}:{number}: {line}")));
        }
    }
    sites
}

/// How many code lines an expression may run to before this gate stops trying to delimit it.
///
/// ⚠ A cap is needed because an unbalanced line this walker misreads would run the span to the end
/// of the file. When the cap is hit the site is counted anyway — rule 6: an expression the gate
/// could not read is not one it may wave through, and the refusal names the line so a person reads
/// it rather than a number quietly absorbing it.
const EXPRESSION_LINES: usize = 12;

/// How far one code line opens or closes parentheses, with quoted spans already blanked.
fn depth_of(code: &str) -> i32 {
    let opened = i32::try_from(code.matches('(').count()).unwrap_or(i32::MAX);
    let closed = i32::try_from(code.matches(')').count()).unwrap_or(i32::MAX);
    opened - closed
}

/// The line numbers in `text` at which a PER-RUN scratch name is minted on top of `scratch_root()`.
///
/// # ⚠⚠ The span is the EXPRESSION, and the first draft delimited it with `;`
///
/// That draft counted `IsolatedCheckout::of(dir, &sprag_scratch::scratch_root()).map(…)` and
/// `Self::resolve(…, &sprag_scratch::scratch_root(), …)` — a tail expression and an argument, both
/// of which hand the ROOT on and neither of which names anything. Their statements end with no
/// semicolon at all, so the walker ran to its cap and counted them under the rule that says an
/// unreadable span is a red. The rule was right and the delimiter was wrong.
///
/// So the span runs while the text is unbalanced, and one line further whenever the next line opens
/// a method call — which is exactly how `scratch_root()` newline `.join(format!(…))` reads, and
/// this workspace wraps it that way five times. Pure, so
/// [`the_per_run_detector_answers_both_ways`] can drive every arm from literals instead of hoping
/// the tree happens to hold one of each.
fn per_run_name_lines(text: &str) -> Vec<usize> {
    let lines: Vec<(usize, String)> = code_lines(text)
        .map(|(number, line)| (number, outside_strings(line)))
        .collect();
    let mut found = Vec::new();
    for (at, (number, code)) in lines.iter().enumerate() {
        if !code.contains("scratch_root()") {
            continue;
        }
        let mut span = String::new();
        let mut depth = 0;
        let mut closed = false;
        for (offset, (_, code)) in lines.iter().skip(at).take(EXPRESSION_LINES).enumerate() {
            span.push_str(code);
            depth += depth_of(code);
            if depth > 0 {
                continue;
            }
            let chained = lines
                .get(at + offset + 1)
                .is_some_and(|(_, next)| next.starts_with('.'));
            if !chained {
                closed = true;
                break;
            }
        }
        if !closed || span.contains("process::id()") {
            found.push(*number);
        }
    }
    found
}

/// Every per-run name minted outside the seam, as `"[where] path:line: text"`.
///
/// ⚠ The `Where` is carried into the message rather than into a second constant. Item 794's file
/// splits its two halves because one of them REACHED zero and the other could not; nothing here has
/// reached anything yet, and inventing that boundary before the measurement supports it would be a
/// number about a division somebody assumed.
fn per_run_name_sites() -> Vec<String> {
    let mut sites = Vec::new();
    for (name, text) in rust_files() {
        // The seam itself mints them; that is what it is for.
        if Path::new(&name).starts_with("crates/sprag-scratch") {
            continue;
        }
        let inline_harness_from = test_module_starts_at(&text);
        let file = where_it_lives(&name);
        let numbered: Vec<(usize, &str)> = code_lines(&text).collect();
        for number in per_run_name_lines(&text) {
            let line = numbered
                .iter()
                .find(|(at, _)| *at == number)
                .map_or("", |(_, line)| *line);
            let placed = match file {
                Where::Harness => Where::Harness,
                Where::Product if inline_harness_from.is_some_and(|start| number > start) => {
                    Where::Harness
                }
                Where::Product => Where::Product,
            };
            sites.push(format!("{placed:?} {name}:{number}: {line}"));
        }
    }
    sites
}

// ⛔⛔⛔⛔⛔ THE THIRD AXIS: A ROOT THE MACHINE WAS NEVER ASKED FOR — register item 931, and the one
// shape BOTH ratchets above are blind to BY CONSTRUCTION.
//
// [`call_sites`] and [`per_run_name_lines`] each read [`outside_strings`], which blanks every
// double-quoted span before the predicate is applied. That is right for them — this repository's
// reasoning quotes the very calls it hunts — and it means a path SPELLED OUT inside a literal is
// invisible to both. A `format!` that writes the root down calls neither `env::temp_dir()` nor
// `scratch_root()`, so neither number moved while four such sites were minting names, and 745
// `sprag-standin-<pid>` files stood in this machine's scratch root AT 2026-09-06T16:31:43Z with
// nothing in the workspace able to name them.
//
// # Two harms, not one
//
//   * `TMPDIR` is never read, so a harness that isolated a run by naming its own root is undone in
//     silence: the run writes into the machine's real scratch directory whatever it was told.
//   * The name carries no prefix any seam knows, so `sprag_scratch::reap_predecessors` can never
//     be asked about it. That is the second axis's harm arriving by a road the second axis cannot
//     see.
//
// # ⚠⚠ WHY THE POPULATION IS A MINTED NAME AND NOT EVERY SPELLED ROOT
//
// Item 931's own done-when asks that question before answering it, and it was answered by ASKING
// THIS GATE rather than a second script — the lesson `HARNESS_SITES_REGISTERED`'s own doc records,
// applied to the axis being built. Force [`mints_a_name`] to `true` and read the list
// [`no_scratch_path_is_spelled_where_the_machine_should_have_been_asked`] prints: **79** code lines
// under `crates/` spell a scratch root, and today every one of them is a VALUE — a fixture path, an
// expected argv, a shell-quoting sample, a socket address nothing ever binds, a `const` a test
// hands every arm so the arm says which root it would have used. **Four were minting names** when
// item 931 was opened, and they are the four this axis took. Forbidding a value would make this
// number a count of something else, which is a mistake item 794's first draft made once already.
//
// The line between them is the one rule 5 draws for the axis above: a FIXED spelled path is one
// entry, reused by every run, and cannot grow. A spelled path carrying a component this RUN mints
// — a `format!` interpolation, the shell's own pid, an `mktemp` template — is unbounded AND
// uncollectable. So the population is *a spelled root with a minted name under it*, and it reaches
// zero exactly as the axis above did: one `scratch_for` call per site.
//
// # ⚠⚠⚠ RULE 6 — WHAT THIS CANNOT SEE, MEASURED RATHER THAN LEFT TO BE DISCOVERED
//
// It is a gate on a SPELLING, as both axes above are (`env::temp_dir()`, `scratch_root()`), and it
// reads ONE code line: the root and the mint have to be in the same breath. Two neighbours are
// therefore out of reach, and both were measured before being accepted:
//
//   * **The expression, wrapped.** Keyed the way the axis above is keyed — the root, then
//     [`EXPRESSION_LINES`] of span — the same tree answers **five** sites rather than four, and the
//     fifth is `sprag.rs:14628`'s `.find(|line| line.contains(<a fixed json path>))`, which spells
//     a FIXED path: the chain continues on the next line into an `unwrap_or_else` whose panic
//     message interpolates a name, and the span borrows that brace. A span costs a false red here
//     and buys a shape this workspace does not hold.
//   * **The root bound to a name first** — a `const` holding the root, and a mint somewhere else in
//     the file. A detector for it was written and run: **23** lines bind a spelled root, and the
//     "minting uses" it finds for them are almost entirely OTHER identifiers of the same name —
//     `dir`, `here`, `named`, `state`, `sequence`, `kept` — because a name-based reader has no
//     scope. Closing that road needs the compiler, not a line filter, and a gate whose population
//     is dominated by collisions is one nobody reads.
//
// ⇒ Both are recorded in the register under item 931 with the commands that produce those numbers,
// so the next reader re-derives them rather than trusting this paragraph.

/// The roots `TMPDIR` can answer.
///
/// ⚠ **THE ORDER HERE DECIDES NOTHING** — [`literal_root_at`] takes the EARLIEST position, so a
/// `/var/tmp` is read from its own first character rather than as a shorter root with four
/// characters in front of it, whichever way round this array is written. A first draft of this doc
/// said "longest first", which would have been a sentence nothing enforced, and
/// [`the_literal_root_detector_answers_both_ways`] asserts the position instead.
///
/// ⚠ `XDG_RUNTIME_DIR`'s per-user runtime directory is deliberately NOT here, though this workspace
/// spells it on 32 code lines. The harm this axis names is *the machine was not asked*, and the
/// machine is asked about a scratch root through `TMPDIR`; the runtime directory is a different
/// variable with its own resolver (`sprag_rpc::resolve_socket_path`) and would be its own axis.
/// Every one of those 32 lines is a fixed value today — measured with this file's own predicate, 0
/// minted — so nothing is riding through on the omission.
const LITERAL_ROOTS: [&str; 2] = ["/var/tmp", "/tmp"];

/// Where a scratch root is spelled out on `line`, if one is.
fn literal_root_at(line: &str) -> Option<usize> {
    LITERAL_ROOTS
        .iter()
        .filter_map(|root| line.find(root))
        .min()
}

/// Whether a spelled path, read from its root onward, carries a component THIS RUN mints.
///
/// ⚠ A DOUBLED BRACE IS `format!`'s ESCAPED ONE AND NOT AN INTERPOLATION. The distinction is
/// load-bearing rather than pedantic: `scratch-litter`'s usage message spells the shell expansion a
/// person should run, braces doubled so `format!` prints them, and reading that as a minted name
/// would red this gate on the one instrument in the workspace whose whole subject is scratch
/// litter.
///
/// Pure, so [`the_literal_root_detector_answers_both_ways`] can drive every arm from a fixture
/// instead of hoping the tree happens to hold one of each.
fn mints_a_name(tail: &str) -> bool {
    // The shell's own pid, an `mktemp` template, and Rust's answer spelled beside the root rather
    // than into it. Named in words here; spelling them is [`LITERAL_MARKS`]' job.
    if LITERAL_MARKS.iter().any(|mark| tail.contains(mark)) {
        return true;
    }
    let bytes = tail.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'{' {
            if bytes.get(at + 1) == Some(&b'{') {
                at += 2;
                continue;
            }
            return true;
        }
        at += 1;
    }
    false
}

/// The non-brace marks of a name minted per run: the shell's pid, an `mktemp` template, and Rust's
/// process id spelled next to the root.
const LITERAL_MARKS: [&str; 3] = ["$$", "XXXXXX", "process::id()"];

/// The line numbers in `text` at which a scratch root is SPELLED and a name minted under it.
fn literal_root_lines(text: &str) -> Vec<usize> {
    code_lines(text)
        .filter_map(|(number, line)| {
            let at = literal_root_at(line)?;
            mints_a_name(&line[at..]).then_some(number)
        })
        .collect()
}

/// Every minted name under a spelled root, as `"[where] path:line: text"`.
///
/// ⚠ The `Where` is carried into the message rather than into a second constant, for the reason
/// [`per_run_name_sites`] gives: this axis reached zero in the round that opened it, and inventing
/// a boundary no measurement supports would be a number about a division somebody assumed.
fn literal_root_sites() -> Vec<String> {
    let mut sites = Vec::new();
    for (name, text) in rust_files() {
        // The seam spells a root in the test that proves it refuses a relative one; that is what it
        // is for, and it is the same exemption [`call_sites`] makes for the same crate.
        if Path::new(&name).starts_with("crates/sprag-scratch") {
            continue;
        }
        let inline_harness_from = test_module_starts_at(&text);
        let file = where_it_lives(&name);
        let numbered: Vec<(usize, &str)> = code_lines(&text).collect();
        for number in literal_root_lines(&text) {
            let line = numbered
                .iter()
                .find(|(at, _)| *at == number)
                .map_or("", |(_, line)| *line);
            let placed = match file {
                Where::Harness => Where::Harness,
                Where::Product if inline_harness_from.is_some_and(|start| number > start) => {
                    Where::Harness
                }
                Where::Product => Where::Product,
            };
            sites.push(format!("{placed:?} {name}:{number}: {line}"));
        }
    }
    sites
}

/// ⛔ **THE GATE.** No product line takes a scratch root from the operating system directly.
///
/// The one place that may is `sprag-scratch`, which asks the question this gate exists to enforce
/// — `is_absolute` — and panics naming `TMPDIR` when the answer is no. That exemption is a single
/// crate, named in [`call_sites`], and its own tests drive both arms (`an_empty_root_is_refused`,
/// `a_relative_root_is_refused_by_the_same_question`).
#[test]
fn no_product_code_takes_a_scratch_root_unchecked() {
    let sites = call_sites();
    let unchecked: Vec<&str> = sites
        .iter()
        .filter(|(placed, _)| *placed == Where::Product)
        .map(|(_, site)| site.as_str())
        .collect();
    assert!(
        unchecked.is_empty(),
        "⛔ ITEM 794: product code took a scratch root from the operating system and trusted it. \
         `std::env::temp_dir()` answers a RELATIVE path when `TMPDIR` is set-and-empty, and \
         nothing downstream refuses one — `create_dir_all`, `File::create` and `git worktree add` \
         all succeed against the process's own working directory, silently. Call \
         `sprag_scratch::scratch_root()` instead: it asks `is_absolute` where the root is taken \
         and panics naming the variable when it cannot be used:\n{}",
        unchecked.join("\n"),
    );
}

/// ⛔⛔ **THE EXEMPTION IS COUNTED, BECAUSE AN EXEMPTION NOBODY MEASURES IS HOW A POPULATION GROWS
/// BACK** — rule 6.
///
/// The gate above reaches zero only because the harness half is out of its population. That half is
/// real: under `TMPDIR=` every one of these sites writes into whatever directory `cargo test` stood
/// its binary in, which is the crate's own directory inside this repository. Measured 2026-08-31 —
/// 131 untracked entries left under `crates/*/`, 414 tests failed, against 0 and 0 for the same
/// tree with a normal `TMPDIR`.
///
/// A move in either direction is a red: a new test calling `std::env::temp_dir()` is a new place
/// the suite litters, and a site converted away is item 795 being paid down. Both are worth one
/// line of somebody's attention, and neither is worth a number that quietly stops being true.
#[test]
fn the_harness_half_cannot_grow_without_being_read() {
    let harness = call_sites()
        .into_iter()
        .filter(|(placed, _)| *placed == Where::Harness)
        .count();
    assert_eq!(
        harness, HARNESS_SITES_REGISTERED,
        "⛔ ITEM 794's harness half moved: {harness} call sites take a scratch root from the \
         operating system in test, bench and example code, against {HARNESS_SITES_REGISTERED} \
         recorded when the product half reached zero. Each one writes into the crate's own \
         directory inside this repository when `TMPDIR` is set-and-empty. If it GREW, call \
         `sprag_scratch::scratch_root()` from the new site instead of the bare std call. If it \
         SHRANK, that is item 795 being paid down: set this constant to {harness} and say so in \
         the register. It is held exactly rather than as a ceiling so the number cannot rot into \
         slack that admits new sites in silence",
    );
}

/// ⛔⛔ **EVERY PER-RUN NAME COMES FROM THE SEAM THAT CAN COLLECT IT** — register items 795 and 930.
///
/// `sprag_scratch::scratch_for` mints the name AND sweeps the same prefix's dead owners, which is
/// the only arrangement in which the two can agree on the prefix. They have to: `owner_in` reads
/// the pid immediately after the prefix it is given, so `sprag-869-<pid>` swept as `sprag` answers
/// **869** — an item number read as a process id, and on the machine this was measured on, a live
/// service. A site that builds the name itself and sweeps somewhere else is one edit away from
/// deleting a running suite's scratch, and a site that builds the name and never sweeps at all is
/// what put 13,898 directories in this machine's scratch root.
///
/// ⚠ **A count became an emptiness on 2026-09-06**, when item 930 took the last four. The constant
/// this held against is gone; the reasoning is on the doc block where it stood. What a reader gets
/// now is the FILE AND LINE of a new site, which is what they would have had to go and find.
#[test]
fn every_per_run_name_is_minted_by_the_seam_that_can_collect_it() {
    let sites = per_run_name_sites();
    assert!(
        sites.is_empty(),
        "⛔ ITEM 795's second axis: {} site(s) build a per-run scratch name on top of \
         `scratch_root()`. Taking the ROOT from `sprag_scratch` says the root is usable; it says \
         nothing about who removes the directory when the run that made it is killed, and nothing \
         ever does — the name carries that run's pid, so its own `remove_dir_all` matches only \
         itself and its `Drop` never runs when it is killed. Call \
         `sprag_scratch::scratch_for(<prefix>, <tail>)` instead: it mints the name with the pid \
         where the reaper reads it and sweeps that prefix's dead owners in the same call. ⚠ If the \
         site cannot afford the sweep where it stands — item 930's one real case was a `OnceLock` \
         initialising inside a timed row — move the initialisation, do not skip the seam:\n{}",
        sites.len(),
        sites.join("\n"),
    );
}

/// ⛔⛔⛔ **AND THE DETECTOR ANSWERS BOTH WAYS** — register item 908's lesson, and item 924's: a
/// count of zero that nothing can raise is a green about nothing.
///
/// Driven from literals rather than from the tree, because the tree is meant to hold none of these
/// and a fixture that only ever sees the empty case cannot tell a working detector from a broken
/// one.
/// ⚠⚠ **EVERY FIXTURE HERE IS ASSEMBLED WITH `concat!`, ONE CLOSED STRING PER LINE**, and that is
/// not a style choice. [`outside_strings`] keeps a line that ENDS inside a string as code — the
/// deliberate behaviour this file's own header records — so a `\`-continued literal spelling
/// `scratch_root()` would be read as a call site and this gate would red on its own fixtures. The
/// header says the answer is to rephrase rather than widen the filter; this is that, applied to the
/// one place in the workspace that has to write the call down verbatim.
#[test]
fn the_per_run_detector_answers_both_ways() {
    let minted = concat!(
        "let dir = sprag_scratch::scratch_root()",
        ".join(format!(\"sprag-x-{}\", std::process::id()));",
    );
    assert_eq!(
        per_run_name_lines(minted),
        vec![1],
        "a per-run name on one line is the plain case",
    );

    let wrapped = concat!(
        "let dir = sprag_scratch::scratch_root()\n",
        "    .join(format!(\n",
        "        \"sprag-x-{}-{tag}\",\n",
        "        std::process::id(),\n",
        "    ));\n",
        "let other = 1;\n",
    );
    assert_eq!(
        per_run_name_lines(wrapped),
        vec![1],
        "the `.join` on the next line and the pid two below it belong to the same expression — a \
         line-at-a-time reader would call this clean, which is how five of this workspace's own \
         wrapped sites would ride through",
    );

    for clean in [
        concat!(
            "let p = sprag_scratch::scratch_root()",
            ".join(\"sprag-latency-manifests.toml\");",
        ),
        "let root = sprag_scratch::scratch_root();",
        concat!(
            "// sprag_scratch::scratch_root()",
            ".join(format!(\"x-{}\", std::process::id()));",
        ),
        "let told = \"scratch_root() and std::process::id() in a string\";",
        "let n = std::process::id();",
        concat!(
            "Self::resolve(\n",
            "    &[],\n",
            "    &sprag_scratch::scratch_root(),\n",
            "    HOST_SOCKET_NAME,\n",
            ")\n",
        ),
        concat!(
            "IsolatedCheckout::of(dir, &sprag_scratch::scratch_root())\n",
            "    .map(|cut| Box::new(cut))\n",
        ),
    ] {
        assert!(
            per_run_name_lines(clean).is_empty(),
            "a fixed name, a bare root, a comment, a string, a lone pid, an argument and a tail \
             expression are none of them this population — counting one would make the ratchet a \
             number about something else: {clean}",
        );
    }

    let unreadable = "let dir = sprag_scratch::scratch_root().join(format!(\n".to_string()
        + &"    a_line_that_never_closes,\n".repeat(EXPRESSION_LINES + 2);
    assert_eq!(
        per_run_name_lines(&unreadable),
        vec![1],
        "an expression the gate could not delimit is counted rather than waved through — rule 6",
    );
}

/// ⛔⛔ **AND NO SCRATCH PATH IS SPELLED WHERE THE MACHINE SHOULD HAVE BEEN ASKED** — register
/// item 931.
///
/// The two gates above hunt a CALL, and a path written down calls nothing. Four sites in this
/// workspace wrote one: three sockets and a state directory in the GUI's live smoke, and a
/// stand-in reader's file inside a shell script the plugin driver hands a pane. None of the four
/// appeared in either number, and the last of them had left 745 files standing in this machine's
/// scratch root by 2026-09-06 — a count nothing in the workspace could produce, because no name in
/// it knew the prefix. Converting it took them: the first run through the seam swept the prefix and
/// the same `find` answered **1**, and that one is kept because its pid reads as alive, which is
/// `may_reap`'s documented safe direction rather than a failure to collect.
///
/// ⚠ An emptiness rather than a count, for the reason item 930's block above gives: what a reader
/// gets is the FILE AND LINE of a new site instead of a number somebody has to update.
#[test]
fn no_scratch_path_is_spelled_where_the_machine_should_have_been_asked() {
    let sites = literal_root_sites();
    assert!(
        sites.is_empty(),
        "⛔ ITEM 931: {} line(s) spell a scratch root and mint a name under it in the same breath. \
         Neither ratchet above can see this — both read `outside_strings`, which blanks the very \
         literal the path is written in — so the site is counted nowhere, `TMPDIR` is not read (a \
         harness that isolated a run by naming its own root is undone in silence), and no seam \
         knows the prefix, so `reap_predecessors` can never be asked about what it leaves. Call \
         `sprag_scratch::scratch_for(<prefix>, <tail>)` and use the path it answers; where the \
         name is minted inside a SHELL script, let Rust take it from the seam and interpolate the \
         result, which is what the line above `agent.rs`'s stand-in already does for its arming \
         file:\n{}",
        sites.len(),
        sites.join("\n"),
    );
}

/// ⛔⛔⛔ **AND THAT DETECTOR ANSWERS BOTH WAYS** — register item 908's lesson: an emptiness nothing
/// can raise is a green about nothing.
///
/// ⚠⚠ **EVERY FIXTURE IS ASSEMBLED AT RUN TIME FROM THE TWO ROOT CONSTANTS BELOW**, and that is
/// not a style choice — it is the mirror image of the `concat!` rule on the detector above. This
/// axis reads the raw code line INCLUDING its string contents, so a fixture that spelled a root and
/// mint together would be a site of the very population this file asserts is empty, and the gate
/// would red on its own test data. A root with nothing minted under it is not in the population, so
/// the two constants are safe to write down and the mint is added by `format!`.
#[test]
fn the_literal_root_detector_answers_both_ways() {
    /// A spelled root, alone: the shape all 79 of this workspace's spelled roots have today, and
    /// not a site.
    const ROOT: &str = "/tmp";
    /// The other root `TMPDIR` can answer, which two of this workspace's tests use on purpose so
    /// an arm says which root it would have used.
    const VAR_ROOT: &str = "/var/tmp";

    for (why, minted) in [
        (
            "an interpolation in the spelled path is the plain case",
            format!("let s = PathBuf::from(format!(\"{ROOT}/sp{{u}}h.sock\"));"),
        ),
        (
            "a shell's own pid, in a script a literal hands a pane",
            format!("\"S={ROOT}/sprag-standin-$$; \\"),
        ),
        (
            "an `mktemp` template under a spelled root",
            format!("Command::new(\"mktemp\").arg(\"{VAR_ROOT}/sprag-x.XXXXXX\");"),
        ),
        (
            "a pid spelled beside the root rather than into it",
            format!("let s = PathBuf::from(\"{ROOT}\").join(name_for(std::process::id()));"),
        ),
    ] {
        assert_eq!(literal_root_lines(&minted), vec![1], "{why}: {minted}");
    }

    for (why, clean) in [
        (
            "a fixed value is what all 79 spelled roots are today",
            format!("assert_eq!(shell_quote(\"{ROOT}/my report.pdf\"), quoted);"),
        ),
        (
            "a bare root, handed on rather than named under",
            format!("let root = PathBuf::from(\"{ROOT}\");"),
        ),
        (
            "an address nothing ever binds — `Host::for_daemon`'s gate says so in its own comment",
            format!("let sock = Path::new(\"{ROOT}/sprag-904-gate.sock\");"),
        ),
        (
            "the root a test hands every arm so the arm says which one it used",
            format!("const SCRATCH: &str = \"{VAR_ROOT}\";"),
        ),
        (
            "a shell-injection sample, whose substitution is the SUBJECT of the assertion",
            format!("assert_eq!(shell_quote(\"{ROOT}/$(reboot)\"), quoted);"),
        ),
        (
            "⛔ `scratch-litter`'s usage message, braces doubled so `format!` prints them — read as \
             a mint, this gate would red on the one instrument whose subject is scratch litter",
            format!(
                "\"On this machine that is `{ROOT}`, or whatever `TMPDIR` names:\\n    \
                 scratch-litter \\\"${{{{TMPDIR:-{ROOT}}}}}\\\"\"",
            ),
        ),
        (
            "a comment, which is where this repository's reasoning quotes the shape it hunts",
            format!("// let s = PathBuf::from(format!(\"{ROOT}/sp{{u}}h.sock\"));"),
        ),
    ] {
        assert!(
            literal_root_lines(&clean).is_empty(),
            "{why} — counting one would make this axis a number about something else, which is \
             item 931's own warning: {clean}",
        );
    }

    assert_eq!(
        literal_root_at(&format!("x = \"{VAR_ROOT}/y\";")),
        Some(5),
        "the longer root is found where it starts, not four characters in — a `/var/tmp` read as a \
         shorter root would report a path that is not the one written down",
    );
}

/// ⚠⚠⚠ **THE MACHINERY REACHES THE CODE, AND IT ANSWERS BOTH WAYS.**
///
/// The gate above passes by finding nothing. So would a walk that reached no files, a `code_lines`
/// that filtered everything away, an `outside_strings` that blanked whole lines, or a classifier
/// that called everything [`Where::Harness`] — and a version of each has happened to a gate in this
/// repository. This test fails if any of them stops working, which is the one failure a green
/// cannot tell apart from success.
#[test]
fn the_walk_reaches_this_workspace_and_the_classifier_answers_both_ways() {
    let files = rust_files();
    assert!(
        files.len() > 100,
        "the walk found only {} Rust files under crates/ — the gate beside this one would pass on \
         an empty population",
        files.len(),
    );

    let product = files
        .iter()
        .filter(|(name, _)| where_it_lives(name) == Where::Product)
        .count();
    let harness = files.len() - product;
    assert!(
        product > 0 && harness > 0,
        "the classifier put all {} files on one side (product {product}, harness {harness}) — a \
         classifier with one answer exempts everything or exempts nothing, and either way it is \
         not reading the layout",
        files.len(),
    );

    let seam = files
        .iter()
        .find(|(name, _)| name == "crates/sprag-scratch/src/lib.rs")
        .expect("the scratch seam is a Rust file under crates/ and the walk must reach it");
    assert!(
        code_lines(&seam.1).any(|(_, line)| outside_strings(line).contains("env::temp_dir()")),
        "the filters no longer see the one call this workspace is allowed to make — whatever they \
         are dropping now, they would drop a violation the same way",
    );
    assert!(
        !outside_strings("    if !code.contains(\"env::temp_dir()\") {")
            .contains("env::temp_dir()"),
        "`outside_strings` stopped blanking string contents, so this gate reds on its own filter \
         and on every message that quotes the call while explaining it",
    );
}

/// Every workspace site that BINDS a unix socket, as `(file, line, code)`.
///
/// ⚠ Discovered by walking the same Rust the gates above walk — a hand list of bind sites is the
/// place that leaks (register items 80, 762, 945), and this population moved twice while item 955
/// was being measured.
fn bind_sites() -> Vec<(String, usize, String)> {
    let mut found = Vec::new();
    for (name, text) in rust_files() {
        for (line, code) in code_lines(&text) {
            let bare = outside_strings(code);
            if bare.contains("UnixListener::bind(") || bare.contains("UnixDatagram::bind(") {
                found.push((name.clone(), line, code.trim().to_owned()));
            }
        }
    }
    found
}

/// Every workspace site that MINTS a unix socket path, as `(file, line, code)`.
///
/// A line is one when it BUILDS a path (`join`, `format!`, `scratch_for`) out of a string literal
/// ending in `.sock`. Walked rather than listed, for [`bind_sites`]' reason exactly.
///
/// ⚠⚠ **THE LITERAL IS THE HANDLE AND IT IS ALSO THE LIMIT.** A factory that takes the file name as
/// a PARAMETER — `fn sock(dir, name)` in `sprag-rpc`'s survey, `resolve_socket_path`'s
/// `socket_name` — has no `.sock` on its own line and is invisible here. That is stated rather than
/// implied: this scan finds the places a name is SPELLED, which is where item 959's measured
/// silences were, and a factory that took the name from elsewhere would walk past it.
fn socket_mints() -> Vec<(String, usize, String)> {
    let mut found = Vec::new();
    for (name, text) in rust_files() {
        for (line, code) in code_lines(&text) {
            let spells_a_socket = code
                .split('"')
                .skip(1)
                .step_by(2)
                .any(|literal| literal.ends_with(".sock"));
            let builds_a_path =
                code.contains("join(") || code.contains("format!") || code.contains("scratch_for(");
            if spells_a_socket && builds_a_path {
                found.push((name.clone(), line, code.trim().to_owned()));
            }
        }
    }
    found
}

/// 🎯🎯🎯🎯🎯 **A FILE THAT MINTS A UNIX SOCKET PATH ASKS WHETHER IT CAN HOLD ONE** — register item
/// 959, and the half of item 955's adoption that the word `bind` hid.
///
/// # ⛔⛔⛔⛔⛔ The check was right and the NAME was read as its scope
///
/// `sun_path` bounds the socket ADDRESS: `bind` and `connect` fill the same `sockaddr_un`. The door
/// item 955 built was called `may_bind`, so the gate beside this one asks only of files that BIND —
/// and *"nothing here listens, so nothing here is at risk"* became a thing a reader could believe.
///
/// **Measured 2026-09-08, while paying item 958.** `sprag-rpc`'s survey minted a path EIGHT bytes
/// over the budget and stood green, because that case only ever connected. Asking the same question
/// of every MINTING file rather than every binding one moved the population from 22 bind sites to
/// **14 files and 28 lines**, of which **7 files were silent** — every one of them a place that
/// hands its name to a process it spawns and then connects to.
///
/// # ⚠⚠⚠ Why MINTING and not connecting, which is the other shape this could have taken
///
/// A connect site usually receives a path it did not make — `sprag.rs` connects to whatever
/// `HostEndpoint` resolved — so demanding the call there would demand it on lines that legitimately
/// cannot have it, which is the objection [`every_file_that_binds_a_socket_asks_whether_the_path_can_hold_one`]
/// already states against a per-bind rule. **The place that can answer is the place that MADE the
/// name**, and the checking constructor hands the path back, so the asking cannot be decorative.
///
/// ⚠⚠ THE COUNT IS ASSERTED, for that gate's reason: a walk that stopped finding mints would be
/// green for the wrong reason.
///
/// ⚠ There is NO exemption arm, deliberately — all seven were brought in before this was written,
/// so the honest number today is zero and an array here would be the escape hatch working rule 6
/// refuses. A site that genuinely cannot ask belongs in the ledger.
#[test]
fn every_file_that_mints_a_socket_path_asks_whether_it_can_hold_one() {
    let sites = socket_mints();
    assert!(
        sites.len() >= 24,
        "⚠⚠⚠ THE POPULATION COLLAPSED: this walk found {} minting line(s), and measured \
         2026-09-08 this workspace has 28 across 14 files. A scan of nothing is green for the \
         wrong reason: {sites:#?}",
        sites.len(),
    );

    // ⚠ Spelled once so the search and the message cannot drift, and BOTH doors count: `may_bind`
    // is a delegate to `may_address` and a bind site reads better for keeping its own word.
    let doors = ["may_address(", "may_bind("];
    let asking: std::collections::BTreeSet<String> = rust_files()
        .into_iter()
        .filter(|(_, text)| {
            code_lines(text).any(|(_, line)| doors.iter().any(|door| line.contains(door)))
        })
        .map(|(name, _)| name)
        .collect();
    let silent: Vec<String> = sites
        .iter()
        .filter(|(name, _, _)| !asking.contains(name))
        .map(|(name, line, code)| format!("  {name}:{line}  {code}"))
        .collect();
    assert!(
        silent.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 959: a file MAKES a unix socket path and never asks whether it \
         can hold one on the platform with the tightest `sun_path`. macOS gives {} bytes and a \
         {}-byte scratch root against Linux's 4 — and this bites whether or not anything here \
         BINDS, because `bind` and `connect` fill the same `sockaddr_un`. Take the path back \
         through `may_address(` in `sprag_scratch`, at the factory that MAKES it:\n{}",
        sprag_scratch::TIGHTEST_SUN_PATH,
        sprag_scratch::LONGEST_SCRATCH_ROOT,
        silent.join("\n"),
    );
}

/// ⛔⛔⛔⛔⛔ **A FILE THAT BINDS A UNIX SOCKET ASKS WHETHER THE PATH CAN HOLD ONE** — register item
/// 955, and the adoption item 950's budget did not have.
///
/// # ⛔⛔⛔⛔ What was measured, and why a budget alone was not enough
///
/// Item 950 gave this workspace `sprag_scratch::socket_fits` after the macOS runner refused
/// `a_live_daemons_residue_is_never_removable_however_empty_its_files_are` with *"path must be
/// shorter than SUN_LEN"* — 104 bytes there against 108 on Linux, under a scratch root of 48 bytes
/// against Linux's 4. **Measured the next day: 21 bind sites, and exactly ONE of them asked.** A
/// budget nothing consults is *somebody's memory* wearing a function's name, which is the shape
/// register items 738 and 853 refuse.
///
/// # ⚠⚠⚠ Why the claim is per FILE and not per bind, said plainly
///
/// The paths come from about six FACTORIES — two `socket_path()`s, one `sock_path(tag)` feeding ten
/// binds, a few `dir.join(…)` — and the right place for the check is where the path is MADE, so a
/// caller cannot forget it. A per-bind rule would demand the call on lines that legitimately do not
/// have it. What every binding file can be held to is that it asks SOMEWHERE, and the checking
/// constructor hands the path back, so the asking cannot be decorative.
///
/// ⚠⚠ THE COUNT IS ASSERTED, because a walk that stopped finding binds would satisfy this claim
/// vacuously — register item 924's shape, and the reason every population in this file is printed.
///
/// ⚠ There is NO exemption arm. This gate is written after all twenty-one were brought in, so the
/// honest number today is zero; an exemption array here would be the escape hatch this workspace's
/// rule 6 refuses, and a site that genuinely cannot ask belongs in the ledger instead.
#[test]
fn every_file_that_binds_a_socket_asks_whether_the_path_can_hold_one() {
    let sites = bind_sites();
    assert!(
        sites.len() >= 20,
        "⚠⚠⚠ THE POPULATION COLLAPSED: this walk found {} bind site(s), and measured 2026-09-08 \
         this workspace has 21. A scan of nothing is green for the wrong reason: {sites:#?}",
        sites.len(),
    );

    // ⚠ Spelled once, so the message below and the search cannot drift apart.
    let door = "may_bind(";
    let asking: std::collections::BTreeSet<String> = rust_files()
        .into_iter()
        .filter(|(_, text)| code_lines(text).any(|(_, line)| line.contains(door)))
        .map(|(name, _)| name)
        .collect();
    let silent: Vec<String> = sites
        .iter()
        .filter(|(name, _, _)| !asking.contains(name))
        .map(|(name, line, code)| format!("  {name}:{line}  {code}"))
        .collect();
    assert!(
        silent.is_empty(),
        "⛔⛔⛔⛔⛔ REGISTER ITEM 955: a file binds a unix socket and never asks whether the path \
         can hold one on the platform with the tightest `sun_path`. macOS gives {} bytes and a \
         {}-byte scratch root against Linux's 4, so a path that binds here is refused there and the \
         test then reports something else entirely. Take the path back through `{door}` in \
         `sprag_scratch`, at the factory that MAKES it:\n{}",
        sprag_scratch::TIGHTEST_SUN_PATH,
        sprag_scratch::LONGEST_SCRATCH_ROOT,
        silent.join("\n"),
    );
}
