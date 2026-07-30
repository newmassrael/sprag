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
//! What the user's [`Keymap`](sprag_host::keymap::Keymap) names is the four things BOTH frontends can do — detach, send the
//! prefix, split the focused pane, move focus on. So this is the table the GUI adopts, whole: the
//! same file, the same defaults, the same one-key mode, and the same live re-read. A rebind typed
//! into a shell reaches this window on the next keystroke, exactly as it reaches an attached
//! `sprag-tui`.
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
//! DEFAULTS plus the reason ([`sprag_host::config::KeymapFile::load_usable`]) and reports through
//! the surface that already exists for exactly this: a broken config is a line in the command
//! palette, beside the one a broken `.sprag.toml` gets ([`report`](ClientKeys::report)).
//!
//! Logging it instead would be a table nobody can see — the failure class R235 was about.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use pinion_core::reactive::Owner;
use sprag_host::config::KeymapFile;
use sprag_host::keymap::{KeySpec, PrefixMode, Routed};

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
    file: RefCell<KeymapFile>,
    /// Where the next keystroke goes. `Cell` because it is a two-state flag, read and written on one
    /// thread inside one event handler.
    mode: Cell<PrefixMode>,
}

impl ClientKeys {
    /// Read the user's keymap now. A file that cannot be used leaves the DEFAULTS in force and its
    /// reason on the file itself, where [`report`](Self::report) reads it.
    fn load() -> Self {
        let (file, _) = KeymapFile::load_usable();
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
    pub(crate) fn route(
        &self,
        mode: PrefixMode,
        name: &str,
        mods: sprag_input::Modifiers,
    ) -> Routed {
        self.reread();
        let routed = self.file.borrow().keymap().route(mode, name, mods);
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

    use super::{CLIENT_KEYS_KEY, Cell, ClientKeys, KeymapFile, Owner, PrefixMode, Rc, RefCell};

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
        let (file, _) = KeymapFile::at(config);
        Owner::current()
            .expect("seed_keys() requires an active Owner scope")
            .cache(CLIENT_KEYS_KEY, || ClientKeys {
                file: RefCell::new(file),
                mode: Cell::new(PrefixMode::default()),
            })
    }
}
