//! **THE PANE SURFACE THIS DAEMON SERVES, READ AND DRIVEN FROM OUTSIDE ITS PROCESS** — register
//! item 544, stage 1.
//!
//! [`RemotePaneAccess`] implements [`PaneAccess`] over a socket. Every method reads or invokes a
//! PUBLISHED address; nothing here touches a workspace, a lock or a pseudoterminal, so the party
//! holding it need not be the party holding the panes.
//!
//! # ⚠⚠⚠⚠⚠ Why this type exists: two lifetimes were sharing one process
//!
//! A terminal multiplexer's natural lifetime is WEEKS — it owns pseudoterminals, panes and windows.
//! A run's is the WORK, hours to days. The driver and its statechart are compiled into the daemon
//! binary, so *"change how a loop reflects"* has meant *"restart the thing that holds your PTYs"*,
//! and a run that outlives nothing is a run that cannot be resumed either.
//!
//! ⚠ It is the one thing in this workspace that drove the daemon from INSIDE. `sprag-mcp` has
//! driven it from another process for rounds; the wire is versioned and swept; the acts a driver
//! needs are already verbs; panes are addressable across windows. What was missing was a
//! [`PaneAccess`] whose answers come off that wire, and — until stage 1a and 1b — three of the six
//! reads it makes had no address at all.
//!
//! # What a remote surface answers, and what it still does not
//!
//! **SIX ARE SERVED** (register item 557), and each one is a production path of the loop's:
//!
//! * `supervision` — what the agent in a pane is doing. Five reads in `outer.rs`.
//! * `lifecycle` — open, REPLACE and close panes. The loop's whole session rollover.
//! * `input_echo` — what was written INTO a pane. `ReadyWhen::Prints`' refusal.
//! * `terminal_modes` — who echoes, and whether a `Ctrl-D` ends the input. `deliver`'s verdict.
//! * `foreground_job` — who owns the pane's terminal. TWO consumers: the predicate
//!   `ReadyWhen::Runs` decides on, and the diagnosis a barrier that never cleared owes its caller.
//! * `output_lines` — what the pane has said SINCE a cursor, with what was evicted unread. The read
//!   a relay is; no whole-output address can be one.
//!
//! **THAT IS EVERY SURFACE THE LOOP READS ON A PRODUCTION PATH.**
//!
//! * `hands` — WHO has written into the pane, and how many times each. Register item 653, and it is
//!   served for a reason none of the six above share: see below.
//!
//! ⚠⚠⚠⚠ **Two remain [`None`]**: `raw_capture` and `job_control`. Each of those absences is safe by
//! that surface's OWN documentation — a host with no job control must report that it could not stop
//! the work rather than write `0x03` and hope. **They are `None` because this build cannot ask those
//! questions over the wire yet, not because a remote driver does not want them**, and each has a
//! consumer that already handles the absence.
//!
//! ⚠⚠⚠⚠⚠ **AND THE SENTENCE ABOVE USED TO COVER `hands`, WHICH WAS FALSE — register item 653.** It
//! said all three absences were safe. The other two are; this one was not, and the difference is
//! WHERE the read sits. [`sprag_plugin::Readiness::reached`] asks *has a person reached into this
//! pane* **first, ahead of the barrier and ahead of any consent**, and it asks it through
//! [`PaneAccess::hands`]. A `None` there is not *I could not look* — the consumer has nowhere to
//! put that — it is *nobody has reached in*. So a run driven from another process was told, for
//! every pane and for its whole life, that the pane was unattended, and typed over whoever was
//! there. **Measured against a real daemon before the address existed**, with the person's write
//! declared and visibly on the screen. It is the one address on this surface whose absence moved
//! [`sprag_rpc::WIRE_PROTOCOL`], and the one reason [`crate::drive`] now handshakes.
//!
//! ⚠⚠⚠⚠⚠ **The echo trail is the one read here that is NOT about the screen, and that is the point.**
//! A pseudoterminal echoes its input, so a barrier matching a marker against the grid cannot tell
//! the program saying it from the driver's own keystroke coming back. Until item 557 a remote driver
//! read that trail as EMPTY — so the refusal that makes `ReadyWhen::Prints` deterministic was
//! silently not running, and the same call converged or fed the shell depending on scheduling.
//!
//! ⚠⚠⚠⚠⚠ **The two absences supervision keeps apart are the reason it needed TWO addresses.**
//! [`PaneAccess::supervision`] answering `None` means *ask a person, this build cannot look*;
//! [`PaneSupervision::pane_agent_state`] answering `None` means *this pane is not an agent*. A
//! surface publishing one word for both lets a supervisor conclude "no agents here" from a host that
//! never looked.
//!
//! ⚠⚠ The one that is NOT merely absent is the PAINT question. [`PaneRow::generation`] is a damage
//! generation, deliberately unpublished (a resize or a palette change stamps every row while no
//! program writes a byte, which is a mistake four plugins in this workspace have already made). So
//! the rows this surface serves carry the TEXT — the content question, which is what
//! [`RowTrail`](sprag_plugin::RowTrail) asks — and a `generation` of zero, which no reader here
//! consults. See the register for the residue.

use std::io;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use sprag_plugin::{
    AgentObservation, KeyStroke, PaneAccess, PaneError, PaneForegroundJob, PaneHands,
    PaneInputEcho, PaneLifecycle, PaneOutputLines, PaneRow, PaneSupervision, PaneTerminalModes,
    Written,
};
use sprag_rpc::{CallError, HostConn, NO_EXTERNAL_FAULT, Outstanding};
use sprag_terminal::{Hands, JobProcess, PaneEcho, PaneEndOfInput, PaneId};
use sprag_vt::LinesSince;

use crate::external::lock;
use crate::wire::{
    AGENT_SUPERVISION_SLOT, ALT_FIELD, CLOSE_ACTION, CTRL_FIELD, DAEMON_INSTANCE_SLOT,
    FULL_LINES_SLOT, FULL_TEXT_SLOT, INJECT_ACTION, INJECT_STROKES_KEY, INJECTED_BYTES_KEY,
    KEY_FIELD, LINES_KEY, LINES_LOST_KEY, LINES_NEXT_KEY, LINES_PARTIAL_KEY, LINES_RESTARTED_KEY,
    PANE_ECHO_SLOT, PANE_END_OF_INPUT_SLOT, PANE_EOF_SLOT, PANE_FOREGROUND_SLOT, PANE_HANDS_SLOT,
    PANE_SUMMARY_ID_KEY, PANES_SLOT, PEER_GONE_REFUSAL, RESPAWN_ACTION, SCREEN_COLLAPSED_SLOT,
    SCREEN_ROWS_SLOT, SESSION_SLOT, SHIFT_FIELD, SPAWN_ACTION, SPAWN_CMD_KEY, SPAWN_COLS_KEY,
    SPAWN_NAME_KEY, SPAWN_ROWS_KEY, SPLIT_PANE_KEY, SUPER_FIELD, agent_slot_for, hands_of,
    lines_since_at, mux_action_path, pane_input_path, recent_input_has, refusal, unknown_action,
    unknown_slot,
};

/// The JSON-RPC method that reads one address.
const QUERY_METHOD: &str = "scene/query";
/// The JSON-RPC method that performs one action.
const INVOKE_METHOD: &str = "scene/invoke";
/// The parameter both of them address with.
const PATH_PARAM: &str = "path";
/// The parameter [`INVOKE_METHOD`] carries an action's arguments under.
const ARGS_PARAM: &str = "args";

/// A [`PaneAccess`] served by a daemon on the other end of a socket.
///
/// # One connection, taken in turn
///
/// The trait reads through `&self` and a call needs `&mut HostConn`, so the connection lives behind
/// a mutex and each read or injection holds it for exactly its own round trip. That is a decision
/// rather than a workaround, and the cost of it is **measured rather than asserted** (register item
/// 565): **55 µs per `supervision()` call, 2026-08-22, on the build machine (32 cores, 125 GB)**, so
/// the five such reads `outer.rs` makes per step cost **277 µs** against a step bounded by an AI
/// turn — seconds to minutes. Serialising them buys the one property a shared connection needs,
/// which is that two answers cannot interleave on the wire.
///
/// ⚠⚠ The number is there because the sentence used to say *"sub-millisecond, so it costs nothing
/// measurable"* with no date, no figure and no instrument — which is what this repository calls
/// UNMEASURED, about the path a run walks every step. The measurement AGREED with the claim; what
/// it also does is make the claim able to go red, which
/// `a_remote_supervision_read_costs_what_its_documentation_claims` now does at a 20 ms tripwire.
///
/// ⚠ It is measured, not cached. A cache would be justified by a cost this is not — and now there
/// is a number to say so rather than an instinct.
///
/// ⚠ It is NOT a claim that two DRIVERS may share one of these. Nothing here stops that and nothing
/// here makes it safe: the pane a run is driving is guarded by whose run it is, one surface up.
pub struct RemotePaneAccess {
    conn: Mutex<HostConn>,
    /// WHICH DAEMON this surface adopted, learned on its first successful call and never changed
    /// afterwards — see [`DAEMON_INSTANCE_SLOT`] and [`world_changed`](Self::world_changed).
    ///
    /// ⚠ `None` on a daemon too old to publish the address. A surface that cannot learn the
    /// identity cannot notice it changing, so it never latches: the honest degradation, and the
    /// behaviour every build before stage 1d had.
    adopted: Mutex<Option<String>>,
    /// LATCHED once a redial reached a DIFFERENT daemon — see [`world_changed`](Self::world_changed).
    changed: AtomicBool,
    /// The agent state word this daemon published that this build cannot spell, once one has been
    /// met — see [`unspellable_state`](Self::unspellable_state). Register item 564.
    unspellable: Mutex<Option<String>>,
    /// **A SECOND CONNECTION, KEPT FOR PARKS ONLY** — [`None`] until a caller supplies one with
    /// [`parking_on`](Self::parking_on), and what makes [`changes`](PaneAccess::changes) answer
    /// anything but [`None`]. Register item 631.
    ///
    /// # ⚠⚠⚠⚠⚠ Why a park cannot share the connection every read uses
    ///
    /// A [`HostConn`] carries ONE outstanding request and matches replies by id. A park is a
    /// request the daemon deliberately does not answer yet — so while one is outstanding, any
    /// ordinary read on the same socket would meet the park's reply first, find an id it is not
    /// waiting for, and **drop it**. The park would then never be answered and the wait would sleep
    /// out its whole bound over a pane that had already moved.
    ///
    /// That is not a locking problem a mutex can solve: the two uses want the connection at the
    /// same time BY DESIGN, because the whole value of a park is that it stays outstanding while
    /// the driver gets on with looking.
    parks: Mutex<Option<Parked>>,
}

/// The park connection and the one question currently outstanding on it.
///
/// # ⚠⚠ ONE park at a time, and it is REUSED across slices
///
/// `sprag_plugin::run::park_until` calls
/// [`pane_moved_after`](sprag_plugin::PaneChanges::pane_moved_after) once
/// per ten-millisecond slice with the SAME `(pane, seen)` until something moves. Sending a fresh
/// request each time would be the polling this surface exists to remove, so the outstanding request
/// is remembered and the next slice merely waits on it again.
struct Parked {
    /// The socket the parks ride. Its own connection — see [`RemotePaneAccess::parks`].
    conn: HostConn,
    /// The pane, the revision, and the request — `None` when nothing is outstanding.
    ///
    /// ⚠ The pane and the revision are held so a slice can tell *the same question again* from *a
    /// different one*. A different one ABANDONS the old park, which is sound and is documented at
    /// the call: an abandoned park fires the next time that pane moves and its answer is dropped by
    /// id, and the daemon releases every one of them when this connection closes.
    asked: Option<(PaneId, u64, Outstanding)>,
}

/// **THE TWO CONNECTIONS ANSWER DIFFERENT SESSIONS** — what
/// [`RemotePaneAccess::parking_on`] refuses with, and register item 641's word.
///
/// # ⚠⚠⚠ It carries the SURFACE, not only the complaint
///
/// The refusal happens at a builder that has taken `self` by value, so an error that carried only
/// the two names would destroy the thing the caller spent two connections making — and a caller
/// that decides *park-less is better than stopping* is making a legitimate choice, not recovering
/// from a bug. Handing the surface back is what lets that choice cost nothing.
///
/// ⚠⚠ Both names are carried because a person reading *"the park connection is on the wrong
/// session"* cannot act on it: which one is wrong depends on which one they meant, and this type
/// has no opinion about that. The same reason [`RemotePaneAccess::unspellable_state`] carries the
/// word verbatim rather than a verdict.
pub struct MisScoped {
    /// The session the READ connection resolves to, as the daemon answered it.
    pub read: String,
    /// The session the PARK connection resolves to — different, which is the whole of this type.
    pub park: String,
    /// The surface itself, WITHOUT a park connection: the documented degradation, built and ready.
    pub degraded: RemotePaneAccess,
}

/// ⚠⚠ WRITTEN OUT, and the alternative was to give [`RemotePaneAccess`] a derive. It holds live
/// SOCKETS and a latched daemon identity, and a surface that can be printed is one that ends up in
/// a log — so the error names the two sessions, which is the whole of what a reader can act on, and
/// says the surface is here rather than rendering it.
impl std::fmt::Debug for MisScoped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MisScoped")
            .field("read", &self.read)
            .field("park", &self.park)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Display for MisScoped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "a park connection scoped to session {:?} cannot serve a driver reading session {:?}: \
             the daemon refuses a park naming a pane the scoped session does not hold, and that \
             refusal retires the park connection — leaving the driver polling for the rest of its \
             run with nothing to say why",
            self.park, self.read,
        )
    }
}

impl std::error::Error for MisScoped {}

/// **THE TWO ANSWERS, JUDGED** — `Some((read, park))` exactly when they are a refusal, and
/// [`None`] every other way. Register item 641.
///
/// # ⚠⚠⚠⚠⚠ A function of its own, because the ABSENCE arm has no fixture at the door
///
/// [`RemotePaneAccess::parking_on`]'s end-to-end gate drives two real connections to a real daemon
/// — **and this build's daemon always answers**, so *a daemon too old to serve the address* is a
/// branch no fixture there can enter. A gate that cannot reach an arm says nothing about it, and
/// the arm in question is the one that decides whether every park against an older daemon is
/// refused. Lifted out, the decision is where it can be asked all four ways.
///
/// The rule: **only two answers that DISAGREE refuse.** An absence is *this build cannot say*, and
/// reading it as *these are different* would refuse exactly the surface that most needs a park —
/// the `None` is not `Some(0)` lesson, applied at the door rather than after it.
fn scopes_that_disagree<'a>(
    read: Option<&'a str>,
    park: Option<&'a str>,
) -> Option<(&'a str, &'a str)> {
    match (read, park) {
        (Some(read), Some(park)) if read != park => Some((read, park)),
        _ => None,
    }
}

/// How long a redial waits for the socket to accept.
///
/// ⚠ Short on purpose. A daemon that is coming back binds within milliseconds of starting; a longer
/// wait would park a driver's step on a host that is not coming back at all, which is the shape a
/// run's own ceilings exist to prevent.
const REDIAL_WITHIN: std::time::Duration = std::time::Duration::from_millis(500);

impl RemotePaneAccess {
    /// Drive panes through `conn`.
    ///
    /// ⚠ Whether to HANDSHAKE first is the caller's decision, not this type's. The handshake is
    /// where the protocol number and the daemon's build are compared, and what an incompatible
    /// daemon means is a judgement the party that opened the connection owns — a driver that
    /// re-handshook somebody else's connection would be taking it from them. Skew still reaches a
    /// caller here without one: see [`inject`](PaneAccess::inject)'s refusal mapping.
    #[must_use]
    pub const fn over(conn: HostConn) -> Self {
        Self {
            conn: Mutex::new(conn),
            adopted: Mutex::new(None),
            changed: AtomicBool::new(false),
            unspellable: Mutex::new(None),
            parks: Mutex::new(None),
        }
    }

    /// **GIVE THIS SURFACE A SECOND CONNECTION TO PARK ON**, so a wait over the wire stops being a
    /// poll — register item 631. Without one, [`changes`](PaneAccess::changes) answers [`None`] and
    /// every wait takes `park_until`'s documented degradation.
    ///
    /// # ⚠⚠⚠⚠⚠ Why the caller supplies it rather than this type dialling one
    ///
    /// The same reason [`over`](Self::over) does not handshake: a connection's SCOPE and its client
    /// identity are the opening party's to decide, and a surface that dialled its own would have to
    /// guess both. A driver scoped to `work` whose park connection silently landed on the default
    /// session would park on a revision no pane of its own can move — a wait that never wakes, and
    /// one nothing about this type could have got right on its own.
    ///
    /// ⚠⚠ **IT MUST BE SCOPED THE WAY THE READ CONNECTION IS, AND THIS ASKS RATHER THAN TRUSTING**
    /// — register item 641.
    ///
    /// # ⚠⚠⚠⚠⚠ What the obligation cost while it was the caller's word, measured
    ///
    /// The daemon refuses a park naming a pane the SCOPED session does not hold, so a mis-scoped
    /// connection does fail LOUDLY — once. What happens next is the cost: a refused park is a
    /// transport failure to [`pane_moved_after`](sprag_plugin::PaneChanges::pane_moved_after),
    /// which drops the park connection (it must — a failed frame may be half-read), and from that
    /// instant [`changes`](PaneAccess::changes) answers [`None`] **for the rest of the run**. The
    /// driver is back to polling a screen a hundred times a second and nothing anywhere says why.
    /// One mistyped session name, one loud refusal nobody was reading, and a silent permanent
    /// degradation — which is the shape this repository keeps paying for.
    ///
    /// # ⚠⚠⚠ Why an ANSWER and not a promise, now that one is available
    ///
    /// [`SESSION_SLOT`] is *"the daemon's own answer to which session is this about"*, resolved by
    /// the scope AT THE DOOR — so both connections can simply be ASKED, and the comparison is the
    /// daemon's own two answers rather than this type's guess about the caller's intent. The
    /// [`Result`] is what makes it unignorable: a caller that drops it fails the workspace's own
    /// `-D warnings`, where a latched flag would need somebody to remember to read it.
    ///
    /// # ⚠⚠⚠⚠ ONLY TWO ANSWERS THAT DISAGREE REFUSE — an absence is not a disagreement
    ///
    /// A daemon too old to serve the address answers nothing, and *this build cannot say* must not
    /// become *these are different*: that would refuse every park against an older daemon, which is
    /// precisely the surface that most needs one. The rule is the type-level lesson this crate has
    /// paid for twice (`None` is not `Some(0)`), applied before the fact.
    ///
    /// ⚠ A caller with only one connection is not wrong, it is DEGRADED, and the degradation is
    /// named at [`PaneChanges`](sprag_plugin::PaneChanges) rather than hidden here.
    ///
    /// # Errors
    ///
    /// [`MisScoped`] when the two connections answer DIFFERENT session names — carrying both names
    /// and the surface itself, so a caller that would rather degrade than stop keeps what it built.
    pub fn parking_on(self, mut parks: HostConn) -> Result<Self, Box<MisScoped>> {
        let read = Self::scope_of(&mut lock(&self.conn));
        let park = Self::scope_of(&mut parks);
        if let Some((read, park)) = scopes_that_disagree(read.as_deref(), park.as_deref()) {
            return Err(Box::new(MisScoped {
                read: read.to_owned(),
                park: park.to_owned(),
                degraded: self,
            }));
        }
        *lock(&self.parks) = Some(Parked {
            conn: parks,
            asked: None,
        });
        Ok(self)
    }

    /// **WHICH SESSION `conn`'s REQUESTS RESOLVE TO**, as the daemon answers it — [`None`] where
    /// this daemon does not serve the address or the call did not come back.
    ///
    /// ⚠ The daemon's answer and not the connection's [`ScopeAsk`](sprag_rpc::ScopeAsk): a
    /// connection scoped to its ATTACHMENT holds no name at all, so asking the connection would
    /// answer nothing for exactly the client that has one — and a NAMED scope pointing at a session
    /// that has since been retired resolves to something else entirely. The scope resolved it once,
    /// at the door; this reads that.
    fn scope_of(conn: &mut HostConn) -> Option<String> {
        conn.call(
            QUERY_METHOD,
            json!({ PATH_PARAM: mux_action_path(SESSION_SLOT) }),
        )
        .ok()?
        .as_str()
        .map(str::to_owned)
    }

    /// **THE AGENT STATE WORD THIS DAEMON SPOKE THAT THIS BUILD CANNOT SPELL**, once one has been
    /// met — register item 564, and the sentence a person needs to see.
    ///
    /// ⚠⚠⚠ It is a SKEW, not an absence: the daemon is ahead of this driver, and the word is carried
    /// verbatim so whoever reads it can tell WHICH build to look at. While it is set,
    /// [`supervision`](PaneAccess::supervision) answers [`None`] — *ask a person, nothing here can
    /// look* — rather than letting every claimed pane read as a shell.
    #[must_use]
    pub fn unspellable_state(&self) -> Option<String> {
        lock(&self.unspellable).clone()
    }

    /// **WHETHER THE DAEMON UNDER THIS SURFACE HAS BEEN REPLACED** — register item 544, stage 1d.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this latches instead of being handled quietly
    ///
    /// A socket is an ADDRESS, not an identity. When the daemon behind it dies and another takes the
    /// same path, a client that merely redialled would go on using the pane ids it holds — and pane
    /// ids are minted from a counter that starts at ZERO, so a fresh daemon's own boot pane **is
    /// pane 0**. A driver holding pane 0 would type its stimulus into a stranger's shell and be told
    /// it succeeded every time.
    ///
    /// So a redial that reaches a different [`DAEMON_INSTANCE_SLOT`] latches this, and from then on
    /// every read answers [`None`] — *I cannot see that pane*, which is the reading every consumer
    /// of this trait already stops on. ⚠ The run is not lost: the world may well be the same one,
    /// restored under the same ids. But that is a judgement about EVIDENCE (has my pane come back,
    /// under the name I gave it?) and it belongs to the driver, which is why it must be told rather
    /// than carried past. See [`readopt`](Self::readopt).
    #[must_use]
    pub fn world_changed(&self) -> bool {
        self.changed.load(Ordering::Acquire)
    }

    /// The daemon instance this surface adopted, or [`None`] where it never learned one.
    #[must_use]
    pub fn adopted_instance(&self) -> Option<String> {
        lock(&self.adopted).clone()
    }

    /// **ADOPT THE DAEMON THAT IS THERE NOW AND DRIVE AGAIN** — what a driver calls once it has
    /// re-established, on its own evidence, that the panes it holds are the ones it means.
    ///
    /// ⚠⚠ It takes no argument and makes no check, deliberately: this type cannot know what the
    /// caller was driving or how it would recognise it again. A run identifies its pane by the NAME
    /// it gave it — the one address that survives a restore — and calling this is the driver saying
    /// *I have looked, and it is mine*. A surface that re-adopted itself would be inventing that
    /// judgement.
    pub fn readopt(&self) {
        *lock(&self.adopted) = None;
        self.changed.store(false, Ordering::Release);
    }

    /// **THE PANE CARRYING `name` ON THE DAEMON THAT IS THERE NOW** — the read a driver re-adopts
    /// with. Register item 544, stage 1e.
    ///
    /// # ⚠⚠⚠⚠⚠ A NAME is the only address that survives a restart; an id is not
    ///
    /// Pane ids are minted from a counter, so *pane 3* means whatever the daemon holding the socket
    /// says it means — and after a restart that is a different sentence. A NAME is given by whoever
    /// asked for the pane and is unique across the registry, so *the pane I called `inner`* is a
    /// question with one answer before and after. That is why a run names the pane it drives.
    ///
    /// # ⚠⚠⚠ It reads THROUGH the latch, deliberately
    ///
    /// Every other read on this surface answers [`None`] once
    /// [`world_changed`](Self::world_changed) is set, because a pane id means nothing then. This one
    /// is the question a caller asks PRECISELY at that moment, so refusing it would leave the latch
    /// with no way out but blind faith. ⚠ It answers about the new daemon and says so by
    /// construction: the id it returns is the NEW daemon's.
    ///
    /// ⚠⚠ `None` where no pane carries the name — which after a restart means *the world did not
    /// come back*, and is a run's cue to end rather than to type.
    #[must_use]
    pub fn pane_named(&self, name: &str) -> Option<PaneId> {
        let listing = lock(&self.conn)
            .try_call(
                QUERY_METHOD,
                json!({ PATH_PARAM: mux_action_path(PANES_SLOT) }),
            )
            .ok()?;
        listing.as_array()?.iter().find_map(|entry| {
            (entry[SPAWN_NAME_KEY].as_str() == Some(name))
                .then(|| entry[PANE_SUMMARY_ID_KEY].as_u64().map(PaneId))
                .flatten()
        })
    }

    /// Redial the socket this connection names, answering whether the SAME daemon is there.
    ///
    /// ⚠⚠⚠ A connection built from a bare stream knows no path, so it cannot redial at all — and
    /// that answers `false` rather than pretending. `HostConn::socket` is what separates the two.
    fn recover(&self) -> bool {
        let mut conn = lock(&self.conn);
        let Some(path) = conn.socket().map(std::path::Path::to_path_buf) else {
            return false;
        };
        let Ok(fresh) = HostConn::connect(&path, REDIAL_WITHIN) else {
            return false;
        };
        *conn = fresh;
        // ⚠⚠ THE IDENTITY IS COMPARED AGAINST WHAT WAS ADOPTED, and an unknown on EITHER side is
        // not a match: a daemon that will not say which it is cannot be shown to be the same one.
        let now = Self::instance_of(&mut conn);
        let adopted = lock(&self.adopted).clone();
        match (adopted, now) {
            (Some(before), Some(after)) if before == after => true,
            (None, _) => {
                // Nothing was adopted yet, so nothing can be said to have changed. The next
                // successful call adopts whatever is there.
                true
            }
            _ => {
                self.changed.store(true, Ordering::Release);
                false
            }
        }
    }

    /// Read [`DAEMON_INSTANCE_SLOT`] on an already-held connection.
    fn instance_of(conn: &mut HostConn) -> Option<String> {
        conn.try_call(
            QUERY_METHOD,
            json!({ PATH_PARAM: mux_action_path(DAEMON_INSTANCE_SLOT) }),
        )
        .ok()?
        .as_str()
        .map(str::to_owned)
    }

    /// Learn which daemon this is, the first time anything succeeds.
    ///
    /// ⚠ ONE extra round trip for the life of the surface, not one per call: the slot is read only
    /// while nothing has been adopted. A daemon too old to answer leaves it unadopted, which is what
    /// makes [`world_changed`](Self::world_changed) stay false there rather than fire on every
    /// redial.
    fn adopt(&self) {
        if lock(&self.adopted).is_some() {
            return;
        }
        let learned = Self::instance_of(&mut lock(&self.conn));
        let mut adopted = lock(&self.adopted);
        if adopted.is_none() {
            *adopted = learned;
        }
    }

    /// Read one address, or [`None`] when this daemon does not answer it.
    ///
    /// # ⚠⚠⚠ A `None` here is *"nothing at this address"*, and it has THREE causes
    ///
    /// The pane is gone (what every [`PaneAccess`] reader means by a `None`), the daemon is older
    /// than this driver and never served the address, or the wire itself failed. All three collapse
    /// to *I cannot see that pane from here*, which is the SAFE reading: a driver that cannot see a
    /// pane stops rather than types. ⚠ They are not the same fact and a supervisor wants them
    /// apart — see the register's residue for this stage.
    fn read(&self, path: &str) -> Option<Value> {
        // ⚠⚠⚠⚠⚠ A SURFACE WHOSE DAEMON WAS REPLACED SEES NOTHING until its caller re-adopts. This
        // is the FIRST statement rather than a late check because every answer below would
        // otherwise be about a world the caller never chose — see `world_changed`.
        if self.world_changed() {
            return None;
        }
        // ⚠⚠⚠⚠⚠ ADOPTED BEFORE ANYTHING IS DRIVEN, AND FROM BOTH DOORS. The first draft adopted
        // only on a successful READ, and the gate caught what that means: a driver whose first act
        // is an INJECTION never learns which daemon it is talking to, so it can never notice one
        // being replaced — the whole property, silently absent for exactly the caller that types
        // first. You adopt, then you drive; the order is the claim.
        self.adopt();
        // ⚠⚠⚠⚠⚠ THE ANSWER IS TAKEN AS A VALUE AND THE LOCK IS GONE BEFORE IT IS EXAMINED. A
        // `match lock(..).try_call(..)` holds the guard for the whole match — the scrutinee's
        // temporaries live that long — so any arm that came to ask this surface a second question
        // would deadlock, and only on the path that has something to say. This workspace has
        // measured that exact shape twice (a format argument that re-locked, evaluated only when
        // the assertion failed: green for as long as it passed, a 93-minute hang the moment it
        // did not). Structure, not vigilance. ⚠ The recovery arm below is exactly such a second
        // question, which is why this shape is load-bearing rather than stylistic.
        let outcome = lock(&self.conn).try_call(QUERY_METHOD, json!({ PATH_PARAM: path }));
        match outcome {
            Ok(value) if value.is_null() => None,
            Ok(value) => Some(value),
            Err(CallError::Fault(fault)) => {
                // Reported at DEBUG rather than swallowed: an address this daemon does not serve is
                // a skew a person can fix, and the sentence naming the remedy already exists.
                if let Some(skew) = unknown_slot(path, &fault) {
                    tracing::debug!(target: "sprag_host", %skew, "a remote read found a skew");
                }
                None
            }
            Err(CallError::Transport(error)) => {
                tracing::debug!(target: "sprag_host", %error, %path, "a remote read did not complete");
                // ⚠⚠⚠ A READ MAY BE ASKED AGAIN — it changes nothing, so a transient socket failure
                // costs a round trip rather than a step. It is asked again ONLY where the daemon
                // that answered is the one this surface adopted; a different daemon latches instead
                // and this returns the `None` every consumer stops on.
                if !self.recover() {
                    return None;
                }
                let retried = lock(&self.conn).try_call(QUERY_METHOD, json!({ PATH_PARAM: path }));
                match retried {
                    Ok(value) if value.is_null() => None,
                    Ok(value) => Some(value),
                    Err(_) => None,
                }
            }
        }
    }

    /// Read one address of one pane.
    fn read_pane(&self, id: PaneId, slot: &str) -> Option<Value> {
        self.read(&pane_input_path(id.0, slot))
    }

    /// Invoke a BIRTH verb — one that answers a new pane's id — and read that id back.
    ///
    /// The one place [`SPAWN_ACTION`] and [`RESPAWN_ACTION`] share, because they answer the same
    /// shape and fail the same three ways, and a second copy would be a second set of sentences to
    /// drift apart.
    ///
    /// ⚠⚠ An answer that is not a number is a REFUSAL and not a zero: a daemon answering a shape
    /// this client cannot read has not opened a pane, and a caller handed `PaneId(0)` would go on to
    /// drive whichever pane really has that id.
    fn born(&self, path: String, args: Value) -> Result<PaneId, PaneError> {
        // THE ANSWER IS A VALUE BEFORE IT IS EXAMINED, for `read`'s reason — see the note there.
        let outcome =
            lock(&self.conn).try_call(INVOKE_METHOD, json!({ PATH_PARAM: path, ARGS_PARAM: args }));
        let answer = outcome.map_err(|error| {
            PaneError::Spawn(match error {
                CallError::Transport(error) => error.to_string(),
                CallError::Fault(fault) => unknown_action(&path, &fault)
                    .or_else(|| refusal(&fault))
                    .unwrap_or_else(|| io::Error::other(fault.to_string()))
                    .to_string(),
            })
        })?;
        answer.as_u64().map(PaneId).ok_or_else(|| {
            PaneError::Spawn(format!(
                "{path} answered {answer}, which names no pane, so nothing here can be driven"
            ))
        })
    }

    /// Turn an [`INJECT_ACTION`] failure into the typed cause it names.
    ///
    /// # ⚠⚠⚠⚠⚠ The peer-gone word is MATCHED, and that is why it is a constant
    ///
    /// A driver that failed to recognise this refusal would read *the pane's child has exited* as
    /// some other error — and the remedy for *some other error* is to try again, which is the
    /// patient march into a pseudoterminal that takes a bounded number of bytes and then blocks for
    /// ever. So the word has one definition ([`PEER_GONE_REFUSAL`]): the daemon refuses with it and
    /// this maps it back.
    ///
    /// # ⚠⚠⚠⚠⚠ A path that resolved to NOTHING is a different fact from a verb that is missing
    ///
    /// A pane nobody knows and a daemon too old to have this door arrive as ONE JSON-RPC code, and
    /// the only thing separating them is pinion's payload word — [`NO_EXTERNAL_FAULT`] for *there
    /// is no surface at that path*, [`UNKNOWN_ACTION_FAULT`](sprag_rpc::UNKNOWN_ACTION_FAULT) for
    /// *the surface is there and has no such verb*. So a gone pane answers
    /// [`PaneError::UnknownPane`], exactly as the in-process door does, and a skew answers the
    /// sentence that carries its remedy.
    ///
    /// ⚠⚠ **AND THE FIRST DRAFT GOT IT WRONG, WHICH IS WHY THE ARM IS GATED**: it knew two words
    /// of the three, so injecting into a pane nobody knew reported *this daemon does not perform
    /// that action* — an operator told to restart a daemon that was perfectly current. Found by
    /// the gate's unknown-pane arm, on the round the mapping was written.
    fn injection_failed(id: PaneId, path: &str, error: CallError) -> PaneError {
        let fault = match error {
            CallError::Transport(error) => return PaneError::Write(error.to_string()),
            CallError::Fault(fault) => fault,
        };
        if fault.refusal() == Some(PEER_GONE_REFUSAL) {
            return PaneError::PeerGone(id);
        }
        if fault.data.as_ref().and_then(Value::as_str) == Some(NO_EXTERNAL_FAULT) {
            return PaneError::UnknownPane(id);
        }
        PaneError::Write(
            unknown_action(path, &fault)
                .or_else(|| refusal(&fault))
                .unwrap_or_else(|| io::Error::other(fault.to_string()))
                .to_string(),
        )
    }
}

/// One stroke in the form [`INJECT_ACTION`] declares — the object form, with every modifier stated.
///
/// ⚠ Stated rather than omitted when false: the daemon reads an absent flag as not-held, so both
/// spellings mean the same thing, and a form that always carries the same keys is one a reader of a
/// captured request can compare against the declaration without knowing which flags were in play.
fn stroke_form(stroke: &KeyStroke) -> Value {
    json!({
        KEY_FIELD: stroke.key,
        CTRL_FIELD: stroke.mods.ctrl,
        ALT_FIELD: stroke.mods.alt,
        SHIFT_FIELD: stroke.mods.shift,
        SUPER_FIELD: stroke.mods.sup,
    })
}

impl PaneAccess for RemotePaneAccess {
    fn pane_ids(&self) -> Vec<PaneId> {
        self.read(&mux_action_path(PANES_SLOT))
            .and_then(|panes| {
                panes.as_array().map(|entries| {
                    entries
                        .iter()
                        .filter_map(|entry| entry[PANE_SUMMARY_ID_KEY].as_u64().map(PaneId))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        self.read_pane(id, SCREEN_COLLAPSED_SLOT)?
            .as_str()
            .map(str::to_owned)
    }

    /// The pane's rows, as TEXT.
    ///
    /// ⚠⚠ The generation is zero on every row, and that is a published decision rather than a
    /// shortcut — see this module's own documentation. A reader asking *what has this pane
    /// produced* compares the text, which is what [`RowTrail`](sprag_plugin::RowTrail) does and
    /// what the paint number was measured to answer wrongly.
    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>> {
        let rows: Vec<String> =
            serde_json::from_value(self.read_pane(id, SCREEN_ROWS_SLOT)?).ok()?;
        Some(
            rows.into_iter()
                .map(|text| PaneRow {
                    generation: 0,
                    text,
                })
                .collect(),
        )
    }

    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        self.read_pane(id, PANE_EOF_SLOT)?.as_bool()
    }

    fn pane_full_text(&self, id: PaneId) -> Option<String> {
        self.read_pane(id, FULL_TEXT_SLOT)?
            .as_str()
            .map(str::to_owned)
    }

    /// The pane's output as the LOGICAL LINES its child wrote.
    ///
    /// ⚠⚠⚠ Read at its own address rather than left to the trait's default, which splits the
    /// RENDERED text back into lines. That default is a documented degradation for a host that
    /// cannot answer the content question — and this one can, because the daemon publishes it. A
    /// driver that took the rendering would have every marker it matches decided by whichever
    /// display client is attached, which is exactly the defect stage 1b's pair exists to prevent.
    fn pane_full_lines(&self, id: PaneId) -> Option<Vec<String>> {
        serde_json::from_value(self.read_pane(id, FULL_LINES_SLOT)?).ok()
    }

    /// Type `keys` into the pane as ONE write, through the door the daemon publishes for a driver.
    ///
    /// ⚠⚠⚠⚠ **The refusal for a pane whose child has left is the DAEMON's, not this client's**, and
    /// that is deliberate: asking [`PANE_EOF_SLOT`] here first would decide on a fact read a round
    /// trip ago about a child that can exit in between. The party holding the atomic answers it at
    /// the write; this maps the word back to [`PaneError::PeerGone`].
    /// **OPENING, REPLACING AND CLOSING PANES ON THE OTHER END** — register item 557.
    ///
    /// ⚠ Always `Some`, and that is a statement rather than a shortcut: a `None` here means *this
    /// host cannot open panes at all*, which is what an in-process access with no workspace answers.
    /// A daemon serving this wire can open panes; a daemon too old to know one of the verbs says so
    /// AT THE CALL, in a sentence naming the remedy, which is the discrimination
    /// [`inject`](PaneAccess::inject) already established for the typing door.
    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        Some(self)
    }

    /// **WHAT HAS BEEN WRITTEN INTO A PANE** — register item 557.
    ///
    /// ⚠ Always `Some`, on [`lifecycle`](PaneAccess::lifecycle)'s terms: a daemon serving this wire
    /// remembers what was typed at its panes, and a daemon too old to publish the address answers a
    /// `None` at the READ, which is the honest degradation the consumer already handles.
    fn input_echo(&self) -> Option<&dyn PaneInputEcho> {
        Some(self)
    }

    /// **WHAT A PANE HAS SAID SINCE A READER'S CURSOR** — register item 557, and the read a RELAY
    /// is.
    ///
    /// ⚠⚠ Served rather than left to the trait's default. That default re-reads the pane's WHOLE
    /// output and slices it, which loses the two facts only the pane can supply: what was evicted
    /// unread, and whether the numbering the cursor came from still exists.
    fn output_lines(&self) -> Option<&dyn PaneOutputLines> {
        Some(self)
    }

    /// **WHO OWNS A PANE'S TERMINAL** — register item 557, and the surface `ReadyWhen::Runs` is.
    ///
    /// ⚠ Always `Some`; the per-pane `None` carries the absence, which is *nothing owns that
    /// terminal* or *this host has no process table*. That second one is a real platform fact and
    /// the reason `PaneDoing` has three arms rather than two.
    fn foreground_job(&self) -> Option<&dyn PaneForegroundJob> {
        Some(self)
    }

    /// **WHO HAS WRITTEN INTO A PANE, AND HOW MANY TIMES EACH** — register item 653, and the read a
    /// run makes BEFORE it makes any other.
    ///
    /// # ⚠⚠⚠⚠⚠ The absence this replaces was not a degradation
    ///
    /// [`Readiness::reached`](sprag_plugin::Readiness::reached) asks *has a person reached into this
    /// pane* first, ahead of the barrier and ahead of any consent, and it asks it here. While this
    /// answered `None`, a run driven from another process concluded **nobody has ever touched this
    /// pane** — for every pane, for its whole life — and typed over whoever was there. Every other
    /// absence on item 557's list makes its consumer degrade in the safe direction; this one made it
    /// confident and wrong, which is why the protocol number moved for the address behind it.
    ///
    /// ⚠ Always `Some`, on [`foreground_job`](PaneAccess::foreground_job)'s terms: a daemon serving
    /// this wire counts hands at every door it has. A daemon too old to publish the address cannot
    /// be reached at all — the handshake refuses a protocol that is not this one — which is the
    /// arrangement this surface needs and did not have while the counts were a safe-to-miss extra.
    fn hands(&self) -> Option<&dyn PaneHands> {
        Some(self)
    }

    /// **WHAT A PANE'S TERMINAL DOES WITH WHAT IS WRITTEN INTO IT** — register item 557.
    ///
    /// ⚠ Always `Some`, and the per-pane `None`s carry the real absence: *this host could not read
    /// the mode*. Collapsing the two would tell a caller that no pane on this daemon has a terminal.
    fn terminal_modes(&self) -> Option<&dyn PaneTerminalModes> {
        Some(self)
    }

    /// **WHETHER THE DAEMON ON THE OTHER END SUPERVISES AT ALL** — register item 557, and one of the
    /// two absences a supervisor must never see collapsed.
    ///
    /// # ⚠⚠⚠⚠⚠ The `None`s below and beside are OPPOSITE instructions
    ///
    /// A `None` HERE says *nothing on that host will ever answer about an agent; ask a person*. A
    /// `None` from [`pane_agent_state`](PaneSupervision::pane_agent_state) says *that pane is a
    /// shell; carry on*. A surface publishing one word for both — which is what a remote driver had
    /// until this address existed, since every optional sub-surface answered `None` — lets a
    /// supervisor conclude "no agents here" from a daemon that never looked.
    ///
    /// ⚠⚠ A daemon too old to serve the address, and a wire that failed, both land on the first
    /// reading. That is the SAFE direction and it is deliberate: *this build cannot supervise* sends
    /// the run to a person, where the other reading would have it decide a turn had ended.
    ///
    /// ⚠ One round trip per call — **55 µs, measured 2026-08-22** (register item 565, and this
    /// type's own documentation for the figure). It is asked rather than cached because a daemon is
    /// a process that can be replaced under this connection, and a capability remembered from a
    /// handshake would outlive the thing it described. ⚠⚠ Stage 1d gave this surface a way to NOTICE
    /// that replacement, so a cache could now be invalidated correctly — the measurement is what
    /// says it is not worth building.
    fn supervision(&self) -> Option<&dyn PaneSupervision> {
        // ⚠⚠⚠⚠⚠ A DAEMON WHOSE VOCABULARY IS AHEAD OF THIS BUILD CANNOT BE SUPERVISED FROM HERE —
        // register item 564. This surface can still READ the verdicts; what it cannot do is
        // understand them, and *"I looked and it is a shell"* about a pane running an agent this
        // driver has never heard of is the worst answer available. `None` here is the honest one:
        // ask a person.
        if self.unspellable_state().is_some() {
            return None;
        }
        self.read(&mux_action_path(AGENT_SUPERVISION_SLOT))?
            .as_bool()?
            .then_some(self as &dyn PaneSupervision)
    }

    /// **WHETHER THIS SURFACE CAN BE WAITED ON RATHER THAN ASKED** — register item 631.
    ///
    /// `Some` exactly when a caller supplied a park connection ([`parking_on`](Self::parking_on)),
    /// and that is the honest discrimination rather than an always-`Some` that fails at the call: a
    /// `None` here means *this build cannot say when a pane changed*, and `park_until` reads it as
    /// the instruction to fall back to a clock. Answering `Some` without somewhere to park would
    /// make every wait return "nothing moved" instantly and spin.
    ///
    /// ⚠⚠ It does NOT ask the daemon whether it serves the address. A daemon too old to know
    /// `pane/waitForRevision` refuses the park with an [`INVALID_PARAMS`](sprag_rpc::INVALID_PARAMS)
    /// -shaped fault at the first slice, and `pane_moved_after` answers `None` to that — which puts
    /// the wait on the same degradation, one round trip later. Paying a probe per surface to save a
    /// round trip once would be the cache this type's own documentation declines.
    fn changes(&self) -> Option<&dyn sprag_plugin::PaneChanges> {
        lock(&self.parks)
            .is_some()
            .then_some(self as &dyn sprag_plugin::PaneChanges)
    }

    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
        let path = pane_input_path(id.0, INJECT_ACTION);
        let strokes: Vec<Value> = keys.iter().map(stroke_form).collect();
        let args = json!({ PATH_PARAM: path, ARGS_PARAM: { INJECT_STROKES_KEY: strokes } });
        // ⚠⚠ THE ANSWER IS A VALUE BEFORE IT IS EXAMINED, for `read`'s reason: a failure mapping
        // that came to ask this surface a second question would deadlock under a guard still held
        // by the scrutinee, and only ever on the path that has something to report. A draft of
        // this one did exactly that — it read the pane list to tell a gone pane from a skew.
        // ⚠⚠⚠⚠⚠ A SURFACE WHOSE DAEMON WAS REPLACED WRITES NOTHING. The read side answers `None`
        // and a driver stops; this one has to REFUSE, because "I cannot see it" and "I typed into
        // something" are not interchangeable when the something might be a stranger's shell.
        if self.world_changed() {
            return Err(PaneError::Write(format!(
                "the daemon behind this connection was replaced, so {path} names a pane this \
                 driver never adopted"
            )));
        }
        // ⚠⚠ ADOPTED HERE TOO — see `read`. A run whose first act is to type is the caller this
        // property exists for, and it is the one the read-only adoption forgot.
        self.adopt();
        let outcome = lock(&self.conn).try_call(INVOKE_METHOD, args);
        // ⚠⚠⚠⚠⚠ A TRANSPORT FAILURE ON A WRITE IS NEVER RETRIED, and that is the difference between
        // this door and the read above. A read changes nothing, so asking again costs a round trip.
        // A write that failed in transit is a write whose FATE IS UNKNOWN — the daemon may have
        // taken every byte and died before answering — and typing a run's stimulus a second time is
        // how a peer is asked its question twice. So this recovers the CONNECTION (the next step
        // has somewhere to go) and reports the failure to the caller, which is whose judgement the
        // second attempt is.
        if matches!(outcome, Err(CallError::Transport(_))) {
            let _ = self.recover();
        }
        let answer = outcome.map_err(|error| Self::injection_failed(id, &path, error))?;
        answer[INJECTED_BYTES_KEY]
            .as_u64()
            .map(Written::of)
            .ok_or_else(|| {
                // NOT a zero. An empty batch answers `bytes: 0`, so a MISSING count is a daemon
                // answering a shape this driver cannot read — and charging a run nothing for a
                // write it cannot measure is the reading that hides it.
                PaneError::Write(format!(
                    "{path} answered no {INJECTED_BYTES_KEY}, so what it wrote cannot be counted"
                ))
            })
    }
}

/// **WAITING ON A PANE FROM OUTSIDE THE DAEMON** — register item 631, and the surface that stops a
/// remote driver re-reading a screen it has no reason to believe has changed.
///
/// # ⚠⚠⚠⚠⚠ What this replaces, measured on a real daemon
///
/// **2026-08-24, build machine (32 cores, 125 GB), a one-second wait ended by the pane printing a
/// marker: 96 looks polling, 2 parked.** A look here is not a cheap one — it is a whole SCREEN
/// over a socket, and a supervisor's detector run over the result. The gate is
/// `a_remote_driver_parks_on_a_pane_instead_of_re_reading_its_screen`
/// (`sprag-host/tests/wire_client.rs`), and it asserts the ratio against a driver holding one
/// connection rather than an absolute number, which is what survives a shared runner.
///
/// # ⚠⚠⚠ It does not close item 631 on its own, and saying so is the point
///
/// A wait whose predicate rests on the AGENT VERDICT still looks every slice, because that verdict
/// arrives with [`Settling::Unknown`](sprag_plugin::Settling::Unknown) — the wire carries no
/// deadline for it — and an unknown deadline must buy a look. What this fixes is every wait whose
/// predicate rests on the pane's BYTES: `readiness::left_the_question`, the orchestrator's sentinel
/// and row-trail arms, which are the ones deliberately built to answer
/// [`Look::Steady`](sprag_plugin::run::Look::Steady). The verdict's own deadline is the other half.
impl sprag_plugin::PaneChanges for RemotePaneAccess {
    /// The pane's revision, read at its own address.
    ///
    /// ⚠ CHEAP BY THE STANDARD OF THIS SURFACE, which is not the standard of the in-process one: it
    /// is a round trip (**55 µs**, register item 565) where in-process it is a lock take and an
    /// integer read. It is still two orders cheaper than the alternative it removes, which is a
    /// whole screen plus a detector — and the contract is the same either way, so no caller has to
    /// know which surface it holds.
    fn pane_revision(&self, id: PaneId) -> Option<u64> {
        self.read_pane(id, crate::wire::PANE_REVISION_SLOT)?
            .as_u64()
    }

    /// Park until `id` passes `seen`, or until `within` elapses — answering the revision as it
    /// stands on the way out.
    ///
    /// # ⚠⚠⚠⚠⚠ The request is sent ONCE and waited on in slices
    ///
    /// `park_until` asks this every ten milliseconds with the same `(id, seen)` so it can hear a
    /// cancel in between. If each of those sent a request, this would be a poll of a cheap number
    /// rather than a poll of a screen — better, and still a poll, and still an hour of patience
    /// spent as 360,000 round trips. So the park is REMEMBERED: the first slice sends it, and every
    /// slice after waits on the same outstanding reply through
    /// [`HostConn::settle`](sprag_rpc::HostConn::settle). A slice that ends in silence costs a
    /// socket read timeout and nothing whatever on the wire or in the daemon.
    ///
    /// ⚠⚠ **A DIFFERENT QUESTION ABANDONS THE OLD PARK**, deliberately and not by oversight. It
    /// happens when the pane moved (so the old park has already been answered) or when a caller
    /// waits on another pane. An abandoned park fires the next time its pane moves and its answer
    /// is dropped by id; the daemon releases every park this connection holds when it closes. What
    /// it costs is one daemon-side entry per pane that never moves again, which is why
    /// `ChannelRegistry::release` names this case in its own comment.
    ///
    /// ⚠⚠⚠ **A FAILURE ANSWERS [`None`], WHICH IS THE DEGRADATION AND NOT AN ERROR.** A daemon too
    /// old to serve the address, a park refused, a socket that died — all three mean *this surface
    /// cannot tell you when the pane moved*, and `park_until` reads that as the instruction to fall
    /// back to a clock. Reporting it as *the pane did not move* would park a driver on a signal
    /// that is never coming.
    fn pane_moved_after(&self, id: PaneId, seen: u64, within: Duration) -> Option<u64> {
        // ⚠ A SURFACE WHOSE DAEMON WAS REPLACED WAITS ON NOTHING — `read`'s first statement, for
        // its reason: pane ids mean nothing across a replacement, and a park on one would be a wait
        // on a stranger's pane.
        if self.world_changed() {
            return None;
        }
        let mut held = lock(&self.parks);
        let parked = held.as_mut()?;
        // The SAME question again — resume it. A different one is a new park, and the old one is
        // abandoned (see this method's own note).
        if !matches!(parked.asked, Some((pane, since, _)) if pane == id && since == seen) {
            let asked = parked
                .conn
                .begin(
                    crate::wire::PANE_WAIT_REVISION_METHOD,
                    json!({ crate::wire::PANE_PARAM: id.0, crate::wire::SINCE_PARAM: seen }),
                )
                .ok()?;
            parked.asked = Some((id, seen, asked));
        }
        // Cloned out so the borrow of `parked.asked` ends before `settle` takes `&mut parked.conn`.
        let (_, _, outstanding) = parked.asked.as_ref()?;
        let outstanding = outstanding.clone();
        match parked.conn.settle(&outstanding, within) {
            // It moved. The answer is where the pane is NOW, not `seen + 1`, so a caller handing it
            // back as the next `since` can neither be woken twice for one move nor miss a second.
            Ok(Some(answer)) => {
                parked.asked = None;
                answer[crate::wire::PANE_REVISION_FIELD].as_u64()
            }
            // The slice elapsed with the park still outstanding. `seen` UNCHANGED is the contract's
            // word for *nothing happened*, and it is what stops the caller taking a look.
            Ok(None) => Some(seen),
            Err(error) => {
                tracing::debug!(
                    target: "sprag_host",
                    error = ?error,
                    pane = id.0,
                    "a remote pane park did not complete; the wait degrades to a clock",
                );
                // ⚠ THE PARK CONNECTION IS DROPPED, not merely forgotten. A transport failure may
                // have left half a frame in the stream, and `HostConn` retires itself for exactly
                // that — keeping it would answer `None` for ever while looking like a capability.
                // Dropping it also releases every park the daemon held for it.
                *held = None;
                None
            }
        }
    }
}

/// **OPENING, REPLACING AND CLOSING PANES OVER THE SOCKET** — register item 557, and the surface
/// `outer.rs` rolls a loop's inner session through.
///
/// # ⚠⚠⚠⚠⚠ `respawn` is a VERB here, not a composition, and that is the whole point
///
/// The other two are calls a client could obviously make. `respawn` is the one it must NOT assemble
/// from them: the replacement's argv, environment, working directory and size are read off the
/// outgoing pane, its seat is handed over as one operation, and the old pane is closed only after
/// the new one exists. A client that composed `close` + `spawn` would start the same program in a
/// different world, drop the seat one declaration at a time, and — on a spawn that failed — have
/// already destroyed the session it was preserving. So the daemon publishes the whole act
/// ([`RESPAWN_ACTION`]) and this forwards it.
impl PaneLifecycle for RemotePaneAccess {
    /// Open a pane running `argv` at `cols` x `rows`.
    ///
    /// ⚠ No working directory and no environment, which is what this door has always meant — the
    /// in-process one says the same, and `respawn` is the caller that has an opinion about both and
    /// reads them off the pane rather than being told.
    ///
    /// # Errors
    ///
    /// [`PaneError::Spawn`] when the daemon refuses the birth, when it does not serve the verb, or
    /// when the wire fails — each carrying the sentence that names it.
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, PaneError> {
        self.born(
            mux_action_path(SPAWN_ACTION),
            json!({ SPAWN_CMD_KEY: argv, SPAWN_COLS_KEY: cols, SPAWN_ROWS_KEY: rows }),
        )
    }

    /// Replace `id` with a fresh pane running the same thing in the same place — see this impl's own
    /// documentation for why it is one call.
    ///
    /// # Errors
    ///
    /// [`PaneError::Spawn`] when the daemon refuses (a pane it does not hold, or one with no
    /// recorded command to re-run), when it is too old to serve the verb, or when the wire fails.
    fn respawn(&self, id: PaneId) -> Result<PaneId, PaneError> {
        self.born(
            mux_action_path(RESPAWN_ACTION),
            json!({ SPLIT_PANE_KEY: id.0 }),
        )
    }

    /// Close `id`, answering whether it existed.
    ///
    /// ⚠⚠ A refusal reads as *it was not there*, which is this method's own contract (`bool`, not a
    /// `Result`) and the safe direction: the caller's next act is to stop using the pane either way.
    /// ⚠ A wire failure therefore reads the same as a closed pane, and cannot be told apart here —
    /// register item 556's shape, on the one door whose type has no room to say so.
    fn close(&self, id: PaneId) -> bool {
        let args = json!({ PATH_PARAM: mux_action_path(CLOSE_ACTION), ARGS_PARAM: { "id": id.0 } });
        let outcome = lock(&self.conn).try_call(INVOKE_METHOD, args);
        outcome.is_ok()
    }
}

/// **WHAT WAS WRITTEN INTO A PANE, READ OVER THE SOCKET** — register item 557, and the read that
/// keeps a remote barrier from converging on the driver's own keystroke.
impl PaneInputEcho for RemotePaneAccess {
    /// The pane's echo trail, or [`None`] for a pane this daemon does not hold (or does not publish
    /// the address for).
    ///
    /// ⚠⚠ Read at its own address rather than derived from any screen slot, and the two are
    /// genuinely different facts: a terminal that does not echo — a password prompt — puts input in
    /// here and nothing on the grid, and output nobody typed is on the grid and not in here. A
    /// client that tried to satisfy this from the screen would answer *the marker was typed* about
    /// text the program printed, which inverts the very refusal this read exists for.
    ///
    /// ⚠⚠⚠⚠⚠ **IT ASKS AND DOES NOT FETCH** — register item 567. This surface is the one that
    /// crosses a socket, so it is the one where a trail could reach a reader who is not the writer.
    /// The needle goes out, a `bool` comes back, and the pane's other bytes never leave the daemon.
    /// `RemotePaneAccess` therefore offers no [`PaneInputTrail`](sprag_plugin::PaneInputTrail) at
    /// all: `input_trail()` keeps the trait's `None`, which on `PaneAccess` means what it always
    /// means — *this surface does not have that capability* — rather than *the trail is empty*.
    fn pane_recent_input_has(&self, id: PaneId, needle: &str) -> Option<bool> {
        self.read_pane(id, &recent_input_has(needle))?.as_bool()
    }
}

/// **WHAT A PANE HAS SAID SINCE A CURSOR, READ OVER THE SOCKET** — register item 557.
impl PaneOutputLines for RemotePaneAccess {
    /// The lines after `cursor`, or [`None`] for a pane this daemon does not hold.
    ///
    /// ⚠⚠⚠ **A MISSING FIELD IS ITS OWN SAFE VALUE, NOT A REFUSAL, AND THE TWO COUNTERS ARE NOT
    /// SAFE IN THE SAME DIRECTION.** `lost` defaults to ZERO — *nothing was evicted* — which is
    /// what a reader from a daemon too old to count it should assume, because the alternative would
    /// have every step report a gap that never happened. `next` defaults to the CURSOR the caller
    /// passed, so a reader whose answer lost that field re-reads rather than skips: an address that
    /// answered zero would rewind the relay to the beginning of the pane on every step.
    fn pane_lines_since(&self, id: PaneId, cursor: u64) -> Option<LinesSince> {
        let answer = self.read_pane(id, &lines_since_at(cursor))?;
        Some(LinesSince {
            lines: answer[LINES_KEY]
                .as_array()
                .map(|lines| {
                    lines
                        .iter()
                        .filter_map(|line| line.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            next: answer[LINES_NEXT_KEY].as_u64().unwrap_or(cursor),
            lost: answer[LINES_LOST_KEY].as_u64().unwrap_or(0),
            partial: answer[LINES_PARTIAL_KEY]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            // ⚠ ABSENT is `false` — *the numbering still holds* — which is the reading that makes a
            // reader carry on rather than restart. A `true` invented here would throw away a cursor
            // that was perfectly good and re-deliver everything the pane retains.
            restarted: answer[LINES_RESTARTED_KEY].as_bool().unwrap_or(false),
        })
    }
}

/// **THE LEADER OF THE JOB THAT OWNS A PANE'S TERMINAL, READ OVER THE SOCKET** — register item 557.
impl PaneForegroundJob for RemotePaneAccess {
    /// The foreground leader, or [`None`] when nothing owns the terminal (its child exited, or the
    /// leader was reaped while its group lives on) and when the host has no process table at all.
    ///
    /// ⚠⚠ Those two absences are ONE `None` here, and that is the trait's own shape rather than a
    /// loss this seam introduces — the in-process reader collapses them identically, because
    /// `foreground_leader_of` does. What a caller CAN tell apart is *no leader* from *no surface*,
    /// which is why [`foreground_job`](PaneAccess::foreground_job) answering `Some` matters:
    /// `PaneDoing::Nothing` and `PaneDoing::Unknown` are built from exactly that distinction.
    fn pane_foreground_leader(&self, id: PaneId) -> Option<JobProcess> {
        serde_json::from_value(self.read_pane(id, PANE_FOREGROUND_SLOT)?).ok()
    }
}

/// **WHO HAS WRITTEN INTO A PANE, READ OVER THE SOCKET** — register item 653.
impl PaneHands for RemotePaneAccess {
    /// The counts, or [`None`] for a pane this daemon does not hold.
    ///
    /// ⚠⚠⚠ **AND `None` FOR AN ANSWER THIS BUILD CANNOT READ, WHICH IS NOT THE SAME AS ZERO.**
    /// [`hands_of`] refuses a value that is not the object it expects rather than defaulting its
    /// keys, because a fabricated zero is the sentence *nobody has written here* — the one reading
    /// this whole address exists to stop a driver reaching by accident.
    fn pane_hands(&self, id: PaneId) -> Option<Hands> {
        hands_of(&self.read_pane(id, PANE_HANDS_SLOT)?)
    }
}

/// **THE KERNEL'S TWO ANSWERS ABOUT A PANE'S TERMINAL, READ OVER THE SOCKET** — register item 557.
///
/// ⚠⚠⚠ Both are read at the moment of asking and neither is cached. They are the PROGRAM's to
/// change at any instant — every interactive agent takes its terminal raw on startup — so a value a
/// driver remembered from a previous step would be a statement about a mode that has since moved.
impl PaneTerminalModes for RemotePaneAccess {
    /// Who echoes this pane's input, or [`None`] where the mode could not be read.
    ///
    /// ⚠⚠ A word this build does not know reads as `None`, never as the other word:
    /// [`PaneEcho::from_wire`] refuses what it cannot spell, and a driver told *the program owns
    /// the screen* on no evidence would report a delivery confirmed by an echo it took for output.
    fn pane_echo(&self, id: PaneId) -> Option<PaneEcho> {
        PaneEcho::from_wire(self.read_pane(id, PANE_ECHO_SLOT)?.as_str()?)
    }

    /// Whether a `Ctrl-D` written here ends the program's input, or [`None`] where the mode could
    /// not be read — [`pane_echo`](Self::pane_echo)'s rule for an unknown word applies here too.
    fn pane_end_of_input(&self, id: PaneId) -> Option<PaneEndOfInput> {
        PaneEndOfInput::from_wire(self.read_pane(id, PANE_END_OF_INPUT_SLOT)?.as_str()?)
    }
}

/// **WHAT THE AGENT IN ONE PANE IS DOING, READ OVER THE SOCKET** — register item 557.
///
/// Reached only through [`PaneAccess::supervision`], so a caller holding one of these has already
/// been told this daemon CAN look. That is what makes the `None` below mean one thing.
impl PaneSupervision for RemotePaneAccess {
    /// The pane's verdict, or [`None`] for a pane no manifest claims.
    ///
    /// # ⚠⚠⚠ Read at the pane's OWN address, not found in the pane list
    ///
    /// The listing carries the same object, and taking it from there would mean fetching every
    /// pane's screen token, title and image summaries to read one verdict — on a path a run walks
    /// each step — and then FINDING this pane among them, which is a second way to supervise the
    /// wrong peer. The address answers about the pane it names or about nothing.
    ///
    /// ⚠⚠ The object is parsed by the reader that lives beside the writer
    /// ([`crate::agent::verdict_of`]), which is what keeps the two spellings of this verdict one
    /// edit apart rather than one round apart. A state word this build does not know reads as
    /// absent rather than as a guess — see that function.
    fn pane_agent_state(&self, id: PaneId) -> Option<AgentObservation> {
        // ⚠⚠⚠⚠⚠ **TAKEN BEFORE THE REQUEST GOES OUT** — register item 640. The settling deadline
        // crosses this wire as a REMAINING TIME, so it has to be anchored to a clock this process
        // owns, and the two candidate moments are not equivalent: anchored to the answer's ARRIVAL
        // the deadline is late by the whole round trip, and late is the direction a waiter parks
        // PAST the publish it is waiting for. Anchored here it is early by that much, which costs
        // one look and cannot lose a wakeup. `crate::agent::Sent` is the type that says which.
        let sent = crate::agent::Sent::now();
        let answer = self.read(&mux_action_path(&agent_slot_for(id.0)))?;
        match crate::agent::verdict_of(&answer, sent) {
            crate::agent::Verdict::Seen(seen) => Some(*seen),
            crate::agent::Verdict::NotAnAgent => None,
            // ⚠⚠⚠⚠⚠ A WORD THIS BUILD CANNOT SPELL TAKES THE WHOLE SURFACE DOWN, register item 564.
            // Answering `None` alone would say *this pane is a shell* about a pane running an agent
            // this driver has never heard of. The honest instruction is the one
            // `PaneAccess::supervision` answering `None` carries — **ask a person** — so the skew
            // latches and [`supervision`](PaneAccess::supervision) stops claiming to look.
            //
            // ⚠⚠ The caller that is mid-step gets this one `None` first, because the trait has no
            // error channel here. That is one step read as *not an agent*, which makes a supervisor
            // skip a pane rather than act wrongly on it; from the next step the answer is the
            // stronger and correct one. See the register's residue.
            crate::agent::Verdict::Unspellable(word) => {
                tracing::warn!(
                    target: "sprag_host",
                    %word,
                    pane = id.0,
                    "the daemon published an agent state this build cannot spell, so this driver \
                     can no longer claim to supervise — the two are different builds"
                );
                *lock(&self.unspellable) = Some(word);
                None
            }
        }
    }
}

/// **THE WORLD A DRIVER OUTSIDE THE DAEMON CHECKS A RUN AGAINST** — register items 544 and 643, and
/// [`PluginWorld`](crate::plugins::PluginWorld)'s second implementation.
///
/// # ⚠⚠⚠⚠⚠ Why this exists at all: one builder, two callers
///
/// A run's plugin is built from its request map, and the builder asks the world exactly two
/// questions — *does this pane exist* and *how big is a pane by default*. When the driver moves out
/// of the daemon it has to build **the same plugin from the same request**, and a second builder
/// over there would be a second answer to one question: it would drift first in whichever key one
/// of them forgot, which is the shape this repository has paid for at every surface it duplicated.
///
/// So the builder stayed ONE function and the world became an argument. This is what that argument
/// is when the caller is on the far side of a socket: both answers come off the wire the driver is
/// already holding.
///
/// ⚠⚠ **IT IS NOT A SECOND `PaneAccess`.** The surface beside it drives panes; this is consulted
/// once, before a run starts, to turn a mistyped id into a synchronous refusal instead of a run
/// that dies on its first step.
pub struct RemotePluginWorld<'a>(&'a RemotePaneAccess);

impl<'a> RemotePluginWorld<'a> {
    /// Check runs against the daemon `access` is driving.
    #[must_use]
    pub const fn over(access: &'a RemotePaneAccess) -> Self {
        Self(access)
    }
}

impl crate::plugins::PluginWorld for RemotePluginWorld<'_> {
    /// ⚠ THE PANE LIST, which is the same set the daemon's own pool answers from — and a surface
    /// whose daemon was replaced answers an EMPTY list, so a run against a stranger's pane is
    /// refused rather than started. That is [`RemotePaneAccess::world_changed`]'s latch reaching
    /// the one check that happens before any driving.
    fn has_pane(&self, pane: PaneId) -> bool {
        self.0.pane_ids().contains(&pane)
    }

    /// ⚠⚠ **THE DAEMON'S ARBITRATED SIZE, NOT THIS PROCESS'S TERMINAL.** A driver process has a
    /// terminal of its own and it is nobody's business: the rectangle a pane is opened at is the
    /// one every client of that session lays the arrangement out over
    /// ([`WINDOW_SIZE_SLOT`](crate::wire::WINDOW_SIZE_SLOT)).
    ///
    /// ⚠ `null` there is *no attached client has said how big it is*, and the fallback is the same
    /// 80×24 a daemon boots with — stated here rather than left to a caller, because a size chosen
    /// by two different processes for one pane is the reflow this address exists to prevent.
    fn default_size(&self) -> (u16, u16) {
        let dim = |value: &Value, key: &str| {
            value[key]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .filter(|n| *n > 0)
        };
        self.0
            .read(&mux_action_path(crate::wire::WINDOW_SIZE_SLOT))
            .and_then(|size| Some((dim(&size, "cols")?, dim(&size, "rows")?)))
            .unwrap_or((80, 24))
    }
}

#[cfg(test)]
mod tests {
    use super::scopes_that_disagree;

    /// ⛔⛔⛔⛔ **ONLY TWO ANSWERS THAT DISAGREE REFUSE A PARK CONNECTION** — register item 641, and
    /// the three absences its end-to-end gate cannot reach.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is here and not only there
    ///
    /// `a_park_connection_scoped_to_another_session_is_refused_where_it_is_handed_over` drives two
    /// real connections to a real daemon, which is the right shape for the DISAGREEMENT — and it
    /// is structurally blind to the rest, because **this build's daemon always answers**. *A daemon
    /// too old to serve the address* is not a state that fixture can stage, so the arm deciding
    /// whether every park against an older daemon is refused had no gate at all. That is the
    /// register's own recorded shape — a gate that lives only on the branch its fixture enters says
    /// less than it looks like it does (item 632).
    ///
    /// # ⚠⚠⚠ And the mutation the next round is tempted by
    ///
    /// `read != park` on the `Option`s directly — one character shorter, and it reads *an absence
    /// disagrees with a name*. Under it, a driver whose park connection reaches a daemon that
    /// cannot answer this address is refused, so item 631's whole repair is unreachable against
    /// exactly the builds a driver most often meets after a rebuild (item 412). It turns the third
    /// and fourth rows below red and nothing else in this workspace notices.
    #[test]
    fn only_two_session_answers_that_disagree_refuse_a_park() {
        assert_eq!(
            scopes_that_disagree(Some("0"), Some("work")),
            Some(("0", "work")),
            "⛔⛔⛔⛔⛔ TWO NAMES THAT DIFFER ARE THE REFUSAL, and both travel: a park on another \
             session is refused by the daemon at its first wait, and that refusal retires the park \
             connection — leaving the driver polling for the rest of its run",
        );
        assert_eq!(
            scopes_that_disagree(Some("work"), Some("work")),
            None,
            "⚠⚠⚠ THE CONTROL: agreement must be accepted, or the check refuses everything and \
             register item 631's repair is unreachable",
        );
        assert_eq!(
            scopes_that_disagree(None, Some("work")),
            None,
            "⛔⛔⛔⛔ A READ CONNECTION THAT CANNOT SAY IS NOT A DISAGREEMENT — *this build cannot \
             tell you* and *these are different* are two facts, and only one of them is a reason to \
             refuse somebody's park",
        );
        assert_eq!(
            scopes_that_disagree(Some("work"), None),
            None,
            "⛔⛔⛔⛔ ...and neither is a PARK connection that cannot say. An older daemon serves no \
             such address, and refusing there would refuse every park against exactly the builds a \
             driver meets after a rebuild",
        );
        assert_eq!(
            scopes_that_disagree(None, None),
            None,
            "⚠⚠ and two silences agree about nothing at all, which is still not a disagreement",
        );
    }
}
