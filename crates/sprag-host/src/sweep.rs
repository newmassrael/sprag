//! The settle waker's SWEEP: one pass over every pane the daemon has, deciding which of them owe an
//! evaluation and paying it for those that do.
//!
//! # Why this is a module and not a closure in the daemon
//!
//! It was a closure in `sprag-term`'s waker thread through slices 3 and 4. Every input it reads is
//! a library type — the [`SessionRegistry`], the [`AgentClock`], the [`ChannelRegistry`] — so the
//! binary held host logic that the library owned every piece of, and the shape of that mistake was
//! the usual one: **the pass could not be called, so it could not be tested and could not be
//! measured.** R260 priced its TERMS one at a time (the per-pane question, the whole-registry read,
//! the prune, the census, the manifest re-read) and had to leave the composition unpriced, because
//! summing terms is not the same claim as running the thing. R261 needed the real pass running
//! against a real registry while another thread served requests, and that is what forced the split.
//!
//! What stays in the daemon is the SCHEDULING — the thread, the park, the `last_sweep` clock, and
//! the manifest re-read that must happen before a pass so the panes an edit invalidates are served
//! by the very pass that invalidated them. What lives here is the WORK, which is a pure function of
//! the registry, the clock and two arguments.
//!
//! # What it costs, and the part that is not the obvious part
//!
//! **Re-measured at R291 against the commit before it (`f7e8b24`) built the same way**, because
//! this pass gained a per-pane `/proc` read and a number that moves needs its control. Three runs
//! each, `--release`, live registry with three real PTY panes.
//!
//! * **A quiet pass now costs the job sample and little else** — 0.46 to 0.64 us at the control,
//!   **12.87 to 14.57 us here**, and the whole difference is one `/proc/<pid>/stat` line per pane.
//!   Against a five-second period that is 0.00025% of one core.
//! * **It is still free to everybody else, and that had to be re-earned.** A concurrent pane-list
//!   reader sees **-10.6 to +0.8 us** on its median and -16.6 to +3.4 us at p99 against a control
//!   sweeping a private registry — indistinguishable from the +0.8 to +1.6 us the control commit
//!   shows. **It was NOT free in the first version of this pass**, which read `/proc` under the
//!   workspace lock: +687 to +3000 us on the median and +41.8 to +51.3 ms at p99. See
//!   [`sweep_once`]'s lock discipline for why the reads moved out.
//! * **A pass in which every pane OWES an evaluation is a different object** — 197.4 to 215.2 us for
//!   three panes here against 158.2 to 185.5 at the control, because the workspace lock is held
//!   across [`AgentClock::observe`] for every pane in that window. That is not hypothetical: **a
//!   manifest reload makes every remembered pane stale at once**, so saving `config.toml` schedules
//!   exactly one such pass, and so does the first pass after boot.
//! * **⚠ R261's recorded 44 to 58 us for that churning pass was already STALE before this round** —
//!   the control commit measures 158.2 to 185.5 us on the same instrument. Nothing in R291 caused
//!   it; it is thirty rounds of a growing detector and a different box, and it is corrected here
//!   rather than left standing because it was only found by running the control at all.
//!
//! The conditions and the one comparison that cannot be made are on [`sweep_once`].

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use sprag_terminal::{PaneId, SessionRegistry, Workspace};

use crate::agent::AgentClock;
use crate::events::Event;
use crate::job::JobWatch;
use crate::notify::ChannelRegistry;

/// What one pass did — returned so a caller can act on it and a test can assert it, rather than
/// inferring the pass's behaviour from its side effects.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Panes visited, which is every pane the daemon has.
    pub visited: usize,
    /// Panes whose screen was actually read — the ones that owed an evaluation. Zero is the steady
    /// state and the number this pass's cost is dominated by when it is not.
    pub evaluated: usize,
    /// Sessions whose published answer MOVED, and so whose clients were woken. A subset of the
    /// sessions visited, and empty on a pass that changed nothing.
    ///
    /// "Answer" is either kind this pass can find — an agent verdict or a foreground job — because
    /// a session is woken ONCE for a pass however many of its facts moved. Counting the two
    /// separately here would be counting wakes that do not happen.
    pub moved: usize,
    /// Panes whose FOREGROUND JOB moved, on a pass that sampled it.
    ///
    /// Reported beside [`moved`](Self::moved) rather than folded into it because the two answer
    /// different questions: that one is how many client sets were woken, this one is how many panes
    /// had news. A test asserting the job half must not have to infer it from a wake that an agent
    /// transition could equally have caused.
    pub jobs_changed: usize,
    /// Panes whose REPORTER's ability to deliver moved — register item 709.
    ///
    /// Counted beside [`jobs_changed`](Self::jobs_changed) and for its reason: it is a different
    /// fact on a different clock, and a gate asserting *this pass noticed the hook go quiet* must not
    /// have to infer it from a wake that an ordinary verdict change would equally have caused.
    ///
    /// ⚠ It counts TRANSITIONS, not mute panes: a reporter that has been mute for an hour moves this
    /// number once. The steady state — every reporter delivering — is zero.
    pub reporters_changed: usize,
}

/// One pass of the sweep: visit every pane, sample its foreground job, evaluate the ones that owe
/// an agent look, prune the trackers of panes that are gone, and wake the clients of the sessions
/// whose answer moved.
///
/// # The two facts, and why one pass carries both
///
/// This pass is the daemon's only observer with a CLOCK, and both facts it publishes need one: an
/// agent verdict rests on a screen and on an absence holding for a settle window, and a foreground
/// job changes when a user types at a shell — neither reaches a dispatch. The dispatch funnel
/// ([`crate::events`]) derives everything it structurally can and says so; these two are what is
/// left.
///
/// They are otherwise independent and are kept so. The job sample is NOT gated on
/// [`AgentRegistry::owes_evaluation`](crate::AgentRegistry::owes_evaluation) — that gate belongs to
/// the settle clock and the ruleset, which have nothing to say about what a shell is running. What
/// they share is the WALK and the WAKE: one pass over the panes, and one
/// [`ChannelRegistry::announce`](crate::ChannelRegistry) per session carrying whatever that pass
/// found for it.
///
/// [`JobWatch`] holds why watching the job is affordable at all: the pass reads its IDENTITY (one
/// `/proc/<pid>/stat` line), never the walk that describes it. **MEASURED (R291), four runs,
/// minima: 9.375 - 12.380 us for three panes including the watch's own bookkeeping, against
/// 2737.59 - 3566.94 us for one `pane_processes` read taken in the same runs** — 236x to 292x, and
/// 0.00025% of one core at [`crate::agent::SWEEP_INTERVAL`].
///
/// It is a real term all the same, and it is the first one this pass pays on EVERY sweep rather
/// than only on a churning one. That is the trade the event is bought with, and the numbers below
/// are re-measured against the commit before it rather than inherited.
///
/// `now` is passed rather than read so a caller can drive the pass at a chosen instant — the tests
/// depend on it and so does any harness that wants a deterministic pass. `discover` is the
/// difference between the waker's two kinds of wake: `false` serves only panes whose deadline has
/// passed, `true` additionally looks for panes nobody has asked about, panes whose ruleset has been
/// replaced, and trackers whose pane is gone. The daemon passes `true` once per
/// [`crate::agent::SWEEP_INTERVAL`].
///
/// # Lock discipline
///
/// The registry lock is taken and RELEASED to clone out the pools, then each workspace lock is taken
/// on its own — never nested, which is what keeps this off the dispatch path's back. The clock's
/// lock is taken INSIDE a workspace lock, because the screen is only reachable there, and never the
/// other way round. The pane-list query takes the same two in the same order for the same reason, so
/// the ordering is a property of both callers rather than a convention this one follows.
///
/// # What the locks cost — MEASURED (R261)
///
/// R260 left this as the one term it would not claim, on the grounds that a single-threaded
/// instrument measures a lock uncontended and uncontended is not what a lock costs. What it costs is
/// the WAIT it inflicts, so the subject is a READER's latency while this runs — measured against a
/// thread serving pane-list requests, with the pass running CONTINUOUSLY rather than once every five
/// seconds, so that a negligible answer settles the real cadence a fortiori.
///
/// **The recurring pass is free — RE-EARNED at R291, not inherited.** Shared minus a control
/// sweeping a PRIVATE registry at the same rate: **-10.6 to +0.8 us on the reader's median and
/// -16.6 to +3.4 us at p99**, against **+0.8 to +1.6 / +1.6 to +5.8** at the commit before this
/// pass sampled the job at all (`f7e8b24`, built the same way). R261's original reading was +0.4 to
/// +0.8 / -2.4 to +0.9, and it held again while another project was building — the paired design
/// survives a 2x change in the box, which is what it is for.
///
/// **⚠ THE FIRST VERSION OF THE JOB SAMPLE BROKE THIS, and only this row said so.** With the
/// `/proc` read inside the pane loop — under the workspace lock, where the screen read already is —
/// the same difference was **+687 to +3000 us on the median and +41.8 to +51.3 ms at p99**. The
/// sweeper was releasing each lock and immediately re-taking it around ~4 us of syscalls per pane,
/// and `std`'s mutex is not fair, so the reader starved.
///
/// The daemon's own duty cycle hides that completely: one pass per
/// [`SWEEP_INTERVAL`](crate::agent::SWEEP_INTERVAL) cannot convoy with itself. **But hiding it is
/// exactly what would have made this instrument useless** — it runs the pass continuously so that a
/// negligible answer settles the real cadence *a fortiori*, and an answer that is not negligible
/// settles nothing at all. So the I/O moved out from under the lock
/// ([`sprag_terminal::foreground_pgid_of`]) rather than the paragraph being rewritten to excuse it.
///
/// **The churning pass cannot be answered the same way, and that is a property of the system rather
/// than of the instrument.** A pane-list request runs the same detector under the same clock, so
/// after a reload the reader evaluates the panes too, and whichever of the two threads arrives first
/// pays. Sharing the registry therefore changes WHO evaluates and not only who waits: the private
/// control's sweeper evaluated three panes on every pass while the shared one evaluated a fraction,
/// the reader having done the rest. The two conditions cannot be matched, so their difference is not
/// a lock cost — R255's shape again, a comparison that cannot be resolved at the level it was asked.
///
/// So that case is bounded DIRECTLY instead, by this pass's own duration: **197.4 to 215.2 us for
/// three panes against 12.87 to 14.57 us quiet**, and a reader wants one window so what it can wait
/// for is that window's share. The consequence is stated rather than left to be met: a manifest edit
/// schedules one pass in which every remembered pane is stale. It is a one-off on a user action,
/// which is why this is documented and not redesigned — and it is the paragraph a future slice that
/// makes evaluations dearer, or lets one window hold many more panes, has to come back and re-read.
///
/// **⚠ The pair this replaces read 44 to 58 us against 0.37 to 0.58, and the first of those was
/// already wrong before R291 touched anything**: the control commit measures 158.2 to 185.5 us for
/// the churning pass on the same instrument. Thirty rounds of a growing detector, not this change —
/// and the only reason it was caught is that a number this round moved got a control, which is the
/// argument for giving one to every number that moves.
/// ⚠⚠⚠⚠⚠ `mute` IS NAMED BY THE CALLER AND NEVER DERIVED HERE — register item 700's ruling, which
/// this pass is the third site to obey. The breadcrumb a hook leaves lives in a state directory and
/// belongs to a daemon GENERATION (item 711), and a pass that resolved both itself would be asserting
/// whatever host and whichever daemon it happened to run under: two gates already went red on a
/// developer's machine and green on CI from one commit for exactly that. The one production caller
/// says [`crate::durability::state_dir`] and [`crate::wire::generation`] out loud, and a fixture that
/// says nothing does not compile.
pub fn sweep_once(
    registry: &Mutex<SessionRegistry>,
    agents: &AgentClock,
    jobs: &JobWatch,
    channels: &ChannelRegistry,
    mute: &crate::hooks::MuteReader<'_>,
    now: Instant,
    discover: bool,
) -> SweepReport {
    // Phase 1 — registry lock ONLY: clone out each session's pools as handles, keeping the session
    // NAME beside each so a published change can wake that session's clients and no others.
    let pools: Vec<(String, std::sync::Arc<Mutex<Workspace>>)> = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        reg.sessions()
            .iter()
            .flat_map(|session| {
                let name = session.name().to_owned();
                session
                    .windows()
                    .iter()
                    .map(move |window| (name.clone(), std::sync::Arc::clone(window.workspace())))
            })
            .collect()
    };

    // Phase 2 — registry lock released. Each pool under its own lock.
    let mut report = SweepReport::default();
    let mut live: HashSet<PaneId> = HashSet::new();
    // Per SESSION, everything this pass found — the wake and its reasons, collected together so
    // they cannot disagree, and so that a session with news of BOTH kinds is woken ONCE. Two
    // `announce` calls for one pass would bump a session's revision twice and send its clients
    // round twice for a single observation.
    let mut woken: HashMap<String, Vec<Event>> = HashMap::new();
    for (session, pool) in &pools {
        // What the foreground-job sample needs out of this window, collected UNDER the lock and
        // read OUTSIDE it. See the loop below for why the two halves are split at all.
        let mut children: Vec<(PaneId, Option<u32>)> = Vec::new();
        // ⚠⚠⚠⚠⚠ AND WHICH PANES HOLD A REPORT — collected under the lock for `children`'s reason,
        // and the evidence about each read AFTER it drops (register item 709).
        //
        // The whole file this pass consults is one `openat` that usually answers ENOENT, and it is
        // still not read here: the paragraph on this function's own docs is a MEASUREMENT — with the
        // job sample's `/proc` reads inside this loop a concurrent pane-list reader's median went
        // from +0.8 us to +687 us, because the sweeper released the lock and immediately re-took it
        // around a few microseconds of syscalls per pane and `std`'s mutex is not fair. *"The I/O
        // moved out from under the lock rather than the paragraph being rewritten to excuse it."*
        //
        // ⚠ Only panes that HOLD a report are collected: a pane nothing has reported for has no
        // report to set aside, so asking about its reporter's health is a syscall spent on a question
        // with no consequence. `reported` is one hash lookup and it is already being taken two lines
        // below for the release checks.
        let mut reporters: Vec<PaneId> = Vec::new();
        let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
        for pane in pool.panes() {
            let id = pane.id();
            live.insert(id);
            report.visited += 1;
            // THE FOREGROUND JOB, first half: the pane's child pid, which is a field read.
            //
            // Only on a SWEEP. `discover` means "additionally look for what nobody has asked
            // about", which a job change is by construction — nothing requests one and no dispatch
            // produces one. That makes `SWEEP_INTERVAL` the latency bound, and the ceiling is not
            // this line's to lower: the waker parks with `park_until_due(SWEEP_INTERVAL)`, so a
            // faster job cadence means moving the park, not adding a constant here that would look
            // independent and would not be.
            //
            // It is deliberately ABOVE the agent gate's `continue`: a job change is a different fact
            // on a different clock, and gating it on `owes_evaluation` would tie it to the settle
            // window and the ruleset, which have nothing to say about what a shell is running.
            if discover {
                children.push((id, pane.pty().pid()));
            }
            // A REPORT outlives the process that made it, and this is what bounds that. The thing
            // that expires is the REPORTER, and it can go in two ways the daemon can see:
            //
            // * the pane's CHILD is gone, so nothing inside the pane can release, correct or
            //   contradict what it last said; or
            // * the report named an OWNER — the process group that held the pane's terminal when it
            //   spoke — and that group no longer exists. This is the shape an agent normally has:
            //   the pane's child is a shell, the agent is what the user typed at its prompt, and
            //   killing it leaves the shell alive, so the first test never fires for it. Without
            //   this one a crashed agent parks `working` on a pane sitting at a prompt forever,
            //   which is the worst answer a status can give.
            //
            // The cheap side first in both, and the agent lock only if it holds: `is_eof` is one
            // atomic load and the owner check is one `kill(2)`, while the lock is contended by every
            // client wake (R261 measured what this loop's locks cost). The release makes the pane owe
            // a look, which `owes_evaluation` below serves in THIS pass rather than the next one.
            let reporter_gone = if pane.pty().is_eof() {
                agents.with(|state| state.reported(id))
            } else {
                agents.with(|state| state.orphaned(id))
            };
            if reporter_gone {
                agents.release(id);
            }
            // ⚠⚠⚠⚠⚠ **AND THE THIRD WAY A REPORTER CAN STOP SPEAKING, which neither test above can
            // reach** — register item 709. The two above ask whether the reporter's PROCESS is gone.
            // A hook whose process is alive and healthy and whose DELIVERY is refused is neither: it
            // leaves word on the filesystem (`crate::hooks::MuteReader`) precisely because the daemon
            // it could not reach is the one party that cannot learn this from a report.
            //
            // ⚠⚠ THE ANSWER IS NOT A RELEASE, and that is the difference. A release DROPS the report,
            // so the pane can only regain an authority when a new one arrives — measured on a live
            // loop, a hook is intermittent (mute at 23:24, released at 23:25, reporting and mute
            // again at 23:35), which makes a one-shot demotion a state that gets reverted rather than
            // an expiry. The reading below is re-taken every pass and the report is HELD throughout,
            // so it comes back on its own the moment the evidence is taken back.
            //
            // ⚠ Gated on `discover`, like the job sample and for its stated reason: this is a *what
            // changed while nobody was asking* fact, so `SWEEP_INTERVAL` is its latency bound. The
            // recovery direction does not wait for it — `Tracker::report` clears the reading on
            // arrival, because a report arriving IS the delivery succeeding.
            if discover && agents.with(|state| state.reported(id)) {
                reporters.push(id);
            }
            // Three reasons to ask about a pane — due, unknown, stale — and none of them applies to
            // a settled pane under unchanged rules, which is every pane in a quiet workspace.
            // `AgentRegistry::owes_evaluation` holds the composition and the argument for each.
            if !agents.with(|state| state.owes_evaluation(id, now, discover)) {
                continue;
            }
            report.evaluated += 1;
            let before = agents.with(|state| state.seq(id));
            let title = pane.title();
            pane.pty().with_screen(|screen| {
                agents.observe(
                    id,
                    screen,
                    title.as_deref(),
                    now,
                    crate::config::agent_settle,
                );
            });
            if agents.with(|state| state.seq(id)) != before {
                // ⚠⚠⚠⚠⚠ **THE PANE MOVED, AND NOTHING WAS TYPED AT IT** — register item 646, and
                // this is the SETTLE half of it: a candidate the tracker has been holding publishes
                // on THIS pass, with the screen unchanged since the observation that raised it. A
                // waiter parked on the pane's counter would sleep through exactly the change
                // `Settling::At` told it to come back for.
                //
                // ⚠⚠ BEFORE the event is queued, for the reason the report path states at length:
                // the announce is what sends the pass that re-evaluates parked waits, and that pass
                // reads this counter.
                pane.pty().revision().bump();
                woken
                    .entry(session.clone())
                    .or_default()
                    .push(Event::AgentStateChanged(id.0));
            }
            // THE LOOP'S LIVENESS RESTS ON THIS. `park_until_due` returns immediately for a deadline
            // already past, so a due pane that an observation does not RESOLVE sends the waker round
            // at full speed forever. It cannot happen: `is_due` is `since + settle <= now`, which is
            // exactly `settle`'s own publish condition, and every other path through `observe` either
            // publishes or re-dates the candidate to `now`. Stated as an assertion rather than a
            // comment because the failure is a spin rather than a wrong answer — the mutation that
            // removed the observe above took the scene revision from 264 to 6,178,283 in twelve
            // seconds.
            debug_assert!(
                !agents.with(|state| state.is_due(id, now)),
                "pane {id} is still due after being observed at the same instant",
            );
        }
        // THE WORKSPACE LOCK IS RELEASED HERE, and the `/proc` reads happen after it. **This is not
        // tidiness — it is measured.** With the reads inside the loop above, a concurrent pane-list
        // reader's median went from +0.8 us to +687 us and its p99 from +5.8 us to +41.8 ms against
        // the private-registry control, because the sweeper released the lock and immediately
        // re-took it around 4 us of syscalls per pane: a convoy, and `std`'s mutex is not fair.
        //
        // The daemon's own duty cycle hides it — one pass per `SWEEP_INTERVAL` cannot convoy with
        // itself — but that made the instrument's whole argument unavailable. R261 measures these
        // locks by running the pass CONTINUOUSLY so that a negligible answer settles the real
        // cadence a fortiori, and an answer that is not negligible settles nothing.
        drop(pool);
        for (id, child) in children {
            // `PanePty::pid` already answered `None` for a reaped child, which is what stops a
            // recycled pid being read; `foreground_pgid_of` is the same read `PanePty::foreground_pgid`
            // performs, named so it can be made without a pane in hand.
            if jobs.observe(id, child.and_then(sprag_terminal::foreground_pgid_of)) {
                report.jobs_changed += 1;
                woken
                    .entry(session.clone())
                    .or_default()
                    .push(Event::PaneJobChanged(id.0));
            }
        }
        // AND THE REPORTERS' OWN WORD ABOUT WHETHER THEY CAN STILL DELIVER, off the filesystem and
        // out from under the lock — register item 709.
        for id in reporters {
            // ⚠⚠⚠⚠⚠ **ONLY `Mute` DEMOTES, AND THE THREE OTHER ARMS ARE A DECISION RATHER THAN AN
            // OVERSIGHT** — the one register item 711 made for the REPORTING surfaces, kept here so
            // the daemon and those surfaces cannot come to disagree about one fact.
            //
            // * `Speaking` — no breadcrumb. Nothing to weigh.
            // * `Inherited` — a breadcrumb an EARLIER daemon generation left under this number. Its
            //   subject is a pane that no longer exists; the number was reissued by this daemon's
            //   counter.
            // * `Unattributed` — a breadcrumb that names no generation at all, so it could belong to
            //   any earlier holder of the number.
            //
            // Acting on either of the last two is exactly what happened on 2026-08-26: a breadcrumb
            // from 14:02 was read against a live pane 4 whose child had started at 22:57, and a
            // watcher took a HEALTHY reporter off its hook. Demoting on them here would be the same
            // mistake with no watcher in the loop to notice — item 711's gate holds that line, and
            // this is the daemon end of it.
            //
            // ⚠ The residue that decision accepts, stated rather than hidden: a breadcrumb written by
            // an image older than item 711 names no generation, so a report standing behind it keeps
            // outranking the screen. That is the class already measured to be lying about live panes,
            // and the hook and this daemon ship in ONE binary — so the window is a stale build
            // directory, not a supported pair. It is still a loss, and it is the one 709 does not
            // close.
            let mute = matches!(mute.word_from(id.0), crate::hooks::MuteWord::Mute { .. });
            if agents.set_reporter_mute(id, mute) {
                report.reporters_changed += 1;
                // The pane's published verdict is about to be re-derived from its screen, or handed
                // back to the report that was set aside. Either way what a client holds is stale, and
                // this is the only thing that will say so: the case is a pane whose screen has
                // stopped moving, so no output event is coming.
                woken
                    .entry(session.clone())
                    .or_default()
                    .push(Event::AgentStateChanged(id.0));
            }
        }
    }

    // A tracker must not outlive its pane. The census is DAEMON-WIDE, which is why it is built here
    // and never in the pane-list query: that walk sees one session, and pruning against it would
    // forget every other session's panes.
    if discover {
        agents.with(|state| state.retain_live(&live));
        jobs.retain_live(&live);
    }
    // Wake the clients of the sessions whose published answer moved, and only those — and tell
    // them WHAT moved, in the same call, so the wake and its reasons land together
    // (`ChannelRegistry::announce` holds the journal lock across the bump for exactly that).
    //
    // ONE call per session, carrying every event this pass found for it, for the reason `woken` is
    // built this way: the announce IS the wake, so a second one is a second wake.
    report.moved = woken.len();
    for (session, events) in woken {
        channels.announce(&session, events);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use sprag_detect::{DEFAULT_SETTLE, Report, Ruleset, built_ins};
    use sprag_terminal::CommandBuilder;

    /// **THE GENERATION THESE GATES' PANES BELONG TO** — the second half of a breadcrumb's subject,
    /// which a fixture has to SUPPLY rather than inherit (register item 711).
    ///
    /// A breadcrumb is filed under a pane NUMBER and the next daemon's counter reissues that number,
    /// so a reader holding only a directory answers about whoever held the number before. The
    /// production caller says [`crate::wire::generation`]; a fixture has no daemon to mint one, so it
    /// makes one and stamps its own breadcrumbs with it.
    const GATE_GENERATION: &str = "sweep-gate.0";

    /// A directory THIS GATE OWNS, standing where the daemon's state directory would be — register
    /// item 700's ruling, which is why [`sweep_once`] takes the reader instead of deriving it.
    ///
    /// Named per gate so two running at once cannot see each other's breadcrumbs, and emptied on
    /// creation so a previous run's cannot be inherited either.
    fn nobody_left_word(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "sprag-sweep-mute-{}-{label}-{:?}",
            std::process::id(),
            std::thread::current().id(),
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a directory this gate owns");
        dir
    }

    /// A reader standing in `dir`, asking about panes of [`GATE_GENERATION`].
    fn looking_in(dir: &std::path::Path) -> crate::hooks::MuteReader<'_> {
        crate::hooks::MuteReader::new(dir, Some(GATE_GENERATION))
    }

    /// A reader over a directory NOBODY HAS LEFT WORD IN — for every gate here whose subject is not
    /// a reporter's health.
    ///
    /// One shared empty directory rather than one per gate, because what these gates need is the
    /// ABSENCE and an absence cannot collide: nothing here ever writes into it. ⚠ It is still a
    /// directory this file owns and not the ambient state home — the whole reason [`sweep_once`]
    /// takes the reader is that a pass which derived it would assert whatever host it ran on
    /// (register item 700).
    fn no_word() -> crate::hooks::MuteReader<'static> {
        static DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
        crate::hooks::MuteReader::new(
            DIR.get_or_init(|| nobody_left_word("no-word")),
            Some(GATE_GENERATION),
        )
    }

    /// **WHAT IS PUBLISHED FOR ONE PANE**, through the daemon's own door — [`AgentRegistry::observe`]
    /// over that pane's live screen, which is exactly what a pane-list request runs.
    ///
    /// The pass under test does not hand its readings back, and neither does it need to: the
    /// published verdict is a fact about the registry, and asking the registry for it is what every
    /// client does. ⚠ A `None` answer means no manifest claims the pane or nothing has settled yet,
    /// which is never what a gate here means to assert — so it panics rather than being folded into
    /// an `Option` a caller could forget to check.
    fn published(
        reg: &Arc<Mutex<SessionRegistry>>,
        agents: &AgentClock,
        session: &str,
        id: PaneId,
        now: Instant,
    ) -> crate::AgentFacts {
        let pool = {
            let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
            guard.workspace_of(session).expect("the session")
        };
        let guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
        let pane = guard.pane(id).expect("the pane this gate opened");
        let title = pane.title();
        pane.pty()
            .with_screen(|screen| {
                agents.observe(
                    id,
                    screen,
                    title.as_deref(),
                    now,
                    sprag_detect::Hysteresis::default,
                )
            })
            .expect("a pane some manifest claims, with something published for it")
    }

    /// A pane that paints `text` and then blocks on its PTY forever, so nothing it does can land in
    /// the middle of a pass.
    fn painting(text: &str) -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%s' \"{text}\"; exec cat"));
        command.env("TERM", "dumb");
        command
    }

    /// The `claude` footer fingerprint — a pane the first built-in manifest claims, so its verdict
    /// can MOVE. A plain shell pane's never does, which is what the two-session test needs.
    fn claude_pane() -> CommandBuilder {
        painting("  \u{23f8} manual mode on \u{b7} ? for shortcuts")
    }

    /// A `claude` pane whose screen a RULE actually answers for — the WORKING footer, so
    /// `working-footer` fires and the pane has a published state rather than `Unknown`.
    ///
    /// ⚠ [`claude_pane`] is claimed by the idle-footer fingerprint and matched by no RULE (its title
    /// is empty, so `idle-glyph` misses), which reads `Unknown` — and an observation is never
    /// produced for a pane with no state. A gate that needs the SCREEN to have an answer to fall back
    /// to has to paint one.
    ///
    /// ⚠⚠ And NOT a dialog: a choice list on the screen outranks a standing report all by itself
    /// (register item 524), which would make *the screen answered* true for a reason that has nothing
    /// to do with the reporter's health.
    fn working_claude_pane() -> CommandBuilder {
        painting("  \u{23f8} esc to interrupt")
    }

    /// ⛔⛔⛔⛔⛔ **A REPORTER THAT LEFT WORD IT CANNOT DELIVER DOES NOT OUTRANK THE SCREEN — AND THE
    /// REPORT COMES BACK WHEN THE WORD IS TAKEN BACK** — register item 709, both arms on ONE pane.
    ///
    /// # The defect, measured before this gate existed
    ///
    /// A report outranks the screen and does not expire, so the last thing a reporter MANAGED to say
    /// stands for ever once its channel breaks. Measured 2026-08-16 on a live run: a pane's screen
    /// held `MILESTONE REACHED` for over an hour while every surface answered `working
    /// source=hook:claude`, and the journal repeated *looked, nothing had happened*. The one command
    /// that freed it was `sprag release-agent`, a PERSON's to call — and running when no person is
    /// there is this loop's whole purpose. Item 344 named the real defect in one line: **silence is
    /// not *unknown*, it is the last thing heard.**
    ///
    /// # ⚠⚠⚠⚠⚠ Why BOTH arms, and why one of them alone would prove nothing
    ///
    /// A build that simply dropped every report would pass arm (a) perfectly. A build that ignored
    /// the evidence entirely would pass arm (b) perfectly. **The fork is the claim**, and it has to be
    /// asked of the same pane, in one gate, with nothing between the two but the breadcrumb.
    ///
    /// ⚠ And it is why the demotion is not a `release`: a release DROPS the report, so arm (b) could
    /// only pass if a NEW report arrived. Nothing new arrives here — the word is simply taken back —
    /// which is what makes this an expiry rather than a one-shot. Measured on a live loop, a hook is
    /// intermittent (mute at 23:24, released at 23:25, reporting and mute again at 23:35), so a
    /// one-shot demotion is a state that gets reverted rather than a rule that holds.
    ///
    /// # ⚠⚠⚠ The premises are asserted INSIDE, because either one would make this vacuous
    ///
    /// * **The screen and the report must DISAGREE.** The screen's own answer is taken first and kept,
    ///   rather than hard-coded, so *picked the screen* and *picked the report* stay distinguishable
    ///   whatever the built-in manifests say today — and the gate asserts the two words differ before
    ///   it asks anything else.
    /// * **The breadcrumb must really be on disk, and really be gone.** Both are read back from the
    ///   filesystem, through the path the product writes, in the directory this gate names. A gate
    ///   that trusted its own `note_mute` calls would pass against a writer that wrote nothing.
    /// * **The pass must NOTICE.** `reporters_changed` is asserted on each transition: a build that
    ///   never took the reading would leave it at zero while both arms still agreed by accident.
    #[test]
    fn a_mute_reporter_does_not_outrank_the_screen_and_comes_back_when_it_can_deliver() {
        let dir = nobody_left_word("mute-outranks-nothing");
        let crumb = dir.join(format!("hook-mute.{}", 0));
        let reg = registry_with(&[("a", working_claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();
        let id = PaneId(0);

        // ── 0. WHAT THE SCREEN SAYS, taken before anything has reported. Two passes because a
        //       scrape goes through the settle window and the first one only raises the candidate.
        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &looking_in(&dir),
            base,
            true,
        );
        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &looking_in(&dir),
            base + DEFAULT_SETTLE,
            true,
        );
        let screen = published(&reg, &agents, "a", id, base + DEFAULT_SETTLE);
        assert!(
            screen.source.is_none() && screen.rule.is_some(),
            "⚠ THE STAGING: nothing has reported yet, so this pane's answer is its screen's and a \
             rule has to have fired for there to be an answer to come back to: {screen:?}",
        );
        let off_the_screen = screen.state;

        // ── 1. A REPORT IN FORCE, saying something the screen does not.
        let (outcome, _) = agents.report(
            id,
            Report {
                state: sprag_detect::AgentState::Blocked,
                agent: Some("claude".to_owned()),
                source: "hook:claude".to_owned(),
                seq: Some(1),
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                running: None,
                transcript: None,
                build: None,
            },
            sprag_detect::Hysteresis::default,
        );
        assert!(outcome.accepted, "the hook's report must be taken");
        let believed = published(&reg, &agents, "a", id, base + DEFAULT_SETTLE);
        assert_eq!(
            believed.source.as_deref(),
            Some("hook:claude"),
            "⚠ THE STAGING: a report outranks the screen, which is the behaviour being qualified — \
             not removed: {believed:?}",
        );
        assert_ne!(
            believed.state, off_the_screen,
            "⚠⚠ THE PREMISE THAT MAKES BOTH ARMS DISTINGUISHABLE: the report and the screen have to \
             disagree, or *picked the screen* and *picked the report* are the same assertion",
        );
        assert!(
            !believed.reporter_mute,
            "and nothing has left word yet — see the arm below for why the FACT is published too",
        );

        // ── 2. ARM (a): THE REPORTER LEAVES WORD IT COULD NOT DELIVER, through the product's own
        //       writer and stamped with the generation this gate's reader asks about (item 711).
        crate::hooks::note_mute(
            &dir,
            id.0,
            Some(GATE_GENERATION),
            Some("the daemon refused the report: no pane 0 on this host"),
        );
        assert!(
            crumb.exists(),
            "⚠ THE PREMISE: the breadcrumb has to be on disk at {} — without a file, everything \
             below is vacuous",
            crumb.display(),
        );
        let noticed = sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &looking_in(&dir),
            base + DEFAULT_SETTLE,
            true,
        );
        assert_eq!(
            noticed.reporters_changed, 1,
            "⚠⚠ THE PASS HAS TO NOTICE. A build that never takes the reading leaves this at zero \
             while the arms below could still agree by accident",
        );
        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &looking_in(&dir),
            base + DEFAULT_SETTLE * 3,
            true,
        );
        let demoted = published(&reg, &agents, "a", id, base + DEFAULT_SETTLE * 3);
        assert_eq!(
            demoted.state, off_the_screen,
            "⛔⛔⛔⛔⛔ ARM (a), AND THE WHOLE ITEM: the reporter has left word it cannot deliver, so \
             the state it last MANAGED to say must stop being the answer and the pane's own screen \
             must be. An hour of a frozen `MILESTONE REACHED` under a stale `working` is what this \
             costs when nothing weighs the evidence: {demoted:?}",
        );
        assert!(
            demoted.source.is_none() && demoted.rule.is_some(),
            "⚠⚠⚠ AND THE AUTHORITY SAYS SO. Publishing a `source` beside the rule that answered \
             would break this wire's own invariant — every reader derives *this was reported* from \
             the presence of `source`, so a driver would go on treating a screen reading as exact: \
             {demoted:?}",
        );
        assert!(
            demoted.reporter_mute,
            "⚠⚠⚠⚠ AND THE REASON IS CARRIED. Without it the authority changes and nothing anywhere \
             says what changed it — which is item 709's body: a person could see the cause with one \
             CLI call and a DRIVER could not see it at all: {demoted:?}",
        );

        // ── 3. ARM (b): THE WORD IS TAKEN BACK — the same call the hook makes when a delivery
        //       succeeds — and NOTHING NEW IS REPORTED. The report that was set aside comes back.
        crate::hooks::note_mute(&dir, id.0, Some(GATE_GENERATION), None);
        assert!(
            !crumb.exists(),
            "⚠ THE OTHER PREMISE: the breadcrumb has to be GONE at {} — a gate that trusted its own \
             call would pass against a writer that removes nothing",
            crumb.display(),
        );
        let cleared = sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &looking_in(&dir),
            base + DEFAULT_SETTLE * 3,
            true,
        );
        assert_eq!(
            cleared.reporters_changed, 1,
            "the pass has to notice the recovery too, and for the same reason",
        );
        let believed_again = published(&reg, &agents, "a", id, base + DEFAULT_SETTLE * 5);
        assert_eq!(
            believed_again.source.as_deref(),
            Some("hook:claude"),
            "⛔⛔⛔⛔ ARM (b): a DEMOTION THAT CANNOT BE UNDONE IS A ONE-SHOT, NOT AN EXPIRY. Nothing \
             new has reported here — the word was simply taken back — so a build that dropped the \
             report instead of setting it aside has nothing to give the pane back, and a hook that \
             recovers stays disbelieved until it happens to report again: {believed_again:?}",
        );
        assert_eq!(
            believed_again.state, believed.state,
            "and it is the SAME report, with the state it claimed, rather than a fresh guess",
        );
        assert!(
            !believed_again.reporter_mute,
            "and the published reason clears with it",
        );
    }

    /// A REPORT dies with its reporter: a pane whose child has exited loses its authority on the next
    /// sweep, and the same pass gives the pane back to the screen.
    ///
    /// This is what makes the report need no expiry clock. A hook releases when its agent finishes, but
    /// a KILLED agent runs no hook — so something has to notice that the process which spoke is gone,
    /// and the daemon can see exactly that. Without it an authoritative `working` would outlive the
    /// agent forever, and no screen reading could ever correct it.
    ///
    /// The screen here is a `claude` footer painted by a child that then EXITS, so the pane keeps its
    /// output (a pane holds its place after its child dies) and the scrape has a real verdict to
    /// return to.
    #[test]
    fn a_report_dies_with_the_process_that_made_it() {
        // Painted, then gone: `printf` without the `exec cat` the other fixtures block on.
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("printf '%s' \"  \u{23f8} manual mode on \u{b7} ? for shortcuts\"");
        command.env("TERM", "dumb");
        let reg = registry_with(&[("a", command)]);
        let channels = Arc::new(ChannelRegistry::default());
        let jobs = JobWatch::new();
        let agents = Arc::new(AgentClock::new(Ruleset::new(built_ins())));
        let id = PaneId(0);

        // Wait for the child to be GONE rather than sleeping: the release is keyed on EOF, so a pass
        // taken before the exit lands would be measuring the wrong moment.
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline {
            let pool = {
                let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard.workspace_of("a").expect("the session")
            };
            let gone = {
                let guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
                guard.pane(id).is_some_and(|pane| pane.pty().is_eof())
            };
            if gone {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        let (outcome, _) = agents.report(
            id,
            Report {
                state: sprag_detect::AgentState::Blocked,
                agent: Some("claude".to_owned()),
                source: "hook".to_owned(),
                seq: None,
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                running: None,
                transcript: None,
                build: None,
            },
            sprag_detect::Hysteresis::default,
        );
        assert!(
            outcome.accepted,
            "the report is taken while the pane exists"
        );
        assert!(agents.with(|state| state.reported(id)));

        // One sweep, far enough ahead that a resting candidate the scrape creates can also settle.
        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &no_word(),
            Instant::now() + DEFAULT_SETTLE * 2,
            true,
        );
        assert!(
            !agents.with(|state| state.reported(id)),
            "the reporter is gone, so its authority is gone",
        );
        assert!(
            !agents.with(|state| state.any_owes_look()),
            "and the same pass re-derived the verdict rather than leaving the pane owing one",
        );
    }

    /// A process in a group of its OWN, standing in for an agent the pane did not spawn.
    ///
    /// `setpgid` is what makes it usable: a spawned child inherits its parent's process group, so
    /// without this the "agent's" group would be the TEST RUNNER's, and the kill that retires the
    /// report would take the test with it. With it, the child's pid is its pgid.
    fn agent_in_its_own_group() -> std::process::Child {
        use std::os::unix::process::CommandExt as _;
        let mut command = std::process::Command::new("/bin/sleep");
        command.arg("300");
        // SAFETY: `setpgid` is async-signal-safe and runs in the forked child before `exec`, which
        // is the only place `pre_exec` bodies are allowed to do anything.
        unsafe {
            command.pre_exec(|| match libc::setpgid(0, 0) {
                0 => Ok(()),
                _ => Err(std::io::Error::last_os_error()),
            });
        }
        command.spawn().expect("a stand-in agent")
    }

    /// A report dies with the process that made it EVEN WHEN that process was not the pane's child —
    /// which is the shape an agent normally has.
    ///
    /// The pane's child is long-lived here and never reaches EOF, so the rule this exercises is the
    /// only one that can fire. That is asserted rather than assumed: a user types `claude` at the
    /// prompt of the shell sprag spawned, so killing the agent leaves the shell alive, and the EOF
    /// rule — the whole of what slice 2 had — never sees it. Without this an authoritative `working`
    /// would sit on a pane showing a shell prompt forever.
    ///
    /// The CONTROL is the first sweep, taken while the stand-in is still running: without it this
    /// passes on a rule that retires every bound report on sight.
    #[test]
    fn a_bound_report_dies_with_a_process_the_pane_did_not_spawn() {
        let reg = registry_with(&[("a", claude_pane())]);
        let channels = Arc::new(ChannelRegistry::default());
        let jobs = JobWatch::new();
        let agents = Arc::new(AgentClock::new(Ruleset::new(built_ins())));
        let id = PaneId(0);

        let mut agent = agent_in_its_own_group();
        let owner = agent.id();
        agents.report(
            id,
            Report {
                state: sprag_detect::AgentState::Blocked,
                agent: Some("claude".to_owned()),
                source: "hook:claude".to_owned(),
                seq: None,
                owner: Some(u64::from(owner)),
                asked: None,
                said: None,
                noticed: None,
                running: None,
                transcript: None,
                build: None,
            },
            sprag_detect::Hysteresis::default,
        );

        let alive = Instant::now() + DEFAULT_SETTLE * 2;
        sweep_once(&reg, &agents, &jobs, &channels, &no_word(), alive, true);
        assert!(
            agents.with(|state| state.reported(id)),
            "CONTROL: the agent is still running, so its report stands",
        );

        agent.kill().expect("kill the stand-in agent");
        agent.wait().expect("reap it, so its group is really gone");

        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &no_word(),
            alive + DEFAULT_SETTLE,
            true,
        );
        assert!(
            !agents.with(|state| state.reported(id)),
            "the agent is gone, so its authority is gone",
        );
        assert!(
            !agents.with(|state| state.any_owes_look()),
            "and the same pass re-derived the verdict rather than leaving the pane owing one",
        );
    }

    /// An UNBOUND report is never retired by that rule, and this is the property most easily broken
    /// by it: `sprag report-agent` is a person saying what a pane is doing, and the command they
    /// said it with has already exited by the time anything could ask about it. Their report is
    /// theirs to withdraw, with `release-agent`, and nobody else's to expire.
    ///
    /// Sweeps run long past the point the bound test above was already released at.
    #[test]
    fn an_unbound_report_is_nobody_elses_to_expire() {
        let reg = registry_with(&[("a", claude_pane())]);
        let channels = Arc::new(ChannelRegistry::default());
        let jobs = JobWatch::new();
        let agents = Arc::new(AgentClock::new(Ruleset::new(built_ins())));
        let id = PaneId(0);

        agents.report(
            id,
            Report {
                state: sprag_detect::AgentState::Blocked,
                agent: Some("claude".to_owned()),
                source: "cli".to_owned(),
                seq: None,
                owner: None,
                asked: None,
                said: None,
                noticed: None,
                running: None,
                transcript: None,
                build: None,
            },
            sprag_detect::Hysteresis::default,
        );
        let mut now = Instant::now();
        for _ in 0..3 {
            now += DEFAULT_SETTLE * 2;
            sweep_once(&reg, &agents, &jobs, &channels, &no_word(), now, true);
        }
        assert!(
            agents.with(|state| state.reported(id)),
            "nothing was bound to this report, so nothing can retire it",
        );
    }

    /// A registry with one session per entry in `panes`, each holding that one pane, and every pane
    /// waited for so a pass reads a painted screen rather than a blank one.
    fn registry_with(panes: &[(&str, CommandBuilder)]) -> Arc<Mutex<SessionRegistry>> {
        let reg = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        for (session, command) in panes {
            let pool = {
                let mut guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard
                    .new_session(Some(session))
                    .expect("a fresh session name");
                guard.workspace_of(session).expect("the session just made")
            };
            let mut guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
            guard
                .spawn(command.clone(), (*session).to_owned(), 80, 24)
                .expect("a pane on a pty");
        }
        wait_for_paint(&reg);
        reg
    }

    /// Block until every pane has painted something, reading the panes rather than sleeping for a
    /// guessed interval — a pass over a blank screen is a different measurement.
    fn wait_for_paint(reg: &Arc<Mutex<SessionRegistry>>) {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline {
            let pools: Vec<_> = {
                let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard
                    .sessions()
                    .iter()
                    .flat_map(|s| s.windows().iter().map(|w| Arc::clone(w.workspace())))
                    .collect()
            };
            let painted = pools.iter().all(|pool| {
                let guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
                guard.panes().iter().all(|pane| {
                    pane.pty().with_screen(|screen| {
                        screen.row_generation(0).is_some_and(|written| written > 0)
                    })
                })
            });
            if painted {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("a pane never painted");
    }

    fn clock() -> AgentClock {
        AgentClock::new(Ruleset::default())
    }

    /// The pass's whole cost story, at the level the cost is claimed: the FIRST pass over a daemon
    /// evaluates every pane because nobody has ever looked at one, and the next pass over the same
    /// quiet workspace evaluates NONE.
    ///
    /// R253 found the first half by driving the binary — until slice 3 the only thing that observed
    /// was the pane-list query, so a daemon nobody had queried held no state for any pane. R260
    /// priced the second half. Neither could be asserted at this level before the pass was a
    /// function.
    #[test]
    fn a_first_pass_discovers_every_pane_and_the_next_evaluates_none() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        let first = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(first.visited, 2);
        assert_eq!(
            first.evaluated, 2,
            "a pane nobody has looked at is invisible to every other question, so only a sweep \
             can give it a state",
        );

        let second = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(second.visited, 2);
        assert_eq!(
            second.evaluated, 0,
            "the steady state of a quiet workspace, and the answer the sweep's cost rests on",
        );
    }

    /// A reload makes the next pass evaluate every pane again — slice 4's layering, asserted where
    /// the work actually happens rather than on the registry alone.
    #[test]
    fn a_reload_makes_the_next_pass_evaluate_every_pane() {
        let reg = registry_with(&[("a", claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(
            sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true).evaluated,
            0
        );

        agents.with(|state| state.reload(Ruleset::new(built_ins())));
        assert_eq!(
            sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true).evaluated,
            1,
            "the input that moved was not on the pane's screen, so nothing else will bring it back",
        );
    }

    /// A wake that is not a SWEEP serves only what is late. Discovery and staleness wait for the
    /// interval, which is the whole meaning of the flag and is decided here rather than by the
    /// caller.
    #[test]
    fn a_wake_that_is_not_a_sweep_discovers_nothing() {
        let reg = registry_with(&[("a", claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        let due_only = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, false);
        assert_eq!(due_only.visited, 1, "it still walks — it just does not ask");
        assert_eq!(
            due_only.evaluated, 0,
            "an undiscovered pane is not LATE, and a deadline wake is for what is late",
        );
    }

    /// **NEITHER** tracker may outlive its pane, and the census that decides it is DAEMON-WIDE.
    ///
    /// The arc register listed this as deliberately uncovered, on the grounds that a leaked tracker
    /// is unobservable through any surface — true while the pass was a closure in a binary. The
    /// registry's own length is the observable, and reaching it only needed the pass to be callable.
    ///
    /// **The [`JobWatch`] half was added at R291 because a revert-proof found nothing to fail.**
    /// Deleting `jobs.retain_live(&live)` from the pass left the whole suite green: the watch's own
    /// unit test calls that method DIRECTLY, so it pins the method and says nothing about whether
    /// the pass ever calls it. Both censuses are asserted here, on one pane close, so they cannot
    /// come apart.
    #[test]
    fn a_pass_forgets_the_trackers_of_a_pane_that_is_gone() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(agents.with(|state| state.len()), 2);
        assert_eq!(jobs.len(), 2, "and the job watch has met both panes");

        // Session `b`'s pane closes. Its trackers are now the only thing that remembers it.
        {
            let pool = {
                let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
                guard.workspace_of("b").expect("session b")
            };
            let mut guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
            let gone = guard.panes()[0].id();
            let _closed = guard.close(gone);
        }

        sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(
            agents.with(|state| state.len()),
            1,
            "a census from one session would have forgotten the other session's panes too",
        );
        assert_eq!(
            jobs.len(),
            1,
            "and a job watch that kept the entry would compare a future pane's first reading \
             against a dead pane's job",
        );
    }

    /// Only the sessions whose published answer MOVED are woken, and the other one is not.
    ///
    /// The arc register's other deliberately-uncovered property, for the reason it gave — it needs a
    /// two-session harness, which needs a callable pass. The cost of getting it wrong is a redundant
    /// client re-read, which is cheap; the cost of never testing it is that the selectivity is
    /// decoration.
    #[test]
    fn only_the_session_whose_answer_moved_is_woken() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        // The first pass gives the claude pane a pending candidate; nothing is published yet,
        // because a verdict resting on an ABSENCE has to hold for the settle window.
        let first = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(first.moved, 0, "a candidate is not a publication");
        let (before_a, before_b) = (
            channels.revision("a").current(),
            channels.revision("b").current(),
        );

        // The window closes. Only `a` has anything to say.
        let settled = sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &no_word(),
            base + DEFAULT_SETTLE,
            true,
        );
        assert_eq!(settled.moved, 1);
        assert!(
            channels.revision("a").current() > before_a,
            "the session whose pane published is woken",
        );
        assert_eq!(
            channels.revision("b").current(),
            before_b,
            "and the one whose answer did not move is not — a shell pane no manifest claims \
             publishes nothing, so its clients have nothing to re-read",
        );
    }

    /// **THE slice-4 claim.** The verdict transition is the event the whole niche is about, and it
    /// is the one thing the dispatch funnel structurally cannot derive: it rests on the pane's
    /// SCREEN, which reaches the daemon through output, and on a clock nothing else in the daemon
    /// runs. So the observer that can see it emits it — and the record must name the very pane whose
    /// `seq` moved, which is the same condition the wake beside it already uses.
    #[test]
    fn the_settle_wakes_a_session_and_says_which_pane_moved() {
        let reg = registry_with(&[("a", claude_pane()), ("b", painting("$ "))]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        let cursor_a = channels.revision("a").current();
        let cursor_b = channels.revision("b").current();
        assert!(
            journal_events(&channels, "a", 0).is_empty(),
            "a candidate is not a publication, so it is not a record either",
        );

        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &no_word(),
            base + DEFAULT_SETTLE,
            true,
        );

        let recorded = journal_events(&channels, "a", cursor_a);
        assert_eq!(
            recorded.len(),
            1,
            "exactly the pane whose verdict moved: {recorded:?}",
        );
        assert!(
            matches!(recorded[0], Event::AgentStateChanged(_)),
            "and it is named as an agent transition: {recorded:?}",
        );
        assert!(
            journal_events(&channels, "b", cursor_b).is_empty(),
            "the session whose answer did not move records nothing, exactly as it is not woken",
        );
    }

    /// The record lands at the revision the WAKE carries, not one behind it.
    ///
    /// A client parks at `R`, is answered `R'` by the bump, and asks for `(R, R']`. A record keyed
    /// `R` would be invisible to that read and would never be offered again — the client's cursor
    /// has already passed it. This is what `ChannelRegistry::announce` holds the journal lock for.
    #[test]
    fn the_record_is_readable_from_the_cursor_the_wake_answers() {
        let reg = registry_with(&[("a", claude_pane())]);
        let agents = clock();
        let channels = ChannelRegistry::default();
        let jobs = JobWatch::new();
        let base = Instant::now();

        sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        // What a parked client would be holding: the revision before the publishing pass.
        let parked_at = channels.revision("a").current();

        sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &no_word(),
            base + DEFAULT_SETTLE,
            true,
        );

        // What the wake answers it with.
        let woken_at = channels.revision("a").current();
        assert!(woken_at > parked_at, "the settle advanced the scene");
        assert_eq!(
            journal_events(&channels, "a", parked_at).len(),
            1,
            "the record is inside the window the client asks for",
        );
        assert!(
            journal_events(&channels, "a", woken_at).is_empty(),
            "and is delivered once — a reader level with the wake has already accounted for it",
        );
    }

    /// **THE R291 claim, driven rather than simulated.** A real shell on a real pty is given a real
    /// job, and the pass that sees the foreground group move records it against that pane.
    ///
    /// The FIRST pass is the control and it is the half most easily got wrong: a pane nobody has
    /// sampled must ESTABLISH silently. Without that assertion this test passes just as well on a
    /// watch that announces every pane on every boot, which is the shape the bug takes.
    ///
    /// `bash -i` because job control is what is being observed: a non-interactive shell runs its
    /// commands in its OWN process group and never hands the terminal over, so the number would
    /// never move and the test would be measuring nothing.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_whose_foreground_job_changes_is_announced() {
        let reg = registry_with(&[("a", interactive_shell())]);
        let agents = clock();
        let jobs = JobWatch::new();
        let channels = ChannelRegistry::default();
        let base = Instant::now();
        let id = PaneId(0);

        // The shell at its prompt owns its own terminal. Waited for rather than assumed: job
        // control settles on the child's schedule.
        let at_rest =
            wait_for_pgid(&reg, "a", id, |pgid| pgid.is_some()).expect("the shell owns its tty");
        let cursor = channels.revision("a").current();

        let first = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(
            first.jobs_changed, 0,
            "CONTROL: nobody had sampled this pane, so its first reading establishes and is not news",
        );
        assert!(
            journal_events(&channels, "a", cursor).is_empty(),
            "and nothing was recorded for it",
        );

        // A job the user starts takes the terminal from the shell.
        write_to_pane(&reg, "a", id, b"sleep 300\n");
        let running = wait_for_pgid(&reg, "a", id, |pgid| {
            pgid.is_some_and(|pgid| pgid != at_rest)
        })
        .expect("a foreground job takes the terminal");
        assert_ne!(running, at_rest, "the job owns the terminal while it runs");

        let cursor = channels.revision("a").current();
        let second = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(
            second.jobs_changed, 1,
            "the pass that sees the group move is the pass that reports it",
        );

        let recorded = journal_events(&channels, "a", cursor);
        assert_eq!(
            recorded,
            vec![Event::PaneJobChanged(id.0)],
            "named by its SUBJECT — a reader answers it by re-reading pane_processes: {recorded:?}",
        );
    }

    /// **ONE PASS, ONE WAKE.** A session whose agent verdict AND whose foreground job both move in
    /// the same pass is woken ONCE, carrying both records.
    ///
    /// This is what the per-session accumulator holds `Vec<Event>` for. A second `announce` loop for
    /// the job half would bump this session's revision twice and send its clients round twice for a
    /// single observation — and every event would still be delivered, so nothing else in the tree
    /// would go red. The revision delta is the only observable that discriminates, which is why it
    /// is asserted as an exact number and not as "it moved".
    ///
    /// The shell's PROMPT is the `claude` footer the first built-in manifest claims, so the pane has
    /// a verdict that can settle while it also has a job that can change.
    #[cfg(target_os = "linux")]
    #[test]
    fn one_pass_wakes_a_session_once_however_many_facts_moved() {
        let reg = registry_with(&[("a", interactive_shell_painting_an_agent())]);
        let agents = AgentClock::new(Ruleset::new(built_ins()));
        let jobs = JobWatch::new();
        let channels = ChannelRegistry::default();
        let base = Instant::now();
        let id = PaneId(0);

        let at_rest =
            wait_for_pgid(&reg, "a", id, |pgid| pgid.is_some()).expect("the shell owns its tty");

        // Pass one: the job is established and the agent gains a candidate. Neither is a publication.
        let first = sweep_once(&reg, &agents, &jobs, &channels, &no_word(), base, true);
        assert_eq!(
            first.moved, 0,
            "a candidate is not a publication, and a first reading is not news"
        );

        write_to_pane(&reg, "a", id, b"sleep 300\n");
        wait_for_pgid(&reg, "a", id, |pgid| {
            pgid.is_some_and(|pgid| pgid != at_rest)
        })
        .expect("a foreground job takes the terminal");

        let before = channels.revision("a").current();
        let second = sweep_once(
            &reg,
            &agents,
            &jobs,
            &channels,
            &no_word(),
            base + DEFAULT_SETTLE,
            true,
        );
        assert_eq!(second.jobs_changed, 1, "the job moved");
        assert_eq!(second.moved, 1, "one session was woken");

        let recorded = journal_events(&channels, "a", before);
        assert!(
            recorded.contains(&Event::PaneJobChanged(id.0))
                && recorded.contains(&Event::AgentStateChanged(id.0)),
            "both facts moved in this pass, so both are on the record: {recorded:?}",
        );
        assert_eq!(
            channels.revision("a").current() - before,
            1,
            "and they arrived on ONE wake — two announces would bump twice for one observation",
        );
    }

    /// A `bash` that runs jobs: `-i` for job control, `--norc` so the box's dotfiles cannot change
    /// what it does, and a prompt with no escapes in it.
    #[cfg(target_os = "linux")]
    fn interactive_shell() -> CommandBuilder {
        let mut command = CommandBuilder::new("/bin/bash");
        command.arg("--norc");
        command.arg("-i");
        command.env("TERM", "dumb");
        command.env("PS1", "$ ");
        command
    }

    /// The same shell, prompting with the `claude` footer the first built-in manifest claims — so
    /// one pane can have both a verdict that settles and a job that changes.
    #[cfg(target_os = "linux")]
    fn interactive_shell_painting_an_agent() -> CommandBuilder {
        let mut command = interactive_shell();
        command.env("PS1", "  \u{23f8} manual mode on \u{b7} ? for shortcuts");
        command
    }

    /// Write bytes to a pane's child, reached the way the daemon reaches it.
    #[cfg(target_os = "linux")]
    fn write_to_pane(reg: &Arc<Mutex<SessionRegistry>>, session: &str, id: PaneId, bytes: &[u8]) {
        let pool = {
            let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
            guard.workspace_of(session).expect("the session")
        };
        let guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .pane(id)
            .expect("the pane")
            .pty()
            .write(bytes, sprag_terminal::Hand::AProgram)
            .expect("write to the pty");
    }

    /// Poll a pane's foreground job until `want` accepts it, or give up.
    ///
    /// Polled rather than slept on for `PanePty`'s own reason: job control settles on the child's
    /// schedule, so a fixed wait is either flaky or slow.
    #[cfg(target_os = "linux")]
    fn wait_for_pgid(
        reg: &Arc<Mutex<SessionRegistry>>,
        session: &str,
        id: PaneId,
        want: impl Fn(Option<u32>) -> bool,
    ) -> Option<u32> {
        let pool = {
            let guard = reg.lock().unwrap_or_else(PoisonError::into_inner);
            guard.workspace_of(session).expect("the session")
        };
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while Instant::now() < deadline {
            let pgid = {
                let guard = pool.lock().unwrap_or_else(PoisonError::into_inner);
                guard.pane(id).and_then(|pane| pane.pty().foreground_pgid())
            };
            if want(pgid) {
                return pgid;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        None
    }

    /// Everything `session`'s journal has recorded above `cursor`.
    fn journal_events(channels: &ChannelRegistry, session: &str, cursor: u64) -> Vec<Event> {
        channels.journal(session).since(cursor).events
    }
}
