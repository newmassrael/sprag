//! Whether every pseudoterminal this workspace opens goes through the ONE door that explains a
//! refusal — register item 776, arm (d).
//!
//! # ⚠⚠⚠⚠⚠ The guess this exists to stop, and it has already been made twice
//!
//! A macOS CI job failed with a bare `ENXIO` — *Device not configured* — and it was filed as *the
//! runner's pty pool was exhausted*. Nothing in the message supported that: re-measuring found the
//! peak concurrent count nowhere near the ceiling and exactly ONE refusal in the whole job, both
//! of which argue against exhaustion. The cause is still unknown, and the round that measured it
//! wrote down what it could not tell rather than a cause.
//!
//! What closes that is not a better guess but a better SENTENCE, and
//! `sprag_terminal::Pty::open` now carries one: how big the host says its pool is, how much
//! of it the host says is in use (Linux publishes it, Darwin does not), and how many this process
//! was holding itself. **That sentence is attached at one call site.** A second call to `openpty`
//! anywhere in this workspace would produce a refusal with none of it, and the next reader would
//! complete it from memory — which is the whole defect, arriving again through a door nobody
//! remembered to route.
//!
//! # ⛔⛔⛔ Why a ratchet and not a comment on the function
//!
//! The one-door fact is TRUE today: measured 2026-08-31, `libc::openpty` appears once in
//! `crates/`, in `sprag-terminal/src/pty.rs`, and every other mention is prose. A comment saying
//! so is a claim nobody re-derives — this workspace's own rule — and the day a second backend or a
//! test helper opens its own pair, the comment stays true-looking and the sentence quietly stops
//! covering every refusal. So it is asked, every run.

use crate::sources::Source;

/// The one file allowed to open a pseudoterminal — the door that explains its own refusals.
pub const THE_DOOR: &str = "crates/sprag-terminal/src/pty.rs";

/// The calls that hand this process a pseudoterminal.
///
/// ⚠ All three, not just the one in use: `openpty` is what this workspace calls today, and a round
/// reaching for `posix_openpt` or `forkpty` instead would be opening the same device by another
/// name — and would get the same undecorated errno.
const DOORS: [&str; 3] = ["openpty", "posix_openpt", "forkpty"];

/// Whether `line` OPENS a pseudoterminal, as opposed to mentioning one.
///
/// ⚠⚠ The distinction is the call, so the needle carries its `(`. `ttyname_r` on a slave fd, a
/// field called `pty`, and a doc link to `Pty::open` all name the subject without allocating one,
/// and a gate that could not tell them apart would either fire on prose or have to be narrowed
/// until it fired on nothing.
#[must_use]
pub fn opens_a_pseudoterminal(line: &str) -> bool {
    let squeezed: String = line.chars().filter(|c| !c.is_whitespace()).collect();
    DOORS
        .iter()
        .any(|door| squeezed.contains(&format!("{door}(")))
}

/// Every site outside [`THE_DOOR`] that opens one, as `(file, line, text)`.
#[must_use]
pub fn strays(sources: &[Source]) -> Vec<(String, usize, String)> {
    sources
        .iter()
        .filter(|source| source.file != THE_DOOR)
        .flat_map(|source| {
            source
                .code
                .iter()
                .filter(|(_, line)| opens_a_pseudoterminal(line))
                .map(|(at, line)| (source.file.clone(), *at, line.clone()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::rust_sources;

    /// ⛔⛔⛔⛔⛔ **EVERY PSEUDOTERMINAL THIS WORKSPACE OPENS COMES FROM THE DOOR THAT EXPLAINS
    /// ITSELF** — register item 776, arm (d).
    #[test]
    fn only_one_door_opens_a_pseudoterminal() {
        let sources = rust_sources();
        // ⚠ THE PROBE IS AIMED FIRST. A needle that matches nothing anywhere would make the
        // assertion below pass by finding nothing, which is the shape this crate's own walker
        // guards against for the same reason.
        assert!(
            sources.iter().any(|source| source.file == THE_DOOR
                && source.code.iter().any(|(_, l)| opens_a_pseudoterminal(l))),
            "⛔ the probe found no pseudoterminal being opened even at {THE_DOOR}, so it is \
             pointed at nothing and the sweep below proves nothing",
        );

        let strays = strays(&sources);
        assert!(
            strays.is_empty(),
            "⛔⛔⛔⛔⛔ A SECOND DOOR OPENS A PSEUDOTERMINAL, and its refusals carry none of what \
             {THE_DOOR} attaches to them — the pool's size, how much of it the host says is in \
             use, and how many this process held. A bare `ENXIO` from here is exactly what got \
             filed as *the runner's pty pool was exhausted* with nothing behind it (register item \
             776, arm (d)), and the reader who meets this one will complete it from memory too. \
             Route it through `sprag_terminal::Pty::open`. Found: {strays:?}",
        );
    }

    /// ⚠⚠ **AND THE PROBE TELLS A DOOR FROM A MENTION**, or the sweep above is either noise or
    /// blind — driven on lines rather than on the tree, so both halves are asked.
    #[test]
    fn the_probe_reads_a_call_and_not_a_mention() {
        // ⛔⛔⛔⛔⛔ THE FIXTURES ARE SPLIT ACROSS `concat!`, AND THAT IS NOT THE GATE BEING
        // DODGED — it is the gate being right about this file.
        //
        // Written whole, these lines ARE calls as far as a source scan is concerned, and the sweep
        // above found them and went red. The alternative was an exemption for this file, which is
        // the shape that hollows a gate out: an exemption list grows, and the day something real
        // lands here it is already excused. Split, the source carries no call and the probe still
        // gets the string it is being asked about — the fixture stops lying about what this file
        // does, rather than the gate stopping looking at it.
        for opening in [
            concat!(
                "let opened = unsafe { libc::open",
                "pty(&raw mut master) };"
            ),
            concat!("libc::fork", "pty(std::ptr::null_mut())"),
            concat!("let fd = posix_open", "pt(libc::O_RDWR);"),
            // ⚠ Wrapped by rustfmt: the needle squeezes whitespace for this exact case.
            concat!("libc :: open", "pty (\n    &raw mut master,\n)"),
        ] {
            assert!(
                opens_a_pseudoterminal(opening),
                "⛔ a call this workspace could make is not read as one: {opening:?}",
            );
        }
        for mention in [
            "/// `openpty` failed once on a macOS CI job with `ENXIO`",
            "// resolved by the PTY backend at `openpty` — so the name is the child's",
            "let name = ttyname_r(slave.as_raw_fd())?;",
            "pub struct Pty { master: OwnedFd }",
            "use sprag_terminal::Pty;",
        ] {
            assert!(
                !opens_a_pseudoterminal(mention),
                "⛔⛔ a MENTION is read as a call, so this gate fires on prose and the next round \
                 will narrow it until it fires on nothing: {mention:?}",
            );
        }
    }
}
