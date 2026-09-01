//! What the HOST hands a client on a WAKE — the closed set, taken in one call.
//!
//! # The defect this module removes
//!
//! Measured at `6884445` by running the shipped `sprag-tui` on a pseudoterminal: with
//! `detach-on-destroy = "next"` and a spare session to land in, `sprag kill-session 0` destroyed
//! the session this client was attached to and the client **stayed exactly where it was** — still
//! `running`, its status row still reading `[0] 0:0*`, naming a session the daemon no longer held,
//! for as long as it was left alone. The same reading under `off` and `previous`. Only the DEFAULT
//! policy worked, and it works for a reason that has nothing to do with the front: a `Detach` is
//! performed by the wire client's own poll thread, which needs no client's help.
//!
//! The four SWITCH policies do need help. A switch joins the poll thread, so the poll thread cannot
//! perform one; it flags the loss and the client's UI thread has to resolve it. `sprag-gui` calls
//! `reconcile_lost_session` from its per-frame reconcile. **`sprag-tui` never called it at all**,
//! so four of the five values of a documented option did nothing on half the product.
//!
//! # Why a type, and not one more line in the terminal front's loop
//!
//! Because that line would have been the third of its kind, and the second is written down four
//! lines above where the missing one belongs. `sprag-tui`'s wake block carries a comment recording
//! that [`chooser::Pick::refresh`](crate::chooser::Pick::refresh) *"had exactly one caller — the
//! GUI's per-frame reconcile — so this front's list was a PHOTOGRAPH while the other's was live"*.
//! One front honouring an obligation the other forgets is not an oversight that happened twice; it
//! is what a per-wake duty list kept only in two `for` loops does.
//!
//! So the duties are a TYPE. [`Woken::take`] is the one door, [`Woken`] is `#[must_use]`, and its
//! fields are public and exhaustively destructured at both fronts — **so a duty added here stops
//! both of them compiling** until each says what it does with it. That is the enforcement; the
//! shipped-binary gates behind it are the proof.
//!
//! # What is a wake duty, and what is not
//!
//! A wake duty is something the HOST is holding that this client must take on the UI thread, on the
//! wake the host itself caused. Two of them:
//!
//! * **What somebody else asked this client to say** ([`Woken::said`], R317) — the message mailbox,
//!   drained on the wake the delivery caused.
//! * **Whether the session under this client is gone** ([`Woken::lost`], R176/R326) — resolved
//!   against the `detach-on-destroy` policy, which only the UI thread can do.
//!
//! [`take_gesture_refusal`](crate::HostClient::take_gesture_refusal) is deliberately NOT one, and
//! its own doc says why: a daemon that performs nothing bumps no channel, so the wake it would be
//! drained on never comes. It belongs to the KEY path that caused it, and both fronts already take
//! it there. A duty list that swept it in would be describing a drain that cannot fire.
//!
//! # Order is part of the contract
//!
//! [`Woken::lost`] is resolved FIRST. A switch replaces this client's whole view — its pane cache,
//! its window list, the session its status row names — so a front that reconciled its own surfaces
//! first would map them onto the dead session's now-absent panes and repair it a frame later. It is
//! also what makes [`Woken::said`] honest: a message copied out to the desktop names the session
//! this client is on ([`crate::outward`]), and the session it was on has just been destroyed.

use crate::report::Announcement;

/// The session this client was attached to was DESTROYED under it, and what became of the client.
///
/// # Why this is a value and not a `bool` that was already acted on
///
/// R316's rule, met at the last host method that returned `()` for an act with an outcome: a client
/// that is silently teleported into somebody else's session has been told nothing, and no repaint
/// can carry the fact, because the thing that would answer the question is the thing that went.
/// `sprag kill-window` has said *"the session went with it"* since R309 and `prefix &` has said it
/// since R325.1 — but only because a KEY produced a [`Report`](crate::report::Report). A destroy
/// that arrives from ANOTHER client, or from the `sprag` CLI, produced no key and therefore no
/// word.
///
/// # `was` is carried, and it is the half that cannot be re-read
///
/// After the move, "where am I" is answerable from the status row. "What happened to the session I
/// was on" is not answerable at all — it is gone from every list — so the name travels in the
/// value or it is lost.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Lost {
    /// The session was destroyed and the `detach-on-destroy` policy moved this client to another
    /// one, which is still serving.
    Moved {
        /// The destroyed session, as this client knew it. Not re-readable: it is gone.
        was: String,
        /// The session this client is attached to now, as the DAEMON named it — never as the
        /// policy guessed it, because a switch can land somewhere other than where it aimed.
        now: String,
    },
    /// The session was destroyed and there was nowhere to go, so this client is LEAVING — a switch
    /// policy that found no survivor falls back to a detach, which is tmux's rule and
    /// `destroy_successor`'s.
    Detached {
        /// The destroyed session, as this client knew it.
        was: String,
    },
}

/// What a host answers a client that has just been WOKEN — the two questions, and nothing else.
///
/// # Why it is its own trait and not two more methods on [`HostClient`](crate::HostClient)
///
/// It was two methods on that trait until R326, and the price was that nothing could drive the
/// door: [`HostClient`](crate::HostClient) has thirty required methods, so a peer written to answer
/// *"which of these did the wake ask for, and in what order"* had to answer twenty-eight questions
/// it has no opinion about. A ROLE this narrow is a role a fixture can play, and this project has
/// paid for that lesson twice already — R321's `StaleHost` and R322's `AgedHost` both refuted a
/// *"this cannot be tested"* note by being a peer the protocol permits.
///
/// [`HostClient`](crate::HostClient) has it as a SUPERTRAIT, so every host still answers both and
/// no caller has to hold two references. Both default to [`None`]: an in-process host — a GUI
/// hosting its own panes, a unit test — has no daemon to be routed from and one default session it
/// can never lose out of band.
///
/// [`take_gesture_refusal`](crate::HostClient::take_gesture_refusal) is deliberately NOT here, and
/// the split is the point: that mailbox is drained by the KEY path that filled it, because a daemon
/// which performs nothing bumps no channel and the wake it would be taken on never comes.
pub trait WakeSource {
    /// Resolve a session this client lost OUT OF BAND — destroyed by ANOTHER client, by the `sprag`
    /// CLI, or by this client's own kill cascading past what it named — against the
    /// `detach-on-destroy` policy: switch to a neighbouring session, or detach when there is no
    /// survivor to move to. Answers WHAT HAPPENED, or [`None`] on the overwhelming majority of
    /// wakes, where nothing was lost.
    ///
    /// # It is a UI-thread duty, and that is why it exists at all
    ///
    /// The destroy is detected on the wire client's background poll thread, which cannot itself
    /// perform the switch: a switch STOPS AND JOINS that very thread. So the poll thread flags the
    /// condition and repaints, and the client's own thread resolves it here — on the wake the flag
    /// was set on. [`Woken::take`] is the one door that calls this, and the reason it is a door
    /// rather than a call at each front is measured in this module's docs.
    ///
    /// The DEFAULT policy never reaches here: a detach needs no client's cooperation, so the poll
    /// thread performs it itself. Everything this answers is one of the four SWITCH policies.
    #[must_use = "a client silently teleported into another session has been told nothing, and no \
                  repaint can carry the fact — the session that would answer it is the one that went"]
    fn resolve_lost_session(&self) -> Option<Lost> {
        None
    }

    /// TAKE whatever somebody asked this client to show a person — `sprag display-message`, routed
    /// by the daemon and collected on the wake this client already has (R317).
    ///
    /// **It TAKES**, which is why it is `&self` returning an owned value rather than a read: a
    /// message is shown once, so the second caller in one frame must get [`None`]. The removal has
    /// already happened at the daemon (`client/messages` collects); this empties the client's own
    /// side of the same hand-off, so a client that reconciles twice between two messages does not
    /// paint the first one twice.
    #[must_use = "TAKING a message empties the client's own side of the hand-off — dropping the \
                  answer loses a person's message silently, and `Option` will not say so"]
    fn take_message(&self) -> Option<Announcement> {
        None
    }
}

/// Everything the host owes a client it has just woken, taken together.
///
/// # Destructure it exhaustively
///
/// Both fronts write `let Woken { lost, said } = ...` with no `..`, and that is the mechanism this
/// type exists for: the next duty added here is a compile error at every front until each one
/// decides what to do with it. A front that reaches past this door for one of these facts has
/// re-created the defect, and the ratchet in this module's tests counts the direct callers.
#[derive(Debug)]
#[must_use = "a wake's duties are what the host is HOLDING for this client — dropping them is the \
              silence this type exists to end"]
pub struct Woken {
    /// This client's session was destroyed under it, resolved against `detach-on-destroy`.
    ///
    /// [`None`] on almost every wake: a session is destroyed once. Resolved BEFORE
    /// [`said`](Self::said) — see the module docs for why the order is part of the contract.
    pub lost: Option<Lost>,
    /// What somebody else asked this client to show a person (R317) — `sprag display-message`,
    /// routed by the daemon and collected on the wake the delivery caused.
    ///
    /// TAKEN, not read: a message is shown once, so a client that wakes twice between two messages
    /// paints the first one once.
    pub said: Option<Announcement>,
}

impl Woken {
    /// Take every duty this wake carries, in the contract's order.
    ///
    /// The ONE door. `?Sized` so both fronts reach it identically — one holds a
    /// `Box<dyn HostClient>` and one holds it behind a view, and `dyn HostClient` satisfies
    /// [`WakeSource`] through the supertrait without either of them naming a second type.
    pub fn take(source: &(impl WakeSource + ?Sized)) -> Self {
        // The loss FIRST, and bound in a `let` rather than written as the first field so the order
        // is not a fact about struct-literal evaluation. A switch replaces the pane cache, the
        // window list, and the session name every other fact on this wake is read against.
        let lost = source.resolve_lost_session();
        Self {
            lost,
            said: source.take_message(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::{MessageText, Severity};
    use std::cell::RefCell;

    /// A peer that plays ONLY the wake's role, and records the order it was asked in.
    ///
    /// Three fields and two methods, which is the whole argument for [`WakeSource`] being its own
    /// trait: as two methods on `HostClient` this fixture would have had to answer thirty questions
    /// about panes and layouts to be allowed to answer the two a wake asks.
    #[derive(Default)]
    struct Peer {
        asked: RefCell<Vec<&'static str>>,
    }

    impl WakeSource for Peer {
        fn resolve_lost_session(&self) -> Option<Lost> {
            self.asked.borrow_mut().push("lost");
            Some(Lost::Moved {
                was: "0".to_owned(),
                now: "beta".to_owned(),
            })
        }

        fn take_message(&self) -> Option<Announcement> {
            self.asked.borrow_mut().push("said");
            Some(Announcement {
                text: MessageText::parse("build finished").expect("a legal row"),
                severity: Severity::Note,
            })
        }
    }

    /// A host that plays the role and answers NOTHING — the in-process arm's shape, and the
    /// control for the two below: without it, a `Woken::take` that answered `None` unconditionally
    /// would look exactly like a correct one to a test that only ever asks a peer with news.
    #[derive(Default)]
    struct Quiet;

    impl WakeSource for Quiet {}

    /// The loss is resolved BEFORE the message is taken, because a switch replaces the session a
    /// message is read (and copied out) against. Asserted on the ORDER, which is the only thing
    /// that can go wrong here and the only thing a caller could silently reverse.
    #[test]
    fn the_loss_is_resolved_before_the_message_is_taken() {
        let peer = Peer::default();
        let woken = Woken::take(&peer);
        assert_eq!(
            peer.asked.into_inner(),
            vec!["lost", "said"],
            "a message taken before the switch names the session the person just left",
        );
        assert!(woken.lost.is_some() && woken.said.is_some());
    }

    /// Both duties come back from ONE call, which is the whole of what makes a front unable to
    /// honour one and forget the other.
    #[test]
    fn one_call_carries_every_duty() {
        let Woken { lost, said } = Woken::take(&Peer::default());
        assert_eq!(
            lost,
            Some(Lost::Moved {
                was: "0".to_owned(),
                now: "beta".to_owned(),
            }),
        );
        assert_eq!(
            said.map(|said| said.text.as_str().to_owned()),
            Some("build finished".to_owned()),
        );
    }

    /// A host with no news answers none, and asks nothing of the caller — the state every wake but
    /// a handful is in. The CONTROL for the two above.
    #[test]
    fn a_wake_with_nothing_to_hand_over_says_so() {
        let Woken { lost, said } = Woken::take(&Quiet);
        assert_eq!(lost, None);
        assert!(said.is_none());
    }

    /// Every `.rs` under a crate's `src`, so a ratchet over a frontend cannot be dodged by putting
    /// the call in a new file. Walked rather than [`include_str!`]-ed for exactly that reason.
    fn source_of(crate_name: &str) -> Vec<(std::path::PathBuf, String)> {
        fn walk(dir: &std::path::Path, into: &mut Vec<(std::path::PathBuf, String)>) {
            for entry in std::fs::read_dir(dir).expect("a frontend's source tree") {
                let path = entry.expect("a directory entry").path();
                if path.is_dir() {
                    walk(&path, into);
                } else if path.extension().is_some_and(|kind| kind == "rs") {
                    let text = std::fs::read_to_string(&path).expect("a source file");
                    into.push((path, text));
                }
            }
        }
        // ⚠ THROUGH `sprag_gate`'s ONE DOOR — register item 809. This used to walk up from
        // `env!("CARGO_MANIFEST_DIR")`, which is the tree this test was COMPILED in and stopped
        // being the tree it runs in; `workspace_root` compares the two and refuses a skew rather
        // than scanning somebody else's sources and calling the result a fact about this one.
        let root = sprag_gate::sources::workspace_root()
            .join("crates")
            .join(crate_name)
            .join("src");
        let mut files = Vec::new();
        walk(&root, &mut files);
        assert!(
            files.len() > 5,
            "the walk found {} files under {root:?} — a ratchet over nothing passes",
            files.len(),
        );
        files
    }

    /// **THE RATCHET: neither frontend reaches past the door for a wake duty.**
    ///
    /// This is the enforcement the module docs claim, made checkable. `Woken` being `#[must_use]`
    /// and exhaustively destructured stops a front DROPPING a duty it took; nothing in the type
    /// system stops a front taking one duty straight off the host and never learning that the other
    /// exists — which is precisely what `sprag-tui` did with `resolve_lost_session` for as long as
    /// that method had a caller anywhere.
    ///
    /// Both frontends are walked, because the defect was a difference BETWEEN them and a ratchet
    /// over the one that was wrong would have passed at every moment except the one it was written
    /// in.
    ///
    /// The needles are method names spelled here and searched THERE, so this assertion cannot be
    /// what it counts — `wake.rs` is not one of the files walked.
    #[test]
    fn neither_frontend_reaches_past_the_door_for_a_wake_duty() {
        let duties = [".resolve_lost_session()", ".take_message()"];
        for front in ["sprag-gui", "sprag-tui"] {
            let files = source_of(front);
            for (path, text) in &files {
                for duty in duties {
                    assert!(
                        !text.contains(duty),
                        "{path:?} calls `{duty}` directly. A wake duty taken outside \
                         `Woken::take` is a duty the OTHER frontend never learns exists — which is \
                         how `detach-on-destroy`'s four switch policies did nothing on `sprag-tui` \
                         for as long as they existed.",
                    );
                }
            }
            // ...and it goes through the door, destructured EXHAUSTIVELY. Without this half the
            // assertion above is satisfied by a frontend that takes NO duties at all.
            //
            // The PATTERN is read, not a spelling of it: a front may bind a field to a local of
            // another name (`said: announcement`) and that is still exhaustive. What may not appear
            // is `..`, which is the one way to write a pattern that keeps compiling when a duty is
            // added — the whole mechanism, dodged in two characters.
            // A BINDING, not every `Woken {` in the file: `fn woken(&self) -> ...Woken {` opens a
            // function body with the same three characters, and counting it read the GUI's
            // passthrough as a second door. The binding must start with `let`.
            //
            // Read from the WHOLE text and not line by line, because rustfmt decides where the
            // pattern breaks: the GUI's fits on one line and the terminal front's — one field bound
            // to a differently named local — does not. A ratchet that only understood the short
            // form would go green on the front it was written for and red on the other, which is
            // the exact asymmetry it exists to catch.
            let patterns: Vec<&str> = files
                .iter()
                .flat_map(|(_, text)| text.match_indices("Woken {").map(move |(at, _)| (text, at)))
                .filter(|(text, at)| {
                    text[..*at]
                        .rsplit_once('\n')
                        .is_some_and(|(_, line)| line.trim_start().starts_with("let "))
                })
                .filter_map(|(text, at)| {
                    let rest = &text[at + "Woken {".len()..];
                    rest.split_once('}').map(|(pattern, _)| pattern)
                })
                .collect();
            assert_eq!(
                patterns.len(),
                1,
                "{front} destructures `Woken` in {} places; it must be exactly one",
                patterns.len(),
            );
            let pattern = patterns[0];
            for field in ["lost", "said"] {
                assert!(
                    pattern.contains(field),
                    "{front}'s `Woken` pattern does not name `{field}`: {pattern:?}",
                );
            }
            assert!(
                !pattern.contains(".."),
                "{front}'s `Woken` pattern uses `..`, which is the one way to keep compiling when \
                 a duty is added — and adding a duty is the moment this ratchet exists for: \
                 {pattern:?}",
            );
        }
    }
}
