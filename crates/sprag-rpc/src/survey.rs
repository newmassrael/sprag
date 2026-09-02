//! WHICH daemons are running, and where — [`crate::endpoint`]'s question in the plural.
//!
//! [`HostEndpoint`](crate::endpoint::HostEndpoint) answers *which socket does THIS process talk
//! to*, from one env var and one well-known name. That is the right answer for a process that
//! already knows which daemon it belongs to. It is the wrong answer for a LAUNCHER, and register
//! item 825 is what that cost:
//!
//! > The owner pressed the dock icon six times over six days. Each press ran a launcher that asked
//! > the well-known socket, found the file a dead daemon had left behind, and said *"no server
//! > running at `/run/user/1000/sprag-host.sock`"* — a sentence that is TRUE and sends the reader
//! > to the wrong place, because the daemon was alive on `/run/user/1000/sprag-loop.sock` the whole
//! > time, serving six windows.
//!
//! # ⚠⚠⚠⚠⚠ Where the list comes from, which is the whole of the decision
//!
//! Not from a second hardcoded path. The register item is explicit that adding one would be the
//! same defect with a longer list, and this machine proves it: **four sockets, and the file name
//! does not say which is a daemon.** Measured 2026-09-02, one connect each:
//!
//! | socket | what answered |
//! |---|---|
//! | `sprag-host.sock` | nothing — the file outlived its daemon |
//! | `sprag-loop.sock` | **a daemon**, 6 windows |
//! | `sprag-gui.sock` | nothing |
//! | `sprag-loop-gui.sock` | something, and it refused `client/hello` |
//!
//! So the population is the RUNTIME DIRECTORY — the operating system's list, not this crate's —
//! narrowed to the names this product gives its own sockets ([`SOCKET_PREFIX`]), and **what makes
//! one of them a daemon is that it ANSWERS**. Nothing here judges by name; the name only decides
//! whose door it is polite to knock on.
//!
//! ⚠⚠ **THE PREFIX IS A POLITENESS BOUND, NOT THE TEST.** A runtime directory holds other
//! programs' sockets — `ssh-askpass-*.sock` and `vscode-git-*.sock` were beside sprag's on the
//! machine this was measured on — and connecting to a stranger's socket to see what it says is not
//! this product's business. The residue is stated rather than hidden: **a daemon whose operator
//! pointed `SPRAG_HOST_RPC_SOCK` outside this naming is not found**, and [`Survey::asked`] carries
//! the population so a reader can see what was NOT asked rather than reading silence as absence.
//!
//! # ⚠⚠⚠ Three answers, because there are three problems
//!
//! The launcher's own header had already written two of them down — *a daemon that is not running
//! and a daemon that is too old are different problems with different fixes* — and item 825 found
//! the third by measuring: **a daemon that is running somewhere else.** [`Answered`] is that set,
//! closed, so no socket can come back unclassified (register rule 6): a survey that could answer
//! *don't know* would be the silence this module exists to end, wearing a third face.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::client::HostConn;

/// The name every socket this product binds by default begins with — `sprag-host.sock`,
/// `sprag-gui.sock`, and by the same convention the ones an operator names for a second daemon
/// (`sprag-loop.sock`, `sprag-loop-gui.sock` on the machine item 825 was measured on).
///
/// ⚠ It bounds WHOSE DOOR IS KNOCKED ON and decides nothing about what is behind it. See this
/// module's header for the residue that leaves.
pub const SOCKET_PREFIX: &str = "sprag";

/// The extension a bound socket carries, so a `.log`, a `.lock` or a snapshot beside it is not
/// connected to.
pub const SOCKET_SUFFIX: &str = ".sock";

/// What one socket said when it was asked.
///
/// ⚠⚠⚠ **CLOSED, AND EVERY MEMBER IS A DIFFERENT REPAIR.** That is the point of the type rather
/// than a `bool`: a reader who is told *no* still has to know whether to start a daemon, rebuild
/// one, or look at another socket, and those were three different afternoons before this existed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answered {
    /// A daemon answered and this build's wire was accepted. The only answer a launcher may act
    /// on.
    Serving,
    /// Something is listening and the exchange failed — a daemon whose wire this build cannot
    /// speak, or a socket served by a program that is not a daemon at all. The sentence is the
    /// product's own, carried verbatim rather than summarised, because *too old* and *not a
    /// daemon* are told apart by it and by nothing else here.
    Refused(String),
    /// Nothing is listening. The file is what a daemon left behind when it died — measured on
    /// `sprag-host.sock`, dated eight days before the survey that found it.
    ///
    /// ⚠ **THE EXISTENCE OF THE FILE IS NOT THE EXISTENCE OF THE DAEMON**, which is the confusion
    /// item 825's six repeated notifications rested on.
    Silent,
}

impl Answered {
    /// The word a row is keyed by — stable, lowercase, one token, so a caller may branch on it
    /// without parsing the sentence beside it.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::Serving => "serving",
            Self::Refused(_) => "refused",
            Self::Silent => "silent",
        }
    }

    /// Whether a launcher may hand this socket to a display client.
    #[must_use]
    pub const fn is_serving(&self) -> bool {
        matches!(self, Self::Serving)
    }
}

impl fmt::Display for Answered {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Serving => write!(
                out,
                "serving — a daemon answered and speaks this build's wire"
            ),
            Self::Refused(why) => write!(
                out,
                "refused — something is listening and would not talk: {why}"
            ),
            Self::Silent => write!(
                out,
                "silent — nothing is listening; the file is what a daemon left behind"
            ),
        }
    }
}

/// One socket and what it said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asked {
    /// The socket that was connected to.
    pub path: PathBuf,
    /// What it answered.
    pub answer: Answered,
}

/// Every socket a survey knocked on, and where it looked.
///
/// ⚠⚠ **THE DIRECTORY AND THE PATTERN TRAVEL WITH THE ROWS**, so a reader who finds no daemon
/// learns what was searched rather than being told a bare *none*. That is
/// [`HostEndpoint`](crate::endpoint::HostEndpoint)'s own rule one level up — a path alone drops the
/// part an operator needs when it is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Survey {
    /// Where the sockets were looked for.
    pub under: PathBuf,
    /// Every socket found there, with its answer, in path order.
    pub asked: Vec<Asked>,
}

impl Survey {
    /// The sockets a daemon answered on, in path order — what a launcher acts on.
    #[must_use]
    pub fn serving(&self) -> Vec<&Path> {
        self.asked
            .iter()
            .filter(|row| row.answer.is_serving())
            .map(|row| row.path.as_path())
            .collect()
    }

    /// The pattern the population was drawn by, as a person would type it.
    #[must_use]
    pub fn pattern() -> String {
        format!("{SOCKET_PREFIX}*{SOCKET_SUFFIX}")
    }
}

/// Every socket under `dir` this product could be listening on, in path order.
///
/// ⚠ A directory that cannot be read answers an EMPTY list rather than an error: the caller's
/// question is *which daemons are running*, and *the runtime directory is gone* is answered by the
/// empty survey plus the directory it names. A `Result` here would make every caller decide the
/// same thing twice.
#[must_use]
pub fn candidates(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(SOCKET_PREFIX) && name.ends_with(SOCKET_SUFFIX)
                })
        })
        .collect();
    // Sorted so two surveys of one directory read the same, and so a launcher that takes the first
    // serving socket takes the same one twice.
    found.sort();
    found
}

/// Knock on one socket and report what answered.
///
/// ⚠⚠⚠ **CONNECT AND HANDSHAKE, BECAUSE EITHER ALONE ANSWERS THE WRONG QUESTION.** A connect that
/// succeeds says only that something is bound — measured on `sprag-loop-gui.sock`, which accepts
/// and then refuses `client/hello`. A handshake is what separates *a daemon* from *a program that
/// happens to own this path*, and it is the same exchange every real client makes, so a socket this
/// says is serving is one a client can really use.
pub fn ask(path: &Path, client_id: &str, timeout: Duration) -> Answered {
    let Ok(mut conn) = HostConn::connect(path, timeout) else {
        return Answered::Silent;
    };
    // Set BEFORE the handshake, so a socket that accepts and then says nothing is bounded by this
    // caller's patience rather than by nobody's — the shape a survey must not have, since it is run
    // from a dock click that has no terminal to interrupt.
    if let Err(why) = conn.set_read_deadline(Some(timeout)) {
        return Answered::Refused(why.to_string());
    }
    match conn.handshake(client_id) {
        Ok(()) => Answered::Serving,
        Err(why) => Answered::Refused(why.to_string()),
    }
}

/// Ask every candidate under `dir`.
#[must_use]
pub fn survey(dir: &Path, client_id: &str, timeout: Duration) -> Survey {
    Survey {
        under: dir.to_path_buf(),
        asked: candidates(dir)
            .into_iter()
            .map(|path| Asked {
                answer: ask(&path, client_id, timeout),
                path,
            })
            .collect(),
    }
}

/// Where a survey looks when nobody says otherwise — the runtime directory the daemon's own policy
/// binds under, so discovery and binding cannot disagree about the place.
#[must_use]
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(sprag_scratch::scratch_root)
}

#[cfg(test)]
mod tests {
    use std::os::unix::net::UnixListener;

    use super::*;

    /// A directory of this test's own, removed by the caller.
    fn scratch(name: &str) -> PathBuf {
        let dir = sprag_scratch::scratch_root().join(format!(
            "sprag-survey-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos()),
        ));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// ⛔⛔⛔⛔⛔ **THE POPULATION IS THE DIRECTORY, NARROWED BY NAME AND BY NOTHING ELSE** —
    /// register item 825.
    ///
    /// The defect this closes is a launcher that knew ONE path. What replaces it must not be a
    /// launcher that knows two: the list has to come from the machine. So this stages a directory
    /// holding what a real runtime directory holds — this product's sockets, another program's,
    /// and files beside a socket that are not one — and asks which are knocked on.
    ///
    /// ⚠⚠ **THE STRANGER'S SOCKET IS THE HALF THAT MATTERS.** `ssh-askpass-*.sock` and
    /// `vscode-git-*.sock` sat beside sprag's on the machine item 825 measured, and a survey that
    /// connected to them would be this product opening other programs' doors to see what they say.
    #[test]
    fn the_population_is_this_products_sockets_in_the_directory_and_nothing_elses() {
        let dir = scratch("population");
        for name in [
            "sprag-host.sock",
            "sprag-loop.sock",
            "sprag-loop-gui.sock",
            "ssh-askpass-abc.sock",
            "vscode-git-9f.sock",
            "sprag-host.lock",
            "sprag-loop.log",
        ] {
            std::fs::write(dir.join(name), b"").expect("a file in the scratch directory");
        }

        let found: Vec<String> = candidates(&dir)
            .iter()
            .filter_map(|path| path.file_name().and_then(|name| name.to_str()))
            .map(ToOwned::to_owned)
            .collect();
        assert_eq!(
            found,
            vec![
                "sprag-host.sock".to_owned(),
                "sprag-loop-gui.sock".to_owned(),
                "sprag-loop.sock".to_owned(),
            ],
            "⛔ ITEM 825: the population must be every socket THIS PRODUCT could be listening on \
             and no other program's. Too few is the defect this module exists to end — a daemon on \
             a socket nobody asked about, and a person told there is none. Too many is this \
             product connecting to a stranger's socket to see what it says, which is not its \
             business and is what the name bound is for. ⚠ A `.lock` and a `.log` sit beside every \
             socket a daemon binds, so the suffix is load-bearing, not decoration",
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ⛔⛔⛔⛔⛔ **A FILE THAT OUTLIVED ITS DAEMON AND A SOCKET SOMETHING ELSE OWNS ARE DIFFERENT
    /// ANSWERS** — register item 825, and the pair the old launcher collapsed into one sentence.
    ///
    /// # ⚠⚠⚠ Why both halves are staged for real
    ///
    /// `Silent` was the answer the owner got six times, and it was TRUE — `sprag-host.sock` had
    /// been a file with no daemon behind it since 2026-08-25. `Refused` is what the machine
    /// answered on `sprag-loop-gui.sock` in the same survey: something accepted the connection and
    /// would not talk. A build that returned one word for both would say *no daemon here* about a
    /// socket that has a program on it, which is the third problem item 825 found.
    ///
    /// ⚠ The serving arm is not staged HERE, and that is deliberate rather than missing: a real
    /// daemon is `sprag-host`'s to boot, and the CLI's own gate drives this survey against one.
    #[test]
    fn a_socket_nobody_serves_and_a_socket_somebody_else_serves_answer_differently() {
        let dir = scratch("answers");
        let dead = dir.join("sprag-dead.sock");
        std::fs::write(&dead, b"").expect("a file where a socket used to be");
        let taken = dir.join("sprag-taken.sock");
        let _listener = UnixListener::bind(&taken).expect("a socket this test owns");

        let survey = survey(&dir, "gate", Duration::from_millis(500));
        let words: Vec<(&str, &str)> = survey
            .asked
            .iter()
            .filter_map(|row| Some((row.path.file_name()?.to_str()?, row.answer.word())))
            .collect();
        assert_eq!(
            words,
            vec![
                ("sprag-dead.sock", "silent"),
                ("sprag-taken.sock", "refused")
            ],
            "⛔ ITEM 825: a file a dead daemon left behind and a socket a live program owns must \
             not answer the same word. The first is *start a daemon*; the second is *this is not a \
             daemon, or not one this build can speak to* — and the owner was given the first \
             sentence about a machine in the second state, six times",
        );
        assert!(
            survey.serving().is_empty(),
            "⚠⚠ AND NEITHER IS SERVING. `serving()` is what a launcher acts on, so a build that \
             counted a listener that refused the handshake would hand a display client a socket it \
             cannot use — the panic-with-no-window this launcher was written to prevent",
        );
        drop(_listener);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// ⚠⚠ **A SURVEY THAT FOUND NOTHING SAYS WHERE IT LOOKED** — the residue this module's header
    /// states, held rather than described.
    ///
    /// A launcher that reports *no daemon* without naming the directory and the pattern leaves its
    /// reader unable to tell *there is none* from *it did not look where mine is*, which is exactly
    /// the ambiguity item 825's own notification had.
    #[test]
    fn an_empty_survey_still_names_the_directory_and_the_pattern() {
        let dir = scratch("empty");
        let survey = survey(&dir, "gate", Duration::from_millis(200));
        assert!(survey.asked.is_empty() && survey.serving().is_empty());
        assert_eq!(survey.under, dir, "⚠ the directory travels with the rows");
        assert_eq!(
            Survey::pattern(),
            "sprag*.sock",
            "⚠⚠ and so does the pattern — a reader who keeps a daemon on a socket named outside \
             it must be able to SEE that it was never asked about, rather than reading an empty \
             survey as an empty machine",
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
