//! WHERE a client is, in one line — the permanent half of the row a [`Report`](crate::report) takes
//! over.
//!
//! # Why the two halves are one surface
//!
//! R316 opened on a measured silence: a key bound to `switch-client -t ghost` left a live
//! `sprag-tui` byte-for-byte unchanged. Giving that key a sentence needs somewhere to paint one,
//! and a row that appears only to carry a refusal would reflow the panes under the user twice per
//! keystroke — so the row is permanent. A permanent row that is blank between refusals is worse
//! than no row, so it answers the question a keystroke's landing was the evidence for: **which
//! session am I on, and which of its windows.**
//!
//! That is not decoration. It is the reason a LANDING needs no message of its own: `switch-client
//! -n` that arrives somewhere changes this line, so a message repeating it would be noise over a
//! fact already on the screen ([`crate::report`]'s own cut).
//!
//! # Derived, never accumulated
//!
//! Every field is read from the daemon's own slots at paint time — the session the client is
//! attached to and that session's window list, the same two reads the GUI's session tabs and window
//! strip are drawn from. There is no state here to go stale, which is the whole difference between
//! this and a status line assembled by hand at each event: tmux's `status-left` / `status-right`
//! are FORMAT STRINGS re-expanded on a timer, and sprag has nothing to expand because it has
//! nothing cached.
//!
//! # What it deliberately does not carry
//!
//! No clock, no hostname, no format language. tmux's `#()` shells out on a timer from inside the
//! multiplexer; a status line that can run a command is a status line that can hang the client that
//! draws it, and this project has a verb for running things in a pane. The day a user wants one,
//! the argument to have is about a SANDBOX, not about a placeholder.

use sprag_terminal::WindowInfo;

/// The permanent content of a client's status row, already cut into the pieces a painter places.
///
/// A record rather than one string, so a frontend with two places to put things (a session tab bar
/// AND a window strip, which `sprag-gui` has) can use the halves it needs — and so the one-line
/// rendering below is the only thing that decides the punctuation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Status {
    /// The session this client is attached to, as the daemon spells it — never as the client
    /// remembers it. R303's rule: a client's scope is its ATTACHMENT, and the name is what the
    /// daemon answers for it now.
    pub session: String,
    /// That session's windows in the daemon's order, each with whether it is CURRENT.
    pub windows: Vec<WindowInfo>,
}

impl Status {
    /// Read this client's whereabouts from the host it is attached to.
    ///
    /// Two reads and no third: the session name and the window list. Both are mirrors the client
    /// already keeps for its other surfaces, so a status row costs a paint nothing on the wire.
    #[must_use]
    pub fn of(host: &dyn crate::HostClient) -> Self {
        Self {
            session: host.current_session(),
            windows: host.windows(),
        }
    }

    /// The line, as a frontend with ONE row paints it: `[session] 0:name 1:name*`.
    ///
    /// tmux's shape, deliberately — a user coming from tmux reads this without being taught — with
    /// tmux's own marker for the current window (`*`) and its bracketed session name. The INDEX is
    /// the window's place in the session's order, which is what `move-window` rearranges and what
    /// `select-window -n` walks; a list without it could not show a reorder happening.
    ///
    /// Not truncated here. The painter owns the width, because a GUI strip and an 80-column
    /// terminal cut a long line in different places and neither cut belongs in a shared definition.
    ///
    /// **One consumer today — `sprag-tui` — and that is stated rather than hidden.** It is here and
    /// not in that crate because of what it is FOR: [`crate::report`] cuts a landing out of what a
    /// client says out loud on the grounds that this line already shows it, and the two halves of
    /// that argument have to be readable together or the silence looks like an omission. The
    /// windowed front answers the same question with its session rail and window strip, which is
    /// chrome a terminal does not have.
    #[must_use]
    pub fn line(&self) -> String {
        let mut line = format!("[{}]", self.session);
        for (index, window) in self.windows.iter().enumerate() {
            line.push(' ');
            line.push_str(&format!("{index}:{}", window.name));
            if window.current {
                line.push('*');
            }
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HostClient;

    fn window(name: &str, current: bool) -> WindowInfo {
        WindowInfo {
            name: name.to_owned(),
            current,
            opened_by: None,
        }
    }

    /// The shape a tmux user already reads, and the marker on the window the client is projecting.
    #[test]
    fn the_line_names_the_session_and_marks_the_current_window() {
        let status = Status {
            session: "work".to_owned(),
            windows: vec![window("shell", false), window("logs", true)],
        };
        assert_eq!(status.line(), "[work] 0:shell 1:logs*");
    }

    /// The INDEX is the place in the order, so a reorder is visible in this line — which is the
    /// half a name-only list could not show.
    #[test]
    fn the_index_is_the_place_in_the_order() {
        let before = Status {
            session: "work".to_owned(),
            windows: vec![window("a", true), window("b", false)],
        };
        let after = Status {
            session: "work".to_owned(),
            windows: vec![window("b", false), window("a", true)],
        };
        assert_eq!(before.line(), "[work] 0:a* 1:b");
        assert_eq!(after.line(), "[work] 0:b 1:a*");
        assert_ne!(
            before.line(),
            after.line(),
            "the same windows in a different order must not paint the same line",
        );
    }

    /// A session with no windows is a transient state (its last one just closed), not an error, and
    /// the line still says where the client is.
    #[test]
    fn a_session_with_no_windows_still_says_which_session() {
        let status = Status {
            session: "work".to_owned(),
            windows: Vec::new(),
        };
        assert_eq!(status.line(), "[work]");
    }

    /// Read off the HOST, so a client cannot paint a session name it is no longer attached to.
    #[test]
    fn it_is_read_from_the_host_rather_than_remembered() {
        let host = crate::Host::new((40, 6));
        let _ = host.new_pane().expect("a shell is born");
        let status = Status::of(&host);
        assert_eq!(status.session, host.current_session());
        assert_eq!(status.windows, host.windows());
        assert!(
            status
                .line()
                .starts_with(&format!("[{}]", host.current_session())),
            "the line leads with the session the host names: {}",
            status.line(),
        );
    }
}
