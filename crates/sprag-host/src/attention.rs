//! A pane's child asking for a PERSON, routed to the people who are looking at it.
//!
//! # The defect this module removes
//!
//! Measured at `3114923` by running the shipped binaries rather than by reading them. A child in an
//! unfocused pane raised `OSC 9 build finished: 3 errors`, with a live `sprag-tui` attached to that
//! session on a real pseudoterminal:
//!
//! * the DAEMON latched it — the `panes` slot answered
//!   `{"title":null,"body":"build finished: 3 errors","seq":1}`, so the words crossed the whole
//!   emulator-to-wire path intact;
//! * the terminal front's screen was **byte-for-byte unchanged**, all twenty-four rows, while
//!   `sprag display-message` on the same client at the same instant painted its row — the control
//!   that says this is about the notification and not about a dead fixture;
//! * `sprag-gui` showed a DOT on the pane title ([`crate::PaneNotification`]'s `seq` against an
//!   acknowledged one) and dropped the words.
//!
//! So the one thing the child actually said reached nobody at one front and half of nobody at the
//! other. Every layer was already carrying it; what was missing is that **nothing was OBLIGED to
//! read it**. A latch is a fact available on request, and a request is what a person who is not
//! looking at that pane never makes.
//!
//! # What replaces it
//!
//! The pane's own reader thread reports an [`Attention`] the moment the batch carrying it is
//! applied, and this module turns that into the value R317 already routes: an
//! [`Announcement`] addressed to
//! [`Audience::Session`](crate::Audience) — the people attached to the session the pane is in. From
//! there it is the same path a `sprag display-message` takes, which is the point: a client cannot
//! come to show a pane's words differently from a person's, because by the time either reaches a
//! surface they are one type.
//!
//! # Why a THREAD and not the hook itself
//!
//! Because the hook runs on the PTY reader thread and `PanePty::Drop` JOINS that thread. A hook that
//! took a workspace lock would deadlock the moment a pane-drop site held one — the reader would wait
//! on the lock while the dropper waited on the join. The daemon already solves this exact shape once,
//! for the reaper ([`crate::spawn_reaper`]): the hook only SENDS on a channel, and a dedicated thread
//! does the work that needs locks. This is that pattern, and it needs it MORE than the reaper does,
//! because naming the pane means walking the registry into a workspace.
//!
//! # What the person is told, and where the words come from
//!
//! `pane 3: build finished: 3 errors`, where the address is [`PaneAddress`]'s own `Display` — a bare
//! number for a pane with no name and a quoted name for one that has one. That is deliberate and it
//! is the round's smallest good decision: **the sentence spells the pane the way a person types it
//! back**, so reading the message tells them what to pass to `select-pane -t`. A sentence that said
//! "window 2" would be a second vocabulary for addressing a pane, which R312 spent a whole round
//! removing.
//!
//! # The rival
//!
//! herdr (`9a4ce5e1`) is AHEAD on having an API-callable notification with OS-native and terminal
//! delivery, and R317's entry says so. On THIS axis they have nothing at all: their `notification`
//! surface is reachable only from their own API (`handle_notification_show`), their libghostty
//! binding declares a bell callback (`GhosttyTerminalBellFn`) that nothing in the tree installs, and
//! no path anywhere reads a pane child's `OSC 9`. So a build script that finishes inside one of their
//! panes tells nobody, by construction rather than by oversight.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};

use sprag_terminal::{Attention, PaneId, SessionRegistry};
use sprag_vt::Urgency;

use crate::notify::ChannelRegistry;
use crate::pane_address::PaneAddress;
use crate::report::{Announcement, MessageText, MessageTextError, Severity};
use crate::{AttachmentRegistry, Audience};

/// The middle of the sentence a refused notification becomes — spelled once, because its LENGTH is
/// load-bearing (see the assertion below) and a format string counted by eye is a number nothing
/// checks.
const CANNOT_SHOW: &str = " raised a notification that cannot be shown: ";

/// **The refusal sentence fits a row, PROVED BY THE COMPILER at the widest input it can have.**
///
/// It did not, and that is why this is here: the first version embedded
/// [`MessageTextError`]'s `Display` — a paragraph explaining why a newline is refused — producing a
/// 216-byte refusal about a 200-byte limit, under an `expect` claiming the case could not arise.
///
/// Every term is a declared bound rather than a guess: `pane ` and [`CANNOT_SHOW`] are this module's
/// own text, a [`PaneName`](sprag_terminal::PaneName) caps a quoted name at its `MAX_BYTES` plus two
/// quotes (the widest address — a `u64` id is at most 20 digits), and
/// [`LONGEST_RULE`](MessageTextError::LONGEST_RULE) caps the reason. A test checks the same thing by
/// BUILDING the sentence; this checks it before the crate compiles, so a wider rule name or a longer
/// pane name cannot ship.
const _: () = assert!(
    "pane ".len()
        + 2
        + sprag_terminal::PaneName::MAX_BYTES
        + CANNOT_SHOW.len()
        + MessageTextError::LONGEST_RULE
        <= MessageText::MAX_BYTES,
    "the sentence a refused notification becomes must fit a row at its widest",
);

/// One pane's raised attention, on its way to the router thread.
///
/// **No session, and that is a measured decision rather than a simplification.** The first version
/// baked the session into the hook, the way [`crate::bump_on_dirty`] bakes its revision token — and
/// the GUI's live smoke found it wrong: `new_session` births its first pane through a surface whose
/// SCOPE is the requesting connection's default session, not the one being created, which is the
/// hazard `bump_on_dirty`'s own doc names two lines further down. So a pane in a brand-new session
/// asked for a person and the people looking at that session were never told.
///
/// The router therefore asks the REGISTRY who holds this pane, which is the only authority on it,
/// and gets the address out of the same walk. That makes the wrong answer unrepresentable rather
/// than merely fixed at one birth site.
struct Raised {
    pane: PaneId,
    attention: Attention,
}

/// The daemon's attention router: a live channel to the thread that turns a pane's raised
/// [`Attention`] into a routed message.
///
/// Held by [`HostState`](crate::HostState) so every pane the daemon spawns can be wired to it. The
/// thread ends when this is dropped (the sender disconnects), which is the same self-terminating
/// shape the reaper uses — nothing has to remember to stop it.
pub struct AttentionRouter {
    tx: Sender<Raised>,
}

impl AttentionRouter {
    /// The `on_attention` signal every pane is wired with — SHARED, exactly as
    /// [`crate::spawn_reaper`]'s death signal is, so every surface that spawns a pane wires the same
    /// one and no pane category can be quietly left out.
    ///
    /// **It names no session**, and that is a measured decision. The first version baked one in, the
    /// way [`crate::bump_on_dirty`] bakes its revision token — and the GUI's live smoke found it
    /// wrong: `new_session` births its first pane through a surface whose SCOPE is the requesting
    /// connection's default session, not the one being created. So a pane in a brand-new session
    /// asked for a person and the people looking at that session were never told. The router asks
    /// the REGISTRY who holds the pane instead, which is the only authority on it.
    ///
    /// Registry-FREE by construction: it captures a [`Sender`] and does nothing but send. That is
    /// what makes it safe to run on the reader thread, and it is a property of this function rather
    /// than a rule a caller has to keep — see the module docs for the deadlock it avoids.
    ///
    /// A send that fails means the router thread is gone (the daemon is shutting down), and it is
    /// dropped rather than logged: a message nobody could have been shown, during teardown, is not
    /// news.
    #[must_use]
    pub fn signal(&self) -> Arc<dyn Fn(PaneId, Attention) + Send + Sync> {
        let tx = Mutex::new(self.tx.clone());
        Arc::new(move |pane, attention| {
            // `Sender` is `Send` but not `Sync`, and this signal is shared across the surfaces that
            // spawn panes — so the clone lives behind the same mutex any shared handle would need.
            // It is taken for the length of one non-blocking send on an unbounded channel, which is
            // what keeps it off the reader thread's critical path.
            let tx = tx.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = tx.send(Raised { pane, attention });
        })
    }
}

/// Start the router thread and answer the handle that feeds it.
///
/// `registry` is read to find WHO HOLDS the pane and what to call it (one walk answering both),
/// `attachments` to queue the message, and `channels` to wake the clients it reached — the same
/// three steps, in the same order, that
/// [`WorkspaceExternal::display_message`](crate::WorkspaceExternal) performs for a person's message,
/// because it is the same delivery.
#[must_use]
pub fn spawn_attention_router(
    registry: Arc<Mutex<SessionRegistry>>,
    attachments: Arc<Mutex<AttachmentRegistry>>,
    channels: Arc<ChannelRegistry>,
) -> AttentionRouter {
    let (tx, rx) = channel::<Raised>();
    std::thread::Builder::new()
        .name("sprag-attention".to_string())
        .spawn(move || route_until_closed(&rx, &registry, &attachments, &channels))
        .expect("spawn the attention router thread");
    AttentionRouter { tx }
}

/// The router thread's loop: one raised attention at a time until the last sender is dropped.
fn route_until_closed(
    rx: &Receiver<Raised>,
    registry: &Mutex<SessionRegistry>,
    attachments: &Mutex<AttachmentRegistry>,
    channels: &ChannelRegistry,
) {
    while let Ok(raised) = rx.recv() {
        let Some((session, announcement)) = announce(&raised, registry) else {
            continue;
        };
        // The attachment lock is taken, the delivery is read out, and the lock is DROPPED before any
        // channel is woken — `display_message`'s order, for its reason: announcing under the
        // attachment lock would take the change-channel lock inside it, an order nothing else in
        // this daemon uses.
        let session_named = session.clone();
        let delivery = {
            let mut attachments = attachments.lock().unwrap_or_else(PoisonError::into_inner);
            attachments.deliver(&Audience::Session(session), &announcement)
        };
        // The wake is derived from the DELIVERY rather than from the pane's session, which is the
        // same rule the person's message follows even though the two sets are equal here: a set
        // computed twice is a set that can come to differ, and the failure mode is a client parked
        // forever on a channel that never moved.
        let woke = delivery.sessions();
        // **A delivery that reached NOBODY is an answer, and this is the one caller that can only
        // log it.** R317's `Delivery` exists because "shown to nobody" is a thing an agent must act
        // on; here the sender is a child in a pane and there is nobody to answer to — so the daemon
        // records it rather than treating an empty set as a successful nothing. It is the state a
        // person hits when they detach and their build finishes, and it is exactly what an operator
        // reading the log needs to see instead of silence.
        if woke.is_empty() {
            tracing::info!(
                target: "sprag_host::attention",
                pane = raised.pane.0,
                session = %session_named,
                "a pane asked for a person and no client is attached to see it",
            );
        }
        for session in woke {
            channels.bump(session);
        }
    }
}

/// WHO to tell and what to say about `raised`, or [`None`] when there is nobody to tell or the user
/// has asked not to hear it.
///
/// Split out from the loop so the whole sentence — the option gate, the audience, the address, the
/// words and the severity — is testable without a thread, a daemon or a pane.
///
/// A pane the registry does not hold answers [`None`], and that is the honest end of the story
/// rather than a fallback: the pane was CLOSED between raising the attention and this thread
/// reading, so there is no session to address — and guessing one (the connection that spawned it,
/// say) is exactly the wrong answer this function's own history is about. It is logged at `warn`,
/// because a message that reached nobody is the outcome this whole path exists to make visible.
fn announce(raised: &Raised, registry: &Mutex<SessionRegistry>) -> Option<(String, Announcement)> {
    if !monitored(&raised.attention) {
        return None;
    }
    let Some((session, address)) = holder_of(raised.pane, registry) else {
        tracing::warn!(
            target: "sprag_host::attention",
            pane = raised.pane.0,
            "a pane asked for a person and was closed before anyone could be told",
        );
        return None;
    };
    Some((
        session,
        Announcement {
            text: words(&address, &raised.attention),
            severity: severity_of(&raised.attention),
        },
    ))
}

/// Whether the user wants to be told about this kind of attention at all.
///
/// Read from the config file per raised attention, which is where this reader's cost decision sits
/// ([`crate::config::agent_settle`] states the rule), and the price is bounded by the SOURCE rather
/// than by hope: the emulator LATCHES one notification and the reader fires once per output BATCH
/// (`take_attention`), so a child printing a thousand escapes into one 8 KiB read produces ONE read
/// of this file — not a thousand. That is what makes "per attention" a rare event even for a
/// runaway child, and it is why the gate can live here rather than being cached against a clock.
/// `set-option` therefore takes effect with nothing to restart.
///
/// A config this daemon cannot read leaves the DEFAULT standing (both are `on`), which is the rule
/// every other reader in [`crate::config`] follows: a user with a typo in their file still gets
/// told their build finished.
fn monitored(attention: &Attention) -> bool {
    let name = match attention {
        Attention::Raised(_) => crate::options::MONITOR_NOTIFICATION,
        Attention::Bell => crate::options::MONITOR_BELL,
    };
    crate::config::option_is_on(name)
}

/// Which session holds `pane`, and how a person is told to reach it — its NAME if it has one, else
/// its id, in [`PaneAddress`]'s own spelling.
///
/// **ONE walk answering both**, because they are one question: the audience is whoever is looking at
/// the session that holds this pane, and the address is what that pane is called there. Asked
/// separately they could disagree — a pane closed between the two reads would be addressed in a
/// session that no longer holds it.
///
/// The registry lock is RELEASED before any workspace lock is taken — never nested, the discipline
/// `sweep_once` measured the cost of and states. [`None`] means no session holds the pane.
fn holder_of(pane: PaneId, registry: &Mutex<SessionRegistry>) -> Option<(String, PaneAddress)> {
    let pools: Vec<(String, Arc<Mutex<sprag_terminal::Workspace>>)> = {
        let reg = registry.lock().unwrap_or_else(PoisonError::into_inner);
        reg.sessions()
            .iter()
            .flat_map(|session| {
                let name = session.name().to_owned();
                session
                    .windows()
                    .iter()
                    .map(move |window| (name.clone(), Arc::clone(window.workspace())))
            })
            .collect()
    };
    for (session, pool) in pools {
        let pool = pool.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(found) = pool.panes().iter().find(|held| held.id() == pane) {
            let address = found.name().map_or(PaneAddress::Number(pane.0), |name| {
                PaneAddress::Name(name.as_str().to_owned())
            });
            return Some((session, address));
        }
    }
    None
}

/// The sentence a person reads, already checked against every rule a terminal row imposes.
///
/// # The child's words are the part that can break a rule
///
/// A notification body is arbitrary bytes chosen by whatever is running in the pane, on their way to
/// being written into somebody's terminal — exactly [`MessageText`]'s subject. When they break a
/// rule the message is NOT sanitised and NOT dropped: the person is told that the pane raised
/// something unshowable and which rule it broke, so they can go and look. A silent drop would leave
/// a build failure invisible for a reason nobody can see, and a sanitised one is the rival's trade
/// (`sanitized_notification_text` truncates to 80 bytes and answers `shown`).
///
/// # The fallback's own length is PROVED, not assumed
///
/// It was assumed, and it was wrong: the first version embedded
/// [`MessageTextError`]'s `Display` — a paragraph explaining why a
/// newline is refused — which produced a 216-byte refusal about a 200-byte limit, under an `expect`
/// claiming the case could not arise. A test found it. The sentence now carries
/// [`MessageTextError::rule`], the SHORT wording, and every term in it is
/// bounded: `PaneName` caps a name at 80 bytes, a `u64` id at 20 digits, and
/// `LONGEST_RULE` caps the reason — so the expect below is unreachable by arithmetic, which
/// `the_refusal_sentence_fits_a_row_for_the_longest_possible_pane_name` checks at the widest input
/// rather than at a convenient one.
fn words(address: &PaneAddress, attention: &Attention) -> MessageText {
    let said = match attention {
        // Title and body joined the way a desktop notification reads, skipping whichever the source
        // did not carry: `OSC 9` has no title, and a kitty title-only chunk has no body.
        Attention::Raised(notification) => [
            notification.title.as_deref().unwrap_or_default(),
            notification.body.as_str(),
        ]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join(": "),
        // A bell carries nothing, so the word is this module's. tmux says "Bell in window 2"; the
        // subject here is the PANE, because that is what sprag can address.
        Attention::Bell => "bell".to_owned(),
    };
    MessageText::parse(&format!("pane {address}: {said}")).unwrap_or_else(|broken| {
        MessageText::parse(&format!("pane {address}{CANNOT_SHOW}{}", broken.rule(),))
            .expect("this module's own sentence breaks no rule a terminal row imposes")
    })
}

/// How much a raised attention matters — the projection of the CHILD's claim onto the severity the
/// surfaces rank messages by.
///
/// # Why [`Urgency::Normal`] is a note and not a warning
///
/// The two scales are both ordered and both have three arms, so an order-preserving map onto
/// [`Severity`] looks obvious — and it would put a false claim on the row. [`Severity::Warn`] means
/// *something did not work*, and it is what this client's OWN refusals are; a child raising an
/// ordinary notification has said its words matter normally, not that anything failed. So the two
/// lower urgencies both land on [`Severity::Note`], and the information is not lost — it is carried
/// faithfully by [`Urgency`] at the source and collapsed HERE, once, where a reader can disagree
/// with it.
///
/// # [`Urgency::Critical`] is the whole point
///
/// It becomes [`Severity::Alert`], which has NO deadline: the row holds until a person touches a
/// key. A child that says *a person is needed* is answered by a surface that waits for one, which is
/// a property neither tmux (no severity at all) nor herdr (a hardcoded three-second toast for every
/// API notification) has. A BELL claims nothing, so it is a note.
fn severity_of(attention: &Attention) -> Severity {
    match attention {
        Attention::Raised(notification) => match notification.urgency {
            Urgency::Low | Urgency::Normal => Severity::Note,
            Urgency::Critical => Severity::Alert,
        },
        Attention::Bell => Severity::Note,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_vt::Notification;

    /// A notification at `urgency` with `title` / `body`, for the fixtures below.
    fn raised(title: Option<&str>, body: &str, urgency: Urgency) -> Attention {
        Attention::Raised(Notification {
            title: title.map(str::to_owned),
            body: body.to_owned(),
            urgency,
        })
    }

    /// **The sentence spells the pane the way a person types it back** — a bare number for an
    /// unnamed pane, a quoted name for a named one, which is [`PaneAddress`]'s own `Display` and not
    /// a second wording invented here.
    #[test]
    fn the_message_names_the_pane_in_the_spelling_that_reaches_it() {
        assert_eq!(
            words(
                &PaneAddress::Number(3),
                &raised(None, "build finished: 3 errors", Urgency::Normal),
            )
            .as_str(),
            "pane 3: build finished: 3 errors",
        );
        assert_eq!(
            words(
                &PaneAddress::Name("buildout".to_owned()),
                &raised(None, "done", Urgency::Normal),
            )
            .as_str(),
            "pane \"buildout\": done",
        );
    }

    /// A TITLE and a BODY read as one sentence, and whichever the source did not carry is not
    /// spelled as an empty half — `OSC 9` has no title and a kitty title-only chunk has no body, so
    /// both absences are real inputs rather than defensive cases.
    #[test]
    fn a_title_and_a_body_join_and_an_absent_half_leaves_no_gap() {
        assert_eq!(
            words(
                &PaneAddress::Number(1),
                &raised(Some("Build"), "done in 3s", Urgency::Normal)
            )
            .as_str(),
            "pane 1: Build: done in 3s",
        );
        assert_eq!(
            words(
                &PaneAddress::Number(1),
                &raised(Some("Build"), "", Urgency::Normal)
            )
            .as_str(),
            "pane 1: Build",
            "a title-only chunk must not read `Build: `",
        );
        assert_eq!(
            words(
                &PaneAddress::Number(1),
                &raised(None, "done", Urgency::Normal)
            )
            .as_str(),
            "pane 1: done",
        );
    }

    /// A bell has no words, so the word is this module's — and it names the PANE, which is what
    /// sprag can address, where tmux names the window.
    #[test]
    fn a_bell_says_which_pane_rang() {
        assert_eq!(
            words(&PaneAddress::Number(7), &Attention::Bell).as_str(),
            "pane 7: bell",
        );
    }

    /// **A child's hostile words are REFUSED and the person is told why** — not sanitised, and not
    /// dropped. The bytes go into somebody's terminal, so a newline would forge a second row and an
    /// escape would be obeyed; a caller that cannot be told (a child) still leaves a person who can.
    #[test]
    fn words_a_terminal_row_refuses_become_a_sentence_about_the_refusal() {
        let hostile = words(
            &PaneAddress::Number(2),
            &raised(None, "clear: \u{1b}[2J and gone", Urgency::Normal),
        );
        assert!(
            !hostile.as_str().contains('\u{1b}'),
            "the escape must not reach a row: {hostile}",
        );
        assert!(
            hostile
                .as_str()
                .starts_with("pane 2 raised a notification that cannot be shown"),
            "the person is told something happened and where: {hostile}",
        );
        assert!(
            hostile.as_str().contains("control characters"),
            "and which rule it broke: {hostile}",
        );
    }

    /// **The refusal sentence fits a row at the WIDEST input it can be built from** — the longest
    /// legal pane name, the longest rule wording. This is the assertion the `expect` in [`words`]
    /// rests on, and it is here because the first version of that sentence did NOT fit: it embedded
    /// the operator-facing paragraph and came to 216 bytes.
    #[test]
    fn the_refusal_sentence_fits_a_row_for_the_longest_possible_pane_name() {
        let longest = "n".repeat(sprag_terminal::PaneName::MAX_BYTES);
        let name = sprag_terminal::PaneName::parse(&longest).expect("the longest legal pane name");
        let said = words(
            &PaneAddress::Name(name.as_str().to_owned()),
            &raised(None, "two\nrows", Urgency::Normal),
        );
        assert!(
            said.as_str().len() <= MessageText::MAX_BYTES,
            "the refusal is {} bytes at the widest input",
            said.as_str().len(),
        );
        assert!(said.as_str().contains("control characters"));
    }

    /// A body longer than a row is refused the same way, which is the case that proves the fallback
    /// is not itself refusable: it is built from this module's words and an address, both bounded.
    #[test]
    fn an_over_long_notification_still_reaches_the_person_as_a_refusal() {
        let long = "x".repeat(MessageText::MAX_BYTES + 1);
        let said = words(
            &PaneAddress::Number(9),
            &raised(None, &long, Urgency::Normal),
        );
        assert!(said.as_str().starts_with("pane 9 raised a notification"));
        assert!(said.as_str().len() <= MessageText::MAX_BYTES);
    }

    /// **A pane the registry does not hold has nobody to tell, and says so rather than guessing.**
    ///
    /// The branch is reachable only from a state no live fixture builds reliably — a pane closed in
    /// the microseconds between its child raising an attention and the router thread walking the
    /// registry — which is the third shape the debt sweep hunts. Driven directly instead of left as
    /// an untested arm, because the WRONG answer here is a delivery to a guessed session.
    #[test]
    fn a_pane_that_is_gone_is_addressed_to_nobody_rather_than_to_a_guess() {
        let registry = Mutex::new(SessionRegistry::new((80, 24)));
        assert_eq!(holder_of(PaneId(7), &registry), None);
        assert!(
            announce(
                &Raised {
                    pane: PaneId(7),
                    attention: raised(None, "the build finished", Urgency::Normal),
                },
                &registry,
            )
            .is_none(),
            "there is no session to address, so there is no announcement",
        );
    }

    /// **The child's claim decides how long the row holds** — and only `critical` asks for a person.
    /// Checked over the whole closed set, so a fourth urgency cannot be added without deciding what
    /// it means here.
    #[test]
    fn only_a_critical_child_takes_the_row_until_somebody_touches_a_key() {
        for urgency in Urgency::ALL {
            let want = match urgency {
                Urgency::Critical => Severity::Alert,
                Urgency::Low | Urgency::Normal => Severity::Note,
            };
            assert_eq!(severity_of(&raised(None, "words", urgency)), want);
        }
        assert_eq!(severity_of(&Attention::Bell), Severity::Note);
        // The consequence, spelled out where it can fail: an alert has no deadline, so the row it
        // takes is not on a clock. This is the property the whole urgency path exists to deliver.
        assert_eq!(
            Severity::Alert.deadline(crate::report::now(), std::time::Duration::from_millis(750)),
            None,
        );
    }
}
