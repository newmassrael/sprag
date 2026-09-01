//! **HOW MUCH OF A PSEUDOTERMINAL NAMESPACE ONE PROCESS ASKED FOR** — register item 817, and a
//! number no test in the suite can report about the suite it is part of.
//!
//! # ⛔⛔⛔⛔⛔ Why this stands outside, like every gate in this crate
//!
//! The quantity is a PROCESS TOTAL. Inside the process it is whatever the threads running beside
//! the assertion have reached by then, and it is not final until the process that owns it has
//! exited — so an assertion inside the suite can only ever be `>=`, which is the shape that stays
//! green under the mutation it exists to catch. It is [`crate::sweep`]'s reason (register item
//! 585) applied to a different quantity: a run cannot be its own witness.
//!
//! # What it reads, and why the answer is a MAXIMUM and never a sum
//!
//! `sprag_terminal::pty` appends one `opened=<n> live=<n>` line to `<SPRAG_PTY_DEMAND>/<pid>` for
//! every pseudoterminal it takes, so a `cargo test` run leaves one file per process. A namespace is
//! exhausted by ONE process's demand — item 817's own note refuses the sum as the unit, because
//! processes that ran one after another never held anything at the same time. So this reports the
//! largest single process and says how many there were.
//!
//! ⚠ `opened` is a sequence number rather than a running re-read, so the LARGEST line in a file is
//! that process's demand. The last line is not: two threads that take at the same moment can write
//! in the other order.
//!
//! # ⛔⛔⛔⛔⛔ AN EMPTY DIRECTORY IS RED, AND SO IS AN ABSENT ONE
//!
//! This workspace's rule that an unclassified case is RED and not a pass, at the one place it
//! costs something. A suite that drives panes takes pseudoterminals; a reading of *no process
//! asked for one* therefore means the instrument was off, was pointed somewhere else, or could not
//! write — three different repairs, none of them *the demand was zero*. A gate that reported zero
//! there would be green forever the day the recording broke, which is the failure this repository
//! has already paid for in a blind ratchet.

use std::path::{Path, PathBuf};

/// One process's demand, as that process's own file recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessDemand {
    /// The file's name — the pid the process had while it ran. A NAME and not a number: it is read
    /// back for reporting only, and a pid that no longer resolves is still the right label.
    pub pid: String,
    /// The largest `opened` in that file: how many pseudoterminals that process had taken in all.
    pub opened: u64,
    /// How many lines the file carries — one per take, so a gap between this and `opened` says a
    /// write was lost rather than that a pseudoterminal was not opened.
    pub takes: usize,
}

/// Why no reading could be made. **Every arm is RED**, and not one of them is a demand of zero —
/// see this module's own note for why that distinction is the whole point.
#[derive(Debug)]
pub enum Unread {
    /// The directory the recordings were supposed to be in could not be listed.
    NoDirectory {
        /// Where the reading was attempted.
        dir: PathBuf,
        /// What the operating system said.
        why: std::io::Error,
    },
    /// The directory is there and no process left a file in it.
    NoProcess {
        /// Where the reading was attempted.
        dir: PathBuf,
    },
    /// A process's file could not be read.
    Unreadable {
        /// The file that refused.
        file: PathBuf,
        /// What the operating system said.
        why: std::io::Error,
    },
    /// A process's file exists and has no line in it — the recording was started and wrote nothing.
    Silent {
        /// The file that says nothing.
        file: PathBuf,
    },
    /// A line was not in the form the recorder writes.
    Unparsable {
        /// The file the line is in.
        file: PathBuf,
        /// The line itself, quoted so the reader can see what changed.
        line: String,
    },
    /// The file has FEWER lines than the count it reports reached — writes were lost, so every
    /// number read out of it is a floor rather than a reading.
    ///
    /// ⚠ The other direction is ORDINARY and not an error: a process can hold a second ledger of
    /// its own (a test that makes one numbers from 1 in the same file), so more lines than the peak
    /// is a thing this gate expects to see.
    Lost {
        /// The file that is short.
        file: PathBuf,
        /// The largest sequence number in it.
        opened: u64,
        /// How many lines it actually has.
        takes: usize,
    },
}

impl std::fmt::Display for Unread {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoDirectory { dir, why } => write!(
                out,
                "{} could not be listed ({why}) — this is NOT a demand of zero: the instrument was \
                 off, pointed elsewhere, or could not write, and each of those is a different \
                 repair (register item 817)",
                dir.display(),
            ),
            Self::NoProcess { dir } => write!(
                out,
                "{} holds no process's recording — a suite that drives panes takes \
                 pseudoterminals, so this says the recording did not happen and NOT that none were \
                 wanted (register item 817)",
                dir.display(),
            ),
            Self::Unreadable { file, why } => {
                write!(out, "{} could not be read ({why})", file.display())
            }
            Self::Silent { file } => write!(
                out,
                "{} was created and never written to — every take appends a line, so a file with \
                 none in it is a recorder that stopped",
                file.display(),
            ),
            Self::Lost {
                file,
                opened,
                takes,
            } => write!(
                out,
                "{} reports {opened} take(s) and carries {takes} line(s) — every take appends one, \
                 so writes were lost and this process's demand can only be read as AT LEAST \
                 {opened}",
                file.display(),
            ),
            Self::Unparsable { file, line } => write!(
                out,
                "{} carries a line this gate cannot read: {line:?}. The recorder writes \
                 `opened=<n> live=<n>`; an unclassified line is refused rather than skipped, \
                 because a skipped line is a demand this gate would under-report",
                file.display(),
            ),
        }
    }
}

/// Every process that recorded a demand in `dir`, largest first.
///
/// # Errors
///
/// Any of [`Unread`] — and none of them is a zero. A directory that cannot be listed, holds no
/// file, or holds a file this cannot read is a reading that was not made.
pub fn demands(dir: &Path) -> Result<Vec<ProcessDemand>, Unread> {
    let listing = std::fs::read_dir(dir).map_err(|why| Unread::NoDirectory {
        dir: dir.to_path_buf(),
        why,
    })?;

    let mut found: Vec<ProcessDemand> = Vec::new();
    for entry in listing {
        let entry = entry.map_err(|why| Unread::NoDirectory {
            dir: dir.to_path_buf(),
            why,
        })?;
        let file = entry.path();
        let body = std::fs::read_to_string(&file).map_err(|why| Unread::Unreadable {
            file: file.clone(),
            why,
        })?;
        found.push(one_process(&file, &body)?);
    }

    if found.is_empty() {
        return Err(Unread::NoProcess {
            dir: dir.to_path_buf(),
        });
    }
    // Largest first, and the pid breaks a tie so the order is the same on two runs that measured
    // the same thing — a report whose rows move for no reason is a report people stop diffing.
    found.sort_by(|left, right| {
        right
            .opened
            .cmp(&left.opened)
            .then_with(|| left.pid.cmp(&right.pid))
    });
    Ok(found)
}

/// One file's worth: the largest `opened` in it and how many lines it has.
fn one_process(file: &Path, body: &str) -> Result<ProcessDemand, Unread> {
    let mut opened = 0;
    let mut takes = 0;
    for line in body.lines() {
        takes += 1;
        opened = opened.max(taken_by(line).ok_or_else(|| Unread::Unparsable {
            file: file.to_path_buf(),
            line: line.to_owned(),
        })?);
    }
    if takes == 0 {
        return Err(Unread::Silent {
            file: file.to_path_buf(),
        });
    }
    if u64::try_from(takes).is_ok_and(|takes| takes < opened) {
        return Err(Unread::Lost {
            file: file.to_path_buf(),
            opened,
            takes,
        });
    }
    Ok(ProcessDemand {
        pid: file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        opened,
        takes,
    })
}

/// The `opened=<n>` a recorded line carries, or `None` if the line is not one this gate knows.
///
/// ⚠ The `live=` half is REQUIRED to be there even though nothing here reads its value: the line
/// this gate accepts is the line the recorder writes, and accepting a shorter one would let a
/// half-written format through unnoticed.
fn taken_by(line: &str) -> Option<u64> {
    let (opened, live) = line.split_once(' ')?;
    live.strip_prefix("live=")?.parse::<u64>().ok()?;
    opened.strip_prefix("opened=")?.parse::<u64>().ok()
}
