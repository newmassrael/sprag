//! Shared scaffolding for sprag's RPC `External` control surfaces.
//!
//! sprag's control Externals (pane input, workspace control, plugin runs) are
//! all Rpc-only, framework-repainted, UI-thread-synchronous engines whose five
//! `External` methods are byte-identical — the Rule-of-Three pinion itself
//! solved for its own proxies with `query_proxy_external_impl!`.
//! [`rpc_external_impl`] is the sprag-local equivalent, but `&[Backend::Rpc]`
//! only (pinion's also declares `Gui`), since sprag's control surfaces have no
//! glyph paint. Each External keeps its own hand-written `ExternalIntrospect`
//! (the part that genuinely differs) and `Debug`.
//!
//! Plus the shared mutex-lock and invoke-arg helpers the Externals' parsers
//! reuse, so the host stops re-hand-rolling `get().and_then(as_u64)...`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use pinion_core::external::{IntrospectValue, InvokeError};
use serde_json::{Map, Value};
use sprag_terminal::PaneId;

/// Emit the five byte-identical `External` methods for an Rpc-only sprag
/// control surface. The type must impl `ExternalIntrospect` (the introspect
/// methods return `Some(self)`).
macro_rules! rpc_external_impl {
    ($t:ty) => {
        impl ::pinion_core::external::External for $t {
            fn backends(&self) -> ::pinion_core::external::BackendSupport {
                // Rpc-only: a scene-as-data control surface, no glyph paint.
                ::pinion_core::external::BackendSupport::new(
                    &[::pinion_core::external::Backend::Rpc],
                    ::pinion_core::external::BackendFallback::Skip,
                )
            }
            fn repaint_ownership(&self) -> ::pinion_core::external::RepaintOwner {
                ::pinion_core::external::RepaintOwner::Framework
            }
            fn thread_ownership(&self) -> ::pinion_core::external::ThreadOwnership {
                // Producer threads sit behind the boundary (R1.7); the
                // External's own work is synchronous.
                ::pinion_core::external::ThreadOwnership::UiThreadSync
            }
            fn introspect(&self) -> Option<&dyn ::pinion_core::external::ExternalIntrospect> {
                Some(self)
            }
            fn introspect_mut(
                &mut self,
            ) -> Option<&mut dyn ::pinion_core::external::ExternalIntrospect> {
                Some(self)
            }
        }
    };
}
pub(crate) use rpc_external_impl;

/// Lock a host mutex, recovering the guard if a holder panicked.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Refuse to fire an action, STATING WHY — the one place this daemon turns a fact it observed
/// into the sentence a person reads.
///
/// # Why this is a funnel and not 90 `format!` calls
///
/// Until PINION-PR82 landed ([`InvokeError::Rejected`] gained a payload) a producer had nowhere to
/// put its reason, so the ninety-odd refusal sites in this crate did the only thing left: they
/// wrote the fact to a `tracing::debug!` and returned an empty variant. The log and the wire then
/// said different things — the log said *"a session named \"beta\" already exists"* and the wire
/// said `Rejected` — and the CLI, having nothing to print, listed the causes it could imagine.
/// Measured at `87cde88`: `rename-session` offered **four**, `break-pane` and `join-pane` **three**
/// each, and in every case the registry had returned a typed error naming exactly one.
///
/// So both halves happen HERE, off one argument. A site cannot log one fact and publish another,
/// and a site cannot refuse anonymously — [`InvokeError::rejected`] requires the sentence and this
/// requires it too.
///
/// # What a reason reads like
///
/// A CLAUSE, not a sentence: lowercase, no trailing period, naming the fact and not the verb. A
/// caller renders it after its own subject (`sprag: join-pane: <clause>`), and a surface that
/// receives it may put it in a status row 200 bytes wide — so it states the ONE thing that was
/// observed rather than a paragraph about what to do instead.
///
/// Most callers pass a typed error straight in ([`sprag_terminal::PaneMoveError`],
/// [`sprag_terminal::SessionError`]), whose `Display` is already that clause and is already the
/// thing the registry decided. Deriving beats re-authoring for this file's usual reason: a sentence
/// written twice is a sentence that drifts.
pub fn refused(reason: impl std::fmt::Display) -> InvokeError {
    let reason = reason.to_string();
    tracing::debug!(target: "sprag_host", %reason, "refused an action");
    InvokeError::rejected(reason)
}

/// Unwrap invoke args as a JSON object (`{...}`), else [`InvokeError::TypeMismatch`].
pub fn as_object(args: &IntrospectValue) -> Result<&Map<String, Value>, InvokeError> {
    match args {
        IntrospectValue::Json(Value::Object(map)) => Ok(map),
        _ => Err(InvokeError::TypeMismatch),
    }
}

/// A required `u64` field read as a [`PaneId`].
pub fn require_pane_id(map: &Map<String, Value>, key: &str) -> Result<PaneId, InvokeError> {
    map.get(key)
        .and_then(Value::as_u64)
        .map(PaneId)
        .ok_or(InvokeError::TypeMismatch)
}

/// A required non-empty string field.
pub fn require_str<'a>(map: &'a Map<String, Value>, key: &str) -> Result<&'a str, InvokeError> {
    map.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(InvokeError::TypeMismatch)
}

/// An optional string field: `None` if absent, `Err` if present but not a string.
pub fn opt_str<'a>(map: &'a Map<String, Value>, key: &str) -> Result<Option<&'a str>, InvokeError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => value.as_str().map(Some).ok_or(InvokeError::TypeMismatch),
    }
}

/// An optional positive `u16` dimension: `None` if absent, `Err` if present but
/// not a positive `u16`.
pub fn opt_dim(map: &Map<String, Value>, key: &str) -> Result<Option<u16>, InvokeError> {
    match map.get(key) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .filter(|&n| n > 0)
            .map(Some)
            .ok_or(InvokeError::TypeMismatch),
    }
}
