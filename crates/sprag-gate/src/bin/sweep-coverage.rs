//! ⚠⚠⚠⚠ **A SWEEP THAT DID NOT SWEEP EXITS NON-ZERO** — register item 585.
//!
//! Run this AFTER the sweep, handed the sweep's own logs. It exits 0 when every workspace member
//! ran something and 1 with the list of the ones that did not.
//!
//! ```text
//! cargo test --workspace --exclude sprag-gui --no-fail-fast > /tmp/leg1.log 2>&1
//! cargo test -p sprag-gui --bins --no-fail-fast             > /tmp/leg2.log 2>&1
//! cargo run -q -p sprag-gate --bin sweep-coverage -- /tmp/leg1.log /tmp/leg2.log
//! ```
//!
//! ⚠⚠ **BOTH LEGS, IN ONE CALL.** This workspace sweeps in two commands because the GPU crate
//! cannot run beside the rest, so judging one leg alone would report the excluded crate missing on
//! every honest sweep. See [`sprag_gate::sweep::unreported`] for why that matters more than it
//! sounds: a gate that is wrong on the common path is a gate people route around.
//!
//! Why the expectation is derived from the manifest instead of written down, and what the nineteen
//! suites that went unnoticed on 2026-08-22 were, are in [`sprag_gate::sweep`]'s own docs.

/// The flag a caller states its own `--exclude` with, so a deliberate gap is DECLARED.
///
/// ⚠⚠⚠⚠⚠ **IT MIRRORS A WORD ALREADY IN THE CALLER'S OTHER COMMAND**, which is the only reason it
/// is not the written-down number this gate exists to avoid. A sweep that says
/// `cargo test --workspace --exclude sprag-gui` is a sweep that knows it left `sprag-gui` out; what
/// it could not do until now is SAY so to anything that checks. CI's headless job really does
/// exclude the GPU crate — its tests run on another runner, in another job, whose log this side
/// will never hold — so without this the guard would refuse every honest CI run, and a gate that is
/// wrong on the common path is one people route around.
///
/// ⚠ A caller that excluded everything would pass, and that is visible in the command rather than
/// hidden here: the excluded crates are named on the line, beside the `--exclude` they mirror.
const EXCLUDING: &str = "--excluding";

fn main() -> std::process::ExitCode {
    let mut logs: Vec<std::path::PathBuf> = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        if arg == EXCLUDING {
            let Some(name) = args.next() else {
                eprintln!("sweep-coverage: {EXCLUDING} takes the crate the sweep left out");
                return std::process::ExitCode::FAILURE;
            };
            excluded.push(name.to_string_lossy().into_owned());
        } else {
            logs.push(arg.into());
        }
    }
    if logs.is_empty() {
        eprintln!(
            "sweep-coverage: name the sweep's log files — both legs, since this workspace sweeps \
             in two commands and either one alone is missing crates the other ran"
        );
        return std::process::ExitCode::FAILURE;
    }

    // ⚠ READ FROM THE MANIFEST BESIDE THIS BINARY'S SOURCE rather than from the current directory:
    // a gate invoked from somewhere else must not silently judge a different workspace, and an
    // empty member list would read as *nothing was expected*, which passes.
    let members = sprag_gate::sweep::members(include_str!("../../../../Cargo.toml"));
    if members.is_empty() {
        eprintln!(
            "sweep-coverage: the root manifest named no workspace members, so there is nothing to \
             hold this sweep to. A gate that cannot say what it expected must not pass"
        );
        return std::process::ExitCode::FAILURE;
    }

    let mut swept = String::new();
    for log in &logs {
        match std::fs::read_to_string(log) {
            Ok(text) => swept.push_str(&text),
            Err(why) => {
                // ⚠⚠ A LOG THAT WENT UNREAD IS A REFUSAL, never a leg with nothing in it: the
                // whole failure this exists for is *silence read as success*, and skipping an
                // unreadable log would reproduce it one layer up.
                eprintln!("sweep-coverage: {} went unread: {why}", log.display());
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    let missed: Vec<String> = sprag_gate::sweep::unreported(&members, &swept)
        .into_iter()
        .filter(|name| !excluded.contains(name))
        .collect();
    if missed.is_empty() {
        // ⚠ THE EXCLUSIONS ARE PRINTED, not merely honoured: a gap nobody sees on the passing run
        // is a gap that grows one crate at a time, and the line a person reads on a green sweep is
        // the only place it can be noticed.
        let owned = match excluded.as_slice() {
            [] => String::new(),
            named => format!(
                ", leaving out {} it was told to: {}",
                named.len(),
                named.join(", ")
            ),
        };
        println!(
            "the sweep reached all {} crates this workspace declares{owned}",
            members.len() - excluded.len(),
        );
        return std::process::ExitCode::SUCCESS;
    }

    eprintln!(
        "sweep-coverage: {} of {} crates ran NOTHING in this sweep — they are not red and not \
         green, and a round that read the exit code alone would report a sweep that never touched \
         them. The usual cause is a missing `--no-fail-fast`, which stops the run at the first \
         failing crate: {}",
        missed.len(),
        members.len(),
        missed.join(", "),
    );
    std::process::ExitCode::FAILURE
}
