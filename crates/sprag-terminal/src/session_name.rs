//! The grammar a SESSION name has to satisfy — the rules for the address every `-t` takes.
//!
//! ## Why this exists one level up from [`PaneName`](crate::pane_name::PaneName)
//!
//! A session name is the daemon's widest address: every scoped request carries it, every `-t`
//! takes it, every attached client holds it, and `sprag ls` prints it. Until a session could be
//! RENAMED it was minted only by [`new_session`](crate::SessionRegistry::new_session) — from a
//! caller's argument or from the registry's own `0`, `1`, … allocation — and nothing checked it.
//!
//! That hole was MEASURED rather than suspected, on a live daemon: `sprag rename-session ""`
//! answered `renamed to `, a name containing a newline printed as TWO rows of `sprag ls`, and
//! `x\ty\x1b[31m` put an escape sequence into the terminal of whoever read the listing. A rename
//! makes the hole reachable for a session that already exists and already has clients on it, so
//! this is the round that had to close it.
//!
//! ## The rules, and the ONE that is deliberately inverted
//!
//! Blank, over-long and control-bearing names are refused for exactly `PaneName`'s reasons, and
//! the control rule has the same teeth: `sprag ls` prints ONE LINE PER SESSION whose leading field
//! before the `:` is a contract, so a `\n` forges a row of it and an `ESC` is injected into a
//! reader's terminal by whoever chose the name.
//!
//! **All-digits is ALLOWED here where a pane name refuses it**, and the difference is not a
//! relaxation — it is the same rule reaching a different fact. A pane name is refused when it is
//! all digits because a pane is ALSO addressed by an ordinal number and one argument carries both,
//! so `"3"` would mean two things. A session is addressed by nothing but its name: there is no
//! ordinal to collide with, tmux's own default session names are `0` and `1`, and this registry
//! MINTS exactly those ([`lowest_free_name`](crate::SessionRegistry::new_session)). Refusing digits
//! would refuse the daemon's own boot session.

use std::fmt;

/// A validated session name — the address form, not the storage form.
///
/// The registry stores a `String`, deliberately: every consumer of a session name addresses by
/// `&str` (the wire's `session` param, the channel map key, the attachment record, `-t`), so a
/// newtype threaded through all of them would buy nothing this type does not already buy at the
/// only two places a name can ENTER. Minting is the whole of the exposure — a name that is in the
/// registry got there through [`new_session`](crate::SessionRegistry::new_session) or
/// [`rename_session`](crate::SessionRegistry::rename_session), and both parse.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionName(String);

/// Why a proposed session name was refused — one variant per rule, so an IN-PROCESS caller can
/// name the mistake instead of listing every rule.
///
/// The same split [`PaneNameError`](crate::pane_name::PaneNameError) records: a WIRE refusal is a
/// disjunction while upstream PINION-PR82 is unlanded, and every caller inside this process can do
/// better than that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionNameError {
    /// Nothing but whitespace. An address nobody can type is not an address.
    Empty,
    /// Longer than [`SessionName::MAX_BYTES`], in bytes. Carries the length that was offered.
    TooLong(usize),
    /// Contains a control character — the rule with teeth. See the module docs.
    Control,
}

impl fmt::Display for SessionNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a session name cannot be blank"),
            Self::TooLong(len) => write!(
                f,
                "a session name is at most {} bytes, and that one is {len}",
                SessionName::MAX_BYTES
            ),
            Self::Control => write!(
                f,
                "a session name cannot contain control characters (a newline would forge a row \
                 of the session listing, and an escape would be interpreted by whoever reads it)"
            ),
        }
    }
}

impl std::error::Error for SessionNameError {}

impl SessionName {
    /// The longest a session name may be, in BYTES.
    ///
    /// Bounded because the name is persisted to the durability snapshot, printed in every listing,
    /// and sent as a parameter on every scoped request — an unbounded peer-controlled string is a
    /// decoration's privilege, not an address's. It agrees with
    /// [`PaneName::MAX_BYTES`](crate::pane_name::PaneName::MAX_BYTES) and does not import it: two
    /// independent decisions that happen to land on the same number, for the reason that type's
    /// own docs give about the project file's limit.
    pub const MAX_BYTES: usize = 80;

    /// Validate `proposed` as a session name, trimming surrounding whitespace first.
    ///
    /// Trimming rather than refusing a padded name is [`PaneName`](crate::pane_name::PaneName)'s
    /// choice, kept for the same reason: `-t " work"` is a shell's doing far more often than a
    /// user's, and the caller is told what was RECORDED rather than what it sent.
    ///
    /// # Errors
    ///
    /// [`SessionNameError`], one variant per rule, so a caller can say which. Note what is NOT an
    /// error: an all-digit name, which is what this registry allocates by default (module docs).
    pub fn parse(proposed: &str) -> Result<Self, SessionNameError> {
        let trimmed = proposed.trim();
        if trimmed.is_empty() {
            return Err(SessionNameError::Empty);
        }
        if trimmed.len() > Self::MAX_BYTES {
            return Err(SessionNameError::TooLong(trimmed.len()));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(SessionNameError::Control);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The name as it will be printed, matched and addressed.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<SessionName> for String {
    fn from(name: SessionName) -> Self {
        name.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every rule, and the two things that are deliberately NOT rules.
    #[test]
    fn the_grammar_refuses_what_would_break_a_listing_and_allows_what_the_daemon_mints() {
        assert_eq!(
            SessionName::parse("  work  ").map(String::from).as_deref(),
            Ok("work")
        );

        assert_eq!(SessionName::parse("").unwrap_err(), SessionNameError::Empty);
        assert_eq!(
            SessionName::parse("   ").unwrap_err(),
            SessionNameError::Empty,
            "whitespace trims to nothing, which is the same unusable address",
        );
        assert_eq!(
            SessionName::parse(&"z".repeat(SessionName::MAX_BYTES + 1)).unwrap_err(),
            SessionNameError::TooLong(SessionName::MAX_BYTES + 1),
        );
        assert!(SessionName::parse(&"z".repeat(SessionName::MAX_BYTES)).is_ok());

        // THE ONE WITH TEETH, and both halves were MEASURED going straight into `sprag ls`.
        assert_eq!(
            SessionName::parse("a\nb").unwrap_err(),
            SessionNameError::Control,
            "a newline forges a row of the listing",
        );
        assert_eq!(
            SessionName::parse("x\u{1b}[31m").unwrap_err(),
            SessionNameError::Control,
            "an escape is injected into the terminal of whoever reads it",
        );

        // NOT rules, and each is load-bearing rather than an oversight.
        assert!(
            SessionName::parse("0").is_ok(),
            "all digits is what this registry ALLOCATES — a session has no ordinal to be confused \
             with, which is the whole difference from a pane name",
        );
        assert!(
            SessionName::parse("my work").is_ok(),
            "a space inside is fine: the listing's contract is the field before the colon",
        );
    }

    /// The rendered sentences, because a refusal a user cannot act on is barely better than none.
    #[test]
    fn every_refusal_says_which_rule_was_broken() {
        assert_eq!(
            SessionNameError::Empty.to_string(),
            "a session name cannot be blank",
        );
        assert_eq!(
            SessionNameError::TooLong(300).to_string(),
            "a session name is at most 80 bytes, and that one is 300",
        );
        assert!(
            SessionNameError::Control
                .to_string()
                .contains("would forge a row of the session listing"),
            "the control refusal names the consequence, not just the rule",
        );
    }
}
