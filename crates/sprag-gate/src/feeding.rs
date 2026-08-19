//! Handing a child process its standard input — and surviving a child that **refuses before it
//! reads a byte**.
//!
//! # ⚠⚠⚠⚠⚠ Why this module exists: a gate that refuses early makes its feeder fail
//!
//! A pipe whose reader has gone answers a write with `EPIPE` (`BrokenPipe`), and Rust's runtime
//! turns the signal into that error rather than killing the process. So a fixture that spawns a
//! program, writes its input and treats a write failure as fatal is asserting something nobody
//! promised: **that the program will read.** A hook that refuses at its first guard — *the tool
//! every check needs is not installed* — exits without reading, which is the behaviour under test,
//! and the fixture then dies with `Broken pipe` instead of reading the exit status it came for.
//!
//! It is a RACE when the payload is small: the bytes fit in the pipe's buffer, so the write
//! succeeds if it happens to land before the child exits and fails if it does not. Measured on this
//! project 2026-08-19: `hooks_judge_the_bytes_being_published`'s
//! `neither_hook_proceeds_when_the_tool_all_its_checks_need_is_absent` failed that way once in the
//! first seven runs of a loop, on a build machine at 20 threads, and had passed every run on its
//! own — register item 471. It is DETERMINISTIC when the payload is larger than the buffer, which
//! is how the tests below stage it rather than waiting for load.
//!
//! ⚠ **Git itself sees the same `EPIPE`** when a hook exits without draining the ref list it feeds,
//! and judges the hook by its EXIT STATUS regardless. A harness standing in for git has to do the
//! same, or it cannot express the case where a gate refuses early — which is the case most worth
//! asserting.
//!
//! # What this is not
//!
//! Not a way to ignore write failures. A full disk, a bad file descriptor and a closed socket all
//! still panic here, with the errno in the message. Only the child having gone is tolerated, and
//! only because a gone child is a legitimate answer.

use std::io::Write;
use std::process::Child;

/// Write `bytes` to `child`'s standard input, then close it.
///
/// The close is what a child reading to end-of-file is waiting for, so this takes the handle rather
/// than borrowing it: holding a piped stdin open is how a fixture hangs.
///
/// # Panics
///
/// When the child was not spawned with a piped stdin — the caller asked to feed something that
/// cannot be fed — or when the write fails for any reason other than the child having gone.
pub fn feed(child: &mut Child, bytes: &[u8]) {
    let mut stdin = child.stdin.take().expect(
        "⚠ a child can only be fed through a PIPED stdin — spawn it with `Stdio::piped()`, or it \
         is inheriting this process's own",
    );
    match stdin.write_all(bytes) {
        Ok(()) => {}
        // ⚠ THE WHOLE POINT: the child refused before reading. Its exit status is the answer the
        // caller came for, and it is still there to be read.
        Err(why) if why.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(why) => panic!("feed {} byte(s) to the child's stdin: {why}", bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// More than any pipe buffer this could meet — Linux's default is 64 KiB, and a write that
    /// exceeds it BLOCKS until somebody reads, which is what turns the race into a certainty.
    const LONGER_THAN_A_PIPE: usize = 1 << 20;

    /// ⚠⚠⚠⚠⚠ **THE DEFECT, STAGED ON PURPOSE RATHER THAN WAITED FOR.**
    ///
    /// `true` exits without reading a byte, so a payload this size cannot fit in the buffer and
    /// the write is guaranteed to meet the closed pipe. Before this module the same shape was a
    /// 1-in-7 failure under load and green on its own, which is how it survived as *a flake*.
    ///
    /// ⚠ Reached through [`crate::doubles::system`] rather than spelled `/bin/true`: macOS has no
    /// such file, and this case went red on the macOS job of `28fb1a6` for that and nothing else.
    #[test]
    fn a_child_that_refuses_before_reading_does_not_take_its_feeder_down() {
        let mut child = Command::new(crate::doubles::system("true"))
            .stdin(Stdio::piped())
            .spawn()
            .expect("a child that exits without reading");

        feed(&mut child, &vec![b'x'; LONGER_THAN_A_PIPE]);

        assert!(
            child
                .wait()
                .expect("the child's status is still there to be read")
                .success(),
            "and the status is what the caller came for, not the write",
        );
    }

    /// ⚠⚠⚠⚠ **AND THE TOLERANCE IS NOT A SWALLOW**: a child that DOES read gets every byte, and
    /// gets the end-of-file that tells it there are no more.
    ///
    /// Without the close this case would hang rather than fail, which is the honest proof that the
    /// handle is dropped: `wc` counts to EOF and prints nothing until it arrives.
    #[test]
    fn a_child_that_reads_gets_every_byte_and_then_the_end_of_them() {
        let mut child = Command::new("wc")
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("a child that reads to end-of-file");

        feed(&mut child, &vec![b'x'; LONGER_THAN_A_PIPE]);

        let counted = child.wait_with_output().expect("wait for the counter");
        assert_eq!(
            String::from_utf8_lossy(&counted.stdout).trim(),
            LONGER_THAN_A_PIPE.to_string(),
            "every byte written must reach a child that reads",
        );
    }
}
