//! WHAT THE LAST KEY DID, as a strip this client puts over the bottom of its window (R316).
//!
//! The windowed half of [`sprag_host::report`]. The sentence, the deadline and the decision about
//! which outcomes are worth saying out loud are all in that shared module, so this front and
//! `sprag-tui` cannot come to disagree about what a key did. **Nothing in this file knows what a
//! session is**, which is the same claim [`crate::chooser`] opens with and for the same reason.
//!
//! # Why a strip and not the terminal front's status ROW
//!
//! `sprag-tui` reserves its bottom row permanently, because a terminal client has no chrome at all
//! and *where am I* had nowhere else to go. This window already answers that: the session rail and
//! the window tab strip name the session and its windows, and they are painted from the same two
//! facts [`sprag_host::status::Status`] reads. What is missing here is only the NEGATIVE half — the
//! key that did nothing — so only that half is built.
//!
//! It is an OVERLAY rather than a layout child, and that is not a style preference: a strip that
//! joined the column would resize the pane grid every time a message appeared and again when it
//! expired, so the daemon would re-arbitrate the window twice per refused keystroke. The panes must
//! not move because a client said something.
//!
//! # The expiry needs a WAKE, and this is where the one timer lives
//!
//! A pinion window repaints on damage, so a message whose deadline passes with nothing else
//! happening would sit on the screen until the next event. One detached thread per shown message
//! sleeps to the deadline and asks for a repaint. It is bounded by construction — a thread lives at
//! most `display-time`, and only a keystroke that SAID something starts one — and it is the same
//! `RepaintSink` seam the PTY producer already wakes this client through.

use pinion_a11y::{AccessNode, AccessValue, AriaRole};
use pinion_core::Scene;
use pinion_core::reactive::{Owner, Signal};
use pinion_core::scene::{ContainerNode, Rect, TextNode};
use pinion_core::style::{
    AlignItems, BoxStyle, FlexDirection, JustifyContent, LayoutStyle, Size, SizeValue, TextStyle,
};
use pinion_core::theme::{ColorRole, Theme};
use sprag_host::report::{Message, Report, display_time, now};

/// The strip's tag — where a test reads the sentence this client is showing, rather than inferring
/// it from pixels.
pub(crate) const MESSAGE_STRIP_TAG: &str = "sprag_message_strip";

/// `Owner::cache` key for the live message.
const MESSAGE_KEY: &str = "sprag_gui.message";

/// The strip's height in pixels — one line of [`FONT_PX`] with room to breathe.
const STRIP_H: u32 = 28;
/// The sentence's size.
const FONT_PX: u32 = 14;
/// How far the strip sits from the window's edges.
const MARGIN: u32 = 8;

/// The message being shown, or `None` when the client has nothing to say.
fn use_message() -> Signal<Option<Message>> {
    Owner::current()
        .expect("use_message() requires an active Owner scope")
        .cache(MESSAGE_KEY, || Signal::new(None))
        .as_ref()
        .clone()
}

/// Show what a GESTURE just did — a key, a palette row, an answered confirmation.
///
/// # A skew this gesture met OUTRANKS what it thought it did (R324)
///
/// Every dispatcher reports what it ASKED for, and against a daemon too old to perform it the
/// honest answer is the one the transport saw
/// ([`HostClient::take_skew`](sprag_host::HostClient::take_skew)). Taken HERE, in the one place a
/// gesture's report reaches this client's strip, rather than at the four sites that call it — a
/// drain per call site is how two of them come to disagree about whether a refusal is worth
/// painting.
///
/// It is NOT taken by [`show_announcement`], which is the daemon's own message arriving: that is
/// not a gesture, and letting it flush a pending skew would paint the refusal instead of the
/// message somebody sent.
pub(crate) fn show(report: &Report) {
    paint(&preferred(
        crate::terminal::use_terminal().slots.take_skew(),
        report,
    ));
}

/// WHICH of the two a gesture's strip shows: the skew it met, or what the dispatcher thought it
/// did.
///
/// A function rather than two lines inside [`show`] so the RULE can be tested without a host: the
/// wiring above it is one call, and this is the decision. It is the same split
/// [`Announcement::over`](sprag_host::report::Announcement::over) already makes for two messages.
fn preferred(skew: Option<sprag_host::report::Announcement>, report: &Report) -> Report {
    skew.map_or_else(|| report.clone(), |said| Report::said(&said))
}

/// Put a report on the strip — the body [`show`] and [`show_announcement`] share.
///
/// A [`Report`] with nothing to say leaves whatever is up ALONE rather than clearing it: a user who
/// pressed a key that spoke and then typed into a pane is still owed the sentence for its remaining
/// lifetime, and the deadline is what takes it away.
fn paint(report: &Report) {
    let Some(said) = Message::of(
        report,
        now(),
        display_time(&crate::keys::use_client_keys().options()),
    ) else {
        return;
    };
    let held = use_message();
    // `over` and not a plain `set`: a key that says something must not take the strip from a live
    // ALERT somebody sent this client (R317). The rule is `sprag_host::report`'s, shared with the
    // terminal front, so the two cannot rank two messages differently.
    let shown = said.over(held.get(), now());
    let until = shown.until();
    held.set(Some(shown));
    // `None` is a message with NO deadline — an alert, cleared by a keystroke rather than by a
    // clock (see `Severity`). Nothing to wake for, so no thread is spawned: a timer here would be
    // this client waking up to decide that it should still be showing what it is showing.
    if let Some(until) = until {
        wake_at(until);
    }
}

/// Show what somebody ELSE asked this client to say — `sprag display-message`, routed by the daemon
/// and collected on this client's own wake (R317).
///
/// It goes through [`show`] and therefore through [`Report`], which is the whole design: a message a
/// person sent and a refusal this client built for itself are one value by the time either reaches a
/// surface, so this front has no way to paint them differently.
pub(crate) fn show_announcement(announcement: &sprag_host::report::Announcement) {
    paint(&Report::said(announcement));
}

/// Clear a message that waits to be ACKNOWLEDGED — what a keystroke does to an alert.
///
/// A no-op for a message on a deadline and for an empty strip, so the key path calls it
/// unconditionally rather than asking first: an alert is the only thing a keystroke may take away,
/// and a caller that had to check would be a caller that could forget to.
pub(crate) fn acknowledge() {
    let held = use_message();
    if held
        .get()
        .as_ref()
        .is_some_and(Message::waits_to_be_acknowledged)
    {
        held.set(None);
    }
}

/// The sentence to paint right now, or `None` once the deadline has passed.
///
/// The clock is read HERE rather than by a timer that clears the signal, so a client that never
/// repaints again shows a stale line to nobody — [`Message::showing`]'s own rule.
#[must_use]
pub(crate) fn showing() -> Option<String> {
    let said = use_message().get()?;
    let line = said.showing(now())?;
    // MARKED, from `Message::mark` — the shared derivation `sprag-tui`'s row reads too, so the two
    // fronts put the same word in front of the same message and neither writes one.
    Some(
        said.mark()
            .map_or_else(|| line.to_owned(), |mark| format!("{mark}: {line}")),
    )
}

/// The container role the strip is filled with — READ OFF THE SEVERITY, which is the windowed
/// front's half of the same fact the row marks in words.
///
/// `ErrorContainer` for an alert, because an alert is the state a person has to act on and the
/// theme's error role is what every other surface in this client uses to say so; the ordinary
/// surface role otherwise, so a note is a sentence rather than an alarm.
fn strip_role() -> ColorRole {
    match use_message().get().map(|said| said.severity()) {
        Some(sprag_host::report::Severity::Alert) => ColorRole::ErrorContainer,
        _ => ColorRole::SurfaceContainerHigh,
    }
}

/// Ask for a repaint once `until` has passed, so the strip clears on its own deadline.
///
/// A detached thread and not a runtime timer, because pinion has no scheduler seam and this client
/// already wakes the same way from the PTY producer's reader thread. Bounded: one thread per
/// keystroke that SAID something, each living at most `display-time`.
fn wake_at(until: sprag_host::report::Moment) {
    let sink = pinion_core::use_repaint_sink();
    std::thread::spawn(move || {
        std::thread::sleep(until.saturating_sub(now()));
        sink.request_repaint();
    });
}

/// The strip's paint: the sentence, pinned to the bottom of the window — or nothing at all when the
/// client has nothing to say.
///
/// The whole overlay is a full-window container with no fill and `JustifyContent::End`, so it
/// covers the window without painting over it and puts its one child at the bottom. It declares no
/// hit target: a message is read, not clicked, and a strip that swallowed a click would take a
/// pane's last row away from the pointer for three quarters of a second.
#[must_use]
pub(crate) fn view_message(theme: &Theme, window: (u32, u32)) -> Option<Scene> {
    let line = showing()?;
    let strip = Scene::Container(
        ContainerNode::new(vec![Scene::Text(TextNode::styled(
            line,
            Rect::default(),
            TextStyle::new()
                .with_size_px(FONT_PX)
                .with_fg(theme.resolve(ColorRole::OnSurface)),
        ))])
        .with_tag(MESSAGE_STRIP_TAG)
        .with_style(
            // The ERROR container role, because everything this strip can say is a thing that did
            // not happen — see [`sprag_host::report`], where a landing is deliberately silent.
            BoxStyle::filled(theme.resolve(strip_role())).with_corner_radius(6),
        )
        .with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Row)
                .with_padding(Rect::new(MARGIN, MARGIN, MARGIN, MARGIN))
                .with_size(Size::auto().with_height(SizeValue::Px(STRIP_H))),
        ),
    );
    Some(Scene::Container(
        ContainerNode::new(vec![strip]).with_layout(
            LayoutStyle::new()
                .flex(FlexDirection::Column)
                .with_align_items(AlignItems::Center)
                .with_justify(JustifyContent::End)
                .with_padding(Rect::new(MARGIN, MARGIN, MARGIN, MARGIN))
                .with_size(Size::px(window.0, window.1)),
        ),
    ))
}

/// The strip as a screen reader reads it — a live region, so a sentence that appears while the
/// user's focus is in a pane is ANNOUNCED rather than only drawn.
///
/// `Status` and not `Alert`: what this says is the outcome of a key the user just pressed, which is
/// polite by definition — an assertive role would interrupt a screen reader mid-word to say that a
/// pane did not move.
#[must_use]
pub(crate) fn message_access_nodes() -> Vec<AccessNode> {
    showing().map_or_else(Vec::new, |line| {
        vec![
            AccessNode::new(MESSAGE_STRIP_TAG, AriaRole::Status)
                .with_name("what the last key did")
                .with_value(AccessValue::Text(line)),
        ]
    })
}

#[cfg(test)]
mod tests {
    use sprag_host::report::{Announcement, MessageText, Severity};

    /// **A SKEW OUTRANKS WHAT THE GESTURE THOUGHT IT DID, and nothing else changes.**
    ///
    /// The rule R324 put in front of this client's strip, without the host wiring that carries it:
    /// a dispatcher reports what it ASKED for, so against a daemon that could not perform it the
    /// answer a person needs is the transport's. The CONTROL is the second half — with no skew the
    /// report stands, unchanged — because a helper that always preferred one side would satisfy
    /// the first assertion alone.
    #[test]
    fn a_skew_outranks_the_report_and_nothing_else_does() {
        let said = Announcement {
            text: MessageText::parse("this daemon does not perform /x").expect("a line"),
            severity: Severity::Warn,
        };
        let key = Report::nowhere(&sprag_host::keymap::BoundAction::KillPane);
        assert_eq!(
            preferred(Some(said.clone()), &key).says(),
            Some("this daemon does not perform /x"),
            "the transport saw why the act did not happen; the dispatcher only saw what it asked",
        );
        assert_eq!(
            preferred(None, &key).says(),
            key.says(),
            "with no skew the gesture's own report stands, byte for byte",
        );
        assert_eq!(
            preferred(Some(said), &Report::on_screen()).says(),
            Some("this daemon does not perform /x"),
            "including over an arm that had nothing to say — which is most of them",
        );
    }

    use super::*;
    use sprag_host::keymap::{BoundAction, SwitchClientAsk};

    /// A report with nothing to say leaves the client silent, so the strip is absent rather than
    /// empty — the state a blank bar would be indistinguishable from.
    #[test]
    fn what_is_on_screen_raises_no_strip() {
        let owner = Owner::new();
        owner.run(|| {
            show(&Report::on_screen());
            assert_eq!(showing(), None);
            assert!(view_message(&Theme::default(), (960, 600)).is_none());
            assert!(message_access_nodes().is_empty());
        });
    }

    /// **`display-time` reaches THIS front**, which is the only thing that drives the option's
    /// windowed consumer end to end.
    ///
    /// Two readings that must DISAGREE on one fixture: `0` is a message that has already expired —
    /// the option's own documented decision, and the one value that puts back the silence this
    /// surface exists to remove — and a real duration raises the strip. Without the pair, a client
    /// that ignored the user's table entirely would pass the second half alone.
    #[test]
    fn display_time_reaches_this_front_and_zero_puts_the_silence_back() {
        let owner = Owner::new();
        owner.run(|| {
            let action = BoundAction::SwitchClient {
                ask: SwitchClientAsk::Named("ghost".into()),
            };
            let (_config, _keys) =
                crate::keys::test_support::Config::seeded("[options]\ndisplay-time = 0\n");
            show(&Report::no_such(&action));
            assert_eq!(
                showing(),
                None,
                "`display-time 0` is a message that has already expired",
            );
            assert!(view_message(&Theme::default(), (960, 600)).is_none());
        });

        // A SECOND owner, because the keymap slot is resolved once per scope and this is the other
        // reading of the same option rather than a re-read of the first.
        let owner = Owner::new();
        owner.run(|| {
            let action = BoundAction::SwitchClient {
                ask: SwitchClientAsk::Named("ghost".into()),
            };
            let (_config, _keys) =
                crate::keys::test_support::Config::seeded("[options]\ndisplay-time = 60000\n");
            show(&Report::no_such(&action));
            assert_eq!(
                showing().as_deref(),
                Some("no session called \"ghost\""),
                "a real duration raises the strip on the same report",
            );
        });
    }

    /// The measured defect's sentence reaches this front's surface, in the SAME words the terminal
    /// front paints — both read [`Report::says`], and neither writes one.
    #[test]
    fn a_refusal_is_shown_and_announced() {
        let owner = Owner::new();
        owner.run(|| {
            let action = BoundAction::SwitchClient {
                ask: SwitchClientAsk::Named("ghost".into()),
            };
            show(&Report::no_such(&action));
            assert_eq!(showing().as_deref(), Some("no session called \"ghost\""));
            assert!(view_message(&Theme::default(), (960, 600)).is_some());
            let nodes = message_access_nodes();
            assert_eq!(nodes.len(), 1, "one live region, not one per frame");
            assert_eq!(nodes[0].role, AriaRole::Status);
        });
    }
}
