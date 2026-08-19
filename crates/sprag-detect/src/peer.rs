//! **WHAT AN AGENT IS, DECLARED ONCE** — and, for each fact, WHICH CHANNEL answers it.
//!
//! # ⚠⚠⚠⚠⚠ Why this module exists: the facts were true, and they lived in four places
//!
//! Register item 452, raised by the owner as *"하드코딩으로 도배되고 있는 거 아니야?"* — and the
//! answer was yes, with the sharp half being the LAYER rather than the string. Measured, the four
//! homes of *what `claude` is* were:
//!
//! | fact | where it lived | what kind of place that is |
//! |---|---|---|
//! | the refusal key | `sprag_plugin::screen::REFUSES` | a Rust `const` |
//! | the consent dialogs | `may_answer` | a kind document |
//! | the readiness marker | `--match settles --marker claude` | a RUN ARGUMENT |
//! | the error vocabulary | `service_needle` | a kind document |
//!
//! **All four are facts about one program at one version, and no two of them lived together.** A
//! second repository adopting the template re-derives every one.
//!
//! # ⚠⚠⚠⚠ What this declares, and what it deliberately does NOT hold
//!
//! Two of those four are **per-REPOSITORY** and belong in the kind document that authors them: which
//! dialogs a caller consents to, and which words that caller's peer prints when its service fails,
//! are decisions about a workload rather than facts about a program. Copying them here would be a
//! fifth home rather than one fewer.
//!
//! So this declares the facts that are the AGENT's, and for the authored two it declares **the
//! channel they arrive on** — which is the half no one could read anywhere. That channel is not
//! decoration: [`Peer::speaks_up`] is what demotes a screen scrape to a fallback (item 452's third
//! clause), and it is read rather than described.
//!
//! # ⚠ Why it is in THIS crate
//!
//! `sprag-detect` is the one crate both the host and the plugin layer already depend on, and it is
//! pure — no lock, no wire, no clock. Register item 150 named it before this module existed:
//! *"which key refuses a tool call is a fact about an AGENT, so its long-term home is that agent's
//! manifest in `sprag-detect`, beside the dialog rules"*.

/// **WHERE A FACT ABOUT A PEER COMES FROM** — the distinction register item 452 says no surface
/// could state.
///
/// # ⚠⚠⚠⚠⚠ It is the difference between evidence and a guess
///
/// A hook payload is the PROGRAM's own account, delivered before a terminal has drawn a cell of it.
/// A screen is pixels this build parses, and every round spent tightening that parse is recorded in
/// this workspace as a round spent on the wrong layer: 40 characters became 40 COLUMNS, a head
/// became a TAIL, an exact match became a whitespace-insensitive one, and a full-screen agent's
/// line addresses were measured frozen at 37 while it wrote reply after reply.
///
/// ⚠⚠ **A FACT WITH NO HOOK IS NOT A DEFECT.** Some things a peer never states — which key its
/// dialog takes is answered by pressing it — and some peers state nothing at all
/// ([`Peer::speaks_up`]). Naming the channel is what lets a reader tell *nobody built the pipe* from
/// *there is no pipe to build*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// The peer states it, through its own hook, in a payload.
    Hook,
    /// This build reads it off the pane, as pixels.
    Screen,
    /// The peer states it where it can, and the screen answers where it cannot — the shape a fact
    /// takes when SOME peers have a hook for it and others do not.
    ///
    /// ⚠ The order is the meaning: the hook is asked FIRST and the screen is the fallback, never
    /// the reverse. A screen consulted first would make the hook decorative.
    HookThenScreen,
}

impl Channel {
    /// The word this channel is published under, for a reader with one line.
    #[must_use]
    pub const fn wire_str(self) -> &'static str {
        match self {
            Self::Hook => "hook",
            Self::Screen => "screen",
            Self::HookThenScreen => "hook-then-screen",
        }
    }
}

/// **ONE AGENT, AND WHAT IS TRUE OF IT** — see the module doc for the four homes this replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    /// What this agent is called — the name a manifest publishes and a run names.
    pub name: &'static str,
    /// **THE KEY THAT TURNS A TOOL CALL DOWN.**
    ///
    /// Measured against a live `claude` 2.1.232: it removes the dialog in ~25 ms and the call is
    /// reported `User rejected` — identical to selecting an offered `No`, except that it needs no
    /// matching option to exist, which is precisely the case a consent cannot reach.
    ///
    /// ⚠ [`Channel::Screen`] and it could not be otherwise: this is not a fact a program reports,
    /// it is a key somebody presses at a dialog and then reads the result of.
    pub refuses: &'static str,
    /// **THE MARKER A RUN WAITS FOR BEFORE IT TYPES ANYTHING** — this agent's own name, as its
    /// settled pane shows it.
    ///
    /// ⚠ [`Channel::Screen`]: readiness is *has this program finished coming up*, and it is
    /// answered by the supervisor's verdict over the pane. A launch states nothing about it.
    pub readiness_marker: &'static str,
    /// **WHETHER THIS PEER RAISES A NOTICE CARRYING PROSE** — the one field here that a caller
    /// BRANCHES on rather than reads.
    ///
    /// # ⚠⚠⚠⚠⚠ It is what makes a screen scrape a FALLBACK rather than the answer
    ///
    /// Register item 452's third clause: `service_needle` is DEMOTED to the fallback for peers with
    /// no hook, **not deleted**. This is the predicate that demotion is written against — `true`
    /// means the peer's own words are worth asking for first, `false` means the screen is the only
    /// thing that will ever answer and asking anything else is theatre.
    ///
    /// ⚠⚠ **MEASURED, NOT ASSUMED**: `sprag_host::hooks::CODEX` carries no `Notification` row at
    /// all — its schema is Claude's with that one substitution — so a codex pane states nothing
    /// this way however well the pipe is built. The two tables are held in step by a gate in the
    /// host, which is the one crate that can see both.
    pub speaks_up: bool,
    /// The channel [`refuses`](Self::refuses) arrives on — always [`Channel::Screen`], named rather
    /// than assumed so the table reads uniformly.
    pub refusal_channel: Channel,
    /// The channel the CONSENT DIALOGS arrive on.
    ///
    /// ⚠ The dialogs themselves are authored per REPOSITORY (`may_answer`, in the kind document),
    /// because which questions a caller consents to is a decision about a workload. What is the
    /// AGENT's, and is declared here, is that the question is read off the pane as a numbered menu —
    /// so a peer blocked on anything else cannot be answered by any consent, however written.
    pub consent_channel: Channel,
    /// The channel the ERROR VOCABULARY arrives on.
    ///
    /// ⚠⚠⚠ [`Channel::HookThenScreen`] for a peer that [`speaks_up`](Self::speaks_up), and
    /// [`Channel::Screen`] for one that does not — which is item 452's demotion stated as a value.
    /// The words themselves stay authored per repository (`service_needle`): what a peer prints when
    /// it is unwell is that peer's sentence, and a template does not know whose agent it will be
    /// talking to.
    pub outage_channel: Channel,
}

impl Peer {
    /// Claude Code — the agent every measurement in this workspace was taken against.
    pub const CLAUDE: Self = Self {
        name: "claude",
        refuses: "Escape",
        readiness_marker: "claude",
        // Its `Notification` row is the whole reason `AGENT_NOTICED_KEY` exists — see item 452.
        speaks_up: true,
        refusal_channel: Channel::Screen,
        consent_channel: Channel::Screen,
        outage_channel: Channel::HookThenScreen,
    };

    /// OpenAI's `codex` — the peer that makes the channel worth naming.
    ///
    /// ⚠⚠⚠ **IT RAISES NO NOTICE**, so every sentence it would otherwise state has to be read off
    /// its pane. That is not a gap to be filled later: its hook schema has a `PermissionRequest`
    /// where Claude's has a `Notification`, and a permission request carries no prose about a
    /// service being down.
    ///
    /// ⚠ Its refusal key is UNMEASURED against a live `codex` (register item 4). Claude's is
    /// carried here rather than a second guess, and the residue is stated rather than hidden: a run
    /// that presses this at a codex dialog is pressing a key nobody has watched land.
    pub const CODEX: Self = Self {
        name: "codex",
        refuses: "Escape",
        readiness_marker: "codex",
        speaks_up: false,
        refusal_channel: Channel::Screen,
        consent_channel: Channel::Screen,
        outage_channel: Channel::Screen,
    };

    /// Every peer this build knows, so a reader and a lookup cannot come to disagree.
    pub const ALL: [Self; 2] = [Self::CLAUDE, Self::CODEX];
}

/// The peer called `name`, or [`None`] for one this build has no declaration for.
///
/// ⚠⚠⚠ **`None` IS *THIS BUILD KNOWS NOTHING ABOUT THAT PROGRAM*, never *it has no hook*.** A caller
/// that read the absence as [`Peer::speaks_up`] `== false` would silently demote every peer somebody
/// adds to the screen — the same collapse this module was built to end. Callers that must decide
/// anyway say what they assume at the site.
#[must_use]
pub fn of(name: &str) -> Option<&'static Peer> {
    // Indexed off `ALL` rather than a second `match`, so a peer added to the table is reachable by
    // the lookup in the same edit — the drift this whole module is about, one function down.
    Peer::ALL.iter().find(|peer| peer.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠⚠ **THE LOOKUP AND THE TABLE ARE ONE LIST** — the drift this module exists to end, asserted
    /// against the module itself.
    #[test]
    fn every_declared_peer_is_reachable_by_name() {
        for peer in &Peer::ALL {
            assert_eq!(
                of(peer.name),
                Some(peer),
                "a peer in the table that the lookup cannot find is a fifth home, not one fewer",
            );
        }
        assert_eq!(
            of("nobody-ships-this"),
            None,
            "and an unknown program is UNKNOWN — not a peer with no hook, which is a different \
             claim and the one that would silently demote a real channel to the screen",
        );
    }

    /// ⚠⚠⚠ **THE REFUSAL KEY HAS ONE HOME, AND THIS IS IT** — the first of item 452's four, asserted
    /// from the side that would otherwise go quiet.
    ///
    /// `sprag_plugin::screen::REFUSES` is now this value rather than its own literal. That is
    /// invisible from either side alone: two copies of `"Escape"` agree perfectly until somebody
    /// edits one. This gate cannot see the plugin (it is downstream), so what it holds is the half
    /// this crate owns — that the declaration says what was measured — and the plugin's own doc
    /// carries the residue that its call site is still agent-blind (item 150).
    #[test]
    fn the_refusal_key_is_the_one_measured_against_a_live_agent() {
        assert_eq!(
            Peer::CLAUDE.refuses,
            "Escape",
            "⚠⚠⚠⚠ this is the key a live `claude` 2.1.232 was measured taking — it removed the \
             dialog in ~25 ms and the call came back `User rejected`. Changing it here changes what \
             every run presses at every dialog, which is the point of there being one home",
        );
        assert_eq!(
            Peer::CLAUDE.refusal_channel,
            Channel::Screen,
            "and it is a SCREEN fact: nothing reports which key a dialog takes — it is pressed, and \
             the result is read",
        );
    }

    /// ⚠⚠⚠⚠⚠ **A PEER THAT STATES NOTHING MUST NOT CLAIM A HOOK CHANNEL** — the invariant item
    /// 452's demotion is written against.
    ///
    /// The two fields could drift in either direction and only one direction is loud. A peer wrongly
    /// marked `speaks_up` gets asked for words it never says: the answer is always absent, the
    /// screen fallback runs anyway, and **nothing fails** — a silent, permanent no-op dressed as a
    /// channel. This gate is the only thing that would say so.
    #[test]
    fn a_peer_that_raises_no_notice_says_the_screen_is_the_only_answer() {
        for peer in &Peer::ALL {
            let expected = if peer.speaks_up {
                Channel::HookThenScreen
            } else {
                Channel::Screen
            };
            assert_eq!(
                peer.outage_channel, expected,
                "⚠⚠⚠⚠⚠ {}'s outage channel disagrees with whether it speaks at all. A `Hook` on a \
                 silent peer is a pipe that can never carry anything and cannot fail loudly; a \
                 `Screen` on a speaking one throws away the evidence item 452 was paid to obtain",
                peer.name,
            );
        }
    }
}
