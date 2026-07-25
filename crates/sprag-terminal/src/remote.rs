//! A pane's structured remote identity: the SSH endpoint a `sprag ssh` workspace connects to.
//!
//! Recorded on the pane that `sprag ssh` births — its EXPLICIT intent marker, distinct from the raw
//! argv — so the host can (a) restore the workspace by RECONNECTING rather than dropping to a shell,
//! and (b) route a dropped-file upload to the right `scp` destination. Absent on an ordinary pane: a
//! shell that merely has `ssh` somewhere in its history is NOT a sanctioned remote workspace, so it
//! is never auto-reconnected. This is a plain data record; assembling it into an `ssh`/`scp` command
//! is the host's job (it owns the `SshTarget` builder).

use serde::{Deserialize, Serialize};

/// The SSH endpoint of a remote workspace pane: enough to reconnect (`ssh -t [-p PORT] user@host`)
/// and to name an `scp` target (`[user@]host`).
///
/// Deliberately NOT the forwards or the remote command — a restore re-establishes the CONNECTION
/// (a login shell), never re-runs a recorded remote command, so a side-effecting `-- rm -rf` can
/// never come back to life on its own. Set ONLY by the `sprag ssh` path, which is what makes a
/// restore's allowlist bypass safe: presence means "the user explicitly created a remote workspace".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SshRemote {
    /// The login user (the `user@` of `user@host`), or `None` to let ssh resolve it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// The remote host — a name or address. Never empty (the CLI rejects an empty destination).
    pub host: String,
    /// The ssh port (`-p`), or `None` for ssh's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}
