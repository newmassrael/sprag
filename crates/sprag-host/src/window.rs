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

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use sprag_terminal::{Rect, SessionRegistry, tile};

use crate::attach::{AttachmentRegistry, ClientSize};
use crate::lock;
use crate::scope::SessionScope;

/// The policy that decides a session's window size from its attached clients — tmux's
/// `window-size` option.
///
/// Every variant here is a rule this daemon PERFORMS. tmux's fourth value, `manual`, is not
/// offered: it means "the window size changes only when `resize-window` says so", which needs a
/// per-session size the daemon stores and a verb that writes it, and an option value naming a rule
/// nothing performs is the defect this front keeps finding. See the module docs of
/// [`crate::options`] for how a refused value is answered with the list of ones that work.
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
        }
    }

    /// Every value, in the order they are offered to a user who typed one that does not exist.
    pub const ALL: [Self; 3] = [Self::Largest, Self::Smallest, Self::Latest];

    /// Parse a value from the user's file or the CLI, or `None` for one that is not a policy.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == value)
    }
}

/// The window `sizes` add up to under `policy`, or `None` when no attached client has reported an
/// area.
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
#[must_use]
pub fn arbitrate(policy: WindowSize, sizes: &[ClientSize]) -> Option<ClientSize> {
    let (first, rest) = sizes.split_first()?;
    Some(match policy {
        // `sizes` is oldest-first, so the last element is the most recent report.
        WindowSize::Latest => *sizes.last().unwrap_or(first),
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
    let Some(window) = arbitrate(crate::config::window_size(), &sizes) else {
        return;
    };
    let Some(scope) = SessionScope::of(&lock(registry), session) else {
        return;
    };
    let Some(layout) = crate::host::reconciled_layout(registry, &scope) else {
        return;
    };
    let tiling = tile(&layout.tree, Rect::screen(window.cols, window.rows));
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
    fn no_reported_client_has_no_window() {
        for policy in WindowSize::ALL {
            assert_eq!(
                arbitrate(policy, &[]),
                None,
                "{} must not invent a window nobody asked for",
                policy.name()
            );
        }
    }

    #[test]
    fn one_client_is_the_window_under_every_policy() {
        // The single-viewer case, which is the one that must not change behaviour: whichever policy
        // a user sets, one client's area IS the window.
        for policy in WindowSize::ALL {
            assert_eq!(
                arbitrate(policy, &[size(100, 30)]),
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
            arbitrate(WindowSize::Largest, &clients),
            Some(size(120, 50)),
            "largest takes the widest AND the tallest"
        );
        assert_eq!(
            arbitrate(WindowSize::Smallest, &clients),
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
        assert_eq!(arbitrate(WindowSize::Latest, &clients), Some(size(120, 24)));
    }

    #[test]
    fn default_is_latest_because_that_is_what_the_code_already_did() {
        assert_eq!(WindowSize::DEFAULT, WindowSize::Latest);
    }

    #[test]
    fn every_name_round_trips_and_a_non_policy_is_refused() {
        for policy in WindowSize::ALL {
            assert_eq!(WindowSize::parse(policy.name()), Some(policy));
        }
        // tmux's fourth value is NOT silently accepted as something else: it is not offered, so it
        // is refused like any other word, and the caller lists what does work.
        assert_eq!(WindowSize::parse("manual"), None);
        assert_eq!(WindowSize::parse(""), None);
        assert_eq!(WindowSize::parse("Largest"), None, "values are lower-case");
    }
}
