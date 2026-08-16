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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use sce_rust_runtime::{Engine, IScriptEngine, ScriptValue};

    use crate::sm::probe_parallel_sm::{
        ProbeParallelEvent, ProbeParallelPolicy, ProbeParallelState,
    };

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
        // ⚠⚠⚠⚠ AND THE ANSWER THAT DECIDED THE DESIGN, WHICH IS NOT THE ONE THIS GATE WAS DRAFTED
        // EXPECTING. `get_current_state` answers with the PARALLEL ROOT — `Both` — not with the
        // region that finished and not with the one still running. That is correct of the engine
        // (a parallel's "current state" is the parallel) and it is fatal to one design:
        //
        //   `OuterLoop::pump` switches on `get_current_state()` to decide what to DO. A machine
        //   whose top level is `<parallel>` answers that call with one root for ever, so every
        //   arm of that match becomes unreachable in one edit.
        //
        // ⚠⚠⚠ SO STANDING ORDERS MAY NOT BE A TOP-LEVEL REGION BESIDE THE WORK. They can live in a
        // region only if the WORK region is itself a compound state the driver reads through the
        // ACTIVE SET rather than through this call — which is a driver change, not a document one,
        // and is the thing to measure before writing either.
        //
        // ⚠ Asserted as the measurement rather than as a preference: if an engine ever answers with
        // the finished region instead, this gate fails and the design opens back up. That is what a
        // gate on somebody else's engine is for.
        assert_eq!(
            engine.get_current_state(),
            ProbeParallelState::Both,
            "⚠⚠⚠ MEASURED: the single-state reader answers with the PARALLEL ROOT. A driver that \
             switches on it — which `OuterLoop::pump` does — sees one state for ever the moment its \
             top level becomes parallel. This is the fact that decides where standing orders may \
             live",
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
}
