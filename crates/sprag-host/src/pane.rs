//! The terminal pane as a pinion `External` — the engine side of the
//! R1.7 split.
//!
//! PINION-REQUIREMENTS R1.7 separates the pane into *data* and *engine*:
//! the cell grid is exposed as a `Scene::TextGrid` (introspectable
//! projection), while the PTY+emulator engine sits behind an `External`
//! boundary (process-side opacity justified). [`SpragPaneExternal`] is that
//! engine surface. It carries no scene state of its own — only a
//! [`SessionHandle`] onto the live producer — so input is a *producer*
//! mutation reached through pinion's canonical `scene/invoke`, not a
//! mutation of pinion's projection (R969: pinion projects, the producer
//! owns state).
//!
//! The action channel is the R2.6 input seam: `invoke("key", {key, …})`
//! encodes the W3C key + modifiers to PTY bytes ([`sprag_input::encode`],
//! sprag-owned) and writes them to the child. The read channel exposes the
//! producer-owned input modes (`query("application_cursor_keys")`) and the
//! pane's full output text (`query("full_text")`, scrollback + visible) — the
//! same `Screen::full_text` the in-process capture path reads, so an external
//! peer and a plugin share one notion of the screen.

use std::fmt;

use pinion_core::external::{
    ExternalIntrospect, IntrospectSchema, IntrospectValue, InterveneError, InvokeError,
};
use serde_json::Value;
use sprag_input::Modifiers;
use sprag_terminal::SessionHandle;
use sprag_vt::Screen;

use crate::external::rpc_external_impl;

/// The invoke action that injects a key into the focused pane.
const KEY_ACTION: &str = "key";
/// The query slot reporting the producer's DECCKM (application cursor
/// keys) state.
const CURSOR_KEYS_SLOT: &str = "application_cursor_keys";
/// The query slot reporting the pane's full output text (scrollback +
/// visible) — the same [`Screen::full_text`] the in-process capture path
/// reads, so an external peer and a plugin see one notion of the screen.
const FULL_TEXT_SLOT: &str = "full_text";

/// The pane engine `External`: a thin, scene-stateless forwarder onto the
/// live [`SessionHandle`]. Input arrives via `scene/invoke` and is encoded
/// to PTY bytes by the sprag-owned encoder (R2.6); the producer's reader
/// thread lives behind this boundary, so the engine is `UiThreadSync` from
/// pinion's vantage (it does its work synchronously when invoked).
pub struct SpragPaneExternal {
    session: SessionHandle,
}

impl SpragPaneExternal {
    /// Build the engine surface over a live session's I/O handle.
    #[must_use]
    pub fn new(session: SessionHandle) -> Self {
        Self { session }
    }

    /// Encode a `key` action's args and write the resulting bytes to the
    /// PTY. A `state:"up"` edge is a no-op success (terminals emit no
    /// release in this mode). An unencodable key or a write failure is an
    /// [`InvokeError::Rejected`].
    fn inject_key(&self, args: &IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        let Some((key, mods)) = parse_key_args(args)? else {
            return Ok(IntrospectValue::Null); // suppressed key-up edge
        };
        let bytes = sprag_input::encode(&key, mods, self.session.input_modes())
            .ok_or(InvokeError::Rejected)?;
        self.session.write(&bytes).map_err(|_| InvokeError::Rejected)?;
        Ok(IntrospectValue::Null)
    }
}

impl fmt::Debug for SpragPaneExternal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `SessionHandle` wraps un-`Debug` PTY/emulator handles; the engine
        // is identified structurally (External: Debug is required by §5.2).
        f.debug_struct("SpragPaneExternal").finish_non_exhaustive()
    }
}

rpc_external_impl!(SpragPaneExternal);

impl ExternalIntrospect for SpragPaneExternal {
    fn schema(&self) -> IntrospectSchema {
        IntrospectSchema::new(&[
            (KEY_ACTION, "action"),
            (CURSOR_KEYS_SLOT, "bool"),
            (FULL_TEXT_SLOT, "string"),
        ])
    }

    fn query(&self, path: &str) -> Option<IntrospectValue> {
        match path {
            CURSOR_KEYS_SLOT => {
                Some(IntrospectValue::Bool(self.session.input_modes().application_cursor_keys))
            }
            FULL_TEXT_SLOT => Some(IntrospectValue::Text(self.session.with_screen(Screen::full_text))),
            _ => None,
        }
    }

    fn intervene(&mut self, _path: &str, _value: IntrospectValue) -> Result<(), InterveneError> {
        // No writable state slots: input is an action (invoke `key`) and the
        // cursor-keys mode is producer-owned (read-only here).
        Err(InterveneError::UnknownPath)
    }

    fn invoke(&mut self, path: &str, args: IntrospectValue) -> Result<IntrospectValue, InvokeError> {
        match path {
            KEY_ACTION => self.inject_key(&args),
            _ => Err(InvokeError::UnknownPath),
        }
    }
}

/// Parse the `key` action's args into a `(key, modifiers)` pair, or `None`
/// when the edge is a `state:"up"` release (which injects nothing).
///
/// Accepts either a bare string (`"a"` → that key, no modifiers, press) or
/// an object `{key, ctrl?, alt?, shift?, super?, state?}`. The `super` JSON
/// field maps to [`Modifiers::sup`]. Malformed args are an
/// [`InvokeError::TypeMismatch`].
fn parse_key_args(args: &IntrospectValue) -> Result<Option<(String, Modifiers)>, InvokeError> {
    match args {
        IntrospectValue::Text(key) if !key.is_empty() => {
            Ok(Some((key.clone(), Modifiers::default())))
        }
        IntrospectValue::Json(Value::Object(map)) => {
            let key = map
                .get("key")
                .and_then(Value::as_str)
                .filter(|k| !k.is_empty())
                .ok_or(InvokeError::TypeMismatch)?;
            match map.get("state").and_then(Value::as_str) {
                Some("up") => return Ok(None),
                Some("down") | None => {}
                Some(_) => return Err(InvokeError::TypeMismatch),
            }
            let flag = |name: &str| map.get(name).and_then(Value::as_bool).unwrap_or(false);
            let mods = Modifiers {
                ctrl: flag("ctrl"),
                alt: flag("alt"),
                shift: flag("shift"),
                sup: flag("super"),
            };
            Ok(Some((key.to_string(), mods)))
        }
        _ => Err(InvokeError::TypeMismatch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn json_args(v: serde_json::Value) -> IntrospectValue {
        IntrospectValue::Json(v)
    }

    #[test]
    fn parses_bare_string_key() {
        let parsed = parse_key_args(&IntrospectValue::Text("a".to_string())).unwrap();
        assert_eq!(parsed, Some(("a".to_string(), Modifiers::default())));
    }

    #[test]
    fn parses_object_with_modifiers() {
        let parsed = parse_key_args(&json_args(json!({"key": "c", "ctrl": true}))).unwrap();
        assert_eq!(parsed, Some(("c".to_string(), Modifiers { ctrl: true, ..Modifiers::default() })));
    }

    #[test]
    fn super_field_maps_to_sup() {
        let parsed = parse_key_args(&json_args(json!({"key": "x", "super": true}))).unwrap();
        assert_eq!(parsed.unwrap().1, Modifiers { sup: true, ..Modifiers::default() });
    }

    #[test]
    fn key_up_edge_is_suppressed() {
        let parsed = parse_key_args(&json_args(json!({"key": "a", "state": "up"}))).unwrap();
        assert_eq!(parsed, None);
    }

    #[test]
    fn missing_or_empty_key_is_type_mismatch() {
        assert_eq!(parse_key_args(&json_args(json!({}))), Err(InvokeError::TypeMismatch));
        assert_eq!(parse_key_args(&json_args(json!({"key": ""}))), Err(InvokeError::TypeMismatch));
        assert_eq!(parse_key_args(&IntrospectValue::Int(1)), Err(InvokeError::TypeMismatch));
    }

    #[test]
    fn unknown_state_is_type_mismatch() {
        assert_eq!(
            parse_key_args(&json_args(json!({"key": "a", "state": "sideways"}))),
            Err(InvokeError::TypeMismatch),
        );
    }
}
