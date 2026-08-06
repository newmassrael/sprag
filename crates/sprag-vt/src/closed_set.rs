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
#[macro_export]
macro_rules! closed_set {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $(
                $(#[$vmeta:meta])*
                $variant:ident
            ),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis enum $name {
            $(
                $(#[$vmeta])*
                $variant,
            )+
        }

        impl $name {
            /// Every variant, in declaration order.
            ///
            /// Generated with the enum from ONE variant list (`sprag_vt::closed_set!`), so it
            /// cannot be missing a variant and its length cannot disagree with its contents. A caller
            /// that iterates this is iterating the whole type, and a ratchet built on it stays honest
            /// when a variant is added.
            pub const ALL: [Self; $crate::closed_set!(@count $($variant)+)] = [
                $(Self::$variant,)+
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

    /// Docs and attributes survive, so a closed set reads as the enum it is.
    #[test]
    fn the_derives_pass_through() {
        // `Debug` and `PartialEq` come from the pass-through `#[derive]`; if the meta capture were
        // dropped this would not compile.
        assert_eq!(format!("{:?}", Sample::Two), "Two");
    }
}
