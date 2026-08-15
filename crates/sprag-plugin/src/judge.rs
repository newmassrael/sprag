//! **A SECOND AGENT, ASKED ONE YES-OR-NO QUESTION ABOUT ONE DIALOG** — what
//! `ai_loop.scxml`'s `cond="_event.data.design"` is decided by.
//!
//! # ⚠⚠⚠ Why quoting could not reach this, measured
//!
//! A [`ScreenRule`](crate::screen::ScreenRule) claims a dialog by quoting it, and R383 established
//! that quoting covers the population it had measured. A later measurement found the case it
//! cannot: these two dialogs, captured verbatim from a live `claude`, ask the identical question.
//!
//! ```text
//! Do you want to create PROBE.txt?     <- writes the word "ready"
//! Do you want to create DESIGN.txt?    <- commits 256 lines that chose JSON over SQLite
//! ```
//!
//! **No needle separates them**, because what differs is what the action MEANS, and that lives in
//! the diff the dialog carries rather than in its wording. A cheap judge told them apart 3 times
//! out of 3, on all four captured dialogs, at 4-6 s per call.
//!
//! # ⚠⚠ Why the verdict is published rather than evaluated in the guard
//!
//! SCXML does not promise how often it evaluates a `cond`, and the pinned engine has no seam to
//! register a host function on ([`IScriptEngine`](sce_rust_runtime) exposes `execute_script`,
//! `evaluate_expression`, `validate_expression` and the variable accessors, and nothing else). So
//! a guard cannot do this and must not try: the driver judges ONCE, when it raises the event, and
//! the document reads `_event.data.design`. The driver measures; the document decides — the same
//! arrangement `_event.data.done` has always used.
//!
//! # ⚠⚠⚠ The failure direction is the safety property
//!
//! No criterion, no judge, a timeout, a crash, a reply that is not a verdict — every one of them
//! answers [`None`], and the caller publishes `false`. **Silence is never a yes.** A `true` on
//! silence would refuse the agent's tool call on nobody's decision, which is the act
//! [`REFUSES`] is structured to keep out of reach.
//!
//! [`REFUSES`]: crate::screen::REFUSES

use std::time::{Duration, Instant};

use sprag_detect::Question;

use crate::access::PaneAccess;
use crate::completion::{Completion, DoneWhen, Over};
use crate::run::RunContext;
use crate::screen::REFUSES;

/// **WHO JUDGES, AND HOW LONG IT MAY TAKE.**
///
/// ⚠ The argv is the CALLER's, not this crate's. Which agent judges — and at what price — is a
/// choice with a real bill attached, made once per run by whoever starts it, exactly as
/// [`Dialogue`](crate::dialogue)'s endpoints are. This crate hard-codes no model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JudgeSpec {
    /// The judge's argv template. The rendered question is APPENDED as the last argument, so a
    /// print-mode CLI reads it positionally and there is no cooked-mode echo to strip.
    pub argv: Vec<String>,
    /// How long one judgement may take before it is abandoned as [`None`].
    ///
    /// ⚠ It sits in the critical path of a BLOCKED turn: the agent is stopped at its dialog and
    /// nothing moves until this returns. Measured at 4-6 s against a cheap model, so a bound of
    /// tens of seconds is patience and not generosity.
    pub within: Duration,
}

impl JudgeSpec {
    /// The wire and datamodel key of the judge's argv.
    pub const ARGV_KEY: &'static str = "judge";
    /// The wire key of the bound.
    pub const WITHIN_KEY: &'static str = "judge_timeout_ms";

    /// The size of the pane a judgement runs in.
    ///
    /// ⚠ Small on purpose and not tuned: nothing reads this pane as a SCREEN. The reply is taken
    /// from the pane's full text once the process has exited, so the only thing a width could do
    /// is wrap a one-word answer.
    const PANE: (u16, u16) = (80, 24);
}

/// **WHAT A JUDGE SAID**, kept whole rather than reduced to the bool, because a run that refused
/// its agent's tool call on this has to be able to show what decided it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgement {
    /// Whether the criterion holds.
    pub holds: bool,
    /// The verdict word actually read — what the reply's FIRST WORD was.
    ///
    /// ⚠ Carried because the reply is not guaranteed to be one word. Measured: the same model
    /// answers some dialogs with a bare verdict and others with a paragraph after it, so a reader
    /// comparing whole replies called a stable verdict unstable. The first word is the verdict and
    /// the rest is commentary — this is the part that was read.
    pub said: String,
    /// How long the agent stood blocked waiting for it.
    pub took: Duration,
}

/// Ask `spec`'s agent whether `criterion` holds of `question`.
///
/// [`None`] when there is no lifecycle to spawn into, the judge could not be started, it did not
/// finish inside [`JudgeSpec::within`], the run ended underneath, or its first word was not a
/// verdict. Every one of those is *this judge said nothing* — see the module doc for why that must
/// not be read as either answer.
#[must_use]
pub fn judges(
    panes: &dyn PaneAccess,
    run: &RunContext,
    criterion: &str,
    question: &Question,
    spec: &JudgeSpec,
) -> Option<Judgement> {
    if criterion.trim().is_empty() || spec.argv.is_empty() {
        return None;
    }
    let life = panes.lifecycle()?;
    let mut argv = spec.argv.clone();
    argv.push(render(criterion, question));

    let began = Instant::now();
    let pane = life
        .spawn(&argv, JudgeSpec::PANE.0, JudgeSpec::PANE.1)
        .ok()?;
    // From here every exit path closes the pane. A judge left running would hold a pty and a
    // process for the rest of the run, once per blocked turn.
    let over = Completion::new(DoneWhen::Exits).wait(panes, pane, spec.within, run);
    let reply = panes.pane_full_text(pane).unwrap_or_default();
    life.close(pane);

    if over != Over::Yes {
        return None;
    }
    let said = reply
        .split_whitespace()
        .next()?
        .trim_matches(|c: char| !c.is_ascii_alphabetic());
    let holds = match said.to_ascii_uppercase().as_str() {
        "YES" => true,
        "NO" => false,
        // ⚠ Anything else is NOT a no. A judge that replied with a sentence, an error, or a
        // refusal has not answered, and the caller must be able to tell that from a measured
        // `false` — a `false` here would look exactly like a judge that considered the dialog and
        // decided against, which is a different fact.
        _ => return None,
    };
    Some(Judgement {
        holds,
        said: said.to_owned(),
        took: began.elapsed(),
    })
}

/// The question put to the judge: the author's criterion, the dialog, its options, and **the one
/// distinction the options alone do not carry**.
///
/// # ⚠⚠⚠ THE OPTIONS ARE BOTH THE SIGNAL AND THE CONFUSION, MEASURED AT BOTH POLES
///
/// ```text
///                        owner's label   with options   without options
/// permission to write            NO       F T T           F F F
/// which files to delete         YES       T T T           F T F
/// ```
///
/// **Neither pole works.** Withholding them removes every false YES and takes the target case with
/// it — `Which report files should I delete?` is two lines, and without its options there is
/// nothing in it that says the outcomes differ. Including them recovers the target and starts
/// reading `1. Yes / 2. Yes, allow all edits / 3. No` as *a set of options on offer*, which is
/// what the criterion asks about.
///
/// So the options stay and the prompt names the difference they cannot show on their own: a menu
/// whose entries are all variations of **yes-or-no about one act** is a permission, however many
/// entries it has. That sentence is doing the work, not the presence of the list.
fn render(criterion: &str, question: &Question) -> String {
    let mut out = String::new();
    out.push_str(
        "An AI agent working in a terminal has stopped and is showing the dialog below.\n\n\
         Decide whether this statement is true of it:\n",
    );
    out.push_str(criterion);
    out.push_str(
        "\n\nOne distinction the options below do not show on their own: when every option is a \
         variation of yes-or-no about ONE action the agent has already settled on — accept it, \
         accept it and stop asking, decline it — that is a permission, however many entries it \
         has. Options that lead to materially different outcomes are a choice.\
         \n\nReply with exactly one word: YES or NO. Do not explain.\n\nThe dialog:\n",
    );
    for line in &question.asked {
        out.push_str(line);
        out.push('\n');
    }
    for choice in &question.choices {
        out.push_str(&format!("  {}. {}\n", choice.number, choice.label));
    }
    out
}

/// What a judged refusal did, beside [`Screened`](crate::screen::Screened) and for its reason: a
/// decision taken on somebody's behalf has to be reportable in the run's own vocabulary.
///
/// ⚠ It carries the CRITERION where `Screened` carries a rule's quote. Neither is the dialog — both
/// are the author's own words for why this dialog was theirs — and a reader auditing a run needs to
/// know which of the two authorities acted, because only one of them is re-readable in the
/// document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Redirected {
    /// The question as it stood when it was turned down.
    pub question: Question,
    /// The criterion the judge was asked about.
    pub criterion: String,
    /// The verdict word the judge replied with.
    pub said: String,
    /// What the agent was told to do instead.
    pub told: String,
    /// PTY bytes the whole act cost: [`REFUSES`] and the redirect together.
    pub bytes: u64,
}

impl Redirected {
    /// ONE LINE for the run's journal.
    ///
    /// ⚠ It says **refused** out loud, `Screened::describe`'s reason: the act's name describes the
    /// judging and not the consequence, and the consequence — a tool call the agent asked for did
    /// not happen — is the part a person auditing this run needs.
    #[must_use]
    pub fn describe(&self) -> String {
        format!(
            "judged {:?} ({}), refused with {REFUSES} and told the agent: {:?}",
            self.criterion, self.said, self.told,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::{KeyStroke, PaneError, PaneRow, Written};
    use sprag_detect::Choice;
    use sprag_terminal::PaneId;

    /// A host with panes but NO LIFECYCLE — so a judgement that tried to spawn would answer `None`
    /// for that reason and not for the one under test.
    ///
    /// ⚠ `pane_full_text` panics rather than answering, because reaching it means a judge was
    /// spawned and waited on. The tests below are about the paths that must ask NOBODY, so being
    /// reached at all is the failure.
    struct AsksNobody;

    impl PaneAccess for AsksNobody {
        fn pane_ids(&self) -> Vec<PaneId> {
            Vec::new()
        }
        fn pane_collapsed(&self, _id: PaneId) -> Option<String> {
            None
        }
        fn pane_rows(&self, _id: PaneId) -> Option<Vec<PaneRow>> {
            None
        }
        fn pane_eof(&self, _id: PaneId) -> Option<bool> {
            None
        }
        fn pane_full_text(&self, _id: PaneId) -> Option<String> {
            unreachable!("a judgement that asks nobody must not read a judge's reply")
        }
        fn inject(&self, _id: PaneId, _keys: &[KeyStroke]) -> Result<Written, PaneError> {
            Err(PaneError::UnknownPane(PaneId(0)))
        }
    }

    fn question() -> Question {
        Question {
            asked: vec!["Do you want to create DESIGN.txt?".to_owned()],
            choices: vec![
                Choice {
                    number: 1,
                    label: "Yes".to_owned(),
                    selected: true,
                },
                Choice {
                    number: 3,
                    label: "No".to_owned(),
                    selected: false,
                },
            ],
        }
    }

    /// The judge is shown the author's words, the dialog, its options, **and the distinction the
    /// options cannot carry**.
    ///
    /// ⚠⚠⚠ That last clause is the assertion worth having. Measured at both poles, the options
    /// alone recover the target case and cost false YES on permissions; the sentence about
    /// yes-or-no menus is what separates them. A change that dropped it would pass every other
    /// test in this module and quietly return the judge to a 2-in-3 false YES rate — see
    /// [`render`].
    #[test]
    fn the_question_carries_the_criterion_the_dialog_its_options_and_the_distinction() {
        let put = render("going ahead would commit a design decision", &question());
        assert!(put.contains("going ahead would commit a design decision"));
        assert!(put.contains("Do you want to create DESIGN.txt?"));
        assert!(
            put.contains("YES or NO"),
            "the verdict vocabulary is stated: {put}"
        );
        assert!(put.contains("1. Yes"), "the options are the signal: {put}");
        assert!(put.contains("3. No"), "{put}");
        assert!(
            put.contains("variation of yes-or-no"),
            "and the one thing the options cannot say about themselves: {put}",
        );
    }

    /// **AN EMPTY CRITERION IS HOW A RUN DECLINES A JUDGE**, and it must cost nothing.
    ///
    /// ⚠ Asserted through a `PaneAccess` with NO lifecycle, so a judge that tried to spawn would
    /// panic on the `expect` rather than quietly returning `None` for the wrong reason — the check
    /// is that nothing was reached, not merely that the answer was `None`.
    #[test]
    fn an_empty_criterion_asks_nobody() {
        let spec = JudgeSpec {
            argv: vec!["claude".to_owned(), "-p".to_owned()],
            within: Duration::from_secs(30),
        };
        for criterion in ["", "   ", "\n"] {
            assert_eq!(
                judges(
                    &AsksNobody,
                    &RunContext::uncancellable(),
                    criterion,
                    &question(),
                    &spec
                ),
                None,
                "{criterion:?} declines the judge entirely",
            );
        }
    }

    /// An empty argv is the other way a run has no judge: nobody to ask.
    #[test]
    fn a_judge_with_no_command_asks_nobody() {
        assert_eq!(
            judges(
                &AsksNobody,
                &RunContext::uncancellable(),
                "anything at all",
                &question(),
                &JudgeSpec {
                    argv: Vec::new(),
                    within: Duration::from_secs(30)
                },
            ),
            None,
        );
    }

    /// The line a run's journal carries names the criterion, the verdict and the redirect.
    #[test]
    fn a_redirect_describes_what_it_refused_and_what_it_said() {
        let line = Redirected {
            question: question(),
            criterion: "commits a design decision".to_owned(),
            said: "YES".to_owned(),
            told: "Reconsider and take the long-term-correct approach.".to_owned(),
            bytes: 61,
        }
        .describe();
        assert!(line.contains("commits a design decision"), "{line}");
        assert!(line.contains("YES"), "{line}");
        assert!(
            line.contains("Escape"),
            "the key that refused is named: {line}"
        );
        assert!(line.contains("Reconsider"), "{line}");
    }
}
