//! The plugin-host control surface — start and observe plugin runs over RPC.
//!
//! `PluginsExternal` is the seam where the pinion-aware host drives the
//! pinion-free plugin substrate ([`sprag_plugin`]). An external AI peer:
//!
//! * `invoke("run", {plugin, …args, guardrails?})` → starts a plugin on a
//!   background thread (so the long, blocking `Driver::run` never freezes the
//!   serve loop) and gets a run id back immediately;
//! * `query("runs")` → observes each run's terminal `Outcome` as scene-as-data;
//! * `query("plugins")` → the available plugin set.
//!
//! Runs are guardrail-bounded by construction: a `run` always gets the default
//! iteration ceiling (the liveness floor), the default WALL-CLOCK deadline, and
//! the plugin's default cost ceiling in its unit — never unbounded, on any of the
//! three axes, because loop safety is first-class. (A print-mode Text dialogue
//! accumulates `Tokens(0)`, so its cost ceiling never binds and the other two are
//! its effective bounds.) Whichever binds first ends the run, and the outcome says
//! WHICH.
//! Target panes are validated at submit time, so a typo is a synchronous
//! `Rejected`, not an async `Failed`.
//!
//! # ⚠⚠⚠ The `ai_loop` form is the door register item 65 had been holding open
//!
//! Five rounds built the outer AI loop's statechart, its driver and its measurement against a live
//! `claude`, and at the end of them **nothing in the daemon constructed one and no surface started
//! one**. It is a plugin like the others now, which is what gives it everything above for free —
//! a run id, the three guardrails, a cancel flag, a journal and a durable record — and what makes
//! `sce-rust-lua` a real dependency of this crate: the loop's document has a script datamodel, so
//! starting one means building an interpreter for it HERE. That trade is written out in the
//! manifest beside the dependency.
//!
//! ⚠ Its own budget is NOT a guardrail. `max_turns` counts the inner agent's turns and one of
//! those is many steps of the loop driving it, so it travels in the brief and a run stopped by it
//! reports the ceiling `turns` — a word whose remedy is in the request rather than in `guardrails`.

use std::fmt;
use std::io;
use std::process::Child;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError,
    ReadRefusal, SchemaField,
};
use serde_json::{Map, Value, json};
use sprag_plugin::{
    Agent, AgentSpec, Attended, Brief, Ceiling, Consent, Consents, Cost, Dialogue, DialogueSpec,
    DoneWhen, Driver, Guardrails, Handback, OrchestrationSpec, Orchestrator, Outcome, OutcomeState,
    Pipe, PipeSpec, Plugin, Readiness, ReadyWhen, ReplyFormat, RunContext, ScreenRule, ScreenRules,
    Turn, WorkspacePaneAccess,
};
use sprag_terminal::{PaneId, Workspace};

use crate::external::{
    as_object, declined, lock, opt_dim, opt_str, refused, require_pane_id, require_str,
    rpc_external_impl,
};
use crate::runs::{RunId, RunRegistry, RunState, RunSummary};

/// The plugin-host external's action that STARTS a run.
pub const RUN_ACTION: &str = "run";
/// The plugin-host external's action that raises a run's cancel flag.
pub const CANCEL_ACTION: &str = "cancel";
/// The plugin-host external's action that asks a run to finish its milestone and then stop.
///
/// ⚠⚠⚠ A SECOND VERB RATHER THAN A MODE ON [`CANCEL_ACTION`], because the outcomes are opposite: a
/// cancel loses the turn in flight and this one banks it. ⚠ ADDING AN ACTION IS ADDITIVE — an older
/// client simply cannot reach it — so this does not earn a `WIRE_PROTOCOL` bump. The residue,
/// stated: a client newer than its daemon gets `UnknownPath` for it, which is the daemon saying it
/// does not serve that address, and is the answer that case should get.
pub const STAND_DOWN_ACTION: &str = "stand_down";
/// **HALT A RUN BETWEEN TURNS, OR LET IT GO** — the third thing a person may say to a run, and the
/// only one they can take back (register item 9).
///
/// Its two neighbours both END the run and differ in what that costs: `cancel` loses the turn in
/// flight, `stand_down` banks the milestone. Neither is *wait, let me read this* — and
/// `ai_loop.scxml` has carried the edge for it (`hold` → `awaiting_human`) since R378 with nothing
/// in the product able to raise it.
///
/// ⚠ ADDITIVE, on [`STAND_DOWN_ACTION`]'s own terms and for its reason: an older client cannot
/// reach an address it does not know, so this earns no `WIRE_PROTOCOL` bump. A client newer than its
/// daemon gets `UnknownPath`, which is the daemon saying it does not serve that address.
pub const HOLD_RUN_ACTION: &str = "hold_run";
/// **A RUN'S OWN DRIVER SAYING WHAT IT HAS DONE SO FAR** — register item 650, and the only action
/// here a PERSON never calls.
///
/// # ⚠⚠⚠⚠⚠ Why a run driven elsewhere needs an address at all
///
/// The four actions above are things somebody says TO a run. This is the run talking BACK, and it
/// exists because `run_to_json` reads a running run's counters out of a
/// [`sprag_plugin::ProgressCell`] — shared memory, which a driver in another process does not
/// share. Without it such a run's row sits at zero for its whole life while the work really happens,
/// which is the difference between a supervised loop and a black box.
///
/// ⚠⚠ **PUSHED, NEVER POLLED.** The driver sends this when its own step count moves; nothing here
/// asks it on a clock. Register items 629, 630, 631 and 640 spent four rounds taking exactly that
/// off the pane axis (*a remote wait that cost 181 reads over two seconds now costs 1*), and a
/// daemon that sampled its drivers would rebuild it one axis over.
///
/// ⚠ ADDITIVE, on [`STAND_DOWN_ACTION`]'s terms and for its reason — an older client cannot reach
/// an address it does not know, so this earns no `WIRE_PROTOCOL` bump.
pub const REPORT_PROGRESS_ACTION: &str = "report_progress";
/// The slot reporting every run this daemon holds.
pub const RUNS_SLOT: &str = "runs";
/// The slot listing the plugins a `run` may name.
pub const PLUGINS_SLOT: &str = "plugins";
/// The slot publishing the guardrail bound a `run` that names none is given.
///
/// # Why a number a client could compile in is served over the wire
///
/// [`DEFAULT_MAX_ITERATIONS`] and its two siblings are this DAEMON's policy, and a client is not
/// necessarily this daemon's build — the whole argument `show-grammar` makes about the request
/// grammar, applied to the one fact a client needs in order to bound a loop it did not choose the
/// bounds for. The agent-facing mouth turns these into its CEILING (an agent may tighten a bound
/// and not loosen it), and a ceiling read from a constant compiled six weeks ago would be a
/// different ceiling from the one the daemon enforces.
pub const GUARDRAIL_DEFAULTS_SLOT: &str = "guardrail_defaults";

/// The REQUEST key carrying the consent LIST — [`Consents::WIRE_KEY`], re-exported.
///
/// # ⚠⚠ Why a projection rather than a literal at each mouth
///
/// The mouths that build a `run` call are not all able to depend on `sprag-plugin`: `sprag-mcp`
/// carries it as a DEV-dependency only, and a tool schema written from a literal `"asked"` would be
/// a second definition of a name whose whole job is to be the same word on both sides of the wire.
/// These three are `const` projections of the type that owns them, so a rename there is a compile
/// error here rather than a mouth that quietly stops matching.
///
/// ⚠ They are the REQUEST-side names and are deliberately separate from [`RUN_ASKED_KEY`], which
/// spells the same word about a different thing: that one is the QUESTION'S OWN LINES in an answer,
/// this one is the NEEDLE a caller sends. One is what the pane said; the other is what the caller
/// will accept. Merging them because they read alike is how two concepts come to move together.
pub const CONSENT_KEY: &str = Consents::WIRE_KEY;
/// The [`CONSENT_KEY`] ELEMENT'S needle naming WHICH QUESTION — [`Consent::ASKED_KEY`],
/// re-exported. ⚠ It lives inside one member of the list, not beside it.
pub const CONSENT_ASKED_KEY: &str = Consent::ASKED_KEY;
/// The [`CONSENT_KEY`] ELEMENT'S needle naming WHICH OPTION — [`Consent::ANSWER_KEY`],
/// re-exported.
pub const CONSENT_ANSWER_KEY: &str = Consent::ANSWER_KEY;

/// The answer key naming a run.
const RUN_ID_KEY: &str = "id";
/// The answer key carrying the pane whose occupant asked for a run — absent for a run nobody
/// claims, on [`sprag_terminal::Pane::opened_by`]'s terms.
const RUN_OPENED_BY_KEY: &str = "opened_by";
/// The answer key naming WHICH BUILD DROVE a run — absent when nothing recorded one.
///
/// # ⚠⚠⚠⚠⚠ What it is for: a walk is evidence about the daemon's build, not about the tree
///
/// A daemon outlives its clients, so the ordinary state after a day's work is a daemon running code
/// the tree has already replaced. Every other column here describes what a run DID; this one says
/// which code did it, and without it a run driven by a daemon that predates a fix reads exactly
/// like one that carries it (register item 438, measured 2026-08-18 at the cost of a round).
///
/// ⚠⚠ **Absent is not "this build".** It is a run restored from a log written before the field
/// existed — see [`crate::runs::RunSummary::build`]. Filling it in with the reader's own build
/// would date a dead daemon's work to its successor.
///
/// ⚠ An added ANSWER key earns no `WIRE_PROTOCOL` bump (that constant's own rule at version 5:
/// absent-not-wrong to an old reader), and no pin covers a slot's answer shape.
pub const RUN_BUILD_KEY: &str = "build";
/// The answer key naming WHICH GUARDRAIL exhausted a run — absent unless one did.
///
/// Its vocabulary is [`sprag_plugin::Ceiling`]'s own words, so the host never spells a variant and
/// a fourth ceiling reaches the wire by being added to that type.
pub const RUN_CEILING_KEY: &str = "ceiling";
/// The outcome key carrying WHAT THE PEER IS ASKING, present only on a run that ended `blocked`
/// and only where this host could read the question — see [`outcome_question`].
/// ⚠ ONE STRING, shared with the pane-level surface ([`crate::wire::ASKING_KEY`]) since R367: the
/// same question is published in both places and a caller moves between them.
pub const RUN_ASKING_KEY: &str = crate::wire::ASKING_KEY;
/// The [`RUN_ASKING_KEY`] member holding the question's own lines, in reading order.
pub const RUN_ASKED_KEY: &str = crate::wire::ASKED_KEY;
/// The [`RUN_ASKING_KEY`] member holding the options, in screen order — each `{number, label,
/// selected}`.
///
/// ⚠ `selected` is where a bare Enter would land, which is the difference between confirming a
/// tool call and declining it. Carried rather than left for a caller to infer.
pub const RUN_CHOICES_KEY: &str = crate::wire::CHOICES_KEY;
/// The [`RUN_ASKING_KEY`] member saying WHY the run did not answer, from
/// [`sprag_plugin::Refusal`]'s own words.
///
/// # ⚠⚠ Present on EVERY blocked run, including the ones with no question
///
/// It is the only member of `asking` that is never absent, and that is deliberate: a run that was
/// GIVEN a consent and stopped anyway looks identical to one that was given none, and the two have
/// completely different remedies — fix a needle, or write a consent. `unreadable` also carries the
/// case that has no question at all (`sprag_plugin::Unanswered::unreadable`), which was published
/// as an absence and explained nowhere until R366.
pub const RUN_WHY_KEY: &str = "why";
/// The outcome key counting HOW MANY of its peer's questions a run answered on the caller's
/// consent — always present, `0` for the runs that answered none.
///
/// # ⚠⚠ Why this is not absence-is-the-claim like its neighbours
///
/// [`RUN_CEILING_KEY`] and [`RUN_STOPPED_KEY`] are absent when they have nothing to say, because
/// their absence is *nothing of this kind happened* and a reader loses nothing. Here the absence
/// would be the same sentence a `0` is — and this is a count of DECISIONS TAKEN ON SOMEBODY'S
/// BEHALF, so *"this run answered nothing"* is a claim a reader must be able to get affirmatively
/// rather than by not finding a key.
pub const RUN_ANSWERED_KEY: &str = "answered";
/// **WHAT A RUN KEPT**, on an ending — `{"completed": N, "unit": "turn"}`, or ABSENT for a plugin
/// that counts no completed work at all.
///
/// ⚠⚠⚠ The absence is a claim and not a gap, which is why this follows [`RUN_CEILING_KEY`]'s
/// omit-rather-than-null rule: *this plugin does not count* and *it counts and there was none* are
/// two different things to tell somebody who just stood a run down, and
/// `crate::plugins::stand_down_sentence` says two different sentences for them.
pub const RUN_BANKED_KEY: &str = "banked";
/// ⛔⛔⛔⛔⛔ **HOW BIG THE BRIEF A LOOP WAS STARTED WITH IS** — register item 719's second
/// direction. A SENTENCE on the row (composed by [`briefing_sentence`]) and the three byte counts
/// under [`REPORTED_BESIDE_KEY`]; ABSENT for a run nobody briefs.
///
/// # The door took any size and nothing ever said so
///
/// A loop's brief becomes the prompt every session it opens is greeted with, and is re-typed in
/// full into every replacement. Item 719 measured one at **9,025 bytes** being refused by a peer's
/// composer and retyped every turn, unbounded — and the caller who wrote it had no channel that
/// would have told them it was large. `orchestrate` answers a run id, and until this key the row it
/// points at said nothing about what had been accepted.
///
/// ⚠⚠⚠ **IT REPORTS RATHER THAN REFUSING, AND THAT IS MEASURED RATHER THAN CAUTIOUS.** Briefs at
/// 9,025, 8,271, 4,532 **and 2,816** bytes were all folded away by a composer, so folding is not a
/// size line and any threshold would be invented. See [`sprag_plugin::Briefing`].
///
/// ⚠ It follows [`RUN_BANKED_KEY`]'s omit-rather-than-null rule for that key's reason: *no loop was
/// briefed here* is a fact about the plugin, not a gap in the row.
pub const RUN_BRIEFED_KEY: &str = "briefed";
/// ⛔⛔⛔⛔⛔ **WHY AN INTERRUPTED RUN IS NOT COMING BACK** — register item 737. A SENTENCE on the
/// row (composed by [`withheld_sentence`]), and ABSENT for every run that is not one a boot read out
/// of a predecessor's log and declined to put back.
///
/// # The one thing `interrupted` could not say is whether anybody would pick it up
///
/// A restored run's row reads `interrupted`, and that word covers two opposite futures: *waiting
/// for a daemon that will put it back* and *no daemon ever will, because the documents it recorded
/// its position against are not this build's*. The second is what a PROMOTION causes — the reason
/// to promote is usually a changed `.scxml`, and `crate::runs::PersistedRun::resumable_place`
/// compares that fingerprint for equality — so the common case was the unsayable one.
///
/// ⚠⚠ It is the same shape as [`RUN_STOOD_DOWN_KEY`] and [`RUN_CHECKS_KEY`]: a fact one layer knows
/// and every reader above it needed, published as a sentence because what a person does about it is
/// a comparison (*these documents against those*) rather than a value.
///
/// ⚠ No [`sprag_rpc::WIRE_PROTOCOL`] bump, on [`RUN_STOOD_DOWN_KEY`]'s argument unchanged: an added
/// answer key withdraws no address and widens no value space a peer decodes whole.
pub const RUN_WITHHELD_KEY: &str = "withheld";
/// The key a driver's [`REPORT_PROGRESS_ACTION`] carries its counters under.
///
/// ⚠⚠ **THE WHOLE OBJECT UNDER ONE KEY, not its fields spread across the request.** What is inside
/// is [`progress_to_json`]'s output, and this daemon stores it WITHOUT reading it apart — so a key
/// added to that renderer reaches the row with nothing here to update. Spread flat, each new key
/// would need a line here too, and the day somebody forgot one it would go missing for
/// out-of-process runs alone.
pub const PROGRESS_KEY: &str = "progress";
/// The answer key carrying WHAT EACH STEP DID — the last [`sprag_plugin::JOURNAL_LIMIT`] of them.
///
/// A run reported its total and its terminal state and nothing about the steps between, so a loop
/// that failed to converge could not be diagnosed at all. ⚠ Compare its length against
/// `iterations` to tell a truncated journal from a complete one.
pub const RUN_JOURNAL_KEY: &str = "journal";
/// The answer key carrying WHAT BECAME OF THE WORK a run had going — absent unless the run was CUT
/// SHORT (cancelled, or out of time), which are the only endings that can land while a step is
/// still blocked on a peer this run set going.
///
/// ⚠⚠ Its presence is itself the claim, the rule [`RUN_CEILING_KEY`] follows. A `cancelled` outcome
/// with no answer here is consistent with two opposite states of the world — the work stopped, or
/// the work is still running and still spending — and the one a caller must act on is the second.
/// Its text is [`sprag_plugin::Stopped`]'s own sentence, so the host never spells a variant.
pub const RUN_STOPPED_KEY: &str = "stopped";
/// The answer key carrying **HOW MANY PROMPTS A RUN HAS PUT INTO ITS PANE** — register item 591,
/// and the denominator without which the key beside it is a number with no scale.
///
/// ⚠ Present whenever a run has delivered anything, and ABSENT for a run that has delivered
/// nothing — which is every run of the three bundled plugins that compose no prompt. That absence
/// is a claim: *this run has no prompts for a composer to fold*, which is a different fact from
/// *this run's prompts are all visible*.
pub const RUN_DELIVERED_KEY: &str = "delivered";
/// The answer key carrying **HOW MANY OF A RUN'S PROMPTS ARE NOWHERE ON ITS PANE** — register item
/// 591, present beside [`RUN_DELIVERED_KEY`] and never alone.
///
/// # ⛔⛔⛔ The fact that was only ever published as a CHANGE
///
/// `ai_loop` already says which road a delivery took, and says it well — but it says it as a diff:
/// `sprag_plugin`'s `Told` publishes the evidence once and again only when the road MOVES, so
/// *"the prompt is NOWHERE ON THAT SCREEN — its composer folded the paste away"* appears in a walk
/// at the moment the road changed and nowhere after. **A supervisor who arrives mid-run, or who
/// reads a finished run's totals, could not ask whether that was true.** Measured 2026-08-22: a
/// live loop carried that sentence on every one of its reflections, and delivery confirmation is
/// the axis this project has spent the most rounds on.
///
/// ⚠⚠ A run whose `folded` equals its [`RUN_DELIVERED_KEY`] is one where *go and look at the pane*
/// is the wrong instruction — `sprag_plugin::Deliveries::all_folded` is the predicate, and both
/// mouths say it in words rather than leaving a reader to divide two numbers.
pub const RUN_FOLDED_KEY: &str = "folded";
/// The answer key carrying **HOW MANY OF A RUN'S PROMPTS ARE SITTING IN A COMPOSER, TYPED AND NEVER
/// ASKED** — register item 617, present beside [`RUN_DELIVERED_KEY`] and never alone.
///
/// # ⛔⛔⛔ It was `0`, which is what a run that sent nothing publishes
///
/// A prompt the pane took and painted, whose submit never became a question, is not a delivery —
/// nothing was asked — so it is rightly outside [`RUN_DELIVERED_KEY`]. But it was outside
/// everything: `sprag_plugin::deliver::Witnessed::of` maps both refusals to `None`, so the wedged
/// run and the run that never typed a byte published the same `0 of 0`, and [`delivery_sentence`]
/// (which returns nothing on a zero denominator) printed no delivery line for either.
///
/// ⚠⚠ **THE REMEDY IS THE OPPOSITE OF [`RUN_FOLDED_KEY`]'s.** A folded prompt means *do not go and
/// look at that pane*; this one means **go and look — it is sitting there**. Two instructions to
/// two different people, and one number until this key existed.
///
/// ⚠ MEASURED 2026-08-23 against a live `claude` on a 44-column pane: the run's own failure
/// sentence said *the text was read back off a screen this delivery changed* while its counters
/// said `made: 0, folded: 0`.
pub const RUN_UNSUBMITTED_KEY: &str = "unsubmitted";
/// The answer key carrying **WHETHER ANYTHING INDEPENDENT VERIFIED THIS RUN'S MILESTONES** —
/// register item 601, a sentence and absent for a run that put no claim to a checker.
///
/// # ⛔⛔⛔ *Checked* and *the checker never started* were the same `converged`
///
/// Register item 428 built the independent check because a milestone certified by the agent that
/// did the work is not certified. Register item 593 then made a silent check say WHICH silence.
/// Neither reached the run's own answer: it says `converged` whether a separate process agreed or
/// whether the checker would not start, and those are opposite facts about what the ending is
/// worth. Measured 2026-08-22 — a run converged carrying *"Silence is not agreement: fix the
/// checker, or the milestone is resting on the working agent's own word"*, in a walk, on a line no
/// mouth prints.
///
/// ⚠⚠ **THE THIRD KEY OF THIS SHAPE**, after [`RUN_STOOD_DOWN_KEY`] and [`RUN_FOLDED_KEY`], and the
/// tendency is worth naming: a fact the driver knows flows into the walk — a bounded, unpersisted
/// stream of CHANGES — and reaches the answer only if somebody carries it. See
/// `sprag_plugin::Checks`.
pub const RUN_CHECKS_KEY: &str = "checks";
/// The answer key carrying **WHICH PANE THIS RUN IS DRIVING RIGHT NOW** — register item 540, and
/// absent for a run that has taken no step or drives no pane of its own.
///
/// # ⛔⛔⛔ It was published all along, as prose inside a name
///
/// A run's `label` reads `ai_loop pane=3`, so every reader that wanted the pane had to parse a
/// human sentence — R431's *derive it from a name*, one surface over — against a string this
/// repository is free to reword. Nothing structured said it.
///
/// ⚠⚠ **AND IT IS HALF OF A QUESTION THAT NEEDED BOTH HALVES** — register item 595. A daemon
/// restart re-runs an allowlisted agent's argv, so a `claude` pane comes back holding its old
/// conversation with **nothing driving it**, and a person cannot tell that from a working loop:
/// both are a `claude` prompt. This key is what lets the PANE surface answer *is anybody driving
/// me* — see `crate::wire`'s pane answer.
pub const RUN_DRIVING_KEY: &str = "driving";
/// **WHERE A RUN'S MACHINE WAS, WRITTEN INTO THE REQUEST A DAEMON HANDS A DRIVER** — register item
/// 543's fourth brick, and the one key on that map a CLIENT may not set.
///
/// # ⚠⚠⚠⚠⚠ Why the daemon strips it rather than the grammar refusing it
///
/// This wire swallows an argument it does not publish ([`crate::wire`]'s own rule), so a key added
/// here would be honoured off a client's `run` call with nothing to say so — *start this loop
/// already at `judging`* as an unpublished verb. Publishing it instead would be worse: where a run
/// resumes is not a thing a caller knows. It is a fact this daemon reads out of the run log it
/// inherited, checked against [`sprag_plugin::STATECHARTS_FINGERPRINT`], and one authority on one
/// fact is the property being defended.
///
/// So the daemon's own `spawn_driven_run` REMOVES whatever arrived under this name, which makes a
/// client's copy inert without the grammar having to grow a word — and leaves
/// [`sprag_rpc::WIRE_PROTOCOL`] where it is, because nothing a client can say or read changed.
///
/// ⚠⚠⚠ **AND NOTHING IN THIS DAEMON WRITES IT YET, which is said here rather than left to be
/// discovered** — but the reason changed on the round that built the boot, so it is restated rather
/// than left to age. It is no longer *a resume would hand its peer an empty prompt*: that was item
/// 543's fifth brick and it is paid (`sprag_plugin`'s
/// `a_resumed_loop_composes_a_real_prompt_and_not_an_empty_one`, green). A boot DOES put inherited
/// runs back now — [`PluginsExternal::put_back`] — and it does so **in this process**, by calling
/// `sprag_plugin::Plugin::resume_at` on a plugin it holds, so nothing has to travel as a key.
///
/// ⚠⚠⚠⚠ **WHAT WOULD WRITE IT IS A BOOT RESUMING AN OUT-OF-PROCESS RUN, AND THAT CANNOT HAPPEN
/// YET — measured.** [`progress_to_json`] carries neither `at` nor `place`, so a run whose driver is
/// another process reports its counters and never its position; its row publishes them from a cell
/// that never moves, and `crate::runs::RunRegistry::persistable` therefore writes such a run's log
/// with **no place in it at all**. There is consequently nothing for a boot to put back for exactly
/// the runs whose driver reads this key, and `put_back` refuses on such a daemon rather than
/// quietly driving one somewhere its own daemon says drivers do not live.
///
/// ⚠⚠ It is the DRIVER's side that reads it ([`drive_request`]), because that is where the plugin
/// is built. An empty list is refused rather than treated as *no place*, on
/// `crate::runs::PersistedRun::resumable_place`'s exact rule.
///
/// # ⚠⚠⚠⚠ ONE WORD, TWO DIRECTIONS — register item 662
///
/// The same key is what a driver REPORTS ([`progress_to_json`]) as well as what a request may carry
/// down to one. That is deliberate and not an overload: it is the same fact — *where this run's
/// machine is* — and one word for one fact is what keeps the two ends readable together. The maps
/// are different objects (a run request, a progress report) so nothing can confuse them, and the
/// asymmetry of trust is the whole design: **a client may not say it and a driver may.**
pub const RUN_PLACE_KEY: &str = "place";
/// **WHAT A DRIVER REPORTS FOR THE FACTS A ROW PUBLISHES BESIDE ITS STATE** — one key holding one
/// object, register item 663.
///
/// # ⚠⚠⚠⚠⚠ Why these travel NESTED when `at` and `place` travel flat
///
/// A progress report becomes the `state` object of a row ([`progress_to_json`] renders it and the
/// row republishes it whole), so a key added flat is a key published INSIDE the state. That was
/// harmless for `at` and `place` — nothing else renders them — and it is not harmless for these
/// five: `run_to_json` already publishes the delivery triple, the checks sentence and the driven
/// pane BESIDE the state. Flat, they would appear twice in one row, and only for a run whose driver
/// is another process — one number with two spellings (register item 445) and the invisible
/// divergence [`crate::options::RUN_DRIVER_PROCESS`] promises cannot happen.
///
/// ⚠⚠ **SO THE REPORT IS A TRANSPORT, AND THE ROW LIFTS THIS OUT OF IT** before the rest becomes
/// `state`. One key, one removal — a structural strip rather than a LIST of names somebody must
/// remember to extend, which is the shape that rots the first time a sixth fact arrives.
///
/// ⚠ Inside it, `checks` is the TALLY (`asked` / `silent` / `why_silent`); beside the state it is
/// the SENTENCE `checks_sentence` composes from that tally. The two can never be confused because
/// one of them is in here.
pub const REPORTED_BESIDE_KEY: &str = "beside";
/// **WHERE A RUN'S MACHINE IS, IN ONE WORD FOR A PERSON** — `sprag_plugin::Plugin::at`, as a
/// driver's progress report carries it. Register item 662.
///
/// ⚠⚠ **THE PAIR WITH [`RUN_PLACE_KEY`], AND THEY TRAVEL THE SAME WAY FOR OPPOSITE READERS**: this
/// one answers *was my run mid-turn, or waiting on me?* and a machine cannot be put back with it;
/// that one is the whole configuration an engine re-enters at. `crate::runs::PersistedRun` keeps
/// both columns for exactly that reason, and before item 662 neither could reach it from a driver
/// in another process.
pub const RUN_AT_KEY: &str = "at";
/// **WHICH PANE A RUN DRIVES**, as every pane-driving form on this surface names it.
///
/// ⚠⚠ A constant so the one reader that is NOT a parse — [`pane_named`], which a boot uses to find
/// the pool a pane is sitting in — cannot drift from the parse. The wire grammar spells the word
/// too and is a separate authority on purpose (it publishes what a caller may say); what this ties
/// together is the two places in THIS file that read it.
pub const RUN_PANE_KEY: &str = "pane";
/// The answer key carrying **WHAT BECAME OF A PERSON'S STAND-DOWN ORDER** — absent unless somebody
/// gave one, the rule [`RUN_CEILING_KEY`] follows.
///
/// # ⛔⛔⛔ Register item 594 — the promise had no surface to be kept on
///
/// `sprag stand-down` tells a person *"its work is kept — `sprag runs` says when it has"*, and
/// **`sprag runs` published nothing whatsoever about the order.** The order was a host-side flag
/// (`sprag_plugin::RunContext::stood_down`) that only the loop document ever read, and the word it
/// closes under (`DoneReason::StoodDown`) reaches a walk and no wire key at all. So a stood-down
/// run printed `converged` — byte-identical to a run that finished on its own — and a stood-down
/// run that was killed first printed `cancelled`, which reads as *the work was thrown away* with
/// nothing beside it to say an order had been standing.
///
/// Measured 2026-08-22: `sprag stand-down 1` was answered *"its work is kept"* and the run was
/// later reported `cancelled after 56 iterations, 23146 bytes`. **The worst shape a failure can
/// take is one that makes somebody believe work was banked and then discards it**, and no reading
/// of that line could tell the two apart.
///
/// ⚠⚠ **THE VALUE IS A SENTENCE, NOT A BOOLEAN**, for [`RUN_STOPPED_KEY`]'s reason one level up: the
/// fact a reader needs is not *was an order given* but *did it land*, and that is the ORDER weighed
/// against the ENDING. [`stand_down_sentence`] is the one place the two are put side by side.
///
/// # Why this earns no [`sprag_rpc::WIRE_PROTOCOL`] bump
///
/// Written down because NOT bumping is a judgement too — `run_to_json`'s rule for `opened_by`.
///
/// It is an ADDED ANSWER KEY, which that constant's own doc settles as *absent-not-wrong* to an
/// older reader: no request argument moved, no address was withdrawn, and no value space a peer
/// decodes whole was widened. The one condition that would overturn it is a reader treating the
/// absence as agreement — and on this wire a client and a daemon that are talking at all hold the
/// SAME protocol number, because the handshake refuses a mismatch outright. So there is no *new
/// client, old daemon* pair for whom a missing key could mean *this daemon cannot say*: to every
/// reader that got an answer, absence means what it says, **nobody ordered this run to stand down**.
///
/// ⚠ The FILE is a separate question with a separate answer, and there absence really is *cannot
/// say* — see `crate::runs::PersistedRun::stood_down`, which is `Option<bool>` for exactly that
/// reason and does not move `crate::runs::RUN_LOG_VERSION` either.
pub const RUN_STOOD_DOWN_KEY: &str = "stood_down";

/// **WHAT A PERSON HAS ORDERED, AS DATA A MACHINE READS** — register item 699, and the key
/// [`StandingOrders`] travels under.
///
/// ⚠⚠⚠ It sits BESIDE [`RUN_STOOD_DOWN_KEY`] rather than replacing it, and the split is the whole
/// repair: that key carries a SENTENCE for a person, this one carries the ORDER for the driver.
/// One key cannot be both, which is exactly what was measured — see [`StandingOrders`].
pub const RUN_ORDERS_KEY: &str = "orders";

/// **THE ORDERS A PERSON HAS GIVEN A RUNNING RUN**, in one type both processes use — register
/// item 699.
///
/// # ⚠⚠⚠⚠⚠ Neither order had ever landed, and the two failed differently
///
/// Measured 2026-08-26 across four repositories: `stand-down` was given nine times and converged a
/// run zero times, and `hold-run` had never parked anything. A run is driven by
/// `sprag-term --drive` — ANOTHER PROCESS — which learns a person's order by re-reading this row,
/// and both readings were wrong in ways nothing could see:
///
/// * **`stand_down` — a type mismatch.** The daemon published
///   `json!(stand_down_sentence(&run.state))`, a STRING for a person to read, and the driver asked
///   `row[RUN_STOOD_DOWN_KEY].as_bool()`. `as_bool` on a string is `None`, so the comparison was
///   `None == Some(true)` — **false on every pass, for every run, for ever.** The document's
///   `judging` edge to `closing` is guarded on `In('standing_down')`, and the event that would put
///   it there was never raised.
/// * **`held` — no writer at all.** The driver read `row["held"]`; no projection ever wrote that
///   key, `RunSummary` had no field for it, and `RunHandle` had no reader. `RunOrder::Hold` was
///   stored into an `AtomicBool` that nothing in any process ever loaded.
///
/// ⚠⚠ **AND `cancel` WORKING IS WHAT PROVES THE DIAGNOSIS RATHER THAN CONTRADICTING IT**:
/// [`RUN_CANCELLED_BY_KEY`] is read as *is it non-null*, never as a bool, and the projection really
/// does write it. The one order spelled compatibly by accident is the one order that reached a run.
///
/// # ⚠⚠⚠ Why a TYPE and not two more hand-spelled keys
///
/// `crate::drive::reporting`'s own doc already states this rule for the other direction —
/// *"`progress_to_json` is called here, not a shape spelled over here … the daemon stores the
/// object without reading it apart, and a key that renderer grows reaches the row with nothing in
/// either process to update"*. The orders path did the opposite: the driver spelled the daemon's
/// shape by hand, in another file, with no compiler between them. **A RECORDED LESSON IS NOT AN
/// APPLIED ONE** — item 618's sentence, and this is the file it was written beside.
///
/// Both ends of this hop live in THIS crate, so a type is free: rename a field and the build stops.
/// That is the ratchet the two hand-spelled keys did not have and could not have grown.
///
/// ⚠ It carries only the orders a person gives a run that is STILL GOING. `cancelled_by` keeps its
/// own key: it names WHO rather than whether, its reader is correct, and folding a working thing
/// into this repair would put a second change on one commit. Residue stated rather than hidden —
/// there are now two places to look for *what was ordered*, and this doc is the pointer between
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct StandingOrders {
    /// A person asked this run to finish its milestone and stand down. A LATCH: there is no way
    /// back, because the orders region of `ai_loop.scxml` has no edge home.
    pub stand_down: bool,
    /// A person is holding this run right now. A LEVEL: `resume-run` sets it back to `false`.
    pub held: bool,
}

impl StandingOrders {
    /// The orders in `row`, or *nothing has been ordered* where the key is absent or unreadable.
    ///
    /// ⚠⚠⚠ **ABSENT MEANS NOBODY ORDERED ANYTHING, AND THAT IS SAFE IN ONE DIRECTION ONLY.** A
    /// driver that guessed *held* from a row it could not parse would park a run nobody asked to
    /// park; one that guesses *not held* keeps working, which is what it was already doing. The
    /// opposite default would let a malformed row stop somebody's loop.
    ///
    /// ⚠ Unreadable is not the same as absent and both answer the same here on purpose: the
    /// alternative is a driver that refuses to drive because a key it does not need is malformed.
    /// ⚠ IT TAKES THE ROW'S OWN MAP and not a [`serde_json::Value`], because that is what
    /// `crate::drive::read_row` — the only caller that matters — actually holds. A signature the
    /// caller has to convert into is a signature that invites the conversion to be spelled twice.
    #[must_use]
    pub fn in_row(row: &serde_json::Map<String, serde_json::Value>) -> Self {
        row.get(RUN_ORDERS_KEY)
            .and_then(|orders| serde_json::from_value(orders.clone()).ok())
            .unwrap_or(Self {
                stand_down: false,
                held: false,
            })
    }
}

/// **WHO RAISED THE CANCEL THAT ENDED THIS RUN, AND WHAT TO DO ABOUT IT** — register item 596.
/// Absent unless a cancel was raised, which is [`RUN_CEILING_KEY`]'s presence-is-the-claim rule.
///
/// # ⚠⚠⚠⚠⚠ Why a key beside `cancelled` and not two words in place of it
///
/// `RunRegistry::cancel` (a person) and `RunRegistry::cancel_all` (a daemon shutting down) raised
/// the same flag, so the driver raised one `OrchestrationEvent::Cancel` and both runs ended on the
/// same word — while [`crate::runs::Canceller::describe`] shows the remedies are opposite: one is a
/// decision to respect, the other is a run **nobody decided anything about** and that a person
/// almost certainly wants back. Splitting the STATE would have made every existing reader of
/// `cancelled` wrong about runs it had already understood; a second key leaves them all correct and
/// merely less informed. That is the shape [`RUN_CEILING_KEY`] and [`RUN_STOPPED_KEY`] chose first.
///
/// ⚠⚠ **A SENTENCE, NOT THE ARM'S NAME** — [`RUN_STOOD_DOWN_KEY`]'s reason verbatim. A reader
/// handed `"shutdown"` still has to know what a shutdown implies for their run; the sentence is
/// the part they were actually after.
///
/// ⚠ No [`sprag_rpc::WIRE_PROTOCOL`] bump, for `RUN_STOOD_DOWN_KEY`'s argument unchanged: an added
/// ANSWER key is absent-not-wrong, and the handshake refuses a version mismatch outright so no
/// reader can mistake absence for *this daemon cannot say*.
pub const RUN_CANCELLED_BY_KEY: &str = "cancelled_by";

sprag_vt::closed_set! {
    /// WHERE A RUN HAS GOT TO — the `status` word inside a run's `state`.
    ///
    /// # ⚠⚠ Why this became a type on the round that added a word to it
    ///
    /// The four words were string literals inside `run_to_json`, so the vocabulary a peer decodes
    /// had no declaration anywhere — and `an_answers_value_space_cannot_widen_under_the_protocol_number`
    /// pins value spaces by walking each closed set's `ALL`. A vocabulary with no type is invisible
    /// to it: adding `interrupted` moved a value space a peer fails the WHOLE document on, and the
    /// pin that exists to catch exactly that could not see it. R353's mouse words were in this state
    /// in two crates; these were in one renderer.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum RunStatus {
        /// A worker is still driving the plugin.
        Running,
        /// It reached a terminal state; `outcome` says which.
        Done,
        /// Its worker panicked (defensive — a plugin step should not).
        Panicked,
        /// ⚠ The daemon driving it died. Added at `WIRE_PROTOCOL` 21.
        Interrupted,
    }
}

impl RunStatus {
    /// This status's word on the wire — the ONE mapping, exhaustive so a fifth cannot reach a
    /// client without one.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Panicked => "panicked",
            Self::Interrupted => "interrupted",
        }
    }
}

sprag_vt::wire_words!(RunStatus: wire_str);

sprag_vt::closed_set! {
    /// WHICH BUNDLED PLUGIN a `run` names — the `plugin` discriminator's whole vocabulary.
    ///
    /// # Why the discriminator is a type
    ///
    /// It was a hand-written `const PLUGINS: &[&str]` beside a `match` over the same four string
    /// literals, so the list a client reads out of the `plugins` slot and the words `build_plugin`
    /// admits were two definitions of one vocabulary — the shape a fifth plugin is left out of, and
    /// the shape [`sprag_input::MouseButton`] was in until R353 (there in two crates, here in one
    /// file). They are one array now, and adding a variant reaches the wire in the compile that adds
    /// it.
    ///
    /// ⚠ Distinct from `PluginKind`, which CARRIES a built plugin: this is the NAME a request sends,
    /// and it exists on its own because a name is what a schema can publish and a built `Dialogue`
    /// is not.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum PluginName {
        /// Drive one pane with a stimulus until a sentinel appears.
        Orchestrator,
        /// Relay one pane's output into another's input.
        Pipe,
        /// Prompt an agent in a pane and collect its reply.
        Agent,
        /// Run two endpoints against each other, turn by turn.
        Dialogue,
        /// ANSWER the question one pane's peer has stopped to ask, once, and stop.
        ///
        /// ⚠ The one that is not a loop, and the reason it is a plugin at all rather than a
        /// synchronous verb: answering takes a keystroke, a look at what the peer did with it, and
        /// possibly a second keystroke — seconds of waiting close to the panes. A wire action doing
        /// that would block the serve loop; a run does it on its own thread and hands back
        /// everything the run registry already gives (an id, a cancel flag, a journal, and the
        /// count of decisions taken on somebody's behalf).
        Answer,
        /// ⚠⚠⚠ **RUN `ai_loop.scxml` AGAINST AN AGENT IN A PANE** — the outer loop, as a run
        /// somebody can start.
        ///
        /// The one plugin whose behaviour is AUTHORED rather than written in Rust: what it prompts
        /// with, when it stops, how many turns it may take and what it does with a blocked peer
        /// are a statechart document, and this is the driver that makes that document act on a
        /// pane. See [`sprag_plugin::ai_loop`].
        ///
        /// ⚠ It is the only form that takes a BRIEF, because it is the only plugin whose job is
        /// not in its arguments. An `agent` run carries the prompt it will send; a loop carries
        /// what it is FOR, and composes each turn's prompt from that in the document's own words.
        AiLoop,
    }
}

impl PluginName {
    /// HOW TO CALL THIS PLUGIN — the `run` form that selects it.
    ///
    /// # ⚠⚠ Why the form belongs to the type and not to a list beside it
    ///
    /// The four forms were a hand-written array in [`crate::wire::PluginGrammar`], and the type's
    /// doc above claimed a variant *"reaches the wire in the compile that adds it"*. That was true
    /// of the WORD and false of the form: a fifth plugin would have been published as a legal
    /// `plugin` value with nothing anywhere saying what to send it, and every gate over that table
    /// would have passed, because a gate over a declaration cannot see one nobody made.
    ///
    /// Exhaustive, so the compiler asks the question instead. `PluginGrammar::RUN` is now a
    /// projection of `ALL` through this.
    #[must_use]
    pub const fn form(self) -> sprag_rpc::CallForm {
        match self {
            Self::Orchestrator => crate::wire::PluginGrammar::ORCHESTRATOR_FORM,
            Self::Pipe => crate::wire::PluginGrammar::PIPE_FORM,
            Self::Agent => crate::wire::PluginGrammar::AGENT_FORM,
            Self::Dialogue => crate::wire::PluginGrammar::DIALOGUE_FORM,
            Self::Answer => crate::wire::PluginGrammar::ANSWER_FORM,
            Self::AiLoop => crate::wire::PluginGrammar::AI_LOOP_FORM,
        }
    }

    /// This plugin's word in a `run` request's `plugin`.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Orchestrator => "orchestrator",
            Self::Pipe => "pipe",
            Self::Agent => "agent",
            Self::Dialogue => "dialogue",
            Self::Answer => "answer",
            // ⚠ `ai_loop` AND NOT `loop`, and the distinction is not decoration: every plugin on
            // this surface is a loop, so the shorter word would claim to be THE one. This is the
            // document's own name, which is what the whole tree already calls it.
            Self::AiLoop => "ai_loop",
        }
    }

    /// The plugin a `plugin` word names, or [`None`] for a word no plugin spells.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|it| it.wire_str() == word)
    }
}

sprag_vt::wire_words!(PluginName: wire_str);

/// The default iteration ceiling for a `run` that omits guardrails — never
/// unbounded (the README makes loop safety first-class), and the floor that
/// bounds every run regardless of its cost unit.
///
/// Published on [`GUARDRAIL_DEFAULTS_SLOT`], which is what makes it one number with one reader
/// rather than a constant every mouth compiles in for itself.
pub const DEFAULT_MAX_ITERATIONS: u32 = 100;
/// The default cost ceiling for a byte-relay plugin (Orchestrator/Pipe/Agent),
/// in injected PTY bytes.
pub const DEFAULT_MAX_BYTES: u64 = 64 * 1024;
/// The default cost ceiling for the token-denominated Dialogue plugin, in real
/// input+output tokens (cache tokens are excluded — see `reply::parse_tokens`).
/// A COARSE backstop, not the primary bound: at the default 100-iteration cap
/// (~2k tokens/turn for a real dialogue) the iteration cap bites first, and a
/// print-mode Text dialogue reports `Tokens(0)` so only iterations bound it.
/// This ceiling exists to stop a single pathological high-token turn; tune it to
/// the model's pricing if a dollar-aware bound is ever needed.
pub const DEFAULT_MAX_TOKENS: u64 = 200_000;

/// THE DEFAULT WALL-CLOCK CEILING for a run that names none, in seconds — one hour.
///
/// Never absent, on [`DEFAULT_MAX_ITERATIONS`]'s exact terms: a run this daemon starts is bounded
/// in time whether or not the caller thought about time, because the README makes loop safety
/// first-class and a bound you have to remember is a bound somebody forgets.
///
/// # Why an hour, and why it is a backstop rather than the primary bound
///
/// The two per-step ceilings bite first for every plugin this build ships. A byte-relay run does
/// its hundred iterations in seconds. A dialogue's turn is bounded by
/// [`sprag_plugin::run::DEFAULT_REPLY_TIMEOUT`] at two minutes, so an hour is
/// roughly thirty full-length turns — beyond any dialogue the iteration ceiling allows to be slow.
/// What an hour catches is the case neither of the others can see: a run whose steps have stopped
/// making progress but have not stopped, which counts no iterations and spends nothing while it
/// holds a pane.
///
/// It is a CEILING as well as a default at the agent-facing mouth, so raising it for an agent is
/// the person's to do — see `tool_orchestrate`.
pub const DEFAULT_MAX_SECONDS: u64 = 3600;

/// **WHAT BUILDING A PLUGIN NEEDS TO KNOW ABOUT THE WORLD IT WILL DRIVE** — the two questions, and
/// no others, that `PluginsExternal::build_plugin` asks of anything outside its request map.
///
/// # ⚠⚠⚠⚠⚠ Why this is a trait rather than a workspace handle — register item 544
///
/// The run driver is moving OUT of this daemon, and a driver in another process has to build the
/// **same plugin from the same request**. A second builder over there would be a second answer to
/// one question — the shape this repository has paid for at every surface it duplicated — and it
/// would drift first in whichever key one of them forgot.
///
/// So the builder stays ONE function and the world it consults becomes an argument. Measured before
/// anything was moved: over the whole of `build_plugin`, exactly two facts come from outside the
/// map — *does this pane exist* and *how big is a pane by default* — and both are answerable from
/// either side of a socket.
///
/// ⚠⚠ **NEITHER IS A `PaneAccess` METHOD, deliberately.** `PaneAccess` is the surface a run DRIVES
/// a pane through; this is what a run is CHECKED against before it starts. Folding them would put
/// *what the daemon's default pane size is* on the trait ten implementations already carry.
pub trait PluginWorld {
    /// Whether `pane` is one this world holds — the fail-fast that turns a mistyped id into a
    /// synchronous refusal instead of a run that dies on its first step.
    fn has_pane(&self, pane: PaneId) -> bool;

    /// How big a pane this world opens is, when the request does not say.
    ///
    /// ⚠ A WORLD-LEVEL default rather than a constant: a daemon's is arbitrated from its attached
    /// clients, so two daemons legitimately answer differently and a driver must ask the one it is
    /// driving.
    fn default_size(&self) -> (u16, u16);

    /// **WHERE `pane` WAS OPENED** — its own record of the directory it was pointed at, or [`None`]
    /// for a pane this world does not hold or cannot say.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the door needs it — register item 738, layer 4
    ///
    /// A loop KIND may name the tree its runs work in, and a pane standing somewhere else walks its
    /// agent into a *"do you trust this folder?"* dialog that the loop's consents do not cover. The
    /// run then waits for a person who is not watching (item 684, measured on a live daemon). The
    /// door is the last place anybody is still looking, so it is where that is caught — and
    /// catching it needs this one fact about a pane it is otherwise only asked to confirm exists.
    ///
    /// ⚠⚠ **IT IS THE PANE'S OWN RECORD AND NOT `/proc/<pid>/cwd`.** That distinction is item 684's
    /// first half: the kernel reading answers `None` the instant the child exits, which is exactly
    /// when a replacement is asked for — so a door reading it would compare against nothing and
    /// wave through the case this exists to catch.
    fn pane_start_dir(&self, pane: PaneId) -> Option<std::path::PathBuf>;
}

/// **WHERE A RUN'S ASKER IS WHEN IT IS NOT IN THIS POOL** — register item 689.
///
/// # ⚠⚠⚠⚠⚠ Why a run needs to ask about a pane it will never drive
///
/// A run has TWO panes in it and only one of them is a target. The target is what it drives, and
/// it must be in this pool — that is [`PluginWorld::has_pane`], and it is right. The other is
/// the request's `opened_by`, the SEAT THE ASKER IS SITTING IN, and there is no reason on earth for
/// that one to be in the same window: an agent works in a window of its own and drives a pane in
/// another one, which is the whole shape `open_window` and `break_pane` exist to make.
///
/// Both were checked against the one pool, and nothing noticed while the only mouth that sends a
/// provenance also always drove its own window. The moment that mouth carried the target's window
/// (register item 687), a request naming a real pane and a real asker came back
/// *"no pane 0 in this workspace, so nothing can be opened by it"* — a refusal about the CALLER,
/// on a call the caller made correctly.
///
/// ⚠⚠ **AN OPAQUE `Fn`, on the exact terms of this surface's four other hooks.** Answering it means
/// walking the session tree, and the session tree is what this layer is deliberately free of
/// (Interface Segregation — see [`crate::workspace_scene`]). What crosses the boundary is a pane id
/// and an answer about it; what never crosses is a registry.
///
/// `None` off a daemon, where this pool is the only world there is and the pool's own answer is the
/// whole truth.
pub type SeatElsewhere = Arc<dyn Fn(PaneId) -> Option<PaneSeat> + Send + Sync>;

/// What a [`SeatElsewhere`] found — the pane EXISTS somewhere this daemon holds, and this is who is
/// sitting in it.
///
/// ⚠ The existence is carried by the [`Option`] around this value and not by a field, because the
/// two questions a provenance asks are *is this a real seat* and *whose conversation is in it*, and
/// a struct with an `exists: bool` would let a caller read the second while the first said no.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PaneSeat {
    /// The agent conversation the pane is holding, or [`None`] when nothing agent-shaped is in it.
    pub session: Option<String>,
}

/// [`PluginWorld::has_pane`] as the REFUSAL a request gets — the sentence, spelled once.
///
/// ⚠ A free function rather than a default method, because the wording is what a person reads when
/// they mistype an id and a second copy of it in a driver process is a second sentence about one
/// fact.
fn require_pane_in(world: &dyn PluginWorld, pane: PaneId) -> Result<(), InvokeError> {
    if world.has_pane(pane) {
        Ok(())
    } else {
        Err(refused(format!("no pane {} in this workspace", pane.0)))
    }
}

/// The daemon's own answer: the pane pool this external was built over.
impl PluginWorld for PluginsExternal {
    fn has_pane(&self, pane: PaneId) -> bool {
        lock(&self.workspace).pane(pane).is_some()
    }

    fn default_size(&self) -> (u16, u16) {
        lock(&self.workspace).default_size()
    }

    /// ⚠ Through [`PaneOrigin`](sprag_plugin::PaneOrigin), which is the SAME read
    /// [`respawn`](sprag_plugin::PaneLifecycle::respawn) makes when it puts a replacement in the
    /// same place — so *where this pane belongs* has one answer in this process rather than two.
    fn pane_start_dir(&self, pane: PaneId) -> Option<std::path::PathBuf> {
        Some(lock(&self.workspace).pane(pane)?.start_dir().to_path_buf())
    }
}

/// **HOW A RUN IS STARTED IN A PROCESS OF ITS OWN** — what
/// [`PluginsExternal::driving_out_of_process`] takes, and see that field for what is on each side
/// of it.
///
/// It takes the two facts about the RUN (its id, and the request it was asked with) and answers the
/// child. Which binary, which endpoint and which session scope are closed over by whoever minted
/// it, because those are facts about the DAEMON and this surface is deliberately free of them.
pub type DriverSpawn = Arc<dyn Fn(RunId, &Map<String, Value>) -> io::Result<Child> + Send + Sync>;

/// **A RUN'S DRIVER, AS THIS LAYER HANDS IT TO THE REGISTRY** — where the driver will write its
/// terminal state, and the handle an order reaches it through.
///
/// ⚠ A named pair because there are two ways to make one (`drive_on_a_thread`, `drive_in_a_process`)
/// and two callers of each, and clippy is right that the spelled-out type is unreadable at four
/// sites. What the pair MEANS is one thing: *the run is going, and here is how to reach it*.
type StartedDriver = (Arc<Mutex<RunState>>, Box<dyn crate::runs::RunHandle>);

/// The plugin host as a pinion `External`: starts background plugin runs over
/// the shared [`Workspace`] and reports their outcomes as scene-as-data.
///
/// # ⚠⚠⚠⚠⚠ Why it is [`Clone`], which is a decision and not a convenience — register item 671
///
/// Every field is a handle to something SHARED — the pane pool, the run directory, the daemon's
/// four hooks, the driver spawner — so a clone is the same surface over the same things and not a
/// second one. What that buys is the answer to *who puts a run back when its driver process dies
/// under a living daemon*: the thread collecting that driver holds a clone, so it can call
/// [`put_back`](Self::put_back) with the same pane pool, the same session's announcements and the
/// same spawner the run started under. A hook passed down from the daemon instead would have to
/// carry all four of those and be handed back INTO the layer it came from, and the replacement
/// driver's own collector would need it again — a cycle this closes by construction, because the
/// clone the new collector takes is made the same way the first one was.
#[derive(Clone)]
pub struct PluginsExternal {
    workspace: Arc<Mutex<Workspace>>,
    runs: Arc<Mutex<RunRegistry>>,
    /// The daemon's opaque pane-exit death-signal ([`crate::spawn_reaper`]), or `None` off a
    /// daemon — passed to each pane a plugin spawns so it feeds the reaper. Registry-free, so
    /// carrying it does not breach the plugin layer's session-tree-free boundary.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The daemon's attention ROUTER ([`crate::attention`]), on exactly the terms above, so a pane a
    /// PLUGIN spawns can ask for a person like any other. `None` off a daemon.
    ///
    /// The router rather than one closure, for the reason [`crate::DaemonShared::attention`] states:
    /// a hook is minted per birth so the reader thread running it takes no lock.
    on_attention: Option<Arc<crate::attention::AttentionRouter>>,
    /// WHAT TO DO WHEN A RUN ENDS — the daemon's announce, as an opaque `Fn(RunId)`.
    ///
    /// # Why the loop's door needed this to be a door at all
    ///
    /// `orchestrate` exists so an agent does not spend its turns driving a loop. Without an event,
    /// the only way to learn a run finished is to ask again — and for an agent every ask is a turn,
    /// which is the cost the feature removes, paid one level up. So the worker announces.
    ///
    /// ⚠ **AN OPAQUE `Fn`, on the exact terms of the three hooks above it.** Announcing means
    /// naming a SESSION channel, and the session tree is what this surface is deliberately free of
    /// (Interface Segregation — see [`crate::workspace_scene`]). The scope that built this external
    /// closed over its own session name; what crosses the boundary is a call with a run id in it.
    on_run_end: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
    /// **WHAT TO DO WHEN A PERSON SAYS SOMETHING TO A RUN** — register item 648, and
    /// [`on_run_end`](Self::on_run_end)'s sibling on exactly its terms.
    ///
    /// # ⚠⚠⚠⚠⚠ Why an order needed a door of its own, measured
    ///
    /// The three orders below ([`cancel`](Self::cancel), [`stand_down`](Self::stand_down),
    /// [`hold_run`](Self::hold_run)) move a flag and announce nothing. That was enough while the
    /// only driver was a THREAD in this process reading those flags directly. It stops being enough
    /// the moment the driver is another process (register item 544): such a driver can READ its
    /// orders — the run row publishes [`RUN_STOOD_DOWN_KEY`] and [`RUN_CANCELLED_BY_KEY`] — and had
    /// **nothing to be woken by**, so it would have to ask on a clock.
    ///
    /// ⚠⚠ That is the shape register items 629, 630, 631 and 640 spent four rounds removing from
    /// the PANE axis, ending at *a remote wait that cost 181 reads over two seconds now costs 1*.
    /// Standing the driver out-of-process without this would have rebuilt it one axis over.
    ///
    /// ⚠ **A SEPARATE HOOK RATHER THAN A WIDENED ONE.** `on_run_end` announces an ENDING and this
    /// announces A PERSON SPEAKING; folding them would make one call site mean two things and force
    /// every reader to re-read a row to find out which. That is the same reason the hooks above it
    /// are separate.
    on_run_ordered: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
    /// The daemon's agent-state memory ([`crate::AgentClock`]), or `None` off a daemon — what lets
    /// a plugin SUPERVISE the agent in a pane instead of guessing from its text.
    ///
    /// The same memory the pane list reads, deliberately: a plugin holding a detector of its own
    /// would be a second authority answering the same question about the same pane, free to
    /// disagree with the row a person is looking at. It crosses into the plugin layer as an opaque
    /// `Fn` ([`agent_state_source`]), so that layer stays registry-free.
    agents: Option<Arc<crate::AgentClock>>,
    /// **WHERE THE ASKER IS SITTING, WHEN IT IS NOT IN THIS POOL** — see [`SeatElsewhere`] for what
    /// this answers and why a run has to be able to ask it. `None` off a daemon.
    seats: Option<SeatElsewhere>,
    /// ⛔⛔⛔⛔ **WHERE A PANE IS WHEN IT IS NOT IN THIS POOL** — register item 682, and
    /// [`seats`](Self::seats)' shape one fact over: that one finds a person, this one finds the
    /// pane a run is DRIVING after somebody moved it to another window. `None` off a daemon, which
    /// leaves a run bound to one window's membership exactly as it was.
    panes: Option<sprag_plugin::access::PaneElsewhere>,
    /// **HOW TO START A RUN IN A PROCESS OF ITS OWN**, or `None` where this host drives runs on
    /// threads of its own — register items 544 and 643.
    ///
    /// # ⚠⚠⚠⚠⚠ What is on each side of this closure, and why the line falls there
    ///
    /// Starting a driver needs four things. Two are facts about THIS DAEMON — which binary it is,
    /// and which endpoint and session scope a client of it must use — and the session tree is
    /// exactly what this surface is deliberately free of (the argument
    /// [`on_run_ordered`](Self::on_run_ordered) and the three hooks above it all make). The other
    /// two are facts about the RUN — its id and the request it was asked with — and this layer is
    /// the only place holding those.
    ///
    /// So the closure is minted where the scope is (`crate::workspace_scene`) and CALLED where the
    /// run is. What crosses is a run id and a request map; what never crosses is a session name.
    ///
    /// ⚠⚠ **IT RETURNS THE CHILD AND NOTHING ELSE.** What becomes of a driver — reading its
    /// outcome, writing [`crate::runs::RunState::Done`], announcing the end — is this layer's, and
    /// is byte-for-byte what an in-process worker does at its own end. A closure that also did that
    /// would be a second spelling of a run's ending, free to drift from the first.
    spawn_driver: Option<DriverSpawn>,
}

impl PluginsExternal {
    /// **DRIVE RUNS IN PROCESSES OF THEIR OWN**, using `spawn` to start each one.
    ///
    /// A setter rather than an eighth constructor argument, the way
    /// [`sprag_plugin::WorkspacePaneAccess`]'s own optional collaborators are taken: seven
    /// positional arguments is already a shape where a caller can transpose two `Option`s of the
    /// same type and compile.
    ///
    /// ⚠ The DAEMON decides whether to call this — see
    /// [`crate::options::RUN_DRIVER_PROCESS`] for why that is not the run requester's choice.
    #[must_use]
    pub fn driving_out_of_process(mut self, spawn: DriverSpawn) -> Self {
        self.spawn_driver = Some(spawn);
        self
    }

    /// **ASK `seats` WHERE A RUN'S ASKER IS WHEN THIS POOL DOES NOT HOLD IT** — register item 689.
    ///
    /// A setter on [`driving_out_of_process`](Self::driving_out_of_process)'s argument: seven
    /// positional arguments is already a shape where a caller can transpose two `Option`s of the
    /// same type and compile.
    ///
    /// ⚠ Whoever installs this is saying *this pool is one window of a daemon that has others*. A
    /// host with no such hook is one whose pool is the whole world, and the pool's own answer is
    /// then the whole truth — which is what an in-process host is.
    #[must_use]
    pub fn reading_seats_elsewhere(mut self, seats: SeatElsewhere) -> Self {
        self.seats = Some(seats);
        self
    }

    /// ⛔⛔⛔⛔⛔ **LET A RUN KEEP THE PANE IT IS DRIVING WHEN SOMEBODY MOVES IT** — register item
    /// 682, and [`reading_seats_elsewhere`](Self::reading_seats_elsewhere)'s shape one fact over.
    ///
    /// Moving a pane between windows is `close` + `adopt`: the pane is untouched and its
    /// MEMBERSHIP changes. A run holds one window's pool for life, so that move turned a healthy
    /// pane into `UnknownPane` and killed the run on its next injection — measured three times on
    /// this repository's own loops, with the pane's program still running across the death.
    ///
    /// ⚠ Whoever installs this is saying *this pool is one window of a session that has others*,
    /// which is the same sentence its neighbour above carries. An in-process host installs
    /// nothing and behaves exactly as it did.
    #[must_use]
    pub fn following_panes_elsewhere(mut self, panes: sprag_plugin::access::PaneElsewhere) -> Self {
        self.panes = Some(panes);
        self
    }

    /// Build the host over the shared workspace + run registry, plus the daemon's
    /// `on_pane_exit` death-signal (`None` off a daemon).
    #[must_use]
    pub fn new(
        workspace: Arc<Mutex<Workspace>>,
        runs: Arc<Mutex<RunRegistry>>,
        on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
        on_attention: Option<Arc<crate::attention::AttentionRouter>>,
        agents: Option<Arc<crate::AgentClock>>,
        on_run_end: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
        on_run_ordered: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
    ) -> Self {
        Self {
            workspace,
            runs,
            on_pane_exit,
            on_attention,
            on_run_end,
            on_run_ordered,
            agents,
            // ⚠ A POOL THAT IS THE WHOLE WORLD is what a host built without saying otherwise has —
            // see `reading_seats_elsewhere`, and `SeatElsewhere` for what a daemon installs there.
            seats: None,
            // ⚠ AND A POOL THAT IS THE WHOLE WORLD CANNOT LOSE A PANE TO ANOTHER WINDOW — register
            // item 682, on the line above's terms. See `following_panes_elsewhere`.
            panes: None,
            // ⚠ IN-PROCESS is what a host built without saying otherwise does — see
            // `driving_out_of_process`, and `crate::options::RUN_DRIVER_PROCESS` for why that is
            // the default rather than the destination.
            spawn_driver: None,
        }
    }

    /// `run` action: build the named plugin, validate its target panes, spawn
    /// it on a background thread, and return its run id.
    fn run(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        // Build the plugin first: it determines the run's cost UNIT, which the
        // guardrails are then sized in (a bare `max_cost` is read in that unit).
        let (plugin, label) = self.build_plugin(map)?;
        let guardrails = parse_guardrails(map, plugin.cost_unit(), plugin.own_bounds())?;
        let opened_by = self.parse_opener(map)?;
        // WHO is in that seat, asked of the daemon rather than taken from the request — see
        // `session_in`. This is what survives the daemon, so it is resolved while the pane is still
        // here to answer.
        let opened_by_session = self.session_in(opened_by);
        // ⚠⚠⚠⚠⚠ THE FORK IS HERE AND NOT IN `spawn_run`, and the reason is what is still in hand:
        // the REQUEST MAP. A driver in another process builds its own plugin from it (one builder —
        // `plugin_from_request`), and `spawn_run` takes a plugin that is already built, so a fork
        // down there would have nothing to hand it.
        //
        // ⚠⚠ EVERYTHING ABOVE HAS ALREADY HAPPENED EITHER WAY. The plugin was built here to
        // VALIDATE — a word no plugin spells, a pane this daemon does not hold, a malformed
        // guardrail — so a bad request is refused synchronously whichever driver would have taken
        // it. The out-of-process arm then throws that plugin away and the driver builds it again
        // from the same map: two builds of one plugin, and that is the property rather than the
        // cost (`crate::options::RUN_DRIVER_PROCESS` promises the same request means the same
        // thing either way, and one builder is how).
        let id = match &self.spawn_driver {
            Some(spawn) => {
                // ⚠ A RUN A CLIENT ASKS FOR STARTS AT THE TOP — where a machine resumes is a fact
                // about a run this daemon INHERITED. See `RUN_PLACE_KEY`.
                self.spawn_driven_run(spawn, map, label, opened_by, opened_by_session, &mut {
                    plugin
                })?
            }
            None => self.spawn_run(label, opened_by, opened_by_session, plugin, guardrails, map),
        };
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
    }

    /// Parse the OPTIONAL `opened_by` — the pane whose occupant is asking for this run.
    ///
    /// The multiplexer's [`parse_opener`](crate::workspace::WorkspaceExternal) rule, verbatim and
    /// for its reason: a caller with a stale `SPRAG_PANE` — a process that outlived its own pane —
    /// would otherwise stamp a provenance naming a pane that does not exist, and nothing would ever
    /// prune it. A non-integer is a MALFORMED request; a pane this daemon does not hold is a
    /// well-formed one it will not honour.
    ///
    /// ⚠⚠⚠⚠⚠ **THIS DAEMON, NOT THIS POOL** — register item 689. The check read one window's pane
    /// list, which made *the asker* and *the pane being driven* obliged to sit in the same window
    /// for no reason either of them has: a provenance is a SEAT, and the whole point of an agent
    /// opening a window of its own is that it is not sitting in the one it works in. The scope that
    /// matches the doc above is *a pane this daemon does not hold*, and [`SeatElsewhere`] is how
    /// this layer asks that without learning what a session tree is.
    fn parse_opener(&self, map: &Map<String, Value>) -> Result<Option<u64>, InvokeError> {
        let opener = match map.get(RUN_OPENED_BY_KEY) {
            None | Some(Value::Null) => return Ok(None),
            Some(value) => value.as_u64().ok_or(InvokeError::TypeMismatch)?,
        };
        if self.require_pane(PaneId(opener)).is_err() && self.seat_of_pane(PaneId(opener)).is_none()
        {
            return Err(refused(format!(
                "no pane {opener} on this daemon, so nothing can be opened by it"
            )));
        }
        Ok(Some(opener))
    }

    /// **THE SEAT `pane` IS**, wherever this daemon is holding it — this pool first, then
    /// [`SeatElsewhere`].
    ///
    /// ⚠ The pool is asked FIRST and not merely as a fallback, so a host with no hook and a host
    /// with one answer identically about every pane the pool holds. What the hook adds is reach,
    /// never a second answer about the same pane.
    fn seat_of_pane(&self, pane: PaneId) -> Option<PaneSeat> {
        if let Some(held) = lock(&self.workspace).pane(pane) {
            return Some(PaneSeat {
                session: held.agent_session().map(str::to_owned),
            });
        }
        self.seats.as_ref().and_then(|look| look(pane))
    }

    /// **WHICH CONVERSATION IS SITTING IN `pane`**, or [`None`] when nothing agent-shaped is.
    ///
    /// Read HERE rather than sent by the caller, on `RunRecord::build`'s argument: it is a fact
    /// about what the daemon is holding, so letting it travel with the request would let a caller
    /// name a conversation it is not in and be answered that conversation's runs. The asker names
    /// its SEAT (`opened_by`, which this daemon then validates); who is in that seat is the
    /// daemon's to say.
    ///
    /// ⚠⚠ **AND IT REACHES AS FAR AS THE CHECK ABOVE IT** — register item 689. Read against this
    /// pool alone, a seat one window over answered [`None`] and the run was recorded as belonging
    /// to no conversation at all: the asker would then not find its OWN run in `list_runs`, which
    /// is the one thing a provenance is for. `parse_opener` accepts a seat this daemon holds
    /// anywhere, so this must read the same seat or the two would disagree about one pane.
    fn session_in(&self, pane: Option<u64>) -> Option<String> {
        self.seat_of_pane(PaneId(pane?))?.session
    }

    /// **WHICH SEAT IS CURRENTLY HOLDING `session`**, or [`None`] when nobody in this workspace is.
    ///
    /// The reverse of [`session_in`](Self::session_in), and the read side of
    /// [`crate::runs::RunRegistry::restore`]'s first rule: a restored run kept the conversation
    /// that asked for it and lost the seat, so the seat is found again here — from whoever is
    /// holding that conversation NOW.
    ///
    /// ⚠⚠⚠⚠ **A LEVEL, RE-DERIVED ON EVERY READ, NEVER A STAMP.** The moment `ai_loop`'s
    /// `restarting` replaces a session the pane holds a FRESH conversation, and this stops matching
    /// on its own — where a value written once at boot would go on claiming an owner that no longer
    /// exists, with nothing to correct it. The cost is a scan of one workspace's panes per read of
    /// the `runs` slot, which is the same order as the slot's own rendering.
    ///
    /// ⚠ Scoped to THIS workspace, which is the conservative half and deliberate: a conversation
    /// sitting in some other scope's pane is not something this reader can be answered about.
    fn seat_of(&self, session: &str) -> Option<u64> {
        lock(&self.workspace)
            .panes()
            .iter()
            .find(|pane| pane.agent_session() == Some(session))
            .map(|pane| pane.id().0)
    }

    /// `cancel` action: raise the cancel flag for run `id`. A synchronous
    /// `Rejected` if no run has that id; the run itself ends `Cancelled`
    /// asynchronously (observe it via `query("runs")`).
    fn cancel(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        if lock(&self.runs).cancel(RunId(id)) {
            // ⚠⚠⚠ NOTHING IS ANNOUNCED HERE — register item 664, and it is a REPAIR rather than an
            // omission. The announcement is raised by `crate::runs::Orders::deliver`, where every
            // order passes and where the daemon's own shutdown sweep passes too; three doors each
            // remembering to publish is what left that fourth caller silent. ⚠ And it lands more
            // precisely there: this arm is `true` for a RESTORED run as well, whose `deliver` does
            // nothing at all, so what used to be published was a wake to re-read a row that had
            // not moved.
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(format!("no run {id} is in flight")))
        }
    }

    /// **ASK A RUN TO FINISH WHAT IT IS DOING AND THEN STOP** — [`STAND_DOWN_ACTION`].
    ///
    /// The one thing a person could say to a run used to be `cancel`, which stops it mid-turn and
    /// throws that turn away. This is the other sentence: the milestone the agent is working toward
    /// is finished, its account is taken, and the run converges. **The work is banked rather than
    /// lost**, which is the whole reason it is a different verb.
    ///
    /// ⚠ It only raises a flag. The worker carries it into the loop document at its next pass, and
    /// the DOCUMENT decides — at its own next milestone — what standing down means. Nothing here
    /// interrupts anything.
    fn stand_down(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        // ⚠⚠⚠⚠ THE REASON IS THE REGISTRY'S, PRINTED VERBATIM — register items 539 and 597. This
        // door used to collapse three states of the world into one boolean and answer `refused: no
        // run N is in flight` for the only one it could name, while a run of a plugin with no
        // reader for the order was answered OK and drove straight on.
        // ⚠ NOTHING IS ANNOUNCED HERE — see `cancel` above and register item 664: the delivery
        // itself publishes, so the accepted arm is the only arm that ever could.
        lock(&self.runs)
            .stand_down(RunId(id))
            .map(|()| IntrospectValue::Null)
            .map_err(|why| refused(why.describe(RunId(id))))
    }

    /// **A RUN'S OWN DRIVER SAYS WHAT IT HAS DONE SO FAR** — [`REPORT_PROGRESS_ACTION`], register
    /// item 650.
    ///
    /// ⚠⚠⚠ **IT ANNOUNCES NOTHING**, and that is the difference between this and the three orders
    /// beside it. Those carry *a person spoke*, which every watcher of a session must be woken for.
    /// This carries *the work moved*, which is a LEVEL — a reader that looks and sees the same
    /// numbers has learned that nothing happened, and waking every client on every step of every
    /// run would be this journal's own *do not announce what a re-read would show* rule inverted.
    ///
    /// ⚠⚠ The progress object is stored WITHOUT being read apart — see `RunRecord::reported`. What
    /// arrives is [`progress_to_json`]'s output, so a key that renderer grows reaches the row with
    /// nothing here to update.
    ///
    /// ⚠ A run this daemon does not hold is REFUSED rather than ignored: a driver reporting for an
    /// id nobody has is a driver that has outlived its run, and telling it so is what lets it stop.
    fn report_progress(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get(RUN_ID_KEY)
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        let progress = match map.get(PROGRESS_KEY) {
            Some(Value::Object(progress)) => Value::Object(progress.clone()),
            _ => return Err(InvokeError::TypeMismatch),
        };
        if lock(&self.runs).report(RunId(id), progress) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(format!("this daemon holds no run {id}")))
        }
    }

    /// **HALT A RUN BETWEEN TURNS, OR LET IT GO AGAIN** — [`HOLD_RUN_ACTION`], and the word a person
    /// did not have (register item 9).
    ///
    /// `cancel` loses the turn and `stand_down` ends the run; neither of them is *wait, let me read
    /// this*. `ai_loop.scxml` has carried the edge for it since R378 with nothing able to raise it.
    ///
    /// ⚠⚠⚠ **IT TAKES `held` WHERE ITS TWO NEIGHBOURS TAKE NOTHING**, and that asymmetry is the
    /// meaning rather than an inconsistency: those two are LATCHES on purpose, and this is a level a
    /// person raises and lowers. The document's `resume` is the way back it was built with.
    ///
    /// ⚠ It only moves a flag. The worker carries it into the document at its next pass and the
    /// DOCUMENT decides; nothing here interrupts anything, and a held run is still running.
    fn hold_run(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = map
            .get("id")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        // ⚠ ABSENT MEANS *hold it* — the direction somebody typing this verb by hand almost always
        // means, and the one a caller that omitted the key cannot have meant to invert. Malformed is
        // refused rather than defaulted, this surface's rule for every optional it reads.
        let held = match map.get("held") {
            None | Some(Value::Null) => true,
            Some(value) => value.as_bool().ok_or(InvokeError::TypeMismatch)?,
        };
        // ⚠⚠⚠⚠ THE REASON IS THE REGISTRY'S — items 539 and 597, its sibling door's argument
        // verbatim. This is the one the register was FILED for: a person holds an `orchestrator` to
        // read its pane, is told the pane is now still, and it is not.
        // ⚠⚠ A REPEATED HOLD ANNOUNCES TOO, and that rule now lives where the order is written —
        // `crate::runs::Orders::deliver`, register item 664. The event says *a person spoke*, not
        // *the level moved*, and suppressing it would need a reader of the previous level, which
        // was never this surface's fact to hold.
        lock(&self.runs)
            .hold(RunId(id), held)
            .map(|()| IntrospectValue::Null)
            .map_err(|why| refused(why.describe(RunId(id))))
    }

    /// Parse the plugin discriminator + its args, validating target panes
    /// exist (fail fast → synchronous `Rejected`).
    ///
    /// ⚠ THIS EXTERNAL'S OWN WORLD, handed to the one builder — see [`plugin_from_request`].
    fn build_plugin(&self, map: &Map<String, Value>) -> Result<(PluginKind, String), InvokeError> {
        plugin_from_request(self, map)
    }
}

/// **WHAT A RUN LEFT BEHIND** — [`drive_request`]'s answer, and the two things the daemon's own
/// worker thread reads off a finished run today.
///
/// ⚠ The plugin is dropped with the run: `captured` is taken from it before it goes, because it is
/// the one thing only the plugin can answer and nothing above this can ask afterwards.
#[derive(Debug)]
pub struct Driven {
    /// How the run ended, in the driver's own vocabulary.
    pub outcome: sprag_plugin::Outcome,
    /// What the plugin captured — an AI adapter's reply, where there is one.
    pub output: Option<String>,
}

/// **BUILD THE PLUGIN THIS REQUEST NAMES AND DRIVE IT TO A TERMINAL STATE** — the one door a run
/// goes through, whichever process is holding it. Register items 544 and 643.
///
/// # ⚠⚠⚠⚠⚠ Why this is a door rather than two calls
///
/// A driver outside this daemon needs to build a plugin and drive it; it never needs to HOLD one.
/// Handing it `PluginKind` would make that private enum public — the whole plugin vocabulary
/// exported so one caller could pass it straight back — where what it actually wants is the
/// outcome. So the door takes a request and answers an ending, and the enum stays where it is.
///
/// ⚠⚠ **THE DAEMON DOES NOT CALL THIS**, and that is deliberate rather than an oversight: it has to
/// register the run — with its label and plugin name — BEFORE the driving starts, so it builds
/// first and drives second. It is the same builder either way — `plugin_from_request`, private
/// because a caller outside this crate has nothing to do with a built plugin — which is
/// the property that matters; a shared door that forced the daemon to register afterwards would be
/// making one caller's shape the other's problem.
///
/// # ⚠⚠⚠⚠ `report` IS A CALL AND NOT A CELL, and that is register item 650 being repaired
///
/// The first form of this door took a [`sprag_plugin::ProgressCell`], because that is what the
/// daemon's own worker shares with its `Driver`. Its one caller — the out-of-process driver — had no
/// reader for such a cell and handed it `ProgressCell::default()`: **progress written on every step
/// and read by nobody**, which is register item 492's shape (*a number authored and never read*) in
/// the feature whose whole subject is a run somebody can watch.
///
/// A cell is right for a watcher that shares memory and wrong for one across a socket, so the
/// parameter is the thing that is true in both places: **where this run's progress goes**. The
/// driver hands a call that puts it on the wire ([`REPORT_PROGRESS_ACTION`]); an in-process caller
/// would hand one that writes its cell.
///
/// # Errors
///
/// Whatever the builder refuses: a word no plugin spells, a malformed argument, or a
/// pane this world does not hold. **All of it before a byte is typed** — that is what the world
/// argument is for.
pub fn drive_request(
    world: &dyn PluginWorld,
    request: &Map<String, Value>,
    access: &dyn sprag_plugin::PaneAccess,
    run: &sprag_plugin::RunContext,
    report: sprag_plugin::ProgressSink,
) -> Result<Driven, InvokeError> {
    let (plugin, _label) = plugin_from_request(world, request)?;
    let mut plugin = plugin;
    // ⚠⚠⚠⚠⚠ **AND HERE IS WHERE A RESTART STOPS KILLING A RUN** — register item 543's fourth
    // brick. The words came out of a predecessor daemon's run log, were checked against THIS
    // image's documents (`PersistedRun::resumable_place`), and were written onto this request by the
    // daemon that started this driver. The plugin is put back at them BEFORE the first step, which
    // is the only moment a machine may be moved by anything but its own driver.
    //
    // ⚠⚠ EVERY REFUSAL ENDS THE DRIVER BEFORE A BYTE IS TYPED, which is this door's stated
    // contract and is load-bearing here rather than tidy: the alternative to refusing is *carry on
    // from the top*, and a loop that starts from the top re-types its opening prompt into somebody's
    // pane. A resume that silently became a restart would spend the peer's tokens saying what was
    // already said — the exact failure item 543 exists to end — and nothing downstream could tell.
    if let Some(place) = opt_place(request)? {
        match plugin.as_plugin().resume_at(&place) {
            sprag_plugin::Resumption::Placed => {}
            // ⚠ THE DAEMON'S MISTAKE, NOT THE CALLER'S: a place is only ever offered to a run whose
            // log carried one, and only `ai_loop` writes one. Reaching this means the request and
            // the log disagree about what plugin this run is.
            sprag_plugin::Resumption::NoMachine => {
                return Err(refused(format!(
                    "this run was to be resumed at a saved place and {} has no machine to put \
                     back; starting it from the top would re-type its opening prompt",
                    plugin.name().wire_str()
                )));
            }
            sprag_plugin::Resumption::NotThisDocument => {
                return Err(refused(
                    "the saved place is not spelled in this build's statecharts, so this run \
                     cannot be resumed into them"
                        .to_owned(),
                ));
            }
            sprag_plugin::Resumption::Refused(why) => {
                return Err(refused(format!(
                    "this build's own machine will not be put back at the saved place: {why}"
                )));
            }
        }
    }
    let guardrails = parse_guardrails(request, plugin.cost_unit(), plugin.own_bounds())?;
    let outcome =
        Driver::new(guardrails)
            .forwarding_to(report)
            .run(plugin.as_plugin(), access, run);
    // ⚠ TAKEN BEFORE THE PLUGIN GOES, exactly as the daemon's worker does it: the capture is the
    // plugin's own and nothing above this layer can ask once it has been dropped.
    let output = plugin.as_plugin().captured();
    Ok(Driven { outcome, output })
}

/// **WHICH PANE A REQUEST'S RUN WOULD DRIVE** — [`RUN_PANE_KEY`], or [`None`] for a request that
/// names none under it. Register item 543's sixth brick.
///
/// # ⚠⚠⚠⚠⚠ It is a LOCATOR and not a second parse, and the difference is what it may be wrong about
///
/// `plugin_from_request` is the one authority on what a request means, and this does not pretend
/// to be a second one: it exists because a daemon BOOT has to find the pane POOL before it can build
/// anything at all — a pane access speaks one pool, and the pool is chosen by where the pane is
/// (`sprag_terminal::SessionRegistry::pool_holding`). The builder cannot answer that, because
/// building is what it needs the answer for.
///
/// ⚠⚠ **SO ITS ONLY FAILURE MODE IS DECLINING TO RESUME.** A request whose plugin names its pane
/// some other way (`pipe` names two, `src` and `dst`) answers [`None`] here, and a boot then leaves
/// that run the honest `interrupted` it already had. It cannot send a run to the WRONG pool: the
/// builder re-reads the key itself and `require_pane_in` refuses a pane the pool does not hold.
///
/// ⚠ Tied to the builder at the KEY, which is the drift that could actually happen — a rename that
/// touched one and not the other would otherwise leave every inherited loop unresumable in silence.
pub fn pane_named(map: &Map<String, Value>) -> Option<PaneId> {
    map.get(RUN_PANE_KEY).and_then(Value::as_u64).map(PaneId)
}

/// **THE ONE BUILDER, AND THE WORLD IS AN ARGUMENT** — register items 544 and 643.
///
/// # ⚠⚠⚠⚠⚠ Why this is a free function and not a method
///
/// The run driver is moving OUT of this daemon, and a driver in another process has to build the
/// **same plugin from the same request**. A second builder over there would be a second answer to
/// one question — the shape this repository has paid for at every surface it duplicated, drifting
/// first in whichever key one of them forgot.
///
/// So the builder is one function and what it needs from the world is [`PluginWorld`]: measured
/// over the whole of this body, exactly two facts come from outside the map — *does this pane
/// exist* and *how big is a pane by default* — and both are answerable from either side of a
/// socket.
///
/// Parse the plugin discriminator + its args, validating target panes exist (fail fast →
/// synchronous `Rejected`).
fn plugin_from_request(
    world: &dyn PluginWorld,
    map: &Map<String, Value>,
) -> Result<(PluginKind, String), InvokeError> {
    // THROUGH THE TYPE, so a word this refuses is a word the wire does not publish. ⚠ A word no
    // plugin spells is a MALFORMED request (`TypeMismatch`), not a rejected one: that is this
    // wire's taxonomy for every other closed vocabulary it reads, and it was the odd one out here
    // — `refused("this daemon has no plugin called …")` carried a friendlier message and put a
    // grammar refusal in the class reserved for "read, and could not be honoured". The message's
    // job belongs to the published vocabulary now, and the completeness gate can only SEE a
    // vocabulary that refuses as malformed.
    let named =
        PluginName::from_wire(require_str(map, "plugin")?).ok_or(InvokeError::TypeMismatch)?;
    match named {
        PluginName::Orchestrator => {
            let pane = require_pane_id(map, RUN_PANE_KEY)?;
            require_pane_in(world, pane)?;
            let stimulus = require_str(map, "stimulus")?.to_string();
            let sentinel = opt_str(map, "sentinel")?.map(str::to_string);
            let ready_when = opt_ready_when(map)?;
            let ready_within = opt_millis(map, Readiness::WIRE_KEY)?;
            let label = format!("orchestrator pane={}", pane.0);
            let spec = OrchestrationSpec {
                stimulus,
                sentinel,
                ready_when,
                ready_within,
                may_answer: opt_may_answer(map)?,
                attended: opt_attended(map)?,
                turn: opt_turn(map)?,
            };
            Ok((
                PluginKind::Orchestrator(Orchestrator::new(pane, spec)),
                label,
            ))
        }
        PluginName::Pipe => {
            let src = require_pane_id(map, "src")?;
            let dst = require_pane_id(map, "dst")?;
            require_pane_in(world, src)?;
            require_pane_in(world, dst)?;
            let spec = PipeSpec {
                src,
                dst,
                ready_when: opt_ready_when(map)?,
                ready_within: opt_millis(map, Readiness::WIRE_KEY)?,
                may_answer: opt_may_answer(map)?,
                attended: opt_attended(map)?,
            };
            Ok((
                PluginKind::Pipe(Pipe::new(spec)),
                format!("pipe {}->{}", src.0, dst.0),
            ))
        }
        PluginName::Agent => {
            let pane = require_pane_id(map, RUN_PANE_KEY)?;
            require_pane_in(world, pane)?;
            let prompt = require_str(map, "prompt")?.to_string();
            let mut spec = AgentSpec::new(prompt);
            if !declined(map, "eof") {
                // `Some`, and the wrapper carries meaning: a caller who SAID so overrides what
                // the completion contract would have implied — see `AgentSpec::eof`.
                spec.eof = Some(map["eof"].as_bool().ok_or(InvokeError::TypeMismatch)?);
            }
            if !declined(map, "shows_prompt") {
                spec.shows_the_prompt = map["shows_prompt"]
                    .as_bool()
                    .ok_or(InvokeError::TypeMismatch)?;
            }
            if let Some(timeout) = opt_millis(map, "timeout_ms")? {
                spec.timeout = timeout;
            }
            if let Some(done_when) = opt_done_when(map)? {
                spec.done_when = done_when;
            }
            spec.ready_when = opt_ready_when(map)?;
            spec.ready_within = opt_millis(map, Readiness::WIRE_KEY)?;
            spec.may_answer = opt_may_answer(map)?;
            spec.attended = opt_attended(map)?;
            let label = format!("agent pane={}", pane.0);
            Ok((PluginKind::Agent(Agent::new(pane, spec)), label))
        }
        PluginName::Dialogue => {
            // Dialogue creates its own per-turn panes, so there is no target
            // pane to validate; the endpoints are argv templates.
            let endpoint_a = require_string_array(map, "endpoint_a")?;
            let endpoint_b = require_string_array(map, "endpoint_b")?;
            let seed = require_str(map, "seed")?.to_string();
            let mut spec = DialogueSpec::new(endpoint_a, endpoint_b, seed);
            // The wire keys stay flat (endpoint_a/label_a/format_a) — the
            // Endpoint struct is an in-Rust cohesion fix, not a protocol
            // change; the host bridges the flat keys into endpoints[0/1].
            if let Some(label) = opt_str(map, "label_a")? {
                spec.endpoints[0].label = label.to_string();
            }
            if let Some(label) = opt_str(map, "label_b")? {
                spec.endpoints[1].label = label.to_string();
            }
            if let Some(format) = parse_reply_format(map, "format_a")? {
                spec.endpoints[0].format = format;
            }
            if let Some(format) = parse_reply_format(map, "format_b")? {
                spec.endpoints[1].format = format;
            }
            let (default_cols, default_rows) = world.default_size();
            spec.cols = opt_dim(map, "cols")?.unwrap_or(default_cols);
            spec.rows = opt_dim(map, "rows")?.unwrap_or(default_rows);
            if let Some(timeout) = opt_millis(map, "timeout_ms")? {
                spec.timeout = timeout;
            }
            // ⚠ NO readiness barrier here, and the absence is measured rather than an
            // oversight: a dialogue passes each turn's prompt as an ARGV ARGUMENT of the pane
            // it spawns for that turn and never injects a byte, so there is no window in which
            // a shell could be typed into. The three plugins that DO inject all take one.
            let label = format!(
                "dialogue {}<->{}",
                spec.endpoints[0].argv.first().map_or("?", String::as_str),
                spec.endpoints[1].argv.first().map_or("?", String::as_str),
            );
            Ok((PluginKind::Dialogue(Box::new(Dialogue::new(spec))), label))
        }
        PluginName::Answer => {
            let pane = require_pane_id(map, RUN_PANE_KEY)?;
            require_pane_in(world, pane)?;
            // ⚠⚠ REQUIRED, alone among the forms — see
            // [`PluginGrammar::MUST_ANSWER`](crate::wire::PluginGrammar::MUST_ANSWER). A run
            // with nothing to answer would occupy a run slot to do what not calling does.
            // Read through the SAME parser the optional key uses, so the two spellings of this
            // contract cannot come to admit different objects.
            let consent = opt_may_answer(map)?.ok_or_else(|| {
                refused(format!(
                    "an `answer` run needs a {} — [{{{}: …, {}: …}}], quoting the peer's own \
                         words. Without one there is nothing it may type, which is what not \
                         calling it already does.",
                    Consents::WIRE_KEY,
                    Consent::ASKED_KEY,
                    Consent::ANSWER_KEY,
                ))
            })?;
            let label = format!("answer pane={}", pane.0);
            Ok((
                PluginKind::Answer(sprag_plugin::Answer::new(pane, consent)),
                label,
            ))
        }
        PluginName::AiLoop => {
            let pane = require_pane_id(map, RUN_PANE_KEY)?;
            require_pane_in(world, pane)?;
            // ⚠⚠⚠ THE CONSTRUCTION SITE THE OUTER DRIVER'S DOC HAS NAMED SINCE R378. Building a
            // concrete `IScriptEngine` here is what made `sce-rust-lua` a real dependency of
            // this crate; the manifest carries the argument. It is per RUN and not shared: a
            // datamodel is a run's own state, and two loops sharing one interpreter would be two
            // runs sharing their north star.
            let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
                Arc::new(sce_rust_lua::LuaEngine::new());
            // ⚠⚠⚠ AND THE DECISIONS THIS REPOSITORY'S RUNS RUN UNDER, read off THIS
            // repository's own document.
            //
            // The template used to author them, which meant sprag's standing yesses authorised
            // every run of a file other repositories copy. They moved to `debt_loop.scxml`, so
            // something has to carry them across — and this is that something. **It decides
            // nothing**: it reads one document and hands the values to another, which is the
            // whole of what the governing rule permits a driver to do with a decision.
            //
            // ⚠⚠ WHICH KIND IS NOT A WIRE ARGUMENT YET, and that is scope rather than design.
            // There is one kind, so naming it would be a key with one legal value — and adding
            // an ARGUMENT is a wire bump. The day a second kind exists, that bump is what pays
            // for it.
            let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
                .map_err(|why| refused(why.to_string()))?;
            // ⚠⚠⚠⚠⚠ RESOLVED BY A FUNCTION THAT HANDS THE BRIEF BACK — register item 492. It
            // was a hundred inline lines here, and the eight fall-throughs to the kind document
            // inside it were held by NOTHING: `sprag_plugin`'s own gate had already measured
            // that deleting one of them left the whole workspace green, and the ceiling's round
            // measured it again with the same answer. A `Brief` is the observable that fixes
            // that, and this is the only call site.
            let brief = ai_loop_brief(map, &kind)?;
            let mut spec =
                sprag_plugin::AiLoopSpec::behind(ai_loop_barrier(map, kind_barrier(&kind)?)?);
            // ⚠⚠ READ AS TWO INDEPENDENT KEYS, where the `agent` form's `opt_turn` refuses a
            // bound with no `done_when` beside it. That rule is right there and wrong here:
            // an `agent` run's default contract is `exits`, so a bare bound would be bounding
            // something the caller did not choose — a loop's default is
            // `INNER_SESSION_ENDS`, the contract this document makes load-bearing, so a bare
            // bound bounds exactly the turn the caller is thinking about.
            // ⚠⚠⚠ AND THE INDEPENDENCE IS NOW STRUCTURAL RATHER THAN A CHOICE MADE HERE: the
            // bound cannot be spelled on this spec at all, so the two keys could not be read
            // together even by a caller who wanted them to be.
            spec.done_when = opt_done_when(map)?.unwrap_or(sprag_plugin::INNER_SESSION_ENDS);
            // ⚠⚠⚠⚠⚠ WHERE THIS RUN'S REVIEWS KEEP THEIR COUNTS, AND THIS IS THE ONLY PLACE
            // THAT KNOWS. `sprag-plugin` used to read `$XDG_STATE_HOME` itself, one library
            // down, which made *the daemon's state directory* mean *the home of whoever ran
            // the process* — so the whole suite appended to a developer's `~/.local/state`
            // (measured 2026-08-19: thirty lines from one crate, the write CI's
            // `ambient-home-guard` was red on). The derivation is
            // [`crate::durability::state_dir`], the one this daemon files every other durable
            // artifact under, so the counts land beside the snapshot and the run registry
            // rather than in a second directory of their own.
            //
            // ⚠⚠ NOT a wire key. A caller does not choose where this machine keeps its files,
            // and the document already owns the two decisions that ARE a caller's: whether to
            // keep counts at all and what to call the file (`ledger_into`, which overrides
            // this outright when it is authored absolute).
            spec.review_ledger = Some(crate::durability::state_dir());
            if !declined(map, "shows_prompt") {
                spec.shows_the_prompt = map["shows_prompt"]
                    .as_bool()
                    .ok_or(InvokeError::TypeMismatch)?;
            }
            // ⚠⚠⚠ THE ANSWERING CONTRACT, read through the SAME two parsers every other
            // injecting form uses. A loop is the form that needs it most and was the only one
            // without it: every kind of real work its agent does raises a permission dialog,
            // and a loop that met one with nothing declared stopped having judged no turns.
            // ⚠⚠⚠ IT IS ON THE BRIEF NOW, not the spec: a consent is a decision somebody made
            // in advance and in writing, which is what this document holds — the same move
            // `screen_rules` made, and the end of refusal and approval living in two worlds.
            // ⚠⚠⚠⚠⚠ THE BOUNDS THIS REPOSITORY'S DOCUMENT NAMES, read HERE because this is where
            // the kind document is open — register item 738, layer 1. `parse_guardrails` runs one
            // call later with the kind long dropped, so what travels to it is the answer rather
            // than a second read of somebody's clause.
            //
            // ⚠⚠ THE UNIT IS THE PLUGIN'S OWN AND IS NAMED BEFORE THE PLUGIN EXISTS, which is the
            // one awkward line here and is stated rather than hidden: a loop spends BYTES, the
            // enum's `cost_unit` says so, and reading the clause after construction would mean
            // building a plugin that might have to be thrown away because its own document names a
            // guardrail no run of it can have.
            let authored = kind_guardrails(&kind, Cost::Bytes(DEFAULT_MAX_BYTES))?;
            // ⛔⛔⛔⛔⛔ AND THE PANE MUST BE STANDING IN A TREE THIS KIND WORKS IN — register item
            // 738, layer 4. See `ai_loop_stands_where_it_works` for the measurement: a pane in
            // `$HOME` walks its agent into a trust dialog no consent of this loop covers, and the
            // run then waits for a person who is not watching.
            //
            // ⚠⚠⚠⚠⚠ AFTER EVERY READ OF THE REQUEST, AND THAT ORDER IS A CONFORMANCE FINDING
            // RATHER THAN A PREFERENCE. Placed first, it answered a request carrying a malformed
            // `north_star` with a sentence about the PANE — so
            // `a_declared_argument_is_one_the_plugin_host_reads` reported that this form's declared
            // arguments were read by nobody, which is the correct reading of what it saw: **a
            // refusal about the WORLD that pre-empts a refusal about the REQUEST makes every
            // argument behind it look unread.** Grammar first, then the facts the world holds.
            ai_loop_stands_where_it_works(
                world.pane_start_dir(pane).as_deref(),
                kind.works_in().as_deref(),
            )?;
            let label = format!("ai_loop pane={}", pane.0);
            let loops = sprag_plugin::AiLoop::new(script, pane, &brief, &spec)
                .map_err(|why| refused(ai_loop_refusal(&why)))?;
            Ok((PluginKind::AiLoop(Box::new(loops), authored), label))
        }
    }
}

impl PluginsExternal {
    fn require_pane(&self, pane: PaneId) -> Result<(), InvokeError> {
        require_pane_in(self, pane)
    }

    /// Spawn the plugin on a background thread that drives it to a terminal
    /// state and writes that into a shared cell; register it.
    fn spawn_run(
        &self,
        label: String,
        opened_by: Option<u64>,
        opened_by_session: Option<String>,
        plugin: PluginKind,
        guardrails: Guardrails,
        request: &Map<String, Value>,
    ) -> RunId {
        let name = plugin.name();
        // The id BEFORE the thread, because the announcement names it and the worker cannot ask the
        // registry for its own id without taking the lock the registry is being written under.
        let id = lock(&self.runs).reserve();
        // The cell the driver writes its counters into, shared with the registry so `runs` can
        // answer them while the run is still spending.
        let progress = sprag_plugin::ProgressCell::default();
        let (state, run) = self.drive_on_a_thread(id, &progress, plugin, guardrails);
        lock(&self.runs).submit(crate::runs::NewRun {
            id,
            label,
            plugin: name,
            // ⚠⚠⚠⚠⚠ **AND WHAT IT WAS ASKED WITH, SO A SUCCESSOR CAN ASK AGAIN** — register item
            // 543's sixth brick. The map is the only thing a restart cannot re-derive: the plugin
            // it describes lives on a thread that will not outlive this daemon, and
            // `plugin_from_request` is the one way to make another one. What the registry does
            // with it is `crate::runs::PersistedRun::request`'s rule, not this layer's.
            request: Some(request.clone()),
            opened_by,
            opened_by_session,
            state,
            run,
            progress,
        })
    }

    /// **DRIVE `plugin` TO A TERMINAL STATE ON A THREAD OF THIS DAEMON'S OWN** — the worker half of
    /// [`spawn_run`](Self::spawn_run), and the half a RESUME needs on its own.
    ///
    /// # ⚠⚠⚠ Why it is separate from the registration around it
    ///
    /// A run being started and a run being PUT BACK ([`put_back`](Self::put_back), register item
    /// 543) want the same driver and different bookkeeping: one reserves an id and submits a new
    /// row, the other is handed an id and a cell that already exist and REPLACES a row's driver. A
    /// second copy of this body for the second case would be a second answer to *what does driving
    /// a run in this daemon mean* — free to drift in whichever hook one of them forgot, exactly the
    /// shape `plugin_from_request`'s own doc argues against one seam further out.
    ///
    /// `id` is taken rather than reserved here because the worker ANNOUNCES under it, and the two
    /// callers get it from different places. `progress` likewise: a fresh run makes one, a resumed
    /// run is handed the cell its row is already publishing.
    fn drive_on_a_thread(
        &self,
        id: RunId,
        progress: &sprag_plugin::ProgressCell,
        mut plugin: PluginKind,
        guardrails: Guardrails,
    ) -> StartedDriver {
        // ⚠⚠⚠⚠ ASKED BEFORE THE PLUGIN MOVES INTO THE WORKER — register items 539 and 597. Once
        // the thread owns it there is nothing left here to ask, so the plugin's own answer is taken
        // now and replayed by `ThreadRun::honours`.
        //
        // ⚠⚠ WALKED FROM `StandingOrder::ALL` rather than listed: an order added to that set is
        // asked about here with nothing to remember, which is this repository's *a list with no
        // glob decides alone* rule pointed the other way — the glob is the type's own list.
        let honoured: Vec<sprag_plugin::StandingOrder> = sprag_plugin::StandingOrder::ALL
            .into_iter()
            .filter(|order| plugin.as_plugin().honours(*order))
            .collect();
        let state = Arc::new(Mutex::new(RunState::Running));
        let worker_state = Arc::clone(&state);
        // The cancel flag is shared two ways: the run's RunContext reads it, and
        // the registry holds a clone so a `cancel`/shutdown can set it.
        let cancel = Arc::new(AtomicBool::new(false));
        // ⚠⚠ THE SECOND THING A PERSON CAN SAY TO A RUN, and it needs its own flag: *finish what you
        // are doing and then stop* is not a softer cancel, it is the opposite outcome — the turn in
        // flight is banked rather than lost. See `RunRecord::order`.
        let order = Arc::new(AtomicBool::new(false));
        // ⚠⚠⚠ AND THE THIRD, which is the only one a person can take back — see `RunRecord::hold`.
        // A flag of its own rather than a mode on `order` because that one is a latch by design.
        let hold = Arc::new(AtomicBool::new(false));
        let run_ctx = RunContext::new(Arc::clone(&cancel))
            .ordered_by(Arc::clone(&order))
            .held_by(Arc::clone(&hold));
        let access = WorkspacePaneAccess::new(Arc::clone(&self.workspace))
            .with_pane_exit(self.on_pane_exit.clone())
            // The detector, as an opaque per-pane read. A run that never supervises never calls
            // it, and a host that has none hands `None` — which is what makes "this build cannot
            // supervise" a different answer from "this pane is not an agent".
            .with_agent_state(self.agents.as_ref().map(|agents| {
                agent_state_source(
                    Arc::clone(&self.workspace),
                    Arc::clone(agents),
                    crate::config::agent_settle,
                )
            }))
            // The router becomes a MINTER at this boundary: the plugin layer asks for a hook per
            // pane and never learns what a router is, which is the same opaque-`Fn` discipline the
            // death signal beside it follows.
            .with_attention(self.on_attention.as_ref().map(|router| {
                let router = Arc::clone(router);
                Arc::new(move || router.signal()) as sprag_plugin::access::AttentionMinter
            }))
            // ⛔⛔⛔⛔⛔ AND WHERE ITS PANE WENT IF SOMEBODY MOVED IT — register item 682. The pool
            // above is ONE WINDOW's and this run holds it for life; the hook is what keeps the
            // run's subject the PANE. A host that installs none is a host whose pool is the whole
            // world, and this run behaves exactly as it did.
            .with_panes_elsewhere(self.panes.clone());
        let on_end = self.on_run_end.clone();
        let worker_progress = Arc::clone(progress);
        let handle = thread::spawn(move || {
            let outcome = Driver::new(guardrails).reporting_to(worker_progress).run(
                plugin.as_plugin(),
                &access,
                &run_ctx,
            );
            // The worker still owns the plugin after the run, so it can read any
            // content the plugin captured (an AI adapter's reply) for the host.
            let output = plugin.as_plugin().captured();
            *lock(&worker_state) = RunState::Done {
                outcome: Box::new(outcome),
                output,
            };
            // ⚠ AFTER the state is written, never before: a client woken by this asks `runs`
            // immediately, and an announcement that raced the write would answer `running` about a
            // run the wake said had finished — the client would then park again on an event that
            // has already fired. The order is the whole correctness of the wake.
            if let Some(announce) = on_end {
                announce(id);
            }
        });
        (
            state,
            // ⚠⚠⚠ THE REGISTRY IS TOLD *A RUN*, NOT *A THREAD AND THREE FLAGS* — register item
            // 544's stage 2. This is the one place in the product that knows the driver is
            // in-process, which is exactly where that knowledge should end up once a run's driver
            // can be another process. See `sprag_host::runs::RunHandle`.
            Box::new(crate::runs::ThreadRun::new(
                // ⚠⚠⚠ AND WHERE AN ORDER TO IT IS ANNOUNCED — register item 664. It used to be
                // this surface's three doors that published, each on its own accepted arm, which
                // left the daemon's shutdown sweep publishing nothing at all. Handed to the record
                // instead, so *an order accepted is an order announced* is one rule rather than
                // four callers remembering it.
                crate::runs::Orders::new(
                    cancel,
                    order,
                    hold,
                    honoured,
                    id,
                    self.on_run_ordered.clone(),
                ),
                handle,
            )),
        )
    }

    /// **START THIS RUN IN A PROCESS OF ITS OWN** and register it — register items 544 / 643.
    ///
    /// # ⚠⚠⚠ What is the same as [`spawn_run`](Self::spawn_run), and what is not
    ///
    /// The same: the plugin's own list of standing orders is taken BEFORE the plugin goes (items 539
    /// / 597), the id is reserved before anything can announce under it, and the registry is told
    /// *a run* rather than *a driver and three flags* (item 544's stage 2).
    ///
    /// Not the same: the three flags are **pure record** here — nothing in this image reads them,
    /// and the driver learns an order by being woken to re-read its row (`Event::RunOrdered`, item
    /// 648). And the outcome is not computed here but READ from the child, which is what the
    /// collector thread below is for.
    ///
    /// # Errors
    ///
    /// A driver that could not be started. That is a REFUSAL rather than a malformed request: the
    /// request was read and could not be honoured, which is this surface's taxonomy for exactly
    /// that. ⚠ And it happens before the run is registered, so a failed spawn leaves no row
    /// claiming a run nobody is driving.
    fn spawn_driven_run(
        &self,
        spawn: &DriverSpawn,
        request: &Map<String, Value>,
        label: String,
        opened_by: Option<u64>,
        opened_by_session: Option<String>,
        plugin: &mut PluginKind,
    ) -> Result<RunId, InvokeError> {
        // ⚠⚠ ASKED OF THE PLUGIN THIS DAEMON BUILT TO VALIDATE, which is the only copy on this side
        // of the seam — the driver's own is built over there and is unreachable from here. Both are
        // `plugin_from_request`'s answer for the same map, so the list is the same list.
        let honoured: Vec<sprag_plugin::StandingOrder> = sprag_plugin::StandingOrder::ALL
            .into_iter()
            .filter(|order| plugin.as_plugin().honours(*order))
            .collect();
        let name = plugin.name();

        // ⚠⚠⚠⚠⚠ **A CALLER MAY NOT SAY WHERE A RUN STARTS** — register item 543, and see
        // [`RUN_PLACE_KEY`] for why that is enforced here instead of in the grammar. This wire
        // SWALLOWS an argument it does not publish, and the driver now READS this one, so without
        // this line a client's `run` call would carry an unpublished verb: *start this loop already
        // at `judging`*. Removed unconditionally, which is exactly today's truth — the only thing
        // entitled to write it is a boot putting an INHERITED run back, and that does not exist
        // yet. When it does, it writes the words it read out of the run log, checked against this
        // build's own statechart fingerprint, and this stays the one place that decides.
        //
        // ⚠⚠⚠ **THAT BOOT NOW EXISTS AND IT IS [`PluginsExternal::put_back`]** — register items 543
        // and 662. So the pair is real rather than hypothetical: a CLIENT's place is dropped here,
        // and the daemon's own reaches the child. Both halves are held by one gate, because a
        // strip nobody measures is a strip that gets deleted as dead code.
        let handed = {
            let mut handed = request.clone();
            handed.remove(RUN_PLACE_KEY);
            handed
        };

        // The id BEFORE the child, because the child is TOLD it (`--drive <id>`) and reports its
        // progress under it — `spawn_run`'s reason, one seam further out.
        let id = lock(&self.runs).reserve();
        let (state, run) = self.drive_in_a_process(spawn, id, &handed, honoured)?;

        Ok(lock(&self.runs).submit(crate::runs::NewRun {
            id,
            label,
            plugin: name,
            // ⚠⚠ THE STRIPPED MAP, which is the same decision the line above it makes: what is
            // recorded is *the request that starts this run from the top*, and where it resumes is
            // never part of that — a boot writes the place it read out of ITS OWN log. Recording
            // `request` with a place on it would let one restart's answer become the next
            // restart's question. Register item 543's sixth brick.
            request: Some(handed.clone()),
            opened_by,
            opened_by_session,
            state,
            run,
            // ⚠⚠⚠ AN EMPTY CELL, AND IT STAYS EMPTY — see `RunRecord::reported`. This run's counters
            // are computed in another process and arrive by `report_progress`; the row prefers that
            // report over this cell precisely because this one can never move, and since register
            // item 662 so does the durable log.
            progress: sprag_plugin::ProgressCell::default(),
        }))
    }

    /// **START A DRIVER PROCESS FOR RUN `id` AND WATCH IT** — the child half of
    /// [`spawn_driven_run`](Self::spawn_driven_run), and the half a RESUME needs on its own.
    ///
    /// [`drive_on_a_thread`](Self::drive_on_a_thread)'s argument, one driver kind over: a run being
    /// started and a run being PUT BACK ([`put_back`](Self::put_back)) want the same child and
    /// different bookkeeping — one reserves an id and submits a row, the other is handed an id and
    /// REPLACES a row's driver. A second copy of this body would be a second answer to *what does
    /// driving a run in another process mean*, free to drift in whichever flag one of them forgot.
    ///
    /// ⚠⚠ `request` is taken already decided: `spawn_driven_run` hands the map with a caller's
    /// [`RUN_PLACE_KEY`] STRIPPED, and `put_back` hands one with the daemon's own place WRITTEN ON.
    /// That asymmetry is the whole trust rule and it is settled by the caller, because this is the
    /// layer that starts a process and not the layer that decides what a run may be told.
    ///
    /// # Errors
    ///
    /// A driver that could not be started — a REFUSAL, because the request was read and could not
    /// be honoured, which is this surface's taxonomy for exactly that.
    fn drive_in_a_process(
        &self,
        spawn: &DriverSpawn,
        id: RunId,
        request: &Map<String, Value>,
        honoured: Vec<sprag_plugin::StandingOrder>,
    ) -> Result<StartedDriver, InvokeError> {
        let child = spawn(id, request).map_err(|why| {
            refused(format!(
                "this daemon could not start a driver process for the run: {why}"
            ))
        })?;
        // ⚠⚠⚠ READ BEFORE THE CHILD IS HANDED TO THE COLLECTOR, because this is the only moment it
        // is in one hand — register item 526. A successor daemon reads it out of the run log to
        // find out whether this process is still typing at the pane before it starts another.
        let pid = child.id();
        let state = Arc::new(Mutex::new(RunState::Running));
        // ⚠⚠⚠⚠⚠ **AND THE COLLECTOR TAKES A SURFACE, NOT ONLY AN ANNOUNCE** — register item 671.
        // This clone is what lets the thread that watches this child put the run back on a new one
        // if the child dies without saying anything, over the same pool and through the same door
        // a boot uses. See this type's own note on why it is `Clone`.
        let reviving = self.clone();
        let collector = collect_driver(
            child,
            id,
            Arc::clone(&state),
            self.on_run_end.clone(),
            Some(Arc::new(move |lost| reviving.put_back_a_lost_driver(lost))),
        );
        Ok((
            state,
            // ⚠⚠ THE THREE FLAGS ARE PURE RECORD HERE — nothing in this image reads them, and the
            // driver learns an order by being woken to re-read its row (register item 648).
            //
            // ⛔⛔⛔ **SO THE ANNOUNCER IS NOT AN EXTRA HERE, IT IS THE DELIVERY** — register item
            // 664. Without it this run's `deliver` writes three flags nobody in either process
            // reads, and the driver is never told anything at all.
            Box::new(crate::runs::ProcessRun::new(
                crate::runs::Orders::new(
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                    Arc::new(AtomicBool::new(false)),
                    honoured,
                    id,
                    self.on_run_ordered.clone(),
                ),
                collector,
                pid,
            )),
        ))
    }

    /// ⚠⚠⚠⚠⚠ **PUT A RUN A DEAD DAEMON LEFT BEHIND BACK ON A DRIVER, WHERE ITS LOG SAYS IT WAS** —
    /// register item 543's sixth brick, and the moment a restart stops being a death.
    ///
    /// # ⚠⚠⚠⚠⚠ What each of the five bricks before this one bought, and what is left for here
    ///
    /// A loop can say where its machine is and be put back there (`sprag_plugin::OuterLoop`); that
    /// place survives being written down as words; the words cross a run log and come back only
    /// through a door that checks the document that wrote them
    /// (`crate::runs::PersistedRun::resumable_place`); a plugin takes them
    /// (`sprag_plugin::Plugin::resume_at`); and the words a machine's entry actions wrote cross
    /// beside the states, so a resumed loop composes a real prompt instead of a blank one. Every
    /// one of those is a capability, and until this function existed **nothing in the product
    /// called any of them at boot** — which is register item 492's shape (*authored and never
    /// read*) spread over five rounds.
    ///
    /// # ⚠⚠⚠⚠ It serves BOTH driver kinds, and the round before this one it could not
    ///
    /// This refused outright on a daemon configured to drive runs in processes of their own, for a
    /// measured reason: [`progress_to_json`] carried **neither `at` nor `place`**, so such a run's
    /// counters and position reached the daemon through no channel at all, and `persistable` wrote
    /// a log with no place in it however long the run had been going. There was nothing to put
    /// back for exactly the driver kind that can read a place (`drive_request`, item 543's fourth
    /// brick). Register item 662 closed that: the report carries both, the log prefers the report,
    /// and the fork below is now the same fork a fresh request takes.
    ///
    /// # ⚠⚠⚠ Everything is checked BEFORE the row is touched
    ///
    /// The plugin is built, the guardrails parsed and the machine placed while the run is still
    /// `interrupted`; only then does the row get a driver. A resume that failed half way would
    /// otherwise leave a row claiming to be running with a plugin nobody could step — strictly
    /// worse than the ending it already had.
    ///
    /// # Errors
    ///
    /// Whatever the builder refuses (a pane this daemon no longer holds, a plugin word this build
    /// no longer spells, a malformed guardrail), whatever the machine refuses about the place, and
    /// a row that stopped being resumable between [`crate::runs::RunRegistry::inheritance`] and
    /// here.
    pub fn put_back(&self, inherited: &crate::runs::InheritedRun) -> Result<(), InvokeError> {
        let (mut plugin, _label) = plugin_from_request(self, &inherited.request)?;
        let guardrails =
            parse_guardrails(&inherited.request, plugin.cost_unit(), plugin.own_bounds())?;
        // ⚠⚠⚠⚠ THE SAME FOUR ANSWERS `drive_request` READS, and they are worth spelling separately
        // here rather than collapsing to *it did not work*: the person who meets one of these is
        // holding a run log and a daemon that has just decided not to bring their run back.
        match plugin.as_plugin().resume_at(&inherited.place) {
            sprag_plugin::Resumption::Placed => {}
            sprag_plugin::Resumption::NoMachine => {
                return Err(refused(format!(
                    "run {} was to be put back at a saved place and {} has no machine to put back",
                    inherited.id.0,
                    plugin.name().wire_str()
                )));
            }
            sprag_plugin::Resumption::NotThisDocument => {
                return Err(refused(format!(
                    "run {}'s saved place is not spelled in this build's statecharts",
                    inherited.id.0
                )));
            }
            sprag_plugin::Resumption::Refused(why) => {
                return Err(refused(format!(
                    "this build's own machine will not be put back where run {} was: {why}",
                    inherited.id.0
                )));
            }
        }
        let name = plugin.name();
        // ⚠⚠⚠⚠⚠ **AND THE FORK IS THE SAME ONE A FRESH REQUEST TAKES** — `run`'s, and for its
        // reason: [`crate::options::RUN_DRIVER_PROCESS`] is the daemon's statement about where its
        // drivers live, and a boot that answered it differently would be the invisible divergence
        // that option promises cannot happen. Everything above this line has already happened
        // either way, which is `spawn_driven_run`'s own property: the plugin was built and PLACED
        // here to VALIDATE, so a run whose pane is gone or whose place this build cannot spell is
        // refused before any driver exists — and the out-of-process arm then throws that copy away
        // and the child builds it again from the same map. Two builds of one plugin, one answer.
        //
        // ⚠⚠ **VALIDATING HERE IS WHAT KEEPS A REFUSAL HONEST.** The child would refuse the same
        // place at its own door (`drive_request`, before a byte is typed) — but by then the run has
        // a driver, so the refusal arrives as a run that FAILED. Checked here, the run keeps the
        // `interrupted` it already had, which is the true statement about it.
        let (state, run) = match &self.spawn_driver {
            Some(spawn) => {
                let honoured: Vec<sprag_plugin::StandingOrder> = sprag_plugin::StandingOrder::ALL
                    .into_iter()
                    .filter(|order| plugin.as_plugin().honours(*order))
                    .collect();
                // ⚠⚠⚠⚠⚠ **THE ONE WRITER OF [`RUN_PLACE_KEY`], AND THE OTHER HALF OF
                // `spawn_driven_run`'S STRIP.** A client may not say where a run starts; a daemon
                // putting an INHERITED run back may, because these words came out of its own
                // predecessor's log and were checked against this image's statechart fingerprint
                // (`crate::runs::PersistedRun::resumable_place`) before they got here.
                let mut handed = inherited.request.clone();
                handed.insert(
                    RUN_PLACE_KEY.to_owned(),
                    Value::Array(
                        inherited
                            .place
                            .iter()
                            .map(|word| Value::String(word.clone()))
                            .collect(),
                    ),
                );
                self.drive_in_a_process(spawn, inherited.id, &handed, honoured)?
            }
            None => self.drive_on_a_thread(inherited.id, &inherited.progress, plugin, guardrails),
        };
        if lock(&self.runs).put_back(inherited.id, name, state, run) {
            return Ok(());
        }
        // ⚠⚠⚠ THE ROW WOULD NOT TAKE IT, AND THE DRIVER WAS ALREADY GOING. Unreachable at a boot,
        // which is single-threaded and acts on a list it has just asked for — and answered anyway,
        // because the alternative to saying this is a caller told nothing. The worker itself is
        // stood down by the door that refused it, which is where the handle is.
        Err(refused(format!(
            "run {} stopped being resumable while its driver was being built, so the driver was \
             stood down again",
            inherited.id.0
        )))
    }

    /// **A RUN'S DRIVER PROCESS DIED WITHOUT AN OUTCOME, SO PUT THE RUN BACK ON A NEW ONE** —
    /// register item 671, and the half of item 544's residue that asks *who supervises the
    /// supervisor*.
    ///
    /// # ⚠⚠⚠⚠⚠ The decision this takes, and the fact that settles it
    ///
    /// A boot already does exactly this for every run a DEAD daemon left behind (register item
    /// 543). Doing nothing here would give one run two different fates for the same fact — *nothing
    /// is driving it* — decided by whether the daemon happened to restart, which is an accident and
    /// not an answer. So the live daemon answers it the same way, through the same door, at the
    /// same cost: what the run did since its last report is lost, which is the price item 543
    /// already accepted for a restart.
    ///
    /// ⚠⚠ **AND IT IS BOUNDED BY WHAT THE REPLACEMENT SAYS, NOT BY A COUNT** — see
    /// [`crate::runs::RunRegistry::revival`]. A driver that dies without ever reporting is a broken
    /// image or a request its own door refuses, and respawning it is a spin; a driver that reported
    /// and then died is a run doing work, and there is no number of times that stops being true.
    ///
    /// ⚠ Every refusal is written into the ROW by that same door, so this only has to say it in the
    /// operator's log — where *which run and why* belongs.
    fn put_back_a_lost_driver(&self, id: RunId) {
        // ⚠⚠⚠ THE GUARD IS DROPPED BEFORE `put_back`, WHICH TAKES IT AGAIN. One statement, so the
        // temporary dies at the semicolon — a `match` over the call directly would hold the
        // directory locked through building a plugin and spawning a process, and `put_back`'s own
        // last act is to lock it.
        let verdict = lock(&self.runs).revival(id);
        let inherited = match verdict {
            crate::runs::Revival::PutBack(inherited) => inherited,
            other => {
                tracing::warn!(
                    target: "sprag_host::runs",
                    run = id.0,
                    "a run's driver process died without reporting an outcome and the run was not \
                     put back on a new one: {}",
                    other.not_put_back().unwrap_or("no reason was given"),
                );
                return;
            }
        };
        match self.put_back(&inherited) {
            Ok(()) => tracing::warn!(
                target: "sprag_host::runs",
                run = id.0,
                "a run's driver process died without reporting an outcome, so the run was put back \
                 on a new driver where its last report said it was: {}",
                inherited.label,
            ),
            Err(why) => tracing::warn!(
                target: "sprag_host::runs",
                run = id.0,
                "a run whose driver process died could not be put back on a new one: {why:?}",
            ),
        }
    }
}

/// **WAIT FOR A DRIVER PROCESS, THEN END ITS RUN THE WAY A WORKER ENDS ITS OWN.**
///
/// # ⚠⚠⚠ This thread is not a driver
///
/// A thread per run in this daemon looks like it gives back what moving the driver out was meant to
/// win. It does not: what register item 544 is about is the PLUGIN LOGIC — a loop's turn model, a
/// sentinel rule — living where it can be replaced without restarting the thing that holds the
/// PTYs. None of that is here. This blocks on a child and then writes what the child said.
///
/// ⚠⚠ **AND IT BLOCKS RATHER THAN SAMPLES.** `wait_with_output` IS the wake. Items 629/630/631/640
/// spent four rounds taking clock-paced waiting off the pane axis; a daemon that polled `try_wait`
/// on its drivers would put it straight back on the run axis.
///
/// ⚠ The announcement is AFTER the state is written, which is `spawn_run`'s rule and its reason: a
/// client woken by it asks `runs` immediately, and an announcement that raced the write would answer
/// `running` about a run the wake said had finished.
fn collect_driver(
    child: Child,
    id: RunId,
    state: Arc<Mutex<RunState>>,
    on_end: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
    on_lost: Option<Arc<dyn Fn(RunId) + Send + Sync>>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let ended = match child.wait_with_output() {
            Ok(output) => {
                // ⚠⚠⚠⚠⚠ THE DRIVER'S OWN DIAGNOSTICS, AND THEY WERE BEING THROWN AWAY. A driver
                // writes to stderr when it cannot do something it was asked to — `watch_orders`
                // says so for a subscription it could not open, and `reporting` for a progress
                // report the daemon refused. On a run that CONVERGED, `driver_ending` reads only
                // stdout, so every one of those sentences went nowhere: authored and never read
                // (register item 492), and it cost a round of not knowing why a row stayed still.
                //
                // ⚠ At `warn` and not on the row: this is about the DRIVER's difficulty, not the
                // run's answer, and an operator's log is where a reader looks for the first.
                let said = String::from_utf8_lossy(&output.stderr);
                let said = said.trim();
                if !said.is_empty() {
                    tracing::warn!(
                        target: "sprag_host::plugins",
                        run = id.0,
                        "a run's driver process said: {said}",
                    );
                }
                driver_ending(&output)
            }
            // ⚠ A child this daemon could not even collect is not a run that failed — it is one
            // whose ending is unknowable, and `Panicked` is this registry's word for that.
            Err(why) => RunState::Panicked(format!("collecting a run's driver process: {why}")),
        };
        // ⚠⚠⚠⚠⚠ **A DEATH WITH NO OUTCOME IS ASKED ABOUT BEFORE ANYBODY IS WOKEN** — register item
        // 671. `driver_ending` reads silence as `Panicked`, and that is the shape a run whose
        // driver was killed arrives in; every other ending is the driver's own answer and nobody
        // supervises an answer.
        //
        // ⚠⚠ THE ORDER IS THE WHOLE OF WHAT A READER CAN TRUST. Written, then decided, then
        // announced — so the first row anybody is woken to look at is already the FINAL one: put
        // back and running, or failed and carrying the reason nothing picked it up. Announcing
        // first would wake every watcher onto a `panicked` this daemon was in the middle of
        // undoing, which is the *two readings with a gap between them* register item 637 is about.
        let lost = matches!(ended, RunState::Panicked(_));
        *lock(&state) = ended;
        if let Some(revive) = on_lost.filter(|_| lost) {
            revive(id);
        }
        if let Some(announce) = on_end {
            announce(id);
        }
    })
}

/// What a driver process's exit MEANS, as a run state.
///
/// ⚠⚠⚠⚠⚠ **SILENCE IS AN OUTCOME AND IT IS NOT `converged`.** A driver that died — killed, panicked,
/// out of memory — writes no JSON, and `crate::drive`'s own module doc calls that *"the honest
/// outcome"* the fused design could not express. Reading an empty stdout as anything but a failure
/// would manufacture an ending nobody reported.
fn driver_ending(output: &std::process::Output) -> RunState {
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(Value::Object(reported)) => RunState::Reported(Box::new(Value::Object(reported))),
        _ => RunState::Panicked(format!(
            "a run's driver process ended {} without reporting an outcome{}",
            output.status,
            match String::from_utf8_lossy(&output.stderr).trim() {
                "" => String::new(),
                said => format!(": {said}"),
            }
        )),
    }
}

/// The daemon's detector, as the opaque per-pane read the plugin layer takes.
///
/// # Why this is a closure and not a type the plugin crate could hold
///
/// The verdict a plugin reads has to be the SAME verdict the pane list shows a person, or a
/// supervisor and a human looking at one pane are being told different things about it. That
/// arbitration lives in [`crate::AgentClock`], which sits beside the session tree — and the plugin
/// layer is session-tree-free by decision. Handing across an `Fn(PaneId)` keeps both: one
/// authority, and a substrate that still knows nothing about registries, manifests or settle
/// windows. It is the discipline the pane-exit and attention hooks beside it already follow.
///
/// # What it does per call, and what it does not
///
/// A pull, and it is meant to be pulled: the screen is read under the workspace lock (the detector's
/// own lock nested inside it, never the reverse — the order [`crate::WorkspaceExternal`] documents),
/// and [`AgentClock::observe`](crate::AgentClock::observe) applies the quiescence gate, so a pane
/// whose screen and title have not moved costs no rule evaluation however often a plugin steps.
///
/// The QUESTION is parsed only for a pane that is actually blocked. That is not only thrift: a menu
/// still painted behind a working agent is scenery, and handing it to a supervisor would invite an
/// answer to a question nobody asked. It is read in [`sprag_detect::DIALOG_WINDOW`], the window the
/// built-in manifests block in — a user manifest that declares a wider one may block on a menu this
/// does not enumerate, and the supervisor then sees `asking: None` and hands the pane to a person,
/// which is the right answer to a question it cannot read.
/// # Why the settle window is a parameter
///
/// It is [`crate::config::agent_settle`] on every real host, and it is INJECTED for the reason R331
/// recorded against `window_size`: the only other way in is `$XDG_CONFIG_HOME`, which is
/// process-global, so a test of this path would otherwise assert whatever the developer's
/// `config.toml` happens to say — and a test whose subject is a TIMED transition would be asserting
/// it about a timing it did not choose.
/// ⚠ VISIBLE TO THE CRATE so the live-agent measurement can drive the loop through the REAL
/// detector — see `crate::live_agent`. It is still built in one place and handed out as an opaque
/// `Fn`, which is the property that mattered; what changed is that the one other reader in this
/// crate is a gate rather than the run path.
pub(crate) fn agent_state_source(
    workspace: Arc<Mutex<Workspace>>,
    agents: Arc<crate::AgentClock>,
    window: fn() -> sprag_detect::Hysteresis,
) -> sprag_plugin::AgentStateSource {
    Arc::new(move |id: PaneId| {
        let guard = lock(&workspace);
        let pane = guard.pane(id)?;
        // The CHILD's own title, never the pane's name — the rule the pane list states and for its
        // reason: a name is chosen by whoever asked for the pane, so reading one here would let
        // anyone who can name a pane forge an agent identity.
        let title = pane.title();
        pane.pty().with_screen(|screen| {
            let facts = agents.observe(
                id,
                screen,
                title.as_deref(),
                std::time::Instant::now(),
                window,
            )?;
            let state = sprag_detect::AgentState::from_wire(facts.state)?;
            let authority = match facts.source {
                Some(source) => sprag_plugin::Authority::Reported { source },
                None => sprag_plugin::Authority::Scraped { rule: facts.rule },
            };
            Some(sprag_plugin::AgentObservation {
                // The REGISTRY's parse, not a second one taken here. It reads the same screen at the
                // same instant, and having two sites derive it is how the run surface and the pane
                // surface would come to disagree about what one pane is asking (R367 moved it).
                asking: facts.asking,
                // The agent's own account, carried through untouched — see `AgentObservation::asked`
                // for what a supervisor can do with it that no screen read can.
                asked: facts.asked,
                // ⚠⚠⚠⚠ AND WHAT IT ANSWERED — the half a driver was reading off a pane that cannot
                // be read for it (register item 441). Carried through untouched, exactly like the
                // question above: this layer states, and the plugin judges.
                said: facts.said,
                // ⚠⚠⚠⚠⚠ AND WHY IT WANTS A PERSON — the half `asking` above is `None` for, which is
                // precisely the case a run has to hand to one (register item 452). Carried through
                // untouched on the same terms: this layer states, the plugin decides what to do about
                // it, and neither invents a sentence the peer did not say.
                noticed: facts.noticed,
                transcript: facts.transcript,
                // ⚠⚠⚠⚠ AND WHETHER THE REPORTER CAN STILL DELIVER — register item 709. The
                // IN-PROCESS driver reads this observation and the remote one reads
                // `agent::verdict_of`'s, so both mouths carry the fact or the asymmetry the item is
                // made of just moves in one step. `Unsaid` is unreachable here and must not be
                // spelled: this side HOLDS the tracker, so it always knows.
                reporter: if facts.reporter_mute {
                    sprag_plugin::ReporterVoice::Mute
                } else {
                    sprag_plugin::ReporterVoice::Speaking
                },
                state,
                agent: facts.agent,
                authority,
                seq: facts.seq,
                // ⚠⚠⚠ AND THE COUNT OF QUESTIONS BESIDE THE COUNT OF STATE CHANGES — register item
                // 441. They move for two different reasons and a supervisor needs the second one:
                // `seq` cannot say whether the peer took the prompt just typed at it, because a
                // submit into an already-`working` pane publishes nothing.
                asked_seq: facts.asked_seq,
                // ⚠⚠⚠⚠⚠ AND THE COUNT THAT MOVES WHILE A TURN IS MERELY WORKING — register item
                // 458. The three counters beside it stand still through a turn calling tools, which
                // reads exactly like a turn nothing will ever end; this one is the peer's reporter
                // being alive, carried through untouched like every other stated fact here.
                reports: facts.reports,
                // ⚠⚠ AND THE COUNT THAT DATES THE ANSWER, without which the text above cannot be
                // told from the previous turn's — see `AgentObservation::said_seq`.
                said_seq: facts.said_seq,
                // ⚠⚠⚠⚠⚠ AND WHEN THIS VERDICT CHANGES WITH NOTHING FURTHER HAPPENING — register
                // item 630. Carried through from the same tracker borrow that produced the verdict
                // above, which is what makes the pair unable to disagree; a waiter that asked the
                // registry separately would hold a deadline belonging to a later observation than
                // its own state. It is what lets a wait park to the instant instead of polling the
                // settle window.
                //
                // ⚠⚠⚠ `Nothing` RATHER THAN `Unknown` FOR THE ABSENCE, and only a source that READ
                // THE TRACKER may say it: an empty answer is then *no candidate is waiting*, and a
                // waiter is entitled to park on the pane and look no more. A surface that cannot
                // see candidates at all must say `Unknown`, which is what `Settling` has three arms
                // for.
                //
                // ⚠⚠ THE REMOTE SURFACE IS NO LONGER ONE OF THOSE — register item 640. The same
                // fold happens on the far side of the socket now, off a REMAINING TIME the answer
                // carries (`crate::wire::AGENT_SETTLES_IN_MS_KEY`), so this arm and that one are
                // two spellings of one rule rather than a rule and a degradation.
                settling: facts
                    .settles_at
                    .map_or(sprag_plugin::Settling::Nothing, sprag_plugin::Settling::At),
            })
        })
    })
}

impl fmt::Debug for PluginsExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginsExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(PluginsExternal);

impl PluginsExternal {
    fn read(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            RUNS_SLOT => {
                // ⚠ The snapshot is taken and the registry lock RELEASED before any seat is
                // re-derived: `seat_of` takes the workspace lock, and holding the run registry
                // across it would invert the workspace-then-registry order the host keeps.
                let runs = {
                    let mut registry = lock(&self.runs);
                    registry.sweep(); // reap finished threads before reporting
                    registry.snapshot()
                };
                let entries = runs
                    .iter()
                    .map(|run| {
                        // A run THIS daemon issued already names its seat. One it inherited kept
                        // only the conversation, so the seat is found again from whoever holds that
                        // conversation now — `seat_of`, and see `RunRegistry::restore`'s rule 1.
                        let seat = run.opened_by.or_else(|| {
                            run.opened_by_session
                                .as_deref()
                                .and_then(|session| self.seat_of(session))
                        });
                        run_to_json(run, seat)
                    })
                    .collect();
                Some(IntrospectValue::Json(Value::Array(entries)))
            }
            // The same array the `run` grammar publishes as its `plugin` vocabulary —
            // one definition, two readers.
            PLUGINS_SLOT => Some(IntrospectValue::Json(json!(PluginName::WIRE_WORDS))),
            // THE BOUND A RUN THAT NAMES NONE IS GIVEN, keyed exactly as the `guardrails` argument
            // spells it, so a client that reads a ceiling here can send it back without a mapping.
            GUARDRAIL_DEFAULTS_SLOT => Some(IntrospectValue::Json(json!({
                "max_iterations": DEFAULT_MAX_ITERATIONS,
                "max_seconds": DEFAULT_MAX_SECONDS,
                "max_bytes": DEFAULT_MAX_BYTES,
                "max_tokens": DEFAULT_MAX_TOKENS,
            }))),
            // HOW TO CALL THIS SURFACE'S TWO VERBS — its own `PLUGINS_GRAMMAR`, answered by
            // the surface that serves them (see `ACTION_GRAMMAR_SLOT`).
            crate::wire::ACTION_GRAMMAR_SLOT => Some(IntrospectValue::Json(
                crate::wire::ActionGrammar::answer(crate::wire::PLUGINS_GRAMMAR),
            )),
            _ => None,
        }
    }
}

impl ExternalIntrospect for PluginsExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(
            const {
                &[
                    SchemaField::action(RUN_ACTION, "action"),
                    SchemaField::action(CANCEL_ACTION, "action"),
                    SchemaField::action(STAND_DOWN_ACTION, "action"),
                    SchemaField::action(HOLD_RUN_ACTION, "action"),
                    // ⚠⚠⚠⚠⚠ AN ADDRESS NOT PUBLISHED HERE IS NOT SERVED — the dispatch arm below is
                    // unreachable without this line, and a caller gets `UnknownInvokePath`. That is
                    // this surface being honest rather than a redundancy: the schema is what a
                    // client discovers, so an action reachable but undiscoverable would be folklore.
                    //
                    // ⚠ It cost a round to learn HERE because the only caller is a driver in
                    // another process, and its refusal went to a stderr nobody read (register item
                    // 492 again). The fix for that is beside `collect_driver`.
                    // ⚠⚠⚠⚠⚠ AN ADDRESS NOT PUBLISHED HERE IS NOT SERVED — the dispatch arm below is
                    // unreachable without this line, and a caller gets `UnknownInvokePath`. That is
                    // this surface being honest rather than a redundancy: the schema is what a
                    // client discovers, so an action reachable but undiscoverable would be folklore.
                    //
                    // ⚠ It cost a round to learn HERE because the only caller is a driver in
                    // another process, and its refusal went to a stderr nobody read (register item
                    // 492 again). The fix for that is beside `collect_driver`, and removing this
                    // line now reds `the_daemon_drives_a_run_in_a_process_of_its_own` — measured.
                    SchemaField::action(REPORT_PROGRESS_ACTION, "action"),
                    SchemaField::new(RUNS_SLOT, "list"),
                    SchemaField::new(PLUGINS_SLOT, "list"),
                    SchemaField::new(GUARDRAIL_DEFAULTS_SLOT, "object"),
                    SchemaField::new(crate::wire::ACTION_GRAMMAR_SLOT, "object"),
                ]
            },
        )
    }

    /// ⚠⚠ **THE IDENTITY MIGRATION, and `UnknownPath` is what a `None` ALWAYS MEANT.**
    ///
    /// pinion R1674 widened a read's failure from an absence into a REFUSAL with a reason
    /// (`ReadRefusal`), and its dispatch maps `UnknownPath` onto the very fault a `None` produced
    /// before it (`QueryError::UnknownIntrospectPath`). So wrapping the reading below preserves
    /// this surface's wire behaviour exactly, which is what a pin bump owes its callers.
    ///
    /// ⚠ The three RICHER arms — `NoSuchMember`, `Unavailable`, `QueryTypeMismatch` — are the
    /// point of the upstream change and are NOT adopted here. Each is a per-path decision about
    /// what this surface knows, and several of them supersede reasoning this file already wrote
    /// down; taking them in the same edit as a pin bump would ship refusal sentences nobody
    /// derived. Registered as owed rather than guessed.
    fn query(&self, path: &str) -> Result<IntrospectValue, ReadRefusal> {
        self.read(path).ok_or(ReadRefusal::UnknownPath)
    }

    /// The reading itself — see [`query`](Self::query) for why it still answers an
    /// `Option` and what that `None` becomes.
    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots: starting a run is an action (invoke `run`).
        Err(InterveneError::UnknownPath)
    }

    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        match path {
            RUN_ACTION => self.run(&args),
            CANCEL_ACTION => self.cancel(&args),
            STAND_DOWN_ACTION => self.stand_down(&args),
            HOLD_RUN_ACTION => self.hold_run(&args),
            REPORT_PROGRESS_ACTION => self.report_progress(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// A bundled plugin chosen at `run` time. An enum (not `Box<dyn Plugin>`) so the
/// worker thread moves a concrete `Send` value and the match stays explicit.
enum PluginKind {
    Orchestrator(Orchestrator),
    Pipe(Pipe),
    Agent(Agent),
    // Boxed: a `Dialogue` carries two embedded SCE session engines, so it is far
    // larger than the byte-relay plugins; boxing the one big variant keeps the
    // enum small instead of every value paying its footprint.
    Dialogue(Box<Dialogue>),
    Answer(sprag_plugin::Answer),
    // Boxed for the `Dialogue` reason above: an `AiLoop` owns a compiled `ai_loop.scxml` engine
    // and the script interpreter its datamodel lives in.
    //
    // ⚠⚠⚠⚠⚠ AND THE BOUNDS ITS KIND DOCUMENT NAMED — register item 738, layer 1. They are read
    // where the kind document is opened (`build_plugin`) and carried here because
    // `parse_guardrails` runs one call later, with the plugin in hand and the kind long dropped.
    // Re-opening the document there would be a SECOND read of one author's clause, which is what
    // `LoopKind`'s own doc says a kind must never become — so what travels is the answer.
    AiLoop(Box<sprag_plugin::AiLoop>, AuthoredGuardrails),
}

/// **THE THREE GUARDRAILS A PLUGIN'S OWN DOCUMENT NAMED**, each [`None`] where it named nothing —
/// register item 738, layer 1.
///
/// # ⚠⚠⚠ Why it is three `Option`s and not a [`Guardrails`]
///
/// A [`Guardrails`] is a run's RESOLVED bounds: every field decided, nothing left to fall through.
/// This is the middle step, and *the document said nothing about this one* has to survive it — a
/// zero or a daemon default sitting where a document's silence belongs is exactly how item 492's
/// ceiling read as *authored* on every run while nothing carried it.
///
/// ⚠⚠ **THE COST FIELD IS A [`Cost`] AND NOT A NUMBER**, so a bound cannot be misloaded into the
/// wrong currency on this road either. `parse_max_cost` already refuses that of a caller; a
/// document that named `max_tokens` for a byte-spending plugin is the same mistake and is refused
/// in the same words, by the same publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
struct AuthoredGuardrails {
    max_iterations: Option<u32>,
    max_cost: Option<Cost>,
    max_duration: Option<Duration>,
}

impl AuthoredGuardrails {
    /// What a plugin with no document of its own to read answers — every field the caller's or
    /// this daemon's, which is what every plugin here did before this type existed.
    const fn none() -> Self {
        Self {
            max_iterations: None,
            max_cost: None,
            max_duration: None,
        }
    }
}

impl PluginKind {
    fn as_plugin(&mut self) -> &mut dyn Plugin {
        match self {
            PluginKind::Orchestrator(orchestrator) => orchestrator,
            PluginKind::Pipe(pipe) => pipe,
            PluginKind::Agent(agent) => agent,
            PluginKind::Dialogue(dialogue) => dialogue.as_mut(),
            PluginKind::Answer(answer) => answer,
            PluginKind::AiLoop(loops, _) => loops.as_mut(),
        }
    }

    /// **WHICH NAME THIS BUILT PLUGIN ANSWERS TO** — register items 539 and 597.
    ///
    /// ⚠⚠⚠ The identity a refusal prints, and it comes from the VALUE rather than from the run's
    /// label. A label is prose composed for a reader (`"orchestrator pane=3"`), and register item
    /// 587's finding is that identity re-derived from prose drifts the day somebody rewords it.
    ///
    /// ⚠ Exhaustive: a seventh plugin is named here in the compile that adds it.
    const fn name(&self) -> PluginName {
        match self {
            PluginKind::Orchestrator(_) => PluginName::Orchestrator,
            PluginKind::Pipe(_) => PluginName::Pipe,
            PluginKind::Agent(_) => PluginName::Agent,
            PluginKind::Dialogue(_) => PluginName::Dialogue,
            PluginKind::Answer(_) => PluginName::Answer,
            PluginKind::AiLoop(..) => PluginName::AiLoop,
        }
    }

    /// This plugin's cost UNIT, in which a bare `max_cost` from the wire is sized: the byte-relay
    /// plugins spend injected bytes; the dialogue spends LLM tokens.
    ///
    /// ⚠ The magnitude carried here is this DAEMON's default and no longer decides on its own —
    /// see [`own_bounds`](Self::own_bounds), which is what a plugin whose document names a ceiling
    /// answers with instead.
    fn cost_unit(&self) -> Cost {
        match self {
            PluginKind::Orchestrator(_)
            | PluginKind::Pipe(_)
            | PluginKind::Agent(_)
            // ⚠ Bytes, and the ceiling never binds: the most an answer can spend is two
            // keystrokes. It is here because a run's cost unit is its plugin's, and a plugin with
            // no unit would be a hole in the one guarantee the guardrails make.
            | PluginKind::Answer(_)
            // ⚠ BYTES, and it is the loop's real currency rather than a fallback: what an
            // `ai_loop` spends on its peer is the prompts it types, and the model's tokens are
            // spent by the AGENT in the pane, which this daemon neither bills nor can count. The
            // budget that bounds an agent's spend is `max_turns`, and it is in the brief.
            | PluginKind::AiLoop(..) => Cost::Bytes(DEFAULT_MAX_BYTES),
            PluginKind::Dialogue(_) => Cost::Tokens(DEFAULT_MAX_TOKENS),
        }
    }

    /// **THE BOUNDS THIS PLUGIN'S OWN DOCUMENT NAMES**, before any caller's — register item 738,
    /// layer 1.
    ///
    /// # ⚠⚠⚠⚠⚠ The defect: three ceilings, and a document could reach none of them
    ///
    /// A run is bounded by five things ([`Ceiling`]). Two were the plugin
    /// document's already — `max_turns` and `hold_within_ms`, which the driver does not own. The
    /// other three are [`Guardrails`], and **their only authors were the caller and this daemon's
    /// constants.** Measured in this daemon's own registry on 2026-08-28: of 49 recorded runs, **8
    /// ended `exhausted (cost)`**, every one between 65,809 and 68,658 bytes — the 64 KiB default —
    /// while the largest run that CONVERGED spent 516,020. So for a debt loop the daemon's backstop
    /// is not a backstop, it is the ceiling that bites first, and it bites mid-round with the work
    /// uncommitted.
    ///
    /// ⚠⚠ What stood in for this was a guard chain in an untracked launcher script, refusing any
    /// launch that did not name all three by hand. **A copy of somebody's memory is not a spec**,
    /// and it is the reason the item exists.
    ///
    /// # ⚠⚠⚠ Why every plugin is asked and only one answers
    ///
    /// [`AuthoredGuardrails::none`] is what a plugin with no document to read says, which is what
    /// every one of these did before this method existed. Asking them all is what keeps the
    /// fall-through in ONE place — [`parse_guardrails`], which every form goes through — rather
    /// than putting a loop-shaped branch inside a function that bounds six kinds of run.
    fn own_bounds(&self) -> AuthoredGuardrails {
        match self {
            PluginKind::Orchestrator(_)
            | PluginKind::Pipe(_)
            | PluginKind::Agent(_)
            | PluginKind::Answer(_)
            | PluginKind::Dialogue(_) => AuthoredGuardrails::none(),
            // ⚠ READ AT BUILD TIME AND CARRIED, not read again here: the kind document is opened
            // once per run in `build_plugin`, and re-opening it would be a second read of one
            // author's clause — the thing `LoopKind`'s own doc says a kind must never become.
            PluginKind::AiLoop(_, authored) => *authored,
        }
    }
}

/// A required argv array (`["program", "args"…]`) of strings, non-empty.
/// A missing/non-array value is a [`InvokeError::TypeMismatch`]; an empty array
/// is a [`InvokeError::Rejected`] (an endpoint needs at least its program).
/// Read an optional millisecond duration argument.
///
/// One spelling for the three `*_ms` arguments a run form takes, so a bound named on the wire is
/// converted the same way wherever it is named. A present-but-not-a-number value is a MALFORMED
/// request rather than a silently ignored one — the class R358 closed for argument NAMES, held
/// here for their values.
/// Read the optional `ready_when` barrier — an object naming WHICH QUESTION its marker asks.
///
/// # ⚠⚠ A bare string is REFUSED, deliberately
///
/// This argument was a needle, and the needle was matched against the whole screen — satisfied by
/// text that was already there, most often the ECHO OF THE COMMAND LINE THAT STARTED THE PROGRAM.
/// Reading an old caller's string as either of the two kinds would answer their question with the
/// other one and never say so, which is the silent-reinterpretation failure the wire refuses
/// everywhere else. The shape is what moved, so `WIRE_PROTOCOL` moved with it (21 → 22) and a
/// pre-bump call meets a grammar refusal at the door.
///
/// A word outside [`ReadyWhen::WIRE_WORDS`] is MALFORMED rather than rejected — R353's rule, and
/// what lets a completeness probe SEE that the vocabulary is closed.
fn opt_ready_when(map: &Map<String, Value>) -> Result<Option<ReadyWhen>, InvokeError> {
    if declined(map, "ready_when") {
        return Ok(None);
    }
    let object = map["ready_when"]
        .as_object()
        .ok_or(InvokeError::TypeMismatch)?;
    // ⚠⚠ THE TWO FIELD NAMES ARE THE TYPE'S, NOT THIS READER'S — register item 738. A loop KIND
    // authors a barrier of its own now, so this object is read out of a JSON request AND out of an
    // `.scxml` datamodel; two readers spelling one key by hand is how they come to admit different
    // objects, which is `Consent::ASKED_KEY`'s argument one type over.
    let matched = require_str(object, ReadyWhen::MATCH_KEY)?;
    let marker = require_str(object, ReadyWhen::MARKER_KEY)?.to_string();
    ReadyWhen::parse(matched, marker)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// Read the optional `may_answer` consents — WHAT THIS RUN MAY ANSWER if its peer stops to ask.
/// Absent (or `null`) is a run that answers nothing, which is what every run did before the key
/// existed.
///
/// ⚠⚠ **BOTH NEEDLES ARE REQUIRED AND NEITHER MAY BE EMPTY.** An empty `asked` is carried by every
/// question and an empty `answer` by every option, so each of them turns a narrow consent into
/// something else — see [`Consent::parse`](sprag_plugin::Consent::parse), which owns the predicate
/// so the parser and the publication cannot drift. A caller who sends one has made a MALFORMED
/// request (R353's rule), which is why this is a `TypeMismatch` rather than a friendly refusal.
///
/// # ⚠⚠⚠ A LIST, and an EMPTY one is malformed rather than an omission
///
/// One turn asks more than one question, so the value is an ARRAY of clauses — see
/// [`PluginGrammar::MAY_ANSWER`](crate::wire::PluginGrammar::MAY_ANSWER) for the measurement. The
/// empty array is refused rather than read as *"no consent"*: `[]` and an absent key would then be
/// two spellings of one meaning, and the one that arrives by accident — a client that built its
/// clause list from a filter and matched nothing — is exactly the one a caller would want told
/// about. [`Consents::of`](sprag_plugin::Consents::of) owns that predicate, as `Consent::parse`
/// owns the needle's.
fn opt_may_answer(map: &Map<String, Value>) -> Result<Option<Consents>, InvokeError> {
    if declined(map, Consents::WIRE_KEY) {
        return Ok(None);
    }
    let listed = map[Consents::WIRE_KEY]
        .as_array()
        .ok_or(InvokeError::TypeMismatch)?;
    let mut clauses = Vec::with_capacity(listed.len());
    for clause in listed {
        let object = clause.as_object().ok_or(InvokeError::TypeMismatch)?;
        let asked = require_str(object, Consent::ASKED_KEY)?.to_string();
        let answer = require_str(object, Consent::ANSWER_KEY)?.to_string();
        clauses.push(Consent::parse(asked, answer).ok_or(InvokeError::TypeMismatch)?);
    }
    Consents::of(clauses)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// Read the optional `screen_rules` — WHAT THIS LOOP TURNS DOWN AND WHAT IT SAYS INSTEAD.
///
/// [`opt_may_answer`]'s shape, for the other authority: a consent takes an option the peer OFFERED,
/// and a screen rule refuses the call and redirects the agent in words. Absent (or `null`) is
/// [`None`], which the loop reads as *"keep whatever the document's author wrote"* — NOT as an
/// empty list, which is why [`ScreenRules`] cannot be empty and an empty array is malformed here.
///
/// ⚠⚠ A rule's own refusals are the plugin's ([`sprag_plugin::Malformed`]) and reach the caller as a
/// type mismatch, exactly as a `Consent` with an empty needle does. A rule that claims every dialog
/// would refuse every tool call the agent ever asks about, so the door is where it is turned away.
fn opt_screen_rules(map: &Map<String, Value>) -> Result<Option<ScreenRules>, InvokeError> {
    if declined(map, ScreenRules::WIRE_KEY) {
        return Ok(None);
    }
    let listed = map[ScreenRules::WIRE_KEY]
        .as_array()
        .ok_or(InvokeError::TypeMismatch)?;
    let mut rules = Vec::with_capacity(listed.len());
    for rule in listed {
        let object = rule.as_object().ok_or(InvokeError::TypeMismatch)?;
        let when = require_str(object, ScreenRule::WHEN_KEY)?.to_string();
        let text = require_str(object, ScreenRule::TEXT_KEY)?.to_string();
        rules.push(ScreenRule::parse(when, text).map_err(|_| InvokeError::TypeMismatch)?);
    }
    ScreenRules::of(rules)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// Read the optional `await_person_ms` — WHETHER ANYBODY IS WATCHING the pane this run drives, and
/// for how long. Absent (or `null`) is [`Attended::NoOne`], which is what every run did before the
/// key existed and is the conservative half of the contract.
///
/// ⚠⚠ **ZERO IS MALFORMED, not a quiet `NoOne`** — [`opt_may_answer`]'s empty-array rule exactly,
/// and for the same reason: two spellings of one behaviour make the caller who reached the first by
/// arithmetic (a deadline already past, a config that defaulted to 0) silently get the other.
/// [`Attended::of`] owns the predicate, so the parser and the type cannot drift.
fn opt_attended(map: &Map<String, Value>) -> Result<Attended, InvokeError> {
    let handback = opt_handback(map)?;
    let Some(patience) = opt_millis(map, Attended::WIRE_KEY)? else {
        // ⚠⚠⚠ A HANDBACK WITH NOBODY WATCHING IS MALFORMED, NOT A QUIET `NoOne`. The pair is one
        // request — *"a person is at this pane, and here is when a pane they take comes back"* —
        // and the caller who sends only the second half has plainly asked for a run that waits.
        // Answering `NoOne` would give them a run that ENDS on the first keystroke, which is the
        // opposite, and the type they are addressing cannot even express what they sent
        // ([`Handback`] lives inside [`Attended::APerson`]). So they are told.
        return if handback == Handback::Never {
            Ok(Attended::NoOne)
        } else {
            Err(InvokeError::TypeMismatch)
        };
    };
    Attended::of(patience, handback).ok_or(InvokeError::TypeMismatch)
}

/// Read the optional `handback_still_ms` — WHEN A PANE THIS RUN'S PERSON TAKES BECOMES THIS RUN'S
/// AGAIN. Absent (or `null`) is [`Handback::Never`]: the run ends when somebody takes the pane,
/// which is what every run did before the key existed and is the conservative half.
///
/// ⚠⚠ **ZERO IS MALFORMED**, [`opt_attended`]'s rule and [`Handback::of`]'s predicate: *"the pane is
/// mine again the instant they pause"* is not something a caller can mean, since every person pauses
/// between keystrokes, and one who reached zero by arithmetic would get a run that typed into the
/// gap between their words.
/// Read the optional `hold_within_ms` — HOW LONG SOMEBODY MAY HOLD THIS RUN before it ends as
/// abandoned. Absent (or `null`) is [`None`]: the loop document's own ceiling stands, which is what
/// *"omitting a duration key means the document decides"* means everywhere else on this form.
///
/// ⚠⚠⚠ **IT IS NOT PART OF THE `await_person_ms` / `handback_still_ms` PAIR, AND THAT IS REGISTER
/// ITEM 534's WHOLE POINT.** Those two are one request about a person who is EXPECTED, and
/// [`Handback`] living inside [`Attended::APerson`] is what enforces it. A hold is an order, and a
/// run nobody is watching can be given one — which is exactly the population that used to park for
/// ever, so a ceiling read through that contract would have been unreachable where it was needed.
/// It is therefore read alone, and sending it without either of the others is well-formed.
///
/// ⚠⚠ **ZERO IS MALFORMED**, [`opt_attended`]'s rule: *"hold this run and end it at once"* is
/// `cancel` spelled wrong, so the two would be two spellings of one behaviour — and the caller who
/// reached zero by arithmetic is the one who has to be told. There is deliberately no spelling for
/// *"no ceiling"*: an unbounded hold is the defect this key closes, not a configuration.
fn opt_hold_within(map: &Map<String, Value>) -> Result<Option<Duration>, InvokeError> {
    let Some(within) = opt_millis(map, sprag_plugin::HOLD_WITHIN_KEY)? else {
        return Ok(None);
    };
    if within.is_zero() {
        return Err(InvokeError::TypeMismatch);
    }
    Ok(Some(within))
}

fn opt_handback(map: &Map<String, Value>) -> Result<Handback, InvokeError> {
    let Some(still) = opt_millis(map, Handback::WIRE_KEY)? else {
        return Ok(Handback::Never);
    };
    Handback::of(still).ok_or(InvokeError::TypeMismatch)
}

/// Read the LOOPING forms' optional turn contract — WHAT MAKES THE PEER'S TURN OVER AND HOW LONG IT
/// MAY TAKE. Absent is `None`: the step ends on the plugin's own 500 ms constant, which is what
/// every run did before the pair existed.
///
/// # ⚠⚠⚠ The pair is ONE request, and half of it is malformed
///
/// [`opt_attended`]'s rule exactly, for the same reason one door over. `done_when` with no
/// `turn_within_ms` is a caller who said *"my peer finishes like this"* and left the run with no
/// idea how long to allow — and the type they are addressing cannot express it ([`Turn`] holds
/// both). `turn_within_ms` with no `done_when` is a bound on a contract that does not exist, which
/// would silently become *"wait this long, then type again anyway"* — a different behaviour from
/// the one they asked for, in the direction of doing more.
///
/// ⚠ Answering a quiet `None` to either half would give the caller the 500 ms timer they were
/// plainly trying to get away from, so they are told instead.
///
/// ⚠⚠ **ZERO IS MALFORMED**, [`Turn::lasting`]'s predicate: *"wait no time at all for my peer to
/// finish"* is not something a caller can mean.
fn opt_turn(map: &Map<String, Value>) -> Result<Option<Turn>, InvokeError> {
    let within = opt_millis(map, Turn::WIRE_KEY)?;
    let Some(when) = opt_done_when(map)? else {
        // A bound with nothing to bound.
        return if within.is_some() {
            Err(InvokeError::TypeMismatch)
        } else {
            Ok(None)
        };
    };
    Turn::lasting(when, within)
        .map(Some)
        .ok_or(InvokeError::TypeMismatch)
}

/// Read the `ai_loop` form's optional `turn_within_ms` — HOW LONG ONE OF THE INNER AGENT'S TURNS
/// MAY TAKE, as a number for the document to hold. Absent is [`None`]: **the document decides**.
///
/// # ⚠⚠⚠ Why this exists beside [`opt_turn`] instead of calling it
///
/// A loop no longer builds a [`Turn`] at all — the bound is `ai_loop.scxml`'s since register item
/// 300, and only `done_when` is left on its spec — so the pairing [`opt_turn`] enforces cannot
/// apply here. It never did: an `agent` run's default contract is `exits`, so a bare bound would
/// bound something the caller did not choose, where a loop's default is [`INNER_SESSION_ENDS`] and
/// a bare bound bounds exactly the turn the caller is thinking about.
///
/// # ⚠⚠⚠ What it keeps, and what would have gone silently wrong without it
///
/// **ZERO IS STILL MALFORMED, and [`Turn::lasting`] is still who says so.** This form used to build
/// a `Turn` and hand the refusal straight back; with the bound moved to the document, a zero would
/// have flowed into `<data>` and been read there as *the author declines a bound* — turning a
/// request the wire REFUSED into a run, which is the direction R385 registered as earning a
/// protocol bump. The type is asked rather than the rule re-typed, so there is still one owner of
/// *"wait no time at all for my peer to finish is not a thing a caller can mean"*.
///
/// [`INNER_SESSION_ENDS`]: sprag_plugin::INNER_SESSION_ENDS
fn opt_ai_loop_turn_ms(map: &Map<String, Value>) -> Result<Option<i64>, InvokeError> {
    let Some(within) = opt_millis(map, Turn::WIRE_KEY)? else {
        return Ok(None);
    };
    Turn::lasting(sprag_plugin::INNER_SESSION_ENDS, Some(within))
        .ok_or(InvokeError::TypeMismatch)?;
    Ok(Some(within.as_millis() as i64))
}

/// Parse the `agent` form's optional `done_when` — WHAT MAKES THE TURN OVER. Absent (or `null`)
/// leaves the spec's default, which is [`DoneWhen::Exits`] and is what this adapter did
/// unconditionally before the argument existed.
///
/// ⚠ A BARE WORD, read through the type's own [`DoneWhen::parse`], so the set this accepts and the
/// set the wire publishes are one list. The first draft took an object with a companion `agent`
/// and two conformance gates refused it — see [`PluginGrammar::DONE_WHEN`](crate::wire::PluginGrammar::DONE_WHEN).
fn opt_done_when(map: &Map<String, Value>) -> Result<Option<DoneWhen>, InvokeError> {
    let Some(word) = opt_str(map, "done_when")? else {
        return Ok(None);
    };
    DoneWhen::parse(word)
        .ok_or(InvokeError::TypeMismatch)
        .map(Some)
}

/// A required COUNT — `max_turns` and its kind.
///
/// ⚠ `i64` because that is what a script datamodel holds and what
/// [`Brief`] carries; reading it as a `u32` here and widening would put a
/// second opinion about the range between the caller and the document that enforces it. A negative
/// or absurd number is refused by the loop's own door, which is where the reason lives.
fn require_count(map: &Map<String, Value>, key: &str) -> Result<i64, InvokeError> {
    map.get(key)
        .and_then(Value::as_i64)
        .ok_or(InvokeError::TypeMismatch)
}

/// The same count, optional — absent (or `null`) is [`None`].
fn opt_count(map: &Map<String, Value>, key: &str) -> Result<Option<i64>, InvokeError> {
    if declined(map, key) {
        return Ok(None);
    }
    require_count(map, key).map(Some)
}

/// **WHAT A LOOP RUN IS FOR, RESOLVED FROM THE CALLER'S REQUEST AND THIS REPOSITORY'S KIND** —
/// every judgement a `Brief` carries, in the one place both roads to it meet.
///
/// # ⚠⚠⚠⚠⚠ Why this is a function and not the inline block it was until register item 492
///
/// Eight of a brief's fields fall back to the kind document, and **not one of those fall-throughs
/// was held by anything.** The residue was registered rather than hidden — `sprag_plugin`'s
/// `a_declined_budget_crosses_as_a_word_and_the_run_is_not_refused` says it in its own doc:
/// *"deleting `.or_else(|| kind.turn_budget())` from `plugins.rs` leaves the entire workspace
/// GREEN. What would catch it is an observable of the RESOLVED budget on a run started through the
/// wire, and `turn_budget` is crate-private"* — and it was measured again on item 492's round, for
/// the ceiling, with the same answer.
///
/// A `Brief` is that observable. It is `pub` in `sprag_plugin`, it is exactly what the door
/// resolves, and handing it back instead of consuming it in place is the whole difference between a
/// wiring nothing checks and one a gate can read. ⚠⚠ **It is not a gate re-implementing the line it
/// checks**: this IS the line, and the test asks the real function what a real request plus the
/// real kind document resolve to.
///
/// ⚠ The engine and the pane stay with the caller: this resolves JUDGEMENTS, and which pane a run
/// drives is a binding.
///
/// # Errors
///
/// [`InvokeError::TypeMismatch`] for a malformed argument, and [`refused`]'s sentence when this
/// repository's own kind document holds a list this driver cannot read.
/// **WHAT MAKES THIS RUN'S PANE READY, RESOLVED IN THREE STEPS** — register item 738, layer 3, and
/// the ORDER is the whole of it.
///
/// A loop's first prompt goes into a pane whose program may still be starting, and R379 measured
/// what no barrier costs: a prompt typed into a pane whose agent had existed for ten milliseconds,
/// the pseudoterminal's own echo confirming the delivery, and the run then sitting in `working` for
/// as long as anybody let it. **So there must always be one** — the question is only who says it.
///
/// 1. **What the caller SPELLED** (`ready_when`) wins over everything. It is the most specific
///    thing anybody said about this pane.
/// 2. **What the caller IMPLIED by naming a program** (`agent`), derived exactly as
///    [`AiLoopSpec::driving`](sprag_plugin::AiLoopSpec::driving) always derived it. Item 300's line
///    is untouched — the barrier is still READ OFF WHICH PROGRAM IS IN THE PANE — so a run driving
///    `codex` still gets `codex`.
/// 3. **What this repository's KIND document authors**, for a launch that named neither. That is
///    the layer this item added, and it is why `agent` could stop being required: the key's own
///    grammar note said it was mandatory *because there is no honest default*, and a document that
///    names its peer is one.
///
/// # ⚠⚠⚠⚠⚠ Why this hands the barrier BACK instead of setting it in place
///
/// [`ai_loop_brief`]'s reason exactly, and it is this workspace's own recorded finding: a
/// resolution consumed where it is computed is a wiring nothing can check, and **deleting a
/// fall-through left the whole workspace green** (measured twice, items 312 and 492). The resolved
/// value is the observable. ⚠ It is not a gate re-implementing the line it checks — this IS the
/// line, and a gate asks it what a real request plus the real kind document resolve to.
///
/// # Errors
///
/// [`InvokeError::TypeMismatch`] for a malformed `ready_when` or an empty `agent`, and [`refused`]'s
/// sentence when nothing at all names a barrier — which is a refusal rather than a `None`, because
/// [`AiLoopSpec::ready_when`](sprag_plugin::AiLoopSpec::ready_when) CAN hold nothing and nothing
/// means *go ahead immediately*, which is R379's failure bought back in silence.
/// **THE GUARDRAILS THIS REPOSITORY'S KIND DOCUMENT NAMES** — register item 738, layer 1.
///
/// # ⚠⚠⚠⚠⚠ The keys are the WIRE's, checked against the publication rather than a list here
///
/// `sprag-plugin` hands back the clause as a map of name → number and reads no meaning into it
/// ([`LoopKind::authored_numbers`](sprag_plugin::kind::LoopKind::authored_numbers)), because the
/// field names belong to [`PluginGrammar::guardrail_fields`](crate::wire::PluginGrammar::guardrail_fields)
/// and that crate cannot see them. **This is where the vocabulary lives, so this is where the
/// clause is judged** — and a fourth guardrail added to the publication is admitted here in the
/// same compile rather than silently dropped.
///
/// ⛔⛔ **A KEY NO GUARDRAIL ADMITS IS REFUSED, NAMING WHAT THE OBJECT TAKES.** That is
/// [`parse_guardrails`]'s own rule turned on a document, and its reason is unchanged: ignoring an
/// ordinary argument makes a verb do less than asked and the caller can see it; **ignoring a BOUND
/// makes the run do more, without limit, and answers success.** A document that spelled
/// `max_byte` would otherwise get the daemon's 64 KiB while plainly naming two megabytes.
///
/// ⚠ The cost key is unit-dependent — `max_bytes` for a byte-spending plugin, `max_tokens` for a
/// token-spending one — so it is not spelled here either: whichever the publication offers for
/// this unit is the one a document may name, and naming the other one is an unknown key.
///
/// # Errors
///
/// [`refused`]'s sentence for a clause this driver cannot read, and for a key no guardrail admits.
fn kind_guardrails(
    kind: &sprag_plugin::kind::LoopKind,
    unit: Cost,
) -> Result<AuthoredGuardrails, InvokeError> {
    let Some(named) = kind.authored_numbers("guardrails").map_err(|why| {
        refused(format!(
            "this repository's loop-kind document holds a `guardrails` clause this driver cannot \
             read ({why:?}); it must be an object of whole numbers, and a run cannot start on a \
             bound nobody can check"
        ))
    })?
    else {
        return Ok(AuthoredGuardrails::none());
    };
    let declared = crate::wire::PluginGrammar::guardrail_fields(unit.unit());
    if let Some(unknown) = named
        .keys()
        .find(|key| !declared.iter().any(|field| field.name == key.as_str()))
    {
        return Err(refused(format!(
            "this repository's loop-kind document names {unknown:?} in its `guardrails` clause, \
             and that is not a guardrail of a run that spends {}. It takes: {}. A bound this \
             daemon does not know would have been ignored, and an ignored bound is not a bound — \
             the run would take this daemon's own default while the document plainly named \
             something else.",
            unit.unit(),
            declared
                .iter()
                .map(|field| field.name)
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }
    // ⚠ A NEGATIVE OR OUT-OF-RANGE NUMBER IS REFUSED rather than clamped, on the same terms: a
    // document that arrived at one by arithmetic is the author who needs telling, and a clamp
    // would run under a bound nobody wrote.
    let out_of_range = |name: &str| {
        refused(format!(
            "this repository's loop-kind document names a `{name}` its runs cannot be bounded by; \
             a guardrail is a whole number of at least one"
        ))
    };
    let max_iterations = match named.get("max_iterations") {
        Some(&held) => Some(
            u32::try_from(held)
                .ok()
                .filter(|it| *it > 0)
                .ok_or_else(|| out_of_range("max_iterations"))?,
        ),
        None => None,
    };
    let max_duration = match named.get("max_seconds") {
        Some(&held) => Some(Duration::from_secs(
            u64::try_from(held)
                .ok()
                .filter(|it| *it > 0)
                .ok_or_else(|| out_of_range("max_seconds"))?,
        )),
        None => None,
    };
    // ⚠⚠ THE COST KEY IS THE UNIT'S OWN, so the same clause read for a token-spending plugin looks
    // for `max_tokens` — and `Cost::sized` is what keeps the currency from being crossed.
    let cost_key = unit.bound_key();
    let max_cost = match named.get(cost_key) {
        Some(&held) => Some(
            unit.sized(
                u64::try_from(held)
                    .ok()
                    .filter(|it| *it > 0)
                    .ok_or_else(|| out_of_range(cost_key))?,
            ),
        ),
        None => None,
    };
    Ok(AuthoredGuardrails {
        max_iterations,
        max_cost,
        max_duration,
    })
}

fn ai_loop_barrier(
    map: &Map<String, Value>,
    authored: Option<sprag_plugin::ReadyWhen>,
) -> Result<sprag_plugin::ReadyWhen, InvokeError> {
    if let Some(spelled) = opt_ready_when(map)? {
        return Ok(spelled);
    }
    match opt_str(map, "agent")? {
        Some(named) if !named.is_empty() => Ok(sprag_plugin::ReadyWhen::Settles(named.to_string())),
        // ⚠ An EMPTY agent is malformed rather than absent, on `reference`'s own rule at this door:
        // `""` is not a caller deferring, and reading it as absence would let a launcher's bug
        // silently acquire this repository's default barrier.
        Some(_) => Err(InvokeError::TypeMismatch),
        None => authored.ok_or_else(|| {
            refused(
                "this run named neither `agent` nor `ready_when`, and this repository's loop-kind \
                 document authors no barrier either — so nothing says when this pane is ready to \
                 be typed into. Name the program in the pane (`agent`), spell the barrier \
                 (`ready_when`), or author one in the kind document: a loop with no barrier types \
                 its first prompt into whatever the pane happens to be running",
            )
        }),
    }
}

/// **THE BARRIER THIS REPOSITORY'S KIND DOCUMENT NAMES**, read and turned into a refusal a caller
/// can act on — the half of [`ai_loop_barrier`] that knows what a kind is.
///
/// ⚠⚠ Split off for register item 739's reason: reading a document and deciding precedence are two
/// jobs, and while they were one function the *nobody named a barrier* arm could not be reached by
/// any gate — this repository's document always names one. See [`ai_loop_reference`], which carries
/// the whole argument.
///
/// # Errors
///
/// [`refused`]'s sentence when the document holds a barrier this driver cannot carry out.
fn kind_barrier(
    kind: &sprag_plugin::kind::LoopKind,
) -> Result<Option<sprag_plugin::ReadyWhen>, InvokeError> {
    kind.ready_when().map_err(|why| {
        refused(format!(
            "this repository's loop-kind document holds a readiness barrier this driver cannot \
             read ({why:?}); a run cannot start on a barrier nobody can check"
        ))
    })
}

/// **A RUN DOES NOT START ON A PANE STANDING SOMEWHERE ITS KIND DOES NOT WORK** — register item
/// 738, layer 4, and item 684's remaining half.
///
/// # ⛔⛔⛔⛔⛔ The measurement this exists for
///
/// A pane opened with no directory stands in `$HOME`, and an agent starting there asks *"Is this a
/// project you created or one you trust?"*. That dialog is not in this loop's consents — they cover
/// editing and running commands — so nothing answers it and the run waits for a person. Measured
/// 2026-08-25 on a live daemon: `inner-wz` `blocked rule=dialog-choice-list` in `/home/coin`, while
/// three sibling panes standing in their own repositories were `working`, all from one restart.
///
/// ⛔ **THE OBVIOUS REPAIR AUTOMATES A FALSE ANSWER.** *Yes, I trust this folder* consents to
/// `/home/coin` — every repository on the machine — while the true fact is narrower and duller:
/// this pane is not in the tree the run is about. Saying the true thing is what this does.
///
/// # ⚠⚠⚠ Why it REFUSES rather than reporting
///
/// The door is the last moment anybody is still watching. Item 684's whole cost was a run that had
/// already started and had nobody left to tell — so the sentence has to arrive where the caller is,
/// and it names BOTH directories because the remedy (reopen the pane pointed at the tree) is not
/// guessable from either one alone.
///
/// # ⛔ And a pane that cannot say where it was opened is a RED, not a pass
///
/// This repository's own rule (`a-guard-that-cannot-read-a-pid-must-not-wave-it-through`): a check
/// that cannot read its subject must not vouch for it. A surface with no such answer cannot host a
/// kind that names a tree, and saying so is cheaper than the run that parks.
///
/// ⚠ **`works_in` of [`None`] IS NO CHECK AT ALL**, which is the shipped state for a kind that says
/// nothing and the right default for a document other repositories copy. That is an absence of a
/// claim rather than an exemption from one — nothing is classified as *fine*, because nothing was
/// claimed.
///
/// ⚠⚠ It takes the two ANSWERS and not the world and the kind, on register item 739's finding: a
/// resolution that reaches for its own inputs has arms no gate can enter, and *nobody said* is a
/// value here rather than a document nobody can write.
///
/// # Errors
///
/// [`refused`]'s sentence when the pane stands somewhere else, and when the kind names a tree the
/// surface cannot check the pane against.
fn ai_loop_stands_where_it_works(
    stands_in: Option<&std::path::Path>,
    works_in: Option<&str>,
) -> Result<(), InvokeError> {
    let Some(marker) = works_in else {
        return Ok(());
    };
    let Some(standing) = stands_in else {
        return Err(refused(format!(
            "this repository's loop-kind document says its runs work in a directory carrying \
             {marker:?}, and this surface cannot say where the pane was opened — so nothing here \
             can tell a pane standing in a tree from one that will meet a `do you trust this \
             folder?` dialog no run of this kind may answer. A check that cannot read its subject \
             does not vouch for it."
        )));
    };
    // ⚠ `exists` AND NOT `is_dir`: a linked worktree carries `.git` as a FILE, and a check that
    // demanded a directory would refuse a tree that is perfectly real.
    if standing.join(marker).exists() {
        return Ok(());
    }
    Err(refused(format!(
        "this pane was opened in {standing:?}, which carries no {marker:?} — and this repository's \
         loop-kind document says its runs work in a directory that does. An agent started outside \
         a tree asks whether the folder is trusted, and that dialog is not one this loop's \
         consents cover, so the run would stop at it and wait for somebody who is not watching. \
         Open the pane pointed at the tree, or change what the kind document says marks one."
    )))
}

/// **WHERE A RUN STARTS READING, AND WHO SAYS SO** — register item 738, layer 2.
///
/// This was `require_str`, so the kind document could not answer it: a launch that named no
/// reference was MALFORMED rather than deferring, which is item 312's own finding about `max_turns`
/// at a string instead of a count. **A required judgement is a decision the document is
/// structurally forbidden from making**, and what filled the gap was a person retyping the ledger's
/// path into every launch out of a memory that dies with the session.
///
/// ⛔ **THE FALL-THROUGH STOPS AT `authored`.** The template ships `'(edit me) paths, URLs or repos
/// to consult'` and R380 measured that placeholder reaching a live agent, so a run with neither is
/// REFUSED naming the key rather than briefing an agent with an instruction to edit a file.
///
/// ⚠ A PRESENT-BUT-EMPTY VALUE IS STILL MALFORMED, which is what `require_str` answered before and
/// must go on answering: `""` is not a caller deferring, it is a caller sending a reference that
/// says nothing, and reading it as absence would let a bug in a launcher silently acquire this
/// repository's default.
///
/// # ⚠⚠⚠⚠⚠ Why it takes the KIND'S ANSWER and not the kind — register item 739
///
/// There is one loop kind and its constructor always opens the real `debt_loop.scxml`, so a
/// function reading `LoopKind` directly has a refusal arm **nothing can reach**: the document
/// always names a reference, and a green gate then means *the refusal was never run* rather than
/// *the refusal is right*. That is items 706 and 482's shape, and the only witness this arm had was
/// a mutation that happened to trip it.
///
/// Taking the ANSWER fixes it and is better factoring on its own terms: **precedence is not the
/// business of whoever knows what a kind is.** What this function decides is *the caller's, else
/// the author's, else refuse* — a statement with no `LoopKind` in it. ⚠ The other half of the claim
/// stays where it was: that `ai_loop_brief` passes THIS document's answer and not something else is
/// held by `a_kind_documents_judgements_reach_a_run_that_named_none_of_them`, and neither gate is
/// the whole claim.
///
/// # Errors
///
/// [`InvokeError::TypeMismatch`] for a present-but-empty value, and [`refused`]'s sentence when
/// neither the caller nor the author names one.
fn ai_loop_reference(
    map: &Map<String, Value>,
    authored: Option<String>,
) -> Result<String, InvokeError> {
    match opt_str(map, "reference")? {
        Some(named) if !named.is_empty() => Ok(named.to_string()),
        Some(_) => Err(InvokeError::TypeMismatch),
        None => authored.ok_or_else(|| {
            refused(
                "this run named no `reference` and this repository's loop-kind document authors \
                 none, so nothing says where its first session should start reading. Name one, or \
                 author it in the kind document — the loop template's own value is the placeholder \
                 `(edit me) paths, URLs or repos to consult`, and briefing an agent with that is \
                 worse than not starting",
            )
        }),
    }
}

fn ai_loop_brief(
    map: &Map<String, Value>,
    kind: &sprag_plugin::kind::LoopKind,
) -> Result<Brief, InvokeError> {
    // ⚠⚠⚠⚠ DECLINABLE SINCE ITEM 312 — see `AI_LOOP_FORM`. A caller who names no budget is
    // deferring to `ai_loop.scxml`'s own, which is resolved where the document can be read
    // (`OuterLoop::brief`) and not here, because this door has no datamodel.
    let max_turns = opt_count(map, "max_turns")?;
    let kind_consents = kind.consents().map_err(|why| {
        refused(format!(
            "this repository's loop-kind document holds a consent list this driver \
                         cannot read ({why:?}); a run cannot start on decisions nobody can check"
        ))
    })?;
    let kind_rules = kind.screen_rules().map_err(|why| {
        refused(format!(
            "this repository's loop-kind document holds a rule list this driver cannot \
                         read ({why:?}); a run cannot start on decisions nobody can check"
        ))
    })?;
    let reference = ai_loop_reference(map, kind.reference())?;
    Ok(Brief {
        // ⚠⚠ NO WIRE KEY, DELIBERATELY. What a repository asks its own runs at the end
        // is its document's business; a caller that could override it could delete the
        // sweep this repository's record says it pays for twice over when it is missing.
        closing_rules: kind.closing_rules(),
        // ⚠⚠⚠⚠⚠ NO WIRE KEY EITHER — register item 738, on the line above's own argument. What a
        // repository holds every turn of its own runs to is its document's business, and a caller
        // who could override it could delete it by naming nothing. ⚠ The measurement that made this
        // a defect: this repository's supervisor typed the rules into `north_star` BY HAND on every
        // launch, out of a session's context, and when the session ended they existed nowhere.
        working_rules: kind.working_rules(),
        // ⛔⛔⛔⛔⛔ AND WHAT THIS REPOSITORY DOES ABOUT ITS CHECKER'S SILENCE — register
        // item 741, on the line above's terms and with a refusal of its own.
        //
        // NO WIRE KEY, for `working_rules`' reason: a launch cannot author what a
        // repository does about ITS checker, and one that could would delete it by
        // naming nothing. ⚠ The refusal is the kind's — a document that filled one
        // clause and left the other empty answers some of its runs and is silent about
        // the rest, and `LoopKind::unverified_rules` names the empty one rather than
        // shipping half a decision.
        unverified_rules: kind.unverified_rules().map_err(|why| {
            refused(format!(
                "this repository's loop-kind document authors only half of what a run owes when \
                 its checker says nothing readable ({why:?}); a silence is either a checker that \
                 produced no verdict or one that answered something that is not a verdict, a run \
                 meets exactly one of the two, and a document that answers one of them leaves the \
                 other's runs with nothing to act on"
            ))
        })?,
        // ⚠⚠⚠ NO WIRE KEY EITHER, and for the same reason one line up — register item
        // 428. What certifies this repository's work is its document's business; a
        // caller who could name the checker could delete it by naming nothing, which is
        // the self-certification the whole item is about.
        milestone_check: kind.milestone_check(),
        // ⚠⚠⚠ NO WIRE KEY EITHER, on the two lines above's terms. What this
        // repository's peer prints when its SERVICE fails is its document's business,
        // and a caller who could name the needle could delete the wait by naming
        // nothing — turning a ten-minute outage back into the dead run that paid for
        // this. See `ServiceOutage`, whose doc carries the measurement.
        service: kind.service_outage(),
        north_star: require_str(map, "north_star")?.to_string(),
        milestone: require_str(map, "milestone")?.to_string(),
        reference,
        // ⚠⚠⚠ ABSENT MEANS "WHAT THIS REPOSITORY'S KIND DOCUMENT SAYS", and only then
        // the template's own number. A debt run ends on its work rather than on a turn
        // count, and that decision is the kind's to make — it reaches here as
        // `Counted::Never` rather than as a number nobody could write.
        max_turns: max_turns
            .map(sprag_plugin::Counted::Of)
            .or_else(|| kind.turn_budget()),
        // ⚠⚠ ABSENT STILL MEANS "NEVER, ON THE BUDGET", spelled as the one number that
        // makes the budget guard unreachable rather than as a magic zero: `judging`
        // tests `turns >= max_turns` BEFORE `turns_since_reflect >= reflect_every`, so
        // an equal pair exhausts first.
        //
        // ⚠⚠⚠⚠ BUT THE `unwrap_or(max_turns)` THAT SAID SO IS NO LONGER HERE — item 312.
        // The default IS the budget, and the budget may now be the document's, which
        // this door cannot read. So both resolve together in `OuterLoop::brief`, where
        // the datamodel is; carrying `None` through is what lets them.
        //
        // ⚠⚠⚠ IT IS NO LONGER A REFUSAL TO NAME A SMALLER ONE — `reflecting` and the
        // session-replace lifecycle behind it are built. The default is kept as it was
        // ON PURPOSE rather than moved to the document's `8`: a restart closes a pane a
        // person may be reading and opens another, and a caller who has said nothing
        // about reflection has not asked for that. What they DO get without asking is a
        // reflection when a standing instruction fires, which is the correctness edge
        // (item 148) and not a budget — `screened > screened_carried` is not spelled
        // here because no caller sets it.
        // ⚠⚠⚠ AND THE KIND ANSWERS THIS TOO, which it MUST when it declines the budget:
        // the template's default for reflection is *the number that makes the reflect
        // guard unreachable*, and that number only exists while there is a budget to
        // borrow it from. `OuterLoop::brief` refuses the pair rather than guessing.
        reflect_every: opt_count(map, "reflect_every")?.or_else(|| kind.reflect_every()),
        // ⚠⚠⚠⚠⚠ REGISTER ITEM 492, and the same three-step fall-through its two
        // neighbours have: the caller's number, then THIS repository's kind document,
        // then the template's own — resolved in `OuterLoop::brief`, which is the only
        // place that can read the last of those.
        //
        // ⚠⚠ Until this line the ceiling had NO road at all. The template's comment
        // said it was the kind's to author while no kind could; item 477 measured what
        // that cost at the far end, where `reviewing` took the fall-back eight times
        // out of eight because the number was 0 on every run ever driven.
        context_ceiling: opt_count(map, "context_ceiling")?.or_else(|| kind.context_ceiling()),
        // ⚠⚠⚠⚠⚠ REGISTER ITEM 494 — the line above's TWIN, and the reason it is a
        // separate item rather than a detail of 492: the template says the number is
        // the kind's to author about exactly TWO of its `<data>`, 492 measured the
        // instance, and the identical defect was still standing one of them up. **A
        // premise that produces one defect produces the rest of its class**, and the
        // ratchet in `sprag-gate`'s `authored` module is what closes the class.
        //
        // ⚠⚠ Same three-step fall-through as its three neighbours: the caller's
        // number, then THIS repository's kind document, then the template's own —
        // resolved in `OuterLoop::brief`, the only place that can read the last.
        reflect_after_refusals: opt_count(map, "reflect_after_refusals")?
            .or_else(|| kind.reflect_after_refusals()),
        // ⚠⚠ ABSENT MEANS "WHAT THE DOCUMENT'S AUTHOR WROTE", not *"screen nothing"*.
        // The rules live in the loop template, so a caller who says nothing about
        // screening is not overriding it — and the driver echoes the document's own
        // rules back through the brief rather than deleting them.
        // ⚠⚠ ABSENT MEANS "WHAT THIS REPOSITORY'S KIND DOCUMENT SAYS", and it used to
        // mean *"what the template's author wrote"*. The template no longer writes any
        // — a standing instruction there is answered on behalf of every repository that
        // copies it — so the fallback moved with the values. A caller who says nothing
        // about screening is still not overriding anything.
        screen_rules: opt_screen_rules(map)?.or(kind_rules),
        may_answer: opt_may_answer(map)?.or(kind_consents),
        // ⚠⚠⚠ THE SAME TWO KEYS, NOW WRITTEN INTO THE DOCUMENT instead of into the
        // spec. `awaiting_human`'s only run-ending exit is *nobody came within the
        // patience*, so the patience is the loop DOCUMENT's own data — the argument
        // `Brief::screen_rules` already makes, applied to the other half of one state.
        //
        // ⚠⚠ THE PAIR IS STILL VALIDATED AS A PAIR. `opt_attended` owns *a call that
        // sends the stillness alone is malformed*, and reading the two keys separately
        // here would have quietly dropped that refusal; the values are taken back OUT
        // of what it built rather than parsed a second time.
        //
        // ⚠⚠⚠ AND OMITTING THEM NOW MEANS *THE DOCUMENT DECIDES*, where it used to mean
        // `Attended::NoOne` — a run that ended at the first dialog it could not answer.
        // That is the change, stated: a caller who wants that says so by authoring it,
        // and the shipped document's own number is what an unspecified run now gets.
        await_person_ms: opt_attended(map)?
            .patience()
            .map(|patience| patience.as_millis() as i64),
        handback_still_ms: opt_attended(map)?
            .handback()
            .stillness()
            .map(|still| still.as_millis() as i64),
        // ⚠⚠⚠⚠ AND HOW LONG A HOLD MAY LAST — register item 534, and it is read on its OWN rather
        // than through `opt_attended` above, which is the whole shape of the item. Those two keys
        // are one request about somebody EXPECTED, and this is a bound on an order a run nobody is
        // watching can also be given: item 534's entire population is the unattended runs, the ones
        // that parked for ever, so routing it through the *is anybody watching* contract would have
        // put the ceiling exactly where it could not reach.
        //
        // ⚠⚠⚠ ZERO IS MALFORMED, on `await_person_ms`'s own rule and refused by the same reader:
        // *hold this run and end it at once* is `cancel` spelled wrong, and a caller who reached
        // zero by arithmetic gets told rather than obeyed. ⚠ Absent means THE DOCUMENT DECIDES,
        // like the two keys above and unlike their pre-item-300 selves.
        // ⚠⚠⚠⚠⚠ AND IT REACHES THE KIND DOCUMENT SINCE ITEM 738, on the same three-step chain as
        // every judgement above it: the caller's number, then THIS repository's kind, then the
        // template's own four hours. It arrived because a GATE asked — `Ceiling::ALL` names five
        // things that end a run, the item's gate walks that set with no exemption arm, and this
        // was the one ceiling a kind still could not author. ⚠ Zero is still malformed at
        // `opt_hold_within`, which is the caller's rule and not a fall-through.
        hold_within_ms: opt_hold_within(map)?
            .map(|held| held.as_millis() as i64)
            .or_else(|| kind.hold_within_ms()),
        // ⚠⚠⚠ AND THE LAST TWO JUDGEMENTS, ON THE SAME ROUTE. Each of them arrived
        // paired with a PREDICATE — `ready_timeout_ms` with `ready_when`,
        // `turn_within_ms` with `done_when` — and register item 300 measured that the
        // pair is one fact plus one decision: what makes a pane ready and how a program
        // signals a turn is over are read off WHICH PROGRAM is in the pane; three
        // minutes and half an hour are read off nobody. **A wire pairing is not evidence
        // of a shared owner.** The predicates stay on the spec below; these two write
        // `<data>`.
        //
        // ⚠⚠ THE WIRE FORM IS UNCHANGED — both keys are still accepted, still optional,
        // still milliseconds. What changed is where the number lands, and what OMITTING
        // one means: it used to be the substrate's default, and it is now *the document
        // decides*, which is `await_person_ms`'s change one round earlier.
        ready_timeout_ms: opt_millis(map, Readiness::WIRE_KEY)?
            .map(|within| within.as_millis() as i64),
        turn_within_ms: opt_ai_loop_turn_ms(map)?,
    })
}

/// **WHY A LOOP DID NOT START, IN A SENTENCE THE CALLER CAN ACT ON.**
///
/// ⚠⚠ Every arm names the KNOB or the FILE, because each of these is refused before anything
/// happens and the whole value of refusing early is that the caller can fix it and call again. A
/// refusal that said only *"the loop could not be started"* would cost them the run they were
/// spared.
fn ai_loop_refusal(why: &sprag_plugin::NotStarted) -> String {
    match why {
        sprag_plugin::NotStarted::Undrivable => {
            "this build's `ai_loop.scxml` does not carry the strings a loop is driven by, so no \
             run could be started against it — the document, or the statechart engine pinned under \
             it, is not the one this driver was written for"
                .to_owned()
        }
        // ⛔⛔⛔⛔⛔ **THE ARM THE SENTENCE ABOVE USED TO ANSWER FOR** — register item 510. The
        // constructor returned `Option`, so a document the door REFUSED was reported as one whose
        // `<data>` block was short of four strings: a reader sent to the wrong half of the right
        // file, with nothing to find when they got there.
        //
        // ⚠⚠ THE FAULT SPEAKS FOR ITSELF, and that is the whole repair rather than a nicety.
        // `Faulted`'s own `Display` names the class, the count, and that an error abandons the
        // rest of its block — which is why a half-composed `onentry` reads as a slow peer. A
        // sentence written here would be a second author of what an unanswered error means.
        sprag_plugin::NotStarted::Unanswered(faulted) => {
            format!(
                "this build's `ai_loop.scxml` did not survive being built: {faulted}. Nothing was \
                 prompted, and no argument of this request can change it — the document needs the \
                 clause that failed repaired AND an edge that answers the error, because it has \
                 neither"
            )
        }
        // ⚠⚠ AND THE THIRD OF THE THREE, which the same `Option` hid — register item 510. It names
        // the ENGINE rather than the document: there is no datamodel here to be missing strings
        // from, so `Undrivable`'s sentence would send a reader to read a `<data>` block that is
        // not the problem.
        sprag_plugin::NotStarted::Sessionless => {
            "this build's statechart engine opened `ai_loop.scxml` with no script session, so \
             nothing could read or write its datamodel — the engine pinned under this build, not \
             the document, is what to look at"
                .to_owned()
        }
        // ⚠⚠⚠⚠⚠ TWO ARMS STOOD HERE UNTIL 2026-08-26 R100, and both read a STATE NAME back out of
        // the refusal to choose a sentence — `Unbuilt(AiLoopState::Exhausted)` for this one, and a
        // general `Unbuilt(state)` beside it that NOTHING EVER BUILT. That was register item 470's
        // defect at one remove: `sprag-host` deciding from `ai_loop.scxml`'s vocabulary, in Rust,
        // for a variant only ever constructed one way. The word carries no payload now, so the
        // compiler is what says this arm exists.
        sprag_plugin::NotStarted::NoTurns => {
            "`max_turns` must be at least 1: a loop allowed no turns judges itself exhausted \
             before its agent has answered anything"
                .to_owned()
        }
        sprag_plugin::NotStarted::Brief(sprag_plugin::Briefed::NotHeld { part, held }) => {
            format!(
                "the loop's datamodel did not hold {part} as it was sent{}, so nothing was \
                 started rather than an agent being prompted with something nobody wrote",
                match held {
                    Some(held) => format!(" (it holds {held:?})"),
                    None => " (it holds nothing a reader can name)".to_owned(),
                },
            )
        }
        // Neither is reachable from here — the machine is built one line above the brief, so it is
        // in `idle`, and `Took` is the success this function is not called for. Said rather than
        // collapsed into a wildcard: a sentence nobody can produce is cheaper than a match that
        // stops being exhaustive when the type grows.
        sprag_plugin::NotStarted::Brief(sprag_plugin::Briefed::TooLate(state)) => {
            format!("the loop was already in {state:?} when it was briefed")
        }
        sprag_plugin::NotStarted::Brief(sprag_plugin::Briefed::Took) => {
            "the loop took its brief and did not start anyway".to_owned()
        }
        sprag_plugin::NotStarted::Screening(sprag_plugin::NotScreenable::Malformed { at, why }) => {
            format!(
                "screen rule {at} (counting from zero) is not one this build can carry out: {}",
                why.describe(),
            )
        }
        // ⚠⚠⚠⚠⚠ THE DOCUMENT'S OWN CONTENT DID NOT EXECUTE — register item 505. Every other arm
        // here names something the CALLER sent; this one names the FILE, and the difference matters
        // to whoever reads it: nothing the request said can fix a clause that will not evaluate. The
        // class is carried verbatim because it says who repairs it — `error.execution` is the
        // document's own content and `error.communication` is a `<send>` this host did not serve.
        sprag_plugin::NotStarted::Faulted(error) => {
            format!(
                "this build's `ai_loop.scxml` raised {error} while its datamodel was being \
                 initialised, so the document stopped itself before a run began — a clause in it \
                 could not be evaluated, and no argument of this request can change that. Nothing \
                 was prompted"
            )
        }
        sprag_plugin::NotStarted::Screening(sprag_plugin::NotScreenable::Unreadable) => {
            format!(
                "this loop's `{}` is not a list of {{{}: …, {}: …}} objects, so nothing could be \
                 read as a standing instruction — the document, or what was sent for it, is not \
                 the shape `screening` carries out",
                ScreenRules::WIRE_KEY,
                ScreenRule::WHEN_KEY,
                ScreenRule::TEXT_KEY,
            )
        }
        // ⛔⛔⛔ AND A CLAUSE THE DOCUMENT AUTHORED HALF OF — register item 741. It names the id
        // that is empty, because the whole value of refusing early is that the author can go and
        // fill it in; *your document is incomplete* would cost them the run they were spared.
        sprag_plugin::NotStarted::Screening(sprag_plugin::NotScreenable::Missing(id)) => {
            format!(
                "this loop's kind document leaves `{id}` empty while its partner is filled, and \
                 the two are one decision: a run meets exactly one of them, so a document that \
                 answers one silence and not the other leaves the rest of its runs with nothing to \
                 act on. Fill `{id}` in, or empty both and say nothing about either"
            )
        }
    }
}

fn opt_millis(map: &Map<String, Value>, key: &str) -> Result<Option<Duration>, InvokeError> {
    if declined(map, key) {
        return Ok(None);
    }
    Ok(Some(Duration::from_millis(
        map[key].as_u64().ok_or(InvokeError::TypeMismatch)?,
    )))
}

/// **WHERE THIS REQUEST SAYS THE RUN'S MACHINE WAS**, or [`None`] for a run starting from the top —
/// [`RUN_PLACE_KEY`]'s only reader. Register item 543.
///
/// # Errors
///
/// A value that is not a list of strings is a MALFORMED request, and so is an EMPTY list — refused
/// here rather than passed on, because `enter_at` would take it as a configuration with no members
/// and answer about the current state's membership: an engine's error where the RECORD was what was
/// wrong. `crate::runs::PersistedRun::resumable_place` refuses the same shape at the other end, and
/// the two agreeing is not duplication — one guards a log and this guards a request, and a driver
/// can be handed one by something that never read a log.
fn opt_place(map: &Map<String, Value>) -> Result<Option<Vec<String>>, InvokeError> {
    let words = match map.get(RUN_PLACE_KEY) {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(items)) => items
            .iter()
            .map(|word| word.as_str().map(str::to_owned))
            .collect::<Option<Vec<String>>>()
            .ok_or(InvokeError::TypeMismatch)?,
        Some(_) => return Err(InvokeError::TypeMismatch),
    };
    if words.is_empty() {
        return Err(InvokeError::TypeMismatch);
    }
    Ok(Some(words))
}

fn require_string_array(map: &Map<String, Value>, key: &str) -> Result<Vec<String>, InvokeError> {
    match map.get(key) {
        Some(Value::Array(items)) => {
            let argv = items
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
                .ok_or(InvokeError::TypeMismatch)?;
            if argv.is_empty() {
                Err(refused(format!(
                    "{key:?} is empty: an endpoint needs at least its program"
                )))
            } else {
                Ok(argv)
            }
        }
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// Parse an optional reply-format key (`"text"` | `"claude_json"`) into a
/// [`ReplyFormat`]. Absent → `None` (the spec keeps its default); an unknown
/// string → [`InvokeError::Rejected`].
fn parse_reply_format(
    map: &Map<String, Value>,
    key: &str,
) -> Result<Option<ReplyFormat>, InvokeError> {
    // THROUGH THE TYPE, whose `WIRE_WORDS` this verb publishes. ⚠ A word outside the vocabulary is
    // `TypeMismatch` — a malformed request — where this answered `Rejected` with a sentence naming the
    // two words. That sentence was the only place the vocabulary was written down; it is in the
    // published grammar now, and the class matters twice over: it is what every other closed
    // vocabulary on this wire answers, and a `Rejected` is invisible to the completeness gate, which
    // can only see an argument the daemon refuses AS MALFORMED.
    match opt_str(map, key)? {
        None => Ok(None),
        Some(word) => ReplyFormat::from_wire(word)
            .map(Some)
            .ok_or(InvokeError::TypeMismatch),
    }
}

/// Read the optional `guardrails` sub-object — the THREE ceilings a run is bounded
/// by, each defaulted so an omitted one is still a bound.
///
/// `max_iterations` defaults to [`DEFAULT_MAX_ITERATIONS`] and `max_seconds` to
/// [`DEFAULT_MAX_SECONDS`] (both always present — the liveness floor). The cost
/// bound is self-describing: `max_bytes` xor `max_tokens` in the plugin's unit
/// (omitted → the plugin's default ceiling). NB a `Tokens(0)`-only run (a
/// print-mode Text dialogue) accumulates no measured cost, so its cost ceiling
/// never binds and the other two are its effective bounds — by design.
///
/// ⚠⚠ **A KEY THIS OBJECT DOES NOT DECLARE IS A MALFORMED REQUEST**, which is not
/// how the rest of this wire treats an unknown key. The asymmetry is the whole
/// point and it is stated on
/// [`guardrail_fields`](crate::wire::PluginGrammar::guardrail_fields): ignoring
/// an ordinary argument makes a verb do LESS than asked and the caller can see
/// that in the result; ignoring a BOUND makes the run do more, without limit, and
/// answers success.
fn parse_guardrails(
    map: &Map<String, Value>,
    default_cost: Cost,
    authored: AuthoredGuardrails,
) -> Result<Guardrails, InvokeError> {
    // ⚠⚠⚠⚠⚠ THE THREE FALL-THROUGHS, IN ONE PLACE — register item 738, layer 1. Each bound is the
    // CALLER's number, then the number this run's own PLUGIN DOCUMENT authored, then this daemon's
    // constant. It is the same three-step chain `max_turns`, `context_ceiling` and
    // `reflect_after_refusals` already travel, arriving at the only bounds that had no document
    // step at all — and those are the three that actually kill runs here (measured: 8 of this
    // daemon's 49 recorded runs ended `exhausted (cost)` at the 64 KiB default, while the largest
    // run that converged spent 516,020 bytes).
    let iterations = || authored.max_iterations.unwrap_or(DEFAULT_MAX_ITERATIONS);
    let cost = || authored.max_cost.unwrap_or(default_cost);
    let duration = || {
        authored
            .max_duration
            .unwrap_or_else(|| Duration::from_secs(DEFAULT_MAX_SECONDS))
    };
    // ⚠ DECLINED, not merely absent — see [`declined`](crate::external::declined). A client whose
    // language serialises an absent optional as `null` sends `"guardrails": null` on every
    // unguarded run, and answering `TypeMismatch` there refuses a well-formed call.
    if declined(map, "guardrails") {
        return Ok(Guardrails {
            max_iterations: iterations(),
            max_cost: Some(cost()),
            max_duration: Some(duration()),
        });
    }
    let Value::Object(g) = &map["guardrails"] else {
        return Err(InvokeError::TypeMismatch);
    };
    // AGAINST THE PUBLICATION, not against a list kept here: the keys this parser honours and the
    // keys the grammar advertises are one set, so neither can grow without the other.
    let declared = crate::wire::PluginGrammar::guardrail_fields(default_cost.unit());
    if let Some(unknown) = g
        .keys()
        .find(|key| !declared.iter().any(|field| field.name == key.as_str()))
    {
        return Err(refused(format!(
            "{unknown:?} is not a guardrail of a run that spends {}. It takes: {}. A bound this \
             daemon does not know would have been ignored, and an ignored bound is not a bound.",
            default_cost.unit(),
            declared
                .iter()
                .map(|field| field.name)
                .collect::<Vec<_>>()
                .join(", "),
        )));
    }
    // ⚠ The SAME declined rule inside the nest. A nested optional is an optional.
    // ⚠⚠ AND A DECLINED FIELD NOW MEANS *WHAT THIS RUN'S DOCUMENT SAYS*, then the daemon's — the
    // three closures above, and the same widening `screen_rules` and `may_answer` took when a kind
    // acquired a voice. A caller who names a bound still overrides it.
    let max_iterations = if declined(g, "max_iterations") {
        iterations()
    } else {
        g["max_iterations"]
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)?
    };
    let max_duration = if declined(g, "max_seconds") {
        duration()
    } else {
        Duration::from_secs(g["max_seconds"].as_u64().ok_or(InvokeError::TypeMismatch)?)
    };
    Ok(Guardrails {
        max_iterations,
        max_cost: parse_max_cost(g, cost())?,
        max_duration: Some(max_duration),
    })
}

/// Parse the optional cost bound: `max_bytes` XOR `max_tokens` (a run has ONE
/// cost unit), or the plugin's default when neither is given. The chosen unit
/// must match the plugin's — so a guardrail cannot be misloaded into the wrong
/// currency. Both keys present, a non-integer, or the wrong unit → a synchronous
/// [`InvokeError`] (a misloaded spend guardrail is a submit-time error, never a
/// silently looser-by-a-factor bound).
fn parse_max_cost(g: &Map<String, Value>, default_cost: Cost) -> Result<Option<Cost>, InvokeError> {
    // ⚠⚠ A DECLINED KEY IS NOT A GIVEN ONE, and here that is load-bearing rather than tidy: the
    // XOR below refuses BOTH-given, so a client declining one unit with `null` would have been told
    // it had named two cost units when it had named one.
    let bound = match (
        (!declined(g, "max_bytes")).then(|| &g["max_bytes"]),
        (!declined(g, "max_tokens")).then(|| &g["max_tokens"]),
    ) {
        (Some(_), Some(_)) => {
            return Err(refused(
                "max_bytes and max_tokens were both given: a run has one cost unit",
            ));
        }
        (Some(v), None) => Cost::Bytes(v.as_u64().ok_or(InvokeError::TypeMismatch)?),
        (None, Some(v)) => Cost::Tokens(v.as_u64().ok_or(InvokeError::TypeMismatch)?),
        (None, None) => return Ok(Some(default_cost)),
    };
    if bound.unit() != default_cost.unit() {
        return Err(refused(format!(
            "this plugin spends {}, so a {} bound cannot guard it",
            default_cost.unit(),
            bound.unit()
        )));
    }
    Ok(Some(bound))
}

/// **WHAT A RUNNING RUN HAS DONE SO FAR**, as the row publishes it — register item 650.
///
/// # ⚠⚠⚠⚠⚠ Why this is a function and not four lines inside the row builder
///
/// A run driven by another process (register items 544 / 643) computes these numbers over there,
/// and this daemon has to end up publishing the same ones. Written inline, the driver would have to
/// spell the shape a second time — and a second speller of one shape is what the driver module's
/// own outcome report already refuses, using [`outcome_to_json`] rather than writing
/// `{"state": …}` over there.
///
/// So this is the one renderer, both kinds of driver feed it, and the row cannot learn which kind
/// filled it in.
///
/// ⚠⚠ **IT TAKES THE NUMBERS AND NOT A [`sprag_plugin::Progress`]**, deliberately. That type is
/// built out of `&'static str` — `at`, and three per journal `Edge` — because it was only ever read
/// in this process; rebuilding one from the wire means interning a statechart vocabulary that is
/// upstream's to publish. Nothing is rebuilt: what crosses is what this produced.
pub fn progress_to_json(progress: &sprag_plugin::Progress) -> Value {
    // ⚠ THE SAME THREE KEYS THE OUTCOME USES (`iterations`, `cost`, `unit`), so a reader that polls
    // a running run and then reads its outcome meets ONE vocabulary rather than two. A run that has
    // not finished a step yet answers zero with a null unit, which is the same shape a run that was
    // cancelled before any step reports — both mean "nothing measured yet".
    let (cost, unit) = progress
        .cost
        .map_or((0, None), |c| (c.amount(), Some(c.unit())));
    let mut answer = json!({
        "iterations": progress.iterations,
        "cost": cost,
        "unit": unit,
        // ⚠⚠ AND THE ANSWER TALLY MID-FLIGHT, under the outcome's own name. The comment above says
        // these keys exist so a reader who polls a run and then reads its outcome meets ONE
        // vocabulary — and this is the key where the polling matters MOST: the other two are
        // watched to tell progress from stuck, this one is watched to see a decision being taken on
        // your behalf while there is still time to cancel.
        RUN_ANSWERED_KEY: progress.answered,
    });
    // ⚠⚠⚠⚠⚠ **AND WHERE THE RUN'S MACHINE IS — register item 662, and this renderer is the ONLY
    // way that fact can cross a process boundary.** A driver in another process reports through
    // here and nowhere else, so a key missing here is a fact the daemon cannot know about such a
    // run *at all*: `RunRegistry::persistable` was reading `at` and `place` off a cell that never
    // moves for those runs, and writing a durable record that said `place: None` however long the
    // run had been going. Item 543's whole chain ends at the out-of-process driver, and it ended
    // in a log with nothing in it.
    //
    // ⚠⚠ PRESENT ONLY WHEN THERE IS ONE, which is this surface's `RUN_CEILING_KEY` rule and here
    // it is load-bearing rather than tidy: absent must keep meaning *nobody said*, because that is
    // what a driver built before this key existed reports, and a daemon that read absence as
    // anything else would put a restarted run somewhere nobody chose.
    //
    // ⚠ NO SECOND SPELLING IS CREATED. Neither of these is rendered beside the state by
    // `run_to_json`, so unlike `RUN_DELIVERED_KEY` and its neighbours there is no cell-fed copy of
    // them for this to disagree with — the reason the two carried here are exactly these two and
    // not everything a `Progress` holds. See register item 663 for the pair that is still split.
    if let Some(at) = progress.at {
        answer[RUN_AT_KEY] = json!(at);
    }
    if let Some(place) = &progress.place {
        answer[RUN_PLACE_KEY] = json!(place);
    }
    // ⚠⚠⚠⚠⚠ **AND EVERYTHING A ROW PUBLISHES BESIDE ITS STATE** — register item 663, nested under
    // one key for the reason [`REPORTED_BESIDE_KEY`] holds: flat, each of these would be a second
    // spelling of a number the row already publishes from the cell.
    //
    // ⚠⚠ UNCONDITIONAL, unlike the two above, and the difference is what absence has to mean. A
    // MISSING `beside` is an older driver saying nothing; a PRESENT one saying `delivered: 0` is
    // this driver saying it has delivered nothing, which is a fact and not a silence. The row's
    // own presence rules (a triple published only once something was typed, a sentence only once a
    // claim was checked) stay where they are — they are about what a READER should be shown.
    answer[REPORTED_BESIDE_KEY] = json!({
        RUN_DELIVERED_KEY: progress.deliveries.made,
        RUN_FOLDED_KEY: progress.deliveries.folded,
        RUN_UNSUBMITTED_KEY: progress.deliveries.unsubmitted,
        RUN_CHECKS_KEY: {
            "asked": progress.checks.asked,
            "silent": progress.checks.silent,
            // ⚠ The checker's own words, carried rather than re-composed: `judge` is the one
            // authority on what a silence means, and it lives on the other side of this wire.
            "why_silent": progress.checks.why_silent,
            // ⚠⚠⚠ AND THE THIRD VERDICT WITH THE DEPTH IT REACHED — register item 499. Added
            // KEYS on an answer, which is the one change this surface's pin does not number: an
            // older daemon omits them and the reader below refuses the tally whole rather than
            // filling in a zero it was not told, so absence is *this daemon did not say* and
            // never *no claim was ever refused*.
            "refused": progress.checks.refused,
            "refused_in_a_row": progress.checks.refused_in_a_row,
            // ⚠⚠⚠⚠⚠ AND THE CLAIMS THAT NEVER REACHED A CHECKER — register item 674. It is
            // OUTSIDE `asked` on purpose (see `Checks::unasked`): a claim the loop could not put
            // is not a question anybody asked, and folding it in would make the denominator
            // flatter the checker by counting a question that never happened.
            "unasked": progress.checks.unasked,
        },
        RUN_DRIVING_KEY: progress.driving.map(|pane| pane.0),
        RUN_BANKED_KEY: progress.banked.as_ref().map(|banked| json!({
            "completed": banked.completed,
            "unit": banked.unit.as_ref(),
        })),
        // ⚠⚠⚠ AND HOW BIG THE BRIEF IS — register item 719's second direction. The THREE PARTS and
        // not the sentence, which is the division every level here keeps: a driver in another
        // process reports what it measured, and the one composer of the prose is
        // `briefing_sentence`, at the row (item 663). Two composers would be two ways to name the
        // largest part.
        RUN_BRIEFED_KEY: progress.briefed.map(|briefed| json!({
            "north_star": briefed.north_star,
            "milestone": briefed.milestone,
            "reference": briefed.reference,
        })),
        // ⚠⚠⚠⚠⚠ **AND THE WALK ITSELF** — register item 544's default flip is what found this. The
        // row publishes a run's per-step journal beside its state, out of the cell, so a run driven
        // in another process had an EMPTY walk however many steps it took — and nine tests that
        // read what a run did step by step went red the day the option's default changed. It
        // travels ALREADY RENDERED, unlike everything else here, because `step_to_json` is the only
        // reader there is: `persistable` deliberately does not save a journal, so nothing downstream
        // wants a typed `StepRecord` back.
        RUN_JOURNAL_KEY: progress.journal.iter().map(step_to_json).collect::<Vec<_>>(),
    });
    answer
}

/// **THE PROGRESS DOCUMENT A ROW SHOWS AS ITS STATE** — everything except the facts published
/// BESIDE the state, which the row lifts out and renders itself. Register item 663.
///
/// ⚠ One removal by one name: see [`REPORTED_BESIDE_KEY`] for why a structural strip beats a list
/// of key names that has to be kept in step with the row.
fn without_the_beside(mut progress: Value) -> Value {
    if let Some(held) = progress.as_object_mut() {
        held.remove(REPORTED_BESIDE_KEY);
    }
    progress
}

/// **WHAT A DRIVER'S PROGRESS REPORT SAYS**, for the one reader that cannot take the blob whole —
/// [`progress_to_json`]'s inverse, register item 662.
///
/// # ⚠⚠⚠⚠⚠ Why this exists when `RunRecord::reported` says the blob is never read apart
///
/// That rule is about the ROW, and it is right there: the row republishes what the renderer
/// produced, so a key the renderer grows arrives with nothing to update. A durable LOG cannot do
/// that — [`crate::runs::PersistedRun`] is a versioned file with named columns, and *carry whatever
/// arrived* is not a thing a file format can mean. So exactly one reader unpacks, it lives beside
/// the writer so the two are edited together, and what it hands back is typed rather than JSON.
///
/// ⚠⚠ **EVERY FIELD IS OPTIONAL AND [`None`] MEANS *THE DRIVER DID NOT SAY*** — never zero, never
/// "no place". A report from a driver built before a key existed is the ordinary case here (the two
/// images are the same binary only until somebody promotes one), and a reader that turned silence
/// into a value would write a record nobody authored.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReportedProgress {
    /// How many steps the driver said it had completed.
    pub iterations: Option<u32>,
    /// What it said it had spent, unit and all — [`None`] when it reported no unit, which is how
    /// [`progress_to_json`] spells *nothing measured yet*.
    pub cost: Option<sprag_plugin::Cost>,
    /// Where it said its machine was, in one word for a person — `sprag_plugin::Plugin::at`.
    pub at: Option<String>,
    /// The whole place its machine was in — `sprag_plugin::Plugin::place`, the thing item 543's
    /// resume is entered at.
    pub place: Option<Vec<String>>,
    /// What it said it had put into its pane — register items 663 / 617. [`None`] for a driver
    /// whose build knew no [`REPORTED_BESIDE_KEY`], never for one that has delivered nothing.
    pub deliveries: Option<sprag_plugin::Deliveries>,
    /// What it said its milestone checks came to — items 663 / 601. The TALLY; the sentence a
    /// reader gets is [`checks_sentence`]'s, composed here from this.
    pub checks: Option<sprag_plugin::Checks>,
    /// Which pane it said it was driving — items 663 / 540.
    pub driving: Option<PaneId>,
    /// How much of its work it said was complete and kept — items 663 / 616.
    ///
    /// ⚠ Here the two silences GENUINELY read alike — *this plugin counts no work* and *this driver
    /// never said* both arrive as [`None`] — and that costs nothing, because the only run this is
    /// read for is one whose cell is empty by construction. A caller that could tell them apart
    /// would have nothing different to do.
    pub banked: Option<sprag_plugin::Banked>,
    /// How big it said the brief it was started with is — items 663 / 719.
    ///
    /// ⚠ The three parts WHOLE or nothing, `deliveries`' rule: the sentence a reader gets names the
    /// LARGEST of them, which is a comparison, so a half-read value would point confidently at the
    /// wrong part.
    pub briefed: Option<sprag_plugin::Briefing>,
    /// What it said it did, step by step — items 663 / 544, and **the one field here that is
    /// already JSON**.
    ///
    /// ⚠⚠ Everything else is typed because a durable log has named columns and needs values. A
    /// journal has exactly one reader — the row, which renders it with `step_to_json` — and
    /// `crate::runs::RunRegistry::persistable` deliberately keeps no journal at all. Decoding a
    /// rendered walk back into `StepRecord`s so the same function could render it again would be a
    /// second spelling of the walk with nothing asking for one.
    pub journal: Option<Vec<Value>>,
}

/// Read a driver's progress report — see [`ReportedProgress`].
#[must_use]
pub fn progress_from_report(reported: &Value) -> ReportedProgress {
    let amount = reported.get("cost").and_then(Value::as_u64);
    // ⚠ `Value::Null` when the report has none, so every read below simply answers [`None`] — an
    // older driver needs no arm of its own.
    let beside = reported.get(REPORTED_BESIDE_KEY).unwrap_or(&Value::Null);
    // ⚠⚠ READ OUT OF THAT NESTED OBJECT AND NOWHERE ELSE — see [`REPORTED_BESIDE_KEY`]. Each is
    // [`None`] for a driver whose build did not know the key, so the caller falls back to the cell
    // rather than publishing zeros nobody reported.
    //
    // ⚠ **WHOLE OR NOTHING** for the two that are compound values, which is those types' own rule:
    // `Deliveries` exists so a fold count can never travel without its denominator, and a reader
    // that filled in the half it did not receive would be the writer those types forbid.
    let deliveries = (|| {
        Some(sprag_plugin::Deliveries {
            made: small(beside.get(RUN_DELIVERED_KEY))?,
            folded: small(beside.get(RUN_FOLDED_KEY))?,
            unsubmitted: small(beside.get(RUN_UNSUBMITTED_KEY))?,
        })
    })();
    let checks = (|| {
        let tally = beside.get(RUN_CHECKS_KEY)?;
        Some(sprag_plugin::Checks {
            asked: small(tally.get("asked"))?,
            silent: small(tally.get("silent"))?,
            why_silent: tally
                .get("why_silent")
                .and_then(Value::as_str)
                .map(str::to_owned),
            // ⚠⚠⚠ WHOLE OR NOTHING REACHES THE TWO NEW KEYS TOO — register item 499, on this
            // block's own rule. A daemon too old to publish them is a daemon that cannot say
            // whether anything was refused, and a `0` filled in here would answer *nothing ever
            // was* on its behalf. Refusing the tally sends the row to the cell instead, which is
            // exactly what a reader gets today from a daemon older than item 663.
            refused: small(tally.get("refused"))?,
            refused_in_a_row: small(tally.get("refused_in_a_row"))?,
            // ⚠⚠⚠⚠⚠ AND WHOLE-OR-NOTHING REACHES THIS ONE TOO — register item 674, on the rule
            // stated directly above and for the sharpest instance of it. A `0` filled in here
            // would answer *this run put every claim it had to a checker* on behalf of a daemon
            // that never said so — which is precisely the flattering silence item 674 exists to
            // end, re-created by the reader after the writer stopped making it.
            unasked: small(tally.get("unasked"))?,
        })
    })();
    let banked = (|| {
        let banked = beside.get(RUN_BANKED_KEY)?;
        Some(sprag_plugin::Banked {
            completed: small(banked.get("completed"))?,
            unit: std::borrow::Cow::Owned(banked.get("unit")?.as_str()?.to_owned()),
        })
    })();
    // ⚠ WHOLE OR NOTHING, the two above's rule — see `ReportedProgress::briefed`.
    let briefed = (|| {
        let briefed = beside.get(RUN_BRIEFED_KEY)?;
        Some(sprag_plugin::Briefing {
            north_star: bytes(briefed.get("north_star"))?,
            milestone: bytes(briefed.get("milestone"))?,
            reference: bytes(briefed.get("reference"))?,
        })
    })();
    ReportedProgress {
        iterations: reported
            .get("iterations")
            .and_then(Value::as_u64)
            .and_then(|held| u32::try_from(held).ok()),
        // ⚠ THE UNIT DECIDES WHETHER THERE IS A COST AT ALL, because the renderer writes
        // `cost: 0, unit: null` for a run that has spent nothing measurable — so an amount with no
        // unit is *not measured* and not *zero of something*.
        cost: match (amount, reported.get("unit").and_then(Value::as_str)) {
            (Some(held), Some("tokens")) => Some(sprag_plugin::Cost::Tokens(held)),
            (Some(held), Some(_)) => Some(sprag_plugin::Cost::Bytes(held)),
            _ => None,
        },
        at: reported
            .get(RUN_AT_KEY)
            .and_then(Value::as_str)
            .map(str::to_owned),
        // ⚠⚠ A LIST OF WORDS OR NOTHING — a `place` that is not a list of strings is a report this
        // build cannot read, and `LoopPlace::from_words`' rule applies at every gate on the way:
        // an answer that cannot be read whole is an answer this declines to have.
        place: reported.get(RUN_PLACE_KEY).and_then(|held| {
            held.as_array()?
                .iter()
                .map(|word| word.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        }),
        deliveries,
        checks,
        driving: beside
            .get(RUN_DRIVING_KEY)
            .and_then(Value::as_u64)
            .map(PaneId),
        banked,
        briefed,
        journal: beside
            .get(RUN_JOURNAL_KEY)
            .and_then(Value::as_array)
            .cloned(),
    }
}

/// A count a report carried, or [`None`] when it is absent or not a number this build can hold.
///
/// ⚠ Refused rather than saturated: these are counts a person reads to decide what to do, and a
/// number too large to hold is a report this build cannot read — which is a different answer from
/// a large count, and the only honest one.
fn small(held: Option<&Value>) -> Option<u32> {
    u32::try_from(held?.as_u64()?).ok()
}

/// A BYTE COUNT a report carried, or [`None`] when it is absent or not a number this build can
/// hold — [`small`]'s rule for the one quantity here that is a length rather than a tally.
///
/// ⚠ Separate from [`small`] because these are `usize` and those are `u32`, and widening `small`
/// would let a tally be read into a length's slot without anybody choosing that.
fn bytes(held: Option<&Value>) -> Option<usize> {
    usize::try_from(held?.as_u64()?).ok()
}

/// The `running` state a row shows for a run whose driver REPORTED `progress` from another process.
///
/// ⚠⚠⚠ `reported` is what [`progress_to_json`] produced over there, carried whole rather than
/// re-read key by key: a reader here that picked out `iterations` and forgot `answered` the day a
/// key was added would publish a row that silently lost it — and lose it only for out-of-process
/// runs, which is the invisible divergence `crate::options::RUN_DRIVER_PROCESS` promises cannot
/// happen.
fn progress_reported(reported: Value) -> Value {
    let mut state = reported;
    state["status"] = json!(RunStatus::Running.wire_str());
    state
}

/// Render one run as JSON for `query("runs")`.
///
/// `seat` is the pane to publish as `opened_by`, already resolved by the caller: `run.opened_by`
/// for a run this daemon issued, and the pane currently holding `run.opened_by_session` for one it
/// inherited from a predecessor. Taken as a parameter rather than read off `run` because the second
/// answer needs the workspace, which this function has no business holding.
///
/// # ⚠⚠⚠ Why re-deriving `opened_by` earns no [`sprag_rpc::WIRE_PROTOCOL`] bump
///
/// Written down because NOT bumping is a judgement too, and this one is close enough to the line to
/// deserve its reasoning rather than its conclusion.
///
/// Nothing about the key moved. `opened_by` still means exactly what it meant — *the pane whose
/// occupant asked for this run* — it still carries a pane id, and it is still OMITTED rather than
/// sent as `null` when nobody claims the run. What changed is that a restored run can now be
/// answered at all, where before the daemon had thrown away the only thing that could have answered
/// it. That is the daemon answering a question it already published MORE OFTEN, which is the
/// widened-value-space case the bump rule explicitly declines.
///
/// ⚠⚠ **The rule it comes closest to is *"reading the absence of an answer key as a guarantee"***,
/// and it is worth saying why that one does not fire: no reader treats a missing `opened_by` as
/// *"this run is nobody's, for ever"*. The agent-facing filter (`sprag-mcp`'s `own_runs`) compares
/// it to its own pane and a miss simply means *not mine* — which is the same sentence before and
/// after. A reader that had encoded *absent ⇒ unclaimable* would be the one to break, and none does.
///
/// ⚠ Ask [`sprag_rpc::WIRE_PROTOCOL`]'s own doc rather than this paragraph if the question comes up
/// again; this records the judgement taken for THIS change, not the rule.
fn run_to_json(run: &RunSummary, seat: Option<u64>) -> Value {
    let (cost, unit) = run
        .progress
        .cost
        .map_or((0, None), |c| (c.amount(), Some(c.unit())));
    // ⚠⚠⚠⚠⚠ **WHAT THIS RUN'S DRIVER SAID, IF IT IS IN ANOTHER PROCESS** — register item 663. Read
    // ONCE here and used by every beside-the-state key below, each of which prefers it and falls
    // back to the cell. There is no report for a run this daemon drives on a thread of its own, so
    // the fallback is not a defensive nicety — it is the whole answer for such a run. ⚠ Since
    // 2026-08-24 that is the arm a daemon told `off` takes rather than the default one, and the
    // fallback is load-bearing exactly because `off` is the way back.
    let reported = run
        .reported
        .as_ref()
        .map(progress_from_report)
        .unwrap_or_default();
    let state_json = match &run.state {
        // ⚠⚠⚠ A REPORT IS PREFERRED OVER THE CELL, and the ordering is the whole of register item
        // 650's second half: for a run driven in another process the cell beside it NEVER MOVES,
        // so reading it first would publish zeros over a report that had already arrived. For every
        // other run there is no report and the cell is the only answer — so one arm serves both,
        // and the row cannot tell which kind of driver filled it in.
        // ⚠⚠ AND WHAT IT CARRIED FOR THE KEYS BESIDE THE STATE IS LIFTED OUT — register item 663.
        // The report is a TRANSPORT: leaving that object in would publish the delivery triple and
        // the driven pane twice in one row, and only for one kind of driver.
        RunState::Running => progress_reported(without_the_beside(
            run.reported
                .as_ref()
                .map_or_else(|| progress_to_json(&run.progress), Clone::clone),
        )),
        RunState::Done { outcome, output } => json!({
            "status": RunStatus::Done.wire_str(),
            "outcome": outcome_to_json(outcome),
            "output": output,
        }),
        // ⚠⚠⚠⚠⚠ `done`, AND THE SAME KEYS — a client asked *what became of this run*, and *whose
        // process computed the answer* is not part of that question. A fifth `status` word would be
        // a break no address or shape pin can see (item 342), for a distinction no reader wants.
        //
        // ⚠⚠ The reported object is spliced WHOLE and its `output` lifted to where a `Done` run
        // publishes it, because that is where every existing reader looks. The driver produced it
        // with THIS daemon's own `outcome_to_json`, so the two arms cannot drift into two shapes.
        RunState::Reported(reported) => {
            let mut outcome = (**reported).clone();
            let output = outcome
                .as_object_mut()
                .and_then(|fields| fields.remove("output"));
            json!({
                "status": RunStatus::Done.wire_str(),
                "outcome": outcome,
                "output": output,
            })
        }
        RunState::Panicked(message) => {
            json!({ "status": RunStatus::Panicked.wire_str(), "error": message })
        }
        // ⚠ A FOURTH STATUS WORD, which is why `WIRE_PROTOCOL` moved: `status` is a value space a
        // peer decodes whole, so an added word is a break no address or shape pin can see (R342).
        // The counters it reached are still here — what it managed before its daemon died is the
        // only thing a reader can still learn about it.
        RunState::Interrupted => json!({
            "status": RunStatus::Interrupted.wire_str(),
            "iterations": run.progress.iterations,
            "cost": cost,
            "unit": unit,
        }),
    };
    // `opened_by` is OMITTED for a run nobody claims rather than sent as `null`, the rule
    // `ArgGrammar::to_answer` follows for an absent vocabulary: a reader tells silence from a claim
    // by the key's absence, and "a person started this" is a silence.
    let mut entry = json!({
        RUN_ID_KEY: run.id.0,
        "label": run.label,
        "state": state_json,
        // ⚠ THE JOURNAL SITS BESIDE THE STATE, NOT INSIDE IT, because it is the one fact that
        // means the same thing whether the run is still going or over: these are the steps it
        // took. Nesting it under `running` would have made a finished run's account vanish at
        // exactly the moment somebody wants to read it.
        // ⚠⚠ THE REPORT'S WALK FIRST — register items 663 / 544. A run driven in another process
        // has an empty cell, so reading it here published *this run has taken no steps* about a run
        // that had taken many.
        RUN_JOURNAL_KEY: reported.journal.clone().unwrap_or_else(|| {
            run.progress.journal.iter().map(step_to_json).collect()
        }),
    });
    // ⚠⚠ THE SEAT IS THE CALLER'S TO RESOLVE, NOT THIS FUNCTION'S, and that is the shape of the
    // fix rather than an accident of plumbing: for a run this daemon issued it is `run.opened_by`,
    // and for one it INHERITED it is whoever is currently holding the conversation that asked
    // (`PluginsExternal::seat_of`). Only the caller can see the workspace, so only the caller can
    // answer the second — see `crate::runs::RunRegistry::restore`'s rule 1.
    if let Some(opener) = seat {
        entry[RUN_OPENED_BY_KEY] = json!(opener);
    }
    // ⚠⚠⚠ AND THE BUILD FOLLOWS THE SAME OMIT-RATHER-THAN-NULL RULE, for a reason of its own:
    // absent means NOTHING RECORDED WHICH BUILD THIS WAS — a run restored from a log written before
    // the field existed — and a reader that filled that in with the daemon it is talking to would
    // date a dead daemon's work to its successor. See `crate::runs::RunSummary::build`.
    if let Some(build) = &run.build {
        entry[RUN_BUILD_KEY] = json!(build);
    }
    // ⚠⚠⚠ AND WHAT BECAME OF A PERSON'S ORDER — register item 594, present only when somebody gave
    // one, which is `RUN_CEILING_KEY`'s presence-is-the-claim rule.
    //
    // ⚠⚠ IT SITS BESIDE THE STATE AND NOT INSIDE IT, `RUN_JOURNAL_KEY`'s argument verbatim: it is
    // one of the two facts that mean the same thing whether the run is still going or over. Nesting
    // it under `done` would make a standing order invisible on exactly the runs a person can still
    // do something about, and nesting it under `running` would erase it at the moment they need it.
    if run.stood_down {
        entry[RUN_STOOD_DOWN_KEY] = json!(stand_down_sentence(&run.state));
    }
    // ⚠⚠⚠⚠⚠ AND THE SAME ORDERS AS DATA, FOR THE DRIVER — register item 699. The sentence above is
    // for a person and cannot be read by a machine; this is the machine's copy and cannot be read
    // by a person. They are written together, from one `RunSummary` read on one pass, so the two
    // cannot disagree about what was ordered.
    //
    // ⚠⚠ UNCONDITIONAL, where the sentence above is present-is-the-claim. The driver asks on every
    // batch and `held` is a LEVEL a person takes back, so *the key is gone* would have to mean
    // *released* — and absence already means *never ordered*. One key cannot carry both, and a
    // release that arrived as an absence is the shape `resume-run` needs most.
    entry[RUN_ORDERS_KEY] = json!(StandingOrders {
        stand_down: run.stood_down,
        held: run.held,
    });
    // ⚠⚠⚠⚠ AND WHO RAISED THE CANCEL — register item 596, the other half of 594's unanswered
    // *why*. Beside the state and not inside it for the same reason as the order above: a run
    // stopped by a shutdown is exactly the run a person still wants to do something about, and the
    // sweep happens on a daemon's way out, so this is read after a restart or not at all.
    if let Some(who) = run.cancelled_by {
        entry[RUN_CANCELLED_BY_KEY] = json!(cancel_sentence(who, &run.state));
    }
    // ⚠⚠⚠⚠ AND WHAT THIS RUN HAS PUT INTO ITS PANE — register item 591, present only for a run
    // that has delivered something, which is `RUN_CEILING_KEY`'s presence-is-the-claim rule.
    //
    // ⚠⚠ BESIDE THE STATE for `RUN_JOURNAL_KEY`'s reason, which is this fact's reason exactly: it
    // means the same thing whether the run is still going or over. Nested under `running` it would
    // vanish from a finished run — the one a person reads to work out why it went wrong — and
    // nested under the outcome it would be invisible while there was still time to act on it.
    //
    // ⚠ THE PAIR, never the fold count alone: `folded: 3` says nothing without the denominator,
    // and `sprag_plugin::Deliveries`' own doc holds the argument.
    // ⚠⚠⚠⚠⚠ **THE CONDITION IS *DID THIS RUN PUT A PROMPT ANYWHERE*, NOT *WAS ONE ASKED*** —
    // register item 617. It used to be `made > 0`, and a WEDGED run has `made == 0` by definition:
    // nothing was asked, so there is no delivery to count. The triple was therefore published under
    // a condition the run it matters most for can never satisfy, and the run whose prompt is still
    // sitting in a composer somebody could walk over and read said nothing at all.
    //
    // ⚠ The absence is still a claim for a run that composed NOTHING — `pipe` and `orchestrator`
    // relay words they did not write — which is why this is a predicate over the value rather than
    // an unconditional publication of three zeroes.
    //
    // ⚠⚠⚠⚠ THE REPORT FIRST — register item 663. For a run driven in another process the cell is
    // all zeros for ever, so reading it first published *nothing was ever typed* about a run that
    // had filled somebody's pane.
    let deliveries = reported.deliveries.unwrap_or(run.progress.deliveries);
    if deliveries.made > 0 || deliveries.unsubmitted > 0 {
        entry[RUN_DELIVERED_KEY] = json!(deliveries.made);
        entry[RUN_FOLDED_KEY] = json!(deliveries.folded);
        // ⚠ AND THE THIRD OF THE TRIPLE — register item 617. Published beside its two neighbours
        // because they are ONE value (`sprag_plugin::Deliveries`), and a key a writer can omit
        // half of is what that type exists to prevent.
        entry[RUN_UNSUBMITTED_KEY] = json!(deliveries.unsubmitted);
    }
    // ⚠⚠⚠⚠ AND WHETHER ANYTHING INDEPENDENT VERIFIED WHAT IT CONVERGED ON — register item 601,
    // beside the state for the two keys above's reason and absent when no claim was ever put to a
    // checker. ⚠ The SENTENCE, because the fact a reader acts on is a comparison (`silent` against
    // `asked`) and not a pair of numbers — `delivery_sentence`'s argument one key over.
    //
    // ⚠ The report's TALLY, and the sentence composed here from it — one composer, item 663.
    if let Some(said) = checks_sentence(reported.checks.as_ref().unwrap_or(&run.progress.checks)) {
        entry[RUN_CHECKS_KEY] = json!(said);
    }
    // ⚠⚠⚠⚠⚠ AND HOW BIG THE BRIEF IT WAS STARTED WITH IS — register item 719's second direction,
    // beside the state on the terms every level above it is published under: the report first, the
    // cell as the fallback, and the SENTENCE rather than the numbers, because what a reader acts on
    // is *which part to shorten* and that is a comparison. Absent for a run nobody briefed.
    if let Some(said) = reported
        .briefed
        .or(run.progress.briefed)
        .and_then(briefing_sentence)
    {
        entry[RUN_BRIEFED_KEY] = json!(said);
    }
    // ⛔⛔⛔⛔⛔ AND WHETHER ANY SUCCESSOR IS GOING TO PUT THIS RUN BACK — register item 737, beside
    // the state on the terms every clause above it is published under, and absent for every run
    // that is not one a boot declined to resume.
    //
    // ⚠⚠ IT IS THE ONE CLAUSE HERE THAT IS ABOUT THIS DAEMON'S OWN DECISION rather than about what
    // the run did, which is exactly why the row has to carry it: the run did nothing wrong and its
    // own account cannot mention that the documents moved underneath it.
    //
    // ⚠⚠⚠ AND IT IS SAID ONLY WHILE THE CLAIM IS TRUE, which is what the state guard is for rather
    // than tidiness: the sentence asserts *no successor is going to pick this up*, and that is a
    // statement about a run that is still `interrupted`. A run this daemon somehow got moving again
    // would carry a claim that had stopped being true, which is worse than not carrying one.
    if let (Some(why), RunState::Interrupted) = (&run.withheld, &run.state) {
        entry[RUN_WITHHELD_KEY] = json!(withheld_sentence(why));
    }
    // ⚠⚠⚠⚠ AND WHICH PANE IT IS DRIVING — register item 540, present only once a step has said so,
    // which is `RUN_CEILING_KEY`'s presence-is-the-claim rule. ⚠ The NUMBER and not the label's
    // prose: a reader that had to parse `ai_loop pane=3` would be deriving a fact from a name.
    if let Some(pane) = reported.driving.or(run.progress.driving) {
        entry[RUN_DRIVING_KEY] = json!(pane.0);
    }
    entry
}

/// Render one journal entry as JSON.
///
/// The step's OWN cost with its own unit, so a reader can find the expensive step rather than only
/// the total — and the plugin's `note` verbatim, which the host does not interpret (the Driver does
/// not either; see [`sprag_plugin::Step::note`]). A step with nothing to say OMITS the key, the
/// rule `run_to_json` follows for `opened_by`: absence is silence, not an empty claim.
fn step_to_json(step: &sprag_plugin::StepRecord) -> Value {
    let mut entry = json!({
        "iteration": step.iteration,
        "cost": step.cost.amount(),
        "unit": step.cost.unit(),
        "verdict": step.verdict.wire_str(),
    });
    if let Some(note) = &step.note {
        entry["note"] = json!(note);
    }
    entry
}

/// Render a plugin [`Outcome`] as JSON (serialization is a host concern, so the
/// pinion-free substrate stays serde-free).
/// An outcome's terminal word — the ONE mapping, read by the wire renderer AND by the durable run
/// log, so a run reloaded from disk cannot come back under a different word than it went out under.
#[must_use]
pub fn outcome_word(outcome: &Outcome) -> &'static str {
    // ⚠⚠ THROUGH THE TYPE, which is where every other variant→name mapping on this wire lives
    // (`Cost::unit`, `Ceiling::wire_str`, `Verdict::wire_str`) and where this one did NOT until
    // R366. Spelled here, the host could name an outcome the type had renamed, and there was no
    // list for the answers pin to walk — so the pin hand-wrote five variants and said so.
    outcome.state.wire_str()
}

/// Which ceiling stopped it, or [`None`] when no ceiling did — [`outcome_word`]'s companion.
#[must_use]
pub fn outcome_ceiling(outcome: &Outcome) -> Option<&'static str> {
    match &outcome.state {
        OutcomeState::Exhausted(ceiling) => Some(ceiling.wire_str()),
        _ => None,
    }
}

/// WHAT THE PEER IS ASKING, for a run that ended [`OutcomeState::Blocked`] — the question's own
/// text and its options, or [`None`] when there is no question to publish.
///
/// # ⚠⚠ Why the OPTIONS and not just the sentence
///
/// A caller reading this has to answer it, and the answer is a NUMBER. Publishing only the prose
/// would leave them to parse the choices back off a screen this host has already parsed — and to
/// guess which one a bare Enter would take, which is the difference between confirming a tool call
/// and declining it. `selected` is that fact, carried rather than inferred.
///
/// ⚠ `None` for a blocked run is a real answer and not a gap: an agent can block on something that
/// is not a numbered list. Its remedy is the one
/// [`AgentObservation::asking`](sprag_plugin::AgentObservation::asking) states — hand the pane to a
/// person — and a caller can tell the two apart because the key is ABSENT rather than empty.
#[must_use]
pub fn outcome_question(outcome: &Outcome) -> Option<Value> {
    let OutcomeState::Blocked(Some(unanswered)) = &outcome.state else {
        return None;
    };
    // ⚠ WHY is unconditional and the question is not. A run that was given a consent and stopped
    // anyway is indistinguishable from one that was given none without it, and those are two
    // different things for the caller to fix — see [`RUN_WHY_KEY`].
    let mut asking = json!({ RUN_WHY_KEY: unanswered.why().wire_str() });
    if let Some(question) = unanswered.question() {
        // The SHARED renderer, so this surface and the pane list cannot come to spell one question
        // two ways — see [`crate::wire::ASKING_KEY`]. `why` is merged over it rather than passed in
        // because it is the one member a RUN owes and a pane does not.
        let Value::Object(rendered) = crate::agent::question_json(question) else {
            unreachable!("the shared renderer answers an object");
        };
        for (key, value) in rendered {
            asking[key] = value;
        }
    }
    Some(asking)
}

/// THE SENTENCE behind an `asking.why` word — what a person or an agent is told to DO about a run
/// that stopped on its peer's question, or the word itself when this build does not know it.
///
/// # ⚠⚠ Why the mouths read this and not the type
///
/// [`sprag_plugin::Refusal`] owns the sentence, and both mouths must say the SAME one — which is
/// the whole reason it lives on the type rather than in a renderer. But the agent-facing mouth
/// depends on this crate and not on the plugin crate, so reaching the type would mean a second
/// binary carrying the whole plugin layer to read six strings. The host already owns every other
/// wire↔type projection a mouth needs ([`outcome_word`], [`outcome_from_words`]); this is one more,
/// and it delegates rather than spelling a variant.
///
/// ⚠ An UNKNOWN word answers itself rather than nothing. A newer daemon may name a reason an older
/// mouth predates, and printing the raw word is honest where silence would be a run that stopped
/// for no stated cause — the rule [`RUN_CEILING_KEY`] follows for the same reason.
#[must_use]
pub fn refusal_sentence(word: &str) -> String {
    sprag_plugin::Refusal::parse(word)
        .map_or_else(|| word.to_owned(), |why| why.describe().to_owned())
}

/// [`outcome_word`] / [`outcome_ceiling`] READ BACK — how a restored run recovers the state it was
/// written out under.
///
/// ⚠ An unreadable pair answers [`OutcomeState::Failed`] rather than guessing a happier one: a
/// record this build cannot parse is one it must not report as having converged.
#[must_use]
pub fn outcome_from_words(word: Option<&str>, ceiling: Option<&str>) -> OutcomeState {
    match word {
        Some("converged") => OutcomeState::Converged,
        Some("cancelled") => OutcomeState::Cancelled,
        // ⚠ The QUESTION is not restored. It was read off a pane that a restart has outlived, and
        // a question re-published from a durable record would be a claim about a screen nobody has
        // looked at since. The WORD survives, which is what tells a reader the run wants an answer.
        Some("blocked") => OutcomeState::Blocked(None),
        // ⚠⚠ READ THROUGH THE TYPE'S OWN LIST, not a match over the words this file knows. It
        // matched two by hand and answered `Iterations` for everything else, so the fourth ceiling
        // (`turns`, the loop's own budget) would have come back from a restart as *"you ran out of
        // steps"* — a false sentence pointing at a guardrail that run never met.
        Some("exhausted") => OutcomeState::Exhausted(
            ceiling
                .and_then(Ceiling::from_wire)
                // A record with no ceiling word at all predates the key or was truncated; name the
                // one bound EVERY run has, which is still true of anything that got here.
                .unwrap_or(Ceiling::Iterations),
        ),
        _ => OutcomeState::Failed,
    }
}

/// **WHAT BECAME OF A PERSON'S STAND-DOWN ORDER**, weighed against where the run actually got to —
/// register item 594, and the sentence [`RUN_STOOD_DOWN_KEY`] carries.
///
/// # ⚠⚠⚠⚠⚠ The two facts have to be read TOGETHER or neither answers anything
///
/// The ORDER alone says a person spoke. The ENDING alone says what the run did. `sprag stand-down`
/// promises *the work is kept*, and that promise is true of exactly one pairing — an order standing
/// over a run that reached a milestone — while every other pairing is the promise BROKEN, which is
/// the case a reader has to be able to see. Publishing the order as a bare `true` would have handed
/// every mouth the job of doing this arithmetic, and R431's rule says what happens then: the
/// broken state and the healthy state render identically and nobody can tell.
///
/// ⚠⚠ **WHAT IT DOES NOT CLAIM.** A converged run's work is banked whatever closed it, so this
/// says the order STOOD while the run converged — never that the order CAUSED the convergence. That
/// distinction belongs to the loop document, which spells it `DoneReason::StoodDown` into the
/// walk; the host cannot see it and does not guess. ⚠ The residue is registered rather than hidden:
/// a run that converged on its own a moment after the order gets this same sentence, and it is
/// true of that run too.
///
/// ⚠ The outcome's own word comes from [`outcome_word`], so the host never spells a variant here —
/// a seventh [`OutcomeState`] gets its sentence on the day it exists rather than a silent omission.
#[must_use]
pub fn stand_down_sentence(state: &crate::runs::RunState) -> String {
    /// What every ending that is not a convergence has to tell the reader, in one place so the
    /// three of them cannot drift into three different degrees of bad news.
    ///
    /// ⚠⚠ IT DOES NOT SAY *"never reached a milestone"*, and that is the same measurement the
    /// running arm below is written from: a milestone is `ai_loop.scxml`'s concept and this
    /// renderer cannot see which plugin it is holding. *Cut short* is true of all of them.
    const CUT_SHORT: &str = "it was cut short, so it did not stop at a milestone of its own \
                             choosing — this is not what `sprag stand-down` promised. Nothing here \
                             counted its completed work, so this ending cannot say what was kept";
    /// **WHAT BECAME OF THE WORK**, for an ending that is not a convergence — read from the run
    /// rather than asserted about it.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this stopped being one constant
    ///
    /// One constant — it said *the turn it had going was NOT banked* — was printed for EVERY such
    /// ending, and register item 604 measured the
    /// ordinary case that makes it false: an agent finishes a turn under a standing order and then
    /// EXITS, which is what a finished agent does. The run ends `peer_gone` with its work recorded,
    /// and the person was told they lost it. **The alarming answer and the relieved one were
    /// swapped** — the one direction a report must never be wrong in, and the pair item 594 exists
    /// because of.
    ///
    /// ⚠⚠⚠ **THE PLUGIN IS WHAT ANSWERS**, in its own unit ([`sprag_plugin::Banked`]). This
    /// renderer holds a `RunState` and cannot see which plugin produced it — the same reason its
    /// sibling above refuses the word *milestone* — so a sentence it composed out of its own belief
    /// was always going to be wrong for somebody.
    ///
    /// ⚠⚠ THE THREE ANSWERS ARE THREE SENTENCES, because they are three different things to do
    /// next: work that is kept needs nothing, work that never started is not a loss, and a plugin
    /// that does not count leaves a person to look — and saying so is better than inventing a zero
    /// on its behalf.
    fn work_after(outcome: &Outcome) -> String {
        banked_after(outcome.banked.as_ref())
    }
    /// [`work_after`]'s body, over the VALUES rather than an [`Outcome`] — so a run whose ending
    /// arrived from another process reaches the same three sentences.
    ///
    /// ⚠⚠⚠ Split out rather than duplicated: these three answers are register item 604's finding,
    /// and a second copy for reported endings would be free to swap the alarming answer for the
    /// relieved one exactly where that item measured the cost.
    fn banked_after(banked: Option<&sprag_plugin::Banked>) -> String {
        match banked {
            Some(banked) if banked.completed > 0 => {
                let unit = if banked.completed == 1 {
                    banked.unit.clone().into_owned()
                } else {
                    format!("{}s", banked.unit)
                };
                format!(
                    "the {} {unit} it had already completed are BANKED and kept; only whatever it \
                     had in flight is lost",
                    banked.completed,
                )
            }
            Some(_) => {
                "it had completed nothing yet, so there was no banked work to lose".to_owned()
            }
            // ⚠⚠⚠⚠⚠ **AND THIS ARM MUST NOT CLAIM A LOSS EITHER.** Nobody counted, so *the turn
            // it had going was NOT banked* is the same unwarranted claim one branch over, made
            // about a run this renderer knows even less about. A RESTORED run reaches here — the
            // log carries a summary and not a count — and so does any plugin with no unit of
            // completed work. Naming what is missing is the only honest answer available.
            None => "this run does not report completed work, so its ending cannot say what was \
                     kept — its walk is where that is readable"
                .to_owned(),
        }
    }
    match state {
        // ⚠⚠⚠⚠⚠ IT SAYS THE ORDER AND NOT WHAT THE ORDER WILL DO, and that is a measurement rather
        // than caution — register item 539's sibling. `RunOrder::StandDown` is delivered to EVERY
        // run and exactly ONE plugin reads it: `RunContext::stood_down` has a single caller in the
        // whole tree (`OuterLoop::pump`). So *it stops at its next milestone* is true of an
        // `ai_loop` run and false of an `orchestrator`, a `pipe` or an `agent` one, and this
        // renderer cannot see which it is holding. The loop-specific promise stays where a gate can
        // hold it to the document — `sprag_plugin::STAND_DOWN_TAKES_EFFECT`, printed by the command
        // — and what a RUNNING run publishes is the fact plus where the answer will come from.
        crate::runs::RunState::Running => {
            "a person asked this run to stand down; it has not stopped yet — its ending is what \
             says whether the order landed"
                .to_owned()
        }
        crate::runs::RunState::Done { outcome, .. } => {
            if outcome.state == OutcomeState::Converged {
                // ⚠ *ended on its own terms* rather than *stopped at a milestone*, for `CUT_SHORT`'s
                // reason: convergence is what every plugin's own `Verdict` can say, and only one of
                // them has milestones to stop at.
                "a person asked this run to stand down and it converged, so it ended on its own \
                 terms and its work is banked"
                    .to_owned()
            } else {
                // ⚠⚠⚠⚠⚠ **TWO FACTS, AND NEITHER MAY EAT THE OTHER.** *The order was not honoured*
                // is register item 594's, and it is true of every ending that is not a convergence
                // whatever became of the work: the run did not stop at a milestone of its own
                // choosing. *What became of the work* is item 604's, and it is a separate question
                // the plugin answers. Collapsing them is how this line got wrong the first time —
                // one constant carried both and could only be right about one.
                format!(
                    "⚠ a person asked this run to stand down and it ended {:?} instead — this is \
                     not what `sprag stand-down` promised; {}",
                    outcome_word(outcome),
                    work_after(outcome),
                )
            }
        }
        // ⚠⚠⚠⚠⚠ A RUN THAT ENDED IN ANOTHER PROCESS — register items 650 and 544. The ending WORD
        // crossed the wire, so the first half of this sentence is as sound as its `Done` sibling's.
        //
        // ⚠⚠⚠ **WHAT BECAME OF THE WORK DID NOT CROSS, AND THIS SAYS SO RATHER THAN GUESSING.**
        // `outcome_to_json` does not carry `banked`, so `work_after`'s three answers are
        // unavailable here — and the pair register item 604 measured is exactly the pair where a
        // guess swaps the alarming answer for the relieved one. An honest *this ending cannot say*
        // is the one thing that is never wrong in that direction, and it names the repair.
        crate::runs::RunState::Reported(reported) => {
            let word = reported.get("state").and_then(Value::as_str);
            // ⚠⚠⚠ ASKED OF THE TYPE, never spelled here — `outcome_word`'s own rule, and it matters
            // more on this arm than on its sibling: a literal `"converged"` in this file would be a
            // SECOND author of a word `OutcomeState::wire_str` already owns, and the two would
            // drift the day upstream renamed it, silently and only for out-of-process runs.
            if word == Some(OutcomeState::Converged.wire_str()) {
                "a person asked this run to stand down and it converged, so it ended on its own \
                 terms"
                    .to_owned()
            } else {
                // ⚠⚠⚠ THE SAME TWO FACTS AND THE SAME TWO AUTHORS as the `Done` arm above —
                // register item 650 closed. *The order was not honoured* is item 594's, and *what
                // became of the work* is item 604's, read off the wire and composed by the ONE
                // function that composes it. This arm used to say `this cannot say what became of
                // the work`, honestly, because the render dropped `banked`; it carries it now.
                format!(
                    "⚠ a person asked this run to stand down and it ended {:?} instead — this is \
                     not what `sprag stand-down` promised; {}",
                    word.unwrap_or("unreported"),
                    banked_after(banked_reported(reported).as_ref()),
                )
            }
        }
        // ⚠ A DRIVER THAT DIED IS NOT AN ENDING THE DOCUMENT CHOSE, so it gets its own clause: the
        // remedy is to look at why the thread went, where the arm above sends a reader to the
        // outcome word.
        crate::runs::RunState::Panicked(_) => {
            format!(
                "⚠ a person asked this run to stand down and its driver died first — {CUT_SHORT}"
            )
        }
        // ⚠⚠ THE ONE THIS ITEM WAS MEASURED ON. A daemon restarted under a standing order used to
        // erase the order entirely, so a person came back to a bare `interrupted` and no way to
        // learn that the thing they asked for had never happened.
        crate::runs::RunState::Interrupted => {
            format!(
                "⚠ a person asked this run to stand down and the daemon driving it died first — \
                 {CUT_SHORT}"
            )
        }
    }
}

/// **WHAT A REPORTED ENDING SAYS IT KEPT**, or [`None`] where it counts no completed work — the one
/// place a [`RUN_BANKED_KEY`] object becomes a [`sprag_plugin::Banked`] again.
///
/// # ⚠⚠⚠ Why this is the only decoder in register item 650, when the item was about four fields
///
/// The other three the render dropped are read by NOBODY on this side: `screened`, `deliveries` and
/// `checks` reach a person through the run's PROGRESS, which a driver reports separately and whole.
/// This one is different because [`stand_down_sentence`] weighs it against the ending, and a
/// sentence cannot be composed from a value that did not arrive.
///
/// ⚠⚠ **AND IT NEEDS NO NEW TYPE.** [`sprag_plugin::Banked::unit`] is a `Cow` and its doc says why:
/// *a run READ AFTER A RESTART hands over a word decoded from the daemon's log, and there is no
/// `'static` to borrow it from*. A word off a socket is that same case, so the sentence written for
/// the durable log pays for this too — which is what a type admitting both costs looks like when it
/// comes good.
///
/// ⚠ A malformed object reads as [`None`], which is the honest answer rather than a lenient one: a
/// report this build cannot understand is one that told it nothing, and *this run does not report
/// completed work* is exactly what a reader must then be told.
fn banked_reported(reported: &Value) -> Option<sprag_plugin::Banked> {
    let banked = reported.get(RUN_BANKED_KEY)?;
    Some(sprag_plugin::Banked {
        completed: u32::try_from(banked.get("completed")?.as_u64()?).ok()?,
        unit: std::borrow::Cow::Owned(banked.get("unit")?.as_str()?.to_owned()),
    })
}

/// **WHO RAISED THE CANCEL, WEIGHED AGAINST WHAT ACTUALLY ENDED THE RUN** — register item 596, and
/// the sentence [`RUN_CANCELLED_BY_KEY`] carries.
///
/// # ⚠⚠⚠⚠⚠ A cancel is RAISED; it is not always what ends the run
///
/// Measured while building this key, by the mutation that was supposed to prove something else: a
/// run whose pane never showed its readiness marker ended `failed`, and a cancel raised over it a
/// moment later still made this key say *"a person cancelled this run, so the turn it was in the
/// middle of was thrown away"*. **That sentence was false about that run** — its turn was ended by
/// the readiness bound and the person's cancel arrived at a run that was already over.
///
/// That is [`stand_down_sentence`]'s finding one key over, and it has the same answer: **the ORDER
/// alone says somebody spoke, and only the ORDER weighed against the ENDING says what became of
/// it.** A renderer that reports the first and lets the reader assume the second sends them looking
/// for a decision that explains an ending it did not cause — which is the search register item 594
/// recorded and could not end.
///
/// ⚠⚠ **THE CANCELLER'S OWN WORDS SURVIVE INTO BOTH ARMS**, because who raised it is the fact this
/// item exists to publish and it is true either way. What changes is whether the sentence claims
/// the cancel is what finished the run.
///
/// ⚠ The ending's word comes from [`outcome_word`], so the host never spells an [`OutcomeState`]
/// here — a new one gets its sentence on the day it exists rather than a silent omission.
#[must_use]
pub fn cancel_sentence(who: crate::runs::Canceller, state: &crate::runs::RunState) -> String {
    // ⚠⚠⚠⚠⚠ THE TENSE-NEUTRAL PHRASE, and it is the DEFAULT here rather than the exception —
    // `Canceller::raiser`'s own doc holds the measurement. Every arm below except one is about a
    // run this cancel did NOT finish, and `describe` would put the word *cancelled* on all of them.
    let raised = who.raiser();
    match state {
        // ⚠⚠ THE RUN IS STILL GOING, so nothing has been thrown away yet and the sentence must not
        // say it has. `describe()` is written in the past tense about a finished run, so the
        // running arm says the fact and sends the reader to the ending, which is the shape
        // `stand_down_sentence`'s own running arm takes.
        crate::runs::RunState::Running => format!(
            "a cancel has been raised over this run and it has not stopped yet — its ending is \
             what says whether the cancel is what stopped it. Who raised it: {raised}"
        ),
        crate::runs::RunState::Done { outcome, .. } => {
            if outcome.state == OutcomeState::Cancelled {
                // ⚠ THE ONE ARM ENTITLED TO THE ENDING WORD, so `describe` and not `raiser`: this
                // run really was finished by this cancel, and the sentence may say so.
                who.describe().to_owned()
            } else {
                format!(
                    "⚠ a cancel was raised over this run and it ended {:?} instead, so the cancel \
                     is NOT what finished it and the ending is the thing to read. Who raised the \
                     cancel: {raised}",
                    outcome_word(outcome),
                )
            }
        }
        // ⚠⚠⚠⚠ A RUN THAT ENDED IN ANOTHER PROCESS, AND THIS ARM LOSES NOTHING — unlike its
        // `stand_down_sentence` counterpart, which had to admit that what was KEPT did not cross.
        // What this weighs is the ending WORD against the cancel, and the word is the one thing
        // `outcome_to_json` has always carried. So a reported ending answers here at full strength,
        // and the difference between the two arms is a fact about the wire form rather than about
        // where the driver lived (register item 650).
        crate::runs::RunState::Reported(reported) => {
            match reported.get("state").and_then(Value::as_str) {
                // ⚠ `describe` for the same reason the `Done` arm uses it: this run really was
                // finished by this cancel. Asked of the type, never spelled here.
                Some(word) if word == OutcomeState::Cancelled.wire_str() => {
                    who.describe().to_owned()
                }
                word => format!(
                    "⚠ a cancel was raised over this run and it ended {:?} instead, so the cancel \
                     is NOT what finished it and the ending is the thing to read. Who raised the \
                     cancel: {raised}",
                    word.unwrap_or("unreported"),
                ),
            }
        }
        // ⚠ A DRIVER THAT DIED, and a DAEMON that died, each get the `stand_down_sentence` arm of
        // the same name: neither is an ending anybody chose, so the cancel is not what did it.
        crate::runs::RunState::Panicked(_) => format!(
            "⚠ a cancel was raised over this run and its driver died first, so the cancel is NOT \
             what finished it. Who raised the cancel: {raised}"
        ),
        crate::runs::RunState::Interrupted => format!(
            "⚠ a cancel was raised over this run and the daemon driving it died first, so the \
             cancel is NOT what finished it. Who raised the cancel: {raised}"
        ),
    }
}

/// **WHAT A RUN'S PROMPTS LOOK LIKE FROM THE PANE**, or [`None`] for a run that has delivered
/// none — register item 591, and the sentence both mouths print for [`RUN_FOLDED_KEY`].
///
/// # ⚠⚠⚠⚠ Why a sentence and not two numbers on a row
///
/// The fact a person acts on is a RATIO and not a count, and the act is *do I go and look at that
/// pane?* Handed `delivered 14, folded 14` a reader has to notice the two are equal; handed
/// `delivered 14, folded 3` they have to notice they are not. **Both readings are one comparison
/// away from the opposite conclusion**, and the mouths this project has are read by tired people
/// and by agents — which is the argument `stand_down_sentence` is built on one key over.
///
/// ⚠ The numbers travel too ([`RUN_DELIVERED_KEY`] / [`RUN_FOLDED_KEY`]), so a caller that wants
/// the ratio for itself is not made to parse prose. This is the reading, not the record.
///
/// # ⚠⚠ Why it takes the RUN and not a [`sprag_plugin::Deliveries`]
///
/// [`refusal_sentence`]'s reason, one key over: the agent-facing mouth depends on this crate and
/// NOT on the plugin crate, so a typed argument would make that binary carry the whole plugin layer
/// to read two integers. And there is a second reason this one has and that one does not — a
/// parameter would put the key-reading at every call site, which is two mouths spelling one
/// projection twice, the exact drift this file's `outcome_to_json` is `pub` to prevent.
#[must_use]
pub fn delivery_sentence(run: &Value) -> Option<String> {
    /// One count off a run's answer, saturating rather than wrapping — a number too large to be a
    /// `u32` is a defect somewhere else, and a reader must not be told a small one instead.
    fn count(run: &Value, key: &str) -> u32 {
        run[key]
            .as_u64()
            .unwrap_or_default()
            .try_into()
            .unwrap_or(u32::MAX)
    }
    let deliveries = sprag_plugin::Deliveries {
        made: count(run, RUN_DELIVERED_KEY),
        folded: count(run, RUN_FOLDED_KEY),
        unsubmitted: count(run, RUN_UNSUBMITTED_KEY),
    };
    // ⚠⚠⚠⚠⚠ **A PROMPT NOBODY WAS ASKED IS SAID FIRST, BECAUSE IT IS THE ONE STILL SITTING
    // SOMEWHERE** — register item 617. It is read BEFORE the zero-denominator return below, and
    // that ordering is the whole repair: a wedged run has no deliveries by definition (nothing was
    // asked), so the early return was swallowing the one run register item 591 was built for.
    //
    // ⚠⚠ THE INSTRUCTION IS THE OPPOSITE OF THE FOLD CLAUSES BELOW. Those say *do not go and look
    // at that pane*; this says **go and look, it is there** — so it is its own sentence rather than
    // a number folded into theirs, on `delivery_sentence`'s own stated argument that the fact a
    // person acts on is the reading and not the count.
    if deliveries.unsubmitted > 0 {
        let wedged = format!(
            "⚠ {} prompt(s) reached that pane and were never asked — the text is sitting in its \
             composer, so go and look at that pane: what is there is a question nobody put",
            deliveries.unsubmitted,
        );
        if deliveries.made == 0 {
            return Some(wedged);
        }
        // A run that delivered AND wedged has both readings, and neither survives being dropped for
        // the other: the earlier prompts are somewhere, and the last one is somewhere else.
        return Some(match delivered_clause(deliveries) {
            Some(delivered) => format!("{wedged}. {delivered}"),
            None => wedged,
        });
    }
    if deliveries.made == 0 {
        return None;
    }
    delivered_clause(deliveries)
}

/// What became of the prompts this run actually DELIVERED — [`delivery_sentence`]'s fold reading,
/// lifted out so the wedged clause above can carry it too.
///
/// ⚠ Split rather than duplicated: two spellings of *all of them were folded* would differ first in
/// whichever one a later round forgot, which is the failure `delivery_sentence`'s own doc argues
/// against one function up.
fn delivered_clause(deliveries: sprag_plugin::Deliveries) -> Option<String> {
    if deliveries.made == 0 {
        return None;
    }
    if deliveries.folded == 0 {
        return Some(format!(
            "{} prompt(s) delivered, all of them on that pane",
            deliveries.made,
        ));
    }
    // ⚠⚠⚠ THE READING THAT CHANGES WHAT SOMEBODY DOES, said in words rather than left to
    // arithmetic: every prompt this run sent is invisible where people are sent to look for it.
    if deliveries.all_folded() {
        return Some(format!(
            "⚠ all {} of this run's prompts were folded away by its peer's composer — NONE of them \
             is on that pane, so looking there for one will find a fold and not the text",
            deliveries.made,
        ));
    }
    Some(format!(
        "⚠ {} of {} prompts were folded away by its peer's composer and are not on that pane",
        deliveries.folded, deliveries.made,
    ))
}

/// **WHY A RUN A BOOT READ OUT OF A PREDECESSOR'S LOG IS NOT COMING BACK** — register item 737, and
/// the sentence [`RUN_WITHHELD_KEY`] carries.
///
/// # ⚠⚠⚠⚠⚠ One spelling, read twice, exactly as `Revival::not_put_back` is
///
/// The boot writes it to the operator's log beside the run id, and the row carries it to whoever
/// opens `sprag runs` afterwards — and a promotion's whole point is that the second person need not
/// be the first. Two compositions of *why did my loop not come back* would be free to disagree
/// about the same run, which is the shape `crate::runs::Revival::not_put_back` already refuses one
/// door over.
///
/// # ⚠⚠⚠ Why the foreign-document arm names BOTH fingerprints
///
/// Because the fact a reader acts on is a COMPARISON — [`delivery_sentence`]'s argument two
/// functions down — and half of it is useless: *your run was recorded against `091c2616…`* leaves
/// the person to go and find what this build's documents hash to before they know whether anything
/// is wrong. With both, the row itself says *these are two different builds' documents*, and the
/// remedy (start it again; it is a new run by construction) follows from the sentence.
///
/// ⚠ It says what was DECIDED and not that something failed: item 544 chose this — a configuration
/// read against a document it did not come from decodes cleanly and is wrong — so the sentence
/// carries the decision rather than an apology for it.
#[must_use]
pub fn withheld_sentence(why: &crate::runs::Withheld) -> String {
    match why {
        crate::runs::Withheld::ForeignDocuments { theirs } => format!(
            "its machine's position was recorded against documents this build does not have \
             (it recorded {theirs}; this build's documents are {}), so no successor can put it \
             back — a changed document makes a NEW run, deliberately. Start it again",
            sprag_plugin::STATECHARTS_FINGERPRINT,
        ),
        // ⚠ NOT A FAULT AND SAID AS ONE FACT: a run that completed no step, and a plugin that walks
        // no statechart at all, are the same thing to a reader — there is no position to return to.
        crate::runs::Withheld::NoPlace => "the daemon that held it never recorded where its \
             machine was, so there is no position to put it back at"
            .to_owned(),
        crate::runs::Withheld::NoDocument => "its position was recorded with no fingerprint \
             beside it, so nothing can say which documents those words came from and reading them \
             here would be a guess"
            .to_owned(),
        crate::runs::Withheld::NoRequest => "its position is one this build can read, and nothing \
             recorded what the run was asked with, so no plugin could be rebuilt to enter at it"
            .to_owned(),
    }
}

/// **WHETHER ANYTHING INDEPENDENT VERIFIED THIS RUN'S MILESTONES**, or [`None`] for a run that put
/// no claim to a checker — register item 601, and the sentence [`RUN_CHECKS_KEY`] carries.
///
/// # ⚠⚠⚠⚠ The two absences a reader must never confuse
///
/// `asked == 0` is a run whose document **authored no checker**: a decision its author took, which
/// `sprag_plugin::outer::Checked::NotAsked` names and which is not a fault. `asked > 0, silent ==
/// asked` is a checker that was declared and never worked once. Telling somebody to *fix the
/// checker* in the first case sends them after a thing that was never meant to exist, so the first
/// answers `None` and says nothing at all.
///
/// ⚠⚠ **AND THE MIDDLE CASE IS NOT SILENCE.** A run where some checks answered and some did not
/// still converged on ones that were verified or on ones that were not, and a reader has to be able
/// to weigh that — so it gets a sentence of its own rather than being folded into either end.
///
/// ⚠ The reason for the last silence is `judge::Unheard`'s own words, carried rather than composed:
/// one authority on what a silence means.
///
/// # ⚠⚠⚠⚠⚠ And what became of the claims the checker DID answer — register item 499
///
/// A refusal is the check working, so it never replaces the readings above: those are about whether
/// anything was verified at all, and this is about what the verdicts came to. It rides as a clause
/// on whichever of them applies, for [`delivery_sentence`]'s reason one function up — *the fact a
/// reader acts on is a comparison*, and the comparison here is a DEPTH against a ceiling.
///
/// ⚠⚠ **THE DEPTH IS SAID EVEN WHEN IT IS ONE, AND THAT IS THE MEASUREMENT RATHER THAN NOISE.** The
/// number `reflect_after_refusals` is set to was authored against a distribution nobody had, and
/// *every refusal stood alone* is precisely the reading that defends or withdraws it. A clause that
/// appeared only once a run got into trouble would leave the ordinary case — the one that says the
/// ceiling is slack — unmeasured all over again.
#[must_use]
pub fn checks_sentence(checks: &sprag_plugin::Checks) -> Option<String> {
    // ⛔⛔⛔⛔⛔ **THE CLAIMS THAT COULD NOT BE PUT ARE SAID FIRST, AND THEY ARE SAID EVEN WHEN
    // `asked` IS ZERO** — register item 674.
    //
    // `asked == 0` used to return `None` outright, on the reading that it means *this author
    // declared no checker*. That is one of two worlds: a run whose datamodel could not answer for
    // `milestone_check` also reaches zero, and it is the loop's instrument failing. Returning
    // `None` for it is how a run that verified NOTHING printed the same row as a run nobody meant
    // to verify — which is the whole of item 674's remaining half.
    let unasked = match checks.unasked {
        0 => String::new(),
        n => format!(
            "⚠ {n} milestone claim(s) could not be put to a checker at all — this run could not \
             read its own `milestone_check`, so nothing was asked and nothing could answer. That \
             is this loop's instrument, not its checker. "
        ),
    };
    if checks.asked == 0 {
        // ⚠ A run with nothing asked and nothing unaskable is the author's decision, and stays
        // silent exactly as before.
        return (!unasked.is_empty()).then(|| unasked.trim_end().to_owned());
    }
    let why = checks
        .why_silent
        .as_deref()
        .map_or_else(String::new, |why| format!(" ({why})"));
    let refused = refusal_clause(checks);
    if checks.silent == 0 {
        // ⚠⚠ THE `unasked` CLAUSE LEADS EVEN HERE, and this is the arm it matters most on: *every
        // one of them answered* is the reassuring reading, and it is true only of the claims that
        // were put. A run that could not put three of them would otherwise print an unqualified
        // success — register item 674.
        return Some(format!(
            "{unasked}{} milestone claim(s) went to an independent checker and every one of them \
             answered{refused}",
            checks.asked,
        ));
    }
    // ⚠⚠⚠ THE READING THAT CHANGES WHAT SOMEBODY DOES: nothing outside this run verified anything
    // it converged on, so the ending rests on the working agent's own word — register item 428's
    // whole reason for existing, arriving where a person reads it.
    if checks.none_answered() {
        return Some(format!(
            "{unasked}⚠ NONE of this run's {} milestone claim(s) was verified — its checker never \
             answered, so anything it converged on rests on the working agent's own word{why}",
            checks.asked,
        ));
    }
    Some(format!(
        "{unasked}⚠ {} of {} milestone claims went unverified — the checker answered for the \
         rest{why}{refused}",
        checks.silent, checks.asked,
    ))
}

/// **WHAT THE VERDICTS CAME TO**, as a clause [`checks_sentence`] appends — empty for a run nothing
/// refused.
///
/// # ⚠⚠⚠⚠⚠ Register item 499: the clause exists so a ceiling can be defended or withdrawn
///
/// `ai_loop.scxml`'s `reflect_after_refusals` bounds refusals **IN A ROW**, and until this ran
/// nothing outside a run's own walk counted them at all. The two numbers are said together on
/// purpose: a total says how often the checker disagreed, and only the depth says whether the
/// ceiling was ever approached — thirty refusals one apiece and fifteen pairs are the same total
/// and opposite facts about the bound.
///
/// ⚠⚠ **THE DEPTH IS NEVER OMITTED WHEN IT IS ONE.** *Every refusal stood alone* is the finding
/// that says a ceiling of two was slack, and a clause that only spoke up on trouble would publish
/// the exceptional case and hide the measurement.
///
/// ⚠ It says nothing about what the ceiling IS. That number is the loop KIND's — `sprag_plugin`'s
/// `LoopKind::reflect_after_refusals`, one document per repository — and a renderer that quoted one
/// would be a second author of a decision it cannot see.
fn refusal_clause(checks: &sprag_plugin::Checks) -> String {
    if checks.refused == 0 {
        return String::new();
    }
    format!(
        ", and {} of them the checker refused — at most {} in a row, which is the depth a \
         reflection ceiling is set against",
        checks.refused, checks.refused_in_a_row,
    )
}

/// ⛔⛔⛔⛔⛔ **WHAT THE DOOR TOOK, SAID ONCE, WHERE THE CALLER WILL READ IT** — register item 719's
/// second direction, and the row's counterpart to [`checks_sentence`] beside it.
///
/// # ⚠⚠⚠ Why the row composes the prose and the driver does not
///
/// [`checks_sentence`]'s rule, item 663: the LEVEL crosses as numbers and there is exactly one
/// composer of the sentence made from them. A driver in another process reports the three byte
/// counts it measured; this is the only place that turns them into a reading, so the answer to
/// *which part should I shorten* cannot be given two different ways by two builds.
///
/// ⚠⚠ **A BRIEF OF NOTHING SAYS NOTHING.** A run whose three parts are all empty is a run nobody
/// briefed in any sense a reader acts on, and a line about zero bytes on such a row is noise on the
/// common path — the discipline `checks_sentence` applies to `asked == 0` one function up.
///
/// ⚠ [`sprag_plugin::Briefing::describe`] is what it delegates to, because the caveat that makes
/// the number readable is the plugin's measurement and not this layer's opinion.
#[must_use]
pub fn briefing_sentence(briefed: sprag_plugin::Briefing) -> Option<String> {
    (briefed.bytes() > 0).then(|| briefed.describe())
}

/// A run's OUTCOME as a client receives it — the projection both mouths render from.
///
/// ⚠ `pub` for the reason [`outcome_word`] beside it is: a mouth's gate has to drive the DAEMON's
/// renderer rather than a hand-written copy of its answer shape, or the gate passes while the two
/// drift. That is the two-readers defect this crate has paid for repeatedly, and a fixture spelling
/// `{"state": …, "asking": …}` itself would be a fresh instance of it.
#[must_use]
pub fn outcome_to_json(outcome: &Outcome) -> Value {
    let (state, ceiling) = (outcome_word(outcome), outcome_ceiling(outcome));
    // Cost is self-describing on the wire: the scalar amount plus its unit label
    // (both from `Cost` itself, so the host never names a variant), so a peer
    // reads it without knowing which plugin ran. A `null` unit means no measured
    // step (e.g. cancelled before any step ran).
    let (cost, unit) = outcome
        .cost
        .map_or((0, None), |c| (c.amount(), Some(c.unit())));
    let mut answer = json!({
        "state": state,
        "iterations": outcome.iterations,
        "cost": cost,
        "unit": unit,
        // ⚠ ALWAYS, including `0` — see `RUN_ANSWERED_KEY`. A decision taken on somebody's behalf
        // must be readable as a claim and not inferred from a key nobody wrote.
        RUN_ANSWERED_KEY: outcome.answered,
        // ⚠ THE SENTENCE, not the variant. This was `format!("{e:?}")` — `Write("Broken pipe (os
        // error 32)")` reaching an agent, which is R283's leak on the loop's own answer.
        "failure": outcome.failure.as_ref().map(ToString::to_string),
    });
    // WHICH CEILING, present only when there was one — so the key's presence is itself the claim,
    // the rule `run_to_json` follows for `opened_by`. `exhausted` with no ceiling beside it told a
    // caller to change something without saying what, and the three ceilings have three different
    // remedies.
    if let Some(ceiling) = ceiling {
        answer[RUN_CEILING_KEY] = json!(ceiling);
    }
    // AND WHAT THE PEER IS ASKING, present only when there is a question to publish — the same
    // presence-is-the-claim rule. A `blocked` run with no `asking` beside it is one whose peer
    // stopped on something this host could not read, which is a different remedy: a person.
    if let Some(asking) = outcome_question(outcome) {
        answer[RUN_ASKING_KEY] = asking;
    }
    // AND WHAT BECAME OF THE WORK, present only for a run that was cut short — see
    // `RUN_STOPPED_KEY`. The SENTENCE and not the variant, for the reason `failure` above is one.
    if let Some(stopped) = &outcome.stopped {
        answer[RUN_STOPPED_KEY] = json!(stopped.to_string());
    }
    // ⚠⚠⚠⚠⚠ AND WHAT THE RUN KEPT — register item 650, the last field this render dropped.
    //
    // It is here because this function stopped being a render for a person and became a TRANSPORT:
    // a run driven by another process (items 544 / 643) computes its ending over there, and every
    // reader on this side has to end up weighing the same one. Three of the four could already —
    // the word, the ceiling, the capture all crossed — and `stand_down_sentence` could not, because
    // what it asks is *what became of the work* and the answer lived only in this struct.
    //
    // ⚠⚠ **PRESENT ONLY WHEN THE PLUGIN COUNTS**, which is the `Option`'s whole content and must
    // survive the trip: absent means *this plugin does not count completed work at all*, and
    // `{"completed": 0}` means *it counts, and there was none*. `work_after` says three different
    // things across that pair, and collapsing them would be item 604's swap in a new place.
    //
    // ⚠ THE VALUES AND NOT A SENTENCE. The sentence is `work_after`'s, composed on the far side of
    // this wire from these two numbers — one author, whichever process ran the plugin.
    if let Some(banked) = &outcome.banked {
        answer[RUN_BANKED_KEY] = json!({
            "completed": banked.completed,
            "unit": banked.unit.as_ref(),
        });
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_detect::{AgentState, Report, Ruleset, built_ins};
    use sprag_plugin::PaneAccess;
    use sprag_terminal::CommandBuilder;
    use std::time::Instant;

    /// ⛔⛔⛔⛔⛔ **THREE REFUSALS, THREE SENTENCES, AND ONLY ONE OF THEM IS ABOUT A `<data>`
    /// BLOCK** — register item 510, arriving at the mouth a caller actually reads.
    ///
    /// # ⚠⚠⚠ Why the negative assertion is the load-bearing one
    ///
    /// `OuterLoop::new` returned `Option` until 2026-08-27, so a document the DOOR had refused and
    /// a machine with no script session both reached [`ai_loop_refusal`] as `Undrivable` and were
    /// told *"this build's `ai_loop.scxml` does not carry the strings a loop is driven by"*. Every
    /// word of that is a claim about the datamodel. A reader who acted on it opened the right file
    /// at the wrong place and found nothing wrong, on the one occasion the product had a precise
    /// answer and could not say it.
    ///
    /// So the claim here is not merely *each arm says something*: it is that the phrase belonging
    /// to the ONE cause it is true of appears in exactly ONE of the three.
    ///
    /// ⚠⚠ The sentences are the whole subject, so they are compared as VALUES rather than by
    /// eye — three arms that quietly rendered the same prose would satisfy any per-arm assertion
    /// written one at a time, which is how the collapse got in.
    /// ⛔⛔⛔⛔⛔ **THE ROW SAYS HOW MANY CLAIMS NEVER REACHED A CHECKER, AND SAYS IT EVEN WHEN
    /// `asked` IS ZERO** — register item 674's remaining half, at the surface a person reads.
    ///
    /// # The escape hatch that made the tally optimistic
    ///
    /// `checks_sentence` returned `None` outright for `asked == 0`, on the reading that it means
    /// *this author declared no checker*. It has a second cause — a run whose datamodel could not
    /// answer for `milestone_check` — and for that one the silence is the defect: the claim leaves
    /// the denominator, so **a run that verified nothing prints the same row as a run nobody meant
    /// to verify**. An unclassified case passing as *not applicable* is the shape this register
    /// refuses; it has to be RED, and here that means a sentence.
    ///
    /// ⚠⚠ **THE THIRD ARM IS THE ONE THAT MATTERS MOST.** *Every one of them answered* is the
    /// reassuring reading and it is true only of the claims that were PUT — so a run that could
    /// not put some must not print it unqualified.
    #[test]
    fn a_row_says_how_many_milestone_claims_never_reached_a_checker() {
        let of = |asked, silent, unasked| {
            checks_sentence(&sprag_plugin::Checks {
                asked,
                silent,
                why_silent: None,
                refused: 0,
                refused_in_a_row: 0,
                unasked,
            })
        };

        // ── THE CONTROL: an author who declared no checker stays silent, exactly as before ────
        assert_eq!(
            of(0, 0, 0),
            None,
            "⚠⚠⚠⚠⚠ THE PREMISE THAT KEEPS THE CLAIM HONEST: a run nobody meant to check must \
             still print nothing. If this spoke, the assertion below would be satisfied by a \
             sentence that appears on every run and says only that a run existed",
        );

        // ── AND THE WORLD THAT USED TO SHARE ITS SILENCE ─────────────────────────────────────
        let unaskable = of(0, 0, 2).expect(
            "⛔⛔⛔⛔⛔ REGISTER ITEM 674: a run that could not put ANY of its claims to a checker \
             must not print the same row as a run nobody meant to check. `asked == 0` was an \
             unconditional `None`, so this run — whose instrument failed twice — reported nothing \
             at all, and its milestone claims stood on the working agent's own word in silence",
        );
        assert!(
            unaskable.contains('2') && unaskable.contains("could not be put"),
            "⚠⚠⚠ and it must say HOW MANY and that they were never put — a count with no subject \
             sends a reader to the checker, which is the one thing that is not broken here. \
             Got {unaskable:?}",
        );

        // ── THE ARM THAT WOULD OTHERWISE FLATTER THE CHECKER ─────────────────────────────────
        let reassuring = of(3, 0, 1).expect("a run that asked three says so");
        assert!(
            reassuring.contains("could not be put") && reassuring.contains("every one of them"),
            "⛔⛔⛔⛔ REGISTER ITEM 674: *every one of them answered* is true of the claims that \
             were PUT, and this run could not put one. Printed unqualified it is a checker being \
             flattered by the claims it never saw — the exact reading this item is about. \
             Got {reassuring:?}",
        );
        assert!(
            reassuring.find("could not be put") < reassuring.find("every one of them"),
            "⚠⚠ and the caveat must come FIRST: a reader who stops at the first clause must not \
             stop at the reassuring one. Got {reassuring:?}",
        );
    }

    #[test]
    fn each_reason_a_loop_could_not_be_built_names_its_own_file() {
        /// The clause that is true only of a datamodel short of its authored strings.
        const ABOUT_THE_DATA: &str = "does not carry the strings a loop is driven by";

        let unanswered = ai_loop_refusal(&sprag_plugin::NotStarted::Unanswered(
            sprag_plugin::document::Faulted {
                unanswered: 1,
                error: Some("error.execution"),
                cascaded: 0,
                // ⚠ ZERO, and it is what keeps this case about the UNANSWERED error: register item
                // 551's truncation speaks FIRST in `Faulted`'s `Display`, so a non-zero here would
                // silently retarget the assertions below at a different sentence.
                truncated: 0,
                truncated_at: None,
            },
        ));
        let sessionless = ai_loop_refusal(&sprag_plugin::NotStarted::Sessionless);
        let undrivable = ai_loop_refusal(&sprag_plugin::NotStarted::Undrivable);

        // ⚠⚠⚠ THE FAULT SPEAKS AND THIS LAYER RELAYS IT — `Faulted`'s own `Display` names the
        // class, which is who repairs it: `error.execution` is the document's own content and
        // `error.communication` is a `<send>` this host did not serve. A sentence composed here
        // would be a second author of that distinction.
        assert!(
            unanswered.contains("error.execution") && unanswered.contains("never ran"),
            "⛔⛔⛔ ITEM 510: the door's own words must reach the caller — which error, and that \
             it abandoned the rest of its block. Said: {unanswered:?}",
        );
        assert!(
            !unanswered.contains(ABOUT_THE_DATA),
            "⛔⛔⛔⛔⛔ ITEM 510, THE DEFECT ITSELF: a document the door refused was reported as a \
             datamodel missing its authored strings. That is the wrong half of the right file, \
             and it is worse than saying nothing — it is a confident answer nobody can act on. \
             Said: {unanswered:?}",
        );
        assert!(
            !sessionless.contains(ABOUT_THE_DATA) && sessionless.contains("engine"),
            "⛔⛔⛔ ITEM 510: a build whose engine opened the document with no script session has \
             no datamodel to be missing strings FROM, so it must name the engine instead. \
             Said: {sessionless:?}",
        );
        assert!(
            undrivable.contains(ABOUT_THE_DATA),
            "⚠⚠⚠ and the arm that clause was ALWAYS true of must keep it, or this item traded \
             one wrong sentence for another. Said: {undrivable:?}",
        );
        assert_eq!(
            [&unanswered, &sessionless, &undrivable]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            3,
            "⛔⛔⛔⛔ ITEM 510: three causes, three sentences. Two arms rendering one string is \
             the collapse this item paid for, moved down a layer where no per-arm assertion would \
             notice it",
        );
    }

    /// ⚠⚠⚠ **AND THE TURN CONTRACT REACHES A RUN THROUGH THE DOOR PRODUCTION USES.**
    ///
    /// The plugin crate gates what the contract DOES; this asks whether a caller can get one, which
    /// R373 paid dearly for learning to ask separately — that round's whole feature was unreachable
    /// from every production path while its unit gates were green.
    ///
    /// ⚠⚠ Both runs are started through `RUN_ACTION` — the verb the MCP `orchestrate` tool and the
    /// outer AI loop call — against the same peer thinking for the same three seconds, differing in
    /// the two keys alone. The uncontracted one spends turns re-asking; the contracted one asks
    /// once. **The pair is the claim**: either number alone would be a fact about this machine.
    #[test]
    fn a_turn_contract_sent_over_the_wire_stops_a_run_re_asking_a_slow_peer() {
        /// One `orchestrator` run against a peer that thinks for three seconds, with `extra`
        /// merged into the request — and the turn count it ended on.
        fn turns_taken(extra: Value) -> u32 {
            let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("while read l; do echo THINKING; sleep 3; echo PEER-REPLIED; done");
            command.env("TERM", "xterm-256color");
            let pane = lock(&workspace)
                .spawn(command, "peer".to_string(), 80, 24)
                .expect("spawn the peer");
            let registry = Arc::new(Mutex::new(RunRegistry::default()));
            let mut external = PluginsExternal::new(
                Arc::clone(&workspace),
                Arc::clone(&registry),
                None,
                None,
                None,
                None,
                None,
            );
            let mut request = json!({
                "plugin": "orchestrator",
                "pane": pane.0,
                "stimulus": "ping",
                "sentinel": "PEER-REPLIED",
                "guardrails": { "max_iterations": 100, "max_seconds": 60 },
            });
            let object = request.as_object_mut().expect("an object");
            for (key, value) in extra.as_object().expect("an object") {
                object.insert(key.clone(), value.clone());
            }
            let started = external
                .invoke(RUN_ACTION, IntrospectValue::Json(request))
                .expect("a well-formed run");
            let IntrospectValue::Int(id) = started else {
                panic!("a run answers its id: {started:?}");
            };
            let entry = ended(
                &registry,
                u64::try_from(id).expect("a run id is not negative"),
                Duration::from_secs(40),
            );
            assert_eq!(
                entry["state"]["outcome"]["state"],
                json!("converged"),
                "the peer answers well inside the run's clock: {entry:?}",
            );
            u32::try_from(
                entry["state"]["outcome"]["iterations"]
                    .as_u64()
                    .expect("a turn count"),
            )
            .expect("a turn count fits")
        }

        let uncontracted = turns_taken(json!({}));
        assert!(
            uncontracted > 1,
            "⚠⚠⚠ THE CONTROL: a run that names no turn contract still ends its steps on the \
             plugin's 500 ms constant, so it re-asks a peer that thinks for three seconds. It took \
             {uncontracted}, and if that is ever 1 the comparison below is measuring nothing",
        );
        let contracted = turns_taken(json!({
            "done_when": "exits",
            sprag_plugin::Turn::WIRE_KEY: 12_000,
        }));
        assert_eq!(
            contracted, 1,
            "⚠⚠⚠ AND THE SAME REQUEST PLUS TWO KEYS ASKS ONCE. The uncontracted run took \
             {uncontracted} turns at the same peer; this one took {contracted}. Nothing else \
             differs, so what the pair measures is the contract arriving over the wire",
        );
    }

    /// A pane running a stand-in agent: announces itself, then echoes back every line it is given.
    ///
    /// ⚠ ECHO OFF, so what appears on the screen is what the PROGRAM printed rather than what the
    /// line discipline painted — the difference between measuring a delivery and measuring the
    /// kernel.
    /// ⚠⚠⚠⚠⚠ **THE SAME AGENT FINDS THE RUNS IT STARTED AFTER A RESTART, AND A STRANGER IN ITS
    /// SEAT DOES NOT** — [`crate::runs::RunRegistry::restore`]'s rule 1, driven end to end.
    ///
    /// This is the payoff of the round that re-took that rule, and neither of the `runs.rs` gates
    /// can see it: they prove the conversation SURVIVES, and this proves it is USED — that a
    /// successor turns a surviving conversation back into the seat the agent-facing filter reads.
    /// ⚠ It goes through `PluginsExternal::read(RUNS_SLOT)`, the product's own door, because a gate
    /// that called `seat_of` directly would be green whether or not anything in the product ever
    /// asked it.
    ///
    /// The staging is the whole argument, so it is spelled out:
    ///
    /// * the predecessor's run recorded a CONVERSATION and pane 0 as its seat;
    /// * the successor restores it — seat dropped, conversation kept;
    /// * a pane is then born into the successor **holding that same conversation**, which is what
    ///   `restore_command`'s `--resume <uuid>` does in the product;
    /// * the run must now name THAT pane, whatever id it happens to have.
    ///
    /// ⚠⚠⚠⚠ **THE SEAT IS DELIBERATELY A DIFFERENT NUMBER FROM THE ORIGINAL.** The successor's
    /// pane comes out of a fresh workspace counter, so if this passed by carrying the old id it
    /// would pass for the wrong reason — the very confusion (a seat mistaken for an identity) the
    /// rule exists to end. Asserting the NEW id is what makes it a re-derivation.
    ///
    /// ⚠⚠⚠ **AND THE CONTROL IS A STRANGER IN THE SAME SEAT**, without which "the conversation is
    /// matched" and "any occupant inherits" are the same green — and the second is the hole the old
    /// rule was conservatively guarding against. A pane holding a DIFFERENT conversation must leave
    /// the run unclaimed.
    #[test]
    fn a_restored_run_finds_the_seat_its_own_conversation_is_sitting_in() {
        const RESUMED: &str = "13cac637-d86c-4fa3-8411-785d552cee16";
        const A_STRANGER: &str = "00000000-0000-0000-0000-000000000000";

        // What a predecessor daemon left on disk: unfinished, and it remembers WHO asked.
        let log = crate::runs::RunLog {
            version: crate::runs::RUN_LOG_VERSION,
            runs: vec![crate::runs::PersistedRun {
                id: 0,
                label: "agent pane=0".to_owned(),
                // ⚠ An older log records no request either — item 543. This fixture's run is
                // therefore one nobody could put back, which is what every log held before it.
                request: None,
                iterations: 1,
                cost: None,
                unit: None,
                finished: false,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                driver: None,
                opened_by_session: Some(RESUMED.to_owned()),
                at: None,
                document: None,
                // ⚠ `None` and not `Some(false)` — this fixture IS a log written by an older
                // daemon, so the honest value is *nobody recorded whether an order was given*.
                stood_down: None,
                // ⚠ Likewise, and here `None` needs no such caveat: a canceller is an option
                // already, so *nobody cancelled it* and *nothing was written down* are one answer.
                cancelled_by: None,
                // ⚠ And this fixture IS an older log, so item 606's field is absent by the same
                // argument as `stood_down` above: it reads as `0 of 0`, which claims nothing.
                deliveries: None,
                // ⚠ And item 616's, for that reason exactly — absent reads as *nobody counted*,
                // which is the honest answer for a log written before the column existed.
                banked: None,
                briefed: None,
                // ⚠ An older log carries no place, which is this fixture's shape — item 543.
                place: None,
            }],
        };

        // A successor: its own registry, its own workspace, its own pane counter.
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        lock(&registry).restore(&log);
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));

        // ── THE CONTROL FIRST, so a later pass cannot be the stranger's ──
        let stranger = resumed_pane(&workspace, A_STRANGER);
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        let listed = read_runs(&external);
        assert!(
            listed[0].get(RUN_OPENED_BY_KEY).is_none(),
            "⚠⚠⚠ A STRANGER IN THE SEAT MUST NOT INHERIT THE RUN. This is the hole the old rule \
             guarded by dropping provenance outright, and it stays shut: pane {} holds a different \
             conversation, so nothing here is its. Entry: {:?}",
            stranger.0,
            listed[0],
        );

        // ── AND NOW THE ASKER ITSELF, resumed into the successor ──
        let mine = resumed_pane(&workspace, RESUMED);
        assert_ne!(
            mine.0, 0,
            "the re-derived seat must be a DIFFERENT id from the one the run was started under, or \
             this gate could pass by carrying the old number",
        );
        let listed = read_runs(&external);
        assert_eq!(
            listed[0].get(RUN_OPENED_BY_KEY).and_then(Value::as_u64),
            Some(mine.0),
            "⚠⚠⚠⚠⚠ THE AGENT MUST FIND ITS OWN RUN. It came back `--resume`d into the same \
             conversation, so the successor can say which seat that conversation is in — which is \
             the whole of `RunRegistry::restore`'s rule 1. Entry: {:?}",
            listed[0],
        );
    }

    /// ⚠⚠⚠⚠⚠ **A RUN'S ASKER MAY BE SITTING IN A WINDOW THIS POOL IS NOT** — register item 689.
    ///
    /// # What was wrong, and why four hundred gates were blind to it
    ///
    /// A run carries two panes and only one of them is a target. `pane` is driven and must be in
    /// this pool. `opened_by` is the SEAT THE ASKER IS IN, and nothing obliges that to be the same
    /// window — an agent opening a workbench of its own is the ordinary case, not the exotic one.
    /// Both were checked against the one pool, and nothing noticed because the only mouth that
    /// sends a provenance also only ever drove its own window. Item 687 gave that mouth the window,
    /// and the very next request came back **`no pane 0 in this workspace, so nothing can be opened
    /// by it`** — a refusal about the CALLER, on a call the caller made correctly.
    ///
    /// # ⚠⚠ The fixture's two pools are SIBLINGS, and that is load-bearing
    ///
    /// Two `Workspace::new`s mint from two counters starting at the same number, so the asker could
    /// be handed an id the target's pool ALSO holds — and then "the pool did not have it" would be
    /// false and every claim here would be about nothing. `sibling()` is how the product makes a
    /// second window and shares the one counter, so the ids cannot collide. The premise is asserted
    /// below as well as arranged, because arranging it is not measuring it.
    #[test]
    fn a_runs_asker_is_accepted_from_a_seat_this_pool_does_not_hold() {
        const ELSEWHERE: &str = "a-conversation-in-another-window";
        let here = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let elsewhere = Arc::new(Mutex::new(lock(&here).sibling()));
        let target = echoing_agent_pane(&here);
        let asker = resumed_pane(&elsewhere, ELSEWHERE);

        // ── THE PREMISE, ARRANGED AND THEN ASSERTED ─────────────────────────────────────────────
        assert!(
            lock(&here).pane(asker).is_none(),
            "⛔ THE PREMISE: the asker's seat must be one this pool does NOT hold, or the claim \
             below passes on a build that never looks further than the pool",
        );

        let pool = Arc::clone(&elsewhere);
        let seats: SeatElsewhere = Arc::new(move |pane| {
            let guard = lock(&pool);
            let held = guard.pane(pane)?;
            Some(PaneSeat {
                session: held.agent_session().map(str::to_owned),
            })
        });
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&here),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        )
        .reading_seats_elsewhere(seats);

        let asked = |opener: u64| -> Value {
            json!({
                "plugin": "orchestrator",
                "pane": target.0,
                "stimulus": "ping",
                "sentinel": "ping",
                "guardrails": { "max_iterations": 1, "max_seconds": 5 },
                RUN_OPENED_BY_KEY: opener,
            })
        };

        // ── THE CLAIM ───────────────────────────────────────────────────────────────────────────
        let started = external
            .invoke(RUN_ACTION, IntrospectValue::Json(asked(asker.0)))
            .unwrap_or_else(|why| {
                panic!(
                    "⛔⛔⛔⛔ REGISTER ITEM 689: a run whose ASKER sits one window over must be \
                     accepted — the seat is a provenance, not a pane this run will drive. \
                     Refused: {why:?}"
                );
            });
        let IntrospectValue::Int(_) = started else {
            panic!("a run answers its id: {started:?}");
        };

        // ── AND THE PROVENANCE IS RECORDED, rather than accepted and dropped ─────────────────────
        let listed = read_runs(&external);
        assert_eq!(
            listed[0].get(RUN_OPENED_BY_KEY).and_then(Value::as_u64),
            Some(asker.0),
            "⚠⚠⚠ AN ACCEPTED PROVENANCE IS NOT A RECORDED ONE. Dropping it would pass the claim \
             above and still leave the asker unable to find its own run: {:?}",
            listed[0],
        );
        // ⚠ READ OFF THE REGISTRY'S OWN RECORD and not off the slot above, because the slot does
        // not publish it: the conversation is what a SUCCESSOR daemon re-derives the seat from
        // (`RunRegistry::restore`'s rule 1), so the record is where it lives and where a reader
        // that matters — a boot — will look for it.
        let recorded = lock(&registry).snapshot();
        assert_eq!(
            recorded[0].opened_by_session.as_deref(),
            Some(ELSEWHERE),
            "⚠⚠⚠⚠ AND WHO IS IN THAT SEAT, read as far as the check that accepted it. `session_in` \
             read this pool alone, so a seat one window over was recorded as belonging to NO \
             conversation — and a successor daemon re-derives the seat from the conversation, so \
             this run would come back belonging to nobody: {recorded:?}",
        );

        // ── THE CONTROL: a seat NOTHING holds is still refused ───────────────────────────────────
        // Reaching further must not become reaching for anything. Without this, both claims above
        // would pass just as well for a build that had simply stopped checking the provenance.
        let stale = external.invoke(RUN_ACTION, IntrospectValue::Json(asked(9999)));
        assert!(
            stale.is_err(),
            "⚠⚠ THE CONTROL: a stale `SPRAG_PANE` naming a pane no window of this daemon holds is \
             what the check exists for, and it must still refuse: {stale:?}",
        );
    }

    /// Poll `query("runs")` until run `id` has DELIVERED a prompt, and answer its entry.
    ///
    /// ⚠⚠⚠ **THE BOUNDARY THIS WATCHES IS THE READINESS BARRIER**, which nothing publishes
    /// directly: a run only injects once `ready_when` is satisfied, so a delivery is the product's
    /// own proof that the barrier is behind it. A gate that cancels before this point is timing the
    /// runner rather than testing the claim — see the caller for the red that established it.
    ///
    /// ⚠ Bounded, and the timeout is a FAILURE rather than a retry: a run that never delivers has
    /// something wrong with it that a longer wait would only hide.
    fn driving(external: &PluginsExternal, id: u64, within: Duration) -> Value {
        let start = Instant::now();
        loop {
            let entry = read_runs(external)
                .into_iter()
                .find(|entry| entry["id"] == json!(id));
            if let Some(entry) = &entry
                && entry[RUN_DELIVERED_KEY].as_u64().unwrap_or(0) > 0
            {
                return entry.clone();
            }
            assert!(
                start.elapsed() < within,
                "run {id} had delivered nothing after {:?}, so it never cleared its readiness \
                 barrier and there is no driving run to stop: {entry:?}",
                start.elapsed(),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// ⛔⛔⛔ **A RUN WHOSE ONLY PROMPT IS STUCK IN A COMPOSER PUTS THAT ON THE WIRE** — register
    /// item 617's second half, and the one the sentence gate cannot reach.
    ///
    /// ⚠⚠⚠⚠⚠ **THE GUARD IS `made > 0`, AND A WEDGED RUN HAS `made == 0` BY DEFINITION.** Nothing
    /// was asked, so there is no delivery to count — which is exactly why this run's count was
    /// invisible: the triple is published under a condition the run it matters most for can never
    /// satisfy. A gate that built the JSON by hand (as `delivery_sentence`'s does) reads a shape
    /// no daemon would ever emit, so this one asks the product's own door instead.
    ///
    /// ⚠⚠ **THE CONTROL IS A RUN THAT TYPED NOTHING**, and it holds the rest of the claim: the
    /// keys stay ABSENT there, which is what keeps a workspace of shells byte-identical to the
    /// pre-591 wire shape. Publishing zeroes on every run would satisfy the arm above while making
    /// the absence — *this plugin has no prompts for a composer to fold* — unsayable.
    #[test]
    fn a_run_whose_prompt_is_stuck_in_a_composer_publishes_that_count() {
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        // Two runs a predecessor daemon left on disk. Restored rather than submitted live, which
        // is both this module's own fixture shape and the harder road: it drives the persistence
        // register item 617 added beside the count, so a field the log dropped fails here too.
        let one = |id: u64, deliveries: Option<crate::runs::PersistedDeliveries>| {
            crate::runs::PersistedRun {
                id,
                label: "ai_loop pane=2".to_owned(),
                // ⚠ FINISHED, so item 543's door would refuse a request even if one were here: a
                // run whose ending was recorded is over, and nothing puts one back.
                request: None,
                iterations: 1,
                cost: None,
                unit: None,
                finished: true,
                outcome: None,
                ceiling: None,
                output: None,
                build: None,
                driver: None,
                opened_by_session: None,
                at: None,
                document: None,
                stood_down: None,
                cancelled_by: None,
                deliveries,
                banked: None,
                briefed: None,
                // ⚠ An older log carries no place, which is this fixture's shape — item 543.
                place: None,
            }
        };
        lock(&registry).restore(&crate::runs::RunLog {
            version: crate::runs::RUN_LOG_VERSION,
            runs: vec![
                // A run that typed ONE prompt onto its pane and never got it asked. `made` is 0
                // because nothing was ever asked, which is the whole shape under test.
                one(
                    0,
                    Some(crate::runs::PersistedDeliveries {
                        made: 0,
                        folded: 0,
                        unsubmitted: 1,
                    }),
                ),
                // ── AND THE CONTROL: a run that put nothing into any pane ──
                one(1, None),
            ],
        });

        let listed = read_runs(&external);
        let entry = |id: u64| {
            listed
                .iter()
                .find(|entry| entry["id"] == json!(id))
                .unwrap_or_else(|| panic!("run {id} is listed"))
        };
        assert_eq!(
            entry(0).get(RUN_UNSUBMITTED_KEY).and_then(Value::as_u64),
            Some(1),
            "⛔⛔⛔ ITEM 617: this run's prompt is sitting in a composer on a pane somebody can go \
             and look at, and the wire says nothing — because the triple is published only when \
             `delivered > 0`, which a run that never got a question asked can never be. Entry: \
             {:?}",
            entry(0),
        );
        for key in [RUN_DELIVERED_KEY, RUN_FOLDED_KEY, RUN_UNSUBMITTED_KEY] {
            assert!(
                entry(1).get(key).is_none(),
                "⚠⚠⚠⚠⚠ THE CONTROL: this run composed no prompt at all, and the absence of {key:?} \
                 is a CLAIM — *this plugin has nothing for a composer to fold*. Zeroes published \
                 on every run would make that unsayable and would change the wire shape every \
                 script reading this slot was written against. Entry: {:?}",
                entry(1),
            );
        }
    }

    /// `query("runs")` as a client reads it — through the product's own door.
    fn read_runs(external: &PluginsExternal) -> Vec<Value> {
        let IntrospectValue::Json(Value::Array(entries)) =
            external.read(RUNS_SLOT).expect("the runs slot answers")
        else {
            panic!("the runs slot answers a JSON array");
        };
        entries
    }

    /// A pane holding `session` as its conversation — what `restore_command`'s `--resume <uuid>`
    /// produces, staged through the workspace's own identity source rather than by writing the
    /// field, so the pane is named the way the product names one.
    fn resumed_pane(workspace: &Arc<Mutex<Workspace>>, session: &str) -> PaneId {
        let named = session.to_owned();
        lock(workspace).set_pane_identity_source(Arc::new(move |_| Some(named.clone())));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("exec cat");
        command.env("TERM", "dumb");
        lock(workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the resumed agent")
    }

    /// **A DIRECTORY THAT IS A TREE**, for a fixture whose pane a loop will be built over —
    /// register item 738, layer 4.
    ///
    /// ⚠⚠⚠⚠⚠ It carries `.git` because `debt_loop.scxml` says a run of that kind works in a
    /// directory that does, and the door refuses one standing anywhere else. **This is not a test
    /// accommodating a check** — it is a fixture that had been standing in `$HOME` and calling
    /// itself an agent's repository, which is the exact state item 684 measured on a live daemon
    /// and the reason the check exists. A pane that would meet the *do you trust this folder?*
    /// dialog is not a pane a debt run can be built over, in a gate any more than in the world.
    ///
    /// ⚠ Leaked deliberately: the tree lives for the process, and a fixture that removed it would
    /// race every pane still standing in it.
    fn a_tree_to_stand_in() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("sprag-loop-tree-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a directory for the fixture's panes to stand in");
        std::fs::write(dir.join(".git"), b"gitdir: nowhere\n")
            .expect("the marker `debt_loop.scxml` names — a FILE, as a linked worktree carries it");
        dir
    }

    fn echoing_agent_pane(workspace: &Arc<Mutex<Workspace>>) -> PaneId {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(
            "stty -echo; printf 'AGENT-READY\\n'; while read l; do printf '%s\\n' \"$l\"; done",
        );
        command.env("TERM", "dumb");
        // ⚠⚠ POINTED AT A TREE — see `a_tree_to_stand_in`. Without it every fixture pane is born in
        // the runner's `$HOME`, which is the placement item 684 measured costing a live run.
        command.cwd(a_tree_to_stand_in());
        lock(workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the stand-in agent")
    }

    /// The `run` request that starts a loop, with `extra` merged over it.
    fn ai_loop_request(pane: PaneId, extra: Value) -> Value {
        let mut request = json!({
            "plugin": "ai_loop",
            "pane": pane.0,
            "agent": "claude",
            "north_star": "SPRAG-NORTH-STAR-CROSSED-THE-WIRE",
            "milestone": "say the marker",
            "reference": "this gate",
            "max_turns": 3,
            // ⚠ The barrier is `shows` rather than the `settles` a real agent gets, and that is
            // this FIXTURE's honesty rather than the product's default: a `/bin/sh` stand-in is not
            // an agent any detector will name, so waiting for one to settle would be waiting for a
            // verdict nothing can produce. The `agent` key above still travels, which is the point
            // — it is what the barrier would be derived from.
            //
            // ⚠⚠ AND `shows` RATHER THAN `prints`, which the first run of this gate paid for: the
            // pane announces itself when it is SPAWNED and the run is asked for afterwards, so
            // `prints` — *more occurrences than when this run started watching* — can never be
            // satisfied by a marker that is already there. The refusal says so in its own sentence;
            // this fixture is the case that sentence was written for.
            "ready_when": { "match": "shows", "marker": "AGENT-READY" },
            // ⚠ FALSE for the peer's reason, not the product's: this stand-in paints only whole
            // lines, so a delivery cannot be confirmed on screen before the newline that submits it.
            "shows_prompt": false,
            "guardrails": { "max_iterations": 200, "max_seconds": 30 },
        });
        let object = request.as_object_mut().expect("an object");
        for (key, value) in extra.as_object().expect("an object") {
            object.insert(key.clone(), value.clone());
        }
        request
    }

    /// ⚠⚠⚠ **A PERSON CAN START AN AI LOOP, AND WHAT THEY BRIEFED IT WITH REACHES THE AGENT** —
    /// register item 65, which R380 called *"the single biggest thing between this loop and a
    /// user"*.
    ///
    /// Five rounds built `ai_loop.scxml`'s machine, gave its turns two endings, wrote its driver
    /// and measured all of it against a live `claude` — and **nothing in the daemon constructed one
    /// and no surface started one.** Every one of those measurements ran inside a test.
    ///
    /// This one goes through `RUN_ACTION`, the verb the MCP mouth and the CLI both call, and
    /// asserts the thing that could not be asserted before: **the caller's own north star is on the
    /// agent's screen.** That single string crossing is the whole chain — the request grammar
    /// parsed it, the daemon built a real script engine for it, the brief crossed into the
    /// document's datamodel as an event, `priming` composed a prompt out of it, and the driver
    /// delivered that prompt into a live pseudoterminal.
    ///
    /// ⚠⚠⚠ **A RUN THAT NAMES NO CONSENTS GETS THIS REPOSITORY'S OWN** — the carrying, gated at the
    /// one place it happens.
    ///
    /// # Why this needs a gate of its own
    ///
    /// The clauses used to be authored in `ai_loop.scxml`, and a run that named none got the
    /// document's. That made this repository's standing yesses authorise every run of a file other
    /// repositories copy, so they moved to `debt_loop.scxml` — and the template now ships an EMPTY
    /// list. **Something has to carry them across, and a carrier nothing observes is a carrier that
    /// can quietly drop what it carries.** What that looks like from outside is a run that comes up
    /// perfectly configured and stops at its first permission dialog: measured once already, on a
    /// live loop that stood there until an iteration ceiling ended it.
    ///
    /// ⚠ The count is asserted against what the KIND holds rather than against `2`, so an author
    /// adding a third clause to their own document does not have to come and edit a number here —
    /// and so this cannot pass by agreeing with a literal that drifted.
    ///
    /// ⚠⚠⚠⚠⚠ **A LOOP THIS DAEMON STARTS KEEPS ITS REVIEWS' COUNTS IN THIS DAEMON'S STATE
    /// DIRECTORY** — the one line only the daemon can write, gated where dropping it would be
    /// invisible.
    ///
    /// # ⚠⚠⚠ Why the library must NOT answer this and once did
    ///
    /// `context_review.scxml` authors a bare file name and says a driver resolves it *"against the
    /// daemon's state directory"*. `sprag-plugin` implemented that by reading `$XDG_STATE_HOME`
    /// itself — so under `cargo test`, where there is no daemon, *the daemon's state directory*
    /// meant **the home of whoever ran the suite**. Measured 2026-08-19: thirty lines per
    /// `cargo test -p sprag-plugin --lib`, and 179 standing in a shared build machine's real
    /// `~/.local/state/sprag/context-review.jsonl`. CI's `ambient-home-guard` had been failing on
    /// exactly that write.
    ///
    /// The library cannot name a home any more, which is the fix. **What that moves here is the
    /// power to forget**: a daemon that drops the assignment builds a run which comes up looking
    /// perfectly configured, reviews normally, and keeps counts nobody can ever compare with the
    /// next run's — [`sprag_plugin::AiLoop::keeping_counts_in`]'s whole reason, and the same shape
    /// as the consents gate below it.
    ///
    /// ⚠⚠ Compared against [`crate::durability::state_dir`] rather than against a literal, because
    /// a literal here would be a SECOND derivation of the path — the exact duplication that
    /// function exists to prevent — and would drift the day the state directory moves.
    /// ⛔⛔⛔⛔⛔ **A LOOP IS REFUSED AT THE DOOR WHEN THE POOL IT WOULD DRIVE THROUGH DOES NOT HOLD
    /// ITS PANE** — register item 682, and the half of it the product already had.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this gate exists over behaviour that was already correct
    ///
    /// A run captures ONE pane pool for its whole life — `SessionScope::of_session` reads it off
    /// the session's CURRENT window, and it reaches the run through `plugin_host` and
    /// `drive_on_a_thread` without ever being re-derived. So the pool and the PANE come from two
    /// different places, and 2026-08-25's diagnosis of item 682 recorded that *nothing checks they
    /// agree*. **That was wrong, and asking the product is what found it out**: this arm of
    /// `build_plugin` has called [`require_pane_in`] all along, whose own doc calls it *"the
    /// fail-fast that turns a mistyped id into a synchronous refusal instead of a run that dies on
    /// its first step"*.
    ///
    /// What was missing was not the check but a MEASUREMENT of it: no gate anywhere named the
    /// refusal, so a future arm added without the call — the `answer` arm beside this one needs it
    /// for the same reason — would ship a run that dies on its first inject, which is precisely the
    /// ending item 682 is about. **A behaviour nothing measures is a behaviour the next round can
    /// delete without noticing.**
    ///
    /// # ⚠⚠⚠ The third arm is the one that is about item 682 rather than about a typo
    ///
    /// A pane can be perfectly ALIVE and still not be this run's to drive, because a pool is a
    /// WINDOW's. That is the live shape exactly: pane id 5 was running in window `pinion` — its
    /// child had been up for 2h40m — while the run that had been driving it died saying `there is
    /// no pane 5`. Arms 1 and 2 would both pass against a check that only asked *does this id exist
    /// anywhere*; only the third says the question is about MEMBERSHIP.
    ///
    /// ⚠⚠ **AND ARM 3 IS NOT INDEPENDENTLY MUTABLE, which is a fact about the boundary rather than
    /// a gap in this gate.** Deleting the `require_pane_in` call from this arm turns it red — at
    /// arm 2, which the mutation reaches first. The rival implementation arm 3 would catch on its
    /// own (*does this id exist anywhere* rather than *is it mine*) **cannot be written here at
    /// all**: [`PluginWorld`] sees one pool and has no way to ask about another. So arm 3
    /// DOCUMENTS the shape item 682 measured rather than discriminating a competitor, and saying so
    /// is cheaper than a reader later assuming it was measured.
    ///
    /// ⚠ What this does NOT close, stated so the next reader is not misled by a green: the check is
    /// at the DOOR. A pane that leaves the pool mid-run is still unnoticed until the next
    /// injection, which is how all three live runs died — they passed this check and were killed
    /// later. That residue is item 682's remaining half, and
    /// `sprag_plugin`'s `a_run_ends_the_same_way_whether_its_pane_moved_or_was_closed` is the
    /// ratchet standing on it.
    #[test]
    fn a_loop_is_refused_at_the_door_when_its_pool_does_not_hold_the_pane() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        // ── 1. THE CONTROL, AND IT COMES FIRST: a pane THIS pool holds builds ──
        external
            .build_plugin(
                ai_loop_request(pane, json!({}))
                    .as_object()
                    .expect("an object"),
            )
            .expect(
                "⚠⚠⚠⚠ THE CONTROL FAILED: a loop over a pane its own pool holds must build, or the \
                 refusals below are a door that refuses everything and this gate measures nothing",
            );

        // ── 2. A PANE THIS POOL HAS NEVER HELD IS REFUSED, SYNCHRONOUSLY ──
        //
        // ⚠⚠ REFUSED AT `build_plugin`, before a run id is reserved or a thread is spawned. The
        // difference is what a caller can do about it: a refusal is an answer to the request they
        // just made, where a run that starts and dies is a row they have to go and read.
        let stranger = PaneId(pane.0 + 4242);
        let refused_stranger = external
            .build_plugin(
                ai_loop_request(stranger, json!({}))
                    .as_object()
                    .expect("an object"),
            )
            // ⚠ The built plugin is dropped rather than named: `PluginKind` is not `Debug`, and a
            // gate about a REFUSAL has nothing to say about what a wrongly-built run would be.
            .map(|(_built, label)| label)
            .expect_err("⚠⚠⚠⚠ THE DOOR BUILT A LOOP OVER A PANE ITS POOL DOES NOT HOLD");
        assert!(
            format!("{refused_stranger:?}")
                .contains(&format!("no pane {} in this workspace", stranger.0)),
            "⚠⚠⚠ and it must NAME the pane and say WHICH workspace could not find it — a refusal \
             that says neither sends a caller to re-read their own request instead of their \
             window: {refused_stranger:?}",
        );

        // ── 3. ⭐ AND A PANE THAT IS ALIVE IN ANOTHER POOL IS REFUSED THE SAME WAY ──
        //
        // ⚠⚠⚠⚠⚠ **THIS IS THE ARM THAT IS ABOUT ITEM 682.** A `sibling` pool is what
        // `Session::break_pane` mints for a window it opens, so this stages the state a
        // cross-window move leaves: the pane is running, the SESSION has it, and the pool this run
        // would drive through does not. A check that asked *does this id exist* rather than *is it
        // mine* would pass arms 1 and 2 and fail here.
        let elsewhere = Arc::new(Mutex::new(lock(&workspace).sibling()));
        let another_window = echoing_agent_pane(&elsewhere);
        let refused_neighbour = external
            .build_plugin(
                ai_loop_request(another_window, json!({}))
                    .as_object()
                    .expect("an object"),
            )
            .map(|(_built, label)| label)
            .expect_err(
                "⚠⚠⚠⚠⚠ THE DOOR BUILT A LOOP OVER A PANE IN SOMEBODY ELSE'S POOL. That run would \
                 deliver nothing and die `there is no pane N` on its first injection — item 682's \
                 ending, reached from the door instead of from a move",
            );
        assert!(
            format!("{refused_neighbour:?}")
                .contains(&format!("no pane {} in this workspace", another_window.0)),
            "⚠⚠ and by the same sentence: *not in THIS workspace* is the true and useful thing to \
             say about a pane that is alive in another one: {refused_neighbour:?}",
        );
        assert!(
            !lock(&elsewhere)
                .pane(another_window)
                .expect("the sibling pool spawned it")
                .pty()
                .is_eof(),
            "⚠⚠⚠⚠ THE FIXTURE'S POINT: that pane is RUNNING. If it were dead this arm would be a \
             second copy of arm 2 rather than the live shape item 682 measured",
        );
    }

    #[test]
    fn a_loop_this_daemon_starts_keeps_its_counts_in_this_daemons_state_directory() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        let asked = ai_loop_request(pane, json!({}));
        let (built, _label) = external
            .build_plugin(asked.as_object().expect("an object"))
            .expect("a plain ai_loop request is well-formed");
        let PluginKind::AiLoop(loops, _) = built else {
            panic!("the control: an `ai_loop` request builds an ai_loop");
        };

        let expected = crate::durability::state_dir();
        assert!(
            expected.is_absolute(),
            "⚠⚠⚠ THE CONTROL: a state directory that is not absolute would make the assertion \
             below pass while the counts landed relative to whatever directory the daemon happened \
             to be started in. Got {expected:?}",
        );
        assert_eq!(
            loops.keeping_counts_in(),
            Some(expected.as_path()),
            "⚠⚠⚠⚠⚠ A RUN THIS DAEMON BUILT MUST CARRY THIS DAEMON'S STATE DIRECTORY. `None` here \
             is the daemon's one line gone: nothing fails, no run stops, and the loop simply stops \
             keeping the readings that make *is this getting better?* a question with an answer. \
             Any OTHER directory is a second derivation of a path this daemon already owns",
        );
    }

    /// ⚠⚠ AND THE CONTROL IS THE OTHER DIRECTION: a caller who DOES name consents must still win.
    /// Without it, "the kind is consulted" and "the kind always wins" are the same green, and the
    /// second one silently discards what a caller asked for.
    #[test]
    fn a_run_that_names_no_consents_gets_this_repositorys_own() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        // What this repository's kind document holds — the authority the assertion compares to.
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let owed = sprag_plugin::kind::LoopKind::debt(script)
            .expect("this repository's kind document must open")
            .consents()
            .expect("its clause list must be readable")
            .expect("a debt run answers dialogs");
        assert!(
            !owed.clauses().is_empty(),
            "the control: the kind must ship clauses, or every assertion below is vacuous",
        );

        let asked = ai_loop_request(pane, json!({}));
        let (built, _label) = external
            .build_plugin(asked.as_object().expect("an object"))
            .expect("a run that names no consents is well-formed");
        let PluginKind::AiLoop(loops, _) = built else {
            panic!("the control: an `ai_loop` request builds an ai_loop");
        };
        let carried = loops
            .consenting()
            .expect("the run's clause list must be readable")
            .expect(
                "⚠⚠⚠ THE RUN CAME UP ANSWERING NOTHING. The template ships an empty list on \
                     purpose and the kind document holds the clauses; a run that reaches its first \
                     permission dialog with none stops there and waits for somebody who is not \
                     watching",
            );
        assert_eq!(
            carried.clauses().len(),
            owed.clauses().len(),
            "⚠⚠⚠ every clause this repository authored must reach the run. Carried {carried:?}, \
             authored {owed:?}",
        );
        for clause in owed.clauses() {
            assert!(
                carried
                    .clauses()
                    .iter()
                    .any(|got| got.asked() == clause.asked() && got.answer() == clause.answer()),
                "⚠⚠ and each one whole — a clause that arrived with half its text claims a dialog \
                 it cannot answer: {clause:?} missing from {carried:?}",
            );
        }

        // ── THE CONTROL: A CALLER WHO NAMES CONSENTS STILL WINS ──
        let named = ai_loop_request(
            pane,
            json!({ Consents::WIRE_KEY: [{ Consent::ASKED_KEY: "only this", Consent::ANSWER_KEY: "and only this" }] }),
        );
        let (built, _label) = external
            .build_plugin(named.as_object().expect("an object"))
            .expect("a run that names its own consents is well-formed");
        let PluginKind::AiLoop(loops, _) = built else {
            panic!("the control: an `ai_loop` request builds an ai_loop");
        };
        let carried = loops
            .consenting()
            .expect("readable")
            .expect("a caller's own list is not nothing");
        assert_eq!(
            carried.clauses().len(),
            1,
            "⚠⚠⚠ A CALLER'S OWN CONSENTS MUST WIN OVER THE KIND'S. Falling back is what an ABSENT \
             key means; overriding a present one would discard what somebody asked for, and would \
             make the assertion above pass for the wrong reason. Got {carried:?}",
        );
    }

    /// ⚠⚠ **AND IT IS CANCELLED RATHER THAN RUN TO CONVERGENCE**, deliberately. Convergence needs a
    /// supervisor that can call this peer's turns over, which is `sprag-plugin`'s own gate against
    /// its `supervised` fixture. What is measured HERE is the door, and a gate that also waited for
    /// an ending would be two claims wearing one name.
    #[test]
    fn a_loop_started_over_the_wire_prompts_its_agent_with_what_the_caller_briefed() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let id = u64::try_from(id).expect("a run id is not negative");

        let access = sprag_plugin::WorkspacePaneAccess::new(Arc::clone(&workspace));
        let began = Instant::now();
        let mut screen = String::new();
        while began.elapsed() < Duration::from_secs(20) {
            screen = access.pane_collapsed(pane).unwrap_or_default();
            if screen.contains("SPRAG-NORTH-STAR-CROSSED-THE-WIRE") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            screen.contains("SPRAG-NORTH-STAR-CROSSED-THE-WIRE"),
            "⚠⚠⚠ the caller's own north star must be on the agent's screen — that string is the \
             whole chain from a wire request to a prompt in a pseudoterminal. Screen: {screen:?}, \
             run: {:?}",
            // ⚠ The run's own record, because a screen that is missing the prompt cannot say WHY:
            // a refused barrier and a machine that never left `idle` look identical from here.
            lock(&registry)
                .snapshot()
                .first()
                .map(|run| run_to_json(run, run.opened_by)),
        );

        assert!(
            lock(&registry).cancel(RunId(id)),
            "the run this call started is one the registry can stop",
        );
        let entry = ended(&registry, id, Duration::from_secs(30));
        assert_eq!(
            entry["state"]["outcome"]["state"],
            json!("cancelled"),
            "⚠⚠ AND IT IS THE RUN REGISTRY'S OWN CANCEL that ends it, not a bound this gate \
             invented — a loop is a run like any other the day it is a plugin: {entry:?}",
        );
        assert_eq!(
            entry["label"],
            json!(format!("ai_loop pane={}", pane.0)),
            "a reader of `runs` must be able to see WHICH pane a loop is driving: {entry:?}",
        );
        // ⛔⛔⛔⛔⛔ **AND HOW BIG THE BRIEF THIS DOOR ACCEPTED IS** — register item 719's second
        // direction, asserted HERE because this gate is the only one that walks the whole chain:
        // a wire request, a real `AiLoop::new`, the brief read back out of a real datamodel, the
        // driver's per-step read, and the row a caller is told to poll. Everything else about this
        // level is measured in pieces, and a chain measured in pieces is one that can be broken at
        // a join nobody owns.
        //
        // ⚠⚠ THE NUMBERS ARE THIS REQUEST'S OWN: the three strings `ai_loop_request` sends, whose
        // lengths nothing here restates — 33 + 14 + 9. If somebody edits that fixture's prose this
        // goes red and the arithmetic is the repair, which is the honest coupling.
        let briefed = entry
            .get(RUN_BRIEFED_KEY)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(
            briefed.contains("56 bytes") && briefed.contains("north_star"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 719: `orchestrate` took this brief and the row it points its \
             caller at says nothing about the size — which is the whole of that item's second \
             direction. A brief is re-typed in full into every session a run opens, and the person \
             who wrote 9,025 bytes of one had no way to find that out. Said: {briefed:?} in \
             {entry:?}",
        );
        // ⛔⛔⛔ **AND NOT ONLY AS PROSE INSIDE THAT NAME** — register item 540.
        //
        // The assertion above passed for a year while the ONLY structured answer to *which pane is
        // this run driving* was a human sentence a reader had to parse — R431's *derive it from a
        // name*, against a label this repository rewords at will. `Plugin::driving` knew it the
        // whole time and nothing carried it out.
        //
        // ⚠⚠ THE PAIR IS THE CLAIM: the key must equal the pane the label names, so a run that
        // reported some OTHER pane structurally would be caught rather than quietly believed. And
        // it is asserted on a run that was CANCELLED — the counters and the pane survive the
        // ending, because the question *what was it driving* is asked most often about a run that
        // stopped.
        assert_eq!(
            entry[RUN_DRIVING_KEY],
            json!(pane.0),
            "⛔⛔⛔ ITEM 540: nothing published names the pane this run is driving except the label's \
             prose. `sprag panes` cannot ask *is anybody driving me* (item 595) until the run says \
             it in a form a program can read: {entry:?}",
        );
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⛔⛔⛔ **`sprag runs` SAYS WHAT BECAME OF A PERSON'S STAND-DOWN, INCLUDING WHEN THE ANSWER IS
    /// «IT DID NOT LAND»** — register item 594, measured on this repository's own loop.
    ///
    /// # What a person was told, and what they were shown
    ///
    /// `sprag stand-down 1` answered *"it stops at its next milestone, and its work is kept —
    /// `sprag runs` says when it has"*. What `sprag runs` said afterwards was **`cancelled after 56
    /// iterations, 23146 bytes`**, and there was nothing anywhere in that answer to say an order had
    /// ever been given. The order lived in a host flag that only the loop document read, and the
    /// word that document closes under reaches a walk and no wire key at all — so the two endings a
    /// person most needs to tell apart, *my order landed and the work is banked* and *my order never
    /// landed and the work is gone*, were published as one word each and neither mentioned the
    /// order.
    ///
    /// ⚠⚠⚠⚠⚠ **THE CONTROL IS THE SAME RUN WITHOUT THE ORDER**, and it is what makes this a
    /// measurement rather than a decoration. Both arms are the same brief over the same stand-in
    /// agent, both are ended by the registry's own cancel, and both report `cancelled`. The ONLY
    /// difference is that somebody spoke to one of them. If the key appeared on both, its presence
    /// would be saying nothing; if it appeared on neither, the promise would still have no surface.
    ///
    /// ⚠⚠ **AND IT IS THE UNHONOURED PAIRING THAT IS DRIVEN HERE**, deliberately. The honoured one —
    /// an order standing over a run that converges — needs a supervisor that can call this peer's
    /// turns over, which is `sprag-plugin`'s own fixture and its own gate
    /// (`the_promise_about_a_stand_down_names_the_word_a_stood_down_run_reports`). The pairing this
    /// register item was FILED for is the one that broke a promise, and it is the one no gate could
    /// reach before this key existed.
    #[test]
    fn a_stood_down_run_publishes_the_order_and_says_when_the_ending_did_not_honour_it() {
        /// One `ai_loop` run over a stand-in agent, optionally stood down, then cancelled — and the
        /// entry `query("runs")` publishes for it once it is over.
        ///
        /// ⚠ ONE BODY FOR BOTH ARMS, so the arm and its control cannot differ in anything except
        /// the order: two hand-written setups is how a control quietly stops being one.
        fn a_cancelled_run(ordered: bool) -> Value {
            let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
            let pane = echoing_agent_pane(&workspace);
            let registry = Arc::new(Mutex::new(RunRegistry::default()));
            let mut external = PluginsExternal::new(
                Arc::clone(&workspace),
                Arc::clone(&registry),
                None,
                None,
                None,
                None,
                None,
            );
            let started = external
                .invoke(
                    RUN_ACTION,
                    IntrospectValue::Json(ai_loop_request(pane, json!({}))),
                )
                .expect("a well-formed ai_loop run");
            let IntrospectValue::Int(id) = started else {
                panic!("a run answers its id: {started:?}");
            };
            let id = u64::try_from(id).expect("a run id is not negative");
            if ordered {
                // ⚠⚠ THROUGH THE WIRE VERB, not `RunRegistry::stand_down` — this gate is about what
                // a PERSON typing `sprag stand-down` is later shown, and the CLI reaches the
                // registry through exactly this action. A gate that called the registry directly
                // would leave the door it is really about untested.
                external
                    .invoke(
                        STAND_DOWN_ACTION,
                        IntrospectValue::Json(json!({ "id": id })),
                    )
                    .expect("a run in the directory takes a stand-down");
            }
            assert!(
                lock(&registry).cancel(RunId(id)),
                "the run this call started is one the registry can stop",
            );
            let entry = ended(&registry, id, Duration::from_secs(30));
            assert!(
                lock(&workspace).close(pane).is_some(),
                "the pane this arm opened was there to close",
            );
            entry
        }

        // ── THE CONTROL: NOBODY SPOKE TO IT ──
        let quiet = a_cancelled_run(false);
        assert_eq!(
            quiet[RUN_STOOD_DOWN_KEY],
            Value::Null,
            "⚠⚠⚠ THE CONTROL: a run nobody ordered must publish NO such key. Presence is the claim \
             here — the rule `RUN_CEILING_KEY` follows — so a key that appeared on every run would \
             make the arm below pass while saying nothing about anybody's order: {quiet:?}",
        );

        // ── THE ARM: A PERSON SPOKE, AND THE RUN DIED BEFORE IT COULD OBEY ──
        let ordered = a_cancelled_run(true);
        // ⛔⛔⛔ **THIS USED TO ASSERT THE TWO ARMS ENDED WITH THE SAME WORD, AND THAT WAS A CLAIM
        // ABOUT THE MACHINE'S SPEED.** It went red on `headless (macos)` the first time it ran
        // there: the arm reported `failed` — *"the pane never showed \"AGENT-READY\", which this
        // run was told to wait for before driving it"* — while the control reported `cancelled`.
        // The ARM makes one extra wire call (the stand-down) between starting the run and
        // cancelling it, so its cancel lands later, and on a loaded runner the readiness bound wins
        // that race. **Both readings are correct behaviour**; which one appears is a fact about the
        // box.
        //
        // ⚠⚠⚠⚠⚠ WHAT THE ORIGINAL ASSERTION WAS PROTECTING IS REAL AND IS KEPT AS A CLASS. The
        // claim under test needs the arm to be a run that did NOT reach a milestone, because that
        // is the branch of `stand_down_sentence` this gate is about — not that it reached any
        // particular one. `converged` is the one ending that would make the sentence say the
        // opposite, so THAT is what has to be impossible here, and it is asserted of both arms.
        //
        // ⚠⚠ AND THE CONTROL'S OWN JOB IS UNAFFECTED EITHER WAY: the key's presence is decided by
        // `run.stood_down` alone, so a control that ended differently still answers *nobody ordered
        // this one*. R358's rule — *what makes the loser impossible, rather than late?* — is why
        // the equality had to go rather than be retried.
        for (label, entry) in [("the ordered run", &ordered), ("the control", &quiet)] {
            assert_ne!(
                entry["state"]["outcome"]["state"],
                json!("converged"),
                "⚠⚠⚠⚠ {label} REACHED A MILESTONE, and this gate is about the pairing where an \
                 order was given and NOT honoured. A converged run's work is banked, so the \
                 sentence below would be the opposite one and the assertion on it would be about a \
                 branch nobody drove: {entry:?}",
            );
            assert_eq!(
                entry["state"]["status"],
                json!("done"),
                "⚠⚠ {label} must have FINISHED, or `ended` handed back a run still going and every \
                 reading below is a snapshot of the middle: {entry:?}",
            );
        }
        let said = ordered[RUN_STOOD_DOWN_KEY].as_str().unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔ ITEM 594: a person asked this run to stand down and `sprag runs` says \
                 nothing about it — which is the surface `sprag stand-down`'s own answer sends \
                 them to. Entry: {ordered:?}"
            )
        });
        // ⚠⚠⚠ **ASKED OF THE CLAIM, NOT OF ITS OLD SPELLING.** This read `contains("NOT banked")`
        // until 2026-08-23, which was that claim's wording at the time and ALSO a second claim
        // welded to it — that the work was lost — which register item 604 measured to be false in
        // the ordinary case. The clause below is the one this gate is about and the one that has
        // to survive every rewording: the promise was not kept.
        assert!(
            said.contains("not what `sprag stand-down` promised"),
            "⛔⛔⛔ ITEM 594, THE WHOLE OF IT: this run was ordered to stand down and then died \
             without reaching a milestone, so the promise *its work is kept* was NOT kept — and \
             the sentence a person reads has to say so. A sentence that reported the order and let \
             them go on believing the order had landed is worse than the silence it replaced. Got \
             {said:?}",
        );
        assert!(
            said.contains(
                ordered["state"]["outcome"]["state"]
                    .as_str()
                    .expect("a finished run publishes its word"),
            ),
            "⚠⚠⚠ AND IT MUST NAME THE ENDING THAT OVERTOOK THE ORDER. *It did not land* is half an \
             answer; the remedy differs by what happened instead, and the word is already in this \
             very entry — a reader should not have to pair two lines by eye. Got {said:?} beside \
             {ordered:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AN ORDER A PERSON GIVES MUST REACH THE READER THE DRIVER ACTUALLY USES** —
    /// register item 699, and NEITHER OF THEM DID.
    ///
    /// # What was measured, and it was not a probability
    ///
    /// A run is driven by `sprag-term --drive` — another process — which learns a person's order by
    /// re-reading this row. Both readings were wrong, and both had been wrong since they were
    /// written:
    ///
    /// * `stand_down` was read as `row[RUN_STOOD_DOWN_KEY].as_bool()` off the key the daemon fills
    ///   with `stand_down_sentence`, a STRING. `as_bool` on a string is `None`, so the driver's
    ///   comparison was `None == Some(true)` — **false on every pass of every run, for ever.**
    ///   Nine stand-downs across four repositories, zero convergences.
    /// * `held` was read off `row["held"]`, **a key no projection has ever written**, from a flag
    ///   `RunHandle` had no reader for and `RunSummary` had no field for. `hold-run` was write-only
    ///   end to end.
    ///
    /// ⚠⚠⚠⚠⚠ **AND THE GATES BESIDE THIS ONE WERE ALL GREEN THROUGHOUT.** The gate directly above
    /// proves the daemon publishes the right SENTENCE, and it is right; `sprag-plugin`'s gates prove
    /// the document converges when its orders region is in `standing_down`, and they are right. What
    /// nobody measured is the HOP BETWEEN THEM, so each half went on being correct about itself
    /// while the pair did nothing. That is why this gate reads the row through the daemon's own
    /// projection and then through [`StandingOrders::in_row`] — **the two doors the product uses**,
    /// never a row spelled here.
    ///
    /// ⚠⚠ THE HOLD ARM RELEASES AS WELL AS TAKES. `held` is the one order a person can take back, so
    /// *it turned on* is half a gate: a latch bolted in where a level belongs would pass the first
    /// assertion and strand every held run for ever.
    #[test]
    fn an_order_a_person_gives_reaches_the_reader_the_driver_uses() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let id = u64::try_from(id).expect("a run id is not negative");

        // The row exactly as a driver's `read_row` would receive it, built by the daemon's own
        // projection — never a literal written here, which would test this test.
        let row_now = || {
            lock(&registry)
                .snapshot()
                .iter()
                .find(|run| run.id.0 == id)
                .map(|run| run_to_json(run, run.opened_by))
                .expect("the run this call started is in the registry's snapshot")
        };
        // ⚠⚠⚠ THE STAGING CONTROL, asked before every reading below. An `EndedRun` answers `false`
        // to both orders BY DESIGN, so a run that died early would make every assertion here pass
        // or fail for a reason that has nothing to do with the subject. This turns that into a
        // named failure instead of a silent one.
        let ordered_now = |what: &str| {
            let row = row_now();
            assert_eq!(
                row["state"]["status"],
                json!("running"),
                "⚠⚠⚠⚠ THE FIXTURE, NOT THE SUBJECT: this run had already ended before {what} could \
                 be read, and an ended run answers `false` to every order by design. Nothing below \
                 is about the wiring this gate is for. Row: {row:?}",
            );
            // ⚠⚠⚠⚠⚠ **THROUGH THE DRIVER'S OWN READER, NEVER `StandingOrders::in_row` DIRECTLY.**
            // The first draft of this gate called the type, and mutating `crate::drive`'s reader
            // back to the shipped defect left it **GREEN** — measured, not feared: the gate called
            // one door and the driver called another, so the step between them had no eye on it.
            // `carry_orders_in` is that step, and driving it here is what makes the mutation red.
            let (cancel, stand_down, hold) = (
                AtomicBool::new(false),
                AtomicBool::new(false),
                AtomicBool::new(false),
            );
            crate::drive::carry_orders_in(
                row.as_object().expect("a run row is a JSON object"),
                &cancel,
                &stand_down,
                &hold,
            );
            StandingOrders {
                stand_down: stand_down.load(std::sync::atomic::Ordering::Acquire),
                held: hold.load(std::sync::atomic::Ordering::Acquire),
            }
        };

        // ── THE CONTROL: NOBODY HAS SAID ANYTHING ──
        assert_eq!(
            ordered_now("the control"),
            StandingOrders {
                stand_down: false,
                held: false,
            },
            "⚠⚠⚠ THE CONTROL: a run nobody has spoken to must read as no orders. Without this the \
             arms below would pass against a reader that answered `true` to everything",
        );

        // ── ARM ONE: A HOLD, AND THEN TAKING IT BACK ──
        external
            .invoke(
                HOLD_RUN_ACTION,
                IntrospectValue::Json(json!({ "id": id, "held": true })),
            )
            .expect("a run in the directory takes a hold");
        assert!(
            ordered_now("the hold").held,
            "⛔⛔⛔ ITEM 699's SECOND HALF: `hold-run` answered a person *it parks at its next pass* \
             and the driver could not learn it had been ordered — the flag was stored in this \
             process and read by nobody, in any process, ever. Row: {:?}",
            row_now(),
        );
        external
            .invoke(
                HOLD_RUN_ACTION,
                IntrospectValue::Json(json!({ "id": id, "held": false })),
            )
            .expect("a held run takes a release");
        assert!(
            !ordered_now("the release").held,
            "⚠⚠⚠⚠ A LEVEL, NOT A LATCH. `resume-run` delivers `Hold(false)`, and a reader that only \
             ever raised would leave every released run held for ever — the failure that verb's own \
             promise is made of. Row: {:?}",
            row_now(),
        );

        // ── ARM TWO: THE STAND-DOWN ──
        // ⚠ THROUGH THE WIRE VERB, the gate above's rule: this is the door `sprag stand-down`
        // reaches, and a call to `RunRegistry::stand_down` would leave it untested.
        external
            .invoke(
                STAND_DOWN_ACTION,
                IntrospectValue::Json(json!({ "id": id })),
            )
            .expect("a run in the directory takes a stand-down");
        assert!(
            ordered_now("the stand-down").stand_down,
            "⛔⛔⛔⛔⛔ ITEM 699's FIRST HALF, AND THE ONE THAT COST NINE RUNS: the daemon published \
             the order as a SENTENCE and the driver read it as a bool, so `In('standing_down')` was \
             never reachable and `judging`'s door to `closing` could not open. Row: {:?}",
            row_now(),
        );

        // ── AND THE PERSON'S SENTENCE IS STILL THE PERSON'S ──
        let row = row_now();
        assert!(
            row[RUN_STOOD_DOWN_KEY].is_string(),
            "⚠⚠⚠ THE REPAIR MUST NOT TAKE THE HUMAN READING WITH IT. `stand_down_sentence` has four \
             readers that call `as_str` on this key, and turning it into a bool to satisfy the \
             driver would have moved the defect rather than closed it. The two readings travel side \
             by side, which is why {RUN_ORDERS_KEY:?} exists. Row: {row:?}",
        );

        assert!(
            lock(&registry).cancel(RunId(id)),
            "the run this gate started is one the registry can stop",
        );
        let _ = ended(&registry, id, Duration::from_secs(30));
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⛔⛔⛔⛔ **A RUN A PERSON STOPPED AND A RUN A SHUTDOWN SWEPT ARE THE SAME WORD AND OPPOSITE
    /// SITUATIONS, AND `sprag runs` NOW TELLS THEM APART** — register item 596, the half of item
    /// 594's unanswered *why* that no amount of reading could settle.
    ///
    /// # What could not be answered, and why it was not a documentation gap
    ///
    /// That round measured a run reported `cancelled after 56 iterations` under a standing
    /// stand-down order, and could not say whether a person had cancelled it or the promotion's
    /// `kill-server` had. **The product genuinely did not know**: `RunRegistry::cancel` and
    /// `RunRegistry::cancel_all` both stored into one `AtomicBool`, so the driver raised one
    /// `OrchestrationEvent::Cancel` and both runs closed on the identical word — while
    /// [`crate::runs::Canceller::describe`] shows the remedies pointing in opposite directions:
    /// *ask whoever stopped it* against *nobody decided anything, bring the daemon back*.
    ///
    /// ⚠⚠⚠⚠⚠ **THE TWO ARMS DIFFER IN ONE CALL AND NOTHING ELSE.** Same brief, same stand-in
    /// agent, same registry, same ending word — one is stopped through `cancel`, the other through
    /// `cancel_all`. That is the whole experiment: if the answers match, the distinction this item
    /// exists to draw is not on the wire, whatever the enum one crate over says.
    ///
    /// ⚠⚠ **AND A THIRD ARM HOLDS THE KEY TO PRESENCE-IS-THE-CLAIM.** A run still going has had no
    /// cancel raised over it, so it must publish NO such key — a key present on every run would let
    /// both arms above pass while saying nothing about anybody's cancel.
    #[test]
    fn a_cancelled_run_says_whether_a_person_or_a_shutting_down_daemon_raised_it() {
        /// One `ai_loop` run over a stand-in agent, stopped by `stopper`, and the entry
        /// `query("runs")` publishes for it once it is over.
        ///
        /// ⚠ ONE BODY FOR BOTH ARMS for the sibling gate's reason: two hand-written setups is how
        /// a control quietly stops being one. The `stopper` closure is the ONLY difference, and it
        /// is handed the registry rather than an id so `cancel_all` — which names no run — can be
        /// one of the two.
        fn a_run_stopped_by(stopper: impl FnOnce(&Arc<Mutex<RunRegistry>>, u64)) -> (Value, Value) {
            let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
            let pane = echoing_agent_pane(&workspace);
            let registry = Arc::new(Mutex::new(RunRegistry::default()));
            let mut external = PluginsExternal::new(
                Arc::clone(&workspace),
                Arc::clone(&registry),
                None,
                None,
                None,
                None,
                None,
            );
            let started = external
                .invoke(
                    RUN_ACTION,
                    IntrospectValue::Json(ai_loop_request(pane, json!({}))),
                )
                .expect("a well-formed ai_loop run");
            let IntrospectValue::Int(id) = started else {
                panic!("a run answers its id: {started:?}");
            };
            let id = u64::try_from(id).expect("a run id is not negative");
            // ⛔⛔⛔⛔ WAIT UNTIL THE LOOP IS PROVABLY PAST ITS READINESS BARRIER BEFORE STOPPING
            // IT — and this is not caution, it is a red this gate's own first mutation produced.
            // Cancelling straight after `submit` races the `ready_when` wait: the run reported
            // **`failed` — "the pane never showed \"AGENT-READY\""** — because the cancel reached a
            // run that had not started driving, and an arm that ended `failed` is not a cancelled
            // run at all. That is register item 600's finding on this repository's own gates
            // (*make the loser impossible, not merely late*), met here rather than tolerated.
            //
            // ⚠⚠ A DELIVERY is the observable, and it is the right one: a run that has put a prompt
            // into its pane has by definition cleared the barrier that guards the injection. It is
            // read through `query("runs")` like everything else here, so the wait cannot see a fact
            // the product does not publish.
            let live = driving(&external, id, Duration::from_secs(30));
            stopper(&registry, id);
            let entry = ended(&registry, id, Duration::from_secs(30));
            assert!(
                lock(&workspace).close(pane).is_some(),
                "the pane this arm opened was there to close",
            );
            (live, entry)
        }

        // ── ARM ONE: A PERSON SAID STOP ──
        let (live, person) = a_run_stopped_by(|registry, id| {
            assert!(
                lock(registry).cancel(RunId(id)),
                "the run this call started is one the registry can stop",
            );
        });
        assert_eq!(
            live[RUN_CANCELLED_BY_KEY],
            Value::Null,
            "⚠⚠⚠ THE THIRD ARM: a run nobody has cancelled must publish NO such key. Presence is \
             the claim here — `RUN_CEILING_KEY`'s rule — so a key that appeared on every run would \
             make both arms below pass while saying nothing about anybody's cancel: {live:?}",
        );

        // ── ARM TWO: THE DAEMON WENT AWAY AND SWEPT EVERY RUN ──
        let (_, shutdown) = a_run_stopped_by(|registry, _| lock(registry).cancel_all());

        // ⚠⚠⚠⚠⚠ THE WORD IS THE SAME ON PURPOSE, AND ASSERTING IT IS WHAT MAKES THE REST A
        // MEASUREMENT. If the two arms ended on different `state` words, a reader could already
        // tell them apart and this key would be answering a question nobody had. R358's rule: name
        // what would make the finding vacuous, then assert it away.
        for (label, entry) in [("the person's run", &person), ("the swept run", &shutdown)] {
            assert_eq!(
                entry["state"]["status"],
                json!("done"),
                "⚠⚠ {label} must have FINISHED, or every reading below is a snapshot of the \
                 middle: {entry:?}",
            );
        }
        assert_eq!(
            person["state"]["outcome"]["state"], shutdown["state"]["outcome"]["state"],
            "⚠⚠⚠⚠ THE TWO ARMS MUST END ON THE SAME WORD — that identity is the PROBLEM this item \
             was filed about, not an accident to be tolerated. If they differ, the key under test \
             is decorating a distinction the state already made and this gate proves nothing. \
             {person:?} against {shutdown:?}",
        );

        let by_person = person[RUN_CANCELLED_BY_KEY].as_str().unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔ ITEM 596: a person stopped this run and `sprag runs` will not say so — \
                 which is the surface they are sent to. Entry: {person:?}"
            )
        });
        let by_shutdown = shutdown[RUN_CANCELLED_BY_KEY].as_str().unwrap_or_else(|| {
            panic!(
                "⛔⛔⛔ ITEM 596: a daemon shutting down stopped this run and `sprag runs` will \
                 not say so. This is the arm that matters MOST — a sweep happens on a daemon's way \
                 out, so it is read after a restart or never. Entry: {shutdown:?}"
            )
        });
        assert_ne!(
            by_person, by_shutdown,
            "⛔⛔⛔⛔ ITEM 596, THE WHOLE OF IT: both runs closed on the same word, and the ONE \
             thing that separates them — who raised the cancel — must reach the reader as \
             something different to read. Identical sentences here mean `cancel` and `cancel_all` \
             have been fused again somewhere between the registry and this key. Got {by_person:?} \
             for both",
        );
        assert!(
            by_shutdown.contains("NOBODY"),
            "⚠⚠⚠⚠ AND THE SWEPT RUN'S SENTENCE HAS TO SAY THE PART THAT CHANGES WHAT A PERSON \
             DOES: nobody decided anything about it. A sentence that merely named the mechanism \
             would leave them looking for a decision that was never taken — which is exactly the \
             search item 594 recorded and could not end. Got {by_shutdown:?}",
        );
    }

    /// ⛔⛔⛔⛔ **A PERSON WHOSE RUN BANKED A TURN IS NEVER TOLD THAT WORK WAS LOST** — register
    /// item 604, driven end to end at the door a person actually reads.
    ///
    /// # The report that was backwards
    ///
    /// `sprag stand-down` promises *"it stops at its next milestone, and its work is kept"*. This
    /// run's turn COMPLETES — the precondition below holds it to the walk — and then its agent
    /// exits, which is what an agent that has finished its work does. The run ends `failed` by way
    /// of `peer_gone`, and until 2026-08-23 [`stand_down_sentence`] told the person **"it was cut
    /// short, so the turn it had going was NOT banked"**.
    ///
    /// ⚠⚠⚠⚠⚠ **THE RELIEVED ANSWER AND THE ALARMING ONE WERE SWAPPED**, which is the one direction
    /// a report must never be wrong in — and register item 594 exists because this exact pair was
    /// unreadable once already.
    ///
    /// # ⚠⚠⚠ Why the ENDING is left alone and only the sentence moved
    ///
    /// `peer_gone` is an honest ending: the agent really did leave, and a run that cannot be asked
    /// anything more has not converged. What was never honest is a renderer asserting, about every
    /// ending that is not a convergence, a fact it had no way to know. The fix is to give it one —
    /// [`crate::Outcome::banked`], published by the plugin in the plugin's own unit — rather than
    /// to soften the word the run ended with.
    ///
    /// # What it settles about item 598
    ///
    /// That item's recorded blocker — *a converging run needs a supervisor the `/bin/sh` stand-in
    /// cannot be* — is nearly right and names the wrong thing. A stood-down run converges at
    /// `judging`, reached only when a TURN COMPLETES, and there are two completion signals:
    ///
    /// * `settles` needs a detector, and every gate in this module builds `PluginsExternal` with
    ///   `agents: None`. **No pane here can ever be called settled** — that is the real blocker.
    /// * `exits` completes the turn, and lands in the run this gate drives.
    ///
    /// ⚠ So 598 still waits on a fixture with a detector, which is named in the register.
    #[test]
    fn a_stood_down_run_whose_peer_exits_after_a_banked_turn_is_told_its_work_is_kept() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        // ⚠⚠ A ONE-SHOT STAND-IN: it announces itself, takes ONE prompt, answers it, and EXITS —
        // which is what a well-behaved agent does when its work is finished. `echoing_agent_pane`'s
        // peer never stops reading, so its turn has no end a shell can signal at all.
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // ⚠⚠⚠ THE `sleep` IS THE WINDOW THE ORDER ARRIVES IN, not padding. Without it the whole run
        // is over in milliseconds — measured: four steps, ending before the stand-down had crossed
        // the wire at all, so the gate was reading a run nobody had ordered. The peer holds its
        // answer long enough for a person's order to land, which is what a real agent's thinking
        // time does for free.
        command.arg("stty -echo; printf 'AGENT-READY\\n'; read l; sleep 2; printf '%s\\n' \"$l\"");
        command.env("TERM", "dumb");
        // ⚠ POINTED AT A TREE, like `echoing_agent_pane` — see `a_tree_to_stand_in`: a debt run is
        // not built over a pane standing where its agent would be asked to trust the folder.
        command.cwd(a_tree_to_stand_in());
        let pane = lock(&workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the one-shot stand-in agent");
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        // ⚠⚠⚠⚠⚠ THE TURN CONTRACT IS WHAT MAKES THIS REACHABLE, and finding that out is what this
        // round measured — twice, because the first two answers were both the product's.
        //
        // 1. The fixture's default barrier left the run sitting in `Working` for sixty seconds
        //    (*"looked, nothing had happened"*): a turn ends when the peer SETTLES, and no detector
        //    will call a `/bin/sh` stand-in settled. **That is the register's recorded blocker, and
        //    it is real.**
        // 2. `done_when: exits` DID end the turn — `Working --TurnDone--> Judging` — and then the
        //    document took `Judging --PeerGone--> PeerGone` and the run reported `failed`. A peer
        //    that exits is gone, standing order or not.
        // 3. A turn BOUND does not end a turn either — it bounds `done_when`, it does not replace
        //    it; the run sat in `Working` through 22 steps until its duration ceiling.
        //
        // ⚠⚠⚠⚠ So `exits` is the ONLY turn-completion a shell can produce, and reaching this
        // pairing meant fixing the document rather than the fixture: `judging` now lets a STANDING
        // order close the run when the peer leaves, instead of calling a banked turn a failure.
        // That is register item 598's real finding and it is not a test convenience — an agent
        // that finishes and exits is the ordinary case.
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({ "done_when": "exits" }))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let id = u64::try_from(id).expect("a run id is not negative");
        // ⚠⚠ THE ORDER IS GIVEN TO A RUN THAT IS PROVABLY DRIVING, not to one that may not have
        // started: a delivery means the prompt is in the pane and the loop is waiting on its peer,
        // which is exactly the moment a person watching a long run reaches for `sprag stand-down`.
        drop(driving(&external, id, Duration::from_secs(30)));
        external
            .invoke(
                STAND_DOWN_ACTION,
                IntrospectValue::Json(json!({ "id": id })),
            )
            .expect("an ai_loop run reads a stand-down");
        // ⚠ NOTHING IS CANCELLED HERE, and that absence is the whole gate. The sibling above ends
        // its run with the registry's own cancel to reach the pairing item 594 measured; this one
        // lets the document do what the order asks and reports what came out.
        let entry = ended(&registry, id, Duration::from_secs(60));
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );

        // ⚠⚠⚠ THE PRECONDITION, ASSERTED SO THE PIN CANNOT DRIFT INTO PINNING SOMETHING ELSE: the
        // turn really did complete. Without this line a run that failed for any other reason would
        // satisfy everything below, and the whole point is that the work WAS banked first.
        let walk = entry[RUN_JOURNAL_KEY].as_array().expect("a walk");
        assert!(
            walk.iter().any(|step| step["note"]
                .as_str()
                .is_some_and(|note| note.contains("TurnDone"))),
            "⚠⚠⚠ this run's turn must have COMPLETED, or the sentence below is right rather than \
             wrong and this gate is pinning nothing: {entry:?}",
        );
        assert_eq!(
            entry["state"]["outcome"]["state"],
            json!("failed"),
            "⛔ IF THIS IS RED, GOOD — a stood-down run whose peer exited after banking its turn no \
             longer reports `failed`. Delete this gate and pay register item 598, which is now \
             reachable from this door: {entry:?}",
        );
        // ⚠⚠⚠⚠⚠ **AND THE SENTENCE MUST NOT CALL THAT BANKED TURN LOST.** The precondition above
        // proves a turn COMPLETED, so *the turn it had going was NOT banked* is false in the one
        // direction a report must never be wrong in: the alarming answer and the relieved one
        // swapped. Register item 604's whole harm is this line, and item 594 exists because the
        // same pair was unreadable once already.
        let said = entry[RUN_STOOD_DOWN_KEY]
            .as_str()
            .expect("the order was given, so its sentence is published");
        assert!(
            !said.contains("NOT banked"),
            "⛔⛔⛔⛔ ITEM 604: this run banked a turn and its agent then left, and the sentence \
             tells the person their work was lost. Said {said:?} for {entry:?}",
        );
        // ⚠⚠⚠⚠⚠ **`BANKED and kept` AND NOT MERELY `kept`.** The arm for *nobody counted* also
        // ends in the word `kept` — *cannot say what was kept* — so an assertion on that word alone
        // passes on the branch this gate exists to rule out, and would have: the first run of this
        // gate went green on it. A control that shares a word with the failure it is guarding
        // against is not a control.
        assert!(
            said.contains("BANKED and kept"),
            "⚠⚠⚠⚠⚠ AND IT HAS TO SAY SO, not merely stop lying: this run BANKED a turn, so the \
             sentence has to report the count the plugin measured rather than fall back to *this \
             run does not report completed work*. Said {said:?}",
        );
    }

    /// ⛔⛔⛔⛔ **AN ORDER ONLY ONE PLUGIN CAN READ IS REFUSED AT THE DOOR, NAMING WHY** — register
    /// items 539 and 597 together, because they are one defect wearing two words.
    ///
    /// # What a person is told today, and what happens
    ///
    /// `RunContext::held` and `RunContext::stood_down` have exactly ONE reader each in this
    /// workspace — `OuterLoop::pump` — and two standing ratchets count them. Every other plugin is
    /// handed the same order and drives straight on. So `sprag hold-run` and `sprag stand-down`
    /// against an `orchestrator`, a `pipe`, a `dialogue` or an `agent` run **answer as though they
    /// worked and change nothing**, and the sentences the CLI prints — *"it parks at its next pass
    /// … nothing further is typed at the pane while it waits"*, *"it stops at its next milestone,
    /// and its work is kept"* — are false for four of the five.
    ///
    /// ⚠⚠⚠ **THE COST IS THE ANSWER, NOT THE MISSING FEATURE.** A person who holds an
    /// `orchestrator` to read its pane is told the pane is now still, and it is not. An
    /// `orchestrator` run is the LONG UNATTENDED one, so *pause it and let me look* is exactly what
    /// somebody reaches for.
    ///
    /// ⚠⚠ **AND THE REFUSAL IS THE EXTENSIBLE HALF.** It is not a list of plugin names: the run is
    /// refused because its plugin ANSWERED that it does not read the order, so the day a second
    /// plugin implements one, its own answer lifts the refusal with nothing here to remember.
    #[test]
    fn an_order_a_runs_plugin_cannot_read_is_refused_rather_than_quietly_accepted() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "max_iterations": 1_000_000,
                })),
            )
            .expect("a well-formed orchestrator run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let id = u64::try_from(id).expect("a run id is not negative");

        for (action, args) in [
            (STAND_DOWN_ACTION, json!({ "id": id })),
            (HOLD_RUN_ACTION, json!({ "id": id, "held": true })),
        ] {
            let answer = external.invoke(action, IntrospectValue::Json(args));
            let Err(refusal) = &answer else {
                panic!(
                    "⛔⛔⛔ ITEMS 539/597: `{action}` was ACCEPTED for a run whose plugin has no \
                     reader for it. The caller was told it worked, the run drives straight on, and \
                     the CLI prints a promise about a pane that is not going to go still. Got \
                     {answer:?}"
                );
            };
            let said = format!("{refusal:?}");
            assert!(
                said.contains("orchestrator"),
                "⚠⚠⚠ AND THE REFUSAL MUST NAME WHICH PLUGIN CANNOT READ IT. *Refused* alone sends \
                 a person to guess whether they typed the wrong id; the fact they need is that \
                 THIS KIND OF RUN has no reader for the order. Got {said}",
            );
        }

        assert!(
            lock(&registry).cancel(RunId(id)),
            "the run this gate started is one the registry can stop",
        );
    }

    /// ⛔⛔⛔⛔ **A CANCEL IS RAISED; IT IS NOT ALWAYS WHAT ENDED THE RUN, AND THE SENTENCE MUST NOT
    /// SAY OTHERWISE** — register item 596's second half, and a defect this repository wrote and
    /// caught in the same round.
    ///
    /// # Where this came from, because it is worth not forgetting
    ///
    /// The first mutation aimed at the gate above — fusing the two cancellers again — went red on
    /// a DIFFERENT assertion, and what it printed was a run that had ended **`failed`** (its pane
    /// never showed the readiness marker) while [`RUN_CANCELLED_BY_KEY`] said *"a person cancelled
    /// this run, so the turn it was in the middle of was thrown away"*. **The key was reporting the
    /// ORDER and letting the reader assume the ENDING** — which is register item 594's finding
    /// exactly, reproduced one key over by the person who had just written 594's fix.
    ///
    /// ⚠⚠⚠ **THIS GATE EXISTS BECAUSE THE LIVE ONE CANNOT REACH THESE ARMS ANY MORE.** The gate
    /// above now waits for a delivery before cancelling, precisely so its runs end `cancelled` —
    /// so the branches where a cancel did NOT do the ending are unreachable there, and a branch
    /// nobody drives is a branch nobody is testing. [`cancel_sentence`] is a pure function of the
    /// two facts, so it is driven directly, over every [`crate::runs::RunState`] there is.
    #[test]
    fn a_cancel_that_did_not_end_the_run_does_not_claim_the_turn_was_thrown_away() {
        use crate::runs::{Canceller, RunState};

        // ⚠ THE ONE PAIRING WHERE THE CANCEL IS THE ENDING, and the only one entitled to speak in
        // the canceller's own words with nothing hedged.
        let did_end = cancel_sentence(
            Canceller::Person,
            &RunState::Done {
                outcome: Box::new(finished(OutcomeState::Cancelled, 0)),
                output: None,
            },
        );
        assert_eq!(
            did_end,
            Canceller::Person.describe(),
            "⚠⚠ a run the cancel DID end must be described in the canceller's own words, with no \
             hedge added — a warning on the healthy pairing is how a reader learns to skip the \
             warning on the broken one. Got {did_end:?}",
        );

        // ⚠⚠⚠⚠ AND EVERY OTHER STATE THERE IS. Listed rather than globbed, so a new `RunState`
        // arrives as a compile error at `cancel_sentence` and as a silent omission here — which is
        // this repository's *a list with no glob decides alone* rule, and the reason the match in
        // that function has no `_` arm.
        for (label, state) in [
            ("a run that failed on its own", {
                RunState::Done {
                    outcome: Box::new(finished(OutcomeState::Failed, 0)),
                    output: None,
                }
            }),
            ("a run that is still going", RunState::Running),
            (
                "a run whose driver died",
                RunState::Panicked("boom".to_owned()),
            ),
            ("a run whose daemon died", RunState::Interrupted),
        ] {
            let said = cancel_sentence(Canceller::Shutdown, &state);
            assert!(
                !said.starts_with(Canceller::Shutdown.describe()),
                "⛔⛔⛔ ITEM 596: {label} is being described as though the cancel is what finished \
                 it. The cancel was RAISED over this run and something else ended it, so a reader \
                 handed the canceller's bare words goes looking for a decision that explains an \
                 ending it did not cause. Got {said:?}",
            );
            assert!(
                said.contains("NOBODY"),
                "⚠⚠⚠ AND WHO RAISED IT MUST SURVIVE THE HEDGE. The hedge says the cancel is not \
                 the ending; it must not swallow the fact this key exists to publish, or the \
                 broken pairing becomes LESS informative than the healthy one. For {label}, got \
                 {said:?}",
            );
            // ⛔⛔⛔⛔ AND NOT THE ENDING WORD, WHICH IS THE HALF THIS GATE LEARNED LAST AND THE
            // HARD WAY. The first version of this key embedded `Canceller::describe` in every arm,
            // and that sentence contains **cancelled** — so a run that was STILL RUNNING carried
            // the word for a run that is over. Two integration suites waited for exactly that word
            // and were satisfied by a live run; a person scanning `sprag runs` for it would have
            // been misled identically. A word that carries a conclusion may appear only where the
            // conclusion holds.
            assert!(
                !said.contains("cancelled"),
                "⛔⛔⛔ {label} is not a run that ended `cancelled`, and its sentence uses that \
                 word anyway — which is the reading this whole item exists to stop being \
                 ambiguous. Got {said:?}",
            );
        }
    }

    /// ⚠⚠⚠ **A BRIEF THIS BUILD CANNOT DRIVE TO THE END IS REFUSED AT THE DOOR, NAMING THE KNOB** —
    /// and the DOCUMENT'S OWN SHIPPED NUMBERS ARE NO LONGER ONE OF THEM.
    ///
    /// # ⚠⚠⚠ What this gate asserted until `restarting` was built
    ///
    /// `ai_loop.scxml` ships `reflect_every: 8` beside `max_turns: 40`, so the DEFAULT numbers walk
    /// into `reflecting` at turn eight — and this surface REFUSED them, because the session-replace
    /// lifecycle behind that state did not exist. A caller who copied the numbers off the document got
    /// a sentence telling them to raise `reflect_every`.
    ///
    /// It is built, so **the shipped pair is now a RUN**, and that is asserted here rather than left
    /// as an absence: a refusal that quietly stopped happening would leave the wire's own grammar
    /// documenting a constraint nothing enforces.
    ///
    /// ⚠⚠ The refusal that REMAINS is the other end of the same arithmetic — a loop allowed no turn at
    /// all — and it is still SYNCHRONOUS and still carries a sentence, which is this surface's own
    /// rule: a caller's mistake is answered at the door with what to change, never as an `outcome` a
    /// minute later.
    #[test]
    fn a_loop_briefed_into_an_unbuilt_state_is_refused_with_the_knob_that_fixes_it() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        // ⚠⚠⚠ A LOOP ALLOWED NO TURN, which is the one arm of this refusal that is left.
        let refused = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({ "max_turns": 0 }))),
            )
            .expect_err("a loop allowed no turns can only judge itself exhausted");
        let sentence = refused
            .reason()
            .map(ToString::to_string)
            .unwrap_or_default();
        assert!(
            sentence.contains("max_turns"),
            "⚠⚠⚠ the refusal must name the knob, because a caller cannot act on a sentence that \
             names none: {sentence:?}",
        );
        assert!(
            lock(&registry).snapshot().is_empty(),
            "⚠⚠ AND NO RUN SLOT WAS TAKEN. A refusal that had already registered a run would have \
             spent the thing refusing early exists to save",
        );

        // ⚠⚠⚠ AND THE DOCUMENT'S OWN SHIPPED PAIR IS A RUN. This used to be the REFUSED case; the
        // session-replace lifecycle it needed is built, and a caller copying the template's numbers
        // gets the loop the template describes.
        external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(
                    pane,
                    json!({ "max_turns": 40, "reflect_every": 8 }),
                )),
            )
            .expect(
                "⚠⚠⚠ the template's own `reflect_every: 8` against `max_turns: 40` must START — it \
                 reaches `reflecting`, and `reflecting` is served",
            );
        lock(&registry).cancel_all();
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THIS REPOSITORY'S KIND DOCUMENT REACHES A RUN THAT NAMED NOTHING** — register item
    /// 492, and the gate that closes a hole the register had already measured and registered.
    ///
    /// # ⚠⚠⚠⚠⚠ What was green while it was broken
    ///
    /// Nine of a brief's fields fall back to `debt_loop.scxml`, and **deleting any one of those
    /// fall-throughs left the entire workspace GREEN.** That was measured for `max_turns` on item
    /// 312's round and written into `sprag_plugin`'s own gate as a registered residue; item 492's
    /// round measured it again for `context_ceiling` and got the same answer. The consequence is
    /// not hypothetical: the kind had authored a ceiling since 2026-08-18, nothing carried it, and
    /// item 477 measured `reviewing` taking the fall-back **eight times out of eight** on a live
    /// run — a state that never once decided, with every gate over it passing.
    ///
    /// # ⚠⚠⚠ Why this can exist now and could not before
    ///
    /// The residue named its own blocker: *"what would catch it is an observable of the RESOLVED
    /// value on a run started through the wire"*, and the driver's readers are crate-private to
    /// `sprag_plugin`. `ai_loop_brief` is that observable — the door's own resolution, handed back
    /// instead of consumed in place. **This asks the real function what a real request plus the real
    /// kind document resolve to**, which is why it is not a test re-implementing the line it checks.
    ///
    /// ⚠⚠ It asserts the AGREEMENT rather than a number: what the kind's document says is that
    /// document's business, and a number pinned here would be a second place it lives. What must
    /// hold is that the two are the same value and that it is one `reviewing` can decide on.
    #[test]
    fn a_kind_documents_judgements_reach_a_run_that_named_none_of_them() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");

        // The fixture minus every key the kind is meant to answer, which is the only way to ask
        // whose value arrived.
        let mut declining = ai_loop_request(PaneId(1), json!({}));
        for key in ["max_turns", "reference"] {
            declining
                .as_object_mut()
                .expect("an object")
                .remove(key)
                .unwrap_or_else(|| panic!("the fixture supplies {key}, which this gate declines"));
        }
        let map = declining.as_object().expect("an object");

        // ⚠⚠⚠⚠⚠ THE PREMISE, ASSERTED HERE RATHER THAN ASSUMED FROM THE LINES ABOVE — register
        // item 738. Every assertion in this gate is about a value the CALLER DID NOT SEND, and
        // against a fixture that sends it they are all vacuously true of a door that ignores the
        // kind entirely. The fixture is shared and grows; a key added to it tomorrow would empty
        // this gate silently, so what the request does NOT hold is checked in the gate.
        for key in ["max_turns", "reference", "closing_rules", "working_rules"] {
            assert!(
                !map.contains_key(key),
                "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE REQUEST NAMES {key:?}: the whole question is \
                 whose value arrives when the caller names none, and a fixture that supplies it \
                 answers the other question with the same green",
            );
        }

        let brief = ai_loop_brief(map, &kind).expect("a well-formed request resolves");

        let ceiling = brief.context_ceiling.expect(
            "⚠⚠⚠⚠⚠ ITEM 492: a run that named no ceiling must arrive holding THIS REPOSITORY'S — \
             the kind document has authored one since 2026-08-18 and until the door carried it, \
             `reviewing` decided on 0 on every run anybody has ever driven",
        );
        assert_eq!(
            Some(ceiling),
            kind.context_ceiling(),
            "and it must be the kind's own number rather than one this door invented",
        );
        assert!(
            ceiling > 0,
            "⚠⚠⚠ and a number `reviewing` can decide on: every deciding edge in that state is \
             guarded on `context_ceiling > 0`, so a zero is the fall-back this item exists to get a \
             run out of. Read {ceiling}",
        );

        // ⚠⚠⚠ AND ITS NEIGHBOURS ON THE SAME ROAD, which is what makes this a CLASS gate rather
        // than a second copy of the ceiling's. Each of these was equally unheld, and the register's
        // own measurement was taken against the first of them.
        assert_eq!(
            brief.max_turns,
            kind.turn_budget(),
            "⚠⚠⚠⚠ the BUDGET is the one the residue was measured on (item 312): a debt run ends on \
             its work, and that decision is a word in the kind's document that nothing carried",
        );
        assert_eq!(
            brief.reflect_every,
            kind.reflect_every(),
            "⚠⚠ and the cadence, which a kind that declines the budget MUST answer or no run of it \
             starts at all",
        );
        assert_eq!(
            brief.milestone_check,
            kind.milestone_check(),
            "⚠⚠⚠ and WHO CERTIFIES A MILESTONE — item 428's second half, where a live run judged \
             `NOTHING CHECKED THAT CLAIM` while this document named a checker",
        );
        assert_eq!(
            brief.closing_rules,
            kind.closing_rules(),
            "and what a run of this kind owes at its ending",
        );
        // ⚠⚠⚠⚠⚠ AND THE TWO REGISTER ITEM 738 ADDED, which are the same defect one level out: 492
        // and 494 were decisions a kind could not make because no channel carried them; these were
        // decisions a kind could not make because the CALLER had to make them on every launch. So
        // they lived in somebody's memory, were retyped by hand into each firing, and vanished with
        // the session that held them.
        assert_eq!(
            brief.working_rules,
            kind.working_rules(),
            "⚠⚠⚠⚠⚠ ITEM 738: the rules every turn of this repository's debt runs is held to. Until \
             this line they were typed into `north_star` BY HAND on every launch — about 2 KB of \
             them, out of one session's context — and the more conscientious the supervisor, the \
             larger the copy",
        );
        assert!(
            brief.working_rules.is_some(),
            "⚠⚠⚠ and the kind must really author some, or the line above is two `None`s agreeing: \
             the template ships `''`, so a run with nothing here composes exactly the prompt it \
             always did and this whole channel would be green about nothing",
        );
        assert_eq!(
            Some(brief.reference.clone()),
            kind.reference(),
            "⚠⚠⚠⚠⚠ ITEM 738: where a run of this kind STARTS READING. This key was `require_str`, \
             so omitting it was malformed rather than deferring — item 312's finding at a string \
             instead of a count — and a required judgement is a decision the document is \
             structurally forbidden from making",
        );
        assert!(
            !brief.reference.contains("edit me"),
            "⚠⚠⚠⚠ AND NOT THE TEMPLATE'S PLACEHOLDER: the fall-through stops at the kind on \
             purpose, because `(edit me) paths, URLs or repos to consult` is composed into the \
             prompt exactly as written and R380 measured a live agent reading three of five \
             clauses that way. Got {:?}",
            brief.reference,
        );
        assert_eq!(
            brief.service.is_some(),
            kind.service_outage().is_some(),
            "and what its peer prints when the service fails, which turned a dead run into a wait",
        );
        assert!(
            brief.may_answer.is_some(),
            "⚠⚠⚠ and the standing yesses: an empty consent list met `Do you want to make this \
             edit?` on the first milestone and stood there until a ceiling ended the run",
        );
        // ⚠⚠⚠⚠⚠ AND THE CEILING'S TWIN — register item 494, and the reason this gate's CLASS
        // framing earned its keep: the sentence in the template that sent a reader to
        // `context_ceiling` says the same thing about this number, and it went a whole further
        // round with no reader, no field and no key.
        assert_eq!(
            brief.reflect_after_refusals,
            kind.reflect_after_refusals(),
            "⚠⚠⚠⚠⚠ ITEM 494: how patient to be with a refusing check is this document's judgement \
             about its own checker — a whole `claude -p` per claimed milestone here — and until the \
             door carried it, `judging` spent the template's three on every run anybody has ever \
             driven",
        );
        assert_eq!(
            brief.reflect_after_refusals,
            Some(2),
            "⚠⚠ and this repository's document says 2 rather than the template's 3, which is what \
             makes the line above evidence instead of two `None`s agreeing: item 448 gave every \
             refusal the check's own words, so the template's own comment says a kind that finds \
             three slack now has a fact it did not have then",
        );

        // ⚠⚠⚠ THE CONTROL. Without it a door that IGNORED every caller and always used the kind's
        // values would satisfy every assertion above — which is the opposite defect and just as
        // silent.
        let named = ai_loop_request(
            PaneId(1),
            json!({ "context_ceiling": 4242, "reference": "what this caller wrote" }),
        );
        let brief = ai_loop_brief(named.as_object().expect("an object"), &kind)
            .expect("a caller naming a ceiling resolves");
        assert_eq!(
            brief.context_ceiling,
            Some(4242),
            "a caller's own number must still win over the kind document's",
        );
        assert_eq!(
            brief.reference, "what this caller wrote",
            "⚠⚠⚠ AND THE CONTROL REACHES ITEM 738'S SIDE TOO: a document that always won would \
             satisfy every assertion above, and it would ALSO delete what a caller wrote — which \
             for this key is the whole of what `reflecting` hands a replacement session",
        );
    }

    /// ⛔⛔⛔⛔⛔ **EVERY CEILING THAT CAN END A RUN IS ONE THIS REPOSITORY'S DOCUMENT SET** —
    /// register item 738, layer 1, and the gate the item asked for in exactly these words: *walk
    /// `Ceiling::ALL` and ask the document*.
    ///
    /// # ⚠⚠⚠⚠⚠ What was true until this round, measured in this daemon's own registry
    ///
    /// `state/sprag/sprag-loop.runs.json` holds 49 runs. **Eight ended `exhausted (cost)`**, every
    /// one between 65,809 and 68,658 bytes — [`DEFAULT_MAX_BYTES`], which is 64 KiB — while the
    /// largest run that CONVERGED spent **516,020** bytes over 1,231 iterations. So this daemon's
    /// backstop was not a backstop for a debt loop, it was the ceiling that bit first, and it bit
    /// mid-round with the work uncommitted in the tree. Nothing in any document could say
    /// otherwise: `max_turns` and `hold_within_ms` were a kind's, and the three [`Guardrails`] were
    /// the caller's or this daemon's constants.
    ///
    /// ⚠⚠ What stood in for it was a guard chain in an untracked launcher script — refuse the
    /// launch unless a person names all three by hand. **A copy of somebody's memory is not a
    /// spec**, and it is the owner's judgement of that copy that filed this item.
    ///
    /// # ⚠⚠⚠⚠ Why the population is [`Ceiling::ALL`] and why there is NO exemption arm
    ///
    /// A gate over a hand-written list of three would have been green on the day a fourth ceiling
    /// arrived, saying nothing — item 470's *a list with no glob decides alone*, and
    /// [`Ceiling`]'s own doc records that exact failure happening to `Hold`. So the loop walks the
    /// closed set and the `match` inside it is EXHAUSTIVE: a sixth ceiling does not slip past this
    /// gate, it fails to compile until somebody says which bound fires it and who authors that.
    ///
    /// ⛔ **AND EVERY ARM MUST ANSWER WITH THE DOCUMENT'S OWN VALUE.** An arm that answered *this
    /// one is nobody's* would be the escape hatch that disarms the gate — rule: an unclassified
    /// ceiling is a RED, not a pass. `Hold` was that ceiling until this round, and the honest way
    /// to keep the gate strict was to give the kind a channel for it rather than to write the
    /// exemption.
    ///
    /// ⚠ The numbers themselves are NOT pinned here. What `debt_loop.scxml` says is that document's
    /// business and a number in this file would be a second place it lives; what must hold is that
    /// a launch naming nothing is bounded by what that document says, and that it is not this
    /// daemon's default.
    #[test]
    fn every_ceiling_that_can_end_a_run_is_one_this_repositorys_document_set() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");

        // ⚠⚠⚠⚠⚠ THE PREMISE. Every assertion below is about a bound the CALLER DID NOT NAME, and
        // against the shared fixture — which names guardrails and a turn budget — they are all
        // vacuously satisfied by a door that never consults a document. So the keys come out and
        // their absence is checked here rather than assumed from the removal.
        let mut declining = ai_loop_request(PaneId(1), json!({}));
        for key in ["guardrails", "max_turns"] {
            declining
                .as_object_mut()
                .expect("an object")
                .remove(key)
                .unwrap_or_else(|| panic!("the fixture supplies {key}, which this gate declines"));
        }
        let map = declining.as_object().expect("an object");
        for key in ["guardrails", "max_turns", sprag_plugin::HOLD_WITHIN_KEY] {
            assert!(
                !map.contains_key(key),
                "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE REQUEST NAMES {key:?}: the question is which \
                 ceiling bounds a launch that named none, and a fixture that names one answers a \
                 different question with the same green",
            );
        }

        // ⚠⚠⚠⚠⚠ THE WHOLE ROAD, NOT THE LAST STEP OF IT — and this is the shape a mutation
        // demanded rather than a preference. The first draft called `parse_guardrails` with the
        // kind's clause handed straight to it, and **deleting the wiring that carries that clause
        // out of `build_plugin` left this gate GREEN**: it was measuring the fall-through and not
        // the channel that feeds it, which is precisely the defect items 312 and 492 each measured
        // once. So the run is BUILT the way the door builds it, and what its bounds are asked of is
        // the plugin the door produced.
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        let mut on_the_pane = declining.clone();
        on_the_pane
            .as_object_mut()
            .expect("an object")
            .insert("pane".to_string(), json!(pane.0));
        let built_from = on_the_pane.as_object().expect("an object");
        let (plugin, _label) = external
            .build_plugin(built_from)
            .expect("a request naming no bounds is well-formed");
        assert!(
            matches!(plugin, PluginKind::AiLoop(..)),
            "the control: this request must build a loop, or `own_bounds` is answering for \
             something else",
        );

        // The three roads a bound travels, all asked of the REAL functions rather than restated:
        // the kind's own clause, the brief the door resolves, and the guardrails the door resolves
        // FROM THE PLUGIN IT BUILT.
        let unit = plugin.cost_unit();
        let authored = plugin.own_bounds();
        assert_eq!(
            authored,
            kind_guardrails(&kind, unit).expect("its guardrail clause must be readable"),
            "⛔⛔⛔⛔⛔ THE PLUGIN THE DOOR BUILT DOES NOT CARRY WHAT ITS OWN DOCUMENT SAID. The \
             clause is readable and the run will not be bounded by it — a channel broken between \
             the document and the driver, which is item 492's shape and is silent",
        );
        let brief = ai_loop_brief(map, &kind).expect("a well-formed request resolves");
        let resolved =
            parse_guardrails(built_from, unit, authored).expect("its guardrails resolve");

        for ceiling in Ceiling::ALL {
            // ⚠⚠⚠ EXHAUSTIVE, AND THAT IS THE GATE'S SPINE. Each arm answers with what the run is
            // BOUND BY and what the document SAID, rendered through one type so the comparison
            // cannot be between two readers. A sixth ceiling stops the build here.
            let (bound, said) = match ceiling {
                Ceiling::Iterations => (
                    Some(u64::from(resolved.max_iterations)),
                    authored.max_iterations.map(u64::from),
                ),
                Ceiling::Cost => (
                    resolved.max_cost.map(Cost::amount),
                    authored.max_cost.map(Cost::amount),
                ),
                Ceiling::Duration => (
                    resolved.max_duration.map(|it| it.as_secs()),
                    authored.max_duration.map(|it| it.as_secs()),
                ),
                // ⚠ The plugin's own budget, which this kind DECLINES with a word rather than a
                // number — so the pair compared is the decision, not an amount. `Counted::Never`
                // is the only value here that is not a count, and folding it to one would be the
                // `probe_absent` defect this document's own comment records.
                Ceiling::Turns => {
                    assert_eq!(
                        brief.max_turns,
                        kind.turn_budget(),
                        "⚠⚠⚠⚠ {ceiling:?}: a run that named no budget must be bounded by what this \
                         repository's document decided, and this one DECLINES the count — a debt \
                         run's job is a list nobody has finished",
                    );
                    assert!(
                        kind.turn_budget().is_some(),
                        "⚠⚠⚠ and the document must really decide it: a `None` here is this gate \
                         comparing two absences and calling them agreement",
                    );
                    continue;
                }
                Ceiling::Hold => (
                    brief.hold_within_ms.map(|it| it as u64),
                    kind.hold_within_ms().map(|it| it as u64),
                ),
            };
            let said = said.unwrap_or_else(|| {
                panic!(
                    "⛔⛔⛔ {ceiling:?} CAN END A RUN OF THIS KIND AND THIS KIND'S DOCUMENT DOES NOT \
                     SET IT. That is register item 738's whole subject: the number then comes from \
                     this daemon's constants or from whoever remembered to type it, and the \
                     measurement says what that costs — 8 of 49 runs here ended `exhausted (cost)` \
                     at a 64 KiB default while a converging run spends half a megabyte. Author it \
                     in `debt_loop.scxml`; do NOT add an exemption arm above."
                )
            });
            assert_eq!(
                bound,
                Some(said),
                "⚠⚠⚠⚠⚠ {ceiling:?}: the document names a bound and the run did not get it, so the \
                 channel is broken somewhere between the clause and the driver — which is item \
                 492's shape and is SILENT, because a run bounded by a default looks exactly like \
                 a run bounded by a decision",
            );
        }

        // ⚠⚠⚠ THE CONTROL, and for this item it is the sharpest assertion in the gate: the value
        // that arrives must not be the daemon's own. Without it, a document that happened to
        // author 64 KiB and a door that ignored the document entirely are the same green — and
        // that green is the defect, not the fix.
        assert_ne!(
            resolved.max_cost,
            Some(Cost::Bytes(DEFAULT_MAX_BYTES)),
            "⛔⛔⛔⛔⛔ THE RUN CAME UP ON THIS DAEMON'S OWN 64 KiB, which is the number that ended \
             eight of this daemon's forty-nine recorded runs mid-round. A debt run that converges \
             spends 516,020 bytes",
        );
        assert!(
            resolved.max_cost.map(Cost::amount).unwrap_or_default() > 516_020,
            "⚠⚠⚠⚠ and it must exceed the largest run this daemon has ever recorded CONVERGING \
             (run 17: 1,231 iterations, 516,020 bytes), or the ceiling still cuts the work this \
             loop exists to do. Read {:?}",
            resolved.max_cost,
        );

        // ⛔⛔⛔⛔⛔ AND THE SHAPE THE ITEM WAS ACTUALLY FILED ON, WHICH IS NOT THE ONE ABOVE. Run 41
        // of this daemon's registry named `max_iterations` and `max_seconds` and **left `max_bytes`
        // out** — a caller who had thought about two ceilings and not the third — and died
        // `exhausted (cost) after 49 iterations, 65809 bytes` with its round uncommitted. A
        // fall-through that only fires when the whole `guardrails` object is absent would leave
        // that exact launch on the daemon's 64 KiB, so the per-FIELD fall-through is the claim and
        // this is the arm that holds it.
        let partly = ai_loop_request(
            PaneId(1),
            json!({ "guardrails": { "max_iterations": 60, "max_seconds": 21600 } }),
        );
        let run_41 = parse_guardrails(partly.as_object().expect("an object"), unit, authored)
            .expect("the launch that filed this item resolves");
        assert_eq!(
            run_41.max_cost, authored.max_cost,
            "⛔⛔⛔⛔⛔ THE LAUNCH THIS ITEM WAS FILED ON IS STILL ON THE DAEMON'S DEFAULT. Naming \
             two ceilings and forgetting the third is what a person does, and it is what run 41 \
             did — so a fall-through keyed on the whole object rather than on each field would fix \
             nothing that actually broke",
        );
        assert_eq!(
            (run_41.max_iterations, run_41.max_duration),
            (60, Some(std::time::Duration::from_secs(21_600))),
            "⚠⚠⚠ and the two the caller DID name are still theirs: a per-field fall-through that \
             overrode a named bound would be the document deciding what a person already decided",
        );

        // ⚠⚠ AND THE OTHER DIRECTION: a caller who names a bound still wins, or *the document is
        // consulted* and *the document always wins* are one green and the second silently discards
        // what somebody asked for.
        let named = ai_loop_request(
            PaneId(1),
            json!({ "guardrails": { "max_bytes": 4096, "max_iterations": 7, "max_seconds": 11 } }),
        );
        let over = parse_guardrails(named.as_object().expect("an object"), unit, authored)
            .expect("a caller naming guardrails resolves");
        assert_eq!(
            (over.max_iterations, over.max_cost, over.max_duration),
            (
                7,
                Some(Cost::Bytes(4096)),
                Some(std::time::Duration::from_secs(11))
            ),
            "⚠⚠⚠ a caller's own bounds must still win over the kind document's, on every one of \
             the three",
        );
    }

    /// ⛔⛔⛔⛔⛔ **WHAT THIS REPOSITORY DOES ABOUT A SILENT CHECKER IS ITS DOCUMENT'S, AND BOTH
    /// HALVES REACH THE RUN** — register item 741, and register item 738's third layer applied to a
    /// DECISION rather than to a number.
    ///
    /// # ⚠⚠⚠⚠ Why the pair and not one clause
    ///
    /// A silence is two facts wearing one word — a checker that produced no verdict wants asking
    /// again, one that answered prose wants its prompt fixed — and a run meets exactly one of them.
    /// So a document that authored one clause would answer some of its runs and be silent about the
    /// rest, and the measurement says how the population splits: across this repository's whole run
    /// log, 15 of 19 silences were `NotAVerdict` and 4 were `Unfinished`.
    ///
    /// ⚠⚠ **BOTH ARE ASSERTED TO BE THE KIND'S OWN VALUES**, never merely present: a door that
    /// invented two sentences would satisfy *the brief carries a pair* while the document that owns
    /// this loop's decisions said nothing at all.
    ///
    /// ⚠ The DISPOSITION — which clause a given silence gets — is the document's, and
    /// `ai_loop`'s `a_check_that_said_nothing_readable_leaves_by_its_own_door_with_its_own_answer`
    /// holds that end. This one holds the channel.
    #[test]
    fn a_kind_says_what_a_silent_checker_owes_and_the_door_carries_both_halves() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");

        // ⚠⚠⚠⚠⚠ THE PREMISE: this kind must really author BOTH, asserted before the door is asked.
        // Against a document that authors neither, every claim below is two absences agreeing —
        // and the honest answer for such a document is `None`, which is a different green.
        let authored = kind
            .unverified_rules()
            .expect("its clauses must be readable")
            .expect(
                "⚠⚠⚠ THE CONTROL: `debt_loop.scxml` must author what a silence owes, or the door \
                 below is carrying nothing and this gate is about an empty pair",
            );
        assert!(
            !authored.unanswered.trim().is_empty() && !authored.unreadable.trim().is_empty(),
            "⚠⚠ and neither half may be blank: an empty clause is composed into the prompt exactly \
             as written, which is what R380 measured a live agent reading as `(edit me)`",
        );
        assert_ne!(
            authored.unanswered, authored.unreadable,
            "⛔⛔⛔ REGISTER ITEM 741: the two clauses are the SAME SENTENCE, so a document that \
             has two channels is saying one thing through both — which is the collapse this item \
             is about, moved from the driver into the document",
        );

        // ── AND THE DOOR CARRIES THEM, over the launch a person actually makes ──────────────
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let launch = json!({
            "plugin": "ai_loop",
            "pane": pane.0,
            "north_star": "SPRAG-NORTH-STAR-CROSSED-THE-WIRE",
            "milestone": "say the marker",
        });
        let map = launch.as_object().expect("an object");
        assert!(
            !map.contains_key("unanswered_rule") && !map.contains_key("unreadable_rule"),
            "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE LAUNCH NAMES EITHER CLAUSE: the question is whose \
             value arrives when the caller names none, and there is no wire key for these at all",
        );
        let brief = ai_loop_brief(map, &kind).expect("a well-formed request resolves");
        assert_eq!(
            brief.unverified_rules.as_ref(),
            Some(&authored),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 741: what this repository does about its own checker's silence \
             did not reach the run. The document authored it, the machine has a door for it, and \
             the channel between them is cut — so a silenced run is told nothing, which is the \
             state before this item was filed",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE LAUNCH THAT STARTS THIS REPOSITORY'S DEBT LOOP IS FOUR ARGUMENTS** — register
    /// item 738's last layer, and the sentence this workspace had been asserting in PROSE.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the four gates beside this one do not say it
    ///
    /// Each of them declines TWO keys from a shared fixture that supplies everything else, so each
    /// answers *whose value arrives for this key*. **None answers the question the item was filed
    /// about**, which is a claim about the WHOLE request: *what does a launch have to type?* That
    /// number is what the item is: the launcher this repository actually fires types
    /// `--reference --match --marker --max-bytes --max-iterations --max-seconds` on every run, out
    /// of a person's memory, and the item's own done-when is that it stops having to.
    ///
    /// # ⚠⚠⚠⚠ Two halves, and only together do they mean anything
    ///
    /// 1. **The PUBLISHED form's required set is exactly the four.** A client builds its call from
    ///    what the daemon publishes, so the day a key goes back to `required` a launcher has to
    ///    type it again — and nothing anywhere would have said so. This half reads the grammar,
    ///    which is the only artefact a caller on the other side of the wire can consult.
    /// 2. **A request holding exactly those four BUILDS a loop.** It builds *only* because the
    ///    document answers every other value: cut the `reference` fall-through and the door refuses
    ///    naming the key, cut the barrier and it refuses naming `agent` and `ready_when`. So the
    ///    door SUCCEEDING here is the end-to-end observation register item 739 arrived at the hard
    ///    way — over the whole set at once rather than one key at a time.
    ///
    /// ⚠⚠ The ceilings are read off **the plugin the door built**, never re-derived, which is what
    /// [`every_ceiling_that_can_end_a_run_is_one_this_repositorys_document_set`](Self) paid a
    /// mutation to learn: a gate handed the document's own answer stays green with the wiring cut.
    ///
    /// ⚠ What is NOT pinned here is any VALUE `debt_loop.scxml` chose — that document's business,
    /// and a number in this file would be a second place it lives. What is pinned is the SIZE of
    /// what a launcher must know, which is the thing that was living in a session's memory.
    #[test]
    fn the_launch_that_starts_this_repositorys_debt_loop_is_four_arguments() {
        // ── HALF ONE: what a caller reading the published grammar has to fill in ────────────
        //
        // ⚠ In declared order, because that is the order a client reading `to_answer` meets them,
        // and an assertion over a SET would go green on a form that had reshuffled into something
        // no launcher could follow.
        let required: Vec<&str> = crate::wire::PluginGrammar::AI_LOOP_FORM
            .args
            .iter()
            .filter(|arg| !arg.optional)
            .map(|arg| arg.name)
            .collect();
        assert_eq!(
            required,
            vec!["plugin", "pane", "north_star", "milestone"],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 738: the launch is no longer four arguments. Every key that \
             becomes required here is a value somebody has to type on every firing — and the place \
             they type it from is a memory that dies with their session, which is the whole of \
             what this item measured. A value with an author belongs in `debt_loop.scxml`; a key \
             this form insists on has no author but the caller.",
        );

        // ── HALF TWO: and a request that fills in exactly those four builds a loop ──────────
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");

        // ⚠⚠⚠⚠⚠ THE DOCUMENT MUST REALLY ANSWER EACH OF THEM, asserted BEFORE the door is asked.
        // Every claim below is *the caller named nothing and got the document's value*, and against
        // a kind that authored nothing they are all satisfied by two absences agreeing.
        assert!(
            kind.reference().is_some(),
            "⚠⚠⚠ the control: this kind must author a `reference`, or *the launcher stopped typing \
             it* means the run reads nothing rather than reads the ledger",
        );
        assert!(
            kind.working_rules().is_some(),
            "⚠⚠⚠ and the standing rules — about 2 KB that used to be retyped into `north_star`",
        );
        let barrier = kind
            .ready_when()
            .expect("its barrier must be readable")
            .expect(
                "⚠⚠⚠ and the barrier the launcher spelled as `--match settles --marker claude`, or \
                 the door below is falling through to nothing",
            );

        // ⚠ BUILT FROM SCRATCH rather than stripped from `ai_loop_request`: the claim is *these \
        // four and nothing else*, and a fixture the gates share is free to grow a key tomorrow —
        // which would leave this gate answering a different question with the same green.
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let launch = json!({
            "plugin": "ai_loop",
            "pane": pane.0,
            "north_star": "SPRAG-NORTH-STAR-CROSSED-THE-WIRE",
            "milestone": "say the marker",
        });
        let map = launch.as_object().expect("an object");
        let mut named: Vec<&str> = map.keys().map(String::as_str).collect();
        named.sort_unstable();
        assert_eq!(
            named,
            vec!["milestone", "north_star", "pane", "plugin"],
            "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE LAUNCH NAMES ANYTHING ELSE: the question is what a \
             caller has to send, and a request carrying a fifth key answers a different one",
        );

        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        let (plugin, label) = external.build_plugin(map).expect(
            "⛔⛔⛔⛔⛔ REGISTER ITEM 738: THE DOOR REFUSED THE FOUR-ARGUMENT LAUNCH. Every value it \
             is missing has an author in `debt_loop.scxml`, so a refusal here means a channel \
             between that document and this door is cut — and the only way to fire a run again \
             would be for a person to type the value back in, which is the state this item exists \
             to leave behind",
        );
        assert!(
            matches!(plugin, PluginKind::AiLoop(..)),
            "the control: this request must build a LOOP, or the assertions below are about \
             something else. Built {label:?}",
        );
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );

        // ── AND EVERY VALUE THE LAUNCHER USED TO TYPE CAME FROM THE DOCUMENT ────────────────
        //
        // ⚠⚠ THE THREE CEILINGS OFF THE PLUGIN THE DOOR BUILT — `--max-bytes --max-iterations
        // --max-seconds`, which is half of what the launcher types and the half that was measured
        // costing a round: run 41 named two of the three and died on the one it forgot.
        assert_eq!(
            plugin.own_bounds(),
            kind_guardrails(&kind, plugin.cost_unit()).expect("its guardrail clause is readable"),
            "⛔⛔⛔⛔ the plugin the door built does not carry the ceilings its own document set, so \
             a launcher that stopped naming them would put its runs back on this daemon's defaults \
             — silently, because a run bounded by a default looks exactly like one bounded by a \
             decision",
        );
        assert!(
            plugin.own_bounds() != AuthoredGuardrails::none(),
            "⚠⚠⚠ and the document must really set them: comparing two empty sets and calling it \
             agreement is how this gate would pass over a kind that authors nothing",
        );

        // ⚠⚠ AND THE OTHER THREE — `--reference`, and the `--match`/`--marker` pair — resolved by
        // the same functions the door used, against the request that named none of them.
        let brief = ai_loop_brief(map, &kind).expect("the four-argument launch resolves a brief");
        assert_eq!(
            (Some(brief.reference.clone()), brief.working_rules.clone()),
            (kind.reference(), kind.working_rules()),
            "⚠⚠⚠⚠⚠ ITEM 738: where a run starts reading, and the rules it is held to, must be the \
             DOCUMENT'S. These were typed by hand into every launch — the ledger's path, and 2 KB \
             of standing rules out of one session's context",
        );
        assert_eq!(
            ai_loop_barrier(map, kind_barrier(&kind).expect("its barrier is readable"))
                .expect("a launch naming neither `agent` nor `ready_when` reaches the document's"),
            barrier,
            "⚠⚠⚠⚠ and the barrier the launcher spelled twice — `--match settles --marker claude` — \
             must be the one its author wrote beside what this peer prints when its service fails",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A LOOP IS NOT BUILT OVER A PANE STANDING OUTSIDE THE TREE ITS KIND WORKS IN** —
    /// register item 738, layer 4, and item 684's remaining half.
    ///
    /// # ⚠⚠⚠⚠⚠ The measurement, and the control that came out of the same restart
    ///
    /// 2026-08-25, read off a live daemon because the owner asked why the same screen kept coming
    /// back: `inner-wz` was `blocked rule=dialog-choice-list` standing in `/home/coin`, showing
    /// *"Quick safety check: Is this a project you created or one you trust?"* — while three
    /// sibling panes standing in their own repositories were all `working`. **The symptom and the
    /// control came from one restart**, which is what made it a measurement rather than a story
    /// about how `claude` behaves.
    ///
    /// ⛔ **AND THE SECOND COST IS NOT THE DIALOG AT ALL.** Since item 710 the milestone checker is
    /// spawned in *the run's repository*, and the repository it is given is the driving pane's
    /// BIRTH directory — the same read this checks. So a loop over a pane in `$HOME` also tells its
    /// checker the work lives in `$HOME`, whether or not any dialog ever appears. That is why this
    /// refuses on placement rather than waiting to see whether a question arrives.
    ///
    /// # ⚠⚠⚠ Why the clause is a MARKER and not a path, which the TESTS decided
    ///
    /// The first form of `debt_loop.scxml`'s clause read `'/home/coin/sprag'` and **nineteen gates
    /// went red at once**: the build machine's checkout is somewhere else entirely. The same
    /// document is compiled into every checkout there will ever be, so a path in it names ONE
    /// machine. `.git` is the fact that is true in all of them, and it separates the measured
    /// symptom from the measured control exactly.
    ///
    /// ⚠⚠ **AND THE FIXTURES WERE WRONG RATHER THAN INCONVENIENCED.** Every pane these gates built
    /// a loop over was born in the runner's `$HOME` — which is the placement that costs a live run,
    /// and which pointed each gate's checker at `$HOME` too. Pointing them at a tree is what makes
    /// them fixtures of a legitimate launch.
    #[test]
    fn a_loop_is_not_built_over_a_pane_standing_outside_the_tree_its_kind_works_in() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");
        let marks = kind.works_in().expect(
            "⛔⛔⛔ THE CONTROL: a kind that names no marker makes every assertion below a statement \
             about `None`, and the door would be checking nothing at all",
        );

        // ⚠⚠⚠⚠⚠ THE PREMISE, and it is what the whole gate is about: a directory that is NOT a
        // tree, standing in for the `$HOME` a pane opened without `-c` is born in. Asserted rather
        // than assumed, because a fixture that accidentally carried the marker would make the
        // refusal below unreachable and the gate green about nothing.
        let bare = std::env::temp_dir().join(format!("sprag-not-a-tree-{}", std::process::id()));
        std::fs::create_dir_all(&bare).expect("a directory that is not a tree");
        assert!(
            !bare.join(&marks).exists(),
            "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE FIXTURE IS A TREE: {bare:?} must not carry {marks:?}",
        );
        let tree = a_tree_to_stand_in();
        assert!(
            tree.join(&marks).exists(),
            "⚠⚠⚠ AND THE CONTROL MUST BE ONE: {tree:?} must carry {marks:?}, or *refused* and \
             *accepted* below are the same case twice",
        );

        let why = ai_loop_stands_where_it_works(Some(&bare), Some(&marks))
            .expect_err("⛔ a loop must NOT be built over a pane standing outside a tree");
        let sentence = why
            .reason()
            .map(ToString::to_string)
            .unwrap_or_else(|| panic!("the refusal must carry a sentence: {why:?}"));
        assert!(
            sentence.contains(&bare.display().to_string()),
            "⚠⚠⚠⚠ 455's RULE: the refusal must name WHERE the pane is standing, because the remedy \
             is to reopen it somewhere else and a sentence that does not say where it is cannot be \
             acted on. Got {sentence:?}",
        );
        assert!(
            sentence.contains(&marks),
            "⚠⚠⚠ and WHAT was looked for, or a reader cannot tell a misplaced pane from a tree this \
             build failed to recognise: {sentence:?}",
        );

        // ⚠⚠ CONTROL ONE: the same check accepts a pane standing in a tree, or this is a door that
        // refuses everything and the nineteen gates it reddened were right for the wrong reason.
        ai_loop_stands_where_it_works(Some(&tree), Some(&marks))
            .expect("a pane standing in a tree is one a loop may be built over");

        // ⚠⚠⚠ CONTROL TWO: a kind that names NO marker is not checked at all — the shipped state of
        // a document other repositories copy. That is an absence of a claim, not an exemption from
        // one: nothing is classified as fine, because nothing was claimed.
        ai_loop_stands_where_it_works(Some(&bare), None)
            .expect("a kind that names no tree makes no claim about where its panes stand");

        // ⛔⛔⛔ CONTROL THREE: a surface that cannot say where the pane was opened is a RED and not
        // a pass — this repository's own rule that a guard which cannot read its subject must not
        // wave it through.
        let blind = ai_loop_stands_where_it_works(None, Some(&marks))
            .expect_err("a surface that cannot answer must not vouch for the pane");
        assert!(
            blind
                .reason()
                .map(ToString::to_string)
                .unwrap_or_default()
                .contains(&marks),
            "⚠⚠ and it must say what it could not check for: {blind:?}",
        );

        // ⛔⛔⛔⛔⛔ AND THE DOOR ITSELF, WHICH IS THE HALF EVERY ASSERTION ABOVE MISSES. Register
        // item 739's round measured this exactly one function over: a gate that hands a resolution
        // its own inputs is GREEN when the wiring that normally feeds it is cut. So the last arm
        // builds a real request over a real pane standing outside a tree, and the door must refuse
        // it — with the wiring intact there is a marker to compare against, and with it cut there
        // is nothing left to refuse on.
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let outside = {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.cwd(&bare);
            lock(&workspace)
                .spawn(command, "outside".to_string(), 80, 24)
                .expect("a pane standing outside a tree")
        };
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            external.pane_start_dir(outside).as_deref(),
            Some(bare.as_path()),
            "⚠⚠⚠ THE PREMISE OF THE ARM BELOW: the world must really say the pane is standing \
             outside a tree, or the refusal it expects has nothing to fire on",
        );
        let built = external.build_plugin(
            ai_loop_request(outside, json!({}))
                .as_object()
                .expect("an object"),
        );
        let Err(refused_at_the_door) = built else {
            panic!(
                "⛔⛔⛔⛔⛔ THE DOOR BUILT A LOOP OVER A PANE STANDING OUTSIDE ITS TREE. Every \
                 assertion above passes with the door's own call to this check deleted — that is \
                 item 739's measured hole, and this arm is what closes it"
            );
        };
        assert!(
            refused_at_the_door
                .reason()
                .map(ToString::to_string)
                .unwrap_or_default()
                .contains(&bare.display().to_string()),
            "⚠⚠ and the door's refusal must be THIS one rather than some other: {refused_at_the_door:?}",
        );
        assert!(
            lock(&workspace).close(outside).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A LAUNCH NOBODY AND NO DOCUMENT ANSWERED IS REFUSED, NAMING EVERY KEY THAT WOULD
    /// FIX IT** — register item 739, and the two arms nothing could reach until this round.
    ///
    /// # ⚠⚠⚠⚠⚠ Why they could not be reached, and why that made a green worthless
    ///
    /// There is ONE loop kind and its constructor always opens the real `debt_loop.scxml` — the
    /// door's own comment says so: *"WHICH KIND IS NOT A WIRE ARGUMENT YET … There is one kind"*.
    /// That document names a `reference` and names a barrier, so every gate driving the resolution
    /// through a `LoopKind` took the SUCCESS path and the refusals ran for nobody. **A green then
    /// means *this arm was never entered*, not *this arm is right*** — items 706 and 482's shape,
    /// and their sentences could rot for as long as they liked.
    ///
    /// ⚠⚠ **THE ONLY WITNESS EITHER ARM HAD WAS A MUTATION.** Item 738's round tripped the
    /// reference refusal by accident while proving a different gate, read its sentence out of a
    /// log, and moved on. **A mutation that happens to trip an arm is not a gate** — nothing runs
    /// it again, and nothing will notice when the sentence stops naming the key.
    ///
    /// # ⚠⚠⚠ What was actually wrong, which is not what item 739 was filed as
    ///
    /// It was filed as *`LoopKind` has one constructor*, with the remedy *let it choose which
    /// document to open*. Re-measured: **both of that item's directions cost more than the defect.**
    /// A document selector needs a second document to select, which the same item forbids (a
    /// test-only copy of the kind is 710 and 722's shape); and making `LoopKind` generic over the
    /// compiled policy would cost the SEVEN readers that go through the generated accessors, whose
    /// whole purpose is that a renamed `<data>` stops the build.
    ///
    /// ⇒ The defect was one function doing two jobs: reading a document AND deciding precedence.
    /// **Precedence is not the business of whoever knows what a kind is.** Split, each resolution
    /// takes the author's ANSWER — and *the author answered nothing* is then a value, not a
    /// document nobody can write.
    ///
    /// ⚠ The half this cannot hold is stated rather than left: that the door passes THIS
    /// repository's answer and not something else is held by
    /// [`a_kind_documents_judgements_reach_a_run_that_named_none_of_them`](Self) and by the barrier
    /// gate below. Neither is the whole claim; a fix that satisfied one and drifted on the other
    /// would not pass both.
    #[test]
    fn a_launch_no_document_answered_is_refused_naming_every_key_that_would_fix_it() {
        // ⚠⚠⚠⚠⚠ THE PREMISE IS THE ARGUMENT ITSELF: `None` is *the author named nothing*, which is
        // the state no `LoopKind` in this repository can be in. Asserting it is asserting what the
        // gate is about, so there is nothing here for a fixture to quietly supply.
        let silent_reference: Option<String> = None;
        let silent_barrier: Option<sprag_plugin::ReadyWhen> = None;

        // ── the reference arm: a launch that named none, against an author who named none ──
        let mut declining = ai_loop_request(PaneId(1), json!({}));
        for key in ["reference", "agent", "ready_when"] {
            declining
                .as_object_mut()
                .expect("an object")
                .remove(key)
                .unwrap_or_else(|| panic!("the fixture supplies {key}, which this gate declines"));
        }
        let map = declining.as_object().expect("an object");
        for key in ["reference", "agent", "ready_when"] {
            assert!(
                !map.contains_key(key),
                "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE REQUEST NAMES {key:?}: both arms are about a \
                 launch that named nothing, and a fixture that names one takes the success path",
            );
        }

        let why = ai_loop_reference(map, silent_reference.clone())
            .expect_err("⛔ a run with no reference from anybody must NOT start");
        let sentence = why
            .reason()
            .map(ToString::to_string)
            .unwrap_or_else(|| panic!("the refusal must carry a sentence: {why:?}"));
        assert!(
            sentence.contains("reference"),
            "⚠⚠⚠⚠ 455's RULE: a refusal must name the key that would fix it, because the whole \
             value of refusing early is that the caller can fix it and call again. Got \
             {sentence:?}",
        );
        assert!(
            sentence.contains("edit me"),
            "⚠⚠⚠ and it must say WHY the fall-through stops here rather than reaching the \
             template: the template's own value is a placeholder R380 measured reaching a live \
             agent, and a reader who does not know that will 'fix' this by making it optional \
             again. Got {sentence:?}",
        );

        // ── the barrier arm: the same launch, against an author who named no peer ──
        let why = ai_loop_barrier(map, silent_barrier)
            .expect_err("⛔ a run with no barrier from anybody must NOT start");
        let sentence = why
            .reason()
            .map(ToString::to_string)
            .unwrap_or_else(|| panic!("the refusal must carry a sentence: {why:?}"));
        for key in ["agent", "ready_when"] {
            assert!(
                sentence.contains(key),
                "⚠⚠⚠⚠ BOTH KEYS, because either one would fix it and a caller told about only one \
                 is told about the wrong one half the time: missing {key:?} in {sentence:?}",
            );
        }
        assert!(
            sentence.contains("kind document"),
            "⚠⚠⚠ and the third road must be named too — authoring it — or a repository that drives \
             one peer is told to retype that peer on every launch, which is item 738's whole \
             subject. Got {sentence:?}",
        );

        // ⚠⚠⚠ THE CONTROL, and without it this gate would pass over a resolution that refuses
        // EVERYTHING. The SAME silent author, plus a caller who said one thing, must start — and
        // it is built from the same declining request so the only difference is the one key.
        let mut speaking = declining.clone();
        let object = speaking.as_object_mut().expect("an object");
        object.insert("reference".to_string(), json!("what this caller wrote"));
        object.insert("agent".to_string(), json!("claude"));
        let spoke = speaking.as_object().expect("an object");
        assert_eq!(
            ai_loop_reference(spoke, silent_reference)
                .expect("a caller who names one still starts"),
            "what this caller wrote",
            "⚠⚠ a silent author must not make a caller's own value unusable",
        );
        assert_eq!(
            ai_loop_barrier(spoke, None).expect("a caller who names the program still starts"),
            sprag_plugin::ReadyWhen::Settles("claude".to_string()),
            "⚠⚠ and naming the PROGRAM must still derive a barrier with no document at all — that \
             road is item 300's and this item did not touch it",
        );
    }

    /// ⚠⚠⚠⚠ **AND A GUARDRAIL A DOCUMENT NAMES THAT NO RUN OF IT CAN HAVE IS REFUSED, NAMING WHAT
    /// THE CLAUSE TAKES** — register item 738, layer 1, and rule: an unclassified key is a RED and
    /// not a pass.
    ///
    /// [`parse_guardrails`] has always refused an unknown key inside a CALLER's `guardrails`, on an
    /// argument that is about bounds rather than about tidiness: *ignoring an ordinary argument
    /// makes a verb do less than asked and the caller can see it; ignoring a BOUND makes the run do
    /// more, without limit, and answers success.* A document is now a second author of the same
    /// object, so it meets the same refusal — a `debt_loop.scxml` that spelled `max_byte` would
    /// otherwise get 64 KiB while plainly naming two megabytes.
    ///
    /// ⚠⚠ **THE KEYS ARE THE PUBLICATION'S**, so this also holds the pairing that made the refusal
    /// possible: a token bound named for a byte-spending run is an unknown key rather than a
    /// crossed currency, because [`PluginGrammar::guardrail_fields`](crate::wire::PluginGrammar::guardrail_fields)
    /// offers each unit only its own.
    #[test]
    fn a_guardrail_a_kind_document_names_that_no_run_of_it_can_have_is_refused() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");

        // THE CONTROL: the real document is read without complaint, or the refusal below is about
        // a reader that refuses everything.
        kind_guardrails(&kind, Cost::Bytes(DEFAULT_MAX_BYTES))
            .expect("this repository's own clause must be admitted");

        // ⚠ THE SAME CLAUSE, ASKED FOR A RUN THAT SPENDS TOKENS. `max_bytes` is not a guardrail of
        // one, so the document's own key becomes the unknown one — which is how this gate reaches
        // the refusal without editing the shipped document.
        let why = kind_guardrails(&kind, Cost::Tokens(0))
            .expect_err("a byte bound cannot guard a run that spends tokens");
        let sentence = why.reason().map(ToString::to_string).unwrap_or_else(|| {
            panic!("a document's own key must be REFUSED with a sentence: {why:?}")
        });
        for named in [
            Cost::Bytes(0).bound_key(),
            Cost::Tokens(0).bound_key(),
            "max_iterations",
        ] {
            assert!(
                sentence.contains(named),
                "⚠⚠⚠ the refusal must name the offending key AND what the clause does take, \
                 because an author cannot act on a sentence that names neither — missing \
                 {named:?} in {sentence:?}",
            );
        }
    }

    /// ⚠⚠⚠⚠⚠ **A LAUNCH THAT NAMES NEITHER `agent` NOR `ready_when` IS STILL BEHIND A BARRIER, AND
    /// IT IS THE KIND DOCUMENT'S** — register item 738, layer 3.
    ///
    /// # ⚠⚠⚠⚠ What was being typed twice, measured
    ///
    /// Every launch of this repository's debt loop spelled `--agent claude --match settles --marker
    /// claude`. Those are **the same fact typed twice**: `AiLoopSpec::driving` derives
    /// `Settles(agent)` from the first, so the last two added nothing — and neither copy was
    /// written down anywhere a build reads. `ready_when` was `{ settles, claude }` on this
    /// repository's runs and on another repository's run 39 alike, which is what *invariant* means
    /// when it is measured rather than assumed.
    ///
    /// # ⚠⚠⚠ The three assertions, and why the order is the claim
    ///
    /// * **NEITHER KEY → THE DOCUMENT'S**, which is the layer this item added. The premise is
    ///   asserted inside the gate: the shared fixture supplies both keys, and against a request
    ///   that still holds them this whole test is vacuously true of a door that never consults the
    ///   kind at all.
    /// * **`agent` ALONE → DERIVED FROM IT**, unchanged. Item 300's line is what this must not
    ///   cross: a barrier is a predicate about the peer, read off which program is in the pane. A
    ///   document that won here would send a run driving `codex` to wait for `claude`.
    /// * **`ready_when` SPELLED → SPELLED**, because the most specific thing anybody said about
    ///   this pane is the caller's own.
    ///
    /// ⚠ What is NOT asserted is the marker's spelling. Which peer this repository drives is its
    /// document's business, and a string pinned here would be a second place it lives — so the
    /// claim is agreement with `LoopKind::ready_when`, plus that it is a `Settles`.
    #[test]
    fn a_run_that_names_no_agent_gets_the_kind_documents_own_barrier() {
        let script: Arc<dyn sce_rust_runtime::IScriptEngine> =
            Arc::new(sce_rust_lua::LuaEngine::new());
        let kind = sprag_plugin::kind::LoopKind::debt(Arc::clone(&script))
            .expect("this repository's kind document opens");
        let authored = kind
            .ready_when()
            .expect("its barrier must be readable")
            .expect(
                "⚠⚠⚠ THE CONTROL: a kind that authors no barrier makes every assertion below a \
                 statement about `None`, and the door would be refusing rather than falling through",
            );

        let mut declining = ai_loop_request(PaneId(1), json!({}));
        for key in ["agent", "ready_when"] {
            declining
                .as_object_mut()
                .expect("an object")
                .remove(key)
                .unwrap_or_else(|| panic!("the fixture supplies {key}, which this gate declines"));
        }
        let map = declining.as_object().expect("an object");
        for key in ["agent", "ready_when"] {
            assert!(
                !map.contains_key(key),
                "⚠⚠⚠⚠ THIS GATE IS VACUOUS IF THE REQUEST NAMES {key:?}: the question is what a \
                 launch that named NEITHER gets, and a fixture supplying one answers a different \
                 question with the same green",
            );
        }

        // ⛔⛔⛔⛔⛔ THE DOOR ITSELF FIRST, AND A MUTATION IS WHY. Register item 739 split the
        // reading of the document from the deciding of precedence, which is right — and it left
        // this gate measuring only the second half: **replacing the door's `kind_barrier(&kind)`
        // with `None` kept every assertion below GREEN**, because they hand the document's answer
        // in themselves. That is exactly the hole item 738's round found with N1 one function over,
        // arriving through the very split that fixed something else.
        //
        // A request naming NEITHER key is the observable that closes it: with the wiring intact the
        // kind supplies the barrier and the door BUILDS; with it cut there is nothing left to
        // supply one and the door refuses. No new accessor is needed — the refusal is the reading.
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        let mut on_the_pane = declining.clone();
        on_the_pane
            .as_object_mut()
            .expect("an object")
            .insert("pane".to_string(), json!(pane.0));
        external
            .build_plugin(on_the_pane.as_object().expect("an object"))
            .expect(
                "⛔⛔⛔⛔⛔ THE DOOR REFUSED A LAUNCH ITS OWN KIND DOCUMENT ANSWERS. Naming neither \
                 `agent` nor `ready_when` is the whole point of layer 3 — the document names the \
                 peer — so a refusal here means the door is not asking it",
            );
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );

        let resolved = ai_loop_barrier(map, kind_barrier(&kind).expect("its barrier is readable"))
            .expect(
                "⚠⚠⚠⚠⚠ ITEM 738: a launch that named neither key must reach the document's own",
            );
        assert_eq!(
            resolved, authored,
            "⚠⚠⚠⚠⚠ and it must be the KIND'S barrier rather than one this door invented: the \
             point of the layer is that the value has an author who also wrote what this peer says \
             when its service fails, not that some barrier turned up",
        );
        assert!(
            matches!(resolved, sprag_plugin::ReadyWhen::Settles(_)),
            "⚠⚠⚠ and `settles` is what an agent CLI can be waited for by — it asks the operating \
             system which program owns the pane's terminal, so no amount of the loop typing the \
             word can satisfy it. Read {resolved:?}",
        );

        // ⚠⚠⚠ CONTROL ONE: a caller who names the PROGRAM still has the barrier derived from it.
        // This is item 300's line, and a document that won here would send a run driving `codex`
        // to wait for a `claude` that is not in the pane.
        let named = ai_loop_request(PaneId(1), json!({ "agent": "codex", "ready_when": null }));
        assert_eq!(
            ai_loop_barrier(
                named.as_object().expect("an object"),
                kind_barrier(&kind).expect("its barrier is readable")
            )
            .expect("a caller naming a program resolves"),
            sprag_plugin::ReadyWhen::Settles("codex".to_string()),
            "⚠⚠⚠⚠ A CALLER SAYING WHICH PROGRAM IS IN THEIR PANE IS MORE SPECIFIC THAN A \
             DOCUMENT'S STANDING DEFAULT, and this key's whole meaning is that derivation",
        );

        // ⚠⚠ CONTROL TWO: and a caller who SPELLS the barrier wins over both.
        let spelled = ai_loop_request(
            PaneId(1),
            json!({ "agent": "codex", "ready_when": { "match": "shows", "marker": "READY" } }),
        );
        assert_eq!(
            ai_loop_barrier(
                spelled.as_object().expect("an object"),
                kind_barrier(&kind).expect("its barrier is readable")
            )
            .expect("a caller spelling a barrier resolves"),
            sprag_plugin::ReadyWhen::Shows("READY".to_string()),
            "⚠⚠⚠ the most specific thing anybody said about this pane is the caller's own words, \
             and a resolution that let the derivation or the document past them would silently \
             answer a question the caller had already answered",
        );
    }

    /// ⚠⚠⚠⚠ **A CALLER CAN DECLINE THE BUDGET AND LET THE DOCUMENT DECIDE** — item 312, PAID, and
    /// this gate is the one that measured the defect, turned around rather than deleted.
    ///
    /// # What it said the round before
    ///
    /// `max_turns` was `required` on `AI_LOOP_FORM`, so `ai_loop.scxml`'s own
    /// `<data id="max_turns" expr="40"/>` was unreachable from every caller there is: omitting the
    /// key was malformed rather than deferring. **A required judgement is a decision the document
    /// is structurally forbidden from making** — a harder case than item 300's two durations, which
    /// were already optional and so already meant *the document decides* when left out.
    ///
    /// ⚠⚠ The refusal also named nothing: `require_count` answered a bare
    /// [`InvokeError::TypeMismatch`], so somebody who declined the key learnt neither that it was
    /// mandatory nor that a 40 was waiting — while every neighbouring refusal here names the knob
    /// or the file. Both halves go together, because the key is declinable now.
    ///
    /// ⚠⚠⚠ **WHAT THIS DOOR CAN AND CANNOT SAY.** It answers *the call is accepted*; it cannot see
    /// the datamodel, so *the run is bounded by 40* is asserted where the document lives —
    /// `sprag_plugin`'s `a_declined_budget_is_the_documents_own`. Neither gate is the whole claim.
    #[test]
    fn a_caller_can_decline_max_turns_and_let_the_document_decide() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        // The well-formed request this whole module uses, with the one key taken back out — which
        // is the only way to ask the question, since the fixture supplies it like every caller.
        let mut declined = ai_loop_request(pane, json!({}));
        declined
            .as_object_mut()
            .expect("an object")
            .remove("max_turns")
            .expect("the fixture supplies the key this gate declines");

        external
            .invoke(RUN_ACTION, IntrospectValue::Json(declined))
            .expect(
                "⚠⚠⚠⚠ ITEM 312: declining `max_turns` must DEFER to the document rather than be \
                 malformed. This expectation is the inverse of the one that measured the defect, \
                 and it is deliberately the same call",
            );
        assert_eq!(
            lock(&registry).snapshot().len(),
            1,
            "⚠⚠ and a deferred budget starts a real run, not a nothing that reports success",
        );

        // ⚠⚠⚠ THE CONTROL, AND IT IS THE HALF THAT SURVIVED THE FIX UNCHANGED. Making the key
        // declinable must not stop a caller who names a number from being obeyed — and without
        // this, a product that ignored `max_turns` entirely would satisfy everything above.
        external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({ "max_turns": 3 }))),
            )
            .expect("a caller who names their own budget is obeyed");
        lock(&registry).cancel_all();
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
    }

    /// ⚠⚠⚠ **HALF OF THE TURN CONTRACT IS MALFORMED, IN BOTH DIRECTIONS.**
    ///
    /// `done_when` says what makes the peer's turn over; `turn_within_ms` says how long it may
    /// take. The two halves are NOT symmetric, and a conformance gate is what taught that:
    ///
    /// * **a bound with no contract** is REFUSED. It would quietly become *"wait this long and then
    ///   type at it anyway"*, which is the 500 ms timer the caller was plainly trying to get away
    ///   from, with a bigger number — doing MORE than asked, silently.
    /// * **a contract with no bound is a RUN**, and the first draft refused it.
    ///   `every_published_word_is_a_word_the_plugin_host_accepts` named that immediately: the wire
    ///   publishes `done_when`'s two words, so an agent that enumerates the vocabulary sends the
    ///   word ALONE and must be served rather than told its own call is malformed. **That gate has
    ///   now caught this same argument twice** — the first time was its companion at version 25.
    ///   Alone it means what it says: wait for the peer to finish, bounded by the run's own clock.
    ///
    /// # ⚠⚠ Why no per-argument harness could have caught the refused half
    ///
    /// [`a_handback_for_a_run_nobody_is_watching_is_malformed`]'s reason exactly, one contract
    /// over: the conformance sweeps drive ONE argument at a time — wrong type, declined, absent —
    /// and this request is well-typed, well-spelt, and wrong only in what it is missing.
    #[test]
    fn a_turn_contract_missing_half_of_itself_is_malformed() {
        let paired = json!({
            "done_when": "exits",
            sprag_plugin::Turn::WIRE_KEY: 12_000,
        });
        assert!(
            matches!(
                opt_turn(paired.as_object().expect("an object")),
                Ok(Some(_))
            ),
            "⚠ THE CONTROL FIRST: the pair these keys exist in is accepted, or the refusals below \
             are about a parser that refuses everything",
        );
        assert!(
            matches!(
                opt_turn(
                    json!({ "done_when": "exits" })
                        .as_object()
                        .expect("an object")
                ),
                Ok(Some(_)),
            ),
            "⚠⚠⚠ AND THE CONTRACT ALONE IS A RUN, not a refusal — this wire PUBLISHES the word, so \
             an agent that enumerated the vocabulary sends exactly this and must be served",
        );
        assert!(
            matches!(
                opt_turn(
                    json!({ sprag_plugin::Turn::WIRE_KEY: 12_000 })
                        .as_object()
                        .expect("an object")
                ),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠⚠ and a bound with no contract is REFUSED rather than read as a bigger timer, which \
             is the behaviour the caller was getting away from",
        );
        assert!(
            matches!(
                opt_turn(
                    json!({ "done_when": "exits", sprag_plugin::Turn::WIRE_KEY: 0 })
                        .as_object()
                        .expect("an object")
                ),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠ and a bound of ZERO is malformed — `await_person_ms`'s rule: *wait no time at all \
             for my peer to finish* is not a thing a caller can mean",
        );
        assert!(
            matches!(
                opt_turn(json!({}).as_object().expect("an object")),
                Ok(None)
            ),
            "⚠⚠⚠ AND THE DEFAULT IS SILENCE MEANING TODAY'S BEHAVIOUR: a caller who names neither \
             gets the step timeout their request has always got, or an added argument would have \
             changed what every existing call does",
        );
    }

    /// ⚠⚠⚠ **THE LOOP'S BOUND MOVED INTO ITS DOCUMENT AND THE WIRE'S ANSWERS DID NOT MOVE WITH
    /// IT** — the residue register item 300's move could have left, asked directly.
    ///
    /// Nothing on the `ai_loop` form builds a [`Turn`] any more: `done_when` binds a run to its
    /// peer and stays on the spec, `turn_within_ms` is a judgement and writes `<data>`. Three
    /// answers had to survive that, and only the first is obvious:
    ///
    /// * **A BOUND ALONE IS A RUN HERE**, where [`opt_turn`] refuses it. That asymmetry is older
    ///   than this round and its reason is the loop's default contract: an `agent` run defaults to
    ///   `exits`, so a bare bound bounds something nobody chose, and a loop defaults to
    ///   [`INNER_SESSION_ENDS`](sprag_plugin::INNER_SESSION_ENDS) — the contract its document makes
    ///   load-bearing — so a bare bound bounds exactly the turn the caller means.
    /// * **ZERO IS STILL REFUSED.** This is the one the move could have broken silently: the old
    ///   code handed the number to `Turn::lasting`, which refuses zero, and a `<data>` reads zero
    ///   as *the author declines a bound*. Had the number simply flowed through, a request the
    ///   wire REFUSED would have become a RUN — the direction R385 registered as earning a
    ///   protocol bump, arrived at by deleting a constructor rather than by deciding anything.
    /// * **AND SILENCE IS STILL SILENCE**, which is what lets the document decide.
    ///
    /// ⚠ It is the `ai_loop` form's own reader that is asked, because that is the only place the
    /// three answers are decided; [`opt_turn`] still serves the forms that build a `Turn`.
    #[test]
    fn a_loops_turn_bound_travels_to_its_document_without_changing_the_wires_answers() {
        let ms = |value: Value| {
            opt_ai_loop_turn_ms(
                json!({ sprag_plugin::Turn::WIRE_KEY: value })
                    .as_object()
                    .expect("an object"),
            )
        };
        assert!(
            matches!(ms(json!(12_000)), Ok(Some(12_000))),
            "⚠ THE CONTROL: a bound ALONE is a run on this form and reaches the document as the \
             number sent — no `done_when` beside it, which is `opt_turn`'s rule and not this one",
        );
        assert!(
            matches!(ms(json!(0)), Err(InvokeError::TypeMismatch)),
            "⚠⚠⚠ AND ZERO IS STILL REFUSED. In the document a zero means *no bound of my own*, so \
             a parser that let it through would turn a refusal into a run — silently, by having \
             stopped calling the constructor that owned the rule",
        );
        assert!(
            matches!(
                opt_ai_loop_turn_ms(json!({}).as_object().expect("an object")),
                Ok(None),
            ),
            "⚠⚠ and silence is silence: a caller who names no bound is not overriding the \
             document, which is the whole point of the move",
        );
        assert!(
            matches!(ms(json!(null)), Ok(None)),
            "⚠ and an explicitly declined key is the same as an absent one, which is what every \
             other optional argument on this surface does",
        );
    }

    /// ⛔⛔⛔ **A HOLD CEILING REACHES THE DOCUMENT, IS REFUSED AT ZERO, AND NEEDS NO PERSON BESIDE
    /// IT** — register item 534, on the door a caller actually calls.
    ///
    /// # ⚠⚠⚠⚠ The third assertion is the one the item is about
    ///
    /// The first two are this surface's ordinary rules restated. The third is the whole finding:
    /// `hold_within_ms` is **well-formed with no `await_person_ms` beside it**, which is the exact
    /// opposite of `handback_still_ms`'s rule in the gate below. Those two are one request about
    /// somebody EXPECTED, enforced by [`Handback`] living inside `Attended::APerson` — and a hold is
    /// an ORDER, which a run nobody is watching can be given. **That population is item 534's
    /// entire population**: the runs that parked for ever were the unattended ones, so a parser
    /// that demanded a watching person here would have refused the ceiling exactly where it was
    /// needed and left the defect standing behind a well-intentioned pairing rule.
    ///
    /// ⚠⚠ ZERO IS REFUSED for `await_person_ms`'s reason, sharpened: *hold this run and end it at
    /// once* is `cancel` spelled wrong, so accepting it would give a caller who reached zero by
    /// arithmetic a run that dies the first time anybody pauses it to read a pane.
    ///
    /// ⚠ AND SILENCE IS SILENCE, which is what lets `ai_loop.scxml` decide — the same answer every
    /// other optional duration on this form gives since register item 300.
    #[test]
    fn a_hold_ceiling_travels_alone_and_a_zero_one_is_refused() {
        let sent = |body: Value| opt_hold_within(body.as_object().expect("an object"));
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: 900_000 })),
                Ok(Some(within)) if within == Duration::from_millis(900_000),
            ),
            "⚠ THE CONTROL: a ceiling a caller sends must reach the document as the number sent, or \
             the key is decoration",
        );
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: 0 })),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠⚠ AND ZERO IS REFUSED. *Hold this run and end it at once* is `cancel` spelled wrong, \
             and a caller who arrived at zero by arithmetic must be told rather than handed a run \
             that dies the first time somebody pauses it",
        );
        // ⚠⚠⚠⚠ THE ITEM'S OWN ASSERTION: no person declared, and the request stands.
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: 60_000 })),
                Ok(Some(_)),
            ),
            "⛔⛔⛔ REGISTER ITEM 534: a hold ceiling sent WITHOUT `await_person_ms` must be \
             well-formed. A hold is an order and not a contract about who is watching — and the \
             runs that parked for ever were precisely the unattended ones, so pairing this key \
             with a person would refuse the ceiling in the only population that needed it",
        );
        assert!(
            matches!(sent(json!({})), Ok(None)),
            "⚠⚠ and silence is silence: a caller who names no ceiling defers to `ai_loop.scxml`'s \
             own, which is what every optional duration on this form has meant since item 300",
        );
        assert!(
            matches!(
                sent(json!({ sprag_plugin::HOLD_WITHIN_KEY: null })),
                Ok(None)
            ),
            "⚠ and an explicitly declined key is the same as an absent one",
        );
    }

    /// ⚠⚠⚠ **HALF OF A PAIRED REQUEST IS MALFORMED — `handback_still_ms` WITH NOBODY WATCHING.**
    ///
    /// A caller who sends it alone has plainly asked for a run that waits for a person. There is no
    /// `Attended` value that can carry their request ([`Handback`] lives inside `APerson`), and the
    /// two answers a daemon could give instead are both worse than a refusal: `NoOne` hands them a
    /// run that ENDS on the first keystroke — the opposite of what they sent, silently — and
    /// inventing a patience would be a bound nobody chose, on a run somebody may be waiting on.
    ///
    /// # ⚠⚠ Why no per-argument harness could have caught this
    ///
    /// The three conformance sweeps this surface runs drive ONE argument at a time: at the wrong
    /// type, declined as `null`, absent. This rule is about a PAIR — well-typed, well-spelt, and
    /// wrong only in what it is missing — so it is the shape those sweeps are blind to by
    /// construction, and it needs a gate of its own.
    #[test]
    fn a_handback_for_a_run_nobody_is_watching_is_malformed() {
        let paired = json!({
            sprag_plugin::Attended::WIRE_KEY: 20_000,
            sprag_plugin::Handback::WIRE_KEY: 400,
        });
        assert!(
            matches!(
                opt_attended(paired.as_object().expect("an object")),
                Ok(Attended::APerson { .. }),
            ),
            "⚠ THE CONTROL FIRST: the pair this key exists in is accepted, or the refusal below is \
             about a parser that refuses everything",
        );
        let alone = json!({ sprag_plugin::Handback::WIRE_KEY: 400 });
        assert!(
            matches!(
                opt_attended(alone.as_object().expect("an object")),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠⚠ and the half-request is REFUSED rather than quietly answered `NoOne`, which would \
             give the caller a run that ends on the first keystroke while their call asked the \
             daemon to wait",
        );
        let zero = json!({
            sprag_plugin::Attended::WIRE_KEY: 20_000,
            sprag_plugin::Handback::WIRE_KEY: 0,
        });
        assert!(
            matches!(
                opt_attended(zero.as_object().expect("an object")),
                Err(InvokeError::TypeMismatch),
            ),
            "⚠⚠ and a stillness of ZERO is malformed too — `await_person_ms`'s own rule, for its \
             reason: every person pauses between keystrokes, so a run given zero would type into \
             the gap between their words",
        );
        assert!(
            matches!(
                opt_attended(json!({}).as_object().expect("an object")),
                Ok(Attended::NoOne),
            ),
            "⚠ and neither key is still `NoOne`, which is what every run did before either existed",
        );
    }

    /// No settle window at all — the injected policy this path takes as a parameter, so a test of a
    /// TIMED transition is not asserting about a timing the developer's `config.toml` chose.
    fn instant_window() -> sprag_detect::Hysteresis {
        sprag_detect::Hysteresis {
            settle: Duration::ZERO,
        }
    }

    /// A REAL pane whose child paints `bytes` and then holds its pty open.
    ///
    /// A live PTY and the live emulator, not a synthetic screen: the subject here is the whole path
    /// from a child's output to what a plugin is told, and the two ends of it are exactly what a
    /// hand-built `Screen` would skip.
    fn pane_painting(bytes: &str) -> (Arc<Mutex<Workspace>>, PaneId) {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%b' '{bytes}'; exec cat"));
        command.env("TERM", "xterm-256color");
        // ⚠ POINTED AT A TREE — see `a_tree_to_stand_in`. A pane a LOOP may be built over must be
        // standing somewhere its agent would not be asked to trust the folder (item 738, layer 4),
        // and the conformance probe drives real `ai_loop` requests against this world.
        command.cwd(a_tree_to_stand_in());
        let id = lock(&workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the pane");
        (workspace, id)
    }

    /// The `claude` permission dialog R249 captured, as the bytes a child would print: the OSC
    /// title first (the IDLE glyph, which is what makes this a test about arbitration), then the
    /// dialog.
    const PERMISSION_SCREEN: &str = "\\033]0;\\342\\234\\263 Remove temporary directory\\007\
         \\r\\n Do you want to allow Claude to fetch this content?\
         \\r\\n \\342\\235\\257 1. Yes\
         \\r\\n   2. Yes, and don'\\''t ask again for example.com\
         \\r\\n   3. No, and tell Claude what to do differently (esc)";

    /// A `claude` at rest, and the same pane a moment later with the braille spinner in its title —
    /// the two screens a turn passes between, in the bytes a child prints.
    ///
    /// The MARKER on each is what a test waits for without observing: the title is not on the
    /// screen, so a fixture that waited for the title would have to ask the detector, and asking the
    /// detector is the sampling these tests are about.
    const CLAUDE_AT_REST: &str =
        "\\033]2;\\342\\234\\263 Claude Code\\007\\033[2J\\033[Hat rest %s\\r\\n";
    const CLAUDE_WORKING: &str =
        "\\033]2;\\342\\240\\213 Claude Code\\007\\033[2J\\033[Hworking\\r\\n";

    /// A pane that paints ON COMMAND: it announces `GO`, then paints the next of `screens` for each
    /// Enter it is sent.
    ///
    /// **It says when its terminal is ready** (R347): a `sh -c` peer takes milliseconds to reach its
    /// `stty`, and a test that injected before then would have its Enter echoed back into the pane.
    ///
    /// **Its line discipline stays CANONICAL**, unlike R347's peer, and that is deliberate: `read`
    /// wants a line, every act here is an Enter, and canonical mode is what turns the carriage
    /// return a keystroke encodes into the newline the shell is waiting for. Echo is off, so the
    /// keystroke itself paints nothing and the screen holds only what the script printed.
    ///
    /// The point of a pane that paints on command rather than on a timer: a turn's boundaries become
    /// the TEST's to place, so "a turn that began and ended between two looks" is an assertion and
    /// not a race.
    fn pane_painting_in_turn(screens: &[String]) -> (Arc<Mutex<Workspace>>, PaneId) {
        let mut script = String::from("stty -echo; printf 'GO\\r\\n'");
        for screen in screens {
            script.push_str(&format!("; read -r _; printf '%b' '{screen}'"));
        }
        script.push_str("; exec cat");
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "xterm-256color");
        let id = lock(&workspace)
            .spawn(command, "agent".to_string(), 80, 24)
            .expect("spawn the pane");
        (workspace, id)
    }

    /// Wait for `needle` on the pane's screen WITHOUT asking the detector anything.
    ///
    /// The distinction this whole pair of tests rests on: [`settle`] polls the supervision source,
    /// and every such poll is a look at the screen. A test about what a look MISSES cannot wait by
    /// looking.
    fn wait_for_screen(access: &WorkspacePaneAccess, id: PaneId, needle: &str) {
        let start = Instant::now();
        let mut last = String::new();
        while start.elapsed() < Duration::from_secs(10) {
            last = access.pane_collapsed(id).unwrap_or_default();
            if last.contains(needle) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the pane never painted {needle:?}; its screen was {last:?}");
    }

    /// Send one Enter to the pane — the act that advances [`pane_painting_in_turn`] to its next
    /// screen.
    fn advance(access: &WorkspacePaneAccess, id: PaneId) {
        let written = access
            .inject(id, &[sprag_plugin::KeyStroke::named("Enter")])
            .expect("the pane takes a keystroke");
        assert!(
            written.bytes() > 0,
            "an Enter that wrote nothing would advance nothing",
        );
    }

    /// Poll the source until `ready`, or give up — the pane's child has to run and its bytes have to
    /// reach the emulator, and neither is synchronous.
    fn settle(
        source: &sprag_plugin::AgentStateSource,
        id: PaneId,
        ready: impl Fn(&sprag_plugin::AgentObservation) -> bool,
    ) -> sprag_plugin::AgentObservation {
        let start = Instant::now();
        let mut last = None;
        while start.elapsed() < Duration::from_secs(10) {
            if let Some(seen) = source(id) {
                if ready(&seen) {
                    return seen;
                }
                last = Some(seen);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the pane never reached the state this test is about; last: {last:?}");
    }

    fn source(
        workspace: &Arc<Mutex<Workspace>>,
        agents: &Arc<crate::AgentClock>,
    ) -> sprag_plugin::AgentStateSource {
        agent_state_source(Arc::clone(workspace), Arc::clone(agents), instant_window)
    }

    /// An outcome with `state`, and nothing else that matters here.
    fn finished(state: OutcomeState, answered: u32) -> Outcome {
        Outcome {
            state,
            iterations: 1,
            cost: None,
            failure: None,
            stopped: None,
            answered,
            screened: 0,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            // ⚠ `None` and not a zero: this fixture is not a run that counted nothing, it is one
            // that does not count — the distinction `Banked` exists to keep.
            banked: None,
            // ⚠ `None` on `banked`'s terms: not a run briefed with nothing, one nobody briefs.
            briefed: None,
        }
    }

    /// The measured shape of a real permission dialog, as a run's outcome carries it.
    fn a_question() -> sprag_detect::Question {
        sprag_detect::Question {
            asked: vec!["Do you want to proceed?".to_owned()],
            choices: vec![
                sprag_detect::Choice {
                    number: 1,
                    label: "Yes".to_owned(),
                    selected: true,
                },
                sprag_detect::Choice {
                    number: 2,
                    label: "No".to_owned(),
                    selected: false,
                },
            ],
        }
    }

    /// ⚠⚠⚠ **A BLOCKED RUN ALWAYS SAYS WHY IT DID NOT ANSWER, EVEN WHEN IT HAS NO QUESTION.**
    ///
    /// Two runs that stop on the same dialog look identical to a client unless the reason travels:
    /// one was given no consent (fix: write one) and one was given a consent that named nothing on
    /// offer (fix: the needle). Those are different actions, and until R366 the answer carried
    /// neither.
    ///
    /// ⚠ The `unreadable` half is the one with NO question at all — a pane blocked on something
    /// this host cannot parse as a menu. It published as an ABSENCE and explained nowhere; the
    /// remedy (a person) lived in a doc comment. Here the key is present and the word says so.
    #[test]
    fn a_blocked_run_publishes_why_it_did_not_answer_with_or_without_the_question() {
        let refused = finished(
            OutcomeState::Blocked(Some(sprag_plugin::Unanswered::refused(
                a_question(),
                sprag_plugin::Refusal::NotOffered,
            ))),
            0,
        );
        let answer = outcome_to_json(&refused);
        assert_eq!(answer["state"], "blocked");
        let asking = &answer[RUN_ASKING_KEY];
        assert_eq!(asking[RUN_WHY_KEY], "not_offered");
        assert_eq!(asking[RUN_ASKED_KEY][0], "Do you want to proceed?");
        assert_eq!(
            asking[RUN_CHOICES_KEY][0]["selected"], true,
            "and where a bare Enter would land, which is what a person answering it needs",
        );

        // ⚠ NO QUESTION, and the key that says so is the one that is never absent.
        let unreadable = finished(
            OutcomeState::Blocked(Some(sprag_plugin::Unanswered::unreadable())),
            0,
        );
        let answer = outcome_to_json(&unreadable);
        assert_eq!(answer[RUN_ASKING_KEY][RUN_WHY_KEY], "unreadable");
        assert!(
            answer[RUN_ASKING_KEY].get(RUN_ASKED_KEY).is_none(),
            "the question is ABSENT rather than empty — a caller tells `this host could not read \
             it` from `it had no lines` by the key's presence: {answer}",
        );
        assert!(
            sprag_plugin::Refusal::parse(
                answer[RUN_ASKING_KEY][RUN_WHY_KEY]
                    .as_str()
                    .expect("a word"),
            )
            .is_some(),
            "and every word published here is one the type spells, never a literal: {answer}",
        );
    }

    /// ⚠⚠ **EVERY OUTCOME SAYS HOW MANY DECISIONS THE RUN TOOK ON SOMEBODY'S BEHALF** — including
    /// `0`, and including the runs that did not end well.
    ///
    /// The key's neighbours (`ceiling`, `stopped`) are absent when they have nothing to say,
    /// because their absence means *nothing of this kind happened* and costs a reader nothing.
    /// This one is a count of APPROVALS, so *"this run answered nothing"* has to be readable as a
    /// claim rather than inferred from a key nobody wrote — and a run that answered a dialog and
    /// then hit its iteration ceiling has to report both.
    #[test]
    fn every_outcome_says_how_many_of_its_peers_questions_it_answered() {
        for state in [
            OutcomeState::Converged,
            OutcomeState::Cancelled,
            OutcomeState::Failed,
            OutcomeState::Exhausted(Ceiling::Iterations),
            OutcomeState::Blocked(Some(sprag_plugin::Unanswered::unreadable())),
        ] {
            let quiet = outcome_to_json(&finished(state.clone(), 0));
            assert_eq!(
                quiet[RUN_ANSWERED_KEY], 0,
                "⚠ PRESENT and zero, not absent — see `RUN_ANSWERED_KEY`: {quiet}",
            );
            let spoke = outcome_to_json(&finished(state, 3));
            assert_eq!(
                spoke[RUN_ANSWERED_KEY], 3,
                "and a run that answered says so whatever became of it afterwards: {spoke}",
            );
        }
    }

    /// ⚠⚠ **THE CONSENT IS READ THROUGH THE TYPE, so what this surface accepts and what the type
    /// admits are one predicate.**
    ///
    /// The two needles are open strings on the wire, which makes the EMPTY one the whole risk: an
    /// empty `asked` is carried by every question and an empty `answer` by every option, so either
    /// turns a narrow consent into something the caller did not write. `Consent::parse` owns that
    /// refusal and this holds the parser to it — R352's shape, where a `String` argument admits
    /// fewer values than its type.
    ///
    /// ⚠ And an absent key is a run that answers NOTHING, which is the default the whole feature
    /// rests on.
    ///
    /// # ⚠⚠⚠ The shape is a LIST, and the two ways that can go wrong are BOTH malformed
    ///
    /// An EMPTY list, because `[]` and an absent key would otherwise be two spellings of *"answer
    /// nothing"* — and the one that arrives by accident (a client whose clause list came from a
    /// filter that matched nothing) is exactly the caller who wants telling. And the PRE-BUMP
    /// OBJECT, which is what a version-28 client sends a version-29 daemon: it must meet the
    /// grammar at the door rather than be read as a one-clause list, because a shape this wire
    /// quietly reinterprets is one no version number can protect.
    #[test]
    fn the_consent_this_surface_reads_is_the_one_the_type_admits() {
        let asked = "Do you want to proceed?";
        let clause = |asked: &str, answer: &str| json!({ Consent::ASKED_KEY: asked, Consent::ANSWER_KEY: answer });
        let good = json!({ Consents::WIRE_KEY: [clause(asked, "Yes")] });
        assert_eq!(
            opt_may_answer(good.as_object().expect("an object")).expect("a well-formed consent"),
            Consents::of(vec![
                Consent::parse(asked.to_owned(), "Yes".to_owned()).expect("two needles"),
            ]),
            "the surface builds exactly what the type would",
        );

        let many = json!({
            Consents::WIRE_KEY: [clause(asked, "Yes"), clause("make this edit", "Yes")],
        });
        assert_eq!(
            opt_may_answer(many.as_object().expect("an object")).expect("two well-formed clauses"),
            Consents::of(vec![
                Consent::parse(asked.to_owned(), "Yes".to_owned()).expect("two needles"),
                Consent::parse("make this edit".to_owned(), "Yes".to_owned()).expect("two needles"),
            ]),
            "⚠⚠⚠ EVERY clause arrives, and IN THE CALLER'S ORDER — a parser that kept only the \
             first would leave an unattended run stopping at the second question of every turn, \
             which is the defect the list exists to close. Compared WHOLE rather than by count, so \
             a parser that read two clauses and built them from one object fails here too",
        );

        for (label, sent) in [
            ("absent", json!({})),
            (
                "declined as null",
                json!({ Consents::WIRE_KEY: Value::Null }),
            ),
        ] {
            assert_eq!(
                opt_may_answer(sent.as_object().expect("an object")).expect("well-formed"),
                None,
                "⚠⚠ {label} is a run that may answer NOTHING — the default every run had before \
                 this key existed, and the reason answering is opt-in",
            );
        }

        for (label, sent) in [
            (
                "an empty question needle",
                json!({ Consents::WIRE_KEY: [clause("", "Yes")] }),
            ),
            (
                "an empty option needle",
                json!({ Consents::WIRE_KEY: [clause(asked, "")] }),
            ),
            (
                "no option needle at all",
                json!({ Consents::WIRE_KEY: [{ Consent::ASKED_KEY: asked }] }),
            ),
            (
                "a bare string where the list goes",
                json!({ Consents::WIRE_KEY: "Yes" }),
            ),
            (
                "an EMPTY list, which is not a second spelling of the default",
                json!({ Consents::WIRE_KEY: [] }),
            ),
            (
                "a bare string INSIDE the list",
                json!({ Consents::WIRE_KEY: ["Yes"] }),
            ),
            (
                "⚠⚠⚠ the PRE-BUMP object a version-28 client sends",
                json!({ Consents::WIRE_KEY: clause(asked, "Yes") }),
            ),
            (
                "one good clause beside a malformed one",
                json!({ Consents::WIRE_KEY: [clause(asked, "Yes"), clause("", "Yes")] }),
            ),
        ] {
            assert!(
                matches!(
                    opt_may_answer(sent.as_object().expect("an object")),
                    Err(InvokeError::TypeMismatch),
                ),
                "⚠⚠⚠ {label} is a MALFORMED request and must meet the grammar at the door — \
                 accepting it would authorise an answer to a question the caller never named",
            );
        }
    }

    /// A plugin reads what the agent in its pane is DOING, and what it is blocked ON — through the
    /// extension API, off a live pane, with no second detector anywhere.
    ///
    /// This is the whole of the supervision requirement in one assertion. Before it, a plugin's
    /// view of a blocked agent was the pane's text: it could see the dialog and had to re-derive
    /// what the daemon had already decided, and every plugin author would have re-derived it
    /// differently.
    ///
    /// The title is the IDLE glyph, deliberately — that is what a real blocked `claude` shows
    /// (R249's measurement, and the reason `Rule::priority` exists), so a surface that read the
    /// title alone would report this pane at rest while it waits for a person.
    #[test]
    fn a_plugin_reads_a_blocked_agents_state_and_the_question_it_is_blocked_on() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);

        let seen = settle(&read, id, |o| o.state == AgentState::Blocked);
        assert_eq!(seen.agent.as_deref(), Some("claude"));
        assert_eq!(
            seen.authority,
            sprag_plugin::Authority::Scraped {
                rule: Some("dialog-choice-list".to_owned()),
            },
            "a screen-read verdict must say so, and say which rule said it",
        );
        assert!(
            !seen.authority.is_exact(),
            "a scrape is a sample of an animation, and a supervisor must be able to know that",
        );

        let asking = seen.asking.as_ref().expect("the question it is blocked on");
        assert_eq!(
            asking
                .choices
                .iter()
                .map(|c| (c.number, c.label.as_str()))
                .collect::<Vec<_>>(),
            vec![
                (1, "Yes"),
                (2, "Yes, and don't ask again for example.com"),
                (3, "No, and tell Claude what to do differently (esc)"),
            ],
        );
        assert_eq!(asking.selected().map(|c| c.number), Some(1));
        assert!(
            asking
                .asked
                .iter()
                .any(|line| line.contains("allow Claude to fetch")),
            "the sentence a policy classifies: {:?}",
            asking.asked,
        );
        assert!(asking.choice(4).is_none(), "a number nobody offered");

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// The two authorities, on ONE pane, told apart by the type.
    ///
    /// The same screen answers `blocked` by SCRAPING it and `working` because the process inside
    /// said so — and a report outranks the screen, which is exactly why a consumer must be able to
    /// see which one it has. A supervisor treating a scrape as a turn boundary is treating a sample
    /// of a spinner as an event.
    #[test]
    fn a_report_from_inside_the_pane_is_marked_exact_and_a_scrape_is_not() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);

        let scraped = settle(&read, id, |o| o.state == AgentState::Blocked);
        assert!(!scraped.authority.is_exact());

        let (outcome, _) = agents.report(
            id,
            Report {
                state: AgentState::Working,
                agent: Some("claude".to_owned()),
                source: "claude-hook".to_owned(),
                seq: Some(1),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            instant_window,
        );
        assert!(outcome.accepted, "the hook's report must be taken");

        let reported = read(id).expect("the pane is still an agent's");
        assert_eq!(reported.state, AgentState::Working);
        assert_eq!(
            reported.authority,
            sprag_plugin::Authority::Reported {
                source: "claude-hook".to_owned(),
            },
        );
        assert!(
            reported.authority.is_exact(),
            "the process inside the pane said so; nothing was sampled",
        );
        assert!(
            reported.asking.is_none(),
            "a working pane is not waiting on the menu still painted behind it",
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// A pane the AGENT ITSELF reported blocked still carries the question off its screen.
    ///
    /// The branch the exact path most needs and the one a design could easily lose: a report is the
    /// authority on the STATE and says nothing about the menu, because the hook fires on an event
    /// and the options are pixels. If the question were tied to the verdict's provenance, the
    /// accurate path would be the blind one — the supervisor would know a person is needed and not
    /// what for, exactly when it has the best information it will ever have.
    #[test]
    fn a_pane_its_own_agent_reported_blocked_still_carries_the_question() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        settle(&read, id, |o| o.state == AgentState::Blocked);

        let (outcome, _) = agents.report(
            id,
            Report {
                state: AgentState::Blocked,
                agent: Some("claude".to_owned()),
                source: "claude-hook".to_owned(),
                seq: Some(1),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                transcript: None,
                build: None,
            },
            instant_window,
        );
        assert!(outcome.accepted);

        let seen = read(id).expect("an agent");
        assert!(
            seen.authority.is_exact(),
            "the state came from inside the pane",
        );
        let asking = seen
            .asking
            .as_ref()
            .expect("...and the question still came from the screen");
        assert_eq!(asking.choices.len(), 3);
        assert_eq!(asking.selected().map(|c| c.number), Some(1));

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// A turn that begins and ends BETWEEN two pulls is still visible — which is the whole reason
    /// this surface is a level and not an event stream.
    ///
    /// The measurement this answers was taken against a rival that publishes agent state as change
    /// EVENTS: a one-second turn produced no event at all, and the supervising machine waited
    /// forever for a turn that had already finished. Here the second pull reads `idle` — the same
    /// value the first one did, so the STATE really is no help — and `seq` says two changes
    /// happened in between. Nothing was lost; it was carried as a level.
    #[test]
    fn a_turn_that_starts_and_ends_between_two_pulls_is_not_lost() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        settle(&read, id, |o| o.state == AgentState::Blocked);

        // The pull a supervisor takes before the turn.
        let hook = |state: AgentState, seq: u64| Report {
            state,
            agent: Some("claude".to_owned()),
            source: "claude-hook".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };
        agents.report(id, hook(AgentState::Idle, 1), instant_window);
        let before = read(id).expect("an agent");
        assert_eq!(before.state, AgentState::Idle);

        // A whole turn, entirely between the two pulls: the agent starts and finishes.
        agents.report(id, hook(AgentState::Working, 2), instant_window);
        agents.report(id, hook(AgentState::Idle, 3), instant_window);

        let after = read(id).expect("an agent");
        assert_eq!(
            after.state, before.state,
            "the STATE is the same at both pulls, so it cannot be what tells them apart",
        );
        assert!(
            after.seq > before.seq,
            "a turn happened between the pulls and the level must carry that: {} -> {}",
            before.seq,
            after.seq,
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// THE PREMISE, MEASURED: a turn the pane really performed, between two looks, leaves the scrape
    /// with nothing to report — the same answer it gives for a pane that never worked at all.
    ///
    /// This is the control the SCE requirement's §5 rests on, and it is here because the requirement
    /// arrived as a claim about somebody else's observer (*"a one-second turn produced no observable
    /// working state at all"*) and a project rule says a handed-over premise is measured before it is
    /// built for. Measured here against sprag's own detector, driving the screens a real `claude`
    /// paints, it reproduces — and the mechanism is sharper than "the sample rate is too low":
    ///
    /// **A scrape's evidence is DESTROYED by the next paint.** The working state lives in the pane's
    /// TITLE, a terminal holds one, and the agent overwrites it the instant the turn ends. So it is
    /// not that a look is unlikely to land inside a short turn; it is that after the turn there is
    /// nothing left for any number of looks to find. No poll interval closes this, which is why the
    /// answer is the agent reporting rather than sprag sampling harder — see the twin below.
    ///
    /// The turn here is not even short: it lasts as long as this test takes to paint it. What makes
    /// it invisible is only that nobody looked DURING it, which is the case a supervisor cannot
    /// prevent and cannot detect.
    #[test]
    fn a_turn_the_scrape_did_not_look_during_leaves_no_trace_of_having_happened() {
        let (workspace, id) = pane_painting_in_turn(&[
            CLAUDE_AT_REST.replace("%s", "one"),
            CLAUDE_WORKING.to_owned(),
            CLAUDE_AT_REST.replace("%s", "two"),
        ]);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));

        wait_for_screen(&access, id, "GO");
        advance(&access, id);
        wait_for_screen(&access, id, "at rest one");

        // The pull a supervisor takes before the turn.
        let before = settle(&read, id, |o| o.state == AgentState::Idle);
        assert!(!before.authority.is_exact(), "this pane reports nothing");

        // The whole turn, and nobody looks: the agent starts working...
        advance(&access, id);
        wait_for_screen(&access, id, "working");
        // ...and finishes.
        advance(&access, id);
        wait_for_screen(&access, id, "at rest two");

        // The pull a supervisor takes after it.
        let after = read(id).expect("the pane is still an agent's");
        assert_eq!(
            after.state,
            AgentState::Idle,
            "the pane is at rest, which is true and is not the question",
        );
        assert_eq!(
            after.seq, before.seq,
            "the turn happened and the scrape can say nothing about it: {} -> {}",
            before.seq, after.seq,
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// THE ANSWER, on the same pane painting the same turn: the agent's own hook reports each
    /// boundary, and the turn is there to be read afterwards.
    ///
    /// The twin of the test above, differing in exactly one thing — whether the agent said anything —
    /// so what it proves is attributable. Both pulls read `idle`, exactly as before; the difference
    /// is entirely in `seq`, which moved by the two changes the turn is made of.
    ///
    /// This is what `--settings` buys ([`crate::pane_args_source`]): the report is made AT the
    /// boundary by the process that alone knows where the boundary is, so it does not depend on
    /// anybody looking, and it survives the next paint.
    #[test]
    fn the_same_turn_reported_by_the_agent_is_still_there_to_be_read() {
        let (workspace, id) = pane_painting_in_turn(&[
            CLAUDE_AT_REST.replace("%s", "one"),
            CLAUDE_WORKING.to_owned(),
            CLAUDE_AT_REST.replace("%s", "two"),
        ]);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let hook = |state: AgentState, seq: u64| Report {
            state,
            agent: Some("claude".to_owned()),
            source: "claude-hook".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };

        wait_for_screen(&access, id, "GO");
        advance(&access, id);
        wait_for_screen(&access, id, "at rest one");
        agents.report(id, hook(AgentState::Idle, 1), instant_window);
        let before = read(id).expect("an agent");
        assert_eq!(before.state, AgentState::Idle);

        // The same turn, with nobody looking — and the agent saying so at each edge.
        advance(&access, id);
        agents.report(id, hook(AgentState::Working, 2), instant_window);
        wait_for_screen(&access, id, "working");
        advance(&access, id);
        agents.report(id, hook(AgentState::Idle, 3), instant_window);
        wait_for_screen(&access, id, "at rest two");

        let after = read(id).expect("an agent");
        assert_eq!(
            after.state, before.state,
            "the STATE is the same at both pulls, exactly as in the scraped twin",
        );
        assert_eq!(
            after.seq,
            before.seq + 2,
            "and the two edges of the turn are still there: {} -> {}",
            before.seq,
            after.seq,
        );
        assert!(
            after.authority.is_exact(),
            "an answer that came from inside the pane: {:?}",
            after.authority,
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// ⚠⚠⚠⚠⚠ **A SUPERVISOR IS TOLD HOW MANY TIMES A PANE HAS BEEN SPOKEN FOR** — register item
    /// 458, at the seam where the count crosses from the tracker into the surface a plugin reads.
    ///
    /// # ⚠⚠⚠⚠ Why this gate is in THIS crate and could not be written in `sprag-plugin`
    ///
    /// The silence ceiling is decided in the plugin, and every gate there builds an
    /// `AgentObservation` — so all of them would go on passing if this line stopped carrying the
    /// number. **A fixture that supplies the very field the product omits cannot see the omission**;
    /// that is item 428's shape, and item 459 is a live example of exactly it one crate over. So
    /// the assertion belongs where the ADAPTER is, driven through
    /// [`AgentClock::report`](crate::AgentClock::report) — the door a hook's payload actually
    /// arrives by.
    ///
    /// # ⚠⚠⚠ What makes it the right number rather than any number
    ///
    /// The two reports below say the SAME THING TWICE — `working`, then `working` — which is
    /// precisely a turn calling tool after tool. So `seq` cannot move, and if it did this would be
    /// measuring the wrong counter. What must move is this one, because it counts REPORTS and not
    /// verdicts, and it is the only sign of life a turn like that leaves.
    #[test]
    fn a_supervisor_is_told_how_many_times_a_pane_has_been_spoken_for() {
        let (workspace, id) = pane_painting_in_turn(&[
            CLAUDE_AT_REST.replace("%s", "one"),
            CLAUDE_WORKING.to_owned(),
        ]);
        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let read = source(&workspace, &agents);
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let hook = |seq: u64| Report {
            state: AgentState::Working,
            agent: Some("claude".to_owned()),
            source: "claude-hook".to_owned(),
            seq: Some(seq),
            owner: None,
            asked: None,
            said: None,
            noticed: None,
            transcript: None,
            build: None,
        };

        wait_for_screen(&access, id, "GO");
        advance(&access, id);
        wait_for_screen(&access, id, "at rest one");

        // ── THE CONTROL FIRST: a pane nothing has reported for ──
        //
        // ⚠⚠⚠⚠⚠ ITS ANSWER IS ZERO AND ALWAYS WILL BE, and that is why a caller may not read zero
        // as silence: it means *this pane has no reporter to be silent*, not *nobody is speaking*.
        // Without this reading, `reports` moving below could be a counter that starts anywhere.
        let scraped = read(id).expect("the pane is an agent's from its screen alone");
        assert_eq!(
            (scraped.reports, scraped.authority.is_exact()),
            (0, false),
            "a pane read from its SCREEN has been spoken for by nobody, and says so through its \
             authority as well as its count: {scraped:?}",
        );

        agents.report(id, hook(1), instant_window);
        let first = read(id).expect("an agent");
        // The same turn, still working, still calling tools — one more report saying nothing new.
        agents.report(id, hook(2), instant_window);
        let second = read(id).expect("an agent");

        assert_eq!(
            (second.seq, second.state),
            (first.seq, first.state),
            "⚠⚠⚠ THE FIXTURE: these two reports must publish NOTHING, or this gate is about `seq` \
             after all. A turn calling tool after tool reports `working` every time and the \
             verdict never moves — which is the whole reason a fourth counter had to exist",
        );
        assert_eq!(
            (first.asked_seq, first.said_seq),
            (second.asked_seq, second.said_seq),
            "and neither counter of STATEMENTS moves either: a turn in flight has stated no \
             question and no answer, so all three stand still together",
        );
        assert_eq!(
            (first.reports, second.reports),
            (1, 2),
            "⚠⚠⚠⚠⚠ AND THIS ONE MOVES, EVERY REPORT, WHATEVER IT SAID. It is the only thing left \
             that separates a peer working slowly from a peer that has stopped speaking, and a \
             supervisor that never receives it is back where the fourteen measured minutes were: \
             `working seq=6 asked=2 said=0`, indistinguishable from a turn nothing will ever end. \
             Got {first:?} then {second:?}",
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// A host with no detector says it cannot supervise, and that is a DIFFERENT answer from a pane
    /// that is not an agent's.
    ///
    /// Collapsing the two would let a supervisor conclude "no agents here" from a build that never
    /// looked — the same class of confident wrong answer `Landing::Unplaced` and
    /// `AgentState::Unknown` are each shaped to avoid.
    #[test]
    fn a_host_with_no_detector_says_so_rather_than_reporting_no_agents() {
        let (workspace, id) = pane_painting(PERMISSION_SCREEN);

        let blind = WorkspacePaneAccess::new(Arc::clone(&workspace));
        assert!(
            blind.supervision().is_none(),
            "a host with no detector must not answer questions about agents at all",
        );

        let agents = Arc::new(crate::AgentClock::new(Ruleset::new(built_ins())));
        let seeing = WorkspacePaneAccess::new(Arc::clone(&workspace))
            .with_agent_state(Some(source(&workspace, &agents)));
        let supervision = seeing
            .supervision()
            .expect("a host WITH a detector supervises");
        // ...and on that host, a pane no manifest claims is the other answer: `None` for this pane,
        // from a surface that exists.
        assert!(
            supervision.pane_agent_state(PaneId(9999)) == sprag_plugin::Supervised::NotAnAgent,
            "a pane nobody knows is not an agent",
        );

        let closed = lock(&workspace).close(id);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }

    /// **REQ §5, the door**: a pane a PLUGIN spawns is told which pane it is and where the daemon
    /// listens — so the agent inside it can report its own turn boundaries instead of being guessed
    /// at from its screen.
    ///
    /// The exact/approximate split is only worth having if the EXACT half is reachable, and the
    /// exact half is a hook inside the agent's own process calling back: it needs the pane's id and
    /// the daemon's address, both published into the child's environment at birth. Every other pane
    /// gets them because the mux surface spawns it. A plugin's pane goes through a different door,
    /// and R337 is this project's record of what that costs — "two doors" onto pane birth turned out
    /// to be FIVE, and the one this layer owns carried a comment claiming the host filled something
    /// in that the host did not.
    ///
    /// So it is asserted rather than trusted to the structure. What the child prints is what the
    /// child was given; the reporting half on the other end of that address is `hooks.rs`'s and is
    /// tested there.
    #[test]
    fn a_pane_a_plugin_spawns_is_told_which_pane_it_is_and_where_to_report() {
        let socket = std::path::Path::new("/tmp/sprag-plugin-door.probe");
        let workspace = Arc::new(Mutex::new(Workspace::new((60, 6))));
        lock(&workspace).set_pane_env_source(crate::pane_env_source(socket));

        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let address = sprag_rpc::HOST_SOCKET.path_env;
        let pane = access
            .lifecycle()
            .expect("the plugin surface spawns panes")
            .spawn(
                &[
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    format!(
                        "printf 'PANE=%s AT=%s' \"${{{}-unset}}\" \"${{{address}-unset}}\"; exec cat",
                        crate::PANE_ENV_VAR,
                    ),
                ],
                60,
                6,
            )
            .expect("spawn");

        let want = format!("PANE={} AT={}", pane.0, socket.display());
        let start = Instant::now();
        let mut seen = String::new();
        while start.elapsed() < Duration::from_secs(10) {
            seen = access.pane_collapsed(pane).unwrap_or_default();
            if seen.contains(&want) {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            seen.contains(&want),
            "a plugin-spawned pane's child must know its own pane and the daemon's address; \
             wanted {want:?}, screen was {seen:?}",
        );
        let closed = lock(&workspace).close(pane);
        assert!(
            closed.is_some(),
            "the pane this test opened was there to close"
        );
    }
    /// A live plugin host over a workspace holding two panes — the fixture the three grammar gates
    /// drive, plus its own non-vacuity counts.
    ///
    /// TWO panes because `pipe` names a `src` and a `dst`, and a fixture holding one of a thing cannot
    /// tell an argument that resolved from one the verb ignored (the mux fixture's rule, one surface
    /// along).
    fn grammar_gate(
        claim: impl Fn(
            &'static [crate::wire::ActionGrammar],
            sprag_conformance::Invoke<'_>,
        ) -> sprag_conformance::Driven,
    ) -> sprag_conformance::Driven {
        let (workspace, _first) = pane_painting("");
        {
            let mut command = CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            // ⚠ POINTED AT A TREE like the first — the probe addresses THIS pane in its `ai_loop`
            // requests, and a loop is not built over a pane standing outside a tree (item 738).
            command.cwd(a_tree_to_stand_in());
            lock(&workspace)
                .spawn(command, "second".to_string(), 80, 24)
                .expect("a second pane the addressing arguments can name");
        }
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
            None,
        );
        claim(crate::wire::PLUGINS_GRAMMAR, &mut |action, args| {
            external.invoke(action, args)
        })
    }

    /// ⚠⚠ **EVERY WORD THIS SURFACE PUBLISHES IS A WORD IT ACCEPTS.**
    ///
    /// ⚠ Some of these calls START A RUN, which is what makes the claim real: the run is spawned on a
    /// background thread against a pane the fixture holds, and the registry goes out of scope with the
    /// test. A `plugin` word that got as far as spawning is a word the parser read.
    #[test]
    fn every_published_word_is_a_word_the_plugin_host_accepts() {
        assert_eq!(
            grammar_gate(sprag_conformance::every_published_word_is_accepted).count_or_panic(),
            32,
            "one call per published word: the ONE plugin word that selects each of the SIX forms, \
             the two reply formats on each of a dialogue's two endpoints, the readiness barrier's \
             FOUR `match` words on each of the four plugins that inject — the last two being \
             `runs` and `settles`, which ask the pane's terminal and its supervisor rather than \
             its screen — and `done_when`'s TWO words on EACH of the three forms that now take it. \
             ⚠⚠⚠ THE SEVEN NEWEST ARE THE `ai_loop` FORM'S, and this gate caught the same argument \
             a THIRD time on it: `agent` was published as declinable and read with `require_str`, \
             so a caller building the minimal call this grammar describes was answered \
             `TypeMismatch`. It was made required, and the two spellings agreed again. \
             ⚠⚠⚠⚠ IT IS DECLINABLE ONCE MORE SINCE ITEM 738, AND THAT IS NOT THAT DEFECT COMING \
             BACK — the two spellings still agree, because the READER moved with the grammar: a \
             launch that names no `agent` now falls through to the barrier the KIND document \
             authors, and one that reaches nothing at all is refused naming both keys. The defect \
             this line records was a published word the daemon would not accept; what changed here \
             is which party answers, not whether the published call is servable. \
             ⚠⚠⚠ Those four are why this gate is worth its own line, and it has caught the SAME \
             argument TWICE. `done_when`'s first draft published `settles` and the parser REFUSED \
             it, because that draft needed a companion `agent` the vocabulary could not demand. \
             The orchestrator's copy repeated it exactly: its first draft required \
             `turn_within_ms` alongside, so an agent that enumerated this vocabulary would have \
             built a call the daemon rejected. **A published word must be servable ALONE** — which \
             is why a turn contract with no bound is a run bounded by the run's own clock rather \
             than a refusal.",
        );
    }

    /// ⚠⚠ **AN ARGUMENT THIS SURFACE CONSTRAINS PUBLISHES WHAT IT ADMITS** — and it is why the two
    /// bad-word arms answer `TypeMismatch` now.
    ///
    /// A vocabulary the daemon refuses as `Rejected` is INVISIBLE to this gate: the probe comes back
    /// refused for a reason the gate cannot read as a grammar refusal, so a closed argument would look
    /// open and pass. Both of this surface's vocabularies were in that state — `plugin` and
    /// `format_a`/`format_b` each answered a friendly sentence — so the gate could not have held them
    /// even after they were published.
    #[test]
    fn an_argument_the_plugin_host_constrains_publishes_what_it_admits() {
        assert_eq!(
            grammar_gate(sprag_conformance::a_constrained_argument_publishes_what_it_admits)
                .count_or_panic(),
            26,
            "one probe per open string argument of every form. ⚠⚠ THE NEWEST TWO ARE A SCREEN \
             RULE's `when` and `text`, open for the consent needles' reason exactly: `when` quotes \
             the AGENT's own dialog and `text` is the AUTHOR's own prose about their own work, so \
             a closed vocabulary at either could only ever be sprag's guess. THE OLD SENTENCE \
             FOLLOWS. An orchestrator's stimulus, \
             sentinel and ready_when, a PIPE's ready_when, an agent's prompt and ready_when, and \
             a dialogue's seed and two labels — PLUS the ANSWERING CONTRACT's two needles on each \
             of the FOUR forms that inject, the newest pair being the loop's. \
             ⚠⚠ The five before them are the `ai_loop` FORM'S OWN: its \
             three BRIEF strings, the `agent` its barrier is derived from, and its own \
             `ready_when` marker. The brief's three are open for the consent needles' reason \
             turned around — a north star is a PERSON's prose about their own work, so a closed \
             vocabulary there could only ever be sprag's guess at what somebody is trying to do. \
             ⚠ Both of those are open on purpose and it is the \
             one place on this surface where that is a safety property rather than a convenience: \
             a consent quotes the AGENT's own words, so a closed vocabulary here could only ever \
             be sprag's guess at what dialogs say",
        );
    }

    /// ⚠⚠ **A DECLARED ARGUMENT IS ONE THIS SURFACE ACTUALLY READS** — the gate that lets this table
    /// be hand-written, over a verb whose forms were transcribed from a parser by eye.
    ///
    /// ⚠ The number moved by twelve when the loop got a door, and both halves are the point: four
    /// `opened_by` arguments (one per form) and **eight nested `guardrails` fields the claim could
    /// not see before it learned to walk them**. `max_iterations` and each form's cost key are now
    /// each driven at the wrong type inside their parent, which is what turns the nested grammar
    /// from a published claim into a held one.
    /// ⚠⚠ **EVERY OPTIONAL ARGUMENT OF THIS SURFACE MAY BE DECLINED AS `null`** — the class a
    /// hand-written check cannot close, because it is the arguments nobody thought about that are
    /// wrong.
    ///
    /// Found live: `sentinel: null` answered `TypeMismatch` while `ready_when: null` and
    /// `ready_timeout_ms: null` did not, so the SAME request was well-formed or malformed depending
    /// on which optional the client declined. A client whose language serialises absence as `null`
    /// — most of them — could not start an orchestrator run at all without a sentinel.
    #[test]
    fn an_optional_argument_of_a_run_may_be_declined_as_null() {
        assert_eq!(
            grammar_gate(sprag_conformance::an_optional_argument_may_be_declined_as_null)
                .count_or_panic(),
            76,
            "one probe per OPTIONAL declared argument of every form, nesting included — required \
             ones are deliberately not driven, because `null` for something the grammar demands is \
             malformed rather than declined. ⚠⚠⚠⚠⚠ THE NEWEST TWO ARE `reference` AND `agent` \
             (item 738), and they are the SECOND and THIRD arguments on this whole surface ever to \
             move from REQUIRED to declinable — `max_turns` was the first, and its reason is \
             theirs: while a key is mandatory, no document can answer it, so a decision the \
             owner's rule puts in an `.scxml` is one the `.scxml` is structurally forbidden from \
             making. ⚠⚠ What filled that gap is what makes this a defect rather than a tidiness: a \
             person retyped both into every launch out of a session's memory, and the session died \
             at the end of the day. ⚠⚠⚠ DECLINING EITHER DOES NOT REACH THE TEMPLATE, unlike every \
             other key here. `reference` stops at the KIND document, because the template's own \
             value is the placeholder `(edit me) paths, URLs or repos to consult` and R380 measured \
             that reaching a live agent; `agent` stops at the kind's authored barrier, because the \
             template has none and a loop with no barrier types its first prompt into whatever the \
             pane is running. A launch that reaches neither is REFUSED naming the keys. \
             ⚠ And `agent` DECLINED is not `agent` overridden: a caller who names a program still \
             has the barrier derived from it, which is item 300's line and is why a run driving \
             `codex` still gets `codex`. THE OLD SENTENCE FOLLOWS. THE NEWEST IS `hold_within_ms` \
             (item 534), and \
             declining it means what declining the two duration keys beside it means since item \
             300: THIS DOCUMENT DECIDES — `ai_loop.scxml`'s own four hours. ⚠⚠ Zero is NOT a value \
             a caller may mean here, unlike `context_ceiling` and `reflect_after_refusals` below: \
             *hold this run and end it at once* is `cancel` spelled wrong, so it is refused rather \
             than obeyed — and that rule is about a VALUE, which no per-argument sweep can see (see \
             `a_hold_ceiling_travels_alone_and_a_zero_one_is_refused`). ⚠ It is also the one key \
             here that needs NO person declared beside it, which is the whole of item 534: the runs \
             that parked for ever were the unattended ones. THE OLD SENTENCE FOLLOWS. THE NEWEST IS \
             `reflect_after_refusals` (item \
             494), and declining it means what declining `context_ceiling` beside it means, one \
             number over: the caller's, then THIS repository's KIND document, then the template's \
             own `expr=\"3\"`. ⚠⚠ It is here because the CLASS was swept rather than the instance — \
             the template claims two of its numbers for the kind, 492 paid one, and the other was \
             still authorable by nobody. ⚠ Zero is a value a caller may MEAN (reflect on the first \
             refusal), so there is no decline word beside it. THE OLD SENTENCE FOLLOWS. \
             THE NEWEST IS `context_ceiling` (item 492), and \
             declining it means what declining `max_turns` means with one more step in the chain: \
             the caller's number, then THIS repository's KIND document, then the template's own \
             `expr=\"0\"`. ⚠⚠ Its arrival is the item itself rather than a detail of it — the kind \
             document had authored a ceiling since 2026-08-18 and nothing could carry it, so \
             `reviewing` guarded every deciding edge on a number that was 0 on every run anybody \
             has ever driven (item 477 measured eight exits out of eight taking the fall-back). \
             ⚠ Zero is a value a caller may MEAN here, so unlike `max_turns` there is no decline \
             word beside it. THE OLD SENTENCE FOLLOWS. THE NEWEST IS `max_turns`, and it is the one \
             argument on this surface that has ever moved from REQUIRED to declinable (item 312): \
             the document authors `expr=\"40\"` and, while the key was mandatory, no caller could \
             let it decide — so a judgement the owner's rule puts in the `.scxml` was one the \
             `.scxml` was structurally forbidden from making. ⚠⚠ Its arrival here is the point of \
             this sweep rather than a detail of it: declining it now has to mean the same thing \
             declining anything else here means, `null` included, and nothing but a per-argument \
             probe would have said so. ⚠⚠⚠ THE ONE BEFORE IT IS `screen_rules`, and declining it \
             means something no other optional here means: NOT *screen nothing*, but *keep whatever \
             the loop document's author wrote*. The rules live in the template, so a caller who \
             says nothing about screening is not overriding one who did — and the driver echoes the \
             document's own rules back through the brief rather than deleting them. ⚠⚠ ITS TWO \
             NESTED FIELDS ARE **NOT** AMONG THESE, and this gate is what said so rather than a \
             reading of the grammar: a nested field is REQUIRED inside its object, so `null` for it \
             is malformed and not declined — exactly as the consent's two needles are absent here. \
             THE OLD SENTENCE FOLLOWS. THE THREE BEFORE IT ARE THE ANSWERING CONTRACT ON \
             THE LOOP, and their declinability is the whole default: a loop that names no consent \
             answers nothing and reports the question, which is what every loop did before the \
             keys existed — and what was measured costing it every turn it had. \
             ⚠⚠ The eleven before them are the `ai_loop` FORM'S own, and \
             what is NOT among them WAS the point: the brief's four and the `agent` were REQUIRED, \
             because a loop with no purpose and a loop with no barrier are both runs nobody can \
             mean. ⚠⚠⚠ TWO OF THOSE FIVE ARE AMONG THEM NOW (item 738) AND THAT SENTENCE STILL \
             HOLDS, which is the only reason they could move: what makes it true is that neither \
             CAN end up unset. `reference` reaches the kind document's own and `agent` reaches the \
             barrier it authors, and a launch that reaches neither is refused. The sentence was \
             never about which party has to speak — it was about a run starting without a purpose \
             or without a barrier, and that is still impossible. \
             ⚠⚠ `reflect_every` IS declinable, and its default is STILL `max_turns` — which \
             used to mean *the one number that keeps the run inside the states this build drives* and \
             now means something else entirely: `reflecting` is served, so that default is a CHOICE \
             rather than a limit. A restart closes a pane somebody may be reading, so a caller who \
             said nothing about reflection has not asked for one; what they get anyway is the \
             reflection a STANDING INSTRUCTION triggers, which is a correctness edge, not a budget. \
             ⚠⚠⚠ The TWO before them are the orchestrator's turn \
             contract, `done_when` and `turn_within_ms`, and their declinability IS the default \
             that keeps every existing caller working: a run that names neither ends its steps on \
             the same 500 ms constant it always did. ⚠ Declinable ALONE is all this drives; that \
             HALF the pair may not be sent alone is a rule no per-argument sweep can see — see \
             `a_turn_contract_missing_half_of_itself_is_malformed`. ⚠⚠⚠ The THREE before them are \
             `handback_still_ms` on each \
             LOOPING form, and its declinability is the default that keeps every existing caller \
             working: a run that names no stillness ends when somebody takes its pane, which is \
             what every run did before the key existed. ⚠ Declinable ALONE is all this drives; that \
             it may not be SENT alone is a rule about a PAIR, which no per-argument sweep can see \
             — see `a_handback_for_a_run_nobody_is_watching_is_malformed`. ⚠⚠ The THREE before \
             them are `await_person_ms` on each \
             LOOPING form, and its declinability is the whole default in the same way the \
             consent's is: a run that names no patience is unattended and ends when its peer asks \
             something no clause covers, which is what every run did before the key existed. \
             ⚠ It is not on the `answer` form, which is CALLED BY the person a wait would wait \
             for. ⚠ The three before them are `may_answer` on each injecting \
             form, and its declinability is the whole default: a run that names no consent answers \
             nothing and reports the question, which is what every run did before the key existed. \
             ⚠⚠ The FIVE this round added are the `answer` form's own optionals — its `opened_by` \
             and its three guardrail fields — and NOT its consent, which is the one argument on \
             this surface that a form REQUIRES: `may_answer` is declinable on the looping \
             forms and mandatory on the one whose whole content it is",
        );
    }

    #[test]
    fn a_declared_argument_is_one_the_plugin_host_reads() {
        assert_eq!(
            grammar_gate(sprag_conformance::a_declared_argument_is_one_the_daemon_reads)
                .count_or_panic(),
            121,
            "one probe per declared argument of every FORM, nesting included: TWENTY for an \
             orchestrator, SEVENTEEN for a pipe, TWENTY-ONE for an agent, sixteen for a dialogue, \
             TEN to answer a pane, THIRTY-ONE to run an AI loop, one to cancel, one to stand a run \
             down, and TWO TO REPORT A RUN'S PROGRESS. \
             ⚠⚠⚠⚠⚠ THE NEWEST TWO ARE `report_progress`'s `id` AND `progress` (item 650), and this \
             gate reaches them for a reason the verbs above it do not have: the only caller is a \
             DRIVER IN ANOTHER PROCESS, so a key this host declared and swallowed would be a \
             counter nobody could ever see move, reported `ok` by both sides. ⚠⚠ `progress` is \
             declared as a bare OBJECT and this gate is what keeps that honest: the host must READ \
             it, and it does — whole, into the run's record, without unpacking it. Declaring its \
             keys instead would make this pin the second author of a shape `progress_to_json` \
             already owns. THE OLD SENTENCE FOLLOWS. THE LOOP'S THIRTY-FIRST WAS `hold_within_ms` \
             (item 534), \
             and this gate is what makes it more than a declaration for its two predecessors' \
             reason: a published argument the host does not READ is a key the surface swallows \
             while the run reports `ok`. ⚠⚠ IT IS ON THIS FORM ALONE, unlike the two person keys it \
             sits beside — the ceiling is a `<data>` in `ai_loop.scxml` and that document is the \
             only thing in this workspace that reads a hold at all, so declaring it on the other \
             three LOOPING forms would advertise an argument they swallow, which is the exact \
             defect this gate exists to catch. THE OLD SENTENCE FOLLOWS. THE LOOP'S THIRTIETH WAS \
             `reflect_after_refusals` (item \
             494), and this gate is what makes it more than a declaration for the same reason it \
             did for its twin: a published argument the host does not READ is a key the surface \
             swallows while the run reports `ok`. ⚠⚠ The twin is the point — the template claims \
             exactly two of its numbers for the KIND to author, item 492 built the road for one, \
             and the identical defect was still standing one `<data>` up with a GATE for its only \
             writer. THE OLD SENTENCE FOLLOWS. THE NEWEST IS THE LOOP'S TWENTY-NINTH, \
             `context_ceiling` (item \
             492), and this gate is the one that makes it more than a declaration: a published \
             argument the host does not READ is a key the surface swallows while the run reports \
             `ok`. That is the whole shape of the item — the number existed in the kind's document \
             since 2026-08-18 and nothing carried it, so `reviewing` decided on 0 for every run \
             this repository has ever driven. THE OLD SENTENCE FOLLOWS. ⚠⚠⚠ THE NEWEST IS THAT \
             LAST ONE — the second thing anybody can say to a \
             run, and the first that does not throw the turn in flight away. It takes a run id and \
             nothing else, exactly as `cancel` does, and it is a SEPARATE verb for that reason \
             rather than in spite of it: the two shapes are identical and the outcomes are \
             opposite, so a mode flag on one of them would let a caller lose a milestone by \
             mistyping a boolean. THE OLD SENTENCE FOLLOWS. one probe per declared argument of \
             every FORM, nesting included: TWENTY for an \
             orchestrator, SEVENTEEN for a pipe, TWENTY-ONE for an agent, sixteen for a dialogue, \
             TEN to answer a pane, TWENTY-EIGHT to run an AI loop, and one to cancel. ⚠⚠⚠ THE \
             NEWEST THREE ARE `screen_rules` AND ITS TWO NESTED FIELDS — the loop author's standing \
             instructions, and the SECOND authority over one dialog. A consent takes an option the \
             peer OFFERED, which structurally cannot cover the question a loop meets when its \
             agent wants a DECISION (*the quick way or the thorough way?* offers nothing anybody \
             could authorise in advance); a rule refuses the call and says what to do instead. ⚠ It \
             names no KEY, and that is a safety property rather than a simplification: the key is \
             the product's, measured, and a rule that could name its own could name the one that \
             APPROVES — a live probe pressed `Tab` and had the agent's file written. THE OLD \
             SENTENCE FOLLOWS. TWENTY for an \
             orchestrator, SEVENTEEN for a pipe, TWENTY-ONE for an agent, sixteen for a dialogue, \
             TEN to answer a pane, TWENTY-FIVE to run an AI loop, and one to cancel. \
             ⚠⚠⚠ THE NEWEST FIVE ARE THE ANSWERING CONTRACT REACHING THE LOOP — `may_answer` with \
             its two needles, `await_person_ms` and `handback_still_ms`. It was the ONE injecting \
             form without them, on the argument that answering a dialog belongs to a state in the \
             document; that state is unbuilt, and the cost was measured as a loop whose agent \
             asked one permission question stopping with ZERO turns judged. \
             ⚠⚠⚠ THE NEWEST TWENTY ARE THE `ai_loop` FORM, the door register item 65 had been \
             holding open since R378 — five rounds built that loop's machine, its driver and its \
             live measurement, and nothing in the daemon constructed one. FOUR of the twenty are \
             the BRIEF (`north_star`, `milestone`, `reference`, `max_turns`), which is the one \
             thing on this whole surface that no other form has: every other plugin is told what \
             to TYPE, and a loop is told what it is FOR and composes each turn's prompt from that \
             itself. ⚠ `agent` is required beside them for a measured reason — a loop with no \
             barrier types its first prompt into whatever the pane happens to be running, which \
             R379 measured costing a whole run. \
             ⚠⚠⚠ The two before them are the ORCHESTRATOR's \
             TURN CONTRACT — `done_when`, which the `agent` form already had, and `turn_within_ms` \
             — and they are on that form because it is where the defect was MEASURED: without them \
             a step ends on a 500 ms constant, so a peer that thinks for three seconds was asked \
             its one question SIX times, every prompt after the first landing while it was still \
             answering. The `agent` adapter never had that defect, because it asks a contract \
             instead of a clock; this is that contract offered to the plugin the MCP verb and the \
             outer AI loop actually drive. ⚠ NOT on `pipe`, which is a scope cut and not a \
             judgement — a relay's destination has turns too. ⚠⚠⚠ The THREE before them are \
             `handback_still_ms`, on each form that LOOPS and on none that does not — the second \
             half of `turn.interrupted`, which shipped with only the first: a run learnt to STOP \
             for a person and had no way to be given the pane back. It is not on the `answer` \
             form for its neighbour's reason, doubled: that form is CALLED BY the person, so a run \
             waiting for their hand to go still would be waiting for its own caller to stop \
             calling it. ⚠⚠ The THREE before them are `await_person_ms`, on \
             each form that LOOPS and on none that does not — the other half of the answering \
             contract: what the run may answer itself, and who answers what it may not. The \
             `answer` form is the one injecting form without it, because its caller IS the person \
             a wait would be waiting for. ⚠ The TEN before them are the whole `answer` form, \
             which is the answering contract with NO LOOP AROUND IT: a pane, a consent, and the \
             bounds every run carries. It declares no stimulus and no readiness barrier, and both \
             absences are the design — the only bytes it can emit are the ones the consent \
             authorised, and a pane whose program has not started cannot be showing a dialog. \
             ⚠ The nine before them are the ANSWERING CONTRACT on the three forms that \
             inject — `may_answer` and its two needles — which completes the turn's three declared \
             contracts: when it may START (`ready_when`), what makes it OVER (`done_when`), and \
             what the run may ANSWER if the peer interrupts it with a question of its own. ⚠ The \
             agent's `done_when` is the one argument of the lot that is a BARE word. ⚠ Eleven are \
             the READINESS BARRIER on the THREE plugins that inject, each carrying `ready_when` \
             AND its two nested fields: a marker alone could not say whether text already on the \
             screen is evidence, so the value became an object",
        );
    }

    /// ⚠⚠ **THE NESTED GUARDRAILS CAN BE OFFERED ONE FLAG AT A TIME** — the property both new mouths
    /// rest on, driven over the declarations rather than assumed by the code that flattens them.
    ///
    /// A `max_iterations` that collided with a top-level argument would make `--max-iterations`
    /// mean two things, and the mouth would pick one silently. This is the only place that can say
    /// it does not, because the collision is a property of the TABLE and no call exhibits it.
    #[test]
    fn the_plugin_hosts_nested_arguments_flatten_without_collision() {
        assert_eq!(
            sprag_conformance::a_flattened_nested_argument_collides_with_nothing(
                crate::wire::PLUGINS_GRAMMAR
            )
            .count_or_panic(),
            26,
            "one per FLATTENED nested field of every form: THREE guardrail fields on each of the \
             SIX run forms, since a run is bounded in steps, in spend and in time, PLUS the \
             readiness barrier's `match` and `marker` on each of the four that inject. \
             ⚠⚠ THE FIVE NEWEST ARE THE `ai_loop` FORM'S: a loop injects, so it takes the barrier \
             every injecting form takes, and it spends BYTES — the prompts it types — so its \
             guardrail object is the byte-relay one. What it does NOT take is a cost bound on the \
             agent's own spend, which this daemon neither bills nor can count; that budget is \
             `max_turns`, and it is in the brief rather than in the guardrails. \
             ⚠⚠ THE CONSENT'S `asked`/`answer` ARE NOT COUNTED, and the drop of eight is R370's \
             design rather than a lost check: `may_answer` is a LIST of clauses now, and a list is \
             the one nested shape that cannot be flattened — N loose `asked`s beside N loose \
             `answer`s cannot say which pairs with which — so both flattening mouths offer it \
             whole. Its fields are never flags, so there is nothing for them to collide with. \
             ⚠ What DOES still run is the mirror: `may_answer` is a top-level flag now, and a \
             field of another nest sharing that name is caught here",
        );
    }

    /// ⚠⚠ **A RUN CAN ONLY BE OPENED BY A PANE THIS DAEMON HOLDS** — the arm the type gate cannot
    /// reach, and the one that keeps the provenance prunable.
    ///
    /// `a_declared_argument_is_one_the_plugin_host_reads` drives `opened_by` at the wrong TYPE and
    /// gets `TypeMismatch`; a well-formed number naming a pane that does not exist is a different
    /// answer and a different branch. Without it a caller with a stale `SPRAG_PANE` — a process
    /// that outlived its own pane — would stamp a run with a provenance nothing can ever resolve,
    /// and the agent-facing mouth would filter on a pane number that means nothing.
    ///
    /// The multiplexer states this rule for a pane's own `opened_by`; this is the same rule one
    /// level up, and it is asserted rather than inherited because they are two parsers.
    #[test]
    fn a_run_opened_by_a_pane_this_daemon_does_not_hold_is_refused() {
        let (workspace, pane) = pane_painting("");
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
            None,
        );
        let mut ask = |opener: u64| {
            external.invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    RUN_OPENED_BY_KEY: opener,
                })),
            )
        };
        let refused = ask(9999).expect_err("a pane nobody holds cannot have opened a run");
        assert!(
            format!("{refused:?}").contains("no pane 9999"),
            "it is a well-formed request this host will not honour, and it says which pane: \
             {refused:?}",
        );

        // THE CONTROL: the same call naming a pane that IS here starts a run, so the refusal is
        // about the opener and not about the shape of the request.
        assert!(
            ask(pane.0).is_ok(),
            "a real pane may open a run, or the refusal above is about something else",
        );
    }

    /// ⚠⚠⚠ **AN ARGUMENT THIS SURFACE DOES NOT DECLARE IS SWALLOWED, NOT REFUSED** — measured,
    /// because it is what decides whether an ADDED argument earns a protocol number.
    ///
    /// The rule this project reasons from is that an addition is additive when an older daemon
    /// **refuses it loudly by name**: the caller learns it is talking to a stale peer, and no
    /// silent difference of behaviour survives. R363 measured exactly that for an added ACTION —
    /// an unknown verb comes back `UnknownPath`, which every mouth renders as skew.
    ///
    /// An added ARGUMENT is the opposite, and this is the gate that says so rather than a comment
    /// asserting it. The plugin host reads the keys it knows and walks past the rest, so a request
    /// carrying a key an older daemon has never heard of is ACCEPTED, the run starts, and it
    /// converges — under the behaviour the key was sent to change. That is version 17's failure and
    /// version 23's (`shows_prompt`): *the request is accepted, the run converges, and the answer
    /// is byte-identical either way.*
    ///
    /// ⚠ So every argument added to this surface owes a `WIRE_PROTOCOL` bump, and this gate is the
    /// evidence for the next person who has to decide.
    #[test]
    fn an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused() {
        let (workspace, pane) = pane_painting("");
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
            None,
        );
        let accepted = external.invoke(
            RUN_ACTION,
            IntrospectValue::Json(json!({
                "plugin": "orchestrator",
                "pane": pane.0,
                "stimulus": "x",
                // A key no version of this surface has ever declared, standing in for one a FUTURE
                // client sends to a daemon that predates it.
                "a_key_from_a_later_protocol": "surprise",
            })),
        );
        assert!(
            accepted.is_ok(),
            "⚠⚠⚠ the request carrying an unknown key was ACCEPTED. A client that sent it to buy \
             different behaviour got the old behaviour and a successful answer, which is why an \
             added ARGUMENT cannot be additive the way an added ADDRESS or ACTION is: {accepted:?}",
        );
    }

    /// ⚠ **NO VERB OF THIS SURFACE TAKES NOTHING, ASSERTED RATHER THAN ASSUMED** — the tripwire that
    /// makes `a_nullary_form_is_a_verb_that_needs_nothing` start holding it the day one does.
    ///
    /// The claim exists because the GUI's five nullary verbs needed it, and R353's `FormKind` doc had
    /// said sprag had none of them. A number here is what keeps that from being a statement about the
    /// surfaces somebody happened to be looking at.
    #[test]
    fn no_verb_of_this_surface_is_nullary_yet() {
        let (workspace, _first) = pane_painting("");
        let mut external = PluginsExternal::new(
            workspace,
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(
            sprag_conformance::a_nullary_form_is_a_verb_that_needs_nothing(
                crate::wire::PLUGINS_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            0,
            "every verb this surface serves takes arguments, so the claim drives nothing — and the \
             number is what says so",
        );
    }

    /// A live plugin host over one pane, and its registry — the fixture the two duration gates
    /// share. The registry is handed back because a run's ending is read off it.
    fn host_with_a_pane() -> (PluginsExternal, Arc<Mutex<RunRegistry>>, PaneId) {
        let (workspace, pane) = pane_painting("");
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let external = PluginsExternal::new(
            workspace,
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        (external, registry, pane)
    }

    /// Poll the registry until run `id` has left `running`, and answer its rendered JSON.
    ///
    /// Bounded well above the ceiling under test, so a run that IGNORED its deadline fails here
    /// with a timeout rather than hanging the suite.
    fn ended(registry: &Arc<Mutex<RunRegistry>>, id: u64, within: Duration) -> Value {
        let start = Instant::now();
        loop {
            let entry = {
                let mut held = lock(registry);
                held.sweep();
                held.snapshot()
                    .iter()
                    .find(|run| run.id.0 == id)
                    // The seat as the record itself names it: this helper watches runs THIS
                    // registry issued, so there is no inherited conversation to re-derive from.
                    .map(|run| run_to_json(run, run.opened_by))
            };
            if let Some(entry) = &entry
                && entry["state"]["status"] != json!("running")
            {
                return entry.clone();
            }
            assert!(
                start.elapsed() < within,
                "run {id} was still running after {:?}: {entry:?}",
                start.elapsed(),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// ⚠⚠ **AN EXPLICIT `null` IS AN OMISSION, NOT A MALFORMED VALUE** — the arm the conformance
    /// walk cannot reach, and which was asserted nowhere.
    ///
    /// That walk drives every declared argument at the WRONG TYPE to prove the parser refuses it,
    /// so it reaches [`InvokeError::TypeMismatch`] for a string where an int belongs. `null` is the
    /// one value that must NOT be refused: a client serialising an absent optional from a language
    /// where absence IS `null` — which is most of them — sends it on every call, and a daemon that
    /// answered `TypeMismatch` would reject well-formed runs from an entire class of client.
    ///
    /// ⚠ Both spellings, in one call: the two `*_ms` bounds and the barrier itself. `ready_when`
    /// carries the rule too, and it is the one that matters most — a nested UNIT read as malformed
    /// rather than absent is a run refused for declining an optional feature.
    #[test]
    fn an_explicitly_null_optional_reads_as_absent_rather_than_malformed() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "sentinel": null,
                    "ready_when": null,
                    "ready_timeout_ms": null,
                    "guardrails": { "max_iterations": 1, "max_seconds": 5 },
                })),
            )
            .expect(
                "an optional spelled `null` is one the caller declined — refusing it would reject \
                 every client whose language serialises absence that way",
            );
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        // ⚠ AND IT REALLY RAN. A parse that accepted `null` and then quietly built a different spec
        // would pass the line above; the run has to reach an ending of its own.
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        assert!(
            entry["state"]["outcome"]["state"].is_string(),
            "the run built from the declined optionals ran to an ending of its own: {entry:?}",
        );
    }

    /// ⚠⚠ **THE `failure` A CLIENT READS IS A SENTENCE ABOUT THE PANE, NOT A RUST VARIANT** — the
    /// wire half, and the half that decides whether the fix R358 made is worth anything.
    ///
    /// This key was `format!("{e:?}")`, so a failed run published `Write("Broken pipe (os error
    /// 32)")` to an agent that has no way to look up what `Write` is. The remedy — a `Display`
    /// impl, published with `ToString::to_string` — had a gate nowhere, so reverting one call
    /// would have broken nothing and the leak would have come straight back.
    ///
    /// Driven through the readiness failure because it is the one a caller can provoke on purpose:
    /// a marker the pane never prints, with `ready_timeout_ms` short enough that the RUN's clock is
    /// provably not what ended it. Three claims: the run FAILED, the text names the marker, and it
    /// does not read as Rust.
    #[test]
    fn a_failed_run_publishes_a_sentence_about_the_pane_rather_than_a_rust_variant() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "ready_when": {
                        "match": "prints",
                        "marker": "A MARKER THIS PANE NEVER PRINTS",
                    },
                    "ready_timeout_ms": 200,
                    // Far above the readiness bound, so neither ceiling can be what ends this.
                    "guardrails": { "max_iterations": 100_000, "max_seconds": 60 },
                })),
            )
            .expect("a run that names a readiness barrier is a well-formed run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        let outcome = &entry["state"]["outcome"];

        assert_eq!(
            outcome["state"],
            json!("failed"),
            "a pane that never becomes ready FAILS the run — it is not a ceiling the run reached, \
             and a client that read `exhausted` here would go looking for a budget it never hit: \
             {entry:?}",
        );
        let said = outcome["failure"]
            .as_str()
            .unwrap_or_else(|| panic!("a failed run publishes its cause as text: {entry:?}"));
        assert!(
            said.contains("A MARKER THIS PANE NEVER PRINTS"),
            "and the text names what the run waited for, which is the only thing that tells the \
             caller WHICH marker they got wrong: {said:?}",
        );
        assert!(
            said.contains(' ') && said.starts_with(char::is_lowercase),
            "it has to read as prose to the agent that receives it, not as a Rust variant and its \
             debug payload: {said:?}",
        );
        assert!(
            !said.contains("NeverReady"),
            "the variant name is the leak itself: {said:?}",
        );
    }

    /// ⚠⚠⚠ **AND THE COMMONEST READINESS MISTAKE IS NAMED ON THE WIRE: THE MARKER WAS ALREADY
    /// THERE.**
    ///
    /// The sibling of the gate above, and the one that matters to a caller who did nothing wrong on
    /// purpose. `prints` means *more occurrences than when this run started watching*, so a pane
    /// that announced itself on the way up can never satisfy it — and **opening a pane and asking
    /// for a run are two separate calls**, which is the normal order and the whole window.
    ///
    /// What came back named the JOB (*"its terminal belonged to `cat`"*): true, about a question
    /// the caller had not asked, and silent on the one fact that corrects the call.
    ///
    /// ⚠⚠⚠ **AND IT IS DRIVEN THROUGH THE WIRE'S OWN DOOR RATHER THAN THE BARRIER'S.** The plugin
    /// crate gates the sentence where it is built; this asks whether it SURVIVES to the `failure`
    /// key a client reads, which is a different question and the one R373 paid for learning to ask
    /// separately.
    #[test]
    fn a_readiness_marker_the_pane_had_already_printed_is_named_as_such_on_the_wire() {
        let (workspace, pane) = pane_painting("BANNER\\r\\n");
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        // ⚠ THE WINDOW EVERY REAL CALLER HAS, OPENED ON PURPOSE. Waiting for the announcement here
        // is what makes this deterministic rather than a race the fast machine happens to win.
        wait_for_screen(
            &WorkspacePaneAccess::new(Arc::clone(&workspace)),
            pane,
            "BANNER",
        );
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "ready_when": { "match": "prints", "marker": "BANNER" },
                    "ready_timeout_ms": 300,
                    // Both ceilings out of reach, so the readiness bound is provably what ended it.
                    "guardrails": { "max_iterations": 100_000, "max_seconds": 60 },
                })),
            )
            .expect("a run that names a readiness barrier is a well-formed run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        let said = entry["state"]["outcome"]["failure"]
            .as_str()
            .unwrap_or_else(|| panic!("a failed run publishes its cause as text: {entry:?}"))
            .to_string();
        assert!(
            said.contains("already on its screen"),
            "⚠⚠⚠ the client must be told the marker IS THERE. Without it they are told what owns \
             the terminal — true, and about a question they did not ask — and the fact that \
             corrects their call never leaves the daemon: {said:?}",
        );
        assert!(
            said.contains("\"shows\""),
            "⚠⚠ and the question that WOULD have read it, in the same wire word they would have to \
             send: {said:?}",
        );
    }

    /// ⚠⚠ **A RUN ASKED TO STOP AFTER A SECOND STOPS AFTER A SECOND** — the wire half of the
    /// duration ceiling, end to end through the verb a client actually calls.
    ///
    /// The iteration ceiling is put out of reach (a hundred thousand steps this pane will never
    /// take) so that the ONLY bound that can end this run is the clock. Before the ceiling existed
    /// the same call was answered `Ok` and bounded by iterations instead — which is the exact
    /// failure this gate is shaped around: not a refusal, an ANSWER OF SUCCESS over a bound nobody
    /// applied.
    ///
    /// ⚠ The `ceiling` key is the second half and not a decoration. A run that stopped at a second
    /// and reported only `exhausted` would be indistinguishable, to every reader on this wire, from
    /// one that ran out of turns.
    #[test]
    fn a_run_asked_to_stop_after_a_second_stops_at_the_clock_and_says_so() {
        let (mut external, registry, pane) = host_with_a_pane();
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                    "guardrails": { "max_iterations": 100_000, "max_seconds": 1 },
                })),
            )
            .expect("a run bounded in time is a well-formed run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };

        let took = Instant::now();
        let entry = ended(
            &registry,
            u64::try_from(id).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        let outcome = &entry["state"]["outcome"];

        assert_eq!(
            outcome["state"],
            json!("exhausted"),
            "a run out of time is exhausted by a guardrail, not converged or failed: {entry:?}",
        );
        assert_eq!(
            outcome[RUN_CEILING_KEY],
            json!("duration"),
            "and the guardrail it names is the CLOCK — the iteration ceiling was a hundred \
             thousand and this pane never took a hundred thousand steps: {entry:?}",
        );
        assert!(
            took.elapsed() < Duration::from_secs(10),
            "it must stop near the second it was given, not at some other bound: {:?}",
            took.elapsed(),
        );
        // ⚠⚠ AND WHAT BECAME OF THE WORK reaches the wire beside it. A run out of time ends while a
        // step may still be blocked on the peer it set going, so `exhausted — duration` alone is
        // consistent with the work having stopped AND with it running on; a caller cannot act on
        // that. This key is the difference, and the ORCHESTRATOR names its pane
        // (`Plugin::driving`), so a run against one must carry it.
        let stopped = outcome[RUN_STOPPED_KEY]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !stopped.is_empty(),
            "a run cut short must say what became of its work, or `exhausted` is half an answer: \
             {entry:?}",
        );
        assert!(
            stopped.contains(' ') && stopped.starts_with(char::is_lowercase),
            "and it reads as prose to the agent that receives it, not as a Rust variant: \
             {stopped:?}",
        );
        assert!(
            !stopped.contains("Stopped") && !stopped.contains("Signalled"),
            "the variant name is the leak itself: {stopped:?}",
        );

        // ⚠ AND THE COST CEILING'S OWN WORD, driven to the wire rather than asserted at the type.
        // `iterations` reaches it through both mouths' end-to-end gates and `duration` through the
        // block above; without this the third word would be the one no test ever spelled — and a
        // ceiling that reaches an agent under the wrong name is worse than one that reaches it
        // under none.
        let spent = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "sentinel": "A SENTINEL THIS PANE NEVER PRINTS",
                    "guardrails": { "max_iterations": 100_000, "max_bytes": 1 },
                })),
            )
            .expect("a run bounded in bytes is a well-formed run");
        let IntrospectValue::Int(spent) = spent else {
            panic!("a run answers its id: {spent:?}");
        };
        let entry = ended(
            &registry,
            u64::try_from(spent).expect("a run id is not negative"),
            Duration::from_secs(20),
        );
        assert_eq!(
            entry["state"]["outcome"][RUN_CEILING_KEY],
            json!("cost"),
            "the SPEND ceiling names itself `cost` — the concept, not `max_bytes` the knob, \
             because the same ceiling is set by `max_tokens` on a run that spends tokens and one \
             answer cannot be two argument names: {entry:?}",
        );
    }

    /// ⚠⚠ **A BOUND THIS DAEMON DOES NOT KNOW IS REFUSED, WHERE EVERY OTHER UNKNOWN KEY ON THIS
    /// WIRE IS IGNORED** — and the asymmetry is the claim, so both halves are driven here.
    ///
    /// Ignoring an ordinary argument makes a verb do LESS than it was asked, and the caller can see
    /// that in the result. Ignoring a bound makes the run do MORE — without limit — and answers
    /// success. `guardrails: {"max_secnods": 5}` was a run with no time ceiling, no way to find
    /// out, and a typo for a cause.
    ///
    /// ⚠ THE CONTROL is the same call with the key spelled right: it must be ACCEPTED. Without it
    /// this gate would also pass over a parser that refused every guardrail object there is.
    /// ⚠⚠ **EVERY DECLARED GUARDRAIL IS ONE THE PARSER ACTUALLY READS** — the direction the gate
    /// beside this one cannot see.
    ///
    /// [`parse_guardrails`] refuses a key the publication does not name, so a bound this daemon
    /// cannot honour is never silently ignored. The MIRROR of that was uncaught: a field ADDED to
    /// `GUARDRAILS_BYTES`/`GUARDRAILS_TOKENS` and never wired into the parser is ACCEPTED by that
    /// same refusal loop — it is declared, after all — and then read by nobody. The caller is told
    /// about a bound, sends it, gets a success, and has no bound. **That is the exact failure the
    /// refusal above exists to prevent, arriving through the other door.**
    ///
    /// ⚠ THE PROBE IS A WRONG TYPE, because it is what separates the two. Every honoured field is
    /// type-checked (`as_u64().ok_or(TypeMismatch)`), so a string where an int is declared must be
    /// REFUSED — and a declared field nobody reads cannot refuse anything. Sending a well-formed
    /// value would be accepted either way and would measure nothing.
    ///
    /// ⚠ Derived from [`PluginGrammar::guardrail_fields`], never from a list here: a fourth
    /// guardrail is covered the day it is declared, which is the day it can go unread.
    #[test]
    fn every_declared_guardrail_is_one_the_parser_actually_reads() {
        let (mut external, _registry, pane) = host_with_a_pane();
        let mut ask = |guardrails: Value| {
            external.invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "guardrails": guardrails,
                })),
            )
        };

        let declared =
            crate::wire::PluginGrammar::guardrail_fields(sprag_plugin::Cost::Bytes(0).unit());
        assert!(
            !declared.is_empty(),
            "the walk must have something to drive, or it reports a clean surface having asked \
             nothing",
        );
        for field in declared {
            let said = format!("{:?}", ask(json!({ field.name: "not a number" })));
            assert!(
                said.contains("Err"),
                "`{}` is DECLARED as a guardrail and the parser does not read it: a string where \
                 an int is published was accepted, so a caller sending this bound gets a success \
                 and no bound — {said}",
                field.name,
            );
        }
    }

    #[test]
    fn a_guardrail_this_daemon_does_not_know_is_refused_rather_than_ignored() {
        let (mut external, _registry, pane) = host_with_a_pane();
        let mut ask = |guardrails: Value| {
            external.invoke(
                RUN_ACTION,
                IntrospectValue::Json(json!({
                    "plugin": "orchestrator",
                    "pane": pane.0,
                    "stimulus": "x",
                    "guardrails": guardrails,
                })),
            )
        };

        let refused = ask(json!({ "max_secnods": 5 }))
            .expect_err("a bound this daemon cannot honour must not be answered with success");
        let said = format!("{refused:?}");
        assert!(
            said.contains("max_secnods") && said.contains("max_seconds"),
            "the refusal names the key it did not know AND what it takes instead, or a caller \
             cannot fix a typo from it: {said}",
        );

        // THE CONTROL — the same shape, spelled as the grammar publishes it.
        assert!(
            ask(json!({ "max_seconds": 5 })).is_ok(),
            "the declared spelling must be accepted, or the refusal above is about guardrails in \
             general rather than about an unknown one",
        );

        // ⚠ AND THE REFUSAL IS PER UNIT, because the published forms are: a byte-relay plugin is
        // not offered `max_tokens`, so naming one is naming a bound that cannot guard this run.
        let wrong_unit = ask(json!({ "max_tokens": 5 }))
            .expect_err("a token bound is not a guardrail of a run that spends bytes");
        assert!(
            format!("{wrong_unit:?}").contains("bytes"),
            "and it says which unit this run spends: {wrong_unit:?}",
        );
    }

    /// ⛔⛔⛔⛔ **A PERSON SPEAKING TO A RUN IS AN EVENT, AND A REFUSED ORDER IS NOT** — register
    /// item 648.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a run needed this and a pane did not
    ///
    /// Every other subject on this daemon's journal reports its CHANGES; a run reported only its
    /// ENDING (`Event::RunFinished`). That was enough while the only driver was a THREAD in this
    /// process, reading the three order flags directly out of shared memory. **It stops being
    /// enough the moment the driver is another process** — register item 544, whose stages 1 and 2
    /// are already standing. Such a driver can READ its orders (the run row publishes
    /// [`RUN_STOOD_DOWN_KEY`] and [`RUN_CANCELLED_BY_KEY`]) and had **nothing to be woken by**, so
    /// it would have to ask on a clock.
    ///
    /// ⚠⚠ **THAT IS THE SHAPE FOUR ROUNDS WERE SPENT REMOVING FROM THE PANE AXIS** — items 629,
    /// 630, 631 and 640, ending at *a remote wait that cost 181 reads over two seconds now costs
    /// 1*. Building the out-of-process driver without this would have rebuilt it one axis over,
    /// which is why this gate comes BEFORE that driver rather than after it.
    ///
    /// ⚠ **AND THE COST HERE IS LATENCY, NOT READS.** An order is not *something is coming*, it is
    /// *a person has just spoken*. A cancel that arrives a poll interval late is a peer typed at
    /// for a poll interval longer and a budget spent on work somebody had already stopped.
    ///
    /// # The control is the half that could go wrong silently
    ///
    /// Announcing on EVERY call — including the refusals — would look identical from the accepted
    /// side and would wake every watcher of the session to re-read a row that never moved. So the
    /// gate drives a REFUSED order at an id no run carries and asserts the journal stayed put.
    #[test]
    fn an_order_a_person_gives_a_run_reaches_the_journal_and_a_refused_one_does_not() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        // The hook the daemon mints in `crate::workspace_scene`, as a recorder: what crosses the
        // boundary is a call with a run id in it, which is exactly what this collects.
        let heard: Arc<Mutex<Vec<u64>>> = Arc::new(Mutex::new(Vec::new()));
        let announced = {
            let heard = Arc::clone(&heard);
            Some(Arc::new(move |id: RunId| lock(&heard).push(id.0))
                as Arc<dyn Fn(RunId) + Send + Sync>)
        };
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            announced,
        );

        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };
        let id = u64::try_from(id).expect("a run id is not negative");
        // ⚠ THE ORDER GOES TO A RUN THAT IS PROVABLY DRIVING, its sibling gates' rule: an order at a
        // run that has not started yet would be answered by the registry rather than by this door.
        drop(driving(&external, id, Duration::from_secs(30)));

        // ── THE CONTROL FIRST: a refused order announces nothing ───────────────────────────────
        // ⚠⚠ Before the claim, deliberately. A hook that fired on every call would satisfy the
        // claim below and be wrong in the way that costs — waking every watcher of a session to
        // re-read a row nothing touched.
        let missing = id + 1_000;
        external
            .invoke(
                CANCEL_ACTION,
                IntrospectValue::Json(json!({ "id": missing })),
            )
            .expect_err("no run carries that id");
        assert!(
            lock(&heard).is_empty(),
            "⚠⚠⚠⚠ A REFUSED ORDER IS A FACT ABOUT THE REQUEST, NOT ABOUT ANY RUN. Announcing it \
             wakes every watcher of this session to re-read a row that never moved — and it is \
             indistinguishable from the accepted case at the assertion below, which is what makes \
             it the half that goes wrong silently. Heard {:?}",
            lock(&heard),
        );

        // ── THE CLAIM: each of the three orders reaches the journal ───────────────────────────
        external
            .invoke(HOLD_RUN_ACTION, IntrospectValue::Json(json!({ "id": id })))
            .expect("a run can be held");
        external
            .invoke(
                STAND_DOWN_ACTION,
                IntrospectValue::Json(json!({ "id": id })),
            )
            .expect("an ai_loop run reads a stand-down");
        external
            .invoke(CANCEL_ACTION, IntrospectValue::Json(json!({ "id": id })))
            .expect("a run in flight can be cancelled");

        let told = lock(&heard).clone();
        assert!(
            lock(&workspace).close(pane).is_some(),
            "the pane this gate opened was there to close",
        );
        assert_eq!(
            told,
            vec![id, id, id],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 648: A PERSON SPOKE TO A RUN AND THE JOURNAL SAID NOTHING. \
             All three orders move a flag the run row already publishes, so a driver OUTSIDE this \
             daemon can read them — and with no event it can only ask on a clock, which is the \
             shape items 629/630/631/640 spent four rounds taking off the pane axis. Expected one \
             announcement per accepted order for run {id}, heard {told:?}",
        );
    }

    /// **A RUN THAT ENDED ELSEWHERE CAN STILL SAY WHAT WAS KEPT** — register item 650, half ①, and
    /// the last field of it.
    ///
    /// # ⚠⚠⚠⚠⚠ What the four readers measured, and why only this one hurt
    ///
    /// Making a reported ending its own state made the compiler name every reader that weighs one:
    /// the row projection, [`cancel_sentence`], the durable log, and [`stand_down_sentence`]. Three
    /// answer at FULL STRENGTH for a reported ending, because what they read — the outcome WORD, its
    /// ceiling, the capture — is what `outcome_to_json` has always carried.
    ///
    /// This one does not. It asks *what became of the work*, whose answer is `Outcome::banked`, and
    /// that field was dropped by the render. So a person who stood a run down was told **the order
    /// was not honoured** and then, for an out-of-process run alone, *this ending cannot say what was
    /// kept* — where an in-process run says `the 3 turns it had already completed are BANKED`.
    ///
    /// ⚠⚠ Register item 604 is why that gap is a DEFECT and not a rough edge: the pair it measured
    /// is the one where a guess swaps the alarming answer for the relieved one, and *cannot say* was
    /// the only honest placeholder available. This closes it with the value rather than a guess.
    ///
    /// ⚠ And it needed no new type. `Banked` is a `Cow` and says why — *a run READ AFTER A RESTART
    /// hands over a word decoded from the daemon's log, and there is no `'static` to borrow it
    /// from* — so a word off the wire is a case the type already admits. That sentence was written
    /// for the durable log and paid for this too.
    #[test]
    fn a_run_that_ended_elsewhere_can_still_say_what_was_kept() {
        let banked = sprag_plugin::Banked {
            completed: 3,
            unit: std::borrow::Cow::Borrowed("turn"),
        };
        let ended = Outcome {
            state: OutcomeState::Exhausted(sprag_plugin::Ceiling::Iterations),
            iterations: 3,
            cost: None,
            failure: None,
            stopped: None,
            answered: 0,
            screened: 0,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: Some(banked),
            briefed: None,
        };

        // The sentence an IN-PROCESS run gets — the standard this reported one must meet.
        let here = stand_down_sentence(&RunState::Done {
            outcome: Box::new(ended.clone()),
            output: None,
        });
        assert!(
            here.contains("3 turns") && here.contains("BANKED"),
            "⚠ THE PREMISE: an in-process ending names the work it kept, or there is no standard \
             for the arm below to meet: {here:?}",
        );

        // The same ending, having crossed a process boundary through the daemon's own renderer.
        let there = stand_down_sentence(&RunState::Reported(Box::new(outcome_to_json(&ended))));
        assert!(
            there.contains("3 turns") && there.contains("BANKED"),
            "⚠⚠⚠⚠⚠ a person who stood a run down must be told what was KEPT whichever process drove \
             it — telling them only that the order was not honoured is register item 604's swap, \
             where the alarming half of the answer arrives without the relieving half: {there:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE CONTROL: A RUN THAT BANKED NOTHING MUST NOT BE TOLD IT BANKED SOMETHING.** The
    /// gate above passes for a renderer that hard-codes a reassuring clause. This drives the same
    /// door with `completed: 0` — a real answer and not an absence, as `Banked::completed`'s own doc
    /// says — and the two sentences must differ.
    #[test]
    fn a_reported_ending_that_banked_nothing_says_so() {
        let nothing = Outcome {
            state: OutcomeState::Exhausted(sprag_plugin::Ceiling::Iterations),
            iterations: 0,
            cost: None,
            failure: None,
            stopped: None,
            answered: 0,
            screened: 0,
            deliveries: sprag_plugin::Deliveries::NONE,
            checks: sprag_plugin::Checks::NONE,
            banked: Some(sprag_plugin::Banked {
                completed: 0,
                unit: std::borrow::Cow::Borrowed("turn"),
            }),
            briefed: None,
        };
        let said = stand_down_sentence(&RunState::Reported(Box::new(outcome_to_json(&nothing))));
        assert!(
            said.contains("completed nothing yet"),
            "⚠⚠⚠ a run that counted work and completed none says exactly that — a reassurance here \
             would be worse than the silence it replaced: {said:?}",
        );
    }

    /// **A RUN DRIVEN SOMEWHERE ELSE IS STILL WATCHABLE FROM HERE** — register item 650, half ②.
    ///
    /// # ⚠⚠⚠⚠⚠ The defect this is written against, and it is one I shipped
    ///
    /// `run_to_json` reads a running run's counters out of [`sprag_plugin::ProgressCell`], which is
    /// SHARED MEMORY with an in-process worker. A driver in another process writes its own cell over
    /// there, and register item 643's first driver was handed `ProgressCell::default()` — a cell
    /// nobody reads. So the row of an out-of-process run sits at zero for its whole life, and the
    /// two gates that shipped with it were both green because they asked about the ENDING.
    ///
    /// That is register item 492's shape (*a number authored and never read*), re-planted in the
    /// repository that keeps a memory file about it — and it is the difference between a supervised
    /// loop and a black box, which is the side of that line this project exists on.
    ///
    /// # ⚠⚠ Why the daemon is handed JSON and never a `Progress`
    ///
    /// [`sprag_plugin::Progress`] is built out of `&'static str` — `at`, and three per journal
    /// [`sprag_plugin::Edge`] — because it was only ever read in this process. Rebuilding one from
    /// the wire means interning every statechart word, whose vocabulary is upstream's.
    ///
    /// So nothing is rebuilt. The driver sends **what this daemon's own renderer produces**, the way
    /// `crate::drive::report` already sends `outcome_to_json`'s output rather than a shape spelled
    /// over there — one renderer, and the row cannot learn which kind of driver filled it.
    #[test]
    fn a_run_driven_somewhere_else_still_moves_its_row() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );

        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };

        // ⚠⚠ THE PREMISE, ASSERTED RATHER THAN ASSUMED. A row that already showed 4 would make
        // every claim below true with nothing having crossed — which is exactly how register item
        // 643's own gate was green while a read had been replaced by a constant.
        let before = row_of(&mut external, id);
        assert_eq!(
            before["state"]["iterations"],
            json!(0),
            "a run nobody has reported for shows nothing done: {before:?}",
        );

        external
            .invoke(
                REPORT_PROGRESS_ACTION,
                IntrospectValue::Json(json!({
                    RUN_ID_KEY: id,
                    PROGRESS_KEY: { "iterations": 4, "cost": 128, "unit": "bytes",
                                    RUN_ANSWERED_KEY: 1 },
                })),
            )
            .expect("a driver reporting its own run");

        let after = row_of(&mut external, id);
        assert_eq!(
            after["state"]["iterations"],
            json!(4),
            "⚠⚠⚠⚠⚠ a run whose driver is another process must still show what it has done — a row \
             frozen at zero for the run's whole life is the black box this feature must not \
             build: {after:?}",
        );
        assert_eq!(
            after["state"][RUN_ANSWERED_KEY],
            json!(1),
            "⚠⚠ and the answer tally most of all: it is the one a person watches to see a decision \
             being taken for them WHILE THERE IS STILL TIME TO CANCEL — `run_to_json` says so \
             itself: {after:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **THE CONTROL, AND IT IS WHAT MAKES THE GATE ABOVE MEAN ANYTHING.** A row that
    /// answered `4` from a constant satisfies every claim up there, and so does one that stored the
    /// first report and never replaced it. This sends a SECOND report through the same door: what a
    /// row shows is what its driver LAST said.
    #[test]
    fn a_row_shows_the_numbers_its_driver_last_sent() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        );
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };

        for iterations in [4, 9] {
            external
                .invoke(
                    REPORT_PROGRESS_ACTION,
                    IntrospectValue::Json(json!({
                        RUN_ID_KEY: id,
                        PROGRESS_KEY: { "iterations": iterations, RUN_ANSWERED_KEY: 1 },
                    })),
                )
                .expect("a driver reporting its own run");
        }

        let row = row_of(&mut external, id);
        assert_eq!(
            row["state"]["iterations"],
            json!(9),
            "⚠⚠⚠ the LATEST report is what a row shows — a first one that stuck would leave the \
             gate beside this proving only that something was stored once: {row:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN A DEAD DAEMON LEFT BEHIND COMES BACK ON A DRIVER OF THIS ONE'S** — register
    /// item 543's sixth brick, at the door a boot puts an inherited run through.
    ///
    /// # ⚠⚠⚠⚠⚠ What was true before this, and it was true for five rounds of building
    ///
    /// A loop could say where its machine was and be put back there; the place survived being
    /// written down as words; the words crossed a run log and came back only through a door that
    /// checked the document that wrote them; a plugin took them; and the datamodel crossed beside
    /// them so a resumed loop composes a real prompt. **Every one of those is green, and nothing
    /// called any of them at boot** — register item 492's shape (*authored and never read*) spread
    /// over five rounds. This is the gate that says something does.
    ///
    /// # ⚠⚠⚠⚠ What it measures, and what it deliberately leaves to the layer below
    ///
    /// It measures the WIRING: a run that came back `interrupted` from a predecessor's log is
    /// handed a driver again, under its own id, having been offered the place that log recorded.
    /// **That the machine honours those words is not asserted here** — that is
    /// `sprag_plugin`'s `a_resumed_loop_is_placed_rather_than_walked_and_does_not_re_open_with_its_prompt`
    /// and `a_place_carries_the_words_a_resumed_loop_cannot_compose_for_itself`, which drive a loop
    /// against a stand-in peer and can see it. The two together are the chain; neither alone is.
    ///
    /// # ⚠⚠⚠ Four controls, because each half of the pair has its own way of being vacuous
    ///
    /// * a place from ANOTHER build's documents is not inherited at all (or the fingerprint is
    ///   decoration and a restart would enter a run into a document it never ran in);
    /// * a log with a place and NO request is not inherited (or *the request crossed* is a claim
    ///   about a field nothing reads);
    /// * a place this build cannot spell is REFUSED at the door and the row stays `interrupted` (or
    ///   `put_back` never reads the place, and a resume is a restart from the top wearing its name);
    /// * a daemon whose drivers are PROCESSES resumes too, and the place reaches the child on its
    ///   request — while the same key from a CLIENT is stripped (register items 543 / 662). The
    ///   pair is one claim: without the second half, *the daemon may say where a run starts* and
    ///   *anybody may* are the same green.
    #[test]
    fn a_run_a_dead_daemon_left_behind_is_put_back_on_a_driver_of_this_ones() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        // ⚠ ONE ITERATION: what this gate measures happens before the first step either way, and a
        // driver left running past the assertions is a thread typing into a dropped fixture.
        let asked = ai_loop_request(
            pane,
            json!({ "guardrails": { "max_iterations": 1, "max_seconds": 5 } }),
        );
        let asked = asked.as_object().expect("a request is an object").clone();

        // ⚠⚠ THE PLACE A REAL LOOP SAYS IT IS IN, asked of the product's own builder over this very
        // request. A fixture that spelled state names here would round-trip its own invention and
        // say nothing about whether a boot can carry what a loop actually produces.
        let aside = Arc::new(Mutex::new(RunRegistry::default()));
        let building =
            PluginsExternal::new(Arc::clone(&workspace), aside, None, None, None, None, None);
        let (mut built, label) =
            plugin_from_request(&building, &asked).expect("the shipped document builds a loop");
        // ⚠⚠⚠⚠⚠ **IT IS STEPPED FIRST, AND THAT IS THE FIXTURE'S OWN HONESTY RATHER THAN A
        // CEREMONY.** A machine that has been CONSTRUCTED and never driven is not in a settled
        // configuration — measured on the round that wrote this: its `place` names a compound state
        // as current, and `enter_at` refuses it (*"not somewhere a settled machine stops"*). Nothing
        // in the product ever writes such a place down (`Driver` asks the plugin AFTER each step,
        // so a log carries only settled ones), so a fixture that saved one would be measuring this
        // gate against a record no daemon can produce.
        built
            .as_plugin()
            .step(
                &sprag_plugin::WorkspacePaneAccess::new(Arc::clone(&workspace)),
                &sprag_plugin::RunContext::uncancellable(),
            )
            .expect("a live pane takes a pass");
        let place = built
            .as_plugin()
            .place()
            .expect("an ai_loop says where its machine is");

        let here = sprag_plugin::STATECHARTS_FINGERPRINT;
        let saved = |place: Option<Vec<String>>,
                     document: Option<&str>,
                     request: Option<Map<String, Value>>| {
            crate::runs::RunLog {
                version: crate::runs::RUN_LOG_VERSION,
                runs: vec![crate::runs::PersistedRun {
                    id: 7,
                    label: label.clone(),
                    request,
                    iterations: 4,
                    cost: None,
                    unit: None,
                    finished: false,
                    outcome: None,
                    ceiling: None,
                    output: None,
                    build: None,
                    driver: None,
                    opened_by_session: None,
                    at: None,
                    document: document.map(str::to_owned),
                    stood_down: None,
                    cancelled_by: None,
                    deliveries: None,
                    banked: None,
                    briefed: None,
                    place,
                }],
            }
        };
        // ⚠ A SUCCESSOR PER CASE: its own registry, restored from its own predecessor's log, which
        // is what a boot is. Sharing one would let an earlier case's row answer a later one.
        let inheriting = |log: &crate::runs::RunLog| {
            let runs = Arc::new(Mutex::new(RunRegistry::default()));
            lock(&runs).restore(log);
            runs
        };
        let over = |runs: &Arc<Mutex<RunRegistry>>| {
            PluginsExternal::new(
                Arc::clone(&workspace),
                Arc::clone(runs),
                None,
                None,
                None,
                None,
                None,
            )
        };

        // ── CONTROL 1: a place from ANOTHER build's documents is not inherited ───────────────
        //
        // ⚠⚠ THE FINGERPRINTS REALLY DIFFER, asserted rather than assumed: a fixture whose foreign
        // word happened to equal this image's would make every claim below vacuously true.
        assert_ne!(
            "0000000000000000",
            sprag_plugin::STATECHARTS_FINGERPRINT,
            "⚠⚠ THE FIXTURE'S OWN PREMISE: the foreign fingerprint must not be this build's",
        );
        let foreign = inheriting(&saved(
            Some(place.clone()),
            Some("0000000000000000"),
            Some(asked.clone()),
        ));
        assert!(
            lock(&foreign).inheritance().resumed.is_empty(),
            "⚠⚠⚠ A CONTROL FAILED: a run whose place was recorded against a document this build \
             does not have was offered for resuming. Nothing migrates a configuration between \
             documents, so putting that run back would enter it into a document it never ran in — \
             register item 544's whole finding.",
        );
        // ⛔⛔⛔⛔⛔ AND IT IS NAMED RATHER THAN DROPPED — register item 737. An empty `resumed` on
        // its own is the same answer a predecessor with no runs at all produces, and a promotion is
        // usually a DOCUMENT change, so this is the common way for a boot to inherit nothing.
        assert_eq!(
            lock(&foreign).inheritance().withheld,
            vec![crate::runs::WithheldRun {
                id: crate::runs::RunId(7),
                label: label.clone(),
                why: crate::runs::Withheld::ForeignDocuments {
                    theirs: "0000000000000000".to_owned()
                },
                // ⚠ THE LOG RECORDED NONE, which is this fixture's own record rather than a
                // property of withholding: a restored run's handle answers whatever the file said.
                driver: None,
            }],
            "⛔⛔⛔ REGISTER ITEM 737: a boot that puts none of a predecessor's runs back must say \
             WHICH runs stayed behind and why. Silence here is what let a promotion discard every \
             loop on the machine while the only thing anybody could read was `interrupted`.",
        );

        // ── CONTROL 2: a place with no request is not inherited ──────────────────────────────
        let placeless = inheriting(&saved(Some(place.clone()), Some(here), None));
        assert!(
            lock(&placeless).inheritance().resumed.is_empty(),
            "⚠⚠⚠ A CONTROL FAILED: a run with a place and nothing to rebuild its plugin from was \
             offered for resuming. If this passes while the claim below does too, *the request \
             crossed the log* is a statement about a field nobody reads.",
        );
        // ⚠⚠⚠ AND ITS REASON IS THE OTHER ONE — register item 737. The two refusals arrive at the
        // same empty list and send a reader to opposite places: a foreign fingerprint is somebody's
        // promotion, and a missing request is a predecessor that never wrote one down.
        assert_eq!(
            lock(&placeless)
                .inheritance()
                .withheld
                .first()
                .map(|run| run.why.clone()),
            Some(crate::runs::Withheld::NoRequest),
            "⛔⛔⛔ REGISTER ITEM 737: a run held back for want of a REQUEST was reported under \
             another reason, or not at all — which sends whoever reads it to look at documents \
             that are perfectly fine.",
        );

        // ── CONTROL 3: a place this build cannot spell is refused, and the row does not move ──
        //
        // ⚠⚠⚠⚠ THE FORGERY IS SHAPED LIKE A REAL RECORD — every word but the head is the loop's
        // own — for the reason item 543's own rounds measured twice: a single junk word is refused
        // for the WRONG reason (a head that is not among the states it names), so it stays green
        // against a door that has stopped reading names at all. ⚠ It cannot occur naturally, since
        // a matching fingerprint means matching documents; that is exactly why it is constructed.
        let mut forged = place.clone();
        forged[0] = "a-state-no-document-in-this-build-has".to_owned();
        let unreadable = inheriting(&saved(Some(forged), Some(here), Some(asked.clone())));
        let offered = lock(&unreadable).inheritance().resumed;
        assert_eq!(
            offered.len(),
            1,
            "⚠⚠ THE CONTROL'S OWN PREMISE: the record has both halves, so the registry must offer \
             it and the refusal below must come from the door rather than from the offering.",
        );
        let mut external = over(&unreadable);
        let refused = external.put_back(&offered[0]);
        assert!(
            refused.is_err(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a place this build cannot read was SWALLOWED and the run \
             was put back anyway — which means put back AT THE TOP, firing every `<onentry>` on the \
             way down and re-typing the loop's opening prompt into somebody's pane. A door that \
             never reads the place answers `Ok` here exactly as it does below.",
        );
        assert_eq!(
            row_of(&mut external, 7)["state"]["status"],
            json!(RunStatus::Interrupted.wire_str()),
            "⚠⚠⚠ AND THE ROW MUST NOT HAVE MOVED: a refusal that still swapped the driver would \
             leave a row claiming to run with nothing driving it, which is strictly worse than the \
             ending it already had.",
        );

        // ── THE OTHER DRIVER KIND, AND THE STRIP THAT GOES WITH IT ───────────────────────────
        //
        // ⚠⚠⚠⚠⚠ **A DAEMON THAT DRIVES RUNS IN PROCESSES OF THEIR OWN PUTS ITS INHERITED ONES
        // BACK THE SAME WAY IT STARTS FRESH ONES** — register items 543 / 662. What the child is
        // handed is captured, because the place is the whole point: the daemon writes onto the
        // request the words its own log carried, and that key is the one `drive_request` reads
        // before the first step.
        //
        // ⚠⚠⚠⚠ **AND THE PAIR IS THE CLAIM**: the same key from a CLIENT is stripped
        // (`spawn_driven_run`), so a strip nobody measured is measured here beside the write that
        // makes it matter. Without the second half, *the daemon may say where a run starts* and
        // *anybody may* are the same green.
        let handed: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::new(Mutex::new(Vec::new()));
        let spawning = {
            let handed = Arc::clone(&handed);
            move |_: RunId, request: &Map<String, Value>| {
                lock(&handed).push(request.clone());
                std::process::Command::new("cat")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            }
        };
        let elsewhere = inheriting(&saved(Some(place.clone()), Some(here), Some(asked.clone())));
        let offered = lock(&elsewhere).inheritance().resumed;
        let mut out_of_process = over(&elsewhere).driving_out_of_process(Arc::new(spawning));
        out_of_process
            .put_back(&offered[0])
            .expect("a daemon whose drivers are processes can put an inherited run back too");
        assert_ne!(
            row_of(&mut out_of_process, 7)["state"]["status"],
            json!(RunStatus::Interrupted.wire_str()),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 662: the driver kind that can actually READ a place is the one \
             whose inherited run stayed dead.",
        );
        // ⚠⚠ TAKEN OUT FROM UNDER THE LOCK BEFORE IT IS ASSERTED ON. A `std::sync::Mutex` is not
        // reentrant and an `assert_eq!` holds its operands' temporaries while it formats the
        // failure message — so locking in both places deadlocks on exactly the run that FAILS,
        // which is a gate that hangs instead of reporting. Measured here.
        let started = lock(&handed).clone();
        assert!(
            !started.is_empty(),
            "⚠⚠ the fixture's premise: a driver was started, so `started[0]` is the request this \
             resume handed it.",
        );
        // ⚠⚠⚠ IT USED TO SAY *EXACTLY ONE*, AND THAT STOPPED BEING THE FIXTURE'S PREMISE — register
        // item 671. `wait_with_output` closes the child's stdin, so this stand-in `cat` ends at
        // once with nothing reported, and a driver that dies without an outcome is now PUT BACK on
        // a new one. So a second start is the product working, and the first is still the resume's.
        // The count is not what this arm measures; the place on `started[0]` is.
        assert_eq!(
            started[0].get(RUN_PLACE_KEY),
            Some(&json!(place)),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: the child was started WITHOUT the place its daemon had \
             just read out of a run log. `drive_request` reads that key before the first step, so \
             a child that never receives it starts the loop AT THE TOP — firing every `<onentry>` \
             and re-typing the opening prompt into somebody's pane. Handed: {:?}",
            started[0],
        );

        // ── AND THE STRIP: a CLIENT saying the same word is not obeyed ───────────────────────
        // ⚠⚠⚠ A RECORDER OF ITS OWN, AND THE SHARED ONE COST A RED TO LEARN — register item 671.
        // Both arms used to push into `handed` and this half read `both[1]`, which is a POSITION
        // rather than a fact: the moment a dead driver started being put back (item 671) the entry
        // at that index could be the resume's rescue instead of the client's run, and the strip
        // this measures would be asserted against the wrong request.
        let by_a_client: Arc<Mutex<Vec<Map<String, Value>>>> = Arc::new(Mutex::new(Vec::new()));
        let mut client = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::new(Mutex::new(RunRegistry::default())),
            None,
            None,
            None,
            None,
            None,
        )
        .driving_out_of_process(Arc::new({
            let handed = Arc::clone(&by_a_client);
            move |_: RunId, request: &Map<String, Value>| {
                lock(&handed).push(request.clone());
                std::process::Command::new("cat")
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .spawn()
            }
        }));
        let mut asking = asked.clone();
        asking.insert(RUN_PLACE_KEY.to_owned(), json!(place));
        client
            .invoke(RUN_ACTION, IntrospectValue::Json(Value::Object(asking)))
            .expect("a run a client asks for, with a word it is not entitled to say");
        let both = lock(&by_a_client).clone();
        assert_eq!(
            both.len(),
            1,
            "the fixture's premise: the client's run started a driver of its own, and a run whose \
             place was stripped records none — so nothing puts it back and this stays at one",
        );
        assert_eq!(
            both[0].get(RUN_PLACE_KEY),
            None,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: a CLIENT said where a run should start and the daemon \
             passed it on. This wire swallows arguments it does not publish, so that is an \
             unpublished verb — *begin this loop already at `judging`* — and the only thing \
             entitled to say it is a boot repeating what its own log recorded. Handed: {:?}",
            both[0],
        );

        // ── THE CLAIM: the run this daemon inherited is driven again, under its own id ────────
        let runs = inheriting(&saved(Some(place.clone()), Some(here), Some(asked)));
        let inheritance = lock(&runs).inheritance();
        // ⚠⚠ AND NOTHING IS HELD BACK ON THE ARM THAT COMES THROUGH — register item 737's control.
        // A reporter that named every restored run as withheld would satisfy the two assertions
        // above while saying nothing at all.
        assert!(
            inheritance.withheld.is_empty(),
            "⚠⚠⚠ A CONTROL FAILED: a run whose place and request BOTH crossed the log was reported \
             as staying behind, so `withheld` is not reading the refusal — it is a second name for \
             *restored*.",
        );
        let offered = inheritance.resumed;
        assert_eq!(offered.len(), 1, "the log's one resumable run is offered");
        assert_eq!(offered[0].id.0, 7, "and it keeps the id its log gave it");
        assert_eq!(
            offered[0].place, place,
            "⚠⚠ and the words offered are the ones the log held, not a re-derivation of them",
        );
        let mut external = over(&runs);
        external
            .put_back(&offered[0])
            .expect("a run whose place and request both survived is one this daemon can put back");
        let row = row_of(&mut external, 7);
        assert_ne!(
            row["state"]["status"],
            json!(RunStatus::Interrupted.wire_str()),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 543: the daemon read a place, read a request, built the plugin, \
             placed the machine — and the run is still `interrupted`. A restart that can resume and \
             does not is the same dead run with more code behind it. Row: {row:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN DRIVEN IN ANOTHER PROCESS RECORDS WHERE IT GOT TO, AND WHERE ITS MACHINE
    /// IS** — register item 662.
    ///
    /// # ⚠⚠⚠⚠⚠ What was measured, and why it is a regression rather than a gap
    ///
    /// [`crate::runs::RunRegistry::persistable`] reads `iterations`, `cost`, `at`, `place`,
    /// `deliveries` and `banked` out of the run's [`sprag_plugin::ProgressCell`] — and for a run
    /// whose driver is another process that cell **never moves**: `spawn_driven_run` files a
    /// `ProgressCell::default()` and says so in its own comment, and the driver's counters arrive
    /// by [`REPORT_PROGRESS_ACTION`] into a separate slot. So such a run's durable record was
    /// `iterations: 0, place: None` however long it ran.
    ///
    /// **The row had already decided the other way.** `run_to_json` prefers the report over the
    /// cell and its own comment argues why (register item 650). So one daemon answered the same
    /// question two ways depending on who asked — and the FILE, the answer that outlives the
    /// process, was the one that was wrong. Item 606 bought exactly this lesson (*a run is read
    /// after it ends, and the daemon that drove it is gone*); this is that lesson undone for one
    /// kind of driver.
    ///
    /// ⚠⚠ **AND IT IS WHAT STOPS ITEM 543 REACHING THE DRIVER THAT READS PLACES.** The out-of-
    /// process driver is the one `drive_request` teaches to resume; with no place in its log there
    /// was nothing for a boot to hand it, so `PluginsExternal::put_back` refused that daemon
    /// outright.
    ///
    /// # ⚠⚠⚠ Two controls, because "prefer the report" has two ways of being wrong
    ///
    /// * a report that says nothing about a place must leave the log saying nothing — an older
    ///   driver's report has no such key, and inventing one would place a machine nobody chose;
    /// * a run whose CELL moved and that has no report must still persist the cell — the arm that
    ///   already worked, which a naive *always read the report* would silently zero.
    #[test]
    fn a_run_driven_somewhere_else_records_where_it_got_to() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        // ⚠ THE PRODUCT'S OWN OUT-OF-PROCESS PATH, so the empty cell under test is the one
        // `spawn_driven_run` really files. The child is a `cat` whose stdin is dropped, so it sees
        // EOF and goes — this gate is about what the daemon RECORDS, not about a driver.
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        )
        .driving_out_of_process(Arc::new(|_, _| {
            std::process::Command::new("cat")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        }));

        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };

        // ⚠⚠ THE PREMISE, ASSERTED RATHER THAN ASSUMED: this run's cell is empty and stays empty,
        // so everything the log carries below can only have come from the report.
        let empty = lock(&registry).persistable();
        assert_eq!(
            (empty.runs[0].iterations, empty.runs[0].place.as_deref()),
            (0, None),
            "⚠⚠ the fixture's premise: a run whose driver is elsewhere starts with an empty cell, \
             or 'the report is what got there' is a claim about a value the cell supplied",
        );

        // ── A DRIVER SAYS WHERE IT IS, through the door a driver really uses ─────────────────
        let place = json!(["judging", "running", "work", "judging"]);
        external
            .invoke(
                REPORT_PROGRESS_ACTION,
                IntrospectValue::Json(json!({
                    RUN_ID_KEY: id,
                    PROGRESS_KEY: progress_to_json(&sprag_plugin::Progress {
                        iterations: 7,
                        cost: Some(sprag_plugin::Cost::Bytes(512)),
                        at: Some("judging"),
                        place: Some(
                            place
                                .as_array()
                                .expect("a list")
                                .iter()
                                .map(|word| word.as_str().expect("a word").to_owned())
                                .collect(),
                        ),
                        ..sprag_plugin::Progress::default()
                    }),
                })),
            )
            .expect("a driver reporting its own run");

        // ── THE CLAIM: the file says what the driver said ────────────────────────────────────
        let log = lock(&registry).persistable();
        let saved = &log.runs[0];
        assert_eq!(
            saved.iterations, 7,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 662: a run driven in another process persisted `iterations: \
             {}` while its driver had reported 7. The row prefers the report and the FILE did not, \
             so the answer that outlives the daemon is the one that is wrong.",
            saved.iterations,
        );
        assert_eq!(
            saved.cost.zip(saved.unit.as_deref()),
            Some((512, "bytes")),
            "⚠⚠ and what it SPENT, which is the number a person reads a dead run's record for",
        );
        assert_eq!(
            saved.resumable_place().map(<[String]>::to_vec),
            Some(
                place
                    .as_array()
                    .expect("a list")
                    .iter()
                    .map(|word| word.as_str().expect("a word").to_owned())
                    .collect::<Vec<_>>()
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 662, AND ITEM 543 RIDES ON IT: the out-of-process driver is \
             the one `drive_request` teaches to resume, and its run log carried NO PLACE — so a \
             boot had nothing to hand the one driver that could take it. Saved: {saved:?}",
        );
        assert_eq!(
            saved.at.as_deref(),
            Some("judging"),
            "⚠⚠ and the word a PERSON reads (*was it mid-turn, or waiting on me?*), which is the \
             half of the pair `resumable_here` answers",
        );

        // ── CONTROL 1: a report that says nothing about a place invents nothing ──────────────
        external
            .invoke(
                REPORT_PROGRESS_ACTION,
                IntrospectValue::Json(json!({
                    RUN_ID_KEY: id,
                    PROGRESS_KEY: { "iterations": 8 },
                })),
            )
            .expect("an older driver reporting only what it knows about");
        let older = lock(&registry).persistable();
        assert_eq!(
            (older.runs[0].iterations, older.runs[0].place.as_deref()),
            (8, None),
            "⚠⚠⚠ A CONTROL FAILED: a report with no place left a place in the record. A driver \
             built before this key existed says nothing about where its machine is, and a daemon \
             that filled that in from anywhere would put a restarted run somewhere nobody chose — \
             worse than the honest `interrupted` it gets today. Wrote: {:?}",
            older.runs[0],
        );

        // ── CONTROL 2: the arm that already worked still works ───────────────────────────────
        //
        // ⚠⚠⚠⚠ WITHOUT THIS, *prefer the report* is indistinguishable from *read only the report*,
        // and the second silently zeroes every run this daemon drives on a thread of its own —
        // which since 2026-08-24 is a daemon told `RUN_DRIVER_PROCESS = off`, the way back from
        // the new default and therefore the arm that must not rot.
        let cell = sprag_plugin::ProgressCell::default();
        {
            let mut moving = lock(&cell);
            moving.iterations = 3;
            moving.place = Some(vec!["working".to_owned(), "work".to_owned()]);
        }
        let in_process = {
            let mut held = lock(&registry);
            let id = held.reserve();
            held.submit(crate::runs::NewRun {
                id,
                label: "ai_loop pane=0".to_owned(),
                plugin: PluginName::AiLoop,
                request: None,
                opened_by: None,
                opened_by_session: None,
                state: Arc::new(Mutex::new(RunState::Running)),
                run: Box::new(crate::runs::EndedRun::restored(false, None, None)),
                progress: Arc::clone(&cell),
            })
        };
        let both = lock(&registry).persistable();
        let mine = both
            .runs
            .iter()
            .find(|run| run.id == in_process.0)
            .expect("the second run is in the log");
        assert_eq!(
            (mine.iterations, mine.place.as_deref().map(<[String]>::len)),
            (3, Some(2)),
            "⚠⚠⚠ A CONTROL FAILED: a run whose driver shares its cell and has reported nothing \
             lost what that cell said. There is no report to prefer, so the cell IS the answer — \
             and this is the arm every run takes under the default configuration. Wrote: {mine:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A RUN DRIVEN IN ANOTHER PROCESS SHOWS WHAT IT PUT INTO ITS PANE, WHAT ITS CHECKS
    /// CAME TO, WHICH PANE IT IS DRIVING, AND WHAT IT BANKED** — register item 663.
    ///
    /// # ⚠⚠⚠⚠⚠ What item 662 left, and why it left it deliberately
    ///
    /// 662 taught the durable log to prefer the driver's report over a cell that never moves, and
    /// carried `at` and `place` down that channel. It stopped there on purpose: those two are
    /// rendered NOWHERE ELSE, so carrying them created no second spelling. These five are not like
    /// that — `run_to_json` publishes the delivery triple (item 617), the checks sentence (601) and
    /// the driven pane (540) BESIDE the state, out of the cell, and `persistable` writes
    /// `deliveries` (606) and `banked` (616) from the same cell. For a run whose driver is another
    /// process that cell is all zeros for ever, so **every one of those five was missing from both
    /// the row and the file** — item 606's own finding (*a run is read after it ends, when its
    /// daemon is gone*) undone for one kind of driver.
    ///
    /// # ⚠⚠⚠⚠ The fix is not "add keys to the report", and the first control says why
    ///
    /// Adding them to [`progress_to_json`] alone would put them inside `state` while the cell-fed
    /// copies stayed beside it: one number, two places, and for in-process runs only — the
    /// invisible divergence [`crate::options::RUN_DRIVER_PROCESS`] promises cannot happen, plus
    /// register item 445's two-authorities shape. So the report is a **TRANSPORT**: what it carries
    /// is taken OUT of it and published once, in the place the row already publishes it. The first
    /// control is what holds that.
    ///
    /// ⚠ Three controls: a fact is published exactly ONCE; the in-process arm (what a daemon told
    /// `RUN_DRIVER_PROCESS = off` takes) still reads its cell; and an older driver's report, which
    /// knows none of these keys, leaves the row saying nothing rather than publishing zeros nobody
    /// reported.
    #[test]
    fn a_run_driven_somewhere_else_shows_what_it_delivered_and_banked() {
        let workspace = Arc::new(Mutex::new(Workspace::new((80, 24))));
        let pane = echoing_agent_pane(&workspace);
        let registry = Arc::new(Mutex::new(RunRegistry::default()));
        let mut external = PluginsExternal::new(
            Arc::clone(&workspace),
            Arc::clone(&registry),
            None,
            None,
            None,
            None,
            None,
        )
        .driving_out_of_process(Arc::new(|_, _| {
            std::process::Command::new("cat")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        }));
        let started = external
            .invoke(
                RUN_ACTION,
                IntrospectValue::Json(ai_loop_request(pane, json!({}))),
            )
            .expect("a well-formed ai_loop run");
        let IntrospectValue::Int(id) = started else {
            panic!("a run answers its id: {started:?}");
        };

        // ── CONTROL 3 FIRST: an older driver knows none of these keys ────────────────────────
        //
        // ⚠⚠ IT GOES FIRST so a later pass cannot be what satisfies it, and because it is the
        // shape a REAL older driver has: the two images are the same binary only until somebody
        // promotes one.
        external
            .invoke(
                REPORT_PROGRESS_ACTION,
                IntrospectValue::Json(json!({
                    RUN_ID_KEY: id,
                    PROGRESS_KEY: { "iterations": 1 },
                })),
            )
            .expect("an older driver reporting only what its build knew");
        let older = row_of(&mut external, id);
        assert!(
            older.get(RUN_DELIVERED_KEY).is_none()
                && older.get(RUN_CHECKS_KEY).is_none()
                && older.get(RUN_BRIEFED_KEY).is_none()
                && older.get(RUN_DRIVING_KEY).is_none(),
            "⚠⚠⚠ A CONTROL FAILED: a report that mentioned none of these left the row publishing \
             them anyway. Absence must keep meaning *nobody said* — a row that answered `0 of 0` \
             for a driver that never counted would be this daemon asserting something no process \
             wrote down. Row: {older:?}",
        );
        // ⚠⚠⚠⚠⚠ **AND THE READER IS ASKED DIRECTLY, BECAUSE THE ROW ABOVE CANNOT TELL TWO THINGS
        // APART.** *Fell back to an empty cell* and *read the silence as zeros* produce the same
        // row here, since this run's cell is empty either way — so the assertion above is true of
        // a reader that has stopped distinguishing them. What the rule actually says is about the
        // DOOR: a missing `beside` yields [`None`], never a value nobody reported, and that is
        // what lets the caller fall back to a cell that DOES hold something (control 2).
        let silence = progress_from_report(&json!({ "iterations": 1 }));
        assert_eq!(
            (
                silence.deliveries,
                silence.checks,
                silence.driving,
                silence.banked,
                silence.briefed
            ),
            (None, None, None, None, None),
            "⚠⚠⚠ A CONTROL FAILED AT THE DOOR: a report carrying none of these was read as though \
             it carried zeros. `None` here is what makes the caller's fallback reachable — with a \
             value in its place, an older driver's silence would overwrite a cell that knew the \
             answer.",
        );

        // ── A DRIVER SAYS WHAT IT HAS DONE, through the door a driver really uses ────────────
        external
            .invoke(
                REPORT_PROGRESS_ACTION,
                IntrospectValue::Json(json!({
                    RUN_ID_KEY: id,
                    PROGRESS_KEY: progress_to_json(&sprag_plugin::Progress {
                        iterations: 9,
                        // ⚠ ONE STEP, because the claim is that a WALK crosses at all — item 544's
                        // default flip found this one empty for every out-of-process run.
                        journal: vec![sprag_plugin::StepRecord {
                            iteration: 1,
                            cost: sprag_plugin::Cost::Bytes(3),
                            verdict: sprag_plugin::Verdict::Continue,
                            note: Some("A-STEP-THIS-DRIVER-TOOK".to_owned()),
                            walked: Vec::new(),
                        }],
                        deliveries: sprag_plugin::Deliveries {
                            made: 5,
                            folded: 2,
                            unsubmitted: 1,
                        },
                        checks: sprag_plugin::Checks {
                            asked: 3,
                            silent: 2,
                            why_silent: Some("THE-CHECKER-NEVER-ANSWERED".to_owned()),
                            // ⚠ DISTINCT FROM EVERY NUMBER BESIDE THEM — register item 499, on
                            // this gate's own terms: a value equal to a neighbour's would let a
                            // transport that dropped one key and duplicated another still pass.
                            refused: 7,
                            refused_in_a_row: 6,
                            // ⚠ AND DISTINCT AGAIN, on the same terms — register item 674.
                            unasked: 9,
                        },
                        driving: Some(pane),
                        banked: Some(sprag_plugin::Banked {
                            completed: 4,
                            unit: std::borrow::Cow::Borrowed("turn"),
                        }),
                        // ⚠⚠ REGISTER ITEM 719's SECOND DIRECTION, on this gate's terms exactly:
                        // it is a level fed from the cell for an in-process run, so a driver in
                        // another process would publish nothing about it unless the transport
                        // carries it — and *what was that run handed?* is asked about runs that
                        // are over, which is when the cell is all zeros.
                        briefed: Some(sprag_plugin::Briefing {
                            north_star: 41,
                            milestone: 1_984,
                            reference: 7_000,
                        }),
                        ..sprag_plugin::Progress::default()
                    }),
                })),
            )
            .expect("a driver reporting its own run");

        // ── THE CLAIM, HALF ONE: the ROW shows it ────────────────────────────────────────────
        let row = row_of(&mut external, id);
        assert_eq!(
            (
                row.get(RUN_DELIVERED_KEY),
                row.get(RUN_FOLDED_KEY),
                row.get(RUN_UNSUBMITTED_KEY),
            ),
            (Some(&json!(5)), Some(&json!(2)), Some(&json!(1))),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 663: a run driven in another process published nothing about \
             what it put into its pane. Item 591 built those counters and item 617 made them the \
             only thing that explains a pane holding an unsubmitted prompt — and they were read \
             off a cell that never moves for this kind of run. Row: {row:?}",
        );
        let said = row
            .get(RUN_CHECKS_KEY)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(
            said.contains("THE-CHECKER-NEVER-ANSWERED"),
            "⛔⛔⛔⛔ REGISTER ITEM 663 / 601: what this run's checks came to did not reach a \
             reader. That sentence is the one that says an ending rests on the working agent's own \
             word, and it is composed HERE from what the driver reported. Said: {said:?}",
        );
        // ⛔⛔⛔⛔⛔ **AND WHAT THE VERDICTS CAME TO CROSSES THE SAME WIRE** — register item 499.
        // The tally's other two numbers are new keys on an answer, which is the change this
        // surface's pin does not number; what that costs is that a transport dropping them fails
        // SILENTLY, because a row missing a clause reads exactly like a run nothing refused. So
        // the numbers are asserted at the ROW, past the report, the wire and the reader — and
        // they are distinct from every count beside them (7 and 6 against 5, 2 and 1) so a hop
        // that carried the wrong field still cannot pass.
        assert!(
            said.contains("7 of them the checker refused") && said.contains("6 in a row"),
            "⛔⛔⛔⛔ REGISTER ITEM 663 / 499: how often this run's claims were REFUSED, and how \
             deep the refusals ran, did not reach a reader. That depth is the only number that \
             says whether `reflect_after_refusals` was ever approached — the question the ceiling \
             was authored without, and the one nothing outside a bounded walk could answer. \
             Said: {said:?}",
        );
        assert_eq!(
            row.get(RUN_DRIVING_KEY),
            Some(&json!(pane.0)),
            "⛔⛔⛔⛔ REGISTER ITEM 663 / 540: the row did not say which pane this run is driving, \
             so a person looking at a busy pane cannot find out what is typing into it",
        );
        let briefed = row
            .get(RUN_BRIEFED_KEY)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        assert!(
            briefed.contains("9025") && briefed.contains("reference") && briefed.contains("2,816"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 663 / 719: what the door ACCEPTED did not reach a reader. \
             `orchestrate` answers a run id and points at this row, so a size that dies here is \
             item 719's second direction unpaid — the caller who wrote 9,025 bytes still has no \
             way to learn it, and the caveat that stops them reading a small number as a safe one \
             goes with it. Said: {briefed:?}",
        );
        assert!(
            row[RUN_JOURNAL_KEY].as_array().is_some_and(
                |walk| walk.len() == 1 && walk[0]["note"] == json!("A-STEP-THIS-DRIVER-TOOK")
            ),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 544: a run driven in another process published an EMPTY walk. \
             Nine tests read what a run did step by step, and the day the option's default flipped \
             every one of them was reading a journal that stops at the process boundary. Walk: {:?}",
            row[RUN_JOURNAL_KEY],
        );

        // ── THE CLAIM, HALF TWO: the FILE keeps it ───────────────────────────────────────────
        let log = lock(&registry).persistable();
        let saved = &log.runs[0];
        assert_eq!(
            saved.deliveries,
            Some(crate::runs::PersistedDeliveries {
                made: 5,
                folded: 2,
                unsubmitted: 1,
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 663 / 606: this is the column item 606 was filed for, and it \
             came back empty for the driver kind that fills it from a report. A run is READ after \
             it ends, when the daemon that drove it is already gone. Saved: {saved:?}",
        );
        assert_eq!(
            saved.banked,
            Some(crate::runs::PersistedBanked {
                completed: 4,
                unit: "turn".to_owned(),
            }),
            "⛔⛔⛔⛔ REGISTER ITEM 663 / 616: how much of the work was kept is what a stand-down \
             sentence weighs, and it did not survive. Saved: {saved:?}",
        );
        assert_eq!(
            saved.briefed,
            Some(crate::runs::PersistedBriefing {
                north_star: 41,
                milestone: 1_984,
                reference: 7_000,
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 663 / 719: how big the brief was did not survive the daemon \
             that took it — and this level needs the file MORE than the two above, because *what \
             was that run handed?* is asked almost only about a run that is already over. Item \
             606's measurement is the general form: thirteen live runs, every one restored. \
             Saved: {saved:?}",
        );

        // ── CONTROL 1: a fact is published EXACTLY ONCE ──────────────────────────────────────
        //
        // ⚠⚠⚠⚠⚠ **IT ASKS FOR THE TRANSPORT WRAPPER BY NAME, AND A GREEN MUTATION IS WHY.** The
        // first version of this control looked for the FLAT keys inside `state` — the shape a
        // flat-carried report would have leaked — and deleting the strip left it green, because
        // what actually leaks is the NESTED object: `state.beside.delivered`, which no assertion
        // about `state.delivered` can see. A control written for the design that was rejected is a
        // control for nothing. Both are asked now: the wrapper must be gone, AND no flat copy may
        // have been unpacked into its place.
        assert!(
            row["state"].get(REPORTED_BESIDE_KEY).is_none()
                && row["state"].get(RUN_DELIVERED_KEY).is_none()
                && row["state"].get(RUN_DRIVING_KEY).is_none(),
            "⚠⚠⚠⚠⚠ A CONTROL FAILED, AND IT IS THE ONE THAT SHAPES THE FIX: the report is a \
             TRANSPORT, not a fragment of the row. Splatting it into `state` while the same facts \
             are published beside the state puts one number in two places — and only for one kind \
             of driver, which is the invisible divergence `RUN_DRIVER_PROCESS` promises cannot \
             happen. State: {:?}",
            row["state"],
        );

        // ── CONTROL 2: the arm every run takes by default is untouched ───────────────────────
        let cell = sprag_plugin::ProgressCell::default();
        {
            let mut moving = lock(&cell);
            moving.deliveries = sprag_plugin::Deliveries {
                made: 7,
                folded: 0,
                unsubmitted: 0,
            };
            moving.driving = Some(pane);
            moving.banked = Some(sprag_plugin::Banked {
                completed: 6,
                unit: std::borrow::Cow::Borrowed("turn"),
            });
        }
        let in_process = {
            let mut held = lock(&registry);
            let next = held.reserve();
            held.submit(crate::runs::NewRun {
                id: next,
                label: "ai_loop pane=0".to_owned(),
                plugin: PluginName::AiLoop,
                request: None,
                opened_by: None,
                opened_by_session: None,
                state: Arc::new(Mutex::new(RunState::Running)),
                run: Box::new(crate::runs::EndedRun::restored(false, None, None)),
                progress: Arc::clone(&cell),
            })
        };
        let mine = row_of(
            &mut external,
            i64::try_from(in_process.0).expect("a small id"),
        );
        assert_eq!(
            (mine.get(RUN_DELIVERED_KEY), mine.get(RUN_DRIVING_KEY)),
            (Some(&json!(7)), Some(&json!(pane.0))),
            "⚠⚠⚠ A CONTROL FAILED: a run whose driver shares its cell lost what that cell said. \
             There is no report to prefer, so the cell IS the answer — and that is every run on a \
             daemon told `run-driver-process = off`, which is the way back. Row: {mine:?}",
        );
        let both = lock(&registry).persistable();
        let logged = both
            .runs
            .iter()
            .find(|run| run.id == in_process.0)
            .expect("the second run is in the log");
        assert_eq!(
            logged.banked,
            Some(crate::runs::PersistedBanked {
                completed: 6,
                unit: "turn".to_owned(),
            }),
            "⚠⚠⚠ AND THE FILE'S HALF OF THE SAME CONTROL: an in-process run's banked work must \
             still come off its cell. Saved: {logged:?}",
        );
    }

    /// This run's row, read back through the slot a client reads.
    fn row_of(external: &mut PluginsExternal, id: i64) -> Value {
        let IntrospectValue::Json(runs) = external.query(RUNS_SLOT).expect("the runs slot answers")
        else {
            panic!("the runs slot is JSON");
        };
        runs.as_array()
            .expect("a list of runs")
            .iter()
            .find(|run| run[RUN_ID_KEY] == json!(id))
            .unwrap_or_else(|| panic!("run {id} is in the listing: {runs:?}"))
            .clone()
    }
}
