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

use crate::plugins::{Driven, drive_request};
use crate::remote_access::{RemotePaneAccess, RemotePluginWorld};

/// The argv word that makes this binary a driver rather than a terminal.
///
/// ⚠ A flag rather than a subcommand, because the daemon's own argument parser is flag-shaped
/// (`--daemon`, `--size`) and a driver is the same kind of choice: which program this image is.
pub const DRIVE_FLAG: &str = "--drive";

/// How long a driver waits for its socket to accept before giving up.
///
/// ⚠ Short: the parent spawned this process and the daemon it names is the one that spawned it, so
/// a socket that is not there is a world that has already gone wrong rather than one still coming.
const CONNECT_WITHIN: Duration = Duration::from_secs(5);

/// **RUN THE DRIVER** — read the request from `stdin`, drive it, write the outcome to `stdout`.
///
/// `run` is the id the daemon registered this run under, which is what lets the orders below name a
/// row. `session` is the scope both connections take.
///
/// # Errors
///
/// A socket that will not accept, a request that is not a JSON object, or a request the builder
/// refuses (a word no plugin spells, a malformed argument, a pane this daemon does not hold).
/// **All of those happen before a byte is typed at anybody.**
pub fn drive(socket: &std::path::Path, session: Option<&str>, run: u64) -> std::io::Result<()> {
    let request = read_request()?;

    // ⚠⚠⚠ FOUR CONNECTIONS, AND EACH IS A DIFFERENT OUTSTANDING QUESTION. A `HostConn` matches
    // replies by id and carries ONE request at a time, so a park that is deliberately unanswered
    // would eat an ordinary read's reply (register item 631 measured that), and a subscription that
    // pushes unprompted must not land in the middle of somebody's call.
    let reading = connect(socket, session)?;
    let parking = connect(socket, session)?;
    let watching = connect(socket, session)?;
    // ⚠⚠⚠ A FOURTH, for the reason the three above are three: a `HostConn` carries ONE outstanding
    // request. Progress is pushed from the driving thread at every step, and the read connection is
    // BUSY at exactly those moments — it is what the step is reading the pane through. Sharing one
    // would make a report wait on a read (or worse, collect its reply).
    let reporting_on = Arc::new(Mutex::new(connect(socket, session)?));

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

    let world = RemotePluginWorld::over(&access);
    let driven = drive_request(
        &world,
        &request,
        &access,
        &context,
        reporting(reporting_on, run),
    )
    .map_err(|why| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{why:?}")))?;

    // ⚠⚠ THE WATCHER IS NOT JOINED. It is parked on a subscription that only the daemon can end,
    // and this process is about to exit — which closes the connection, which is how a subscription
    // is released (`EVENTS_UNSUBSCRIBE_METHOD`'s own doc says the disconnect arm does it). Waiting
    // for it would be waiting for a wake nobody is going to send.
    drop(orders);
    report(&driven)
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
fn reporting(conn: Arc<Mutex<HostConn>>, run: u64) -> sprag_plugin::ProgressSink {
    // ⚠ ONCE, not per step: a driver whose reports are all refused would otherwise fill the
    // daemon's log with one line per turn, which buries the first one — the only one that says
    // anything new.
    let said = Arc::new(AtomicBool::new(false));
    Arc::new(move |progress: &sprag_plugin::Progress| {
        let mut held = conn
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let sent = held.call(
            "scene/invoke",
            json!({
                "path": crate::plugins_path(crate::plugins::REPORT_PROGRESS_ACTION),
                "args": {
                    "id": run,
                    crate::plugins::PROGRESS_KEY: crate::plugins::progress_to_json(progress),
                },
            }),
        );
        if let Err(why) = sent
            && !said.swap(true, Ordering::Release)
        {
            eprintln!(
                "sprag-term --drive {run}: this daemon refused a progress report, so the run's row \
                 will not move while it works. Reported once; later refusals are silent. {why}"
            );
        }
    })
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
fn connect(socket: &std::path::Path, session: Option<&str>) -> std::io::Result<HostConn> {
    let mut conn = HostConn::connect(socket, CONNECT_WITHIN)?;
    if let Some(session) = session {
        conn.scope_to(session);
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
        if row
            .get(crate::plugins::RUN_CANCELLED_BY_KEY)
            .is_some_and(|who| !who.is_null())
        {
            cancel.store(true, Ordering::Release);
        }
        if row
            .get(crate::plugins::RUN_STOOD_DOWN_KEY)
            .and_then(Value::as_bool)
            == Some(true)
        {
            stand_down.store(true, Ordering::Release);
        }
        // ⚠ A LEVEL, NOT A LATCH — its two neighbours above are latches by design and this one is
        // the order a person can take back, so it is stored as read rather than only ever raised.
        if let Some(held) = row.get("held").and_then(Value::as_bool) {
            hold.store(held, Ordering::Release);
        }
    }
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
fn report(driven: &Driven) -> std::io::Result<()> {
    let mut answer = crate::plugins::outcome_to_json(&driven.outcome);
    // ⚠ CARRIED BESIDE THE OUTCOME rather than folded into it: the capture is what the plugin
    // produced, and the daemon's own worker reads it as a separate field for the same reason.
    if let Some(output) = &driven.output {
        answer["output"] = json!(output);
    }
    let mut out = std::io::stdout().lock();
    serde_json::to_writer(&mut out, &answer)?;
    writeln!(out)?;
    out.flush()
}
