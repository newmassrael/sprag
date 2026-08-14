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
use std::time::Duration;

use pinion_core::external::{SchemaArg, SchemaField};
use serde_json::{Map, Value};
use sprag_rpc::RpcFault;
use sprag_terminal::{OrderStep, PaneDir, PaneId, PlaceHow, SessionId, WindowId, WindowPlace};

use crate::{INPUT_TAG, MUX_TAG};

/// The pane-input external invoke action that injects a key (W3C key + mods →
/// PTY bytes, the R2.6 encoder).
///
/// Answers `null`, or [`UNSIGNALLED_KEY`] when what it wrote MEANT a signal this pane will not
/// raise.
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
/// The answer key EVERY pane-input action that writes bytes carries when what it just wrote
/// MEANT a signal and this pane's terminal will raise none — a list of
/// `{`[`key`](UNSIGNALLED_WHICH_KEY)`, `[`because`](UNSIGNALLED_WHY_KEY)`}`, one per distinct key,
/// and ABSENT when there is nothing to say.
///
/// # ⚠⚠⚠ The wait this answers before it is spent
///
/// **Writing `0x03` into a pane is not interrupting its job, and the write succeeds either way.**
/// What makes it a `SIGINT` is the line discipline, and only while `ISIG` is set — which every
/// editor, every full-screen TUI and every interactive agent CLI clears on startup. So a caller
/// that sends `Ctrl-C` and waits for the job to end is, on such a pane, waiting for something it
/// never asked for. Measured (R363): a pane running `stty -isig; sleep 300`, sent `C-c` through
/// this product's own `send-keys`, echoes `^C` and the `sleep` lives on.
///
/// [`STOP_JOB_ACTION`] is what a caller wanted and this is what points at it — it sends the signal
/// to the process group itself, so nothing depends on the terminal's modes.
///
/// ⚠ **The byte is still WRITTEN.** A person's `Ctrl-C` must reach a full-screen program as input,
/// which is how that program learns to cancel its own prompt; refusing the write would break the
/// display client to warn the automation one. This key REPORTS, it does not withhold.
///
/// ⚠⚠ **And the DISPLAY clients deliberately do not consume it.** A person pressing `Ctrl-C` inside
/// an editor MEANT the program to receive it, so a warning there would be noise on the one path
/// where the behaviour is already what was wanted. The caller this exists for is the one that sent
/// the chord to STOP A JOB and cannot see what became of it — an automation client, which reaches
/// the pane through this surface and reads what it answers.
///
/// ⚠ ABSENT rather than empty when there is nothing to report. A caveat on every keystroke is
/// noise, and a reader who learns to skip it is not warned by it.
pub const UNSIGNALLED_KEY: &str = "unsignalled";
/// The [`UNSIGNALLED_KEY`] member naming WHICH key was meant — a
/// [`SignalKey`](sprag_terminal::SignalKey) word.
pub const UNSIGNALLED_WHICH_KEY: &str = "key";
/// The [`UNSIGNALLED_KEY`] member naming WHY no signal followed — an
/// [`Unraised`](sprag_terminal::Unraised) word.
///
/// A closed vocabulary rather than a sentence, because the two causes are two different states of
/// the pane: one says the program is full-screen, the other says the terminal was reconfigured,
/// and a caller retries differently for each.
pub const UNSIGNALLED_WHY_KEY: &str = "because";
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
/// The pane-input external query slot: the pane's output as the LOGICAL LINES THE CHILD WROTE —
/// a JSON array of strings, one per line however the terminal's width broke it
/// ([`Screen::full_lines`](sprag_vt::Screen::full_lines)).
///
/// # ⚠⚠ Why this is an ADDRESS OF ITS OWN and not a shape [`FULL_TEXT_SLOT`] grew
///
/// The two answer different questions about the same pane and both are wanted. `read_pane`
/// publishes *"what a human sees in that pane"* and must keep reporting where the terminal broke
/// each line; anything reasoning about CONTENT — a marker, a model's reply, text to relay — must
/// not have the width in its answer, because **the width belongs to whichever client attached** and
/// two readers of the same output would otherwise disagree without either being able to tell.
///
/// ⚠ An ARRAY, not a string with newlines in it. A `\n` inside [`FULL_TEXT_SLOT`] can be the
/// program's or the terminal's and nothing says which — the ambiguity this address exists to
/// remove. Handing back a joined string would put it straight back.
///
/// ⚠ ADDITIVE: a new address earns no [`WIRE_PROTOCOL`] bump (the rule is written on that constant
/// — *"the added addresses … are additive on their own and would not have earned a bump; the
/// changed value did"*), and [`FULL_TEXT_SLOT`] is untouched, so every existing client keeps the
/// answer it has always read.
pub const FULL_LINES_SLOT: &str = "full_lines";
sprag_vt::closed_set! {
    /// WHOSE LINE BREAKS a pane read reports — the choice between [`FULL_TEXT_SLOT`] and
    /// [`FULL_LINES_SLOT`], named so a caller says which they mean instead of guessing.
    ///
    /// ⚠⚠ A CLOSED SET rather than a boolean, so the published `enum` is DERIVED from the type and
    /// a third source of line breaks (a raw byte capture, say) cannot be added without every
    /// surface that walks this list seeing it.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
    pub enum LineBreaks {
        /// Where the TERMINAL broke each line at the width it had — what a person sees in the
        /// pane, and the answer every existing caller has always been given.
        #[default]
        Screen,
        /// Where the PROGRAM ended each line — what the child actually wrote, with the width taken
        /// out of the answer.
        Program,
    }
}

impl LineBreaks {
    /// The published word for this choice — the single spelling, which the tool schema's `enum` is
    /// generated from.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Screen => "screen",
            Self::Program => "program",
        }
    }

    /// The choice a published word names, or `None` for a word this build does not know.
    ///
    /// ⚠ Derived by walking [`Self::ALL`] rather than by a second `match`, which is the shape that
    /// let `AgentState` publish three words and decode them in a hand-written list beside it.
    #[must_use]
    pub fn from_wire(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.wire_str() == word)
    }

    /// The pane-input slot that answers this question.
    #[must_use]
    pub const fn slot(self) -> &'static str {
        match self {
            Self::Screen => FULL_TEXT_SLOT,
            Self::Program => FULL_LINES_SLOT,
        }
    }
}

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
/// output — `find.<needle>` — as `{matches: [{line, row, col, cols, wrapped?}], lines, truncated}`.
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
/// The answer names both of a pane's axes ([`FindMatch`](sprag_vt::FindMatch)), because a search
/// crosses them: `line` is the LOGICAL line, counted by the retained row it begins on — the
/// [`PROMPT_MARKS_SLOT`] axis, so a client jumps to a match with the scroll `offset_y` it already
/// speaks — while `row`/`col`/`cols` say where the match's first cell sits and `wrapped` carries
/// the widths it covers on the rows it runs on to. A match that fits on its row omits `wrapped`
/// entirely. An EMPTY needle is a malformed member and answers `Null`, the same shape a malformed
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

/// The EMPTY member of a parametric family, as the address it is.
///
/// # ⚠⚠⚠ TEN ADDRESSES THIS DAEMON SERVED AND NEVER PUBLISHED
///
/// A parametric family has a member whose argument is EMPTY — `find.` beside `find.<needle>` — and
/// this daemon has always answered it, on purpose: `Null`, meaning *"that search is nothing"*,
/// which is a different fact from `find` (**not an address at all**). Three cases, three answers,
/// and the middle one is the reason a caller can tell *"this build has no find"* from *"I sent an
/// empty box"*.
///
/// **What was never true is that the schema said so.** Four `query` slots in this workspace carry a
/// comment claiming *"the path IS in the schema"* and it was not, in any of them. Measured across
/// both published surfaces: **eleven parametric families, ten of which answer their empty member,
/// and not one of the ten was declared.**
///
/// Nothing could see it until pinion R1637 (`dd9743eb`) made the declaration a GATE — *"a call must
/// be declared first"* — at which point an undeclared address stopped being reachable. Two of the
/// ten had gates and went red; **the other eight had been silently unreachable with nothing in the
/// workspace to say so.** That gate is right, and this is the defect it found.
///
/// Derived from the family rather than spelled beside it, because a hand-written twin is the list
/// the twelfth family is left out of — and the reading is checked from the other end too, by a gate
/// that asks each live surface which empty members it ANSWERS and compares that with what its
/// schema DECLARES.
///
/// ⚠ It takes the family's own `ty` rather than a type of its own. A family's declared type already
/// permits `Null` for a malformed member — `cells.<offset>` says `frame` and answers `Null` for an
/// offset that addresses nothing — so a second type word here would publish a distinction the
/// surface does not make.
#[must_use]
pub const fn empty_member_of(family: &SchemaField) -> SchemaField {
    SchemaField::new(literal_prefix_of(family.path), family.ty)
}

/// The literal run of `template` before its first placeholder — pinion's
/// [`SchemaField::literal_prefix`](pinion_core::external::SchemaField::literal_prefix), in a form a
/// `const` schema can call.
///
/// It exists only because that accessor is not `const`; a template with no placeholder answers
/// itself, exactly as pinion's does. The split is on a byte, and safe: every placeholder in this
/// workspace is preceded by ASCII, so the cut can never land inside a codepoint — and if it somehow
/// did, this answers the whole template rather than reaching for `unsafe`.
const fn literal_prefix_of(template: &'static str) -> &'static str {
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            break;
        }
        i += 1;
    }
    let (head, _) = bytes.split_at(i);
    match std::str::from_utf8(head) {
        Ok(prefix) => prefix,
        Err(_) => template,
    }
}

/// ⚠⚠⚠ **WHAT A SURFACE ANSWERS FOR AN EMPTY ARGUMENT IS WHAT ITS SCHEMA MUST DECLARE** — read
/// from the LIVE surface, in both directions, for every parametric family it publishes.
///
/// This is the gate for [`empty_member_of`], and it is written against the surface rather than
/// against the array because the array is the thing that was wrong: ten families answered an
/// address none of them declared, for as long as they have existed, and the four comments claiming
/// *"the path IS in the schema"* were the only record of a contract nothing enforced.
///
/// Both directions, because they fail differently and each is a real defect:
///
/// * OWNED and not DECLARED — the address is unreachable through a daemon whose boundary gates
///   on the declaration (pinion R1637 onward), and the surface's own doc is a false statement.
/// * DECLARED and not OWNED — `$schema` advertises an address the surface disclaims, so a client
///   enumerating the schema to build a call builds one the daemon says it has never heard of.
///
/// # ⚠⚠⚠ R372: THE QUESTION IS OWNERSHIP, AND IT USED TO BE *"answers with a value"*
///
/// That predicate was right for exactly as long as an empty member answered `Null`. R372 gave every
/// parametric family its own refusal, so `events.` now answers `QueryTypeMismatch` — *a declared
/// family whose argument is not the declared type, **including an empty one*** (pinion R1667's
/// definition, which made that the SURFACE's call rather than the matcher's).
///
/// So all eleven flipped to *"refuses"* at once and the gate went red for the change that made it
/// correct. **A refusal naming the type is not the surface disclaiming the address — it is the
/// surface owning it and saying what is wrong**, which is the whole point of declaring it: without
/// the declaration the boundary answers `UnknownIntrospectPath` (*"not in my schema"*, false) and
/// the caller never reaches the sentence that would have helped them.
///
/// The one answer that still means *not mine* is [`ReadRefusal::UnknownPath`], so that is what this
/// asks about.
///
/// Placed here, beside the declarations, rather than in either surface's test module, because ONE
/// rule over TWO published schemas is exactly the duplication this module exists to prevent — and a
/// third surface gets it by calling this instead of by remembering.
#[cfg(test)]
pub(crate) fn assert_empty_members_are_declared(
    schema: &'static [SchemaField],
    surface: &str,
    mut answers: impl FnMut(&str) -> bool,
) {
    let mut families = 0_usize;
    for family in schema {
        let empty = literal_prefix_of(family.path);
        if empty == family.path {
            // A scalar, an action, or an already-declared empty member: no placeholder, no
            // separate address to reconcile.
            continue;
        }
        families += 1;
        let declared = schema.iter().any(|field| field.path == empty);
        let answered = answers(empty);
        assert_eq!(
            declared,
            answered,
            "`{surface}` disagrees with itself about `{empty}`, the EMPTY member of \
             `{}`: the schema {} it and the surface {} it. An address a surface owns and does \
             not declare is unreachable through the declaration gate and its doc is false; one it \
             declares and disclaims is advertised to every client that reads `$schema`.",
            family.path,
            if declared { "declares" } else { "omits" },
            if answered { "owns" } else { "disclaims" },
        );
    }
    assert!(
        families > 0,
        "`{surface}` published no parametric family at all, so this gate measured nothing — it is \
         written against a schema that has some",
    );
}

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
    SchemaField::action(KEY_ACTION, "action"),
    SchemaField::action(MOUSE_ACTION, "action"),
    SchemaField::action(TEXT_ACTION, "action"),
    SchemaField::action(FOCUS_ACTION, "action"),
    SchemaField::action(PASTE_ACTION, "action"),
    CELLS_FIELD,
    empty_member_of(&CELLS_FIELD),
    SchemaField::new(FRAMES_SLOT, "int"),
    SchemaField::new(CURSOR_KEYS_SLOT, "bool"),
    SchemaField::new(FULL_TEXT_SLOT, "string"),
    SchemaField::new(FULL_LINES_SLOT, "array"),
    SchemaField::new(LAST_COMMAND_SLOT, "object"),
    SchemaField::new(PROMPT_MARKS_SLOT, "array"),
    SchemaField::new(LINKS_SLOT, "array"),
    FIND_FIELD,
    empty_member_of(&FIND_FIELD),
    REGEX_FIELD,
    empty_member_of(&REGEX_FIELD),
    IMAGE_DATA_FIELD,
    empty_member_of(&IMAGE_DATA_FIELD),
    SchemaField::new(CLIPBOARD_WRITE_SLOT, "object"),
    SchemaField::action(CLIPBOARD_ANSWER_ACTION, "action"),
    // HOW TO CALL THE SIX VERBS ABOVE — this surface's own [`PANE_GRAMMAR`]. A client that
    // holds a pane's path can ask that pane what its input verbs take, which is the read that made
    // every argument on this surface folklore until R353.
    SchemaField::new(ACTION_GRAMMAR_SLOT, "object"),
];

/// Whether `schema` DECLARES `path` as a verb — the guard a surface runs before it dispatches, so
/// a verb it does not publish is a verb it does not run.
///
/// # The defect this is a guard for rather than a test
///
/// ⚠⚠ **`report_agent` and `release_agent` were dispatched by the mux surface and declared nowhere**
/// from the round that built them until R352, and `activate` was the same on the GUI's hyperlink
/// oracle. Nothing could catch that: every gate over a surface walks its DECLARED fields, so an
/// omission declares nothing to audit — pinion says so of its own `IntrospectSchema` and it is true
/// of sprag's gates word for word. The only thing that closes it is making the undeclared arm
/// UNREACHABLE.
///
/// # Why sprag runs it as well as pinion
///
/// pinion refuses an undeclared `scene/invoke` at the RPC boundary from R1637 and its own docs name
/// what that leaves open: *"In-process dispatch. A binding that calls `ExternalIntrospect::invoke`
/// directly — a keybinding forwarding a verb, say — does not pass through here, and the framework
/// has no seam that could intercept it."* **A keybinding forwarding a verb is most of how sprag is
/// used**, and the GUI's own surfaces are driven in-process by its shell — so the check belongs at
/// each surface's own door, where both callers pass.
///
/// Declared HERE, in the module that claims to be the ONE definition of the wire's grammar, because
/// the display crate needs the same rule for its own externals and two spellings of one rule is
/// what this module exists to prevent.
///
/// The cost is one linear scan of a `&'static [SchemaField]` per action — paid at keystroke
/// cadence, not per frame.
#[must_use]
pub fn declares_verb(schema: &pinion_core::external::IntrospectSchema, path: &str) -> bool {
    schema.fields.iter().any(|field| {
        field.path == path && field.channel == pinion_core::external::SchemaChannel::Invoke
    })
}

/// THE SHAPES A PUBLISHED GRAMMAR IS MADE OF, re-exported so this module stays the one door onto the
/// wire's grammar.
///
/// They live in [`sprag_rpc::grammar`] because the conformance harness that drives these declarations
/// against a live surface has to reach them from every crate that serves one, and neither a
/// `#[cfg(test)]` module nor a dev-dependency cycle on this crate could deliver that — the cycle
/// compiled and linked two different `sprag_host` crates, so the types were not the same types. The
/// TABLES stayed here, where the actions they name are.
pub use sprag_rpc::grammar::{ActionGrammar, ArgGrammar, CallForm, FormKind, WireSurface};

/// The request grammar of the SPAWNING verbs, and of the three that carry a closed vocabulary —
/// declared here rather than beside an ask type, because these verbs read their arguments inline
/// out of the request map and have no ask type to be read off.
///
/// ⚠⚠ **A HAND-WRITTEN DECLARATION IS ONLY AS GOOD AS WHAT HOLDS IT TO THE PARSER**, and until
/// R352's third gate there was nothing, which is why [`MUX_GRAMMAR`] began with ask-backed
/// verbs alone. Three gates hold every one of these now, by RUNNING them against the daemon:
/// each published word is accepted, each open string argument is genuinely open, and — the one
/// that makes hand-writing safe — **each declared argument is refused at the wrong TYPE**, which a
/// parser that never reads the key cannot do.
///
/// What none of them catches is an argument the parser reads and this omits: absent-not-wrong, a
/// client told less rather than something false. Only an ask type closes that, and giving these
/// verbs one is the next round's mechanical work.
pub struct InlineGrammar;

impl InlineGrammar {
    /// The pane a spawning verb's child is named after, and where it starts — the four keys
    /// `parse_spawn` reads, plus the two its callers read beside it.
    ///
    /// All optional: a bare `spawn` with no arguments at all opens the user's default shell, which
    /// is the commonest call on this wire.
    const BIRTH: &'static [ArgGrammar] = &[
        ArgGrammar::open(SPAWN_CMD_KEY, "array").optional(),
        ArgGrammar::open(SPAWN_CWD_KEY, "string").optional(),
        ArgGrammar::open(SPAWN_COLS_KEY, "int").optional(),
        ArgGrammar::open(SPAWN_ROWS_KEY, "int").optional(),
        ArgGrammar::open(SPAWN_NAME_KEY, "string").optional(),
        ArgGrammar::open(WINDOW_OPENED_BY_KEY, "int").optional(),
    ];

    /// [`SPAWN_ACTION`] — the birth keys and nothing else.
    pub const SPAWN: &'static [CallForm] = &[CallForm::object(Self::BIRTH)];

    /// [`SPLIT_ACTION`] — a pane to divide, WHICH WAY, and the birth keys for the child that fills
    /// the half that opens.
    ///
    /// `dir` is the vocabulary that had no type at all before R352: the two words were matched as
    /// string literals inside the parser, so they could not be published. They are
    /// [`SplitDir::WIRE_WORDS`](sprag_terminal::SplitDir::WIRE_WORDS) now, and the parser reads
    /// through the same definition.
    pub const SPLIT: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int").optional(),
        ArgGrammar::one_of(
            SPLIT_DIR_KEY,
            "string",
            &sprag_terminal::SplitDir::WIRE_WORDS,
        ),
        ArgGrammar::open(SPLIT_BEFORE_KEY, "bool").optional(),
        ArgGrammar::open(SPAWN_CMD_KEY, "array").optional(),
        ArgGrammar::open(SPAWN_CWD_KEY, "string").optional(),
        ArgGrammar::open(SPAWN_COLS_KEY, "int").optional(),
        ArgGrammar::open(SPAWN_ROWS_KEY, "int").optional(),
        ArgGrammar::open(SPAWN_NAME_KEY, "string").optional(),
        ArgGrammar::open(WINDOW_OPENED_BY_KEY, "int").optional(),
    ])];

    /// [`DISPLAY_MESSAGE_ACTION`] — what to say, how loudly, and to whom.
    ///
    /// `severity` publishes [`Severity`](crate::report::Severity)'s three words. An absent one is
    /// the default rather than a refusal, which is why it is optional.
    pub const DISPLAY_MESSAGE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(MESSAGE_TEXT_KEY, "string"),
        ArgGrammar::one_of(
            MESSAGE_SEVERITY_KEY,
            "string",
            &crate::report::Severity::WIRE_WORDS,
        )
        .optional(),
        ArgGrammar::open(MESSAGE_CLIENT_KEY, "string").optional(),
    ])];

    /// The key that leaves the session where it is, on the two verbs that create a window.
    ///
    /// One declaration each rather than a shared ARRAY, because a `const fn` cannot concatenate
    /// slices — so the two verbs spell the same two arguments by NAMING them, which is
    /// [`WindowRef::NAMED_ARG`]'s arrangement and stops the pair drifting between them.
    const DETACHED_ARG: ArgGrammar = ArgGrammar::open(DETACHED_KEY, "bool").optional();
    /// The key naming the pane whose occupant asked for the window — [`DETACHED_ARG`](Self::DETACHED_ARG)'s peer.
    const OPENED_BY_ARG: ArgGrammar = ArgGrammar::open(WINDOW_OPENED_BY_KEY, "int").optional();

    /// [`CLOSE_ACTION`] — the pane to end. Absent means the session's active one.
    pub const CLOSE: &'static [CallForm] =
        &[CallForm::object(
            &[ArgGrammar::open("id", "int").optional()],
        )];

    /// [`STOP_JOB_ACTION`] — a pane, and WHICH stop to deliver to its job. Absent `signal` asks for
    /// the one a person's `Ctrl-C` means, because that is what *"stop this"* means to everybody who
    /// is not thinking about signals — and the harder two must be typed out on purpose.
    pub const STOP_JOB: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int"),
        ArgGrammar::one_of(
            STOP_JOB_SIGNAL_KEY,
            "string",
            &sprag_terminal::Stop::WIRE_WORDS,
        )
        .optional(),
    ])];

    /// [`ZOOM_PANE_ACTION`] — a pane, and whether to zoom it. Absent `on` TOGGLES.
    pub const ZOOM_PANE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int").optional(),
        ArgGrammar::open("on", "bool").optional(),
    ])];

    /// [`SET_FLOATING_ACTION`] — a pane, and which way to move it across the tiling's edge.
    pub const SET_FLOATING: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open("id", "int"),
        ArgGrammar::open("floating", "bool"),
    ])];

    /// [`DROP_FILE_ACTION`] — a pane, and the path a display client dropped on it.
    pub const DROP_FILE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int"),
        ArgGrammar::open("path", "string"),
    ])];

    /// [`RELEASE_AGENT_ACTION`] — the pane whose agent claim is given up.
    ///
    /// [`REPORT_AGENT`](Self::REPORT_AGENT)'s other half, and the second verb R352 found declared
    /// nowhere at all.
    pub const RELEASE_AGENT: &'static [CallForm] =
        &[CallForm::object(&[ArgGrammar::open(AGENT_ID_KEY, "int")])];

    /// [`RESIZE_ACTION`] — the pane a display client is telling the host it is showing at.
    ///
    /// # ⚠⚠ It was EXEMPTED as a nested value and it is FLAT
    ///
    /// `SURFACES` listed this among the three verbs that publish nothing, with the reason *"a
    /// client's cell metrics"* — a nested object the flat grammar could not describe. Reading the
    /// parser says otherwise: it takes five keys, none of them an object, and the grammar that has
    /// existed since R352 describes it exactly. **A filing's own diagnosis is a claim** (R337), and
    /// this one was inherited through three rounds without anybody re-deriving it.
    ///
    /// The two `cell_*` keys are the display's font metric, optional because a headless client has
    /// none — and their absence has a MEANING (leave the pane's last-known geometry alone), which
    /// is why `ArgGrammar::optional`'s doc says optional never means unimportant.
    pub const RESIZE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(AGENT_ID_KEY, "int"),
        ArgGrammar::open(SPAWN_COLS_KEY, "int"),
        ArgGrammar::open(SPAWN_ROWS_KEY, "int"),
        ArgGrammar::open("cell_width", "int").optional(),
        ArgGrammar::open("cell_height", "int").optional(),
    ])];

    /// [`GRANT_PANE_ACTION`] — what ONE pane is allowed of the machine.
    ///
    /// Exempted as *"a share object"* on [`RESIZE`](Self::RESIZE)'s terms, and flat for the same
    /// reason: three optional numbers beside a pane, no object anywhere.
    ///
    /// ⚠ **THE GRAMMAR CANNOT SAY "AT LEAST ONE OF THESE"** and the daemon refuses a grant that
    /// sets nothing — deliberately, because a grant with no settings is somebody who meant
    /// something and typed it wrong. That is a semantic rule rather than a shape, so it stays where
    /// it can be stated in words: publishing all three as required would be false, and there is no
    /// form-level alternation that means "any non-empty subset".
    pub const GRANT_PANE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int"),
        ArgGrammar::open("share", "int").optional(),
        ArgGrammar::open("memory", "int").optional(),
        ArgGrammar::open("processes", "int").optional(),
    ])];

    /// [`RENAME_PANE_ACTION`] — a pane, and what to call it. An absent name CLEARS.
    pub const RENAME_PANE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int"),
        ArgGrammar::open(SPAWN_NAME_KEY, "string").optional(),
    ])];

    /// [`RENAME_SESSION_ACTION`] — what to call the request's own session.
    pub const RENAME_SESSION: &'static [CallForm] = &[CallForm::object(&[ArgGrammar::open(
        SPAWN_NAME_KEY,
        "string",
    )])];

    /// [`KILL_SESSION_ACTION`] — which session to end. Named, never scoped: ending the one you are
    /// attached to by omission is not a thing this verb lets a caller do by accident.
    pub const KILL_SESSION: &'static [CallForm] = &[CallForm::object(&[ArgGrammar::open(
        SPAWN_NAME_KEY,
        "string",
    )])];

    /// [`RENAME_WINDOW_ACTION`] — which window, and what to call it.
    pub const RENAME_WINDOW: &'static [CallForm] = &[CallForm::object(&[
        WindowRef::NAMED_ARG,
        ArgGrammar::open(SPAWN_NAME_KEY, "string"),
    ])];

    /// [`KILL_WINDOW_ACTION`] — which window to end, by either spelling, or the scoped one.
    pub const KILL_WINDOW: &'static [CallForm] = &[
        CallForm::object(&[WindowRef::NAMED_ARG]),
        CallForm::object(&[WindowRef::PICKED_ARG]),
    ];

    /// [`NEW_WINDOW_ACTION`] — a name for the window, how it is born, and the birth keys for the
    /// pane that fills it.
    pub const NEW_WINDOW: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPAWN_NAME_KEY, "string").optional(),
        Self::DETACHED_ARG,
        Self::OPENED_BY_ARG,
        ArgGrammar::open(SPAWN_CMD_KEY, "array").optional(),
        ArgGrammar::open(SPAWN_CWD_KEY, "string").optional(),
        ArgGrammar::open(SPAWN_COLS_KEY, "int").optional(),
        ArgGrammar::open(SPAWN_ROWS_KEY, "int").optional(),
    ])];

    /// [`NEW_SESSION_ACTION`] — a name for the session, and the birth keys for its first pane.
    pub const NEW_SESSION: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPAWN_NAME_KEY, "string").optional(),
        ArgGrammar::open(SPAWN_CMD_KEY, "array").optional(),
        ArgGrammar::open(SPAWN_CWD_KEY, "string").optional(),
        ArgGrammar::open(SPAWN_COLS_KEY, "int").optional(),
        ArgGrammar::open(SPAWN_ROWS_KEY, "int").optional(),
    ])];

    // ⚠ THE FOUR VERBS THAT PUBLISH NOTHING, AND WHY — a stated boundary rather than an oversight.
    //
    // `set_layout` takes an arrangement TREE, `resize` takes a client's cell metrics, and
    // `grant_pane` takes a nested share object. `ArgGrammar` describes a FLAT key: a name, a type
    // and the vocabulary it admits. Declaring `{"tree": "object"}` would be true and would tell a
    // client nothing it did not already know, which is the affirmative-noise version of the
    // affirmative false statement this whole surface avoids. A nested grammar is a real design
    // question — pinion's own `SchemaArg` cannot express one either — and it is not this round's.
    //
    // The fourth is `WindowBirthAsk`'s `new_window` half, which IS published above: the ask models
    // only the birth, so the verb's grammar is declared here where its other half can be said too.

    /// [`BREAK_PANE_ACTION`] — the pane to take out of its window, and the window it becomes.
    pub const BREAK_PANE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int"),
        ArgGrammar::open(SPAWN_NAME_KEY, "string").optional(),
        Self::DETACHED_ARG,
        Self::OPENED_BY_ARG,
    ])];

    /// [`MOVE_PANE_ACTION`] — a pane, the pane it lands beside, and which side of it.
    ///
    /// The third verb carrying [`SplitDir`](sprag_terminal::SplitDir)'s two words, after
    /// [`SPLIT`](Self::SPLIT).
    pub const MOVE_PANE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(SPLIT_PANE_KEY, "int").optional(),
        ArgGrammar::open("target", "int"),
        ArgGrammar::one_of(
            SPLIT_DIR_KEY,
            "string",
            &sprag_terminal::SplitDir::WIRE_WORDS,
        ),
        ArgGrammar::open(SPLIT_BEFORE_KEY, "bool").optional(),
    ])];

    /// [`REPORT_AGENT_ACTION`] — an agent reporting its own turn, the SCE requirement's verb.
    ///
    /// ⚠ This is the verb R352 found DISPATCHED AND DECLARED NOWHERE. `state` publishes the three
    /// words a reporter may name
    /// ([`AgentState::REPORTED_WORDS`](sprag_detect::AgentState::REPORTED_WORDS)) — `unknown` is
    /// excluded by the same predicate the parser refuses it with, because it is a conclusion about
    /// the rules rather than a state a reporter is in.
    pub const REPORT_AGENT: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open(AGENT_ID_KEY, "int"),
        ArgGrammar::open(AGENT_SOURCE_KEY, "string"),
        ArgGrammar::one_of(
            AGENT_STATE_KEY,
            "string",
            &sprag_detect::AgentState::REPORTED_WORDS,
        ),
        ArgGrammar::open(AGENT_NAME_KEY, "string").optional(),
        ArgGrammar::open(AGENT_SEQ_KEY, "int").optional(),
        ArgGrammar::open(AGENT_BIND_KEY, "bool").optional(),
    ])];
}

/// The request grammar of the PLUGIN-HOST verbs — how a client starts and cancels a plugin run.
///
/// # The surface a gate found, not a person
///
/// R353's own new coverage gate derived the list of surfaces serving verbs from the SERVED SCENE and
/// immediately named a third one that [`SURFACES`] did not: `sprag_plugins`, whose two verbs had
/// published nothing since they existed. That is the hand-written-list rule catching the round's own
/// hand-written list, one hour old, which is the strongest argument for deriving a list there is.
///
/// # `run` is ONE FORM PER BUNDLED PLUGIN, and the list is projected rather than written
///
/// The `plugin` word chooses which arguments the rest of the call carries, so a flat list would have
/// said every one of them was optional — the same defect R352's first draft shipped for `move_window`.
/// The vocabulary is [`PluginName`](crate::plugins::PluginName)'s own, which the `plugins` slot
/// publishes as its list too: one definition, two readers.
///
/// # `guardrails` is published PER FORM, and that is what removed its alternation
///
/// It was declared `object` with its inner keys unnamed, because a nested value had no grammar. It
/// has one now ([`ArgGrammar::fields`]) and the three keys are named — but the interesting half is
/// that each form publishes only the cost key ITS plugin admits. `max_bytes` and `max_tokens` are
/// mutually exclusive on the wire (a run has one cost unit, and
/// [`parse_max_cost`](crate::plugins) refuses a mismatched one), which a flat list of optional keys
/// could not have said. The UNIT is a property of the plugin, so it is a property of the form that
/// selects the plugin: a byte-relay form offers `max_bytes`, the dialogue form offers `max_tokens`,
/// and there is no alternation left to describe.
///
/// ⚠ The residue: `max_iterations`, `max_seconds` and the form's own cost key are each optional and
/// can be given together, which is true — they are three independent axes and not an alternation.
/// What is NOT published is the DEFAULT each takes when omitted — that is
/// the `guardrail_defaults` slot, because a default is a fact about this daemon rather than about
/// the request's shape.
pub struct PluginGrammar;

impl PluginGrammar {
    /// The liveness bound every form of `run` accepts — the iteration ceiling.
    ///
    /// Declared once and shared by EVERY form, because the LOOP bound is unit-free where the
    /// COST bound is not.
    const MAX_ITERATIONS: ArgGrammar = ArgGrammar::open("max_iterations", "int").optional();

    /// The WALL-CLOCK bound every form of `run` accepts — the run's deadline, in seconds.
    ///
    /// Beside [`MAX_ITERATIONS`](Self::MAX_ITERATIONS) and shared by every form for the same
    /// reason: time, like the loop count, is unit-free where the COST bound is not — a second is a
    /// second whether the run spends bytes or tokens.
    ///
    /// ⚠ **SECONDS, where the per-turn `timeout_ms` beside it is MILLISECONDS**, and the two are
    /// different scales because they bound different things. `timeout_ms` bounds ONE reply, and a
    /// caller tuning it is thinking about how long a model takes to answer. This bounds the whole
    /// LOOP, and a caller setting it is saying *"do not spend more than ten minutes on this"* —
    /// a sentence with no millisecond in it. Spelling the unit into both names is what keeps a
    /// reader of the published grammar from having to guess which scale a bare number is in.
    const MAX_SECONDS: ArgGrammar = ArgGrammar::open("max_seconds", "int").optional();

    /// The guardrail object a BYTE-RELAY form takes — `orchestrator`, `pipe` and `agent` all spend
    /// injected PTY bytes.
    const GUARDRAILS_BYTES: ArgGrammar = ArgGrammar::nested(
        "guardrails",
        &[
            Self::MAX_ITERATIONS,
            Self::MAX_SECONDS,
            ArgGrammar::open("max_bytes", "int").optional(),
        ],
    )
    .optional();

    /// The guardrail object the DIALOGUE form takes — it spends LLM tokens, so a byte bound cannot
    /// guard it and is not offered.
    const GUARDRAILS_TOKENS: ArgGrammar = ArgGrammar::nested(
        "guardrails",
        &[
            Self::MAX_ITERATIONS,
            Self::MAX_SECONDS,
            ArgGrammar::open("max_tokens", "int").optional(),
        ],
    )
    .optional();

    /// The keys a `guardrails` object of `run` may carry, in the unit a given plugin spends.
    ///
    /// # ⚠⚠ Why the PARSER reads the publication instead of listing these again
    ///
    /// An unknown key inside `guardrails` was accepted and ignored, which for a BOUND is the worst
    /// answer there is: every other argument that is ignored makes the verb do LESS than it was
    /// asked, and a bound that is ignored makes the run do MORE — unboundedly more, silently, with
    /// a success reply. `guardrails: {"max_secnods": 5}` was a run with no time ceiling and no way
    /// to find out.
    ///
    /// So [`parse_guardrails`](crate::plugins) refuses a key that is not declared here, and reads
    /// the declaration rather than a list of its own: R352's rule that the predicate the parser
    /// applies and the vocabulary the surface publishes must be ONE thing, or the publication is an
    /// affirmative false statement the first time they drift.
    #[must_use]
    pub fn guardrail_fields(unit: &str) -> &'static [ArgGrammar] {
        let declared = if unit == sprag_plugin::Cost::Tokens(0).unit() {
            &Self::GUARDRAILS_TOKENS
        } else {
            &Self::GUARDRAILS_BYTES
        };
        declared.fields
    }

    /// WHO ASKED for this run — the pane whose occupant wants it, on
    /// [`sprag_terminal::Pane::opened_by`]'s exact terms.
    ///
    /// Optional, and its absence means a run nobody claims — which is what a person starting one
    /// from a shell is. It is PROVENANCE and not authorisation: this wire has no authentication at
    /// all, so a caller can say anything, exactly as it can for a pane's `opened_by`. What it buys
    /// is that the agent-facing mouth can keep an agent to its OWN runs without the daemon having
    /// to grow a notion of identity it does not have.
    const OPENED_BY: ArgGrammar = ArgGrammar::open("opened_by", "int").optional();

    /// ⚠⚠ **EACH FORM'S `plugin` PUBLISHES ONLY THE WORD THAT SELECTS IT**, and that is what makes an
    /// alternation over a VALUE readable at all.
    ///
    /// Every other alternation on this wire is told apart by which KEYS a form carries — `select_pane`
    /// takes a `pane` or a `dir`. These forms ALL carry `plugin`, and differ by its value, so
    /// publishing the whole vocabulary on every one of them would have left a client a pile of key
    /// sets and no way to know that `src`/`dst` is the `pipe` one. A one-word vocabulary per form
    /// says it
    /// exactly, and the UNION over the forms is still the whole set — nothing is hidden, and
    /// `an_argument_the_daemon_constrains_publishes_what_it_admits` drives each word inside the form
    /// that admits it.
    ///
    /// The words are not re-spelled here: each const names the VARIANT and reads its own
    /// [`wire_str`](crate::plugins::PluginName::wire_str).
    const ORCHESTRATOR: &'static [&'static str] =
        &[crate::plugins::PluginName::Orchestrator.wire_str()];
    /// `pipe`'s own word — see [`ORCHESTRATOR`](Self::ORCHESTRATOR).
    const PIPE: &'static [&'static str] = &[crate::plugins::PluginName::Pipe.wire_str()];
    /// `agent`'s own word — see [`ORCHESTRATOR`](Self::ORCHESTRATOR).
    const AGENT: &'static [&'static str] = &[crate::plugins::PluginName::Agent.wire_str()];
    /// `dialogue`'s own word — see [`ORCHESTRATOR`](Self::ORCHESTRATOR).
    const DIALOGUE: &'static [&'static str] = &[crate::plugins::PluginName::Dialogue.wire_str()];
    /// `answer`'s own word — see [`ORCHESTRATOR`](Self::ORCHESTRATOR).
    const ANSWER: &'static [&'static str] = &[crate::plugins::PluginName::Answer.wire_str()];
    /// `ai_loop`'s own word — see [`ORCHESTRATOR`](Self::ORCHESTRATOR).
    const AI_LOOP: &'static [&'static str] = &[crate::plugins::PluginName::AiLoop.wire_str()];

    /// The `plugin` discriminator at the one word that selects this form.
    const fn selected_by(word: &'static [&'static str]) -> ArgGrammar {
        ArgGrammar::one_of("plugin", "string", word)
    }

    /// [`RUN_ACTION`](crate::plugins::RUN_ACTION) — one form per bundled plugin, in
    /// [`PluginName::ALL`](crate::plugins::PluginName) order so a form added to the type is a form
    /// this table has to decide about.
    /// [`RUN_ACTION`](crate::plugins::RUN_ACTION)'s forms, ONE PER BUNDLED PLUGIN, projected from
    /// [`PluginName::ALL`](crate::plugins::PluginName) rather than written out.
    ///
    /// # ⚠⚠ Why this is a projection and not the array it used to be
    ///
    /// The four forms were a hand-written list beside a four-variant type, and the type's own doc
    /// claimed *"adding a variant reaches the wire in the compile that adds it"*. That was true of
    /// the plugin's WORD — published from `WIRE_WORDS` — and false of the thing a client actually
    /// needs, which is how to CALL it. A fifth plugin would have been advertised as a legal
    /// `plugin` value by a surface that said nothing about its arguments, and every gate here would
    /// have passed: they walk the forms that exist.
    ///
    /// [`PluginName::form`](crate::plugins::PluginName::form) is an exhaustive match, so a variant
    /// added to the type does not compile until somebody says how to call it. **The omission is
    /// unrepresentable rather than checked** — the shape R352 asks for, since a gate over a
    /// declaration cannot see one that was never made.
    pub const RUN: &'static [CallForm] = &{
        let mut forms = [CallForm::object(&[]); crate::plugins::PluginName::ALL.len()];
        let mut at = 0;
        while at < crate::plugins::PluginName::ALL.len() {
            forms[at] = crate::plugins::PluginName::ALL[at].form();
            at += 1;
        }
        forms
    };

    /// The readiness barrier, on every form whose plugin INJECTS.
    ///
    /// ⚠ An OBJECT and not a needle, and `match` is REQUIRED inside it. A marker on its own cannot
    /// say whether text already on the screen is evidence — for a program the caller just started
    /// it is not (the likeliest such text is the ECHO of the command line that started it), and for
    /// a REPL already at its prompt it is the only evidence there will be. See
    /// [`ReadyWhen`](sprag_plugin::ReadyWhen); the words come from that type, never from literals.
    ///
    /// ⚠⚠ **`marker` MEANS WHATEVER `match` SAYS IT MEANS**, and one of the three words makes it
    /// not a screen needle at all: under `runs` it is a PROGRAM NAME, matched against the job that
    /// owns the pane's terminal, with no screen read. That is the word to prefer — it is the only
    /// one a program that prints nothing on startup can be waited for by, and no amount of typing
    /// the name can satisfy it.
    ///
    /// ⚠ **`runs` was added to `match` WITHOUT a `WIRE_PROTOCOL` bump, and that is the rule rather
    /// than an oversight**: R342 settled that widening an argument's VALUE SPACE is not what earns
    /// one. Nothing a pre-`runs` client sends changes meaning, and a client that sends `runs` to a
    /// daemon without it meets an ordinary grammar refusal at the door — the words are published
    /// here, so it can ask first.
    pub const READY_WHEN: ArgGrammar = ArgGrammar::nested(
        "ready_when",
        &[
            ArgGrammar::one_of("match", "string", sprag_plugin::ReadyWhen::WIRE_WORDS),
            ArgGrammar::open("marker", "string"),
        ],
    )
    .optional();

    /// The `agent` form's COMPLETION contract — what makes the peer's turn over, the mirror of
    /// [`READY_WHEN`](Self::READY_WHEN) at the other end of the same turn.
    ///
    /// # ⚠⚠⚠ Why this earns a protocol number where `ready_when`'s `runs` did not
    ///
    /// R342 settled that widening an argument's VALUE SPACE does not earn one, which is why
    /// `ready_when` gained a fourth word for free: a client sending an unknown word meets an
    /// ordinary grammar refusal at the door, because the words are PUBLISHED and it can ask first.
    ///
    /// A whole added ARGUMENT is the opposite, and it is measured rather than argued —
    /// `an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused` sends a key no
    /// version has ever declared and the run **starts and converges**. So a caller naming
    /// `settles` to a daemon that predates this key is answered `ok`, waits for an exit that a
    /// long-lived peer will never make, and gets back a snapshot taken when its timeout ran out —
    /// under the same shape of answer a working call returns. That is version 23's failure exactly.
    ///
    /// # ⚠⚠ A BARE WORD, and the first draft's two defects that made it one
    ///
    /// It was `{"match": …, "agent": …}` — [`READY_WHEN`](Self::READY_WHEN)'s shape, copied — and
    /// the conformance gates refused it twice in one run, each for a reason worth keeping:
    ///
    /// * **`done_when.match` and `ready_when.match` collide when FLATTENED.** A mouth that offers
    ///   nested arguments one flag at a time (`--match`) could not say which of the two a caller
    ///   meant, and this form carries both. `no_surface_publishes_a_nested_argument_that_collides`
    ///   named it in both directions.
    /// * **The wire published `settles` and the daemon REFUSED it.** `agent` was optional in the
    ///   grammar and required by that word, so an agent enumerating the vocabulary would build a
    ///   call this daemon cannot read — `every_published_word_is_a_word_the_plugin_host_accepts`.
    ///
    /// Both dissolve once the companion argument goes away, and it could: the agent's NAME is not
    /// the caller's to give at this end of the turn — see
    /// [`DoneWhen::Settles`](sprag_plugin::DoneWhen::Settles). One word, no nesting, nothing to
    /// collide with, and every published word is one the parser takes alone.
    pub const DONE_WHEN: ArgGrammar =
        ArgGrammar::one_of("done_when", "string", sprag_plugin::DoneWhen::WIRE_WORDS).optional();

    /// WHAT THIS RUN MAY ANSWER if its peer stops to ASK — on every form whose plugin INJECTS, the
    /// third of the turn's three declared contracts.
    ///
    /// # ⚠⚠⚠ Why BOTH needles are required, and why neither is a number
    ///
    /// An agent that stops to ask shows a numbered menu, so the obvious argument is a NUMBER — and
    /// it is the one shape this must not have. A number means a different thing in every dialog
    /// (`sprag_detect::Choice::number` is read off the screen precisely because a list that has
    /// scrolled does not start at one), so *"always press 2"* is a consent to whatever happens to
    /// be second, which nobody can have agreed to.
    ///
    /// So the caller quotes the agent's own words twice. `asked` says WHICH QUESTION this is about
    /// — without it, a `Yes` authorised for *"overwrite the draft?"* answers *"delete the
    /// production database?"*. `answer` says WHICH OPTION, and it must name exactly one: a needle
    /// carried by two options answers NEITHER, because the measured shape of a real permission
    /// dialog is `Yes` / `Yes, and don't ask again` / `No` and a first-match policy grants a
    /// standing permission the day an agent reorders its list.
    ///
    /// ⚠ **ABSENT IS THE DEFAULT AND IT ANSWERS NOTHING**, which is what every run did before this
    /// key existed. The run reports `blocked` with the question, and a person answers it.
    ///
    /// ⚠ An OBJECT and not a bare word, unlike [`DONE_WHEN`](Self::DONE_WHEN): the two needles are
    /// independent values a caller supplies, so there is no closed vocabulary to spell.
    ///
    /// # ⚠⚠⚠ A LIST of those objects, because ONE TURN ASKS MORE THAN ONE QUESTION
    ///
    /// Measured (R370): an agent turn that runs a command and then edits a file asks *"Bash command
    /// … Do you want to proceed?"* and then *"Edit file … Do you want to make this edit?"*. With one
    /// clause an unattended run answered the first and stopped at the second reporting
    /// `other_question` — correct, and still a run somebody has to come back to, which is the case
    /// the whole argument exists to serve. So the caller writes one clause PER QUESTION they have
    /// decided about, and [`sprag_plugin::Consents`] owns what several of them say about one dialog
    /// (including the one failure a list makes possible: two clauses that disagree answer NEITHER).
    ///
    /// ⚠ Because it is a list, this is the one nested argument a mouth must NOT flatten — see
    /// [`ArgGrammar::nested_list`]. N occurrences of a flat `--asked` beside N of a flat `--answer`
    /// cannot say which belongs with which, so both flattening mouths offer it whole.
    pub const MAY_ANSWER: ArgGrammar = ArgGrammar::nested_list(
        sprag_plugin::Consents::WIRE_KEY,
        &[
            ArgGrammar::open(sprag_plugin::Consent::ASKED_KEY, "string"),
            ArgGrammar::open(sprag_plugin::Consent::ANSWER_KEY, "string"),
        ],
    )
    .optional();

    /// **WHAT A LOOP TURNS DOWN AND WHAT IT SAYS INSTEAD** — the AUTHOR's standing instructions,
    /// where [`MAY_ANSWER`](Self::MAY_ANSWER) is the CALLER's consent.
    ///
    /// # ⚠⚠⚠ Two authorities over one dialog, and why the second is not the first widened
    ///
    /// A consent TAKES AN OPTION THE PEER OFFERED. That covers every dialog whose answer is on the
    /// menu — measured, one clause quoting *"Do you want to"* covers what three tool families ask —
    /// and it structurally cannot cover the question a loop meets when its agent wants a DECISION:
    /// *"Which way should I build this — the quick one, or the thorough one?"* has no option a
    /// caller could have authorised in advance, because the answer they want is not being offered.
    ///
    /// A screen rule is the other act. It **refuses the call** and tells the agent what to do
    /// instead, which is what a person does all day and what no consent can express.
    ///
    /// ⚠⚠⚠ **A RULE NAMES NO KEY, AND THAT IS THE SAFETY PROPERTY.** The key belongs to the product
    /// (`sprag_plugin::REFUSES`), measured against a live `claude` 2.1.232: Escape makes it report
    /// `User rejected` and the file is never written, identically to the offered `3. No`, in 25 ms.
    /// The same probe found that `Tab` — the agent's own *"amend"* — leaves the dialog UP and
    /// rewrites option 1 into *"Yes, and tell Claude what to do next"*, so typing into it APPROVES.
    /// If a rule could name its own key it could name that one, and *"nobody gets a standing
    /// permission by writing a rule that happened to match"* would be a hope. Here it is a property.
    ///
    /// ⚠ **`when` QUOTES THE DIALOG**, exactly as [`Consent::asked`](sprag_plugin::Consent::asked)
    /// does and through the same matcher. The loop document used to match a dialog KIND
    /// (`design-decision`, …), which needed somebody to classify another program's dialogs into a
    /// taxonomy nothing in this workspace maintains; R383 measured that quoting the agent's own
    /// words covers what a taxonomy would have.
    ///
    /// ⚠ **ABSENT MEANS THE DOCUMENT'S OWN RULES**, not *"screen nothing"* — these live in the loop
    /// template, so a caller who says nothing is not overriding an author who did. An EMPTY list is
    /// malformed for [`MAY_ANSWER`](Self::MAY_ANSWER)'s reason: it is a second spelling of absent.
    ///
    /// ⚠ Nested and never flattened, [`MAY_ANSWER`](Self::MAY_ANSWER)'s rule: N flat `--when`s
    /// beside N flat `--text`s cannot say which belongs with which.
    pub const SCREEN_RULES: ArgGrammar = ArgGrammar::nested_list(
        sprag_plugin::ScreenRules::WIRE_KEY,
        &[
            ArgGrammar::open(sprag_plugin::ScreenRule::WHEN_KEY, "string"),
            ArgGrammar::open(sprag_plugin::ScreenRule::TEXT_KEY, "string"),
        ],
    )
    .optional();

    /// **WHETHER ANYBODY IS WATCHING the pane this run drives, and for how long** — the other half
    /// of [`MAY_ANSWER`](Self::MAY_ANSWER), and the argument that makes a SUPERVISED loop
    /// expressible.
    ///
    /// # ⚠⚠⚠ The case a blocked run could not tell apart, and got wrong for half its callers
    ///
    /// Every refusal the answering contract can report ends its sentence the same way — **hand the
    /// pane to a person** — and until this key existed a run acted on that by STOPPING. That is the
    /// only honest thing to do when the pane is on a screen nobody is looking at, and the wrong
    /// thing when the run IS the inner session of a loop somebody is watching: the pane is on their
    /// desk, they read every turn as it happens, and they can answer the dialog with their own
    /// hands. Measured (R371): a run whose supervisor answered the dialog a moment later had
    /// already reported `blocked` — in FORTY MILLISECONDS — and their answer landed in a pane
    /// nothing was driving any more.
    ///
    /// ⚠⚠ **IT WIDENS WHAT A RUN MAY WAIT FOR AND NOTHING IT MAY DECIDE.** A waiting run still
    /// types nothing: `may_answer` remains the only thing that can put a byte into a dialog, and
    /// this wait ends when the PERSON has moved the peer off the question. The two keys are read
    /// at one door ([`sprag_plugin::Readiness`]) so that stays true by construction.
    ///
    /// ⚠ A DURATION and not a flag, because a bare *"somebody is watching"* would need a patience
    /// from somewhere and the only somewhere is a default nobody chose. **Zero is malformed**, not
    /// a quiet *"nobody"* — [`Attended::of`](sprag_plugin::Attended::of) owns that predicate the
    /// way `Consents::of` owns the empty list's.
    ///
    /// ⚠ NOT on the `answer` form. That one is CALLED BY the person a wait would be waiting for, so
    /// waiting there would block a supervisor on their own answer — the same reasoning that keeps
    /// its consent to one clause.
    pub const AWAIT_PERSON: ArgGrammar =
        ArgGrammar::open(sprag_plugin::Attended::WIRE_KEY, "int").optional();

    /// WHEN A PANE THIS RUN'S PERSON TAKES BECOMES THIS RUN'S AGAIN, in milliseconds of a still
    /// hand. Absent is *"never"* — the run ends, which is what every run did before this key.
    ///
    /// # ⚠⚠⚠ The other half of `taken_over`, and the half the loop document always asked for
    ///
    /// R372 taught a run to notice a person typing into a pane it was driving and to stop. That is
    /// half of `ai_loop.scxml`, whose `awaiting_human` is a WAITING state with four exits and only
    /// one of them ends the run. Measured before this key existed: a supervisor who typed ONE key
    /// into a pane a loop was driving, finished, and let go, left a run holding **thirty-seven of
    /// its forty iterations unspent** with its goal one turn away. Somebody had to restart it by
    /// hand.
    ///
    /// ⚠⚠ **IT IS READ ONLY BESIDE `await_person_ms`, AND A CALL THAT SENDS IT ALONE IS MALFORMED.**
    /// The pair is one request, and the type says so —
    /// [`Handback`](sprag_plugin::Handback) lives INSIDE
    /// [`Attended::APerson`](sprag_plugin::Attended::APerson), so *"give the pane back to a run
    /// nobody is watching"* cannot be constructed. A daemon that answered `NoOne` to half a request
    /// would give the caller a run that ENDS on the first keystroke, which is the opposite of what
    /// they sent.
    ///
    /// ⚠ **Zero is malformed**, not a quiet *"never"* —
    /// [`Handback::of`](sprag_plugin::Handback::of) owns the predicate, `Attended::of`'s rule: every
    /// person pauses between keystrokes, so *"the pane is mine again the instant they pause"* is not
    /// a thing a caller can mean.
    ///
    /// ⚠ NOT on the `answer` form, for [`AWAIT_PERSON`](Self::AWAIT_PERSON)'s reason exactly: that
    /// form is called BY the person, and a run that waited for their hand to go still would be
    /// waiting for the caller to stop calling it.
    pub const HANDBACK_STILL: ArgGrammar =
        ArgGrammar::open(sprag_plugin::Handback::WIRE_KEY, "int").optional();

    /// The SAME consent, REQUIRED — the `answer` form, whose whole content it is.
    ///
    /// # ⚠⚠⚠ Why the one argument that is optional everywhere else is mandatory here
    ///
    /// On the looping forms a consent is a thing a run MAY be given, and its absence is the
    /// default that answers nothing. This form has no other content: a run told to answer a pane
    /// with no consent has been told to type nothing at a peer that is asking, which is what NOT
    /// calling it does, for free, without occupying a run slot.
    ///
    /// ⚠ And the consequence is worth stating, because it removes a whole arm from what a caller
    /// can meet: [`Refusal::NoConsent`](sprag_plugin::Refusal::NoConsent) is UNREACHABLE through
    /// this form. The grammar refuses the call at the door, so every refusal an `answer` run can
    /// report is one about the question on the screen — something the caller can fix by re-reading
    /// the dialog rather than by re-reading their own call.
    ///
    /// ⚠ It is the same [`ArgGrammar`] value, one `.optional()` short, and NOT a second spelling:
    /// a client that can build a consent for `orchestrate` hands the identical object here.
    pub const MUST_ANSWER: ArgGrammar = ArgGrammar::nested_list(
        sprag_plugin::Consents::WIRE_KEY,
        &[
            ArgGrammar::open(sprag_plugin::Consent::ASKED_KEY, "string"),
            ArgGrammar::open(sprag_plugin::Consent::ANSWER_KEY, "string"),
        ],
    );

    /// HOW LONG ONE TURN MAY TAKE — the bound half of the looping forms' turn contract, whose
    /// other half is [`DONE_WHEN`](Self::DONE_WHEN).
    ///
    /// ⚠⚠⚠ **THE PAIR IS ONE REQUEST**, and the daemon refuses either half alone — see
    /// `opt_turn`. Published as two optional keys because that is what the grammar can say; the
    /// COUPLING is a fact about the type ([`Turn`](sprag_plugin::Turn) holds both) and is stated in
    /// the daemon's refusal rather than in a shape this vocabulary can express. That residue is the
    /// same one `await_person_ms` / `handback_still_ms` already carry.
    pub const TURN_WITHIN: ArgGrammar =
        ArgGrammar::open(sprag_plugin::Turn::WIRE_KEY, "int").optional();

    /// `orchestrator` — drive ONE pane with a stimulus until a sentinel appears.
    pub const ORCHESTRATOR_FORM: CallForm = CallForm::object(&[
        Self::selected_by(Self::ORCHESTRATOR),
        ArgGrammar::open("pane", "int"),
        ArgGrammar::open("stimulus", "string"),
        ArgGrammar::open("sentinel", "string").optional(),
        // ⚠⚠⚠ THE TURN CONTRACT, and this is the form that MEASURED why it had to exist: without
        // it a step ends on a 500 ms constant, so a peer that thinks for three seconds was asked
        // its one question SIX times. See [`Turn`](sprag_plugin::Turn).
        Self::DONE_WHEN,
        Self::TURN_WITHIN,
        Self::READY_WHEN,
        ArgGrammar::open("ready_timeout_ms", "int").optional(),
        Self::MAY_ANSWER,
        Self::AWAIT_PERSON,
        Self::HANDBACK_STILL,
        Self::OPENED_BY,
        Self::GUARDRAILS_BYTES,
    ]);

    /// `pipe` — relay one pane's output into another's input.
    ///
    /// ⚠ It takes the SAME readiness barrier the orchestrator does, and needs it more: a relay's
    /// destination is a pane somebody else prepared. See
    /// [`PipeSpec::ready_when`](sprag_plugin::PipeSpec::ready_when).
    pub const PIPE_FORM: CallForm = CallForm::object(&[
        Self::selected_by(Self::PIPE),
        ArgGrammar::open("src", "int"),
        ArgGrammar::open("dst", "int"),
        Self::READY_WHEN,
        ArgGrammar::open("ready_timeout_ms", "int").optional(),
        Self::MAY_ANSWER,
        Self::AWAIT_PERSON,
        Self::HANDBACK_STILL,
        Self::OPENED_BY,
        Self::GUARDRAILS_BYTES,
    ]);

    /// `agent` — prompt the agent in a pane and collect its reply.
    pub const AGENT_FORM: CallForm = CallForm::object(&[
        Self::selected_by(Self::AGENT),
        ArgGrammar::open("pane", "int"),
        ArgGrammar::open("prompt", "string"),
        ArgGrammar::open("eof", "bool").optional(),
        ArgGrammar::open("shows_prompt", "bool").optional(),
        ArgGrammar::open("timeout_ms", "int").optional(),
        Self::DONE_WHEN,
        Self::READY_WHEN,
        ArgGrammar::open("ready_timeout_ms", "int").optional(),
        Self::MAY_ANSWER,
        Self::AWAIT_PERSON,
        Self::HANDBACK_STILL,
        Self::OPENED_BY,
        Self::GUARDRAILS_BYTES,
    ]);

    /// `dialogue` — two endpoints against each other, turn by turn. It spawns its OWN panes, which
    /// is why it names argv templates instead of a pane.
    pub const DIALOGUE_FORM: CallForm = CallForm::object(&[
        Self::selected_by(Self::DIALOGUE),
        ArgGrammar::open("endpoint_a", "array"),
        ArgGrammar::open("endpoint_b", "array"),
        ArgGrammar::open("seed", "string"),
        ArgGrammar::open("label_a", "string").optional(),
        ArgGrammar::open("label_b", "string").optional(),
        // The two reply formats, published from `ReplyFormat`'s own words — two string literals
        // inside the host's parser until R353.
        ArgGrammar::one_of("format_a", "string", &sprag_plugin::ReplyFormat::WIRE_WORDS).optional(),
        ArgGrammar::one_of("format_b", "string", &sprag_plugin::ReplyFormat::WIRE_WORDS).optional(),
        ArgGrammar::open("cols", "int").optional(),
        ArgGrammar::open("rows", "int").optional(),
        ArgGrammar::open("timeout_ms", "int").optional(),
        Self::OPENED_BY,
        Self::GUARDRAILS_TOKENS,
    ]);

    /// `answer` — ANSWER THE QUESTION ONE PANE'S PEER IS ASKING, once, and stop.
    ///
    /// # ⚠⚠⚠ The form that makes the safe act reachable where the question is published
    ///
    /// R367 put what a peer is asking on the pane-level surface — the options, and which one a bare
    /// Enter would take — and left no way to answer it there. What a caller had instead was
    /// `send_keys`: a raw digit and a raw Enter, carrying none of the guarantees the other four
    /// forms get from [`MAY_ANSWER`](Self::MAY_ANSWER). The number is a SCREEN fact that a
    /// re-render invalidates, the Enter lands wherever the peer has got to, nothing checks that the
    /// key was taken, and nothing records that a machine answered at all.
    ///
    /// This form is the same contract with no loop around it. It takes the pane and the consent and
    /// nothing else — there is no stimulus, so the only bytes it can ever emit are the ones
    /// [`Consent::covers`](sprag_plugin::Consent::covers) authorised on the question that is on the
    /// screen at the moment it looks.
    ///
    /// ⚠ **NO `ready_when`, and its absence is the design.** A pane whose program has not started
    /// cannot be showing a dialog, so there is nothing for a readiness barrier to wait for. The
    /// barrier is still the door — see [`Answer`](sprag_plugin::Answer) — but the question it is
    /// asked here is only ever the answering one.
    pub const ANSWER_FORM: CallForm = CallForm::object(&[
        Self::selected_by(Self::ANSWER),
        ArgGrammar::open("pane", "int"),
        Self::MUST_ANSWER,
        Self::OPENED_BY,
        Self::GUARDRAILS_BYTES,
    ]);

    /// `ai_loop` — RUN THE OUTER LOOP'S STATECHART AGAINST AN AGENT IN A PANE.
    ///
    /// # ⚠⚠⚠ The door the register had been holding open since R378
    ///
    /// `ai_loop.scxml` compiled at R376, got a turn with two endings at R377, got a driver at R378,
    /// was measured against a live `claude` at R379 and converged against one at R380 — and
    /// **nothing in the daemon constructed one and no surface started one.** This form is that
    /// surface. It is additive in the way this wire's own rule calls free: a new ACTION or a new
    /// FORM is not what earns a protocol number, and a client sending `ai_loop` to a daemon that
    /// predates it meets an ordinary vocabulary refusal at the door, because the `plugins` slot
    /// publishes the word and it can ask first.
    ///
    /// # ⚠⚠ The four brief keys, and why a loop takes them where an `agent` run takes a prompt
    ///
    /// An `agent` run carries the exact text it will send. A loop composes each turn's prompt
    /// itself, in the document's own words, out of what it is FOR — so what a caller supplies is
    /// the purpose and not the sentence. `north_star` is where the loop is ultimately going and is
    /// never rewritten; `milestone` is the step being worked on now; `reference` is prior art the
    /// agent should read first. All three are required, and that is measured rather than strict:
    /// the document ships `(edit me)` placeholders for exactly these, and a live agent read three
    /// of its five clauses as `(edit me)` until R380.
    ///
    /// ⚠⚠ **`max_turns` COUNTS THE AGENT'S TURNS AND IS NOT A GUARDRAIL**, which is why it is here
    /// and not in `guardrails`. One turn of a real agent is many steps of the loop driving it, so
    /// `max_iterations` cannot express *"give this agent eight turns"* — and a run stopped by this
    /// number reports `exhausted` with the ceiling `turns`, so a reader is told which knob to turn.
    ///
    /// ⚠⚠⚠ **`reflect_every` USED TO HAVE TO BE AT LEAST `max_turns`, AND NO LONGER DOES.** A smaller
    /// one reaches `reflecting`, and that state — improve the loop's own setup, then close the inner
    /// session and open a fresh one that reads it — is served. What a caller gets by naming a smaller
    /// number is a run that periodically replaces its agent's session, which is what lets one run
    /// outlive one agent's context.
    ///
    /// ⚠⚠ **ITS DEFAULT IS STILL `max_turns`**, deliberately, and that is not the old refusal in
    /// disguise: a restart closes a pane somebody may be reading and opens another, so a caller who
    /// has said nothing about reflection has not asked for it. What they get without asking is the
    /// reflection a STANDING INSTRUCTION triggers — the document's own `screened > screened_carried`
    /// edge — because that one is a correctness edge and not a budget: without it, a loop told *"do it
    /// another way"* is asked for the original way on the very next turn.
    ///
    /// # ⚠⚠⚠ And the three the loop was the ONLY injecting form without
    ///
    /// `may_answer`, `await_person_ms` and `handback_still_ms` were left off this form on an
    /// argument rather than a measurement: *"answering a dialog is `screening`'s job in the
    /// document"*. That is a true sentence about a state nothing drives, and what it cost was
    /// measured — **a loop whose agent asked one permission question stopped with ZERO turns
    /// judged**, and no argument on this whole form could have covered it.
    ///
    /// Every kind of work a real loop does raises such a question: an agent that edits a file, runs
    /// a command or fetches a URL asks first. So a loop takes the same answering contract every
    /// other injecting form takes, and a question no clause covers still reaches the machine's own
    /// `turn.blocked`. See [`sprag_plugin::AiLoopSpec::may_answer`], which holds the argument for
    /// why two authorities over one dialog is the right number rather than a duplication.
    ///
    /// ⚠ **`agent` IS THE PROGRAM NAME, and it is what the barrier waits for**, under
    /// [`ReadyWhen::Settles`](sprag_plugin::ReadyWhen) — the one barrier word a program that prints
    /// nothing on startup can be waited for by. R379 measured what its absence costs: a loop typed
    /// its first prompt into a pane whose agent had existed for ten milliseconds, the
    /// pseudoterminal's own echo confirmed the delivery, and the run then sat in `working` for as
    /// long as anyone let it.
    pub const AI_LOOP_FORM: CallForm = CallForm::object(&[
        Self::selected_by(Self::AI_LOOP),
        ArgGrammar::open("pane", "int"),
        ArgGrammar::open("north_star", "string"),
        ArgGrammar::open("milestone", "string"),
        ArgGrammar::open("reference", "string"),
        ArgGrammar::open("max_turns", "int"),
        ArgGrammar::open("reflect_every", "int").optional(),
        // ⚠⚠⚠ REQUIRED, and the conformance sweep is what settled it. It was declared optional and
        // read with `require_str`, which is the exact defect `DONE_WHEN` beside it records from
        // version 25: the wire published an argument as declinable and the daemon refused every
        // call that declined it, so an agent building a call from the published form gets
        // `TypeMismatch` for a request the grammar says is well-formed.
        //
        // It is required rather than optional-with-a-default because there IS no honest default: a
        // loop with no barrier types its first prompt into whatever the pane happens to be running
        // — R379 measured that costing a whole run — and only the caller knows which program is in
        // their pane.
        ArgGrammar::open("agent", "string"),
        Self::READY_WHEN,
        ArgGrammar::open("ready_timeout_ms", "int").optional(),
        Self::DONE_WHEN,
        ArgGrammar::open(sprag_plugin::Turn::WIRE_KEY, "int").optional(),
        ArgGrammar::open("shows_prompt", "bool").optional(),
        Self::MAY_ANSWER,
        // ⚠⚠⚠ THE OTHER AUTHORITY, and the only form that has it: `screen_rules` are the loop
        // DOCUMENT's, so they exist nowhere else. See [`SCREEN_RULES`](Self::SCREEN_RULES) for why
        // a consent cannot be widened into one.
        //
        // ⚠⚠⚠ AND ARMING ONE NOW MEANS A RULE THAT FIRES **REPLACES THE AGENT'S SESSION**, which a
        // caller has to know because nothing else on this form implies it. An instruction that was
        // said once lived exactly as long as that agent's context — measured as ONE delivery against
        // SIX re-issues of the milestone it overrode — so making it stick means composing it into the
        // prompts, and the prompts are composed when a session STARTS. The loop therefore reflects at
        // the very next judgement and restarts, once per distinct instruction. ⚠ It happens whatever
        // `reflect_every` says: that argument is a BUDGET, and this is a correctness edge.
        Self::SCREEN_RULES,
        Self::AWAIT_PERSON,
        Self::HANDBACK_STILL,
        Self::OPENED_BY,
        Self::GUARDRAILS_BYTES,
    ]);

    /// [`CANCEL_ACTION`](crate::plugins::CANCEL_ACTION) — the run to stop.
    pub const CANCEL: &'static [CallForm] = &[CallForm::object(&[ArgGrammar::open("id", "int")])];
}

/// The request grammar of the PANE-INPUT verbs — the six ways a client drives what is inside a pane.
///
/// # Every one of these was folklore, and four vocabularies had no definition
///
/// `key`, `mouse`, `focus`, `text`, `paste` and `clipboard_answer` read their arguments inline like
/// [`InlineGrammar`]'s verbs, and until this table they published nothing at all: `$schema` named the
/// six addresses and said not one word about what to send them. A client that wanted to type a
/// Ctrl-C had to know the modifier key names, the object shape, and — for a mouse report — twelve
/// words that existed only inside the host's own `match`.
///
/// Those words have types now, in the crates that own the concepts:
/// [`MouseButton`](sprag_input::MouseButton), [`MouseEventKind`](sprag_input::MouseEventKind),
/// [`KeyEdge`](sprag_input::KeyEdge) and [`ClipboardTarget`](sprag_vt::ClipboardTarget). Two of them
/// were spelled TWICE — the display client encoded the vocabulary and the host decoded it, in two
/// crates, with nothing comparing the two lists.
///
/// The same three gates hold this table that hold [`InlineGrammar`]'s, driven through a LIVE pane
/// surface over a real PTY: every published word accepted, every open string argument genuinely open,
/// and every declared argument refused at the wrong type.
///
/// ⚠ The KEY NAMES are literals here where [`InlineGrammar`]'s come from shared consts, because the
/// pane parsers read literals too — and inventing a const one side of the wire reads would be a
/// second definition dressed as a first. What ties these names to the parser is not a shared token
/// but execution: `a_declared_argument_is_one_the_daemon_reads` sends each one at the wrong type and
/// requires a refusal, which a key the daemon does not read cannot produce.
pub struct PaneGrammar;

impl PaneGrammar {
    /// The key a `key` action reports, in its object form — a W3C `KeyboardEvent.key` string, which
    /// is the caller's own value and has no vocabulary to publish (a printable character, or one of
    /// [`NAMED_KEYS`](sprag_input::NAMED_KEYS)).
    const KEY_ARG: ArgGrammar = ArgGrammar::open("key", "string");

    /// **WHOSE KEYSTROKES THESE ARE** — on the OBJECT form of every input verb that writes.
    ///
    /// # ⚠⚠⚠ The argument a display client on a socket could not do without
    ///
    /// A pane records whose hand wrote each input ([`sprag_terminal::Hand`]) so a run can stop
    /// driving a pane a person has taken. That worked for everything except the case it was built
    /// for: **both frontends attach over this socket**, so a person's keystroke arrived at the one
    /// door that stamped *a program*, and a supervised run never saw them. Measured end to end
    /// through a real `sprag-tui` before this key existed — the run converged as though nobody had
    /// touched the pane.
    ///
    /// ⚠ **ABSENT MEANS A PROGRAM**, which is every existing caller's behaviour unchanged and the
    /// half that cannot be claimed by silence: an unauthenticated caller cannot pass for a person by
    /// omitting an argument. A word outside the vocabulary is malformed.
    ///
    /// ⚠ **NOT ON THE SCALAR FORMS.** A bare string has nowhere to carry a second argument, and
    /// that is the right shape rather than a limitation: the scalar spellings are the cheap answer
    /// to *"how do I press `a`"*, and a client faithful enough to say whose hand it holds is
    /// sending the object form already.
    ///
    /// ⚠ Not on `mouse` or `focus`. Those are stamped a program even when a person moved the
    /// mouse — a hover would make a false positive of the whole signal, and a focus edge is raised
    /// by the window system, which has no hand at all.
    const HAND_ARG: ArgGrammar = ArgGrammar::one_of(
        sprag_terminal::Hand::WIRE_KEY,
        "string",
        &sprag_terminal::Hand::WIRE_WORDS,
    );

    /// [`KEY_ACTION`] — a keystroke, either spelling.
    ///
    /// ⚠ The SCALAR form first, because it is the shorter answer to *"how do I press `a`"*, and a
    /// client reading forms in order meets the cheap one before the complete one. The object form is
    /// strictly more expressive: a bare string cannot carry a modifier or an edge.
    pub const KEY: &'static [CallForm] = &[
        CallForm::scalar(&Self::KEY_ARG),
        CallForm::object(&[
            Self::KEY_ARG,
            // The two words that lived as string literals inside `parse_key_args`. An absent `state`
            // means `down`, which is why it is optional — and `up` is ACCEPTED and injects nothing,
            // so a client that faithfully reports both halves of a keystroke is not refused.
            ArgGrammar::one_of("state", "string", &sprag_input::KeyEdge::WIRE_WORDS).optional(),
            ArgGrammar::open("ctrl", "bool").optional(),
            ArgGrammar::open("alt", "bool").optional(),
            ArgGrammar::open("shift", "bool").optional(),
            // The logo key. Named `super` on the wire (the W3C spelling) and `sup` on
            // [`Modifiers`](sprag_input::Modifiers), which is why the key is written here rather than
            // derived from the field.
            ArgGrammar::open("super", "bool").optional(),
            Self::HAND_ARG.optional(),
        ]),
    ];

    /// The literal UTF-8 a `text` or `paste` action writes.
    const TEXT_ARG: ArgGrammar = ArgGrammar::open("text", "string");

    /// [`TEXT_ACTION`] — literal UTF-8, not key-encoded. The scalar form is what an IME commit
    /// sends; both forms accept the EMPTY string, which writes nothing (the composition-cancel
    /// shape).
    pub const TEXT: &'static [CallForm] = &[
        CallForm::scalar(&Self::TEXT_ARG),
        CallForm::object(&[Self::TEXT_ARG, Self::HAND_ARG.optional()]),
    ];

    /// [`PASTE_ACTION`] — [`TEXT`](Self::TEXT)'s grammar exactly, and deliberately the same
    /// declaration rather than a copy of it: the two verbs differ in what the HOST does with the text
    /// (bracketing it when the child enabled DEC 2004), never in what a client sends.
    pub const PASTE: &'static [CallForm] = Self::TEXT;

    /// [`MOUSE_ACTION`] — a button edge at a cell.
    ///
    /// ⚠⚠ **`button` and `kind` are the two vocabularies that were spelled once per side of the
    /// wire** — see [`MouseButton::wire_str`](sprag_input::MouseButton::wire_str). Publishing them is
    /// what makes a mouse report writable by a client that has read the grammar and nothing else.
    ///
    /// No `super`: a mouse report has no encoding for the logo key, so the parser does not read one,
    /// and a surface that does not read a key does not declare it.
    pub const MOUSE: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::one_of("button", "string", &sprag_input::MouseButton::WIRE_WORDS),
        ArgGrammar::one_of("kind", "string", &sprag_input::MouseEventKind::WIRE_WORDS),
        ArgGrammar::open("col", "int"),
        ArgGrammar::open("row", "int"),
        ArgGrammar::open("ctrl", "bool").optional(),
        ArgGrammar::open("alt", "bool").optional(),
        ArgGrammar::open("shift", "bool").optional(),
    ])];

    /// [`FOCUS_ACTION`] — which way the focus edge went. REQUIRED: there is no sensible default for
    /// "did this pane gain or lose focus", and the parser refuses a call without it.
    pub const FOCUS: &'static [CallForm] =
        &[CallForm::object(&[ArgGrammar::open("focused", "bool")])];

    /// [`CLIPBOARD_ANSWER_ACTION`] — the answer to a pending OSC 52 read.
    ///
    /// `sel` publishes [`ClipboardTarget`](sprag_vt::ClipboardTarget)'s two words, which the host
    /// matched as bare literals before. All three arguments are required: the `seq` says WHICH query
    /// is being answered (the exactly-once arbiter's whole instrument), and an EMPTY `text` is a
    /// legitimate answer — an empty selection — so it cannot be spelled by omitting the key.
    pub const CLIPBOARD_ANSWER: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::open("seq", "int"),
        ArgGrammar::one_of("sel", "string", &sprag_vt::ClipboardTarget::WIRE_WORDS),
        ArgGrammar::open("text", "string"),
    ])];
}

/// The `cmd` key every spawning verb takes: the child's argv, or absent for the user's shell.
pub const SPAWN_CMD_KEY: &str = "cmd";
/// The `cwd` key: where the child starts.
pub const SPAWN_CWD_KEY: &str = "cwd";
/// The `cols` key: the child's initial width.
pub const SPAWN_COLS_KEY: &str = "cols";
/// The `rows` key: the child's initial height.
pub const SPAWN_ROWS_KEY: &str = "rows";
/// The `name` key: what to call the pane that is born.
pub const SPAWN_NAME_KEY: &str = "name";
/// [`SPLIT_ACTION`]'s key naming the pane to divide.
pub const SPLIT_PANE_KEY: &str = "pane";
/// [`SPLIT_ACTION`]'s key naming WHICH WAY the division runs.
pub const SPLIT_DIR_KEY: &str = "dir";
/// [`SPLIT_ACTION`]'s key putting the new pane in the FIRST half rather than the second.
pub const SPLIT_BEFORE_KEY: &str = "before";
/// [`DISPLAY_MESSAGE_ACTION`]'s key carrying what to say.
pub const MESSAGE_TEXT_KEY: &str = "text";
/// [`DISPLAY_MESSAGE_ACTION`]'s key carrying how loudly.
pub const MESSAGE_SEVERITY_KEY: &str = "severity";
/// [`DISPLAY_MESSAGE_ACTION`]'s key naming which client, or every one of them.
pub const MESSAGE_CLIENT_KEY: &str = "client";
/// [`REPORT_AGENT_ACTION`]'s key naming the pane being reported.
pub const AGENT_ID_KEY: &str = "id";
/// [`REPORT_AGENT_ACTION`]'s key naming who is reporting.
pub const AGENT_SOURCE_KEY: &str = "source";
/// [`REPORT_AGENT_ACTION`]'s key carrying the state being claimed.
pub const AGENT_STATE_KEY: &str = "state";
/// [`REPORT_AGENT_ACTION`]'s key carrying the agent's own name.
pub const AGENT_NAME_KEY: &str = "name";
/// [`REPORT_AGENT_ACTION`]'s key carrying the reporter's turn counter.
pub const AGENT_SEQ_KEY: &str = "seq";
/// [`REPORT_AGENT_ACTION`]'s key asking the daemon to bind the report to the pane's process group.
pub const AGENT_BIND_KEY: &str = "bind";

/// The key carrying WHAT A BLOCKED PEER IS ASKING — on a pane's `agent` object and on a run's
/// outcome alike.
///
/// # ⚠⚠ Why the two surfaces share one spelling and one renderer
///
/// A run's `asking` and a pane's `asking` are the same fact read off the same parse
/// ([`crate::AgentFacts::asking`]), and they are reached by callers who move between the two: an
/// agent watching a sibling pane go `blocked` and an agent whose RUN stopped on a peer are the same
/// agent asking the same question one surface apart. Two spellings, or two shapes for `choices`,
/// would make a caller written against one of them wrong against the other — the drift this tree
/// keeps paying to remove.
///
/// ⚠ The run's object carries one member this one does not: `why`, the [`sprag_plugin::Refusal`] a
/// RUN owes for not answering. A pane is not a run, has been given no consent and refuses nothing,
/// so there is no reason to invent. See [`crate::agent::question_json`] for what the shared part is.
pub const ASKING_KEY: &str = "asking";
/// The [`AGENT_STATE_KEY`] word a BLOCKED pane publishes — the state [`ASKING_KEY`] belongs to.
///
/// # ⚠⚠ Why a mouth reads this instead of the type, and instead of the literal
///
/// A mouth has to recognise `blocked` to decide whether an ABSENT [`ASKING_KEY`] is a claim (this
/// daemon looked and read no menu) or simply not that kind of pane. The agent-facing mouth carries
/// `sprag-detect` as a DEV dependency only — deliberately, so a binary that renders wire values does
/// not link the detector to read one word — so reaching the type is not available to it, and
/// spelling `"blocked"` there would be a second definition of a vocabulary this tree keeps insisting
/// has one.
///
/// DERIVED from [`sprag_detect::AgentState::wire_str`] at compile time rather than typed, which is
/// the same rule [`crate::plugins::outcome_word`] follows: the word moves when the type moves.
pub const AGENT_BLOCKED_STATE: &str = match sprag_detect::AgentState::Blocked.wire_str() {
    Some(word) => word,
    // A published state has a word by construction; only `Unknown` does not, and this is not it.
    None => panic!("a blocked agent publishes a wire word"),
};
/// The [`ASKING_KEY`] member holding the question's own lines, in reading order.
pub const ASKED_KEY: &str = "asked";
/// The [`ASKING_KEY`] member holding the options, in screen order.
pub const CHOICES_KEY: &str = "choices";
/// A [`CHOICES_KEY`] entry's key carrying the number a caller would type to pick it.
///
/// Taken from the SCREEN and never from the option's position — a list that has scrolled does not
/// start at one, which is the measurement [`sprag_detect::Choice::number`] records.
pub const CHOICE_NUMBER_KEY: &str = "number";
/// A [`CHOICES_KEY`] entry's key carrying what the option says.
pub const CHOICE_LABEL_KEY: &str = "label";
/// A [`CHOICES_KEY`] entry's key marking WHERE A BARE ENTER WOULD LAND.
///
/// ⚠⚠ The difference between confirming a tool call and declining it. Carried rather than left for
/// a caller to infer, for the reason R366 measured: the commonest consent of all is answered by the
/// marker's own position, and a reader that cannot see it must either guess or type a number.
pub const CHOICE_SELECTED_KEY: &str = "selected";

/// Every MULTIPLEXER verb that publishes its grammar — what [`MUX_SCHEMA`]'s surface serves.
///
/// ⚠ A LIST, and the one place in this feature that is. What holds it honest is not review:
/// each entry's args come from the ask type named beside it, every published word is driven
/// through the daemon's own parser, and a declared string argument the parser CONSTRAINS must
/// publish a vocabulary — so an entry that drifts, and an argument left out of one, both fail.
/// A verb this LEAVES OUT is caught too: [`SURFACES`] pairs each schema with its table, and
/// `every_verb_a_surface_declares_publishes_its_grammar` requires an omission to be a NAMED
/// exemption rather than a silence.
pub const MUX_GRAMMAR: &[ActionGrammar] = &[
    ActionGrammar {
        action: SELECT_PANE_ACTION,
        forms: SelectAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: SWAP_PANE_ACTION,
        forms: SwapAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: RESIZE_PANE_ACTION,
        forms: ResizeAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: JOIN_PANE_ACTION,
        forms: JoinAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: SELECT_WINDOW_ACTION,
        forms: SelectWindowAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: MOVE_WINDOW_ACTION,
        forms: MoveWindowAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: RESIZE_WINDOW_ACTION,
        forms: ResizeWindowAsk::GRAMMAR,
        from_ask: true,
    },
    ActionGrammar {
        action: SPAWN_ACTION,
        forms: InlineGrammar::SPAWN,
        from_ask: false,
    },
    ActionGrammar {
        action: SPLIT_ACTION,
        forms: InlineGrammar::SPLIT,
        from_ask: false,
    },
    ActionGrammar {
        action: DISPLAY_MESSAGE_ACTION,
        forms: InlineGrammar::DISPLAY_MESSAGE,
        from_ask: false,
    },
    ActionGrammar {
        action: REPORT_AGENT_ACTION,
        forms: InlineGrammar::REPORT_AGENT,
        from_ask: false,
    },
    ActionGrammar {
        action: CLOSE_ACTION,
        forms: InlineGrammar::CLOSE,
        from_ask: false,
    },
    ActionGrammar {
        action: STOP_JOB_ACTION,
        forms: InlineGrammar::STOP_JOB,
        from_ask: false,
    },
    ActionGrammar {
        action: ZOOM_PANE_ACTION,
        forms: InlineGrammar::ZOOM_PANE,
        from_ask: false,
    },
    ActionGrammar {
        action: SET_FLOATING_ACTION,
        forms: InlineGrammar::SET_FLOATING,
        from_ask: false,
    },
    ActionGrammar {
        action: DROP_FILE_ACTION,
        forms: InlineGrammar::DROP_FILE,
        from_ask: false,
    },
    ActionGrammar {
        action: RELEASE_AGENT_ACTION,
        forms: InlineGrammar::RELEASE_AGENT,
        from_ask: false,
    },
    ActionGrammar {
        action: RENAME_PANE_ACTION,
        forms: InlineGrammar::RENAME_PANE,
        from_ask: false,
    },
    ActionGrammar {
        action: RESIZE_ACTION,
        forms: InlineGrammar::RESIZE,
        from_ask: false,
    },
    ActionGrammar {
        action: GRANT_PANE_ACTION,
        forms: InlineGrammar::GRANT_PANE,
        from_ask: false,
    },
    ActionGrammar {
        action: RENAME_SESSION_ACTION,
        forms: InlineGrammar::RENAME_SESSION,
        from_ask: false,
    },
    ActionGrammar {
        action: KILL_SESSION_ACTION,
        forms: InlineGrammar::KILL_SESSION,
        from_ask: false,
    },
    ActionGrammar {
        action: RENAME_WINDOW_ACTION,
        forms: InlineGrammar::RENAME_WINDOW,
        from_ask: false,
    },
    ActionGrammar {
        action: KILL_WINDOW_ACTION,
        forms: InlineGrammar::KILL_WINDOW,
        from_ask: false,
    },
    ActionGrammar {
        action: NEW_WINDOW_ACTION,
        forms: InlineGrammar::NEW_WINDOW,
        from_ask: false,
    },
    ActionGrammar {
        action: BREAK_PANE_ACTION,
        forms: InlineGrammar::BREAK_PANE,
        from_ask: false,
    },
    ActionGrammar {
        action: MOVE_PANE_ACTION,
        forms: InlineGrammar::MOVE_PANE,
        from_ask: false,
    },
    ActionGrammar {
        action: NEW_SESSION_ACTION,
        forms: InlineGrammar::NEW_SESSION,
        from_ask: false,
    },
];

/// Every PANE-INPUT verb, all six of them — what [`PANE_SCHEMA`]'s surface serves.
///
/// # The surface an agent uses most was the one that said nothing
///
/// R352 published the multiplexer's twenty-five verbs and left these out, because
/// [`ActionGrammar`] was keyed by a mux action and had no surface dimension. So a client could
/// learn how to split a window and nothing about how to TYPE INTO ONE — and typing into a pane is
/// what a terminal is for. Every argument here was folklore until now, and four of the
/// vocabularies below had no definition at all outside the parser's own `match`.
///
/// ⚠ **Three of these verbs accept a bare SCALAR as well as an object**, which is why
/// [`FormKind`] exists: `invoke("text", "한")` is the seam an IME commit reaches, and describing
/// it as an object would have been false about half of what the daemon takes.
pub const PANE_GRAMMAR: &[ActionGrammar] = &[
    ActionGrammar {
        action: KEY_ACTION,
        forms: PaneGrammar::KEY,
        from_ask: false,
    },
    ActionGrammar {
        action: TEXT_ACTION,
        forms: PaneGrammar::TEXT,
        from_ask: false,
    },
    ActionGrammar {
        action: PASTE_ACTION,
        forms: PaneGrammar::PASTE,
        from_ask: false,
    },
    ActionGrammar {
        action: MOUSE_ACTION,
        forms: PaneGrammar::MOUSE,
        from_ask: false,
    },
    ActionGrammar {
        action: FOCUS_ACTION,
        forms: PaneGrammar::FOCUS,
        from_ask: false,
    },
    ActionGrammar {
        action: CLIPBOARD_ANSWER_ACTION,
        forms: PaneGrammar::CLIPBOARD_ANSWER,
        from_ask: false,
    },
];

/// Both PLUGIN-HOST verbs — what the surface under `sprag_plugins` serves.
///
/// ⚠ Found by [`SURFACES`]'s own derivation, not by a person: these two had published nothing
/// since they were built. [`PluginGrammar`] says what `run`'s forms are and why `guardrails`
/// is named without its inner keys.
pub const PLUGINS_GRAMMAR: &[ActionGrammar] = &[
    ActionGrammar {
        action: crate::plugins::RUN_ACTION,
        forms: PluginGrammar::RUN,
        from_ask: false,
    },
    ActionGrammar {
        action: crate::plugins::CANCEL_ACTION,
        forms: PluginGrammar::CANCEL,
        from_ask: false,
    },
];

/// HOW TO CALL THE VERBS **THIS** SURFACE SERVES — the asking surface's own [`ActionGrammar`] table,
/// as a slot.
///
/// The read half of the wire has always been discoverable: `$schema` names every address, and a
/// client walks it. The WRITE half was not — a name and nothing else — so this is the address that
/// answers *"what do I send?"* for the verbs that can say.
///
/// A SLOT rather than an addition to each verb's declaration because the declaration cannot hold it
/// at the pinned pinion ([`ArgGrammar`] says why), and a slot is the shape that survives the day it
/// can: this answer becomes a projection of the same table, and the table moves into `$schema`.
///
/// ⚠ **ONE NAME, ONE PER SURFACE.** The multiplexer answers [`MUX_GRAMMAR`] here and a pane
/// answers [`PANE_GRAMMAR`], which is why the const is a bare slot name and not a path: the
/// answer is scoped by the surface a client asked, so `/pane_7/sprag_input/external/action_grammar`
/// describes exactly the verbs `/pane_7/sprag_input/external/…` accepts. A single global table would
/// have needed a surface dimension inside every key to say the same thing, and could not have
/// described two same-named verbs on different surfaces at all.
pub const ACTION_GRAMMAR_SLOT: &str = "action_grammar";

/// EVERY SURFACE THIS CRATE SERVES, paired with the grammar table it publishes and the verbs it
/// deliberately does not describe.
///
/// # What this pairing is for
///
/// A verb can be left out of a grammar table with nothing to notice — the same shape as R352's
/// `report_agent`, one level up: a gate that walks a TABLE cannot see a verb missing from it. So the
/// table is paired with the SCHEMA here, and `every_verb_a_surface_declares_publishes_its_grammar`
/// requires each declared verb to be in the table or in the named exemption beside it. An omission
/// becomes a decision somebody wrote down.
///
/// ⚠ The GUI's three surfaces are not here: they live in another crate and declare their own schemas,
/// so this list is honest about the surfaces THIS crate's wire owns. That is the residue, stated —
/// their grammar is [`crate::wire`]'s next front, not a silence this list is pretending about.
pub const SURFACES: &[WireSurface] = &[
    WireSurface {
        name: "the multiplexer",
        author: sprag_rpc::grammar::SurfaceAuthor::Sprag,
        tag: MUX_TAG,
        grammar: MUX_GRAMMAR,
        // ⚠ THE THREE VERBS THAT TAKE NESTED VALUES, and the reason they say nothing is that
        // `ArgGrammar` describes a FLAT key: `set_layout` takes an arrangement tree, `resize` a
        // client's cell metrics, `grant_pane` a share object. `{"tree": "object"}` would be true and
        // useless — the affirmative-noise cousin of the affirmative false statement this whole
        // surface exists to avoid. pinion's `SchemaArg` cannot express a nested grammar either, so
        // this is a shape neither side has met and one to design rather than bolt on.
        // ⚠ ONE LEFT, and it is the only one whose reason survived being re-derived: `set_layout`
        // takes an arrangement TREE — recursive without a bound, so a declaration would have to
        // describe a grammar rather than a key list. `resize` and `grant_pane` were exempted beside
        // it as nested values and are FLAT (see `InlineGrammar::RESIZE`); they publish now.
        undescribed: &[SET_LAYOUT_ACTION],
    },
    WireSurface {
        name: "a pane's input",
        author: sprag_rpc::grammar::SurfaceAuthor::Sprag,
        tag: INPUT_TAG,
        grammar: PANE_GRAMMAR,
        // Nothing. Every verb this surface serves publishes how to call it.
        undescribed: &[],
    },
    WireSurface {
        name: "the plugin host",
        author: sprag_rpc::grammar::SurfaceAuthor::Sprag,
        tag: crate::PLUGINS_TAG,
        grammar: PLUGINS_GRAMMAR,
        // Nothing — and this surface is the reason the list above is checked against the SERVED scene
        // rather than trusted: it served two verbs and published nothing about either, and no person
        // noticed in the rounds since they were built.
        undescribed: &[],
    },
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
    SchemaField::action(SPAWN_ACTION, "action"),
    SchemaField::action(SPLIT_ACTION, "action"),
    SchemaField::action(CLOSE_ACTION, "action"),
    SchemaField::action(RESIZE_ACTION, "action"),
    SchemaField::action(RENAME_PANE_ACTION, "action"),
    SchemaField::action(STOP_JOB_ACTION, "action"),
    SchemaField::action(GRANT_PANE_ACTION, "action"),
    SchemaField::action(SET_LAYOUT_ACTION, "action"),
    SchemaField::action(SET_FLOATING_ACTION, "action"),
    SchemaField::action(NEW_SESSION_ACTION, "action"),
    SchemaField::action(KILL_SESSION_ACTION, "action"),
    SchemaField::action(NEW_WINDOW_ACTION, "action"),
    SchemaField::action(SELECT_WINDOW_ACTION, "action"),
    SchemaField::action(MOVE_WINDOW_ACTION, "action"),
    SchemaField::action(SELECT_PANE_ACTION, "action"),
    SchemaField::action(RENAME_WINDOW_ACTION, "action"),
    SchemaField::action(RENAME_SESSION_ACTION, "action"),
    SchemaField::action(DISPLAY_MESSAGE_ACTION, "action"),
    SchemaField::action(KILL_WINDOW_ACTION, "action"),
    SchemaField::action(RESIZE_WINDOW_ACTION, "action"),
    SchemaField::action(BREAK_PANE_ACTION, "action"),
    SchemaField::action(JOIN_PANE_ACTION, "action"),
    SchemaField::action(MOVE_PANE_ACTION, "action"),
    SchemaField::action(SWAP_PANE_ACTION, "action"),
    SchemaField::action(RESIZE_PANE_ACTION, "action"),
    SchemaField::action(ZOOM_PANE_ACTION, "action"),
    SchemaField::action(DROP_FILE_ACTION, "action"),
    // ⚠ THESE TWO WERE DISPATCHED AND DECLARED NOWHERE, from the round that built them until
    // R352. The surface answered them and `$schema` never mentioned them, so the agent
    // self-report the SCE requirement asked for was a verb no agent could discover — and the wire
    // ratchet could not see the gap, because an omission declares nothing to audit. What makes it
    // unrepeatable is not this line: it is `WorkspaceExternal::dispatch` refusing any path this
    // list does not carry.
    SchemaField::action(REPORT_AGENT_ACTION, "action"),
    SchemaField::action(RELEASE_AGENT_ACTION, "action"),
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
    SchemaField::new(ACTION_GRAMMAR_SLOT, "object"),
    // ⚠⚠⚠ R372: `PROJECT_FIELD` NOW HAS ITS EMPTY MEMBER, AND THE NOTE THAT USED TO STAND HERE
    // WAS READING A DEFECT AS A DESIGN. It said this was *"the one parametric family of the eleven
    // whose surface answers `None` for an empty argument, so declaring one would publish an address
    // this daemon does not serve"* — true as a symptom, wrong as a decision. `project.` was the
    // CATCH-ALL arm of its surface's reading match, and its two `?` did different jobs onto one
    // `None`: *not my prefix* (correct, the fallthrough) and *`project.zzz` is malformed* (a lie —
    // `PROJECT_FIELD` is right here in the schema). It was not the family that opted out; it was
    // the one nobody came back to after R155 corrected `cells.`.
    // ⚠ That note also stated the condition that retires it — *"the day it starts answering, it
    // must declare"* — and this is that day: the member now answers `QueryTypeMismatch`, the same
    // real answer the other ten give.
    PROJECT_FIELD,
    empty_member_of(&PROJECT_FIELD),
    NEIGHBORS_FIELD,
    empty_member_of(&NEIGHBORS_FIELD),
    EVENTS_FIELD,
    empty_member_of(&EVENTS_FIELD),
    SESSION_ACTIVITY_FIELD,
    empty_member_of(&SESSION_ACTIVITY_FIELD),
    PANE_PROCESSES_FIELD,
    empty_member_of(&PANE_PROCESSES_FIELD),
    PANE_RESOURCES_FIELD,
    empty_member_of(&PANE_RESOURCES_FIELD),
    DOCTOR_FIELD,
    empty_member_of(&DOCTOR_FIELD),
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

/// The mux control external query slot: WHAT EACH PANE IS TAKING of the machine — the cores it
/// holds, how much of the recent past it spent waiting for cores it did not get, its memory, and how
/// many processes it has.
///
/// # Why the settings could not answer this and a reading has to
///
/// R336 gave every pane a weight and R337 gave it ceilings, and both are things a PERSON said. A
/// weight is not a cap (a pane weighted 10 beside an idle neighbour took all 8 cores it was offered)
/// and it is not even a ratio (a nominal 10:100 measured 18:82, because the kernel distributes
/// weight per runqueue). So a client that rendered the setting would be rendering a prediction that
/// is measurably wrong, and the only honest source is what the kernel CHARGED.
///
/// A SAMPLED fact and therefore its own address, exactly like [`PANE_PROCESSES_FIELD`] and by that
/// field's rule: a pane deciding to spend a core is not an event this daemon performs. Its answer
/// ([`PaneResourcesWire`]) carries the age it has, and each row's rate carries the WINDOW it covers
/// — a rate over 40 ms and one over a minute are different claims, and a reader that cannot tell
/// them apart reads a build's opening burst as its steady state.
///
/// REGISTRY-WIDE, not scoped, for [`PANE_PROCESSES_FIELD`]'s reason turned around: the question a
/// person asks here is *which pane is eating my machine*, and a machine is not divided by session.
/// An answer scoped to one session could not name the pane that was taking everything.
///
/// A QUERY, not an invoke: observing the world is not a mutation of the scene, and serving it as an
/// action would bump the revision and wake the very `waitFor` it was answering.
pub const PANE_RESOURCES_FIELD: SchemaField =
    SchemaField::parametric("pane_resources.<max_age_ms>", "object", PANE_RESOURCES_ARGS);

/// [`PANE_RESOURCES_FIELD`]'s argument: how stale an answer this caller will accept, in
/// milliseconds. Open, because nothing on this surface bounds it.
const PANE_RESOURCES_ARGS: &[SchemaArg] = &[SchemaArg::open("max_age_ms", "int")];

/// [`PANE_RESOURCES_FIELD`]'s address with the tolerance filled in — `pane_resources_at(0)` is the
/// always-fresh read.
///
/// Built from the declared field rather than by re-spelling the prefix, so the address a client
/// sends and the prefix the host strips cannot drift ([`pane_processes_at`]'s discipline).
#[must_use]
pub fn pane_resources_at(max_age_ms: u64) -> String {
    format!("{}{max_age_ms}", PANE_RESOURCES_FIELD.literal_prefix())
}

/// What [`PANE_RESOURCES_FIELD`] answers: every pane's
/// [resources](sprag_terminal::PaneResources), and how long ago the reading they all came from was
/// taken.
///
/// The age is in the ENVELOPE for [`PaneProcessesWire`]'s reason — one pass produces them all. The
/// per-row `over_ms` is a DIFFERENT quantity and both are wanted: the envelope says how long ago the
/// reading was taken, and the row says how long a window its rate covers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PaneResourcesWire {
    /// How long ago the [`panes`](Self::panes) below were sampled, in milliseconds. `0` for a sample
    /// taken to answer this very request.
    pub sampled_ms_ago: u64,
    /// One row per pane in the registry, in the registry's own order — join on
    /// [`id`](sprag_terminal::PaneResources::id), never on position.
    pub panes: Vec<sprag_terminal::PaneResources>,
}

impl From<sprag_terminal::PaneResourceReading> for PaneResourcesWire {
    /// The one conversion from the in-process reading to the wire's shape. Saturating for
    /// [`ActivityWire`]'s reason.
    fn from(reading: sprag_terminal::PaneResourceReading) -> Self {
        Self {
            sampled_ms_ago: u64::try_from(reading.age.as_millis()).unwrap_or(u64::MAX),
            panes: reading.value,
        }
    }
}

impl PaneResourcesWire {
    /// Whether any row is still [settling](sprag_terminal::Cpu::Settling) — one sample so far, so no
    /// rate yet.
    ///
    /// The discriminator behind [`settled`], and a method rather than a client-side loop because
    /// BOTH one-shot clients need it and two copies of "is this answer usable yet" is how two
    /// surfaces come to disagree about what a settling row means.
    #[must_use]
    pub fn settling(&self) -> bool {
        self.panes.iter().any(|pane| {
            matches!(
                pane.taken,
                sprag_terminal::Taken::Measured {
                    cpu: sprag_terminal::Cpu::Settling,
                    ..
                }
            )
        })
    }
}

/// Read until the answer has a rate in it — ONE definition, for every client that asks once and
/// leaves.
///
/// # Why a one-shot caller cannot just read
///
/// A rate needs two samples. A daemon nobody has asked yet has one sample of each pane the moment it
/// is asked, so the first answer to `sprag resources` on a quiet daemon is all
/// [`Settling`](sprag_terminal::Cpu::Settling) — which is honest and is not a number. A client that
/// polls (a display) simply gets rates from its second wake onwards and needs none of this; a client
/// that asks once and prints has to wait out one window, and
/// [`SETTLE`](sprag_terminal::SETTLE) is that window by definition.
///
/// It is HERE rather than in either client because the `sprag` CLI and `sprag-mcp` both ask once —
/// two copies would be two answers to "how long is long enough", which is the drift this crate's
/// whole vocabulary half exists to prevent.
///
/// # Errors
///
/// Whatever `read` returns. It is attempted at most twice, so a caller sees at most one extra
/// round trip and never a loop.
/// The mux control external query slot: WHAT IS WRONG with the machine the panes run on.
///
/// # Why the terminal is the one that answers a question that is mostly not about it
///
/// [`PANE_RESOURCES_FIELD`] says what each pane TOOK, and a person reading it who sees every pane
/// starved has learned that the machine is short and nothing about why. The investigation behind
/// this design found seven causes and **one** of them belonged to the multiplexer: the rest were a
/// compiler cache the shells walked past, kernel swap tuning, a systemd delegation policy and a CI
/// runner competing at equal weight. So a diagnosis scoped to sprag's own state would answer a
/// seventh of the question.
///
/// The daemon answers it — rather than each client reading `/proc` for itself — because the daemon
/// is where the panes are. Half the checks need a pane's own pid, its own cgroup and the `PATH` its
/// child was executed with, and a client attached over ssh is not even on the same machine as the
/// thing it is asking about. The rule this crate keeps everywhere applies unchanged: the process
/// that PERFORMS a thing is the one that says what it did.
///
/// # Why its argument is a WINDOW where its three neighbours take a tolerance — and why it must
/// have one at all
///
/// It must be PARAMETRIC, and that is structural rather than stylistic. `scene/snapshot` with an
/// empty path walks the whole surface and reads every bare slot; a parametric address has an
/// argument a walk cannot fill in, so it is not walked. This read costs a window of real time and
/// runs a program, and as a bare slot it went straight onto the snapshot path — measured, by a
/// plugin test from four rounds ago that asserts a snapshot returns in under half a second and
/// began failing at 518 ms the moment this address existed. R282's rule (keep the costly reads OFF
/// the polled slot) is kept by the address SHAPE. Its three sampled siblings are parametric for the
/// same reason, which is the pattern this arrived at the hard way.
///
/// The argument is a window and not a staleness tolerance because a diagnosis is never cached: it
/// is pressed by a person who has just changed something and wants to know whether it helped, and
/// an answer from before their change is the one answer that is certainly useless. What the caller
/// does get to choose is how long the one un-cacheable measurement takes — a cumulative counter
/// says a neighbour used CPU at some point since boot, and only a window says it is using it now. A
/// caller in a hurry passes `0` and every rate comes back settling; one that wants the competition
/// separated passes longer. Every rate states the window it covers, so neither is a number a reader
/// has to know in advance.
///
/// A QUERY and not an invoke, for [`PANE_RESOURCES_FIELD`]'s reason: reading the world is not a
/// mutation of the scene, and serving it as an action would bump the revision and wake the very
/// `waitFor` it was answering.
pub const DOCTOR_FIELD: SchemaField =
    SchemaField::parametric("doctor.<window_ms>", "object", DOCTOR_ARGS);

/// [`DOCTOR_FIELD`]'s argument: how long to measure the competition over, in milliseconds. Open,
/// because nothing on this surface bounds it.
const DOCTOR_ARGS: &[SchemaArg] = &[SchemaArg::open("window_ms", "int")];

/// [`DOCTOR_FIELD`]'s address with the window filled in.
///
/// Built from the declared field rather than by re-spelling the prefix, so the address a client
/// sends and the prefix the host strips cannot drift ([`pane_processes_at`]'s discipline).
#[must_use]
pub fn doctor_over(window_ms: u64) -> String {
    format!("{}{window_ms}", DOCTOR_FIELD.literal_prefix())
}

/// The window [`DOCTOR_FIELD`] is asked for by every client that has no reason to want another.
///
/// The same window a pane's rate settles over, and one constant rather than two because they are
/// the same window: long enough that scheduler granularity is noise, short enough that a person
/// waiting on the command does not think it hung.
pub const DOCTOR_WINDOW: Duration = sprag_terminal::SETTLE;

pub fn settled<E>(
    mut read: impl FnMut() -> Result<PaneResourcesWire, E>,
) -> Result<PaneResourcesWire, E> {
    let first = read()?;
    if !first.settling() {
        return Ok(first);
    }
    std::thread::sleep(sprag_terminal::SETTLE);
    read()
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    fn wire(cpu: sprag_terminal::Cpu) -> PaneResourcesWire {
        PaneResourcesWire {
            sampled_ms_ago: 0,
            panes: vec![sprag_terminal::PaneResources {
                id: 1,
                taken: sprag_terminal::Taken::Measured {
                    cpu,
                    waiting: sprag_terminal::Waiting::NotAccounted,
                    memory: sprag_terminal::Counted::NoController,
                    processes: sprag_terminal::Counted::NoController,
                    granted: sprag_terminal::Granted {
                        share: sprag_terminal::Counted::NoController,
                        memory: sprag_terminal::Ceiling::NoController,
                        processes: sprag_terminal::Ceiling::NoController,
                    },
                },
            }],
        }
    }

    const HELD: sprag_terminal::Cpu = sprag_terminal::Cpu::Held {
        millicores: 1000,
        over_ms: 500,
    };

    /// A one-shot caller reads TWICE when the first answer has no rate in it, and ONCE when it does.
    ///
    /// Both halves, because they fail differently: never re-reading hands `sprag resources` an
    /// all-`Settling` answer on any daemon nobody has polled, and always re-reading makes every call
    /// wait half a second for a rate it already had.
    #[test]
    fn a_one_shot_caller_reads_again_only_when_the_first_answer_has_no_rate() {
        let mut reads = 0;
        let answer = settled(|| {
            reads += 1;
            Ok::<_, ()>(wire(if reads == 1 {
                sprag_terminal::Cpu::Settling
            } else {
                HELD
            }))
        })
        .expect("the second read answers");
        assert_eq!(reads, 2, "a settling answer is read again");
        assert!(!answer.settling(), "and the second one is what comes back");

        let mut reads = 0;
        settled(|| {
            reads += 1;
            Ok::<_, ()>(wire(HELD))
        })
        .expect("one read is enough");
        assert_eq!(reads, 1, "an answer that already has a rate is not re-read");
    }

    /// An UNMEASURED pane is not a settling one, so a host that measures nothing does not make every
    /// caller wait half a second to be told so twice.
    #[test]
    fn a_pane_with_no_reading_at_all_does_not_look_like_one_that_is_still_settling() {
        let unmeasured = PaneResourcesWire {
            sampled_ms_ago: 0,
            panes: vec![sprag_terminal::PaneResources {
                id: 1,
                taken: sprag_terminal::Taken::Unmeasured {
                    reason: sprag_terminal::Unmeasured::NothingEnforced,
                },
            }],
        };
        assert!(!unmeasured.settling());
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

/// The mux control external invoke action that STOPS WHAT A PANE IS RUNNING without ending the pane
/// (`{pane, signal?}`), answering `{stop, pgid, job?}`.
///
/// # ⚠⚠⚠ Why this is not [`KEY_ACTION`] with a `C-c`
///
/// **Writing `0x03` into a pane is not a stop.** The byte becomes a `SIGINT` only if the terminal's
/// line discipline is willing — a program that took the terminal raw has turned that off — and it
/// reaches whichever process group owns the terminal at the instant the kernel processes it. Neither
/// condition is one the caller can see, and a write reports success either way, so a caller could
/// not even find out. Measured: a pane running `stty -isig; sleep 300` echoes `^C` and keeps
/// sleeping. See [`sprag_terminal::stop`](../../sprag_terminal/stop/index.html).
///
/// This delivers the signal itself, to the group the terminal actually has in the foreground, and
/// says what received it — the whole difference being that a daemon that owns the pty can act and
/// report where a byte can only be sent and hoped for.
///
/// # ⚠ Why it is not [`CLOSE_ACTION`] either
///
/// That ends the PANE — its shell, its scrollback, its place in the layout. This ends the pane's
/// current JOB and leaves the pane. For an AI control loop those are not degrees of the same act:
/// the loop wants the peer's turn over and the peer still there for the next turn.
///
/// ⚠ **NO [`WIRE_PROTOCOL`] BUMP, and the reason is the constant's own
/// rule rather than this note.** A bump is earned when an old peer would MISREAD or SILENTLY DROP.
/// Neither direction does here: an old client never names this action, and a new client naming it
/// at an old daemon is refused by name at [`declares_verb`] before any handler runs — the loud
/// failure version 15's whole-new-capability bump existed because it did NOT have (a message to a
/// daemon with no mailbox reached nobody, with no key whose absence could say so).
pub const STOP_JOB_ACTION: &str = "stop_job";
/// The answer key of [`STOP_JOB_ACTION`] naming WHAT the pane's job answers to, absent when the
/// group's leader has already gone and its other members keep the group alive.
///
/// The name the CALLER would use — `JobLeader`'s own choice of `argv[0]` over the kernel's spelling
/// — so a stop and a readiness refusal name one program the same way. The kernel's name, when the
/// two disagree, is on `pane_processes`; publishing both here would invite a reader to take the
/// same fact two ways.
pub const STOP_JOB_LEADER_KEY: &str = "job";
/// The [`STOP_JOB_ACTION`] argument naming WHICH stop to deliver — a
/// [`Stop`](sprag_terminal::Stop) word. Absent asks for the one a `Ctrl-C` means.
pub const STOP_JOB_SIGNAL_KEY: &str = "signal";
/// The [`STOP_JOB_ACTION`] answer key echoing WHICH stop was delivered — the same vocabulary the
/// argument takes, so a caller that omitted it learns what it got rather than having to know the
/// default.
pub const STOP_JOB_STOP_KEY: &str = "stop";
/// The [`STOP_JOB_ACTION`] answer key naming the process GROUP that received the signal.
///
/// The ADDRESS and not only the name: it is what a person types into `kill` to check, and a report
/// naming only a program leaves them nothing to verify with.
pub const STOP_JOB_PGID_KEY: &str = "pgid";

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

/// The mux control external action: give ONE pane the CPU weight and the ceilings a person just
/// asked for, and answer with what the kernel holds afterwards.
///
/// `{pane, share?, memory?, processes?}` in, [`sprag_terminal::Granted`] out. Every setting is
/// optional and an omitted one is left alone, so raising a ceiling is not a way to silently reset a
/// weight somebody set an hour ago; `0` on either ceiling is the user's spelling of "no ceiling",
/// which is what `pane-memory-limit` and `pane-process-limit` already mean in the config file.
///
/// # Why this is an ACTION and not a write to a slot
///
/// [`RENAME_PANE_ACTION`]'s reason, and one more. The first half is the same: the request is
/// REFUSABLE — there may be no such pane, and a weight outside the kernel's `1..=10000` is not a
/// grant — and a plain write would have nowhere to say so.
///
/// The second half is what makes this different from every other setter here: **the answer is not
/// the request.** A ceiling on a host whose `memory` controller was never delegated goes nowhere,
/// and a daemon that echoed the argument back would agree with itself about a setting that is not
/// in force. So the action re-reads the pane's leaf and answers with THAT — the same discipline
/// `rename_pane` follows for a trimmed name, applied where the disagreement is a whole missing
/// controller rather than two spaces.
pub const GRANT_PANE_ACTION: &str = "grant_pane";
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

    // ⚠ NO `GRAMMAR`, AND THE ABSENCE IS THE DECISION. These two keys are the BIRTH's, not the
    // verb's: `new_window` also takes the pane spec every spawning verb takes, and that half has no
    // ask type to be read off. Publishing these two under `ArgForm::Object` would claim they are
    // the arguments, which is the affirmative false statement pinion's `ArgForm::Undeclared` exists
    // to let a surface avoid — so this verb says nothing until its other half can be said too.

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
    /// `{window}` XOR `{window_id}` — make THAT window current.
    ///
    /// A [`WindowRef`] rather than a bare name since R330: a client that read a row off a list it
    /// painted holds an IDENTITY, and sending the label on that row lands the select on whatever
    /// carries it now. The keyboard cannot reach the identity arm at all — see
    /// [`crate::keymap::SelectWindowBind`], the vocabulary this one is deliberately not.
    At(WindowRef),
    /// `{relative}` — one step along the ring from the current window, WRAPPING. Total: a session
    /// always has a window, so this always lands somewhere and answers its name.
    Step(OrderStep),
}

impl SelectWindowAsk {
    /// The request key naming which way to step along the ring.
    pub const RELATIVE_KEY: &'static str = "relative";

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — what [`SELECT_WINDOW_ACTION`]'s
    /// declaration publishes; see [`ResizeAsk::GRAMMAR`] for why it lives beside the keys.
    ///
    /// `relative` publishes the whole of [`OrderStep`], projected from
    /// the type, so the two words a client may step by are discoverable rather than folklore.
    pub const GRAMMAR: &'static [CallForm] = &[
        // `At`, by NAME — whatever window carries it when the request arrives.
        CallForm::object(&[WindowRef::NAMED_ARG.required()]),
        // `At`, by IDENTITY — that window, or none.
        CallForm::object(&[WindowRef::PICKED_ARG.required()]),
        // `Step` — one place along the ring, which has no ends to a walk.
        CallForm::object(&[ArgGrammar::one_of(
            Self::RELATIVE_KEY,
            "string",
            &sprag_terminal::OrderStep::WIRE_WORDS,
        )]),
    ];

    /// The `args` object a client sends for this ask.
    ///
    /// The named arm emits exactly the bytes it emitted before the step existed, so the request
    /// every client already sends is unchanged and a reader of a trace tells the two apart by eye
    /// ([`SelectAsk::to_args`]'s rule).
    #[must_use]
    pub fn to_args(&self) -> Value {
        let mut map = Map::new();
        match self {
            Self::At(window) => window.write(&mut map),
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
        let at = WindowRef::read(map?).ok()?;
        let step = match field(Self::RELATIVE_KEY) {
            None => None,
            Some(value) => Some(OrderStep::from_wire(value.as_str()?)?),
        };
        match (at, step) {
            (Some(window), None) => Some(Self::At(window)),
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
    /// The request key naming the window being placed — [`WindowRef`]'s, because there is one
    /// spelling of *which window* in this product and a second copy is what R330 hoisted that type
    /// to prevent. The ADDRESS here is still a name only: nothing paints a per-window move row, so
    /// no caller holds an identity to send (register item 55a).
    pub const WINDOW_KEY: &'static str = WindowRef::WINDOW_KEY;
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

    /// Every word [`PLACE_KEY`](Self::PLACE_KEY) admits: the two ENDS this type names, then the
    /// whole of [`OrderStep`].
    ///
    /// ⚠ A UNION IS THE SHAPE A NEW MEMBER IS LEFT OUT OF, so it is assembled rather than typed:
    /// the length is `2 + OrderStep::WIRE_WORDS.len()` and the tail is COPIED from that array, so
    /// a third step arm widens this in the same compile that adds it. What is left hand-written is
    /// the two ends — and they are hand-written in [`parse`](Self::parse) too, as the same two
    /// constants, which is what
    /// [`every_published_word_is_a_word_the_daemon_accepts`](self) holds them to.
    pub const PLACE_WORDS: [&'static str; 2 + sprag_terminal::OrderStep::WIRE_WORDS.len()] = {
        const STEPS: [&str; sprag_terminal::OrderStep::WIRE_WORDS.len()] =
            sprag_terminal::OrderStep::WIRE_WORDS;
        let steps = STEPS;
        let mut words = [""; 2 + STEPS.len()];
        words[0] = Self::FIRST_WORD;
        words[1] = Self::LAST_WORD;
        let mut at = 0;
        while at < steps.len() {
            words[2 + at] = steps[at];
            at += 1;
        }
        words
    };

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — what [`MOVE_WINDOW_ACTION`]'s
    /// declaration publishes; see [`ResizeAsk::GRAMMAR`] for why it lives beside the keys.
    ///
    /// Three of the four are alternatives ([`SwapAsk::GRAMMAR`]'s note), and only `place`
    /// draws from a vocabulary: an anchor is another window's NAME, which is the person's string.
    pub const GRAMMAR: &'static [CallForm] = &[
        // `First` / `Last` / `Step` — an end of the order, or one place along it.
        CallForm::object(&[
            WindowRef::NAMED_ARG,
            ArgGrammar::one_of(Self::PLACE_KEY, "string", &Self::PLACE_WORDS),
        ]),
        // `Before` — immediately ahead of the window this names.
        CallForm::object(&[
            WindowRef::NAMED_ARG,
            ArgGrammar::open(Self::BEFORE_KEY, "string"),
        ]),
        // `After` — immediately behind it.
        CallForm::object(&[
            WindowRef::NAMED_ARG,
            ArgGrammar::open(Self::AFTER_KEY, "string"),
        ]),
    ];

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

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — what [`SELECT_PANE_ACTION`]'s
    /// declaration publishes; see [`ResizeAsk::GRAMMAR`] for why it lives beside the keys, and
    /// [`SwapAsk::GRAMMAR`] for why an alternation publishes as three optional arguments.
    pub const GRAMMAR: &'static [CallForm] = &[
        // `Pane` — select the one named.
        CallForm::object(&[ArgGrammar::open(Self::PANE_KEY, "int")]),
        // `Toward` — step that way, from the active pane unless an origin says otherwise.
        CallForm::object(&[
            ArgGrammar::one_of(
                Self::DIR_KEY,
                "string",
                &sprag_terminal::PaneDir::WIRE_WORDS,
            ),
            ArgGrammar::open(Self::FROM_KEY, "int").optional(),
        ]),
    ];

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
/// of the SCOPED session, and returns its name (`{pane, name?, detached?, opened_by?}`) — tmux
/// `break-pane`.
///
/// `pane` is the id of the pane to move; its source window is DERIVED (a [`PaneId`]
/// is registry-unique, so the window that holds it is unambiguous — the caller never names the
/// source). `name` absent ⇒ the lowest free integer window name. Refused (`Rejected`) if the pane's
/// window tiles only that one pane, if an explicit `name` is taken, or if no window holds `pane`.
///
/// # HOW THE NEW WINDOW IS BORN — [`WindowBirthAsk`]'s two keys, since R335
///
/// [`DETACHED_KEY`] and [`WINDOW_OPENED_BY_KEY`] mean here exactly what they mean on
/// [`NEW_WINDOW_ACTION`], are parsed by the same function, and default to what this action did
/// before they existed: take the screen, claim nobody. A break MAKES A WINDOW, so the type that
/// says how a window is born is the one that belongs here — that it was spelled on only one of the
/// two actions that create windows was an omission, and the agent surface is where it bit.
///
/// **Measured**: an AI agent tidying a pane it had opened out of somebody's window took the
/// person's whole screen doing it, and could not afterwards `close_window` what it had made,
/// because that gate reads a [`sprag_terminal::Window::opened_by`] a break never wrote. Both are
/// the exact intrusions [`DETACHED_KEY`] and R294's authorship rule exist to prevent, arriving
/// through the one window-creating door neither had been spelled on.
///
/// **This is why [`WIRE_PROTOCOL`] moved to 17.** [`DETACHED_KEY`]'s own hazard, verbatim: a daemon
/// older than a client that sends it ACCEPTS the request and DROPS the key, so a caller that
/// believed it had tidied up quietly has moved every attached client, with nothing in the answer to
/// say so. An added ARGUMENT is invisible to `client/hello`; only the version is not.
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

    /// [`WINDOW_KEY`](Self::WINDOW_KEY) as a SCHEMA argument, for the verbs that address a window.
    ///
    /// Declared once here for the reason the keys are: four verbs take this reference, and four
    /// copies of the same two declarations is the drift this type was hoisted to end. Optional
    /// because [`read`](Self::read) answers [`None`] for neither key — a request that names no
    /// window means the one it is SCOPED to, which is a well-formed call.
    ///
    /// The domain is [`Open`](pinion_core::external::ArgDomain::Open) and that is honest rather
    /// than lazy: a window NAME is a string the person chose, so there is no vocabulary to
    /// enumerate — the answerable values live on the `windows` slot, which is a different surface
    /// from this one and a bound this declaration cannot state.
    pub const NAMED_ARG: ArgGrammar = ArgGrammar::open(Self::WINDOW_KEY, "string").optional();
    /// [`WINDOW_ID_KEY`](Self::WINDOW_ID_KEY) as a SCHEMA argument, [`NAMED_ARG`](Self::NAMED_ARG)'s
    /// peer — the identity spelling of the same reference.
    pub const PICKED_ARG: ArgGrammar = ArgGrammar::open(Self::WINDOW_ID_KEY, "int").optional();

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

    /// [`read`](Self::read) at a door that has only the NAME form: the name, [`None`] for the
    /// request's own scope, and an ERROR for an identity.
    ///
    /// # Why an identity is a malformation here and not an ignorable extra
    ///
    /// `rename_window` and `resize_window` reach the registry by NAME — there is no
    /// identity-addressed entry for either, and nothing in this product paints a row that commits
    /// one for them (register item 55a). A well-formed `window_id` dropped on the floor would leave
    /// the request acting on the SCOPED window instead, which is the silent wrong act
    /// [`WIRE_PROTOCOL`] 16 moved for. Refusing says so.
    ///
    /// It exists as its own function because it was written out twice — once inside the daemon's
    /// `window_target` and once beside it — and the second copy is where a grammar starts to drift.
    /// The day one of those verbs gains an identity door, this call site is the one to change, and
    /// its callers are the list of who is waiting.
    ///
    /// # Errors
    ///
    /// [`MalformedWindowRef`] for a malformed reference, a doubly-spelled one, or an IDENTITY.
    pub fn read_named(map: &Map<String, Value>) -> Result<Option<&str>, MalformedWindowRef> {
        match Self::read(map)? {
            None => Ok(None),
            // Borrowed back out of the map rather than returned from the owned `read` above: every
            // caller wants a `&str` and the owned copy would be allocated only to be dropped. The
            // key is `read`'s own, so the two cannot come apart.
            Some(Self::Named(_)) => match map.get(Self::WINDOW_KEY) {
                Some(Value::String(name)) => Ok(Some(name)),
                _ => Err(MalformedWindowRef),
            },
            Some(Self::Picked(_)) => Err(MalformedWindowRef),
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

/// The REQUEST grammar of [`RESIZE_WINDOW_ACTION`] — which window, and which of the four ways of
/// naming a rectangle.
///
/// [`JoinAsk`]'s shape one verb over and for its reason: the daemon [`parse`](Self::parse)s one of
/// these and every caller [`to_args`](Self::to_args) builds one, so the six keys are spelled ONCE
/// for the daemon, the CLI verb, the [`crate::HostClient`] call and the keybinding.
///
/// # This is the hole the shape pin names in its own doc
///
/// `the_wire_shape_is_what_this_protocol_number_stands_for` pins *"the grammars that are a TYPE"*
/// and says a key spelled at a `json!` call site is invisible to it. `resize-window` was that call
/// site: the CLI built `args["cols"]`, `args["adjust_cols"]` and `args["from"]` by hand while the
/// daemon read them through its own private function, so the two halves of one grammar could drift
/// with nothing failing. R330 found three of exactly this class the day it hoisted [`WindowRef`];
/// this is the fourth, and it is now pinned by bytes like the rest.
///
/// # The window is a NAME or the scope, never an identity
///
/// [`WindowRef::read_named`]'s rule, and this type carries `Option<String>` rather than a
/// [`WindowRef`] so that the refusal is not something a caller can construct and be surprised by:
/// an identity is unrepresentable in this ask, which is the same trick
/// [`crate::keymap::SelectWindowBind`] plays one surface up. The wire KEYS are still
/// [`WindowRef`]'s, so a client that sends `window_id` meets one stated refusal rather than a
/// second grammar.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ResizeWindowAsk {
    /// Which window of the request's session — [`None`] for the one it is scoped to.
    pub window: Option<String>,
    /// Which rectangle, in the four spellings [`crate::window::SizeRequest`] admits.
    pub size: crate::window::SizeRequest,
}

impl ResizeWindowAsk {
    /// The request key naming an exact width (tmux `-x`).
    pub const COLS_KEY: &'static str = "cols";
    /// The request key naming an exact height (tmux `-y`).
    pub const ROWS_KEY: &'static str = "rows";
    /// The request key moving the vertical edges by a SIGNED amount (tmux `-L` / `-R`).
    pub const ADJUST_COLS_KEY: &'static str = "adjust_cols";
    /// The request key moving the horizontal edges by a SIGNED amount (tmux `-U` / `-D`).
    pub const ADJUST_ROWS_KEY: &'static str = "adjust_rows";
    /// The request key naming a `window-size` policy to fold the attached clients under (tmux
    /// `-a` / `-A`).
    pub const FROM_KEY: &'static str = "from";

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — what [`RESIZE_WINDOW_ACTION`]'s
    /// declaration publishes; see [`ResizeAsk::GRAMMAR`] for why it lives beside the keys.
    ///
    /// Every argument is omittable and that is load-bearing rather than lax: the request naming NO
    /// rectangle is the UN-PIN, so the empty object is a well-formed call with a meaning of its own.
    ///
    /// `from` publishes the whole of [`WindowSize`](crate::WindowSize) — the same four names the
    /// user's `window-size` option takes, which is the point: one vocabulary, whether a person
    /// writes it in a file or a client sends it here.
    pub const GRAMMAR: &'static [CallForm] = &[
        // `Exact` — pin this rectangle. Both edges, because half a rectangle is refused.
        CallForm::object(&[
            WindowRef::NAMED_ARG,
            ArgGrammar::open(Self::COLS_KEY, "int"),
            ArgGrammar::open(Self::ROWS_KEY, "int"),
        ]),
        // `Adjust` — move the edges that are named, leave the rest. ⚠ And with NEITHER named this
        // is also `Clear`, the un-pin: the request naming no rectangle at all is the one that
        // throws the pin away, which is why the un-pin is not a fourth form with a key of its own.
        CallForm::object(&[
            WindowRef::NAMED_ARG,
            ArgGrammar::open(Self::ADJUST_COLS_KEY, "int").optional(),
            ArgGrammar::open(Self::ADJUST_ROWS_KEY, "int").optional(),
        ]),
        // `Clients` — fold the attached clients under a policy.
        CallForm::object(&[
            WindowRef::NAMED_ARG,
            ArgGrammar::one_of(
                Self::FROM_KEY,
                "string",
                &crate::WindowSize::CLIENT_FOLD_WORDS,
            ),
        ]),
    ];

    /// The `args` object a caller sends for this ask.
    ///
    /// [`SizeRequest::Clear`](crate::window::SizeRequest::Clear) renders as the EMPTY object, which
    /// is the action's own reading of a request naming no rectangle. That is what makes the un-pin
    /// spelling additive rather than a fifth key.
    #[must_use]
    pub fn to_args(&self) -> Value {
        use crate::window::SizeRequest;

        let mut map = Map::new();
        if let Some(window) = &self.window {
            WindowRef::Named(window.clone()).write(&mut map);
        }
        match self.size {
            SizeRequest::Clear => {}
            SizeRequest::Exact(size) => {
                map.insert(Self::COLS_KEY.to_owned(), Value::from(size.cols));
                map.insert(Self::ROWS_KEY.to_owned(), Value::from(size.rows));
            }
            // Each axis only when it MOVES, so `-R 4` and `-R 4 -U 0` are one request rather than
            // two spellings — and an unnamed axis stays unnamed on the wire, which is the reading
            // the action gives it ("leave that edge").
            SizeRequest::Adjust { cols, rows } => {
                if cols != 0 {
                    map.insert(Self::ADJUST_COLS_KEY.to_owned(), Value::from(cols));
                }
                if rows != 0 {
                    map.insert(Self::ADJUST_ROWS_KEY.to_owned(), Value::from(rows));
                }
                // ...and an adjustment that names NEITHER edge still says so, because the empty
                // object is the UN-PIN. No surface builds one (every flag parser refuses a zero
                // count), but a grammar in which "move nothing" and "throw the size away" render
                // identically is one a caller can be silently betrayed by, and the cost of not
                // being is one key.
                if cols == 0 && rows == 0 {
                    map.insert(Self::ADJUST_COLS_KEY.to_owned(), Value::from(0));
                }
            }
            SizeRequest::Clients(policy) => {
                map.insert(Self::FROM_KEY.to_owned(), Value::from(policy.name()));
            }
        }
        Value::Object(map)
    }

    /// The ask an `args` value names, or [`None`] for anything this grammar does not admit — which
    /// is what the action turns into its one `TypeMismatch`.
    ///
    /// Refused rather than resolved, in the order a caller can act on:
    ///
    /// * an IDENTITY for the window ([`WindowRef::read_named`]'s rule);
    /// * HALF a rectangle — a width whose height came from a different decision is a shape nobody
    ///   chose, so it is refused whole rather than completed from what happens to be pinned;
    /// * a `from` naming `manual` or a policy this build does not know — `manual` reads a STORED
    ///   size rather than folding clients, so as a SOURCE for a new stored size it names nothing;
    /// * TWO spellings at once, because they are four ways to name ONE rectangle and a request
    ///   carrying two is a caller that has not decided.
    #[must_use]
    pub fn parse(args: &Value) -> Option<Self> {
        use crate::window::SizeRequest;

        let map = match args {
            Value::Object(map) => map,
            _ => return None,
        };
        let window = WindowRef::read_named(map).ok()?.map(str::to_owned);
        let exact = match (
            dimension(map, Self::COLS_KEY)?,
            dimension(map, Self::ROWS_KEY)?,
        ) {
            (Some(cols), Some(rows)) => Some(crate::attach::ClientSize { cols, rows }),
            (None, None) => None,
            _ => return None,
        };
        let adjust = match (
            delta(map, Self::ADJUST_COLS_KEY)?,
            delta(map, Self::ADJUST_ROWS_KEY)?,
        ) {
            (None, None) => None,
            (cols, rows) => Some(SizeRequest::Adjust {
                cols: cols.unwrap_or(0),
                rows: rows.unwrap_or(0),
            }),
        };
        let from = match map.get(Self::FROM_KEY).filter(|value| !value.is_null()) {
            None => None,
            // ⚠ THE PREDICATE, not a match arm naming `Manual`. `folds_clients` is what this
            // refusal means, and it is the same const the published vocabulary
            // (`WindowSize::CLIENT_FOLD_WORDS`) is projected through — so the words a client is
            // told it may send here are exactly the words this admits, and neither can move
            // without the other.
            Some(Value::String(name)) => match crate::WindowSize::parse(name) {
                Some(policy) if policy.folds_clients() => Some(policy),
                _ => return None,
            },
            Some(_) => return None,
        };
        let size = match (exact, adjust, from) {
            (None, None, None) => SizeRequest::Clear,
            (Some(size), None, None) => SizeRequest::Exact(size),
            (None, Some(adjust), None) => adjust,
            (None, None, Some(policy)) => SizeRequest::Clients(policy),
            _ => return None,
        };
        Some(Self { window, size })
    }
}

/// A dimension key of a [`ResizeWindowAsk`]: absent, or a positive extent that fits a cell count.
///
/// `Some(None)` is absent and `None` is malformed — the two outcomes a caller must not confuse,
/// which is why this is not an `Option<u16>`. A ZERO is refused here rather than clamped: a
/// zero-column window is not a window, and a caller that typed one has made a mistake the far end
/// cannot repair.
fn dimension(map: &Map<String, Value>, key: &str) -> Option<Option<u16>> {
    match map.get(key).filter(|value| !value.is_null()) {
        None => Some(None),
        Some(value) => u16::try_from(value.as_u64()?)
            .ok()
            .filter(|extent| *extent > 0)
            .map(Some),
    }
}

/// An adjustment key of a [`ResizeWindowAsk`]: absent, or a signed cell count that fits an `i32`.
///
/// [`dimension`]'s two-level answer for its reason. A zero is ADMITTED here where a dimension
/// refuses one — "move this edge by nothing" is a legal request that resolves to the current size,
/// and refusing it would make an arithmetic caller special-case the identity.
fn delta(map: &Map<String, Value>, key: &str) -> Option<Option<i32>> {
    match map.get(key).filter(|value| !value.is_null()) {
        None => Some(None),
        Some(value) => i32::try_from(value.as_i64()?).ok().map(Some),
    }
}

/// The ANSWER of [`RESIZE_WINDOW_ACTION`] — the rectangle now pinned, and whether the daemon is
/// laying the panes out over it.
///
/// # Why the POLICY is on the answer and not read from the caller's own config
///
/// The pin is stored whatever `window-size` says, so a pin under `largest` is a value that silently
/// does nothing — which is the exact failure this project keeps finding, and the CLI has printed a
/// note about it since the verb existed. That note was built by reading the user's file **in the
/// CLI's own process**, with a comment saying so: *"the daemon was never asked what it thinks the
/// policy is"*. It is the wrong authority twice over — a second reader of a fact the daemon
/// arbitrates by, and a reader that a differing `XDG_CONFIG_HOME` makes wrong in both directions
/// (silent when the pin is inert, and noisy when it is in force). Building a KEYBOARD on that shape
/// would have made a third copy, in a client whose config is re-read per keystroke (R319).
///
/// So the process that PERFORMS the arbitration says what it arbitrated under, and every surface
/// spells the consequence with [`note`](Self::note).
///
/// # The skew arm
///
/// [`policy`](Self::policy) is [`None`] from a daemon older than this key. That is
/// absent-not-wrong: such a daemon still pins, and the honest degradation is to say NOTHING about
/// the policy rather than to guess from a file this process happens to be able to read. An added
/// answer key is additive, so [`WIRE_PROTOCOL`] does not move for it — the rule
/// [`unknown_slot`]'s doc states.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct WindowPin {
    /// The rectangle the daemon RESOLVED and stored, or [`None`] for an un-pin.
    ///
    /// The resolved one, never the requested one: three of the four spellings are descriptions the
    /// caller cannot work out, which is why they are sent as descriptions at all.
    pub size: Option<crate::attach::ClientSize>,
    /// The `window-size` policy the daemon is arbitrating under, or [`None`] from one too old to
    /// say.
    pub policy: Option<crate::WindowSize>,
}

impl WindowPin {
    /// The answer key naming the pinned width.
    pub const COLS_KEY: &'static str = ResizeWindowAsk::COLS_KEY;
    /// The answer key naming the pinned height.
    pub const ROWS_KEY: &'static str = ResizeWindowAsk::ROWS_KEY;
    /// The answer key naming the policy in force at the DAEMON.
    pub const POLICY_KEY: &'static str = "policy";

    /// The answer value the action returns.
    ///
    /// An un-pin renders the two dimensions as JSON `null` rather than omitting them, because a
    /// reader that saw no key could not tell an un-pin from a daemon that answered nothing — and
    /// `null` is what the action has always sent for it.
    #[must_use]
    pub fn to_answer(&self) -> Value {
        let mut map = Map::new();
        let (cols, rows) = match self.size {
            Some(size) => (Value::from(size.cols), Value::from(size.rows)),
            None => (Value::Null, Value::Null),
        };
        map.insert(Self::COLS_KEY.to_owned(), cols);
        map.insert(Self::ROWS_KEY.to_owned(), rows);
        if let Some(policy) = self.policy {
            map.insert(Self::POLICY_KEY.to_owned(), Value::from(policy.name()));
        }
        Value::Object(map)
    }

    /// The pin an answer carries. A missing or unreadable dimension pair is an UN-PIN, and a
    /// missing policy is the skew arm.
    ///
    /// Read key by key with explicit fallbacks — the shape pin's own rule for what it does not
    /// pin — so a daemon that grows a key here cannot break this reader.
    #[must_use]
    pub fn read(answer: &Value) -> Self {
        let size = match (
            answer[Self::COLS_KEY].as_u64(),
            answer[Self::ROWS_KEY].as_u64(),
        ) {
            (Some(cols), Some(rows)) => u16::try_from(cols)
                .ok()
                .zip(u16::try_from(rows).ok())
                .map(|(cols, rows)| crate::attach::ClientSize { cols, rows }),
            _ => None,
        };
        Self {
            size,
            policy: answer[Self::POLICY_KEY]
                .as_str()
                .and_then(crate::WindowSize::parse),
        }
    }

    /// What a person needs told that the screen cannot show them: this resize changed a stored
    /// value and the daemon is not laying anything out over it.
    ///
    /// [`None`] when there is nothing to add — the policy IS `manual`, so the panes moved and that
    /// is the answer, or the daemon did not say what policy it is under.
    ///
    /// # It carries the RECTANGLE, because a display client has shown nothing else
    ///
    /// The CLI prints the resolved size on stdout; a key press prints nothing anywhere. A note that
    /// said only *"size stored"* would leave the person who pressed `resize-window -a` with no way
    /// to learn what their own client folded to — and three of the four spellings are descriptions
    /// the caller cannot work out, so there is nothing for them to infer it from. The CLI repeating
    /// the number on stderr is the honest cost of one wording; the two go to different streams and
    /// different readers.
    ///
    /// # The UN-PIN speaks too, under a policy that was ignoring the pin
    ///
    /// `-u` under `manual` releases a size that WAS in force, so the panes reflow and the screen is
    /// the answer. Under any other policy it removes something that was doing nothing, and nothing
    /// changes — which is the same "a key that did nothing looks like a key that is not bound"
    /// failure the pin half is about, arrived at from the other side.
    ///
    /// **One wording, three surfaces.** `sprag resize-window` prints it, and both display clients
    /// show it in the row a keybinding's report goes to; a second wording would be two answers to
    /// one question, which is what this module exists to prevent. It is written to fit a status row
    /// ([`crate::report::MessageText::MAX_BYTES`]), asserted at the widest rectangle a `u16` pair
    /// can spell.
    #[must_use]
    pub fn note(&self) -> Option<String> {
        let policy = self.policy?;
        if policy == crate::WindowSize::Manual {
            return None;
        }
        Some(match self.size {
            Some(size) => format!(
                "{}x{} stored, but window-size is {} so the panes still follow the clients — \
                 `sprag set-option window-size manual` to use it",
                size.cols,
                size.rows,
                policy.name(),
            ),
            None => format!(
                "un-pinned, but window-size is {} so this window was following the clients either \
                 way",
                policy.name(),
            ),
        })
    }
}

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

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — what [`JOIN_PANE_ACTION`]'s declaration
    /// publishes; see [`ResizeAsk::GRAMMAR`] for why it lives beside the keys.
    ///
    /// The pane is the one REQUIRED argument on this verb: a join with no pane names nothing to
    /// move. No argument here draws from a vocabulary — a pane is an id and a window is a name or
    /// an id, all three of them values the caller reads off another slot.
    pub const GRAMMAR: &'static [CallForm] = &[
        // The destination by NAME. Required on this verb, unlike everywhere else the reference
        // appears: a join with no window names nowhere to put the pane.
        CallForm::object(&[
            ArgGrammar::open(Self::PANE_KEY, "int"),
            WindowRef::NAMED_ARG.required(),
        ]),
        // ...and by IDENTITY.
        CallForm::object(&[
            ArgGrammar::open(Self::PANE_KEY, "int"),
            WindowRef::PICKED_ARG.required(),
        ]),
    ];

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

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — [`ResizeAsk::GRAMMAR`]'s peer, and
    /// what [`SWAP_PANE_ACTION`]'s declaration publishes.
    ///
    /// EVERY argument is optional here, and that is a limit of the DECLARATION rather than of the
    /// grammar: this ask is an enum, so a well-formed call carries `with` or `dir` and never both,
    /// and a schema argument can say *"a call is well-formed without me"* but not *"exactly one of
    /// these two"*. Marking both optional is the true half of that; the alternation is stated in
    /// [`SWAP_PANE_ACTION`]'s prose and enforced by [`parse`](Self::parse). Publishing them as
    /// REQUIRED would be the affirmative false statement — it would tell an agent to send both.
    pub const GRAMMAR: &'static [CallForm] = &[
        // `With` — trade with a pane named outright.
        CallForm::object(&[
            ArgGrammar::open(Self::PANE_KEY, "int").optional(),
            ArgGrammar::open(Self::WITH_KEY, "int"),
        ]),
        // `Toward` — trade with whatever lies that way.
        CallForm::object(&[
            ArgGrammar::open(Self::PANE_KEY, "int").optional(),
            ArgGrammar::one_of(
                Self::DIR_KEY,
                "string",
                &sprag_terminal::PaneDir::WIRE_WORDS,
            ),
        ]),
    ];

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

    /// THE GRAMMAR ABOVE, AS DATA A CLIENT CAN DISCOVER — what
    /// [`RESIZE_PANE_ACTION`]'s declaration publishes.
    ///
    /// Beside [`to_args`](Self::to_args) and [`parse`](Self::parse) because it is the third face of
    /// the one grammar those two are the other two faces of, and a declaration living anywhere else
    /// is a fourth list of these three keys. `dir` publishes the whole vocabulary it admits, taken
    /// from [`PaneDir::WIRE_WORDS`](sprag_terminal::PaneDir::WIRE_WORDS) — which is projected from
    /// the type, so it is the same set [`PaneDir::from_wire`](sprag_terminal::PaneDir::from_wire)
    /// reads and cannot be a stale copy of it.
    ///
    /// It is the one REQUIRED argument here: a boundary is always named by a direction, and
    /// [`parse`](Self::parse) refuses a request without one. The other two are omittable, and both
    /// have a meaning when omitted rather than a default that happens to work — the scoped window's
    /// active pane, and [`CELLS_DEFAULT`](Self::CELLS_DEFAULT).
    pub const GRAMMAR: &'static [CallForm] = &[CallForm::object(&[
        ArgGrammar::one_of(
            Self::DIR_KEY,
            "string",
            &sprag_terminal::PaneDir::WIRE_WORDS,
        ),
        ArgGrammar::open(Self::PANE_KEY, "int").optional(),
        ArgGrammar::open(Self::CELLS_KEY, "int").optional(),
    ])];

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

/// The `scene/invoke` / `scene/query` path addressing the PLUGIN HOST's `address` —
/// `/sprag_plugins/external/<address>`.
///
/// One function for actions and slots, unlike the pane's, because this surface's two verbs and its
/// three slots hang off one node — and because every caller of it was formatting the string inline
/// until the loop got a door. The integration tests that drove `/sprag_plugins/external/run` for
/// rounds each spelled it by hand, which is exactly the folklore
/// [`pane_input_path`] exists to stop one surface along.
#[must_use]
pub fn plugins_path(address: &str) -> String {
    format!("/{}/external/{address}", crate::PLUGINS_TAG)
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

    /// **The RESIZE-WINDOW grammar round trips through the bytes it sends** (R331) — the request
    /// half that was a `json!` at a CLI call site and so invisible to the shape pin.
    ///
    /// Every spelling is asserted as BYTES and not only as a round trip, because the failure this
    /// prevents is not an unreadable request: it is the CLI and the daemon drifting to two spellings
    /// of one key, which round-trips perfectly on each side and matches on neither.
    ///
    /// REVERT-PROOF: make `to_args` omit the zero-axis guard and the un-pin/no-op pair collapses;
    /// make `parse` accept a `window_id` and the identity assertion goes green with a request the
    /// registry has no door for.
    #[test]
    fn the_window_resize_grammar_round_trips_through_the_bytes_it_sends() {
        use crate::window::SizeRequest;

        let shapes = [
            (
                ResizeWindowAsk {
                    window: None,
                    size: SizeRequest::Exact(crate::attach::ClientSize {
                        cols: 100,
                        rows: 30,
                    }),
                },
                json!({"cols": 100, "rows": 30}),
            ),
            (
                ResizeWindowAsk {
                    window: Some("build".to_owned()),
                    size: SizeRequest::Adjust { cols: -20, rows: 0 },
                },
                json!({"window": "build", "adjust_cols": -20}),
            ),
            (
                ResizeWindowAsk {
                    window: None,
                    size: SizeRequest::Clients(crate::WindowSize::Smallest),
                },
                json!({"from": "smallest"}),
            ),
            (
                ResizeWindowAsk {
                    window: None,
                    size: SizeRequest::Clear,
                },
                json!({}),
            ),
            // The all-zero adjustment, which no flag parser builds and which the grammar must still
            // keep DISTINCT from the un-pin above it: the empty object means "throw the size away".
            (
                ResizeWindowAsk {
                    window: None,
                    size: SizeRequest::Adjust { cols: 0, rows: 0 },
                },
                json!({"adjust_cols": 0}),
            ),
        ];
        for (ask, bytes) in &shapes {
            assert_eq!(&ask.to_args(), bytes, "{ask:?}");
            assert_eq!(
                ResizeWindowAsk::parse(bytes),
                Some(ask.clone()),
                "{bytes} must read back as what wrote it",
            );
        }

        // THE ADDRESS: a NAME or the scope, and an identity is REFUSED rather than acted on. This
        // door has no identity-addressed registry entry, so a `window_id` dropped on the floor
        // would resize the SCOPED window instead — the silent wrong act R330 moved the protocol
        // number for, one verb over.
        assert_eq!(ResizeWindowAsk::parse(&json!({"window_id": 7})), None);
        assert_eq!(
            ResizeWindowAsk::parse(&json!({"window": "build", "window_id": 7})),
            None,
        );
        for refused in [
            json!(null),
            json!([]),
            json!("100x30"),
            // HALF a rectangle, both ways round.
            json!({"cols": 100}),
            json!({"rows": 30}),
            // A zero dimension is not a window.
            json!({"cols": 0, "rows": 30}),
            // TWO spellings of one rectangle — four ways, one per request.
            json!({"cols": 100, "rows": 30, "adjust_cols": 4}),
            json!({"cols": 100, "rows": 30, "from": "largest"}),
            json!({"adjust_rows": 2, "from": "smallest"}),
            // `manual` reads a STORED size rather than folding clients, so as a SOURCE it names
            // nothing — a request to pin the window to whatever it is pinned to.
            json!({"from": "manual"}),
            json!({"from": "nonesuch"}),
            json!({"from": 3}),
            json!({"window": 7}),
            json!({"adjust_cols": "4"}),
            json!({"cols": "100", "rows": 30}),
        ] {
            assert_eq!(ResizeWindowAsk::parse(&refused), None, "admitted {refused}");
        }
        assert_eq!(
            ResizeWindowAsk::parse(&json!({"window": null, "cols": 100, "rows": 30})),
            Some(shapes[0].0.clone()),
            "an explicit null is an absent key — `WindowRef::read`'s rule",
        );
    }

    /// **The ANSWER carries the POLICY, and a daemon too old to say so is absent-not-wrong** (R331).
    ///
    /// The note is the whole reason the key exists, so both directions are pinned: a pin under a
    /// policy that ignores it SAYS so, and a pin the daemon performs says nothing — because the
    /// panes moved, which is the answer. The skew arm is the third: no policy key means no claim
    /// about one, never a guess from the reading process's own config file.
    #[test]
    fn a_pin_answers_the_policy_it_was_resolved_under() {
        let pinned = |policy| WindowPin {
            size: Some(crate::attach::ClientSize {
                cols: 100,
                rows: 30,
            }),
            policy,
        };
        assert_eq!(
            pinned(Some(crate::WindowSize::Manual)).to_answer(),
            json!({"cols": 100, "rows": 30, "policy": "manual"}),
        );
        assert_eq!(
            WindowPin {
                size: None,
                policy: Some(crate::WindowSize::Latest),
            }
            .to_answer(),
            json!({"cols": null, "rows": null, "policy": "latest"}),
            "an un-pin sends the keys as NULL rather than omitting them, so a reader can tell it \
             from a daemon that answered nothing at all",
        );
        for policy in [
            None,
            Some(crate::WindowSize::Manual),
            Some(crate::WindowSize::Largest),
        ] {
            assert_eq!(WindowPin::read(&pinned(policy).to_answer()), pinned(policy));
        }

        assert_eq!(
            pinned(Some(crate::WindowSize::Manual)).note(),
            None,
            "a pin the daemon USES needs no words: the panes moved",
        );
        let inert = pinned(Some(crate::WindowSize::Largest))
            .note()
            .expect("a stored-but-inert pin has something to say");
        assert!(
            inert.contains("largest") && inert.contains("manual"),
            "it names the policy in force AND the way out: {inert:?}",
        );
        assert!(
            inert.len() <= crate::report::MessageText::MAX_BYTES,
            "the note is shown in a status row, so it has to fit one: {} bytes",
            inert.len(),
        );
        assert!(
            inert.contains("100x30"),
            "the note carries the RECTANGLE, because a key press shows it nowhere else: {inert:?}",
        );
        // THE WIDEST a `u16` pair can spell, so the status-row bound above is asserted at the
        // extreme rather than at a convenient value.
        let widest = WindowPin {
            size: Some(crate::attach::ClientSize {
                cols: u16::MAX,
                rows: u16::MAX,
            }),
            policy: Some(crate::WindowSize::Smallest),
        }
        .note()
        .expect("the widest pin still has something to say");
        assert!(
            widest.len() <= crate::report::MessageText::MAX_BYTES,
            "the widest note must still fit a status row: {} bytes",
            widest.len(),
        );
        // THE UN-PIN under a policy that was ignoring the pin: nothing changes on screen, which is
        // the pin half's failure arrived at from the other side.
        let released = WindowPin {
            size: None,
            policy: Some(crate::WindowSize::Largest),
        }
        .note()
        .expect("an un-pin under a policy that ignored the pin changed nothing visible");
        assert!(
            released.contains("un-pinned") && released.contains("largest"),
            "it says what happened and why nothing moved: {released:?}",
        );
        assert_eq!(
            WindowPin {
                size: None,
                policy: Some(crate::WindowSize::Manual),
            }
            .note(),
            None,
            "an un-pin the policy WAS using needs no words: the panes reflow",
        );
        assert_eq!(
            pinned(None).note(),
            None,
            "and a daemon that did not say makes no claim: the skew arm is silent, never a guess \
             from this process's own config file",
        );

        // THE PRE-R331 ANSWER, which a client of this build still meets: `{cols, rows}` and no
        // policy. It reads as a pin with nothing said about the policy — not as an un-pin, and not
        // as `manual`.
        assert_eq!(
            WindowPin::read(&json!({"cols": 100, "rows": 30})),
            pinned(None),
        );
        assert_eq!(
            WindowPin::read(&Value::Null),
            WindowPin {
                size: None,
                policy: None,
            },
            "the older un-pin answer was a bare `null`, and it still reads as an un-pin",
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

        // The RESIZE request and its ANSWER (R331), both sides of a grammar that was two `json!`
        // call sites until this round — the exact hole this pin's own doc says it cannot see. The
        // un-pin renders EMPTY, which is what makes every other spelling additive to it.
        assert_eq!(
            serde_json::to_string(
                &ResizeWindowAsk {
                    window: None,
                    size: crate::window::SizeRequest::Clear,
                }
                .to_args()
            )
            .expect("an un-pin serialises"),
            "{}",
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &ResizeWindowAsk {
                    window: Some("build".to_owned()),
                    size: crate::window::SizeRequest::Adjust { cols: -20, rows: 3 },
                }
                .to_args()
            )
            .expect("a relative resize serialises"),
            r#"{"window":"build","adjust_cols":-20,"adjust_rows":3}"#,
            "{}",
            BUMP,
        );
        assert_eq!(
            serde_json::to_string(
                &WindowPin {
                    size: Some(crate::attach::ClientSize {
                        cols: 100,
                        rows: 30
                    }),
                    policy: Some(crate::WindowSize::Largest),
                }
                .to_answer()
            )
            .expect("a pin serialises"),
            r#"{"cols":100,"rows":30,"policy":"largest"}"#,
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
                    // EMPTY, and the pinned string below is what that buys: the row-share fact is
                    // skipped when it says nothing, so a frame from a host nobody asked for it is
                    // byte-identical to every frame this wire has ever carried.
                    shares: sprag_grid::RowShares::default(),
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
            serde_json::to_string(
                &SelectWindowAsk::At(WindowRef::Named("logs".to_owned())).to_args()
            )
            .expect("an ask renders"),
            r#"{"window":"logs"}"#,
            "{}",
            BUMP,
        );
        // The IDENTITY arm (R330). Pinned beside the name because the two are one ask now: a client
        // that sent the wrong key would be addressing a window it does not hold.
        assert_eq!(
            serde_json::to_string(&SelectWindowAsk::At(WindowRef::Picked(WindowId(4))).to_args())
                .expect("an ask renders"),
            r#"{"window_id":4}"#,
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

    /// ⚠⚠ THE VALUE-SPACE PIN — the half of the wire the surface pin above is BLIND to.
    ///
    /// [`PINNED_SURFACE`] compares the ADDRESSES the daemon serves, and the shape pin renders one
    /// canonical value of each type a client decodes. Neither can see an enum GAINING AN ARM: no
    /// address moves, and every canonical value it already rendered still renders byte-identically.
    /// R342 added `Unmeasured::Refused` and `Check::PaneAdmission`, ran the entire suite, and it
    /// went green — while a `sprag` built the day before could no longer parse either answer.
    ///
    /// # Why an added arm is a BREAK where an added key is not
    ///
    /// An added answer KEY is absent-not-wrong to an old reader: it reads the keys it knows and
    /// ignores the rest. An added VARIANT is not, because serde has nowhere to put it — a decoder
    /// meeting `"pane-admission"` for a three-armed enum fails the whole document, so one refused
    /// pane on one host turns `sprag doctor` into a parse error rather than a row nobody reads.
    ///
    /// # What this asserts
    ///
    /// The two closed sets a client decodes WHOLE, pinned by their serialised words. The list is
    /// derived from `ALL` on both — a hand-typed list is the one a new arm is left out of, which is
    /// the defect this pin exists for and would be an absurd way to build it.
    #[test]
    fn an_answers_value_space_cannot_widen_under_the_protocol_number() {
        const PINNED_VALUES: (u32, &[&str]) = (
            // R344: the number moved for a MEANING under an unchanged name (`PaneMatch::line`),
            // not for a widened value space — every word below is the word it was at 18.
            // R353: and again, for a published FORM's SHAPE (`action_grammar`'s `{form, args}`).
            // Neither of the two enums a peer decodes whole gained or lost a word, which is exactly
            // what this re-stamp says.
            // ⚠⚠ R357 IS THE FIRST RE-STAMP THIS PIN EARNED ITSELF: `run_status` joined the list
            // (a fourth word, `interrupted`) AND joined this pin at all. Its four words lived as
            // string literals inside `run_to_json` until then, so the vocabulary had no `ALL` to
            // walk and this gate — the one written for exactly this break — was blind to it.
            // R359b: re-stamped for a REQUEST value that changed shape (`ready_when`); neither
            // enum a peer decodes whole moved, which is what this says.
            // R364: re-stamped for an ADDED REQUEST ARGUMENT (`shows_prompt`, which buys a
            // guarantee whose absence is indistinguishable from it holding). No ANSWER word moved
            // — what an unconfirmed delivery says reaches a caller as a step's NOTE, which is free
            // text by design and so has no value space to widen.
            // ⚠⚠ R365 IS THE SECOND RE-STAMP THIS PIN EARNED ITSELF, and it is R357's shape
            // exactly: `signal_key` and `unraised` JOIN the list at all, because a pane input
            // action's answer now carries closed vocabularies where it carried nothing. They are
            // the deliberate opposite of R364's free-text note — *which* key was swallowed and
            // *why* are two things a caller BRANCHES on (retry, reconfigure, or reach for
            // `stop_job`), and a sentence cannot be branched on.
            // R365 again: re-stamped for an ADDED REQUEST ARGUMENT (`done_when`). No ANSWER word
            // moved — a completion contract is something a caller SAYS, not something it is told.
            // ⚠⚠⚠ R365 A THIRD TIME, AND THIS PIN HAD A BLIND SPOT IT DID NOT SEE: a run's
            // OUTCOME word (`converged` | `exhausted` | `failed` | `cancelled`) is an answer a peer
            // decodes whole, and it lived outside this list exactly as `run_status` did before
            // R357. `blocked` is the fifth, and it JOINS the pin in the same edit that adds it.
            // ⚠⚠⚠ R366 AND THE SAME BLIND SPOT ONE LEVEL DOWN: a STEP's `verdict` is the other
            // closed set a run's answer carries whole, and it was outside this list for the same
            // reason the outcome word was — four literals inside `Verdict::wire_str` with nothing
            // walking them. `answered` is the fifth verdict and it JOINS the pin in the edit that
            // adds it, exactly as `blocked` did one round ago. So does `refusal`, which is new in
            // both senses: a fresh vocabulary on a fresh key (`asking.why`), published so a caller
            // can BRANCH on why a run stopped rather than read a sentence — R365's argument for
            // `signal_key` and `unraised`, and the reason those are not free-text notes either.
            // R367: re-stamped for an ADDED ANSWER KEY (`asking` on a pane's `agent` object) whose
            // ABSENCE a reader takes as a claim. NEITHER enum a peer decodes whole moved, which is
            // what this re-stamp says: the question's own members are lines, numbers, labels and a
            // flag — a caller reads them, it does not decode a closed set out of them. The one word
            // near it that IS closed (`state`) is unchanged; `blocked` has been there since 26.
            // ⚠⚠ R370 IS THE FOURTH RE-STAMP THIS PIN EARNED ITSELF: `refusal` gained a SEVENTH
            // word (`contradicted`), and it is the one failure a LIST of consents can have that a
            // single clause could not — two clauses about one question naming different options.
            // A caller branches on it exactly as on the other six (narrow one of your own rules,
            // rather than re-read the dialog), which is why it is a word and not a sentence.
            // ⚠⚠ R371 IS THE FIFTH, and the same shape one contract further out: `refusal` gained
            // an EIGHTH word (`unattended`), the failure that only a run which may WAIT can have —
            // a person was promised, the patience ran out, and the dialog is still up. It is the
            // one arm in this vocabulary about a HUMAN rather than about a clause, and its remedy
            // differs from all seven others (be there, or wait longer), which is exactly the test
            // for a word rather than a sentence. ⚠ The clause-level reason it would otherwise have
            // reported is NOT lost and does NOT widen anything: it rides in the free-text detail,
            // R364's shape.
            // ⚠⚠⚠ R372 IS THE SIXTH, AND THE FIRST TO MOVE **TWO** OF THESE VOCABULARIES AT ONCE:
            // a run's OUTCOME gains a sixth word and a step's VERDICT a fifth, both `taken_over`.
            // They move together because they are one fact reported at two altitudes — the step
            // that stopped, and the run that ended because of it — which is the shape `blocked`
            // has had since R365/R366.
            // ⚠⚠ WHAT IS NEW IS NOT A WIDER SPACE BUT A FACT THE PRODUCT COULD NOT SEE: a PERSON
            // typing into a pane a run is driving. `send_key` is one encoder shared by a display
            // client's keyboard and this wire — deliberately, so the two encode identically — and
            // nothing recorded WHICH had written, so a run typed over its supervisor and reported
            // `exhausted`. The hand is recorded at the write now (`sprag_terminal::Hand`).
            // ⚠ It is a WORD and not a sentence by this pin's own test: the remedy differs from
            // every other outcome's. `blocked` says answer the question, `failed` says fix
            // something, `exhausted` says raise a budget — and this one says do NOTHING, because
            // the pane already belongs to somebody who is using it.
            // ⚠⚠⚠ R372: RE-STAMPED WITH NOT ONE ARM MOVED, AND THAT IS THE ROUND'S FINDING. Eleven
            // parametric families turned a `null` ANSWER into a `-32602 QueryTypeMismatch` REFUSAL,
            // which is as wire-visible a change as this workspace has shipped — and this pin cannot
            // see it, because `QueryTypeMismatch` is PINION's word rather than one of sprag's own
            // closed vocabularies. **A value that becomes a refusal is a fourth bump cause, and no
            // pin here covers it.** Recorded rather than papered over: the honest fix would be a
            // pin over what each declared address ANSWERS WITH (a value / which fault), and this
            // round did not build one.
            // ⚠ R373: re-stamped with not one ARM moved, and this time that is the DESIGN rather
            // than a gap. A pane coming back from a person is `continue` with a journal note, on
            // R369's ruling about the sixth outcome word: a run that spelt a human's act with a
            // word of its own would let a reader count it among the decisions the RUN took.
            // `taken_over` still means what it meant — the person still has it.
            // ⚠ R375: re-stamped with not one ARM moved, and again by design. A run that waits for
            // its peer's turn to END rather than for a 500 ms clock reaches the SAME endings by a
            // better route: it converges on the sentinel it was always looking for, and it
            // exhausts, blocks and is taken over exactly as before. Nothing a peer decodes whole
            // learnt a word, because nothing about the ANSWER changed — only how long the run was
            // willing to wait before speaking again.
            // ⚠⚠⚠ R384: THREE WORDS JOIN AT ONCE — a SEVENTH verdict (`screened`) and a NINTH and
            // TENTH refusal (`no_rule`, `not_dismissed`) — and none of them costs the number, on
            // R381's specific fact rather than on a general rule.
            //
            // All three are produced by ONE state of ONE plugin, `ai_loop.scxml`'s `screening`, and
            // that plugin is selected by a `plugin` value no client older than this build knows. An
            // old client never sends `ai_loop`, so it never receives a journal or an `asking.why`
            // that can carry any of them; a new client that sends `ai_loop` to an old daemon meets
            // an ordinary vocabulary refusal at the door, because the `plugins` slot publishes the
            // word and it can ask first. **Neither half of a skewed pair can misread anything.**
            //
            // ⚠⚠ AND `screen_rules` IS AN ADDED ARGUMENT ON THAT SAME FORM, which is normally this
            // wire's second-commonest bump cause because the surface SWALLOWS an undeclared key and
            // the run succeeds. It is free here for the identical reason and NOT for a general one.
            // ⚠⚠⚠ THE RESIDUE, WHICH IS THE SAME ONE R382 LEFT AND IS NOW LARGER: the day `ai_loop`
            // ships, the next argument added to it — and the next word `screening` learns — earns
            // the number by the ordinary rule.
            //
            // ⚠ `screen_permissions` was NEVER on this wire, so its removal from the loop document
            // moves nothing here. It is recorded because a reader looking for it should find out
            // that a measurement removed the need for it rather than that somebody forgot it.
            34,
            &[
                "check:pane-isolation",
                "check:pane-admission",
                "check:controller-delegation",
                "check:competing-weight",
                "check:cpu-stall",
                "check:io-stall",
                "check:memory-stall",
                "check:swapping",
                "check:build-saturation",
                "check:ccache-on-path",
                "check:ccache-sizing",
                "check:fast-linker",
                "unmeasured:nothing_enforced",
                "unmeasured:not_placed",
                "unmeasured:refused",
                // ⚠ R357: the run status, and the fourth word is the one that cost the number.
                "run_status:running",
                "run_status:done",
                "run_status:panicked",
                "run_status:interrupted",
                "unmeasured:gone",
                // ⚠ R365: the two an injection's caveat is built from.
                "signal_key:interrupt",
                "signal_key:quit",
                "signal_key:suspend",
                "unraised:raw",
                "unraised:unbound",
                // ⚠⚠ R365: a RUN'S OUTCOME, which this pin could not see until now.
                "outcome:converged",
                "outcome:exhausted",
                "outcome:failed",
                "outcome:cancelled",
                "outcome:blocked",
                // ⚠⚠ R372: the sixth outcome — a person took the pane.
                "outcome:taken_over",
                // ⚠⚠ R366: a STEP's verdict, which this pin could not see until now — the fifth
                // word is the one that cost the number.
                "verdict:continue",
                "verdict:converged",
                "verdict:blocked",
                "verdict:answered",
                // ⚠⚠ R372: the fifth verdict — the step that stopped because a person took over.
                "verdict:taken_over",
                // ⚠⚠⚠ R381: THE SIXTH VERDICT — a plugin whose OWN document carries a budget saying
                // that budget is spent. `ai_loop.scxml`'s `max_turns` counts the inner agent's
                // turns and one of those is many steps of the loop driving it, so no guardrail can
                // see it; without this word the run had to report `exhausted — iterations` about a
                // ceiling it never met.
                //
                // ⚠⚠⚠ **AND IT DID NOT COST THE NUMBER, which is a judgement and not an
                // oversight.** This pin's standing sentence says an added answer word breaks older
                // readers and the protocol number must rise. It cannot here, and the reason is
                // specific rather than general: this word is produced by exactly one plugin, and
                // that plugin is selected by a `plugin` value no client older than this build
                // knows. An old client never sends `ai_loop`, so it never receives a journal that
                // can carry this word; a new client that sends `ai_loop` to an old daemon meets an
                // ordinary vocabulary refusal at the door, because the `plugins` slot publishes
                // the word and it can ask first. **Neither half of a skewed pair can misread
                // anything**, which is the one escape the argument-shape pin beside this offers.
                // ⚠ The same argument covers `ceiling: "turns"`, which this pin deliberately does
                // not walk (see `sprag_plugin::Ceiling`).
                "verdict:exhausted",
                // ⚠⚠⚠ R384: THE SEVENTH VERDICT — a step that REFUSED its peer's tool call on the
                // loop author's standing instruction and told it what to do instead. Not folded
                // into `answered` because the two are opposite decisions: one takes an option the
                // peer offered, and a reader asking *what did my run let its agent DO* must not be
                // handed a count that also includes what it stopped.
                "verdict:screened",
                // ⚠⚠ R366: WHY a blocked run did not answer. A caller branches on these — fix a
                // needle, write a consent, or fetch a person — so they are words and not prose.
                "refusal:unreadable",
                "refusal:not_taken",
                "refusal:no_consent",
                "refusal:other_question",
                "refusal:not_offered",
                "refusal:ambiguous",
                // ⚠⚠ R370: the arm a LIST of consents made possible — the caller's own clauses
                // disagreeing about the question on screen.
                "refusal:contradicted",
                // ⚠⚠⚠ R371: the arm `await_person_ms` made possible, and the ONLY one in this
                // vocabulary about a HUMAN rather than about a clause — a run that waited for the
                // person it was promised and gave up. Its remedy is its own (be there, or raise the
                // patience), which is why it is a word and not the clause-level reason it carries
                // underneath in free text.
                "refusal:unattended",
                // ⚠⚠⚠ R384: the two `screening` made possible, and each has a remedy no arm above
                // it has. `no_rule` is about the loop DOCUMENT — no standing instruction quotes the
                // dialog, so edit the rules — where every arm above is answered by changing the
                // call or fetching somebody. `not_dismissed` is about the AGENT: a rule fired, the
                // key that refuses a call went in, and the dialog stayed, so **nothing was typed**.
                "refusal:no_rule",
                "refusal:not_dismissed",
            ],
        );

        // The serialised WORD of each arm. A unit variant renders as its string; a carrying one
        // renders as a one-key object, and the key is the word an older decoder would reject.
        fn word(value: &serde_json::Value) -> String {
            match value {
                serde_json::Value::String(name) => name.clone(),
                serde_json::Value::Object(map) => map
                    .keys()
                    .next()
                    .cloned()
                    .unwrap_or_else(|| "<empty object>".to_owned()),
                other => format!("<not a variant: {other}>"),
            }
        }

        let mut served: Vec<String> = sprag_terminal::Check::ALL
            .iter()
            .map(|check| {
                format!(
                    "check:{}",
                    word(&serde_json::to_value(check).expect("render"))
                )
            })
            .chain(sprag_terminal::Unmeasured::ALL.iter().map(|reason| {
                format!(
                    "unmeasured:{}",
                    word(&serde_json::to_value(reason).expect("render"))
                )
            }))
            // ⚠⚠ THE RUN STATUS, which was invisible to this pin until R357 gave it a type. The two
            // above are serde enums and render through serde; this one is a hand-written renderer's
            // vocabulary, so it joins through its OWN words — which is the point: a value space with
            // no declaration cannot be pinned, and `interrupted` was added to a set a peer decodes
            // whole with nothing here able to notice.
            .chain(
                crate::plugins::RunStatus::WIRE_WORDS
                    .iter()
                    .map(|word| format!("run_status:{word}")),
            )
            // ⚠⚠ R365: the injection caveat's two vocabularies, joined the same way and for the
            // same reason — they are hand-rendered words, so nothing else here could see one being
            // added. `SignalKey` is bounded by `termios` (there are three such characters and the
            // kernel defines no fourth), which is a reason to expect this half to hold still, NOT
            // a reason to leave it unpinned: an expectation nothing checks is how the last one got
            // through.
            .chain(
                sprag_terminal::SignalKey::WIRE_WORDS
                    .iter()
                    .map(|word| format!("signal_key:{word}")),
            )
            .chain(
                sprag_terminal::Unraised::WIRE_WORDS
                    .iter()
                    .map(|word| format!("unraised:{word}")),
            )
            // ⚠⚠ A RUN'S OUTCOME WORD, read through `outcome_word` — the same renderer the wire and
            // the durable run log both use, so this pin is over the words a peer really meets.
            //
            // ⚠⚠ R366: the LIST, not five variants written out here. This walked a hand-written
            // array because `OutcomeState` carries data and had no `ALL` — the residue R365 stated,
            // and the shape by which a fifth word reached the wire with every ratchet green. The
            // type publishes its own words now and a gate beside it holds them to `wire_str`, so a
            // sixth outcome cannot be spelled without being published or published without a word.
            .chain(
                sprag_plugin::OutcomeState::WIRE_WORDS
                    .iter()
                    .map(|word| format!("outcome:{word}")),
            )
            // ⚠⚠ A STEP'S VERDICT, joined through the type's own published list. `Verdict` carries
            // data too, so its list is hand-ordered where `Refusal`'s is projected — and the
            // round-trip gate beside the type holds the hand-written half to `wire_str`, which is
            // what keeps this from pinning a list the product does not serve.
            .chain(
                sprag_plugin::Verdict::WIRE_WORDS
                    .iter()
                    .map(|word| format!("verdict:{word}")),
            )
            .chain(
                sprag_plugin::Refusal::WIRE_WORDS
                    .iter()
                    .map(|word| format!("refusal:{word}")),
            )
            .collect();
        let mut pinned: Vec<String> = PINNED_VALUES.1.iter().map(|n| (*n).to_owned()).collect();
        served.sort_unstable();
        pinned.sort_unstable();
        assert_eq!(
            served, pinned,
            "AN ANSWER'S VALUE SPACE MOVED. A peer that decodes one of these enums whole cannot \
             read a word it does not have, and serde fails the whole document rather than the \
             field. An arm ADDED breaks OLDER READERS of the answer (the number must rise); an arm \
             REMOVED or RENAMED breaks them too. Update this pin and \
             sprag_rpc::WIRE_PROTOCOL together.",
        );
        assert_eq!(
            PINNED_VALUES.0,
            sprag_rpc::WIRE_PROTOCOL,
            "THE PROTOCOL NUMBER MOVED WITH EVERY VALUE SPACE UNCHANGED — legitimate when some \
             other part of the wire moved, and a mistake when this pin was simply not re-stamped.",
        );
    }

    /// ⚠⚠ **THE PUBLISHED VOCABULARY PIN — the value spaces a client READS OFF THE WIRE.**
    ///
    /// [`an_answers_value_space_cannot_widen_under_the_protocol_number`] pins the two closed sets a
    /// peer DECODES, where a widened set breaks the decode. This pins the closed sets the daemon
    /// PUBLISHES, where a widened set is something else: an agent that enumerated the vocabulary
    /// yesterday built its calls from a shorter list, and one that reads it today gets a longer
    /// one. Neither breaks — which is exactly why nothing else can see the change.
    ///
    /// # Why this is a pin and not left to the two gates
    ///
    /// [`every_published_word_is_a_word_the_daemon_accepts`](crate::workspace) holds the published
    /// set to the PARSER, so the two move together — and that is the point: they move together
    /// SILENTLY. A round that adds an arm to `PaneDir` widens the type, the parser, and the wire in
    /// one compile with every test green, and the client that hard-coded four directions is the
    /// last to find out. This is the line that makes that a decision.
    ///
    /// ⚠⚠ **THIS IS THE ONLY GATE THAT CAN SEE A CHANGED WORD, and that was measured rather than
    /// assumed.** Renaming `SplitDir::Horizontal`'s word to `sideways` leaves
    /// [`every_published_word_is_a_word_the_daemon_accepts`](crate::workspace) GREEN — necessarily,
    /// because the parser reads through the same spelling the publication is projected from, so the
    /// two move together by construction. That is the device working, and it is exactly why a pin
    /// is still owed: agreement between publisher and parser says nothing about agreement with the
    /// CLIENT that enumerated the vocabulary last week. This one goes RED naming
    /// `split:dir=sideways,vertical`.
    ///
    /// ⚠ **DERIVED FROM THE SERVED ANSWER**, never from the const table: it reads
    /// [`ACTION_GRAMMAR_SLOT`]'s own JSON, so a vocabulary that stops being published — because the
    /// verb left the table, or the slot stopped answering — fails here too. R320's rule: a ratchet
    /// over a declaration is not a ratchet over the product.
    #[test]
    fn a_published_value_space_cannot_widen_under_the_protocol_number() {
        const PINNED_WORDS: (u32, &[&str]) = (
            // R357: the number moved for an ANSWER's value space (`status` gained `interrupted`),
            // with every PUBLISHED (argument) vocabulary unchanged — the two are different lists
            // and this pin holds the request half.
            // ⚠⚠ R359b IS A RE-STAMP THIS PIN EARNED ITSELF: `run` gained a nested `ready_when`
            // whose `match` is a closed set (`prints` | `shows`), so the request half DID widen.
            // R364: re-stamped for an ADDED REQUEST ARGUMENT (`shows_prompt` on the agent
            // form). An argument NAME is not a published VALUE space — this pin holds the words a
            // client picks a value from — so nothing here moved, which is what this says.
            // R365: re-stamped for an ANSWER that gained two closed vocabularies (`unsignalled`'s
            // `key` and `because`). This pin holds the REQUEST half — the words a client picks a
            // value FROM — and no verb here takes a new one: the caveat is something a caller is
            // TOLD, never something it says. `an_answers_value_space_cannot_widen_…` is the pin
            // that moved, and the two lists being different is the whole reason there are two.
            // R365 (third): re-stamped for a widened ANSWER value space (a run's fifth outcome
            // word, `blocked`). No REQUEST vocabulary moved.
            // ⚠⚠ R365 AGAIN, and THIS time this pin is the one that moved: `run` gained
            // `done_when=exits,settles`, a vocabulary a caller picks a value FROM. The number moves
            // with it — not because the space widened (R342 settled that widening alone need not)
            // but because the ARGUMENT is new, and an older daemon SWALLOWS an undeclared key
            // rather than refusing it. Measured:
            // `an_argument_this_surface_does_not_declare_is_swallowed_rather_than_refused`.
            // ⚠⚠ R366: re-stamped for an ADDED REQUEST ARGUMENT (`may_answer` and its two needles,
            // on each form that injects) with NO published vocabulary moving — and the absence is
            // the design rather than an omission. A consent quotes the AGENT'S OWN WORDS back at
            // it, so both needles are open strings; a closed set here could only be sprag's guess
            // at what dialogs say, which is the guess `sprag_detect::question` exists to not make.
            // The number moves for the argument, on version 25's measured grounds.
            // R367: re-stamped for an ADDED ANSWER KEY (`asking` on a pane's `agent` object). This
            // pin holds the REQUEST half, and R367 added no argument at all — the question travels
            // outward only. ⚠ That asymmetry is the round's open residue rather than an oversight:
            // a pane-level surface that can say what a peer is asking still offers no way to ANSWER
            // it there, which is the run surface's `may_answer` and is registered as owed.
            // ⚠⚠ R370: re-stamped for a REQUEST VALUE THAT CHANGED SHAPE (`may_answer`, an object
            // to a LIST of them), with every published vocabulary unchanged — which is exactly what
            // this pin can and cannot see. It holds the WORDS a client picks a value from, and a
            // consent's two needles are open strings by design, so a shape change under those names
            // is invisible here in both directions. ⚠ That blindness is what
            // `a_published_argument_shape_cannot_move_under_the_protocol_number` was written for,
            // in the same round: this pin's own gap, closed by the pin beside it rather than by
            // widening this one, because a VALUE SPACE and a SHAPE fail differently.
            // ⚠⚠ R371: re-stamped for an ADDED REQUEST ARGUMENT (`await_person_ms`, on the three
            // forms that loop) with no published vocabulary moving. The argument is a DURATION, so
            // there is no closed set for a client to pick a value from — the same absence-by-design
            // `may_answer`'s two needles have, and invisible here for the same reason. ⚠ THIS TIME
            // THE BLIND SPOT WAS COVERED: the shape pin beside this one went red, which is exactly
            // what R370 built it for.
            // ⚠ R372: re-stamped with EVERY published REQUEST vocabulary unchanged. What moved is
            // an ANSWER's value space (`taken_over`, on two enums at once), and this pin walks the
            // words a CALLER may send — a run's outcome is not one of them. The pin that owns that
            // half went red first and named both arms, which is the division of labour these four
            // pins exist for.
            // ⚠ R372: re-stamped with every published REQUEST vocabulary unchanged. Eleven
            // parametric families started REFUSING a malformed member instead of answering `null`,
            // and not one word a caller may SEND changed to do it.
            // ⚠⚠ R373: re-stamped for an ADDED REQUEST ARGUMENT (`handback_still_ms`) with no
            // published vocabulary moving — R371's case exactly, and for its reason: the argument is
            // a DURATION, so there is no closed set for a client to pick from. ⚠ THE SHAPE PIN
            // BESIDE THIS ONE WENT RED AGAIN, by name, which is the second time it has covered this
            // pin's designed blind spot.
            // ⚠⚠⚠ R373 AGAIN, AND THIS TIME THIS PIN IS THE ONE THAT SEES IT: a NEW CLOSED
            // VOCABULARY on the three pane-input verbs that write (`hand=person,program`). It is
            // the argument that made version 31's whole feature reachable — both frontends attach
            // over this socket, so until it existed a person's keystroke was stamped `program` and
            // no supervised run could see them. A widened value space usually leaves the number
            // standing (R342); this one moved it, because it arrived with the ARGUMENT that
            // carries it and an added argument is a bump on this wire.
            // ⚠⚠⚠ R375: re-stamped for a vocabulary that reached a SECOND FORM. `done_when`'s two
            // words are unchanged and have been published since 25; what moved is that the
            // `orchestrator` form now offers them too, so the same closed set is now advertised at
            // a place it was not. **And this pin's sibling caught the draft that published them
            // there without serving them** — the second time that has happened to this exact
            // argument, so the word is servable ALONE and the bound beside it is optional.
            34,
            // An entry with nothing after the colon publishes a grammar and NO closed vocabulary —
            // ids, names, paths and numbers, all of them values the caller invents. They are here
            // rather than filtered out because a verb that GAINS a vocabulary must move this pin,
            // and a list of only the verbs that already have one could not notice.
            //
            // ⚠ EACH ENTRY IS PREFIXED BY ITS SURFACE, because two surfaces publish grammars now
            // and a bare verb name could not say which. The pane's six were added at R353, and FOUR of
            // their vocabularies had never been published anywhere: a mouse button, a mouse edge, a
            // key edge, and a clipboard selection.
            &[
                "sprag_workspace/sprag_mux/break_pane:",
                "sprag_workspace/sprag_mux/close:",
                "sprag_workspace/sprag_mux/display_message:severity=note,warn,alert",
                "sprag_workspace/sprag_mux/drop_file:",
                "sprag_workspace/sprag_mux/grant_pane:",
                "sprag_workspace/sprag_mux/join_pane:",
                "sprag_workspace/sprag_mux/kill_session:",
                "sprag_workspace/sprag_mux/kill_window:",
                "sprag_workspace/sprag_mux/move_pane:dir=horizontal,vertical",
                "sprag_workspace/sprag_mux/move_window:place=first,last,next,previous",
                "sprag_workspace/sprag_mux/new_session:",
                "sprag_workspace/sprag_mux/new_window:",
                "sprag_workspace/sprag_mux/release_agent:",
                "sprag_workspace/sprag_mux/rename_pane:",
                "sprag_workspace/sprag_mux/rename_session:",
                "sprag_workspace/sprag_mux/rename_window:",
                "sprag_workspace/sprag_mux/report_agent:state=working,blocked,idle",
                "sprag_workspace/sprag_mux/resize:",
                "sprag_workspace/sprag_mux/resize_pane:dir=left,right,up,down",
                "sprag_workspace/sprag_mux/resize_window:from=largest,smallest,latest",
                "sprag_workspace/sprag_mux/select_pane:dir=left,right,up,down",
                "sprag_workspace/sprag_mux/select_window:relative=next,previous",
                "sprag_workspace/sprag_mux/set_floating:",
                "sprag_workspace/sprag_mux/spawn:",
                "sprag_workspace/sprag_mux/split:dir=horizontal,vertical",
                "sprag_workspace/sprag_mux/stop_job:signal=interrupt,terminate,kill",
                "sprag_workspace/sprag_mux/swap_pane:dir=left,right,up,down",
                "sprag_workspace/sprag_mux/zoom_pane:",
                // A PANE'S INPUT, all six verbs. `key`'s `state` and `clipboard_answer`'s `sel` were
                // string literals inside the parsers; `mouse`'s two were spelled once per SIDE of the
                // wire, in two crates, with nothing comparing the lists.
                "sprag_workspace/pane_<id>/sprag_input/clipboard_answer:sel=c,p",
                "sprag_workspace/pane_<id>/sprag_input/focus:",
                "sprag_workspace/pane_<id>/sprag_input/key:hand=person,program state=down,up",
                "sprag_workspace/pane_<id>/sprag_input/mouse:button=left,middle,right,wheelup,\
                 wheeldown,wheelleft,wheelright,none kind=press,release,drag,motion",
                "sprag_workspace/pane_<id>/sprag_input/paste:hand=person,program",
                "sprag_workspace/pane_<id>/sprag_input/text:hand=person,program",
                // THE PLUGIN HOST, whose two verbs published nothing until R353. ⚠ `plugin` appears
                // FIVE TIMES because each form publishes only the word that SELECTS it — the union is
                // the whole vocabulary, and a client can tell which key set goes with which plugin.
                //
                // ⚠⚠ `plugin=answer` IS THE ONE THIS ROUND ADDED, AND `WIRE_PROTOCOL` STANDS AT 28.
                // R342 settled that widening an argument's VALUE SPACE does not earn a number, and
                // the reason applies here in its strongest form: `plugin` is the one argument on
                // this wire whose value is READ AND MATCHED rather than swallowed, so a client that
                // sends `answer` to a daemon without it is answered `TypeMismatch` at the door.
                // That is the opposite of the failure a bump exists to prevent — `may_answer`
                // earned 27 precisely because an older daemon SWALLOWS an undeclared key and
                // answers `ok` to a run that will never answer anything. Here the older daemon
                // cannot mistake the request for one it can serve, and the words are published, so
                // a client can ask first.
                "sprag_workspace/sprag_plugins/cancel:",
                // ⚠⚠ R381: the SIXTH `plugin` word, `ai_loop` — the outer loop as a run somebody
                // can start. A widened ARGUMENT vocabulary leaves the number standing (R342's
                // rule): a client that never heard of the word cannot send it, and one that sends
                // it to an older daemon meets a vocabulary refusal at the door rather than a run
                // that quietly does something else.
                "sprag_workspace/sprag_plugins/run:done_when=exits,settles \
                 format_a=text,claude_json format_b=text,claude_json plugin=agent plugin=ai_loop \
                 plugin=answer plugin=dialogue plugin=orchestrator plugin=pipe",
            ],
        );

        // ⚠ THROUGH THE DAEMON'S OWN SCENE, not through `ActionGrammar::answer()`. The first
        // version of this ratchet called the renderer directly and its own doc claimed it read the
        // served answer — the exact defect R320 records, one level down: deleting the slot's arm
        // from `RegistryView::query` left it GREEN, because the table it was reading is not the
        // thing a client can reach.
        // ⚠ EVERY SURFACE THAT SERVES THE SLOT, derived from the walk rather than from a list of
        // two: a THIRD surface publishing a grammar joins this pin in the compile that adds it, and
        // one that stops serving the slot drops out of it. That is the difference between a pin over
        // the product and a pin over the two names somebody remembered.
        let serving: Vec<String> = served_fields()
            .into_iter()
            .filter(|field| field.path == ACTION_GRAMMAR_SLOT && field.answers)
            .map(|field| field.under)
            .collect();
        assert!(
            !serving.is_empty(),
            "the grammar slot is SERVED, or everything below is about a table nobody can read",
        );
        let mut served: Vec<String> = Vec::new();
        for under in &serving {
            let published = query_served_on(under, ACTION_GRAMMAR_SLOT).expect("the slot answers");
            let verbs = published.as_object().expect("the slot answers an object");
            served.extend(verbs.iter().map(|(action, forms)| {
                let mut spaces: Vec<String> = forms
                    .as_array()
                    .expect("a verb answers its forms")
                    .iter()
                    .flat_map(|form| {
                        // A FORM IS AN OBJECT now — `{form, args}` — because a form that carries its
                        // shape can describe a scalar call, which an array of arguments could not.
                        // That is the value change this pin's number rose for.
                        //
                        // ⚠ THE SHAPE WORD IS A PUBLISHED VALUE TOO, and this is the only pin that
                        // can see it: it is not an argument, so it carries no `one_of` and the three
                        // property gates never look at it. Held to the TYPE's own vocabulary, so a
                        // third shape reaches a client's reader only if `FormKind` spells it.
                        let shape = form[CallForm::FORM_KEY]
                            .as_str()
                            .expect("a form says which shape it is");
                        assert!(
                            FormKind::WIRE_WORDS.contains(&shape),
                            "`{shape}` is a form shape no `FormKind` spells, so a client decoding \
                             the published forms meets a word it cannot have",
                        );
                        form.get(CallForm::ARGS_KEY)
                            .and_then(Value::as_array)
                            .expect("a form answers its arguments")
                    })
                    .filter_map(|arg| {
                        let words = arg.get(ArgGrammar::ONE_OF_KEY)?.as_array()?;
                        Some(format!(
                            "{}={}",
                            arg[ArgGrammar::NAME_KEY].as_str().unwrap_or("<unnamed>"),
                            words
                                .iter()
                                .map(|word| word.as_str().unwrap_or("<not a word>"))
                                .collect::<Vec<_>>()
                                .join(","),
                        ))
                    })
                    .collect();
                // A vocabulary spelled the same way in two forms of one verb is ONE value space —
                // `swap_pane` publishes `dir` on one arm only, but a verb that came to publish it
                // on two would otherwise read here as a widening that never happened.
                spaces.sort_unstable();
                spaces.dedup();
                // ⚠ THE SURFACE'S TAG, with a pane's ID FOLDED to the placeholder the schema
                // itself uses for a parametric address (`cells.<offset>`). Otherwise this pin would
                // be about the FIXTURE's pane numbering — a second pane in the fixture would read
                // here as new vocabulary, which is a pin measuring the wrong thing.
                let surface = under.replace(&pane_container_tag(0), "pane_<id>");
                format!("{surface}/{action}:{}", spaces.join(" "))
            }));
        }
        // ⚠ A PANE'S SURFACE IS SERVED ONCE PER PANE, so a two-pane fixture would publish the pane
        // grammar twice — identical, and this pin is about the WORDS. Deduped rather than counted.
        served.sort_unstable();
        served.dedup();
        let mut pinned: Vec<String> = PINNED_WORDS.1.iter().map(|n| (*n).to_owned()).collect();
        served.sort_unstable();
        pinned.sort_unstable();
        assert_eq!(
            served, pinned,
            "A PUBLISHED VALUE SPACE MOVED. An agent enumerates these to build a call, so a word \
             ADDED gives every client written against the old list a gap it cannot know about, and \
             a word REMOVED or RENAMED breaks the calls it already builds. Update this pin, and \
             decide about sprag_rpc::WIRE_PROTOCOL: a widened space usually leaves the number \
             standing, a narrowed one does not.",
        );
        assert_eq!(
            PINNED_WORDS.0,
            sprag_rpc::WIRE_PROTOCOL,
            "THE PROTOCOL NUMBER MOVED WITH EVERY PUBLISHED VOCABULARY UNCHANGED — legitimate \
             when some other part of the wire moved, and a mistake when this pin was simply not \
             re-stamped.",
        );
    }

    /// **THE FOURTH PIN: a published argument's SHAPE cannot move under the protocol number.**
    ///
    /// # ⚠⚠⚠ The gap the three other pins each admit, and which R370 drove straight through
    ///
    /// The surface pin holds ADDRESSES, so an argument — which lives inside a form — is invisible
    /// to it, and its own doc says so. [`PINNED_WORDS`] holds the words a client picks a VALUE
    /// from, so an argument with no closed vocabulary is invisible to it too. `PINNED_VALUES` holds
    /// the ANSWER enums. Between them, **the TYPE of a request argument was held by nothing.**
    ///
    /// R370 changed `may_answer` from an object to a LIST of objects on five forms. No address
    /// moved, no name moved, no vocabulary moved — every client of the old shape breaks in both
    /// directions and the whole ratchet suite stayed green except for two hand-written COUNTS,
    /// which is folklore doing a pin's job.
    ///
    /// So this walks the served grammar and pins, per surface and verb, each form's shape word and
    /// each argument's `name:type`, its optionality, and the fields nested inside it. What it
    /// catches that nothing else can:
    ///
    /// * an argument RE-TYPED (`object` → `array`, `string` → `int`) — a break in both directions;
    /// * an argument that became OPTIONAL or REQUIRED — one direction breaks silently, which is
    ///   worse;
    /// * a nested field ADDED, REMOVED or re-typed inside a parent whose own name never moves.
    ///
    /// ⚠ It does NOT replace the value-space pin and deliberately does not carry `one_of`: a shape
    /// and a vocabulary fail differently — a widened vocabulary usually leaves the number standing
    /// (R342) and a changed shape never does — and one pin holding both would have to argue both
    /// cases in one message.
    ///
    /// ⚠ THROUGH THE DAEMON'S OWN SCENE, for [`PINNED_WORDS`]'s reason: a ratchet over the
    /// declaration is not a ratchet over the product (R320).
    #[test]
    fn a_published_argument_shape_cannot_move_under_the_protocol_number() {
        const PINNED_SHAPES: (u32, &[&str]) = (
            // R370: born at 29, in the round whose own change proved the gap. Every entry below is
            // the shape as it stands at that number; a later round that moves one of them decides
            // about `sprag_rpc::WIRE_PROTOCOL` here.
            // ⚠⚠⚠ R371 IS THE FIRST MOVE, AND IT IS THE ONE THIS PIN WAS BORN FOR: three forms
            // gained `await_person_ms:int?`, and this went red for it. R370's own re-typing had
            // been caught by nothing but two hand-written counts, which is what the pin exists to
            // replace. The number rises for the ADDED ARGUMENT on version 25's grounds — an older
            // daemon swallows an undeclared key and reports success for a run that will never wait.
            // ⚠ R372: re-stamped with every argument shape unchanged. `taken_over` is something a
            // run is TOLD, not something a caller asks for — it took no argument on any form,
            // deliberately: typing over somebody was never a behaviour a caller chose, so there was
            // nothing to opt into.
            // ⚠ R372: re-stamped with every argument SHAPE unchanged. The eleven families take the
            // arguments they always took; what changed is the answer when one is malformed.
            // ⚠⚠⚠ R373 IS THIS PIN'S SECOND MOVE AND ITS SECOND OF THE SAME KIND: the same three
            // LOOPING forms gained `handback_still_ms:int?`, and it went red by NAME before the
            // number was touched. Same grounds as R371's, and the silence is worse here: an older
            // daemon swallows the key and reports a run that ENDED ON THE FIRST KEYSTROKE using the
            // same word (`taken_over`) it would use for a run that waited and gave up. ⚠ The pair
            // is one request — `handback_still_ms` without `await_person_ms` is malformed — and NO
            // per-argument pin can see that, which is why it has a gate of its own
            // (`a_handback_for_a_run_nobody_is_watching_is_malformed`).
            // ⚠⚠⚠ AND A SECOND MOVE IN THE SAME ROUND, on a surface no round had touched in a
            // while: `hand:string?` on `key`, `text` and `paste`'s OBJECT forms. It is the
            // argument that made `taken_over` reachable in production at all — a display client
            // attaches over this socket, so its keystrokes came through the door stamped *a
            // program*. ⚠ The SCALAR spellings did not move, deliberately: a bare string has
            // nowhere to carry a second argument, and that is the right shape rather than a gap.
            // ⚠⚠⚠ R375: `done_when:string?` and `turn_within_ms:int?` on the `orchestrator` form —
            // AND THIS PIN CAUGHT THEM BY NAME BEFORE ANY NUMBER WAS TOUCHED, for the third round
            // running. That is what it was built for: an added argument is invisible to the
            // surface pin and to the value-space pins, and the only other thing that would have
            // noticed is a hand-written count.
            34,
            &[
                "sprag_workspace/pane_<id>/sprag_input/clipboard_answer[object]:seq:int sel:string text:string",
                "sprag_workspace/pane_<id>/sprag_input/focus[object]:focused:bool",
                "sprag_workspace/pane_<id>/sprag_input/key[object]:key:string state:string? ctrl:bool? alt:bool? shift:bool? super:bool? hand:string?",
                "sprag_workspace/pane_<id>/sprag_input/key[scalar]:key:string",
                "sprag_workspace/pane_<id>/sprag_input/mouse[object]:button:string kind:string col:int row:int ctrl:bool? alt:bool? shift:bool?",
                "sprag_workspace/pane_<id>/sprag_input/paste[object]:text:string hand:string?",
                "sprag_workspace/pane_<id>/sprag_input/paste[scalar]:text:string",
                "sprag_workspace/pane_<id>/sprag_input/text[object]:text:string hand:string?",
                "sprag_workspace/pane_<id>/sprag_input/text[scalar]:text:string",
                "sprag_workspace/sprag_mux/break_pane[object]:pane:int name:string? detached:bool? opened_by:int?",
                "sprag_workspace/sprag_mux/close[object]:id:int?",
                "sprag_workspace/sprag_mux/display_message[object]:text:string severity:string? client:string?",
                "sprag_workspace/sprag_mux/drop_file[object]:pane:int path:string",
                "sprag_workspace/sprag_mux/grant_pane[object]:pane:int share:int? memory:int? processes:int?",
                "sprag_workspace/sprag_mux/join_pane[object]:pane:int window:string",
                "sprag_workspace/sprag_mux/join_pane[object]:pane:int window_id:int",
                "sprag_workspace/sprag_mux/kill_session[object]:name:string",
                "sprag_workspace/sprag_mux/kill_window[object]:window:string?",
                "sprag_workspace/sprag_mux/kill_window[object]:window_id:int?",
                "sprag_workspace/sprag_mux/move_pane[object]:pane:int? target:int dir:string before:bool?",
                "sprag_workspace/sprag_mux/move_window[object]:window:string? after:string",
                "sprag_workspace/sprag_mux/move_window[object]:window:string? before:string",
                "sprag_workspace/sprag_mux/move_window[object]:window:string? place:string",
                "sprag_workspace/sprag_mux/new_session[object]:name:string? cmd:array? cwd:string? cols:int? rows:int?",
                "sprag_workspace/sprag_mux/new_window[object]:name:string? detached:bool? opened_by:int? cmd:array? cwd:string? cols:int? rows:int?",
                "sprag_workspace/sprag_mux/release_agent[object]:id:int",
                "sprag_workspace/sprag_mux/rename_pane[object]:pane:int name:string?",
                "sprag_workspace/sprag_mux/rename_session[object]:name:string",
                "sprag_workspace/sprag_mux/rename_window[object]:window:string? name:string",
                "sprag_workspace/sprag_mux/report_agent[object]:id:int source:string state:string name:string? seq:int? bind:bool?",
                "sprag_workspace/sprag_mux/resize[object]:id:int cols:int rows:int cell_width:int? cell_height:int?",
                "sprag_workspace/sprag_mux/resize_pane[object]:dir:string pane:int? cells:int?",
                "sprag_workspace/sprag_mux/resize_window[object]:window:string? adjust_cols:int? adjust_rows:int?",
                "sprag_workspace/sprag_mux/resize_window[object]:window:string? cols:int rows:int",
                "sprag_workspace/sprag_mux/resize_window[object]:window:string? from:string",
                "sprag_workspace/sprag_mux/select_pane[object]:dir:string from:int?",
                "sprag_workspace/sprag_mux/select_pane[object]:pane:int",
                "sprag_workspace/sprag_mux/select_window[object]:relative:string",
                "sprag_workspace/sprag_mux/select_window[object]:window:string",
                "sprag_workspace/sprag_mux/select_window[object]:window_id:int",
                "sprag_workspace/sprag_mux/set_floating[object]:id:int floating:bool",
                "sprag_workspace/sprag_mux/spawn[object]:cmd:array? cwd:string? cols:int? rows:int? name:string? opened_by:int?",
                "sprag_workspace/sprag_mux/split[object]:pane:int? dir:string before:bool? cmd:array? cwd:string? cols:int? rows:int? name:string? opened_by:int?",
                "sprag_workspace/sprag_mux/stop_job[object]:pane:int signal:string?",
                "sprag_workspace/sprag_mux/swap_pane[object]:pane:int? dir:string",
                "sprag_workspace/sprag_mux/swap_pane[object]:pane:int? with:int",
                "sprag_workspace/sprag_mux/zoom_pane[object]:pane:int? on:bool?",
                "sprag_workspace/sprag_plugins/cancel[object]:id:int",
                "sprag_workspace/sprag_plugins/run[object]:plugin:string endpoint_a:array endpoint_b:array seed:string label_a:string? label_b:string? format_a:string? format_b:string? cols:int? rows:int? timeout_ms:int? opened_by:int? guardrails:object?{max_iterations:int?,max_seconds:int?,max_tokens:int?}",
                // ⚠⚠⚠ THE FOUR ENTRIES THIS PIN WAS BORN FOR. `may_answer:array{…}` is a LIST of
                // clauses on the `answer` form (required — the consent IS the call) and
                // `may_answer:array?{…}` on the three that loop. At 28 all four read `object`, and
                // NOTHING in this suite could see them change.
                "sprag_workspace/sprag_plugins/run[object]:plugin:string pane:int may_answer:array{asked:string,answer:string} opened_by:int? guardrails:object?{max_iterations:int?,max_seconds:int?,max_bytes:int?}",
                // ⚠⚠⚠ R381: A WHOLE NEW FORM — the outer AI loop, as a run somebody can start. Its
                // four brief keys are the only REQUIRED strings on this surface that are not a
                // pane or a program: what the loop is for is not derivable from anything, and the
                // document ships `(edit me)` placeholders where they belong.
                //
                // ⚠⚠ **A FORM ADDED LEAVES THE NUMBER STANDING**, which is this pin's own escape
                // used deliberately: an older client is unaffected because it cannot select this
                // form — the `plugin` word that reaches it is one it has never heard of — and a
                // newer client that sends it to an older daemon is refused at the door by that
                // daemon's own `plugin` vocabulary. Neither half of a skewed pair can act on a
                // request it has misread, which is the failure a bump exists to prevent.
                //
                // ⚠⚠⚠ **AND THE ANSWERING CONTRACT ARRIVED ON IT WITHOUT COSTING THE NUMBER
                // EITHER, ON THAT SAME PROPERTY AND NOT ON THE GENERAL RULE.** An ADDED ARGUMENT is
                // this wire's second-commonest bump cause precisely because this surface SWALLOWS
                // an undeclared key and the run succeeds — a `may_answer` sent to a daemon that
                // does not read it is a run that answers nothing and reports `ok`. That cannot
                // happen HERE while no released daemon serves `ai_loop` at all: the whole form is
                // refused, key and all. ⚠ **THE RESIDUE, STATED: the day this form ships, an
                // argument added to it earns the number by the ordinary rule.**
                "sprag_workspace/sprag_plugins/run[object]:plugin:string pane:int north_star:string milestone:string reference:string max_turns:int reflect_every:int? agent:string ready_when:object?{match:string,marker:string} ready_timeout_ms:int? done_when:string? turn_within_ms:int? shows_prompt:bool? may_answer:array?{asked:string,answer:string} screen_rules:array?{when:string,text:string} await_person_ms:int? handback_still_ms:int? opened_by:int? guardrails:object?{max_iterations:int?,max_seconds:int?,max_bytes:int?}",
                // ⚠⚠⚠ AND THE PIN EARNED ITS KEEP ON THE VERY NEXT ROUND. R371 added
                // `await_person_ms:int?` to the three forms that LOOP, and this is what went red
                // for it — where R370's own re-typing had been noticed by nothing but two
                // hand-written counts. An argument ADDED is the case its doc predicts: a client
                // built against the old shape is unaffected, and an older DAEMON swallows the key
                // and reports a run that will never wait, which is why the number rises.
                "sprag_workspace/sprag_plugins/run[object]:plugin:string pane:int prompt:string eof:bool? shows_prompt:bool? timeout_ms:int? done_when:string? ready_when:object?{match:string,marker:string} ready_timeout_ms:int? may_answer:array?{asked:string,answer:string} await_person_ms:int? handback_still_ms:int? opened_by:int? guardrails:object?{max_iterations:int?,max_seconds:int?,max_bytes:int?}",
                "sprag_workspace/sprag_plugins/run[object]:plugin:string pane:int stimulus:string sentinel:string? done_when:string? turn_within_ms:int? ready_when:object?{match:string,marker:string} ready_timeout_ms:int? may_answer:array?{asked:string,answer:string} await_person_ms:int? handback_still_ms:int? opened_by:int? guardrails:object?{max_iterations:int?,max_seconds:int?,max_bytes:int?}",
                "sprag_workspace/sprag_plugins/run[object]:plugin:string src:int dst:int ready_when:object?{match:string,marker:string} ready_timeout_ms:int? may_answer:array?{asked:string,answer:string} await_person_ms:int? handback_still_ms:int? opened_by:int? guardrails:object?{max_iterations:int?,max_seconds:int?,max_bytes:int?}",
            ],
        );

        let serving: Vec<String> = served_fields()
            .into_iter()
            .filter(|field| field.path == ACTION_GRAMMAR_SLOT && field.answers)
            .map(|field| field.under)
            .collect();
        assert!(
            !serving.is_empty(),
            "the grammar slot is SERVED, or everything below is about a table nobody can read",
        );
        let mut served: Vec<String> = Vec::new();
        for under in &serving {
            let published = query_served_on(under, ACTION_GRAMMAR_SLOT).expect("the slot answers");
            // A pane's ID folded to the schema's own placeholder, so this pin is about the WIRE
            // and not about the fixture's pane numbering — [`PINNED_WORDS`]'s reason exactly.
            let surface = under.replace(&pane_container_tag(0), "pane_<id>");
            // ⚠⚠ THE RENDERER IS `sprag_conformance`'s, not a copy here. The GUI's three surfaces
            // serve this same slot and this crate's audit structurally cannot reach them, so the
            // one place both can call is where the shape spelling belongs.
            served.extend(
                sprag_conformance::published_shapes(&published)
                    .into_iter()
                    .map(|form| format!("{surface}/{form}")),
            );
        }
        assert!(
            !served.is_empty(),
            "the renderer answered nothing about a slot that plainly serves — a pin over an empty \
             list passes about nothing",
        );
        // A pane's surface is served once per pane, so a two-pane fixture publishes it twice —
        // identical, and this pin is about the SHAPES.
        served.sort_unstable();
        served.dedup();
        let mut pinned: Vec<String> = PINNED_SHAPES.1.iter().map(|n| (*n).to_owned()).collect();
        pinned.sort_unstable();
        assert_eq!(
            served, pinned,
            "A PUBLISHED ARGUMENT'S SHAPE MOVED. A client builds its call from these, so a TYPE \
             that changed breaks every caller of the old one IN BOTH DIRECTIONS, and an \
             optionality that changed breaks one of them SILENTLY. Update this pin, and raise \
             sprag_rpc::WIRE_PROTOCOL unless you can say why an older client is unaffected.",
        );
        assert_eq!(
            PINNED_SHAPES.0,
            sprag_rpc::WIRE_PROTOCOL,
            "THE PROTOCOL NUMBER MOVED WITH EVERY PUBLISHED ARGUMENT SHAPE UNCHANGED — legitimate \
             when some other part of the wire moved, and a mistake when this pin was simply not \
             re-stamped.",
        );
    }

    /// ⚠ And the pin above is only worth its words if an older decoder REALLY refuses. Measured.
    ///
    /// Stand-ins for the two enums as a build one commit older declares them — same serde
    /// attributes, one arm short. This is the claim `WIRE_PROTOCOL` 18 rests on, and it is the kind
    /// of claim this project has twice found to be false when it finally ran it: the surface pin's
    /// own docs record that reverting the number left the whole suite green.
    #[test]
    fn a_reader_of_the_previous_shape_cannot_parse_the_new_words() {
        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        #[allow(dead_code, reason = "only its parse is exercised")]
        enum CheckBefore {
            PaneIsolation,
            ControllerDelegation,
            CompetingWeight,
            CpuStall,
            IoStall,
            MemoryStall,
            Swapping,
            BuildSaturation,
            CcacheOnPath,
            CcacheSizing,
            FastLinker,
        }

        #[derive(Debug, serde::Deserialize)]
        #[serde(rename_all = "snake_case")]
        #[allow(dead_code, reason = "only its parse is exercised")]
        enum UnmeasuredBefore {
            NothingEnforced,
            NotPlaced,
            Gone,
        }

        // THE CONTROL FIRST: the old shape reads every word it always could, so a refusal below is
        // the new arm and not a broken stand-in.
        for (word, what) in [("pane-isolation", "a check"), ("cpu-stall", "another")] {
            assert!(
                serde_json::from_value::<CheckBefore>(json!(word)).is_ok(),
                "the stand-in still reads {what} it always read: {word}",
            );
        }
        assert!(serde_json::from_value::<UnmeasuredBefore>(json!("not_placed")).is_ok());

        let refused = serde_json::from_value::<CheckBefore>(json!("pane-admission")).unwrap_err();
        assert!(
            refused.to_string().contains("pane-admission"),
            "an older reader fails on the new check, naming it: {refused}",
        );
        let refused =
            serde_json::from_value::<UnmeasuredBefore>(json!({"refused": 13})).unwrap_err();
        assert!(
            refused.to_string().contains("refused"),
            "and on the new reason: {refused}",
        );

        // And THIS build reads both, which is the other half of a skew claim.
        assert_eq!(
            serde_json::from_value::<sprag_terminal::Check>(json!("pane-admission"))
                .expect("this build reads its own word"),
            sprag_terminal::Check::PaneAdmission,
        );
        assert_eq!(
            serde_json::from_value::<sprag_terminal::Unmeasured>(json!({"refused": 13}))
                .expect("this build reads its own word"),
            sprag_terminal::Unmeasured::Refused(sprag_terminal::Refusal::from_errno(13)),
        );
    }

    /// ⚠⚠ And the OTHER kind of break the number is owed for: one an older reader does not notice.
    ///
    /// `WIRE_PROTOCOL` 19's cause is not a new word — every key still parses — it is that `line`
    /// SAYS something else. Before R344 it was the retained row a match sat on; it is now the row
    /// that match's LOGICAL line begins on, and for a match past a soft wrap those are different
    /// numbers. A stand-in for the previous shape is the only way to state that, because the
    /// failure has no error: serde ignores the keys it does not know, hands back a perfectly formed
    /// value, and the client paints a row too high.
    ///
    /// The control is the ordinary match, where the two readings AGREE — which is why the version
    /// is the only thing that can catch this and why nothing in the suite noticed when the meaning
    /// moved.
    #[test]
    fn a_reader_of_the_previous_shape_misreads_a_match_past_a_wrap() {
        use crate::PaneMatch;

        /// `PaneMatch` as a build before R344 declared it — no `deny_unknown_fields`, exactly like
        /// the real one, so the new keys are silently dropped rather than refused.
        #[derive(Debug, serde::Deserialize)]
        struct PaneMatchBefore {
            line: usize,
            #[allow(dead_code, reason = "carried so the stand-in is the real shape")]
            col: u16,
            #[allow(dead_code, reason = "carried so the stand-in is the real shape")]
            cols: u16,
        }

        // A match on the CONTINUATION row of a line that began one row earlier.
        let past_a_wrap = serde_json::to_value(PaneMatch {
            line: 7,
            row: 8,
            col: 1,
            cols: 5,
            wrapped: Vec::new(),
        })
        .expect("a match serialises");

        let old: PaneMatchBefore = serde_json::from_value(past_a_wrap.clone())
            .expect("THE DANGER ITSELF: the old shape accepts the new answer without complaint");
        assert_eq!(
            old.line, 7,
            "and reads 7 — the row the LINE starts on — for a match whose cells are on row 8",
        );
        let now: PaneMatch =
            serde_json::from_value(past_a_wrap).expect("this build reads its own answer");
        assert_eq!(now.row, 8, "this build paints where the match actually is");

        // THE CONTROL: on a match that does not start past a wrap the two agree exactly, which is
        // why no existing pin, and no test in this suite, could see the meaning move.
        let ordinary = serde_json::to_value(PaneMatch {
            line: 7,
            row: 7,
            col: 1,
            cols: 5,
            wrapped: Vec::new(),
        })
        .expect("a match serialises");
        let old: PaneMatchBefore =
            serde_json::from_value(ordinary.clone()).expect("the old shape reads it");
        let now: PaneMatch = serde_json::from_value(ordinary).expect("and so does this one");
        assert_eq!(
            (old.line, now.row),
            (7, 7),
            "the readings agree for every match that was findable before R344",
        );

        // The absent-not-wrong half, stated so it is not confused with the cause above: a match
        // that DOES wrap omits nothing an old reader needs to parse, it simply tells it less.
        let wrapping = serde_json::to_value(PaneMatch {
            line: 2,
            row: 2,
            col: 18,
            cols: 2,
            wrapped: vec![3],
        })
        .expect("a match serialises");
        assert!(
            serde_json::from_value::<PaneMatchBefore>(wrapping).is_ok(),
            "`wrapped` on its own would not have owed a version — `line` did",
        );
    }

    /// **WHAT THE ROW-SHARE FACT COSTS A FRAME, in bytes** — R349's own answer to the standing debt
    /// that nothing in this suite asks what a READ costs.
    ///
    /// The fact is on the per-frame path: every client re-fetches every pane's frame on every wake,
    /// so a fact that is cheap per row is not automatically cheap. Measured on the pane size the
    /// project quotes everywhere: **107 bytes on a 5999-byte frame, 1.8%**. The bound below is a
    /// CEILING over that, so the encoding cannot quietly become an expensive one.
    ///
    /// The shape it forbids is a real alternative that was considered: one self-describing object
    /// per row (`{"row":0,"cells":80}` × 24) is roughly 600 bytes and would blow this bound. What
    /// is sent instead is two positional arrays — the same idiom as `row_generations` beside it —
    /// and the sparse half is EMPTY on a screen where nothing wrapped, which is the ordinary one.
    #[test]
    fn the_row_shares_cost_a_frame_a_bounded_number_of_bytes() {
        use sprag_vt::{Emulator, Palette, VtPort};

        let mut em = Emulator::new(80, 24);
        // Twenty lines of ordinary output, one of them long enough to wrap.
        for line in 0..20 {
            em.advance(format!("line {line}: the quick brown fox\r\n").as_bytes());
        }
        em.advance(&[b'x'; 100]);
        let screen = em.screen().clone();
        let palette = Palette::xterm_default();

        let with = crate::CellFrame {
            cells: sprag_grid::project(&screen, &palette),
            facts: crate::PaneScrollFacts::of(&screen, 0),
        };
        assert!(
            !with.facts.shares.continues.is_empty(),
            "the fixture must contain a wrap or this measures the empty case",
        );
        let without = crate::CellFrame {
            cells: sprag_grid::project(&screen, &palette),
            facts: crate::PaneScrollFacts {
                shares: sprag_grid::RowShares::default(),
                ..crate::PaneScrollFacts::of(&screen, 0)
            },
        };

        let whole = serde_json::to_string(&with).expect("a frame encodes").len();
        let bare = serde_json::to_string(&without)
            .expect("a frame encodes")
            .len();
        let cost = whole - bare;
        assert!(
            cost <= 160,
            "the row shares cost {cost} bytes of a {whole}-byte frame; a positional pair of \
             arrays for 24 rows is about a hundred, and a per-row object would be six times that",
        );
        // And the ordinary screen — nothing wrapped — pays for the sparse half not at all.
        let mut plain = Emulator::new(80, 24);
        plain.advance(b"hello\r\n");
        let quiet = crate::PaneScrollFacts::of(plain.screen(), 0);
        assert!(
            quiet.shares.continues.is_empty(),
            "nothing wrapped, so the sparse half is empty and encodes as `[]`",
        );
    }

    /// **THE KEY IS ADDITIVE, IN BOTH DIRECTIONS** — R342's rule, driven rather than asserted.
    ///
    /// An added answer KEY is absent-not-wrong to an older reader: the verb still works, the reply
    /// still parses, the client is simply told less. That is the property this whole fact rests on
    /// — it is why no `WIRE_PROTOCOL` bump is owed — and it is exactly the property no address pin
    /// and no shape pin can see, because neither of them decodes a payload the other end wrote.
    ///
    /// So both ends are stood up: a decoder of the PREVIOUS shape reads a frame carrying the new
    /// key, and today's decoder reads a payload without it and answers "cannot say", which every
    /// caller reads as "draw the pane as it stands".
    #[test]
    fn a_frame_carrying_the_row_shares_still_reads_on_a_decoder_that_has_never_heard_of_them() {
        /// `PaneScrollFacts` as it was BEFORE the shares — a stand-in for a peer built yesterday.
        #[derive(serde::Deserialize)]
        struct Older {
            scrollback_len: usize,
            visible_rows: u16,
        }

        let facts = crate::PaneScrollFacts {
            scrollback_len: 12,
            visible_rows: 24,
            shares: sprag_grid::RowShares {
                upto: vec![80, 3],
                continues: vec![0],
            },
        };
        let json = serde_json::to_string(&facts).expect("facts encode");
        let older: Older = serde_json::from_str(&json).expect(
            "an older peer must still read a frame that carries a key it has never heard of",
        );
        assert_eq!((older.scrollback_len, older.visible_rows), (12, 24));

        // ...and the other direction: a payload from a daemon that predates the fact.
        let ancient = r#"{"scrollback_len":3,"visible_rows":9}"#;
        let read: crate::PaneScrollFacts =
            serde_json::from_str(ancient).expect("today's decoder reads yesterday's payload");
        assert_eq!((read.scrollback_len, read.visible_rows), (3, 9));
        assert!(
            read.shares.is_empty(),
            "and says it cannot tell where the lines end, which is what makes a client draw \
             the pane un-wrapped rather than cut it in the wrong place",
        );
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
    /// ⚠ **R338 added `pane_resources.<max_age_ms>` and the number STOOD, which is this rule's
    /// first case measured rather than reasoned.** The register had priced that round as a protocol
    /// bump — *"per-pane CPU usage onto the wire (`WIRE_PROTOCOL` 17→18), a round of its own"* —
    /// and a new ADDRESS is additive by the rule three paragraphs up: an older daemon refuses it by
    /// name (`UnknownIntrospectPath`, which every client renders as skew), and an older client never
    /// asks. The pin is what turned that from an argument into a measurement.
    /// ⚠ **R344 moved the number with this list UNCHANGED, which is the case the assertion below
    /// names and the first time it happened.** A search match's `line` stopped meaning "the
    /// retained row this match is on" and started meaning "the retained row its LOGICAL line
    /// begins on" — same address, same key, same JSON type, different answer. No pin over
    /// ADDRESSES can see that, which is exactly why the version exists and why this constant
    /// carries the number beside the list rather than only the list.
    /// ⚠ **R353 moved the number AND the list, which is both cases at once.** The pane surface gained
    /// an `action_grammar` of its own — additive, an older client never asks — and in the same edit a
    /// published FORM stopped being an array of arguments and became `{form, args}`, so the mux
    /// slot's answer changed shape under a name that did not move. The added name would not have
    /// justified the bump; the changed value did.
    const PINNED_SURFACE: (u32, &[&str]) = (
        // R359b: the number moved for a VALUE THAT CHANGED SHAPE (`ready_when`, a string to an
        // object naming WHICH QUESTION its marker asks), with every ADDRESS unchanged — which is
        // what this re-stamp says and what this pin cannot see. R357 re-stamped it for a value
        // SPACE (`status` gained `interrupted`), also invisible here.
        // R364: re-stamped for an ADDED REQUEST ARGUMENT (`shows_prompt`), with every ADDRESS
        // unchanged. An argument lives inside a form this pin does not walk, which is exactly the
        // blind spot named above and the reason the argument grammar has ratchets of its own.
        // ⚠⚠ R364 AGAIN, and this time the ADDRESSES moved: TEN empty members
        // (`find.`, `cells.`, …) that both surfaces have ALWAYS ANSWERED and never declared. The
        // number stands, by this pin's own rule and by measurement: every one of them is a name
        // ADDED, an old client that never asks is unaffected, and one that does ask gets the
        // answer it got before pinion's declaration gate made an undeclared address unreachable.
        // ⚠⚠ R365: re-stamped for a MEANING that changed under names that did not — R344's case,
        // on the ANSWER side. `key`, `text` and `paste` answered `null` on every success and now
        // answer a caveat when the bytes they wrote MEANT a signal the pane will raise none. Same
        // three addresses, same request grammar, different answer; no pin over ADDRESSES can see
        // it, which is why the number lives beside the list rather than the list living alone.
        // R365 again: re-stamped for an ADDED REQUEST ARGUMENT (`done_when`), which lives inside a
        // form this pin does not walk — the blind spot named above, and the reason the argument
        // grammar has ratchets of its own.
        // R365 (third): re-stamped for a widened ANSWER value space, invisible here for the reason
        // stated above — an outcome word lives in a value, not at an address.
        // R366: re-stamped for an ADDED REQUEST ARGUMENT and two widened ANSWER value spaces (a
        // step's fifth `verdict` word, and `asking.why`'s six). Every one of those lives inside a
        // form or a value, and this pin walks ADDRESSES — the blind spot named above, and the
        // reason the argument grammar and the answer vocabularies have ratchets of their own.
        // R367: re-stamped for a MEANING that changed under a name that did not move — the message
        // above names exactly this case. The `panes` slot is the same address answering the same
        // shape, except that a `blocked` pane's `agent` object now carries `asking`, and its
        // ABSENCE there is a claim (this daemon read no menu) that an older daemon's silence does
        // not make. No address moved, so this pin could not have seen it.
        // ⚠⚠⚠ R370: re-stamped for a REQUEST VALUE THAT CHANGED SHAPE — `may_answer` went from an
        // object to a LIST of them on five forms. NOT ONE ADDRESS MOVED and not one NAME moved, so
        // this pin was blind to a change that breaks every client of the old shape in both
        // directions. That is the sharpest instance yet of the gap this const's own doc admits, and
        // R370 closed it rather than restating it: a fourth pin
        // (`a_published_argument_shape_cannot_move_under_the_protocol_number`) walks the served
        // grammar for each argument's TYPE, OPTIONALITY and NESTING, so the next re-typed argument
        // is caught by a gate instead of by whoever remembered.
        // R371: re-stamped for an ADDED REQUEST ARGUMENT (`await_person_ms`) and a widened ANSWER
        // value space (an eighth `why` word, `unattended`). Both live inside a form or a value, and
        // this pin walks ADDRESSES — the blind spot named above. ⚠ And it is now a NAMED blind spot
        // rather than an admitted one: the argument was caught by the shape pin and the word by the
        // answer-vocabulary pin, which is the arrangement R370 left behind.
        // ⚠ R372: re-stamped with the SURFACE unchanged. `taken_over` added no address and no
        // action — a run reports it through the outcome key it already had, on the forms it
        // already served. This pin walking addresses is why it is silent, and the
        // answer-vocabulary pin going red is why the number moved.
        // ⚠⚠ R372 AGAIN, AND THIS TIME THIS PIN IS THE ONLY ONE THAT SAW ANYTHING: `project.` is
        // ADDED, the eleventh family's empty member finally declared. An addition alone does not
        // move the number — what moved it is a behaviour NONE of these four can see, a `null` that
        // became a `-32602` on eleven families. See `WIRE_PROTOCOL`'s own entry for 32.
        // ⚠ R373: re-stamped with the SURFACE unchanged. `handback_still_ms` is an ARGUMENT inside
        // three forms the daemon already served at addresses that did not move — this pin's named
        // blind spot, covered by the shape pin, which went red by name before the number was
        // touched.
        // ⚠ R375: re-stamped with the SURFACE unchanged, for R373's reason exactly one round on.
        // `done_when` and `turn_within_ms` are ARGUMENTS inside a form served at an address that
        // did not move — this pin's named blind spot, covered by the shape pin, which again went
        // red by name first.
        34,
        &[
            // ⚠ TWICE, and not a duplicate: this list is the flat set of ADDRESSES the daemon serves
            // across every surface, and both the multiplexer and each pane's input surface answer a
            // slot of this name — each describing the verbs IT serves. A name appearing on two
            // surfaces appears twice here, which is what makes the count a count of addresses rather
            // than of words.
            "action_grammar",
            "action_grammar",
            // ⚠ AND A THIRD, the plugin host's — which this pin is how anybody would ever have
            // learned about, if the coverage gate had not named the surface first.
            "action_grammar",
            "agent_manifests",
            "application_cursor_keys",
            "break_pane",
            "cancel",
            "cells.",
            "cells.<offset>",
            "clients",
            "clipboard_answer",
            "clipboard_write",
            "close",
            "commands",
            "display_message",
            "doctor.",
            "doctor.<window_ms>",
            "drop_file",
            "events.",
            "events.<since>",
            "find.",
            "find.<needle>",
            "focus",
            "frames",
            "full_lines",
            "full_text",
            "grant_pane",
            "grid_work",
            // ADDED at R355 with the orchestration loop's door: the bound a `run` that names no
            // guardrails is given. A name added leaves every older client's requests working, so
            // WIRE_PROTOCOL stands — and this slot exists precisely so a client need not compile
            // the number in.
            "guardrail_defaults",
            "image_data.",
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
            "neighbors.",
            "neighbors.<pane>",
            "new_session",
            "new_window",
            "pane_processes.",
            "pane_processes.<max_age_ms>",
            "pane_resources.",
            "pane_resources.<max_age_ms>",
            "panes",
            "paste",
            "plugins",
            // ⚠ R372: the eleventh family's EMPTY member, ADDED. An addition leaves an older
            // client's requests working — nothing it used to send stops being served — so this name
            // does not by itself move the number. What moves it is the ANSWER a malformed member
            // now gets; see `PINNED_VALUES`.
            "project.",
            "project.<pane>",
            "prompt_marks",
            "regex.",
            "regex.<pattern>",
            "rename_pane",
            "rename_session",
            "release_agent",
            "rename_window",
            "resize",
            "report_agent",
            "resize_pane",
            "resize_window",
            "run",
            "runs",
            "select_pane",
            "select_window",
            "session",
            "session_activity.",
            "session_activity.<max_age_ms>",
            "sessions",
            "set_floating",
            "set_layout",
            "spawn",
            "split",
            "stop_job",
            "swap_pane",
            "text",
            "tree",
            "window_size",
            "windows",
            "zoom_pane",
        ],
    );

    /// One declared member of the surface the daemon SERVES, taken with the answer that surface
    /// gives when the address is QUERIED — the declaration and the behaviour, read in one pass so
    /// no test can compare a claim against a fixture built separately from it.
    struct ServedField {
        /// WHICH SURFACE served it — the tag chain of the containers it hangs under, joined with
        /// `/` (`sprag_mux`, `pane_1/sprag_input`).
        ///
        /// ⚠ Added at R353, when a name came to live on TWO surfaces: both the multiplexer and each
        /// pane's input surface serve an `action_grammar`, describing different verbs. Without this
        /// a ratchet reading "the" answer at that path would have silently read whichever one the
        /// walk reached first — the shape of a pin that pins the wrong thing and passes.
        under: String,
        /// The declared path, verbatim — a parametric family spells its placeholders.
        path: String,
        /// Which channel the declaration puts this path on: read, or invoke.
        channel: pinion_core::external::SchemaChannel,
        /// The declared arguments, empty for a scalar read that has said so.
        args: &'static [pinion_core::external::SchemaArg],
        /// Whether `query` at this exact path ANSWERED. Meaningful only for a path with no
        /// arguments: a parametric family's template is not an address any client sends.
        answers: bool,
        /// WHAT it answered, for a ratchet whose subject is the value rather than the declaration.
        answer: Option<serde_json::Value>,
    }

    /// Every address the DAEMON SERVES, read off the scene it assembles for a request — the whole
    /// point of the correction: a schema this module declares and a schema the daemon returns are
    /// two different facts, and only the second one is the wire.
    ///
    /// Walked through `External::introspect`, which is the same accessor `scene/query` resolves a
    /// path with, so a surface reachable by a client is a surface counted here.
    fn served_addresses() -> Vec<String> {
        served_fields()
            .into_iter()
            .map(|field| field.path)
            .collect()
    }

    /// The same walk, keeping each field's DECLARATION beside what its surface does — what
    /// [`served_addresses`] projects a path out of.
    fn served_fields() -> Vec<ServedField> {
        let mut found = Vec::new();
        walk(&served_scene(), "", &mut found);
        found
    }

    /// THE SCENE THIS DAEMON SERVES a request from, with one pane in it — the fixture every ratchet
    /// and the conformance audit read.
    ///
    /// Split out of [`served_fields`] when the audit moved into `sprag_conformance`, which takes the
    /// scene itself: one assembly, two readers, rather than a second fixture that could drift into
    /// describing a different daemon.
    fn served_scene() -> pinion_core::scene::Scene {
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
        crate::workspace_scene(
            &crate::SessionScope::unscoped(&registry),
            &registry,
            &std::sync::Arc::new(std::sync::Mutex::new(crate::runs::RunRegistry::default())),
            &std::sync::Arc::new(crate::notify::ChannelRegistry::default()),
            crate::DaemonShared::default(),
            crate::PaneCells::Omitted,
        )
    }

    /// Collect every external's declared fields, depth first — a container's children included,
    /// because the pane surfaces hang under one.
    ///
    /// Each field is QUERIED as it is collected, through the same handle that declared it, so the
    /// pair this returns is one surface answering about itself rather than two readings a fixture
    /// could have taken from different states.
    fn walk(scene: &pinion_core::scene::Scene, under: &str, found: &mut Vec<ServedField>) {
        use pinion_core::scene::Scene;
        let tagged = |tag: &Option<std::borrow::Cow<'static, str>>| match tag {
            Some(tag) if under.is_empty() => tag.to_string(),
            Some(tag) => format!("{under}/{tag}"),
            None => under.to_owned(),
        };
        match scene {
            Scene::External(node) => {
                if let Some(introspect) = node.handle.introspect() {
                    let under = tagged(&node.tag);
                    for field in introspect.schema().fields {
                        // ⚠⚠⚠ R372: `answers` IS NOW OWNERSHIP, AND THE NOTE THAT STOOD HERE SAID
                        // WHY IT COULD NOT BE. It read: *"they refuse only with `UnknownPath` … a
                        // ratchet that started reading the richer arms would be pinning refusal
                        // sentences this workspace has not derived yet."* This round derived them,
                        // so the collapse is no longer honest: the eleven parametric families now
                        // refuse their EMPTY member with `QueryTypeMismatch`, and reading that as
                        // *answers nothing* would report the whole read surface as broken for the
                        // change that made it correct.
                        //
                        // The claim this ratchet actually makes is *a declared read is not met with
                        // the same refusal a daemon too old to know the name would give* — and that
                        // refusal is `UnknownPath` alone. Every other arm names the address and
                        // tells the caller what is wrong with the CALL, which is the surface owning
                        // it.
                        let reached = introspect.query(field.path);
                        let owned = !matches!(
                            reached,
                            Err(pinion_core::external::ReadRefusal::UnknownPath)
                        );
                        let answered = reached.ok();
                        found.push(ServedField {
                            under: under.clone(),
                            path: field.path.to_owned(),
                            channel: field.channel,
                            args: field.args,
                            answers: owned,
                            // Only the JSON arm: every slot this ratchet reads answers a
                            // document, and a scalar arm coerced into one would let a gate about a
                            // structure pass over something that is not one.
                            answer: answered.and_then(|value| match value {
                                pinion_core::external::IntrospectValue::Json(json) => Some(json),
                                _ => None,
                            }),
                        });
                    }
                }
            }
            Scene::Container(node) => {
                let under = tagged(&node.tag);
                for child in &node.children {
                    walk(child, &under, found);
                }
            }
            _ => {}
        }
    }

    /// What the daemon ANSWERS at `path` on the surface hanging under `under`, off the scene it
    /// assembles for a request.
    ///
    /// The peer of [`served_fields`], and separate from it because a ratchet over an answer wants
    /// the answer rather than the declaration. Both go through `External::introspect`, which is
    /// what `scene/query` resolves with, so this is the value a client gets.
    ///
    /// ⚠⚠ **THE SURFACE IS A PARAMETER AND THE MATCH MUST BE UNIQUE.** This took a bare path until
    /// R353 and answered with the FIRST field that had it, which was safe only while every name was
    /// served once. Two surfaces publish `action_grammar` now, so a bare-path reader would have
    /// pinned one of them and reported nothing about the other — a ratchet whose subject is decided
    /// by scene order. An ambiguous ask panics rather than choosing.
    fn query_served_on(under: &str, path: &str) -> Option<serde_json::Value> {
        let mut found: Vec<ServedField> = served_fields()
            .into_iter()
            .filter(|field| field.path == path && field.under == under)
            .collect();
        assert!(
            found.len() <= 1,
            "{under} serves {} fields at `{path}`, so an answer read from it would be whichever one \
             the walk reached first",
            found.len(),
        );
        found.pop().and_then(|field| field.answer)
    }

    /// ⚠⚠ **A DECLARED PATH IS A CLAIM THAT A CLIENT CAN READ IT** — and the channel is where that
    /// claim is made.
    ///
    /// pinion's schema has two channels: [`SchemaChannel::Read`](pinion_core::external::SchemaChannel::Read)
    /// says `scene/query` answers here, [`Invoke`](pinion_core::external::SchemaChannel::Invoke)
    /// says the address is CALLED and probing it answers nothing. The distinction exists so a
    /// client auditing *"does every declared path answer?"* can skip the verbs **without being
    /// handed a list of names to maintain** — which is this project's own hand-written-list rule,
    /// upstream.
    ///
    /// This asserts sprag makes that claim truthfully in both directions, over the surface the
    /// daemon SERVES rather than over the constants this module declares. It is the read half of
    /// the pair: [`every_published_word_is_a_word_the_daemon_accepts`] is the write half.
    ///
    /// # What the two halves cost when they are wrong
    ///
    /// A verb mis-declared as readable sends an agent to `scene/query` for a name that will never
    /// answer, and the refusal it gets (`UnknownIntrospectPath`) is the same refusal a client
    /// meeting a DAEMON TOO OLD gets — so the surface's own mistake reads as version skew. A slot
    /// mis-declared as a verb is invisible in the other direction: nobody reads it at all.
    ///
    /// # Why parametric fields are counted and not queried
    ///
    /// `cells.<offset>` is a TEMPLATE, not an address — no client sends those angle brackets, so
    /// querying it verbatim would answer nothing and this gate would report the whole read surface
    /// as broken. They are asserted to be exactly the fields carrying arguments, and the count is
    /// reported, because a silent skip is the shape that reads as coverage.
    #[test]
    fn a_declared_read_answers_and_a_declared_verb_does_not() {
        use pinion_core::external::SchemaChannel;

        let served = served_fields();
        assert!(
            served.len() > 40,
            "the fixture serves the whole wire, not a corner of it: {}",
            served.len(),
        );

        let unreadable_reads: Vec<&str> = served
            .iter()
            .filter(|field| {
                field.channel == SchemaChannel::Read && field.args.is_empty() && !field.answers
            })
            .map(|field| field.path.as_str())
            .collect();
        assert_eq!(
            unreadable_reads,
            Vec::<&str>::new(),
            "THESE ADDRESSES ARE DECLARED ON THE READ CHANNEL AND ANSWER NOTHING. An agent \
             discovering the wire from its own schema queries each of them and is refused with \
             the same error a daemon too old to know the name would give. Declare a verb with \
             `SchemaField::action`/`action_with` so the channel says what it is.",
        );

        let readable_verbs: Vec<&str> = served
            .iter()
            .filter(|field| field.channel == SchemaChannel::Invoke && field.answers)
            .map(|field| field.path.as_str())
            .collect();
        assert_eq!(
            readable_verbs,
            Vec::<&str>::new(),
            "THESE ADDRESSES ARE DECLARED AS VERBS AND ALSO ANSWER A QUERY — one address serving \
             two channels, so what a client gets depends on which door it knocked on.",
        );

        // NOT SILENTLY SKIPPED: the fields this gate could not query are exactly the parametric
        // ones, and it says how many.
        let skipped: Vec<&str> = served
            .iter()
            .filter(|field| field.channel == SchemaChannel::Read && !field.args.is_empty())
            .map(|field| field.path.as_str())
            .collect();
        assert!(
            skipped.iter().all(|path| path.contains('<')),
            "a skipped read is a TEMPLATE, and a template spells its placeholder: {skipped:?}",
        );
        assert!(
            !skipped.is_empty(),
            "the fixture reaches the parametric families too, or the skip rule is about nothing",
        );
    }

    /// ⚠⚠ **EVERY PUBLISHED LINE-BREAK WORD ROUND-TRIPS, AND EACH NAMES ITS OWN ADDRESS.**
    ///
    /// Walks [`LineBreaks::ALL`] rather than the two words, so a third source of line breaks is
    /// covered the day it is declared — and it is the arm a hand-written check leaves out, because
    /// the DEFAULT path is what every existing caller drives and an explicitly-sent `screen` is
    /// what nothing does.
    ///
    /// The distinctness half is the one that matters: two words naming the same slot would publish
    /// a choice that changes nothing, and every caller who read the description and asked for the
    /// other answer would get the first with no error to tell them.
    #[test]
    fn every_published_line_break_word_round_trips_and_names_its_own_slot() {
        for kind in LineBreaks::ALL {
            assert_eq!(
                LineBreaks::from_wire(kind.wire_str()),
                Some(kind),
                "{kind:?} publishes a word its own parser does not take back",
            );
        }
        assert_eq!(
            LineBreaks::from_wire("sideways"),
            None,
            "and a word the vocabulary does not publish is refused rather than defaulted — a \
             default here would answer a question the caller did not ask",
        );

        let slots: std::collections::BTreeSet<&str> =
            LineBreaks::ALL.iter().map(|kind| kind.slot()).collect();
        assert_eq!(
            slots.len(),
            LineBreaks::ALL.len(),
            "each choice must name a DIFFERENT address, or the argument is a promise the surface \
             does not keep: {slots:?}",
        );
        assert!(
            slots
                .iter()
                .all(|slot| PANE_SCHEMA.iter().any(|field| field.path == *slot)),
            "and every address it names is one the pane surface actually DECLARES — an argument \
             pointing at an address nobody serves is a tool that fails only when somebody uses \
             it: {slots:?}",
        );
        assert_eq!(
            LineBreaks::default().slot(),
            FULL_TEXT_SLOT,
            "⚠ THE DEFAULT IS THE ANSWER EVERY EXISTING CALLER HAS ALWAYS HAD. A new default would \
             re-answer every call that was written before this argument existed",
        );
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
        // ...and no surface serves one address TWICE.
        //
        // ⚠⚠ **THIS CLAIM WAS STATED OVER THE FLAT LIST AND ITS PREMISE WAS FALSE.** It read "one
        // address, one surface" across the whole wire, defended as *"a client addressing a duplicated
        // name reaches whichever surface the dispatcher tries first"* — which is not how this wire is
        // addressed. A client sends a PATH: `/sprag_mux/external/action_grammar` and
        // `/pane_0/sprag_input/external/action_grammar` are two addresses that share a last segment,
        // and pinion resolves each to exactly one surface. R353 made the two real, so the false
        // premise had to be paid.
        //
        // RE-STATED rather than deleted (R351b's rule — deleting a gate whose product moved writes
        // the gate off): the ambiguity it was reaching for is real WITHIN a surface, where two fields
        // of one schema sharing a path would make `query` answer whichever the walk reached first.
        // That is now checkable, because the walk records which surface each field came from.
        for surface in SURFACES {
            let under = surface.name;
            let mut names: Vec<String> = served_fields()
                .into_iter()
                .filter(|field| field.under.ends_with(surface.tag))
                .map(|field| field.path)
                .collect();
            names.sort_unstable();
            let mut unique = names.clone();
            unique.dedup();
            assert_eq!(names, unique, "one address, one field, on {under}");
            assert!(
                !names.is_empty(),
                "{under} is a surface the walk reaches, or this loop is about nothing",
            );
        }
    }

    /// ⚠⚠ **EVERY VERB A SURFACE DECLARES PUBLISHES ITS GRAMMAR, OR IS A NAMED EXEMPTION** — the
    /// omission direction, over the scene this daemon SERVES.
    ///
    /// The claim itself is `sprag_conformance::every_verb_a_surface_declares_publishes_its_grammar`,
    /// because the GUI's window needs the same audit over its own scene and a second copy is what this
    /// whole feature refuses — each copy passes on its own scene. What stays here is the SCENE and the
    /// COUNT.
    ///
    /// ⚠ It is what found the plugin host: derived from what the daemon serves, it named a surface
    /// [`SURFACES`] did not, one hour after that list was hand-written (R353). The GUI's own run of it
    /// then found ninety more, of which sprag wrote none.
    #[test]
    fn every_verb_a_surface_declares_publishes_its_grammar() {
        assert_eq!(
            sprag_conformance::every_verb_a_surface_declares_publishes_its_grammar(
                &served_scene(),
                SURFACES,
            )
            .count_or_panic(),
            37,
            "the whole write half of this crate's wire: twenty-nine multiplexer verbs, a pane's six, \
             and the plugin host's two",
        );
    }

    /// ⚠⚠ **NO SURFACE OF THIS CRATE PUBLISHES A NESTED ARGUMENT THAT CANNOT BE FLATTENED** — over
    /// every surface, derived from [`SURFACES`] rather than named one at a time.
    ///
    /// A mouth built on the published grammar offers a nested field as a flag of its own
    /// (`--max-iterations`, never `--guardrails '{"max_iterations":5}'`), which is only sound while
    /// no field shares a name with a top-level argument of the same form. Today one surface has
    /// nesting and two have none, so this drives eight probes and would drive them on a mux verb
    /// the day `set_layout` publishes its tree — which is the point of walking the list instead of
    /// asserting about the plugin host alone.
    ///
    /// ⚠ The COUNT is what stops it from being vacuous: a claim over three surfaces with no nesting
    /// anywhere would pass while proving nothing, and this says how many nested fields it actually
    /// reached.
    #[test]
    fn no_surface_publishes_a_nested_argument_that_collides() {
        let driven: usize = SURFACES
            .iter()
            .map(|surface| {
                sprag_conformance::a_flattened_nested_argument_collides_with_nothing(
                    surface.grammar,
                )
                .count_or_panic()
            })
            .sum();
        assert_eq!(
            driven, 26,
            "the FLATTENED nested fields this crate's wire publishes: THREE guardrail fields on \
             each of the plugin host's SIX run forms, `ready_when`'s two on each of the four \
             that inject, and nothing on the multiplexer or a pane. \
             ⚠⚠ THE FIVE NEWEST ARE THE `ai_loop` FORM'S — its own guardrail three and its \
             barrier's two. A loop injects, so it takes the barrier every injecting form takes; \
             it spends BYTES, so its guardrail object is the byte-relay one. \
             ⚠⚠ THE CONSENT'S TWO ARE NO LONGER AMONG THEM, and the drop of eight is the point \
             rather than a regression: `may_answer` became a LIST of clauses (R370), and a list \
             CANNOT be flattened — N loose `asked`s beside N loose `answer`s say nothing about \
             which belongs with which — so both flattening mouths offer it whole and its fields \
             never become flags. A collision claim over fields nobody turns into flags would be \
             this gate asserting a property the product stopped needing. \
             ⚠ The MIRROR is still driven and is now the one that matters: `may_answer` is itself \
             a flag, so a field of some OTHER nested argument sharing that name is caught here \
             with the roles reversed",
        );
    }
}
