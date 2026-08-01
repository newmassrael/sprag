//! The per-pane memory: what a single frame cannot know.
//!
//! H3 slice 2. [`detect`](crate::detect) answers from one screen and one title, and that is the
//! whole of what it can do. Three of the measurements this front is built on are about a pane over
//! TIME, and none of them can be honoured without somewhere to remember:
//!
//! * **The working signal is an ANIMATION.** `claude`'s title alternates between braille frames at
//!   about 1 Hz (R249's M2). A detector that publishes every frame publishes a flicker, so a
//!   verdict that rests on NOT seeing that signal has to hold before it is believed. That is
//!   [`Hysteresis`], and M2 is why the design calls it a correctness requirement of the input
//!   rather than a later polish.
//! * **A quiet pane costs nothing, exactly.** An idle agent pane moved ZERO row generations over
//!   eight seconds (M3). Because the rules are a pure function of the screen, the title and the
//!   RULE LIST, a pane where none of the three has moved cannot reach a different verdict — so
//!   skipping the evaluation is a skip with a proof rather than a heuristic that trades accuracy for
//!   cost. The third input is the one slice 4 added and it is named here rather than left implied:
//!   the list is replaced when a user edits `config.toml`, and a gate watching only the first two
//!   would hold a stale verdict on every quiet pane for as long as it stayed quiet. See
//!   [`Ruleset`].
//! * **A modal hides WHO the pane belongs to.** A `codex` dialog covers the composer line and the
//!   footer its fingerprint is made of, so the pane goes unclaimed in the one state this front
//!   exists to report (R251). Nothing on that screen says `codex`, so no better fingerprint fixes
//!   it; only memory does.
//!
//! The clock arrives as a PARAMETER, the shape `Keymap::route` uses and for the same reason: a
//! settle window becomes arithmetic in a test instead of a sleep.

use std::time::{Duration, Instant};

use sprag_vt::Screen;

use crate::{Manifest, Ruleset, Verdict};

/// The default settle window.
///
/// Longer than the roughly 1 Hz the working spinner was measured alternating at (R249's M2),
/// because the artifact this window exists to absorb is one frame of that animation: a window
/// shorter than the period it guards against publishes exactly the flicker it was added for. Two
/// seconds rather than one leaves room for a slower box and a slower agent, and it is only ever
/// spent on a pane coming to REST — [`AgentState::is_active`](crate::AgentState::is_active) states
/// are published on sight, so the state a person is waiting for is never delayed by it.
pub const DEFAULT_SETTLE: Duration = Duration::from_secs(2);

/// How long a candidate state must hold before [`Tracker`] publishes it.
///
/// # One parameter, where the design named two
///
/// H3's design asked for "N consecutive evaluations, with a wall-clock cap so a slow-changing pane
/// still settles". The count is not a second mechanism, and this is the slice that had to find out:
///
/// * A pending candidate is REPLACED by any observation that disagrees with it, so "this has been
///   the answer since `t`" already means "nothing has disagreed since `t`". A count of agreeing
///   observations adds no evidence to that.
/// * Under the OR the design's own sentence implies — the cap exists precisely because a quiet pane
///   never reaches N — the count can only publish EARLIER than the window. On a pane that repaints
///   quickly, which is what an agent printing output IS, N observations arrive in milliseconds and
///   the guard collapses to nothing.
/// * Under an AND it is worse: a pane that goes quiet after one observation never reaches N and
///   freezes in its previous state forever, which is the failure the pending exception below exists
///   to prevent.
///
/// So the window is time, and time alone. The struct stays a struct because the option that will
/// carry it (slice 3, where an evaluation site exists to read it) wants a name, and because a
/// second knob added later must not change [`Tracker::new`]'s shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hysteresis {
    /// How long a candidate that rests on an ABSENCE must hold before it is published.
    pub settle: Duration,
}

impl Default for Hysteresis {
    fn default() -> Self {
        Self {
            settle: DEFAULT_SETTLE,
        }
    }
}

/// A verdict waiting to be believed, and when it was first seen.
#[derive(Debug)]
struct Pending {
    candidate: Verdict,
    since: Instant,
}

/// The inputs the rules read, as of the last evaluation — the quiescence key.
///
/// It is the exactness that earns the skip, so each field is here because the rules can read it
/// and for no other reason.
#[derive(Debug)]
struct Seen {
    /// Per-row damage stamps, which stand in for every row's text: a row whose stamp has not moved
    /// has not been written to. This is the same comparison the projection gate and the wire client
    /// already make (R218, R220), against the same stamps.
    generations: Vec<u64>,
    /// The width, because a row's TEXT is its cells and how many of them fit — and the two can move
    /// independently. `Screen::resized`, the resize path an alternate screen takes, copies every
    /// row's damage stamp verbatim while truncating the cells to the new width, so a narrowing
    /// resize is a content change no stamp records.
    cols: u16,
    /// The title, with an absent one stored as the empty string exactly as
    /// [`detect`](crate::detect) reads it — so two titles this cannot tell apart are two titles the
    /// rules cannot tell apart either.
    title: String,
    /// WHICH rules produced the last verdict ([`Ruleset::revision`](crate::Ruleset::revision)).
    ///
    /// The other three fields are what the rules READ; this one is the rules themselves. It belongs
    /// in the same key for the same reason: the skip claims a re-evaluation would reach the same
    /// answer, and a re-evaluation against a list the user has since edited would not.
    rules: u64,
}

impl Seen {
    fn of(screen: &Screen, title: &str, rules: u64) -> Self {
        let mut seen = Self {
            generations: Vec::new(),
            cols: 0,
            title: String::new(),
            rules,
        };
        seen.refresh(screen, title, rules);
        seen
    }

    /// Whether nothing the rules can read has moved, and the rules are still the same rules.
    fn unchanged(&self, screen: &Screen, title: &str, rules: u64) -> bool {
        self.rules == rules
            && self.cols == screen.cols()
            && self.title == title
            && self.generations.len() == screen.rows() as usize
            && (0..screen.rows())
                .all(|row| screen.row_generation(row) == Some(self.generations[row as usize]))
    }

    /// Take the reading again, reusing the buffers so a steady-state pane allocates nothing.
    fn refresh(&mut self, screen: &Screen, title: &str, rules: u64) {
        self.rules = rules;
        self.generations.clear();
        self.generations
            .extend((0..screen.rows()).map(|row| screen.row_generation(row).unwrap_or_default()));
        self.cols = screen.cols();
        if self.title != title {
            self.title.clear();
            self.title.push_str(title);
        }
    }
}

/// One pane's agent state over time: the quiescence gate, the settle window, and the identity a
/// modal covers.
///
/// One per pane, held by whoever serves the pane's facts. Slice 3 puts it in a host-side registry,
/// NOT on `sprag_terminal::Pane` as this comment said when slice 2 wrote it: `sprag-terminal` is the
/// producer and owns the emulator and the PTY only, while a detector is scene-side — the division
/// `sprag-grid` already sits on the far side of. It owns no clock and no manifests: `now` and the
/// manifest list arrive on every [`observe`](Self::observe) call, so a workspace has one manifest
/// list and a test has arithmetic.
///
/// [`observe`](Self::observe) is meant to be called on EVERY tick, including the ticks where
/// nothing has happened. The quiescence gate lives inside it rather than at the call site, so a
/// caller cannot accidentally hold a pane's pending transition open by deciding for itself that
/// nothing was worth asking about.
///
/// # There is no tick, so a pending transition has to ASK for one
///
/// Slice 2 wrote "every tick" as though a tick existed. Slice 3 read the daemon and found none: the
/// pane list is served when a client asks, and a client asks when the scene revision moves, which
/// pane OUTPUT and user ACTIONS advance. That is enough for a verdict resting on present evidence —
/// the output that paints a dialog is the same event that wakes the reader — and it is not enough for
/// one resting on an absence, because the last thing to move the revision was the output that
/// STOPPED. A pane going quiet is a transition whose confirming observation nothing produces.
///
/// So [`pending_deadline`](Self::pending_deadline) exists: it is the instant at which this tracker
/// would publish if it were asked, and it is `None` when nothing is pending. A caller drives the
/// clock by observing again at that instant, and a caller with nothing pending owes nothing — which
/// keeps M3's "a quiet workspace costs nothing" true of the confirmation as well as of the
/// evaluation.
#[derive(Debug)]
pub struct Tracker {
    policy: Hysteresis,
    /// The verdict on the wire. [`Verdict::default`] until something is published.
    published: Verdict,
    /// Increments on every PUBLISHED change, so a client can tell "still blocked" from "blocked
    /// again" without diffing strings — the treatment `notification_seq` already gets.
    seq: u64,
    pending: Option<Pending>,
    seen: Option<Seen>,
    /// Which agent this pane was last IDENTIFIED as, independent of what it is doing.
    ///
    /// By name rather than by index into the manifest list, because slice 4 reloads that list from
    /// a file: a name survives a reload and a position does not.
    identity: Option<String>,
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new(Hysteresis::default())
    }
}

impl Tracker {
    /// A tracker for one pane, with nothing published yet.
    #[must_use]
    pub fn new(policy: Hysteresis) -> Self {
        Self {
            policy,
            published: Verdict::default(),
            seq: 0,
            pending: None,
            seen: None,
            identity: None,
        }
    }

    /// The verdict currently published for this pane.
    #[must_use]
    pub const fn verdict(&self) -> &Verdict {
        &self.published
    }

    /// How many times the published verdict has changed.
    #[must_use]
    pub const fn seq(&self) -> u64 {
        self.seq
    }

    /// When a pending candidate would be published, or `None` when nothing is pending.
    ///
    /// The instant is derived (`since + settle`) rather than stored, so a policy that changes while a
    /// candidate is pending moves the deadline it is already waiting on — which is what a user who
    /// just shortened the window means by shortening it. See [`set_policy`](Self::set_policy).
    ///
    /// A caller uses this to know when to [`observe`](Self::observe) again; see the type's own docs
    /// for why the answer cannot be left to whatever else happens to be moving.
    #[must_use]
    pub fn pending_deadline(&self) -> Option<Instant> {
        self.pending
            .as_ref()
            .map(|pending| pending.since + self.policy.settle)
    }

    /// Replace the settle policy.
    ///
    /// Exists because the window is a user OPTION and this project's options are read from the file
    /// on every call rather than held — the daemon is a reader of the user's config, not an owner of
    /// it, so `set-option` takes effect with nothing to restart. A tracker built once at a pane's
    /// first sighting would otherwise pin the window at whatever the file said that day.
    ///
    /// It does NOT touch a pending candidate's `since`: the window is how long the answer must hold,
    /// and re-dating the evidence because the policy moved would restart a wait the pane has already
    /// served — the same defect as re-starting the window on every re-observation, which
    /// `a_pane_that_keeps_repainting_settles_when_the_candidate_has_held_long_enough` pins.
    pub const fn set_policy(&mut self, policy: Hysteresis) {
        self.policy = policy;
    }

    /// Take a reading of the pane and return what is published for it now.
    ///
    /// Call it on every tick. Two things can happen without the rules running at all, and both are
    /// deliberate:
    ///
    /// * **Nothing the rules read has moved and nothing is pending** — the answer cannot have
    ///   changed, so it is not recomputed. The skip is exact, not a sampling policy.
    /// * **Nothing has moved but a transition IS pending** — the rules still do not run, because
    ///   with both their inputs unchanged they would reach the candidate they already reached. What
    ///   has moved is the CLOCK, which is a third input while a transition is pending, and asking
    ///   it is what keeps a pane that went quiet mid-transition from freezing in its previous state
    ///   forever.
    /// * **The RULES have moved** — a user edited `config.toml`, so the list is not the list that
    ///   produced the published verdict. The screen and the title are unchanged and the answer can
    ///   still differ, which is why [`Ruleset`] carries its identity into the key rather than
    ///   leaving the caller to remember to invalidate anything.
    pub fn observe(
        &mut self,
        screen: &Screen,
        title: Option<&str>,
        rules: &Ruleset,
        now: Instant,
    ) -> &Verdict {
        let title = title.unwrap_or_default();
        let revision = rules.revision();
        if self
            .seen
            .as_ref()
            .is_some_and(|seen| seen.unchanged(screen, title, revision))
        {
            self.settle(now);
            return &self.published;
        }
        if let Some(seen) = &mut self.seen {
            seen.refresh(screen, title, revision);
        } else {
            self.seen = Some(Seen::of(screen, title, revision));
        }
        let candidate = self.evaluate(screen, title, rules.manifests());
        self.consider(candidate, now);
        &self.published
    }

    /// Which rules produced the published verdict, or `None` for a pane never observed.
    ///
    /// The waker's test for "this pane's answer was reached under a list that has since been
    /// replaced". A pane can be neither due nor unknown and still owe an evaluation, which is the
    /// third reason to ask about a pane and the one slice 4 added.
    #[must_use]
    pub fn evaluated_under(&self) -> Option<u64> {
        self.seen.as_ref().map(|seen| seen.rules)
    }

    /// The verdict this frame argues for, with the memory consulted where the screen has gone
    /// silent about who the pane belongs to.
    ///
    /// Identification is [`detect`](crate::detect)'s rather than a second walk of the list here,
    /// and the reason is the same one [`Rule::id`](crate::Rule::id) gives for keeping arbitration in
    /// one function: a second matcher is a matcher that can disagree. It also leaves exactly one
    /// place where the rules run, which is what lets [`work`](crate::work) meter the cost the
    /// quiescence gate exists to avoid — see [`DetectWork`](crate::DetectWork). A manifest that
    /// claims a pane always names itself on the verdict, so `agent.is_some()` is precisely "the
    /// list claimed this pane".
    fn evaluate(&mut self, screen: &Screen, title: &str, manifests: &[Manifest]) -> Verdict {
        let claimed = crate::detect(screen, Some(title), manifests);
        if claimed.agent.is_some() {
            if self.identity != claimed.agent {
                self.identity.clone_from(&claimed.agent);
            }
            return claimed;
        }
        // Nothing claims the pane — which is what a `codex` pane looks like the moment a modal
        // covers the composer and the footer (R251). A pane that was an agent a moment ago is
        // still that agent, so the remembered manifest is asked directly.
        //
        // Only for an ACTIVE verdict, and that bound is the same measurement read the other way:
        // at rest a `codex` title is the bare working directory and its screen is a prompt line,
        // which is what a SHELL looks like too. Without the fingerprint there is nothing left to
        // tell an agent at rest from the shell that outlived it, so a resting pane is not asserted
        // to still be anybody — while a dialog or a spinner is evidence the remembered agent is
        // still there, and is exactly the state the memory exists to rescue.
        self.identity
            .as_deref()
            .and_then(|name| manifests.iter().find(|m| m.name == name))
            .map(|manifest| manifest.verdict(screen, title))
            .filter(|verdict| verdict.state.is_active())
            .unwrap_or_default()
    }

    /// Weigh a fresh candidate against what is published.
    fn consider(&mut self, candidate: Verdict, now: Instant) {
        if candidate == self.published {
            // The pane moved without changing what it MEANS — the next spinner frame, another line
            // of output. Whatever transition was pending is off, because something disagreed with
            // it.
            self.pending = None;
            return;
        }
        if candidate.state.is_active() {
            // Positive evidence: a spinner frame in the title, a choice list on the screen. There
            // is no sampling artifact that INVENTS one of those, and the state a person is waiting
            // for is the one it would be perverse to delay.
            self.publish(candidate);
            return;
        }
        let restart = !matches!(&self.pending, Some(pending) if pending.candidate == candidate);
        if restart {
            self.pending = Some(Pending {
                candidate,
                since: now,
            });
        }
        self.settle(now);
    }

    /// Publish a pending candidate once it has held for the settle window.
    fn settle(&mut self, now: Instant) {
        let Some(pending) = &self.pending else {
            return;
        };
        if now.saturating_duration_since(pending.since) >= self.policy.settle {
            let candidate = pending.candidate.clone();
            self.publish(candidate);
        }
    }

    fn publish(&mut self, verdict: Verdict) {
        self.pending = None;
        // Let the identity go exactly when the published answer is that nobody claims this pane.
        // The memory and the wire then agree, and a pane an agent has EXITED cannot go on being
        // reported as that agent the next time something dialog-shaped appears in it.
        if verdict.agent.is_none() {
            self.identity = None;
        }
        self.published = verdict;
        self.seq += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentState, claude, codex, detect};
    use sprag_vt::{Emulator, VtPort};

    /// The measurement the settle window is sized against: R249's M2 sampled `claude`'s working
    /// title alternating between two braille frames at about 1 Hz.
    const MEASURED_SPINNER_PERIOD: Duration = Duration::from_secs(1);

    fn painted(lines: &[&str]) -> Emulator {
        let mut em = Emulator::new(80, 24);
        em.advance(lines.join("\r\n").as_bytes());
        em
    }

    /// Repaint the SAME pane, the way an agent redrawing its screen does — so the row damage
    /// stamps move exactly as they would live, rather than being compared across two emulators
    /// that never shared a generation counter.
    fn repaint(em: &mut Emulator, lines: &[&str]) {
        em.advance(b"\x1b[2J\x1b[H");
        em.advance(lines.join("\r\n").as_bytes());
    }

    /// Enough of a `claude` pane to be claimed by its footer fingerprint with no title at all.
    const CLAUDE_FOOTER: &[&str] = &["❯", "  ⏸ manual mode on · ? for shortcuts"];

    /// The smallest screen that is a choice list. The dialog RULE's fidelity to a real agent is
    /// slice 1's business, proven there against three captured `claude` dialogs and three `codex`
    /// ones; these tests are about time, and need only a screen the rule fires on.
    const DIALOG: &[&str] = &["❯ 1. Yes", "  2. No"];

    /// A `codex` pane at rest: the composer line and the footer shape, which is the conjunction its
    /// fingerprint is made of.
    const CODEX_AT_REST: &[&str] = &[
        "› Write tests for @filename",
        "  gpt-5.6-sol default · /tmp",
    ];

    /// A `codex` modal, which covers both halves of that conjunction — the R251 finding, in the
    /// smallest form that reproduces it. Every test using it asserts that it really is unclaimed
    /// rather than assuming it.
    const CODEX_MODAL: &[&str] = &[
        "  Select Model",
        "› 1. gpt-5.6-sol (current)",
        "  2. gpt-5.5",
    ];

    #[test]
    fn the_default_settle_window_outlasts_the_measured_spinner_period() {
        assert!(
            DEFAULT_SETTLE > MEASURED_SPINNER_PERIOD,
            "a window shorter than the animation it guards against publishes the flicker it exists \
             to absorb",
        );
    }

    /// D9: the deadline is the tracker's whole answer to "when should somebody ask me again", so it
    /// has to be absent exactly when nothing is waiting and exact when something is.
    #[test]
    fn the_pending_deadline_is_the_instant_a_waiting_candidate_would_publish() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        // A dialog publishes on sight, so nothing is left waiting.
        repaint(&mut em, DIALOG);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Blocked);
        assert_eq!(
            tracker.pending_deadline(),
            None,
            "a verdict published on sight leaves nothing to come back for",
        );

        // The dialog goes away: a return to rest is an ABSENCE, so it waits — and says until when.
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(
            tracker.pending_deadline(),
            Some(base + DEFAULT_SETTLE),
            "the deadline is when the candidate was first seen plus the window",
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "and it has NOT published yet — otherwise the deadline above is describing nothing",
        );

        // Asked at the deadline, it publishes and the deadline is spent.
        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &rules,
            base + DEFAULT_SETTLE,
        );
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        assert_eq!(tracker.pending_deadline(), None);
    }

    /// F3: the window is a user option, so it can move while a candidate is already waiting. It moves
    /// the DEADLINE and not the evidence — a pane that has already held for the new window is done
    /// waiting, rather than starting over because the user typed `set-option`.
    #[test]
    fn a_shortened_window_publishes_a_candidate_that_has_already_held_long_enough() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        repaint(&mut em, DIALOG);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        repaint(&mut em, CLAUDE_FOOTER);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.pending_deadline(), Some(base + DEFAULT_SETTLE));

        let shorter = Duration::from_millis(250);
        tracker.set_policy(Hysteresis { settle: shorter });
        assert_eq!(
            tracker.pending_deadline(),
            Some(base + shorter),
            "the candidate keeps the instant it was first seen; only the window moved",
        );

        // Half of the OLD window is already twice the new one, so the next reading publishes.
        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &rules,
            base + DEFAULT_SETTLE / 2,
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "time already served counts against the new window",
        );
    }

    /// The list exists so a caller compiles the patterns once; if it ever stops carrying every
    /// built-in, a pane of that agent goes unclaimed on the wire with nothing failing here.
    #[test]
    fn the_built_in_list_carries_every_built_in_manifest() {
        let names: Vec<String> = crate::built_ins().into_iter().map(|m| m.name).collect();
        assert_eq!(names, vec![claude().name, codex().name]);
    }

    /// The gate H3's design named for this slice: one animation is one publication.
    #[test]
    fn a_spinner_animation_publishes_one_working_not_six() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        for (tick, frame) in ["⠂", "⠐", "⠂", "⠐", "⠂", "⠐"].iter().enumerate() {
            let title = format!("{frame} Run sleep command for 25 seconds");
            let now = base + MEASURED_SPINNER_PERIOD * u32::try_from(tick).expect("six of them");
            let verdict = tracker.observe(em.screen(), Some(&title), &rules, now);
            assert_eq!(verdict.state, AgentState::Working, "frame {frame}");
        }
        assert_eq!(
            tracker.seq(),
            1,
            "six frames of one animation are one publication",
        );

        // Non-vacuity: a tracker that had frozen after the first frame would have passed every
        // assertion above. This one is still reading the pane.
        repaint(&mut em, DIALOG);
        let verdict = tracker.observe(
            em.screen(),
            Some("⠂ Run sleep command for 25 seconds"),
            &rules,
            base + Duration::from_secs(6),
        );
        assert_eq!(verdict.state, AgentState::Blocked);
        assert_eq!(tracker.seq(), 2);
    }

    /// The RULES are the quiescence key's fourth term, and this test replaces the one R252 wrote.
    ///
    /// R252 proved the skip by rewriting a rule underneath the tracker and asserting the verdict did
    /// NOT move: only an evaluation could notice the rewrite, so a frozen verdict meant the rules had
    /// not run. That instrument is the defect slice 4 exists to fix — a user edits `config.toml` to
    /// correct a rule that is misfiring, and the pane they are watching is quiet, which is why the
    /// wrong answer is visible and stuck there. So the assertion is INVERTED here.
    ///
    /// What is lost with it is the only behavioural view of the skip, and that is worth stating
    /// rather than discovering: an exact skip is one whose absence changes no answer, so with the
    /// rules in the key there is nothing left for a test to see. The gate's remaining proof is the
    /// evaluation it does not run, which is a COST, and cost is what H3's open measurement 3
    /// instruments.
    #[test]
    fn a_rewritten_rule_reaches_a_pane_that_has_not_moved() {
        let mut rewritten = claude();
        rewritten
            .rules
            .iter_mut()
            .find(|rule| rule.id == "idle-glyph")
            .expect("the manifest has an idle rule")
            .state = AgentState::Working;

        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let title = Some("✳ Claude Code");
        let base = Instant::now();

        let rules = Ruleset::new(vec![claude()]);
        tracker.observe(em.screen(), title, &rules, base);
        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE);
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        let generations: Vec<Option<u64>> = (0..em.screen().rows())
            .map(|row| em.screen().row_generation(row))
            .collect();

        let edited = Ruleset::new(vec![rewritten]);
        tracker.observe(em.screen(), title, &edited, base + DEFAULT_SETTLE * 2);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "a replaced rule list is a changed input, so the answer is recomputed",
        );

        // The premise, asserted rather than assumed — the same discipline R252's resize test
        // applies. If the pane had repainted, the re-evaluation would be explained by the screen
        // and this test would prove nothing about the rules term.
        assert_eq!(
            generations,
            (0..em.screen().rows())
                .map(|row| em.screen().row_generation(row))
                .collect::<Vec<_>>(),
            "not one row moved between the two verdicts",
        );
    }

    /// `codex`'s sign-in picker, which NAMES the product — the screen the title-free fingerprint
    /// was built for.
    const CODEX_SIGNIN: &[&str] = &[
        "  Welcome to Codex, OpenAI's command-line coding agent",
        "> 1. Sign in with ChatGPT",
        "  2. Sign in with Device Code",
    ];

    /// ...and the directory-trust dialog that follows it, which names NOBODY. Any program could ask
    /// this question, so no fingerprint can claim it and only the memory can.
    const CODEX_TRUST: &[&str] = &[
        "  Do you trust the contents of this directory?",
        "› 1. Yes, continue",
        "  2. No, quit",
    ];

    /// The onboarding SEQUENCE, which is worth more than either screen alone.
    ///
    /// Slice 1 shipped `codex`'s trust dialog as an unclaimable screen and it still is one: nothing
    /// on it belongs to `codex` rather than to any program that could ask the same question. What
    /// closes it is not a better fingerprint but the ORDER a user meets it in — sign-in first, and
    /// that screen does name the product. Once the pane has been claimed, the identity memory the
    /// modal case (R251) was built for carries it through.
    ///
    /// So the two mechanisms compose: the fingerprint reaches a screen the memory could not have
    /// been seeded from, and the memory reaches a screen no fingerprint can match. The premise is
    /// asserted first, because a test whose second half passes for the wrong reason would look
    /// exactly like this one.
    #[test]
    fn a_remembered_agent_carries_through_the_dialog_that_names_nobody() {
        let rules = Ruleset::new(vec![codex()]);
        let base = Instant::now();

        // THE PREMISE: on its own, the trust dialog is claimed by nobody.
        let mut cold = Tracker::default();
        let alone = painted(CODEX_TRUST);
        cold.observe(alone.screen(), None, &rules, base);
        assert_eq!(
            cold.verdict().agent,
            None,
            "the screen really is unclaimable, so the sequence below is what does the work",
        );

        // The pane a user actually has: sign-in, then trust.
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_SIGNIN);
        tracker.observe(em.screen(), None, &rules, base);
        assert_eq!(
            tracker.verdict().agent.as_deref(),
            Some("codex"),
            "the picker names the product, so the fingerprint reaches it with no title at all",
        );

        repaint(&mut em, CODEX_TRUST);
        let after = tracker.observe(em.screen(), None, &rules, base + Duration::from_millis(50));
        assert_eq!(
            after.agent.as_deref(),
            Some("codex"),
            "and the memory carries the identity onto the screen that names nobody",
        );
        assert_eq!(
            after.state,
            AgentState::Blocked,
            "which is the whole point: it is a question waiting on the user",
        );
    }

    /// THE MISS THAT IS NOT CLOSED, asserted at last — and slice 1 said it was.
    ///
    /// The crate docs have recorded a second measured miss since slice 1 (`codex` replaces its
    /// footer with a transient hint for a few seconds after `esc`, and its fingerprint is a
    /// conjunction over that footer) and claimed *"both are recorded here, and asserted by tests"*.
    /// Only the onboarding one ever was. There is no captured screen of the hint in this tree, so
    /// this test is written from the STRUCTURE the miss follows from rather than from a fixture
    /// pretending to be a measurement: the composer is there and the footer is not.
    ///
    /// What it pins is the reason the memory does not rescue this one, which is the part that looks
    /// like a bug and is not. `evaluate` asserts a remembered identity only for an ACTIVE verdict,
    /// and a pane that has just been `esc`-ed is at rest. Widening that filter would trade this miss
    /// for a worse one: at rest a `codex` screen is a prompt line and its title is a bare directory
    /// name, so a pane whose agent EXITED would be reported as that agent for as long as the shell
    /// that outlived it stayed quiet.
    ///
    /// REVERT-PROOF: drop the `is_active` filter in `evaluate` and this fails, together with
    /// `a_remembered_agent_that_shows_nothing_active_is_let_go` and
    /// `a_resize_that_truncates_a_row_is_not_quiescence`. Naming the co-casualties is the point
    /// rather than an aside: the filter already had a test, so what this one adds is not coverage of
    /// the filter but the connection between the filter and a MISS the crate docs describe — the two
    /// were recorded in different places and nothing said they were the same fact.
    #[test]
    fn a_resting_pane_that_loses_its_fingerprint_is_not_asserted_to_still_be_anybody() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let base = Instant::now();

        // Claimed normally first, so the memory HAS an identity to offer. `idle` rests on an
        // absence, so it takes the window to be published — hence the second observation.
        let mut em = painted(CODEX_AT_REST);
        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let claimed = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            base + DEFAULT_SETTLE,
        );
        assert_eq!(
            claimed.agent.as_deref(),
            Some("codex"),
            "the composer-and-footer conjunction claims a pane at rest",
        );

        // The footer is replaced. Structurally this is the post-`esc` hint: the composer survives,
        // the footer conjunction cannot hold, and nothing about the screen is active.
        repaint(
            &mut em,
            &["› Write tests for @filename", "  press esc again to clear"],
        );
        let began = base + DEFAULT_SETTLE + Duration::from_millis(10);
        tracker.observe(em.screen(), Some("codexprobe"), &rules, began);
        let after = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            began + DEFAULT_SETTLE,
        );
        assert_eq!(
            after.agent, None,
            "the memory declines to assert an identity for a pane that is merely quiet",
        );
    }

    /// A RELOAD DOES NOT RESTART A WAIT THE PANE HAS ALREADY SERVED.
    ///
    /// Every reload test written so far uses a SETTLED pane, which is the easy half: nothing is in
    /// flight, so nothing can be lost. The pane mid-transition is the one with a decision in it,
    /// and the decision is [`consider`](Tracker::consider)'s `restart` guard — a candidate the new
    /// rules still argue for keeps its original `since`.
    ///
    /// That is right because of what the window is FOR. It absorbs a sampling artifact of the
    /// SCREEN — one frame of a spinner — and the screen here has held for the whole window. The
    /// rules moving is not a reason to disbelieve evidence that has not moved. The tempting
    /// implementation, re-dating the candidate on every re-evaluation, would make a user who edits
    /// `config.toml` while a pane is going quiet wait a second window for a verdict the first one
    /// had already earned — and would do it invisibly, because the answer that eventually arrives
    /// is the right one.
    ///
    /// The assertion is the DEADLINE, not the verdict: observing at exactly `since + settle` is
    /// what a re-dated candidate cannot satisfy.
    ///
    /// REVERT-PROOF: make `restart` unconditional and this fails at the deadline — together with
    /// `a_pane_that_keeps_repainting_settles_when_the_candidate_has_held_long_enough`, and that
    /// second casualty is worth naming rather than hiding. The guard was already load-bearing for a
    /// pane whose SCREEN keeps moving; what had never been asked is whether it holds when the thing
    /// that moves is the RULE LIST, which reaches `consider` by a different route and was the one
    /// input the guard was not written against.
    #[test]
    fn a_reload_mid_transition_keeps_the_window_the_candidate_has_already_served() {
        let mut tracker = Tracker::default();
        let mut em = painted(DIALOG);
        let base = Instant::now();

        // Blocked on sight — positive evidence is never delayed, so the pane starts from a
        // published state with nothing pending.
        let rules = Ruleset::new(vec![claude()]);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Blocked);

        // The dialog is answered: the pane now argues for `idle`, which rests on an ABSENCE and so
        // has to hold for the window.
        repaint(&mut em, CLAUDE_FOOTER);
        let began = base + Duration::from_millis(10);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, began);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "the candidate is pending, not published",
        );
        assert_eq!(tracker.pending_deadline(), Some(began + DEFAULT_SETTLE));

        // HALFWAY THROUGH THE WINDOW the user edits an unrelated part of the file. A new list, a
        // new revision, and the same verdict for this screen.
        let reloaded = Ruleset::new(vec![claude()]);
        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &reloaded,
            began + DEFAULT_SETTLE / 2,
        );
        assert_eq!(
            tracker.pending_deadline(),
            Some(began + DEFAULT_SETTLE),
            "the deadline is the one the candidate earned, not one re-dated by the edit",
        );

        tracker.observe(
            em.screen(),
            Some("✳ Claude Code"),
            &reloaded,
            began + DEFAULT_SETTLE,
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "so it publishes on time",
        );
    }

    /// ...and a reload the pane DISAGREES with does restart it, which is the same rule read from
    /// the other side: the window is per-candidate, so a different answer serves its own wait.
    ///
    /// Written because the guard above is only half a claim. An implementation that never restarted
    /// would satisfy the test above and would publish a brand-new candidate the instant an older
    /// one's window happened to elapse — a verdict with no evidence behind it at all.
    #[test]
    fn a_reload_the_pane_disagrees_with_starts_the_window_again() {
        let mut tracker = Tracker::default();
        let mut em = painted(DIALOG);
        let base = Instant::now();

        let rules = Ruleset::new(vec![claude()]);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Blocked);

        repaint(&mut em, CLAUDE_FOOTER);
        let began = base + Duration::from_millis(10);
        tracker.observe(em.screen(), Some("✳ Claude Code"), &rules, began);
        assert_eq!(tracker.pending_deadline(), Some(began + DEFAULT_SETTLE));

        // The user disables the rule the pending candidate came from. The same screen now argues
        // for a DIFFERENT answer, so the evidence for the old one is gone.
        let mut stripped = claude();
        stripped.rules.retain(|rule| rule.id != "idle-glyph");
        let edited = Ruleset::new(vec![stripped]);
        let moved = began + DEFAULT_SETTLE / 2;
        tracker.observe(em.screen(), Some("✳ Claude Code"), &edited, moved);
        assert_eq!(
            tracker.pending_deadline(),
            Some(moved + DEFAULT_SETTLE),
            "a candidate nothing has argued for before starts its own window",
        );
        assert_eq!(
            tracker.verdict().state,
            AgentState::Blocked,
            "and nothing is published until it has served it",
        );
    }

    /// The same list, offered again, is not a change — the other direction of the term above, and the
    /// one that keeps it from being "re-evaluate always" wearing a key's costume.
    ///
    /// This is also slice 3's two-clients-one-wake property read from underneath: two readers of one
    /// pane observe against the same ruleset, so the second call must publish nothing new.
    #[test]
    fn the_same_ruleset_offered_twice_publishes_nothing_new() {
        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let title = Some("✳ Claude Code");
        let base = Instant::now();
        let rules = Ruleset::new(vec![claude()]);

        tracker.observe(em.screen(), title, &rules, base);
        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE);
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        let settled = tracker.seq();

        tracker.observe(em.screen(), title, &rules, base + DEFAULT_SETTLE * 2);
        assert_eq!(
            tracker.seq(),
            settled,
            "the same rules on an unmoved pane are the same answer",
        );
    }

    /// A resize is a content change no damage stamp records, and the test asserts that premise
    /// rather than assuming it — if `sprag-vt` ever stamps rows on resize, this says the premise
    /// died instead of quietly becoming vacuous.
    #[test]
    fn a_resize_that_truncates_a_row_is_not_quiescence() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = Emulator::new(80, 24);
        // The alternate screen, because that is the resize path that copies the stamps verbatim
        // (`Screen::resized`); the main screen reflows and stamps every row afresh.
        em.advance(b"\x1b[?1049h");
        em.advance(CLAUDE_FOOTER.join("\r\n").as_bytes());
        let base = Instant::now();

        tracker.observe(em.screen(), None, &rules, base);
        let verdict = tracker.observe(em.screen(), None, &rules, base + DEFAULT_SETTLE);
        assert_eq!(
            verdict.agent.as_deref(),
            Some("claude"),
            "claimed by the footer fingerprint, with no title at all",
        );

        let before: Vec<Option<u64>> = (0..em.screen().rows())
            .map(|row| em.screen().row_generation(row))
            .collect();
        em.resize(20, 24);
        let after: Vec<Option<u64>> = (0..em.screen().rows())
            .map(|row| em.screen().row_generation(row))
            .collect();
        assert_eq!(
            before, after,
            "the premise: this resize moved no damage stamp, so only the width can carry it",
        );

        tracker.observe(em.screen(), None, &rules, base + DEFAULT_SETTLE);
        tracker.observe(em.screen(), None, &rules, base + DEFAULT_SETTLE * 2);
        assert_eq!(
            tracker.verdict().agent,
            None,
            "the footer was truncated away, and the rules were asked because the width moved",
        );
    }

    /// The pending exception: a pane that goes quiet mid-transition still settles, because while a
    /// transition is pending the clock is an input.
    #[test]
    fn a_pending_transition_settles_on_the_clock_although_nothing_moved() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ Compacting"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Working);

        let rested = Some("✳ Compacting");
        let began = base + Duration::from_millis(100);
        tracker.observe(em.screen(), rested, &rules, began);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "an absence has to hold before it is believed",
        );

        // From here NOTHING moves: same screen, same title. Only the clock advances.
        tracker.observe(em.screen(), rested, &rules, began + DEFAULT_SETTLE / 2);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Working,
            "and it has not held long enough yet",
        );

        tracker.observe(em.screen(), rested, &rules, began + DEFAULT_SETTLE);
        assert_eq!(tracker.verdict().state, AgentState::Idle);
        assert_eq!(tracker.seq(), 2, "one transition, one publication");
    }

    /// The window is measured from when a candidate was FIRST seen, not from the last time it was
    /// seen again. A pane that keeps repainting — an agent printing its transcript — re-reaches the
    /// same candidate on every tick, and a window restarted by each of those never expires at all:
    /// the pane would be stuck in its previous state for as long as it stayed busy, which is the
    /// same freeze the pending exception exists to prevent, arriving by the other door.
    #[test]
    fn a_pane_that_keeps_repainting_settles_when_the_candidate_has_held_long_enough() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ Reading files"), &rules, base);
        assert_eq!(tracker.verdict().state, AgentState::Working);

        let rested = Some("✳ Reading files");
        let began = base + Duration::from_millis(100);
        tracker.observe(em.screen(), rested, &rules, began);

        // The spinner has stopped but the transcript has not: every tick moves a row.
        let mut printed = CLAUDE_FOOTER.to_vec();
        for tick in 1..=3 {
            printed.insert(0, "● and then it said something else");
            repaint(&mut em, &printed);
            tracker.observe(
                em.screen(),
                rested,
                &rules,
                began + Duration::from_millis(500) * tick,
            );
            assert_eq!(
                tracker.verdict().state,
                AgentState::Working,
                "tick {tick}: the candidate has not held long enough yet",
            );
        }

        printed.insert(0, "● one more");
        repaint(&mut em, &printed);
        tracker.observe(em.screen(), rested, &rules, began + DEFAULT_SETTLE);
        assert_eq!(
            tracker.verdict().state,
            AgentState::Idle,
            "it has been the answer for the whole window, busy pane or not",
        );
    }

    /// A pause shorter than the window is absorbed entirely — the flicker M2 made hysteresis a
    /// correctness requirement for.
    #[test]
    fn a_pause_in_the_animation_does_not_publish_a_return_to_rest() {
        let rules = Ruleset::new(vec![claude()]);
        let mut tracker = Tracker::default();
        let em = painted(CLAUDE_FOOTER);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("⠂ Working"), &rules, base);
        tracker.observe(
            em.screen(),
            Some("✳ Working"),
            &rules,
            base + MEASURED_SPINNER_PERIOD,
        );
        // A tick DURING the pause, or this test would pass with a window of one nanosecond: it is
        // the window that has to be asked, not merely the pane.
        tracker.observe(
            em.screen(),
            Some("✳ Working"),
            &rules,
            base + MEASURED_SPINNER_PERIOD + MEASURED_SPINNER_PERIOD / 2,
        );
        assert_eq!(tracker.verdict().state, AgentState::Working);
        tracker.observe(
            em.screen(),
            Some("⠐ Working"),
            &rules,
            base + MEASURED_SPINNER_PERIOD * 2,
        );

        assert_eq!(tracker.verdict().state, AgentState::Working);
        assert_eq!(tracker.seq(), 1, "the pause never reached the wire");
    }

    /// The asymmetry, with both sides driven at the SAME instants so the difference is the policy
    /// and not the timings: evidence that is PRESENT publishes on sight, evidence that is an
    /// absence has to hold.
    #[test]
    fn a_dialog_publishes_at_once_where_a_return_to_rest_waits() {
        let rules = Ruleset::new(vec![claude()]);
        let base = Instant::now();
        let mut asking = Tracker::default();
        let mut resting = Tracker::default();
        let mut em_asking = painted(CLAUDE_FOOTER);
        let em_resting = painted(CLAUDE_FOOTER);

        asking.observe(em_asking.screen(), Some("⠂ x"), &rules, base);
        resting.observe(em_resting.screen(), Some("⠂ x"), &rules, base);

        let moment = base + Duration::from_millis(100);
        repaint(&mut em_asking, DIALOG);
        asking.observe(em_asking.screen(), Some("✳ x"), &rules, moment);
        resting.observe(em_resting.screen(), Some("✳ x"), &rules, moment);

        assert_eq!(asking.verdict().state, AgentState::Blocked);
        assert_eq!(
            asking.seq(),
            2,
            "the state a person is waiting for is not delayed"
        );
        assert_eq!(
            resting.verdict().state,
            AgentState::Working,
            "the same instant, and this one still has to hold",
        );
        assert_eq!(resting.seq(), 1);
    }

    /// R251's finding, which is what grew this slice: a modal covers the two things `codex`'s
    /// fingerprint is made of, so the pane the front exists to report is the pane no fingerprint
    /// claims. The memory supplies the half the screen has hidden.
    #[test]
    fn a_modal_that_covers_the_fingerprint_keeps_the_agent_the_pane_already_was() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_AT_REST);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let verdict = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            base + DEFAULT_SETTLE,
        );
        assert_eq!(verdict.agent.as_deref(), Some("codex"));
        assert_eq!(verdict.state, AgentState::Idle);

        repaint(&mut em, CODEX_MODAL);
        // The premise, asserted rather than assumed: from one frame this pane is nobody's.
        assert_eq!(
            detect(em.screen(), Some("codexprobe"), rules.manifests()),
            Verdict::default(),
            "if this screen were still claimed the test would prove nothing about memory",
        );

        let verdict = tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            base + DEFAULT_SETTLE + Duration::from_millis(100),
        );
        assert_eq!(verdict.state, AgentState::Blocked);
        assert_eq!(
            verdict.agent.as_deref(),
            Some("codex"),
            "the memory answered what the screen would not",
        );
    }

    /// The OTHER measured miss this slice inherited, and the settle window is what absorbs it:
    /// `codex` replaces its footer with a transient hint for a few seconds, so the fingerprint's
    /// conjunction fails and the pane briefly belongs to nobody. Because a resting verdict has to
    /// hold, that flicker never reaches the wire at all — which is why the window applies to a
    /// FIRST publication too, and not only to a state that is being left.
    #[test]
    fn a_fingerprint_covered_for_less_than_the_window_never_reaches_the_wire() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_AT_REST);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let settled = base + DEFAULT_SETTLE;
        tracker.observe(em.screen(), Some("codexprobe"), &rules, settled);
        assert_eq!(tracker.verdict().agent.as_deref(), Some("codex"));
        assert_eq!(tracker.seq(), 1);

        let hint = &[
            "› Write tests for @filename",
            "  press esc again to interrupt",
        ];
        repaint(&mut em, hint);
        assert_eq!(
            detect(em.screen(), Some("codexprobe"), rules.manifests()),
            Verdict::default(),
            "the premise: with the footer replaced, one frame claims nobody",
        );
        tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            settled + Duration::from_millis(100),
        );

        repaint(&mut em, CODEX_AT_REST);
        tracker.observe(
            em.screen(),
            Some("codexprobe"),
            &rules,
            settled + Duration::from_millis(900),
        );
        assert_eq!(tracker.verdict().agent.as_deref(), Some("codex"));
        assert_eq!(
            tracker.seq(),
            1,
            "the pane never stopped being codex as far as anybody downstream can tell",
        );
    }

    /// The bound on that memory, and the other half of the same measurement: an unclaimed pane
    /// showing nothing active is not asserted to still be anybody. Otherwise the shell that
    /// outlives an agent goes on being reported as the agent.
    #[test]
    fn a_remembered_agent_that_shows_nothing_active_is_let_go() {
        let rules = Ruleset::new(vec![codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(CODEX_AT_REST);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("codexprobe"), &rules, base);
        let settled = base + DEFAULT_SETTLE;
        tracker.observe(em.screen(), Some("codexprobe"), &rules, settled);
        assert_eq!(tracker.verdict().agent.as_deref(), Some("codex"));

        // The agent exits and the pane is a shell again.
        repaint(&mut em, &["coin@box:~$ "]);
        tracker.observe(em.screen(), Some("coin@box: ~"), &rules, settled);
        assert_eq!(
            tracker.verdict().agent.as_deref(),
            Some("codex"),
            "an absence, so it has to hold like any other",
        );
        tracker.observe(
            em.screen(),
            Some("coin@box: ~"),
            &rules,
            settled + DEFAULT_SETTLE,
        );
        assert_eq!(tracker.verdict(), &Verdict::default());

        // And the identity is really gone: the very screen that rode on memory in the test above
        // is nobody's dialog now.
        repaint(&mut em, CODEX_MODAL);
        let verdict = tracker.observe(
            em.screen(),
            Some("coin@box: ~"),
            &rules,
            settled + DEFAULT_SETTLE * 2,
        );
        assert_eq!(
            verdict,
            &Verdict::default(),
            "a dialog in a shell is not the agent that used to live here",
        );
    }

    /// A pane nobody ever claimed publishes nothing at all, so a workspace of ordinary shells keeps
    /// the pre-H3 wire shape — the additive discipline, checked at the only place that can decide
    /// it before slice 3 exists.
    #[test]
    fn a_pane_that_was_never_an_agent_never_publishes() {
        let rules = Ruleset::new(vec![claude(), codex()]);
        let mut tracker = Tracker::default();
        let mut em = painted(&["coin@box:~$ cargo build"]);
        let base = Instant::now();

        tracker.observe(em.screen(), Some("coin@box: ~"), &rules, base);
        repaint(
            &mut em,
            &["coin@box:~$ cargo build", "   Compiling sprag-vt"],
        );
        tracker.observe(
            em.screen(),
            Some("coin@box: ~"),
            &rules,
            base + DEFAULT_SETTLE * 2,
        );

        assert_eq!(tracker.verdict(), &Verdict::default());
        assert_eq!(tracker.seq(), 0, "nothing was ever published");
    }
}
