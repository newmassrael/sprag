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
