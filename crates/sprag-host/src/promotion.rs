//! ⛔⛔⛔⛔⛔ **WHETHER THIS REPOSITORY MAY PUT ITS FIXES IN FRONT OF A PERSON RIGHT NOW** —
//! register item 868, and the three conditions nothing measured in one place.
//!
//! # ⛔⛔⛔⛔⛔ What it cost to hold them in a head
//!
//! A promotion is the ONLY way this repository's own fixes reach the loop that pays its debts, and
//! it needs three things true at once. Item 868 was opened the day a window actually opened —
//! another repository's watcher signalled that its push had landed and its next round had not yet
//! touched the tree — and **this side was not ready, so the window was missed**. Its own
//! measurement that day: condition ⑴ held, ⑵ and ⑶ did not, and the window was *몇 분*.
//!
//! ⇒ And the cost is not only a missed window. Item 868 records another watcher's own words about
//! what the closed door did to their diagnosis:
//!
//! > *"저는 그 10 커밋이 있는 줄 몰랐고, 그동안 제 런이 두 번 그 문으로 죽었습니다. 「고쳤는데
//! > 안 실렸다」가 제 창에서는 **「제품이 못 고치는 결함」**으로 보였습니다."*
//!
//! **When nobody can see what is behind the door, *unpaid* and *unshipped* are the same shape from
//! outside** — and a watcher then writes the wrong cause into a register.
//!
//! # 📊 Measured 2026-09-05, and the number has grown since the item was filed
//!
//! ```text
//! git log --oneline 7181c74..HEAD | wc -l   → 19
//! git status --porcelain | wc -l            → 0
//! ~/.local/share/sprag-loop/bin/sprag --version → sprag 0.0.1 (7181c7483168)
//! ```
//!
//! Item 868 was opened when that first number was **10** and wrote *오늘 그 수가 10 이 될 때까지
//! 아무것도 안 울었다*. It is **19** now and still nothing rings — four of them are this session's
//! own payments for items 891, 895, 893 and 896.
//!
//! # ⭐ WHY THE BINARY'S OWN WORD AND NEVER ITS MTIME
//!
//! Item 868's ⑶ prescribed *바이너리 mtime vs HEAD 커밋 시각*. **That prescription is refuted by
//! this repository's own north star**, which says to check a binary by a symbol unique to the
//! newest fix *rather than by its mtime* — an mtime moves when nothing changed and stands still
//! when a file is copied. And no symbol grep is needed either for a binary that will answer: it
//! states its build, so the condition is a string comparison against `HEAD` with nothing inferred.
//!
//! ⛔⛔⛔⛔⛔ **BUT ONLY ONE OF THE FOUR ANSWERS, AND AN EARLIER DRAFT OF THIS PARAGRAPH SAID
//! *every one of these binaries states its build*.** This tool's own first output refuted it in
//! the hour it was written — `sprag-term` reads `--version` as a command to spawn, `sprag-gui`
//! and `sprag-mcp` print nothing, so **three of the four cannot be asked at all** (register item
//! 897). That is why [`all_of`] has a third arm rather than two: the sentence a module writes
//! about its own subject is unmeasured until the module is run against it, which is this
//! workspace's rule 10 caught inside the instrument built to end a different silence.
//!
//! # ⛔⛔⛔⛔⛔ AND ONE CONDITION ONLY A PERSON CAN ANSWER, WHICH MUST NOT READ AS *MET*
//!
//! [`Condition::AWindowSomebodyElseOpened`] is about ANOTHER repository's live run, and this
//! process cannot see it — item 865 built `sprag my-runs` so a person can be asked. An instrument
//! that treated *nobody told me* as *met* would answer **promote now** at the one moment the
//! window is shut, which is the failure that opened item 868. So it is its own answer and it holds
//! the verdict back: this workspace's rule 6, where an unclassified case is stated and never
//! glossed.

use std::fmt;

/// ⛔⛔⛔⛔⛔ **THE IMAGES A PROMOTION MOVES** — register item 868's ⑶ counts FOUR, so a promotion
/// that moved three would leave a daemon serving one image and a CLI another.
///
/// # ⚠⚠ Why the list is here and not at each reader
///
/// It had two readers the moment item 897 was taken — the door that checks them and the gate that
/// holds them to saying so — and a second spelling is how a fifth binary gets built, promoted and
/// never asked. `sprag_gate::sibling_bin` derives every path from one of these names, so a name
/// added here has to be a binary that exists or the gate says so.
pub const IMAGES: [&str; 4] = ["sprag", "sprag-term", "sprag-gui", "sprag-mcp"];

/// ⛔⛔⛔⛔⛔ **THE ONE SHAPE AN IMAGE SAYS ITS BUILD IN** — register item 897, and the sentence
/// three of these four could not say at all.
///
/// # 📊 What was measured, 2026-09-05T11:33Z, over the promoted copies
///
/// ```text
/// sprag       : sprag 0.0.1 (7181c7483168)
/// sprag-term  : Error: … PanePtyError { context: "spawn command" … }   ← reads the FLAG as a command
/// sprag-gui   : (nothing)
/// sprag-mcp   : (nothing)
/// ```
///
/// ⇒ [`Condition::BinariesThatSayHead`] asks all four and answers [`Answer::Unknowable`] for one that
/// cannot say, so **the verdict had no path to YES in any build of this product** — this
/// workspace's rule 5 read backwards, and the reason item 897 is what stands between the debt loop
/// and every clause waiting on a promotion (items 856 ⑴, 872 ⑶b, 894 ⑶, 895 ⑷).
///
/// # ⚠⚠ Why a function and not four `println!`s
///
/// [`said_build`] reads the build out of the PARENTHESES, so the shape is a contract between the
/// binaries and the door. Four spellings of a contract is three chances to drift, and the drift is
/// invisible: a binary that answered `sprag-gui 0.0.1 build 7181c74` would look right to a person
/// and be unreadable to the instrument.
///
/// ⚠ The version is [`env!`]`("CARGO_PKG_VERSION")` of THIS crate, which the workspace pins for
/// every member — so the four cannot disagree about the number either.
#[must_use]
pub fn version_line(image: &str) -> String {
    format!(
        "{image} {} ({})",
        env!("CARGO_PKG_VERSION"),
        crate::wire::BUILD
    )
}

/// **WHETHER THIS ARGUMENT ASKS AN IMAGE WHAT IT IS** — asked of the vocabulary rather than spelled
/// here, so `sprag --version` and `sprag-gui --version` cannot come to mean different flags.
///
/// ⚠ [`crate::vocabulary::Verb::Version`] already owns the spellings (`version`, `-V`, `--version`)
/// and a gate already holds that `-h`/`--help` and `-V`/`--version` parse to the same two verbs. A
/// second list here would be the *one question, two answers* shape this module's own `all_of` was
/// written against.
#[must_use]
pub fn asks_its_build(arg: &str) -> bool {
    crate::vocabulary::Verb::parse(arg) == Some(crate::vocabulary::Verb::Version)
}

/// **WHAT `binary --version` SAYS ITS BUILD IS** — the parenthesised hash of [`version_line`], or
/// why it could not be read.
///
/// # ⚠⚠ It is the DOOR's reader and the GATE's, which is the point
///
/// Register item 897's ⑷ is *the instrument can answer for all four*, and an instrument whose
/// parser lives in a binary can only be held by running that binary. Moved here so the gate that
/// makes the four SAY it uses the very reader that will ASK them — a shape they satisfy in the test
/// and not at the door would satisfy nothing.
///
/// ⚠ Read off the parentheses rather than by splitting on spaces: the leading words are the image's
/// name and number, and either may gain a word.
///
/// # Errors
///
/// The spawn failure, or a sentence saying the line carried no readable build.
pub fn said_build(binary: &std::path::Path) -> Result<String, String> {
    let out = std::process::Command::new(binary)
        .arg("--version")
        .output()
        .map_err(|error| error.to_string())?;
    let said = String::from_utf8_lossy(&out.stdout);
    let (_, after) = said.split_once('(').ok_or("no build in its version line")?;
    let (build, _) = after.split_once(')').ok_or("its build is not closed")?;
    Ok(build.trim().to_owned())
}

/// ⛔⛔⛔⛔⛔ **WHEN A CONDITION HAS TO BE TRUE** — register item 868's own correction, and the
/// half that was missing when it was filed.
///
/// The item wrote condition ⑵ as *내 트리가 깨끗하다* and then measured itself wrong: at the second
/// promotion the build was made on a clean tree and the tree was dirty by the time of the copy, and
/// **nothing was wrong** — the binary already held HEAD and later edits cannot reach it. Another
/// watcher corrected its own overreach the same hour, about its own side of the same window.
///
/// ⇒ ⭐ Both overreaches are one shape, in that watcher's words: *조건을 실제로 지키는 «순간»을 안
/// 갈라서 생긴 과잉.* A condition written without its moment holds on longer than it needs to, and
/// item 868's done-when ⑴ therefore asks for the moment beside each answer rather than a bare
/// yes/no.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Moment {
    /// **BEFORE EITHER OF THE OTHERS** — it is somebody else's run, and it is what makes the rest
    /// worth doing.
    BeforeBoth,
    /// **WHILE THE BUILD IS BEING MADE**, never at the copy: clean before, clean after, nothing
    /// edited in between. That is the corrected form of item 868's ⑵.
    WhileBuilding,
    /// **AT THE COPY** — the one condition that has to hold at the instant the binaries move.
    AtTheCopy,
}

impl Moment {
    /// The word it is reported under.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::BeforeBoth => "before both",
            Self::WhileBuilding => "while building",
            Self::AtTheCopy => "at the copy",
        }
    }
}

/// ⛔⛔⛔⛔⛔ **THE THREE CONDITIONS A PROMOTION NEEDS** — register item 868, as a closed set so a
/// fourth arrives with a row rather than being left out of a list somebody typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    /// **ANOTHER REPOSITORY'S LIVE RUN HAS A WINDOW** — its push has landed and its next round has
    /// not yet edited its tree. ⚠ This process cannot see it; see the module's last section.
    AWindowSomebodyElseOpened,
    /// **NOTHING WAS EDITED WHILE THE BUILD WAS BEING MADE** — [`Moment::WhileBuilding`], and the
    /// clause item 868 first wrote as a condition of the copy and then corrected.
    ATreeNothingEditedWhileBuilding,
    /// **THE BINARIES SAY THEY ARE HEAD** — read off each binary's own version line, never off an
    /// mtime.
    BinariesThatSayHead,
}

impl Condition {
    /// Every condition, in the order they have to become true.
    pub const ALL: [Self; 3] = [
        Self::AWindowSomebodyElseOpened,
        Self::ATreeNothingEditedWhileBuilding,
        Self::BinariesThatSayHead,
    ];

    /// The word it is reported under.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::AWindowSomebodyElseOpened => "a window somebody else opened",
            Self::ATreeNothingEditedWhileBuilding => "nothing edited while building",
            Self::BinariesThatSayHead => "binaries that say HEAD",
        }
    }

    /// ⭐ **WHEN IT HAS TO HOLD** — the half item 868 was missing. See [`Moment`].
    #[must_use]
    pub const fn moment(self) -> Moment {
        match self {
            Self::AWindowSomebodyElseOpened => Moment::BeforeBoth,
            Self::ATreeNothingEditedWhileBuilding => Moment::WhileBuilding,
            Self::BinariesThatSayHead => Moment::AtTheCopy,
        }
    }

    /// ⛔⛔⛔⛔⛔ **WHETHER A FINISHED RUN DECLINING TO RE-FIRE IS WHAT MAKES THIS TRUE** —
    /// register item 868's ⑶, and the whole of what separates the two claims on that moment.
    ///
    /// A finished run is the one instant this tree is quiet. Item 827 says fill it immediately —
    /// it measured **3 h 49 m** of a dead loop against two to three minutes elsewhere. Item 868
    /// says leave it empty — a build needs a tree nobody is editing, and re-firing is what closes
    /// the window. Both are true, and 868 recorded that *그 갈등이 지금 아무 데도 안 적혀 있다*.
    ///
    /// ⇒ This is the axis that settles it, and it is a property of each CONDITION rather than a
    /// preference between two items: **the only thing a re-fire can take away is a condition a
    /// stand-down would have bought.** A condition standing down cannot reach is one that holding
    /// the loop back does not move, so waiting on it is dead time item 827 already priced.
    ///
    /// ⚠ It is deliberately NOT [`Moment`] re-spelled. [`Moment::WhileBuilding`] is *when* the
    /// condition has to hold; this is *who can make it hold*. They agree on today's three and
    /// would part on a fourth — a condition true only at the copy could still be one a build
    /// produces — so a fourth condition has to answer both rather than have one inferred from the
    /// other.
    #[must_use]
    pub const fn mended_by_standing_down(self) -> bool {
        match self {
            // ⛔ ANOTHER REPOSITORY'S RUN. No amount of quiet here opens it, and a loop that
            // waited on it would wait for ever — item 865 built `sprag my-runs` because a PERSON
            // is the only channel this fact has.
            Self::AWindowSomebodyElseOpened => false,
            // ⭐ THIS IS THE ONE THE RE-FIRE DESTROYS. It is true exactly while no run is editing.
            Self::ATreeNothingEditedWhileBuilding => true,
            // ⭐ AND THIS IS WHAT THE QUIET IS FOR — the build and the copy the stand-down makes
            // room for are what put HEAD into the binaries.
            Self::BinariesThatSayHead => true,
        }
    }
}

/// What a measurement of one [`Condition`] came to.
///
/// ⚠⚠ **THREE ANSWERS AND NOT TWO.** [`Unknowable`](Self::Unknowable) is not a soft *no*: it is
/// *this process cannot see it and a person can*, and folding it into either neighbour is what
/// makes an instrument either miss every window or claim one that is shut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    /// Measured, and it holds.
    Met,
    /// Measured, and it does not — with what a reader has to change.
    Blocked(String),
    /// **NOT THIS PROCESS'S TO KNOW.** Ask a person; `sprag my-runs` is what item 865 built for it.
    Unknowable(String),
}

/// ⛔⛔⛔⛔⛔ **THE ONE ANSWER TO *MAY I PROMOTE NOW*** — register item 868's done-when ⑴ and ⑵:
/// one place, and a `no` that names what blocks it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Readiness {
    /// One answer per [`Condition::ALL`], in that array's order.
    answers: [Answer; Condition::ALL.len()],
}

impl Readiness {
    /// Build a reading from one answer per condition, in [`Condition::ALL`]'s order.
    #[must_use]
    pub const fn of(answers: [Answer; Condition::ALL.len()]) -> Self {
        Self { answers }
    }

    /// Each condition with what it came to and when it has to hold.
    pub fn rows(&self) -> impl Iterator<Item = (Condition, Moment, &Answer)> {
        Condition::ALL
            .into_iter()
            .zip(self.answers.iter())
            .map(|(condition, answer)| (condition, condition.moment(), answer))
    }

    /// ⛔⛔⛔⛔⛔ **WHETHER TO GO** — `true` only when every condition is [`Answer::Met`].
    ///
    /// ⚠⚠ An [`Answer::Unknowable`] is NOT a go. The window item 868 was filed over was somebody
    /// else's to see, and an instrument that read its own blindness as consent would say *promote*
    /// at exactly the wrong moment.
    #[must_use]
    pub fn may_promote(&self) -> bool {
        self.answers.iter().all(|answer| *answer == Answer::Met)
    }

    /// ⛔⛔⛔⛔⛔ **WHETHER THE RUN THAT JUST ENDED SHOULD BE RE-FIRED, OR THE LOOP SHOULD STAND
    /// DOWN SO A PROMOTION CAN HAPPEN** — register item 868's done-when ⑶, the clause that asks
    /// for *되걸기와 승격 중 무엇이 이기는지* to be settled by something a reader can ask rather
    /// than by two register entries pushing at each other.
    ///
    /// # ⛔⛔⛔⛔⛔ The judgement, and why it is not a preference
    ///
    /// It reads off [`Condition::mended_by_standing_down`], one row at a time:
    ///
    /// * **any condition a stand-down cannot mend is unmet ⇒ [`WhatFollowsAnEnding::ReFire`].**
    ///   Holding the loop back buys nothing that is not already in reach, so all it buys is the
    ///   dead time item 827 measured at 3 h 49 m. **Item 827 wins, and it wins by measurement.**
    /// * **otherwise, some condition a stand-down WOULD mend is unmet ⇒
    ///   [`WhatFollowsAnEnding::StandDown`].** The quiet a finished run leaves is exactly what
    ///   that condition needs and exactly what a re-fire destroys. **Item 868 wins.**
    /// * **every condition already holds ⇒ [`WhatFollowsAnEnding::ReFire`].** The copy does not
    ///   read this tree — item 868's own correction, measured at its second promotion — so there
    ///   is nothing for the loop to stay out of the way of.
    ///
    /// ⇒ ⭐⭐ Stated as one sentence: **a promotion wins the moment a finished run leaves, but
    /// only when the part nobody here controls is already true.** The loop never stands down on
    /// speculation, which is item 868's own finding that *창은 「기다리면 오는 것」이 아니라
    /// «만들어야 하는 것»이다* — a window is made by a person saying so, not by waiting.
    ///
    /// ⚠⚠ **IT DECIDES AND IT DOES NOT ACT.** Item 827 wrote *「자동으로 다시 걸어라」가 답이라고
    /// 미리 정하지 마라*, item 867 repeated it, and item 872's ⑵ made *`person` and `nothing` are
    /// never fired unattended* a gate. Nothing here fires anything: this says which of two
    /// register entries governs the instant, and the disposition of the ENDING still says whether
    /// that re-fire is one a machine may make at all. Two questions, two answers, and folding them
    /// would let this function authorise a re-fire item 872 forbids.
    #[must_use]
    pub fn what_follows_an_ending(&self) -> WhatFollowsAnEnding {
        let unmet = |mendable: bool| -> Vec<String> {
            self.rows()
                .filter(|(condition, _, _)| condition.mended_by_standing_down() == mendable)
                .filter_map(|(condition, _, answer)| match answer {
                    Answer::Met => None,
                    Answer::Blocked(why) | Answer::Unknowable(why) => {
                        Some(format!("{} ({why})", condition.word()))
                    }
                })
                .collect()
        };
        let out_of_reach = unmet(false);
        if !out_of_reach.is_empty() {
            return WhatFollowsAnEnding::ReFire(format!(
                "standing down would not mend {} — waiting on it is dead loop time",
                out_of_reach.join(", ")
            ));
        }
        let waiting = unmet(true);
        if waiting.is_empty() {
            return WhatFollowsAnEnding::ReFire(
                "every condition already holds and the copy does not read this tree, so a \
                 promotion needs no quiet"
                    .to_owned(),
            );
        }
        WhatFollowsAnEnding::StandDown(format!(
            "this quiet is what {} needs, and a re-fire is what takes it away",
            waiting.join(", ")
        ))
    }

    /// What stands in the way, in [`Condition::ALL`]'s order — empty when [`may_promote`] is true.
    ///
    /// [`may_promote`]: Self::may_promote
    #[must_use]
    pub fn held_back_by(&self) -> Vec<(Condition, &str)> {
        self.rows()
            .filter_map(|(condition, _, answer)| match answer {
                Answer::Met => None,
                Answer::Blocked(why) | Answer::Unknowable(why) => Some((condition, why.as_str())),
            })
            .collect()
    }
}

/// ⛔⛔⛔⛔⛔ **WHAT HAPPENS AFTER THE RUN THAT JUST FINISHED** — register item 868's done-when ⑶,
/// where two register entries claim the same instant.
///
/// ⚠⚠ **TWO ARMS AND EACH CARRIES ITS REASON.** A bare verdict here would be prose one line later
/// — a watcher told *stand down* with no reason re-fires anyway, and this repository's rule 10 is
/// that an unmeasured justification is one nobody reads. So the reason is part of the value and
/// [`Readiness::what_follows_an_ending`] composes it from the rows it actually read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WhatFollowsAnEnding {
    /// **RE-FIRE NOW** — item 827, with why holding the loop back would buy nothing.
    ReFire(String),
    /// **LEAVE THE TREE QUIET AND PROMOTE** — item 868, with what the re-fire would take away.
    StandDown(String),
}

impl WhatFollowsAnEnding {
    /// The word it is reported under.
    #[must_use]
    pub const fn word(&self) -> &'static str {
        match self {
            Self::ReFire(_) => "RE-FIRE NOW",
            Self::StandDown(_) => "STAND DOWN — PROMOTE FIRST",
        }
    }

    /// Why, in the words the reading composed.
    #[must_use]
    pub fn why(&self) -> &str {
        let (Self::ReFire(why) | Self::StandDown(why)) = self;
        why
    }
}

impl fmt::Display for WhatFollowsAnEnding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} — {}", self.word(), self.why())
    }
}

/// ⛔⛔⛔⛔⛔ **ONE ANSWER FOR A CONDITION MEASURED OVER SEVERAL THINGS** — register item 868's ⑶,
/// which is about FOUR binaries and one verdict.
///
/// # ⛔⛔⛔⛔⛔ *Cannot be measured* is not *does not hold*, and mixing them costs both ways
///
/// Measured 2026-09-05 over the promoted binaries: **only `sprag` states its build**
/// (`sprag 0.0.1 (7181c7483168)`). `sprag-term` reads `--version` as a command to spawn,
/// `sprag-gui` and `sprag-mcp` print nothing. So three of the four cannot answer at all, and the
/// first draft of this tool called that **BLOCKED** — which reads as *the binaries are stale* and
/// would have kept the verdict at `NO` for ever, an instrument nobody reads. Calling it `Met`
/// would be the mirror: three unidentified images promoted on one binary's word.
///
/// ⇒ So the fold is ordered: any [`Answer::Blocked`] wins (something was measured and is wrong),
/// then any [`Answer::Unknowable`] (something could not be asked), and only an all-measured
/// all-matching set is [`Answer::Met`]. Rule 6 — the unclassified case is stated, never glossed.
#[must_use]
pub fn all_of(answers: Vec<Answer>) -> Answer {
    let mut blocked: Vec<&str> = Vec::new();
    let mut unknowable: Vec<&str> = Vec::new();
    for answer in &answers {
        match answer {
            Answer::Met => {}
            Answer::Blocked(why) => blocked.push(why),
            Answer::Unknowable(why) => unknowable.push(why),
        }
    }
    if !blocked.is_empty() {
        // ⛔⛔⛔⛔⛔ AND THE SILENCES TRAVEL WITH IT — the third hole this tool's own output showed.
        // Reporting the mismatch alone hid *three of these four cannot say what build they are*,
        // which is a defect of its own (registered as item 897). The VERDICT takes one arm; the
        // REASON must carry everything that was measured, or the fold throws away a finding.
        if unknowable.is_empty() {
            return Answer::Blocked(blocked.join(", "));
        }
        return Answer::Blocked(format!(
            "{}; and cannot be asked: {}",
            blocked.join(", "),
            unknowable.join(", ")
        ));
    }
    if !unknowable.is_empty() {
        return Answer::Unknowable(unknowable.join(", "));
    }
    Answer::Met
}

/// ⭐⭐⭐ **WHAT IS BEHIND THE DOOR** — register item 868's done-when ⑷, the clause another
/// watcher's wrong diagnosis paid for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BehindTheDoor {
    /// The build the daemon says it is, or [`None`] when **nothing could say**.
    ///
    /// ⛔⛔⛔⛔⛔ **AN OPTION, AND THE FIRST DRAFT OF THIS TOOL PROVED WHY** — register item 891's
    /// rule one surface over. It held a `String` and the reader passed `"unknown"` when the daemon
    /// could not be asked; `git log unknown..HEAD` then failed, the commit list came back empty,
    /// and this type printed **`nothing is waiting`** with nineteen commits behind the door. The
    /// reassuring reading of an unmeasured value, in the very instrument built to end it.
    pub daemon: Option<String>,
    /// The commit this tree is at.
    pub head: String,
    /// The commits between them, newest first, as `<short> <subject>`.
    pub commits: Vec<String>,
}

impl fmt::Display for BehindTheDoor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Some(daemon) = &self.daemon else {
            // ⛔ NEVER `nothing is waiting` — see the field. *Nobody could say* and *nothing is
            // waiting* are opposite facts, and the second is the one that stops a reader looking.
            return write!(
                f,
                "nothing could say which build the daemon is, so what is behind the door is \
                 UNMEASURED — this tree is at {}",
                self.head
            );
        };
        if self.commits.is_empty() {
            return write!(
                f,
                "the daemon says {daemon} and this tree is at {} — nothing is waiting",
                self.head
            );
        }
        // ⚠ THE COUNT AND THE SUBJECTS, never the count alone — item 868's ⑷ is *「데몬이 N 커밋
        // 뒤처졌고 «그중 이런 것들이 있다»」*, and the number alone is what a person then goes and
        // types `git log` to expand. The whole point is that they do not have to.
        writeln!(
            f,
            "the daemon says {daemon} and this tree is at {} — {} commit(s) are waiting behind the \
             door:",
            self.head,
            self.commits.len()
        )?;
        for commit in &self.commits {
            writeln!(f, "    {commit}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⛔⛔⛔⛔⛔ **A BLINDNESS IS NOT A CONSENT** — register item 868, and the arm that decides
    /// whether this instrument can miss the window it was built for.
    ///
    /// # ⛔⛔⛔⛔⛔ What the wrong answer would have looked like
    ///
    /// Condition ⑴ is another repository's live run, which this process cannot see. Two of the
    /// three ways to write that are wrong in opposite directions and both read as reasonable:
    ///
    /// | folding | what it says | what happens |
    /// |---|---|---|
    /// | `Unknowable` → `Met` | *promote now* | promotes into a shut window — item 868's own failure |
    /// | `Unknowable` → `Blocked` | *never promote* | the instrument is silent for ever and nobody reads it |
    ///
    /// So it is a third answer that holds the verdict back AND names who to ask, and this test is
    /// what keeps it from collapsing into either neighbour.
    ///
    /// ⚠ The moments are asserted too, because item 868's own ⑵ was measured WRONG for want of
    /// one: it read *the tree is clean* as a condition of the copy, and the second promotion proved
    /// it is a condition of the BUILD. A condition with no moment holds on longer than it needs to.
    #[test]
    fn a_blindness_is_not_a_consent_and_every_condition_states_its_moment() {
        // ── THE MOMENTS ARE THREE, AND DISTINCT ─────────────────────────────────────────────
        let moments: Vec<Moment> = Condition::ALL.into_iter().map(Condition::moment).collect();
        assert_eq!(
            moments,
            vec![Moment::BeforeBoth, Moment::WhileBuilding, Moment::AtTheCopy],
            "⛔⛔⛔⛔ REGISTER ITEM 868: two conditions share a moment, so the reading cannot say \
             WHEN each has to hold — which is the overreach that made this item's own ⑵ wrong. A \
             clean tree is a condition of the BUILD; a build that says HEAD is a condition of the \
             COPY. Collapsing them holds a promotion back for an edit that cannot reach the binary",
        );

        // ── AND THE THIRD ANSWER IS NOT A GO ────────────────────────────────────────────────
        let blind = Readiness::of([
            Answer::Unknowable("ask the owner — `sprag my-runs`".to_owned()),
            Answer::Met,
            Answer::Met,
        ]);
        assert!(
            !blind.may_promote(),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 868: an instrument that reads its own blindness as consent \
             says *promote* at the one moment the window is shut. That is the failure this item \
             was opened over — a window opened, this side was not ready, and it was missed",
        );
        assert_eq!(
            blind.held_back_by(),
            vec![(
                Condition::AWindowSomebodyElseOpened,
                "ask the owner — `sprag my-runs`",
            )],
            "⚠⚠ AND A `no` NAMES WHAT BLOCKS IT — item 868's done-when ⑵. A bare `no` sends a \
             person to measure the three conditions by hand, which is the thing this replaces",
        );

        // ── A BLOCKED CONDITION IS REPORTED BESIDE IT, NOT INSTEAD OF IT ────────────────────
        let two = Readiness::of([
            Answer::Unknowable("ask the owner".to_owned()),
            Answer::Blocked("3 file(s) edited since the build".to_owned()),
            Answer::Met,
        ]);
        assert_eq!(
            two.held_back_by().len(),
            2,
            "⚠ every blocker is listed: a reader who fixes the first and finds a second waiting \
             has been told half of what the instrument knew",
        );

        // ── AND EVERY CONDITION MET IS A GO ─────────────────────────────────────────────────
        assert!(
            Readiness::of([Answer::Met, Answer::Met, Answer::Met]).may_promote(),
            "⚠ THE CONTROL: the arms above must not be a gate that never says yes — an instrument \
             that always refuses is one nobody reads, which is the other way to lose the window",
        );
    }

    /// ⭐⭐⭐ **THE DOOR SAYS WHAT IS BEHIND IT, WITH SUBJECTS AND NOT ONLY A COUNT** — register
    /// item 868's ⑷.
    ///
    /// A count alone is what a person then expands by typing `git log` — and the measurement that
    /// opened this clause is that nobody typed it: the number reached **10** with nothing ringing,
    /// and another watcher read *unshipped* as *unfixable* on top of that silence.
    #[test]
    fn the_door_names_the_commits_waiting_behind_it() {
        let waiting = BehindTheDoor {
            daemon: Some("7181c7483168".to_owned()),
            head: "462d19b".to_owned(),
            commits: vec![
                "462d19b feat(gate): a parent line has to say it was made".to_owned(),
                "979da0c test(host): a finished run keeps the conversation".to_owned(),
            ],
        };
        let said = waiting.to_string();
        assert!(
            said.contains("2 commit(s) are waiting")
                && said.contains("7181c7483168")
                && said.contains("462d19b")
                && said.contains("a finished run keeps the conversation"),
            "⛔⛔⛔⛔ REGISTER ITEM 868 ⑷: the door has to name the commits, both builds and the \
             count. A count with no subjects is a number a person expands by hand, and this clause \
             exists because that hand-typed expansion never happened. Said: {said}",
        );
        assert_eq!(
            BehindTheDoor {
                daemon: Some("462d19b".to_owned()),
                head: "462d19b".to_owned(),
                commits: Vec::new(),
            }
            .to_string(),
            "the daemon says 462d19b and this tree is at 462d19b — nothing is waiting",
            "⚠ THE CONTROL: a daemon that IS head must say so plainly rather than printing an \
             empty list, or a reader cannot tell *nothing waiting* from *nothing measured*",
        );

        // ⛔⛔⛔⛔⛔ ── AND THE ONE THIS TOOL'S FIRST DRAFT GOT WRONG ────────────────────────────
        //
        // It held a bare `String` and the reader passed `"unknown"`; `git log unknown..HEAD`
        // failed, the list came back empty, and it printed `nothing is waiting` with NINETEEN
        // commits behind the door. Register item 891's rule, met inside the instrument written to
        // stop a door being silent.
        let unmeasured = BehindTheDoor {
            daemon: None,
            head: "462d19b".to_owned(),
            commits: Vec::new(),
        }
        .to_string();
        assert!(
            unmeasured.contains("UNMEASURED") && !unmeasured.contains("nothing is waiting"),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 868 ⑷ and item 891: *nobody could say* printed as *nothing \
             is waiting*, which is the reassuring reading of an unmeasured value and the one that \
             stops a reader looking. Said: {unmeasured}",
        );
    }

    /// ⛔⛔⛔⛔⛔ **THE CONFLICT BETWEEN RE-FIRING AND PROMOTING IS SETTLED BY THE ROWS, NOT BY A
    /// PREFERENCE** — register item 868's done-when ⑶, the clause that asks *되걸기와 승격 중
    /// 무엇이 이기는지* to become something a reader can ask.
    ///
    /// # ⛔⛔⛔⛔⛔ What both wrong answers look like, and what each costs
    ///
    /// | fixed answer | what it says | what it costs |
    /// |---|---|---|
    /// | always re-fire | item 827 always wins | the window is never made — 23 commits deep today |
    /// | always stand down | item 868 always wins | 3 h 49 m of dead loop, against 2-3 min elsewhere |
    ///
    /// Both read as decisive and both are wrong on some rounds, which is why item 868 left the
    /// clause open rather than picking one. The axis that settles it is
    /// [`Condition::mended_by_standing_down`]: **quiet is worth waiting for only when quiet is
    /// what the unmet condition needs.**
    ///
    /// # ⚠⚠⚠ The arms, and why the second is the one that keeps this honest
    ///
    /// Arm ② is today's real state — condition ⑴ is another repository's and comes back
    /// *ask a person*, so this tree standing down buys nothing and item 827 wins **by
    /// measurement**. A build that read that blindness as a reason to wait would hold the loop
    /// down for ever on a window nobody had opened, which is [`Readiness::may_promote`]'s failure
    /// wearing the opposite face.
    #[test]
    fn a_stand_down_is_owed_only_to_a_condition_that_a_stand_down_can_mend() {
        // ── THE AXIS IS STATED PER CONDITION, and it is not `Moment` re-spelled ─────────────
        assert_eq!(
            Condition::ALL.map(Condition::mended_by_standing_down),
            [false, true, true],
            "⛔⛔⛔⛔⛔ REGISTER ITEM 868 ⑶: a condition that does not say whether quiet reaches \
             it cannot take part in this judgement, and the judgement then collapses into a \
             preference between two register entries — which is the state the clause was left in",
        );

        // ── ① THE WINDOW IS OPEN AND THE TREE IS DIRTY: item 868 wins ───────────────────────
        //
        // A person has said the other repository's window is open, so the only thing left is what
        // this tree can give — and what it can give is exactly what a re-fire would take.
        let owed = Readiness::of([
            Answer::Met,
            Answer::Blocked("3 path(s) edited in this tree".to_owned()),
            Answer::Blocked("sprag says 7181c74".to_owned()),
        ])
        .what_follows_an_ending();
        let WhatFollowsAnEnding::StandDown(why) = &owed else {
            panic!(
                "⛔⛔⛔⛔⛔ REGISTER ITEM 868 ⑶: the window a person confirmed is open, the tree \
                 is the only thing standing in the way, and this said re-fire — which closes the \
                 window in the minutes item 868 measured it lasting. Got: {owed:?}"
            );
        };
        assert!(
            why.contains("nothing edited while building") && why.contains("3 path(s)"),
            "⚠⚠ AND IT NAMES WHAT THE RE-FIRE WOULD TAKE: a watcher told *stand down* with no \
             reason re-fires anyway, which is rule 10 inside this instrument. Said: {why}",
        );

        // ── ② NOBODY HAS OPENED A WINDOW: item 827 wins, and by measurement ─────────────────
        let dead_time = Readiness::of([
            Answer::Unknowable("ask whoever holds the other run (`sprag my-runs`)".to_owned()),
            Answer::Blocked("3 path(s) edited in this tree".to_owned()),
            Answer::Blocked("sprag says 7181c74".to_owned()),
        ])
        .what_follows_an_ending();
        let WhatFollowsAnEnding::ReFire(why) = &dead_time else {
            panic!(
                "⛔⛔⛔⛔⛔ REGISTER ITEM 827: no quiet in THIS tree opens another repository's \
                 window, so a loop that stood down for it would wait for ever — 3 h 49 m was the \
                 measured cost of one such wait, against 2-3 minutes in two other repositories. \
                 This is also today's real reading, so a build that gets it wrong stops the loop \
                 the moment it is deployed. Got: {dead_time:?}"
            );
        };
        assert!(
            why.contains("a window somebody else opened"),
            "⚠⚠ the reason has to name the condition that is out of reach, or a reader cannot \
             tell *nothing to wait for* from *nothing measured*. Said: {why}",
        );

        // ── ③ EVERYTHING HOLDS: still re-fire, on item 868's own correction ─────────────────
        //
        // ⚠ The copy does not read this tree — the item measured that at its second promotion,
        // where the build was made clean and the tree was dirty by the copy and nothing was wrong.
        // So a ready door asks the loop for nothing.
        let ready = Readiness::of([Answer::Met, Answer::Met, Answer::Met]).what_follows_an_ending();
        assert!(
            matches!(ready, WhatFollowsAnEnding::ReFire(_)),
            "⚠⚠⚠ THE CONTROL AGAINST A GATE THAT ALWAYS STANDS THE LOOP DOWN: with every \
             condition met there is nothing for the loop to stay out of the way of, and holding \
             it back would be item 827's dead time bought for nothing. Got: {ready:?}",
        );

        // ── ④ AND THE VERDICT IS READABLE WITHOUT PARSING THE REASON ───────────────────────
        assert_eq!(
            (owed.word(), dead_time.word()),
            ("STAND DOWN — PROMOTE FIRST", "RE-FIRE NOW"),
            "⚠ the two arms must be tellable apart by a word, because the reader is a person \
             scanning one line of a tool's output",
        );
    }

    /// ⛔⛔⛔⛔⛔ **A CONDITION MEASURED OVER FOUR THINGS FOLDS WITHOUT LOSING *CANNOT ASK*** —
    /// register item 868 ⑶, and see [`all_of`].
    #[test]
    fn cannot_be_measured_does_not_fold_into_does_not_hold() {
        assert_eq!(
            all_of(vec![Answer::Met, Answer::Met]),
            Answer::Met,
            "⚠ THE CONTROL: an all-measured all-matching set has to be a go, or the instrument \
             never says yes and nobody reads it",
        );
        assert_eq!(
            all_of(vec![
                Answer::Met,
                Answer::Unknowable("sprag-term cannot say".to_owned()),
            ]),
            Answer::Unknowable("sprag-term cannot say".to_owned()),
            "⛔⛔⛔⛔⛔ REGISTER ITEM 868 ⑶: three of the four promoted binaries do not state \
             their build (measured 2026-09-05), and calling that BLOCKED reads as *they are \
             stale* — a verdict stuck at NO for a reason nobody can act on. It is *cannot ask*",
        );
        // ⛔⛔⛔⛔⛔ A MEASURED MISMATCH TAKES THE VERDICT AND THE SILENCE STILL GETS SAID
        //
        // This tool's own first output showed why: `sprag says 7181c74` won, and *three of these
        // four cannot say what build they are* — a defect of its own — vanished from the line. A
        // fold that keeps one arm must not drop the other's evidence.
        let mixed = all_of(vec![
            Answer::Unknowable("sprag-term cannot say".to_owned()),
            Answer::Blocked("sprag says 7181c74".to_owned()),
        ]);
        let Answer::Blocked(why) = &mixed else {
            panic!(
                "⚠⚠ a measured mismatch is the arm a reader must act on, and it has to win the \
                 verdict: {mixed:?}"
            );
        };
        assert!(
            why.contains("sprag says 7181c74") && why.contains("sprag-term cannot say"),
            "⛔⛔⛔⛔ REGISTER ITEM 868 ⑶: the fold reported the mismatch and threw the silence \
             away, so a reader never learns that three of the four binaries cannot state their \
             build at all. Said: {why}",
        );
    }
}
