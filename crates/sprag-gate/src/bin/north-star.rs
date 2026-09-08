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

    // 🎯🎯🎯🎯🎯 THE FOUR RATCHETED BACKLOGS, NAMING THEIR ITEMS — register item 934. This binary
    // printed `.len()` over four `Vec`s it then dropped, so the largest population in the ledger
    // could be seen and not asked about. The LINE is `north_star::Backlog`'s and not this file's,
    // which is what keeps *the set that reds* and *the set that prints* one object; the gate in
    // `north_star`'s tests refuses a build where this file formats one of these lines itself.
    let backlogs = reading.backlogs();

    let population = reading.population();
    let spelled: Vec<String> = population.iter().map(ToString::to_string).collect();
    println!("population {}: {}", population.len(), spelled.join(" "));
    println!("{}", backlogs.unclassified);
    // ⚠⚠ PRINTED ABOVE THE TOTAL, because this is the line a round acts on — register item 833(1).
    // The population says what is owed; this says what to take first.
    let critical = reading.critical();
    let ranked: Vec<String> = critical.iter().map(ToString::to_string).collect();
    println!("critical {}: {}", critical.len(), ranked.join(" "));
    println!("{}", backlogs.unranked);
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
    // 🎯🎯🎯🎯🎯 AND THE CHAIN THAT DEFERS EACH ONE, LINK BY LINK — register item 920. The line
    // above was a set of bare numbers, and the depth behind each was an integer with no way back
    // to the ledger lines that produced it: auditing a deferral meant walking the ledger by hand,
    // which is why nothing had ever audited one while both `@sev: critical` items sat behind it.
    //
    // ⚠ Printed under the line it explains and never instead of it: the set is what a round acts
    // on, and this is the evidence for it.
    for number in &deferred {
        match reading.chain(*number) {
            Some(links) => {
                let spelled: Vec<String> = links.iter().map(ToString::to_string).collect();
                println!("  {number} held by: {}", spelled.join(", "));
            }
            // Unreachable while `deferred` reports it — a chain that cannot be walked has no depth
            // and is takeable — but stated rather than unwrapped, because the two are read from the
            // same walk and a build that let them disagree should say so instead of panicking.
            None => println!("  {number} held by: a chain that cannot be walked"),
        }
    }
    // ⛔⛔⛔⛔⛔ AND WHAT THE CAP WOULD HAVE HELD AND NO LONGER DOES — register item 921. An empty
    // `deferred` line has two completely different causes and a reader has to be able to tell
    // *nothing sits deep* from *everything deep sits under closed parents*: the second is a claim
    // about specific items and the specific ancestors that released them. Register item 914's
    // finding, one instrument over — a green gate has to say which population it is green for.
    let released = reading.released(cap.depth());
    if !released.is_empty() {
        let freed: Vec<String> = released.iter().map(ToString::to_string).collect();
        println!(
            "released {} the cap no longer holds: {}",
            released.len(),
            freed.join(" "),
        );
        for number in &released {
            if let Some(links) = reading.chain(*number) {
                let spelled: Vec<String> = links.iter().map(ToString::to_string).collect();
                println!("  {number} was held by: {}", spelled.join(", "));
            }
        }
    }
    // ⛔⛔⛔⛔⛔ AND EVERY GATE THAT CAN RED, EACH SAYING HOW MANY QUESTIONS IT PUT — register
    // items 920, 902, 937 for the three of them, 924 for the count and 940 for the enumeration.
    //
    // ⚠⚠ PRINTED EVEN WHEN THEY FOUND NOTHING. A gate whose population emptied and a gate that
    // examined its population and found it clean report the same empty set, and item 924 measured
    // exactly that happening to the first of these after item 921 paid its deferrals to none.
    //
    // ⚠⚠⚠ ASKED IN ONE CALL AND JUDGED FROM ONE VALUE, which is the whole of item 940: the
    // verdict below reads `screenings` and NOTHING else about these three, so a fourth gate that
    // is not in `north_star::Screenings` cannot red at all. The lines are `Screening`'s and never
    // this file's — the split register item 934 drew for the four backlogs, held by the same gate
    // in `north_star`'s tests.
    let screenings = match reading.screenings(cap.depth(), &Repository) {
        Ok(screenings) => screenings,
        // ⚠⚠ A FAILURE TO ASK IS ITS OWN FAULT and never forty item faults — see
        // `north_star::Reading::paid_commits`. Being unable to reach `git` says nothing about any
        // ledger line, and a RED that fires on a broken environment is one readers learn to skip.
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    for screening in screenings.each() {
        println!("{screening}");
    }
    println!("{}", backlogs.unrooted);
    // ⛔⛔⛔⛔⛔ AND THE PAID MARKS WHOSE CLAIM NOTHING CHECKED — register item 902, printed
    // beside its three ratchet neighbours because it is the same kind of statement: a backlog with
    // a floor that may only fall. See `north_star::PAID_DECLARATION`.
    println!("{}", backlogs.paid_unnamed);
    // ⚠ A TOTAL AND NOT A BACKLOG, so it names no items and register item 934 does not widen to
    // it: nothing is held back by this number and there is nothing in it for a round to take.
    println!("items {} in section A", reading.items.len());

    // ⛔⛔⛔⛔⛔ AND THE REDS THE LEDGER CLAIMS, AGAINST WHAT THE SUITE SAYS — register item 843.
    //
    // ⚠⚠ PRINTED EVEN WHEN THERE ARE NONE, which is register item 924's finding one gate over: a
    // machinery that examined nothing and a machinery that examined things and found them clean
    // read identically unless the count is stated. `claimed 0` is the sentence a later round needs
    // in order to know this said nothing rather than said yes.
    let claims = reading.red_claims();
    let (standing, refuted) = match reading.standing_reds(&RunTheSuite) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let confirmed: Vec<String> = standing.iter().map(ToString::to_string).collect();
    println!(
        "reds {} claimed, {} standing: {}",
        claims.len(),
        standing.len(),
        confirmed.join(" "),
    );
    // ⛔ A CLAIM THE SUITE REFUTES IS A FAULT ABOUT THE DOCUMENT, and the mirror of item 902's
    // wrongly-paid mark: this one would buy an item past the severity gate on a red that is not
    // there. It is reported here, beside `standing_reds`, because only the caller could ask.
    for number in &refuted {
        eprintln!(
            "north-star: item {number} claims a red the suite says is green — the claim is stale, \
             so remove the `{}` line rather than leaving it to admit the item on a fact that has \
             stopped being true",
            north_star::RED,
        );
    }

    // 🎯🎯🎯🎯🎯 AND WHETHER THE WHOLE THING IS FINISHED — register item 936, printed LAST because
    // it is the conclusion the lines above are the evidence for.
    //
    // ⚠⚠ NOT AN EXIT CODE. `rc` answers *is this ledger well-formed*, and `NOT REACHED` is the
    // ordinary state of a repository with work in it — reding on it would make every round red and
    // teach the reader to skip the one line that matters. The line is the answer; the faults are a
    // different question.
    //
    // ⚠ Printed on BOTH verdicts, with both counts, for register item 924's reason one instrument
    // over: a run that examined nothing and a run that examined everything and found it finished
    // must not read alike.
    println!("{}", reading.ending());

    // ⛔⛔⛔⛔⛔ THE VERDICT, AND IT READS THE SCREENINGS AS ONE VALUE — register item 940.
    //
    // ⚠⚠⚠ THREE TERMS AND NOT FIVE, AND THE TWO BESIDE `screenings` ARE NOT AN ESCAPE HATCH: each
    // already prints its own denominator. `is_green` is the PARSE, whose population is the
    // `items N in section A` line above; `refuted` is judged against the claims counted by the
    // `claimed` half of the `reds` line. So every term here has a printed population — which is
    // what register item 940 was actually asking for, and `north_star`'s tests assert it rather
    // than leaving it to this comment.
    if reading.is_green() && refuted.is_empty() && screenings.all_clean() {
        return std::process::ExitCode::SUCCESS;
    }
    let faults: Vec<&north_star::Fault> =
        reading.faults.iter().chain(screenings.faults()).collect();
    if !faults.is_empty() {
        eprintln!("\n{} fault(s):", faults.len());
        for fault in faults {
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

/// ⛔⛔⛔⛔⛔ **AND WHETHER A CLAIMED RED IS RED** — register item 843, asked by RUNNING the thing.
///
/// # ⚠⚠⚠ Why the suite and not the ledger, and why that is worth what it costs
///
/// Item 843's done-when says the fact must be confirmed *«원장이 아니라 저장소»에 물어서* — the
/// suite knows what is red. A `@red:` line is a claim, and item 902 already paid for the difference
/// between a well-formed mark and a true one. This is the only thing that can tell them apart.
///
/// ⚠ `--no-fail-fast` is NOT passed and `--quiet` is: the question is *is this selection failing*,
/// one bit, and a selection that names several is answered by the first failure as truly as by all
/// of them. ⚠⚠ The arguments are ARGV, checked by `north_star::RED`'s own reader before they get
/// here, and no shell is opened.
///
/// # ⛔⛔ A build that cannot run is *could not ask* and never *green*
///
/// `cargo test` exits 101 for a failing test and 101 for a compile error alike, so this cannot tell
/// them apart and does not pretend to: both are RED, which is the conservative side and the honest
/// one — a tree that does not build is not a tree with no failing tests. What is an [`Err`] is
/// cargo not being runnable at all, which says nothing about any claim.
struct RunTheSuite;

impl north_star::Suite for RunTheSuite {
    fn is_red(&self, names: &str) -> Result<bool, String> {
        let asked = std::process::Command::new("cargo")
            .arg("test")
            .arg("--quiet")
            .args(names.split_whitespace())
            .output()
            .map_err(|why| {
                format!("cannot run the suite to ask whether `{names}` is red: {why}")
            })?;
        match asked.status.code() {
            Some(0) => Ok(false),
            Some(_) => Ok(true),
            // ⚠ Killed by a signal. Nothing was decided, so nothing is reported — the rule
            // `Commits::resolves` states one fact over.
            None => Err(format!(
                "the suite was killed while being asked whether `{names}` is red ({})",
                asked.status,
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
    // ⛔⛔⛔⛔⛔ AND THE SUITE IS ASKED HERE TOO — register item 843. A classifier that judged a
    // proposal on a set the round's own measurement would have widened is the same rule wearing two
    // answers, which is the defect item 839 exists to prevent one layer down.
    //
    // ⚠⚠ IT RUNS NOTHING WHERE NOTHING IS CLAIMED, which is the ordinary case; where a red IS
    // claimed this costs that test on every proposal, and that is the price of the answer being the
    // repository's. A failure to ask is a REFUSAL to judge and never a silent `NO`.
    let standing = match reading.standing_reds(&RunTheSuite) {
        Ok((standing, _)) => standing,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let admitted = reading.admits(cap.depth(), &standing);
    // ⚠⚠ BOUNDED, since register item 936 gave this set a third tier: where nothing marked is
    // takeable it falls through to the unclassified, which is 330 items on the real ledger. An
    // unbounded `join` there is the 1,300-byte line register item 934 was opened by, re-created on
    // the one line a refused run reads to find out what it may do instead.
    let spelled = north_star::name_some(&admitted, north_star::Ends::Lowest);
    let proposal = proposal.to_string_lossy();
    let Some(number) = reading.names(&proposal) else {
        println!(
            "NO — this proposal names no item of the register, so nothing here can say it is one \
             to take now. What a round may take: {spelled}",
        );
        return std::process::ExitCode::SUCCESS;
    };
    // ⛔⛔⛔⛔⛔ **AMBIGUITY IS A REFUSAL, NOT A PASS** — register item 842, and working rule 6.
    //
    // The subject is read as the FIRST item a proposal names, which is a CONVENTION. A proposal
    // that cites an admissible item and is about an inadmissible one reads as being about the
    // citation, and this mode's whole job is then silently past. The two readings are
    // indistinguishable, so both are refused and the reply says how to re-propose. See
    // `Reading::unadmitted_named` for why the population is the OPEN items and for what this costs.
    //
    // ⚠ IT ASKS THE ADMISSION TOO, and that is not redundant: a proposal whose SUBJECT this
    // register withholds is already refused further down, with a reason naming which rule held it
    // back. Answering that one here instead would replace a precise refusal with a vaguer one.
    let muddled = reading.unadmitted_named(&proposal, &admitted);
    if !muddled.is_empty() && admitted.contains(&number) {
        println!(
            "NO — this proposal names item {number}, which may be taken, AND {} this register \
             would not hand out ({}). Nothing here can tell a citation from the work itself, so \
             neither reading is acted on: name the item you are taking, alone. What a round may \
             take: {spelled}",
            match muddled.len() {
                1 => "an open item".to_owned(),
                many => format!("{many} open items"),
            },
            north_star::name_some(&muddled, north_star::Ends::Lowest),
        );
        return std::process::ExitCode::SUCCESS;
    }
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
        // 🎯🎯🎯🎯🎯 AND IF WHAT IS BEING HANDED OVER IS AN UNREAD BLOCK, WHAT TO DO WITH IT —
        // register item 936(1). See `north_star::CLASSIFY_REMEDY`, which owns the sentence and the
        // measurement behind each of its clauses.
        //
        // ⚠⚠ A SECOND LINE, never a change to the first: the reply's opening word is the verdict
        // a run reads, and `sprag_plugin::judge` takes the first MARKED word.
        if reading.backlogs().unclassified.items.contains(&number) {
            println!("  {}", north_star::CLASSIFY_REMEDY);
        }
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
    println!("NO — {why}. What a round may take: {spelled}");
    std::process::ExitCode::SUCCESS
}
