//! **WHICH CODE THIS IMAGE WAS BUILT FROM**, as one word, for anything that has to say so.
//!
//! # ⚠⚠⚠⚠⚠ Why this is a crate and not a constant in whichever crate needed it first
//!
//! Register item 1066. The stamp lived in `sprag-rpc` beside `WIRE_PROTOCOL` from register item
//! 438 until 2026-09-12, and the argument for putting it there was sound — see `build.rs`, which
//! still carries it. What that placement decided WITHOUT SAYING SO is who may ask: only a crate
//! that links the wire.
//!
//! The crate that most needed to ask is the one that could not. `sprag-plugin` drives the AI loop
//! and composes sentences a person reads and acts on; it links no wire by charter (pinion-free,
//! serde-free), so a sentence it wrote could not name the build that wrote it. Measured 2026-09-12
//! (R317): a driver's sentence produced by a seven-day-old daemon was read as today's, and telling
//! the two apart cost a process audit and four string searches.
//!
//! ⚠⚠ **A SECOND `build.rs` WOULD HAVE BEEN THE CHEAP FIX AND IT IS THE WRONG ONE**: one fact with
//! two authors, two copies of a worktree resolution with a documented gotcha in it, and two places
//! for a policy change (the commit's width, the dirty flag `build.rs` refuses) to reach one binary
//! as two spellings of one commit. A crate below everything has one author and one spelling.
//!
//! # ⚠⚠ Why the name is neither `sprag-build` nor `sprag-image`
//!
//! `-build` is this workspace's word for build-time machinery — `sce-build` is the SCXML compiler
//! two manifests over — so a crate by that name reads as a helper rather than as the fact. `-image`
//! is the word `build.rs` uses for the subject and it is already taken at the product surface,
//! where a pane's images are pictures. `stamp` is what `build.rs` has always called the mechanism
//! and it collides with nothing.

/// **WHICH CODE THIS IMAGE WAS BUILT FROM** — the abbreviated commit of the checkout this binary
/// was compiled in, or the word `unknown` where there was no git to ask.
///
/// # ⚠⚠⚠⚠⚠ Why a number the wire already carries could not answer this — register item 438
///
/// `sprag_rpc::WIRE_PROTOCOL` is a SHAPE, and it moves only when a shape moves. A fix that changes
/// what a run DOES — a new transition, a guard, a different word on a walk — earns no bump by that
/// pin's own list, so both ends agree across it and neither can tell that one of them predates the
/// fix. A daemon outlives its clients by design, so that skew is the ordinary state after a rebuild
/// rather than an exotic one, and it is invisible from either end.
///
/// Measured 2026-08-18, which is what this exists for: a loop's entire walk was produced by a
/// daemon built before two commits that changed the very edges the walk was being read for, and it
/// was indistinguishable from a walk that carried them. The only probe that answered was `grep`
/// over `/proc/<pid>/exe`.
///
/// ⚠⚠ **IT IS DELIBERATELY NOT A VERSION CHECK.** Nothing refuses a connection over it and nothing
/// should: a skew here is a fact a reader needs, not a shape neither end can parse. `WIRE_PROTOCOL`
/// owns refusal; this owns provenance, and conflating them would make every rebuild a forced
/// restart.
///
/// ⚠ **Every binary linking this crate is stamped with the SAME value** — this is the identity of
/// the image, not of a role — so a client and a daemon built from one `cargo build` agree, and one
/// built later does not. Since register item 1066 that sentence is true of the substrate too: this
/// crate takes no dependencies, so a crate that must say which build it is can link it without
/// linking anything else.
///
/// ⚠ **It is an identity and is compared for EQUALITY only.** Nothing here can say which of two
/// stamps is newer — `git` can, and a reader holding the word has what it takes to ask.
pub const BUILD: &str = env!("SPRAG_BUILD");
