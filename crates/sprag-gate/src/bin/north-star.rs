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
//!
//! # ⛔⛔⛔⛔⛔ `--elsewhere <ledger> <platform> <report>` — register item 973
//!
//! ```text
//! gh run view <run-id> --json jobs            # register item 948 already calls this every round
//! gh api /repos/<owner>/<repo>/actions/jobs/<job-id>/logs > /tmp/macos.log
//! north-star --elsewhere <debt-open.md> macos /tmp/macos.log
//! ```
//!
//! ⛔⛔⛔⛔⛔ **`gh run view --log` IS NOT THAT COMMAND HERE, AND ITS FAILURE IS SILENT.** Measured
//! 2026-09-09 on gh 2.45.0 against run `34304776948`: `--log`, `--log-failed`, per-job and
//! whole-run alike, every one **exit 0, empty stdout, empty stderr** — including the job that had
//! just reported three FAILED tests. `gh api …/jobs/<job-id>/logs` returned 579 kB of the same
//! job. So the reachable road is the API one, and the doc that named the other was measured before
//! it was written down — this workspace's rule 10.
//!
//! ⚠⚠⚠ **AND IT IS NOT *`--log-failed` NEVER WORKS*, WHICH IS WHAT THE PARAGRAPH ABOVE FIRST
//! SAID.** Three runs of this same workflow, three tries each, all `run_attempt` 1 and all with
//! `headless (macos)` as the one failing job: `34301982406` printed 600,181 B every time, while
//! `34304776948` and `34243200719` printed 0 B every time. So it is deterministic PER RUN and it
//! disagrees BETWEEN runs — neither a flake nor a property of this gh. ⛔ **What separates those
//! runs is not measured, so do not build a cause from this.** What is measured is that the API road
//! answered all three (578–579 kB), which is why it is the one named above: a command that is
//! silently empty for some runs cannot be the one a gate's evidence comes through.
//!
//! **THE CLAIMS THIS HOST CANNOT JUDGE, JUDGED FROM THE PLATFORM'S OWN ANSWER.** The default run
//! puts every `@red:` claim to the suite HERE and says so of the rest: *"N claim(s) are about
//! another platform, so this linux run did not judge them"*. Item 949 bought the ability to WRITE a
//! macOS-only red; nothing could ever retire one, so a claim that had since been fixed would stand
//! for ever and go on admitting its item on a fact that had stopped being true.
//!
//! # ⛔⛔⛔⛔⛔ **AND THE FAILURES NO CLAIM HOLDS** — register item 998
//!
//! The same pass prints a second line, over the population the first cannot reach:
//!
//! ```text
//! unclaimed on linux: 1 of 1 reported failure(s): rpc::tests::a_wait_sleeps_through_another_sessions_changes
//! ```
//!
//! `reds … standing` walks the LEDGER's claims; this walks the REPORT's failures. They are
//! different questions over different populations, and the gap between them was measured:
//! 2026-09-09 this binary printed `reds 2 claimed, 0 standing` while `headless (linux)` was failing
//! a test of this workspace, and `--elsewhere` fed the very log carrying that failure answered
//! `0 standing` with rc=0. The red stood eight hosted runs and was found by a person glancing at the
//! job — a CONVENTION, which is the shape rule 10 exists to refuse.
//!
//! ⚠⚠ **THE TWO GREENS SAY WHICH GREEN THEY ARE.** *That report names no failing test at all* and
//! *every failure it reported is held by a claim* are different facts, and a zero that cannot tell
//! them apart cannot say whether this pass looked at anything — register item 924's hazard, one gate
//! over. ⚠ A claim marked for ANOTHER platform holds nothing here: one `@macos` mark must not excuse
//! a linux red of the same test.
//!
//! ⚠⚠⚠ **WHAT THIS DOES NOT REACH, STATED RATHER THAN HIDDEN**: the DEFAULT run. It has no report
//! to read — it asks the suite here, one bit per claim — so a test failing locally that no item
//! claims is still uncounted. Closing that needs the local suite to enumerate its failures, which is
//! a different road from this one.
//!
//! # ⚠⚠⚠ Why the evidence is handed in rather than fetched
//!
//! Item 973 measured two roads and called both the owner's: put the ledger where a runner can read
//! it, or move the job-reading to a machine. This is the second, and it is the cheaper half of it —
//! **nothing here opens a network connection.** The round already reads that job once, because
//! register item 948 obliges it to; this turns that read into a JUDGEMENT instead of a glance. A
//! gate that fetched for itself would need credentials inside a hook, and would be unrunnable
//! exactly where this repository runs its gates.
//!
//! ⚠⚠ **IT FORECLOSES NEITHER ROAD.** Putting the ledger in the repository stays open and would
//! make this mode redundant rather than wrong; until somebody decides that, a stale claim can be
//! retired today instead of never.
//!
//! ⚠ **AN UNREADABLE REPORT IS *COULD NOT ASK*, NEVER *GREEN*** — see [`ReportedFailures`]. The
//! register's answer to a green claim is *delete the `@red:` line*, so a report this could not
//! parse must not be able to instruct that.

use sprag_gate::north_star;
use sprag_gate::north_star::Reds;

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
    if path == *"--elsewhere" {
        return elsewhere(args);
    }
    // ⛔⛔⛔⛔⛔ **`--order` IS HOW THE ORDER IS ASKED FOR, AND THE ONLY HOW** — register item 1052.
    // The head of it prints on every run because that is the line a round acts on; the rest is
    // behind a flag because a round needs one item and a reader auditing the rule needs all of
    // them. Neither is ever written down: a copied order is a snapshot, and the ledger moves under
    // it every time an item is paid or opened.
    let mut whole_order = false;
    for extra in args {
        if extra == *"--order" {
            whole_order = true;
            continue;
        }
        eprintln!(
            "north-star: one ledger, not several — and the only flag after it is `--order`, not {}",
            extra.to_string_lossy(),
        );
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
    // ⛔ THE SUITE IS ASKED HERE AND REPORTED LOWER DOWN — register item 1052. A standing red is one
    // of the two declared overrides that decide what this ledger admits, so the work-order
    // screening cannot be assembled before it is known. Only the ASKING moved: the `reds` lines
    // print where they always did, because the report's order is what a reader has learned.
    let claims = reading.red_claims();
    let here = std::env::consts::OS;
    let found = match reading.standing_reds(&RunTheSuite, here) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let Reds {
        standing,
        refuted,
        elsewhere,
    } = found;
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
    let screenings = match reading.screenings(cap.depth(), &standing, &Repository, &Repository) {
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
    // ⛔⛔⛔⛔⛔ AND A CLAIM ABOUT ANOTHER PLATFORM IS COUNTED RATHER THAN SILENT — register item
    // 949. Measured over four hosted runs on 2026-09-08: `headless (macos)` failed at `Test` on
    // every one while this line printed `reds 0 claimed, 0 standing`. The counter was not wrong
    // about its population; it had no way to HOLD that population, so *nobody asked* and *asked
    // and clean* were the same zero. This third number is that distinction, and item 924 is the
    // rule it is written under.
    // ⚠ The suite was asked further up — see the note there. What is left here is the REPORT, which
    // stays where it has always been.
    let confirmed: Vec<String> = standing.iter().map(ToString::to_string).collect();
    let others: Vec<String> = elsewhere.iter().map(ToString::to_string).collect();
    println!(
        "reds {} claimed, {} standing: {}",
        claims.len(),
        standing.len(),
        confirmed.join(" "),
    );
    println!(
        "  {} claim(s) are about another platform, so this {here} run did not judge them: {}",
        elsewhere.len(),
        others.join(" "),
    );
    // ⛔⛔⛔⛔⛔ **WHAT TO TAKE NEXT, DERIVED — AND THE TERM THAT PLACED IT** — register item 1052.
    //
    // Working rule 11 forbids choosing by eye and then, when `critical` is empty, hands the round a
    // sentence: *population 을 이음매가 맞는 순서로*. MEASURED 2026-09-11: this binary printed no
    // such line and `north_star` derived no order (0 and 0), `critical` read 0 on all twelve
    // readings of that session, and the supervisor chose by eye eight times in one day.
    //
    // ⚠⚠ It is printed BELOW the reds because it depends on them — a standing red is one of the two
    // declared overrides — and above `north star:` because it is the line acted on. The terms are
    // [`north_star::Reading::work_order`]'s and are not re-spelled here: a second author of the
    // order would be the drift item 213 is named for.
    let order = reading.work_order(cap.depth(), &standing);
    match order.first() {
        Some(first) => println!("next {}: {}", first.number, first.why),
        // ⛔ AN ANSWER, NOT AN ABSTENTION — rule 6. Nothing admitted means the ledger has nothing a
        // round may take, which is a fact about the ledger and not a missing line.
        None => println!("next none: this ledger admits nothing a round may take"),
    }
    println!(
        "  {} item(s) in the order. ⛔ Do not write this order down; ask for it (--order).",
        order.len(),
    );
    if whole_order {
        for placed in &order {
            println!("  {} — {}", placed.number, placed.why);
        }
    }
    // ⛔⛔⛔ WHETHER THE ORDER IS THE WHOLE OF WHAT IS ADMITTED is a `Screening` and is printed with
    // its four siblings above — register item 1052, corrected by item 940's own gate. The first
    // draft judged it here with a bare `missed.is_empty()` in the verdict, and that gate refused
    // the build in as many words: a term that is not `screenings` is a gate whose population
    // nothing prints.
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
    fn descends(&self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let asked = std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .output()
            .map_err(|why| {
                format!("cannot ask git whether {descendant} descends from {ancestor}: {why}")
            })?;
        match asked.status.code() {
            Some(0) => Ok(true),
            // ⚠ 1 is *no*, and every other code is *could not ask* — the split
            // [`north_star::Commits`] states, and the reason a broken environment does not read as
            // a verdict about the ledger.
            Some(1) => Ok(false),
            _ => Err(format!(
                "git could not be asked whether {descendant} descends from {ancestor} ({}): {}",
                asked.status,
                String::from_utf8_lossy(&asked.stderr).trim(),
            )),
        }
    }

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

/// ⛔⛔⛔⛔⛔ **AND THE SAME REPOSITORY, ASKED WHAT ITS FILES SAY** — register item 488.
///
/// # ⚠⚠⚠ The working directory is the subject, and that is already this binary's contract
///
/// [`north_star::Commits`] above resolves ids by running `git` here, so this binary has always
/// meant *the repository I am standing in* and the register's own operating note says so in as many
/// words: *cwd 는 저장소*. A witness path is read the same way and against the same tree, so the two
/// answers cannot come from two different repositories.
///
/// ⚠ A path that is not there is `Ok(None)` — a FACT about this tree, which the caller turns into
/// one item's fault. A directory, or bytes that are not UTF-8, is the same answer for the same
/// reason: no text to search is not a broken environment, it is a witness naming the wrong thing.
/// Only being unable to look at all would be an `Err`, and `std::fs::read_to_string` reports that
/// as the same `io::Error` it reports absence with, so this reads absence as absence and says so.
impl north_star::Tree for Repository {
    fn read(&self, path: &str) -> Result<Option<String>, String> {
        match std::fs::read_to_string(path) {
            Ok(body) => Ok(Some(body)),
            Err(_) => Ok(None),
        }
    }
}

/// ⛔⛔⛔⛔⛔ **WHETHER A CLAIMED RED IS RED *SOMEWHERE ELSE*, ASKED OF THAT PLACE'S OWN ANSWER** —
/// register item 973, and [`RunTheSuite`]'s counterpart for the claims this host cannot put.
///
/// # ⛔⛔⛔⛔⛔ What could not be retired, and what it was costing
///
/// Item 949 made a macOS-only red WRITABLE; nothing could ever make one FALSE. `standing_reds`
/// skips a claim about another platform, so `refuted` — the answer that says *delete the `@red:`
/// line* — was unreachable for every one of them. A claim fixed on macOS would go on admitting its
/// item past the severity gate for ever, on a fact that had stopped being true. That is item 902's
/// shape with the sign flipped, and 973 is where it was written down.
///
/// # ⚠⚠⚠⚠⚠ THE REPORT IS EVIDENCE, AND AN UNREADABLE ONE SAYS NOTHING
///
/// A missing download and a job with nothing to report are the same empty file, and only one of
/// them means *green*. Since *green* is the answer that instructs a deletion, this refuses to give
/// it on a file it cannot recognise: a report with no `test … ok|FAILED|ignored` line in it at all
/// is [`None`] from [`ReportedFailures::of`], and the run then refuses instead of judging a single
/// claim. That is [`north_star::Suite::is_red`]'s own stated split one level up, and this
/// workspace's rule 6: an unclassified thing is a RED and not a pass.
///
/// ⚠⚠ **SO A GREEN JOB MUST BE HANDED ITS WHOLE LOG.** The contract is stated because it cannot be
/// inferred: `gh run view --log-failed` prints nothing for a job that passed, and nothing is
/// exactly what a failed fetch prints. ⛔ And measured on this machine it prints nothing for some
/// runs whose job FAILED either — the module doc carries that measurement and the road that
/// answered every run it was asked about.
struct ReportedFailures {
    /// Every test the report says FAILED, in the harness's own spelling.
    failed: Vec<String>,
}

/// ⛔⛔⛔⛔⛔ **WHY A REPORT COULD NOT BE READ, AS A NAMED CAUSE** — register item 998.
///
/// Two files this reader must refuse, and they are refused for OPPOSITE reasons, so one sentence
/// over both would send a reader to the wrong place. A [`Display`](std::fmt::Display) rather than a
/// bare discriminant because the sentence is the whole value of the refusal: this mode's answer to a
/// green claim is *delete the `@red:` line*, and a reader told only *unreadable* cannot tell a
/// download that brought nothing back from a parser that has fallen out of step with the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unread {
    /// Nothing in the file reads as a harness line — an empty download, an error page, the wrong
    /// file. It cannot answer a question about tests at all.
    NotATestLog,
    /// ⛔⛔⛔ The file SUMMARISES a failing run and not one `test … FAILED` line could be read out
    /// of it — register item 998's ⑷, and register item 971's `ran == 0` guard wearing this reader's
    /// clothes.
    ///
    /// The failure names are extracted by matching the harness's per-test line. If that spelling
    /// ever changes, the extraction silently yields NOTHING — and nothing is indistinguishable from
    /// a platform that failed nothing, which is the answer that refutes every claim and reports no
    /// unclaimed red. The tally line is a second witness to the same fact, written by the same
    /// harness, and a disagreement between the two is this reader being wrong rather than the
    /// platform being green.
    NamesOutOfStep { tallies: usize },
}

impl std::fmt::Display for Unread {
    fn fmt(&self, into: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotATestLog => write!(
                into,
                "nothing in it reads as a harness line, so this cannot tell a job that reported \
                 nothing from a fetch that brought nothing back"
            ),
            Self::NamesOutOfStep { tallies } => write!(
                into,
                "{tallies} tally line(s) in it say a run FAILED, and not one `test <name> ... \
                 FAILED` line could be read out of it — so this reader's spelling is out of step \
                 with the log rather than the platform being green"
            ),
        }
    }
}

impl ReportedFailures {
    /// The lines a test harness prints per test, which is what makes a report RECOGNISABLE.
    ///
    /// ⚠ Read as a prefix and a marker rather than a full grammar: the report is a CI log with the
    /// runner's own timestamps and job names glued to the front of every line, so an anchored match
    /// would recognise nothing. What has to be true is that the file is a test log at all.
    fn reads_as_a_test_log(text: &str) -> bool {
        text.lines().any(|line| {
            line.contains(" ... ok")
                || line.contains(" ... FAILED")
                || line.contains(" ... ignored")
                || line.contains("test result:")
        })
    }

    /// What the report says failed. ⚠ The NAME only — the harness prints `test <name> ... FAILED`
    /// and a CI log puts its own prefix before the word `test`.
    ///
    /// # Errors
    ///
    /// An [`Unread`] naming which of the two unreadable files this is. ⛔ Never an empty reading for
    /// either of them: an empty reading is what says *this platform failed nothing*, and that is the
    /// answer which instructs a deletion.
    fn of(text: &str) -> Result<Self, Unread> {
        if !Self::reads_as_a_test_log(text) {
            return Err(Unread::NotATestLog);
        }
        let failed: Vec<String> = text
            .lines()
            .filter_map(|line| line.split_once(" ... FAILED").map(|(head, _)| head))
            .filter_map(|head| head.rsplit_once("test ").map(|(_, name)| name))
            .map(|name| name.trim().to_owned())
            .collect();
        // ⛔⛔⛔⛔⛔ THE TWO WITNESSES MUST AGREE — register item 998's ⑷. The harness writes both
        // lines; this reads one of them for names and the other for a count, and the only way they
        // disagree is that the reading is wrong.
        let tallies_that_failed = text
            .lines()
            .filter(|line| line.contains("test result: FAILED"))
            .count();
        if failed.is_empty() && tallies_that_failed > 0 {
            return Err(Unread::NamesOutOfStep {
                tallies: tallies_that_failed,
            });
        }
        Ok(Self { failed })
    }

    /// ⛔⛔⛔ **WHAT AN ARGV SELECTS**, as the harness would filter on it — register item 973.
    ///
    /// A `@red:` argv is `cargo test`'s, so the selection is the one bare word that is neither a
    /// flag nor a flag's value nor past the `--`. `-p sprag-gate --lib launcher::tests -- --exact`
    /// selects `launcher::tests`.
    ///
    /// ⚠⚠ [`None`] where the argv names NO filter (`-p sprag-host --lib` selects a whole target).
    /// That is not *nothing failed*: it is a question this reader cannot put to a list of names,
    /// and it is answered as *could not ask* rather than guessed at.
    fn selected_by(argv: &str) -> Option<String> {
        let mut takes_a_value = false;
        for token in argv.split_whitespace() {
            if token == "--" {
                // ⚠ Everything past it is the HARNESS's arguments (`--exact`, `--nocapture`), never
                // a selection. A reader that kept going would take `--exact` for a test name.
                return None;
            }
            if takes_a_value {
                takes_a_value = false;
                continue;
            }
            if token.starts_with('-') {
                // ⚠ The flags that eat the next word. A list rather than a guess: an unknown flag
                // that took a value would otherwise make its value look like a selection.
                takes_a_value =
                    matches!(token, "-p" | "--package" | "--test" | "--bin" | "--example");
                continue;
            }
            return Some(token.to_owned());
        }
        None
    }
}

impl north_star::Suite for ReportedFailures {
    fn is_red(&self, names: &str) -> Result<bool, String> {
        let Some(selection) = Self::selected_by(names) else {
            return Err(format!(
                "the claim `{names}` names no test selection this reader can put to a list of \
                 names, so what that platform reported cannot answer it"
            ));
        };
        // ⚠⚠ A PREFIX, because `cargo test` filters by substring and a claim may name a MODULE
        // (`launcher::tests`) whose failures are reported per test. ⚠ `--exact` is not honoured
        // here and must not be: the report says which tests failed, and a module whose tests failed
        // is a module that is red however the claim spells its filter.
        Ok(self
            .failed
            .iter()
            .any(|name| name == &selection || name.starts_with(&selection)))
    }
}

/// ⛔⛔⛔⛔⛔ **AND WHICH OF ITS FAILURES THE LEDGER DOES NOT ACCOUNT FOR** — register item 998.
///
/// One match rule, reached from both directions: [`north_star::Suite::is_red`] above asks *does any
/// failure answer this claim* and [`north_star::Reported::accounts_for`] asks *does this claim
/// answer that failure*.
/// Both go through [`ReportedFailures::selected_by`] and the same prefix reading, so a module-shaped
/// claim (`launcher::tests`) holds the tests inside it in the inverse direction too — a second
/// spelling here would let one direction hold a failure the other reported unclaimed.
impl north_star::Reported for ReportedFailures {
    fn failures(&self) -> Vec<String> {
        self.failed.clone()
    }

    fn accounts_for(&self, argv: &str, failure: &str) -> Result<bool, String> {
        let Some(selection) = Self::selected_by(argv) else {
            return Err(format!(
                "the claim `{argv}` names no test selection this reader can put to a name, so \
                 whether it holds `{failure}` cannot be asked"
            ));
        };
        Ok(failure == selection || failure.starts_with(&selection))
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
/// one — a tree that does not build is not a tree with no failing tests.
///
/// ⚠⚠⚠ **AND EVERY OTHER NON-ZERO CODE IS *COULD NOT ASK*** — register item 949, which is a
/// correction to this paragraph and not an addition to it. It used to end *what is an `Err` is
/// cargo not being runnable at all*, and that was too narrow: cargo also refuses the ARGUMENTS,
/// with exit 1, and this reader called that a standing red. See [`verdict_of`], where the three
/// answers are and where the measurement is.
///
/// ⚠⚠ **AND [`ReportedFailures`] IS ITS COUNTERPART FOR THE CLAIMS THIS HOST CANNOT PUT** —
/// register item 973. Same question, evidence from somewhere else's own report rather than from
/// running the suite here.
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
        verdict_of(
            asked.status.code(),
            tests_run(&String::from_utf8_lossy(&asked.stdout)),
            names,
        )
    }
}

/// ⛔⛔⛔⛔⛔ **HOW MANY TESTS THE SELECTION ACTUALLY RAN** — register item 971's ⑴, read from the
/// harness's own line.
///
/// # ⛔⛔⛔ Why exit 0 was two different answers
///
/// Measured 2026-09-08: `cargo test --quiet -p sprag-gate --lib zzz_no_such_test_anywhere --
/// --exact` exits **0** and prints
///
/// ```text
/// running 0 tests
///
/// test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 185 filtered out; finished in 0.00s
/// ```
///
/// ⇒ A `@red:` line whose test NAME has a typo therefore reads as *the suite says this is green*,
/// and the register's answer to a green claim is *the claim is stale, remove the `@red:` line*. So
/// a typo does not merely fail to confirm the red — it instructs the next round to DELETE the
/// claim. That is item 949's shape from the other side: there, a malformed mark manufactured the
/// red it claimed; here, it manufactures the refutation.
///
/// # ⚠⚠ Summed across targets, because one selection can reach several harnesses
///
/// `-p sprag-host` alone runs the lib and every integration test, and each prints its own line. A
/// selection that ran nothing ANYWHERE is the case this exists for; one that ran nothing in one
/// target and something in another has still had its question put.
///
/// ⚠ It reads `running N tests` rather than the `test result:` tally, because that line is printed
/// BEFORE the tests and survives a run the harness never got to summarise.
fn tests_run(stdout: &str) -> usize {
    stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("running "))
        .filter_map(|rest| rest.split_whitespace().next())
        .filter_map(|count| count.parse::<usize>().ok())
        .sum()
}

/// ⛔⛔⛔⛔⛔ **WHAT AN EXIT CODE FROM `cargo test` SAYS ABOUT A CLAIM** — register item 949, and a
/// function taking its input rather than a `match` inside the call that spawns the process.
///
/// The three cases below cannot all be produced by a real run of this binary: `Ok(false)` needs a
/// green selection, `Ok(true)` a red one, and the [`Err`] arms a malformed claim — so asserting
/// around the process would leave two of them unmeasured, which is the dead control this workspace
/// keeps paying for. Handed the code, every case is driven.
fn verdict_of(code: Option<i32>, ran: usize, names: &str) -> Result<bool, String> {
    match code {
        // ⛔⛔⛔⛔⛔ **EXIT 0 HAVING RUN NOTHING IS *THE QUESTION WAS NOT PUT*** — register item
        // 971's ⑴, and the mirror of the 949 arm below.
        //
        // A selection no test matches exits 0 and says `running 0 tests` (see [`tests_run`] for
        // the measurement). Read as green, that instructs the next round to delete the `@red:`
        // line as stale — so a typo in the NAME refutes the claim just as surely as a typo in the
        // FLAGS used to confirm it. Neither is an answer, and both are refusals to judge.
        Some(0) if ran == 0 => Err(format!(
            "the selection `{names}` matched no test at all — it exited 0 having run nothing, so \
             the suite has said neither red nor green. A green answer here would read as *the \
             claim is stale* and instruct its removal. Check the test NAME (a harness filter is a \
             substring unless `--exact` follows a bare `--`, and either way it has to exist)"
        )),
        Some(0) => Ok(false),
        // ⛔⛔⛔⛔⛔ 101 IS *THE SUITE RAN AND SOMETHING FAILED*, AND EVERY OTHER NON-ZERO IS
        // *THE QUESTION COULD NOT BE PUT* — register item 949, measured the hard way.
        //
        // `Some(_) => Ok(true)` was here, on the reasoning that a tree which does not build is
        // not a tree with no failing tests. That is right about a COMPILE error, which cargo
        // also exits 101 for. It is wrong about cargo refusing the ARGUMENTS: measured
        // 2026-09-08 while writing the first two platform claims of item 949, `cargo test
        // --quiet -p sprag-host --test cli <name> --exact` exits **1** with *"unexpected
        // argument '--exact' found"* — because `--exact` belongs to the harness, past `--`.
        //
        // ⇒ Both of this round's own `@red:` lines were malformed, and this reader called them
        // STANDING REDS. A typo in the mark manufactured the very fact the mark exists to
        // claim, and it would have bought both items past the severity gate. That is item 902's
        // shape from the third side: not a mark that is false, a mark that MAKES itself true.
        //
        // ⚠ A failure to ask is its own fault and never a verdict — `Commits::resolves`' rule,
        // and the reason this function returns a `Result` at all.
        Some(101) => Ok(true),
        Some(other) => Err(format!(
            "cargo refused the question `{names}` with exit {other} rather than running it — a \
             test failure and a compile error are both 101, so this is the arguments themselves. \
             Harness flags such as `--exact` go after a bare `--`"
        )),
        // ⚠ Killed by a signal. Nothing was decided, so nothing is reported — the rule
        // `Commits::resolves` states one fact over.
        None => Err(format!(
            "the suite was killed while being asked whether `{names}` is red (no exit code)"
        )),
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

/// ⛔⛔⛔⛔⛔ **JUDGE THE CLAIMS THIS HOST CANNOT** — register item 973, and the answer to the line
/// the default run has been printing since item 949: *"N claim(s) are about another platform, so
/// this linux run did not judge them"*.
///
/// It is the SAME judgement, with the platform and the suite swapped: [`north_star::Reading::standing_reds`]
/// walks the same claims, `refuted` means the same thing, and the sentence a stale claim earns is
/// the one the default run already prints. Nothing about what a claim MEANS is decided here.
///
/// ⚠⚠ **IT EXITS 1 ON A STALE CLAIM AND ON A REPORT IT COULD NOT READ ALIKE**, which is the
/// conservative pairing: one says the ledger is wrong, the other says this could not tell, and
/// neither is a run whose silence should read as *everything is still red*.
///
/// # Errors
///
/// Prints and exits 1: the arguments are wrong, the ledger or report will not read, the platform is
/// not one this build knows, or the report is not a test log.
fn elsewhere(mut args: impl Iterator<Item = std::ffi::OsString>) -> std::process::ExitCode {
    let (Some(ledger), Some(platform), Some(report), Some(run), Some(at)) = (
        args.next(),
        args.next(),
        args.next(),
        args.next(),
        args.next(),
    ) else {
        eprintln!(
            "north-star: --elsewhere needs the ledger's path, the platform, a file holding that \
             platform's own test log, and THE RUN THAT LOG IS FROM — its id and the commit it was \
             for.\n  \
             gh run view <run-id> --json jobs,headSha\n  \
             gh api /repos/<owner>/<repo>/actions/jobs/<job-id>/logs > /tmp/macos.log\n  \
             north-star --elsewhere <debt-open.md> macos /tmp/macos.log <run-id> <commit>\n\
             The run is REQUIRED because evidence with no date cannot be compared with the \
             evidence the ledger already holds — measured 2026-09-10, a log from two runs earlier \
             judged the same claims and exited 0.\n\
             Not `gh run view --log-failed`: measured on three runs of one workflow, it exits 0 \
             printing nothing at all for some of them — and nothing is the one thing this mode \
             refuses to read as green.",
        );
        return std::process::ExitCode::FAILURE;
    };
    if args.next().is_some() {
        eprintln!(
            "north-star: --elsewhere takes one ledger, one platform, one report and the one run \
             that report is from"
        );
        return std::process::ExitCode::FAILURE;
    }
    let run = run.to_string_lossy().into_owned();
    let at = at.to_string_lossy().into_owned();
    // ⛔⛔⛔⛔⛔ A DATE THIS TREE CANNOT RESOLVE IS NO DATE — register item 1000, and register item
    // 902's rule at the moment the evidence is taken rather than after it is written down. A commit
    // nobody has cannot be compared with anything, so every answer below would be *unrelated*, and
    // an author would be told to fix the ledger about a report that was mislabelled.
    match north_star::Commits::resolves(&Repository, &at) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!(
                "north-star: this tree cannot resolve {at}, so the report cannot be dated and its \
                 judgement cannot be compared with what the ledger already holds. Name the commit \
                 the run was FOR — `gh run view {run} --json headSha`",
            );
            return std::process::ExitCode::FAILURE;
        }
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    }
    let platform = platform.to_string_lossy().into_owned();
    // ⛔ A PLATFORM THIS BUILD DOES NOT KNOW IS A REFUSAL THAT NAMES WHAT THERE IS — rule 6. A run
    // asked about `darwin` would otherwise judge nothing and exit 0, which reads as *every claim
    // is fine* about a question nobody answered.
    // ⚠⚠ Asked of the reader that reads the mark, and answered with the list that vocabulary
    // itself keeps: a second way of spelling the set here could disagree with the `@red:` lines.
    if north_star::Platform::parse(&platform).is_none() {
        eprintln!(
            "north-star: nothing in this build spells a platform as {platform:?}. The ones it \
             knows are {}.",
            north_star::Platform::words(),
        );
        return std::process::ExitCode::FAILURE;
    }
    let text = match std::fs::read_to_string(&ledger) {
        Ok(text) => text,
        Err(error) => {
            eprintln!(
                "north-star: cannot read {}: {error}",
                ledger.to_string_lossy()
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let said = match std::fs::read_to_string(&report) {
        Ok(said) => said,
        Err(error) => {
            eprintln!(
                "north-star: cannot read {}: {error}",
                report.to_string_lossy()
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let suite = match ReportedFailures::of(&said) {
        Ok(suite) => suite,
        Err(why) => {
            eprintln!(
                "north-star: {} cannot be read as a test report: {why}. The answer to a green \
                 claim is *delete the `{}` line*, and a file this could not read must not be able \
                 to instruct that.",
                report.to_string_lossy(),
                north_star::RED,
            );
            return std::process::ExitCode::FAILURE;
        }
    };
    let reading = north_star::read(&text);
    let found = match reading.standing_reds(&suite, &platform) {
        Ok(found) => found,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    let Reds {
        standing,
        refuted,
        elsewhere,
    } = found;
    let confirmed: Vec<String> = standing.iter().map(ToString::to_string).collect();
    let others: Vec<String> = elsewhere.iter().map(ToString::to_string).collect();
    println!(
        "reds on {platform}: {} standing: {}",
        standing.len(),
        confirmed.join(" "),
    );
    println!(
        "  {} claim(s) are about somewhere else, so this pass did not judge them: {}",
        elsewhere.len(),
        others.join(" "),
    );
    // ⚠ THE SAME SENTENCE THE DEFAULT RUN PRINTS, because it is the same finding: a claim the
    // evidence refutes is stale wherever the evidence came from.
    for number in &refuted {
        eprintln!(
            "north-star: item {number} claims a red that {platform} reports green — the claim is \
             stale, so remove the `{}` line rather than leaving it to admit the item on a fact \
             that has stopped being true",
            north_star::RED,
        );
    }
    // ⛔⛔⛔⛔⛔ **AND THE OTHER DIRECTION, WHICH NOTHING USED TO ASK** — register item 998. The
    // lines above judge the ledger's CLAIMS; this judges the report's FAILURES, and a job can be red
    // with every claim in the register standing true.
    let unclaimed = match reading.unclaimed_failures(&suite, &platform) {
        Ok(unclaimed) => unclaimed,
        Err(why) => {
            eprintln!("north-star: {why}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // ⚠⚠ THE TWO GREENS SAY WHICH GREEN THEY ARE — this workspace's rule 5 asked of a zero, and
    // register item 924's hazard one gate over: *nothing failed* and *everything that failed is
    // accounted for* are different facts, and a reader who cannot tell them apart cannot know
    // whether this pass looked at anything.
    if unclaimed.failures.is_empty() {
        if unclaimed.reported == 0 {
            println!(
                "unclaimed on {platform}: 0 — that report names no failing test at all, so there \
                 is nothing here for a claim to hold ({} claim(s) could have been asked)",
                unclaimed.asked,
            );
        } else {
            println!(
                "unclaimed on {platform}: 0 of {} reported failure(s) — every one is held by a \
                 claim ({} claim(s) applied here)",
                unclaimed.reported, unclaimed.asked,
            );
        }
    } else {
        println!(
            "unclaimed on {platform}: {} of {} reported failure(s): {}",
            unclaimed.failures.len(),
            unclaimed.reported,
            unclaimed.failures.join(" "),
        );
        for failure in &unclaimed.failures {
            eprintln!(
                "north-star: {platform} reports `{failure}` FAILING and no open item claims it — \
                 none of the {} claim(s) this report could answer selects that test, so this red \
                 stands in nobody's register and every count this instrument prints is green about \
                 it. Open an item for it, or mark an existing one with a `{}` line that selects it.",
                unclaimed.asked,
                north_star::RED,
            );
        }
    }
    // ⛔⛔⛔⛔⛔ **AND WHAT THIS JUDGEMENT LEAVES THE LEDGER OWING** — register item 1000. A claim
    // this pass CONFIRMED was judged just now, at a run this caller named; if the ledger records an
    // older run, that record is stale from this moment and the line to write is one this pass can
    // print in full.
    //
    // ⚠ Only the STANDING claims. A refuted one's remedy is to delete its `@red:` line, and the
    // evidence obligation goes with it — telling an author to record a run for a claim they are
    // being told to remove would be two instructions pointing opposite ways.
    let mut unrecorded = 0;
    for number in &standing {
        let owed = match reading.recording_of(*number, &at, &Repository) {
            Ok(owed) => owed,
            Err(why) => {
                eprintln!("north-star: {why}");
                return std::process::ExitCode::FAILURE;
            }
        };
        let line = format!("{} @{platform} {run} {at}", north_star::JUDGED);
        match owed {
            north_star::Recording::Current => {}
            north_star::Recording::Absent => {
                unrecorded += 1;
                eprintln!(
                    "north-star: item {number} is confirmed red by run {run} and records no \
                     `{}` line at all. Write it, under that item's `{}` line: `{line}`",
                    north_star::JUDGED,
                    north_star::RED,
                );
            }
            north_star::Recording::Stale(was) => {
                unrecorded += 1;
                eprintln!(
                    "north-star: item {number} is confirmed red by run {run} at {at}, and still \
                     records run {} at {}, which that commit descends from. The judgement just \
                     made is the newer one and nothing has written it down — replace that line \
                     with: `{line}`",
                    was.run, was.at,
                );
            }
            // ⚠⚠ NOT COUNTED AS UNRECORDED, and that is the point: the ledger is AHEAD of this
            // report, so there is nothing to record and recording would move the evidence
            // backwards. It is said out loud because feeding an old log is a mistake a reader
            // should hear about, and it is not this ledger's fault.
            north_star::Recording::Behind(was) => eprintln!(
                "north-star: item {number} already records run {} at {}, which descends from {at} \
                 — this report is OLDER than the evidence the ledger holds, so it was not \
                 recorded. Judging with it would move the record backwards",
                was.run, was.at,
            ),
            north_star::Recording::Unrelated(was) => {
                unrecorded += 1;
                eprintln!(
                    "north-star: item {number} records run {} at {}, and neither that commit nor \
                     {at} is in the other's history — so which judgement is the later one cannot \
                     be read here. Name a run this branch contains",
                    was.run, was.at,
                );
            }
        }
    }
    println!(
        "unrecorded on {platform}: {unrecorded} of {} standing claim(s)",
        standing.len()
    );
    if refuted.is_empty() && unclaimed.failures.is_empty() && unrecorded == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
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
    // ⚠⚠ AND `admits` GETS THE CONFIRMED ONES ONLY — register item 949. A claim about another
    // platform is not a red this machine can put anybody onto: it would be admitting an item on a
    // fact nothing here checked, which is the same purchase-past-the-gate `refuted` refuses.
    let standing = match reading.standing_reds(&RunTheSuite, std::env::consts::OS) {
        Ok(found) => found.standing,
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

#[cfg(test)]
mod tests {
    use super::{ReportedFailures, Unread, tests_run, verdict_of};
    use sprag_gate::north_star::{Reported, Suite};

    /// A macOS job log's shape, cut to what this reader has to recognise: the runner glues a job
    /// name and a timestamp to the front of every line, which is why nothing here is anchored.
    const A_MACOS_LOG: &str = "\
headless (macos)\tTest\t2026-09-09T02:16:09Z test launcher::tests::a_build_that_fits ... FAILED
headless (macos)\tTest\t2026-09-09T02:16:09Z test launcher::tests::a_daemon_moved_past ... FAILED
headless (macos)\tTest\t2026-09-09T02:18:43Z test plugins::tests::a_loop_over_the_wire ... FAILED
headless (macos)\tTest\t2026-09-09T02:18:52Z test north_star::tests::something_else ... ok
headless (macos)\tTest\t2026-09-09T02:18:52Z test result: FAILED. 610 passed; 3 failed";

    /// ⛔⛔⛔⛔⛔ **A CLAIM ABOUT ANOTHER PLATFORM CAN NOW BE REFUTED, AND ONLY BY THAT PLATFORM'S
    /// OWN ANSWER** — register item 973.
    ///
    /// # ⛔⛔⛔⛔⛔ What could not be retired
    ///
    /// Item 949 made a macOS-only red WRITABLE and nothing could make one FALSE: `standing_reds`
    /// skips a claim about another platform, so `refuted` — the answer that says *delete the
    /// `@red:` line* — was unreachable for every one of them. A claim fixed on macOS would go on
    /// admitting its item past the severity gate for ever.
    ///
    /// # ⚠⚠⚠⚠⚠ Both answers, or this is a constant
    ///
    /// A reader that answered `true` to everything confirms every claim and retires none — which is
    /// today's behaviour with more code. A reader that answered `false` to everything instructs the
    /// deletion of every macOS claim there is. So the arms below assert a name the report carries
    /// AND a name it does not, off ONE report.
    #[test]
    fn a_platforms_own_report_confirms_what_it_failed_and_refutes_what_it_did_not() {
        let suite = ReportedFailures::of(A_MACOS_LOG).expect("that reads as a test log");
        assert_eq!(
            suite.is_red("-p sprag-gate --lib launcher::tests -- --exact"),
            Ok(true),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 973: a claim whose tests this platform REPORTS FAILING reads \
             as green, so the round would be told to delete a mark that is still true",
        );
        assert_eq!(
            suite.is_red("-p sprag-gate --lib north_star::tests::something_else -- --exact"),
            Ok(false),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 973: a claim this platform reports PASSING still reads as \
             red, so nothing could ever retire a macOS claim — which is the whole of the item",
        );
        // ⚠⚠ THE PREFIX IS THE POINT AND NOT A CONVENIENCE: 970's claim names a MODULE
        // (`launcher::tests`) and the report names the tests inside it. A reader matching only
        // whole names would answer `false` for every module-shaped claim — the refutation
        // manufactured, which is item 949's defect from the other side.
        assert_eq!(
            suite.is_red("-p sprag-gate --lib launcher::tests::a_daemon_moved_past -- --exact"),
            Ok(true),
            "⚠ and a claim naming one test of that module is answered by that test's own line",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A REPORT THIS COULD NOT READ SAYS NOTHING, AND NEVER *GREEN*** — register item
    /// 973, and this workspace's rule 6 at the one place it decides a DELETION.
    ///
    /// A failed fetch and a job with nothing to report are the same empty file, and only one of
    /// them means every claim is stale. Since *green* is the answer that instructs removing a
    /// `@red:` line, an unrecognisable report must not be able to give it.
    #[test]
    fn a_report_that_is_not_a_test_log_is_refused_rather_than_read_as_green() {
        assert_eq!(
            ReportedFailures::of("").err(),
            Some(Unread::NotATestLog),
            "⛔⛔⛔⛔⛔ RULE 6: an EMPTY report — which is exactly what a failed download leaves — \
             reads as a platform that failed nothing, and the register's answer to that is *delete \
             every macOS claim*",
        );
        assert_eq!(
            ReportedFailures::of("gh: could not find run 1234\nnot found\n").err(),
            Some(Unread::NotATestLog),
            "⛔⛔⛔ and so does an error page: nothing in it is a harness line, so it cannot \
             answer a question about tests",
        );
        assert!(
            ReportedFailures::of("some prefix test result: ok. 3 passed; 0 failed").is_ok(),
            "⚠⚠ THE CONTROL: a job that reported and failed NOTHING must still be readable, or a \
             green platform could never retire a stale claim — which is the case this item most \
             wants to catch",
        );
        let green = ReportedFailures::of("test result: ok. 3 passed; 0 failed")
            .expect("a tally alone is a test log");
        assert_eq!(
            green.is_red("-p sprag-gate --lib launcher::tests -- --exact"),
            Ok(false),
            "⚠ and every claim against it is refuted, which is the answer a green platform owes",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A TALLY THAT SAYS *FAILED* WITH NO NAMES TO SHOW IS THIS READER BEING WRONG** —
    /// register item 998's ⑷, and register item 971's `ran == 0` guard in this reader's clothes.
    ///
    /// # ⛔⛔⛔ What a silent zero here would answer
    ///
    /// Both questions this mode asks are answered from the extracted NAMES: a claim is refuted when
    /// no name matches it, and a failure is unclaimed when no claim matches it. So an extraction that
    /// silently yields nothing answers *every claim is stale* AND *no red is unclaimed* — both greens,
    /// about a job that failed. The harness writes a tally line too, and the two cannot disagree
    /// unless this reader's spelling has fallen out of step with the log.
    ///
    /// ⚠⚠ **THE CONTROL ARM IS WHAT MAKES THIS MORE THAN A CONSTANT**: the same tally with its
    /// per-test lines present reads fine, so the guard is judging the DISAGREEMENT and not the word
    /// `FAILED`.
    #[test]
    fn a_report_that_tallies_failures_it_cannot_name_is_refused_rather_than_read_as_green() {
        // The tally the harness prints, with the per-test lines dropped — exactly what a changed
        // per-test spelling would leave this reader holding.
        let names_gone = "\
headless (macos)\tTest\t2026-09-09T02:18:52Z test result: FAILED. 610 passed; 3 failed";
        assert_eq!(
            ReportedFailures::of(names_gone).err(),
            Some(Unread::NamesOutOfStep { tallies: 1 }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 998: a log that SAYS three tests failed and shows this reader \
             none of them reads as a platform that failed nothing — which refutes every claim and \
             reports no unclaimed red, two greens about a red job",
        );
        let both_witnesses = ReportedFailures::of(A_MACOS_LOG).expect("that reads as a test log");
        assert_eq!(
            both_witnesses.failed.len(),
            3,
            "⚠⚠ THE CONTROL: the same tally WITH its per-test lines is read, so the guard above is \
             about the two witnesses disagreeing and not about the word FAILED appearing",
        );
        let green = ReportedFailures::of("test result: ok. 3 passed; 0 failed")
            .expect("a passing tally is a test log");
        assert!(
            green.failed.is_empty(),
            "⚠ AND THE OTHER CONTROL: a tally that says nothing failed must still read as zero \
             failures, or a green platform could never retire a stale claim",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE SAME MATCH RULE, REACHED FROM THE OTHER DIRECTION** — register item 998.
    ///
    /// [`north_star::Suite::is_red`] asks *does any failure answer this claim*; [`Reported::accounts_for`]
    /// asks *does this claim answer that failure*. A second spelling of the match would let one
    /// direction hold a failure the other direction reported unclaimed, so both arms are asserted off
    /// ONE report here — including the module-prefix reading, which is what a `launcher::tests` claim
    /// needs to hold the tests inside it.
    #[test]
    fn a_claim_accounts_for_a_failure_exactly_as_the_suite_answers_that_claim() {
        let suite = ReportedFailures::of(A_MACOS_LOG).expect("that reads as a test log");
        let module = "-p sprag-gate --lib launcher::tests -- --exact";
        assert_eq!(
            suite.accounts_for(module, "launcher::tests::a_daemon_moved_past"),
            Ok(true),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 998: a MODULE-shaped claim must hold the tests inside it, or \
             970's two failures would both be reported as claimed by nobody",
        );
        assert_eq!(
            suite.accounts_for(module, "plugins::tests::a_loop_over_the_wire"),
            Ok(false),
            "⛔⛔ AND IT MUST NOT HOLD A TEST OUTSIDE IT — a reader answering true to everything \
             excuses every red there is, which is the quiet version of the hole this item is about",
        );
        // ⚠⚠ THE TWO DIRECTIONS AGREE, asserted rather than assumed: the claim the suite confirms
        // is the claim that accounts for the name, off the one report.
        assert_eq!(
            suite.is_red(module),
            Ok(true),
            "⚠ the control: that is the same claim the forward question confirms",
        );
        assert!(
            suite
                .accounts_for("-p sprag-host --lib", "rpc::tests::anything")
                .is_err_and(|why| why.contains("no test selection")),
            "⛔⛔⛔ REGISTER ITEM 998: a claim this reader cannot place must REFUSE — answered \
             false it invents an unclaimed red, answered true it excuses one, and it is the same \
             refusal the forward question already gives",
        );
    }

    /// ⛔⛔⛔ **AN ARGV THAT NAMES NO TEST IS *COULD NOT ASK*** — register item 973.
    ///
    /// `-p sprag-host --lib` selects a whole target, and a list of failed NAMES cannot answer it.
    /// Guessed either way it would be wrong in the direction that matters: `false` instructs a
    /// deletion, `true` confirms a claim nobody put.
    #[test]
    fn an_argv_naming_no_selection_is_a_question_this_reader_refuses() {
        assert_eq!(
            ReportedFailures::selected_by("-p sprag-gate --lib launcher::tests -- --exact"),
            Some("launcher::tests".to_owned()),
            "⚠ THE PREMISE: the selection is the one bare word before the `--`",
        );
        // ⚠⚠ PAST THE `--` IS THE HARNESS'S, and a reader that kept walking would take `--exact`
        // — or worse, a bare `--nocapture`'s neighbour — for a test name.
        assert_eq!(
            ReportedFailures::selected_by("-p sprag-host --lib -- --exact"),
            None,
            "⛔ a whole-target selection names no test, and past the `--` there are no names",
        );
        assert_eq!(
            ReportedFailures::selected_by("-p sprag-host --lib"),
            None,
            "⛔ and neither does one with no `--` at all",
        );
        let suite = ReportedFailures::of(A_MACOS_LOG).expect("that reads as a test log");
        assert!(
            suite
                .is_red("-p sprag-host --lib -- --exact")
                .is_err_and(|why| why.contains("no test selection")),
            "⛔⛔⛔ REGISTER ITEM 973: a question this reader cannot put is answered rather than \
             refused, and both answers are wrong — one deletes a live mark, the other confirms a \
             claim nobody asked about",
        );
    }

    /// How many tests a selection ran, in a run that ran SOME — the number every arm below that is
    /// not about emptiness needs, named once so no assertion carries a bare literal.
    const SOME_RAN: usize = 185;

    /// ⛔⛔⛔⛔⛔ **A MALFORMED CLAIM MUST NOT MANUFACTURE THE RED IT CLAIMS** — register item 949,
    /// and this round's own two `@red:` lines are what found it.
    ///
    /// # ⛔⛔⛔ What it costs, measured 2026-09-08
    ///
    /// `--exact` is a HARNESS flag and belongs past a bare `--`. Written as a cargo argument,
    /// `cargo test --quiet -p sprag-host --test cli <name> --exact` exits **1** with *"unexpected
    /// argument '--exact' found"* — nothing ran. The reader here answered `Ok(true)`, so
    /// `north-star` printed `reds 2 claimed, 2 standing: 951 952` about two tests that pass on this
    /// machine, and both items would have been admitted past the severity gate on it.
    ///
    /// ⇒ **A mark that makes itself true is worse than one that is false**, because the refutation
    /// item 843 built cannot catch it: there is nothing stale about a claim the instrument keeps
    /// confirming. So a code that is not the harness's own is a REFUSAL to judge.
    ///
    /// ⚠⚠ 101 STAYS RED, and that is not an oversight: cargo exits 101 for a failing test and for
    /// a compile error alike, and a tree that does not build is not a tree with no failing tests.
    /// That fold is deliberate and stated; the one this arm ends is between *the suite answered*
    /// and *cargo refused the question*.
    #[test]
    fn a_code_cargo_gives_for_refusing_the_arguments_is_not_a_red() {
        assert_eq!(
            verdict_of(Some(0), SOME_RAN, "-p sprag-gate --lib"),
            Ok(false),
            "a green selection is a claim the suite refutes",
        );
        assert_eq!(
            verdict_of(Some(101), SOME_RAN, "-p sprag-gate --lib"),
            Ok(true),
            "⚠ 101 is the harness's own failure — and a compile error's, deliberately",
        );
        let refused = verdict_of(Some(1), 0, "-p sprag-host --test cli name --exact").expect_err(
            "⛔ ITEM 949: exit 1 is cargo refusing the arguments, and calling that a RED lets a \
             typo in a `@red:` line confirm itself for ever",
        );
        assert!(
            refused.contains("refused the question") && refused.contains("--exact"),
            "⚠ the refusal must name the shape that causes it, or the next author writes the same \
             line: {refused}",
        );
        let killed = verdict_of(None, 0, "-p sprag-gate --lib")
            .expect_err("a signal decided nothing, so nothing is reported");
        assert!(
            killed.contains("killed"),
            "⚠ and *killed* is a different sentence from *refused*, because the remedy is: \
             {killed}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A SELECTION THAT MATCHED NOTHING IS NOT GREEN** — register item 971's ⑴, and the
    /// mirror of the arm above.
    ///
    /// # ⛔⛔⛔ The two typos have opposite consequences and both are refusals to judge
    ///
    /// Item 949 found a typo in the FLAGS: cargo refused the arguments, exited 1, and the reader
    /// called that a standing red — a mark that manufactured the fact it claimed. This is a typo in
    /// the NAME: nothing matches, the harness exits 0 having run nothing, and a green answer means
    /// *the claim is stale, remove the `@red:` line*. **So one typo confirms the claim for ever and
    /// the other deletes it**, and neither is the suite answering.
    ///
    /// ⚠⚠ The distinction is visible ONLY in the output, which is why [`tests_run`] exists: the
    /// exit code is 0 either way, and a run that genuinely passed everything is indistinguishable
    /// from one that ran nothing without reading `running N tests`.
    #[test]
    fn a_selection_that_ran_no_test_is_neither_red_nor_green() {
        let empty = verdict_of(
            Some(0),
            0,
            "-p sprag-gate --lib zzz_no_such_test -- --exact",
        )
        .expect_err(
            "⛔ ITEM 971: a selection that matched nothing has not been answered, and calling \
                 it green tells the next round to delete a claim nobody checked",
        );
        assert!(
            empty.contains("matched no test") && empty.contains("run nothing"),
            "⚠ the refusal must say WHAT happened, because the remedy is to fix the name rather \
             than the flags: {empty}",
        );
        // ⚠⚠ THE CONTRAST, so this is not an assertion about `Some(0)` in general: the same code
        // with tests behind it is still the suite refuting the claim.
        assert_eq!(
            verdict_of(Some(0), 1, "-p sprag-gate --lib one_test -- --exact"),
            Ok(false),
            "⚠⚠ ONE test that ran and passed IS a green answer — the emptiness is the defect, not \
             the exit code",
        );
    }

    /// ⚠⚠ **THE COUNT IS READ FROM THE HARNESS'S OWN BYTES** — register item 971, and the fixture
    /// is the output this round actually measured rather than a shape somebody remembered.
    ///
    /// ⛔ Register item 964 is the entry for a fixture written from memory: a gate whose fixtures
    /// are all hand-typed only knows the author's idea of the format. These two blocks are copied
    /// from runs of `cargo test --quiet` on 2026-09-08.
    ///
    /// ⚠ The multi-target block is what makes the SUM load-bearing — `-p sprag-host` alone reaches
    /// the lib and every integration test, so a reader that took the first line would call a
    /// selection empty because its first target had nothing to run.
    #[test]
    fn the_count_is_summed_over_every_harness_the_selection_reached() {
        let nothing = "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 \
                       measured; 185 filtered out; finished in 0.00s\n\n";
        assert_eq!(tests_run(nothing), 0, "the measured empty selection");
        let one_target = "\nrunning 185 tests\ntest a ... ok\n\ntest result: ok. 185 passed; 0 \
                          failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.70s\n\n";
        assert_eq!(
            tests_run(one_target),
            SOME_RAN,
            "one harness, {SOME_RAN} tests"
        );
        // Two targets, the FIRST of which matched nothing — the shape that breaks a reader taking
        // only the first line.
        let two_targets = "\nrunning 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 \
                           measured; 1050 filtered out; finished in 0.00s\n\n\nrunning 3 \
                           tests\n\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 \
                           filtered out; finished in 0.01s\n\n";
        assert_eq!(
            tests_run(two_targets),
            3,
            "⚠⚠ SUMMED: a selection whose first target ran nothing and whose second ran three has \
             had its question put. Taking the first line would refuse to judge a real answer",
        );
        // ⛔ And nothing that merely says the word counts — a test NAMED `running_...` prints on a
        // line of its own, and a compile error prints no `running` line at all.
        assert_eq!(
            tests_run("test running_a_verb_is_bounded ... ok\nerror: could not compile\n"),
            0,
            "⛔ only the harness's own `running N tests` line is a count",
        );
    }
}
