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
            ingress.contains(&AiLoopEvent::Brief),
            "⚠⚠ and so must the one thing a caller TELLS the machine rather than reports to it. \
             `brief` is how somebody who did not edit this file says what the run is for; a \
             machine that does not accept it from outside is one whose template nothing can fill \
             in, which is exactly the state this round found it in: {ingress:?}",
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
    /// * `start_prompt` — a COMPOSED string, and **as of this round the composition
    ///   is an `<assign>` in `priming`'s `onentry` rather than a `<data expr>`**. That
    ///   is a shape this gate had never driven, and the document's own caveat says a
    ///   shape not driven here is a shape nobody has measured — so the walk to
    ///   `priming` below is the point, not a detour around it.
    /// * `screen_rules` — a LIST OF OBJECTS, the shape debt 60's `screening` is
    ///   built out of, and the only one whose syntax the codegen rewrote.
    /// * `max_turns` — a scalar, which the outer `judging` budget compares against.
    #[test]
    fn the_whole_authored_surface_crosses_into_the_datamodel() {
        let (mut engine, lua, session) = started();

        // ── the control: a bare literal crosses unharmed ──
        let north_star = lua.get_variable(&session, "north_star");
        assert!(
            matches!(&north_star, Ok(ScriptValue::String(text)) if text.contains("edit me")),
            "⚠ THE CONTROL FAILED, so nothing below means anything: a bare string \
             literal must reach the datamodel. Got {north_star:?}",
        );

        // ── the SECOND control, and it is what makes the composition below a claim
        //    about `priming` rather than about `<data>`: nothing is composed yet.
        let unprimed = lua.get_variable(&session, "start_prompt");
        assert!(
            matches!(&unprimed, Ok(ScriptValue::String(text)) if text.is_empty()),
            "⚠ a machine that has not primed must hold no composed prompt, or the walk \
             below proves nothing about where the composition happens: {unprimed:?}",
        );

        // ── a COMPOSED string, built by an `<assign expr>` on the way into `priming` ──
        engine.process_event(AiLoopEvent::Start);
        assert_eq!(
            engine.get_current_state(),
            AiLoopState::Priming,
            "the control: the composition runs on entry to `priming`",
        );
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
        // ⚠⚠ AND ONE `<assign>` MUST HAVE SEEN THE ONE BEFORE IT. `done_instruction` is
        // composed first and the two working prompts end with it, so executable content
        // running out of document order would append the PREVIOUS entry's instruction —
        // correct for every entry but the first, and silent.
        assert!(
            start_prompt.trim_end().ends_with("MILESTONE REACHED"),
            "⚠⚠ `done_instruction` must have been composed BEFORE the prompt that ends \
             with it: {start_prompt:?}",
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

    /// ⚠⚠⚠ **WHICH SIDE OF THE DATAMODEL MANGLES A NON-ASCII STRING** — asked of the engine,
    /// because the driver's brief came back mojibake and a diagnosis read off either end would be
    /// a guess about the other.
    ///
    /// `OuterLoop::brief` sends a person's prose in as event data and reads it back out. An em
    /// dash went in and `â\u{80}\u{94}` came out: the three UTF-8 bytes of U+2014, each widened
    /// into its own `char`. That is a byte string being turned into a Rust `String` one byte at a
    /// time, and it can happen at either of two seams — the JSON payload becoming `_event.data`,
    /// or [`IScriptEngine::get_variable`] converting a Lua value back.
    ///
    /// This separates them, and the separation matters because only one of the two is reachable
    /// from a document. **`screen_rules` in the shipped template is Korean**, so this is not a
    /// hypothetical about a caller: it is about text `screening` will read the day it is built.
    ///
    /// ⚠ Whatever this reports is a fact about the PINNED SCE rev and belongs upstream rather than
    /// in a workaround here — see the workspace manifest's SCE block.
    #[test]
    fn a_non_ascii_string_says_which_seam_it_is_mangled_at() {
        let (_engine, lua, session) = started();

        // ── seam one: a literal in the DOCUMENT, initialised by `<data expr>` ──
        //
        // The template's own third rule, which is Korean prose a person wrote into this file.
        let rules = lua.get_variable(&session, "screen_rules");
        let Ok(ScriptValue::Array(rules)) = &rules else {
            panic!("the control: the rules must cross as a list at all: {rules:?}");
        };
        let ScriptValue::Object(first) = &rules[0] else {
            panic!("the control: a rule is an object: {:?}", rules[0]);
        };
        let Some(ScriptValue::String(text)) = first.get("text") else {
            panic!("the control: a rule carries a reply text: {first:?}");
        };
        assert!(
            text.starts_with("비용 무시하고"),
            "⚠⚠⚠ SEAM ONE: a non-ASCII literal AUTHORED IN THE DOCUMENT does not survive the \
             datamodel. Every `screening` rule this template ships is Korean, so the day that \
             state is built it would send an agent bytes nobody wrote. Got {text:?}",
        );

        // ── seam two: a string arriving as EVENT DATA, assigned by a transition ──
        //
        // `idle`'s `brief` transition is the one place this document takes a string from outside,
        // and it is the path `OuterLoop::brief` uses. Same engine, same session, same variable
        // kind — the ONLY difference from seam one is how the value got there.
        let mut engine = _engine;
        let sent = "북극성 — ship it";
        engine.raise_external(
            AiLoopEvent::Brief,
            &serde_json::json!({
                "north_star": sent,
                "milestone": "m",
                "reference": "r",
                "max_turns": 3,
                "reflect_every": 9,
            })
            .to_string(),
            "",
        );
        engine.step();
        let held = lua.get_variable(&session, "north_star");
        let Ok(ScriptValue::String(held)) = &held else {
            panic!("the control: the brief must have assigned something at all: {held:?}");
        };
        assert_ne!(
            held, sent,
            "⚠⚠⚠ UPSTREAM LANDED IT: a non-ASCII string now survives EVENT DATA. Delete this half, \
             widen `outer`'s brief gates back to prose in a person's own language, and drop \
             `a_brief_the_engine_cannot_carry_is_refused_rather_than_delivered`",
        );
        // ⚠ THE SHAPE OF THE DAMAGE, DERIVED rather than pasted, so a DIFFERENT breakage cannot be
        // read as this one. `json_to_lua_table` walks the payload with `bytes[i] as char`, which is
        // a Latin-1 decode of UTF-8 — every byte becomes the char of the same number and is then
        // re-encoded. Reproducing that here is what makes the diagnosis a claim rather than a
        // guess: if the mangling is ever something else, this stops matching.
        let latin1_widened: String = sent.bytes().map(char::from).collect();
        assert_eq!(
            held, &latin1_widened,
            "the damage must be exactly a Latin-1 widening of the UTF-8 bytes — that is the \
             mechanism at `sce-rust-lua`'s `json_to_lua_table`, and anything else is a different \
             defect wearing its symptom",
        );
    }
}
