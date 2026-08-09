//! ⚠⚠ THE SUITE MUST NOT WRITE THE HOME OF WHOEVER RAN IT.
//!
//! Run this AFTER the suite, under the same three XDG variables the suite ran under. It exits 0
//! when they are still empty and 1 with the list when they are not.
//!
//! ```text
//! export XDG_CONFIG_HOME=$PWD/ambient/config XDG_DATA_HOME=$PWD/ambient/data \
//!        XDG_STATE_HOME=$PWD/ambient/state
//! mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME"
//! cargo test --workspace --exclude sprag-gui --no-fail-fast
//! cargo run -q -p sprag-gate --bin ambient-home-guard
//! ```
//!
//! Why this cannot be a test, and why it looks at the homes THEMSELVES rather than at a directory
//! above them, is in [`sprag_gate`]'s own docs.

fn main() -> std::process::ExitCode {
    let homes = match sprag_gate::ambient_homes() {
        Ok(homes) => homes,
        Err(unwatchable) => {
            eprintln!("ambient-home-guard: {unwatchable}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let mut wrote = Vec::new();
    for (var, home) in &homes {
        match sprag_gate::writes_under(home) {
            Ok(found) => wrote.extend(found.into_iter().map(|path| (*var, path))),
            Err(why) => {
                eprintln!(
                    "ambient-home-guard: {var} ({}) went unread: {why}",
                    home.display()
                );
                return std::process::ExitCode::FAILURE;
            }
        }
    }

    if wrote.is_empty() {
        println!(
            "the suite left all {} ambient homes untouched",
            sprag_gate::AMBIENT_HOMES.len()
        );
        return std::process::ExitCode::SUCCESS;
    }

    eprintln!(
        "ambient-home-guard: the test suite wrote the ambient XDG home. On a developer's machine \
         that is THEIR ~/.config, and a sibling test that reads it is green there and red \
         everywhere else. Give the call site below a seam."
    );
    for (var, path) in wrote {
        eprintln!("  {var}: {}", path.display());
    }
    std::process::ExitCode::FAILURE
}
