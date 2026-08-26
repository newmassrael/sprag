//! The debt-repayment loop's decisions must move OUT of the driver, never back in — item 470.
//!
//! # What this gate is for
//!
//! The loop is driven by `ai_loop.scxml` and a Rust driver. Item 470 measured that the DECISIONS
//! are in the driver: a table keyed by the document's own states, which is a second copy of the
//! topology. ⚠ This gate was written while stages 2 and 3 of the repayment were refuted at the
//! pinned SCE (item 483 — a host could not register its own `<send>`/`<invoke>` type); **that was a
//! fact about a REV and it did not survive one.** The first act crossed on 2026-08-25, the third
//! the same day, and `SERVED_ACTS` counts the ones that have. The decisions still cannot all move
//! in one round.
//!
//! ⚠⚠⚠⚠⚠ **THREE NUMBERS ARE PINNED HERE AND THEY WATCH THREE DIFFERENT THINGS.** It reads like
//! redundancy and it is a division of labour, each half of it paid for: [`DECLARED_ACTS`] catches
//! an act being DELETED and is blind to one moving (measured twice); [`DRIVER_ARMS`] catches a
//! decision moving back INTO Rust and is blind to one moving out whenever the arm left behind is a
//! naming rather than a deletion (measured on `priming`); [`SERVED_ACTS`] is the only one that sees
//! the move item 470's stage 2 IS. ⚠ So a round paying this item may see exactly one of these six
//! tests go red, and that red is the whole record.
//!
//! ⚠⚠⚠⚠⚠ **AND MEANWHILE IT GREW.** The register recorded 153 state-keyed sites on 2026-08-19;
//! this gate measured more the next day, added by the very rounds that were paying the item down.
//! Nothing said so, because nothing was counting.
//!
//! # ⚠⚠⚠⚠⚠ Why the pin is EXACT rather than a ceiling
//!
//! A ceiling rots. Pay the debt down and the ceiling stays where it was, floating above the
//! measurement, biting nothing — green forever in the voice of a working gate, which is register
//! item 453's finding in a different costume. There is no moment at which anybody is told the
//! ceiling has gone slack, because a ceiling never complains about having room.
//!
//! So the pin is an equality and it is refused from BOTH sides:
//!
//! * measured ABOVE the pin — a decision moved INTO the driver. That is the regression, and the
//!   round that did it must say why in the register.
//! * measured BELOW the pin — a decision moved OUT of it. That is the debt being PAID, and the pin
//!   comes down with it in the same commit, so the gate is never looser than the truth.
//!
//! ⚠ The cost is stated rather than hidden: an ordinary driver edit that adds or drops a mention of
//! a state goes red here and wants one number changed. That is the point. `build.rs` in the plugin
//! makes the same trade in its own words — *there is deliberately no glob, so that adding a machine
//! is a decision somebody makes rather than a side effect of creating a file.*

use sprag_gate::loop_shape::{
    DOCUMENT, HOST_TYPE, StateKeyed, announced_sends, declared_acts, document_states, served_acts,
    state_keyed, tally,
};
use sprag_gate::sources::{rust_sources, workspace_root};

/// Every state of `ai_loop.scxml` and how many places this workspace's SHIPPING Rust keys
/// behaviour on it — measured 2026-08-20, and every row is a debt.
///
/// ⚠ States at `0` are the goal, not filler: a state the document decides entirely by itself needs
/// no line of Rust naming it. `converged`, `exhausted` and `failed` reaching zero would mean the
/// loop's ENDINGS are the document's alone.
/// ⚠⚠⚠⚠⚠ **EVERY ROW BELOW CAME DOWN BY ONE ON 2026-08-25, AND ONE DELETION DID ALL OF IT** —
/// register item 470, stage 2, first act.
///
/// `Owed::asked_for_an_account(state)` answered *was this state's turn asking for an account of the
/// run, rather than for work* with a `match` over all twenty-eight states of `ai_loop.scxml`,
/// twenty-six of them written out to say `false`. It is deleted: `closing` and `stopping` now
/// declare `<send type="x-sprag-host" event="prompt.say">` with `<param name="asks" expr="'account'"/>`,
/// and the driver reads what the sentence that opened the turn was FOR.
///
/// **250 sites over 28 states → 222.** The register's own first measurement, for scale, was 153 in
/// one file with its tests mixed in (2026-08-19); this walk is the whole workspace's shipping Rust.
///
/// ⚠⚠ **A ROW AT 7 IS NOT A ROW AT ZERO.** Seven exhaustive matches over this document's states
/// remain, and each of them costs every state one arm. What retires a row completely is those
/// matches going, one decision at a time — which is what the row's own number is here to watch.
///
/// ⭐⭐⭐⭐⭐ **AND ON 2026-08-26 EVERY ROW HERE FELL BY ONE, FOR THE FIRST TIME.** Stage 3 opened by
/// DELETING `Owed::on` — the first of the seven exhaustive state-matches — and the number that had
/// held for three rounds moved for all twenty-eight states at once, because that is what one match
/// costs: **one arm per state, and the same one.** ⚠ That is the shape to expect from stage 3 and
/// the shape stage 2 could never produce: an act MOVING trades a mention for another and this list
/// cannot see it; a match GOING takes a mention from every state and nothing else can.
///
/// ⭐⭐⭐⭐⭐ **AND EVERY ROW FELL BY ONE AGAIN, THE SAME DAY, FOR THE SECOND MATCH** — `pumping`'s,
/// which is the one item 470 was largest about. `OuterLoop::pump` chose WHAT TO DO from the name
/// of the state it was in, over all twenty-eight of them: thirteen arms that named an effect and
/// thirteen more that named the document's finals and regions so the match stayed exhaustive
/// without a wildcard. It is gone. The driver raises `pass`, `ai_loop.scxml`'s `work` region
/// answers with `<send type="x-sprag-host" event="pass.do">`, and the driver's remaining `match` is
/// over `sprag_plugin::Does` — a vocabulary of TWELVE effects, not a copy of a topology. ⚠ Named
/// rather than linked: this crate takes no dependencies by charter, so the type is not one it can
/// reach.
///
/// ⭐⭐⭐⭐⭐ **AND A THIRD TIME, THE SAME DAY, FOR `AiLoop::is_final`** — the match that answered *is
/// this state an ending* by naming all twenty-eight of them, seven `true` and twenty-one `false`.
/// It was the THIRD telling of one sentence: `ai_loop.scxml` declares seven `<final>` elements,
/// `pumping` has asked the engine since the entry above, and this said it again in Rust off a state
/// name. `OuterLoop::finished` — `is_in_final_state`, and nothing else — is the one reader now.
///
/// ⚠⚠ **WHAT THAT TRADE COST IS NAMED RATHER THAN HIDDEN**: the match was exhaustive, so an eighth
/// `<final>` used to break the BUILD, and that is how `peer_gone` and `abandoned` both arrived.
/// Nothing breaks now — the engine answers a new ending correctly the moment the document gains
/// one — and the failure mode this opens is a final placed INSIDE the `<parallel>`, which parses,
/// reads as an ending, and turns the question into *did every region complete*.
/// `every_ending_this_document_declares_sits_outside_the_parallel` is the gate that holds it, in
/// `sprag-plugin`'s own proving module, with the live half beside the structural one.
///
/// ⚠⚠ **THE UNIFORM FALL IS WHAT SAYS A MATCH WENT, AND IT HAS NOW SAID IT THREE TIMES IN ONE DAY.**
/// Read the entries together before concluding anything from a fourth: a round that moves every row
/// by a flat one has retired a match, and a round that moves none may still have moved an act.
///
/// ⚠ **FOUR MATCHES LEFT**, and three of them are in `ai_loop.rs` rather than here: which verdict
/// does an ending publish, which pane would a ceiling signal, may a ceiling account for it. The
/// fourth is `pumping`'s neighbour — the `because` match, which is keyed on where a pass ARRIVED
/// rather than on what it does.
///
/// ⚠⚠⚠⚠⚠ **AND A ROW HOLDING STILL IS NOT AN ACT STAYING PUT** — measured 2026-08-25 R75, the
/// round after the one above. `priming`'s act moved into the document and **not one row here
/// changed**: `Owed::on`'s `Priming => Start` arm was deleted, and `AiLoopState::Priming` had to be
/// added to the exhaustive arm that says *this state owes no prompt* in the same edit, because the
/// match has no wildcard. One mention for another. *Decides a prompt* and *owes nothing* look the
/// same to anything that counts mentions of a state, so this ratchet is blind to an act moving out
/// whenever the arm it leaves behind is a naming rather than a deletion — which is the ordinary
/// case, not the exception. [`SERVED_ACTS`] is what saw that move, and this list's job is the other
/// direction: a decision coming BACK.
const DRIVER_ARMS: &[(&str, usize)] = &[
    // ⚠⚠⚠⚠ ADDED 2026-08-21 BY ITEM 534, AND THE PIN WAS RAISED WITH ITS OWN REASON RATHER THAN
    // BECAUSE A GATE ASKED. Every one was an EFFECT arm in an exhaustive match — is it final, which
    // verdict does it publish, does it owe a prompt, may a ceiling account for it — and every one of
    // them existed because the match has no wildcard, which is what made this new final land as
    // compile errors instead of as silent defaults. ⚠ Two of those four questions have since left
    // Rust entirely (*owes a prompt* on 2026-08-26 with `Owed::on`, *is it final* the same day with
    // `AiLoop::is_final`), which is why the number below is not the one this note was written at.
    //
    // ⚠⚠⚠ IT IS EXACTLY `peer_gone`'s AND `held`'s NUMBER, which is the useful fact here: a seventh
    // ending costs this workspace what the sixth did, so the price of a new final is known and flat
    // rather than growing. What would be a REGRESSION is one more arm appearing later — a decision
    // about being abandoned taken in Rust — and that is what this row catches.
    //
    // ⚠ It came down 8 -> 7 on 2026-08-25 and 7 -> 6 -> 5 -> 4 on 2026-08-26, and NOT because
    // anything about being abandoned changed any of the four times: each was one of the seven
    // matches that cost every state an arm being deleted. See this constant's own note above.
    //
    // ⚠⚠ THE FOURTH WAS `is_final` ITSELF — *is it final* is the first question this comment lists,
    // and it is no longer asked here at all: `OuterLoop::finished` asks the engine. What remains in
    // this row is the other three.
    // ⚠⚠⚠⚠⚠ **SEVEN ROWS FELL TO ZERO ON 2026-08-26 R98 AND NOT ONE DRIVER LINE WAS DELETED FOR
    // THEM** — `cancelled`, `converged`, `failed`, `priming`, `working` outright, and `exhausted`
    // and `judging` in part. **The INSTRUMENT was wrong.** `sprag-host/src/live_agent.rs` is
    // declared `#[cfg(test)] mod live_agent;` in `lib.rs`, and `Source::product` splits test from
    // shipping ONE FILE AT A TIME — so it could not see an attribute that governs a module from
    // another file, and **fourteen of the twenty-four sites this list pinned were assertions inside
    // that measurement**. This ratchet was counting its own gates.
    //
    // ⚠⚠ **AND THE HARM WAS NOT THE OVERCOUNT.** A per-state pin satisfied by a MIXTURE of driver
    // arms and test assertions can be held flat while a real arm appears, because a gate deleted in
    // the same commit pays for it. An overstated ratchet is not a stricter one; it is one with a
    // second, invisible way to stay green. `rust_sources` now blanks the product of a file its
    // crate declares test-only.
    ("abandoned", 0),
    // ⚠⚠⚠⚠⚠ **THE LAST PER-STATE DECISION IN THIS LIST, AND IT WENT ON 2026-08-26 R99.**
    // `AiLoop::unbuilt` carried two arms keyed on this state — *a person took the pane* and *the
    // peer is asking and nothing got the run past it* — written when the driver had no act for it.
    // `attend` was built, so the document answers its `pass` and the arms could not fire.
    //
    // ⚠⚠ THE UNREACHABILITY WAS AN ARGUMENT UNTIL THAT ROUND, and this register has been bitten by
    // one that aged. It is now MEASURED: `sprag-plugin`'s
    // `every_driven_state_says_what_a_pass_of_it_is_for` drives the document to each state, raises
    // `pass`, and reads what the host was handed — so a document that ever dropped an arm is red
    // there rather than quietly answered out here.
    //
    // ⚠ **WHAT IS LEFT IN THIS LIST IS NO LONGER A DECISION.** Five sites: two SENTINELS
    // (`NotStarted::Unbuilt(Exhausted)` as an error value, and the sentence `plugins.rs` renders
    // for it), two GUARDS (`brief` refusing outside `idle`, and the work region's own membership
    // test), and one READER (`standing_down`, item 605). A round that wants this at zero has to
    // decide what those become, not merely move them.
    ("awaiting_human", 0),
    ("blocked", 0),
    ("cancelled", 0),
    ("closing", 0),
    ("converged", 0),
    ("disputing", 0),
    // ⚠⚠⚠⚠⚠ **THE TWO SENTINELS WENT ON 2026-08-26 R100, AND A RECORDED DECISION HAD TO BE
    // REVERSED TO DO IT.** `NotStarted::Unbuilt(AiLoopState)` carried the state a bad brief would
    // reach, and its own note said why: *"the variant stays a STATE rather than becoming a sentence
    // about turn budgets … the next state this build does not serve gets the same treatment."*
    //
    // ⚠⚠ **THE PREMISE IS GONE.** Since stage 3 the driver holds no list of states: one it cannot
    // drive is one the DOCUMENT answers nothing for, found at RUN time and reported as
    // `PaneError::Undrivable`. So no construction-time refusal for an unserved state can arise, the
    // one arm left was about a NUMBER, and the general `Unbuilt(state)` arm in `sprag-host` was
    // prose nothing could ever select. It is `NotStarted::NoTurns` now — no payload, so the
    // compiler is what says every caller answers it.
    ("exhausted", 0),
    ("failed", 0),
    ("held", 0),
    // ⚠⚠⚠⚠ **DECIDED 2026-08-26 R100: A COPY, AND RETIRABLE — BUT NOT WITH WHAT THIS DRIVER CAN
    // READ TODAY.** `OuterLoop::brief` refuses with `Briefed::TooLate(at)` unless the machine is in
    // `idle`, which is the document's own `brief` transition said a second time in Rust: an author
    // who made a second state briefable would still be refused out here.
    //
    // ⚠⚠ THE MOVE IS *raise it and ask whether the machine took it*, and the blocker was MEASURED
    // rather than argued (R100, by disabling the guard and running
    // `a_brief_that_arrives_after_the_run_started_is_refused_and_says_where_it_was`): a late brief
    // then comes back as **`NotHeld { part: "north_star", held: Some(<the OLD north star>) }`**.
    // ⇒ **The read-back cannot tell *refused* from *the old values are legitimately still there*,
    // because they are the same bytes.** Deleting the guard does not move a decision into the
    // document; it turns a true refusal into a false diagnosis pointing at the wrong file.
    //
    // ⚠ THE READER THAT WOULD FIX IT IS NAMED: the case is *dequeued, no transition matched*, which
    // SCE reports as `discarded_external_events` — and `OuterLoop::unseen` is NOT it (its own doc
    // says it answers about an event never dequeued because the machine had STOPPED). A round that
    // retires this row builds that reader first, or files an upstream STOP if the pin has no count.
    ("idle", 1),
    // ⚠⚠⚠⚠⚠ **AND THE LAST TOPOLOGY COPY IN `pumping` WENT WITH THEM** — 2026-08-26 R98. Three
    // lines asked `from == AiLoopState::Judging` to decide whether a pass reports its verdict, its
    // reason and its instrument: one question, asked three times, keyed on the name of a state
    // written in a file this driver does not parse. It now asks what the pass was FOR
    // (`does == Does::Judge`), which the document already says — and which is not the same
    // condition respelled: a document that ever gives `judge` to a second state gets its verdict
    // reported, where the state test would have dropped it silently.
    ("judging", 0),
    ("orders", 0),
    ("peer_gone", 0),
    ("priming", 0),
    ("redirecting", 0),
    ("reflecting", 0),
    ("restarting", 0),
    ("resuming", 0),
    ("reviewing", 0),
    ("running", 0),
    ("screening", 0),
    ("service_down", 0),
    ("standing", 0),
    // ⚠ NINE since 2026-08-22, and the ninth is a READER rather than a decision — register item
    // 605. `OuterLoop::standing_down` answers *has the machine heard a stand-down*, which nothing
    // could ask before: `sprag-host` publishes only its own flag, which says a person SPOKE. No
    // `cond` moved out of the document to get it, and the reader decides nothing.
    //
    // ⚠⚠⚠⚠⚠ **DECIDED 2026-08-26 R100: EXEMPT BY KIND, WITH THE ROW BELOW.** Both name a REGION
    // ROOT to answer *which of the active states is the one I mean* — `standing_down` in the orders
    // region, `work` in `OuterLoop::state`'s upward walk. A parallel configuration holds several
    // active states at once and the generated policy publishes `get_parent` and nothing else, so
    // the region's id is the ONLY handle there is. **No behaviour is chosen from either name**, and
    // there is no act a document could declare that would answer *you are in the work region* —
    // the question is about the arrangement, not about the run.
    //
    // ⚠⚠ SO THIS PIN CANNOT HONESTLY REACH ZERO, and saying so is the decision rather than a
    // failure to finish: two of the twenty-eight rows are readers of the topology and the item is
    // about DECISIONS in the driver. ⇒ **The floor is 2 + whatever `idle` becomes.** A round that
    // drives it lower is deleting a reader, which is deleting the only way this driver can tell
    // which region it is looking at.
    ("standing_down", 1),
    ("stopping", 0),
    ("work", 1),
    ("working", 0),
];

/// How many acts `ai_loop.scxml` declares for itself — one per `<onentry>`.
///
/// ⚠⚠ This side must GROW. A ceiling on the Rust alone can be satisfied by DELETING behaviour
/// instead of moving it, and a loop that decides less is not a loop that decides in its document.
///
/// ⚠⚠⚠⚠⚠ **IT DID NOT MOVE ON THE ROUND THAT TOOK 28 ARMS OUT OF THE DRIVER**, and that is this
/// number's measured blindness rather than a fact about the round: `closing` and `stopping` already
/// had an `<onentry>`. [`SERVED_ACTS`] is the number that saw it.
///
/// ⚠⚠⚠ **AND IT DID NOT MOVE ON THE THIRD ACT EITHER, WHICH IS WHAT MAKES THAT A PROPERTY AND NOT
/// AN ACCIDENT** — measured 2026-08-25 R75, when `priming` handed its first sentence to the host.
/// `priming` already had an `<onentry>` too, and it always will have: an act moves by changing what
/// a block CONTAINS, so a counter of blocks is blind to item 470's stage 2 by construction. Two
/// separate rounds have now confirmed that from the other side, which is worth more than the
/// reasoning — this number stays because it is the side that catches an act being DELETED, and it
/// held at 11 through every act that moved.
///
/// ⭐⭐⭐⭐⭐ **AND THEN IT MOVED, 11 → 18, FOR THE FIRST TIME SINCE IT WAS PINNED** — 2026-08-26, the
/// fourth match of stage 3. The prediction above is not broken by this and is confirmed by it: the
/// seven new blocks are on the seven `<final>` elements, which had **no `<onentry>` at all** before
/// this round. That is what this number was written to see — a block that did not exist coming into
/// existence — as against an act moving INTO a block that was already there, which is what it is
/// blind to and what the two entries above measured it being blind to.
///
/// ⚠ So the two numbers moved TOGETHER here (`SERVED_ACTS` 22 → 29, this 11 → 18) and that
/// agreement is itself the reading: seven acts arrived in seven new blocks, one each.
///
/// ⛔⛔⛔⛔⛔ **AND THEN IT WENT DOWN, 18 → 14, WHICH IS THE DIRECTION THIS NUMBER EXISTS TO REFUSE**
/// — 2026-08-26. It is argued here rather than quietly re-pinned, because *"a ceiling on the Rust
/// alone can be satisfied by DELETING behaviour instead of moving it"* is exactly what a fall looks
/// like and this one has to be told apart from that.
///
/// What went: the document's **last five `<send>`s carrying no `type`** — `screen.begin`,
/// `notify.human`, `review.begin`, `session.replace`, `redirect.begin`. Four of them were the SOLE
/// content of their state's `<onentry>`, so four blocks went with them.
///
/// ⚠⚠ **NO BEHAVIOUR LEFT WITH THEM, AND THAT IS MEASURABLE RATHER THAN ASSERTED.** Each named an
/// effect the driver already performs through `pass.do`: `screen`, `review`, `replace`, `attend`
/// and `redirect` are five of [`SERVED_ACTS`]'s own words. The announcements were RESIDUE — the act
/// moved in stage 2 and the shout stayed — and nothing listened to any of them: zero
/// `<transition>`s and, for four of the five, not one Rust reader. The fifth (`session.replace`)
/// had exactly one, a gate ASSERTING nothing sends it, and that gate's own comment had predicted
/// this day: *"the day `restarting`'s act moves too, this stops compiling."* It did.
///
/// ⚠ [`announced_sends`](sprag_gate::loop_shape::announced_sends) is now pinned at ZERO, which is
/// what keeps this fall from being repeatable: a state that owes an effect must declare it as an
/// act the host serves, where a document asking for something nobody performs is REFUSED.
const DECLARED_ACTS: usize = 14;

/// How many acts `ai_loop.scxml` asks THIS HOST to perform — one per `<send type="x-sprag-host">`.
///
/// # ⚠⚠⚠⚠⚠ This is the number item 470's second stage is measured in
///
/// An act leaves the document exactly one way: the document keeps saying WHAT and WITH WHAT, and a
/// host performs it. Every one of these is a decision that used to be derived out in Rust from the
/// name of the state it belonged to — **two of them on 2026-08-25 cost the driver twenty-eight
/// arms**, because what left was not a line but a whole table keyed by this document's states.
///
/// ⚠⚠⚠⚠⚠ **AND THE THIRD ONE COST IT NONE, WHICH IS WHY THIS COUNTER HAS TO EXIST.** Measured
/// 2026-08-25 R75, on `priming`'s first sentence: of the three numbers this file pins, this is the
/// **only** one that moved. [`DECLARED_ACTS`] stayed at 11 because `priming` already had its
/// `<onentry>`, and **every `DRIVER_ARMS` row stayed put as well** — `Owed::on`'s `Priming =>
/// Start` arm was deleted and `AiLoopState::Priming` joined the exhaustive list that says *this
/// state owes nothing*, so a decision left the driver at the cost of one mention traded for
/// another. *Owes nothing* and *decides nothing here* are indistinguishable to anything that counts
/// mentions of a state, and the exhaustive list cannot be dropped — it is what makes a new state
/// fail to compile.
///
/// ⚠⚠⚠ So the division of labour is now MEASURED rather than argued, and it is the whole of item
/// 470's stage 2: [`DECLARED_ACTS`] catches an act being deleted, [`DRIVER_ARMS`] catches a
/// decision moving back INTO Rust, and **only this number can see one move OUT.** A round that
/// paid this item and watched the other two would have watched two green gates and concluded
/// nothing happened.
///
/// ⚠⚠⚠⚠⚠ **AND THE FOURTH REPEATED IT, ON THE STATE THAT ADDED A WORD TO THE ACT'S ARGUMENT.**
/// Measured 2026-08-26, on `reflecting`: 3 → 4 here, `DECLARED_ACTS` 11 → 11, and **every
/// `DRIVER_ARMS` row held again** — `Owed::on`'s `Reflecting => Reflect` arm was deleted and
/// `AiLoopState::Reflecting` joined the same exhaustive list, one mention traded for another for the
/// second round running. So the blindness above is not `priming`'s special case; it is what this
/// item's remaining acts will do, and this number is the only eye on any of them.
///
/// ⚠⚠⚠⚠⚠ **AND THE FIFTH IS EVERY `<onentry>` PROMPT THE DOCUMENT HAS.** Measured 2026-08-26, on
/// `disputing`: 4 → 5 here, `DECLARED_ACTS` 11 → 11, and **every `DRIVER_ARMS` row held for the
/// third round running**. What is left in `Owed::on` is `working`'s arm, and it is NOT the next one
/// in this sequence: `prompt.turn` is a **transition** send, so no state entry can declare it and
/// moving it is a different move. ⚠ So a run of this number from 5 to 6 is not what finishes stage
/// 2 — read the register.
///
/// ⚠⚠⚠⚠⚠ **AND THE SIXTH MOVE WAS THE EDGES, WHICH IS WHY THIS JUMPED FOUR.** Measured 2026-08-26:
/// 5 → **9**. `prompt.turn` sat on FOUR transitions into `working` — not the three the driver's
/// table appeared to hold, because `judging` reaches `working` twice on `judge` (one guarded on an
/// unreadable turn) and `Owed::on` keyed on `(event, landed)` saw those two as ONE key. **A
/// document can say what an edge owes; a table keyed by arrivals could not tell those two edges
/// apart at all.** `probe.rs`'s `a_transition_can_ask_this_host_for_an_act_and_its_arguments_reach_it`
/// is what said the road existed before any of this was written.
///
/// ⚠ **THIS IS THE NUMBER'S LAST BIG JUMP FROM PROMPTS.** Every `prompt.*` the document announced
/// has moved; what still announces a name is five sends nothing reads, which is a different
/// question — read the register rather than expecting this to keep climbing.
///
/// ⭐⭐⭐ **AND THE TENTH IS THE ONE THAT WAS NEVER A PROMPT** — 2026-08-26, stage 3's opening.
/// `service_down`'s edge to `working` declares `service_retry_text`, the word that ends an outage,
/// and with it gone `Owed::on` answered for nothing and was **deleted**. That is the first of the
/// seven exhaustive state-matches to go, and the first round in which [`DRIVER_ARMS`] rows FALL.
///
/// ⭐⭐⭐⭐⭐ **AND THE ELEVENTH THROUGH TWENTY-SECOND ARE NOT PROMPTS AT ALL** — 2026-08-26, the
/// second match of stage 3. 10 → **22**, the largest jump this number has ever taken, and every one
/// of the twelve is a `pass.do`: *what the driver DOES while the machine is here*. They live on the
/// `work` region rather than on its states, which was `sce-build`'s answer rather than a taste —
/// an event on every child of a compound satisfies its handler-consistency precondition, and that
/// compound then reports twenty-seven deliberate protocol-stage gaps it has always had. The
/// validator's own documentation names a parent-level handler as the repair.
///
/// ⚠⚠ **TWELVE ACTS FOR FIFTEEN DRIVEN STATES, AND THE DIFFERENCE IS THE FINDING.** `priming` and
/// `disputing` both answer `sent`; `working`, `closing` and `stopping` all answer `watch`. In the
/// driver those were five arms. A decision that was being made five times turns out to be two, and
/// nothing could see that while the answer came from the state's own name.
///
/// ⚠ **SO THIS NUMBER IS NO LONGER ONLY ABOUT SENTENCES.** A reader expecting it to count prompts
/// is reading an older comment: it counts what the document asks this host to DO, and since stage 3
/// most of that is passes.
///
/// ⚠⚠⚠⚠⚠ **AND IT DID NOT MOVE FOR THE THIRD MATCH, WHICH IS THIS NUMBER'S OTHER BLINDNESS** —
/// measured 2026-08-26, the round that deleted `AiLoop::is_final`. Twenty-eight arms left the
/// driver and nothing was added here, because that decision did not move to an ACT at all: the
/// document was already answering it STRUCTURALLY, with the seven `<final>` elements it has always
/// declared, and what changed is that the driver finally asks the engine instead of keeping a copy.
/// ⚠ So a stage-3 round can be real with this number flat. [`DRIVER_ARMS`]'s uniform fall is what
/// sees that one, exactly as this number is what sees an act moving while those rows hold still —
/// each is blind where the other looks, which is the division of labour the header claims.
///
/// ⭐⭐⭐⭐⭐ **AND THE TWENTY-THIRD THROUGH TWENTY-NINTH ARE THE SEVEN ENDINGS** — 2026-08-26, the
/// fourth match of stage 3. 22 → **29**. Every `<final>` in `ai_loop.scxml` now declares
/// `<send type="x-sprag-host" event="end.publish">` with the word it publishes, which is what
/// `AiLoop::ended` decided from a twenty-eight-arm state match until this round.
///
/// ⚠⚠ **AN ENDING'S ENTRY IS A DIFFERENT REGIME FROM EVERY OTHER ACT HERE, and it was PROBED before
/// it was used**: entering a top-level `<final>` completes the machine, so no further macrostep is
/// owed and no reply can be delivered. `probe.rs` measured that the handler is nevertheless called
/// with its `<param>`, with the control on the edge INTO the ending so a dead handler could not
/// read as a NO.
///
/// ⭐⭐⭐⭐⭐ **AND THE THIRTIETH THROUGH THIRTY-FOURTH ARE THE ACCOUNT QUESTION** — 2026-08-26, the
/// last of stage 3's seven matches. 29 → **34**. `ask_for_an_account` chose between granting a
/// window and twenty ways of refusing one with a `match` over all twenty-eight states; the fifteen
/// states a driver actually drives now answer it on five guarded targetless
/// `<transition event="account">`s, `pass.do`'s shape exactly.
///
/// ⚠⚠ **FIVE SENDS FOR FIFTEEN STATES, AND THE DIFFERENCE IS THE FINDING AGAIN** — the same shape
/// `pass.do` produced when twelve acts covered fifteen states. Eight states share one answer
/// (`within`) and four share another (`between_sessions`), so what looked like twenty-eight
/// decisions was five.
///
/// ⚠ [`DECLARED_ACTS`] does NOT move for this one: a `<transition>` is not an `<onentry>`, so the
/// block count is blind to it exactly as it was blind to every act that moved in stage 2.
///
/// ⭐⭐⭐⭐⭐ **AND THE THIRTY-FIFTH THROUGH FORTY-FIRST ARE THE ARROW'S OWN WORD** — 2026-08-26, the
/// seventh and last of stage 3's matches. 34 → **41**. `OuterLoop::pumping` chose which of the
/// run's readings belonged on the arrow it had just drawn from a `match` over all twenty-eight
/// states; five arrivals now declare it on a targetless `<transition event="arrived">`, and the two
/// `judging -> working` EDGES declare it on themselves.
///
/// ⚠⚠ **THE TWO EDGE-DECLARED ONES ARE THE POINT.** `working` is entered from four states and only
/// a judgement has a reading behind it, so no `In(…)` could answer it — the old match needed an
/// `if from == AiLoopState::Judging` for exactly that. An edge saying *this door* is the document
/// holding a fact the driver was reconstructing.
///
/// ⚠⚠ Refused from BOTH sides, for [`DRIVER_ARMS`]'s reason exactly: below is behaviour coming back
/// out of the document, above is the debt being paid and the pin owes the same commit.
const SERVED_ACTS: usize = 41;

fn document() -> String {
    let path = workspace_root().join(DOCUMENT);
    std::fs::read_to_string(&path)
        .unwrap_or_else(|why| panic!("{} is this loop's document: {why}", path.display()))
}

fn measured() -> (Vec<String>, Vec<StateKeyed>) {
    let states = document_states(&document());
    let sites = state_keyed(&rust_sources(), &states);
    (states, sites)
}

/// ⚠⚠⚠⚠⚠ **A PROBE POINTED AT NOTHING MUST NEVER READ AS CLEAN.** Every other gate here is an
/// equality, and an equality against an empty measurement is satisfied by the measurement having
/// failed. This is the one that says the walk found the loop at all.
#[test]
fn the_measurement_reaches_the_loop_it_is_judging() {
    let (states, sites) = measured();

    assert!(
        states.len() > 20,
        "`{DOCUMENT}` declares the loop's states and this walk found only {}: a reader pointed at \
         the wrong file answers about the wrong file",
        states.len(),
    );
    // ⚠⚠⚠⚠⚠ **THIS WAS `sites.len() > 50` AND THE FLOOR ROTTED, WHICH IS THIS ITEM'S OWN LESSON
    // ARRIVING FROM THE OTHER SIDE** — 2026-08-26, stage 3's last match. The number was written
    // when the walk found 250 sites and it read as *the walk found the loop at all*; by the round
    // that retired the seventh match the debt was genuinely down to 24, and a control calibrated
    // against the DEFECT went red for the defect being fixed. A ceiling rots because the debt
    // shrinks under it; a floor rots for exactly the same reason, upwards.
    //
    // ⚠⚠ SO IT IS ANCHORED TO THE PIN INSTEAD OF TO A REMEMBERED SIZE. The pins are what the walk
    // is measured against, so an empty or misdirected walk cannot satisfy this while any pin is
    // nonzero — and it needs no maintenance as the debt falls.
    //
    // ⚠ **THE RESIDUE, STATED RATHER THAN HIDDEN: when the pins reach ZERO this control goes
    // blind**, and zero is the item's declared goal (`converged`, `exhausted` and `failed` at 0
    // would mean the loop's endings are the document's alone). On that day a walk that found
    // nothing and a driver that names nothing read the same here, and something else has to stand
    // in — the document-side numbers above, which grow where this one shrinks.
    let pinned: usize = DRIVER_ARMS.iter().map(|(_, arms)| *arms).sum();
    assert!(
        pinned > 0 && sites.len() == pinned,
        "this walk found {} state-keyed site(s) and the pin accounts for {pinned}: a probe pointed \
         at nothing, or at the wrong tree, reads as clean in every equality below it",
        sites.len(),
    );

    let files: std::collections::BTreeSet<&str> =
        sites.iter().map(|site| site.file.as_str()).collect();
    assert!(
        files.contains("crates/sprag-plugin/src/outer.rs"),
        "the loop's driver is where item 470 measured the defect and the walk must reach it: {files:?}",
    );
}

/// ⚠⚠⚠⚠ The pin names the DOCUMENT's states, so a state added there cannot slip in unpinned.
///
/// Without this, a new state's arms would be counted by nobody: `DRIVER_ARMS` is a list, and a list
/// with no glob decides alone. Here the document is the glob.
#[test]
fn the_pin_names_this_documents_states_and_no_others() {
    let (states, _) = measured();
    let pinned: std::collections::BTreeSet<&str> =
        DRIVER_ARMS.iter().map(|(state, _)| *state).collect();
    let declared: std::collections::BTreeSet<&str> = states.iter().map(String::as_str).collect();

    let unpinned: Vec<&&str> = declared.difference(&pinned).collect();
    let stale: Vec<&&str> = pinned.difference(&declared).collect();

    assert!(
        unpinned.is_empty() && stale.is_empty(),
        "`{DOCUMENT}` and this pin have to name the same states.\n  \
         the document declares and the pin does not mention: {unpinned:?}\n  \
         the pin mentions and the document no longer declares: {stale:?}\n\
         A state the pin does not name is a state whose driver arms nothing counts.",
    );
}

/// ⚠⚠⚠⚠⚠ The ratchet itself — refused from BOTH sides, and the refusal says which.
#[test]
fn no_decision_moves_back_into_the_driver_and_the_pin_follows_the_ones_that_leave() {
    let (states, sites) = measured();
    let counted = tally(&sites, &states);

    let mut grew = Vec::new();
    let mut shrank = Vec::new();
    for (state, pinned) in DRIVER_ARMS {
        let now = counted.get(*state).copied().unwrap_or_default();
        if now == *pinned {
            continue;
        }
        let where_at: Vec<String> = sites
            .iter()
            .filter(|site| site.state == *state)
            .map(|site| format!("{}:{}", site.file, site.line))
            .collect();
        let line = format!("  {state}: pinned {pinned}, measured {now}  {where_at:?}");
        if now > *pinned {
            grew.push(line)
        } else {
            shrank.push(line)
        }
    }

    assert!(
        grew.is_empty(),
        "⚠⚠⚠ A DECISION MOVED INTO THE DRIVER. Item 470's whole finding is that the loop's \
         behaviour lives in Rust rather than in `{DOCUMENT}`, and these states gained arms:\n\
         {}\n\
         If the decision belongs in the document, say it there — a `cond` is the datamodel's and \
         needs no registry (item 470 stage 1, `8ac134a`). If it is an EFFECT and genuinely belongs \
         here, raise the pin in the same commit and say why in the register.",
        grew.join("\n"),
    );

    assert!(
        shrank.is_empty(),
        "The debt got SMALLER and the pin did not follow, so this gate is now looser than the \
         truth and would not notice the next regression:\n\
         {}\n\
         Lower these numbers in the same commit. A ratchet that only ever refuses increases drifts \
         above the measurement and goes quietly green forever (item 453).",
        shrank.join("\n"),
    );
}

/// ⚠⚠ The other side of the trade: the document must not decide LESS than it does today.
#[test]
fn the_document_keeps_every_act_it_has_taken_over() {
    let acts = declared_acts(&document());

    assert!(
        acts >= DECLARED_ACTS,
        "`{DOCUMENT}` declared {acts} acts and the pin says {DECLARED_ACTS}. Behaviour left the \
         document, which is item 470 running backwards: a ceiling on the driver's arms can be \
         satisfied by deleting an act instead of moving one.",
    );
    assert_eq!(
        acts, DECLARED_ACTS,
        "the document declares MORE acts than the pin, which is the debt being paid — raise \
         `DECLARED_ACTS` to {acts} in the same commit so the floor stays under the truth",
    );
}

/// ⚠⚠⚠⚠⚠ **AND THE SIDE THAT CAN SEE AN ACT MOVE** — register item 470, stage 2.
///
/// The gate above counts `<onentry>` blocks, and an act moving into the document does not
/// necessarily add one: the two that moved first were already inside blocks that existed. This
/// counts the construct that actually carries a decision out of the file — a `<send>` addressed to
/// this host — and it is the number that answers *how much of this loop does its own document
/// decide*.
#[test]
fn the_document_announces_nothing_to_a_machine_that_is_not_listening() {
    let announced = announced_sends(&document());

    assert_eq!(
        announced, 0,
        "`{DOCUMENT}` raises {announced} `<send>` with no `type` — a name put on the machine's own \
         queue that no `<transition>` listens for and no driver reads. It LOOKS like an \
         instruction and cannot be one: `sprag-plugin`'s \
         `the_machine_instructs_its_driver_through_its_state_not_through_its_sends` measured that \
         the event-driven driver those sends imply cannot be written, because the one handle that \
         looks like a subscription mints a fresh empty queue on every call.\n\
         ⚠⚠ AND AN ACT NOBODY CARRIES OUT IS INDISTINGUISHABLE FROM ONE THAT WORKED, which is the \
         silence item 470 exists to end. If this state owes an effect, declare it as an act the \
         host serves (`<send type=\"{HOST_TYPE}\" …>`) so a document that asks for something \
         nobody performs is REFUSED where the machine can hear it.",
    );
}

/// ⚠⚠⚠⚠ The other side of the same ratchet: acts the host really does perform.
#[test]
fn the_document_asks_this_host_for_every_act_it_has_taken_over() {
    let served = served_acts(&document());

    assert!(
        served >= SERVED_ACTS,
        "`{DOCUMENT}` asks this host for {served} act(s) and the pin says {SERVED_ACTS}. An act \
         that stopped being the document's went back into the driver, which is item 470 running \
         backwards.",
    );
    assert_eq!(
        served, SERVED_ACTS,
        "the document asks for MORE acts than the pin, which is the debt being PAID — raise \
         `SERVED_ACTS` to {served} in the same commit, and lower whatever `DRIVER_ARMS` rows the \
         act took with it. ⚠ THAT MAY BE NONE, and this gate going red alone is then the only \
         record that anything moved: measured on `priming` (2026-08-25), where an arm that decided \
         a prompt was traded for a mention in the exhaustive list that says the state owes nothing, \
         and every row held. Do not read the other two gates' green as this one being wrong.",
    );
}

/// ⚠⚠⚠⚠⚠ **AN ACT THE DOCUMENT ASKS FOR IS ONLY SERVED IF THE BUILD DECLARED THE TYPE** — and the
/// two halves are in two files, which is why this reads both.
///
/// SCE's contract: a host declares its Event I/O Processor types at BUILD time so codegen emits a
/// dispatch, and registers a handler at RUN time. Either half missing produces the same thing —
/// `error.execution` at the send — so a `build.rs` that dropped the type would turn **every**
/// act-declaring `<send>` in the document into a refusal, and the run into a `failed`.
///
/// ⚠⚠⚠⚠⚠ **AND THE FIRST DRAFT OF THIS COMMENT CLAIMED SOMETHING THE MUTATION REFUTED.** It said
/// *this is the half no product test can reach, because a crate that failed to declare its type
/// still compiles and passes every test that does not enter the state* — item 470's own "THE BUILD
/// SAYS NOTHING". Measured 2026-08-25 by dropping the type from `HOST_TYPES`: the product gate goes
/// red too, and loudly — `Reflecting --ReflectDone--> Failed`, because item 505 gave the document an
/// `error.execution` edge that did not exist when 470 wrote that sentence. **The build is no longer
/// silent; a run that meets an undeclared type ends `failed`.**
///
/// What this gate is worth is therefore narrower and still real: it names the CAUSE. The product's
/// answer is a run that failed, and reading it back to *`build.rs` stopped declaring a type* is a
/// walk and a document and two crates away. This says it in one line, at the file a person would
/// edit — and it says it without running a pane.
#[test]
fn an_act_this_document_asks_for_is_declared_to_the_build() {
    let build = workspace_root().join("crates/sprag-plugin/build.rs");
    let source = std::fs::read_to_string(&build)
        .unwrap_or_else(|why| panic!("{} declares this host's types: {why}", build.display()));

    assert!(
        served_acts(&document()) > 0,
        "⚠⚠⚠ THE CONTROL: this gate is about a type the document USES, and a document that asks \
         for no act would make it pass by being about nothing.",
    );
    assert!(
        source.contains(&format!("\"{HOST_TYPE}\"")),
        "`{DOCUMENT}` addresses {} act(s) to `{HOST_TYPE}` and {} does not declare that type. A \
         type the build did not declare is refused at the send — the document's acts would all \
         raise `error.execution` and every run that reached one would end `failed`.",
        served_acts(&document()),
        build.display(),
    );
}
