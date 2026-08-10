//! A closed set's WIRE VOCABULARY: the array of every word the type can be spelled with on the
//! wire, projected from [`ALL`](crate::closed_set!) rather than written a second time.
//!
//! # The gap between the two devices
//!
//! [`closed_set!`](crate::closed_set!) answers ONE question — *which variants are there* — and its
//! own docs say why it stops: *"No `FromStr`, no `Display`, no wire word. Every one of these enums
//! spells itself for a different audience … folding those into one macro would put this file in
//! the business of deciding how a swap outcome reads on a wire."* That decision stands and this
//! module keeps it: the SPELLING still lives on the type, as the `const fn` it always was.
//!
//! What was missing is the JOIN. A surface that wants to publish *"this argument is one of
//! `left`, `right`, `up`, `down`"* needs the words as data — a `&'static [&'static str]` — and
//! until this existed there were only two ways to get one, both of them the defect this workspace
//! keeps paying for:
//!
//! * type the words again beside the declaration, which is the hand-written list a new arm is left
//!   out of (R335, R342, R348, R349 — four rounds, four different lists);
//! * build the list at runtime, which no schema can hold: a declaration is `const`.
//!
//! [`wire_words!`](crate::wire_words!) is the third way. It walks `ALL` in const context and calls
//! the type's own spelling on each member, so **the published vocabulary and the vocabulary the
//! parser admits are the same array**, and an arm added to the enum appears on the wire in the
//! same compile that adds it — there is nowhere for a stale copy to live.
//!
//! # What it refuses at COMPILE TIME
//!
//! Two arms spelled the same way. That is not a hypothetical tidiness rule: a duplicated word makes
//! the published set SHORTER than the type, so a client enumerating it is told a value is
//! unreachable that the parser accepts, and — worse in the other direction — whichever arm parses
//! first silently swallows the other's requests. A `const` duplicate check turns that into a build
//! failure at the declaration, which is the only place anybody can see both spellings at once.
//!
//! An EMPTY word is refused for the same reason: it is what a spelling returns when somebody adds
//! an arm and leaves the match arm's string for later.

/// Whether two `&str` hold the same bytes, in `const` context.
///
/// `==` on `&str` is not `const` on stable, and the duplicate check
/// [`wire_words!`](crate::wire_words!) performs has to run at compile time or it is just another
/// test somebody can forget to write. `str::as_bytes` IS `const`, so the comparison is spelled over
/// the bytes.
///
/// Public because the macro expands into the caller's crate and its expansion names this.
#[must_use]
pub const fn same_word(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut at = 0;
    while at < left.len() {
        if left[at] != right[at] {
            return false;
        }
        at += 1;
    }
    true
}

/// Declare `WIRE_WORDS` for a [`closed_set!`](crate::closed_set!) enum: every variant's wire word,
/// projected from `ALL` through the type's own spelling.
///
/// The spelling must be a `const fn` taking `self` and answering `&'static str` — the shape
/// `PaneDir::wire_str` and `Forward::word` already have. It stays on the type, because the type is
/// where the audience is decided; this only reads it.
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
///
/// impl Door {
///     /// This door's wire word.
///     #[must_use]
///     pub const fn wire_str(self) -> &'static str {
///         match self {
///             Self::In => "in",
///             Self::Out => "out",
///         }
///     }
/// }
///
/// sprag_vt::wire_words!(Door: wire_str);
/// assert_eq!(Door::WIRE_WORDS, ["in", "out"]);
/// ```
///
/// # Why an inherent const and not a trait
///
/// A trait would need every one of these enums to implement it, and the implementations would be
/// identical — which is what a macro is for. It would also make the words reachable only through a
/// generic bound, where a schema declaration needs them as a plain `&'static [&'static str]` in a
/// `const` position. The inherent const is directly usable there: `&Door::WIRE_WORDS`.
///
/// # The two build failures it produces
///
/// A DUPLICATE word and an EMPTY one, both at the declaration, both `const`. Neither is reachable
/// as a runtime test on a `const` this size — by the time a test could look, the schema built from
/// it has already been published. So they are driven as `compile_fail` doctests, **each one a copy
/// of the passing example above with a single word changed**: a `compile_fail` block passes for any
/// compile error at all, so the control is the example it was copied from, and the only difference
/// between the two is the mistake being claimed.
///
/// Two arms spelled the same way:
///
/// ```compile_fail
/// sprag_vt::closed_set! {
///     #[derive(Clone, Copy, PartialEq, Eq, Debug)]
///     pub enum Door {
///         In,
///         Out,
///     }
/// }
///
/// impl Door {
///     pub const fn wire_str(self) -> &'static str {
///         match self {
///             Self::In => "in",
///             // The one changed line: the second arm answers the FIRST arm's word.
///             Self::Out => "in",
///         }
///     }
/// }
///
/// sprag_vt::wire_words!(Door: wire_str);
/// ```
///
/// An arm whose word was left for later:
///
/// ```compile_fail
/// sprag_vt::closed_set! {
///     #[derive(Clone, Copy, PartialEq, Eq, Debug)]
///     pub enum Door {
///         In,
///         Out,
///     }
/// }
///
/// impl Door {
///     pub const fn wire_str(self) -> &'static str {
///         match self {
///             Self::In => "in",
///             // The one changed line.
///             Self::Out => "",
///         }
///     }
/// }
///
/// sprag_vt::wire_words!(Door: wire_str);
/// ```
#[macro_export]
macro_rules! wire_words {
    // ⚠ THE SUBSET FORM, and the defect it exists for is measured. `resize-window`'s `from`
    // argument takes a `WindowSize` and REFUSES `manual` — folding the attached clients under a
    // policy that means "a pin" is not a thing to ask for — so the vocabulary that argument admits
    // is three words where the type has four. Publishing the type's whole `WIRE_WORDS` there was an
    // affirmative false statement, and the gate that drives every published word through the daemon
    // caught it on its first run.
    //
    // The fix is not a hand-typed short list, which would be the same defect with a smaller number.
    // It is a PREDICATE: a `const fn` on the type saying which members are in this subset, read
    // BOTH by the parser that admits them and by the array that publishes them. One rule, two
    // readers, no way for them to disagree.
    ($set:ty : $spelling:ident, $subset:ident where $admits:ident) => {
        impl $set {
            /// The wire words of the members this argument admits — a SUBSET of
            /// `WIRE_WORDS`, chosen by the same predicate the parser gates on.
            ///
            /// Projected from `ALL` like the full vocabulary, so a variant added to the type joins
            /// this in the same compile if the predicate admits it, and cannot be forgotten if it
            /// does.
            pub const $subset: [&'static str; {
                let mut admitted = 0;
                let mut at = 0;
                while at < <$set>::ALL.len() {
                    if <$set>::ALL[at].$admits() {
                        admitted += 1;
                    }
                    at += 1;
                }
                admitted
            }] = {
                // The count is spelled twice — here and in the length above — because naming the
                // const being defined is a cycle, and a macro cannot mint a second name to hold it
                // in. Both copies are one expansion of one token sequence, so they cannot differ.
                let mut words = [""; {
                    let mut admitted = 0;
                    let mut at = 0;
                    while at < <$set>::ALL.len() {
                        if <$set>::ALL[at].$admits() {
                            admitted += 1;
                        }
                        at += 1;
                    }
                    admitted
                }];
                let (mut at, mut into) = (0, 0);
                while at < <$set>::ALL.len() {
                    if <$set>::ALL[at].$admits() {
                        words[into] = <$set>::ALL[at].$spelling();
                        into += 1;
                    }
                    at += 1;
                }
                words
            };
        }

        // The same three refusals the full form makes, over the subset — a member admitted by the
        // predicate is published, so an empty or duplicated word among THOSE is exactly as wrong.
        // ⚠ The first version of this arm checked only that the subset was non-empty, which is the
        // shape a spelling with an unreachable "" arm slips through: the word is legal for the
        // members the predicate excludes and would be a hole in the vocabulary if it ever were not.
        const _: () = {
            if <$set>::$subset.is_empty() {
                panic!(
                    "this subset's predicate admits no member of the closed set, so the argument \
                     it publishes would offer a client nothing it may send",
                );
            }
            let words = <$set>::$subset;
            let mut outer = 0;
            while outer < words.len() {
                if words[outer].is_empty() {
                    panic!(
                        "a member this subset ADMITS spells itself as the EMPTY string, so the \
                         vocabulary it publishes has a hole no client can address",
                    );
                }
                let mut inner = outer + 1;
                while inner < words.len() {
                    if $crate::wire_words::same_word(words[outer], words[inner]) {
                        panic!(
                            "TWO MEMBERS THIS SUBSET ADMITS SHARE ONE WIRE WORD, so the published \
                             vocabulary is shorter than the set the parser accepts.",
                        );
                    }
                    inner += 1;
                }
                outer += 1;
            }
        };
    };

    ($set:ty : $spelling:ident) => {
        impl $set {
            /// Every variant's WIRE WORD, in declaration order — the vocabulary a client may send
            /// or must be able to read, as data a schema can publish.
            ///
            /// Projected from `ALL` through this type's own spelling by `sprag_vt::wire_words!`, so
            /// it holds one word per variant by construction: a variant added to the enum lands here
            /// in the same compile, and the length is `ALL.len()` rather than a number anybody typed.
            ///
            /// ⚠ The macro's name is a CODE SPAN and not a link, because this doc is generated into
            /// whichever crate expands the macro: `sprag_vt::wire_words!` does not resolve inside
            /// `sprag_vt` itself, and `crate::wire_words!` does not resolve outside it. The first
            /// in-crate expansion (`ClipboardTarget`) is what found that, on the doc gate.
            pub const WIRE_WORDS: [&'static str; <$set>::ALL.len()] = {
                let mut words = [""; <$set>::ALL.len()];
                let mut at = 0;
                while at < words.len() {
                    words[at] = <$set>::ALL[at].$spelling();
                    at += 1;
                }
                words
            };
        }

        // THE TWO BUILD FAILURES. A `const` block is evaluated for its side effect of being
        // evaluated: a panic here is a compile error at the declaration, which is where both
        // mistakes are visible and the only place either is cheap to fix.
        const _: () = {
            let words = <$set>::WIRE_WORDS;
            let mut outer = 0;
            while outer < words.len() {
                if words[outer].is_empty() {
                    panic!(
                        "a variant of this closed set spells itself as the EMPTY string, so the \
                         vocabulary this publishes has a hole in it that no client can address",
                    );
                }
                let mut inner = outer + 1;
                while inner < words.len() {
                    if $crate::wire_words::same_word(words[outer], words[inner]) {
                        panic!(
                            "TWO VARIANTS OF THIS CLOSED SET SHARE ONE WIRE WORD. The published \
                             vocabulary would be shorter than the type, so one arm is unreachable \
                             from the wire while the other silently answers for it.",
                        );
                    }
                    inner += 1;
                }
                outer += 1;
            }
        };
    };
}

#[cfg(test)]
mod tests {
    crate::closed_set! {
        /// A three-word set, for the properties below.
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

    impl Sample {
        /// This member's wire word.
        pub const fn wire_str(self) -> &'static str {
            match self {
                Self::One => "one",
                Self::Two => "two",
                Self::Three => "three",
            }
        }
    }

    crate::wire_words!(Sample: wire_str);

    impl Sample {
        /// Whether this member is in the subset below — the shape a parser gates on.
        pub const fn is_odd(self) -> bool {
            matches!(self, Self::One | Self::Three)
        }
    }

    crate::wire_words!(Sample: wire_str, ODD_WORDS where is_odd);

    /// A SUBSET publishes the members its predicate admits, and nothing else.
    ///
    /// The property that matters is that the length is COUNTED by the predicate rather than typed:
    /// a fourth member admitted by `is_odd` would land here without anybody editing this file,
    /// which is the whole reason an argument that takes a subset gets a predicate instead of a
    /// short hand-written list.
    #[test]
    fn a_subset_holds_the_members_its_predicate_admits() {
        assert_eq!(Sample::ODD_WORDS, ["one", "three"]);
        assert_eq!(
            Sample::ODD_WORDS.len(),
            Sample::ALL.iter().filter(|m| m.is_odd()).count(),
            "counted by the predicate, over the whole type",
        );
        for word in Sample::ODD_WORDS {
            assert!(
                Sample::WIRE_WORDS.contains(&word),
                "{word} is one of the type's own words",
            );
        }
    }

    /// The words are the whole type, in declaration order, and the length is `ALL`'s.
    ///
    /// The property that matters is the JOIN: nothing here re-states the variant list, so this is
    /// asserting that the projection reached every member rather than that somebody typed three
    /// strings correctly.
    #[test]
    fn the_words_are_all_projected_through_the_types_own_spelling() {
        assert_eq!(Sample::WIRE_WORDS, ["one", "two", "three"]);
        assert_eq!(
            Sample::WIRE_WORDS.len(),
            Sample::ALL.len(),
            "one word per variant, counted from ALL",
        );
        for (word, member) in Sample::WIRE_WORDS.iter().zip(Sample::ALL) {
            assert_eq!(
                *word,
                member.wire_str(),
                "each word is the one the type spells, in the type's order",
            );
        }
    }

    /// The words are usable where a SCHEMA needs them: a `&'static [&'static str]` in a `const`.
    ///
    /// This is the whole point of the device and it is a compile-time property, so the assertion is
    /// that the borrow below EXISTS — a runtime `Vec` would have satisfied every other test here
    /// and could not have been declared.
    #[test]
    fn the_words_are_a_static_slice_a_declaration_can_hold() {
        const PUBLISHED: &[&str] = &Sample::WIRE_WORDS;
        assert_eq!(PUBLISHED, ["one", "two", "three"]);
    }

    /// `same_word` compares bytes, including the two ways a naive length-only check would be wrong.
    ///
    /// It is the duplicate check's whole instrument, and the check runs at COMPILE time where no
    /// test can watch it — so the instrument itself is what gets driven here. R351's rule: an
    /// instrument is a claim.
    #[test]
    fn same_word_is_byte_equality() {
        assert!(super::same_word("left", "left"));
        assert!(!super::same_word("left", "lef"));
        assert!(!super::same_word("left", "left "));
        assert!(
            !super::same_word("up", "on"),
            "same length, different bytes"
        );
        assert!(super::same_word("", ""));
    }
}
