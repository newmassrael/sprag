//! The SESSION a request acts on — resolving the out-of-band
//! [`session`](crate::wire::SESSION_PARAM) param to exactly one session, once per request.
//!
//! One daemon holds every session, so "which session is this about?" is a question every
//! request must answer. [`SessionScope`] is that answer, resolved at the door and threaded
//! down, and the reason it is a TYPE rather than a `&str` passed around is that it carries
//! proof: constructing one requires the session to have resolved, so nothing downstream has
//! to re-ask, re-check, or decide what to do when the answer is no.
//!
//! ## Resolve once — the POOL and the WINDOW IDENTITY together.
//!
//! The scene assembly and the mux control external both need the scope, and both used to
//! resolve "the current window" for themselves. Two independent resolutions of one question
//! is the second-authority pattern this arc keeps flagging: they agree only as long as
//! nothing moves between them. Resolving here, ONCE, closes that: this reads the scoped
//! session's current window — its NAME and its pool — under one registry lock, off the same
//! [`Window`](sprag_terminal::Window), so the two cannot describe different windows.
//!
//! The pool is an `Arc`, so it is CARRIED down. The window itself is an inline field of its
//! session (only its inner `Workspace` is `Arc`'d), so it cannot be — the layout paths
//! re-resolve it, but now BY THE NAME THIS SCOPE PINNED
//! ([`SessionRegistry::window_mut`], name-addressed on both dimensions), not as "whichever is
//! current at the moment of use". So a request acts on the window it was ASSEMBLED for even if
//! the current window moved between its resolve and its use — the window-switch bound this
//! module used to carry (a request reading one window's panes and reconciling them against
//! another's tree) is now closed BY CONSTRUCTION, not by the single-window / single-thread
//! accident it rested on before window-switching existed.
//!
//! ## The failure it exists to prevent
//!
//! A request scoped to `work` whose write lands in the default session is far worse than a
//! refused request: it is silent, and it corrupts a session the client never named. pinion
//! fought exactly this for its display windows and recorded the verdict — a dropped scope
//! means "wrong target for writes, wrong data for reads" — so both ways a scope can fail
//! here ([`ScopeError`]) refuse the request WHOLE. Neither falls back to the default.

use std::fmt;
use std::sync::{Arc, Mutex};

use pinion_rpc::Request;
use serde_json::Value;
use sprag_terminal::{Session, SessionRegistry, Workspace};

use crate::external::lock;
use crate::wire::SESSION_PARAM;

/// Why a request's session scope could not be honored. The request is refused whole in
/// either case, and the registry is untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    /// The param is present but is not a string (`{"session": 42}`).
    ///
    /// Its own variant rather than folding into [`Unknown`](Self::Unknown), because the two
    /// are different mistakes: a non-string is a client that does not know the ABI, an
    /// unknown name is one that does and named a session that has gone. The pre-R890.1 bug
    /// pinion records was precisely this corner going unnoticed — a malformed scope silently
    /// dropped, the request acting on the primary — so naming it is what keeps it visible.
    NotAString,
    /// The param is a well-formed name that no session carries.
    Unknown(String),
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAString => write!(f, "params.{SESSION_PARAM} must be a string"),
            Self::Unknown(name) => write!(f, "no session named {name:?}"),
        }
    }
}

impl std::error::Error for ScopeError {}

/// The one session-and-window a request acts on: the session name, the NAME of the window that
/// session is currently showing, and that window's pane pool.
///
/// The window name and the pool are read off the SAME [`Window`](sprag_terminal::Window) at
/// resolve time, so they cannot describe different windows — which is what lets the layout
/// paths re-resolve the window by [`window`](Self::window) rather than as "the current window",
/// and act on the one the request was assembled for (see the module docs).
///
/// Cheap to clone (two names and a handle), because the scene assembly and the control external
/// both hold the SAME resolved answer rather than each deriving one.
#[derive(Clone)]
pub struct SessionScope {
    session: String,
    window: String,
    workspace: Arc<Mutex<Workspace>>,
}

impl SessionScope {
    /// Resolve `request`'s scope against `registry` — the ONE place the
    /// [`session`](crate::wire::SESSION_PARAM) param is read, so the extraction cannot drift
    /// between the sites that need it (pinion's own scar: a hand-rolled second copy of its
    /// window extraction that had to agree forever for its gate to hold).
    ///
    /// Absent → the default session. A string → that session. The contract and its rationale
    /// are on [`crate::wire::SESSION_PARAM`].
    ///
    /// The workspace is resolved HERE, while the registry lock is already held and the
    /// session is known to exist, and travels with the name — so the assembly downstream
    /// needs no second lookup and has no absent case to invent an answer for.
    ///
    /// Scope resolution gates EVERY method, including the registry-WIDE ones
    /// (`new_session`, `sessions`, `kill_session`) — a scoped connection carries its `session` on
    /// all of them. So a client scoped to a session that is then KILLED gets a refusal on its
    /// very next request, including a would-be `sessions`/`new_session`. That is not a gap but the
    /// DETACH signal: the client's poll thread reads the refusal and ends the client (tmux's "the
    /// session is gone, so the client leaves" — see `sprag-gui`'s `detach_reason`), rather than
    /// lingering to list or re-create over a dead scope. A client creates its FIRST session on an
    /// UNSCOPED connection before scoping, so the create path is never gated on a session that
    /// does not exist yet.
    ///
    /// # Errors
    ///
    /// [`ScopeError::NotAString`] for a present-but-non-string param;
    /// [`ScopeError::Unknown`] for a name no session carries. Both refuse the request whole.
    pub fn resolve(
        registry: &Arc<Mutex<SessionRegistry>>,
        request: &Request,
    ) -> Result<Self, ScopeError> {
        let registry = lock(registry);
        let named = match request.params.as_ref().and_then(|p| p.get(SESSION_PARAM)) {
            None => return Ok(Self::of_default(&registry)),
            Some(Value::String(name)) => name.clone(),
            Some(_) => return Err(ScopeError::NotAString),
        };
        let session = registry.session(&named).ok_or(ScopeError::Unknown(named))?;
        Ok(Self::of_session(session))
    }

    /// The default session's scope, for a caller with no request to name one: the in-process
    /// [`Host`](crate::Host), which owns the boot panes and has no wire to carry a param.
    ///
    /// Total, and the same answer an absent param gets in [`resolve`](Self::resolve) — one
    /// definition of "the default", so the in-process arm and an unscoped wire client cannot
    /// come to act on different sessions.
    #[must_use]
    pub fn unscoped(registry: &Arc<Mutex<SessionRegistry>>) -> Self {
        Self::of_default(&lock(registry))
    }

    /// The default session's scope, read straight off the registry. Total without a fallible
    /// lookup: [`SessionRegistry::default_session`] hands back the session itself, so there is no
    /// name to fail to resolve.
    fn of_default(registry: &SessionRegistry) -> Self {
        Self::of_session(registry.default_session())
    }

    /// The scope of `session`'s CURRENT window — the ONE construction of a scope, reading the
    /// window's NAME and its pool off the SAME [`Window`](sprag_terminal::Window) so they cannot
    /// come apart. Both [`resolve`](Self::resolve) (a named session) and
    /// [`of_default`](Self::of_default) go through it.
    fn of_session(session: &Session) -> Self {
        let window = session.current_window();
        Self {
            session: session.name().to_owned(),
            window: window.name().to_owned(),
            workspace: Arc::clone(window.workspace()),
        }
    }

    /// The scoped session's name — how the control external addresses its window
    /// ([`SessionRegistry::window_mut`]).
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The NAME of the window the scoped session was showing when this request resolved — how
    /// the layout paths address the window ([`SessionRegistry::window_mut`]), so a request acts
    /// on the window it was ASSEMBLED for even if the current window has since moved.
    #[must_use]
    pub fn window(&self) -> &str {
        &self.window
    }

    /// The pane pool of the window the scoped session is showing — what the scene assembly
    /// builds pane children from and what the plugin host operates on.
    #[must_use]
    pub fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        &self.workspace
    }
}

impl fmt::Debug for SessionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionScope")
            .field("session", &self.session)
            .field("window", &self.window)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pinion_rpc::parse_request;

    fn registry() -> Arc<Mutex<SessionRegistry>> {
        Arc::new(Mutex::new(SessionRegistry::new((80, 24))))
    }

    /// A parsed request carrying `params` verbatim — built through pinion's own
    /// [`parse_request`], so these tests read the param off the same `Request` the live
    /// dispatch path hands to [`SessionScope::resolve`], not a hand-built stand-in.
    fn request(params: &str) -> Request {
        parse_request(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{params}}}"#
        ))
        .expect("a well-formed request")
    }

    #[test]
    fn an_absent_param_resolves_to_the_default_session() {
        let reg = registry();
        // Both shapes of "did not ask": no session key, and no params object at all.
        for params in [r#"{"path":""}"#, "null"] {
            let scope = SessionScope::resolve(&reg, &request(params)).expect("the default");
            assert_eq!(scope.session(), "0", "params: {params}");
        }
    }

    #[test]
    fn a_named_session_resolves_to_that_session_and_its_pool() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        let scope = SessionScope::resolve(&reg, &request(r#"{"session":"work"}"#))
            .expect("a real name resolves");
        assert_eq!(scope.session(), "work");
        // The pool that travels with the name is WORK's, not the default's — the whole
        // point of resolving both together. Compared by pointer: two sessions' pools are
        // distinct allocations, and an `is_empty()` check would pass for either.
        let work_pool = lock(&reg).workspace_of("work").unwrap();
        let default_pool = lock(&reg).workspace_of("0").unwrap();
        assert!(Arc::ptr_eq(scope.workspace(), &work_pool));
        assert!(!Arc::ptr_eq(scope.workspace(), &default_pool));
    }

    /// The corner pinion's own campaign missed for a whole round: a malformed scope must not
    /// fall through to the default. Every non-string JSON type, because "it must be a string"
    /// is the claim under test — not "it must not be 42".
    #[test]
    fn a_non_string_param_is_refused_and_never_falls_back_to_the_default() {
        let reg = registry();
        for bad in ["42", "true", "null", "[\"work\"]", "{\"name\":\"work\"}"] {
            // `matches!`, not `==`: a scope is not a comparable value (its pool is an
            // opaque handle), and pattern-matching the variant is exactly the claim — this
            // is refused, as `NotAString`.
            assert!(
                matches!(
                    SessionScope::resolve(&reg, &request(&format!(r#"{{"session":{bad}}}"#))),
                    Err(ScopeError::NotAString)
                ),
                "a {bad} scope must be refused, not silently aliased to the default",
            );
        }
    }

    #[test]
    fn an_unknown_session_name_is_refused_whole() {
        let reg = registry();
        assert!(
            matches!(
                SessionScope::resolve(&reg, &request(r#"{"session":"ghost"}"#)),
                Err(ScopeError::Unknown(name)) if name == "ghost"
            ),
            "a name no session carries is refused as Unknown, carrying the name asked for",
        );
    }

    /// The closure of the window-switch bound: the scope pins the session's CURRENT window —
    /// its NAME and its pool, read off the SAME window — so they cannot describe different
    /// windows, and a scope resolved after a switch names the NEW current window and carries
    /// ITS pool, never a mix of the two.
    #[test]
    fn the_scope_pins_the_current_windows_name_and_its_pool_together() {
        let reg = registry();
        let default = lock(&reg).default_session().name().to_owned();
        // A second window, which `new_window` also makes current.
        lock(&reg).new_window(&default, Some("win1")).unwrap();

        let scope = SessionScope::resolve(&reg, &request(r#"{"path":""}"#)).expect("the default");
        assert_eq!(scope.session(), default);
        assert_eq!(scope.window(), "win1", "the scope names the current window");
        let win1_pool = lock(&reg).workspace_of(&default).unwrap();
        assert!(
            Arc::ptr_eq(scope.workspace(), &win1_pool),
            "and carries THAT window's pool, off the same Window",
        );

        // Switch the current window back to "0": a fresh scope names "0" and carries "0"'s pool,
        // not the window it was switched away from.
        lock(&reg).select_window(&default, "0").unwrap();
        let scope0 = SessionScope::resolve(&reg, &request(r#"{"path":""}"#)).expect("the default");
        assert_eq!(scope0.window(), "0");
        let pool0 = lock(&reg).workspace_of(&default).unwrap();
        assert!(
            Arc::ptr_eq(scope0.workspace(), &pool0),
            "name and pool both followed"
        );
        assert!(
            !Arc::ptr_eq(scope0.workspace(), &win1_pool),
            "the pool is the current window's, never the switched-away one's",
        );
    }
}
