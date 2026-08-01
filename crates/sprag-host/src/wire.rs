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

use pinion_core::external::{SchemaArg, SchemaField};

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

/// The client-lifecycle wire vocabulary (R-PR67 Stage 1), re-exported from the transport client
/// that WRITES it ([`sprag_rpc`]) so the host that READS it shares ONE spelling — exactly as
/// [`SESSION_PARAM`] is. [`CLIENT_HELLO_METHOD`] announces a connection's client id
/// ([`CLIENT_PARAM`]); [`CLIENT_ATTACH_METHOD`] declares/switches that client's attached session
/// (reusing [`SESSION_PARAM`]); [`CLIENT_SIZE_METHOD`] reports the cell area that client can give a
/// window ([`COLS_PARAM`] / [`ROWS_PARAM`]), which is the input tmux's `window-size` arbitrates
/// over. All three are intercepted before the generic dispatch core, since they act on the frame's
/// connection id, which no scene external sees. The reader's contract lives in [`crate::rpc`] (the
/// dispatch owner's client-lifecycle intercept); the writer's is on each `sprag_rpc` const.
pub use sprag_rpc::{
    CLIENT_ATTACH_METHOD, CLIENT_HELLO_METHOD, CLIENT_PARAM, CLIENT_SIZE_METHOD, COLS_PARAM,
    ROWS_PARAM,
};

/// The mux control external query slot: every session's name, plus which one an unscoped
/// request acts on — how a client discovers what it can address with [`SESSION_PARAM`].
///
/// Served rather than left to convention: a scope param an agent cannot enumerate is one it
/// must guess at, and the whole point of the scene-as-data surface is that a peer ASKS.
pub const SESSIONS_SLOT: &str = "sessions";

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
/// (`{name?, cmd?, cols?, rows?}`), returning its name.
///
/// Creating one spawns its first pane, mirroring tmux's `new-session -x -y [command]`
/// (`cmd`/`cols`/`rows` shape that pane; absent → `$SHELL` at the pool's default size), so on the
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

/// The mux control external invoke action that spawns a pane, returning its id.
pub const SPAWN_ACTION: &str = "spawn";
/// The mux control external invoke action that DIVIDES a named pane and spawns the new one into
/// the half it opens (`{pane, dir, before?, cmd?, cols?, rows?, remote?}`), returning the new
/// pane's id — tmux `split-window -h` / `-v`.
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
/// `pane` is REQUIRED and has no default, because the daemon has no active-pane concept to mean
/// "here" (the same fact that makes `select-pane` unbuilt): a direction is meaningless without
/// the pane it is relative to, so the caller must name one.
///
/// REFUSED — with nothing spawned and the arrangement untouched — when `pane` holds no leaf in
/// the scoped session's current window: it exited, it is floating, or it is another window's. A
/// split that cannot reach its target must not quietly become an append, which is the same lie as
/// accepting `-h` and ignoring it.
pub const SPLIT_ACTION: &str = "split";
/// The mux control external invoke action that closes a pane.
pub const CLOSE_ACTION: &str = "close";
/// The mux control external invoke action that resizes a pane's PTY + emulator.
pub const RESIZE_ACTION: &str = "resize";
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

/// The mux control external invoke action that creates a window in the SCOPED session, born with
/// a shell, selects it, and returns its name (`{name?, cmd?, cols?, rows?}`) — tmux `new-window`.
///
/// SCOPED (it acts on the request's session), unlike [`NEW_SESSION_ACTION`] which names a session
/// directly. `name` absent ⇒ the lowest free integer; `cmd`/`cols`/`rows` shape the birth pane,
/// exactly as [`NEW_SESSION_ACTION`]. Selecting the new window is session state — every attached
/// client follows it, as tmux does.
pub const NEW_WINDOW_ACTION: &str = "new_window";

/// The mux control external invoke action that makes a window current in the SCOPED session
/// (`{window}`) — tmux `select-window`. Session state: every attached client follows.
pub const SELECT_WINDOW_ACTION: &str = "select_window";

/// The mux control external invoke action that renames a window of the SCOPED session
/// (`{window?, name}`) — tmux `rename-window`. `window` absent ⇒ the current one; `name` is the
/// new name.
pub const RENAME_WINDOW_ACTION: &str = "rename_window";

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
/// `pane` is the id of the pane to move; its source window is DERIVED (a [`PaneId`](sprag_terminal::PaneId)
/// is registry-unique, so the window that holds it is unambiguous — the caller never names the
/// source). `name` absent ⇒ the lowest free integer window name. Refused (`Rejected`) if the pane's
/// window tiles only that one pane, if an explicit `name` is taken, or if no window holds `pane`.
pub const BREAK_PANE_ACTION: &str = "break_pane";

/// The mux control external invoke action that JOINS a pane into another window of the SCOPED
/// session (`{pane, window}`) — tmux `join-pane`. Answers `{closed_source: bool}`.
///
/// `pane` is the id of the pane to move (its source window is DERIVED); `window` is the DESTINATION
/// window's name. The pane appends as a new tiled leaf; a join that empties the source window
/// closes it (`closed_source: true`). Refused (`Rejected`) if `pane` already lives in `window`, if
/// no window holds `pane`, or if `window` names no window.
pub const JOIN_PANE_ACTION: &str = "join_pane";

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
    use pinion_core::external::ArgDomain;

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
}
