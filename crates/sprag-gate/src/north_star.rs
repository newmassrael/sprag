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
//!
//! ⚠⚠⚠ AND A COUNT *BELOW* THE DECLARATION IS [`crate::north_star::Fault::RatchetSlack`] — register
//! item 926, and the half this paragraph described for as long as it existed without holding it. A
//! floor left standing above the count is not a harmless margin: it is exactly that many items that
//! can be registered unmarked before anything says a word, and two of these four ratchets had
//! drifted that way on the real ledger. So the floor must EQUAL the count, which makes paying the
//! backlog down a two-part edit — mark the item, lower the floor — and the refusal names the number
//! to write so that stays one line rather than an investigation.

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

/// ⛔⛔⛔⛔⛔ **WHAT ONE [`PARENT`] REASON STATES** — [`MET_WHILE`] and [`MADE_BY_PAYING`] read as
/// a single answer, so the third case has a NAME instead of being the gap between two `bool`s.
/// Register item 920.
///
/// # ⛔⛔⛔ Why the third case is a value and not a `None` nobody looks at
///
/// [`MET_WHILE`]'s own doc records three reasons in neither vocabulary — `입구`, `골라 넣은 수다`,
/// `갈랐다` — and calls widening the arrays *the one move the north star forbids*. That left the
/// unreadable reason with no representation at all: [`met_while`] answers [`None`] for *it says
/// made* and for *nobody can tell* alike, which is working rule 6's escape hatch exactly.
/// [`Says::Neither`] is the name that lets a caller refuse it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Says {
    /// Paying the named item MADE this debt — [`MADE_BY_PAYING`]. Carries the word it matched, so
    /// a reader is shown the evidence rather than asked to trust the verdict.
    Made(&'static str),
    /// The round MET this debt while paying the named item — [`MET_WHILE`] with no creation word
    /// beside it. On a numbered [`PARENT`] that is [`Fault::MetWhileNotMade`].
    Met(&'static str),
    /// Neither vocabulary reaches it. **Not a pass** — see [`Reading::deferred_unread`].
    Neither,
}

/// ⛔⛔⛔⛔⛔ **READ ONE [`PARENT`] REASON** — the single place the two vocabularies meet.
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
///
/// ⚠ A line carrying BOTH answers [`Says::Made`]: it is drawing the distinction rather than falling
/// foul of it, which [`MADE_BY_PAYING`]'s doc argues from the two lines that do it on purpose.
#[must_use]
pub fn says(reason: &str) -> Says {
    if let Some(word) = MET_WHILE.into_iter().find(|word| reason.contains(word)) {
        let without = reason.replace(word, " ");
        return match MADE_BY_PAYING.iter().find(|made| without.contains(*made)) {
            Some(made) => Says::Made(made),
            None => Says::Met(word),
        };
    }
    match MADE_BY_PAYING.iter().find(|made| reason.contains(*made)) {
        Some(made) => Says::Made(made),
        None => Says::Neither,
    }
}

/// Whether `reason` states that the debt was MET rather than made — [`Says::Met`] spelled as the
/// [`Option`] the reader's fault arm wants. Register item 896.
#[must_use]
pub fn met_while(reason: &str) -> Option<&'static str> {
    match says(reason) {
        Says::Met(word) => Some(word),
        Says::Made(_) | Says::Neither => None,
    }
}

/// ⛔⛔⛔⛔⛔ **THE LINE AN ITEM CLAIMS TO BE A STANDING RED ON**: `@red: <cargo test arguments>` —
/// register item 843.
///
/// # ⛔⛔⛔⛔ Two rules collided and only one of them was a machine
///
/// Working rule 11 — *while anything is critical, take from those* — became [`Reading::admits`],
/// enforced on every proposal. The standing rule beside it — **a red is paid in its own round** —
/// stayed prose in `CLAUDE.md` and the round ritual, measured by nobody. So the enforced one won:
/// item 837 was `@sev: ordinary` and **stood red for four days**, unreachable to an unattended loop
/// for as long as any critical item stood, because a proposal naming it was counted and not taken.
///
/// ⇒ ⛔ And the repair is NOT to write `@sev: critical` on the red, which 843 refuses in its own
/// words: that makes the instrument disagree with what severity MEANS. Being red is a fact about
/// the tree, not a judgement about how much a debt hurts, so it is a THIRD thing and it gets its
/// own mark.
///
/// # ⚠⚠⚠ What the value is, and why it is argv rather than prose
///
/// The ledger already names its reds — item 837's own table carries
/// `plugins::tests::a_loop_started_over_the_wire_prompts_its_agent_with_what_the_caller_briefed`
/// beside `cargo test -p sprag-host --lib` — and **nothing has ever parsed it**, which is this
/// workspace's rule 10 exactly. This mark is that table's first two columns in a form the
/// repository can be ASKED: the arguments after `cargo test`, e.g.
///
/// ```text
/// @red: -p sprag-host --lib plugins::tests::a_loop_started_over_the_wire --exact
/// ```
///
/// ⚠⚠ Passed as ARGV and never through a shell, and every token is checked against
/// `safe_argument` first — SPELLED rather than linked, because it is private and this constant is
/// public, which is `private_intra_doc_links` under `-D warnings` (register item 365, and the doc
/// gate refused this file for it). The same care [`Commits`] takes with an id read off a mark line,
/// and for a sharper reason: this one is executed rather than looked up.
///
/// ⚠ It is read from OPEN items only. A paid item's red is history, and one outside the population
/// was never this loop's to run.
pub const RED: &str = "@red:";

/// Whether one token of a [`RED`] value may be handed to `cargo test`.
///
/// ⛔⛔⛔ **A CLOSED SET RATHER THAN A LIST OF THINGS TO REFUSE.** A denylist of shell
/// metacharacters would be a guess about every future reader of this string; this admits the
/// characters cargo's own selectors are made of — `--flags`, `crate-names`, `module::paths`,
/// `file.rs` — and refuses everything else, so a value that could do anything surprising cannot be
/// spelled at all. Register item 843, and this crate does not open a shell in any case.
fn safe_argument(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|at| at.is_ascii_alphanumeric() || matches!(at, '-' | '_' | ':' | '.' | '/'))
}

/// The line the ledger declares its own count of items that state no [`PARENT`]:
/// `@from-unclassified: <n>`.
///
/// ⚠ Every item written before this mark existed is unstated, and demanding they all be annotated
/// at once is the kind of retroactive sweep that gets abandoned half-done. The ratchet holds the
/// standing count instead: a NEW item must state its parentage, because adding one without it
/// raises the count above the floor and reds.
///
/// # ⛔⛔⛔⛔⛔ That last sentence was FALSE for as long as nobody re-asked it — register item 926
///
/// It is a claim about *today's* floor, not about the code, and it is only true while the floor
/// still touches the count. Measured 2026-09-06 12:43 UTC: this declaration said 386 against 382
/// counted, so **four items could have been registered with no parentage and this sentence would
/// have been a lie about every one of them.** Item 823 repeats the claim and was equally stale.
///
/// ⚠⚠ What makes it true now is not that somebody corrected the number — that lasts until the next
/// payment moves the count — but that [`Fault::RatchetSlack`] re-asks it on every reading and
/// refuses a floor standing above the count. **A promise in a doc comment is kept by a gate or it
/// is not kept.** Re-derived rather than trusted: with the floors brought down, adding one unmarked
/// item to a copy of the real ledger takes it from rc=0 to rc=1, which is the experiment item 926
/// ran to show the opposite.
pub const PARENT_DECLARATION: &str = "@from-unclassified:";

/// ⛔⛔⛔⛔⛔ **THE LINE THE LEDGER DECLARES ITS OWN COUNT OF PAID ITEMS THAT NAME NO COMMIT**:
/// `@paid-uncommitted: <n>` — register item 902.
///
/// # ⛔⛔⛔⛔⛔ What went wrong, measured rather than argued
///
/// A round opened with the ledger's `@ns: paid` section for item 866(2) fully written — the
/// argument, the mutation table, the numbers — while the code it describes was **720 uncommitted
/// lines across eleven files**, and had never been through clippy, rustfmt or the rustdoc gate.
/// The mark was correctly formed and correctly parsed. It was simply not TRUE about the product.
///
/// ⇒ ⭐ **No predicate inside the ledger can catch that, because the ledger does not look at the
/// tree.** [`DECLARATION`]'s hazard is an item with no mark; this one's is a mark whose claim is
/// false, which is strictly worse: an unmarked item is still in [`Reading::unclassified`] and still
/// owed, while a wrongly-paid item leaves [`Reading::population`] and **cannot be found again**.
///
/// # ⚠⚠ Why a COMMIT ID and never *is the working tree clean*
///
/// This repository's tree has two writers (register item 196 — an unattended loop commits into it),
/// so a dirty tree can never be attributed to a particular item: it says somebody is mid-round, not
/// that item 866 is unpaid. A commit id is per-item, is what a reader chases anyway, and can be put
/// to `git` — see [`Commits`]. The convention already exists and is already unevenly kept, which is
/// what makes this a ratchet rather than a demand: it is read off the mark line, in backticks.
///
/// # ⚠ The ratchet direction, and rule 5
///
/// The floor may only fall. Every existing paid line that names nothing is held here, and the path
/// to zero is to read them and fill in what each round actually committed — the same shape
/// [`DECLARATION`] holds for unmarked items, and the same reason: a retroactive sweep of forty-odd
/// items gets abandoned half-done, while a NEW paid mark that names no commit raises the count
/// above the floor and reds on the round that wrote it.
pub const PAID_DECLARATION: &str = "@paid-uncommitted:";

/// The shortest and longest a commit id may be for [`named_commits`] to read it as one.
///
/// ⚠ SEVEN is git's own abbreviation floor, and it is what keeps this from reading ordinary
/// backticked prose as a commit: `` `abcdef` `` is six and `` `held()` `` is not hex at all.
const COMMIT_ID: std::ops::RangeInclusive<usize> = 7..=40;

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

/// 🎯🎯🎯 **ONE HOP OF A [`PARENT`] CHAIN** — register item 920. An item, the item it names, and
/// what its own sentence [`Says`] about which of working rule 13's two things happened.
///
/// ⚠ A link is a STATEMENT, not a relation: `number` states that `named` is where it came from.
/// Only [`Parent::Item`] makes one — reaching [`Parent::Root`] ends the chain and adds no link,
/// which is what makes a root's depth 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    /// The item whose [`PARENT`] line this is.
    pub number: u32,
    /// The item that line names.
    pub named: u32,
    /// What the sentence after the number says. See [`says`].
    pub says: Says,
    /// ⛔⛔⛔⛔⛔ **WHETHER THE PARENT IT NAMES IS STILL A DEBT** — register item 921, and the fact
    /// [`Reading::debts_above`] counts. [`None`] where that item states no mark at all, which is
    /// **not** the same as a closed one: see [`Reading::debts_above`] for why an unknown mark holds
    /// the link rather than releasing it.
    pub parent: Option<Tag>,
}

impl fmt::Display for Link {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            number,
            named,
            says,
            parent,
        } = self;
        // ⚠ The parent's mark is printed BESIDE the reason and not instead of it: one says why the
        // link exists and the other whether it still holds anything back — register item 921 split
        // exactly those two, and a reader of a deferral needs both.
        let mark = match parent {
            Some(Tag::Open) => "owed",
            Some(Tag::Paid) => "PAID",
            Some(Tag::Out) => "OUT",
            None => "unmarked",
        };
        match says {
            Says::Made(word) => write!(f, "{number} from {named} [{mark}] (made, `{word}`)"),
            Says::Met(word) => write!(f, "{number} from {named} [{mark}] (MET, `{word}`)"),
            Says::Neither => write!(f, "{number} from {named} [{mark}] (UNREAD)"),
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
    /// ⛔⛔⛔⛔⛔ **WHAT THIS ITEM CLAIMS IS RED**, if it claims anything — register item 843. The
    /// value of its [`RED`] line, which is the argv `cargo test` is to be asked with. [`None`] for
    /// every item that makes no such claim, which is nearly all of them.
    ///
    /// ⚠ A CLAIM AND NOT A FACT. Whether it is true is [`Reading::standing_reds`]' question, and it
    /// is put to the repository — see [`Suite`].
    pub red: Option<String>,
    /// ⛔⛔⛔ **THE SENTENCE THAT FOLLOWS [`PARENT`]'s VALUE**, kept rather than dropped once the
    /// number is read — register item 920. It comes off the SAME block that settled
    /// [`Item::parent`], so a superseded block cannot explain a mark that beat it. [`None`] where
    /// the item states no parentage at all. See [`Reading::chain`].
    pub reason: Option<String>,
    /// ⛔⛔⛔⛔⛔ **THE COMMIT IDS ITS MARK LINE NAMES** — register item 902. Empty for an item
    /// whose mark names none, and empty for every item that is not [`Tag::Paid`], because only a
    /// claim of payment can be checked against the repository. See [`PAID_DECLARATION`].
    pub commits: Vec<String>,
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
        /// What this reading counted — the items themselves, register item 934.
        counted: Vec<u32>,
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
    /// ⛔⛔⛔⛔⛔ **AN ITEM HELD BELOW THE DEPTH CAP BY A LINK IN NEITHER VOCABULARY** — register
    /// item 920. The reason answers [`Says::Neither`], so no round ever stated which of working
    /// rule 13's two things happened, and the deferral it produces rests on nothing anybody argued.
    ///
    /// ⚠ The repair is never to widen [`MET_WHILE`] or [`MADE_BY_PAYING`] — [`MET_WHILE`]'s doc
    /// calls that *the one move the north star forbids*. It is to make the ledger line STATE its
    /// verdict: a creation word if paying `named` made it, `@from: none` if a round met it.
    DeferredByUnreadLink {
        /// The deferred item whose depth this link is part of.
        held: u32,
        /// The item whose [`PARENT`] line cannot be read.
        at: u32,
        /// The parent that line names.
        named: u32,
    },
    /// ⛔⛔⛔⛔⛔ **A [`RED`] CLAIM THIS INSTRUMENT CANNOT PUT TO THE REPOSITORY** — register item
    /// 843. Empty, or carrying a token outside `safe_argument` — spelled and not linked, for
    /// [`RED`]'s stated reason.
    ///
    /// ⚠ It is a RED and not a silence for item 902's reason one mark over: a claim that is never
    /// checked reads exactly like a checked one, and this claim buys an item past the severity
    /// gate. **An unrunnable claim must not be the cheap answer.**
    UnrunnableRed {
        /// The item that made it.
        number: u32,
        /// The line as written.
        line: String,
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
    /// More paid items name no commit than [`PAID_DECLARATION`] declares. Register item 902.
    PaidRatchetGrew {
        /// What this pass counted — the items themselves, register item 934.
        counted: Vec<u32>,
        /// What the ledger declared.
        declared: usize,
    },
    /// [`PAID_DECLARATION`] appears other than exactly once.
    PaidDeclaration {
        /// How many lines carried it.
        found: usize,
    },
    /// [`PAID_DECLARATION`] is present and states no number.
    UnreadablePaidDeclaration {
        /// The line as written.
        line: String,
    },
    ParentRatchetGrew {
        /// What this reading counted — the items themselves, register item 934.
        counted: Vec<u32>,
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
        /// What this reading counted — the items themselves, register item 934.
        counted: Vec<u32>,
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
    /// ⛔⛔⛔⛔⛔ **A BACKLOG WHOSE DECLARED OWNER HAS LEFT THE POPULATION** — register item 937.
    ///
    /// [`Reckoning::Owned`] is a claim about the ledger, and the ledger moves: the day that item
    /// is paid, the backlog silently has no owner again and nothing would have said so. **This is
    /// the disposition asking to be re-stated**, not a defect in the payment — the remedy is to
    /// judge the backlog again (a new owner, or an exemption with its sentence).
    BacklogOwnerClosed {
        /// The declaration whose backlog said it, e.g. [`PAID_DECLARATION`].
        token: &'static str,
        /// The item it named.
        owner: u32,
    },
    /// A declared floor standing **above** what this reading counted — register item 926.
    ///
    /// # ⛔⛔⛔⛔⛔ Why slack is a RED and not a tidy-up
    ///
    /// The four ratchets above hold *may shrink, never grow*, and each was written as a single
    /// comparison: `counted > floor`. That leaves the other direction unwatched, and the gap it
    /// leaves is not cosmetic — **it is exactly as many free unmarked items as the gap is wide.**
    /// Measured on this repository 2026-09-06 12:43 UTC: `@sev-unclassified:` declared 49 against
    /// 47 counted and `@from-unclassified:` declared 386 against 382, so two items could be
    /// registered with no severity and four with no parentage, and the ratchet would have stayed
    /// green through every one of them.
    ///
    /// ⚠⚠ That made two sentences elsewhere FALSE while both read as enforced: this file's own
    /// [`PARENT_DECLARATION`] doc says *"adding one without it raises the count above the floor and
    /// reds"*, and register item 823 says the same. They were true on the day the floors were
    /// written and nothing re-asked. **A ratchet is only as tight as its last honest reading.**
    ///
    /// ⚠ ONE VARIANT FOR ALL FOUR, unlike [`Fault::RatchetGrew`] and its three siblings, and that
    /// is a claim rather than brevity: growth needs per-backlog advice (*mark the item*, *say
    /// whether it is critical*), while slack has exactly one remedy whatever the backlog — bring
    /// the floor down to what is actually there. The token is carried so the message can name it.
    RatchetSlack {
        /// The declaration whose floor is standing too high, e.g. [`SEVERITY_DECLARATION`].
        token: &'static str,
        /// What this reading counted — the items themselves, register item 934.
        counted: Vec<u32>,
        /// What the declaration claims.
        declared: usize,
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
            // 🎯🎯🎯 AND WHICH ITEMS — register item 934. `{counted} unmarked items` sent its
            // reader to a ledger of five hundred blocks with nothing to search for. See [`Ends`]
            // for why the NEW end is the one shown, and why that is likely rather than certain.
            Self::RatchetGrew { counted, declared } => write!(
                f,
                "{} unmarked items, but the ledger declares {declared}: the backlog may \
                 shrink, never grow. Mark the new item, or lower `{DECLARATION}` if you paid some \
                 down. Newest of what was counted (a number is `max+1`, so a NEWLY REGISTERED item \
                 is here; one that entered because a mark CHANGED or was DELETED sits anywhere): {}",
                counted.len(),
                name_some(counted, Ends::Highest),
            ),
            Self::Declaration { found } => write!(
                f,
                "found {found} `{DECLARATION}` lines, need exactly 1 — a ratchet with no floor \
                 holds nothing",
            ),
            // ⚠⚠ THE MESSAGE SAYS WHAT TO DO, and the number to do it with — register item 926's
            // second clause, and the shape item 794's ratchet already uses. A refusal that only
            // names the discrepancy makes the reader re-derive the fix the tool already knows.
            Self::RatchetSlack {
                token,
                counted,
                declared,
            } => write!(
                f,
                "the ledger declares {declared} on `{token}` but this reading counted {}: a \
                 floor ABOVE the count is slack, and {} more item(s) could be registered unmarked \
                 before anything went red. Lower `{token}` to {}. What was counted: {}",
                counted.len(),
                declared - counted.len(),
                counted.len(),
                // ⚠ THE OLD END, unlike the four `…Grew` arms. Nothing was added here — the count
                // is BELOW the floor — so there is no new item to point at, and the end worth
                // handing a reader is the one this ledger's rule 13 says sinks.
                name_some(counted, Ends::Lowest),
            ),
            Self::BacklogOwnerClosed { token, owner } => write!(
                f,
                "`{token}`'s backlog says item {owner} owns bringing it to zero, and {owner} is \
                 not in the open population — so nothing carries that work and no round can be \
                 routed to it. Judge the backlog again: name an open owner, or write why it needs \
                 none — the judgement is the `Reckoning` beside this backlog in `north_star.rs`, \
                 not a line in the ledger",
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
                "{} open items state no `{SEVERITY}`, but the ledger declares {declared}: \
                 this backlog may shrink, never grow. Say whether the new item is critical, or \
                 lower `{SEVERITY_DECLARATION}` if you classified some. Newest of what was counted \
                 (a number is `max+1`, so a NEWLY REGISTERED item is here; one that entered \
                 because a mark CHANGED — an old item just became open — or was DELETED sits \
                 anywhere): {}",
                counted.len(),
                name_some(counted, Ends::Highest),
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
            Self::DeferredByUnreadLink { held, at, named } => write!(
                f,
                "item {held} is held below the depth cap by `{at} {PARENT} {named}`, whose reason \
                 is in neither vocabulary — nobody ever said whether paying {named} MADE it or a \
                 round MET it, so this deferral rests on a sentence no round argued. Say it in the \
                 line (a creation word, or `{PARENT} none`); do not widen the vocabulary",
            ),
            Self::UnrunnableRed { number, line } => write!(
                f,
                "item {number}: `{}` is a red this instrument cannot put to the repository — a \
                 `{RED}` value is the argv `cargo test` is asked with, so every token must be a \
                 flag, a crate name, a module path or a file (letters, digits and `-_:./`). A \
                 claim nothing can check buys an item past the severity gate on a line nobody \
                 verified",
                line.trim()
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
                "{} items state no `{PARENT}`, but the ledger declares {declared}: this \
                 backlog may shrink, never grow. A new item says what found it (`{PARENT} <n>`) or \
                 that nothing did (`{PARENT} none`). Newest of what was counted (a number is \
                 `max+1`, so a NEWLY REGISTERED item is here; one that entered because a mark \
                 CHANGED or was DELETED sits anywhere): {}",
                counted.len(),
                name_some(counted, Ends::Highest),
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
            Self::PaidRatchetGrew { counted, declared } => write!(
                f,
                "{} paid items name no commit, but the ledger declares {declared}: a `paid` \
                 mark that names nothing is a claim about the repository that nothing checked, and \
                 an item marked paid has already left the population. Newest of what was counted \
                 (a number is `max+1`, so a NEWLY REGISTERED item is here; one that entered \
                 because a mark CHANGED — an old item was just marked paid — or was DELETED sits \
                 anywhere): {}",
                counted.len(),
                name_some(counted, Ends::Highest),
            ),
            Self::PaidDeclaration { found } => write!(
                f,
                "found {found} `{PAID_DECLARATION}` lines, need exactly 1 — a ratchet with no \
                 floor holds nothing",
            ),
            Self::UnreadablePaidDeclaration { line } => write!(
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
    /// What [`PAID_DECLARATION`] said, when exactly one line said it. Register item 902.
    pub paid_declared: Option<usize>,
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
        self.backlogs().unclassified.items
    }

    /// 🎯🎯🎯🎯🎯 **THE FOUR BACKLOGS A FLOOR HOLDS, EACH CARRYING ITS ITEMS** — register item
    /// 934, and the ONE place their four predicates are written.
    ///
    /// # ⛔⛔⛔⛔⛔ There had been eight predicates for these four questions
    ///
    /// [`read`] ratcheted each backlog off its own `items.iter().filter(…).count()` while the four
    /// accessors on this type re-derived the same sets for the report — so the number that RED and
    /// the number that PRINTED were two authors agreeing by inspection. They agreed; that is not
    /// the point. Register item 445 is the file this repository keeps re-learning it from, and
    /// register item 926 had already found one of these four floors drifted while the other
    /// direction went unwatched.
    ///
    /// ⚠ Recomputed per call rather than stored: [`read`] must ratchet these before this type
    /// exists, and a cached copy would be a fifth thing that can disagree. The walk is one pass
    /// over section A.
    #[must_use]
    pub fn backlogs(&self) -> Backlogs {
        let of = |keep: fn(&Item) -> bool| -> Vec<u32> {
            self.items
                .iter()
                .filter(|item| keep(item))
                .map(|item| item.number)
                .collect()
        };
        Backlogs {
            unclassified: Backlog {
                label: "unclassified",
                token: DECLARATION,
                items: of(|item| item.tag.is_none()),
                declared: self.declared,
                // Register item 936: the ending counts it, and the third tier hands it over.
                reckoning: Reckoning::Counted,
            },
            // ⚠⚠⚠ OPEN ONLY, and that is the measured choice register item 926 recorded rather
            // than an oversight — counting every item's severity would make a ratchet that grows
            // when a round does its job. The note at [`read`]'s severity ratchet carries the two
            // numbers it was decided on.
            unranked: Backlog {
                label: "unranked",
                token: SEVERITY_DECLARATION,
                items: of(|item| item.tag == Some(Tag::Open) && item.severity.is_none()),
                declared: self.severity_declared,
                // ⚠ SUBSUMED, and that is a proof rather than a preference: this counts OPEN items
                // only, so it is a subset of `population` and an empty population forces it empty.
                // Naming it in the ending as well would count one fact twice.
                reckoning: Reckoning::Exempt(
                    "a subset of the population, so an empty population forces it empty",
                ),
            },
            unrooted: Backlog {
                label: "unrooted",
                token: PARENT_DECLARATION,
                items: of(|item| item.parent.is_none()),
                declared: self.parent_declared,
                // ⚠⚠ ZERO IS NOT THE GOAL HERE, which is why this is an exemption and not a debt.
                // It counts items under EVERY mark, so a paid item still states no parent and
                // paying never moves it — register item 926 measured that asymmetry and chose to
                // leave the population wide. What it is FOR is refusing a NEW item that states no
                // parentage, and that works: measured 2026-09-07, a fresh block with no `@from:`
                // raises the count above the floor and reds.
                reckoning: Reckoning::Exempt(
                    "a ratchet on new items rather than a queue: it counts paid items too, so \
                     paying never lowers it and zero is not what it is for",
                ),
            },
            paid_unnamed: Backlog {
                label: "paid-uncommitted",
                token: PAID_DECLARATION,
                items: of(|item| item.tag == Some(Tag::Paid) && item.commits.is_empty()),
                declared: self.paid_declared,
                // ⛔⛔⛔ AND THIS ONE IS REAL WORK, so it gets an OWNER rather than an excuse.
                // Exempting it would have been the dishonest arm: unlike its two neighbours this
                // number CAN reach zero — twenty-six paid marks, and a sample of five showed three
                // already citing hashes in their own prose (823 cites three, so which one paid it
                // is a judgement, not a move — 823's own warning about item 212). Register item
                // 938 carries it, which is what puts it in `population` where `admits` can reach
                // it: register item 937's finding that opening an item IS the route.
                reckoning: Reckoning::Owned(938),
            },
        }
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
        // ⚠⚠ THE DEPTH IS THE CHAIN'S LENGTH AND NOT A SECOND WALK — register item 920. Two walks
        // over the same marks can disagree, and the one that is PRINTED would not be the one that
        // defers: a reader auditing the chain would then be auditing a different object.
        //
        // ⚠ The cast cannot lose: `chain` inserts each item into `seen` before it is followed, so
        // a chain is never longer than section A, which is a `Vec` on a 64-bit host and nowhere
        // near `u32::MAX` — and `saturating` states that rather than leaving it to be noticed.
        self.chain(number)
            .map(|links| u32::try_from(links.len()).unwrap_or(u32::MAX))
    }

    /// 🎯🎯🎯🎯🎯 **THE `@from:` LINKS THAT CARRY AN ITEM DOWN TO ITS ROOT**, nearest first, each
    /// carrying what its own sentence [`Says`] — register item 920.
    ///
    /// # ⛔⛔⛔⛔⛔ Why a chain nobody can SEE is a chain nobody audits
    ///
    /// [`Reading::deferred`] prints bare numbers, and the depth behind each one was a single
    /// integer with no way back to the lines that made it. Auditing it meant walking the ledger by
    /// hand, which is why — measured on the day this shipped — **it had never been done once**,
    /// while the depths it produced were holding both `@sev: critical` items out of every round's
    /// reach for six rounds running.
    ///
    /// ⇒ The chain is the object the audit is ABOUT, so it is returned rather than summarised. The
    /// per-link verdict comes with it because working rule 13's question — *did paying it MAKE
    /// this, or did a round merely MEET it?* — is asked of a LINK and never of an item.
    ///
    /// [`None`] where the walk cannot finish: an item stating no parentage, a parent section A does
    /// not have, or a chain that returns to itself. **Unknown is never an empty chain**, for the
    /// same reason [`Reading::depth`] never reports it as 0.
    #[must_use]
    pub fn chain(&self, number: u32) -> Option<Vec<Link>> {
        let mut seen = std::collections::BTreeSet::new();
        let mut links = Vec::new();
        let mut at = number;
        loop {
            if !seen.insert(at) {
                return None;
            }
            let item = self.items.iter().find(|item| item.number == at)?;
            match item.parent? {
                Parent::Root => return Some(links),
                Parent::Item(named) => {
                    links.push(Link {
                        number: at,
                        named,
                        says: says(item.reason.as_deref().unwrap_or_default()),
                        // ⚠ The PARENT's mark, read here rather than by the caller — register item
                        // 921. A caller looking it up again would be a second authority on the one
                        // fact the cap now decides on, which is the rule `depth` follows one
                        // method down about the walk itself.
                        parent: self
                            .items
                            .iter()
                            .find(|item| item.number == named)
                            .and_then(|item| item.tag),
                    });
                    at = named;
                }
            }
        }
    }

    /// ⛔⛔⛔⛔⛔ **THE ITEMS CLAIMING TO BE A STANDING RED**, as `(number, argv)` — register item
    /// 843. Open items only, and only those whose claim this instrument could read; an unrunnable
    /// one is [`Fault::UnrunnableRed`] and is not carried here.
    ///
    /// ⚠⚠ **A CLAIM, NOT A FACT.** Printed by the binary even when empty, because *zero claims* and
    /// *zero confirmed* are different sentences and a reader who cannot tell them apart cannot know
    /// whether this machinery looked at anything — register item 924, which is the same hazard one
    /// gate over.
    #[must_use]
    pub fn red_claims(&self) -> Vec<(u32, String)> {
        self.items
            .iter()
            .filter(|item| item.tag == Some(Tag::Open))
            .filter_map(|item| item.red.clone().map(|argv| (item.number, argv)))
            .collect()
    }

    /// 🎯🎯🎯🎯🎯 **THE REDS THE REPOSITORY CONFIRMS**, in numeric order — register item 843, and
    /// the half of [`Reading::red_claims`] the ledger is not allowed to answer.
    ///
    /// Returns the confirmed items and, separately, the claims the suite ran and found GREEN: those
    /// are a ledger that says a red where there is none, which is the mirror of item 902's
    /// wrongly-paid mark and belongs in the same place — a fault about the document, raised by the
    /// caller that could ask.
    ///
    /// # Errors
    ///
    /// A sentence naming why the suite could not be RUN. **A failure to ask is its own fault and
    /// never a verdict about any claim** — [`Reading::paid_unresolved`]'s rule exactly, and the
    /// reason both of these return a [`Result`] rather than folding *could not tell* into *no*.
    ///
    /// ⚠⚠ THE COST, STATED: this runs the named selection. With no claims it runs nothing at all,
    /// which is the ordinary case — measured 2026-09-06, this ledger carries none. With one it
    /// costs whatever that test costs, on every call, which is the price of the answer being the
    /// repository's rather than the document's.
    pub fn standing_reds(&self, suite: &dyn Suite) -> Result<(Vec<u32>, Vec<u32>), String> {
        let mut red = Vec::new();
        let mut green = Vec::new();
        for (number, argv) in self.red_claims() {
            if suite.is_red(&argv)? {
                red.push(number);
            } else {
                green.push(number);
            }
        }
        Ok((red, green))
    }

    /// 🎯🎯🎯🎯🎯 **HOW MANY STILL-OPEN DEBTS THIS ONE SITS UNDER** — register item 921, and the
    /// number [`Reading::deferred`] and [`Reading::takeable`] actually decide on.
    ///
    /// # ⛔⛔⛔⛔⛔ Two questions were one number, and only one of them is the cap's
    ///
    /// [`Reading::depth`] answers *how far down the causal chain does this debt sit* — a FACT about
    /// what created what, permanent once the `@from:` lines are written. The cap asks something
    /// else, and `debt_loop.scxml` says so in its own words: *"HOW FAR A DEBT RUN MAY RE-AIM ITSELF
    /// AWAY FROM THE CHECKPOINT A PERSON GAVE IT"*, where *"Depth 1 is what the run finds WHILE
    /// PAYING that item"*. **A parent that is paid is not being paid by anybody.** 부채의 부채 is a
    /// debt whose parent is a debt; a closed parent is not one.
    ///
    /// # ⛔⛔⛔⛔ What the conflation cost, measured
    ///
    /// [`Reading::deferred`]'s own doc says a deferral is *"registered, and not to be worked until
    /// the budget allows"*. **Nothing ever allowed.** The chain is fixed once written, so a depth
    /// above the cap was permanent — and measured 2026-09-06T11:41:46Z, every one of the five
    /// items the cap was holding sat under ancestors that were ALL `@ns: paid` (839 at 09-02
    /// 23:0x, 840 at 09-03 00:5x, 833 at 09-04 22:xx), while the oldest of them had been standing
    /// since 09-03. So the cap was holding back STANDING debt on the strength of a chain whose
    /// upper links had been closed for days — the exact inversion [`PARENT`]'s doc names, *the
    /// oldest debts sink fastest*, arriving from a second cause after register item 920 fixed the
    /// first.
    ///
    /// # ⚠⚠ Why not simply raise the cap, which is the document's OTHER prescription
    ///
    /// `reaim_max`'s comment says *"If that number climbs while the register does not, this cap has
    /// become a way of losing findings and the right answer is a bigger number, not a quieter
    /// one."* That clause is conditioned on findings being LOST, and they are not: the register grew
    /// by three on the day this shipped while `deferred` fell from six to five. The defect is not
    /// *findings are lost*; it is *a deferral has no end*. The owner's number is untouched at 1.
    ///
    /// # ⛔⛔⛔ AN UNMARKED PARENT HOLDS THE LINK, and that is working rule 6
    ///
    /// A parent stating no mark is *nobody said*, not *closed*. Releasing on it would hand every
    /// one of the ledger's 330 unmarked items the power to free its children silently, which is the
    /// escape hatch that disables its own gate. Unknown counts; only a mark that says the parent is
    /// no longer this loop's debt ([`Tag::Paid`] or [`Tag::Out`]) stops the walk.
    ///
    /// # ⚠⚠⚠ THE RESIDUE, STATED RATHER THAN HIDDEN
    ///
    /// Paying a parent now unlocks its children. That is a path which did not exist before: a run
    /// could in principle pay a shallow parent to reach a deep child. It is not free — paying a
    /// parent is a whole round, and the parent has to be takeable itself — but it is real, and it
    /// is the price of giving a deferral an end. ⚠ What it is NOT is a run boundary resetting the
    /// count, which is the laundering [`Reading::depth`]'s own doc refused and which this leaves
    /// refused: nothing here changes because a run started or ended.
    #[must_use]
    pub fn debts_above(&self, number: u32) -> Option<u32> {
        let links = self.chain(number)?;
        let owed = links
            .iter()
            .take_while(|link| !matches!(link.parent, Some(Tag::Paid | Tag::Out)))
            .count();
        // ⚠ The cast cannot lose, for [`Reading::depth`]'s reason: a chain is never longer than
        // section A.
        Some(u32::try_from(owed).unwrap_or(u32::MAX))
    }

    /// ⛔⛔⛔⛔⛔ **THE ITEMS THE CAP WOULD HAVE HELD AND NO LONGER DOES** — register item 921, and
    /// the answer to *green for WHICH population*.
    ///
    /// # ⚠⚠⚠ Why an empty `deferred` line has to say which kind of empty it is
    ///
    /// After this item [`Reading::deferred`] reads 0 on this ledger. Two completely different
    /// facts produce that: *nothing sits deep in a chain at all*, and *everything that does sits
    /// under parents that are closed*. The second is a claim about five specific items and the
    /// three ancestors that released them, and a reader who cannot tell the two apart cannot audit
    /// this item's own change — which is register item 914's finding, one instrument over.
    ///
    /// ⇒ So the release is PRINTED with the deferral rather than inferred from its absence: these
    /// are the items whose causal [`Reading::depth`] is above `cap` while their
    /// [`Reading::debts_above`] is not.
    #[must_use]
    pub fn released(&self, cap: u32) -> Vec<u32> {
        self.population()
            .into_iter()
            .filter(|number| {
                self.depth(*number).is_some_and(|depth| depth > cap)
                    && self.debts_above(*number).is_some_and(|owed| owed <= cap)
            })
            .collect()
    }

    /// ⛔⛔⛔⛔⛔ **THE DEFERRALS THAT REST ON A LINK NOBODY CLASSIFIED** — register item 920, and
    /// working rule 6 made into a predicate for the one place an unread reason costs something.
    ///
    /// # ⛔⛔⛔ What this is, and what item 896 keeps
    ///
    /// Item 896 owns *every* [`PARENT`] line whose reason is in neither vocabulary: that is a
    /// standing backlog of seventy-odd lines and needs a floor to pay down, exactly like
    /// [`Reading::unrooted`]. **This is the subset where being unread has a PRICE** — a link inside
    /// a chain that holds an item below the cap. Every link of such a chain adds one to the depth
    /// that defers it, so an unread link there is not an annotation nobody got to: it is a debt
    /// held back on a reason no round ever argued.
    ///
    /// ⇒ Which is why this needs no ratchet and is a RED on the spot: the set can be, and after the
    /// round that shipped it was, empty.
    ///
    /// ⚠ Takes the cap rather than reading it, because the cap is the loop document's and this
    /// crate does not open that document — the split register item 833(1) drew and `cap()` in the
    /// binary keeps.
    ///
    /// # ⛔⛔⛔⛔⛔ Returns a [`Screening`] and not a `Vec<Fault>` — register item 924
    ///
    /// This gate's population is `deferred(cap)`'s chains, and register item 921 emptied it. From
    /// that round on the walk put **zero questions** and returned the same empty vector it returns
    /// after putting them all and having every one pass. See [`Screening`] for why the count is
    /// carried out of the walk rather than reconstructed by a caller.
    #[must_use]
    pub fn deferred_unread(&self, cap: u32) -> Screening {
        let mut judged = 0;
        let mut faults = Vec::new();
        for held in self.deferred(cap) {
            // ⚠ Unreachable while `deferred` reports it — an unwalkable chain has no depth and is
            // takeable — and deliberately contributing NOTHING to `judged` if it ever happens: a
            // question that could not be put is not a question answered.
            let Some(links) = self.chain(held) else {
                continue;
            };
            for link in links {
                judged += 1;
                if link.says == Says::Neither {
                    faults.push(Fault::DeferredByUnreadLink {
                        held,
                        at: link.number,
                        named: link.named,
                    });
                }
            }
        }
        Screening {
            label: "deferral links",
            judged,
            faults,
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
            // ⛔⛔⛔ [`Reading::debts_above`] AND NOT [`Reading::depth`] — register item 921. The
            // fact and the budget were one number; the cap is about how much of the chain is still
            // OWED, and a paid parent is not a debt.
            .filter(|number| self.debts_above(*number).is_none_or(|owed| owed <= cap))
            .collect()
    }

    /// The open items the cap holds back — registered, and not to be worked until the budget
    /// allows. **This is the number that says the scheme is doing anything at all.**
    #[must_use]
    pub fn deferred(&self, cap: u32) -> Vec<u32> {
        self.population()
            .into_iter()
            // ⛔⛔⛔ [`Reading::debts_above`], register item 921 — and this is where the sentence
            // one line up stopped being false. *Until the budget allows* had no allowing in it: a
            // depth taken over a FIXED chain never fell, so every deferral was permanent.
            .filter(|number| self.debts_above(*number).is_some_and(|owed| owed > cap))
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
    /// # ⛔⛔⛔⛔⛔ AND A STANDING RED IS IN THE SET WHATEVER ITS SEVERITY — register item 843
    ///
    /// The composition above is a gate on SEVERITY, and it made the loop unable to reach its own
    /// reds: item 837 was `@sev: ordinary`, **stood red for four days**, and every proposal naming
    /// it was counted and not taken for as long as any critical item stood. Two rules were in
    /// conflict — *take from critical* (a machine since register item 839) and *a red is paid in
    /// its own round* (prose in `CLAUDE.md`, measured by nobody) — and the enforced one won, which
    /// is register item 839's own finding arriving one layer up.
    ///
    /// ⚠⚠ `reds` IS A PARAMETER AND NOT A LOOKUP, because this crate may not decide it: the answer
    /// is the repository's ([`Suite`]), and a caller that has not asked must pass `&[]` VISIBLY
    /// rather than get the old behaviour by writing nothing.
    ///
    /// ⚠ They still have to be [`Reading::takeable`]. Item 843's words are *등급과 무관하게* —
    /// severity, not the depth cap, which is register item 833's separate rule and a different
    /// question. THE RESIDUE, STATED: a red held back by the cap is still unreachable. Since
    /// register item 921 the cap holds only chains whose ancestors are still owed, so this is a
    /// narrow case rather than the standing one it was.
    ///
    /// # ⛔⛔⛔⛔⛔ AND WHEN NOTHING MARKED IS TAKEABLE, THE UNCLASSIFIED ARE — register item 936
    ///
    /// Working rule 11 says *while anything is critical take from those, otherwise work the
    /// population*. It says nothing about the population being EMPTY, and this returned an empty
    /// set there — *nothing to do*, which is the reassuring reading of an unstated case that this
    /// workspace's rule 6 exists to refuse. **Measured on a two-item ledger** (one `paid`, one
    /// never marked): `population 0`, `critical 0`, every floor matching its count, `rc=0`,
    /// **stderr empty** — and `--admits … "Take item 899"` answered `NO` with *"What a round may
    /// take:"* followed by nothing. So a run standing there could neither finish honestly nor move:
    /// the register said a block was an unpaid debt and simultaneously that no round might take it.
    ///
    /// ⚠⚠ **THIS IS RULE 6 BEING ENFORCED, NOT A NEW POLICY OUT-VOTING THE DOCUMENT.** Register
    /// item 833(1) is the standing warning about this direction — an instrument holding a value the
    /// loop document no longer declares — and it does not reach here, because the document declares
    /// nothing for this case. This module's own doc already calls an unmarked item *"a debt with a
    /// ratchet on it rather than a silence"*; the arm below is that sentence being true of the set
    /// a round may take, and it can only ever ADD work.
    ///
    /// ⚠ **THIRD, never blended.** It fires only where the two above are empty, so the 330 standing
    /// on the real ledger cannot drown the 82 marked debts — which would be the same *widen it
    /// until it is green* this repository keeps paying for, wearing the opposite sign.
    ///
    /// ⚠ The cap does not reach these and must not: an unclassified block states no parentage, so
    /// there is no chain to be deep in. What it needs first is a mark, and that is the work.
    #[must_use]
    pub fn admits(&self, cap: u32, reds: &[u32]) -> Vec<u32> {
        let takeable = self.takeable(cap);
        let mut first: Vec<u32> = self
            .critical()
            .into_iter()
            .chain(reds.iter().copied())
            .filter(|number| takeable.contains(number))
            .collect();
        first.sort_unstable();
        first.dedup();
        if !first.is_empty() {
            return first;
        }
        if !takeable.is_empty() {
            return takeable;
        }
        self.backlogs().unclassified.items
    }

    /// ⛔⛔⛔⛔⛔ **THE BACKLOGS WHOSE DECLARED OWNER IS NOT OPEN IN THIS LEDGER** — register item
    /// 937, and asked HERE rather than inside [`read`] for a measured reason.
    ///
    /// # ⛔⛔⛔⛔⛔ What putting it in `read` did, measured
    ///
    /// [`Reckoning::Owned`] carries an item number, which is a claim about **one** ledger. This
    /// crate reads *a* ledger: every fixture in this file is a different one. Checking the claim
    /// inside `read` turned **eleven** tests red at once — not because any of them was wrong, but
    /// because none of their miniature ledgers contains the item this repository's register filed
    /// the work under. A gate that reds on every document except one is not a gate.
    ///
    /// ⚠⚠ So it joins [`Reading::paid_unresolved`] and [`Reading::standing_reds`] as a question
    /// **only the caller should put** — the caller being the one thing that knows which ledger is
    /// the one the claim is about. The binary folds these into its exit code beside the others.
    ///
    /// ⚠ **THE RESIDUE, STATED RATHER THAN HIDDEN**: the number is still spelled in this crate,
    /// and this file's own [`DECLARATION`] doc says the opposite — *"the number lives in the LEDGER
    /// rather than in this crate for the reason a written-down expectation always rots
    /// invisibly"*. Register item 833(1) is the same disagreement one policy over. Making the
    /// LEDGER declare its own backlog owners is register item 939's.
    #[must_use]
    pub fn backlog_owners_gone(&self) -> Vec<Fault> {
        let population = self.population();
        let backlogs = self.backlogs();
        let mut gone = Vec::new();
        for backlog in backlogs.each() {
            if let Reckoning::Owned(owner) = backlog.reckoning
                && !population.contains(&owner)
            {
                gone.push(Fault::BacklogOwnerClosed {
                    token: backlog.token,
                    owner,
                });
            }
        }
        gone
    }

    /// 🎯🎯🎯🎯🎯 **WHETHER THE NORTH STAR IS REACHED, AS A READING RATHER THAN A JUDGEMENT** —
    /// register item 936. See [`Ending`] for the population and how it was measured.
    #[must_use]
    pub fn ending(&self) -> Ending {
        Ending {
            open: self.population(),
            unclassified: self.backlogs().unclassified.items,
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
        self.backlogs().unrooted.items
    }

    /// The OPEN items that state no severity — the backlog [`Fault::SeverityRatchetGrew`] holds.
    ///
    /// ⚠⚠ These are not "ordinary". **Unclassified is not a pass** — the same rule that makes an
    /// unmarked item a debt rather than a "no" (working rule 6). A round that wants one of these
    /// worked says so by classifying it.
    #[must_use]
    pub fn severity_unclassified(&self) -> Vec<u32> {
        self.backlogs().unranked.items
    }

    /// ⛔⛔⛔⛔⛔ **THE PAID ITEMS WHOSE MARK NAMES NO COMMIT** — the backlog
    /// [`Fault::PaidRatchetGrew`] holds, register item 902.
    ///
    /// ⚠⚠ **PAID ONLY, and that is the whole population question here.** An OPEN item has nothing
    /// to have committed and an `out` one was never this loop's; demanding an id of either would be
    /// a ratchet that grows when a round does its job, which is the trap [`SEVERITY_DECLARATION`]
    /// records avoiding.
    #[must_use]
    pub fn paid_unnamed(&self) -> Vec<u32> {
        self.backlogs().paid_unnamed.items
    }

    /// ⛔⛔⛔⛔⛔ **AND THE PAID ITEMS WHOSE NAMED COMMIT THIS TREE CANNOT RESOLVE** — the other
    /// half of register item 902, and the half no document can answer on its own.
    ///
    /// # ⛔⛔ Why this is NOT a ratchet, when its neighbour is
    ///
    /// [`paid_unnamed`](Self::paid_unnamed) counts a convention that is unevenly kept, so it has a
    /// backlog and a floor. An id that is WRITTEN and does not resolve has no backlog: it is either
    /// a typo or a claim about a commit that was never made, and there is no round in which it is
    /// acceptable. So every one of these is returned, and the caller reds on any.
    ///
    /// # ⚠⚠⚠ A FAILURE TO ASK IS NOT AN ANSWER
    ///
    /// If `commits` cannot be consulted at all — run outside a repository, `git` missing — this
    /// returns that error rather than declaring every id unresolvable. Forty-odd item faults from
    /// one broken environment would be a RED that teaches the reader to ignore this line, which is
    /// how a gate dies. See [`Commits::resolves`].
    ///
    /// # Errors
    ///
    /// Whatever `commits` said when it could not answer at all.
    pub fn paid_unresolved(&self, commits: &dyn Commits) -> Result<Vec<(u32, String)>, String> {
        let mut found = Vec::new();
        for item in self.items.iter().filter(|it| it.tag == Some(Tag::Paid)) {
            for id in &item.commits {
                if !commits.resolves(id)? {
                    found.push((item.number, id.clone()));
                }
            }
        }
        Ok(found)
    }

    /// Whether this reading is clean.
    #[must_use]
    pub fn is_green(&self) -> bool {
        self.faults.is_empty()
    }
}

/// ⛔⛔⛔⛔⛔ **WHO ANSWERS WHETHER A COMMIT ID NAMES A COMMIT** — register item 902, injected
/// rather than reached for, so [`read`] stays a function of its text alone.
///
/// ⚠ The ledger is not in the repository it talks about, and this crate must not decide where that
/// repository is. The caller that knows both is the one that supplies this.
pub trait Commits {
    /// Whether `id` names a commit.
    ///
    /// # Errors
    ///
    /// A sentence naming why the question could not be PUT — never why one id failed. An id that
    /// simply is not there is `Ok(false)`, and the difference is the whole of why this returns a
    /// [`Result`]: see [`Reading::paid_unresolved`].
    fn resolves(&self, id: &str) -> Result<bool, String>;
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

/// ⛔⛔⛔⛔⛔ **THE COMMIT IDS A MARK LINE NAMES** — register item 902, and the whole of what this
/// crate can read about whether a `paid` claim reached the repository.
///
/// A candidate is a BACKTICKED run of lowercase hex whose length is [`COMMIT_ID`]. Anything else on
/// the line — dates, item numbers, the sentence saying what was done — is not a commit id and is
/// not guessed at.
///
/// # ⚠⚠ READ FROM THE MARK LINE ALONE, deliberately, and that is the NARROW direction
///
/// A mark whose sentence wraps onto the next line and puts its hash there is counted as naming
/// nothing. That is a false RED for that item and it is the safe way round: the fix is to move the
/// hash onto the mark line, which is the convention this gate exists to hold. Reading the whole
/// block would be the WIDE direction — every paid item quotes commits in its prose, so almost
/// nothing would ever be counted and the ratchet would hold air.
fn named_commits(value: &str) -> Vec<String> {
    value
        .split('`')
        .skip(1)
        .step_by(2)
        .filter(|token| {
            // ⚠ LOWERCASE ONLY, which is what `git` writes and therefore what a line copied from a
            // real commit carries. `is_ascii_hexdigit` would also admit `DEADBEE`, and a prose
            // token in capitals is far likelier to be an acronym than an id somebody typed by hand.
            COMMIT_ID.contains(&token.len())
                && token.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
        })
        .map(ToString::to_string)
        .collect()
}

/// The value of a [`SEVERITY`] line, by the same whole-line rule [`mark_value`] holds.
fn severity_value(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(SEVERITY)
}

/// The value of a [`PARENT`] line, by the same whole-line rule [`mark_value`] holds.
fn parent_value(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(PARENT)
}

/// The value of a [`RED`] line, by the same whole-line rule [`mark_value`] holds.
fn red_value(line: &str) -> Option<&str> {
    line.trim_start().strip_prefix(RED)
}

/// ⛔⛔⛔⛔⛔ **WHETHER A CLAIMED RED IS ACTUALLY RED**, asked of the repository — register item 843,
/// and [`Commits`]' shape one fact over.
///
/// # ⚠⚠ Why the ledger may not answer this by itself
///
/// Item 843's own done-when says it in as many words: *「서 있는 red」가 admits 의 모집단에
/// 들어간다 — 등급과 무관하게, 그리고 그 사실이 «원장이 아니라 저장소»에 물어서 확인된다(빨간 것은
/// 스위트가 안다)*. A ledger line saying *this is red* is a claim about a tree, and item 902 already
/// paid for the difference between a correctly-formed mark and a TRUE one: item 866(2) sat marked
/// paid over 720 lines that were never committed. A red mark nobody checks is the same defect
/// pointing the other way — it would let any item promote itself past the severity gate by typing
/// one line.
pub trait Suite {
    /// Whether the thing `names` selects is failing NOW.
    ///
    /// # Errors
    ///
    /// A sentence naming why the question could not be PUT — never why one claim failed. A claim
    /// the suite ran and found GREEN is `Ok(false)`, which is a fault about the ledger; a suite
    /// that could not be run at all says nothing about any claim, and the difference is the whole
    /// of why this returns a [`Result`]. [`Commits::resolves`]' rule exactly.
    fn is_red(&self, names: &str) -> Result<bool, String>;
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

/// ⛔⛔⛔⛔⛔ **HOW MANY BYTES OF ITEM NUMBERS ONE LINE SPENDS BEFORE SUMMARISING THE REST** —
/// register item 934, and a BYTE budget rather than a count of items on purpose.
///
/// # ⛔⛔⛔ Why bytes and not `take(N)`
///
/// This ledger's numbers are three digits today and cross into four the moment it registers item
/// 1000. A budget of *items* would silently lengthen every line on that day; a budget of bytes
/// names fewer items instead, which is the thing actually being protected.
///
/// # 📊 The measurement this number came from — 2026-09-07
///
/// ```text
/// north-star <ledger> | awk '{print length($0)}'
/// ```
///
/// Every line of the report is 12–341 bytes: `population` is the longest at **341** (82 items) and
/// every other line is at most **93**. Naming all 330 `unclassified` items would cost ≈ 1,320
/// bytes and all 382 `unrooted` ≈ 1,528 — four to five times the longest line this report has ever
/// printed, on lines that are annotations rather than the set a round acts on.
///
/// ⇒ **120 bytes**, which names about thirty three-digit items and lands the whole line near 180 —
/// under the 341 the `population` line already spends, so no backlog line becomes this report's
/// longest, and an order of magnitude more items than the `deferred`/`released` sets a round reads
/// today.
///
/// ⚠⚠ **AT LEAST ONE ITEM IS NAMED WHATEVER THE BUDGET**, see [`name_some`]. A line that named a
/// count and no item is the whole of register item 934, and a budget that could bring that back by
/// being set low would be the defect returning through the knob meant to bound it.
///
/// ⚠ **PUBLIC alongside [`name_some`] since register item 936**: a caller that can render a
/// bounded list must be able to read what the bound IS, or the number becomes the same unmeasured
/// thing this constant was written to stop being.
pub const NAMED_IN_A_LINE: usize = 120;

/// Which end of a set [`name_some`] shows when it cannot show all of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ends {
    /// The lowest numbers — the OLDEST items. What a report line shows: this ledger's rule 13 is
    /// that old debt sinks, so the end a reader must be handed is the end that sinks.
    Lowest,
    /// The highest numbers — the NEWEST items. What a grown ratchet shows: a number is assigned
    /// `max+1` at registration, so a backlog that grew by an item being ADDED grew at its top.
    ///
    /// ⚠ It is the likely end and not the certain one: a backlog also grows when an existing
    /// item's mark is deleted, and that item sits wherever it sits. The fault says so rather than
    /// letting the reader read a heuristic as a location.
    Highest,
}

/// 🎯🎯🎯🎯🎯 **NAME THE ITEMS, NOT ONLY HOW MANY** — register item 934, in the one place every
/// line that carries a set is rendered.
///
/// Empty for an empty set. Every number when they fit inside [`NAMED_IN_A_LINE`]; otherwise as
/// many as fit from the `from` end, then `… and N more, all higher`/`, all lower` — **said**, so a
/// reader can tell a short set from a cut one, which is the rule `pty-demand`'s own head already
/// keeps.
///
/// ⚠⚠ Ascending in both directions. The ORDER a set is read in is not the END it is cut at, and
/// printing the high end backwards would make two lines of one report disagree about what a list
/// looks like.
///
/// ⚠ **PUBLIC since register item 936**, because the admissible set is now one of the lists that
/// can run to three hundred: `--admits`'s refusal prints *"What a round may take"* whole, and once
/// [`Reading::admits`] falls through to the unclassified that line is the 1,300-byte one register
/// item 934 was opened by. One bound, one place.
#[must_use]
pub fn name_some(items: &[u32], from: Ends) -> String {
    if items.is_empty() {
        return String::new();
    }
    // How many fit, counted the way they will be printed: each number plus the space before it,
    // except the first. ⚠ At least one, whatever the budget — see [`NAMED_IN_A_LINE`].
    let mut spent = 0;
    let mut fits = 0;
    for number in items {
        let width = number.to_string().len() + usize::from(fits > 0);
        if fits > 0 && spent + width > NAMED_IN_A_LINE {
            break;
        }
        spent += width;
        fits += 1;
    }
    let (shown, hidden) = match from {
        Ends::Lowest => (&items[..fits], "higher"),
        Ends::Highest => (&items[items.len() - fits..], "lower"),
    };
    let spelled: Vec<String> = shown.iter().map(ToString::to_string).collect();
    let line = spelled.join(" ");
    match items.len() - fits {
        0 => line,
        rest => format!("{line} … and {rest} more, all {hidden}"),
    }
}

/// ⛔⛔⛔⛔⛔ **ONE OF THE FOUR BACKLOGS A FLOOR HOLDS, CARRYING THE ITEMS AND NOT ONLY THE COUNT**
/// — register item 934.
///
/// # ⛔⛔⛔ What was wrong with a count
///
/// The four numbers were printed as `.len()` over a `Vec` that was then dropped, and the ratchet
/// that judges them took a `usize`. So the largest population in this ledger — 330 unclassified items,
/// four times the standing `population` — could be *seen* and not *asked about*: no round could
/// name one of them, and a ratchet that moved said a number had moved and never which item.
///
/// ⚠ This type exists so the label, the floor's token, the items and the floor travel together.
/// The binary printing `unclassified` beside the severity backlog's items would have been a
/// one-word edit while the four were four unrelated locals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlog {
    /// The word this backlog's report line opens with, e.g. `unclassified`.
    pub label: &'static str,
    /// The declaration whose floor holds it, e.g. [`DECLARATION`].
    pub token: &'static str,
    /// Every item in it, ascending.
    pub items: Vec<u32>,
    /// The floor the ledger declares, or [`None`] when it declares none readably.
    pub declared: Option<usize>,
    /// ⛔ **How this backlog ever reaches zero — register item 937.** See [`Reckoning`].
    pub reckoning: Reckoning,
}

/// ⛔⛔⛔⛔⛔ **HOW ONE RATCHETED BACKLOG EVER REACHES ZERO** — register item 937, and the
/// generalisation register item 936 turned out to be one case of.
///
/// # ⛔⛔⛔ What 936 fixed for one backlog and left standing for two
///
/// Register item 936 gave `unclassified` two things: the ending counts it, and
/// [`Reading::admits`] hands it over. Measured 2026-09-07, **neither of the other two had
/// anything**: the ending deliberately excludes them (working rule 5 — neither falls by paying),
/// no open register item named them (902 and 926, which opened and studied them, are both `paid`),
/// and no sentence anywhere said why they needed neither. So a later round would have re-discovered
/// them as a fresh defect — register item 933's shape, arriving a third time.
///
/// ⚠⚠ **AN UNSTATED DISPOSITION IS A RED, NOT A DEFAULT.** There is no `Unknown` arm and no
/// [`Default`]: a fifth backlog cannot be added without its author writing one of these three, and
/// the three are checked rather than taken at their word — see [`Reading::backlog_owners_gone`]
/// for the arm a document can take away after the fact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reckoning {
    /// **The ending counts it**, so the north star cannot be reached while it stands. See
    /// [`Ending`], whose own doc carries why only two of the five qualify.
    Counted,
    /// **An open register item owns the work**, so `population` carries it and
    /// [`Reading::admits`] can hand it over like any other debt. The number is checked: an owner
    /// that has been paid is [`Fault::BacklogOwnerClosed`], which is the disposition asking to be
    /// re-stated rather than quietly expiring.
    Owned(u32),
    /// **It needs neither, and this is why.** The sentence is the whole of this arm's honesty, and
    /// the COUNT of backlogs standing on it is asserted by a gate — an exemption nobody counts is
    /// the escape hatch this workspace's rule 6 exists to refuse.
    Exempt(&'static str),
}

impl fmt::Display for Backlog {
    /// The report line: `unclassified 330 (declared 330): 80 470 … and 300 more, all higher`.
    ///
    /// ⚠ Shaped after the `population` line — count, then the items after a colon — because a
    /// reader who has learnt one line of this report has learnt this one.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} (declared {}): {}",
            self.label,
            self.items.len(),
            self.declared
                .map_or_else(|| "none".to_string(), |n| n.to_string()),
            name_some(&self.items, Ends::Lowest),
        )
    }
}

/// ⛔⛔⛔⛔⛔ **ALL FOUR OF THEM, NAMED RATHER THAN INDEXED** — register item 934.
///
/// ⚠⚠ Built in ONE place ([`Reading::backlogs`]) and handed to both readers: the ratchets that
/// judge these sets and the lines that print them. They had been two separate `filter` chains per
/// backlog — eight predicates for four questions — which is register item 445's two authors sitting
/// inside the instrument that counts them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backlogs {
    /// Items stating no [`TAG`] at all.
    pub unclassified: Backlog,
    /// OPEN items stating no [`SEVERITY`].
    pub unranked: Backlog,
    /// Items stating no [`PARENT`].
    pub unrooted: Backlog,
    /// PAID items naming no commit — register item 902.
    pub paid_unnamed: Backlog,
}

impl Backlogs {
    /// 🎯🎯🎯🎯🎯 **ALL FOUR, ENUMERATED IN ONE PLACE** — register item 937.
    ///
    /// # ⛔⛔⛔⛔⛔ Why this is destructured rather than four field reads
    ///
    /// [`Reckoning`] forces a fifth backlog's author to JUDGE it — the struct literal will not
    /// compile without the field. It does not force anyone to CHECK that judgement, and the check
    /// lived in a hand-listed array of four: a fifth backlog declaring
    /// [`Reckoning::Owned`] would have been judged and then never verified, which needs no
    /// deliberate evasion, only forgetting.
    ///
    /// Destructuring `Self` here makes a fifth field a compile error in **this one place**, and
    /// everything that walks the backlogs walks it through here. Measured before writing it: this
    /// struct has four fields and three destructurings, none of them using `..`, so the judgement
    /// was already forced everywhere — only its checking was not.
    #[must_use]
    pub fn each(&self) -> [&Backlog; 4] {
        let Self {
            unclassified,
            unranked,
            unrooted,
            paid_unnamed,
        } = self;
        [unclassified, unranked, unrooted, paid_unnamed]
    }
}

/// ⛔⛔⛔⛔⛔ **WHAT A GATE PUT ITS QUESTION TO, BESIDE WHAT IT FOUND** — register item 924, and
/// the answer to *green for WHICH population* at the one gate here that had no answer.
///
/// # ⛔⛔⛔ A gate whose population emptied, reading exactly like a gate that passed
///
/// [`Reading::deferred_unread`]'s population is not this ledger: it is the links of the chains that
/// defer something, so it is `deferred(cap)`'s and empties whenever the deferrals do. Register item
/// 921 paid them down to none, and **measured 2026-09-06 the gate then examined zero links against
/// the real ledger** while the run exited 0 exactly as it had when it examined some. Nothing in the
/// report moved, because an empty `Vec<Fault>` is printed as ABSENCE — and absence is the one shape
/// that cannot separate *asked, and found nothing wrong* from *never asked*.
///
/// ⇒ So the count travels beside the faults and is printed on BOTH verdicts. That is what
/// `reds N claimed, M standing` already does two gates over, and what register items 914 and 918
/// found in two other instruments: **a green gate has to say which population it is green for.**
///
/// # ⚠⚠⚠ Why `judged` is counted by the walk and never recomputed beside it
///
/// A second `filter().count()` over the same set is register item 445's two authors for one
/// question, and register item 934 is this file's own case of what that costs: the printed number
/// and the judged number drift, and the report goes on reading fine. So
/// [`Reading::deferred_unread`] builds this in ONE pass — every question it puts increments
/// `judged`, and the ones that fail land in `faults`. `faults.len() <= judged` is therefore a fact
/// about the walk rather than an invariant somebody has to keep.
///
/// ⚠ `judged` counts QUESTIONS PUT, not distinct links: a link shared by two deferred chains is
/// asked about once per chain, because that is what the walk does, and a count describing a
/// different walk would be the drift this type exists to refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screening {
    /// The words this screening's report line opens with, e.g. `deferral links`.
    pub label: &'static str,
    /// How many questions this run actually put. **Zero is the reading register item 924 was
    /// opened by**, and it is printed rather than inferred from an empty `faults`.
    pub judged: usize,
    /// The questions that failed, in the order the walk met them.
    pub faults: Vec<Fault>,
}

impl fmt::Display for Screening {
    /// The report line: `deferral links 0 judged, 0 unread`.
    ///
    /// ⚠ No list after a colon, unlike [`Backlog`] and the `population` line: these are FAULTS, and
    /// the report already prints every one of them as its own sentence. What this line adds is the
    /// denominator those sentences are silent about.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {} judged, {} unread",
            self.label,
            self.judged,
            self.faults.len(),
        )
    }
}

/// 🎯🎯🎯🎯🎯 **WHETHER THIS LEDGER'S NORTH STAR IS REACHED** — register item 936, and the
/// sentence that had never been anything but prose.
///
/// # ⛔⛔⛔⛔⛔ The ending was a WORD THE AGENT SAYS, judged against a sentence in a runtime string
///
/// Measured 2026-09-07: `ai_loop.scxml` carries `north_star_marker = 'NORTH STAR REACHED'` and
/// `north_star` itself is assigned from `_event.data.north_star` — a string handed to the run. The
/// only thing that ends a run is the agent emitting that marker (`ai_loop.rs`, *ARM 1: THE AGENT
/// SAID THE NORTH STAR WAS REACHED*). **Nothing anywhere evaluates the condition.** So the most
/// consequential predicate in the loop was the one thing this workspace's rule 10 says goes
/// unmeasured, and register item 936's own guess — *"`is_green` already sees the four through its
/// faults"* — is FALSE: [`Reading::is_green`] is `faults.is_empty()`, and on the two-item ledger
/// above every floor matched its count, so it was green with a debt standing.
///
/// # ⛔⛔⛔⛔⛔ Why these TWO and not the four — working rule 5, asked of each
///
/// *Is there a path by which this number becomes 0?* A population carrying a role that cannot
/// reach zero is one that makes the ending unreachable rather than honest.
///
/// | number | path to 0 | in the ending |
/// |---|---|---|
/// | `population` (`@ns: open`) | paying them | ✅ |
/// | `unclassified` (no [`TAG`]) | reading the block and marking it — and it is what makes the population TOTAL | ✅ |
/// | `unranked` (OPEN with no [`SEVERITY`]) | subsumed: it is a subset of `population`, so `population == 0` forces it to 0 | ⛔ counted twice otherwise |
/// | `unrooted` (no [`PARENT`], **any** mark) | only by annotating every historical block; a PAID item still has none, so paying never moves it | ⛔ would make the ending unreachable |
/// | `paid-uncommitted` (PAID naming no commit) | adding ids to old marks — real work, but it says nothing about whether anything is OWED | ⛔ not the question |
///
/// ⚠⚠ **AND IT NAMES ITS POPULATION ON EVERY LINE, REACHED OR NOT** — register item 914's finding:
/// a gate that says *green* without saying *green for what* cannot be audited. Both counts are
/// printed either way, so a reader never has to know which of the five this line was about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ending {
    /// The items still marked [`Tag::Open`].
    pub open: Vec<u32>,
    /// The items carrying no [`TAG`] at all — unread rather than answered.
    pub unclassified: Vec<u32>,
}

impl Ending {
    /// Whether the north star is reached: nothing open **and** nothing unclassified.
    ///
    /// ⚠ `unclassified` is not a second opinion about the same set. An unmarked block has never
    /// been asked whether it is this loop's, so a `population` of zero beside it means *nothing
    /// MARKED is owed* — which is the claim register item 823 spent itself replacing, arriving one
    /// level up.
    #[must_use]
    pub fn reached(&self) -> bool {
        self.open.is_empty() && self.unclassified.is_empty()
    }
}

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "north star: {} — open {}, unclassified {}",
            if self.reached() {
                "REACHED"
            } else {
                "NOT REACHED"
            },
            self.open.len(),
            self.unclassified.len(),
        )
    }
}

/// 🎯🎯🎯🎯🎯 **WHAT TO DO WITH AN UNREAD BLOCK, SAID WHERE ONE IS HANDED OVER** — register item
/// 936(1), whose done-when asks that the work say *what you read and what you write so the count
/// falls by one*.
///
/// # ⛔⛔⛔ Why this is a sentence the INSTRUMENT says
///
/// [`Reading::admits`]'s third tier makes an unread block takeable, but the tier only fires once
/// the population empties — on this ledger roughly a hundred and sixty rounds from the day it was
/// written. A procedure recorded in a register entry that far ahead is prose nobody re-reads,
/// which is this workspace's rule 10 arriving at the worst possible distance. So the answer rides
/// on the reply that offers the block.
///
/// # 📊 Every clause measured, 2026-09-07 — and what each omission costs
///
/// Driven against the real ledger, one unread block marked and the reading re-run:
///
/// | what was written | result |
/// |---|---|
/// | `@ns: out` **and** the floor lowered by one | `rc=0`, stderr empty |
/// | the mark alone, floor left standing | `rc=1` — [`Fault::RatchetSlack`], which names the number to write |
/// | `@ns: open` with no [`SEVERITY`] | `rc=1` — [`Fault::SeverityRatchetGrew`] |
/// | `@ns: paid` with no commit id on the mark line | `rc=1` — [`Fault::PaidRatchetGrew`] |
///
/// ⚠⚠ **NOT A WORD OF THIS IS SPELLED IN CAPITALS, AND THAT IS LOAD-BEARING.** This is the only
/// free prose that reaches an `--admits` reply — everything else interpolated there is a fixed
/// string, an item number, or [`Reaim::spelled`]'s two words — and `sprag_plugin::judge` finds a
/// verdict by taking the first MARKED word, where the marks are *opens the reply* **or** *all
/// capitals*. A capitalised yes-or-no word here could be read as the verdict itself; register item
/// 743 records what one misread verdict cost.
pub const CLASSIFY_REMEDY: &str = "it carries no `@ns:` mark — read the block, write \
     `@ns: open|paid|out`, and lower `@ns-unclassified:` by one. An `open` verdict needs `@sev:` \
     too and a `paid` one needs a commit id on the mark line, or that ratchet reds instead.";

/// Judge one backlog against the floor its ledger declares — **in both directions**.
///
/// # ⛔⛔⛔⛔⛔ Why this is a function, and why all four go through it
///
/// Register item 926. The four ratchets were four copies of `if counted > floor { … }`, and the
/// missing half — a floor standing *above* the count — was missing from all four at once, because
/// a shape written four times is corrected in one place and left in three. That is register item
/// 213 exactly, and it is why *all four are held* is arranged here as a property of the code rather
/// than as a list somebody keeps in step: a fifth ratchet added tomorrow gets both directions by
/// calling this, and cannot get only one by forgetting.
///
/// ⚠ `declared` of [`None`] returns without a word, and that is not a hole: [`declared_floor`] has
/// already faulted for the missing or unreadable line, and a second complaint about the same
/// absence would be noise. A ratchet with no floor is refused there, not weakened here.
/// ⚠⚠ **IT TAKES THE BACKLOG AND NOT ITS SIZE** — register item 934. A fault reading *331, but the
/// ledger declares 330* hands its reader nothing to open; the set it counted is the only thing
/// this can hand over, because a floor is a scalar and no reading can diff a set against a number.
/// That limit is stated in the faults themselves rather than papered over.
fn ratchet(
    backlog: &Backlog,
    grew: impl FnOnce(Vec<u32>, usize) -> Fault,
    faults: &mut Vec<Fault>,
) {
    let Some(floor) = backlog.declared else {
        return;
    };
    let counted = backlog.items.len();
    if counted > floor {
        faults.push(grew(backlog.items.clone(), floor));
    } else if counted < floor {
        faults.push(Fault::RatchetSlack {
            token: backlog.token,
            counted: backlog.items.clone(),
            declared: floor,
        });
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
    // ⛔⛔⛔⛔⛔ AND THE FLOOR FOR PAID MARKS THAT NAME NO COMMIT — register item 902, read the
    // same way its three neighbours are and for their reason. See [`PAID_DECLARATION`].
    let paid_declared = declared_floor(
        text,
        PAID_DECLARATION,
        &mut faults,
        |found| Fault::PaidDeclaration { found },
        |line| Fault::UnreadablePaidDeclaration { line },
    );

    let mut items: Vec<Item> = Vec::new();
    for (number, bodies) in &blocks {
        let mut tag = None;
        let mut severity = None;
        let mut parent = None;
        let mut reason = None;
        let mut red = None;
        // ⛔⛔⛔⛔⛔ THE COMMIT IDS THE MARK LINE NAMES — register item 902, collected in the same
        // walk and settled by the same topmost-block rule the mark itself is, because an id read
        // off a block whose mark lost the tie would be evidence for a claim this item is not
        // making. See [`named_commits`].
        let mut commits: Vec<String> = Vec::new();
        for body in bodies {
            let mut in_block: Vec<Tag> = Vec::new();
            let mut named: Vec<String> = Vec::new();
            let mut severities: Vec<Severity> = Vec::new();
            let mut parents: Vec<Parent> = Vec::new();
            let mut reasons: Vec<String> = Vec::new();
            let mut reds: Vec<String> = Vec::new();
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
                            // ⛔ AND THE SENTENCE IS KEPT, not only asked one question and thrown
                            // away — register item 920. `met_while` above consumed it and dropped
                            // it, so nothing downstream could say WHY a chain holds an item back.
                            reasons.push(value.to_string());
                        }
                        None => faults.push(Fault::UnknownParent {
                            number: Some(*number),
                            line: (*line).to_string(),
                        }),
                    }
                }
                // ⛔⛔⛔⛔⛔ AND WHAT IT CLAIMS IS RED — register item 843. An unrunnable value is a
                // FAULT and never a silence: a claim this instrument cannot put to the repository
                // would otherwise sit in the ledger looking like a checked one, which is exactly
                // the shape item 902 measured on a `paid` mark that named no commit.
                if let Some(value) = red_value(line) {
                    let named = value.trim();
                    if named.is_empty() || !named.split_whitespace().all(safe_argument) {
                        faults.push(Fault::UnrunnableRed {
                            number: *number,
                            line: (*line).to_string(),
                        });
                    } else {
                        reds.push(named.to_string());
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
                named.extend(named_commits(value));
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
            // ⚠ Topmost block wins, exactly as the three marks above it do — see `Item`'s own doc
            // about a number owning several blocks. Register item 843.
            if red.is_none() {
                red = reds.first().cloned();
            }
            if parent.is_none() {
                parent = parents.first().copied();
                // ⚠ In the SAME breath as the parent it explains, for the reason item 902 gives
                // about commit ids: a reason gathered a step later could come off a block whose
                // parentage lost the tie.
                reason = reasons.first().cloned();
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
                // ⚠ AND ITS IDS COME WITH IT, in the same breath and never a step later: a mark
                // settled by one block and ids gathered from all of them would let a superseded
                // block vouch for the block that beat it. Register item 902.
                commits = std::mem::take(&mut named);
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
            reason,
            red,
            commits,
            names_the_loop,
            reads_as_closed,
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
        paid_declared,
        faults,
    };

    // ⛔⛔⛔⛔ ALL FOUR RATCHETS ARE JUDGED IN BOTH DIRECTIONS, THROUGH ONE DOOR — register item
    // 926. Each of these was `if counted > floor`, which holds *may shrink, never grow* and says
    // nothing when the floor drifts ABOVE the count. Two of the four had drifted on the real
    // ledger, and the gap was free room for unmarked items. See [`ratchet`].
    //
    // 🎯🎯🎯 AND OFF THE SAME SETS THE REPORT PRINTS — register item 934. These were four
    // `filter(…).count()` chains here and four more on `Reading`, so what red and what printed
    // were written twice. See [`Reading::backlogs`], which is now the only author of the four.
    //
    // ⚠⚠⚠ THE SEVERITY BACKLOG IS *OPEN* ITEMS, AND THAT IS THE MEASURED CHOICE, NOT AN OVERSIGHT.
    // Item 926 asks which way that backlog should be counted now that slack reds, and names the
    // alternative: count EVERY item rather than only open ones, so that paying does not move the
    // number and the floor never has to follow. Measured on this ledger, 2026-09-06 13:1x UTC:
    //
    //   open items with no severity  ...  47      shrinks as unclassified items are paid
    //   ALL items with no severity   ... 380      a paid item still has none, so it never shrinks
    //
    // The wider population is eight times larger AND monotonically non-decreasing — paying an item
    // does not take it out, so the floor could never be tightened by doing the work. That is the
    // *"ratchet that punishes payment"* this file's own doc warns about, arriving by the other
    // road. So the narrow population stays, and the cost is accepted and named: a round that pays
    // an unclassified item now goes red until it lowers the floor by one. The refusal names the
    // number to write, which is what keeps that a one-line edit rather than an investigation.
    //
    // ⛔ The fourth is the PAID marks that name no commit — register item 902, held by the same
    // ratchet its three neighbours are. See [`PAID_DECLARATION`] for why an item that has LEFT the
    // population is the one whose claim most needs checking.
    let backlogs = reading.backlogs();
    ratchet(
        &backlogs.unclassified,
        |counted, declared| Fault::RatchetGrew { counted, declared },
        &mut reading.faults,
    );
    ratchet(
        &backlogs.unranked,
        |counted, declared| Fault::SeverityRatchetGrew { counted, declared },
        &mut reading.faults,
    );
    ratchet(
        &backlogs.unrooted,
        |counted, declared| Fault::ParentRatchetGrew { counted, declared },
        &mut reading.faults,
    );
    ratchet(
        &backlogs.paid_unnamed,
        |counted, declared| Fault::PaidRatchetGrew { counted, declared },
        &mut reading.faults,
    );

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
@paid-uncommitted: 1

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

    /// ⛔⛔⛔⛔⛔ **A `paid` MARK IS COUNTED UNTIL IT NAMES A COMMIT** — register item 902's ⑴, and
    /// the arm the whole ratchet stands on.
    ///
    /// Item 866(2) sat in the ledger marked `paid`, argued in full, while its 720 lines had never
    /// been committed or seen a hook. The mark parsed perfectly; it was simply not true about the
    /// repository, and an item marked paid has already left [`Reading::population`].
    #[test]
    fn a_paid_mark_is_counted_until_it_names_a_commit() {
        assert_eq!(
            read(LEDGER).paid_unnamed(),
            vec![899],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 902: a `paid` mark naming no commit is a claim about the \
             repository that nothing checked, and this is the only place it is counted",
        );
        let named = LEDGER.replace(
            "     @ns: paid\n",
            "     @ns: paid — 2026-09-02 `5243b28`\n",
        );
        assert!(
            read(&named).paid_unnamed().is_empty(),
            "⚠⚠ AND NAMING ONE CLEARS IT, or the ratchet has no zero and this workspace's rule 5 \
             is broken by the predicate: {:?}",
            read(&named).paid_unnamed(),
        );
    }

    /// ⛔⛔ **AND ONLY A `paid` MARK IS ASKED** — an OPEN item has nothing to have committed, and an
    /// `out` one was never this loop's. A ratchet that counted those would grow every time a round
    /// opened an item, which is the trap [`SEVERITY_DECLARATION`] records avoiding.
    #[test]
    fn only_a_paid_mark_is_asked_for_a_commit() {
        let counted = read(LEDGER).paid_unnamed();
        assert!(
            !counted.contains(&900) && !counted.contains(&898) && !counted.contains(&897),
            "⛔⛔⛔ REGISTER ITEM 902: the open item, the out one and the unmarked one are not \
             claims of payment and must not be in this population: {counted:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A BACKTICKED WORD THAT IS NOT A COMMIT ID IS NOT READ AS ONE** — register item
    /// 902, holding the lesson register item 901 was opened by: a needle that matches inside
    /// innocent text refuses innocent files, and two readings of *what is a commit id* would let a
    /// `paid` mark clear itself by quoting a function name.
    #[test]
    fn only_a_hex_run_of_commit_length_is_read_as_a_commit() {
        for innocent in [
            "`held()`",
            "`abcdef`",
            "`0.0.1`",
            "`Cargo.toml`",
            "`DEADBEE`",
        ] {
            let ledger = LEDGER.replace(
                "     @ns: paid\n",
                &format!("     @ns: paid — closed by {innocent}\n"),
            );
            assert_eq!(
                read(&ledger).paid_unnamed(),
                vec![899],
                "⛔⛔⛔⛔⛔ REGISTER ITEM 902: {innocent} is not a commit id — six hex is under \
                 git's own abbreviation floor, and the rest are not hex at all. A mark that clears \
                 itself with one of these is the defect this gate exists to catch",
            );
        }
        for real in ["`deadbee`", &format!("`{}`", "a".repeat(40))] {
            let ledger = LEDGER.replace(
                "     @ns: paid\n",
                &format!("     @ns: paid — closed by {real}\n"),
            );
            assert!(
                read(&ledger).paid_unnamed().is_empty(),
                "⚠⚠ AND THE CONTROL: {real} is a commit id at both ends of the length this reader \
                 admits, and refusing it would make the ratchet unclearable",
            );
        }
    }

    /// ⛔⛔⛔⛔⛔ **A NEW `paid` MARK THAT NAMES NOTHING RAISES THE RATCHET** — register item 902,
    /// and the whole point of a floor: the backlog is tolerated, adding to it is not.
    #[test]
    fn a_new_paid_mark_that_names_nothing_is_red() {
        let ledger = LEDGER.replace(
            "     @ns: out — a rendering defect, nothing to do with the loop",
            "     @ns: paid — done, and nobody wrote down where",
        );
        assert!(
            read(&ledger).faults.contains(&Fault::PaidRatchetGrew {
                // ⚠ THE ITEMS AND NOT A `2` — register item 934. A `counted: 2` here would pass
                // against a reading that counted the wrong two.
                counted: vec![898, 899],
                declared: 1,
            }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 902: the floor may only fall. A round that marks something \
             paid without naming the commit must go red on that round, because afterwards the item \
             is out of the population and nobody looks at it again: {:?}",
            read(&ledger).faults,
        );
    }

    /// ⛔⛔⛔⛔⛔ **A LEDGER THAT DECLARES NO FLOOR IS REFUSED** — this workspace's rule 6, and the
    /// arm that stops the whole of register item 902 from being switched off by deleting one line.
    #[test]
    fn a_ledger_that_declares_no_paid_floor_is_red() {
        let ledger = LEDGER.replace("@paid-uncommitted: 1\n", "");
        assert!(
            read(&ledger)
                .faults
                .contains(&Fault::PaidDeclaration { found: 0 }),
            "⛔⛔⛔⛔⛔ RULE 6: an absent declaration must be a RED and never a ratchet that \
             quietly holds nothing — deleting one line would otherwise retire this gate: {:?}",
            read(&ledger).faults,
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND AN ID THIS TREE CANNOT RESOLVE IS RED WITH NO FLOOR AT ALL** — register item
    /// 902's ⑵, the half no document can answer about itself.
    ///
    /// ⚠⚠⚠ AND A FAILURE TO **ASK** IS AN ERROR RATHER THAN FORTY FINDINGS: being unable to reach
    /// `git` says nothing about any ledger line, and a RED that fires whenever the environment is
    /// odd is one a reader learns to skip — which is how a gate dies while still passing.
    #[test]
    fn a_named_commit_that_does_not_resolve_is_red_and_a_broken_asker_is_not() {
        struct Answers(Result<bool, String>);
        impl Commits for Answers {
            fn resolves(&self, _id: &str) -> Result<bool, String> {
                self.0.clone()
            }
        }

        let ledger = LEDGER.replace(
            "     @ns: paid\n",
            "     @ns: paid — 2026-09-02 `deadbee`\n",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.paid_unresolved(&Answers(Ok(false))).expect("asked"),
            vec![(899, "deadbee".to_string())],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 902: an id that is WRITTEN and does not resolve has no \
             backlog — it is a typo or a claim about a commit nobody made, and there is no round \
             in which it is acceptable",
        );
        assert!(
            reading
                .paid_unresolved(&Answers(Ok(true)))
                .expect("asked")
                .is_empty(),
            "⚠ THE CONTROL: an id that resolves is the state this gate wants and must be silent",
        );
        assert_eq!(
            reading.paid_unresolved(&Answers(Err("no git here".to_owned()))),
            Err("no git here".to_owned()),
            "⚠⚠⚠ AND THE SHARPEST ARM: a broken asker must come back as ONE error about the \
             instrument, never as a finding against the ledger — see the doc above",
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
                // 896 is the item this mutation adds; 897 is the fixture's standing unmarked one.
                counted: vec![896, 897],
                declared: 1,
            }),
            "the backlog may shrink, never grow: {:?}",
            reading.faults,
        );
    }

    /// And paying the backlog down is NOT a fault — otherwise the ratchet would punish the only
    /// move that ends it, and register item 823 would have no zero.
    #[test]
    fn marking_an_item_shrinks_the_backlog_and_the_floor_must_follow_it_down() {
        let marked = LEDGER.replace(
            "897. ⛔ **Unmarked and quiet**\n     no mark, no mention",
            "897. ⛔ **Now classified**\n     @ns: out — a build-system item\n     no mention",
        );
        let reading = read(&marked);
        assert!(reading.unclassified().is_empty(), "the backlog emptied");

        // ⛔⛔⛔⛔⛔ THIS ASSERTION IS THE REVERSE OF WHAT IT WAS, AND THE REVERSAL IS THE POINT —
        // register item 926. It used to read *"counting fewer than declared is the goal, not a
        // fault"* and assert `is_green()`. That sentence is what left the other direction
        // unwatched: on the real ledger the floors then drifted two and four above their counts,
        // and that gap was room for six items to be registered unmarked with nothing going red.
        //
        // ⚠ Shrinking the count is still the goal. What is refused is shrinking it and LEAVING THE
        // FLOOR where it was, because a floor that no longer touches the count has stopped being a
        // ratchet and become a budget.
        assert!(
            reading.faults.contains(&Fault::RatchetSlack {
                token: DECLARATION,
                counted: Vec::new(),
                declared: 1,
            }),
            "paying the backlog down without lowering the floor leaves slack, and slack is free \
             room for the next unmarked item: {:?}",
            reading.faults,
        );

        // …and lowering it in the same edit is green. This is the whole ritual item 926's third
        // clause asks for, and the refusal above names the number to write, so it is one line.
        let lowered = marked.replace("@ns-unclassified: 1", "@ns-unclassified: 0");
        let reading = read(&lowered);
        assert!(
            reading.is_green(),
            "the floor now equals the count, which is what a tightened ratchet looks like: {:?}",
            reading.faults,
        );
    }

    /// ⛔⛔⛔⛔⛔ **A FLOOR ABOVE THE COUNT IS REFUSED, ON EVERY ONE OF THE FOUR** — register item
    /// 926, and the mutation it asks for by name: *put a fixture's floor one above what it counts,
    /// and if the reading is green the gate is not there.*
    ///
    /// # ⚠⚠ Why the four are a TABLE and not four test functions
    ///
    /// The defect being paid was one shape written four times with the second half missing from
    /// every copy. Four hand-written cases would reproduce exactly that: the fifth ratchet somebody
    /// adds is covered only if they remember to add a fifth case. Here the declarations are walked,
    /// so a new one is covered by construction — and the count is asserted, so a table that
    /// silently lost an entry cannot pass as *all four*.
    #[test]
    fn a_floor_standing_above_the_count_is_refused_on_every_ratchet() {
        // The floors the fixture declares, each with the count it actually sits on.
        let floors: [(&str, usize); 4] = [
            (DECLARATION, 1),
            (SEVERITY_DECLARATION, 0),
            (PARENT_DECLARATION, 3),
            (PAID_DECLARATION, 1),
        ];
        assert_eq!(
            floors.len(),
            4,
            "this repository declares four floors; a table that lost one would still read as \
             covering every ratchet",
        );

        // ⚠ THE FIXTURE SITS EXACTLY ON ITS FLOORS, asserted rather than assumed — every case
        // below is a ONE-STEP move off this point, and if the fixture already carried slack the
        // moves would prove nothing.
        assert!(
            !read(LEDGER)
                .faults
                .iter()
                .any(|fault| matches!(fault, Fault::RatchetSlack { .. })),
            "the fixture must start with its floors on its counts: {:?}",
            read(LEDGER).faults,
        );

        let mut missed: Vec<String> = Vec::new();
        for (token, count) in floors {
            let raised = LEDGER.replace(
                &format!("{token} {count}"),
                &format!("{token} {}", count + 1),
            );
            assert_ne!(
                raised, LEDGER,
                "the mutation for `{token}` changed nothing, so its case is asserting about the \
                 unmutated fixture",
            );
            let reading = read(&raised);
            let found = reading.faults.iter().any(|fault| {
                matches!(
                    fault,
                    Fault::RatchetSlack { token: named, counted, declared }
                        if *named == token && counted.len() == count && *declared == count + 1
                )
            });
            if !found {
                missed.push(format!(
                    "`{token}` raised to {}: {:?}",
                    count + 1,
                    reading.faults
                ));
            }
        }
        assert!(
            missed.is_empty(),
            "⛔ ITEM 926: a floor one above the count went unrefused, which is one free unmarked \
             item nothing would have said a word about:\n{}",
            missed.join("\n"),
        );
    }

    /// ⚠⚠ **AND THE REFUSAL SAYS WHAT TO DO** — register item 926's second clause. A gate that
    /// only names the discrepancy makes the reader work out the fix the tool already holds; item
    /// 794's ratchet sets the house style by naming the number to write.
    #[test]
    fn a_slack_refusal_names_the_number_to_write() {
        let raised = LEDGER.replace("@sev-unclassified: 0", "@sev-unclassified: 3");
        let reading = read(&raised);
        let said = reading
            .faults
            .iter()
            .find(|fault| matches!(fault, Fault::RatchetSlack { .. }))
            .map(ToString::to_string)
            .expect("a floor three above the count is refused");
        assert!(
            said.contains("Lower `@sev-unclassified:` to 0"),
            "the remedy and its number belong in the sentence: {said}",
        );
        assert!(
            said.contains("3 more item(s)"),
            "and so does what the slack was WORTH, which is the reason it is a red: {said}",
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
                counted: vec![900],
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
    ///
    /// ⚠ Each link STATES its verdict (`만들었다`), because register item 920 made an unstated one
    /// a [`Fault::DeferredByUnreadLink`] and a fixture that trips a gate by accident is a fixture
    /// nobody can mutate deliberately.
    fn with_a_chain() -> String {
        let link = |number: u32, from: u32| {
            format!(
                "{number}. ⛔ **Found while paying {from}**\n     @ns: open\n     @sev: ordinary\n \
                 \u{20}   @from: {from} — {from} 을 갚으며 «만들었다»\n\n"
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
            reading.admits(1, &[]),
            vec![900],
            "🎯 the ordinary one is open, takeable, and NOT what a round takes next — that is the \
             whole of working rule 11, and until this predicate existed it was prose. ⚠ No red is \
             claimed here, and register item 843's `&[]` says so out loud rather than by omission",
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
            reading.admits(1, &[]),
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
            reading.admits(1, &[]),
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
                // 895 is the item this mutation adds; 897–899 are the fixture's standing three.
                counted: vec![895, 897, 898, 899],
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

    // ── register item 920: the chain that defers is READ, not merely counted ────────────────────

    /// The three answers a reason can give, over the words the ledger was measured to use.
    #[test]
    fn a_reason_is_read_as_made_met_or_neither() {
        assert_eq!(
            says("840 을 갚으며 «만든» 리더다"),
            Says::Made("만든"),
            "the creation vocabulary, and the word it matched comes back as the evidence",
        );
        assert_eq!(
            says("852 를 재느라 GUI 로그를 읽다 «마주친» 것이다"),
            Says::Met("마주친"),
        );
        assert_eq!(
            says("856 의 접힘이 세션 교체를 부르고, 그 교체가 이 결함의 «입구»다"),
            Says::Neither,
            "⛔⛔⛔ THE WHOLE OF THIS ITEM: `입구` is in neither array, and the build before this \
             one had no way to SAY so — `met_while` answers `None` here and `None` for a creation \
             word alike, which is working rule 6's escape hatch",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE SUBSTRING LESSON SURVIVES THE UNIFICATION** — `드러났다` contains `났다`, and
    /// [`says`] is now the only place that knows it. A build that asked the creation set over the
    /// whole sentence would call every *was revealed* line a made-by-paying line, which is exactly
    /// what item 896 measured and repaired.
    #[test]
    fn a_reason_that_says_revealed_is_still_not_a_reason_that_says_made() {
        assert_eq!(
            says("865 가 그 물음을 세우자 «성공»했고, 그때 드러났다"),
            Says::Met("드러났다"),
            "⛔ the matched word is cut out BEFORE the other set is asked",
        );
    }

    /// The chain comes back as links, nearest first, each carrying its own verdict — the object an
    /// audit is about, rather than the integer it collapses to.
    #[test]
    fn the_chain_that_defers_an_item_is_returned_link_by_link() {
        let reading = read(&with_a_chain());
        assert_eq!(
            reading.chain(902),
            Some(vec![
                Link {
                    number: 902,
                    named: 901,
                    says: Says::Made("만들"),
                    parent: Some(Tag::Open),
                },
                Link {
                    number: 901,
                    named: 900,
                    says: Says::Made("만들"),
                    parent: Some(Tag::Open),
                },
            ]),
            "two hops to a declared root, and 900 itself adds no link because a root is where a \
             chain ends. ⚠ Each link carries the PARENT's mark — register item 921, the fact the \
             cap decides on: both of these are still owed, so both still hold",
        );
        assert_eq!(
            reading.chain(900),
            Some(Vec::new()),
            "a root's chain is EMPTY and not absent — that is what makes its depth 0",
        );
    }

    /// ⚠⚠ **THE DEPTH AND THE PRINTED CHAIN ARE ONE WALK.** Two walks over the same marks can
    /// disagree, and then a reader auditing the chain is auditing a different object from the one
    /// that defers the item.
    #[test]
    fn the_depth_is_the_length_of_the_chain_it_prints() {
        let reading = read(&with_a_chain());
        for number in [900, 901, 902] {
            assert_eq!(
                reading.depth(number),
                reading
                    .chain(number)
                    .map(|links| u32::try_from(links.len()).unwrap()),
                "item {number}",
            );
        }
        assert_eq!(reading.depth(897), None, "and unknown stays unknown, not 0");
        assert_eq!(reading.chain(897), None, "in both answers");
    }

    /// 🎯🎯🎯🎯🎯 **THE MUTATION**: turn one link's reason into a sentence in neither vocabulary —
    /// the real `846 @from: 840 — 840 을 갚으며 «내가» 골라 넣은 수다` — and the deferral it
    /// produces must go RED. The build before this one printed the same `deferred` line and said
    /// nothing.
    #[test]
    fn a_deferral_resting_on_a_reason_in_neither_vocabulary_is_red() {
        let ledger = with_a_chain().replace(
            "@from: 901 — 901 을 갚으며 «만들었다»",
            "@from: 901 — 901 을 갚으며 «내가» 골라 넣은 수다",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.deferred(1),
            vec![902],
            "the item is still deferred — the depth did not move, only the reason for it",
        );
        assert!(
            reading.is_green(),
            "⛔⛔ AND `read` ITSELF STAYS SILENT, which is the point: item 896's ratchet does not \
             reach this and no other fault does either: {:?}",
            reading.faults,
        );
        let screened = reading.deferred_unread(1);
        assert_eq!(
            screened.faults,
            vec![Fault::DeferredByUnreadLink {
                held: 902,
                at: 902,
                named: 901,
            }],
            "the deferral names the link it rests on",
        );
        // 🎯 REGISTER ITEM 924: and the denominator, so this assertion is about a walk that
        // happened. 902's chain is `902 → 901` and `901 → 900`: two links, one of them unread.
        assert_eq!(
            screened.judged, 2,
            "both links of 902's chain were asked about, and one of them answered: {screened}",
        );
    }

    /// ⚠⚠⚠ **THE CONTROL** — the same ledger with every link stating its verdict is SILENT. A gate
    /// that fires on the state it wants teaches a reader to skip it.
    #[test]
    fn a_chain_whose_every_link_states_its_verdict_is_silent() {
        let reading = read(&with_a_chain());
        assert_eq!(
            reading.deferred(1),
            vec![902],
            "there IS a deferral here, so the silence below is not an empty population",
        );
        let screened = reading.deferred_unread(1);
        assert!(screened.faults.is_empty(), "{:?}", screened.faults);
        // 🎯🎯🎯 REGISTER ITEM 924: AND THE SILENCE IS THE SECOND KIND. The line above said the
        // population was not empty by asking a NEIGHBOURING predicate; this says it in the gate's
        // own terms, which is the only place the two kinds of empty can actually be told apart.
        assert_eq!(
            screened.judged, 2,
            "a control that examined nothing controls nothing: {screened}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE POPULATION IS THE DEFERRED, AND WIDENING IT WOULD BE THE OTHER ITEM'S JOB.**
    /// An unread reason on a TAKEABLE item costs nothing today — no debt is held back on it — and
    /// it is item 896's standing backlog of seventy-odd lines, which needs a floor to pay down.
    /// A build that red on all of them would be red for months and read as broken.
    #[test]
    fn an_unread_reason_that_defers_nothing_is_item_896s_backlog_and_not_this_gate() {
        let ledger = with_a_chain().replace(
            "@from: 900 — 900 을 갚으며 «만들었다»",
            "@from: 900 — 900 을 갚으며 «내가» 골라 넣은 수다",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.depth(901),
            Some(1),
            "901 sits AT the cap, so nothing is held back by this link",
        );
        let screened = reading.deferred_unread(1);
        assert_eq!(
            screened.faults,
            vec![Fault::DeferredByUnreadLink {
                held: 902,
                at: 901,
                named: 900,
            }],
            "⚠ 902 IS still deferred and its chain runs through that link, so it is named — the \
             link is charged where it costs, and 901's own takeability is not this gate's subject",
        );
        // 🎯 REGISTER ITEM 924: the denominator is LINKS and not deferred items. One item is held
        // back here and two links hold it, so a `judged` reading 1 would be counting the line
        // above's set under this line's name.
        assert_eq!(
            screened.judged, 2,
            "both links of the one deferred chain were asked about: {screened}",
        );
    }

    // ── register item 843: a standing red is admissible whatever its severity ───────────────────

    /// A suite that answers from a table rather than by running anything — the two verdicts and the
    /// third answer, *could not be asked*.
    struct Answers(std::collections::BTreeMap<String, Result<bool, String>>);

    impl Suite for Answers {
        fn is_red(&self, names: &str) -> Result<bool, String> {
            self.0.get(names).cloned().unwrap_or_else(|| {
                panic!("the fixture was asked about `{names}`, which it has no answer for")
            })
        }
    }

    /// A ledger shaped like the day item 843 was registered: one critical item standing, and an
    /// ORDINARY item that is red right now.
    fn with_a_standing_red() -> String {
        LEDGER
            .replace(
                "899. ✅✅ **PAID 2026-09-02**",
                "898. ⛔ **An ordinary item that is RED**\n     @ns: open\n     @sev: ordinary\n     \
                 @from: none\n     @red: -p sprag-gate --lib north_star\n\n899. ✅✅ **PAID \
                 2026-09-02**",
            )
            // ⚠⚠ AND THE FLOOR FOLLOWS THE LEDGER THIS BUILDS — register item 926. The block above
            // is inserted ABOVE the existing 898 and therefore wins the topmost-block tie, so item
            // 898 acquires a `@from: none` it did not have and the unrooted count falls 3 → 2.
            // Leaving the declaration at 3 would make every caller of this helper carry a slack
            // fault about a discrepancy the helper itself created. ⛔ This is the declaration being
            // made TRUE of its own text, not a guard being loosened: the count really is 2 here.
            .replace("@from-unclassified: 3", "@from-unclassified: 2")
    }

    /// ⛔⛔⛔⛔⛔ **THE FINDING: A RED IS IN THE SET THOUGH A CRITICAL ITEM STANDS** — register item
    /// 843's done-when ⑴, and the four days item 837 spent unreachable.
    #[test]
    fn a_standing_red_is_admissible_while_a_critical_item_stands() {
        let reading = read(&with_a_standing_red());
        let claims = reading.red_claims();
        assert_eq!(
            claims,
            vec![(898, "-p sprag-gate --lib north_star".to_string())],
            "⚠ THE CONTROL: the ledger claims exactly one red, so the two arms below differ by the \
             SUITE's answer and by nothing else",
        );
        assert_eq!(
            reading.critical(),
            vec![900],
            "and a critical item stands, which is what shuts the gate",
        );

        assert_eq!(
            reading.admits(1, &[]),
            vec![900],
            "⛔⛔⛔⛔⛔ WITHOUT THE RED, THIS IS THE DEFECT: item 898 is open, takeable, and RED, \
             and a proposal naming it is counted and not taken because something else is critical. \
             Item 837 sat here for four days",
        );
        assert_eq!(
            reading.admits(1, &[898]),
            vec![898, 900],
            "🎯 with it, the red joins the set WITHOUT its severity moving — which is the repair \
             843 asks for and refuses the alternative to: writing `@sev: critical` on a red makes \
             the instrument disagree with what severity means",
        );
        assert_eq!(
            reading
                .items
                .iter()
                .find(|item| item.number == 898)
                .and_then(|item| item.severity),
            Some(Severity::Ordinary),
            "⚠⚠ AND ITS SEVERITY IS UNTOUCHED, asserted rather than assumed: the whole of 843's \
             refusal is that the mark must not be manipulated to get the item admitted",
        );
        assert!(
            !reading.critical().contains(&898),
            "which is the same claim from the other side — the red is admitted WITHOUT joining \
             the critical set: {:?}",
            reading.critical(),
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND THE LEDGER DOES NOT GET TO ANSWER IT** — done-when ⑴'s second half: *그
    /// 사실이 «원장이 아니라 저장소»에 물어서 확인된다*.
    #[test]
    fn the_suite_and_not_the_ledger_says_whether_a_claimed_red_is_red() {
        let reading = read(&with_a_standing_red());
        let asked = "-p sprag-gate --lib north_star".to_string();

        let red = Answers([(asked.clone(), Ok(true))].into_iter().collect());
        assert_eq!(
            reading.standing_reds(&red).expect("the suite answered"),
            (vec![898], Vec::new()),
            "the suite ran it and it failed, so the claim stands",
        );

        let green = Answers([(asked.clone(), Ok(false))].into_iter().collect());
        assert_eq!(
            reading.standing_reds(&green).expect("the suite answered"),
            (Vec::new(), vec![898]),
            "⛔ THE SAME LEDGER, THE OPPOSITE ANSWER. A `@red:` line is a CLAIM about a tree, and \
             an item whose claim the suite refutes must not be admitted on it — item 902's \
             wrongly-paid mark, pointing the other way",
        );

        let mute = Answers(
            [(asked, Err("cargo is not on the path".to_string()))]
                .into_iter()
                .collect(),
        );
        assert_eq!(
            reading.standing_reds(&mute),
            Err("cargo is not on the path".to_string()),
            "⚠⚠ AND *COULD NOT ASK* IS ITS OWN ANSWER, never folded into `false`: a suite that \
             cannot be run says nothing about any claim, which is `Commits::resolves`' rule and \
             the reason both of these return a `Result`",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A CLAIM THIS INSTRUMENT CANNOT PUT TO THE REPOSITORY IS RED, NOT SILENT** —
    /// working rule 6, on the one mark in this file that is EXECUTED rather than looked up.
    #[test]
    fn a_red_claim_that_could_do_anything_is_refused_at_the_door() {
        for spelled in [
            "@red: -p sprag-gate; rm -rf /",
            "@red: $(whoami)",
            "@red:",
            "@red: --lib `id`",
        ] {
            let ledger = with_a_standing_red()
                .replace("@red: -p sprag-gate --lib north_star", spelled.trim_start());
            let reading = read(&ledger);
            assert!(
                reading
                    .faults
                    .iter()
                    .any(|fault| matches!(fault, Fault::UnrunnableRed { number: 898, .. })),
                "{spelled:?} must be refused: {:?}",
                reading.faults,
            );
            assert!(
                reading.red_claims().is_empty(),
                "and it must not be carried as a claim either — a value the reader rejected must \
                 not reach the thing that runs it: {:?}",
                reading.red_claims(),
            );
        }
    }

    /// ⚠⚠⚠ **THE CONTROL FOR THE ONE ABOVE**: the ordinary spellings a red is actually named with
    /// go through, or the door refuses everything and this gate is about nothing.
    #[test]
    fn the_spellings_a_red_is_really_named_with_are_admitted() {
        for spelled in [
            "@red: -p sprag-host --lib plugins::tests::a_loop_started_over_the_wire --exact",
            "@red: --workspace",
            "@red: -p sprag-gate --test no_product_code_takes_a_scratch_root_unchecked",
        ] {
            let ledger = with_a_standing_red()
                .replace("@red: -p sprag-gate --lib north_star", spelled.trim_start());
            let reading = read(&ledger);
            assert!(
                reading.is_green(),
                "{spelled:?} is how item 837's own table names a red: {:?}",
                reading.faults,
            );
            assert_eq!(reading.red_claims().len(), 1, "{spelled:?}");
        }
    }

    /// A red on a PAID item is history and must not admit anything — the rule every other mark
    /// here follows about the population.
    #[test]
    fn a_red_an_item_used_to_have_is_not_a_red_now() {
        let ledger = with_a_standing_red().replace(
            "898. ⛔ **An ordinary item that is RED**\n     @ns: open",
            "898. ⛔ **An ordinary item that is RED**\n     @ns: paid `0c034d6`",
        );
        let reading = read(&ledger);
        assert!(
            reading.red_claims().is_empty(),
            "a paid item's red is not this loop's to run: {:?}",
            reading.red_claims(),
        );
    }

    // ── register item 921: the cap counts DEBTS above, not links ────────────────────────────────

    /// ⚠⚠⚠⚠⚠ **THE CONTROL, AND IT IS THE WHOLE OF WHETHER THIS ITEM IS A REPAIR OR A HOLE.**
    /// Register item 921 makes a closed parent stop holding its child, and the one thing that must
    /// survive it is the case the cap was BUILT from: a run paying an open item, finding a second,
    /// and finding a third while paying that. Every link there is still owed, so every one still
    /// counts and the third is still deferred.
    ///
    /// ⚠⚠ Asserted FIRST and in its own test, because the change is one this round wanted: it makes
    /// item 843 takeable. A finding that arrives with the outcome its author was hoping for has to
    /// be held by the case that would embarrass it.
    #[test]
    fn a_chain_whose_parents_are_all_still_owed_defers_exactly_as_before() {
        let reading = read(&with_a_chain());
        assert_eq!(
            reading.debts_above(900),
            Some(0),
            "a root sits under nothing",
        );
        assert_eq!(
            reading.debts_above(901),
            Some(1),
            "found while paying 900, which is STILL OPEN — the link holds",
        );
        assert_eq!(
            reading.debts_above(902),
            Some(2),
            "and this is the 2026-09-02 shape the cap was measured from: a chain built and chased \
             inside one day, every link still owed",
        );
        assert_eq!(
            reading.deferred(1),
            vec![902],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 921 MUST NOT EMPTY THE CAP. If this line ever reads `[]` the \
             change stopped being *a closed parent releases its child* and became *the cap defers \
             nothing*, which is the one move the north star forbids",
        );
        assert!(
            reading.released(1).is_empty(),
            "and nothing was released here, because nothing above these is closed: {:?}",
            reading.released(1),
        );
    }

    /// 🎯🎯🎯🎯🎯 **A PAID PARENT STOPS HOLDING ITS CHILD** — register item 921's finding, and the
    /// sentence [`Reading::deferred`] had been making falsely: *not to be worked until the budget
    /// allows*, where nothing ever allowed.
    #[test]
    fn a_parent_that_has_been_paid_no_longer_holds_the_debt_below_it() {
        // The middle of the chain closes — 901 is paid, exactly as 840 and 839 were for days while
        // the items under them stayed deferred.
        let ledger = with_a_chain().replace(
            "901. ⛔ **Found while paying 900**\n     @ns: open",
            "901. ⛔ **Found while paying 900**\n     @ns: paid `0c034d6`",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.depth(902),
            Some(2),
            "⚠⚠ THE FACT IS UNCHANGED: 902 still came out of 901 which came out of 900. Register \
             item 921 did not rewrite what created what, and a build that moved THIS number would \
             be falsifying the chain rather than reading it",
        );
        assert_eq!(
            reading.debts_above(902),
            Some(0),
            "⚠⚠ AND ZERO IS THE RIGHT ANSWER, not one: 부채의 부채 is a debt whose PARENT is a \
             debt, and 902's parent is 901, which has been paid. Nothing open sits above it — what \
             900 was to 901 is 901's history and not 902's, because the walk stops where the chain \
             stops being owed",
        );
        assert!(
            reading.deferred(1).is_empty(),
            "so the cap lets it go: {:?}",
            reading.deferred(1),
        );
        assert_eq!(
            reading.released(1),
            vec![902],
            "and the instrument SAYS SO rather than leaving an empty line to be read two ways",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AN UNMARKED PARENT HOLDS THE LINK** — working rule 6, and the escape hatch this
    /// item was two lines from opening.
    ///
    /// This ledger carries 330 items that state no mark. Treating *nobody said* as *closed* would
    /// hand every one of them the power to free its children silently, and nothing about the
    /// instrument's output would look wrong.
    #[test]
    fn a_parent_nobody_marked_is_not_a_parent_that_was_paid() {
        let ledger = with_a_chain().replace(
            "901. ⛔ **Found while paying 900**\n     @ns: open",
            "901. ⛔ **Found while paying 900**\n     nothing says what became of it",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.debts_above(902),
            Some(2),
            "⛔ UNKNOWN COUNTS. A mark that is absent is not a mark that says paid, and the \
             conservative side of that is the side that keeps the deferral",
        );
        assert_eq!(
            reading.deferred(1),
            vec![902],
            "so it is still held: {:?}",
            reading.deferred(1),
        );
    }

    /// An item marked [`Tag::Out`] is not this loop's debt either, so it releases what sits under
    /// it — the same rule as [`Tag::Paid`], for a different reason that lands in the same place.
    #[test]
    fn a_parent_that_was_never_this_loops_debt_releases_what_is_under_it() {
        let ledger = with_a_chain().replace(
            "901. ⛔ **Found while paying 900**\n     @ns: open",
            "901. ⛔ **Found while paying 900**\n     @ns: out — a rendering defect",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.debts_above(902),
            Some(0),
            "nothing OWED sits above it, for the paid case's reason exactly",
        );
        assert!(reading.deferred(1).is_empty(), "{:?}", reading.deferred(1),);
    }

    /// ⚠⚠⚠ **AND A CLOSED LINK IN THE MIDDLE STOPS THE WALK RATHER THAN BEING SKIPPED**, which is
    /// the arithmetic a reader would most likely get wrong: what sits above a closed parent is that
    /// parent's history, not its child's. Register item 921.
    #[test]
    fn the_walk_stops_at_a_closed_parent_instead_of_stepping_over_it() {
        // 900 — the ROOT of this chain — is the one that closes, so 901 is still owed and 902 sits
        // under exactly it.
        let ledger = with_a_chain().replace(
            "     @ns: open — the loop's own driver",
            "     @ns: paid `0c034d6` — the loop's own driver",
        );
        let reading = read(&ledger);
        assert_eq!(
            reading.debts_above(901),
            Some(0),
            "901's only parent is 900, and 900 is paid",
        );
        assert_eq!(
            reading.debts_above(902),
            Some(1),
            "⛔ ONE, not two and not zero: 901 is still open so its link counts, and the link ABOVE \
             901 names a paid item so the walk ends there",
        );
        assert!(
            reading.deferred(1).is_empty(),
            "which the cap of one admits: {:?}",
            reading.deferred(1),
        );
    }

    /// ⚠⚠ **AND THE CAP'S OWN NUMBER IS UNTOUCHED**, which is the other half of not widening a
    /// gate: this item changed WHAT IS COUNTED and not HOW MANY are allowed. `debt_loop.scxml`
    /// still authors 1, and `reaim_max`'s own comment offers a bigger number for a different
    /// condition — *findings being lost* — which this ledger does not meet.
    #[test]
    fn register_item_921_did_not_move_the_owners_number() {
        let document = include_str!("../../sprag-plugin/src/debt_loop.scxml");
        assert_eq!(
            declared_reaim(document),
            Ok(Reaim::Of(1)),
            "⛔⛔⛔⛔⛔ the cap is the owner's decision of 2026-09-02 and register item 921 is not \
             a licence to move it. If this line ever fails, read whether the round that moved it \
             had the owner's word",
        );
    }

    /// A cap that holds nothing back has nothing to audit, and must not manufacture a finding out
    /// of the same unread lines — working rule 5, asked of this gate's own population.
    ///
    /// # ⛔⛔⛔⛔⛔ AND THIS IS REGISTER ITEM 924's OWN SHAPE, WRITTEN DOWN
    ///
    /// The silence asserted below is the VACUOUS one: nothing was examined. Until item 924 this
    /// test could not say so and neither could the report — it read exactly like
    /// [`a_chain_whose_every_link_states_its_verdict_is_silent`], which examines two links and
    /// finds them clean. Measured 2026-09-06, that is not a hypothetical: after register item 921
    /// the real ledger's deferrals went to none and this gate ran vacuously for a whole round while
    /// its `rc` stayed 0.
    ///
    /// ⚠⚠ The `judged` line is also the counter-example a wrong denominator dies on. This ledger
    /// still carries two `@from:` links and one of them is unread — a `judged` counting the LEDGER
    /// rather than the WALK reads 2 here, and a `judged` that were just `faults.len()` reads 1 in
    /// the sibling test. Only the walk reads 0.
    #[test]
    fn a_cap_that_defers_nothing_finds_nothing_to_read() {
        let ledger = with_a_chain().replace(
            "@from: 901 — 901 을 갚으며 «만들었다»",
            "@from: 901 — 901 을 갚으며 «내가» 골라 넣은 수다",
        );
        let reading = read(&ledger);
        assert!(reading.deferred(9).is_empty(), "{:?}", reading.deferred(9),);
        let screened = reading.deferred_unread(9);
        assert!(
            screened.faults.is_empty(),
            "the reason is as unread as ever; what changed is that it now costs nothing: {:?}",
            screened.faults,
        );
        assert_eq!(
            screened.judged, 0,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 924: this gate examined NOTHING, and the difference between \
             that and examining things and finding them clean is the whole of the item",
        );
        assert_eq!(
            screened.to_string(),
            "deferral links 0 judged, 0 unread",
            "⛔⛔⛔⛔⛔ REGISTER ITEM 924(1): and it is PRINTED. A count a reader never sees is \
             the state this item was opened in",
        );
        // ⚠⚠ THE PREMISE OF THE COUNTER-EXAMPLE, MEASURED RATHER THAN ASSERTED: the links are
        // still there and one of them is still unread. Without this the `0` above would be
        // satisfied by a ledger that simply had no links, and the mutation it is meant to catch
        // — a denominator taken from the ledger — would walk through.
        let links = reading.chain(902).expect("902's chain is still walkable");
        assert_eq!(links.len(), 2, "{links:?}");
        assert_eq!(
            links.iter().filter(|l| l.says == Says::Neither).count(),
            1,
            "{links:?}",
        );
    }

    // ⛔⛔⛔⛔⛔ ────────────── REGISTER ITEM 934: A COUNT NOBODY CAN OPEN ──────────────
    //
    // The four ratcheted numbers were printed as `.len()` over lists that were then dropped, so
    // the largest population this ledger holds — 330 unclassified items, four times the standing
    // `population` — could be READ and not ASKED ABOUT. The mutation these tests exist to fail
    // under is item 934's own third clause: *erase or truncate the list and the suite must go
    // red*, which is why every assertion below COUNTS the numbers a line names rather than
    // checking that it says something.

    /// The item numbers a rendered line actually names, read the way a person reads it: whatever
    /// follows the last `": "`, up to the first token that is not a number.
    ///
    /// ⚠⚠ NOT `line.contains("934")`. Every one of these messages carries counts, floors and item
    /// numbers in its prose, so a `contains` would pass against a line that named no item at all —
    /// which is the exact defect being paid off.
    fn numbers_named_by(line: &str) -> Vec<u32> {
        let Some((_, tail)) = line.rsplit_once(": ") else {
            return Vec::new();
        };
        tail.split_whitespace()
            .map_while(|word| word.parse().ok())
            .collect()
    }

    /// 🎯🎯🎯🎯🎯 **A BACKLOG LINE NAMES THE ITEMS IT COUNTED** — register item 934(1).
    #[test]
    fn a_backlog_line_names_the_items_and_not_only_how_many() {
        let backlogs = read(LEDGER).backlogs();
        let Backlogs {
            unclassified,
            unranked,
            unrooted,
            paid_unnamed,
        } = &backlogs;
        // ⚠ `unranked` is EMPTY in this fixture and is asserted about separately below: a line
        // with nothing to name must name nothing, and folding it in here would either weaken this
        // assertion to *0 or more* or make an empty backlog print a phantom.
        assert!(unranked.items.is_empty(), "{unranked:?}");
        for backlog in [unclassified, unrooted, paid_unnamed] {
            let line = backlog.to_string();
            let named = numbers_named_by(&line);
            assert_eq!(
                named,
                backlog.items,
                "⛔⛔⛔⛔⛔ REGISTER ITEM 934: `{}` counted {} item(s) and its line names {}. A \
                 number nobody can open is the whole of that item — the line is what a round reads \
                 to pick one of these up. Line: {line}",
                backlog.label,
                backlog.items.len(),
                named.len(),
            );
        }
        assert_eq!(
            numbers_named_by(&unranked.to_string()),
            Vec::<u32>::new(),
            "an empty backlog names nothing: {}",
            unranked,
        );
    }

    /// 🎯🎯🎯🎯🎯 **AND WHEN IT CANNOT NAME THEM ALL, IT NAMES SOME AND SAYS HOW MANY IT HID** —
    /// register item 934(1)'s bound, which that item left for this round to choose. See
    /// [`NAMED_IN_A_LINE`] for the measurement it was chosen on.
    ///
    /// # ⛔⛔⛔ The two mutations this is built to fail under
    ///
    /// *Naming none* and *naming all of them* are both refused here, and they are opposite
    /// failures: the first is item 934 returning, and the second is the 1,300-byte line that made
    /// printing the list look impossible in the first place.
    #[test]
    fn a_backlog_too_long_for_one_line_names_some_and_says_how_many_it_hid() {
        let many: Vec<u32> = (100..=499).collect();
        let backlog = Backlog {
            label: "unclassified",
            token: DECLARATION,
            items: many.clone(),
            declared: Some(many.len()),
            reckoning: Reckoning::Counted,
        };
        let line = backlog.to_string();
        let named = numbers_named_by(&line);

        assert!(
            !named.is_empty(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 934: a line that names NO item is the defect this pays off, \
             and a bound is not a licence to bring it back: {line}",
        );
        assert!(
            named.len() < many.len(),
            "the bound did not bite on a 400-item backlog, so it would not bite on the real \
             ledger's 382 either: {line}",
        );
        assert_eq!(
            named,
            many[..named.len()],
            "a report line shows the LOWEST numbers — this ledger's rule 13 is that old debt \
             sinks, so the end handed to a reader is the end that sinks: {line}",
        );
        // ⚠⚠ THE HIDDEN COUNT IS ASSERTED AGAINST THE ARITHMETIC, not merely present. A tail
        // reading `and 0 more` would be a line that stopped saying anything while still looking
        // like it did.
        assert!(
            line.ends_with(&format!(
                "… and {} more, all higher",
                many.len() - named.len()
            )),
            "a reader must be able to tell a short set from a cut one, and by how much: {line}",
        );
        assert!(
            line.len() < 341,
            "⚠ 341 bytes is what the `population` line spends on the real ledger (measured \
             2026-09-07) — a backlog line is an annotation on this report and must not become its \
             longest: {} bytes, {line}",
            line.len(),
        );
    }

    /// Whether a fault carries the SET it counted, said **exhaustively** — register item 934(2).
    ///
    /// ⚠⚠ NO `_` ARM, which is this workspace's rule 6 wearing its usual shape: a twenty-fifth
    /// fault added tomorrow cannot slip past by being unlisted. Its author has to say here whether
    /// it names items, and if it does, the test below starts holding it to naming them.
    fn set_carried_by(fault: &Fault) -> Option<&[u32]> {
        match fault {
            Fault::RatchetGrew { counted, .. }
            | Fault::SeverityRatchetGrew { counted, .. }
            | Fault::ParentRatchetGrew { counted, .. }
            | Fault::PaidRatchetGrew { counted, .. }
            | Fault::RatchetSlack { counted, .. } => Some(counted),
            // Everything else is about ONE line or ONE item, which its own message already names.
            Fault::UnknownTag { .. }
            | Fault::ConflictingTags { .. }
            | Fault::UntaggedCandidate { .. }
            | Fault::UnknownSeverity { .. }
            | Fault::ConflictingSeverities { .. }
            | Fault::SeverityDeclaration { .. }
            | Fault::UnreadableSeverityDeclaration { .. }
            | Fault::UnknownParent { .. }
            | Fault::MetWhileNotMade { .. }
            | Fault::DeferredByUnreadLink { .. }
            | Fault::UnrunnableRed { .. }
            | Fault::DanglingParent { .. }
            | Fault::ParentCycle { .. }
            | Fault::PaidDeclaration { .. }
            | Fault::UnreadablePaidDeclaration { .. }
            | Fault::ParentDeclaration { .. }
            | Fault::UnreadableParentDeclaration { .. }
            | Fault::Declaration { .. }
            | Fault::UnreadableDeclaration { .. }
            // ⚠ Register item 937's fault names ONE backlog and ONE item, both in its message —
            // and this arm is the gate above working: the variant could not be added without its
            // author saying here whether it carries a set.
            | Fault::BacklogOwnerClosed { .. } => None,
        }
    }

    /// 🎯🎯🎯🎯🎯 **A RATCHET THAT MOVED NAMES THE ITEMS IT COUNTED** — register item 934(2).
    ///
    /// `331 unmarked items, but the ledger declares 330` sent its reader to a ledger of five
    /// hundred blocks with nothing to search for. **A floor is a scalar, so no reading can diff a
    /// set against it** — the set it counted is the only thing this can hand over, and the four
    /// `…Grew` messages say which end of it is the likely one rather than implying certainty.
    #[test]
    fn every_fault_that_carries_a_set_names_items_in_its_message() {
        let counted: Vec<u32> = (100..=499).collect();
        let faults = [
            Fault::RatchetGrew {
                counted: counted.clone(),
                declared: 1,
            },
            Fault::SeverityRatchetGrew {
                counted: counted.clone(),
                declared: 1,
            },
            Fault::ParentRatchetGrew {
                counted: counted.clone(),
                declared: 1,
            },
            Fault::PaidRatchetGrew {
                counted: counted.clone(),
                declared: 1,
            },
            Fault::RatchetSlack {
                token: DECLARATION,
                counted: counted.clone(),
                declared: counted.len() + 1,
            },
        ];
        for fault in &faults {
            let carried = set_carried_by(fault).expect("these five carry a set by construction");
            assert_eq!(carried, counted, "{fault:?}");
            let said = fault.to_string();
            let named = numbers_named_by(&said);
            assert!(
                !named.is_empty(),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 934(2): this fault reports that a backlog moved and \
                 hands its reader no item to open. The count already told them a number moved; \
                 what they cannot do is find it: {said}",
            );
            assert!(
                named.iter().all(|number| counted.contains(number)),
                "every number named must be one that was counted: {named:?} in {said}",
            );
        }
        // ⚠ The `…Grew` four show the NEW end and slack shows the OLD one — see `Ends`. Asserted
        // because the two directions are the reason the ends differ, and a refactor that made them
        // one would silently stop pointing at the item a growth most likely added.
        let grown = faults[0].to_string();
        assert_eq!(
            *numbers_named_by(&grown).last().expect("named some"),
            *counted.last().expect("non-empty"),
            "a growth is `max+1`, so the newest item must be in the window: {grown}",
        );
        let slack = faults[4].to_string();
        assert_eq!(
            numbers_named_by(&slack).first(),
            counted.first(),
            "nothing was added under slack, so the end worth naming is the one that sinks: {slack}",
        );
    }

    // ⛔⛔⛔⛔⛔ ───── REGISTER ITEM 936: A LEDGER THAT IS GREEN, FINISHED, AND STILL OWED ─────
    //
    // The end state, as a fixture: everything marked has been paid, and one block was never read.
    // On this the instrument printed `population 0`, `critical 0`, `rc=0` and an EMPTY stderr —
    // and `--admits` offered nothing at all. A run standing here could neither finish honestly nor
    // move. These tests hold both halves of that: the ending must not read as reached, and the
    // block must become the thing to take.
    const AT_THE_END: &str = "\
# Ledger
## A. THE SHARPEST THINGS OPEN
@ns-unclassified: 1
@sev-unclassified: 0
@from-unclassified: 2
@paid-uncommitted: 1

900. Everything the loop ever owed, paid
     @ns: paid

899. A block nobody ever classified
     no mark of any kind
";

    /// 🎯🎯🎯🎯🎯 **AN UNREAD BLOCK IS NOT AN ENDING** — register item 936(3), and the mutation
    /// that item asks for by name: *make one unclassified, ask the ending, and `finished` is red.*
    #[test]
    fn the_ending_is_not_reached_while_one_block_was_never_classified() {
        let reading = read(AT_THE_END);
        // ⚠⚠ THE PREMISE FIRST, because without it this test could pass on a fixture that simply
        // had work left: what makes it item 936's case is that everything MARKED is done and the
        // instrument is otherwise perfectly green.
        assert!(
            reading.population().is_empty(),
            "{:?}",
            reading.population()
        );
        assert!(reading.critical().is_empty(), "{:?}", reading.critical());
        assert!(
            reading.is_green(),
            "⚠ the premise: every floor matches its count, so nothing here is a FAULT — which is \
             exactly why `is_green` could not have answered this: {:?}",
            reading.faults,
        );

        let ending = reading.ending();
        assert!(
            !ending.reached(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 936: nothing is open and the register is green, but item 899 \
             has never been ASKED whether it is this loop's. Reading that as finished is the claim \
             register item 823 spent itself replacing — *nothing marked is owed* standing in for \
             *nothing is owed*. Said: {ending}",
        );
        assert_eq!(ending.unclassified, vec![899], "{ending:?}");
        assert!(
            ending.to_string().contains("open 0") && ending.to_string().contains("unclassified 1"),
            "⚠ register item 914: the line must name the population it judged, on both verdicts — \
             a reader must never have to know which of the five backlogs it meant: {ending}",
        );
    }

    /// ⛔⛔⛔⛔ **AND THE TWO THAT CANNOT FALL BY PAYING ARE NOT IN IT** — working rule 5, asked of
    /// the ending's own population. See [`Ending`] for the table this asserts.
    ///
    /// `unrooted` counts PAID items too, so paying never moves it; putting it in the ending would
    /// make the north star unreachable rather than honest. `paid-uncommitted` is real work that
    /// says nothing about whether anything is owed.
    #[test]
    fn the_ending_counts_only_what_paying_can_bring_to_zero() {
        let done = AT_THE_END.replace(
            "899. A block nobody ever classified\n     no mark of any kind\n",
            "899. A block that WAS classified\n     @ns: out — a rendering defect\n",
        );
        let lowered = done.replace("@ns-unclassified: 1", "@ns-unclassified: 0");
        let reading = read(&lowered);
        assert!(
            reading.is_green(),
            "the fixture must be green for this to say anything: {:?}",
            reading.faults,
        );
        let backlogs = reading.backlogs();
        // ⚠ The premise: these two are still standing, and the ending must be REACHED anyway.
        assert_eq!(backlogs.unrooted.items, vec![899, 900], "{backlogs:?}");
        assert_eq!(backlogs.paid_unnamed.items, vec![900], "{backlogs:?}");
        assert!(
            reading.ending().reached(),
            "⛔ WORKING RULE 5: `unrooted` counts paid items, so no amount of paying brings it to \
             0 — an ending that waited for it would be a condition with no path. Said: {}",
            reading.ending(),
        );
        // ⚠⚠ AND `unranked` IS SUBSUMED RATHER THAN DROPPED: it is a subset of the population, so
        // an empty population forces it empty. Asserted so the omission is a proof and not a
        // preference — the difference register item 924 is about.
        assert!(
            backlogs.unranked.items.is_empty(),
            "an empty population must force the severity backlog empty, which is why the ending \
             does not name it separately: {backlogs:?}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **AND THE UNREAD BLOCK BECOMES THE THING TO TAKE** — register item 936(1). The
    /// ending refusing to close is only half: a run told *not finished* with nothing admissible is
    /// the `unadmitted` stall, which is the failure this repository has already watched happen.
    #[test]
    fn when_nothing_marked_is_takeable_the_unclassified_become_so() {
        let reading = read(AT_THE_END);
        assert!(
            reading.takeable(1).is_empty(),
            "the premise: nothing marked is takeable: {:?}",
            reading.takeable(1),
        );
        assert_eq!(
            reading.admits(1, &[]),
            vec![899],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 936: with nothing marked to take, the register must offer the \
             block nobody read — otherwise it says a debt stands and that no round may take it, \
             and the run stalls with `unadmitted`. Working rule 6: an unstated case is not a pass",
        );
    }

    /// 🎯🎯🎯🎯🎯 **EVERY RATCHETED BACKLOG IS JUDGED, AND THE EXEMPTIONS ARE COUNTED** —
    /// register item 937.
    ///
    /// # ⛔⛔⛔⛔⛔ The three checks, and why a bare `match` would not be one
    ///
    /// [`Reckoning`] has no `Unknown` arm, so a fifth backlog cannot be added without its author
    /// writing a disposition — but a written disposition is a CLAIM, and item 937 exists because
    /// three of four backlogs had claims nobody had ever checked. So each arm is checked against
    /// something:
    ///
    /// * `Counted` — [`Ending`] must actually count it. A backlog that says the ending waits for
    ///   it while the ending has never heard of it is register item 936 returning.
    /// * `Owned(n)` — `n` must be an OPEN item. Checked on every reading, not here: see
    ///   [`Fault::BacklogOwnerClosed`], because only a reading of the ledger can answer it.
    /// * `Exempt(why)` — the sentence must be there, **and the number standing on this arm is
    ///   asserted**. That last clause is the whole of working rule 6 here: an exemption list
    ///   nobody counts is how a gate is switched off one honest-looking sentence at a time.
    #[test]
    fn every_backlog_is_judged_and_the_exemptions_are_counted() {
        let reading = read(LEDGER);
        let backlogs = reading.backlogs();
        let ending = reading.ending();

        // ⚠ THROUGH `each`, not a list written here — see its doc. A judgement counted by a
        // hand-listed array is one a fifth backlog joins only if somebody remembers.
        let mut exempt: Vec<&'static str> = Vec::new();
        for backlog in backlogs.each() {
            match &backlog.reckoning {
                Reckoning::Counted => assert_eq!(
                    backlog.items, ending.unclassified,
                    "⛔⛔⛔⛔⛔ REGISTER ITEM 937: `{}` says the ending counts it, and the ending \
                     counts something else. A disposition nothing checks is the claim item 937 was \
                     opened by",
                    backlog.label,
                ),
                // ⚠ The owner is checked by `read` against the ledger's own population — a unit
                // test cannot, because the fixture is not the ledger the claim is about.
                Reckoning::Owned(owner) => assert!(
                    *owner > 0,
                    "`{}` names item {owner} as its owner",
                    backlog.label,
                ),
                Reckoning::Exempt(why) => {
                    assert!(
                        !why.trim().is_empty(),
                        "⛔ `{}` is exempt for no stated reason, which is the escape hatch working \
                         rule 6 refuses",
                        backlog.label,
                    );
                    exempt.push(why);
                }
            }
        }
        // ⛔⛔⛔⛔⛔ AND THE COUNT — register item 903's shape, which this ledger has paid for
        // twice: *an exemption is written as a sentence and its NUMBER is asserted*. A third
        // exemption cannot be added quietly; whoever adds one has to come here and say so.
        assert_eq!(
            exempt.len(),
            2,
            "⛔⛔⛔⛔⛔ REGISTER ITEM 937: exactly two backlogs stand on an exemption today — \
             `unranked` (a subset of the population) and `unrooted` (a ratchet on new items, which \
             counts paid items so paying never lowers it). A third means a backlog stopped being \
             owned or counted, and that is the defect item 937 is: {exempt:?}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND AN OWNER THAT HAS BEEN PAID IS A RED** — register item 937, the one arm a
    /// document can take away after the fact.
    #[test]
    fn a_backlog_whose_owner_left_the_population_is_red() {
        // ⚠ ASKED OF THE READING, not read off `faults` — see `backlog_owners_gone`, which records
        // the eleven tests that went red when this lived inside `read`.
        let reading = read(LEDGER);
        assert!(
            reading
                .backlog_owners_gone()
                .contains(&Fault::BacklogOwnerClosed {
                    token: PAID_DECLARATION,
                    owner: 938,
                }),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 937: a backlog naming an owner that is not open has no owner \
             at all, and nothing said so. The remedy is to judge it again — a new owner, or an \
             exemption with its sentence: {:?}",
            reading.backlog_owners_gone(),
        );
        // ⚠ AND THE MESSAGE SAYS WHAT TO DO, which is register item 926's rule for every refusal
        // in this file: a reader told only *the owner is gone* has to re-derive the two remedies.
        let said = Fault::BacklogOwnerClosed {
            token: PAID_DECLARATION,
            owner: 938,
        }
        .to_string();
        assert!(
            said.contains("name an open owner, or write why it needs none"),
            "the refusal must name both remedies: {said}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **THE REMEDY NAMES EVERY EDIT ITS OWN GATES DEMAND** — register item 936(1).
    ///
    /// A procedure that leaves one clause out is worse than none: a round follows it, goes red on
    /// the ratchet it was not told about, and learns to distrust the sentence. Each token below is
    /// the one a measured omission reds on — see [`CLASSIFY_REMEDY`]'s table.
    #[test]
    fn the_remedy_names_each_edit_and_marks_no_verdict() {
        for token in [TAG, DECLARATION, SEVERITY, "commit id"] {
            assert!(
                CLASSIFY_REMEDY.contains(token),
                "⛔⛔⛔⛔ REGISTER ITEM 936: the remedy omits `{token}`, so a round that follows it \
                 goes red on a gate it was not told about: {CLASSIFY_REMEDY}",
            );
        }
        // ⛔⛔⛔⛔⛔ AND IT MARKS NO VERDICT — the constraint `CLASSIFY_REMEDY`'s doc states, held
        // here rather than trusted: this is the only free prose in an `--admits` reply, and
        // `judge` reads a verdict from the first word that OPENS the reply or is ALL CAPITALS.
        let marked: Vec<&str> = CLASSIFY_REMEDY
            .split_whitespace()
            .map(|word| word.trim_matches(|c: char| !c.is_ascii_alphabetic()))
            .filter(|word| {
                !word.is_empty()
                    && word.chars().all(|c| c.is_ascii_uppercase())
                    && matches!(word.to_ascii_uppercase().as_str(), "YES" | "NO")
            })
            .collect();
        assert!(
            marked.is_empty(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 743's defect, one road over: a capitalised verdict word in \
             this sentence can be read AS the verdict. Found {marked:?} in: {CLASSIFY_REMEDY}",
        );
    }

    /// 🎯🎯🎯🎯🎯 **AN EMPTY ADMISSIBLE SET NOW MEANS *FINISHED* AND NEVER *STUCK*** — register
    /// item 936, and the single statement the whole item buys.
    ///
    /// # ⛔⛔⛔⛔⛔ The two were the same shape, and a run walked into the wrong one
    ///
    /// Before this, `--admits` answering with nothing had two causes a reader could not separate:
    /// *everything is paid* and *the register holds debts none of which any round may take*. The
    /// second is what run 248 hit — it stopped `unadmitted` — and it is the reason this item was
    /// opened rather than a tidier one.
    ///
    /// The biconditional below makes them one question again, provably:
    ///
    /// * NOT reached ⇒ `population` or `unclassified` is non-empty. A non-empty population always
    ///   has a takeable member (an item's depth counts the ancestors STILL OWED, chains are
    ///   finite, and unknown depth is takeable — so the topmost owed item of any chain qualifies).
    ///   A non-empty `unclassified` with an empty population is exactly the third tier. **Either
    ///   way the set is non-empty.**
    /// * Reached ⇒ both are empty ⇒ every tier is empty.
    ///
    /// ⚠⚠ Asserted over every fixture in this file that carries a distinct shape, because a
    /// biconditional shown on one ledger is an anecdote. The shapes are: marked work standing,
    /// marked work with the cap biting, nothing but unread blocks, and nothing at all.
    #[test]
    fn an_empty_admissible_set_means_finished_and_never_stuck() {
        let finished = AT_THE_END.replace(
            "899. A block nobody ever classified\n     no mark of any kind\n",
            "899. A block that was read and answered\n     @ns: out — a rendering defect\n",
        );
        let finished = finished.replace("@ns-unclassified: 1", "@ns-unclassified: 0");
        for (name, text, reached) in [
            ("marked work standing", LEDGER, false),
            ("only unread blocks left", AT_THE_END, false),
            ("nothing left at all", finished.as_str(), true),
        ] {
            let reading = read(text);
            let ending = reading.ending();
            assert_eq!(
                ending.reached(),
                reached,
                "the fixture `{name}` does not have the shape this case is about: {ending}",
            );
            assert_eq!(
                reading.admits(1, &[]).is_empty(),
                ending.reached(),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 936 on `{name}`: an empty admissible set and a reached \
                 north star must be the SAME fact. Where they come apart, a run is either told to \
                 stop with work standing, or told to carry on with nothing it may take — which is \
                 the `unadmitted` stall. Said: {ending}; may take: {:?}",
                reading.admits(1, &[]),
            );
        }
    }

    /// ⛔⛔⛔⛔ **AND IT IS A THIRD TIER, NOT A BLEND** — the guard on the arm above. On the real
    /// ledger the unclassified outnumber the population four to one; admitting them beside marked
    /// work would drown it, which is *widen it until it is green* wearing the opposite sign.
    #[test]
    /// ⚠⚠⚠ **AND IT IS THE FALL-THROUGH TIER THAT IS TESTED, NOT THE CRITICAL ONE.** Written first
    /// against `LEDGER` unchanged, this was VACUOUS: that fixture's item 900 is `@sev: critical`,
    /// so the FIRST tier answered and a blended third tier never ran. The mutation *blend instead
    /// of tier* was caught only by two unrelated tests while this one — the guard written for it —
    /// stayed green. So the severity is dropped here on purpose: nothing critical, marked work
    /// still standing, which is the only shape that puts the second and third tiers side by side.
    fn the_unclassified_are_not_admitted_while_marked_work_stands() {
        // ⚠⚠⚠ AND THE OTHER WAY `takeable` CAN EMPTY, which this test did not reach until it was
        // asked how: marked work that stands but is entirely HELD BACK BY THE CAP. Measured before
        // it was asserted — `population 3`, `deferred 1`, and the unclassified block still refused
        // with *"What a round may take: 899 900"*.
        //
        // ⭐ It cannot in fact happen, and that is the tighter statement this arm pins: an item's
        // depth counts the ancestors STILL OWED, chains are finite, and a cycle or a dangling
        // parent reads as unknown depth — which `takeable` admits. So the topmost owed item of any
        // chain always has zero owed above it and is takeable. **`takeable` empties only when
        // `population` does**, which makes the third tier fire at exactly one moment rather than
        // whenever the cap happens to bite.
        let held = "\
# Ledger
## A. THE SHARPEST THINGS OPEN
@ns-unclassified: 1
@sev-unclassified: 0
@from-unclassified: 1
@paid-uncommitted: 0

900. The root debt, owed
     @ns: open
     @sev: ordinary
     @from: none

899. Found while paying 900
     @ns: open
     @sev: ordinary
     @from: 900 — 900 을 갚으며 «만들었다»

898. Found while paying 899
     @ns: open
     @sev: ordinary
     @from: 899 — 899 를 갚으며 «만들었다»

897. A block nobody ever classified
     no mark of any kind
";
        let deep = read(held);
        assert_eq!(deep.deferred(1), vec![898], "the premise: the cap bites");
        assert_eq!(
            deep.backlogs().unclassified.items,
            vec![897],
            "the premise: an unclassified block is standing beside it",
        );
        assert_eq!(
            deep.admits(1, &[]),
            vec![899, 900],
            "⛔⛔⛔⛔ REGISTER ITEM 936: the cap holding one item back is not the register running \
             out of marked work. A tier that opened here would offer unread blocks while a debt \
             the loop can reach is sitting in front of it: {:?}",
            deep.takeable(1),
        );

        let ordinary = LEDGER.replace(
            "     @sev: critical — it stops the loop dead",
            "     @sev: ordinary — it does not stop the loop",
        );
        assert_ne!(
            ordinary, LEDGER,
            "the mutation of the fixture changed nothing"
        );
        let reading = read(&ordinary);
        assert!(
            reading.critical().is_empty(),
            "the premise: nothing critical, so the SECOND tier is the one answering: {:?}",
            reading.critical(),
        );
        let unclassified = reading.backlogs().unclassified.items;
        assert_eq!(unclassified, vec![897], "the premise: {unclassified:?}");
        assert!(
            !reading.takeable(9).is_empty(),
            "the premise: marked work stands: {:?}",
            reading.takeable(9),
        );
        assert_eq!(
            reading.admits(9, &[]),
            vec![900],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 936: the third tier is a TIER and not a blend. On the real \
             ledger the unclassified outnumber the population four to one, so admitting them \
             beside marked work would drown it — *widen it until it is green* wearing the \
             opposite sign",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND THE PRINTED LINE IS THE ONE THESE TESTS JUDGE** — register item 934(3).
    ///
    /// # ⛔⛔⛔ Why a source gate and not another assertion
    ///
    /// The tests above judge [`Backlog`]'s rendering; the lines a round actually reads are printed
    /// by `bin/north-star.rs`. While that binary formatted its own four lines, every assertion
    /// here was about a string nothing printed — a gate green for a population of one, which is
    /// register item 914's finding. So the binary is held to printing THESE lines: it may not
    /// carry a format string that opens with one of the four labels.
    ///
    /// ⚠⚠ Both halves are needed and neither is enough. *Prints the backlog* alone would pass a
    /// binary that printed it and then a hand-rolled copy; *formats no label* alone would pass a
    /// binary that had simply stopped printing them.
    #[test]
    fn the_binary_prints_these_lines_rather_than_formatting_its_own() {
        const BIN: &str = include_str!("bin/north-star.rs");
        let Backlogs {
            unclassified,
            unranked,
            unrooted,
            paid_unnamed,
        } = read(LEDGER).backlogs();
        assert!(
            BIN.contains("reading.backlogs()"),
            "⛔ the binary no longer asks for the backlogs at all",
        );
        // 🎯 AND THE ENDING — register item 936, held by the same gate for the same reason: the
        // `Ending` tests above judge a string, and only this says the string is the one printed.
        assert!(
            BIN.contains("println!(\"{}\", reading.ending())"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 936: the report no longer prints whether the north star is \
             reached, so the only mechanical answer to that question is one nobody sees — and the \
             ending goes back to being a sentence an agent judges by eye",
        );
        // 🎯 AND THE REMEDY, at the one moment an unread block is handed to a round — register
        // item 936(1). Held here because the test above judges the CONSTANT, and only this says
        // the constant is what reaches a reader.
        // ⚠⚠⚠ THE PRINT AND NOT THE MENTION. Written first as `contains("CLASSIFY_REMEDY")`,
        // this passed against a binary whose line had been replaced by `let _ = …CLASSIFY_REMEDY;`
        // — the constant was still named and nothing reached a reader. A gate that a mutation
        // walks through is the hole, not the mutation.
        assert!(
            BIN.contains("println!(\"  {}\", north_star::CLASSIFY_REMEDY)"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 936(1): the reply that hands over an unread block no longer \
             says what to do with it, so the procedure is back to living in a register entry a \
             hundred and sixty rounds away from the round that needs it",
        );
        // ⛔⛔⛔⛔⛔ AND THE SCREENING'S DENOMINATOR — register item 924, held by both halves for
        // the reason this test's own doc gives. `prints it` alone would pass a binary that printed
        // it and a hand-rolled copy; `formats no label` alone would pass a binary that had gone
        // back to printing nothing, which is the exact state the item was opened in.
        let screened = read(LEDGER).deferred_unread(1);
        assert!(
            BIN.contains("println!(\"{unread}\")"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 924: the report no longer says how many links this gate put \
             its question to, so a run that examined nothing and a run that examined some and \
             found them clean go back to reading identically — and the assertions above are about \
             a string nobody sees",
        );
        assert!(
            !BIN.contains(&format!("\"{} ", screened.label)),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 924: the binary formats a line opening with `{}` itself. Two \
             authors for one line is how the printed count and the walked count come to disagree \
             — the line belongs to `Screening`, for register item 934's reason one gate over",
            screened.label,
        );
        for (backlog, field) in [
            (&unclassified, "backlogs.unclassified"),
            (&unranked, "backlogs.unranked"),
            (&unrooted, "backlogs.unrooted"),
            (&paid_unnamed, "backlogs.paid_unnamed"),
        ] {
            assert!(
                BIN.contains(&format!("println!(\"{{}}\", {field})")),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 934(3): the report line for `{}` is not printed from \
                 `{field}`, so the assertions in this file are about a string nobody reads",
                backlog.label,
            );
            assert!(
                !BIN.contains(&format!("\"{} ", backlog.label)),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 934(3): the binary formats a line opening with `{}` \
                 itself. Two authors for one line is how the printed number and the ratcheted \
                 number come to disagree — the line belongs to `Backlog`",
                backlog.label,
            );
        }
    }
}
