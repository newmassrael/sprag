//! The termwiz-backed terminal emulator.
//!
//! DESIGN.md §4: termwiz is the embedded verified escape parser; this
//! module is the sprag-owned state machine that turns the parsed
//! [`Action`] stream into a [`Screen`]. termwiz tokenizes the bytes
//! (the max-risk part, delegated to a verified library); sprag decides
//! what each semantic action does to the grid.
//!
//! Scope is the walking-skeleton subset (DESIGN.md §5): print with
//! autowrap and wide-cluster handling, CR/LF/BS/HT, SGR pen, cursor
//! moves, erase-in-line/display, and alternate-screen + cursor-visibility
//! private modes. Unhandled sequences are ignored (see [`Emulator::advance`]).

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use termwiz::cell::{Blink, Intensity, Underline};
use termwiz::color::ColorSpec;
use termwiz::escape::apc::{
    KittyImage, KittyImageCompression, KittyImageData, KittyImageDelete, KittyImageFormat,
    KittyImageTransmit,
};
use termwiz::escape::csi::{
    CSI, Cursor as CsiCursor, CursorStyle, DecPrivateMode, DecPrivateModeCode, Device, Edit,
    EraseInDisplay, EraseInLine, Keyboard, KittyKeyboardMode, Mode, Sgr,
};
use termwiz::escape::osc::{FinalTermSemanticPrompt, Selection};
use termwiz::escape::parser::Parser;
use termwiz::escape::{
    Action, ControlCode, Esc, EscCode, OperatingSystemCommand, Sixel, SixelData,
};

use crate::port::{
    Attrs, Cell, ClipboardQuery, ClipboardTarget, ClipboardTargets, ClipboardWrite, Color, Cursor,
    CursorShape, Hyperlink, Image, InputModes, KittyKeyboardFlags, MouseEncoding, MouseProtocol,
    Notification, PromptMark, Rgb, Screen, ScreenKind, ShellState, UnderlineStyle, VtPort, Width,
    char_columns,
};

/// The cursor state DECSC (`ESC 7` / `CSI s`) saves and DECRC (`ESC 8` / `CSI u`) restores:
/// position plus the SGR pen and cursor shape. Charset state is out of the emulator's subset, so
/// it is not part of the save (a documented bound, consistent with the rest of the skeleton).
#[derive(Clone, Copy)]
struct SavedCursor {
    col: u16,
    row: u16,
    fg: Color,
    bg: Color,
    underline_color: Option<Color>,
    attrs: Attrs,
    cursor_shape: CursorShape,
}

/// A terminal emulator: feed PTY bytes via [`VtPort::advance`], read the
/// resulting [`Screen`] via [`VtPort::screen`].
pub struct Emulator {
    parser: Parser,
    /// The active screen (main, or alternate while a fullscreen app runs).
    screen: Screen,
    /// The saved main screen while the alternate screen is active.
    saved_main: Option<Screen>,
    cols: u16,
    rows: u16,
    /// The DECSTBM scroll region as an INCLUSIVE, 0-based row range
    /// `[scroll_top, scroll_bottom]`, defaulting to `[0, rows - 1]` = the whole screen.
    /// A line feed / IND at `scroll_bottom` scrolls the region up (its top line leaving);
    /// a reverse index (RI) at `scroll_top` scrolls it down; IL/DL/SU/SD act within it;
    /// rows outside the region stay put — the `less` / `vim` / tmux-status-bar split-region
    /// idiom. Reset to the full screen on resize and on every alt-screen transition (the
    /// region is screen-relative, and a fullscreen app sets its own after entering the alt
    /// screen). Origin mode (DECOM) is not modeled, so a DECSTBM homes the cursor to the
    /// SCREEN top-left, not the region's top — a documented bound consistent with the rest
    /// of the skeleton.
    scroll_top: u16,
    scroll_bottom: u16,
    // Cursor + pen state the screen does not itself track.
    col: u16,
    row: u16,
    cursor_visible: bool,
    cursor_shape: CursorShape,
    fg: Color,
    bg: Color,
    /// SGR 58 / 59 underline colour — a third pen colour channel, peer of
    /// `fg` / `bg` (`None` = SGR-59 default, draw the underline in `fg`).
    underline_color: Option<Color>,
    attrs: Attrs,
    /// Input modes set by the child (DECCKM, …) that the key encoder
    /// reads; tracked here, exposed via [`VtPort::input_modes`].
    input_modes: InputModes,
    /// The Kitty keyboard protocol enhancement-flag STACK the child pushes/pops (`CSI > u` /
    /// `CSI < u` / `CSI = u`). Each entry is a flag bitmask ALREADY masked to what sprag honors
    /// ([`KITTY_KEYBOARD_SUPPORTED`]); the CURRENT flags exposed via [`VtPort::input_modes`] are
    /// the top of the stack (or empty). Bounded by [`KITTY_STACK_CAP`] against a runaway pusher.
    kitty_kbd_stack: Vec<u8>,
    /// Bytes the terminal owes the child in reply to a query it made (the device-response channel:
    /// currently the Kitty `CSI ? u` flags query). Drained by [`VtPort::take_responses`] after each
    /// batch and written back to the PTY. Not row damage — carries no cells.
    responses: Vec<u8>,
    /// The child's self-reported window TITLE (`OSC 0` / `OSC 2`), `None` until it
    /// sets one. Exposed via [`VtPort::title`]; a shell's `PROMPT_COMMAND` (or vim,
    /// ssh, tmux…) rewrites it continuously, so this is live state, NOT the spawn
    /// command label. Deliberately does NOT bump [`Self::generation`] — that stamp is
    /// ROW DAMAGE, and a title carries no cells; marking rows dirty for it would force
    /// needless cell re-render. The change still reaches consumers because the OSC
    /// bytes arrive as PTY output, which already fires the session's `on_dirty`.
    title: Option<String>,
    /// The most recent attention notification the child raised (`OSC 9` / `OSC
    /// 777;notify` / `OSC 99`), or `None`. Latched (last wins), exposed via
    /// [`VtPort::notification`]. Like [`Self::title`] it deliberately does NOT bump
    /// [`Self::generation`] — it carries no cells — and reaches consumers because the
    /// OSC bytes arrive as PTY output, which already fires the session's `on_dirty`.
    notification: Option<Notification>,
    /// Monotonic count of notifications raised (`0` before the first), exposed via
    /// [`VtPort::notification_seq`]. Bumped once per captured notification so a
    /// consumer can tell a NEW one from a re-read of the same latched payload.
    notification_seq: u64,
    /// Monotonic count of BELLs (`\a`, `ControlCode::Bell`) the child has rung (`0` before the
    /// first), exposed via [`VtPort::bell_seq`]. A bell is the tmux `monitor-bell` signal — a
    /// text-less "pay attention" ping, DISTINCT from a [`Notification`] (a desktop toast that
    /// carries text). It is kept as its own counter, NOT folded into [`Self::notification_seq`],
    /// so the two attention sources stay individually addressable (as tmux keeps its bell flag
    /// separate from activity); the multiplexer's attention marker sums them. Like the
    /// notification seq it does NOT bump [`Self::generation`] — a bell carries no cells — and
    /// reaches consumers because the byte arrives as PTY output, which fires the session's
    /// `on_dirty`. NB the `\a` that TERMINATES an OSC string (its `ST`) is consumed by the parser
    /// as part of the OSC, so only a bare BEL in the stream counts here.
    bell_seq: u64,
    /// The most recent OSC 52 clipboard WRITE the child requested (`None` until one), LATCHED
    /// like [`Self::notification`]. Exposed via [`VtPort::clipboard_write`]; a display client
    /// applies it to its system clipboard when [`Self::clipboard_write_seq`] grows. Carries no
    /// cells, so — like the title / notification — it does NOT bump [`Self::generation`]; the OSC
    /// bytes arrive as PTY output, which already fires the session's `on_dirty`.
    clipboard_write: Option<ClipboardWrite>,
    /// Monotonic count of OSC 52 clipboard writes requested (`0` before the first), exposed via
    /// [`VtPort::clipboard_write_seq`] — lets a consumer apply each write once.
    clipboard_write_seq: u64,
    /// The most recent OSC 52 clipboard READ (query) the child requested (`None` until one),
    /// LATCHED. Exposed via [`VtPort::clipboard_query`]; a display client answers it, subject to
    /// policy, when [`Self::clipboard_query_seq`] grows. Does NOT bump [`Self::generation`].
    clipboard_query: Option<ClipboardQuery>,
    /// Monotonic count of OSC 52 clipboard reads requested (`0` before the first), exposed via
    /// [`VtPort::clipboard_query_seq`] — lets a consumer answer each query once.
    clipboard_query_seq: u64,
    /// The cursor position + pen saved by DECSC (`ESC 7` / `CSI s`) and restored by DECRC
    /// (`ESC 8` / `CSI u`), or `None` before any save. Saves the same set a terminal restores —
    /// position, SGR foreground/background/attributes, and cursor shape — so an app that saves,
    /// draws in a different pen, then restores comes back exactly where and how it was.
    saved_cursor: Option<SavedCursor>,
    /// The last GRAPHIC character printed, for REP (`CSI b` — REPEAT). `None` until one is printed
    /// or after an action that is not a plain print. Repeat re-emits this, so it tracks exactly
    /// what a bare `print` would repeat.
    last_print: Option<char>,
    /// The OSC-8 hyperlink PEN: the link (`Arc<Hyperlink>`) currently open via `\e]8;…`, or `None`
    /// between links. Each printed cell clones this handle ([`Self::print_str`]), so a link and its
    /// wrap continuations share one `Arc` and the projection groups them by pointer identity.
    /// Cleared by `\e]8;;` (the OSC-8 close). Deliberately NOT saved by DECSC — OSC-8 URL mode is
    /// separate from the SGR pen a cursor save restores (a documented bound, matching tmux). Carries
    /// no cells of its own, so setting it does NOT bump [`Self::generation`]; the cells printed
    /// under it carry their own damage.
    current_hyperlink: Option<Arc<Hyperlink>>,
    /// Interns OSC-8 links by their `id=` grouping key, so a link that reappears non-adjacently (the
    /// same `id` after intervening text) reuses the SAME `Arc` and its runs group as one logical
    /// link (R-69.3.b — what pure position-based grouping cannot express). Anonymous links (no `id`)
    /// are never cached — each `\e]8;;uri` opens a fresh run. Bounded by [`HYPERLINK_ID_CAP`]: once
    /// full a new `id` gets a fresh (ungrouped) `Arc` rather than growing unbounded — a documented
    /// degradation, not a leak.
    hyperlink_ids: HashMap<String, Arc<Hyperlink>>,
    /// A monotonic counter minting an [`Image::id`] for each SIXEL image (Kitty images carry their
    /// own `i=` id; sixel has none). Namespaced with the high bit set (`0x8000_0000 | n`) so a
    /// synthesized sixel id does not collide with a small child-chosen Kitty id in the shared
    /// replace-by-id [`Screen`] store — a documented bound (a Kitty image with the top bit set,
    /// unusual, could still collide).
    next_sixel_id: u32,
    /// The SINGLE in-flight Kitty graphics CHUNKED transmission (`m=1`), or `None`. kitty allows one
    /// chunked transmission at a time and its continuation chunks carry ONLY the `m` key (no `i=`),
    /// so the terminal tracks the pending transmission itself rather than keying by id. The first
    /// chunk records the format / dimensions / display intent / id; each subsequent chunk appends its
    /// base64; the closing chunk (`m=0`) decodes the assembled payload and — if the first chunk asked
    /// to display (`a=T`) — stores the image (Stage 4). Bounded by [`MAX_KITTY_CHUNK_BYTES`]: a
    /// transmission whose accumulated base64 exceeds the cap is dropped (a runaway / never-closing
    /// chunk cannot pin unbounded memory).
    kitty_chunk: Option<KittyChunk>,
    /// Monotonic damage stamp, bumped on every row-mutating action.
    generation: u64,
    /// `true` while the line editor (bash/readline) is redrawing its wrapped prompt
    /// on `SIGWINCH`. Opened by a resize (on the MAIN screen); two behaviours change
    /// inside it so the redraw stays one clean logical line that collapses on a later
    /// widen — the way a reflowing terminal (vte/`gnome-terminal`) handles the same
    /// bytes:
    ///
    /// * an explicit `CR LF` (which readline emits at a width the line exactly
    ///   fills, instead of relying on autowrap) is treated as a SOFT wrap by
    ///   [`Self::control`], not a hard line end — otherwise it splits the prompt
    ///   into separate logical lines that cannot rejoin, leaving per-width copies
    ///   stacked as ghosts (the resize-stale accumulation);
    /// * the redraw's leading erase-in-line ([`Self::edit`]) clears the whole
    ///   wrapped active line, not just the cursor's row, so the stale tail left by
    ///   the prior width does not survive as a growing leftover in the input.
    ///
    /// WHY a window and not a purely structural signal: the editor's `CR LF` lands
    /// MID-row (a premature break, columns short of the margin), so it is
    /// byte-for-byte indistinguishable from a genuine newline — only the resize
    /// CONTEXT marks it as a soft wrap. (vte/`gnome-terminal` likewise rely on
    /// context, not a pending-wrap latch, and likewise show the break until widen.)
    ///
    /// WHEN it closes (the design that makes a MULTI-BATCH redraw correct — a long
    /// line reflowed narrow can overflow a single PTY read, so the redraw arrives as
    /// several `advance` batches):
    ///
    /// * while the shell reports (OSC 133) it is AT A PROMPT, the window persists
    ///   ACROSS batches (see [`VtPort::advance`]) — the redraw is exactly what a
    ///   resize triggers at a prompt, however many reads it spans;
    /// * it closes the instant the consumer sends the child input
    ///   ([`VtPort::note_input`]) — the user typing or submitting is the definitive
    ///   end of the redraw epoch, so a submitting `CR LF` and the command output that
    ///   follows are hard breaks again;
    /// * with NO shell integration (prompt state [`ShellState::Unknown`]) it falls
    ///   back to the SINGLE-batch close (cleared at the end of the first `advance`) —
    ///   the previous behaviour, kept because a non-integrated shell cannot be told
    ///   apart from a foreground command's output, so persisting would wrongly
    ///   soft-wrap that output. tmux reflows neither and uses no such signal; keying
    ///   the window off the shell's own prompt state is where sprag does better.
    in_resize_redraw: bool,
}

impl Emulator {
    /// A fresh emulator with a blank `cols x rows` main screen.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        Self {
            parser: Parser::new(),
            screen: Screen::new(cols.max(1), rows.max(1)),
            saved_main: None,
            cols: cols.max(1),
            rows: rows.max(1),
            scroll_top: 0,
            scroll_bottom: rows.max(1) - 1,
            col: 0,
            row: 0,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: Attrs::default(),
            input_modes: InputModes::default(),
            kitty_kbd_stack: Vec::new(),
            responses: Vec::new(),
            title: None,
            notification: None,
            notification_seq: 0,
            bell_seq: 0,
            clipboard_write: None,
            clipboard_write_seq: 0,
            clipboard_query: None,
            clipboard_query_seq: 0,
            saved_cursor: None,
            last_print: None,
            current_hyperlink: None,
            hyperlink_ids: HashMap::new(),
            next_sixel_id: 0,
            kitty_chunk: None,
            generation: 0,
            in_resize_redraw: false,
        }
    }

    fn next_gen(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Apply one parsed action to the grid.
    fn apply(&mut self, action: Action) {
        match action {
            Action::Print(ch) => self.print_str(&ch.to_string()),
            Action::PrintString(s) => self.print_str(&s),
            Action::Control(code) => self.control(code),
            Action::CSI(csi) => self.csi(csi),
            Action::OperatingSystemCommand(osc) => self.osc(&osc),
            Action::Esc(esc) => self.esc(esc),
            // Kitty graphics (APC `_G…`) + Sixel (DCS `P…q…`), pinion R1404: capture a
            // transmitted image anchored at the cursor. Both rasterize to the same [`Image`]
            // and flow through the one image store -> panes-slot wire -> GUI composite.
            Action::KittyImage(img) => self.kitty_image(&img),
            Action::Sixel(sixel) => self.sixel_image(&sixel),
            _ => {}
        }
    }

    /// Capture / manage a Kitty graphics image (APC `_G…`, pinion R1404). Handles `a=T`
    /// transmit-and-display (RGB `f=24` / RGBA `f=32` / PNG `f=100`, single-chunk or chunked
    /// `m=1`), `a=t` transmit (chunk continuation), `a=d` delete, and `a=q` support-query ack.
    /// Deferred with documented bounds (ignored, NOT misrendered): `a=p` Display-without-transmit,
    /// compression, file/shm transmit (`t=f/s`), animation (`a=f/c`), and placement geometry
    /// (columns/rows/z/offsets — one placement per image id, anchored at the cursor).
    fn kitty_image(&mut self, img: &KittyImage) {
        match img {
            KittyImage::TransmitDataAndDisplay { transmit, .. } => {
                self.kitty_transmit(transmit, true);
            }
            KittyImage::TransmitData { transmit, .. } => self.kitty_transmit(transmit, false),
            KittyImage::Delete { what, .. } => self.kitty_delete(what),
            KittyImage::Query { transmit } => self.kitty_query(transmit),
            // a=p Display / a=f,c animation frames: documented bounds.
            _ => {}
        }
    }

    /// Apply a Kitty transmit with CHUNKING (`m=1`, R1404 Stage 4). A chunk (`more_data_follows`)
    /// accumulates its base64 payload keyed by image id / number; the FIRST chunk records the format
    /// / dimensions / display intent. The closing chunk (`m=0`, or a non-chunked transmit) decodes
    /// the assembled payload via [`Self::decode_image`] and, when the transmission asked to display
    /// (`a=T`), stores the image at the cursor. `display` is honoured from the FIRST chunk only, so
    /// an `a=T`-then-`m=1` continuation still displays. A transmission whose accumulated base64
    /// exceeds [`MAX_KITTY_CHUNK_BYTES`] (a runaway / never-closing chunk) is dropped.
    fn kitty_transmit(&mut self, transmit: &KittyImageTransmit, display: bool) {
        if transmit.more_data_follows {
            let accum = self.kitty_chunk.get_or_insert_with(|| KittyChunk {
                base64: String::new(),
                format: transmit.format.clone(),
                width: transmit.width,
                height: transmit.height,
                display,
                id: transmit.image_id.or(transmit.image_number).unwrap_or(0),
            });
            if let KittyImageData::Direct(b64) = &transmit.data {
                if accum.base64.len().saturating_add(b64.len()) > MAX_KITTY_CHUNK_BYTES {
                    self.kitty_chunk = None; // runaway / never-closing — drop, never leak.
                } else {
                    accum.base64.push_str(b64);
                }
            }
            return;
        }
        // Closing chunk of the in-flight transmission, or a single non-chunked transmit.
        if let Some(mut accum) = self.kitty_chunk.take() {
            if let KittyImageData::Direct(b64) = &transmit.data {
                accum.base64.push_str(b64);
            }
            if accum.display
                && let Ok(bytes) = BASE64.decode(accum.base64.as_bytes())
                && let Some(image) =
                    self.decode_image(accum.format, accum.width, accum.height, &bytes, accum.id)
            {
                self.screen.add_image(image);
            }
            return;
        }
        if display && let Some(image) = self.decode_kitty(transmit) {
            self.screen.add_image(image);
        }
    }

    /// Delete inline images per a Kitty `a=d` command (R1404 Stage 4): all (`d=a`/`d=A`) or by image
    /// id (`d=i`/`d=I`). Other delete targets (by placement / cursor / cell / z / animation frames)
    /// are documented bounds — sprag stores one placement per image id this stage.
    fn kitty_delete(&mut self, what: &KittyImageDelete) {
        match what {
            KittyImageDelete::All { .. } => self.screen.clear_images(),
            KittyImageDelete::ByImageId { image_id, .. } => self.screen.delete_image(*image_id),
            _ => {}
        }
    }

    /// Reply to a Kitty graphics support probe (`a=q`, R1404 Stage 4): append `ESC _ G i=<id>;OK ESC
    /// \` to the device-response channel ([`Self::responses`], written back to the pty by the reader
    /// like the DA / DSR / Kitty-keyboard replies), so an app detects sprag renders images — a
    /// capability tmux lacks. Carries no cells, so no damage-generation bump. termwiz models a query
    /// as always non-quiet, so sprag always acks (a quiet query is not representable = a bound).
    fn kitty_query(&mut self, transmit: &KittyImageTransmit) {
        let id = transmit.image_id.unwrap_or(0);
        self.responses
            .extend_from_slice(format!("\x1b_Gi={id};OK\x1b\\").as_bytes());
    }

    /// Rasterize a Sixel (DCS) image and store it anchored at the cursor (pinion R1404, Stage 2).
    /// Sixel carries no image id, so one is minted from [`Self::next_sixel_id`] (high-bit
    /// namespaced against child-chosen Kitty ids). A raster that is empty or over
    /// [`MAX_IMAGE_BYTES`] is dropped ([`raster_sixel`]). Like a Kitty image it clears only on a
    /// whole-screen erase this stage (scroll / reflow eviction is a documented later bound).
    fn sixel_image(&mut self, sixel: &Sixel) {
        let Some((width, height, rgba)) = raster_sixel(sixel) else {
            return;
        };
        let id = 0x8000_0000 | (self.next_sixel_id & 0x7fff_ffff);
        self.next_sixel_id = self.next_sixel_id.wrapping_add(1);
        self.screen.add_image(Image {
            id,
            width,
            height,
            rgba,
            anchor: (self.col, self.row),
            seq: 0, // assigned by Screen::add_image
        });
    }

    /// Decode a SINGLE-CHUNK Kitty transmit into an [`Image`] anchored at the cursor, or `None` when
    /// it is chunked / compressed (chunking is [`Self::kitty_transmit`]'s job) or otherwise
    /// undecodable. Base64-decodes the `Direct` payload (or takes `DirectBin`), then defers to
    /// [`Self::decode_image`] for the format handling + cap.
    fn decode_kitty(&self, transmit: &KittyImageTransmit) -> Option<Image> {
        if transmit.more_data_follows || transmit.compression != KittyImageCompression::None {
            return None;
        }
        let bytes = match &transmit.data {
            KittyImageData::Direct(b64) => BASE64.decode(b64.as_bytes()).ok()?,
            KittyImageData::DirectBin(raw) => raw.clone(),
            _ => return None,
        };
        self.decode_image(
            transmit.format.clone(),
            transmit.width,
            transmit.height,
            &bytes,
            transmit.image_id.unwrap_or(0),
        )
    }

    /// Decode already-base64-decoded / assembled image `bytes` in `format` to an [`Image`] anchored
    /// at the cursor, or `None` if undecodable. `RGB` (`f=24`, alpha-expanded) / `RGBA` (`f=32`)
    /// validate `bytes.len() == width * height * bpp`; `PNG` (`f=100`, R1404 Stage 4) reads its own
    /// dimensions. Every path applies [`image_within_cap`] BEFORE allocating, so a hostile raster is
    /// dropped, never stored / wired. The byte count not matching the raster → `None` (never
    /// misrendered).
    fn decode_image(
        &self,
        format: Option<KittyImageFormat>,
        width: Option<u32>,
        height: Option<u32>,
        bytes: &[u8],
        id: u32,
    ) -> Option<Image> {
        let (w, h, rgba) = match format {
            Some(KittyImageFormat::Png) => decode_png(bytes)?,
            Some(KittyImageFormat::Rgb) => {
                let (w, h) = (width?, height?);
                if !image_within_cap(w, h) {
                    return None;
                }
                let expected = (w as usize).checked_mul(h as usize)?.checked_mul(3)?;
                if bytes.len() != expected {
                    return None;
                }
                let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
                for px in bytes.chunks_exact(3) {
                    rgba.extend_from_slice(px);
                    rgba.push(255); // RGB -> RGBA: opaque alpha.
                }
                (w, h, rgba)
            }
            Some(KittyImageFormat::Rgba) => {
                let (w, h) = (width?, height?);
                if !image_within_cap(w, h) {
                    return None;
                }
                let expected = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
                if bytes.len() != expected {
                    return None;
                }
                (w, h, bytes.to_vec())
            }
            _ => return None,
        };
        Some(Image {
            id,
            width: w,
            height: h,
            rgba,
            anchor: (self.col, self.row),
            seq: 0, // assigned by Screen::add_image
        })
    }

    /// The two-byte `ESC <final>` sequences in the subset:
    ///
    /// * DECSC (`ESC 7`) / DECRC (`ESC 8`) save and restore the cursor + pen — the same
    ///   save/restore the `CSI s` / `CSI u` forms drive ([`cursor_op`](Self::cursor_op)).
    /// * IND (`ESC D`, index) moves the cursor down one line, scrolling the region up at
    ///   the bottom margin — identical to a bare line feed ([`line_feed`](Self::line_feed)),
    ///   minus the carriage return.
    /// * RI (`ESC M`, reverse index) is the mirror: up one line, scrolling the region DOWN
    ///   at the top margin ([`reverse_index`](Self::reverse_index)).
    ///
    /// Every other ESC (charset selection, NEL, keypad modes) is out of the subset and
    /// dropped.
    fn esc(&mut self, esc: Esc) {
        if let Esc::Code(code) = esc {
            match code {
                EscCode::DecSaveCursorPosition => self.save_cursor(),
                EscCode::DecRestoreCursorPosition => self.restore_cursor(),
                EscCode::Index => self.line_feed(),
                EscCode::ReverseIndex => self.reverse_index(),
                _ => {}
            }
        }
    }

    /// RI (reverse index, `ESC M`): move the cursor UP one line. At the top margin this
    /// scrolls the region DOWN by one (a blank line opens at the top margin, the cursor
    /// stays put) — the mirror of a line feed at the bottom margin. Above the region (a
    /// cursor parked over a fixed header) it just steps up, stopping at the screen top.
    fn reverse_index(&mut self) {
        if self.row == self.scroll_top {
            let g = self.next_gen();
            self.screen
                .scroll_region_down(self.scroll_top, self.scroll_bottom, 1, g);
        } else if self.row > 0 {
            self.row -= 1;
        }
    }

    /// Reset the DECSTBM scroll region to the whole screen. The region is screen-relative,
    /// so a resize or an alt-screen transition (a fresh buffer) starts it at the full
    /// extent; an app that wants a sub-region sets one with DECSTBM afterwards.
    fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }

    /// DECSTBM (`CSI Pt ; Pb r`): set the scroll region to the inclusive, 0-based rows
    /// `[top, bottom]`. termwiz supplies defaults for omitted parameters (top 1, bottom the
    /// last row) as 1-based `OneBased`; the caller passes them already 0-based, `bottom`
    /// clamped here to the last row (termwiz's "big default" is `u32::MAX - 1`). A region
    /// needs at least two lines: an invalid one (`top >= bottom`) is IGNORED, leaving the
    /// margins and cursor unchanged (the VT100 rule). On a valid set the cursor homes to
    /// the screen top-left (origin mode is not modeled — see the field doc).
    fn set_scroll_region(&mut self, top: u32, bottom: u32) {
        let max_row = self.rows.saturating_sub(1);
        let top = u16::try_from(top).unwrap_or(u16::MAX).min(max_row);
        let bottom = u16::try_from(bottom).unwrap_or(u16::MAX).min(max_row);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            self.row = 0;
            self.col = 0;
        }
    }

    /// Save the cursor position + pen (DECSC / `CSI s`).
    fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            col: self.col,
            row: self.row,
            fg: self.fg,
            bg: self.bg,
            underline_color: self.underline_color,
            attrs: self.attrs,
            cursor_shape: self.cursor_shape,
        });
    }

    /// Restore the cursor position + pen saved by DECSC (DECRC / `CSI u`). With no prior save the
    /// spec homes the cursor and resets the pen — the state a fresh save would hold — so a restore
    /// is always well-defined.
    fn restore_cursor(&mut self) {
        let saved = self.saved_cursor.unwrap_or(SavedCursor {
            col: 0,
            row: 0,
            fg: Color::Default,
            bg: Color::Default,
            underline_color: None,
            attrs: Attrs::default(),
            cursor_shape: CursorShape::Block,
        });
        self.col = saved.col.min(self.cols.saturating_sub(1));
        self.row = saved.row.min(self.rows.saturating_sub(1));
        self.fg = saved.fg;
        self.bg = saved.bg;
        self.underline_color = saved.underline_color;
        self.attrs = saved.attrs;
        self.cursor_shape = saved.cursor_shape;
    }

    /// Operating-system commands. Two families are in the subset:
    ///
    /// * the WINDOW-TITLE family — `OSC 0` (icon name AND window title) and `OSC 2`
    ///   (window title), plus termwiz's Sun-style spelling. `OSC 1` sets only the ICON
    ///   name — not a window title — so it is ignored.
    /// * the ATTENTION-NOTIFICATION family — `OSC 9` (iTerm2/xterm `SystemNotification`),
    ///   `OSC 777;notify;title;body` (urxvt), and `OSC 99` (kitty) — captured as a
    ///   [`Notification`] the multiplexer surfaces as "this pane wants attention".
    /// * the CLIPBOARD family — `OSC 52` (`ManipulateSelectionData`): a WRITE (set / clear a
    ///   selection) is captured as a [`ClipboardWrite`], a READ (`?`) as a [`ClipboardQuery`].
    ///   The clipboard itself is the display client's, so the emulator only records the request +
    ///   a monotonic sequence; the client applies a write to its system clipboard and answers a
    ///   read (both policy-gated), the read reply going back to the PTY (see [`osc52_reply`]).
    ///
    /// Every other OSC (hyperlinks, colour queries) is dropped, per the skeleton contract.
    ///
    /// Child-controlled strings (the title, each notification field, the clipboard write text)
    /// are CLAMPED ([`clamp_title`] / [`MAX_NOTIFICATION_BYTES`] / [`MAX_CLIPBOARD_BYTES`]): the
    /// underlying `vtparse` bounds the OSC parameter COUNT, not the payload BYTE length, so an
    /// uncapped store would let a hostile/buggy child buffer an arbitrarily large string that is
    /// then cloned on demand. This mirrors the `RAW_CAPTURE_CAP` bound on the sibling
    /// child-controlled buffer.
    fn osc(&mut self, osc: &OperatingSystemCommand) {
        match osc {
            OperatingSystemCommand::SetWindowTitle(t)
            | OperatingSystemCommand::SetWindowTitleSun(t)
            | OperatingSystemCommand::SetIconNameAndWindowTitle(t) => {
                self.title = Some(clamp_title(t));
            }
            // OSC 9 — the iTerm2/xterm growl notification: a single message, no title.
            // (termwiz routes `OSC 9;4;…` ConEmu progress to its own variant, so a
            // `SystemNotification` reaching here is always a genuine notification.)
            OperatingSystemCommand::SystemNotification(message) => {
                self.raise_notification(None, message);
            }
            // OSC 777 — urxvt's extension family; `notify` is its desktop notification,
            // `OSC 777 ; notify ; <title> ; <body>` (body optional). Any other urxvt
            // extension is ignored.
            OperatingSystemCommand::RxvtExtension(params) => {
                if let Some(kind) = params.first()
                    && kind == "notify"
                {
                    let title = params.get(1).map(String::as_str);
                    let body = params.get(2).map(String::as_str).unwrap_or("");
                    self.raise_notification(title, body);
                }
            }
            // OSC 99 — kitty's desktop-notification protocol. termwiz does not model it,
            // so it arrives as `Unspecified` raw params. [`parse_kitty_notification`]
            // handles the common single-chunk, unencoded case (see its doc for the
            // bounds); a multi-chunk or base64 payload is left uncaptured, not misparsed.
            OperatingSystemCommand::Unspecified(params) => {
                if let Some((title, body)) = parse_kitty_notification(params) {
                    self.raise_notification(title.as_deref(), &body);
                }
            }
            // OSC 133 (FinalTerm) shell-integration boundary marks — prompt / output / command
            // end. These drive jump-to-prompt + command-boundary detection (the modern-terminal
            // feature; tmux only passes them through).
            OperatingSystemCommand::FinalTermSemanticPrompt(prompt) => {
                self.shell_integration(prompt);
            }
            // OSC 52 — clipboard manipulation. A SET / CLEAR is a write (clear = write the empty
            // string); a `?` is a read query. termwiz has already base64-decoded + UTF-8-validated
            // the write data (a payload that was not valid UTF-8 fails to parse and never reaches
            // here). The clipboard is the display client's, so we only record the request.
            OperatingSystemCommand::SetSelection(sel, data) => {
                self.set_clipboard_write(sel_to_targets(sel), data);
            }
            OperatingSystemCommand::ClearSelection(sel) => {
                self.set_clipboard_write(sel_to_targets(sel), "");
            }
            OperatingSystemCommand::QuerySelection(sel) => {
                self.set_clipboard_query(sel_to_query_target(sel));
            }
            // OSC 8 — hyperlink. `Some(link)` OPENS a link: the cells printed after it belong to it
            // (a pen-like state, like SGR), until `None` (`\e]8;;`) CLOSES it. termwiz has fully
            // parsed the URI and params (the `id=` grouping key lives in params); a link's cell
            // footprint is the emulator's to track. Extract the child-controlled strings here (so
            // the helper never names termwiz's type) and clamp them like the other OSC payloads.
            OperatingSystemCommand::SetHyperlink(link) => {
                let link = link.as_ref().map(|l| {
                    (
                        clamp_bytes(l.uri(), MAX_CLIPBOARD_BYTES),
                        l.params()
                            .get("id")
                            .map(|id| clamp_bytes(id, MAX_TITLE_BYTES)),
                    )
                });
                self.set_hyperlink(link);
            }
            _ => {}
        }
    }

    /// Apply an OSC-8 hyperlink control: `Some((uri, id))` opens a link (subsequent printed cells
    /// carry it via [`Self::current_hyperlink`]); `None` (`\e]8;;`) closes the current one. A link
    /// tagged with an `id=` is INTERNED ([`Self::hyperlink_ids`]) so a later run sharing that id
    /// reuses the same `Arc` and groups with it into one logical link across intervening text or a
    /// wrap (R-69.3.b); an anonymous link (no `id`) opens a fresh `Arc` each time, so two anonymous
    /// links to the same URI stay distinct runs. Carries no cells, so it does not bump the damage
    /// generation (the cells printed under the pen carry their own).
    fn set_hyperlink(&mut self, link: Option<(String, Option<String>)>) {
        let Some((uri, id)) = link else {
            self.current_hyperlink = None;
            return;
        };
        let arc = match &id {
            Some(id_key) => {
                if let Some(existing) = self.hyperlink_ids.get(id_key) {
                    existing.clone()
                } else {
                    let arc = Arc::new(Hyperlink {
                        uri,
                        id: id.clone(),
                    });
                    // Cache for cross-run grouping, but only up to the cap — a runaway id stream then
                    // degrades to fresh ungrouped links rather than growing the map without bound.
                    if self.hyperlink_ids.len() < HYPERLINK_ID_CAP {
                        self.hyperlink_ids.insert(id_key.clone(), arc.clone());
                    }
                    arc
                }
            }
            None => Arc::new(Hyperlink { uri, id: None }),
        };
        self.current_hyperlink = Some(arc);
    }

    /// Latch an OSC 52 clipboard WRITE (clamping the child-controlled text) and bump the monotonic
    /// sequence. A write addressing no selection sprag models (an X-cut-buffer-only request) is a
    /// no-op — neither latched nor counted, so it cannot supersede a real pending write.
    fn set_clipboard_write(&mut self, targets: ClipboardTargets, text: &str) {
        if targets.is_empty() {
            return;
        }
        self.clipboard_write = Some(ClipboardWrite {
            targets,
            text: clamp_bytes(text, MAX_CLIPBOARD_BYTES),
        });
        self.clipboard_write_seq += 1;
    }

    /// Latch an OSC 52 clipboard READ query and bump the monotonic sequence.
    fn set_clipboard_query(&mut self, target: ClipboardTarget) {
        self.clipboard_query = Some(ClipboardQuery { target });
        self.clipboard_query_seq += 1;
    }

    /// Apply one OSC 133 (FinalTerm) semantic-prompt marker, attaching a [`PromptMark`] to the
    /// cursor's current row where one belongs. `A` (prompt start) and the fresh-line forms do a
    /// FinalTerm "fresh line" first — a `CR LF` unless already at the left margin — so the mark
    /// lands on a clean line. `B` (end of prompt / start of input) sets no row mark: at row
    /// granularity the user's input sits on the prompt row, so the command text is the rows from
    /// the prompt up to the output start (see [`PromptMark`]).
    fn shell_integration(&mut self, prompt: &FinalTermSemanticPrompt) {
        use FinalTermSemanticPrompt as F;
        match prompt {
            F::FreshLine => self.fresh_line(),
            // A — start of a prompt.
            F::FreshLineAndStartPrompt { .. } | F::StartPrompt(_) => {
                self.fresh_line();
                self.screen.set_mark(self.row, Some(PromptMark::Prompt));
            }
            // C — the command executed; its output starts here.
            F::MarkEndOfInputAndStartOfOutput { .. } => {
                self.screen.set_mark(self.row, Some(PromptMark::Output));
            }
            // D — the command finished, with the exit status the shell reported.
            F::CommandStatus { status, .. } => {
                self.screen
                    .set_mark(self.row, Some(PromptMark::CommandEnd(Some(*status))));
            }
            // D without a status (bare end-of-command), then a fresh line for the next prompt.
            F::MarkEndOfCommandWithFreshLine { .. } => {
                self.screen
                    .set_mark(self.row, Some(PromptMark::CommandEnd(None)));
                self.fresh_line();
            }
            // B — end of prompt / start of input: no row mark (see the doc above).
            F::MarkEndOfPromptAndStartOfInputUntilNextMarker
            | F::MarkEndOfPromptAndStartOfInputUntilEndOfLine => {}
        }
    }

    /// A FinalTerm "fresh line": if the cursor is not at the left margin, do the equivalent of a
    /// `CR LF` (column home + a region-aware line feed); otherwise nothing. Used by the OSC 133
    /// prompt / command-end markers so a mark lands at the head of a clean line.
    fn fresh_line(&mut self) {
        if self.col != 0 {
            self.col = 0;
            self.line_feed();
        }
    }

    /// Latch a captured attention notification (clamping both child-controlled fields)
    /// and bump the monotonic sequence so a consumer can tell it is new. Shared by every
    /// notification OSC so the clamp + counter live in ONE place.
    fn raise_notification(&mut self, title: Option<&str>, body: &str) {
        self.notification = Some(Notification {
            title: title.map(|t| clamp_bytes(t, MAX_NOTIFICATION_BYTES)),
            body: clamp_bytes(body, MAX_NOTIFICATION_BYTES),
        });
        self.notification_seq += 1;
    }

    fn control(&mut self, code: ControlCode) {
        match code {
            ControlCode::LineFeed | ControlCode::VerticalTab | ControlCode::FormFeed => {
                // A line feed ends this row's logical line (a hard break) — UNLESS it
                // is the editor's resize-redraw wrap idiom, where it CONTINUES the
                // line (a soft wrap). See `in_resize_redraw` for why.
                let soft_wrap = self.in_resize_redraw;
                self.screen.set_wrapped(self.row, soft_wrap);
                self.line_feed();
            }
            ControlCode::CarriageReturn => self.col = 0,
            ControlCode::Backspace => self.col = self.col.saturating_sub(1),
            ControlCode::HorizontalTab => {
                // Advance to the next 8-column tab stop, clamped to width.
                let next = ((self.col / 8) + 1) * 8;
                self.col = next.min(self.cols.saturating_sub(1));
            }
            // BEL (`\a`) — the tmux monitor-bell attention ping. Count it (a text-less attention
            // event); it does not touch the grid. See `bell_seq`.
            ControlCode::Bell => self.bell_seq += 1,
            _ => {}
        }
    }

    fn csi(&mut self, csi: CSI) {
        match csi {
            CSI::Sgr(sgr) => self.sgr(sgr),
            CSI::Cursor(c) => self.cursor_op(c),
            CSI::Edit(e) => self.edit(e),
            CSI::Mode(m) => self.mode(m),
            CSI::Keyboard(k) => self.kitty_keyboard(k),
            CSI::Device(dev) => self.device(&dev),
            _ => {}
        }
    }

    /// Apply one Kitty keyboard protocol negotiation command. The protocol is a STACK of
    /// enhancement-flag sets: `CSI > flags u` PUSHES a level, `CSI < n u` POPS `n`, `CSI = flags ; mode u`
    /// MODIFIES the current (top) level, and `CSI ? u` QUERIES it (the terminal replies
    /// `CSI ? flags u`). Every stored/reported value is MASKED to [`KITTY_KEYBOARD_SUPPORTED`] — the
    /// flags sprag can encode truthfully — so an unsupported bit is dropped at negotiation time and
    /// a query never reports a capability the encoder does not honor. The active flags (the top of
    /// the stack) reach the key encoder via [`VtPort::input_modes`]. Carries no cells (no damage).
    fn kitty_keyboard(&mut self, k: Keyboard) {
        match k {
            // `CSI > flags u` — push a new level (bounded against a runaway pusher).
            Keyboard::PushKittyState { flags, .. } => {
                if self.kitty_kbd_stack.len() < KITTY_STACK_CAP {
                    let bits = (flags.bits() as u8) & KITTY_KEYBOARD_SUPPORTED;
                    self.kitty_kbd_stack.push(bits);
                }
            }
            // `CSI < n u` — pop n levels (n == 0 pops one, per the protocol; saturating).
            Keyboard::PopKittyState(n) => {
                let n = usize::try_from(n).unwrap_or(usize::MAX).max(1);
                let keep = self.kitty_kbd_stack.len().saturating_sub(n);
                self.kitty_kbd_stack.truncate(keep);
            }
            // `CSI = flags ; mode u` — modify the CURRENT level (creating a base 0 level if the
            // stack is empty, since a set implies an active entry to set).
            Keyboard::SetKittyState { flags, mode } => {
                let requested = (flags.bits() as u8) & KITTY_KEYBOARD_SUPPORTED;
                if self.kitty_kbd_stack.is_empty() {
                    self.kitty_kbd_stack.push(0);
                }
                let top = self
                    .kitty_kbd_stack
                    .last_mut()
                    .expect("just ensured non-empty");
                *top = match mode {
                    KittyKeyboardMode::AssignAll => requested,
                    KittyKeyboardMode::SetSpecified => *top | requested,
                    KittyKeyboardMode::ClearSpecified => *top & !requested,
                };
            }
            // `CSI ? u` — report the current flags back to the child.
            Keyboard::QueryKittySupport => {
                let current = self.kitty_keyboard_flags().bits();
                self.responses
                    .extend_from_slice(format!("\x1b[?{current}u").as_bytes());
            }
            // A child would not send us a report; ignore.
            Keyboard::ReportKittyState(_) => {}
        }
    }

    /// The CURRENTLY active Kitty keyboard flags — the top of the negotiation stack, already masked
    /// to the supported set, or empty when no level is pushed. Exposed via [`VtPort::input_modes`].
    fn kitty_keyboard_flags(&self) -> KittyKeyboardFlags {
        KittyKeyboardFlags::from_bits(self.kitty_kbd_stack.last().copied().unwrap_or(0))
    }

    /// Answer a Device Attributes / Device Status query on the device-response channel
    /// ([`Self::responses`], written back to the pty by the reader). The child probes the terminal's
    /// identity/status at startup and the terminal replies as if it typed the answer — the reverse
    /// channel [`VtPort::take_responses`] drains. Out of the DA/DSR subset (SoftReset/DECSTR,
    /// tertiary DA, terminal name/version, DECREQTPARM, graphics-attrs) stay dropped, unchanged.
    fn device(&mut self, dev: &Device) {
        match dev {
            // Primary DA (`CSI c`) — who are you + what can you do.
            Device::RequestPrimaryDeviceAttributes => {
                self.responses.extend_from_slice(PRIMARY_DA);
            }
            // Secondary DA (`CSI > c`) — terminal type / firmware version.
            Device::RequestSecondaryDeviceAttributes => {
                self.responses.extend_from_slice(SECONDARY_DA);
            }
            // DSR status (`CSI 5 n`) — report "ready". The cursor-position report (`CSI 6 n`) is a
            // `CSI::Cursor(RequestActivePositionReport)`, answered in `cursor_op`.
            Device::StatusReport => {
                self.responses.extend_from_slice(DSR_OK);
            }
            _ => {}
        }
    }

    fn sgr(&mut self, sgr: Sgr) {
        match sgr {
            Sgr::Reset => {
                self.fg = Color::Default;
                self.bg = Color::Default;
                self.underline_color = None;
                self.attrs = Attrs::default();
            }
            Sgr::Intensity(Intensity::Bold) => {
                self.attrs.bold = true;
                self.attrs.dim = false;
            }
            Sgr::Intensity(Intensity::Half) => {
                self.attrs.dim = true;
                self.attrs.bold = false;
            }
            Sgr::Intensity(Intensity::Normal) => {
                self.attrs.bold = false;
                self.attrs.dim = false;
            }
            Sgr::Underline(u) => self.attrs.underline = conv_underline(u),
            Sgr::UnderlineColor(c) => self.underline_color = conv_underline_color(c),
            Sgr::Blink(b) => self.attrs.blink = b != Blink::None,
            Sgr::Italic(on) => self.attrs.italic = on,
            Sgr::Inverse(on) => self.attrs.reverse = on,
            Sgr::Invisible(on) => self.attrs.hidden = on,
            Sgr::StrikeThrough(on) => self.attrs.strikethrough = on,
            Sgr::Foreground(c) => self.fg = conv_color(c),
            Sgr::Background(c) => self.bg = conv_color(c),
            // Font, Overline, VerticalAlign: ignored.
            _ => {}
        }
    }

    fn cursor_op(&mut self, c: CsiCursor) {
        let max_col = self.cols.saturating_sub(1);
        let max_row = self.rows.saturating_sub(1);
        match c {
            CsiCursor::Up(n) => self.row = self.row.saturating_sub(clamp_count(n)),
            CsiCursor::Down(n) => self.row = (self.row + clamp_count(n)).min(max_row),
            CsiCursor::Left(n) => self.col = self.col.saturating_sub(clamp_count(n)),
            CsiCursor::Right(n) => self.col = (self.col + clamp_count(n)).min(max_col),
            CsiCursor::Position { line, col } => {
                self.row = zero_based_u16(line.as_zero_based()).min(max_row);
                self.col = zero_based_u16(col.as_zero_based()).min(max_col);
            }
            CsiCursor::CharacterAbsolute(c) | CsiCursor::CharacterPositionAbsolute(c) => {
                self.col = zero_based_u16(c.as_zero_based()).min(max_col);
            }
            CsiCursor::LinePositionAbsolute(n) => {
                self.row = zero_based_u16(n.saturating_sub(1)).min(max_row);
            }
            CsiCursor::NextLine(n) => {
                self.row = (self.row + clamp_count(n)).min(max_row);
                self.col = 0;
            }
            CsiCursor::PrecedingLine(n) => {
                self.row = self.row.saturating_sub(clamp_count(n));
                self.col = 0;
            }
            // DECSC / DECRC in their `CSI s` / `CSI u` spelling (same save/restore as `ESC 7/8`).
            CsiCursor::SaveCursor => self.save_cursor(),
            CsiCursor::RestoreCursor => self.restore_cursor(),
            // DECSCUSR — the cursor SHAPE (block / underline / bar); blink is not modeled, so the
            // steady and blinking variants of each shape map to the same shape.
            CsiCursor::CursorStyle(style) => self.cursor_shape = cursor_shape_of(style),
            // DECSTBM — set the top/bottom scroll margins (`SetLeftAndRightMargins`, DECSLRM,
            // stays out of the subset).
            CsiCursor::SetTopAndBottomMargins { top, bottom } => {
                self.set_scroll_region(top.as_zero_based(), bottom.as_zero_based());
            }
            // CPR — Cursor Position Report (`CSI 6 n`): answer the 1-based cursor row;col on the
            // device-response channel (`CSI r ; c R`). Origin mode (DECOM) is not modeled, so this is
            // the absolute screen position — a documented bound. `ActivePositionReport` (the reply
            // form a child never sends us) stays dropped in the wildcard below.
            CsiCursor::RequestActivePositionReport => {
                let report = format!("\x1b[{};{}R", self.row + 1, self.col + 1);
                self.responses.extend_from_slice(report.as_bytes());
            }
            _ => {}
        }
    }

    fn edit(&mut self, e: Edit) {
        match e {
            Edit::EraseInLine(mode) => {
                let g = self.next_gen();
                let (start, end) = match mode {
                    EraseInLine::EraseToEndOfLine => (self.col, self.cols),
                    EraseInLine::EraseToStartOfLine => (0, self.col.saturating_add(1)),
                    EraseInLine::EraseLine => (0, self.cols),
                };
                let row = self.row;
                for c in start..end.min(self.cols) {
                    self.screen.set_cell(c, row, Cell::blank(), g);
                }
                // A line editor's resize redraw opens with erase-to-end-of-line
                // (`ESC [ K`) at the active line's head, then reprints the whole
                // wrapped line. Clear that line's stale continuation rows too (one
                // atomic, invariant-safe op on the `Screen`), so the prior width's
                // tail — which the reprint may only partly cover — does not linger.
                // Scoped to that exact idiom: only `EraseToEndOfLine`, only during a
                // redraw (`in_resize_redraw`); a plain erase touches one row.
                if self.in_resize_redraw && matches!(mode, EraseInLine::EraseToEndOfLine) {
                    self.screen.clear_soft_wrap_continuation(row, g);
                }
                // Erasing to the right margin truncates the line, so it no
                // longer soft-wraps onto the next row.
                if end >= self.cols {
                    self.screen.set_wrapped(row, false);
                }
            }
            Edit::EraseInDisplay(mode) => {
                let g = self.next_gen();
                match mode {
                    EraseInDisplay::EraseToEndOfDisplay => {
                        let row = self.row;
                        for c in self.col..self.cols {
                            self.screen.set_cell(c, row, Cell::blank(), g);
                        }
                        for r in (row + 1)..self.rows {
                            self.screen.clear_row(r, g);
                        }
                    }
                    EraseInDisplay::EraseToStartOfDisplay => {
                        for r in 0..self.row {
                            self.screen.clear_row(r, g);
                        }
                        let row = self.row;
                        for c in 0..=self.col.min(self.cols.saturating_sub(1)) {
                            self.screen.set_cell(c, row, Cell::blank(), g);
                        }
                    }
                    EraseInDisplay::EraseDisplay => {
                        for r in 0..self.rows {
                            self.screen.clear_row(r, g);
                        }
                        // Stage-1 image lifecycle: a whole-screen clear drops inline images too
                        // (they carry no cells, so the row clears above don't touch them). Scroll
                        // / erase-covered-cells / reflow eviction is a later stage.
                        self.screen.clear_images();
                    }
                    // ED-3: drop the retained scrollback (R16 models it).
                    EraseInDisplay::EraseScrollback => self.screen.clear_scrollback(),
                }
            }
            // ICH — insert n blanks at the cursor, shifting the rest of the row right.
            Edit::InsertCharacter(n) => {
                let g = self.next_gen();
                self.screen
                    .insert_cells(self.col, self.row, clamp_count(n), g);
            }
            // DCH — delete n cells at the cursor, shifting the rest of the row left.
            Edit::DeleteCharacter(n) => {
                let g = self.next_gen();
                self.screen
                    .delete_cells(self.col, self.row, clamp_count(n), g);
            }
            // ECH — blank n cells at the cursor in place (no shift).
            Edit::EraseCharacter(n) => {
                let g = self.next_gen();
                self.screen
                    .erase_cells(self.col, self.row, clamp_count(n), g);
            }
            // REP — reprint the last graphic char n times (a no-op before any print).
            Edit::Repeat(n) => {
                if let Some(ch) = self.last_print {
                    let repeated: String =
                        std::iter::repeat_n(ch, clamp_count(n) as usize).collect();
                    self.print_str(&repeated);
                }
            }
            // IL — insert n blank lines at the cursor, within the scroll region: rows from
            // the cursor down to the bottom margin shift down, the tail falling past the
            // margin. A no-op when the cursor is outside the region (the VT100 rule). The
            // active position moves to the line home (column 0) per ECMA-48.
            Edit::InsertLine(n) => {
                if self.row >= self.scroll_top && self.row <= self.scroll_bottom {
                    let g = self.next_gen();
                    self.screen
                        .scroll_region_down(self.row, self.scroll_bottom, clamp_count(n), g);
                    self.col = 0;
                }
            }
            // DL — delete n lines at the cursor, within the scroll region: rows below shift
            // up to the cursor, blanks opening at the bottom margin. A no-op outside the
            // region; column homes to 0. DL is an EDIT, so it never feeds the scrollback —
            // even a DL at row 0 removes the line rather than scrolling it off the top.
            Edit::DeleteLine(n) => {
                if self.row >= self.scroll_top && self.row <= self.scroll_bottom {
                    let g = self.next_gen();
                    self.screen.scroll_region_up(
                        self.row,
                        self.scroll_bottom,
                        clamp_count(n),
                        false, // an edit, not an output-flow scroll: no scrollback
                        g,
                    );
                    self.col = 0;
                }
            }
            // SU — scroll the region up n lines (data moves up); the cursor does not move.
            // An output-flow scroll, so a top-anchored region feeds the scrollback.
            Edit::ScrollUp(n) => {
                let g = self.next_gen();
                self.screen.scroll_region_up(
                    self.scroll_top,
                    self.scroll_bottom,
                    clamp_count(n),
                    true,
                    g,
                );
            }
            // SD — scroll the region down n lines (data moves down); the cursor does not
            // move. A down scroll discards the bottom line, never the top, so no scrollback.
            Edit::ScrollDown(n) => {
                let g = self.next_gen();
                self.screen.scroll_region_down(
                    self.scroll_top,
                    self.scroll_bottom,
                    clamp_count(n),
                    g,
                );
            }
        }
    }

    fn mode(&mut self, m: Mode) {
        match m {
            Mode::SetDecPrivateMode(DecPrivateMode::Code(code)) => match code {
                DecPrivateModeCode::ShowCursor => self.cursor_visible = true,
                DecPrivateModeCode::ApplicationCursorKeys => {
                    self.input_modes.application_cursor_keys = true;
                }
                DecPrivateModeCode::ClearAndEnableAlternateScreen
                | DecPrivateModeCode::EnableAlternateScreen
                | DecPrivateModeCode::OptEnableAlternateScreen => self.enter_alt(),
                DecPrivateModeCode::BracketedPaste => {
                    self.input_modes.bracketed_paste = true;
                }
                // DECSET 1000 / 1002 / 1003 — X11 mouse tracking at three levels: press+release
                // (Click), + DRAG while a button is held (ButtonEvent), + BARE motion (AnyEvent).
                // sprag keeps ONE highest-active protocol field rather than xterm's independent
                // mode bits (documented bound on `MouseProtocol`): a child sets exactly one level
                // and resets the same, which this models faithfully; only a pathological stack
                // (set two, reset the higher) differs.
                DecPrivateModeCode::MouseTracking => {
                    self.input_modes.mouse_protocol = MouseProtocol::Click;
                }
                DecPrivateModeCode::ButtonEventMouse => {
                    self.input_modes.mouse_protocol = MouseProtocol::ButtonEvent;
                }
                DecPrivateModeCode::AnyEventMouse => {
                    self.input_modes.mouse_protocol = MouseProtocol::AnyEvent;
                }
                // DECSET 1006 — SGR mouse encoding. Orthogonal to the tracking mode: a child sets a
                // tracking mode AND (optionally) this encoding.
                DecPrivateModeCode::SGRMouse => {
                    self.input_modes.mouse_encoding = MouseEncoding::Sgr;
                }
                // DECSET 1004 — focus reporting. Orthogonal to mouse tracking: the terminal sends
                // ESC [ I / ESC [ O when the pane gains / loses focus.
                DecPrivateModeCode::FocusTracking => {
                    self.input_modes.focus_tracking = true;
                }
                _ => {}
            },
            Mode::ResetDecPrivateMode(DecPrivateMode::Code(code)) => match code {
                DecPrivateModeCode::ShowCursor => self.cursor_visible = false,
                DecPrivateModeCode::ApplicationCursorKeys => {
                    self.input_modes.application_cursor_keys = false;
                }
                DecPrivateModeCode::ClearAndEnableAlternateScreen
                | DecPrivateModeCode::EnableAlternateScreen
                | DecPrivateModeCode::OptEnableAlternateScreen => self.exit_alt(),
                DecPrivateModeCode::BracketedPaste => {
                    self.input_modes.bracketed_paste = false;
                }
                // DECRST 1000 / 1002 / 1003 — stop mouse tracking at any level, hand the mouse back
                // to the terminal (single-field model: any tracking reset returns to None).
                DecPrivateModeCode::MouseTracking
                | DecPrivateModeCode::ButtonEventMouse
                | DecPrivateModeCode::AnyEventMouse => {
                    self.input_modes.mouse_protocol = MouseProtocol::None;
                }
                // DECRST 1006 — back to the legacy X10 encoding (the tracking mode is unaffected).
                DecPrivateModeCode::SGRMouse => {
                    self.input_modes.mouse_encoding = MouseEncoding::X10;
                }
                // DECRST 1004 — stop focus reporting.
                DecPrivateModeCode::FocusTracking => {
                    self.input_modes.focus_tracking = false;
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn enter_alt(&mut self) {
        if self.saved_main.is_none() {
            let mut alt = Screen::new(self.cols, self.rows);
            alt.set_kind(ScreenKind::Alternate);
            let main = std::mem::replace(&mut self.screen, alt);
            self.saved_main = Some(main);
            self.col = 0;
            self.row = 0;
            // The alt screen is a fresh buffer; it starts with the full-screen region.
            self.reset_scroll_region();
        }
    }

    fn exit_alt(&mut self) {
        if let Some(main) = self.saved_main.take() {
            self.screen = main;
            let cur = self.screen.cursor();
            self.col = cur.col.min(self.cols.saturating_sub(1));
            self.row = cur.row.min(self.rows.saturating_sub(1));
            // DECSTBM is not part of the saved cursor state; the restored main screen
            // resumes with the full-screen region (a well-behaved app reset it on exit).
            self.reset_scroll_region();
        }
    }

    /// Print one or more graphemes, advancing the cursor with autowrap.
    fn print_str(&mut self, s: &str) {
        // Char-level is sufficient for the skeleton; ZWJ emoji clusters
        // are a known gap (DESIGN.md §5 — logged, not silently capped).
        for ch in s.chars() {
            let w = char_columns(ch); // the one width authority (port::char_columns)
            if w == 0 {
                // Combining mark: merge into the previous cell if possible.
                self.merge_combining(ch);
                continue;
            }
            let cell_w = w as u16;
            if self.col + cell_w > self.cols {
                // Autowrap: this row's logical line continues onto the next.
                self.screen.set_wrapped(self.row, true);
                self.col = 0;
                self.line_feed();
            }
            let g = self.next_gen();
            let head = Cell {
                cluster: ch.to_string(),
                fg: self.fg,
                bg: self.bg,
                underline_color: self.underline_color,
                attrs: self.attrs,
                // Stamp the OSC-8 pen: a link and every cell it covers (including
                // wrap continuations printed on later rows) share this one `Arc`.
                hyperlink: self.current_hyperlink.clone(),
                width: if cell_w == 2 {
                    Width::Wide
                } else {
                    Width::Narrow
                },
            };
            let (col, row) = (self.col, self.row);
            if cell_w == 2 && col + 1 < self.cols {
                self.screen
                    .set_cell(col + 1, row, Cell::trailer_for(&head), g);
            }
            self.screen.set_cell(col, row, head, g);
            self.col += cell_w;
            // Remember the last graphic char for REP (`CSI b`).
            self.last_print = Some(ch);
        }
    }

    fn merge_combining(&mut self, ch: char) {
        if self.col == 0 {
            return;
        }
        let (col, row) = (self.col - 1, self.row);
        if let Some(prev) = self.screen.cell(col, row) {
            let mut merged = prev.clone();
            merged.cluster.push(ch);
            let g = self.next_gen();
            self.screen.set_cell(col, row, merged, g);
        }
    }

    /// Move the cursor down one line (IND / the LF part of a line feed). At the bottom
    /// margin this scrolls the scroll region up by one — for the default full-screen region
    /// that is the ordinary "output flows off the top into scrollback" scroll. Below the
    /// bottom margin (a cursor parked over a fixed footer) it advances until the last row,
    /// then stops; it never scrolls a region it is not the bottom of.
    fn line_feed(&mut self) {
        if self.row == self.scroll_bottom {
            let g = self.next_gen();
            self.screen
                .scroll_region_up(self.scroll_top, self.scroll_bottom, 1, true, g);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    /// Publish the tracked cursor into the screen (call after a batch).
    fn sync_cursor(&mut self) {
        self.screen.set_cursor(Cursor {
            col: self.col.min(self.cols.saturating_sub(1)),
            row: self.row.min(self.rows.saturating_sub(1)),
            shape: self.cursor_shape,
            visible: self.cursor_visible,
        });
    }
}

impl VtPort for Emulator {
    fn advance(&mut self, bytes: &[u8]) {
        // Parse the whole batch into actions first, then apply: this
        // avoids borrowing `self.parser` and `self` simultaneously.
        let actions = self.parser.parse_as_vec(bytes);
        for action in actions {
            self.apply(action);
        }
        // The line editor's resize redraw follows a resize (see `in_resize_redraw`). While the
        // shell reports (OSC 133) it is still AT A PROMPT, the redraw may span several PTY-read
        // batches, so keep the window open across them; otherwise close it at this batch's end —
        // a non-integrated shell (Unknown) falls back to the single-batch window, and a shell that
        // has left the prompt to run a command is producing output, not a redraw. `note_input`
        // closes it the moment the consumer sends input, so a submitting CR LF stays a hard break.
        if self.screen.shell_state() != ShellState::AtPrompt {
            self.in_resize_redraw = false;
        }
        self.sync_cursor();
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        // Reflow rewraps the visible MAIN screen's logical lines to the new width
        // (the alt screen / degenerate sizes fall back to a verbatim copy inside
        // `reflowed`). A fresh damage stamp marks every re-laid-out row.
        let g = self.next_gen();
        let reflowed = self.screen.reflowed(cols, rows, g);
        // Adopt the cursor re-derived from the reflow (clamped for the verbatim
        // alt-screen path, a no-op for the in-bounds reflow path).
        self.col = reflowed.cursor().col.min(cols - 1);
        self.row = reflowed.cursor().row.min(rows - 1);
        self.screen = reflowed;
        if let Some(main) = &self.saved_main {
            self.saved_main = Some(main.reflowed(cols, rows, g));
        }
        self.cols = cols;
        self.rows = rows;
        // The scroll region was defined against the old geometry; a resize returns it to
        // the full new screen (apps re-issue DECSTBM if they still want a sub-region).
        self.reset_scroll_region();
        // The next batch of bytes is the line editor's `SIGWINCH` redraw; apply the
        // soft-wrap / erase reinterpretations to it (see `in_resize_redraw`). Only
        // the MAIN screen runs a line editor; a fullscreen app owns the alt screen.
        self.in_resize_redraw = self.screen.screen_kind() == ScreenKind::Main;
        self.sync_cursor();
    }

    fn note_input(&mut self) {
        // The user acted (typed / submitted / pasted): the line editor's resize redraw is over, so
        // close the reinterpretation window. The transport calls this before writing the bytes to
        // the child, so the child's echo / prompt / command output is emulated with the epoch
        // closed — a submitting CR LF is a hard break again. See `in_resize_redraw`.
        self.in_resize_redraw = false;
    }

    fn screen(&self) -> &Screen {
        &self.screen
    }

    fn input_modes(&self) -> InputModes {
        // The Kitty keyboard flags live in the negotiation stack (their SSOT); overlay the current
        // top onto the mode flags the key encoder reads.
        InputModes {
            kitty_keyboard: self.kitty_keyboard_flags(),
            ..self.input_modes
        }
    }

    fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    fn notification(&self) -> Option<&Notification> {
        self.notification.as_ref()
    }

    fn notification_seq(&self) -> u64 {
        self.notification_seq
    }

    fn bell_seq(&self) -> u64 {
        self.bell_seq
    }

    fn clipboard_write(&self) -> Option<&ClipboardWrite> {
        self.clipboard_write.as_ref()
    }

    fn clipboard_write_seq(&self) -> u64 {
        self.clipboard_write_seq
    }

    fn clipboard_query(&self) -> Option<ClipboardQuery> {
        self.clipboard_query
    }

    fn clipboard_query_seq(&self) -> u64 {
        self.clipboard_query_seq
    }

    fn take_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.responses)
    }
}

/// The Kitty keyboard protocol enhancement flags sprag ADVERTISES + honors — currently only
/// *Disambiguate escape codes*. A child may request more; the negotiation masks to this so the
/// terminal never claims (via a `CSI ? u` reply) a flag its key encoder does not implement.
const KITTY_KEYBOARD_SUPPORTED: u8 = KittyKeyboardFlags::DISAMBIGUATE;

/// Depth cap on the Kitty keyboard flag stack — bounds memory against a child that pushes without
/// popping. Deep real nesting is a handful; past the cap further pushes are ignored (the current
/// level holds), which degrades safely rather than growing unbounded.
const KITTY_STACK_CAP: usize = 32;

/// Cap on the OSC-8 `id=` intern cache ([`Emulator::hyperlink_ids`]) — bounds memory against a
/// child that emits an unbounded stream of distinct link ids. A screenful of distinct grouped links
/// is small; past the cap a new id gets a fresh (ungrouped) `Arc` rather than being cached, which
/// degrades the non-adjacent-grouping nicety safely rather than growing without bound.
const HYPERLINK_ID_CAP: usize = 4096;

/// Primary Device Attributes reply (`CSI c` / `CSI 0 c`): `CSI ? 62 ; 4 ; 22 c` — identify as a
/// VT220-class terminal (`62`) advertising ONLY the extensions sprag actually honors: Sixel graphics
/// (`4`, the image round renders it) and ANSI colour (`22`). Same truthful-capability discipline as
/// the Kitty flags mask — never claim a feature the emulator drops, so an app's DA-gated code path
/// only turns on what works (e.g. Sixel auto-detect, a tmux-superior win — tmux has no Sixel).
const PRIMARY_DA: &[u8] = b"\x1b[?62;4;22c";

/// Secondary Device Attributes reply (`CSI > c` / `CSI > 0 c`): `CSI > 0 ; 0 ; 0 c` — terminal type
/// `0` (VT100-family), firmware version `0`, ROM cartridge `0`. Deliberately does NOT impersonate a
/// specific xterm/rxvt build an app might feature-gate on a version threshold; a generic identity so
/// version-sniffing apps fall back to their portable path rather than enabling something sprag drops.
const SECONDARY_DA: &[u8] = b"\x1b[>0;0;0c";

/// Device Status Report "ready, no malfunction" reply (`CSI 5 n` -> `CSI 0 n`).
const DSR_OK: &[u8] = b"\x1b[0n";

/// Map a termwiz OSC 52 [`Selection`] set to the [`ClipboardTargets`] a WRITE addresses. The
/// clipboard (`c`) maps to the clipboard; the "configured selection" (`s`) and the empty-`Pc`
/// default (which termwiz expands to `s` + cut buffer 0) fold onto the clipboard too, matching
/// the common intent that an unqualified copy be pasteable. PRIMARY (`p`) maps to primary. The X
/// cut buffers (`0`-`9`) alone have no windowing analog and map to nothing (an empty set).
fn sel_to_targets(sel: &Selection) -> ClipboardTargets {
    ClipboardTargets {
        clipboard: sel.contains(Selection::CLIPBOARD) || sel.contains(Selection::SELECT),
        primary: sel.contains(Selection::PRIMARY),
    }
}

/// Reduce an OSC 52 READ [`Selection`] to the single [`ClipboardTarget`] a reply carries. A reply
/// echoes one selection, so a query naming several resolves by priority: PRIMARY only when it is
/// the sole modeled selection asked for, else the clipboard (covering `c`, `s`, the empty
/// default, and any cut-buffer-only request that has no analog to read).
fn sel_to_query_target(sel: &Selection) -> ClipboardTarget {
    let clipboard = sel.contains(Selection::CLIPBOARD) || sel.contains(Selection::SELECT);
    if sel.contains(Selection::PRIMARY) && !clipboard {
        ClipboardTarget::Primary
    } else {
        ClipboardTarget::Clipboard
    }
}

/// Build the framed OSC 52 reply bytes that answer a clipboard READ (`OSC 52 ; <sel> ; ?`):
/// `ESC ] 52 ; <c|p> ; <base64(text)> ST`. The whole wire form — the `ESC ]` introducer, the
/// selection token, the base64 encoding, and the `ST` terminator — is termwiz's own
/// [`OperatingSystemCommand`] emission, the SSOT that mirrors the parser sprag reads OSC 52
/// through, so an encode here and the child's decode agree. This is the ONE place sprag emits
/// the OSC 52 wire form: a display client fetches these bytes and feeds them back to the pane's
/// PTY (the child receives its reply as if the terminal typed it).
#[must_use]
pub fn osc52_reply(target: ClipboardTarget, text: &str) -> Vec<u8> {
    let sel = match target {
        ClipboardTarget::Clipboard => Selection::CLIPBOARD,
        ClipboardTarget::Primary => Selection::PRIMARY,
    };
    OperatingSystemCommand::SetSelection(sel, text.to_string())
        .to_string()
        .into_bytes()
}

/// Parse a kitty `OSC 99` desktop notification from termwiz's raw `Unspecified` params
/// (`[b"99", b"<metadata>", b"<payload>", …]`), returning `(title, body)` or `None` when it
/// is not an `OSC 99` or carries no capturable text.
///
/// Kitty's form is `OSC 99 ; <metadata> ; <payload>`, where `<metadata>` is `k=v:k=v` pairs.
/// This handles the COMMON single-chunk case and reads two keys:
///
/// * `p` — payload type: `title` (kitty's default) or `body`; other types (`icon`, `close`,
///   `buttons`, …) are not text to show, so they are dropped.
/// * `e` — encoding: `1` means the payload is base64. sprag has no base64 decoder in this
///   layer, so an encoded payload is dropped rather than shown as gibberish.
///
/// BOUNDS (honestly limited, not misrepresented): a MULTI-CHUNK notification (`d=0`, streamed
/// across several `OSC 99`s) is NOT reassembled — each chunk is read independently, so a body
/// split across chunks yields only its first piece; `i`/`d`/actions are ignored. These are the
/// advanced-protocol tail; the single unencoded chunk is what shells and CLIs emit in practice.
fn parse_kitty_notification(params: &[Vec<u8>]) -> Option<(Option<String>, String)> {
    // Only OSC 99; anything else in `Unspecified` is some other unhandled OSC.
    if params.first().map(Vec::as_slice) != Some(b"99".as_slice()) {
        return None;
    }
    let metadata = params.get(1).map(Vec::as_slice).unwrap_or(b"");
    // The payload is everything after the second `;`; termwiz split it on `;`, so rejoin
    // (a plain-text payload may itself contain a semicolon).
    if params.len() < 3 {
        return None;
    }
    let payload_bytes = params[2..].join(&b';');
    let payload = String::from_utf8_lossy(&payload_bytes);

    let mut payload_type = "title"; // kitty's default when `p` is absent.
    let mut base64 = false;
    for pair in metadata.split(|&b| b == b':') {
        let mut kv = pair.splitn(2, |&b| b == b'=');
        let key = kv.next().unwrap_or(b"");
        let value = kv.next().unwrap_or(b"");
        match key {
            b"p" => {
                payload_type = match value {
                    b"body" => "body",
                    b"title" => "title",
                    _ => return None, // icon / close / buttons / … : nothing to display.
                };
            }
            b"e" if value == b"1" => base64 = true,
            _ => {}
        }
    }
    if base64 {
        return None; // encoded payload: not decoded in this layer (see the doc bound).
    }
    match payload_type {
        "body" => Some((None, payload.into_owned())),
        // Default / explicit title: the heading, with no separate body in this chunk.
        _ => Some((Some(payload.into_owned()), String::new())),
    }
}

/// Upper bound on a stored child window title. A title is a single taskbar / titlebar
/// line, so a few KiB is generous; the cap exists to stop a hostile or runaway child from
/// growing an unbounded `String` (see [`Emulator::osc`]). Bytes, not chars — the truncation
/// respects a UTF-8 boundary.
const MAX_TITLE_BYTES: usize = 2048;

/// Upper bound on a stored notification field (title or body). A notification is a short
/// desktop toast, so the same few-KiB budget as a window title is generous; the cap exists
/// for the same reason (a child-controlled string stored, cloned per poll wake, and shipped
/// over the wire must be bounded — see [`Emulator::osc`]).
const MAX_NOTIFICATION_BYTES: usize = 2048;

/// Upper bound on a stored OSC 52 clipboard WRITE payload. A clipboard write is legitimately far
/// larger than a title or a toast — a paste can be a whole file — so the cap is generous (1 MiB),
/// but it exists for the same reason (a child-controlled string, latched and cloned on demand,
/// must be bounded so a hostile or runaway child cannot pin unbounded memory). An over-cap write
/// is truncated on a char boundary; this is a rare, documented bound.
const MAX_CLIPBOARD_BYTES: usize = 1 << 20;

/// Upper bound on a single inline image's RGBA raster (16 MiB = a 2048x2048 image). Same reason as
/// the clipboard cap: a child-controlled buffer must be bounded so a hostile / runaway child cannot
/// pin unbounded memory — and in Stage 1 an image rides the panes slot INLINE (base64) on every
/// poll, so an uncapped raster would flood the wire, not just the store. An over-cap image is
/// DROPPED (never stored / wired / painted); large images await on-demand transport (a later stage).
const MAX_IMAGE_BYTES: usize = 16 << 20;

/// Upper bound on a Kitty CHUNKED transmission's accumulated base64 (`m=1` reassembly, Stage 4).
/// A 16 MiB RGBA raster base64-encodes to ~21.3 MiB, so 24 MiB gives slack; a transmission that
/// exceeds it (a runaway or never-closing chunk sequence) is dropped before it can pin memory.
const MAX_KITTY_CHUNK_BYTES: usize = 24 << 20;

/// Whether an image's RGBA raster (`width * height * 4`) fits within [`MAX_IMAGE_BYTES`] — `false`
/// for an over-cap size OR one that overflows `usize`. The guard [`Emulator::decode_image`] applies
/// before it decodes / stores a child-controlled image.
fn image_within_cap(width: u32, height: u32) -> bool {
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .is_some_and(|bytes| bytes <= MAX_IMAGE_BYTES)
}

/// One in-flight Kitty CHUNKED transmission ([`Emulator::kitty_chunks`], Stage 4): the accumulated
/// base64 payload plus the FIRST chunk's format / dimensions / display intent.
struct KittyChunk {
    base64: String,
    format: Option<KittyImageFormat>,
    width: Option<u32>,
    height: Option<u32>,
    display: bool,
    id: u32,
}

/// Decode a PNG payload to `(width, height, RGBA8)` (Kitty `f=100`, R1404 Stage 4), or `None` on a
/// malformed / oversized / unsupported PNG (dropped, never misrendered). Reads the PNG's OWN
/// dimensions and applies [`image_within_cap`] before allocating; `EXPAND` + `STRIP_16` normalise
/// palette / low-bit / 16-bit inputs to 8-bit, then every colour type folds to RGBA8.
fn decode_png(bytes: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    let (width, height) = (reader.info().width, reader.info().height);
    if !image_within_cap(width, height) {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let px = (width as usize).checked_mul(height as usize)?;
    let mut rgba = Vec::with_capacity(px.checked_mul(4)?);
    match info.color_type {
        png::ColorType::Rgba => return Some((width, height, buf)),
        png::ColorType::Rgb => {
            for c in buf.chunks_exact(3) {
                rgba.extend_from_slice(c);
                rgba.push(255);
            }
        }
        png::ColorType::Grayscale => {
            for g in &buf {
                rgba.extend_from_slice(&[*g, *g, *g, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for ga in buf.chunks_exact(2) {
                rgba.extend_from_slice(&[ga[0], ga[0], ga[0], ga[1]]);
            }
        }
        // EXPAND removes the palette; a residual indexed frame is unexpected — drop, not misrender.
        png::ColorType::Indexed => return None,
    }
    Some((width, height, rgba))
}

/// Rasterize a termwiz [`Sixel`] into `(width, height, rgba)`, or `None` when it is empty or its
/// raster would exceed [`MAX_IMAGE_BYTES`]. Standard sixel semantics: a data byte's low six bits are
/// six VERTICAL pixels (bit 0 = topmost) painted in the current colour at the current column;
/// [`Repeat`](SixelData::Repeat) paints the same byte across a run of columns; `$`
/// ([`CarriageReturn`](SixelData::CarriageReturn)) returns to column 0 in the same 6-row band; `-`
/// ([`NewLine`](SixelData::NewLine)) drops to the next band. `#n;2;r;g;b`
/// ([`DefineColorMapRGB`](SixelData::DefineColorMapRGB)) both defines register `n` AND selects it;
/// `#n` ([`SelectColorMapEntry`](SixelData::SelectColorMapEntry)) selects. The background is
/// transparent — an unpainted pixel keeps alpha `0`. Registers seed the VT340 default 16-colour
/// palette; an undefined index paints black.
fn raster_sixel(sixel: &Sixel) -> Option<(u32, u32, Vec<u8>)> {
    let (width, height) = sixel.dimensions();
    if width == 0 || height == 0 || !image_within_cap(width, height) {
        return None;
    }
    let w = width as usize;
    let mut rgba = vec![0u8; w * height as usize * 4];
    let mut registers = default_sixel_palette();
    let mut color = *registers.get(&0).unwrap_or(&(0, 0, 0));
    let mut x: u32 = 0;
    let mut band_top: u32 = 0;
    // Paint a data byte's six vertical pixels at column `x` in `color`. Bit 0 is the topmost.
    let paint = |rgba: &mut [u8], x: u32, value: u8, band_top: u32, color: (u8, u8, u8)| {
        for bit in 0..6u32 {
            if value & (1 << bit) != 0 && x < width && band_top + bit < height {
                let idx = ((band_top + bit) as usize * w + x as usize) * 4;
                rgba[idx] = color.0;
                rgba[idx + 1] = color.1;
                rgba[idx + 2] = color.2;
                rgba[idx + 3] = 255;
            }
        }
    };
    for item in &sixel.data {
        match item {
            SixelData::Data(v) => {
                paint(&mut rgba, x, *v, band_top, color);
                x += 1;
            }
            SixelData::Repeat { repeat_count, data } => {
                for _ in 0..*repeat_count {
                    paint(&mut rgba, x, *data, band_top, color);
                    x += 1;
                }
            }
            SixelData::SelectColorMapEntry(n) => color = *registers.get(n).unwrap_or(&(0, 0, 0)),
            SixelData::DefineColorMapRGB { color_number, rgb } => {
                let c = rgb.to_tuple_rgb8();
                registers.insert(*color_number, c);
                color = c; // `#n;2;…` defines AND selects.
            }
            SixelData::DefineColorMapHSL {
                color_number,
                hue_angle,
                lightness,
                saturation,
            } => {
                let c = hsl_to_rgb(*hue_angle, *lightness, *saturation);
                registers.insert(*color_number, c);
                color = c;
            }
            SixelData::CarriageReturn => x = 0,
            SixelData::NewLine => {
                x = 0;
                band_top += 6;
            }
        }
    }
    Some((width, height, rgba))
}

/// The VT340 default sixel colour registers (0-15), converted from the standard percentage triples
/// to 8-bit RGB. Most sixels [`DefineColorMapRGB`](SixelData::DefineColorMapRGB) their own colours;
/// this seeds a sensible palette for one that selects an index it never defined.
fn default_sixel_palette() -> HashMap<u16, (u8, u8, u8)> {
    [
        (0, (0, 0, 0)),
        (1, (51, 51, 204)),
        (2, (204, 33, 33)),
        (3, (51, 204, 51)),
        (4, (204, 51, 204)),
        (5, (51, 204, 204)),
        (6, (204, 204, 51)),
        (7, (135, 135, 135)),
        (8, (66, 66, 66)),
        (9, (84, 84, 153)),
        (10, (153, 66, 66)),
        (11, (84, 153, 84)),
        (12, (153, 84, 153)),
        (13, (84, 153, 153)),
        (14, (153, 153, 84)),
        (15, (204, 204, 204)),
    ]
    .into_iter()
    .collect()
}

/// Convert a sixel HLS colour to 8-bit RGB. Sixel's hue is the DEC convention (0 deg = BLUE), so it
/// is rotated onto the standard HSL wheel (0 deg = red) by +240 deg; lightness / saturation are
/// percentages. Best-effort — sixels overwhelmingly use RGB (`;2;`), so HSL is a rarely-exercised
/// path and its exact DEC rounding is a documented bound.
fn hsl_to_rgb(hue_angle: u16, lightness: u8, saturation: u8) -> (u8, u8, u8) {
    let h = f32::from((hue_angle + 240) % 360) / 360.0;
    let l = f32::from(lightness.min(100)) / 100.0;
    let s = f32::from(saturation.min(100)) / 100.0;
    if s == 0.0 {
        let v = (l * 255.0).round() as u8;
        return (v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let channel = |mut t: f32| -> u8 {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round() as u8
    };
    (channel(h + 1.0 / 3.0), channel(h), channel(h - 1.0 / 3.0))
}

/// Clamp a child-set title to [`MAX_TITLE_BYTES`].
fn clamp_title(t: &str) -> String {
    clamp_bytes(t, MAX_TITLE_BYTES)
}

/// Clamp a child-controlled `String` to `max` BYTES, truncating on a char boundary so the
/// stored value stays valid UTF-8. Most values are far under the cap and clone as-is. Shared
/// by the window title and the notification fields — both are unbounded child input.
fn clamp_bytes(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_owned()
}

/// Map a termwiz DECSCUSR [`CursorStyle`] to the port's [`CursorShape`]. Blink is not modeled, so
/// each shape's steady and blinking variants collapse to the same shape; `Default` is a block (the
/// power-on default).
fn cursor_shape_of(style: CursorStyle) -> CursorShape {
    match style {
        CursorStyle::Default | CursorStyle::BlinkingBlock | CursorStyle::SteadyBlock => {
            CursorShape::Block
        }
        CursorStyle::BlinkingUnderline | CursorStyle::SteadyUnderline => CursorShape::Underline,
        CursorStyle::BlinkingBar | CursorStyle::SteadyBar => CursorShape::Bar,
    }
}

/// Convert a termwiz `ColorSpec` to the port's `Color`.
fn conv_color(spec: ColorSpec) -> Color {
    match spec {
        ColorSpec::Default => Color::Default,
        ColorSpec::PaletteIndex(i) => Color::Indexed(i),
        ColorSpec::TrueColor(srgba) => {
            let (r, g, b, _a) = srgba.to_srgb_u8();
            Color::Rgb(Rgb::new(r, g, b))
        }
    }
}

/// Convert a termwiz `Underline` (SGR 4:x) to the port's [`UnderlineStyle`].
/// The two enums share their six variants one-for-one.
fn conv_underline(u: Underline) -> UnderlineStyle {
    match u {
        Underline::None => UnderlineStyle::None,
        Underline::Single => UnderlineStyle::Single,
        Underline::Double => UnderlineStyle::Double,
        Underline::Curly => UnderlineStyle::Curly,
        Underline::Dotted => UnderlineStyle::Dotted,
        Underline::Dashed => UnderlineStyle::Dashed,
    }
}

/// Convert an SGR 58 / 59 underline colour to the pen's `Option<Color>`.
/// `ColorSpec::Default` is SGR 59 (reset) → `None`: the underline is then
/// drawn in the cell's own foreground, not in `Color::Default` literally.
fn conv_underline_color(spec: ColorSpec) -> Option<Color> {
    match spec {
        ColorSpec::Default => None,
        other => Some(conv_color(other)),
    }
}

/// A movement count: termwiz may emit 0 for an omitted parameter; ANSI
/// treats that as 1.
fn clamp_count(n: u32) -> u16 {
    u16::try_from(n.max(1)).unwrap_or(u16::MAX)
}

fn zero_based_u16(n: u32) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cluster(em: &Emulator, col: u16, row: u16) -> &str {
        em.screen()
            .cell(col, row)
            .map_or("", |c| c.cluster.as_str())
    }

    #[test]
    fn prints_and_advances_cursor() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"hi");
        assert_eq!(cluster(&em, 0, 0), "h");
        assert_eq!(cluster(&em, 1, 0), "i");
        assert_eq!(em.screen().cursor().col, 2);
        assert_eq!(em.screen().cursor().row, 0);
    }

    #[test]
    fn sgr_sets_color_and_attrs() {
        let mut em = Emulator::new(10, 1);
        em.advance(b"\x1b[1;31mA");
        let cell = em.screen().cell(0, 0).unwrap();
        assert_eq!(cell.fg, Color::Indexed(1));
        assert!(cell.attrs.bold);
    }

    #[test]
    fn truecolor_foreground() {
        let mut em = Emulator::new(10, 1);
        em.advance(b"\x1b[38;2;10;20;30mX");
        assert_eq!(
            em.screen().cell(0, 0).unwrap().fg,
            Color::Rgb(Rgb::new(10, 20, 30))
        );
    }

    #[test]
    fn underline_styles_map_each_sgr_variant() {
        // Each ECMA-48 SGR 4:x underline reaches the pen as its own style,
        // not flattened to a single on/off — one char per style at its column.
        let mut em = Emulator::new(20, 1);
        em.advance(b"\x1b[4mA"); // 4    -> single
        em.advance(b"\x1b[4:2mB"); // 4:2  -> double
        em.advance(b"\x1b[4:3mC"); // 4:3  -> curly (undercurl)
        em.advance(b"\x1b[4:4mD"); // 4:4  -> dotted
        em.advance(b"\x1b[4:5mE"); // 4:5  -> dashed
        em.advance(b"\x1b[24mF"); // 24   -> off
        let style = |c| em.screen().cell(c, 0).unwrap().attrs.underline;
        assert_eq!(style(0), UnderlineStyle::Single);
        assert_eq!(style(1), UnderlineStyle::Double);
        assert_eq!(style(2), UnderlineStyle::Curly);
        assert_eq!(style(3), UnderlineStyle::Dotted);
        assert_eq!(style(4), UnderlineStyle::Dashed);
        assert_eq!(style(5), UnderlineStyle::None);
    }

    #[test]
    fn underline_color_sgr_58_is_orthogonal_to_style() {
        // SGR 58 sets the underline colour (truecolor + palette); SGR 59
        // resets it to None (draw in fg) WITHOUT touching the style axis.
        let mut em = Emulator::new(20, 1);
        em.advance(b"\x1b[4;58:2::255:0:0mA"); // single + red underline
        em.advance(b"\x1b[58:5:3mB"); // palette-3 underline, style stays
        em.advance(b"\x1b[59mC"); // reset colour, style stays single
        let a = em.screen().cell(0, 0).unwrap();
        assert_eq!(a.attrs.underline, UnderlineStyle::Single);
        assert_eq!(a.underline_color, Some(Color::Rgb(Rgb::new(255, 0, 0))));
        let b = em.screen().cell(1, 0).unwrap();
        assert_eq!(b.underline_color, Some(Color::Indexed(3)));
        assert_eq!(b.attrs.underline, UnderlineStyle::Single);
        let c = em.screen().cell(2, 0).unwrap();
        assert_eq!(c.underline_color, None);
        assert_eq!(c.attrs.underline, UnderlineStyle::Single);
    }

    #[test]
    fn sgr_reset_clears_underline_style_and_color() {
        let mut em = Emulator::new(20, 1);
        em.advance(b"\x1b[4:3;58:2::0:255:0mX"); // curly + green
        em.advance(b"\x1b[0mY"); // full reset
        let y = em.screen().cell(1, 0).unwrap();
        assert_eq!(y.attrs.underline, UnderlineStyle::None);
        assert_eq!(y.underline_color, None);
    }

    #[test]
    fn decsc_decrc_round_trip_underline_style_and_color() {
        // The underline axes ride the pen, so DECSC/DECRC save and restore
        // them alongside fg/bg/bold — a reset between the two is undone.
        let mut em = Emulator::new(20, 2);
        em.advance(b"\x1b[4:3;58:5:5m"); // pen: curly + palette-5 underline
        em.advance(b"\x1b7"); // DECSC
        em.advance(b"\x1b[0m"); // clobber the pen
        em.advance(b"\x1b8"); // DECRC restores the saved pen + home
        em.advance(b"Z");
        let z = em.screen().cell(0, 0).unwrap();
        assert_eq!(z.attrs.underline, UnderlineStyle::Curly);
        assert_eq!(z.underline_color, Some(Color::Indexed(5)));
    }

    #[test]
    fn wide_cjk_head_and_trailer() {
        let mut em = Emulator::new(10, 1);
        em.advance("世".as_bytes());
        let head = em.screen().cell(0, 0).unwrap();
        assert_eq!(head.cluster, "世");
        assert_eq!(head.width, Width::Wide);
        let trailer = em.screen().cell(1, 0).unwrap();
        assert_eq!(trailer.width, Width::Trailer);
        assert_eq!(em.screen().cursor().col, 2);
    }

    #[test]
    fn carriage_return_and_line_feed() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"ab\r\nc");
        assert_eq!(cluster(&em, 0, 0), "a");
        assert_eq!(cluster(&em, 0, 1), "c");
        assert_eq!(em.screen().cursor().row, 1);
        assert_eq!(em.screen().cursor().col, 1);
    }

    #[test]
    fn erase_line_clears_row() {
        let mut em = Emulator::new(10, 1);
        em.advance(b"abc\x1b[2K");
        for c in 0..3 {
            assert_eq!(cluster(&em, c, 0), " ");
        }
    }

    #[test]
    fn autowrap_to_next_row() {
        let mut em = Emulator::new(3, 2);
        em.advance(b"abcd");
        assert_eq!(cluster(&em, 0, 0), "a");
        assert_eq!(cluster(&em, 2, 0), "c");
        assert_eq!(cluster(&em, 0, 1), "d");
        assert_eq!(em.screen().cursor().row, 1);
    }

    #[test]
    fn alternate_screen_round_trip() {
        let mut em = Emulator::new(10, 2);
        em.advance(b"main");
        em.advance(b"\x1b[?1049h");
        assert_eq!(em.screen().screen_kind(), ScreenKind::Alternate);
        assert_eq!(cluster(&em, 0, 0), " ");
        em.advance(b"\x1b[?1049l");
        assert_eq!(em.screen().screen_kind(), ScreenKind::Main);
        assert_eq!(cluster(&em, 0, 0), "m");
    }

    #[test]
    fn scroll_on_overflow_keeps_last_line() {
        let mut em = Emulator::new(4, 2);
        em.advance(b"a\r\nb\r\nc");
        // After two line feeds past a 2-row screen, the top scrolls away.
        assert_eq!(cluster(&em, 0, 0), "b");
        assert_eq!(cluster(&em, 0, 1), "c");
    }

    /// `OSC 2` (window title) and `OSC 0` (icon name AND window title) both set the
    /// title; the latest write wins (a shell rewrites it on every prompt).
    #[test]
    fn osc_0_and_2_set_the_window_title() {
        let mut em = Emulator::new(8, 2);
        assert_eq!(em.title(), None, "no title until the child sets one");

        em.advance(b"\x1b]2;vim README\x07");
        assert_eq!(em.title(), Some("vim README"));

        em.advance(b"\x1b]0;coin@host:~\x07");
        assert_eq!(em.title(), Some("coin@host:~"), "latest OSC wins");
    }

    /// `OSC 1` sets only the ICON name — not a window title — so it must NOT be
    /// mistaken for one (the whole point of matching the variants, not the OSC code).
    #[test]
    fn osc_1_icon_name_does_not_set_the_window_title() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]2;real title\x07");
        em.advance(b"\x1b]1;icon-only\x07");
        assert_eq!(
            em.title(),
            Some("real title"),
            "OSC 1 must not overwrite it"
        );
    }

    /// A child-controlled title is CLAMPED to `MAX_TITLE_BYTES` (vtparse bounds the OSC
    /// param count, not the byte length, so a hostile/runaway child could otherwise buffer
    /// an unbounded title). The truncation lands on a UTF-8 char boundary — here a `é`
    /// (2 bytes) straddling the cap must not split into an invalid `String`.
    #[test]
    fn a_hostile_oversized_title_is_clamped_on_a_char_boundary() {
        let mut em = Emulator::new(8, 2);
        // A title of many `é` (2 bytes each), well over the 2048-byte cap.
        let payload = "é".repeat(4000); // 8000 bytes
        em.advance(format!("\x1b]2;{payload}\x07").as_bytes());
        let title = em.title().expect("title set");
        assert!(
            title.len() <= MAX_TITLE_BYTES,
            "clamped to the cap ({} <= {MAX_TITLE_BYTES})",
            title.len(),
        );
        assert!(
            title.chars().all(|c| c == 'é'),
            "truncated on a char boundary — no split/replacement char",
        );
        // A title UNDER the cap is stored verbatim (the common path).
        em.advance(b"\x1b]2;short\x07");
        assert_eq!(em.title(), Some("short"));
    }

    /// A title carries NO cells, so it must not stamp ROW DAMAGE — else every prompt
    /// (which rewrites the title) would force a needless cell re-render. Consumers
    /// still learn of it: the OSC bytes are PTY output, which fires `on_dirty`.
    #[test]
    fn setting_the_title_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b]2;no damage\x07");
        assert_eq!(em.title(), Some("no damage"));
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "a title-only OSC leaves row damage untouched",
        );
    }

    /// `OSC 9` (iTerm2/xterm) raises a body-only notification and bumps the sequence, so a
    /// consumer can tell a new one arrived.
    #[test]
    fn osc_9_raises_a_body_only_notification() {
        let mut em = Emulator::new(8, 2);
        assert_eq!(em.notification(), None, "none until the child raises one");
        assert_eq!(em.notification_seq(), 0);

        em.advance(b"\x1b]9;build finished\x07");
        let n = em.notification().expect("notification set");
        assert_eq!(n.title, None, "OSC 9 carries no title");
        assert_eq!(n.body, "build finished");
        assert_eq!(em.notification_seq(), 1, "the sequence bumped once");

        // A second one latches over the first and bumps the sequence again.
        em.advance(b"\x1b]9;tests passed\x07");
        assert_eq!(
            em.notification().unwrap().body,
            "tests passed",
            "latest wins"
        );
        assert_eq!(em.notification_seq(), 2);
    }

    /// `OSC 777;notify;<title>;<body>` (urxvt) raises a titled notification; a non-`notify`
    /// urxvt extension raises nothing (only the notification sub-command is in the subset).
    #[test]
    fn osc_777_notify_raises_a_titled_notification() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]777;notify;Build;done in 3s\x07");
        let n = em.notification().expect("notification set");
        assert_eq!(n.title.as_deref(), Some("Build"));
        assert_eq!(n.body, "done in 3s");
        assert_eq!(em.notification_seq(), 1);

        // A different urxvt extension (not `notify`) is ignored — no new notification.
        em.advance(b"\x1b]777;something;else\x07");
        assert_eq!(
            em.notification_seq(),
            1,
            "a non-notify OSC 777 raises nothing",
        );
    }

    /// `OSC 99` (kitty): the default single-chunk payload is the TITLE; an explicit
    /// `p=body` payload is the BODY. The advanced tail (base64 `e=1`, non-text `p`) captures
    /// nothing rather than misparsing.
    #[test]
    fn osc_99_kitty_notification_maps_the_payload_by_type() {
        let mut em = Emulator::new(8, 2);
        // No metadata ⇒ kitty's default p=title.
        em.advance(b"\x1b]99;;Attention needed\x07");
        let n = em.notification().expect("title notification");
        assert_eq!(n.title.as_deref(), Some("Attention needed"));
        assert_eq!(n.body, "", "a title-only chunk has no body");
        assert_eq!(em.notification_seq(), 1);

        // Explicit p=body.
        em.advance(b"\x1b]99;p=body;the message\x07");
        let n = em.notification().expect("body notification");
        assert_eq!(n.title, None);
        assert_eq!(n.body, "the message");
        assert_eq!(em.notification_seq(), 2);

        // A base64-encoded payload is NOT decoded here — it must not be shown as gibberish,
        // and it must not bump the sequence (nothing was captured).
        em.advance(b"\x1b]99;e=1;aGk=\x07");
        assert_eq!(
            em.notification_seq(),
            2,
            "an encoded payload is dropped, not misparsed",
        );
        // A non-text payload type (e.g. an icon) captures nothing either.
        em.advance(b"\x1b]99;p=icon;whatever\x07");
        assert_eq!(em.notification_seq(), 2, "a non-text p= is ignored");
    }

    /// A notification carries NO cells, so — like the title — it must not stamp ROW DAMAGE.
    /// It still reaches consumers because the OSC bytes are PTY output (which fires `on_dirty`).
    #[test]
    fn a_notification_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b]9;ping\x07");
        assert_eq!(em.notification().unwrap().body, "ping");
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "a notification OSC leaves row damage untouched",
        );
    }

    /// Both notification fields are child-controlled, so both are CLAMPED like the title —
    /// on a UTF-8 char boundary, so an oversized payload cannot store an invalid `String`.
    #[test]
    fn a_hostile_oversized_notification_is_clamped() {
        let mut em = Emulator::new(8, 2);
        let payload = "é".repeat(4000); // 8000 bytes, over the cap
        em.advance(format!("\x1b]777;notify;{payload};{payload}\x07").as_bytes());
        let n = em.notification().expect("notification set");
        let title = n.title.as_deref().expect("titled");
        assert!(title.len() <= MAX_NOTIFICATION_BYTES && n.body.len() <= MAX_NOTIFICATION_BYTES);
        assert!(
            title.chars().all(|c| c == 'é') && n.body.chars().all(|c| c == 'é'),
            "truncated on a char boundary — no split/replacement char",
        );
    }

    /// ICH (`CSI @`) inserts blanks at the cursor, shifting the rest of the row right; cells
    /// pushed past the right margin fall off.
    #[test]
    fn insert_character_shifts_the_row_right() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"abcd"); // a b c d _ _
        em.advance(b"\x1b[1G"); // cursor to column 1 (CHA, 1-based)
        em.advance(b"\x1b[2@"); // ICH 2
        assert_eq!(cluster(&em, 0, 0), " ");
        assert_eq!(cluster(&em, 1, 0), " ");
        assert_eq!(cluster(&em, 2, 0), "a", "the row shifted right by 2");
        assert_eq!(cluster(&em, 5, 0), "d", "d rode to the right margin");
    }

    /// DCH (`CSI P`) deletes cells at the cursor, shifting the rest of the row left and blanking
    /// the vacated tail.
    #[test]
    fn delete_character_shifts_the_row_left() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"abcdef");
        em.advance(b"\x1b[1G"); // column 1
        em.advance(b"\x1b[2P"); // DCH 2
        assert_eq!(cluster(&em, 0, 0), "c", "the row shifted left by 2");
        assert_eq!(cluster(&em, 3, 0), "f");
        assert_eq!(cluster(&em, 4, 0), " ", "the tail is blanked");
        assert_eq!(cluster(&em, 5, 0), " ");
    }

    /// ECH (`CSI X`) blanks cells at the cursor IN PLACE — unlike DCH, the cells to the right do
    /// not move.
    #[test]
    fn erase_character_blanks_in_place_without_shifting() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"abcdef");
        em.advance(b"\x1b[3G"); // column 3 (0-based col 2)
        em.advance(b"\x1b[2X"); // ECH 2
        assert_eq!(cluster(&em, 1, 0), "b");
        assert_eq!(cluster(&em, 2, 0), " ", "erased in place");
        assert_eq!(cluster(&em, 3, 0), " ");
        assert_eq!(cluster(&em, 4, 0), "e", "cells to the right did NOT shift");
    }

    /// REP (`CSI b`) reprints the last graphic char n times; it is a no-op before any print.
    #[test]
    fn repeat_reprints_the_last_graphic_char() {
        let mut em = Emulator::new(6, 1);
        em.advance(b"x"); // print x (cursor now at column 1)
        em.advance(b"\x1b[3b"); // REP 3
        for c in 0..4 {
            assert_eq!(cluster(&em, c, 0), "x", "x plus 3 repeats");
        }
        assert_eq!(em.screen().cursor().col, 4);

        // REP before any print does nothing (no last graphic char).
        let mut fresh = Emulator::new(4, 1);
        fresh.advance(b"\x1b[3b");
        assert_eq!(cluster(&fresh, 0, 0), " ");
        assert_eq!(fresh.screen().cursor().col, 0);
    }

    /// DECSC / DECRC (`ESC 7` / `ESC 8`) save and restore the cursor POSITION and the SGR PEN, so
    /// a save-draw-elsewhere-restore round trip returns to the exact spot and colour.
    #[test]
    fn decsc_decrc_save_and_restore_the_cursor_and_pen() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"\x1b[31mR"); // a RED 'R' at row0 col0 — capture what red maps to
        let red = em.screen().cell(0, 0).unwrap().fg;
        assert_ne!(red, Color::Default, "red is a non-default pen");

        em.advance(b"\x1b[2;5H"); // move to row 2 col 5 (0-based row1 col4), pen still red
        em.advance(b"\x1b7"); // DECSC — save pos + red pen
        em.advance(b"\x1b[1;1H\x1b[0m"); // home + reset the pen to default
        em.advance(b"\x1b8"); // DECRC — restore pos + pen
        em.advance(b"Z"); // print at the restored spot with the restored pen

        let z = em.screen().cell(4, 1).unwrap();
        assert_eq!(z.cluster, "Z", "restored the saved POSITION (row1 col4)");
        assert_eq!(
            z.fg, red,
            "restored the saved PEN (red), not the reset default"
        );
    }

    /// The `CSI s` / `CSI u` spelling of DECSC / DECRC drives the SAME save/restore.
    #[test]
    fn csi_s_and_u_save_and_restore_the_cursor() {
        let mut em = Emulator::new(10, 3);
        em.advance(b"\x1b[2;5H"); // row1 col4
        em.advance(b"\x1b[s"); // save
        em.advance(b"\x1b[1;1H"); // home
        em.advance(b"\x1b[u"); // restore
        em.advance(b"Q");
        assert_eq!(
            em.screen().cell(4, 1).unwrap().cluster,
            "Q",
            "CSI u restored the position"
        );
    }

    /// DECSCUSR (`CSI SP q`) sets the cursor SHAPE; blink is not modeled, so each shape's steady
    /// and blinking codes map to the same shape, and `0`/`1` are the block default.
    #[test]
    fn decscusr_sets_the_cursor_shape() {
        let mut em = Emulator::new(4, 1);
        assert_eq!(
            em.screen().cursor().shape,
            CursorShape::Block,
            "block by default"
        );
        em.advance(b"\x1b[4 q"); // steady underline
        assert_eq!(em.screen().cursor().shape, CursorShape::Underline);
        em.advance(b"\x1b[5 q"); // blinking bar
        assert_eq!(em.screen().cursor().shape, CursorShape::Bar);
        em.advance(b"\x1b[0 q"); // default -> block
        assert_eq!(em.screen().cursor().shape, CursorShape::Block);
    }

    /// The title survives an alt-screen round trip: it is emulator-level state, not a
    /// property of either screen buffer (a fullscreen app sets a title, then restores).
    #[test]
    fn title_survives_the_alt_screen_round_trip() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]2;editor\x07");
        em.advance(b"\x1b[?1049h");
        assert_eq!(em.title(), Some("editor"));
        em.advance(b"\x1b[?1049l");
        assert_eq!(em.title(), Some("editor"));
    }

    #[test]
    fn damage_generation_advances_on_write() {
        let mut em = Emulator::new(4, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"x");
        assert!(em.screen().row_generation(0).unwrap() > g0);
    }

    #[test]
    fn application_cursor_keys_mode_defaults_off() {
        let em = Emulator::new(4, 2);
        assert!(!em.input_modes().application_cursor_keys);
    }

    #[test]
    fn decckm_set_and_reset_tracked() {
        let mut em = Emulator::new(4, 2);
        // DECSET 1 (ESC [ ? 1 h) enables application cursor keys.
        em.advance(b"\x1b[?1h");
        assert!(em.input_modes().application_cursor_keys);
        // DECRST 1 (ESC [ ? 1 l) restores normal cursor keys.
        em.advance(b"\x1b[?1l");
        assert!(!em.input_modes().application_cursor_keys);
    }

    #[test]
    fn bracketed_paste_mode_defaults_off() {
        let em = Emulator::new(4, 2);
        assert!(!em.input_modes().bracketed_paste);
    }

    #[test]
    fn bracketed_paste_set_and_reset_tracked() {
        let mut em = Emulator::new(4, 2);
        // DECSET 2004 (ESC [ ? 2004 h) asks for pastes to arrive bracketed.
        em.advance(b"\x1b[?2004h");
        assert!(em.input_modes().bracketed_paste);
        // DECRST 2004 (ESC [ ? 2004 l) returns to raw paste.
        em.advance(b"\x1b[?2004l");
        assert!(!em.input_modes().bracketed_paste);
    }

    #[test]
    fn bracketed_paste_mode_does_not_bump_the_damage_generation() {
        // A mode toggle carries no cells; a consumer polling row damage must not see churn.
        let mut em = Emulator::new(4, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b[?2004h");
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "no row damage from a mode set"
        );
    }

    #[test]
    fn mouse_modes_default_off() {
        let em = Emulator::new(4, 2);
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::None);
        assert!(!em.input_modes().mouse_protocol.is_active());
        assert_eq!(em.input_modes().mouse_encoding, MouseEncoding::X10);
    }

    #[test]
    fn decset_1000_sets_click_tracking_and_reset_clears_it() {
        let mut em = Emulator::new(4, 2);
        // DECSET 1000 (ESC [ ? 1000 h) — X11 mouse tracking (press/release).
        em.advance(b"\x1b[?1000h");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::Click);
        // DECRST 1000 hands the mouse back to the terminal.
        em.advance(b"\x1b[?1000l");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::None);
    }

    #[test]
    fn decset_1002_and_1003_set_drag_and_any_motion_levels() {
        let mut em = Emulator::new(4, 2);
        // DECSET 1002 — button-event tracking: press/release + drag (motion with a button held).
        em.advance(b"\x1b[?1002h");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::ButtonEvent);
        assert!(em.input_modes().mouse_protocol.reports_drag());
        assert!(!em.input_modes().mouse_protocol.reports_motion());
        // Upgrade to 1003 — any-event tracking adds bare motion (no button held).
        em.advance(b"\x1b[?1003h");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::AnyEvent);
        assert!(em.input_modes().mouse_protocol.reports_drag());
        assert!(em.input_modes().mouse_protocol.reports_motion());
        // DECRST 1003 hands the mouse back (single-field model: any reset -> None).
        em.advance(b"\x1b[?1003l");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::None);
        // 1002 reset likewise returns to None.
        em.advance(b"\x1b[?1002h\x1b[?1002l");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::None);
    }

    #[test]
    fn mouse_protocol_round_trips_through_its_wire_token() {
        for proto in [
            MouseProtocol::None,
            MouseProtocol::Click,
            MouseProtocol::ButtonEvent,
            MouseProtocol::AnyEvent,
        ] {
            assert_eq!(MouseProtocol::from_wire_str(proto.wire_str()), proto);
        }
        assert_eq!(
            MouseProtocol::from_wire_str(Some("bogus")),
            MouseProtocol::None,
            "an unknown token is not tracking"
        );
    }

    #[test]
    fn decset_1006_selects_sgr_encoding_orthogonally_to_tracking() {
        let mut em = Emulator::new(4, 2);
        // 1006 is an ENCODING; it does not enable reporting on its own.
        em.advance(b"\x1b[?1006h");
        assert_eq!(em.input_modes().mouse_encoding, MouseEncoding::Sgr);
        assert_eq!(
            em.input_modes().mouse_protocol,
            MouseProtocol::None,
            "1006 alone reports nothing"
        );
        // A tracking mode composes with the encoding.
        em.advance(b"\x1b[?1000h");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::Click);
        assert_eq!(em.input_modes().mouse_encoding, MouseEncoding::Sgr);
        // DECRST 1006 returns to X10 without touching the tracking mode.
        em.advance(b"\x1b[?1006l");
        assert_eq!(em.input_modes().mouse_encoding, MouseEncoding::X10);
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::Click);
    }

    #[test]
    fn decset_1004_toggles_focus_tracking() {
        let mut em = Emulator::new(4, 2);
        assert!(!em.input_modes().focus_tracking, "off by default");
        em.advance(b"\x1b[?1004h");
        assert!(em.input_modes().focus_tracking, "DECSET 1004 enables it");
        em.advance(b"\x1b[?1004l");
        assert!(!em.input_modes().focus_tracking, "DECRST 1004 disables it");
        // Orthogonal to mouse tracking: a child may set both.
        em.advance(b"\x1b[?1000h\x1b[?1004h");
        assert_eq!(em.input_modes().mouse_protocol, MouseProtocol::Click);
        assert!(em.input_modes().focus_tracking);
    }

    #[test]
    fn mouse_mode_set_does_not_bump_the_damage_generation() {
        // Like every input-mode toggle, enabling mouse tracking carries no cells.
        let mut em = Emulator::new(4, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(em.screen().row_generation(0).unwrap(), g0, "no row damage");
    }

    // ----- B1: soft-wrap continuation metadata (`Screen::wrapped`) -----

    #[test]
    fn autowrap_marks_the_row_wrapped() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcdef"); // 6 chars in 4 cols -> row0 "abcd" wraps to row1 "ef"
        assert!(em.screen().wrapped(0), "row 0 soft-wrapped onto row 1");
        assert!(!em.screen().wrapped(1), "row 1 did not wrap");
    }

    #[test]
    fn hard_linefeed_clears_the_wrapped_flag() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcde"); // wraps -> wrapped[0] = true
        assert!(em.screen().wrapped(0));
        em.advance(b"\x1b[H\n"); // home to row 0, then a hard line feed
        assert!(
            !em.screen().wrapped(0),
            "a hard line feed ends the logical line"
        );
    }

    #[test]
    fn erase_line_clears_the_wrapped_flag() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcde"); // wraps -> wrapped[0] = true
        assert!(em.screen().wrapped(0));
        em.advance(b"\x1b[H\x1b[2K"); // home, then erase the whole line
        assert!(
            !em.screen().wrapped(0),
            "erasing the line drops the soft wrap"
        );
    }

    // ----- resize-redraw reinterpretation (`in_resize_redraw`) -----

    #[test]
    fn resize_redraw_crlf_is_a_soft_wrap() {
        // After a resize, the line editor's redraw uses an explicit CR LF to
        // continue a wrapped line at an exact-fill width; treat it as a soft wrap
        // so the redraw stays one logical line (collapses on a later widen).
        let mut em = Emulator::new(10, 4);
        em.advance(b"x");
        em.resize(10, 4); // arms the redraw window
        em.advance(b"\rAAAA\r\nBBBB"); // CR, content, CR LF (the wrap idiom), content
        assert!(
            em.screen().wrapped(0),
            "a CR LF inside the resize redraw is a soft wrap"
        );
    }

    #[test]
    fn normal_crlf_outside_a_redraw_is_a_hard_break() {
        // Without a preceding resize, the same CR LF ends the logical line — so
        // ordinary command output keeps its real line breaks.
        let mut em = Emulator::new(10, 4);
        em.advance(b"AAAA\r\nBBBB");
        assert!(
            !em.screen().wrapped(0),
            "a CR LF in normal output is a hard line break"
        );
    }

    #[test]
    fn redraw_window_ends_after_the_first_batch_without_shell_integration() {
        // NON-INTEGRATED shell (no OSC 133, so the prompt state is Unknown): the soft-wrap
        // reinterpretation falls back to the single batch — a CR LF in a later batch is hard
        // again, because without shell integration a prompt redraw cannot be told apart from a
        // foreground command's output, so the window must not persist.
        let mut em = Emulator::new(10, 4);
        em.resize(10, 4);
        em.advance(b"\rAAAA"); // first batch (the redraw) — window closes after it
        em.advance(b"BBBB\r\nCCCC"); // a later batch
        assert!(
            !em.screen().wrapped(0),
            "with no shell integration the redraw window closes after one batch"
        );
    }

    #[test]
    fn redraw_window_persists_across_batches_while_at_a_prompt() {
        // With shell integration reporting AT A PROMPT, a redraw that overflows one PTY read (a
        // long line reflowed narrow) arrives as several batches; the CR LF soft-wrap must persist
        // across all of them, not close after the first (the multi-batch redraw fix).
        let mut em = Emulator::new(10, 4);
        em.advance(b"\x1b]133;A\x1b\\"); // OSC 133 A: the shell is at a prompt
        em.resize(10, 4); // arms the redraw window
        em.advance(b"\rAAAA"); // batch 1 of the redraw
        em.advance(b"\r\nBBBB"); // batch 2 — its CR LF is STILL a soft wrap
        assert!(
            em.screen().wrapped(0),
            "the redraw window persisted past the first batch while at a prompt"
        );
    }

    #[test]
    fn note_input_ends_the_resize_redraw_epoch() {
        // The user acting (a keystroke / a submit) ends the redraw epoch: the CR LF that submits a
        // command, and the command output after it, are hard breaks again — not joined onto the
        // prompt line. Without this, a persisted at-a-prompt window would soft-wrap the submit.
        let mut em = Emulator::new(10, 4);
        em.advance(b"\x1b]133;A\x1b\\"); // at a prompt (the window would otherwise persist)
        em.resize(10, 4);
        em.advance(b"\rAAAA"); // redraw batch (window persists at a prompt)
        VtPort::note_input(&mut em); // the user typed / submitted
        em.advance(b"\r\nBBBB"); // output after the submit — a HARD break now
        assert!(
            !em.screen().wrapped(0),
            "note_input closed the epoch, so this CR LF is a hard break"
        );
    }

    #[test]
    fn a_running_command_does_not_persist_the_redraw_window() {
        // Resizing while a command is RUNNING (OSC 133 C) must not soft-wrap its output across
        // batches — only a PROMPT redraw persists. The window falls back to the single batch, so a
        // later CR LF in the command's output stays a hard break (no foreground-output regression).
        let mut em = Emulator::new(10, 4);
        em.advance(b"\x1b]133;C\x1b\\"); // the shell is running a command
        em.resize(10, 4);
        em.advance(b"\rAAAA"); // batch 1 — window closes at its end (not at a prompt)
        em.advance(b"\r\nBBBB"); // batch 2 — hard break
        assert!(
            !em.screen().wrapped(0),
            "a running command's output does not keep the redraw window open"
        );
    }

    #[test]
    fn resize_redraw_erase_clears_the_wrapped_continuation() {
        // The redraw's leading erase-in-line clears the whole wrapped active line,
        // not just the cursor's row, so the stale tail of the prior width is gone.
        let mut em = Emulator::new(4, 4);
        em.advance(b"abcdefgh"); // row0 "abcd" (wrapped) -> row1 "efgh"
        assert_eq!(em.screen().row_text(1), "efgh");
        em.resize(4, 4); // arms the window; cursor anchored to the line top (row 0)
        em.advance(b"\r\x1b[K"); // CR + erase-to-end-of-line at the line top
        assert_eq!(
            em.screen().row_text(1),
            "",
            "the wrapped continuation row was cleared too"
        );
    }

    // ----- B2: reflow on resize -----

    fn row(em: &Emulator, r: u16) -> String {
        em.screen().row_text(r)
    }

    #[test]
    fn reflow_rejoins_a_wrapped_line_when_widened() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcdef"); // wraps: row0 "abcd" -> row1 "ef"
        assert!(em.screen().wrapped(0));
        em.resize(8, 3);
        // The logical line now fits in one row, cleanly rejoined.
        assert_eq!(row(&em, 0), "abcdef");
        assert!(
            !em.screen().wrapped(0),
            "no longer wrapped at the wider width"
        );
        assert_eq!(row(&em, 1), "", "the continuation row is gone");
        // Cursor preserved by logical position: after 'f'.
        assert_eq!((em.screen().cursor().col, em.screen().cursor().row), (6, 0));
    }

    #[test]
    fn reflow_rebreaks_a_line_when_narrowed() {
        let mut em = Emulator::new(8, 3);
        em.advance(b"abcdef"); // fits in one row at width 8
        assert!(!em.screen().wrapped(0));
        em.resize(4, 3);
        // The logical line re-breaks at the new margin.
        assert_eq!(row(&em, 0), "abcd");
        assert!(
            em.screen().wrapped(0),
            "row 0 soft-wraps at the narrow width"
        );
        assert_eq!(row(&em, 1), "ef");
        // The cursor anchors to the FIRST physical row of its logical line (row 0),
        // not the continuation row it wrapped onto — keeping a line editor's resize
        // redraw (CR + erase + reprint, which assumes the cursor is at the line's
        // top) overwriting in place instead of stacking. Pulled up from a lower row,
        // its column pins to 0 (the line start) rather than the natural `offset %
        // width`, which would slide as the line re-breaks at different widths; see
        // `Screen::reflowed`'s cursor-anchor note.
        assert_eq!((em.screen().cursor().col, em.screen().cursor().row), (0, 0));
    }

    #[test]
    fn reflow_keeps_natural_column_on_a_single_row_line() {
        // When the cursor's logical line still fits on ONE physical row after the
        // reflow, the anchor row IS the cursor's own row, so its natural column is
        // preserved (no pin-to-0) — the caret stays after the text, not at the start.
        let mut em = Emulator::new(4, 3);
        em.advance(b"abcdef"); // wraps at width 4: row0 "abcd" -> row1 "ef", cursor after 'f'
        em.resize(8, 3); // widen: the line rejoins onto a single row
        assert_eq!(
            (em.screen().cursor().col, em.screen().cursor().row),
            (6, 0),
            "single-row line keeps the cursor after the text, column intact"
        );
    }

    #[test]
    fn reflow_anchors_cursor_to_logical_line_top() {
        // A line that wraps to several physical rows: after a reflow the cursor must
        // sit on the line's FIRST physical row so a live shell's `SIGWINCH` redraw
        // (CR + erase-in-line + reprint, no cursor-up) overwrites the old prompt
        // rather than stacking a fresh copy below it (the resize-stale bug).
        let mut em = Emulator::new(12, 4);
        em.advance(b"abcdefghijkl"); // exactly fills row 0 at width 12
        em.advance(b"mnop"); // wraps onto row 1; cursor after 'p'
        assert!(em.screen().wrapped(0), "the logical line spans rows 0..1");
        em.resize(4, 6); // re-break the 16-glyph line to width 4 -> 4 physical rows
        assert_eq!(
            em.screen().cursor().row,
            0,
            "cursor anchors to the logical line's top row, not its wrapped bottom"
        );
    }

    #[test]
    fn reflow_round_trips_stably() {
        let mut em = Emulator::new(8, 3);
        em.advance(b"abcdef");
        let text = em.screen().full_text();
        em.resize(4, 4); // narrow (rewraps)
        em.resize(8, 3); // back to the original width
        assert_eq!(
            em.screen().full_text(),
            text,
            "widen∘narrow restores the text"
        );
    }

    #[test]
    fn reflow_skips_the_alternate_screen() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[?1049h"); // enter the alternate screen
        em.advance(b"abcdef"); // fits at width 8 on the alt screen
        em.resize(4, 2);
        // The alt screen is NOT reflowed (verbatim) — a fullscreen app owns its
        // layout. The verbatim copy truncates to the new width, no rejoin.
        assert_eq!(row(&em, 0), "abcd", "alt screen truncated, not rewrapped");
    }

    // ----- scroll-region slice (DECSTBM + IL/DL/SU/SD + RI/IND) -----

    /// DECSTBM (`CSI Pt;Pb r`) confines a line feed's scroll to the margins: a line feed at
    /// the bottom margin scrolls only the region, leaving the rows above and below fixed.
    /// It also homes the cursor when the region is set.
    #[test]
    fn scroll_region_confines_the_line_feed_to_the_margins() {
        let mut em = Emulator::new(4, 6);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4\r\n5"); // rows 0..5 labelled
        em.advance(b"\x1b[2;4r"); // region 1-based [2,4] = 0-based [1,3]
        assert_eq!(
            (em.screen().cursor().col, em.screen().cursor().row),
            (0, 0),
            "DECSTBM homes the cursor",
        );
        em.advance(b"\x1b[4;1H\n"); // to the bottom margin (row 3), then a line feed
        assert_eq!(
            em.screen().row_text(0),
            "0",
            "the row above the region is fixed"
        );
        assert_eq!(em.screen().row_text(1), "2", "the region scrolled up");
        assert_eq!(em.screen().row_text(2), "3");
        assert_eq!(
            em.screen().row_text(3),
            "",
            "the bottom margin row is blank"
        );
        assert_eq!(
            em.screen().row_text(4),
            "4",
            "the rows below the region are fixed"
        );
        assert_eq!(em.screen().row_text(5), "5");
    }

    /// RI (`ESC M`) at the top margin scrolls the region DOWN (a blank opens at the top);
    /// IND (`ESC D`) at the bottom margin scrolls it UP. Both keep the cursor at the margin.
    #[test]
    fn reverse_index_and_index_scroll_the_region_at_its_margins() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;4r"); // region 0-based [1,3]
        em.advance(b"\x1b[2;1H\x1bM"); // to the top margin (row 1), then RI
        assert_eq!(
            em.screen().row_text(1),
            "",
            "RI opened a blank at the top margin"
        );
        assert_eq!(em.screen().row_text(2), "1");
        assert_eq!(em.screen().row_text(3), "2");
        assert_eq!(em.screen().row_text(0), "0", "outside the region is fixed");
        assert_eq!(em.screen().row_text(4), "4");
        assert_eq!(em.screen().cursor().row, 1, "RI stays at the top margin");
        em.advance(b"\x1b[4;1H\x1bD"); // to the bottom margin (row 3), then IND
        assert_eq!(
            em.screen().row_text(1),
            "1",
            "IND scrolled the region back up"
        );
        assert_eq!(em.screen().row_text(2), "2");
        assert_eq!(em.screen().row_text(3), "");
        assert_eq!(
            em.screen().cursor().row,
            3,
            "IND stays at the bottom margin"
        );
    }

    /// Away from a margin, IND / RI just move the cursor — they do NOT scroll. (Revert
    /// proof for the margin condition: without it, these would scroll and mangle contents.)
    #[test]
    fn index_and_reverse_index_move_without_scrolling_off_the_margins() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;4r"); // region [1,3]
        em.advance(b"\x1b[2;1H\x1bD"); // top margin -> IND -> down one, no scroll
        assert_eq!(em.screen().cursor().row, 2);
        assert_eq!(
            em.screen().row_text(1),
            "1",
            "no scroll: the region is unchanged"
        );
        assert_eq!(em.screen().row_text(2), "2");
        em.advance(b"\x1b[4;1H\x1bM"); // bottom margin -> RI -> up one, no scroll
        assert_eq!(em.screen().cursor().row, 2);
        assert_eq!(
            em.screen().row_text(3),
            "3",
            "no scroll: the region is unchanged"
        );
    }

    /// IL (`CSI L`) opens blank lines and DL (`CSI M`) removes them, both bounded by the
    /// scroll region and homing the cursor column (ECMA-48). Rows outside the region (a
    /// fixed footer here) never move.
    #[test]
    fn insert_and_delete_line_are_region_bounded_and_home_the_column() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4"); // rows 0..4
        em.advance(b"\x1b[1;4r"); // region 0-based [0,3]; row 4 is the fixed footer
        em.advance(b"\x1b[2;3H\x1b[L"); // to row 1 col 2, then IL 1
        assert_eq!(em.screen().cursor().col, 0, "IL homes the column");
        assert_eq!(em.screen().row_text(0), "0", "above the insert is fixed");
        assert_eq!(
            em.screen().row_text(1),
            "",
            "a blank line opened at the cursor"
        );
        assert_eq!(em.screen().row_text(2), "1");
        assert_eq!(
            em.screen().row_text(3),
            "2",
            "the line at the bottom margin fell off"
        );
        assert_eq!(
            em.screen().row_text(4),
            "4",
            "the footer below the region is fixed"
        );
        em.advance(b"\x1b[2;3H\x1b[M"); // to row 1 col 2, then DL 1
        assert_eq!(em.screen().cursor().col, 0, "DL homes the column");
        assert_eq!(
            em.screen().row_text(1),
            "1",
            "the blank was removed, rows moved up"
        );
        assert_eq!(em.screen().row_text(2), "2");
        assert_eq!(
            em.screen().row_text(3),
            "",
            "a blank opened at the bottom margin"
        );
        assert_eq!(em.screen().row_text(4), "4", "the footer stays fixed");
    }

    /// SU (`CSI S`) and SD (`CSI T`) scroll the region by n without moving the cursor.
    #[test]
    fn scroll_up_and_scroll_down_move_the_region_by_n() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;4r"); // region [1,3]
        em.advance(b"\x1b[2S"); // SU 2
        assert_eq!(
            em.screen().row_text(1),
            "3",
            "the region scrolled up by two"
        );
        assert_eq!(em.screen().row_text(2), "");
        assert_eq!(em.screen().row_text(3), "");
        assert_eq!(em.screen().row_text(0), "0", "outside the region is fixed");
        assert_eq!(em.screen().row_text(4), "4");
        em.advance(b"\x1b[T"); // SD 1
        assert_eq!(
            em.screen().row_text(1),
            "",
            "SD opened a blank at the top margin"
        );
        assert_eq!(em.screen().row_text(2), "3");
    }

    /// The scrollback rule: only an output-flow scroll of a TOP-ANCHORED region reaches
    /// the scrollback. A mid-screen region does not (interior lines), and DL never does
    /// (an edit, not a scroll) — even at row 0 of a top-anchored region.
    #[test]
    fn only_a_top_anchored_output_scroll_feeds_the_scrollback() {
        let mut mid = Emulator::new(4, 5);
        mid.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        mid.advance(b"\x1b[2;4r\x1b[S"); // mid-screen region [1,3], SU 1
        assert_eq!(
            mid.screen().scrollback_len(),
            0,
            "a mid-screen region scroll is not history",
        );

        let mut top = Emulator::new(4, 5);
        top.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        top.advance(b"\x1b[1;4r\x1b[S"); // top-anchored region [0,3], SU 1
        assert_eq!(
            top.screen().scrollback_rows().collect::<Vec<_>>(),
            ["0"],
            "a top-anchored output scroll is history",
        );

        let mut del = Emulator::new(4, 5);
        del.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        del.advance(b"\x1b[1;4r\x1b[1;1H\x1b[M"); // top-anchored region, home, DL 1
        assert_eq!(
            del.screen().scrollback_len(),
            0,
            "DL removes a line; it is not scrolled into history",
        );
    }

    /// An invalid DECSTBM (`top >= bottom`) is ignored — the margins and the cursor are
    /// left untouched (the region stays full-screen).
    #[test]
    fn an_invalid_scroll_region_is_ignored() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[3;3H"); // move the cursor off home (row 2 col 2)
        em.advance(b"\x1b[4;2r"); // top 4 >= bottom 2: invalid
        assert_eq!(
            (em.screen().cursor().col, em.screen().cursor().row),
            (2, 2),
            "an invalid DECSTBM does not home the cursor",
        );
        em.advance(b"\x1b[5;1H\n"); // last row, line feed
        assert_eq!(
            em.screen().scrollback_rows().collect::<Vec<_>>(),
            ["0"],
            "the region is still full-screen, so the whole screen scrolled",
        );
    }

    /// A resize returns the scroll region to the full screen: a line feed at the LAST row
    /// then scrolls the whole screen (a stale sub-region would not scroll from there).
    #[test]
    fn a_resize_resets_the_scroll_region_to_full_screen() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"\x1b[2;3r"); // set a sub-region [1,2]
        em.resize(4, 5); // resets the region
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4"); // fills rows 0..4
        em.advance(b"\n"); // LF at the last row -> whole-screen scroll iff region is full
        assert_eq!(
            em.screen().row_text(0),
            "1",
            "the screen scrolled as a whole"
        );
        assert_eq!(em.screen().scrollback_rows().next().as_deref(), Some("0"));
    }

    /// IL / DL are no-ops when the cursor is outside the scroll region (the VT100 rule).
    #[test]
    fn insert_and_delete_line_outside_the_region_do_nothing() {
        let mut em = Emulator::new(4, 5);
        em.advance(b"0\r\n1\r\n2\r\n3\r\n4");
        em.advance(b"\x1b[2;3r"); // region [1,2]; rows 0, 3, 4 are outside
        em.advance(b"\x1b[5;1H\x1b[L\x1b[M"); // cursor to row 4 (outside), IL then DL
        for (r, want) in [(0u16, "0"), (1, "1"), (2, "2"), (3, "3"), (4, "4")] {
            assert_eq!(
                em.screen().row_text(r),
                want,
                "IL/DL outside the region changed nothing",
            );
        }
    }

    /// The real split-region idiom: a scrolling area above a fixed footer. Output flows and
    /// scrolls within the region; the footer never moves; the line that leaves a
    /// top-anchored region becomes scrollback.
    #[test]
    fn output_flows_within_the_region_leaving_a_footer_fixed() {
        let mut em = Emulator::new(6, 4);
        em.advance(b"\x1b[4;1Hstatus"); // paint a footer on the last row (row 3)
        em.advance(b"\x1b[1;3r"); // rows 0..2 scroll; row 3 is fixed
        em.advance(b"\x1b[1;1Haaa\r\nbbb\r\nccc\r\nddd"); // 4 lines into a 3-row region
        assert_eq!(em.screen().row_text(0), "bbb", "the region scrolled");
        assert_eq!(em.screen().row_text(1), "ccc");
        assert_eq!(em.screen().row_text(2), "ddd");
        assert_eq!(em.screen().row_text(3), "status", "the footer never moved");
        assert_eq!(
            em.screen().scrollback_rows().next().as_deref(),
            Some("aaa"),
            "the line that scrolled off the top-anchored region is history",
        );
    }

    // ----- BEL -> attention (tmux monitor-bell) -----

    /// A BARE bell (`\a`) bumps `bell_seq` (the tmux monitor-bell ping); it is kept apart from
    /// the notification and stamps no row damage. The `\a` that TERMINATES an OSC is that OSC's
    /// string terminator, consumed by the parser — so it must NOT count as a bell.
    #[test]
    fn a_bare_bell_bumps_bell_seq_apart_from_the_notification() {
        let mut em = Emulator::new(8, 2);
        assert_eq!(em.bell_seq(), 0, "no bell until one rings");

        em.advance(b"\x07"); // a bare BEL
        assert_eq!(em.bell_seq(), 1);
        em.advance(b"a\x07b\x07"); // two more bares, interleaved with prints
        assert_eq!(em.bell_seq(), 3, "each bare bell counts");
        assert_eq!(em.notification(), None, "a bell is not a notification");

        // The `\a` terminating an OSC (OSC 2 window title, ST = BEL) is not a bell.
        em.advance(b"\x1b]2;t\x07");
        assert_eq!(em.bell_seq(), 3, "an OSC-terminating BEL does not count");
        assert_eq!(em.title(), Some("t"), "the OSC still applied");

        // A bell carries no cells: a bell-only batch leaves row damage untouched.
        let g = em.screen().row_generation(0).unwrap();
        em.advance(b"\x07");
        assert_eq!(em.bell_seq(), 4);
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g,
            "a bell stamps no row damage",
        );
    }

    // ----- OSC 133 (FinalTerm) shell-integration marks -----

    /// A full prompt cycle marks the prompt (A) row, the output (C) row, and the command-end (D)
    /// row, and the derived [`ShellState`] + last exit status track it. B (input start) sets no
    /// row mark.
    #[test]
    fn osc_133_marks_prompt_output_and_command_end() {
        use crate::port::ShellState;
        let mut em = Emulator::new(20, 6);
        assert_eq!(
            em.screen().shell_state(),
            ShellState::Unknown,
            "no marks yet"
        );
        assert_eq!(em.screen().last_exit_status(), None);

        em.advance(b"\x1b]133;A\x07"); // A: prompt start on row 0
        em.advance(b"$ \x1b]133;B\x07"); // draw the prompt, B: input start (no mark)
        em.advance(b"ls\r\n"); // command echoed + Enter -> row 1
        assert_eq!(
            em.screen().mark(0),
            Some(PromptMark::Prompt),
            "row 0 = prompt"
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::AtPrompt,
            "idle at the prompt / awaiting input",
        );

        em.advance(b"\x1b]133;C\x07"); // C: output starts on row 1
        assert_eq!(
            em.screen().mark(1),
            Some(PromptMark::Output),
            "row 1 = output"
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::Running,
            "a command is running",
        );

        em.advance(b"hello\r\n"); // output -> row 2
        em.advance(b"\x1b]133;D;0\x07"); // D: finished, exit 0, row 2
        assert_eq!(
            em.screen().mark(2),
            Some(PromptMark::CommandEnd(Some(0))),
            "row 2 = command end, exit 0",
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::AtPrompt,
            "finished -> idle again",
        );
        assert_eq!(
            em.screen().last_exit_status(),
            Some(0),
            "the reported exit status"
        );

        // A second command that FAILS (exit 1) — the newer status wins.
        em.advance(b"\x1b]133;A\x07"); // next prompt (row 3)
        em.advance(b"bad\r\n\x1b]133;C\x07\r\n\x1b]133;D;1\x07");
        assert_eq!(
            em.screen().last_exit_status(),
            Some(1),
            "the most recent command's exit wins",
        );
    }

    /// A finished command slices into its line, its output, and its reported exit.
    #[test]
    fn last_command_slices_line_output_and_exit() {
        let mut em = Emulator::new(20, 6);
        em.advance(b"\x1b]133;A\x07$ ls\x1b]133;B\x07"); // prompt + typed command, row 0
        em.advance(b"\r\n\x1b]133;C\x07"); // Enter -> row 1, output starts here
        em.advance(b"a.txt\r\nb.txt\r\n"); // output rows 1, 2; cursor -> row 3
        em.advance(b"\x1b]133;D;0\x07"); // finished, exit 0, row 3
        let cmd = em.screen().last_command().expect("a command ran");
        assert_eq!(cmd.command, "$ ls", "the prompt row up to output start");
        assert_eq!(cmd.output, "a.txt\nb.txt", "the rows between C and D");
        assert_eq!(cmd.exit_status, Some(0));
        assert!(!cmd.running);
    }

    /// A command still running (no `D` yet) reports its output-so-far, no exit.
    #[test]
    fn last_command_reports_a_running_command_without_exit() {
        let mut em = Emulator::new(20, 6);
        em.advance(b"\x1b]133;A\x07$ sleep 9\x1b]133;B\x07\r\n");
        em.advance(b"\x1b]133;C\x07working...\r\n"); // output starts row 1, still running
        let cmd = em.screen().last_command().expect("a command is running");
        assert_eq!(cmd.command, "$ sleep 9");
        assert_eq!(
            cmd.output, "working...",
            "output to the bottom, blanks trimmed"
        );
        assert_eq!(cmd.exit_status, None);
        assert!(cmd.running);
    }

    /// No output mark (`C`) yet — at a bare prompt, nothing has run — is `None`.
    #[test]
    fn last_command_is_none_without_an_output_mark() {
        let mut em = Emulator::new(20, 3);
        em.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07"); // a prompt, nothing executed
        assert!(em.screen().last_command().is_none());
    }

    /// `prompt_positions` lists the logical row index of every prompt-start mark, oldest
    /// first — the jump-to-prompt targets.
    #[test]
    fn prompt_positions_lists_the_prompt_rows() {
        let mut em = Emulator::new(20, 8);
        em.advance(
            b"\x1b]133;A\x07$ echo a\x1b]133;B\x07\r\n\x1b]133;C\x07a\r\n\x1b]133;D;0\x07\r\n",
        );
        em.advance(b"\x1b]133;A\x07$ echo b\x1b]133;B\x07\r\n\x1b]133;C\x07b\r\n\x1b]133;D;0\x07");
        assert_eq!(
            em.screen().prompt_positions(),
            vec![0, 3],
            "prompts on rows 0 and 3"
        );
    }

    /// A prompt that scrolled off the top is still listed — at its scrollback index (0 =
    /// oldest), so jump-to-prompt spans history.
    #[test]
    fn prompt_positions_span_scrollback() {
        let mut em = Emulator::new(20, 3);
        em.advance(b"\x1b]133;A\x07$ x\x1b]133;B\x07\r\n\x1b]133;C\x07");
        em.advance(b"1\r\n2\r\n3\r\n4\r\n"); // scroll the A-marked row 0 into scrollback
        assert_eq!(
            em.screen().prompt_positions(),
            vec![0],
            "the one prompt, now the oldest scrollback line"
        );
    }

    /// The anchor is the last OUTPUT start, not the last mark: at a fresh prompt after a
    /// command finished, `last_command` still returns that finished command.
    #[test]
    fn last_command_is_the_finished_command_at_a_fresh_prompt() {
        let mut em = Emulator::new(20, 8);
        em.advance(b"\x1b]133;A\x07$ echo hi\x1b]133;B\x07\r\n\x1b]133;C\x07");
        em.advance(b"hi\r\n\x1b]133;D;0\x07\r\n"); // output row 1, D row 2, then row 3
        em.advance(b"\x1b]133;A\x07$ \x1b]133;B\x07"); // a NEW prompt on row 3, nothing typed
        let cmd = em.screen().last_command().expect("the finished command");
        assert_eq!(cmd.command, "$ echo hi");
        assert_eq!(cmd.output, "hi");
        assert_eq!(cmd.exit_status, Some(0));
        assert!(!cmd.running, "the new prompt does not make it look running");
    }

    /// A prompt mark travels WITH its row when that row scrolls off the top into the scrollback —
    /// so a prompt in history stays a jump target and still feeds the derived state. REVERT-PROOF:
    /// if the scroll dropped the mark, `scrollback_mark(0)` would be `None`.
    #[test]
    fn osc_133_mark_scrolls_into_the_scrollback_with_its_row() {
        use crate::port::ShellState;
        let mut em = Emulator::new(8, 2); // 2 visible rows -> a quick scroll
        em.advance(b"\x1b]133;A\x07a"); // row 0: prompt mark + an 'a'
        assert_eq!(em.screen().mark(0), Some(PromptMark::Prompt));
        // Two line feeds push row 0 off the top into the scrollback.
        em.advance(b"\r\nb\r\nc");
        assert_eq!(
            em.screen().scrollback_mark(0),
            Some(PromptMark::Prompt),
            "the prompt row's mark scrolled into history with it",
        );
        assert_eq!(
            em.screen().mark(0),
            None,
            "the new visible row 0 is unmarked"
        );
        assert_eq!(
            em.screen().shell_state(),
            ShellState::AtPrompt,
            "the state still derives from the scrollback mark",
        );
    }

    /// A mark follows its LOGICAL line through a reflow: it re-attaches to the re-broken line's
    /// FIRST physical row, whether the line rejoins (widen) or re-wraps (narrow). REVERT-PROOF: if
    /// reflow dropped marks, `mark(0)` after a resize would be `None`.
    #[test]
    fn osc_133_prompt_mark_follows_its_line_through_reflow() {
        let mut em = Emulator::new(8, 4);
        em.advance(b"\x1b]133;A\x07"); // row 0 prompt
        em.advance(b"0123456789"); // wraps at width 8: row0 "01234567" (mark), row1 "89"
        assert_eq!(em.screen().mark(0), Some(PromptMark::Prompt));
        assert!(em.screen().wrapped(0));

        em.resize(16, 4); // widen: the logical line rejoins onto one row
        assert_eq!(em.screen().row_text(0), "0123456789");
        assert_eq!(
            em.screen().mark(0),
            Some(PromptMark::Prompt),
            "mark stays on the rejoined line's head",
        );

        em.resize(4, 6); // narrow: re-break to three rows
        assert_eq!(em.screen().row_text(0), "0123");
        assert_eq!(
            em.screen().mark(0),
            Some(PromptMark::Prompt),
            "mark on the re-broken line's head",
        );
        assert_eq!(em.screen().mark(1), None, "not on a continuation row");
        assert_eq!(em.screen().mark(2), None);
    }

    // ----- OSC 52 (clipboard) -----

    /// A WRITE (`OSC 52 ; c ; <base64>`) captures the base64-DECODED text against the requested
    /// selection and bumps the write sequence; a later write to a different selection supersedes.
    /// (`aGk=` is base64("hi").)
    #[test]
    fn osc_52_write_captures_the_decoded_text_and_targets() {
        let mut em = Emulator::new(8, 2);
        assert!(
            em.clipboard_write().is_none(),
            "none until the child writes"
        );
        assert_eq!(em.clipboard_write_seq(), 0);

        em.advance(b"\x1b]52;c;aGk=\x07");
        let w = em.clipboard_write().expect("write latched");
        assert_eq!(w.text, "hi", "base64 decoded to the plain text");
        assert_eq!(
            w.targets,
            ClipboardTargets {
                clipboard: true,
                primary: false
            }
        );
        assert_eq!(em.clipboard_write_seq(), 1);

        em.advance(b"\x1b]52;p;aGk=\x07");
        let w = em.clipboard_write().expect("write latched");
        assert_eq!(
            w.targets,
            ClipboardTargets {
                clipboard: false,
                primary: true
            }
        );
        assert_eq!(em.clipboard_write_seq(), 2, "each write bumps the seq");
    }

    /// A write naming BOTH selections (`OSC 52 ; cp ; …`) sets both — a write is not reducible to
    /// one target.
    #[test]
    fn osc_52_write_to_both_selections() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]52;cp;aGk=\x07");
        assert_eq!(
            em.clipboard_write().unwrap().targets,
            ClipboardTargets {
                clipboard: true,
                primary: true
            },
        );
    }

    /// A CLEAR (`OSC 52 ; c` with no data field) is captured as a write of the empty string — the
    /// consumer clears that selection.
    #[test]
    fn osc_52_clear_is_an_empty_write() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]52;c;aGk=\x07");
        em.advance(b"\x1b]52;c\x07"); // clear
        let w = em.clipboard_write().expect("clear latched as a write");
        assert_eq!(w.text, "", "a clear is an empty write");
        assert!(w.targets.clipboard);
        assert_eq!(em.clipboard_write_seq(), 2, "the clear bumped the seq");
    }

    /// A READ (`OSC 52 ; c ; ?`) captures a query for that selection and bumps the query
    /// sequence, distinct from the write sequence.
    #[test]
    fn osc_52_query_captures_the_read_request() {
        let mut em = Emulator::new(8, 2);
        assert!(em.clipboard_query().is_none());
        assert_eq!(em.clipboard_query_seq(), 0);

        em.advance(b"\x1b]52;c;?\x07");
        assert_eq!(
            em.clipboard_query().unwrap().target,
            ClipboardTarget::Clipboard
        );
        assert_eq!(em.clipboard_query_seq(), 1);
        assert_eq!(em.clipboard_write_seq(), 0, "a read is not a write");

        em.advance(b"\x1b]52;p;?\x07");
        assert_eq!(
            em.clipboard_query().unwrap().target,
            ClipboardTarget::Primary
        );
        assert_eq!(em.clipboard_query_seq(), 2);
    }

    /// An OSC 52 naming ONLY an X cut buffer (`0`-`9`) has no clipboard/primary analog in sprag's
    /// model, so it is a no-op: nothing latched, the seq untouched — it cannot supersede a real
    /// pending write.
    #[test]
    fn osc_52_cut_buffer_only_write_is_ignored() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]52;0;aGk=\x07");
        assert!(
            em.clipboard_write().is_none(),
            "cut-buffer-only maps to no selection"
        );
        assert_eq!(em.clipboard_write_seq(), 0);
    }

    /// A clipboard write carries no cells, so — like the title / notification — it must not stamp
    /// ROW DAMAGE. Consumers still learn of it: the OSC bytes are PTY output, which fires
    /// `on_dirty`.
    #[test]
    fn osc_52_write_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b]52;c;aGk=\x07");
        assert!(em.clipboard_write().is_some());
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "no row damage from OSC 52"
        );
    }

    /// A hostile oversized write is CLAMPED to `MAX_CLIPBOARD_BYTES` on a char boundary — a
    /// child-controlled buffer must be bounded. (Feeds the decoded text via [`osc52_reply`], the
    /// same encoder a reply uses, so the base64 is correct without hand-encoding.)
    #[test]
    fn osc_52_write_over_cap_is_clamped() {
        let mut em = Emulator::new(8, 2);
        let payload = "é".repeat(MAX_CLIPBOARD_BYTES); // 2 bytes each, well over the cap
        let bytes = osc52_reply(ClipboardTarget::Clipboard, &payload); // ESC ] 52;c;<base64> ST
        em.advance(&bytes);
        let w = em.clipboard_write().expect("write latched");
        assert!(w.text.len() <= MAX_CLIPBOARD_BYTES, "clamped to the cap");
        assert!(
            w.text.chars().all(|c| c == 'é'),
            "truncated on a char boundary"
        );
    }

    /// [`osc52_reply`] frames the answer as `ESC ] 52 ; <sel> ; <base64> ST`, and the bytes
    /// ROUND-TRIP: parsing them back yields the original text against the reply's selection.
    #[test]
    fn osc_52_reply_frames_and_round_trips() {
        // Exact wire form (aGk= is base64("hi")).
        assert_eq!(
            osc52_reply(ClipboardTarget::Clipboard, "hi"),
            b"\x1b]52;c;aGk=\x1b\\"
        );
        assert_eq!(
            osc52_reply(ClipboardTarget::Primary, "hi"),
            b"\x1b]52;p;aGk=\x1b\\"
        );

        // Round-trip a non-trivial payload through the parser: reply bytes -> a WRITE of the
        // same text on the same selection.
        let mut em = Emulator::new(8, 2);
        em.advance(&osc52_reply(ClipboardTarget::Primary, "clip board!"));
        let w = em.clipboard_write().expect("reply parsed as a write");
        assert_eq!(w.text, "clip board!");
        assert_eq!(
            w.targets,
            ClipboardTargets {
                clipboard: false,
                primary: true
            }
        );
    }

    // ----- Kitty keyboard protocol (negotiation) -----

    /// `CSI > 1 u` pushes the disambiguate flag; the current flags reach the key encoder via
    /// `input_modes`. `CSI < u` pops back to the legacy (empty) state.
    #[test]
    fn kitty_keyboard_push_and_pop_the_flag_stack() {
        let mut em = Emulator::new(8, 2);
        assert!(
            em.input_modes().kitty_keyboard.is_empty(),
            "legacy until pushed"
        );

        em.advance(b"\x1b[>1u"); // push disambiguate
        assert!(em.input_modes().kitty_keyboard.disambiguate());

        em.advance(b"\x1b[<u"); // pop (default 1)
        assert!(
            em.input_modes().kitty_keyboard.is_empty(),
            "popped back to legacy"
        );
    }

    /// A child requesting flags sprag does NOT honor (here the full 0b11111) has the unsupported
    /// bits DROPPED at push time — so the terminal never advertises a capability its encoder lacks.
    #[test]
    fn kitty_keyboard_masks_off_unsupported_flags() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[>31u"); // all five flags requested
        let flags = em.input_modes().kitty_keyboard;
        assert!(flags.disambiguate());
        assert_eq!(
            flags.bits(),
            KittyKeyboardFlags::DISAMBIGUATE,
            "only the supported bit survives"
        );
    }

    /// `CSI = flags ; mode u` modifies the CURRENT level: mode 1 assigns, 2 sets bits, 3 clears.
    #[test]
    fn kitty_keyboard_set_modes_modify_the_current_level() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[=1;1u"); // assign-all disambiguate (creates a base level)
        assert!(em.input_modes().kitty_keyboard.disambiguate());
        em.advance(b"\x1b[=1;3u"); // clear-specified disambiguate
        assert!(em.input_modes().kitty_keyboard.is_empty(), "cleared");
        em.advance(b"\x1b[=1;2u"); // set-specified disambiguate
        assert!(
            em.input_modes().kitty_keyboard.disambiguate(),
            "set back on"
        );
    }

    /// `CSI ? u` makes the terminal REPLY `CSI ? flags u` with the CURRENT honored flags — the
    /// device-response the reader writes back to the child. The reply reports the SUPPORTED subset,
    /// never the raw request, so it cannot claim an unimplemented capability.
    #[test]
    fn kitty_keyboard_query_replies_with_the_supported_flags() {
        let mut em = Emulator::new(8, 2);
        // No level pushed → reports 0.
        em.advance(b"\x1b[?u");
        assert_eq!(em.take_responses(), b"\x1b[?0u", "empty stack reports 0");

        // A request for all flags is masked to the honored disambiguate bit, and the reply agrees.
        em.advance(b"\x1b[>31u");
        em.advance(b"\x1b[?u");
        assert_eq!(
            em.take_responses(),
            b"\x1b[?1u",
            "reply reports only the honored subset"
        );
        assert!(
            em.take_responses().is_empty(),
            "take drains the response buffer"
        );
    }

    /// Primary DA (`CSI c`, and the `CSI 0 c` spelling) replies with sprag's honored attributes —
    /// VT220 class + Sixel (4) + ANSI colour (22) — on the device-response channel.
    #[test]
    fn primary_device_attributes_reports_the_honored_capabilities() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[c");
        assert_eq!(em.take_responses(), b"\x1b[?62;4;22c");
        // The explicit-zero spelling parses the same.
        em.advance(b"\x1b[0c");
        assert_eq!(em.take_responses(), b"\x1b[?62;4;22c");
    }

    /// Secondary DA (`CSI > c`) replies with a generic VT100-family identity (type 0, version 0),
    /// deliberately not impersonating a version-gated xterm build.
    #[test]
    fn secondary_device_attributes_reports_a_generic_identity() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[>c");
        assert_eq!(em.take_responses(), b"\x1b[>0;0;0c");
    }

    /// DSR status (`CSI 5 n`) reports "ready, no malfunction" (`CSI 0 n`).
    #[test]
    fn device_status_report_replies_ready() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b[5n");
        assert_eq!(em.take_responses(), b"\x1b[0n");
    }

    /// CPR (`CSI 6 n`) reports the 1-based cursor position after the cursor has moved — so a
    /// hard-coded `1;1` would fail. Revert-proof: the position is read from the live cursor.
    #[test]
    fn cursor_position_report_answers_the_live_1_based_cursor() {
        let mut em = Emulator::new(8, 3);
        // Move to line 2, col 3 (1-based) = row 1, col 2 (0-based), then query.
        em.advance(b"\x1b[2;3H\x1b[6n");
        assert_eq!(em.take_responses(), b"\x1b[2;3R");
        // At the home position the report is 1;1.
        em.advance(b"\x1b[H\x1b[6n");
        assert_eq!(em.take_responses(), b"\x1b[1;1R");
    }

    /// A DA / DSR query carries no cells, so — like the keyboard negotiation — it must not stamp ROW
    /// DAMAGE (the generation counter stays put).
    #[test]
    fn a_device_query_stamps_no_row_damage() {
        let mut em = Emulator::new(8, 2);
        let before = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b[c\x1b[5n\x1b[6n");
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            before,
            "a query is not a paint"
        );
        assert!(!em.take_responses().is_empty(), "but it did answer");
    }

    /// A keyboard negotiation carries no cells, so it must not stamp ROW DAMAGE.
    #[test]
    fn kitty_keyboard_negotiation_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b[>1u\x1b[?u");
        assert!(em.input_modes().kitty_keyboard.disambiguate());
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "no row damage from negotiation"
        );
    }

    // ---- OSC 8 hyperlinks ----

    /// The OSC-8 hyperlink `Arc` a cell carries, or `None` for an unlinked cell.
    fn link_at(em: &Emulator, col: u16, row: u16) -> Option<Arc<Hyperlink>> {
        em.screen().cell(col, row).unwrap().hyperlink.clone()
    }

    /// The pen opened by `\e]8;;<uri>` stamps every cell printed until `\e]8;;`
    /// closes it, and a contiguous run shares ONE interned `Arc`.
    #[test]
    fn osc8_pen_stamps_cells_between_open_and_close() {
        let mut em = Emulator::new(20, 2);
        em.advance(b"a\x1b]8;;https://example.com\x1b\\LK\x1b]8;;\x1b\\b");
        assert!(link_at(&em, 0, 0).is_none(), "before the link: unlinked");
        let l = link_at(&em, 1, 0).expect("L is linked");
        let k = link_at(&em, 2, 0).expect("K is linked");
        assert_eq!(l.uri, "https://example.com");
        assert_eq!(l.id, None);
        assert!(Arc::ptr_eq(&l, &k), "contiguous link cells share one Arc");
        assert!(link_at(&em, 3, 0).is_none(), "after `8;;`: unlinked");
    }

    /// The `id=` grouping key ties NON-ADJACENT runs (separated by plain text)
    /// into one logical link — they share the interned `Arc` (R-69.3.b).
    #[test]
    fn osc8_id_groups_non_adjacent_runs() {
        let mut em = Emulator::new(40, 2);
        em.advance(
            b"\x1b]8;id=r1;http://x\x1b\\AA\x1b]8;;\x1b\\ZZ\x1b]8;id=r1;http://x\x1b\\BB\x1b]8;;\x1b\\",
        );
        let a = link_at(&em, 0, 0).expect("A linked");
        let b = link_at(&em, 4, 0).expect("B linked");
        assert!(
            link_at(&em, 2, 0).is_none(),
            "the ZZ between runs is unlinked"
        );
        assert_eq!(a.id.as_deref(), Some("r1"));
        assert!(
            Arc::ptr_eq(&a, &b),
            "same id groups non-adjacent runs into one link"
        );
    }

    /// Two ANONYMOUS links (no `id`) to the same URI are DISTINCT runs — each
    /// opens a fresh `Arc`, so pointer identity keeps them apart even though
    /// their URIs are equal. (Grouping anonymous links by URI would be wrong.)
    #[test]
    fn osc8_anonymous_links_to_same_uri_are_distinct_runs() {
        let mut em = Emulator::new(40, 2);
        em.advance(b"\x1b]8;;http://x\x1b\\A\x1b]8;;\x1b\\ \x1b]8;;http://x\x1b\\B\x1b]8;;\x1b\\");
        let a = link_at(&em, 0, 0).expect("A linked");
        let b = link_at(&em, 2, 0).expect("B linked");
        assert_eq!(a.uri, b.uri, "same URI");
        assert!(
            !Arc::ptr_eq(&a, &b),
            "anonymous links are distinct runs even with an equal URI"
        );
    }

    /// A link that WRAPS onto the next row is one logical link — the head cell
    /// and the wrapped continuation share the pen's `Arc` (the marquee OSC-8
    /// "link split across a wrap" case, grouped without any position math).
    #[test]
    fn osc8_link_wrapping_to_next_row_shares_one_arc() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"\x1b]8;;http://w\x1b\\ABCDEF\x1b]8;;\x1b\\");
        let head = link_at(&em, 0, 0).expect("row0 linked");
        let wrapped = link_at(&em, 1, 1).expect("wrapped onto row1, still linked");
        assert!(
            Arc::ptr_eq(&head, &wrapped),
            "a wrapped link is one Arc across the wrap"
        );
    }

    /// The link rides PHYSICALLY with the cell: a linked row scrolled off the
    /// top keeps its target in scrollback (no separate interning table to sync
    /// — the win over an OSC-133-style parallel-array approach).
    #[test]
    fn osc8_link_rides_the_cell_into_scrollback() {
        let mut em = Emulator::new(8, 2);
        em.advance(b"\x1b]8;;http://s\x1b\\HI\x1b]8;;\x1b\\");
        em.advance(b"\r\n\r\n"); // push row 0 ("HI") off the top into scrollback
        let scrollback: Vec<&[Cell]> = em.screen().scrollback_cells().collect();
        let first = scrollback.first().expect("a row scrolled off");
        let link = first[0]
            .hyperlink
            .as_ref()
            .expect("the scrolled-off cell keeps its link");
        assert_eq!(link.uri, "http://s");
    }

    /// An OSC-8 control carries no cells, so — like the title / notification —
    /// it must not stamp ROW DAMAGE.
    #[test]
    fn osc8_control_does_not_bump_the_damage_generation() {
        let mut em = Emulator::new(8, 2);
        let g0 = em.screen().row_generation(0).unwrap();
        em.advance(b"\x1b]8;;http://x\x1b\\");
        assert_eq!(
            em.screen().row_generation(0).unwrap(),
            g0,
            "OSC 8 carries no cells"
        );
    }

    /// `hyperlink_runs` reports the visible links as data — the covered text, the
    /// URI, and the `id` — so an agent reads a link's destination without OCR
    /// (the tmux-superior surface).
    #[test]
    fn osc8_hyperlink_runs_report_visible_links_as_data() {
        let mut em = Emulator::new(20, 2);
        em.advance(b"go \x1b]8;id=k;https://ok\x1b\\here\x1b]8;;\x1b\\ end");
        let runs = em.screen().hyperlink_runs();
        assert_eq!(runs.len(), 1, "exactly one linked run");
        assert_eq!(runs[0].text, "here");
        assert_eq!(runs[0].uri, "https://ok");
        assert_eq!(runs[0].id.as_deref(), Some("k"));
    }

    /// A link that wraps onto the next row folds into ONE run with continuous
    /// text — the run tracks the link handle, not row boundaries.
    #[test]
    fn osc8_hyperlink_runs_fold_a_wrapped_link_into_one_run() {
        let mut em = Emulator::new(4, 3);
        em.advance(b"\x1b]8;;http://w\x1b\\ABCDEF\x1b]8;;\x1b\\");
        let runs = em.screen().hyperlink_runs();
        assert_eq!(runs.len(), 1, "a wrapped link is one run");
        assert_eq!(runs[0].text, "ABCDEF", "text continues across the wrap");
    }

    // ---- Kitty graphics images (pinion R1404, Stage 1) ----

    /// A Kitty `a=T` RGBA transmit stores a decoded image anchored at the cursor,
    /// keyed by its `i=` id, with the exact pixels.
    #[test]
    fn kitty_rgba_transmit_and_display_stores_an_image_at_the_cursor() {
        let mut em = Emulator::new(10, 4);
        em.advance(b"xy"); // cursor now at col 2
        let px: Vec<u8> = (1..=16).collect(); // 2x2 RGBA
        let b64 = BASE64.encode(&px);
        em.advance(format!("\x1b_Ga=T,f=32,s=2,v=2,i=7;{b64}\x1b\\").as_bytes());
        let imgs = em.screen().images();
        assert_eq!(imgs.len(), 1, "one image captured");
        assert_eq!(imgs[0].id, 7);
        assert_eq!((imgs[0].width, imgs[0].height), (2, 2));
        assert_eq!(imgs[0].rgba, px, "exact pixels");
        assert_eq!(imgs[0].anchor, (2, 0), "anchored at the cursor");
    }

    /// An RGB (`f=24`) transmit expands to RGBA with an opaque alpha.
    #[test]
    fn kitty_rgb_expands_to_opaque_rgba() {
        let mut em = Emulator::new(6, 2);
        let b64 = BASE64.encode([10u8, 20, 30]); // 1x1 RGB
        em.advance(format!("\x1b_Ga=T,f=24,s=1,v=1;{b64}\x1b\\").as_bytes());
        assert_eq!(em.screen().images()[0].rgba, vec![10, 20, 30, 255]);
    }

    /// A re-transmit under the same id REPLACES the image (Kitty update), not append.
    #[test]
    fn kitty_same_id_retransmit_replaces() {
        let mut em = Emulator::new(6, 2);
        let a = BASE64.encode([1u8, 1, 1, 1]);
        let b = BASE64.encode([9u8, 9, 9, 9]);
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=3;{a}\x1b\\").as_bytes());
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=3;{b}\x1b\\").as_bytes());
        let imgs = em.screen().images();
        assert_eq!(imgs.len(), 1, "same id replaced, not appended");
        assert_eq!(imgs[0].rgba, vec![9, 9, 9, 9]);
    }

    /// Every insert stamps a fresh monotonic `seq` — a re-transmit that REPLACES the same id gets a
    /// NEW seq, so an on-demand client (R1404 Stage 5) can tell a content change from a re-poll and
    /// re-fetch the RGBA exactly once.
    #[test]
    fn kitty_retransmit_bumps_the_content_seq() {
        let mut em = Emulator::new(6, 2);
        let a = BASE64.encode([1u8, 1, 1, 1]);
        let b = BASE64.encode([9u8, 9, 9, 9]);
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=3;{a}\x1b\\").as_bytes());
        let seq0 = em.screen().images()[0].seq;
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=3;{b}\x1b\\").as_bytes());
        let seq1 = em.screen().images()[0].seq;
        assert!(seq1 > seq0, "a re-transmit bumps seq ({seq1} > {seq0})");
    }

    /// A whole-screen clear (ED 2) drops the inline images (Stage-1 lifecycle).
    #[test]
    fn screen_clear_drops_the_inline_images() {
        let mut em = Emulator::new(6, 3);
        let b64 = BASE64.encode([5u8, 5, 5, 5]);
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1;{b64}\x1b\\").as_bytes());
        assert_eq!(em.screen().images().len(), 1);
        em.advance(b"\x1b[2J"); // ED 2
        assert!(em.screen().images().is_empty(), "screen-clear drops images");
    }

    /// A byte count that does not match `w*h*bpp` is DROPPED, never misrendered.
    #[test]
    fn kitty_malformed_byte_count_is_dropped() {
        let mut em = Emulator::new(6, 2);
        let b64 = BASE64.encode([1u8, 2, 3, 4]); // 4 bytes, but claims 2x2 RGBA (16)
        em.advance(format!("\x1b_Ga=T,f=32,s=2,v=2;{b64}\x1b\\").as_bytes());
        assert!(em.screen().images().is_empty(), "malformed image dropped");
    }

    /// A child-controlled raster past [`MAX_IMAGE_BYTES`] (or one that overflows) is refused BEFORE
    /// it is decoded / stored / wired — the same bound as the clipboard / title caps, and (Stage 1)
    /// the guard against flooding the inline panes-slot wire.
    #[test]
    fn image_cap_rejects_oversized_and_overflowing_rasters() {
        // At the cap: 2048x2048x4 = 16 MiB exactly is allowed.
        assert!(image_within_cap(2048, 2048));
        // One row over the cap is refused.
        assert!(!image_within_cap(2048, 2049), "past 16 MiB is refused");
        // A raster whose byte count overflows usize is refused, not wrapped.
        assert!(!image_within_cap(u32::MAX, u32::MAX), "overflow is refused");
    }

    /// The sixel rasterizer's correctness core: the low six bits of a data byte are six VERTICAL
    /// pixels (bit 0 = topmost), `Repeat` runs one byte across columns, `NewLine` advances a 6-row
    /// band, `#n;2;…` defines AND selects a colour, and unpainted pixels stay transparent.
    #[test]
    fn sixel_rasters_the_six_bit_columns_and_colors() {
        use termwiz::color::RgbColor;
        use termwiz::escape::{Sixel, SixelData};
        let sixel = Sixel {
            pan: 1,
            pad: 1,
            pixel_width: Some(4),
            pixel_height: Some(12),
            background_is_transparent: true,
            horizontal_grid_size: None,
            data: vec![
                SixelData::DefineColorMapRGB {
                    color_number: 0,
                    rgb: RgbColor::new_8bpc(255, 0, 0),
                },
                SixelData::Data(0b000001), // col 0: bit 0 -> row 0 red
                SixelData::Data(0b100000), // col 1: bit 5 -> row 5 red
                SixelData::Repeat {
                    repeat_count: 2,
                    data: 0b000001,
                }, // cols 2,3: row 0 red
                SixelData::NewLine,        // -> band_top 6, x 0
                SixelData::DefineColorMapRGB {
                    color_number: 1,
                    rgb: RgbColor::new_8bpc(0, 255, 0),
                },
                SixelData::Data(0b000001), // col 0, band 1: row 6 green
            ],
        };
        let (w, h, rgba) = raster_sixel(&sixel).expect("rasters");
        assert_eq!((w, h), (4, 12));
        let wu = w as usize;
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * wu + x) * 4;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        assert_eq!(px(0, 0), [255, 0, 0, 255], "col0 bit0 -> row0 red");
        assert_eq!(
            px(1, 5),
            [255, 0, 0, 255],
            "col1 bit5 -> row5 (6-bit unpack)"
        );
        assert_eq!(px(2, 0), [255, 0, 0, 255], "repeat col2 row0");
        assert_eq!(px(3, 0), [255, 0, 0, 255], "repeat col3 row0");
        assert_eq!(px(0, 6), [0, 255, 0, 255], "newline advanced 6 rows; green");
        assert_eq!(px(0, 1), [0, 0, 0, 0], "unpainted -> transparent");
        assert_eq!(px(1, 0), [0, 0, 0, 0], "col1 row0 unpainted");
    }

    /// A DCS sixel through the REAL termwiz parser stores an image at the cursor cell, painted
    /// opaque (the whole vertical column the `~` filled).
    #[test]
    fn sixel_dcs_stores_an_image_at_the_cursor() {
        let mut em = Emulator::new(10, 5);
        em.advance(b"\x1b[2;3H"); // cursor -> row 1, col 2 (0-based)
        em.advance(b"\x1bPq#0;2;100;0;0~\x1b\\"); // define+select red, one all-6 column
        let imgs = em.screen().images();
        assert_eq!(imgs.len(), 1, "one sixel image stored");
        assert_eq!(imgs[0].anchor, (2, 1), "anchored at the cursor cell");
        // The `~` (all six bits) painted the whole column opaque, red-dominant.
        assert_eq!(imgs[0].rgba[3], 255, "top-left pixel is opaque (painted)");
        assert!(
            imgs[0].rgba[0] > imgs[0].rgba[1] && imgs[0].rgba[0] > imgs[0].rgba[2],
            "painted red"
        );
    }

    #[test]
    fn sixel_image_is_dropped_on_screen_clear() {
        let mut em = Emulator::new(10, 5);
        em.advance(b"\x1bPq#0;2;100;0;0~\x1b\\");
        assert_eq!(em.screen().images().len(), 1);
        em.advance(b"\x1b[2J"); // ED2 whole-screen clear
        assert!(
            em.screen().images().is_empty(),
            "screen clear drops the sixel"
        );
    }

    #[test]
    fn sixel_over_the_cap_is_dropped() {
        use termwiz::escape::{Sixel, SixelData};
        let sixel = Sixel {
            pan: 1,
            pad: 1,
            pixel_width: Some(3000), // 3000*3000*4 = ~34 MiB > 16 MiB cap
            pixel_height: Some(3000),
            background_is_transparent: true,
            horizontal_grid_size: None,
            data: vec![SixelData::Data(1)],
        };
        assert!(
            raster_sixel(&sixel).is_none(),
            "an over-cap sixel is refused"
        );
    }

    /// An inline image survives a resize / reflow verbatim — a plain resize must NOT drop it
    /// (position-eviction under scroll / reflow is a later stage). Regression guard: a GUI's boot
    /// settling resize was silently dropping every image.
    #[test]
    fn an_image_survives_a_resize() {
        let mut em = Emulator::new(10, 5);
        em.advance(b"\x1bPq#0;2;100;0;0~\x1b\\"); // a sixel image at the cursor
        assert_eq!(em.screen().images().len(), 1);
        VtPort::resize(&mut em, 20, 8); // reflow to a new size
        assert_eq!(
            em.screen().images().len(),
            1,
            "the image survives the resize/reflow"
        );
    }

    /// A 1x1 RGBA Kitty image APC anchored at the current cursor (Stage-3 lifecycle tests).
    fn tiny_image_apc() -> String {
        format!(
            "\x1b_Ga=T,f=32,s=1,v=1;{}\x1b\\",
            BASE64.encode([9u8, 9, 9, 255])
        )
    }

    /// R1404 Stage 3: an image tracks its text — a line feed that scrolls the screen scrolls the
    /// image's anchor up with it.
    #[test]
    fn image_anchor_scrolls_up_with_a_line_feed() {
        let mut em = Emulator::new(8, 4);
        // Anchor on the bottom row (row 3), then a line feed at the bottom scrolls the region up 1.
        em.advance(format!("\x1b[4;1H{}", tiny_image_apc()).as_bytes());
        assert_eq!(
            em.screen().images()[0].anchor,
            (0, 3),
            "anchored at the bottom"
        );
        em.advance(b"\n");
        assert_eq!(
            em.screen().images()[0].anchor,
            (0, 2),
            "the image scrolled up one row with the text"
        );
    }

    /// R1404 Stage 3: an image scrolled off the TOP of the screen is evicted, not pinned.
    #[test]
    fn image_scrolled_off_the_top_is_evicted() {
        let mut em = Emulator::new(8, 4);
        // Anchor on the top row (row 0), park the cursor at the bottom, then one line feed scrolls
        // row 0 off the top of the region.
        em.advance(format!("{}\x1b[4;1H", tiny_image_apc()).as_bytes());
        assert_eq!(em.screen().images()[0].anchor, (0, 0));
        em.advance(b"\n");
        assert!(
            em.screen().images().is_empty(),
            "the image scrolled off the top is gone"
        );
    }

    /// R1404 Stage 3: an image is dropped when erase-in-display clears its anchor row (no ghost).
    #[test]
    fn image_dropped_when_erase_display_clears_its_row() {
        let mut em = Emulator::new(8, 4);
        // Anchor on row 2, then erase-to-end-of-display from row 1 clears rows 1..=3, dropping it.
        em.advance(format!("\x1b[3;1H{}", tiny_image_apc()).as_bytes());
        assert_eq!(em.screen().images()[0].anchor, (0, 2));
        em.advance(b"\x1b[2;1H\x1b[0J"); // cursor row 1, ED0 (to end of display)
        assert!(
            em.screen().images().is_empty(),
            "erase-in-display over the image's row dropped it"
        );
    }

    /// R1404 Stage 3: an image OUTSIDE a scroll region is not moved when the region scrolls —
    /// the region check is load-bearing (a naive whole-screen shift would wrongly move it).
    #[test]
    fn image_outside_the_scroll_region_is_unmoved() {
        let mut em = Emulator::new(8, 4);
        // Image on the bottom row (row 3), then a scroll region of the top two rows [0,1]; scrolling
        // that region must leave the row-3 image where it is.
        em.advance(format!("\x1b[4;1H{}", tiny_image_apc()).as_bytes());
        em.advance(b"\x1b[1;2r"); // DECSTBM rows 1..=2 (1-indexed) = [0,1]; homes the cursor
        em.advance(b"\x1b[2;1H\n"); // cursor to the region bottom (row 1), line feed scrolls [0,1]
        assert_eq!(
            em.screen().images()[0].anchor,
            (0, 3),
            "an image below the scroll region is unmoved"
        );
    }

    /// #8 reflow: NARROWING that re-breaks a logical line follows the anchor onto the cell's new
    /// (col, row). Revert-proof — a verbatim carry would leave the anchor at (4, 0).
    #[test]
    fn image_anchor_follows_a_cell_onto_a_narrowed_wrap_row() {
        let mut em = Emulator::new(8, 4);
        em.advance(b"abcdefg"); // one logical line on row 0 (fits width 8)
        // Anchor the image on 'e' (row 0, col 4).
        em.advance(format!("\x1b[1;5H{}", tiny_image_apc()).as_bytes());
        assert_eq!(em.screen().images()[0].anchor, (4, 0));
        VtPort::resize(&mut em, 4, 4); // "abcdefg" -> "abcd" (row 0, wrapped) + "efg" (row 1)
        assert_eq!(
            em.screen().images()[0].anchor,
            (0, 1),
            "the anchor followed 'e' onto the wrapped continuation row"
        );
    }

    /// #8 reflow: WIDENING that rejoins a soft-wrapped line follows the anchor back onto the merged
    /// row. Revert-proof — a verbatim carry would leave the anchor at (0, 1).
    #[test]
    fn image_anchor_follows_a_cell_onto_a_widened_rejoin_row() {
        let mut em = Emulator::new(6, 3);
        em.advance(b"abcdefgh"); // width 6 autowraps: "abcdef" (row 0, wrapped) + "gh" (row 1)
        // Anchor on 'g' (row 1, col 0).
        em.advance(format!("\x1b[2;1H{}", tiny_image_apc()).as_bytes());
        assert_eq!(em.screen().images()[0].anchor, (0, 1));
        VtPort::resize(&mut em, 8, 3); // rejoin: "abcdefgh" on row 0, 'g' at col 6
        assert_eq!(
            em.screen().images()[0].anchor,
            (6, 0),
            "the anchor followed 'g' back onto the rejoined row"
        );
    }

    /// #8 reflow: an image whose anchor cell reflows ABOVE the visible window is evicted — the same
    /// no-scrollback-image bound as a scrolled-off image, now reached via a rewrap instead of a
    /// scroll.
    #[test]
    fn image_reflowed_above_the_window_is_evicted() {
        let mut em = Emulator::new(8, 3);
        em.advance(b"\x1b[1;1H"); // cursor at row 0
        em.advance(tiny_image_apc().as_bytes()); // anchor (0, 0), on the top row
        em.advance(b"XY\r\naaaaaaaa\r\nbbbbbbbb"); // rows fit in 3 at width 8
        assert_eq!(em.screen().images().len(), 1);
        // Narrow to 4: "aaaaaaaa"/"bbbbbbbb" each double to 2 rows, so "XY" (with the anchor) is
        // pushed above the 3-row window into the overflow that becomes scrollback.
        VtPort::resize(&mut em, 4, 3);
        assert!(
            em.screen().images().is_empty(),
            "the image whose anchor reflowed above the window was evicted"
        );
    }

    /// #8 reflow: a narrow-then-widen round trip restores the anchor cell (the exact-cell tracking
    /// is reversible, not lossy).
    #[test]
    fn image_anchor_round_trips_across_narrow_then_widen() {
        let mut em = Emulator::new(8, 4);
        em.advance(b"abcdefg");
        em.advance(format!("\x1b[1;5H{}", tiny_image_apc()).as_bytes());
        let original = em.screen().images()[0].anchor; // (4, 0)
        VtPort::resize(&mut em, 4, 4);
        VtPort::resize(&mut em, 8, 4);
        assert_eq!(
            em.screen().images()[0].anchor,
            original,
            "narrow then widen restores the anchor cell"
        );
    }

    /// #8 reflow: the reflow carries `next_image_seq`, so a re-transmit AFTER a resize keeps a
    /// content seq above every surviving image's. Revert-proof — a reset to 0 would make the
    /// re-transmit's seq read as older than the pre-resize image's.
    #[test]
    fn a_retransmit_after_a_reflow_keeps_a_monotonic_seq() {
        let mut em = Emulator::new(8, 4);
        em.advance(tiny_image_apc().as_bytes());
        assert_eq!(em.screen().images()[0].seq, 0);
        VtPort::resize(&mut em, 10, 4); // reflow must carry next_image_seq (not reset it to 0)
        em.advance(tiny_image_apc().as_bytes()); // re-transmit under the same id
        assert_eq!(
            em.screen().images()[0].seq,
            1,
            "the re-transmit's content seq stayed monotonic across the reflow"
        );
    }

    // ---- Kitty graphics Stage 4: chunking / delete / query / PNG ----

    /// Encode `rgba` (`width x height`, 8-bit RGBA) to PNG bytes for a decode round-trip test.
    fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut out, width, height);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header().unwrap().write_image_data(rgba).unwrap();
        }
        out
    }

    /// A chunked transmit (`m=1` then `m=0`) reassembles its split base64 into one image at the
    /// closing chunk — and nothing appears until it closes.
    #[test]
    fn kitty_chunked_transmit_reassembles() {
        let mut em = Emulator::new(6, 2);
        let raw = [10u8, 20, 30, 40, 50, 60, 70, 80]; // 2x1 RGBA
        let b64 = BASE64.encode(raw);
        let (h1, h2) = b64.split_at(b64.len() / 2);
        em.advance(format!("\x1b_Ga=T,f=32,s=2,v=1,i=5,m=1;{h1}\x1b\\").as_bytes());
        assert!(em.screen().images().is_empty(), "nothing yet — chunk open");
        em.advance(format!("\x1b_Gm=0;{h2}\x1b\\").as_bytes());
        let imgs = em.screen().images();
        assert_eq!(imgs.len(), 1, "the closing chunk assembled one image");
        assert_eq!(imgs[0].id, 5);
        assert_eq!(imgs[0].rgba, raw.to_vec(), "reassembled payload matches");
    }

    /// A chunk opened with `m=1` that never sends its `m=0` closer stores no image (and does not
    /// leak — the accumulator is bounded).
    #[test]
    fn kitty_never_closed_chunk_leaves_no_image() {
        let mut em = Emulator::new(6, 2);
        let b64 = BASE64.encode([10u8, 20, 30, 40]);
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=5,m=1;{b64}\x1b\\").as_bytes());
        assert!(
            em.screen().images().is_empty(),
            "an unclosed chunk shows nothing"
        );
    }

    /// `a=d,d=i,i=N` deletes the image with id N; `a=d,d=a` clears all.
    #[test]
    fn kitty_delete_removes_by_id_then_all() {
        let mut em = Emulator::new(6, 2);
        let b64 = BASE64.encode([1u8, 2, 3, 4]); // 1x1 RGBA
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=7;{b64}\x1b\\").as_bytes());
        em.advance(format!("\x1b_Ga=T,f=32,s=1,v=1,i=8;{b64}\x1b\\").as_bytes());
        assert_eq!(em.screen().images().len(), 2);
        em.advance(b"\x1b_Ga=d,d=i,i=7\x1b\\");
        assert_eq!(em.screen().images().len(), 1, "delete-by-id dropped id 7");
        assert_eq!(em.screen().images()[0].id, 8, "id 8 remains");
        em.advance(b"\x1b_Ga=d,d=a\x1b\\");
        assert!(
            em.screen().images().is_empty(),
            "delete-all cleared the store"
        );
    }

    /// A PNG (`f=100`) payload decodes to the exact RGBA it encoded (dimensions from the PNG).
    #[test]
    fn kitty_png_decodes_to_exact_rgba() {
        let mut em = Emulator::new(6, 2);
        let rgba = [255u8, 0, 0, 255, 0, 255, 0, 128]; // 2x1: opaque red, semi-transparent green
        let png = encode_png(2, 1, &rgba);
        let b64 = BASE64.encode(&png);
        em.advance(format!("\x1b_Ga=T,f=100,i=9;{b64}\x1b\\").as_bytes());
        let imgs = em.screen().images();
        assert_eq!(imgs.len(), 1, "PNG decoded to an image");
        assert_eq!((imgs[0].width, imgs[0].height), (2, 1));
        assert_eq!(imgs[0].rgba, rgba.to_vec(), "exact RGBA round-trip");
    }

    /// A malformed PNG payload is dropped, never misrendered.
    #[test]
    fn kitty_malformed_png_is_dropped() {
        let mut em = Emulator::new(6, 2);
        let b64 = BASE64.encode(b"this is not a PNG");
        em.advance(format!("\x1b_Ga=T,f=100,i=9;{b64}\x1b\\").as_bytes());
        assert!(em.screen().images().is_empty(), "a bad PNG stores nothing");
    }

    /// A graphics support probe (`a=q`) is acked over the device-response channel with the image id
    /// — how an app learns sprag renders images (tmux cannot).
    #[test]
    fn kitty_query_acks_graphics_support() {
        let mut em = Emulator::new(6, 2);
        let b64 = BASE64.encode([0u8, 0, 0, 0]);
        em.advance(format!("\x1b_Ga=q,f=32,s=1,v=1,i=3;{b64}\x1b\\").as_bytes());
        let resp = String::from_utf8(VtPort::take_responses(&mut em)).unwrap();
        assert_eq!(
            resp, "\x1b_Gi=3;OK\x1b\\",
            "acks support with the queried image id"
        );
    }
}
