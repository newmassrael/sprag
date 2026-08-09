//! The guard as CI runs it: a PROCESS, given three variables, answering with an exit code.
//!
//! `lib.rs` has the walk and the environment reading under test as functions. This drives the
//! BINARY, because that is the door — `ci.yml` runs `cargo run -p sprag-gate --bin
//! ambient-home-guard` and reads nothing but the status. A unit test on a row is not a test that
//! the surface offers it (R331), and the surface here is an exit code.
//!
//! ⚠ Every outcome the guard has is driven, INCLUDING the two that must fail. The guard it replaced
//! shipped able to fail and unable to pass; the opposite mistake — a guard that cannot fail — is
//! the worse one, because it reads exactly like a product that is behaving.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Three fresh homes under one root, named for the test that owns them.
fn homes(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
    let root = std::env::temp_dir().join(format!("sprag-gate-bin-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (config, data, state) = (root.join("config"), root.join("data"), root.join("state"));
    for home in [&config, &data, &state] {
        std::fs::create_dir_all(home).expect("create an ambient home");
    }
    (config, data, state)
}

/// Run the guard against three named homes. `None` leaves that variable UNSET, which is its own
/// case: the guard must refuse to judge rather than walk nothing and call it clean.
fn guard(config: Option<&Path>, data: Option<&Path>, state: Option<&Path>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ambient-home-guard"));
    // Cleared first: this test process inherits whatever the developer — or CI — set, and a guard
    // that read THOSE would be judging a directory this test knows nothing about. Which is, exactly,
    // the defect the guard exists to prevent, one level up.
    command.env_remove("XDG_CONFIG_HOME");
    command.env_remove("XDG_DATA_HOME");
    command.env_remove("XDG_STATE_HOME");
    for (var, home) in [
        ("XDG_CONFIG_HOME", config),
        ("XDG_DATA_HOME", data),
        ("XDG_STATE_HOME", state),
    ] {
        if let Some(home) = home {
            command.env(var, home);
        }
    }
    command.output().expect("run the guard")
}

#[test]
fn a_suite_that_wrote_nothing_passes() {
    let (config, data, state) = homes("clean");
    let run = guard(Some(&config), Some(&data), Some(&state));
    assert!(
        run.status.success(),
        "three empty homes must pass: {}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
}

/// The file R341 measured a test writing, in the place it wrote it.
#[test]
fn the_file_that_started_all_this_fails_the_guard() {
    let (config, data, state) = homes("written");
    std::fs::create_dir_all(config.join("sprag")).expect("the product's own directory");
    std::fs::write(config.join("sprag").join("config.toml"), "x = 1\n").expect("write it");

    let run = guard(Some(&config), Some(&data), Some(&state));
    assert!(!run.status.success(), "a written home must fail");
    let said = String::from_utf8_lossy(&run.stderr);
    assert!(
        said.contains("config.toml"),
        "and it must NAME what appeared, so the call site can be found: {said}",
    );
}

/// ⚠ The one the shell version got wrong in the other direction: pointed at a path nobody created,
/// it walked nothing, found nothing, and would have called the suite clean forever.
#[test]
fn a_home_that_is_not_there_fails_rather_than_passing_quietly() {
    let (config, data, state) = homes("absent");
    let run = guard(
        Some(&config.join("never-created")),
        Some(&data),
        Some(&state),
    );
    assert!(
        !run.status.success(),
        "a guard pointed at nothing must refuse to judge, not report clean",
    );
}

#[test]
fn a_variable_nobody_set_is_named() {
    let (config, data, _state) = homes("unset");
    let run = guard(Some(&config), Some(&data), None);
    assert!(!run.status.success(), "an unset home must fail");
    let said = String::from_utf8_lossy(&run.stderr);
    assert!(
        said.contains("XDG_STATE_HOME"),
        "and it must say WHICH one: {said}",
    );
}
