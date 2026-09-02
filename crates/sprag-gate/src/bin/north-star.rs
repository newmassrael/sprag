//! ⚠⚠⚠⚠ **WHAT THE NORTH STAR IS COUNTING, AS ONE COMMAND** — register item 823.
//!
//! ```text
//! cargo run -q -p sprag-gate --bin north-star -- ~/.claude/projects/-home-coin-sprag/memory/debt-open.md
//! ```
//!
//! Exits 0 with the population on one line and the backlog on another; exits 1 having named every
//! fault. The index keeps no list of its own — it prints this one.
//!
//! Why a mark rather than a word list, and why an unmarked item is a debt instead of a "no", are in
//! [`sprag_gate::north_star`]'s own docs.
//!
//! # 🎯🎯🎯🎯🎯 `--admits <ledger> <proposal>` — register item 839
//!
//! ```text
//! north-star --admits <debt-open.md> "항목 839 를 갚아라 — …"
//! ```
//!
//! Answers `YES` or `NO` as its first word, then one sentence. It is the shape a loop's
//! `successor_check` is read by — a verdict is a WORD, and everything about how that reply is found
//! is `sprag_plugin::judge`'s, not this binary's.
//!
//! ⚠⚠ **THIS BINARY DECIDES WHAT THIS REPOSITORY ADMITS, AND NOTHING ABOUT ANY OTHER.** The machine
//! that counts a refused proposal and declines to take it is in `ai_loop.scxml`, which other
//! repositories copy; the MEANING is here, where this ledger's marks are. That split is the whole
//! of item 839.

use sprag_gate::north_star;

fn main() -> std::process::ExitCode {
    let mut args = std::env::args_os().skip(1);
    let Some(path) = args.next() else {
        eprintln!(
            "north-star: needs the ledger's path.\n  \
             cargo run -q -p sprag-gate --bin north-star -- <debt-open.md>",
        );
        return std::process::ExitCode::FAILURE;
    };
    if path == *"--admits" {
        return admits(args);
    }
    if args.next().is_some() {
        eprintln!("north-star: one ledger, not several");
        return std::process::ExitCode::FAILURE;
    }

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "north-star: cannot read {}: {error}",
                path.to_string_lossy()
            );
            return std::process::ExitCode::FAILURE;
        }
    };

    let reading = north_star::read(&text);

    // ⚠⚠ A ledger with no section A reads as EMPTY, and an empty reading must not be a pass — a
    // probe pointed at nothing reading clean is the defect this crate's first gate shipped with.
    if reading.items.is_empty() {
        eprintln!(
            "north-star: {} has no section A items — a probe pointed at nothing is not a clean run",
            path.to_string_lossy(),
        );
        return std::process::ExitCode::FAILURE;
    }

    let population = reading.population();
    let spelled: Vec<String> = population.iter().map(ToString::to_string).collect();
    println!("population {}: {}", population.len(), spelled.join(" "));
    println!(
        "unclassified {} (declared {})",
        reading.unclassified().len(),
        reading
            .declared
            .map_or_else(|| "none".to_string(), |n| n.to_string()),
    );
    // ⚠⚠ PRINTED ABOVE THE TOTAL, because this is the line a round acts on — register item 833(1).
    // The population says what is owed; this says what to take first.
    let critical = reading.critical();
    let ranked: Vec<String> = critical.iter().map(ToString::to_string).collect();
    println!("critical {}: {}", critical.len(), ranked.join(" "));
    println!(
        "unranked {} (declared {})",
        reading.severity_unclassified().len(),
        reading
            .severity_declared
            .map_or_else(|| "none".to_string(), |n| n.to_string()),
    );
    // ⚠⚠ THE CAP IS THE DOCUMENT'S, NOT THIS BINARY'S — register item 833(2) and 773's axis ("the
    // subject is the launcher's, the policy is the document's"). This prints what the shipped
    // default holds so a reader can see the chain; a round that changes the document's number and
    // wants this line to agree passes it.
    let cap: u32 = std::env::var("SPRAG_REAIM_MAX")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let deferred = reading.deferred(cap);
    let held: Vec<String> = deferred.iter().map(ToString::to_string).collect();
    println!(
        "deferred {} at depth > {cap}: {}",
        deferred.len(),
        held.join(" "),
    );
    println!(
        "unrooted {} (declared {})",
        reading.unrooted().len(),
        reading
            .parent_declared
            .map_or_else(|| "none".to_string(), |n| n.to_string()),
    );
    println!("items {} in section A", reading.items.len());

    if reading.is_green() {
        return std::process::ExitCode::SUCCESS;
    }
    eprintln!("\n{} fault(s):", reading.faults.len());
    for fault in &reading.faults {
        eprintln!("  {fault}");
    }
    std::process::ExitCode::FAILURE
}

/// 🎯🎯🎯🎯🎯 **IS THIS PROPOSAL ONE A ROUND MAY TAKE NEXT?** — register item 839, and the half of
/// register item 833(1) that had been written as prose.
///
/// # ⛔⛔⛔⛔⛔ Why a `NO` and a *"cannot tell"* are the same answer here
///
/// Working rule 6 in one place: an unclassified thing is not a pass. A proposal this cannot place
/// in the ledger — it names no item, the ledger will not read — is one nothing has said is
/// admissible, and admitting it would make every failure of this instrument read as a green.
///
/// ⚠ The COST of that direction is real and is stated rather than hidden: a broken ledger path
/// stops a loop re-aiming at all. It is visible where it happens — the run counts every proposal it
/// set aside, and the sentence below travels with the verdict — which is exactly what a silent
/// admission would not be.
///
/// ⚠⚠ **THE CAP IS THE DOCUMENT'S AND THIS ONLY MIRRORS IT**, as the report above already does:
/// `SPRAG_REAIM_MAX` names the same number `ai_loop.scxml` holds, and a round that moves the
/// document's value and wants this to agree passes it.
fn admits(mut args: impl Iterator<Item = std::ffi::OsString>) -> std::process::ExitCode {
    // ⚠ NOT `println!` on the failure paths: this reply is read as a VERDICT, and a first word that
    // is not YES or NO is *the checker said nothing this run could read* — the honest answer for an
    // instrument that could not judge, and the one that sends a reader to the instrument.
    let Some(path) = args.next() else {
        eprintln!(
            "north-star: --admits needs the ledger's path, then the checkpoint in hand, \
                   then the proposal"
        );
        return std::process::ExitCode::FAILURE;
    };
    // 🎯 THE CHECKPOINT THE RUN IS ON, APPENDED BY THE DRIVER AHEAD OF THE PROPOSAL — register
    // item 840. Without it this can say whether a proposal is admissible and NOT whether taking it
    // goes deeper or sideways, which are opposite movements the budget was pricing the same.
    let Some(holding) = args.next() else {
        eprintln!("north-star: --admits needs the checkpoint in hand before the proposal");
        return std::process::ExitCode::FAILURE;
    };
    let Some(proposal) = args.next() else {
        eprintln!("north-star: --admits needs the proposal after the checkpoint in hand");
        return std::process::ExitCode::FAILURE;
    };
    if args.next().is_some() {
        eprintln!("north-star: --admits takes one ledger, one checkpoint and one proposal");
        return std::process::ExitCode::FAILURE;
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "north-star: cannot read {}: {error}",
                path.to_string_lossy()
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let reading = north_star::read(&text);
    if reading.items.is_empty() {
        eprintln!(
            "north-star: {} has no section A items — a ledger nothing was read out of cannot \
             admit or refuse anything",
            path.to_string_lossy(),
        );
        return std::process::ExitCode::FAILURE;
    }
    let cap: u32 = std::env::var("SPRAG_REAIM_MAX")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1);
    let admitted = reading.admits(cap);
    let spelled: Vec<String> = admitted.iter().map(ToString::to_string).collect();
    let proposal = proposal.to_string_lossy();
    let Some(number) = reading.names(&proposal) else {
        println!(
            "NO — this proposal names no item of the register, so nothing here can say it is one \
             to take now. What a round may take: {}",
            spelled.join(" "),
        );
        return std::process::ExitCode::SUCCESS;
    };
    if admitted.contains(&number) {
        // 🎯🎯🎯🎯🎯 AND WHETHER TAKING IT GOES DEEPER OR SIDEWAYS — register item 840, carried as
        // a SECOND MARKED WORD on the same reply the verdict rides. `FRESH` is an unrelated root:
        // the chain it is on has length zero, so adopting it is progress and must not spend a
        // budget meant for the debts this work itself creates. `STEP` is everything else.
        //
        // ⛔⛔ AND *cannot tell* IS SPELLED AS `STEP`, which is working rule 6 rather than a
        // guess: a chain that runs into an item stating no parentage is UNCLASSIFIED, and an
        // unclassified proposal must not be the cheap one. What unlocks the cheaper answer is the
        // `@from:` annotation the unrooted ratchet already asks for — so this instrument pays that
        // ratchet down by making it worth something.
        //
        // ⚠ A proposal whose CHECKPOINT this cannot place is `STEP` for the same reason: nothing
        // was compared, so nothing may be called unrelated.
        let sideways = reading.sideways(reading.names(&holding.to_string_lossy()), number);
        let (chain, how) = match sideways {
            true => ("FRESH", "and nothing this run is paying created it"),
            false => (
                "STEP",
                "and it is a step off the work in hand, or nothing here can say it is not",
            ),
        };
        println!(
            "YES {chain} — item {number} is in what this register says to take next ({} item(s)), \
             {how}.",
            admitted.len(),
        );
        return std::process::ExitCode::SUCCESS;
    }
    // ⚠⚠ THE REASON NAMES WHICH RULE REFUSED IT, because the two remedies differ: an item the
    // severity gate holds back waits for the critical set to empty, and one the depth cap holds
    // back waits for the cap to lift. A reader told only *"not in the set"* cannot act.
    let why = if reading.deferred(cap).contains(&number) {
        format!(
            "item {number} sits deeper than {cap} in the chain that found it, so it is \
                 registered rather than worked"
        )
    } else if reading.population().contains(&number) {
        format!("item {number} is open but not in the set to take next")
    } else {
        format!("item {number} is not in this register's open population")
    };
    println!("NO — {why}. What a round may take: {}", spelled.join(" "));
    std::process::ExitCode::SUCCESS
}
