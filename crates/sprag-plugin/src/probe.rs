//! **WHAT THIS CRATE HAS PROVEN ABOUT THE ENGINE IT RUNS ON** — the probes, and only the probes.
//!
//! # ⚠⚠⚠ Why a construct gets a probe before anything is built on it
//!
//! A generator's filters and a conformance suite's fixtures are facts about SOMEWHERE ELSE. The
//! `ai_loop` document carries the same warning about `===`, `JSON.stringify` and template
//! literals — every one of them promised by the engine's NAME and never shown to work at the
//! pinned rev. `probe_parent.scxml` asked that question of `<invoke>` and got a yes; this module
//! is where the answers live for the ones that need a RUN rather than a compile.
//!
//! ⚠ The `<invoke>` probe's own gate still lives beside the loop it was built for. This module
//! exists so the next probe does not have to.
//!
//! # ⚠⚠⚠⚠ A DRIVER CANNOT REACH AN `<invoke>`d CHILD — measured, and it refutes a planned design
//!
//! The composed shape this crate's probes were built to justify is *one topology as a CHILD, one
//! document per loop KIND as a PARENT that fills it in*. The probe below proved the two halves that
//! were asked for — a parent's `<param>` reaches the child's `<data>`, and the child's `<donedata>`
//! comes back — and **the question nobody asked was whether the DRIVER can still drive the child.**
//!
//! It cannot. At the pinned SCE the generated parent owns its child as a PRIVATE field with no
//! accessor, and the runtime's `Engine` publishes nothing about children at all:
//!
//! ```text
//! child_probe: Option<Box<sce_rust_runtime::Engine<super::probe_child_sm::ProbeChildPolicy>>>,
//! ```
//! ```text
//! error[E0616]: field `child_probe` of struct `ProbeParentPolicy` is private
//!   --> crates/sprag-plugin/src/probe.rs:48:38
//!    |
//! 48 |         let _child = engine.policy().child_probe.as_ref();
//!    |                                      ^^^^^^^^^^^ private field
//! ```
//!
//! **The line was written and compiled rather than reasoned about**, because a compiler's refusal is
//! stronger than a gate. What it costs is exact: an invoked `ai_loop` could not be read
//! (`OuterLoop::state()` would answer the PARENT's state), could not be pumped, and could not be
//! sent the events the driver raises — so the loop would be undriveable the moment it became a
//! child. ⚠ There IS a child→parent route (`parent_external_queue`, drained before each tick), which
//! is what `<donedata>` rides; there is no route the other way that a driver can use.
//!
//! ⚠⚠ **THIS IS NOT AN ARGUMENT AGAINST THE SPLIT, only against one arrangement of it.** The
//! template must stay the machine the driver holds. A kind's decisions can still arrive by
//! `<invoke>` — with the template as the PARENT and the kind document as a short-lived CHILD that
//! finishes at once, handing its decisions back through `<donedata>`. Both halves of that are
//! exactly what the probe below already proved, in that direction.
//!
//! ⚠ And one consequence to weigh before building it: the generated child is a CONCRETE TYPE chosen
//! at codegen, so `srcexpr` cannot select a kind at runtime. A template that invokes its decisions
//! names ONE filename, and every repository adopting it supplies a document of that name.
//!
//! # ⚠⚠⚠⚠ A HOST CANNOT NAME ITS OWN `<send>` OR `<invoke>` TYPE — measured, and it refutes another
//!
//! Register item 470's second stage would have let the document say `<send type="sprag">` and this
//! crate carry the act out, so that what a run DOES stopped being a `match` in `outer.rs`. The type
//! registry is closed at the pinned engine: an unknown `type` compiles, then raises
//! `error.execution` when the state is entered, and takes the rest of that executable block with
//! it. There is no registration point — the one send-side hook is bound to a type the engine
//! already knows, and the invoke side has none.
//!
//! ⚠⚠ **The build says nothing**, which is the part worth carrying past this round: a document
//! naming a type nobody implements is green on every compile and in every test that does not enter
//! the state. So stage 2 is an upstream request rather than a design, and acts stay in the driver
//! while the document keeps saying WHICH act. Stage 1's guard is untouched by this — a `cond` is
//! the datamodel's and needs no registry.
//!
//! # ⚠⚠⚠⚠⚠ A MACHINE CAN SAY WHERE IT IS AND CANNOT BE PUT THERE — measured, and it is what run
//! persistence waits on
//!
//! Register item 549, and the reason 543 (*a daemon restarted mid-run brings that run back
//! RUNNING*) and 544's stage 3b are not being built. The engine publishes its configuration —
//! `get_current_state`, `get_active_states` — and offers no way to enter at one: `initialize()` is
//! the only door and it enters the DOCUMENT's initial configuration, running `<onentry>` on the
//! way. So a restarted host can say exactly where a run was and can only replay it from the start,
//! which for `ai_loop.scxml` means re-sending prompts its agent already answered.
//!
//! ⚠⚠⚠ **THE ANSWER USED TO BE A SOURCE READING AND IS NOW TWO GATES.** It was measured by reading
//! `backends/rust/runtime/src` three times — at registration, at the 550 bump, and while writing
//! `SCE-PR90`. A reading is true of the file somebody opened and nothing re-opens it at the next
//! pin, so the finding could only ever age silently. The two gates ask the same question of the
//! pinned crate instead: one of the TYPE (no inherent door of these names resolves) and one of a
//! RUN (the door that exists re-runs entry actions). The day SCE ships the door, the sweep says so
//! rather than a person remembering to look.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};

    use crate::sm::probe_parallel_sm::{
        ProbeParallelEvent, ProbeParallelPolicy, ProbeParallelState,
    };
    use crate::sm::probe_send_type_sm::{
        ProbeSendTypeEvent, ProbeSendTypePolicy, ProbeSendTypeState,
    };

    /// ⚠⚠⚠⚠⚠ **A HOST CANNOT REGISTER ITS OWN `<send>` OR `<invoke>` TYPE — measured, and it
    /// refutes register item 470's second stage as a design.**
    ///
    /// # What was being decided
    ///
    /// Stage 1 moved one DECISION into the document. Stage 2 would have moved the ACTS: the
    /// document says `<send type="sprag" event="…"/>`, this crate registers a handler for that
    /// type, and what a run does stops being a `match` in `outer.rs` at all. Every part of that
    /// rests on one question nobody had asked the engine.
    ///
    /// # ⚠⚠⚠⚠ The answer, in the order it matters
    ///
    /// 1. **Codegen ACCEPTS an unknown type.** `probe_send_type.scxml` carries
    ///    `type="x-sprag-host"` on both a `<send>` and an `<invoke>`, it is in `build.rs`'s list,
    ///    and the crate builds. **So the build says nothing**, which is the sharp half: a document
    ///    naming a type no one implements is green on every compile and green in every test that
    ///    does not enter the state.
    /// 2. **The runtime refuses it** — `error.execution`, once per site, at the moment the state is
    ///    entered.
    /// 3. **The named event never arrives.** That is the difference between *refused* and
    ///    *ignored*: an engine that dropped the `type` and delivered internally would let a
    ///    document call for an act nobody carries out and look like it worked.
    /// 4. **And the failure takes the rest of the block with it** — see the fixture's own note. An
    ///    unsupported `type` is an error in executable content, so every action after it in that
    ///    `onentry` is abandoned. A document that put a custom send first would lose the work
    ///    beside it, not just the send.
    ///
    /// ⚠ There is no registration point to reach for. The pinned runtime's one send-side extension
    /// is `Engine::set_http_send_callback`, which is bound to `BasicHTTPEventProcessor` — a type
    /// the engine already knows — and there is no invoke-side equivalent at all.
    ///
    /// # ⚠⚠⚠⚠⚠ THE CONTROL IS THE ONLY REASON ANY OF THAT IS TRUE
    ///
    /// This case reached a WRONG finding twice before the control let it through, and both are
    /// worth carrying because both looked exactly like the answer:
    ///
    /// - **One `step()` read as *refused*.** The `error.execution` had been processed and the
    ///   control send had not, so `landed == 0` beside `plain == 0` looked like the typed send
    ///   being rejected. It was a queue that had not been drained. `tick()` polls the scheduler;
    ///   `step()` runs a macrostep without it, and a `<send>` goes through the scheduler even
    ///   with no delay.
    /// - **The control ordered FIRST never ran**, because of finding 4 above — so `plain == 0`
    ///   read as *`<send>` does not deliver in this fixture at all*, which would have made the
    ///   whole probe unanswerable. Running the control ALONE is what separated the two, and the
    ///   order in the document is now load-bearing.
    ///
    /// ⚠⚠ Neither mistake was visible from the passing side. Had the control been left out — or
    /// left as the untyped `<send>` it started as, which distinguishes nothing — this case would
    /// have asserted `landed == 0` and recorded a true conclusion on evidence that did not support
    /// it.
    ///
    /// # ⚠⚠ What this costs, stated rather than implied
    ///
    /// Item 470 stage 2 is not a design at the pinned rev; it is a request filed upstream, which is
    /// this workspace's rule for an SCE gap. Acts stay in the driver, and the document keeps saying
    /// WHICH act rather than carrying it. Stage 1's guard is unaffected — a `cond` is evaluated by
    /// the datamodel and needs no registry.
    #[test]
    fn a_host_cannot_register_its_own_send_or_invoke_type() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<ProbeSendTypePolicy>, name: &str| match engine
            .policy()
            .script_engine
            .get_variable(&session, name)
        {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
        };

        // ⚠⚠⚠ STEPPED TO QUIESCENCE RATHER THAN ONCE, and the control below is what taught this
        // case that it had to be. One `step()` had already processed the `error.execution` while
        // the untyped send was still queued — so a single step read as *the typed send was refused
        // and the plain one never arrived*, which is exactly the false reading the control exists
        // to catch. It caught it here, on the first run, before any of it was written down.
        //
        // ⚠⚠ `tick()` RATHER THAN `step()`, and that is the second thing the control taught. A
        // `<send>` goes through the SCHEDULER even with no delay, and `step()` runs a macrostep
        // without polling it — so an untyped send sat there while the `error.execution` raised
        // beside it had already been processed.
        for _ in 0..8 {
            engine.tick();
        }

        // ── THE CONTROL FIRST: `<send>` DELIVERS IN THIS DOCUMENT AT ALL ──
        assert_eq!(
            count(&engine, "plain"),
            1,
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, SO THIS PROBE ANSWERS NOTHING. The untyped `<send>` beside \
             the typed one did not come back either — so `landed` being zero below would mean \
             *`<send>` does not deliver here*, which is a fact about this fixture and not about \
             types. Fix the fixture before reading anything else in this case",
        );

        // ── THE TYPED SEND: REFUSED, AND REFUSED RATHER THAN IGNORED ──
        assert_eq!(
            count(&engine, "errors"),
            1,
            "⚠⚠⚠⚠ a `<send>` carrying a type nothing implements must raise `error.execution` — \
             W3C SCXML 6.2. It did not, so this engine is doing something with the type that is \
             neither supporting it nor refusing it, and item 470 stage 2 needs to know WHAT before \
             anything is built on it",
        );
        assert_eq!(
            count(&engine, "landed"),
            0,
            "⚠⚠⚠⚠⚠ THE TYPE WAS IGNORED RATHER THAN REFUSED, which is the dangerous answer and the \
             one this case exists to tell apart. The event arrived internally, so a document could \
             name an act NOBODY carries out and every test would pass — the send would look \
             delivered because it was, to the wrong place",
        );

        // ── AND THE INVOKE SIDE IS A SECOND REGISTRY, SO IT IS ASKED SEPARATELY ──
        engine.process_event(ProbeSendTypeEvent::Go);
        assert_eq!(
            count(&engine, "errors"),
            2,
            "⚠⚠⚠⚠ `<invoke>` PICKS AN INVOKER AND `<send>` PICKS AN EVENT I/O PROCESSOR — two \
             registries, and this asks the second. A host that could reach this one and not the \
             other would change item 470 stage 2 from refused to narrowed, which is a different \
             answer and worth a round",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AND WITH A HANDLER REGISTERED, THE ACT REACHES THIS CRATE** — register item 483,
    /// consumed at the engine rev where it stopped being true.
    ///
    /// # What changed, and what did not
    ///
    /// The case above is the same document with NOTHING registered, and it still reads a refusal.
    /// That is not a leftover: SCE's registry has two halves that must agree — the build declares
    /// the types it serves (`build.rs`'s `HOST_TYPES`, so codegen emits a dispatch rather than a
    /// refusal) and the run registers a handler for each. **A declared type nobody serves raises
    /// `error.execution` exactly as an undeclared one does**, because from the document's side an
    /// act nobody performed is one fact either way.
    ///
    /// So the two cases are one axis apart — the registration — and they are what says this crate
    /// reached the act rather than the engine having started ignoring types.
    ///
    /// # ⚠⚠⚠ What this unblocks, stated rather than done
    ///
    /// Item 470 stage 2 (*the document names the ACT and this crate carries it out*) was filed
    /// upstream because no registration point existed. It exists now. The design is still a round's
    /// work — an act vocabulary, a refusal for one nobody serves, the driver's acts moving out of
    /// `outer.rs` — and none of it is done here. This case proves the road, and the register entry
    /// for stage 2 is where the road gets used.
    #[test]
    fn a_host_serves_its_own_send_and_invoke_type_once_it_registers_one() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));

        // ⚠⚠ REGISTERED BEFORE `initialize`, because the send is in an `onentry` of the INITIAL
        // state: a handler registered afterwards would be registered after the act it is for.
        let sends = Arc::new(AtomicUsize::new(0));
        let invokes = Arc::new(AtomicUsize::new(0));
        engine.register_event_processor("x-sprag-host", {
            let sends = Arc::clone(&sends);
            move |request| {
                sends.fetch_add(1, Ordering::SeqCst);
                // ⚠ The document's own event name, handed back — the request/reply shape. A
                // handler that answered a name of its own would be inventing the document's
                // vocabulary, which is the thing stage 2 must not do.
                //
                // ⚠⚠ ONE-ELEMENT LIST, and the list is the engine's rather than this crate's
                // taste: SCE widened the reply at `084dfdbf` so an act can produce SEVERAL events
                // in a stated order. This handler produces exactly one, and says so by length —
                // see `a_host_act_can_answer_with_two_events_in_the_order_it_names` for the case
                // the widening was made for.
                vec![sce_rust_runtime::host_processor::HostSendResponse {
                    event_name: request.event_name,
                    event_data: String::new(),
                }]
            }
        });
        engine.register_invoker("x-sprag-host", {
            let invokes = Arc::clone(&invokes);
            move |_event| {
                invokes.fetch_add(1, Ordering::SeqCst);
                None
            }
        });
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<ProbeSendTypePolicy>, name: &str| match engine
            .policy()
            .script_engine
            .get_variable(&session, name)
        {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
        };

        // ⚠ `tick()` for the same reason the case above uses it: a `<send>` goes through the
        // scheduler even with no delay, and `step()` runs a macrostep without polling it.
        for _ in 0..8 {
            engine.tick();
        }

        assert_eq!(
            count(&engine, "plain"),
            1,
            "⚠⚠⚠ THE CONTROL, unchanged: the untyped `<send>` beside the typed one must still \
             deliver, or nothing below is about types",
        );
        assert_eq!(
            sends.load(Ordering::SeqCst),
            1,
            "⚠⚠⚠⚠⚠ ITEM 483: the `<send type=\"x-sprag-host\">` must reach THIS CRATE's handler. \
             Zero means the declaration and the registration did not meet — check that `build.rs` \
             still declares the type, because a registration for a type the build did not declare \
             is inert by design",
        );
        assert_eq!(
            count(&engine, "landed"),
            1,
            "⚠⚠⚠⚠ and the handler's reply must come back as the event the DOCUMENT named — a \
             handler that is called but whose answer never lands would make an act look performed \
             to this crate and refused to the machine",
        );
        assert_eq!(
            count(&engine, "errors"),
            0,
            "⚠⚠⚠⚠⚠ AND NO `error.execution`, which is the half that separates *served* from \
             *ignored*. The case above reads 1 here on the same document with nothing registered.",
        );

        // ── THE INVOKE SIDE, WHICH IS A SECOND REGISTRY AND SO A SECOND CLAIM ──
        engine.process_event(ProbeSendTypeEvent::Go);
        for _ in 0..8 {
            engine.tick();
        }
        assert_eq!(
            invokes.load(Ordering::SeqCst),
            1,
            "⚠⚠⚠⚠ `<invoke type=\"x-sprag-host\">` must reach the invoker this crate registered. \
             An engine that served the send half and not this one would narrow item 470 stage 2 \
             rather than open it",
        );
        assert_eq!(
            count(&engine, "errors"),
            0,
            "⚠⚠⚠ and the invoke raised no refusal either",
        );
    }

    /// ⚠⚠⚠⚠⚠ **CAN A TRANSITION ASK THIS HOST FOR AN ACT, WITH ITS ARGUMENTS, THE WAY A STATE
    /// ENTRY CAN?** — the question register item 470's stage 2 stopped at, asked of the engine
    /// rather than reasoned about.
    ///
    /// # What is being decided
    ///
    /// Stage 2 moved five acts out of the driver, one per `<onentry>` prompt the loop has, and
    /// `Owed::on` now answers for exactly one thing: `working`'s arm. That arm survives because
    /// `prompt.turn` is a **transition** send on three of the four edges into one state — the
    /// arrival cannot say what is owed, so no state entry can declare it. Either a transition can
    /// declare an act to this host, and the last arm follows the other five; or it cannot, and
    /// what is left of stage 2 is a different design (or a request filed upstream).
    ///
    /// ⚠⚠⚠ **THE PRECEDENT FOR ASKING RATHER THAN REASONING IS THIS FILE'S OWN.** The case at the
    /// top of this module recorded stage 2 as REFUTED on 2026-08-20, correctly, about the rev
    /// pinned that day — and a pin bump overturned it hours later. *A NO is as useful as a YES* is
    /// this module's rule precisely because both age.
    ///
    /// # ⚠⚠⚠⚠ The control, and why a bare green would prove nothing without it
    ///
    /// `edging`'s ENTRY declares the same act with the same argument, one step before the edge
    /// does. One handler, one run, that order. Without it a zero on the edge reads equally as *a
    /// transition cannot* and as *arguments do not cross into this document at all*, and those are
    /// different findings with different consequences.
    ///
    /// ⚠⚠ **AND THE ARGUMENT'S VALUE IS THE DOCUMENT'S, NOT THIS TEST'S.** `word` carries a
    /// `<data>` the document composes, so a handler reading it back has read an EXPRESSION
    /// evaluated at send time. A literal in the `<param>` would have been satisfied by an engine
    /// that copied the attribute through without evaluating anything, which is precisely the
    /// half `<param name="text" expr="…"/>` depends on everywhere stage 2 already shipped.
    ///
    /// ⚠ **A NO HAS THREE SHAPES AND THEY ARE TOLD APART**: refused (`errors` rises), reached with
    /// an EMPTY argument bag (the handler was called and `word` was not there), or delivered
    /// internally with the type ignored (the reply arrives and the handler was never called) —
    /// the last being the dangerous one the module's first case exists to name.
    #[test]
    fn a_transition_can_ask_this_host_for_an_act_and_its_arguments_reach_it() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        /// What the handler saw, in the order the engine called it.
        type Seen = Vec<(String, HashMap<String, Vec<String>>)>;

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));

        let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Vec::new()));
        // ⚠⚠ REGISTERED BEFORE `initialize`, for the reason the case above states: the initial
        // state's own entry already declares an act, and a handler registered after it is a
        // handler registered after the act it was for.
        engine.register_event_processor("x-sprag-host", {
            let seen = Arc::clone(&seen);
            move |request| {
                seen.lock()
                    .expect("the record")
                    .push((request.event_name.clone(), request.params.clone()));
                // ⚠ The document's own event name handed back — the request/reply shape. A handler
                // naming an event of its own would be inventing the document's vocabulary.
                vec![sce_rust_runtime::host_processor::HostSendResponse {
                    event_name: request.event_name,
                    event_data: String::new(),
                }]
            }
        });
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<ProbeSendTypePolicy>, name: &str| match engine
            .policy()
            .script_engine
            .get_variable(&session, name)
        {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number this document keeps: {other:?}"),
        };
        let word = |engine: &Engine<ProbeSendTypePolicy>| match engine
            .policy()
            .script_engine
            .get_variable(&session, "carried")
        {
            Ok(ScriptValue::String(held)) => held,
            other => panic!("`carried` must be the sentence this document composes: {other:?}"),
        };
        let composed = word(&engine);
        assert!(
            !composed.is_empty(),
            "⚠⚠ the document must compose an argument for either measurement to mean anything",
        );

        // ── THE CONTROL: the same act, the same argument, declared on a state ENTRY ──
        engine.process_event(ProbeSendTypeEvent::ProbeEnter);
        for _ in 0..8 {
            engine.tick();
        }
        let after_entry: Seen = seen.lock().expect("the record").clone();
        let entry: Vec<&(String, HashMap<String, Vec<String>>)> = after_entry
            .iter()
            .filter(|(named, _)| named == "carried.entry")
            .collect();
        assert_eq!(
            entry.len(),
            1,
            "⚠⚠⚠ THE CONTROL DID NOT REACH THIS HOST, so nothing below is about transitions: an \
             `<onentry>` act with a `<param>` is the shape five shipped acts already use. Saw \
             {after_entry:?}",
        );
        assert_eq!(
            entry[0].1.get("word").map(Vec::as_slice),
            Some([composed.clone()].as_slice()),
            "⚠⚠⚠ and the control's ARGUMENT must arrive, evaluated: {:?}",
            entry[0].1,
        );
        assert_eq!(
            count(&engine, "entry_landed"),
            1,
            "⚠⚠ and the document must hear the reply, or the act went somewhere this run cannot see",
        );

        // ── THE QUESTION: the same act, the same argument, declared on a TARGETED transition ──
        engine.process_event(ProbeSendTypeEvent::ProbeTake);
        for _ in 0..8 {
            engine.tick();
        }
        let after_edge: Seen = seen.lock().expect("the record").clone();
        let edge: Vec<&(String, HashMap<String, Vec<String>>)> = after_edge
            .iter()
            .filter(|(named, _)| named == "carried.edge")
            .collect();
        assert_eq!(
            count(&engine, "errors"),
            0,
            "⚠⚠⚠⚠⚠ A TRANSITION'S HOST SEND WAS REFUSED. That is one of the three shapes of NO, \
             and it is the one that closes the road: `prompt.turn` cannot leave the driver by this \
             route, and what is left of item 470's stage 2 needs a different design. Saw \
             {after_edge:?}",
        );
        assert_eq!(
            edge.len(),
            1,
            "⚠⚠⚠⚠⚠ A TRANSITION'S HOST SEND DID NOT REACH THIS HOST. With the control above green, \
             this is a fact about TRANSITIONS rather than about this document — the second shape \
             of NO. ⚠ If `edge_landed` is nonetheless 1, the type was IGNORED and the event was \
             delivered internally, which is the dangerous answer this module's first case names: \
             a document could declare an act nobody carries out and look served. Saw \
             {after_edge:?}, `edge_landed` = {}",
            count(&engine, "edge_landed"),
        );
        assert_eq!(
            edge[0].1.get("word").map(Vec::as_slice),
            Some([composed.as_str().to_owned()].as_slice()),
            "⚠⚠⚠⚠⚠ THE ACT REACHED THIS HOST WITH NO ARGUMENTS, which is the third shape of NO and \
             the narrowest: a transition may declare an act but cannot carry one. `prompt.turn` \
             is a SENTENCE, so an act without arguments would move the act's NAME into the \
             document and leave the driver deciding what it says — half the move, and the half \
             item 470 is not about. Got {:?}",
            edge[0].1,
        );
        assert_eq!(
            count(&engine, "edge_landed"),
            1,
            "⚠⚠⚠⚠ and the document must hear the reply from a transition's act as it hears the \
             entry's, or a run could not route on what the act answered — which is what \
             `prompt.sent` does for every prompt this loop delivers",
        );

        // ── THE THIRD SHAPE: the same act on a TARGETLESS transition, which is what stage 3 needs ──
        //
        // ⚠⚠⚠⚠⚠ It is a DIFFERENT question from the one above, not an easier retelling of it.
        // Stage 3's subject is `pump`'s state match, and a state ENTRY cannot answer it: `working`
        // is pumped many times over one entry, so *what does this pass do* has to be askable on
        // every pass. W3C SCXML 3.13's targetless transition is the only shape that answers a
        // driver's question without moving the machine and without re-running an `<onentry>` that
        // would re-type the prompt the state was entered with.
        //
        // ⚠⚠ The state must not have been RE-ENTERED afterwards, and that is counted rather than
        // inferred from where the machine is. An engine that treated a missing `target` as a self
        // transition would leave `get_current_state` reading exactly what it read before — so the
        // obvious comparison is blind to the one failure this shape is chosen to avoid, and the
        // document counts its own `<onentry>` instead.
        let standing_from = engine.get_current_state();
        let entered_before = count(&engine, "entered");
        assert_eq!(
            entered_before, 1,
            "⚠⚠⚠ THE CONTROL'S OWN PREMISE: the targeted edge above entered `edged` exactly once, \
             and a counter that is not already moving cannot show a re-entry below",
        );
        engine.process_event(ProbeSendTypeEvent::ProbeStand);
        for _ in 0..8 {
            engine.tick();
        }
        let after_standing: Seen = seen.lock().expect("the record").clone();
        let standing: Vec<&(String, HashMap<String, Vec<String>>)> = after_standing
            .iter()
            .filter(|(named, _)| named == "carried.standing")
            .collect();
        assert_eq!(
            count(&engine, "errors"),
            0,
            "⚠⚠⚠⚠⚠ A TARGETLESS TRANSITION'S HOST SEND WAS REFUSED. With the two shapes above \
             green this is a fact about targetless transitions alone, and it closes the road \
             register item 470's stage 3 was going to take: `pump`'s state match cannot become a \
             question the document answers per pass. Saw {after_standing:?}",
        );
        assert_eq!(
            standing.len(),
            1,
            "⚠⚠⚠⚠⚠ A TARGETLESS TRANSITION'S HOST SEND DID NOT REACH THIS HOST. ⚠ If \
             `standing_landed` is nonetheless 1 the type was IGNORED and the event was delivered \
             internally — the dangerous answer this module's first case names, because a document \
             could then declare an act nobody carries out and look served. Saw {after_standing:?}, \
             `standing_landed` = {}",
            count(&engine, "standing_landed"),
        );
        assert_eq!(
            standing[0].1.get("word").map(Vec::as_slice),
            Some([composed.as_str().to_owned()].as_slice()),
            "⚠⚠⚠⚠⚠ THE ACT REACHED THIS HOST WITH NO ARGUMENTS. That is the narrowest NO and it \
             still closes the road: stage 3's act carries WHICH effect the pass is for, so an act \
             that cannot carry an argument would move the question's NAME into the document and \
             leave the driver's state match deciding the answer. Got {:?}",
            standing[0].1,
        );
        assert_eq!(
            engine.get_current_state(),
            standing_from,
            "⚠⚠⚠⚠ THE MACHINE MOVED ON A TRANSITION WITH NO `target`, so this shape cannot be what \
             a driver asks a question with: a pass would relocate the run it was only looking at",
        );
        assert_eq!(
            count(&engine, "entered"),
            entered_before,
            "⚠⚠⚠⚠⚠ THE STATE WAS RE-ENTERED. Then a missing `target` is a SELF transition here, \
             the assertion above is worthless — a self transition leaves the current state \
             unchanged too — and the road closes for the reason stage 3 picked this shape to \
             avoid: `<onentry>` would run once per pass, so every state that greets its peer with \
             a sentence would re-type it on every look the driver takes",
        );
        assert_eq!(
            count(&engine, "standing_landed"),
            1,
            "⚠⚠⚠ and the document must hear the reply here too: a driver that asks its document \
             what a pass is for is asking a question, and a question whose answer the machine \
             cannot route on is half a question",
        );

        // ── THE FOURTH SHAPE: the same act on a top-level `<final>`'s ENTRY ──
        //
        // ⚠⚠⚠⚠⚠ REGISTER ITEM 470's STAGE 3, THIRD MATCH. `AiLoop::ended` maps each of the loop's
        // seven endings to a `Verdict` with a twenty-eight-arm state match. The only road that
        // does not put that answer straight back into Rust is the DOCUMENT declaring, at the
        // ending itself, which word it publishes — and an ending's entry is the one place that can
        // say it, because by the time the driver asks, the machine is already there.
        //
        // ⚠⚠ AN ENDING IS A DIFFERENT REGIME FROM EVERY ACT THIS CRATE SHIPS, which is why none of
        // the three shapes above stands in for it: entering a top-level `<final>` COMPLETES the
        // machine. No further macrostep is owed, the reply has nowhere to be delivered, and an
        // engine that tore the session down on completion would drop the send unsent.
        //
        // ⚠ SO THE DOCUMENT IS NOT THE WITNESS HERE — it has no transitions left to hear anything
        // with. The handler's own record is, and the reply is expected to go nowhere. What is
        // being asked is only whether the handler is CALLED, carrying its argument.
        let ending_before = seen.lock().expect("the record").len();
        engine.process_event(ProbeSendTypeEvent::ProbeEnd);
        for _ in 0..8 {
            engine.tick();
        }
        let after_ending: Seen = seen.lock().expect("the record").clone();
        assert!(
            engine.is_in_final_state(),
            "⚠⚠⚠ THE FIXTURE: `probe.end` must actually complete this machine, or what is measured \
             below is an ordinary state entry wearing a final's name — the whole question is what \
             happens in the regime where the machine is OVER. Saw {:?}",
            engine.get_current_state(),
        );
        // ⚠⚠⚠⚠⚠ THE CONTROL, AND IT IS DECLARED IN THE SAME MACROSTEP AS THE QUESTION. The first
        // draft asserted only that the record had GROWN, and that was not a control at all: the
        // ending's own act was the only send in this step, so the control and the question were
        // one measurement wearing two names — a control that shares its failure with the defect,
        // which this workspace has already paid to learn. The edge into the ending now carries an
        // act of the shape proven green two measurements above, so *the handler stopped being
        // called* and *a final cannot ask* land on different assertions.
        let leaving: Vec<&(String, HashMap<String, Vec<String>>)> = after_ending
            .iter()
            .filter(|(named, _)| named == "carried.leaving")
            .collect();
        assert_eq!(
            leaving.len(),
            1,
            "⚠⚠⚠⚠ THE CONTROL FAILED — a TARGETED transition's act, the shape measured green two \
             assertions above, did not reach this host in the macrostep that ends the machine. So \
             nothing below is about finals: what this run found is that the handler stops being \
             called once an ending is in the transition's target, which is a wider finding and a \
             different one. Saw {after_ending:?}, record was {ending_before} long before",
        );
        let ending: Vec<&(String, HashMap<String, Vec<String>>)> = after_ending
            .iter()
            .filter(|(named, _)| named == "carried.final")
            .collect();
        assert_eq!(
            ending.len(),
            1,
            "⚠⚠⚠⚠⚠ AN ENDING'S ENTRY ACT DID NOT REACH THIS HOST. With all three shapes above \
             green this is a fact about FINALS alone, and it closes the road item 470's stage 3 \
             would take for `AiLoop::ended`: which `Verdict` an ending publishes cannot become the \
             document's to say by this route, and that match stays in Rust with this measurement \
             beside it in the register. Saw {after_ending:?}",
        );
        assert_eq!(
            ending[0].1.get("word").map(Vec::as_slice),
            Some([composed.as_str().to_owned()].as_slice()),
            "⚠⚠⚠⚠⚠ THE ENDING'S ACT REACHED THIS HOST WITH NO ARGUMENTS — the narrowest NO, and it \
             still closes the road: the act must carry WHICH word the ending publishes, so an act \
             that cannot carry an argument moves the question's name into the document and leaves \
             the driver's state match deciding the answer. Got {:?}",
            ending[0].1,
        );
    }

    /// ⚠⚠⚠⚠⚠ **CAN A NUMBER CROSS AS A `<param>`, AND DO ZERO, NEGATIVE AND NIL STAY TELLABLE
    /// APART?** — the question register item 470's last three datamodel back doors turn on.
    ///
    /// # What is being decided
    ///
    /// `expecting()`, `consenting()` and `ready_within()` read `<data>` straight out of the script
    /// session with TYPED getters. Shutting those doors the way `service_needle`'s was shut means
    /// the values arrive as a host act's arguments instead — and a host act's arguments are
    /// STRINGS. Three distinctions have to survive that crossing or the move destroys meaning:
    ///
    /// ⚠⚠ **TWO OF THE THREE ARE NOW SHUT ON THIS ANSWER** (2026-08-26): `ready_within()` went
    /// first as `within`, and `expecting()` followed as the `awaits`/`stills` pair — both riding
    /// the `pass.do` act that is already raised once per pass. `consenting()` is the one left, and
    /// it is the one this measurement does NOT cover: its value is a LIST OF OBJECTS rather than a
    /// number, so it needed an answer of its own.
    ///
    /// ⚠⚠⚠ **AND THE SENTENCE THAT STOOD HERE SAID A `<param>` CANNOT CARRY ONE, WHICH WAS A GUESS
    /// AND IS REFUTED.** See `a_clause_list_crossing_as_an_argument` below, written the next round
    /// to ask rather than assume: the list crosses as JSON, a repeated name keeps every value in
    /// order, and indexing selects two levels deep. ⇒ **This module's own rule caught its own
    /// author: a road called shut by reading is a road nobody measured.**
    ///
    /// * **zero is a real answer, not an absence.** `await_person_ms = 0` means NOBODY IS
    ///   EXPECTED; `handback_still_ms = 0` means a person who takes the pane KEEPS it. Both are
    ///   decisions the file is entitled to state.
    /// * **nil is a different real answer** — the document declares no bound of its own.
    /// * **a negative is refused today**, so it has to stay tellable to be refusable.
    ///
    /// ⚠⚠ **A NO IS THE USEFUL ANSWER TOO**, and this module's rule: if the three collapse, the
    /// doors stay open and the register says WHY on a measurement rather than on a preference.
    ///
    /// ⚠ Whatever the engine answers is recorded rather than demanded — the assertions below say
    /// what was measured, and the one that matters is that the four readings are DISTINCT.
    #[test]
    fn a_number_crosses_as_an_argument_and_zero_is_not_nil() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        type Seen = Vec<(String, HashMap<String, Vec<String>>)>;

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));
        let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Vec::new()));
        engine.register_event_processor("x-sprag-host", {
            let seen = Arc::clone(&seen);
            move |request| {
                seen.lock()
                    .expect("the record")
                    .push((request.event_name.clone(), request.params.clone()));
                vec![sce_rust_runtime::host_processor::HostSendResponse {
                    event_name: request.event_name,
                    event_data: String::new(),
                }]
            }
        });
        engine.initialize();

        // The numbers are declared on `edged`'s neighbour, so the machine is walked there first.
        engine.process_event(ProbeSendTypeEvent::ProbeEnter);
        engine.process_event(ProbeSendTypeEvent::ProbeTake);
        for _ in 0..8 {
            engine.tick();
        }
        engine.process_event(ProbeSendTypeEvent::ProbeCount);
        for _ in 0..8 {
            engine.tick();
        }

        let after: Seen = seen.lock().expect("the record").clone();
        let counted: Vec<&(String, HashMap<String, Vec<String>>)> = after
            .iter()
            .filter(|(named, _)| named == "carried.count")
            .collect();
        assert_eq!(
            counted.len(),
            1,
            "⚠⚠⚠ THE CONTROL: the act must have reached this host at all, or nothing below is \
             about numbers. Saw {after:?}",
        );
        let got = |name: &str| counted[0].1.get(name).map(Vec::as_slice);

        assert_eq!(
            got("counted"),
            Some(["42".to_owned()].as_slice()),
            "⚠⚠⚠⚠ A NUMBER MUST CROSS AS ITS OWN DIGITS. If it arrives as `42.0`, as an empty \
             string, or not at all, every millisecond bound item 470 wants to move would have to \
             be re-derived at the parse site. Got {:?}",
            counted[0].1,
        );
        assert_eq!(
            got("zeroed"),
            Some(["0".to_owned()].as_slice()),
            "⚠⚠⚠⚠⚠ **ZERO MUST NOT ARRIVE AS AN ABSENCE.** `await_person_ms = 0` is the file \
             saying NOBODY IS EXPECTED and `handback_still_ms = 0` is it saying a person who takes \
             the pane KEEPS it — both real decisions. If zero reads the same as nil here, moving \
             those keys onto an act would make one of the two answers unsayable from the file that \
             owns the decision, and the door stays open. Got {:?}",
            counted[0].1,
        );
        assert_eq!(
            got("negated"),
            Some(["-1".to_owned()].as_slice()),
            "⚠⚠⚠ A NEGATIVE MUST STAY TELLABLE, because it is REFUSED today: a reader that could \
             not see the sign would accept a bound the product rejects. Got {:?}",
            counted[0].1,
        );
        // ⭐ MEASURED 2026-08-26: nil arrives as the EMPTY STRING, `Some([""])`, which is distinct
        // from zero's `Some(["0"])`. Asserted as a difference rather than as `Some([""])` because
        // what the road needs is that the two are TELLABLE APART — the exact spelling of nil is the
        // engine's to choose and a gate pinned to it would go red for a harmless change.
        assert_ne!(
            got("blanked"),
            got("zeroed"),
            "⚠⚠⚠⚠⚠ **NIL AND ZERO ARRIVED AS THE SAME THING, WHICH CLOSES THE ROAD.** *The \
             document declares no bound* and *the document declares a bound of zero* are different \
             instructions, and a crossing that folds them cannot carry `await_person_ms` or \
             `handback_still_ms` at all. Both read {:?}",
            got("zeroed"),
        );
    }

    /// ⚠⚠⚠⚠⚠ **CAN A LIST OF OBJECTS CROSS AS A `<param>`, AND IF NOT, CAN A REPEATED NAME CARRY
    /// THE PARTS?** — the question register item 470's LAST datamodel back door turns on.
    ///
    /// # What is being decided
    ///
    /// `consenting()` is the one reader left that goes behind the machine's back, and the answer
    /// that shut the other three does not reach it. `may_answer` is not a number: it is an ARRAY OF
    /// OBJECTS, each `{asked, answer}`, which `OuterLoop::consents_in` walks with typed getters two
    /// levels deep. A host act's arguments arrive as `HashMap<String, Vec<String>>`.
    ///
    /// ⚠⚠ **A NO WOULD HAVE BEEN THE USEFUL ANSWER TOO**, and it is the answer this case was
    /// written expecting. It is not what was measured.
    ///
    /// # ⭐⭐⭐ MEASURED 2026-08-26: **YES, THREE WAYS OVER**
    ///
    /// * **the whole list crosses as JSON** — `[{"answer":"yes","asked":"…"},{…}]`, which a driver
    ///   can walk back. The road `service_needle`, `within` and `awaits`/`stills` took is open for
    ///   `may_answer` too.
    /// * **a repeated `<param>` name keeps every value, in document order** — so a fixed number of
    ///   clause slots could cross with no structure crossing at all. A fallback, measured.
    /// * **indexing selects, two levels deep** — `clauses[0]` is the FIRST element (0-based, though
    ///   the engine underneath is Lua) and `clauses[1].asked` arrives as its own bare string.
    ///
    /// ⚠ **WHAT THIS DOES NOT DECIDE**, stated rather than assumed: walking that JSON back means a
    /// `serde_json` parse in the driver, and this crate's manifest admits `serde_json` for
    /// *perception of an external program* rather than for its own types. Whether an SCXML engine's
    /// `<param>` counts as that is the CARRIER round's decision, on the record, not this probe's.
    ///
    /// ⚠ The value is PLANTED rather than authored in the document, as the exact
    /// `ScriptValue::Array(Object)` shape `consents_in` reads — so what is measured is the crossing
    /// and not how a Lua-backed `ecmascript` datamodel parses an array literal.
    #[test]
    fn a_clause_list_crossing_as_an_argument() {
        use std::collections::HashMap;
        use std::sync::Mutex;

        type Seen = Vec<(String, HashMap<String, Vec<String>>)>;

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));
        let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Vec::new()));
        engine.register_event_processor("x-sprag-host", {
            let seen = Arc::clone(&seen);
            move |request| {
                seen.lock()
                    .expect("the record")
                    .push((request.event_name.clone(), request.params.clone()));
                vec![sce_rust_runtime::host_processor::HostSendResponse {
                    event_name: request.event_name,
                    event_data: String::new(),
                }]
            }
        });
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let clause = |asked: &str, answer: &str| {
            let mut fields = HashMap::new();
            fields.insert("asked".to_owned(), ScriptValue::String(asked.to_owned()));
            fields.insert("answer".to_owned(), ScriptValue::String(answer.to_owned()));
            ScriptValue::Object(fields)
        };
        engine
            .policy()
            .script_engine
            .set_variable(
                &session,
                "clauses",
                ScriptValue::Array(vec![
                    clause("trust the files in this folder", "yes"),
                    clause("run without asking again", "no"),
                ]),
            )
            .expect("the datamodel takes the shape its own reader walks");

        // ⚠⚠⚠ THE FIXTURE'S OWN CONTROL, BEFORE A SINGLE SEND. If the plant did not round-trip as
        // an Array of Objects, every reading below would be about a variable holding something
        // else — and `consents_in` is the definition of the shape that has to cross.
        let planted = engine
            .policy()
            .script_engine
            .get_variable(&session, "clauses")
            .expect("the planted variable is readable");
        assert!(
            matches!(&planted, ScriptValue::Array(items)
                if items.len() == 2
                    && items.iter().all(|it| matches!(it, ScriptValue::Object(_)))),
            "⚠⚠⚠⚠⚠ THE PLANT DID NOT SURVIVE THE DATAMODEL, so this probe answers nothing about \
             `<param>`: {planted:?}",
        );

        engine.process_event(ProbeSendTypeEvent::ProbeEnter);
        engine.process_event(ProbeSendTypeEvent::ProbeTake);
        for _ in 0..8 {
            engine.tick();
        }
        engine.process_event(ProbeSendTypeEvent::ProbeList);
        for _ in 0..8 {
            engine.tick();
        }
        engine.process_event(ProbeSendTypeEvent::ProbeDeep);
        for _ in 0..8 {
            engine.tick();
        }

        let after: Seen = seen.lock().expect("the record").clone();
        let only = |named: &str| {
            let hits: Vec<&(String, HashMap<String, Vec<String>>)> =
                after.iter().filter(|(it, _)| it == named).collect();
            assert_eq!(
                hits.len(),
                1,
                "⚠⚠⚠ THE CONTROL: `{named}` must have reached this host exactly once, or nothing \
                 read off it is about a crossing. Saw {after:?}",
            );
            hits[0].1.clone()
        };

        // ── THE SEND ARRIVED AT ALL: a plain string in the same send ──
        let listing = only("carried.list");
        assert_eq!(
            listing.get("word").map(Vec::as_slice),
            Some(["plain".to_owned()].as_slice()),
            "⚠⚠⚠⚠⚠ THE CONTROL FAILED, SO THIS PROBE ANSWERS NOTHING. A `listed` that reads empty \
             below would then mean *this send did not arrive*, which is a fact about the fixture \
             and not about structures. Got {listing:?}",
        );

        // ── ⭐ THE ANSWER: THE LIST CROSSES, AND IT CROSSES WITH ITS CONTENT ──
        //
        // ⭐ MEASURED 2026-08-26: `listed` arrives as JSON —
        // `[{"answer":"yes","asked":"trust the files in this folder"},{…}]`. So the door
        // `consenting()` holds open CAN be shut by the same carrier the other three used, with a
        // parse on the driver side rather than a typed getter.
        //
        // ⚠⚠ ASSERTED AS STRUCTURE AND NOT AS BYTES. The measured spelling puts `answer` before
        // `asked`, which is the serialiser's ordering to choose and not a promise; a gate pinned to
        // the exact string would go red for a harmless change. What the road needs is that both
        // clauses ARRIVE and are TELLABLE APART, which is what this reads.
        let listed = listing
            .get("listed")
            .and_then(|values| values.first())
            .cloned()
            .unwrap_or_default();
        let held: serde_json::Value = serde_json::from_str(&listed).unwrap_or_else(|why| {
            panic!(
                "⚠⚠⚠⚠⚠ **A LIST OF OBJECTS DID NOT CROSS WITH ITS CONTENT, WHICH SHUTS THIS ROAD \
                 FOR `may_answer`.** A handle (`table: 0x…`), an empty string or anything else a \
                 driver cannot walk back means the last back door cannot be shut the way the other \
                 three were, and item 470 needs a different shape for it. Got {listed:?}: {why}",
            )
        });
        let asked: Vec<String> = held
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|it| it.get("asked").and_then(serde_json::Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            asked,
            vec![
                "trust the files in this folder".to_owned(),
                "run without asking again".to_owned(),
            ],
            "⚠⚠⚠⚠⚠ **BOTH CLAUSES MUST ARRIVE, IN THE CALLER'S OWN ORDER.** `Consents` is a list \
             of INDEPENDENT clauses and `covers` takes the first that resolves, so a crossing that \
             dropped one, merged them, or reordered them would change which dialog a run answers. \
             Got {held:?}",
        );

        // ── ⭐ AND THE FALLBACK IS REAL TOO: A REPEATED NAME KEEPS EVERY VALUE, IN ORDER ──
        assert_eq!(
            listing.get("twice").map(Vec::len),
            Some(2),
            "⚠⚠⚠⚠ A REPEATED `<param>` NAME MUST KEEP EVERY VALUE. `Params` is a vector per name \
             and says so in a comment; this is the measurement behind it, and it is the shape a \
             carrier would fall back to if the serialisation above were ever lost. Got {listing:?}",
        );
        assert_eq!(
            listing.get("twice").and_then(|values| values.first()),
            Some(&"plain".to_owned()),
            "⚠⚠⚠ AND IN THE ORDER THE DOCUMENT WROTE THEM, which is the whole of what a vector \
             adds over a set: clause ORDER decides which one `covers` takes. Got {listing:?}",
        );

        // ── ⭐ AND INDEXING SELECTS, TWO LEVELS DEEP ──
        //
        // ⭐ MEASURED 2026-08-26: `clauses[0]` is the FIRST element and `clauses[1]` the second, so
        // this pairing presents the array 0-based even though the engine underneath is Lua — and
        // `clauses[1].asked` arrives as its own bare string rather than as JSON. A document can
        // therefore carry named parts as well as the whole.
        let deep = only("carried.deep");
        let at = |name: &str| deep.get(name).and_then(|values| values.first()).cloned();
        assert_ne!(
            at("at_zero"),
            at("at_one"),
            "⚠⚠⚠⚠ INDEXING MUST SELECT RATHER THAN HAND BACK THE WHOLE. If both bases read the \
             same, a document could not name one clause — it could only ever send all of them, and \
             the fallback shape above is the only road left. Got {deep:?}",
        );
        assert_eq!(
            at("field"),
            Some("run without asking again".to_owned()),
            "⚠⚠⚠⚠⚠ A FIELD TWO LEVELS DOWN MUST ARRIVE AS ITS OWN VALUE, unquoted and unwrapped. \
             This is what says the crossing understands the structure rather than stringifying \
             whatever it is handed. Got {deep:?}",
        );
    }

    /// ⚠⚠⚠⚠⚠ **ONE HOST ACT PRODUCES TWO EVENTS, IN THE ORDER IT NAMED THEM** — consuming SCE
    /// `084dfdbf`, taken from `pinion@38c908b2` as the shared-instance rule requires.
    ///
    /// # ⚠⚠⚠ Why the ORDER is the whole claim, and two counters would have measured nothing
    ///
    /// A handler's reply was ONE event until this rev. A host that needed two had two ways out and
    /// the engine's own notes record rejecting both: re-enter the engine from inside the handler —
    /// impossible on this backend, because the registry hands out a `&mut` borrowed from the engine
    /// — or return one and deliver the rest on the host's next step, which puts a pending slot back
    /// in the host, the hidden state that moving an act into the document exists to remove.
    ///
    /// So what the list adds is the ORDER, and nothing else. A gate counting arrivals would read
    /// `1` and `1` whichever way round they came, and would stay green against an engine that
    /// delivered a reply backwards.
    ///
    /// # ⚠⚠⚠⚠ THE CONTROL IS THE SAME ACT WITH THE LIST REVERSED, and without it this proves
    /// nothing
    ///
    /// One run cannot tell *the engine preserved my order* from *the engine has an order of its
    /// own that happens to agree* — alphabetical, hash, queue insertion, anything. So the same
    /// document and the same handler are driven twice with the two events swapped, and the ranks
    /// must swap with them. **A green needs both halves**; either alone is a coincidence.
    ///
    /// # ⚠⚠ Why this matters here rather than upstream
    ///
    /// `ai_loop.scxml`'s `priming` leaves on `prompt.sent` — *the session has been told what it is
    /// here for* — and only then is the machine anywhere a turn result means anything. An act that
    /// reported the turn FIRST would leave the run sitting in `priming` for ever. Prompting a fresh
    /// session is therefore exactly two events with exactly one correct order, and until this rev
    /// the engine could not carry it.
    ///
    /// ⚠ What a green does NOT say: no shipped document names a two-event act yet. Item 470 stage
    /// 2 is where that gets built; this is the road being proven on the day it opened.
    #[test]
    fn a_host_act_answers_with_two_events_in_the_order_it_named_them() {
        /// One run of the probe document with a host handler that answers `first` then `second`,
        /// giving back what the DATAMODEL recorded — the engine's ordering, not a `Vec`'s.
        fn ranks(first: &'static str, second: &'static str) -> (i64, i64, i64, i64, i64) {
            let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
            let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));
            // ⚠ REGISTERED BEFORE `initialize`, for the reason its neighbour above states: the
            // send is in the `onentry` of the INITIAL state.
            engine.register_event_processor("x-sprag-host", move |_request| {
                vec![
                    sce_rust_runtime::host_processor::HostSendResponse {
                        event_name: first.to_owned(),
                        event_data: String::new(),
                    },
                    sce_rust_runtime::host_processor::HostSendResponse {
                        event_name: second.to_owned(),
                        event_data: String::new(),
                    },
                ]
            });
            engine.register_invoker("x-sprag-host", |_event| None);
            engine.initialize();
            // ⚠ `tick()` rather than `step()`, for the reason the gate above gives: a `<send>`
            // goes through the scheduler even with no delay.
            for _ in 0..8 {
                engine.tick();
            }
            let session = engine
                .policy()
                .session_id
                .clone()
                .expect("a script datamodel opens a script session");
            let count =
                |name: &str| match engine.policy().script_engine.get_variable(&session, name) {
                    Ok(ScriptValue::Int(held)) => held,
                    other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
                };
            (
                count("rank1"),
                count("rank2"),
                count("plain"),
                count("errors"),
                count("landed"),
            )
        }

        let (one_first, two_first, plain, errors, landed) = ranks("ordered.one", "ordered.two");

        // ── the controls: the document ran, and nothing but this handler spoke ──
        assert_eq!(
            plain, 1,
            "⚠⚠⚠ THE CONTROL: the untyped `<send>` in the same entry must still deliver, or the \
             ranks below are about a document that never ran",
        );
        assert_eq!(
            errors, 0,
            "⚠⚠ and a `<send>` whose handler answers a LIST must raise no `error.execution` — a \
             refusal here would say the engine took the widened reply as a malformed one",
        );
        assert_eq!(
            landed, 0,
            "⚠⚠⚠⚠ AND THE ENGINE MUST NOT INVENT A REPLY. This handler never names the request's \
             own `reached.host`, so a `landed` above zero would mean the events the document saw \
             were not the ones the handler listed — which would make every rank below unreadable",
        );

        // ── the claim: BOTH arrived, and in the order the handler listed them ──
        assert_eq!(
            (one_first, two_first),
            (1, 2),
            "⚠⚠⚠⚠⚠ A HOST ACT'S TWO EVENTS DID NOT REACH THE DOCUMENT IN THE ORDER IT NAMED \
             THEM. A rank of 0 is an event that never arrived at all — the reply was truncated to \
             one, which is the pre-`084dfdbf` behaviour — and (2, 1) is both arriving BACKWARDS. \
             `ai_loop.scxml`'s `priming` leaves on the FIRST of a pair; reversed, a run sits there \
             for ever",
        );

        // ── the control that makes the claim a claim: swap the list, the ranks must swap ──
        let (one_second, two_second, ..) = ranks("ordered.two", "ordered.one");
        assert_eq!(
            (one_second, two_second),
            (2, 1),
            "⛔⛔⛔⛔⛔ THE CONTROL FAILED — the arrival order does not follow the HANDLER's list. \
             With the same two events listed the other way round the document still saw \
             ({one_second}, {two_second}), so whatever ordered them belongs to the engine and not \
             to the host. The assertion above would then be green against an engine that ignores \
             the order entirely, which is the one thing a list adds over the single reply it \
             replaced",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AN `error.*` NOBODY ANSWERED IS A FACT THE HOST CAN NOW READ** — consuming SCE's
    /// `unhandled_error_events`, 2026-08-20.
    ///
    /// # ⚠⚠⚠⚠ Why this loop needed it, measured
    ///
    /// W3C SCXML 3.12.2: an error event goes on the internal queue and is IGNORED when nothing
    /// matches. **`ai_loop.scxml` and `debt_loop.scxml` carry ZERO `error.*` transitions between
    /// them**, so every failure their own `<assign>`s, guards and `<send>`s can raise has always
    /// been swallowed — and no reading a driver took could tell a run that worked from a run whose
    /// executable content failed on entry.
    ///
    /// Item 483 is what makes it sharp rather than theoretical: a `<send>` naming a type nobody
    /// serves raises exactly this error AND abandons the rest of its block. The day this loop names
    /// an act (item 470 stage 2, unblocked the same day), an unregistered handler would make the
    /// loop do NOTHING, quietly, and every gate in this crate would stay green.
    ///
    /// # ⚠⚠⚠ Two documents, one axis: whether the document ANSWERS
    ///
    /// `probe_unanswered.scxml` raises `error.execution` from an undeclared `<assign>` location and
    /// declares no error transition anywhere. `probe_send_type.scxml` raises the same error and
    /// ANSWERS it. So a reading of *how many went unanswered* is attributable to the document
    /// rather than to the engine — without the control, `1` here could be the engine counting every
    /// error it ever raised.
    #[test]
    fn an_error_the_document_never_answers_is_still_a_fact_the_host_can_read() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(crate::sm::probe_unanswered_sm::ProbeUnansweredPolicy::new(
            Arc::clone(&lua),
        ));
        engine.initialize();
        for _ in 0..8 {
            engine.tick();
        }

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<crate::sm::probe_unanswered_sm::ProbeUnansweredPolicy>,
                     name: &str| {
            match engine.policy().script_engine.get_variable(&session, name) {
                Ok(ScriptValue::Int(held)) => held,
                other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
            }
        };

        // ── THE CONTROL FIRST: the failure really happened, and it cost the block ──
        assert_eq!(
            count(&engine, "after"),
            0,
            "⚠⚠⚠⚠ THE UNSERVED-TYPE `<send>` MUST HAVE FAILED, and item 483's finding is what says \
             so from out here: an error in executable content abandons the REST of the block, so the \
             `<assign>` after it never ran. A `1` means nothing was raised at all, and then this \
             case is measuring nothing about errors. ⚠ This message named an UNDECLARED `<assign>` \
             until item 505's round: that was the probe's first draft, and the document's own \
             comment records why it was replaced — a location this engine has never declared is \
             simply created, so it raises nothing",
        );

        assert_eq!(
            engine.unhandled_error_events(),
            1,
            "⚠⚠⚠⚠⚠ THE SUBJECT: exactly one `error.*` was raised and nothing in the document \
             matched it, and the HOST can now say so. Zero is the silence this loop has always run \
             in — W3C 3.12.2 ignores the event, so before this reading existed a document whose \
             `onentry` failed was indistinguishable from one that worked",
        );
        assert!(
            engine.last_unhandled_error().is_some(),
            "⚠⚠⚠ and WHICH error it was, because the class is the whole diagnostic: \
             `error.execution` is the document's own content failing and `error.communication` is a \
             `<send>` that could not be delivered — one is a bug in the document and the other is a \
             host that did not answer",
        );

        // ── AND THE MACHINE IS STILL ALIVE, which is what makes the silence dangerous ──
        engine.process_event(crate::sm::probe_unanswered_sm::ProbeUnansweredEvent::Go);
        for _ in 0..8 {
            engine.tick();
        }
        assert_eq!(
            count(&engine, "moved"),
            1,
            "⚠⚠⚠ a run that had STOPPED would be visible in every reading a host takes; this one \
             went on assigning. That is why the count above is the only thing that could have \
             reported the failure",
        );

        // ── THE CONTROL DOCUMENT: the same error, ANSWERED, reads zero ──
        let mut answers = Engine::new(ProbeSendTypePolicy::new(lua));
        answers.initialize();
        for _ in 0..8 {
            answers.tick();
        }
        assert_eq!(
            answers.unhandled_error_events(),
            0,
            "⚠⚠⚠⚠⚠ `probe_send_type.scxml` raises `error.execution` (a `<send>` naming a type \
             nothing serves) and has a transition for it — so it must read ZERO here. A `1` would \
             mean this count is counting errors RAISED rather than errors UNANSWERED, and the \
             assertion above would be about the engine instead of about the document",
        );
    }

    /// ⚠⚠⚠ **CAN A PARENT FILL ITS CHILD'S DATAMODEL** — the question a composed design rests on,
    /// asked of a real engine rather than of the specification.
    ///
    /// # ⚠⚠⚠ What is being decided here, and why it is asked before anything is built
    ///
    /// The template is meant to serve OTHER repositories and to hold several loop KINDS — a debt
    /// loop, a feature loop — possibly running at once. Measured, what separates those kinds is
    /// entirely `<data>`: consents, standing instructions, prompt wording, the marker, where the
    /// work list lives. **The topology is identical.** So the shape that does not duplicate a
    /// 2,000-line machine per kind is: one topology as a CHILD, and one document per kind as a
    /// PARENT that owns the decisions and fills the child in.
    ///
    /// That shape is only available if `<param>` on an `<invoke>` actually reaches a `<data>` the
    /// child declared. **If it does not, copying is back on the table** — and this workspace has
    /// already recorded what two copies of one rule cost once they drift.
    ///
    /// ⚠⚠ **THE PROOF TRAVELS OUT AND BACK.** A child cannot report its own datamodel to anybody
    /// but its parent, so the child echoes what it was told through `<donedata>` and the parent
    /// compares. The child's default is a sentence nobody would send — so the failure where the
    /// param never crossed reads differently from the failure where the child never ran.
    #[test]
    fn a_parent_fills_its_childs_datamodel_and_the_child_can_prove_it() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(crate::sm::probe_parent_sm::ProbeParentPolicy::new(lua));
        engine.initialize();
        // ⚠ The invoke is DEFERRED (W3C SCXML 6.4), so the parent is stepped rather than read.
        for _ in 0..8 {
            engine.step();
        }

        assert_eq!(
            engine.get_current_state(),
            crate::sm::probe_parent_sm::ProbeParentState::Heard,
            "⚠ the control: without `done.invoke.probe` nothing below is about a child that ran",
        );

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let read = |name: &str| engine.policy().script_engine.get_variable(&session, name);

        let sent = read("told_it");
        let echoed = read("echoed");
        let (Ok(ScriptValue::String(sent)), Ok(ScriptValue::String(echoed))) = (&sent, &echoed)
        else {
            panic!(
                "both sides of this comparison must be strings the datamodel holds: {sent:?} / \
                 {echoed:?}"
            );
        };
        assert_eq!(
            echoed, sent,
            "⚠⚠⚠ THE PARENT'S `<param>` MUST REACH THE CHILD'S `<data>`. What came back is what the \
             child held when it finished; if it is the child's own placeholder, the param never \
             crossed and a parent has nothing it can say to a child — which makes *one topology, \
             one document per loop kind* unbuildable and puts COPYING the machine back on the \
             table. Compared against what was sent rather than against a literal, so rewording the \
             parent's value cannot make this pass by accident",
        );

        // ── AND THE SHAPE THE SPLIT ACTUALLY NEEDS: A LIST OF OBJECTS ──
        //
        // ⚠⚠⚠ A STRING CROSSING PROVES NOTHING ABOUT THIS. Every decision that would move from the
        // template to a per-kind parent — `may_answer`, `screen_rules`, `judged_rules` — is a list
        // of objects, and this workspace has already been bitten exactly here: `sce-rust-lua` used
        // to rewrite JSON payloads with `bytes[i] as char`, and valid JSON carrying an array or an
        // escape was DEMOTED TO A STRING with nothing raised anywhere. PR-87, fixed upstream. This
        // is the assertion that would notice it coming back.
        let Ok(ScriptValue::Array(back)) = read("echoed_rules") else {
            panic!(
                "⚠⚠⚠ THE LIST MUST COME BACK A LIST. Anything else — a string most of all — is the \
                 PR-87 demotion returning, and it fails SILENTLY: a parent would appear to hand its \
                 child a rule list and the child would hold one long string that claims no dialog. \
                 Got {:?}",
                read("echoed_rules"),
            );
        };
        assert_eq!(
            back.len(),
            2,
            "⚠⚠ TWO, not one: a list that lost an element and a list that arrived whole are the \
             same length at one, which is why the fixture sends two. Got {back:?}",
        );
        // ⚠ Compared against what the PARENT holds rather than against literals, so rewording the
        // fixture cannot make this pass by accident — the two sides must agree, whatever they say.
        let Ok(ScriptValue::Array(sent_rules)) = read("told_rules") else {
            panic!("the parent's own list must be readable for this to be a comparison");
        };
        for (at, (sent, got)) in sent_rules.iter().zip(back.iter()).enumerate() {
            let (ScriptValue::Object(sent), ScriptValue::Object(got)) = (sent, got) else {
                panic!(
                    "⚠⚠⚠ EACH ELEMENT MUST STILL BE AN OBJECT. An element flattened to a string \
                     still has a length, so the list check above cannot catch this: clause {at} is \
                     {got:?}",
                );
            };
            for field in ["when", "text"] {
                assert_eq!(
                    got.get(field),
                    sent.get(field),
                    "⚠⚠⚠ AND EVERY FIELD MUST SURVIVE. A parent that fills a child's rule list is \
                     the whole reason one topology can serve several loop kinds; a field lost in \
                     transit is a standing instruction that quietly claims nothing. Clause {at}, \
                     field {field:?}",
                );
            }
        }
    }

    /// ⚠⚠⚠⚠ **WHAT AN ENGINE SAYS WHEN ONE REGION IS DONE AND THE OTHER NEVER WILL BE** — the
    /// question the STANDING-ORDERS design rests on, and the one the first parallel probe did not
    /// ask because it had no finals at all.
    ///
    /// # ⚠⚠⚠ Why a whole probe for this
    ///
    /// The owner asked for a handle: a running loop that finishes the debt in front of it and then
    /// STOPS, instead of reflecting into the next one. Today the only thing anybody can say to a
    /// running loop is `cancel`, which throws away the turn in flight — measured this afternoon,
    /// when a pin bump cost exactly that.
    ///
    /// The long-term shape puts standing orders in **their own region**, orthogonal to the work,
    /// because an order is not a step of the work and repeating one transition on every state of a
    /// flat machine is *two copies of one rule* — the failure this workspace has already paid for.
    ///
    /// ⚠⚠ **BUT SCXML COMPLETES A `<parallel>` ONLY WHEN EVERY REGION IS FINAL, AND AN ORDERS
    /// REGION HAS NO ENDING.** So the driver's whole ending detection — `Driver` loops
    /// `while !engine.is_in_final_state()` — could sit there for ever on a run whose work was over.
    /// **That would not be a bug in the design, it would be the design being impossible**, and the
    /// answer decides between the orders region and the flat repetition.
    #[test]
    fn a_region_that_finishes_beside_one_that_never_does_is_still_readable() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();

        engine.raise(sce_rust_runtime::EventWithMetadata::new(
            ProbeParallelEvent::Finish,
        ));
        engine.step();

        let active = engine.get_active_states();
        assert!(
            active.contains(&ProbeParallelState::Done),
            "⚠⚠ the control: region A must actually have reached its own final, or nothing below is \
             about the arrangement this asks about. active = {active:?}",
        );
        assert!(
            active.contains(&ProbeParallelState::Watching),
            "⚠⚠⚠ AND THE ENDLESS REGION MUST STILL BE RUNNING. A region that was exited when its \
             sibling finished would take the standing orders with it exactly when a person is most \
             likely to be giving one. active = {active:?}",
        );

        // ⚠⚠⚠ THE ANSWER THE DESIGN TURNS ON, RECORDED WHICHEVER WAY IT FALLS. This asserts what the
        // ENGINE says rather than what a reader would prefer: a `<parallel>` is complete only when
        // every region is final, and one region here never can be. If this is `false`, the
        // orders-region design cannot use `is_in_final_state` as its ending signal and the driver
        // must read the WORK REGION's own final instead — which is a fact about how to build it,
        // not a reason not to.
        let whole = engine.is_in_final_state();
        assert!(
            !whole,
            "⚠⚠⚠ MEASURED: an engine that called this whole machine FINAL while a region is still \
             running would be completing a parallel on one region, which is the defect the gate \
             above exists for wearing different clothes. If this ever flips, the orders-region \
             design gets simpler and this gate is the place that says so",
        );
        // ⚠⚠⚠⚠ AND THE ANSWER THAT DECIDED THE DESIGN — WHICH IS NOT A VALUE, IT IS THAT THE VALUE
        // MOVES. `get_current_state()` was measured TWICE on this same document, and it answered
        // differently for two arrangements that a reader would call equivalent:
        //
        //   `<final>` as a sibling of the regions   -> `Both`, the parallel root
        //   `<final>` inside its own region         -> `Done`, the finished leaf
        //
        // ⚠ The first arrangement was also wrong for another reason — `get_parallel_regions`
        // counted the stray final as a THIRD region — so it is not that one answer is right. It is
        // that **a single-state reader has no stable meaning once a machine has regions**, because
        // it must flatten a set to one value and the flattening depends on shape.
        //
        // ⚠⚠⚠ SO THE FINDING IS NOT *"it answers Both"*, IT IS *"a driver may not switch on it"*.
        // `OuterLoop::pump` does exactly that today. Giving the loop regions therefore requires the
        // driver to read the ACTIVE SET — which the gate below proves it can — and this assertion
        // is deliberately the WEAK one, because pinning a value here would pin an arrangement
        // rather than a property.
        let flattened = engine.get_current_state();
        assert!(
            matches!(
                flattened,
                ProbeParallelState::Both | ProbeParallelState::Done
            ),
            "⚠⚠⚠ MEASURED: the single-state reader flattens a REGION SET to one value, and which \
             one depends on where the finals sit — `Both` with the final beside the regions, \
             `Done` with it inside one. Either way a driver switching on it is switching on an \
             arrangement rather than on a state, which is why `pump` must read the active set \
             before this machine ever gets regions. Got {flattened:?}",
        );
    }

    /// ⚠⚠⚠⚠ **CAN A DRIVER ASK A NAMED REGION WHAT STATE IT IS IN** — the remaining gate on the
    /// standing-orders design, and the one that decides whether it is buildable at all.
    ///
    /// # ⚠⚠⚠ What this is deciding
    ///
    /// The gate above measured that `get_current_state()` answers with the PARALLEL ROOT. So a
    /// driver that switches on it — `OuterLoop::pump` does — cannot be given a parallel top level
    /// without every arm of that match going dead in one silent edit.
    ///
    /// The way out, if there is one, is for the driver to stop asking *"what state am I in"* and
    /// start asking *"what state is the WORK region in"*. That question has to be answerable from
    /// what the generated policy publishes, or the design is finished here.
    ///
    /// ⚠⚠ **THIS IS A CAPABILITY QUESTION ABOUT SOMEBODY ELSE'S CODEGEN**, which is the whole
    /// reason it is a probe: `StatePolicy` declaring `get_parent` and `get_parallel_regions` is a
    /// fact about the TRAIT, and what a generated document puts behind them is a fact about the
    /// generator. This drives the real one.
    #[test]
    fn a_driver_can_ask_a_named_region_what_state_it_is_in() {
        use sce_rust_runtime::StatePolicy as _;

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();

        // ── THE ROOT MUST NAME ITS REGIONS ──
        //
        // ⚠ Asked of the POLICY rather than written down here: a driver that carried its own list
        // of region roots would hold a second copy of the document's topology, which is the failure
        // this workspace calls *two copies of one rule*.
        assert!(
            ProbeParallelPolicy::is_parallel_state(ProbeParallelState::Both),
            "⚠⚠ the control: the root must be known to BE parallel, or nothing below is about a \
             parallel machine",
        );
        let regions = ProbeParallelPolicy::get_parallel_regions(ProbeParallelState::Both);
        assert_eq!(
            regions.len(),
            2,
            "⚠⚠⚠ THE ROOT MUST PUBLISH ITS REGIONS. A driver that cannot enumerate them cannot ask \
             any of them anything, and standing orders in a region would be a design nothing can \
             read. Got {regions:?}",
        );

        /// What `region` is currently in — the question a driver would actually ask, answered from
        /// the active set by walking each member's parents until the region is found.
        ///
        /// ⚠ The ANCESTRY is what makes this work for a region that is itself compound, which the
        /// work region would be: a leaf several levels down still names its region through
        /// `get_parent`. A driver reading only the direct children would answer `None` the moment
        /// somebody nested a state, and nothing would say why.
        ///
        /// ⚠⚠ **THE DEEPEST ACTIVE DESCENDANT, NOT THE FIRST.** A compound region is ITSELF active
        /// while its child is, so a `find` answers with the region root and a driver switching on
        /// that would see one state for every state the region is ever in. Measured: the first
        /// draft did exactly that and reported `Ticking` for a region sitting in `Done`.
        fn state_of(
            engine: &Engine<ProbeParallelPolicy>,
            region: ProbeParallelState,
        ) -> Option<ProbeParallelState> {
            /// How far `state` sits below `region`, or `None` if it is not under it at all.
            fn depth_under(state: ProbeParallelState, region: ProbeParallelState) -> Option<usize> {
                let mut at = Some(state);
                let mut deep = 0;
                while let Some(here) = at {
                    if here == region {
                        return Some(deep);
                    }
                    deep += 1;
                    at = ProbeParallelPolicy::get_parent(here);
                }
                None
            }

            engine
                .get_active_states()
                .into_iter()
                .filter_map(|active| depth_under(active, region).map(|deep| (deep, active)))
                .max_by_key(|(deep, _)| *deep)
                .map(|(_, active)| active)
        }

        assert_eq!(
            state_of(&engine, ProbeParallelState::Ticking),
            // ⚠ The LEAF inside the region, not the region itself — which is exactly what a driver
            // needs to switch on, and exactly what `get_current_state` cannot be trusted to give.
            Some(ProbeParallelState::Counting),
            "⚠⚠⚠ A NAMED REGION MUST ANSWER WITH ITS OWN CURRENT STATE. This is the call that would \
             replace `get_current_state` in a driver whose machine has regions — if it cannot be \
             built out of what the policy publishes, standing orders cannot live in a region and \
             the alternative is repeating one transition on every state of a flat machine",
        );
        assert_eq!(
            state_of(&engine, ProbeParallelState::Watching),
            // ⚠ Its own LEAF, as region A answers with its own — the orders region became compound
            // when `In()` was measured, because an order that is a STATE needs somewhere to move to.
            Some(ProbeParallelState::Unalerted),
            "⚠⚠ and the SIBLING must answer independently, or the two regions are one answer wearing \
             two names and a standing order could not be read while work was going on",
        );

        // ── AND IT MUST KEEP ANSWERING AFTER ONE REGION HAS FINISHED ──
        //
        // ⚠⚠⚠ The arrangement the design lives in: work over, orders still standing. A driver that
        // could read the regions only while both ran would go blind at exactly the moment it has to
        // decide what a finished run means.
        engine.raise(sce_rust_runtime::EventWithMetadata::new(
            ProbeParallelEvent::Finish,
        ));
        engine.step();
        assert_eq!(
            state_of(&engine, ProbeParallelState::Ticking),
            Some(ProbeParallelState::Done),
            "⚠⚠⚠ the work region must report its FINAL by the same call — that is how a driver \
             learns a run ended when `is_in_final_state` cannot say so, which the gate above \
             measured it cannot",
        );
        assert_eq!(
            state_of(&engine, ProbeParallelState::Watching),
            Some(ProbeParallelState::Unalerted),
            "⚠⚠⚠ AND THE ORDERS REGION MUST STILL ANSWER — with its own LEAF, so a person can still \
             see whether the order was given. If a finished sibling silenced it, a person could not \
             stand a run down at the one moment the handle is for",
        );
    }

    /// ⚠⚠⚠⚠ **CAN ONE REGION BE GUARDED ON WHAT ANOTHER IS DOING** — `In()`, and it decides whether
    /// a standing order is a STATE or a boolean.
    ///
    /// # What rests on the answer
    ///
    /// The stand-down handle puts a person's order in its own region while work carries on, and the
    /// work region then has to ask, at its own decision point, *has that order been given*. W3C
    /// SCXML's predicate for exactly that is `In('<state>')`.
    ///
    /// ⚠⚠ **The alternative is a boolean, and it is worse for a stated reason.** An order the orders
    /// region sets as `<data>` is invisible in the run's walk: nothing records when it arrived or
    /// what the run was doing at the time. An order held as a STATE is in the journal by
    /// construction — which is this workspace's rule for machines everywhere else, argued at length
    /// by `context_review.scxml` for a sub-analysis, and an order is not less of a thing than that.
    ///
    /// ⚠⚠⚠ So this is asked BEFORE the loop is given regions, on the same terms as every probe here:
    /// the generator has `In` handling and the conformance suite has `In` tests, and **both are
    /// facts about somewhere else** until a document in this crate compiles and runs one. A NO is as
    /// useful as a YES — it is the difference between an order that appears in the walk and one that
    /// does not.
    ///
    /// # ⚠⚠ Why the guarded arrow is written ABOVE the plain one
    ///
    /// Document order decides which of two matching transitions wins. A guarded arrow placed second
    /// that never fired would be indistinguishable from a guard that evaluated false — so the
    /// fixture puts it first, and the plain arrow is what proves the event was delivered at all.
    #[test]
    fn a_transition_in_one_region_can_be_guarded_on_what_the_other_region_is_doing() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<ProbeParallelPolicy>, name: &str| match engine
            .policy()
            .script_engine
            .get_variable(&session, name)
        {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
        };
        let tick = |engine: &mut Engine<ProbeParallelPolicy>| {
            engine.raise(sce_rust_runtime::EventWithMetadata::new(
                ProbeParallelEvent::Tick,
            ));
            engine.step();
        };

        // ── BEFORE THE ORDER: the guard must be FALSE, or it says nothing ──
        tick(&mut engine);
        assert_eq!(
            count(&engine, "ticks"),
            1,
            "the control: the plain arrow fires, so the event really did reach region A",
        );
        assert_eq!(
            count(&engine, "noticed"),
            0,
            "⚠⚠⚠ A GUARD THAT IS TRUE BEFORE THE ORDER IS GIVEN IS NOT A GUARD. `In('alerted')` \
             must be false while the other region is resting — otherwise a run would stand itself \
             down before anybody asked it to, which is the failure that reads as a loop quietly \
             refusing to work",
        );

        // ── THE ORDER ARRIVES IN THE OTHER REGION ──
        engine.raise(sce_rust_runtime::EventWithMetadata::new(
            ProbeParallelEvent::Alert,
        ));
        engine.step();
        let active = engine.get_active_states();
        assert!(
            active.contains(&ProbeParallelState::Alerted),
            "the control: the order must have moved the orders region. active = {active:?}",
        );

        // ── AND NOW THE WORK REGION CAN SEE IT ──
        tick(&mut engine);
        assert_eq!(
            count(&engine, "noticed"),
            1,
            "⚠⚠⚠⚠ `In()` CANNOT READ ACROSS REGIONS HERE. The work region took its tick and did not \
             see an order standing beside it — so a standing order cannot be a STATE at this engine, \
             and the handle has to be built on a `<data>` boolean instead, losing the order from \
             every run's walk. That is a fact about how to build it, not a reason not to: record it \
             and take the flag",
        );
        assert_eq!(
            count(&engine, "ticks"),
            1,
            "⚠⚠ and the GUARDED arrow won, rather than both firing. Two matching transitions in one \
             region would mean the order changed what work does IN ADDITION to what it did before, \
             where the whole point is that it changes it INSTEAD",
        );
    }

    /// ⚠⚠⚠⚠ **CAN A CROSS-REGION GUARD BE NEGATED, AND CAN IT NAME A COMPOUND ANCESTOR** — the two
    /// questions register item 470's first stage rests on, asked before the loop's contract is
    /// edited.
    ///
    /// # What rests on the answer
    ///
    /// Item 470 moves one decision out of the driver and into the document: *a run a person is
    /// deliberately holding is not a run nobody came to*, which today lives in Rust as
    /// `if run.held() { … }` in `OuterLoop::attend`. In the document it is a guard on
    /// `awaiting_human`'s `unattended` edge, and the obvious spelling is `!In('held')`.
    ///
    /// ⚠⚠⚠ **`ai_loop.scxml` says `datamodel="ecmascript"` and a `LuaEngine` is what evaluates
    /// it.** Every cond that document carries today is a conjunction, a comparison or a bare read —
    /// `_event.data.done && In('standing_down')` is the closest thing to this one — and **not one
    /// of them is a negation**. So `!` is exactly the shape this module exists for: promised by the
    /// language named on the document, unmeasured on the thing that runs it. The guard would have
    /// been written, the suite would have passed on every path that does not hold a run, and a
    /// negation that silently evaluated false would read as *a held run ends as unattended* — the
    /// defect item 470 is removing, restored by the fix for it.
    ///
    /// # ⚠⚠ Why the ancestor question is asked in the same breath
    ///
    /// Because it is the FALLBACK, and a probe that answers only *no* leaves the next round where
    /// this one started. If `In()` matches a compound ancestor, the guard can be written
    /// positively — the orders that permit the edge go inside an umbrella state, the hold order
    /// sits outside it, and the guard names the umbrella.
    ///
    /// ⚠⚠ **Both answered yes, and item 470 took the NEGATION** — which is worth writing down
    /// because the umbrella looks like the tidier shape and is not, here. The set this guard
    /// EXCLUDES is one order and the set it permits is the one that keeps growing: `orders` holds
    /// `standing` and `standing_down` today and is documented as the place the next order goes.
    /// `!In('held')` names the exclusion and needs nothing of an order added beside it; the
    /// umbrella needs every future order to be placed inside it, and an order added outside it
    /// would silently stop `unattended` from ever firing. The tidier-looking spelling is the one
    /// that ages.
    ///
    /// ⚠ Both are counted at TWO moments, before the order and after it. A guard asserted only
    /// where it should be true cannot tell a working predicate from one that is always true, which
    /// is the vacuous control this workspace keeps paying for.
    #[test]
    fn a_cross_region_guard_can_be_negated_and_can_name_a_compound_ancestor() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<ProbeParallelPolicy>, name: &str| match engine
            .policy()
            .script_engine
            .get_variable(&session, name)
        {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
        };
        let ask = |engine: &mut Engine<ProbeParallelPolicy>, what: ProbeParallelEvent| {
            engine.raise(sce_rust_runtime::EventWithMetadata::new(what));
            engine.step();
        };

        // ── WHILE THE SIBLING RESTS: the negation must be TRUE and the ancestor must match ──
        ask(&mut engine, ProbeParallelEvent::AskQuiet);
        assert_eq!(
            count(&engine, "quiet"),
            1,
            "⚠⚠⚠⚠⚠ `!In()` DOES NOT EVALUATE AT THIS ENGINE. The sibling region is resting, so \
             `!In('alerted')` is true and the arrow had to fire. It did not — so register item \
             470's guard cannot be spelled as a negation, and `unattended` has to be guarded \
             positively on a compound ancestor instead (the assertion below says whether that is \
             available). Record it and take the other shape",
        );
        ask(&mut engine, ProbeParallelEvent::AskNested);
        assert_eq!(
            count(&engine, "nested"),
            1,
            "⚠⚠⚠⚠⚠ `In()` MATCHES ONLY LEAVES HERE. `watching` is the compound parent of the \
             region that is active, so a guard naming it had to fire. It did not — so an umbrella \
             state cannot carry item 470's guard, and every such guard has to enumerate the leaves \
             that permit the edge, which is a list that ages every time an order is added beside it",
        );

        // ── THE ORDER ARRIVES IN THE OTHER REGION ──
        engine.raise(sce_rust_runtime::EventWithMetadata::new(
            ProbeParallelEvent::Alert,
        ));
        engine.step();
        let active = engine.get_active_states();
        assert!(
            active.contains(&ProbeParallelState::Alerted),
            "the control: the order must have moved the orders region. active = {active:?}",
        );

        // ── AND NOW THE NEGATION MUST GO FALSE, WHICH IS THE HALF THAT MAKES IT A GUARD ──
        ask(&mut engine, ProbeParallelEvent::AskQuiet);
        assert_eq!(
            count(&engine, "quiet"),
            1,
            "⚠⚠⚠⚠⚠ `!In('alerted')` IS STILL TRUE WITH THE ORDER STANDING. A predicate that never \
             goes false is not a guard — it is a transition with extra words, and item 470's edge \
             would end a held run exactly as it does today while looking like it could not",
        );
        assert_eq!(
            count(&engine, "nested"),
            1,
            "the control: nothing else moved this while the negation was being read",
        );

        // ⚠ And the ancestor is STILL matched, now that the region rests on its other child —
        // which is what makes it an umbrella rather than an alias for one leaf.
        ask(&mut engine, ProbeParallelEvent::AskNested);
        assert_eq!(
            count(&engine, "nested"),
            2,
            "⚠⚠⚠⚠ `In('watching')` STOPPED MATCHING WHEN THE REGION CHANGED CHILD, so it was never \
             an ancestor match at all — it agreed with the initial leaf and nothing more. A guard \
             built on it would silently narrow to one order the moment a second arrived",
        );
    }

    /// ⚠⚠⚠⚠ **THE DRIVER'S OWN READER, DRIVEN AGAINST A MACHINE THAT HAS REGIONS** — the gate that
    /// holds `OuterLoop::state`'s change, because the loop's own document cannot exercise it yet.
    ///
    /// # ⚠⚠⚠ Why this gate exists at all
    ///
    /// `OuterLoop::state` stopped asking `get_current_state` and started taking the deepest member
    /// of the active set. Against `ai_loop.scxml` — which is flat — **both answers are identical**,
    /// so the whole plugin suite stayed green and NOTHING held the change. A change no gate can
    /// break is a change nobody is holding, and the next person to "simplify" it back would find
    /// every test agreeing with them.
    ///
    /// ⚠⚠ The hazard only appears on a machine with regions, and the only one this crate has is
    /// this probe. So the READER is re-implemented here over the probe's policy and asserted to
    /// disagree with the flattening call — which is the entire point of having made the change.
    #[test]
    fn the_deepest_active_state_is_not_what_the_flattening_call_answers() {
        use sce_rust_runtime::StatePolicy as _;

        /// `OuterLoop::state`'s rule, over this probe's machine: the deepest active member.
        fn deepest(engine: &Engine<ProbeParallelPolicy>) -> Option<ProbeParallelState> {
            engine
                .get_active_states()
                .into_iter()
                .map(|active| {
                    let mut depth = 0_usize;
                    let mut at = ProbeParallelPolicy::get_parent(active);
                    while let Some(parent) = at {
                        depth += 1;
                        at = ProbeParallelPolicy::get_parent(parent);
                    }
                    (depth, active)
                })
                .max_by_key(|(depth, _)| *depth)
                .map(|(_, active)| active)
        }

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();

        // ⚠ At rest the two agree on nothing useful: the flattening call names the ROOT, and the
        // deepest member names a leaf a driver could switch on. That difference IS the change.
        assert_eq!(
            engine.get_current_state(),
            ProbeParallelState::Both,
            "⚠ the control: the flattening call names the parallel root while both regions run, \
             which is what makes it useless to a driver",
        );
        assert_ne!(
            deepest(&engine),
            Some(engine.get_current_state()),
            "⚠⚠⚠ THE TWO READERS MUST DISAGREE HERE, or this gate is holding nothing and \
             `OuterLoop::state`'s change is unmeasured. The flattening call answers the root; the \
             deepest member answers a state a driver can act on",
        );
        assert!(
            deepest(&engine).is_some_and(|state| matches!(
                state,
                ProbeParallelState::Counting | ProbeParallelState::Unalerted
            )),
            "⚠⚠ and it must be a LEAF of one of the regions rather than a region root — a compound \
             state is active while its child is, so an answer that stopped at the region would stop \
             distinguishing the states inside it. ⚠ BOTH regions are compound now (the orders region \
             gained the resting/ordered pair that `In()` reads), so either leaf is a legitimate \
             answer here — which is exactly the ambiguity `OuterLoop::state` must stop relying on \
             depth to resolve. Got {:?}",
            deepest(&engine),
        );
    }

    /// Read one of the probe's counters out of the live datamodel.
    fn counter(engine: &Engine<ProbeParallelPolicy>, name: &str) -> i64 {
        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        match engine.policy().script_engine.get_variable(&session, name) {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("the probe's `{name}` must be a number the datamodel holds: {other:?}"),
        }
    }

    /// ⚠⚠⚠ **`<parallel>` RUNS IN THIS CRATE, AND A SELF-TRANSITION DOES NOT SWALLOW ITS SIBLING**
    /// — the four questions `probe_parallel.scxml` asks, answered by driving a real engine.
    ///
    /// # ⚠⚠⚠ Why the fourth question is the one worth the probe
    ///
    /// Questions 1-3 (does it compile, do both regions enter, does an event reach both) are the
    /// ones a person expects to ask, and an engine that failed any of them would fail loudly the
    /// first time anybody tried. The fourth is different: SCE's own suite records a parallel defect
    /// that SHIPPED (`1419a050ed`) — a self-transition whose exit set swallowed the parallel root —
    /// and records that **every W3C fixture missed it because they are all one region deep.**
    ///
    /// A supervisor running N loops concurrently takes exactly that arrow constantly: `ai_loop`'s
    /// `working` state self-transitions on every look that found nothing. So the arrow this gate
    /// fires is not a synthetic edge case, it is the commonest thing the design would do.
    ///
    /// ⚠⚠ **THE TWO REGIONS ARE DELIBERATELY UNLIKE EACH OTHER**, and that asymmetry is the
    /// measurement rather than a style choice: `ticking` takes the self-transition, `watching` only
    /// counts. Two regions doing the same thing could not tell *"both regions got the event"* from
    /// *"one region got it twice"*, and the defect under test presents as exactly that confusion.
    #[test]
    fn a_self_transition_in_one_region_leaves_its_sibling_running() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();

        // ── QUESTION 2: DID BOTH REGIONS ENTER ──
        //
        // ⚠ Asked of the ACTIVE SET rather than of `get_current_state`, which answers with one
        // state and cannot express two. An engine that entered only the first region would answer
        // the same single state either way, which is the whole reason this is not a state compare.
        let entered = engine.get_active_states();
        for region in [ProbeParallelState::Ticking, ProbeParallelState::Watching] {
            assert!(
                entered.contains(&region),
                "⚠⚠⚠ BOTH REGIONS MUST BE ACTIVE AT ONCE, or `<parallel>` is a compound state \
                 wearing a different tag and every design that runs two loops together is running \
                 one. Missing {region:?}; active = {entered:?}",
            );
        }

        // ── QUESTIONS 3 AND 4: ONE EVENT, BOTH REGIONS, REPEATEDLY ──
        //
        // ⚠ FIVE, not one. A root that is swallowed is swallowed by the transition's EXIT SET, so
        // the first `tick` can look perfectly correct and the second finds the sibling already
        // gone. One fire would be a gate that passes on the defect it exists to catch.
        const FIRES: i64 = 5;
        for _ in 0..FIRES {
            engine.raise(sce_rust_runtime::EventWithMetadata::new(
                ProbeParallelEvent::Tick,
            ));
            engine.step();
        }

        assert_eq!(
            counter(&engine, "ticks"),
            FIRES,
            "⚠⚠ the self-transitioning region must take its own arrow every time — if this is \
             short, the failure is that region's own and questions 3 and 4 cannot be read from it",
        );
        assert_eq!(
            counter(&engine, "seen"),
            FIRES,
            "⚠⚠⚠ AND THE SIBLING MUST HAVE SEEN EVERY ONE OF THEM. This is the defect SCE's suite \
             records shipping and every W3C fixture missing: a self-transition whose exit set \
             swallows the parallel root leaves this counter stuck at the fire that killed it, \
             while `ticks` marches on. A supervisor built on `<parallel>` would lose a loop per \
             look",
        );

        // ── AND THE SIBLING IS STILL THERE TO BE ASKED ──
        //
        // ⚠ The counter alone cannot say this: a region exited after its last increment leaves the
        // number correct and the machine wrong. The active set is what tells them apart.
        let still = engine.get_active_states();
        assert!(
            still.contains(&ProbeParallelState::Watching),
            "⚠⚠⚠ the sibling must still be ACTIVE, not merely have counted correctly on its way \
             out — a run that lost a region after its last useful step is a run that will not \
             answer the next one. active = {still:?}",
        );
    }

    /// ⚠⚠⚠⚠ **CAN A DOCUMENT SPELL *"NO BOUND"* WITH ONE `<data>`** — asked before the debt loop's
    /// turn budget is made declinable, because the wrong answer is a loop that exhausts on its
    /// first judged turn.
    ///
    /// # ⚠⚠⚠ What is being decided
    ///
    /// The owner has asked for a debt loop that reflects every five turns and **never ends on
    /// turns**. `judging` guards on `turns >= max_turns`, so the absence of a bound needs a
    /// spelling. Two are available and only one of them is measured here, because the other is
    /// known to work and known to cost:
    ///
    /// * a number and a BOOLEAN beside it, read together — one decision written in two places,
    ///   which is the shape this workspace has twice recorded the price of;
    /// * ⭐ ONE `<data>`, declared and empty, with the guard short circuiting on it.
    ///
    /// The second is better if the generator allows it, and whether it does is a fact about THIS
    /// crate at the pinned rev rather than about ECMAScript.
    ///
    /// # ⚠⚠⚠⚠ Why three arms, and why none is decoration
    ///
    /// A `cond` is PARSED as ECMAScript and EVALUATED as Lua, and one the generator cannot parse
    /// becomes an `error(...)` call that evaluates FALSE — it compiles, it runs, the edge is never
    /// taken and nothing is reported. That trap already cost this repository two wrong conclusions.
    /// So *"the guard did not fire"* is the same observation for **the short circuit worked** and
    /// for **the guard was never a guard**, and a single-arm probe would report the second as the
    /// first.
    ///
    /// Arm 2 fires the SAME guard shape over an id holding a number: it proves the shape parses.
    /// Arm 3 names ONLY the empty id and expects it falsy: it proves arm 1's silence is about the
    /// VALUE, not about a guard mentioning that id failing to parse. ⚠ Arm 2 alone would not do —
    /// it names a different id, so a parse failure peculiar to arm 1's text would still read green.
    #[test]
    fn an_id_a_document_declares_and_leaves_empty_is_falsy_and_safe_to_guard_on() {
        use crate::sm::probe_absent_sm::{ProbeAbsentEvent, ProbeAbsentPolicy};

        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeAbsentPolicy::new(lua));
        engine.initialize();

        let session = engine
            .policy()
            .session_id
            .clone()
            .expect("a script datamodel opens a script session");
        let count = |engine: &Engine<ProbeAbsentPolicy>, name: &str| match engine
            .policy()
            .script_engine
            .get_variable(&session, name)
        {
            Ok(ScriptValue::Int(held)) => held,
            other => panic!("`{name}` must be a number the datamodel holds: {other:?}"),
        };
        let send = |engine: &mut Engine<ProbeAbsentPolicy>, event: ProbeAbsentEvent| {
            engine.raise(sce_rust_runtime::EventWithMetadata::new(event));
            engine.step();
        };

        // Three turns, so the control's bound of two is passed rather than merely reached.
        for _ in 0..3 {
            send(&mut engine, ProbeAbsentEvent::Tick);
        }
        assert_eq!(count(&engine, "turns"), 3, "the counter must have moved");

        // ⚠⚠⚠⚠ AND THE READING THAT DECIDES WHERE THIS SPELLING MAY BE USED — measured, and it
        // refutes the design it was asked for.
        //
        // An empty `<data>` is a fine spelling for *no bound* in a document whose declarations are
        // FIXED and checkable as text: the template's own purity gate already reads `<data id="…"`
        // out of the file. It is NOT a fine spelling for a DECISION one document carries to another,
        // and this is why.
        let declared_empty = engine
            .policy()
            .script_engine
            .get_variable(&session, "absent");
        let never_declared = engine
            .policy()
            .script_engine
            .get_variable(&session, "no_such_id_is_declared_anywhere_in_this_document");
        assert_eq!(
            format!("{declared_empty:?}"),
            format!("{never_declared:?}"),
            "the finding below rests on these two reading the SAME; if they have diverged at a new \
             SCE rev, the constraint this records has been lifted and the kind document may spell \
             its decline by declaring an empty id after all",
        );
        assert_eq!(
            format!("{declared_empty:?}"),
            "Ok(Null)",
            "⚠⚠⚠⚠ DECLARED-AND-EMPTY AND NEVER-DECLARED ARE ONE OBSERVATION, and that forbids a \
             design. A loop KIND that spelled *no turn bound* by declaring the id and leaving it \
             empty could not be told apart from a kind that FORGOT the key — so forgetting would \
             grant an unbounded run. ⚠⚠⚠ It kills the obvious alternative too: a boolean beside \
             the number is no better, because an absent boolean and a `false` one are both falsy. \
             **Only a value that is neither a number nor nil can carry this decision between \
             documents**, which is why the kind spells its decline as a WORD",
        );

        send(&mut engine, ProbeAbsentEvent::AskPresent);
        assert_eq!(
            count(&engine, "present_fired"),
            1,
            "⚠⚠⚠⚠ THE CONTROL FOR THE WHOLE PROBE. `present && turns >= present` over an id \
             holding 2, with turns at 3, MUST fire. If it does not, this guard shape does not \
             parse at the pinned rev and every other assertion here is measuring a syntax error \
             rather than a datamodel",
        );

        send(&mut engine, ProbeAbsentEvent::AskUnset);
        assert_eq!(
            count(&engine, "unset_seen"),
            1,
            "⚠⚠⚠⚠ THE SECOND CONTROL, and the one that makes the answer specific. `!absent` must \
             be TRUE, which says two things at once: the empty id is falsy, AND a `cond` naming it \
             parses. Without this, arm 1 staying at zero would be the same observation for *the \
             short circuit worked* and *a guard mentioning `absent` is an error() that returns \
             false*",
        );

        send(&mut engine, ProbeAbsentEvent::AskAbsent);
        assert_eq!(
            count(&engine, "absent_asked"),
            1,
            "⚠⚠⚠⚠ THE DELIVERY PROOF, and it comes FIRST because the assertion after it is a ZERO. \
             `absent_fired == 0` is equally what this reads when the event never arrived, when the \
             transition was never generated, or when the name is misspelled in the document — a \
             guarded arrow that declined and an arrow nobody rang are the same observation. The \
             plain arrow behind the guarded one is what tells them apart",
        );
        assert_eq!(
            count(&engine, "absent_fired"),
            0,
            "⚠⚠⚠⚠ THE ANSWER. `absent && turns >= absent` must NOT fire, at any number of turns. \
             This is what lets `max_turns` mean *no bound* by being declared and left empty, \
             instead of needing a boolean beside it. If this fires, the one-`<data>` spelling is \
             REFUSED and the debt loop's *never ends on turns* has to be written as a pair",
        );
    }

    // ══════════════════════════════════════════════════════════════════════════════════════════
    // REGISTER ITEM 549 — CAN A MACHINE BE PUT WHERE IT SAYS IT IS?
    //
    // The two gates below are that item's VERDICT, and they exist because the answer had been
    // carried three times as a source reading (549's registration, the 550 bump, SCE-PR90's
    // measurement section). A reading is true of the file somebody opened and nothing re-opens it
    // at the next pin; these run against the engine the tree is pinned to, so the sweep is what
    // notices when the answer changes.
    // ══════════════════════════════════════════════════════════════════════════════════════════

    /// What an item-549 probe found about ONE method name on the pinned `Engine`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Door {
        /// No inherent method of that name exists, so the call reached the blanket fallback in
        /// `NoConfigurationDoor` instead.
        Absent,
        /// An INHERENT method of that name exists on `Engine` and won method resolution.
        Present,
    }

    /// What every fallback in `NoConfigurationDoor` returns. Nothing else produces one, which is
    /// the whole instrument: seeing this type back means the call found no inherent method.
    struct NoSuchMethod;

    /// Maps whatever a probe call actually returned onto a [`Door`].
    trait DoorOf {
        fn door(self) -> Door;
    }

    impl DoorOf for NoSuchMethod {
        fn door(self) -> Door {
            Door::Absent
        }
    }

    /// `Engine::has_ready_events` returns this, and it is the positive control's whole point: a
    /// `bool` coming back means an INHERENT method answered.
    impl DoorOf for bool {
        fn door(self) -> Door {
            Door::Present
        }
    }

    /// Every method here is reachable ONLY while `Engine` has no inherent method of that name —
    /// inherent methods win method resolution over trait ones, so the day SCE grows a door, the
    /// call site stops reaching this trait and the gate below turns red.
    ///
    /// The argument is generic so a door taking the saved configuration resolves rather than
    /// failing to type-check on its parameter.
    trait NoConfigurationDoor {
        fn initialize_at<A>(&mut self, _at: A) -> NoSuchMethod {
            NoSuchMethod
        }
        /// ⚠⚠⚠⚠⚠ **DEAD SINCE SCE `f3765c95c9`, AND ITS DEADNESS IS NOW THE RATCHET POINTING THE
        /// OTHER WAY — which is why it is kept rather than deleted.**
        ///
        /// This is the control above's pattern, earned rather than copied: the body is unreachable
        /// precisely because `Engine::enter_at` exists and wins resolution. Deleting it would throw
        /// away the only cheap thing that notices the door being TAKEN AWAY again — a pin that
        /// regressed would make this fallback reachable, the expectation unfulfilled, and the build
        /// would say so under `-D warnings`. Item 549 was open for a day because nothing watched
        /// the absence; nothing should now watch the presence any less.
        ///
        /// ⚠ It is deliberately NOT in the probe loop below any more. The real method takes two
        /// arguments and returns a `Result`, so a call there is a compile error rather than a
        /// verdict — see the note at that loop.
        #[expect(
            dead_code,
            reason = "unreachable while `Engine::enter_at` exists, which is item 549 delivered; \
                      the day it becomes reachable the door was withdrawn"
        )]
        fn enter_at<A>(&mut self, _at: A) -> NoSuchMethod {
            NoSuchMethod
        }
        fn enter_configuration<A>(&mut self, _at: A) -> NoSuchMethod {
            NoSuchMethod
        }
        fn restore_configuration<A>(&mut self, _at: A) -> NoSuchMethod {
            NoSuchMethod
        }
        fn set_active_states<A>(&mut self, _at: A) -> NoSuchMethod {
            NoSuchMethod
        }
        fn resume_at<A>(&mut self, _at: A) -> NoSuchMethod {
            NoSuchMethod
        }
        /// THE POSITIVE CONTROL, and it names a method SCE really has. If this ever answers
        /// `Absent`, the instrument is broken and every `Absent` beside it means nothing.
        ///
        /// ⚠⚠⚠⚠ **AND ITS DEADNESS IS A SECOND INSTRUMENT, WHICH IS WHY THIS IS `expect` AND NOT
        /// `allow`.** This body is unreachable precisely because `Engine::has_ready_events` wins
        /// resolution over it — so `dead_code` firing here is the control's claim restated by the
        /// compiler. The day it stops firing, the fallback has become reachable, the lint
        /// expectation goes unfulfilled, and the build says so under `-D warnings` before any
        /// assertion in the test below gets a chance to read a meaningless `Absent`.
        #[expect(
            dead_code,
            reason = "unreachable while the inherent method it is named after exists, which is \
                      exactly what the control asserts"
        )]
        fn has_ready_events(&self) -> NoSuchMethod {
            NoSuchMethod
        }
    }

    impl<T: ?Sized> NoConfigurationDoor for T {}

    /// ⚠⚠⚠⚠⚠ **A MACHINE CAN SAY WHERE IT IS AND CANNOT BE PUT THERE — item 549's verdict, asked
    /// of the TYPE.**
    ///
    /// # What a red here means
    ///
    /// A `Present` verdict is GOOD NEWS: SCE grew the door SCE-PR90 asked for. The response is to
    /// re-open item 549, re-read the new method's contract (does it run `<onentry>`? the sibling
    /// gate below is what answers that half), build 544 stage 3b on it, and delete the arm that
    /// went red. This gate is written to be deleted.
    ///
    /// # ⚠⚠⚠⚠ Why the instrument is method resolution rather than a grep
    ///
    /// A grep reads a checkout somebody happened to have. This reads the crate the tree is
    /// COMPILED against: the fallback trait below is reachable only while no inherent method of
    /// that name exists, because inherent methods win resolution. So the verdict is produced by
    /// the same rustc invocation that builds the product, at whatever rev `Cargo.toml` pins.
    ///
    /// # ⚠⚠⚠ THE TWO LIMITS, STATED RATHER THAN HIDDEN
    ///
    /// 1. **The names are a list with no glob.** A door named something none of the arms below
    ///    guesses is missed by this gate — registered as item 583 rather than left as a residue.
    ///    The names are SCE-PR90's own two (`initialize_at`, `enter_at`) plus four spellings an
    ///    upstream author might reach for.
    /// 2. **A door of a DIFFERENT SHAPE stops this file compiling instead of failing.** An
    ///    inherent `initialize_at` taking two arguments, or returning something with no `DoorOf`
    ///    impl, is a compile error at the call site rather than a `Present`. That is still the
    ///    alarm — this module's own header records a case where a compiler's refusal was the
    ///    strongest evidence available — and a reader who lands on an `E0061` or a trait-bound
    ///    error inside this test is being told exactly what this gate exists to tell them.
    #[test]
    fn sce_publishes_no_door_to_enter_a_machine_at_a_configuration() {
        let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
        let mut engine = Engine::new(ProbeParallelPolicy::new(lua));
        engine.initialize();
        // ⚠ Driven off the initial configuration first, so what each probe is handed is a REAL
        // configuration of more than one state — the thing a resumed run would have to hand back,
        // rather than a value that happens to type-check.
        engine.raise(sce_rust_runtime::EventWithMetadata::new(
            ProbeParallelEvent::Alert,
        ));
        engine.step();

        let saved = engine.get_active_states();
        assert!(
            saved.len() > 1 && saved.contains(&ProbeParallelState::Alerted),
            "⚠⚠ THE SUBJECT MUST BE A CONFIGURATION, not one state: item 549 is about handing a \
             whole active set back, and a probe handed a single-state chain would be asking an \
             easier question than the one 543 needs answered. saved = {saved:?}",
        );

        // ── THE INSTRUMENT'S OWN CONTROL, AND IT COMES FIRST ──
        // A name the pinned engine really has must read `Present` through the SAME resolution the
        // arms below use. Without it, six `Absent`s are equally what this test reads when the
        // blanket impl shadows everything, when the receiver is wrong, or when `Engine` grew a
        // `Deref` that moved resolution somewhere else.
        assert_eq!(
            engine.has_ready_events().door(),
            Door::Present,
            "⚠⚠⚠⚠⚠ THE INSTRUMENT IS BROKEN, not the finding. `Engine::has_ready_events` exists \
             at every rev this tree has pinned, so a fallback answering here means inherent \
             methods are no longer winning resolution for this receiver — and every `Absent` \
             below is then measuring the probe rather than the engine",
        );

        // ⚠⚠⚠⚠⚠ `enter_at` IS NO LONGER PROBED HERE, AND THAT IS THIS GATE HAVING WORKED.
        // It was in this list until SCE `f3765c95c9` (*"Enter a saved configuration without
        // re-running onentry"*) — item 549's ask, delivered. The arm did not fail its assertion:
        // it stopped COMPILING, because the real method takes two arguments and returns a
        // `Result`, neither of which the fallback's shape admits. ⚠ This doc's own limit 2 called
        // that outcome in advance and called it correct, so the `E0061` a reader lands on is the
        // alarm working rather than a surprise — the prediction is the author's, not a finding of
        // the round that collected it.
        // What `enter_at` DOES is now held by
        // `a_saved_configuration_is_entered_without_re_running_onentry`, which drives it against
        // the same harness the sibling gate below uses.
        for (name, verdict) in [
            ("initialize_at", engine.initialize_at(saved.clone()).door()),
            (
                "enter_configuration",
                engine.enter_configuration(saved.clone()).door(),
            ),
            (
                "restore_configuration",
                engine.restore_configuration(saved.clone()).door(),
            ),
            (
                "set_active_states",
                engine.set_active_states(saved.clone()).door(),
            ),
            ("resume_at", engine.resume_at(saved.clone()).door()),
        ] {
            assert_eq!(
                verdict,
                Door::Absent,
                "⚠⚠⚠⚠⚠ `Engine::{name}` EXISTS AT THE PINNED REV — item 549 may be unblocked, \
                 which is what this gate was built to catch. Read the new method's contract \
                 first: a door that re-runs `<onentry>` is a replay and 543 still cannot be \
                 built on it (the sibling gate here is what measures that half). Then re-open \
                 549, build 544 stage 3b, and delete this arm",
            );
        }
    }

    /// **THE ENGINE REVISION THE DOOR PROBE'S NAME LIST WAS LAST READ AGAINST.**
    ///
    /// ⚠⚠⚠ Move this ONLY after re-reading
    /// [`sce_publishes_no_door_to_enter_a_machine_at_a_configuration`]'s six spellings against the
    /// new revision — that is the whole transaction the gate below exists to force.
    ///
    /// # ⚠⚠ WHAT WAS READ AT `035c2dae`, so the next reader can tell a reading from a bump
    ///
    /// The whole `impl<P: StatePolicy> Engine<P>` block was enumerated rather than the six names
    /// searched for, because the gate's own limit 1 is that a SEVENTH spelling is invisible to a
    /// name list. `enter_at` is there and is item 549 delivered (already consumed by
    /// `a_saved_configuration_is_entered_without_re_running_onentry`); **nothing else in the block
    /// takes a configuration.** This rev's additions are all one subject and it is not a door:
    /// `register_invoker`, `has_invoker`, `perform_host_invoke`, `cancel_host_invoke`,
    /// `perform_host_send`, `has_event_processor`, `schedule_host_send` — a host serving `<invoke>`
    /// and `<send>`.
    ///
    /// ⚠ And the second half the gate's message asks for: **no trait is implemented for `Engine`
    /// at all** at this rev, so nothing shadows the method resolution the probe's verdict is made
    /// of.
    ///
    /// ⚠⚠⚠ **THE DOOR THIS REV DID ADD IS ON THE POLICY, NOT THE ENGINE** —
    /// `StatePolicy::get_state_from_name` (SCE `ef6c6d4c`), which is sprag's own SCE-PR92 delivered
    /// and is what makes `enter_at`'s argument buildable from a recorded name. The reading above
    /// stands unchanged BECAUSE of where it landed: the probe asks about `Engine`, and this is the
    /// second time a door has arrived on the policy instead — the blind spot the `set_active_states`
    /// note below already named. **That is the finding to carry forward, not a name to add here.**
    const DOORS_READ_AT_SCE_REV: &str = "035c2dae1e37c00540b31fe00653ca5423c96339";

    /// ⛔⛔⛔⛔ **THE DOOR PROBE'S NAME LIST IS RE-READ WHENEVER THE ENGINE PIN MOVES** — register
    /// item 583, and the mechanism its `Done when` asked for.
    ///
    /// # ⚠⚠⚠⚠⚠ What a list of names cannot do, and what this does instead
    ///
    /// The probe above asks about SIX spellings. Upstream picking a seventh leaves it green while
    /// the door exists, and item 549 then reads *still blocked* for as long as nobody looks. A glob
    /// would fix that and Rust has no stable way to ask *does this type have a method of this
    /// shape* without naming it — so the list cannot become one, and the repair has to be a
    /// mechanism that makes somebody LOOK on the one day the answer can change: a pin bump.
    ///
    /// ⚠⚠⚠⚠ **IT READS BOTH ARTEFACTS, WHICH IS WHY IT CANNOT ROT** (register item 470's rule). The
    /// audited revision is written here by hand; the revision actually built is read out of the
    /// workspace manifest at COMPILE time. A ratchet that held only its own copy would agree with
    /// itself forever.
    ///
    /// # What the last reading found, at `87f4b1d8` on 2026-08-23
    ///
    /// * `enter_at` EXISTS (item 549, delivered and consumed — the probe's own arm says so).
    /// * The other four spellings do not exist on `Engine`.
    /// * ⚠⚠ `set_active_states` exists — on `StatePolicy`, NOT on `Engine`. So the probe is right
    ///   not to see it as a door on the engine, and wrong to imply nothing wears that name: a door
    ///   arriving on the POLICY is a shape the probe does not look at at all.
    /// * ⚠⚠ **NO EXTENSION TRAIT IS IMPLEMENTED FOR `Engine`** at this revision — item 583 recorded
    ///   that blind spot as UNMEASURED and this is the measurement. It is true of THIS pin only,
    ///   which is exactly why it has to be re-taken when the pin moves.
    #[test]
    fn the_door_probes_name_list_is_re_read_whenever_the_engine_pin_moves() {
        // ⚠ The workspace manifest, from three directories up: `crates/sprag-plugin/src` → root.
        // Compiled IN, so the gate reads the pin this build actually resolved rather than a copy
        // somebody kept in step by hand.
        const MANIFEST: &str = include_str!("../../../Cargo.toml");

        let pinned = MANIFEST
            .lines()
            .find(|line| line.starts_with("sce-rust-runtime = "))
            .and_then(|line| line.split_once("rev = \""))
            .and_then(|(_, rest)| rest.split_once('"'))
            .map(|(rev, _)| rev)
            .unwrap_or_else(|| {
                panic!(
                    "⚠⚠⚠⚠⚠ THE PIN COULD NOT BE READ, so this ratchet is blind rather than \
                     satisfied. `sce-rust-runtime` must be a `[workspace.dependencies]` line \
                     carrying `rev = \"…\"`; if that moved, this reader has to move with it — a \
                     gate that cannot find its second artefact must fail, never pass"
                )
            });

        assert_eq!(
            pinned, DOORS_READ_AT_SCE_REV,
            "⛔⛔⛔⛔ THE ENGINE PIN MOVED AND THE DOOR PROBE'S NAME LIST HAS NOT BEEN RE-READ. \
             Open `sce_publishes_no_door_to_enter_a_machine_at_a_configuration`, read its six \
             spellings against the engine at {pinned}, check whether any extension trait is now \
             implemented for `Engine`, and only then move `DOORS_READ_AT_SCE_REV`. ⚠ A green here \
             is a claim that somebody looked — item 583 exists because a list of names is silent \
             about the seventh one",
        );
    }

    /// ⚠⚠⚠⚠⚠ **AND THE ONE DOOR THAT DOES EXIST IS A REPLAY — item 549's other half, asked of a
    /// RUN.**
    ///
    /// # Why both halves are needed
    ///
    /// SCE-PR90's acceptance criterion 2 is the one with teeth: *`onentry` must not fire*.
    /// Criterion 1 alone cannot tell resumption from replay. The sibling gate above asks whether
    /// a door exists at all; this one measures what the EXISTING way in does, because that is
    /// what a host restarting mid-run is actually left with today.
    ///
    /// # What it measures, in order
    ///
    /// 1. the entry actions of the initial configuration run once, and they are observable
    ///    OUTSIDE the machine — `<send type="x-sprag-host">` reaches this crate's handler. That is
    ///    the analogue of the loop's re-typed prompt: an entry action that leaves the process.
    /// 2. the run moves elsewhere and `get_active_states()` says so — **the READ side works**,
    ///    which is why item 549 is a missing door and not a blind machine.
    /// 3. a second `Engine`, handed nothing but `initialize()`, lands at the DOCUMENT's initial
    ///    configuration rather than at the saved one, and
    /// 4. runs those entry actions a SECOND time.
    ///
    /// So a daemon that restarted mid-run and called `initialize()` would not resume the run; it
    /// would re-send everything the entry actions send. For `ai_loop.scxml` that is the prompt.
    ///
    /// # ⚠⚠ What a red here means
    ///
    /// Either the engine stopped landing at the initial configuration, or it stopped re-running
    /// entry actions on the way in. The second would be a CONTRACT change under the tree — read
    /// `Engine::initialize` at the new rev before believing the good news, because a resumed run
    /// that skips entry actions everywhere is not the same offer as one that skips them once.
    #[test]
    fn the_only_way_into_a_machine_runs_its_entry_actions_again() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        // ⚠ ONE closure builds both engines, so the second is not a different machine dressed as
        // the same one: same document, same registrations, a fresh script session each time.
        let build = || {
            let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
            let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));
            let left_the_process = Arc::new(AtomicUsize::new(0));
            engine.register_event_processor("x-sprag-host", {
                let left_the_process = Arc::clone(&left_the_process);
                move |request| {
                    left_the_process.fetch_add(1, Ordering::SeqCst);
                    vec![sce_rust_runtime::host_processor::HostSendResponse {
                        event_name: request.event_name,
                        event_data: String::new(),
                    }]
                }
            });
            engine.register_invoker("x-sprag-host", |_event| None);
            (engine, left_the_process)
        };

        // ── THE RUN THAT WOULD HAVE BEEN INTERRUPTED ──
        let (mut before, before_sends) = build();
        before.initialize();
        for _ in 0..8 {
            before.tick();
        }
        assert_eq!(
            before_sends.load(Ordering::SeqCst),
            1,
            "the control: entering the initial configuration must fire the `<onentry>` send once, \
             or the count read after the restart below is not counting entry actions at all",
        );

        before.process_event(ProbeSendTypeEvent::Go);
        for _ in 0..8 {
            before.tick();
        }
        let saved = before.get_active_states();
        assert!(
            saved.contains(&ProbeSendTypeState::Invoking)
                && !saved.contains(&ProbeSendTypeState::Sending),
            "⚠⚠⚠ THE READ SIDE IS THE HALF THAT WORKS, and the rest of this gate needs it: the \
             run must have MOVED and be able to say so, or 'it cannot be put back' would be \
             indistinguishable from 'it never went anywhere'. saved = {saved:?}",
        );

        // ── THE RESTART: `initialize()` is the entire vocabulary of ways in ──
        let (mut after, after_sends) = build();
        after.initialize();
        for _ in 0..8 {
            after.tick();
        }
        let landed = after.get_active_states();
        assert!(
            landed.contains(&ProbeSendTypeState::Sending),
            "the control for the assertion below: the fresh engine must have entered the \
             DOCUMENT's initial configuration. landed = {landed:?}",
        );
        assert_ne!(
            landed, saved,
            "⚠⚠⚠⚠⚠ ITEM 549: a machine that came back at the configuration it was saved in would \
             mean the only way in stopped being the document's own start — read `Engine` for the \
             door before celebrating, because this gate cannot tell 'resumed' from 'the initial \
             configuration moved'",
        );
        assert_eq!(
            after_sends.load(Ordering::SeqCst),
            1,
            "⚠⚠⚠⚠⚠ ITEM 549's HARD HALF: the entry actions ran AGAIN on the way in. A zero here \
             says `initialize` stopped executing `<onentry>`, which would change what every run \
             in this crate does on its first turn — check `Engine::initialize` at the pinned rev \
             rather than reading this as resumption having arrived",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A SAVED CONFIGURATION IS ENTERED AND THE ENTRY ACTIONS DO NOT RUN — item 549's
    /// hard half, ANSWERED, and the reason 544 stage 3b is buildable at all.**
    ///
    /// SCE `f3765c95c9` (*"Enter a saved configuration without re-running onentry"*) is
    /// `SCE-PR90` delivered; sprag reached it at pin `ebee8c932a`, taken from `pinion@3d58da75`
    /// as the shared-instance rule requires. This gate is the consumption: the door is DRIVEN
    /// here, not read.
    ///
    /// # Why this is the inverse of the gate above, deliberately
    ///
    /// `the_only_way_into_a_machine_runs_its_entry_actions_again` measures `initialize` and
    /// asserts the sends fire a SECOND time — that is still true and still the reason a restart
    /// cannot resume. This one takes the same document, the same harness and the same counter,
    /// and asserts the new door fires them ZERO times. Two ways in, opposite verdicts, one
    /// observable: a `<send type="x-sprag-host">` that leaves the process. **Entry actions that
    /// stay inside the datamodel would not do** — the loop's re-typed prompt leaves the process,
    /// so the analogue has to as well.
    ///
    /// # ⚠⚠ What this does NOT claim
    ///
    /// The datamodel is NOT restored — `enter_at`'s own doc says so (`<data>` is re-declared at
    /// document defaults and a host must put saved values back through `IScriptEngine`). So this
    /// gate says a run's POSITION survives, not its variables; 544 stage 3b owes the second half
    /// and this test is not evidence for it.
    #[test]
    fn a_saved_configuration_is_entered_without_re_running_onentry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let build = || {
            let lua: Arc<dyn IScriptEngine> = Arc::new(sce_rust_lua::LuaEngine::new());
            let mut engine = Engine::new(ProbeSendTypePolicy::new(lua));
            let left_the_process = Arc::new(AtomicUsize::new(0));
            engine.register_event_processor("x-sprag-host", {
                let left_the_process = Arc::clone(&left_the_process);
                move |request| {
                    left_the_process.fetch_add(1, Ordering::SeqCst);
                    vec![sce_rust_runtime::host_processor::HostSendResponse {
                        event_name: request.event_name,
                        event_data: String::new(),
                    }]
                }
            });
            engine.register_invoker("x-sprag-host", |_event| None);
            (engine, left_the_process)
        };

        // ── THE RUN THAT GETS INTERRUPTED, driven off its initial configuration ──
        let (mut before, before_sends) = build();
        before.initialize();
        for _ in 0..8 {
            before.tick();
        }
        before.process_event(ProbeSendTypeEvent::Go);
        for _ in 0..8 {
            before.tick();
        }
        let saved = before.get_active_states();
        let current = before.get_current_state();
        assert!(
            saved.contains(&ProbeSendTypeState::Invoking)
                && !saved.contains(&ProbeSendTypeState::Sending),
            "the premise: the run must have MOVED off the document's start, or landing at `saved` \
             below proves nothing about resumption. saved = {saved:?}",
        );
        assert_eq!(
            before_sends.load(Ordering::SeqCst),
            1,
            "the control: the entry actions of the initial configuration DID leave the process \
             once, so a zero after the door below is a fact about that door",
        );

        // ── THE RESTART, THROUGH THE NEW DOOR: no `initialize()` anywhere in this arm ──
        let (mut after, after_sends) = build();
        after
            .enter_at(&saved, current)
            .expect("⚠⚠⚠ the saved configuration is a configuration of THIS document, so the door \
                     must not refuse it — a rejection here is `configuration::validate` disagreeing \
                     with `get_active_states`, which would make the pair unusable for resumption");

        assert_eq!(
            after.get_active_states(),
            saved,
            "⚠⚠⚠⚠⚠ the run must come back WHERE IT WAS. Entering 'near' the requested \
             configuration is the one outcome `enter_at` documents it must never produce, and a \
             host has no way to detect it afterwards",
        );
        assert_eq!(
            after_sends.load(Ordering::SeqCst),
            0,
            "⚠⚠⚠⚠⚠ ITEM 549's HARD HALF, and the whole reason this door is worth having: entering \
             a saved configuration must NOT run `<onentry>`. A one here means the door is a \
             REPLAY — for `ai_loop.scxml` that is the run re-typing every prompt its states \
             compose, which is exactly what 543 cannot ship",
        );
    }
}
