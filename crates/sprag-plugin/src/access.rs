//! `PaneAccess` — the plugin extension API.
//!
//! A plugin's whole view of the core: enumerate panes, read a pane's screen as
//! scene-as-data, and inject input — all addressed by [`PaneId`], never by
//! reaching into a `PanePtyHandle` or `Screen` directly. This is the single
//! read+inject path: every plugin (and any future control consumer) goes
//! through it, so reads and injections are consistent and the input-encoding
//! lives in one place.
//!
//! [`WorkspacePaneAccess`] is the production implementation over a shared
//! [`Workspace`]; it stays pinion-free (the producer/control layer).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use sprag_input::{Modifiers, encode};
use sprag_terminal::{
    Attention, CommandBuilder, Pane, PaneBirthHooks, PaneId, PanePtyHandle, RawOutput, Workspace,
};
use sprag_vt::Screen;

/// One screen row: its damage `generation` paired with its (trailing-trimmed)
/// text, read in a single locked snapshot so the two never tear.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaneRow {
    pub generation: u64,
    pub text: String,
}

/// A key to inject: a W3C `KeyboardEvent.key` string plus modifiers, encoded
/// to PTY bytes by [`PaneAccess::inject`] (the sprag-owned encoder, R2.6).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyStroke {
    pub key: String,
    pub mods: Modifiers,
}

impl KeyStroke {
    /// A single unmodified named key (e.g. `KeyStroke::named("Enter")`).
    #[must_use]
    pub fn named(key: &str) -> Self {
        Self {
            key: key.to_string(),
            mods: Modifiers::default(),
        }
    }

    /// Expand text into one unmodified character keystroke per `char`.
    #[must_use]
    pub fn text(s: &str) -> Vec<Self> {
        s.chars()
            .map(|ch| Self {
                key: ch.to_string(),
                mods: Modifiers::default(),
            })
            .collect()
    }
}

/// Why [`PaneAccess::inject`] failed — a typed cause, not a discarded error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneError {
    /// No pane has the given id.
    UnknownPane(PaneId),
    /// A keystroke had no PTY-byte encoding (the offending key).
    Encode(String),
    /// Writing the encoded bytes to the pane failed (the IO error message).
    Write(String),
    /// Spawning a pane failed: no [`PaneLifecycle`] support, an empty argv, or
    /// the pseudoterminal/child could not start (the cause message).
    Spawn(String),
}

/// The plugin extension API: a plugin's view of the core's panes.
pub trait PaneAccess {
    /// The ids of the live panes, in order.
    fn pane_ids(&self) -> Vec<PaneId>;

    /// The pane's collapsed screen text (each row trailing-trimmed, rows joined
    /// without separators) — the read for substring/sentinel matching across
    /// wrapped lines. `None` if no pane has that id.
    fn pane_collapsed(&self, id: PaneId) -> Option<String>;

    /// The pane's screen as per-row `(generation, text)`, read in one snapshot.
    /// `None` if no pane has that id.
    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>>;

    /// Whether the pane's child has closed its PTY (exited): no more output is
    /// coming and every byte it produced has already been applied to the
    /// screen. `None` if no pane has that id. This is the race-free completion
    /// signal a one-shot adapter (a tool that replies then exits) converges on.
    fn pane_eof(&self, id: PaneId) -> Option<bool>;

    /// The pane's full output text: scrolled-off lines (scrollback) then the
    /// visible rows, trailing blank lines stripped, joined by `"\n"`. `None` if
    /// no pane has that id. Unlike `pane_rows`/`pane_collapsed` (visible screen
    /// only), this captures output longer than the grid — a scrolled AI reply.
    fn pane_full_text(&self, id: PaneId) -> Option<String>;

    /// Inject `keys` into the pane, returning the number of PTY bytes written.
    ///
    /// # Errors
    ///
    /// [`PaneError`] when the pane is unknown, a key cannot be encoded, or
    /// the write fails.
    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<u64, PaneError>;

    /// The pane *lifecycle* surface (spawn/close), if this implementation
    /// supports it. `None` by default — read/inject plugins never need it, so
    /// they (and test doubles) pay nothing; a plugin that manages panes (e.g.
    /// an AI dialogue spawning one pane per turn) asks for it and fails cleanly
    /// when it is absent. Kept a separate sub-trait so [`PaneAccess`] stays the
    /// read/inject surface (interface segregation).
    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        None
    }

    /// The pane *raw-output capture* surface, if this implementation supports it.
    /// `None` by default — only a plugin that parses structured machine output (a
    /// `claude --output-format json` envelope the grid would corrupt) needs the
    /// source bytes, so read/inject plugins and test doubles pay nothing AND
    /// cannot reach raw bytes at all: the scene-as-data invariant ("a plugin
    /// reads structured screen data, never raw bytes") is then enforced by the
    /// type, not a doc comment. Mirrors [`lifecycle`](PaneAccess::lifecycle).
    fn raw_capture(&self) -> Option<&dyn PaneRawCapture> {
        None
    }
}

/// Pane *lifecycle* control: spawn and close panes. The capability a plugin
/// needs to orchestrate one-shot tools across turns (each turn a fresh pane).
/// Reached via [`PaneAccess::lifecycle`] so it does not fatten the read/inject
/// surface every plugin depends on.
pub trait PaneLifecycle {
    /// Spawn a pane running `argv` (`[program, args…]`) at `cols × rows`,
    /// returning its [`PaneId`].
    ///
    /// # Errors
    ///
    /// [`PaneError::Spawn`] when `argv` is empty or the pane cannot start.
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, PaneError>;

    /// Close (reap) the pane with `id`, returning whether it existed. The
    /// pane's blocking teardown runs outside any shared lock.
    fn close(&self, id: PaneId) -> bool;
}

/// Pane *raw-output* capture: the child's **source** bytes, before the emulator
/// renders them onto the grid. Reached via [`PaneAccess::raw_capture`].
///
/// Kept a separate sub-trait (like [`PaneLifecycle`]) so [`PaneAccess`] stays
/// the structured scene-as-data surface. The `pane_*_text` family returns the
/// *rendered grid* (wrapped to the pane width, trailing-trimmed, control-
/// stripped — a lossy projection for display); this returns the *exact bytes*
/// the child emitted, the read for **structured machine output** (a single-line
/// JSON envelope a long reply wraps across rows, which the grid's wrap-`\n`
/// insertion and trailing-trim would corrupt). Only a plugin that parses such
/// output asks for it, so the "structured data, never raw bytes" invariant is
/// enforced by the type rather than by convention.
pub trait PaneRawCapture {
    /// The pane child's raw output bytes (the source stream, before emulation),
    /// or `None` if no pane has that `id`. A truncated [`RawOutput`] is an
    /// incomplete capture, and a structured read should degrade.
    fn pane_raw_output(&self, id: PaneId) -> Option<RawOutput>;
}

/// A MINTER for one pane's attention hook: called per birth, answering a hook that pane owns.
///
/// A named type because the shape is genuinely three-deep (`Arc<dyn Fn() -> Box<dyn Fn(..)>>`) and
/// reads as noise inline — and because the field it fills and the builder
/// ([`WorkspacePaneAccess::with_attention`]) that sets it must say the same thing.
///
/// **Why a minter and not one shared closure**: the hook the daemon hands out owns a channel sender
/// per pane, and asking for it per birth is what keeps the PTY reader thread that runs it from taking
/// a lock. This layer expresses that as *"give me a hook"* without knowing why — the same opaque
/// discipline the pane-exit death signal follows.
pub type AttentionMinter = Arc<dyn Fn() -> Box<dyn Fn(PaneId, Attention) + Send> + Send + Sync>;

/// [`PaneAccess`] over a shared [`Workspace`] — the production implementation.
pub struct WorkspacePaneAccess {
    workspace: Arc<Mutex<Workspace>>,
    /// An OPAQUE hook run once when a pane this surface [`spawn`](PaneLifecycle::spawn)ed
    /// exits (the daemon's reaper death-signal), or `None`. Deliberately a bare
    /// `Fn` and NOT the registry: the plugin layer stays session-tree-free (Interface
    /// Segregation, the R144 decision) while a plugin-spawned pane still feeds the daemon's
    /// self-cleaning exactly like a mux one — this layer never learns what the hook does.
    /// Set only by the host's plugin surface via [`with_pane_exit`](Self::with_pane_exit); the
    /// default is `None`, so nothing but the daemon wires it.
    on_pane_exit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// A MINTER for the daemon's attention hook: called once per pane this surface spawns to get
    /// that pane its own `on_attention`. Opaque, exactly as [`Self::on_pane_exit`] is — this layer
    /// never learns what the hook does, so a plugin-spawned pane whose child asks for a person
    /// reaches that person like a mux-spawned one. Wired by the host's plugin surface, `None`
    /// everywhere else.
    ///
    /// **A minter and not one shared closure**, because the hook the daemon hands out owns a channel
    /// sender per pane; asking for it per birth is what keeps the PTY reader thread that runs it from
    /// taking a lock. This layer expresses that as *"give me a hook"* without knowing why.
    ///
    /// **A separate signal and not a second use of the exit hook**, because they answer different
    /// questions about different moments — and because a pane category quietly left out of one of
    /// them is exactly the shape the notification path was in: every layer carrying the fact and one
    /// surface obliged to read it.
    on_attention: Option<AttentionMinter>,
}

impl WorkspacePaneAccess {
    /// Wrap a shared workspace as the plugin pane-access surface (no pane-exit hook — see
    /// [`with_pane_exit`](Self::with_pane_exit)).
    #[must_use]
    pub fn new(workspace: Arc<Mutex<Workspace>>) -> Self {
        Self {
            workspace,
            on_pane_exit: None,
            on_attention: None,
        }
    }

    /// Attach the daemon's opaque pane-exit death-signal, so a pane this surface spawns feeds
    /// the reaper on its death. A builder (not a `new` parameter) so the many non-daemon
    /// constructors — plugin machinery, tests — stay untouched and pass nothing.
    #[must_use]
    pub fn with_pane_exit(mut self, hook: Option<Arc<dyn Fn() + Send + Sync>>) -> Self {
        self.on_pane_exit = hook;
        self
    }

    /// Attach the daemon's opaque ATTENTION signal, so a pane this surface spawns can ask for a
    /// person. A builder for [`with_pane_exit`](Self::with_pane_exit)'s reason.
    #[must_use]
    pub fn with_attention(mut self, mint: Option<AttentionMinter>) -> Self {
        self.on_attention = mint;
        self
    }

    /// Clone the pane's I/O handle under the workspace lock (released before
    /// the handle is used), so screen reads / writes never hold the workspace
    /// lock.
    fn handle(&self, id: PaneId) -> Option<PanePtyHandle> {
        lock(&self.workspace).pane(id).map(Pane::handle)
    }
}

impl PaneAccess for WorkspacePaneAccess {
    fn pane_ids(&self) -> Vec<PaneId> {
        lock(&self.workspace).panes().iter().map(Pane::id).collect()
    }

    fn pane_collapsed(&self, id: PaneId) -> Option<String> {
        Some(self.handle(id)?.with_screen(read_collapsed))
    }

    fn pane_rows(&self, id: PaneId) -> Option<Vec<PaneRow>> {
        Some(self.handle(id)?.with_screen(read_rows))
    }

    fn pane_eof(&self, id: PaneId) -> Option<bool> {
        // A quick atomic load; reading it under the workspace lock (rather than
        // cloning the handle) is negligible and needs no producer change.
        lock(&self.workspace)
            .pane(id)
            .map(|pane| pane.pty().is_eof())
    }

    fn pane_full_text(&self, id: PaneId) -> Option<String> {
        Some(self.handle(id)?.with_screen(Screen::full_text))
    }

    fn inject(&self, id: PaneId, keys: &[KeyStroke]) -> Result<u64, PaneError> {
        let handle = self.handle(id).ok_or(PaneError::UnknownPane(id))?;
        let modes = handle.input_modes();
        let mut bytes = Vec::new();
        for stroke in keys {
            let encoded = encode(&stroke.key, stroke.mods, modes)
                .ok_or_else(|| PaneError::Encode(stroke.key.clone()))?;
            bytes.extend_from_slice(&encoded);
        }
        handle
            .write(&bytes)
            .map_err(|e| PaneError::Write(e.to_string()))?;
        Ok(bytes.len() as u64)
    }

    fn lifecycle(&self) -> Option<&dyn PaneLifecycle> {
        Some(self)
    }

    fn raw_capture(&self) -> Option<&dyn PaneRawCapture> {
        Some(self)
    }
}

impl PaneRawCapture for WorkspacePaneAccess {
    fn pane_raw_output(&self, id: PaneId) -> Option<RawOutput> {
        Some(self.handle(id)?.raw_output())
    }
}

impl PaneLifecycle for WorkspacePaneAccess {
    fn spawn(&self, argv: &[String], cols: u16, rows: u16) -> Result<PaneId, PaneError> {
        let (program, rest) = argv
            .split_first()
            .ok_or_else(|| PaneError::Spawn("empty argv".to_string()))?;
        let mut command = CommandBuilder::new(program.as_str());
        for arg in rest {
            command.arg(arg.as_str());
        }
        // The emulator parses (and strips) escape sequences, so captured cell
        // text stays clean regardless of TERM; match the host's spawn default.
        command.env("TERM", "xterm-256color");
        // Carry the daemon's death-signal (if any) so a plugin-spawned pane feeds the reaper
        // exactly like a boot/mux one — the opaque hook is just a channel send, so this
        // registry-free layer wires it without learning what it does.
        let hooks = PaneBirthHooks {
            on_dirty: None,
            on_exit: self.on_pane_exit.as_ref().map(|hook| {
                let hook = Arc::clone(hook);
                Box::new(move || hook()) as Box<dyn Fn() + Send>
            }),
            // ...and its ATTENTION, on the same terms: opaque, registry-free, and minted for THIS
            // pane rather than shared with every other one.
            on_attention: self.on_attention.as_ref().map(|mint| mint()),
        };
        // Nothing here says where the pane's cgroup goes, and that is the point: the pool this
        // spawns into carries its window's lineage and the daemon's subtree, so a plugin-spawned
        // pane is weighted exactly like every other one (R337). It used to carry a `home: None` over
        // a comment saying "the host fills this in when it has a tree" — the host did no such thing
        // for this door, and the comment was the only thing that said otherwise.
        lock(&self.workspace)
            .spawn_with_dirty(command, program.clone(), cols, rows, hooks)
            .map_err(|e| PaneError::Spawn(e.to_string()))
    }

    fn close(&self, id: PaneId) -> bool {
        // Bind the removed Pane so the workspace guard (the temporary) drops
        // first; the Pane's blocking Drop (kill/wait/join) then runs OUTSIDE
        // the workspace lock (R11 lesson).
        let removed = lock(&self.workspace).close(id);
        removed.is_some()
    }
}

/// Lock the workspace, recovering the guard if a holder panicked.
fn lock(workspace: &Mutex<Workspace>) -> MutexGuard<'_, Workspace> {
    workspace.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Per-row `(generation, text)` for the whole screen. Text via the canonical
/// [`Screen::row_text`] so capture and the emulator's scrollback never drift.
fn read_rows(screen: &Screen) -> Vec<PaneRow> {
    (0..screen.rows())
        .map(|row| PaneRow {
            generation: screen.row_generation(row).unwrap_or(0),
            text: screen.row_text(row),
        })
        .collect()
}

/// Collapsed screen text: trailing-trimmed rows joined without separators, so
/// a sentinel the terminal wrapped across rows still matches.
fn read_collapsed(screen: &Screen) -> String {
    (0..screen.rows()).map(|row| screen.row_text(row)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sprag_terminal::CommandBuilder;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    fn cat_workspace(cols: u16, rows: u16) -> Arc<Mutex<Workspace>> {
        let workspace = Arc::new(Mutex::new(Workspace::new((cols, rows))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("cat");
        command.env("TERM", "dumb");
        lock(&workspace)
            .spawn(command, "cat".to_string(), cols, rows)
            .expect("spawn pane");
        workspace
    }

    /// A pane a PLUGIN opens lands in the cgroup its pool's window names.
    ///
    /// The fifth door onto pane birth, and the one whose comment used to claim "the host fills this
    /// in when it has a tree" while the host did no such thing for this path (R337). It is gated
    /// here rather than trusted to the structure, because that comment is exactly what trusting the
    /// structure looks like from the inside.
    ///
    /// A stand-in cgroup root of ordinary files: this asserts the pool PLACED the pane, which is
    /// this layer's whole responsibility. That the kernel then honours the weight is
    /// `sprag-terminal/tests/pane_share_cgroup.rs`, against a real delegated scope.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pane_a_plugin_opens_lands_in_the_cgroup_its_window_names() {
        use sprag_terminal::share::{PaneHomes, PoolLineage, Tree};
        use sprag_terminal::{SessionId, WindowId};

        let root = std::env::temp_dir().join(format!("sprag-plugin-share-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let cgroup = |relative: &str| {
            let path = root.join(relative);
            std::fs::create_dir_all(&path).expect("fixture cgroup");
            std::fs::write(path.join("cgroup.procs"), "").expect("fixture procs");
            std::fs::write(path.join("cgroup.subtree_control"), "").expect("fixture subtree");
            // What the parent enabled here — read by every level's `enable_controllers`, and
            // present on every real cgroup.
            std::fs::write(path.join("cgroup.controllers"), "cpu memory pids\n")
                .expect("fixture controllers");
            std::fs::write(path.join("cpu.weight"), "100\n").expect("fixture weight");
        };
        cgroup("");
        cgroup("session-3");
        cgroup("session-3/window-4");

        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        {
            let mut pool = lock(&workspace);
            pool.set_home(PoolLineage {
                session: SessionId(3),
                window: WindowId(4),
            });
            pool.set_pane_homes(Arc::new(PaneHomes::over(
                Tree::adopt(root.clone()).expect("adopt the stand-in root"),
            )));
        }

        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let pane = access
            .spawn(
                &["/bin/sh".to_owned(), "-c".to_owned(), "cat".to_owned()],
                20,
                4,
            )
            .expect("a plugin opens a pane");

        assert!(
            root.join(format!("session-3/window-4/pane-{}", pane.0))
                .is_dir(),
            "a plugin's pane was born outside the share tree its window owns",
        );

        let _ = access.close(pane);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn injects_and_reads_back_through_the_api() {
        let access = WorkspacePaneAccess::new(cat_workspace(20, 4));
        let pane = access.pane_ids()[0];

        let mut keys = KeyStroke::text("hi");
        keys.push(KeyStroke::named("Enter"));
        let written = access.inject(pane, &keys).expect("inject");
        assert!(written >= 3, "wrote {written} bytes");

        // The echo is async; poll the collapsed text until it lands.
        let start = Instant::now();
        let mut echoed = false;
        while !echoed && start.elapsed() < Duration::from_secs(5) {
            echoed = access
                .pane_collapsed(pane)
                .is_some_and(|t| t.contains("hi"));
            if !echoed {
                sleep(Duration::from_millis(20));
            }
        }
        assert!(echoed, "injected 'hi' never echoed back");

        // pane_rows snapshots generation+text together.
        let rows = access.pane_rows(pane).expect("rows");
        assert_eq!(rows.len(), 4);
        assert!(
            rows.iter()
                .any(|r| r.text.contains("hi") && r.generation > 0)
        );
    }

    #[test]
    fn inject_into_unknown_pane_is_typed() {
        let access = WorkspacePaneAccess::new(cat_workspace(20, 4));
        let err = access
            .inject(PaneId(999), &KeyStroke::text("x"))
            .unwrap_err();
        assert_eq!(err, PaneError::UnknownPane(PaneId(999)));
    }

    #[test]
    fn lifecycle_spawn_and_close_roundtrip() {
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let access = WorkspacePaneAccess::new(Arc::clone(&workspace));
        let life = access
            .lifecycle()
            .expect("workspace access exposes lifecycle");

        let id = life
            .spawn(
                &["/bin/sh".to_string(), "-c".to_string(), "cat".to_string()],
                20,
                4,
            )
            .expect("spawn");
        assert!(lock(&workspace).pane(id).is_some(), "pane should be live");

        assert!(life.close(id), "close reports the pane existed");
        assert!(lock(&workspace).pane(id).is_none(), "pane should be gone");
        assert!(!life.close(id), "closing again reports absence");
    }

    #[test]
    fn lifecycle_spawn_rejects_empty_argv() {
        let access = WorkspacePaneAccess::new(Arc::new(Mutex::new(Workspace::new((20, 4)))));
        let life = access.lifecycle().unwrap();
        assert!(matches!(life.spawn(&[], 20, 4), Err(PaneError::Spawn(_))));
    }

    #[test]
    fn pane_full_text_includes_scrolled_off_lines() {
        // 30 numbered lines on a 4-row pane: the early ones scroll off. Full
        // text must include a line the visible-only read has lost.
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg("seq 1 30");
        command.env("TERM", "dumb");
        let id = lock(&workspace)
            .spawn(command, "seq".to_string(), 20, 4)
            .expect("spawn");
        let access = WorkspacePaneAccess::new(workspace);

        // Wait until the child has finished (all output applied at EOF).
        let start = Instant::now();
        while access.pane_eof(id) != Some(true) && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let full = access.pane_full_text(id).expect("full text");
        // "\n5\n": line 5 as a standalone line — deep in the scrolled-off region.
        assert!(
            full.contains("\n5\n"),
            "scrolled-off line 5 missing: {full:?}"
        );
        assert!(
            full.contains("30"),
            "last line missing from full text: {full:?}"
        );
        // The visible-only read lost it — proving scrollback was needed (the
        // last visible rows are ~27..30, none containing '5').
        let visible = access.pane_collapsed(id).expect("visible");
        assert!(
            !visible.contains('5'),
            "line 5 should have scrolled off: {visible:?}"
        );
    }

    #[test]
    fn pane_raw_output_is_byte_exact_for_a_wrapping_line() {
        // A single logical line wider than the pane: the grid wraps and trims
        // it, but the raw source read returns the emitted bytes verbatim — the
        // capture path structured output (a wrapped JSON envelope) relies on.
        let payload = "abc def  ghi   ".repeat(12); // 180 chars, embedded runs of spaces
        let workspace = Arc::new(Mutex::new(Workspace::new((20, 4))));
        let mut command = CommandBuilder::new("/bin/sh");
        command.arg("-c");
        command.arg(format!("printf '%s' '{payload}'"));
        command.env("TERM", "dumb");
        let id = lock(&workspace)
            .spawn(command, "printf".to_string(), 20, 4)
            .expect("spawn");
        let access = WorkspacePaneAccess::new(workspace);

        let start = Instant::now();
        while access.pane_eof(id) != Some(true) && start.elapsed() < Duration::from_secs(5) {
            sleep(Duration::from_millis(20));
        }

        let raw = access
            .raw_capture()
            .expect("workspace access exposes raw capture");
        let RawOutput { bytes, truncated } = raw.pane_raw_output(id).expect("raw output");
        assert!(!truncated);
        assert_eq!(
            String::from_utf8_lossy(&bytes),
            payload,
            "raw bytes must be verbatim"
        );
        // The grid lost interior spaces to trailing-trim at wrap boundaries, so
        // it cannot reconstruct the source — exactly why raw capture exists.
        assert!(
            raw.pane_raw_output(PaneId(999)).is_none(),
            "unknown pane is None"
        );
    }

    /// **A pane a PLUGIN spawned can ask for a person** — the wiring
    /// [`WorkspacePaneAccess::with_attention`] exists for, driven end to end rather than merely
    /// present.
    ///
    /// It was present and undriven: every live caller of the attention path went through the mux
    /// surface, so a minter that was never called — or one called once and shared, which is the
    /// defect the type exists to prevent — would have left every test green while a dialogue
    /// plugin's own pane told nobody its build had finished.
    ///
    /// Three claims, and the third is the one a shared closure would fail:
    ///
    /// * the hook fires at all, carrying the CHILD's own words;
    /// * it names the pane THIS surface spawned, so the router can find who holds it;
    /// * each pane gets its OWN hook — two births, two mints — which is what keeps the sender the
    ///   hook owns per-pane and the PTY reader thread free of a lock.
    #[test]
    fn a_pane_a_plugin_spawned_can_ask_for_a_person() {
        let raised: Arc<Mutex<Vec<(PaneId, Attention)>>> = Arc::new(Mutex::new(Vec::new()));
        let mints = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let access = WorkspacePaneAccess::new(Arc::new(Mutex::new(Workspace::new((40, 6)))))
            .with_attention(Some({
                let (raised, mints) = (Arc::clone(&raised), Arc::clone(&mints));
                Arc::new(move || {
                    mints.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let raised = Arc::clone(&raised);
                    Box::new(move |pane, attention| {
                        raised
                            .lock()
                            .expect("the raised log")
                            .push((pane, attention));
                    }) as Box<dyn Fn(PaneId, Attention) + Send>
                }) as AttentionMinter
            }));

        // The CHILD raises it, exactly as a build script inside a plugin's pane would.
        let pane = access
            .spawn(
                &[
                    "/bin/sh".to_owned(),
                    "-c".to_owned(),
                    "printf '\\033]9;the plugin pane needs you\\007'; exec cat".to_owned(),
                ],
                40,
                6,
            )
            .expect("a plugin spawns a pane");
        // A second pane, so the mint count below is a claim about PER-BIRTH minting rather than
        // about the one call every wiring would make.
        let other = access
            .spawn(
                &["/bin/sh".to_owned(), "-c".to_owned(), "exec cat".to_owned()],
                40,
                6,
            )
            .expect("a plugin spawns a second pane");
        assert_ne!(pane, other);

        let start = Instant::now();
        while raised.lock().expect("the raised log").is_empty()
            && start.elapsed() < Duration::from_secs(10)
        {
            sleep(Duration::from_millis(20));
        }
        let seen = raised.lock().expect("the raised log").clone();
        let (told, attention) = seen.first().unwrap_or_else(|| {
            panic!("the plugin pane's child asked for a person and the hook never fired: {seen:?}")
        });
        assert_eq!(*told, pane, "the hook must name the pane that raised it");
        match attention {
            Attention::Raised(notification) => assert_eq!(
                notification.body, "the plugin pane needs you",
                "the child's own words must arrive",
            ),
            other => panic!("an OSC 9 is a raised notification, not {other:?}"),
        }
        assert_eq!(
            mints.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "a hook is minted PER BIRTH — one shared closure would be minted once",
        );
    }
}
