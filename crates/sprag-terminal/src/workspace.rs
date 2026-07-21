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

use sprag_vt::{ClipboardQuery, ClipboardWrite, Image, Notification, ShellState};

use crate::pane_pty::{CommandBuilder, PanePty, PanePtyError, PanePtyHandle};

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

/// Read-only metadata describing a pane, for introspection over the
/// scene-as-data control surface (the host maps this to JSON).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneInfo {
    pub id: u64,
    pub cols: u16,
    pub rows: u16,
    pub command_label: String,
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
    /// The pane's shell-integration state (OSC 133), `Unknown` without integration. Derived from
    /// the screen's prompt marks — the "idle at a prompt vs running a command" summary.
    pub shell_state: ShellState,
    /// The last finished command's exit status (OSC 133 `D`), `None` when none has finished with a
    /// reported status. Pair with [`Self::shell_state`] to tell "no command ran" from "unreported".
    pub last_exit_status: Option<i32>,
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
        }
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
        }
    }

    /// Spawn `command` on a fresh `cols x rows` pane, returning its id.
    /// `label` is the introspection label (typically the program name).
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be
    /// started; on failure no pane is added and the id is not consumed.
    pub fn spawn(
        &mut self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
    ) -> Result<PaneId, PanePtyError> {
        self.spawn_with_dirty(command, label, cols, rows, None, None)
    }

    /// [`Self::spawn`] with the pane's two PTY-reader callbacks (threaded to
    /// [`PanePty::spawn_with_dirty`]): `on_dirty` (per output batch + at exit) and
    /// `on_exit` (once, at the child's exit).
    ///
    /// A windowed host passes `on_dirty = Some(Box::new(move || sink.request_repaint()))`
    /// (the pinion R999 `RepaintSink` seam) so this pane's output wakes the shell to
    /// repaint; the headless host passes `bump_on_dirty`. `on_exit` is the "this child is
    /// gone" event the host lifetime turns on (the daemon exits when its last live pane
    /// does). Both are pinion-free (`Box<dyn Fn() + Send>`), keeping this crate decoupled
    /// from the GUI shell and the host lifetime; callers with neither use [`Self::spawn`].
    ///
    /// # Errors
    ///
    /// Returns [`PanePtyError`] if the pseudoterminal or child cannot be
    /// started; on failure no pane is added and the id is not consumed.
    pub fn spawn_with_dirty(
        &mut self,
        command: CommandBuilder,
        label: String,
        cols: u16,
        rows: u16,
        on_dirty: Option<Box<dyn Fn() + Send>>,
        on_exit: Option<Box<dyn Fn() + Send>>,
    ) -> Result<PaneId, PanePtyError> {
        // Capture the launch argv BEFORE the builder is moved into the spawn, so a snapshot can
        // later re-run it (an allowlisted program) or fall back to a shell.
        let argv = argv_of(&command);
        let pty = PanePty::spawn_with_dirty(command, cols, rows, on_dirty, on_exit)?;
        // Mint AFTER a successful spawn so a failed spawn consumes no id (preserving the
        // old counter's gap-free-on-failure behaviour). Relaxed ordering: ids need only
        // uniqueness + monotonicity, not synchronization with other memory.
        let id = PaneId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.panes.push(Pane {
            id,
            pty,
            command_label: label,
            argv,
        });
        Ok(id)
    }

    /// Spawn `command` on a fresh `cols x rows` pane at a CALLER-GIVEN `id`, rather than a
    /// freshly minted one — the restore primitive.
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
    pub fn spawn_with_dirty_id(
        &mut self,
        id: PaneId,
        command: CommandBuilder,
        label: String,
        size: (u16, u16),
        on_dirty: Option<Box<dyn Fn() + Send>>,
        on_exit: Option<Box<dyn Fn() + Send>>,
    ) -> Result<(), PanePtyError> {
        let (cols, rows) = size;
        let argv = argv_of(&command);
        let pty = PanePty::spawn_with_dirty(command, cols, rows, on_dirty, on_exit)?;
        // Reserve the id above the counter so a future mint cannot reissue it (saturating so a
        // pathological u64::MAX id cannot wrap the reservation back to 0). Relaxed matches the
        // mint path: ids need only uniqueness + monotonicity, not synchronization.
        self.next_id
            .fetch_max(id.0.saturating_add(1), Ordering::Relaxed);
        self.panes.push(Pane {
            id,
            pty,
            command_label: label,
            argv,
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
    pub fn resize(&self, id: PaneId, cols: u16, rows: u16) -> Result<bool, PanePtyError> {
        match self.panes.iter().find(|p| p.id == id) {
            Some(pane) => {
                pane.pty.resize(cols, rows)?;
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
                    title: p.title(),
                    notification,
                    notification_seq,
                    bell_seq: p.bell_seq(),
                    shell_state,
                    last_exit_status,
                    clipboard_write_seq: p.clipboard_write().1,
                    clipboard_query,
                    clipboard_query_seq,
                    images: p.images(),
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

    /// A long-lived child (`cat` reads stdin) so the pane's PTY stays open
    /// across resize/close assertions.
    fn cmd() -> CommandBuilder {
        let mut c = CommandBuilder::new("/bin/sh");
        c.arg("-c");
        c.arg("cat");
        c.env("TERM", "dumb");
        c
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
        ws.spawn_with_dirty_id(PaneId(5), cmd(), "sh".into(), (80, 24), None, None)
            .unwrap();
        ws.spawn_with_dirty_id(PaneId(1), cmd(), "sh".into(), (80, 24), None, None)
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
        assert!(ws.resize(a, 100, 30).unwrap());
        assert_eq!(ws.pane(a).unwrap().pty().dimensions(), (100, 30));
        assert!(!ws.resize(PaneId(999), 10, 10).unwrap());
        // Through a SHARED &Workspace — the path the GUI reflow Effect uses via
        // an Rc; resize needs no &mut now that the pty is interior-mutable.
        let shared: &Workspace = &ws;
        assert!(shared.resize(a, 64, 20).unwrap());
        assert_eq!(ws.pane(a).unwrap().pty().dimensions(), (64, 20));
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
