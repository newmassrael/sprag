//! What an agent's state SAYS to a person — H3's verdict rendered as words.
//!
//! # Why the wording lives here and not in a frontend
//!
//! This crate's membership rule is that a module belongs here when its subject is the HOST
//! RELATIONSHIP, and a rendering is a display decision. This one is here because of what it renders
//! FROM and what would happen if it did not: the state token is a host fact
//! ([`sprag_host::PaneAgent::state`]), and both frontends have to say the same thing about it.
//!
//! The GUI puts the phrase beside a pane's title on every title surface it owns; the terminal client
//! puts it in the outer terminal's window title, which is the only chrome it has. Those are different
//! PLACES, and that is exactly why the WORDS have to be one function: two `match` blocks over
//! `"blocked"` are how a user comes to read "blocked" in one client and "needs an answer" in the
//! other, for one pane, in one session. The GUI already learned this at a smaller scale — its exit
//! marker is one function precisely so the sighted title and the a11y name cannot drift.
//!
//! It is display-INDEPENDENT in the sense this crate means: words, not cells and not pixels. Each
//! frontend still owns where they go and what frames them.

use sprag_host::PaneAgent;

/// What the agent in a pane is doing, as a phrase a person can act on — `"claude working"`,
/// `"claude needs an answer"`, or bare `"working"` for a verdict with no manifest name on it.
///
/// # The three editorial rules, each with the reason it is not a preference
///
/// **`blocked` is rendered as "needs an answer".** `sprag_detect::AgentState::Blocked` — a type this crate cannot name in a link, since the detector is
/// not in its dependency graph — is "the agent has ASKED something and cannot continue until it is
/// answered": a request. "Blocked" reads
/// as a fault, and this project has already taken that decision once for the same reason: the GUI's
/// exit marker says "exited" rather than tmux's "dead" because most endings are not failures.
///
/// **A state this build does not know is passed through VERBATIM.** The token arrives from a daemon
/// that may be newer than the client reading it — they are separate processes and a user upgrades one
/// first — so an unmatched token is a real case rather than a defensive one. Passing it through costs
/// a slightly odd phrase; dropping it costs the pane its state, silently, which is the failure that
/// looks like nothing being wrong.
///
/// **A nameless verdict still speaks.** R251 measured the case: a modal can cover the very lines an
/// agent's fingerprint is made of, so the daemon's tracker publishes a state with no identity
/// rather than dropping the state. The state is the part a person needs.
#[must_use]
pub fn agent_phrase(agent: &PaneAgent) -> String {
    let doing = match agent.state.as_str() {
        "blocked" => "needs an answer",
        other => other,
    };
    match &agent.name {
        Some(name) => format!("{name} {doing}"),
        None => doing.to_owned(),
    }
}

/// How URGENT a state is to the person reading it — `0` is the most urgent.
///
/// Used where several panes' states share one line and the line can be truncated (a terminal's window
/// title is the case that forced this), so the ordering is not cosmetic: D3 says `Blocked` is the
/// state this whole front exists for, and a digest that buried it behind two working panes would put
/// the one thing a person has to act on in the part their terminal cuts off.
///
/// `idle` outranks `working` for the same reason one place further on: an agent at rest is waiting for
/// somebody, and one that is working is not.
///
/// An unknown token sorts LAST rather than first. It is the honest place for it — this build cannot
/// know what a future state means, and guessing that it is urgent would let a newer daemon's routine
/// state displace a blocked pane.
#[must_use]
pub fn agent_urgency(state: &str) -> u8 {
    match state {
        "blocked" => 0,
        "idle" => 1,
        "working" => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(state: &str, name: Option<&str>) -> PaneAgent {
        PaneAgent {
            state: state.to_owned(),
            name: name.map(str::to_owned),
            rule: Some("idle-glyph".to_owned()),
            seq: 1,
        }
    }

    /// The vocabulary as a person reads it, including the one token that is deliberately NOT its wire
    /// spelling.
    ///
    /// REVERT-PROOF: render `agent.state` verbatim for every arm and the `blocked` assertion fails —
    /// which is the difference between a pane that has asked the user a question and a pane that
    /// reads as broken.
    #[test]
    fn a_blocked_agent_is_asking_rather_than_failing() {
        assert_eq!(
            agent_phrase(&agent("blocked", Some("claude"))),
            "claude needs an answer",
        );
        assert_eq!(
            agent_phrase(&agent("working", Some("claude"))),
            "claude working"
        );
        assert_eq!(agent_phrase(&agent("idle", Some("codex"))), "codex idle");
    }

    /// The two shapes only a client can meet: a token from a NEWER daemon, and a verdict a modal left
    /// without a name (R251).
    ///
    /// REVERT-PROOF: match the token exhaustively and return `""` for anything else, and the first
    /// assertion fails; require the name and the second does.
    #[test]
    fn an_unknown_token_and_a_nameless_verdict_both_still_say_something() {
        assert_eq!(
            agent_phrase(&agent("compacting", Some("claude"))),
            "claude compacting",
            "a state this build never heard of reaches the user rather than vanishing",
        );
        assert_eq!(
            agent_phrase(&agent("working", None)),
            "working",
            "the state is the part a person needs; the name is not always available",
        );
    }

    /// The ordering a truncated line depends on: blocked first, then idle, then working, with an
    /// unknown state last.
    ///
    /// REVERT-PROOF: give every state the same rank and the sort becomes input order, so a blocked
    /// pane sits wherever the pane list happened to put it — behind the working panes, in the half of
    /// a terminal title that gets cut off.
    #[test]
    fn urgency_puts_the_state_that_needs_a_person_first() {
        let mut states = ["working", "later-thing", "idle", "blocked"];
        states.sort_by_key(|state| agent_urgency(state));
        assert_eq!(states, ["blocked", "idle", "working", "later-thing"]);
    }
}
