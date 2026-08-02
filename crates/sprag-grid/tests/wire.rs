//! What the frame's wire form has to be worth, and what it must never cost.
//!
//! [`sprag_grid::wire`] replaces the shape a projected `GridBuffer` crossed the socket in. It is
//! worth having only if two things hold, and they pull against each other:
//!
//! > **exact** — `decode(encode(buffer)) == buffer`, for every buffer the projection can produce;
//! > **smaller** — by enough that R221's measurement of the reply, not the projection, as the cost
//! > of a `cells.<offset>` fetch is actually answered.
//!
//! The exactness half is asserted with `GridBuffer`'s own `PartialEq`, which compares every field
//! it has. That makes it a structural guard: a cell field this codec has never heard of fails the
//! assertion the moment the projection starts populating it, because the decoded buffer would be
//! missing it. `TermCell` is `#[non_exhaustive]`, so no exhaustive destructuring is available to
//! catch that at compile time, and this is the strongest guard the type allows.
//!
//! The smaller half is stated the way this crate's other measurements are — as a COUNT and, where
//! possible, as an EQUALITY rather than a tuned threshold. See
//! [`run_count_follows_the_content_not_the_column_count`].

use std::borrow::Cow;

use pinion_core::GridBuffer;
use serde::Deserialize;
use sprag_grid::wire;
use sprag_grid::{project, project_scrolled};
use sprag_vt::{Emulator, VtPort};

/// The seam `sprag_host::CellFrame` uses, standing in for it here so the tests drive the real
/// `#[serde(with = …)]` path — including the error a malformed payload produces — without
/// sprag-grid depending on the host crate.
#[derive(Deserialize)]
struct Framed {
    #[serde(with = "sprag_grid::wire")]
    cells: GridBuffer,
}

/// A pane-sized screen with a prompt on it, so a driver that only matters against existing content
/// (a re-colour, a reflow) has content to matter against. Mirrors `tests/token.rs`'s start state.
fn started() -> Emulator {
    let mut emulator = Emulator::new(40, 8);
    emulator.advance(b"\x1b[1;32muser@host\x1b[0m:~$ ls -la\r\ntotal 4\r\n");
    emulator
}

/// One driver in the battery: what it is called and what it does to a started screen.
struct Axis {
    name: &'static str,
    drive: fn(&mut Emulator),
}

/// Every state a projection can be asked for, driven by a real VT sequence rather than assembled
/// by hand — so a cell shape nobody enumerated is still covered, as long as a terminal can reach
/// it. The list mirrors `tests/token.rs`'s axis matrix, which was built to reach every input the
/// projection reads; here the same reach is what makes the round-trip claim general.
fn battery() -> Vec<Axis> {
    vec![
        Axis {
            name: "a bare started screen",
            drive: |_| {},
        },
        Axis {
            name: "printed output",
            drive: |e| e.advance(b"drwxr-xr-x  2 user user 4096 .\r\n"),
        },
        Axis {
            name: "an SGR-styled run",
            drive: |e| e.advance(b"\x1b[1;4;31mERROR\x1b[0m"),
        },
        Axis {
            name: "every SGR attribute at once",
            drive: |e| e.advance(b"\x1b[1;2;3;4;5;7;8;9mALL\x1b[0m"),
        },
        Axis {
            name: "a curly underline in its own SGR 58 colour",
            drive: |e| e.advance(b"\x1b[4:3m\x1b[58;2;255;0;0mtypo\x1b[59m\x1b[0m"),
        },
        Axis {
            name: "24-bit truecolor",
            drive: |e| e.advance(b"\x1b[38;2;12;34;56m\x1b[48;2;65;43;21mtrue\x1b[0m"),
        },
        Axis {
            name: "an indexed 256-colour pair",
            drive: |e| e.advance(b"\x1b[38;5;208m\x1b[48;5;17midx\x1b[0m"),
        },
        Axis {
            name: "OSC 4 redefines a palette index",
            drive: |e| e.advance(b"\x1b]4;2;rgb:00/00/ff\x1b\\"),
        },
        Axis {
            name: "OSC 10 / 11 change the defaults",
            drive: |e| e.advance(b"\x1b]10;rgb:11/22/33\x1b\\\x1b]11;rgb:44/55/66\x1b\\"),
        },
        Axis {
            name: "OSC 12 sets the cursor colour",
            drive: |e| e.advance(b"\x1b]12;rgb:00/00/ff\x1b\\"),
        },
        Axis {
            name: "DECTCEM hides the cursor",
            drive: |e| e.advance(b"\x1b[?25l"),
        },
        Axis {
            name: "DECSCUSR changes the cursor shape",
            drive: |e| e.advance(b"\x1b[5 q"),
        },
        Axis {
            name: "a bare cursor move",
            drive: |e| e.advance(b"\x1b[5;3H"),
        },
        Axis {
            name: "an OSC 8 hyperlink",
            drive: |e| e.advance(b"\x1b]8;;https://example\x1b\\link\x1b]8;;\x1b\\"),
        },
        Axis {
            name: "two OSC 8 hyperlinks, one of them grouped by id",
            drive: |e| {
                e.advance(b"\x1b]8;id=one;https://a\x1b\\A\x1b]8;;\x1b\\ ");
                e.advance(b"\x1b]8;;https://b\x1b\\B\x1b]8;;\x1b\\");
            },
        },
        Axis {
            name: "a wide CJK cluster",
            drive: |e| e.advance("世界".as_bytes()),
        },
        Axis {
            name: "a wide cluster at the right edge (a clipped head)",
            drive: |e| {
                e.advance(b"\x1b[1;40H");
                e.advance("世".as_bytes());
            },
        },
        Axis {
            name: "a combining cluster and an emoji",
            drive: |e| e.advance("e\u{301} 🙂".as_bytes()),
        },
        Axis {
            name: "the alternate screen",
            drive: |e| e.advance(b"\x1b[?1049hALT"),
        },
        Axis {
            name: "erasing the display",
            drive: |e| e.advance(b"\x1b[2J"),
        },
        Axis {
            name: "a scroll that grows the scrollback",
            drive: |e| {
                e.advance(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\nsix\r\nseven\r\neight\r\nnine\r\nten\r\n")
            },
        },
        Axis {
            name: "styled output scrolled into the history",
            drive: |e| {
                for line in 0..12u8 {
                    e.advance(b"\x1b[33m");
                    e.advance(format!("line {line}\r\n").as_bytes());
                }
                e.advance(b"\x1b[0m");
            },
        },
        Axis {
            name: "a narrower resize",
            drive: |e| e.resize(24, 8),
        },
        Axis {
            name: "a wider resize",
            drive: |e| e.resize(96, 8),
        },
    ]
}

/// THE claim. Every state the battery reaches, projected live and at three history offsets,
/// survives the round trip byte for byte — asserted with `GridBuffer`'s own equality, so a field
/// this codec forgets is a failure whatever the field turns out to be.
///
/// The history offsets are not decoration: `project_scrolled` builds its rows from the scrollback
/// through a DIFFERENT path than the live grid (`project_glyph_row`, which pads short rows where
/// the live path does not), and it deliberately reports no cursor. A codec proven only against
/// live frames would be proven against half the frames a client actually fetches.
#[test]
fn the_round_trip_is_exact_over_every_state_the_battery_reaches() {
    for axis in battery() {
        let mut emulator = started();
        (axis.drive)(&mut emulator);
        let screen = VtPort::screen(&emulator);
        let palette = VtPort::palette(&emulator);

        for offset in [0usize, 1, 3, 99] {
            let original = project_scrolled(screen, offset, palette);
            let decoded = wire::decode(wire::encode(&original))
                .unwrap_or_else(|error| panic!("{}, offset {offset}: {error}", axis.name));
            assert_eq!(
                decoded, original,
                "{}, offset {offset}: the wire form lost something",
                axis.name,
            );
        }
    }
}

/// The same claim through the real serde seam rather than the encode/decode pair — the path
/// `CellFrame` actually takes, JSON text and all. A codec that round-trips in memory but whose
/// serde wiring drops a field would pass the test above and fail in production.
#[test]
fn the_round_trip_survives_the_json_the_socket_carries() {
    for axis in battery() {
        let mut emulator = started();
        (axis.drive)(&mut emulator);
        let original = project(VtPort::screen(&emulator), VtPort::palette(&emulator));

        let json = serde_json::to_string(&Wrapped {
            cells: original.clone(),
        })
        .expect("a frame encodes");
        let back: Framed =
            serde_json::from_str(&json).unwrap_or_else(|error| panic!("{}: {error}", axis.name));

        assert_eq!(
            back.cells, original,
            "{}: the JSON lost something",
            axis.name
        );
    }
}

/// The serialize half of [`Framed`] — separate because `#[serde(with)]` needs the field on a
/// struct, and a test that shares one struct for both directions cannot show which half broke.
#[derive(serde::Serialize)]
struct Wrapped {
    #[serde(with = "sprag_grid::wire")]
    cells: GridBuffer,
}

/// The size claim, stated as an EQUALITY so it needs no tuned constant (the discipline
/// `tests/allocs.rs` established): the encoding's size follows the CONTENT, not the cell count.
///
/// Two screens carrying the same text at very different widths hold wildly different numbers of
/// cells — and produce the SAME number of runs, because the extra columns are blank and fold into
/// the run already in progress. That is the whole mechanism in one assertion, and it fails loudly
/// the moment a per-cell term returns: the wide screen would simply cost more.
#[test]
fn run_count_follows_the_content_not_the_column_count() {
    let narrow = encoded_line_of("\x1b[31mred\x1b[0m plain", 40);
    let wide = encoded_line_of("\x1b[31mred\x1b[0m plain", 400);

    assert_eq!(
        narrow.run_count(),
        wide.run_count(),
        "ten times the columns must not cost one more run",
    );
    assert_eq!(
        narrow.style_count(),
        wide.style_count(),
        "ten times the columns must not cost one more style",
    );
}

/// The same content on a screen `cols` wide, encoded.
fn encoded_line_of(content: &str, cols: u16) -> wire::GridWire {
    let mut emulator = Emulator::new(cols, 8);
    emulator.advance(content.as_bytes());
    wire::encode(&project(
        VtPort::screen(&emulator),
        VtPort::palette(&emulator),
    ))
}

/// What the change is worth in bytes, as a floor rather than a measurement.
///
/// Both figures are exact byte counts of deterministic encodings, so this cannot flake; the floor
/// is set well under the observed ratio because the point is that the improvement is an order of
/// magnitude, not that it is a particular number this month. The failure it guards is a future
/// edit that quietly re-introduces a per-cell term — the ratio would collapse toward 1.
#[test]
fn the_wire_form_is_at_least_twenty_times_smaller_than_the_derived_one() {
    let mut emulator = Emulator::new(80, 24);
    emulator.advance(b"\x1b[1;32muser@host\x1b[0m:~$ cargo test\r\n");
    emulator.advance(b"   Compiling sprag-grid v0.0.1\r\n    Finished in 0.66s\r\n");
    let buffer = project(VtPort::screen(&emulator), VtPort::palette(&emulator));

    let derived = serde_json::to_string(&buffer)
        .expect("a buffer encodes")
        .len();
    let compact = serde_json::to_string(&wire::encode(&buffer))
        .expect("a wire form encodes")
        .len();

    assert!(
        compact * 20 <= derived,
        "the wire form is {compact} bytes against the derived shape's {derived} — \
         a factor of {:.1}, under the floor of 20",
        derived as f64 / compact as f64,
    );
}

/// Runs are MAXIMAL — no two adjacent runs carry the same value.
///
/// The decoder deliberately does not require this (a non-maximal payload decodes to the same
/// buffer, and rejecting it would be stricter than correct), so nothing else would notice an
/// encoder that stopped folding. It would simply cost more, silently, which is exactly the class
/// of regression this round exists to remove.
#[test]
fn the_encoder_folds_every_run_it_can() {
    let mut emulator = Emulator::new(40, 8);
    emulator.advance(b"\x1b[31maaa\x1b[32mbbb\x1b[0m");
    let encoded = wire::encode(&project(
        VtPort::screen(&emulator),
        VtPort::palette(&emulator),
    ));
    let json = serde_json::to_value(&encoded).expect("a wire form encodes");

    for line in json["lines"].as_array().expect("lines is an array") {
        for axis in ["text", "style"] {
            let runs = line[axis].as_array().expect("an axis is an array");
            for pair in runs.windows(2) {
                assert_ne!(
                    pair[0][1], pair[1][1],
                    "two adjacent {axis} runs carry the same value: {runs:?}",
                );
            }
        }
    }
}

/// A decoded blank or ASCII cell BORROWS its cluster, exactly as a projected one does.
///
/// This is the half of R219 that never reached the display client. Serde's own `Cow` impl always
/// produces `Owned`, so before this codec every cell of every frame the client deserialized owned
/// a heap string — and the client deep-copies its mirrored buffer on every pane on every painted
/// frame. Routing the decoded cluster through the projection's own `cluster` function is what
/// makes that copy allocation-free for the cells a terminal is mostly made of.
///
/// Asserted on the `Cow` DISCRIMINANT rather than through an allocation counter: the counter is a
/// whole-binary instrument (`tests/allocs.rs` holds exactly one test for that reason), and the
/// discriminant states the claim exactly.
///
/// It goes through the JSON, and that is not incidental. An in-memory `decode(encode(…))` clones
/// the ORIGINAL cells' `Cow`s, which the projection already borrowed, so it reports `Borrowed`
/// whatever the decoder does — this test passed against a decoder with the rule deliberately
/// removed before it was written this way. The claim is about what a client receives off a socket,
/// so only a payload that has actually been text can test it.
#[test]
fn a_decoded_ascii_cell_borrows_its_cluster() {
    let mut emulator = Emulator::new(40, 8);
    emulator.advance("hi 世".as_bytes());
    let original = project(VtPort::screen(&emulator), VtPort::palette(&emulator));
    let json = serde_json::to_string(&Wrapped { cells: original }).expect("a frame encodes");
    let decoded = serde_json::from_str::<Framed>(&json)
        .expect("a frame decodes")
        .cells;

    let borrowed = |col: u16, row: u16| {
        matches!(
            decoded.cell(col, row).expect("an in-range cell").cluster,
            Cow::Borrowed(_),
        )
    };

    assert!(borrowed(0, 0), "an ASCII glyph borrows");
    assert!(borrowed(30, 0), "a blank borrows");
    assert!(
        borrowed(4, 0),
        "a wide cluster's trailer borrows its empty string"
    );
    assert!(
        !borrowed(3, 0),
        "a wide CJK cluster is outside the nameable range and must still own its string",
    );
}

/// Every way a payload can fail to describe a buffer is caught BEFORE a `GridBuffer` is handed
/// out, and says which one it was.
///
/// The checks matter because this path bypasses pinion's own `TryFrom<GridBufferWire>` validator:
/// it builds through the public builders instead of deserializing a buffer directly, so the
/// guarantees that validator gives — a cell count that matches the geometry, a hyperlink index
/// that never dangles — have to be given here or not at all.
#[test]
fn a_malformed_payload_is_rejected_with_its_reason() {
    let cases: [(&str, &str, &str); 6] = [
        (
            "a line count that disagrees with rows",
            r#"{"cols":1,"rows":2,"cursor":{"col":0,"row":0,"shape":"Block","visible":false},
                "screen":"Main","generations":[0,0],"styles":[],"lines":[],"hyperlinks":[]}"#,
            "line count",
        ),
        (
            "a generation count that disagrees with rows",
            r#"{"cols":1,"rows":1,"cursor":{"col":0,"row":0,"shape":"Block","visible":false},
                "screen":"Main","generations":[],"styles":[],"lines":[],"hyperlinks":[]}"#,
            "generation count",
        ),
        (
            "text runs that do not tile the row",
            r#"{"cols":4,"rows":1,"cursor":{"col":0,"row":0,"shape":"Block","visible":false},
                "screen":"Main","generations":[0],"styles":[STYLE],
                "lines":[{"text":[[2," "]],"style":[[4,0]]}],"hyperlinks":[]}"#,
            "text run list covers 2 columns",
        ),
        (
            "a zero-length run",
            r#"{"cols":4,"rows":1,"cursor":{"col":0,"row":0,"shape":"Block","visible":false},
                "screen":"Main","generations":[0],"styles":[STYLE],
                "lines":[{"text":[[0," "],[4," "]],"style":[[4,0]]}],"hyperlinks":[]}"#,
            "zero-length run",
        ),
        (
            "a style index with no entry",
            r#"{"cols":4,"rows":1,"cursor":{"col":0,"row":0,"shape":"Block","visible":false},
                "screen":"Main","generations":[0],"styles":[],
                "lines":[{"text":[[4," "]],"style":[[4,0]]}],"hyperlinks":[]}"#,
            "style index 0 is out of range",
        ),
        (
            "a hyperlink index with no table entry",
            r#"{"cols":4,"rows":1,"cursor":{"col":0,"row":0,"shape":"Block","visible":false},
                "screen":"Main","generations":[0],"styles":[LINKED],
                "lines":[{"text":[[4," "]],"style":[[4,0]]}],"hyperlinks":[]}"#,
            "hyperlink index 0 is out of range",
        ),
    ];

    // One well-formed style, spelled once — the cases differ in the ONE thing each is named for.
    const STYLE: &str = r#"{"fg":"Default","bg":"Default","attrs":{"bold":false,"dim":false,
        "italic":false,"underline":"none","blink":false,"reverse":false,"hidden":false,
        "strikethrough":false},"underline_color":null,"hyperlink":null,"width":"Narrow"}"#;

    for (name, payload, expected) in cases {
        let json = payload.replace("STYLE", STYLE).replace(
            "LINKED",
            &STYLE.replace(r#""hyperlink":null"#, r#""hyperlink":0"#),
        );
        let error = serde_json::from_str::<Framed>(&format!(r#"{{"cells":{json}}}"#))
            .err()
            .unwrap_or_else(|| panic!("{name} must be rejected"));
        assert!(
            error.to_string().contains(expected),
            "{name}: expected a message naming {expected:?}, got {error}",
        );
    }
}

/// A well-formed payload that the encoder would never produce still decodes, and to the right
/// buffer — the other side of the maximality decision above. Rejecting a non-maximal payload
/// would be rejecting a correct one.
#[test]
fn a_non_maximal_payload_is_correct_and_accepted() {
    const STYLE: &str = r#"{"fg":"Default","bg":"Default","attrs":{"bold":false,"dim":false,
        "italic":false,"underline":"none","blink":false,"reverse":false,"hidden":false,
        "strikethrough":false},"underline_color":null,"hyperlink":null,"width":"Narrow"}"#;
    let split = format!(
        r#"{{"cells":{{"cols":4,"rows":1,"cursor":{{"col":0,"row":0,"shape":"Block",
           "visible":false}},"screen":"Main","generations":[7],"styles":[{STYLE}],
           "lines":[{{"text":[[1," "],[3," "]],"style":[[2,0],[2,0]]}}],"hyperlinks":[]}}}}"#,
    );

    let framed: Framed = serde_json::from_str(&split).expect("a split-run payload decodes");
    let folded = GridBuffer::new(4, 1).with_row_generation(0, 7);

    assert_eq!(
        framed.cells, folded,
        "a payload split into redundant runs must decode to the same buffer as a folded one",
    );
}
