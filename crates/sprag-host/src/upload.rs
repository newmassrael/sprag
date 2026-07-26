//! Dropped-file delivery: what a file dropped onto a pane does.
//!
//! ONE policy covers both pane kinds, because both answer the same question — *"make this file
//! reachable from this pane's shell"*:
//!
//! - an ORDINARY pane runs on THIS machine, so the file is already reachable: its local path is
//!   pasted straight in;
//! - a REMOTE workspace pane (one carrying a structured [`SshRemote`] — born of `sprag ssh`, see
//!   [`crate::ssh`]) runs on another machine, where the dropped file does not exist. So the file is
//!   UPLOADED first ([`SshTarget::scp_argv`]) and the REMOTE path is pasted once it lands.
//!
//! That symmetry is the point: a drop never silently does nothing, and what the pane receives is
//! always a path its own shell can open. It is also the cmux "drag-to-upload" affordance, which tmux
//! has no equivalent of at all.
//!
//! ## Why the path is PASTED, not typed
//!
//! The path goes in through [`crate::paste`], so it is wrapped in bracketed-paste markers when the
//! child asked for them (DEC 2004). That is a security property, not a nicety: a POSIX file name may
//! contain a NEWLINE, and a raw write of `evil\nrm -rf ~` would hand the shell a second line to
//! EXECUTE. Bracketed paste makes the shell hold it as literal text, and the path is
//! [shell-quoted](shell_quote) on top of that, so neither layer alone is load-bearing.
//!
//! ## Asynchrony and its one honest limit
//!
//! An upload is a network copy — far too slow to run on the request thread (it would stall every
//! other client of the single-dispatch host). Everything KNOWABLE up front is therefore validated
//! synchronously and refused to the caller (no such pane, no such file, an unusable path); the `scp`
//! itself runs on a detached background thread that owns nothing but the pane's
//! [`PanePtyHandle`] — no registry lock, so it can neither deadlock the host nor block a frame — and
//! pastes the remote path only if the transfer SUCCEEDED. A transfer that fails after that point
//! (auth, network, remote disk) is reported to the daemon log with scp's own stderr, and the pane
//! receives nothing: there is no host-originated pane message channel to surface it in, and silently
//! pasting a path to a file that never arrived would be worse than the log.

use std::path::Path;
use std::process::{Command, Stdio};

use sprag_terminal::{PanePtyHandle, SshRemote};

use crate::pane::paste;
use crate::shellword::shell_quote;
use crate::ssh::SshTarget;

/// The remote path an uploaded file lands at: `~/<basename>`, the basename shell-quoted.
///
/// `scp DEST:` (an empty remote path) delivers into the remote login HOME keeping the local
/// basename, so this is where the file provably is. The `~` is left OUTSIDE the quotes on purpose —
/// `'~/name'` would be a literal directory called `~`, whereas `~/'name'` expands the home directory
/// and keeps the name inert.
fn remote_home_path(basename: &str) -> String {
    format!("~/{}", shell_quote(basename))
}

/// Deliver a dropped file to a pane: the module's whole policy in one call.
///
/// `remote` is the pane's recorded endpoint ([`sprag_terminal::Pane::remote`]) — `Some` for a
/// `sprag ssh` workspace, `None` for an ordinary pane. Returns the path the pane is given (the
/// REMOTE path for an upload, the local path otherwise), or `None` when the drop is refused: a path
/// that does not exist, or one that cannot be expressed as UTF-8 / has no file name. The returned
/// path is what the caller answers on the wire; the pane itself receives it with a trailing SPACE,
/// so dropping several files in a row builds a usable argument list rather than one glued word.
pub(crate) fn deliver(
    handle: PanePtyHandle,
    remote: Option<SshRemote>,
    dropped: &Path,
) -> Option<String> {
    // Canonicalize FIRST: it proves the file exists (the drop may name something already gone),
    // resolves a symlink to what scp would actually copy, and yields the absolute path a pane's
    // shell can use whatever its cwd is.
    let local = std::fs::canonicalize(dropped)
        .inspect_err(|error| {
            tracing::debug!(target: "sprag_host", path = %dropped.display(), ?error, "refused a dropped file that cannot be resolved");
        })
        .ok()?;
    let Some(local_path) = local.to_str() else {
        tracing::debug!(target: "sprag_host", path = %local.display(), "refused a dropped file whose path is not UTF-8");
        return None;
    };

    let Some(remote) = remote else {
        // An ordinary pane: the file is already reachable here, so the drop IS the paste.
        let path = shell_quote(local_path);
        if !paste(&handle, &format!("{path} ")) {
            tracing::debug!(target: "sprag_host", %path, "a dropped path could not be written to the pane");
            return None;
        }
        return Some(path);
    };

    let basename = local.file_name().and_then(std::ffi::OsStr::to_str)?;
    let remote_path = remote_home_path(basename);
    let recursive = local.is_dir();
    let argv = SshTarget::from_remote(&remote).scp_argv(local_path, recursive);
    spawn_upload(handle, argv, remote_path.clone());
    Some(remote_path)
}

/// Run `argv` (an [`SshTarget::scp_argv`]) on a detached thread and, only on success, paste
/// `remote_path` into the pane.
///
/// The thread holds ONLY the pane handle — no workspace or registry lock — so a slow or wedged
/// transfer cannot block a frame, a request, or the host's shutdown. `stdin` is null so nothing can
/// wait on input that no one can type (scp's `-B` already refuses to prompt); stdout/stderr are
/// captured so a failure can be reported with the reason scp gave rather than a bare exit code, and
/// so no remote noise lands on the host's own terminal. A pane that closed mid-transfer just fails
/// the write — the upload still completed, which is the honest outcome to log.
fn spawn_upload(handle: PanePtyHandle, argv: Vec<String>, remote_path: String) {
    let (program, args) = match argv.split_first() {
        Some(split) => split,
        None => return, // unreachable: scp_argv always yields at least the program
    };
    let program = program.to_owned();
    let args: Vec<String> = args.to_vec();
    std::thread::spawn(move || {
        let outcome = Command::new(&program)
            .args(&args)
            .stdin(Stdio::null())
            .output();
        match outcome {
            Ok(output) if output.status.success() => {
                if !paste(&handle, &format!("{remote_path} ")) {
                    tracing::debug!(target: "sprag_host", %remote_path, "uploaded, but the pane closed before its path could be pasted");
                }
            }
            Ok(output) => {
                tracing::warn!(
                    target: "sprag_host",
                    status = %output.status,
                    stderr = %String::from_utf8_lossy(&output.stderr).trim(),
                    "a dropped-file upload failed; the pane was left untouched",
                );
            }
            Err(error) => {
                tracing::warn!(target: "sprag_host", %program, ?error, "a dropped-file upload could not be started");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_leaves_an_ordinary_path_bare() {
        assert_eq!(shell_quote("/home/me/report.pdf"), "/home/me/report.pdf");
        assert_eq!(shell_quote("a-b_c.d+e=f:g,h@i%j"), "a-b_c.d+e=f:g,h@i%j");
    }

    #[test]
    fn shell_quote_wraps_a_path_a_shell_would_reinterpret() {
        // Revert-proof for the quoting gate: each of these is a DIFFERENT shell hazard — a word
        // split, a home expansion, a command substitution, and (the injection that matters most) an
        // embedded newline, which a shell would otherwise take as a second command line.
        assert_eq!(shell_quote("/tmp/my report.pdf"), "'/tmp/my report.pdf'");
        assert_eq!(shell_quote("~/notes"), "'~/notes'");
        assert_eq!(shell_quote("/tmp/$(reboot)"), "'/tmp/$(reboot)'");
        assert_eq!(shell_quote("/tmp/a\nrm -rf x"), "'/tmp/a\nrm -rf x'");
    }

    #[test]
    fn shell_quote_closes_and_reopens_an_embedded_single_quote() {
        // The one escape POSIX single quoting allows; getting it wrong would leave the quote OPEN
        // and swallow the rest of the line.
        assert_eq!(shell_quote("/tmp/it's"), r"'/tmp/it'\''s'");
    }

    #[test]
    fn remote_home_path_expands_the_tilde_and_quotes_only_the_name() {
        // Revert-proof for the tilde placement: quoting the WHOLE `~/name` would name a literal
        // directory called `~`, which is not where scp put the file.
        assert_eq!(remote_home_path("report.pdf"), "~/report.pdf");
        assert_eq!(remote_home_path("my report.pdf"), "~/'my report.pdf'");
    }
}
