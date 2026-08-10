//! What a display surface's DECLARATION claims, held against what it does — the display half of
//! `sprag_host::wire`'s `a_declared_read_answers_and_a_declared_verb_does_not`.
//!
//! # Why the same property is asserted twice
//!
//! The daemon's gate walks the scene a request is served from, which is the mux and the pane
//! surfaces. **The GUI's own externals are not in it**: the palette, the confirmation front and the
//! hyperlink oracle hang in the window's scene, are addressed by `scene/invoke` from the pixel
//! smoke and from any client driving this window, and nothing was checking their declarations at
//! all. R352 found the whole set mis-declared — eight verbs on the READ channel, and `activate`
//! dispatched and declared nowhere — which is the same two defects the mux surface had, in a crate
//! the daemon's gate structurally cannot reach.
//!
//! So the property lives here once and each surface's own test module applies it to itself. A
//! surface added later gets it by calling one function, and the reason it should is written down.

#[cfg(test)]
use pinion_core::external::{ExternalIntrospect, IntrospectValue, InvokeError};
use pinion_core::external::{IntrospectSchema, SchemaChannel};

/// Whether `schema` DECLARES `path` as a verb — the guard each of these surfaces runs before it
/// dispatches, so a verb it does not publish is a verb it does not run.
///
/// ⚠⚠ **`activate` was dispatched by the hyperlink oracle and declared nowhere**, and the gate
/// below could not see it: it walks the DECLARED fields, so an omission declares nothing to audit.
/// That is pinion's own observation about `IntrospectSchema` and it applies here word for word. The
/// only thing that closes it is making the undeclared arm UNREACHABLE, which is what this does —
/// pinion refuses an undeclared `scene/invoke` at the RPC boundary from R1637 and says plainly what
/// that leaves open (*"In-process dispatch … the framework has no seam that could intercept it"*),
/// and every one of these surfaces is driven in-process by this binary's own shell.
pub(crate) fn declares_verb(schema: &IntrospectSchema, path: &str) -> bool {
    schema
        .fields
        .iter()
        .any(|field| field.path == path && field.channel == SchemaChannel::Invoke)
}

/// Assert that every path this surface DECLARES is the kind of path it says it is.
///
/// ⚠ This is the MIS-DECLARATION half only. An implemented verb left out of the schema is invisible
/// here by construction, and what closes that is [`declares_verb`] making the arm unreachable —
/// with a test that CALLS the verb, which then fails the moment the declaration is missing.
///
/// Two directions, because they fail differently:
///
/// * a path on the **read** channel must ANSWER a query — an agent discovering the surface from
///   its schema queries it, and a name that answers nothing gives back the refusal a client too
///   old for the surface gets, so the surface's own mistake reads as version skew;
/// * a path on the **invoke** channel must NOT answer one, and must be REACHABLE as a call — a
///   declared verb its dispatch does not know is a name published to nobody's benefit, and pinion
///   refuses an undeclared call outright from R1637.
///
/// The verb half calls the surface, so `invoke` takes `&mut` and the caller hands over the surface
/// itself. ⚠ **The calls are made with `Null` arguments**, which most of these verbs decline — that
/// is fine and is the point: this asks whether the NAME is dispatched, and
/// [`InvokeError::UnknownPath`] is the only answer that says it is not.
#[cfg(test)]
pub(crate) fn a_declared_path_is_what_it_claims(surface: &mut dyn ExternalIntrospect) {
    let fields: Vec<_> = surface.schema().fields.to_vec();
    assert!(
        !fields.is_empty(),
        "a surface with no declarations proves nothing about declarations",
    );

    let mut reads = 0;
    let mut verbs = 0;
    for field in &fields {
        match field.channel {
            SchemaChannel::Read if field.args.is_empty() => {
                assert!(
                    surface.query(field.path).is_some(),
                    "`{}` is declared on the READ channel and answers nothing — declare it with \
                     `SchemaField::action` if it is a verb",
                    field.path,
                );
                reads += 1;
            }
            // A parametric read's template is not an address any client sends, so there is nothing
            // to query it at; its members are its own surface's business.
            SchemaChannel::Read => {}
            _ => {
                assert!(
                    surface.query(field.path).is_none(),
                    "`{}` is declared as a verb and also answers a query — one address serving \
                     two channels, so what a client gets depends on which door it knocked on",
                    field.path,
                );
                assert!(
                    !matches!(
                        surface.invoke(field.path, IntrospectValue::Null),
                        Err(InvokeError::UnknownPath)
                    ),
                    "`{}` is declared as a verb and this surface's dispatch does not know it",
                    field.path,
                );
                verbs += 1;
            }
        }
    }

    assert!(
        reads > 0 && verbs > 0,
        "this surface declares both kinds, or one of the two directions above is about nothing: \
         {reads} reads, {verbs} verbs",
    );
}
