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
//! # ⚠⚠⚠⚠ What a remote surface DOES NOT answer, and why each absence is safe
//!
//! Every optional sub-surface of [`PaneAccess`] is [`None`] here, which each one's own
//! documentation defines: a host that cannot supervise says so rather than reporting *"not an
//! agent"*, a host that cannot tell echo from output makes its consumer degrade in the safe
//! direction, and a host with no job control must report that it could not stop the work rather
//! than write `0x03` and hope. **They are `None` because this build cannot ask those questions over
//! the wire yet, not because a remote driver does not want them** — the addresses are the next
//! stages of item 544, and every one of them has a consumer that already handles the absence.
//!
//! ⚠⚠ The one that is NOT merely absent is the PAINT question. [`PaneRow::generation`] is a damage
//! generation, deliberately unpublished (a resize or a palette change stamps every row while no
//! program writes a byte, which is a mistake four plugins in this workspace have already made). So
//! the rows this surface serves carry the TEXT — the content question, which is what
//! [`RowTrail`](sprag_plugin::RowTrail) asks — and a `generation` of zero, which no reader here
//! consults. See the register for the residue.

use std::io;
use std::sync::Mutex;

use serde_json::{Value, json};
use sprag_plugin::{KeyStroke, PaneAccess, PaneError, PaneRow, Written};
use sprag_rpc::{CallError, HostConn, NO_EXTERNAL_FAULT};
use sprag_terminal::PaneId;

use crate::external::lock;
use crate::wire::{
    ALT_FIELD, CTRL_FIELD, FULL_LINES_SLOT, FULL_TEXT_SLOT, INJECT_ACTION, INJECT_STROKES_KEY,
    INJECTED_BYTES_KEY, KEY_FIELD, PANE_EOF_SLOT, PANE_SUMMARY_ID_KEY, PANES_SLOT,
    PEER_GONE_REFUSAL, SCREEN_COLLAPSED_SLOT, SCREEN_ROWS_SLOT, SHIFT_FIELD, SUPER_FIELD,
    mux_action_path, pane_input_path, refusal, unknown_action, unknown_slot,
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
/// rather than a workaround: a driver's step is bounded by an AI turn — seconds to minutes — and a
/// round trip is sub-millisecond, so serialising them costs nothing measurable and buys the one
/// property a shared connection needs, which is that two answers cannot interleave on the wire.
///
/// ⚠ It is NOT a claim that two DRIVERS may share one of these. Nothing here stops that and nothing
/// here makes it safe: the pane a run is driving is guarded by whose run it is, one surface up.
pub struct RemotePaneAccess {
    conn: Mutex<HostConn>,
}

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
        // ⚠⚠⚠⚠⚠ THE ANSWER IS TAKEN AS A VALUE AND THE LOCK IS GONE BEFORE IT IS EXAMINED. A
        // `match lock(..).try_call(..)` holds the guard for the whole match — the scrutinee's
        // temporaries live that long — so any arm that came to ask this surface a second question
        // would deadlock, and only on the path that has something to say. This workspace has
        // measured that exact shape twice (a format argument that re-locked, evaluated only when
        // the assertion failed: green for as long as it passed, a 93-minute hang the moment it
        // did not). Structure, not vigilance.
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
                None
            }
        }
    }

    /// Read one address of one pane.
    fn read_pane(&self, id: PaneId, slot: &str) -> Option<Value> {
        self.read(&pane_input_path(id.0, slot))
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
    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<Written, PaneError> {
        let path = pane_input_path(id.0, INJECT_ACTION);
        let strokes: Vec<Value> = keys.iter().map(stroke_form).collect();
        let args = json!({ PATH_PARAM: path, ARGS_PARAM: { INJECT_STROKES_KEY: strokes } });
        // ⚠⚠ THE ANSWER IS A VALUE BEFORE IT IS EXAMINED, for `read`'s reason: a failure mapping
        // that came to ask this surface a second question would deadlock under a guard still held
        // by the scrutinee, and only ever on the path that has something to report. A draft of
        // this one did exactly that — it read the pane list to tell a gone pane from a skew.
        let outcome = lock(&self.conn).try_call(INVOKE_METHOD, args);
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
