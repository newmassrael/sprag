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
//! means "wrong target for writes, wrong data for reads" — so every way a scope can fail
//! here ([`ScopeError`]) refuses the request WHOLE. None of them falls back to the default.
//!
//! ## A NAME is an address, and a display client must not scope by one
//!
//! That failure has a second door, and R303 measured a live daemon walking through it: a name can
//! be RETIRED and RE-ISSUED. A client holding `work` across a `rename-session` is first refused
//! (its session is alive and it detaches from it), and then, once a new session takes the freed
//! name, silently SERVED — reading and typing into a session it never named. The scope was
//! well-formed and honoured at every step; the address had simply stopped meaning what the client
//! meant by it.
//!
//! So the grammar has a third arm ([`ScopeAsk::Attached`]): *the session this client is VIEWING*,
//! resolved through the attachment the daemon already maintains and already moves with a rename.
//! It is not a fallback for a name that failed — a fallback would never fire in the case above,
//! because the stale name RESOLVES — it is a different question, asked by the clients whose reads
//! were always about their own view. Naming a session stays exactly what it was, for every caller
//! that means another one: `-t`, `client/attach`, the CLI.

use std::fmt;
use std::sync::{Arc, Mutex};

use pinion_rpc::Request;
use sprag_rpc::{ScopeAsk, ScopeFault, WINDOW_PARAM};
use sprag_terminal::{Session, SessionId, SessionRegistry, WindowId, Workspace};

use crate::external::lock;
use crate::wire::{ATTACHED_PARAM, SESSION_PARAM};

/// Why a request's session scope could not be honored. The request is refused whole in
/// every case, and the registry is untouched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeError {
    /// The session param is present but is not a string (`{"session": 42}`).
    ///
    /// Its own variant rather than folding into [`Unknown`](Self::Unknown), because the two
    /// are different mistakes: a non-string is a client that does not know the ABI, an
    /// unknown name is one that does and named a session that has gone. The pre-R890.1 bug
    /// pinion records was precisely this corner going unnoticed — a malformed scope silently
    /// dropped, the request acting on the primary — so naming it is what keeps it visible.
    NotAString,
    /// The [`ATTACHED_PARAM`] flag is present and is not a boolean
    /// ([`ScopeFault::AttachedNotABool`]) — the same mistake as [`NotAString`](Self::NotAString),
    /// on the other key, refused for the same reason. `false` is not this: it is a well-typed
    /// "not by my attachment" and reads as an absent key.
    AttachedNotABool,
    /// Both scope keys ask at once ([`ScopeFault::TwoScopes`]).
    ///
    /// They are different questions — a NAME can address any session, an attachment addresses the
    /// one this client is viewing — and a request that asks both has no answer that is not a
    /// guess. Refused rather than resolved by precedence, because whichever key the daemon picked
    /// would be right half the time and silently wrong the rest.
    TwoScopes,
    /// The request asked for its client's ATTACHMENT ([`ScopeAsk::Attached`]) and that client is
    /// attached to nothing — it never sent `client/attach`, its connection never said hello, or
    /// the session it was viewing was DESTROYED (which releases the attachment, see
    /// [`AttachmentRegistry::session_ended`](crate::AttachmentRegistry::session_ended)).
    ///
    /// A client viewing nothing has nothing to read, so this is a refusal and not a fall-back to
    /// the default: a display client that silently started painting some other session would be
    /// the exact failure the attached scope exists to prevent. Its poll thread classifies the
    /// refusal the way it classifies an unknown name — detach, or switch under a
    /// `detach-on-destroy` switch policy — so the destroyed-session path behaves as it always did.
    NotAttached,
    /// The param is a well-formed name that no session carries.
    Unknown(String),
    /// [`WINDOW_PARAM`] is present and is not a string.
    WindowNotAString,
    /// The window param is a well-formed name that the SCOPED SESSION holds no window under.
    ///
    /// It carries both names because either can be the mistake and a caller cannot tell which from
    /// one of them: a window that has been renamed, and a window of a DIFFERENT session, are the
    /// two ways to get here and they have different remedies.
    UnknownWindow {
        /// The session that was scoped.
        session: String,
        /// The window name it holds none under.
        window: String,
    },
}

impl From<ScopeFault> for ScopeError {
    /// The grammar's faults, mapped onto this daemon's refusals. Exhaustive on purpose: a fault
    /// added to the shared grammar must be answered here rather than folded into a neighbour.
    fn from(fault: ScopeFault) -> Self {
        match fault {
            ScopeFault::NotAString => Self::NotAString,
            ScopeFault::AttachedNotABool => Self::AttachedNotABool,
            ScopeFault::TwoScopes => Self::TwoScopes,
            ScopeFault::WindowNotAString => Self::WindowNotAString,
        }
    }
}

impl ScopeError {
    /// Whether this refusal means *"the session this reader is scoped to is not there"* — as
    /// opposed to *"this request is malformed"*.
    ///
    /// The two are answered identically today (the request is refused whole either way, and no
    /// caller of this type may treat them otherwise), and they diverge in exactly one place: the
    /// dispatch door that serves a read whose subject is the REGISTRY on a scope that did not
    /// resolve (`crate::registry_scene`). That door exists for a client whose session was DESTROYED
    /// under it, which must still be able to read the session list to decide where to go. A client
    /// that sent `{"session": 42}` is not in that position: it does not know the ABI, and answering
    /// part of what it asked for would be the silent partial acceptance this module refuses
    /// everywhere else.
    ///
    /// [`UnknownWindow`](Self::UnknownWindow) is deliberately on the malformed side even though its
    /// SESSION resolved: the reader is not lost, it named a window that is not there, and nothing
    /// about that is helped by being handed the registry.
    ///
    /// Exhaustive on purpose — a refusal added later must decide which of these it is rather than
    /// inherit an answer.
    #[must_use]
    pub const fn is_session_gone(&self) -> bool {
        match self {
            Self::Unknown(_) | Self::NotAttached => true,
            Self::NotAString
            | Self::AttachedNotABool
            | Self::TwoScopes
            | Self::WindowNotAString
            | Self::UnknownWindow { .. } => false,
        }
    }
}

impl fmt::Display for ScopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAString => write!(f, "params.{SESSION_PARAM} must be a string"),
            Self::AttachedNotABool => write!(f, "params.{ATTACHED_PARAM} must be a boolean"),
            Self::TwoScopes => write!(
                f,
                "params.{SESSION_PARAM} and params.{ATTACHED_PARAM} name two different scopes; \
                 send one",
            ),
            Self::NotAttached => write!(
                f,
                "params.{ATTACHED_PARAM} asks for this client's session and it is attached to none",
            ),
            Self::Unknown(name) => write!(f, "no session named {name:?}"),
            Self::WindowNotAString => {
                write!(
                    f,
                    "params.{WINDOW_PARAM} must be a string (a window has a NAME, not a number)"
                )
            }
            Self::UnknownWindow { session, window } => {
                write!(f, "session {session:?} has no window named {window:?}")
            }
        }
    }
}

impl std::error::Error for ScopeError {}

/// The one session-and-window a request acts on: the session name, the NAME of the window the
/// request is about, and that window's pane pool.
///
/// Which window that is comes from the request: [`WINDOW_PARAM`] names one, and an absent key means
/// the session's CURRENT window — the only thing it could mean before R311 and still the default.
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
    /// The scoped session's IDENTITY, pinned beside its name at the same instant off the same
    /// [`Session`].
    ///
    /// It is here for one caller: the change funnel, which runs AFTER the dispatch and must observe
    /// the session's changes at the address the session HAS rather than the one the request
    /// carried. Those are the same string for every method but one — `rename_session` moves it —
    /// and observing the retired name there would read a session that no longer answers to it and
    /// record its every window as closed.
    ///
    /// A name is the ADDRESS and stays the thing a client sends; this is the identity behind it,
    /// and like [`SessionId`] itself it never reaches the wire.
    id: SessionId,
    window: String,
    /// The scoped WINDOW's identity, pinned beside its name off the same
    /// [`Window`](sprag_terminal::Window) — the session id's twin, one level down, and here for the
    /// same kind of caller.
    ///
    /// A window's NAME is what a request carries and what a person types, and `rename-window` moves
    /// it. This is what a fact about the window is KEYED by once the request has resolved: which
    /// clients are watching it ([`AttachmentRegistry::sizes_for`](crate::attach::AttachmentRegistry::sizes_for)),
    /// so an arbitration cannot come to fold in a client that is looking at a different window
    /// which happens to have taken this one's name.
    window_id: WindowId,
    /// The CONNECTION this request arrived on, when it had one — who is asking.
    ///
    /// A scope answers "what is this request about"; this is the other half, and it is here because
    /// since R346 one verb writes it back. `select-window` moves the SESSION's landing window and
    /// the ACTING CLIENT's own view, and the acting client is only knowable from the request. Every
    /// scope minted for a caller with no connection — the CLI's string entry, the in-process host,
    /// the daemon's own retile — carries [`None`] and moves nobody's view, which is the whole of
    /// the rule the owner chose: a caller with no seat sets where the next client LANDS.
    conn: Option<pinion_rpc::ConnId>,
    workspace: Arc<Mutex<Workspace>>,
}

impl SessionScope {
    /// Resolve `request`'s scope against `registry` — the ONE place the
    /// [`session`](crate::wire::SESSION_PARAM) param is read, so the extraction cannot drift
    /// between the sites that need it (pinion's own scar: a hand-rolled second copy of its
    /// window extraction that had to agree forever for its gate to hold).
    ///
    /// Absent → the default session. A string → that session. `{"attached": true}` → the session
    /// this connection's client is VIEWING. The grammar is [`ScopeAsk`], defined once for both
    /// ends of the wire; the param's own contract is on [`crate::wire::SESSION_PARAM`].
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
    /// does not exist yet. A client scoped to its ATTACHMENT meets the same signal by the other
    /// door — the kill releases the attachment, so its next request is
    /// [`NotAttached`](ScopeError::NotAttached) rather than [`Unknown`](ScopeError::Unknown) —
    /// which is why the two refusals are classified identically by a poll thread.
    ///
    /// ## The ATTACHED ask, and why the caller supplies it lazily
    ///
    /// [`ScopeAsk::Attached`] resolves through a DIFFERENT registry — the per-client attachment map
    /// — which this module deliberately does not hold: a scope is a fact about a request, and
    /// "which session is this client viewing" is a fact about a connection that only the dispatch
    /// owner knows the id of. So the caller passes `attached`, called AT MOST ONCE and only on that
    /// arm. Lazy for two reasons, both real:
    ///
    /// * **Cost.** Every request resolves a scope, including every keystroke. A caller that took
    ///   the attachment lock eagerly would pay for it on the whole hot path to serve one arm.
    /// * **Lock order.** `window::retile` takes attachments THEN the registry. This resolves the
    ///   ask, calls `attached` (which may take the attachment lock and releases it), and only then
    ///   takes the registry — the same order, and never nested, so no path here can order against
    ///   that one.
    ///
    /// A caller with no connection to speak of (the string entry, whose requests arrive with no
    /// frame) passes `|| None`, and an `attached` ask over it is refused as
    /// [`NotAttached`](ScopeError::NotAttached) — which is true: there is no client there to be
    /// attached.
    ///
    /// # Errors
    ///
    /// [`ScopeError::NotAString`] / [`ScopeError::AttachedNotABool`] / [`ScopeError::TwoScopes`]
    /// for a params object this grammar does not admit; [`ScopeError::NotAttached`] for an
    /// attached ask from a client viewing nothing; [`ScopeError::Unknown`] for a name no session
    /// carries. All refuse the request whole.
    pub fn resolve(
        registry: &Arc<Mutex<SessionRegistry>>,
        request: &Request,
        attached: impl FnOnce() -> Option<String>,
        viewing: impl FnOnce() -> Option<WindowId>,
    ) -> Result<Self, ScopeError> {
        // BOTH questions parsed BEFORE any lock: a malformed scope is refused without touching
        // either registry, and the attached lookup below happens with nothing held. The window is
        // read first so a request wrong in both ways is told about the key it is likelier to have
        // got wrong — the session key predates this grammar and every caller already sends it.
        let window = ScopeAsk::window(request.params.as_ref())?;
        // WHICH WINDOW THIS CLIENT IS ON, asked here and not in `narrowed`, because `narrowed` runs
        // under the registry lock and this reads the ATTACHMENT map: the order this daemon keeps is
        // attachments THEN registry, and calling it there would invert it on every request. Skipped
        // entirely when the request named a window, which is the only case where the answer cannot
        // be used.
        let seat = if window.is_some() { None } else { viewing() };
        let named = match ScopeAsk::parse(request.params.as_ref())? {
            ScopeAsk::Default => {
                let registry = lock(registry);
                return Self::narrowed(registry.default_session(), window.as_deref(), seat);
            }
            ScopeAsk::Named(name) => name,
            ScopeAsk::Attached => attached().ok_or(ScopeError::NotAttached)?,
        };
        let registry = lock(registry);
        let session = registry.session(&named).ok_or(ScopeError::Unknown(named))?;
        Self::narrowed(session, window.as_deref(), seat)
    }

    /// `session`'s scope, narrowed to the window it NAMES — or to its current one for [`None`].
    ///
    /// The one place the window ask becomes a window, so the three [`ScopeAsk`] arms cannot come to
    /// disagree about what narrowing means. It reads the name and the pool off the SAME
    /// [`Window`](sprag_terminal::Window), which is [`of_session`](Self::of_session)'s whole
    /// discipline applied to a window the request chose instead of the one the session is showing.
    fn narrowed(
        session: &Session,
        window: Option<&str>,
        seat: Option<WindowId>,
    ) -> Result<Self, ScopeError> {
        let Some(name) = window else {
            // NO WINDOW NAMED, so this request is about wherever its client is sitting — which
            // since R346 is a fact about the CLIENT and only falls back to the session's current
            // window for a caller that has no seat (the CLI, an agent, the in-process host).
            //
            // The id is checked against THIS session's windows rather than trusted: a client that
            // has just switched sessions carries the window it was last on, and a window of a
            // session this request is not about is not a narrowing, it is a wrong target.
            let held = seat.and_then(|id| {
                session
                    .windows()
                    .iter()
                    .find(|window| window.id() == id)
                    .map(|window| Self::at(session, window))
            });
            return Ok(held.unwrap_or_else(|| Self::of_session(session)));
        };
        let window = session
            .windows()
            .iter()
            .find(|window| window.name() == name)
            .ok_or_else(|| ScopeError::UnknownWindow {
                session: session.name().to_owned(),
                window: name.to_owned(),
            })?;
        Ok(Self::at(session, window))
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

    /// The scope of one NAMED window of `session`, or `None` for a name it holds no window under.
    ///
    /// For a caller walking a session's windows rather than resolving a request — the daemon's own
    /// re-tiling, which owes every window of a session a re-derivation when a client's lifecycle
    /// moved what any of them arbitrate from. Goes through the same window/pool pairing as every
    /// other scope, so it cannot describe a window it did not read.
    #[must_use]
    pub fn of_window(session: &Session, window: &str) -> Option<Self> {
        Self::narrowed(session, Some(window), None).ok()
    }

    /// The scope of the session NAMED, or `None` for a name this registry does not hold.
    ///
    /// For a caller that has a session name but no request to resolve one from — the daemon's own
    /// `window::retile`, reached from the client-lifecycle methods, which carry a
    /// connection rather than a `session` param. Goes through the same
    /// `of_session` as every other scope, so a scope minted here addresses the
    /// same window a client's would.
    #[must_use]
    pub fn of(registry: &SessionRegistry, name: &str) -> Option<Self> {
        registry.session(name).map(Self::of_session)
    }

    /// The scope of `session`'s CURRENT window — the ONE construction of a scope, reading the
    /// window's NAME and its pool off the SAME [`Window`](sprag_terminal::Window) so they cannot
    /// come apart. Both [`resolve`](Self::resolve) (a named session) and
    /// [`of_default`](Self::of_default) go through it.
    fn of_session(session: &Session) -> Self {
        Self::at(session, session.current_window())
    }

    /// `session` scoped to one of ITS OWN windows — the single construction, so a scope's name, its
    /// identity and its pool always come off the same [`Window`](sprag_terminal::Window).
    ///
    /// The caller owes the pairing: `window` must be a window of `session`. Every caller reads it
    /// out of that session's own list one line above.
    fn at(session: &Session, window: &sprag_terminal::Window) -> Self {
        Self {
            session: session.name().to_owned(),
            id: session.id(),
            window: window.name().to_owned(),
            window_id: window.id(),
            conn: None,
            workspace: Arc::clone(window.workspace()),
        }
    }

    /// This scope, stamped with the connection the request came in on — the ONE writer of
    /// [`conn`](Self::conn).
    #[must_use]
    pub fn from_conn(mut self, conn: pinion_rpc::ConnId) -> Self {
        self.conn = Some(conn);
        self
    }

    /// The connection this request arrived on, or [`None`] for a caller that has no seat.
    #[must_use]
    pub const fn conn(&self) -> Option<pinion_rpc::ConnId> {
        self.conn
    }

    /// The scoped session's name — how the control external addresses its window
    /// ([`SessionRegistry::window_mut`]).
    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }

    /// The scoped session's IDENTITY — what its name is only the address of, and what the change
    /// funnel resolves the CURRENT address from after a dispatch that may have moved it. See
    /// [`SessionScope::id`] for why it is pinned here.
    #[must_use]
    pub const fn id(&self) -> SessionId {
        self.id
    }

    /// The IDENTITY of the window this request resolved to — what a fact ABOUT that window is
    /// keyed by, where [`window`](Self::window) is what a request addresses it with.
    ///
    /// The one reader that matters is the arbitration: "how big is this window" folds the areas of
    /// the clients WATCHING it, and a client's view is recorded as an id for the reason every other
    /// identity in this tree is — a name can be retired and re-issued, and folding in a client that
    /// is looking at a different window which has since taken this one's name would size a window
    /// from somebody who cannot see it.
    #[must_use]
    pub const fn window_id(&self) -> WindowId {
        self.window_id
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
    use sprag_terminal::WindowBirth;

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

    /// Resolve `params` for a caller with NO attachment — the shape of every caller that names its
    /// session, so these read exactly as they did before an attached scope existed.
    fn resolve(
        reg: &Arc<Mutex<SessionRegistry>>,
        params: &str,
    ) -> Result<SessionScope, ScopeError> {
        SessionScope::resolve(reg, &request(params), || None, || None)
    }

    /// Resolve `params` for a caller whose client IS attached to `viewing`.
    fn resolve_attached(
        reg: &Arc<Mutex<SessionRegistry>>,
        params: &str,
        viewing: &str,
    ) -> Result<SessionScope, ScopeError> {
        SessionScope::resolve(reg, &request(params), || Some(viewing.to_owned()), || None)
    }

    #[test]
    fn an_absent_param_resolves_to_the_default_session() {
        let reg = registry();
        // Both shapes of "did not ask": no session key, and no params object at all.
        for params in [r#"{"path":""}"#, "null"] {
            let scope = resolve(&reg, params).expect("the default");
            assert_eq!(scope.session(), "0", "params: {params}");
        }
    }

    #[test]
    fn a_named_session_resolves_to_that_session_and_its_pool() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        let scope = resolve(&reg, r#"{"session":"work"}"#).expect("a real name resolves");
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
                    resolve(&reg, &format!(r#"{{"session":{bad}}}"#)),
                    Err(ScopeError::NotAString)
                ),
                "a {bad} scope must be refused, not silently aliased to the default",
            );
        }
    }

    /// A request can NARROW itself to one window of its scoped session, and the narrowing composes
    /// with every arm rather than replacing one — R311's whole seam in one test.
    ///
    /// The fixture is deliberately sitting on a DIFFERENT window from the one it asks for, so "the
    /// narrowing worked" and "the session happened to be showing that window" cannot be confused:
    /// `new_window` selects what it creates, so the current window is `win1` and the request names
    /// the boot window `0`.
    ///
    /// REVERT-PROOF: drop the `window` read from `resolve` and both narrowed rows answer `win1`;
    /// resolve the window against the CURRENT one rather than by name and the same two fail.
    #[test]
    fn a_request_can_narrow_itself_to_one_window_of_its_session() {
        let reg = registry();
        let default = lock(&reg).default_session().name().to_owned();
        lock(&reg)
            .new_window(&default, Some("win1"), WindowBirth::default())
            .unwrap();
        let pool_of = |name: &str| {
            let guard = lock(&reg);
            let session = guard.session(&default).expect("the session");
            let window = session
                .windows()
                .iter()
                .find(|window| window.name() == name)
                .expect("the window");
            Arc::as_ptr(window.workspace())
        };
        let (boot, current) = (pool_of("0"), pool_of("win1"));
        assert_ne!(
            boot, current,
            "the two windows really hold different pools, or nothing below discriminates",
        );

        // ABSENT ⇒ the CURRENT window, which is what every request meant before this key existed.
        let wide = resolve(&reg, r#"{"session":"0"}"#).expect("the session resolves");
        assert_eq!(wide.window(), "win1");
        assert_eq!(Arc::as_ptr(wide.workspace()), current);

        // NAMED ⇒ that window, name and pool read off the same `Window`.
        for params in [r#"{"session":"0","window":"0"}"#, r#"{"window":"0"}"#] {
            let narrow = resolve(&reg, params).expect("the window resolves");
            assert_eq!(narrow.window(), "0", "{params}");
            assert_eq!(
                Arc::as_ptr(narrow.workspace()),
                boot,
                "{params} carries the NAMED window's pool, not the current one",
            );
            assert_eq!(narrow.session(), "0", "and the session is unchanged");
        }
    }

    /// The two ways to get a window narrowing wrong, each with its own refusal — because the
    /// remedies differ and a caller cannot tell them apart from one sentence.
    ///
    /// A window NUMBER is the likeliest mistake (tmux windows have numbers and sprag's have names),
    /// which is why it is not folded into the session key's `NotAString`.
    #[test]
    fn a_window_narrowing_is_refused_by_name_and_by_type() {
        let reg = registry();
        assert!(
            matches!(
                resolve(&reg, r#"{"session":"0","window":"ghost"}"#),
                Err(ScopeError::UnknownWindow { session, window })
                    if session == "0" && window == "ghost"
            ),
            "an absent window is refused carrying BOTH names",
        );
        assert!(
            matches!(
                resolve(&reg, r#"{"session":"0","window":2}"#),
                Err(ScopeError::WindowNotAString),
            ),
            "a window NUMBER is refused as a type error, not as an unknown name",
        );
        // The SENTENCES, pinned: a refusal a caller reads is a claim about the daemon.
        assert_eq!(
            resolve(&reg, r#"{"session":"0","window":"ghost"}"#)
                .unwrap_err()
                .to_string(),
            r#"session "0" has no window named "ghost""#,
        );
        assert_eq!(
            resolve(&reg, r#"{"window":2}"#).unwrap_err().to_string(),
            "params.window must be a string (a window has a NAME, not a number)",
        );
        // CONTROL: the same requests without the bad key resolve, so the refusals are about the
        // WINDOW key and not about the shape of the request.
        assert!(resolve(&reg, r#"{"session":"0"}"#).is_ok());
    }

    #[test]
    fn an_unknown_session_name_is_refused_whole() {
        let reg = registry();
        assert!(
            matches!(
                resolve(&reg, r#"{"session":"ghost"}"#),
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
        lock(&reg)
            .new_window(&default, Some("win1"), WindowBirth::default())
            .unwrap();

        let scope = resolve(&reg, r#"{"path":""}"#).expect("the default");
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
        let scope0 = resolve(&reg, r#"{"path":""}"#).expect("the default");
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

    /// The whole point of the arm: the SAME request bytes resolve to whatever session the client is
    /// viewing, so a rename of that session is invisible to it.
    ///
    /// The fixture is chosen so the two things being told apart DISAGREE: the attachment names
    /// `work`, the default session is `0`, and each is asserted to carry its OWN pool — a check
    /// against the session name alone would pass for a resolve that quietly fell back to the
    /// default, since a name is just a string either way.
    #[test]
    fn an_attached_ask_resolves_to_the_session_that_client_is_viewing() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        let scope =
            resolve_attached(&reg, r#"{"attached":true}"#, "work").expect("the viewed session");
        assert_eq!(scope.session(), "work");
        let work_pool = lock(&reg).workspace_of("work").unwrap();
        assert!(
            Arc::ptr_eq(scope.workspace(), &work_pool),
            "and it carries the VIEWED session's pool, not the default's",
        );

        // The same request, from a client viewing the other session: the bytes did not move, the
        // answer did. This is what a name can never do.
        let other = resolve_attached(&reg, r#"{"attached":true}"#, "0").expect("the other view");
        assert_eq!(other.session(), "0");
        assert!(!Arc::ptr_eq(other.workspace(), &work_pool));
    }

    /// A rename is the case this arm exists for, driven end to end: the attachment moves (what
    /// `rename_session` does to the attachment registry), the request bytes do not, and the client
    /// lands on the SAME session under its new name — where the same client scoped by NAME is
    /// refused outright.
    #[test]
    fn a_renamed_session_still_resolves_for_the_client_attached_to_it() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let work_pool = lock(&reg).workspace_of("work").unwrap();
        lock(&reg).rename_session("work", "prod").unwrap();

        // The attachment followed the rename (R302), so the client's unchanged request follows too.
        let scope =
            resolve_attached(&reg, r#"{"attached":true}"#, "prod").expect("the moved session");
        assert_eq!(scope.session(), "prod");
        assert!(
            Arc::ptr_eq(scope.workspace(), &work_pool),
            "the SAME session, addressed by the attachment rather than by a name",
        );

        // The control that makes the claim mean something: a client still sending the old NAME is
        // refused — which is the detach measured at R303, and is why the arm above exists.
        assert!(
            matches!(
                resolve(&reg, r#"{"session":"work"}"#),
                Err(ScopeError::Unknown(name)) if name == "work"
            ),
            "the retired name resolves to nothing, so a name-scoped client is refused",
        );
    }

    /// A client attached to NOTHING is told so, and is never quietly served the default session —
    /// which for a display client would mean painting and typing into a session nobody put it on.
    #[test]
    fn an_attached_ask_from_a_client_viewing_nothing_is_refused() {
        let reg = registry();
        assert!(
            matches!(
                resolve(&reg, r#"{"attached":true}"#),
                Err(ScopeError::NotAttached)
            ),
            "no attachment is a refusal, not a fall-back to the default",
        );
    }

    /// The two keys are two different questions. Asking both is refused rather than resolved by
    /// precedence: whichever the daemon picked would be silently wrong the other half of the time.
    #[test]
    fn naming_a_session_and_asking_for_the_attachment_at_once_is_refused() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        assert!(
            matches!(
                resolve_attached(&reg, r#"{"session":"work","attached":true}"#, "0"),
                Err(ScopeError::TwoScopes)
            ),
            "two scopes in one request have no honest answer",
        );
    }

    /// `false` is a well-typed "not by my attachment" and reads as ABSENT; `null` and every other
    /// type is a client that does not know the ABI and is refused — the same rule the session key
    /// has carried since pinion's aliasing scar, applied to the key beside it.
    #[test]
    fn an_attached_flag_is_absent_when_false_and_refused_when_it_is_not_a_boolean() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        // false alone: the default session, exactly as omitting the key.
        let scope = resolve_attached(&reg, r#"{"attached":false}"#, "work").expect("the default");
        assert_eq!(
            scope.session(),
            "0",
            "an explicit no is the same question as not asking",
        );
        // ...and it does not poison a name sent beside it.
        let named = resolve_attached(&reg, r#"{"session":"work","attached":false}"#, "0")
            .expect("the named session");
        assert_eq!(named.session(), "work");

        for bad in ["null", "1", r#""true""#, "[]", "{}"] {
            assert!(
                matches!(
                    resolve_attached(&reg, &format!(r#"{{"attached":{bad}}}"#), "work"),
                    Err(ScopeError::AttachedNotABool)
                ),
                "an {bad} attachment flag must be refused, not read as a yes or as absent",
            );
        }
    }

    /// Every refusal says which one it is, in a sentence an operator reads off a socket. Pinned as
    /// bytes because these reach a user through `sprag`'s stderr and a client's log, and a
    /// refusal's WORDING is the only thing that distinguishes two failures with the same code.
    #[test]
    fn each_refusal_says_which_one_it_is() {
        for (error, sentence) in [
            (ScopeError::NotAString, "params.session must be a string"),
            (
                ScopeError::AttachedNotABool,
                "params.attached must be a boolean",
            ),
            (
                ScopeError::TwoScopes,
                "params.session and params.attached name two different scopes; send one",
            ),
            (
                ScopeError::NotAttached,
                "params.attached asks for this client's session and it is attached to none",
            ),
            (
                ScopeError::Unknown("ghost".to_owned()),
                r#"no session named "ghost""#,
            ),
        ] {
            assert_eq!(error.to_string(), sentence);
        }
    }
}
