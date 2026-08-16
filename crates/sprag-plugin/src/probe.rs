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
            Some(ProbeParallelState::Watching),
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
            Some(ProbeParallelState::Watching),
            "⚠⚠⚠ AND THE ORDERS REGION MUST STILL ANSWER. If a finished sibling silenced it, a \
             person could not stand a run down at the one moment the handle is for",
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
                ProbeParallelState::Counting | ProbeParallelState::Watching
            )),
            "⚠⚠ and it must be a LEAF of one of the regions rather than a region root — a compound \
             state is active while its child is, so an answer that stopped at the region would stop \
             distinguishing the states inside it. Got {:?}",
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
}
