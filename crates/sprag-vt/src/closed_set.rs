//! A CLOSED SET: an enum and the array of every one of its variants, declared ONCE.
//!
//! # The defect this removes
//!
//! NINE enums in this workspace carried a hand-written `pub const ALL` — a caller asking
//! for "every direction", "every outcome word", "every subject" iterates it, and a RATCHET walks it
//! demanding a reader per member. All of that rests on the array naming every variant, and **nothing
//! checked that it did**: a variant added later leaves the array stale, the length is a literal the
//! author retypes, and every ratchet built on top goes on passing while covering one fewer case.
//!
//! It was registered three times — as `PaneDir::ALL`'s residual (R299/R300), as `SwapHow`'s
//! (R301) and as `PlaceHow`'s "a fifth array literal no compiler checks" (R310) — and the count in
//! the register was **five** when the tree already held **eight**. A gap that grows while it is
//! being described is a gap nobody is measuring — the ninth (`EventKind::ALL`) was a SLICE
//! rather than an array and had escaped every count.
//!
//! # Why the macro DECLARES the enum
//!
//! Because any form that takes the variant list a second time is the same defect wearing a hat. A
//! helper handed `PaneDir { Left, Right, Up, Down }` beside the real declaration drifts exactly as
//! the array did. So the variant list is written once and the enum, the array and its length are
//! all generated from it — [`closed_set!`] cannot produce an `ALL` that is missing a variant,
//! because there is nowhere for one to hide.
//!
//! Attributes and docs pass through on both the type and each variant, so a closed set reads as the
//! enum it is: the macro adds a fact, it does not take the vocabulary away.
//!
//! # What it deliberately does NOT do
//!
//! No `FromStr`, no `Display`, no wire word. Every one of these enums spells itself for a different
//! audience — `wire_str`, `heading`, `name`, `Display` — and folding those into one macro would put
//! this file in the business of deciding how a swap outcome reads on a wire. It answers one
//! question: **which variants are there.**

/// Declare an enum together with `ALL`, the array of every one of its variants.
///
/// ```
/// sprag_vt::closed_set! {
///     /// A door.
///     #[derive(Clone, Copy, PartialEq, Eq, Debug)]
///     pub enum Door {
///         /// The way in.
///         In,
///         /// The way out.
///         Out,
///     }
/// }
/// assert_eq!(Door::ALL, [Door::In, Door::Out]);
/// ```
///
/// The generated `ALL` is a `pub const [Self; N]` where `N` is COUNTED from the variant list rather
/// than written, so the length cannot disagree with the contents either.
///
/// A variant that CARRIES something declares one sample beside its field types, and that sample is
/// what it contributes to `ALL`:
///
/// ```
/// sprag_vt::closed_set! {
///     /// Why a door would not open.
///     #[derive(Clone, Copy, PartialEq, Eq, Debug)]
///     pub enum Refused {
///         /// It is locked.
///         Locked,
///         /// The lock answered with a number.
///         Errno(i32) = (13),
///     }
/// }
/// assert_eq!(Refused::ALL, [Refused::Locked, Refused::Errno(13)]);
/// ```
///
/// A variant with NAMED fields declares its sample the same way, in braces:
///
/// ```
/// sprag_vt::closed_set! {
///     /// Why a door would not open.
///     #[derive(Clone, PartialEq, Eq, Debug)]
///     pub enum Refusal {
///         /// It is locked.
///         Locked,
///         /// A guard refused, and said which door and to whom.
///         Guarded { door: String, whom: String } = { door: String::new(), whom: String::new() },
///     }
/// }
/// assert_eq!(Refusal::ALL.len(), 2);
/// ```
///
/// ⚠ Named fields were the form this could NOT express, and the omission was load-bearing:
/// `sprag_plugin::PaneError` — **the failure sentence an agent reads** — carries its readiness cause
/// in a `NeverReady { wanted, instead }`, so it could not be a closed set, so the gate over every
/// variant's sentence stayed a hand-written list of five. A macro that covers most shapes leaves the
/// types it cannot describe to the very hand-written lists it exists to abolish.
///
/// Without this form such an enum simply has no `ALL`, which is how `sprag_terminal::Unmeasured`
/// came to be enumerated by hand on three surfaces — and how a fourth variant added to it reached
/// an agent's screen with no test mentioning it. A ratchet that cannot describe the type it guards
/// is not guarding it.
#[macro_export]
macro_rules! closed_set {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident
                $( ( $($field:ty),+ $(,)? ) = ( $($sample:expr),+ $(,)? ) )?
                $( { $($(#[$fmeta:meta])* $named:ident : $nty:ty),+ $(,)? } = { $($sname:ident : $ssample:expr),+ $(,)? } )?
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $(
                $(#[$vmeta])*
                $variant
                    $( ( $($field),+ ) )?
                    $( { $($(#[$fmeta])* $named : $nty),+ } )?,
            )+
        }

        impl $name {
            /// Every variant, in declaration order.
            ///
            /// Generated with the enum from ONE variant list (`sprag_vt::closed_set!`), so it
            /// cannot be missing a variant and its length cannot disagree with its contents. A caller
            /// that iterates this is iterating the whole type, and a ratchet built on it stays honest
            /// when a variant is added.
            ///
            /// # For a set with a variant that CARRIES something
            ///
            /// That variant contributes the SAMPLE its declaration names, so this is one value per
            /// variant rather than every value of the type — the two are the same thing for a set of
            /// unit variants and are not for the other kind. It is still the total the ratchets
            /// want: what they ask is *does every variant have a reader*, and a variant that
            /// carries a payload answers that question with any one inhabitant.
            pub const ALL: [Self; $crate::closed_set!(@count $($variant)+)] = [
                $(
                    Self::$variant
                        $( ( $($sample),+ ) )?
                        $( { $($sname : $ssample),+ } )?,
                )+
            ];
        }
    };

    // The length, counted from the variant list rather than typed. Recursive rather than
    // `[$(stringify!($v)),+].len()` because the result has to be usable as an ARRAY LENGTH, which
    // is a const-generic position a slice method cannot reach on stable.
    (@count) => { 0usize };
    (@count $head:ident $($tail:ident)*) => { 1usize + $crate::closed_set!(@count $($tail)*) };
}

#[cfg(test)]
mod tests {
    crate::closed_set! {
        /// A three-variant set, for the properties below.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Sample {
            /// The first.
            One,
            /// The second.
            Two,
            /// The third.
            Three,
        }
    }

    /// `ALL` holds every variant, in declaration order, and its length is counted rather than typed.
    #[test]
    fn all_is_the_whole_type_in_declaration_order() {
        assert_eq!(Sample::ALL, [Sample::One, Sample::Two, Sample::Three]);
        assert_eq!(Sample::ALL.len(), 3);
    }

    /// `ALL` names each variant EXACTLY ONCE — no gap and no repeat.
    ///
    /// The gap is what the hand-written arrays kept getting wrong, and the compiler now prevents it
    /// structurally: there is one variant list, so a variant absent from `ALL` cannot exist. The
    /// REPEAT is what this asserts, because a macro that expanded its list twice would produce an
    /// `ALL` that is still the right type and the wrong set — and every ratchet built on it would
    /// count one member twice while still "covering everything".
    #[test]
    fn all_names_each_variant_exactly_once() {
        for variant in Sample::ALL {
            assert_eq!(
                Sample::ALL.iter().filter(|held| **held == variant).count(),
                1,
                "{variant:?} appears once in ALL",
            );
        }
    }

    crate::closed_set! {
        /// A set where one variant CARRIES something — the form `sprag_terminal::Unmeasured` needs.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Mixed {
            /// Nothing to say.
            Silent,
            /// The kernel said a number.
            Errno(i32) = (13),
            /// Two of them.
            Pair(u8, u8) = (1, 2),
            /// NAMED fields, each with its own doc — the shape this could not declare, which is
            /// why `sprag_plugin::PaneError` had no `ALL` and the gate over the sentence an agent
            /// reads walked five literals instead.
            Named {
                /// Which one.
                which: u8,
                /// And whether it said so.
                said: bool,
            } = { which: 3, said: true },
        }
    }

    /// A carrying variant is in `ALL` like any other, holding the sample its declaration names.
    ///
    /// The property that matters is the LENGTH: before this form existed, such an enum could have
    /// no `ALL` at all, so every surface that wanted "each reason" wrote the list again by hand and
    /// a new variant was added to none of them.
    #[test]
    fn a_variant_that_carries_something_is_still_in_all() {
        assert_eq!(
            Mixed::ALL,
            [
                Mixed::Silent,
                Mixed::Errno(13),
                Mixed::Pair(1, 2),
                // ⚠ THE VALUE, field for field, not the length. `ALL.len() == 4` passes for an
                // expansion that put the wrong field in the wrong slot or defaulted one whose
                // sample was dropped — and a set whose inhabitant is silently not the one its
                // author wrote is a ratchet driving something nobody chose.
                Mixed::Named {
                    which: 3,
                    said: true,
                },
            ],
        );
        assert_eq!(
            Mixed::ALL.len(),
            4,
            "counted from the variant list, as ever"
        );
    }

    /// Docs and attributes survive, so a closed set reads as the enum it is.
    #[test]
    fn the_derives_pass_through() {
        // `Debug` and `PartialEq` come from the pass-through `#[derive]`; if the meta capture were
        // dropped this would not compile.
        assert_eq!(format!("{:?}", Sample::Two), "Two");
    }
}
