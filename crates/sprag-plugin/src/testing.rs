//! Fixture vocabulary shared by the plugin gates — the shell fragments whose CORRECTNESS is not
//! visible in reading them.
//!
//! Test-only ([`cfg(test)`]), and it exists because one of these cost two rebuilds of the same
//! gate before it measured anything. A plugin gate about *readiness* has to build a pane that is
//! genuinely NOT ready, and "not ready" is a stronger condition than it looks: see
//! [`STANDIN_READS_TTY`].

/// Redirection that gives a BACKGROUNDED reader the pane's own input.
///
/// # ⚠⚠ Why a stand-in without this measures nothing
///
/// A gate for a readiness barrier needs a pane that is not ready yet, and the only stand-in that
/// discriminates is one that EATS what it is given — because an un-eaten injection does not
/// vanish. It sits in the pseudoterminal's buffer, and the program that starts next reads it. So a
/// run that injected far too early still converges, off bytes it sent to nobody, and the gate
/// reports that the barrier worked.
///
/// The obvious spelling of a stand-in that steps aside on a timer is a background reader:
///
/// ```sh
/// while read early; do echo "ATE $early"; done &   # ⚠ reads /dev/null, eats NOTHING
/// sleep 2; kill $!; exec the-real-peer
/// ```
///
/// **A background job of a NON-INTERACTIVE shell gets its stdin from `/dev/null`** (POSIX: job
/// control is off, so the shell redirects it). The stand-in therefore reads end-of-file
/// immediately, consumes nothing, and the fixture is back to the un-eaten-bytes case with nothing
/// in its text to show it. Appending this reopens the controlling terminal — the pane's own input —
/// for that job, which is what makes the stand-in a stand-in.
///
/// ⚠ Belongs on the READER, before the `&`: `while read x; do …; done </dev/tty &`.
pub(crate) const STANDIN_READS_TTY: &str = "</dev/tty";

/// Assert that a readiness barrier REFUSED for `wanted`, and that the job it blames is the one the
/// pane was LAUNCHED as.
///
/// # ⚠⚠ Why not `assert_eq!` against the whole error
///
/// That is what these gates did, and it made every one of them assert a PLATFORM's spelling. A pane
/// spawned as `/bin/sh` is led by a process the kernel calls `sh` on Linux and `bash` on macOS, so a
/// gate comparing the error to `Job("sh")` passes on one runner and fails on the other — which is
/// how this workspace found the divergence, one red at a time, after a push.
///
/// The spelling was never the claim. The claim is that **the refusal names the program the caller
/// launched, in the caller's own word** — and [`JobLeader::answers_to`] is what the product itself
/// decides that with, so a gate written this way measures the product's answer rather than a
/// distribution's packaging.
/// ⚠ Takes the FAILURE rather than the outcome, because a barrier's refusal reaches its two callers
/// in two shapes — `Err` from [`Readiness::reached`](crate::Readiness::reached), and an
/// `Outcome::failure` from a run — and a gate for either is asking the same question.
pub(crate) fn refused_naming(
    failure: Option<&crate::access::PaneError>,
    wanted: &crate::ReadyWhen,
    launched_as: &str,
    why: &str,
) {
    let Some(crate::access::PaneError::NeverReady {
        wanted: asked,
        instead,
    }) = failure
    else {
        panic!(
            "{why} — but the barrier did not refuse for a readiness it never reached: {failure:?}"
        );
    };
    assert_eq!(
        asked, wanted,
        "{why} — and the refusal hands back the WHOLE question, or a caller cannot tell which of \
         the kinds they got wrong",
    );
    let Some(leader) = instead.leader() else {
        panic!("{why} — but nothing was reported as owning the pane's terminal: {instead}");
    };
    assert!(
        leader.answers_to(launched_as),
        "{why} — the refusal blames {leader} on a pane launched as {launched_as:?}, and a \
         correction phrased in a word the caller never wrote is one they cannot act on",
    );
}

#[cfg(test)]
mod tests {
    use super::refused_naming;
    use crate::access::{JobLeader, PaneDoing, PaneError};
    use crate::readiness::ReadyWhen;

    /// ⚠⚠ **THE HELPER'S OWN REFUSALS FIRE** — the two paths that were registered as *"built by
    /// nothing"* rather than written, which took a `#[should_panic]` each.
    ///
    /// It matters because this helper is what NINE gates decide a readiness refusal by. A shape
    /// check that silently accepted the wrong shape would make all nine pass over a barrier that
    /// refused for some other reason entirely, or over one that blamed nobody — and a gate that
    /// cannot fail is worse than no gate, because its green is read as evidence.
    #[test]
    #[should_panic(expected = "did not refuse for a readiness it never reached")]
    fn a_failure_that_is_not_a_readiness_refusal_is_rejected() {
        refused_naming(
            Some(&PaneError::Write("broken pipe".to_string())),
            &ReadyWhen::Runs("claude".to_string()),
            "claude",
            "a write failure is not a barrier giving up",
        );
    }

    /// The other half: a refusal that names no program at all. `PaneDoing::Unknown` is the honest
    /// answer from a host with no process table — and a gate asserting *the refusal names what the
    /// caller launched* must not read it as a pass.
    #[test]
    #[should_panic(expected = "nothing was reported as owning the pane's terminal")]
    fn a_refusal_that_blames_nobody_is_rejected() {
        refused_naming(
            Some(&PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Unknown,
            }),
            &ReadyWhen::Runs("claude".to_string()),
            "claude",
            "a host that cannot see the process table blames nobody",
        );
    }

    /// ⚠ AND THE CONTROL: the shape it is FOR passes. Without this the two above are satisfied by a
    /// helper that panics unconditionally.
    #[test]
    fn the_shape_it_exists_for_passes() {
        refused_naming(
            Some(&PaneError::NeverReady {
                wanted: ReadyWhen::Runs("claude".to_string()),
                instead: PaneDoing::Job(JobLeader::known_as("sh".to_string())),
            }),
            &ReadyWhen::Runs("claude".to_string()),
            "sh",
            "a barrier that gave up on a pane still running its shell",
        );
    }
}
