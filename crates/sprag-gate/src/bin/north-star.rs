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
    // ⚠⚠ THE CAP IS THE DOCUMENT'S, NOT THIS BINARY'S — register item 833(1) and 773's axis ("the
    // subject is the launcher's, the policy is the document's"). See [`cap`], which is where that
    // sentence stopped being a comment.
    let cap = match cap() {
        Ok(cap) => cap,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let deferred = reading.deferred(cap.depth());
    let held: Vec<String> = deferred.iter().map(ToString::to_string).collect();
    println!(
        "deferred {} at depth > {}: {}",
        deferred.len(),
        cap.spelled(),
        held.join(" "),
    );
    println!(
        "unrooted {} (declared {})",
        reading.unrooted().len(),
        reading
            .parent_declared
            .map_or_else(|| "none".to_string(), |n| n.to_string()),
    );
    // ⛔⛔⛔⛔⛔ AND THE PAID MARKS WHOSE CLAIM NOTHING CHECKED — register item 902, printed
    // beside its three ratchet neighbours because it is the same kind of statement: a backlog with
    // a floor that may only fall. See `north_star::PAID_DECLARATION`.
    let unnamed = reading.paid_unnamed();
    println!(
        "paid-uncommitted {} (declared {})",
        unnamed.len(),
        reading
            .paid_declared
            .map_or_else(|| "none".to_string(), |n| n.to_string()),
    );
    println!("items {} in section A", reading.items.len());

    // ⛔⛔⛔⛔⛔ AND THE OTHER HALF, WHICH NEEDS THE REPOSITORY AND NOT THE DOCUMENT — register
    // item 902. An id that is WRITTEN and does not resolve has no backlog and no floor: it is a
    // typo or a claim about a commit nobody made, so any one of them is a RED on the spot.
    //
    // ⚠⚠ A FAILURE TO ASK IS ITS OWN FAULT and never forty item faults — see
    // `north_star::Reading::paid_unresolved`. Being unable to reach `git` says nothing about any
    // ledger line, and a RED that fires on a broken environment is one readers learn to skip.
    let unresolved = match reading.paid_unresolved(&Repository) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    for (number, id) in &unresolved {
        eprintln!("north-star: item {number} names commit {id}, which this tree cannot resolve");
    }

    if reading.is_green() && unresolved.is_empty() {
        return std::process::ExitCode::SUCCESS;
    }
    if !reading.faults.is_empty() {
        eprintln!("\n{} fault(s):", reading.faults.len());
        for fault in &reading.faults {
            eprintln!("  {fault}");
        }
    }
    std::process::ExitCode::FAILURE
}

/// ⛔⛔⛔⛔⛔ **THE REPOSITORY THIS BINARY IS RUN INSIDE, AS THE ANSWER TO *DOES THIS COMMIT
/// EXIST*** — register item 902, and the one place in this program that knows where the tree is.
///
/// ⚠⚠⚠ **`^{commit}` IS NOT DECORATION.** An id resolves for a blob or a tree too, and a ledger
/// line naming a forty-hex blob would have been accepted as a commit. The suffix makes the question
/// the one the ledger is actually asking.
///
/// # ⛔⛔⛔⛔⛔ Why `rev-parse --verify --quiet` and NOT `cat-file -e`, measured by mutation
///
/// The first build of this asked `git cat-file -e <id>^{commit}` and read status 1 as *absent* and
/// anything else as *could not ask*. Driving it against a ledger line carrying a plausible but
/// fictional id produced:
///
/// > `north-star: git could not be asked whether deadbee is a commit (exit status: 128): fatal:
/// > Not a valid object name deadbee^{commit}`
///
/// **`cat-file` answers an unresolvable NAME with 128, the same code it uses for *this is not a
/// repository*.** So the one case this gate exists to catch was reported as a broken environment.
/// It was still a RED — but a red that names the wrong cause is the thing register item 901 was
/// opened by: a reader who sees two misattributed refusals ignores the third, real one.
///
/// ⇒ `rev-parse --verify --quiet` splits them the way this code always claimed to: **1 for a name
/// that does not resolve, 128 for a question that could not be put.**
///
/// ⚠⚠ AND A FAILURE TO RUN `git` AT ALL IS STILL AN ERROR, never `false`: see
/// [`north_star::Commits::resolves`] for why that difference is the whole point.
struct Repository;

impl north_star::Commits for Repository {
    fn resolves(&self, id: &str) -> Result<bool, String> {
        let asked = std::process::Command::new("git")
            .args([
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{id}^{{commit}}"),
            ])
            .output()
            .map_err(|why| format!("cannot ask git whether {id} is a commit: {why}"))?;
        match asked.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(format!(
                "git could not be asked whether {id} is a commit ({}): {}",
                asked.status,
                String::from_utf8_lossy(&asked.stderr).trim(),
            )),
        }
    }
}

/// ⛔⛔⛔⛔⛔ **THE DOCUMENT THIS REPOSITORY'S DEBT RUNS ARE DRIVEN BY**, read at build time.
///
/// # ⚠⚠⚠ Why `debt_loop.scxml` and not the template beside it
///
/// `ai_loop.scxml` is the TEMPLATE other repositories copy; `debt_loop.scxml` is the kind this
/// repository's own loop runs, and its `reaim_max` is the one a round here is actually held to.
/// The two happen to ship the same number today, and reading the template would be reading a
/// document no run of this ledger is driven by — a second author with a coincidence for a gate.
///
/// ⚠⚠ `include_str!` and not a path read at run time, which is a decision rather than a saving:
/// `sprag_gate::sources::workspace_root` PANICS when the tree it was compiled in is not the tree it
/// is running in, and this binary is handed a ledger that lives outside the repository entirely.
/// Baked in, the number travels with the binary — and a binary older than its document is the
/// same staleness every other gate in this crate already has, said the same way.
///
/// ⚠ It is a text file, so this costs the crate none of its charter: `north-star` still builds
/// when the product does not.
const DRIVING_DOCUMENT: &str = include_str!("../../../sprag-plugin/src/debt_loop.scxml");

/// ⛔⛔⛔⛔⛔ **THE RE-AIM CAP THIS BINARY JUDGES UNDER — THE DOCUMENT'S, OR NOTHING** — register
/// item 833(1).
///
/// # ⛔⛔⛔⛔⛔ It was `.unwrap_or(1)`, under a comment saying it was the document's
///
/// Measured 2026-09-04: `debt_loop.scxml`'s `reaim_max` was set to `2` and this binary rebuilt. It
/// went on printing `deferred 10 at depth > 1`, and `--admits` refused item 843 with *"sits deeper
/// than 1 in the chain that found it"*. **Five critical items stayed held back by a number the
/// document no longer declared**, and nothing anywhere said the two disagreed — which is register
/// item 445's two-authors defect sitting inside the instrument item 833 exists to be.
///
/// ⚠⚠ **AND THE `SPRAG_REAIM_MAX` OVERRIDE IS GONE WITH IT.** Its whole reason was that the
/// document was not read — *"a round that changes the document's number and wants this line to
/// agree passes it"*. Kept beside a reader that DOES read the document it would be an escape hatch
/// that can silently disagree with the policy's author, which is this workspace's rule 6 and the
/// exact thing being removed. A round that wants a different cap changes the document.
///
/// # Errors
///
/// The document's own sentence, when it declares no cap, declares it twice, or declares something
/// this reader can make neither a number nor `never` of — see [`north_star::declared_reaim`]. Rule
/// 6: not a `1`.
fn cap() -> Result<north_star::Reaim, String> {
    north_star::declared_reaim(DRIVING_DOCUMENT)
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
/// ⚠⚠ **THE CAP IS THE DOCUMENT'S AND THIS ONLY MIRRORS IT** — [`cap`], which reads it out of the
/// document rather than holding a number of its own. That sentence stood here while the code two
/// screens down said `.unwrap_or(1)`; register item 833(1) and [`cap`]'s own doc carry what the
/// disagreement cost.
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
    // ⚠⚠ THE SAME ONE READING, and the failure is a REFUSAL rather than a verdict: a checker that
    // cannot say what cap it is judging under has said nothing, and `stderr` is where this binary
    // puts *the instrument could not judge* so a reader is sent to the instrument.
    let cap = match cap() {
        Ok(cap) => cap,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let admitted = reading.admits(cap.depth());
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
    let why = if reading.deferred(cap.depth()).contains(&number) {
        format!(
            "item {number} sits deeper than {} in the chain that found it, so it is \
                 registered rather than worked",
            cap.spelled(),
        )
    } else if reading.population().contains(&number) {
        format!("item {number} is open but not in the set to take next")
    } else {
        format!("item {number} is not in this register's open population")
    };
    println!("NO — {why}. What a round may take: {}", spelled.join(" "));
    std::process::ExitCode::SUCCESS
}
