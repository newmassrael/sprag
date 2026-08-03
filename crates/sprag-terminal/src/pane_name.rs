//! The name a PERSON gives a pane — a validated address, not a decoration.
//!
//! A pane already carries two name-shaped facts and this is neither of them:
//!
//! * [`Pane::command_label`](crate::Pane::command_label) — what was LAUNCHED. Always present,
//!   never changes, chosen by nobody.
//! * [`Pane::title`](crate::Pane::title) — the child's own `OSC 0`/`2` window title. Live,
//!   rewritten on every shell prompt, **chosen by the child**, and an INPUT to the agent detector.
//!
//! A [`PaneName`] is chosen by a person (or by the pane's opener, which is a party since R294) and
//! changes only when somebody says so. That is what makes it usable as an ADDRESS where the other
//! two are not: one is not unique, and the other is not stable for as long as a single prompt.
//!
//! ## Why the type exists at all, rather than a `String` field
//!
//! Because every refusal below is load-bearing, and a `String` field would let each caller decide
//! for itself which of them to apply. Parsing once, here, is what makes the rules a property of
//! the name rather than of whichever surface happened to accept it.

use std::fmt;

/// A pane's operator-given name — trimmed, bounded, printable, and never a bare number.
///
/// Construct with [`PaneName::parse`]; there is no other way in, which is the point. Compares and
/// hashes as its string, so a lookup is an ordinary equality against what a caller typed.
///
/// It is deliberately NOT `Copy`/`Default`: there is no such thing as a default pane name, and a
/// pane without one is `None` rather than an empty name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
#[serde(transparent)]
pub struct PaneName(String);

/// Why a proposed pane name was refused — one variant per rule, so a surface can render the
/// caller's actual mistake instead of a disjunction of four.
///
/// This is the whole reason the rules live on a type. A WIRE refusal cannot carry a payload while
/// upstream PINION-PR82 is unlanded, so a client sees one disjunction — but every IN-PROCESS
/// caller (the CLI, the daemon's own logs) can say exactly which rule was broken, and does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneNameError {
    /// Nothing but whitespace. Clearing a name is a distinct request (an ABSENT name), not an
    /// empty one — a caller that sends `""` meaning "clear" and a caller that sends it by accident
    /// are indistinguishable, so neither is served.
    Empty,
    /// Longer than [`PaneName::MAX_BYTES`], in bytes (not chars — the bound is on what is written
    /// to disk and printed in a listing). Carries the length that was offered.
    TooLong(usize),
    /// Contains a control character. See [`PaneName::parse`] for why this one has teeth.
    Control,
    /// Every character is an ASCII digit, so the name would be indistinguishable from a pane
    /// NUMBER wherever the two share an argument. See [`PaneName::parse`].
    Numeric,
}

impl fmt::Display for PaneNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "a pane name cannot be blank"),
            Self::TooLong(len) => write!(
                f,
                "a pane name is at most {} bytes, and that one is {len}",
                PaneName::MAX_BYTES
            ),
            Self::Control => write!(
                f,
                "a pane name cannot contain control characters (a newline would forge a row of \
                 the pane listing, and an escape would be interpreted by whoever reads it)"
            ),
            Self::Numeric => write!(
                f,
                "a pane name cannot be all digits — it would be indistinguishable from a pane \
                 number wherever the two are passed in one argument"
            ),
        }
    }
}

impl std::error::Error for PaneNameError {}

impl PaneName {
    /// The longest a pane name may be, in BYTES.
    ///
    /// Bounded because the name is persisted to the snapshot file and printed in every pane
    /// listing — an unbounded peer-controlled string is a decoration's privilege, not an address's.
    /// The number matches the one the project file's action names take
    /// (`sprag_host::project::MAX_NAME_BYTES`); the two are independent decisions that happen to
    /// agree, and neither imports the other, because a config file's parse limit and a live
    /// object's identity limit answer to different pressures.
    pub const MAX_BYTES: usize = 80;

    /// Validate `proposed` as a pane name, trimming surrounding whitespace first.
    ///
    /// # The rules, and why each is not a matter of taste
    ///
    /// * **Blank is refused** ([`PaneNameError::Empty`]). Clearing is an absent name, not an empty
    ///   one.
    /// * **Over [`MAX_BYTES`](Self::MAX_BYTES) is refused** ([`PaneNameError::TooLong`]).
    /// * **A control character is refused** ([`PaneNameError::Control`]). This is the rule with
    ///   teeth. `sprag panes` prints ONE LINE PER PANE and its leading field is a contract (`cut
    ///   -d: -f1` yields exactly the ids the other verbs take, per that command's own docs) — so a
    ///   name containing `\n` forges a row of that contract, and an `ESC` is an escape sequence
    ///   injected into the terminal of whoever reads the listing by whoever named the pane.
    /// * **All-ASCII-digits is refused** ([`PaneNameError::Numeric`]). A name is a stable handle
    ///   that shares one argument with an ordinal pane NUMBER, discriminated by JSON's own types
    ///   (`pane: 3` is the third pane, `pane: "build"` is the pane called build). A pane called
    ///   `"3"` would make that argument's meaning depend on whether the caller quoted it, which is
    ///   a distinction no operator reads and no error message can recover afterwards. Refusing the
    ///   form makes the ambiguity unrepresentable rather than documented.
    ///
    /// # Errors
    ///
    /// [`PaneNameError`], one variant per rule above, so a caller can say which.
    pub fn parse(proposed: &str) -> Result<Self, PaneNameError> {
        let trimmed = proposed.trim();
        if trimmed.is_empty() {
            return Err(PaneNameError::Empty);
        }
        if trimmed.len() > Self::MAX_BYTES {
            return Err(PaneNameError::TooLong(trimmed.len()));
        }
        if trimmed.chars().any(char::is_control) {
            return Err(PaneNameError::Control);
        }
        if trimmed.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(PaneNameError::Numeric);
        }
        Ok(Self(trimmed.to_owned()))
    }

    /// The name as it will be printed and matched.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PaneName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<PaneName> for String {
    fn from(name: PaneName) -> Self {
        name.0
    }
}

impl AsRef<str> for PaneName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// Deserialised through [`PaneName::parse`], so a name read back from a SNAPSHOT obeys the same
/// rules as one a caller sent.
///
/// A snapshot is a file this daemon wrote, but a file on disk is not a trusted input: an older
/// daemon (or a hand-edited file) can hold a name this build refuses, and letting it in through
/// the back door would put a forgeable name into a listing that the front door exists to keep out.
impl<'de> serde::Deserialize<'de> for PaneName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_trimmed_and_kept_verbatim_otherwise() {
        let name = PaneName::parse("  the build  ").expect("a plain name is a name");
        assert_eq!(
            name.as_str(),
            "the build",
            "surrounding whitespace is trimmed"
        );
        assert_eq!(
            PaneName::parse("작업 판").unwrap().as_str(),
            "작업 판",
            "the bound is on bytes, but nothing restricts a name to ASCII"
        );
    }

    #[test]
    fn a_blank_name_is_refused_rather_than_read_as_a_clear() {
        assert_eq!(PaneName::parse("").unwrap_err(), PaneNameError::Empty);
        assert_eq!(PaneName::parse("   ").unwrap_err(), PaneNameError::Empty);
        assert_eq!(PaneName::parse("\t\n ").unwrap_err(), PaneNameError::Empty);
    }

    #[test]
    fn a_name_is_bounded_in_bytes_not_characters() {
        let ascii = "n".repeat(PaneName::MAX_BYTES);
        assert!(
            PaneName::parse(&ascii).is_ok(),
            "exactly the bound is allowed"
        );
        assert_eq!(
            PaneName::parse(&format!("{ascii}n")).unwrap_err(),
            PaneNameError::TooLong(PaneName::MAX_BYTES + 1)
        );
        // 27 three-byte characters are 81 bytes and only 27 chars: a char-counted bound would
        // admit this, and the thing being bounded is what reaches the disk and the listing.
        let wide = "판".repeat(27);
        assert_eq!(wide.chars().count(), 27);
        assert_eq!(
            PaneName::parse(&wide).unwrap_err(),
            PaneNameError::TooLong(81)
        );
    }

    #[test]
    fn a_name_that_could_forge_a_listing_row_or_an_escape_is_refused() {
        // The listing prints one line per pane and its leading field feeds every other verb.
        assert_eq!(
            PaneName::parse("build\n7: 80x24  bash").unwrap_err(),
            PaneNameError::Control,
            "a newline in a name forges a row of `sprag panes`"
        );
        assert_eq!(
            PaneName::parse("build\x1b[2J").unwrap_err(),
            PaneNameError::Control,
            "an ESC in a name is a sequence executed by whoever reads the listing"
        );
        assert_eq!(
            PaneName::parse("build\ttest").unwrap_err(),
            PaneNameError::Control,
            "a tab shifts columns in a listing whose columns are read positionally"
        );
        assert_eq!(
            PaneName::parse("build\u{7}").unwrap_err(),
            PaneNameError::Control,
            "a BEL rings the reader's terminal"
        );
    }

    #[test]
    fn a_name_that_is_all_digits_is_refused_because_a_number_means_something_else() {
        assert_eq!(PaneName::parse("3").unwrap_err(), PaneNameError::Numeric);
        assert_eq!(PaneName::parse(" 12 ").unwrap_err(), PaneNameError::Numeric);
        // The rule is about a name that is INDISTINGUISHABLE from a number, so a name that merely
        // starts or ends with digits is fine — nothing reads `3rd` as an ordinal.
        assert_eq!(PaneName::parse("3rd").unwrap().as_str(), "3rd");
        assert_eq!(PaneName::parse("pane-3").unwrap().as_str(), "pane-3");
        // Not ASCII digits, so not a number to any parser that would take one.
        assert_eq!(PaneName::parse("٣").unwrap().as_str(), "٣");
    }

    #[test]
    fn the_rules_are_checked_in_an_order_a_caller_can_act_on() {
        // A blank over-length string is EMPTY, not TooLong: trimming happens first, so the caller
        // is told the thing they can fix rather than a length they did not send.
        assert_eq!(
            PaneName::parse(&" ".repeat(200)).unwrap_err(),
            PaneNameError::Empty
        );
    }

    #[test]
    fn a_name_read_back_from_a_snapshot_obeys_the_same_rules() {
        assert_eq!(
            serde_json::from_str::<PaneName>("\"build\"")
                .unwrap()
                .as_str(),
            "build"
        );
        // A file an older daemon wrote — or one somebody edited — cannot smuggle in a name the
        // front door refuses.
        let forged = serde_json::from_str::<PaneName>("\"build\\n8: 80x24  sh\"");
        assert!(
            forged.is_err(),
            "a snapshot is a file, and a file is not a trusted input"
        );
        assert!(serde_json::from_str::<PaneName>("\"7\"").is_err());
        assert!(serde_json::from_str::<PaneName>("\"\"").is_err());
    }

    #[test]
    fn a_name_serialises_as_its_bare_string() {
        let name = PaneName::parse("build").unwrap();
        assert_eq!(serde_json::to_string(&name).unwrap(), "\"build\"");
    }

    #[test]
    fn every_refusal_says_what_is_wrong_with_the_name_that_was_sent() {
        // Rendered, not just constructed: these sentences reach an operator through the CLI.
        assert_eq!(
            PaneNameError::Empty.to_string(),
            "a pane name cannot be blank"
        );
        assert_eq!(
            PaneNameError::TooLong(81).to_string(),
            "a pane name is at most 80 bytes, and that one is 81"
        );
        assert!(
            PaneNameError::Control.to_string().contains("newline"),
            "the control refusal names the concrete hazard, not the character class"
        );
        assert!(
            PaneNameError::Numeric.to_string().contains("pane number"),
            "the numeric refusal names what the name would collide WITH"
        );
    }
}
