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
//! there: the crate's `ports` module's worked on bytes and said why, while [`crate::pane_pty`]'s went through
//! `read_to_string` and therefore answered `None` for precisely the process the other one had been
//! written to survive. One parse, on bytes, removes the class rather than the instance.
//!
//! # What is per-platform here, and what is not
//!
//! The PARSE is a byte parser over a line's layout and has no platform in it at all, so it is
//! compiled — and tested — everywhere. Only `stat` and `walk` touch an OS: `/proc` on Linux,
//! `proc_pidinfo` and `proc_listpids` on macOS, and elsewhere the honest absence every caller here
//! already handles — no such process, and no processes on the box.
//!
//! ⚠ **The macOS half was that absence until R343, and nothing said so out loud.** `stat` answered
//! `None` there, so a pane could not name its foreground job, an agent report bound to a process
//! group was never released, and `sprag processes` had nothing to print — three user-facing
//! silences from one unimplemented reader, none of which mentions this module. The first macOS run
//! of this suite is what named it.
//!
//! That line was in the wrong place until the first non-Linux build ran. Gating the whole MODULE
//! made `Stat` — a plain struct that names four fields of a line — vanish on macOS, and
//! [`crate::processes`] holds one in a map whose type is not itself conditional, so a portable data
//! structure failed to compile for want of a `/proc`. **A platform gate belongs on the syscall, not
//! on the type the syscall fills in**; put it on the type and every caller inherits a `cfg` it has
//! no opinion about. The five parser tests below now also run on every platform CI builds, where
//! before they ran on one.

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
    ///
    /// ⚠ COMPILED AND TESTED EVERYWHERE, CALLED ON LINUX. It is the Linux TRANSPORT's parser —
    /// macOS fills the same struct from a kernel struct and needs no parse — and the `allow` states
    /// that rather than a `cfg` deleting it off Linux, for the reason this module's own docs give:
    /// gating the parse by platform is how the type came to vanish on macOS in the first place.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
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
#[cfg(target_os = "linux")]
pub(crate) fn stat(pid: u32) -> Option<Stat> {
    Stat::parse(&std::fs::read(format!("/proc/{pid}/stat")).ok()?)
}

/// macOS: the same four facts, from `proc_pidinfo(PROC_PIDTBSDINFO)`.
///
/// There is no line to parse here — the kernel fills in a struct — so this builds a [`Stat`]
/// directly rather than going through [`Stat::parse`]. The struct is the portable shape; the parse
/// is the Linux TRANSPORT's problem, and that split is the reason this module gates its syscalls
/// instead of its type.
///
/// The four fields line up exactly, which is why this is a port and not a redesign: `pbi_comm` is
/// the kernel's short name (capped at `MAXCOMLEN`, as Linux caps `comm` at 15 — a name to show a
/// person, never an identity), `pbi_ppid` is the parent, `pbi_pgid` the process group, and
/// **`e_tpgid` is the controlling terminal's foreground group** — the same fact Linux puts in field
/// 8 and the one a pane's live job is read from.
///
/// # The absence rule, and it differs from Linux's
///
/// Linux writes `-1` into `tpgid` for *no controlling terminal*. Here the terminal is named
/// separately by `e_tdev`, which is `NODEV` (all bits set) when there is none — so that, rather
/// than a sentinel in the group field, is what an absence is keyed on. The extra `!= 0` guard costs
/// nothing and covers the other spelling: group 0 is not a job anybody can name.
#[cfg(target_os = "macos")]
pub(crate) fn stat(pid: u32) -> Option<Stat> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let want = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: `info` is a live allocation of exactly the type this flavour writes, and the size
    // passed is its own. A `pid` that has exited is an ordinary short answer, not unsound.
    let got = unsafe {
        libc::proc_pidinfo(
            i32::try_from(pid).ok()?,
            libc::PROC_PIDTBSDINFO,
            0,
            std::ptr::from_mut(&mut info).cast(),
            libc::c_int::try_from(want).ok()?,
        )
    };
    if usize::try_from(got).ok()? != want {
        return None;
    }
    let comm: Vec<u8> = info
        .pbi_comm
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    Some(Stat {
        comm: String::from_utf8_lossy(&comm).into_owned(),
        ppid: info.pbi_ppid,
        pgrp: info.pbi_pgid,
        tpgid: (info.e_tdev != u32::MAX && info.e_tpgid != 0).then_some(info.e_tpgid),
    })
}

/// Neither `/proc` nor `libproc`, so no process has facts to read — the same absence a process that
/// has already exited produces, which every caller here already handles.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn stat(_pid: u32) -> Option<Stat> {
    None
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
#[cfg(target_os = "linux")]
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

/// macOS: the pid list from `proc_listpids`, then [`stat`] for each.
///
/// N+1 calls where Linux's walk is a directory read plus N file reads, so the SHAPE of the cost is
/// the same and neither is a snapshot: a process that exits between the listing and its own read
/// simply does not appear, exactly as on Linux, and every index built from this is short rather
/// than wrong.
///
/// ⚠ `PROC_ALL_PIDS` is spelled here because libc does not export it. It is `1`, from
/// `<sys/proc_info.h>`, and a wrong value would make `proc_listpids` answer nothing or refuse —
/// which is why the walk's test asserts that the list contains THIS process rather than merely that
/// the call returned. A constant nobody can check is a constant that should not be written down.
#[cfg(target_os = "macos")]
pub(crate) fn walk() -> Vec<(u32, Stat)> {
    /// `<sys/proc_info.h>`'s `PROC_ALL_PIDS`.
    const PROC_ALL_PIDS: u32 = 1;

    // Asked for its size FIRST (a null buffer answers the bytes needed), because the count moves
    // between the two calls on a live machine — so the buffer is over-allocated on purpose and the
    // second answer, not the first, decides how much of it is real.
    // SAFETY: a null buffer of size 0 is the documented way to ask for the size.
    let bytes = unsafe { libc::proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
    let Ok(bytes) = usize::try_from(bytes) else {
        return Vec::new();
    };
    let slack = bytes / std::mem::size_of::<u32>() + 64;
    let mut pids = vec![0u32; slack];
    // SAFETY: the buffer is `slack` u32s long and its byte length is what is handed over.
    let filled = unsafe {
        libc::proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr().cast(),
            libc::c_int::try_from(std::mem::size_of_val(pids.as_slice()))
                .unwrap_or(libc::c_int::MAX),
        )
    };
    let Ok(filled) = usize::try_from(filled) else {
        return Vec::new();
    };
    pids.truncate(filled / std::mem::size_of::<u32>());
    pids.into_iter()
        // pid 0 is the kernel, which `proc_listpids` pads a short list with.
        .filter(|&pid| pid != 0)
        .filter_map(|pid| Some((pid, stat(pid)?)))
        .collect()
}

/// Neither `/proc` nor `libproc`, so the walk finds no processes — which is the same answer a Linux
/// box whose `/proc` could not be opened gives, and every index built from this is empty rather
/// than wrong.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn walk() -> Vec<(u32, Stat)> {
    Vec::new()
}

/// Every live process whose KERNEL NAME is `comm`, by pid.
///
/// The name is the kernel's short one — capped (15 bytes on Linux, `MAXCOMLEN` here) and settable by
/// the process itself — so this NARROWS a search and never identifies: a caller that acts on the
/// answer must confirm which one it found by something else. Its consumer does exactly that, matching
/// the socket in the process's own environment before signalling anything, because a developer's box
/// runs their own daemon of the same name (R278).
///
/// A narrow public door rather than exposing `Stat` and `walk`, for [`parent`]'s reason.
#[must_use]
pub fn pids_named(comm: &str) -> Vec<u32> {
    walk()
        .into_iter()
        .filter_map(|(pid, stat)| (stat.comm == comm).then_some(pid))
        .collect()
}

/// A process's PARENT, or `None` when it is gone or this build cannot ask.
///
/// A narrow public door rather than exposing `Stat`: the one consumer outside this crate walks
/// ancestors and wants exactly this number, and a struct of four fields would make it inherit three
/// it has no opinion about. `sprag-mcp` read `/proc/<pid>/status` for it and therefore climbed no
/// ancestors at all off Linux.
#[must_use]
pub fn parent(pid: u32) -> Option<u32> {
    stat(pid).map(|stat| stat.ppid)
}

/// A process's ENVIRONMENT as the NUL-separated `KEY=VALUE` bytes it was exec'd with.
///
/// **The environment at EXEC, not now** — on both platforms. A process that calls `setenv` after
/// starting does not change this, which is exactly the property its callers want: the question they
/// ask is *"what did whoever started this hand it?"*, and an answer that drifted with the process's
/// own later choices could not be trusted to name the daemon an ancestor was launched beside.
///
/// `None` when the process is gone, or when this build cannot ask (see `stat` for the same
/// three-way split). **An absence, never an empty environment**: every real process has at least
/// `PATH`, so an empty answer would be a lie a caller cannot tell from a miss.
///
/// # ⚠ Why this is not a caller's own `read`
///
/// It was: `sprag-mcp` opened `/proc/<pid>/environ` directly, so the whole ancestor walk behind
/// *"which daemon is the agent running under?"* answered nothing off Linux — and its own test said
/// so on the first macOS run this suite ever completed. macOS keeps the environment in the SAME
/// `KERN_PROCARGS2` payload as the arguments, right behind them, so this and the `argv` beside it are one read
/// with two answers rather than two features.
#[cfg(target_os = "linux")]
pub fn environ(pid: u32) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/environ")).ok()
}

/// macOS: the tail of the `KERN_PROCARGS2` payload, behind the arguments the count ends.
#[cfg(target_os = "macos")]
#[must_use]
pub fn environ(pid: u32) -> Option<Vec<u8>> {
    let raw = procargs2(pid)?;
    let (_, environ) = split_procargs2(&raw);
    (!environ.is_empty()).then_some(environ)
}

/// Neither source, so nothing to read — the honest absence `stat` and `walk` also answer with.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
#[must_use]
pub fn environ(_pid: u32) -> Option<Vec<u8>> {
    None
}

/// macOS: a process's ARGUMENTS, from the same payload [`environ`] reads.
#[cfg(target_os = "macos")]
pub(crate) fn argv(pid: u32) -> Vec<String> {
    let Some(raw) = procargs2(pid) else {
        return Vec::new();
    };
    split_procargs2(&raw).0
}

/// The raw `KERN_PROCARGS2` payload for `pid`.
///
/// `KERN_ARGMAX` rather than a size probe: this flavour does not answer a length for a null buffer,
/// and the kernel's own ceiling is the only honest bound on how big the answer can be. Reading a
/// process this user does not own is refused by the kernel — a non-zero return and an empty answer,
/// never a partial read of somebody else's memory.
#[cfg(target_os = "macos")]
fn procargs2(pid: u32) -> Option<Vec<u8>> {
    let pid = i32::try_from(pid).ok()?;
    let mut argmax: libc::c_int = 0;
    let mut size = std::mem::size_of::<libc::c_int>();
    let mut ask = [libc::CTL_KERN, libc::KERN_ARGMAX];
    // SAFETY: `ask` is a two-element MIB and the out-buffer with its length describe one `c_int`.
    if unsafe {
        libc::sysctl(
            ask.as_mut_ptr(),
            2,
            std::ptr::from_mut(&mut argmax).cast(),
            &raw mut size,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }

    let mut buffer = vec![0u8; usize::try_from(argmax).ok()?];
    let mut filled = buffer.len();
    let mut ask = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
    // SAFETY: `ask` is a three-element MIB and `filled` is `buffer`'s real length.
    if unsafe {
        libc::sysctl(
            ask.as_mut_ptr(),
            3,
            buffer.as_mut_ptr().cast(),
            &raw mut filled,
            std::ptr::null_mut(),
            0,
        )
    } != 0
    {
        return None;
    }
    buffer.truncate(filled);
    Some(buffer)
}

/// Split a `KERN_PROCARGS2` payload into its ARGUMENTS and its ENVIRONMENT.
///
/// # The layout, which is not `/proc`'s
///
/// One buffer: a 4-byte `argc`, then the **executable path**, then padding NULs, then `argc`
/// NUL-terminated arguments, then the environment as `KEY=VALUE` records. So two things have to be
/// stepped over before the arguments begin, and **the count is what says where they end** — taking
/// records until an empty one would hand a caller their own environment as a command line, secrets
/// and all, which is why the count is load-bearing rather than a nicety.
///
/// The environment is returned as the NUL-separated BYTES behind them, in `/proc/<pid>/environ`'s
/// own shape, so one parser serves both platforms' callers.
///
/// `argv[0]` is the program as the process was invoked with it, matching Linux's
/// `/proc/<pid>/cmdline`; the executable path in front of it is the kernel's own resolution and is
/// dropped, or a person would read the same program twice.
///
/// Compiled and TESTED on every platform: the shape of a payload is not a syscall, and a parser
/// only one runner ever builds is one only that runner can catch a mistake in.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn split_procargs2(raw: &[u8]) -> (Vec<String>, Vec<u8>) {
    let empty = (Vec::new(), Vec::new());
    let Some((count, rest)) = raw.split_at_checked(std::mem::size_of::<u32>()) else {
        return empty;
    };
    let argc = u32::from_ne_bytes([count[0], count[1], count[2], count[3]]);
    let Ok(argc) = usize::try_from(argc) else {
        return empty;
    };
    // Step over the executable path, then over the padding NULs the kernel aligns it with.
    let Some(end) = rest.iter().position(|&byte| byte == 0) else {
        return empty;
    };
    let after_exec = &rest[end..];
    let Some(start) = after_exec.iter().position(|&byte| byte != 0) else {
        return empty;
    };
    let body = &after_exec[start..];

    let mut args = Vec::with_capacity(argc);
    let mut cursor = 0usize;
    for _ in 0..argc {
        let Some(len) = body[cursor..].iter().position(|&byte| byte == 0) else {
            // A payload that ends inside the arguments has no environment behind them either.
            args.push(String::from_utf8_lossy(&body[cursor..]).into_owned());
            return (args, Vec::new());
        };
        args.push(String::from_utf8_lossy(&body[cursor..cursor + len]).into_owned());
        cursor += len + 1;
    }
    (args, body[cursor.min(body.len())..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`stat`] answers about a process this one already knows the answers for — ITSELF.
    ///
    /// # Platform-blind on purpose
    ///
    /// The parser tests below drive a Linux line's LAYOUT on every platform, which is right for a
    /// byte parser and says nothing at all about whether this box can read a process. That was the
    /// gap: `stat` answered `None` everywhere but Linux for the whole life of this module, and the
    /// first macOS run of this suite (R343) failed on it in `pane_pty`, in `processes` and in the
    /// CLI — none of which named `procfs`.
    ///
    /// So this asks for facts the OS will also state directly, and fails on whichever platform's
    /// implementation is wrong. `comm` is asserted non-empty rather than by name: the kernel caps it
    /// (15 bytes on Linux, `MAXCOMLEN` here) so a test binary's long hashed name is truncated
    /// differently on each, and pinning the spelling would be a claim about cargo rather than about
    /// this reader.
    #[test]
    fn stat_reads_back_the_parent_and_group_this_process_was_given() {
        let read = stat(std::process::id());

        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_eq!(
                read, None,
                "a platform with no reader answers an honest absence, never a guess",
            );
            return;
        }

        let read = read.expect("a platform with a reader must answer about its own process");
        // SAFETY: both are argument-free getters that cannot fail.
        let (ppid, pgrp) = unsafe { (libc::getppid(), libc::getpgrp()) };
        assert_eq!(
            read.ppid,
            u32::try_from(ppid).expect("a pid is not negative"),
            "the parent the OS reports for this process",
        );
        assert_eq!(
            read.pgrp,
            u32::try_from(pgrp).expect("a group is not negative"),
            "and the process group it reports",
        );
        assert!(!read.comm.is_empty(), "and a name to show a person");
    }

    /// [`walk`] finds this process, with the same facts [`stat`] gives for it.
    ///
    /// ⚠ The one assertion that can catch a wrong `PROC_ALL_PIDS`: a bad flavour makes
    /// `proc_listpids` answer nothing, and "the walk returned" would pass over that in silence
    /// while every index built on it came back empty. Asking it to contain a process we can NAME is
    /// what makes the constant checkable.
    #[test]
    fn the_walk_contains_this_process_and_agrees_with_stat_about_it() {
        let me = std::process::id();
        let walked = walk();

        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            assert!(walked.is_empty(), "no reader, no processes, and no guesses");
            return;
        }

        assert!(
            walked.len() > 1,
            "a box running this test is running more than one process: {}",
            walked.len(),
        );
        let mine = walked
            .iter()
            .find(|(pid, _)| *pid == me)
            .map(|(_, stat)| stat.clone())
            .expect("the walk must contain the process doing the walking");
        assert_eq!(
            Some(&mine),
            stat(me).as_ref(),
            "and the walk's row for a process is the row a single read gives",
        );
    }

    /// The macOS payload is a DIFFERENT shape from `/proc`'s, and the count is what ends the
    /// arguments — everything behind them is the ENVIRONMENT.
    ///
    /// ⚠ A reader that split on NUL and took everything would hand a caller their own environment
    /// as a command line, secrets included. That is why the last case matters most, and why this
    /// parse is driven on every platform: the shape of a payload is not a syscall.
    #[test]
    fn the_macos_payload_splits_at_argc_and_the_rest_is_the_environment() {
        let payload = |argc: u32, body: &[u8]| {
            let mut raw = argc.to_ne_bytes().to_vec();
            raw.extend_from_slice(body);
            raw
        };
        let env = |raw: Vec<u8>| String::from_utf8(split_procargs2(&raw).1).expect("utf-8 env");

        let two = payload(
            2,
            b"/usr/bin/sleep\x00\x00\x00sleep\x00600\x00SHELL=/bin/zsh\x00PATH=/usr/bin\x00",
        );
        assert_eq!(
            split_procargs2(&two).0,
            vec!["sleep", "600"],
            "two arguments, and the environment behind them is not one of them",
        );
        assert_eq!(
            env(two),
            "SHELL=/bin/zsh\u{0}PATH=/usr/bin\u{0}",
            "and the environment comes back in `/proc/<pid>/environ`'s own NUL-separated shape",
        );

        let one = payload(1, b"/bin/cat\x00cat\x00AWS_SECRET=hunter2\x00");
        assert_eq!(split_procargs2(&one).0, vec!["cat"]);
        assert_eq!(
            env(one),
            "AWS_SECRET=hunter2\u{0}",
            "the count ends the vector even with no padding between the path and the args",
        );

        assert_eq!(
            split_procargs2(&payload(3, b"/bin/sh\x00\x00\x00sh\x00-c\x00\x00")).0,
            vec!["sh", "-c", ""],
            "an empty argument in the middle survives, as it does on Linux",
        );
        assert_eq!(
            split_procargs2(&payload(0, b"/bin/true\x00\x00true\x00")).0,
            Vec::<String>::new(),
            "argc 0 is no arguments, not all of them",
        );
        assert_eq!(split_procargs2(b"ab"), (Vec::new(), Vec::new()));
        assert_eq!(
            split_procargs2(&payload(2, b"/no/terminator")),
            (Vec::new(), Vec::new()),
            "a payload that ends inside the executable path has nothing to give",
        );
    }

    /// [`environ`] answers about a process this one already knows the answer for — ITSELF.
    ///
    /// `PATH` is in this binary's EXEC environment, so it is there; a name nobody exported is not.
    /// That pair is what separates "this build can read an environment" from "this build answers
    /// yes to everything".
    #[test]
    fn environ_reads_back_a_variable_this_process_was_exec_with() {
        let read = environ(std::process::id());

        if !cfg!(any(target_os = "linux", target_os = "macos")) {
            assert_eq!(read, None, "no reader, an honest absence, never a guess");
            return;
        }

        let read = read.expect("a platform with a reader must answer about its own process");
        let has = |key: &str| {
            read.split(|&byte| byte == 0)
                .filter_map(|record| std::str::from_utf8(record).ok())
                .any(|record| record.starts_with(&format!("{key}=")))
        };
        assert!(has("PATH"), "the exec environment carries PATH");
        assert!(
            !has("SPRAG_NOBODY_EXPORTED_THIS"),
            "and it does not carry a name nobody exported",
        );
    }

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
