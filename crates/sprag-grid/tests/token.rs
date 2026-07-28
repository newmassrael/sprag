//! The projection token's safety property, and the axis matrix behind it.
//!
//! A display client uses [`sprag_grid::projection_token`] to decide whether re-fetching a pane's
//! cells would tell it anything new. The whole arrangement rests on ONE claim:
//!
//! > equal token ⇒ equal projection
//!
//! If that ever fails, a pane freezes on screen while its child keeps writing — a visible, silent
//! defect. So this file does not spot-check the fields; it drives a battery of real VT sequences
//! and asserts the implication holds for every one of them. A field the token FORGETS shows up
//! here as a driver that left the token equal and the projection different, whatever the reason,
//! including reasons nobody enumerated.
//!
//! ## Why the converse is not asserted
//!
//! Equal projection ⇒ equal token does NOT hold, and asserting it would be wrong. Writing a
//! character over an identical one stamps its row: the token moves, the projection does not. That
//! costs a redundant fetch and never a stale pane, so the asymmetry is the safe one. The handful of
//! drivers that genuinely must leave BOTH untouched are listed separately below, as an efficiency
//! check rather than a correctness one.

use pinion_core::GridBuffer;
use sprag_grid::{project, projection_token};
use sprag_vt::{Emulator, VtPort};

/// What a client actually receives for a pane — the projected cells AND the non-cell facts that
/// ride with them in the same frame (`sprag_host::PaneScrollFacts`: history depth and the visible
/// row count). The token guards the whole frame, so the property is stated over the whole frame:
/// a driver that changes only the history depth still has to force a re-fetch, or the scrollbar
/// freezes while the cells stay right.
fn frame(emulator: &Emulator) -> (GridBuffer, usize, u16) {
    let screen = VtPort::screen(emulator);
    (
        project(screen, VtPort::palette(emulator)),
        screen.scrollback_len(),
        screen.rows(),
    )
}

/// Read the token off an emulator.
fn token(emulator: &Emulator) -> sprag_grid::ProjectionToken {
    projection_token(VtPort::screen(emulator), VtPort::palette(emulator))
}

/// A pane-sized screen with a prompt on it — a realistic starting state, so a driver that only
/// matters against existing content (a re-colour, a reflow) has content to matter against.
fn started() -> Emulator {
    let mut emulator = Emulator::new(40, 8);
    emulator.advance(b"\x1b[1;32muser@host\x1b[0m:~$ ls -la\r\ntotal 4\r\n");
    emulator
}

/// One driver in the battery: what it is called, the state it starts from, and what it does.
///
/// `start` is separate because two axes are only reachable from a state the default start is not
/// in — a width resize takes the stamp-PRESERVING copy path only on the alternate screen, and
/// dropping the scrollback needs a scrollback to drop.
struct Axis {
    name: &'static str,
    start: fn() -> Emulator,
    drive: fn(&mut Emulator),
}

/// Drivers that CHANGE the frame a client would receive. Each must move the token, and the
/// implication must hold for each — the two checks the axis matrix exists to make.
fn changing() -> Vec<Axis> {
    vec![
        Axis {
            start: started,
            name: "printed output",
            drive: |e| e.advance(b"drwxr-xr-x  2 user user 4096 .\r\n"),
        },
        Axis {
            start: started,
            name: "a bare cursor move (stamps no row)",
            drive: |e| e.advance(b"\x1b[5;3H"),
        },
        Axis {
            start: started,
            name: "DECTCEM hides the cursor",
            drive: |e| e.advance(b"\x1b[?25l"),
        },
        Axis {
            start: started,
            name: "DECSCUSR changes the cursor shape and blink",
            drive: |e| e.advance(b"\x1b[5 q"),
        },
        Axis {
            start: started,
            name: "OSC 4 redefines a palette index",
            drive: |e| e.advance(b"\x1b]4;2;rgb:00/00/ff\x1b\\"),
        },
        Axis {
            start: started,
            name: "OSC 10 changes the default foreground",
            drive: |e| e.advance(b"\x1b]10;rgb:11/22/33\x1b\\"),
        },
        Axis {
            start: started,
            name: "OSC 11 changes the default background",
            drive: |e| e.advance(b"\x1b]11;rgb:44/55/66\x1b\\"),
        },
        Axis {
            start: started,
            // The one the emulator documents as deliberately damaging no cell — and which the
            // projection nonetheless carries, on the cursor.
            name: "OSC 12 sets the cursor colour (no cell damage by design)",
            drive: |e| e.advance(b"\x1b]12;rgb:00/00/ff\x1b\\"),
        },
        Axis {
            start: started,
            name: "the alternate screen",
            drive: |e| e.advance(b"\x1b[?1049h"),
        },
        Axis {
            start: started,
            name: "a scroll that grows the scrollback",
            drive: |e| e.advance(b"\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n"),
        },
        Axis {
            start: started,
            name: "erasing the display",
            drive: |e| e.advance(b"\x1b[2J"),
        },
        Axis {
            start: started,
            name: "an SGR-styled run",
            drive: |e| e.advance(b"\x1b[1;4;31mERROR\x1b[0m"),
        },
        Axis {
            start: started,
            name: "an OSC 8 hyperlink",
            drive: |e| e.advance(b"\x1b]8;;https://example\x1b\\link\x1b]8;;\x1b\\"),
        },
        Axis {
            start: started,
            name: "a wide CJK cluster",
            drive: |e| e.advance("世界".as_bytes()),
        },
        Axis {
            start: started,
            // A resize COPIES surviving rows' stamps, so the generations alone would not notice it.
            name: "a narrower resize",
            drive: |e| e.resize(24, 8),
        },
        Axis {
            start: started,
            name: "a taller resize",
            drive: |e| e.resize(40, 16),
        },
        Axis {
            // The main-screen resize above REFLOWS, which restamps every row, so it would pass
            // with a token that had forgotten the width entirely. The alternate screen takes the
            // copy path instead (an alt-screen app owns its own layout, so history is not
            // rewrapped) — surviving rows keep their stamps, and the WIDTH is then the only thing
            // that moved. This is the axis that proves `cols` is load-bearing.
            start: || {
                let mut e = started();
                e.advance(b"\x1b[?1049hALT SCREEN CONTENT");
                e
            },
            name: "a width resize on the alternate screen (the stamp-preserving copy path)",
            drive: |e| e.resize(24, 8),
        },
        Axis {
            // ED 3 drops the retained history and stamps NO row, so the cells are untouched and
            // only the frame's `scrollback_len` moves. This is the axis that proves the token has
            // to guard the whole frame rather than just the projection.
            start: || {
                let mut e = started();
                e.advance(b"\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n");
                e
            },
            name: "ED 3 drops the scrollback without stamping a row",
            drive: |e| e.advance(b"\x1b[3J"),
        },
    ]
}

/// Drivers that must change NOTHING a projection reads. These are the efficiency half: if one of
/// them moved the token, every client would re-fetch every pane over a window title.
fn inert() -> Vec<Axis> {
    vec![
        Axis {
            start: started,
            name: "an OSC 0 window title",
            drive: |e| e.advance(b"\x1b]0;a new title\x1b\\"),
        },
        Axis {
            start: started,
            name: "a bell",
            drive: |e| e.advance(b"\x07"),
        },
        Axis {
            start: started,
            name: "an OSC 9 notification",
            drive: |e| e.advance(b"\x1b]9;build finished\x1b\\"),
        },
        Axis {
            start: started,
            name: "a device-attributes query",
            drive: |e| e.advance(b"\x1b[c"),
        },
    ]
}

#[test]
fn an_equal_token_means_an_equal_projection() {
    for axis in changing().into_iter().chain(inert()) {
        let mut emulator = (axis.start)();
        let (before_token, before) = (token(&emulator), frame(&emulator));

        (axis.drive)(&mut emulator);

        let (after_token, after) = (token(&emulator), frame(&emulator));

        // THE safety property. A driver that changed the projection while leaving the token equal
        // is a pane that would freeze on a client that trusted the token.
        if before_token == after_token {
            assert_eq!(
                before, after,
                "`{}` changed the frame without moving the token — a client would freeze this pane",
                axis.name,
            );
        }
    }
}

#[test]
fn every_axis_that_changes_the_projection_moves_the_token() {
    for axis in changing() {
        let mut emulator = (axis.start)();
        let (before_token, before) = (token(&emulator), frame(&emulator));

        (axis.drive)(&mut emulator);

        let (after_token, after) = (token(&emulator), frame(&emulator));

        // Non-vacuity first: an axis listed as changing that did not change the projection is a
        // mis-written driver, and would make the token assertion below prove nothing.
        assert_ne!(
            before, after,
            "`{}` is listed as changing the frame but did not — fix the driver, not the token",
            axis.name,
        );
        assert_ne!(
            before_token, after_token,
            "`{}` changed the frame, so the token must move",
            axis.name,
        );
    }
}

#[test]
fn an_inert_driver_moves_neither() {
    for axis in inert() {
        let mut emulator = (axis.start)();
        let (before_token, before) = (token(&emulator), frame(&emulator));

        (axis.drive)(&mut emulator);

        assert_eq!(
            before,
            frame(&emulator),
            "`{}` is listed as inert but changed the frame",
            axis.name,
        );
        assert_eq!(
            before_token,
            token(&emulator),
            "`{}` changes nothing a projection reads, so it must not cost a re-fetch",
            axis.name,
        );
    }
}
