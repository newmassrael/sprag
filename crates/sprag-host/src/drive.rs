//! **A RUN'S DRIVER, AS A PROCESS OF ITS OWN** — register items 544 and 643.
//!
//! # ⚠⚠⚠⚠⚠ What this exists to undo: two lifetimes in one process
//!
//! The daemon is a terminal multiplexer. It owns pseudoterminals, panes and windows, and its
//! natural lifetime is weeks. A RUN is a supervisor: it holds a statechart and drives a pane, and
//! its natural lifetime is the work — hours to days. Because the driver and its `.scxml` are
//! compiled into the daemon binary, the consequence is a sentence nobody would design on purpose:
//! **changing how an AI loop reflects requires restarting the thing that holds your PTYs.** On this
//! machine four watchers share one daemon, so promoting sprag's own build kills three other
//! repositories' loops — which is why register item 544 calls the fusion the root that items 526,
//! 285, 412 and 543 are all shadows of.
//!
//! Stage 1 of that item proved a driver CAN live out here: the shipped `Driver`, stepping the
//! shipped `Orchestrator`, over a `RemotePaneAccess` whose every answer comes off a socket, running
//! a pane to convergence and then outliving the daemon it drove. What it did not do is give that
//! capability a caller — item 643's whole content is *`RemotePaneAccess::over` has exactly one
//! caller and it is a test*. **This module is the caller.**
//!
//! # ⚠⚠⚠ Why it is a mode of `sprag-term` and not a binary of its own
//!
//! The driver must be the SAME BUILD as the document it compiles and the wire it speaks. A separate
//! artefact is a second thing to ship and a second version to skew (items 412 and 587 are what that
//! costs), while a mode of the daemon's own binary makes *the driver is this daemon's image* true by
//! construction: the parent spawns it from [`std::env::current_exe`].
//!
//! ⚠ It is deliberately NOT a `sprag` verb either. That vocabulary is what a PERSON types — each
//! entry declares its keystroke and its agent tool — and a process the daemon spawns is none of
//! those things.
//!
//! ⚠⚠ And it does not weaken what 544 wanted: a changed loop document rebuilds the binary, the
//! daemon keeps running with the image it started with, and the NEXT run spawns the new one.
//!
//! # The four channels, and why only one of them is a new wire address
//!
//! * **In** — the run's request arrives on STDIN as the same JSON object `run` takes. Not argv: a
//!   brief carries prose a person wrote, and quoting somebody's paragraph through a command line is
//!   a class of bug this avoids rather than handles.
//! * **Out** — the outcome leaves on STDOUT as JSON. The parent spawned this process, so the pipe
//!   is already theirs; publishing an address for it would be a wire change for a fact only one
//!   reader ever wants. ⚠ And it gives the fused design something it cannot express: **a driver
//!   that dies writes no JSON, and that silence is the honest outcome** rather than a run that
//!   simply stops being mentioned.
//! * **Orders** — a person's cancel, stand-down and hold are READ from this run's own row, and the
//!   driver is WOKEN by `events/subscribe` (register item 648 put `run_ordered` on that journal for
//!   exactly this). ⚠⚠⚠ A subscription rather than `events/waitFor`: that method's own doc says its
//!   reply is a `FnOnce`, so following costs **a round trip per change**. Items 629/630/631/640
//!   spent four rounds taking clock-paced waiting off the pane axis, ending at *a remote wait that
//!   cost 181 reads over two seconds now costs 1*; a driver that polled its own orders would put it
//!   straight back one axis over.
//! * **Progress** — and this one IS a new address ([`REPORT_PROGRESS_ACTION`], register item 650),
//!   because it is the one fact whose reader is neither this process nor the parent's pipe: it is
//!   whoever is watching the run's ROW while the work is still going. The daemon reads a running
//!   run's counters out of a `ProgressCell` it SHARES with an in-process worker, and a driver out
//!   here shares no memory with it — so without an address such a run's row sits at zero for its
//!   whole life, which is the difference between a supervised loop and a black box.
//!   ⚠ **PUSHED, not sampled**, for the reason the orders channel above is a subscription.
//!
//! [`REPORT_PROGRESS_ACTION`]: crate::plugins::REPORT_PROGRESS_ACTION

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use sprag_rpc::HostConn;

use crate::external::lock;
use crate::plugins::{Driven, drive_request};
use crate::remote_access::{RemotePaneAccess, RemotePluginWorld};

/// The argv word that makes this binary a driver rather than a terminal.
///
/// ⚠ A flag rather than a subcommand, because the daemon's own argument parser is flag-shaped
/// (`--daemon`, `--size`) and a driver is the same kind of choice: which program this image is.
pub const DRIVE_FLAG: &str = "--drive";

/// The argv word that tells a driver WHICH WINDOW of its scoped session it is driving in —
/// register item 690.
///
/// ⚠ Spelled here beside [`DRIVE_FLAG`] rather than at the two ends that use it, because the
/// daemon writes this argv and this binary reads it: a flag spelled twice is a flag one side can
/// rename. `-w` follows the session flag's `-t` shape, which is the vocabulary a person already
/// reads in `sprag -t SESSION`.
pub const DRIVE_WINDOW_FLAG: &str = "-w";

/// How long a driver waits for its socket to accept before giving up.
///
/// ⚠ Short: the parent spawned this process and the daemon it names is the one that spawned it, so
/// a socket that is not there is a world that has already gone wrong rather than one still coming.
const CONNECT_WITHIN: Duration = Duration::from_secs(5);

/// **RUN THE DRIVER** — read the request from `stdin`, drive it, write the outcome to `stdout`.
///
/// `run` is the id the daemon registered this run under, which is what lets the orders below name a
/// row. `session` is the scope every connection takes.
///
/// # ⚠⚠⚠⚠⚠ `window` is the OTHER half of the address, and it was missing — register item 690
///
/// A driver reads one pane and types at one pane for its whole life, and until this argument
/// existed it never said WHICH WINDOW that pane is in. The daemon answers a pane id against ONE
/// window's pane pool (`plugins::require_pane_in`), so a request naming no window is read against
/// whatever window the session is CURRENTLY showing — and a person or another agent selecting a
/// different window moved that under a running driver's feet.
///
/// Measured, not reasoned: run 23 died `failed at reflecting: there is no pane 23` while pane 23
/// was `idle seq=12` and the session's current window was `pinion`.
///
/// [`None`] keeps the old shape exactly — a driver spawned by a daemon that does not name a window
/// is one whose requests follow the session, which is what every driver did before this.
///
/// # Errors
///
/// A socket that will not accept, a request that is not a JSON object, or a request the builder
/// refuses (a word no plugin spells, a malformed argument, a pane this daemon does not hold).
/// **All of those happen before a byte is typed at anybody.**
pub fn drive(
    socket: &std::path::Path,
    session: Option<&str>,
    window: Option<&str>,
    run: u64,
) -> std::io::Result<()> {
    let request = read_request()?;

    // ⚠⚠⚠ FOUR CONNECTIONS, AND EACH IS A DIFFERENT OUTSTANDING QUESTION. A `HostConn` matches
    // replies by id and carries ONE request at a time, so a park that is deliberately unanswered
    // would eat an ordinary read's reply (register item 631 measured that), and a subscription that
    // pushes unprompted must not land in the middle of somebody's call.
    let reading = connect(socket, session, window)?;
    let parking = connect(socket, session, window)?;
    let watching = connect(socket, session, window)?;
    // ⚠⚠⚠ A FOURTH, for the reason the three above are three: a `HostConn` carries ONE outstanding
    // request. Progress is pushed from the driving thread at every step, and the read connection is
    // BUSY at exactly those moments — it is what the step is reading the pane through. Sharing one
    // would make a report wait on a read (or worse, collect its reply).
    let reporting_on = Arc::new(Mutex::new(connect(socket, session, window)?));

    let access = RemotePaneAccess::over(reading)
        .parking_on(parking)
        .map_err(|mis| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{mis}")))?;

    // The three flags a run reads, raised by the watcher below rather than by a registry in this
    // process — see `watch_orders`.
    let cancel = Arc::new(AtomicBool::new(false));
    let stand_down = Arc::new(AtomicBool::new(false));
    let hold = Arc::new(AtomicBool::new(false));
    let context = sprag_plugin::RunContext::new(Arc::clone(&cancel))
        .ordered_by(Arc::clone(&stand_down))
        .held_by(Arc::clone(&hold));

    let orders = {
        let (cancel, stand_down, hold) = (
            Arc::clone(&cancel),
            Arc::clone(&stand_down),
            Arc::clone(&hold),
        );
        std::thread::spawn(move || watch_orders(watching, run, &cancel, &stand_down, &hold))
    };

    // ⛔⛔⛔⛔⛔ **WHERE A DAEMON'S REFUSAL OF THIS RUN LANDS** — register item 764. The reporting
    // sink is the one channel that asks *are you still driving me* on every step, and until this
    // existed its answer was dropped: a driver whose run had been set aside by a successor daemon
    // was told so and typed on. It stops (the cancel below) and its ENDING carries the daemon's own
    // sentence, which is the difference between register item 685's silence and a reason.
    let abandoned: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    let world = RemotePluginWorld::over(&access);
    let driven = drive_request(
        &world,
        &request,
        &access,
        &context,
        reporting(
            reporting_on,
            run,
            Arc::clone(&abandoned),
            Arc::clone(&cancel),
        ),
    )
    .map_err(|why| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{why:?}")))?;

    // ⚠⚠ THE WATCHER IS NOT JOINED. It is parked on a subscription that only the daemon can end,
    // and this process is about to exit — which closes the connection, which is how a subscription
    // is released (`EVENTS_UNSUBSCRIBE_METHOD`'s own doc says the disconnect arm does it). Waiting
    // for it would be waiting for a wake nobody is going to send.
    drop(orders);
    // ⛔⛔⛔⛔⛔ **AND IF THE DAEMON UNDER THIS RUN WAS REPLACED, ASK THE ONE THAT IS THERE NOW** —
    // register item 777. The refusal register item 764 built reaches a driver only over a live
    // connection, and a promotion leaves this process holding four dead ones; what survives is the
    // read surface, which redials and judges the daemon's identity for itself. So the reason comes
    // from the SUCCESSOR's row rather than from a refusal that cannot arrive.
    let why = lock(&abandoned)
        .clone()
        .or_else(|| replaced_under_this_run(&access, run));
    report(&driven, why.as_deref())
}

/// **WHAT THE DAEMON THAT IS THERE NOW SAYS ABOUT THIS RUN**, or [`None`] while the world under
/// this driver is the one it adopted — register item 777.
///
/// # ⚠⚠⚠⚠⚠ Why a leftover driver has to ask at all, when it is already stopping
///
/// It stops on its own: every read answers `None` once
/// [`RemotePaneAccess::world_changed`] latches, and the write door refuses in words — **measured at
/// 192.9 µs, 0 iterations**, so *within one step* was true before this existed. What was NOT true
/// is that its ending said anything a person could act on. The write door's sentence names THIS
/// driver's problem (*the daemon behind this connection was replaced*) and cannot name the one that
/// matters: **what the successor decided about the run itself** — item 737's *the documents moved,
/// start a new run*, item 771's *the pane it was on did not come back*, or simply *there is no such
/// run here*. Those are the remedies, and only the daemon holding the socket now knows which.
///
/// # ⚠⚠⚠⚠ It READS, and never reports — the safety argument this round was built on
///
/// The obvious repair is to redial the reporting connection and let register item 764's refusal
/// come back. **It is unsafe**, and the refutation is this repository's own: run ids are seeded
/// from the log (`RunRegistry::restore`), so a successor with no log to read mints them FROM ZERO —
/// and a progress report sent blind would land on a stranger's row, which is the pane-id hazard
/// [`RemotePaneAccess::world_changed`] exists for, one axis over. So nothing is sent. The question
/// is asked through [`RemotePaneAccess::through_the_latch`], which is read-only and rides the one
/// connection that has already redialled and already compared identities.
///
/// ⚠⚠ **THE SUCCESSOR'S OWN WORDS, CARRIED AND NEVER RE-AUTHORED.** `RUN_WITHHELD_KEY`,
/// `RUN_NOT_RESUMED_KEY` and `RUN_LEFTOVER_KEY` are sentences that daemon composed for the row a
/// person opens (`crate::plugins::withheld_sentence` and its siblings), so a driver that wrote its
/// own account would be a second mouth on a decision it did not take.
///
/// ⚠ A row with no such clause still answers: *what became of it* is the run's own status word, and
/// **an absent row is an answer too** — this driver's id names nothing there, which is the one
/// thing a person reading a stopped loop most needs to know.
///
/// # ⚠⚠ `pub` so a gate can stage two daemons on one socket, and it has to be
///
/// The end-to-end path CANNOT be observed: a leftover driver writes its ending to the stdout pipe
/// of the daemon that spawned it, and that daemon is dead by definition here (`driver_spawn` gives
/// the child `Stdio::piped()` for both streams). So the only place this behaviour can be measured
/// is the seam, and the seam has to be reachable from the suite that already knows how to put a
/// second daemon on one socket path. That absence of a reader is the RESIDUE of register item 777,
/// stated rather than hidden: the ending is composed correctly and nobody is there to read it —
/// which is the trade register item 764's own entry accepted in as many words (*"the answer goes to
/// a dead pipe, but the ending is a REASON rather than a silence"*).
#[must_use]
pub fn replaced_under_this_run(access: &RemotePaneAccess, run: u64) -> Option<String> {
    if !access.world_changed() {
        return None;
    }
    let listing = access.through_the_latch(&crate::plugins_path(crate::plugins::RUNS_SLOT));
    let row = listing
        .as_ref()
        .and_then(|value| value.as_array())
        .and_then(|rows| {
            rows.iter()
                .find(|entry| entry.get("id").and_then(Value::as_u64) == Some(run))
        });
    let Some(row) = row else {
        // ⚠⚠ TWO ABSENCES, TWO SENTENCES — register item 778's rule at this door. A daemon that
        // ANSWERED and holds no such run is a fact a person can act on; a daemon that would not
        // answer at all is not, and saying the first about the second would be this round's own
        // defect arriving one layer up.
        return Some(match listing {
            Some(_) => format!(
                "the daemon holding this socket was replaced while this run was working, and the \
                 one there now holds no run {run} — nothing is going to collect this run's answer, \
                 so it stopped here rather than typing on"
            ),
            None => format!(
                "the daemon holding this socket was replaced while run {run} was working, and the \
                 one there now would not say what became of it — this driver stopped rather than \
                 typing into a world it cannot read"
            ),
        });
    };
    // ⚠ THE ROW'S OWN CLAUSES, in the order a reader wants them: the decision first, because that
    // is what says whether to start the loop again, and the status word last as the fallback for a
    // successor that recorded no decision.
    let said = [
        crate::plugins::RUN_WITHHELD_KEY,
        crate::plugins::RUN_NOT_RESUMED_KEY,
        crate::plugins::RUN_LEFTOVER_KEY,
    ]
    .into_iter()
    .filter_map(|key| row.get(key).and_then(Value::as_str))
    .collect::<Vec<_>>()
    .join("; ");
    let status = row
        .get("state")
        .and_then(|state| state.get("status"))
        .and_then(Value::as_str)
        .unwrap_or("unreadable");
    Some(if said.is_empty() {
        format!(
            "the daemon holding this socket was replaced while this run was working, and the one \
             there now holds run {run} as {status:?} with no reason recorded — this driver stopped \
             rather than typing into a world it no longer belongs to"
        )
    } else {
        format!(
            "the daemon holding this socket was replaced while this run was working, and the one \
             there now says of run {run} ({status}): {said}"
        )
    })
}

/// **WHERE THIS DRIVER'S PROGRESS GOES** — a call that puts it on the wire, for
/// [`drive_request`]'s `report` argument.
///
/// # ⚠⚠⚠⚠⚠ It sends what the DAEMON's renderer produces
///
/// [`progress_to_json`](crate::plugins::progress_to_json) is called here, not a shape spelled over
/// here — the rule [`report`] already follows for the outcome. So the daemon stores the object
/// without reading it apart, and a key that renderer grows reaches the row with nothing in either
/// process to update.
///
/// # ⚠⚠ A failed report is DROPPED — but never SILENTLY
///
/// Progress is a level: the next publish carries the whole of it, so a call that could not be made
/// costs a watcher the freshness of one step and nothing else. Failing the RUN because a status
/// update did not land would let a reporting problem end work that is going fine — and the run's
/// real answer travels on stdout, which this is not.
///
/// ⚠⚠⚠⚠⚠ **THE FIRST FORM OF THIS DROPPED THE ERROR TOO, AND THAT COST A ROUND.** Every report of a
/// three-turn run failed, the row never moved, and the gate could not tell *the driver is not
/// sending* from *the daemon is not storing* — because the only party who knew said nothing. A
/// swallowed error is register item 492's shape wearing a different coat: something happened and
/// nobody can read it. So a refusal is written to stderr ONCE, which is what `watch_orders` beside
/// this already does for the same reason and is all a driver can do about it.
///
/// # ⛔⛔⛔⛔⛔ Except for ONE refusal, which is not a reporting problem at all — item 764
///
/// *Nothing here is driving your run* is a fact about the RUN and not about this call, and the
/// paragraph above does not reach it: dropping it leaves a driver typing at somebody's pane on
/// behalf of a run that no daemon will ever collect, with its answer bound for a pipe that closed
/// with the daemon that spawned it. [`carry_refusal_in`] is what tells the two apart.
fn reporting(
    conn: Arc<Mutex<HostConn>>,
    run: u64,
    abandoned: Arc<Mutex<Option<String>>>,
    cancel: Arc<AtomicBool>,
) -> sprag_plugin::ProgressSink {
    // ⚠ ONCE, not per step: a driver whose reports are all refused would otherwise fill the
    // daemon's log with one line per turn, which buries the first one — the only one that says
    // anything new.
    let said = Arc::new(AtomicBool::new(false));
    Arc::new(move |progress: &sprag_plugin::Progress| {
        let mut held = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // ⚠⚠ `try_call` and not `call`: this caller has to ACT on which failure it was, and that
        // method's own doc names exactly this case — recovering a code from a rendered sentence
        // means one crate matching on another's wording.
        let sent = held.try_call(
            "scene/invoke",
            json!({
                "path": crate::plugins_path(crate::plugins::REPORT_PROGRESS_ACTION),
                "args": {
                    "id": run,
                    crate::plugins::PROGRESS_KEY: crate::plugins::progress_to_json(progress),
                },
            }),
        );
        let Err(why) = sent else { return };
        // ⛔⛔⛔ THE RUN IS ENDED BEFORE THE LINE IS PRINTED, because this arm is not a degradation
        // to be reported and lived with — it is the run being over.
        let ending = carry_refusal_in(&why, &abandoned, &cancel);
        // ⚠ THE WIRE'S OWN RENDERING and not a second one spelled here — `From<CallError> for
        // io::Error` exists so a caller that opted into the typed error still prints what
        // `HostConn::call` would have printed.
        let rendered = std::io::Error::from(why);
        if ending {
            eprintln!(
                "sprag-term --drive {run}: this daemon will not take this run's progress, so the \
                 run is ending here rather than typing on. {rendered}"
            );
            return;
        }
        if !said.swap(true, Ordering::Release) {
            eprintln!(
                "sprag-term --drive {run}: this daemon refused a progress report, so the run's row \
                 will not move while it works. Reported once; later refusals are silent. {rendered}"
            );
        }
    })
}

/// **TURN ONE REFUSED PROGRESS REPORT INTO THE END OF THIS RUN** — register item 764, and
/// [`carry_orders_in`]'s shape one channel over.
///
/// # ⚠⚠⚠⚠⚠ Why this is a function and not four lines inside the sink above
///
/// [`carry_orders_in`]'s own doc holds the argument and it was earned rather than reasoned: a gate
/// on the TYPE came back green against the shipped defect, because the gate called the type and the
/// loop called the socket. The step between is what has to be nameable. So the daemon's clause is
/// recognised HERE, by [`crate::runs::Unreported::spoken_in`] — the far side of the very
/// [`describe`](crate::runs::Unreported::describe) that composed it — and a gate can drive one into
/// the other with no socket in the way.
///
/// # ⚠⚠⚠ What it must NOT fire on, which is most of what can go wrong here
///
/// * **A transport failure.** The daemon is unreachable, which says nothing about whether the run
///   is still somebody's — and a driver that ended on a socket hiccup would throw away work over a
///   fact it never established.
/// * **Any other refusal.** An older daemon that does not serve the address, a malformed argument,
///   a scope it will not answer: all of those are this CALL failing, which is the paragraph
///   [`reporting`] already answers by carrying on.
///
/// ⚠⚠ Answering `true` does two things and they are one decision: the reason is kept for the ENDING
/// (register item 685 — *silence is an outcome and it is not converged*), and the run's cancel is
/// raised, which is the only channel a sink has into a plugin that is mid-turn. The flag is the one
/// a person's cancel arrives on, so nothing new has to be honoured for this to take effect.
pub(crate) fn carry_refusal_in(
    failure: &sprag_rpc::CallError,
    abandoned: &Mutex<Option<String>>,
    cancel: &AtomicBool,
) -> bool {
    let sprag_rpc::CallError::Fault(fault) = failure else {
        return false;
    };
    let Some(clause) = fault
        .refusal()
        .filter(|clause| crate::runs::Unreported::spoken_in(clause))
    else {
        return false;
    };
    *lock(abandoned) = Some(clause.to_owned());
    cancel.store(true, Ordering::Release);
    true
}

/// One scoped connection to `socket`, **through the door every other client of this daemon passes
/// through**.
///
/// # ⚠⚠⚠⚠⚠ Why a driver of all clients must agree on the wire's shape before it types
///
/// [`HostConn::handshake`] refuses a daemon whose [`sprag_rpc::WIRE_PROTOCOL`] is not this build's,
/// and until register item 653 this was the ONE client that skipped it — the CLI, both frontends,
/// `sprag-mcp` and `sprag-peer` all announce and agree. That was survivable while every address a
/// driver reads degrades safely when it is missing: it would discover a skew slot by slot and each
/// discovery would be an absence.
///
/// **`hands` broke that.** A daemon too old to publish it does not leave a driver knowing less; it
/// leaves the driver believing *nobody has ever reached into this pane* and typing over whoever is
/// there. An address whose absence is a FALSE answer cannot be discovered at the read, so the
/// agreement has to happen at the door — which is what the number is for and why item 653 moved it.
///
/// ⚠⚠ **THE SPAWNING DAEMON IS NORMALLY THE SAME IMAGE** ([`std::env::current_exe`], this module's
/// header), so this refuses nothing on the ordinary path. What it catches is the path this
/// repository has already paid for: a PROMOTION replacing the binary under a running daemon
/// (register item 344), after which the next run's driver is a newer build than the daemon it was
/// spawned by.
///
/// ⚠ The id names this process, not this run: a run's four connections are one logical client, and
/// [`sprag_rpc::CLIENT_HELLO_METHOD`] groups them by exactly that.
fn connect(
    socket: &std::path::Path,
    session: Option<&str>,
    window: Option<&str>,
) -> std::io::Result<HostConn> {
    let mut conn = HostConn::connect(socket, CONNECT_WITHIN)?;
    if let Some(session) = session {
        conn.scope_to(session);
    }
    // ⚠⚠⚠⚠⚠ AND THE WINDOW, on the CONNECTION rather than at each of the fifteen places that build
    // params — register item 690, and [`sprag_rpc::HostConn::view_window`] carries the argument.
    // Every request this driver will ever make is about one pane in one window, so the narrowing
    // belongs to the connection and cannot be forgotten by a call that does not exist yet.
    if let Some(window) = window {
        conn.view_window(window);
    }
    conn.handshake(&format!("drive-{}", std::process::id()))?;
    Ok(conn)
}

/// The request object, read whole from `stdin`.
///
/// ⚠ WHOLE, then parsed: a JSON object arrives as one value and a driver that read line by line
/// would be inventing a framing its writer never used.
fn read_request() -> std::io::Result<Map<String, Value>> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw)?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(other) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("a run request is a JSON object, not {other}"),
        )),
        Err(why) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("the run request on stdin is not JSON: {why}"),
        )),
    }
}

/// **FOLLOW THIS RUN'S ORDERS UNTIL THE CONNECTION ENDS** — register item 648's other half.
///
/// # ⚠⚠⚠⚠⚠ Woken, never polled
///
/// The daemon announces `run_ordered` when a person's cancel, stand-down or hold is ACCEPTED (and
/// never when it is refused). This subscribes to that journal and re-reads the run's own row on
/// each batch, so an order costs one notification and one read — no clock anywhere.
///
/// ⚠⚠ **THE ROW IS RE-READ RATHER THAN THE EVENT BEING BELIEVED.** The event names its subject and
/// carries no value, which is this journal's rule for every kind it has: a reader turns it into a
/// targeted re-read. That is what lets three orders share one event, and it is why a hold being
/// TAKEN BACK arrives here as an ordinary batch rather than needing a word of its own.
fn watch_orders(
    mut conn: HostConn,
    run: u64,
    cancel: &AtomicBool,
    stand_down: &AtomicBool,
    hold: &AtomicBool,
) {
    let Ok(opened) = conn.call(
        sprag_rpc::EVENTS_SUBSCRIBE_METHOD,
        json!({ sprag_rpc::SINCE_PARAM: 0 }),
    ) else {
        // ⚠⚠⚠ A DAEMON THAT WILL NOT SUBSCRIBE LEAVES THE RUN UNORDERABLE, and saying so on stderr
        // is the whole of what this thread can do: the flags stay false, the run drives on, and a
        // person's cancel will not reach it. That is a DEGRADATION and it is loud, where a silent
        // one would have somebody typing `sprag cancel-run` at a run that cannot hear.
        eprintln!(
            "sprag-term --drive: this daemon refused a subscription, so run {run} cannot be cancelled"
        );
        return;
    };
    let _ = opened;
    loop {
        // The first read also drains anything the subscribe reply raced.
        if conn
            .next_notification(sprag_rpc::EVENTS_CHANGED_METHOD)
            .is_err()
        {
            return;
        }
        let Some(row) = read_row(&mut conn, run) else {
            continue;
        };
        carry_orders_in(&row, cancel, stand_down, hold);
    }
}

/// **TURN ONE ROW INTO THE THREE FLAGS THIS DRIVER STEERS BY** — register item 699.
///
/// # ⚠⚠⚠⚠⚠ Why this is a function and not four lines inside the loop above
///
/// It was four lines inside the loop, and **two of the three had never once been true**:
/// `stand_down` was read as `row[RUN_STOOD_DOWN_KEY].as_bool()` off a key the daemon fills with a
/// SENTENCE (`None == Some(true)`, false for ever), and `held` was read off `row["held"]`, a key no
/// projection has ever written. Nine stand-downs across four repositories, zero convergences.
///
/// The repair put a shared type on the wire ([`crate::plugins::StandingOrders`]), and **a first
/// gate on that type came back GREEN when this reader was mutated back to the shipped defect** —
/// because the gate called the type and the loop above called the socket, so nothing measured the
/// step between. A green mutation is the name of the gate you owe: the step is named here, so a
/// gate can drive the row the daemon really produced through the reader this driver really uses,
/// without a subscription in the way.
///
/// ⚠⚠ THE CANCEL ARM TRAVELS WITH THEM ON PURPOSE. It is the one order that always worked — it
/// asks *is this non-null*, never `as_bool`, and the projection really does write its key — so it
/// is this function's own control: a change that breaks the two below while leaving it alone is a
/// change this file's gate can still tell apart.
pub(crate) fn carry_orders_in(
    row: &serde_json::Map<String, Value>,
    cancel: &AtomicBool,
    stand_down: &AtomicBool,
    hold: &AtomicBool,
) {
    if row
        .get(crate::plugins::RUN_CANCELLED_BY_KEY)
        .is_some_and(|who| !who.is_null())
    {
        cancel.store(true, Ordering::Release);
    }
    // ⚠⚠ `reporting` below states the rule this follows, for the other direction: the shape belongs
    // to the party that OWNS it and the other end must not respell it. Both ends are in this crate,
    // so the type puts a compiler between them.
    let ordered = crate::plugins::StandingOrders::in_row(row);
    // ⚠ A LATCH — `stand_down` has no way back, so it is only ever raised.
    if ordered.stand_down {
        stand_down.store(true, Ordering::Release);
    }
    // ⚠ A LEVEL, NOT A LATCH — the order a person can take back, so it is stored as read.
    hold.store(ordered.held, Ordering::Release);
}

/// This run's row from the daemon's `runs` listing, or [`None`] where it is not there to read.
///
/// # ⛔⛔⛔⛔⛔ The surface is the PLUGIN HOST's, not the multiplexer's — register item 660
///
/// [`RUNS_SLOT`](crate::plugins::RUNS_SLOT) is served by `PluginsExternal` and reached at
/// [`plugins_path`](crate::plugins_path). This asked the MUX for it, and a mux that does not serve
/// that name answers nothing — so **every** read here returned [`None`], for every run, for the
/// whole life of the out-of-process driver. What that cost is the whole of register item 648's
/// remaining half: the order reached the journal, the subscription woke this watcher, and the
/// watcher then failed to read the row it had been woken to re-read — so a person's cancel,
/// stand-down and hold never reached a run driven from another process, silently, because
/// `read_row`'s `None` is indistinguishable here from *the run is not listed yet*.
///
/// ⚠⚠ Measured rather than reasoned: with the watcher instrumented, one `subscribed`, one `woken`,
/// and one `read_row=None` — the wake was arriving and the read was the thing that failed. ⚠ A
/// dedicated second connection for this read was tried first, on the theory that a call was
/// colliding with the park, and it changed nothing; that hypothesis is refuted and the path was the
/// cause.
fn read_row(conn: &mut HostConn, run: u64) -> Option<Map<String, Value>> {
    let listing = conn
        .call(
            "scene/query",
            json!({ "path": crate::plugins_path(crate::plugins::RUNS_SLOT) }),
        )
        .ok()?;
    listing
        .as_array()?
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_u64) == Some(run))?
        .as_object()
        .cloned()
}

/// Write the outcome to `stdout` as the object the parent reaps.
///
/// ⚠⚠⚠ **THE DAEMON'S OWN RENDERER**, never a shape spelled here.
/// [`outcome_to_json`](crate::plugins::outcome_to_json) is `pub` for exactly this reason and says
/// so: a second mouth writing `{"state": …}` itself is the two-readers defect that drifts first in
/// whichever key one of them forgot. What the parent reaps is byte-identical to what a client reads
/// off the run row.
///
/// ⚠⚠ FLUSHED, and the flush is checked: a driver whose last act is buffered is a run whose ending
/// the parent reads as *the process died without saying anything*, which is a real and different
/// outcome this must not manufacture.
///
/// # ⛔⛔⛔⛔⛔ `abandoned` is why this ending is a REASON and not a bare `cancelled` — item 764
///
/// A driver that [`carry_refusal_in`] stopped ends with the outcome its plugin produced under a
/// raised cancel, and `cancelled` alone is register item 596's collapse arriving by a third road:
/// a person's stop, a daemon's shutdown and *the daemon holding the socket has set your run aside*
/// would all read the same. The daemon's own clause rides beside it under
/// [`RUN_ABANDONED_KEY`](crate::plugins::RUN_ABANDONED_KEY).
///
/// ⚠ [`None`] is the ordinary run and writes NOTHING, on the row's own rule for every added answer
/// key: the key's presence is the claim, so a run that ended on its own terms must not carry an
/// empty one.
fn report(driven: &Driven, abandoned: Option<&str>) -> std::io::Result<()> {
    let answer = ending(driven, abandoned);
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &answer)?;
    writeln!(out)?;
    out.flush()
}

/// **THE OBJECT [`report`] WRITES**, as a value — [`carry_orders_in`]'s rule applied to this end:
/// the step between the fact and the pipe is named, so a gate can read what a parent would reap
/// without a process in the way.
pub(crate) fn ending(driven: &Driven, abandoned: Option<&str>) -> Value {
    let mut answer = crate::plugins::outcome_to_json(&driven.outcome);
    // ⚠ CARRIED BESIDE THE OUTCOME rather than folded into it: the capture is what the plugin
    // produced, and the daemon's own worker reads it as a separate field for the same reason.
    if let Some(output) = &driven.output {
        answer["output"] = json!(output);
    }
    if let Some(why) = abandoned {
        answer[crate::plugins::RUN_ABANDONED_KEY] = json!(why);
    }
    answer
}
