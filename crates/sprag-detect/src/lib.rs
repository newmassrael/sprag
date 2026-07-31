//! sprag-detect — what an agent pane is DOING, derived from its screen and its title.
//!
//! H3 slice 1. sprag can already tell you a pane exists, how big it is, and whether its child
//! announced something; it cannot tell you that the agent in it is waiting on you. This crate is
//! the pure half of that answer: a rule engine over a [`Screen`] and an OSC title, with no lock, no
//! wire shape and no clock.
//!
//! ## Why pure, and why here
//!
//! `sprag-grid` is the precedent and the analogy is exact — a one-directional
//! read of a `sprag_vt::Screen` that owns nothing. Purity is what makes a verdict testable from a
//! synthetic screen instead of from a live agent, which matters more here than anywhere: the rules
//! encode somebody else's UI, and the only way to keep them honest is to be able to replay a
//! captured screen and assert on it.
//!
//! Hysteresis, the per-pane memory and the publish seam are deliberately NOT here. They need a
//! clock and a place to remember, and both belong to the caller (slice 2).
//!
//! ## What the design rests on, measured rather than assumed (R249)
//!
//! Every rule in [`claude`]'s built-in manifest comes from driving a real agent in a real pane:
//!
//! * **The title's LEADING GLYPH is the state, and its suffix is not.** A working pane's title
//!   begins with a braille spinner frame; an idle one begins with `✳`. The rest of the title is a
//!   task summary that PERSISTS after the task ends, so a rule reading it reports a state from a
//!   sentence that is merely stale.
//! * **The title cannot separate `Blocked` from `Idle`.** A real mid-session permission dialog was
//!   measured showing the `✳` glyph — the idle one — beside a suffix naming a task that had already
//!   finished. That is the whole reason [`Rule::priority`] exists and why the blocked rule outranks
//!   the idle one: on a blocked pane BOTH match, and the answer that helps a person must win.
//! * **A dialog is a bottom-anchored numbered choice list.** Three independent dialogs (first-run
//!   trust, a slash-command picker, a tool permission request) share exactly one shape: a selection
//!   marker `❯` followed by `<digit>.` at the start of a line, with at least one more numbered
//!   option below it. Nothing in an agent's transcript output has that shape.
//!
//! ## What a SECOND agent changed, and what it left standing (R251)
//!
//! One agent is a sample, not a vocabulary, so `codex` was driven the same way before this
//! vocabulary was allowed to freeze. It is an independent implementation — Rust and `ratatui`
//! against `claude`'s TypeScript and Ink — and it moved three things:
//!
//! * **The selection marker is not a constant.** Three markers were measured across the two
//!   agents: `❯` (U+276F) in `claude`, `>` (U+003E) in `codex`'s sign-in picker, and `›` (U+203A)
//!   in `codex`'s directory-trust dialog. Two of those are inside ONE agent, so the marker is not
//!   even a per-agent constant and [`dialog_pattern`] matches the measured CLASS. Everything else
//!   about the shape held: marker, `<digit>.`, and at least one more numbered option below.
//! * **A fingerprint needs conjunction, which is why [`Fingerprint`] exists.** `claude` is
//!   identified by one string in a fixed footer. `codex` has no such string at any width: its
//!   footer is `<model> <effort> · <cwd>`, every part of which is configurable, and the one banner
//!   naming the product scrolls out of the transcript. It is identifiable only as a composer line
//!   AND a footer shape together — the same argument [`Rule::all`] already makes, arriving one
//!   level up.
//! * **The braille spinner generalised, and the resting glyph did not.** `codex`'s working title
//!   is a braille frame exactly as `claude`'s is, and [`spinner_pattern`]'s range matched four
//!   frames no `claude` probe ever produced — which is what writing the pattern for the animation
//!   rather than for the two frames observed was for. But `codex` has no resting glyph at all: at
//!   rest its title is the bare working-directory name, indistinguishable from a shell's. Its idle
//!   rule is therefore the lowest-priority FALLBACK, taking its specificity from [`Rule::priority`]
//!   rather than from a pattern — a distinction a revert-proof had to settle, because the negation
//!   it was first written as could be widened to match anything without turning a test red.
//!
//! What survived unchanged: [`Region`]'s two variants, [`Test`]'s three, [`Rule::priority`], and
//! the twelve-row window — `codex`'s two dialogs put their marker 3 and 7 non-empty rows up, inside
//! the spread the window was already sized for.
//!
//! ## The bounds this slice ships with, stated rather than discovered later
//!
//! A pane is identified as an agent's by [`Manifest::any`], and every fingerprint needs the pane to
//! have painted something recognisable. Two measured misses follow from that, and both read
//! [`AgentState::Unknown`] rather than the true state:
//!
//! * A first-run trust dialog arrives BEFORE the agent has set a title and while its footer is
//!   replaced by the dialog. Measured in BOTH agents, so it is a property of onboarding rather than
//!   a quirk of one — which also means a title-free fingerprint is worth more than it looked.
//! * `codex` shows a transient hint in place of its footer for a few seconds after `esc`, and its
//!   fingerprint is a footer conjunction, so it goes unclaimed for exactly that window.
//!
//! Both are recorded here, and asserted by tests, so the next author does not rediscover them from
//! a bug report.

use regex::Regex;
use sprag_vt::Screen;

/// What an agent pane is doing, as a person would describe it.
///
/// The vocabulary is deliberately small. [`Blocked`](Self::Blocked) is the state this whole front
/// exists for — the one that means "come back to me" — and [`Unknown`](Self::Unknown) is a real
/// answer rather than a failure: "this is not an agent" and "this agent wants you" are opposite
/// instructions to somebody reading a pane list, so they must not collapse into one another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentState {
    /// No manifest claimed the pane, or one did and none of its rules matched.
    #[default]
    Unknown,
    /// The agent is running: thinking, calling a tool, or printing.
    Working,
    /// The agent has ASKED something and cannot continue until it is answered.
    Blocked,
    /// The agent is at rest, waiting for input it has not asked for.
    Idle,
}

impl AgentState {
    /// The wire / display token for a KNOWN state, or `None` for
    /// [`Unknown`](Self::Unknown) — the single source of the vocabulary, so a serializer omits the
    /// key rather than inventing a spelling for "no answer" (the additive shape every other
    /// per-pane fact on sprag's pane list already uses).
    #[must_use]
    pub const fn wire_str(self) -> Option<&'static str> {
        match self {
            Self::Unknown => None,
            Self::Working => Some("working"),
            Self::Blocked => Some("blocked"),
            Self::Idle => Some("idle"),
        }
    }
}

/// Which text a [`Test`] reads.
///
/// Both variants come from a measurement rather than from a rival's list. A region addressing the
/// middle of a screen is not here because nothing observed needed one, and a vocabulary is easier
/// to widen than to narrow.
#[derive(Debug, Clone)]
pub enum Region {
    /// The pane's OSC window title, or the empty string when it has set none.
    ///
    /// Empty rather than skipped so a manifest cannot accidentally match a pane that has no title
    /// at all: `starts_with` and a `^`-anchored regex both fail on `""`, which is the safe
    /// direction. A `Contains` test with an empty needle would match, and that is the manifest
    /// author's error rather than something this can prevent.
    Title,
    /// The last `n` NON-EMPTY rows of the visible screen, joined bottom-up into reading order.
    ///
    /// Non-empty rather than simply the last `n` rows because a dialog's height varies and an
    /// agent pads its layout with blanks — measured at 5 to 12 rows across three real dialogs — so
    /// counting raw rows would make the window's usefulness depend on the pane's height.
    BottomLines(u16),
}

/// How a [`Region`]'s text is judged.
///
/// `Regex` carries a COMPILED pattern, so a manifest that cannot compile is rejected where it is
/// built rather than failing silently on every pane forever after. That is the same choice
/// `sprag-host`'s keymap makes for a key spec: parse once, at the edge.
#[derive(Debug, Clone)]
pub enum Test {
    /// The region's text begins with this.
    StartsWith(String),
    /// The region's text contains this anywhere.
    Contains(String),
    /// The region's text matches this pattern. Multi-line by convention — use `(?m)` and `^` to
    /// anchor to a row, which is what a bottom-anchored dialog rule wants.
    Regex(Regex),
}

/// One region-and-test pair — the atom both a fingerprint and a rule are built from.
#[derive(Debug, Clone)]
pub struct Match {
    /// Which text to read.
    pub region: Region,
    /// How to judge it.
    pub test: Test,
}

impl Match {
    /// A convenience constructor, so a manifest reads as a table rather than as a struct literal
    /// repeated forty times.
    #[must_use]
    pub const fn new(region: Region, test: Test) -> Self {
        Self { region, test }
    }

    /// Whether this holds for the given screen and title.
    fn holds(&self, screen: &Screen, title: &str) -> bool {
        let text = match &self.region {
            Region::Title => title.to_owned(),
            Region::BottomLines(n) => bottom_lines(screen, *n),
        };
        match &self.test {
            Test::StartsWith(needle) => text.starts_with(needle.as_str()),
            Test::Contains(needle) => text.contains(needle.as_str()),
            Test::Regex(pattern) => pattern.is_match(&text),
        }
    }
}

/// One piece of independent evidence that a pane belongs to an agent.
///
/// A fingerprint is a CONJUNCTION for the same reason [`Rule::all`] is: an author makes it specific
/// by adding a second readable condition rather than by folding both into one regex that spans
/// rows. That this is needed at all is a measurement rather than a symmetry — `claude` is
/// identified by a single string in a fixed footer, but `codex` has no constant string anywhere on
/// its screen, and is recognisable only as its composer line AND the shape of its footer together.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    /// Every match that must hold for this fingerprint to claim the pane.
    ///
    /// An EMPTY list holds vacuously and so claims every pane. That is the manifest author's error
    /// rather than something this can prevent, exactly as an empty [`Test::Contains`] needle is —
    /// stated here because the failure is silent and total.
    pub all: Vec<Match>,
}

impl Fingerprint {
    /// A fingerprint that is one match — the common case, and the shape every `claude` fingerprint
    /// still has.
    #[must_use]
    pub fn one(m: Match) -> Self {
        Self { all: vec![m] }
    }

    /// A fingerprint that needs several matches at once.
    #[must_use]
    pub const fn all(all: Vec<Match>) -> Self {
        Self { all }
    }

    /// Whether every match holds.
    fn holds(&self, screen: &Screen, title: &str) -> bool {
        self.all.iter().all(|m| m.holds(screen, title))
    }
}

/// One state conclusion, its evidence, and how strongly it outranks a competing one.
#[derive(Debug, Clone)]
pub struct Rule {
    /// A stable name for this rule, carried out on the [`Verdict`].
    ///
    /// This is not decoration. A rule engine that answers "Working" and cannot say why is the
    /// silent gate this project has already paid for twice, and a manifest author debugging without
    /// an answer writes worse manifests. Because the id rides the verdict the detector already
    /// produced, an `explain` surface can never be a second code path that disagrees with the
    /// first.
    pub id: String,
    /// What this rule concludes.
    pub state: AgentState,
    /// Every match that must hold — ALL of them, so a rule can be made specific by conjunction
    /// rather than by writing one unreadable regex.
    pub all: Vec<Match>,
    /// Higher wins. Ties are broken by declaration order, so a manifest's own layout is the
    /// tie-break a reader can see.
    ///
    /// This exists because of a measurement: on a blocked pane the blocked rule and the idle rule
    /// BOTH match, since the title's glyph is identical in the two states.
    pub priority: i32,
}

/// One agent's fingerprints and rules.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// The agent's name, carried out on the [`Verdict`].
    pub name: String,
    /// Fingerprints that identify a pane as this agent's — ANY one is enough.
    ///
    /// Any rather than all, because identification wants independent evidence: a pane mid-dialog
    /// shows none of what a pane at rest shows, and requiring every fingerprint would mean the
    /// agent is only recognised in the state that happens to show all of them. The field is named
    /// for its semantics so the difference from [`Rule::all`] is visible at the use site.
    ///
    /// Each element is itself a conjunction, so this is a disjunction of conjunctions rather than
    /// of single matches. The extra level is not symmetry for its own sake: `codex` cannot be
    /// identified by any one condition on its screen, and flattening it back would put that
    /// conjunction inside a regex spanning rows.
    pub any: Vec<Fingerprint>,
    /// The state rules, in declaration order. Arbitration is by [`Rule::priority`] first.
    pub rules: Vec<Rule>,
}

/// A state, with the evidence that produced it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Verdict {
    /// What the pane is doing.
    pub state: AgentState,
    /// Which manifest claimed the pane, `None` when none did.
    pub agent: Option<String>,
    /// Which [`Rule::id`] fired, `None` when a manifest claimed the pane but no rule matched.
    pub rule: Option<String>,
}

/// Read a pane's state from its `screen` and its OSC `title`, against `manifests` in order.
///
/// The two inputs are separate parameters rather than one bundled pane, because they are exactly
/// the two things a caller must compare to know whether re-running this could change the answer —
/// which is what makes slice 2's quiescence gate an EXACT skip rather than a heuristic one. A
/// bundle would hide that.
///
/// The FIRST manifest whose fingerprints match claims the pane; later manifests are not consulted.
/// Ordering is therefore meaningful and belongs to whoever assembles the list, which is how a user
/// manifest will layer over a built-in one in slice 4 without this function learning about files.
///
/// A manifest that claims a pane but matches no rule yields [`AgentState::Unknown`] WITH the agent
/// named — "I know what this is and not what it is doing" is a different fact from "I do not know
/// what this is", and a person debugging a manifest needs to tell them apart.
#[must_use]
pub fn detect(screen: &Screen, title: Option<&str>, manifests: &[Manifest]) -> Verdict {
    let title = title.unwrap_or_default();
    let Some(manifest) = manifests
        .iter()
        .find(|manifest| manifest.any.iter().any(|fp| fp.holds(screen, title)))
    else {
        return Verdict::default();
    };
    // Highest priority wins; `max_by_key` on a plain iterator returns the LAST maximum, so the
    // order is reversed to make it the FIRST — declaration order is the documented tie-break, and
    // a reader should not have to know which end of the iterator the tie fell off.
    let fired = manifest
        .rules
        .iter()
        .rev()
        .filter(|rule| rule.all.iter().all(|m| m.holds(screen, title)))
        .max_by_key(|rule| rule.priority);
    Verdict {
        state: fired.map_or(AgentState::Unknown, |rule| rule.state),
        agent: Some(manifest.name.clone()),
        rule: fired.map(|rule| rule.id.clone()),
    }
}

/// The last `n` non-empty rows of the visible screen, in reading order.
///
/// "Non-empty" is after trimming, so a row of spaces — which is what an agent's own box drawing
/// leaves behind — does not consume the window a dialog needs.
fn bottom_lines(screen: &Screen, n: u16) -> String {
    let mut rows: Vec<String> = Vec::with_capacity(n as usize);
    for row in (0..screen.rows()).rev() {
        if rows.len() == n as usize {
            break;
        }
        let text = screen.row_text(row);
        if !text.trim().is_empty() {
            rows.push(text);
        }
    }
    rows.reverse();
    rows.join("\n")
}

/// The regex a dialog is recognised by, shared by [`claude`] and by the tests that prove what it
/// does and does not match.
///
/// Two halves, and the second is the load-bearing one: the selection marker on a numbered option,
/// and at least one MORE numbered option below it. Without the second half a user's own typed line
/// — `❯ 1. do the thing` echoed into the prompt box — reads as a dialog, because a prompt echo and
/// a choice list's first row are the same string. A choice list always offers more than one choice;
/// an echo is one line. That is the difference the pattern is written to see.
///
/// The marker is a CLASS, not a literal, and that is a measurement rather than caution: `claude`
/// marks with `❯` (U+276F) while `codex` marks its sign-in picker with `>` (U+003E) and its trust
/// dialog with `›` (U+203A). Two markers inside one agent means the marker is not a per-agent
/// constant either, so a rule keyed on the glyph one probe happened to see is a rule about that
/// probe.
pub fn dialog_pattern() -> Regex {
    Regex::new(r"(?m)^\s*[❯›>]\s+\d+\.[\s\S]*?^\s*\d+\.").expect("a literal pattern compiles")
}

/// The Braille Patterns block, which every frame of the spinner is drawn from.
///
/// The block rather than the two frames that were observed: matching the glyphs seen would be a
/// rule about one run of the animation instead of about the animation.
pub fn spinner_pattern() -> Regex {
    Regex::new(r"^[\u{2800}-\u{28FF}]").expect("a literal range compiles")
}

/// The glyph the agent shows when it is NOT working — measured identical in the idle and blocked
/// states, which is why it cannot be the whole answer.
const RESTING_GLYPH: &str = "✳";

/// The built-in manifest for Anthropic's `claude` CLI, derived from R249's measurements.
///
/// # Panics
///
/// Panics if a pattern here fails to compile, which is a build-time error in this file and cannot
/// depend on input. Compiling at the edge is the point: a manifest that cannot compile must fail
/// where it is written, not silently never match on every pane forever.
#[must_use]
pub fn claude() -> Manifest {
    let spinner = spinner_pattern();
    Manifest {
        name: "claude".to_owned(),
        any: vec![
            // The idle glyph, which is distinctive enough to be a fingerprint on its own...
            Fingerprint::one(Match::new(
                Region::Title,
                Test::StartsWith(RESTING_GLYPH.to_owned()),
            )),
            // ...and the spinner, so a pane that is busy the first time it is looked at is still
            // recognised. Two fingerprints covering the two title shapes.
            Fingerprint::one(Match::new(Region::Title, Test::Regex(spinner.clone()))),
            // The footer, for the window between states where the title is momentarily neither.
            // Each of these is one condition, which is what an agent with a constant string in a
            // fixed footer affords -- see `codex` for the case that does not.
            Fingerprint::one(Match::new(
                Region::BottomLines(4),
                Test::Contains("? for shortcuts".to_owned()),
            )),
        ],
        rules: vec![
            Rule {
                id: "dialog-choice-list".to_owned(),
                state: AgentState::Blocked,
                // Twelve, because a dialog is bottom-anchored but VARIES in height: the three
                // measured ones put their selection marker 5, 8 and 9 non-empty rows from the
                // bottom. A window sized to the smallest would miss the others, and a window
                // sized to the pane would make the rule depend on how tall the pane is.
                all: vec![Match::new(
                    Region::BottomLines(12),
                    Test::Regex(dialog_pattern()),
                )],
                // Above `idle-glyph` because BOTH match on a blocked pane -- the measured fact
                // this whole ordering exists for.
                priority: 30,
            },
            Rule {
                id: "spinner-glyph".to_owned(),
                state: AgentState::Working,
                all: vec![Match::new(Region::Title, Test::Regex(spinner))],
                priority: 20,
            },
            Rule {
                id: "idle-glyph".to_owned(),
                state: AgentState::Idle,
                all: vec![Match::new(
                    Region::Title,
                    Test::StartsWith(RESTING_GLYPH.to_owned()),
                )],
                priority: 10,
            },
        ],
    }
}

/// `codex`'s composer line — its marker followed by a space, whatever the user has typed after it.
///
/// Measured empty (`› Write tests for @filename`, a placeholder), mid-typing (`› x`) and holding a
/// submitted message (`› hi`). It is the one row `codex` paints in every steady state.
fn codex_composer_pattern() -> Regex {
    Regex::new(r"(?m)^›\s").expect("a literal pattern compiles")
}

/// `codex`'s footer — `<model> <effort> · <absolute path>`.
///
/// Matched as a SHAPE rather than by any of its words, because all three parts vary: the model and
/// the effort are configurable and the path is the user's. Measured identical at 80 and at 160
/// columns, so it is the footer itself rather than a truncation artefact of one probe width.
fn codex_footer_pattern() -> Regex {
    Regex::new(r"(?m)^\s*\S+\s+\S+\s+·\s+/").expect("a literal pattern compiles")
}

/// A title with anything in it at all.
///
/// `codex` has no resting glyph — at rest its title is the bare working-directory name, exactly
/// what a shell puts there — so its idle rule cannot read a glyph the way `claude`'s does. What it
/// must NOT do is state the absence of the spinner as a pattern of its own: [`Rule::priority`]
/// already separates the two, and a second spelling of the same fact is one that can silently
/// disagree with [`spinner_pattern`] the day either is edited. A revert-proof is what settled it —
/// written as a negation, the character class could be widened to match anything without turning a
/// single test red, which is the definition of a mechanism that is not there.
///
/// So the pattern carries only what is genuinely its own: the pane has painted a title. An empty
/// one fails, which is the safe direction — a pane that has said nothing is not asserted to be at
/// rest.
fn painted_title_pattern() -> Regex {
    Regex::new(r"\S").expect("a literal pattern compiles")
}

/// The built-in manifest for OpenAI's `codex` CLI, derived from R251's measurements.
///
/// The second agent, and the reason the vocabulary above is trusted to be about agents rather than
/// about `claude`. Its rules are structurally `claude`'s — the same dialog shape, the same braille
/// spinner — and its FINGERPRINT is where the two genuinely differ.
///
/// # Panics
///
/// Panics if a pattern here fails to compile, which is a build-time error in this file and cannot
/// depend on input.
#[must_use]
pub fn codex() -> Manifest {
    let spinner = spinner_pattern();
    Manifest {
        name: "codex".to_owned(),
        // ONE fingerprint, and it is a conjunction, because no single condition on a `codex` screen
        // is both stable and specific: the composer marker alone is a character a shell prompt may
        // well use, and the footer shape alone is three tokens and a path. Together they are the
        // agent. This is the case `Fingerprint` was added for.
        any: vec![Fingerprint::all(vec![
            Match::new(
                Region::BottomLines(3),
                Test::Regex(codex_composer_pattern()),
            ),
            Match::new(Region::BottomLines(1), Test::Regex(codex_footer_pattern())),
        ])],
        rules: vec![
            // The same rule as `claude`'s, matching a `codex` dialog through the widened marker
            // class. It is written from a MEASURED mid-session picker, and the pane it was measured
            // on reads `Unknown` today because the modal covers the footer the fingerprint needs --
            // see the crate docs. The rule is correct and the identification is what is missing,
            // which is asserted rather than described in
            // `codex_dialog_matches_the_rule_although_the_pane_is_not_claimed`.
            Rule {
                id: "dialog-choice-list".to_owned(),
                state: AgentState::Blocked,
                all: vec![Match::new(
                    Region::BottomLines(12),
                    Test::Regex(dialog_pattern()),
                )],
                priority: 30,
            },
            Rule {
                id: "spinner-glyph".to_owned(),
                state: AgentState::Working,
                all: vec![Match::new(Region::Title, Test::Regex(spinner))],
                priority: 20,
            },
            // `claude`'s idle rule reads a glyph that is THERE; `codex` has none, so this one is
            // the LOWEST-priority fallback and takes its specificity from the ordering rather than
            // from its pattern. The same measured reason applies: on a blocked `codex` pane the
            // title is the resting one, so this rule and the dialog rule both match.
            Rule {
                id: "no-working-signal".to_owned(),
                state: AgentState::Idle,
                all: vec![Match::new(
                    Region::Title,
                    Test::Regex(painted_title_pattern()),
                )],
                priority: 10,
            },
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::{Emulator, VtPort};

    /// An ordinary pane, with `lines` painted into it top-down.
    ///
    /// Where the text SITS does not change what [`bottom_lines`] returns, because it skips blank
    /// rows — so a fixture only has to say which rows have content, in order. The one test that
    /// cares about distance from the bottom says so by including the rows below.
    fn painted(lines: &[&str]) -> Emulator {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    fn verdict(lines: &[&str], title: Option<&str>) -> Verdict {
        let em = painted(lines);
        detect(em.screen(), title, &[claude()])
    }

    // ── The four screens below were CAPTURED from a real `claude` in a real sprag pane
    //    (R249's probes), not composed. They are the reason this crate can be trusted about
    //    somebody else's UI at all: a rule written from a description would pass tests written
    //    from the same description.

    /// Idle: the prompt box empty, the footer showing. Title `✳ Claude Code`.
    const IDLE: &[&str] = &[
        "✻ Crunched for 37s",
        "────────────────────────────────────────────────────────────────────────────────",
        "❯",
        "────────────────────────────────────────────────────────────────────────────────",
        "  ⚠ Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker · res…",
        "  ⏸ manual mode on · ? for shortcuts",
    ];

    /// A tool PERMISSION request — the measurement slice 1 was blocked on.
    const PERMISSION_DIALOG: &[&str] = &[
        "────────────────────────────────────────────────────────────────────────────────",
        " Fetch",
        "   url: \"https://example.com\", prompt: \"What is the exact text inside the",
        "   page's <title> tag? Report it verbatim.\"",
        "   Claude wants to fetch content from example.com",
        " Do you want to allow Claude to fetch this content?",
        " ❯ 1. Yes",
        "   2. Yes, and don't ask again for example.com",
        "   3. No, and tell Claude what to do differently (esc)",
    ];

    /// The first-run trust dialog — a SECOND independent dialog, so the shape the rule matches is
    /// an invariant of the agent's UI rather than of one screen.
    const TRUST_DIALOG: &[&str] = &[
        "────────────────────────────────────────────────────────────────────────────────",
        " Accessing workspace:",
        " /tmp/h3-agent",
        " Quick safety check: Is this a project you created or one you trust?",
        " Claude Code'll be able to read, edit, and execute files here.",
        " ❯ 1. Yes, I trust this folder",
        "   2. No, exit",
        " Enter to confirm · Esc to cancel",
    ];

    /// A slash-command picker — a THIRD dialog, and the tallest measured, which is what sized
    /// `BottomLines(12)`.
    const MODEL_PICKER: &[&str] = &[
        "▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔",
        "   Select model",
        "   Switch between Claude models. Your pick becomes the default for new",
        "   sessions. For other/previous model names, specify with --model.",
        "     1. Default (recommended)  Opus 5 with 1M context · Best for everyday,",
        "                               complex tasks",
        "   ❯ 2. Opus (1M context) ✔    Opus 5 with 1M context · Best for everyday,",
        "                               complex tasks",
        "     3. Fable                  Fable 5 · Most capable for your hardest and",
        "     4. Sonnet                 Sonnet 5 · Efficient for routine tasks",
        "     5. Haiku                  Haiku 4.5 · Fastest for quick answers",
        "   ◉ xHigh effort ←/→ to adjust",
        "   Enter to set as default · s to use this session only · Esc to cancel",
    ];

    #[test]
    fn an_idle_pane_reads_idle() {
        let v = verdict(IDLE, Some("✳ Claude Code"));
        assert_eq!(v.state, AgentState::Idle);
        assert_eq!(v.agent.as_deref(), Some("claude"));
        assert_eq!(v.rule.as_deref(), Some("idle-glyph"));
    }

    #[test]
    fn a_spinner_in_the_title_reads_working() {
        // Same SCREEN as the idle case -- only the title's leading glyph differs, which is the
        // whole claim the working rule makes.
        let v = verdict(IDLE, Some("⠂ Run sleep command for 25 seconds"));
        assert_eq!(v.state, AgentState::Working);
        assert_eq!(v.rule.as_deref(), Some("spinner-glyph"));
    }

    /// THE test this slice was blocked on, and the one the whole priority scheme exists for.
    ///
    /// The title on a real blocked pane was measured as `✳ Remove temporary h3-agent directory`:
    /// the IDLE glyph, beside a summary of a task that had already finished. So the idle rule
    /// matches here too, and the verdict is decided by priority alone.
    #[test]
    fn a_permission_dialog_reads_blocked_although_the_title_says_idle() {
        let title = "✳ Remove temporary h3-agent directory";
        let v = verdict(PERMISSION_DIALOG, Some(title));
        assert_eq!(v.state, AgentState::Blocked);
        assert_eq!(v.rule.as_deref(), Some("dialog-choice-list"));

        // Non-vacuity: the idle rule really does match the same pane, so the assertion above is
        // about ARBITRATION and not about the idle rule happening to miss.
        let manifest = claude();
        let idle = manifest
            .rules
            .iter()
            .find(|r| r.id == "idle-glyph")
            .expect("the manifest has an idle rule");
        let em = painted(PERMISSION_DIALOG);
        assert!(
            idle.all.iter().all(|m| m.holds(em.screen(), title)),
            "the idle rule must also match, or this test proves nothing about priority",
        );
    }

    #[test]
    fn the_trust_dialog_and_the_model_picker_read_blocked_too() {
        for (name, lines) in [("trust", TRUST_DIALOG), ("picker", MODEL_PICKER)] {
            let v = verdict(lines, Some("✳ Claude Code"));
            assert_eq!(v.state, AgentState::Blocked, "{name} dialog");
            assert_eq!(v.rule.as_deref(), Some("dialog-choice-list"), "{name}");
        }
    }

    /// The one that keeps the dialog rule from being a menace: a user's own typed line.
    #[test]
    fn a_single_numbered_line_the_user_typed_is_not_a_dialog() {
        let echoed = &[
            "────────────────────────────────────────────────────────────────────────────────",
            "❯ 1. rewrite the parser",
            "────────────────────────────────────────────────────────────────────────────────",
            "  ⏸ manual mode on · ? for shortcuts",
        ];
        let v = verdict(echoed, Some("✳ Claude Code"));
        assert_eq!(
            v.state,
            AgentState::Idle,
            "one numbered line is an echo, not a choice list",
        );
    }

    /// The spinner rule must cover frames nobody watched, or it is a rule about one run of the
    /// animation. `⠂` and `⠐` were observed; these were not.
    #[test]
    fn the_spinner_rule_matches_braille_frames_that_were_never_observed() {
        for frame in ["⠁", "⠈", "⡀", "⢀", "⣿"] {
            let title = format!("{frame} Doing something");
            let v = verdict(IDLE, Some(&title));
            assert_eq!(v.state, AgentState::Working, "frame {frame}");
        }
    }

    #[test]
    fn a_pane_that_is_not_an_agent_reads_unknown_and_names_nobody() {
        let shell = &["coin@box:~$ cargo build", "   Compiling sprag-vt v0.0.1"];
        let v = verdict(shell, Some("coin@box: ~"));
        assert_eq!(v, Verdict::default());
        assert_eq!(v.agent, None, "no manifest may claim a plain shell");
    }

    /// A pane with no title at all must not be claimed by a `StartsWith` fingerprint, because the
    /// empty string is a prefix of nothing. The footer fingerprint is what still recognises it.
    #[test]
    fn a_titleless_agent_pane_is_still_recognised_by_its_footer() {
        let v = verdict(IDLE, None);
        assert_eq!(v.agent.as_deref(), Some("claude"));
        assert_eq!(
            v.state,
            AgentState::Unknown,
            "recognised, but no state rule can fire without a title",
        );
        assert_eq!(
            v.rule, None,
            "and the verdict says so rather than inventing a rule",
        );
    }

    /// The bound this slice ships with, asserted so it is a KNOWN miss rather than a surprise: a
    /// first-run trust dialog arrives before any title and with the footer replaced.
    #[test]
    fn a_first_run_dialog_before_any_title_is_a_known_miss() {
        let v = verdict(TRUST_DIALOG, None);
        assert_eq!(
            v,
            Verdict::default(),
            "documented in the crate docs; recoverable by a title-free fingerprint later",
        );
    }

    #[test]
    fn a_dialog_further_up_than_the_window_does_not_match() {
        let mut lines: Vec<&str> = TRUST_DIALOG.to_vec();
        // Thirteen non-empty rows of transcript below the dialog push its marker out of the
        // twelve-row window -- the window is a claim about distance and this is what tests it.
        lines.extend(std::iter::repeat_n("● and then it said something else", 13));
        let v = verdict(&lines, Some("✳ Claude Code"));
        assert_eq!(v.state, AgentState::Idle);
    }

    #[test]
    fn bottom_lines_skips_blank_rows_and_reads_downward() {
        let em = painted(&["first", "", "   ", "second", "", "third"]);
        assert_eq!(bottom_lines(em.screen(), 2).trim_end(), "second\nthird");
        assert_eq!(
            bottom_lines(em.screen(), 99).trim_end(),
            "first\nsecond\nthird",
            "asking for more rows than exist returns what there is",
        );
    }

    #[test]
    fn priority_beats_declaration_order_in_both_directions() {
        let both = |first: i32, second: i32| Manifest {
            name: "t".to_owned(),
            any: vec![Fingerprint::one(Match::new(
                Region::Title,
                Test::Contains("t".to_owned()),
            ))],
            rules: vec![
                Rule {
                    id: "first".to_owned(),
                    state: AgentState::Working,
                    all: vec![],
                    priority: first,
                },
                Rule {
                    id: "second".to_owned(),
                    state: AgentState::Blocked,
                    all: vec![],
                    priority: second,
                },
            ],
        };
        let em = painted(&["t"]);
        assert_eq!(
            detect(em.screen(), Some("t"), &[both(1, 2)])
                .rule
                .as_deref(),
            Some("second"),
        );
        assert_eq!(
            detect(em.screen(), Some("t"), &[both(2, 1)])
                .rule
                .as_deref(),
            Some("first"),
        );
        // ...and a TIE falls to declaration order, the tie-break a reader can see.
        assert_eq!(
            detect(em.screen(), Some("t"), &[both(5, 5)])
                .rule
                .as_deref(),
            Some("first"),
        );
    }

    #[test]
    fn every_match_in_a_rule_must_hold() {
        let manifest = Manifest {
            name: "t".to_owned(),
            any: vec![Fingerprint::one(Match::new(
                Region::Title,
                Test::Contains("t".to_owned()),
            ))],
            rules: vec![Rule {
                id: "both".to_owned(),
                state: AgentState::Blocked,
                all: vec![
                    Match::new(Region::Title, Test::Contains("t".to_owned())),
                    Match::new(Region::BottomLines(2), Test::Contains("absent".to_owned())),
                ],
                priority: 1,
            }],
        };
        let em = painted(&["present"]);
        let v = detect(em.screen(), Some("t"), &[manifest]);
        assert_eq!(
            v.agent.as_deref(),
            Some("t"),
            "the manifest still claims it"
        );
        assert_eq!(
            v.state,
            AgentState::Unknown,
            "but a half-matched rule is no match"
        );
    }

    #[test]
    fn the_first_manifest_that_fingerprints_the_pane_claims_it() {
        let claimer = |name: &str| Manifest {
            name: name.to_owned(),
            any: vec![Fingerprint::one(Match::new(
                Region::Title,
                Test::Contains("x".to_owned()),
            ))],
            rules: vec![],
        };
        let em = painted(&["x"]);
        let v = detect(em.screen(), Some("x"), &[claimer("a"), claimer("b")]);
        assert_eq!(v.agent.as_deref(), Some("a"));
    }

    #[test]
    fn wire_str_omits_only_the_unknown_state() {
        assert_eq!(AgentState::Unknown.wire_str(), None);
        assert_eq!(AgentState::Working.wire_str(), Some("working"));
        assert_eq!(AgentState::Blocked.wire_str(), Some("blocked"));
        assert_eq!(AgentState::Idle.wire_str(), Some("idle"));
    }

    // ── The SECOND agent (R251). Every fixture below was captured from a real `codex` driven in a
    //    real sprag pane, the same way `claude`'s were. One agent is a sample; these are what make
    //    the vocabulary above a claim about agents rather than about one vendor's UI.

    fn codex_verdict(lines: &[&str], title: Option<&str>) -> Verdict {
        let em = painted(lines);
        detect(em.screen(), title, &[codex()])
    }

    /// At rest: the composer holding its placeholder, the footer showing model, effort and cwd.
    /// Title is the bare directory name — `codexprobe`, with no glyph of any kind.
    const CODEX_IDLE: &[&str] = &[
        "  Tip: Our most capable model yet. GPT-5.6 Sol can tackle complex code changes,",
        "  dig into research, produce polished documents, and take on your most ambitious",
        "› Write tests for @filename",
        "  gpt-5.6-sol default · /tmp/claude-1000/-home-coin-sprag/6fa43ef3-48fe-4a56-a7…",
    ];

    /// The sign-in picker, `codex`'s first screen. Marker is ASCII `>` at column zero.
    const CODEX_SIGNIN_PICKER: &[&str] = &[
        "  Welcome to Codex, OpenAI's command-line coding agent",
        "  Sign in with ChatGPT to use Codex as part of your paid plan",
        "  or connect an API key for usage-based billing",
        "> 1. Sign in with ChatGPT",
        "     Usage included with Plus, Pro, Business, and Enterprise plans",
        "  2. Sign in with Device Code",
        "     Sign in from another device with a one-time code",
        "  3. Provide your own API key",
        "     Pay for what you use",
        "  Press enter to continue",
    ];

    /// The directory-trust dialog. Marker is `›` — a DIFFERENT glyph from the screen above, in the
    /// same agent, which is the measurement that made the marker a class.
    const CODEX_TRUST_DIALOG: &[&str] = &[
        "> You are in /tmp/claude-1000/-home-coin-sprag/6fa43ef3-48fe-4a56-a7d7-5b36f3d23",
        "  Do you trust the contents of this directory? Working with untrusted contents",
        "  comes with higher risk of prompt injection. Trusting the directory allows",
        "  project-local config, hooks, and exec policies to load.",
        "› 1. Yes, continue",
        "  2. No, quit",
        "  Press enter to continue",
    ];

    /// A MID-SESSION modal — the `/model` picker, opened with no model call at all. This is the
    /// state the front exists for, and the one the footer conjunction cannot see.
    const CODEX_MODEL_PICKER: &[&str] = &[
        "  Select Model and Effort",
        "  Access legacy models by running codex -m <model_name> or in your config.toml",
        "› 1. gpt-5.6-sol (current)  Latest frontier agentic coding model.",
        "  2. gpt-5.6-terra          Balanced agentic coding model for everyday work.",
        "  3. gpt-5.6-luna           Fast and affordable agentic coding model.",
        "  4. gpt-5.5                Frontier model for complex coding, research, and",
        "                            real-world work.",
        "  5. gpt-5.2                Optimized for professional work and long-running",
        "                            agents.",
        "  Press enter to confirm or esc to go back",
    ];

    #[test]
    fn codex_at_rest_reads_idle_from_a_title_with_no_glyph_at_all() {
        let v = codex_verdict(CODEX_IDLE, Some("codexprobe"));
        assert_eq!(v.state, AgentState::Idle);
        assert_eq!(v.agent.as_deref(), Some("codex"));
        assert_eq!(v.rule.as_deref(), Some("no-working-signal"));
    }

    /// The payoff of writing [`spinner_pattern`] for the braille BLOCK rather than for the two
    /// frames `claude` happened to show: these four came from a different agent and none of them
    /// was ever produced by a `claude` probe.
    #[test]
    fn codex_working_is_read_from_braille_frames_no_claude_probe_produced() {
        for frame in ["⠼", "⠏", "⠸", "⠇"] {
            let title = format!("{frame} codexprobe");
            let v = codex_verdict(CODEX_IDLE, Some(&title));
            assert_eq!(v.state, AgentState::Working, "frame {frame}");
            assert_eq!(v.rule.as_deref(), Some("spinner-glyph"), "frame {frame}");
        }
    }

    /// Both `codex` dialogs match the shared pattern, and the test asserts WHY that needed a
    /// change: the marker literal `claude` was written from misses both of them. Without this
    /// second half the widening would be untested — the pattern would pass either way.
    #[test]
    fn both_codex_markers_match_and_claudes_lone_marker_would_have_missed_them() {
        let widened = dialog_pattern();
        let claude_only = Regex::new(r"(?m)^\s*❯\s+\d+\.[\s\S]*?^\s*\d+\.").expect("compiles");
        for fixture in [CODEX_SIGNIN_PICKER, CODEX_TRUST_DIALOG, CODEX_MODEL_PICKER] {
            let em = painted(fixture);
            let text = bottom_lines(em.screen(), 12);
            assert!(widened.is_match(&text), "widened pattern must match");
            assert!(
                !claude_only.is_match(&text),
                "the single-marker pattern must MISS, or the widening proves nothing"
            );
        }
    }

    /// The slice's second measured miss, pinned exactly like the first. A mid-session modal covers
    /// the footer, so the fingerprint cannot claim the pane — and the assertion goes on to show the
    /// RULE matches that very screen, which is what makes this a miss of identification rather than
    /// of the rule. That distinction is what tells slice 2 where to fix it: the per-pane memory has
    /// to carry WHICH AGENT a pane is, not only what it was doing.
    #[test]
    fn codex_dialog_matches_the_rule_although_the_pane_is_not_claimed() {
        let v = codex_verdict(CODEX_MODEL_PICKER, Some("codexprobe"));
        assert_eq!(v.state, AgentState::Unknown);
        assert_eq!(v.agent, None, "the footer the fingerprint needs is covered");

        let manifest = codex();
        let blocked = manifest
            .rules
            .iter()
            .find(|r| r.id == "dialog-choice-list")
            .expect("codex has a dialog rule");
        let em = painted(CODEX_MODEL_PICKER);
        assert!(
            blocked
                .all
                .iter()
                .all(|m| m.holds(em.screen(), "codexprobe")),
            "the rule matches; only the identification is missing"
        );
    }

    /// The conjunction is load-bearing in BOTH directions, so each half is dropped in turn. Either
    /// half alone is a shape an ordinary shell can produce, which is the whole reason
    /// [`Fingerprint`] carries a list rather than one match.
    #[test]
    fn neither_half_of_the_codex_fingerprint_claims_a_pane_alone() {
        let composer_only = &["› Write tests for @filename", "  just some output"];
        assert_eq!(codex_verdict(composer_only, Some("codexprobe")).agent, None);

        let footer_only = &[
            "  some transcript line",
            "  gpt-5.6-sol default · /tmp/claude-1000/-home-coin-sprag/6fa43ef3-48fe-4a56-a7…",
        ];
        assert_eq!(codex_verdict(footer_only, Some("codexprobe")).agent, None);
    }

    /// The idle rule fires on a title being THERE, so the one input that can hold it honest is a
    /// claimed pane that has painted none. Widen the pattern to accept the empty string and this
    /// goes red; nothing else does, which is how the rule's own doc knows what it is allowed to
    /// claim.
    #[test]
    fn a_codex_pane_that_has_painted_no_title_is_not_asserted_to_be_resting() {
        let v = codex_verdict(CODEX_IDLE, None);
        assert_eq!(v.state, AgentState::Unknown);
        assert_eq!(
            v.agent.as_deref(),
            Some("codex"),
            "the pane IS claimed — this is a miss about the state, not about identity"
        );
    }

    /// Two manifests in one list must not claim each other's panes, which is the property slice 4's
    /// user-layered manifests will depend on and which no single-manifest test can show.
    #[test]
    fn the_two_built_in_manifests_do_not_claim_each_others_panes() {
        let both = [claude(), codex()];

        let em = painted(CODEX_IDLE);
        let v = detect(em.screen(), Some("codexprobe"), &both);
        assert_eq!(v.agent.as_deref(), Some("codex"));

        let em = painted(IDLE);
        let v = detect(em.screen(), Some("✳ Claude Code"), &both);
        assert_eq!(v.agent.as_deref(), Some("claude"));
    }
}
