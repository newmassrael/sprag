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
use pinion_core::external::{ExternalIntrospect, IntrospectValue, InvokeError, SchemaChannel};

/// The one definition of *"is this path published as a verb?"*, re-exported so this crate's
/// surfaces run the SAME rule the daemon's mux surface runs.
///
/// It lives in `sprag_host::wire`, which claims to be the one definition of the wire's grammar; the
/// first version of this round wrote it out a second time here, which is the two-readers-of-one-rule
/// shape the debt sweep exists to catch — found by asking the question of this round's own code.
pub(crate) use sprag_host::wire::declares_verb;

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

/// HOW TO CALL THE VERBS THIS WINDOW'S SURFACES SERVE — the GUI's half of the published call grammar.
///
/// # The surfaces the daemon's audit cannot reach
///
/// `sprag_host::wire::SURFACES` is checked against the scene THAT crate assembles, which is how R353
/// found the plugin host an hour after the list was written. It structurally cannot see these three:
/// they hang in this window's scene, and a client driving this window addresses them directly. So the
/// list lives here, the claims live in `sprag_conformance`, and each surface's own tests apply them —
/// the same shape `declares_verb` already has.
///
/// # ⚠⚠ FIVE OF THESE EIGHT VERBS TAKE NOTHING, AND R353 SAID SPRAG HAD NONE
///
/// `FormKind`'s own documentation claimed a nullary verb was a shape "sprag has none" of. It was true
/// of the three surfaces that round had in front of it and false of the product: `open`, `execute`,
/// `accept`, `dismiss` and `activate` each ignore their `args` entirely, because what they act on is
/// the surface's own state — the palette's armed request, the prompt's answer, the hovered link. The
/// remaining three are SCALARS, and not one of the eight takes an object.
pub(crate) mod grammar {
    use sprag_host::wire::{ActionGrammar, ArgGrammar, CallForm};
    // The surface LIST is the audit's, so its two types are test-only here — the tables above are
    // production paths and need neither.
    #[cfg(test)]
    use sprag_host::wire::WireSurface;
    #[cfg(test)]
    use sprag_rpc::grammar::SurfaceAuthor;

    /// The composite event payload every one of these surfaces takes on `send` — pinion's own send
    /// wire, whose name a click arrives under (`"grid:PointerDown"`, or the bare `"PointerDown"` an
    /// RPC caller sends).
    ///
    /// ⚠ **NO VOCABULARY, and that is measured rather than assumed.** These surfaces ACCEPT any
    /// string and act on the two or three names they know, so there is no set they refuse — which is
    /// exactly what `a_constrained_argument_publishes_what_it_admits` checks, and publishing a
    /// vocabulary here would be a claim that the others are rejected. What a name MEANS is pinion's
    /// to describe; what this surface admits is any string.
    const SEND_PAYLOAD: ArgGrammar = ArgGrammar::open("event", "string");

    /// The COMMAND PALETTE's four verbs.
    const PALETTE: &[ActionGrammar] = &[
        ActionGrammar {
            action: "open",
            // Nullary: the palette opens over the app's own pane, so there is nothing for a caller to
            // name. It ARMS the open (the reducer performs it) and answers only that the request was
            // taken — the modal face at `sprag_palette_modal` is what says whether it is up.
            forms: &[CallForm::nullary()],
            from_ask: false,
        },
        ActionGrammar {
            action: "select",
            // A ROW INDEX as the whole args value — `invoke("select", 3)`. Bounded by `row_count`,
            // and a row outside it is REJECTED rather than malformed (the request was read).
            forms: &[CallForm::scalar(&ArgGrammar::open("row", "int"))],
            from_ask: false,
        },
        ActionGrammar {
            action: "execute",
            // Nullary: it runs the row the cursor is ON, which `select` or the query text chose.
            forms: &[CallForm::nullary()],
            from_ask: false,
        },
        ActionGrammar {
            action: "send",
            forms: &[CallForm::scalar(&SEND_PAYLOAD)],
            from_ask: false,
        },
    ];

    /// The CONFIRMATION prompt's three verbs.
    const CONFIRM: &[ActionGrammar] = &[
        // The two answers, each its own verb rather than a cursor move plus a key: an RPC caller says
        // which one it means. Both nullary — the sentence being answered is the surface's, and a
        // caller that could name a different one would be confirming something else.
        ActionGrammar {
            action: "accept",
            forms: &[CallForm::nullary()],
            from_ask: false,
        },
        ActionGrammar {
            action: "dismiss",
            forms: &[CallForm::nullary()],
            from_ask: false,
        },
        ActionGrammar {
            action: "send",
            forms: &[CallForm::scalar(&SEND_PAYLOAD)],
            from_ask: false,
        },
    ];

    /// The per-pane HYPERLINK oracle's two verbs.
    const HYPERLINK: &[ActionGrammar] = &[
        ActionGrammar {
            action: "send",
            forms: &[CallForm::scalar(&SEND_PAYLOAD)],
            from_ask: false,
        },
        ActionGrammar {
            action: "activate",
            // Nullary, and the reason is the one that makes this verb exist: it opens the link the
            // pane is HOVERING, and the hover is set through `intervene("hover_index", …)`. An
            // AI-first click is therefore two calls, which is deliberate — a client that could pass a
            // uri would be asking this surface to open something it never showed anybody.
            forms: &[CallForm::nullary()],
            from_ask: false,
        },
    ];

    /// The palette's grammar, for the surface that serves it.
    pub(crate) const fn palette() -> &'static [ActionGrammar] {
        PALETTE
    }

    /// The confirmation prompt's grammar, for the surface that serves it.
    pub(crate) const fn confirm() -> &'static [ActionGrammar] {
        CONFIRM
    }

    /// A pane oracle's grammar, for the surface that serves it.
    pub(crate) const fn hyperlink() -> &'static [ActionGrammar] {
        HYPERLINK
    }

    /// EVERY SURFACE THIS WINDOW SERVES, paired with the grammar it publishes.
    ///
    /// ⚠ The oracle's tag carries its PANE INDEX (`sprag_gui.pane.0`), so its entry names the stem and
    /// the audit matches by prefix — with its own check that no two tags here are prefixes of each
    /// other, since that would make one surface's verbs read as another's.
    ///
    /// ⚠ `#[cfg(test)]` because the AUDIT is the only reader: the three surfaces serve their own
    /// tables through [`palette`](self)/[`confirm`](self)/[`hyperlink`](self), which are production
    /// paths, while the LIST exists to be checked against the window's registrations. A shipped list
    /// nothing reads would be dead weight in the binary and clippy says so.
    #[cfg(test)]
    pub(crate) const SURFACES: &[WireSurface] = &[
        WireSurface {
            name: "the command palette",
            author: SurfaceAuthor::Sprag,
            tag: crate::palette::PALETTE_TAG,
            grammar: PALETTE,
            undescribed: &[],
        },
        WireSurface {
            name: "the confirmation prompt",
            author: SurfaceAuthor::Sprag,
            tag: crate::confirm::CONFIRM_TAG,
            grammar: CONFIRM,
            undescribed: &[],
        },
        WireSurface {
            name: "a pane's hyperlink oracle",
            author: SurfaceAuthor::Sprag,
            // ONE PER PANE, so the index is spelled with the schema's own placeholder idiom.
            tag: "sprag_gui.pane.<i>",
            grammar: HYPERLINK,
            undescribed: &[],
        },
        // ── PINION'S WIDGETS, which sprag REGISTERS and does not describe ────────────────────────
        //
        // ⚠⚠ **THIS WINDOW SERVES SIXTY-EIGHT VERB ADDRESSES AND SPRAG WROTE EIGHT OF THEM.** R352
        // counted "eight verbs in the GUI" and meant the three surfaces above; the audit derived from
        // the real registration list found the rest, and every one is a pinion widget type sprag
        // instantiates — verified at its construction site, not inferred from its tag:
        // `ButtonExternal` (the window and session tab strips, and the five standalone buttons),
        // `TextFieldExternal` (the find and prompt fields), `CheckboxExternal` (the regex toggle),
        // `ScrollbarExternal`, `ContextMenuExternal`, `DockReorganizeExternal`.
        //
        // They are LISTED so the set stays complete — a widget added tomorrow fails this audit until
        // somebody decides whose it is — and NOT described, because publishing a request grammar for
        // another project's widget is sprag speaking for pinion.
        // ⚠ THIS ONE IS THE PROOF THAT THE MATCHER'S FIRST RULE WAS WRONG. A bare-prefix match read
        // `sprag_palette_query` as part of `sprag_palette`, so this text field's verbs were attributed
        // to the palette and the palette's table looked like it described them. The placeholder rule
        // exposed it as the one surface still unnamed.
        upstream(
            "the palette's query field",
            crate::palette::PALETTE_FIELD_TAG,
        ),
        upstream("the find bar's query field", crate::find::FIND_FIELD_TAG),
        upstream("the find bar's regex toggle", "sprag_find_regex"),
        upstream("the prompt's field", crate::prompt::PROMPT_FIELD_TAG),
        upstream("the context menu", crate::ctxmenu::CTXMENU_TAG),
        upstream("the dock reorganizer", crate::split::DOCK_REORGANIZE_TAG),
        // ⚠⚠⚠ **THIS ONE ARRIVED BY MOVING THE PIN, AND THE WIDGET DID NOT CHANGE.** pinion R1637
        // (`a call must be declared first`) reclassified three of `DockPanelExternal`'s fields from
        // READ to INVOKE — its own comment says they *"were declared readable, so `$schema` said
        // 'query me' about a name only `invoke` answers"*. They were always verbs; only the
        // declaration was wrong. So every dock leaf sprag registers began serving verbs, and this
        // audit reported a surface it had never had reason to name.
        //
        // That is the audit doing its job, in the words two paragraphs up: *a widget added tomorrow
        // fails this audit until somebody decides whose it is.* Decided: pinion's, like every entry
        // around it. ⚠ Verified at the construction site rather than inferred from the tag —
        // `split.rs` mounts each leaf with `panel_id = terminal-{i}` and pinion's dock surface
        // builds one `DockPanelExternal` per leaf, aligned with `panel_ids()[i]`.
        upstream("a docked pane's panel", "terminal-<i>"),
        upstream("a pane's scrollbar", "sprag_gui.scrollbar.<i>"),
        upstream("a window tab", "sprag_gui.wtab.<i>"),
        upstream("the new-window button", "sprag_gui.wnew"),
        upstream("the close-window button", "sprag_gui.wclose"),
        upstream("a session tab", "sprag_gui.stab.<i>"),
        upstream("the new-session button", "sprag_gui.snew"),
        upstream("a session's kill button", "sprag_gui.skill.<i>"),
        upstream("the kill-session confirm button", "sprag_gui.skillok"),
        upstream("the kill-session cancel button", "sprag_gui.skillno"),
    ];

    /// A surface sprag REGISTERS and pinion WROTE — listed, never described.
    ///
    /// A helper rather than fourteen literal structs, because every field but the name and the tag is
    /// the same by definition: an upstream surface has no grammar of sprag's and nothing to exempt.
    #[cfg(test)]
    const fn upstream(name: &'static str, tag: &'static str) -> WireSurface {
        WireSurface {
            name,
            author: SurfaceAuthor::Upstream,
            tag,
            grammar: &[],
            undescribed: &[],
        }
    }
}
