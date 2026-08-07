//! The multiplexer control surface — pane + layout management as a pinion `External`.
//!
//! The multiplexer's state (a [`SessionRegistry`] of sessions → windows → pane pools, a
//! producer-layer concern in sprag-terminal) is exposed to AI peers through one engine
//! `External`: the mux control plane. It generalizes the R6 input pattern — producer
//! mutations ride pinion's canonical `scene/invoke` against a producer-owned handler,
//! never new RPC methods (pinion RPC vocabulary stays SSOT).
//!
//! Action channel (`scene/invoke`):
//!
//! * `spawn {cmd?:[..], cols?, rows?}` → spawns a pane, returns its id.
//! * `close {id}` → reaps a pane.
//! * `resize {id, cols, rows}` → resizes a pane's PTY + emulator.
//! * `set_layout {tree}` → installs a client's settled arrangement, returns the canonical one.
//! * `set_floating {id, floating}` → takes a pane out of the tiling / puts it back.
//! * `new_session {name?, cmd?, cols?, rows?, remote?}` → creates a session BORN with one pane (absent
//!   name → lowest free; `cmd`/`cols`/`rows` shape the birth pane), returns its name.
//! * `kill_session {name}` → kills a session; the last one ends the daemon (tmux kill-session).
//!
//! Read channel (`scene/query`):
//!
//! * `panes` → the live pane list as JSON.
//! * `layout` → the scoped session's current-window LOGICAL arrangement + its revision.
//! * `sessions` → every session, and which one an unscoped request lands in.
//!
//! ## Why this surface holds the REGISTRY (and the plugin host does not)
//!
//! This is the multiplexer's own control plane, and sessions / windows / layout ARE mux
//! concerns — so it holds the `Arc<Mutex<SessionRegistry>>`. The PLUGIN host
//! ([`crate::PluginsExternal`]) deliberately still takes only `Arc<Mutex<Workspace>>`: a
//! plugin operates on a pane pool and has no business knowing about the session tree
//! (Interface Segregation).
//!
//! ## Why it is the one surface carrying a scope
//!
//! Holding the registry is holding EVERY session, so this surface alone can act outside the
//! one the request named — which is why it is handed a [`SessionScope`] and every action but
//! the registry-wide session ones goes through it. The rest of the scene needs no such care: a
//! pane child and the plugin host are built from the scoped pool and can address nothing
//! else. The privilege is what creates the obligation.
//!
//! Three members are deliberately registry-WIDE rather than scoped, and for the same reason:
//! their subject is the set of sessions itself, so answering them within one session would
//! answer a question nobody asked. `sessions` enumerates the scopes a client may name;
//! `new_session` makes one and `kill_session` removes one — each NAMES its session directly
//! rather than acting on the request's scope, and none can silently disturb another client.
//!
//! Still no PIXEL division here — headless multiplexing is pane control, not screen
//! division (the Round 7 note). The `layout` slot is the LOGICAL arrangement (which
//! panes are split, in what order, at what proportion), which is session state a
//! detached client must not take with it; rects stay a rendering concern of whichever
//! client projects it (see [`sprag_terminal::layout`]).

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pinion_core::external::{
    ExternalIntrospect, InterveneError, IntrospectSchema, IntrospectValue, InvokeError, RawJson,
};
use serde_json::{Map, Value, json};
use sprag_terminal::{
    CommandBuilder, KillOutcome, LayoutSnapshot, LayoutWire, Pane, PaneId, PaneKillOutcome,
    SessionInfo, SessionRegistry, SplitDir, SplitSide, SshRemote, WindowBirth, WindowKillOutcome,
    Workspace,
};

use crate::attach::ClientSize;
use crate::bump_on_dirty;
use crate::external::{as_object, lock, opt_dim, refused, require_pane_id, rpc_external_impl};
use crate::notify::ChannelRegistry;
use crate::scope::SessionScope;
use crate::window::{SizeRequest, WindowSize};

// The mux control action names + query slots are the shared wire ABI vocabulary
// ([`crate::wire`]) — the SAME consts a client addresses for pane lifecycle.
use crate::wire::{
    AGENT_MANIFESTS_SLOT, ActivityWire, BREAK_PANE_ACTION, CLIENTS_SLOT, CLOSE_ACTION,
    DETACHED_KEY, DISPLAY_MESSAGE_ACTION, DROP_FILE_ACTION, EVENTS_FIELD, GLOBAL_COMMANDS_SLOT,
    GRID_WORK_SLOT, JOIN_PANE_ACTION, JoinAsk, KILL_SESSION_ACTION, KILL_WINDOW_ACTION,
    LAYOUT_SLOT, MOVE_PANE_ACTION, MOVE_WINDOW_ACTION, MoveWindowAsk, NEIGHBORS_FIELD,
    NEW_SESSION_ACTION, NEW_WINDOW_ACTION, PANE_PROCESSES_FIELD, PANES_SLOT, PaneProcessesWire,
    RELEASE_AGENT_ACTION, RENAME_PANE_ACTION, RENAME_SESSION_ACTION, RENAME_WINDOW_ACTION,
    REPORT_AGENT_ACTION, RESIZE_ACTION, RESIZE_PANE_ACTION, RESIZE_WINDOW_ACTION, ResizeAsk,
    SELECT_PANE_ACTION, SELECT_WINDOW_ACTION, SESSION_ACTIVITY_FIELD, SESSION_SLOT, SESSIONS_SLOT,
    SET_FLOATING_ACTION, SET_LAYOUT_ACTION, SPAWN_ACTION, SPLIT_ACTION, SWAP_PANE_ACTION,
    SelectWindowAsk, SwapAsk, TREE_SLOT, WINDOW_SIZE_SLOT, WINDOWS_SLOT, WindowRef,
    ZOOM_PANE_ACTION,
};

/// The refusal every agent-report verb shares: this host runs no agent detector, so there is no
/// memory to report into and none to release from.
///
/// A `const` because TWO actions state it and the rule this crate keeps is that a sentence written
/// twice is a sentence that drifts. It is a fact about the HOST rather than about the request, which
/// is why it needs no interpolation.
const NO_DETECTOR: &str = "this daemon installs no agent detector";

/// The refusal every client-addressed verb shares: this host holds no attachment map, so there is
/// nobody to address.
///
/// [`NO_DETECTOR`]'s twin one surface over — an in-process host (a GUI's own, a unit test) has no
/// wire clients, and answering "delivered" there would report a sentence shown to somebody who does
/// not exist.
const NO_CLIENTS: &str = "this daemon serves no attached clients";

/// The refusal both arrangement writes share when the tree they produced will not serialise.
///
/// A daemon-side fault rather than a caller's mistake, and it says so: nothing the caller can send
/// differently would help. Stated instead of hidden because the alternative — the payload-free
/// `Rejected` these two used to answer — put it in the same bucket as a stale revision, which the
/// caller CAN act on.
const UNRENDERABLE_LAYOUT: &str = "this daemon could not render the resulting arrangement";

/// Everything a read whose SUBJECT is the registry needs — and, decisively, nothing a read about
/// ONE session needs.
///
/// # The distinction this type exists to make unrepresentable
///
/// Half the mux surface's slots answer about the session the request is scoped to (`panes`,
/// `layout`, `windows`, …) and half answer about the set of sessions, or about the daemon itself
/// (`sessions`, `tree`, `clients`, …). Until R327 that split lived only in the prose beside each
/// arm, and one consequence was measured: a client whose own session had just been destroyed could
/// not re-read the SESSION LIST, because scope resolution gates every method and its scope no
/// longer resolved. A `detach-on-destroy` policy decides where to land by reading exactly that
/// list, so `no-detached` — tmux's *"switch, but never onto a session somebody else is in"* — was
/// deciding on a mirror nothing bounds the staleness of, and walked into an occupied session.
///
/// So the split is a TYPE. This view holds no [`SessionScope`], which is what makes the guarantee
/// structural rather than careful: an arm here **cannot** read the scope, so a read served through
/// it cannot be about a session — least of all about the wrong one. That is why the dead-scope
/// door ([`RegistryExternal`]) can serve a request whose scope was refused without re-deciding, per
/// slot, whether doing so is safe.
///
/// Borrowed rather than owned, and built per query: both surfaces hand it the handles they already
/// hold, so there is exactly ONE spelling of each registry-subject answer and no second copy to
/// drift ([`RegistryView::query`] is the only place any of them is produced).
pub(crate) struct RegistryView<'a> {
    registry: &'a Arc<Mutex<SessionRegistry>>,
    attachments: Option<&'a Mutex<crate::AttachmentRegistry>>,
    agents: Option<&'a crate::AgentClock>,
    samplers: &'a crate::Samplers,
}

impl RegistryView<'_> {
    /// The answer to a query whose subject is the REGISTRY, or [`None`] for every other address —
    /// including the addresses this daemon does serve about ONE session, which have no answer that
    /// does not name a scope.
    ///
    /// `None` is what makes this total without lying: [`WorkspaceExternal::query`] falls through to
    /// its own scoped arms, and the dead-scope door turns it back into the scope refusal the reader
    /// had coming. Neither caller has to know which addresses are in here.
    fn query(&self, path: &str) -> Option<IntrospectValue> {
        // The parametric families go FIRST, before the exact-path arms: their argument rides the
        // path, so they are matched by prefix rather than by equality (`cells.<offset>`'s shape, and
        // the reason a malformed member answers `Null` rather than `None` — `session_activity.zzz`
        // IS in this surface's schema, so denying the address exists would be the wrong refusal).
        if let Some(arg) = path.strip_prefix(SESSION_ACTIVITY_FIELD.literal_prefix()) {
            return Some(arg.parse::<u64>().map_or(IntrospectValue::Null, |max_age| {
                let reading = self
                    .samplers
                    .activity
                    .read(self.registry, Duration::from_millis(max_age));
                encoded_answer(&ActivityWire::from(reading), "session activity")
                    .unwrap_or(IntrospectValue::Null)
            }));
        }
        if let Some(arg) = path.strip_prefix(PANE_PROCESSES_FIELD.literal_prefix()) {
            return Some(arg.parse::<u64>().map_or(IntrospectValue::Null, |max_age| {
                let reading = self
                    .samplers
                    .processes
                    .read(self.registry, Duration::from_millis(max_age));
                encoded_answer(&PaneProcessesWire::from(reading), "pane processes")
                    .unwrap_or(IntrospectValue::Null)
            }));
        }
        match path {
            // Every session, plus which one an unscoped request lands in — how a client
            // discovers what it may name in `session`. Registry-WIDE by design: this is the
            // one slot whose subject is the set of scopes, so scoping it to the caller's own
            // session would answer a question nobody asked.
            //
            // ONE builder ([`crate::host::listable_sessions`]) shared with the in-process arm and
            // with `switch-client`'s ring walk, serialised here the way `windows` serialises its
            // `WindowInfo`s — so neither the shape, nor what `windows`/`default` mean, nor WHICH
            // SESSIONS APPEAR can drift between the three. It fills the per-session attached count
            // (dispatch-layer state the registry cannot know) and drops the resting empty anchor.
            // `default` says where an UNSCOPED request lands — not "is it current", nothing is
            // current here.
            SESSIONS_SLOT => {
                let infos: Vec<SessionInfo> =
                    crate::host::listable_sessions(self.registry, self.attachments);
                encoded_answer(&infos, "sessions")
            }
            // The same sessions, DESCENDING: every window and every pane, each carrying the identity
            // a chooser commits by (R315). Registry-WIDE for `sessions`' reason, and a SECOND slot
            // rather than a wider first one because that one is polled and this one is pressed —
            // see `TREE_SLOT`. It shares that slot's listability rule through one predicate, so a
            // chooser cannot offer a session `sprag ls` denies exists.
            TREE_SLOT => {
                let tree: Vec<sprag_terminal::TreeSession> =
                    crate::host::listable_tree(self.registry, self.attachments);
                encoded_answer(&tree, "tree")
            }
            // Every currently-attached client and the session it views — tmux `list-clients`.
            // Registry-WIDE like `sessions` (its subject is the set of clients), and filled from
            // the SAME dispatch-layer attachment map that fills each session's `attached` count.
            // `None` off a daemon (no wire clients) serialises to an empty list — an honest "no
            // clients", the same additive story as an unattached session's absent `attached`.
            CLIENTS_SLOT => {
                let clients = match self.attachments {
                    Some(attachments) => lock(attachments).clients(),
                    None => Vec::new(),
                };
                encoded_answer(&clients, "clients")
            }
            // What this host has paid to project its cells. Read straight off the meter rather
            // than recomputed, and UNSCOPED on purpose: the counters are process-wide, so scoping
            // them to the request's session would name a session for work every session shares.
            // Serialised by hand rather than through the type, so the wire keys are spelled once
            // in the place the schema declares them.
            GRID_WORK_SLOT => {
                let work = sprag_grid::work();
                Some(IntrospectValue::Json(serde_json::json!({
                    "projections_total": work.projections_total,
                    "cells_total": work.cells_total,
                })))
            }
            // The USER's own declared commands — no pane, no session, no scope: this answer is the
            // same for every request the host serves, which is exactly why it is a fixed slot beside
            // the parametric project one rather than a variant of it.
            GLOBAL_COMMANDS_SLOT => Some(global_commands_value()),
            // Why the agent manifests in force are not the user's. Unscoped like the one above and
            // for a stronger reason: the ruleset is the DAEMON's, one list for every session it
            // serves, so scoping this answer would name a session for a fact no session owns.
            AGENT_MANIFESTS_SLOT => Some(agent_manifests_value(self.agents)),
            _ => None,
        }
    }
}

/// The mux control surface a reader whose OWN SESSION HAS GONE meets: the reads whose subject is
/// the REGISTRY, and nothing else.
///
/// A client scoped to a destroyed session is refused every method, and that refusal is load-bearing
/// — it is the DETACH signal a display client's poll thread runs on. What R326 measured is that it
/// was also refusing the reads that need no session at all, and the `detach-on-destroy` policies
/// decide by making exactly one of those reads. So the refusal keeps its whole meaning for anything
/// ABOUT a session, and the reads whose subject is the registry are served from here instead.
///
/// **It holds no [`SessionScope`], and that is the design.** A door that carried one "just for the
/// scene" would be one misclassified slot away from serving a client the DEFAULT session's panes
/// under a scope it never named — pinion's "wrong target for writes, wrong data for reads", which
/// [`crate::scope`] exists to prevent. Here there is no session to be wrong about: the type cannot
/// express one.
///
/// Writes are absent for the same reason and a second one: an act needs a session, and a client
/// whose session died has none to act on. [`ExternalIntrospect::invoke`] refuses every address.
pub struct RegistryExternal {
    registry: Arc<Mutex<SessionRegistry>>,
    attachments: Option<Arc<Mutex<crate::AttachmentRegistry>>>,
    agents: Option<Arc<crate::AgentClock>>,
    samplers: crate::Samplers,
}

impl RegistryExternal {
    /// Build the dead-scope control surface from the daemon's shared state — the same handles
    /// [`WorkspaceExternal`] is given, minus everything that names a session.
    #[must_use]
    pub fn new(registry: Arc<Mutex<SessionRegistry>>, daemon: crate::DaemonShared) -> Self {
        let crate::DaemonShared {
            attachments,
            agents,
            samplers,
            ..
        } = daemon;
        Self {
            registry,
            attachments,
            agents,
            samplers,
        }
    }
}

impl fmt::Debug for RegistryExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegistryExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(RegistryExternal);

impl ExternalIntrospect for RegistryExternal {
    /// The MUX surface's schema, whole — not a narrowed copy listing only what this door answers.
    ///
    /// The schema describes the addresses this DAEMON serves, and that set does not shrink because
    /// one reader's session died. A second, shorter list would be a claim that the product's
    /// vocabulary depends on who is asking, and it would be a second copy of the declaration
    /// [`crate::wire::MUX_SCHEMA`] exists to be the only one of.
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(crate::wire::MUX_SCHEMA)
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        RegistryView {
            registry: &self.registry,
            attachments: self.attachments.as_deref(),
            agents: self.agents.as_deref(),
            samplers: &self.samplers,
        }
        .query(path)
    }

    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        Err(InterveneError::UnknownPath)
    }

    /// Every action, refused. A client reaching here has no session to act on, and the scope
    /// refusal it gets instead says exactly that — see the type docs.
    fn invoke(
        &mut self,
        _path: &str,
        _args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        Err(InvokeError::UnknownPath)
    }
}

/// The mux-management engine `External`: a control surface over the shared
/// [`SessionRegistry`]. Holds `Arc<Mutex<SessionRegistry>>` so its `scene/invoke`
/// handlers mutate the live pane pool of the CURRENT window (which the serve loop also
/// reads to assemble the scene) and its `layout` slot can serve that window's
/// arrangement, plus the per-session change channels ([`ChannelRegistry`]) so a pane-lifecycle
/// mutation wakes the `scene/waitFor` replies parked on the session it happened in.
pub struct WorkspaceExternal {
    registry: Arc<Mutex<SessionRegistry>>,
    /// The session this surface may act on — the request's own, resolved once at the door.
    ///
    /// **The one child of the scene that needs telling.** Every other surface is built from
    /// the scoped pool and so cannot reach another session; this one holds the REGISTRY
    /// (sessions / windows / layout are mux concerns), which is every session. Without the
    /// scope it would read the arrangement of the session it was assembled for and write to
    /// whichever one happened to be the default — the silent cross-session write, which is
    /// the exact failure the scope param exists to prevent.
    scope: SessionScope,
    /// The per-session change channels ([`crate::HostState`]'s). Two roles: each pane this surface
    /// SPAWNS is wired with a `bump_on_dirty` hook over ITS SESSION's token (so its output wakes
    /// the waits on that session, like the boot pane), and a spawn / close announces on that
    /// session directly (so a pane-set change wakes a waiter before the new pane's first output).
    /// The whole registry is cloned per scene-assembly rather than one session's token, because
    /// two actions announce somewhere other than this request's scope — a `new_session` births a
    /// pane in a session that did not exist when the scope was resolved, and a `kill_session`
    /// closes one.
    channels: Arc<ChannelRegistry>,
    /// The self-cleaning daemon's pane-`on_exit` death-signal hook ([`crate::spawn_reaper`]),
    /// or `None` off a daemon (a GUI's in-process host, the unit tests). Wired into each pane
    /// this surface SPAWNS so its death feeds the reaper. Injected (not named here) so this
    /// library never decides process lifetime, and so a test spawns and reaps panes through
    /// this exact surface without ending its own process. It is registry-FREE (just a channel
    /// send), so it is safe to run from any thread.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The daemon's per-client attachment map ([`crate::AttachmentRegistry`]), or `None` off a
    /// daemon (a GUI's in-process host, the unit tests — nothing attaches to those over a wire).
    /// Read ONLY to fill each [`SessionInfo::attached`](sprag_terminal::SessionInfo::attached)
    /// when the `sessions` slot is served; this surface never mutates it (the dispatch layer owns
    /// the writes, off the frame's connection id, which no external sees). `None` leaves every
    /// `attached` at 0 — an honest "no wire clients here".
    attachments: Option<Arc<Mutex<crate::AttachmentRegistry>>>,
    /// The daemon's attention ROUTER ([`crate::attention::AttentionRouter`]), or `None` off a daemon.
    /// Each pane this surface SPAWNS asks it for a hook, so a child raising a notification or a bell
    /// reaches the people looking at the session that holds it — the same injection and the same
    /// per-birth wiring [`Self::on_pane_exit`] gets, and for the same reason: this library never
    /// decides who hears about a pane, it only makes sure the pane can be heard.
    attention: Option<Arc<crate::attention::AttentionRouter>>,
    /// The daemon's agent-state memory ([`crate::AgentRegistry`]), or `None` off a daemon — the same
    /// injection [`Self::attachments`] gets, for the same two reasons.
    ///
    /// It cannot be a plain field on this struct: a `WorkspaceExternal` is rebuilt for every
    /// JSON-RPC request, so a map owned here would be born empty per poll and the hysteresis would
    /// be silently inert while every individual verdict still looked right. It is `Arc<Mutex<_>>`
    /// because the memory outlives the request that reads it, and it is `Option` because a `None`
    /// leaves the `agent` key ABSENT — which is exactly what D8 says a pane with no agent looks
    /// like, so an in-process host without a detector serves the pre-H3 wire shape rather than a
    /// wrong answer.
    ///
    /// The lock is taken INSIDE the workspace lock (the screen is only reachable there) and never
    /// the other way round.
    agents: Option<Arc<crate::AgentClock>>,
    /// The HOST's [samplers](crate::Samplers), read when the `session_activity` (R282) or
    /// `pane_processes` (R290) slot is served.
    ///
    /// Like [`Self::agents`] they cannot be plain values on this struct — a `WorkspaceExternal` is
    /// rebuilt for every JSON-RPC request, so a sampler owned here would hold nothing at the moment
    /// it was asked and every request would take its own `/proc` walk, which is exactly the cost the
    /// split was made to remove. Unlike `agents` this is not `Option`: there is no host that cannot
    /// answer where its sessions are working or what its panes are running.
    samplers: crate::Samplers,
}

/// A VALIDATED pane-spawn request — the command, its label, and any explicit dims. Produced by
/// [`WorkspaceExternal::parse_spawn`] from the wire args, so a MALFORMED request is rejected
/// (`TypeMismatch`) before any pane — or session — is built. `cols`/`rows` left `None` take the
/// target pool's default size at spawn.
struct SpawnSpec {
    command: CommandBuilder,
    label: String,
    cols: Option<u16>,
    rows: Option<u16>,
    /// The structured remote endpoint for a `sprag ssh` birth pane, stamped onto the pane after the
    /// spawn so the host can reconnect it on restore and `scp` to it. `None` for an ordinary spawn.
    remote: Option<SshRemote>,
    /// The directory the child starts in, `None` to inherit the DAEMON's — which is where every
    /// pane started before this existed, and where a person's split still starts.
    ///
    /// Validated as an existing directory when the request is parsed rather than left to the exec:
    /// a `posix_spawn` into a directory that is not there does not fail loudly on this path, it
    /// produces a pane whose child died, and on screen that is indistinguishable from a shell that
    /// exited for no reason.
    cwd: Option<PathBuf>,
}

impl WorkspaceExternal {
    /// Build the control surface over the shared mux registry, the session it is scoped to,
    /// the shared scene-version token, and the daemon's `on_pane_exit` death-signal (`None` off
    /// a daemon) — see the struct docs for each field's role.
    #[must_use]
    pub fn new(
        registry: Arc<Mutex<SessionRegistry>>,
        scope: SessionScope,
        channels: Arc<ChannelRegistry>,
        daemon: crate::DaemonShared,
    ) -> Self {
        let crate::DaemonShared {
            on_pane_exit,
            attachments,
            attention,
            agents,
            samplers,
        } = daemon;
        Self {
            registry,
            scope,
            channels,
            on_pane_exit,
            attachments,
            attention,
            agents,
            samplers,
        }
    }

    /// This surface's registry-subject half, borrowed — the reads that would still be answerable if
    /// the session this surface is scoped to were destroyed mid-request.
    ///
    /// Borrowed and built per query rather than stored: a [`RegistryView`] is four references, and
    /// keeping one as a field beside the handles it points at would be two owners of one fact. The
    /// point of routing through it at all is that [`RegistryExternal`] serves the identical answers
    /// from the identical code.
    fn registry_view(&self) -> RegistryView<'_> {
        RegistryView {
            registry: &self.registry,
            attachments: self.attachments.as_deref(),
            agents: self.agents.as_deref(),
            samplers: &self.samplers,
        }
    }

    /// The scoped session's current-window pane pool — resolved when the scope was, so a
    /// spawn lands in the session the request named and nowhere else. No registry lock is
    /// taken to reach it, so it cannot nest with the workspace lock.
    fn workspace(&self) -> &Arc<Mutex<Workspace>> {
        self.scope.workspace()
    }

    /// Announce that the SCOPED session changed: advance its token, waking the `scene/waitFor`
    /// replies parked on that session and no others.
    ///
    /// Every action here acts on the session the request named, so the scope IS the answer to
    /// "whose scene moved" — which is why this takes no argument. The two places that announce
    /// somewhere ELSE ([`new_session`](Self::new_session), which births a pane in a session that
    /// did not exist when this scope was resolved, and [`kill_session`](Self::kill_session), which
    /// ends one) say so explicitly rather than through this.
    fn announce(&self) {
        self.channels.bump(self.scope.session());
    }

    /// Parse + VALIDATE the `{cmd?, cols?, rows?}` spawn spec — the REQUEST-validation half of a
    /// spawn, pool-free so it runs before anything is built. A malformed field (`cmd` present but
    /// not an array, a non-`u16` `cols`/`rows`) is a `TypeMismatch` HERE, the same refusal the
    /// `spawn` action and `new_session`'s own `name` field give a type error — so a `new_session`
    /// can reject a malformed birth spec before it creates the session. `cmd` (an argv array)
    /// defaults to the user's [`default-command`](crate::options::DEFAULT_COMMAND), then to `$SHELL`
    /// ([`default_pane_command`](crate::config::default_pane_command)); `cols`/`rows` left `None` take
    /// the pool's default size at spawn.
    fn parse_spawn(map: &Map<String, Value>) -> Result<SpawnSpec, InvokeError> {
        let (command, label) = match map.get("cmd") {
            // The user's `default-command`, falling through to `$SHELL` — one resolver for every
            // birth, so a setting cannot be honoured by some spawn paths and not others.
            None => crate::config::default_pane_command(),
            Some(Value::Array(argv)) => build_command(argv)?,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        Ok(SpawnSpec {
            command,
            label,
            cols: opt_dim(map, "cols")?,
            rows: opt_dim(map, "rows")?,
            remote: Self::parse_remote(map)?,
            cwd: Self::parse_cwd(map)?,
        })
    }

    /// Parse the OPTIONAL `cwd` — the directory the newborn child starts in. Absent (or `null`) is
    /// `None`, the daemon's own directory.
    ///
    /// A non-string is a `TypeMismatch` (a malformed request); a string that does not name an
    /// existing DIRECTORY is `Rejected` (a well-formed request the host cannot honour), which is the
    /// same split every other argument here keeps. The stat happens with no lock held and before
    /// anything is built, so the refusal costs no pane.
    ///
    /// This is a birth fact, so it lives on [`SpawnSpec`] and every action that builds one gets it
    /// — see [`crate::wire::SPAWN_ACTION`], which is where the vocabulary is written down.
    fn parse_cwd(map: &Map<String, Value>) -> Result<Option<PathBuf>, InvokeError> {
        let dir = match map.get("cwd") {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(dir)) => PathBuf::from(dir),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        if !dir.is_dir() {
            return Err(InvokeError::rejected(format!(
                "{} is not a directory",
                dir.display()
            )));
        }
        Ok(Some(dir))
    }

    /// Parse the OPTIONAL `opened_by` — the pane whose occupant is asking for this one
    /// ([`sprag_terminal::Pane::opened_by`]). Absent is a pane nobody claims.
    ///
    /// A pane this DAEMON does not hold is `Rejected`, on
    /// [`report_agent`](Self::report_agent)'s stated reason: a caller with a stale `SPRAG_PANE` —
    /// a process that outlived its own pane — would otherwise stamp a provenance naming a pane that
    /// does not exist, and nothing would ever prune it. Checked daemon-wide rather than against this
    /// request's scope for [`holds_pane`](Self::holds_pane)'s reason: pane ids are registry-unique,
    /// and an asking pane may legitimately sit in a session other than the connection's default.
    fn parse_opener(&self, map: &Map<String, Value>) -> Result<Option<PaneId>, InvokeError> {
        let opener = match map.get("opened_by") {
            None | Some(Value::Null) => return Ok(None),
            Some(value) => PaneId(value.as_u64().ok_or(InvokeError::TypeMismatch)?),
        };
        if !self.holds_pane(opener) {
            return Err(InvokeError::rejected(format!(
                "no pane {} on this host, so nothing can be opened by it",
                opener.0
            )));
        }
        Ok(Some(opener))
    }

    /// Parse the OPTIONAL `name` — what to call the newborn pane
    /// ([`sprag_terminal::Pane::name`]). Absent (or `null`) is a pane nobody names.
    ///
    /// A non-string is a `TypeMismatch` (a malformed request); a string that breaks one of
    /// [`PaneName::parse`](sprag_terminal::PaneName::parse)'s rules, or that another pane of this
    /// DAEMON already carries, is `Rejected` — a well-formed request the host cannot honour, which
    /// is the split [`parse_cwd`](Self::parse_cwd) already keeps. Both checks run before anything
    /// is built, so a refusal costs no pane.
    ///
    /// Checked daemon-wide for [`pane_named`](Self::pane_named)'s reason: a name stands in for a
    /// registry-unique id, so the set it must be unique in is the registry's and not this request's
    /// scope. That also means a caller can be refused by a pane it cannot see, which is the correct
    /// answer — the alternative is two panes one address resolves to.
    ///
    /// **Deliberately NOT on [`SpawnSpec`]**, unlike `cwd`, so it reaches only the two PANE births
    /// and never `new_window` / `new_session`. Their `name` argument is the WINDOW's or SESSION's
    /// name, so putting a pane name behind the same key would give one word two meanings — the
    /// exact ambiguity this whole feature exists to remove — and spelling it differently there
    /// would give one fact two spellings. A birth that creates a container names the container;
    /// naming the pane inside it is a second request, and one this surface can express.
    fn parse_pane_name(
        &self,
        map: &Map<String, Value>,
    ) -> Result<Option<sprag_terminal::PaneName>, InvokeError> {
        let proposed = match map.get("name") {
            None | Some(Value::Null) => return Ok(None),
            Some(Value::String(name)) => name,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let name = sprag_terminal::PaneName::parse(proposed).map_err(refused)?;
        // NAMES THE HOLDER, which is the fact a caller can act on — `rename_pane`'s wording one
        // door over, so the two births of a name refusal read alike. `pane_named` already has the
        // id in hand; answering "another pane" would throw it away at the one place that knows it.
        if let Some(holder) = self.pane_named(&name) {
            return Err(refused(format!(
                "pane {} is already called {:?}",
                holder.0,
                name.as_str()
            )));
        }
        Ok(Some(name))
    }

    /// Parse the OPTIONAL `remote` object (`{host, user?, port?}`) a `sprag ssh` birth request
    /// carries — the structured endpoint that marks the pane a sanctioned remote workspace. Absent
    /// is `None` (an ordinary spawn); present-but-malformed (no string `host`, a non-string `user`,
    /// or a `port` outside `1..=65535`) is a `TypeMismatch`, validated before anything is built.
    fn parse_remote(map: &Map<String, Value>) -> Result<Option<SshRemote>, InvokeError> {
        let Some(value) = map.get("remote") else {
            return Ok(None);
        };
        let obj = value.as_object().ok_or(InvokeError::TypeMismatch)?;
        let host = obj
            .get("host")
            .and_then(Value::as_str)
            .filter(|host| !host.is_empty())
            .ok_or(InvokeError::TypeMismatch)?
            .to_owned();
        let user = match obj.get("user") {
            None | Some(Value::Null) => None,
            Some(Value::String(user)) => Some(user.clone()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let port = match obj.get("port") {
            None | Some(Value::Null) => None,
            Some(value) => {
                let port = value.as_u64().ok_or(InvokeError::TypeMismatch)?;
                let port = u16::try_from(port).map_err(|_| InvokeError::TypeMismatch)?;
                if port == 0 {
                    return Err(InvokeError::TypeMismatch);
                }
                Some(port)
            }
        };
        Ok(Some(SshRemote { user, host, port }))
    }

    /// Fork/exec a validated [`SpawnSpec`] into `pool` — the RUNTIME half, shared by the `spawn`
    /// action and a [`new_session`](Self::new_session)'s birth pane.
    ///
    /// Wired WITH the change-notification hook (so this pane's output bumps the SAME revision the
    /// boot pane's does — a client's `scene/waitFor` wakes on it exactly as on the boot pane) and,
    /// under a daemon, the `on_pane_exit` death-signal (so THIS pane's death feeds the reaper). A
    /// fork/exec failure is `Rejected` — a WELL-FORMED request the OS could not honor (a broken
    /// `$SHELL`, an argv it cannot `exec`), DISTINCT from the malformed request
    /// [`parse_spawn`](Self::parse_spawn) already rejected. Does NOT bump the revision — the caller signals its set change once (a
    /// plain `spawn`, or the create that births this pane), so the two never double-bump or drift.
    fn spawn_parsed(
        &self,
        pool: &Arc<Mutex<Workspace>>,
        spec: SpawnSpec,
        opener: Option<PaneId>,
        name: Option<sprag_terminal::PaneName>,
    ) -> Result<PaneId, InvokeError> {
        let SpawnSpec {
            mut command,
            label,
            cols,
            rows,
            remote,
            cwd,
        } = spec;
        if let Some(cwd) = cwd {
            command.cwd(cwd);
        }
        // The three reader-thread hooks a DAEMON's pane gets, as one value: the wake over THIS
        // session's token (sound because a pane cannot change session), the reaper's death signal,
        // and the attention signal — which names no session, because the router asks the registry
        // who holds the pane (see `attention::Raised`).
        let hooks = sprag_terminal::PaneBirthHooks {
            on_dirty: Some(bump_on_dirty(&self.channels.revision(self.scope.session()))),
            on_exit: self.on_pane_exit.as_ref().map(crate::pane_exit_hook),
            on_attention: self.attention.as_ref().map(crate::pane_attention_hook),
        };
        let mut workspace = lock(pool);
        let (default_cols, default_rows) = workspace.default_size();
        let id = workspace
            .spawn_with_dirty(
                command,
                label,
                cols.unwrap_or(default_cols),
                rows.unwrap_or(default_rows),
                hooks,
            )
            .map_err(|error| refused(format!("the pane's command could not be run: {error}")))?;
        // Stamp the remote endpoint onto the just-born pane (metadata the process does not need),
        // so a restore reconnects it and a dropped-file upload knows its `scp` target.
        if let Some(remote) = remote {
            workspace.set_pane_remote(id, remote);
        }
        // And its PROVENANCE, on the same terms and at the same moment. Stamped HERE — the one
        // runtime half every birth goes through — rather than at each action, so a birth path that
        // takes an opener cannot forget to record it; the actions that have no opener to name pass
        // `None` visibly, which is the decision stated at the call site rather than by omission.
        if let Some(opener) = opener {
            workspace.set_pane_opened_by(id, opener);
        }
        // And its NAME, on the same terms and at the same moment. Already validated and already
        // checked unique against the whole daemon by `parse_pane_name`, which ran before the fork —
        // the pool cannot see the set it would have to check against, and says so.
        if let Some(name) = name {
            workspace.set_pane_name(id, Some(name));
        }
        Ok(id)
    }

    /// `spawn` action: create a pane in THIS request's session and return its id. `cmd` (an argv
    /// array) defaults to `$SHELL`; `cols`/`rows` default to the workspace's default size; `cwd`
    /// defaults to the daemon's directory; `opened_by` names the pane whose occupant is asking;
    /// `name` is what to call the pane.
    fn spawn(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let empty = Map::new();
        let map = match args {
            IntrospectValue::Json(Value::Object(m)) => m,
            IntrospectValue::Null => &empty,
            _ => return Err(InvokeError::TypeMismatch),
        };
        // All three parses run BEFORE the birth, so a request that names an opener the daemon does
        // not hold — or a directory that is not there, or a name already taken — costs no forked
        // child.
        let spec = Self::parse_spawn(map)?;
        let opener = self.parse_opener(map)?;
        let name = self.parse_pane_name(map)?;
        let id = self.spawn_parsed(self.workspace(), spec, opener, name)?;
        // A NEW pane changed the set: wake parked waiters now, before its first output, so a
        // mirror learns the pane exists immediately (the pane-set change-notification, distinct
        // from the per-pane output bump the hook fires).
        self.announce();
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
    }

    /// `split {pane, dir, before?, cmd?, cols?, rows?, remote?}` action: divide `pane` and spawn
    /// the new one into the half that opens, answering with its id — see
    /// [`crate::wire::SPLIT_ACTION`].
    ///
    /// Ordered PRE-FLIGHT, spawn, place. The pre-flight is what lets a caller's mistake — a pane
    /// id that names nothing, a floating pane, another window's — be refused with no child
    /// forked, which matters because the alternative is to fork the user's shell and then have to
    /// kill it. The placement is what actually decides the outcome, and it is checked again there
    /// because the two cannot be one atomic step: the spawn needs the workspace lock and the
    /// placement needs the registry's, and this codebase never nests them.
    ///
    /// If the target exits in that window — between the pre-flight and the placement — the pane
    /// is already born and lands APPENDED instead, and the id is still returned. Killing a shell
    /// the user just asked for would be the worse answer, and the arrangement is readable
    /// ([`crate::wire::LAYOUT_SLOT`]), so the outcome is reported rather than guessed at. It is
    /// the same degradation an unhonorable [`LeafHome`](sprag_terminal::LeafHome) already takes.
    fn split(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let target = self.pane_target(map, "pane")?;
        let (side, dir) = Self::parse_placement(map)?;
        // The birth spec is validated BEFORE the target is looked up, so a request that is
        // malformed in two ways reports the malformed-request error rather than the refusal.
        let spec = Self::parse_spawn(map)?;
        let opener = self.parse_opener(map)?;
        let name = self.parse_pane_name(map)?;
        if !crate::host::tiled_panes(&self.registry, &self.scope).contains(&target) {
            return Err(self.not_tiled(target));
        }
        let id = self.spawn_parsed(self.workspace(), spec, opener, name)?;
        if !crate::host::split_pane(&self.registry, &self.scope, id, target, side, dir) {
            tracing::warn!(
                target: "sprag_host",
                %id,
                %target,
                session = self.scope.session(),
                "the split's target left the tiling while its pane was being born; appended it",
            );
        }
        // The new pane becomes ACTIVE — tmux's `split-window` behaviour, and it belongs HERE rather
        // than in each client. A display client used to publish it after its own split answered,
        // which raced anyone else's `select-pane` landing in between: the second write said "the
        // user is here" about a decision taken before the first. One authority, one write, and a
        // caller that draws nothing (`sprag split-window`, an agent) gets the same behaviour for
        // free instead of being the one caller that ends up somewhere else.
        let _ = crate::host::select_pane(
            &self.registry,
            &self.scope,
            crate::wire::SelectAsk::Pane(id),
        );
        // Both the pane set and the arrangement changed: one announce covers both, exactly as a
        // plain spawn's does for the set alone.
        self.announce();
        Ok(IntrospectValue::Int(
            i64::try_from(id.0).unwrap_or(i64::MAX),
        ))
    }

    /// `close` action: reap the pane with `id` — tmux `kill-pane`. See
    /// [`crate::wire::CLOSE_ACTION`] for the answer's grammar and the cascade.
    ///
    /// The outcome is bound off the registry lock so the reaped owners' blocking `Drop`
    /// (kill/wait/join) runs *outside* it, and the escalations route through the SAME
    /// [`handle_session_kill`](Self::handle_session_kill) a `kill_window` and a `kill_session` use,
    /// so a session ended from the pane end releases its viewers exactly as one ended by name does.
    fn close(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let id = self.pane_target(as_object(args)?, "id")?;
        // The window is the SCOPE's, unchanged: `close` has always acted within the scoped
        // session's current window (its pool was the only thing it could reach), and widening the
        // target to any window holding the id is a separate decision about addressing.
        let outcome =
            lock(&self.registry).close_pane(self.scope.session(), self.scope.window(), id);
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return Err(refused(error)),
        };
        let ended = outcome.ended();
        match outcome {
            // The set shrank: wake parked waiters so a mirror drops the pane's tile promptly. The
            // reaped `Pane` is still bound, so its blocking `Drop` runs after this returns, outside
            // the lock — the bump only signals the already-completed removal.
            PaneKillOutcome::Pane(_reaped) => self.announce(),
            // The window went with it. A removed window is a set change like any other; a session
            // ended from below is the same escalation `kill_window` reports, handled in one place.
            PaneKillOutcome::Window(WindowKillOutcome::Removed(_panes)) => self.announce(),
            PaneKillOutcome::Window(WindowKillOutcome::Session(kill)) => {
                self.handle_session_kill(kill);
            }
        }
        Ok(IntrospectValue::Json(
            serde_json::json!({ crate::wire::ENDED_KEY: ended.as_wire() }),
        ))
    }

    /// `rename_pane {pane, name?}` action: name a pane, or take its name away — see
    /// [`crate::wire::RENAME_PANE_ACTION`] for the vocabulary and every refusal.
    ///
    /// Acts DAEMON-WIDE, unlike every other pane action here, and that is the one thing worth
    /// reading twice. The others reach a pane through the scoped session's current window because
    /// what they do is about that window (splitting it, tiling it, selecting within it). A rename
    /// is about the PANE, whose id and whose name are both registry-unique, so scoping it would
    /// refuse a rename of a pane that plainly exists — the reason
    /// [`report_agent`](Self::report_agent) is daemon-wide too.
    ///
    /// The order is: resolve the target, then validate the name against the daemon, then write.
    /// Validating first would let a caller learn whether a name is free by renaming a pane that
    /// does not exist, and — more to the point — a request wrong in two ways should report the one
    /// the caller can act on first.
    fn rename_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = require_pane_id(map, "pane")?;
        // The proposed name, validated but NOT yet checked for uniqueness — a pane keeping its own
        // name must not be refused by itself, and that is the one bearer this check has to forgive.
        let proposed = match map.get("name") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => {
                Some(sprag_terminal::PaneName::parse(name).map_err(refused)?)
            }
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        if let Some(name) = &proposed
            && let Some(holder) = self.pane_named(name).filter(|holder| *holder != id)
        {
            return Err(refused(format!(
                "pane {} is already called {:?}",
                holder.0,
                name.as_str()
            )));
        }
        // ONE walk, which both finds the pane's own pool and writes into it. Resolving the target
        // and then writing would be two traversals with a gap between them, and the pane could
        // close in that gap — so the answer to "does this pane exist" IS the write's own report
        // (`set_pane_name` says whether the pool held it), not a separate question asked earlier.
        let recorded = Value::from(proposed.as_ref().map(sprag_terminal::PaneName::as_str));
        if self.with_pool_of(id, |pool| pool.set_pane_name(id, proposed)) != Some(true) {
            return Err(refused(format!("no pane {} on this host", id.0)));
        }
        // A pane's published name moved: wake the session's parked clients, which is what turns
        // this into `Event::PaneRenamed` at the dispatch funnel.
        self.announce();
        // Answer with the name that was RECORDED, not the one that was sent. A name is trimmed on
        // the way in, so `" build "` lands as `"build"` — and a caller that echoed its own argument
        // would tell a user the pane is called something it is not. It is the same lesson R294 paid
        // for on the birth path (read the fact back) reached one call earlier: the write itself can
        // say what it wrote, so nobody has to ask a second time or re-implement the rule.
        Ok(IntrospectValue::Json(
            serde_json::json!({ "name": recorded }),
        ))
    }

    /// Run `write` against the POOL that holds the pane with `id`, or answer `None` when this
    /// daemon holds no such pane — [`scan_panes`](Self::scan_panes)'s mutating counterpart.
    ///
    /// Separate from `scan_panes` because a mutation needs the pool, not the pane: a `&mut Pane`
    /// cannot leave the workspace guard, and handing out the guard itself would let a caller hold
    /// the workspace lock while taking another.
    fn with_pool_of<T>(&self, id: PaneId, write: impl FnOnce(&mut Workspace) -> T) -> Option<T> {
        let registry = lock(&self.registry);
        for session in registry.sessions() {
            for window in session.windows() {
                let mut workspace = lock(window.workspace());
                if workspace.panes().iter().any(|pane| pane.id() == id) {
                    return Some(write(&mut workspace));
                }
            }
        }
        None
    }

    /// `report_agent` action: take a report from a process inside the pane — see
    /// [`crate::wire::REPORT_AGENT_ACTION`] for the argument vocabulary.
    ///
    /// Three things happen in one call and their ORDER is the correctness here: the report is taken
    /// under the agent lock, and only if it MOVED the published verdict does this announce — the same
    /// condition the settle waker uses, so the two publishers of an agent verdict cannot come to
    /// disagree about what counts as a change. The record and the wake travel together
    /// ([`ChannelRegistry::announce`] holds the journal lock across the bump), so a client woken by
    /// this can always read the event that woke it.
    ///
    /// A pane the daemon does not hold is REFUSED rather than remembered. Without that check a
    /// reporter with a stale `SPRAG_PANE` — a process that outlived its pane — would create a tracker
    /// for a pane that does not exist, which nothing would ever prune (the sweep's census forgets
    /// panes it cannot see, and it cannot see this one either).
    fn report_agent(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = require_pane_id(map, "id")?;
        let source = map
            .get("source")
            .and_then(Value::as_str)
            .filter(|source| !source.is_empty())
            .ok_or(InvokeError::TypeMismatch)?;
        let state = map
            .get("state")
            .and_then(Value::as_str)
            .and_then(sprag_detect::AgentState::from_wire)
            .ok_or(InvokeError::TypeMismatch)?;
        let name = map.get("name").and_then(Value::as_str).map(str::to_owned);
        // Absent is "I have no clock", which is always fresh. A malformed `seq` is a TypeMismatch
        // rather than a silent fall-back to that: a reporter whose counter arrived as a string has a
        // bug, and answering "accepted" would hide it behind a report that can never be refused.
        let seq = match map.get("seq") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.as_u64().ok_or(InvokeError::TypeMismatch)?),
        };
        // `bind` asks for the report to last only as long as whatever is running in the pane; it
        // does NOT say what that is. The daemon reads which process group owns the pane's terminal
        // itself, so a caller can neither name somebody else's group nor park a release on a pane it
        // does not speak for. Absent is a report that stands until it is released, which is what a
        // person at a command line means by making one.
        let bind = match map.get("bind") {
            None | Some(Value::Null) => false,
            Some(value) => value.as_bool().ok_or(InvokeError::TypeMismatch)?,
        };
        let Some(agents) = self.agents.as_ref() else {
            // No detector on this host (a GUI's in-process host, a unit test): there is no memory to
            // report INTO, and inventing one here would publish a verdict the pane list cannot read.
            return Err(refused(NO_DETECTOR));
        };
        let Some(owner) = self.with_pane(id, |pane| bind.then(|| pane.pty().foreground_pgid()))
        else {
            return Err(refused(format!("no pane {} on this host", id.0)));
        };
        let (outcome, seq_published) = agents.report(
            id,
            sprag_detect::Report {
                state,
                agent: name,
                source: source.to_owned(),
                seq,
                owner: owner.flatten().map(u64::from),
            },
            crate::config::agent_settle,
        );
        if outcome.changed {
            self.channels.announce(
                self.scope.session(),
                vec![crate::events::Event::AgentStateChanged(id.0)],
            );
        }
        Ok(IntrospectValue::Json(serde_json::json!({
            "accepted": outcome.accepted,
            "changed": outcome.changed,
            "seq": seq_published,
        })))
    }

    /// `release_agent` action: drop the report in force for `id` — see
    /// [`crate::wire::RELEASE_AGENT_ACTION`].
    ///
    /// It announces NOTHING, and that is not an omission. A release does not publish a verdict; it
    /// asks for one to be re-derived from the screen, and the pass that does that
    /// ([`crate::sweep_once`]) announces whatever it finds through the one publisher those events
    /// already come from. Announcing here would wake every client of the session to read a verdict
    /// that has not been recomputed yet.
    fn release_agent(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let id = require_pane_id(as_object(args)?, "id")?;
        let Some(agents) = self.agents.as_ref() else {
            return Err(refused(NO_DETECTOR));
        };
        if !self.holds_pane(id) {
            return Err(refused(format!("no pane {} on this host", id.0)));
        }
        Ok(IntrospectValue::Json(
            serde_json::json!({ "released": agents.release(id) }),
        ))
    }

    /// `display_message` action: put a sentence in front of the people looking at this daemon — see
    /// [`crate::wire::DISPLAY_MESSAGE_ACTION`] for the argument vocabulary and the address.
    ///
    /// Three things happen and their ORDER is the correctness here, exactly as it is one method up
    /// in [`report_agent`](Self::report_agent): the message is queued under the attachment lock, the
    /// lock is DROPPED, and only then are the sessions it landed in woken. Announcing while holding
    /// the registry would take the change-channel lock inside the attachment one, which is an order
    /// nothing else in this daemon uses — and [`crate::notify`]'s own rule is that nothing expensive
    /// happens under a lock a keystroke has to pass through.
    ///
    /// **The wake is derived from the delivery** ([`crate::Delivery::sessions`]) rather than from the
    /// request's scope, and that is not tidiness: a `client` target may be attached to a session
    /// other than this request's, so bumping the scope would leave the one client that was actually
    /// written to parked on a channel that never moved — a message queued forever behind a wake that
    /// went somewhere else.
    fn display_message(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let text = map
            .get("text")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        // Refused rather than trimmed: the rules are the ones a terminal row imposes, and a caller
        // whose sentence broke one has a bug that a silent truncation would hide (which is what the
        // rival's `sanitized_notification_text` does).
        let text =
            crate::report::MessageText::parse(text).map_err(|_| InvokeError::TypeMismatch)?;
        // Absent is `note`: a caller that did not think about severity has not claimed urgency. A
        // word this build does not know is a TypeMismatch rather than a silent fall-back to that —
        // `-s alrt` must not quietly become a note.
        let severity = match map.get("severity") {
            None | Some(Value::Null) => crate::report::Severity::default(),
            Some(Value::String(word)) => {
                crate::report::Severity::parse(word).ok_or(InvokeError::TypeMismatch)?
            }
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let audience = match map.get("client") {
            None | Some(Value::Null) => crate::Audience::Session(self.scope.session().to_owned()),
            Some(Value::String(client)) => crate::Audience::Client(client.clone()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let Some(attachments) = self.attachments.as_ref() else {
            // No attachment map on this host (a GUI's in-process host, a unit test): there are no
            // wire clients to address, and inventing a delivery here would report a sentence shown to
            // somebody who does not exist.
            return Err(refused(NO_CLIENTS));
        };
        let delivery = {
            let mut attachments = lock(attachments);
            // A NAMED client that is not attached is a caller's mistake, not an empty audience, and
            // the two must not answer alike: `{clients: []}` for a typo would read as "nobody is
            // watching" and send an agent looking for a person who is right there.
            if let crate::Audience::Client(client) = &audience
                && !attachments.is_attached(client)
            {
                // NAMES WHO IS THERE, rather than pointing at another command to run. The CLI used
                // to append *"run `sprag list-clients`"* because a payload-free refusal left it
                // nothing else to offer; this end already holds the answer that verb would print,
                // and a client id is minted per process — nobody types one from memory.
                let attached: Vec<String> = attachments
                    .clients()
                    .into_iter()
                    .map(|info| info.client)
                    .collect();
                return Err(refused(if attached.is_empty() {
                    format!("no client called {client:?} is attached; none are")
                } else {
                    format!(
                        "no client called {client:?} is attached; these are: {}",
                        attached.join(", ")
                    )
                }));
            }
            attachments.deliver(&audience, &crate::report::Announcement { text, severity })
        };
        for session in delivery.sessions() {
            self.channels.bump(session);
        }
        Ok(IntrospectValue::Json(serde_json::json!({
            "clients": delivery.clients(),
        })))
    }

    /// Whether the DAEMON holds a pane with this id — any session, any window.
    ///
    /// Daemon-wide rather than this request's scope on purpose: the agent memory is keyed by
    /// [`PaneId`] alone (every window's pool draws from one counter), so a report about a pane in
    /// another session is about a pane that really does exist and really does have a tracker. Scoping
    /// the check would refuse a hook whose pane sits in a session other than the connection's default
    /// — which is most of them.
    fn holds_pane(&self, id: PaneId) -> bool {
        self.with_pane(id, |_| ()).is_some()
    }

    /// The refusal a verb states when the scope resolves to no window at all — a session killed
    /// under a connection that was already scoped to it.
    fn no_current_window(&self) -> String {
        format!(
            "session {:?} has no window {:?}",
            self.scope.session(),
            self.scope.window()
        )
    }

    /// WHY a `swap_pane` traded nothing — [`no_such_selection`](Self::no_such_selection)'s peer one
    /// verb over, and the same discrimination.
    ///
    /// An edge and a floating origin are OUTCOMES this verb answers with
    /// ([`SwapHow`](crate::wire::SwapHow)); the only refusals left are a pane no window of the
    /// session holds and a window with no active pane to default to.
    fn no_such_swap(&self, ask: &SwapAsk) -> InvokeError {
        let session = self.scope.session();
        refused(match ask.origin() {
            Some(pane) => format!("session {session:?} holds no pane {}", pane.0),
            None => format!("session {session:?}'s current window has no active pane"),
        })
    }

    /// WHY a `select_pane` landed nowhere — read off the ASK, which is what names the pane the
    /// window turned out not to hold.
    ///
    /// A direction that runs off an edge is NOT here: that is an outcome
    /// ([`SelectHow::AtEdge`](crate::wire::SelectHow)) the verb answers with, not a refusal. The
    /// only ways this call answers nothing are a pane the window does not hold and a window with no
    /// active pane at all, and the two send a caller somewhere different.
    fn no_such_selection(&self, ask: &crate::wire::SelectAsk) -> InvokeError {
        let here = format!("session {:?}'s current window", self.scope.session());
        refused(match ask {
            crate::wire::SelectAsk::Pane(pane) => format!("{here} holds no pane {}", pane.0),
            crate::wire::SelectAsk::Toward {
                from: Some(from), ..
            } => {
                format!("{here} holds no pane {} to step from", from.0)
            }
            crate::wire::SelectAsk::Toward { from: None, .. } => {
                format!("{here} has no active pane to step from")
            }
        })
    }

    /// WHY `id` is not in the scoped window's tiling — the refusal the placement verbs share.
    ///
    /// Three distinct facts, and separating them is the whole point of this function. The CLI used
    /// to print all three joined by `or` (*"it exited, it is floating, or it belongs to another
    /// window"*) because the daemon answered a payload-less `Rejected` and the client had to guess;
    /// each one sends the user somewhere different, and only this end can tell them apart.
    ///
    /// Called only on the failing branch, so the extra walks cost nothing on the path that works.
    fn not_tiled(&self, id: PaneId) -> InvokeError {
        if !self.holds_pane(id) {
            return refused(format!("no pane {} on this host", id.0));
        }
        let floating = lock(&self.registry)
            .window(self.scope.session(), self.scope.window())
            .is_some_and(|window| window.floating().contains(&id));
        if floating {
            return refused(format!(
                "pane {} is floating, so the tiling does not hold it",
                id.0
            ));
        }
        refused(format!(
            "pane {} is not in session {:?}'s window {:?}",
            id.0,
            self.scope.session(),
            self.scope.window(),
        ))
    }

    /// Run `read` against the pane with `id`, or answer `None` when this daemon does not hold it.
    ///
    /// One walk and one definition of "we hold this pane": [`holds_pane`](Self::holds_pane) is this
    /// asked for nothing, so a caller that needs the pane ITSELF cannot end up checking membership
    /// against a second traversal that might one day disagree with the first.
    fn with_pane<T>(&self, id: PaneId, read: impl FnOnce(&Pane) -> T) -> Option<T> {
        let mut read = Some(read);
        self.scan_panes(|pane| {
            if pane.id() != id {
                return None;
            }
            // `scan_panes` stops at the first `Some`, so this runs at most once and the `?` is
            // unreachable rather than a silent skip.
            Some(read.take()?(pane))
        })
    }

    /// Answer which pane this daemon holds under the name `name`, refusing to guess if two do.
    ///
    /// **`None` is total over two different situations, and the caller must not distinguish them**:
    /// no pane carries that name, or MORE than one does. The second cannot arise from a correct
    /// sequence of requests — every surface that accepts a name checks it daemon-wide first — but
    /// the check and the write are not one atomic step (closing that would mean holding the
    /// registry lock across a `posix_spawn`, which is a convoy), so two births racing for one name
    /// could in principle both land.
    ///
    /// Refusing an ambiguous name is what makes that residual SAFE rather than silent: the whole
    /// reason a name exists is that a positional pane number can quietly resolve to the wrong pane,
    /// and a resolver that returned "the first match" would reintroduce exactly that. A duplicate
    /// is then a loud failure at USE time, never a wrong-pane write.
    ///
    /// Walks the whole daemon for [`holds_pane`](Self::holds_pane)'s reason: a name stands in for a
    /// [`PaneId`], which is registry-unique, so its scope is the registry and not this request's
    /// session.
    fn pane_named(&self, name: &sprag_terminal::PaneName) -> Option<PaneId> {
        let mut found = None;
        self.scan_panes(|pane| {
            if pane.name() != Some(name) {
                return None;
            }
            match found {
                // A second bearer: stop the walk and answer nothing at all.
                Some(_) => Some(None),
                None => {
                    found = Some(pane.id());
                    None
                }
            }
        })
        .unwrap_or(found)
    }

    /// The ONE traversal of every pane this daemon holds: registry lock, then each window's
    /// workspace lock in turn, stopping at the first pane `pick` answers `Some` for.
    ///
    /// Every "does the daemon hold …" question goes through here rather than writing its own nested
    /// loop, so [`holds_pane`](Self::holds_pane), [`with_pane`](Self::with_pane) and
    /// [`pane_named`](Self::pane_named) are three questions with ONE answer to what the pane set is
    /// — the property `with_pane`'s docs already asked for when it was the only asker.
    fn scan_panes<T>(&self, mut pick: impl FnMut(&Pane) -> Option<T>) -> Option<T> {
        let registry = lock(&self.registry);
        for session in registry.sessions() {
            for window in session.windows() {
                let workspace = lock(window.workspace());
                if let Some(picked) = workspace.panes().iter().find_map(&mut pick) {
                    return Some(picked);
                }
            }
        }
        None
    }

    /// `resize` action: resize the pane with `id` to `cols x rows`.
    fn resize(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = self.pane_target(map, "id")?;
        let cols = opt_dim(map, "cols")?.ok_or(InvokeError::TypeMismatch)?;
        let rows = opt_dim(map, "rows")?.ok_or(InvokeError::TypeMismatch)?;
        // The display's cell pixel geometry, OPTIONAL: a GUI client sends it (its font metric) so
        // the PTY winsize and XTWINOPS pixel reports are truthful; a headless / older client omits
        // it and `(0, 0)` leaves the pane's last-known cell geometry untouched.
        let cell_px = (
            opt_dim(map, "cell_width")?.unwrap_or(0),
            opt_dim(map, "cell_height")?.unwrap_or(0),
        );
        match lock(self.workspace()).resize(id, cols, rows, cell_px) {
            Ok(true) => Ok(IntrospectValue::Null),
            Ok(false) => Err(refused(format!(
                "session {:?}'s current window holds no pane {}",
                self.scope.session(),
                id.0
            ))),
            Err(error) => Err(refused(format!(
                "pane {}'s terminal would not take the new size: {error}",
                id.0
            ))),
        }
    }

    /// `set_layout {tree}` action: install a client's settled arrangement, answering with
    /// the canonical one — the write half of the arc (see [`crate::wire::SET_LAYOUT_ACTION`]).
    ///
    /// The answer carries the tree as the host stores it, with every client-minted divider
    /// now named, so one round trip both records the gesture and tells the client which
    /// identities to key its per-split state on.
    fn set_layout(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        // The revision this gesture was authored against — the arrangement the client was
        // looking at. Required: a write with no answer to "which layout is this about?"
        // cannot be adjudicated, and silently accepting it is how one client reverts
        // another's gesture with neither told.
        let expected = map
            .get("expected_revision")
            .and_then(Value::as_u64)
            .ok_or(InvokeError::TypeMismatch)?;
        let tree = map.get("tree").ok_or(InvokeError::TypeMismatch)?.clone();
        // A tree that will not even deserialise is a malformed REQUEST (the client and host
        // disagree on the shape), distinct from the well-formed-JSON-but-invalid-arrangement
        // the tree's own validation rejects — so it fails here as a TypeMismatch rather than
        // reaching the window.
        let tree: LayoutWire = serde_json::from_value(tree).map_err(|error| {
            tracing::warn!(target: "sprag_host", %error, "set_layout: undeserialisable tree");
            InvokeError::TypeMismatch
        })?;
        // Optional: the NAME of the window the gesture was authored against. Absent ⇒ no
        // window-staleness check (an older or single-client caller); present-but-not-a-string is
        // malformed, the same refusal a wrong-typed `expected_revision` gets. It closes the
        // per-window-revision bound: a write drawn on a window the client has switched away from
        // is refused rather than mis-applied to whatever is current now.
        let expected_window = match map.get("expected_window") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let snapshot =
            crate::host::set_layout(&self.registry, &self.scope, tree, expected, expected_window)
                .ok_or_else(|| refused(self.no_current_window()))?;
        // The arrangement changed: wake parked waiters so another attached client
        // re-projects promptly, exactly as a pane-set change does.
        self.announce();
        layout_value(snapshot).ok_or_else(|| refused(UNRENDERABLE_LAYOUT))
    }

    /// `set_floating {id, floating}` action: take a pane out of the tiling or put it back,
    /// answering with the resulting arrangement (see [`crate::wire::SET_FLOATING_ACTION`]).
    fn set_floating(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let id = require_pane_id(map, "id")?;
        let floating = map
            .get("floating")
            .and_then(Value::as_bool)
            .ok_or(InvokeError::TypeMismatch)?;
        let snapshot = crate::host::set_floating(&self.registry, &self.scope, id, floating)
            .ok_or_else(|| self.not_tiled(id))?;
        self.announce();
        layout_value(snapshot).ok_or_else(|| refused(UNRENDERABLE_LAYOUT))
    }

    /// `new_session {name?, cmd?, cols?, rows?, remote?}` action: create a session BORN WITH A
    /// SHELL, answering with its name.
    ///
    /// `name` mirrors the `session` scope param's own three-way shape: ABSENT asks the
    /// registry to allocate the lowest free name (tmux's `new-session` with no `-s`), a STRING
    /// names it, and a NON-STRING is a malformed request — rejected, never silently allocated,
    /// which would be the same alias smell the scope param refuses in its type-error corner. The
    /// `cmd`/`cols`/`rows` birth spec is validated the SAME way, before the session is created
    /// ([`parse_spawn`](Self::parse_spawn)): a malformed one is rejected with nothing built.
    ///
    /// On the happy path a session is born with one pane — tmux's `new-session`, where creating a
    /// session always spawns its first window's pane — so it is not empty. `cmd`/`cols`/`rows`
    /// shape that birth pane, mirroring tmux's `new-session -x -y [command]`: the GUI passes its
    /// own first pane so a client's configured layout tops up from it without a default-shell
    /// mismatch; the CLI passes nothing and gets the default `$SHELL` at the pool's default size.
    /// Putting the spawn HERE (the server-side command handler, sprag's `cmd-new-session`) rather
    /// than in each caller keeps "a session has a shell" an invariant at the authority, not a
    /// convention every caller must remember — and it is here, not in the pinion-free registry,
    /// because the pane's death-signal ([`on_pane_exit`]) is a daemon-lifetime concern the
    /// registry does not carry. A RUNTIME fork/exec failure is the one case that still leaves the
    /// session empty (see the birth site below) — best-effort, not an absolute invariant.
    ///
    /// Creating is not attaching, and nothing here changes what any other client sees: every
    /// client's scope is either its own name or the default (which a `kill_session` of the current
    /// default re-points, but a `new_session` never moves). The answer is the name — indispensable
    /// for the allocated case (the caller did not choose it) — so a caller can scope its next
    /// request with what it just made, without a round trip to [`SESSIONS_SLOT`] to confirm.
    ///
    /// [`on_pane_exit`]: Self::on_pane_exit
    fn new_session(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let name = match map.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        // VALIDATE the birth spec BEFORE creating anything, so a malformed `cmd`/`cols`/`rows` is
        // rejected with no session built — uniform with the `spawn` action and with `name` above.
        // Only a RUNTIME fork/exec failure (the birth site below) is tolerated non-fatally.
        let spec = Self::parse_spawn(map)?;
        // Create the empty session shell under the registry authority, then clone its pool Arc OUT
        // (via `workspace_of`, the one helper for exactly this) so the birth pane spawns OFF the
        // registry lock — the established registry->workspace order, never nested across a
        // fork/exec that would otherwise stall every other request on the registry lock.
        //
        // The session is EMPTY until the spawn below lands, and an empty session reads as "nothing
        // live" to the daemon's reaper — so an unrelated last pane dying in that gap used to end
        // the daemon under the client that had just asked for the session. A [`BirthPin`] taken
        // under THIS lock (never after it, which would leave the same gap narrower) says a pane is
        // on its way; it is released when `pin` falls at the end of this call, spawned or not, and
        // its release nudges the reaper so a birth that FAILED still lets an idle daemon go.
        let (allocated, pool, pin) = {
            let mut registry = lock(&self.registry);
            // A taken name is the client's mistake, not a malformed request: it is well-formed
            // and simply cannot be honored — and `SessionError`'s own Display says WHICH mistake,
            // which is the sentence a caller prints.
            let allocated = registry.new_session(name).map_err(refused)?;
            let pool = registry
                .workspace_of(&allocated)
                .expect("the session just created resolves");
            let pin = crate::BirthPin::taken(
                &self.registry,
                &mut registry,
                self.on_pane_exit.as_ref().map(crate::pane_exit_hook),
            );
            (allocated, pool, pin)
        };
        // Birth the pane. Only a RUNTIME fork/exec failure reaches here (a broken `$SHELL`, an argv
        // the OS cannot `exec`) — the malformed request was already rejected above. It is logged,
        // not fatal: the session still exists as a valid attach target, merely empty until a pane
        // is added, so "a well-formed create with a free name succeeds" stays total rather than
        // orphaning a half-created session behind an error.
        // Two `None`s, each a decision rather than an omission. No OPENER: a session's BIRTH pane is
        // nobody's work pane — the request that creates a session is about the session, and stamping
        // the creator here would make every new session's first pane read as something an agent must
        // clean up. No NAME: this request's own `name` argument is the SESSION's, so a pane name
        // would need a second spelling of one fact, and giving one key two meanings is the ambiguity
        // a pane name exists to remove (`parse_pane_name`).
        if let Err(error) = self.spawn_parsed(&pool, spec, None, None) {
            tracing::warn!(
                target: "sprag_host",
                ?error,
                session = %allocated,
                "the birth pane could not spawn; the session was created empty",
            );
        }
        // Two sessions changed, and only one of them is this request's scope. The NEW session now
        // holds a live pane — announced on its OWN channel, because a client that asked for it will
        // scope its next wait there and would otherwise sleep through its own birth pane's first
        // output. The scoped session's list of sessions grew, which is a change to what IT shows.
        self.channels.bump(&allocated);
        self.announce();
        // Explicit, because the ORDER matters and a lexical drop would not say so: the claim must
        // outlive the spawn above (that is its whole job) and must fall before this call answers,
        // so the client's next request meets a daemon whose liveness is settled either way.
        drop(pin);
        Ok(IntrospectValue::Json(Value::String(allocated)))
    }

    /// `kill_session {name}` action: kill a session (tmux `kill-session`).
    ///
    /// A non-last kill removes the session and answers `null` — a set change, so a client
    /// watching [`SESSIONS_SLOT`] is woken. Killing the LAST session drains it and ENDS the
    /// daemon: it fires the SAME death-signal a pane exit does ([`on_pane_exit`]), so the reaper
    /// re-checks liveness, finds none, and exits through the one SIGTERM shutdown funnel — the
    /// library names neither exit nor SIGTERM. Off a daemon (a GUI's in-process host, the tests)
    /// `on_pane_exit` is `None`, so the kill removes the session but nothing exits, which is
    /// exactly right there.
    ///
    /// [`on_pane_exit`]: Self::on_pane_exit
    fn kill_session(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let name = as_object(args)?
            .get("name")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        // Remove/drain UNDER the lock, then RELEASE it before dropping the reaped owners: binding
        // `outcome` here means the lock guard falls at the `;`, so the removed session / drained
        // panes ride in `outcome` and their blocking `Drop` (kill / wait / join the reader) runs
        // OFF the lock — the `close` action's discipline.
        let outcome = lock(&self.registry).kill_session(name);
        let ended = match outcome {
            Ok(outcome) => {
                let ended = outcome.ended();
                self.handle_session_kill(outcome);
                ended
            }
            Err(error) => return Err(refused(error)),
        };
        Ok(IntrospectValue::Json(
            serde_json::json!({ crate::wire::ENDED_KEY: ended.as_wire() }),
        ))
    }

    /// React to a [`KillOutcome`] that has already been bound OFF the registry lock (so its
    /// reaped owners drop here, outside the lock): a removed session is a set change that wakes a
    /// client watching the sessions list; the last-session case nudges the reaper (via the pane
    /// death-signal) to re-check liveness and exit through the SIGTERM funnel. Shared by
    /// [`kill_session`](Self::kill_session) and the last-window escalation in
    /// [`kill_window`](Self::kill_window), so the two cannot drift.
    fn handle_session_kill(&self, outcome: KillOutcome) {
        match outcome {
            KillOutcome::Removed(removed) => {
                // CLOSE the dead session's channel before announcing on this one. A client parked
                // on `scene/waitFor` for the session that just went is waiting on a token nothing
                // can ever advance again — no pane of it survives to produce output and no request
                // will be scoped to it — so closing is what releases it to re-read, meet the scope
                // refusal, and detach. The name comes off the removed session itself rather than
                // from the caller's argument: the last-window escalation reaches here with no name
                // in hand, and one of the two paths guessing would be the one that leaked.
                self.channels.close(removed.name());
                // ...and RELEASE its viewers, beside the close and for the same reason: this is the
                // one place a removed session's departure is published, so both things keyed by its
                // NAME are unkeyed here rather than at each caller. Left behind, an attachment
                // outlives its session and is then adopted by whatever next takes the name —
                // measured at R303, both halves: `list-clients` naming a session the registry no
                // longer held, and a new session of the same name reporting a viewer it never had.
                if let Some(attachments) = &self.attachments {
                    let released = lock(attachments).session_ended(removed.name());
                    if released > 0 {
                        tracing::debug!(
                            target: "sprag_host",
                            session = removed.name(),
                            released,
                            "released the viewers of a session that was killed",
                        );
                    }
                }
                self.announce();
            }
            KillOutcome::KilledServer(_drained) => {
                if let Some(on_pane_exit) = &self.on_pane_exit {
                    on_pane_exit();
                }
            }
        }
    }

    /// Resolve a `window` TARGET arg (used by `rename_window` / `kill_window`): absent ⇒ the
    /// current window (the scope's), a string ⇒ that window, present-but-not-a-string ⇒ malformed
    /// — the same aliasing corner the session scope param refuses, rather than silently falling
    /// back to the current window and acting on the wrong one.
    fn window_target<'a>(&'a self, map: &'a Map<String, Value>) -> Result<&'a str, InvokeError> {
        match map.get("window") {
            None => Ok(self.scope.window()),
            Some(Value::String(name)) => Ok(name),
            Some(_) => Err(InvokeError::TypeMismatch),
        }
    }

    /// `new_window {name?, detached?, opened_by?, cmd?, cols?, rows?}` action: create a window in
    /// THIS request's session, born with a shell, select it unless `detached`, and answer with its
    /// name — tmux `new-window` and its `-d`.
    ///
    /// The window is created under the registry lock, then its pool is cloned OUT and the birth
    /// pane spawned OFF the lock — the exact [`new_session`](Self::new_session) pattern one level
    /// down, so the same death-signal and change-notification wiring applies and no fork runs
    /// under the registry lock. A runtime fork/exec failure leaves the window empty (logged,
    /// non-fatal); a malformed birth spec is rejected before anything is created.
    ///
    /// ⚠ **The pool is looked up BY NAME, not as "the current window's".** It used to be the
    /// latter, which was correct only because `new_window` always selected what it created — so
    /// the moment `detached` existed, that lookup would have spawned the birth pane into the
    /// window the user was already on. A silent wrong answer: a `-d` window would come back EMPTY
    /// and the person's window would grow a pane nobody asked for.
    fn new_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let name = match map.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let born = self.parse_window_birth(map)?;
        // Validate the birth spec BEFORE creating anything (uniform with `new_session`).
        let spec = Self::parse_spawn(map)?;
        let (created, pool) = {
            let mut registry = lock(&self.registry);
            // A taken window name is well-formed and simply cannot be honored; `SessionError`
            // says which of its rules the name broke.
            let created = registry
                .new_window(self.scope.session(), name, born)
                .map_err(refused)?;
            let pool = registry
                .window_workspace(self.scope.session(), &created)
                .expect("the scoped session resolves; new_window just created that window");
            (created, pool)
        };
        // THE BIRTH PANE INHERITS THE WINDOW'S OPENER, and that sentence used to read "both `None`
        // for `new_session`'s stated reasons: a WINDOW's birth pane is nobody's work pane".
        //
        // ⚠ That premise MOVED in the round that stated it. It was written when only a person could
        // make a window; R313 let a caller that is not a person make one, and measured what the old
        // rule then said: an agent that opened a window of its own was told **"this pane was opened
        // by a PERSON, not by you"** about the pane its own request had just created — by
        // `rename_pane`, `close_pane` and `resize_pane` alike, while `close_window` destroyed that
        // same pane without a murmur. A surface refusing with a sentence that is false about its
        // subject is the exact shape R311 exists to have removed.
        //
        // Whoever asked for the window asked for its first pane. `new_session` keeps `None` because
        // its own reason still holds: a session is not creatable by anything but a person.
        //
        // The NAME stays `None`: this request's `name` is the WINDOW's, so a pane name here would be
        // one fact under two spellings, which is the ambiguity a pane name exists to remove.
        if let Err(error) = self.spawn_parsed(&pool, spec, born.opened_by, None) {
            tracing::warn!(
                target: "sprag_host",
                ?error,
                session = self.scope.session(),
                window = %created,
                "the window's birth pane could not spawn; the window was created empty",
            );
        }
        self.announce();
        Ok(IntrospectValue::Json(Value::String(created)))
    }

    /// Read [`WindowBirth`] out of a `new_window` request — whether the window takes the screen,
    /// and who asked for it.
    ///
    /// Both keys are ADDITIVE and both default to what every caller did before they existed
    /// (`detached: false`, no opener), so a client that sends neither is unchanged. A key of the
    /// WRONG TYPE is refused rather than defaulted: `detached: "true"` is a caller that believes it
    /// asked for something, and honouring it as `false` would take the screen it asked to be left
    /// alone — the one failure this flag exists to prevent.
    fn parse_window_birth(&self, map: &Map<String, Value>) -> Result<WindowBirth, InvokeError> {
        let detached = match map.get(DETACHED_KEY) {
            None | Some(Value::Null) => false,
            Some(Value::Bool(detached)) => *detached,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        // The pane half goes through [`parse_opener`], the SAME function a pane's provenance uses
        // — so a stale `SPRAG_PANE` is refused at the window level for the reason it is refused at
        // the pane level, and there is one rule rather than two that agree today.
        Ok(WindowBirth {
            detached,
            opened_by: self.parse_opener(map)?,
        })
    }

    /// `select_window {window}` action: make a window current in THIS request's session — tmux
    /// `select-window`. Session state: every attached client follows on its next read.
    fn select_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        // The grammar is parsed by `SelectWindowAsk` rather than key by key here, for the reason
        // `select_pane` states one verb over: the CLI and the keybinding BUILD one, and this is the
        // end that has to admit exactly what they can spell.
        let value = match args {
            IntrospectValue::Json(value) => value,
            IntrospectValue::Null => &Value::Null,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let ask = SelectWindowAsk::parse(value).ok_or(InvokeError::TypeMismatch)?;
        let session = self.scope.session();
        let landed = match &ask {
            // The name arm answers the name it was GIVEN; the identity arm has to read the name
            // back, because the whole point is that the caller does not know it — it holds a row
            // whose label may already be stale, and the answer is what a status line paints.
            SelectWindowAsk::At(WindowRef::Named(window)) => lock(&self.registry)
                .select_window(session, window)
                .map(|()| window.clone()),
            SelectWindowAsk::At(WindowRef::Picked(window)) => {
                let mut registry = lock(&self.registry);
                registry.select_window_id(session, *window).and_then(|()| {
                    registry
                        .session(session)
                        .map(|s| s.current_window().name().to_owned())
                        .ok_or_else(|| sprag_terminal::SessionError::Unknown(session.to_owned()))
                })
            }
            // TOTAL once the session resolves — a session always has a window — so the only error
            // this arm can carry is an unknown SESSION, which the scope already refused at the door.
            SelectWindowAsk::Step(step) => {
                lock(&self.registry).select_window_relative(session, *step)
            }
        }
        .map_err(refused)?;
        self.announce();
        // The window it LANDED on, for both arms: a caller that stepped cannot know it, and giving
        // the named arm the same answer is what keeps one shape for one verb.
        Ok(IntrospectValue::Json(Value::String(landed)))
    }

    /// `move_window {window?, place|before|after}` action: move a window's PLACE in THIS request's
    /// session's order — tmux `move-window`. See [`crate::wire::MOVE_WINDOW_ACTION`].
    ///
    /// Answers `{window, how}` — which window moved (a caller may have omitted it) and WHAT
    /// happened, in [`sprag_terminal::PlaceHow`]'s four words. The daemon states the reason because
    /// three of the four leave the order untouched and a caller re-reading `windows` cannot tell
    /// them apart: "already there", "this session holds one window" and "the anchor was the window
    /// itself" have three different remedies and one appearance.
    fn move_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let value = match args {
            IntrospectValue::Json(value) => value,
            IntrospectValue::Null => &Value::Null,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let ask = MoveWindowAsk::parse(value).ok_or(InvokeError::TypeMismatch)?;
        let session = self.scope.session();
        // Resolved HERE and not in the registry, because "the current window" is a fact about the
        // scope and the registry's verb takes a name — the same split `rename_window` keeps. The
        // resolved name is also what the answer carries, so a caller that omitted it learns which
        // window it moved without a second read at a second instant.
        let (window, how) = {
            let mut registry = lock(&self.registry);
            let window = match &ask.window {
                Some(window) => window.clone(),
                None => registry
                    .session(session)
                    .ok_or_else(|| refused(format!("no session named {session:?}")))?
                    .current_window()
                    .name()
                    .to_owned(),
            };
            let how = registry
                .move_window(session, &window, &ask.place)
                .map_err(refused)?;
            (window, how)
        };
        // Announced whatever the outcome: an announcement is a WAKE, and the change funnel behind
        // it derives nothing from a move that moved nothing (`Event::WindowsReordered` is a
        // sequence comparison, so an unchanged order produces an empty batch). Gating the wake on
        // `how.changed()` here would be a second place deciding what counts as a change.
        self.announce();
        Ok(IntrospectValue::Json(MoveWindowAsk::answer(&window, how)))
    }

    /// `select_pane {pane?} | {dir?, from?}` action: make a pane active in THIS request's session's
    /// current window — tmux `select-pane`. See [`crate::wire::SELECT_PANE_ACTION`].
    ///
    /// Answers `{pane, changed, outcome}` — where the window is, whether that moved, and WHY it is
    /// there ([`crate::wire::SelectHow`]). The reason is the daemon's to state because one of its
    /// four cases cannot be reconstructed from the other two keys by any caller: an edge and a
    /// floating origin both leave the window where it was.
    ///
    /// The grammar is parsed by [`SelectAsk`](crate::wire::SelectAsk) rather than key by key here,
    /// because the CLI verb and the MCP tool BUILD one and this is the end that has to admit exactly
    /// what they can spell. Every reading it refuses is one `TypeMismatch`: "select nothing", "select
    /// two things" and "step from a pane toward nowhere" are one class of caller bug, and
    /// `InvokeError` has no payload to separate them with anyway.
    fn select_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let value = match args {
            IntrospectValue::Json(value) => value,
            IntrospectValue::Null => &Value::Null,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let ask = crate::wire::SelectAsk::parse(value).ok_or(InvokeError::TypeMismatch)?;
        let selection = crate::host::select_pane(&self.registry, &self.scope, ask)
            .ok_or_else(|| self.no_such_selection(&ask))?;
        if selection.how.changed() {
            // Only on a real move: the announce is what wakes every parked client to re-read, and
            // a select that changed nothing has nothing for them to read.
            self.announce();
        }
        Ok(IntrospectValue::Json(serde_json::json!({
            "pane": selection.pane.0,
            // Kept beside `outcome` rather than replaced by it: it is the one bit every existing
            // client reads, and it is the whole question for one that only has to decide whether to
            // re-project. `SelectHow::changed` is the ONE derivation of it.
            "changed": selection.how.changed(),
            crate::wire::OUTCOME_KEY: selection.how.wire_str(),
        })))
    }

    /// The pane a request means when it names none: the current window's ACTIVE pane.
    ///
    /// The exact mirror of [`window_target`](Self::window_target) one level down, and the reason
    /// the actions below can finally have an optional target at all — before the daemon held an
    /// active pane there was no "here" for a default to resolve to.
    ///
    /// Deliberately NOT used by [`report_agent`](Self::report_agent) or
    /// [`release_agent`](Self::release_agent): those are driven by a PROCESS inside a pane, which
    /// reads its own id from `SPRAG_PANE`, and a default there would let a hook whose environment
    /// was lost report on somebody else's pane. A person's command defaults to where the person is;
    /// a process's must name what it speaks for.
    fn pane_target(&self, map: &Map<String, Value>, key: &str) -> Result<PaneId, InvokeError> {
        match map.get(key) {
            None | Some(Value::Null) => crate::host::active_pane(&self.registry, &self.scope)
                .ok_or_else(|| {
                    refused(format!(
                        "no pane was named and session {:?}'s current window has no active one",
                        self.scope.session()
                    ))
                }),
            Some(value) => value.as_u64().map(PaneId).ok_or(InvokeError::TypeMismatch),
        }
    }

    /// `rename_window {window?, name}` action: rename a window of THIS request's session — tmux
    /// `rename-window`. `window` absent ⇒ the current one; `name` is the new name (required).
    fn rename_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let window = self.window_target(map)?.to_owned();
        let new = map
            .get("name")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        let recorded = lock(&self.registry)
            .rename_window(self.scope.session(), &window, new)
            .map_err(refused)?;
        self.announce();
        // The name that was RECORDED, not the one that was sent — `rename_pane`'s rule (R295) and
        // `rename_session`'s (R302) met a third time, and the last of the three to get it. A window
        // name is trimmed and validated on the way in ([`WindowName`](sprag_terminal::WindowName)),
        // so a caller that echoed its own argument would tell a user the window is called something
        // it is not. R306's prompt paints this answer.
        Ok(IntrospectValue::Json(
            serde_json::json!({ "name": recorded }),
        ))
    }

    /// `rename_session {name}` action: rename THIS request's session — tmux `rename-session`.
    ///
    /// The session renamed is the SCOPE's, so a caller renames what it already named; there is no
    /// second target argument that could disagree with it.
    ///
    /// # The three things that move, and why the order is the order
    ///
    /// 1. **The registry entry**, which is what makes the new name resolve and the old one stop.
    /// 2. **The change CHANNEL** ([`ChannelRegistry::rename`]) — its revision token, its journal
    ///    and every parked wait. Without this the session would be alive under its new name while
    ///    every client parked on it slept forever: the channel map is keyed by NAME.
    /// 3. **The ATTACHMENTS**, so `list-clients` and the per-session viewer badge keep naming a
    ///    session that exists.
    ///
    /// Then the wake, on the NEW name — [`announce`](Self::announce) would bump the scope's, which
    /// is the name this call just retired, minting a fresh empty channel for it and waking nobody.
    ///
    /// The change itself is DERIVED at the dispatch funnel like every other, and it derives as one
    /// rename because a session's identity does not move with its name (`sprag_terminal::SessionId`).
    ///
    /// Answers the name the registry RECORDED (trimmed, validated —
    /// [`SessionName`](sprag_terminal::SessionName)), so a caller reports the address that now
    /// resolves rather than the string it happened to send.
    fn rename_session(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let new = map
            .get("name")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        let from = self.scope.session().to_owned();
        // The RECORDED name, never the argument: a name is trimmed on the way in, so
        // `rename-session "  work  "` lands as `work` and everything below — the channel key, the
        // attachments, the wake, the answer a client prints — has to be about what the registry
        // now holds. Answering the argument is the mistake R295 fixed one level down for
        // `rename_pane`, met again here.
        let to = lock(&self.registry)
            .rename_session(&from, new)
            .map_err(refused)?;
        self.channels.rename(&from, &to);
        if let Some(attachments) = &self.attachments {
            lock(attachments).rename_session(&from, &to);
        }
        self.channels.bump(&to);
        Ok(IntrospectValue::Json(Value::String(to)))
    }

    /// `kill_window {window?}` action: kill a window of THIS request's session — tmux
    /// `kill-window`. `window` absent ⇒ the current one. Killing the session's LAST window ends
    /// the SESSION (the last session ends the daemon), handled through the SAME
    /// [`handle_session_kill`](Self::handle_session_kill) path a `kill_session` uses.
    fn kill_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        // The one destructive verb in this family, so it is the one that gained an IDENTITY first:
        // a client that PAINTED the row it is killing from cannot honestly send the label on it.
        // Absent ⇒ the request's scoped window, `window_target`'s rule kept.
        let subject = WindowRef::read(map)
            .map_err(|_| InvokeError::TypeMismatch)?
            .unwrap_or_else(|| WindowRef::Named(self.scope.window().to_owned()));
        // Bind off-lock so the reaped panes' blocking Drop runs outside the registry lock.
        let outcome = {
            let mut registry = lock(&self.registry);
            match &subject {
                WindowRef::Named(name) => registry.kill_window(self.scope.session(), name),
                WindowRef::Picked(window) => registry.kill_window_id(self.scope.session(), *window),
            }
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(error) => return Err(refused(error)),
        };
        let ended = outcome.ended();
        match outcome {
            WindowKillOutcome::Removed(_panes) => {
                // A non-last window: its drained panes drop here, off-lock; wake clients watching
                // the windows list.
                self.announce();
            }
            WindowKillOutcome::Session(kill) => self.handle_session_kill(kill),
        }
        Ok(IntrospectValue::Json(
            serde_json::json!({ crate::wire::ENDED_KEY: ended.as_wire() }),
        ))
    }

    /// `resize_window {window?, cols?, rows?, adjust_cols?, adjust_rows?, from?}` action: PIN the
    /// size of a window of THIS request's session, or un-pin it — tmux `resize-window`. `window`
    /// absent ⇒ the current one.
    ///
    /// The four spellings are [`SizeRequest`]'s, and every one of them is RESOLVED here rather than
    /// by the caller, because three of them are descriptions that only become a rectangle against
    /// facts this process holds — the window's current size and its clients' reported areas. A CLI
    /// that read those back and did the arithmetic would be a second geometry model in a client,
    /// which is the defect this module exists to remove.
    ///
    /// * `cols` + `rows` — exactly that (tmux `-x`/`-y`). Both together or NEITHER: HALF of a size is
    ///   refused rather than completed from somewhere, the rule `client/size` already follows, since a
    ///   window whose height came from a different decision than its width is a rectangle nobody
    ///   chose.
    /// * `adjust_cols` / `adjust_rows` — signed, relative to what the window currently IS (tmux
    ///   `-L`/`-R`/`-U`/`-D`). Either alone is fine: an unnamed axis is not half a decision, it is
    ///   "leave that edge".
    /// * `from` — a [`WindowSize`] name to fold the clients under (tmux `-a`/`-A`). `manual` is
    ///   refused here: it is the one policy that folds no clients, so as a SOURCE it names nothing.
    /// * none of the above — un-pin.
    ///
    /// Mixing two spellings is refused. They are four ways to name one rectangle, so a request
    /// carrying two is a caller that has not decided, and picking one for them silently is how a
    /// resize ends up somewhere nobody asked for.
    ///
    /// It stores and announces; it resizes nothing itself. The panes follow through the invoke
    /// BOUNDARY's re-derivation, the same one a split and a client's attach go through, so a pinned
    /// window and a derived one reach the panes by ONE path.
    ///
    /// **The announce is NOT independently falsifiable, and measuring said so.** Removing it leaves
    /// the whole PTY suite green, including the tests written for this action — because the
    /// re-derivation resizes panes, a pane resize marks it dirty, and the dirty path bumps. That is
    /// R241's finding about `client/size` repeating one action along: the wake arrives either way, a
    /// reflow late, over a chain nobody states as a contract (it holds only while a window change
    /// always changes some pane's size — false for a window of nothing but floats, and false for a
    /// re-pin the tiling absorbs). It is kept for the earlier, contracted wake and because every
    /// sibling action here announces; it is not kept on the strength of a test.
    fn resize_window(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let window = self.window_target(map)?.to_owned();
        let request = size_request(map)?;
        // The reports FIRST and off the registry lock, then the pin under it — attachments before
        // registry, never nested, the lock order `retile` keeps so neither path can invert the other.
        let reports = self.client_areas();
        let pinned = lock(&self.registry)
            .window(self.scope.session(), &window)
            .and_then(sprag_terminal::Window::manual_size)
            .map(|(cols, rows)| ClientSize { cols, rows });
        // What a relative request moves: what the window IS — the same derivation every client reads,
        // so "wider" means wider than what is on their screen — falling back to the PIN when the
        // window has no derived size at all.
        //
        // The fallback is not a guess. With a pin stored under a policy that does not read it and no
        // client attached, the arbitration honestly answers "no window", and yet the pin is the one
        // rectangle anybody has declared for this window; refusing to move it would make
        // `resize-window -x 100 -y 30` followed by `resize-window -R 20` fail on a detached session,
        // which is the plainest use there is. Under `manual` the two are the same value anyway.
        let current =
            crate::window::arbitrate(crate::config::window_size(), &reports, pinned).or(pinned);
        let size = request
            .resolve(current, &reports)
            .map_err(refused)?
            .map(|size| (size.cols, size.rows));
        lock(&self.registry)
            .resize_window(self.scope.session(), &window, size)
            .map_err(refused)?;
        self.announce();
        // ANSWER with the rectangle that was pinned, `null` for an un-pin. Three of the four
        // spellings are descriptions the caller cannot resolve — that is why they are resolved here —
        // so a caller told only "accepted" would have to guess what it had asked for, or read the
        // window back and race the next change. The resolver knows; it says.
        Ok(IntrospectValue::Json(match size {
            Some((cols, rows)) => json!({ "cols": cols, "rows": rows }),
            None => Value::Null,
        }))
    }

    /// The cell areas this request's session's clients have reported — the arbitration's inputs, or
    /// empty for a host that tracks no wire clients (the in-process one, which has a single surface
    /// and never needed arbitrating).
    fn client_areas(&self) -> Vec<ClientSize> {
        self.attachments
            .as_ref()
            .map(|attachments| lock(attachments).sizes(self.scope.session()))
            .unwrap_or_default()
    }

    /// `break_pane {pane, name?}` action: move a pane out of its window into a NEW window of THIS
    /// request's session (born current), and answer with the new window's name — tmux `break-pane`.
    ///
    /// The pane's SOURCE window is derived from its id in the registry (a `PaneId` is unique across
    /// the registry), so the wire carries only the pane and an optional new-window name. The move
    /// is whole (no re-spawn) and runs under the registry lock; nothing blocks (a break drops no
    /// pane). Every client watching the windows list wakes on the revision bump.
    fn break_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let pane = require_pane_id(map, "pane")?;
        let name = match map.get("name") {
            None => None,
            Some(Value::String(name)) => Some(name.as_str()),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let created = lock(&self.registry)
            .break_pane(self.scope.session(), pane, name)
            // A rejection is well-formed but cannot be honored, and `PaneMoveError` has an arm per
            // way: its `Display` is what the caller prints, so the CLI's old three-cause guess
            // (*"is its window's only pane, no window holds it, or the name is taken"*) is now the
            // one the registry actually decided.
            .map_err(refused)?;
        self.announce();
        Ok(IntrospectValue::Json(Value::String(created)))
    }

    /// `join_pane {pane, window}` XOR `{pane, window_id}` action: move a pane into another window of
    /// THIS request's session, appending it as a tiled leaf — tmux `join-pane`. Answers
    /// `{closed_source}` (whether the join emptied and closed the pane's old window).
    ///
    /// The pane's SOURCE window is derived from its id; the wire carries the pane and the
    /// DESTINATION — as a NAME a caller typed or as the IDENTITY a caller picked, which is
    /// [`JoinAsk`]'s whole reason and [`crate::wire::JOIN_PANE_ACTION`]'s. Whole move, under the
    /// registry lock; the revision bump wakes every client (a closed source window drops out of
    /// their windows list on the next read).
    ///
    /// The grammar is parsed by a type rather than key by key, [`swap_pane`](Self::swap_pane)'s rule
    /// and for its reason: no combination this parse admits needs checking again below, and the
    /// two spellings cannot both arrive.
    fn join_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let value = match args {
            IntrospectValue::Json(value) => value,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let JoinAsk { pane, window } = JoinAsk::parse(value).ok_or(InvokeError::TypeMismatch)?;
        let mut registry = lock(&self.registry);
        let closed = match &window {
            WindowRef::Named(name) => registry.join_pane(self.scope.session(), pane, name),
            WindowRef::Picked(window) => {
                registry.join_pane_into(self.scope.session(), pane, *window)
            }
        }
        .map_err(refused)?;
        drop(registry);
        self.announce();
        Ok(IntrospectValue::Json(
            serde_json::json!({ "closed_source": closed }),
        ))
    }

    /// `move_pane {pane, target, dir, before?}` action: place an existing pane beside another —
    /// tmux `move-pane`. Answers `{closed_source}`. See [`crate::wire::MOVE_PANE_ACTION`].
    ///
    /// NEITHER window is named: both are derived from the two pane ids, so the one request covers a
    /// re-placement inside one window and a move into another. The direction vocabulary is
    /// [`split`](Self::split)'s, parsed here the same way — an absent `before` is the common side
    /// and a non-bool is malformed rather than silently defaulted.
    fn move_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let pane = self.pane_target(map, "pane")?;
        let target = require_pane_id(map, "target")?;
        let (side, dir) = Self::parse_placement(map)?;
        let closed = lock(&self.registry)
            .move_pane(self.scope.session(), pane, target, side, dir)
            .map_err(refused)?;
        self.announce();
        Ok(IntrospectValue::Json(
            serde_json::json!({ "closed_source": closed }),
        ))
    }

    /// `swap_pane {pane?, with}` XOR `{pane?, dir}` action: exchange two panes' positions — tmux
    /// `swap-pane`. Answers `{a, b, changed, outcome}`. See [`crate::wire::SWAP_PANE_ACTION`].
    ///
    /// The naming shape is [`select_pane`](Self::select_pane)'s, parsed by
    /// [`SwapAsk`] rather than key by key, and so is the split between a
    /// refusal and a quiet "nothing moved": a direction that finds no neighbour is a well-formed
    /// request at the edge of a layout, while a pane id naming nothing is a caller's mistake.
    ///
    /// **A pane the session does not hold is refused in BOTH arms**, which is a fix rather than a
    /// restatement: the direction arm used to answer `{a: <that id>, b: null, changed: false}` —
    /// success, about a pane that does not exist — because the registry's old `neighbor_of` gave one
    /// `None` for an unheld pane, a floating one and an edge. It answers a
    /// [`PaneStep`](sprag_terminal::PaneStep) inside an [`Option`] now, so the refusal and the two
    /// nothings are three different values at the one place that can tell them apart.
    fn swap_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let value = match args {
            IntrospectValue::Json(value) => value,
            IntrospectValue::Null => &Value::Null,
            _ => return Err(InvokeError::TypeMismatch),
        };
        // Exactly one partner, for `select_pane`'s reason: "swap with nothing" and "swap with two
        // things" are a caller's bug, and guessing a reading for them would hide it. The type is
        // what refuses them, so no combination this parse admits needs checking again below.
        let ask = SwapAsk::parse(value).ok_or(InvokeError::TypeMismatch)?;
        let swap = crate::host::swap_pane(&self.registry, &self.scope, ask)
            .ok_or_else(|| self.no_such_swap(&ask))?;
        if swap.how.changed() {
            // Only on a real move, `select_pane`'s rule: the announce wakes every parked client to
            // re-read, and a swap that traded nothing has nothing for them to read.
            self.announce();
        }
        Ok(IntrospectValue::Json(serde_json::json!({
            "a": swap.a.0,
            "b": swap.b.map_or(Value::Null, |pane| Value::from(pane.0)),
            // Kept beside `outcome` rather than replaced by it, `select_pane`'s rule: it is the one
            // bit every existing client reads, and the whole question for one that only has to
            // decide whether to re-read the arrangement. `SwapHow::changed` is its ONE derivation.
            "changed": swap.how.changed(),
            crate::wire::OUTCOME_KEY: swap.how.wire_str(),
        })))
    }

    /// `resize_pane {pane?, dir, cells?}` action: move the boundary that bounds `pane` on that
    /// axis — tmux `resize-pane -L|-R|-U|-D`. Answers `{pane, cells, outcome}`. See
    /// [`crate::wire::RESIZE_PANE_ACTION`].
    ///
    /// [`swap_pane`](Self::swap_pane)'s shape: the grammar is parsed by a type rather than key by
    /// key, and the same line is drawn between a REFUSAL and a quiet "nothing moved" — a direction
    /// with no boundary to move is a well-formed request at the edge of a layout, where a pane id
    /// naming nothing is a caller's mistake.
    ///
    /// **It needs the ATTACHMENTS**, unlike every other arrangement action here, because a cell has
    /// no length until the window has a size and the size is arbitrated across every attached
    /// client. An external built without them cannot answer — the same `None` a window with no
    /// reported area gives, and for the same reason rather than by coincidence.
    fn resize_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let value = match args {
            IntrospectValue::Json(value) => value,
            IntrospectValue::Null => &Value::Null,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let ask = ResizeAsk::parse(value).ok_or(InvokeError::TypeMismatch)?;
        let attachments = self
            .attachments
            .as_ref()
            .ok_or_else(|| refused(NO_CLIENTS))?;
        let resize = crate::host::resize_pane(&self.registry, attachments, &self.scope, ask)
            .map_err(refused)?;
        if resize.how.changed() {
            // Only on a real move, `select_pane`'s rule: the announce wakes every parked client to
            // re-read, and a boundary that did not move has nothing for them to read.
            self.announce();
        }
        Ok(IntrospectValue::Json(serde_json::json!({
            "pane": resize.pane.0,
            // How far it ACTUALLY went, which is below what was asked when it ran into the last
            // cell a side may keep — so a caller learns it was clamped without holding a second
            // copy of where the limit is.
            "cells": resize.cells,
            crate::wire::OUTCOME_KEY: resize.how.wire_str(),
        })))
    }

    /// `zoom_pane {pane?, on?}` action: fill the window that holds `pane` with it alone, or end
    /// that window's zoom — tmux `resize-pane -Z`. Answers `{pane, zoomed, changed}`. See
    /// [`crate::wire::ZOOM_PANE_ACTION`].
    ///
    /// `changed` comes from the registry, which compared the two zoom TARGETS rather than two
    /// flags — so the two edges fall out instead of being special-cased: a floating target and a
    /// re-assertion of the state already in force both leave the window where it was, report
    /// `false`, and wake nobody.
    fn zoom_pane(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let empty = Map::new();
        let map = match args {
            IntrospectValue::Json(Value::Object(m)) => m,
            IntrospectValue::Null => &empty,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let pane = self.pane_target(map, "pane")?;
        // Absent TOGGLES; a non-bool is malformed rather than silently defaulted, the rule every
        // other optional flag on this external follows (`parse_placement`'s `before`).
        let on = match map.get("on") {
            None | Some(Value::Null) => None,
            Some(Value::Bool(on)) => Some(*on),
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        let outcome = lock(&self.registry)
            .zoom_pane(self.scope.session(), pane, on)
            .ok_or_else(|| {
                refused(format!(
                    "session {:?} holds no pane {}",
                    self.scope.session(),
                    pane.0
                ))
            })?;
        if outcome.changed {
            // Only on a real change, `select_pane`'s rule: the announce wakes every parked client
            // to re-read, and a zoom that moved nothing has nothing for them to read.
            self.announce();
        }
        Ok(IntrospectValue::Json(serde_json::json!({
            "pane": pane.0,
            "zoomed": outcome.zoomed,
            "changed": outcome.changed,
        })))
    }

    /// The `{dir, before?}` half of a placement request — [`split`](Self::split)'s and
    /// [`move_pane`](Self::move_pane)'s, parsed once so the two cannot drift into two spellings of
    /// one vocabulary.
    ///
    /// `dir` names how the two halves are LAID OUT (tmux's own `-h` / `-v`), and `before` is tmux's
    /// `-b`: absent is the common side (right / below), and a non-bool is malformed rather than
    /// silently defaulted — the rule every other optional flag on this external follows.
    fn parse_placement(map: &Map<String, Value>) -> Result<(SplitSide, SplitDir), InvokeError> {
        let dir = match map.get("dir").and_then(Value::as_str) {
            Some("horizontal") => SplitDir::Horizontal,
            Some("vertical") => SplitDir::Vertical,
            _ => return Err(InvokeError::TypeMismatch),
        };
        let side = match map.get("before") {
            None | Some(Value::Null) | Some(Value::Bool(false)) => SplitSide::Second,
            Some(Value::Bool(true)) => SplitSide::First,
            Some(_) => return Err(InvokeError::TypeMismatch),
        };
        Ok((side, dir))
    }

    /// `drop_file {pane, path}` action: deliver a file dropped on a display client to `pane`, and
    /// answer `{path}` — the path the pane is handed ([`crate::upload`] owns the paste-vs-upload
    /// policy). A refused delivery (no such pane, an unresolvable path) is `Rejected`.
    ///
    /// The pane is resolved to its PTY handle + recorded remote under the workspace lock, and the
    /// guard is dropped BEFORE the delivery runs: an upload spawns a thread and a local drop writes
    /// to the PTY, neither of which may hold the pool other clients are reading.
    ///
    /// No revision bump: the pane's own output (its shell echoing the pasted path) bumps it through
    /// the spawn-time dirty hook, and an upload's paste lands long after this returns — a bump here
    /// would announce a change that has not happened yet.
    fn drop_file(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let map = as_object(args)?;
        let pane = require_pane_id(map, "pane")?;
        let path = map
            .get("path")
            .and_then(Value::as_str)
            .ok_or(InvokeError::TypeMismatch)?;
        let target = {
            let workspace = lock(self.workspace());
            let held = workspace.pane(pane).ok_or_else(|| {
                refused(format!(
                    "session {:?}'s current window holds no pane {}",
                    self.scope.session(),
                    pane.0
                ))
            })?;
            (held.handle(), held.remote().cloned())
        };
        let (handle, remote) = target;
        let delivered =
            crate::upload::deliver(handle, remote, Path::new(path)).ok_or_else(|| {
                refused(format!(
                    "{path:?} could not be resolved to a deliverable file"
                ))
            })?;
        Ok(IntrospectValue::Json(
            serde_json::json!({ "path": delivered }),
        ))
    }
}

impl fmt::Debug for WorkspaceExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkspaceExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(WorkspaceExternal);

impl ExternalIntrospect for WorkspaceExternal {
    fn schema(&self) -> IntrospectSchema {
        // Declared in `wire`, beside the addresses and beside the pane surface's own
        // ([`PANE_SCHEMA`](crate::wire::PANE_SCHEMA)) — this vocabulary has ONE home, and the
        // ratchet that keeps the wire's surface from moving under the protocol number reads it
        // from there rather than from a second copy.
        IntrospectSchema::new(crate::wire::MUX_SCHEMA)
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        // The reads whose subject is the REGISTRY, answered FIRST and through the ONE place any of
        // them is produced ([`RegistryView`]) — which is also the surface a reader whose own session
        // has gone is served from, so the two doors cannot come to answer `sessions` differently.
        // The parametric families (`session_activity.<max_age>`, `pane_processes.<max_age>`) live in
        // there too and are matched by prefix ahead of everything, which is why this delegation is
        // the first statement rather than an arm of the match below.
        if let Some(answer) = self.registry_view().query(path) {
            return Some(answer);
        }
        match path {
            PANES_SLOT => {
                // The DTOs and each pane's PROJECTION TOKEN, read under ONE workspace lock so the
                // token a client compares describes the same moment as the rest of its entry. A
                // token read later than the pane list could only ever be NEWER than the frame the
                // client goes on to fetch, which is the one direction that serves a stale pane.
                let (panes, tokens, agents) = {
                    let guard = lock(self.workspace());
                    let tokens: std::collections::HashMap<u64, sprag_grid::ProjectionToken> = guard
                        .panes()
                        .iter()
                        .map(|pane| {
                            (
                                pane.id().0,
                                pane.pty().with_screen_palette(sprag_grid::projection_token),
                            )
                        })
                        .collect();
                    // The agent verdicts, under the SAME lock and for the same reason the tokens
                    // are: the screen a rule read and the entry describing it have to be the same
                    // moment. It leaves as a map keyed by pane id rather than on `PaneInfo`, so the
                    // producer's DTO stays what its doc says it is and no `sprag_detect` type enters
                    // `sprag-terminal`.
                    //
                    // The window is a user OPTION read from the file, so it is read LAZILY — at most
                    // once for this whole walk, and not at all when every pane is settled. See
                    // `AgentRegistry::observe`.
                    let agents = self.agents.as_ref().map(|agents| {
                        let mut window: Option<sprag_detect::Hysteresis> = None;
                        let now = Instant::now();
                        guard
                            .panes()
                            .iter()
                            .filter_map(|pane| {
                                // The CHILD's own title, never `pane.name()` beside it — a name is
                                // chosen by whoever asked for the pane, and `claude`'s first
                                // fingerprint is one condition on the title alone, so reading a
                                // name here would let anyone who can name a pane forge an agent
                                // identity. Pinned by
                                // `a_name_that_looks_like_an_agents_title_claims_nothing`.
                                let title = pane.title();
                                let facts = pane.pty().with_screen(|screen| {
                                    agents.observe(pane.id(), screen, title.as_deref(), now, || {
                                        *window.get_or_insert_with(crate::config::agent_settle)
                                    })
                                })?;
                                Some((pane.id().0, facts))
                            })
                            .collect::<std::collections::HashMap<u64, crate::AgentFacts>>()
                    });
                    (guard.list(), tokens, agents)
                };
                // AFTER the workspace guard above has dropped: this reconciles the window under the
                // REGISTRY lock, and the two are never nested (see `crate::host::active_pane`).
                // Reconciled rather than read raw, so a pane that has just exited cannot be marked
                // active in the very list that no longer contains it.
                let active = crate::host::active_pane(&self.registry, &self.scope);
                let entries = panes
                    .iter()
                    .map(|p| {
                        let mut entry = serde_json::json!({
                            "id": p.id,
                            "cols": p.cols,
                            "rows": p.rows,
                            "command": p.command_label,
                            // The child's live OSC 0/2 window title, `null` until it sets
                            // one. A DISPLAY name (a client prefers it over the command
                            // label and falls back); never identity — the child sets it.
                            "title": p.title,
                        });
                        // The name a PERSON gave the pane. ADDITIVE on the terms every key below
                        // keeps: present only on a pane somebody named, so a workspace where
                        // nobody has is byte-identical to the pre-name wire shape.
                        //
                        // Unlike `title` beside it this one IS identity — it is unique across the
                        // registry and a surface may resolve it back to this pane — which is why a
                        // display client prefers it OVER the title rather than the other way
                        // round: a name a person chose outranks one the child rewrites on every
                        // prompt.
                        if let Some(name) = &p.name {
                            entry["name"] = serde_json::json!(name);
                        }
                        // The most recent attention notification (OSC 9 / 777;notify / 99),
                        // with its monotonic `seq`. ADDITIVE: the key is present only when the
                        // child has raised one, so a pane that never did is byte-identical to
                        // the pre-notification wire shape. A client detects a NEW one by the
                        // `seq` growing past the last it acknowledged (the attention badge).
                        if let Some(note) = &p.notification {
                            entry["notification"] = serde_json::json!({
                                "title": note.title,
                                "body": note.body,
                                "seq": p.notification_seq,
                            });
                        }
                        // The tmux monitor-bell count (`\a`), kept SEPARATE from the
                        // notification (a bell carries no text). ADDITIVE: present only once the
                        // child has rung one, so a pane that never did is byte-identical to the
                        // pre-bell wire shape. A viewer's "unseen attention" combines this with
                        // the notification `seq`.
                        if p.bell_seq > 0 {
                            entry["bell_seq"] = serde_json::json!(p.bell_seq);
                        }
                        // Whether this is the window's ACTIVE pane — tmux's `select-pane` target,
                        // the daemon's answer to "here". ADDITIVE on the same terms as every key
                        // around it: present only on the ONE row it is true of, so a client that
                        // has never heard of it reads the pre-active wire shape. Exactly one row
                        // carries it because these rows ARE the current window's panes.
                        if Some(PaneId(p.id)) == active {
                            entry["active"] = serde_json::json!(true);
                        }
                        // Whether the pane's child has EXITED. ADDITIVE and one-way: the key is
                        // present only once it is true (a live pane is byte-identical to the
                        // pre-liveness wire shape), and a pane never comes back to life, so a
                        // client that has seen it needs no re-check.
                        if p.dead {
                            entry["dead"] = serde_json::json!(true);
                        }
                        // The pane whose OCCUPANT asked for this one. ADDITIVE: present only for a
                        // pane somebody claims, so a workspace no agent has touched is
                        // byte-identical to the pre-provenance wire shape. Unlike every key around
                        // it this one is fixed at BIRTH and never moves again, which is what makes
                        // it safe to gate a destructive verb on: a reader acting on it cannot be
                        // acting on a fact that changed under it.
                        if let Some(opener) = p.opened_by {
                            entry["opened_by"] = serde_json::json!(opener);
                        }
                        // ...and HOW it exited, once the child has been reaped. A SECOND key rather
                        // than a richer `dead`, because it is a second fact that arrives later and
                        // may never arrive at all (a child that hands its pty on and lingers) — see
                        // `PaneExit`. ADDITIVE on the same terms, and `code` is always written while
                        // `signal` rides only a signalled death, so a plain exit stays two fields.
                        //
                        // `child_exit`, not `exit`, to keep it clear of `exit_status` below: that
                        // one is the OSC 133 status of the last command the SHELL ran, and the two
                        // answer opposite questions about a stopped pane.
                        if let Some(exit) = &p.child_exit {
                            let mut value = serde_json::json!({ "code": exit.code });
                            if let Some(signal) = &exit.signal {
                                value["signal"] = serde_json::json!(signal);
                            }
                            entry["child_exit"] = value;
                        }
                        // Shell-integration (OSC 133) summary: the idle/running state and the last
                        // command's exit status. ADDITIVE — the state key is present only when the
                        // child emitted a mark (`wire_str` returns `None` for `Unknown`), and the
                        // exit status only when a command finished with one, so a pane without
                        // shell integration is byte-identical to the pre-OSC133 wire shape.
                        if let Some(shell) = p.shell_state.wire_str() {
                            entry["shell"] = serde_json::json!(shell);
                        }
                        if let Some(status) = p.last_exit_status {
                            entry["exit_status"] = serde_json::json!(status);
                        }
                        // Mouse-tracking mode (DECSET 1000/1002/1003). ADDITIVE — the key is present
                        // only while the child is tracking (`wire_str` returns `None` for the
                        // resting `MouseProtocol::None`), so a pane that never enabled mouse
                        // reporting is byte-identical to the pre-mouse wire shape. A display client
                        // reads it to decide whether to capture the pointer for reporting.
                        if let Some(mouse) = p.mouse_protocol.wire_str() {
                            entry["mouse"] = serde_json::json!(mouse);
                        }
                        // Focus-tracking mode (DECSET 1004). ADDITIVE — the key is present only while
                        // the child is tracking focus, so a pane that never enabled it is
                        // byte-identical to the pre-focus wire shape (mirrors `mouse`). A display
                        // client reads it to decide whether to emit a focus edge on a pane focus
                        // change; an agent reads it to learn the app reacts to focus.
                        if p.focus_tracking {
                            entry["focus_tracking"] = serde_json::json!(true);
                        }
                        // OSC 52 clipboard signals. The write SEQ travels here (ADDITIVE, present
                        // only once the child has written a clipboard); the write PAYLOAD does NOT
                        // — it can be a whole paste, so a client fetches it on demand off this seq
                        // via the `clipboard_write` pane slot. A pane whose child never wrote is
                        // byte-identical to the pre-OSC52 wire shape.
                        if p.clipboard_write_seq > 0 {
                            entry["clipboard_write_seq"] = serde_json::json!(p.clipboard_write_seq);
                        }
                        // A pending OSC 52 READ query: the single selection the child asked to
                        // read back (`c`/`p`) + its seq. Tiny, so — unlike the write — it travels
                        // inline. ADDITIVE: present only once the child has issued a read.
                        if let Some(query) = p.clipboard_query {
                            entry["clipboard_query"] = serde_json::json!({
                                "sel": query.target.osc_char().to_string(),
                                "seq": p.clipboard_query_seq,
                            });
                        }
                        // Inline images (Kitty graphics / Sixel, R1404). ADDITIVE: present only once
                        // the child has transmitted one, so an image-less pane is byte-identical to
                        // the pre-R1404 wire shape. Each entry is a SUMMARY — `{id, width, height,
                        // anchor, seq}`, NO rgba (R1404 Stage 5): the RGBA is up to a MiB, so a
                        // display client fetches it ON DEMAND via `image_data.<id>` keyed on
                        // `(id, seq)`, not per poll (the `clipboard_write` payload precedent). An
                        // agent reads the summary here to learn "a WxH image sits at cell (col,row)".
                        if !p.images.is_empty() {
                            let images: Vec<Value> = p
                                .images
                                .iter()
                                .map(|img| {
                                    serde_json::json!({
                                        "id": img.id,
                                        "width": img.width,
                                        "height": img.height,
                                        "anchor": [img.anchor.0, img.anchor.1],
                                        "seq": img.seq,
                                    })
                                })
                                .collect();
                            entry["images"] = Value::Array(images);
                        }
                        // What a fetch of this pane's CELLS would depend on
                        // ([`sprag_grid::ProjectionToken`]) — the per-row damage stamps, the
                        // cursor (colour included), the screen kind, the width and the history
                        // depth. A display client that already holds a frame for this pane and
                        // sees an unchanged token can SKIP the fetch: the frame it would receive
                        // is the one it has. ADDITIVE, and its absence means "fetch anyway", so an
                        // older daemon — or a token that failed to serialize — costs a redundant
                        // fetch and never a stale pane.
                        if let Some(token) =
                            tokens.get(&p.id).and_then(|t| serde_json::to_value(t).ok())
                        {
                            entry["projection"] = token;
                        }
                        // The agent this pane is running and what it is doing (H3). ADDITIVE: the key
                        // is present only for a pane some manifest CLAIMS and some rule answered for,
                        // so a workspace of shells is byte-identical to the pre-H3 wire shape — and
                        // the absence is carried by `AgentRegistry::observe` returning nothing rather
                        // than by this site remembering to check, so it cannot drift.
                        //
                        // `state` is the answer a person wants ("which pane is waiting on me"),
                        // `rule` is what makes it diagnosable (D7 — a gate that cannot say what it saw
                        // cannot be debugged, and this is `explain`'s whole content), and `seq` moves
                        // on a published CHANGE so a client tells "still blocked" from "blocked again"
                        // without diffing strings — `notification_seq`'s treatment exactly.
                        if let Some(facts) = agents.as_ref().and_then(|map| map.get(&p.id)) {
                            let mut value = serde_json::json!({
                                "state": facts.state,
                                "seq": facts.seq,
                            });
                            if let Some(name) = &facts.agent {
                                value["name"] = serde_json::json!(name);
                            }
                            if let Some(rule) = &facts.rule {
                                value["rule"] = serde_json::json!(rule);
                            }
                            // WHO said so, for a verdict that was REPORTED rather than inferred. The
                            // counterpart of `rule` for the other kind of evidence: a reported verdict
                            // carries no rule and a scraped one carries no source, so a reader never
                            // has to guess which authority answered.
                            if let Some(source) = &facts.source {
                                value["source"] = serde_json::json!(source);
                            }
                            entry["agent"] = value;
                        }
                        entry
                    })
                    .collect();
                Some(IntrospectValue::Json(Value::Array(entries)))
            }
            // The SCOPED session's current-window LOGICAL arrangement (no pixels) + its
            // revision: what a display client projects, and what lets a reattaching client
            // restore the layout. Reconciled against the live pool first, since pane
            // lifecycle runs through the Workspace directly and the tree is not the
            // membership authority.
            LAYOUT_SLOT => {
                layout_value(crate::host::reconciled_layout(&self.registry, &self.scope)?)
            }
            // The NAME of the session this request is scoped to. Read straight off the scope rather
            // than re-derived: the scope resolved it once, at the door, under the registry lock, and
            // the whole value of the slot is that it cannot disagree with what the same request's
            // other slots answered about.
            SESSION_SLOT => Some(IntrospectValue::Json(Value::String(
                self.scope.session().to_owned(),
            ))),
            // The SCOPED session's arbitrated window — the rectangle every client tiles over, so
            // that two clients of different sizes give one pane one size. The POLICY is read from
            // the user's file here (no option crosses the wire); the clients' areas come from the
            // same dispatch-layer attachment map that fills `clients` and `attached`.
            //
            // `null` off a daemon that tracks no wire clients, and equally when no attached client
            // has reported an area yet. Both mean "nobody has said how big this is", and a client
            // reading it leaves its panes at the size they already have — an in-process host has
            // exactly one surface, so it never needed arbitrating in the first place.
            WINDOW_SIZE_SLOT => {
                let window = self.attachments.as_ref().and_then(|attachments| {
                    let sizes = lock(attachments).sizes(self.scope.session());
                    // The PINNED size of the window this request was assembled for — a decision an
                    // operator stored, where the areas above are reports clients made.
                    let pinned = lock(&self.registry)
                        .window(self.scope.session(), self.scope.window())
                        .and_then(sprag_terminal::Window::manual_size)
                        .map(|(cols, rows)| ClientSize { cols, rows });
                    crate::window::arbitrate(crate::config::window_size(), &sizes, pinned)
                });
                encoded_answer(&window, "window_size")
            }
            // The SCOPED session's windows — each window's name and whether it is the CURRENT
            // one — how a tabbed client learns which tabs to draw and which is active. Scoped
            // (unlike `sessions`): windows are a property of a session, so this answers about the
            // one the request named. `None` only if that session has since gone (a killed scope),
            // which within a single request is unreachable.
            WINDOWS_SLOT => {
                let registry = lock(&self.registry);
                let infos = registry.session(self.scope.session())?.window_infos();
                // One `WindowInfo` shape, shared with a client's mirror and the in-process arm —
                // serialised here the way the `layout` slot serialises its snapshot.
                encoded_answer(&infos, "windows")
            }
            // The project governing ONE pane: the commands its `.sprag.toml` declares. Parametric,
            // so it is matched after the fixed slots above (`project.<pane>`, see `PROJECT_FIELD`
            // for why this lives on the mux external rather than the pane's own).
            // What has CHANGED in the scoped session since a cursor — parametric like the project
            // slot, and a QUERY so that reading the log cannot advance the token the log is keyed
            // by (see `EVENTS_FIELD`).
            path if path.starts_with(EVENTS_FIELD.literal_prefix()) => {
                let arg = path
                    .strip_prefix(EVENTS_FIELD.literal_prefix())
                    .expect("the guard just matched this prefix");
                // A malformed member of a family this surface ADVERTISES is `Null`
                // (present-but-empty), never `None`: `None` becomes `UnknownIntrospectPath`, whose
                // meaning is "not in its schema", and `events.zzz` IS in the schema. The same
                // taxonomy `cells.<offset>` had to be corrected into by R155's review.
                let Ok(since) = arg.parse::<u64>() else {
                    return Some(IntrospectValue::Null);
                };
                Some(events_value(&self.channels, self.scope.session(), since))
            }
            // What is ADJACENT to one pane. Parametric like the two above, and answered from the
            // ARRANGEMENT rather than from any client's rectangles — see `NEIGHBORS_FIELD`.
            path if path.starts_with(NEIGHBORS_FIELD.literal_prefix()) => {
                let arg = path
                    .strip_prefix(NEIGHBORS_FIELD.literal_prefix())
                    .expect("the guard just matched this prefix");
                // Present-but-empty for a malformed member of a family this surface ADVERTISES,
                // never `None` — `events.<since>` states the taxonomy.
                let Ok(pane) = arg.parse::<u64>() else {
                    return Some(IntrospectValue::Null);
                };
                Some(neighbors_value(&self.registry, &self.scope, PaneId(pane)))
            }
            path => {
                let pane = path.strip_prefix("project.")?.parse::<u64>().ok()?;
                Some(project_value(self.workspace(), PaneId(pane)))
            }
        }
    }

    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots. Pane management and the arrangement write are both
        // action-shaped (invoke): neither is a plain assignment — a spawn answers with a
        // new id, and an arrangement write names the client's dividers, validates the
        // shape, and answers with the canonical tree.
        Err(InterveneError::UnknownPath)
    }

    /// Dispatch a mux action, then give every tiled pane the size the session's window says it has
    /// (`window::retile`).
    ///
    /// Re-derived HERE, at the boundary, rather than inside each action: every action above can
    /// move the arrangement — a split adds a share, a close returns one, a `set_layout` re-weights
    /// them, a `select_window` changes which tree is showing — and a derivation some actions
    /// performed and others forgot is the asymmetry R237 named. One site cannot forget.
    ///
    /// It runs after the action's own work has returned, so no pool lock is held across it: the
    /// re-derivation takes the attachment, registry and pool locks in turn, and taking one here
    /// while an action still held another is the deadlock this placement avoids by construction.
    ///
    /// Only on SUCCESS. A refused action changed nothing, and re-deriving after it would answer a
    /// question that was never asked.
    fn invoke(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        let answer = self.dispatch(path, args);
        if answer.is_ok()
            && let Some(attachments) = self.attachments.as_ref()
        {
            crate::window::retile(&self.registry, attachments, self.scope.session());
        }
        answer
    }
}

impl WorkspaceExternal {
    /// The action table itself — [`Self::invoke`]'s match, split out so the boundary can act on
    /// what an action answered without wrapping every arm.
    fn dispatch(
        &mut self,
        path: &str,
        args: IntrospectValue,
    ) -> Result<IntrospectValue, InvokeError> {
        match path {
            SPAWN_ACTION => self.spawn(&args),
            SPLIT_ACTION => self.split(&args),
            CLOSE_ACTION => self.close(&args),
            RESIZE_ACTION => self.resize(&args),
            RENAME_PANE_ACTION => self.rename_pane(&args),
            SET_LAYOUT_ACTION => self.set_layout(&args),
            SET_FLOATING_ACTION => self.set_floating(&args),
            NEW_SESSION_ACTION => self.new_session(&args),
            KILL_SESSION_ACTION => self.kill_session(&args),
            NEW_WINDOW_ACTION => self.new_window(&args),
            SELECT_WINDOW_ACTION => self.select_window(&args),
            MOVE_WINDOW_ACTION => self.move_window(&args),
            SELECT_PANE_ACTION => self.select_pane(&args),
            RENAME_WINDOW_ACTION => self.rename_window(&args),
            RENAME_SESSION_ACTION => self.rename_session(&args),
            KILL_WINDOW_ACTION => self.kill_window(&args),
            RESIZE_WINDOW_ACTION => self.resize_window(&args),
            BREAK_PANE_ACTION => self.break_pane(&args),
            JOIN_PANE_ACTION => self.join_pane(&args),
            MOVE_PANE_ACTION => self.move_pane(&args),
            SWAP_PANE_ACTION => self.swap_pane(&args),
            RESIZE_PANE_ACTION => self.resize_pane(&args),
            ZOOM_PANE_ACTION => self.zoom_pane(&args),
            DROP_FILE_ACTION => self.drop_file(&args),
            REPORT_AGENT_ACTION => self.report_agent(&args),
            RELEASE_AGENT_ACTION => self.release_agent(&args),
            DISPLAY_MESSAGE_ACTION => self.display_message(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// The `project.<pane>` answer for one pane: the commands the project it sits in declares.
///
/// Three outcomes, each distinct on the wire (see [`crate::wire::PROJECT_FIELD`]): `null` for a pane in no
/// project — or one whose cwd is not local, or that has since gone; the project object for a
/// usable config; and
/// `{error}` for a project whose config is unusable, because a typo must be reported rather than
/// look like "this project declares nothing".
///
/// The workspace lock is taken ONLY to read the two facts the registry owns (is this pane remote,
/// and where is it) and is DROPPED before the filesystem walk. A config read is IO — holding a
/// registry lock across it would stall every other request behind someone else's slow disk, the
/// same discipline the file-drop upload follows by handing its thread only a PTY handle.
fn project_value(workspace: &Arc<Mutex<Workspace>>, pane: PaneId) -> IntrospectValue {
    let cwd = {
        let pool = lock(workspace);
        let Some(pane) = pool.pane(pane) else {
            // A pane the caller named that this window does not hold (closed, or another window's).
            return IntrospectValue::Null;
        };
        if pane.remote().is_some() {
            // A REMOTE workspace's working directory is on another machine, so walking THIS
            // filesystem for a `.sprag.toml` would either find nothing or — worse — find the local
            // project the daemon happens to sit in and offer its commands for a remote shell.
            return IntrospectValue::Null;
        }
        pane.pty().cwd()
    };
    let Some(cwd) = cwd else {
        // No readable cwd: the child has exited, or the platform has no `/proc`.
        return IntrospectValue::Null;
    };
    match crate::project::load(&cwd) {
        None => IntrospectValue::Null,
        Some(Ok(project)) => encoded_answer(&project, "project").unwrap_or(IntrospectValue::Null),
        Some(Err(error)) => IntrospectValue::Json(serde_json::json!({
            "error": error.to_string(),
        })),
    }
}

/// Serialise the USER's declared commands for the wire — the same three-way answer
/// [`project_value`] gives, over the user's config instead of a pane's project: `null` for "no
/// config written", an `{error}` object for one that cannot be used, and the config itself
/// otherwise.
///
/// The error is RENDERED here rather than sent structurally, exactly as the project's is, because
/// what a client needs is the sentence to show — and rendering it host-side is what makes it name
/// `config.toml` ([`crate::ConfigError`]) rather than whichever file the client guessed.
fn global_commands_value() -> IntrospectValue {
    match crate::config::load() {
        None => IntrospectValue::Null,
        Some(Ok(config)) => encoded_answer(&config, "user config").unwrap_or(IntrospectValue::Null),
        Some(Err(error)) => IntrospectValue::Json(serde_json::json!({
            "error": error.to_string(),
        })),
    }
}

/// Serialise the daemon's verdict on the user's agent manifests: an `{error}` object naming why the
/// ruleset in force is not the one `config.toml` declares, or `null` when it is.
///
/// The `{error}` shape is [`global_commands_value`]'s, deliberately, because a client meets the
/// three of them in one list and a third spelling would be a third parser. What it does NOT share is
/// the disk read: the sentence was rendered when the daemon last read the file, so this is a lock
/// and a clone (see [`AGENT_MANIFESTS_SLOT`]).
///
/// `None` agents is an in-process host with no detector at all — no manifests, so nothing to report,
/// which is the same `null` a working file gives. The two are indistinguishable ON PURPOSE: a client
/// paints nothing in both cases, and a host that cannot detect agents has no verdict to defend.
fn agent_manifests_value(agents: Option<&crate::AgentClock>) -> IntrospectValue {
    let report =
        agents.and_then(|clock| clock.with(|state| state.manifest_report().map(str::to_owned)));
    match report {
        None => IntrospectValue::Null,
        Some(error) => IntrospectValue::Json(serde_json::json!({ "error": error })),
    }
}

/// Serialise a session's change batch: [`Batch::to_wire`](crate::events::Batch::to_wire).
///
/// The mapping itself is NOT here. It used to be — an eleven-arm match writing each `type` name out
/// as a literal — and that was the second spelling of the event vocabulary: correct while nothing
/// else read those words, and free to drift the moment something did.
/// [`EventFilter`](crate::events::EventFilter) reads them, so they moved to
/// [`EventKind::wire_str`](crate::events::EventKind::wire_str) and this function became the slot's
/// half of the answer: pick
/// the journal, take the batch, hand back its one wire shape.
///
/// The read is UNFILTERED, deliberately. A filter belongs to a WAIT, where it decides whether a
/// caller is woken at all; a slot read is a caller asking what happened, and answering that with a
/// subset would make the cursor it advances past mean something different per caller.
fn events_value(channels: &ChannelRegistry, session: &str, since: u64) -> IntrospectValue {
    IntrospectValue::Json(channels.journal(session).since(since).to_wire())
}

/// Serialise one pane's neighbourhood: `{left, right, up, down}`, each a pane id or `null`.
///
/// Keyed by [`PaneDir::wire_str`](sprag_terminal::PaneDir::wire_str) — the same four words the
/// `select_pane` action reads a direction from, so a caller can feed one straight back to the
/// other. `null` is not "unknown": it is the statement that the pane is at that EDGE of the window
/// (see [`crate::wire::NEIGHBORS_FIELD`]).
fn neighbors_value(
    registry: &Arc<Mutex<SessionRegistry>>,
    scope: &SessionScope,
    pane: PaneId,
) -> IntrospectValue {
    let mut object = serde_json::Map::new();
    for (dir, neighbor) in crate::host::neighbors(registry, scope, pane) {
        object.insert(
            dir.wire_str().to_owned(),
            neighbor.map_or(Value::Null, |pane| serde_json::json!(pane.0)),
        );
    }
    IntrospectValue::Json(Value::Object(object))
}

/// Serialise an arrangement for the wire — the ONE place a [`LayoutSnapshot`] becomes JSON,
/// shared by the `layout` read and both writes' answers, so a client cannot meet two shapes
/// for one fact.
///
/// A serialisation failure is unreachable (the tree's own validation rejects the non-finite
/// ratio that is the only way to author bad JSON here), but it is TRACED rather than
/// silently answered as "unknown slot" — this file's own "the swallow is honest, not
/// silent" bar.
fn layout_value(snapshot: LayoutSnapshot) -> Option<IntrospectValue> {
    encoded_answer(&snapshot, "layout")
}

/// Answer `subject` with JSON text encoded ONCE, or trace the failure and answer absence.
///
/// The ONE place this file turns a serialisable answer into an [`IntrospectValue`], for the
/// reason `crate::pane`'s `cells` arm states: [`IntrospectValue::Raw`] (pinion R1480, delivering
/// PINION-PR79) carries text the producer already holds and `scene/query` splices it into the
/// reply, so nothing here builds a `serde_json::Value` tree for the dispatch to walk and encode a
/// second time. **The wire bytes do not change** — only how many times they are produced.
///
/// [`RawJson::encode`] rather than [`IntrospectValue::raw`] on purpose: the convenience
/// constructor degrades a failure to `Null` SILENTLY, and every caller here already had an error
/// channel it was using. Keeping the `Result` keeps this file's "the swallow is honest, not
/// silent" bar — the trace names which answer failed, and absence stays distinguishable from a
/// present-but-empty one.
fn encoded_answer<T: ?Sized + serde::Serialize>(
    value: &T,
    subject: &str,
) -> Option<IntrospectValue> {
    match RawJson::encode(value) {
        Ok(raw) => Some(IntrospectValue::Raw(raw)),
        Err(error) => {
            tracing::error!(target: "sprag_host", %error, subject, "answer failed to serialise");
            None
        }
    }
}

/// Build a [`CommandBuilder`] from an argv JSON array (`[program, args…]`),
/// returning it plus the program label. Empty or non-string argv is a
/// [`InvokeError::TypeMismatch`].
fn build_command(argv: &[Value]) -> Result<(CommandBuilder, String), InvokeError> {
    // Policy: the mux spec is a JSON `[program, args...]` string array (validated
    // here). The assembly (TERM, args, label) is the shared SSOT.
    let parts: Vec<&str> = argv
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()
        .ok_or(InvokeError::TypeMismatch)?;
    let (program, rest) = parts.split_first().ok_or(InvokeError::TypeMismatch)?;
    Ok(sprag_terminal::command_from_parts(
        *program,
        rest.iter().copied(),
    ))
}

/// Read a [`SizeRequest`] out of a `resize_window` action's args — the ONE place the four spellings
/// are told apart.
///
/// Exactly one spelling, or none (which is [`SizeRequest::Clear`]). Two is refused rather than
/// ordered by precedence: they are four ways to name one rectangle, so a caller sending two has not
/// decided, and a precedence rule would resolve that silently into a size nobody asked for.
fn size_request(map: &Map<String, Value>) -> Result<SizeRequest, InvokeError> {
    let exact = match (opt_dim(map, "cols")?, opt_dim(map, "rows")?) {
        (Some(cols), Some(rows)) => Some(ClientSize { cols, rows }),
        (None, None) => None,
        // Half a rectangle. Refused whole, never completed from the other decision's numbers.
        _ => return Err(InvokeError::TypeMismatch),
    };
    let adjust = match (
        opt_delta(map, "adjust_cols")?,
        opt_delta(map, "adjust_rows")?,
    ) {
        (None, None) => None,
        // An unnamed axis is not half a decision: it is "leave that edge where it is".
        (cols, rows) => Some(SizeRequest::Adjust {
            cols: cols.unwrap_or(0),
            rows: rows.unwrap_or(0),
        }),
    };
    let from = match map.get("from") {
        None => None,
        Some(Value::String(name)) => match WindowSize::parse(name) {
            // `manual` reads a stored size rather than folding clients, so as a SOURCE for a new
            // stored size it names nothing — a request to pin the window to whatever it is pinned to.
            Some(WindowSize::Manual) | None => return Err(InvokeError::TypeMismatch),
            Some(policy) => Some(policy),
        },
        Some(_) => return Err(InvokeError::TypeMismatch),
    };
    match (exact, adjust, from) {
        (None, None, None) => Ok(SizeRequest::Clear),
        (Some(size), None, None) => Ok(SizeRequest::Exact(size)),
        (None, Some(adjust), None) => Ok(adjust),
        (None, None, Some(policy)) => Ok(SizeRequest::Clients(policy)),
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// A SIGNED cell delta from an action's args — [`opt_dim`]'s counterpart for a relative resize,
/// where zero and negative are both meaningful and only an out-of-range magnitude is a bug.
fn opt_delta(map: &Map<String, Value>, key: &str) -> Result<Option<i32>, InvokeError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .map(Some)
            .ok_or(InvokeError::TypeMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::{pane_processes_at, session_activity_at};
    use pinion_core::SceneRevision;
    use serde_json::json;
    use sprag_terminal::PaneId;

    /// A registry at its boot state: one session, one window, an empty pool — what the
    /// mux control surface acts on.
    fn registry() -> Arc<Mutex<SessionRegistry>> {
        Arc::new(Mutex::new(SessionRegistry::new((80, 24))))
    }

    /// **The dead-scope surface WRITES NOTHING, and the claim is made of the type rather than of
    /// the caller that currently guards it.**
    ///
    /// `rpc::registry_only` admits `scene/query` alone, so nothing reaches these two arms today —
    /// which is exactly why they are worth a test. The guard is a policy in another module, one
    /// edit away from changing; this is the surface's own contract, and if it ever stopped holding,
    /// a client whose session was destroyed could ACT on a daemon through a door built to let it
    /// READ. A branch no test builds is the third shape the debt sweep looks for, and an
    /// unreachable branch is still the one a later refactor reaches first.
    ///
    /// The schema is asserted whole for the reason its own doc gives: the addresses this DAEMON
    /// serves do not shrink because one reader's session died, and a second shorter list would be a
    /// copy of the declaration `wire::MUX_SCHEMA` exists to be the only one of.
    #[test]
    fn the_dead_scope_surface_reads_the_registry_and_writes_nothing() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let mut surface = RegistryExternal::new(Arc::clone(&reg), crate::DaemonShared::none());

        // It READS every registry-subject slot, and answers each of them BYTE FOR BYTE what the
        // SCOPED surface answers. That is the real claim and a stronger one than "the list is not
        // empty": the two doors share `RegistryView`, so a build in which they could disagree is
        // the drift this round removed rather than a value this fixture happens to hold.
        let (scoped, _) = control(&reg);
        for slot in [
            SESSIONS_SLOT,
            TREE_SLOT,
            CLIENTS_SLOT,
            GRID_WORK_SLOT,
            GLOBAL_COMMANDS_SLOT,
            AGENT_MANIFESTS_SLOT,
        ] {
            let here = surface
                .query(slot)
                .expect("the registry answers its own slot");
            let there = scoped.query(slot).expect("and so does the scoped surface");
            assert_eq!(
                format!("{here:?}"),
                format!("{there:?}"),
                "{slot} must read the same through either door",
            );
        }
        // ...and NOT what a session's would be, on the very slots the scoped surface serves.
        for scoped in [PANES_SLOT, LAYOUT_SLOT, SESSION_SLOT, WINDOWS_SLOT] {
            assert!(
                surface.query(scoped).is_none(),
                "{scoped} is about ONE session and this surface has none to be wrong about",
            );
        }

        // EVERY action is refused — swept rather than sampled, because the failure this guards is
        // one arm quietly gaining a body.
        for action in crate::wire::MUX_SCHEMA.iter().filter(|f| f.ty == "action") {
            assert!(
                matches!(
                    surface.invoke(action.path, IntrospectValue::Null),
                    Err(InvokeError::UnknownPath)
                ),
                "{} must be refused: a reader with no session has none to act on",
                action.path,
            );
        }
        assert!(matches!(
            surface.intervene(SESSIONS_SLOT, IntrospectValue::Null),
            Err(InterveneError::UnknownPath),
        ));
        assert_eq!(
            surface.schema().fields.len(),
            crate::wire::MUX_SCHEMA.len(),
            "the surface publishes the daemon's whole vocabulary, not a narrowed copy",
        );
    }

    /// The DEFAULT session's pane pool — where an unscoped surface's spawns land, so a test
    /// asserts against the same pool the surface resolves.
    fn pool(reg: &Arc<Mutex<SessionRegistry>>) -> Arc<Mutex<Workspace>> {
        Arc::clone(SessionScope::unscoped(reg).workspace())
    }

    /// The pane pool of the session named `session`.
    fn pool_of(reg: &Arc<Mutex<SessionRegistry>>, session: &str) -> Arc<Mutex<Workspace>> {
        lock(reg).workspace_of(session).expect("a real session")
    }

    /// The scope a request naming `session` resolves to — built through the REAL resolution
    /// path, so a test cannot scope a surface to something no request could produce.
    fn scope_of(reg: &Arc<Mutex<SessionRegistry>>, session: &str) -> SessionScope {
        let request = pinion_rpc::parse_request(&format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"scene/query","params":{{"session":"{session}"}}}}"#
        ))
        .expect("a well-formed request");
        SessionScope::resolve(reg, &request, || None).expect("a session that exists")
    }

    /// A control surface over `reg` scoped to the DEFAULT session (what an unscoped request
    /// gets), plus the SCOPED session's token (returned so a test can assert the pane-lifecycle
    /// bumps). The token is read out of the channels by NAME, which is also what a test asserting
    /// a bump has to do now: a bump lands on one session's counter, so reading "the revision"
    /// without saying whose would be reading the wrong one.
    fn control(reg: &Arc<Mutex<SessionRegistry>>) -> (WorkspaceExternal, Arc<SceneRevision>) {
        let scope = SessionScope::unscoped(reg);
        scoped_control(reg, scope)
    }

    /// Samplers for a surface built one test at a time. Fresh per call, deliberately: a test builds
    /// its surface after arranging its registry, so samplers shared across tests could hand one of
    /// them a reading taken before its own sessions existed. In production there is exactly one set
    /// per host ([`crate::Host::samplers`]), which is what makes the samples shared; here the
    /// sharing is what would leak.
    fn sampler() -> crate::Samplers {
        crate::Samplers::default()
    }

    /// A control surface scoped to `scope` — what the assembly builds for a request that
    /// named a session.
    fn scoped_control(
        reg: &Arc<Mutex<SessionRegistry>>,
        scope: SessionScope,
    ) -> (WorkspaceExternal, Arc<SceneRevision>) {
        let channels = Arc::new(ChannelRegistry::default());
        let revision = channels.revision(scope.session());
        (
            WorkspaceExternal::new(
                Arc::clone(reg),
                scope,
                channels,
                crate::DaemonShared {
                    on_pane_exit: None,
                    attachments: None,
                    attention: None,
                    agents: None,
                    samplers: sampler(),
                },
            ),
            revision,
        )
    }

    /// A control surface WITH a detector installed, plus the registry it shares — the daemon's
    /// wiring, which is the only configuration the `agent` key exists in.
    fn control_with_agents(
        reg: &Arc<Mutex<SessionRegistry>>,
    ) -> (WorkspaceExternal, Arc<crate::AgentClock>) {
        let agents = Arc::new(crate::AgentClock::default());
        let scope = SessionScope::unscoped(reg);
        let channels = Arc::new(ChannelRegistry::default());
        (
            WorkspaceExternal::new(
                Arc::clone(reg),
                scope,
                channels,
                crate::DaemonShared {
                    on_pane_exit: None,
                    attachments: None,
                    attention: None,
                    agents: Some(Arc::clone(&agents)),
                    samplers: sampler(),
                },
            ),
            agents,
        )
    }

    /// The manifest report crosses as `{error}` and its absence as `null` — the shape the two config
    /// slots beside it already use, so a client that meets all three parses one thing.
    ///
    /// The three readings here are the three states a client has to tell apart, and the middle one
    /// is the one that could quietly stop working: a report is published by a THREAD this test does
    /// not run, so a slot that read the file itself, or a clock the surface never got, would answer
    /// `null` here and look exactly like a healthy daemon.
    ///
    /// A surface with NO detector answers `null` too, and that is not a gap. A host that evaluates
    /// nothing has no ruleset for the user's file to have failed to replace, so it has no verdict to
    /// report and paints nothing — which is what `null` asks a client to do.
    ///
    /// REVERT-PROOF: have `agent_manifests_value` stop asking the clock and the middle reading goes
    /// back to `null` — the shape a slot that answered from anywhere but the daemon's own holder
    /// would have.
    #[test]
    fn the_manifest_slot_says_null_or_the_daemons_own_sentence() {
        let reg = registry();
        let (ext, agents) = control_with_agents(&reg);
        assert_eq!(
            ext.query(AGENT_MANIFESTS_SLOT),
            Some(IntrospectValue::Null),
            "a daemon whose manifests ARE the user's reports nothing"
        );

        let sentence = "config.toml: `disable` names no rule `nope` in agent `claude`";
        agents.with(|state| state.set_manifest_report(Some(sentence.to_owned())));
        let Some(IntrospectValue::Json(answer)) = ext.query(AGENT_MANIFESTS_SLOT) else {
            panic!("a published report answers with a JSON object");
        };
        assert_eq!(
            answer["error"], sentence,
            "carried VERBATIM: the daemon rendered it because only it knows the file"
        );

        let (plain, _) = control(&reg);
        assert_eq!(
            plain.query(AGENT_MANIFESTS_SLOT),
            Some(IntrospectValue::Null),
            "and a surface with no detector has no verdict to defend"
        );
    }

    /// One pane's entry from the panes slot.
    fn pane_entry(ext: &mut WorkspaceExternal, id: u64) -> Value {
        let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
            panic!("the panes slot answers with a JSON array");
        };
        panes
            .into_iter()
            .find(|p| p["id"].as_u64() == Some(id))
            .expect("the pane is listed")
    }

    /// A spawn NAMES its opener, and the fact reaches the slot a client reads.
    ///
    /// Driven through `invoke` rather than through the pool, because the claim is that the ACTION
    /// carries the argument the whole way: the pool's own recording is pinned in `sprag-terminal`
    /// and stayed green through every version of this that dropped the argument on the wire.
    #[test]
    fn a_spawn_records_the_pane_that_asked_for_it() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("the opener is born first");
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "opened_by": 0})),
        )
        .expect("a spawn naming a live pane is honoured");

        assert_eq!(
            pane_entry(&mut ext, 1)["opened_by"],
            json!(0),
            "the pane says which pane asked for it",
        );
        assert_eq!(
            pane_entry(&mut ext, 0).get("opened_by"),
            None,
            "and the key is ABSENT — not null — for a pane nobody claims, so a workspace no agent \
             touched is byte-identical to the pre-provenance wire shape",
        );
    }

    /// An opener the daemon does not hold is REFUSED, and nothing is born.
    ///
    /// The second half is the load-bearing one: a request refused AFTER forking would leave a live
    /// pane the caller was never told about, which is the "a split that cannot reach its target must
    /// not quietly become an append" rule applied to a birth.
    #[test]
    fn a_spawn_naming_a_pane_that_is_gone_is_refused_and_births_nothing() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("one live pane");
        assert!(matches!(
            ext.invoke(
                SPAWN_ACTION,
                IntrospectValue::Json(json!({"cmd": ["cat"], "opened_by": 99})),
            ),
            Err(InvokeError::Rejected(_)),
        ));
        assert!(
            matches!(
                ext.invoke(
                    SPAWN_ACTION,
                    IntrospectValue::Json(json!({"cmd": ["cat"], "opened_by": "one"})),
                ),
                Err(InvokeError::TypeMismatch),
            ),
            "and a non-number is the MALFORMED refusal, not the unreachable-pane one"
        );
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            1,
            "neither refusal forked a child",
        );
    }

    /// A birth NAMES its pane, and the name reaches the slot a client reads.
    ///
    /// Driven through `invoke` for the reason `a_spawn_records_the_pane_that_asked_for_it` states:
    /// the pool's own recording is pinned in `sprag-terminal` and would stay green through every
    /// version of this that dropped the argument on the wire.
    #[test]
    fn a_birth_can_name_the_pane_it_opens() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("a pane nobody names");
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "name": "build"})),
        )
        .expect("a birth carrying a free name is honoured");

        assert_eq!(
            pane_entry(&mut ext, 1)["name"],
            json!("build"),
            "the pane says what it is called",
        );
        assert_eq!(
            pane_entry(&mut ext, 0).get("name"),
            None,
            "and the key is ABSENT — not null — for a pane nobody named, so a workspace nobody has \
             named anything in is byte-identical to the pre-name wire shape",
        );
    }

    /// A name already in use, or one broken by its own rules, is REFUSED and nothing is born.
    ///
    /// The second half is the load-bearing one, for
    /// `a_spawn_naming_a_pane_that_is_gone_is_refused_and_births_nothing`'s reason: a birth refused
    /// after forking leaves a live pane the caller was never told about.
    #[test]
    fn a_birth_naming_a_pane_something_already_taken_is_refused_and_births_nothing() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "name": "build"})),
        )
        .expect("the first claimant gets the name");
        assert!(
            matches!(
                ext.invoke(
                    SPAWN_ACTION,
                    IntrospectValue::Json(json!({"cmd": ["cat"], "name": "build"})),
                ),
                Err(InvokeError::Rejected(_)),
            ),
            "a name is unique registry-wide, so the second claimant is refused",
        );
        assert!(
            matches!(
                ext.invoke(
                    SPAWN_ACTION,
                    IntrospectValue::Json(json!({"cmd": ["cat"], "name": "7"})),
                ),
                Err(InvokeError::Rejected(_)),
            ),
            "and a name the type refuses is refused here too, rather than parsed by this surface",
        );
        assert!(
            matches!(
                ext.invoke(
                    SPAWN_ACTION,
                    IntrospectValue::Json(json!({"cmd": ["cat"], "name": 7})),
                ),
                Err(InvokeError::TypeMismatch),
            ),
            "a non-string is the MALFORMED refusal, not the name-in-use one",
        );
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            1,
            "none of the three refusals forked a child",
        );
    }

    /// A CONTAINER birth takes no pane name, and its own `name` stays the container's.
    ///
    /// This is the decision `parse_pane_name` states, asserted out loud rather than left to a
    /// reader of the parse site — the shape R294 had to add for `opened_by` after the fact.
    #[test]
    fn a_window_birth_names_the_window_and_never_the_pane_inside_it() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(
            NEW_WINDOW_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "name": "editors"})),
        )
        .expect("a window is born");

        assert_eq!(
            lock(&pool(&reg))
                .panes()
                .last()
                .expect("the new window's birth pane")
                .name(),
            None,
            "the request's `name` named the WINDOW; the pane inside it is unnamed",
        );
        let windows = without_ids(answer_doc(ext.query(WINDOWS_SLOT)));
        assert!(
            windows
                .as_array()
                .is_some_and(|ws| ws.iter().any(|w| w["name"] == json!("editors"))),
            "and the window really took it: {windows}",
        );
    }

    /// A rename lands on the pane, reaches the published listing, and can be taken back off.
    #[test]
    fn a_pane_can_be_named_and_unnamed_after_it_was_born() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("a pane to name");
        assert_eq!(
            ext.invoke(
                RENAME_PANE_ACTION,
                // Sent with whitespace: the ANSWER is the recorded name, not the argument, so a
                // caller never has to re-implement the trimming rule to report what it did.
                IntrospectValue::Json(json!({"pane": 0, "name": "  build  "})),
            ),
            Ok(IntrospectValue::Json(json!({"name": "build"}))),
            "a free name is taken, and the write says what it wrote",
        );
        assert_eq!(pane_entry(&mut ext, 0)["name"], json!("build"));

        // Re-naming to the name it ALREADY has is not a duplicate: the pane holding it is the one
        // being renamed. Without that forgiveness a caller could not re-assert its own name.
        ext.invoke(
            RENAME_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0, "name": "build"})),
        )
        .expect("a pane keeping its own name is not refused by itself");

        ext.invoke(
            RENAME_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0, "name": "test"})),
        )
        .expect("and it can be changed");
        assert_eq!(pane_entry(&mut ext, 0)["name"], json!("test"));

        assert_eq!(
            ext.invoke(
                RENAME_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0}))
            ),
            Ok(IntrospectValue::Json(json!({"name": null}))),
            "an absent name takes the name away, and the answer says the pane now has none",
        );
        assert_eq!(
            pane_entry(&mut ext, 0).get("name"),
            None,
            "and the key goes back to absent, not null",
        );
    }

    /// The four things a rename refuses, each with the pane left exactly as it was.
    #[test]
    fn a_rename_refuses_a_pane_it_cannot_reach_and_a_name_it_cannot_honour() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("one pane");
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "name": "build"})),
        )
        .expect("and one already named");

        for (args, why) in [
            (
                json!({"pane": 99, "name": "x"}),
                "a pane the daemon does not hold",
            ),
            (
                json!({"pane": 0, "name": "build"}),
                "a name another pane already carries",
            ),
            (json!({"pane": 0, "name": "12"}), "a name that is a number"),
            (
                json!({"pane": 0, "name": "a\nb"}),
                "a name that would forge a listing row",
            ),
        ] {
            assert!(
                matches!(
                    ext.invoke(RENAME_PANE_ACTION, IntrospectValue::Json(args.clone())),
                    Err(InvokeError::Rejected(_)),
                ),
                "{why} is refused: {args}",
            );
        }
        assert_eq!(
            pane_entry(&mut ext, 0).get("name"),
            None,
            "and pane 0 was left exactly as it was by all four",
        );
        assert_eq!(
            pane_entry(&mut ext, 1)["name"],
            json!("build"),
            "as was the pane holding the contested name",
        );
    }

    /// A rename reaches a pane in ANOTHER session, which is the one place this action's scope
    /// differs from every other pane action here.
    ///
    /// Pinned because it would otherwise be an argument in a doc comment: a name stands in for a
    /// registry-unique id, so a scoped rename would refuse a pane that plainly exists.
    #[test]
    fn a_rename_reaches_a_pane_in_a_session_the_request_did_not_name() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"name": "other", "cmd": ["cat"]})),
        )
        .expect("a second session with a pane of its own");
        let elsewhere = lock(&pool_of(&reg, "other"))
            .panes()
            .first()
            .expect("the other session's birth pane")
            .id();

        ext.invoke(
            RENAME_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": elsewhere.0, "name": "build"})),
        )
        .expect("the default-scoped surface renames a pane of another session");
        assert_eq!(
            lock(&pool_of(&reg, "other"))
                .panes()
                .first()
                .and_then(|pane| pane.name().map(sprag_terminal::PaneName::as_str)),
            Some("build"),
        );
        // And the uniqueness that check rests on is registry-wide too: a pane HERE cannot take a
        // name a pane THERE holds. The id is read back from the pool rather than assumed, because
        // the other session's birth pane already drew from the same counter.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("a pane in the default session");
        let here = lock(&pool(&reg))
            .panes()
            .last()
            .expect("the pane just born here")
            .id();
        assert_ne!(here, elsewhere, "the two panes are really different panes");
        assert!(
            matches!(
                ext.invoke(
                    RENAME_PANE_ACTION,
                    IntrospectValue::Json(json!({"pane": here.0, "name": "build"})),
                ),
                Err(InvokeError::Rejected(_)),
            ),
            "a name taken in another session is taken",
        );
    }

    /// A spawn opens its child in the directory it was given, and refuses one that is not there
    /// before anything is built.
    #[test]
    fn a_spawn_opens_in_the_directory_it_was_given() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        let dir = std::env::temp_dir();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "cwd": dir.to_str().unwrap()})),
        )
        .expect("an existing directory is honoured");
        assert!(
            matches!(
                ext.invoke(
                    SPAWN_ACTION,
                    IntrospectValue::Json(
                        json!({"cmd": ["cat"], "cwd": "/no/such/directory/here"})
                    ),
                ),
                Err(InvokeError::Rejected(_)),
            ),
            "a directory that is not there is a well-formed request the host cannot honour"
        );
        assert!(matches!(
            ext.invoke(
                SPAWN_ACTION,
                IntrospectValue::Json(json!({"cmd": ["cat"], "cwd": 7})),
            ),
            Err(InvokeError::TypeMismatch),
        ));
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            1,
            "only the honoured spawn forked a child",
        );
        // The child's OWN view of where it is, read from the pane rather than from the argument —
        // an action that parsed the directory and never applied it would pass every assertion above.
        let cwd = lock(&pool(&reg)).panes()[0].pty().cwd();
        assert_eq!(
            cwd.as_deref().and_then(|p| p.canonicalize().ok()),
            dir.canonicalize().ok(),
            "the child really started there",
        );
    }

    /// The birth spec is ONE spec, so `cwd` reaches every birth — and a window's birth pane
    /// INHERITS the window's opener, where a window nobody claims births a pane nobody claims.
    ///
    /// # ⚠ This test PINNED THE OPPOSITE, deliberately, and R313 re-decided it
    ///
    /// It used to assert *"a window's birth pane is claimed by nobody, however the request was
    /// spelled"*, with a comment saying an `opened_by` on this action was **sent and IGNORED** —
    /// and that was right at the time, because only a person could make a window and a window's
    /// birth pane really was nobody's work pane.
    ///
    /// R313 let a caller that is NOT a person make a window, and measured what the old rule then
    /// said: an agent that opened a window of its own was told *"this pane was opened by a PERSON,
    /// not by you"* about the pane its own request had just created — by `rename_pane`,
    /// `close_pane` and `resize_pane` alike, while `close_window` destroyed that same pane without
    /// a murmur. Whoever asked for the window asked for its first pane.
    ///
    /// `new_session` keeps the old rule and its own reason still holds: a session is not creatable
    /// by anything but a person.
    #[test]
    fn a_window_is_born_where_it_was_told_and_claimed_by_whoever_asked() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        let dir = std::env::temp_dir();
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("a pane that could be named as an opener");

        // THE CONTROL FIRST: a window nobody claims births a pane nobody claims, which is every
        // window a person makes and the half that must NOT have changed.
        ext.invoke(
            NEW_WINDOW_ACTION,
            IntrospectValue::Json(json!({ "cmd": ["cat"] })),
        )
        .expect("a window is born");
        assert_eq!(
            lock(&pool(&reg))
                .panes()
                .last()
                .and_then(sprag_terminal::Pane::opened_by),
            None,
            "a window nobody asked for births a pane nobody claims",
        );

        ext.invoke(
            NEW_WINDOW_ACTION,
            IntrospectValue::Json(json!({
                "cmd": ["cat"],
                "cwd": dir.to_str().unwrap(),
                "opened_by": 0,
            })),
        )
        .expect("a window is born");

        let born = lock(&pool(&reg))
            .panes()
            .last()
            .map(|pane| (pane.opened_by(), pane.pty().cwd()))
            .expect("the new window's birth pane");
        assert_eq!(
            born.0,
            Some(sprag_terminal::PaneId(0)),
            "the pane of a window an agent asked for is the agent's too, or the surface refuses \
             it with a sentence that is false about who made it",
        );
        assert_eq!(
            born.1.as_deref().and_then(|p| p.canonicalize().ok()),
            dir.canonicalize().ok(),
            "and the directory really reached the child, so the shared spec is not just parsed",
        );
    }

    /// Wait until pane `id` carries an `agent` key, then return its entry.
    ///
    /// Waits on the CONDITION the assertions read rather than on a timer: the child's `printf` reaches
    /// the emulator asynchronously, so a sleep long enough today is a flake tomorrow. Each poll
    /// re-queries the slot, which is also what drives the evaluation — a detector wired to this site
    /// answers only when asked, which is the whole of D9's problem and is fine HERE because the caller
    /// is asking.
    fn await_agent(ext: &mut WorkspaceExternal, id: u64) -> Value {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let entry = pane_entry(ext, id);
            if entry.get("agent").is_some() {
                return entry;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pane {id} never published an agent state: {entry:?}",
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// A pane painted as a BLOCKED `claude`: a choice list over the footer its fingerprint reads, and
    /// a `cat` to hold the pane open. `Blocked` is the state that publishes on sight, so a test built
    /// on it waits for no window — and it is the state the whole front exists to report.
    ///
    /// The rules' fidelity to a REAL agent screen is slice 1's business, proven there against six
    /// captured dialogs; this is a screen those rules already answer for.
    const BLOCKED_CLAUDE: &str =
        "printf '\\033[2J\\033[H❯ 1. Yes\\n  2. No\\n  ⏸ manual mode on · ? for shortcuts\\n'; cat";

    /// The same pane at REST: the resting glyph in the title, and the footer. `Idle` rests on an
    /// ABSENCE, so it is the state the settle window applies to.
    const IDLE_CLAUDE: &str = "printf '\\033]2;✳ Claude Code\\007\\033[2J\\033[H❯\\n  ⏸ manual mode \
                               on · ? for shortcuts\\n'; cat";

    /// A report crosses the wire, reaches the pane list, and is RECORDED as a change a reader that
    /// arrives later still learns about.
    ///
    /// The event matters as much as the verdict: a client parked on `scene/waitFor` is woken by the
    /// announce, and R269's journal is what tells it which pane to re-read. A report that published
    /// without recording would move the pane list under a client with nothing to point at it.
    #[test]
    fn a_report_over_the_wire_publishes_and_records_the_change() {
        let reg = registry();
        let (mut ext, _agents) = control_with_agents(&reg);
        // A plain shell, so the SCRAPE has nothing to say about this pane at all — every word of the
        // answer below therefore comes from the report.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        let answer = ext
            .invoke(
                REPORT_AGENT_ACTION,
                IntrospectValue::Json(json!({
                    "id": 0,
                    "source": "herdr:claude",
                    "state": "working",
                    "name": "claude",
                    "seq": 11,
                })),
            )
            .expect("a well-formed report is taken");
        let IntrospectValue::Json(answer) = answer else {
            panic!("a report answers with an object");
        };
        assert_eq!(answer["accepted"], json!(true));
        assert_eq!(
            answer["changed"],
            json!(true),
            "nothing was published before"
        );
        assert_eq!(answer["seq"], json!(1), "the first published generation");

        let entry = pane_entry(&mut ext, 0);
        assert_eq!(
            entry["agent"],
            json!({
                "state": "working",
                "name": "claude",
                "seq": 1,
                "source": "herdr:claude",
            }),
            "a pane no rule claims is published because a process inside it said so, and the answer \
             names the authority instead of a rule: {entry:?}",
        );

        let Some(IntrospectValue::Json(batch)) = ext.query(&crate::wire::events_slot_since(0))
        else {
            panic!("the events family answers");
        };
        assert!(
            batch["events"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|event| event["type"] == "pane_agent_state_changed" && event["pane"] == 0),
            "the report is readable as a typed change: {batch}",
        );
    }

    /// A job change crosses the wire as `pane_job_changed`, naming the PANE and nothing else.
    ///
    /// The whole object is pinned rather than probed with `.any(…)`, because the claim has two
    /// halves and a containment check only tests the first: the type string a client matches on,
    /// and the ABSENCE of the process group. Publishing the pgid here would be a second encoding of
    /// a fact `pane_processes` already serves — the defect R290 found in the rival's own answer
    /// (`cmdline` beside `argv`), and an extra key is exactly what a containment assertion cannot
    /// see.
    #[test]
    fn a_job_change_crosses_the_wire_naming_only_its_pane() {
        let reg = registry();
        let channels = Arc::new(ChannelRegistry::default());
        let ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::clone(&channels),
            crate::DaemonShared {
                on_pane_exit: None,
                attachments: None,
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        );

        channels.announce("0", vec![crate::events::Event::PaneJobChanged(7)]);

        let Some(IntrospectValue::Json(batch)) = ext.query(&crate::wire::events_slot_since(0))
        else {
            panic!("the events family answers");
        };
        assert_eq!(
            batch["events"],
            json!([{ "type": "pane_job_changed", "pane": 7 }]),
            "the subject is the pane, and the process group is NOT on the wire: {batch}",
        );
    }

    /// A DUPLICATE report is accepted and records nothing — the same condition the settle waker uses,
    /// so the two publishers of an agent verdict cannot come to disagree about what a change is.
    #[test]
    fn a_duplicate_report_wakes_nobody() {
        let reg = registry();
        let (mut ext, _agents) = control_with_agents(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        let report = |ext: &mut WorkspaceExternal, seq: u64| {
            let answer = ext
                .invoke(
                    REPORT_AGENT_ACTION,
                    IntrospectValue::Json(json!({
                        "id": 0, "source": "hook", "state": "working", "seq": seq,
                    })),
                )
                .expect("accepted");
            let IntrospectValue::Json(answer) = answer else {
                panic!("an object");
            };
            answer
        };
        let events = |ext: &WorkspaceExternal| -> usize {
            let Some(IntrospectValue::Json(batch)) = ext.query(&crate::wire::events_slot_since(0))
            else {
                panic!("the events family answers");
            };
            batch["events"].as_array().map_or(0, Vec::len)
        };

        report(&mut ext, 1);
        let recorded = events(&ext);
        let second = report(&mut ext, 2);
        assert_eq!(second["accepted"], json!(true), "a repeat is heard");
        assert_eq!(second["changed"], json!(false), "and publishes nothing");
        assert_eq!(
            events(&ext),
            recorded,
            "so it records nothing and wakes nobody",
        );
    }

    /// Every way a report can be malformed, refused at the door — and each for its own reason.
    #[test]
    fn a_report_this_daemon_cannot_honour_is_refused() {
        let reg = registry();
        let (mut ext, _agents) = control_with_agents(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        let report = |ext: &mut WorkspaceExternal, args: Value| {
            ext.invoke(REPORT_AGENT_ACTION, IntrospectValue::Json(args))
        };

        assert!(
            matches!(
                report(
                    &mut ext,
                    json!({"id": 7, "source": "hook", "state": "idle"})
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a pane the daemon does not hold — a reporter that outlived its pane",
        );
        assert_eq!(
            report(&mut ext, json!({"id": 0, "state": "idle"})),
            Err(InvokeError::TypeMismatch),
            "an authority that cannot be named is not an authority",
        );
        assert_eq!(
            report(&mut ext, json!({"id": 0, "source": "", "state": "idle"})),
            Err(InvokeError::TypeMismatch),
            "nor is an empty one",
        );
        assert_eq!(
            report(
                &mut ext,
                json!({"id": 0, "source": "hook", "state": "unknown"})
            ),
            Err(InvokeError::TypeMismatch),
            "`unknown` is a conclusion about the RULES, not a state a reporter is in",
        );
        assert_eq!(
            report(
                &mut ext,
                json!({"id": 0, "source": "hook", "state": "busy"})
            ),
            Err(InvokeError::TypeMismatch),
            "and a spelling the vocabulary does not have",
        );
        assert_eq!(
            report(
                &mut ext,
                json!({"id": 0, "source": "hook", "state": "idle", "seq": "11"}),
            ),
            Err(InvokeError::TypeMismatch),
            "a counter that arrived as a string is a bug in the reporter, not a report with no clock",
        );
        let entry = pane_entry(&mut ext, 0);
        assert!(
            entry.get("agent").is_none(),
            "and none of them published anything: {entry:?}",
        );
    }

    /// A release says whether it actually dropped anything, so a retry is not silently told it worked.
    #[test]
    fn a_release_says_whether_there_was_anything_to_release() {
        let reg = registry();
        let (mut ext, _agents) = control_with_agents(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        let release = |ext: &mut WorkspaceExternal, id: u64| {
            ext.invoke(
                RELEASE_AGENT_ACTION,
                IntrospectValue::Json(json!({"id": id})),
            )
        };

        assert_eq!(
            release(&mut ext, 0),
            Ok(IntrospectValue::Json(json!({"released": false}))),
            "nobody is reporting this pane yet",
        );
        ext.invoke(
            REPORT_AGENT_ACTION,
            IntrospectValue::Json(json!({"id": 0, "source": "hook", "state": "working"})),
        )
        .unwrap();
        assert_eq!(
            release(&mut ext, 0),
            Ok(IntrospectValue::Json(json!({"released": true}))),
        );
        assert_eq!(
            release(&mut ext, 0),
            Ok(IntrospectValue::Json(json!({"released": false}))),
            "and the second release has nothing left to drop",
        );
        assert!(
            matches!(release(&mut ext, 7), Err(InvokeError::Rejected(_))),
            "a pane the daemon does not hold is refused, as it is for a report",
        );
    }

    /// D8: the key is present for a claimed pane and ABSENT for everything else, so a workspace of
    /// shells is byte-identical to the pre-H3 wire shape. Both halves in one query, because the
    /// absence is only meaningful beside a presence — a detector that answered for nothing at all
    /// would pass the first assertion on its own.
    #[test]
    fn the_agent_key_is_present_for_a_claimed_pane_and_absent_for_a_shell() {
        let reg = registry();
        let (mut ext, _agents) = control_with_agents(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["sh", "-c", BLOCKED_CLAUDE]})),
        )
        .unwrap();

        let claimed = await_agent(&mut ext, 1);
        assert_eq!(
            claimed["agent"],
            json!({
                "state": "blocked",
                "name": "claude",
                "rule": "dialog-choice-list",
                "seq": 1,
            }),
            "the state a person wants, the agent, the rule that says WHY (D7), and the seq",
        );

        let shell = pane_entry(&mut ext, 0);
        assert!(
            shell.get("agent").is_none(),
            "a pane no manifest claims carries no key at all: {shell:?}",
        );
    }

    /// **A pane's NAME never reaches the agent detector**, however much it looks like a title.
    ///
    /// The sharpest thing a name could break. `claude`'s first fingerprint is ONE condition on the
    /// title alone — it starts with `✳` — so if the panes-slot walk ever passed a name where it
    /// passes `pane.title()`, anyone who can name a pane could forge an agent identity that every
    /// other agent then reads back through `agent_state`. The two facts are adjacent on the same
    /// struct and one line apart at the call site, which is exactly why this is asserted rather
    /// than argued.
    #[test]
    fn a_name_that_looks_like_an_agents_title_claims_nothing() {
        let reg = registry();
        let (mut ext, agents) = control_with_agents(&reg);
        ext.invoke(
            SPAWN_ACTION,
            // A plain `cat`: no agent screen, no agent title, nothing but the name.
            IntrospectValue::Json(json!({"cmd": ["cat"], "name": "✳ Claude Code"})),
        )
        .expect("the name is legal — it is only the DETECTOR that must not read it");

        let entry = pane_entry(&mut ext, 0);
        assert_eq!(
            entry["name"],
            json!("✳ Claude Code"),
            "the pane really carries the forgery-shaped name",
        );
        assert!(
            entry.get("agent").is_none(),
            "and it publishes no agent verdict: {entry:?}",
        );
        // The load-bearing half, and the reason this test is not the two lines above. A claim does
        // not publish until it has SETTLED, so an absent `agent` key is what an unsettled forgery
        // and a rejected one both look like — the first version of this test asserted only that and
        // stayed GREEN with the leak deliberately wired in. What separates them is the CANDIDATE:
        // a pane the fingerprint claimed is pending on the clock from the first look.
        assert_eq!(
            agents.with(|state| state.pending_deadline(PaneId(0))),
            None,
            "and it was never even a CANDIDATE: a name is chosen by whoever asked, so a verdict \
             about what is RUNNING may only be derived from what the child itself put on its \
             screen or in its own title",
        );
    }

    /// The `seq` is what lets a client tell "still blocked" from "blocked again" without diffing
    /// strings, so it must NOT move on a re-read of an unchanged pane — which is what two attached
    /// clients polling one wake do.
    #[test]
    fn re_reading_an_unchanged_agent_pane_does_not_move_the_seq() {
        let reg = registry();
        let (mut ext, _agents) = control_with_agents(&reg);
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["sh", "-c", BLOCKED_CLAUDE]})),
        )
        .unwrap();

        let first = await_agent(&mut ext, 0);
        let second = pane_entry(&mut ext, 0);
        let third = pane_entry(&mut ext, 0);
        assert_eq!(first["agent"], second["agent"]);
        assert_eq!(
            second["agent"], third["agent"],
            "the verdict is published once however many times it is read",
        );
        assert_eq!(second["agent"]["seq"], json!(1));
    }

    /// The settle window is a user OPTION, and this is the assertion that it is a control something
    /// OBEYS rather than one `show-options` merely prints.
    ///
    /// `agent-settle-time = 0` means "publish every reading as it arrives", so an IDLE pane — a verdict
    /// that rests on an absence and would otherwise wait two seconds — publishes on the first look. A
    /// site that ignored the option, or read it once at startup, fails here on the clock rather than on
    /// a value: the test would hang until its own deadline.
    #[test]
    fn the_settle_option_reaches_the_evaluation_site() {
        crate::config::with_config(Some("[options]\nagent-settle-time = 0\n"), || {
            let reg = registry();
            let (mut ext, _agents) = control_with_agents(&reg);
            ext.invoke(
                SPAWN_ACTION,
                IntrospectValue::Json(json!({"cmd": ["sh", "-c", IDLE_CLAUDE]})),
            )
            .unwrap();

            let entry = await_agent(&mut ext, 0);
            assert_eq!(
                entry["agent"]["state"],
                json!("idle"),
                "a zero window publishes a resting verdict on sight: {entry:?}",
            );
        });
    }

    #[test]
    fn spawn_default_returns_first_id_and_adds_a_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        let id = ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(id, IntrospectValue::Int(0));
        assert_eq!(lock(&pool(&reg)).panes().len(), 1);
    }

    #[test]
    fn spawn_with_cmd_array_sets_label() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(lock(&pool(&reg)).list()[0].command_label, "cat");
    }

    /// The answer a kill gives now that all three of them say how far the CASCADE reached — the
    /// expected value spelled once for the six sites that assert it, so a change to the key or the
    /// vocabulary lands in one place rather than in six literals.
    fn ended(word: sprag_terminal::Ended) -> Result<IntrospectValue, InvokeError> {
        Ok(IntrospectValue::Json(
            json!({ crate::wire::ENDED_KEY: word.as_wire() }),
        ))
    }

    #[test]
    fn close_existing_then_missing() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(
            ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0}))),
            ended(sprag_terminal::Ended::Server),
            "this fixture holds ONE session of ONE window, so its only pane takes all three with \
             it — the answer says so instead of the `null` it said before R309",
        );
        assert!(
            matches!(
                ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0}))),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0}))),
        );
    }

    #[test]
    fn spawn_and_close_bump_the_revision() {
        // The pane-set change-notification: a spawn (set grew) and a close (set
        // shrank) each bump the revision synchronously, so a client long-polling
        // `scene/waitFor` learns the set changed WITHOUT waiting for pane output.
        // `cat` produces no output on its own, so the ONLY bumps here are the two
        // set-change bumps under test (no output on_dirty to confound the counts).
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        let before = rev.current();
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert!(
            rev.current() > before,
            "spawn bumps the revision (set grew)"
        );
        let after_spawn = rev.current();
        ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0})))
            .unwrap();
        assert!(
            rev.current() > after_spawn,
            "close bumps the revision (set shrank)"
        );
    }

    #[test]
    fn mux_spawned_pane_output_bumps_the_revision() {
        use std::time::{Duration, Instant};
        // The subtle half: a mux-`spawn`ed pane is wired with `bump_on_dirty`, so
        // its OWN output bumps the revision with no client input — exactly as the
        // boot pane's does. `spawn` first bumps once (set change), then the pane's
        // "hi" stdout drives its on_dirty into a further bump. Waiting for `+2` over
        // the pre-spawn baseline is race-free regardless of how the synchronous
        // set-change bump and the async output bump interleave.
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        let before = rev.current();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["sh", "-c", "printf hi"]})),
        )
        .unwrap();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if rev.current() >= before + 2 {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("a mux-spawned pane's output never bumped the shared revision");
    }

    #[test]
    fn resize_requires_dims_and_targets_a_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        // Missing rows -> type mismatch.
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(json!({"id": 0, "cols": 100}))
            ),
            Err(InvokeError::TypeMismatch)
        );
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(json!({"id": 0, "cols": 100, "rows": 30}))
            ),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            lock(&pool(&reg))
                .pane(PaneId(0))
                .unwrap()
                .pty()
                .dimensions(),
            (100, 30)
        );
    }

    #[test]
    fn resize_threads_the_optional_cell_metric_to_the_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        // A GUI client sends cell_width/cell_height (its font metric) — they reach the emulator.
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(
                    json!({"id": 0, "cols": 100, "rows": 30, "cell_width": 9, "cell_height": 18})
                )
            ),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            lock(&pool(&reg))
                .pane(PaneId(0))
                .unwrap()
                .pty()
                .cell_pixel_size(),
            (9, 18),
            "the invoke's cell metric reaches the pane's emulator"
        );
        // A resize WITHOUT the metric (a headless client) is still accepted and preserves it.
        assert_eq!(
            ext.invoke(
                RESIZE_ACTION,
                IntrospectValue::Json(json!({"id": 0, "cols": 80, "rows": 24}))
            ),
            Ok(IntrospectValue::Null)
        );
        assert_eq!(
            lock(&pool(&reg))
                .pane(PaneId(0))
                .unwrap()
                .pty()
                .cell_pixel_size(),
            (9, 18),
            "a metric-less resize preserves the last-known cell geometry"
        );
    }

    /// The pane-list entries with the per-pane PROJECTION TOKEN lifted out — so a test can assert
    /// the stable wire shape exactly, while the token (whose `row_generations` are as long as the
    /// pane is tall) is checked for what it must contain rather than transcribed.
    fn panes_without_projection(value: Option<IntrospectValue>) -> (Value, Vec<Value>) {
        let IntrospectValue::Json(Value::Array(entries)) = value.expect("a pane list") else {
            panic!("the pane list is a JSON array");
        };
        let mut tokens = Vec::new();
        let stripped: Vec<Value> = entries
            .into_iter()
            .map(|mut entry| {
                let token = entry
                    .as_object_mut()
                    .and_then(|map| map.remove("projection"))
                    .expect("every pane entry carries a projection token");
                tokens.push(token);
                entry
            })
            .collect();
        (Value::Array(stripped), tokens)
    }

    #[test]
    fn query_panes_lists_metadata() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["cat"], "cols": 40, "rows": 12})),
        )
        .unwrap();
        let (panes, tokens) = panes_without_projection(ext.query(PANES_SLOT));
        assert_eq!(
            panes,
            // `title` is null until the child sets an OSC 0/2 window title (R128). `active` rides
            // because a window is ON its only pane the moment it has one — nobody selected it.
            json!([{
                "id": 0, "cols": 40, "rows": 12, "command": "cat", "title": null, "active": true,
            }])
        );
        // ...and the token beside it describes the pane a client would fetch: one damage stamp per
        // row, at the pane's own width. A client compares it whole; this asserts it is not a stub.
        let token: sprag_grid::ProjectionToken =
            serde_json::from_value(tokens[0].clone()).expect("the token round-trips");
        assert_eq!(token.cols, 40);
        assert_eq!(token.row_generations.len(), 12);
    }

    /// The child's `OSC 2` window title reaches the WIRE (R128) — the pane-list query is
    /// how a display client (the GUI) learns it. The child emits the escape on stdout,
    /// exactly as a shell's `PROMPT_COMMAND` does.
    #[test]
    fn query_panes_reports_the_childs_osc_window_title() {
        use std::time::{Duration, Instant};
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(
                json!({"cmd": ["sh", "-c", "printf '\\033]2;vim README\\007'"], "cols": 40, "rows": 12}),
            ),
        )
        .unwrap();

        // The reader thread applies the bytes asynchronously — poll the wire until the
        // title lands (what a client does after a `scene/waitFor` wake).
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(IntrospectValue::Json(Value::Array(entries))) = ext.query(PANES_SLOT)
                && entries.first().and_then(|pane| pane["title"].as_str()) == Some("vim README")
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the child's OSC 2 window title never reached the pane-list wire");
    }

    /// A child's live INPUT-MODE state — mouse tracking (DECSET 1000/1002/1003) and focus tracking
    /// (DECSET 1004) — reaches the pane-list WIRE, ADDITIVELY: the keys appear only once the child
    /// has enabled them. This is the producer→wire path the agent-facing `list_panes` MCP tool reads;
    /// `query_panes_lists_metadata` proves the resting shape carries NEITHER key.
    #[test]
    fn query_panes_reports_the_childs_mouse_and_focus_tracking() {
        use std::time::{Duration, Instant};
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        // A raw child that enables button-event mouse tracking (1002) AND focus reporting (1004).
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({
                "cmd": ["sh", "-c", "stty raw -echo 2>/dev/null; printf '\\033[?1002h\\033[?1004h'; cat"],
                "cols": 40, "rows": 12,
            })),
        )
        .unwrap();

        // The reader thread applies the bytes asynchronously — poll the wire until both land.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Some(IntrospectValue::Json(Value::Array(entries))) = ext.query(PANES_SLOT)
                && let Some(pane) = entries.first()
                && pane.get("mouse").and_then(Value::as_str) == Some("button")
                && pane.get("focus_tracking").and_then(Value::as_bool) == Some(true)
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("the child's DECSET 1002/1004 modes never reached the pane-list wire");
    }

    /// The `layout` slot serves the CURRENT window's arrangement — and reconciles it
    /// against the live pool first. That reconcile is load-bearing: panes arrive through
    /// the `Workspace` (here via `spawn`), never through the layout, so an un-reconciled
    /// read would report an empty arrangement for a window that plainly has panes.
    #[test]
    fn query_layout_reports_the_current_windows_arrangement() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            answer_doc(ext.query(LAYOUT_SLOT)),
            json!({"revision": 0, "tree": {"nodes": [], "root": null}, "floating": []}),
            "an empty window has no arrangement — and the wire carries no minting counter",
        );

        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        let layout = query_layout(&mut ext);
        // Two spawned panes arrange as one split of leaf 0 | leaf 1 — the tree the
        // display client projects, carrying no pixels.
        let root = root_node(&layout);
        assert_eq!(child(&layout, &root, "first")["leaf"], 0);
        assert_eq!(child(&layout, &root, "second")["leaf"], 1);
        assert_eq!(root["split"]["dir"], "horizontal");
        assert_eq!(root["split"]["ratio"], 0.5);

        // A closed pane's leaf collapses: the survivor takes the root, no half-split.
        ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({"id": 0})))
            .unwrap();
        let layout = query_layout(&mut ext);
        assert_eq!(
            root_node(&layout)["leaf"],
            1,
            "the survivor reclaimed the space"
        );
    }

    /// The DOCUMENT a structural answer carries, whichever way it is encoded.
    ///
    /// A `Raw` answer and a `Json` answer holding the same document are INDISTINGUISHABLE on the
    /// wire — the dispatch splices one and serialises the other to the same bytes — so a test
    /// whose subject is the CONTENT must not accidentally also be an assertion about which
    /// encoding produced it. Every content test below reads through here; the one test whose
    /// subject IS the encoding asserts the variant directly and is named for it
    /// (`a_structural_answer_is_encoded_text_not_a_dom`).
    fn answer_doc(value: Option<IntrospectValue>) -> Value {
        match value.expect("the slot answers") {
            IntrospectValue::Json(v) => v,
            IntrospectValue::Raw(raw) => raw.to_value().expect("an encoded answer is valid JSON"),
            other => panic!("the slot answered a non-structural value: {other:?}"),
        }
    }

    /// The `windows` slot's rows WITHOUT their identities — for the tests whose subject is which
    /// windows a session sees and which one is current.
    ///
    /// The identity is asserted by exactly one test
    /// ([`the_windows_slot_publishes_each_windows_identity`](self)), and against the registry's own
    /// minted value rather than a literal. Spreading it across every window test would pin an id
    /// ALLOCATION order in six places — a number nothing promises — and none of them would then be
    /// about what they are named for.
    fn without_ids(value: Value) -> Value {
        Value::Array(
            value
                .as_array()
                .expect("the windows slot answers a list")
                .iter()
                .map(|row| {
                    let mut row = row.as_object().expect("a window row is an object").clone();
                    row.remove("id");
                    Value::Object(row)
                })
                .collect(),
        )
    }

    /// [`answer_doc`]'s sibling for the WRITE channel: the layout writes answer with the
    /// arrangement now in force, built by the same `layout_value`, so they carry the same
    /// encoding and their tests must read it the same way.
    fn write_doc(result: Result<IntrospectValue, InvokeError>) -> Value {
        answer_doc(Some(result.expect("the write answers")))
    }

    /// THE ENCODING, which every content test above is deliberately blind to.
    ///
    /// `answer_doc` reads through both variants BECAUSE the wire cannot tell them apart — which
    /// is the point of the change and also what makes it invisible: revert the consumption and
    /// every other assertion in this file still passes. So the claim "this file's structural
    /// answers are encoded once, not built as a `serde_json::Value` for the dispatch to walk and
    /// encode again" gets the one test that can fail for it.
    #[test]
    fn a_structural_answer_is_encoded_text_not_a_dom() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        for slot in [
            SESSIONS_SLOT,
            CLIENTS_SLOT,
            WINDOWS_SLOT,
            WINDOW_SIZE_SLOT,
            LAYOUT_SLOT,
        ] {
            let answer = ext.query(slot).expect("the slot answers");
            assert!(
                answer.as_raw().is_some(),
                "`{slot}` still builds a serde_json::Value DOM for the dispatch to re-encode; \
                 the answer was {answer:?}",
            );
        }
    }

    /// The reason these callers use [`RawJson::encode`] and not the `IntrospectValue::raw`
    /// convenience: the convenience degrades a failure to `Null` SILENTLY, and every caller here
    /// had an error channel worth keeping. A value serde_json refuses (a map whose keys are not
    /// strings) answers ABSENCE — distinguishable from a present-but-empty `Null` — and is traced.
    #[test]
    fn an_unserialisable_answer_is_absent_rather_than_a_silent_null() {
        let refused: std::collections::BTreeMap<[u8; 2], u8> = [([1, 2], 3)].into_iter().collect();
        assert!(
            encoded_answer(&refused, "a test value").is_none(),
            "a refusal must not be spelled as a present answer",
        );
        // The control: an ordinary value DOES answer, so the assertion above is about the
        // serialization failure and not about the helper never answering at all.
        assert!(encoded_answer(&json!({"ok": true}), "a test value").is_some());
    }

    /// The mux `layout` slot as JSON (the shape a client actually parses).
    fn query_layout(ext: &mut WorkspaceExternal) -> Value {
        answer_doc(ext.query(LAYOUT_SLOT))
    }

    /// The node an arrangement roots at, resolved through the arena.
    ///
    /// A window's `tree` is a FLAT list of nodes naming their children by index, so that a
    /// user's pane count cannot deepen the JSON a client has to parse (R264, and
    /// `sprag_terminal::MAX_LAYOUT_DEPTH` for why). These two helpers are what a read
    /// assertion walks it with — one hop per level, the same shape the old nested spelling
    /// let a test index into directly.
    fn root_node(layout: &Value) -> Value {
        let index = layout["tree"]["root"]
            .as_u64()
            .expect("an arrangement with panes names its root by index");
        layout["tree"]["nodes"][index as usize].clone()
    }

    /// A split's `first` or `second` child, resolved through the same arena.
    fn child(layout: &Value, node: &Value, side: &str) -> Value {
        let index = node["split"][side]
            .as_u64()
            .unwrap_or_else(|| panic!("a split names its {side} child by index: {node}"));
        layout["tree"]["nodes"][index as usize].clone()
    }

    /// The write half through the control surface: a client's settled arrangement installs,
    /// its self-minted divider comes back NAMED, and the answer IS what the slot then
    /// serves — so one round trip records the gesture and tells the client the identity to
    /// key its per-split state on.
    #[test]
    fn set_layout_installs_a_clients_arrangement_and_names_its_divider() {
        let reg = registry();
        let (mut ext, revision) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let before = revision.current();

        // The client sends a VERTICAL split at a dragged ratio, through a divider it minted
        // itself (no `id` — naming one is the host's job).
        let at = query_layout(&mut ext)["revision"]
            .as_u64()
            .expect("a revision");
        let answer = write_doc(ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(json!({ "expected_revision": at, "tree": {
                "nodes": [
                    { "leaf": 1 },
                    { "leaf": 0 },
                    { "split": { "dir": "vertical", "ratio": 0.75, "first": 0, "second": 1 } },
                ],
                "root": 2,
            } })),
        ));

        let root = root_node(&answer);
        assert_eq!(root["split"]["dir"], "vertical");
        assert_eq!(root["split"]["ratio"], 0.75);
        assert_eq!(
            child(&answer, &root, "first")["leaf"],
            1,
            "the client's pane ORDER is the user's intent, and it stuck",
        );
        assert!(
            root["split"]["id"].is_number(),
            "the host NAMED the client's divider: {answer}",
        );
        assert_eq!(
            query_layout(&mut ext),
            answer,
            "the answer is what is served"
        );
        assert!(
            revision.current() > before,
            "an arrangement change wakes parked waiters, as a pane-set change does",
        );
    }

    /// Float is session state: taking a pane out of the tiling collapses its leaf HERE, so
    /// a display client renders an exact projection rather than filtering one itself.
    #[test]
    fn set_floating_takes_a_pane_out_of_the_tiling_and_puts_it_back() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        let answer = write_doc(ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({ "id": 1, "floating": true })),
        ));
        assert_eq!(
            root_node(&answer)["leaf"],
            0,
            "the floated pane's leaf collapsed; its sibling reclaimed the space",
        );
        // The float set is SERVED, not just applied: "absent from the tiling" is all a
        // floated pane and an unknown pane have in common, so a client that could not read
        // this would render the floated pane neither as a leaf nor as a window — it would
        // vanish. This is what lets a reattaching client restore the user's floats.
        assert_eq!(answer["floating"], json!([1]));

        // Docked back with no gesture to place it, it returns to the home the float captured
        // (`Window::set_floating`). With two panes that is also the end, so this pins the
        // shape rather than the mechanism — the home path's own guards live beside it, in
        // `sprag-terminal`.
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({ "id": 1, "floating": false })),
        )
        .unwrap();
        let layout = query_layout(&mut ext);
        let root = root_node(&layout);
        assert_eq!(child(&layout, &root, "first")["leaf"], 0);
        assert_eq!(child(&layout, &root, "second")["leaf"], 1);
        assert_eq!(
            layout["floating"],
            json!([]),
            "docked back, it floats no more"
        );
    }

    /// A client cannot install an arrangement that breaks the tree's invariants: the
    /// session keeps the layout it had rather than absorbing a wrong-but-plausible one that
    /// would outlive the buggy client that sent it.
    #[test]
    fn set_layout_rejects_a_malformed_arrangement_and_keeps_the_one_in_force() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let good = query_layout(&mut ext);

        // The same pane twice, at a ratio that is not a share.
        let at = good["revision"].as_u64().expect("a revision");
        let answer = write_doc(ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(json!({ "expected_revision": at, "tree": {
                "nodes": [
                    { "leaf": 0 },
                    { "leaf": 0 },
                    { "split": { "dir": "horizontal", "ratio": 4.2, "first": 0, "second": 1 } },
                ],
                "root": 2,
            } })),
        ));
        assert_eq!(answer, good, "the arrangement in force is untouched");

        // A tree that does not even deserialise is a malformed REQUEST, not a bad
        // arrangement — the client and host disagree on the shape.
        assert_eq!(
            ext.invoke(
                SET_LAYOUT_ACTION,
                IntrospectValue::Json(
                    json!({ "expected_revision": at, "tree": { "nodes": [], "root": "sideways" } })
                ),
            ),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(SET_LAYOUT_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
            "the tree arg is required",
        );
        assert_eq!(
            ext.invoke(
                SET_LAYOUT_ACTION,
                IntrospectValue::Json(json!({ "tree": { "nodes": [{ "leaf": 0 }], "root": 0 } })),
            ),
            Err(InvokeError::TypeMismatch),
            "so is the revision it was authored against — a write with no answer to \
             'which layout is this about?' cannot be adjudicated",
        );
        assert_eq!(query_layout(&mut ext), good, "and still untouched");
    }

    #[test]
    fn unknown_action_is_unknown_path() {
        let (mut ext, _rev) = control(&registry());
        assert_eq!(
            ext.invoke("teleport", IntrospectValue::Null),
            Err(InvokeError::UnknownPath)
        );
    }

    // ─── C1a: the session scope ───

    /// THE reason this surface carries a scope: a spawn scoped to `work` creates a pane in
    /// WORK, and the default session never sees it.
    ///
    /// The second half is a CONTROL, and without it this proves nothing: an unscoped surface
    /// over the SAME registry must land in the default. Otherwise a surface that always
    /// spawned into `work` — or always into whichever session happened to be first — would
    /// pass the first half exactly as well.
    #[test]
    fn a_spawn_lands_in_the_session_the_request_named_and_nowhere_else() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        let (mut work, _rev) = scoped_control(&reg, scope_of(&reg, "work"));
        work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(lock(&pool_of(&reg, "work")).panes().len(), 1);
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            0,
            "a spawn scoped to `work` must not reach the default session",
        );

        // The control: unscoped, same registry, lands in the default.
        let (mut default, _rev) = control(&reg);
        default
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        assert_eq!(
            lock(&pool(&reg)).panes().len(),
            1,
            "an unscoped spawn lands in the default",
        );
        assert_eq!(
            lock(&pool_of(&reg, "work")).panes().len(),
            1,
            "...and leaves `work` where it was",
        );
    }

    /// The same property for a WRITE, which is the half that would fail SILENTLY: a pane in
    /// the wrong session is at least visible, but an arrangement written to the wrong one
    /// corrupts a layout the client never named — and answers the client that it worked.
    #[test]
    fn a_layout_write_reaches_the_scoped_sessions_window_only() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();

        // Ids are minted from ONE registry-wide counter, so spawning work's pair first makes
        // them 0 and 1, and the default's 2 and 3 — distinct, which is what lets the
        // assertions below tell the two arrangements apart.
        let (mut work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        let (mut default, _d) = control(&reg);
        for _ in 0..2 {
            work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        for _ in 0..2 {
            default
                .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let default_before = query_layout(&mut default);
        assert_eq!(
            child(&default_before, &root_node(&default_before), "first")["leaf"],
            2,
            "the default session holds the second pair: {default_before}",
        );

        // Work's client drags its divider: vertical, 0.75, panes reversed.
        let at = query_layout(&mut work)["revision"]
            .as_u64()
            .expect("a revision");
        let answer = write_doc(work.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(json!({ "expected_revision": at, "tree": {
                "nodes": [
                    { "leaf": 1 },
                    { "leaf": 0 },
                    { "split": { "dir": "vertical", "ratio": 0.75, "first": 0, "second": 1 } },
                ],
                "root": 2,
            } })),
        ));
        let root = root_node(&answer);
        assert_eq!(root["split"]["ratio"], 0.75);
        assert_eq!(
            child(&answer, &root, "first")["leaf"],
            1,
            "work's own gesture stuck, in work's own window: {answer}",
        );
        assert_eq!(
            query_layout(&mut default),
            default_before,
            "the default session's arrangement was never touched — not its tree, not even \
             its revision",
        );
    }

    /// A read is scoped too, and by the same seam: `layout` answers about the session the
    /// request named. A client that read another session's arrangement would project it over
    /// its own panes.
    #[test]
    fn the_layout_slot_answers_about_the_scoped_session() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let (mut work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        let (mut default, _d) = control(&reg);

        // One pane in work, two in the default: the arrangements are different SHAPES, so
        // neither can be mistaken for the other.
        work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        for _ in 0..2 {
            default
                .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        assert_eq!(
            root_node(&query_layout(&mut work))["leaf"],
            0,
            "work has a lone pane at its root",
        );
        let default_tree = query_layout(&mut default);
        let root = root_node(&default_tree);
        assert_eq!(child(&default_tree, &root, "first")["leaf"], 1);
        assert_eq!(child(&default_tree, &root, "second")["leaf"], 2);
    }

    /// A pane of another session is not merely refused — it is not THERE. The scoped
    /// assembly builds pane children from the scoped pool alone, so scoping is structural:
    /// there is no check to forget.
    #[test]
    fn the_panes_slot_lists_only_the_scoped_sessions_panes() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let (mut work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        let (mut default, _d) = control(&reg);
        work.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        default
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();

        let ids = |ext: &mut WorkspaceExternal| -> Vec<u64> {
            let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
                panic!("the panes slot answers with a JSON array");
            };
            panes.iter().filter_map(|p| p["id"].as_u64()).collect()
        };
        assert_eq!(ids(&mut work), vec![0]);
        assert_eq!(ids(&mut default), vec![1]);
    }

    /// The `dead` key is ADDITIVE and appears exactly when the child has exited: absent while it
    /// runs (so a live pane's entry is byte-identical to the pre-liveness wire shape), present and
    /// `true` afterwards. That key is the only thing on the wire that distinguishes a finished
    /// command from a hung one — the pane itself stays either way.
    ///
    /// REVERT-PROOF: emit the key unconditionally and the live assertion fails; drop the emission
    /// and the exited one does.
    #[test]
    fn the_panes_slot_reports_a_dead_child_additively() {
        let reg = registry();
        let (mut ext, _guard) = control(&reg);
        // A child that exits at once, beside one that stays up.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["true"]})),
        )
        .unwrap();

        let entry = |ext: &mut WorkspaceExternal, id: u64| -> Value {
            let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
                panic!("the panes slot answers with a JSON array");
            };
            panes
                .into_iter()
                .find(|p| p["id"].as_u64() == Some(id))
                .expect("the pane is listed")
        };

        // Wait on the CONDITION the assertion reads, never on a timer.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while entry(&mut ext, 1).get("dead").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the short-lived child never reported dead"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(entry(&mut ext, 1)["dead"], json!(true));
        assert!(
            entry(&mut ext, 1)["id"].as_u64() == Some(1),
            "and it is still LISTED — a dead pane keeps its place"
        );
        assert!(
            entry(&mut ext, 0).get("dead").is_none(),
            "the live pane carries no key at all: {:?}",
            entry(&mut ext, 0)
        );
    }

    /// ...and HOW it exited follows, on its own key, once the host has reaped the child.
    ///
    /// A SECOND wait rather than an assertion folded into the one above, because the two facts are
    /// published at different moments: `dead` lands when the output stream ends, `child_exit` when
    /// `waitpid` returns. Asserting them together would be a race, and would also be a lie about
    /// what the wire promises.
    ///
    /// REVERT-PROOF: drop the `child_exit` emission and this never converges; emit it for a live
    /// pane too and the last assertion fails.
    #[test]
    fn the_panes_slot_reports_how_the_child_exited_once_it_is_reaped() {
        let reg = registry();
        let (mut ext, _guard) = control(&reg);
        // One child that FAILS with a code worth reading, beside one that stays up.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .unwrap();
        ext.invoke(
            SPAWN_ACTION,
            IntrospectValue::Json(json!({"cmd": ["sh", "-c", "exit 7"]})),
        )
        .unwrap();

        let entry = |ext: &mut WorkspaceExternal, id: u64| -> Value {
            let Some(IntrospectValue::Json(Value::Array(panes))) = ext.query(PANES_SLOT) else {
                panic!("the panes slot answers with a JSON array");
            };
            panes
                .into_iter()
                .find(|p| p["id"].as_u64() == Some(id))
                .expect("the pane is listed")
        };

        // Wait on the CONDITION the assertion reads — the reap, not the EOF that precedes it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while entry(&mut ext, 1).get("child_exit").is_none() {
            assert!(
                std::time::Instant::now() < deadline,
                "the failing child was never reaped: {:?}",
                entry(&mut ext, 1)
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            entry(&mut ext, 1)["child_exit"],
            json!({ "code": 7 }),
            "the code travels, and `signal` is absent for a process that returned normally",
        );
        assert_eq!(
            entry(&mut ext, 1)["dead"],
            json!(true),
            "and the liveness bit is still there — the status refines it, never replaces it",
        );
        assert!(
            entry(&mut ext, 0).get("child_exit").is_none(),
            "a live pane carries no status: {:?}",
            entry(&mut ext, 0)
        );
    }

    /// The session set is discoverable, so a client learns what it may name in `session` by
    /// ASKING rather than by guessing — and learns where an unscoped request lands. A RESTING
    /// empty anchor is HIDDEN (no pane, nobody attached), matching `tmux ls` at rest while the
    /// daemon lives on; see [`SessionInfo::is_listable`].
    #[test]
    fn the_sessions_slot_lists_working_sessions_and_hides_the_resting_anchor() {
        // Compare only the STRUCTURAL discovery fields — a session's live cwd / git branch (Slice 2)
        // depend on where the birth pane happens to run and on the host's git state, which is
        // orthogonal to what this slot promises (which sessions exist, and which is the default).
        let structural = |value: Option<IntrospectValue>| -> Vec<(String, u64, bool)> {
            let Value::Array(items) = answer_doc(value) else {
                panic!("the sessions slot answers with a JSON array");
            };
            items
                .iter()
                .map(|s| {
                    (
                        s["name"].as_str().unwrap().to_owned(),
                        s["windows"].as_u64().unwrap(),
                        s["default"].as_bool().unwrap(),
                    )
                })
                .collect()
        };

        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        // At boot the only session is the empty anchor "0": no pane, nobody attached, so the human
        // list is empty — the phantom "0" that `sprag ls` used to print at rest is gone.
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            Vec::<(String, u64, bool)>::new(),
            "the resting empty anchor is not listed",
        );

        // A pane makes the default session real: now it lists, and it still names itself default.
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            vec![("0".to_owned(), 1, true)],
            "a session holding a pane lists, and it is where an unscoped request goes",
        );

        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"name": "work"})),
        )
        .unwrap();
        assert_eq!(
            structural(ext.query(SESSIONS_SLOT)),
            vec![("0".to_owned(), 1, true), ("work".to_owned(), 1, false)],
            "the new session (born with a pane) is listed; creating it moved the default for nobody",
        );

        // This slot is deliberately registry-WIDE: a WORK-scoped surface still sees EVERY
        // session, not just its own. It is the one member whose subject is the set of scopes,
        // so scoping it would answer a question nobody asked — and a client discovers what it
        // may name by asking from wherever it happens to be scoped.
        let (work, _rev) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            work.query(SESSIONS_SLOT),
            ext.query(SESSIONS_SLOT),
            "the sessions list does not narrow to the caller's own scope",
        );
    }

    /// R282's SPLIT, at the surface that serves both halves: the session list carries no sampled
    /// field, and the sampled fields have their own address that answers them for every session.
    ///
    /// The two together are the property — either alone would pass under a mistake. A `sessions`
    /// answer with no `cwd` key would also be produced by a daemon that simply lost the fact, and an
    /// activity row would also be produced by one that kept serving it in both places. Asserting
    /// that the fact moved means asserting it left one address AND arrived at the other.
    #[test]
    fn the_session_list_carries_no_sampled_field_and_the_activity_address_does() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();

        let Value::Array(listed) = answer_doc(ext.query(SESSIONS_SLOT)) else {
            panic!("the sessions slot answers with a JSON array");
        };
        let row = listed.first().expect("the session holding the pane lists");
        for sampled in ["cwd", "branch", "ports"] {
            assert!(
                row.get(sampled).is_none(),
                "the session list must not carry {sampled}: {row}",
            );
        }

        // ZERO tolerance: this read samples for itself, so what it answers describes the pane just
        // spawned rather than anything held from before it existed.
        let reading = answer_doc(ext.query(&session_activity_at(0)));
        assert!(
            reading["sampled_ms_ago"].is_u64(),
            "the reading states its own age: {reading}",
        );
        let rows = reading["sessions"]
            .as_array()
            .expect("the reading carries a row per session");
        assert_eq!(
            rows.iter().map(|r| r["name"].as_str()).collect::<Vec<_>>(),
            vec![Some("0")],
            "one row, addressed by the same name the list uses: {reading}",
        );
        // The pane's cwd is wherever this test process runs, which is not a fact worth pinning; that
        // the sampled fact ARRIVED here is. `cwd` is the one of the three every live pane has.
        assert!(
            rows[0]["cwd"].is_string(),
            "a live pane's session reports where it is working: {reading}",
        );
    }

    /// R290's SPLIT, asserted the same way R282's is: the pane LIST carries nothing sampled, and the
    /// sampled address answers for every pane.
    ///
    /// Both halves, for the same reason as above — a pane list without process facts would also be
    /// produced by a daemon that never had them, and a `pane_processes` row would also be produced
    /// by one that served them in both places. The fact MOVING is the property.
    #[test]
    fn the_pane_list_carries_no_process_fact_and_the_processes_address_does() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();

        let Value::Array(listed) = answer_doc(ext.query(PANES_SLOT)) else {
            panic!("the panes slot answers with a JSON array");
        };
        let row = listed.first().expect("the spawned pane lists");
        for sampled in ["tty", "shell_pid", "foreground"] {
            assert!(
                row.get(sampled).is_none(),
                "the pane list must not carry {sampled}: {row}",
            );
        }

        // ZERO tolerance, so this read samples for itself and describes the pane just spawned.
        let reading = answer_doc(ext.query(&pane_processes_at(0)));
        assert!(
            reading["sampled_ms_ago"].is_u64(),
            "the reading states its own age: {reading}",
        );
        let rows = reading["panes"]
            .as_array()
            .expect("the reading carries a row per pane");
        assert_eq!(
            rows.iter().map(|r| r["id"].as_u64()).collect::<Vec<_>>(),
            listed.iter().map(|r| r["id"].as_u64()).collect::<Vec<_>>(),
            "a row per pane, addressed by the same id the pane list uses: {reading}",
        );
        assert!(
            rows[0]["shell_pid"].is_u64(),
            "and a live pane names the child the daemon spawned: {reading}",
        );
    }

    /// A malformed tolerance on the PROCESSES family answers `Null` too — one taxonomy for every
    /// parametric address on this surface, not one per family.
    #[test]
    fn a_malformed_process_tolerance_is_empty_not_absent() {
        let reg = registry();
        let (ext, _rev) = control(&reg);
        assert!(
            matches!(ext.query("pane_processes.zzz"), Some(IntrospectValue::Null)),
            "a malformed tolerance is a malformed MEMBER, not an unknown path",
        );
        assert!(
            ext.query("pane_processes").is_none(),
            "and the family's bare name is not itself an address",
        );
    }

    /// A malformed member of the activity family answers `Null` — present-but-empty — rather than
    /// absence, the taxonomy `cells.<offset>` established: `session_activity.zzz` IS in this
    /// surface's schema, so denying the address exists would be the wrong refusal.
    #[test]
    fn a_malformed_activity_tolerance_is_empty_not_absent() {
        let reg = registry();
        let (ext, _rev) = control(&reg);
        assert!(
            matches!(
                ext.query("session_activity.zzz"),
                Some(IntrospectValue::Null)
            ),
            "a malformed tolerance is a malformed MEMBER, not an unknown path",
        );
        assert!(
            ext.query("session_activity").is_none(),
            "and the family's bare name is not itself an address",
        );
    }

    /// A tolerance the held sample already meets is answered from THAT sample — the coalescing that
    /// makes N readers cost what one does.
    ///
    /// The control is the registry's own content rather than a clock: a session created between the
    /// two reads appears only in an answer that was freshly sampled. So the second read admitting a
    /// wide tolerance must NOT see it, and a third read admitting none must.
    #[test]
    fn a_tolerated_read_reuses_the_held_sample_and_a_zero_one_does_not() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        ext.invoke(SPAWN_ACTION, IntrospectValue::Null).unwrap();
        let rows = |value: Option<IntrospectValue>| -> usize {
            answer_doc(value)["sessions"]
                .as_array()
                .expect("a reading carries rows")
                .len()
        };
        assert_eq!(rows(ext.query(&session_activity_at(0))), 1, "one session");

        ext.invoke(
            NEW_SESSION_ACTION,
            IntrospectValue::Json(json!({"name": "work"})),
        )
        .unwrap();
        assert_eq!(
            rows(ext.query(&session_activity_at(3_600_000))),
            1,
            "an hour of tolerance is met by the held sample, which predates the new session",
        );
        assert_eq!(
            rows(ext.query(&session_activity_at(0))),
            2,
            "and a caller admitting no staleness pays for a sample that sees it",
        );
    }

    /// The tmux-SUPERIOR half of the listing rule: an EMPTY session a client is attached to still
    /// lists (so the client can see where it is), even though it holds no pane. tmux cannot reach
    /// this state at all — killing the last pane there destroys the session — so honestly showing it
    /// is a refinement, not a divergence. Proves the filter reads the HOST-filled `attached`, not
    /// just the registry's pane count.
    #[test]
    fn an_empty_session_a_client_is_attached_to_still_lists() {
        let session_names = |value: Option<IntrospectValue>| -> Vec<String> {
            let Value::Array(items) = answer_doc(value) else {
                panic!("the sessions slot answers with a JSON array");
            };
            items
                .iter()
                .map(|s| s["name"].as_str().unwrap().to_owned())
                .collect()
        };

        let reg = registry(); // the only session is the empty anchor "0"
        let attachments = Arc::new(Mutex::new(crate::AttachmentRegistry::default()));
        {
            let mut a = lock(&attachments);
            let conn = pinion_rpc::ConnId::allocate();
            a.hello(conn, "gui".to_owned());
            let id = lock(&reg).default_session().id();
            a.attach(conn, "0".to_owned(), id); // a client is viewing the empty anchor
        }
        let ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            crate::DaemonShared {
                on_pane_exit: None,
                attachments: Some(attachments),
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        );
        assert_eq!(
            session_names(ext.query(SESSIONS_SLOT)),
            vec!["0".to_owned()],
            "an empty session a client is attached to lists (the anchor would otherwise hide)",
        );
    }

    /// A name is an ADDRESS: two sessions sharing one would make a request ambiguous, so the
    /// duplicate is refused — and refused as a REJECTION, not a type error. The request was
    /// perfectly well-formed; it just cannot be honored.
    #[test]
    fn new_session_creates_by_name_and_refuses_a_duplicate() {
        let reg = registry();
        let (mut ext, revision) = control(&reg);
        let before = revision.current();

        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            ),
            Ok(IntrospectValue::Json(Value::String("work".to_owned()))),
            "the answer is the name, so a caller can scope its next request with it",
        );
        assert!(
            revision.current() > before,
            "the session SET changed, which a watching client must be woken for",
        );
        assert!(lock(&reg).session("work").is_some());

        let after = revision.current();
        let refused = ext
            .invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            )
            .unwrap_err();
        assert!(
            matches!(refused, InvokeError::Rejected(_)),
            "a taken name is a refusal, not a TypeMismatch — the request was well-formed, \
             got {refused:?}",
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            2,
            "the refused create added nothing"
        );
        assert_eq!(
            revision.current(),
            after,
            "and a refused create is inert — it must not even move the revision",
        );

        // A present name must be a STRING — a non-string is a malformed request, never
        // silently allocated (the scope param's own type-error corner).
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": 42}))
            ),
            Err(InvokeError::TypeMismatch),
        );
    }

    /// An ABSENT name is no longer an error: it asks the registry to ALLOCATE the lowest free
    /// one (tmux's `new-session` with no `-s`), and the answer tells the caller what it got —
    /// the only way it can learn a name it did not choose.
    #[test]
    fn an_unnamed_new_session_over_the_wire_allocates_the_lowest_free_name() {
        let reg = registry();
        let (mut ext, revision) = control(&reg);
        let before = revision.current();

        // The boot session is "0", so the first allocation is "1".
        assert_eq!(
            ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Ok(IntrospectValue::Json(Value::String("1".to_owned()))),
            "no name asks the registry to allocate the lowest free one",
        );
        assert!(
            revision.current() > before,
            "an allocation is a real create — a watching client must be woken",
        );

        // And the next one is "2": each allocation is its own independent session.
        assert_eq!(
            ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Ok(IntrospectValue::Json(Value::String("2".to_owned()))),
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            3,
            "the boot session, plus 1 and 2"
        );
    }

    /// tmux's `new-session`: a created session is BORN with one pane — never empty — and the
    /// birth pane takes the request's `cmd`/`cols`/`rows` (tmux's `new-session -x -y command`),
    /// so a client's first pane is exactly what it asked for. The DEFAULT session is untouched: a
    /// create births a pane in the NEW session, never the default (the daemon's empty anchor).
    #[test]
    fn a_new_session_is_born_with_a_shell() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(
                    json!({"name": "work", "cmd": ["cat"], "cols": 40, "rows": 12})
                ),
            ),
            Ok(IntrospectValue::Json(Value::String("work".to_owned()))),
        );
        assert_eq!(
            lock(&pool_of(&reg, "work")).panes().len(),
            1,
            "a session is born with exactly one pane (tmux new-session), never empty",
        );
        // The birth pane runs the request's cmd at its size — the caller's first pane, exact.
        let (work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            panes_without_projection(work.query(PANES_SLOT)).0,
            json!([{
                "id": 0, "cols": 40, "rows": 12, "command": "cat", "title": null, "active": true,
            }]),
            "the birth pane runs the request's cmd at its size, and the new session is ON it",
        );
        assert!(
            lock(&pool(&reg)).panes().is_empty(),
            "the default session is untouched — a create births a pane in the NEW session",
        );
    }

    /// A create CLAIMS the daemon's life across its own empty window, and hands it back afterwards
    /// — whether the birth pane made it or not.
    ///
    /// The claim is what stops an unrelated last pane's death from ending the daemon between the
    /// session existing and its shell existing ([`sprag_host::BirthPin`]). The failing half is the
    /// one worth pinning: a birth that cannot fork/exec is deliberately non-fatal, so a claim that
    /// only released on success would trade a daemon that exits too eagerly for one that never
    /// exits at all.
    ///
    /// REVERT-PROOF: hold the pin past the end of `new_session` (bind it in a `static`, or take it
    /// without a guard) and both assertions fail.
    #[test]
    fn a_create_releases_its_claim_however_the_birth_ends() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work", "cmd": ["cat"]})),
            )
            .is_ok(),
        );
        assert!(
            !lock(&reg).birth_in_flight(),
            "a born session claims nothing once its pane exists",
        );

        // A `cmd` the OS cannot exec: the session is created and left EMPTY (the one tolerated
        // non-fatal path), which is exactly when a leaked claim would be invisible and permanent.
        assert!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(
                    json!({"name": "broken", "cmd": ["/nonexistent/sprag-no-such-program"]})
                ),
            )
            .is_ok(),
            "a runtime exec failure is non-fatal — the session still exists",
        );
        assert!(
            !lock(&reg).birth_in_flight(),
            "and a birth that FAILED releases its claim too, or the daemon never exits",
        );
    }

    #[test]
    fn parse_remote_reads_the_endpoint_and_rejects_malformed() {
        let obj = |value: Value| value.as_object().cloned().unwrap();
        // Absent -> None; a full and a host-only endpoint parse.
        assert_eq!(WorkspaceExternal::parse_remote(&Map::new()), Ok(None));
        assert_eq!(
            WorkspaceExternal::parse_remote(&obj(
                json!({"remote": {"host": "srv", "user": "me", "port": 2222}})
            )),
            Ok(Some(SshRemote {
                user: Some("me".to_owned()),
                host: "srv".to_owned(),
                port: Some(2222),
            })),
        );
        assert_eq!(
            WorkspaceExternal::parse_remote(&obj(json!({"remote": {"host": "srv"}}))),
            Ok(Some(SshRemote {
                user: None,
                host: "srv".to_owned(),
                port: None,
            })),
        );
        // Malformed: no host, empty host, a zero/overflowing port, or a non-object all reject.
        for bad in [
            json!({"remote": {}}),
            json!({"remote": {"host": ""}}),
            json!({"remote": {"host": "srv", "port": 0}}),
            json!({"remote": {"host": "srv", "port": 99999}}),
            json!({"remote": "srv"}),
        ] {
            assert_eq!(
                WorkspaceExternal::parse_remote(&obj(bad)),
                Err(InvokeError::TypeMismatch),
            );
        }
    }

    #[test]
    fn a_new_session_with_a_remote_marks_its_birth_pane() {
        // Revert-proof for `spawn_parsed`'s `set_pane_remote`: drop it and the birth pane has no
        // endpoint, so this `expect` panics.
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({
                    "name": "remote",
                    "cmd": ["ssh", "-t", "srv"],
                    "remote": {"host": "srv", "user": "me", "port": 2222},
                })),
            ),
            Ok(IntrospectValue::Json(Value::String("remote".to_owned()))),
        );
        let pool = pool_of(&reg, "remote");
        let pool = lock(&pool);
        let pane = pool.panes().first().expect("born with a pane");
        let remote = pane
            .remote()
            .expect("the birth pane is marked a remote workspace");
        assert_eq!(remote.host, "srv");
        assert_eq!(remote.user.as_deref(), Some("me"));
        assert_eq!(remote.port, Some(2222));
    }

    /// A file dropped on an ORDINARY pane is pasted straight in as a local path: the file is already
    /// reachable from a pane running on this machine, so there is nothing to upload. Driven through
    /// the real action, and observed the only honest way — the `cat` pane ECHOES what reached its
    /// PTY, so this proves the paste, not just the return value.
    ///
    /// The name carries a space, which pins the quoting at the same time: an unquoted paste would
    /// hand the shell two words.
    #[test]
    fn a_dropped_file_on_a_local_pane_is_pasted_as_a_quoted_local_path() {
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("sprag-drop-unit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the drop dir");
        let dropped = dir.join("a file.txt");
        std::fs::write(&dropped, b"payload").expect("write the dropped file");
        let canonical = std::fs::canonicalize(&dropped).expect("canonicalize");
        let quoted = format!("'{}'", canonical.display());

        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        let id = ext
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("spawn a local pane");
        let IntrospectValue::Int(id) = id else {
            panic!("spawn answers the pane id")
        };

        assert_eq!(
            ext.invoke(
                DROP_FILE_ACTION,
                IntrospectValue::Json(json!({"pane": id, "path": canonical.to_str().unwrap()})),
            ),
            Ok(IntrospectValue::Json(json!({ "path": quoted }))),
            "a local pane is handed the dropped file's own path, shell-quoted",
        );

        // `cat` echoes what was pasted — the paste is what makes this more than a return value.
        let pool = pool(&reg);
        let pane = PaneId(u64::try_from(id).unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        let echoed = loop {
            let text = lock(&pool)
                .pane(pane)
                .expect("the pane is alive")
                .pty()
                .with_screen(sprag_vt::Screen::full_text);
            if text.contains(&quoted) {
                break true;
            }
            if Instant::now() > deadline {
                break false;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(echoed, "the quoted local path never reached the pane's PTY");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The refusals a drop must make, each a DIFFERENT category: a request that names no `path` is
    /// MALFORMED (`TypeMismatch`), while a well-formed request naming a pane that does not exist —
    /// or a file that does not — is `Rejected`. Collapsing the two would tell a client to fix the
    /// wrong end.
    #[test]
    fn drop_file_separates_a_malformed_request_from_a_refused_one() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        let id = ext
            .invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
            .expect("spawn a local pane");
        let IntrospectValue::Int(id) = id else {
            panic!("spawn answers the pane id")
        };

        assert_eq!(
            ext.invoke(DROP_FILE_ACTION, IntrospectValue::Json(json!({"pane": id}))),
            Err(InvokeError::TypeMismatch),
            "a drop with no path is malformed",
        );
        assert!(
            matches!(
                ext.invoke(
                    DROP_FILE_ACTION,
                    IntrospectValue::Json(json!({"pane": 9999, "path": "/etc/hostname"})),
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a well-formed drop on a pane that does not exist is refused, not a type error",
        );
        assert!(
            matches!(
                ext.invoke(
                    DROP_FILE_ACTION,
                    IntrospectValue::Json(json!({"pane": id, "path": "/no/such/file/at/all"})),
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a drop naming a file that cannot be resolved is refused",
        );
    }

    /// A session born via `new_session` feeds the reaper when its birth pane dies — the guard
    /// that the birth pane is HOOKED with the death-signal. This is WHY the birth spawn lives at
    /// the host layer (which carries [`on_pane_exit`](WorkspaceExternal::on_pane_exit)) and NOT
    /// in the pinion-free registry: a registry-side birth would be the unhooked-pane category
    /// R160/R161 closed, leaving a daemon lingering over a dead session. Driven through the real
    /// action, so a refactor that moved the spawn off the hooked path would fail here.
    #[test]
    fn a_new_session_born_pane_feeds_the_reaper() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::time::{Duration, Instant};

        let reg = registry();
        let fired = Arc::new(AtomicUsize::new(0));
        let f = Arc::clone(&fired);
        // The signal is counted DIRECTLY rather than through [`crate::spawn_reaper`], and that is
        // what makes this test sound. Routed through the reaper, the observable is `on_empty`,
        // which fires only when a signal is DRAINED while the workspace happens to be empty — and
        // draining is asynchronous, so the count is at-least-once and attributable to no particular
        // sender. Measured both ways: asserting `== 1` on it failed about 1 full-suite run in 5
        // (two signals drained after the death, 15us apart), and relaxing to `>= 1` made the guard
        // VACUOUS — unhooking the pane still passed, because the [`crate::BirthPin`]'s own release
        // signal was drained late and fired instead. Counting sends removes both problems: a send
        // is synchronous and each one is attributable.
        let signal: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            f.fetch_add(1, Ordering::SeqCst);
        });
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            crate::DaemonShared {
                on_pane_exit: Some(signal),
                attachments: None,
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        );
        // `new_session` sends exactly one signal by itself: the [`crate::BirthPin`] it takes fires
        // on release, deliberately, so a birth that FAILED still lets an idle daemon go. A BLOCKING
        // birth (`cat` waits on stdin) keeps the pane's own signal out of that count until this
        // test asks for it, which is what separates the two senders.
        let born = ext
            .invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({"cmd": ["cat"]})),
            )
            .expect("new_session births a pane");
        let IntrospectValue::Json(Value::String(name)) = born else {
            panic!("new_session answers the allocated session name");
        };
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the pin's release is the one signal a birth sends on its own",
        );

        // Now end the pane. A SECOND signal can come from one place only — the `on_exit` the birth
        // spawn hooked onto it — so this is the assertion the test is named for: unhook the pane
        // and the count stays at the pin's 1 forever.
        let pool = lock(&reg)
            .workspace_of(&name)
            .expect("the born session resolves");
        let id = lock(&pool).panes().first().expect("the birth pane").id();
        drop(lock(&pool).close(id));
        let start = Instant::now();
        while fired.load(Ordering::SeqCst) < 2 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            fired.load(Ordering::SeqCst),
            2,
            "the birth pane's death fed the death-signal, proving it was hooked — a \
             registry-side birth would feed no reaper and leave a lingering daemon",
        );
    }

    /// A MALFORMED birth spec is rejected BEFORE the session is created — uniform with the `spawn`
    /// action and with a bad `name`. Birthing at this authority buys uniform validation; a `cmd`
    /// that is not an array, or a non-`u16` `cols`/`rows`, must not slip through as a silently
    /// empty session reported as success.
    #[test]
    fn new_session_rejects_a_malformed_birth_spec_and_creates_nothing() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        for bad in [
            json!({"cmd": 42}),
            json!({"name": "work", "cmd": "cat"}),
            json!({"cols": 0}),
        ] {
            assert_eq!(
                ext.invoke(NEW_SESSION_ACTION, IntrospectValue::Json(bad.clone())),
                Err(InvokeError::TypeMismatch),
                "a malformed birth spec is a type error, not a silent empty session: {bad}",
            );
        }
        assert_eq!(
            lock(&reg).sessions().len(),
            1,
            "every rejected create built nothing — only the boot session remains",
        );
    }

    /// A RUNTIME birth failure (an argv the OS cannot `exec`) is non-fatal: the session is still
    /// created (a valid attach target, merely empty) and answered with its name — so "a well-formed
    /// create with a free name succeeds" stays total. This is the ONE case a created session is
    /// empty, distinct from a malformed request (which creates nothing — the guard above).
    #[test]
    fn a_new_session_whose_birth_pane_cannot_exec_is_created_empty() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        assert_eq!(
            ext.invoke(
                NEW_SESSION_ACTION,
                IntrospectValue::Json(json!({
                    "name": "work",
                    "cmd": ["/nonexistent/definitely-not-a-real-binary"],
                })),
            ),
            Ok(IntrospectValue::Json(Value::String("work".to_owned()))),
            "a well-formed create with a free name succeeds even when the birth pane cannot exec",
        );
        assert!(
            lock(&reg).session("work").is_some(),
            "the session exists as a valid attach target",
        );
        assert!(
            lock(&pool_of(&reg, "work")).panes().is_empty(),
            "...but it is empty: the birth pane's exec failed, non-fatally",
        );
    }

    /// Killing a NON-last session over the wire removes it and answers `null`, and — a set
    /// change — wakes a client watching the sessions list.
    #[test]
    fn kill_session_removes_a_non_last_session_over_the_wire() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        let (mut ext, revision) = control(&reg);
        let before = revision.current();

        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            ),
            ended(sprag_terminal::Ended::Session),
        );
        assert!(lock(&reg).session("work").is_none(), "the session is gone");
        assert_eq!(lock(&reg).sessions().len(), 1, "only the default remains");
        assert!(
            revision.current() > before,
            "the session set changed, which a watching client must be woken for",
        );

        // An unknown name is a REJECTION, not a type error; a missing / non-string name IS a
        // type error (you must name the session to kill).
        assert!(
            matches!(
                ext.invoke(
                    KILL_SESSION_ACTION,
                    IntrospectValue::Json(json!({"name": "ghost"})),
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "ghost"})),
            ),
        );
        assert_eq!(
            ext.invoke(KILL_SESSION_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": 42})),
            ),
            Err(InvokeError::TypeMismatch),
        );
    }

    /// A kill RELEASES the dead session's viewers, driven through the real invoke rather than
    /// through [`AttachmentRegistry::session_ended`] directly — a unit test on the method is not a
    /// test that the caller calls it, and this caller is the only one there is.
    ///
    /// Two things are asserted, and the second is why it matters: the badge falls, and a NEW
    /// session taking the freed name does NOT inherit the viewer. Measured at R303, the daemon did
    /// both wrong — `sprag list-clients` named a dead session and `sprag ls` credited the impostor
    /// with a viewer it never had.
    ///
    /// The kept session is the CONTROL: its viewer must survive, or "released the attachments"
    /// would read the same as "dropped the whole registry".
    #[test]
    fn killing_a_session_releases_its_viewers_and_a_new_one_of_that_name_inherits_none() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        lock(&reg).new_session(Some("keeper")).unwrap();
        let attachments = Arc::new(Mutex::new(crate::AttachmentRegistry::default()));
        {
            let mut a = lock(&attachments);
            for (client, session) in [("gui", "work"), ("tui", "work"), ("other", "keeper")] {
                let conn = pinion_rpc::ConnId::allocate();
                a.hello(conn, client.to_owned());
                let id = lock(&reg)
                    .session(session)
                    .expect("the fixture session")
                    .id();
                a.attach(conn, session.to_owned(), id);
            }
        }
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            crate::DaemonShared {
                on_pane_exit: None,
                attachments: Some(Arc::clone(&attachments)),
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        );
        assert_eq!(lock(&attachments).attached_count("work"), 2, "two viewers");

        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "work"})),
            ),
            ended(sprag_terminal::Ended::Session),
        );

        assert_eq!(
            lock(&attachments).attached_count("work"),
            0,
            "the killed session's viewers were released by the kill itself",
        );
        assert_eq!(
            lock(&attachments).attached_count("keeper"),
            1,
            "control: a session that was NOT killed keeps its viewer",
        );

        // The inheritance. A fresh session takes the freed name; nobody is watching it.
        lock(&reg).new_session(Some("work")).unwrap();
        assert_eq!(
            lock(&attachments).attached_count("work"),
            0,
            "a new session of the same name must inherit no viewer",
        );
        assert!(
            lock(&attachments)
                .clients()
                .iter()
                .all(|info| info.session == "keeper"),
            "and list-clients names only sessions that exist",
        );
    }

    /// Killing the LAST session ENDS the daemon: it fires the injected death-signal (the same
    /// one a pane's exit does), so the reaper re-checks liveness and exits through the SIGTERM
    /// funnel. Proven with a recording signal — the empty last session drains nothing, so this
    /// firing, not a pane Drop, is what triggers the exit there.
    #[test]
    fn kill_session_on_the_last_ends_the_daemon_via_the_death_signal() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = registry(); // the only session, "0", empty
        let fired = Arc::new(AtomicUsize::new(0));
        let signal: Arc<dyn Fn() + Send + Sync> = {
            let fired = Arc::clone(&fired);
            Arc::new(move || {
                fired.fetch_add(1, Ordering::SeqCst);
            })
        };
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            crate::DaemonShared {
                on_pane_exit: Some(signal),
                attachments: None,
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        );

        assert_eq!(
            ext.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "0"})),
            ),
            ended(sprag_terminal::Ended::Server),
        );
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "killing the last session fired the daemon-exit signal exactly once",
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            1,
            "the last session is NOT removed — the shell is kept so the default stays total",
        );

        // Off a daemon (no injected signal), the same kill still removes/drains but nothing
        // exits — exactly right for a GUI's in-process host and the tests.
        let (mut headless, _rev) = control(&reg);
        assert_eq!(
            headless.invoke(
                KILL_SESSION_ACTION,
                IntrospectValue::Json(json!({"name": "0"})),
            ),
            ended(sprag_terminal::Ended::Server),
            "a last-session kill with no reaper is a no-op exit, not an error",
        );
    }

    // ─── windows: new / select / rename / kill over the wire ───

    /// `new_window` over the surface creates a window in the SCOPED session, SELECTS it, births a
    /// shell into it, and answers with its name — the birth landing in the NEW window, not the one
    /// the request was scoped to.
    #[test]
    fn new_window_creates_a_selected_window_born_with_a_pane() {
        let reg = registry();
        let (mut ext, rev) = control(&reg); // scoped to session "0", window "0"
        let before = rev.current();

        let created = ext
            .invoke(
                NEW_WINDOW_ACTION,
                IntrospectValue::Json(json!({"cmd": ["cat"]})),
            )
            .unwrap();
        assert_eq!(
            created,
            IntrospectValue::Json(Value::String("1".to_owned())),
            "the lowest free window name",
        );
        assert!(rev.current() > before, "a window creation wakes waiters");

        // The windows slot (read fresh) shows two, the new one current.
        assert_eq!(
            without_ids(answer_doc(ext.query(WINDOWS_SLOT))),
            json!([
                {"name": "0", "current": false},
                {"name": "1", "current": true},
            ]),
        );

        // The birth pane landed in the NEW window: a fresh scope to the session (now current =
        // "1") sees exactly one pane, while the request's own window "0" is still empty.
        let (fresh, _r) = scoped_control(&reg, scope_of(&reg, "0"));
        let Some(IntrospectValue::Json(Value::Array(panes))) = fresh.query(PANES_SLOT) else {
            panic!("the panes slot answers with a JSON array");
        };
        assert_eq!(panes.len(), 1, "the new window is born with its shell");
        assert!(
            lock(ext.workspace()).panes().is_empty(),
            "and the window the request was scoped to is untouched",
        );
    }

    /// `select_window` moves the session's current window (a set-ish change that wakes waiters),
    /// and a missing / unknown target is refused with the current window left put.
    #[test]
    fn select_window_moves_the_current_window() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        lock(&reg)
            .new_window("0", Some("logs"), WindowBirth::default())
            .unwrap(); // current is now "logs"
        let before = rev.current();

        assert_eq!(
            ext.invoke(
                SELECT_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "0"}))
            ),
            // The window it LANDED on (R305), where this used to answer null. The named arm knew
            // already; giving it the same answer as the STEP arm — which cannot know — is what
            // keeps one shape for one verb.
            Ok(IntrospectValue::Json(json!("0"))),
        );
        assert!(rev.current() > before, "a select wakes waiters to re-read");
        assert_eq!(
            without_ids(answer_doc(ext.query(WINDOWS_SLOT))),
            json!([
                {"name": "0", "current": true},
                {"name": "logs", "current": false},
            ]),
        );

        // A target is required, and an unknown one is a rejection (well-formed, unhonorable).
        assert_eq!(
            ext.invoke(SELECT_WINDOW_ACTION, IntrospectValue::Json(json!({}))),
            Err(InvokeError::TypeMismatch),
        );
        assert!(
            matches!(
                ext.invoke(
                    SELECT_WINDOW_ACTION,
                    IntrospectValue::Json(json!({"window": "ghost"}))
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(
                SELECT_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "ghost"}))
            ),
        );
    }

    /// `rename_window` renames the CURRENT window by default (`window` absent ⇒ the scope's),
    /// answers the name it RECORDED, and refuses a rename onto a name another window holds or one
    /// that breaks the grammar.
    #[test]
    fn rename_window_renames_the_current_by_default_and_refuses_a_duplicate() {
        let reg = registry();
        let (mut ext, _r) = control(&reg); // scope window "0"

        // THE GRAMMAR FIRST, and the ORDER is the assertion. A rename moves the name this
        // request's SCOPE names, so once the window is called `main` a scope still saying `0`
        // refuses everything with `Unknown` — which is `Rejected` on the wire and looks exactly
        // like a refused NAME. The first version of this test asserted the two below AFTER the
        // rename and passed with the daemon's grammar deleted, which is the vacuous-fixture hazard
        // this project has now recorded four times: choose a fixture where the two things being
        // told apart actually disagree.
        assert!(
            matches!(
                ext.invoke(
                    RENAME_WINDOW_ACTION,
                    IntrospectValue::Json(json!({"name": "   "}))
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a blank name is refused where it used to be STORED — the loud half of the bump",
        );
        assert!(
            matches!(
                ext.invoke(
                    RENAME_WINDOW_ACTION,
                    IntrospectValue::Json(json!({"name": "a\nb"}))
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a newline would forge a row of every listing that prints a window name",
        );
        assert_eq!(
            without_ids(answer_doc(ext.query(WINDOWS_SLOT))),
            json!([{"name": "0", "current": true}]),
            "and neither refusal touched the window, which is what makes them about the NAME",
        );

        // window absent ⇒ the current window ("0") is renamed. The argument is PADDED and the
        // answer is not: what comes back is what the registry recorded, which is the whole reason
        // this answer exists (R306) and the discriminator against a handler that echoed its input.
        assert_eq!(
            ext.invoke(
                RENAME_WINDOW_ACTION,
                IntrospectValue::Json(json!({"name": "  main  "}))
            ),
            Ok(IntrospectValue::Json(json!({"name": "main"}))),
        );
        lock(&reg)
            .new_window("0", Some("logs"), WindowBirth::default())
            .unwrap();
        // Renaming "logs" onto the taken name "main" is refused.
        assert!(
            matches!(
                ext.invoke(
                    RENAME_WINDOW_ACTION,
                    IntrospectValue::Json(json!({"window": "logs", "name": "main"})),
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(
                RENAME_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "logs", "name": "main"})),
            ),
        );
        // The rename took and "logs" kept its name; the slot reads the session fresh, so "current"
        // reflects reality ("logs", which new_window selected).
        assert_eq!(
            without_ids(answer_doc(ext.query(WINDOWS_SLOT))),
            json!([
                {"name": "main", "current": false},
                {"name": "logs", "current": true},
            ]),
        );
    }

    /// `resize_window`'s ACTION-level argument rules, which no CLI test can reach: the `sprag` verb
    /// refuses these combinations before they ever cross the wire, so without this the guard is
    /// unfalsifiable — and the wire is a public surface a GUI or an agent addresses directly.
    ///
    /// Measured: replacing the exclusivity arm with a precedence order left the whole suite green.
    #[test]
    fn resize_window_refuses_two_spellings_of_one_rectangle() {
        let reg = registry();
        let (mut ext, _r) = control(&reg); // scope window "0"

        // The four legal spellings, so the refusals below are about MIXING rather than about any one
        // of them being unreadable. `from` folds no clients here (none are attached), so it refuses
        // for its own honest reason — which is itself the distinction that keeps an unresolvable
        // request from silently becoming an un-pin.
        assert_eq!(
            ext.invoke(
                RESIZE_WINDOW_ACTION,
                IntrospectValue::Json(json!({"cols": 100, "rows": 30})),
            ),
            Ok(IntrospectValue::Json(json!({"cols": 100, "rows": 30}))),
            "an exact rectangle is answered with itself",
        );
        assert_eq!(
            ext.invoke(
                RESIZE_WINDOW_ACTION,
                IntrospectValue::Json(json!({"adjust_cols": -20})),
            ),
            Ok(IntrospectValue::Json(json!({"cols": 80, "rows": 30}))),
            "a relative request moves the pin and leaves the unnamed axis",
        );
        assert_eq!(
            ext.invoke(RESIZE_WINDOW_ACTION, IntrospectValue::Json(json!({}))),
            Ok(IntrospectValue::Json(Value::Null)),
            "nothing named is an un-pin, answered as `null`",
        );

        for args in [
            json!({"cols": 80, "rows": 24, "adjust_cols": 4}),
            json!({"cols": 80, "rows": 24, "from": "largest"}),
            json!({"adjust_rows": 2, "from": "smallest"}),
            // Half a rectangle, which is its own rule rather than a mixing one.
            json!({"cols": 80}),
            json!({"rows": 24}),
            // `manual` folds no clients, so as a SOURCE it names nothing.
            json!({"from": "manual"}),
            json!({"from": "nonsense"}),
            json!({"from": 7}),
            json!({"adjust_cols": "wide"}),
        ] {
            assert_eq!(
                ext.invoke(RESIZE_WINDOW_ACTION, IntrospectValue::Json(args.clone())),
                Err(InvokeError::TypeMismatch),
                "{args} must be refused",
            );
        }
    }

    /// **A kill addressed by IDENTITY destroys the window that was pointed at, over the wire** —
    /// the daemon's half of R330, driven through the action a client really sends.
    ///
    /// The registry gate proves the resolution; this proves the ACTION carries the address there,
    /// which is a different claim: `window_target`'s absent-key default used to be the only reading
    /// of a kill request, so a `window_id` reaching an unchanged handler would have killed the
    /// SCOPED window and looked like a success.
    ///
    /// The rename shuffle is what makes the two readings disagree. Both are asserted, because the
    /// NAME arm is not the defect — `sprag kill-window -t s alpha` means whatever holds it now.
    ///
    /// REVERT-PROOF: drop the `WindowRef::read` and default the subject to the scope, and the
    /// identity arm kills the current window instead; make the handler prefer `window` when both
    /// keys arrive and the refusal below goes green with a kill nobody asked for.
    #[test]
    fn a_kill_addressed_by_identity_destroys_the_window_pointed_at() {
        let reg = registry();
        for name in ["alpha", "beta"] {
            lock(&reg)
                .new_window("0", Some(name), WindowBirth::default())
                .unwrap();
        }
        let (mut ext, _rev) = control(&reg);
        let id_of = |name: &str| {
            lock(&reg)
                .session("0")
                .expect("the default session")
                .windows()
                .iter()
                .find(|w| w.name() == name)
                .map(|w| w.id().0)
        };
        fn names(ext: &mut WorkspaceExternal) -> Vec<String> {
            answer_doc(ext.query(WINDOWS_SLOT))
                .as_array()
                .expect("a list")
                .iter()
                .map(|row| row["name"].as_str().expect("a name").to_owned())
                .collect()
        }
        let pointed = id_of("alpha").expect("alpha exists");

        // Another client renames while the confirmation dialog is up.
        lock(&reg).rename_window("0", "alpha", "archive").unwrap();
        lock(&reg).rename_window("0", "beta", "alpha").unwrap();

        assert!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window_id": pointed })),
            )
            .is_ok()
        );
        assert_eq!(
            names(&mut ext),
            vec!["0", "alpha"],
            "the POINT killed the window it named, whatever it is called now",
        );

        // ...and a NAME still means whatever holds it, which is the reading a typed argument has.
        assert!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": "alpha" })),
            )
            .is_ok()
        );
        assert_eq!(names(&mut ext), vec!["0"]);

        // A name AND an identity names two windows, so it names none: refused, nothing killed.
        assert!(matches!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": "0", "window_id": 0 })),
            ),
            Err(InvokeError::TypeMismatch)
        ));
        assert_eq!(names(&mut ext), vec!["0"], "a refusal kills nothing");

        // ...and NEITHER key is the request's SCOPED window — the reading `sprag kill-window -t s`
        // with no argument sends, and the branch `WindowRef::read`'s `Ok(None)` exists for.
        //
        // A FRESH control, because a scope freezes the window it resolved on and this one's died
        // above — which is the scope's own documented behaviour and not this branch's business.
        lock(&reg)
            .new_window("0", Some("spare"), WindowBirth::default())
            .unwrap();
        let (mut fresh, _rev) = control(&reg);
        assert_eq!(names(&mut fresh), vec!["0", "spare"]);
        assert!(
            fresh
                .invoke(KILL_WINDOW_ACTION, IntrospectValue::Json(json!({})))
                .is_ok()
        );
        assert_eq!(
            names(&mut fresh),
            vec!["0"],
            "an absent reference is the SCOPED window, which `new_window` had just made current",
        );
    }

    /// Killing a NON-last window over the wire removes it, keeps the current window valid, and
    /// wakes a client watching the windows list.
    #[test]
    fn kill_window_removes_a_non_last_window_over_the_wire() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        lock(&reg)
            .new_window("0", Some("logs"), WindowBirth::default())
            .unwrap(); // current = "logs"; two windows
        let before = rev.current();

        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "logs"}))
            ),
            ended(sprag_terminal::Ended::Window),
        );
        assert!(rev.current() > before, "a window kill wakes waiters");
        assert_eq!(
            without_ids(answer_doc(ext.query(WINDOWS_SLOT))),
            json!([{"name": "0", "current": true}]),
            "logs is gone and the current fell back to the surviving window",
        );
        assert!(
            matches!(
                ext.invoke(
                    KILL_WINDOW_ACTION,
                    IntrospectValue::Json(json!({"window": "ghost"}))
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "ghost"}))
            ),
        );
    }

    /// Killing a session's LAST window ends the SESSION (tmux) — the escalation removes the whole
    /// session when it is not the last one.
    #[test]
    fn kill_window_on_a_sessions_last_window_ends_the_session() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap(); // "work" holds one window "0"
        assert_eq!(lock(&reg).sessions().len(), 2);

        let (mut work, _r) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            work.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "0"}))
            ),
            ended(sprag_terminal::Ended::Session),
        );
        assert!(
            lock(&reg).session("work").is_none(),
            "the session went with its last window",
        );
        assert_eq!(lock(&reg).sessions().len(), 1, "only the default remains");
    }

    /// Killing the last window of the LAST session ends the DAEMON: the escalation reaches
    /// `kill_session`'s last-session arm, firing the injected death-signal so the reaper exits
    /// through the SIGTERM funnel — the same path a `kill_session` of the last session takes.
    #[test]
    fn kill_window_on_the_last_window_of_the_last_session_ends_the_daemon() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let reg = registry(); // one session "0", one window "0", empty
        let fired = Arc::new(AtomicUsize::new(0));
        let signal: Arc<dyn Fn() + Send + Sync> = {
            let fired = Arc::clone(&fired);
            Arc::new(move || {
                fired.fetch_add(1, Ordering::SeqCst);
            })
        };
        let mut ext = WorkspaceExternal::new(
            Arc::clone(&reg),
            SessionScope::unscoped(&reg),
            Arc::new(ChannelRegistry::default()),
            crate::DaemonShared {
                on_pane_exit: Some(signal),
                attachments: None,
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        );

        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({"window": "0"}))
            ),
            ended(sprag_terminal::Ended::Server),
        );
        assert_eq!(
            fired.load(Ordering::SeqCst),
            1,
            "the last window's kill escalated to a last-session kill and fired the exit signal once",
        );
        assert_eq!(
            lock(&reg).sessions().len(),
            1,
            "the last session is drained, not removed — the default stays total",
        );
    }

    /// **The `windows` slot publishes each window's IDENTITY, and a RENAME does not move it** — the
    /// fact a client needs to address a window it painted a moment ago (R329).
    ///
    /// Read against the registry's own minted ids rather than literals, so this asserts the slot
    /// SERVES the identity and not that a fixture allocates 1 and 2.
    ///
    /// The rename is what makes it a test of an identity rather than of a field: the row that comes
    /// back carries a different NAME and the same id, which is precisely the pair a client needs to
    /// tell "the window I picked" from "whatever is called that now". Every other window test here
    /// reads through `without_ids`, so this is the only place that would notice the key going away.
    ///
    /// REVERT-PROOF: drop `id` from `Session::window_infos` and the first assertion sees `None`;
    /// key it off the window's POSITION and the rename assertion still passes while a reorder
    /// breaks it — which is why the pin is the rename and not the read alone.
    #[test]
    fn the_windows_slot_publishes_each_windows_identity() {
        let reg = registry();
        lock(&reg)
            .new_window("0", Some("logs"), WindowBirth::default())
            .unwrap();
        let (mut ext, _rev) = control(&reg);

        let minted: Vec<Value> = lock(&reg)
            .session("0")
            .expect("the default session")
            .windows()
            .iter()
            .map(|window| Value::from(window.id().0))
            .collect();
        assert_eq!(minted.len(), 2, "two windows, two identities");
        let served = |ext: &mut WorkspaceExternal| {
            answer_doc(ext.query(WINDOWS_SLOT))
                .as_array()
                .expect("a list")
                .iter()
                .map(|row| {
                    (
                        row["name"].as_str().expect("a name").to_owned(),
                        row.get("id").cloned(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            served(&mut ext),
            vec![
                ("0".to_owned(), Some(minted[0].clone())),
                ("logs".to_owned(), Some(minted[1].clone())),
            ],
        );

        lock(&reg).rename_window("0", "logs", "archive").unwrap();
        assert_eq!(
            served(&mut ext),
            vec![
                ("0".to_owned(), Some(minted[0].clone())),
                ("archive".to_owned(), Some(minted[1].clone())),
            ],
            "a rename moves the NAME and leaves the identity where it was",
        );
    }

    /// The `windows` slot is SCOPED: a session sees only its OWN windows, with the current one
    /// marked — the read a tabbed client draws from.
    #[test]
    fn the_windows_slot_lists_the_scoped_sessions_windows_and_marks_current() {
        let reg = registry();
        lock(&reg).new_session(Some("work")).unwrap();
        lock(&reg)
            .new_window("work", Some("logs"), WindowBirth::default())
            .unwrap(); // work: "0", "logs"(current)

        let (default_ext, _d) = control(&reg);
        assert_eq!(
            without_ids(answer_doc(default_ext.query(WINDOWS_SLOT))),
            json!([{"name": "0", "current": true}]),
            "the default session sees only its own one window",
        );

        let (work, _w) = scoped_control(&reg, scope_of(&reg, "work"));
        assert_eq!(
            without_ids(answer_doc(work.query(WINDOWS_SLOT))),
            json!([
                {"name": "0", "current": false},
                {"name": "logs", "current": true},
            ]),
            "work sees its two windows, logs current",
        );
    }

    /// The per-window-revision bound (d): a `set_layout` that NAMES a window other than the one
    /// the request is scoped to is refused — the belt to the revision compare-and-set's suspenders,
    /// so a client that switched windows cannot land a stale write on the wrong one whose revision
    /// happened to collide. The CONTROL is the same gesture naming the scoped window, which applies.
    #[test]
    fn set_layout_refuses_a_write_authored_against_a_different_window() {
        let reg = registry();
        let (mut ext, _r) = control(&reg); // scope.window() == "0"
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let good = query_layout(&mut ext);
        let at = good["revision"].as_u64().expect("a revision");
        let gesture = |window: &str| {
            json!({
                "expected_revision": at,
                "expected_window": window,
                "tree": {
                    "nodes": [
                        { "leaf": 1 },
                        { "leaf": 0 },
                        { "split": { "dir": "vertical", "ratio": 0.75, "first": 0, "second": 1 } },
                    ],
                    "root": 2,
                },
            })
        };

        // Naming a window OTHER than the scoped one is refused: the arrangement in force is
        // untouched, and the answer is that truth for the client to re-project.
        let answer = write_doc(ext.invoke(
            SET_LAYOUT_ACTION,
            IntrospectValue::Json(gesture("elsewhere")),
        ));
        assert_eq!(
            answer, good,
            "a window mismatch refused it; window 0 kept its arrangement"
        );

        // Control: naming the ACTUAL scoped window lets the SAME gesture through.
        let answer = write_doc(ext.invoke(SET_LAYOUT_ACTION, IntrospectValue::Json(gesture("0"))));
        let root = root_node(&answer);
        assert_eq!(
            root["split"]["ratio"], 0.75,
            "naming the current window applied the gesture",
        );
        assert_eq!(child(&answer, &root, "first")["leaf"], 1, "the order stuck");
    }

    /// The window actions refuse a NON-STRING arg where the ABI promises a string — never a silent
    /// fall-back (the same aliasing corner the session scope param refuses).
    #[test]
    fn window_actions_reject_non_string_args() {
        let reg = registry();
        let (mut ext, _r) = control(&reg);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let at = query_layout(&mut ext)["revision"]
            .as_u64()
            .expect("a revision");

        // set_layout: a non-string `expected_window` is malformed, like a wrong-typed
        // `expected_revision` — not a silent "no window check".
        assert_eq!(
            ext.invoke(
                SET_LAYOUT_ACTION,
                IntrospectValue::Json(json!({
                    "expected_revision": at, "expected_window": 42,
                    "tree": { "nodes": [], "root": null },
                })),
            ),
            Err(InvokeError::TypeMismatch),
        );
        // rename / kill: a non-string `window` target is malformed (never a silent fall-back to
        // the current window and acting on the wrong one).
        assert_eq!(
            ext.invoke(
                RENAME_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": 42, "name": "x" })),
            ),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                KILL_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": 42 }))
            ),
            Err(InvokeError::TypeMismatch),
        );
        // new_window / select_window: a non-string name / target is malformed too.
        assert_eq!(
            ext.invoke(
                NEW_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "name": 42 }))
            ),
            Err(InvokeError::TypeMismatch),
        );
        assert_eq!(
            ext.invoke(
                SELECT_WINDOW_ACTION,
                IntrospectValue::Json(json!({ "window": 42 })),
            ),
            Err(InvokeError::TypeMismatch),
        );
    }

    /// Which pane each row of the `panes` slot says is ACTIVE — the wire's own answer, read the way
    /// a client reads it rather than through the registry behind it.
    fn active_row(ext: &mut WorkspaceExternal) -> Option<u64> {
        let Some(IntrospectValue::Json(Value::Array(rows))) = ext.query(PANES_SLOT) else {
            panic!("the panes slot answers a list");
        };
        let marked: Vec<u64> = rows
            .iter()
            .filter(|row| row["active"] == Value::Bool(true))
            .map(|row| row["id"].as_u64().expect("a pane id"))
            .collect();
        assert!(
            marked.len() <= 1,
            "at most ONE row is active — these rows are one window's panes: {marked:?}",
        );
        marked.first().copied()
    }

    /// Two spawns and a split, giving a window whose arrangement is `0 | (1 over 2)` — the shape
    /// that separates a structural neighbour walk from "the next pane in the list".
    ///
    /// It leaves the session ON pane 2, because a split makes its new pane active (tmux's rule).
    fn three_pane_window(ext: &mut WorkspaceExternal) {
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        ext.invoke(
            SPLIT_ACTION,
            IntrospectValue::Json(json!({"pane": 1, "dir": "vertical", "cmd": ["cat"]})),
        )
        .unwrap();
    }

    /// A SPLIT carries the birth vocabulary too, and refuses an opener that is gone with the
    /// arrangement untouched.
    ///
    /// Pinned because the fact has to be TOTAL over births: a provenance available on one of the two
    /// birth actions and not the other would mean what a pane can say about itself depends on which
    /// action happened to make it, and the two are the same birth with and without a place.
    #[test]
    fn a_split_records_the_pane_that_asked_and_refuses_an_opener_that_is_gone() {
        let reg = registry();
        let (mut ext, _revision) = control(&reg);
        three_pane_window(&mut ext); // 0 | (1 / 2)

        ext.invoke(
            SPLIT_ACTION,
            IntrospectValue::Json(
                json!({"pane": 0, "dir": "horizontal", "cmd": ["cat"], "opened_by": 2}),
            ),
        )
        .expect("a split naming a live pane is honoured");
        assert_eq!(pane_entry(&mut ext, 3)["opened_by"], json!(2));

        let before = tiled_order(&mut ext);
        assert!(matches!(
            ext.invoke(
                SPLIT_ACTION,
                IntrospectValue::Json(
                    json!({"pane": 0, "dir": "horizontal", "cmd": ["cat"], "opened_by": 99}),
                ),
            ),
            Err(InvokeError::Rejected(_)),
        ));
        assert_eq!(
            tiled_order(&mut ext),
            before,
            "the refused split divided nothing — the check runs before the birth, so there is no \
             half-placed pane to find afterwards",
        );
    }

    /// The arrangement the `layout` slot serves, in paint order — decoded through
    /// [`LayoutWire`](sprag_terminal::LayoutWire), the REAL consumer's type, so a test reads a
    /// placement the way a client does rather than by hand-walking the arena.
    fn tiled_order(ext: &mut WorkspaceExternal) -> Vec<u64> {
        let wire: sprag_terminal::LayoutWire =
            serde_json::from_value(answer_doc(ext.query(LAYOUT_SLOT))["tree"].clone())
                .expect("the layout slot serves a decodable arrangement");
        wire.panes().into_iter().map(|pane| pane.0).collect()
    }

    /// `move_pane` re-places a pane INSIDE its own window — the request herdr's two verbs leave a
    /// hole for (`pane.swap` refuses to cross a tab; `pane.move` refuses to stay in one), and which
    /// before this took a whole-tree `set_layout` write only a client with a tree could author.
    #[test]
    fn move_pane_re_places_a_pane_inside_its_own_window() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        three_pane_window(&mut ext); // 0 | (1 / 2)
        assert_eq!(tiled_order(&mut ext), vec![0, 1, 2]);
        let before = rev.current();

        let answer = ext
            .invoke(
                MOVE_PANE_ACTION,
                IntrospectValue::Json(
                    json!({"pane": 2, "target": 0, "dir": "horizontal", "before": true}),
                ),
            )
            .expect("a tiled target is placeable");

        assert_eq!(answer_doc(Some(answer))["closed_source"], false);
        assert_eq!(
            tiled_order(&mut ext),
            vec![2, 0, 1],
            "pane 2 landed LEFT of pane 0, which is what -b asked for",
        );
        assert!(
            rev.current() > before,
            "and the arrangement change woke the clients"
        );
    }

    /// The same verb crossing a window, with the destination never named — it is DERIVED from the
    /// target's id. `join_pane` can only append into a window; this states the place.
    #[test]
    fn move_pane_crosses_a_window_without_naming_it() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // window "0": 0 | (1 / 2)
        ext.invoke(
            NEW_WINDOW_ACTION,
            IntrospectValue::Json(json!({"name": "1"})),
        )
        .unwrap();
        // A scope PINS the window it resolved against, so reading the new window needs a new
        // surface — which is what production does too: one scope per request.
        let name = lock(&reg).default_session().name().to_owned();
        let (mut ext, _rev) = scoped_control(&reg, scope_of(&reg, &name));
        let arrived = tiled_order(&mut ext);
        assert_eq!(arrived.len(), 1, "a new window is born with one pane");
        let host_pane = arrived[0];

        let answer = ext
            .invoke(
                MOVE_PANE_ACTION,
                IntrospectValue::Json(
                    json!({"pane": 1, "target": host_pane, "dir": "vertical", "before": false}),
                ),
            )
            .expect("a pane in another window is a legal target");

        assert_eq!(
            answer_doc(Some(answer))["closed_source"],
            false,
            "window 0 kept two panes, so it was not closed",
        );
        assert_eq!(
            tiled_order(&mut ext),
            vec![host_pane, 1],
            "pane 1 arrived BELOW the window's own pane",
        );
    }

    /// `move_pane` refuses without moving anything: a pane beside itself, an unknown pane, and a
    /// target that is not tiled. A missing axis is MALFORMED rather than a refusal — the caller
    /// has not said what it wants, which is a different mistake from asking for the impossible.
    #[test]
    fn move_pane_refuses_the_impossible_and_rejects_the_unsaid() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext);
        let before = tiled_order(&mut ext);

        // `true` is a REFUSAL — a well-formed request the workspace declined, carrying the
        // registry's own sentence; `false` is a MALFORMED one. The table pairs the two rather than
        // naming a value because a refusal is no longer a unit: it holds the reason the daemon
        // stated, which is not what this test is about (the CLI gate pins those).
        for (args, refusal) in [
            (json!({"pane": 1, "target": 1, "dir": "horizontal"}), true),
            (json!({"pane": 9, "target": 1, "dir": "horizontal"}), true),
            (json!({"pane": 1, "target": 9, "dir": "horizontal"}), true),
            (json!({"pane": 1, "target": 0}), false),
            (json!({"pane": 1, "target": 0, "dir": "sideways"}), false),
            (
                json!({"pane": 1, "target": 0, "dir": "horizontal", "before": "yes"}),
                false,
            ),
        ] {
            let error = ext
                .invoke(MOVE_PANE_ACTION, IntrospectValue::Json(args.clone()))
                .unwrap_err();
            assert_eq!(
                matches!(error, InvokeError::Rejected(_)),
                refusal,
                "{args} -> {error:?}",
            );
        }
        assert_eq!(
            tiled_order(&mut ext),
            before,
            "and none of them moved a pane"
        );
    }

    /// `swap_pane {with}` trades two panes' places and every divider survives — the whole reason it
    /// is not two placements.
    #[test]
    fn swap_pane_trades_two_places_and_keeps_the_dividers() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        three_pane_window(&mut ext);
        let shape = query_layout(&mut ext);
        let before = rev.current();

        let answer = answer_doc(Some(
            ext.invoke(
                SWAP_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "with": 2})),
            )
            .expect("both panes are tiled"),
        ));

        assert_eq!(
            answer,
            json!({"a": 0, "b": 2, "changed": true, "outcome": "swapped"}),
        );
        assert_eq!(
            tiled_order(&mut ext),
            vec![2, 1, 0],
            "the two traded places"
        );
        let after = query_layout(&mut ext);
        assert_eq!(
            after["nodes"].as_array().map(Vec::len),
            shape["nodes"].as_array().map(Vec::len),
            "the arena is the same size — nothing was retired and re-minted",
        );
        assert!(rev.current() > before);
    }

    /// A control surface whose session has a WINDOW — an attached client reporting `cols x rows`.
    ///
    /// Every other surface in this module is built without attachments, which is exactly right for
    /// an action whose answer does not depend on how big anything is. `resize_pane`'s does: a cell
    /// has no length until somebody has measured the window, so this is the harness that action
    /// needs and the reason it is not `control`.
    fn control_with_a_window(
        reg: &Arc<Mutex<SessionRegistry>>,
        cols: u16,
        rows: u16,
    ) -> WorkspaceExternal {
        let attachments = Arc::new(Mutex::new(crate::AttachmentRegistry::default()));
        {
            let mut a = lock(&attachments);
            let conn = pinion_rpc::ConnId::allocate();
            a.hello(conn, "gui".to_owned());
            let id = lock(reg).default_session().id();
            a.attach(conn, "0".to_owned(), id);
            a.size(conn, crate::ClientSize { cols, rows });
        }
        WorkspaceExternal::new(
            Arc::clone(reg),
            SessionScope::unscoped(reg),
            Arc::new(ChannelRegistry::default()),
            crate::DaemonShared {
                on_pane_exit: None,
                attachments: Some(attachments),
                attention: None,
                agents: None,
                samplers: sampler(),
            },
        )
    }

    /// How wide the PTY of `pane` is — the fact a resize exists to move, read where the program in
    /// the pane reads it rather than off the ratio this action writes.
    fn pane_cols(reg: &Arc<Mutex<SessionRegistry>>, pane: u64) -> u16 {
        lock(&pool(reg))
            .pane(PaneId(pane))
            .expect("the pane is alive")
            .pty()
            .dimensions()
            .0
    }

    /// **THE POINT OF THE VERB**, asserted where a program in the pane would feel it: five cells
    /// asked for, five cells answered, and the PTY five columns wider.
    ///
    /// Two panes side by side over an 80-column window divide 79 usable columns (one is the
    /// divider) at an even share, so the left pane opens at `floor(79 * 0.5)` = 39. Moving the
    /// boundary five cells right puts it at 44, and the assertion is on the PTY rather than on the
    /// ratio because a share that moved without reflowing anything is the defect this whole front
    /// keeps finding.
    #[test]
    fn resize_pane_moves_the_boundary_and_says_how_far() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        assert_eq!(pane_cols(&reg, 0), 39, "an even share of 79 usable columns");

        let answer = write_doc(ext.invoke(
            RESIZE_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 5})),
        ));

        assert_eq!(answer, json!({"pane": 0, "cells": 5, "outcome": "resized"}),);
        assert_eq!(
            pane_cols(&reg, 0),
            44,
            "the pane the boundary moved away from"
        );
        assert_eq!(pane_cols(&reg, 1), 35, "and the one it moved into");
    }

    /// **The direction moves the BOUNDARY, not the pane** — the rule that makes this verb one
    /// sentence instead of a table, and the one a test written from the other end would get
    /// backwards. `left` from the RIGHT pane grows it, because the boundary it is measured against
    /// is the one on its near side.
    #[test]
    fn a_direction_moves_the_boundary_whichever_pane_asked() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        let (left, right) = (pane_cols(&reg, 0), pane_cols(&reg, 1));

        write_doc(ext.invoke(
            RESIZE_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 1, "dir": "left", "cells": 4})),
        ));

        assert_eq!(pane_cols(&reg, 1), right + 4, "the asker GREW");
        assert_eq!(pane_cols(&reg, 0), left - 4, "at its neighbour's expense");
        assert_ne!(
            left, right,
            "and the fixture's two panes are not the same width, so 'grew' and 'shrank' can be told apart"
        );
    }

    /// A boundary that runs into the last cell a side may keep answers how far it ACTUALLY went,
    /// and asking again from there is [`ResizeHow::AtMinimum`] — a fact with its own remedy, where
    /// the rival answers the same `bool` it answers for an edge, a float and a zoom.
    #[test]
    fn a_clamped_resize_says_how_far_it_got_and_then_says_it_cannot() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        let answer = write_doc(ext.invoke(
            RESIZE_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 500})),
        ));
        assert_eq!(
            answer,
            json!({"pane": 0, "cells": 39, "outcome": "resized"}),
            "39 of the 500 asked for — the boundary stopped one cell short of the far wall",
        );
        assert_eq!(pane_cols(&reg, 1), 1, "the far pane keeps its cell");

        assert_eq!(
            write_doc(ext.invoke(
                RESIZE_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 1})),
            )),
            json!({"pane": 0, "cells": 0, "outcome": "at_minimum"}),
            "and from there it says WHICH nothing happened",
        );
    }

    /// A pane that spans the window on the axis asked for has no boundary to move — a DIFFERENT
    /// fact from one whose boundary is at its limit, with a different remedy (split first), so it
    /// is a different word.
    #[test]
    fn a_resize_with_no_boundary_that_way_is_an_edge() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        assert_eq!(
            write_doc(ext.invoke(
                RESIZE_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "dir": "down", "cells": 3})),
            )),
            json!({"pane": 0, "cells": 0, "outcome": "at_edge"}),
            "two panes SIDE BY SIDE have no horizontal boundary between them",
        );
        assert_eq!(pane_cols(&reg, 0), 39, "and nothing moved");
    }

    /// A floating pane has no leaf in the arrangement, so it has no boundaries in any direction —
    /// `SwapHow::Untiled`'s fact one verb over, and named rather than collapsed into the edge.
    #[test]
    fn a_resize_of_a_floating_pane_is_untiled() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({"id": 0, "floating": true})),
        )
        .unwrap();

        assert_eq!(
            write_doc(ext.invoke(
                RESIZE_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 3})),
            )),
            json!({"pane": 0, "cells": 0, "outcome": "untiled"}),
        );
    }

    /// A zoomed window's arrangement is not what is on screen, so the boundary is NOT moved and the
    /// answer says why.
    ///
    /// Acting invisibly is the one outcome worse than doing nothing: R285 made the zoom a
    /// PROJECTION precisely so the arrangement is untouched by it, and a resize that quietly
    /// re-weighted a split the user cannot see would hand them a different layout on unzooming
    /// than the one they left.
    #[test]
    fn a_resize_under_a_zoom_moves_nothing_and_says_which_nothing() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        ext.invoke(
            ZOOM_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0, "on": true})),
        )
        .unwrap();

        assert_eq!(
            write_doc(ext.invoke(
                RESIZE_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 5})),
            )),
            json!({"pane": 0, "cells": 0, "outcome": "zoomed"}),
        );
        ext.invoke(
            ZOOM_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0, "on": false})),
        )
        .unwrap();
        assert_eq!(
            pane_cols(&reg, 0),
            39,
            "and the arrangement came back exactly as it was left",
        );
    }

    /// A window NOBODY HAS MEASURED refuses the request, because a cell has no length in it — the
    /// same fact `resize-window` already refuses on, and a refusal rather than an outcome because
    /// it is a state of the daemon rather than a shape of the layout.
    #[test]
    fn a_resize_needs_a_window_somebody_has_measured() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg); // no attachments: nothing has reported an area
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }

        assert!(
            matches!(
                ext.invoke(
                    RESIZE_PANE_ACTION,
                    IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 5})),
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(
                RESIZE_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "dir": "right", "cells": 5})),
            ),
        );
    }

    /// The grammar refuses what it cannot mean, as a TYPE — no direction, an unknown direction, and
    /// a zero distance, which is a caller spelling a move that has no reading rather than one
    /// leaving the amount out.
    #[test]
    fn a_resize_request_must_name_a_direction_and_a_real_distance() {
        let reg = registry();
        let mut ext = control_with_a_window(&reg, 80, 24);
        for _ in 0..2 {
            ext.invoke(SPAWN_ACTION, IntrospectValue::Json(json!({"cmd": ["cat"]})))
                .unwrap();
        }
        for malformed in [
            json!({"pane": 0}),
            json!({"pane": 0, "dir": "sideways"}),
            json!({"pane": 0, "dir": "right", "cells": 0}),
            json!({"pane": 0, "dir": "right", "cells": -2}),
        ] {
            assert_eq!(
                ext.invoke(RESIZE_PANE_ACTION, IntrospectValue::Json(malformed.clone())),
                Err(InvokeError::TypeMismatch),
                "{malformed} is not a request this action has a reading for",
            );
        }
        assert_eq!(
            write_doc(ext.invoke(
                RESIZE_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0, "dir": "right"})),
            )),
            json!({"pane": 0, "cells": 1, "outcome": "resized"}),
            "an ABSENT distance is tmux's default of one cell, which is what a bare key means",
        );
    }

    /// A DIRECTION resolves through the same adjacency `select_pane -L` uses, and at the EDGE the
    /// answer is "nothing to trade with" rather than a refusal — a key bound to `swap-pane -L`
    /// pressed at the left edge is well-formed, and refusing it would log a failure every time a
    /// user reaches the side of their layout.
    #[test]
    fn a_swap_toward_an_edge_is_answered_not_refused() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // 0 | (1 / 2), so pane 0 is at the left edge
        let before = tiled_order(&mut ext);

        assert_eq!(
            answer_doc(Some(
                ext.invoke(
                    SWAP_PANE_ACTION,
                    IntrospectValue::Json(json!({"pane": 0, "dir": "left"}))
                )
                .expect("an edge is not an error"),
            )),
            json!({"a": 0, "b": Value::Null, "changed": false, "outcome": "at_edge"}),
            "the EDGE, named — and not the same bytes a floating origin answers",
        );
        assert_eq!(tiled_order(&mut ext), before, "and nothing moved");

        assert_eq!(
            answer_doc(Some(
                ext.invoke(
                    SWAP_PANE_ACTION,
                    IntrospectValue::Json(json!({"pane": 0, "dir": "right"}))
                )
                .expect("pane 1 is to the right"),
            )),
            json!({"a": 0, "b": 1, "changed": true, "outcome": "swapped"}),
            "and the direction resolved to the pane the arrangement says is adjacent",
        );
    }

    /// The `layout` slot serves BOTH facts, and that is what makes the zoom readable in one wire
    /// read. A boolean here would have to be joined against the active pane from another slot
    /// fetched at another instant, and a client that woke between the two writes would fill its
    /// window with the wrong pane.
    ///
    /// The arrangement stays whole underneath, deliberately: a caller that draws nothing still
    /// reads where every pane is while one of them is filling the window.
    #[test]
    fn the_layout_slot_names_the_zoomed_pane_beside_the_arrangement_it_filters() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        three_pane_window(&mut ext);
        let arrangement = query_layout(&mut ext)["tree"].clone();
        let before = rev.current();

        assert_eq!(
            answer_doc(Some(
                ext.invoke(ZOOM_PANE_ACTION, IntrospectValue::Json(json!({"pane": 1})))
                    .expect("pane 1 is tiled"),
            )),
            json!({"pane": 1, "zoomed": true, "changed": true}),
        );

        let zoomed = query_layout(&mut ext);
        assert_eq!(zoomed["zoomed"], json!(1), "the wire names the PANE");
        assert_eq!(
            zoomed["tree"], arrangement,
            "and the arrangement underneath is untouched, so a caller that draws nothing still \
             reads where every pane is",
        );
        assert_eq!(
            tiled_order(&mut ext),
            vec![0, 1, 2],
            "all three panes are still tiled — a zoom is a projection, not an edit",
        );
        assert!(
            rev.current() > before,
            "a zoom changes what every client must draw, so it moves the revision the float set \
             already moves for the weaker version of that reason",
        );

        // And the key that opened it closes it: no `on` toggles the target's own state.
        assert_eq!(
            answer_doc(Some(
                ext.invoke(ZOOM_PANE_ACTION, IntrospectValue::Json(json!({"pane": 1})))
                    .expect("pane 1 is tiled"),
            )),
            json!({"pane": 1, "zoomed": false, "changed": true}),
        );
        assert_eq!(
            query_layout(&mut ext)["zoomed"],
            Value::Null,
            "an unzoomed window carries no zoom key at all",
        );
    }

    /// The zoom's invariant reaches the OTHER verbs without any of them mentioning it. A split
    /// selects its new pane, and selecting a pane that is not the zoom target ends the zoom — so
    /// "split while zoomed" is answered by the focus rule rather than by a line inside `split`.
    ///
    /// herdr needs the opposite: `self.zoomed = false` written by hand in the split path, the close
    /// path, the move-out path and the insert path (`src/workspace/tab.rs:414` `:483` `:505` `:527`
    /// at `9a4ce5e1`).
    ///
    /// Revert-proof: `split` gained no zoom code to remove, which is the claim — take `heal_zoom`
    /// out of `Window::set_active` instead and this reads `1`, a pane nothing is drawing while
    /// every keystroke goes to the new one.
    #[test]
    fn a_split_ends_a_zoom_because_a_split_selects_and_no_other_reason() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext);
        ext.invoke(ZOOM_PANE_ACTION, IntrospectValue::Json(json!({"pane": 1})))
            .expect("pane 1 is tiled");
        assert_eq!(query_layout(&mut ext)["zoomed"], json!(1));

        ext.invoke(
            SPLIT_ACTION,
            IntrospectValue::Json(json!({"pane": 1, "dir": "horizontal", "cmd": ["cat"]})),
        )
        .expect("the target is tiled");

        assert_eq!(
            query_layout(&mut ext)["zoomed"],
            Value::Null,
            "the new pane is the one the user is on, so the zoom is over",
        );
        assert_eq!(
            active_row(&mut ext),
            Some(3),
            "and they are on it, which is the whole reason the zoom ended",
        );
    }

    /// `pane` absent means the ACTIVE pane, `split`'s default. A pane id naming nothing is refused
    /// — `select_pane`'s rule for a typo — and a non-bool `on` is malformed rather than defaulted,
    /// the rule every other optional flag on this external follows.
    #[test]
    fn zoom_pane_defaults_to_here_and_refuses_a_typo() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // a split leaves the session on pane 2

        assert_eq!(
            answer_doc(Some(
                ext.invoke(ZOOM_PANE_ACTION, IntrospectValue::Null)
                    .expect("the window has an active pane"),
            )),
            json!({"pane": 2, "zoomed": true, "changed": true}),
            "no arguments at all zooms where the session is",
        );

        let refused = ext
            .invoke(
                ZOOM_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 99, "on": true})),
            )
            .unwrap_err();
        assert!(
            matches!(refused, InvokeError::Rejected(_)),
            "a pane the session does not hold is a refusal, not {refused:?}",
        );
        assert_eq!(
            ext.invoke(
                ZOOM_PANE_ACTION,
                IntrospectValue::Json(json!({"on": "yes"})),
            )
            .unwrap_err(),
            InvokeError::TypeMismatch,
        );
        assert_eq!(
            query_layout(&mut ext)["zoomed"],
            json!(2),
            "and neither refusal disturbed the zoom already in force",
        );
    }

    /// The naming shape is `select_pane`'s: exactly one of `with` / `dir`, because neither "swap
    /// with nothing" nor "swap with two things" has an obvious reading. A pane swapped with ITSELF
    /// is legal and moves nothing; a pane id naming nothing is refused.
    #[test]
    fn swap_pane_takes_exactly_one_partner() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext);

        for args in [
            json!({"pane": 0}),
            json!({"pane": 0, "with": 1, "dir": "right"}),
            json!({"pane": 0, "dir": "sideways"}),
        ] {
            assert_eq!(
                ext.invoke(SWAP_PANE_ACTION, IntrospectValue::Json(args.clone()))
                    .unwrap_err(),
                InvokeError::TypeMismatch,
                "{args}",
            );
        }
        assert_eq!(
            answer_doc(Some(
                ext.invoke(
                    SWAP_PANE_ACTION,
                    IntrospectValue::Json(json!({"pane": 1, "with": 1}))
                )
                .expect("a pane swapped with itself is legal"),
            )),
            json!({"a": 1, "b": 1, "changed": false, "outcome": "same_pane"}),
        );
        let refused = ext
            .invoke(
                SWAP_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 1, "with": 9})),
            )
            .unwrap_err();
        assert!(
            matches!(refused, InvokeError::Rejected(_)),
            "a pane id naming nothing is a caller's mistake, not an edge: {refused:?}",
        );
        let refused = ext
            .invoke(
                SWAP_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 9, "dir": "left"})),
            )
            .unwrap_err();
        assert!(
            matches!(refused, InvokeError::Rejected(_)),
            "and so is one in the DIRECTION arm — which answered a null partner and changed: false \
             until R301, a success sentence about a pane that does not exist: {refused:?}",
        );
    }

    /// The three ways a directional swap can go nowhere are THREE ANSWERS, and until R301 they were
    /// one — measured at `a7375f4` against a live daemon, an edge and a FLOATING origin answered the
    /// same bytes (`{"a":N,"b":null,"changed":false}`) and an id naming nothing answered them too.
    ///
    /// The remedies differ, which is what makes the collapse a defect rather than a terseness: an
    /// edge means "look the other way", a float means "there is no way to look", and an unheld id
    /// means the caller is wrong about what exists.
    #[test]
    fn a_swap_that_trades_nothing_says_which_nothing_it_was() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        three_pane_window(&mut ext); // 0 | (1 / 2)
        let before = rev.current();

        // FLOATED: the origin is a pane of this session with no leaf in the arrangement, so there
        // is no adjacency to walk in ANY direction — not an edge in one.
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({"id": 2, "floating": true})),
        )
        .expect("a pane can be floated out of the tiling");
        for dir in ["left", "right", "up", "down"] {
            assert_eq!(
                answer_doc(Some(
                    ext.invoke(
                        SWAP_PANE_ACTION,
                        IntrospectValue::Json(json!({"pane": 2, "dir": dir}))
                    )
                    .expect("a floating origin is answered, not refused"),
                )),
                json!({"a": 2, "b": Value::Null, "changed": false, "outcome": "untiled"}),
                "{dir}",
            );
        }
        assert_eq!(
            rev.current(),
            before + 1,
            "and none of the four woke a parked client — the float itself is the only bump",
        );
    }

    /// `select_pane {pane}` moves the daemon's active pane, and the `panes` slot says so — the two
    /// halves of the fact herdr spends `pane.focus` and `pane.current` on.
    #[test]
    fn select_pane_moves_the_active_pane_and_the_pane_list_reports_it() {
        let reg = registry();
        let (mut ext, rev) = control(&reg);
        three_pane_window(&mut ext);
        assert_eq!(
            active_row(&mut ext),
            Some(2),
            "a split leaves the session on the pane it just opened — tmux's rule, applied in the \
             daemon so a caller that draws nothing gets it too",
        );
        let before = rev.current();

        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 0, "changed": true, "outcome": "moved"})
            )),
        );

        assert_eq!(active_row(&mut ext), Some(0));
        assert!(
            rev.current() > before,
            "a select wakes the session's clients"
        );

        // Re-selecting the pane the window is already on is accepted and changes nothing — a
        // caller re-asserting where it is must not be an error, and must not wake anybody.
        let settled = rev.current();
        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 0}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 0, "changed": false, "outcome": "already_active"})
            )),
            "and it says WHICH kind of nothing happened: a re-select, not an edge",
        );
        assert_eq!(rev.current(), settled, "an unchanged select wakes nobody");
    }

    /// `select_pane {dir}` walks the ARRANGEMENT — the tmux `-L/-R/-U/-D` half. In `0 | (1 over 2)`
    /// the pane to the right of 0 is whichever of 1 and 2 covers more of it, and from 1 the way
    /// back is left. The last assertion is the one that separates this from an index walk: DOWN
    /// from 1 is 2, and there is no "down" in a pane list.
    #[test]
    fn select_pane_toward_a_direction_walks_the_arrangement() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // the split leaves the session on pane 2, the bottom right

        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"dir": "up"}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 1, "changed": true, "outcome": "moved"})
            )),
            "UP from the bottom of the right column is the pane above it — and there is no 'up' \
             in a pane list, which is what separates this from an index walk",
        );
        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"dir": "left"}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 0, "changed": true, "outcome": "moved"})
            )),
        );
        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"dir": "right"}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 1, "changed": true, "outcome": "moved"})
            )),
            "and back to whichever of the right column's panes covers most of pane 0",
        );
    }

    /// Reaching the EDGE is not a failure. A key bound to `select-pane -L` pressed on the leftmost
    /// pane is a well-formed request whose honest answer is "nothing to move to" — refusing it
    /// would log an error every time a user walked into the side of their own layout.
    #[test]
    fn a_direction_with_no_neighbour_changes_nothing_and_is_not_an_error() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext);
        ext.invoke(
            SELECT_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 0})),
        )
        .expect("pane 0 is there to select");

        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"dir": "left"}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 0, "changed": false, "outcome": "at_edge"})
            )),
            "and the word is at_edge, which is what makes it sayable as 'nothing to the left of 0' \
             rather than as 'already on 0' — an answer to a question the caller did not ask",
        );
        assert_eq!(active_row(&mut ext), Some(0), "and nothing moved");
    }

    /// The fourth outcome, and the one NO caller can derive: a direction asked from a pane the
    /// arrangement holds no leaf for. An edge and a floating pane both leave the window where it
    /// was, so `changed: false` cannot tell them apart — and they have opposite remedies.
    ///
    /// The rival reports one word for both (`PaneFocusDirectionReason::NoNeighbor`, herdr
    /// `9a4ce5e1`): `directional_pane_target` looks its source pane up among the rects it last drew
    /// and answers `None` when it is absent, exactly as at an edge.
    #[test]
    fn a_direction_from_a_floating_pane_says_it_is_in_no_arrangement() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // the split leaves the session on pane 2

        // THE CONTROL, taken first: while pane 2 is tiled, the same request moves.
        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"dir": "up"}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 1, "changed": true, "outcome": "moved"})
            )),
        );
        ext.invoke(
            SELECT_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 2})),
        )
        .expect("back onto the pane about to be floated");
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({"id": 2, "floating": true})),
        )
        .expect("float the active pane");

        for dir in ["up", "left", "right", "down"] {
            assert_eq!(
                ext.invoke(
                    SELECT_PANE_ACTION,
                    IntrospectValue::Json(json!({"dir": dir}))
                ),
                Ok(IntrospectValue::Json(
                    json!({"pane": 2, "changed": false, "outcome": "untiled"})
                )),
                "a floating pane has no neighbour in ANY direction, and that is not an edge",
            );
        }
        assert_eq!(
            active_row(&mut ext),
            Some(2),
            "and the user stays where they are",
        );
    }

    /// `select_pane {dir, from}` measures the step from the pane the CALLER names, not from where
    /// the user is — the question an agent asks about the panes around its own.
    ///
    /// Every case below is paired with the SAME direction asked without an origin, and each pair
    /// answers differently. That is the whole test: a fixture where the two agree cannot tell an
    /// origin that is read from one that is dropped, which is exactly what an old daemon does with
    /// it (R294) and why this argument costs a `WIRE_PROTOCOL` bump.
    #[test]
    fn select_pane_steps_from_the_pane_the_caller_names() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // 0 | (1 over 2), the session left on pane 2

        let ask = |ext: &mut WorkspaceExternal, args: Value| {
            ext.invoke(SELECT_PANE_ACTION, IntrospectValue::Json(args))
        };

        // RIGHT of 0 is 1. From the active pane (2, in the right column) the same word is an edge,
        // so the two arms cannot be confused for each other.
        assert_eq!(
            ask(&mut ext, json!({"dir": "right"})),
            Ok(IntrospectValue::Json(
                json!({"pane": 2, "changed": false, "outcome": "at_edge"})
            )),
            "the control: from where the user IS, right is the edge",
        );
        assert_eq!(
            ask(&mut ext, json!({"dir": "right", "from": 0})),
            Ok(IntrospectValue::Json(
                json!({"pane": 1, "changed": true, "outcome": "moved"})
            )),
            "and from pane 0 the same word crosses the split",
        );

        // UP from 1 is the top of the window. The user is on 1 now, so the unmoved answer names 1
        // either way — go back to 2 first, where the two panes differ.
        ask(&mut ext, json!({"pane": 2})).expect("pane 2 is there to select");
        assert_eq!(
            ask(&mut ext, json!({"dir": "up", "from": 1})),
            Ok(IntrospectValue::Json(
                json!({"pane": 2, "changed": false, "outcome": "at_edge"})
            )),
            "nothing above 1 — and the user stays on 2, the pane they were on. Answering 1 would \
             move them onto the origin because its edge was empty, which is a question nobody asked",
        );
        assert_eq!(
            ask(&mut ext, json!({"dir": "up"})),
            Ok(IntrospectValue::Json(
                json!({"pane": 1, "changed": true, "outcome": "moved"})
            )),
            "the control again: from 2 the same word moves",
        );

        // A step that lands back on the pane the session is already on — reachable ONLY with an
        // origin, and the one combination R299's four words could not produce.
        ask(&mut ext, json!({"pane": 2})).expect("pane 2 is there to select");
        assert_eq!(
            ask(&mut ext, json!({"dir": "down", "from": 1})),
            Ok(IntrospectValue::Json(
                json!({"pane": 2, "changed": false, "outcome": "already_active"})
            )),
            "down from 1 IS pane 2, and the user was already there: a re-select, not an edge",
        );
    }

    /// A named origin that is FLOATING answers `untiled` while the user's own pane is tiled — the
    /// two facts that word conflated for as long as the origin could only be the active pane.
    ///
    /// The rival spends one word (`NoNeighbor`) on this, on the edge, AND on an origin missing from
    /// the rectangles it last drew (herdr `9a4ce5e1`, `directional_pane_target`).
    #[test]
    fn a_named_origin_that_floats_is_untiled_while_the_user_is_not() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // the session is on pane 2, tiled

        // THE CONTROL, and it MOVES: while 1 is tiled, stepping onto it from 2 lands there.
        assert_eq!(
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"dir": "up", "from": 2}))
            ),
            Ok(IntrospectValue::Json(
                json!({"pane": 1, "changed": true, "outcome": "moved"})
            )),
        );
        ext.invoke(
            SELECT_PANE_ACTION,
            IntrospectValue::Json(json!({"pane": 2})),
        )
        .expect("back onto 2, so the user is on a TILED pane while the origin floats");
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({"id": 1, "floating": true})),
        )
        .expect("float a pane the user is NOT on");

        for dir in ["up", "left", "right", "down"] {
            assert_eq!(
                ext.invoke(
                    SELECT_PANE_ACTION,
                    IntrospectValue::Json(json!({"dir": dir, "from": 1}))
                ),
                Ok(IntrospectValue::Json(
                    json!({"pane": 2, "changed": false, "outcome": "untiled"})
                )),
                "the ORIGIN is in no arrangement, and the user — who is not — stays on 2",
            );
        }
        assert_eq!(active_row(&mut ext), Some(2));
    }

    /// An origin the current window does not hold is REFUSED, not answered `untiled`.
    ///
    /// The distinction is the point: a pane of another window is not "in no arrangement", it is in
    /// one this request cannot see, and `step` cannot tell them apart because a tree holds no leaf
    /// for either. A caller told the floating story would go looking for a float that is not there.
    #[test]
    fn an_origin_the_window_does_not_hold_is_refused_rather_than_called_floating() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext);

        assert!(
            matches!(
                ext.invoke(
                    SELECT_PANE_ACTION,
                    IntrospectValue::Json(json!({"dir": "left", "from": 99}))
                ),
                Err(InvokeError::Rejected(_))
            ),
            "the same answer the same pane id gets as a TARGET, one argument over",
        );
        assert_eq!(
            active_row(&mut ext),
            Some(2),
            "and the refusal left the window where it was",
        );
    }

    /// The argument shape: exactly ONE way of naming the target per request. Neither and both are
    /// refused as malformed rather than guessed at, and an unknown pane is a REJECTION — the
    /// well-formed-but-unhonorable answer `split` already gives its target.
    #[test]
    fn select_pane_takes_exactly_one_naming_and_refuses_a_pane_it_does_not_hold() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext);

        for args in [
            json!({}),
            json!({"pane": 1, "dir": "left"}),
            json!({"dir": "sideways"}),
            json!({"dir": 3}),
            json!({"pane": "1"}),
            // An ORIGIN with nothing to be the origin OF, and one of the wrong type. Both are the
            // same class of caller bug as the three above and get the same one answer.
            json!({"from": 1}),
            json!({"pane": 1, "from": 0}),
            json!({"dir": "left", "from": "1"}),
        ] {
            assert_eq!(
                ext.invoke(SELECT_PANE_ACTION, IntrospectValue::Json(args.clone())),
                Err(InvokeError::TypeMismatch),
                "malformed: {args}",
            );
        }
        assert!(
            matches!(
                ext.invoke(
                    SELECT_PANE_ACTION,
                    IntrospectValue::Json(json!({"pane": 99}))
                ),
                Err(InvokeError::Rejected(_))
            ),
            "a refusal, not {:?}",
            ext.invoke(
                SELECT_PANE_ACTION,
                IntrospectValue::Json(json!({"pane": 99}))
            ),
        );
        assert_eq!(
            active_row(&mut ext),
            Some(2),
            "every refusal left the window where it was",
        );
    }

    /// `neighbors.<pane>` answers all four directions at once, and `null` IS the edge — the shape
    /// that makes herdr's `pane.neighbor` and `pane.edges` one derivation instead of two that
    /// nothing keeps in agreement.
    #[test]
    fn the_neighbours_slot_answers_four_directions_with_null_for_an_edge() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // 0 | (1 over 2)

        assert_eq!(
            answer_doc(ext.query("neighbors.1")),
            json!({"left": 0, "right": null, "up": null, "down": 2}),
        );
        assert_eq!(
            answer_doc(ext.query("neighbors.0")),
            json!({"left": null, "right": 1, "up": null, "down": null}),
            "the leftmost pane spans the height, so it has no vertical neighbour either",
        );
        // A pane the tiling does not hold answers all four `null` — it is not at an edge, it is
        // not in the arrangement. Same answer for a pane that never existed and for a FLOATED one.
        assert_eq!(
            answer_doc(ext.query("neighbors.99")),
            json!({"left": null, "right": null, "up": null, "down": null}),
        );
        ext.invoke(
            SET_FLOATING_ACTION,
            IntrospectValue::Json(json!({"id": 2, "floating": true})),
        )
        .unwrap();
        assert_eq!(
            answer_doc(ext.query("neighbors.2")),
            json!({"left": null, "right": null, "up": null, "down": null}),
            "a floated pane is out of the tiling adjacency is a property of",
        );
        assert_eq!(
            answer_doc(ext.query("neighbors.1")),
            json!({"left": 0, "right": null, "up": null, "down": null}),
            "THE CONTROL: its old neighbour lost it too, so this is the live tiling and not a \
             cached one",
        );
        // A malformed member of a family the schema ADVERTISES is present-but-empty, never an
        // unknown path (the taxonomy `events.<since>` states).
        assert_eq!(ext.query("neighbors.zzz"), Some(IntrospectValue::Null));
    }

    /// The sentence `SPLIT_ACTION`'s docs used to carry — *"`pane` is REQUIRED, because the daemon
    /// has no active-pane concept to mean here"* — retired. A person's command defaults to where
    /// the person is; a `close` with no target closes the same pane.
    #[test]
    fn a_pane_action_with_no_target_acts_on_the_active_pane() {
        let reg = registry();
        let (mut ext, _rev) = control(&reg);
        three_pane_window(&mut ext); // which leaves the session on pane 2

        // A split with no `pane` divides the ACTIVE one: the new pane 3 lands beside pane 2, which
        // `neighbors` reports rather than a pane count could.
        ext.invoke(
            SPLIT_ACTION,
            IntrospectValue::Json(json!({"dir": "horizontal", "cmd": ["cat"]})),
        )
        .unwrap();
        assert_eq!(
            answer_doc(ext.query("neighbors.3"))["left"],
            json!(2),
            "the new pane opened beside the pane the user was on",
        );
        assert_eq!(
            active_row(&mut ext),
            Some(3),
            "and the session moved onto it — tmux's split-window rule, which is why the next \
             targetless verb acts on the NEW pane",
        );

        // A close with no target closes the active pane, and the window hands off to its neighbour.
        ext.invoke(CLOSE_ACTION, IntrospectValue::Json(json!({})))
            .unwrap();
        assert!(
            lock(&pool(&reg)).pane(PaneId(3)).is_none(),
            "THE CONTROL: the close really took the active pane, not some default one",
        );
        assert_eq!(
            active_row(&mut ext),
            Some(2),
            "and the window is back on the neighbour that inherited",
        );
    }
}
