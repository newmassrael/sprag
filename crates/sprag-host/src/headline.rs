//! ⛔⛔⛔⛔⛔ **WHAT AN OUTER-LOOP WATCHER READS OFF `sprag runs`** — register item 892, and the
//! contract that lived in a file this repository cannot see.
//!
//! # ⛔⛔⛔⛔⛔ Five positional reads, and the source of all five was outside the tree
//!
//! The repayment skill's `watch.sh` parses `sprag runs` by POSITION: a block opens on a head line,
//! the run's status is the line after it, the run's stamp is bracketed at the head's end, and a
//! walk line is recognised by its indent and its byte count. None of that is a format anybody
//! declared — it is five expressions in a shell script under `$HOME`, which `include_str!` cannot
//! reach and which this repository must not depend on by path.
//!
//! So every product-side claim about those positions was a **human's paraphrase**. Measured
//! 2026-09-06 over `crates/sprag-host/src/bin/sprag.rs`: **30 comments** assert one of these
//! constraints and **one** gate held any of them — item 890's, which re-spelled the stamp read as
//! `head.ends_with("[…]")`. Two anchors had no gate at all.
//!
//! ⇒ Register item 890 broke one of them for real: appending a clause after the stamp made the
//! watcher's `sed` yield the empty string, while the stamp was still printed and still looked
//! right to a person. What caught it was somebody running the expression by hand.
//!
//! # ⛔⛔⛔⛔⛔ And the paraphrase had already drifted from the expression it paraphrased
//!
//! `render_run` carried *"its walk as the block's LAST line (`… | tail -1`)"* — and `watch.sh` has
//! not read the last line since it was rewritten. `tail -1` survives there only in two comments
//! explaining why it was REPLACED: it could not see two transitions landing inside one poll, and
//! it could not reach behind its own birth (five of five logs began at iteration 2). The live
//! expression matches a walk line by SHAPE, anywhere in the block.
//!
//! ⇒ ⭐ So the product was defending a constraint the watcher no longer has, while the constraint
//! it does have was asserted by nothing. **A copied contract does not fail loudly; it drifts and
//! goes on reading true.**
//!
//! # ⚠⚠ What this module is, and what it deliberately is not
//!
//! It is the contract stated ONCE, as a value: each anchor carries the pattern the watcher applies
//! and applies it itself, so a gate asks *does the real rendered block still answer this* rather
//! than *does a paraphrase still look right*. `sprag runs --contract` prints it, so the file
//! outside this tree has something to derive from instead of a memory.
//!
//! ⚠ It is NOT a promise that the watcher agrees. That file is outside the repository and no gate
//! here can read it; what changes is that a drift is now findable by running one command instead
//! of by somebody thinking to try a `sed`.

/// ⛔⛔⛔⛔⛔ **ONE THING AN OUTER-LOOP WATCHER READS OFF A RUN'S BLOCK** — register item 892, and
/// a closed vocabulary because the alternative is what this replaces: five expressions in a file
/// this tree cannot see, paraphrased at thirty comment sites and gated at one.
///
/// ⚠⚠ A sixth read added to `watch.sh` and not declared here is a read nothing protects — which is
/// the state all five were in. [`ALL`](Self::ALL) plus an exhaustive `match` at every method is
/// this workspace's rule for a vocabulary, so the arm cannot be half-added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum WatcherAnchor {
    /// **A RUN'S BLOCK OPENS ON ITS HEAD LINE AND ENDS AT THE NEXT ONE** — `awk -v r="^run $RUN "`,
    /// with `/^run [0-9]/` closing it. Everything else here is read inside that block.
    BlockHead,
    /// **THE RUN'S STATUS IS THE LINE IMMEDIATELY AFTER THE HEAD** — `$0 ~ r {getline; print}`.
    ///
    /// ⚠ This is the anchor most of `render_run`'s thirty comments are about: a clause added to the
    /// heading is safe, and a clause added between the heading and the status silently becomes the
    /// status for every watcher on this machine.
    StatusAfterHead,
    /// **THE RUN'S STAMP IS BRACKETED AT THE END OF THE HEAD** — `sed -n
    /// 's/.*\[\([^][]*\)\]$/\1/p'`, anchored at `$`.
    ///
    /// ⛔ Register item 890 appended a clause after it, with a comment claiming the read *"still
    /// takes the stamp"*. It does not: the head then ends in a tree path, the expression yields
    /// nothing, and item 887's whole point — a watcher can tell a reissued run number from its own
    /// run — is off while the stamp is still printed and still looks right.
    StampAtHeadEnd,
    /// **THE ITERATION COUNT IS A WORD ON THE STATUS LINE** — `grep -oE "[0-9]+ iterations"`.
    ///
    /// ⚠ It is how the watcher tells *a run that is blocked* from *a run that is merely slow*: it
    /// reports a needed person only when the count has ALSO not moved since the previous poll.
    IterationsInStatus,
    /// **A WALK LINE IS RECOGNISED BY ITS SHAPE, ANYWHERE IN THE BLOCK** — `/^ +[0-9]+ +[0-9]+
    /// bytes /`, with the first field the iteration number.
    ///
    /// ⛔⛔ **NOT *the block's last line*, which is what this product's own comment said.** That was
    /// true of a `tail -1` the watcher replaced, for two measured reasons it wrote down: the last
    /// line cannot see two transitions landing inside one three-second poll, and it cannot reach
    /// behind its own birth — five of five logs began at iteration 2.
    WalkLine,
}

impl WatcherAnchor {
    /// Every read, in the order a watcher performs them.
    pub const ALL: [Self; 5] = [
        Self::BlockHead,
        Self::StatusAfterHead,
        Self::StampAtHeadEnd,
        Self::IterationsInStatus,
        Self::WalkLine,
    ];

    /// **THE NAME THE CONTRACT IS PUBLISHED UNDER**, so the file outside this tree and a gate
    /// inside it can name the same read — ⛔ an exhaustive `match` with no `_`.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::BlockHead => "block-head",
            Self::StatusAfterHead => "status-after-head",
            Self::StampAtHeadEnd => "stamp-at-head-end",
            Self::IterationsInStatus => "iterations-in-status",
            Self::WalkLine => "walk-line",
        }
    }

    /// ⛔⛔⛔⛔⛔ **THE PATTERN, AND IT IS APPLIED RATHER THAN DESCRIBED** — [`read`](Self::read)
    /// runs exactly this, so the published contract and the gate cannot come apart.
    ///
    /// ⚠⚠ It is an ERE, which is what `awk` takes and what this crate's `regex` accepts. The one
    /// read spelled in `sed`'s BRE ([`StampAtHeadEnd`](Self::StampAtHeadEnd)) differs only in which
    /// parentheses and brackets carry a backslash; the published contract prints the shell's own
    /// form beside this one in [`as_written`](Self::as_written) so a person can diff them without
    /// having to hold that rule in their head.
    #[must_use]
    pub const fn pattern(self) -> &'static str {
        match self {
            Self::BlockHead => "^run [0-9]+ ",
            // ⚠ THE STATUS IS A POSITION, NOT A SHAPE, so the pattern is the head's — the read is
            // *the line after whatever this matches*, and `read` is what performs that step.
            Self::StatusAfterHead => "^run [0-9]+ ",
            // ⛔⛔⛔ AND THIS ONE CANNOT BE BYTE-IDENTICAL TO THE SHELL'S, WHICH IS MEASURED AND
            // NOT A STYLE CHOICE. POSIX says a `]` FIRST in a bracket expression is a literal, so
            // `[^][]` is *neither bracket* — and Rust's `regex` refuses it as an unclosed class
            // (`error: unclosed character class`, hit while writing this module). The escaped form
            // below is that same set, and `as_written` publishes the shell's spelling beside it so
            // the difference is on the page rather than in somebody's head.
            Self::StampAtHeadEnd => r".*\[([^\]\[]*)\]$",
            Self::IterationsInStatus => "[0-9]+ iterations",
            Self::WalkLine => "^ +[0-9]+ +[0-9]+ bytes ",
        }
    }

    /// ⛔⛔⛔⛔⛔ **THE SMALLEST TEXT THAT MUST APPEAR VERBATIM IN THE WATCHER**, so the two sides
    /// can be compared by a machine instead of by a reader's eye.
    ///
    /// # ⚠⚠⚠ Why a fragment and not the whole pipeline
    ///
    /// The obvious publication is the command the watcher runs. It cannot be: that script writes
    /// its block reader across five lines with its own spacing, so a one-line rendering of it is
    /// **already a paraphrase** — the exact thing register item 892 is about, reintroduced by the
    /// fix. What both sides can hold identically is the ANCHOR: the distinctive text each read is
    /// pinned on, which is also precisely what may not drift.
    ///
    /// ⇒ The watcher greps this repository's published list against its own source at startup and
    /// refuses to watch when one is unmatched, so a clause moved on either side is loud rather than
    /// silent. [`pattern`](Self::pattern) is what this product applies; this is what that file must
    /// contain.
    #[must_use]
    pub const fn as_written(self) -> &'static str {
        match self {
            // ⚠ The block TERMINATOR rather than its opener: the opener is built from a shell
            // variable (`-v r="^run $RUN "`), so the text in the file is not the text that runs.
            // This one is a literal on both sides and pins the same fact — where a block ends.
            Self::BlockHead => "/^run [0-9]/",
            Self::StatusAfterHead => "{getline; print; exit}",
            Self::StampAtHeadEnd => r"sed -n 's/.*\[\([^][]*\)\]$/\1/p'",
            Self::IterationsInStatus => "grep -oE \"[0-9]+ iterations\"",
            Self::WalkLine => "/^ +[0-9]+ +[0-9]+ bytes /",
        }
    }

    /// **WHAT THE PRODUCT MUST KEEP TRUE FOR THAT READ TO WORK**, in one clause — the sentence a
    /// person adding a line to `render_run` needs, at the moment they are adding it.
    #[must_use]
    pub const fn requires(self) -> &'static str {
        match self {
            Self::BlockHead => {
                "every run's block begins with a line starting `run <id> `, and no other line in a \
                 block may start that way"
            }
            Self::StatusAfterHead => {
                "the run's status is the line IMMEDIATELY after the head — a clause inserted \
                 between them becomes the status for every watcher"
            }
            Self::StampAtHeadEnd => {
                "the head line ENDS with the bracketed stamp — a clause appended after it yields \
                 the empty string, silently, while the stamp is still printed"
            }
            Self::IterationsInStatus => {
                "a running row's status line carries `<n> iterations` in those words, so a blocked \
                 run can be told from a slow one"
            }
            Self::WalkLine => {
                "each walk line is indented and reads `<iteration> <bytes> bytes …`, anywhere in \
                 the block rather than only at its end"
            }
        }
    }

    /// ⛔⛔⛔⛔⛔ **PERFORM THE READ ON A REAL RENDERED BLOCK**, and answer what the watcher would
    /// get — [`None`] where it would get nothing.
    ///
    /// # ⚠⚠ Why a gate applies the read instead of asserting a shape
    ///
    /// Item 890's gate said `head.ends_with("[…]")`, which is that `sed` reduced by a person to
    /// what they believed it required. It happened to be right. The next reduction is the one
    /// nobody checks — and the walk read had already drifted in exactly that way, its paraphrase
    /// naming a `tail -1` the watcher had dropped. Running the pattern removes the step where a
    /// belief enters.
    ///
    /// ⚠ `block` is one run's block as `sprag runs` prints it: the head line and everything under
    /// it up to the next head.
    ///
    /// # Panics
    ///
    /// Never in practice — every [`pattern`](Self::pattern) is a literal in this file and a gate
    /// compiles all five. A failure here is this module's own pattern being unparseable, which is
    /// the one state a caller cannot repair.
    #[must_use]
    pub fn read(self, block: &str) -> Option<String> {
        let matcher =
            regex::Regex::new(self.pattern()).expect("this module's own patterns compile");
        let head = block.lines().find(|line| matcher.is_match(line));
        match self {
            Self::BlockHead => head.map(str::to_owned),
            // ⚠ THE POSITION IS THE READ: `getline` takes whatever line follows, with no test of
            // its own — which is exactly why a clause inserted there is invisible to the watcher
            // and fatal to it at the same time.
            Self::StatusAfterHead => {
                let at = block.lines().position(|line| matcher.is_match(line))?;
                block.lines().nth(at + 1).map(str::to_owned)
            }
            // ⚠⚠ THE CAPTURE, not the match: `sed`'s `\1` is the bracketed text, and a head whose
            // last bracket is not at the end of the line yields nothing at all.
            Self::StampAtHeadEnd => {
                let matcher = regex::Regex::new(Self::StampAtHeadEnd.pattern())
                    .expect("this module's own patterns compile");
                let head = block.lines().next()?;
                matcher.captures(head).map(|found| found[1].to_owned())
            }
            // ⚠ ON THE STATUS LINE and nowhere else — the watcher pipes `$status` into it, so a
            // count printed further down the block is not what it reads.
            Self::IterationsInStatus => {
                let status = Self::StatusAfterHead.read(block)?;
                matcher.find(&status).map(|found| found.as_str().to_owned())
            }
            // ⚠ THE FIRST FIELD, which is the iteration number: `$1` after the shape matched.
            Self::WalkLine => head
                .and_then(|line| line.split_whitespace().next())
                .map(str::to_owned),
        }
    }

    /// **THE CONTRACT, AS `sprag runs --contract` PRINTS IT** — one block per anchor, so the file
    /// outside this tree has something to derive from.
    #[must_use]
    pub fn published() -> Vec<String> {
        let mut said = vec![
            "the reads an outer-loop watcher performs on `sprag runs`, stated here so a file \
             outside this repository has one source — register item 892"
                .to_owned(),
        ];
        for anchor in Self::ALL {
            said.push(format!("  {}", anchor.word()));
            said.push(format!("    requires   {}", anchor.requires()));
            said.push(format!("    pattern    {}", anchor.pattern()));
            said.push(format!("    as written {}", anchor.as_written()));
        }
        said
    }
}

#[cfg(test)]
mod tests {
    use super::WatcherAnchor;

    /// A block shaped exactly as `sprag runs` prints a running row — head, status, walk lines.
    fn a_block() -> String {
        [
            "run 240  ai_loop pane=7  asked for by pinion-66  judged as debt  (driven by build \
             52459b9)  (in /home/coin/sprag)  [1f4a-17e2c9d31bb40000-0.c7]",
            "  running — 12 iterations, 8451 bytes so far",
            "  2 prompt(s) delivered, all of them on that pane",
            "     11     4120 bytes  working --step--> reviewing",
            "     12     4331 bytes  reviewing --hold--> working",
        ]
        .join("\n")
    }

    /// ⛔⛔⛔⛔⛔ **EVERY READ THE WATCHER PERFORMS STILL ANSWERS OFF A REAL BLOCK** — register item
    /// 892, and the gate the contract never had.
    ///
    /// # ⚠⚠⚠ Each anchor is asserted with its OWN answer, not merely as non-empty
    ///
    /// Four of these reads return something for a block that has moved underneath them — the
    /// status read returns whatever line is in that position, the walk read returns the first field
    /// of whatever matched. *It answered* is therefore not the question; *it answered THIS* is, and
    /// it is what tells a clause inserted in the wrong place from a clause added safely.
    #[test]
    fn every_read_the_watcher_performs_still_answers() {
        let block = a_block();
        for (anchor, expected) in [
            (
                WatcherAnchor::StatusAfterHead,
                "  running — 12 iterations, 8451 bytes so far",
            ),
            (WatcherAnchor::StampAtHeadEnd, "1f4a-17e2c9d31bb40000-0.c7"),
            (WatcherAnchor::IterationsInStatus, "12 iterations"),
            (WatcherAnchor::WalkLine, "11"),
        ] {
            assert_eq!(
                anchor.read(&block).as_deref(),
                Some(expected),
                "⛔⛔⛔⛔⛔ REGISTER ITEM 892: the watcher's `{}` read no longer answers what it \
                 must. {} — and the whole finding of this item is that this fails SILENTLY: the \
                 value is still printed, the row still looks right to a person, and the monitoring \
                 is off. Expression: {}",
                anchor.word(),
                anchor.requires(),
                anchor.as_written(),
            );
        }
        // ⚠ THE HEAD IS ASSERTED APART, because its answer is the whole line and quoting it twice
        // would make this gate a copy of the fixture rather than a test of the read.
        assert!(
            WatcherAnchor::BlockHead
                .read(&block)
                .is_some_and(|head| head.starts_with("run 240 ")),
            "⛔⛔⛔ REGISTER ITEM 892: a block must open on `run <id> `, or a watcher cannot find \
             the run it was started for at all",
        );
    }

    /// ⛔⛔⛔⛔⛔ **AND EACH READ BREAKS WHEN ITS OWN POSITION MOVES, WHILE THE OTHERS DO NOT** —
    /// register item 892's `⛔` clause: the gate must be red per anchor, not in aggregate.
    ///
    /// # ⚠⚠ Driven on blocks rather than on the renderer, so the four failures are separable
    ///
    /// A renderer test can only move one clause at a time and each move is a source edit. These are
    /// the four moves stated as data — a clause between head and status, a clause after the stamp
    /// (item 890's actual mistake), a status that stops saying the word, and a walk line that loses
    /// its shape — so the gate above is shown to be sensitive to each one separately.
    #[test]
    fn a_clause_in_the_wrong_place_breaks_exactly_one_read() {
        let head = "run 240  ai_loop pane=7  [1f4a-17e2c9d31bb40000-0.c7]";
        let status = "  running — 12 iterations, 8451 bytes so far";
        let walk = "     11     4120 bytes  working --step--> reviewing";

        // ⛔ ITEM 890's ACTUAL MISTAKE: a clause appended after the stamp. The stamp is still on
        // the line and a person reading the row sees nothing wrong.
        let after_stamp = [&format!("{head}  (in /home/coin/sprag)"), status, walk].join("\n");
        assert_eq!(
            WatcherAnchor::StampAtHeadEnd.read(&after_stamp),
            None,
            "⚠ THE PREMISE OF THIS ITEM: a clause after the stamp must break the stamp read, and \
             it must break ONLY that one",
        );
        assert!(
            WatcherAnchor::StatusAfterHead.read(&after_stamp).is_some()
                && WatcherAnchor::WalkLine.read(&after_stamp).is_some(),
            "⚠⚠ AND THE OTHER READS SURVIVE IT — the anchors are separate or this gate is one \
             coarse assertion wearing five names",
        );

        // ⛔ A CLAUSE BETWEEN THE HEAD AND THE STATUS: the status read answers, and answers the
        // WRONG LINE, which is why *it answered* cannot be the test.
        let pushed_down = [head, "  the daemon says something new here", status, walk].join("\n");
        assert_eq!(
            WatcherAnchor::StatusAfterHead.read(&pushed_down).as_deref(),
            Some("  the daemon says something new here"),
            "⚠⚠⚠ A LINE INSERTED HERE SILENTLY BECOMES THE STATUS — the read still succeeds, so \
             only an assertion about WHAT it read can catch this",
        );
        assert_eq!(
            WatcherAnchor::IterationsInStatus.read(&pushed_down),
            None,
            "⚠⚠ AND THE ITERATION READ GOES WITH IT, because it reads the status line — which is \
             how the watcher stops being able to tell a blocked run from a slow one",
        );
        assert!(
            WatcherAnchor::StampAtHeadEnd.read(&pushed_down).is_some(),
            "⚠ while the stamp read is untouched by it",
        );

        // ⛔ A WALK LINE THAT LOSES ITS SHAPE — no byte count, so the block's steps stop being
        // countable and a restart cannot tell which iterations it already reported.
        let unshaped = [head, status, "     11  working --step--> reviewing"].join("\n");
        assert_eq!(
            WatcherAnchor::WalkLine.read(&unshaped),
            None,
            "⚠ the walk read is anchored on `<iteration> <bytes> bytes `, not on the block's last \
             line — the paraphrase this item found had drifted to the latter",
        );
        assert!(
            WatcherAnchor::StatusAfterHead.read(&unshaped).is_some(),
            "⚠ and it is the only read that moves",
        );
    }

    /// ⚠⚠ **THE PUBLISHED CONTRACT CARRIES EVERY ANCHOR**, or the file outside this tree derives
    /// from a list with a hole in it — this workspace's rule 6, applied to what is handed out.
    #[test]
    fn the_published_contract_names_every_read() {
        let said = WatcherAnchor::published().join("\n");
        for anchor in WatcherAnchor::ALL {
            assert!(
                said.contains(anchor.word())
                    && said.contains(anchor.pattern())
                    && said.contains(anchor.as_written()),
                "⛔⛔⛔ REGISTER ITEM 892: `{}` must be published with BOTH forms — the pattern \
                 this product applies and the expression the watcher writes. A contract that \
                 hands out only one of them is one a reader has to complete from memory, which is \
                 the state this item is about. Got:\n{said}",
                anchor.word(),
            );
        }
    }
}
