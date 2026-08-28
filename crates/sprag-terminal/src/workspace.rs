//! The workspace — sprag's pane registry (the multiplexer's producer pool).
//!
//! README core scope ("멀티플렉싱: ... pane 생명주기"): the multiplexer
//! manages a set of live [`PanePty`] panes. This is a producer-layer
//! concern — owning PTYs and their lifecycle — so it stays pinion-free here;
//! the pinion scene/control surface lives one layer up in sprag-host (the
//! `WorkspaceExternal`).
//!
//! Headless multiplexing is pane *control*, not visual tiling: each pane is
//! an independently-sized terminal addressed by [`PaneId`]. This pool holds no
//! arrangement at all — it is the membership authority (which panes exist), and
//! nothing more.
//!
//! ## Round 7's "no split tree here" note, superseded in part
//!
//! That note said a split tree "only has meaning relative to a display surface
//! to divide, so it is a rendering concern". True of PIXEL geometry (what rect a
//! pane occupies at one client's size) — that stays in the display client. But it
//! conflated pixels with the LOGICAL arrangement (which panes are split, in what
//! order, at what proportion), which is session state: tmux keeps it server-side
//! so a client can detach and reattach — at a different size, from a different
//! machine — and get its layout back. The detach/reattach arc therefore moved the
//! logical arrangement host-side into [`Window`](crate::Window)'s
//! [`LayoutTree`](crate::LayoutTree) (still pinion-free, still rect-free); pixels
//! remain the client's. It is deliberately NOT in this pool: membership and
//! arrangement are separate authorities, and the arrangement reconciles against
//! this one.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use sprag_vt::{ClipboardQuery, ClipboardWrite, Image, MouseProtocol, Notification, ShellState};

use crate::pane_pty::{
    Attention, CommandBuilder, PaneExit, PaneHooks, PanePty, PanePtyError, PanePtyHandle,
};
use crate::remote::SshRemote;
use crate::share::{Landing, PaneHomes, PaneLineage, PoolLineage};

/// A stable, monotonic identifier for a pane within a [`Workspace`].
///
/// Ids are never reused, so a stale reference fails closed (the pane is
/// simply absent) rather than aliasing a pane that took its place. Unique
/// across a whole [`SessionRegistry`](crate::SessionRegistry) (every window's
/// pool draws from one counter), so a pane is addressable by id alone —
/// independent of which window holds it.
///
/// Serialises as its bare number, matching the `id` the pane-list wire has
/// always carried; it is the identity a [`LayoutTree`](crate::LayoutTree) leaf
/// names over the wire.
/// `Ord` is by mint order (the counter is monotonic and never reused), which is what lets a
/// set of ids be serialised in a STABLE order — a wire list whose order wobbled would read
/// as a change to a client watching for one.
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct PaneId(pub u64);

impl fmt::Display for PaneId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// One managed pane: a live [`PanePty`] plus its id, a human/AI-readable command label
/// (surfaced via introspection), and the full argv it was launched with (for an exact-command
/// restore).
pub struct Pane {
    id: PaneId,
    pty: PanePty,
    command_label: String,
    /// The full launch command (`[program, args…]`), captured from the [`CommandBuilder`] at
    /// spawn. What an exact-command restore re-runs for an allowlisted program (else it falls
    /// back to a shell). Distinct from [`command_label`](Self::command_label), which is just the
    /// program name for display. A live pane always has one (every spawn captures it); a pane
    /// restored from a pre-argv snapshot re-runs a shell, so it comes back with the shell's argv,
    /// never empty.
    argv: Vec<String>,
    /// **THE ENVIRONMENT ENTRIES ITS LAUNCHER SET**, captured from the [`CommandBuilder`] at the same
    /// moment as [`argv`](Self::argv) and for the same reason: so a REPLACEMENT can be the same
    /// command, and not merely the same program.
    ///
    /// # ⚠⚠⚠ Why the daemon's own additions are deliberately NOT in here
    ///
    /// Captured BEFORE `instrument` and before `pane_env`, exactly as the argv is. What those two add
    /// — this daemon's endpoint flag, this pane's own id — are facts about the process doing the
    /// spawning and about the pane being born, so a replacement RE-DERIVES them rather than inheriting
    /// the previous pane's. Copying them would give a fresh pane the closed pane's id.
    ///
    /// # ⚠⚠ What it does not carry, stated
    ///
    /// The INHERITED environment. A child gets the daemon's own variables plus these, and a
    /// replacement spawned by the same daemon inherits the same ones — so the only part that has to be
    /// recorded is the part somebody chose. ⚠ A pane restored from a snapshot has none of these: the
    /// snapshot records argv and cwd, so a restored pane's replacement is the program without its
    /// launcher's variables. Its own limitation, said out loud.
    env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    /// **WHERE THIS PANE'S CHILD WAS POINTED**, captured from the [`CommandBuilder`] at the same
    /// moment as [`argv`](Self::argv) and [`env`](Self::env), and for the third half of the same
    /// reason: so a REPLACEMENT can be the same command, with the same variables, **in the same
    /// place**.
    ///
    /// It is `CommandBuilder::start_dir`'s answer — a `pub(crate)` function, so this is a name and
    /// not a link, exactly as `pty.rs` and `access.rs` spell it — the directory the child got, after
    /// that function has applied its `$HOME` default and its does-it-still-exist check — so it is
    /// what the caller ASKED FOR, resolved, and never a guess about it.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is a field and not a live read (register item 684)
    ///
    /// `respawn` used to take this from [`PanePty::cwd`](crate::PanePty::cwd), which reads
    /// `/proc/<pid>/cwd` LIVE — and that reader says of itself that it answers `None` *"when the
    /// child has exited (no pid)"*. **A replacement is asked for at exactly the moment the previous
    /// child is gone**, so the read that was supposed to carry the pane's place answered nothing and
    /// the replacement fell through to `start_dir`'s default: `$HOME`.
    ///
    /// Measured 2026-08-25, in the owner's own daemon: every restarted `claude` pane sitting in
    /// `/home/coin` was `blocked  rule=dialog-choice-list` on *"Accessing workspace: /home/coin —
    /// Quick safety check"*, with `Resume this session with: claude --resume …` printed above it —
    /// the previous child's dying words, which is what made the live read `None` in the first place.
    /// A loop that replaces its inner session on purpose walks into this EVERY time.
    ///
    /// ⚠⚠ **The live read is not wrong, it is a different question.** *Where is that child now* is
    /// what a durability snapshot wants (a person's shell rewrites it on every `cd`, and putting them
    /// back where they were working is the point). *Where was this pane pointed* is what a
    /// replacement wants, and only one of the two survives the child it describes.
    ///
    /// ⚠⚠⚠ **So a SNAPSHOT records both, and the restore door reads them for opposite purposes** —
    /// [`PaneSnapshot::start_dir`](crate::PaneSnapshot::start_dir), item 684's second clause. It
    /// spawns the child in the reading and re-stamps THIS from the recorded intent, resolved by the
    /// same `dir_or_home` rule the fresh-spawn door uses. Deriving it from the restored command
    /// instead — which this door did until then — laundered the intent back into the reading, and a
    /// snapshot taken after the child had exited carries no reading at all.
    start_dir: std::path::PathBuf,
    /// **THE NAME OF THE CONVERSATION THIS PANE'S LAUNCH JOINED**, read out of the argv it actually
    /// exec'd by the pool's [`PaneIdentitySource`] — `None` for every pane that is not a named agent,
    /// which is nearly all of them.
    ///
    /// # ⚠⚠⚠ Why it is beside [`argv`](Self::argv) rather than in it
    ///
    /// The two answer opposite questions and are read by opposite doors:
    ///
    /// * [`argv`](Self::argv) is what a REPLACEMENT re-runs, and a replacement must be a FRESH
    ///   session — `ai_loop.scxml` replaces its inner session precisely to throw the accumulated
    ///   context away, so a name replayed there would defeat the state that exists to discard it.
    ///   `live_agent`'s respawn gate holds exactly that, and names this temptation in its own
    ///   message: *"the identity reached the pane's RECORDED argv and was replayed"*.
    /// * this is what a RESTORE resumes. A daemon restart is not a replacement — nobody asked for the
    ///   work to be thrown away, and the transcript is still on disk under this name.
    ///
    /// ⚠ It is therefore the ONE thing the daemon adds that a restore carries rather than re-derives.
    /// Everything else in [`PaneArgsSource`]'s output names THIS daemon (its endpoint, its hook
    /// binary) and would point a restored agent at a socket that is gone.
    agent_session: Option<String>,
    /// The structured remote endpoint, set ONLY for a pane born via `sprag ssh` (its explicit
    /// intent marker). `Some` marks a sanctioned remote workspace — the host reconnects it on
    /// restore (bypassing the argv allowlist) and can `scp` a dropped file to it; `None` is an
    /// ordinary local pane. Distinct from [`argv`](Self::argv), which merely happens to contain
    /// `ssh`: a shell with `ssh` in its history is not a remote workspace and is never reconnected.
    remote: Option<SshRemote>,
    /// The pane whose OCCUPANT asked for this one, `None` for a pane nobody claims — a person's
    /// split, a plain `sprag split-window`, the session's birth pane.
    ///
    /// A pane's PROVENANCE, and the only durable answer to "what did the agent in that pane open?".
    /// It is what lets an agent surface refuse to close somebody else's pane while still letting it
    /// clean up after itself across a context reset, a client restart or a reboot — a set the asking
    /// process kept in memory would be lost at exactly the moment it is needed.
    ///
    /// Safe as an id because pane ids are monotonic and NEVER reused (see
    /// [`spawn_restored`](Workspace::spawn_restored), which reserves a restored id against reuse):
    /// a provenance that outlives its opener names a pane that is gone, never a DIFFERENT pane that
    /// arrived later.
    ///
    /// It is deliberately not cleared when the opener closes, because "pane 3 asked for this" stays
    /// TRUE and is what a person reading the pane list wants to know. **What that costs is worth
    /// stating exactly, because the first version of this comment got it backwards**: clearing it
    /// would NOT strand the pane, because a stranded pane is what happens EITHER WAY. An id is never
    /// reissued, so once the opener is gone no live pane can ever match it — and a cleared `None`
    /// reads as "a person made this", which the agent surface refuses just as firmly. So an
    /// agent-opened pane whose opener has closed is closable by no agent under either rule, and only
    /// a person's `sprag kill-pane` removes it. Keeping the id at least says WHO to ask.
    opened_by: Option<PaneId>,
    /// **WHETHER A RESTORE GAVE THIS PANE ITS PROCESS** — `true` only for a pane
    /// [`spawn_restored`](Workspace::spawn_restored) brought back, `false` for every pane a person
    /// or a run opened.
    ///
    /// ⚠⚠⚠⚠⚠ **IT IS THE ONLY THING THAT SEPARATES A REVIVED AGENT FROM A WANTED ONE** — register
    /// item 595. A daemon restart re-runs an allowlisted argv, so a `claude` pane comes back holding
    /// its old conversation; asking *is a run driving it* cannot separate the two, because a restart
    /// brings panes back ALIVE and runs back ENDED, leaving every agent pane undriven. Measured
    /// 2026-08-22, three times in one day, the last with no running run beside three live `claude`
    /// processes.
    ///
    /// It is a fact of the BIRTH, so nothing later in the pane's life could recompute it — which is
    /// why it is a field rather than something the pane list derives. It is deliberately NOT
    /// snapshotted: a snapshot records what a pane WAS, and whether the pane in front of you was
    /// re-run is a fact of THIS daemon's boot, exactly as its environment and instrumentation are
    /// (`spawn_restored` re-derives both for that reason). A snapshot of a restored pane restored
    /// again is revived again, which is true.
    ///
    /// ⚠⚠ **AND IT MUST NOT FOLLOW A REPLACEMENT**, on [`agent_session`](Self::agent_session)'s
    /// terms: `respawn` runs the command afresh in THIS daemon because something asked it to, so the
    /// occupant of that seat is wanted even though the seat came back from a snapshot. Carrying it
    /// would make a loop's own restart look like an orphan forever.
    revived: bool,
    /// Which cgroup this pane's processes ARE in, as the three ids that spell it — or WHY there is
    /// no such cgroup, which is two different answers and not one.
    ///
    /// It is the ANSWER to the placement and not the address the placement would have used; see
    /// [`Workspace::landed_home`] for what that distinction buys and which half of it was missing.
    ///
    /// # Why the absence is a reason and not a `None`
    ///
    /// R341 made this the join rather than the descriptor, which was right and left every pane on a
    /// refusing host answering *never placed* — true of the outcome and wrong about the cause. A
    /// person reading it went looking for a fault in a daemon that had done everything correctly,
    /// and the kernel's own sentence, which names the real one, had already been read and thrown
    /// away four lines earlier. [`Landing`] is that sentence kept.
    ///
    /// # Why the pane holds this and the sweep holds nothing
    ///
    /// [`Tree::sweep`](crate::share::Tree::sweep) deliberately keeps no pane-to-cgroup table,
    /// because the kernel already answers "is this pane still running?" and a table that is wrong is
    /// invisible. This is a different question — *where is this pane's cgroup NOW* — and the kernel
    /// cannot answer it once the pane has moved: `/proc/<pid>/cgroup` says where the processes are,
    /// which is exactly the stale answer a move has to correct. So the pane carries its own address,
    /// written at the two births and re-written by the one [`adopt`](Workspace::adopt) that moves it.
    home: Landing,
    /// What a person said about THIS pane's resources in particular, `None` for a pane nobody has
    /// singled out — which is every pane until somebody says otherwise.
    ///
    /// # Why this is held when the kernel already holds the numbers
    ///
    /// It is not the same fact. The leaf holds the EFFECTIVE grant, which
    /// [`PaneHomes::granted`](crate::share::PaneHomes::granted) reads back and which is the honest
    /// answer to *what is in force*. What no file records is WHO SAID SO: a leaf capped at 512 MiB
    /// looks identical whether that number came from the machine's config or from a person typing it
    /// at this pane, and the two must behave differently the moment anything re-derives the grant.
    ///
    /// No accessor: the two readers of this are both in this module ([`Workspace::adopt`] and
    /// [`Workspace::pane_grant_or_default`]), and the question a caller outside actually has is
    /// *what is this pane following*, which is what that second one answers. A public getter
    /// handing back the raw `Option` would be a second door onto one fact, and the one that omits
    /// the fallback.
    ///
    /// [`Workspace::adopt`] is that moment. A pane that moves is re-placed, and re-placing has to
    /// decide which authority to ask: an overridden pane must carry its own number across (somebody
    /// capped this pane and then pulled it into its own window, which is when they most expect the
    /// cap to hold), and an ordinary one must re-ask the config (so a person who raised a ceiling
    /// there is not stuck with the old one). One bit of provenance is what separates them, and the
    /// kernel keeps no such bit.
    ///
    /// So: `None` means *follow the machine*, and it stays `None` for the whole life of almost every
    /// pane. This is the smallest thing that could be stored and it is stored beside
    /// [`home`](Self::home), which is the other fact about a pane that only the daemon knows.
    grant: Option<crate::share::Grant>,
    /// The name a PERSON gave this pane (or the agent that opened it), `None` for a pane nobody
    /// named — which is every pane until somebody says otherwise.
    ///
    /// Neither of the two name-shaped facts beside it: [`command_label`](Self::command_label) is
    /// what was launched and is chosen by nobody, and [`title`](Self::title) is the child's own
    /// `OSC 0`/`2`, rewritten on every prompt and chosen by the CHILD. This one changes only when
    /// somebody says so, which is what lets it be an ADDRESS — see [`crate::PaneName`] for the
    /// forms it refuses and why each refusal is load-bearing.
    ///
    /// **Uniqueness is not this type's to hold.** The pool is the membership authority for ONE
    /// window while a name is unique across the whole REGISTRY (it stands in for the id, which is
    /// registry-unique), so the check belongs to whoever accepts a name from a caller — exactly the
    /// split [`set_pane_opened_by`](Workspace::set_pane_opened_by) already states for a provenance.
    name: Option<crate::PaneName>,
}

impl Pane {
    /// The pane's stable id.
    #[must_use]
    pub fn id(&self) -> PaneId {
        self.id
    }

    /// The live pseudoterminal backing this pane.
    #[must_use]
    pub fn pty(&self) -> &PanePty {
        &self.pty
    }

    /// The label this pane was spawned with (typically the program name).
    #[must_use]
    pub fn command_label(&self) -> &str {
        &self.command_label
    }

    /// The full argv this pane was launched with (`[program, args…]`) — what an exact-command
    /// restore re-runs (for an allowlisted program) or falls back to a shell for. Captured at
    /// spawn from the [`CommandBuilder`]; empty only for a pane restored from a pre-argv snapshot.
    #[must_use]
    pub fn argv(&self) -> &[String] {
        &self.argv
    }

    /// The environment entries this pane's launcher set — what a REPLACEMENT has to set too. See the
    /// [field](Self::env) for what is in it and what is deliberately not.
    #[must_use]
    pub fn env(&self) -> &[(std::ffi::OsString, std::ffi::OsString)] {
        &self.env
    }

    /// The directory this pane's child was pointed at, captured at spawn — what a REPLACEMENT has to
    /// start in. See the [field](Self::start_dir) for why a replacement must not ask the OS instead.
    #[must_use]
    pub fn start_dir(&self) -> &std::path::Path {
        &self.start_dir
    }

    /// The pane's structured remote endpoint, `Some` only for a `sprag ssh` workspace pane — the
    /// marker the host reads to reconnect it on restore or to `scp` a dropped file to it.
    #[must_use]
    pub fn remote(&self) -> Option<&SshRemote> {
        self.remote.as_ref()
    }

    /// The pane whose occupant asked for this one — this pane's provenance, `None` for a pane
    /// nobody claims. See the [field](Self::opened_by) for what rests on it.
    #[must_use]
    pub fn opened_by(&self) -> Option<PaneId> {
        self.opened_by
    }

    /// Whether a RESTORE gave this pane its process — `true` only for a pane
    /// [`spawn_restored`](Workspace::spawn_restored) brought back out of a snapshot. See the
    /// [field](Self::revived) for what rests on it.
    #[must_use]
    pub fn revived(&self) -> bool {
        self.revived
    }

    /// The name of the conversation this pane's launch joined, `None` for a pane that is not a named
    /// agent. See the [field](Self::agent_session) for why a RESTORE reads this and a REPLACEMENT
    /// must not.
    #[must_use]
    pub fn agent_session(&self) -> Option<&str> {
        self.agent_session.as_deref()
    }

    /// Which cgroup this pane's processes ARE in, or why there is none. See the
    /// [field](Self::home) for why this is the placement's answer rather than its address — which
    /// is what makes it the right thing to MEASURE through, since a reading taken at an address the
    /// pane was never put at would report somebody else's numbers under this pane's id.
    #[must_use]
    pub fn home(&self) -> Landing {
        self.home
    }

    /// The name a person gave this pane, `None` for a pane nobody named. See the
    /// [field](Self::name) for how it differs from the two name-shaped facts beside it.
    #[must_use]
    pub fn name(&self) -> Option<&crate::PaneName> {
        self.name.as_ref()
    }

    /// The child's self-reported window title (`OSC 0` / `OSC 2`), `None` until it sets
    /// one. Read LIVE from the emulator — a shell rewrites it on every prompt — so it is
    /// NOT stored on the pane beside [`Self::command_label`] (which names what was
    /// launched and never changes). A display surface prefers this and falls back to a
    /// stable name; pane IDENTITY never derives from it, since a child sets it freely.
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.pty.title()
    }

    /// The most recent attention notification the child raised (`OSC 9` / `OSC 777;notify`
    /// / `OSC 99`), or `None`, with its monotonic sequence — read LIVE from the emulator like
    /// [`Self::title`]. A DISPLAY signal the multiplexer surfaces as "this pane wants
    /// attention"; a client detects a NEW one via the sequence growing (see
    /// [`sprag_vt::VtPort::notification`]).
    #[must_use]
    pub fn notification(&self) -> (Option<Notification>, u64) {
        self.pty.notification()
    }

    /// The monotonic count of BELLs (`\a`) the child has rung — the tmux `monitor-bell` attention
    /// signal, read LIVE from the emulator. Kept apart from [`Self::notification`] (a bell carries
    /// no text). See [`sprag_vt::VtPort::bell_seq`].
    #[must_use]
    pub fn bell_seq(&self) -> u64 {
        self.pty.bell_seq()
    }

    /// The inline images (Kitty graphics, R1404) the child has transmitted, each anchored at its
    /// transmit-time cursor cell — read LIVE from the emulator's screen. A display client
    /// composites each over the grid at its anchor cell × the cell metric. See
    /// [`sprag_vt::Screen::images`].
    #[must_use]
    pub fn images(&self) -> Vec<Image> {
        self.pty.with_screen(|s| s.images().to_vec())
    }

    /// The pane's shell-integration state (OSC 133) + last command exit status, read LIVE from the
    /// emulator's screen marks. Surfaced so a monitor / an AI sibling knows whether the shell is
    /// idle at a prompt or running a command, and how the last one exited.
    #[must_use]
    pub fn shell(&self) -> (ShellState, Option<i32>) {
        self.pty.shell()
    }

    /// Which pointer events the child has asked the terminal to report (the DECSET mouse-tracking
    /// mode), read LIVE from the emulator. A display client reads this to decide whether to capture
    /// the pointer for reporting. See [`sprag_vt::MouseProtocol`].
    #[must_use]
    pub fn mouse_protocol(&self) -> MouseProtocol {
        self.pty.mouse_protocol()
    }

    /// Whether the child has asked the terminal to report focus changes (DECSET 1004), read LIVE
    /// from the emulator. Surfaced so a display client emits a focus edge on a pane focus change,
    /// and an AI sibling learns whether the app reacts to focus at all. See
    /// [`sprag_vt::InputModes::focus_tracking`].
    #[must_use]
    pub fn focus_tracking(&self) -> bool {
        self.pty.focus_tracking()
    }

    /// The most recent OSC 52 clipboard WRITE the child requested, or `None`, with its monotonic
    /// sequence — read LIVE from the emulator. Potentially large (a paste), so it is fetched on
    /// demand off the sequence, not shipped every poll. See [`sprag_vt::VtPort::clipboard_write`].
    #[must_use]
    pub fn clipboard_write(&self) -> (Option<ClipboardWrite>, u64) {
        self.pty.clipboard_write()
    }

    /// The cheap monotonic count of OSC 52 clipboard writes — no payload clone. A display client
    /// polls this each frame and fetches [`Self::clipboard_write`] only when it grows.
    #[must_use]
    pub fn clipboard_write_seq(&self) -> u64 {
        self.pty.clipboard_write_seq()
    }

    /// The most recent OSC 52 clipboard READ query the child requested, or `None`, with its
    /// monotonic sequence — read LIVE from the emulator. See [`sprag_vt::VtPort::clipboard_query`].
    #[must_use]
    pub fn clipboard_query(&self) -> (Option<ClipboardQuery>, u64) {
        self.pty.clipboard_query()
    }

    /// A cloneable I/O handle onto this pane's pseudoterminal.
    #[must_use]
    pub fn handle(&self) -> PanePtyHandle {
        self.pty.handle()
    }
}

/// **WHAT A CALLER DECLARED ABOUT A PANE'S SEAT** — the position in the window — as distinct from
/// what its OCCUPANT is. Handed to a replacement by
/// [`Workspace::hand_seat_over`](Workspace::hand_seat_over), which is the only thing that builds one.
///
/// Private on purpose: this is not a fact anybody reads, it is the *set* the replacement path must
/// not be able to get wrong. A public getter would invite a second caller to take three of the four.
struct Seat {
    name: Option<crate::PaneName>,
    opened_by: Option<PaneId>,
    remote: Option<SshRemote>,
    grant: Option<crate::share::Grant>,
}

impl Seat {
    /// ⚠⚠⚠⚠⚠ **EVERY FIELD OF [`Pane`] IS NAMED BELOW AND THE PATTERN CARRIES NO `..`**, so a field
    /// added to a pane does not compile until somebody has said whether a replacement inherits it.
    /// That is the whole reason this walks a destructuring pattern instead of reading four getters:
    /// register item 478 was one forgotten field, and a list with no glob is a list that decides
    /// alone. **The compiler is the thing that asks.**
    ///
    /// The seven that are NOT the seat's, in the four groups they stay behind for:
    ///
    /// * `id` and `pty` ARE the new pane — carrying either would make this a rename, not a
    ///   replacement;
    /// * `command_label`, `argv` and `env` come from the spawn the replacement was launched with,
    ///   which read them off the outgoing pane already (`PaneLifecycle::respawn`), so taking them
    ///   here would be the same rule written twice;
    /// * `agent_session` is the OCCUPANT'S OWN IDENTITY and must never follow: a loop replaces its
    ///   inner session precisely to throw that session away, and an agent handed a name already in
    ///   use refuses itself at startup. See the [field](Pane::agent_session).
    /// * `home` is the cgroup the replacement was actually placed in — an answer about this pane,
    ///   not a request about the seat. The seat's REQUEST is `grant`, and that is carried.
    fn of(pane: &Pane) -> Self {
        let Pane {
            name,
            opened_by,
            remote,
            grant,
            id: _,
            pty: _,
            command_label: _,
            argv: _,
            env: _,
            // ⚠ Carried by the LAUNCH, not by the seat: `respawn` reads it off the old pane and
            // passes it to the spawn, so the replacement captures its own from its own builder —
            // exactly as it does for `argv` and `env`. Copying it here would be a second writer of
            // one fact.
            start_dir: _,
            agent_session: _,
            // ⚠ On `agent_session`'s terms exactly, stated above: the seat may have come back from a
            // snapshot, but the REPLACEMENT was asked for by whatever called `respawn`, so carrying
            // this would leave a loop's own restart reading as an orphan for the pane's whole life.
            revived: _,
            home: _,
        } = pane;
        Self {
            name: name.clone(),
            opened_by: *opened_by,
            remote: remote.clone(),
            grant: *grant,
        }
    }
}

/// Read-only metadata describing a pane, for introspection over the
/// scene-as-data control surface (the host maps this to JSON).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
    pub command_label: String,
    /// The name a PERSON gave this pane, `None` for a pane nobody named — [`Pane::name`],
    /// republished verbatim.
    ///
    /// The only one of this struct's three name-shaped fields that is IDENTITY: it is unique
    /// across the registry and a surface may resolve it to this pane. The other two are display
    /// (`command_label` is what was launched, `title` is what the child calls itself).
    pub name: Option<crate::PaneName>,
    /// The child's self-reported window title (`OSC 0` / `OSC 2`), `None` until it sets
    /// one. Live and child-controlled, so it is a DISPLAY name only — never identity.
    pub title: Option<String>,
    /// The most recent attention notification the child raised (`OSC 9` / `OSC 777;notify`
    /// / `OSC 99`), or `None`. A DISPLAY signal — the multiplexer's "this pane wants
    /// attention" — never identity, exactly like [`Self::title`].
    pub notification: Option<Notification>,
    /// Monotonic count of notifications this pane's child has raised (`0` before the first).
    /// A client that remembers the value it last saw learns a NEW notification arrived when
    /// this grows — the payload alone cannot distinguish a re-raise of the same text.
    pub notification_seq: u64,
    /// Monotonic count of BELLs (`\a`) this pane's child has rung (`0` before the first) — the
    /// tmux `monitor-bell` signal. Kept SEPARATE from [`Self::notification_seq`] (a bell is not a
    /// desktop toast — it carries no text) so the two attention sources stay individually
    /// addressable; a viewer's "unseen attention" combines both. See [`sprag_vt::VtPort::bell_seq`].
    pub bell_seq: u64,
    /// Whether the pane's CHILD has exited ([`PanePty::is_eof`](crate::PanePty::is_eof)).
    ///
    /// A dead pane is not removed — nothing reaps one, so it keeps its place and its final screen
    /// (tmux's `remain-on-exit`, except that it is sprag's only behaviour rather than an option).
    /// That is what makes running something in a pane and reading its output afterwards work at
    /// all, and it is also why this fact has to travel: without it a finished command and a hung
    /// one look identical, and the pane is the only thing on screen that could say which.
    pub dead: bool,
    /// HOW the child ended — its exit code, or the signal that killed it — and `None` while it runs
    /// or has not yet been reaped ([`PaneExit`] has the whole distinction).
    ///
    /// A SECOND fact beside [`Self::dead`], not a replacement for it: `dead` answers "is this
    /// finished?", which is the question that makes a stopped screen readable at all, and this
    /// answers "and did it work?", which only a status can. They are published in that order and
    /// only that implication holds — a `Some` here always comes with `dead`, never the reverse.
    /// Named for the CHILD, not for "exit", because [`Self::last_exit_status`] sits three fields
    /// below and means something else entirely — the status of the last command the shell ran, which
    /// says nothing about whether the shell itself is still there. One vocabulary carries the same
    /// name to the wire key and the client method.
    pub child_exit: Option<PaneExit>,
    /// The pane's shell-integration state (OSC 133), `Unknown` without integration. Derived from
    /// the screen's prompt marks — the "idle at a prompt vs running a command" summary.
    pub shell_state: ShellState,
    /// The last finished command's exit status (OSC 133 `D`), `None` when none has finished with a
    /// reported status. Pair with [`Self::shell_state`] to tell "no command ran" from "unreported".
    pub last_exit_status: Option<i32>,
    /// Which pointer events the child has asked the terminal to report (the DECSET mouse-tracking
    /// mode), [`MouseProtocol::None`] when it is not tracking. A display client reads this each poll
    /// to decide whether to capture the pointer for reporting instead of handling it itself
    /// (selection, wheel-scroll), and — from the level — whether to forward drag / motion.
    pub mouse_protocol: MouseProtocol,
    /// Whether the child has asked the terminal to report focus changes (DECSET 1004), `false` when
    /// it has not. A display client reads this to decide whether to emit a focus-in / focus-out edge
    /// on a pane focus change; an agent reads it to learn the app reacts to focus (invisible in the
    /// pane's text). Orthogonal to [`Self::mouse_protocol`] — a child may set either, both, or neither.
    pub focus_tracking: bool,
    /// Monotonic count of OSC 52 clipboard WRITES this pane's child has requested (`0` before the
    /// first). A display client that remembers the value it last applied learns a NEW write
    /// arrived when this grows, then fetches the (potentially large) payload on demand and applies
    /// it — subject to policy — to its own system clipboard. The seq alone travels in the pane
    /// list; the payload does not (see [`Pane::clipboard_write`]).
    pub clipboard_write_seq: u64,
    /// The most recent OSC 52 clipboard READ query the child requested, or `None` — the single
    /// selection it wants read back. Tiny, so it travels inline (unlike the write payload).
    pub clipboard_query: Option<ClipboardQuery>,
    /// Monotonic count of OSC 52 clipboard READS this pane's child has requested (`0` before the
    /// first). A display client answers a NEW query — subject to policy — when this grows, the
    /// answer arbitrated to exactly one reply across clients (see [`Pane::clipboard_query`]).
    pub clipboard_query_seq: u64,
    /// The inline images (Kitty graphics, R1404) the child has transmitted, each anchored at its
    /// transmit-time cursor cell. Empty when the child transmitted none. A display client
    /// composites each over the grid; see [`Pane::images`].
    pub images: Vec<Image>,
    /// The pane whose OCCUPANT asked for this one, `None` for a pane nobody claims — see
    /// [`Pane::opened_by`], which this republishes verbatim.
    ///
    /// Unlike every DISPLAY fact above it, this is fixed at birth and never moves, which is what
    /// makes it usable as the thing an agent surface gates a destructive verb on: a reader that
    /// acts on it cannot be acting on a fact that changed under it.
    pub opened_by: Option<u64>,
    /// Whether a RESTORE gave this pane its process — see [`Pane::revived`], which this republishes
    /// verbatim.
    ///
    /// Fixed at birth like [`Self::opened_by`] beside it, and for the same reason a reader can act
    /// on it: it names how the pane in front of you started, which nothing later can change. What
    /// rests on it is register item 595 — telling an agent somebody opened from one the daemon
    /// re-ran unasked, which no question about live runs can answer after a restart.
    pub revived: bool,
    /// **WHICH CONVERSATION THE OCCUPANT IS** — see [`Pane::agent_session`], which this republishes
    /// verbatim, and [`None`] for a pane running anything else.
    ///
    /// ⚠⚠⚠ Republished for register item 619: it is the one name that survives a daemon restart
    /// when a pane id does not, so it is what a successor joins a revived pane to the run whose
    /// conversation it is. `PersistedRun::opened_by_session` is the other half of that join.
    ///
    /// ⚠ Unlike [`Self::revived`] and [`Self::opened_by`] beside it this is NOT fixed at birth — a
    /// loop replaces its agent's session on purpose — so a reader must take it as *the conversation
    /// in this pane now* rather than as a fact about how the pane started.
    pub agent_session: Option<String>,
}

/// [`PaneHooks`] for a pane that DOES NOT EXIST YET — the form a spawn site can supply, where the
/// attention hook has not been told which pane it is about.
///
/// **Two types rather than an `Option<PaneId>` in one**, because the difference is exactly the fact
/// this pair is for: below [`Self::bind`] there is a pane with an id, and above it there is not. A
/// single type carrying "the id, maybe" would let a hook that needs one be built where none exists,
/// which is the state that made "which pane raised this?" unanswerable in the first place.
#[derive(Default)]
pub struct PaneBirthHooks {
    /// The repaint wake, as [`PaneHooks::on_dirty`].
    pub on_dirty: Option<Box<dyn Fn() + Send>>,
    /// The child-exited signal, as [`PaneHooks::on_exit`].
    pub on_exit: Option<Box<dyn Fn() + Send>>,
    /// The child-is-asking-for-a-person signal, as [`PaneHooks::on_attention`] — but taking the
    /// [`PaneId`] too, because the caller cannot know it and the pool can.
    pub on_attention: Option<Box<dyn Fn(PaneId, Attention) + Send>>,
}

impl PaneBirthHooks {
    /// Bind these hooks to the pane `id` names — the one moment the two are together.
    ///
    /// [`PaneHooks::home`] is left unset here and filled by the pool, which is the whole of R337's
    /// correction: this type is what a CALLER supplies, a pane's cgroup is not a caller's to know,
    /// and the four doors that each had to remember to say it are now four doors that cannot.
    #[must_use]
    pub fn bind(self, id: PaneId) -> PaneHooks {
        PaneHooks {
            on_dirty: self.on_dirty,
            on_exit: self.on_exit,
            on_attention: self.on_attention.map(|tell| {
                Box::new(move |attention| tell(id, attention)) as Box<dyn Fn(Attention) + Send>
            }),
            #[cfg(unix)]
            home: None,
        }
    }
}

/// Everything a RESTORED pane is reborn from: the recorded identity the layout still references it
/// by, what to run, and what it carries back with it.
///
/// A struct rather than a parameter list because restore-time facts keep accruing — first the id,
/// then the size, then the birth hooks, now the recorded scrollback — and each one would otherwise
/// widen a signature every caller has to edit. Grouping them means the NEXT restore-time fact is an
/// added field, not a churned call site. See [`Workspace::spawn_restored`].
pub struct PaneRebirth {
    /// The id to come back under. The window's arrangement, float set and homes all reference the
    /// pane by it, so a restored pane that took a fresh id would leave the tree pointing at nothing.
    pub id: PaneId,
    /// What to run in the pane — the recorded command, or the shell a non-allowlisted argv falls
    /// back to. The caller (the host's restore) owns that decision, not this crate.
    pub command: CommandBuilder,
    /// The pane's display label — DERIVED from what actually re-ran, so a pane that fell back to a
    /// shell is labelled a shell.
    pub label: String,
    /// **WHAT A REPLACEMENT OF THIS PANE RE-RUNS** — the rebuilt argv WITHOUT whatever the restore
    /// added so the child could come back to its own conversation. Register item 695.
    ///
    /// # ⛔⛔⛔⛔⛔ Why it is a field and not [`command`](Self::command)'s own argv
    ///
    /// This used to be `argv_of(&command)`, taken here. The caller's command carries `--resume
    /// <uuid>` when the pane was a named agent — correctly, because a restore is a pane coming BACK
    /// — and [`Pane::argv`] is *what a replacement re-runs*. So every session replacement after a
    /// reboot re-entered the same conversation, which is the exact thing `ai_loop.scxml`'s
    /// `restarting` exists to prevent: it replaces the inner session to throw the accumulated
    /// context away.
    ///
    /// ⚠⚠ **RESTORING AND REPLACING WANT OPPOSITE ANSWERS OUT OF ONE REBUILD**, and this is
    /// [`start_dir`](Self::start_dir)'s shape exactly — item 684 split *where the child comes back*
    /// from *where the pane belongs* for the same reason and in the same struct. Two questions, two
    /// fields.
    ///
    /// ⚠ Measured in the field with a control: a restored pane's replacements carried one uuid five
    /// times while its transcript grew from 2.78 MB to 6.6 MB, and a pane made fresh in the same
    /// daemon minted a new id at every replacement. The only variable was *had it been restored*.
    pub replacement_argv: Vec<String>,
    /// **WHERE THE PANE WAS POINTED**, out of the snapshot
    /// ([`PaneRestore::start_dir`](crate::PaneRestore::start_dir)) — what the reborn pane REMEMBERS,
    /// and therefore where its first replacement starts.
    ///
    /// It is a separate argument from [`command`](Self::command)'s own directory because a restore
    /// resolves TWO: the command spawns where the pane's CHILD was (item 417 — a person comes back
    /// where they were working), and this is where the pane was ASKED to be. Only the second one
    /// survives the child that described it, which is register item 684's whole subject.
    ///
    /// `None` — a snapshot older than that field — falls back to deriving the place from the command,
    /// which is what the daemon that wrote such a file did.
    pub start_dir: Option<std::path::PathBuf>,
    /// The `(cols, rows)` to open at, so the restored pane is the size it was.
    pub size: (u16, u16),
    /// The pane's reader-thread callbacks, UNBOUND — the same [`PaneBirthHooks`] a fresh spawn
    /// takes, even though a restore does not mint an id.
    ///
    /// They used to arrive already bound, on the argument that the caller had the id in hand. What
    /// that also gave the caller was a [`PaneHooks::home`] to fill, and a restore is one of the
    /// three doors that never filled it (R337). Binding in [`Workspace::spawn_restored`] instead
    /// makes the pool the only thing that can say where a pane's cgroup is, on either path.
    ///
    /// A restored pane replays its recorded scrollback, which may contain the OSC that raised a
    /// notification BEFORE the reboot. That replay does not fire `on_attention`: the reader thread
    /// reads its starting marks after the replay ([`PanePty::spawn_with_dirty`]), so a restore is
    /// silent and only what THIS child says reaches a person.
    pub hooks: PaneBirthHooks,
    /// The pane's recorded scrollback as replayable terminal bytes, applied to the fresh emulator
    /// before its child can write a byte. EMPTY brings the pane back blank — the behaviour before
    /// history was persisted, and what a disabled or unreadable history degrades to.
    pub history: Vec<u8>,
}

/// The multiplexer's pane pool: a set of live panes, a monotonic id
/// counter, and the default size a dimension-less spawn adopts.
///
/// Pinion-free by design (producer layer). The host wraps this in
/// `Arc<Mutex<Workspace>>` and exposes spawn/close/resize as `scene/invoke`
/// actions on the `WorkspaceExternal`.
///
/// The id counter is an [`Arc<AtomicU64>`] so a [`SessionRegistry`](crate::SessionRegistry)
/// can SHARE one counter across every window's workspace — giving pane ids that are
/// unique across the WHOLE registry, not just within one window. That global
/// uniqueness is what keeps a pane addressable by id alone (the per-pane wire path
/// stays window-free). A standalone [`Workspace::new`] gets its own private counter.
pub struct Workspace {
    panes: Vec<Pane>,
    next_id: Arc<AtomicU64>,
    default_size: (u16, u16),
    history_limit: HistoryLimitSource,
    pane_env: PaneEnvSource,
    /// What each birth adds to its launch's argv — see [`PaneArgsSource`].
    ///
    /// Inherited by a [`sibling`](Self::sibling) on [`pane_env`](Self::pane_env)'s argument: a
    /// window opened later must instrument its agents exactly as the first one does, or which
    /// window a person happened to open an agent in would decide whether it can report.
    pane_args: PaneArgsSource,
    /// Which part of a launch NAMES it durably — see [`PaneIdentitySource`].
    ///
    /// Inherited by a [`sibling`](Self::sibling) on [`pane_args`](Self::pane_args)'s argument, and it
    /// has to be the same argument: a pane whose agent is named in one window and anonymous in
    /// another would come back from a snapshot resumed or fresh depending on where somebody happened
    /// to open it.
    pane_identity: PaneIdentitySource,
    /// Which window this pool IS, and whose session — the two thirds of a
    /// [`PaneLineage`] that are the same for every pane here.
    ///
    /// `None` for a pool no registry owns ([`Workspace::new`] standing alone, which is what a unit
    /// test builds). Such a pool places nothing, and that is right: a pane with no window has no
    /// leaf to be under.
    ///
    /// NOT inherited by a [`sibling`](Self::sibling) — a sibling is a DIFFERENT window, and a
    /// lineage copied across would put two windows' panes under one cgroup. It is stamped by the
    /// registry at the one moment a pool becomes a window's.
    home: Option<PoolLineage>,
    /// Where this pool's panes live in the machine, and what puts them there (R336, R337).
    ///
    /// Held by the POOL rather than passed at each birth because a pool is the one thing every door
    /// onto pane life goes through — a spawn, a restore, and an [`adopt`](Self::adopt) from another
    /// window. Passing it left three of four doors placing nothing, with the gate written for the
    /// fourth green throughout.
    ///
    /// Inherited by a [`sibling`](Self::sibling), unlike [`home`](Self::home): the subtree is the
    /// whole daemon's and the lineage is one window's.
    homes: Arc<PaneHomes>,
}

/// Asked, at each pane's BIRTH, how many logical lines of scrollback that pane should retain —
/// tmux's `history-limit`.
///
/// A source rather than a number because the answer is the user's and it can change: `sprag-host`
/// installs one that reads `config.toml`, so raising the setting deepens the NEXT pane's history
/// without restarting the daemon, exactly as `default-command` already behaves. A stored `usize`
/// would freeze the setting at daemon boot.
///
/// It is a source rather than a parameter on `spawn` because this crate owns the only two places a
/// pane is ever born, and a caller that had to pass the limit is a caller that could forget to. It
/// is shared (`Arc`) and inherited by [`Workspace::sibling`] for the same reason `next_id` is: every
/// window of a session must answer from one place, or a pane's history would depend on which window
/// it happened to open in.
///
/// `Send + Sync` because a workspace lives behind a `Mutex` in the daemon and is read from the
/// snapshot thread; the closure only reads a file, so it holds no state that could race.
pub type HistoryLimitSource = Arc<dyn Fn() -> usize + Send + Sync>;

/// The [`HistoryLimitSource`] a workspace uses when nobody installs one: the emulator's own default.
///
/// A standalone pool and every unit test get this, so a workspace that is not part of a configured
/// daemon behaves exactly as it did before the setting existed.
#[must_use]
fn default_history_limit_source() -> HistoryLimitSource {
    Arc::new(|| sprag_vt::DEFAULT_SCROLLBACK_LINES)
}

/// Asked, at each pane's BIRTH, which environment variables that pane's child should carry beyond
/// the command's own — how a process INSIDE a pane learns which pane it is in and where to reach
/// the daemon that owns it (tmux's `$TMUX` / `$TMUX_PANE`).
///
/// Without it a pane's child is told only `TERM`, so nothing running in a pane can name itself: it
/// can be driven and scraped, but it cannot report. That asymmetry is what this seam removes.
///
/// **It returns PAIRS rather than editing the [`CommandBuilder`].** A closure handed the builder
/// could also change the program, the argv or the cwd, and the authority a source needs is
/// strictly "add these variables" — the same "hand out the operation, not the resource" line
/// [`sibling`](Workspace::sibling) draws around the id counter.
///
/// **It takes the [`PaneId`] and nothing else.** A pane's SESSION is deliberately absent: an id is
/// unique across the whole registry (see the type docs), so the owner can resolve the session from
/// the id, while a session name stored per pool would be a second authority that
/// [`sibling`](Workspace::sibling) silently copies — and `new_session` builds its first pool as a
/// sibling of the DEFAULT session's, so every pane of every later session would publish the
/// default session's name.
///
/// A source rather than a `spawn` parameter for [`HistoryLimitSource`]'s reason: this crate owns
/// the only two places a pane is born, and a caller that had to pass the environment is one that
/// could forget to. Consulted at each birth rather than cached, so a daemon that learns its
/// endpoint late still publishes it to the next pane.
pub type PaneEnvSource = Arc<dyn Fn(PaneId) -> Vec<(String, String)> + Send + Sync>;

/// The [`PaneEnvSource`] a workspace uses when nobody installs one: no variables at all.
///
/// A standalone pool, a GUI's in-process host and every unit test get this, so a pane that is not
/// part of a daemon publishing an endpoint is spawned exactly as it was before this seam existed —
/// and a child that finds no `SPRAG_PANE` correctly concludes there is nobody to report to.
#[must_use]
fn default_pane_env_source() -> PaneEnvSource {
    Arc::new(|_| Vec::new())
}

/// Asked, at each pane's BIRTH, what further arguments this pane's launch should carry — how sprag
/// instruments an AGENT it starts so the agent reports its own turn boundaries instead of being
/// guessed at from its screen.
///
/// [`PaneEnvSource`] gave a pane's child somewhere to report to; a child still has to be CONFIGURED
/// to report, and for an agent that configuration is command-line. `sprag-host` installs a source
/// that recognises an agent in the argv it is shown and answers the flag that carries sprag's hooks
/// for that one launch — so an agent started in a sprag pane is exact, and every `claude` elsewhere
/// on the machine is left alone.
///
/// **It ANSWERS ARGUMENTS rather than editing the [`CommandBuilder`], and they are APPENDED.**
/// [`PaneEnvSource`] draws the same line for the same reason: a closure handed the builder could
/// change the program, the cwd, or the arguments its caller meant, and the authority an
/// instrumenter needs is strictly *also pass these*. A source that cannot reach `argv[0]` cannot turn
/// somebody's shell into something else, which is what makes it safe to consult on every birth
/// rather than only on the ones a host believes are agents.
///
/// **It is shown the argv and nothing else.** What to add is decided by what is being launched, and
/// a source that also knew the pane, the window or the session would invite an instrumentation that
/// differed between two panes running the same program — which is a per-pane document to build,
/// version and clean up. There is one document per daemon because there is one daemon to report to.
///
/// A source rather than a `spawn` parameter for [`HistoryLimitSource`]'s reason: this crate owns the
/// only two places a pane is born, and a caller that had to pass the instrumentation is one that
/// could forget to.
pub type PaneArgsSource = Arc<dyn Fn(&[String]) -> Vec<String> + Send + Sync>;

/// The [`PaneArgsSource`] a workspace uses when nobody installs one: nothing added, ever.
///
/// A standalone pool, a GUI's in-process host and every unit test get this, so a pane that is not
/// part of a daemon runs exactly the argv its caller wrote.
#[must_use]
fn default_pane_args_source() -> PaneArgsSource {
    Arc::new(|_| Vec::new())
}

/// What of a LAUNCHED argv names a conversation that outlives the process — shown the argv a pane
/// actually exec'd (the caller's, plus whatever [`PaneArgsSource`] added), answering the durable
/// name or [`None`].
///
/// # ⚠⚠⚠ Why this is a SECOND source rather than part of the first
///
/// [`PaneArgsSource`] answers *what does this daemon add*, and [`Pane::argv`] deliberately excludes
/// all of it: a REPLACEMENT re-derives the daemon's additions instead of inheriting a dead daemon's.
/// That rule is right for every flag but one. An agent's session name is not a fact about the daemon
/// that spawned it — it names a transcript on disk that is still there after the daemon is gone —
/// so it is the one addition a RESTORE must carry rather than re-mint.
///
/// ⚠⚠⚠ **AND IT MUST NOT REACH [`Pane::argv`], WHICH IS WHY THE TWO ARE SEPARATE.** `respawn` re-runs
/// that argv to replace a pane's agent, and a replacement must be a FRESH session — an `ai_loop`
/// replaces its inner session precisely to throw the old context away. `live_agent`'s
/// `respawn` gate already holds that, naming this exact temptation: *"the identity reached the
/// pane's RECORDED argv and was replayed"*. Restoring and replacing want opposite answers, so they
/// read different fields.
///
/// **It is shown the argv and nothing else**, on [`PaneArgsSource`]'s argument: what a launch is
/// called is decided by what was launched, and a source that also knew the pane would invite a name
/// that differed between two panes running the same program.
pub type PaneIdentitySource = Arc<dyn Fn(&[String]) -> Option<String> + Send + Sync>;

/// The [`PaneIdentitySource`] a workspace uses when nobody installs one: no launch is ever named.
///
/// A pool outside a daemon adds no identity flag either ([`default_pane_args_source`]), so there is
/// nothing for this to find — the two defaults answer the same emptiness from both ends.
#[must_use]
fn default_pane_identity_source() -> PaneIdentitySource {
    Arc::new(|_| None)
}

impl Workspace {
    /// A new, empty workspace with its OWN private id counter, whose dimension-less
    /// spawns adopt `default_size`. For a standalone pane pool (and unit tests); a
    /// registry-owned window uses [`Self::sibling`] to share the global counter.
    #[must_use]
    pub fn new(default_size: (u16, u16)) -> Self {
        Self::with_id_source(default_size, Arc::new(AtomicU64::new(0)))
    }

    /// A new, empty workspace drawing pane ids from `next_id`.
    ///
    /// PRIVATE: sharing a counter is [`sibling`](Self::sibling)'s job, and routing every
    /// sharer through it is what keeps the counter inside the type that owns the
    /// never-reused invariant. A public constructor taking the counter would let a caller
    /// supply a fresh one for a pool that ought to share, re-introducing the duplicate ids
    /// `sibling` exists to prevent.
    fn with_id_source(default_size: (u16, u16), next_id: Arc<AtomicU64>) -> Self {
        Self {
            panes: Vec::new(),
            next_id,
            default_size,
            history_limit: default_history_limit_source(),
            pane_env: default_pane_env_source(),
            pane_args: default_pane_args_source(),
            pane_identity: default_pane_identity_source(),
            home: None,
            homes: Arc::new(PaneHomes::none()),
        }
    }

    /// Say which window this pool is, and whose session — the registry's to call, at the one moment
    /// a pool becomes a window's.
    ///
    /// Until it is called the pool places nothing, which is what a standalone
    /// [`Workspace::new`] should do. Panes already born here are NOT re-placed: a pool learns its
    /// window before it holds anything, and a re-placement would be a move nobody asked for.
    pub fn set_home(&mut self, home: PoolLineage) {
        self.home = Some(home);
    }

    /// Which window this pool is, if a registry has told it.
    #[must_use]
    pub fn home(&self) -> Option<PoolLineage> {
        self.home
    }

    /// Install where this pool's panes live in the machine — see
    /// [`SessionRegistry::set_pane_homes`](crate::SessionRegistry::set_pane_homes), which is how a
    /// daemon reaches every pool at once.
    ///
    /// Affects FUTURE births and moves. A pane already running keeps the cgroup it was born in
    /// until something moves it, which is the same rule every other installed source keeps.
    pub fn set_pane_homes(&mut self, homes: Arc<PaneHomes>) {
        self.homes = homes;
    }

    /// Where this pool's panes live in the machine — the same homes every birth and move goes
    /// through, handed out so a READING can be taken through the one door that placed them.
    ///
    /// An `Arc` clone rather than a borrow because the caller is
    /// [`PaneResourceSampler`](crate::PaneResourceSampler), which must release this pool's lock
    /// before it touches a filesystem: a pool lock is what a pane's own output is waiting on, and
    /// holding one across four file reads per pane would make measuring the panes the reason they
    /// stutter.
    #[must_use]
    pub fn pane_homes(&self) -> Arc<PaneHomes> {
        Arc::clone(&self.homes)
    }

    /// The full lineage of a pane of this pool, if this pool has a window.
    fn lineage(&self, pane: PaneId) -> Option<PaneLineage> {
        self.home.map(|home| home.pane(pane))
    }

    /// Open the cgroup a pane of this pool belongs in, for its child to join before it execs.
    ///
    /// The composition the whole design rests on: the pool supplies the two ids it has held since
    /// its window was made, the birth supplies the third, and no caller anywhere says a word about
    /// cgroups. `None` where there is no window yet or nothing to enforce.
    #[cfg(unix)]
    fn open_home(&self, pane: PaneId) -> Option<std::os::fd::OwnedFd> {
        self.homes.open(self.lineage(pane)?)
    }

    /// The grant the pane with `id` is currently FOLLOWING — its own if a person has singled it
    /// out, otherwise the machine's, read fresh. `None` when this pool holds no such pane.
    ///
    /// # Why an editor of one setting needs this
    ///
    /// A person changing a pane's memory ceiling has said nothing about its CPU weight, and the two
    /// must not move together. So a caller building the NEW grant starts from the one in force and
    /// overwrites only what was asked for, which is what makes `grant --memory 512` leave a weight
    /// somebody set an hour ago alone. Composing that out of the pane's own grant (private) and
    /// the machine's
    /// default at each call site would be the same rule written twice, and the second copy is the
    /// one that forgets the fallback.
    #[must_use]
    pub fn pane_grant_or_default(&self, id: PaneId) -> Option<crate::share::Grant> {
        let own = self.panes.iter().find(|pane| pane.id == id)?.grant;
        Some(own.unwrap_or_else(|| self.homes.default_grant()))
    }

    /// Give the pane with `id` the resources a person just asked for, and answer with what the
    /// kernel holds afterwards.
    ///
    /// `None` when this pool has no such pane — the same absence every other id-taking method here
    /// reports, and distinct from the inner [`Unmeasured`](crate::share::Unmeasured), which says the
    /// pane exists and there is nothing measuring it.
    ///
    /// # Why the request is recorded even when the write did not take
    ///
    /// The two are different facts and the pane keeps both halves of the distinction: what a person
    /// SAID is remembered on the pane (its private `grant` field) so a later move carries it, and
    /// what the
    /// kernel HOLDS is what comes back. A daemon that recorded only what took would forget an
    /// override the moment a host could not honour it — and then honour nothing when that pane moved
    /// to a level that could.
    pub fn set_pane_grant(
        &mut self,
        id: PaneId,
        grant: crate::share::Grant,
    ) -> Option<Result<crate::share::Granted, crate::share::Unmeasured>> {
        let home = self.panes.iter().find(|pane| pane.id == id)?.home;
        let answer = self.homes.grant(home, grant);
        // After the write, so a pane that is not in this pool has not been recorded against.
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == id) {
            pane.grant = Some(grant);
        }
        Some(answer)
    }

    /// Install the [`HistoryLimitSource`] this pool's births consult — the seam `sprag-host` uses to
    /// put the user's `history-limit` behind every pane without this crate learning what a config
    /// file is.
    ///
    /// Affects FUTURE births only. It cannot do anything else: a pane's retention lives in its
    /// emulator from the moment it is born, so panes already in this pool keep the limit they were
    /// spawned with — which is also the behaviour the option itself promises, and tmux's.
    ///
    /// Held rather than passed at each spawn for the reason `sprag-host`'s own `set_pane_hooks`
    /// states (named in prose, not linked — a crate this one cannot depend on): the callers that
    /// birth panes take no argument for it, and one that had to would be one that could forget.
    pub fn set_history_limit_source(&mut self, source: HistoryLimitSource) {
        self.history_limit = source;
    }

    /// Install the [`PaneEnvSource`] this pool's births consult — the seam `sprag-host` uses to put
    /// a pane's identity and its daemon's address into every pane's child without this crate
    /// learning what a socket is.
    ///
    /// Affects FUTURE births only, and cannot do anything else: a child's environment is fixed at
    /// `exec`, so a pane already spawned keeps what it was born with. That is also why the seam is
    /// a source and not a mutable map — there is no live view of it to offer.
    pub fn set_pane_env_source(&mut self, source: PaneEnvSource) {
        self.pane_env = source;
    }

    /// Install the [`PaneArgsSource`] this pool's births consult — the seam `sprag-host` uses to
    /// instrument an agent's own launch without this crate learning what an agent is.
    ///
    /// Affects FUTURE births only, for [`set_pane_env_source`](Self::set_pane_env_source)'s reason
    /// and more absolutely: an argv is fixed at `exec` and there is not even a later moment to
    /// correct it in.
    pub fn set_pane_args_source(&mut self, source: PaneArgsSource) {
        self.pane_args = source;
    }

    /// Install the [`PaneIdentitySource`] this pool's births consult — the seam `sprag-host` uses to
    /// say which flag of a launch NAMES a conversation, without this crate learning what an agent is.
    ///
    /// Affects FUTURE births only, on [`set_pane_args_source`](Self::set_pane_args_source)'s terms:
    /// it reads the argv, and an argv is fixed at `exec`.
    pub fn set_pane_identity_source(&mut self, source: PaneIdentitySource) {
        self.pane_identity = source;
    }

    /// The default `(cols, rows)` a dimension-less spawn adopts.
    #[must_use]
    pub fn default_size(&self) -> (u16, u16) {
        self.default_size
    }

    /// The next id this pool's shared counter would mint — the global high-water mark, for a
    /// durability snapshot to store so a restore never reissues a retired id
    /// (see [`SessionRegistry::from_snapshot`](crate::SessionRegistry::from_snapshot)). A HINT, not
    /// a reservation: reading
    /// it takes no id, and `Relaxed` matches the mint path (the value only advances, and a
    /// best-effort snapshot needs no synchronization with it).
    #[must_use]
    pub fn next_id_hint(&self) -> u64 {
        self.next_id.load(Ordering::Relaxed)
    }

    /// A new, empty workspace whose shared id counter STARTS at `next` — so its first mint is
    /// `next`, not 0.
    ///
    /// How a restore rebuilds the pane pool without reissuing an id the pre-reboot session had
    /// already minted ([`SessionRegistry::from_snapshot`](crate::SessionRegistry::from_snapshot)).
    /// Seeding to the stored high-water mark — rather than deriving it from the restored ids —
    /// is what preserves the never-reused invariant across the gap a top-of-range close leaves:
    /// pane 5 minted then closed pre-reboot leaves live ids `{0,1,2}`, and a counter derived from
    /// those would reissue 3, 4, 5. `pub(crate)`: seeding a counter is a restore concern of this
    /// crate's registry, not a knob for arbitrary callers (same reason
    /// [`with_id_source`](Self::with_id_source) is private).
    #[must_use]
    pub(crate) fn with_seeded_counter(default_size: (u16, u16), next: u64) -> Self {
        Self::with_id_source(default_size, Arc::new(AtomicU64::new(next)))
    }

    /// A new, empty pool minting from THIS one's id counter and inheriting its default size —
    /// how a [`SessionRegistry`](crate::SessionRegistry) adds a window or a session.
    ///
    /// **Hands out the OPERATION, not the resource.** The obvious shape — a getter returning
    /// the `Arc<AtomicU64>` — looks like a read handle and is not: the caller also gets
    /// `.store()`, and one call would reset the counter and mint duplicate [`PaneId`]s across
    /// every window in every session, which is the invariant this module calls load-bearing.
    /// The enforcement of an invariant must not leave the type that owns it, so the counter
    /// never leaves; only the ability to start a pool that shares it does.
    #[must_use]
    pub fn sibling(&self) -> Self {
        Self {
            panes: Vec::new(),
            next_id: Arc::clone(&self.next_id),
            default_size: self.default_size,
            // Shared for the same reason the id counter is: a new window is not a new configuration,
            // and a sibling that defaulted here would give a session's second window shallower
            // history than its first for no reason the user could see.
            history_limit: Arc::clone(&self.history_limit),
            // Inherited for that same reason, and SAFELY so only because a `PaneEnvSource` is handed
            // nothing but the pane's id. `new_session` builds a new session's first pool as a sibling
            // of the DEFAULT session's, so a source that closed over a session name would publish the
            // wrong one to every pane of every session created after boot.
            pane_env: Arc::clone(&self.pane_env),
            // Inherited on exactly that argument: a source shown nothing but an argv cannot carry a
            // fact about the window it was cloned from, and an agent must be instrumented the same
            // way in every window or which one it was opened in decides whether it can report.
            pane_args: Arc::clone(&self.pane_args),
            // Inherited on exactly `pane_args`' argument, and it must travel WITH it: a window whose
            // launches were instrumented but not named would snapshot agents nothing could resume.
            pane_identity: Arc::clone(&self.pane_identity),
            // NOT inherited: a sibling is a DIFFERENT window, and this pair names one window.
            //
            // Belt-and-braces rather than load-bearing, and MEASURED as such: mutating this to
            // `self.home` leaves every test green, because every sibling reaches a pane through
            // `Window::new` or `Window::restore` and both stamp the new window's own lineage over
            // whatever was here. The mutation that IS red is dropping that stamp. So this is `None`
            // to say what a pool with no window of its own is, not to defend against a live path.
            home: None,
            // Inherited, on `pane_env`'s argument and for a sharper reason: the subtree belongs to
            // the whole daemon, and a window whose panes were the only unweighted ones in it would
            // be a gap that appeared the first time somebody opened a second window.
            homes: Arc::clone(&self.homes),
        }
    }

    /// Spawn `command` on a fresh `cols x rows` pane, returning its id.
    /// `label` is the introspection label (typically the program name).
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be
    /// started; on failure no pane is added, though the id IS consumed — see
    /// [`spawn_with_dirty`](Self::spawn_with_dirty).
    pub fn spawn(
        &mut self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId, PanePtyError> {
        self.spawn_with_dirty(command, label, cols, rows, PaneBirthHooks::default())
    }

    /// [`Self::spawn`] with the pane's reader-thread callbacks ([`PaneBirthHooks`], threaded to
    /// [`PanePty::spawn_with_dirty`] once this function has minted the id to bind them to).
    ///
    /// The hooks arrive UNBOUND and are bound here, because this is the only place the pane and its
    /// id are together: [`PanePty`] does not know its pane's id, and the id does not exist until this
    /// function mints it. A caller that had to bind it itself would be binding it from a `Result` it
    /// has not received yet — the gap that makes "which pane raised this?" the wrong question to
    /// answer with a cell filled in later.
    ///
    /// A windowed host passes `on_dirty = Some(Box::new(move || sink.request_repaint()))`
    /// (the pinion R999 `RepaintSink` seam) so this pane's output wakes the shell to
    /// repaint; the headless host passes `bump_on_dirty`. `on_exit` is the "this child is
    /// gone" event the host lifetime turns on (the daemon exits when its last live pane
    /// does). Both are pinion-free (`Box<dyn Fn() + Send>`), keeping this crate decoupled
    /// from the GUI shell and the host lifetime; callers with neither use [`Self::spawn`].
    ///
    /// The pane's id is reserved BEFORE its child starts, because the id travels in that child's
    /// environment ([`PaneEnvSource`]).
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be
    /// started; on failure no pane is added, but the reserved id is CONSUMED — the counter has
    /// already advanced past it, leaving a gap that the never-reused invariant tolerates (see the
    /// mint site).
    pub fn spawn_with_dirty(
        &mut self,
        mut command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
        hooks: PaneBirthHooks,
    ) -> Result<PaneId, PanePtyError> {
        // Capture the launch argv BEFORE the builder is moved into the spawn, so a snapshot can
        // later re-run it (an allowlisted program) or fall back to a shell.
        let argv = argv_of(&command);
        // ⚠⚠ AND THE LAUNCHER'S ENVIRONMENT, at the same moment and for the same reason — see
        // [`Pane::env`]. Everything below this line that touches the environment is the DAEMON's own,
        // which a replacement re-derives.
        let env = command.env_pairs().to_vec();
        // Instrumentation is added AFTER that capture and is deliberately not part of it: what a
        // snapshot records is what the USER asked to run, and the flag added here names THIS
        // daemon's endpoint. A restore re-derives it from the daemon doing the restoring — the
        // reason `PaneEnvSource` is re-read there rather than stored, one layer out.
        instrument(&mut command, &(self.pane_args)(&argv));
        // ⚠ Read from the argv AFTER instrumentation, because the name is something the instrumenting
        // added — and kept BESIDE `argv` rather than in it, because a replacement re-runs that and
        // must be a fresh session. See [`Pane::agent_session`].
        let agent_session = (self.pane_identity)(&argv_of(&command));
        // Asked HERE rather than cached on the pool, so a user who edits `history-limit` gets it on
        // their next pane rather than on their next daemon.
        let history_limit = (self.history_limit)();
        // The id is minted BEFORE the spawn because the pane's own id travels in its child's
        // environment ([`PaneEnvSource`]) and a child cannot be told a number that does not exist
        // yet: an environment is fixed at `exec`, so there is no later moment to correct.
        //
        // The COST is stated rather than avoided: a spawn that fails now consumes an id, where it
        // used to leave the counter untouched. Nothing rests on gap-freeness — ids need uniqueness
        // and monotonicity only (that is what makes a pane addressable by id alone), and a
        // durability snapshot stores the high-water mark rather than deriving it from live ids, so
        // a gap survives a restore as harmlessly as the one a top-of-range close already leaves.
        // The alternative — peeking the counter for the environment and minting after — would let
        // the two disagree under a concurrent spawn, which is the failure this ordering makes
        // unrepresentable. Relaxed ordering: ids need no synchronization with other memory.
        let id = PaneId(self.next_id.fetch_add(1, Ordering::Relaxed));
        for (key, value) in (self.pane_env)(id) {
            command.env(key, value);
        }
        // The id binds HERE, which is the whole reason this layer takes `PaneBirthHooks`: below this
        // line there is a pane with an id and a child, and above it there is neither.
        let mut bound = hooks.bind(id);
        // And the pane's cgroup is opened HERE, for the same reason and at the same moment: this is
        // where the pane and its id are together, and the pool has held the other two ids since its
        // window was made. No caller passes it, which is precisely why no caller can omit it.
        self.offer_home(id, &mut bound);
        // ⚠⚠ AND WHERE IT WAS POINTED, read off the builder on the line before it is moved into the
        // spawn — so this is the directory the child actually got, not a claim about it. Taken here
        // rather than beside `argv` because `start_dir` resolves a default and checks the directory
        // still exists, and the answer wanted is the resolved one. See [`Pane::start_dir`].
        let start_dir = std::path::PathBuf::from(command.start_dir());
        let pty = PanePty::spawn_with_dirty(command, cols, rows, bound, &[], history_limit)?;
        // AFTER the spawn, because the birth is what answers it — see `landed_home`.
        let home = self.landed_home(id, &pty);
        self.panes.push(Pane {
            id,
            pty,
            command_label: label,
            argv,
            env,
            start_dir,
            agent_session,
            remote: None,
            opened_by: None,
            // A person or a run asked for this one, which is what the fresh-spawn door MEANS.
            revived: false,
            name: None,
            home,
            grant: None,
        });
        Ok(id)
    }

    /// Open the newborn pane's cgroup into `hooks`, for its child to join before it execs.
    fn offer_home(&self, id: PaneId, hooks: &mut PaneHooks) {
        #[cfg(unix)]
        {
            hooks.home = self.open_home(id);
        }
        // A platform with no cgroups places nothing, so a pane there is never anywhere to be moved
        // out of. `Share` is still a fact of the product there; only its enforcement is missing.
        #[cfg(not(unix))]
        {
            let _ = (id, hooks);
        }
    }

    /// Where the newborn pane's processes ACTUALLY landed — its lineage if its child got into the
    /// cgroup [`offer_home`](Self::offer_home) opened, `None` if it is running in the daemon's.
    ///
    /// # Why the answer is the JOIN and not the OPEN
    ///
    /// [`Pane::home`] is what a later [`adopt`](Self::adopt) migrates OUT of, so it has to mean
    /// *this pane's processes are in this cgroup* and not *this is the cgroup they would be in*. A
    /// pane whose placement failed — no tree, or a tree that would not take it — is in the daemon's
    /// own cgroup, and recording an address for it would make a later move read an empty source,
    /// log a failure and leave the processes exactly where they were while a fresh empty leaf
    /// appeared under the new window.
    ///
    /// Deriving it from the DESCRIPTOR made that state unrepresentable for one of the two ways in
    /// and not the other, and the miss was the one the doc above already names: *a tree that would
    /// not take it*. Opening `cgroup.procs` tests the permissions on a file; admitting a process to
    /// it runs cgroup v2's containment rule against the WRITER's own cgroup, which no inspection of
    /// the destination can predict. So a descriptor opens on hosts where every migration is then
    /// refused — GitHub's Linux runner is one — and every pane there had a home recorded that its
    /// processes were never in. The birth is the first moment the difference is knowable, so the
    /// answer is taken from the birth.
    #[allow(
        clippy::unused_self,
        reason = "the `cfg(not(unix))` arm uses neither field"
    )]
    fn landed_home(&self, id: PaneId, pty: &PanePty) -> Landing {
        #[cfg(unix)]
        {
            match pty.joined() {
                // `lineage` is `None` for a pane whose pool has no window home, which is a pane
                // nothing was opened for — so the join cannot have been asked, let alone answered.
                crate::pty::Joined::Joined => {
                    self.lineage(id).map_or(Landing::Unplaced, Landing::At)
                }
                crate::pty::Joined::NotAsked => Landing::Unplaced,
                // The errno the CHILD reported, kept rather than reduced to an absence: it is the
                // only account of the refusal that will ever exist, the kernel does not record it
                // anywhere a later read could find, and it is what tells a person the fault is not
                // in this daemon. `raw_os_error` is `Some` by construction — `Pty::heard` builds
                // this error with `from_raw_os_error` and nothing else constructs the arm — and the
                // fallback spells the impossible case rather than unwrapping it.
                crate::pty::Joined::Refused(error) => Landing::Refused(crate::Refusal::from_errno(
                    error.raw_os_error().unwrap_or_default(),
                )),
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (id, pty);
            Landing::Unplaced
        }
    }

    /// Re-spawn a pane exactly as it was recorded — the restore primitive. See [`PaneRebirth`] for
    /// what a restored pane comes back with.
    ///
    /// A [`SessionRegistry`](crate::SessionRegistry) restore re-spawns each pane the pre-reboot
    /// session held, and the arrangement ([`LayoutTree`](crate::LayoutTree)), float set, and
    /// homes all reference those panes by id — so a restored pane MUST come back under its old
    /// id or the tree would point at nothing. The id is reserved as it is used
    /// (`next_id = max(next_id, id + 1)`), so a later mint can never reissue it: the never-reused
    /// invariant holds across a restore, not only within one process's monotonic minting.
    ///
    /// The caller owns uniqueness — restore draws ids from a snapshot where they are unique by
    /// construction. Unlike [`spawn`](Self::spawn) there is no id to return (the caller chose it).
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be started; on failure no
    /// pane is added.
    pub fn spawn_restored(&mut self, pane: PaneRebirth) -> Result<(), PanePtyError> {
        let PaneRebirth {
            id,
            mut command,
            label,
            replacement_argv,
            start_dir: pointed_at,
            size: (cols, rows),
            hooks,
            history,
        } = pane;
        // ⚠⚠⚠⚠⚠ TWO ARGVS, AND THIS IS THE ONE THAT IS ABOUT TO RUN — register item 695. It carries
        // whatever the restore added so the child comes back to its own conversation, and the
        // instrumenting below has to see it: `identity_args` stands down when it sees a resume, so
        // a restored agent is instrumented afresh and named not at all. Handing this step the
        // REPLACEMENT's argv would mint an identity beside a resume and name one pane twice.
        let launched = argv_of(&command);
        // Re-derived from THIS daemon rather than restored, exactly as the environment below is and
        // for a sharper version of its reason: the recorded argv names what the pane ran, and the
        // flag this adds names an endpoint that did not survive the reboot. A restore that replayed
        // a stored instrumentation would point a fresh agent at a dead socket.
        instrument(&mut command, &(self.pane_args)(&launched));
        // ⚠⚠ AND WHAT THAT LAUNCH IS CALLED, on the fresh-spawn path's terms. The caller's `command`
        // may already carry a RESUME of the recorded name (`Host::restore` puts one there), in which
        // case the instrumenting above mints nothing and this reads back the name being resumed — so
        // a pane that comes back to its own conversation says so, and a chained restore carries it
        // again. A restore that fell back to a shell names nothing, which is the honest answer.
        let agent_session = (self.pane_identity)(&argv_of(&command));
        // A RESTORED pane reads the setting live too, rather than inheriting whatever it had before
        // the reboot: the snapshot records what a pane WAS, and its retention is a current setting,
        // not a property of the pane the user is getting back. Replaying more history than the
        // limit allows is safe either way — the replay scrolls through the same eviction path any
        // live output does, so the pane settles at exactly the configured depth.
        let history_limit = (self.history_limit)();
        // A restored pane's environment is re-derived from the CURRENT daemon rather than restored
        // from the snapshot, for the reason the argv path already states: the snapshot records what
        // a pane WAS, and the endpoint serving it now is a fact of this process. Its id is the
        // caller's, so there is no ordering constraint here — only the same publication.
        for (key, value) in (self.pane_env)(id) {
            command.env(key, value);
        }
        // Bound HERE rather than by the caller, for the reason [`spawn_with_dirty`] binds: this is
        // where the pane's cgroup is opened, and a caller that bound its own hooks would be a caller
        // able to arrive with `home` unset. A restore chose the id, so there is no minting to do —
        // only the same one binding site.
        let mut bound = hooks.bind(id);
        self.offer_home(id, &mut bound);
        // ⚠⚠⚠⚠⚠ THE RESTORE DOOR RESOLVES **TWO** DIRECTORIES, AND THIS IS THE SECOND — register
        // item 684. The command's own is where the CHILD comes back (`restore_command` set it from
        // the snapshot's `/proc` reading, so a person returns to the directory they were working in —
        // item 417's decision, deliberately kept). This is where the pane was POINTED, and it is what
        // the reborn pane REMEMBERS, so the first replacement after a reboot goes there.
        //
        // ⚠ Deriving it from the command — which is what this line used to do — laundered the intent
        // back into the reading item 684 had just removed from `respawn`: a snapshot taken after the
        // child exited carries `cwd: None`, so the restored pane came back pointed at `$HOME` and
        // every replacement after it inherited that.
        //
        // Both go through the SAME rule (`dir_or_home`), so a recorded intent whose directory was
        // deleted between the snapshot and this boot means `$HOME` here exactly as the command's does
        // — the pane comes back saying somewhere it can actually spawn, never a path nothing can.
        // ⚠ `None` is a snapshot written before the field existed: derive it, as its daemon did.
        let start_dir = std::path::PathBuf::from(match &pointed_at {
            Some(pointed_at) => command.dir_or_home(Some(pointed_at.as_os_str())),
            None => command.start_dir(),
        });
        let pty = PanePty::spawn_with_dirty(command, cols, rows, bound, &history, history_limit)?;
        // The RESTORE door, and it takes the answer from the birth for the same reason the spawn
        // door does: a pane coming back from a snapshot joins its new cgroup exactly as a fresh one
        // does, and can be refused exactly as one can.
        let home = self.landed_home(id, &pty);
        // Reserve the id above the counter so a future mint cannot reissue it (saturating so a
        // pathological u64::MAX id cannot wrap the reservation back to 0). Relaxed matches the
        // mint path: ids need only uniqueness + monotonicity, not synchronization.
        self.next_id
            .fetch_max(id.0.saturating_add(1), Ordering::Relaxed);
        self.panes.push(Pane {
            id,
            pty,
            command_label: label,
            // ⛔⛔⛔⛔⛔ THE CALLER'S, NOT `argv_of(&command)` — register item 695. This field is
            // what a REPLACEMENT re-runs, and the command that just spawned carries a resume so the
            // child could come back to its own conversation. Taking it from there made every
            // session replacement after a reboot re-enter the same conversation, defeating the
            // state `ai_loop.scxml` replaces a session in order to reach.
            argv: replacement_argv,
            // ⚠ A RESTORED PANE RECORDS NONE, and that is the snapshot's shape rather than an
            // omission: it stores argv and cwd, so there is nothing here to restore. What it costs is
            // stated on [`Pane::env`] — a restored pane's REPLACEMENT is the program without its
            // original launcher's variables.
            env: Vec::new(),
            // ⚠ NOT empty here, unlike `env`: the snapshot stores where the pane was POINTED as its
            // own field, so a restored pane knows where it is MEANT to be — not merely where its
            // last child had got to — and its replacement can be put there.
            start_dir,
            agent_session,
            remote: None,
            opened_by: None,
            // ⚠⚠⚠⚠⚠ **THE ONE PLACE THIS IS TRUE**, and it is a constant rather than a parameter for
            // that reason: every pane born through this door was re-run out of a snapshot, so a
            // caller able to pass `false` would be a caller able to hide an orphan. See the
            // [field](Pane::revived).
            revived: true,
            name: None,
            home,
            grant: None,
        });
        Ok(())
    }

    /// Remove the pane with `id`, **returning it** so the caller drops it —
    /// running [`PanePty`]'s `kill` / `wait` / `join` on `Drop` —
    /// *outside* any lock the caller is holding (those are blocking process
    /// ops; reaping under a shared lock would stall everything contending on
    /// it, e.g. an in-flight plugin run). Returns `None` if no pane has `id`.
    #[must_use]
    pub fn close(&mut self, id: PaneId) -> Option<Pane> {
        let index = self.panes.iter().position(|pane| pane.id == id)?;
        Some(self.panes.remove(index))
    }

    /// Take in an ALREADY-LIVE pane — the exact inverse of [`close`](Self::close), and the one
    /// primitive a cross-window move (`break-pane` / `join-pane`) needs.
    ///
    /// A pane LEAVES one pool through [`close`](Self::close) (removed and RETURNED, its blocking
    /// `Drop` deliberately NOT run) and ENTERS another through this, its object intact — PTY,
    /// emulator, scrollback, and reader thread all untouched, because a pane carries its whole
    /// world and this only moves the owning `Vec` slot. Nothing is re-spawned and no child is
    /// signalled: the move is a pure relocation, so a `break-pane` keeps the user's shell, its
    /// history, and its running program exactly as they were.
    ///
    /// **Why the id is already safe.** Every pool in a [`SessionRegistry`](crate::SessionRegistry)
    /// shares ONE id counter ([`sibling`](Self::sibling)), so a pane brought in from a sibling pool
    /// already carries a [`PaneId`] unique across the whole registry — this cannot introduce a
    /// collision. The counter is nonetheless advanced past the adopted id
    /// (`next_id = max(next_id, id + 1)`, saturating), so the never-reused invariant holds even for
    /// a pane adopted from a pool that did NOT share this counter (there is no such caller today;
    /// the reservation makes the primitive correct regardless, the same discipline
    /// [`spawn_restored`](Self::spawn_restored) keeps for a restore).
    ///
    /// The caller owns membership: it must have obtained `pane` from a [`close`](Self::close) it
    /// just performed, so the same id is never live in two pools at once.
    ///
    /// **A pane's share moves with it.** The resource tree is a projection of the identity tree, and
    /// this is the one place a live pane's identity changes — so this is where the projection is
    /// re-computed: the pane's processes are moved into the cgroup its NEW window spells and its old
    /// leaf is released. Doing it here rather than in each of `break-pane` / `join-pane` /
    /// `move-pane` / `swap` is the same argument the rest of this type makes: four callers of one
    /// rule is three chances to write it differently.
    pub fn adopt(&mut self, mut pane: Pane) {
        self.next_id
            .fetch_max(pane.id.0.saturating_add(1), Ordering::Relaxed);
        let arriving = self.lineage(pane.id);
        // Both halves have to be known: a pane arriving from a pool that never had a window has no
        // cgroup to move out of, and a pool with no window of its own has none to move it into.
        //
        // ⚠ AND THE PANE'S ANSWER FOLLOWS THE MOVE THAT HAPPENED, not the one that was available.
        // Assigning the destination unconditionally — which is what shipped — told a pane that had
        // never been in a cgroup that it was now in one, on the strength of a `relocate` that the
        // same condition had just declined to call. That is the defect `home` was made the join's
        // answer to remove (R341), one layer up: an address recorded for processes that are not at
        // it. A pane the kernel refused keeps the refusal, because refusing to move a process the
        // kernel would not admit in the first place is not a move.
        match (pane.home.leaf(), arriving) {
            (Some(from), Some(to)) => {
                // The pane's OWN grant travels with it, and `None` is the answer for the pane
                // nobody has singled out — see the field for why the kernel cannot supply this bit.
                self.homes.relocate(from, to, pane.grant);
                pane.home = Landing::At(to);
            }
            // A pool with no window of its own has nowhere to move it into, so nothing moved and
            // the processes are still where they were. Recording the destination here would name a
            // leaf that was never made.
            (Some(_), None) => {}
            // Nothing to move out of: the birth's answer stands, whichever of the two it was.
            (None, _) => {}
        }
        self.panes.push(pane);
    }

    /// Resize the pane with `id` to `cols x rows` (PTY + emulator).
    ///
    /// Returns `Ok(true)` when the pane exists and was resized, `Ok(false)`
    /// when no pane has that id.
    ///
    /// Takes `&self`: [`PanePty::resize`] is `&self` (interior-mutable
    /// PTY + emulator), so a shared `&Workspace` — e.g. one reached through an
    /// `Rc` in the GUI's resize Effect — can reflow a pane without owning the
    /// pool. The host caller (which holds a `MutexGuard<Workspace>`) is
    /// unaffected: a `&mut` guard still calls a `&self` method.
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the PTY winsize ioctl fails.
    /// `cell_px` is the display's `(cell_width, cell_height)` in logical pixels (`(0, 0)` = unknown);
    /// it is forwarded to [`PanePty::resize`] so the PTY winsize carries real pixel extents.
    pub fn resize(
        &self,
        id: PaneId,
        cols: u16,
        rows: u16,
        cell_px: (u16, u16),
    ) -> Result<bool, PanePtyError> {
        match self.panes.iter().find(|p| p.id == id) {
            Some(pane) => {
                pane.pty.resize(cols, rows, cell_px)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// The pane with `id`, or `None`.
    #[must_use]
    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// Mark the pane with `id` as a remote workspace pane (the `sprag ssh` intent marker). Set
    /// AFTER the spawn — the endpoint is metadata the pane process does not need — by the birth
    /// path and by a restore. A no-op for an unknown id.
    pub fn set_pane_remote(&mut self, id: PaneId, remote: SshRemote) {
        if let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) {
            pane.remote = Some(remote);
        }
    }

    /// Record that `opener`'s occupant is the one that asked for the pane with `id`
    /// ([`Pane::opened_by`]). Set AFTER the spawn for [`set_pane_remote`](Self::set_pane_remote)'s
    /// reason — provenance is metadata the pane process does not need — by the birth path and by a
    /// restore. A no-op for an unknown id.
    ///
    /// Takes no interest in whether `opener` names a live pane: this pool is the membership
    /// authority for ONE window, and an opener may legitimately sit in another. Whoever accepts a
    /// provenance from a caller is the one that must check it against the whole session (the host's
    /// `spawn`/`split` actions do, before ever reaching here).
    pub fn set_pane_opened_by(&mut self, id: PaneId, opener: PaneId) {
        if let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) {
            pane.opened_by = Some(opener);
        }
    }

    /// Give the pane with `id` the name `name`, or take its name away with `None`
    /// ([`Pane::name`]). Answers whether this pool held that pane at all.
    ///
    /// Set AFTER the spawn on the same terms as
    /// [`set_pane_opened_by`](Self::set_pane_opened_by) — a name is metadata the pane PROCESS does
    /// not need — by the birth path, by a rename and by a restore.
    ///
    /// **Takes no interest in whether the name is already taken**, for the reason that function
    /// states about a provenance: this pool is the membership authority for ONE window, while a
    /// name is unique across the whole REGISTRY, so the pool cannot see the set it would have to
    /// check against. Whoever accepts a name from a caller is the one that must check it
    /// daemon-wide, and the host's `rename_pane` / birth actions do, before ever reaching here.
    ///
    /// Answers `false` for an unknown id rather than silently doing nothing, because a rename that
    /// found no pane and a rename that landed are different outcomes to a caller — where a
    /// provenance stamp's caller has just spawned the pane it is stamping and cannot miss.
    pub fn set_pane_name(&mut self, id: PaneId, name: Option<crate::PaneName>) -> bool {
        match self.panes.iter_mut().find(|p| p.id == id) {
            Some(pane) => {
                pane.name = name;
                true
            }
            None => false,
        }
    }

    /// **HAND EVERYTHING A CALLER DECLARED ABOUT `from`'s SEAT TO `to`** — the pane replacing it —
    /// answering whether both panes were here to do it with.
    ///
    /// A `Seat` is what somebody SAID about this position in the window (what to call it, who
    /// asked for it, what it may spend, whether it is a remote workspace); the occupant is the
    /// program that happens to be sitting in it. A replacement changes the occupant, so the seat's
    /// declarations follow it and the occupant's own facts do not — see `Seat::of`, where the
    /// split is spelled field by field and the compiler asks about every new one.
    ///
    /// ⚠ `Seat` is named in prose and NOT linked, deliberately: it is private, and a public item
    /// linking to it is `rustdoc::private_intra_doc_links` — an error under this workspace's doc
    /// gate, which refused this very paragraph the first time it was written.
    ///
    /// # ⚠⚠⚠⚠⚠ Why this is ONE operation and not four calls at the call site
    ///
    /// Register item 478, measured on a live AI-loop run: after eight session replacements
    /// `sprag agent inner` answered *"no pane is called \"inner\""* and the run's own agent was an
    /// unnamed pane. **The name was one of four**, and the replacement dropped all four for the same
    /// reason — nothing carried them — so a fix that moved the name would have left the three beside
    /// it, each failing later and separately. The sharpest is the PROVENANCE: a pane nobody claims
    /// is one the agent surface refuses to close, so a run that replaced its inner session could no
    /// longer clean it up. Four `set_pane_*` calls at the caller is a list somebody extends by
    /// forgetting; this is the whole seat, and `Seat::of` is what keeps it whole.
    ///
    /// # ⚠⚠⚠ The name MOVES and the other three are COPIED, and the asymmetry is the point
    ///
    /// A name is unique daemon-wide, so a moment in which both panes hold it is a moment a lookup
    /// may resolve to the pane that is about to be closed — the caller's `respawn` closes the old
    /// pane only after the new one is up, precisely so a failed spawn leaves the run holding
    /// something. Moving it under this one lock means no reader can ever see two. Provenance, remote
    /// marker and grant are not unique and nothing is confused by the outgoing pane keeping its copy
    /// for the moment it has left.
    ///
    /// # ⚠⚠ It is a total hand-over, including the absences
    ///
    /// `to` ends up with exactly what `from` declared — an unnamed seat's replacement is unnamed,
    /// and it is not given a name it never had. A caller replacing a pane wants the seat as it was,
    /// not the union of two.
    ///
    /// Answers `false` — and changes nothing at all — when either id is a stranger to this pool, so
    /// a hand-over to a pane that failed to spawn cannot take the name off the pane that is staying.
    pub fn hand_seat_over(&mut self, from: PaneId, to: PaneId) -> bool {
        let Some(seat) = self.pane(from).map(Seat::of) else {
            return false;
        };
        if !self.panes.iter().any(|pane| pane.id == to) {
            return false;
        }
        let Seat {
            name,
            opened_by,
            remote,
            grant,
        } = seat;
        // ⚠ The vacated seat first, so the name it is handing over is never held twice — see above.
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == from) {
            pane.name = None;
        }
        if let Some(pane) = self.panes.iter_mut().find(|pane| pane.id == to) {
            pane.name = name;
            pane.opened_by = opened_by;
            pane.remote = remote;
        }
        // ⚠⚠ THE GRANT GOES THROUGH ITS OWN DOOR, because recording it is not applying it: a
        // ceiling copied onto the field and never written to the replacement's cgroup is a limit a
        // person can read and the kernel is not holding. `set_pane_grant` writes and then records.
        if let Some(grant) = grant {
            self.set_pane_grant(to, grant);
        }
        true
    }

    /// All panes, in spawn order.
    #[must_use]
    pub fn panes(&self) -> &[Pane] {
        &self.panes
    }

    /// Introspection metadata for every pane, in spawn order.
    #[must_use]
    pub fn list(&self) -> Vec<PaneInfo> {
        self.panes
            .iter()
            .map(|p| {
                let (cols, rows) = p.pty.dimensions();
                let (notification, notification_seq) = p.notification();
                let (shell_state, last_exit_status) = p.shell();
                let (clipboard_query, clipboard_query_seq) = p.clipboard_query();
                PaneInfo {
                    id: p.id.0,
                    cols,
                    rows,
                    command_label: p.command_label.clone(),
                    name: p.name.clone(),
                    title: p.title(),
                    notification,
                    notification_seq,
                    bell_seq: p.bell_seq(),
                    dead: p.pty.is_eof(),
                    child_exit: p.pty.exit_status(),
                    shell_state,
                    last_exit_status,
                    mouse_protocol: p.mouse_protocol(),
                    focus_tracking: p.focus_tracking(),
                    clipboard_write_seq: p.clipboard_write().1,
                    clipboard_query,
                    clipboard_query_seq,
                    images: p.images(),
                    opened_by: p.opened_by.map(|opener| opener.0),
                    revived: p.revived,
                    agent_session: p.agent_session.clone(),
                }
            })
            .collect()
    }
}

/// The argv of a [`CommandBuilder`] as owned strings (`[program, args…]`) — read at spawn so a
/// pane remembers what to re-run on restore. `to_string_lossy`, so a non-UTF-8 ARGUMENT (a
/// filename in a legacy encoding, say) is mojibake'd and an exact restore would open the wrong
/// path; the program name and ASCII flags — the common case — are exact. A faithful `OsString`
/// argv does not round-trip cleanly through the JSON snapshot, so the lossy `String` is the
/// deliberate trade-off.
/// Append `extra` to `command`'s argv — the whole of what a [`PaneArgsSource`] is allowed to do.
///
/// It is a function rather than two inline loops so the two birth doors cannot come to differ about
/// what "instrument" means, and it appends through [`CommandBuilder::arg`] rather than reaching for
/// the argv so that even this cannot touch `argv[0]`: the program a person asked for is not
/// something a source is allowed to change, and here that is enforced by the API rather than
/// promised by a comment.
fn instrument(command: &mut CommandBuilder, extra: &[String]) {
    for arg in extra {
        command.arg(arg);
    }
}

fn argv_of(command: &CommandBuilder) -> Vec<String> {
    command
        .get_argv()
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A long-lived child (`cat` reads stdin) so the pane's PTY stays open
    /// across resize/close assertions.
    fn cmd() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
    }

    /// A pane that was never placed is never MOVED out of a cgroup it was never in.
    ///
    /// The branch: a pool that knows its window but has nothing to enforce still spells a perfectly
    /// good lineage for every pane it births. If a pane recorded that address anyway, a later move
    /// into a pool that DOES have a tree would try to migrate processes out of a cgroup that does
    /// not exist — logging a failure and leaving a fresh empty leaf under the new window while the
    /// processes stayed in the daemon's own. Recording the placement's ANSWER instead of its
    /// intended address makes that unreachable, and this is what says so.
    ///
    /// **A pane the kernel would not admit is born anyway, and records NO home.**
    ///
    /// The pool's half of `pane_join_refused.rs`. That gate proves the birth survives; this proves
    /// the pane does not then claim a cgroup its processes are not in — which is the half that
    /// keeps every later reader honest. A `home` recorded for a refused pane would make
    /// [`PaneHomes::charge`](crate::share::PaneHomes::charge) report an EMPTY leaf's counters as
    /// that pane's usage (zero cores, zero memory, for a pane that may be pinning the machine) and
    /// would make a later `break-pane` migrate out of a cgroup holding nothing.
    ///
    /// TWO panes in one pool, which is what makes this discriminate: the fixture refuses the second
    /// one's `cgroup.procs` and accepts the first's, so a `landed_home` that answered `None` for
    /// everything — or `Some` for everything — fails on one of them. Measured: with the answer
    /// taken from the OPEN rather than from the JOIN, the second pane records a home.
    ///
    /// `/dev/full` is the refusal, for `pane_join_refused.rs`'s reason: it opens for writing and
    /// fails every write, on every Linux, with no cgroup tree and no privileges. Nothing on the
    /// birth path READS a leaf's `cgroup.procs` — `sweep` only `remove_dir`s and `place` only
    /// writes weights — which is what makes a device that reads as an endless stream of zeros safe
    /// to put here.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_the_kernel_will_not_admit_is_born_without_a_home() {
        let (root, mut pool) = a_tree_that_refuses_pane("born", &[1]);

        let admitted = pool.spawn(cmd(), "sh".to_string(), 80, 24).expect("pane 0");
        let refused_pane = pool
            .spawn(cmd(), "sh".to_string(), 80, 24)
            .expect("a refused cgroup join must not cost the person their pane");
        // The fixture spells its leaves by id, and ids are minted `0, 1, ...` from a fresh pool.
        // Asserted rather than assumed so a change to the mint fails HERE, loudly, instead of
        // silently aiming both panes at leaves the fixture never built.
        assert_eq!(
            (admitted.0, refused_pane.0),
            (0, 1),
            "the fixture's leaves are named for these ids",
        );

        assert!(
            matches!(
                pool.pane(admitted).expect("the admitted pane").home(),
                Landing::At(_),
            ),
            "a pane whose cgroup took it records where it is",
        );
        // ⚠ `Refused` AND NOT MERELY "no leaf". The weaker assertion — that the pane claims no
        // address — is the one R341 shipped, and it is satisfied by throwing the kernel's answer
        // away, which is what shipped with it. This is the same claim plus the reason, and the
        // reason is the whole of what a person on such a host can act on.
        assert_eq!(
            pool.pane(refused_pane).expect("the refused pane").home(),
            Landing::Refused(crate::Refusal::from_errno(REFUSED_ERRNO)),
            "a pane the kernel would not admit is in the DAEMON's cgroup, and must not claim a \
             leaf of its own — every later read and every later move would be aimed at it — but \
             it must say WHY, or a correct daemon reads as a broken one",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **And the RESTORE door answers the same way.**
    ///
    /// A separate test rather than two panes in the one above, because a door is what this project
    /// keeps getting wrong: R336 wired the join at one of five and said so, R337 found the restore
    /// among the three that never filled a pane's home, and `PaneRebirth::hooks` carries that
    /// history in its own doc. A restored pane joins a NEW cgroup exactly as a fresh one does and
    /// can be refused exactly as one can, so the two doors are one claim asserted twice — which is
    /// the only arrangement in which deleting the answer from one of them fails.
    ///
    /// It also gates the arm the spawn door cannot: a restore is handed its id, so this one names
    /// the refusing leaf outright instead of relying on what a fresh pool's counter mints.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_restored_pane_the_kernel_will_not_admit_comes_back_without_a_home() {
        let (root, mut pool) = a_tree_that_refuses_pane("restored", &[7]);

        pool.spawn_restored(PaneRebirth {
            id: PaneId(7),
            command: cmd(),
            label: "sh".to_string(),
            replacement_argv: Vec::new(),
            start_dir: None,
            size: (80, 24),
            hooks: PaneBirthHooks::default(),
            history: Vec::new(),
        })
        .expect("a refused cgroup join must not cost a person their restored pane");

        assert_eq!(
            pool.pane(PaneId(7)).expect("the restored pane").home(),
            Landing::Refused(crate::Refusal::from_errno(REFUSED_ERRNO)),
            "a restored pane the kernel would not admit must not claim a leaf either, and must \
             carry the same reason the spawn door does",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the kernel answers a write to the leaf [`a_tree_that_refuses_pane`] refuses with.
    ///
    /// `ENOSPC`, because that leaf is `/dev/full` and that is what the device is for. MEASURED
    /// rather than looked up (`write` to `/dev/full` → `Some(28)`, against a control write to an
    /// ordinary file that returned `Ok`), and pinned rather than matched loosely: a gate that
    /// accepted any refusal at all would stay green if the errno were dropped on the way through,
    /// which is the one thing the arm carrying it exists to prevent.
    ///
    /// It is NOT the errno a real refusing host gives — that is `EACCES`, from cgroup v2's
    /// delegation containment rule. The two differ on purpose: what is being gated here is that the
    /// KERNEL'S OWN number arrives intact, and a fixture that could only produce the number the
    /// product hard-codes would not be able to tell.
    #[cfg(target_os = "linux")]
    const REFUSED_ERRNO: i32 = 28;

    /// A stand-in delegated root under `tag`, and a pool placing into it, where the leaf of every
    /// pane id in `refusing` opens for writing and REFUSES every write.
    ///
    /// Every level is built in advance because a plain directory is not a cgroup: nothing here
    /// creates `cgroup.procs`, so a leaf left to `place` could not be joined by ANY pane and the
    /// admitted and refused arms would stop differing. Ids `0..=8` get a leaf, which is more than
    /// any caller needs and keeps the fixture from depending on where a pool's counter starts.
    ///
    /// `/dev/full` is the refusal, for `tests/pane_join_refused.rs`'s reason: it opens for writing
    /// and fails every write, on every Linux, with no cgroup tree and no privileges. Nothing on the
    /// birth path READS a leaf's `cgroup.procs` — `sweep` only `remove_dir`s and `place` only
    /// writes weights — which is what makes a device that reads as an endless stream of zeros safe
    /// to put here.
    #[cfg(target_os = "linux")]
    fn a_tree_that_refuses_pane(tag: &str, refusing: &[u64]) -> (std::path::PathBuf, Workspace) {
        let root = std::env::temp_dir().join(format!("sprag-refused-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cgroup = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            std::fs::write(path.join("cgroup.procs"), "").expect("fixture procs");
            std::fs::write(path.join("cgroup.subtree_control"), "").expect("fixture subtree");
            std::fs::write(path.join("cgroup.controllers"), "cpu memory pids\n")
                .expect("fixture controllers");
            std::fs::write(path.join("cpu.weight"), "100\n").expect("fixture weight");
            path
        };
        cgroup("");
        cgroup("session-1");
        cgroup("session-1/window-1");
        for id in 0..=8 {
            let leaf = cgroup(&format!("session-1/window-1/pane-{id}"));
            if refusing.contains(&id) {
                std::fs::remove_file(leaf.join("cgroup.procs")).expect("replace the fixture procs");
                std::os::unix::fs::symlink("/dev/full", leaf.join("cgroup.procs"))
                    .expect("a write that always fails");
            }
        }

        let mut pool = Workspace::new((80, 24));
        pool.set_home(PoolLineage {
            session: crate::registry::SessionId(1),
            window: crate::registry::WindowId(1),
        });
        pool.set_pane_homes(Arc::new(crate::share::PaneHomes::over(
            crate::share::Tree::adopt(root.clone()).expect("adopt a plain directory"),
        )));
        (root, pool)
    }

    /// Asserted by the FILESYSTEM: the destination tree is a real directory, and the claim is that
    /// nothing appears in it.
    #[test]
    fn a_pane_that_was_never_placed_is_not_relocated_when_it_moves() {
        // A stand-in for a delegated root: real directories, with the interface files the KERNEL
        // would have made (`share`'s own `FakeCgroupFs`, which is private to that module).
        //
        // The destination's interior levels are pre-made, and that is what makes this a gate rather
        // than a fixture accident: without them `place` fails at `session-1` and NOTHING appears
        // under the root whatever the code does, so the assertion below would hold for a pane that
        // was wrongly relocated. Measured — the mutation was GREEN until these two lines existed.
        let root = std::env::temp_dir().join(format!("sprag-unplaced-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cgroup = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            std::fs::write(path.join("cgroup.procs"), "").expect("fixture procs");
            std::fs::write(path.join("cgroup.subtree_control"), "").expect("fixture subtree");
            // What the parent enabled here, which is what a level may enable below it. Present on
            // every real cgroup; a fixture that omits it is a directory no placement can read.
            std::fs::write(path.join("cgroup.controllers"), "cpu memory pids\n")
                .expect("fixture controllers");
            std::fs::write(path.join("cpu.weight"), "100\n").expect("fixture weight");
        };
        cgroup("");
        cgroup("session-1");
        cgroup("session-1/window-2");

        // Born into a pool that has a window and NO tree — the GUI's in-process host, and every
        // host on a machine that cannot enforce a share.
        let mut origin = Workspace::new((80, 24));
        origin.set_home(PoolLineage {
            session: crate::registry::SessionId(1),
            window: crate::registry::WindowId(1),
        });
        let id = origin.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let taken = origin.close(id).expect("the pane leaves its pool");

        // ...and moved into one that does.
        let mut destination = origin.sibling();
        destination.set_home(PoolLineage {
            session: crate::registry::SessionId(1),
            window: crate::registry::WindowId(2),
        });
        destination.set_pane_homes(Arc::new(crate::share::PaneHomes::over(
            crate::share::Tree::adopt(root.clone()).expect("adopt a plain directory"),
        )));

        destination.adopt(taken);

        assert!(
            !root
                .join(format!("session-1/window-2/pane-{}", id.0))
                .exists(),
            "a pane that was never in a cgroup was given one by moving",
        );
        // ⚠ AND WHAT THE PANE SAYS AFTERWARDS, which the filesystem assertion above cannot see.
        // `adopt` used to write the destination onto every arriving pane, including one it had
        // just declined to relocate — so this pane claimed a leaf that the line above proves was
        // never created, and the next reading of it would have gone looking there. Measured: with
        // the destination assigned unconditionally the filesystem claim stays GREEN and only this
        // one moves, which is why both are here.
        assert_eq!(
            destination.pane(id).expect("the moved pane").home(),
            Landing::Unplaced,
            "a pane nothing was opened for is still a pane nothing was opened for after a move",
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A pane that IS relocated records the leaf it was moved INTO** — the projection's whole
    /// point, and the arm the round's debt sweep found ungated by the ENTIRE suite.
    ///
    /// ⚠ Not a defect this round introduced: the shipped line was `pane.home = arriving`, which is
    /// the same value by a different route, and replacing it with an absence left all 52 binaries
    /// GREEN (measured). So the one arm of `adopt` that actually moves a pane's processes had no
    /// assertion anywhere that the pane then says where they went — while the two arms that move
    /// nothing acquired one each this round. That asymmetry is what the sweep is for.
    ///
    /// What it would cost: `home` is what a LATER `adopt` migrates out of and what every resource
    /// reading is taken through, so a pane that forgot its new leaf would be measured at the old
    /// one — reporting a stranger's numbers under this pane's id, or a `Gone` where the pane is
    /// running fine — and the next move would migrate out of an empty source.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_that_moves_records_the_leaf_it_moved_into() {
        let (root, mut origin) = a_tree_that_refuses_pane("relocated", &[]);
        // The DESTINATION window's levels, which the fixture only builds for window 1.
        for relative in ["session-1/window-2", "session-1/window-2/pane-0"] {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            for (file, body) in [
                ("cgroup.procs", ""),
                ("cgroup.subtree_control", ""),
                ("cgroup.controllers", "cpu memory pids\n"),
                ("cpu.weight", "100\n"),
            ] {
                std::fs::write(path.join(file), body).expect("fixture file");
            }
        }

        let id = origin.spawn(cmd(), "sh".to_string(), 80, 24).expect("pane");
        let born = PoolLineage {
            session: crate::registry::SessionId(1),
            window: crate::registry::WindowId(1),
        }
        .pane(id);
        assert_eq!(
            origin.pane(id).expect("the pane").home(),
            Landing::At(born),
            "it starts in its birth window's leaf, or the move below proves nothing",
        );

        let taken = origin.close(id).expect("the pane leaves its pool");
        let mut destination = origin.sibling();
        let moved_to = PoolLineage {
            session: crate::registry::SessionId(1),
            window: crate::registry::WindowId(2),
        };
        destination.set_home(moved_to);
        destination.adopt(taken);

        assert_eq!(
            destination.pane(id).expect("the moved pane").home(),
            Landing::At(moved_to.pane(id)),
            "a pane that was relocated says the leaf it is in NOW; the old one is not where its \
             processes are, and every read and every later move goes through this answer",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **And a PLACED pane moving into a pool with no window of its own keeps its old leaf.**
    ///
    /// The third arm of `adopt`'s match, and the one the round's own debt sweep found untested —
    /// measured, by making it answer `Unplaced` and watching every test stay green.
    ///
    /// Nothing is relocated here because there is nowhere to relocate TO, so the pane's processes
    /// are still in the leaf they were born into and saying otherwise would be a lie in the
    /// direction this whole type exists to prevent: `home` is where the processes ARE. The shipped
    /// code assigned the destination unconditionally, so this pane answered `None` — an absence
    /// that would have made a later `break-pane` migrate out of nothing while a full leaf sat
    /// under the old window.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_placed_pane_moving_nowhere_keeps_the_leaf_it_is_in() {
        let (root, mut origin) = a_tree_that_refuses_pane("homeless", &[]);
        let id = origin.spawn(cmd(), "sh".to_string(), 80, 24).expect("pane");
        let Landing::At(born) = origin.pane(id).expect("the pane").home() else {
            panic!("the fixture admits this pane, or the move below proves nothing");
        };

        let taken = origin.close(id).expect("the pane leaves its pool");
        // A destination pool with NO window of its own — `sibling` does not inherit one.
        let mut destination = origin.sibling();
        assert!(
            destination.home().is_none(),
            "the destination has no window, which is the state this test is about",
        );
        destination.adopt(taken);

        assert_eq!(
            destination.pane(id).expect("the moved pane").home(),
            Landing::At(born),
            "nothing moved, so the pane is still where it was — an absence here would aim every \
             later read and every later move at a leaf that is not the one holding its processes",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// **And a REFUSED pane keeps the kernel's reason across a move.**
    ///
    /// The same claim as the test above with the other absence, and it is not the same test: the
    /// two arrive at `adopt` through different arms and the reason is what distinguishes them. A
    /// move that flattened a refusal into *unplaced* would lose the one fact a person on such a
    /// host can act on, at the moment they are most likely to be acting — pulling a pane into its
    /// own window is what somebody does when they are trying to contain it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_the_kernel_refused_still_says_so_after_it_moves() {
        let (root, mut origin) = a_tree_that_refuses_pane("adopted", &[0]);
        let id = origin.spawn(cmd(), "sh".to_string(), 80, 24).expect("pane");
        assert_eq!(
            origin.pane(id).expect("the refused pane").home(),
            Landing::Refused(crate::Refusal::from_errno(REFUSED_ERRNO)),
            "the fixture refused this pane's leaf, or the move below proves nothing",
        );

        let taken = origin.close(id).expect("the pane leaves its pool");
        let mut destination = origin.sibling();
        destination.set_home(PoolLineage {
            session: crate::registry::SessionId(1),
            window: crate::registry::WindowId(2),
        });
        destination.adopt(taken);

        assert_eq!(
            destination.pane(id).expect("the moved pane").home(),
            Landing::Refused(crate::Refusal::from_errno(REFUSED_ERRNO)),
            "a move is not an admission: the kernel refused this child and nothing since has \
             asked it again",
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pane_is_born_with_the_limit_its_pool_names() {
        // The whole seam in one assertion: the pool is asked at BIRTH, so the pane the user gets
        // carries the setting rather than the emulator's default.
        let mut ws = Workspace::new((80, 24));
        ws.set_history_limit_source(Arc::new(|| 4_242));
        let id = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(ws.pane(id).unwrap().pty().history_limit(), 4_242);
    }

    #[test]
    fn the_source_is_read_per_birth_not_captured_once() {
        // `default-command`'s rule: a user editing their config gets it on the NEXT pane, with
        // nothing restarted. A pool that cached the first answer would need a new daemon instead,
        // and that difference is invisible to a test that only ever spawns one pane.
        let answers = Arc::new(Mutex::new(vec![100_usize, 7]));
        let mut ws = Workspace::new((80, 24));
        let queue = Arc::clone(&answers);
        ws.set_history_limit_source(Arc::new(move || queue.lock().unwrap().remove(0)));
        let first = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let second = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(ws.pane(first).unwrap().pty().history_limit(), 100);
        assert_eq!(
            ws.pane(second).unwrap().pty().history_limit(),
            7,
            "the second birth asked again rather than reusing the first answer",
        );
    }

    #[test]
    fn a_sibling_pool_inherits_the_limit_source() {
        // A new WINDOW is not a new configuration. `sibling` shares the id counter for the same
        // reason, and a sibling that defaulted here would give a session's second window shallower
        // history than its first with nothing to explain it.
        let mut ws = Workspace::new((80, 24));
        ws.set_history_limit_source(Arc::new(|| 321));
        let mut next = ws.sibling();
        let id = next.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(next.pane(id).unwrap().pty().history_limit(), 321);
    }

    /// A pane's command that PRINTS one environment variable and exits — the only way to assert
    /// what a child actually received, as opposed to what the builder was told.
    fn echoes(var: &str) -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg(format!("printf %s \"${{{var}-unset}}\""));
        c.env("TERM", "dumb");
        c
    }

    /// What the child printed on its first row, once it has exited.
    fn printed(ws: &Workspace, id: PaneId) -> String {
        let pty = ws.pane(id).unwrap().pty();
        let start = std::time::Instant::now();
        while !pty.is_eof() && start.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        pty.with_screen(|screen| {
            (0..screen.cols())
                .filter_map(|col| screen.cell(col, 0).map(|cell| cell.cluster.to_string()))
                .collect::<String>()
        })
        .trim_end()
        .to_owned()
    }

    #[test]
    fn a_pane_s_child_is_born_knowing_which_pane_it_is_in() {
        // The whole seam, proven from the CHILD's side: the pool is asked at each birth and what it
        // answers reaches the process. Read twice with the input changed — each pane prints its OWN
        // id, so a source consulted once and reused (or one handed the wrong id) fails the second
        // assertion while passing the first.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_env_source(Arc::new(|id: PaneId| {
            vec![("WS_TEST_PANE".to_owned(), id.0.to_string())]
        }));
        let first = ws
            .spawn(echoes("WS_TEST_PANE"), "sh".to_string(), 20, 4)
            .unwrap();
        let second = ws
            .spawn(echoes("WS_TEST_PANE"), "sh".to_string(), 20, 4)
            .unwrap();
        assert_eq!(printed(&ws, first), first.0.to_string());
        assert_eq!(
            printed(&ws, second),
            second.0.to_string(),
            "each pane's child is told its own id, not the first pane's",
        );
    }

    #[test]
    fn a_pool_with_no_source_publishes_nothing() {
        // The default is not "some empty value" but ABSENCE, which is what lets a child conclude
        // there is no daemon to report to. A standalone pool and every unit test above rely on it.
        let mut ws = Workspace::new((80, 24));
        let id = ws
            .spawn(echoes("WS_TEST_PANE"), "sh".to_string(), 20, 4)
            .unwrap();
        assert_eq!(printed(&ws, id), "unset");
    }

    #[test]
    fn a_sibling_pool_inherits_the_pane_env_source() {
        // A new WINDOW is not a new configuration, exactly as with the history limit. This is also
        // what makes a session created AFTER boot work at all: `SessionRegistry::new_session` builds
        // its first pool as a sibling of the default session's.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_env_source(Arc::new(|id: PaneId| {
            vec![("WS_TEST_PANE".to_owned(), format!("sib{}", id.0))]
        }));
        let mut next = ws.sibling();
        let id = next
            .spawn(echoes("WS_TEST_PANE"), "sh".to_string(), 20, 4)
            .unwrap();
        assert_eq!(printed(&next, id), format!("sib{}", id.0));
    }

    #[test]
    fn a_restored_pane_s_child_is_told_too() {
        // A restore is the other birth site, and a pane that came back unable to name itself would
        // be a gap visible only after a reboot. Its id is the CALLER's, so this also pins that the
        // publication uses the id the pane is coming back under.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_env_source(Arc::new(|id: PaneId| {
            vec![("WS_TEST_PANE".to_owned(), format!("re{}", id.0))]
        }));
        ws.spawn_restored(PaneRebirth {
            id: PaneId(41),
            command: echoes("WS_TEST_PANE"),
            label: "sh".to_owned(),
            replacement_argv: Vec::new(),
            start_dir: None,
            size: (20, 4),
            hooks: PaneBirthHooks::default(),
            history: Vec::new(),
        })
        .unwrap();
        assert_eq!(printed(&ws, PaneId(41)), "re41");
    }

    /// A child that prints the arguments it was launched with, beyond the ones this fixture wrote.
    ///
    /// `sh -c <script> <a> <b>` puts `a` in `$0` and `b` in `$1`, so an APPENDED argument is exactly
    /// what this reads back — proven from inside the process rather than from the builder, which is
    /// what makes it a claim about the launch and not about a struct. With nothing appended `$0` is
    /// the shell's own name and `$1` is unset, which is why an un-instrumented launch reads
    /// `/bin/sh/none` rather than anything symmetrical.
    fn echoes_extra_args() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("printf %s \"${0-none}/${1-none}\"");
        c.env("TERM", "dumb");
        c
    }

    /// The instrumentation source, answering only for the argv it recognises — the shape
    /// `sprag-host`'s own installs, in miniature.
    fn instruments(program: &'static str) -> PaneArgsSource {
        Arc::new(move |argv: &[String]| {
            if argv.first().is_some_and(|first| first.ends_with(program)) {
                vec!["--settings".to_owned(), "DOC".to_owned()]
            } else {
                Vec::new()
            }
        })
    }

    #[test]
    fn a_pane_s_child_is_launched_with_what_the_source_added() {
        // The whole seam, proven from the CHILD's side: the pool is asked at each birth with the
        // argv it is about to run, and what it answers reaches the process as arguments.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_args_source(instruments("sh"));
        let id = ws
            .spawn(echoes_extra_args(), "sh".to_string(), 40, 4)
            .unwrap();
        assert_eq!(printed(&ws, id), "--settings/DOC");
    }

    #[test]
    fn a_pool_with_no_args_source_launches_the_argv_as_written() {
        // The default is ABSENCE, so every pane in a GUI's in-process host and every unit test above
        // runs exactly the command its caller wrote.
        let mut ws = Workspace::new((80, 24));
        let id = ws
            .spawn(echoes_extra_args(), "sh".to_string(), 40, 4)
            .unwrap();
        assert_eq!(printed(&ws, id), "/bin/sh/none");
    }

    #[test]
    fn a_source_that_does_not_recognise_the_program_adds_nothing() {
        // The answer for nearly every pane ever opened. Read twice with the input changed against
        // the test above: the same pool, the same child, and a source looking for something else.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_args_source(instruments("claude"));
        let id = ws
            .spawn(echoes_extra_args(), "sh".to_string(), 40, 4)
            .unwrap();
        assert_eq!(printed(&ws, id), "/bin/sh/none");
    }

    #[test]
    fn a_sibling_pool_inherits_the_pane_args_source() {
        // A new WINDOW is not a new configuration: without this, which window a person happened to
        // open an agent in would decide whether that agent can report.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_args_source(instruments("sh"));
        let mut next = ws.sibling();
        let id = next
            .spawn(echoes_extra_args(), "sh".to_string(), 40, 4)
            .unwrap();
        assert_eq!(printed(&next, id), "--settings/DOC");
    }

    #[test]
    fn a_restored_pane_is_instrumented_by_the_daemon_restoring_it() {
        // The other birth site. An agent brought back after a reboot would otherwise be the one
        // agent in the daemon that cannot report — a gap visible only after a reboot, which is the
        // shape `PaneEnvSource` met at exactly this door.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_args_source(instruments("sh"));
        ws.spawn_restored(PaneRebirth {
            id: PaneId(41),
            command: echoes_extra_args(),
            label: "sh".to_owned(),
            replacement_argv: Vec::new(),
            start_dir: None,
            size: (40, 4),
            hooks: PaneBirthHooks::default(),
            history: Vec::new(),
        })
        .unwrap();
        assert_eq!(printed(&ws, PaneId(41)), "--settings/DOC");
    }

    #[test]
    fn what_a_snapshot_records_is_the_argv_its_caller_wrote() {
        // THE ORDERING THAT MATTERS, and the one a later edit could quietly lose. The recorded argv
        // is what a restore re-runs, and the instrumentation names the daemon that added it: a
        // snapshot that stored the instrumented argv would bring an agent back pointed at the
        // endpoint of a daemon that no longer exists. So the capture happens BEFORE the source is
        // asked, and the restore door above re-derives instead.
        let mut ws = Workspace::new((80, 24));
        ws.set_pane_args_source(instruments("sh"));
        let id = ws
            .spawn(echoes_extra_args(), "sh".to_string(), 40, 4)
            .unwrap();
        let argv = ws.pane(id).expect("the pane").argv().to_vec();
        assert_eq!(
            argv.len(),
            3,
            "the recorded argv is `/bin/sh -c <script>` and nothing sprag added: {argv:?}",
        );
        assert!(
            !argv.iter().any(|arg| arg == "--settings"),
            "a restore must not replay a dead daemon's instrumentation: {argv:?}",
        );
        // ...and the child really did get it, so this is not passing because the source was inert.
        assert_eq!(printed(&ws, id), "--settings/DOC");
    }

    #[test]
    fn a_pane_s_id_is_reserved_before_its_child_starts() {
        // The ordering the environment forces, asserted as the COST it has: the id is minted before
        // the spawn, so a birth that fails consumes it. Restoring the old "mint after a successful
        // spawn" makes the surviving pane id 0 and turns this red — which is the point, since
        // nothing else in the suite can see the difference.
        let mut ws = Workspace::new((80, 24));
        let mut doomed = CommandBuilder::new("/nonexistent/sprag-no-such-program");
        doomed.env("TERM", "dumb");
        assert!(
            ws.spawn(doomed, "doomed".to_string(), 20, 4).is_err(),
            "a program the OS cannot exec is a failed birth",
        );
        let id = ws.spawn(cmd(), "sh".to_string(), 20, 4).unwrap();
        assert_eq!(
            id,
            PaneId(1),
            "the failed birth had already taken 0; ids need uniqueness and monotonicity, not density",
        );
        assert_eq!(ws.panes().len(), 1, "and it added no pane");
    }

    #[test]
    fn spawn_assigns_monotonic_ids() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let b = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(a, PaneId(0));
        assert_eq!(b, PaneId(1));
        assert_eq!(ws.panes().len(), 2);
    }

    #[test]
    fn a_pane_claims_no_opener_until_one_is_recorded() {
        let mut ws = Workspace::new((80, 24));
        let opener = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let opened = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(
            ws.list().iter().filter_map(|p| p.opened_by).count(),
            0,
            "a plain spawn is a pane nobody claims — the default has to be the person's, not an \
             agent's, or every GUI split would read as agent-opened",
        );
        ws.set_pane_opened_by(opened, opener);
        assert_eq!(
            ws.pane(opened).unwrap().opened_by(),
            Some(opener),
            "the pane itself carries the provenance",
        );
        assert_eq!(
            ws.list()
                .iter()
                .find(|p| p.id == opened.0)
                .unwrap()
                .opened_by,
            Some(opener.0),
            "and it reaches the published view a client reads",
        );
        assert_eq!(
            ws.pane(opener).unwrap().opened_by(),
            None,
            "recording it on one pane does not stamp the opener itself",
        );
    }

    #[test]
    fn recording_an_opener_for_a_pane_this_pool_does_not_hold_changes_nothing() {
        // The pool is ONE window's membership authority, so it cannot answer whether a target is
        // absent or merely elsewhere — it just records nothing. The check belongs to whoever
        // accepts a provenance from a caller (the host's spawn/split actions).
        let mut ws = Workspace::new((80, 24));
        let live = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        ws.set_pane_opened_by(PaneId(999), live);
        assert_eq!(
            ws.panes().len(),
            1,
            "no pane was invented for the absent id"
        );
        assert_eq!(ws.pane(live).unwrap().opened_by(), None);
    }

    #[test]
    fn a_pane_carries_no_name_until_somebody_gives_it_one() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(
            ws.list().iter().filter(|p| p.name.is_some()).count(),
            0,
            "a name is chosen, so no birth path may invent one",
        );
        assert!(
            ws.set_pane_name(a, Some(crate::PaneName::parse("build").unwrap())),
            "naming a pane this pool holds reports that it landed",
        );
        assert_eq!(
            ws.pane(a).unwrap().name().map(crate::PaneName::as_str),
            Some("build"),
            "the pane itself carries the name",
        );
        assert_eq!(
            ws.list().iter().find(|p| p.id == a.0).unwrap().name,
            Some(crate::PaneName::parse("build").unwrap()),
            "and it reaches the published view a client reads",
        );
        assert!(ws.set_pane_name(a, None), "and a name can be taken away");
        assert_eq!(ws.pane(a).unwrap().name(), None);
    }

    #[test]
    fn naming_a_pane_this_pool_does_not_hold_says_so_rather_than_doing_nothing() {
        // The companion of `recording_an_opener_for_a_pane_this_pool_does_not_hold_changes_nothing`,
        // and deliberately NOT the same shape: a provenance stamp's caller has just spawned the
        // pane it is stamping and cannot miss, where a rename's caller named a pane that may have
        // closed a moment ago — two different outcomes it has to be able to tell apart.
        let mut ws = Workspace::new((80, 24));
        let live = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert!(
            !ws.set_pane_name(PaneId(999), Some(crate::PaneName::parse("build").unwrap())),
            "an absent pane is reported, not silently ignored",
        );
        assert_eq!(ws.panes().len(), 1, "and no pane was invented for it");
        assert_eq!(ws.pane(live).unwrap().name(), None);
    }

    #[test]
    fn one_pool_will_take_a_name_twice_because_it_cannot_see_the_registry() {
        // Documented behaviour, not an oversight: uniqueness is REGISTRY-wide, and this pool is one
        // window's membership authority. The check belongs to whoever accepts a name from a caller.
        // If this ever starts failing, somebody has moved the check to the wrong layer.
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let b = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let name = crate::PaneName::parse("build").unwrap();
        assert!(ws.set_pane_name(a, Some(name.clone())));
        assert!(ws.set_pane_name(b, Some(name)));
        assert_eq!(
            ws.list().iter().filter(|p| p.name.is_some()).count(),
            2,
            "the pool records both; the host is what refuses the second",
        );
    }

    /// A seat's declarations reach its replacement — and the NAME is the one that moves, asserted
    /// while BOTH panes are still live, which is the only moment at which the difference between
    /// moving and copying exists at all. `PaneLifecycle::respawn` closes the outgoing pane
    /// immediately afterwards, so a copy would read as a move to every gate that looks later.
    #[test]
    fn a_seat_handed_over_moves_its_name_and_copies_the_rest() {
        let mut ws = Workspace::new((80, 24));
        let opener = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let leaving = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let arriving = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let name = crate::PaneName::parse("inner").unwrap();
        let grant = crate::share::Grant {
            share: crate::share::Share::new(4242).unwrap(),
            limits: crate::share::Limits::UNCAPPED.with_memory(Some(512 << 20)),
        };
        let remote = SshRemote {
            user: None,
            host: "pc4".to_string(),
            port: None,
        };
        assert!(ws.set_pane_name(leaving, Some(name.clone())));
        ws.set_pane_opened_by(leaving, opener);
        ws.set_pane_remote(leaving, remote.clone());
        assert!(ws.set_pane_grant(leaving, grant).is_some());

        assert!(ws.hand_seat_over(leaving, arriving), "both panes are here");

        let took = ws.pane(arriving).unwrap();
        assert_eq!(took.name(), Some(&name), "the name reaches the replacement");
        assert_eq!(took.opened_by(), Some(opener), "and the provenance");
        assert_eq!(took.remote(), Some(&remote), "and the remote marker");
        assert_eq!(
            ws.pane_grant_or_default(arriving),
            Some(grant),
            "and the grant, which is APPLIED and not merely recorded",
        );

        let vacated = ws.pane(leaving).unwrap();
        assert_eq!(
            vacated.name(),
            None,
            "⚠⚠⚠ AND THE OUTGOING PANE NO LONGER HOLDS IT: a name is unique daemon-wide, so a copy \
             leaves a window in which a lookup may reach the pane that is about to be closed",
        );
        assert_eq!(
            vacated.opened_by(),
            Some(opener),
            "⚠ while provenance is not unique and nothing is confused by both answering it — a \
             hand-over that cleared it would be inventing a second rule for no reason",
        );
    }

    /// The refusals, and the sharp one is the second: a hand-over that could not land must not have
    /// taken the name off the pane that is staying. That is the arm a caller meets when the
    /// replacement failed to spawn.
    #[test]
    fn a_seat_is_not_handed_to_or_from_a_pane_this_pool_does_not_hold() {
        let mut ws = Workspace::new((80, 24));
        let live = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let name = crate::PaneName::parse("inner").unwrap();
        assert!(ws.set_pane_name(live, Some(name.clone())));

        assert!(
            !ws.hand_seat_over(PaneId(4242), live),
            "a seat nobody holds cannot be handed over",
        );
        assert_eq!(
            ws.pane(live).unwrap().name(),
            Some(&name),
            "and that refusal does not touch the pane it was aimed at",
        );

        assert!(
            !ws.hand_seat_over(live, PaneId(4242)),
            "nor can one be handed to a pane this pool does not hold",
        );
        assert_eq!(
            ws.pane(live).unwrap().name(),
            Some(&name),
            "⚠⚠⚠⚠ AND THE NAME IS STILL THERE — a replacement that failed to spawn leaves the run \
             holding the pane it had, and an unnamed one is not what it was holding",
        );
    }

    /// The control: a seat nobody declared anything about hands over nothing, and the replacement is
    /// not given a name, an opener or a ceiling it never had.
    #[test]
    fn an_undeclared_seat_hands_over_nothing() {
        let mut ws = Workspace::new((80, 24));
        let leaving = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let arriving = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let machine = ws.pane_grant_or_default(arriving);

        assert!(ws.hand_seat_over(leaving, arriving), "both panes are here");

        let took = ws.pane(arriving).unwrap();
        assert_eq!(took.name(), None, "no name is invented");
        assert_eq!(took.opened_by(), None, "and no provenance");
        assert_eq!(took.remote(), None, "and no remote endpoint");
        assert_eq!(
            ws.pane_grant_or_default(arriving),
            machine,
            "and it keeps following the machine's default rather than being pinned to a copy of it",
        );
    }

    #[test]
    fn close_removes_and_ids_are_not_reused() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        let _b = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert!(ws.close(a).is_some());
        assert!(ws.close(a).is_none()); // already gone
        assert!(ws.pane(a).is_none());
        // The freed id is not reclaimed by the next spawn.
        let c = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert_eq!(c, PaneId(2));
    }

    #[test]
    fn spawn_with_id_uses_the_given_id_and_reserves_it_against_reuse() {
        let mut ws = Workspace::new((80, 24));
        // Restore two panes OUT of monotonic order, leaving a gap at the top (id 5 is the
        // high-water mark; 3 and 4 were minted then closed pre-reboot and did not come back).
        ws.spawn_restored(PaneRebirth {
            id: PaneId(5),
            command: cmd(),
            label: "sh".into(),
            replacement_argv: Vec::new(),
            start_dir: None,
            size: (80, 24),
            hooks: PaneBirthHooks::default(),
            history: Vec::new(),
        })
        .unwrap();
        ws.spawn_restored(PaneRebirth {
            id: PaneId(1),
            command: cmd(),
            label: "sh".into(),
            replacement_argv: Vec::new(),
            start_dir: None,
            size: (80, 24),
            hooks: PaneBirthHooks::default(),
            history: Vec::new(),
        })
        .unwrap();
        assert!(ws.pane(PaneId(5)).is_some());
        assert!(ws.pane(PaneId(1)).is_some());
        // A fresh mint goes ABOVE the highest reserved id — it never reissues 5.
        let next = ws.spawn(cmd(), "sh".into(), 80, 24).unwrap();
        assert_eq!(
            next,
            PaneId(6),
            "the counter was reserved above the restored ids"
        );
    }

    #[test]
    fn a_seeded_counter_starts_minting_at_the_seed() {
        // A restore seeds the counter to the pre-reboot high-water mark, so a retired id whose
        // pane did NOT come back (a gap at the very top) is still never reissued — deriving the
        // counter from the restored panes alone could not know it existed.
        let mut ws = Workspace::with_seeded_counter((80, 24), 6);
        assert_eq!(ws.spawn(cmd(), "sh".into(), 80, 24).unwrap(), PaneId(6));
        assert_eq!(ws.spawn(cmd(), "sh".into(), 80, 24).unwrap(), PaneId(7));
    }

    #[test]
    fn resize_updates_dimensions() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        // The emulator resizes synchronously (only the PTY ioctl is debounced),
        // so `dimensions()` is current immediately after `resize`.
        assert!(ws.resize(a, 100, 30, (0, 0)).unwrap());
        assert_eq!(ws.pane(a).unwrap().pty().dimensions(), (100, 30));
        assert!(!ws.resize(PaneId(999), 10, 10, (0, 0)).unwrap());
        // Through a SHARED &Workspace — the path the GUI reflow Effect uses via
        // an Rc; resize needs no &mut now that the pty is interior-mutable.
        let shared: &Workspace = &ws;
        assert!(shared.resize(a, 64, 20, (0, 0)).unwrap());
        assert_eq!(ws.pane(a).unwrap().pty().dimensions(), (64, 20));
    }

    #[test]
    fn resize_threads_the_cell_pixel_geometry_to_the_pane() {
        let mut ws = Workspace::new((80, 24));
        let a = ws.spawn(cmd(), "sh".to_string(), 80, 24).unwrap();
        assert!(ws.resize(a, 100, 30, (9, 18)).unwrap());
        assert_eq!(
            ws.pane(a).unwrap().pty().cell_pixel_size(),
            (9, 18),
            "the display cell metric reaches the pane's emulator"
        );
    }

    #[test]
    fn list_reports_metadata() {
        let mut ws = Workspace::new((80, 24));
        ws.spawn(cmd(), "alpha".to_string(), 40, 12).unwrap();
        let info = ws.list();
        assert_eq!(info.len(), 1);
        assert_eq!(info[0].id, 0);
        assert_eq!((info[0].cols, info[0].rows), (40, 12));
        assert_eq!(info[0].command_label, "alpha");
    }
}
