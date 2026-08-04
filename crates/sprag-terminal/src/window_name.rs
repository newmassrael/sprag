//! The grammar a WINDOW name has to satisfy — the rules for the address `select-window -t` takes.
//!
//! ## Why this is the THIRD of these, and what was holding it open
//!
//! A session name has had a grammar since R302 and a pane name since R295. A window name had none:
//! [`Session::rename_window`](crate::Session::rename_window) did `self.windows[idx].name =
//! new.to_owned()`, and [`Session::new_window`](crate::Session::new_window) stored a caller's
//! argument the same way. So the one level BETWEEN two validated ones accepted anything — an empty
//! string, a name with a newline in it, an escape sequence.
//!
//! It stayed open because until R306 nothing put a HUMAN's keystrokes into it. `sprag
//! rename-window` is typed by an operator who sees the result, and the MCP tool is a program. A
//! prompt bound to `prefix ,` is the first surface where a name arrives one character at a time
//! from somebody who has not finished thinking, which is exactly when "what happens to a blank
//! one?" stops being hypothetical.
//!
//! ## The rules, and the one that is deliberately inverted
//!
//! Blank, over-long and control-bearing names are refused for [`SessionName`](crate::SessionName)'s
//! reasons, and the control rule has the same teeth one level down: a window name is printed in
//! `sprag ls`, in both frontends' window strips and in `sprag panes`, so a `\n` forges a row and an
//! `ESC` is injected into the terminal of whoever reads the listing.
//!
//! **All-digits is ALLOWED**, where [`PaneName`](crate::PaneName) refuses it — and this is
//! `SessionName`'s reasoning rather than a relaxation. A pane is addressed by an ordinal NUMBER as
//! well as by a name and one argument carries both, so `"3"` would mean two things. A window is
//! addressed by nothing but its name: `lowest_free_window_name` MINTS `0`, `1`, `2`, tmux's own
//! window names are indices, and refusing digits would refuse every window this registry creates.
//!
//! ## Where it is enforced
//!
//! At the three places a window name ENTERS — [`Session::new_window`](crate::Session::new_window),
//! [`Session::rename_window`](crate::Session::rename_window) and
//! [`Session::break_pane`](crate::Session::break_pane)'s optional name. The registry stores a
//! `String`, as it does for a session name and for the same reason: every consumer addresses by
//! `&str`, so a newtype threaded through all of them would buy nothing this type does not already
//! buy at the only doors.

use std::fmt;

/// A validated window name — the address form, not the storage form.
///
/// Construct with [`WindowName::parse`]; there is no other way in. A name that is in the registry
/// got there through one of the three doors the module docs name, and each of them parses.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowName(String);

/// Why a proposed window name was refused — one variant per rule, so an IN-PROCESS caller can name
/// the mistake instead of listing every rule.
///
/// The split [`PaneNameError`](crate::PaneNameError) and
/// [`SessionNameError`](crate::SessionNameError) both record: a WIRE refusal is a disjunction while
/// upstream PINION-PR82 is unlanded, and every caller inside this process can do better.
///
/// **R306 made that split pay twice.** The client-side prompt parses with this same function before
/// it sends, so a grammar refusal is precise and instant — and the only refusal left for the
/// payload-less wire `Rejected` to carry is *the name is already taken*, which is one cause and so
/// one sentence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowNameError {
    /// Nothing but whitespace. An address nobody can type is not an address.
    Empty,
    /// Longer than [`WindowName::MAX_BYTES`], in bytes. Carries the length that was offered.
    TooLong(usize),
    /// Contains a control character — the rule with teeth. See the module docs.
    Control,
}

impl fmt::Display for WindowNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a window name cannot be blank"),
            Self::TooLong(len) => write!(
                f,
                "a window name is at most {} bytes, and that one is {len}",
                WindowName::MAX_BYTES
            ),
            Self::Control => write!(
                f,
                "a window name cannot contain control characters (a newline would forge a row of \
                 the window listing, and an escape would be interpreted by whoever reads it)"
            ),
        }
    }
}

impl std::error::Error for WindowNameError {}

impl WindowName {
    /// The longest a window name may be, in BYTES.
    ///
    /// Bounded for [`SessionName::MAX_BYTES`](crate::SessionName::MAX_BYTES)'s reasons: the name is
    /// persisted, printed in every listing, and sent as a parameter. It agrees with the other two
    /// and does not import either — three independent decisions that land on the same number,
    /// because they are answering the same question about the same terminals.
    pub const MAX_BYTES: usize = 80;

    /// Validate `proposed` as a window name, trimming surrounding whitespace first.
    ///
    /// Trimming rather than refusing a padded name is the choice both sibling types make, kept for
    /// the same reason: `-t " build"` is a shell's doing far more often than a user's, and the
    /// caller is told what was RECORDED rather than what it sent.
    ///
    /// # Errors
    ///
    /// [`WindowNameError`], one variant per rule. Note what is NOT an error: an all-digit name,
    /// which is what this registry allocates by default (module docs).
    pub fn parse(proposed: &str) -> Result<Self, WindowNameError> {
        let trimmed = proposed.trim();
        if trimmed.is_empty() {
            return Err(WindowNameError::Empty);
        }
        if trimmed.len() > Self::MAX_BYTES {
            return Err(WindowNameError::TooLong(trimmed.len()));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(WindowNameError::Control);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The name as it will be printed, matched and addressed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WindowName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<WindowName> for String {
    fn from(name: WindowName) -> Self {
        name.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rule, and the two things that are deliberately NOT rules.
    #[test]
    fn the_grammar_refuses_what_would_break_a_listing_and_allows_what_the_registry_mints() {
        assert_eq!(
            WindowName::parse("  build  ").map(String::from).as_deref(),
            Ok("build")
        );

        assert_eq!(WindowName::parse("").unwrap_err(), WindowNameError::Empty);
        assert_eq!(
            WindowName::parse("   ").unwrap_err(),
            WindowNameError::Empty,
            "whitespace trims to nothing, which is the same unusable address",
        );
        assert_eq!(
            WindowName::parse(&"z".repeat(WindowName::MAX_BYTES + 1)).unwrap_err(),
            WindowNameError::TooLong(WindowName::MAX_BYTES + 1),
        );
        assert!(WindowName::parse(&"z".repeat(WindowName::MAX_BYTES)).is_ok());

        assert_eq!(
            WindowName::parse("a\nb").unwrap_err(),
            WindowNameError::Control,
            "a newline forges a row of the window listing",
        );
        assert_eq!(
            WindowName::parse("x\u{1b}[31m").unwrap_err(),
            WindowNameError::Control,
            "an escape is injected into the terminal of whoever reads it",
        );

        // NOT rules, and each is load-bearing rather than an oversight.
        assert!(
            WindowName::parse("0").is_ok(),
            "all digits is what this registry ALLOCATES — a window has no ordinal to be confused \
             with, which is the whole difference from a pane name",
        );
        assert!(
            WindowName::parse("my build").is_ok(),
            "a space inside is fine: nothing splits a window name on one",
        );
    }

    /// The rendered sentences, because a refusal a user cannot act on is barely better than none.
    #[test]
    fn every_refusal_says_which_rule_was_broken() {
        assert_eq!(
            WindowNameError::Empty.to_string(),
            "a window name cannot be blank",
        );
        assert_eq!(
            WindowNameError::TooLong(300).to_string(),
            "a window name is at most 80 bytes, and that one is 300",
        );
        assert!(
            WindowNameError::Control
                .to_string()
                .contains("would forge a row of the window listing"),
            "the control refusal names the consequence, not just the rule",
        );
    }
}
