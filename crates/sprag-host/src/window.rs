//! How big a session's window is when more than one client is looking at it — tmux's
//! `window-size`, and the arbitration behind it.
//!
//! # Why a session needs a window at all
//!
//! A pane's `(cols, rows)` is a fact about the SESSION, not about whoever is watching: the program
//! inside it wrapped its lines at some width, and there is only one program. So when two clients of
//! different sizes view one session, something has to choose, and every client has to be told the
//! same answer. Before this module the choice was made by accident — each client laid the
//! arrangement out over its own screen and resized the panes to match, so the pane took whichever
//! size was written last, and the other client went on painting a grid whose real dimensions it had
//! never been told (a resize does not bump the scene). That is not a policy, it is a race with a
//! silent loser.
//!
//! [`arbitrate`] is the policy, stated once and applied to every client's reported area
//! ([`ClientSize`]). What comes out is the window every client tiles over
//! ([`sprag_terminal::tile`]), which is what makes them agree.
//!
//! # The option does not cross the wire; the ANSWER does
//!
//! `window-size` is read from the user's `config.toml` by the daemon itself, exactly as
//! `default-command` is (R240) — no option travels the socket, so `sprag show-options` still
//! answers with no daemon running. What the daemon publishes is the arbitrated `(cols, rows)`, a
//! derived fact rather than a setting, and a client that reads it needs to know nothing about which
//! policy produced it.
//!
//! # What arbitrates, and who applies the answer
//!
//! The arbitration is over the clients that REPORT an area
//! ([`crate::wire::CLIENT_SIZE_METHOD`]), and that is BOTH frontends. `sprag-tui` reports its
//! terminal. `sprag-gui` cannot report its window — its chrome is PER PANE (every dock panel
//! carries its own header, above a tab strip and beside a session sidebar), so the cells available
//! to the ARRANGEMENT depend on the shape of the tiling rather than on the size of the window — so
//! it reports what its panes MEASURED, folded back into one window by
//! [`sprag_terminal::fit_window`].
//!
//! What comes out is applied by `retile`, HERE, once per session. Both inputs to a pane's size
//! are this daemon's — the tree it owns and the window it just arbitrated — so the size is derived
//! where they live rather than in each client. A client writes a pane's size only when this daemon
//! has no window to derive one from, which is the honest fallback and what both did before any of
//! this existed.
//!
//! # The one policy that arbitrates over nobody
//!
//! [`WindowSize::Manual`] does not read the clients at all: its window is a size an operator PINNED
//! on the window itself (`sprag resize-window`, stored beside the arrangement it sizes and restored
//! with it). So the sentences above hold for three of the four values, and the fourth is why this
//! module takes a `pinned` argument as well as a list of reports.
//!
//! It exists because a derived window has no memory. Measured: a session whose panes a user had
//! arranged at 100x30 was reflowed by the first client to attach — to that client's size, and it
//! stayed there after the client left, because the honest answer with nobody attached is "leave the
//! panes alone". So the wrap width of a long-running program was decided by whoever last glanced at
//! it, permanently, and no value of this option could hold it. `manual` is the value that can.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sprag_terminal::{Rect, SessionRegistry, tile};

use crate::attach::{AttachmentRegistry, ClientSize};
use crate::lock;
use crate::scope::SessionScope;

sprag_terminal::closed_set! {
    // `ALL` is GENERATED with this enum from ONE variant list, so it cannot be missing a
    // variant and its length cannot disagree with its contents — see `closed_set!`. The
    // hand-written array it replaces was checked by nothing, which three register items
    // said and none closed (R299/R301/R310).
    /// The policy that decides a session's window size from its attached clients — tmux's
    /// `window-size` option.
    ///
    /// Every variant here is a rule this daemon PERFORMS — see the module docs of [`crate::options`]
    /// for how a refused value is answered with the list of ones that work.
    ///
    /// Three of the four DERIVE the window from the clients looking at it. [`Manual`](Self::Manual) is
    /// the one that does not, and that difference is why it needs storage nothing else here needs: a
    /// derived answer can be recomputed from facts the daemon already holds, and a declared one exists
    /// only because somebody said it.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
    pub enum WindowSize {
        /// The largest attached client, per dimension — nobody's view is cropped, and smaller clients
        /// see part of the window. tmux's `largest`.
        Largest,
        /// The smallest attached client, per dimension — nothing is ever cropped, and larger clients
        /// have margin. tmux's `smallest`.
        Smallest,
        /// The client that most recently attached or reported a new area. tmux's `latest`, and what
        /// sprag did by construction before any of this existed, which is why it is the default: a
        /// user with one client sees exactly what they always saw.
        Latest,
        /// The size an operator PINNED with `resize-window`
        /// ([`Window::manual_size`](sprag_terminal::Window::manual_size)) — the window stops following
        /// its clients, and a client bigger than it has margin while a smaller one sees part of it.
        /// tmux's `manual`.
        ///
        /// The point of it is that the window survives who is watching. Measured before this variant
        /// existed: a session whose panes a user had arranged at 100x30 was reflowed to 80x24 the
        /// instant any client attached, and stayed there after it left — a program's wrap width decided
        /// by whoever happened to look at it last, permanently, with nothing that could pin it.
        ///
        /// With NOTHING pinned it is not a rule yet, so it defers to [`DEFAULT`](Self::DEFAULT); see
        /// [`arbitrate`].
        Manual,
    }
}

impl WindowSize {
    /// The policy in force when the user has not set one.
    ///
    /// [`Latest`](Self::Latest) because it is what the code already did: one client's window IS the
    /// latest report, so a session with a single viewer behaves identically whether or not this
    /// module exists. A default that changed a solo user's pane sizes would be this front shipping
    /// a behaviour change disguised as a parity feature.
    pub const DEFAULT: Self = Self::Latest;

    /// The value's name in the user's file and in `show-options` — the same spellings tmux uses.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Largest => "largest",
            Self::Smallest => "smallest",
            Self::Latest => "latest",
            Self::Manual => "manual",
        }
    }

    /// Parse a value from the user's file or the CLI, or `None` for one that is not a policy.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == value)
    }
}

/// How a caller NAMES the rectangle `resize-window` is to pin — tmux's `resize-window` flags, as a
/// type.
///
/// # Why this is resolved HERE and not in the caller
///
/// Three of these four are not rectangles; they are descriptions that become one only against facts
/// this daemon holds — the window's CURRENT size and the areas its clients reported. A CLI that
/// resolved them would have to read both back over the wire and do the arithmetic itself, which is a
/// second geometry model in a client: exactly the defect this module was written to remove, arriving
/// by the back door as a convenience. So the wire carries the DESCRIPTION and the daemon answers it.
///
/// That the descriptions reduce to [`arbitrate`] is the point rather than a saving:
/// [`Clients`](Self::Clients) IS an arbitration, and [`Adjust`](Self::Adjust) moves whatever the
/// arbitration currently says. Neither introduces geometry of its own.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SizeRequest {
    /// Exactly this rectangle — tmux `-x`/`-y`.
    Exact(ClientSize),
    /// The window's CURRENT size moved by these signed amounts, per dimension — tmux's `-U`/`-D`
    /// (rows) and `-L`/`-R` (columns), which name an EDGE and how far to push it.
    ///
    /// Relative to what the window currently IS, not to what it was pinned to, because that is what
    /// the user can see: under a derived policy with nothing pinned, "a bit wider" means wider than
    /// the rectangle on their screen. A window with no current size has nothing to be relative to,
    /// and that is [`NoBasis`].
    Adjust { cols: i32, rows: i32 },
    /// Whatever the attached clients fold to under this policy — tmux's `-a` (smallest) and `-A`
    /// (largest), which exist so a user can pin "what I have right now" without reading numbers off
    /// `list-clients` and typing them back.
    Clients(WindowSize),
    /// No rectangle at all: un-pin the window and hand it back to whichever policy derives one.
    Clear,
}

/// A [`SizeRequest`] that named no rectangle a caller could have meant: an [`Adjust`](SizeRequest::Adjust)
/// with no current size to move, or a [`Clients`](SizeRequest::Clients) with no client reporting an area.
///
/// A distinct outcome from [`SizeRequest::Clear`] on purpose. Both could be spelled "no size", and
/// collapsing them would make `resize-window -R 10` on a window nobody is watching silently UN-PIN
/// it — the opposite of what was asked, which is the class of thing this front keeps finding.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NoBasis;

impl std::fmt::Display for NoBasis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this window has no size to resize: nothing is pinned and no client has reported an area")
    }
}

impl std::error::Error for NoBasis {}

/// The smallest window a resize may produce. One cell, not zero: a zero-column window is not a
/// window, and [`tile`] already states what happens as a window shrinks past what its panes need —
/// it DROPS panes rather than shrinking them past nothing, which is its rule and not a decision
/// taken here.
const MIN_WINDOW: u16 = 1;

impl SizeRequest {
    /// The rectangle this request names — `Ok(None)` to un-pin.
    ///
    /// `current` is the window's arbitrated size right now and `reports` the areas its clients gave;
    /// both are the daemon's, which is the whole reason this resolution lives on this side of the
    /// wire.
    ///
    /// # Clamping, and why the ABSOLUTE form refuses instead
    ///
    /// An [`Adjust`](Self::Adjust) saturates at one cell rather than failing, because it names a
    /// DIRECTION and a distance: at the edge, the honest answer to "narrower" is the narrowest, which
    /// is what a user holding a repeat key means. `-x 0` is a different act — a caller naming a
    /// rectangle that does not exist — and it is refused at the CLI, where the value was typed.
    ///
    /// # Errors
    ///
    /// [`NoBasis`] when the description cannot be resolved: an adjustment with no current size, or a
    /// client fold with no client reporting one.
    pub fn resolve(
        self,
        current: Option<ClientSize>,
        reports: &[ClientSize],
    ) -> Result<Option<ClientSize>, NoBasis> {
        match self {
            Self::Clear => Ok(None),
            Self::Exact(size) => Ok(Some(size)),
            Self::Clients(policy) => arbitrate(policy, reports, None).map(Some).ok_or(NoBasis),
            Self::Adjust { cols, rows } => {
                let base = current.ok_or(NoBasis)?;
                Ok(Some(ClientSize {
                    cols: adjusted(base.cols, cols),
                    rows: adjusted(base.rows, rows),
                }))
            }
        }
    }
}

/// One dimension moved by a signed amount and held at `MIN_WINDOW` — saturating at BOTH ends, so
/// neither a huge adjustment nor a huge current size can wrap a `u16` into a window nobody asked for.
fn adjusted(extent: u16, by: i32) -> u16 {
    let moved = i64::from(extent) + i64::from(by);
    u16::try_from(moved.clamp(i64::from(MIN_WINDOW), i64::from(u16::MAX)))
        .unwrap_or(MIN_WINDOW)
        .max(MIN_WINDOW)
}

/// The window `sizes` add up to under `policy`, or `None` when the question has no answer — which
/// is `manual` with nothing pinned and no client reporting, or any other policy with no client
/// reporting.
///
/// `None` is not "use a default": it means the question has no answer yet, and the caller's honest
/// response is to leave the panes at whatever size they already have. Inventing 80x24 here would
/// reflow every program in the session the moment the last client detached.
///
/// # Per DIMENSION, not per client
///
/// `largest` over an 80x50 and a 120x24 client is 120x50 — not "the 120x24 one, because it is
/// wider". This is tmux's rule and it is the only one that keeps the promise each name makes:
/// `largest` exists so that no client's own area is wasted, and picking a whole client would waste
/// 26 rows of the first one. The same argument inverts for `smallest`, which yields 80x24 and
/// guarantees both clients see the whole window.
///
/// [`WindowSize::Latest`] is the exception and necessarily so: it names a CLIENT, so it takes that
/// client's two numbers together rather than mixing dimensions from different ones.
///
/// # `pinned` answers ALONE, and answers nothing when it is absent
///
/// [`WindowSize::Manual`] reads `pinned` and ignores `sizes` entirely — a window that follows nobody
/// is the whole feature, so a client attaching, resizing or leaving moves nothing.
///
/// With `manual` set and NOTHING pinned, this falls through to [`WindowSize::DEFAULT`] rather than
/// answering `None`, and the reason is that `None` is not harmless here. It means "no window", which
/// sends both frontends to their no-window fallback of sizing panes from their own surface — the
/// two-authority defect R242 removed, re-reachable by one `set-option`. Deferring instead makes
/// `set-option window-size manual` on its own a no-op the user cannot see, which is the honest
/// reading of it: they have named a source, and the source is empty until `resize-window` writes it.
#[must_use]
pub fn arbitrate(
    policy: WindowSize,
    sizes: &[ClientSize],
    pinned: Option<ClientSize>,
) -> Option<ClientSize> {
    if let (WindowSize::Manual, Some(pinned)) = (policy, pinned) {
        // The one answer that needs no client at all — including when there are none.
        return Some(pinned);
    }
    let (first, rest) = sizes.split_first()?;
    Some(match policy {
        // `sizes` is oldest-first, so the last element is the most recent report. `Manual` arrives
        // here only with nothing pinned, where it means DEFAULT — and DEFAULT is `Latest`, a
        // coupling `manual_with_nothing_pinned_is_the_default_policy` holds to this arm.
        WindowSize::Latest | WindowSize::Manual => *sizes.last().unwrap_or(first),
        WindowSize::Largest => rest.iter().fold(*first, |acc, size| ClientSize {
            cols: acc.cols.max(size.cols),
            rows: acc.rows.max(size.rows),
        }),
        WindowSize::Smallest => rest.iter().fold(*first, |acc, size| ClientSize {
            cols: acc.cols.min(size.cols),
            rows: acc.rows.min(size.rows),
        }),
    })
}

/// Give every TILED pane of `session` the size the session's window says it has — the derivation
/// that makes a pane's `(cols, rows)` one number instead of one per client.
///
/// # Why the daemon does this rather than each client
///
/// A pane's size is `tile(tree, window)`. The TREE is this daemon's (it owns the arrangement) and
/// the WINDOW is this daemon's (it arbitrates one from every client's report), so the size is a
/// DERIVED fact of two things only this process holds. A client computing it is re-deriving, and
/// two re-derivations are the defect this front keeps finding: before this, `sprag-gui` sized its
/// panes from its own pixels while `sprag-tui` sized them from the window, so attaching a terminal
/// to a session a window was showing silently left the GUI painting a grid whose real dimensions it
/// had never been told.
///
/// It also puts the derivation where the TRIGGERS are. A client can only re-derive when something
/// it can see changes; the window moves when ANOTHER client attaches, resizes or detaches, and
/// `sprag-gui` has no seam that observes any of those (its poll thread requests a repaint; its view
/// is pure). Here, every one of those moments is already a call site.
///
/// # What it does not touch
///
/// * A FLOATING pane has no leaf in the tree, so no tiling ever names it: its window is its own
///   surface and its size stays that surface's business.
/// * `None` from [`arbitrate`] — no attached client has reported an area — resizes nothing. That is
///   the rule the `window_size` slot already states: nobody has said how big this is, so the panes
///   keep the size they have rather than reflowing every program to a number nobody chose.
/// * A pane the window is too small to show is absent from the tiling and so keeps its size, which
///   is [`tile`]'s own stated rule rather than a decision taken here.
///
/// `cell_px` is `(0, 0)` — "unknown", which leaves the pane's last-known pixel geometry alone. A
/// cell's pixel size is a property of a client's FONT; this daemon has none and must not invent
/// one, and it does not have to, because the client that last set it still owns it.
pub(crate) fn retile(
    registry: &Arc<Mutex<SessionRegistry>>,
    attachments: &Arc<Mutex<AttachmentRegistry>>,
    session: &str,
) {
    // Each lock is taken and released in turn — attachments, then registry, then the pool — so this
    // never holds two at once and cannot invert an order some other path takes.
    let sizes = lock(attachments).sizes(session);
    // The scope FIRST, because the pinned size is a property of the window this is about to tile,
    // and both come out of the one registry lock: reading them under two would let a window switch
    // land between, laying one window's tree out over another window's pinned rectangle.
    let (scope, pinned) = {
        let registry = lock(registry);
        let Some(scope) = SessionScope::of(&registry, session) else {
            return;
        };
        let pinned = registry
            .window(session, scope.window())
            .and_then(sprag_terminal::Window::manual_size)
            .map(|(cols, rows)| ClientSize { cols, rows });
        (scope, pinned)
    };
    let Some(window) = arbitrate(crate::config::window_size(), &sizes, pinned) else {
        return;
    };
    let Some(layout) = crate::host::reconciled_layout(registry, &scope) else {
        return;
    };
    // The PROJECTION, not the arrangement: a zoomed pane is sized to the WHOLE window, here, by the
    // daemon, so every attached client shows the same one pane at the same cells — and the panes
    // the zoom hides keep the size they had, exactly as a pane the window is too small to show
    // does, because the tiling does not name them either.
    let tiling = tile(&layout.projection(), Rect::screen(window.cols, window.rows));
    let pool = lock(scope.workspace());
    for held in &tiling.panes {
        // The no-op guard is against WORK, not against correctness: a resize is idempotent, but in
        // the steady state every one of these calls would be a redundant ioctl and a SIGWINCH the
        // program in the pane would have to answer.
        if pool.pane(held.pane).map(|pane| pane.pty().dimensions())
            == Some((held.area.cols, held.area.rows))
        {
            continue;
        }
        if let Err(error) = pool.resize(held.pane, held.area.cols, held.area.rows, CELL_PX_UNKNOWN)
        {
            tracing::warn!(
                target: "sprag_host::window",
                pane = held.pane.0,
                %error,
                "could not resize a pane to its share of the window"
            );
        }
    }
}

/// The `cell_px` a daemon-side reflow carries: unknown, so the pane keeps whatever pixel geometry
/// the client that last resized it established. See [`retile`].
const CELL_PX_UNKNOWN: (u16, u16) = (0, 0);

#[cfg(test)]
mod tests {
    use super::*;

    fn size(cols: u16, rows: u16) -> ClientSize {
        ClientSize { cols, rows }
    }

    #[test]
    fn no_reported_client_and_nothing_pinned_has_no_window() {
        for policy in WindowSize::ALL {
            assert_eq!(
                arbitrate(policy, &[], None),
                None,
                "{} must not invent a window nobody asked for",
                policy.name()
            );
        }
    }

    #[test]
    fn one_client_is_the_window_under_every_policy() {
        // The single-viewer case, which is the one that must not change behaviour: whichever policy
        // a user sets, one client's area IS the window. `manual` is in here too, with nothing
        // pinned — the state a user reaches by setting the option and stopping there.
        for policy in WindowSize::ALL {
            assert_eq!(
                arbitrate(policy, &[size(100, 30)], None),
                Some(size(100, 30)),
                "{}",
                policy.name()
            );
        }
    }

    #[test]
    fn largest_and_smallest_mix_dimensions_across_clients() {
        // Deliberately CROSSED: the wider client is the shorter one, so a policy that picked a
        // whole client rather than folding per dimension would answer 120x24 or 80x50 and be
        // caught here. Neither answer is either client's own area.
        let clients = [size(80, 50), size(120, 24)];
        assert_eq!(
            arbitrate(WindowSize::Largest, &clients, None),
            Some(size(120, 50)),
            "largest takes the widest AND the tallest"
        );
        assert_eq!(
            arbitrate(WindowSize::Smallest, &clients, None),
            Some(size(80, 24)),
            "smallest takes the narrowest AND the shortest"
        );
    }

    #[test]
    fn latest_takes_one_clients_two_numbers_together() {
        // The ordering contract with `AttachmentRegistry::sizes`: oldest first, so the LAST element
        // is the most recent report. A `latest` that folded per dimension would answer 120x50 here,
        // which is a window no client ever reported.
        let clients = [size(80, 50), size(120, 24)];
        assert_eq!(
            arbitrate(WindowSize::Latest, &clients, None),
            Some(size(120, 24))
        );
    }

    /// THE claim `manual` is for: the answer is the pinned rectangle and the clients do not enter
    /// into it — including when there are none, which is the case every other policy answers `None`
    /// to and the one a detached session lives in.
    #[test]
    fn a_pinned_window_ignores_every_client_and_needs_none() {
        let pinned = size(100, 30);
        assert_eq!(
            arbitrate(WindowSize::Manual, &[], Some(pinned)),
            Some(pinned),
            "a pinned window has a size with nobody attached at all"
        );
        // Two clients that agree on neither dimension with the pin, one of them the most recent
        // report — so a `manual` that folded, picked, or fell through would answer something else.
        let clients = [size(80, 50), size(120, 24)];
        assert_eq!(
            arbitrate(WindowSize::Manual, &clients, Some(pinned)),
            Some(pinned),
            "a pinned window follows nobody"
        );
    }

    /// The coupling the `Latest | Manual` arm of [`arbitrate`] rests on: `manual` with nothing
    /// pinned IS [`WindowSize::DEFAULT`], so if that default ever moves off `latest` this fails
    /// rather than the arm silently answering the wrong policy's question.
    #[test]
    fn manual_with_nothing_pinned_is_the_default_policy() {
        let cases: [&[ClientSize]; 3] = [
            &[],
            &[size(100, 30)],
            &[size(80, 50), size(120, 24), size(90, 40)],
        ];
        for clients in cases {
            assert_eq!(
                arbitrate(WindowSize::Manual, clients, None),
                arbitrate(WindowSize::DEFAULT, clients, None),
                "manual with nothing pinned must answer exactly what {} does, over {clients:?}",
                WindowSize::DEFAULT.name()
            );
        }
    }

    /// A pinned size is INERT under every policy that derives one, which is what makes
    /// `resize-window` safe to run before choosing to use it — and what the CLI's note is about.
    #[test]
    fn a_pinned_size_changes_nothing_until_the_policy_names_it() {
        let clients = [size(80, 50), size(120, 24)];
        let pinned = Some(size(100, 30));
        for policy in WindowSize::ALL {
            if policy == WindowSize::Manual {
                continue;
            }
            assert_eq!(
                arbitrate(policy, &clients, pinned),
                arbitrate(policy, &clients, None),
                "{} must not read a pinned size",
                policy.name()
            );
        }
    }

    #[test]
    fn default_is_latest_because_that_is_what_the_code_already_did() {
        assert_eq!(WindowSize::DEFAULT, WindowSize::Latest);
    }

    /// A relative request moves what the window IS, per dimension and independently, and an unnamed
    /// axis stays put.
    #[test]
    fn an_adjustment_moves_the_current_size_one_axis_at_a_time() {
        let now = Some(size(80, 24));
        let go = |cols, rows| {
            SizeRequest::Adjust { cols, rows }
                .resolve(now, &[])
                .expect("a current size is a basis")
        };
        assert_eq!(go(10, 0), Some(size(90, 24)), "wider, same height");
        assert_eq!(go(0, -4), Some(size(80, 20)), "shorter, same width");
        assert_eq!(go(-10, 6), Some(size(70, 30)), "both edges at once");
        assert_eq!(go(0, 0), Some(size(80, 24)), "a zero adjustment is a no-op");
    }

    /// The clamp is at BOTH ends, and it saturates rather than wrapping — a `u16` that wrapped would
    /// answer a huge window to a request for a smaller one.
    #[test]
    fn an_adjustment_saturates_instead_of_wrapping() {
        let request = |cols, rows| SizeRequest::Adjust { cols, rows };
        assert_eq!(
            request(-1000, -1000).resolve(Some(size(80, 24)), &[]),
            Ok(Some(size(MIN_WINDOW, MIN_WINDOW))),
            "past the bottom is the smallest window, not a wrapped one"
        );
        assert_eq!(
            request(i32::MAX, i32::MAX).resolve(Some(size(80, 24)), &[]),
            Ok(Some(size(u16::MAX, u16::MAX))),
            "past the top is the largest, not a wrapped one"
        );
        assert_eq!(
            request(i32::MIN, i32::MIN).resolve(Some(size(u16::MAX, u16::MAX)), &[]),
            Ok(Some(size(MIN_WINDOW, MIN_WINDOW))),
            "the widest window shrunk by the most negative delta"
        );
    }

    /// `-a` / `-A` ARE arbitrations: the request folds the clients through the same function a policy
    /// does, so there is no second rule for "the largest client" to disagree with.
    #[test]
    fn a_client_fold_request_is_the_arbitration_itself() {
        let clients = [size(80, 50), size(120, 24)];
        for policy in [
            WindowSize::Largest,
            WindowSize::Smallest,
            WindowSize::Latest,
        ] {
            assert_eq!(
                SizeRequest::Clients(policy).resolve(Some(size(9, 9)), &clients),
                Ok(arbitrate(policy, &clients, None)),
                "{} as a SOURCE must answer exactly what it answers as a POLICY",
                policy.name()
            );
        }
        // ...and it ignores whatever the window currently is, which is what makes it a fold of the
        // clients rather than an adjustment of the pin.
        assert_eq!(
            SizeRequest::Clients(WindowSize::Largest).resolve(None, &clients),
            Ok(Some(size(120, 50)))
        );
    }

    /// THE distinction that keeps a resize from becoming an un-pin: a request that cannot be resolved
    /// is an ERROR, not "no size". Collapsing the two would make `resize-window -R 10` on a window
    /// nobody is watching silently un-pin it.
    #[test]
    fn an_unresolvable_request_is_refused_and_is_not_a_clear() {
        assert_eq!(
            SizeRequest::Adjust { cols: 10, rows: 0 }.resolve(None, &[]),
            Err(NoBasis),
            "an adjustment with no current size has nothing to move"
        );
        assert_eq!(
            SizeRequest::Clients(WindowSize::Largest).resolve(Some(size(80, 24)), &[]),
            Err(NoBasis),
            "a fold of no clients is not the window's current size either"
        );
        assert_eq!(
            SizeRequest::Clear.resolve(None, &[]),
            Ok(None),
            "and the request that really does mean `no size` still says so"
        );
    }

    /// An exact request is the one spelling that needs nothing from the daemon, and it must not pick
    /// anything up from what happens to be around.
    #[test]
    fn an_exact_request_ignores_the_window_and_its_clients() {
        let want = size(111, 33);
        assert_eq!(
            SizeRequest::Exact(want).resolve(Some(size(80, 24)), &[size(60, 20)]),
            Ok(Some(want))
        );
        assert_eq!(SizeRequest::Exact(want).resolve(None, &[]), Ok(Some(want)));
    }

    #[test]
    fn every_name_round_trips_and_a_non_policy_is_refused() {
        for policy in WindowSize::ALL {
            assert_eq!(WindowSize::parse(policy.name()), Some(policy));
        }
        assert_eq!(WindowSize::parse(""), None);
        assert_eq!(WindowSize::parse("Largest"), None, "values are lower-case");
        // A word that names no rule is refused rather than mapped to something near it; the caller
        // answers with the list that works.
        assert_eq!(WindowSize::parse("automatic"), None);
    }

    /// **THE ZOOM'S POINT, end to end through the real reflow.** A zoomed pane's PTY is resized to
    /// the WHOLE arbitrated window by the daemon — so the program in it reflows to the size every
    /// attached client is showing, and it does so once, here, rather than per frontend.
    ///
    /// This is where sprag's placement of the filter pays: herdr applies its zoom in the renderer
    /// and resizes the focused runtime there (`src/ui/panes.rs:169-197` at `9a4ce5e1`), a branch it
    /// carries twice already and would need a third copy of for a second frontend. sprag has one
    /// `tile`, so the GUI, the terminal client and this reflow cannot disagree about how big the
    /// zoomed pane is.
    ///
    /// The other half is asserted alongside and is not a detail: the panes the zoom HIDES keep the
    /// size they had. They are absent from the tiling exactly as a pane the window is too small to
    /// show is, which is `tile`'s own stated rule rather than a decision taken for the zoom — and
    /// it is what makes ending the zoom cost no reflow for the panes that never moved.
    ///
    /// Revert-proof: tile the ARRANGEMENT here instead of the projection and the first assertion
    /// reads the zoomed pane's share of a three-way split, not the window.
    #[test]
    fn a_zoomed_pane_is_reflowed_to_the_whole_window_and_the_hidden_ones_are_left_alone() {
        use sprag_terminal::{PaneId, SessionRegistry, Workspace};
        use std::sync::Mutex;

        let mut command = sprag_terminal::CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        let registry = Arc::new(Mutex::new(SessionRegistry::new((80, 24))));
        let session = lock(&registry).default_session().name().to_owned();
        let pool = lock(&registry)
            .workspace_of(&session)
            .expect("the default session always resolves");
        let panes: Vec<PaneId> = (0..3)
            .map(|_| {
                lock(&pool)
                    .spawn(command.clone(), "sh".to_owned(), 80, 24)
                    .expect("a pane spawns")
            })
            .collect();

        // One attached client reporting 90x30 — with a single client every policy answers its area,
        // so the arbitrated window is exactly that whatever the user's `window-size` is set to.
        let attachments = Arc::new(Mutex::new(crate::attach::AttachmentRegistry::default()));
        let conn = pinion_rpc::ConnId::allocate();
        lock(&attachments).hello(conn, "client-under-test".to_owned());
        let id = lock(&registry)
            .session(&session)
            .expect("the fixture session")
            .id();
        lock(&attachments).attach(conn, session.clone(), id);
        lock(&attachments).size(conn, ClientSize { cols: 90, rows: 30 });
        assert_eq!(
            lock(&attachments).sizes(&session),
            vec![ClientSize { cols: 90, rows: 30 }],
            "the fixture's client is the one this session's window is arbitrated from",
        );

        retile(&registry, &attachments, &session);
        let dims = |pane: PaneId| {
            let pool: &Mutex<Workspace> = &pool;
            lock(pool)
                .pane(pane)
                .map(|pane| pane.pty().dimensions())
                .expect("the pane is alive")
        };
        let arranged = panes.iter().map(|pane| dims(*pane)).collect::<Vec<_>>();
        assert!(
            arranged.iter().all(|held| *held != (90, 30)),
            "three tiled panes each get a SHARE of the window, so none of them is the whole of \
             it — which is what makes the assertion below about the zoom: {arranged:?}",
        );

        assert!(
            lock(&registry)
                .zoom_pane(&session, panes[1], Some(true))
                .expect("the pane is one of the window's")
                .zoomed
        );
        retile(&registry, &attachments, &session);

        assert_eq!(
            dims(panes[1]),
            (90, 30),
            "the zoomed pane is the window, in the daemon's own reflow",
        );
        assert_eq!(
            (dims(panes[0]), dims(panes[2])),
            (arranged[0], arranged[2]),
            "and the panes it hides keep the size they had — absent from a tiling is not resized \
             to nothing",
        );
    }
}
