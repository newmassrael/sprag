//! The projection's WIRE FORM — how a projected `GridBuffer` crosses the socket.
//!
//! ## The measurement this exists to answer
//!
//! R221 priced a display client's steady-state read — one pane's `cells.0` fetch — at about 4 ms,
//! a quarter of a 60 Hz frame, and found the projection it is named for to be 0.5% of that. The
//! cost was the REPLY: **570,583 bytes for 1920 cells, 297 bytes per cell**, because a
//! [`GridBuffer`] on the wire is its derived `Serialize` — every cell a whole struct, its fg and
//! bg fully expanded RGBA objects, its attributes eight named booleans:
//!
//! ```json
//! {"cluster":"a","fg":{"Rgb":{"r":229,"g":229,"b":229,"a":255}},"bg":{"Rgb":{…}},
//!  "attrs":{"bold":false,"dim":false,"italic":false,"underline":"None",…},
//!  "underline_color":null,"hyperlink":null,"width":"Narrow"}
//! ```
//!
//! Size is the reason for the time: building that reply's `serde_json::Value` costs 2.2-3.0 ms of
//! the 4 ms, and a DOM's cost is its NODE COUNT. So the correction is not a faster encoder — it is
//! encoding less.
//!
//! ## What it encodes instead, and why in exactly this shape
//!
//! A terminal cell varies along two axes that compress completely differently. Its **cluster**
//! changes at almost every column in a line of text and repeats hundreds of times across a blank
//! one. Its **style** — colours, attributes, underline colour, hyperlink, width role — changes a
//! handful of times per screen and is drawn from a vocabulary of a few distinct values per frame.
//!
//! Folding both into one run key would couple them: a run would break at every character, and a
//! line of dense text would carry its full style again per glyph. So the two axes are encoded
//! SEPARATELY, by the same mechanism:
//!
//! * each line carries two run lists, `text` and `style`, each a `[len, value]` sequence covering
//!   exactly `cols` columns left to right;
//! * a style run's value is an INDEX into the frame's `styles` table, so a style is spelled once
//!   per frame however many runs name it.
//!
//! This is the shape `scene/snapshot` already uses for this same data (line text plus style runs),
//! which is why R221 could observe three panes of snapshot costing 40,706 bytes against one pane
//! of cells costing 570,605: the compact encoding for a terminal grid was already in the daemon,
//! just not on this path.
//!
//! A run carries only its LENGTH, not its start column: the runs tile the line exhaustively, so a
//! start is derivable and transmitting it would be a second spelling of a fact the reader already
//! has. `scene/snapshot` does carry `start`, and correctly — it is an AI-readable surface whose
//! consumer addresses columns directly. This is a codec, and its consumer is [`decode`].
//!
//! ## The property that has to hold
//!
//! > `decode(encode(buffer)) == buffer`, for every buffer the projection can produce.
//!
//! Not "equal in the fields this module enumerates" — equal by [`GridBuffer`]'s own `PartialEq`,
//! which compares every field it has, including any pinion adds later. That is what makes the
//! round-trip test in `tests/wire.rs` a structural guard rather than an enumerated one: a cell
//! field this module has never heard of breaks the assertion the moment the projection populates
//! it, because the decoded buffer would be missing it. (`TermCell` is `#[non_exhaustive]`, so no
//! exhaustive destructuring is available to catch it at compile time — this is the strongest guard
//! the type allows.)
//!
//! ## Bounds taken deliberately
//!
//! * **Geometry is declared, not materialised.** A run-length payload can name a 65535-column line
//!   in a dozen bytes, so a small payload can ask [`decode`] for a large allocation — amplification
//!   the verbose shape did not have (it needed one serialized cell per cell). `rows` is still
//!   bounded by the payload, since [`decode`] requires one line object per row. The peer is the
//!   client's own daemon over the session socket, the same peer it already trusts for its screen
//!   contents and its keystrokes, so the exposure is a malformed reply rather than a hostile one,
//!   and no arbitrary cap is invented here to pretend otherwise.
//! * **A wire change is lockstep.** This replaces the shape `cells.<offset>` answered before, so a
//!   newly built client cannot read an OLD running daemon's frames. sprag has no protocol version
//!   handshake, so the skew surfaces as a frame fetch that fails to deserialize; `sprag-gui` logs
//!   that case distinctly from the pane-close race it otherwise tolerates.

use std::borrow::Cow;

use pinion_core::{
    CellAttrs, CellWidth, GridBuffer, GridCursor, Hyperlink, HyperlinkId, ScreenKind, TermCell,
    TermColor,
};
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::cluster;

/// A run of `len` consecutive columns all carrying `value` — the one repeated shape, used for
/// both of a line's axes so the encoding has one idea in it rather than two.
type Run<T> = (u16, T);

/// One projected [`GridBuffer`] in the form it crosses the socket in.
///
/// Its fields are private: a `GridWire` is only ever produced by [`encode`] and consumed by
/// [`decode`], so there is no way to hand one out that those two disagree about. The module docs
/// explain why the per-cell axes are split the way they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GridWire {
    /// Column count — the width every line's run lists must tile exactly.
    cols: u16,
    /// Row count — the length of [`generations`](Self::generations) and [`lines`](Self::lines).
    rows: u16,
    /// The grid cursor, verbatim: one per frame, so nothing is gained by re-spelling it.
    cursor: GridCursor,
    /// Main or alternate screen, verbatim.
    screen: ScreenKind,
    /// The producer's per-row damage stamps, one per row. Already small (one integer per row) and
    /// already load-bearing — pinion's `TextGrid` re-rasterizes only rows whose stamp advanced —
    /// so they ride unchanged.
    generations: Vec<u64>,
    /// Every DISTINCT cell style in the frame, interned. A style run names one by index, so a
    /// screen painted in two colours spells two styles however many runs there are.
    styles: Vec<CellStyle>,
    /// One entry per row, in row order.
    lines: Vec<LineWire>,
    /// The buffer's OSC-8 hyperlink interning table, verbatim — it is already interned upstream
    /// (a cell holds an index, not a URI), so there is nothing here to improve.
    hyperlinks: Vec<Hyperlink>,
}

impl GridWire {
    /// How many distinct styles this frame spells — the number the interning exists to keep small,
    /// exposed so a test can assert on it rather than on a byte count that only holds on one box.
    #[must_use]
    pub fn style_count(&self) -> usize {
        self.styles.len()
    }

    /// How many runs this frame spells across both axes of every line — the encoding's real size,
    /// as a COUNT. R221's rule applies: a count repeats to the digit where a duration does not.
    #[must_use]
    pub fn run_count(&self) -> usize {
        self.lines
            .iter()
            .map(|line| line.text.len() + line.style.len())
            .sum()
    }
}

/// One row: the two run lists, each tiling the frame's `cols` columns exactly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LineWire {
    /// The grapheme cluster per column, run-length encoded. A wide cluster's trailer column
    /// carries the empty string, exactly as [`TermCell::trailer`] does, so this tiles every
    /// column rather than only the ones that draw a glyph.
    text: Vec<Run<Cow<'static, str>>>,
    /// The style per column as an index into [`GridWire::styles`], run-length encoded.
    style: Vec<Run<u32>>,
}

/// Everything a [`TermCell`] carries EXCEPT its cluster — the half that repeats across a frame and
/// so is worth interning.
///
/// The fields are pinion's own types, carried verbatim rather than re-spelled compactly. That is
/// deliberate: a style is written once per frame per distinct value, so shrinking it buys a few
/// hundred bytes, while re-spelling it would put a second definition of pinion's cell vocabulary
/// in sprag and make an upstream addition a silent data loss instead of a compile error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CellStyle {
    fg: TermColor,
    bg: TermColor,
    attrs: CellAttrs,
    underline_color: Option<TermColor>,
    hyperlink: Option<HyperlinkId>,
    width: CellWidth,
}

impl CellStyle {
    /// Read a cell's style off it — every field but the cluster.
    fn of(cell: &TermCell) -> Self {
        Self {
            fg: cell.fg,
            bg: cell.bg,
            attrs: cell.attrs,
            underline_color: cell.underline_color,
            hyperlink: cell.hyperlink,
            width: cell.width,
        }
    }

    /// Rebuild a cell from this style and a cluster — the inverse of [`Self::of`], written as
    /// field assignments because [`TermCell`] is `#[non_exhaustive]` and cannot be constructed
    /// literally from outside pinion.
    fn cell(&self, cluster: Cow<'static, str>) -> TermCell {
        let mut cell = TermCell::new(cluster, self.fg, self.bg);
        cell.attrs = self.attrs;
        cell.underline_color = self.underline_color;
        cell.hyperlink = self.hyperlink;
        cell.width = self.width;
        cell
    }
}

/// Encode a projected buffer into its wire form.
///
/// Runs are MAXIMAL: `push_run` extends the run in progress whenever the next column repeats it,
/// so two adjacent runs never carry the same value. [`decode`] does not require that — a
/// non-maximal payload decodes to the same buffer and rejecting it would be stricter than correct
/// — but the encoder produces it, and `tests/wire.rs` holds it to it.
#[must_use]
pub fn encode(buffer: &GridBuffer) -> GridWire {
    let cols = buffer.cols();
    let rows = buffer.rows();
    let mut styles: Vec<CellStyle> = Vec::new();
    let mut lines = Vec::with_capacity(usize::from(rows));

    for row in 0..rows {
        let mut text: Vec<Run<Cow<'static, str>>> = Vec::new();
        let mut style: Vec<Run<u32>> = Vec::new();
        for col in 0..cols {
            // Structurally `Some`: `GridBuffer::new` allocates `cols * rows` cells and `with_row`
            // only overwrites in place, so an in-range coordinate always addresses a cell. An
            // absence here would mean pinion's own sizing invariant had broken, and producing a
            // short line — which `decode` would then reject — is a worse answer than saying so.
            let cell = buffer
                .cell(col, row)
                .expect("a GridBuffer addresses every in-range column (its sizing invariant)");
            // The cluster CLONES, and for the cells a terminal is mostly made of that costs
            // nothing: the projection borrows blanks and printable ASCII out of a `'static`
            // string, so cloning the `Cow` copies a pointer (see `crate::cluster`).
            push_run(&mut text, cell.cluster.clone());
            push_run(&mut style, intern(&mut styles, cell));
        }
        lines.push(LineWire { text, style });
    }

    GridWire {
        cols,
        rows,
        cursor: buffer.cursor(),
        screen: buffer.screen(),
        generations: (0..rows)
            .map(|row| buffer.row_generation(row).unwrap_or_default())
            .collect(),
        styles,
        lines,
        // The table is read back by walking ids from 0 until one misses — the same enumeration
        // `GridBuffer::hyperlink` defines, so an entry no cell happens to reference still travels.
        hyperlinks: (0u32..)
            .map_while(|id| buffer.hyperlink(HyperlinkId(id)).cloned())
            .collect(),
    }
}

/// Rebuild a buffer from its wire form, or say why the payload is malformed.
///
/// Every check here is one a reader downstream would otherwise discover as a panic or a dangling
/// lookup, so they are all made before a [`GridBuffer`] is handed out — the same fail-fast
/// discipline pinion applies in its own `TryFrom<GridBufferWire>`, which this path bypasses by
/// building through the public builders instead of deserializing a buffer directly.
///
/// # Errors
///
/// Returns the reason as a message when the payload does not describe a buffer: a line or
/// generation count that disagrees with `rows`, a run list that does not tile `cols` exactly, a
/// zero-length run, a style index with no entry, or a hyperlink index with no table entry.
pub fn decode(wire: GridWire) -> Result<GridBuffer, String> {
    let GridWire {
        cols,
        rows,
        cursor,
        screen,
        generations,
        styles,
        lines,
        hyperlinks,
    } = wire;

    if generations.len() != usize::from(rows) {
        return Err(format!(
            "grid row generation count {} does not match rows = {rows}",
            generations.len(),
        ));
    }
    if lines.len() != usize::from(rows) {
        return Err(format!(
            "grid line count {} does not match rows = {rows}",
            lines.len(),
        ));
    }
    // A style's hyperlink index is checked ONCE per style rather than once per cell: the styles
    // are the only place an index can appear, so this covers every cell that will name one and
    // keeps `GridBuffer::hyperlink` from ever dangling — the guarantee pinion's own validator
    // gives on its deserialize path.
    if let Some((index, style)) = styles.iter().enumerate().find(|(_, style)| {
        style
            .hyperlink
            .is_some_and(|id| usize::try_from(id.0).unwrap_or(usize::MAX) >= hyperlinks.len())
    }) {
        return Err(format!(
            "grid style {index} hyperlink index {} is out of range for a {}-entry table",
            style.hyperlink.unwrap_or(HyperlinkId(0)).0,
            hyperlinks.len(),
        ));
    }

    let mut buffer = GridBuffer::new(cols, rows);
    for (row, line) in lines.iter().enumerate() {
        let row = u16::try_from(row).unwrap_or(u16::MAX);
        let mut text = RunReader::new(&line.text, cols, "text")?;
        let mut style = RunReader::new(&line.style, cols, "style")?;
        let mut cells = Vec::with_capacity(usize::from(cols));
        for _ in 0..cols {
            // Both readers were validated to tile `cols`, so neither runs dry inside this loop.
            let (cluster_text, style_index) = (text.next(), style.next());
            let index = usize::try_from(*style_index).unwrap_or(usize::MAX);
            let Some(found) = styles.get(index) else {
                return Err(format!(
                    "grid style index {index} is out of range for a {}-entry table",
                    styles.len(),
                ));
            };
            // Routed through the projection's OWN cluster function, so a decoded blank or ASCII
            // cell BORROWS its glyph exactly as a projected one does. That is not cosmetic: the
            // display client deep-copies its mirrored buffer on every pane on every painted frame,
            // and a borrowed `Cow` clones by copying a pointer. R219 removed that per-cell
            // allocation on the producing side; decoding this way is what carries it to the
            // consuming side, which serde's own `Cow` impl — always `Owned` — could not.
            cells.push(found.cell(cluster(cluster_text)));
        }
        buffer = buffer.with_row(row, cells);
    }

    for (row, generation) in generations.into_iter().enumerate() {
        buffer = buffer.with_row_generation(u16::try_from(row).unwrap_or(u16::MAX), generation);
    }
    Ok(buffer
        .with_cursor(cursor)
        .with_screen(screen)
        .with_hyperlinks(hyperlinks))
}

/// Serialize a [`GridBuffer`] in this module's form — the `serialize` half of
/// `#[serde(with = "sprag_grid::wire")]`.
///
/// # Errors
///
/// Propagates the serializer's own error; the encoding itself cannot fail.
pub fn serialize<S>(buffer: &GridBuffer, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    encode(buffer).serialize(serializer)
}

/// Deserialize a [`GridBuffer`] from this module's form — the `deserialize` half of
/// `#[serde(with = "sprag_grid::wire")]`.
///
/// # Errors
///
/// The deserializer's own error for a payload that is not a [`GridWire`], or [`decode`]'s message
/// for one that is but does not describe a buffer.
pub fn deserialize<'de, D>(deserializer: D) -> Result<GridBuffer, D::Error>
where
    D: Deserializer<'de>,
{
    decode(GridWire::deserialize(deserializer)?).map_err(D::Error::custom)
}

/// Extend the run in progress, or start a new one — the whole of the run-length encoder, shared by
/// both axes so neither can grow its own idea of what a run is.
fn push_run<T: PartialEq>(runs: &mut Vec<Run<T>>, value: T) {
    match runs.last_mut() {
        Some((len, last)) if *last == value => *len += 1,
        _ => runs.push((1, value)),
    }
}

/// Find `cell`'s style in the frame's table, adding it if this is its first appearance.
///
/// A linear scan, and deliberately: interning happens once per RUN rather than once per cell, so
/// the scan runs a few hundred times per frame against a table of a few entries. A hash map would
/// need [`CellStyle`] to be `Hash`, which would mean requiring it of every pinion type it carries.
fn intern(styles: &mut Vec<CellStyle>, cell: &TermCell) -> u32 {
    let style = CellStyle::of(cell);
    let index = styles
        .iter()
        .position(|known| *known == style)
        .unwrap_or_else(|| {
            styles.push(style);
            styles.len() - 1
        });
    // A `GridBuffer` holds at most `u16::MAX * u16::MAX` = 4,294,836,225 cells, which is fewer
    // than `u32::MAX`, so a style table indexed one-per-cell in the worst case still fits.
    u32::try_from(index).expect("a frame has fewer distinct styles than u32::MAX")
}

/// Walks a validated run list one column at a time.
///
/// Construction is where the tiling is checked, so the walk itself cannot run dry: a reader that
/// exists has been proven to cover exactly `cols` columns with no empty run.
struct RunReader<'a, T> {
    runs: &'a [Run<T>],
    /// Which run the next column comes from.
    index: usize,
    /// How many columns are left in that run.
    left: u16,
}

impl<'a, T> RunReader<'a, T> {
    /// Check that `runs` tiles `cols` columns exactly, then open a reader over it.
    fn new(runs: &'a [Run<T>], cols: u16, axis: &str) -> Result<Self, String> {
        let mut total: u32 = 0;
        for (len, _) in runs {
            if *len == 0 {
                return Err(format!("grid {axis} run list holds a zero-length run"));
            }
            total += u32::from(*len);
        }
        if total != u32::from(cols) {
            return Err(format!(
                "grid {axis} run list covers {total} columns, not cols = {cols}",
            ));
        }
        Ok(Self {
            runs,
            index: 0,
            left: runs.first().map_or(0, |(len, _)| *len),
        })
    }

    /// The next column's value.
    ///
    /// # Panics
    ///
    /// Only if called more than `cols` times, which [`decode`] does not do — the loop that drives
    /// this is bounded by the very `cols` [`Self::new`] validated the runs against.
    fn next(&mut self) -> &'a T {
        while self.left == 0 {
            self.index += 1;
            self.left = self.runs[self.index].0;
        }
        self.left -= 1;
        &self.runs[self.index].1
    }
}
