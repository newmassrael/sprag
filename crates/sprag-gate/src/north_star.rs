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

    let mut items: Vec<Item> = Vec::new();
    for (number, bodies) in &blocks {
        let mut tag = None;
        let mut severity = None;
        for body in bodies {
            let mut in_block: Vec<Tag> = Vec::new();
            let mut severities: Vec<Severity> = Vec::new();
            for line in body {
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

    Reading {
        items,
        declared,
        severity_declared,
        faults,
    }
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

900. ⛔ **An open loop item**
     @ns: open — the loop's own driver
     @sev: critical — it stops the loop dead
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
}
