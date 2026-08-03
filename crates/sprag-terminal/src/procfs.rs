//! The ONE parse of a process's `/proc/<pid>/stat` line.
//!
//! Three facts in this crate come off that single line — a process's PARENT (the subtree walk
//! behind a session's listening ports), its process GROUP (the foreground-job enumeration behind
//! what a pane is running) and its terminal's FOREGROUND group (a pane's live job) — and until this
//! module existed two of them were parsed by two functions that had come to disagree.
//!
//! # Why one parser, stated once
//!
//! The layout is `pid (comm) state ppid pgrp session tty_nr tpgid ...`, and **field 2 is the
//! executable name in parentheses**. It may contain spaces AND parentheses, so every field index
//! past it has to be counted from the LAST `)` rather than from the first space — splitting naively
//! is the classic way to read this file wrong, and it misparses for any process whose name contains
//! a space, which is a rename away from being somebody's bug.
//!
//! Worse, the kernel caps `comm` at 15 BYTES, which can truncate a multibyte name mid-codepoint. So
//! the line is not necessarily valid UTF-8, and the two pre-existing readers differed exactly
//! there: [`crate::ports`]'s worked on bytes and said why, while [`crate::pane_pty`]'s went through
//! `read_to_string` and therefore answered `None` for precisely the process the other one had been
//! written to survive. One parse, on bytes, removes the class rather than the instance.
//!
//! Linux-only, like every other `/proc` reader here: off Linux the callers report an honest absence
//! rather than a guess, so there is nothing for this module to do and it is not compiled.

/// The fields of one `/proc/<pid>/stat` line that this crate reads.
///
/// A struct rather than three parsing functions because the hard part — finding where the fields
/// begin — is shared, and a caller that wants two of them should pay for that once. Every field is
/// already decoded: the numbers are plain ASCII after the last `)`, and [`comm`](Self::comm) is
/// lossy-decoded because it is the one part that can be invalid UTF-8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Stat {
    /// Field 2 — the executable name the KERNEL holds for this process, without its parentheses.
    ///
    /// Capped at 15 bytes by the kernel and settable by the process itself (`prctl(PR_SET_NAME)`),
    /// so it is a name to show a person, never an identity to match on. Decoded lossily: a name
    /// truncated mid-codepoint must still produce a row rather than drop the process.
    pub(crate) comm: String,
    /// Field 4 — the parent process id.
    pub(crate) ppid: u32,
    /// Field 5 — the process GROUP this process belongs to. A shell's job is a group, which is what
    /// makes this the key the foreground-job enumeration indexes by.
    pub(crate) pgrp: u32,
    /// Field 8 — the FOREGROUND process group of this process's controlling terminal, or `None`
    /// when it has none.
    ///
    /// The same number `tcgetpgrp` would return for that terminal — measured equal at a prompt,
    /// under a foreground job, and after that job was killed — and reachable without a master fd.
    /// The kernel writes `-1` for "no controlling terminal"; a caller asking which job owns a pane
    /// deserves an absence, not a sentinel it must know about.
    pub(crate) tpgid: Option<u32>,
}

impl Stat {
    /// Parse one `/proc/<pid>/stat` line. `None` if it does not have the shape at all (no `)`, or
    /// too few fields behind it) — never a partial row, because every caller here would rather skip
    /// a process than attribute a wrong parent or group to it.
    ///
    /// Takes BYTES: see the module docs for why a `&str` signature would be the bug.
    pub(crate) fn parse(stat: &[u8]) -> Option<Self> {
        let close = stat.iter().rposition(|&b| b == b')')?;
        let open = stat.iter().position(|&b| b == b'(')?;
        // `open < close` holds for any real line; an input where it does not is malformed, and the
        // range would panic rather than answering `None`, so it is checked instead of assumed.
        let comm = String::from_utf8_lossy(stat.get(open + 1..close)?).into_owned();
        // Everything past the last `)` is `state ppid pgrp session tty_nr tpgid ...` — plain ASCII,
        // so it is safe to decode, and the field indices below are counted from there.
        let mut fields = std::str::from_utf8(stat.get(close + 1..)?)
            .ok()?
            .split_whitespace()
            .skip(1);
        let ppid = fields.next()?.parse().ok()?;
        let pgrp = fields.next()?.parse().ok()?;
        // `session` and `tty_nr` sit between `pgrp` and `tpgid` and nothing here reads them.
        let tpgid = fields
            .nth(2)?
            .parse::<i32>()
            .ok()
            .and_then(|tpgid| u32::try_from(tpgid).ok());
        Some(Self {
            comm,
            ppid,
            pgrp,
            tpgid,
        })
    }
}

/// Read and parse one process's `/proc/<pid>/stat`. `None` when the process is gone (the common
/// case, and a race no caller can avoid) or the line does not parse.
pub(crate) fn stat(pid: u32) -> Option<Stat> {
    Stat::parse(&std::fs::read(format!("/proc/{pid}/stat")).ok()?)
}

/// EVERY process on the box, as `(pid, its stat)`, from one pass over `/proc`.
///
/// The single walk this crate's whole-machine questions are built from — a `pid → children` map for
/// a session's listening ports, a `pgrp → members` map for a pane's foreground job. Each caller
/// INDEXES the rows for its own question rather than this module guessing which index is wanted,
/// so adding a third question adds no walk and no parser.
///
/// Robust by construction: `stat` always exists for a live process, unlike the
/// `CONFIG_PROC_CHILDREN`-gated `/proc/<pid>/task/*/children` — so every walk built on this works
/// on any kernel. The cost of the full pass is the price of that robustness.
///
/// A process that exits mid-walk simply does not appear; there is no consistent snapshot of `/proc`
/// to be had, and a caller that needed one would be asking the wrong question of the wrong OS.
pub(crate) fn walk() -> Vec<(u32, Stat)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            // Only NUMERIC `/proc` entries are pids (`net`, `self`, `sys`, ... are not).
            let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
            Some((
                pid,
                Stat::parse(&std::fs::read(entry.path().join("stat")).ok()?)?,
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole reason this parse counts from the LAST `)`: a `comm` full of spaces and
    /// parentheses (a real hazard — a process can name itself `(my proc)`) must not shift any field
    /// index, and the name itself comes back whole.
    #[test]
    fn a_parenthesised_spacey_comm_shifts_no_field() {
        let stat = Stat::parse(b"1234 (odd (name) :)) S 42 77 77 34816 99 4194304").unwrap();
        assert_eq!(
            stat.comm, "odd (name) :)",
            "the name between the outer parens"
        );
        assert_eq!(stat.ppid, 42, "field 4, counted from the last ')'");
        assert_eq!(stat.pgrp, 77, "field 5");
        assert_eq!(stat.tpgid, Some(99), "field 8, two past pgrp");
    }

    /// The ordinary line, so the field offsets are pinned by something without the hazard too.
    #[test]
    fn a_plain_line_reads_every_field() {
        let stat = Stat::parse(b"7 (bash) S 1 7 7 34817 7 4194304 0 0").unwrap();
        assert_eq!(
            (stat.comm.as_str(), stat.ppid, stat.pgrp, stat.tpgid),
            ("bash", 1, 7, Some(7)),
        );
    }

    /// A `comm` truncated mid-codepoint is invalid UTF-8, and the process must still produce a row.
    ///
    /// This is the case the two pre-R290 parsers disagreed about: `ports`' byte parse survived it
    /// and `pane_pty`'s `read_to_string` did not, so a pane whose child had such a name reported no
    /// foreground group at all. Pinned here because a `&str` signature would silently restore it.
    #[test]
    fn a_comm_that_is_not_utf8_still_yields_every_number() {
        let stat = Stat::parse(b"9 (odd\xff name) S 3 9 9 34816 9 0").unwrap();
        assert_eq!(stat.ppid, 3);
        assert_eq!(stat.pgrp, 9);
        assert_eq!(stat.tpgid, Some(9));
        assert!(
            stat.comm.contains('\u{fffd}'),
            "the undecodable byte became a replacement character rather than dropping the row: {:?}",
            stat.comm,
        );
    }

    /// `-1` is the kernel's "no controlling terminal", and it leaves as an absence rather than as a
    /// sentinel every caller would have to know about.
    #[test]
    fn no_controlling_terminal_is_an_absence() {
        let stat = Stat::parse(b"3 (init) S 1 3 3 0 -1 0").unwrap();
        assert_eq!(stat.tpgid, None);
        assert_eq!(stat.pgrp, 3, "the fields either side still read");
    }

    /// Nothing with the wrong shape produces a partial row.
    #[test]
    fn a_line_without_the_shape_is_refused_whole() {
        assert_eq!(Stat::parse(b"garbage with no parens"), None);
        assert_eq!(
            Stat::parse(b"5 (sh) S 1"),
            None,
            "too few fields past the name is refused rather than defaulted",
        );
        assert_eq!(Stat::parse(b""), None);
    }
}
