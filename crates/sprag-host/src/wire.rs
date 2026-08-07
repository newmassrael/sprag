//! The wire ABI vocabulary — the ONE definition of the JSON-RPC address grammar
//! and action names the host serves and a client addresses.
//!
//! pinion's `scene/invoke` / `scene/query` address a node as
//! `/{container_tag}/{external_tag}/external/{action}`; sprag owns the tags
//! ([`INPUT_TAG`] / [`MUX_TAG`]) and the action
//! names. Before this module those were transcribed in three places — the host's
//! own externals ([`SpragPaneExternal`](crate::SpragPaneExternal) /
//! [`WorkspaceExternal`](crate::WorkspaceExternal) schema + dispatch), the host's
//! tests, and the wire client (`sprag-gui`'s `WireHost`) — so a path or action
//! rename could silently desync a client. This module is the single home: the host
//! externals match on these consts, and the client builds its request paths from the
//! same builders, so the ABI is defined once.

use std::io;

use pinion_core::external::{SchemaArg, SchemaField};
use serde_json::{Map, Value};
use sprag_rpc::RpcFault;
use sprag_terminal::{OrderStep, PaneDir, PaneId, PlaceHow, SessionId, WindowId, WindowPlace};

use crate::{INPUT_TAG, MUX_TAG};

/// The pane-input external invoke action that injects a key (W3C key + mods →
/// PTY bytes, the R2.6 encoder).
pub const KEY_ACTION: &str = "key";
/// The pane-input external invoke action that reports a MOUSE event — a semantic pointer edge
/// (`{button, kind, col, row, ctrl?, alt?, shift?}`) which the host gates against the pane's active
/// mouse-tracking mode and, if wanted, encodes to an X10 / SGR report at the PTY boundary (the same
/// mode-authority-at-the-boundary discipline as [`PASTE_ACTION`]). A display client sends the raw
/// cell + button; it never encodes the report itself.
pub const MOUSE_ACTION: &str = "mouse";
/// The pane-input external invoke action that reports a pane FOCUS change (`{focused: bool}`): the
/// host sends `ESC [ I` / `ESC [ O` to the child when it has enabled focus reporting (DEC private
/// mode 1004), a no-op otherwise. Same mode-authority-at-the-boundary discipline as [`MOUSE_ACTION`]
/// — a display client reports the edge, the host gates + encodes.
pub const FOCUS_ACTION: &str = "focus";
/// The pane-input external invoke action that writes literal UTF-8 (IME commit /
/// paste), no key-encoding.
pub const TEXT_ACTION: &str = "text";
/// The pane-input external invoke action that PASTES literal UTF-8: like [`TEXT_ACTION`], but the
/// host wraps it in the bracketed-paste markers (`ESC [ 200 ~` … `ESC [ 201 ~`) when the pane's
/// child has enabled DEC private mode 2004, and filters any embedded end marker so the paste
/// cannot break out of the bracket. Distinct from [`TEXT_ACTION`] because only a paste is
/// bracketed — typed / IME-committed text never is. The bracketing decision lives at the PTY
/// boundary (which holds the authoritative mode), so a display client just forwards the raw text.
pub const PASTE_ACTION: &str = "paste";
/// The pane-input external query slot: how many distinct frames [`CELLS_FIELD`] can address
/// — `scrollback_len + 1` (the live view, plus one per retained history line).
///
/// It exists to make [`CELLS_FIELD`]'s argument able to state its bound. A count, not a
/// length, and the
/// `+ 1` is the whole point: offsets run `0..=scrollback_len` INCLUSIVE (`0` is live,
/// `scrollback_len` is fully scrolled back, both answerable), so the EXCLUSIVE `0..frames`
/// that [`ArgDomain::IndexOf`](pinion_core::external::ArgDomain::IndexOf) means is exactly
/// right against `scrollback_len + 1` and would have been false against the length.
pub const FRAMES_SLOT: &str = "frames";

/// The arguments of [`CELLS_FIELD`] — one scrollback `offset`, bounded by [`FRAMES_SLOT`].
///
/// **`IndexOf`, not `Open`, and R155's review is why.** The first draft declared
/// [`ArgDomain::Open`](pinion_core::external::ArgDomain::Open) with a confident paragraph
/// arguing the bound was "real but not expressible", citing pinion's `datepicker` as the
/// same shape. **pinion's datepicker comment refutes that analogy in its own words**: its
/// `state.<day>` is inexpressible because it is ONE-BASED (`1..=days`, so `IndexOf` "would be
/// false at BOTH ends"), and it *publishes* `days` regardless. This offset is ZERO-based, so
/// `0..=scrollback_len` is plainly `0..(scrollback_len + 1)` — an `IndexOf` against a count
/// this surface simply had to publish. `Open` was not a bound we could not state; it was a
/// count we had not exposed, and pinion's own doc calls an unearned `Open` "an affirmative
/// false statement … worse than the pre-R1353 silence it replaced, because now it carries a
/// schema's authority".
///
/// An out-of-range offset still CLAMPS rather than erroring (`project_scrolled`), which
/// `IndexOf` does not promise away — pinion declares `width.<col>` the same way over a
/// clamping reader. The domain says which offsets are MEANINGFUL, so an agent reads one
/// scalar instead of fetching whole cell grids to discover where history ends.
const CELLS_ARGS: &[SchemaArg] = &[SchemaArg::index("offset", FRAMES_SLOT)];

/// The pane-input external query FAMILY: the pane's cell FRAME
/// ([`CellFrame`](crate::CellFrame)) at scrollback `offset` — `cells.<offset>`, where
/// `cells.0` is the live view a display client projects each frame and a larger offset
/// windows into history.
///
/// **This declaration is the ONE definition of the family**, and everything else derives
/// from it rather than re-spelling it: the host's schema publishes it, the host's `query`
/// strips [`SchemaField::literal_prefix`] off an arriving path, and both ends build an
/// address through [`cells_slot_at`]. So the template an agent discovers in `$schema`, the
/// prefix the host matches, and the path a client sends cannot drift apart —
/// `the_cells_family_declares_the_paths_it_answers` holds them to it.
///
/// ## Why ONE name, and why a query
///
/// This was TWO wire names until PR-61 landed (a `frame` query for the live view and a
/// `cells` invoke for history), and the reason was a mistake worth keeping recorded. A
/// display client re-reads the live frame on every scene-revision move, an invoke is a
/// `MethodOcc::Mutate` that bumps the revision, so serving the live frame through an action
/// woke the very waiter it answered — a ~30Hz idle livelock that burned a full core (R152).
/// The livelock was real and the query fixed it; the SPLIT was the part that was wrong.
/// sprag split the concept because it read `query(&self, path: &str)`'s argument-free
/// signature as "a parameterized read is impossible" — and PR-61's answer (pinion R1352) was
/// that it never was: **the argument rides the path**, as `width.<col>` and `id_at.<pos>`
/// already did upstream. What was genuinely missing was the ability to SAY so, which R1353
/// then delivered ([`SchemaField::parametric`]) — so the family is now discoverable rather
/// than conventional.
///
/// Collapsing the names is what retires the split's last defect. A scrollback read was still
/// an invoke, so it still bumped: one client's wheel tick woke every OTHER attached client's
/// parked `waitFor` into a full re-fetch. Bounded and terminating, so never the livelock —
/// but the same defect, and one door to one concept is what removes it.
pub const CELLS_FIELD: SchemaField = SchemaField::parametric("cells.<offset>", "frame", CELLS_ARGS);
/// The pane-input external query slot: the pane's full output text (scrollback +
/// visible).
pub const FULL_TEXT_SLOT: &str = "full_text";
/// The pane-input external query slot: the producer's DECCKM (application cursor
/// keys) mode.
pub const CURSOR_KEYS_SLOT: &str = "application_cursor_keys";
/// The pane-input external query slot: the LAST shell command sliced from the pane's OSC 133
/// marks — a JSON object `{command, output, exit_status, running}`
/// ([`Screen::last_command`](sprag_vt::Screen::last_command)), or `null` when no command has run
/// under shell integration (the agent then falls back to [`FULL_TEXT_SLOT`]). The command-scoped
/// read tmux's whole-pane `capture-pane` cannot express.
pub const LAST_COMMAND_SLOT: &str = "last_command";
/// The pane-input external query slot: the OSC 133 prompt-mark positions
/// ([`Screen::prompt_positions`](sprag_vt::Screen::prompt_positions)) — a JSON array of logical
/// line indices (from the oldest retained line, the scroll `offset_y` unit) a display client's
/// jump-to-prompt scrolls to. Read ON DEMAND (a keyboard jump), never per frame.
pub const PROMPT_MARKS_SLOT: &str = "prompt_marks";
/// The pane-input external query slot: the OSC-8 hyperlink runs on the visible grid
/// ([`Screen::hyperlink_runs`](sprag_vt::Screen::hyperlink_runs)) — a JSON array of
/// `{text, uri, id}`, one per contiguous link run, or `[]` when the pane shows no links. An agent
/// reads a link's DESTINATION as data (the URI, without OCR), which tmux's `capture-pane` cannot
/// expose because it flattens OSC 8 to plain text. Read ON DEMAND (a `read_pane_links` call).
pub const LINKS_SLOT: &str = "links";
/// The pane-input external query slot: the pane's most recent OSC 52 clipboard WRITE — a JSON
/// object `{targets:{clipboard,primary}, text, seq}`, or `null` when the child has written none.
/// Fetched ON DEMAND when the `clipboard_write_seq` in the pane list grows (the payload can be a
/// whole paste, so it is not carried per poll). A client applies it — subject to its clipboard
/// policy — to its own system clipboard.
pub const CLIPBOARD_WRITE_SLOT: &str = "clipboard_write";
/// The pane-input external action: ANSWER a pending OSC 52 read query. Params `{seq, sel, text}`
/// — the query `seq` a display client saw in the pane list, the selection char (`c`/`p`) it read,
/// and that selection's current text. The host writes the `OSC 52` reply back to the PTY, admitting
/// EXACTLY ONE reply per query across all attached clients; the answer reports `{wrote}`.
pub const CLIPBOARD_ANSWER_ACTION: &str = "clipboard_answer";

/// The arguments of [`IMAGE_DATA_FIELD`] — one inline image `id`, `Open`. Unlike
/// [`CELLS_FIELD`]'s bounded `IndexOf`, a Kitty image id is a PRODUCER-CHOSEN key, not sprag's
/// index into a count, so there is no count to publish; the ids are the open set the child
/// transmitted (enumerated in the panes-slot `images` summary). `Open` is earned here, not a
/// default hiding a count.
const IMAGE_DATA_ARGS: &[SchemaArg] = &[SchemaArg::open("id", "int")];
/// The pane-input external query FAMILY: one inline image's RGBA as base64 (`image_data.<id>`,
/// R1404 Stage 5). Fetched ON DEMAND when a display client sees a NEW / CHANGED image in the
/// panes-slot `images` summary (keyed on `{id, seq}`) — the RGBA is up to
/// [`MAX_IMAGE_BYTES`](sprag_vt) and must NOT ride the per-poll panes slot, the
/// [`FULL_TEXT_SLOT`] / [`CLIPBOARD_WRITE_SLOT`] on-demand precedent. `Null` for an id the pane
/// is not currently showing.
pub const IMAGE_DATA_FIELD: SchemaField =
    SchemaField::parametric("image_data.<id>", "string", IMAGE_DATA_ARGS);

/// The arguments of [`FIND_FIELD`] — one search `needle`, `Open`.
///
/// `Open` like [`IMAGE_DATA_ARGS`] and for the same earned reason, not as a default that hides a
/// count: a needle is a string the CALLER invents, so there is no domain to enumerate and no bound
/// to publish. (`IndexOf` would be a lie about a set that does not exist.)
const FIND_ARGS: &[SchemaArg] = &[SchemaArg::open("needle", "string")];

/// The pane-input external query FAMILY: every literal match of `needle` in the pane's retained
/// output — `find.<needle>` — as `{matches: [{line, col, cols}], truncated}`.
///
/// **The needle rides the path VERBATIM, and that is exact rather than lax.** pinion hands an
/// External everything after the first `/external/` untouched
/// ([`split_at_external`](pinion_rpc::path::split_at_external)), so a needle may contain `.`, `/`,
/// a space, or any UTF-8 without escaping — and because nothing is encoded, nothing has a second
/// spelling: one needle, one address. That is the same property `cells.<offset>` had to REJECT
/// aliases to earn (`cells.007`), obtained here by not encoding at all.
///
/// A READ, not an action, which is the whole point of PR-61's lesson: a find bar re-queries on every
/// keystroke, and an invoke would put a bump on that path — one client's typing waking every other
/// attached client's parked `waitFor`. Searching a pane changes nothing about it.
///
/// The answer is in the pane's LOGICAL coordinate ([`FindMatch`](sprag_vt::FindMatch)): `line`
/// counts from the oldest retained line — the [`PROMPT_MARKS_SLOT`] axis, so a client jumps to a
/// match with the scroll `offset_y` it already speaks — and `col`/`cols` are CELL columns, ready to
/// overlay. An EMPTY needle is a malformed member and answers `Null`, the same shape a malformed
/// `cells.<offset>` reports.
pub const FIND_FIELD: SchemaField = SchemaField::parametric("find.<needle>", "object", FIND_ARGS);

/// The arguments of [`REGEX_FIELD`] — one search `pattern`, `Open`, for the same reason
/// [`FIND_ARGS`] is: a pattern is a string the CALLER invents, so there is no domain to enumerate.
const REGEX_ARGS: &[SchemaArg] = &[SchemaArg::open("pattern", "string")];

/// The pane-input external query FAMILY for a REGULAR-EXPRESSION search — `regex.<pattern>` —
/// answering the same `{matches, lines, truncated}` shape as [`FIND_FIELD`], plus an `error` when
/// the engine refused the pattern.
///
/// **A separate ADDRESS, not a flag on `find`, because a needle and a pattern are separate
/// LANGUAGES.** `find.a.b` and `regex.a.b` carry the same three characters and mean different
/// things — three literal characters versus "a, anything, b". Keeping them on one address with a
/// mode argument would make what a stored or in-flight search MEANS depend on something other than
/// the address, which is exactly the aliasing `cells.<offset>` had to reject. One address, one
/// language, one answer.
///
/// Case follows from the same principle: [`FIND_FIELD`] folds ASCII case (a literal search is a
/// convenience), while this is case-SENSITIVE because the pattern language owns that decision
/// through `(?i)`.
///
/// A READ like every other query here, so a find bar can re-query per keystroke without waking the
/// waiters it is parked beside. An EMPTY pattern is a malformed member and answers `Null`, matching
/// [`FIND_FIELD`]'s taxonomy; an INVALID pattern is NOT — it is a well-formed address whose value
/// the engine rejected, so it answers the normal shape carrying the engine's message, which `Null`
/// could not distinguish from "this pane does not exist".
pub const REGEX_FIELD: SchemaField =
    SchemaField::parametric("regex.<pattern>", "object", REGEX_ARGS);

/// The pane-input external's DECLARED SCHEMA — every path it answers, with its type and any
/// arguments, in `$schema` order. [`SpragPaneExternal`](crate::SpragPaneExternal) publishes
/// this verbatim.
///
/// The declarations live HERE, beside the addresses, because this module claims to be "the
/// ONE definition of the … grammar and action names" and a field's TYPE is part of its
/// declaration. R155's review caught the module owning `CELLS_FIELD` whole while the other
/// four fields kept their names here and their types at the use site — one concept, two
/// homes, split by nothing more principled than which field happened to be parametric.
///
/// Each entry reuses its address const rather than re-spelling it, so a field's path and the
/// path a client builds are the same string by construction.
pub const PANE_SCHEMA: &[SchemaField] = &[
    SchemaField::new(KEY_ACTION, "action"),
    SchemaField::new(MOUSE_ACTION, "action"),
    SchemaField::new(FOCUS_ACTION, "action"),
    SchemaField::new(TEXT_ACTION, "action"),
    SchemaField::new(PASTE_ACTION, "action"),
    CELLS_FIELD,
    SchemaField::new(FRAMES_SLOT, "int"),
    SchemaField::new(CURSOR_KEYS_SLOT, "bool"),
    SchemaField::new(FULL_TEXT_SLOT, "string"),
    SchemaField::new(LAST_COMMAND_SLOT, "object"),
    SchemaField::new(PROMPT_MARKS_SLOT, "array"),
    SchemaField::new(LINKS_SLOT, "array"),
    FIND_FIELD,
    REGEX_FIELD,
    IMAGE_DATA_FIELD,
    SchemaField::new(CLIPBOARD_WRITE_SLOT, "object"),
    SchemaField::new(CLIPBOARD_ANSWER_ACTION, "action"),
];

/// Every address the MULTIPLEXER surface serves — the actions a client invokes and the slots it
/// queries, in the order a reader of `show-options`-style output would want them: the verbs, then
/// the facts.
///
/// Declared HERE for [`PANE_SCHEMA`]'s reason, one surface along: this module claims to be the ONE
/// definition of the wire's grammar, and a schema that lived at its use site would be a second copy
/// of exactly the vocabulary this module exists to hold. It moved here when the surface acquired a
/// RATCHET (`the_wire_surface_cannot_move_under_the_protocol_number`), which needs both schemas
/// readable from one place without constructing a daemon.
pub const MUX_SCHEMA: &[SchemaField] = &[
    SchemaField::new(SPAWN_ACTION, "action"),
    SchemaField::new(SPLIT_ACTION, "action"),
    SchemaField::new(CLOSE_ACTION, "action"),
    SchemaField::new(RESIZE_ACTION, "action"),
    SchemaField::new(RENAME_PANE_ACTION, "action"),
    SchemaField::new(SET_LAYOUT_ACTION, "action"),
    SchemaField::new(SET_FLOATING_ACTION, "action"),
    SchemaField::new(NEW_SESSION_ACTION, "action"),
    SchemaField::new(KILL_SESSION_ACTION, "action"),
    SchemaField::new(NEW_WINDOW_ACTION, "action"),
    SchemaField::new(SELECT_WINDOW_ACTION, "action"),
    SchemaField::new(MOVE_WINDOW_ACTION, "action"),
    SchemaField::new(SELECT_PANE_ACTION, "action"),
    SchemaField::new(RENAME_WINDOW_ACTION, "action"),
    SchemaField::new(RENAME_SESSION_ACTION, "action"),
    SchemaField::new(DISPLAY_MESSAGE_ACTION, "action"),
    SchemaField::new(KILL_WINDOW_ACTION, "action"),
    SchemaField::new(RESIZE_WINDOW_ACTION, "action"),
    SchemaField::new(BREAK_PANE_ACTION, "action"),
    SchemaField::new(JOIN_PANE_ACTION, "action"),
    SchemaField::new(MOVE_PANE_ACTION, "action"),
    SchemaField::new(SWAP_PANE_ACTION, "action"),
    SchemaField::new(RESIZE_PANE_ACTION, "action"),
    SchemaField::new(ZOOM_PANE_ACTION, "action"),
    SchemaField::new(DROP_FILE_ACTION, "action"),
    SchemaField::new(PANES_SLOT, "list"),
    SchemaField::new(LAYOUT_SLOT, "tree"),
    SchemaField::new(SESSIONS_SLOT, "list"),
    SchemaField::new(TREE_SLOT, "list"),
    SchemaField::new(SESSION_SLOT, "string"),
    SchemaField::new(CLIENTS_SLOT, "list"),
    SchemaField::new(GRID_WORK_SLOT, "object"),
    SchemaField::new(WINDOWS_SLOT, "list"),
    SchemaField::new(WINDOW_SIZE_SLOT, "object"),
    SchemaField::new(GLOBAL_COMMANDS_SLOT, "object"),
    SchemaField::new(AGENT_MANIFESTS_SLOT, "object"),
    PROJECT_FIELD,
    NEIGHBORS_FIELD,
    EVENTS_FIELD,
    SESSION_ACTIVITY_FIELD,
    PANE_PROCESSES_FIELD,
];

/// The out-of-band request param naming the SESSION a request acts on — `{"session": "work"}`
/// alongside `path` / `args`, never part of the address.
///
/// ## Why a param and not connection identity
///
/// One daemon holds every session, so a request must say which one it is about. The obvious
/// shape — "the host learns who is asking from the connection" — is not available and should
/// not be: pinion's `RpcFrame` carries a request string and a reply sink and no identity, so
/// teaching the funnel about connections would be an upstream ask. It is also the wrong
/// model. Which session a client is attached to is the CLIENT's state; it knows it, and
/// sending it makes each request self-describing and the host free of per-connection
/// bookkeeping. tmux's `switch-client` then costs nothing here — a client switches by sending
/// a different name.
///
/// ## The contract, copied from pinion's own precedent
///
/// pinion carries the same shape for its display windows (`Request::window_scope`, R890.1
/// pinion §5.49 — a different concept that merely shares the word "window"), and sprag mirrors it
/// deliberately rather than inventing a second convention for one idea:
///
/// * **absent** → the default scope ([`SessionRegistry::default_session`](sprag_terminal::SessionRegistry::default_session));
/// * **a string** → the session of that name;
/// * **present but not a string** → `-32602`, before method routing, never a silent fallback
///   to the default. pinion's doc records why in its own scars: silently dropping a
///   malformed scope made the request act on the primary — "wrong target for writes, wrong
///   data for reads" — and that survived an entire campaign to kill aliasing precisely
///   because it hid in the type-error corner;
/// * **a name no session carries** → refused wholesale, for the same reason.
///
/// The string itself is defined by the transport client that WRITES it
/// ([`sprag_rpc::SESSION_PARAM`]) and re-exported here for the host that READS it, so the two
/// ends of the wire share ONE spelling. This doc is the reader's contract; the writer's is on
/// the definition.
pub use sprag_rpc::SESSION_PARAM;

/// The OTHER scope key ([`sprag_rpc::ATTACHED_PARAM`]) — `{"attached": true}`, asking for the
/// session the calling connection's client is VIEWING rather than one by name.
///
/// Re-exported here for the same reason [`SESSION_PARAM`] is, and read through the same grammar
/// ([`sprag_rpc::ScopeAsk`]) rather than key by key, so the two keys' interaction — they are
/// mutually exclusive, and an empty one is an ABSENT one — is decided in exactly one place.
/// [`crate::ScopeError`] is the reader's contract for what each refusal means.
pub use sprag_rpc::ATTACHED_PARAM;

/// The client-lifecycle wire vocabulary (R-PR67 Stage 1), re-exported from the transport client
/// that WRITES it ([`sprag_rpc`]) so the host that READS it shares ONE spelling — exactly as
/// [`SESSION_PARAM`] is. [`CLIENT_HELLO_METHOD`] announces a connection's client id
/// ([`CLIENT_PARAM`]); [`CLIENT_ATTACH_METHOD`] declares/switches that client's attached session
/// (reusing [`SESSION_PARAM`]); [`CLIENT_SIZE_METHOD`] reports the cell area that client can give a
/// window ([`COLS_PARAM`] / [`ROWS_PARAM`]), which is the input tmux's `window-size` arbitrates
/// over; [`CLIENT_MESSAGES_METHOD`] collects whatever the daemon is holding to SAY to that client
/// ([`MESSAGE_FIELD`], R317). All four are intercepted before the generic dispatch core, since they
/// act on the frame's connection id, which no scene external sees. The reader's contract lives in
/// [`crate::rpc`] (the dispatch owner's client-lifecycle intercept); the writer's is on each
/// `sprag_rpc` const.
pub use sprag_rpc::{
    CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_MESSAGES_METHOD, CLIENT_PARAM,
    CLIENT_SIZE_METHOD, COLS_PARAM, MESSAGE_FIELD, ROWS_PARAM,
};

/// The [`CLIENT_ATTACH_METHOD`] `params` key asking to be moved to the session this client was
/// viewing BEFORE this one — `{"last": true}`, tmux `switch-client -l`. What it is FOR is on
/// [`AttachAsk::LastViewed`].
///
/// It is an ATTACH key, not a scope key: it says where the client is going, where [`SESSION_PARAM`]
/// and [`ATTACHED_PARAM`] say which session a request is about. They can appear together on one
/// attach without ambiguity for that reason — the scope is the target only when nothing else names
/// one.
pub const LAST_PARAM: &str = "last";

/// The [`CLIENT_ATTACH_METHOD`] `params` key narrowing [`LAST_PARAM`] to a session NO OTHER client
/// is viewing — tmux `detach-on-destroy no-detached`'s "most recently used detached session".
/// Meaningless on its own, and refused there ([`AttachFault::UnattachedWithoutLast`]).
pub const UNATTACHED_PARAM: &str = "unattached";

/// The [`CLIENT_ATTACH_METHOD`] `params` key asking for the session one STEP along the daemon's
/// order from the one this client is on — `{"step": "next"}` / `{"step": "previous"}`, tmux
/// `switch-client -n` / `-p` (R314). What it is FOR is on [`AttachAsk::Step`].
///
/// Its two words are [`OrderStep`]'s own, read with [`OrderStep::from_wire`] — the same pair
/// `select_window`'s [`SelectWindowAsk::Step`] takes one level down, because it is the same
/// direction along a different order. A third spelling of "next" cannot appear in one of them alone.
pub const STEP_PARAM: &str = "step";

/// The [`CLIENT_ATTACH_METHOD`] `params` key carrying a CHOOSER's pick — a path of IDENTITIES down
/// the tree [`TREE_SLOT`] published: `{"goto": {"session": 3, "window": 7, "pane": 2}}` (R315).
/// What it is FOR is on [`AttachAsk::Goto`].
pub const GOTO_PARAM: &str = "goto";

/// The [`GOTO_PARAM`] member naming the picked SESSION — a [`sprag_terminal::SessionId`], and the
/// one member that is not optional. A goto with no session names no target at all.
pub const GOTO_SESSION_PARAM: &str = "session";

/// The [`GOTO_PARAM`] member naming the picked WINDOW — a [`sprag_terminal::WindowId`]. Absent when
/// a SESSION row was picked, which means *wherever that session is currently looking*.
pub const GOTO_WINDOW_PARAM: &str = "window";

/// The [`GOTO_PARAM`] member naming the picked PANE — a [`sprag_terminal::PaneId`]. Absent unless a
/// PANE row was picked, and refused without a window ([`AttachFault::GotoPaneWithoutWindow`]): a
/// pane id is registry-unique, but a pick that did not say which window it came from is a path this
/// grammar cannot check WHOLE, which is the one thing it exists to do.
pub const GOTO_PANE_PARAM: &str = "pane";

/// WHICH session a [`CLIENT_ATTACH_METHOD`] moves its client to, as the request ASKS for it —
/// defined ONCE for both ends of the wire, beside every other ask grammar in this module.
///
/// # Why it lives here and [`sprag_rpc::ScopeAsk`] does not
///
/// It sat in `sprag-rpc` beside the scope until R314, on the stated reason that both are wire
/// grammars. That reason is real for the SCOPE — [`sprag_rpc::HostConn`] holds a `ScopeAsk` field
/// and merges it into every request it sends, so the transport crate genuinely uses it — and it was
/// never true of this one, which that crate only DEFINED. The cost showed up the moment a target
/// needed a TYPE: `sprag-rpc` is seam-only by construction and cannot see [`OrderStep`], so an
/// attach step would have had to spell its own two words. One vocabulary, so it moved to where the
/// vocabulary is.
///
/// The three arms are three ways of naming a target and only one of them is a name:
///
/// * [`Scoped`](Self::Scoped) — the session this connection is SCOPED to ([`sprag_rpc::ScopeAsk`]).
///   Every attach before R304 meant this, and it is still what a client sends to go somewhere it
///   can name: `scope_to(name)` then attach.
/// * [`LastViewed`](Self::LastViewed) — *the session I was viewing before this one*, tmux
///   `switch-client -l`. The caller cannot name it, because the only honest answer is held by the
///   daemon: see the arm's own doc.
/// * [`Step`](Self::Step) — *the next one along*, tmux `switch-client -n` / `-p`. The caller could
///   name it and must not: see the arm's own doc.
/// * [`Goto`](Self::Goto) — *the row I just picked*, a path of IDENTITIES. The caller could name it
///   and must not, for a reason one step stronger than the step's: see the arm's own doc.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachAsk {
    /// No attach key at all ⇒ attach to whatever this connection is scoped to.
    #[default]
    Scoped,
    /// `{"last": true}` ⇒ the most recent OTHER session this client has viewed that is still live.
    /// `{"last": true, "unattached": true}` narrows it to one no other client is viewing.
    ///
    /// # Why a client cannot ask this by name
    ///
    /// It could hold the name itself — and that is precisely the defect R304 measured. A visit
    /// history of NAMES is a set of addresses nobody maintains: after `rename-session` the entry
    /// resolves to nothing (the visit is silently lost), and once a NEW session takes the freed
    /// name it resolves to A STRANGER — so "take me back where I was" attaches the client to a
    /// session it has never seen, on the connection it types down.
    ///
    /// The daemon's copy is keyed by session IDENTITY, which is why it can be right: an id that
    /// resolves is that same session under whatever it is called now, and an id that does not is a
    /// session that is gone. The general rule, and the reason the ATTACHMENT does not need this
    /// while the history does: a fact about the PRESENT can be kept true by a hook where the change
    /// is published; a fact about the PAST cannot, because its subject may no longer exist to be
    /// updated.
    ///
    /// `unattached` is tmux `detach-on-destroy no-detached`'s "most recently used DETACHED session":
    /// it is answered from the daemon's own attachment map, exactly, where a client filtering its
    /// own session list reads a poll mirror that can be a beat behind.
    LastViewed {
        /// Skip a session another client is already viewing.
        unattached: bool,
    },
    /// `{"step": "next"}` / `{"step": "previous"}` ⇒ one step along the DAEMON's session order from
    /// the session this client is attached to, wrapping — tmux `switch-client -n` / `-p` (R314).
    ///
    /// # Why the client sends a direction and not a name
    ///
    /// Unlike [`LastViewed`](Self::LastViewed), a client COULD answer this itself: it polls the
    /// session list, so it could find its own row and take the next one. That is the second answer
    /// [`SelectWindowAsk::Step`] exists to prevent one level down, and the argument is the same
    /// — a mirror is a revision behind, so `switch-client -n` and `sprag ls` could disagree about
    /// what comes next, and the client would then attach BY NAME to a row that has since moved.
    ///
    /// It is also the only arm whose origin the client cannot state: the step is measured from the
    /// client's ATTACHMENT, which lives in the daemon's attachment map, not from the connection's
    /// scope. A connection that never attached steps from its scope instead — where a plain attach
    /// would have put it — so there is one rule and no refusal to write.
    ///
    /// **A one-session daemon answers that same session**, and that is not an error: the ring
    /// wrapped onto itself, which is what [`sprag_terminal::SessionRegistry::select_window_relative`]
    /// does one level down.
    Step(OrderStep),
    /// `{"goto": {"session": 3, "window": 7, "pane": 2}}` ⇒ the row a CHOOSER picked, as a path of
    /// IDENTITIES down the tree [`TREE_SLOT`] published (R315).
    ///
    /// # Why a pick cannot be a name, and cannot be a position either
    ///
    /// This is [`LastViewed`](Self::LastViewed)'s argument with the client on the other side of it.
    /// There the daemon holds the past; here the CLIENT does — a chooser paints a list and then
    /// waits for a person to read it, so what comes back is a fact that was true when it was drawn.
    /// R304's rule covers both: *a fact about the present can be kept true by a hook where the
    /// change is published; a fact about the past cannot.*
    ///
    /// * A picked NAME resolves to whatever holds it NOW. R304 measured that landing: a client went
    ///   back to a session it had never seen, because a new one had taken the freed name.
    /// * A picked POSITION resolves to whatever sits there now — R295's rule one level down, and
    ///   what the rival's chooser commits by at two of its three levels
    ///   (`NavigatorTarget::Workspace { ws_idx }`, herdr `9a4ce5e1`).
    /// * A picked IDENTITY resolves to that same thing under whatever it is called now, or to
    ///   NOTHING — and nothing is an answer, which is the whole difference. It is the one form that
    ///   can be REFUSED.
    ///
    /// # It is checked WHOLE before anything moves
    ///
    /// A path whose window has gone refuses the attach as well, rather than landing the client on
    /// the session and stopping. This grammar's own rule ([`AttachFault`]) is that a target that
    /// cannot be read must not fall back to one, and here the fallback would be a place the user did
    /// not pick.
    ///
    /// # What it changes for everybody else
    ///
    /// Attaching moves THIS client. Selecting a window or a pane moves the SESSION, which every
    /// other client viewing it sees — tmux's `choose-tree` has exactly this property, because
    /// `select-window` is a session verb there too. Stated rather than discovered.
    Goto {
        /// The picked session. Not optional: a goto names a target or it is not one.
        session: SessionId,
        /// The picked window, or [`None`] for a SESSION row — *wherever that session is looking*.
        window: Option<WindowId>,
        /// The picked pane, or [`None`]. Refused without a window.
        pane: Option<PaneId>,
    },
}

/// Why a params object does not name an attach target this grammar admits. Every arm refuses the
/// request WHOLE: an attach whose target cannot be read must not fall back to one, because the
/// fallback would be *the session the client is already on* — a switch that silently does nothing
/// and reports success.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachFault {
    /// [`LAST_PARAM`] is present and is not a boolean (`{"last": 1}`).
    LastNotABool,
    /// [`UNATTACHED_PARAM`] is present and is not a boolean.
    UnattachedNotABool,
    /// [`UNATTACHED_PARAM`] is present without [`LAST_PARAM`] — a filter with no subject. Refused
    /// rather than ignored: a caller that wrote it meant to narrow something, and quietly attaching
    /// it to the connection's scope would answer a question it did not ask.
    UnattachedWithoutLast,
    /// [`STEP_PARAM`] is present and is not a string (`{"step": 1}`).
    StepNotAString,
    /// [`STEP_PARAM`] is a string [`OrderStep::from_wire`] does not know (`{"step": "sideways"}`).
    StepUnknown,
    /// [`STEP_PARAM`] and [`LAST_PARAM`] are both asked for. TWO targets is no target: they name
    /// different sessions and nothing here may choose between them, so the request is refused
    /// rather than resolved by precedence — [`AttachFault`]'s own rule, and the one that keeps a
    /// caller from learning a silent ordering it would then depend on.
    TwoTargets,
    /// [`GOTO_PARAM`] is present and is not an object (`{"goto": 3}`).
    ///
    /// Refused rather than read as a bare session id, which is the shorthand that would be
    /// convenient and wrong: a path that can be spelled two ways is a path two decoders come to
    /// disagree about, and this grammar's whole job is to be checkable WHOLE.
    GotoNotAnObject,
    /// [`GOTO_PARAM`] carries no [`GOTO_SESSION_PARAM`]. A goto names a target or it is not one.
    GotoWithoutSession,
    /// A [`GOTO_PARAM`] member is present and is not a non-negative integer — carrying WHICH member,
    /// because "a goto id is malformed" is three sentences pretending to be one.
    GotoIdNotANumber(&'static str),
    /// [`GOTO_PANE_PARAM`] is present without [`GOTO_WINDOW_PARAM`]. See that key for why a pane id
    /// alone is refused even though it is registry-unique.
    GotoPaneWithoutWindow,
}

impl AttachAsk {
    /// Write this ask into an attach request's `params` map — the ONE place a client spells it.
    ///
    /// [`Scoped`](Self::Scoped) writes NOTHING, so the request every client sent before this
    /// grammar existed is unchanged byte for byte ([`sprag_rpc::ScopeAsk::write_into`]'s rule, and
    /// for the same reason). `unattached` is written only when it is asked for, so the commonest
    /// `switch-client -l` is `{"last": true}` and nothing else.
    pub fn write_into(&self, params: &mut Map<String, Value>) {
        match self {
            Self::Scoped => {}
            Self::LastViewed { unattached } => {
                params.insert(LAST_PARAM.to_owned(), Value::Bool(true));
                if *unattached {
                    params.insert(UNATTACHED_PARAM.to_owned(), Value::Bool(true));
                }
            }
            Self::Step(step) => {
                params.insert(
                    STEP_PARAM.to_owned(),
                    Value::String(step.wire_str().to_owned()),
                );
            }
            Self::Goto {
                session,
                window,
                pane,
            } => {
                let mut path = Map::new();
                path.insert(GOTO_SESSION_PARAM.to_owned(), Value::from(session.0));
                // Written only when the pick HAD one, so a session row's goto is
                // `{"session": N}` and nothing else — the same "omit what was not asked for" rule
                // `unattached` follows one arm up, and what keeps the three row kinds one grammar
                // instead of one grammar with two holes in it.
                if let Some(window) = window {
                    path.insert(GOTO_WINDOW_PARAM.to_owned(), Value::from(window.0));
                }
                if let Some(pane) = pane {
                    path.insert(GOTO_PANE_PARAM.to_owned(), Value::from(pane.0));
                }
                params.insert(GOTO_PARAM.to_owned(), Value::Object(path));
            }
        }
    }

    /// The target an attach request's `params` names — the ONE place these keys are read.
    ///
    /// `false` reads as ABSENT on both booleans, exactly as [`sprag_rpc::ScopeAsk`] reads
    /// `{"attached": false}`: a well-typed "no" says what omitting the key says, so a client that
    /// fills in a whole struct asks what one that omits it asks. Every other type is refused,
    /// including `null` — the divergence `ScopeAsk::parse` documents applies here for the stronger
    /// reason: an unreadable attach target that fell back to the connection's scope would be a
    /// `switch-client` that left the client exactly where it was and said it had moved.
    ///
    /// [`STEP_PARAM`] has no such "well-typed no": a step is a WORD, so absent is the only way to
    /// not ask for one, and every string that is not one of [`OrderStep`]'s two is a fault rather
    /// than a fallback.
    ///
    /// # Errors
    ///
    /// [`AttachFault`], one variant per way a target can be malformed.
    pub fn parse(params: Option<&Value>) -> Result<Self, AttachFault> {
        let flag = |key: &str, fault: AttachFault| match params.and_then(|params| params.get(key)) {
            None => Ok(false),
            Some(Value::Bool(asked)) => Ok(*asked),
            Some(_) => Err(fault),
        };
        let last = flag(LAST_PARAM, AttachFault::LastNotABool)?;
        let unattached = flag(UNATTACHED_PARAM, AttachFault::UnattachedNotABool)?;
        let step = match params.and_then(|params| params.get(STEP_PARAM)) {
            None => None,
            Some(Value::String(word)) => {
                Some(OrderStep::from_wire(word).ok_or(AttachFault::StepUnknown)?)
            }
            Some(_) => return Err(AttachFault::StepNotAString),
        };
        let goto = match params.and_then(|params| params.get(GOTO_PARAM)) {
            None => None,
            Some(Value::Object(path)) => {
                // ONE reader for the three members, so a missing bound check cannot be written into
                // two of them and left out of the third. `as_u64` is what refuses a negative, a
                // float and a string in one place — an id is a counter, and no other JSON number is
                // one.
                let id = |key: &'static str| match path.get(key) {
                    None => Ok(None),
                    Some(value) => value
                        .as_u64()
                        .map(Some)
                        .ok_or(AttachFault::GotoIdNotANumber(key)),
                };
                let session = id(GOTO_SESSION_PARAM)?.ok_or(AttachFault::GotoWithoutSession)?;
                let window = id(GOTO_WINDOW_PARAM)?;
                let pane = id(GOTO_PANE_PARAM)?;
                if window.is_none() && pane.is_some() {
                    return Err(AttachFault::GotoPaneWithoutWindow);
                }
                Some(Self::Goto {
                    session: SessionId(session),
                    window: window.map(WindowId),
                    pane: pane.map(PaneId),
                })
            }
            Some(_) => return Err(AttachFault::GotoNotAnObject),
        };
        // TWO targets is no target, whichever pair asks — the rule `TwoTargets` already states, now
        // over three keys instead of two. Written as one match over the whole tuple rather than as
        // a chain of early returns, so a fourth target arm cannot be added without deciding what it
        // means beside each of these.
        match (last, unattached, step, goto) {
            (true, _, Some(_), _) | (true, _, _, Some(_)) | (_, _, Some(_), Some(_)) => {
                Err(AttachFault::TwoTargets)
            }
            (true, unattached, None, None) => Ok(Self::LastViewed { unattached }),
            (false, true, _, _) => Err(AttachFault::UnattachedWithoutLast),
            (false, false, Some(step), None) => Ok(Self::Step(step)),
            (false, false, None, Some(goto)) => Ok(goto),
            (false, false, None, None) => Ok(Self::Scoped),
        }
    }
}

/// The filtered CHANGE WAIT, re-exported from the client that writes it for the same one-spelling
/// reason as the vocabulary above: [`EVENTS_WAIT_METHOD`] blocks until a change matching the caller's
/// [`EventFilter`](crate::events::EventFilter) lands after [`SINCE_PARAM`], answering the same
/// `{events, next, lost}` batch [`EVENTS_FIELD`] serves.
///
/// The pair with [`EVENTS_FIELD`] is deliberate and is the whole surface: **the slot is the read, the
/// method is the wait**. A caller that already knows something happened reads the slot; a caller that
/// wants to be told parks on the method. Neither is expressible as the other — a slot cannot block,
/// and a blocking read cannot be answered from a snapshot.
///
/// Intercepted before the generic dispatch core, like the three client-lifecycle methods, because it
/// PARKS its reply rather than returning one.
pub use sprag_rpc::{EVENTS_WAIT_METHOD, SINCE_PARAM};

/// The STREAMING form of that wait, re-exported for the same one-spelling reason:
/// [`EVENTS_SUBSCRIBE_METHOD`] takes the identical `{since, match?}` and answers ONCE, then writes an
/// [`EVENTS_CHANGED_METHOD`] notification per batch until [`EVENTS_UNSUBSCRIBE_METHOD`] or the
/// connection ends it.
///
/// **The trio completes a surface that was already three-quarters built.** The slot is the read, the
/// wait is the one-shot, and this is the follow — and all three take one cursor vocabulary and answer
/// one batch shape, so a caller moves between them without re-writing its reader. Until pinion R1552
/// (PINION-PR83) the third was not a design gap but a transport impossibility: a frame could be
/// answered at most once.
pub use sprag_rpc::{
    EVENTS_CHANGED_METHOD, EVENTS_SUBSCRIBE_METHOD, EVENTS_UNSUBSCRIBE_METHOD, SUBSCRIPTION_PARAM,
};

/// The OUTPUT WAIT, re-exported for the same one-spelling reason: [`PANE_WAIT_OUTPUT_METHOD`] blocks
/// until the pane named by [`PANE_PARAM`] has retained output matching [`NEEDLE_PARAM`] (a literal)
/// or [`PATTERN_PARAM`] (a regular expression).
///
/// It completes the slot/method pair one axis over from the change wait: **[`FIND_FIELD`] and
/// [`REGEX_FIELD`] are the read, this method is the wait** — and it answers [`crate::PaneFind`],
/// the very type those slots answer, so "does it say X" and "wait until it says X" are one shape a
/// caller can hand to one reader. A second answer shape here is the drift that pairing exists to
/// prevent.
///
/// Intercepted before the generic dispatch core, like [`EVENTS_WAIT_METHOD`], because it PARKS its
/// reply rather than returning one.
pub use sprag_rpc::{NEEDLE_PARAM, PANE_PARAM, PANE_WAIT_OUTPUT_METHOD, PATTERN_PARAM};

/// The wire's SHAPE agreement, re-exported from the transport that defines it for the same reason
/// the vocabulary above is: one spelling, both ends.
///
/// [`WIRE_PROTOCOL`] is the number this build speaks, [`PROTOCOL_PARAM`] the request key every
/// client declares it in, and [`PROTOCOL_FIELD`] the [`CLIENT_HELLO_METHOD`] reply key the daemon
/// answers with. The reader's contract — refuse at the door, before scope, before any handler —
/// lives on [`crate::rpc`]'s `protocol_refused`; what keeps the NUMBER honest is this module's own
/// `the_wire_shape_is_what_this_protocol_number_stands_for`, which fails on any change to a shape
/// a client decodes whole.
pub use sprag_rpc::{INVALID_PARAMS, PROTOCOL_FIELD, PROTOCOL_PARAM, WIRE_PROTOCOL};

/// The mux control external query slot: every session's name, plus which one an unscoped
/// request acts on — how a client discovers what it can address with [`SESSION_PARAM`].
///
/// Served rather than left to convention: a scope param an agent cannot enumerate is one it
/// must guess at, and the whole point of the scene-as-data surface is that a peer ASKS.
pub const SESSIONS_SLOT: &str = "sessions";

/// The mux control external query slot: the whole registry as a NAVIGABLE TREE — every session,
/// its windows and their panes, each carrying the IDENTITY a chooser commits by (R315).
///
/// Registry-WIDE like [`SESSIONS_SLOT`] and for its stated reason: the subject is the set of
/// scopes, so scoping it to the caller's own session would answer a question nobody asked. It
/// DESCENDS where [`WINDOWS_SLOT`] and [`PANES_SLOT`] are scoped, which is what a chooser needs and
/// what neither of those can give it — a client cannot see another session's windows at all today.
///
/// # Why it is a second slot rather than a wider `sessions`
///
/// [`SESSIONS_SLOT`] is read on every poll wake by every attached client. This is read when a
/// person presses one key. Widening the first would make the commonest question in the mux pay for
/// every window's pane pool — the trade R282 already made once, in the other direction, when it
/// split the `/proc` walk OUT of `sessions`.
///
/// # ONE READ
///
/// The whole tree comes back in one answer. A chooser that asked per session would assemble a list
/// whose levels disagreed, which is the torn read this project has removed twice; see
/// [`sprag_terminal::TreeSession`] for the exact bound one call gives and the one it does not.
pub const TREE_SLOT: &str = "tree";

/// A READ refused because this daemon does not have the address at all, as a sentence — `None` for
/// anything else, which is what keeps a caller from dressing up a fault it cannot explain.
///
///
/// # Why they live here and not in the client that first needed them
///
/// A slot and an action are both ADDITIVE — [`WIRE_PROTOCOL`] deliberately does not rise when
/// either is added, and the ratchet over this surface says so in its own assertion. So a client
/// that gained an address or a verb meets same-numbered daemons that lack it, and every client has
/// to be able to say which of those happened. THREE do: the `sprag` CLI reads slots and performs
/// actions, and `sprag-mcp` does both on an agent's behalf.
///
/// They were the CLI's private functions until an agent surface was measured against a peer that
/// serves nothing and knows no verb: **eight of eight tools** either printed the Rust variant name
/// at an agent or blamed the agent's own arguments — `display_message` answered *"the message may
/// be unacceptable"* about a message that was fine. One definition, because the alternative is two
/// clients disagreeing about what an old daemon is called.
///
/// The rest of the mechanism:
///
/// Matched on the fault's structured `data`, never on its rendered line: `Display` prefers `data`
/// and so the two agree today, but a substring test against a rendering is a test against a
/// presentation decision, and it would also fire on a daemon that merely mentioned the word.
/// Captured from a live daemon rather than invented — the reply is
/// `{"code":-32602,"message":"Invalid params","data":"UnknownIntrospectPath"}`.
///
/// It is also the DISCRIMINATOR the CLI's scoped pre-flight reads, which is why it is a function
/// of the fault alone and not a branch inside a caller: an unknown address and a refused SCOPE
/// arrive under one JSON-RPC code, and only the `data` tells them apart.
#[must_use]
pub fn unknown_slot(path: &str, fault: &RpcFault) -> Option<io::Error> {
    if fault.data.as_ref().and_then(Value::as_str)? != sprag_rpc::UNKNOWN_SLOT_FAULT {
        return None;
    }
    Some(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("this daemon does not serve {path} — {SKEW_REMEDY}"),
    ))
}

/// What every skew sentence ends with, and the reason it is a `const`.
///
/// **It is written to fit a STATUS ROW** ([`crate::report::MessageText::MAX_BYTES`]), which is the
/// change R324 made to it: the longer form it replaces came to 215 bytes for a 200-byte cap with the
/// shortest address this daemon serves, so the same fact could not be said at both surfaces. Two
/// sentences for one situation is the drift this module spent three rounds removing, so the sentence
/// got shorter rather than the surfaces getting one each.
///
/// `sprag kill-server` and *"older than this build"* are load-bearing words, not phrasing: the CLI
/// gate, the agent gate and both display gates match on them.
///
/// It is NOT shared with the protocol-mismatch refusal in [`crate::rpc`], which reads *"rebuild the
/// client, or restart this daemon to the client's build"* — that one is a TWO-SIDED disagreement
/// where either end may be the old one, and this is a daemon that is simply behind.
pub const SKEW_REMEDY: &str =
    "it is older than this build of sprag. Restart it: `sprag kill-server` (sessions are restored)";

/// An invoke refused because this daemon has never HEARD of the action — the invoke-side twin of
/// [`unknown_slot`], and `None` for any other fault so a caller's own disjunction still runs.
///
/// # Why it is worth telling apart from a refusal
///
/// Both arrive as `-32602 Invalid params`, so a verb that maps every fault to its own sentence
/// tells a user their name was taken when the truth is that their daemon predates the verb. That is
/// not hypothetical — it is what R297's skew run MEASURED, one direction at a time, against a
/// parent-commit daemon: `sprag rename-session` said *"prod" is already another session's name*
/// about a name no session held.
///
/// Captured from that live daemon rather than invented: an action it does not serve answers
/// `{"code":-32602,"message":"Invalid params","data":"UnknownInvokePath"}`, where a genuine refusal
/// of an action it DOES serve answers `"InvokeRejected"`. Matched on the structured `data` for
/// [`unknown_slot`]'s reason — a substring test against a rendering is a test against a
/// presentation decision.
///
/// # It names the ADDRESS, not the verb
///
/// It took a command name until this round, and three call sites passed one. A name a caller hands
/// in is a name a caller can hand in WRONG — copied with the line it was pasted from — and it is
/// the half of the sentence the shell line above the error already shows. The address is the half
/// that says WHICH act the daemon lacks, it comes from the params the caller already built, and it
/// makes this the exact twin of [`unknown_slot`] rather than a second style. Same argument, same
/// round, as the one `sprag`'s `query_slot` records for the reading side.
#[must_use]
pub fn unknown_action(path: &str, fault: &RpcFault) -> Option<io::Error> {
    if fault.data.as_ref().and_then(Value::as_str)? != sprag_rpc::UNKNOWN_ACTION_FAULT {
        return None;
    }
    Some(io::Error::new(
        io::ErrorKind::Unsupported,
        format!("this daemon does not perform {path} — {SKEW_REMEDY}"),
    ))
}

/// The DAEMON'S OWN sentence for an action it had and declined — [`None`] for any other fault, so
/// a caller's remaining handling still runs.
///
/// # It is [`unknown_action`]'s opposite, and the pair is the whole discrimination
///
/// One says *"this daemon cannot do that at all"* (a version skew: restart it) and this says *"this
/// daemon would not do that"* (a fact about the workspace: fix the argument). They used to arrive
/// as the same JSON-RPC code with the same empty payload, so every verb in this product wrote a
/// client-side DISJUNCTION and named every cause it could think of. Measured at `87cde88`, four of
/// them survived to the end: `rename-session` offered four causes, `break-pane` and `join-pane`
/// three, `rename-window` two — and in each case the registry had returned a typed error naming
/// exactly one.
///
/// # It re-labels and does not re-word
///
/// The sentence is the daemon's, verbatim. That is the point: a client that improved the wording
/// would be authoring a claim about state it cannot see, which is what a disjunction IS. What a
/// caller adds is its own subject (`sprag: join-pane: <this>`), because only the caller knows what
/// the user typed.
///
/// [`io::ErrorKind::InvalidInput`] rather than [`io::ErrorKind::Other`]: the request was well
/// formed and the WORKSPACE said no, which is a caller's input to change — distinct from
/// `unknown_action`'s [`io::ErrorKind::Unsupported`], where changing the input cannot help.
#[must_use]
pub fn refusal(fault: &RpcFault) -> Option<io::Error> {
    fault
        .refusal()
        .map(|reason| io::Error::new(io::ErrorKind::InvalidInput, reason.to_owned()))
}

/// What a caller says when a daemon refused `path` and STATED NOTHING — the one degradation left
/// once [`refusal`] and [`unknown_action`] have had their turn.
///
/// # It is a skew, and saying so is the whole of it
///
/// On this build a refusal cannot be anonymous: the type requires the sentence
/// (`InvokeError::rejected`). So a refusal that arrives bare is from a daemon older than the build
/// that made it mandatory — which makes this the same news [`unknown_action`] carries, and it ends
/// with the same remedy rather than a second story about the same situation.
///
/// **What it replaces is ten client-side DISJUNCTIONS.** Every acting verb used to answer a bare
/// refusal by naming every cause it could imagine, because that was genuinely all anyone knew;
/// `join-pane` cast doubt on three arguments when the daemon had rejected exactly one, and
/// `rename-pane` listed six rules. Keeping them as a fallback would preserve the sentences this
/// round exists to remove, at the one moment nobody is looking.
#[must_use]
pub fn unstated_refusal(path: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        format!("this daemon refused {path} and did not say why — {SKEW_REMEDY}"),
    )
}

/// The SAME fact as [`unknown_action`], as a message a display client can paint on its one row.
///
/// # Why a client needs this at all
///
/// A `scene/invoke` happens only because somebody ACTED — a key, a palette row, a drag — so a
/// client that swallows its refusal has taken a person's gesture and answered nothing. Measured at
/// `d651f50` against a daemon serving every read and knowing no verb: `prefix c` on a live
/// `sprag-tui` left the status row unchanged and created no window. A `scene/query` is the opposite
/// case and is deliberately NOT given one of these: reads happen on every poll wake, and a client
/// that reported each would have nothing else on its row.
///
/// # Why it is here and not in the client
///
/// Both display clients paint it, so neither writes it — the rule this module already holds for the
/// shell's sentence, applied to the surface R316/R317 built. The words are
/// [`unknown_action`]'s, minus the leading article a row does not need.
///
/// [`Severity::Warn`](crate::report::Severity::Warn): the person's act did not happen, which they
/// must be told, and it is not the alert kind that waits for an acknowledgement — a degraded daemon
/// is a standing condition, and every further gesture will say so again.
#[must_use]
pub fn skew_announcement(path: &str) -> Option<crate::report::Announcement> {
    use crate::report::{Announcement, MessageText, Severity};
    // A path long enough to overflow a row's budget is not reachable through any address this
    // daemon serves — and R318 recorded what an `expect` resting on exactly that reasoning cost, so
    // there is a fallback rather than a panic. It drops the ADDRESS, which is the only part that
    // can be long, and keeps the fact and the remedy: both are things a person can act on without
    // it. `the_row_sentence_survives_an_address_no_row_could_hold` drives it.
    let text = MessageText::parse(&format!(
        "this daemon does not perform {path} — {SKEW_REMEDY}"
    ))
    .or_else(|_| MessageText::parse(&format!("this daemon cannot act — {SKEW_REMEDY}")));
    text.ok().map(|text| Announcement {
        text,
        severity: Severity::Warn,
    })
}

/// The DAEMON'S OWN refusal, as a message a display client can paint on its one row —
/// [`skew_announcement`]'s peer for the act that was understood and DECLINED.
///
/// # Why the pair is two functions and not one
///
/// Two pieces of news with two remedies, which is the distinction PINION-PR82 spent an error code
/// on: a skew says *restart the daemon* and this says *that cannot be done to this workspace*. A
/// surface that painted one sentence for both would undo the split at the last inch.
///
/// # It forwards the daemon's words and adds none
///
/// Every other sentence in this module is one it authored; this one is the producer's. A client
/// improving on it would be authoring a claim about state it cannot see, which is what the ten
/// disjunctions R325 deleted were. What it replaces at the two display fronts is a client-side
/// GENERIC: `prefix !` on a lone pane painted *"break-pane: nowhere to go"* while the daemon was
/// saying *"cannot break the only pane in a window"*.
///
/// A reason too long for a row answers [`None`] rather than being truncated —
/// [`skew_announcement`]'s rule, and here it is reachable rather than theoretical, since the text
/// is a producer's and not an address. The caller keeps its own report for that case, which is the
/// behaviour every one of these had before this existed.
///
/// [`Severity::Warn`](crate::report::Severity::Warn), [`skew_announcement`]'s reason exactly: the
/// person's act did not happen and they must be told, and it is not the kind that waits for an
/// acknowledgement.
#[must_use]
pub fn refusal_announcement(reason: &str) -> Option<crate::report::Announcement> {
    use crate::report::{Announcement, MessageText, Severity};
    MessageText::parse(reason).ok().map(|text| Announcement {
        text,
        severity: Severity::Warn,
    })
}

/// Which session holds `pane`, read off a [`TREE_SLOT`] answer — how a process that knows only
/// which PANE it is in finds out which session it is in.
///
/// # Why this is one function and not one per client
///
/// The daemon publishes a pane id into every pane's environment
/// ([`crate::PANE_ENV_VAR`]) and two clients need it turned back into a scope: the `sprag` CLI, so
/// an unscoped command acts where its caller is standing, and `sprag-mcp`, so an agent's tools
/// answer about the agent's own session. Written twice, the two would be free to disagree about
/// which session a pane is in — a torn answer between the tool an agent reads with and the command
/// it acts with, in a mux whose whole subject is which session a thing belongs to.
///
/// It takes the DECODED tree rather than the raw JSON so the shape is the daemon's published type:
/// a wire change that moved panes under windows would stop this compiling instead of quietly
/// finding nothing.
///
/// [`None`] means the tree does not hold that pane at all, which is what a caller sees when its
/// `$SPRAG_PANE` outlived the daemon that set it (ids restart with the process). That is not an
/// error anywhere: it means nobody said which session, so the daemon's default is the one.
#[must_use]
pub fn session_holding(
    tree: &[sprag_terminal::TreeSession],
    pane: sprag_terminal::PaneId,
) -> Option<&str> {
    tree.iter()
        .find(|session| {
            session
                .windows
                .iter()
                .any(|window| window.panes.iter().any(|held| held.id == pane))
        })
        .map(|session| session.name.as_str())
}

/// The mux control external query FAMILY: every session's live ACTIVITY — where it is working, on
/// what branch, and what it is serving — with the AGE of the sample they were all taken in.
///
/// Registry-WIDE like [`SESSIONS_SLOT`], and split out of it by R282 for a reason that is about
/// KINDS of fact rather than about cost. [`SESSIONS_SLOT`] answers the registry's own structure,
/// which moves when this daemon performs an event the scene revision already announces. This answers
/// the operating system, which moves with nothing the daemon can see — so it must be SAMPLED, and a
/// sample has an age. Serving them together made the cheapest question in the mux cost a `/proc`
/// walk of every process on the box, on every poll wake of every attached client, for three facts a
/// printed character says nothing about.
///
/// The address carries the caller's staleness TOLERANCE — `session_activity.<max_age_ms>`, where
/// `session_activity.0` admits nothing and always samples afresh. The answer ([`ActivityWire`])
/// carries the age it actually has. See [`sprag_terminal::ActivitySampler`] for what a read does and
/// does not pay for.
///
/// That tolerance is the design's ONLY cadence control, and it is deliberately the CALLER's: `sprag
/// ls` is a one-shot human command that would rather wait than print a stale port, and a sidebar
/// poll would rather paint a second-old subtitle than make somebody's keystroke walk `/proc`. One
/// constant shared between them would have to be wrong for one of them.
///
/// A QUERY, not an invoke, and R152's livelock is why: a display client reads this on the same wake
/// it reads everything else, and an invoke bumps the scene revision — so serving it as an action
/// would wake the very `waitFor` it was answering. Sampling is not a mutation of the scene; it
/// observes a world the scene does not own.
///
/// The argument rides the PATH because that is what a `query` can carry (`cells.<offset>`'s lesson,
/// PR-61): the signature takes a path and nothing else. It is [`SchemaArg::open`] rather than an
/// index — a tolerance is not bounded by anything the surface can count, and saying `IndexOf` some
/// list would be inventing a domain to look precise.
pub const SESSION_ACTIVITY_FIELD: SchemaField = SchemaField::parametric(
    "session_activity.<max_age_ms>",
    "object",
    SESSION_ACTIVITY_ARGS,
);

/// [`SESSION_ACTIVITY_FIELD`]'s argument: how stale an answer this caller will accept, in
/// milliseconds. Open, because nothing on this surface bounds it.
const SESSION_ACTIVITY_ARGS: &[SchemaArg] = &[SchemaArg::open("max_age_ms", "int")];

/// The staleness a LIVE DISPLAY accepts for [`SESSION_ACTIVITY_FIELD`] — what a session sidebar's
/// subtitle may be behind the world, stated once so every display path agrees.
///
/// Used by a wire client's poll thread (which asks the daemon for this window) and by the
/// in-process arm (which samples at it), so a sidebar drawn over a daemon and one drawn in process
/// show facts of the same age. `sprag ls` does NOT use it: a one-shot human command asks for zero
/// and waits, because it prints once and is then read for as long as the operator looks at it.
///
/// One second, and the reasoning is about what the facts DO rather than about a budget. A cwd
/// changes when somebody types `cd`, a branch when they check one out, a port when a server binds:
/// all human-paced acts whose result a person expects to see "in a moment", not in the same frame.
/// A second is under that threshold and two orders of magnitude above what the sample costs, so the
/// walk lands at most once per second per host however many clients are attached and however fast
/// anyone types. herdr's counterpart for its git facts is 1500 ms (`GIT_REMOTE_STATUS_REFRESH_
/// INTERVAL` at `9a4ce5e1`), reached by the same reasoning about the same kind of fact — a rival's
/// number is not evidence, but a rival arriving nearby is worth recording.
pub const SESSION_ACTIVITY_DISPLAY_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(1);

/// [`SESSION_ACTIVITY_FIELD`]'s address with the tolerance filled in — `session_activity_at(0)` is
/// the always-fresh read.
///
/// Built from the declared field rather than by re-spelling `"session_activity."`, so the address a
/// client sends and the prefix the host strips cannot drift — `cells_slot_at`'s discipline, for the
/// same reason.
#[must_use]
pub fn session_activity_at(max_age_ms: u64) -> String {
    format!("{}{max_age_ms}", SESSION_ACTIVITY_FIELD.literal_prefix())
}

/// What [`SESSION_ACTIVITY_FIELD`] answers: every session's [activity](sprag_terminal::SessionActivity),
/// and how long ago the reading they all came from was taken.
///
/// The age is on the wire, in the ENVELOPE rather than on each row, because one pass produces them
/// all — the `/proc` walk that attributes listening sockets is shared across every session, so no
/// row is fresher than another and a per-row age would invite a reader to believe otherwise.
///
/// Why it is carried at all: a sampled fact read without its age is one whose freshness the reader
/// has to assume, and the assumption is wrong exactly when it matters — a `ports` list that predates
/// the server somebody just started looks identical to one that does not. A client can render
/// staleness, an operator can distrust a number for a reason, and neither has to know what tolerance
/// some other caller asked for.
///
/// Milliseconds, not a `Duration`: this is the wire, and `Duration`'s serialised form is a pair of
/// integers whose meaning a non-Rust peer would have to be told.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ActivityWire {
    /// How long ago the [`sessions`](Self::sessions) below were sampled, in milliseconds. `0` for a
    /// sample taken to answer this very request.
    pub sampled_ms_ago: u64,
    /// One row per session, in the registry's own order — the same order [`SESSIONS_SLOT`] answers
    /// in, though a reader should join on [`name`](sprag_terminal::SessionActivity::name) rather
    /// than on position: the two answers are separate requests, and a session can be created between
    /// them.
    pub sessions: Vec<sprag_terminal::SessionActivity>,
}

impl From<sprag_terminal::ActivityReading> for ActivityWire {
    /// The one conversion from the in-process reading to the wire's shape — the seam where a
    /// `Duration` becomes the integer a peer can read.
    ///
    /// Saturating rather than wrapping: a sample old enough to overflow a `u64` of milliseconds
    /// cannot exist (it would predate the daemon by half a billion years), but "impossible" is not a
    /// reason to let the arithmetic decide what happens if it did.
    fn from(reading: sprag_terminal::ActivityReading) -> Self {
        Self {
            sampled_ms_ago: u64::try_from(reading.age.as_millis()).unwrap_or(u64::MAX),
            sessions: reading.value,
        }
    }
}

/// The mux control external query slot: WHAT EACH PANE IS RUNNING — its terminal device, the child
/// the daemon spawned, and the foreground job that owns its terminal, with every process in it.
///
/// A SAMPLED fact and therefore its own address, exactly like [`SESSION_ACTIVITY_FIELD`] and by the
/// same rule: [`PANES_SLOT`] carries what changes when this daemon performs a change, and a user
/// typing `cargo build` at a shell prompt is not one of those — the daemon sees bytes. Folding it
/// into the pane list would make a `/proc` walk of the whole box the price of every poll wake of
/// every attached client, which is the cost R282 measured at 3478 us and removed.
///
/// The address carries the caller's staleness TOLERANCE — `pane_processes.<max_age_ms>`, where
/// `pane_processes.0` admits nothing and always samples afresh. The answer
/// ([`PaneProcessesWire`]) carries the age it actually has.
///
/// REGISTRY-WIDE, not scoped: `/proc` has no index by process group, so enumerating one pane's job
/// costs the same full pass that answers every other pane. Serving one session's panes would
/// therefore cost the same and let two scopes each pay it. A reader takes the ids it cares about
/// from [`PANES_SLOT`] and joins on them — pane ids are registry-unique and never reused, so that
/// join cannot pair one read's row with another's pane.
///
/// A QUERY, not an invoke, for [`SESSION_ACTIVITY_FIELD`]'s reason: observing the world is not a
/// mutation of the scene, and serving it as an action would bump the revision and wake the very
/// `waitFor` it was answering.
pub const PANE_PROCESSES_FIELD: SchemaField =
    SchemaField::parametric("pane_processes.<max_age_ms>", "object", PANE_PROCESSES_ARGS);

/// [`PANE_PROCESSES_FIELD`]'s argument: how stale an answer this caller will accept, in
/// milliseconds. Open, because nothing on this surface bounds it.
const PANE_PROCESSES_ARGS: &[SchemaArg] = &[SchemaArg::open("max_age_ms", "int")];

/// [`PANE_PROCESSES_FIELD`]'s address with the tolerance filled in — `pane_processes_at(0)` is the
/// always-fresh read.
///
/// Built from the declared field rather than by re-spelling the prefix, so the address a client
/// sends and the prefix the host strips cannot drift ([`session_activity_at`]'s discipline).
#[must_use]
pub fn pane_processes_at(max_age_ms: u64) -> String {
    format!("{}{max_age_ms}", PANE_PROCESSES_FIELD.literal_prefix())
}

/// What [`PANE_PROCESSES_FIELD`] answers: every pane's
/// [processes](sprag_terminal::PaneProcesses), and how long ago the reading they all came from was
/// taken.
///
/// The age is in the ENVELOPE rather than on each row, because one `/proc` pass produces them all —
/// no row is fresher than another and a per-row age would invite a reader to believe otherwise.
/// Milliseconds, not a `Duration`, for [`ActivityWire`]'s reason: this is the wire, and a
/// `Duration`'s serialised form is a pair of integers whose meaning a non-Rust peer would have to be
/// told.
///
/// Two fields of each row do NOT age with it — a pane's device is fixed at its birth and its child
/// pid until the reap — and each says so in its own doc. They ride here because this is the question
/// that wants them, and the pane list every client re-reads per wake should not grow a string per
/// pane to carry them.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneProcessesWire {
    /// How long ago the [`panes`](Self::panes) below were sampled, in milliseconds. `0` for a sample
    /// taken to answer this very request.
    pub sampled_ms_ago: u64,
    /// One row per pane in the registry, in the registry's own order — join on
    /// [`id`](sprag_terminal::PaneProcesses::id), never on position.
    pub panes: Vec<sprag_terminal::PaneProcesses>,
}

impl From<sprag_terminal::PaneProcessReading> for PaneProcessesWire {
    /// The one conversion from the in-process reading to the wire's shape. Saturating for
    /// [`ActivityWire`]'s reason.
    fn from(reading: sprag_terminal::PaneProcessReading) -> Self {
        Self {
            sampled_ms_ago: u64::try_from(reading.age.as_millis()).unwrap_or(u64::MAX),
            panes: reading.value,
        }
    }
}

/// The mux control external query slot: every currently-ATTACHED client and the session it is
/// viewing (`[{client, session}]`) — tmux `list-clients`, behind the `sprag list-clients` CLI.
///
/// Registry-WIDE like [`SESSIONS_SLOT`] (its subject is the set of clients, not any one session),
/// and filled HOST-side from the dispatch layer's [`crate::AttachmentRegistry`] — the same
/// per-client state that fills each [`SESSIONS_SLOT`] row's `attached` count. Empty off a daemon
/// that tracks no wire clients (a GUI's in-process host), so it degrades to "no clients".
pub const CLIENTS_SLOT: &str = "clients";

/// The mux control external query slot: the SCOPED session's arbitrated window size
/// (`{cols, rows}`, or `null` when no attached client has reported an area) — the rectangle every
/// client lays the arrangement out over ([`sprag_terminal::tile`]).
///
/// This is the ANSWER to tmux's `window-size`, not the option: the daemon reads the policy from the
/// user's `config.toml` itself and publishes only the size it arbitrated
/// ([`crate::window::arbitrate`]), so no option crosses the wire (R240) and a client needs to know
/// nothing about which rule produced its window.
///
/// Scoped, because a window belongs to a session. `null` is a real answer meaning "no client has
/// said how big it is" — a client that reads it then leaves the panes at the size they have, rather
/// than reflowing every program in the session to a number nobody chose.
pub const WINDOW_SIZE_SLOT: &str = "window_size";

/// The mux control external invoke action that creates a session BORN WITH A SHELL
/// (`{name?, cmd?, cols?, rows?, cwd?}`), returning its name.
///
/// Creating one spawns its first pane, mirroring tmux's `new-session -x -y [command]`
/// (`cmd`/`cols`/`rows`/`cwd` shape that pane; absent → `$SHELL` at the pool's default size in the
/// daemon's own directory — `cwd` is [`SPAWN_ACTION`]'s, because a birth is a birth and the spec is
/// one spec; it is NOT an opener, which only the two PANE births take), so on the
/// happy path a session is not empty (only a runtime fork/exec failure leaves it so). A malformed
/// birth spec is rejected before the session is created, the same as a bad `name`. An ACTION, not
/// an `intervene` slot: it is not a plain assignment — the
/// name must be free (a name is an ADDRESS, so a duplicate would make one ambiguous) and the
/// create is refusable. Creating is deliberately NOT attaching: it changes no other client's
/// scope, because nothing can — a `new_session` never moves the default, and every other client
/// names its own.
pub const NEW_SESSION_ACTION: &str = "new_session";

/// The mux control external invoke action that kills a session (`{name}`) — tmux
/// `kill-session`.
///
/// An ACTION, not an `intervene` slot: it removes a session (or, for the last one, drains it
/// and ends the daemon), so it is a lifecycle event a client requests, not an assignment. A
/// non-last kill answers `null`; killing the LAST session ends the server, so its answer may
/// never reach the client (the socket closes as the daemon exits).
pub const KILL_SESSION_ACTION: &str = "kill_session";

/// The mux control external query slot: what this host has paid to PROJECT its cells
/// (`{projections_total, cells_total}`) — [`sprag_grid::work`], on the wire.
///
/// Registry-WIDE like [`SESSIONS_SLOT`], and for a stronger reason than convenience: the counters
/// are process-wide, so hanging them off any one pane would invite a reader to attribute the whole
/// host's work to that pane.
///
/// Served because sprag's grid is the ONE surface no other instrument can see. pinion prices its
/// own side on `scene/frame_timings`, and sprag R216 read that wire to prove terminal output never
/// reaches pinion's shaper — true, and only half an answer, because sprag does not paint its cells
/// through pinion's text path at all. It projects a whole `GridBuffer` per served frame, and what
/// THAT costs was unmeasurable from outside this process. This slot is that measurement, in the
/// form the rest of the surface uses: data a peer asks for, not a log line it has to be running to
/// catch.
///
/// Both totals are monotonic since boot, so a reader takes a DELTA across whatever it is pricing.
pub const GRID_WORK_SLOT: &str = "grid_work";

/// The mux control external invoke action that spawns a pane, returning its id
/// (`{cmd?, cols?, rows?, remote?, cwd?, opened_by?, name?}`).
///
/// # `name` — what to call the pane
///
/// The pane's operator-given name ([`sprag_terminal::PaneName`]), absent for a pane nobody names.
/// Naming at BIRTH is what lets a caller never hold a pane NUMBER at all: the number is positional
/// and moves when any earlier pane closes, where a name is the caller's own and does not.
///
/// Refused (`Rejected`) when the name breaks one of [`PaneName::parse`](sprag_terminal::PaneName::parse)'s
/// rules, or when another pane of this DAEMON already carries it — a name is unique registry-wide
/// because it stands in for a registry-unique id. Both are checked before anything is built, so a
/// refusal costs no pane, exactly as `cwd`'s is.
///
/// # `cwd` — where the child starts
///
/// Absent is the DAEMON's own directory, which is where every pane started before this argument
/// existed. A string that does not name an existing directory is `Rejected` before anything is
/// built, rather than left to the exec: a spawn into a missing directory produces a pane whose
/// child died, and on screen that is indistinguishable from a shell that exited for no reason.
///
/// # `opened_by` — who asked for the pane
///
/// The pane whose OCCUPANT is asking ([`sprag_terminal::Pane::opened_by`]), absent for a pane
/// nobody claims — a person's split, a plain `sprag split-window`, a session's birth pane. It is
/// the caller's own identity, so it is a CLAIM the daemon records rather than derives: a connection
/// carries no pane, and the peers of this socket are all one user's own clients (the trust model
/// [`crate::events`]' parked waits already state). What it is checked for is EXISTENCE — a pane this
/// daemon does not hold is `Rejected`, so a caller with a stale `SPRAG_PANE` cannot stamp a
/// provenance naming a pane that is gone.
///
/// What rests on it is an agent surface that refuses to close a pane its caller did not open. That
/// gate is ergonomic, not a security boundary: an agent that can type into a shell can run
/// `sprag kill-pane` regardless. It exists because the mistake it prevents — a mis-resolved pane
/// number destroying a person's work — is the one that actually happens.
///
/// [`SPLIT_ACTION`] takes both arguments identically; a spawn is the one that states no opinion
/// about the arrangement, which is why it is the one an agent's work pane is born through.
pub const SPAWN_ACTION: &str = "spawn";
/// The mux control external invoke action that DIVIDES a named pane and spawns the new one into
/// the half it opens (`{pane, dir, before?, cmd?, cols?, rows?, remote?, cwd?, opened_by?, name?}`),
/// returning the new pane's id — tmux `split-window -h` / `-v`.
///
/// `cwd`, `opened_by` and `name` are [`SPAWN_ACTION`]'s, verbatim — a split IS a spawn with a place,
/// so the birth vocabulary is one vocabulary and is written down there.
///
/// [`SPAWN_ACTION`] with a PLACE. A spawn appends, which states where only by convention, so
/// every directional split had to be expressed as a spawn plus a whole rewritten tree
/// ([`SET_LAYOUT_ACTION`]) — an author with pixels and a gesture can do that, and a shell script
/// or a terminal client cannot. This is the op that makes "put a shell below pane 3" sayable by
/// a caller that draws nothing.
///
/// `dir` is `"horizontal"` (the new pane goes RIGHT of `pane`) or `"vertical"` (BELOW it) — it
/// names how the two halves are LAID OUT, not which way the divider is drawn, exactly as
/// [`SplitDir`](sprag_terminal::SplitDir) and tmux's own `-h` / `-v` do, so one vocabulary spans
/// the wire and the tree. `before` (default `false`) puts the new pane on the other side
/// instead — left of, or above — which is tmux's `-b`.
///
/// `pane` ABSENT means the current window's ACTIVE pane — tmux's "split where I am". It stopped
/// being required when the daemon gained an active pane to mean "here"
/// ([`SELECT_PANE_ACTION`]); before that a direction had no pane to be relative to and every
/// caller had to name one, including the ones that draw nothing and had no way to know which.
/// A window holding no pane at all has no "here", and a targetless split there is `Rejected`.
///
/// REFUSED — with nothing spawned and the arrangement untouched — when `pane` holds no leaf in
/// the scoped session's current window: it exited, it is floating, or it is another window's. A
/// split that cannot reach its target must not quietly become an append, which is the same lie as
/// accepting `-h` and ignoring it.
pub const SPLIT_ACTION: &str = "split";
/// The mux control external invoke action that closes a pane (`{id?}`) — tmux `kill-pane`.
///
/// `id` ABSENT means the current window's ACTIVE pane, the default [`SPLIT_ACTION`] takes and for
/// the same reason. The window then hands the active pane on to the closed one's neighbour, so a
/// caller can close repeatedly without naming anything and walk the window down.
///
/// # What it ENDS, and what it answers
///
/// Answers `{ended}` — one of [`Ended`](sprag_terminal::Ended)'s four words, and the reason this
/// action is not a fire-and-forget `null` any more. A mux is nested, so closing a pane can take
/// three other things with it: a window's LAST pane ends the WINDOW, a session's last window ends
/// the SESSION, and the last session ends the SERVER. Until R309 the daemon did none of that — it
/// removed the pane and left a window tiling nothing, which `sprag layout` reported as
/// `no panes tiled` and both frontends drew as a void — while the GUI's own palette was already
/// telling users *"It is this window's last pane and this session's last window."*
///
/// The cascade lives in [`SessionRegistry::close_pane`](sprag_terminal::SessionRegistry::close_pane)
/// and delegates upward to `kill_window`, so `kill-pane`, [`KILL_WINDOW_ACTION`] and
/// [`KILL_SESSION_ACTION`] are three entrances to ONE chain and answer with one vocabulary.
///
/// **`"server"` races its own delivery**, and that is a property of the answer rather than a
/// defect: the caller is being told the daemon is ending, so the reply may be severed by the exit.
/// A severed connection therefore reads as success — the `server_gone` arm every kill verb in
/// `sprag` already had.
pub const CLOSE_ACTION: &str = "close";
/// The key carrying [`Ended`](sprag_terminal::Ended)'s word in the answer of [`CLOSE_ACTION`],
/// [`KILL_WINDOW_ACTION`] and [`KILL_SESSION_ACTION`].
///
/// Spelled once, here, because three handlers write it and four readers (the CLI, the wire client,
/// the MCP tool and the wire's own shape pin) parse it — the shape R300 found had grown a FIFTH
/// hand-built copy of a request's keys in the crate both frontends share.
pub const ENDED_KEY: &str = "ended";
/// The mux control external invoke action that resizes a pane's PTY + emulator (`{id?, cols, rows,
/// cell_width?, cell_height?}`). `id` absent ⇒ the current window's ACTIVE pane.
pub const RESIZE_ACTION: &str = "resize";

/// The mux control external invoke action that NAMES a pane (`{pane, name?}`). `name` absent (or
/// `null`) takes the pane's name away.
///
/// Answers `{name}` — the name that was RECORDED, or `null` after a clear. Not the argument that
/// was sent: a name is trimmed on the way in, so `" build "` lands as `"build"`, and a caller that
/// echoed its own request would report a name the pane does not have. The write says what it wrote,
/// so nobody re-reads and nobody re-implements the trimming rule.
///
/// # Why a pane has a name at all, when it already has an id
///
/// Because the id is not what the callers hold. The agent surface addresses a pane by its 1-BASED
/// NUMBER in the pane listing, and that number is positional — closing any earlier pane shifts it.
/// So a caller's remembered number silently comes to name a DIFFERENT pane, and the write it then
/// makes succeeds against the wrong subject, which is the worst answer a surface can give.
///
/// The stable handle could not be the id, because a number and an id are both integers and one
/// argument cannot carry the two without a mode flag. **A name is a string, so JSON's own types
/// discriminate it**: `pane: 3` is the third pane and `pane: "build"` is the pane called build.
/// That is why the stable handle is a name — and why
/// [`PaneName`](sprag_terminal::PaneName) refuses an all-digit one.
///
/// # What it refuses, and why the pane is named DAEMON-WIDE
///
/// * `pane` naming no pane THIS DAEMON holds ⇒ `Rejected`. Daemon-wide rather than scoped, because
///   a pane id is registry-unique and so is a name; scoping it would refuse a rename of a pane that
///   plainly exists.
/// * A `name` breaking one of [`PaneName::parse`](sprag_terminal::PaneName::parse)'s rules
///   (blank, over 80 bytes, containing a control character, all digits) ⇒ `Rejected`.
/// * A `name` another pane already carries ⇒ `Rejected`. A name that resolved to two panes would
///   reintroduce the very ambiguity it exists to remove. Renaming a pane to the name it already
///   has is NOT refused: the pane carrying it is the one being renamed, so nothing is ambiguous.
///
/// The four are one `Rejected` on the wire, because `InvokeError::Rejected` carries no payload
/// (upstream PINION-PR82). The daemon logs which; the in-process callers say which.
///
/// # Why this is an ACTION and not an `intervene` slot
///
/// [`NEW_SESSION_ACTION`]'s reason, verbatim: a name is an ADDRESS, so the assignment is refusable
/// and a plain write would have nowhere to say so.
pub const RENAME_PANE_ACTION: &str = "rename_pane";
/// The mux control external query slot: the live pane list as JSON.
pub const PANES_SLOT: &str = "panes";

/// The arguments of [`PROJECT_FIELD`] — one pane `id`, `Open` (a pane id is minted by the host and
/// never bounded by a list this schema publishes, the same reason [`IMAGE_DATA_ARGS`] is open).
const PROJECT_ARGS: &[SchemaArg] = &[SchemaArg::open("pane", "int")];

/// The mux control external query slot: the PROJECT governing one pane — the commands its
/// `.sprag.toml` declares ([`Project`](crate::Project)), as
/// `{root, actions:[{name,title,run}]}`; `{error}` when that project's config is unusable, and
/// `null` when the pane is in no project at all.
///
/// Pane-PARAMETRIC but served on the MUX external rather than the pane's own, because the answer
/// needs two facts only the registry holds together: the pane's live working directory (which
/// decides WHICH project) and whether the pane is a REMOTE workspace (in which case that cwd is on
/// another machine and no local walk can describe it, so the answer is `null`).
///
/// Read ON DEMAND — a client asks when it opens a palette or runs a command, never per frame, so a
/// filesystem walk never lands on the paint path. The three outcomes are deliberately distinct:
/// "no project" is not an error, and a project whose config has a typo must say so rather than look
/// empty (the same rule the find bar's refused-pattern report follows).
pub const PROJECT_FIELD: SchemaField =
    SchemaField::parametric("project.<pane>", "object", PROJECT_ARGS);

/// The mux control external query slot: the USER's own declared commands ([`crate::UserConfig`]),
/// as `{path, commands:[{name,title,run}]}`; `{error}` when that config is unusable, and `null` when
/// the user has written none.
///
/// A SIBLING of [`PROJECT_FIELD`], deliberately not folded into it. The two answer different
/// questions with different lifetimes — one is a function of a pane's working directory, the other of
/// the host's user — and a pane in NO project must still be offered the user's commands, which a
/// pane-parametric slot returning `null` could never do. Keeping them apart is also what lets a
/// client report WHICH config has a typo when both are broken.
///
/// Read ON DEMAND (a palette opening), never per frame: like the project read, it touches the disk.
pub const GLOBAL_COMMANDS_SLOT: &str = "commands";

/// The mux control external query slot: why the agent manifests IN FORCE are not the ones the user's
/// `config.toml` declares, as `{error}` — and `null` when they are (or when the user declares none).
///
/// GLOBAL, which is what separates it from every H3 surface beside it: an agent verdict is a
/// property of one pane and is published on the pane's own key, while a broken `[[agent]]` block
/// takes the whole daemon's detection down to the built-ins. There is no pane to hang it on, so it
/// is a fixed slot like [`GLOBAL_COMMANDS_SLOT`] rather than a field on the pane list.
///
/// Answered from the daemon's own holder rather than by re-reading the file, and the difference is
/// not an optimisation. After a broken edit the rules in force are the last ones that WORKED, not
/// the built-ins and not the file's — a fact only the waker that did the re-read knows. A slot that
/// parsed the file itself would be a second authority reporting on a first, and would say "broken"
/// for up to a sweep before the daemon had acted on it.
///
/// Read ON DEMAND — a palette opening, a `sprag agent`, an `agent_explain` — never per frame. It
/// touches no disk at all (the report is already rendered and held), which is the one way it is
/// CHEAPER than the two config slots above rather than merely alike.
pub const AGENT_MANIFESTS_SLOT: &str = "agent_manifests";

/// The mux control external action: a process inside a pane REPORTS what it is doing, and that
/// report outranks anything the screen argues until it is released.
///
/// `{id, source, state, name?, seq?}` → `{accepted, changed, seq}`.
///
/// * `id` — the pane, which a reporter inside one reads from its own environment (`SPRAG_PANE`, the
///   variable a daemon publishes at each pane's birth). The same `id` every other pane-addressing
///   action takes, because there is only one name for a pane.
/// * `source` — who is speaking (`herdr:claude`'s shape: an integration, not a person). REQUIRED: an
///   authority that cannot be named cannot be told from another one, cannot be shown to a user, and
///   cannot have its own replays refused.
/// * `state` — `working` / `blocked` / `idle`, read through
///   [`AgentState::from_wire`](sprag_detect::AgentState::from_wire) so the vocabulary has one
///   definition. `unknown` is REFUSED: a reporter that no longer knows is asking to be scraped, which
///   is [`RELEASE_AGENT_ACTION`], and accepting it here would pin "not an agent" over a pane the
///   screen can read perfectly well.
/// * `name` — which agent is speaking, published as the pane's `agent.name`. Optional, because the
///   report's subject is the STATE; a reporter that omits it leaves the pane's identity to the rules.
/// * `seq` — the reporter's own monotonic clock. Optional, and when present a value at or below the
///   last one accepted FROM THAT SOURCE is refused as a replay (`accepted: false`), which is the only
///   way a reporter learns that its message arrived out of order.
/// * `bind` — whether this report should last only as long as whatever is currently running in the
///   pane. Optional, default false. A HOOK sets it, because it speaks for the agent that spawned it
///   and must not outlive it; a person does not, because their report is theirs to withdraw and the
///   command they typed it with has already exited.
///
///   It does NOT say what to bind to, and that is the point: the daemon reads which process group
///   owns the pane's terminal itself, so a caller can neither name somebody else's process nor park
///   a release on a pane it does not speak for. A pane whose foreground group cannot be read (no
///   `/proc`, or a child already reaped) yields an unbound report rather than a refused one — the
///   honest degradation, which is where this action was before the field existed.
///
/// The answer's `changed` says whether the published verdict actually moved — a duplicate report is
/// accepted and changes nothing — and it is what decides whether the daemon records an
/// `agent_state_changed` event and wakes the session's clients.
///
/// A report is published WITHOUT the settle window: hysteresis exists because a resting verdict rests
/// on the absence of an animated signal, and a report is not a sample of a screen (see
/// [`Tracker::report`](sprag_detect::Tracker::report)).
pub const REPORT_AGENT_ACTION: &str = "report_agent";

/// The mux control external action: give a pane back to the screen — `{id}` → `{released}`.
///
/// The other half of [`REPORT_AGENT_ACTION`], and part of why that one needs no expiry clock — the
/// rest being that a bound report is retired when the process group it named is gone. A
/// reporter calls it when the agent it speaks for is finished or gone; a person calls it when a
/// reporter has wandered off; and the daemon calls it for a pane whose CHILD has exited, since a
/// process that no longer exists cannot be the authority on what its pane is doing.
///
/// `released` is `false` for a pane nobody was reporting, so "stopped listening" is distinguishable
/// from "there was nobody to stop listening to" — a caller retrying a release is not silently told
/// it worked.
///
/// The released pane does not go blank: it keeps the last published verdict until the daemon's next
/// pass re-derives one from its screen, which the release itself asks for (the waker is signalled and
/// the pane owes a look).
pub const RELEASE_AGENT_ACTION: &str = "release_agent";

/// The mux control external action that puts a sentence in front of the people looking at this
/// daemon — `{text, severity?, client?}` → `{clients: [<client id>…]}` — tmux `display-message`.
///
/// # The gap it closes, measured rather than argued
///
/// Measured at `5acde43` by running the shipped binaries: with a real `sprag-tui` on a real
/// pseudoterminal, **nothing outside that client could put a word on its screen**. `report-agent
/// blocked` from another process left the screen byte-for-byte unchanged (it moves the terminal's
/// window TITLE, and carries a three-word state rather than a sentence); `send-keys` put the words
/// *inside the person's program*, which is typing and not a message; and a pane child's OSC 9 —
/// which this daemon latches — showed the terminal front nothing at all. The one thing that could
/// reach the status row R316 built was that client's own keyboard.
///
/// # The address
///
/// * `client` absent ⇒ every client attached to the request's SCOPED session
///   ([`crate::Audience::Session`]). That is what a script or a hook means: it knows a session, not
///   a window on somebody's desk.
/// * `client` present ⇒ that ONE client, wherever it is attached ([`crate::Audience::Client`]) —
///   tmux `display-message -c`. A client id that is not attached is `Rejected` rather than delivered
///   to nobody, because a caller that named a target got the name wrong, which is a different fact
///   from *nobody is watching*.
///
/// The ids are the ones the `clients` slot lists ([`CLIENTS_SLOT`], `sprag list-clients`), so the
/// listing a caller reads to choose a target and the set this reaches are one map.
///
/// # The answer
///
/// `{clients: […]}` — WHO it reached, ordered by client id, empty when nobody is attached. Not a
/// bool and not `ok`: an agent that says *"the deploy needs you"* into a daemon nobody is watching
/// has told nobody, and R316's whole finding is that an outcome no caller reads is a defect waiting
/// for a user to find. See [`crate::Delivery`] for what "reached" claims and what it does not.
///
/// # The text
///
/// A [`MessageText`](crate::report::MessageText): non-blank, bounded, and **free of control
/// characters**, because these bytes are written into somebody's terminal — a newline forges a row
/// and an escape is obeyed. Refused (`TypeMismatch`) rather than sanitised, so a caller whose
/// message was unacceptable learns it instead of watching it be quietly truncated.
///
/// `severity` is one of [`Severity`](crate::report::Severity)'s own words, defaulting to `note`.
pub const DISPLAY_MESSAGE_ACTION: &str = "display_message";

/// The `project.<pane>` query path for pane `id` — the ONE place that name is built, so a client and
/// the host cannot spell it differently.
#[must_use]
pub fn project_slot_for(pane: u64) -> String {
    format!("project.{pane}")
}

/// The argument of [`EVENTS_FIELD`] — the revision the reader has already accounted for.
///
/// **`Open`, and unlike [`CELLS_ARGS`] it is EARNED.** R155 refused an `Open` there because the
/// bound was "a count we had not exposed", and pinion calls an unearned `Open` an affirmative false
/// statement carrying a schema's authority. Both alternatives are checked here rather than skipped:
///
/// * [`ArgDomain::IndexOf`](pinion_core::external::ArgDomain::IndexOf) means the answerable
///   arguments are exactly `0..count`. Every revision at or below the current one is answerable AND
///   so is every value above it (it answers an empty batch, which is the truthful answer to "what
///   happened after a moment that has not arrived"). A count would be false at the top end — the
///   `datepicker` case pinion names as an honest `Open`.
/// * [`ArgDomain::ValuesOf`](pinion_core::external::ArgDomain::ValuesOf) means a key drawn from a
///   list this surface publishes. A cursor is not drawn from a list; it is whatever revision the
///   reader last saw, which reaches it from `scene/waitFor` rather than from here.
///
/// And nothing is left unexposed, which was R155's actual complaint: what a reader needs is whether
/// it fell behind, and the ANSWER carries that (`lost`). That is strictly more than a domain could
/// state, because it is evaluated against this reader's own cursor rather than published as a bound.
const EVENTS_ARGS: &[SchemaArg] = &[SchemaArg::open("since", "int")];

/// The mux control external query FAMILY: what has CHANGED in the scoped session since revision
/// `since` — `events.<since>`, answering `{events, next, lost}`.
///
/// ## Why a QUERY, and why the argument rides the path
///
/// A reader wakes on `scene/waitFor {since: R}`, which answers the revision `R'` the scene advanced
/// to, and then asks this family at `R` for what happened in `(R, R']`. So the cursor vocabulary is
/// the scene's own token and this needs no counter of its own ([`crate::events`] has the scar that
/// rule comes from).
///
/// That makes it a read WITH AN ARGUMENT, which is exactly the shape sprag once concluded was
/// impossible: [`CELLS_FIELD`] records the whole episode. An argument-bearing read served as an
/// invoke is a `MethodOcc::Mutate`, so it BUMPS — and a reader that also parks on `scene/waitFor`
/// would wake its own waiter by reading, which for an event stream is worse than it was for frames,
/// because reading events would generate events. The argument rides the path (pinion R1352), the
/// method stays `MethodOcc::Read`, and the read is free.
///
/// ## Unscoped in name, scoped in answer
///
/// The path carries no session: the answer is the SCOPED session's, like [`PANES_SLOT`] and
/// [`LAYOUT_SLOT`], because a journal is keyed by a revision and revisions are only comparable
/// within one session ([`crate::notify`]). A reader watching several sessions opens a scoped
/// connection per session — the same shape its `scene/waitFor` already has to take.
pub const EVENTS_FIELD: SchemaField =
    SchemaField::parametric("events.<since>", "object", EVENTS_ARGS);

/// The [`EVENTS_FIELD`] query path for cursor `since` — built from the declaration's own
/// [`literal_prefix`](SchemaField::literal_prefix) like [`cells_slot_at`], so the address a client
/// sends, the prefix the host strips and the template an agent discovers cannot drift apart.
#[must_use]
pub fn events_slot_since(since: u64) -> String {
    format!("{}{since}", EVENTS_FIELD.literal_prefix())
}
/// The mux control external query slot: the LOGICAL layout of the current window of the
/// session the request is SCOPED to ([`SESSION_PARAM`]), plus the
/// revision it is at ([`LayoutSnapshot`](sprag_terminal::LayoutSnapshot)) as JSON — the
/// arrangement a display client projects, and the state that lets a reattaching client
/// restore the user's layout. Logical only: it carries no pixels.
pub const LAYOUT_SLOT: &str = "layout";
/// The mux control external invoke action that INSTALLS a client's settled arrangement
/// (`{tree, expected_revision, expected_window?}`), returning the canonical
/// [`LayoutSnapshot`](sprag_terminal::LayoutSnapshot).
///
/// `expected_revision` is the revision the gesture was authored against — a compare-and-set,
/// so a write against an arrangement that has moved on is REFUSED rather than silently
/// reverting whoever moved it.
///
/// `expected_window` is optional: the NAME of the window the gesture was drawn on. Because the
/// per-window revision has no cross-window ordering, a client that has since switched windows
/// could otherwise land a stale write on a DIFFERENT window whose revision happened to collide;
/// naming the window makes the compare-and-set refuse that too. Absent ⇒ no window check (a
/// single-client / older caller); present but not a string ⇒ malformed, refused like a
/// wrong-typed `expected_revision`.
///
/// The write half of the arc: a client resolves a gesture on its own surface and sends
/// the result here, which is what turns the user's arrangement into session state. It is
/// an ACTION, not an `intervene` slot, because it is not a plain assignment — the host
/// names the dividers the client minted, validates the shape, and answers with the
/// canonical tree ([`LayoutTree::set_from_wire`](sprag_terminal::LayoutTree::set_from_wire)).
pub const SET_LAYOUT_ACTION: &str = "set_layout";
/// The mux control external invoke action that takes a pane OUT of the tiling or puts it
/// back (`{id, floating}`), returning the resulting
/// [`LayoutSnapshot`](sprag_terminal::LayoutSnapshot).
///
/// Float is session state, not a client's private display concern: a floated pane is one
/// the user took out of the tiling, which is the same class of fact as how the rest are
/// split. WHERE the client then puts that pane's window on screen is pixels, and stays the
/// client's alone.
pub const SET_FLOATING_ACTION: &str = "set_floating";

/// The mux control external query slot: the SCOPED session's windows — each window's name and
/// whether it is the current one (`[{name, current}]`) — [`SESSIONS_SLOT`] one level down. How a
/// tabbed client learns which tabs to draw and which is active.
///
/// SCOPED to the request's session (unlike `sessions`, whose subject is the set of sessions):
/// windows are a property OF a session, so this answers about the one the request named.
pub const WINDOWS_SLOT: &str = "windows";

/// The mux control external query slot: the NAME of the session this request is SCOPED to — one
/// string, the daemon's own answer to "which session is this about".
///
/// Trivial for a request that named its session, and the point for one that did not
/// ([`ScopeAsk::Attached`](sprag_rpc::ScopeAsk::Attached)): a display client scoped to its
/// ATTACHMENT has deliberately stopped holding a name, and a name is still what it must PAINT —
/// the session rail's highlighted row, the palette's "current" mark, the next/previous-session walk,
/// and `sprag-tui`'s terminal title all say which session the user is looking at.
///
/// Before this slot the client cached the name it booted with, and R303 measured what that costs
/// the moment the daemon stopped killing it for a rename: the terminal title stayed `sprag: alpha`
/// for the whole life of a client the daemon was reporting on `production`, and every "is this row
/// me" comparison in the sidebar was against a name no session carried. Mirrored like every other
/// fact a client paints (the poll thread refreshes it on each wake), rather than fetched from the
/// paint path.
///
/// It is deliberately the SCOPE's name and not "this connection's attachment": the scope already
/// resolved the question once, at the door, and a second derivation of one fact is how the two
/// come to disagree.
pub const SESSION_SLOT: &str = "session";

/// The mux control external invoke action that creates a window in the SCOPED session, born with
/// a shell, selects it, and returns its name (`{name?, cmd?, cols?, rows?, cwd?}`) — tmux
/// `new-window`.
///
/// SCOPED (it acts on the request's session), unlike [`NEW_SESSION_ACTION`] which names a session
/// directly. `name` absent ⇒ the lowest free integer; `cmd`/`cols`/`rows`/`cwd` shape the birth
/// pane, exactly as [`NEW_SESSION_ACTION`] — and, like it, no opener. Selecting the new window is session state — every attached
/// client follows it, as tmux does.
pub const NEW_WINDOW_ACTION: &str = "new_window";

/// The REQUEST grammar of [`NEW_WINDOW_ACTION`]'s two window-level keys — [`SelectWindowAsk`]'s
/// sibling, and a TYPE for the reason that one is: the keys are spelled ONCE for the daemon, the
/// CLI verb and the agent surface, so no caller can invent a third spelling.
///
/// # Why it is a type, and why THIS round needed it to be
///
/// The shape pin (`the_wire_shape_is_what_this_protocol_number_stands_for`) renders the BYTES of
/// every grammar this project owns as a type, and that is what keeps [`WIRE_PROTOCOL`] from being a
/// number nobody remembers to move. A key spelled at a `json!` call site is invisible to it —
/// which is exactly the hole R300 found for `select_pane`'s origin, and which this round's own
/// audit found again here: `detached` bumped the protocol 11 → 12 and reverting the bump left the
/// whole suite green, because nothing looked at what a client SENDS.
///
/// The BIRTH SPEC (`cmd` / `cols` / `rows` / `cwd`) is deliberately NOT here: it predates this,
/// it is shared verbatim with [`SPAWN_ACTION`], and pulling it in would move a grammar this round
/// has no reason to touch. What is here is what this round added.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowBirthAsk(pub sprag_terminal::WindowBirth);

impl WindowBirthAsk {
    /// The request key that leaves the session on the window it is already on.
    pub const DETACHED_KEY: &'static str = DETACHED_KEY;
    /// The request key naming the pane whose occupant asked for the window.
    pub const OPENED_BY_KEY: &'static str = WINDOW_OPENED_BY_KEY;

    /// The `args` keys a client sends for this ask, merged into whatever else the request carries.
    ///
    /// A key is emitted only when it says something: the default birth emits NOTHING, so a caller
    /// that wants what every caller wanted before these keys existed sends exactly the bytes it
    /// sent then. That is what makes the addition additive on the wire — and what makes the SKEW
    /// hazard precise rather than general: only a request that ASKS for a detached window can be
    /// misanswered by a daemon that drops the key.
    #[must_use]
    pub fn to_args(&self) -> serde_json::Map<String, Value> {
        let mut map = serde_json::Map::new();
        if self.0.detached {
            map.insert(Self::DETACHED_KEY.to_owned(), Value::Bool(true));
        }
        if let Some(opener) = self.0.opened_by {
            map.insert(Self::OPENED_BY_KEY.to_owned(), Value::from(opener.0));
        }
        map
    }
}

/// The [`NEW_WINDOW_ACTION`] request key that leaves the session on the window it is already on —
/// tmux's `new-window -d`. Absent (or `false`) selects the new window, which is what every caller
/// did before this key existed and what tmux's own default is.
///
/// # Why a window can be born without taking the screen
///
/// Because CREATING a place and SHOWING it are two acts, and only the second is about the person.
/// While they were one act, a caller that is not a person could not make itself a workbench without
/// taking over the user's screen — which is exactly the intrusion R294's authorship gate exists to
/// prevent one level down, arriving through the level above it.
///
/// **This key is why `WIRE_PROTOCOL` moved.** A daemon older than a client that sends it ACCEPTS it
/// and DROPS it (measured at `37d3971`: the window was created and selected anyway), so a caller
/// that believed it had opened a quiet window has moved every attached client, with nothing in the
/// answer to say so. An added ARGUMENT is invisible to `client/hello`; only the version is not.
pub const DETACHED_KEY: &str = "detached";

/// The [`NEW_WINDOW_ACTION`] request key naming the pane whose occupant asked for the window
/// ([`sprag_terminal::Window::opened_by`]) — [`SPAWN_ACTION`]'s key of the same name, one level up,
/// parsed by the same function so a stale pane id is refused the same way.
pub const WINDOW_OPENED_BY_KEY: &str = "opened_by";

/// The mux control external invoke action that makes a window current in the SCOPED session
/// (`{window}` XOR `{relative}`) — tmux `select-window`, `next-window` and `previous-window` in one
/// verb. Session state: every attached client follows.
///
/// It ANSWERS the window it landed on. A caller that named one already knew; a caller that STEPPED
/// could not, and giving both arms the same answer is what lets a client learn where it went rather
/// than infer it from a mirror (R295/R302/R304's rule, one level down).
pub const SELECT_WINDOW_ACTION: &str = "select_window";

/// The REQUEST grammar of [`SELECT_WINDOW_ACTION`] — [`SelectAsk`]'s twin one level up, and a
/// separate type for the same reason that one is: the keys are spelled ONCE for the daemon, the CLI
/// verb and the keybinding, and the XOR lives in the type so no code in this tree can build "a name
/// and a direction".
///
/// # Two arms, not four
///
/// A pane walk is SPATIAL — four ways, and an edge at each end that the answer has to name. A
/// window list is an ORDINAL RING TO WALK: two ways, no ends, and the step always lands — where
/// [`MOVE_WINDOW_ACTION`] treats the same order as a SEQUENCE with a front and a back. That is why
/// [`OrderStep`] is its own vocabulary rather than a reuse of [`PaneDir`], and why this ask has no
/// origin key: a step is always measured from the window the session is CURRENTLY on, because that
/// is the only thing "next" can mean for a ring the session itself walks.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SelectWindowAsk {
    /// `{window}` — make the window with that NAME current. Refused if the session has none.
    Named(String),
    /// `{relative}` — one step along the ring from the current window, WRAPPING. Total: a session
    /// always has a window, so this always lands somewhere and answers its name.
    Step(OrderStep),
}

impl SelectWindowAsk {
    /// The request key naming a window outright.
    pub const WINDOW_KEY: &'static str = "window";
    /// The request key naming which way to step along the ring.
    pub const RELATIVE_KEY: &'static str = "relative";

    /// The `args` object a client sends for this ask.
    ///
    /// The named arm emits exactly the bytes it emitted before the step existed, so the request
    /// every client already sends is unchanged and a reader of a trace tells the two apart by eye
    /// ([`SelectAsk::to_args`]'s rule).
    #[must_use]
    pub fn to_args(&self) -> Value {
        let mut map = Map::new();
        match self {
            Self::Named(window) => {
                map.insert(Self::WINDOW_KEY.to_owned(), Value::from(window.clone()));
            }
            Self::Step(step) => {
                map.insert(Self::RELATIVE_KEY.to_owned(), Value::from(step.wire_str()));
            }
        }
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit — a key
    /// of the wrong type, both namings, or neither.
    ///
    /// One [`None`] for every refusal, as [`SelectAsk::parse`] has and for the same stated reason:
    /// the action answers one error for all of them, and the SURFACES say which one because each of
    /// them knows what it sent.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        let map = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => return None,
        };
        // An explicit `null` reads as absent — the rule the sibling grammar states, so a caller
        // filling in a whole argument struct asks what one omitting the halves asks.
        let field = |key: &str| {
            map.and_then(|map| map.get(key))
                .filter(|value| !value.is_null())
        };
        let named = match field(Self::WINDOW_KEY) {
            None => None,
            Some(value) => Some(value.as_str()?.to_owned()),
        };
        let step = match field(Self::RELATIVE_KEY) {
            None => None,
            Some(value) => Some(OrderStep::from_wire(value.as_str()?)?),
        };
        match (named, step) {
            (Some(window), None) => Some(Self::Named(window)),
            (None, Some(step)) => Some(Self::Step(step)),
            _ => None,
        }
    }
}

/// The mux control external invoke action that makes a pane ACTIVE in the scoped session's current
/// window (`{pane?}` XOR `{dir?}`) — tmux `select-pane`. Answers `{pane, changed}`.
///
/// The mux control external invoke action that moves a window's PLACE in the scoped session's
/// order — tmux `move-window`. Answers `{window, how}`.
///
/// [`SELECT_WINDOW_ACTION`]'s companion, and the two together are why this project draws the
/// distinction its own docs used to leave implicit: **the same collection is a RING to walk and a
/// SEQUENCE to arrange.** The select wraps because attention comes back round; this one stops at
/// the ends, because the order the `windows` slot publishes as an ARRAY — and the strip
/// `sprag-gui` paints from it — has a front and a back.
///
/// The order was, until this verb, **walkable, paintable and unchangeable**: no CLI verb, no key, no
/// wire action anywhere in this tree could move a window past another one.
///
/// # The grammar
///
/// [`MoveWindowAsk`] — `{window?}` plus exactly one of `place` / `before` / `after`. `window`
/// ABSENT means the session's CURRENT window, which is what a keypress means and the default
/// [`SELECT_PANE_ACTION`]'s origin takes for its own reason.
///
/// An anchor is a NAME, never an index. The rival's `tab.move` takes `insert_index`
/// (`src/app/api/tabs.rs:179` at herdr `9a4ce5e1`), a position the CLIENT computes from a list it
/// read earlier — [`sprag_terminal::PaneName`]'s whole argument one level up, since a position
/// silently comes to mean a different slot and the caller cannot tell. A name is resolved under the
/// registry lock at the instant the move happens.
///
/// # The answer: `{window, how}`
///
/// `window` is the window AS RESOLVED, so a caller that omitted it learns which one moved. `how` is
/// [`sprag_terminal::PlaceHow`]'s four words, because "nothing happened" has four causes with four
/// remedies — already in that place, the session holds one window, the anchor was the window
/// itself, or it moved. The rival answers `bool` for the first three at once and then reports
/// SUCCESS with no event (`Workspace::move_tab`, herdr `src/workspace.rs:619`).
///
/// A window that does not exist, or an anchor that does not, is REFUSED rather than answered —
/// R301's rule, kept here so a client cannot read "succeeded" about something absent.
pub const MOVE_WINDOW_ACTION: &str = "move_window";

/// The REQUEST grammar of [`MOVE_WINDOW_ACTION`] — [`SelectWindowAsk`]'s companion, and a type for
/// the same reason: the keys are spelled ONCE for the daemon, the CLI verb and the keybinding, and
/// the XOR lives in the type so no code in this tree can build "before AND last".
///
/// # Why three keys and not one
///
/// `place` carries the four placings that need no name (`"first"` / `"last"` / `"next"` /
/// `"previous"`); `before` and `after` each carry a window name. One key holding either a word or a
/// name would make `{"place": "first"}` ambiguous the day a window is CALLED `first` — and a window
/// may be called that, since [`sprag_terminal::WindowName`] admits it. Separate keys make the
/// ambiguity unrepresentable rather than resolved by precedence.
///
/// The two step words are [`OrderStep`]'s own, not a second pair: it is the same direction the
/// ring walks, and only the WRAP differs between the verbs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MoveWindowAsk {
    /// The window being placed. [`None`] ⇒ the scoped session's CURRENT window.
    pub window: Option<String>,
    /// Where it goes.
    pub place: WindowPlace,
}

impl MoveWindowAsk {
    /// The request key naming the window being placed.
    pub const WINDOW_KEY: &'static str = "window";
    /// The request key carrying a placing that needs no anchor.
    pub const PLACE_KEY: &'static str = "place";
    /// The request key naming the anchor a window goes BEFORE.
    pub const BEFORE_KEY: &'static str = "before";
    /// The request key naming the anchor a window goes AFTER.
    pub const AFTER_KEY: &'static str = "after";
    /// [`WindowPlace::First`]'s word under [`PLACE_KEY`](Self::PLACE_KEY).
    pub const FIRST_WORD: &'static str = "first";
    /// [`WindowPlace::Last`]'s word under [`PLACE_KEY`](Self::PLACE_KEY).
    pub const LAST_WORD: &'static str = "last";

    /// The `args` object a client sends for this ask.
    ///
    /// An absent window emits no key at all rather than a null — [`SelectAsk::to_args`]'s rule, so a
    /// reader of a trace tells "move the current one" from "move that one" by eye.
    #[must_use]
    pub fn to_args(&self) -> Value {
        let mut map = Map::new();
        if let Some(window) = &self.window {
            map.insert(Self::WINDOW_KEY.to_owned(), Value::from(window.clone()));
        }
        let (key, value) = match &self.place {
            WindowPlace::First => (Self::PLACE_KEY, Self::FIRST_WORD.to_owned()),
            WindowPlace::Last => (Self::PLACE_KEY, Self::LAST_WORD.to_owned()),
            WindowPlace::Step(step) => (Self::PLACE_KEY, step.wire_str().to_owned()),
            WindowPlace::Before(anchor) => (Self::BEFORE_KEY, anchor.clone()),
            WindowPlace::After(anchor) => (Self::AFTER_KEY, anchor.clone()),
        };
        map.insert(key.to_owned(), Value::from(value));
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit — a key
    /// of the wrong type, a `place` word this build does not know, no placing at all, or more than
    /// one.
    ///
    /// One [`None`] for every refusal, [`SelectWindowAsk::parse`]'s stated rule: the action answers
    /// one error for all of them and each SURFACE knows what it sent.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        let map = match args {
            Value::Object(map) => Some(map),
            Value::Null => None,
            _ => return None,
        };
        // An explicit `null` reads as absent — the sibling grammar's rule, so a caller filling in a
        // whole argument struct asks what one omitting the halves asks.
        let field = |key: &str| {
            map.and_then(|map| map.get(key))
                .filter(|value| !value.is_null())
        };
        let word = match field(Self::PLACE_KEY) {
            None => None,
            Some(value) => Some(match value.as_str()? {
                Self::FIRST_WORD => WindowPlace::First,
                Self::LAST_WORD => WindowPlace::Last,
                other => WindowPlace::Step(OrderStep::from_wire(other)?),
            }),
        };
        let anchored = |key: &'static str, wrap: fn(String) -> WindowPlace| match field(key) {
            None => Ok(None),
            Some(value) => value
                .as_str()
                .map(|name| Some(wrap(name.to_owned())))
                .ok_or(()),
        };
        let before = anchored(Self::BEFORE_KEY, WindowPlace::Before).ok()?;
        let after = anchored(Self::AFTER_KEY, WindowPlace::After).ok()?;
        let mut named = [word, before, after].into_iter().flatten();
        let place = named.next()?;
        if named.next().is_some() {
            return None;
        }
        let window = match field(Self::WINDOW_KEY) {
            None => None,
            Some(value) => Some(value.as_str()?.to_owned()),
        };
        Some(Self { window, place })
    }

    /// The answer key naming the window that was placed.
    pub const ANSWER_WINDOW_KEY: &'static str = "window";
    /// The answer key carrying [`PlaceHow`]'s word.
    pub const ANSWER_HOW_KEY: &'static str = "how";

    /// The answer a daemon sends: `{window, how}`.
    ///
    /// Built here rather than at the action, so the daemon writing it and every client reading it
    /// agree by construction — the rule [`SwapAsk`] states for its own answer.
    #[must_use]
    pub fn answer(window: &str, how: PlaceHow) -> Value {
        let mut map = Map::new();
        map.insert(
            Self::ANSWER_WINDOW_KEY.to_owned(),
            Value::from(window.to_owned()),
        );
        map.insert(Self::ANSWER_HOW_KEY.to_owned(), Value::from(how.wire_str()));
        Value::Object(map)
    }

    /// Read that answer back, or [`None`] if it is not one — a daemon too old to know this verb,
    /// or a word this build's [`PlaceHow`] does not have.
    #[must_use]
    pub fn read_answer(value: &Value) -> Option<(String, PlaceHow)> {
        let window = value.get(Self::ANSWER_WINDOW_KEY)?.as_str()?.to_owned();
        let how = PlaceHow::from_wire(value.get(Self::ANSWER_HOW_KEY)?.as_str()?)?;
        Some((window, how))
    }
}

/// The pane half of [`SELECT_WINDOW_ACTION`], and session state for the same reason: which pane a
/// user is ON outlives any one client, so every attached client follows it and a reattaching one
/// inherits it. Before this the daemon had no such concept and each display client kept its own
/// private answer, which is why nothing that draws nothing — an agent, a shell — could say "here".
///
/// Two ways to NAME the target, and exactly one of them per request (the shape
/// [`RESIZE_WINDOW_ACTION`] uses for its four) — the grammar is [`SelectAsk`], which is where the
/// three keys below are spelled and the only place a client builds them:
///
/// * `pane` — that pane, which must be one of the current window's. A FLOATING pane is a legal
///   target: it is still a pane of the window and still takes input.
/// * `dir` — `"left"` / `"right"` / `"up"` / `"down"`: one step that way (tmux's `select-pane
///   -L/-R/-U/-D`), resolved by [`LayoutTree::neighbor`](sprag_terminal::LayoutTree::neighbor) from
///   the ARRANGEMENT rather than from any client's rectangles.
/// * `from` — the pane that step is measured FROM, and only alongside `dir`. Absent ⇒ the pane that
///   is active NOW, which is what a keypress means and what the CLI and the keybinding always mean.
/// * neither `pane` nor `dir`, or both, or `from` without `dir` ⇒ `TypeMismatch`. "Select nothing",
///   "select two things" and "step from here toward nowhere" are not requests with an obvious
///   reading, and guessing one would make a client's bug silent.
///
/// **A direction with no neighbour is not an error.** The answer is the active pane unmoved: a key
/// bound to `select-pane -L` pressed at the left edge is a well-formed request whose honest answer is
/// "nothing to move to", and refusing it would log a failure every time a user reached the edge of
/// their layout. A `pane` that names no pane of the current window IS refused (`Rejected`) — the rule
/// [`SPLIT_ACTION`] already applies to its target — **and so is a `from` that names one**, because an
/// origin the window does not hold is the same mistake one argument over. It is refused rather than
/// answered [`Untiled`](SelectHow::Untiled): a pane of another window is not "in no arrangement", it
/// is in one this request cannot see, and a caller told the floating story would go looking for a
/// float that is not there.
///
/// # Why an ORIGIN belongs on the wire and not in the caller
///
/// Because the caller that wants one cannot compute it. "Put the user on the pane left of THAT one"
/// is a layout read at one instant and a select at another — the two-instant join the `dir` arm
/// exists to remove, rebuilt the moment the origin stops being the active pane. The walk is free
/// here: it happens under the lock that is already held, on the arrangement the daemon already owns.
///
/// It does NOT break the arm's own rule that *neither end is the client's fact*. What a client may
/// not supply is a POSITION — adjacency derived from its rectangles, which is where the rival's
/// answer comes from. An IDENTITY is different: a pane id is the client's to hold (a process inside
/// a pane reads its own from `SPRAG_PANE`), and naming one says nothing about where it sits.
///
/// # The answer: `{pane, changed, outcome}`
///
/// `pane` is the pane the window is ON afterwards, which a caller adopts either way, and `changed`
/// says whether that differs from before. `outcome` names WHY it is that pane, in [`SelectHow`]'s
/// four words — because `changed: false` alone reads the same for a re-select of the pane the session
/// was already on and for a direction that had nowhere to go, and those need opposite sentences. A
/// caller that only has to project the answer reads `pane`; one that has to SAY what happened reads
/// `outcome`.
pub const SELECT_PANE_ACTION: &str = "select_pane";

/// The REQUEST grammar of [`SELECT_PANE_ACTION`] — what a caller may ask, as a type that cannot
/// spell the combinations the action refuses.
///
/// The daemon [`parse`](Self::parse)s one of these and every client [`to_args`](Self::to_args)
/// builds one, so the three keys are spelled ONCE for four surfaces (the daemon, the CLI verb, the
/// MCP tool, the keybinding). Before it each end wrote its own `{"dir": …}` literal, which is the
/// shape R292 removed from the event filter for the same reason: a wire word spelled twice is a wire
/// word that can drift.
///
/// The XOR is in the type rather than in a validator, so a client CANNOT construct "a pane and a
/// direction" or "an origin with nowhere to go" — the daemon still refuses those, because they can
/// arrive from something that is not this type, but no code in this tree can produce one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SelectAsk {
    /// `{pane}` — make THAT pane active. It must be one of the scoped window's, floating or tiled.
    Pane(PaneId),
    /// `{dir, from?}` — one step `dir` from `from`, or from the ACTIVE pane when `from` is absent.
    Toward {
        /// Which way to step.
        dir: PaneDir,
        /// The pane the step starts at. [`None`] ⇒ the active pane — what a keypress means.
        from: Option<PaneId>,
    },
}

impl SelectAsk {
    /// The request key naming a pane to select outright.
    pub const PANE_KEY: &'static str = "pane";
    /// The request key naming which way to step.
    pub const DIR_KEY: &'static str = "dir";
    /// The request key naming the pane a step is measured FROM.
    pub const FROM_KEY: &'static str = "from";

    /// The `args` object a client sends for this ask.
    ///
    /// A [`Toward`](Self::Toward) with no origin emits exactly the bytes it emitted before origins
    /// existed — the key is absent, not null — so the commonest request on this action is unchanged
    /// on the wire and a reader of a trace can still tell the two asks apart by eye.
    #[must_use]
    pub fn to_args(self) -> Value {
        let mut map = Map::new();
        match self {
            Self::Pane(pane) => {
                map.insert(Self::PANE_KEY.to_owned(), Value::from(pane.0));
            }
            Self::Toward { dir, from } => {
                map.insert(Self::DIR_KEY.to_owned(), Value::from(dir.wire_str()));
                if let Some(from) = from {
                    map.insert(Self::FROM_KEY.to_owned(), Value::from(from.0));
                }
            }
        }
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit.
    ///
    /// One [`None`] for every refusal because the action answers one error for all of them
    /// (`TypeMismatch`): a key of the wrong type, both namings, neither, and an origin with no
    /// direction are the same class of caller bug, and `InvokeError` carries no payload to tell them
    /// apart with anyway (upstream PINION-PR82). The SURFACES say which one, because each of them
    /// knows what it sent.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        let map = match args {
            Value::Object(map) => Some(map),
            // A request with no args at all is not malformed JSON, it is the empty ask — which this
            // grammar does not admit either, one line down.
            Value::Null => None,
            _ => return None,
        };
        // An explicit `null` reads as absent, so a client that fills its whole argument struct in
        // (and leaves the optional halves null) asks the same thing as one that omits them.
        let field = |key: &str| {
            map.and_then(|map| map.get(key))
                .filter(|value| !value.is_null())
        };
        let pane_id = |key: &str| match field(key) {
            None => Ok(None),
            Some(value) => value.as_u64().map(|id| Some(PaneId(id))).ok_or(()),
        };
        let pane = pane_id(Self::PANE_KEY).ok()?;
        let from = pane_id(Self::FROM_KEY).ok()?;
        let dir = match field(Self::DIR_KEY) {
            None => None,
            Some(value) => Some(PaneDir::from_wire(value.as_str()?)?),
        };
        match (pane, dir, from) {
            (Some(pane), None, None) => Some(Self::Pane(pane)),
            (None, Some(dir), from) => Some(Self::Toward { dir, from }),
            _ => None,
        }
    }

    /// The direction this ask stepped in, if it stepped — what [`SelectHow::read`] needs to read a
    /// pre-`outcome` answer, and what a surface needs to word its own sentence.
    #[must_use]
    pub fn toward(self) -> Option<PaneDir> {
        match self {
            Self::Pane(_) => None,
            Self::Toward { dir, .. } => Some(dir),
        }
    }

    /// The pane this ask measured its step from, when the caller named one.
    #[must_use]
    pub fn origin(self) -> Option<PaneId> {
        match self {
            Self::Pane(_) => None,
            Self::Toward { from, .. } => from,
        }
    }
}

sprag_terminal::closed_set! {
    // `ALL` is GENERATED with this enum from ONE variant list, so it cannot be missing a
    // variant and its length cannot disagree with its contents — see `closed_set!`. The
    // hand-written array it replaces was checked by nothing, which three register items
    // said and none closed (R299/R301/R310).
    /// The `outcome` key of a [`SELECT_PANE_ACTION`] answer: why the session is on the pane it names.
    ///
    /// **Four words, total over the request grammar, each with exactly one cause** — the property that
    /// makes an operator-facing or agent-facing sentence exact rather than a list of possibilities
    /// ([`sprag_terminal::ZoomOutcome`] states the same rule for the zoom's two bools). A `pane` request
    /// can only [`Moved`](Self::Moved) or find itself [`AlreadyActive`](Self::AlreadyActive); a `dir`
    /// request can also stop [`AtEdge`](Self::AtEdge) or be measured from an
    /// [`Untiled`](Self::Untiled) pane.
    ///
    /// A `dir` request reaches `AlreadyActive` only by naming an ORIGIN
    /// ([`SelectAsk::Toward::from`]) whose neighbour is the pane the session is already on — the cause
    /// is the same one word for one fact ("the pane it resolved to is the pane it was on"), and the
    /// arms that can produce it grew rather than the vocabulary.
    ///
    /// # Why the daemon says it instead of the caller deriving it
    ///
    /// Three of the four ARE derivable by a caller that remembers which arm it asked
    /// ([`read`](Self::read) does exactly that for a daemon too old to answer this key). The fourth is
    /// not: telling "nothing that way" from "the pane you are on is floating, so there is no that-way"
    /// takes the arrangement, and a client that read the arrangement to explain its own move would join
    /// two instants to describe one — the torn read the whole directional arm exists to remove.
    ///
    /// The rival spends one word here (`PaneFocusDirectionReason::NoNeighbor`, herdr `9a4ce5e1`) and
    /// reports it for both cases: `directional_pane_target` looks the source pane up in the rects of its
    /// last composed frame and answers `None` when it is absent, exactly as it does at an edge.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum SelectHow {
        /// The active pane MOVED to the pane the answer names.
        Moved,
        /// The request RESOLVED to the pane the session was already on — a `pane` naming it (a
        /// re-select, which is a legitimate no-op rather than a failure: a client publishing the focus
        /// it already shows), or a `dir` whose step from a named origin landed back on it.
        AlreadyActive,
        /// A `dir` request whose origin the arrangement holds, with nothing that way: the window's edge.
        AtEdge,
        /// A `dir` request whose ORIGIN the arrangement holds NO LEAF for — it is floating, so it has no
        /// neighbours in any direction. That origin is the active pane unless the request named one.
        /// Distinct from [`AtEdge`](Self::AtEdge) on purpose: the remedy is different (dock it, or
        /// select by name), and an edge is a boundary the movement ran into where this is a request with
        /// no adjacency to walk at all.
        Untiled,
    }
}

impl SelectHow {
    /// This outcome's wire word — the value of the answer's `outcome` key.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Moved => "moved",
            Self::AlreadyActive => "already_active",
            Self::AtEdge => "at_edge",
            Self::Untiled => "untiled",
        }
    }

    /// The outcome a wire word names, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|how| how.wire_str() == word)
    }

    /// Whether the active pane MOVED — the answer's `changed` key, spelled once.
    ///
    /// The daemon announces on exactly this, because a select that moved nothing gives a parked
    /// client nothing to re-read.
    #[must_use]
    pub fn changed(self) -> bool {
        matches!(self, Self::Moved)
    }

    /// Read the outcome of an answer, from any daemon — including one that does not carry the key.
    ///
    /// `toward` is the direction the caller asked for, if it asked for one — which is what makes the
    /// derivation exact rather than a guess. `{pane, changed}` plus the arm the caller chose
    /// determine three of the four words: a `pane` request that changed nothing was `AlreadyActive`,
    /// and a `dir` request that changed nothing went nowhere. Only the reason it went nowhere is
    /// unrecoverable, and the answer says the honest thing rather than the specific one
    /// ([`AtEdge`](Self::AtEdge), which is the case a user meets; a floating pane needs a client that
    /// floated it).
    ///
    /// One reader for every client, so a degraded sentence is decided here rather than re-derived
    /// per surface.
    ///
    /// # It stopped being a SKEW path when the origin arrived
    ///
    /// It was written as one: the `outcome` key was ADDITIVE, so R299 shipped without moving
    /// [`WIRE_PROTOCOL`] and a new CLIENT still had to say something sensible to an old daemon. The
    /// origin argument is not additive in that way — an old daemon would ACCEPT it, DROP it, and move
    /// the user from the wrong pane — so the number moved, and a daemon that omits `outcome` is
    /// now refused by name before a request reaches it.
    ///
    /// What is left is a TOTAL function's default: `read` must answer for any value, and this is the
    /// answer it gives. The `(dir, unchanged)` branch assumes the ask carried no origin, which the
    /// protocol number is now what makes safe — an answer old enough to lack `outcome` cannot have
    /// come from a request new enough to carry `from`.
    #[must_use]
    pub fn read(answer: &Value, toward: Option<PaneDir>) -> Self {
        if let Some(how) = answer[OUTCOME_KEY].as_str().and_then(Self::from_wire) {
            return how;
        }
        match (answer["changed"].as_bool().unwrap_or(false), toward) {
            (true, _) => Self::Moved,
            (false, None) => Self::AlreadyActive,
            (false, Some(_)) => Self::AtEdge,
        }
    }
}

/// The arguments of [`NEIGHBORS_FIELD`] — one pane `id`, `Open` for [`PROJECT_ARGS`]' reason.
const NEIGHBORS_ARGS: &[SchemaArg] = &[SchemaArg::open("pane", "int")];

/// The mux control external query slot: what is ADJACENT to one pane in the scoped session's
/// current window — `{left, right, up, down}`, each a pane id or `null`.
///
/// **`null` IS the edge**, and that is the whole design. herdr spends two methods here
/// (`pane.neighbor` returns a pane, `pane.edges` returns four booleans) and derives them by two
/// different routes — a rect-versus-area comparison and a walk over the other panes' rects — so
/// nothing makes the two agree. Here they are one derivation and cannot disagree: a pane with no
/// neighbour to its left IS a pane at the window's left edge.
///
/// Answered from the ARRANGEMENT ([`LayoutTree::neighbor`](sprag_terminal::LayoutTree::neighbor)),
/// so it is the same answer for every client, at every size, and with no client attached at all —
/// the rival computes it from the rectangles of its last composed FRAME, which moves with a
/// sidebar, a tab bar and cell rounding.
///
/// All four are `null` for a pane the current window's tiling does not hold: one that has exited,
/// one in another window, or one a client has FLOATED out. A floating pane can still be the ACTIVE
/// pane; it is simply not in the arrangement adjacency is a property of.
pub const NEIGHBORS_FIELD: SchemaField =
    SchemaField::parametric("neighbors.<pane>", "object", NEIGHBORS_ARGS);

/// The mux control external invoke action that renames a window of the SCOPED session
/// (`{window?, name}`) — tmux `rename-window`. `window` absent ⇒ the current one; `name` is the
/// new name.
pub const RENAME_WINDOW_ACTION: &str = "rename_window";

/// The mux control external invoke action that renames the SCOPED session (`{name}`) — tmux
/// `rename-session`. `name` is the new name; the session renamed is the request's own scope, so
/// there is no target argument to disagree with it.
///
/// # This one moves an ADDRESS, which no other rename here does
///
/// A window name addresses a window inside its session and a pane name stands in for an id, but a
/// session name is what every scoped request, every `-t` and every attached client holds. So the
/// daemon carries three things across with it in one act: the registry entry, the session's change
/// CHANNEL (`crate::notify::ChannelRegistry::rename` — its revision token, its journal, and every
/// parked wait), and the ATTACHMENTS
/// (`crate::AttachmentRegistry::rename_session`). Renaming only the first would leave every parked
/// client waiting on a key nothing reaches again.
///
/// The change funnel reports it as ONE [`Event::SessionRenamed`](crate::events::Event::SessionRenamed)
/// rather than a death and a birth, which is what a session's IDENTITY
/// ([`SessionId`]) exists for.
pub const RENAME_SESSION_ACTION: &str = "rename_session";

/// The mux control external invoke action that kills a window of the SCOPED session (`{window?}`)
/// — tmux `kill-window`. `window` absent ⇒ the current one. Killing the session's LAST window
/// ends the SESSION (and the last session ends the daemon), tmux's "kill the last window ⇒ the
/// session is gone".
pub const KILL_WINDOW_ACTION: &str = "kill_window";

/// The mux control external invoke action that PINS the size of a window of the SCOPED session
/// (`{window?, cols?, rows?, adjust_cols?, adjust_rows?, from?}`) — tmux `resize-window`. `window`
/// absent ⇒ the current one.
///
/// Four ways to NAME one rectangle, and exactly one of them per request:
///
/// * `cols` + `rows` — that rectangle (tmux `-x`/`-y`). Both or neither; half is refused.
/// * `adjust_cols` / `adjust_rows` — SIGNED, relative to the window's current size (tmux
///   `-L`/`-R`/`-U`/`-D`). Either alone leaves the other edge.
/// * `from` — a `window-size` policy name to fold the attached clients under (tmux `-a`/`-A`).
/// * none of them — un-pin.
///
/// The last three are DESCRIPTIONS, not rectangles, and the daemon resolves them
/// (`sprag_host::window::SizeRequest`) because their inputs — the window's current size and the
/// clients' reported areas — are facts only it holds. A caller reading those back to do the
/// arithmetic itself would be a second geometry model in a client, which is the defect
/// `sprag_host::window` exists to remove.
///
/// It writes a stored fact and nothing else: whether the pinned size is what the panes are laid out
/// over is the `window-size` option's answer ([`crate::options::WINDOW_SIZE`]), which the daemon
/// reads from the user's file. So this action never becomes a way to set an option over the wire —
/// the invariant [`crate::options`] states — and pinning a size before choosing to use it is a
/// legal order rather than an error.
///
/// Unlike [`RESIZE_ACTION`] this touches no PTY directly. The panes follow because every mux action
/// re-derives the session's window at the invoke BOUNDARY, which is the same path a split or a
/// client's attach takes — one derivation, thirteen callers, now fourteen.
pub const RESIZE_WINDOW_ACTION: &str = "resize_window";

/// The mux control external invoke action that BREAKS a pane out of its window into a new window
/// of the SCOPED session, born current, and returns its name (`{pane, name?}`) — tmux `break-pane`.
///
/// `pane` is the id of the pane to move; its source window is DERIVED (a [`PaneId`]
/// is registry-unique, so the window that holds it is unambiguous — the caller never names the
/// source). `name` absent ⇒ the lowest free integer window name. Refused (`Rejected`) if the pane's
/// window tiles only that one pane, if an explicit `name` is taken, or if no window holds `pane`.
pub const BREAK_PANE_ACTION: &str = "break_pane";

/// The mux control external invoke action that JOINS a pane into another window of the SCOPED
/// session (`{pane, window}` XOR `{pane, window_id}`) — tmux `join-pane`. Answers
/// `{closed_source: bool}`.
///
/// `pane` is the id of the pane to move (its source window is DERIVED); the destination is named by
/// the grammar [`JoinAsk`]. The pane appends as a new tiled leaf; a join that empties the source
/// window closes it (`closed_source: true`). Refused (`Rejected`) if `pane` already lives in the
/// destination, if no window holds `pane`, or if the destination resolves to nothing.
///
/// # Why the destination has two spellings
///
/// Because a caller can know two different things and only one of them is a name — the split
/// [`sprag_terminal::Session::join_pane_into`] states, and R304's sentence at the level a join
/// commits. A person who TYPES a name means whatever holds it when they press Enter; a person who
/// PICKS a row read an identity out of a list that was already a fact about the past. Spelling the
/// second as a name was MEASURED landing a join in a window nobody chose, and it was the only
/// spelling this action had.
///
/// `window_id` is ADDED, so an older daemon that never knew it refuses the request rather than
/// dropping the key and joining somewhere — the loud half of the rule [`DETACHED_KEY`] states,
/// which is why [`WIRE_PROTOCOL`] does not move for it.
pub const JOIN_PANE_ACTION: &str = "join_pane";

/// **WHICH window a request is about** — a NAME the daemon resolves NOW, or the IDENTITY a client
/// PICKED off a list it painted earlier. The one grammar every window-addressing request shares.
///
/// # Why two addresses is the answer and not a wart
///
/// Because a caller can honestly know two different things, and only one of them is a name. A
/// person who TYPES `sprag kill-window -t s build` means whatever holds that name at the instant
/// they press Enter — resolving it any later would be second-guessing them. A person who CLICKS
/// `Kill window 'build'` means the window on the row they read, and the row is a fact about the
/// PAST: R304's sentence, and between the paint and the click sits a confirmation dialog.
///
/// Both readings were MEASURED at the registry
/// (`a_kill_lands_on_the_window_pointed_at_and_a_name_lands_on_whatever_holds_it`) and they land on
/// DIFFERENT windows across a rename. So the request has to say which it meant, and the type is
/// what stops a client holding an identity from sending the label beside it.
///
/// # Why one type and not one per verb
///
/// R329 built this XOR inside `JoinAsk` and R330 needed it again for the kill — the point at which
/// a second copy becomes the thing this project's grammars exist to prevent. Hoisted, so `window`
/// and `window_id` are spelled ONCE for every verb that addresses a window, and a third verb costs
/// a [`read`](Self::read) call rather than a decision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum WindowRef {
    /// `{window: "build"}` — whatever window carries that name when the request arrives.
    Named(String),
    /// `{window_id: 7}` — that window, or none.
    Picked(WindowId),
}

impl WindowRef {
    /// The request key addressing a window by NAME.
    pub const WINDOW_KEY: &'static str = "window";
    /// The request key addressing a window by IDENTITY.
    pub const WINDOW_ID_KEY: &'static str = "window_id";

    /// Write this reference into a request's `args` map.
    pub fn write(&self, map: &mut Map<String, Value>) {
        let (key, value) = match self {
            Self::Named(name) => (Self::WINDOW_KEY, Value::from(name.clone())),
            Self::Picked(window) => (Self::WINDOW_ID_KEY, Value::from(window.0)),
        };
        map.insert(key.to_owned(), value);
    }

    /// The reference `map` carries: [`None`] for neither key (the caller means whatever the request
    /// is SCOPED to), [`Err`] for both at once or for a key of the wrong type.
    ///
    /// The error is a NAMED type rather than `()` so a caller reading this signature learns what
    /// went wrong is a malformed reference and not an absent one — the same distinction the three
    /// outcomes exist to draw.
    ///
    /// An explicit `null` reads as ABSENT — [`SwapAsk::parse`]'s rule, so a client filling in a
    /// whole argument struct asks what one omitting the optional halves asks.
    ///
    /// # Errors
    ///
    /// [`MalformedWindowRef`] for a malformed or doubly-spelled reference; the caller turns that
    /// into the one `TypeMismatch` its action answers.
    pub fn read(map: &Map<String, Value>) -> Result<Option<Self>, MalformedWindowRef> {
        let field = |key: &str| map.get(key).filter(|value| !value.is_null());
        let named = match field(Self::WINDOW_KEY) {
            None => None,
            Some(value) => Some(value.as_str().ok_or(MalformedWindowRef)?.to_owned()),
        };
        let picked = match field(Self::WINDOW_ID_KEY) {
            None => None,
            Some(value) => Some(WindowId(value.as_u64().ok_or(MalformedWindowRef)?)),
        };
        match (named, picked) {
            (None, None) => Ok(None),
            (Some(name), None) => Ok(Some(Self::Named(name))),
            (None, Some(window)) => Ok(Some(Self::Picked(window))),
            // A name AND an identity is two destinations, which is not a request this daemon can
            // honour — and guessing one would hide a client bug at the end that can least afford it.
            (Some(_), Some(_)) => Err(MalformedWindowRef),
        }
    }
}

/// A [`WindowRef`] a request could not name: a key of the wrong type, or a NAME and an IDENTITY at
/// once.
///
/// One value for both, [`SwapAsk::parse`]'s rule and for its reason: the action answers a single
/// `TypeMismatch` for every malformed request, because `InvokeError` carries no payload to tell
/// them apart, and the SURFACES say which one because each knows what it sent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MalformedWindowRef;

/// The REQUEST grammar of [`JOIN_PANE_ACTION`] — the pane to move and where it goes.
///
/// [`SwapAsk`]'s shape one verb over and for its reason: the daemon [`parse`](Self::parse)s one of
/// these and every client [`to_args`](Self::to_args) builds one, so the keys are spelled ONCE for
/// the daemon, the CLI verb, the GUI menu and the keybinding. The destination is a [`WindowRef`],
/// which is where the XOR lives.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct JoinAsk {
    /// The pane to move; its source window is derived from the id.
    pub pane: PaneId,
    /// Where it goes.
    pub window: WindowRef,
}

impl JoinAsk {
    /// The request key naming the pane to move.
    pub const PANE_KEY: &'static str = "pane";

    /// The `args` object a client sends for this ask.
    #[must_use]
    pub fn to_args(&self) -> Value {
        let mut map = Map::new();
        map.insert(Self::PANE_KEY.to_owned(), Value::from(self.pane.0));
        self.window.write(&mut map);
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit —
    /// [`SwapAsk::parse`]'s rule, including that an explicit `null` reads as ABSENT.
    ///
    /// A join with NO destination is refused here rather than defaulted to the scoped window: that
    /// would be a request to move a pane into the window it is probably already in, whose only
    /// answer is `SameWindow`.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        let map = match args {
            Value::Object(map) => map,
            _ => return None,
        };
        let pane = PaneId(
            map.get(Self::PANE_KEY)
                .filter(|value| !value.is_null())?
                .as_u64()?,
        );
        let window = WindowRef::read(map).ok()??;
        Some(Self { pane, window })
    }
}

/// The mux control external invoke action that PLACES an existing pane beside another
/// (`{pane, target, dir, before?}`) — tmux `move-pane`. Answers `{closed_source: bool}`.
///
/// This is to [`JOIN_PANE_ACTION`] exactly what [`SPLIT_ACTION`] is to [`SPAWN_ACTION`]: the same
/// move with a PLACE. `join_pane` appends, which states where only by convention, so putting a pane
/// at a chosen position meant rewriting the whole tree ([`SET_LAYOUT_ACTION`]) — which an author
/// with pixels and a gesture can do, and a shell script or an agent cannot.
///
/// **NEITHER window is named**, and that is the design rather than a shorthand. A
/// [`PaneId`] is registry-unique, so `pane` implies its source window (the
/// rule [`BREAK_PANE_ACTION`] already states) and `target` implies its destination. One request
/// therefore covers both a re-placement inside one window and a move into another, with no mode
/// flag: whether the two windows differ is an observation about the two ids, never a choice the
/// caller has to spell. The rival needs two methods for this and still leaves a hole between them —
/// herdr's `pane.swap` refuses to cross a tab and its `pane.move` refuses to stay inside one, so
/// moving a pane WITHIN its own tab is expressible in neither.
///
/// `dir` and `before` are [`SPLIT_ACTION`]'s, verbatim: `"horizontal"` puts the moved pane RIGHT of
/// `target` and `"vertical"` BELOW it, and `before` (default `false`) puts it on the other side
/// instead. One vocabulary spans placing a NEW pane and placing an existing one, so a caller who can
/// split can move.
///
/// `closed_source` reports whether the move emptied and therefore CLOSED the pane's old window
/// (tmux's behaviour, and [`JOIN_PANE_ACTION`]'s answer) — always `false` for a within-window move,
/// which is the honest value rather than an absent field.
///
/// REFUSED (`Rejected`), with nothing moved: `pane` or `target` naming no pane of the scoped
/// session, `target` not being TILED where it lives (it exited, or a client floated it out), or the
/// two being the SAME pane — a pane cannot be placed beside itself, and unlike a swap that request
/// has no reading at all.
pub const MOVE_PANE_ACTION: &str = "move_pane";

/// The mux control external invoke action that EXCHANGES two panes' positions
/// (`{pane?, with}` XOR `{pane?, dir}`) — tmux `swap-pane`. Answers `{a, b, changed, outcome}`.
///
/// The one arrangement gesture that is not a placement: a placement names where a pane goes, while a
/// swap names only that two panes trade, and the shapes they trade into are whatever each already
/// had. Every division keeps its id, direction and ratio — by construction, because the two leaves
/// are exchanged where they sit rather than removed and put back.
///
/// The grammar is [`SwapAsk`], which is where the three keys are spelled and the only place a client
/// builds them — [`SELECT_PANE_ACTION`]'s rule one verb over, and for its reason.
///
/// `pane` ABSENT means the current window's ACTIVE pane, the default [`SPLIT_ACTION`] takes and for
/// the same reason. It is this action's ORIGIN, the thing [`SelectAsk::Toward::from`] is for the
/// select: the pane being placed, which a direction is measured from. Then exactly one of:
///
/// * `with` — that pane, which may live in ANOTHER window of the session. herdr refuses a
///   cross-tab swap outright (`PaneSwapReason::CrossTab`); sprag allows it because
///   [`MOVE_PANE_ACTION`] already crosses a window, and a swap that could not would be the same
///   asymmetry in the other verb. Each window's ACTIVE pane then follows the CELL — it lands on the
///   pane that arrived, since the one it was on has left.
/// * `dir` — `"left"` / `"right"` / `"up"` / `"down"`: the neighbour of `pane`, resolved by
///   [`LayoutTree::step`](sprag_terminal::LayoutTree::step) from the ARRANGEMENT rather than
///   from any client's rectangles, exactly as [`SELECT_PANE_ACTION`] resolves its own. Same-window
///   by construction — adjacency is a property of one tiling.
/// * neither, or both ⇒ `TypeMismatch`, [`SELECT_PANE_ACTION`]'s rule.
///
/// **A direction with no neighbour is not an error**, and neither is a pane swapped with itself.
/// Both answer `changed: false` with the arrangement unmoved, for [`SELECT_PANE_ACTION`]'s reason:
/// a key bound to `swap-pane -L` pressed at the left edge is a well-formed request whose honest
/// answer is "nothing to trade with", and refusing it would log a failure every time a user reaches
/// the edge of their layout.
///
/// **A pane id that names no pane of the SESSION is refused (`Rejected`) — in BOTH arms.** The
/// direction arm used to answer `{a: <that id>, b: null, changed: false}` for one, which is a
/// success sentence about a pane that does not exist; measured at `a7375f4`, `{pane: 999,
/// dir: "left"}` answered exactly that while `{pane: 0, with: 999}` one key over was refused and
/// [`SELECT_PANE_ACTION`]'s own origin was refused too. One verb disagreeing with itself and with
/// its twin is three readings of one rule.
///
/// # The answer: `{a, b, changed, outcome}`
///
/// `a` and `b` are the two panes AS RESOLVED, so a `dir` caller learns who it swapped with; `b` is
/// `null` when a direction found nothing. `outcome` names WHY, in [`SwapHow`]'s four words, because
/// `b: null` alone reads the same for an edge and for an origin the arrangement does not hold —
/// facts with different remedies, which a caller cannot tell apart without a second read at a
/// second instant. Measured at `a7375f4`: an edge and a FLOATING origin answered the same bytes,
/// `{"a":N,"b":null,"changed":false}`.
pub const SWAP_PANE_ACTION: &str = "swap_pane";

/// The REQUEST grammar of [`SWAP_PANE_ACTION`] — what a caller may ask, as a type that cannot spell
/// the combinations the action refuses.
///
/// [`SelectAsk`]'s shape one verb over, for its reason: the daemon [`parse`](Self::parse)s one of
/// these and every client [`to_args`](Self::to_args) builds one, so the three keys are spelled ONCE
/// for four surfaces (the daemon, the CLI verb, the MCP tool, the keybinding). Before it the daemon
/// read the keys one at a time and the CLI hand-built a `json!` — the fifth-spelling shape R300
/// removed from the select while leaving it standing here.
///
/// **The ORIGIN is a field of both arms rather than a third variant**, which is where this differs
/// from [`SelectAsk`]. There a `from` without a `dir` has no reading at all (a target names itself);
/// here `pane` is the pane BEING PLACED and both partners can be named against it, so "swap pane 3
/// with pane 5" and "swap pane 3 with whatever is left of it" are one question asked two ways.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwapAsk {
    /// `{pane?, with}` — trade `pane` with THAT pane, which may live in another window.
    With {
        /// The pane being placed. [`None`] ⇒ the scoped window's active pane.
        pane: Option<PaneId>,
        /// The pane it trades places with.
        with: PaneId,
    },
    /// `{pane?, dir}` — trade `pane` with its neighbour that way, within its own window.
    Toward {
        /// The pane being placed, and the pane the step is measured FROM. [`None`] ⇒ the scoped
        /// window's active pane, which is what a keypress means.
        pane: Option<PaneId>,
        /// Which way to look for the partner.
        dir: PaneDir,
    },
}

impl SwapAsk {
    /// The request key naming the pane being placed — this action's ORIGIN.
    pub const PANE_KEY: &'static str = "pane";
    /// The request key naming the pane to trade with outright.
    pub const WITH_KEY: &'static str = "with";
    /// The request key naming which way to look for the partner.
    pub const DIR_KEY: &'static str = "dir";

    /// The `args` object a client sends for this ask.
    ///
    /// An absent origin emits no key at all rather than a null, [`SelectAsk::to_args`]'s rule: the
    /// commonest request on this action is unchanged on the wire and a reader of a trace can still
    /// tell the two asks apart by eye.
    #[must_use]
    pub fn to_args(self) -> Value {
        let mut map = Map::new();
        let (pane, key, value) = match self {
            Self::With { pane, with } => (pane, Self::WITH_KEY, Value::from(with.0)),
            Self::Toward { pane, dir } => (pane, Self::DIR_KEY, Value::from(dir.wire_str())),
        };
        if let Some(pane) = pane {
            map.insert(Self::PANE_KEY.to_owned(), Value::from(pane.0));
        }
        map.insert(key.to_owned(), value);
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit.
    ///
    /// One [`None`] for every refusal, [`SelectAsk::parse`]'s rule and for its reason: the action
    /// answers one error for all of them (`TypeMismatch`), because `InvokeError` carries no payload
    /// to tell them apart with (upstream PINION-PR82) and the SURFACES say which one, each knowing
    /// what it sent.
    ///
    /// An explicit `null` reads as ABSENT — the same rule, so a client that fills its whole argument
    /// struct in asks what one that omits the optional halves asks.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        let map = match args {
            Value::Object(map) => Some(map),
            // No args at all is the empty ask, which this grammar does not admit either.
            Value::Null => None,
            _ => return None,
        };
        let field = |key: &str| {
            map.and_then(|map| map.get(key))
                .filter(|value| !value.is_null())
        };
        let pane_id = |key: &str| match field(key) {
            None => Ok(None),
            Some(value) => value.as_u64().map(|id| Some(PaneId(id))).ok_or(()),
        };
        let pane = pane_id(Self::PANE_KEY).ok()?;
        let with = pane_id(Self::WITH_KEY).ok()?;
        let dir = match field(Self::DIR_KEY) {
            None => None,
            Some(value) => Some(PaneDir::from_wire(value.as_str()?)?),
        };
        match (with, dir) {
            (Some(with), None) => Some(Self::With { pane, with }),
            (None, Some(dir)) => Some(Self::Toward { pane, dir }),
            _ => None,
        }
    }

    /// The pane being placed, when the caller named one — [`None`] ⇒ the active pane.
    #[must_use]
    pub fn origin(self) -> Option<PaneId> {
        match self {
            Self::With { pane, .. } | Self::Toward { pane, .. } => pane,
        }
    }

    /// The direction this ask looked in, if it looked — what [`SwapHow::read`] needs to read a
    /// pre-`outcome` answer, and what a surface needs to word its own sentence.
    #[must_use]
    pub fn toward(self) -> Option<PaneDir> {
        match self {
            Self::With { .. } => None,
            Self::Toward { dir, .. } => Some(dir),
        }
    }
}

sprag_terminal::closed_set! {
    // `ALL` is GENERATED with this enum from ONE variant list, so it cannot be missing a
    // variant and its length cannot disagree with its contents — see `closed_set!`. The
    // hand-written array it replaces was checked by nothing, which three register items
    // said and none closed (R299/R301/R310).
    /// The `outcome` key of a [`SWAP_PANE_ACTION`] answer: what became of the two panes.
    ///
    /// **Four words, total over the request grammar, each with exactly one cause** — the property
    /// [`SelectHow`] states for the verb beside this one, and the reason an operator-facing or
    /// agent-facing sentence can be exact rather than a list of possibilities. A `with` request can only
    /// [`Swapped`](Self::Swapped) or find itself [`SamePane`](Self::SamePane); a `dir` request can only
    /// `Swapped`, stop [`AtEdge`](Self::AtEdge), or be measured from an [`Untiled`](Self::Untiled) pane.
    ///
    /// # Why the daemon says it instead of the caller deriving it
    ///
    /// [`SelectHow`]'s reason, one verb over. Three of the four ARE derivable by a caller that remembers
    /// which arm it asked and compares `a` with `b` ([`read`](Self::read) does exactly that for a daemon
    /// too old to answer this key). The fourth is not: telling "nothing that way" from "the pane you are
    /// placing is floating, so it has no that-way" takes the arrangement, and a client that read the
    /// arrangement to explain its own swap would join two instants to describe one.
    ///
    /// The rival is AHEAD of where sprag was here and this is the axis they lose on:
    /// `PaneSwapReason` (herdr `9a4ce5e1`, `src/api/schema/panes.rs:481`) has FOUR words too —
    /// `NoNeighbor` / `SamePane` / `NotFound` / `CrossTab` — where sprag answered `b: null` for two
    /// different facts. But `NoNeighbor` is still one word for an edge AND for a source missing from the
    /// rectangles they last composed (`directional_pane_target`), which is the same collapse their
    /// directional FOCUS has; `NotFound` is a refusal here rather than an outcome; and `CrossTab` is a
    /// capability sprag has and they refuse.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum SwapHow {
        /// The two panes TRADED PLACES: `a` sits where `b` was and `b` where `a` was.
        Swapped,
        /// A `with` request whose two panes are ONE pane. A legitimate no-op rather than a failure — a
        /// client re-asserting a placement it already has — and never reachable from a `dir` request,
        /// because a step never lands on the pane it started from.
        SamePane,
        /// A `dir` request whose origin the arrangement holds, with nothing that way: the window's edge.
        AtEdge,
        /// A `dir` request whose ORIGIN the arrangement holds NO LEAF for — it is floating, so it has no
        /// neighbours in any direction. Distinct from [`AtEdge`](Self::AtEdge) on purpose: the remedy is
        /// different (dock it, or name a partner), and an edge is a boundary the movement ran into where
        /// this is a request with no adjacency to walk at all.
        Untiled,
    }
}

impl SwapHow {
    /// This outcome's wire word — the value of the answer's `outcome` key.
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Swapped => "swapped",
            Self::SamePane => "same_pane",
            Self::AtEdge => "at_edge",
            Self::Untiled => "untiled",
        }
    }

    /// The outcome a wire word names, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|how| how.wire_str() == word)
    }

    /// Whether the arrangement MOVED — the answer's `changed` key, spelled once.
    ///
    /// The daemon announces on exactly this, because a swap that traded nothing gives a parked
    /// client nothing to re-read.
    #[must_use]
    pub fn changed(self) -> bool {
        matches!(self, Self::Swapped)
    }

    /// Read the outcome of an answer, from any daemon — including one that does not carry the key.
    ///
    /// `toward` is the direction the caller asked for, if it asked for one — which is what makes the
    /// derivation exact rather than a guess. `changed` plus the arm the caller chose determine three
    /// of the four words: a `with` request that changed nothing traded a pane with itself, and a
    /// `dir` request that changed nothing went nowhere. Only WHICH nothing is unrecoverable, and
    /// this answers the
    /// honest half of it ([`AtEdge`](Self::AtEdge), the case a user meets; a floating origin needs a
    /// client that floated it).
    ///
    /// One reader for every client, [`SelectHow::read`]'s rule, so a degraded sentence is decided
    /// here rather than re-derived at each surface.
    #[must_use]
    pub fn read(answer: &Value, toward: Option<PaneDir>) -> Self {
        if let Some(how) = answer
            .get(OUTCOME_KEY)
            .and_then(Value::as_str)
            .and_then(Self::from_wire)
        {
            return how;
        }
        if answer
            .get("changed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            return Self::Swapped;
        }
        match toward {
            Some(_) => Self::AtEdge,
            None => Self::SamePane,
        }
    }
}

/// The key both [`SelectHow`] and [`SwapHow`] answer under — one word for one question ("why is it
/// like that"), spelled once because two actions carry it.
pub const OUTCOME_KEY: &str = "outcome";

/// The mux control external invoke action that MOVES A BOUNDARY between panes
/// (`{pane?, dir, cells?}`) — tmux `resize-pane -L|-R|-U|-D`. Answers `{pane, cells, outcome}`.
///
/// # The op this daemon did not have
///
/// Until R307 a split's ratio could be moved by exactly ONE gesture in this whole product: a
/// pointer drag on a divider in `sprag-gui`, settled back through [`SET_LAYOUT_ACTION`] as a whole
/// tree. `sprag resize-pane`'s own docs said why — *"they move a DIVIDER, which is layout the
/// daemon does not model as an op"* — so `sprag-tui` had no way to change a pane's share at all,
/// and neither did a key, a shell or an agent.
///
/// It is the daemon's op rather than a client's for the reason this crate's own daemon-side reflow
/// states about a pane's size: the answer is `tile(tree, window)`, the TREE is this process's and
/// the WINDOW is this process's (arbitrated across every attached client), so a client converting
/// cells to a share would be re-deriving one of them from a rectangle of its own. It is also the
/// reason the amount can be CELLS at all — see below.
///
/// # The grammar
///
/// [`ResizeAsk`], spelled once and built by every client, [`SwapAsk`]'s rule.
///
/// * `pane` ABSENT means the scoped window's ACTIVE pane, the default [`SPLIT_ACTION`],
///   [`SWAP_PANE_ACTION`] and [`ZOOM_PANE_ACTION`] all take, and the only thing a keystroke can mean.
/// * `dir` — `"left"` / `"right"` / `"up"` / `"down"`. **It moves the BOUNDARY, not the pane**:
///   `right` takes the boundary right, and whether that grows or shrinks `pane` follows from which
///   side of it the pane sits on. That is tmux's behaviour and it needs no case analysis to state,
///   because the boundary is chosen by the AXIS alone
///   ([`LayoutTree::divider_on`](sprag_terminal::LayoutTree::divider_on)).
/// * `cells` — how far, defaulting to 1. A COUNT OF CELLS, not a share, and that is the argument
///   worth stating: a share cannot mean "five columns". The rival's `amount` is an `f32` defaulting
///   to `0.05` (herdr `9a4ce5e1`, `handle_pane_resize`), so one keypress moves a different number of
///   columns on a different window — and a different number again on a nested split, because a share
///   is measured against its own sub-region rather than against the screen.
///
/// # The answer: `{pane, cells, outcome}`
///
/// `pane` is the pane AS RESOLVED, so a caller that named none learns which one it moved. `cells`
/// is how many the boundary ACTUALLY travelled, which is below `cells` asked when it ran into the
/// last cell a side may keep — so a caller learns it was clamped without holding a second copy of
/// where the limit is. `outcome` is [`ResizeHow`], which says WHICH nothing happened when nothing
/// did.
///
/// REFUSED (`Rejected`), with nothing moved: `pane` naming no pane of the scoped session, or the
/// window having NO SIZE — no client has reported an area and nothing is pinned. The second is a
/// refusal rather than an outcome because a cell has no length in a window nobody has measured, and
/// it is the same fact `resize-window` already refuses on ([`NoBasis`](crate::window::NoBasis)).
pub const RESIZE_PANE_ACTION: &str = "resize_pane";

/// The REQUEST grammar of [`RESIZE_PANE_ACTION`] — what a caller may ask, as a type that cannot
/// spell what the action refuses.
///
/// [`SwapAsk`]'s shape one verb over and for its reason: the daemon [`parse`](Self::parse)s one of
/// these and every client [`to_args`](Self::to_args) builds one, so the three keys are spelled ONCE
/// for four surfaces (the daemon, the CLI verb, the MCP tool, the keybinding).
///
/// It is a STRUCT rather than an enum, unlike its two neighbours, because this action has one arm:
/// a boundary is always named by a direction. There is no `with`-shaped alternative — naming the
/// split itself would be naming a [`SplitId`](sprag_terminal::SplitId), which is the drag's handle
/// and not a thing a human or an agent has ever seen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ResizeAsk {
    /// The pane whose boundary moves. [`None`] ⇒ the scoped window's active pane, which is what a
    /// keypress means.
    pub pane: Option<PaneId>,
    /// Which way the BOUNDARY travels.
    pub dir: PaneDir,
    /// How many cells. Never zero — a request to move nothing is refused by
    /// [`parse`](Self::parse) rather than answered, because its honest answer would be
    /// indistinguishable from a boundary that could not move.
    pub cells: u16,
}

impl ResizeAsk {
    /// The request key naming the pane whose boundary moves.
    pub const PANE_KEY: &'static str = "pane";
    /// The request key naming which way the boundary travels.
    pub const DIR_KEY: &'static str = "dir";
    /// The request key naming how far, in cells.
    pub const CELLS_KEY: &'static str = "cells";

    /// How far a request that names no distance means — tmux's own `resize-pane` default, and the
    /// only amount a bare key can sensibly carry.
    pub const CELLS_DEFAULT: u16 = 1;

    /// The `args` object a client sends for this ask.
    ///
    /// An absent origin emits no key at all rather than a null, [`SwapAsk::to_args`]'s rule; the
    /// DEFAULT distance is emitted anyway, because unlike an origin it is a number the caller
    /// chose and a trace that dropped it would not show what was asked.
    #[must_use]
    pub fn to_args(self) -> Value {
        let mut map = Map::new();
        if let Some(pane) = self.pane {
            map.insert(Self::PANE_KEY.to_owned(), Value::from(pane.0));
        }
        map.insert(Self::DIR_KEY.to_owned(), Value::from(self.dir.wire_str()));
        map.insert(Self::CELLS_KEY.to_owned(), Value::from(self.cells));
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit.
    ///
    /// One [`None`] for every refusal, [`SwapAsk::parse`]'s rule and for its reason. An explicit
    /// `null` reads as ABSENT, so a client that fills its whole argument struct in asks what one
    /// that omits the optional halves asks — and `cells: 0` is REFUSED rather than defaulted,
    /// because a caller that spelled a zero meant something this action cannot do.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        let map = match args {
            Value::Object(map) => map,
            // No args at all names no direction, which this grammar does not admit.
            _ => return None,
        };
        let field = |key: &str| map.get(key).filter(|value| !value.is_null());
        let pane = match field(Self::PANE_KEY) {
            None => None,
            Some(value) => Some(PaneId(value.as_u64()?)),
        };
        let dir = PaneDir::from_wire(field(Self::DIR_KEY)?.as_str()?)?;
        let cells = match field(Self::CELLS_KEY) {
            None => Self::CELLS_DEFAULT,
            Some(value) => u16::try_from(value.as_u64()?).ok().filter(|n| *n > 0)?,
        };
        Some(Self { pane, dir, cells })
    }
}

sprag_terminal::closed_set! {
    // `ALL` is GENERATED with this enum from ONE variant list, so it cannot be missing a
    // variant and its length cannot disagree with its contents — see `closed_set!`. The
    // hand-written array it replaces was checked by nothing, which three register items
    // said and none closed (R299/R301/R310).
    /// The `outcome` key of a [`RESIZE_PANE_ACTION`] answer: what became of the boundary.
    ///
    /// **Five words, total over the request grammar, each with exactly one cause and one remedy** —
    /// [`SwapHow`]'s property one verb over, and the axis this verb beats the rival on outright: their
    /// `resize_pane` answers a `bool` (herdr `9a4ce5e1`, `src/layout.rs:241`), so an edge, a floating
    /// pane, a zoom and a boundary already as far as it goes are ONE value with four remedies.
    ///
    /// A move that was CLAMPED is [`Resized`](Self::Resized) with a smaller `cells` than was asked for,
    /// not a word of its own: it changed, and the number says by how much. That is what keeps
    /// [`changed`](Self::changed) derivable from the word alone — the property every parked client
    /// depends on, since it decides whether there is anything to re-read.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ResizeHow {
        /// The boundary MOVED. `cells` says how far, which is below what was asked when it ran into
        /// the last cell a side may keep.
        Resized,
        /// There is a boundary and it is already as far that way as it can go — one cell is the least
        /// a side may keep, which is
        /// [`Divider::stepped`](sprag_terminal::tiling::Divider::stepped)'s clamp, shared with the
        /// pointer drag's so the key and the mouse stop in the same place.
        AtMinimum,
        /// The arrangement holds the pane and has no division on that axis at all: the pane spans the
        /// window that way, so there is no boundary to move. Distinct from
        /// [`AtMinimum`](Self::AtMinimum) because the remedy is different — split first, rather than
        /// resize the other way.
        AtEdge,
        /// The arrangement holds no leaf for the pane: it is floating, so it has no boundaries in any
        /// direction. [`SwapHow::Untiled`]'s fact, one verb over.
        Untiled,
        /// The window is ZOOMED, so its arrangement is not what is on screen.
        ///
        /// **The boundary is deliberately NOT moved.** R285 made a zoom a PROJECTION precisely so the
        /// arrangement is untouched by it; moving a boundary the user cannot see, and answering
        /// success for it, is the one outcome worse than doing nothing.
        Zoomed,
    }
}

impl ResizeHow {
    /// This outcome's wire word — the value of the answer's [`OUTCOME_KEY`].
    #[must_use]
    pub fn wire_str(self) -> &'static str {
        match self {
            Self::Resized => "resized",
            Self::AtMinimum => "at_minimum",
            Self::AtEdge => "at_edge",
            Self::Untiled => "untiled",
            Self::Zoomed => "zoomed",
        }
    }

    /// The outcome a wire word names, or [`None`] for a word this build does not know.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|how| how.wire_str() == word)
    }

    /// Whether the ARRANGEMENT moved — [`SwapHow::changed`]'s rule, and what the daemon announces
    /// on: a resize that moved no boundary gives a parked client nothing to re-read.
    #[must_use]
    pub fn changed(self) -> bool {
        matches!(self, Self::Resized)
    }

    /// The sentence a surface says when nothing moved — [`None`] when something did.
    ///
    /// One wording for every surface (the CLI verb, the MCP tool, whatever a frontend shows),
    /// [`PaneDir::beyond`](sprag_terminal::PaneDir::beyond)'s rule: four adjectives copied per
    /// surface is the shape this project keeps finding drifted. It takes the direction because
    /// three of the five sentences are only exact with it in them.
    #[must_use]
    pub fn why(self, dir: PaneDir) -> Option<String> {
        match self {
            Self::Resized => None,
            Self::AtMinimum => Some(format!(
                "the boundary is already as far {} as it goes",
                dir.wire_str()
            )),
            Self::AtEdge => Some(format!(
                "the pane spans the window that way, so there is no boundary to move {}",
                dir.wire_str()
            )),
            Self::Untiled => {
                Some("the pane is floating, so it has no boundaries to move".to_owned())
            }
            Self::Zoomed => Some(
                "the window is zoomed, so its arrangement is not on screen; unzoom to resize"
                    .to_owned(),
            ),
        }
    }
}

/// The mux control external invoke action that fills a window with ONE pane, or ends that
/// (`{pane?, on?}`) — tmux `resize-pane -Z`. Answers `{pane, zoomed, changed}`.
///
/// `pane` ABSENT means the current window's ACTIVE pane, the default [`SPLIT_ACTION`] and
/// [`SWAP_PANE_ACTION`] take. `on` absent TOGGLES that pane's own zoom, so one binding is a switch
/// whichever pane it is aimed at; `true` / `false` are the explicit forms.
///
/// **The window is DERIVED from the pane**, [`MOVE_PANE_ACTION`]'s rule at both ends: a zoom is a
/// per-window fact and a [`PaneId`] is registry-unique, so zooming a pane
/// of a window nobody is looking at is a well-formed request. herdr's `pane.zoom` cannot express
/// it — its flag is per-tab and its target is resolved inside the active tab.
///
/// # What it changes, and what it deliberately does not
///
/// A zoom is a filter on the PROJECTION, never an edit to the arrangement
/// ([`sprag_terminal::Projection`]). The tree is untouched, [`SET_LAYOUT_ACTION`] still serves and
/// accepts it, [`MOVE_PANE_ACTION`] and [`SWAP_PANE_ACTION`] still act on it — herdr refuses a move
/// into or out of a zoomed tab outright (`PaneMoveReason::ZoomedTab`) — and a caller that draws
/// nothing can still read where every pane is. What moves is which pane the window projects to,
/// which is why the daemon reflows that pane's PTY to the whole window and every attached client
/// shows the same thing.
///
/// **Zooming SELECTS.** The daemon holds one invariant here — *a zoom names the pane its window is
/// ON, or there is no zoom* — so the pane a user types into is always one they can see. The other
/// side of it is that moving to a different pane ENDS the zoom (herdr instead RETARGETS it, which
/// is a coherent different feature: their zoom is a mode over a tab, sprag's is a fact about a
/// pane). Nothing else ends it, and nothing else has to: a split ends it because a split selects
/// its new pane, closing the zoomed pane ends it because the active pane hands off, and floating it
/// ends it because it stops being tiled.
///
/// **A single-pane window is not refused.** herdr answers `PaneZoomNoopReason::SinglePane`
/// (`src/app/actions.rs:1925` at `9a4ce5e1`); this accepts it. A zoom is a stored state, not a
/// repaint, and whether it changes what is on SCREEN depends on a pane count that can move a
/// moment later — so failing on it would make one caller's result depend on another caller's
/// timing, for no gain.
///
/// REFUSED (`Rejected`), with nothing moved: `pane` naming no pane of the scoped session, or
/// naming one its window does not TILE because a client floated it out. That is
/// [`SPLIT_ACTION`]'s rule and [`MOVE_PANE_ACTION`]'s, and a zoom belongs with them because all
/// three act on the TILING — a floated pane has no leaf to divide, to place beside, or to fill a
/// window with. It is deliberately NOT [`SWAP_PANE_ACTION`]'s edge rule: an edge is a boundary a
/// MOVEMENT ran into, while a floated target cannot be zoomed at all in the state it is in.
///
/// So `{zoomed, changed}` is total over four DISTINCT cases — now filling / already filling /
/// arrangement back / arrangement already showing — and an operator-facing sentence can name each
/// one exactly instead of listing the causes an answer is consistent with.
pub const ZOOM_PANE_ACTION: &str = "zoom_pane";

/// The mux control external invoke action that delivers a DROPPED FILE to a pane (`{pane, path}`) —
/// the wire form of a display client's drag-and-drop. Answers `{path}`: the path the pane is handed.
///
/// `path` is a LOCAL absolute path (the drop happens on the machine the display client and the host
/// share). For an ordinary pane the answer is that same path, shell-quoted, pasted straight in; for a
/// remote workspace pane (one carrying a structured `remote` — see [`crate::ssh`]) the file is
/// UPLOADED with `scp` and the answer is its REMOTE path (`~/<name>`), pasted when the transfer
/// completes. An upload is therefore ASYNC: a successful answer means the delivery started with a
/// valid file and a known destination, not that the bytes have landed.
///
/// Refused (`Rejected`) if no such pane exists, or if `path` names nothing that can be resolved on
/// this machine.
pub const DROP_FILE_ACTION: &str = "drop_file";

/// The container tag of the pane with host id `pane_id` — the `pane_<id>` node the
/// per-pane data grid + input external live under (the head of a pane-addressed
/// wire path). The ONE place this tag is formatted, shared by the scene assembly
/// ([`workspace_scene`](crate::workspace_scene)'s pane container) and
/// [`pane_input_path`].
#[must_use]
pub fn pane_container_tag(pane_id: u64) -> String {
    format!("pane_{pane_id}")
}

/// The `scene/invoke` / `scene/query` path addressing pane `pane_id`'s input
/// external `action` — `/pane_<id>/sprag_input/external/<action>`. Built from the
/// shared tags so the client and the host agree on the grammar by construction.
#[must_use]
pub fn pane_input_path(pane_id: u64, action: &str) -> String {
    format!(
        "/{}/{INPUT_TAG}/external/{action}",
        pane_container_tag(pane_id)
    )
}

/// The `scene/invoke` / `scene/query` path addressing the mux control external's
/// `action` — `/sprag_mux/external/<action>`.
#[must_use]
pub fn mux_action_path(action: &str) -> String {
    format!("/{MUX_TAG}/external/{action}")
}

/// The [`CELLS_FIELD`] query slot addressing the frame at scrollback `offset` —
/// `cells.<offset>` with the argument filled in (`cells_slot_at(0)` is the live view).
///
/// Derived from the declaration's own [`literal_prefix`](SchemaField::literal_prefix)
/// rather than re-spelling `"cells."`, so the address a client sends is built from the
/// same string the schema publishes and the host strips. Compose it with
/// [`pane_input_path`] to address a specific pane.
#[must_use]
pub fn cells_slot_at(offset: usize) -> String {
    format!("{}{offset}", CELLS_FIELD.literal_prefix())
}

/// The [`FIND_FIELD`] query slot searching for `needle` — `find.<needle>` with the argument filled
/// in. Compose it with [`pane_input_path`] to address a specific pane.
///
/// Built from the declaration's own [`literal_prefix`](SchemaField::literal_prefix) like
/// [`cells_slot_at`], so the address a client sends, the prefix the host strips, and the template
/// the schema publishes are one string. The needle is appended VERBATIM — see [`FIND_FIELD`] for why
/// no escaping is needed, and why that is what makes the address canonical.
#[must_use]
pub fn find_slot_for(needle: &str) -> String {
    format!("{}{needle}", FIND_FIELD.literal_prefix())
}

/// The [`REGEX_FIELD`] query slot searching for `pattern` — `regex.<pattern>` with the argument
/// filled in. The regex peer of [`find_slot_for`], built the same way and for the same reasons; the
/// pattern rides the path VERBATIM, so a pattern full of `.`, `/`, `\` and `|` needs no escaping and
/// has exactly one spelling.
#[must_use]
pub fn regex_slot_for(pattern: &str) -> String {
    format!("{}{pattern}", REGEX_FIELD.literal_prefix())
}
#[cfg(test)]
mod tests {
    /// **THE ROW SENTENCE SURVIVES AN ADDRESS NO ROW COULD HOLD** — the fallback, driven.
    ///
    /// [`skew_announcement`] builds a [`crate::report::MessageText`], which refuses anything past
    /// 200 bytes, and the address is the only part of the sentence that can be long. R318 recorded
    /// what an `expect` resting on *"that cannot happen"* cost, so there is a fallback — and a
    /// fallback nothing drives is a branch that is wrong the first time it runs.
    ///
    /// The CONTROL is the ordinary address: it keeps the path, which is what says WHICH act the
    /// daemon lacks.
    #[test]
    fn the_row_sentence_survives_an_address_no_row_could_hold() {
        let ordinary = skew_announcement(&mux_action_path(NEW_WINDOW_ACTION))
            .expect("an ordinary address fits a row");
        assert!(
            ordinary.text.as_str().contains("new_window")
                && ordinary.text.as_str().contains("does not perform"),
            "the ordinary sentence names the act: {}",
            ordinary.text.as_str(),
        );

        let absurd = format!("/{}", "x".repeat(300));
        let said = skew_announcement(&absurd).expect("the fallback fits a row");
        let text = said.text.as_str();
        assert!(
            !text.contains(&absurd) && text.contains("cannot act") && text.contains("kill-server"),
            "an address no row can hold is dropped, and the fact and the remedy stay: {text}",
        );
        assert!(
            text.len() <= crate::report::MessageText::MAX_BYTES,
            "the fallback is itself within the cap it exists for: {} bytes",
            text.len(),
        );
    }

    use pinion_core::external::ArgDomain;
    use serde_json::json;

    use super::*;

    #[test]
    fn pane_input_path_matches_the_documented_grammar() {
        assert_eq!(
            pane_input_path(0, KEY_ACTION),
            "/pane_0/sprag_input/external/key"
        );
        assert_eq!(
            pane_input_path(3, &cells_slot_at(12)),
            "/pane_3/sprag_input/external/cells.12"
        );
    }

    /// The declaration says the exact words the wire uses — a TRIPWIRE on the one spelling
    /// everything else derives from.
    ///
    /// **This test's first draft claimed far more and proved less, and R155's review proved
    /// the gap by experiment.** It asserted "the family answers exactly the paths it
    /// advertises — checked with pinion's OWN matcher, the same predicate its dispatch
    /// uses". Three lies in one sentence: `SchemaField::addresses` runs in NO dispatch path
    /// (pinion's `scene/query` calls `intro.query(path).ok_or(UnknownIntrospectPath)`; the
    /// matcher is reachable only through `read_only_or_unknown`, on `intervene`); the test
    /// calls no `query`, so it cannot observe what the family ANSWERS; and the `addresses`
    /// assertions were TAUTOLOGIES — a reviewer re-ran all four against a field renamed to
    /// `frames.<offset>` and every one still passed, because `cells_slot_at` builds its
    /// probe FROM `literal_prefix()`, so both sides move together. It was the R154 scar
    /// ("the test builds its tag from the very const under test") repeated one round later,
    /// wearing a doc that congratulated itself for avoiding it.
    ///
    /// So the tautologies are gone and what remains is the hardcoded spelling the old doc
    /// apologized for ("rather than sprag's spelling of it"). That line is the whole value:
    /// it is the only assertion a rename cannot satisfy, and a rename is the only drift
    /// worth catching here. `the_cells_family_answers_the_paths_it_declares` in `rpc.rs`
    /// owns the other half — what the surface actually answers — because that needs a live
    /// pane, which is exactly why this test could never have proved it.
    #[test]
    fn the_cells_family_declares_the_wire_words_it_uses() {
        // The template IS the definition; pin it verbatim.
        assert_eq!(CELLS_FIELD.path, "cells.<offset>");
        // What a `query` impl strips, and what `cells_slot_at` builds from.
        assert_eq!(CELLS_FIELD.literal_prefix(), "cells.");
        assert_eq!(cells_slot_at(7), "cells.7");
        // The declared argument, and the count path that bounds it — `frames`, which this
        // surface must actually serve for the domain to be true (`rpc.rs` proves it does).
        assert_eq!(CELLS_FIELD.args.len(), 1);
        assert_eq!(CELLS_FIELD.args[0].name, "offset");
        assert!(matches!(
            CELLS_FIELD.args[0].domain,
            ArgDomain::IndexOf(FRAMES_SLOT)
        ));
    }

    /// The same spelling tripwire for the search family — and one thing `cells` cannot have: the
    /// needle is appended UNESCAPED, so the address of a needle is the needle. A future "let us just
    /// percent-encode it" would break this and should have to argue with it, since encoding is
    /// exactly what re-introduces two spellings of one search (`cells.007`'s lesson).
    #[test]
    fn the_find_family_declares_the_wire_words_it_uses() {
        assert_eq!(FIND_FIELD.path, "find.<needle>");
        assert_eq!(FIND_FIELD.literal_prefix(), "find.");
        assert_eq!(find_slot_for("a.b c"), "find.a.b c");
        // `Open`, and honestly so: a needle is caller-invented, so there is no domain to publish.
        assert_eq!(FIND_FIELD.args.len(), 1);
        assert_eq!(FIND_FIELD.args[0].name, "needle");
        assert!(matches!(FIND_FIELD.args[0].domain, ArgDomain::Open));
    }

    /// The regex family is a SEPARATE address, and the tripwire says so: `find.a.b` and
    /// `regex.a.b` are two different searches of the same three characters. Collapsing them onto
    /// one address with a mode argument would make what a search MEANS depend on something the
    /// address does not carry, which is the aliasing the whole family grammar exists to prevent.
    #[test]
    fn the_regex_family_is_a_distinct_address_from_the_literal_one() {
        assert_eq!(REGEX_FIELD.path, "regex.<pattern>");
        assert_eq!(REGEX_FIELD.literal_prefix(), "regex.");
        assert_eq!(regex_slot_for("a.b|c"), "regex.a.b|c");
        assert_ne!(
            regex_slot_for("a.b"),
            find_slot_for("a.b"),
            "the same string in two languages must not share one address",
        );
        // `Open`, honestly: a pattern is caller-invented, so there is no domain to publish.
        assert_eq!(REGEX_FIELD.args.len(), 1);
        assert_eq!(REGEX_FIELD.args[0].name, "pattern");
        assert!(matches!(REGEX_FIELD.args[0].domain, ArgDomain::Open));
        // And both are published, so an agent discovers that the choice exists.
        assert!(PANE_SCHEMA.contains(&FIND_FIELD) && PANE_SCHEMA.contains(&REGEX_FIELD));
    }

    /// The REQUEST grammar round trips through the bytes, both ways, for every shape a caller can
    /// spell — which is what makes one type serve the daemon that parses and the three clients that
    /// build.
    ///
    /// The `from`-less step must emit the bytes it emitted before origins existed, and that is
    /// asserted as a LITERAL rather than as `to_args` compared with itself: the commonest request on
    /// this action is the one a trace reader and an old-daemon probe both recognise by eye.
    #[test]
    fn the_select_grammar_round_trips_through_the_bytes_it_sends() {
        let shapes = [
            SelectAsk::Pane(PaneId(7)),
            SelectAsk::Toward {
                dir: PaneDir::Left,
                from: None,
            },
            SelectAsk::Toward {
                dir: PaneDir::Down,
                from: Some(PaneId(0)),
            },
        ];
        for ask in shapes {
            assert_eq!(SelectAsk::parse(&ask.to_args()), Some(ask), "{ask:?}");
        }
        assert_eq!(shapes[0].to_args(), json!({"pane": 7}));
        assert_eq!(
            shapes[1].to_args(),
            json!({"dir": "left"}),
            "a step with no origin says nothing about one — the key is ABSENT, not null",
        );
        assert_eq!(shapes[2].to_args(), json!({"dir": "down", "from": 0}));
        assert_eq!(shapes[2].toward(), Some(PaneDir::Down));
        assert_eq!(shapes[2].origin(), Some(PaneId(0)));
        assert_eq!(shapes[0].toward(), None, "a named pane stepped nowhere");
        assert_eq!(shapes[1].origin(), None);
    }

    /// Every reading the grammar does NOT admit, in one place — because each of them is a caller
    /// bug the daemon answers with one `TypeMismatch`, so this is the only surface that says what
    /// the set is.
    ///
    /// An explicit `null` reads as ABSENT, deliberately: a client that fills in a whole argument
    /// struct and leaves the optional halves null must ask the same thing as one that omits them,
    /// or the same request would mean two things depending on how it was built.
    #[test]
    fn the_select_grammar_admits_nothing_else() {
        for refused in [
            json!({}),
            json!(null),
            json!([]),
            json!("left"),
            json!({"pane": 1, "dir": "left"}),
            json!({"pane": 1, "from": 2}),
            json!({"from": 2}),
            json!({"dir": "sideways"}),
            json!({"dir": 3}),
            json!({"pane": "1"}),
            json!({"dir": "left", "from": "2"}),
            json!({"dir": "left", "from": -1}),
        ] {
            assert_eq!(SelectAsk::parse(&refused), None, "admitted {refused}");
        }
        assert_eq!(
            SelectAsk::parse(&json!({"pane": 4, "dir": null, "from": null})),
            Some(SelectAsk::Pane(PaneId(4))),
            "an explicit null is an absent key",
        );
        assert_eq!(
            SelectAsk::parse(&json!({"dir": "up", "extra": 1})),
            Some(SelectAsk::Toward {
                dir: PaneDir::Up,
                from: None
            }),
            "a key this grammar does not know is not its business to police — the request \
             declares its WIRE_PROTOCOL, which is the check that catches a shape it cannot read",
        );
    }

    /// The `outcome` vocabulary round trips, and `changed` has ONE derivation — so the key the
    /// daemon writes beside it can never disagree with the word.
    #[test]
    fn the_outcome_words_round_trip_and_only_a_move_counts_as_changed() {
        for how in SelectHow::ALL {
            assert_eq!(SelectHow::from_wire(how.wire_str()), Some(how));
        }
        assert_eq!(SelectHow::from_wire("Moved"), None);
        assert_eq!(SelectHow::from_wire("edge"), None);
        assert_eq!(SelectHow::from_wire(""), None);
        assert_eq!(
            SelectHow::ALL.map(SelectHow::changed),
            [true, false, false, false],
            "exactly one of the four moved the active pane, which is what wakes parked clients",
        );
    }

    /// The reader takes the daemon's word when there is one — and stays exact against a daemon built
    /// before the word existed, which is the direction an ADDITIVE answer key does not cover by
    /// itself (an old CLIENT simply ignores a new key; a new client meets a missing one).
    ///
    /// Three of the four are recoverable from `changed` plus the arm the caller chose. The fourth is
    /// not, and the fallback says the honest thing rather than the specific one.
    #[test]
    fn an_outcome_is_read_from_the_word_and_falls_back_to_the_arm_that_was_asked() {
        let word = |how: SelectHow| json!({"pane": 3, "changed": how.changed(), "outcome": how.wire_str()});
        for how in SelectHow::ALL {
            assert_eq!(SelectHow::read(&word(how), None), how);
            assert_eq!(SelectHow::read(&word(how), Some(PaneDir::Left)), how);
        }

        // A pre-R299 daemon: `{pane, changed}` and nothing more.
        let old = |changed: bool| json!({"pane": 3, "changed": changed});
        assert_eq!(SelectHow::read(&old(true), None), SelectHow::Moved);
        assert_eq!(
            SelectHow::read(&old(true), Some(PaneDir::Up)),
            SelectHow::Moved,
        );
        assert_eq!(
            SelectHow::read(&old(false), None),
            SelectHow::AlreadyActive,
            "a PANE request that moved nothing was a re-select — exact without the word",
        );
        assert_eq!(
            SelectHow::read(&old(false), Some(PaneDir::Left)),
            SelectHow::AtEdge,
            "a DIRECTION that moved nothing went nowhere; which nothing is what the word adds, and \
             the edge is the case a user meets",
        );
        // A word this build does not know, and a malformed answer, both degrade rather than fail:
        // rendering is the last thing a caller does, and a select that already happened must still
        // be reported.
        assert_eq!(
            SelectHow::read(
                &json!({"pane": 3, "changed": true, "outcome": "teleported"}),
                None
            ),
            SelectHow::Moved,
        );
        assert_eq!(SelectHow::read(&json!({}), None), SelectHow::AlreadyActive);
    }

    /// The SWAP's grammar round trips through the bytes, every shape, both ways — the select's rule
    /// one verb over, and the reason one type can serve the daemon that parses and the three clients
    /// that build.
    #[test]
    fn the_swap_grammar_round_trips_through_the_bytes_it_sends() {
        let shapes = [
            SwapAsk::With {
                pane: None,
                with: PaneId(5),
            },
            SwapAsk::With {
                pane: Some(PaneId(3)),
                with: PaneId(5),
            },
            SwapAsk::Toward {
                pane: None,
                dir: PaneDir::Left,
            },
            SwapAsk::Toward {
                pane: Some(PaneId(3)),
                dir: PaneDir::Up,
            },
        ];
        for ask in shapes {
            assert_eq!(SwapAsk::parse(&ask.to_args()), Some(ask), "{ask:?}");
        }
        assert_eq!(
            shapes[0].to_args(),
            json!({"with": 5}),
            "a swap with no origin says nothing about one — the key is ABSENT, not null",
        );
        assert_eq!(shapes[1].to_args(), json!({"pane": 3, "with": 5}));
        assert_eq!(shapes[2].to_args(), json!({"dir": "left"}));
        assert_eq!(shapes[3].to_args(), json!({"pane": 3, "dir": "up"}));
        assert_eq!(shapes[3].toward(), Some(PaneDir::Up));
        assert_eq!(shapes[3].origin(), Some(PaneId(3)));
        assert_eq!(shapes[1].toward(), None, "a named partner looked nowhere");
        assert_eq!(shapes[2].origin(), None);
    }

    /// The JOIN's grammar round trips through the bytes, both addresses — the swap's rule one verb
    /// over, and the reason one type serves the daemon that parses and the clients that build.
    ///
    /// The BYTES are what matters here more than the round trip: `window` and `window_id` are two
    /// keys for one destination, and a client that spelled either its own way would be committing a
    /// different address from the one it holds.
    ///
    /// REVERT-PROOF: make `Picked::to_args` emit `WINDOW_KEY` and the byte assertions fail with the
    /// two spellings side by side; make `parse` take the first key it finds instead of refusing
    /// both-at-once and the pair below goes green with a request that names two destinations.
    #[test]
    fn the_join_grammar_round_trips_through_the_bytes_it_sends() {
        let shapes = [
            JoinAsk {
                pane: PaneId(3),
                window: WindowRef::Named("build".to_owned()),
            },
            JoinAsk {
                pane: PaneId(3),
                window: WindowRef::Picked(WindowId(7)),
            },
        ];
        for ask in &shapes {
            assert_eq!(JoinAsk::parse(&ask.to_args()), Some(ask.clone()), "{ask:?}");
        }
        assert_eq!(shapes[0].to_args(), json!({"pane": 3, "window": "build"}));
        assert_eq!(shapes[1].to_args(), json!({"pane": 3, "window_id": 7}));

        // THE SHARED GRAMMAR, on its own (R330). A join is one of several verbs that address a
        // window, and the keys are pinned HERE because a second verb spelling `window_id` its own
        // way is the drift `WindowRef` was hoisted to prevent.
        let mut map = Map::new();
        WindowRef::Named("build".to_owned()).write(&mut map);
        assert_eq!(Value::Object(map.clone()), json!({"window": "build"}));
        map.clear();
        WindowRef::Picked(WindowId(7)).write(&mut map);
        assert_eq!(Value::Object(map), json!({"window_id": 7}));

        let read = |value: Value| match value {
            Value::Object(map) => WindowRef::read(&map),
            _ => panic!("the fixture builds objects"),
        };
        assert_eq!(
            read(json!({})),
            Ok(None),
            "neither key is the SCOPED window"
        );
        assert_eq!(
            read(json!({"window": null, "window_id": null})),
            Ok(None),
            "an explicit null is an absent key",
        );
        assert_eq!(
            read(json!({"window": "build", "window_id": 7})),
            Err(MalformedWindowRef),
            "a name AND an identity is two destinations, which is no request at all",
        );
        assert_eq!(read(json!({"window": 7})), Err(MalformedWindowRef));
        assert_eq!(read(json!({"window_id": "7"})), Err(MalformedWindowRef));
        assert_eq!(read(json!({"window_id": -1})), Err(MalformedWindowRef));

        // A NAME and an IDENTITY are two addresses, so a request carrying both names no destination
        // this daemon can honour — and one carrying neither names none at all.
        for refused in [
            json!({}),
            json!(null),
            json!([]),
            json!({"pane": 3}),
            json!({"window": "build"}),
            json!({"window_id": 7}),
            json!({"pane": 3, "window": "build", "window_id": 7}),
            json!({"pane": "3", "window_id": 7}),
            json!({"pane": 3, "window_id": "7"}),
            json!({"pane": 3, "window": 7}),
            json!({"pane": 3, "window_id": -1}),
        ] {
            assert_eq!(JoinAsk::parse(&refused), None, "admitted {refused}");
        }
        assert_eq!(
            JoinAsk::parse(&json!({"pane": 3, "window": "build", "window_id": null})),
            Some(shapes[0].clone()),
            "an explicit null is an absent key",
        );
    }

    /// Every reading the SWAP grammar does not admit — one `TypeMismatch` at the daemon, so this is
    /// the only surface that says what the set is.
    ///
    /// The set differs from the select's in exactly one place, and deliberately: `{pane, with}` is
    /// LEGAL here where `{pane, from}` is not there. An origin with no direction has no reading when
    /// the other key is a target; here the origin is the pane being placed and the partner is named
    /// outright, so both are needed at once.
    #[test]
    fn the_swap_grammar_admits_nothing_else() {
        for refused in [
            json!({}),
            json!(null),
            json!([]),
            json!("left"),
            json!({"pane": 1}),
            json!({"with": 2, "dir": "left"}),
            json!({"pane": 1, "with": 2, "dir": "left"}),
            json!({"dir": "sideways"}),
            json!({"dir": 3}),
            json!({"pane": "1", "with": 2}),
            json!({"with": "2"}),
            json!({"with": -1}),
        ] {
            assert_eq!(SwapAsk::parse(&refused), None, "admitted {refused}");
        }
        assert_eq!(
            SwapAsk::parse(&json!({"pane": null, "with": 4, "dir": null})),
            Some(SwapAsk::With {
                pane: None,
                with: PaneId(4)
            }),
            "an explicit null is an absent key",
        );
        assert_eq!(
            SwapAsk::parse(&json!({"dir": "up", "extra": 1})),
            Some(SwapAsk::Toward {
                pane: None,
                dir: PaneDir::Up
            }),
            "a key this grammar does not know is not its business to police — the request declares \
             its WIRE_PROTOCOL, which is the check that catches a shape it cannot read",
        );
    }

    /// The swap's `outcome` vocabulary round trips, and `changed` has ONE derivation.
    #[test]
    fn the_swap_outcome_words_round_trip_and_only_a_trade_counts_as_changed() {
        for how in SwapHow::ALL {
            assert_eq!(SwapHow::from_wire(how.wire_str()), Some(how));
        }
        assert_eq!(SwapHow::from_wire("Swapped"), None);
        assert_eq!(SwapHow::from_wire("edge"), None);
        assert_eq!(SwapHow::from_wire(""), None);
        assert_eq!(
            SwapHow::ALL.map(SwapHow::changed),
            [true, false, false, false],
            "exactly one of the four moved the arrangement, which is what wakes parked clients",
        );
        // The two verbs answer under ONE key, so a client reading `outcome` needs no per-action
        // spelling — and the two vocabularies are DISJOINT except where they mean the same thing.
        assert_eq!(SelectHow::AtEdge.wire_str(), SwapHow::AtEdge.wire_str());
        assert_eq!(SelectHow::Untiled.wire_str(), SwapHow::Untiled.wire_str());
        assert!(
            SelectHow::from_wire(SwapHow::SamePane.wire_str()).is_none(),
            "a select cannot answer the swap's own word",
        );
    }

    /// The swap's reader takes the daemon's word when there is one, and stays exact against a daemon
    /// built before the word existed — the direction an additive answer key does not cover by itself.
    #[test]
    fn a_swap_outcome_is_read_from_the_word_and_falls_back_to_the_arm_that_was_asked() {
        let word = |how: SwapHow| json!({"a": 3, "b": 4, "changed": how.changed(), "outcome": how.wire_str()});
        for how in SwapHow::ALL {
            assert_eq!(SwapHow::read(&word(how), None), how);
            assert_eq!(SwapHow::read(&word(how), Some(PaneDir::Left)), how);
        }

        // A pre-R301 daemon: `{a, b, changed}` and nothing more.
        let old = |b: Value, changed: bool| json!({"a": 3, "b": b, "changed": changed});
        assert_eq!(
            SwapHow::read(&old(json!(4), true), Some(PaneDir::Left)),
            SwapHow::Swapped,
        );
        assert_eq!(SwapHow::read(&old(json!(4), true), None), SwapHow::Swapped);
        assert_eq!(
            SwapHow::read(&old(json!(3), false), None),
            SwapHow::SamePane,
            "a PARTNER request that traded nothing traded a pane with itself — exact without the word",
        );
        assert_eq!(
            SwapHow::read(&old(Value::Null, false), Some(PaneDir::Left)),
            SwapHow::AtEdge,
            "a DIRECTION that traded nothing found nothing; WHICH nothing is what the word adds, \
             and the edge is the case a user meets",
        );
        // A word this build does not know, and a malformed answer, both degrade rather than fail:
        // a swap that already happened must still be reported.
        assert_eq!(
            SwapHow::read(
                &json!({"a": 3, "changed": true, "outcome": "teleported"}),
                None
            ),
            SwapHow::Swapped,
        );
        assert_eq!(SwapHow::read(&json!({}), None), SwapHow::SamePane);
    }

    #[test]
    fn mux_action_path_matches_the_documented_grammar() {
        assert_eq!(mux_action_path(SPAWN_ACTION), "/sprag_mux/external/spawn");
        assert_eq!(mux_action_path(PANES_SLOT), "/sprag_mux/external/panes");
    }

    #[test]
    fn pane_container_tag_is_the_scene_assembly_tag() {
        // The head of a pane-addressed path must equal the container tag the scene
        // assembly stamps, or an invoke would address a non-existent node.
        assert_eq!(pane_container_tag(7), "pane_7");
        assert!(pane_input_path(7, KEY_ACTION).starts_with("/pane_7/"));
    }

    /// [`AttachAsk`]'s round trip: one grammar, both directions, for EVERY target an attach can
    /// name.
    ///
    /// The scope is written into the SAME params object, because that is how these travel on the
    /// wire — a display client's attach carries `{"attached": true}` from its connection and
    /// `{"last": true}` (or `{"step": …}`) from the gesture, and the two must not read each other.
    /// A parse that took the scope keys for its own would fail on the last line of each pass.
    #[test]
    fn every_attach_target_round_trips_beside_a_scope() {
        for ask in [
            AttachAsk::Scoped,
            AttachAsk::LastViewed { unattached: false },
            AttachAsk::LastViewed { unattached: true },
            AttachAsk::Step(OrderStep::Next),
            AttachAsk::Step(OrderStep::Previous),
        ] {
            let mut params = Map::new();
            sprag_rpc::ScopeAsk::Attached.write_into(&mut params);
            ask.write_into(&mut params);
            assert_eq!(
                AttachAsk::parse(Some(&Value::Object(params.clone()))),
                Ok(ask),
                "{ask:?}",
            );
            assert_eq!(
                sprag_rpc::ScopeAsk::parse(Some(&Value::Object(params))),
                Ok(sprag_rpc::ScopeAsk::Attached),
                "and the scope beside it is untouched by {ask:?}",
            );
        }
    }

    /// Every way an attach target can be malformed, each its own refusal — and the CONTROLS that
    /// keep the test from passing vacuously: a well-typed `false` on each boolean key is an absent
    /// key, not a fault.
    ///
    /// REVERT-PROOF for the step half: fold `StepUnknown` into `StepNotAString` and the
    /// `"sideways"` line fails; let a `step` beside a `last` resolve by precedence instead of
    /// refusing and the `TwoTargets` line fails; read `{"step": false}` as absent (the booleans'
    /// rule) and the `StepNotAString` line fails.
    #[test]
    fn each_malformed_attach_target_is_its_own_refusal() {
        let parse = |params: Value| AttachAsk::parse(Some(&params));
        assert_eq!(parse(json!({"last": 1})), Err(AttachFault::LastNotABool));
        assert_eq!(
            parse(json!({"last": null})),
            Err(AttachFault::LastNotABool),
            "null is refused here for the reason `AttachAsk::parse` states: the fallback would be \
             the session the client is already on",
        );
        assert_eq!(
            parse(json!({"last": true, "unattached": "yes"})),
            Err(AttachFault::UnattachedNotABool),
        );
        assert_eq!(
            parse(json!({"unattached": true})),
            Err(AttachFault::UnattachedWithoutLast),
            "a filter with no subject is refused, never quietly attached to the scope",
        );
        assert_eq!(
            parse(json!({"step": 1})),
            Err(AttachFault::StepNotAString),
            "a step is a WORD; there is no well-typed no to read as absent",
        );
        assert_eq!(
            parse(json!({"step": false})),
            Err(AttachFault::StepNotAString),
            "and a boolean is not one either, unlike the two keys above it",
        );
        assert_eq!(
            parse(json!({"step": "sideways"})),
            Err(AttachFault::StepUnknown),
            "a string that is not one of the two words is its OWN refusal, not a type error",
        );
        assert_eq!(
            parse(json!({"last": true, "step": "next"})),
            Err(AttachFault::TwoTargets),
            "two targets is no target: nothing here may choose between them",
        );
        assert_eq!(
            parse(json!({"last": false})),
            Ok(AttachAsk::Scoped),
            "the CONTROL: a well-typed no is an absent key",
        );
        assert_eq!(
            parse(json!({"last": true, "unattached": false})),
            Ok(AttachAsk::LastViewed { unattached: false }),
            "the second CONTROL: an explicit unnarrowed ask is the plain one",
        );
        assert_eq!(
            parse(json!({"last": false, "step": "previous"})),
            Ok(AttachAsk::Step(OrderStep::Previous)),
            "the third CONTROL: a step beside an explicit no-last is ONE target, not two",
        );
        assert_eq!(
            AttachAsk::parse(None),
            Ok(AttachAsk::Scoped),
            "an attach with no params at all goes where the connection is scoped",
        );
    }

    /// THE SHAPE PIN — what keeps [`WIRE_PROTOCOL`] from being a number nobody remembers to move.
    ///
    /// A hand-maintained protocol version fails on the day someone changes a shape and forgets to
    /// bump it, and the failure is silent until two builds meet. herdr's `PROTOCOL_VERSION`
    /// (`9a4ce5e1`) has exactly that hole: nothing in their tree fails when a shape moves under it.
    ///
    /// So the pin makes the number a CONSEQUENCE of the shape rather than a promise about it: this
    /// renders one canonical value of each type a client deserialises structurally and compares the
    /// bytes. Change any of those shapes and this fails, right here, with the instruction.
    ///
    /// The hand-parsed slots (`panes`, `revision`) are deliberately absent: a client reads those
    /// key by key with explicit fallbacks, so adding a key cannot break one. What is pinned is what
    /// serde decodes WHOLE, which is where a shape change turns into a type error at slot nine.
    ///
    /// # And the REQUEST shapes, which version 2 of this pin did not cover
    ///
    /// Every shape above is an ANSWER. R300 moved the number for a REQUEST — `select_pane` gained an
    /// origin argument — and reverting the bump left the whole suite green, because nothing here
    /// looked at what a client SENDS. That is the same hole this pin exists to close, on the other
    /// side of the wire, and it is the more dangerous side: an added answer key is absent-not-wrong
    /// to an old reader, where an added ARGUMENT is ACCEPTED AND DROPPED by an old daemon and the
    /// request still parses (R294 measured it).
    ///
    /// So a request grammar this project owns is pinned by its BYTES here too. Only the grammars
    /// that are a TYPE — a hand-built `json!` at a call site is a literal a reader can already see,
    /// where a type's rendering can move underneath every one of its four callers at once.
    ///
    /// # The case that motivates rendering bytes rather than reading diffs
    ///
    /// [`CellFrame`](crate::CellFrame) is the biggest thing a client decodes whole, and sprag does
    /// not own its shape: [`sprag_grid::wire`]'s interned style carries pinion's cell vocabulary
    /// verbatim, on purpose, so that an upstream ADDITION is a compile error here instead of
    /// silent data loss. A respelling is neither. pinion R1540 gave `UnderlineStyle` a
    /// `rename_all = "lowercase"` and every sprag frame's `"underline"` changed value — **with no
    /// sprag source line touched, in a commit whose entire sprag-side diff was a rev string**.
    /// Version 1 of this pin would not have noticed; two unrelated tests holding literal payloads
    /// did, by luck. So the frame is pinned here, where the failure names the number to move.
    #[test]
    fn the_wire_shape_is_what_this_protocol_number_stands_for() {
        use pinion_core::term_grid::{CellAttrs, GridBuffer, Hyperlink, TermCell, UnderlineStyle};
        use pinion_core::{Color, TermColor};
        use sprag_terminal::{
            LayoutSnapshot, LayoutWire, PaneId, SessionActivity, SessionInfo, WindowInfo,
        };

        assert_eq!(
            serde_json::to_string(&WindowInfo {
                name: "0".to_owned(),
                id: None,
                current: true,
                opened_by: None,
            })
            .expect("a window serialises"),
            r#"{"name":"0","current":true}"#,
            "{}",
            BUMP,
        );
        // The CLAIMED form too, and the IDENTIFIED one. An additive `Option` that is `None` in the
        // pinned value is INVISIBLE to this pin — so pinning only the unclaimed window would have
        // let the new key's spelling move without a word. Found by auditing R319's own addition,
        // and it is why R329's `id` is pinned in both states rather than only in the absent one
        // this daemon never serves.
        assert_eq!(
            serde_json::to_string(&WindowInfo {
                name: "agentwork".to_owned(),
                id: None,
                current: false,
                opened_by: Some(PaneId(7)),
            })
            .expect("a claimed window serialises"),
            r#"{"name":"agentwork","current":false,"opened_by":7}"#,
            "{}",
            BUMP,
        );
        // What the DAEMON actually serves: `Session::window_infos` fills the id on every row, so a
        // client of this build never meets the two values above. They are pinned because an OLDER
        // daemon serves them and this build's clients must keep reading them.
        assert_eq!(
            serde_json::to_string(&WindowInfo {
                name: "0".to_owned(),
                id: Some(sprag_terminal::WindowId(4)),
                current: true,
                opened_by: None,
            })
            .expect("an identified window serialises"),
            r#"{"name":"0","id":4,"current":true}"#,
            "{}",
            BUMP,
        );
        // And the REQUEST grammar this round added — the side R300 found this pin blind to. The
        // DEFAULT birth must render EMPTY: that is what makes the addition additive, and a version
        // of it that emitted `{"detached":false}` would change every existing caller's bytes.
        assert_eq!(
            serde_json::to_string(&WindowBirthAsk::default().to_args())
                .expect("a default birth serialises"),
            "{}",
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &WindowBirthAsk(sprag_terminal::WindowBirth {
                    detached: true,
                    opened_by: Some(PaneId(7)),
                })
                .to_args()
            )
            .expect("a detached birth serialises"),
            r#"{"detached":true,"opened_by":7}"#,
            "{}",
            BUMP,
        );

        assert_eq!(
            serde_json::to_string(&SessionInfo {
                name: "0".to_owned(),
                windows: 1,
                panes: 2,
                default: true,
                attached: 1,
            })
            .expect("a session serialises"),
            r#"{"name":"0","windows":1,"panes":2,"default":true,"attached":1}"#,
            "{}",
            BUMP,
        );

        // The SAMPLE, split out of the line above by R282. Two shapes, pinned for two different
        // reasons: a client decodes the row whole, and the reading's envelope is what carries the
        // sample's AGE — a client that read the rows without it would be reading a fact whose
        // freshness it had to assume.
        assert_eq!(
            serde_json::to_string(&SessionActivity {
                name: "0".to_owned(),
                cwd: Some("/tmp".to_owned()),
                branch: Some("main".to_owned()),
                ports: vec![8080],
            })
            .expect("an activity row serialises"),
            r#"{"name":"0","cwd":"/tmp","branch":"main","ports":[8080]}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(&ActivityWire {
                sampled_ms_ago: 12,
                sessions: vec![SessionActivity {
                    name: "0".to_owned(),
                    cwd: None,
                    branch: None,
                    ports: Vec::new(),
                }],
            })
            .expect("a reading serialises"),
            r#"{"sampled_ms_ago":12,"sessions":[{"name":"0"}]}"#,
            "{}",
            BUMP,
        );

        // The arrangement — the shape whose last change (R264's flattening) is the reason this
        // whole check exists. `root` is an arena INDEX here; it used to be a nested node.
        let mut tree = sprag_terminal::LayoutTree::new();
        tree.reconcile(&[PaneId(1)], &mut std::collections::HashMap::new());
        assert_eq!(
            serde_json::to_string(&LayoutSnapshot {
                revision: 3,
                tree: LayoutWire::from(&tree),
                floating: vec![PaneId(9)],
                zoomed: Some(PaneId(1)),
            })
            .expect("an arrangement serialises"),
            r#"{"revision":3,"tree":{"nodes":[{"leaf":1}],"root":0},"floating":[9],"zoomed":1}"#,
            "{}",
            BUMP,
        );

        // The cell frame — the shape sprag BORROWS. Every field of the interned style is spelled
        // by making the one cell carry a non-default value of it: a default-everywhere buffer
        // would pin only the unit variants and would miss a rename of `Rgb` or `Curly`. The
        // hyperlink table has the entry the cell's index names, so this canonical value is one
        // `decode` accepts rather than merely one `encode` can emit.
        // Built through the builders because `CellAttrs` is `#[non_exhaustive]` — which is the
        // right shape for a pin anyway: an upstream field ADDED here does not fail to compile, it
        // appears in the rendered bytes below, and that is the failure carrying the instruction.
        let attrs = CellAttrs::empty()
            .with_bold(true)
            .with_dim(true)
            .with_italic(true)
            .with_underline_style(UnderlineStyle::Curly)
            .with_blink(true)
            .with_reverse(true)
            .with_hidden(true)
            .with_strikethrough(true);
        let mut styled = TermCell::new(
            "A",
            TermColor::Rgb(Color::rgb(1, 2, 3)),
            TermColor::Indexed(4),
        )
        .with_attrs(attrs)
        .with_underline_color(TermColor::Indexed(5));
        styled.hyperlink = Some(pinion_core::term_grid::HyperlinkId(0));
        let cells = GridBuffer::new(1, 1)
            .with_row(0, [styled])
            .with_hyperlinks([Hyperlink::new("https://example.test")])
            .with_row_generation(0, 7);
        assert_eq!(
            serde_json::to_string(&crate::CellFrame {
                cells,
                facts: crate::PaneScrollFacts {
                    scrollback_len: 11,
                    visible_rows: 1,
                },
            })
            .expect("a cell frame serialises"),
            r#"{"cells":{"cols":1,"rows":1,"cursor":{"col":0,"row":0,"shape":"Block","visible":false,"cursor_color":null,"blink":false},"screen":"Main","generations":[7],"styles":[{"fg":{"Rgb":{"r":1,"g":2,"b":3,"a":255}},"bg":{"Indexed":4},"attrs":{"bold":true,"dim":true,"italic":true,"underline":"curly","blink":true,"reverse":true,"hidden":true,"strikethrough":true},"underline_color":{"Indexed":5},"hyperlink":0,"width":"Narrow"}],"lines":[{"text":[[1,"A"]],"style":[[1,0]]}],"hyperlinks":[{"uri":"https://example.test","id":null}]},"scrollback_len":11,"visible_rows":1}"#,
            "{}",
            BUMP,
        );

        // The SCOPE grammar, which is the request half of EVERY method rather than one action's
        // (`sprag_rpc::ScopeAsk`, R303). All three arms, and the empty one especially: `Default`
        // writing nothing is what keeps the commonest request on the wire byte-identical to what it
        // was before an attached scope existed, and a change that started emitting a key there
        // would make every unscoped request a new shape without a line of this file moving.
        //
        // It earns its place here for the reason the section exists at all, in its sharpest form:
        // an old daemon reading `{"attached":true}` finds no key it knows, which to it is an
        // ABSENT scope — the DEFAULT session. Not a dropped argument that narrows an answer, but
        // every read and every keystroke landing in a session nobody named.
        let mut scope = serde_json::Map::new();
        sprag_rpc::ScopeAsk::Default.write_into(&mut scope);
        assert!(scope.is_empty(), "{}", BUMP);
        sprag_rpc::ScopeAsk::Named("work".to_owned()).write_into(&mut scope);
        assert_eq!(
            serde_json::to_string(&scope).expect("a scope renders"),
            r#"{"session":"work"}"#,
            "{}",
            BUMP,
        );
        let mut scope = serde_json::Map::new();
        sprag_rpc::ScopeAsk::Attached.write_into(&mut scope);
        assert_eq!(
            serde_json::to_string(&scope).expect("a scope renders"),
            r#"{"attached":true}"#,
            "{}",
            BUMP,
        );

        // The ATTACH TARGET grammar ([`AttachAsk`], R304 + R314), pinned beside the scope because
        // the two travel in ONE params object and a key that started colliding would be invisible
        // from either type alone. EVERY arm, and `Scoped` writing nothing for the same reason
        // `Default` does.
        //
        // Its skew failure is the scope's, one level quieter: an old daemon finds no `last` (or no
        // `step`) key, falls through to the connection's scope — the client's OWN attachment — and
        // answers success, so the gesture is a switch that did nothing and said it had moved.
        let mut target = serde_json::Map::new();
        AttachAsk::Scoped.write_into(&mut target);
        assert!(target.is_empty(), "{}", BUMP);
        AttachAsk::LastViewed { unattached: false }.write_into(&mut target);
        assert_eq!(
            serde_json::to_string(&target).expect("a target renders"),
            r#"{"last":true}"#,
            "{}",
            BUMP,
        );
        // The WINDOW narrowing (R311), which rides BESIDE whichever arm wrote the session rather
        // than replacing one — so both spellings are pinned, and the absent one is pinned as
        // writing nothing at all.
        let mut narrowed = serde_json::Map::new();
        sprag_rpc::ScopeAsk::Named("work".to_owned()).write_into(&mut narrowed);
        sprag_rpc::ScopeAsk::write_window_into(Some("build"), &mut narrowed);
        assert_eq!(
            serde_json::to_string(&narrowed).expect("a scope renders"),
            r#"{"session":"work","window":"build"}"#,
            "{}",
            BUMP,
        );
        let mut wide = serde_json::Map::new();
        sprag_rpc::ScopeAsk::Attached.write_into(&mut wide);
        sprag_rpc::ScopeAsk::write_window_into(None, &mut wide);
        assert_eq!(
            serde_json::to_string(&wide).expect("a scope renders"),
            r#"{"attached":true}"#,
            "{}",
            BUMP,
        );

        let mut target = serde_json::Map::new();
        AttachAsk::LastViewed { unattached: true }.write_into(&mut target);
        assert_eq!(
            serde_json::to_string(&target).expect("a target renders"),
            r#"{"last":true,"unattached":true}"#,
            "{}",
            BUMP,
        );

        // The STEP arm (R314). BOTH words, because the pin's job is the BYTES: a step written as
        // `{"step":"prev"}` would parse here and be dropped by every daemon, and one arm rendered
        // correctly says nothing about the other.
        for (step, bytes) in [
            (OrderStep::Next, r#"{"step":"next"}"#),
            (OrderStep::Previous, r#"{"step":"previous"}"#),
        ] {
            let mut target = serde_json::Map::new();
            AttachAsk::Step(step).write_into(&mut target);
            assert_eq!(
                serde_json::to_string(&target).expect("a target renders"),
                bytes,
                "{}",
                BUMP,
            );
        }

        // The GOTO arm (R315) — ALL THREE DEPTHS, because the wire grammar's whole content is which
        // members are present: a session pick that wrote `"window":null` would be a different
        // request, and a pane pick that dropped its window is one this daemon refuses outright.
        for (ask, bytes) in [
            (
                AttachAsk::Goto {
                    session: SessionId(3),
                    window: None,
                    pane: None,
                },
                r#"{"goto":{"session":3}}"#,
            ),
            (
                AttachAsk::Goto {
                    session: SessionId(3),
                    window: Some(WindowId(7)),
                    pane: None,
                },
                r#"{"goto":{"session":3,"window":7}}"#,
            ),
            (
                AttachAsk::Goto {
                    session: SessionId(3),
                    window: Some(WindowId(7)),
                    pane: Some(PaneId(2)),
                },
                r#"{"goto":{"session":3,"window":7,"pane":2}}"#,
            ),
        ] {
            let mut target = serde_json::Map::new();
            ask.write_into(&mut target);
            assert_eq!(
                serde_json::to_string(&target).expect("a target renders"),
                bytes,
                "{}",
                BUMP,
            );
            // ...and it READS BACK as itself, which the two arms above are not checked for and
            // this one needs: those carry a flag and a word, and this carries three numbers whose
            // ORDER a hand-written reader could transpose without any test noticing.
            assert_eq!(
                AttachAsk::parse(Some(&Value::Object(target))),
                Ok(ask),
                "{}",
                BUMP,
            );
        }

        // The REQUEST half. Both arms, because an argument added to either is a request an older
        // daemon accepts, silently drops, and answers about something else.
        assert_eq!(
            serde_json::to_string(&SelectAsk::Pane(PaneId(7)).to_args()).expect("an ask renders"),
            r#"{"pane":7}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &SelectAsk::Toward {
                    dir: PaneDir::Down,
                    from: Some(PaneId(0)),
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"dir":"down","from":0}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &SelectAsk::Toward {
                    dir: PaneDir::Down,
                    from: None,
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"dir":"down"}"#,
            "{}",
            BUMP,
        );

        // The WINDOW select's grammar (R305), pinned beside the pane's because they are twins and a
        // key that drifted between them would be invisible from either type alone. The named arm
        // especially: it is the shape every client already sent, and a change there would move a
        // request nobody edited.
        assert_eq!(
            serde_json::to_string(&SelectWindowAsk::Named("logs".to_owned()).to_args())
                .expect("an ask renders"),
            r#"{"window":"logs"}"#,
            "{}",
            BUMP,
        );
        for (step, rendered) in [
            (OrderStep::Next, r#"{"relative":"next"}"#),
            (OrderStep::Previous, r#"{"relative":"previous"}"#),
        ] {
            assert_eq!(
                serde_json::to_string(&SelectWindowAsk::Step(step).to_args())
                    .expect("an ask renders"),
                rendered,
                "{}",
                BUMP,
            );
        }

        // The MOVE's grammar (R310), beside the select's for its reason — the two are companions
        // over one order, and a key that drifted between them would be invisible from either type
        // alone. EVERY arm, plus the window field present and absent, because a place is what this
        // verb is FOR: a spelling that moved would send a well-formed request meaning something
        // else, which is the failure the anchored arms have and the bare ones do not.
        for (place, rendered) in [
            (WindowPlace::First, r#"{"place":"first"}"#),
            (WindowPlace::Last, r#"{"place":"last"}"#),
            (WindowPlace::Step(OrderStep::Next), r#"{"place":"next"}"#),
            (
                WindowPlace::Step(OrderStep::Previous),
                r#"{"place":"previous"}"#,
            ),
            (
                WindowPlace::Before("logs".to_owned()),
                r#"{"before":"logs"}"#,
            ),
            (WindowPlace::After("logs".to_owned()), r#"{"after":"logs"}"#),
        ] {
            assert_eq!(
                serde_json::to_string(
                    &MoveWindowAsk {
                        window: None,
                        place,
                    }
                    .to_args()
                )
                .expect("an ask renders"),
                rendered,
                "{}",
                BUMP,
            );
        }
        assert_eq!(
            serde_json::to_string(
                &MoveWindowAsk {
                    window: Some("logs".to_owned()),
                    place: WindowPlace::First,
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"window":"logs","place":"first"}"#,
            "{}",
            BUMP,
        );
        // ...and its ANSWER, which a client parses key by key but through ONE function, so a moved
        // key moves under every caller at once.
        assert_eq!(
            serde_json::to_string(&MoveWindowAsk::answer("logs", PlaceHow::AlreadyThere))
                .expect("an answer renders"),
            r#"{"window":"logs","how":"already_there"}"#,
            "{}",
            BUMP,
        );

        // The swap's grammar, all four spellings: an origin present and absent on each arm, because
        // the origin is a FIELD of both here rather than a variant of its own.
        assert_eq!(
            serde_json::to_string(
                &SwapAsk::With {
                    pane: Some(PaneId(3)),
                    with: PaneId(5),
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"pane":3,"with":5}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &SwapAsk::With {
                    pane: None,
                    with: PaneId(5),
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"with":5}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &SwapAsk::Toward {
                    pane: Some(PaneId(3)),
                    dir: PaneDir::Right,
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"pane":3,"dir":"right"}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &SwapAsk::Toward {
                    pane: None,
                    dir: PaneDir::Right,
                }
                .to_args()
            )
            .expect("an ask renders"),
            r#"{"dir":"right"}"#,
            "{}",
            BUMP,
        );

        // The RESIZE's grammar (R307). It is one arm, but three spellings that matter: the origin
        // present and absent, and the DISTANCE, which is the only number any request grammar in
        // this file carries. A `cells` that stopped being emitted when it equals the default would
        // render identically to a bare ask here and mean something else on a daemon whose default
        // ever changed — which is exactly the absent-argument hazard this half of the pin exists
        // for, with the argument being one this project owns both ends of.
        for (ask, rendered) in [
            (
                ResizeAsk {
                    pane: Some(PaneId(3)),
                    dir: PaneDir::Left,
                    cells: 5,
                },
                r#"{"pane":3,"dir":"left","cells":5}"#,
            ),
            (
                ResizeAsk {
                    pane: None,
                    dir: PaneDir::Down,
                    cells: ResizeAsk::CELLS_DEFAULT,
                },
                r#"{"dir":"down","cells":1}"#,
            ),
        ] {
            assert_eq!(
                serde_json::to_string(&ask.to_args()).expect("an ask renders"),
                rendered,
                "{}",
                BUMP,
            );
            assert_eq!(
                ResizeAsk::parse(&ask.to_args()),
                Some(ask),
                "and what it renders is what it reads back",
            );
        }
    }

    /// What a failing shape pin has to say, since the person reading it is the one who moved the
    /// shape and is the only one who can decide the number.
    const BUMP: &str = "THE WIRE SHAPE CHANGED. An older peer cannot read this, and it will find \
                        out as a type error mid-boot rather than as a sentence. Bump \
                        sprag_rpc::WIRE_PROTOCOL and update this pin, in that order.";

    /// **The wire's whole SURFACE, pinned to the protocol version that serves it** — the ratchet that
    /// turns [`sprag_rpc::WIRE_PROTOCOL`] from a ritual into a decision somebody has to
    /// take.
    ///
    /// # The defect this removes
    ///
    /// Recorded by R313 and re-verified unchanged at R315 and again at R319, each time by RUNNING it:
    /// **reverting the protocol number left the entire suite green** (exit 0, 2268 tests at `4f471bb`).
    /// The number's correctness rested on a SKEW RUN performed by hand, both directions, every round —
    /// which is a ritual, and a ritual is a gate that fails the moment somebody is in a hurry.
    ///
    /// # What it can and cannot prove
    ///
    /// It cannot decide COMPATIBILITY: whether a client of version N still works against a daemon that
    /// gained an action is a judgement about meaning, and no test makes it. What it makes impossible is
    /// making the change SILENTLY. The pair moves together or the suite goes red, and the failure names
    /// the decision:
    ///
    /// * a name ADDED — an older client never asks for it, so the number usually stands;
    /// * a name REMOVED or RENAMED — an older client's request now fails, so the number must rise;
    /// * the number moved with no surface change — legitimate (a MEANING changed under a name), and the
    ///   pin below has to be re-stamped to say so.
    ///
    /// ⚠ **THE FIRST VERSION OF THIS RATCHET READ THE CONSTANTS AND NOT THE DAEMON**, which is the
    /// *one family is not the API* defect committed by the round that closed a six-round-old debt: it
    /// covered `PANE_SCHEMA` + [`MUX_SCHEMA`] directly, so emptying `WorkspaceExternal::schema()`
    /// left it GREEN (measured) and the PLUGIN surface's four addresses — mounted in the daemon's own
    /// scene at `PLUGINS_TAG` — were missing from it entirely. It walks `workspace_scene` now: what
    /// the daemon SERVES, through the same `introspect()` the wire itself resolves a path with.
    ///
    /// The list is deliberately the flat set of NAMES rather than a digest: a digest fails with two hex
    /// strings and leaves the reader to diff by hand, where this fails naming the address that appeared
    /// or vanished.
    ///
    /// ⚠ **It is STAMPED FROM THE DERIVATION, never typed** — the first version was hand-written and the
    /// ratchet refuted it on its first run: a parametric field's ARGUMENT is part of its published
    /// address (`cells.<offset>`, `events.<since>`), and two slots were spelled here by their const's
    /// name rather than by the string it holds. So renaming an ARG moves this surface too, which is
    /// right: an agent reading the schema learns the argument's name from it.
    /// ⚠ **R330 moved the number with this list UNCHANGED, and that is the legitimate case this
    /// assertion names**: `window_id` is a request KEY, not an address, so nothing here moves — but
    /// the MEANING of a `kill_window` request changed under a name that did not. A daemon older
    /// than the key does not refuse `{window_id: 7}`; its `window_target` reads `window` as ABSENT
    /// and kills the SCOPED window instead. That is `DETACHED_KEY`'s case exactly — an added
    /// argument is invisible to `client/hello` and only the version is not — and it is the reason
    /// R329's own two additions did NOT move it: both of those were refused by an older daemon
    /// rather than silently honoured as something else.
    const PINNED_SURFACE: (u32, &[&str]) = (
        16,
        &[
            "agent_manifests",
            "application_cursor_keys",
            "break_pane",
            "cancel",
            "cells.<offset>",
            "clients",
            "clipboard_answer",
            "clipboard_write",
            "close",
            "commands",
            "display_message",
            "drop_file",
            "events.<since>",
            "find.<needle>",
            "focus",
            "frames",
            "full_text",
            "grid_work",
            "image_data.<id>",
            "join_pane",
            "key",
            "kill_session",
            "kill_window",
            "last_command",
            "layout",
            "links",
            "mouse",
            "move_pane",
            "move_window",
            "neighbors.<pane>",
            "new_session",
            "new_window",
            "pane_processes.<max_age_ms>",
            "panes",
            "paste",
            "plugins",
            "project.<pane>",
            "prompt_marks",
            "regex.<pattern>",
            "rename_pane",
            "rename_session",
            "rename_window",
            "resize",
            "resize_pane",
            "resize_window",
            "run",
            "runs",
            "select_pane",
            "select_window",
            "session",
            "session_activity.<max_age_ms>",
            "sessions",
            "set_floating",
            "set_layout",
            "spawn",
            "split",
            "swap_pane",
            "text",
            "tree",
            "window_size",
            "windows",
            "zoom_pane",
        ],
    );

    /// Every address the DAEMON SERVES, read off the scene it assembles for a request — the whole
    /// point of the correction: a schema this module declares and a schema the daemon returns are
    /// two different facts, and only the second one is the wire.
    ///
    /// Walked through `External::introspect`, which is the same accessor `scene/query` resolves a
    /// path with, so a surface reachable by a client is a surface counted here.
    fn served_addresses() -> Vec<String> {
        let registry = std::sync::Arc::new(std::sync::Mutex::new(
            sprag_terminal::SessionRegistry::new((80, 24)),
        ));
        // ⚠ A PANE HAS TO EXIST, and the first version of this fixture had none: the pane surfaces
        // hang under one container per pane, so an empty registry produces a scene serving the mux
        // and the plugins and NOTHING a client types into. A ratchet over that state would have
        // pinned two thirds of the wire and called it whole — the same shape as reading the
        // constants instead of the daemon, one level down.
        {
            let scope = crate::SessionScope::unscoped(&registry);
            let mut command = sprag_terminal::CommandBuilder::new("/bin/sh");
            command.arg("-c");
            command.arg("exec cat");
            command.env("TERM", "dumb");
            crate::lock(scope.workspace())
                .spawn(command, "cat".to_owned(), 20, 4)
                .expect("a pane the pane surface can hang under");
        }
        let scene = crate::workspace_scene(
            &crate::SessionScope::unscoped(&registry),
            &registry,
            &std::sync::Arc::new(std::sync::Mutex::new(crate::runs::RunRegistry::default())),
            &std::sync::Arc::new(crate::notify::ChannelRegistry::default()),
            crate::DaemonShared::default(),
            crate::PaneCells::Omitted,
        );
        let mut found = Vec::new();
        walk(&scene, &mut found);
        found
    }

    /// Collect every external's declared addresses, depth first — a container's children included,
    /// because the pane surfaces hang under one.
    fn walk(scene: &pinion_core::scene::Scene, found: &mut Vec<String>) {
        use pinion_core::scene::Scene;
        match scene {
            Scene::External(node) => {
                if let Some(introspect) = node.handle.introspect() {
                    found.extend(
                        introspect
                            .schema()
                            .fields
                            .iter()
                            .map(|field| field.path.to_owned()),
                    );
                }
            }
            Scene::Container(node) => {
                for child in &node.children {
                    walk(child, found);
                }
            }
            _ => {}
        }
    }

    /// **THE RATCHET: the wire's surface cannot move under the protocol number.**
    ///
    /// See [`PINNED_SURFACE`] for what this can and cannot prove. Measured before it was written, by
    /// running: at `4f471bb`, reverting `WIRE_PROTOCOL` from 15 to 14 left the whole suite green —
    /// six rounds after R313 first wrote that down.
    ///
    /// Both halves are asserted, because they fail differently and a build could pass one while
    /// breaking the other: the NAMES against the two schemas the daemon actually serves, and the
    /// NUMBER against the one a client actually sends.
    #[test]
    fn the_wire_surface_cannot_move_under_the_protocol_number() {
        let mut served = served_addresses();
        served.sort_unstable();
        let mut pinned: Vec<String> = PINNED_SURFACE.1.iter().map(|n| (*n).to_owned()).collect();
        pinned.sort_unstable();
        assert_eq!(
            served, pinned,
            "THE WIRE'S SURFACE MOVED. Update PINNED_SURFACE, and decide what it means for \
             WIRE_PROTOCOL: a name ADDED leaves an older client's requests working (the number \
             usually stands); a name REMOVED or RENAMED breaks them (the number must rise). \
             Then run the skew check both ways, which is the half no test can do for you.",
        );
        assert_eq!(
            PINNED_SURFACE.0,
            sprag_rpc::WIRE_PROTOCOL,
            "THE PROTOCOL NUMBER MOVED WITH THE SURFACE UNCHANGED. That is legitimate when a \
             MEANING changed under a name that did not — re-stamp PINNED_SURFACE's version to say \
             so — and it is a mistake when the number was edited by hand.",
        );
        // ...and no address is served TWICE, which the two schemas being separate makes possible:
        // a client addressing a duplicated name reaches whichever surface the dispatcher tries
        // first, which is a decision nothing here would have made deliberately.
        let mut unique = served.clone();
        unique.dedup();
        assert_eq!(served, unique, "one address, one surface");
    }
}
