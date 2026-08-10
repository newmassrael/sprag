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
//! The rules answer from ONE frame. What a frame cannot know — that the frame before said
//! something else — is [`Tracker`], the per-pane memory this crate also owns (slice 2). It keeps
//! the clock a parameter for the same reason everything else here is pure. The publish seam is
//! still the caller's (slice 3).
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
//!   even a per-agent constant, so [`question`] matches the measured CLASS. Everything else about
//!   the shape held: marker, `<digit>.`, and at least one more numbered option below.
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
//! ## The bounds, and what closing two of them cost
//!
//! A pane is identified as an agent's by [`Manifest::any`], and every fingerprint needs the pane to
//! have painted something recognisable. Slice 1 shipped two measured misses that followed from that,
//! both reading [`AgentState::Unknown`] rather than the true state. **Both are now closed, and the
//! answer to the first was inside the screen that recorded it.**
//!
//! * **Onboarding, in BOTH agents** — the first-run dialog arrives before any title and with the
//!   footer replaced, so every other fingerprint is blind to it. But the captured screens NAME the
//!   product, in the sentence each prints to explain itself, so a title-free fingerprint was
//!   buildable from the fixtures that had been sitting in this file's tests since slice 1. It is a
//!   CONJUNCTION of the name and the dialog shape: on the name alone, any pane displaying the
//!   agent's name — a README, a terminal someone is discussing it in — would be claimed, and that is
//!   worse than a wrong row, because [`Tracker`] remembers an identity and would go on rescuing that
//!   pane forever.
//! * **`codex`'s transient post-`esc` hint**, which replaces the footer its fingerprint is a
//!   conjunction over. **This one is NOT closed, and it is not closable by the memory**, which is
//!   worth stating because the memory looks like it should. [`Tracker`] asserts a remembered
//!   identity only for an ACTIVE verdict, and a pane that has just been `esc`-ed is at REST — the
//!   filter is deliberate and measured: at rest a `codex` title is the bare working directory and
//!   its screen is a prompt line, which is what a SHELL looks like, so a pane whose agent EXITED
//!   would otherwise go on being reported as that agent.
//!
//! Two residues follow, and both are properties of the screens rather than of this crate:
//!
//! * The hint window above, for a pane at rest.
//! * `codex`'s directory-trust dialog names nothing at all — any program could ask "do you trust the
//!   contents of this directory", so no fingerprint can claim it. It IS rescued when it follows the
//!   sign-in picker in the same pane, which is the order a new user meets it in, because that
//!   screen is a dialog and therefore active; a pane whose first screen is the trust dialog, on a
//!   box where sign-in already happened, is still a miss.
//!
//! **The trap that shape avoids is not hypothetical.** The `codex` trust-dialog fixture was captured
//! under `/tmp/claude-1000/…`, so the string `claude` is on that screen. A fingerprint written on
//! the bare NAME rather than on a phrase would read a `codex` pane as `claude` on any box whose
//! scratch directory is named that way — a cross-agent misattribution produced by the capture
//! environment and by neither agent. `the_codex_trust_dialog_names_nobody_and_is_still_a_miss`
//! asserts the string is there and that the fingerprint does not fire on it.

use std::sync::atomic::{AtomicU64, Ordering};

use regex::Regex;
use sprag_vt::Screen;

mod choice;
mod track;

pub use choice::{Choice, Question, question};
pub use track::{DEFAULT_SETTLE, Hysteresis, Report, ReportOutcome, Tracker};

/// How many rule evaluations have run, process-wide.
static EVALUATIONS: AtomicU64 = AtomicU64::new(0);
/// How many manifests those evaluations were OFFERED, process-wide.
static MANIFESTS: AtomicU64 = AtomicU64::new(0);

/// What this crate has cost the process so far — the meter for the work [`Tracker`]'s quiescence
/// gate exists to avoid.
///
/// # Why this exists, which is a story about an instrument rather than about a counter
///
/// The gate is an EXACT skip: it claims a re-evaluation would reach the answer already published,
/// so its absence changes no answer. That is what makes it worth having and it is also what makes
/// it hard to prove. R252 proved it BEHAVIOURALLY — rewrite a rule underneath a tracker, observe an
/// unchanged pane, and assert the verdict did not move, because only an evaluation could have
/// noticed the rewrite. R254 then put the rule list's identity INTO the gate's key, which is
/// correct and which destroyed that instrument: a rewritten list is now a different list, so the
/// gate no longer skips it and the one observable a test could reach is gone. Deleting the gate
/// outright turned no test red.
///
/// So the proof moves from behaviour to COST, and a cost this project can assert is a count.
/// `sprag_grid::work` is the precedent in this tree — R217 metered projections for the same reason
/// and R221 measured why: wall-clock on this box drifts 20-30% between runs of the same binary, so
/// a threshold in microseconds is a flake by construction, while a count is unaffected by what else
/// the machine is doing. `sprag-latency` prices the gate in TIME and says what that saving is
/// worth; this is the half that can go red.
///
/// Both totals are monotonic and process-wide. A caller reads them twice and takes the DELTA; a
/// single reading means nothing on its own, because it includes every evaluation since boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectWork {
    /// Rule evaluations run — one per [`detect`] call, and so one per look at a pane the
    /// quiescence gate did NOT skip. This is the number the gate exists to hold down.
    pub evaluations_total: u64,
    /// Manifests those evaluations were offered — the VOLUME, and it is not `evaluations *
    /// list_len`. Identification stops at the first manifest that claims the pane, so a claimed
    /// pane costs its own position in the list and a pane nobody claims costs the whole list.
    /// That asymmetry is the price of slice 4's layering rule (a user's new agent goes to the
    /// FRONT), and it is charged to every ordinary shell pane in the workspace.
    pub manifests_total: u64,
}

/// Read the meter. See [`DetectWork`] for why the answer is only meaningful as a delta.
#[must_use]
pub fn work() -> DetectWork {
    DetectWork {
        evaluations_total: EVALUATIONS.load(Ordering::Relaxed),
        manifests_total: MANIFESTS.load(Ordering::Relaxed),
    }
}

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

    /// The state a REPORTER named, or `None` for a token that is not one of the three.
    ///
    /// [`wire_str`](Self::wire_str)'s inverse, written here so the vocabulary has ONE definition: a
    /// process reporting its own state uses the spelling a client already reads, and a spelling
    /// invented at a wire boundary could not be published.
    ///
    /// **`unknown` is deliberately not accepted**, and the asymmetry is the point.
    /// [`Unknown`](Self::Unknown) means "no manifest claims this pane", which is a conclusion about
    /// the RULES and not a state a reporter is in. A reporter that no longer knows what it is doing
    /// is asking to be scraped again — which is a RELEASE, not a report of `unknown`, and the two
    /// must not collapse: the second would pin an authoritative "not an agent" over a pane the
    /// screen can see perfectly well.
    #[must_use]
    pub fn from_wire(token: &str) -> Option<Self> {
        match token {
            "working" => Some(Self::Working),
            "blocked" => Some(Self::Blocked),
            "idle" => Some(Self::Idle),
            _ => None,
        }
    }

    /// Whether this state is asserted by evidence PRESENT on the screen, rather than by the absence
    /// of it.
    ///
    /// [`Working`](Self::Working) is a spinner frame in the title and [`Blocked`](Self::Blocked) is
    /// a choice list on the screen: both are things a rule SAW. The other two are what a pane reads
    /// as when the working signal or the fingerprint is not there — and an absence can be an
    /// artifact of the instant the sample was taken, because the working signal is an ANIMATION
    /// (R249's M2, a title alternating at about 1 Hz).
    ///
    /// [`Tracker`] rests two decisions on that asymmetry and no third thing should read it as
    /// "busy": an active verdict is published on sight while a resting one has to hold
    /// ([`Hysteresis`]), and a pane whose fingerprint a modal has covered may keep the identity it
    /// already had only while it is showing something active.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
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
    /// The region holds a CHOICE LIST: a marked, consecutively numbered set of at least two
    /// options — see [`question`], which is both this test and the parse a supervisor reads.
    ///
    /// The one test that is not a string comparison, and the reason is that its answer is wanted
    /// twice. A pane blocked on a dialog is a pane somebody has to ANSWER, and answering means
    /// naming an option; a rule that concluded "blocked" from a pattern while the options were
    /// re-derived somewhere else would be two readers of one screen, free to disagree about
    /// whether there is a list at all. There is one function, so the rule fires exactly when the
    /// options can be enumerated.
    ///
    /// On [`Region::Title`] it is always false, and not vacuously: a title is one line and a
    /// choice list is at least two options.
    ChoiceList,
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
        // The structured test reads the region's ROWS rather than one joined string, so it is
        // taken before the text is ever built — a pane with no menu in its window costs no join.
        if matches!(self.test, Test::ChoiceList) {
            return match self.region {
                Region::Title => false,
                Region::BottomLines(n) => question(screen, n).is_some(),
            };
        }
        let text = match &self.region {
            Region::Title => title.to_owned(),
            Region::BottomLines(n) => bottom_lines(screen, *n),
        };
        match &self.test {
            Test::StartsWith(needle) => text.starts_with(needle.as_str()),
            Test::Contains(needle) => text.contains(needle.as_str()),
            Test::Regex(pattern) => pattern.is_match(&text),
            Test::ChoiceList => unreachable!("taken above, before the region's text is built"),
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

impl Manifest {
    /// Whether any fingerprint claims this pane as this agent's.
    fn claims(&self, screen: &Screen, title: &str) -> bool {
        self.any.iter().any(|fp| fp.holds(screen, title))
    }

    /// The verdict this manifest's rules reach on a pane already taken to be this agent's —
    /// arbitration WITHOUT identification.
    ///
    /// Separate from [`detect`] because the two halves have different answers available to them.
    /// Identification reads the screen and can fail on a screen that is still this agent's: a modal
    /// covers the composer and the footer `codex` is recognised by, which is the one state this
    /// whole front exists to report (R251). [`Tracker`] remembers what a pane was and calls this
    /// directly, so the memory supplies the half the screen has hidden. Keeping it ONE function is
    /// [`Rule::id`]'s argument again — a second arbitration path is a path that can disagree.
    fn verdict(&self, screen: &Screen, title: &str) -> Verdict {
        // Highest priority wins; `max_by_key` on a plain iterator returns the LAST maximum, so the
        // order is reversed to make it the FIRST — declaration order is the documented tie-break,
        // and a reader should not have to know which end of the iterator the tie fell off.
        let fired = self
            .rules
            .iter()
            .rev()
            .filter(|rule| rule.all.iter().all(|m| m.holds(screen, title)))
            .max_by_key(|rule| rule.priority);
        Verdict {
            state: fired.map_or(AgentState::Unknown, |rule| rule.state),
            agent: Some(self.name.clone()),
            rule: fired.map(|rule| rule.id.clone()),
        }
    }
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
///
/// This is the ONE place the rules run, which is what makes [`work`] a meter rather than an
/// estimate: [`Tracker`] delegates here rather than walking the list itself, so an evaluation
/// cannot be paid for without being counted. See [`DetectWork`] for why a count is the gate's
/// remaining proof.
#[must_use]
pub fn detect(screen: &Screen, title: Option<&str>, manifests: &[Manifest]) -> Verdict {
    let title = title.unwrap_or_default();
    EVALUATIONS.fetch_add(1, Ordering::Relaxed);
    // Counted inside the predicate rather than as `manifests.len()`, because `find` short-circuits
    // and the difference is the whole point of the number: a claimed pane pays for its position in
    // the list, an unclaimed one pays for all of it.
    let mut offered = 0_u64;
    let claimed = manifests.iter().find(|manifest| {
        offered += 1;
        manifest.claims(screen, title)
    });
    MANIFESTS.fetch_add(offered, Ordering::Relaxed);
    claimed.map_or_else(Verdict::default, |manifest| manifest.verdict(screen, title))
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

/// The pattern a dialog USED to be recognised by, kept only so a test can show what replacing it
/// bought.
///
/// It is not wired into any manifest: [`Test::ChoiceList`] is, and [`question`] is what answers it.
/// The regex could say a menu was there and could not say what was on it, so the options had to be
/// re-derived by whoever wanted to answer one — two readers of one screen, which is the shape a
/// round of this project's history is named after. What the parser adds beyond enumerating the
/// options is the CONSECUTIVE numbering: this pattern matches any marked `N.` line followed
/// anywhere below by any other `M.` line, so two unrelated numbered lines read as a menu.
/// `a_marked_line_and_an_unrelated_numbered_line_are_not_a_menu` is that difference, driven.
#[cfg(test)]
fn dialog_pattern() -> Regex {
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

/// How many non-empty rows up from the bottom a dialog's selection marker may sit.
///
/// Twelve, because a dialog is bottom-anchored but VARIES in height: the measured ones put their
/// marker 3, 5, 7, 8 and 9 non-empty rows from the bottom across both agents. A window sized to the
/// smallest would miss the others, and one sized to the pane would make the rule depend on how tall
/// the pane happens to be.
///
/// Named rather than repeated because it is now read by four matches in two manifests — the two
/// dialog RULES and the two onboarding FINGERPRINTS, which have to agree about the region or the
/// conjunction would be looking at a different screen than the rule that then fires on it.
///
/// PUBLIC because a fifth reader needs it and could otherwise only re-spell it: a caller wanting
/// the OPTIONS a blocked pane is offering ([`question`]) has to ask in the same window the rule
/// that blocked it read, or the two would be describing different screens. A manifest of the
/// user's own may declare a different window, and then this constant is not that manifest's —
/// which is why `question` takes the window as a parameter rather than assuming this one.
pub const DIALOG_WINDOW: u16 = 12;

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
            // ONBOARDING, where neither of the two above exists: the trust dialog arrives before
            // the agent has set a title and with the footer replaced by the dialog itself. This is
            // the title-free fingerprint the crate docs owed, and it is a CONJUNCTION for a reason
            // the alternative makes obvious. On the name alone, any pane displaying this agent's
            // name — a README, a terminal someone is discussing it in — would be claimed, and a
            // false claim is not merely a wrong row: `Tracker` remembers the identity and would go
            // on rescuing that pane as an agent every time anything dialog-shaped appeared in it.
            // Requiring the dialog SHAPE as well narrows it to what was actually measured.
            Fingerprint::all(vec![
                Match::new(
                    Region::BottomLines(DIALOG_WINDOW),
                    Test::Contains("Claude Code".to_owned()),
                ),
                Match::new(Region::BottomLines(DIALOG_WINDOW), Test::ChoiceList),
            ]),
        ],
        rules: vec![
            Rule {
                id: "dialog-choice-list".to_owned(),
                state: AgentState::Blocked,
                all: vec![Match::new(
                    Region::BottomLines(DIALOG_WINDOW),
                    Test::ChoiceList,
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
        // A conjunction, because no single condition on a `codex` screen is both stable and
        // specific: the composer marker alone is a character a shell prompt may well use, and the
        // footer shape alone is three tokens and a path. Together they are the agent. This is the
        // case `Fingerprint` was added for.
        any: vec![
            Fingerprint::all(vec![
                Match::new(
                    Region::BottomLines(3),
                    Test::Regex(codex_composer_pattern()),
                ),
                Match::new(Region::BottomLines(1), Test::Regex(codex_footer_pattern())),
            ]),
            // ONBOARDING, the state the conjunction above cannot see: the sign-in picker replaces
            // both the composer and the footer, and arrives before any title. `claude`'s equivalent
            // carries the same argument for requiring the dialog SHAPE alongside the name.
            //
            // This one buys more than the screen it matches. Once this pane is claimed, `Tracker`
            // remembers the identity — so the directory-trust dialog that FOLLOWS a sign-in, which
            // names nothing at all, is rescued by the memory rather than needing a fingerprint it
            // cannot have.
            Fingerprint::all(vec![
                Match::new(
                    Region::BottomLines(DIALOG_WINDOW),
                    Test::Contains("Welcome to Codex".to_owned()),
                ),
                Match::new(Region::BottomLines(DIALOG_WINDOW), Test::ChoiceList),
            ]),
        ],
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
                    Region::BottomLines(DIALOG_WINDOW),
                    Test::ChoiceList,
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

/// Every manifest this crate ships, in the order they are offered to a pane.
///
/// # Why this is a function and not a `const`
///
/// A manifest owns compiled [`Regex`]es, so this ALLOCATES — and that is the whole reason the list
/// exists as one named thing. A caller that built `vec![claude(), codex()]` per evaluation would
/// recompile every pattern on a path served once per client wake; a caller that holds the result
/// compiles them once for the life of the daemon. Naming the list is what makes the second shape the
/// obvious one.
///
/// # Order
///
/// First match wins for IDENTIFICATION — the fingerprint check [`detect`] runs — so the order is part
/// of the answer.
/// It is `claude` then `codex` for no better reason than the order they were measured in (R249, then
/// R251), and nothing rests on it: the two fingerprints are asserted not to claim each other's panes
/// (`the_two_built_in_manifests_do_not_claim_each_others_panes`), so the order is currently
/// unobservable. Slice 4 layers user manifests over this list, and its layering rule is where order
/// becomes load-bearing.
#[must_use]
pub fn built_ins() -> Vec<Manifest> {
    vec![claude(), codex()]
}

/// The manifest list an evaluation runs against, carrying the identity of that list.
///
/// # Why the list and its identity cannot be passed apart
///
/// [`Tracker`]'s quiescence gate is EXACT, and the proof has a premise: the rules are a pure
/// function of the screen and the title, so a pane where neither has moved cannot reach a different
/// verdict. That holds for exactly as long as the rules themselves do not move — and slice 4 makes
/// them move, because a user edits `config.toml` and the list is replaced underneath a workspace of
/// settled panes. The rules are a THIRD input, and the gate was watching two.
///
/// Left unwatched, the stale verdict survives for as long as the pane stays quiet. That is not a
/// corner: a quiet pane is exactly where a wrong verdict is visible and stuck, so it is the pane a
/// user is editing a manifest to correct. The edit would appear to do nothing while every individual
/// verdict still looked right.
///
/// So the identity belongs BESIDE the list rather than in a parameter a caller can forget to pass —
/// `AgentClock`'s argument one crate up, where the condvar lives in one type with the mutex so no
/// caller can create a candidate and neglect to signal.
#[derive(Debug, Clone)]
pub struct Ruleset {
    /// The manifests, in the order they are offered to a pane. First claim wins ([`detect`]).
    manifests: Vec<Manifest>,
    /// Which list this is.
    ///
    /// Compared for EQUALITY and never for order: the gate asks whether these are the same rules
    /// that produced the last verdict, not which of two is newer. Drawn from a process-wide counter
    /// rather than incremented per ruleset, so two lists built independently are never mistaken for
    /// one — a positional number would make `Ruleset::new(a)` and `Ruleset::new(b)` interchangeable
    /// to the gate, which is the bug this field exists to prevent wearing the costume of the fix.
    revision: u64,
}

impl Default for Ruleset {
    /// The manifests this crate ships.
    fn default() -> Self {
        Self::new(built_ins())
    }
}

impl Ruleset {
    /// A ruleset over `manifests`, with an identity nothing else shares.
    #[must_use]
    pub fn new(manifests: Vec<Manifest>) -> Self {
        Self {
            manifests,
            revision: next_revision(),
        }
    }

    /// The manifests, for the matcher.
    #[must_use]
    pub fn manifests(&self) -> &[Manifest] {
        &self.manifests
    }

    /// This list's identity: equal for two readings of the same list, different for any two lists.
    ///
    /// Compared for EQUALITY and never for order — a reader asks whether these are the rules that
    /// produced its last answer, not which of two is newer.
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// How many manifests are offered to a pane.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.manifests.len()
    }

    /// Whether no manifest is offered at all — every pane reads [`AgentState::Unknown`].
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.manifests.is_empty()
    }
}

/// The next ruleset identity.
///
/// Process-wide and monotonic, so an identity is never reused within a run. `Relaxed` because
/// nothing is ordered against this: the only operation on the value is equality against a copy a
/// tracker took, and the tracker took it while holding the registry's lock.
fn next_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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

    /// Every dialog this crate ever captured, enumerated: the numbers a caller would type, and
    /// which one Enter would land on.
    ///
    /// This is the answer `Blocked` could not give. Six screens from two independently written
    /// agents, and the parse is asserted against what a person reading the fixture sees rather
    /// than against what the code returns — the numbers and the marked option were read off the
    /// captured screens above before this test was run.
    #[test]
    fn every_captured_dialog_yields_the_options_it_offers() {
        let cases: [(&str, &[&str], &[u32], u32); 6] = [
            ("permission", PERMISSION_DIALOG, &[1, 2, 3], 1),
            ("trust", TRUST_DIALOG, &[1, 2], 1),
            ("model picker", MODEL_PICKER, &[1, 2, 3, 4, 5], 2),
            ("codex sign-in", CODEX_SIGNIN_PICKER, &[1, 2, 3], 1),
            ("codex trust", CODEX_TRUST_DIALOG, &[1, 2], 1),
            ("codex model", CODEX_MODEL_PICKER, &[1, 2, 3, 4, 5], 1),
        ];
        for (name, fixture, numbers, marked) in cases {
            let em = painted(fixture);
            let q = question(em.screen(), 12).unwrap_or_else(|| panic!("{name}: no options read"));
            assert_eq!(
                q.choices.iter().map(|c| c.number).collect::<Vec<_>>(),
                numbers,
                "{name}: the numbers a caller would type",
            );
            assert_eq!(
                q.selected().map(|c| c.number),
                Some(marked),
                "{name}: where Enter would land",
            );
            assert!(
                q.choices.iter().all(|c| !c.label.is_empty()),
                "{name}: an option with no label is one nobody can classify: {:?}",
                q.choices,
            );
        }
    }

    /// The two agents' shortest dialogs, spelled out in full — the numbers AND the words, so the
    /// test above is a claim about content and not only about counting.
    #[test]
    fn the_two_trust_dialogs_are_read_word_for_word() {
        let claude = question(painted(TRUST_DIALOG).screen(), 12).expect("claude's trust dialog");
        assert_eq!(
            claude
                .choices
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Yes, I trust this folder", "No, exit"],
        );
        assert!(
            claude
                .asked
                .iter()
                .any(|line| line.contains("Is this a project you created or one you trust?")),
            "the sentence a policy classifies must survive: {:?}",
            claude.asked,
        );

        let codex =
            question(painted(CODEX_TRUST_DIALOG).screen(), 12).expect("codex's trust dialog");
        assert_eq!(
            codex
                .choices
                .iter()
                .map(|c| c.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Yes, continue", "No, quit"],
        );
    }

    /// A model picker's option runs onto a second row, and the row below the list does not.
    ///
    /// Both halves matter and they are the same measurement seen twice: the description indented
    /// under an option is part of that option, and the footer at the option indent is part of
    /// none. Asserted on BOTH agents' pickers, because an indent rule read off one vendor's layout
    /// is a rule about that vendor.
    #[test]
    fn an_options_second_row_belongs_to_it_and_the_footer_belongs_to_nobody() {
        let claude = question(painted(MODEL_PICKER).screen(), 12).expect("claude's picker");
        assert!(
            claude
                .choice(2)
                .is_some_and(|c| c.label.ends_with("complex tasks")),
            "the marked option's second row is part of it: {:?}",
            claude.choice(2),
        );
        assert!(
            claude
                .choices
                .iter()
                .all(|c| !c.label.contains("Enter to set as default")),
            "the footer sits at the option indent and belongs to no option: {:?}",
            claude.choices,
        );

        let codex = question(painted(CODEX_MODEL_PICKER).screen(), 12).expect("codex's picker");
        assert!(
            codex
                .choice(4)
                .is_some_and(|c| c.label.ends_with("real-world work.")),
            "codex's fourth option runs onto a second row too: {:?}",
            codex.choice(4),
        );
        assert!(
            codex
                .choices
                .iter()
                .all(|c| !c.label.contains("Press enter to confirm")),
            "and its footer belongs to nobody either: {:?}",
            codex.choices,
        );
    }

    /// Enter does not always land on the first option, and a supervisor that assumed it did would
    /// answer the wrong question on every screen like this one.
    #[test]
    fn the_marked_option_is_not_always_the_first() {
        let q = question(painted(MODEL_PICKER).screen(), 12).expect("the picker");
        assert_eq!(q.selected().map(|c| c.number), Some(2));
        assert!(
            q.choice(1).is_some_and(|c| !c.selected),
            "the option ABOVE the marker exists and is not selected",
        );
    }

    /// A pane narrow enough to TEAR an option's label still reports the whole label.
    ///
    /// The window is still measured in rows (R344's decision, pinned by
    /// `a_dialog_still_reads_as_blocked_when_every_line_of_it_wraps`) — what changed is what is
    /// read out of it. `differently` is split across two rows at forty columns, and half a word is
    /// not something a policy can classify. The join goes through the emulator's own share
    /// arithmetic, so this is R344's primitive doing R344's job for a fourth reader.
    #[test]
    fn a_narrow_pane_does_not_tear_an_options_label() {
        const WHOLE: &str = "No, and tell Claude what to do differently (esc)";
        let mut torn = 0;
        for cols in [80_u16, 60, 40, 30] {
            let mut em = Emulator::new(cols, 24);
            em.advance(PERMISSION_DIALOG.join("\r\n").as_bytes());
            // Non-vacuity: at the narrow widths the option's own row really is torn.
            if !em.screen().row_text(em.screen().rows() - 1).is_empty() {
                // (the fixture never fills the screen; the check below is the real one)
            }
            let q = question(em.screen(), 12).unwrap_or_else(|| panic!("no options at {cols}"));
            assert_eq!(
                q.choice(3).map(|c| c.label.as_str()),
                Some(WHOLE),
                "at {cols} columns",
            );
            if (0..em.screen().rows())
                .filter(|row| em.screen().wrapped(*row))
                .any(|row| em.screen().row_text(row).contains("3. No,"))
            {
                torn += 1;
            }
        }
        assert!(
            torn > 0,
            "no width tore the option, so this test proves nothing about joining",
        );
    }

    /// The clause the retired regex did not have, driven against the regex itself.
    ///
    /// `dialog_pattern` reads a marked `N.` line followed ANYWHERE below by another `M.` line, and
    /// says nothing about the two being one list. So two unrelated numbered lines — a user's
    /// echoed choice in the prompt box, and a leftover transcript step below it — read as a menu
    /// whose second option does not exist. A supervisor acting on that would press `3` at a prompt
    /// that has no third option.
    ///
    /// The screen is SYNTHETIC, unlike every fixture above it: what it probes is the pattern's
    /// shape rather than an agent's UI, and the assertion that matters is that the control fires
    /// and the replacement does not.
    #[test]
    fn a_number_below_a_marked_line_is_not_a_second_option() {
        let screen = &[
            "● Here is the plan I typed back at you:",
            "────────────────────────────────────────────────────────────────────────────────",
            "❯ 1. rewrite the parser",
            "────────────────────────────────────────────────────────────────────────────────",
            "  3. and then run the suite",
            "  ⏸ manual mode on · ? for shortcuts",
        ];
        let em = painted(screen);
        assert!(
            dialog_pattern().is_match(&bottom_lines(em.screen(), 12)),
            "the control must fire, or this proves nothing about the replacement",
        );
        assert!(
            question(em.screen(), 12).is_none(),
            "numbers that do not RUN are not a list of choices",
        );
        assert_eq!(
            verdict(screen, Some("✳ Claude Code")).state,
            AgentState::Idle,
            "and so the pane is not blocked",
        );
    }

    /// The rule that fires and the options a caller reads are ONE answer.
    ///
    /// There is no state in which a pane is reported blocked by a choice list nobody can
    /// enumerate, nor one in which options can be read off a pane the rule calls idle — because
    /// `Test::ChoiceList` IS `question`. Driven over every captured screen in this file, blocked
    /// and not, so a later round that reintroduces a second reader finds out here.
    #[test]
    fn the_rule_that_fires_and_the_options_a_caller_reads_are_one_answer() {
        let cases: [(&str, &[&str], Option<&str>, bool); 8] = [
            ("permission", PERMISSION_DIALOG, Some("✳ x"), true),
            ("trust", TRUST_DIALOG, Some("✳ x"), true),
            ("model picker", MODEL_PICKER, Some("✳ x"), true),
            (
                "codex sign-in",
                CODEX_SIGNIN_PICKER,
                Some("codexprobe"),
                true,
            ),
            ("codex trust", CODEX_TRUST_DIALOG, Some("codexprobe"), true),
            ("codex model", CODEX_MODEL_PICKER, Some("codexprobe"), true),
            ("claude idle", IDLE, Some("✳ Claude Code"), false),
            ("codex idle", CODEX_IDLE, Some("codexprobe"), false),
        ];
        for (name, fixture, title, is_menu) in cases {
            let em = painted(fixture);
            assert_eq!(
                question(em.screen(), 12).is_some(),
                is_menu,
                "{name}: the parse",
            );
            // The RULE, asked of both manifests so a screen from either agent is judged by the
            // rule that would judge it in production.
            let blocked = detect(em.screen(), title, &built_ins()).state == AgentState::Blocked
                || codex()
                    .verdict(em.screen(), title.unwrap_or_default())
                    .state
                    == AgentState::Blocked;
            assert_eq!(
                blocked, is_menu,
                "{name}: the rule must agree with the parse"
            );
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

    /// The onboarding miss slice 1 shipped as a known bound, CLOSED — and closed out of the very
    /// screen that recorded it, because the fixture had the answer in it all along.
    ///
    /// The dialog arrives before any title and with the footer replaced, so both of `claude`'s other
    /// fingerprints are blind to it. What it does carry is the agent's own name, in a sentence it
    /// prints to explain what it is about to be allowed to do.
    ///
    /// The state matters as much as the identification: an onboarding dialog is a person being
    /// asked a question, so a pane that is now claimed must also read `Blocked` rather than merely
    /// becoming visible.
    #[test]
    fn a_first_run_dialog_is_claimed_by_the_title_free_fingerprint() {
        let v = verdict(TRUST_DIALOG, None);
        assert_eq!(
            v.agent.as_deref(),
            Some("claude"),
            "no title and no footer, and it is still recognised",
        );
        assert_eq!(
            v.state,
            AgentState::Blocked,
            "and the first thing a new user meets reads as what it is: a question",
        );
    }

    /// The other half of the conjunction, and the reason it IS one.
    ///
    /// On the name alone, every pane that displays the agent's name would be claimed — a README, a
    /// terminal someone is discussing it in, this project's own source. That is not merely a wrong
    /// row: `Tracker` remembers an identity once it is established, so a pane falsely claimed here
    /// would go on being rescued as an agent every time anything dialog-shaped appeared in it.
    ///
    /// REVERT-PROOF: drop the `ChoiceList` match from the onboarding fingerprint and this fails
    /// while `a_first_run_dialog_is_claimed_by_the_title_free_fingerprint` stays green — the two
    /// tests are the two halves of one `all`.
    #[test]
    fn merely_naming_the_agent_does_not_claim_a_pane() {
        let prose = &[
            "$ cat NOTES.md",
            "We should check what Claude Code does with a narrow pane.",
            "1. it might reflow",
            "$ ",
        ];
        let v = verdict(prose, None);
        assert_eq!(
            v.agent, None,
            "the name is in the text and there is no dialog, so nothing is claimed",
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

    /// A dialog on a NARROW pane, where every line of it wraps, still reads as blocked.
    ///
    /// R344 made the search walk logical lines and then asked the same question of every other
    /// reader that builds text out of rows. This one is a reader that should NOT change: the
    /// window is a claim about how far up the SCREEN a dialog sits
    /// (`a_dialog_further_up_than_the_window_does_not_match`), and rows are the unit distance is
    /// measured in. The risk was that wrapped continuations would eat the window and push a
    /// marker out of it.
    ///
    /// Measured across four widths rather than argued: 80 columns is the fixture's natural size,
    /// 30 wraps every line of the dialog into two or three rows. The verdict does not move, so the
    /// row window is the right unit here and this pins it — a later round that "fixes" this reader
    /// the way R344 fixed the search will find out here.
    #[test]
    fn a_dialog_still_reads_as_blocked_when_every_line_of_it_wraps() {
        for cols in [80_u16, 60, 40, 30] {
            let mut em = Emulator::new(cols, 24);
            em.advance(PERMISSION_DIALOG.join("\r\n").as_bytes());
            // Non-vacuity: at the narrow widths the pane really is wrapping.
            if cols <= 60 {
                assert!(
                    (0..em.screen().rows()).any(|row| em.screen().wrapped(row)),
                    "the fixture must wrap at {cols} columns or it says nothing",
                );
            }
            let v = detect(em.screen(), Some("✳ Claude Code"), &built_ins());
            assert_eq!(v.state, AgentState::Blocked, "at {cols} columns");
            assert_eq!(v.rule.as_deref(), Some("dialog-choice-list"), "at {cols}");
        }
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

    /// The vocabulary has ONE definition, so what a client reads is what a reporter may write — and
    /// `unknown` is the one asymmetry, on purpose.
    #[test]
    fn from_wire_round_trips_the_three_states_and_refuses_the_fourth() {
        for state in [AgentState::Working, AgentState::Blocked, AgentState::Idle] {
            let token = state.wire_str().expect("a known state has a token");
            assert_eq!(
                AgentState::from_wire(token),
                Some(state),
                "{token:?} must read back as the state it is published as",
            );
        }
        assert_eq!(
            AgentState::from_wire("unknown"),
            None,
            "`unknown` is a conclusion about the RULES, not a state a reporter can be in — a \
             reporter that no longer knows is asking to be scraped, which is a release",
        );
        assert_eq!(AgentState::from_wire(""), None);
        assert_eq!(
            AgentState::from_wire("Working"),
            None,
            "and the token is exact"
        );
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

    /// Both `codex` dialogs are read, and the test asserts WHY the marker had to become a class:
    /// the marker literal `claude` was written from misses both of them. Without this second half
    /// the widening would be untested — a parser that accepted anything would pass either way.
    #[test]
    fn both_codex_markers_are_read_and_claudes_lone_marker_would_have_missed_them() {
        let claude_only = Regex::new(r"(?m)^\s*❯\s+\d+\.[\s\S]*?^\s*\d+\.").expect("compiles");
        for fixture in [CODEX_SIGNIN_PICKER, CODEX_TRUST_DIALOG, CODEX_MODEL_PICKER] {
            let em = painted(fixture);
            assert!(
                question(em.screen(), 12).is_some(),
                "the widened marker class must read this dialog",
            );
            assert!(
                !claude_only.is_match(&bottom_lines(em.screen(), 12)),
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

    /// `codex`'s onboarding screen names the product, so the title-free fingerprint reaches it —
    /// and reaching it is worth more than the one screen, because of what follows it.
    #[test]
    fn the_codex_sign_in_picker_is_claimed_before_any_title_exists() {
        let v = codex_verdict(CODEX_SIGNIN_PICKER, None);
        assert_eq!(v.agent.as_deref(), Some("codex"));
        assert_eq!(
            v.state,
            AgentState::Blocked,
            "a sign-in picker is a person being asked a question",
        );
    }

    /// THE BOUND THAT REMAINS, measured rather than assumed: `codex`'s directory-trust dialog names
    /// nothing.
    ///
    /// Read the fixture. It says "Do you trust the contents of this directory", it shows a path, and
    /// it offers two choices — there is no token on that screen that belongs to `codex` rather than
    /// to any program that could ask the same question. So it is not claimable by a fingerprint at
    /// all, and the only thing that rescues it is having been claimed EARLIER in the same pane,
    /// which `a_remembered_agent_carries_through_the_dialog_that_names_nobody` holds one module
    /// over.
    ///
    /// A pane whose FIRST screen is this one, on a box where sign-in already happened, is therefore
    /// still a miss. That is the honest residue of this front and it is written down rather than
    /// rounded off.
    #[test]
    fn the_codex_trust_dialog_names_nobody_and_is_still_a_miss() {
        let v = codex_verdict(CODEX_TRUST_DIALOG, None);
        assert_eq!(
            v,
            Verdict::default(),
            "nothing on this screen identifies the agent that drew it",
        );

        // AND IT MUST NOT BE CLAIMED BY THE OTHER AGENT EITHER, which is not a hypothetical: this
        // fixture was captured under `/tmp/claude-1000/...`, so the string `claude` is on the
        // screen. A fingerprint written on the bare NAME rather than on a phrase would read a
        // `codex` pane as `claude` on any box whose scratch directory is named that way — a
        // cross-agent misattribution produced by the capture environment, not by either agent.
        let em = painted(CODEX_TRUST_DIALOG);
        let text = bottom_lines(em.screen(), DIALOG_WINDOW);
        assert!(
            text.contains("claude"),
            "the premise: the capture path really does put that string on this screen",
        );
        assert_eq!(
            detect(em.screen(), None, &[claude()]).agent,
            None,
            "and `claude`'s onboarding fingerprint is a PHRASE, so it does not fire on it",
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
