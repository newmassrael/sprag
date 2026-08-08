//! This client's KEYMAP: the user's prefix table, the one-key mode it arms, and the live re-read
//! that makes `sprag bind-key` reach a running window.
//!
//! # Why the GUI has a prefix at all
//!
//! Every chord this client owned before now lives in the `Ctrl+Shift+*` space
//! ([`crate::input`]: clipboard, find, palette, session, dock), and that choice was the
//! arbitration — a terminal cannot encode `Ctrl+Shift+<key>` distinctly, so those chords steal no
//! key from a pane's child. It works, and it is not a keymap: the keys are written into the binary
//! and name surfaces (a find bar, a palette) that the binding vocabulary has no word for.
//!
//! What the user's [`Keymap`](sprag_host::keymap::Keymap) names is what BOTH frontends can do — detach, send the
//! prefix, split the focused pane, move focus on, fill the window with one pane. So this is the
//! table the GUI adopts, whole: the
//! same file, the same defaults, the same modes, and the same live re-read. A rebind typed
//! into a shell reaches this window on the next keystroke, exactly as it reaches an attached
//! `sprag-tui`.
//!
//! That includes both of slice 4's additions, and neither cost this file anything: a ROOT binding is
//! one more answer out of `Keymap::route`, and a REPEAT window is one more state in the mode this
//! holder already carried. The root table's placement is [`crate::input::route_key`]'s question, not
//! this one's — it sits after the pane gate, so a bare bound key cannot be taken out of a search
//! field.
//!
//! # Why not `WidgetCore::keybinding`
//!
//! Because it would be dropped. sprag declares NO primary surface (R127), and pinion routes a
//! keybinding event to the no-op `send_to_primary` on a no-primary binding — which is why
//! `keybinding` is deliberately left empty and a test says so. The GUI's keyboard is
//! `apply_key` -> [`crate::input::route_key`], and that is where a keymap can act.
//!
//! # Why a broken file does not stop the window
//!
//! `sprag-tui` refuses to START on a config it cannot use, because the screen able to show the
//! message is the one it has not yet replaced. This client has no such screen, so it takes the
//! DEFAULTS plus the reason ([`sprag_host::config::ClientConfig::load_usable`]) and reports through
//! the surface that already exists for exactly this: a broken config is a line in the command
//! palette, beside the one a broken `.sprag.toml` gets ([`report`](ClientKeys::report)).
//!
//! Logging it instead would be a table nobody can see — the failure class R235 was about.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Instant;

use pinion_core::reactive::Owner;
use sprag_host::config::ClientConfig;
use sprag_host::keyhelp::KeyHelp;
use sprag_host::keymap::{BoundAction, KeySpec, PrefixMode, Routed};

/// `Owner::cache` key for this client's keymap holder.
const CLIENT_KEYS_KEY: &str = "sprag_gui.keys";

/// The user's table, where the next keystroke goes, and why the file could not be used.
///
/// One holder rather than three cache slots because the three are read together on every keystroke
/// and only ever change together: a re-read that fails is the same event as the report appearing.
///
/// NOT reactive, and that is deliberate. Nothing in `view` reads any of this — a keymap decides what
/// a key MEANS, and the paint that follows is decided by what the action did. A [`Signal`] here would
/// flip the root owner dirty on every prefix keystroke and arm a repaint with nothing to repaint.
///
/// [`Signal`]: pinion_core::reactive::Signal
pub(crate) struct ClientKeys {
    /// The user's keymap plus the text it was read from, so a `bind-key` (or an editor) is noticed.
    file: RefCell<ClientConfig>,
    /// Where the next keystroke goes. `Cell` because it is a small `Copy` state, read and written on
    /// one thread inside one event handler.
    mode: Cell<PrefixMode>,
}

impl ClientKeys {
    /// Read the user's keymap now. A file that cannot be used leaves the DEFAULTS in force and its
    /// reason on the file itself, where [`report`](Self::report) reads it.
    fn load() -> Self {
        let (file, _) = ClientConfig::load_usable();
        Self {
            file: RefCell::new(file),
            mode: Cell::new(PrefixMode::default()),
        }
    }

    /// Take the prefix mode out, leaving the steady state behind.
    ///
    /// **Called once, first, on every keystroke that reaches this client** — before anything looks
    /// at what the key is or where it is going. The mode is one key long whatever that key turns out
    /// to be, and [`crate::input::route_key`] has five surfaces that can consume a key before the
    /// pane is even resolved; taking the mode out in one place is what keeps a prefix armed in a
    /// pane from surviving a keystroke typed into a find field.
    ///
    /// A REPEAT window is taken out by the same move, and deliberately: a window is the prefix table
    /// still being armed, so a character typed into a search needle has to end it for the same
    /// reason it ends the one-key mode. The window is a duration only in the absence of other input.
    pub(crate) fn take(&self) -> PrefixMode {
        self.mode.replace(PrefixMode::ToPane)
    }

    /// Route a keystroke through the user's table, re-reading the file first and arming the mode if
    /// this key was the prefix.
    ///
    /// `mode` is what [`take`](Self::take) removed at the top of this keystroke, so the arming here
    /// is the ONLY write that can put the mode back.
    ///
    /// A broken save KEEPS the last good table and records the reason for the palette. Swapping in
    /// the defaults would take a user's own bindings away because they typo'd a line in an editor.
    /// The clock is read HERE rather than inside the keymap so a repeat window can be tested by
    /// passing an instant. This is the whole of what `-r` costs the event loop: no timer, no thread,
    /// no tick — nothing observes a window closing except the next keystroke.
    ///
    /// ⚠ **THIS CLOCK IS THIS CLIENT'S, NOT THE PERSON'S — and that is PINION-PR84, filed and open.**
    /// A repeat window is a statement about the user's own timeline, so it has to be judged against
    /// the moment the keystroke ARRIVED; `Instant::now()` here is the moment this client got round
    /// to it, which is later by however long the previous key's blocking round trip to the daemon
    /// took ([`SlotView::resize_toward`](crate::slotview::SlotView::resize_toward) and its peers).
    /// `sprag-tui` measured that as a user-facing defect and fixed it — 3 failures in 6 runs at 2x
    /// CPU oversubscription, down to 0 — by dating each keystroke at the read it came out of.
    ///
    /// **This frontend cannot do the same**: `pinion_core::event::KeyEvent` carries a key code and
    /// nothing else, there is no event time anywhere on the dispatch path, and the event queue
    /// cannot be drained by the embedder, so not even "these two arrived together" is knowable. The
    /// line changes to pass an arrival instant the moment PR-84 is delivered; until then the two
    /// frontends judge one user's table by two clocks, which is the cost this comment exists to
    /// keep visible rather than to justify.
    pub(crate) fn route(
        &self,
        mode: PrefixMode,
        name: &str,
        mods: sprag_input::Modifiers,
    ) -> Routed {
        self.reread();
        let routed = self
            .file
            .borrow()
            .keymap()
            .route(mode, Instant::now(), name, mods);
        self.mode.set(routed.next());
        routed
    }

    /// Re-read the file if it has moved, keeping the last good table if the new content is unusable.
    ///
    /// **Called from the two places the file's answer is USED, and nowhere else**: routing a keystroke
    /// ([`route`](Self::route)) and showing the report ([`report`](Self::report)). That rule is the
    /// whole design — no thread, no timer, no watch, just a read of a few hundred bytes on a wake the
    /// client already had.
    ///
    /// The palette being the second consumer was found by RUNNING it. With routing as the only
    /// re-reader, a user whose config was broken could fix the file, reopen the palette, and still be
    /// told it was broken — because the palette's field holds the keyboard while it is open, so no
    /// keystroke reaches a pane to trigger the re-read. A surface showing a state the file has left
    /// behind is exactly the failure class R235 was about, and it survived a green suite.
    fn reread(&self) {
        if let Err(error) = self.file.borrow_mut().refresh() {
            tracing::warn!(
                target: "sprag_gui::keys",
                %error,
                "the edited config was not usable; keeping the loaded keymap",
            );
        }
    }

    /// The prefix key, for the `send-prefix` action to deliver.
    ///
    /// Owned rather than borrowed: the caller is about to send it through the host client, and a
    /// borrow held across that would outlive the one place the table can be re-read.
    pub(crate) fn prefix(&self) -> KeySpec {
        self.file.borrow().keymap().prefix().clone()
    }

    /// The chord that reaches `action` in the table in force, or [`None`] when no key does.
    ///
    /// What the palette's hint column shows, and the reason that column stopped being five hardcoded
    /// strings (R308): a rebound key now teaches itself and an unbound one advertises nothing.
    ///
    /// **No re-read, unlike [`help`](Self::help), and that is deliberate**: this is called once per
    /// painted row, on every frame the palette is open, and re-reading the file per row would turn a
    /// list into a directory walk. It does not need one either — the palette re-reads through
    /// [`report`](Self::report) when it OPENS, which is the same reason that consumer exists (see
    /// [`reread`](Self::reread)), so the table these rows are drawn from is the table that was on
    /// disk when the list was frozen.
    pub(crate) fn chord_of(&self, action: &BoundAction) -> Option<String> {
        self.file.borrow().keymap().chord_of(action)
    }

    /// The table as a view a client can PAINT — what `list-keys` shows on the screen (R308).
    ///
    /// **The third place the file's answer is used, and so the third re-reader** — see
    /// [`reread`](Self::reread), whose rule this follows rather than extends. It matters here for
    /// the palette's own recorded reason, one surface over: a user whose config was broken can fix
    /// the file and press `?`, and a view built from the last routed table would show them what
    /// they have just stopped having. Nothing else in the client re-reads on their behalf, because
    /// no keystroke reaches a pane while a modal holds the keyboard.
    ///
    /// Built under the borrow and handed back OWNED, for [`prefix`](Self::prefix)'s reason: the
    /// caller stores it in a `Signal` for as long as the panel is up, which is far longer than a
    /// borrow of the one thing that can be re-read.
    pub(crate) fn help(&self) -> KeyHelp {
        self.reread();
        KeyHelp::of(self.file.borrow().keymap())
    }

    /// The OPTIONS table in force — what a client-side setting (`display-time`) is read from.
    ///
    /// **The fourth place the file's answer is used, and so the fourth re-reader** — see
    /// [`reread`](Self::reread), whose rule this follows for a reason of its own: a user who raises
    /// `display-time` because a message went by too fast is a user who wants the NEXT message to
    /// last longer, and a table read once at boot would make them restart the client to get it.
    ///
    /// Handed back OWNED for [`prefix`](Self::prefix)'s reason: the caller reads a number out of it
    /// after the borrow would have ended.
    pub(crate) fn options(&self) -> sprag_host::Options {
        self.reread();
        self.file.borrow().options().clone()
    }

    /// Why the user's config could not be used, if it could not — already worded to name the file, and
    /// re-read so the answer describes the file as it is NOW.
    ///
    /// The palette shows this beside the reports a broken project config gets. It is collected THERE
    /// rather than by [`crate::command::catalog`] because the catalog asks the HOST, and a keymap is
    /// the one thing in that file the host never reads: a keybinding is what one client does with one
    /// keyboard, so `global_commands` deserializes the same file and a bad `action = "…"` string is
    /// invisible to it.
    pub(crate) fn report(&self) -> Option<String> {
        self.reread();
        self.file.borrow().unusable().map(ToString::to_string)
    }
}

/// This client's keymap holder, `Owner::cache`-backed so it survives between keystrokes.
///
/// A test PRE-FILLS this slot (`test_support::seed_keys`, `#[cfg(test)]` so rustdoc cannot link it) rather than writing a file and pointing
/// `XDG_CONFIG_HOME` at it: the cache slot is this crate's injection seam, and reading the ambient
/// config in a unit suite would make every keymap assertion depend on whether the developer running
/// it happens to have rebound their prefix.
pub(crate) fn use_client_keys() -> Rc<ClientKeys> {
    Owner::current()
        .expect("use_client_keys() requires an active Owner scope")
        .cache(CLIENT_KEYS_KEY, ClientKeys::load)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;

    use super::{CLIENT_KEYS_KEY, Cell, ClientConfig, ClientKeys, Owner, PrefixMode, Rc, RefCell};

    /// Seed the keymap slot from `config` — a file the test wrote — so a test drives a KNOWN keymap
    /// and can still edit that file to prove the live re-read.
    ///
    /// A pre-fill rather than `$XDG_CONFIG_HOME`: the environment is process-global, so pointing it
    /// at a temp dir would have every sibling test in this crate reading whatever config the last one
    /// wrote — and without a pre-fill of some kind, every keymap assertion here would depend on
    /// whether the developer running it happens to have rebound their own prefix.
    ///
    /// Must run before anything else resolves the slot: the first resolution wins, which is what
    /// makes this an injection rather than an override.
    pub(crate) fn seed_keys(config: &Path) -> Rc<ClientKeys> {
        // The reason a file cannot be used lives ON the file, so a seed does not have to carry it.
        let (file, _) = ClientConfig::at(config);
        Owner::current()
            .expect("seed_keys() requires an active Owner scope")
            .cache(CLIENT_KEYS_KEY, || ClientKeys {
                file: RefCell::new(file),
                mode: Cell::new(PrefixMode::default()),
            })
    }

    /// A throwaway `config.toml` plus the seeded keymap that reads it.
    ///
    /// Here rather than in one module's test block because two now need it — the routing tests and
    /// the palette's hint column — and a second copy of a fixture that writes a file and seeds a
    /// cache slot is a second thing that can drift about what "a known keymap" means.
    pub(crate) struct Config(std::path::PathBuf);

    impl Config {
        /// Write `text` as this client's config and seed the keymap from it.
        pub(crate) fn seeded(text: &str) -> (Self, Rc<ClientKeys>) {
            use std::sync::atomic::{AtomicU32, Ordering};
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "sprag-gui-keys-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            std::fs::create_dir_all(&dir).expect("temp config dir");
            let config = Self(dir);
            config.write(text);
            let keys = seed_keys(&config.path());
            (config, keys)
        }

        /// Where the file is.
        pub(crate) fn path(&self) -> std::path::PathBuf {
            self.0.join("config.toml")
        }

        /// Rewrite the file — what `sprag bind-key` does, and what an editor does.
        pub(crate) fn write(&self, text: &str) {
            std::fs::write(self.path(), text).expect("write config");
        }
    }

    impl Drop for Config {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
