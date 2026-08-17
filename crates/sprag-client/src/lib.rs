//! `sprag-client` — the display-independent half of a sprag display client.
//!
//! A sprag frontend is two things bolted together: a relationship with a `sprag-term` host
//! (which session, which panes, what changed, what to send when a key is pressed) and a way of
//! DRAWING that. The first half is identical whether the pixels are painted by a GPU or written
//! as escape sequences to a terminal. This crate is that half.
//!
//! It exists because sprag is growing a SECOND frontend — a terminal client, so a session can be
//! attached over ssh. The alternative was for that client to
//! reimplement the wire, and the wire's own module says why that would be wrong:
//! [`sprag_host::wire`] is "the ONE definition of the JSON-RPC address grammar and action names".
//! A second client spelling those addresses itself would be a second definition wearing a
//! different name.
//!
//! # The membership rule
//!
//! A module belongs here when its SUBJECT is the host relationship, and not merely when it
//! happens to compile without a GPU. Those are different tests, and the second one is far too
//! generous: most of `sprag-gui`'s widgets compile GPU-free (pinion's paint helpers do) while
//! emitting scene nodes a terminal client can never use. Membership is decided by what a module
//! is ABOUT.
//!
//! # GPU-free BY GATE
//!
//! The property this crate exists to have is mechanical, so it is asserted mechanically rather
//! than trusted:
//!
//! ```text
//! cargo tree -p sprag-client -e normal | grep -icE '^(vello|wgpu|winit)'   # must be 0
//! ```
//!
//! `tests/gpu_free.rs` runs exactly that and fails the build if it is not zero.
//!
//! **Run it against the WORKSPACE, never a crate in isolation**, because `cargo tree -p X` on its
//! own activates X's DEFAULT features and will tell you a crate is GPU-bearing when this
//! workspace resolves it without the GPU features at all. `pinion-runtime` reports 16 such
//! dependencies standalone and ZERO inside `sprag-host`'s tree. The honest question is never
//! whether a crate CAN pull the GPU stack — it is what this workspace resolves for the crate
//! under test.

mod agent;
mod wire;

/// H3's agent verdict rendered for a person — the WORDS both frontends put their own frame around.
///
/// Public because two display clients consume it and neither may spell the vocabulary itself; see the
/// module's own docs for why a rendering belongs in this crate at all.
pub use agent::{agent_phrase, agent_urgency};

/// The topology-B wire client: a display client's whole relationship with a `sprag-term` host
/// PROCESS, behind the same [`HostClient`](sprag_host::HostClient) protocol an in-process
/// [`Host`](sprag_host::Host) implements.
///
/// Re-exported at the crate root as the ONE public address for this type — the module stays
/// private so there is no second spelling of it, the same anti-aliasing rule
/// [`sprag_host::wire`]'s address families follow.
pub use wire::WireHost;

/// The boot's own vocabulary: what a client is booting ([`BootSpec`]), and what a boot that failed
/// did about the session it had created ([`BootError`]).
///
/// Public for the same reason [`WireHost`] is — a frontend that boots a client is the caller of
/// [`WireHost::boot`], and a caller that must decide what to tell its user about a failed boot
/// needs the facts (which daemon, what was left behind) rather than a formatted sentence.
pub use wire::{BootError, BootSpec};

/// WHICH KIND OF FRONTEND is booting, for the defaults whose right answer differs between a window
/// and a terminal.
///
/// Public because only the caller can answer it: this crate serves both frontends and one shared
/// default is what register item 282 measured — a window that closed its attached session and quit
/// with three others alive, because it inherited the default that is correct for a terminal.
pub use wire::Frontend;
