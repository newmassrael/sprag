//! 🎯🎯🎯🎯🎯 **THE SHIPPED CLASSIFIER REFUSES A PROPOSAL IT CANNOT PLACE** — register item 842.
//!
//! # ⛔⛔⛔⛔⛔ The hole, and why no test could see it
//!
//! `--admits` reads a proposal's subject as the FIRST register item it names. That is this ledger's
//! convention — *"항목 839 를 갚아라 — …"* — and **a convention is not a predicate**. A proposal
//! that CITES an admissible item and is about an inadmissible one, *"669 가 말한 얼굴을 837 에서
//! 갚는다"*, was answered `YES` about 669, and the enforcement this mode exists to be was silently
//! past. One direction only: citing an inadmissible item first costs a refusal, which is safe.
//!
//! Nothing in this workspace drove the `--admits` MODE at all before this file. `Reading::admits`
//! and `Reading::names` each had unit tests; what a caller runs — the argument handling, the
//! verdict word its reply opens with, the exit code — had none, and the composition is where the
//! hole was.
//!
//! # ⚠⚠⚠ Why the ledger is a fixture and not this repository's own
//!
//! The confusion only exists where the admissible set is NARROW, and this repository's register
//! currently has `critical 0` — so `admits` hands back the whole population and there is nothing to
//! be confused about. A gate pointed at the live ledger would be green today and green for the
//! wrong reason. The fixture is two open items, one of them critical, which is the shape the
//! enforcement is FOR.
//!
//! ⚠ The binary is still the real one, built from this tree by cargo. What is substituted is the
//! document it judges, which is an argument it already takes.

use sprag_gate::sources::workspace_root;
use std::path::PathBuf;
use std::process::Command;

/// A register with two open items — one critical, one not — so that exactly one of them is
/// admissible and a proposal can be genuinely ambiguous.
const LEDGER: &str = "\
# Ledger
## A. THE SHARPEST THINGS OPEN
@ns-unclassified: 0
@sev-unclassified: 0
@from-unclassified: 0
@paid-uncommitted: 1

900. **The one this register is holding a run to**
     @ns: open — it stops the loop dead
     @sev: critical — it stops the loop dead
     @from: none

895. **Open, and nobody called it critical**
     @ns: open — ordinary work
     @sev: ordinary — it waits
     @from: none

899. **PAID, and cited constantly**
     @ns: paid `deadbeef`
     @sev: ordinary — it was, once
     @from: none
";

/// Where this run may leave the fixture — [`sprag_scratch`] and never `std::env::temp_dir()`,
/// register item 794.
fn ledger_on_disk() -> PathBuf {
    let tail = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.subsec_nanos());
    let path = sprag_scratch::scratch_for("sprag-admits-fixture", &format!("{tail}"));
    std::fs::write(&path, LEDGER)
        .expect("the fixture register is written where this run may write");
    path
}

/// Put one proposal to the shipped classifier and hand back what it printed.
///
/// ⚠ Driven as `cargo run --bin`, which is what `debt_loop.scxml` deploys — the argument handling
/// and the exit code are parts a caller depends on and calling `Reading::admits` exercises neither.
fn asked(ledger: &std::path::Path, holding: &str, proposal: &str) -> (bool, String) {
    let run = Command::new(env!("CARGO"))
        .args([
            "run",
            "-q",
            "--locked",
            "-p",
            "sprag-gate",
            "--bin",
            "north-star",
            "--",
            "--admits",
        ])
        .arg(ledger)
        .arg(holding)
        .arg(proposal)
        .current_dir(workspace_root())
        .output()
        .expect("the classifier runs");
    (
        run.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr),
        ),
    )
}

/// ⛔⛔⛔⛔⛔ **A PROPOSAL THAT COULD BE ABOUT TWO THINGS IS REFUSED, AND ONE THAT COULD NOT IS
/// NOT** — item 842's ⑵ and ⑶ in one run, because either half alone is satisfied by a program
/// that answers the same thing to everything.
#[test]
fn a_proposal_that_could_be_about_two_things_is_refused() {
    let ledger = ledger_on_disk();
    let holding = "Take item 900";

    // ── THE CONTROL FIRST, so a total refuser cannot pass this file ──────────────────────────
    let (plain_ok, plain) = asked(&ledger, holding, "항목 900 을 갚아라");
    assert!(
        plain_ok && plain.starts_with("YES"),
        "⚠⚠ THE STAGING AND THE CONTROL: the admissible item, named alone, must still be admitted \
         — everything below is about a program that says YES to something. It said:\n{plain}",
    );

    // ── AND THE LEDGER'S ORDINARY VOICE: citing PAID work is not ambiguity ───────────────────
    let (cited_ok, cited) = asked(
        &ledger,
        holding,
        "항목 900 을 갚아라 — 899 도 같은 얼굴이다",
    );
    assert!(
        cited_ok && cited.starts_with("YES"),
        "⛔⛔⛔ ITEM 842: a paid item cannot be a smuggled subject — nobody proposes to pay what \
         is paid — and this register's own voice cites paid work in almost every milestone. A \
         rule that refused this would be switched off rather than fixed. It said:\n{cited}",
    );

    // ── THE ARM ──────────────────────────────────────────────────────────────────────────────
    let (muddled_ok, muddled) = asked(&ledger, holding, "900 이 말한 얼굴을 895 에서 갚는다");
    assert!(
        muddled_ok,
        "⚠ a refusal is a VERDICT and exits 0; a non-zero exit here would be the instrument \
         failing, which is a different fact:\n{muddled}",
    );
    assert!(
        muddled.starts_with("NO"),
        "⛔⛔⛔⛔⛔ ITEM 842: read by the convention this proposal is about 900, which this \
         register hands out. It is WRITTEN to be about 895, which it does not. Answering YES here \
         is the enforcement being walked past in silence — the one thing this mode exists to \
         stop. It said:\n{muddled}",
    );
    assert!(
        muddled.contains("895"),
        "⚠⚠ AND IT NAMES WHAT CONFUSED IT: a refusal that does not say which item sends the \
         reader to re-read their own sentence with no idea what to change:\n{muddled}",
    );

    let _ = std::fs::remove_file(&ledger);
}
