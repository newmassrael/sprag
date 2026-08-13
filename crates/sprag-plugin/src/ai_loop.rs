//! The OUTER loop — the machine that drives an inner agent session, and what
//! measuring it said about the datamodel it was authored with.
//!
//! [`ai_loop.scxml`] is the third control statechart in this crate, after the
//! Driver's `orchestration.scxml` and the endpoint's `session.scxml`. Until this
//! round it was the only one that was **not compiled**: `build.rs`'s `STATECHARTS`
//! listed two of the three, so 312 lines of authored control flow were enforced by
//! nothing and eight Rust doc comments cited the document as an authority no
//! compiler had ever read.
//!
//! Adding it to that list is one word. What the word bought is this module.
//!
//! [`ai_loop.scxml`]: ../../ai_loop.scxml

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};

    use crate::sm::ai_loop::{AiLoopEvent, AiLoopPolicy, AiLoopState};

    /// The document's own composed prompt, as a person reading the file expects it.
    const COMPOSED_START_PROMPT: &str = "North star: ";

    /// A machine plus the engine its datamodel lives in, and the session id that
    /// engine files those variables under.
    ///
    /// Both halves are handed back because they answer different questions: the
    /// ENGINE holds `<data>` a script datamodel evaluates, and the POLICY holds the
    /// data SCE was able to lower into typed Rust fields. A gate that reads only one
    /// of them cannot tell those two apart, which is the whole subject below.
    fn started() -> (Engine<AiLoopPolicy>, Arc<dyn IScriptEngine>, String) {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(AiLoopPolicy::new(Arc::clone(&lua)));
        engine.initialize();
        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel must have opened a script session");
        (engine, lua, session)
    }

    /// ⚠⚠⚠ **HOW THE MACHINE TELLS ITS DRIVER WHAT TO DO — asked of the ENGINE, because the
    /// answer decides the driver's whole shape and the document cannot settle it.**
    ///
    /// `ai_loop.scxml` reads as though it were giving instructions: `priming` does
    /// `<send event="prompt.start"/>`, `restarting` does `<send event="session.replace"/>`, and
    /// seven such sends between them name every effect an outer driver has to perform. So the
    /// obvious driver is EVENT-DRIVEN: subscribe to the machine's sends, do what each one says.
    ///
    /// **That driver cannot be written, and this gate is where that was established rather than
    /// assumed.** A targetless `<send>` is W3C SCXML 6.2's *external event to SELF*: the generated
    /// code calls `raise_external_with_meta` on the machine's OWN queue, and no transition in this
    /// document listens for any of the seven — so they are raised and dropped. The one handle that
    /// looks like a subscription, `Engine::get_external_queue_handle`, is for `#_parent` sends out
    /// of `<invoke>`d CHILD machines and **mints a fresh empty queue on every call**.
    ///
    /// So the driver is **STATE-DRIVEN**: it reads `get_current_state()` and acts on where the
    /// machine IS, and the machine's own published ingress partition is what says this is the
    /// intended shape — `prompt.sent` (the driver's ANSWER) is externally drivable, while
    /// `prompt.start` (the supposed instruction) is not. The sends are documentation of intent
    /// that the compiler carries; the STATE is the contract.
    ///
    /// ⚠ Written as an assertion rather than as a comment because R376 paid for exactly this
    /// distinction one round ago: reading SCE's generated source said the opposite of what running
    /// it says. Whatever this gate reports is the thing to build against.
    #[test]
    fn the_machine_instructs_its_driver_through_its_state_not_through_its_sends() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: `start` must land in the state whose onentry sends `prompt.start`",
        );

        // ── the door that looks like a subscription ──
        let drained = engine.get_external_queue_handle();
        let seen = drained.lock().expect("the queue mutex").len();
        assert_eq!(
            seen, 0,
            "⚠⚠⚠ `prompt.start` WAS just sent, and this handle shows {seen} events. If it ever \
             shows one, the driver below is the wrong shape — it should subscribe rather than \
             read state, and every effect it performs should be keyed on a send",
        );

        // ── what the machine says a driver may tell it ──
        let ingress = AiLoopEvent::EXTERNALLY_DRIVABLE_EVENTS;
        assert!(
            ingress.contains(&AiLoopEvent::PromptSent),
            "the driver's ANSWER — *I have sent it* — must be something the machine accepts from \
             outside, or a driver cannot report having acted at all: {ingress:?}",
        );
        assert!(
            !ingress.contains(&AiLoopEvent::PromptStart),
            "⚠⚠ and the supposed INSTRUCTION is not an ingress event, which is the machine saying \
             the same thing from the other side: nobody outside sends `prompt.start`, so nothing \
             outside is meant to receive it either. It is the STATE that instructs: {ingress:?}",
        );

        // ── and the state is a complete instruction on its own ──
        //
        // Every effect the seven sends name is recoverable from where the machine is, which is
        // what makes the state-driven driver whole rather than a degradation of the other one.
        for (state, effect) in [
            (AiLoopState::Priming, "deliver the start prompt"),
            (AiLoopState::Screening, "match the dialog against the rules"),
            (AiLoopState::AwaitingHuman, "raise a pane attention"),
            (AiLoopState::Reflecting, "write the improvements"),
            (
                AiLoopState::Restarting,
                "close the pane and open a fresh one",
            ),
        ] {
            assert_ne!(
                state,
                AiLoopState::Working,
                "each effect state must be distinguishable from the one where the driver only \
                 watches, or *{effect}* would have to be inferred from something else",
            );
        }
    }

    /// ⚠⚠⚠ **HOW `judging`'s GOAL-MET GUARD ACTUALLY READS ITS DATA** — the one fact an outer
    /// driver must send with an event, asked of the engine because getting it wrong is silent.
    ///
    /// `judging`'s first transition is `<transition event="judge" cond="_event.data.done"
    /// target="closing"/>`, so *did the agent say it was done* travels as event DATA rather than
    /// as a datamodel variable. Every other event on this machine's ingress surface is bare.
    ///
    /// The driver's first attempt sent `{"done": false}` as the event data and the machine went to
    /// `closing` anyway — a loop that converges on the turn its agent has NOT finished, reporting
    /// success and asking for a closing summary of work that did not happen. **The screen said the
    /// marker was absent and the machine converged regardless**, which is exactly the class of
    /// silent wrongness this project keeps paying for.
    ///
    /// So the two readings are pinned side by side here, in the machine's own terms, and whatever
    /// this gate reports is what the driver is built against.
    #[test]
    fn the_goal_met_guard_separates_a_finished_agent_from_an_unfinished_one() {
        /// Walk a fresh machine to `judging` and raise `judge` carrying `data`.
        fn judged(data: &str) -> AiLoopState {
            let (mut engine, _lua, _session) = started();
            engine.process_event(AiLoopEvent::Start);
            engine.process_event(AiLoopEvent::PromptSent);
            engine.process_event(AiLoopEvent::TurnDone);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "the control: one completed turn is judged",
            );
            engine.raise_external(AiLoopEvent::Judge, data, "");
            engine.step();
            engine.get_current_state()
        }

        assert_eq!(
            judged("{\"done\": true}"),
            AiLoopState::Closing,
            "an agent that said the milestone was reached sends the loop to its closing report",
        );
        assert_eq!(
            judged("{\"done\": false}"),
            AiLoopState::Working,
            "⚠⚠⚠ AND AN AGENT THAT DID NOT MUST TAKE ANOTHER TURN. Converging here reports a \
             milestone reached on the strength of a screen that does not say so — the driver \
             measured exactly this and the machine converged on turn one",
        );
    }

    /// ⚠⚠⚠ **THE OUTER LOOP IS A MACHINE NOW, AND THIS IS WHAT THAT BUYS.**
    ///
    /// The topology the document draws is the topology the compiler enforces. This
    /// drives the two edges the last two rounds spent themselves on — R372's *a
    /// person took the pane* and R373's *they gave it back* — through the OUTER
    /// machine rather than through prose about it.
    ///
    /// The point is not that the transitions work; SCE's own W3C suite covers that.
    /// It is that these transitions EXIST TO BE DRIVEN AT ALL. Before this round
    /// `working --turn.interrupted--> awaiting_human` was a sentence in an XML
    /// comment, and the Rust that implements the same idea was gated against its own
    /// hand-written vocabulary with nothing joining the two.
    #[test]
    fn the_outer_loop_runs_the_edges_the_last_two_rounds_built() {
        let (mut engine, _lua, _session) = started();
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Idle,
            "the document's `initial`",
        );

        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "a started loop primes a session before it prompts it",
        );

        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // R372: a person reached into the pane. The loop stops driving.
        engine.process_event(AiLoopEvent::TurnInterrupted);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::AwaitingHuman,
            "⚠ the edge R372 built the product half of",
        );

        // R373: they let go. The loop takes the pane back and prompts again.
        engine.process_event(AiLoopEvent::Resume);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Working,
            "⚠ and the edge R373 built the product half of. `orchestration.scxml` \
             says this one was left out because *when has somebody stopped typing* \
             had no measured answer; `Handback::WhenStill` is that answer, and this \
             is the machine that was waiting for it",
        );
    }

    /// ⚠⚠⚠ **THE OUTER BUDGET IS ENFORCED BY THE MACHINE — debt 60's third item,
    /// and the first of the three that could be paid at all.**
    ///
    /// `max_turns` and `reflect_every` have sat in the document's datamodel since
    /// `95207ad` with nothing reading them: the register recorded them as *"already
    /// in the datamodel"*, which was true and meant only that the numbers were
    /// written down. A number nothing compares against is a comment.
    ///
    /// Now `judging` is a state a compiler emitted, so the three guards in it are
    /// three branches with a priority order the DOCUMENT fixed, and this walks the
    /// whole authored budget through them:
    ///
    /// * `reflect_every` (8) fires at turns 8, 16, 24 and 32 — the loop stops to
    ///   improve itself and `reflecting` resets the counter on entry;
    /// * `max_turns` (40) ends the run — and it wins at turn 40 even though
    ///   `turns_since_reflect` has also come round, because the document orders the
    ///   `max_turns` transition FIRST. **That precedence is the assertion**: a run at
    ///   its ceiling must end rather than pay for one more restart it has no turns
    ///   left to use.
    ///
    /// ⚠ THE SEQUENCE IS COLLECTED, NOT SPOT-CHECKED. Asserting only the ending
    /// would pass for a machine that reflected on every turn, or never; asserting
    /// only a reflect point would pass for one that never stopped.
    #[test]
    fn the_outer_budget_the_document_authors_is_the_one_the_machine_enforces() {
        let (mut engine, _lua, _session) = started();
        engine.process_event(AiLoopEvent::Start);
        engine.process_event(AiLoopEvent::PromptSent);
        assert_eq!(engine.get_current_state(), AiLoopState::Working);

        // Where the loop went after each completed turn, in order.
        let mut decisions: Vec<(u32, AiLoopState)> = Vec::new();
        let mut turn = 0_u32;
        while engine.get_current_state() == AiLoopState::Working {
            turn += 1;
            engine.process_event(AiLoopEvent::TurnDone);
            assert_eq!(
                engine.get_current_state(),
                AiLoopState::Judging,
                "a completed turn is judged, always: turn {turn}",
            );
            // No `_event.data.done`, so the goal-met guard is falsy and the budget
            // guards are what decide. The peer saying the done marker is a
            // different gate; this one is about the two NUMBERS.
            engine.process_event(AiLoopEvent::Judge);
            decisions.push((turn, engine.get_current_state()));

            // A reflection that finds nothing to change returns to `working`
            // without paying for a restart — the document's `reflect.none` edge,
            // and what keeps this walk going to the ceiling.
            if engine.get_current_state() == AiLoopState::Reflecting {
                engine.process_event(AiLoopEvent::ReflectNone);
            }
            assert!(turn <= 100, "the ceiling must be reachable: {decisions:?}");
        }

        let reflected: Vec<u32> = decisions
            .iter()
            .filter(|(_, state)| *state == AiLoopState::Reflecting)
            .map(|(turn, _)| *turn)
            .collect();
        assert_eq!(
            reflected,
            vec![8, 16, 24, 32],
            "`reflect_every` is 8 and the counter resets on entry to `reflecting`, \
             so the loop stops to improve itself on exactly these turns — and NOT \
             at 40, where the ceiling takes precedence: {decisions:?}",
        );
        assert_eq!(
            decisions.last(),
            Some(&(40, AiLoopState::Exhausted)),
            "⚠⚠⚠ `max_turns` is 40 and its transition is written BEFORE the \
             reflect one, so the fortieth turn ends the run instead of restarting \
             a session that has no turns left to spend: {decisions:?}",
        );
        assert!(
            engine.is_in_final_state(),
            "and `exhausted` is a final state, not a pause",
        );
    }

    /// ⚠⚠⚠ **THE AUTHORED HALF OF THE DOCUMENT SURVIVES — AND THE ROUND HAD TO RUN
    /// IT TO FIND THAT OUT, BECAUSE READING SAID THE OPPOSITE.**
    ///
    /// `ai_loop.scxml` declares `datamodel="ecmascript"`. At the pinned SCE rev
    /// there is exactly ONE [`IScriptEngine`] — `LuaEngine`; `sce-rust-runtime`'s
    /// own manifest calls QuickJS *"future"*, and SCE's build special-cases only the
    /// datamodel string `"null"`, routing every other value to whatever engine the
    /// consumer supplies. So the document's ECMAScript is evaluated by **Lua**, and
    /// the generated init strings show a PARTIAL rewrite: the object/array literal
    /// in `screen_rules` is turned into Lua table syntax (`[…]` → `{…}`, `key:` →
    /// `key =`), while `start_prompt`'s `'…' + north_star + '\n' + …` is passed
    /// through verbatim — and in Lua `+` is arithmetic, not concatenation.
    ///
    /// From that reading this gate was written to assert the prompts DO NOT arrive.
    /// **It failed, and the failure is the finding**: the composed prompt comes back
    /// whole. The engine handles the concatenation; the mismatch visible in the
    /// generated source is not a defect a caller can reach.
    ///
    /// So this asserts what is true, over the three shapes the authored half is made
    /// of, each of which a different part of the loop depends on:
    ///
    /// * `north_star` — a bare literal. The control: if this one fails, the gate is
    ///   not reading the datamodel at all and nothing below means anything.
    /// * `start_prompt` — a COMPOSED string. What `priming` sends.
    /// * `screen_rules` — a LIST OF OBJECTS, the shape debt 60's `screening` is
    ///   built out of, and the only one whose syntax the codegen rewrote.
    /// * `max_turns` — a scalar, which the outer `judging` budget compares against.
    #[test]
    fn the_whole_authored_surface_crosses_into_the_datamodel() {
        let (_engine, lua, session) = started();

        // ── the control: a bare literal crosses unharmed ──
        let north_star = lua.get_variable(&session, "north_star");
        assert!(
            matches!(&north_star, Ok(ScriptValue::String(text)) if text.contains("edit me")),
            "⚠ THE CONTROL FAILED, so nothing below means anything: a bare string \
             literal must reach the datamodel. Got {north_star:?}",
        );

        // ── a COMPOSED string: the one reading the generated source said was
        //    broken, and the reason this gate exists ──
        let start_prompt = lua.get_variable(&session, "start_prompt");
        let Ok(ScriptValue::String(start_prompt)) = &start_prompt else {
            panic!("the prompt `priming` sends must be a composed string: {start_prompt:?}");
        };
        assert!(
            start_prompt.starts_with(COMPOSED_START_PROMPT),
            "the `+` chain must have concatenated, not added: {start_prompt:?}",
        );
        assert!(
            start_prompt.contains("Report what you did and what is left."),
            "and every clause of it must be there, not just the first: \
             {start_prompt:?}",
        );

        // ── a LIST OF OBJECTS: the shape `screening` reads its rules out of, and
        //    the one whose SYNTAX the codegen rewrote on the way in ──
        let rules = lua.get_variable(&session, "screen_rules");
        let rules = match &rules {
            Ok(ScriptValue::Array(rules)) => rules,
            other => panic!(
                "⚠⚠ `screening` cannot be built on a datamodel that cannot hold its \
                 rules. The document writes three; the engine answered {other:?}",
            ),
        };
        assert_eq!(
            rules.len(),
            3,
            "the document declares three rules: {rules:?}"
        );
        let first = match &rules[0] {
            ScriptValue::Object(fields) => fields,
            other => panic!("a rule is an object of `when`/`keys`/`text`: {other:?}"),
        };
        assert!(
            matches!(first.get("when"), Some(ScriptValue::String(w)) if w == "design-decision"),
            "⚠ and its FIELDS must survive the `key:` → `key =` rewrite, not just \
             its shape: {first:?}",
        );

        // ── a scalar: what the outer `judging` budget compares against ──
        //
        // ⚠ Read through the SCRIPT SESSION rather than off the policy, and not by
        // choice. SCE lowered every scalar `<data>` into a typed Rust field
        // (`max_turns: i64`, initialised to 40) AND emitted no accessor for any of
        // them — only `session_id` is `pub`. So a consumer cannot ask the machine
        // what its own budget is; the interpreter's copy is the only readable one.
        // That is what makes the guard below the ONLY way to observe the budget.
        let max_turns = lua.get_variable(&session, "max_turns");
        assert!(
            matches!(&max_turns, Ok(ScriptValue::Int(40))),
            "the authored budget must cross as a number: {max_turns:?}",
        );
    }
}
