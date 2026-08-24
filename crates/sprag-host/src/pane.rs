//! The terminal pane as a pinion `External` — the engine side of the
//! R1.7 split.
//!
//! PINION-REQUIREMENTS R1.7 separates the pane into *data* and *engine*:
//! the cell grid is exposed as a `Scene::TextGrid` (introspectable
//! projection), while the PTY+emulator engine sits behind an `External`
//! boundary (process-side opacity justified). [`SpragPaneExternal`] is that
//! engine surface. It carries no scene state of its own — only a
//! [`PanePtyHandle`] onto the live producer — so input is a *producer*
//! mutation reached through pinion's canonical `scene/invoke`, not a
//! mutation of pinion's projection (R969: pinion projects, the producer
//! owns state).
//!
//! The action channel is the R2.6 input seam: `invoke("key", {key, …})`
//! encodes the W3C key + modifiers to PTY bytes ([`sprag_input::encode`],
//! sprag-owned) and writes them to the child. A sibling `invoke("text",
//! {text})` writes **literal** UTF-8 to the child (no key-encoding) — the seam
//! for IME-composed input (a Hangul/CJK commit is text, not a keystroke); the
//! AI peer drives the same wire. `invoke("paste", {text})` is the paste seam:
//! like `text`, but the host brackets it (`ESC [ 200 ~` … `ESC [ 201 ~`) when
//! the child enabled DEC private mode 2004, so a multi-line paste is held as
//! one paste, not executed line by line.
//!
//! The read channel serves the pane's cell FRAME as the `query("cells.<offset>")` family
//! ([`CELLS_FIELD`]) — the projected [`GridBuffer`] at that scrollback offset (serde-able
//! since PINION-PR49) plus the scroll facts (scrollback depth + visible rows) that ride with
//! it — the wire display client's per-frame read (topology B: the client reconstructs the
//! exact buffer the host projected and paints it, so "read data, not pixels" reaches the
//! human path). The argument RIDES THE PATH, which is what makes a frame read a read: this
//! was an `invoke` while sprag believed the path-only `scene/query` could not carry an
//! offset, and PINION-PR61 established it never could not (`width.<col>` and `id_at.<pos>`
//! were already doing it upstream). An invoke is a `MethodOcc::Mutate`, so that belief cost a
//! ~30Hz idle livelock and, after R152 half-fixed it, a wheel tick that woke every other
//! attached client. It also exposes the producer-owned input modes
//! (`query("application_cursor_keys")`) and the pane's full output text
//! (`query("full_text")`, scrollback + visible) — the same `Screen::full_text` the in-process
//! capture path reads, so an external peer and a plugin share one notion of the screen.

use std::fmt;

use pinion_core::GridBuffer;
use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError,
    ReadRefusal, read_only_or_unknown,
};
use serde_json::{Value, json};
use sprag_input::{KeyEdge, Modifiers, MouseButton, MouseEventKind, MouseInput};
use sprag_terminal::{Hand, PanePtyHandle, SignalKey};
use sprag_vt::{ClipboardTarget, Screen, osc52_reply};

use crate::external::{declined, encoded_member, refused, rpc_external_impl};
use crate::host::PaneScrollFacts;

/// The refusal every write to a pane's terminal shares: the bytes were formed and the child would
/// not take them.
///
/// A `const` because six actions state it and one situation deserves one sentence — the rule
/// [`crate::external::refused`] exists to keep. It is deliberately about the CHILD rather than
/// about the request: nothing the caller sends differently reaches a pane whose program has gone.
const NOT_WRITTEN: &str = "the pane's terminal would not take the write";

// The action names + query slots this external answers are the shared wire ABI
// vocabulary ([`crate::wire`]) — the SAME consts the wire client addresses, so the
// two cannot drift.
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::wire::{
    ACTION_GRAMMAR_SLOT, ALT_FIELD, ActionGrammar, CELLS_FIELD, CLIPBOARD_ANSWER_ACTION,
    CLIPBOARD_WRITE_SLOT, CTRL_FIELD, CURSOR_KEYS_SLOT, FIND_FIELD, FOCUS_ACTION, FRAMES_SLOT,
    FULL_LINES_SLOT, FULL_TEXT_SLOT, IMAGE_DATA_FIELD, INJECT_ACTION, INJECT_STROKES_KEY,
    INJECTED_BYTES_KEY, KEY_ACTION, KEY_FIELD, KEY_STATE_FIELD, LAST_COMMAND_SLOT, LINES_KEY,
    LINES_LOST_KEY, LINES_NEXT_KEY, LINES_PARTIAL_KEY, LINES_RESTARTED_KEY, LINES_SINCE_FIELD,
    LINKS_SLOT, MOUSE_ACTION, PANE_ECHO_SLOT, PANE_END_OF_INPUT_SLOT, PANE_EOF_SLOT,
    PANE_FOREGROUND_SLOT, PANE_GRAMMAR, PANE_HANDS_SLOT, PANE_RAW_OUTPUT_SLOT, PANE_REVISION_SLOT,
    PANE_SCHEMA, PASTE_ACTION, PEER_GONE_REFUSAL, PROMPT_MARKS_SLOT, RECENT_INPUT_FIELD,
    REGEX_FIELD, SCREEN_COLLAPSED_SLOT, SCREEN_ROWS_SLOT, SHIFT_FIELD, SUPER_FIELD, TEXT_ACTION,
};

/// Search `screen`'s retained output for the LITERAL `needle` — the one place the
/// `find.<needle>` language is bound to an engine call.
///
/// It is a function rather than two lines inlined at the slot because it has TWO callers with very
/// different cadences: the query slot (a find bar's keystroke) and the output-wait pass
/// (`crate::rpc`, once per coalesced burst of a watched pane's output). A round whose thesis is that
/// "does it say X" and "wait until it says X" are one semantics cannot afford two places deciding
/// which engine a language reaches — the shared [`crate::PaneFind`] makes the ANSWER one shape, and
/// this makes the QUESTION one mapping.
pub(crate) fn search_literal(screen: &Screen, needle: &str) -> crate::PaneFind {
    crate::PaneFind::from_screen_result(&screen.find(needle))
}

/// The same search read as a REGULAR EXPRESSION — [`search_literal`]'s peer, and separate for the
/// reason [`crate::wire::REGEX_FIELD`] gives: a needle and a pattern are separate languages.
///
/// A pattern the engine refuses answers the normal shape carrying its message rather than an
/// absence, because an invalid pattern is a well-formed question whose VALUE was rejected.
pub(crate) fn search_pattern(screen: &Screen, pattern: &str) -> crate::PaneFind {
    match screen.find_regex(pattern) {
        Ok(found) => crate::PaneFind::from_screen_result(&found),
        Err(bad) => crate::PaneFind::refused(&bad),
    }
}

/// Why a key never reached a pane's child — [`send_key`]'s refusal.
///
/// Two facts, and this type exists because they were ONE `bool` until R325. The two send a caller
/// somewhere completely different: an unencodable key is a request this build cannot express and a
/// failed write is a child that is gone, and a surface handed `false` had to name both or neither.
/// It is the same fusion the whole round removes one layer up, met inside sprag's own code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyUnsent {
    /// [`sprag_input::encode`] has no bytes for that key + modifier combination under the pane's
    /// live input modes — a name this build's encoder does not know.
    Unencodable,
    /// The bytes were encoded and the pane's terminal would not take them; its child has gone.
    NotWritten,
}

impl std::fmt::Display for KeyUnsent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // NAMES THE VOCABULARY, because the encoder is the only thing that knows it. The
            // `sprag` CLI used to write this sentence itself — it had to, since a payload-free
            // refusal could not carry one — and a client authoring a claim about another process's
            // encoder is what this round removes everywhere else too.
            Self::Unencodable => f.write_str(
                "not a key this build encodes — a key is a W3C key name (Enter, Escape, Tab, \
                 ArrowUp, F5) or a single character, optionally prefixed C- / M- / S-",
            ),
            Self::NotWritten => f.write_str("the pane's terminal would not take the keystroke"),
        }
    }
}

impl std::error::Error for KeyUnsent {}

/// Encode a W3C `key` + `mods` to PTY bytes (the sprag-owned R2.6 encoder,
/// [`sprag_input::encode`]) and write them to `pty`, answering WHICH way it failed.
///
/// This is the key->PTY SSOT shared by the RPC input surface
/// ([`SpragPaneExternal`]'s `key` action, which parses the JSON/scene wire) and the
/// in-process display client ([`HostClient::send_key`](crate::HostClient::send_key), which calls
/// this directly with typed args) — so the human keyboard path and the AI
/// `scene/invoke` path encode IDENTICALLY.
///
/// A [`KeyUnsent`] rather than `false`: see that type for why the two causes had to come apart. A
/// caller that only needs "did it go" writes `.is_ok()`, which is what the display clients do.
///
/// ⚠ Answers the BYTES it wrote, because the encoding is the only place they exist and a caller
/// that must say what the write MEANT cannot re-derive them: a W3C key plus modifiers becomes PTY
/// bytes through the pane's live input modes, so `Ctrl-C` is `0x03` only because this encoder said
/// so — which is what the caveat below is computed from.
///
/// # ⚠⚠⚠ `by` is the one thing the two paths must NOT share
///
/// The sentence above — *the human keyboard path and the AI `scene/invoke` path encode
/// IDENTICALLY* — is right about the BYTES and was wrong as a whole story, because it left the two
/// indistinguishable once written. That cost the product an entire question: *has a person taken
/// this pane?* ([`sprag_terminal::Hand`], measured). So the encoding stays shared and the HAND is
/// the caller's to declare: the display client says [`Hand::APerson`], the wire says
/// [`Hand::AProgram`].
pub fn send_key(
    pty: &PanePtyHandle,
    key: &str,
    mods: Modifiers,
    by: Hand,
) -> Result<Vec<u8>, KeyUnsent> {
    let bytes = sprag_input::encode(key, mods, pty.input_modes()).ok_or(KeyUnsent::Unencodable)?;
    pty.write(&bytes, by)
        .map(|_| bytes)
        .map_err(|_| KeyUnsent::NotWritten)
}

/// The caveat a pane-input action answers when `written` MEANT a signal this pane will not raise —
/// [`UNSIGNALLED_KEY`](crate::wire::UNSIGNALLED_KEY) — or `None` when there is nothing to say.
///
/// # ⚠⚠⚠ Why this is answered by the WRITE and not by a slot a caller reads first
///
/// The modes are the program's to change at any moment, so a caller that read them and then wrote
/// would be reporting a terminal that need not still exist. Answering it here closes that window:
/// the reading is taken for the bytes that were just delivered, which is the only moment the claim
/// is about.
///
/// # ⚠⚠ The two questions, and why BOTH are asked
///
/// What the caller MEANT is a fact about the caller — [`SignalKey::conventional_byte`] is what a
/// person pressing that chord produces and what every surface offering it means by it. What the
/// DEVICE does is read from the kernel. The whole value is that the two can disagree, so neither
/// can be inferred from the other.
///
/// ⚠ A pane that will not answer at all (`None` from the kernel) yields NO caveat, on
/// [`PaneSignalKeys`](sprag_terminal::PaneSignalKeys)' own terms: `None` is *this platform's device
/// would not say*, never the negative, and manufacturing a warning out of it would be the false
/// confidence in the other direction.
fn unsignalled(pty: &PanePtyHandle, written: &[u8]) -> Option<Value> {
    // The cheap gate first: ordinary typing contains no signal character, so the syscall below is
    // never reached on the path a display client walks a keystroke at a time.
    let meant: Vec<SignalKey> = SignalKey::ALL
        .into_iter()
        .filter(|key| written.contains(&key.conventional_byte()))
        .collect();
    if meant.is_empty() {
        return None;
    }
    let raises = pty.signal_keys()?;
    let unraised: Vec<Value> = meant
        .into_iter()
        .filter_map(|key| {
            let why = raises.unraised(key.conventional_byte())?;
            Some(json!({
                crate::wire::UNSIGNALLED_WHICH_KEY: key.wire_str(),
                crate::wire::UNSIGNALLED_WHY_KEY: why.wire_str(),
            }))
        })
        .collect();
    // ABSENT rather than empty: every signal the caller meant was raised, so there is nothing to
    // warn about and a caveat here would teach a reader to skip the key.
    (!unraised.is_empty()).then(|| json!({ crate::wire::UNSIGNALLED_KEY: unraised }))
}

/// [`unsignalled`] as this surface's ANSWER: the caveat, or [`IntrospectValue::Null`] when the
/// write has nothing to report — which is what every one of these actions answered before.
fn injected(pty: &PanePtyHandle, written: &[u8]) -> IntrospectValue {
    unsignalled(pty, written).map_or(IntrospectValue::Null, IntrospectValue::Json)
}

/// The [`INJECT_ACTION`] answer: [`INJECTED_BYTES_KEY`], plus the caveat every writing action
/// carries when what it wrote MEANT a signal this pane will raise none.
///
/// ⚠ ALWAYS an object, where [`injected`] answers `null` on the quiet path. The count is not a
/// caveat — it is the answer — so there is nothing for its absence to mean, and a driver charging
/// its run for what it typed reads one shape every time. The caveat keeps its own rule: absent
/// when there is nothing to say.
fn injected_batch(pty: &PanePtyHandle, written: &[u8]) -> IntrospectValue {
    let mut answer = serde_json::Map::new();
    answer.insert(INJECTED_BYTES_KEY.to_owned(), json!(written.len()));
    if let Some(Value::Object(caveat)) = unsignalled(pty, written) {
        answer.extend(caveat);
    }
    IntrospectValue::Json(Value::Object(answer))
}

/// Parse an [`INJECT_ACTION`] call's args into the strokes to write, in order.
///
/// # One keystroke vocabulary, read by one parser
///
/// Every element goes through [`parse_key_args`] — the same function the `key` action reads a
/// keystroke with — so a batch is not a second spelling of the keystroke form and cannot come to
/// admit a different one. A suppressed edge (a `state` of `up`) drops out of the batch, which is
/// what it does at the single-keystroke door too: accepted, and injecting nothing.
fn parse_inject_args(args: &IntrospectValue) -> Result<Vec<(String, Modifiers)>, InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    let Some(Value::Array(strokes)) = map.get(INJECT_STROKES_KEY) else {
        return Err(InvokeError::TypeMismatch);
    };
    strokes
        .iter()
        .filter_map(|stroke| parse_key_args(&IntrospectValue::Json(stroke.clone())).transpose())
        .collect()
}

/// Write literal UTF-8 `text` to `pty` (no key-encoding) — the IME-commit /
/// paste seam. Empty text is a no-op success. `true` on success; `false` on a
/// write failure. The text->PTY SSOT shared by [`SpragPaneExternal`]'s `text`
/// action and the in-process client. `by` is the caller's to declare — see [`send_key`].
#[must_use]
pub fn send_text(pty: &PanePtyHandle, text: &str, by: Hand) -> bool {
    if text.is_empty() {
        return true;
    }
    pty.write(text.as_bytes(), by).is_ok()
}

/// The bracketed-paste START marker (`ESC [ 200 ~`) — written before pasted text when the pane's
/// child has enabled DEC private mode 2004.
const PASTE_BRACKET_START: &str = "\x1b[200~";
/// The bracketed-paste END marker (`ESC [ 201 ~`) — written after pasted text.
const PASTE_BRACKET_END: &str = "\x1b[201~";

/// PASTE literal UTF-8 `text` into `pty` — the clipboard-paste seam, distinct from [`send_text`]
/// (typed / IME-committed text). When the pane's child has enabled bracketed paste (DEC private
/// mode 2004, read LIVE from the emulator), the text is wrapped in `ESC [ 200 ~` … `ESC [ 201 ~`
/// so the child can tell a paste from typed keystrokes (a shell / editor then holds a multi-line
/// paste instead of executing each line). Otherwise it is written raw, exactly like [`send_text`].
///
/// The bracketing decision lives HERE, at the PTY boundary, because the emulator holds the
/// authoritative mode — the same reason [`send_key`] encodes here rather than in the display
/// client (which never sees [`crate::HostClient`]-side input modes).
///
/// Security: any embedded END marker in the pasted text is stripped BEFORE wrapping, so a paste
/// whose content contains `ESC [ 201 ~` cannot close the bracket early and have its tail
/// interpreted as typed commands (the paste-injection guard xterm applies). Empty text is a
/// no-op success. `true` on success; `false` on a write failure.
#[must_use]
pub fn paste(pty: &PanePtyHandle, text: &str, by: Hand) -> bool {
    if text.is_empty() {
        return true;
    }
    if !pty.input_modes().bracketed_paste {
        return pty.write(text.as_bytes(), by).is_ok();
    }
    pty.write(&frame_bracketed_paste(text), by).is_ok()
}

/// REPORT a mouse event to `pty`'s child — the mouse-tracking seam, distinct from [`send_key`] /
/// [`send_text`]. The semantic event (a cell + a button edge) is gated against the pane's active
/// mouse-tracking mode and encoded to an X10 / SGR report by the sprag-owned encoder
/// ([`sprag_input::encode_mouse`], reading the LIVE modes from the emulator). Like [`send_key`], the
/// encoding lives HERE at the PTY boundary because the emulator holds the authoritative mode; a
/// display client supplies only the raw cell + button.
///
/// An event the active mode does not want (no tracking on, a motion outside any-event tracking, a
/// drag outside button/any-event tracking) is a no-op SUCCESS — it is silently dropped, not a
/// failure, exactly as an empty [`send_text`] is. `true` on a successful write or a legitimate drop;
/// `false` only on a write failure.
///
/// ⚠⚠ Written as [`Hand::AProgram`] EVEN WHEN A PERSON MOVED THE MOUSE, and this is a decision
/// rather than an oversight. A mouse report is not somebody's input travelling to the child — it is
/// SPRAG describing the environment to a program that asked to be told, and the person did not type
/// it. Counting it as a person's hand would fire *"somebody has taken this pane"* on a stray hover
/// across a pane a run is driving, which is the false positive that would make the whole signal
/// unusable.
#[must_use]
pub fn mouse(pty: &PanePtyHandle, event: MouseInput) -> bool {
    match sprag_input::encode_mouse(event, pty.input_modes()) {
        Some(bytes) => pty.write(&bytes, Hand::AProgram).is_ok(),
        None => true, // the mode did not want this event — a no-op, not a rejection
    }
}

/// Report a pane FOCUS change to its child, gated + encoded against the pane's live focus-reporting
/// mode ([`sprag_input::encode_focus`]): `ESC [ I` on focus gained, `ESC [ O` on focus lost, only
/// while the child has enabled DEC private mode 1004. The mode authority lives HERE at the PTY
/// boundary (like [`mouse`] / key encoding); a display client reports only the edge. A change the
/// mode does not want (1004 off) is a no-op SUCCESS, not a failure. `true` on a successful write or
/// a legitimate drop; `false` only on a write failure.
///
/// ⚠⚠ [`Hand::AProgram`], for [`mouse`]'s reason and more plainly still: a focus edge is raised by
/// the WINDOW SYSTEM, so there is no hand at all. A run whose pane merely gained focus has not been
/// taken by anybody.
#[must_use]
pub fn focus(pty: &PanePtyHandle, focused: bool) -> bool {
    match sprag_input::encode_focus(focused, pty.input_modes()) {
        Some(bytes) => pty.write(&bytes, Hand::AProgram).is_ok(),
        None => true, // focus reporting is off — a no-op, not a rejection
    }
}

/// Frame `text` as a bracketed paste: `ESC [ 200 ~` + `text` (with any embedded bracket marker
/// filtered out) + `ESC [ 201 ~`. Pure (no PTY) so the framing and the paste-injection guard are
/// deterministically testable. The END marker is filtered so a paste whose content contains
/// `ESC [ 201 ~` cannot close the bracket early and have its tail read as typed commands; the
/// START marker is filtered too (a forged start only confuses a child that already saw ours).
fn frame_bracketed_paste(text: &str) -> Vec<u8> {
    let sanitized = text
        .replace(PASTE_BRACKET_END, "")
        .replace(PASTE_BRACKET_START, "");
    let mut framed = Vec::with_capacity(PASTE_BRACKET_START.len() + sanitized.len() + 6);
    framed.extend_from_slice(PASTE_BRACKET_START.as_bytes());
    framed.extend_from_slice(sanitized.as_bytes());
    framed.extend_from_slice(PASTE_BRACKET_END.as_bytes());
    framed
}

/// The pane engine `External`: a thin, scene-stateless forwarder onto the
/// live [`PanePtyHandle`]. Input arrives via `scene/invoke` and is encoded
/// to PTY bytes by the sprag-owned encoder (R2.6); the producer's reader
/// thread lives behind this boundary, so the engine is `UiThreadSync` from
/// pinion's vantage (it does its work synchronously when invoked).
pub struct SpragPaneExternal {
    pty: PanePtyHandle,
}

impl SpragPaneExternal {
    /// Build the engine surface over a live pane's PTY I/O handle.
    #[must_use]
    pub fn new(pty: PanePtyHandle) -> Self {
        Self { pty }
    }

    /// Encode a `key` action's args and write the resulting bytes to the
    /// PTY. A `state:"up"` edge is a no-op success (terminals emit no
    /// release in this mode). An unencodable key or a write failure is an
    /// [`InvokeError::Rejected`].
    fn inject_key(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let Some((key, mods)) = parse_key_args(args)? else {
            return Ok(IntrospectValue::Null); // suppressed key-up edge
        };
        // ⚠⚠⚠ THE WIRE IS A PROGRAM UNLESS THE CALLER SAYS OTHERWISE, and the second half of that
        // sentence is what was missing. The first half is right and unchanged: a caller reaching
        // this surface is usually driving the pane through an API — a plugin, `sprag send-keys`, an
        // MCP tool — and stamping all of them as a person would make every scripted keystroke read
        // as *"somebody has taken this pane"*, stopping every supervised run the moment it worked.
        //
        // What the old form got wrong was the case it excluded by assumption: **a display client is
        // on the far side of this same socket.** `sprag_client::WireHost::send_key` — the key path
        // of both frontends — lands here, so `Hand::AProgram` unconditionally meant a person at a
        // real keyboard was recorded as a program and no run could ever see them. See [`Hand`],
        // which carried the false premise in its own doc.
        send_key(&self.pty, &key, mods, parse_hand(args)?)
            .map(|written| injected(&self.pty, &written))
            .map_err(refused)
    }

    /// Write a whole keystroke BATCH as one PTY write and answer what it wrote — [`INJECT_ACTION`],
    /// the door a run driver types through. See that constant for why it is a verb of its own.
    ///
    /// # ⚠⚠⚠⚠⚠ The refusal comes BEFORE the write, which is the whole of it
    ///
    /// Asked after the write this function returns the identical refusal to a caller and every gate
    /// over it stays green — and the bytes are already in a queue that will never drain, so the walk
    /// to the 16,896-byte wall is exactly as long as it was. That is not a hypothetical: it is the
    /// in-process door's own recorded mutation, and the reason its guard is the first statement in
    /// the function rather than a check on the way out.
    ///
    /// ⚠⚠ **AND IT IS THE DAEMON'S GUARD, NOT THE DRIVER'S.** A remote driver could ask
    /// [`PANE_EOF_SLOT`] itself and refuse before calling — and would then be deciding on a fact it
    /// read a round trip ago, about a child that can exit in between. The party that holds the
    /// atomic is the party that can answer it AT the write, so the refusal lives here and a driver
    /// maps the word back to its own typed error.
    ///
    /// ⚠ A batch with no strokes writes nothing and answers zero bytes rather than refusing: it is
    /// a well-formed request whose answer is *nothing was written*, which is what an empty list
    /// means. The suppressed key-up edges of a faithful client collapse to exactly that.
    fn inject_strokes(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let strokes = parse_inject_args(args)?;
        if self.pty.is_eof() {
            return Err(refused(PEER_GONE_REFUSAL));
        }
        // ONE WRITE, which is the second thing this door has that a loop over `key` has not: the
        // strokes are encoded under the modes read at this instant and handed to the terminal in a
        // single write, so a program reading its input takes the whole prompt in one read.
        let mut bytes = Vec::new();
        for (key, mods) in strokes {
            let encoded = sprag_input::encode(&key, mods, self.pty.input_modes())
                .ok_or_else(|| refused(KeyUnsent::Unencodable))?;
            bytes.extend_from_slice(&encoded);
        }
        // ⚠⚠ A PROGRAM, always — see [`INJECT_ACTION`]. There is no hand to parse.
        if bytes.is_empty() || self.pty.write(&bytes, Hand::AProgram).is_ok() {
            Ok(injected_batch(&self.pty, &bytes))
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Write a `text` action's literal UTF-8 to the PTY — **not** key-encoded.
    /// This is the seam for IME-composed input (a Hangul/CJK
    /// [`CompositionEvent::Commit`](pinion_core::CompositionEvent) is finished
    /// text, not a keystroke) and for pasting. Empty text is a no-op success
    /// (the IME's cancel-via-empty-commit shape). A write failure is an
    /// [`InvokeError::Rejected`].
    fn inject_text(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let text = parse_text_args(args)?;
        // ⚠ THE SAME QUESTION AS [`Self::inject_key`]'s, and it matters here for a reason of its
        // own: an IME commit is a person's word finished, and a display client sends it down this
        // door rather than the key one.
        if send_text(&self.pty, &text, parse_hand(args)?) {
            Ok(injected(&self.pty, text.as_bytes()))
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// PASTE a `paste` action's literal UTF-8 into the PTY — like [`Self::inject_text`], but the
    /// text is bracketed (and its embedded end marker filtered) when the child enabled DEC private
    /// mode 2004. The seam a display client's clipboard paste reaches over the wire. Empty text is
    /// a no-op success. A write failure is an [`InvokeError::Rejected`].
    fn inject_paste(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let text = parse_text_args(args)?;
        // ⚠ AND THE SAME AGAIN: somebody pressing paste at a display client is a person acting on
        // this pane, and a program relaying a buffer into it is not.
        if paste(&self.pty, &text, parse_hand(args)?) {
            // The BRACKETS are not consulted: they wrap the text, they do not shield it. The line
            // discipline sits below this device and raises its signals from the bytes whatever
            // markers surround them, which is exactly why a pasted `0x03` surprises people.
            Ok(injected(&self.pty, text.as_bytes()))
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Parse a `mouse` action's args into a [`MouseInput`] and report it to the PTY, gated + encoded
    /// against the pane's live mouse-tracking mode ([`mouse`]). An event the mode drops is a no-op
    /// success; a write failure is an [`InvokeError::Rejected`].
    fn inject_mouse(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let event = parse_mouse_args(args)?;
        if mouse(&self.pty, event) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Parse a `focus` action's args (`{focused: bool}`) and report the edge to the PTY, gated +
    /// encoded against the pane's live focus-reporting mode ([`focus`]). A change 1004 does not want
    /// is a no-op success; a write failure is an [`InvokeError::Rejected`].
    fn inject_focus(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let focused = parse_focus_args(args)?;
        if focus(&self.pty, focused) {
            Ok(IntrospectValue::Null)
        } else {
            Err(refused(NOT_WRITTEN))
        }
    }

    /// Answer a pending OSC 52 read query: format the `OSC 52` reply for the requested selection
    /// and hand it to the pane's exactly-once arbiter, which writes it to the PTY only if no
    /// client answered this query (or a newer one) first. Returns `{wrote}` — whether THIS answer
    /// reached the PTY — so a caller can tell it lost the race (harmless; the child got its
    /// reply). A write failure is an [`InvokeError::Rejected`].
    fn answer_clipboard(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let (seq, target, text) = parse_clipboard_answer_args(args)?;
        let reply = osc52_reply(target, &text);
        match self.pty.answer_clipboard_query(seq, &reply) {
            Ok(wrote) => Ok(IntrospectValue::Json(json!({ "wrote": wrote }))),
            Err(_) => Err(refused(NOT_WRITTEN)),
        }
    }

    /// The pane's cell FRAME at scrollback `offset` — the wire display client's
    /// per-frame read (topology B). A JSON-able struct:
    ///
    /// * `cells` — the projected [`GridBuffer`]
    ///   ([`sprag_grid::project_scrolled`], serde-able since PINION-PR49), the
    ///   paint-authoritative buffer the client reconstructs byte-for-byte;
    /// * `scrollback_len` — the retained history depth (the scrollbar extent + the
    ///   top-anchored offset math);
    /// * `visible_rows` — one scrollback page.
    ///
    /// The [`GridBuffer`] and the [`PaneScrollFacts`] are read under ONE screen lock —
    /// an atomically consistent snapshot (the cells and the scroll facts describe the
    /// SAME screen state, never a torn read across two locks). The facts flatten into
    /// the frame from the ONE [`PaneScrollFacts`] type (its field names ARE the wire
    /// keys), read through [`PaneScrollFacts::from_screen`] — the same population the
    /// in-process [`HostClient::pane_scroll_facts`](crate::HostClient::pane_scroll_facts)
    /// uses, so the two clients cannot disagree on the frame's non-cell shape.
    ///
    /// Served at every offset through the ONE [`CELLS_FIELD`] query family: `offset == 0` is
    /// the live view and a larger offset windows into history, self-clamping to the retained
    /// depth (so `0..=scrollback_len` are all answerable, and past the top gets the top).
    fn frame_at(&self, offset: usize) -> CellFrame {
        self.pty.with_screen_palette(|screen, palette| CellFrame {
            cells: sprag_grid::project_scrolled(screen, offset, palette),
            facts: PaneScrollFacts::of(screen, offset),
        })
    }
}

/// The wire frame the [`CELLS_FIELD`] family answers: the projected paint buffer plus the
/// non-cell [`PaneScrollFacts`] that ride with it, serialized as one flat JSON object
/// (`{cells, scrollback_len, visible_rows}`). `#[serde(flatten)]` pulls the facts'
/// field names up as the wire keys, so the frame's non-cell keys are defined ONCE
/// (on [`PaneScrollFacts`]) rather than re-listed here.
///
/// This is the ONE definition of the whole pane frame's wire shape — the envelope
/// (the `cells` key + its [`GridBuffer`] type) AND the flattened facts — with BOTH
/// `Serialize` (the host `query` end) and `Deserialize` (the wire client end,
/// `sprag-gui`'s `WireHost`). A single type owned by this crate, so a field rename
/// on either end is a compile error, not a silent runtime divergence (the exact
/// SSOT the R116 review established for the facts, here extended to the envelope).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CellFrame {
    /// The projected paint-authoritative cell buffer (serde-able since PINION-PR49), written in
    /// [`sprag_grid::wire`]'s run-length form rather than through its derived `Serialize`.
    ///
    /// The derived shape spells every cell in full — R221 measured it at **297 bytes per cell,
    /// 570,583 for one 80x24 pane** — and the reply's size is what the request's time is made of:
    /// building the `serde_json::Value` for it cost 2.2-3.0 ms of a ~4 ms fetch, because a DOM's
    /// cost is its node count. The attribute changes only how the field crosses the wire; the
    /// field is the same [`GridBuffer`] to every reader on both ends, so nothing downstream of
    /// here knows the difference.
    #[serde(with = "sprag_grid::wire")]
    pub cells: GridBuffer,
    /// The non-cell per-frame facts, flattened so `scrollback_len` / `visible_rows`
    /// are top-level wire keys (their names come from [`PaneScrollFacts`], the SSOT).
    #[serde(flatten)]
    pub facts: PaneScrollFacts,
}

impl fmt::Debug for SpragPaneExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `PanePtyHandle` wraps un-`Debug` PTY/emulator handles; the engine
        // is identified structurally (External: Debug is required by pinion §5.2).
        f.debug_struct("SpragPaneExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(SpragPaneExternal);

impl SpragPaneExternal {
    /// **THIS SURFACE'S PARAMETRIC FAMILIES** — `None` when `path` is not one of their prefixes,
    /// which is what keeps [`query`](Self::query)'s fallthrough to [`read`](Self::read) intact.
    ///
    /// # ⚠⚠⚠ Why they live OUTSIDE the reading chain now
    ///
    /// [`read`](Self::read) answers an `Option`, and a family has to answer three things: here is
    /// your member, your ARGUMENT is wrong, and I could not encode MY OWN reading. Squeezed into an
    /// `Option` the last two collapse onto one `Null` — see
    /// [`encoded_member`] for what that cost.
    ///
    /// Lifting them out rather than turning `read` into a `Result` is deliberate: a `None` from the
    /// reading chain still means *"not an address of mine"*, unchanged, and the surfaces that hang
    /// scope / dead-scope arms off exactly that meaning cannot be broken by an edit here.
    fn parametric(&self, path: &str) -> Option<Result<IntrospectValue, ReadRefusal>> {
        // Every frame read — live and history alike — is a READ, so no client can wake the waiter
        // it is parked on merely by looking (the R152 livelock, and the wheel-tick bump that
        // outlived it).
        if let Some(arg) = path.strip_prefix(CELLS_FIELD.literal_prefix()) {
            // Stripping the DECLARED prefix is what makes a path a MEMBER of the family — the same
            // question `SchemaField::addresses` answers. An argument that is not an offset is the
            // CALLER's to fix, and `QueryTypeMismatch` is pinion's word for it (R1667 made the
            // empty argument this surface's call rather than the matcher's, so `cells.` lands here
            // too).
            let Some(offset) = cells_offset(arg) else {
                return Some(Err(ReadRefusal::QueryTypeMismatch));
            };
            // ENCODED ONCE, not built as a DOM and re-encoded. `RawJson` (pinion R1480, delivering
            // PINION-PR79) carries JSON TEXT the producer already holds, and `scene/query` splices
            // those bytes into the reply instead of walking a tree — so a frame is serialized
            // exactly once, here. This is the answer R222 measured at 297 -> 5 B/cell.
            return Some(encoded_member(&self.frame_at(offset), "cell frame"));
        }
        // Every literal match of a needle in the pane's retained output, read ON DEMAND (a find
        // bar's keystroke, never per frame). A READ — searching a pane changes nothing about it, so
        // a client that re-queries as the user types cannot wake the waiters it is parked beside
        // (the R152 lesson `cells` was moved off an invoke for). The needle rides the path verbatim;
        // an EMPTY one is not a needle, which is the caller's to fix.
        // ⚠⚠⚠⚠⚠ WHAT THIS PANE HAS SAID SINCE A READER'S CURSOR — register item 557, and the read a
        // RELAY is. `full_lines` beside it answers the pane's whole history, so a reader following a
        // running program would re-read everything each step and could not tell what is new. This
        // one also carries the three facts a re-read cannot reconstruct: what was EVICTED unread,
        // what is still being WRITTEN, and whether the numbering the cursor came from still exists.
        //
        // ⚠⚠ The cursor is parsed with the SAME rule `cells.<offset>` uses (`007` is not `7`
        // spelled twice), so the two families refuse a malformed argument identically.
        if let Some(arg) = path.strip_prefix(LINES_SINCE_FIELD.literal_prefix()) {
            let Some(cursor) = cells_offset(arg) else {
                return Some(Err(ReadRefusal::QueryTypeMismatch));
            };
            let since = self
                .pty
                .with_screen(|screen| screen.lines_since(cursor as u64));
            return Some(Ok(IntrospectValue::Json(json!({
                LINES_KEY: since.lines,
                LINES_NEXT_KEY: since.next,
                LINES_LOST_KEY: since.lost,
                LINES_PARTIAL_KEY: since.partial,
                LINES_RESTARTED_KEY: since.restarted,
            }))));
        }
        // ⚠⚠⚠⚠⚠ WAS THIS NEEDLE WRITTEN INTO THIS PANE — register items 557 and 567, and the one
        // read on this surface that is NOT about the screen. A pseudoterminal echoes its input, so a
        // driver matching a marker against the grid cannot tell the program saying it from its own
        // keystroke coming back; `ReadyWhen::Prints` refuses a marker that is in this trail, and
        // that refusal is the difference between a barrier and a race.
        //
        // ⚠⚠⚠ It answers the QUESTION and never the trail. The trail holds input the terminal was
        // told not to echo — a password at a `sudo` prompt — which is nowhere on the grid, so
        // serving it here was the only way a read-only client could harvest one. The one production
        // consumer always asked about a marker it already held.
        //
        // ⚠⚠ The same `echo_trail` the in-process `PaneAccess` reads, deliberately: a second record
        // of what was typed would be a second answer to drift from, and this one decides whether a
        // run converges. It is the PANE's memory rather than any writer's, which is what makes it
        // answer about a display client's keystrokes too.
        if let Some(needle) = path.strip_prefix(RECENT_INPUT_FIELD.literal_prefix()) {
            if needle.is_empty() {
                return Some(Err(ReadRefusal::QueryTypeMismatch));
            }
            return Some(Ok(IntrospectValue::Bool(
                self.pty.echo_trail().contains(needle),
            )));
        }
        if let Some(needle) = path.strip_prefix(FIND_FIELD.literal_prefix()) {
            if needle.is_empty() {
                return Some(Err(ReadRefusal::QueryTypeMismatch));
            }
            let found = self
                .pty
                .with_screen(|screen| search_literal(screen, needle));
            // Serialized from the SHARED wire type, not a hand-built object: the client
            // deserializes that same type, so the keys are symmetric by construction.
            return Some(encoded_member(&found, "find result"));
        }
        // The same search over a REGULAR EXPRESSION — a separate address because a needle and a
        // pattern are separate languages, so one string cannot be allowed to mean both depending on
        // a mode carried somewhere other than the address (see `REGEX_FIELD`). An EMPTY pattern is
        // the caller's to fix exactly as an empty needle is; an INVALID one is not malformed
        // ADDRESSING but a rejected VALUE, so it answers the normal shape carrying the engine's
        // message — a refusal there would read as "no such address".
        if let Some(pattern) = path.strip_prefix(REGEX_FIELD.literal_prefix()) {
            if pattern.is_empty() {
                return Some(Err(ReadRefusal::QueryTypeMismatch));
            }
            let found = self
                .pty
                .with_screen(|screen| search_pattern(screen, pattern));
            return Some(encoded_member(&found, "regex result"));
        }
        // One inline image's RGBA as base64, fetched ON DEMAND (R1404 Stage 5) — the RGBA can be
        // megabytes, so it does not ride the per-poll panes slot (only the `{id,seq}` summary does).
        if let Some(arg) = path.strip_prefix(IMAGE_DATA_FIELD.literal_prefix()) {
            let Ok(id) = arg.parse::<u32>() else {
                return Some(Err(ReadRefusal::QueryTypeMismatch));
            };
            // ⚠ AND AN ID THE PANE IS NOT SHOWING STAYS `Null`, deliberately. That is
            // `NoSuchMember`'s case — *the argument is well typed and addresses nothing* — and it
            // is a per-path decision about what this surface knows rather than the shared rule
            // above. Moving it would mean authoring a sentence nobody derived; see
            // [`encoded_member`].
            return Some(Ok(self
                .pty
                .with_screen(|s| {
                    s.images()
                        .iter()
                        .find(|im| im.id == id)
                        .map(|im| STANDARD.encode(&im.rgba))
                })
                .map_or(IntrospectValue::Null, IntrospectValue::Text)));
        }
        None
    }

    /// The FIXED slots. Every parametric family left this chain — see
    /// [`parametric`](Self::parametric) — so a `None` here means *"not an address of mine"* and
    /// nothing else.
    fn read(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            // The count that bounds `cells.<offset>` (`IndexOf(FRAMES_SLOT)`): the live view
            // plus one per retained history line. An agent reads this scalar to learn where
            // history ends, instead of fetching whole cell grids to find out.
            FRAMES_SLOT => Some(IntrospectValue::Int(
                i64::try_from(self.pty.with_screen(Screen::scrollback_len)).unwrap_or(i64::MAX) + 1,
            )),
            CURSOR_KEYS_SLOT => Some(IntrospectValue::Bool(
                self.pty.input_modes().application_cursor_keys,
            )),
            FULL_TEXT_SLOT => Some(IntrospectValue::Text(
                self.pty.with_screen(Screen::full_text),
            )),
            // The same pane as the LOGICAL LINES the child wrote — the content answer, where
            // `full_text` is the rendered one. An array because a `\n` in a joined string cannot
            // say whether the program or the terminal put it there.
            FULL_LINES_SLOT => Some(IntrospectValue::Json(json!(
                self.pty.with_screen(Screen::full_lines)
            ))),
            // ⚠⚠⚠⚠⚠ THE VISIBLE SCREEN, the two ways a driver reads it — register item 544's stage
            // 1b. The two slots above are the pane's WHOLE output; these are the screen, and they
            // are TWO because neither derives from the other: `screen_rows` trims each row's
            // trailing blanks (what a person sees on that row) and `screen_collapsed` joins each
            // row's SHARE of its logical line (what the child printed, wrap taken back out).
            // Joining the trimmed rows — the derivation a remote client would write for itself —
            // drops the space a wrap sat on, and the width belongs to whichever display attached.
            //
            // ⚠⚠⚠ Both read the same `Screen` methods `PaneAccess::pane_collapsed` and
            // `pane_rows` read in-process, deliberately: a second join is a second answer to drift
            // from, and this one decides whether a marker matches.
            SCREEN_COLLAPSED_SLOT => Some(IntrospectValue::Text(
                self.pty.with_screen(Screen::collapsed_text),
            )),
            SCREEN_ROWS_SLOT => Some(IntrospectValue::Json(json!(
                self.pty.with_screen(Screen::row_texts)
            ))),
            // ⚠⚠⚠⚠⚠ WHETHER THIS PANE'S CHILD HAS EXITED — register item 544, and the read a
            // driver living OUTSIDE this process could not previously ask at all. The two slots
            // above say what the pane HOLDS; this says whether anything more is coming, which no
            // amount of reading the text can answer (a dead pane and a thinking one look the same).
            //
            // ⚠⚠⚠ It is the same atomic load `PaneAccess::pane_eof` does in-process, deliberately:
            // a second way of deciding *has the child gone* is a second answer to drift from, and
            // this one is what `ai_loop.scxml`'s `peer_gone` — and the 43-hour wedge behind it —
            // stands on. See `PANE_EOF_SLOT`.
            PANE_EOF_SLOT => Some(IntrospectValue::Bool(self.pty.is_eof())),
            // ⚠⚠⚠⚠⚠ HOW MANY TIMES THIS PANE HAS MOVED — register item 631, and the cheap half of
            // `pane/waitForRevision`. Every arm above renders a screen or asks the kernel; this one
            // is a lock take and an integer read, which is the whole point: a driver outside this
            // process used to pay a SCREEN over the wire to be told nothing had happened —
            // **96 times in a one-second wait, measured 2026-08-24, against 2 once it can park**.
            //
            // ⚠⚠⚠ THE SAME COUNTER THE PARK IS WOKEN BY, deliberately — `PaneRevision`, bumped on
            // the reader thread at the three moments a pane moves. A second count would let this
            // slot say *look* while the park says *nothing yet*, and the two would be right about
            // different things.
            PANE_REVISION_SLOT => Some(IntrospectValue::Json(json!(self.pty.revision().now()))),
            // ⚠⚠⚠⚠⚠ WHAT THIS PANE'S TERMINAL DOES WITH WHAT IS WRITTEN INTO IT — register item
            // 557. Read from the KERNEL at the moment of asking, never cached at the pane's birth:
            // both are the program's to change and every interactive agent changes them, so a value
            // taken at birth says the opposite of the truth for exactly the panes a loop drives.
            //
            // ⚠⚠ `Null` where the mode cannot be read, which is what the in-process reader answers
            // too — and it is NOT the other word. A driver told "the program owns the screen" on no
            // evidence would report a delivery confirmed by an echo it mistook for output.
            PANE_ECHO_SLOT => Some(self.pty.echo().map_or(IntrospectValue::Null, |echo| {
                IntrospectValue::Text(echo.wire_str().to_owned())
            })),
            // ⚠⚠⚠⚠ WHO OWNS THIS PANE'S TERMINAL — register item 557, through the same
            // `foreground_leader_of` the in-process reader calls. `Null` covers both honest
            // absences that function already has: nothing owns the terminal (the child exited, or
            // the leader was reaped while its group lives on), and no process table at all.
            //
            // ⚠⚠ NOT the sampler beside it. That answers about every pane and pays a full `/proc`
            // pass; a readiness barrier asks THIS about ONE pane every 10 ms, which is two
            // `stat`-sized reads because a process group's id is its leader's pid.
            PANE_FOREGROUND_SLOT => Some(
                self.pty
                    .pid()
                    .and_then(sprag_terminal::foreground_leader_of)
                    .and_then(|leader| serde_json::to_value(leader).ok())
                    .map_or(IntrospectValue::Null, IntrospectValue::Json),
            ),
            // ⚠⚠⚠⚠⚠ WHO HAS WRITTEN INTO THIS PANE — register item 653, off the SAME `Hands` the
            // in-process reader takes. Never `Null`: a pane this daemon holds always has an answer,
            // and zero is one. The absence a caller must be able to see is *there is no pane at
            // this path*, which the surface itself carries — a `Null` here would say *this pane has
            // no history of being written to*, which is the sentence a driver must never be handed
            // for a pane a person is typing into.
            //
            // ⚠⚠ THROUGH THE ONE BUILDER (`wire::hands_json`), whose reader sits directly below it.
            // The keys are `Hand`'s own published words, so the vocabulary a caller DECLARES a
            // write with and the vocabulary it READS the counts back under cannot drift apart.
            PANE_HANDS_SLOT => Some(IntrospectValue::Json(crate::wire::hands_json(
                self.pty.hands(),
            ))),
            // ⚠⚠⚠⚠⚠ WHAT THIS PANE'S CHILD WROTE — register item 656, off the SAME
            // `PanePtyHandle::raw_output` the in-process `PaneRawCapture` takes, so the two halves
            // of one product cannot come to hold different bytes. Never `Null`: a pane this daemon
            // holds always has a capture, and an empty one is an answer. The absence a caller must
            // be able to see is *there is no pane at this path*, which the surface itself carries.
            //
            // ⚠⚠ THROUGH THE ONE BUILDER (`wire::raw_output_json`), whose reader sits directly
            // below it — the bytes ride base64 because a source stream is not a JSON string.
            PANE_RAW_OUTPUT_SLOT => Some(IntrospectValue::Json(crate::wire::raw_output_json(
                &self.pty.raw_output(),
            ))),
            PANE_END_OF_INPUT_SLOT => Some(
                self.pty
                    .end_of_input()
                    .map_or(IntrospectValue::Null, |end| {
                        IntrospectValue::Text(end.wire_str().to_owned())
                    }),
            ),
            // The last command sliced from the OSC 133 marks (scrollback + visible). `Null`
            // (present-but-empty) when no command has run under shell integration — the
            // agent then falls back to `full_text`, exactly as a malformed `cells.<off>` is
            // `Null` rather than absent.
            LAST_COMMAND_SLOT => Some(self.pty.with_screen(Screen::last_command).map_or(
                IntrospectValue::Null,
                |cmd| {
                    IntrospectValue::Json(json!({
                        "command": cmd.command,
                        "output": cmd.output,
                        "exit_status": cmd.exit_status,
                        "running": cmd.running,
                    }))
                },
            )),
            // The OSC 133 prompt positions (logical indices from the oldest line) a
            // jump-to-prompt scrolls to — a JSON array, read on demand not per frame.
            PROMPT_MARKS_SLOT => Some(IntrospectValue::Json(json!(
                self.pty.with_screen(|screen| screen.prompt_positions())
            ))),
            // The OSC-8 hyperlink runs on the visible grid — a JSON array of {text, uri, id}, read
            // on demand by `read_pane_links`. `[]` when the pane shows no links. The link's URI as
            // data, which tmux's `capture-pane` cannot give (it flattens OSC 8 to plain text).
            LINKS_SLOT => Some(IntrospectValue::Json(json!(
                self.pty
                    .with_screen(Screen::hyperlink_runs)
                    .iter()
                    .map(|run| json!({ "text": run.text, "uri": run.uri, "id": run.id }))
                    .collect::<Vec<_>>()
            ))),
            // The pane's most recent OSC 52 clipboard WRITE, fetched ON DEMAND when the write seq
            // in the pane list grows (the payload can be a whole paste, so it is not carried per
            // poll). `Null` (present-but-empty) when the child has written no clipboard — the
            // client then has nothing to apply, exactly as a malformed `cells.<off>` is `Null`.
            // HOW TO CALL THIS SURFACE'S VERBS. Answered from the surface a client already holds
            // a path to, so the address it asked scopes the answer — see `ACTION_GRAMMAR_SLOT`. The
            // table is a `const`, so this costs one walk of it per ask and nothing per frame.
            ACTION_GRAMMAR_SLOT => Some(IntrospectValue::Json(ActionGrammar::answer(PANE_GRAMMAR))),
            CLIPBOARD_WRITE_SLOT => {
                let (write, seq) = self.pty.clipboard_write();
                Some(write.map_or(IntrospectValue::Null, |w| {
                    IntrospectValue::Json(json!({
                        "targets": {
                            "clipboard": w.targets.clipboard,
                            "primary": w.targets.primary,
                        },
                        "text": w.text,
                        "seq": seq,
                    }))
                }))
            }
            _ => None,
        }
    }
}

impl ExternalIntrospect for SpragPaneExternal {
    fn schema(&self) -> IntrospectSchema {
        // Declared in `wire`, beside the addresses — a field's TYPE is part of its
        // declaration, and this surface's vocabulary has ONE home.
        IntrospectSchema::new(PANE_SCHEMA)
    }

    /// ⚠⚠ **THE IDENTITY MIGRATION, and `UnknownPath` is what a `None` ALWAYS MEANT.**
    ///
    /// pinion R1674 widened a read's failure from an absence into a REFUSAL with a reason
    /// (`ReadRefusal`), and its dispatch maps `UnknownPath` onto the very fault a `None` produced
    /// before it (`QueryError::UnknownIntrospectPath`). So wrapping the reading below preserves
    /// this surface's wire behaviour exactly, which is what a pin bump owes its callers.
    ///
    /// ⚠⚠ **AND THE RICHER ARMS ARE NOW ADOPTED, for the parametric families.** `QueryTypeMismatch`
    /// and `Unavailable` split the one `Null` that used to carry both *your argument is wrong* and
    /// *this daemon could not encode its own reading* — see
    /// `crate::external::encoded_member`.
    ///
    /// ⚠ `NoSuchMember` is still not adopted: *"well typed, addresses nothing"* is a per-path
    /// decision about what this surface knows, and `image_data.<id>` keeps its `Null` for it.
    fn query(&self, path: &str) -> Result<IntrospectValue, ReadRefusal> {
        if let Some(answer) = self.parametric(path) {
            return answer;
        }
        self.read(path).ok_or(ReadRefusal::UnknownPath)
    }

    /// The reading itself — see [`query`](Self::query) for why it still answers an
    /// `Option` and what that `None` becomes.
    fn intervene(&mut self, path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // Nothing here is writable: input is an action (invoke `key`), and every slot is
        // producer-owned. But "not writable" and "not there" are different facts, and
        // saying the wrong one is pinion's §2 #7 lie — an agent told `UnknownPath` for
        // `full_text`, which it can plainly `query`, learns something false about the
        // surface. `read_only_or_unknown` answers from the SCHEMA (routing through
        // `SchemaField::addresses`, so `cells.0` reports read-only like any other member),
        // which is why the declaration belongs in one place. R1353 shipped this helper; the
        // first draft of R155 imported the vocabulary and kept the flat lie.
        Err(read_only_or_unknown(&self.schema(), path))
    }

    /// # A verb this surface does not DECLARE is a verb it does not run
    ///
    /// ⚠⚠ **This surface was the ODD ONE OUT.** R352 put [`declares_verb`](crate::wire::declares_verb)
    /// at the door of the mux surface and of the GUI's three, and left the pane's input surface
    /// dispatching six verbs without ever consulting [`PANE_SCHEMA`] — five surfaces guarding and one
    /// not, which is R330's rule pointing straight at it. Nothing was unreachable-but-dispatched here
    /// TODAY; what was missing is the property that it cannot become so, on the surface a keybinding
    /// drives in-process most often of all.
    ///
    /// The cost is one linear scan of a `&'static [SchemaField]` per action — paid per keystroke, not
    /// per frame, and the same scan is what makes a declared verb reachable at all, so an arm added
    /// here without a declaration is dead in the same edit.
    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        if !crate::wire::declares_verb(&self.schema(), path) {
            return Err(InvokeError::UnknownPath);
        }
        match path {
            KEY_ACTION => self.inject_key(&args),
            MOUSE_ACTION => self.inject_mouse(&args),
            FOCUS_ACTION => self.inject_focus(&args),
            TEXT_ACTION => self.inject_text(&args),
            PASTE_ACTION => self.inject_paste(&args),
            CLIPBOARD_ANSWER_ACTION => self.answer_clipboard(&args),
            INJECT_ACTION => self.inject_strokes(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// Parse the `clipboard_answer` action's params `{seq, sel, text}`: the query `seq` a client
/// saw, the selection char it read (`c` clipboard / `p` primary), and that selection's current
/// text. Only the object form is accepted (this is a machine wire, never hand-typed). A missing
/// or ill-typed field is an [`InvokeError::TypeMismatch`]; an empty `text` is valid (an empty
/// clipboard reads back as the empty string).
fn parse_clipboard_answer_args(
    args: &IntrospectValue,
) -> Result<(u64, ClipboardTarget, String), InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    let seq = map
        .get("seq")
        .and_then(Value::as_u64)
        .ok_or(InvokeError::TypeMismatch)?;
    // The selection's two words come from the type that owns them, which is what lets the pane
    // surface publish `sel`'s vocabulary instead of leaving a client to discover it from a reply.
    let target = map
        .get("sel")
        .and_then(Value::as_str)
        .and_then(ClipboardTarget::from_wire)
        .ok_or(InvokeError::TypeMismatch)?;
    let text = map
        .get("text")
        .and_then(Value::as_str)
        .ok_or(InvokeError::TypeMismatch)?
        .to_owned();
    Ok((seq, target, text))
}

/// Parse the `text` action's args into the literal string to write. Accepts a
/// bare string (`"한"`) or an object `{text: "한"}` (the AI/JSON wire). A
/// missing/non-string `text`, or a non-string/non-object arg, is an
/// [`InvokeError::TypeMismatch`]. Empty is allowed (the caller no-ops it).
/// Read the optional `hand` — **WHOSE KEYSTROKES THESE ARE**. Absent (or a scalar form, which has
/// nowhere to carry it) is [`Hand::AProgram`]: what this surface did before the argument existed,
/// and the conservative half.
///
/// # ⚠⚠⚠ Why the safe default is the one that cannot be claimed by accident
///
/// Reading a person's presence where there is none stops runs that should carry on, and reading a
/// program where a person is at the keyboard lets a run type over them. Both are wrong and only one
/// is reachable by silence — so silence means the program, and a caller who is genuinely carrying
/// somebody's keystrokes has to say so. An unauthenticated caller cannot pretend to be a person by
/// omission, which is the direction the mobile-transport work needs this to fail in.
///
/// ⚠ A word outside the vocabulary is MALFORMED, not a quiet default: `{"hand": "human"}` is a
/// caller who believes they said something.
fn parse_hand(args: &IntrospectValue) -> Result<Hand, InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Ok(Hand::AProgram);
    };
    if declined(map, Hand::WIRE_KEY) {
        return Ok(Hand::AProgram);
    }
    match &map[Hand::WIRE_KEY] {
        Value::String(word) => Hand::parse(word).ok_or(InvokeError::TypeMismatch),
        _ => Err(InvokeError::TypeMismatch),
    }
}

fn parse_text_args(args: &IntrospectValue) -> Result<String, InvokeError> {
    match args {
        IntrospectValue::Text(text) => Ok(text.clone()),
        IntrospectValue::Json(Value::Object(map)) => map
            .get("text")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or(InvokeError::TypeMismatch),
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// Parse a [`CELLS_FIELD`] member's `<offset>` argument — canonical non-negative decimal
/// only. `None` means the member is MALFORMED (the caller answers `Null`), never that the
/// path is unknown.
///
/// **Canonical, because `cells.7` / `cells.007` / `cells.+7` were three addresses for one
/// frame** — shipped by R155's first draft (a bare `parse::<usize>`, which accepts both) in
/// the same commit whose test doc says "a tolerated alias is how a split grows back". The
/// retired JSON-arg parser could not express either spelling (JSON has no `007`), so
/// consuming PR-61 had quietly INTRODUCED the aliases it was meant to remove. One frame, one
/// address.
///
/// An integer too large for `usize` SATURATES rather than being called malformed: it means
/// exactly what every other past-the-top offset means, and [`frame_at`]'s projection clamps
/// it to the top. Answering `2^64-1` with the top frame while calling `2^64` malformed would
/// split one concept across two answers for no reason a caller could predict.
///
/// [`frame_at`]: SpragPaneExternal::frame_at
fn cells_offset(arg: &str) -> Option<usize> {
    if arg.is_empty() || !arg.bytes().all(|b| b.is_ascii_digit()) {
        return None; // a sign, a space, or a non-digit: not this argument's type
    }
    if arg.len() > 1 && arg.starts_with('0') {
        return None; // `007` is `7` spelled a second way
    }
    Some(arg.parse::<usize>().unwrap_or(usize::MAX))
}

/// Parse the `key` action's args into a `(key, modifiers)` pair, or `None`
/// when the edge is a `state:"up"` release (which injects nothing).
///
/// Accepts either a bare string (`"a"` → that key, no modifiers, press) or
/// an object `{key, ctrl?, alt?, shift?, super?, state?}`. The `super` JSON
/// field maps to [`Modifiers::sup`]. Malformed args are an
/// [`InvokeError::TypeMismatch`].
fn parse_key_args(args: &IntrospectValue) -> Result<Option<(String, Modifiers)>, InvokeError> {
    match args {
        IntrospectValue::Text(key) if !key.is_empty() => {
            Ok(Some((key.clone(), Modifiers::default())))
        }
        IntrospectValue::Json(Value::Object(map)) => {
            let key = map
                .get(KEY_FIELD)
                .and_then(Value::as_str)
                .filter(|k| !k.is_empty())
                .ok_or(InvokeError::TypeMismatch)?;
            // THE EDGE IS A CLOSED SET, and it used to be two string literals here — the same place
            // `SplitDir`'s two words lived before R352b, with the same consequence: the vocabulary
            // had no definition the pane surface could publish. ⚠ A `state` PRESENT at the wrong
            // JSON type is refused rather than read as a press: `and_then(Value::as_str)` folded
            // `{"state": 4}` into the `None` arm, so a malformed edge was injected as a keystroke.
            let edge = if declined(map, KEY_STATE_FIELD) {
                KeyEdge::Down
            } else {
                match &map[KEY_STATE_FIELD] {
                    Value::String(word) => {
                        KeyEdge::from_wire(word).ok_or(InvokeError::TypeMismatch)?
                    }
                    _ => return Err(InvokeError::TypeMismatch),
                }
            };
            if !edge.injects() {
                return Ok(None);
            }
            Ok(Some((key.to_string(), parse_modifier_flags(map, true)?)))
        }
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// Parse the `mouse` action's args `{button, kind, col, row, ctrl?, alt?, shift?}` into a
/// [`MouseInput`]. Only the object form is accepted (a machine wire, never hand-typed). `button` is
/// `left`/`middle`/`right`/`wheelup`/`wheeldown`/`wheelleft`/`wheelright`/`none`, `kind` is
/// `press`/`release`/`drag`/
/// `motion`, and `col`/`row` are 0-based cells. A missing/ill-typed field is an
/// [`InvokeError::TypeMismatch`].
fn parse_mouse_args(args: &IntrospectValue) -> Result<MouseInput, InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    // THROUGH THE TYPE'S OWN VOCABULARY, both of them. These two matches used to be spelled out
    // here, opposite an identical pair in `sprag_client` — one vocabulary, two definitions, two
    // crates. `MouseButton::wire_str` records what that cost; what matters here is that the words
    // this admits are now the same array the pane surface PUBLISHES, so a client that reads the
    // grammar cannot be told a word this refuses.
    let word = |name: &str| map.get(name).and_then(Value::as_str);
    let button = word("button")
        .and_then(MouseButton::from_wire)
        .ok_or(InvokeError::TypeMismatch)?;
    let kind = word("kind")
        .and_then(MouseEventKind::from_wire)
        .ok_or(InvokeError::TypeMismatch)?;
    let coord = |name: &str| -> Result<u16, InvokeError> {
        map.get(name)
            .and_then(Value::as_u64)
            .and_then(|n| u16::try_from(n).ok())
            .ok_or(InvokeError::TypeMismatch)
    };
    Ok(MouseInput {
        button,
        kind,
        col: coord("col")?,
        row: coord("row")?,
        mods: parse_modifier_flags(map, false)?,
    })
}

/// The modifier flags of an input action's object form — `ctrl`, `alt`, `shift`, and `super` when
/// the action carries one — each ABSENT-or-`bool`.
///
/// # ⚠⚠ A MALFORMED FLAG USED TO BE READ AS `false`
///
/// Both parsers did `map.get(name).and_then(Value::as_bool).unwrap_or(false)`, which cannot tell a
/// key that is missing from one holding `"true"`, `1`, or `null` — so `{"key": "a", "ctrl": 1}` was
/// silently injected as an UNMODIFIED `a` and answered success. That is R352b's `report_agent`
/// defect exactly (a `name` dropped by `and_then(as_str)` where its siblings refused one), and
/// R330's odd-one-out rule catches it here too: `col` and `row` in the very same parser refuse a
/// malformed value.
///
/// Absent still means `false` — a call that says nothing about `ctrl` is not holding it, and that is
/// the shape every client sends. What is refused is a flag PRESENT at the wrong type, which no
/// well-formed caller sends and which the wire now advertises as a `bool`
/// ([`PANE_GRAMMAR`]). A declared argument the daemon
/// coerces instead of reading is what `a_declared_argument_is_one_the_daemon_reads` refuses to let
/// the grammar publish.
fn parse_modifier_flags(
    map: &serde_json::Map<String, Value>,
    with_super: bool,
) -> Result<Modifiers, InvokeError> {
    // ⚠ DECLINED means `false`, and declined includes an explicit `null`: a client whose language
    // serialises an absent optional that way is not holding the modifier, and refusing the call
    // would make every one of its keystrokes malformed. A flag present at any OTHER wrong type is
    // still refused — that is the coercion this parser exists to have stopped doing.
    let flag = |name: &str| -> Result<bool, InvokeError> {
        if declined(map, name) {
            return Ok(false);
        }
        match &map[name] {
            Value::Bool(held) => Ok(*held),
            _ => Err(InvokeError::TypeMismatch),
        }
    };
    Ok(Modifiers {
        ctrl: flag(CTRL_FIELD)?,
        alt: flag(ALT_FIELD)?,
        shift: flag(SHIFT_FIELD)?,
        // A mouse report has no encoding for the logo key, so that action does not read the key at
        // all — and a surface that does not read a key does not publish one either. Spelled as an
        // `if` rather than `with_super && flag(..)?` so it is visible that the flag is not READ
        // there, instead of being read and discarded.
        sup: if with_super {
            flag(SUPER_FIELD)?
        } else {
            false
        },
    })
}

/// Parse a `focus` action's args `{focused: bool}` into the focus edge. Only the object form is
/// accepted (a machine wire, never hand-typed); a missing/ill-typed `focused` is an
/// [`InvokeError::TypeMismatch`].
fn parse_focus_args(args: &IntrospectValue) -> Result<bool, InvokeError> {
    let IntrospectValue::Json(Value::Object(map)) = args else {
        return Err(InvokeError::TypeMismatch);
    };
    map.get("focused")
        .and_then(Value::as_bool)
        .ok_or(InvokeError::TypeMismatch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn json_args(v: serde_json::Value) -> IntrospectValue {
        IntrospectValue::Json(v)
    }

    /// A quiescent pane and the surface over it — a child that blocks on its PTY and never
    /// writes, so the frame these tests read cannot change under them.
    fn surface() -> (sprag_terminal::Workspace, SpragPaneExternal) {
        surface_running("exec cat")
    }

    /// [`surface`], but the pane runs `script` — for the gates that need a child which has
    /// CONFIGURED its terminal, which is the one thing a caller cannot see from the outside.
    fn surface_running(script: &str) -> (sprag_terminal::Workspace, SpragPaneExternal) {
        let mut workspace = sprag_terminal::Workspace::new((20, 4));
        let mut command = sprag_terminal::CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "dumb");
        let id = workspace
            .spawn(command, "cat".to_owned(), 20, 4)
            .expect("a pane spawns");
        let external = SpragPaneExternal::new(
            workspace
                .pane(id)
                .expect("the pane is there")
                .pty()
                .handle(),
        );
        (workspace, external)
    }

    /// Poll until `mark` is on the pane's screen, so a gate whose subject is the child's OWN
    /// `stty` cannot race the shell that runs it. Answers whether it ever arrived.
    fn until_printed(external: &SpragPaneExternal, mark: &str) -> bool {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(10) {
            if let Some(IntrospectValue::Text(screen)) =
                external.query(crate::wire::FULL_TEXT_SLOT).ok()
                && screen.contains(mark)
            {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        false
    }

    /// ⚠⚠⚠ **A `Ctrl-C` THAT CANNOT BECOME A SIGNAL IS ANSWERED AS ONE THAT DID.**
    ///
    /// This is the ai-loop's stop, and until this gate the surface could not tell the two apart.
    /// `send_keys` is the door an agent reaches for — its own description offers *"chords such as
    /// Ctrl+C"* — and the warning that a full-screen program has turned signals off is written on
    /// `stop_job`, **a tool the agent did not call**. So the caller writes `0x03` into a pane whose
    /// child ran `stty -isig`, is told the write succeeded, and waits for a job that was never
    /// asked to stop.
    ///
    /// The SUBJECT is the pane with `ISIG` off; the CONTROL is the same call into a pane that never
    /// touched its terminal, which must stay silent — a caveat on every keystroke would be noise a
    /// reader learns to skip, and then it is not a warning.
    #[test]
    fn a_key_that_cannot_become_a_signal_says_so() {
        let (_workspace, mut raw) = surface_running("stty -isig; printf RAW; exec cat");
        assert!(
            until_printed(&raw, "RAW"),
            "the child announces AFTER its `stty`, so the reading below cannot race it",
        );
        let said = raw
            .invoke(KEY_ACTION, json_args(json!({"key": "c", "ctrl": true})))
            .expect("the byte is still written — a person's Ctrl-C must reach a raw program");

        let (_control_workspace, mut cooked) = surface_running("printf COOKED; exec cat");
        assert!(until_printed(&cooked, "COOKED"), "the control pane starts");
        let quiet = cooked
            .invoke(KEY_ACTION, json_args(json!({"key": "c", "ctrl": true})))
            .expect("the control's write succeeds too");

        assert_eq!(
            quiet,
            IntrospectValue::Null,
            "a pane whose terminal DOES raise the signal has nothing to report — a caveat on \
             every keystroke is noise, and a reader who learns to skip it is not warned by it",
        );
        assert_ne!(
            said, quiet,
            "⚠⚠⚠ the two panes answer the SAME sentence for opposite outcomes: one interrupted \
             its job and one wrote a byte a program will read as text. `stop_job` knows the \
             difference and says so; the verb an agent actually calls does not.",
        );

        // WHICH key and WHY — a caveat that only said "something" would leave a caller to guess
        // whether to retry, reconfigure, or reach for `stop_job`.
        let IntrospectValue::Json(said) = said else {
            panic!("the caveat is a JSON answer: {said:?}");
        };
        assert_eq!(
            said,
            json!({
                crate::wire::UNSIGNALLED_KEY: [{
                    crate::wire::UNSIGNALLED_WHICH_KEY: SignalKey::Interrupt.wire_str(),
                    crate::wire::UNSIGNALLED_WHY_KEY:
                        sprag_terminal::Unraised::TerminalRaisesNone.wire_str(),
                }]
            }),
            "the answer names the key the caller MEANT and the state of the pane that swallowed \
             it — `raw` is the program having taken its terminal, which is a different act from a \
             rebound character and is retried differently",
        );
    }

    /// ⚠⚠ **THE SAME CAVEAT ON THE VERB THAT TYPES**, because a `0x03` reaches a pane as literal
    /// text at least as often as it reaches one as a key: `write_pane` is how an agent drives a
    /// pane, and the byte is the byte whichever door it came through.
    ///
    /// The CONTROL is ordinary text through the same verb into the same pane — a caveat there
    /// would fire on every command an agent ever types, and the syscall that reads the terminal
    /// must not be on that path either.
    #[test]
    fn typing_a_signal_character_is_answered_the_same_way_a_key_is() {
        let (_workspace, mut raw) = surface_running("stty -isig; printf RAW; exec cat");
        assert!(
            until_printed(&raw, "RAW"),
            "the child announces after its `stty`"
        );

        let ordinary = raw
            .invoke(TEXT_ACTION, json_args(json!({"text": "ls -al\n"})))
            .expect("ordinary text is written");
        assert_eq!(
            ordinary,
            IntrospectValue::Null,
            "text that means no signal reports nothing — this is the path EVERY typed command \
             walks, and a caveat on it would be noise on every call an agent makes",
        );

        let interrupt = String::from(char::from(SignalKey::Interrupt.conventional_byte()));
        let said = raw
            .invoke(TEXT_ACTION, json_args(json!({"text": interrupt})))
            .expect("the byte is still written");
        assert_eq!(
            said,
            IntrospectValue::Json(json!({
                crate::wire::UNSIGNALLED_KEY: [{
                    crate::wire::UNSIGNALLED_WHICH_KEY: SignalKey::Interrupt.wire_str(),
                    crate::wire::UNSIGNALLED_WHY_KEY:
                        sprag_terminal::Unraised::TerminalRaisesNone.wire_str(),
                }]
            })),
            "⚠⚠ a caller that wrote the interrupt character as TEXT is in exactly the position \
             the key path was, and answering only one of the two doors would make the warning a \
             property of the spelling instead of a property of the pane",
        );
    }

    /// THE ENCODING ITSELF — the one claim no other test in this crate can see.
    ///
    /// The consumption of PINION-PR79 deliberately changes NOTHING observable: a `Raw` answer
    /// and the `Json` answer it replaced reach the wire as the same bytes in the same order, so
    /// every test that reads a decoded frame stays green whether the change is present or
    /// reverted. That makes the usual "assert the behaviour" discipline blind here — the claim
    /// is "the frame is serialized ONCE, not built as a tree and encoded again", and the only
    /// witness to it is the VARIANT the surface answers with.
    ///
    /// The three parametric reads are covered together because they share one reason to exist:
    /// each answers a whole structure the producer has already built, and each was a
    /// `to_value(..).map_or(Null, Json)` before.
    #[test]
    fn a_panes_structural_answers_are_encoded_text_not_a_dom() {
        let (_workspace, external) = surface();
        for path in ["cells.0", "find.a", "regex.a"] {
            let answer = external.query(path).expect("the surface owns the address");
            assert!(
                answer.as_raw().is_some(),
                "`{path}` still builds a serde_json::Value DOM for the dispatch to re-encode; \
                 the answer was {answer:?}",
            );
        }
    }

    /// ⚠⚠⚠ **THIS SURFACE'S EMPTY MEMBERS ARE DECLARED** — all four of its families, against the
    /// live surface. See [`crate::wire::assert_empty_members_are_declared`] for what it costs when
    /// they are not.
    #[test]
    fn every_empty_member_this_pane_answers_is_one_it_declares() {
        let (_workspace, external) = surface();
        crate::wire::assert_empty_members_are_declared(
            crate::wire::PANE_SCHEMA,
            "the pane surface",
            // OWNERSHIP, not a value — `QueryTypeMismatch` is this surface owning the address and
            // naming what is wrong with the argument. See the helper's own doc.
            |path| !matches!(external.query(path), Err(ReadRefusal::UnknownPath)),
        );
    }

    /// The other half of the same claim: encoded-once must not mean encoded-DIFFERENTLY.
    ///
    /// A `RawJson` carries text the producer wrote, and nothing downstream re-checks it against
    /// the type it came from — so the guard that the bytes still say what the DOM path said has
    /// to be here. Parsed back, the answer is exactly `to_value` of the frame the SAME surface
    /// produces, rather than of a frame this test built a second way.
    #[test]
    fn an_encoded_answer_carries_the_document_the_dom_would_have() {
        let (_workspace, external) = surface();
        let answer = external.query("cells.0").expect("the surface answers");
        let encoded = answer.as_raw().expect("an encoded answer");
        assert_eq!(
            encoded.to_value().expect("the text is valid JSON"),
            serde_json::to_value(external.frame_at(0)).expect("the frame serialises"),
            "the spliced text and the DOM path describe the same frame",
        );
    }

    #[test]
    fn frame_bracketed_paste_wraps_multiline_text() {
        // The whole multi-line paste sits INSIDE one bracket pair, so a child holds it as one
        // paste instead of executing each line — the reason bracketed paste exists.
        assert_eq!(
            frame_bracketed_paste("git status\nls -a"),
            b"\x1b[200~git status\nls -a\x1b[201~",
        );
    }

    #[test]
    fn frame_bracketed_paste_strips_an_embedded_end_marker() {
        // Paste-injection guard: a payload carrying the END marker must NOT be able to close the
        // bracket early and have its tail (`rm -rf /`) read as typed input.
        let framed = frame_bracketed_paste("safe\x1b[201~rm -rf /");
        assert_eq!(framed, b"\x1b[200~saferm -rf /\x1b[201~".to_vec());
        // Exactly ONE end marker survives — the framing's own trailer.
        let end = b"\x1b[201~";
        let count = framed.windows(end.len()).filter(|w| *w == end).count();
        assert_eq!(count, 1, "no forged end marker survives the sanitize");
    }

    #[test]
    fn frame_bracketed_paste_strips_an_embedded_start_marker() {
        assert_eq!(
            frame_bracketed_paste("a\x1b[200~b"),
            b"\x1b[200~ab\x1b[201~".to_vec(),
        );
    }

    #[test]
    fn parses_bare_string_key() {
        let parsed = parse_key_args(&IntrospectValue::Text("a".to_string())).unwrap();
        assert_eq!(parsed, Some(("a".to_string(), Modifiers::default())));
    }

    #[test]
    fn parses_object_with_modifiers() {
        let parsed = parse_key_args(&json_args(json!({"key": "c", "ctrl": true}))).unwrap();
        assert_eq!(
            parsed,
            Some((
                "c".to_string(),
                Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                }
            ))
        );
    }

    #[test]
    fn super_field_maps_to_sup() {
        let parsed = parse_key_args(&json_args(json!({"key": "x", "super": true}))).unwrap();
        assert_eq!(
            parsed.unwrap().1,
            Modifiers {
                sup: true,
                ..Modifiers::default()
            }
        );
    }

    #[test]
    fn key_up_edge_is_suppressed() {
        let parsed = parse_key_args(&json_args(json!({"key": "a", "state": "up"}))).unwrap();
        assert_eq!(parsed, None);
    }

    #[test]
    fn missing_or_empty_key_is_type_mismatch() {
        assert_eq!(
            parse_key_args(&json_args(json!({}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_key_args(&json_args(json!({"key": ""}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_key_args(&IntrospectValue::Int(1)),
            Err(InvokeError::TypeMismatch)
        );
    }

    #[test]
    fn unknown_state_is_type_mismatch() {
        assert_eq!(
            parse_key_args(&json_args(json!({"key": "a", "state": "sideways"}))),
            Err(InvokeError::TypeMismatch),
        );
    }

    #[test]
    fn parses_bare_string_text() {
        assert_eq!(
            parse_text_args(&IntrospectValue::Text("한".to_string())),
            Ok("한".to_string())
        );
        // Empty is allowed (the caller no-ops it — IME cancel-via-empty-commit).
        assert_eq!(
            parse_text_args(&IntrospectValue::Text(String::new())),
            Ok(String::new())
        );
    }

    #[test]
    fn parses_object_text() {
        assert_eq!(
            parse_text_args(&json_args(json!({"text": "안녕"}))),
            Ok("안녕".to_string())
        );
    }

    #[test]
    fn non_string_text_is_type_mismatch() {
        assert_eq!(
            parse_text_args(&json_args(json!({}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_text_args(&json_args(json!({"text": 1}))),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            parse_text_args(&IntrospectValue::Int(1)),
            Err(InvokeError::TypeMismatch)
        );
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    /// A `sh` that puts its PTY in raw `-echo` (so the raw capture is byte-clean, not cooked /
    /// caret-mangled) and copies stdin to stdout with `cat`, optionally enabling bracketed paste
    /// (DECSET 2004) first — the child echoes whatever the host writes, so what we pasted appears
    /// verbatim in [`PanePtyHandle::raw_output`].
    fn raw_cat(enable_2004: bool) -> sprag_terminal::PanePty {
        use sprag_terminal::{CommandBuilder, PanePty};
        let script = if enable_2004 {
            "stty raw -echo 2>/dev/null; printf '\\033[?2004h'; cat"
        } else {
            "stty raw -echo 2>/dev/null; cat"
        };
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(script);
        command.env("TERM", "xterm");
        PanePty::spawn(command, 40, 6).expect("spawn a pty")
    }

    fn wait_until(mut done: impl FnMut() -> bool) -> bool {
        use std::time::{Duration, Instant};
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    #[test]
    fn paste_brackets_when_the_child_enabled_2004() {
        let pty = raw_cat(true);
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().bracketed_paste),
            "the child's DECSET 2004 was never emulated",
        );
        assert!(paste(&handle, "a\nb", Hand::APerson));
        // The raw `cat` echoes the bytes the host wrote, so the bracket wrap rides the capture.
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[200~a\nb\x1b[201~")),
            "the paste reached the child UNBRACKETED (mode 2004 was on)",
        );
    }

    #[test]
    fn paste_is_raw_when_the_child_did_not_enable_2004() {
        let pty = raw_cat(false);
        let handle = pty.handle();
        assert!(!handle.input_modes().bracketed_paste);
        assert!(paste(&handle, "a\nb", Hand::APerson));
        // The literal text echoes back with NO bracket markers (legacy, == send_text).
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"a\nb")),
            "the raw paste never reached the child",
        );
        assert!(
            !contains(&pty.raw_output().bytes, b"\x1b[200~"),
            "an un-negotiated pane must NOT get bracket markers",
        );
    }

    /// A `raw -echo` `cat` that first enables mouse tracking (DECSET 1000) + SGR encoding (1006), so
    /// a report the host writes echoes back verbatim in [`PanePtyHandle::raw_output`].
    fn raw_cat_mouse() -> sprag_terminal::PanePty {
        use sprag_terminal::{CommandBuilder, PanePty};
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[?1000h\\033[?1006h'; cat");
        command.env("TERM", "xterm");
        PanePty::spawn(command, 40, 6).expect("spawn a pty")
    }

    fn press(button: MouseButton, col: u16, row: u16) -> MouseInput {
        MouseInput {
            button,
            kind: MouseEventKind::Press,
            col,
            row,
            mods: Modifiers::default(),
        }
    }

    #[test]
    fn parse_mouse_args_reads_the_object_form() {
        let parsed = parse_mouse_args(&json_args(json!({
            "button": "right", "kind": "press", "col": 4, "row": 2, "ctrl": true,
        })))
        .expect("valid");
        assert_eq!(parsed.button, MouseButton::Right);
        assert_eq!(parsed.kind, MouseEventKind::Press);
        assert_eq!((parsed.col, parsed.row), (4, 2));
        assert!(parsed.mods.ctrl && !parsed.mods.shift);
        // An unknown button / kind, or a bare string, is a type mismatch (a machine wire).
        assert_eq!(
            parse_mouse_args(&json_args(
                json!({"button": "x", "kind": "press", "col": 0, "row": 0})
            )),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            parse_mouse_args(&IntrospectValue::Text("left".into())),
            Err(InvokeError::TypeMismatch),
        );
    }

    #[test]
    fn mouse_report_reaches_the_child_when_tracking_is_on() {
        let pty = raw_cat_mouse();
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().mouse_protocol == sprag_vt::MouseProtocol::Click),
            "the child's DECSET 1000 was never emulated",
        );
        // A left press at cell (col 4, row 2) → SGR report ESC [ < 0 ; 5 ; 3 M (1-based).
        assert!(mouse(&handle, press(MouseButton::Left, 4, 2)));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[<0;5;3M")),
            "the SGR mouse report never reached the child",
        );
    }

    #[test]
    fn a_wheel_report_reaches_the_child_when_tracking_is_on() {
        let pty = raw_cat_mouse();
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().mouse_protocol == sprag_vt::MouseProtocol::Click),
            "the child's DECSET 1000 was never emulated",
        );
        // A wheel-up step at cell (col 4, row 2) → xterm pseudo-button 64, SGR ESC [ < 64 ; 5 ; 3 M
        // (a wheel step is a press, no release). This is the Stage 2 report the GUI's `apply_wheel`
        // sends when the pointer wheels over a tracking pane's grid.
        assert!(mouse(&handle, press(MouseButton::WheelUp, 4, 2)));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[<64;5;3M")),
            "the SGR wheel report never reached the child",
        );
    }

    #[test]
    fn a_drag_report_reaches_the_child_under_button_event_tracking() {
        use sprag_terminal::{CommandBuilder, PanePty};
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        // DECSET 1002 (button-event) + 1006 (SGR) — this level reports drag.
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[?1002h\\033[?1006h'; cat");
        command.env("TERM", "xterm");
        let pty = PanePty::spawn(command, 40, 6).expect("spawn a pty");
        let handle = pty.handle();
        assert!(
            wait_until(
                || handle.input_modes().mouse_protocol == sprag_vt::MouseProtocol::ButtonEvent
            ),
            "the child's DECSET 1002 was never emulated",
        );
        // A LEFT drag at cell (col 4, row 2) -> button 0 | motion bit 32 = 32, SGR ESC[<32;5;3M
        // (the Stage 4 report the GUI sends when the pointer moves cell with a button held).
        let drag = MouseInput {
            button: MouseButton::Left,
            kind: MouseEventKind::Drag,
            col: 4,
            row: 2,
            mods: Modifiers::default(),
        };
        assert!(mouse(&handle, drag));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[<32;5;3M")),
            "the SGR drag report never reached the child",
        );
    }

    #[test]
    fn focus_reports_reach_the_child_when_1004_is_on() {
        use sprag_terminal::{CommandBuilder, PanePty};
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("stty raw -echo 2>/dev/null; printf '\\033[?1004h'; cat");
        command.env("TERM", "xterm");
        let pty = PanePty::spawn(command, 40, 6).expect("spawn a pty");
        let handle = pty.handle();
        assert!(
            wait_until(|| handle.input_modes().focus_tracking),
            "the child's DECSET 1004 was never emulated",
        );
        // Focus gained -> ESC [ I, focus lost -> ESC [ O (Stage 5).
        assert!(focus(&handle, true));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[I")),
            "the focus-in report never reached the child",
        );
        assert!(focus(&handle, false));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"\x1b[O")),
            "the focus-out report never reached the child",
        );
    }

    #[test]
    fn a_focus_report_is_dropped_when_1004_is_off() {
        let pty = raw_cat(false); // a cat that never enables 1004
        let handle = pty.handle();
        assert!(!handle.input_modes().focus_tracking);
        // The seam accepts the edge as a no-op success — the mode wanted nothing.
        assert!(focus(&handle, true));
        // Prove nothing was written by ordering a sentinel after it.
        assert!(send_text(&handle, "SENTINEL", Hand::APerson));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"SENTINEL")),
            "the sentinel never echoed",
        );
        assert!(
            !contains(&pty.raw_output().bytes, b"\x1b[I"),
            "a pane with 1004 off must receive NO focus report",
        );
    }

    #[test]
    fn mouse_report_is_dropped_when_no_tracking_mode_is_active() {
        // A `cat` that never enables a tracking mode.
        let pty = raw_cat(false);
        let handle = pty.handle();
        assert_eq!(
            handle.input_modes().mouse_protocol,
            sprag_vt::MouseProtocol::None
        );
        // The seam accepts the event as a no-op success — the mode wanted nothing.
        assert!(mouse(&handle, press(MouseButton::Left, 4, 2)));
        // Prove NOTHING was written by ordering a sentinel after it: if a report had been written it
        // would echo before the sentinel does.
        assert!(send_text(&handle, "SENTINEL", Hand::APerson));
        assert!(
            wait_until(|| contains(&pty.raw_output().bytes, b"SENTINEL")),
            "the sentinel never echoed",
        );
        assert!(
            !contains(&pty.raw_output().bytes, b"\x1b[<"),
            "a pane with no tracking mode must receive NO mouse report",
        );
    }

    /// ⚠ **THE FIXTURE IS WHAT THESE SHARE, NOT A HELPER THAT TAKES THE CLAIM.** A
    /// `fn(claim: impl Fn(table, Invoke<'_>) -> Driven)` reads well and does not compile across a
    /// crate boundary: the `'_` in the bound desugars to a higher-ranked lifetime the plain `fn` item
    /// cannot satisfy, and the mismatch prints two signatures that look identical. Three lines per
    /// test is the honest form, and [`surface`](self) is already the fixture.
    ///
    /// The fixture is a LIVE pane over a real PTY, so every probe goes through the parser the daemon
    /// uses and the writes it survives reach a real child.
    ///
    /// ⚠ Many of these calls come back `Rejected`, and that is the design rather than a weakness: a
    /// `cat` child enables no mouse tracking and holds no pending clipboard query, and the generic
    /// filler for a required open string is not an encodable key name. `TypeMismatch` is the
    /// discriminator — the GRAMMAR's refusal — so a call the parser read and the pane declined to
    /// perform answers the question exactly as well as one that wrote bytes.
    /// ⚠⚠ **EVERY WORD THE PANE SURFACE PUBLISHES IS A WORD IT ACCEPTS** — none of which a client
    /// could discover at all before R353.
    #[test]
    fn every_published_word_is_a_word_the_pane_accepts() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::every_published_word_is_accepted(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            24,
            "one call per published word: the key edges wherever a keystroke is declared, the \
             eight mouse buttons, the four pointer edges, the two clipboard selections, and the \
             TWO HANDS on each verb that writes AND takes one. ⚠⚠ The newest two are the key \
             edges NESTED inside `inject`'s batch (register item 544) — the same closed set `key` \
             publishes, reached at a second place, and this probe walks into a nested element \
             where the value-space PIN does not",
        );
    }

    /// ⚠⚠ **AN ARGUMENT THIS SURFACE CONSTRAINS PUBLISHES WHAT IT ADMITS** — the gate that named
    /// `state` and `sel` when this table was first written with neither vocabulary declared.
    #[test]
    fn an_argument_the_pane_constrains_publishes_what_it_admits() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::a_constrained_argument_publishes_what_it_admits(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            8,
            "one probe per open string argument of every form: a key name in each of `key`'s two \
             forms and again inside `inject`'s batch element, the literal text in each of `text`'s \
             and `paste`'s two, and a clipboard answer's text — every one of them a value the \
             caller invents",
        );
    }

    /// ⚠⚠ **A DECLARED ARGUMENT IS ONE THE PANE ACTUALLY READS** — and it found four coercions the
    /// first time it ran.
    ///
    /// `ctrl`, `alt`, `shift` and `super` were read with `and_then(Value::as_bool).unwrap_or(false)`,
    /// so `{"key": "a", "ctrl": 1}` injected an UNMODIFIED `a` and answered success — while `col` and
    /// `row`, two lines away in the same file, refused a malformed value. R330's odd-one-out rule and
    /// R352b's `report_agent` defect, in the parser a keystroke goes through.
    /// ⚠⚠ **AN OPTIONAL ARGUMENT OF THE INPUT SURFACE MAY BE DECLINED AS `null`** — the third
    /// surface asked, because the defect was never about one verb: a client whose language
    /// serialises an absent optional as `null` calls EVERY verb that way.
    #[test]
    fn an_optional_argument_of_the_pane_surface_may_be_declined_as_null() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::an_optional_argument_may_be_declined_as_null(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            16,
            "one probe per OPTIONAL declared argument of every form — required ones are not \
             driven, because `null` for something the grammar demands is malformed rather than \
             declined. ⚠⚠ The newest FIVE are the declinable fields of one stroke inside \
             `inject`'s batch (register item 544): an edge and four modifiers, each of which a \
             driver assembling strokes in code will leave out for most of them",
        );
    }

    #[test]
    fn a_declared_argument_is_one_the_pane_reads() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::a_declared_argument_is_one_the_daemon_reads(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            32,
            "one probe per declared argument of every FORM: EIGHT across `key`'s two forms, THREE \
             each for `text` and `paste`, seven for a mouse report, one focus edge, and three for a \
             clipboard answer. ⚠⚠ The newest SEVEN are `inject`'s (register item 544): the batch \
             itself and the six fields of one stroke inside it. ⚠ That the NESTED six are probed \
             is what makes this pin worth its count here — the driver's door would otherwise be \
             one declared argument with six undriven ones inside it, which is precisely the shape \
             this claim exists to refuse",
        );
    }

    /// ⚠ **NO VERB OF THIS SURFACE TAKES NOTHING, ASSERTED RATHER THAN ASSUMED** — the tripwire that
    /// makes `a_nullary_form_is_a_verb_that_needs_nothing` start holding it the day one does.
    ///
    /// The claim exists because the GUI's five nullary verbs needed it, and R353's `FormKind` doc had
    /// said sprag had none of them. A number here is what keeps that from being a statement about the
    /// surfaces somebody happened to be looking at.
    #[test]
    fn no_verb_of_this_surface_is_nullary_yet() {
        let (_workspace, mut external) = surface();
        assert_eq!(
            sprag_conformance::a_nullary_form_is_a_verb_that_needs_nothing(
                crate::wire::PANE_GRAMMAR,
                &mut |action, args| external.invoke(action, args)
            )
            .count_or_panic(),
            0,
            "every verb this surface serves takes arguments, so the claim drives nothing — and the \
             number is what says so",
        );
    }

    /// ⚠⚠ **A VERB THAT TAKES TWO SHAPES PUBLISHES TWO SHAPES** — built from the SERVED ANSWER, one
    /// call per published form.
    ///
    /// # The first version of this test was the defect it exists to catch
    ///
    /// It sent a bare string and an object straight to `invoke` and asserted both were read, and its
    /// own doc claimed that dropping the scalar declaration would redden it. **Measured: it stayed
    /// GREEN.** Of course it did — the daemon accepts the bare string whether or not the wire mentions
    /// it, so the test was about the PARSER while claiming to be about the PUBLICATION. R320's rule,
    /// which R352 paid once already at the ratchet level and this repeats one level down.
    ///
    /// So the shapes come off the served slot now. `text` must publish BOTH kinds, and each published
    /// form is filled and driven — a form the wire stops mentioning is a form this cannot drive, and
    /// the count says so.
    #[test]
    fn a_verb_that_takes_two_shapes_publishes_both_of_them() {
        let (_workspace, mut external) = surface();
        let published = external
            .query(crate::wire::ACTION_GRAMMAR_SLOT)
            .expect("the surface owns the address");
        let IntrospectValue::Json(Value::Object(verbs)) = published else {
            panic!("the grammar slot answers an object");
        };
        let forms = verbs[TEXT_ACTION]
            .as_array()
            .expect("text answers its forms")
            .clone();

        let mut drove: Vec<&str> = Vec::new();
        for form in &forms {
            let kind = form[crate::wire::CallForm::FORM_KEY]
                .as_str()
                .expect("a form says which shape it is");
            let args = form[crate::wire::CallForm::ARGS_KEY]
                .as_array()
                .expect("a form answers its arguments");
            // FILLED FROM THE DECLARATION ALONE, the way an agent that has read this and nothing else
            // would: a scalar form's one argument IS the value, an object form's are its members.
            let call = match kind {
                "scalar" => {
                    assert_eq!(args.len(), 1, "a scalar form carries exactly one argument");
                    sprag_conformance::as_the_wire_delivers_it(&json!("한"))
                }
                "object" => {
                    let mut map = serde_json::Map::new();
                    for arg in args {
                        // ⚠⚠ A CONSTRAINED ARGUMENT IS FILLED FROM ITS OWN PUBLISHED VOCABULARY,
                        // which is what "as an agent that has read this and nothing else would"
                        // actually means. The first form of this filled every member with the same
                        // arbitrary string, which worked only while no member of this verb had a
                        // closed set — and the day one did (`hand`), the gate failed claiming the
                        // published object form was a call the surface would not read. It reads it;
                        // what it refuses is a word outside the set, correctly.
                        let value = arg
                            .get(crate::wire::ArgGrammar::ONE_OF_KEY)
                            .and_then(Value::as_array)
                            .and_then(|words| words.first().cloned())
                            .unwrap_or_else(|| json!("한"));
                        map.insert(
                            arg[crate::wire::ArgGrammar::NAME_KEY]
                                .as_str()
                                .expect("an argument is named")
                                .to_owned(),
                            value,
                        );
                    }
                    json_args(Value::Object(map))
                }
                other => panic!("`{other}` is not a shape this surface publishes"),
            };
            assert!(
                external.invoke(TEXT_ACTION, call).is_ok(),
                "the {kind} form is published, so it is a call this surface reads",
            );
            drove.push(match kind {
                "scalar" => "scalar",
                _ => "object",
            });
        }
        assert_eq!(
            drove,
            ["scalar", "object"],
            "BOTH shapes are published — the bare value an IME commit sends, and the object form. A \
             verb that takes two and publishes one tells a client less than it could, and this is the \
             only gate that can see which one went missing.",
        );

        // THE CONTROL, both ways: neither form is the other's shape wearing a hat, so a client that
        // picked a published form and filled it as published cannot have picked wrong.
        assert_eq!(
            external.invoke(TEXT_ACTION, IntrospectValue::Json(json!(4242))),
            Err(InvokeError::TypeMismatch),
            "a bare NUMBER is not this verb's scalar form",
        );
        assert_eq!(
            external.invoke(TEXT_ACTION, json_args(json!({"content": "한"}))),
            Err(InvokeError::TypeMismatch),
            "and an object keyed by anything else is not its object form",
        );
    }

    /// ⚠⚠ **A VERB THIS SURFACE DOES NOT DECLARE IS A VERB IT DOES NOT RUN** — the guard this surface
    /// was the odd one out for lacking.
    ///
    /// Five surfaces ran [`declares_verb`](crate::wire::declares_verb) at their door from R352 and
    /// this one did not, so a verb added to the `match` below without a line in
    /// [`PANE_SCHEMA`](crate::wire::PANE_SCHEMA) would have been dispatched and discoverable by
    /// nobody — R352's own defect, on the surface a keybinding drives in-process most often.
    ///
    /// ⚠ **WHAT THIS TEST CAN AND CANNOT SHOW.** The guard reads a `const` schema, so no fixture can
    /// hand it a schema with a verb missing; what a test can do is show the guard is REACHED, and the
    /// MUTATION PAIR is what shows it is load-bearing. Both halves were run:
    ///
    /// * deleting `SchemaField::action(FOCUS_ACTION, "action")` from `PANE_SCHEMA` **with the guard in
    ///   place** reddens [`a_declared_argument_is_one_the_pane_reads`](self) — the verb became
    ///   unreachable, so its arguments answer `UnknownPath` where the grammar promises `TypeMismatch`,
    ///   and the gate that drives the published grammar is what reports it;
    /// * the same deletion **with the guard removed** leaves that gate GREEN: the verb is dispatched
    ///   while `$schema` never mentions it, which is R352's defect exactly and the state this surface
    ///   was in until R353.
    ///
    /// So the guard is what makes an undeclared arm unreachable, and the count below is a second,
    /// independent instrument — it noticed the undeclaration in BOTH halves, because it reads the
    /// schema rather than the dispatch.
    #[test]
    fn a_verb_this_surface_does_not_declare_is_a_verb_it_does_not_run() {
        let (_workspace, mut external) = surface();

        // Every declared verb is reachable — the half that makes the guard's cost visible rather
        // than theoretical, and derived from the SCHEMA so a verb added there joins this loop.
        let declared: Vec<&str> = crate::wire::PANE_SCHEMA
            .iter()
            .filter(|field| field.channel == pinion_core::external::SchemaChannel::Invoke)
            .map(|field| field.path)
            .collect();
        assert_eq!(
            declared.len(),
            7,
            "the verbs this surface declares — the display client's, plus the run driver's own \
             `inject` (register item 544)",
        );
        for verb in declared {
            assert_ne!(
                external.invoke(verb, IntrospectValue::Null),
                Err(InvokeError::UnknownPath),
                "`{verb}` is declared, so the guard lets it through to its own parser",
            );
        }

        // And a name this surface does not declare is refused as UNKNOWN rather than reaching a
        // parser — the same answer a daemon too old to know it would give.
        assert_eq!(
            external.invoke("type_this_for_me", IntrospectValue::Null),
            Err(InvokeError::UnknownPath),
        );
    }

    /// The pane surface ANSWERS its own grammar, and the answer describes the verbs IT serves.
    ///
    /// ⚠ Read off the surface, not off the table: a slot that stopped answering — or one answering
    /// the multiplexer's table by a copy-paste — fails here, which is R320's rule applied to the
    /// second surface that publishes one.
    #[test]
    fn the_pane_surface_answers_how_to_call_its_own_verbs() {
        let (_workspace, external) = surface();
        let answer = external
            .query(crate::wire::ACTION_GRAMMAR_SLOT)
            .expect("the surface owns the address");
        let IntrospectValue::Json(Value::Object(verbs)) = answer else {
            panic!("the grammar slot answers an object");
        };

        let mut published: Vec<&String> = verbs.keys().collect();
        published.sort_unstable();
        assert_eq!(
            published,
            [
                CLIPBOARD_ANSWER_ACTION,
                FOCUS_ACTION,
                INJECT_ACTION,
                KEY_ACTION,
                MOUSE_ACTION,
                PASTE_ACTION,
                TEXT_ACTION
            ],
            "the verbs this surface serves, and not the multiplexer's",
        );

        // `key`'s two forms, with the shapes that make them two — the fact no array of arguments
        // could have carried.
        let forms = verbs[KEY_ACTION].as_array().expect("key answers its forms");
        let shapes: Vec<&str> = forms
            .iter()
            .map(|form| {
                form[crate::wire::CallForm::FORM_KEY]
                    .as_str()
                    .unwrap_or("?")
            })
            .collect();
        assert_eq!(shapes, ["scalar", "object"]);
        assert_eq!(
            forms[0][crate::wire::CallForm::ARGS_KEY]
                .as_array()
                .expect("a form answers its arguments")
                .len(),
            1,
            "a scalar form carries exactly one argument: the whole value",
        );
    }
}
