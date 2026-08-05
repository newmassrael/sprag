//! How a caller NAMES the pane it means — the one grammar every surface resolves.
//!
//! # Why this is a module and not a helper in each binary
//!
//! A pane NAME is an ADDRESS. [`PaneName`](sprag_terminal::PaneName) is validated and unique
//! across the whole registry (R295) precisely so a caller can hold a handle that does not move,
//! and R311 made it reach across a window. An address only some doors accept is not an address,
//! though — it is a convenience some functions happen to implement, and that is what R312 measured
//! at `e7be5eb`: of the agent surface's eighteen pane-addressed tools **seven reached a pane one
//! window over and eleven refused**, and of the CLI's pane-taking verbs **none accepted a name at
//! all**, refusing in SIX different sentences:
//!
//! ```text
//! capture-pane faraway    -> pane id "faraway" must be a number
//! processes faraway       -> "faraway" is not a pane id
//! select-pane faraway     -> "faraway" is neither a direction flag nor a pane id
//! zoom-pane faraway       -> "faraway" is neither a flag nor a pane id
//! agent faraway           -> "faraway" is neither -t nor a pane id
//! find zz --pane faraway  -> --pane "faraway" is not a pane id (a number)
//! ```
//!
//! One cause: **a pane address was parsed and resolved independently at every entry point**, so
//! which doors took a name was an accident of when each was written. Six spellings of one rule is
//! the same shape as the FIFTH hand-built copy of a request's keys R300's audit found, and the
//! remedy is the same — the rule gets one home and the doors call it.
//!
//! # What is shared here, and what deliberately is not
//!
//! A NAME means the same thing at every surface: the pane carrying it, in any window of the
//! session. A NUMBER does not — the CLI's is a registry-unique host id and the agent surface's is
//! a 1-based position in the caller's own window listing. Both meanings are load-bearing for
//! callers that already exist, so this module shares exactly the NAME half plus the rule that
//! tells the two apart, and each surface keeps its own numeric meaning and its own numeric
//! refusal.

use std::fmt;

/// How a caller spelled the pane it means.
///
/// The two arms are told apart by ONE rule, and it is not invented here:
/// [`PaneName::parse`](sprag_terminal::PaneName::parse) refuses an all-digit name so that a single
/// argument can carry both spellings without a mode flag. A surface whose arguments are typed
/// (JSON) gets that discrimination for free; a surface whose arguments are all strings (a shell)
/// needs [`PaneAddress::parse`] to make the same decision the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneAddress {
    /// A number, whose MEANING belongs to the surface that read it — a host id in `sprag`, a
    /// 1-based position in the agent surface's `list_panes`. Kept as the raw digits' value so the
    /// surface can interpret it; this module never resolves one.
    Number(u64),
    /// A pane's NAME, which addresses the same pane at every surface and in any window of the
    /// session.
    Name(String),
}

impl PaneAddress {
    /// Read a raw argument as an address.
    ///
    /// All digits (and non-empty, and in `u64` range) is a NUMBER; anything else is a NAME. A name
    /// is trimmed here as well as in the daemon, so `" build "` resolves rather than reporting
    /// that no pane is called that — this applies no rule the daemon does not, and a name that
    /// breaks one simply matches nothing.
    ///
    /// An all-digit token that OVERFLOWS `u64` reads as a NAME rather than as a broken number:
    /// no pane can be called that (a name is at most 80 bytes but this one is a legal `PaneName`
    /// shape), so it resolves to nothing and is refused by the sentence that lists what does
    /// exist — which tells a caller more than "out of range" would.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        let trimmed = raw.trim();
        match trimmed.parse::<u64>() {
            // `str::parse::<u64>` accepts a leading `+`, which is not what any surface here means
            // by a number and IS a legal name shape, so the digits are checked explicitly.
            Ok(number) if trimmed.bytes().all(|byte| byte.is_ascii_digit()) => Self::Number(number),
            _ => Self::Name(trimmed.to_owned()),
        }
    }

    /// The NAME this address carries, or [`None`] when it is a number.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            Self::Number(_) => None,
        }
    }
}

impl fmt::Display for PaneAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Number(number) => write!(f, "{number}"),
            Self::Name(name) => write!(f, "{name:?}"),
        }
    }
}

/// A pane that HAS a name, and the window it is in — what a refusal lists so the caller can pick a
/// real one.
///
/// The window rides along because it is the half a caller cannot otherwise reach: a name that
/// missed in `list_panes` is very often a real pane one window over, and naming the window turns
/// "no such pane" into "here is where it is".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NamedPane {
    /// The name the pane carries.
    pub name: String,
    /// The window holding it.
    pub window: String,
}

impl NamedPane {
    /// A named pane in a window.
    pub fn new(name: impl Into<String>, window: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            window: window.into(),
        }
    }
}

/// How a refusal tells the caller where to look — the one listing that shows what it is offering.
///
/// The two surfaces publish the session's named panes under different verbs, and a sentence that
/// named the wrong one would send a caller to a command they do not have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneListing {
    /// The agent surface's `list_windows`.
    ListWindows,
    /// The CLI's `sprag panes -t SESSION`, which lists a window at a time, so the sentence points
    /// at the window listing beside it.
    SpragPanes,
}

impl PaneListing {
    /// The sentence fragment that sends a caller to this listing.
    fn advice(self) -> &'static str {
        match self {
            Self::ListWindows => "Call list_windows.",
            Self::SpragPanes => "Run `sprag windows` and `sprag panes` to see them.",
        }
    }
}

/// The sentence for a name no pane in the session carries.
///
/// It says three things, and each was measured missing from a real refusal: WHICH name failed,
/// what names DO exist and which windows hold them, and where to see the rest. R311's predecessor
/// said *"no pane is called \"buildout\"; no pane in this terminal has a name yet"* about a
/// terminal that had one — both halves false — because it looked only in the caller's own window.
#[must_use]
pub fn unknown_pane_name(name: &str, known: &[NamedPane]) -> String {
    if known.is_empty() {
        return format!("no pane is called {name:?}; no pane in this session has a name yet.");
    }
    format!(
        "no pane is called {name:?}; the session's named panes are {}.",
        list_named(known),
    )
}

/// [`unknown_pane_name`] with the listing to go and read appended — the form a surface uses when
/// it has a verb to offer.
#[must_use]
pub fn unknown_pane_name_with(name: &str, known: &[NamedPane], listing: PaneListing) -> String {
    let mut out = unknown_pane_name(name, known);
    out.push(' ');
    out.push_str(listing.advice());
    out
}

/// The sentence for a name more than one pane answers to.
///
/// The daemon holds names unique across itself, so a second bearer cannot arise from a correct
/// sequence of requests — but the uniqueness check and the write are not one atomic step there, so
/// every surface REFUSES rather than taking the first. Silently resolving an ambiguous name would
/// rebuild the very failure a name exists to remove: a plausible answer against the wrong pane.
#[must_use]
pub fn ambiguous_pane_name(name: &str, bearers: &[NamedPane]) -> String {
    format!(
        "more than one pane is called {name:?} ({}), so it does not name one pane. Rename one and \
         try again.",
        list_named(bearers),
    )
}

/// `"a" (window 0), "b" (window 1)` — the shared rendering of a candidate list, so the two
/// sentences above cannot come to disagree about how a pane is written down.
fn list_named(panes: &[NamedPane]) -> String {
    panes
        .iter()
        .map(|pane| format!("{:?} (window {})", pane.name, pane.window))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule that lets one argument carry both spellings, checked at both edges — and against
    /// the daemon's own, which is what makes them one rule rather than two that agree today.
    #[test]
    fn digits_are_a_number_and_everything_else_is_a_name() {
        assert_eq!(PaneAddress::parse("3"), PaneAddress::Number(3));
        assert_eq!(PaneAddress::parse("0"), PaneAddress::Number(0));
        assert_eq!(
            PaneAddress::parse("build"),
            PaneAddress::Name("build".to_owned())
        );
        // The shapes that look numeric and are not: a name may hold digits, just not only digits.
        for name in ["3b", "b3", "-1", "1.0", "+3", " 3 x", "3_"] {
            assert_eq!(
                PaneAddress::parse(name),
                PaneAddress::Name(name.trim().to_owned()),
                "{name:?} is a NAME",
            );
        }
        // The daemon's rule is the same rule: an all-digit name is refused there, which is the
        // whole reason the split above is safe.
        assert!(
            sprag_terminal::PaneName::parse("3").is_err(),
            "an all-digit name is refused by the daemon, so a number can never shadow one",
        );
        assert!(sprag_terminal::PaneName::parse("3b").is_ok());
    }

    /// A trimmed name resolves — the daemon trims on the way in, so a surface that did not would
    /// report that no pane is called something the daemon calls exactly that.
    #[test]
    fn a_name_is_trimmed_the_way_the_daemon_trims_it() {
        assert_eq!(
            PaneAddress::parse("  build  "),
            PaneAddress::Name("build".to_owned())
        );
        assert_eq!(PaneAddress::parse("  7  "), PaneAddress::Number(7));
    }

    /// An all-digit token too large for `u64` is a NAME, not an error — see [`PaneAddress::parse`].
    #[test]
    fn an_overflowing_digit_string_is_a_name_rather_than_a_broken_number() {
        let huge = "99999999999999999999999999";
        assert_eq!(
            PaneAddress::parse(huge),
            PaneAddress::Name(huge.to_owned()),
            "it resolves to nothing and is refused by the sentence that lists what exists",
        );
    }

    /// The refusal names the candidates AND their windows — the half a caller cannot otherwise
    /// reach, and the half whose absence made R311's predecessor say something false.
    #[test]
    fn the_unknown_sentence_names_where_the_real_panes_are() {
        let known = [
            NamedPane::new("buildout", "1"),
            NamedPane::new("tests", "docs"),
        ];
        let sentence = unknown_pane_name_with("buildout ", &known, PaneListing::ListWindows);
        assert_eq!(
            sentence,
            "no pane is called \"buildout \"; the session's named panes are \"buildout\" \
             (window 1), \"tests\" (window docs). Call list_windows.",
        );
        assert_eq!(
            unknown_pane_name_with("x", &[], PaneListing::SpragPanes),
            "no pane is called \"x\"; no pane in this session has a name yet. Run `sprag windows` \
             and `sprag panes` to see them.",
        );
    }

    /// An ambiguous name is refused with the bearers named, not resolved to the first.
    #[test]
    fn the_ambiguous_sentence_names_every_bearer() {
        let bearers = [NamedPane::new("build", "0"), NamedPane::new("build", "2")];
        assert_eq!(
            ambiguous_pane_name("build", &bearers),
            "more than one pane is called \"build\" (\"build\" (window 0), \"build\" (window 2)), \
             so it does not name one pane. Rename one and try again.",
        );
    }
}
