//! Per-PROJECT configuration: the commands a project declares for itself.
//!
//! A `.sprag.toml` at a project's root lists the commands that only make sense THERE — `cargo test`
//! in this repo, `npm run dev` in that one — and a client offers them beside its own built-in
//! commands. cmux reaches the same shape from the other direction ("define project-specific actions
//! in `cmux.json` that launch from the command palette"); this is sprag's, derived against sprag's
//! own seams rather than ported.
//!
//! ## Why the HOST owns the file
//!
//! The project is a function of a pane's LIVE working directory, and the host is the one process
//! that knows it (it reads `/proc/<pid>/cwd` for the durability ring already). Parsing here means
//! the `sprag` CLI, every attached GUI and the MCP surface all read ONE answer for a pane — a
//! display client that read the file itself would disagree with its sibling the moment the two ran
//! on different machines, and could not answer for a pane it does not display.
//!
//! ## The project is DISCOVERED, never registered
//!
//! [`discover`] walks up from the pane's cwd to the nearest ancestor holding a [`PROJECT_FILE`],
//! exactly as git finds its repository root. So the commands change as a shell `cd`s between
//! projects, with no "open this folder" step to perform and nothing to keep in sync. That is finer
//! than cmux's per-workspace config, which is bound to a window rather than to where the shell
//! actually is.
//!
//! ## Trust, stated plainly
//!
//! A `.sprag.toml` arrives with a repository, so it is UNTRUSTED input that names programs to run.
//! Three rules follow, and they are the whole security posture:
//!
//! 1. **Nothing here ever runs anything.** This module parses and validates; running is a separate,
//!    explicit user action. There is deliberately no "run on open" hook — the durability ring drew
//!    the same line for restored commands (`SPRAG_RESTORE_PROGRAMS` is an operator trust boundary,
//!    not a repository's).
//! 2. **A command is DECLARED as an argv, not as a shell string.** `run = ["cargo", "test"]` names
//!    exactly two words, so there is no word-splitting or expansion in what the config MEANS, and a
//!    project that genuinely wants a shell writes `["bash", "-lc", "…"]` and says so in its own file.
//!    Rendering that argv back into a command line for a pane's prompt does involve quoting, which is
//!    why it happens in exactly one place ([`ProjectAction::command_line`], over the crate-private
//!    `shellword` quoting SSOT the file-drop paste also uses) rather than at each call site.
//! 3. **The command is shown before it runs.** A client offering these must display what it would
//!    execute (the palette paints the command line beside the title), so "run the tests" cannot
//!    quietly be something else — and the line is pasted WITHOUT a newline, so the keystroke that
//!    runs it is still the user's.
//!
//! ## Bounds
//!
//! Every list is capped ([`MAX_ACTIONS`], [`MAX_ARGV`], [`MAX_NAME_BYTES`]) because the file is
//! untrusted: without a bound, a hostile or generated config could hand a client an arbitrarily long
//! list to paint and an arbitrarily long string to carry over the wire. Malformed input is REPORTED
//! ([`ProjectError`]), never silently dropped — a project whose config has a typo must be told, the
//! same reason the find bar reports a refused pattern instead of showing "no matches".

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file a project declares its commands in, at the project's root.
///
/// A dotfile, like `.editorconfig` / `.envrc`: it configures a TOOL for the project rather than
/// being the project's own manifest, and a repository root is scarce space.
pub const PROJECT_FILE: &str = ".sprag.toml";

/// The most actions one project may declare. Past this the file is refused rather than truncated —
/// a silently shortened list would leave a client claiming to offer a project's commands while
/// hiding some of them.
pub const MAX_ACTIONS: usize = 64;

/// The most words one action's argv may hold.
pub const MAX_ARGV: usize = 32;

/// The longest an action's name or title may be, in bytes.
pub const MAX_NAME_BYTES: usize = 80;

/// One command a project declares.
///
/// Serde in BOTH directions because this IS the wire form: the host serialises it into the mux
/// `project.<pane>` slot and a wire client deserialises the same type, so there is one definition of
/// what a project action is rather than a struct and a matching hand-built JSON object.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ProjectAction {
    /// The action's ADDRESS — what `sprag run <name>` names and what must be unique within a file.
    pub name: String,
    /// What a client shows for it. Defaults to [`name`](Self::name) when the file omits it, so a
    /// short address and a readable label are both available without demanding both.
    pub title: String,
    /// The argv to execute, verbatim and non-empty (see the module docs on why this is not a shell
    /// string). Public so a client can SHOW what it would run before running it.
    pub run: Vec<String>,
}

impl ProjectAction {
    /// The action as ONE shell command line — each argv word quoted so a shell reproduces exactly
    /// the words the config declared.
    ///
    /// This is how an action reaches a pane: a client PASTES this line at the pane's prompt. That is
    /// deliberate, and it is the whole answer to "where does a project command run":
    ///
    /// * the user's own shell runs it, so the output stays in their scrollback, the line enters their
    ///   history, and `Ctrl+C` works — none of which is true of a command run in a pane that
    ///   disappears the moment the command exits (sprag has no remain-on-exit yet, so a
    ///   spawn-a-pane-and-run target would make a fast command flash and vanish);
    /// * it is pasted WITHOUT a trailing newline, so the user still presses `Enter`. A file arriving
    ///   from a repository named this command; the person at the keyboard is the one who runs it.
    ///
    /// Quoted through the crate-private `shellword::shell_quote` SSOT, the same one the file-drop
    /// paste uses (de-linked deliberately: this is public documentation and that module is not) — so
    /// `run = ["git", "commit", "-m", "two words"]` becomes
    /// `git commit -m 'two words'` and a word containing `$(…)` is inert rather than expanded.
    #[must_use]
    pub fn command_line(&self) -> String {
        self.run
            .iter()
            .map(|word| crate::shellword::shell_quote(word))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A project and the commands it declares.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Project {
    /// The directory holding the [`PROJECT_FILE`] — the project's root, as discovered by walking up
    /// from a pane's cwd. Carried so a client can say WHICH project the commands came from (two
    /// panes in different repositories offer different lists, and the user needs to see which).
    pub root: PathBuf,
    /// The declared commands, in file order (the order a client lists them in).
    pub actions: Vec<ProjectAction>,
}

/// Why a project's config could not be used. Carried to the client as a message rather than swallowed
/// — see the module docs.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum ProjectError {
    /// The file was found but could not be read (permissions, a race with a delete).
    Unreadable(String),
    /// The file is not valid TOML. Carries the parser's own message, which names the line and column
    /// — the only part of the report that tells the user WHERE to look.
    Malformed(String),
    /// The file parsed but says something impossible (an empty argv, a duplicate name, a list past
    /// its cap).
    Invalid(String),
}

impl std::fmt::Display for ProjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable(why) => write!(f, "cannot read {PROJECT_FILE}: {why}"),
            Self::Malformed(why) => write!(f, "{PROJECT_FILE} is not valid TOML: {why}"),
            Self::Invalid(why) => write!(f, "{PROJECT_FILE}: {why}"),
        }
    }
}

/// The `.sprag.toml` file governing `from`, or `None` when no ancestor has one.
///
/// Walks `from` and its ancestors, nearest first, which is git's rule for finding a repository root:
/// a shell deep inside a project still gets that project's commands. The walk is LEXICAL and so
/// cannot loop; the cwd it is given already came from the OS with symlinks resolved.
#[must_use]
pub fn discover(from: &Path) -> Option<PathBuf> {
    from.ancestors()
        .map(|dir| dir.join(PROJECT_FILE))
        .find(|candidate| candidate.is_file())
}

/// The project governing `cwd`, or `None` when there is no [`PROJECT_FILE`] above it.
///
/// `Some(Err(_))` means a project WAS found and its config is unusable — a distinct outcome from
/// "this directory is not in a project", because only the first is something to tell the user about.
///
/// Reads the file on every call, deliberately: a client asks when it opens a palette or runs a
/// command, never per frame, and an edited config should take effect the next time it is asked
/// rather than at the next daemon restart. (The same on-demand rule the pane's `full_text` read
/// follows.)
pub fn load(cwd: &Path) -> Option<Result<Project, ProjectError>> {
    let file = discover(cwd)?;
    Some(read_project(&file))
}

/// Read + validate one config file into a [`Project`].
fn read_project(file: &Path) -> Result<Project, ProjectError> {
    let text = std::fs::read_to_string(file)
        .map_err(|error| ProjectError::Unreadable(error.to_string()))?;
    let parsed: ProjectFile =
        toml::from_str(&text).map_err(|error| ProjectError::Malformed(error.to_string()))?;
    // The file's own directory is the root. `discover` built the path by joining the file name onto
    // a directory, so the parent is always present.
    let root = file
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    Ok(Project {
        root,
        actions: validate(parsed.command)?,
    })
}

/// Turn the declared entries into validated actions, refusing the whole file on the first problem.
///
/// Refusing the FILE rather than skipping the bad entry is the deliberate choice: a client that
/// silently dropped one action would offer a list the project's author cannot explain, and the
/// author is the person who can fix it.
fn validate(declared: Vec<DeclaredAction>) -> Result<Vec<ProjectAction>, ProjectError> {
    if declared.len() > MAX_ACTIONS {
        return Err(ProjectError::Invalid(format!(
            "{} commands declared; at most {MAX_ACTIONS} are supported",
            declared.len()
        )));
    }
    let mut actions: Vec<ProjectAction> = Vec::with_capacity(declared.len());
    for entry in declared {
        let name = entry.name.trim().to_owned();
        if name.is_empty() {
            return Err(ProjectError::Invalid(
                "a command has an empty name".to_owned(),
            ));
        }
        if name.len() > MAX_NAME_BYTES {
            return Err(ProjectError::Invalid(format!(
                "the name {name:?} is longer than {MAX_NAME_BYTES} bytes"
            )));
        }
        // A duplicate name would make `sprag run <name>` ambiguous, and a palette would show two
        // rows a user cannot tell apart.
        if actions.iter().any(|existing| existing.name == name) {
            return Err(ProjectError::Invalid(format!(
                "two commands are both named {name:?}"
            )));
        }
        if entry.run.is_empty() {
            return Err(ProjectError::Invalid(format!(
                "the command {name:?} has an empty `run`"
            )));
        }
        if entry.run.len() > MAX_ARGV {
            return Err(ProjectError::Invalid(format!(
                "the command {name:?} has more than {MAX_ARGV} argv words"
            )));
        }
        // An empty PROGRAM (the first word) cannot be executed; an empty later word is a legitimate
        // argument, so only the program is checked.
        if entry.run[0].trim().is_empty() {
            return Err(ProjectError::Invalid(format!(
                "the command {name:?} names no program to run"
            )));
        }
        let title = match entry.title {
            Some(title) if !title.trim().is_empty() => title.trim().to_owned(),
            _ => name.clone(),
        };
        if title.len() > MAX_NAME_BYTES {
            return Err(ProjectError::Invalid(format!(
                "the title of {name:?} is longer than {MAX_NAME_BYTES} bytes"
            )));
        }
        actions.push(ProjectAction {
            name,
            title,
            run: entry.run,
        });
    }
    Ok(actions)
}

/// The file's shape, as written by a human.
///
/// `deny_unknown_fields` on both levels: a typo'd key (`tittle = "…"`) that silently did nothing
/// would be worse than an error, because the author would see their config accepted and their
/// intent ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectFile {
    /// `[[command]]` entries. Defaulted so a file that declares none is valid rather than an error —
    /// an empty project config is a reasonable placeholder to commit.
    #[serde(default)]
    command: Vec<DeclaredAction>,
}

/// One `[[command]]` entry as written.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeclaredAction {
    name: String,
    title: Option<String>,
    run: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `text` as a `.sprag.toml` in a fresh temporary project, returning its root.
    ///
    /// Uses the process id and a per-call counter for the directory name so parallel tests cannot
    /// collide (no temp-file crate in this workspace, and the daemon's own tests build paths the
    /// same way).
    fn project_with(text: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "sprag-project-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("deep/deeper")).expect("temp project");
        std::fs::write(root.join(PROJECT_FILE), text).expect("write config");
        root
    }

    #[test]
    fn an_action_becomes_a_shell_command_line_with_every_word_quoted_as_needed() {
        // The line a client pastes at a prompt. REVERT-PROOF: drop the quoting and the two-word
        // message becomes two arguments, which is a different command.
        let action = ProjectAction {
            name: "commit".to_owned(),
            title: "Commit".to_owned(),
            run: vec![
                "git".to_owned(),
                "commit".to_owned(),
                "-m".to_owned(),
                "two words".to_owned(),
            ],
        };
        assert_eq!(action.command_line(), "git commit -m 'two words'");

        let hostile = ProjectAction {
            name: "x".to_owned(),
            title: "x".to_owned(),
            run: vec!["echo".to_owned(), "$(reboot)".to_owned()],
        };
        assert_eq!(
            hostile.command_line(),
            "echo '$(reboot)'",
            "a substitution from an untrusted config is inert on the pasted line"
        );
    }

    #[test]
    fn a_project_is_found_from_a_subdirectory_the_way_git_finds_its_root() {
        let root = project_with("[[command]]\nname = \"test\"\nrun = [\"cargo\", \"test\"]\n");
        let deep = root.join("deep/deeper");

        let found = discover(&deep).expect("the config governs a subdirectory too");
        assert_eq!(found, root.join(PROJECT_FILE));

        let project = load(&deep).expect("a project is found").expect("it parses");
        assert_eq!(project.root, root, "the root is the config's directory");
        assert_eq!(project.actions.len(), 1);
        assert_eq!(project.actions[0].run, vec!["cargo", "test"]);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_directory_in_no_project_is_not_an_error() {
        // The distinction that matters: "not in a project" is None, not an Err — only a project
        // whose config is broken is worth telling the user about.
        let empty = std::env::temp_dir().join(format!("sprag-no-project-{}", std::process::id()));
        std::fs::create_dir_all(&empty).expect("temp dir");
        assert!(load(&empty).is_none());
        std::fs::remove_dir_all(&empty).ok();
    }

    #[test]
    fn a_title_defaults_to_the_name_and_survives_trimming() {
        let root = project_with(
            "[[command]]\nname = \"  test  \"\nrun = [\"cargo\", \"test\"]\n\
             [[command]]\nname = \"dev\"\ntitle = \"Start the dev server\"\nrun = [\"npm\", \"run\", \"dev\"]\n",
        );
        let project = load(&root).unwrap().expect("it parses");
        assert_eq!(project.actions[0].name, "test", "the name is trimmed");
        assert_eq!(
            project.actions[0].title, "test",
            "an omitted title falls back to the name"
        );
        assert_eq!(project.actions[1].title, "Start the dev server");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn broken_toml_is_reported_with_the_parsers_own_message() {
        let root = project_with("[[command]]\nname = \"test\"\nrun = [\n");
        let error = load(&root).unwrap().expect_err("broken TOML is refused");
        assert!(
            matches!(error, ProjectError::Malformed(ref why) if !why.is_empty()),
            "the parser's message is carried so the user learns WHERE: {error:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_typo_in_a_key_is_refused_rather_than_ignored() {
        // The whole point of `deny_unknown_fields`: a config whose author misspelled `title` must
        // hear about it instead of watching their label silently vanish.
        let root =
            project_with("[[command]]\nname = \"t\"\ntittle = \"Test\"\nrun = [\"cargo\"]\n");
        assert!(matches!(
            load(&root).unwrap(),
            Err(ProjectError::Malformed(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_impossible_command_is_refused_and_says_which_one() {
        for (config, expected) in [
            ("[[command]]\nname = \"\"\nrun = [\"x\"]\n", "empty name"),
            ("[[command]]\nname = \"a\"\nrun = []\n", "empty `run`"),
            (
                "[[command]]\nname = \"a\"\nrun = [\"  \"]\n",
                "names no program",
            ),
            (
                "[[command]]\nname = \"a\"\nrun = [\"x\"]\n[[command]]\nname = \"a\"\nrun = [\"y\"]\n",
                "both named",
            ),
        ] {
            let root = project_with(config);
            let error = load(&root).unwrap().expect_err("refused");
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "the report says what is wrong: {message:?} should mention {expected:?}"
            );
            std::fs::remove_dir_all(&root).ok();
        }
    }

    #[test]
    fn the_action_and_argv_caps_refuse_the_file_rather_than_truncating_it() {
        let many = (0..=MAX_ACTIONS)
            .map(|i| format!("[[command]]\nname = \"c{i}\"\nrun = [\"x\"]\n"))
            .collect::<String>();
        let root = project_with(&many);
        assert!(matches!(
            load(&root).unwrap(),
            Err(ProjectError::Invalid(_))
        ));
        std::fs::remove_dir_all(&root).ok();

        let words = (0..=MAX_ARGV)
            .map(|i| format!("\"w{i}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let root = project_with(&format!("[[command]]\nname = \"a\"\nrun = [{words}]\n"));
        assert!(matches!(
            load(&root).unwrap(),
            Err(ProjectError::Invalid(_))
        ));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_config_declaring_no_commands_is_valid() {
        // An empty file is a reasonable thing to commit as a placeholder; it must not read as broken.
        let root = project_with("# nothing yet\n");
        let project = load(&root).unwrap().expect("an empty config is valid");
        assert!(project.actions.is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_nearest_project_wins_over_an_outer_one() {
        // A repository inside a repository (a vendored checkout, a monorepo package) must get its
        // OWN commands, which is what makes the nearest-first walk the right rule.
        let outer = project_with("[[command]]\nname = \"outer\"\nrun = [\"x\"]\n");
        let inner = outer.join("deep");
        std::fs::write(
            inner.join(PROJECT_FILE),
            "[[command]]\nname = \"inner\"\nrun = [\"y\"]\n",
        )
        .expect("write inner config");

        let project = load(&inner.join("deeper")).unwrap().expect("it parses");
        assert_eq!(project.root, inner);
        assert_eq!(project.actions[0].name, "inner");
        std::fs::remove_dir_all(&outer).ok();
    }
}
