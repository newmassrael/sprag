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
