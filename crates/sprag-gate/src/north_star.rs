//! **WHAT THE NORTH STAR IS COUNTING** — register item 823, and a claim no reader of the ledger
//! can make by reading it.
//!
//! # ⛔⛔⛔⛔⛔ Two predicates, and neither could say what "zero" meant
//!
//! The north star is *"零 unpaid ai-loop items in section A of the ledger"*. Measured 2026-09-02,
//! that population had **two** answers and they were different questions:
//!
//! * a MACHINE predicate — *the block carries the string `ai_loop` and no closing word* — which
//!   returned **13**;
//! * a HAND-KEPT list in the memory index — *what is left after reading* — which returned **45**.
//!
//! Every one of the 13 was inside the 45, so the machine could not add anything; and the 32 it
//! could not see split cleanly in two, which is what this module is shaped by:
//!
//! * **3 read as CLOSED although they are open** (470, 738, 745) — 470 because it says
//!   *"WHY THIS ITEM IS **NOT** CLOSED"* and the word `CLOSED` is in that sentence; 738 and 745
//!   because a PARTIAL payment writes `완납` in a section heading while a limb of the item is still
//!   owed.
//! * **29 carry the string `ai_loop` nowhere at all** — an item about the loop written without
//!   naming it.
//!
//! ⚠⚠⚠⚠⚠ **AND THE FIX IS NOT A BETTER WORD LIST.** The counting notes had already refuted that
//! direction twice, and this round refuted it a third time: adding `갚았다` to the closing
//! vocabulary — which a round genuinely needed, since an item really had been closed with that word
//! — is what makes the list longer, never right. A predicate over PROSE is a predicate over what
//! somebody happened to type.
//!
//! # ⭐ So the population is a MARK, and the words are only an alarm
//!
//! ⚠ The links below are written WHOLE — `crate::north_star::…` — and the short spellings are not
//! an option: this module's docs are the file's `//!` JOINED to the `///` on `lib.rs`'s `pub mod`
//! line, and rustdoc resolves the pair from the crate root, where `TAG` is not in scope. Measured
//! here 2026-09-02 (six broken links, doc gate red); [`crate::sweep`] carries the same note from
//! the day it paid for the same thing.
//!
//! An item states its own membership on one line, [`crate::north_star::TAG`], whose value is one of
//! [`crate::north_star::Tag`]. Nothing
//! else in the ledger can produce that token — measured: zero occurrences before this existed — so
//! it cannot be written by an item that merely MENTIONS another item's status, which is the exact
//! way `완납` once closed the wrong block.
//!
//! The prose predicate survives in one role only: [`crate::north_star::Fault::UntaggedCandidate`]
//! uses it to demand a
//! mark IMMEDIATELY of any open item that names the loop. It can be wrong in both directions and
//! neither is a hole — being wrong the loose way costs a mark somebody has to write, and being
//! wrong the tight way (a false close, exactly 470's shape) drops the item into
//! [`crate::north_star::Reading::unclassified`], which is a debt with a ratchet on it rather than
//! a silence.
//!
//! # ⚠⚠ Why an UNMARKED item is not "not in the population"
//!
//! That would be the escape hatch that retires the gate: everything is out by default and the count
//! is zero forever. So the reading carries the unmarked items as their own number, the ledger
//! DECLARES that number on the [`crate::north_star::DECLARATION`] line, and a count above the
//! declaration is [`crate::north_star::Fault::RatchetGrew`]. A new item added without a mark raises
//! the count and reds; the standing
//! backlog is paid down by reading it. **Zero is reachable and it means the population is total.**

use std::collections::BTreeMap;
use std::fmt;

/// The line an item states its north-star membership on: `@ns: <value>`, at any indentation.
///
/// ⚠ Deliberately not a word anybody writes by accident, and not a word an item can write ABOUT
/// another item. That is the whole difference from the closing vocabulary this replaces — `완납`
/// appears in sentences like *"821 completed"* filed under a different number, and it closed the
/// wrong block once (item 721).
///
/// # ⛔⛔⛔⛔⛔ A MARK IS A LINE THAT STARTS WITH THIS, never a sentence containing it
///
/// **Measured the hour this was introduced.** Register item 823's own entry explains the scheme it
/// was paid with, and to do that it quotes the token — *"모집단 = `@ns: open` 인 항목"*. A reading
/// that took the token from anywhere in the line turned three sentences OF THE DOCUMENTATION into
/// three malformed marks, and the ledger went red for describing itself.
///
/// That is the counting notes' own warning arriving in a new place: *"말하면 술어가 뒤집힌다"* —
/// a predicate over prose is broken by prose that talks about the predicate. So a mark must be the
/// whole line's business: leading whitespace, then this, then the value.
pub const TAG: &str = "@ns:";

/// The line the ledger declares its own unmarked count on: `@ns-unclassified: <n>`.
///
/// ⚠⚠ The number lives in the LEDGER rather than in this crate for the reason a written-down
/// expectation always rots invisibly (see [`crate::sweep`]): here the thing being counted and the
/// thing declaring the count are the same file, so a round that marks ten items and forgets to
/// lower the declaration is merely not credited, while a round that adds an item without marking it
/// goes red. Only one such line may exist.
pub const DECLARATION: &str = "@ns-unclassified:";

/// The line an item states its SEVERITY on: `@sev: <value>`, at any indentation.
///
/// Register item 833(1), the owner's decision of 2026-09-02: *"크리티컬한 문제만 먼저 갚고,
/// 나머지는 우선순위를 낮추고 북극성을 목표로 나아가도록"*.
///
/// # ⚠⚠⚠⚠⚠ WHY A MARK, WHEN THE LEDGER ALREADY HAS A SEVERITY SIGNAL THAT NOTHING READS
///
/// It has `⚠` and `⛔` glyphs, and register item 659 measured what they are worth: *"`⚠` 의 개수가
/// 사실상 유일한 신호인데 그건 **문자열이지 필드가 아니고**, 무엇도 그것을 읽지 않는다"*. Counting
/// glyphs would be a predicate over prose, which is the mistake [`TAG`]'s own docs record being
/// paid for twice. So severity gets a PLACE, exactly as membership did.
///
/// # ⛔⛔ TWO VALUES, BECAUSE THE DECISION THAT ASKED FOR THIS HAS TWO
///
/// The owner's sentence splits the world in two — critical, and the rest whose priority drops. A
/// three- or five-level scale would be a finer answer to a question nobody asked, and every level
/// nobody can define is a level items land in by default.
///
/// # ⚠⚠⚠ AND IT IS A GATE, NOT A SORT — register item 659's counter-argument, kept
///
/// 659 measured the cost of always chasing the sharpest thing: *"늘 가장 날카로운 것만 쫓는 루프는
/// 축을 **끝내지 못한다**"*, because items that share a seam are cheap together and a severity sort
/// scatters them. This scheme does not sort. It says: while anything is [`Severity::Critical`],
/// take from those; when none is, the population is worked in whatever order coheres. The critical
/// set is meant to be small and to empty.
pub const SEVERITY: &str = "@sev:";

/// The line the ledger declares its own count of open items with no severity: `@sev-unclassified:
/// <n>`.
///
/// ⚠ Only OPEN items are counted here. A paid or out item needs no severity, and demanding one
/// would make the backlog grow every time something is closed — a ratchet that punishes payment.
pub const SEVERITY_DECLARATION: &str = "@sev-unclassified:";

/// The line an item states its PARENT on: `@from: <item number>`, or `@from: none` for a debt
/// nobody found while paying something else.
///
/// Register item 833(2), the owner's decision of 2026-09-02: *"부채의 부채는 몇 depth까지 갚을지
/// scxml에 지정할수있게하고 default로 1 depth로해"*.
///
/// # ⛔⛔⛔⛔⛔ WHY THE DEPTH IS THE ITEM'S AND NOT THE RUN'S
///
/// The first build of this counted **re-aims inside one run** and reset at every run boundary.
/// Measured the evening it shipped: the five runs placed that day all took a milestone registered
/// THAT SAME DAY, three of them registered hours earlier by the watcher placing the run. The chain
/// the owner asked to bound was never inside a run — it crossed run boundaries, and a fresh run
/// starting at zero is exactly the laundering step that made it invisible.
///
/// So depth is carried by the DEBT: an item found while paying `X` says so, and its depth is one
/// more than `X`'s. Nothing a run does can reset that, because a run does not own it.
///
/// # ⛔⛔⛔⛔ CAUSED BY, NEVER MERELY MET WHILE — and the difference is the whole scheme
///
/// **Handed over by `sprag-14` the hour this was written, and it is a defect this would have had.**
/// A round paying `X` runs into two different things and only one of them is `X`'s child:
///
/// * it BROKE something, or its own repair left a residue → that debt exists *because* `X` was
///   paid, and it is one step down;
/// * it walked into a red that was **already there** — someone else's, or HEAD's — and merely
///   noticed it. That debt is a ROOT. Nothing created it; a round happened to be standing there.
///
/// ⚠⚠ Counting the second as a child is not a rounding error, it inverts the cap: **every
/// pre-existing debt anybody stumbles over gets pushed further down the chain**, and the deeper it
/// is pushed the longer it is deferred. The oldest debts would sink fastest. That is the exact
/// opposite of what item 833 exists to do.
///
/// The measured pair, from that round: item 836 was a mutation build left in `target/` that became
/// the dock's app — the payment *made* it, so `@from: 825`. Item 837 was a red already standing in
/// HEAD that the same suite happened to reach — so `none`, though both were written in one hour by
/// one round.
///
/// # ⚠⚠ `none` IS A VALUE, NOT AN ABSENCE
///
/// A debt nothing created — found by a person, by CI, or by a watcher reading the product — is a
/// ROOT and says `none`. Leaving the line off instead would make "nobody wrote it down" and
/// "nothing created it" the same reading, which is the distinction [`Fault::UnknownTag`]'s
/// neighbours already exist to keep. Unstated items are carried by [`Reading::unrooted`] under
/// their own ratchet.
pub const PARENT: &str = "@from:";

/// ⛔⛔⛔⛔⛔ **THE WORDS THIS LEDGER USES FOR *I MET IT WHILE PAYING SOMETHING ELSE*** — register
/// item 896, and the vocabulary [`PARENT`]'s own *CAUSED BY, NEVER MERELY MET WHILE* section
/// argues about while nothing read it.
///
/// # ⛔⛔⛔⛔⛔ The rule was written into this file and the parser threw the reason away
///
/// `Parent::parse` — SPELLED, not linked: it is private and this constant is public, so a link is
/// `private_intra_doc_links` under `-D warnings` (register item 365, and the commit hook refused
/// this file for it) — reads the FIRST WORD of the value, the number, and discards the sentence
/// after it. So the distinction that section calls *the whole scheme* was enforced by whoever
/// happened to be writing the line, which is this workspace's rule 10 exactly: prose nobody
/// measures.
///
/// **Measured 2026-09-05 over the ledger's own 78 `@from:` lines**, and the vocabulary is not
/// invented here — it is counted:
///
/// | form | on `@from: none` | on `@from: <n>` |
/// | --- | ---: | ---: |
/// | `마주친` / `마주쳤다` | **9** | **1** |
/// | `드러났다` | 0 | 2 |
/// | `넘겨줬다` | 0 | 1 |
///
/// ⇒ **Nine lines say *met while* and root themselves; four say *met while* and name a parent.**
/// The nine are the convention — each carries its own pre-existence evidence (*그 부재는 X 전부터
/// 있었다*) — so the four are slips, and one of them is `@from: 852 — 852 를 재느라 GUI 로그를 읽다
/// «마주친» 것이다` sitting eight lines below a sibling that writes `@from: none` for the same
/// sentence. No round ever argued for the numbered form; it was typed.
///
/// ⇒ And the cost is the one that section predicted in as many words: **the oldest debts sink
/// fastest.** Item 868 is `@sev: critical`, was opened 2026-09-03, and three separate rounds
/// recorded *규칙 14 로 이번 라운드에 낼 수도 없다* about it — held back by a depth it never earned.
///
/// ⚠ Surface forms rather than stems, because Korean inflects by suffix and `만든` does not
/// contain `만들`. ⛔ A reason outside BOTH sets is not caught here and it is not pretended
/// otherwise — three open items write one (`입구`, `골라 넣은 수다`, `갈랐다`) — so it is registered
/// as item 896 rather than settled by growing this array. **Widening a vocabulary until a ledger
/// passes is the one move the north star forbids.**
pub const MET_WHILE: [&str; 4] = ["마주친", "마주쳤다", "넘겨줬다", "드러났다"];

/// ⛔⛔⛔ **AND THE WORDS IT USES FOR *PAYING THAT MADE THIS*** — [`MET_WHILE`]'s other half.
///
/// A line carrying BOTH is drawing the distinction rather than falling foul of it — the ledger
/// does this twice on purpose (`갚다 «마주친» 것이 아니라 갚으면서 내가 만들었다`, and item 869's
/// `갚다가 «생긴» 것이 아니라 승격이 «만드는» 것이다`) — so a creation word settles the line.
///
/// ⚠⚠ **THE RESIDUE, STATED**: that also lets a line saying *made, not met* while MEANING the
/// reverse through. It is narrower than reading nothing, which is what this file did until now,
/// and the hole is registered rather than hidden — see register item 896.
pub const MADE_BY_PAYING: [&str; 6] = ["만들", "만든", "만드는", "생겼", "생긴", "났다"];

/// Whether `reason` states that the debt was MET rather than made — [`MET_WHILE`] with no
/// [`MADE_BY_PAYING`] word beside it.
///
/// # ⛔⛔⛔⛔⛔ THE MATCHED WORD IS CUT OUT BEFORE THE OTHER SET IS ASKED, and that is not tidiness
///
/// `드러났다` **contains** `났다`. The first build of this asked both sets over the whole sentence,
/// so every *was revealed* line looked like a *made by paying* line and the check silently passed
/// the two items it was written for — including item 868, the `@sev: critical` one three rounds
/// had recorded as un-takeable. A Korean suffix is a substring of the word it inflects, so two
/// vocabularies over one string collide by construction rather than by accident.
///
/// ⇒ Held by `a_reason_that_says_revealed_is_not_a_reason_that_says_made`, which is red for the
/// version that asks the whole sentence.
#[must_use]
pub fn met_while(reason: &str) -> Option<&'static str> {
    let word = MET_WHILE.into_iter().find(|word| reason.contains(word))?;
    let without = reason.replace(word, " ");
    if MADE_BY_PAYING.iter().any(|made| without.contains(made)) {
        return None;
    }
    Some(word)
}

/// The line the ledger declares its own count of items that state no [`PARENT`]:
/// `@from-unclassified: <n>`.
///
/// ⚠ Every item written before this mark existed is unstated, and demanding they all be annotated
/// at once is the kind of retroactive sweep that gets abandoned half-done. The ratchet holds the
/// standing count instead: a NEW item must state its parentage, because adding one without it
/// raises the count above the floor and reds.
pub const PARENT_DECLARATION: &str = "@from-unclassified:";

/// Where section A begins and ends. A number outside it is not this population's business.
const SECTION_A: &str = "## A. ";
/// Any other top-level section heading ends A.
const SECTION_ANY: &str = "## ";

/// The string whose presence used to BE the population, kept only as an alarm — see the module
/// docs.
const LOOP_WORD: &str = "ai_loop";

/// The words a round writes when it closes something. Used for one thing only: deciding whether an
/// UNMARKED item is loud enough to demand a mark today ([`Fault::UntaggedCandidate`]).
///
/// ⚠⚠⚠ **THIS LIST IS KNOWN TO BE WRONG AND THAT IS TOLERATED HERE AND NOWHERE ELSE.** It reads
/// *"WHY THIS ITEM IS NOT CLOSED"* as a closure and a half-paid item as a whole one. What that
/// costs is a demand not made — the item stays unmarked, lands in [`Reading::unclassified`] and is
/// held by the ratchet. What it must never cost is a population number, and it cannot: the
/// population is [`Tag::Open`], which this never touches.
const CLOSING_WORDS: [&str; 5] = ["PAID", "완납", "CLOSED", "답이 났다", "갚았다"];

/// An item's declared relationship to the north star.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tag {
    /// In the population and unpaid. **This, and only this, is the number the north star counts.**
    Open,
    /// Was in the population and has been paid. Kept distinct from [`Tag::Out`] because *"it was
    /// deleted"* and *"it never existed"* are two different sentences, and the ledger has paid for
    /// confusing them before.
    Paid,
    /// Not in the population. The reason belongs on the same line, after the value.
    Out,
}

impl Tag {
    /// Parse the value that follows [`TAG`]. The first word decides; anything after it is the
    /// author's reason and is not read here.
    fn parse(value: &str) -> Option<Self> {
        match value.split_whitespace().next()? {
            "open" => Some(Self::Open),
            "paid" => Some(Self::Paid),
            "out" => Some(Self::Out),
            _ => None,
        }
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Open => "open",
            Self::Paid => "paid",
            Self::Out => "out",
        })
    }
}

/// How urgently an OPEN item wants a round — [`SEVERITY`]'s value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Take this before anything else. **The set this scheme exists to keep small.**
    Critical,
    /// Everything else. Not "unimportant" — it is the north star's ordinary work, and saying so is
    /// what stops [`Reading::severity_unclassified`] counting it.
    Ordinary,
}

impl Severity {
    /// Parse the value that follows [`SEVERITY`]. The first word decides; the rest is the author's
    /// reason, which is where the argument for calling something critical belongs.
    fn parse(value: &str) -> Option<Self> {
        match value.split_whitespace().next()? {
            "critical" => Some(Self::Critical),
            "ordinary" => Some(Self::Ordinary),
            _ => None,
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Critical => "critical",
            Self::Ordinary => "ordinary",
        })
    }
}

/// What an item says about where it came from — [`PARENT`]'s value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parent {
    /// Nothing found this while paying something else: a person, CI, or a watcher reading the
    /// product. **Depth 0** — the debts the north star is actually about.
    Root,
    /// Found while paying that item. Its depth is one more than that item's.
    Item(u32),
}

impl Parent {
    /// Parse the value that follows [`PARENT`]. `none` is the root; anything else must be a number.
    fn parse(value: &str) -> Option<Self> {
        let word = value.split_whitespace().next()?;
        if word == "none" {
            return Some(Self::Root);
        }
        word.parse().ok().map(Self::Item)
    }
}

impl fmt::Display for Parent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => f.write_str("none"),
            Self::Item(number) => write!(f, "{number}"),
        }
    }
}

/// One numbered item of section A, after its blocks have been grouped.
///
/// ⚠ A number can own several blocks: this ledger closes an item by laying a new block ON TOP of
/// the original rather than editing it. So the blocks are grouped by number and the TOPMOST mark
/// wins, which is the same rule a reader uses — the newest block is the current one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The number the ledger files it under.
    pub number: u32,
    /// Its mark, if it carries one.
    pub tag: Option<Tag>,
    /// Its severity, if it states one. Read for OPEN items only — see [`SEVERITY_DECLARATION`].
    pub severity: Option<Severity>,
    /// What it says found it, if it says. See [`PARENT`].
    pub parent: Option<Parent>,
    /// Whether any block of it names the loop — the alarm's input, never the population's.
    pub names_the_loop: bool,
    /// Whether the prose vocabulary reads it as closed — likewise only the alarm's input.
    pub reads_as_closed: bool,
}

/// Something the ledger has to fix. Every variant is a RED; there is no advisory level, because a
/// finding nobody has to act on is how this file's predecessors rotted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fault {
    /// A [`TAG`] line whose value is not one of [`Tag`]'s three. A typo must not read as absent.
    UnknownTag {
        /// The item it was found in, or `None` when it sits outside any numbered block.
        number: Option<u32>,
        /// The line as written.
        line: String,
    },
    /// One block carries two different marks. The topmost-wins rule resolves blocks, never a single
    /// block arguing with itself.
    ConflictingTags {
        /// The item.
        number: u32,
        /// The marks found, in the order written.
        found: Vec<Tag>,
    },
    /// An unmarked item that names the loop and does not read as closed — the prose alarm firing.
    UntaggedCandidate {
        /// The item that must state its membership.
        number: u32,
    },
    /// A [`SEVERITY`] line whose value is neither of [`Severity`]'s two. A typo must not read as
    /// absent — the same rule [`Fault::UnknownTag`] holds, for the same reason.
    UnknownSeverity {
        /// The item it was found in, or `None` when it sits outside any numbered block.
        number: Option<u32>,
        /// The line as written.
        line: String,
    },
    /// One block states two different severities.
    ConflictingSeverities {
        /// The item.
        number: u32,
        /// The severities found, in the order written.
        found: Vec<Severity>,
    },
    /// More OPEN items with no severity than the ledger declares. **This backlog may shrink, never
    /// grow** — the same ratchet [`Fault::RatchetGrew`] holds over membership.
    SeverityRatchetGrew {
        /// What this reading counted.
        counted: usize,
        /// What [`SEVERITY_DECLARATION`] claims.
        declared: usize,
    },
    /// No [`SEVERITY_DECLARATION`] line, or more than one.
    SeverityDeclaration {
        /// How many were found.
        found: usize,
    },
    /// The one [`SEVERITY_DECLARATION`] line carries no number.
    UnreadableSeverityDeclaration {
        /// The line as written.
        line: String,
    },
    /// A [`PARENT`] line whose value is neither `none` nor a number.
    UnknownParent {
        /// The item it was found in, or `None` when it sits outside any numbered block.
        number: Option<u32>,
        /// The line as written.
        line: String,
    },
    /// ⛔⛔⛔⛔⛔ **A [`PARENT`] LINE THAT NAMES A NUMBER AND SAYS IT WAS MET WHILE PAYING IT** —
    /// register item 896, and the inversion [`PARENT`]'s own doc calls *the whole scheme*.
    ///
    /// The reason states [`MET_WHILE`] with no [`MADE_BY_PAYING`] word beside it, so the item is a
    /// ROOT wearing a child's number. Every debt below it inherits a depth nobody earned, and
    /// [`Reading::deferred`] then holds it back — *the oldest debts sink fastest*, measured on the
    /// four lines that were doing exactly this.
    ///
    /// ⚠ The repair is `{PARENT} none`, keeping the sentence: it already says why.
    MetWhileNotMade {
        /// The item that said it.
        number: u32,
        /// The parent it named.
        named: u32,
        /// The word in its reason that says it was met rather than made.
        word: &'static str,
    },
    /// An item names a parent section A does not have. **A chain that leaves the ledger cannot be
    /// walked**, so the depth of everything below it is unknown rather than zero.
    DanglingParent {
        /// The item that said it.
        number: u32,
        /// What it named.
        named: u32,
    },
    /// The parent chain comes back to where it started. Depth would not terminate.
    ParentCycle {
        /// The item the walk started from.
        number: u32,
    },
    /// More items with no [`PARENT`] than the ledger declares.
    ParentRatchetGrew {
        /// What this reading counted.
        counted: usize,
        /// What [`PARENT_DECLARATION`] claims.
        declared: usize,
    },
    /// No [`PARENT_DECLARATION`] line, or more than one.
    ParentDeclaration {
        /// How many were found.
        found: usize,
    },
    /// The one [`PARENT_DECLARATION`] line carries no number.
    UnreadableParentDeclaration {
        /// The line as written.
        line: String,
    },
    /// More unmarked items than the ledger declares. **The backlog may shrink, never grow.**
    RatchetGrew {
        /// What this reading counted.
        counted: usize,
        /// What [`DECLARATION`] claims.
        declared: usize,
    },
    /// No [`DECLARATION`] line, or more than one. A ratchet with no floor is not a ratchet.
    Declaration {
        /// How many were found.
        found: usize,
    },
    /// The one [`DECLARATION`] line carries no number. **A floor nobody can read is not a floor**,
    /// and dropping it silently is what let a prose quotation stand in for the real one.
    UnreadableDeclaration {
        /// The line as written.
        line: String,
    },
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownTag { number, line } => {
                let where_ =
                    number.map_or_else(|| "outside any item".to_string(), |n| format!("item {n}"));
                write!(
                    f,
                    "{where_}: `{}` is not one of open/paid/out — a mistyped mark must not read as \
                     an absent one",
                    line.trim()
                )
            }
            Self::ConflictingTags { number, found } => {
                let spelled: Vec<String> = found.iter().map(ToString::to_string).collect();
                write!(
                    f,
                    "item {number}: one block carries {} marks ({}) — the topmost-wins rule settles \
                     blocks, not a block arguing with itself",
                    found.len(),
                    spelled.join(", "),
                )
            }
            Self::UntaggedCandidate { number } => write!(
                f,
                "item {number}: names the loop, reads as open, and states no `{TAG}` — say whether \
                 it is in the population",
            ),
            Self::RatchetGrew { counted, declared } => write!(
                f,
                "{counted} unmarked items, but the ledger declares {declared}: the backlog may \
                 shrink, never grow. Mark the new item, or lower `{DECLARATION}` if you paid some \
                 down",
            ),
            Self::Declaration { found } => write!(
                f,
                "found {found} `{DECLARATION}` lines, need exactly 1 — a ratchet with no floor \
                 holds nothing",
            ),
            Self::UnreadableDeclaration { line } => write!(
                f,
                "`{}` states no number — a floor nobody can read is not a floor, and skipping it \
                 quietly is how a sentence about the scheme comes to stand in for it",
                line.trim(),
            ),
            Self::UnknownSeverity { number, line } => {
                let where_ =
                    number.map_or_else(|| "outside any item".to_string(), |n| format!("item {n}"));
                write!(
                    f,
                    "{where_}: `{}` is not one of critical/ordinary — a mistyped severity must not \
                     read as an absent one",
                    line.trim()
                )
            }
            Self::ConflictingSeverities { number, found } => {
                let spelled: Vec<String> = found.iter().map(ToString::to_string).collect();
                write!(
                    f,
                    "item {number}: one block states {} severities ({}) — the topmost-wins rule \
                     settles blocks, not a block arguing with itself",
                    found.len(),
                    spelled.join(", "),
                )
            }
            Self::SeverityRatchetGrew { counted, declared } => write!(
                f,
                "{counted} open items state no `{SEVERITY}`, but the ledger declares {declared}: \
                 this backlog may shrink, never grow. Say whether the new item is critical, or \
                 lower `{SEVERITY_DECLARATION}` if you classified some",
            ),
            Self::SeverityDeclaration { found } => write!(
                f,
                "found {found} `{SEVERITY_DECLARATION}` lines, need exactly 1 — a ratchet with no \
                 floor holds nothing",
            ),
            Self::UnreadableSeverityDeclaration { line } => write!(
                f,
                "`{}` states no number — a floor nobody can read is not a floor",
                line.trim(),
            ),
            Self::UnknownParent { number, line } => {
                let where_ =
                    number.map_or_else(|| "outside any item".to_string(), |n| format!("item {n}"));
                write!(
                    f,
                    "{where_}: `{}` is neither `none` nor an item number — say what found this, or \
                     say nothing found it",
                    line.trim()
                )
            }
            Self::MetWhileNotMade {
                number,
                named,
                word,
            } => write!(
                f,
                "item {number} names {named} as its parent and its reason says `{word}` — met \
                 while paying, not made by paying. That is a ROOT wearing a child's number, and \
                 every debt under it inherits a depth nobody earned: write `{PARENT} none` and \
                 keep the sentence, which already says why",
            ),
            Self::DanglingParent { number, named } => write!(
                f,
                "item {number} says it was found while paying {named}, which section A does not \
                 have — a chain that leaves the ledger cannot be walked, so nothing below it has a \
                 depth",
            ),
            Self::ParentCycle { number } => write!(
                f,
                "item {number}: its parent chain returns to it — depth would not terminate",
            ),
            Self::ParentRatchetGrew { counted, declared } => write!(
                f,
                "{counted} items state no `{PARENT}`, but the ledger declares {declared}: this \
                 backlog may shrink, never grow. A new item says what found it (`{PARENT} <n>`) or \
                 that nothing did (`{PARENT} none`)",
            ),
            Self::ParentDeclaration { found } => write!(
                f,
                "found {found} `{PARENT_DECLARATION}` lines, need exactly 1 — a ratchet with no \
                 floor holds nothing",
            ),
            Self::UnreadableParentDeclaration { line } => write!(
                f,
                "`{}` states no number — a floor nobody can read is not a floor",
                line.trim(),
            ),
        }
    }
}

/// What one pass over the ledger saw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    /// Every numbered item of section A, in numeric order.
    pub items: Vec<Item>,
    /// What [`DECLARATION`] said, when exactly one line said it.
    pub declared: Option<usize>,
    /// What [`SEVERITY_DECLARATION`] said, when exactly one line said it.
    pub severity_declared: Option<usize>,
    /// What [`PARENT_DECLARATION`] said, when exactly one line said it.
    pub parent_declared: Option<usize>,
    /// Everything that has to be fixed.
    pub faults: Vec<Fault>,
}

impl Reading {
    /// **THE NORTH STAR'S POPULATION** — the items marked [`Tag::Open`], in numeric order.
    ///
    /// One predicate, one place. The index does not keep its own copy; it prints this.
    #[must_use]
    pub fn population(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|item| item.tag == Some(Tag::Open))
            .map(|item| item.number)
            .collect()
    }

    /// The items that state nothing — the backlog the ratchet holds.
    #[must_use]
    pub fn unclassified(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|item| item.tag.is_none())
            .map(|item| item.number)
            .collect()
    }

    /// **WHAT A ROUND TAKES FIRST** — the open items marked [`Severity::Critical`], in numeric
    /// order. Register item 833(1).
    ///
    /// ⚠ Open only. A paid item's severity is history, and an item outside the population was
    /// never this loop's to rank.
    #[must_use]
    pub fn critical(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|item| item.tag == Some(Tag::Open) && item.severity == Some(Severity::Critical))
            .map(|item| item.number)
            .collect()
    }

    /// **HOW FAR DOWN THE DEBT CHAIN AN ITEM SITS** — register item 833(2).
    ///
    /// 0 is a debt nothing found while paying something else. 1 is a debt found while paying one of
    /// those. `None` is *nobody wrote it down* — an item with no [`PARENT`], or one whose chain
    /// runs into a dangling parent or a cycle. **Unknown is never reported as 0**: that would make
    /// the whole standing backlog look like roots and hand the cap nothing to hold.
    #[must_use]
    pub fn depth(&self, number: u32) -> Option<u32> {
        let mut seen = std::collections::BTreeSet::new();
        let mut at = number;
        let mut depth = 0;
        loop {
            if !seen.insert(at) {
                return None;
            }
            let item = self.items.iter().find(|item| item.number == at)?;
            match item.parent? {
                Parent::Root => return Some(depth),
                Parent::Item(up) => {
                    depth += 1;
                    at = up;
                }
            }
        }
    }

    /// 🎯🎯🎯🎯🎯 **DOES `to` COME OUT OF `from`?** — `Some(true)` where `to`'s chain of
    /// [`PARENT`] marks reaches `from`, `Some(false)` where it reaches a declared root without
    /// meeting it, and [`None`] where the chain cannot be walked to either answer. Register item
    /// 840.
    ///
    /// # ⛔⛔⛔⛔⛔ The two things a run's budget was counting as one
    ///
    /// Register item 833(2) gave a run a bound on how far it may re-aim, and it counted CHANGES:
    /// any milestone other than the one it holds spends a step. Two opposite movements were
    /// therefore priced the same:
    ///
    /// * **going deeper** — taking a debt that the work in hand CREATED. The chain lengthens, and
    ///   this is the thing the budget exists to stop.
    /// * **going sideways** — taking an unrelated ROOT. The chain is length zero, and this is
    ///   progress, not waste.
    ///
    /// So a capped run could not move to the next debt at all: moving spent the budget it had
    /// already spent, and the run ended. This predicate is the difference, and it is the register's
    /// to answer because only the register knows what created what.
    ///
    /// # ⚠⚠⚠ [`None`] IS NOT `Some(false)`, and the difference is working rule 6
    ///
    /// *Nobody wrote down where this came from* must not read as *nothing created it*. An unstated
    /// parentage would otherwise make every item in the standing backlog free of the budget — the
    /// escape hatch that disables its own gate. So the caller of this must treat [`None`] the way
    /// it treats a step: **unclassified is not a pass**, and what unlocks the cheaper answer is the
    /// same annotation [`Reading::unrooted`]'s ratchet already asks for.
    ///
    /// ⚠ `from == to` is `Some(true)`: a proposal naming the item the run is already on has not
    /// gone anywhere, and the guard that reads this is reached only where the milestone MOVED.
    #[must_use]
    pub fn descends(&self, from: u32, to: u32) -> Option<bool> {
        let mut seen = std::collections::BTreeSet::new();
        let mut at = to;
        loop {
            if at == from {
                return Some(true);
            }
            if !seen.insert(at) {
                return None;
            }
            let item = self.items.iter().find(|item| item.number == at)?;
            match item.parent? {
                Parent::Root => return Some(false),
                Parent::Item(up) => at = up,
            }
        }
    }

    /// 🎯🎯🎯🎯🎯 **AND THE TWO ANSWERS AS ONE PREDICATE** — register item 840, and the arrangement
    /// the loop's document reads through a classifier.
    ///
    /// A proposal is *sideways* only when the register can WALK the chain to a declared root
    /// without meeting `from`. Everything else — a chain that reaches `from`, one that runs into an
    /// item stating no parentage, one that cannot be placed at all — is charged as a step.
    ///
    /// ⚠⚠ **THIS IS WHERE WORKING RULE 6 LIVES FOR THIS FEATURE**, spelled once so no caller has to
    /// remember it: *unclassified is not the cheap answer*. A build that folded [`None`] into
    /// *sideways* would let every unannotated item escape the budget, and nothing about that would
    /// look wrong from the outside.
    #[must_use]
    pub fn sideways(&self, from: Option<u32>, to: u32) -> bool {
        from.and_then(|held| self.descends(held, to))
            .is_some_and(|derived| !derived)
    }

    /// **WHAT A ROUND MAY TAKE** — the open items whose [`Reading::depth`] is known and within
    /// `cap`, in numeric order. Register item 833(2), and the owner's default of 1.
    ///
    /// ⚠⚠ An item of UNKNOWN depth is takeable, and that is deliberate rather than an oversight:
    /// the standing backlog states no parentage and refusing all of it would stop the loop dead on
    /// the day this shipped. What the cap bites on is the chain this scheme can actually see — a
    /// debt that SAYS it came from another. As the backlog is annotated the cap reaches further,
    /// which is the same direction [`Reading::unclassified`] pays down in.
    #[must_use]
    pub fn takeable(&self, cap: u32) -> Vec<u32> {
        self.population()
            .into_iter()
            .filter(|number| self.depth(*number).is_none_or(|depth| depth <= cap))
            .collect()
    }

    /// The open items the cap holds back — registered, and not to be worked until the budget
    /// allows. **This is the number that says the scheme is doing anything at all.**
    #[must_use]
    pub fn deferred(&self, cap: u32) -> Vec<u32> {
        self.population()
            .into_iter()
            .filter(|number| self.depth(*number).is_some_and(|depth| depth > cap))
            .collect()
    }

    /// **WHAT A ROUND MAY TAKE NEXT, AS ONE SET** — working rules 11 and 14 made into a predicate
    /// instead of a sentence somebody reads. Register item 839.
    ///
    /// # ⛔⛔⛔⛔⛔ Why this exists when [`Reading::critical`] and [`Reading::takeable`] both did
    ///
    /// The two lists were already printed and **nothing read them**. The rule that says what to do
    /// with them — *while anything is critical, take from those; the cap holds the rest back* —
    /// lived in prose the loop was greeted with, and a run that ignored it was refused by nobody.
    /// Measured 2026-09-02: the loop's own supervisor wrote those rules into a prompt fragment
    /// **on the line under the rule that says prose is measured by nobody**.
    ///
    /// So the two lists are composed HERE, once, and a proposal is admissible exactly when it names
    /// a member of this set.
    ///
    /// # ⚠⚠ The composition, and why the fall-through is not an escape hatch
    ///
    /// * While any OPEN item is [`Severity::Critical`] **and within `cap`**, the set is those.
    /// * When none is, it is [`Reading::takeable`] — the population minus what the cap holds back.
    ///
    /// The fall-through cannot admit something the cap refuses, because both arms are drawn from
    /// [`Reading::takeable`]. And it is not an ordering: register item 659's counter-argument is
    /// kept exactly as [`SEVERITY`]'s own docs state it — **this is a gate, not a sort**, so items
    /// that share a seam may still be worked in whatever order coheres, as long as they are in the
    /// set.
    #[must_use]
    pub fn admits(&self, cap: u32) -> Vec<u32> {
        let takeable = self.takeable(cap);
        let critical: Vec<u32> = self
            .critical()
            .into_iter()
            .filter(|number| takeable.contains(number))
            .collect();
        if critical.is_empty() {
            takeable
        } else {
            critical
        }
    }

    /// **WHICH REGISTER ITEM A PROPOSAL NAMES** — the FIRST number in `text` that this ledger files
    /// an item under, or [`None`] where it names none. Register item 839.
    ///
    /// # ⚠⚠⚠ Why the first, and why membership rather than a shape
    ///
    /// A milestone is prose a person or an agent wrote, and it cites other numbers freely — a date,
    /// a byte count, the item that produced the one it is about. What it is ABOUT is the first
    /// register item it names, which is this ledger's own convention (*"항목 839 를 갚아라 — …"*)
    /// and the only rule here that does not need a parser for prose.
    ///
    /// Numbers are filtered through [`Reading::items`] rather than through a shape, so a year and a
    /// byte count are skipped for the reason they should be: **nothing is filed under them.**
    ///
    /// ⚠ The residue, stated rather than hidden: a proposal that cites another item before naming
    /// its own is read as being about the citation. The remedy is the ledger's convention, not a
    /// longer rule — and where the citation is outside the admissible set the answer is a REFUSAL,
    /// which is the safe direction for a check whose whole job is to hold a run to its brief.
    #[must_use]
    pub fn names(&self, text: &str) -> Option<u32> {
        let mut digits = String::new();
        for character in text.chars().chain(std::iter::once(' ')) {
            if character.is_ascii_digit() {
                digits.push(character);
                continue;
            }
            let read = digits.parse::<u32>().ok();
            digits.clear();
            // ⚠ A number the ledger files nothing under is not a citation of anything, so the scan
            // goes on rather than stopping at the first integer it meets.
            if read.is_some_and(|number| self.items.iter().any(|item| item.number == number)) {
                return read;
            }
        }
        None
    }

    /// The items that state no [`PARENT`] — the backlog [`Fault::ParentRatchetGrew`] holds.
    #[must_use]
    pub fn unrooted(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|item| item.parent.is_none())
            .map(|item| item.number)
            .collect()
    }

    /// The OPEN items that state no severity — the backlog [`Fault::SeverityRatchetGrew`] holds.
    ///
    /// ⚠⚠ These are not "ordinary". **Unclassified is not a pass** — the same rule that makes an
    /// unmarked item a debt rather than a "no" (working rule 6). A round that wants one of these
    /// worked says so by classifying it.
    #[must_use]
    pub fn severity_unclassified(&self) -> Vec<u32> {
        self.items
            .iter()
            .filter(|item| item.tag == Some(Tag::Open) && item.severity.is_none())
            .map(|item| item.number)
            .collect()
    }

    /// Whether this reading is clean.
    #[must_use]
    pub fn is_green(&self) -> bool {
        self.faults.is_empty()
    }
}

/// The `<data>` id a loop document declares its re-aim cap under.
///
/// ⚠ `reaim_max` and not `debt_depth_max`: register item 833 renamed it when a gate caught the word
/// `debt` in `ai_loop.scxml`, which other repositories copy. The ledger's own prose still calls it
/// the depth cap, and it is the same number.
pub const REAIM_MAX: &str = "reaim_max";

/// ⛔⛔⛔⛔⛔ **HOW DEEP A RUN MAY RE-AIM, AS THE DOCUMENT DECLARES IT** — register item 833(1),
/// and the number this crate must hold no opinion about.
///
/// # ⛔⛔⛔⛔⛔ The binary held a literal `1` under a comment saying it did not
///
/// Measured 2026-09-04. `north-star.rs` carried, in as many words, *"THE CAP IS THE DOCUMENT'S,
/// NOT THIS BINARY'S"* — and two lines below it, twice, `.unwrap_or(1)`. **The document was never
/// read.** Driven: `debt_loop.scxml`'s `reaim_max` was set to `2` and the binary rebuilt, and it
/// went on printing `deferred 10 at depth > 1` and refusing item 843 with *"sits deeper than 1"*.
/// Five critical items stayed held back by a number the document no longer declared.
///
/// ⇒ That is register item 445's shape — **two authors for one policy** — sitting inside the
/// instrument item 833 exists to build, and it is the failure mode 773's axis names: *the subject
/// is the launcher's, the policy is the DOCUMENT's.*
///
/// # ⚠⚠⚠ Rule 6: a document that declares none is a FAULT and never a `1`
///
/// A default here is the escape hatch that retires the gate. `sprag_plugin` already refuses at the
/// door a document that declares no cap (`Briefed::NotHeld`), and this is that refusal on the
/// reading side: the answer to *what cap is this ledger being judged under* has to be a document's,
/// or nobody's.
///
/// # ⚠⚠ `never` is a value and not an absence
///
/// The document's own guard reads `reaim_max != 'never'`, so declining the cap is a thing a
/// document may say. It is [`Reaim::Never`] here rather than a very large number, because a reader
/// told `at depth > 4294967295` learns nothing and a reader told `never` learns the whole of it.
///
/// # Errors
///
/// A sentence naming what is wrong, for a document that declares the cap zero times, more than
/// once, or with a value this reader cannot make a number or `never` of.
pub fn declared_reaim(document: &str) -> Result<Reaim, String> {
    let needle = format!("<data id=\"{REAIM_MAX}\"");
    let stated: Vec<&str> = document
        .lines()
        .filter(|line| line.contains(&needle))
        .collect();
    let [only] = stated.as_slice() else {
        return Err(format!(
            "this document declares `{REAIM_MAX}` {} times, and the cap a run obeys must be one \
             number one document states once",
            stated.len(),
        ));
    };
    let said = only
        .split_once("expr=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(value, _)| value.trim())
        .ok_or_else(|| {
            format!(
                "`{REAIM_MAX}` is declared without an `expr`: {}",
                only.trim()
            )
        })?;
    // ⚠ The quotes are the DOCUMENT's: `expr` is an expression in its datamodel, so a word arrives
    // quoted and a number does not. Stripping them here is reading the document's own spelling,
    // not guessing at it.
    let bare = said.trim_matches('\'');
    if bare == "never" {
        return Ok(Reaim::Never);
    }
    bare.parse().map(Reaim::Of).map_err(|_| {
        format!(
            "`{REAIM_MAX}` is declared as {said:?}, which is neither a number nor `never` — and an \
             unreadable policy must not be read as the default this reader would otherwise have \
             invented"
        )
    })
}

/// ⛔⛔⛔ **WHAT A DOCUMENT SAYS ABOUT RE-AIMING** — [`declared_reaim`]'s answer.
///
/// ⚠ A word and not an `Option<u32>`, for the reason `sprag_plugin::Counted` gives one crate over:
/// [`None`] already means *nothing readable is there*, and *the author declined the cap* is the
/// opposite of that — one is a document to refuse, the other is a document to obey.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reaim {
    /// A run may re-aim this many times before a further checkpoint is registered and not taken.
    Of(u32),
    /// The document declines the cap: nothing is ever held back for being too deep.
    Never,
}

impl Reaim {
    /// The depth [`Reading::deferred`] and [`Reading::admits`] are asked with.
    ///
    /// ⚠ [`Never`](Self::Never) is the largest depth there is, which is the honest translation: a
    /// chain cannot be deeper than the register has items. It is spelled here once so no caller
    /// invents its own translation — the defect this whole function exists to remove, one layer in.
    #[must_use]
    pub const fn depth(self) -> u32 {
        match self {
            Self::Of(cap) => cap,
            Self::Never => u32::MAX,
        }
    }

    /// The word a reader is shown beside a count.
    #[must_use]
    pub fn spelled(self) -> String {
        match self {
            Self::Of(cap) => cap.to_string(),
            Self::Never => "never".to_owned(),
        }
    }
}

/// Split the ledger at section A's boundaries. Returns the lines of A alone.
///
/// ⚠ A missing section A yields nothing, and the caller must treat an EMPTY reading as a fault
/// rather than as a clean one — a probe pointed at nothing must never read as clean, which is the
/// defect [`crate`]'s first gate shipped with.
fn section_a(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with(SECTION_A) {
            inside = true;
            continue;
        }
        if inside && line.starts_with(SECTION_ANY) && !line.starts_with(SECTION_A) {
            break;
        }
        if inside {
            out.push(line);
        }
    }
    out
}

/// The number a block header opens with, as in `823. ⛔ …` at column zero.
fn block_number(line: &str) -> Option<u32> {
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !line[digits.len()..].starts_with(". ") {
        return None;
    }
    digits.parse().ok()
}

/// Whether a line is a section heading INSIDE a block — `### ✅ …`, which this ledger indents.
///
/// ⚠ The indentation is why: a `^#{2,4}` anchor at column zero read eleven closed items as open on
/// 2026-09-02, because block bodies are indented five spaces and their headings with them.
fn is_heading(line: &str) -> bool {
    let bare = line.trim_start();
    bare.starts_with("## ") || bare.starts_with("### ") || bare.starts_with("#### ")
}

/// Whether a line carries a closing word, for the alarm only.
fn closes(line: &str) -> bool {
    CLOSING_WORDS.iter().any(|word| line.contains(word))
}

/// The value of a mark, when this line IS one — see [`TAG`] for why a sentence that merely quotes
/// the token is not.
fn mark_value(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(TAG)
}

/// The value of a [`SEVERITY`] line, by the same whole-line rule [`mark_value`] holds.
fn severity_value(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(SEVERITY)
}

/// The value of a [`PARENT`] line, by the same whole-line rule [`mark_value`] holds.
fn parent_value(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(PARENT)
}

/// Read the one line that declares a ratchet's floor, faulting when there is not exactly one or
/// when its number cannot be read.
///
/// ⚠ Shared by both ratchets deliberately: two hand-written copies of this drifted apart in every
/// register item that ever wrote the same rule twice.
fn declared_floor(
    text: &str,
    token: &str,
    faults: &mut Vec<Fault>,
    many: impl Fn(usize) -> Fault,
    unreadable: impl Fn(String) -> Fault,
) -> Option<usize> {
    let stated: Vec<&str> = text
        .lines()
        .filter(|line| line.trim_start().starts_with(token))
        .collect();
    match stated.as_slice() {
        [only] => {
            let value = only
                .trim_start()
                .strip_prefix(token)
                .and_then(|rest| rest.split_whitespace().next())
                .and_then(|word| word.parse().ok());
            if value.is_none() {
                faults.push(unreadable((*only).to_string()));
            }
            value
        }
        found => {
            faults.push(many(found.len()));
            None
        }
    }
}

/// Read section A of a ledger and judge it.
///
/// The `declared` floor is read from the same text: see [`DECLARATION`].
#[must_use]
pub fn read(text: &str) -> Reading {
    let lines = section_a(text);

    // Group blocks by number, keeping document order so the topmost mark can win.
    let mut order: Vec<u32> = Vec::new();
    let mut blocks: BTreeMap<u32, Vec<Vec<&str>>> = BTreeMap::new();
    let mut current: Option<(u32, Vec<&str>)> = None;
    let mut faults: Vec<Fault> = Vec::new();

    for line in &lines {
        if let Some(number) = block_number(line) {
            if let Some((n, body)) = current.take() {
                blocks.entry(n).or_default().push(body);
            }
            if !order.contains(&number) {
                order.push(number);
            }
            current = Some((number, vec![line]));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        } else if mark_value(line).is_some() {
            faults.push(Fault::UnknownTag {
                number: None,
                line: (*line).to_string(),
            });
        }
    }
    if let Some((n, body)) = current.take() {
        blocks.entry(n).or_default().push(body);
    }

    // The declaration is read from the WHOLE document: it is the ledger's statement about itself
    // and need not sit inside section A.
    //
    // ⚠⚠⚠⚠⚠ A DECLARATION IS A LINE THAT STARTS WITH THE TOKEN, and its value being unreadable is
    // a FAULT rather than a line that quietly does not count. Both halves were measured wrong here
    // on 2026-09-02: register item 823 explains the scheme and therefore QUOTES this token in
    // prose, so two lines carried it — and the gate stayed green only because the prose one failed
    // to parse and was silently dropped from the count. That is luck, and it cuts both ways: a
    // sentence that happened to quote a number would have gone red, and a typo in the REAL
    // declaration would have handed its job to the sentence.
    let declared = declared_floor(
        text,
        DECLARATION,
        &mut faults,
        |found| Fault::Declaration { found },
        |line| Fault::UnreadableDeclaration { line },
    );
    let severity_declared = declared_floor(
        text,
        SEVERITY_DECLARATION,
        &mut faults,
        |found| Fault::SeverityDeclaration { found },
        |line| Fault::UnreadableSeverityDeclaration { line },
    );
    let parent_declared = declared_floor(
        text,
        PARENT_DECLARATION,
        &mut faults,
        |found| Fault::ParentDeclaration { found },
        |line| Fault::UnreadableParentDeclaration { line },
    );

    let mut items: Vec<Item> = Vec::new();
    for (number, bodies) in &blocks {
        let mut tag = None;
        let mut severity = None;
        let mut parent = None;
        for body in bodies {
            let mut in_block: Vec<Tag> = Vec::new();
            let mut severities: Vec<Severity> = Vec::new();
            let mut parents: Vec<Parent> = Vec::new();
            for line in body {
                if let Some(value) = parent_value(line) {
                    match Parent::parse(value) {
                        Some(found) => {
                            // ⛔⛔⛔⛔⛔ AND THE REASON IS READ, NOT DISCARDED — register item 896.
                            // `Parent::parse` takes the first word and drops the sentence, so the
                            // rule this file's own `PARENT` doc calls *the whole scheme* was
                            // enforced by whoever typed the line. Four lines were on the wrong
                            // side of it, one of them a `@sev: critical` item three rounds had
                            // recorded as un-takeable.
                            if let Parent::Item(named) = found
                                && let Some(word) = met_while(value)
                            {
                                faults.push(Fault::MetWhileNotMade {
                                    number: *number,
                                    named,
                                    word,
                                });
                            }
                            parents.push(found);
                        }
                        None => faults.push(Fault::UnknownParent {
                            number: Some(*number),
                            line: (*line).to_string(),
                        }),
                    }
                }
                if let Some(value) = severity_value(line) {
                    match Severity::parse(value) {
                        Some(found) => severities.push(found),
                        None => faults.push(Fault::UnknownSeverity {
                            number: Some(*number),
                            line: (*line).to_string(),
                        }),
                    }
                }
                let Some(value) = mark_value(line) else {
                    continue;
                };
                match Tag::parse(value) {
                    Some(found) => in_block.push(found),
                    None => faults.push(Fault::UnknownTag {
                        number: Some(*number),
                        line: (*line).to_string(),
                    }),
                }
            }
            if severities.len() > 1 && severities.iter().any(|found| *found != severities[0]) {
                faults.push(Fault::ConflictingSeverities {
                    number: *number,
                    found: severities.clone(),
                });
            }
            // Topmost block wins, exactly as the membership mark does.
            if severity.is_none() {
                severity = severities.first().copied();
            }
            if parent.is_none() {
                parent = parents.first().copied();
            }
            if in_block.len() > 1 && in_block.iter().any(|found| *found != in_block[0]) {
                faults.push(Fault::ConflictingTags {
                    number: *number,
                    found: in_block.clone(),
                });
            }
            // Topmost block wins: the first body that states anything settles the item.
            if tag.is_none() {
                tag = in_block.first().copied();
            }
        }

        let names_the_loop = bodies
            .iter()
            .any(|body| body.iter().any(|line| line.contains(LOOP_WORD)));
        let reads_as_closed = bodies.iter().any(|body| {
            body.first().is_some_and(|head| closes(head))
                || body
                    .iter()
                    .skip(1)
                    .any(|line| is_heading(line) && closes(line))
        });

        if tag.is_none() && names_the_loop && !reads_as_closed {
            faults.push(Fault::UntaggedCandidate { number: *number });
        }
        items.push(Item {
            number: *number,
            tag,
            severity,
            parent,
            names_the_loop,
            reads_as_closed,
        });
    }

    let unmarked = items.iter().filter(|item| item.tag.is_none()).count();
    if let Some(floor) = declared
        && unmarked > floor
    {
        faults.push(Fault::RatchetGrew {
            counted: unmarked,
            declared: floor,
        });
    }

    let unranked = items
        .iter()
        .filter(|item| item.tag == Some(Tag::Open) && item.severity.is_none())
        .count();
    if let Some(floor) = severity_declared
        && unranked > floor
    {
        faults.push(Fault::SeverityRatchetGrew {
            counted: unranked,
            declared: floor,
        });
    }

    let unrooted = items.iter().filter(|item| item.parent.is_none()).count();
    if let Some(floor) = parent_declared
        && unrooted > floor
    {
        faults.push(Fault::ParentRatchetGrew {
            counted: unrooted,
            declared: floor,
        });
    }

    // ⚠ The chain is judged AFTER every item is known: a parent may be filed below its child, and
    // reading forward-only would call a legal chain dangling.
    let numbers: std::collections::BTreeSet<u32> = items.iter().map(|item| item.number).collect();
    for item in &items {
        if let Some(Parent::Item(named)) = item.parent
            && !numbers.contains(&named)
        {
            faults.push(Fault::DanglingParent {
                number: item.number,
                named,
            });
        }
    }

    let mut reading = Reading {
        items,
        declared,
        severity_declared,
        parent_declared,
        faults,
    };
    // A cycle is only visible once the walk exists, and it must be a fault rather than a silent
    // `None` — an item whose chain eats itself would otherwise read as merely unstated.
    let cycles: Vec<u32> = reading
        .items
        .iter()
        .filter(|item| item.parent.is_some())
        .map(|item| item.number)
        .filter(|number| {
            let mut seen = std::collections::BTreeSet::new();
            let mut at = *number;
            loop {
                if !seen.insert(at) {
                    return true;
                }
                let Some(found) = reading.items.iter().find(|item| item.number == at) else {
                    return false;
                };
                match found.parent {
                    Some(Parent::Item(up)) => at = up,
                    _ => return false,
                }
            }
        })
        .collect();
    for number in cycles {
        reading.faults.push(Fault::ParentCycle { number });
    }
    reading
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ledger shaped like the real one: section A, numbered blocks at column zero, bodies
    /// indented five spaces, headings indented with them.
    const LEDGER: &str = "\
# Ledger
## A. THE SHARPEST THINGS OPEN
@ns-unclassified: 1
@sev-unclassified: 0
@from-unclassified: 3

900. ⛔ **An open loop item**
     @ns: open — the loop's own driver
     @sev: critical — it stops the loop dead
     @from: none
     body mentioning ai_loop here

899. ✅✅ **PAID 2026-09-02**
     @ns: paid
     ### ✅ 완납 — closed properly

898. ⛔ **Something else entirely**
     @ns: out — a rendering defect, nothing to do with the loop

897. ⛔ **Unmarked and quiet**
     no mark, no mention

## B. Live product residues
896. ⛔ **Outside section A**
     @ns: open — must not be counted
";

    #[test]
    fn the_population_is_the_marks_and_nothing_else() {
        let reading = read(LEDGER);
        assert_eq!(
            reading.population(),
            vec![900],
            "only `{TAG} open` counts — a paid item, an out item and an unmarked one are all not \
             the population, for three different reasons",
        );
        assert_eq!(
            reading.unclassified(),
            vec![897],
            "an item that states nothing is carried as its own number rather than silently dropped",
        );
        assert!(reading.is_green(), "faults: {:?}", reading.faults);
    }

    /// ⚠⚠⚠ **SECTION B IS NOT THE POPULATION**, and a mark there must not leak in — the north star
    /// names section A and only section A.
    #[test]
    fn a_mark_outside_section_a_is_not_read() {
        let reading = read(LEDGER);
        assert!(
            !reading.items.iter().any(|item| item.number == 896),
            "896 lives under `## B.` and this reading stopped there: {:?}",
            reading.items,
        );
    }

    /// **THE 470 SHAPE**: an item whose own words say it is NOT closed used to be read as closed,
    /// because the sentence contains the closing word. The mark settles it and the prose does not
    /// get a vote.
    #[test]
    fn an_item_that_says_it_is_not_closed_is_still_its_mark() {
        let ledger = LEDGER.replace(
            "900. ⛔ **An open loop item**",
            "900. ⛔ **An open loop item**\n     ### ⛔ WHY THIS ITEM IS NOT CLOSED",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.population(),
            vec![900],
            "the word CLOSED inside a sentence denying closure changed nothing, because the \
             population is the mark",
        );
    }

    /// **THE 738 SHAPE**: four limbs paid, one owed, and a `완납` heading for each paid limb. The
    /// prose reads the whole item closed; the mark says otherwise and wins.
    #[test]
    fn a_partly_paid_item_is_still_open_when_its_mark_says_so() {
        let ledger = LEDGER.replace(
            "     body mentioning ai_loop here",
            "     ### ✅ 완납 2026-08-28 — limbs 1-3\n     body mentioning ai_loop here",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.population(),
            vec![900],
            "the limb still owed is the item's state"
        );
    }

    /// **THE ESCAPE HATCH THAT IS NOT ONE**: an item that names the loop and reads as open must say
    /// which side it is on. Silence is a fault, not a "no".
    #[test]
    fn an_unmarked_item_that_names_the_loop_is_red() {
        let ledger = LEDGER.replace("     @ns: open — the loop's own driver\n", "");
        let reading = read(&ledger);
        assert!(
            reading
                .faults
                .contains(&Fault::UntaggedCandidate { number: 900 }),
            "the alarm must fire on an unmarked candidate: {:?}",
            reading.faults,
        );
    }

    /// ⛔⛔⛔⛔⛔ **A DOCUMENT THAT EXPLAINS THE MARK MUST NOT BE READ AS MARKED** — measured on the
    /// real ledger the hour this shipped.
    ///
    /// Item 823's entry had to quote the token to say what the scheme is, and a reading that took
    /// [`TAG`] from anywhere in a line turned three sentences of that explanation into three
    /// malformed marks. The counting notes had already named this shape — *"말하면 술어가
    /// 뒤집힌다"* — about a different predicate; a mark is the whole line's business, so a sentence
    /// containing it is prose.
    #[test]
    fn a_sentence_that_quotes_the_token_is_not_a_mark() {
        let ledger = LEDGER.replace(
            "     body mentioning ai_loop here",
            "     the population is `@ns: open` items — and `@ns: nonsense` is not a value\n     \
             ### ⭐ **처방: `@ns:` 한 줄**\n     body mentioning ai_loop here",
        );
        let reading = read(&ledger);
        assert!(
            reading.is_green(),
            "explaining the scheme is not writing a mark: {:?}",
            reading.faults,
        );
        assert_eq!(
            reading.population(),
            vec![900],
            "and the item's own mark still settles it",
        );
    }

    /// The declaration line begins with `@ns-` and must not be mistaken for a mark whose value is
    /// `unclassified:` — they share a prefix and only one of them is a membership statement.
    #[test]
    fn the_declaration_line_is_not_read_as_a_mark() {
        let reading = read(LEDGER);
        assert_eq!(reading.declared, Some(1), "the floor was read");
        assert!(
            !reading
                .faults
                .iter()
                .any(|fault| matches!(fault, Fault::UnknownTag { .. })),
            "and it produced no malformed mark: {:?}",
            reading.faults,
        );
    }

    /// A typo must not read as an absent mark — that would let one keystroke retire an item from
    /// the population with nothing said.
    #[test]
    fn a_mistyped_mark_is_a_fault_rather_than_a_silence() {
        let ledger = LEDGER.replace("@ns: out — a rendering", "@ns: outside — a rendering");
        let reading = read(&ledger);
        assert!(
            reading.faults.iter().any(|fault| matches!(
                fault,
                Fault::UnknownTag {
                    number: Some(898),
                    ..
                }
            )),
            "an unparseable value is its own fault: {:?}",
            reading.faults,
        );
        assert!(
            !reading
                .items
                .iter()
                .any(|item| item.number == 898 && item.tag.is_some()),
            "and it did not quietly become a valid mark",
        );
    }

    /// **THE RATCHET**: an item added without a mark raises the unmarked count above what the
    /// ledger declares, and that is red. This is what makes "unmarked" a debt rather than a default.
    #[test]
    fn an_item_added_without_a_mark_grows_the_backlog_and_reds() {
        let ledger = LEDGER.replace(
            "\n## B. Live product residues",
            "\n896. ⛔ **A new item nobody classified**\n     body\n\n## B. Live product residues",
        );
        let reading = read(&ledger);
        assert!(
            reading.faults.contains(&Fault::RatchetGrew {
                counted: 2,
                declared: 1,
            }),
            "the backlog may shrink, never grow: {:?}",
            reading.faults,
        );
    }

    /// And paying the backlog down is NOT a fault — otherwise the ratchet would punish the only
    /// move that ends it, and register item 823 would have no zero.
    #[test]
    fn marking_an_item_shrinks_the_backlog_without_complaint() {
        let ledger = LEDGER.replace(
            "897. ⛔ **Unmarked and quiet**\n     no mark, no mention",
            "897. ⛔ **Now classified**\n     @ns: out — a build-system item\n     no mention",
        );
        let reading = read(&ledger);
        assert!(reading.unclassified().is_empty(), "the backlog emptied");
        assert!(
            reading.is_green(),
            "counting fewer than declared is the goal, not a fault: {:?}",
            reading.faults,
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE SAME SHAPE, ON THE FLOOR LINE** — and this one was green BY LUCK until it was
    /// measured (2026-09-02).
    ///
    /// Item 823 explains the scheme, so its entry quotes this token in prose; the ledger therefore
    /// carried two lines containing it. The first reading counted a line by the token appearing
    /// ANYWHERE, and stayed green only because the prose one had no number after it and was
    /// silently dropped. Both halves were wrong: a sentence that happened to quote a number would
    /// have reddened the ledger for describing itself, and a typo in the real declaration would
    /// have let the sentence take its place.
    #[test]
    fn a_sentence_that_quotes_the_declaration_is_not_one() {
        let ledger = LEDGER.replace(
            "     body mentioning ai_loop here",
            "     the ledger declares its floor on an `@ns-unclassified: 7` line — that is prose\n     \
             body mentioning ai_loop here",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.declared,
            Some(1),
            "the real floor still won: {:?}",
            reading.faults,
        );
        assert!(
            reading.is_green(),
            "and quoting the token in a sentence is not a second declaration: {:?}",
            reading.faults,
        );
    }

    /// **A FLOOR NOBODY CAN READ IS NOT A FLOOR.** Dropping an unparseable declaration quietly is
    /// the escape hatch that let the prose stand in for the real line.
    #[test]
    fn a_declaration_with_no_number_is_a_fault_rather_than_a_skip() {
        let ledger = LEDGER.replace("@ns-unclassified: 1", "@ns-unclassified: soon");
        let reading = read(&ledger);
        assert!(
            reading
                .faults
                .iter()
                .any(|fault| matches!(fault, Fault::UnreadableDeclaration { .. })),
            "an unreadable floor is its own fault: {:?}",
            reading.faults,
        );
        assert_eq!(reading.declared, None, "and it declared nothing");
    }

    /// A missing floor is a fault: a ratchet nobody declared holds nothing, and this gate would
    /// then be green on any ledger at all.
    #[test]
    fn a_ledger_with_no_declaration_is_red() {
        let ledger = LEDGER.replace("@ns-unclassified: 1", "");
        let reading = read(&ledger);
        assert!(
            reading.faults.contains(&Fault::Declaration { found: 0 }),
            "no floor is not a pass: {:?}",
            reading.faults,
        );
    }

    /// The topmost block wins, which is how this ledger closes an item — by laying a new block on
    /// top of the original rather than editing it.
    #[test]
    fn the_newest_block_of_a_number_states_the_items_case() {
        let ledger = LEDGER.replace(
            "899. ✅✅ **PAID 2026-09-02**\n     @ns: paid",
            "899. ✅✅ **PAID 2026-09-02**\n     @ns: paid\n\n899. ⛔ **(original)**\n     @ns: open",
        );
        let reading = read(&ledger);
        assert!(
            !reading.population().contains(&899),
            "the payment sits above the original and settles it: {:?}",
            reading.population(),
        );
    }

    // ── register item 833(1): severity ─────────────────────────────────────────────────────────

    /// **WHAT A ROUND TAKES FIRST.** The critical set is the mark, exactly as the population is.
    #[test]
    fn what_to_take_first_is_the_severity_mark_and_nothing_else() {
        let reading = read(LEDGER);
        assert_eq!(
            reading.critical(),
            vec![900],
            "only an OPEN item marked `{SEVERITY} critical` is taken first",
        );
        assert!(
            reading.severity_unclassified().is_empty(),
            "every open item states a severity here: {:?}",
            reading.severity_unclassified(),
        );
        assert!(reading.is_green(), "faults: {:?}", reading.faults);
    }

    /// ⚠⚠ **A PAID ITEM IS NOT TAKEN FIRST, WHATEVER IT SAYS.** Severity is read for the
    /// population only — otherwise closing something would keep it at the head of the queue.
    #[test]
    fn a_severity_on_a_paid_item_does_not_reach_the_queue() {
        let ledger = LEDGER.replace(
            "899. ✅✅ **PAID 2026-09-02**\n     @ns: paid",
            "899. ✅✅ **PAID 2026-09-02**\n     @ns: paid\n     @sev: critical — it was, once",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.critical(),
            vec![900],
            "899 is paid, so its severity is history: {:?}",
            reading.critical(),
        );
    }

    /// **UNCLASSIFIED IS NOT ORDINARY** — working rule 6. An open item that states nothing is
    /// carried as a debt rather than silently sorted to the back.
    #[test]
    fn an_open_item_with_no_severity_is_carried_not_assumed() {
        let ledger = LEDGER.replace("     @sev: critical — it stops the loop dead\n", "");
        let reading = read(&ledger);
        assert_eq!(
            reading.severity_unclassified(),
            vec![900],
            "it is named, not assumed ordinary: {:?}",
            reading.severity_unclassified(),
        );
        assert!(
            reading.critical().is_empty(),
            "and it is certainly not critical: {:?}",
            reading.critical(),
        );
        assert!(
            reading.faults.contains(&Fault::SeverityRatchetGrew {
                counted: 1,
                declared: 0,
            }),
            "the backlog grew and the ratchet says so: {:?}",
            reading.faults,
        );
    }

    /// A mistyped severity must not read as an absent one — [`Fault::UnknownTag`]'s rule, one mark
    /// over.
    #[test]
    fn a_mistyped_severity_is_a_fault_rather_than_a_silence() {
        let ledger = LEDGER.replace("@sev: critical — it stops", "@sev: urgent — it stops");
        let reading = read(&ledger);
        assert!(
            reading.faults.iter().any(|fault| matches!(
                fault,
                Fault::UnknownSeverity {
                    number: Some(900),
                    ..
                }
            )),
            "`urgent` is not a value and saying nothing about it would hide the item: {:?}",
            reading.faults,
        );
    }

    /// ⚠⚠⚠ **THE 823 SHAPE, ONE MARK OVER**: this module's own prose quotes the token, and a
    /// reading that took it from anywhere in the line would turn documentation into malformed
    /// marks. A mark is the whole line's business.
    #[test]
    fn a_sentence_that_quotes_the_severity_token_is_not_one() {
        let ledger = LEDGER.replace(
            "     body mentioning ai_loop here",
            "     the queue is `@sev: critical` items — and `@sev: nonsense` is not a value\n     \
             body mentioning ai_loop here",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.critical(),
            vec![900],
            "the sentence describes the scheme and does not join it",
        );
        assert!(
            reading.is_green(),
            "describing the scheme is not a fault: {:?}",
            reading.faults,
        );
    }

    /// The severity declaration begins with `@sev-` and must not be mistaken for a mark whose
    /// value is `-unclassified:` — the trap [`DECLARATION`] already fell into once.
    #[test]
    fn the_severity_declaration_line_is_not_read_as_a_mark() {
        let reading = read(LEDGER);
        assert!(
            !reading
                .faults
                .iter()
                .any(|fault| matches!(fault, Fault::UnknownSeverity { .. })),
            "`{SEVERITY_DECLARATION}` is a floor, not a malformed mark: {:?}",
            reading.faults,
        );
        assert_eq!(reading.severity_declared, Some(0));
    }

    /// A ledger that declares no floor for this backlog is red — a ratchet with nothing to ratchet
    /// against holds nothing.
    #[test]
    fn a_ledger_with_no_severity_declaration_is_red() {
        let ledger = LEDGER.replace("@sev-unclassified: 0\n", "");
        let reading = read(&ledger);
        assert!(
            reading
                .faults
                .contains(&Fault::SeverityDeclaration { found: 0 }),
            "no floor is not a pass: {:?}",
            reading.faults,
        );
    }

    // ── register item 833(2): the debt chain ───────────────────────────────────────────────────

    /// A chain hung off 900: 901 was found while paying it, 902 while paying 901.
    fn with_a_chain() -> String {
        let link = |number: u32, from: u32| {
            format!(
                "{number}. ⛔ **Found while paying {from}**\n     @ns: open\n     @sev: ordinary\n \
                 \u{20}   @from: {from}\n\n"
            )
        };
        LEDGER.replace(
            "900. ⛔ **An open loop item**",
            &format!(
                "{}{}900. ⛔ **An open loop item**",
                link(902, 901),
                link(901, 900)
            ),
        )
    }

    /// **DEPTH IS THE ITEM'S, NOT THE RUN'S** — the whole point of register item 833(2). A debt
    /// found while paying a debt is one step further down, and no run boundary resets that.
    #[test]
    fn a_debt_found_while_paying_one_sits_a_step_below_it() {
        let reading = read(&with_a_chain());
        assert_eq!(reading.depth(900), Some(0), "nothing found it");
        assert_eq!(reading.depth(901), Some(1), "found while paying a root");
        assert_eq!(reading.depth(902), Some(2), "found while paying that");
        assert!(reading.is_green(), "faults: {:?}", reading.faults);
    }

    /// **THE CAP THE OWNER SET** — at 1, the chain's third link is registered and not taken.
    #[test]
    fn the_default_cap_takes_one_step_off_the_brief_and_defers_the_next() {
        let reading = read(&with_a_chain());
        assert_eq!(
            reading.takeable(1),
            vec![900, 901],
            "a root and one step off it are work; the step below is not",
        );
        assert_eq!(
            reading.deferred(1),
            vec![902],
            "and the held-back one is NAMED — a cap nobody can count is a quiet deferral",
        );
    }

    // ── register item 839: what a round may take, as a predicate ──────────────────────────────

    /// 🎯🎯🎯🎯🎯 **WHILE ANYTHING IS CRITICAL, THE SET IS THOSE** — working rule 11 made into a
    /// predicate, and the half of register item 833(1) nothing measured.
    ///
    /// ⚠ [`LEDGER`]'s 900 is the only open item and it is critical, so the two arms are separated
    /// by a ledger with an ordinary sibling: a build that returned the whole population would be
    /// green against a ledger whose critical set IS its population.
    #[test]
    fn while_anything_is_critical_the_admissible_set_is_those() {
        let ledger = LEDGER.replace(
            "900. ⛔ **An open loop item**",
            "895. ⛔ **An ordinary open item**\n     @ns: open\n     @sev: ordinary\n     @from: \
             none\n\n900. ⛔ **An open loop item**",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.population(),
            vec![895, 900],
            "THE CONTROL: two open items, or the narrowing below is about nothing",
        );
        assert_eq!(
            reading.admits(1),
            vec![900],
            "🎯 the ordinary one is open, takeable, and NOT what a round takes next — that is the \
             whole of working rule 11, and until this predicate existed it was prose",
        );
    }

    /// **AND WHEN NONE IS, THE SET IS THE POPULATION THE CAP ALLOWS** — working rule 11's other
    /// half, and register item 659's counter-argument kept: this is a gate, not a sort.
    #[test]
    fn with_nothing_critical_the_admissible_set_is_what_the_cap_allows() {
        let ledger = with_a_chain().replace(
            "     @sev: critical — it stops the loop dead",
            "     @sev: ordinary — nothing is urgent here",
        );
        let reading = read(&ledger);
        assert!(
            reading.critical().is_empty(),
            "THE CONTROL: nothing is critical, or this is the other arm: {:?}",
            reading.critical(),
        );
        assert_eq!(
            reading.admits(1),
            vec![900, 901],
            "the population minus what the depth cap holds back — and 902 is held back, which is \
             what makes the fall-through incapable of admitting more than `takeable` does",
        );
    }

    /// ⛔⛔⛔ **AND A CRITICAL ITEM THE CAP HOLDS BACK IS NOT ADMITTED EITHER** — the arms are
    /// composed rather than chosen between, so severity cannot smuggle a deferred item past the
    /// depth cap.
    #[test]
    fn a_critical_item_deeper_than_the_cap_is_still_held_back() {
        let ledger = with_a_chain()
            .replace(
                "     @sev: critical — it stops the loop dead",
                "     @sev: ordinary — nothing is urgent here",
            )
            .replace(
                "902. ⛔ **Found while paying 901**\n     @ns: open\n     @sev: ordinary",
                "902. ⛔ **Found while paying 901**\n     @ns: open\n     @sev: critical",
            );
        let reading = read(&ledger);
        assert_eq!(
            reading.critical(),
            vec![902],
            "THE CONTROL: the only critical item is the one at depth 2: {:?}",
            reading.faults,
        );
        assert_eq!(
            reading.admits(1),
            vec![900, 901],
            "⛔ a critical mark does not lift the depth cap. Taking 902 here would make the cap \
             something a round could escape by ranking its own finding",
        );
    }

    // ── register item 840: deeper against sideways ────────────────────────────────────────────

    /// 🎯🎯🎯🎯🎯 **A DEBT THE WORK IN HAND CREATED IS A STEP; AN UNRELATED ROOT IS NOT** — the
    /// distinction a re-aiming budget was counting as one movement, and the reason a capped run
    /// could not move to the next thing at all.
    #[test]
    fn a_finding_that_came_out_of_the_work_in_hand_is_a_step_and_a_root_is_not() {
        let reading = read(&with_a_chain());
        assert_eq!(
            reading.descends(900, 901),
            Some(true),
            "901 says it was found while paying 900, so taking it goes one step deeper",
        );
        assert_eq!(
            reading.descends(900, 902),
            Some(true),
            "and so does the step below that — the chain is walked, not just its first link",
        );
        assert_eq!(
            reading.descends(901, 900),
            Some(false),
            "🎯 but 900 is a declared ROOT: nothing found it, so moving there from 901 is going \
             SIDEWAYS. Charging that is what left a capped run unable to take the next thing",
        );
        assert!(
            reading.sideways(Some(901), 900),
            "and the composed predicate says so in one call",
        );
        assert!(
            !reading.sideways(Some(900), 901),
            "while the movement the budget exists to bound is not sideways",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AN UNWRITTEN CHAIN IS CHARGED, WHICH IS WORKING RULE 6** — *unclassified is not
    /// a pass*, and *free* is the pass here. [`LEDGER`]'s 897 states no parentage.
    #[test]
    fn a_chain_nobody_wrote_down_is_not_the_cheap_answer() {
        let reading = read(LEDGER);
        assert_eq!(
            reading.descends(900, 897),
            None,
            "THE CONTROL: 897 states no `@from:`, so this cannot be walked to either answer",
        );
        assert!(
            !reading.sideways(Some(900), 897),
            "⛔ and *cannot tell* must not read as *unrelated*: a build that folded them together \
             would let every unannotated item escape the budget, and nothing about it would look \
             wrong. What unlocks the cheaper reading is the annotation the unrooted ratchet asks \
             for — this is what makes that ratchet worth paying down",
        );
        assert!(
            !reading.sideways(None, 900),
            "⚠ and neither may a proposal whose CHECKPOINT could not be placed: nothing was \
             compared, so nothing may be called unrelated",
        );
    }

    /// **WHICH ITEM A PROPOSAL NAMES** — the first number in it this ledger files something under.
    #[test]
    fn a_proposal_is_read_as_the_first_register_item_it_names() {
        let reading = read(LEDGER);
        assert_eq!(
            reading.names("항목 900 을 갚아라 — 897 도 같은 얼굴이다"),
            Some(900),
            "the first item named is what the milestone is about; the later citation is a citation",
        );
        assert_eq!(
            reading.names("2026-09-02 에 잰 72 바이트 한계"),
            None,
            "⚠ a year and a byte count are not items — numbers are filtered through what the \
             ledger actually files, which is why no shape rule is needed",
        );
        assert_eq!(
            reading.names("항목 896 은 섹션 B 다"),
            None,
            "and section B is not this ledger's population, so nothing here is filed under 896",
        );
    }

    /// ⚠ The standing backlog states no parentage, and refusing all of it would stop the loop on
    /// the day this shipped. Unknown depth is takeable; the cap bites on the chain it can see.
    #[test]
    fn an_item_that_states_no_parent_is_still_work() {
        let reading = read(LEDGER);
        assert_eq!(reading.depth(897), None, "897 states nothing");
        assert!(
            reading.takeable(1).contains(&900),
            "the population is still workable: {:?}",
            reading.takeable(1),
        );
        assert!(
            reading.deferred(1).is_empty(),
            "nothing is held back by a chain nobody stated: {:?}",
            reading.deferred(1),
        );
    }

    /// A parent section A does not have breaks the walk, and **that must not read as depth 0** —
    /// a dangling chain would otherwise promote everything below it to a root.
    #[test]
    fn a_parent_the_ledger_does_not_have_is_a_fault_and_not_a_root() {
        let ledger = with_a_chain().replace("     @from: 900", "     @from: 404");
        let reading = read(&ledger);
        assert!(
            reading.faults.contains(&Fault::DanglingParent {
                number: 901,
                named: 404,
            }),
            "the chain leaves the ledger and the reading says so: {:?}",
            reading.faults,
        );
        assert_eq!(
            reading.depth(902),
            None,
            "and nothing below it claims a depth it cannot support",
        );
    }

    /// A chain that returns to itself must be a fault rather than a silent `None`.
    #[test]
    fn a_parent_chain_that_eats_itself_is_named() {
        let ledger = with_a_chain().replace("     @from: none", "     @from: 902");
        let reading = read(&ledger);
        assert!(
            reading
                .faults
                .iter()
                .any(|fault| matches!(fault, Fault::ParentCycle { .. })),
            "depth would not terminate and saying nothing would hide it: {:?}",
            reading.faults,
        );
    }

    /// `none` is a VALUE. A typo must not read as absent, and absent must not read as a root.
    #[test]
    fn a_mistyped_parent_is_a_fault_rather_than_a_silence() {
        let ledger = LEDGER.replace("     @from: none", "     @from: nobody");
        let reading = read(&ledger);
        assert!(
            reading.faults.iter().any(|fault| matches!(
                fault,
                Fault::UnknownParent {
                    number: Some(900),
                    ..
                }
            )),
            "`nobody` is neither `none` nor a number: {:?}",
            reading.faults,
        );
    }

    /// ⛔⛔⛔⛔ **CAUSED BY, NEVER MERELY MET WHILE** — handed over by `sprag-14`, and the reason
    /// this is a gate rather than a sentence in the docs.
    ///
    /// Two items written in one hour by one round paying 825: 836 was CREATED by that payment (a
    /// mutation build left behind became the dock's app), 837 was a red already standing in HEAD
    /// that the same suite reached. If both were children, every pre-existing debt anybody stumbles
    /// over sinks a level — and the oldest debts, which are stumbled over most, sink fastest.
    #[test]
    fn a_debt_a_round_merely_walked_into_is_a_root_and_does_not_sink() {
        let met = LEDGER.replace(
            "897. ⛔ **Unmarked and quiet**",
            "895. ⛔ **A red that was already in HEAD**\n     @ns: open\n     @sev: ordinary\n     \
             @from: none\n\n897. ⛔ **Unmarked and quiet**",
        );
        let reading = read(&met);
        assert_eq!(
            reading.depth(895),
            Some(0),
            "the round met it; nothing created it",
        );
        assert!(
            reading.takeable(1).contains(&895),
            "so the cap does not hold it back: {:?}",
            reading.takeable(1),
        );
        // The same item written as a child of the thing that was being paid sinks, which is what
        // the distinction buys — and what makes getting it wrong expensive.
        let caused = met.replace("     @from: none\n\n897.", "     @from: 900\n\n897.");
        let reading = read(&caused);
        assert_eq!(reading.depth(895), Some(1), "created by 900's payment");
    }

    /// An item added without parentage raises the standing count and reds — the same ratchet the
    /// other two marks hold.
    #[test]
    fn a_reason_that_says_revealed_is_not_a_reason_that_says_made() {
        // ⛔⛔⛔⛔⛔ ── THE SUBSTRING COLLISION, WHICH IS THE WHOLE OF THIS TEST ──────────────────
        //
        // `드러났다` CONTAINS `났다`. Asking both vocabularies over the whole sentence therefore
        // reads every *was revealed* line as a *made by paying* line, and the first build of
        // `met_while` did exactly that: it reported two of the four lines it was written for and
        // silently passed item 868, the `@sev: critical` one three rounds had recorded as
        // un-takeable. Two vocabularies over one string collide because Korean inflects by suffix.
        assert_eq!(
            met_while("865 가 그 물음을 세우자 «성공»했고, 그때 드러났다"),
            Some("드러났다"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 896: a `드러났다` line read as a creation claim, because \
             `났다` is inside it. This is the arm that made the check pass the item it existed for",
        );
        assert_eq!(
            met_while("852 를 재느라 GUI 로그를 읽다 «마주친» 것이다"),
            Some("마주친"),
            "⚠ the plainest form, and the one nine `@from: none` lines already use",
        );
        assert_eq!(
            met_while("pinion 감시자가 «자기 런에서» 재서 넘겨줬다"),
            Some("넘겨줬다"),
        );

        // ── AND A LINE DRAWING THE DISTINCTION IS NOT CAUGHT ────────────────────────────────
        //
        // ⚠ The ledger does this twice on purpose, so a creation word beside the other settles
        // the line. Without these arms the gate would red on two correctly-filed children and the
        // repair would be to delete a sentence that is doing real work.
        assert_eq!(
            met_while("갚다 «마주친» 것이 아니라 갚으면서 내가 만들었다"),
            None,
            "⚠⚠ item 864's own line: it names the wrong reading in order to refuse it",
        );
        assert_eq!(
            met_while(
                "868 의 승격을 실제로 해서 났다. 갚다가 «생긴» 것이 아니라 승격이 «만드는» 것이다"
            ),
            None,
            "⚠⚠ item 869's own line, and `났다` is doing the work here rather than colliding",
        );
        assert_eq!(
            met_while("840 을 갚으며 «만든» 리더다"),
            None,
            "⚠ the ordinary child, which must stay quiet",
        );

        // ── AND THE PARSE SITE ACTUALLY CALLS IT, which is a hop of its own ─────────────────
        //
        // ⛔⛔⛔⛔⛔ The assertions above hold the PREDICATE. `Parent::parse` takes the first word
        // of the value and drops the rest, so a build that handed this function that first word —
        // or never called it — passes every one of them and reads the ledger exactly as it did
        // before item 896. Items 889, 894, 891 and 893 each found a hop outside their gate while
        // the one beside it was green; this is that lesson spent before paying for it again.
        let ledger = LEDGER.replace(
            "     @from: none\n     body mentioning ai_loop here",
            "     @from: 898 — 898 을 재느라 로그를 읽다 «마주친» 것이다\n     body mentioning \
             ai_loop here",
        );
        let reading = read(&ledger);
        assert!(
            reading.faults.contains(&Fault::MetWhileNotMade {
                number: 900,
                named: 898,
                word: "마주친",
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 896: the reading did not carry the fault, so the reason is \
             being discarded at the parse site again — the exact shape that let four lines name a \
             parent they had only met. Faults: {:?}",
            reading.faults,
        );
    }

    #[test]
    fn an_item_added_without_a_parent_grows_the_backlog_and_reds() {
        let ledger = LEDGER.replace(
            "897. ⛔ **Unmarked and quiet**",
            "895. ⛔ **New, and says nothing about where it came from**\n     @ns: out\n\n897. ⛔ \
             **Unmarked and quiet**",
        );
        let reading = read(&ledger);
        assert!(
            reading.faults.contains(&Fault::ParentRatchetGrew {
                counted: 4,
                declared: 3,
            }),
            "the backlog may shrink, never grow: {:?}",
            reading.faults,
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE CAP THIS INSTRUMENT JUDGES UNDER IS THE DOCUMENT'S** — register item 833(1),
    /// and the sentence that was a comment while the code held a literal.
    ///
    /// # ⛔⛔⛔⛔⛔ What the disagreement cost, measured
    ///
    /// `north-star.rs` carried *"THE CAP IS THE DOCUMENT'S, NOT THIS BINARY'S"* two lines above
    /// `.unwrap_or(1)`, twice. Driven 2026-09-04: `debt_loop.scxml`'s `reaim_max` set to `2`, the
    /// binary rebuilt, and it went on printing `deferred 10 at depth > 1` and refusing item 843
    /// with *"sits deeper than 1"*. **Five critical items held back by a number the document no
    /// longer declared.**
    ///
    /// # ⚠⚠⚠ The arms are the three ways a document can fail to say it, and none is a `1`
    ///
    /// Rule 6: an unreadable policy must be a REFUSAL, because a default here is the escape hatch
    /// that retires the gate — and the default it would take is precisely the value that was wrong.
    #[test]
    fn the_reaim_cap_is_the_documents_and_a_document_that_says_none_is_refused() {
        // ── ① THE NUMBER COMES OUT OF THE TEXT, AND A DIFFERENT TEXT GIVES A DIFFERENT NUMBER ──
        //
        // ⚠ Both arms, because *reads the document* and *returns 1* agree on every document that
        // says 1 — which is every document this repository ships today.
        assert_eq!(
            declared_reaim("  <data id=\"reaim_max\" expr=\"1\"/>"),
            Ok(Reaim::Of(1)),
        );
        assert_eq!(
            declared_reaim("  <data id=\"reaim_max\" expr=\"2\"/>"),
            Ok(Reaim::Of(2)),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 833(1): a document that raised its cap is being judged under \
             the old one, which is the measured defect — five critical items held back by a number \
             nobody had declared since",
        );

        // ── ② `never` IS A VALUE THE DOCUMENT MAY SAY, AND IT IS NOT AN ABSENCE ──
        //
        // ⚠ The document's own guard reads `reaim_max != 'never'`, so this is its vocabulary and
        // not an invention here. It is quoted in the text because `expr` is an expression.
        assert_eq!(
            declared_reaim("<data id=\"reaim_max\" expr=\"'never'\"/>"),
            Ok(Reaim::Never),
        );
        assert_eq!(
            Reaim::Never.depth(),
            u32::MAX,
            "declining the cap must hold nothing back",
        );
        assert_eq!(Reaim::Never.spelled(), "never");

        // ── ③ RULE 6: THE THREE SILENCES ARE REFUSALS, NEVER THE DEFAULT ──
        for (named, document) in [
            ("declares none", "<data id=\"something_else\" expr=\"1\"/>"),
            (
                "declares two",
                "<data id=\"reaim_max\" expr=\"1\"/>\n<data id=\"reaim_max\" expr=\"3\"/>",
            ),
            (
                "declares a word nobody can read",
                "<data id=\"reaim_max\" expr=\"'soon'\"/>",
            ),
            ("declares it with no expr", "<data id=\"reaim_max\"/>"),
        ] {
            assert!(
                declared_reaim(document).is_err(),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 833(1) AND RULE 6: a document that {named} was read \
                 anyway. The value this reader would otherwise invent is `1` — the exact number \
                 that was wrong — so a default here is the escape hatch that retires the gate",
            );
        }
    }

    /// ⛔⛔⛔⛔ **AND THE DOCUMENT THIS REPOSITORY'S RUNS OBEY DECLARES ONE** — the control for the
    /// gate above, and the one arm that would notice the file being renamed or the `<data>` going.
    ///
    /// ⚠⚠ It reads `debt_loop.scxml` and not `ai_loop.scxml`: the template is what other
    /// repositories copy, and a run of THIS ledger is driven by the kind. `north-star.rs` bakes the
    /// same file in, and this is the assertion that the file it bakes in still answers.
    #[test]
    fn this_repositorys_own_loop_document_declares_a_cap_this_reader_can_use() {
        let document = include_str!("../../sprag-plugin/src/debt_loop.scxml");
        let cap = declared_reaim(document).expect(
            "⛔⛔⛔⛔⛔ REGISTER ITEM 833(1): `debt_loop.scxml` declares no cap this reader can \
             use, so `north-star` refuses to judge — which is the honest answer and a red here, \
             because every round of this repository is gated by that judgement",
        );
        assert_eq!(
            cap,
            Reaim::Of(1),
            "⚠⚠ THE OWNER'S DEFAULT, register item 833(2): *부채의 부채는 몇 depth까지 갚을지 \
             scxml에 지정할수있게하고 default로 1 depth로해*. A round that moves it moves this \
             line with it — which is the whole point of the number living in one place.",
        );
    }
}
