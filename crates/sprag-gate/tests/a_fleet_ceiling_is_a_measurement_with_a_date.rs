//! **THE FLEET'S RAM CEILING MUST BE A MEASUREMENT, AND IT MUST NOT OUTLIVE WHAT IT MEASURED** —
//! register item 456.
//!
//! # What was missing
//!
//! `bx` places a run on a build machine with `min(free_cores, free_gb / peak_gb_per_task)`. This
//! repository declared no `peak_gb_per_task`, so the RAM half of that never applied to it and
//! parallelism was decided by cores alone. Measured 2026-08-19 by the remote-build session:
//! `bx --explain-declaration` answered that field EMPTY. The same day, here, a `cargo doc
//! --workspace --no-deps --document-private-items` ran five rustdocs at once, one of them 6.5GB,
//! pushed 13GB into swap and left the box at load 29 with 82% of the CPU idle — the whole load was
//! processes blocked on the paging disk.
//!
//! # ⚠⚠⚠ Why a number alone would not have been a fix
//!
//! The remote-build skill records the trap from the other side: **a stale or over-large
//! `peak_gb_per_task` shows up not as safety but as a SHRUNKEN FLEET.** A value of 24 was once
//! measured for another repository and it excluded a build machine with 20GB available — the tool
//! read it as "one task needs 24GB" and refused a host that could have run the work. So a guessed
//! ceiling is not the safe direction, and neither is a ceiling that was right last year.
//!
//! Two things can rot the number: the COMMANDS can change, and the code can grow. Nothing can gate
//! the second — that is what the date is for, and why the measurement is recorded rather than
//! folded into a bare integer. The first is gateable and is what most of this file does: every
//! command the declaration names has to carry its own measurement, recorded with the command's own
//! text, so **editing a command is editing something this gate compares** and the round that edits
//! it is told to measure again.
//!
//! # ⚠⚠ Where the numbers come from, and why they are kilobytes
//!
//! `[peak_measured]` records the peak resident size of the LARGEST SINGLE PROCESS the command
//! creates, which is exactly the quantity the formula divides free RAM by. Kilobytes rather than a
//! rounded GB so the file holds what the instrument said and this gate does the arithmetic; a
//! rounded figure in the file would be a number nobody could check against a re-run.
//!
//! ⚠⚠⚠⚠ **AND `/usr/bin/time -v` IS NOT THE INSTRUMENT, THOUGH IT LOOKS LIKE IT.** Its `Maximum
//! resident set size` is `ru_maxrss` of `RUSAGE_CHILDREN`, documented as the largest single
//! descendant — so it answers the right question in principle. Measured 2026-08-20 it gave ~1.2GB
//! for five workloads as different as a 5-second rustdoc pass and a 36-second cold build. That
//! turned out to be TRUE (the same rustc dominates all of them), but nothing in the reading could
//! separate it from the instrument reporting `cargo`'s own footprint every time, and a number that
//! cannot be told from an artefact is not a measurement. Sample `/proc/<pid>/status`' `VmHWM` over
//! every process under the run instead: the kernel keeps that high-water mark per process, so the
//! sampling interval only has to catch a process ALIVE, and the answer arrives with the command
//! line of whatever peaked — which is what `worst` below is for and what a re-measurement is
//! pointed at.
//!
//! ⚠ **6.5 IS NOT THE NUMBER FOR `[commands]`**, and the register says so in capitals. That
//! reading was `--document-private-items`, which is the COMMIT HOOK's doc gate and is not a command
//! this declaration SENDS. The same tree answers differently per command, which is why the record
//! is per-command rather than one figure for "sprag".
//!
//! ⛔⛔⛔⛔⛔ **AND THAT SENTENCE WAS TAKEN TO MEAN NOBODY BUDGETS IT, WHICH WAS FALSE** — register
//! item 932. The declaration does not send that command; this repository's own commit hook HANDS it
//! to the wrapper, and `bx` divides free RAM by `peak_gb_per_task` for whatever it is handed.
//! Measured 2026-09-06 from the wrapper's own logs: 767 of its 10,029 runs here carried
//! `--document-private-items`, and the single most frequent command it has ever been given for this
//! repository is that hook lane — 360 runs. Re-measured with this repository's instrument, the lane
//! peaks at **9,260,688 kB**, four times the figure it was being budgeted by.
//!
//! ⇒ So there is a second population and a second table. `[routed]` holds what this repository
//! HANDS OVER, measured to the same standard, and
//! [`every_command_this_repository_hands_the_wrapper_is_one_it_measured`] holds that every
//! `"${BX}"` call site in `.githooks/` is in it AND bounds itself from its reading. Those readings
//! stay OUT of `[peak_measured]` on purpose: folding 8.83 GiB in would derive
//! `peak_gb_per_task = 9` and divide the same host into two tasks for `build` as well, whose own
//! reading is 1,204,328 kB — the "too HIGH" failure named below, self-inflicted.

use std::collections::BTreeMap;

use sprag_gate::sources::workspace_root;

/// The declaration `bx` reads to decide where this repository's work goes.
const DECL: &str = ".claude/remote-build.toml";

/// One whole gigabyte, as `free -g` and therefore `bx` count them.
///
/// `bx` compares this field against `free -g`'s output, which is GiB, so the conversion here has to
/// be the binary one or the ceiling would be wrong by 7% in the unsafe direction.
const KB_PER_GIB: u64 = 1024 * 1024;

// ─────────────────────────────────────────────────────────────────────────────
// The gates
// ─────────────────────────────────────────────────────────────────────────────

/// ⚠⚠⚠⚠ **THE FIELD EXISTS, AND IN THE SHAPE THE TOOL THAT READS IT CAN READ.**
///
/// The absence was the defect. The spelling is here too because `bx` parses this field with
/// `sed -n 's/^[[:space:]]*peak_gb_per_task[[:space:]]*=[[:space:]]*\([0-9]\+\).*/\1/p'` — an
/// INTEGER regex. Measured by reading that program on 2026-08-20: a declared `2.5` is captured as
/// `2`, so the file would say one thing and the fleet would act on another, silently and in the
/// unsafe direction. A ceiling nobody can see is wrong is worse than no ceiling.
#[test]
fn the_ram_ceiling_is_declared_as_a_whole_number_of_gigabytes() {
    let decl = read_decl();
    let raw = decl.top.get("peak_gb_per_task").unwrap_or_else(|| {
        panic!(
            "⚠ `{DECL}` declares no `peak_gb_per_task`, so `bx` places this repository's work by \
             cores alone and one task's RAM is a fact the tool does not have — register item 456. \
             Measure it and record it under `[peak_measured]`.",
        )
    });

    assert!(
        !raw.is_empty() && raw.bytes().all(|b| b.is_ascii_digit()),
        "⚠ `peak_gb_per_task = {raw}` is not a whole number. `bx` reads this field with an integer \
         regex, so anything after the digits is DROPPED rather than rejected: the file would \
         declare one ceiling and the fleet would use another. Round up to the next whole GiB.",
    );
    assert!(
        raw.parse::<u64>().is_ok_and(|gb| gb > 0),
        "⚠ `peak_gb_per_task = {raw}` — a ceiling of zero tasks is not a ceiling.",
    );
}

/// ⚠⚠⚠⚠⚠ **EVERY COMMAND THIS DECLARATION SENDS HAS BEEN MEASURED, AND MEASURED AS IT NOW READS.**
///
/// This is the half that keeps the number from outliving what it measured. The rows are taken from
/// `[commands]` — the thing that changes — rather than from a list written here, so a command added
/// tomorrow is checked tomorrow without this file being touched. That is register item 445's rule
/// (a list with no glob decides alone) applied to a config file.
///
/// The command's TEXT is compared, not just its name. A `verify` that grows a fifth clause is a
/// different amount of work by a different program, and the ceiling that covered the old one is a
/// claim nobody re-checked.
#[test]
fn every_command_the_declaration_sends_carries_its_own_measurement() {
    let decl = read_decl();
    let commands = decl.table("commands");
    let measured = decl.table("peak_measured");

    assert!(
        !commands.is_empty(),
        "⚠ `{DECL}` declares no `[commands]`. This gate reads that table to know what must have \
         been measured, so an empty one would make it vacuously green — register item 441.",
    );

    for (name, text) in &commands {
        let recorded = measured.get(&format!("{name}_cmd")).unwrap_or_else(|| panic!(
            "⚠ `[commands] {name}` has no `{name}_cmd` under `[peak_measured]`: this declaration \
             sends a command whose RAM cost nobody measured. Run it on a build machine with a COLD \
             `CARGO_TARGET_DIR` — a warm one skips the work whose peak is the answer — while \
             sampling `VmHWM` in `/proc` for every process under it, and record the largest, its \
             command line and the date. ⚠ NOT `/usr/bin/time -v`: this file's own header says why \
             its answer cannot be told from an artefact.",
        ));
        assert_eq!(
            recorded, text,
            "\n⚠⚠⚠ `[commands] {name}` HAS CHANGED SINCE IT WAS MEASURED, so the ceiling above it \
             is a measurement of a command that no longer exists.\n  measured: {recorded}\n  now:   \
             \x20  {text}\nMeasure it again and update `{name}_kb` and `{name}_cmd` together.",
        );
        assert!(
            measured.contains_key(&format!("{name}_kb")),
            "⚠ `[peak_measured] {name}_cmd` is recorded but `{name}_kb` is not, so the command was \
             named and its number was not.",
        );
    }

    for key in measured.keys() {
        if let Some(name) = key.strip_suffix("_cmd") {
            assert!(
                commands.contains_key(name),
                "⚠ `[peak_measured] {key}` measures `{name}`, which `[commands]` no longer sends. \
                 A record for a command that is gone reads exactly like a current one; drop it, or \
                 restore the command it belongs to.",
            );
        }
    }
}

/// ⚠⚠⚠⚠⚠ **THE CEILING IS DERIVED FROM THE MEASUREMENTS, IN BOTH DIRECTIONS.**
///
/// The declared integer must be **the smallest whole GiB strictly above the largest measurement**.
/// Both failures are real and neither is the safe one:
///
/// * below it, the tool packs more tasks onto a host than its RAM holds, which is the swap storm
///   that opened this item;
/// * above it, the tool divides free RAM by a number nobody measured and refuses hosts that could
///   have done the work — the shrunken fleet, and the failure that is invisible because it wears
///   the face of caution.
///
/// `floor + 1` rather than `ceil` deliberately: `ceil` of an exact 2.0 GiB reading would leave zero
/// headroom, and this rounding IS the headroom. It is at most one whole gigabyte of it, which is
/// the most that can be taken without the number ceasing to be the measurement.
#[test]
fn the_ceiling_is_the_next_whole_gigabyte_above_the_worst_measurement() {
    let decl = read_decl();
    let measured = decl.table("peak_measured");

    let mut readings: Vec<(String, u64)> = measured
        .iter()
        .filter_map(|(k, v)| {
            let name = k.strip_suffix("_kb")?;
            let kb = v.parse::<u64>().unwrap_or_else(|_| {
                panic!("⚠ `[peak_measured] {k} = {v}` is not a plain number of kilobytes")
            });
            Some((name.to_string(), kb))
        })
        .collect();
    readings.sort_by_key(|(_, kb)| std::cmp::Reverse(*kb));

    let (worst_name, worst_kb) = readings.first().cloned().unwrap_or_else(|| {
        panic!(
            "⚠ `[peak_measured]` records no `*_kb` reading at all, so there is nothing to derive \
                a ceiling from and this gate would pass on an empty file — register item 441."
        )
    });

    let want = worst_kb / KB_PER_GIB + 1;
    let declared: u64 = declared_ceiling(&decl);

    assert_eq!(
        declared,
        want,
        "\n⚠⚠⚠ `peak_gb_per_task = {declared}` is not what the measurements say.\n  worst reading: \
         {worst_name} at {worst_kb} kB ({:.2} GiB)\n  ceiling that follows from it: {want}\n\
         Too LOW packs a host past its RAM; too HIGH divides free RAM by a number nobody measured \
         and shrinks the fleet. Change the reading or change the ceiling, not the rule.",
        worst_kb as f64 / KB_PER_GIB as f64,
    );
}

/// ⚠⚠⚠ **THE MEASUREMENT SAYS WHEN AND WHERE IT WAS TAKEN.**
///
/// No gate can notice the code growing under a ceiling that is still arithmetically consistent, so
/// the date is the only thing that tells a reader whether to believe it — register item 416's rule
/// that a document claiming a state ages and nothing tells you it has. The host matters for the
/// same reason a divergent test's side matters: a reading from a 125GB machine and one from an 8GB
/// machine are not the same claim.
#[test]
fn the_measurement_says_when_and_where_it_was_taken() {
    let decl = read_decl();
    let measured = decl.table("peak_measured");

    let date = measured.get("date").unwrap_or_else(|| {
        panic!(
            "⚠ `[peak_measured]` carries no `date`, so nothing tells the \
                                   next reader whether the ceiling is current."
        )
    });
    let iso: Vec<&str> = date.split('-').collect();
    assert!(
        iso.len() == 3
            && iso[0].len() == 4
            && iso[1].len() == 2
            && iso[2].len() == 2
            && date.bytes().all(|b| b.is_ascii_digit() || b == b'-'),
        "⚠ `[peak_measured] date = {date}` is not an ISO `YYYY-MM-DD` date.",
    );

    for field in ["host", "worst"] {
        let value = measured.get(field).unwrap_or_else(|| panic!(
            "⚠ `[peak_measured]` carries no `{field}`. A number with no account of what produced \
             it cannot be re-measured, only re-guessed.",
        ));
        assert!(
            value.len() > 8,
            "⚠ `[peak_measured] {field} = {value}` says too little to point a re-measurement at.",
        );
    }
}

/// ⛔⛔⛔⛔⛔ **EVERY COMMAND THIS REPOSITORY HANDS THE WRAPPER IS ONE IT MEASURED, AND EVERY SUCH
/// CALL SITE BOUNDS ITSELF FROM THAT READING** — register item 932.
///
/// The gates above are about `[commands]`, which is what this DECLARATION sends. They are not
/// about what the wrapper RUNS. Measured 2026-09-06 from the wrapper's own logs: it has been given
/// **2,514 distinct commands** for this repository across 10,029 runs, and **2,600** of those runs
/// printed the divisor derived from the two rows in `[peak_measured]`. Nothing said so, because
/// `bx` reads exactly one key from this file — `peak_gb_per_task` — and never reads either table.
///
/// So the population here is the one thing this repository controls: its own `"${BX}"` call sites.
/// Today there is exactly one, and rule 5's question has a plain answer — a site is one `[routed]`
/// row away, and a row is one `measure-peak` run away.
///
/// ⚠⚠ **AND THE READING HAS TO BE USED, NOT MERELY RECORDED.** A row nobody divides by is a number
/// in a file. The bound is applied at the call site because that is the only place that covers the
/// branch with no wrapper in it at all — which is the branch this hook actually takes whenever the
/// operating rules call for `env -u BX`.
///
/// ⛔ Rule 6 runs through all of it: a call site whose argv this cannot compose, a routed command
/// with no reading, and a site that does not bound itself are each a RED naming the line. A shape
/// the composer does not recognise must never be a silent skip — that is the escape hatch this
/// clause exists to close.
#[test]
fn every_command_this_repository_hands_the_wrapper_is_one_it_measured() {
    let decl = read_decl();
    let routed = decl.table("routed");
    let sites = wrapper_call_sites();

    assert!(
        !sites.is_empty(),
        "⚠ no `\"${{BX}}\"` call site was found under `{HOOKS}`. This clause reads them to know \
         what must have been measured, so finding none would make it vacuously green — register \
         item 441. If the hooks genuinely stopped routing anything, delete this clause and the \
         `[routed]` table together and say so in the register.",
    );

    for (site, command) in &sites {
        let name = routed
            .iter()
            .find(|(key, value)| !key.contains('_') && *value == command)
            .map(|(key, _)| key.clone())
            .unwrap_or_else(|| {
                panic!(
                    "⛔ ITEM 932: `{site}` hands the wrapper a command that `[routed]` does not \
                 declare, so `bx` will budget it by `peak_gb_per_task` — a figure derived from \
                 `[commands]`, which this command is not in. Measure it and record it:\n    bash \
                 crates/sprag-gate/tests/doubles/declared-verify/measure-peak <name>\nThe command \
                 as this hook composes it:\n{command}",
                )
            });

        let recorded = routed.get(&format!("{name}_cmd")).unwrap_or_else(|| panic!(
            "⚠ `[routed] {name}` has no `{name}_cmd`: the command is declared and the text it was \
             MEASURED as is not, so nothing can say the reading still describes it.",
        ));
        assert_eq!(
            recorded, command,
            "\n⚠⚠⚠ `[routed] {name}` HAS CHANGED SINCE IT WAS MEASURED.\n  measured: {recorded}\n  \
             now:      {command}\nMeasure it again and update `{name}_kb` and `{name}_cmd` \
             together.",
        );

        let kb: u64 = routed
            .get(&format!("{name}_kb"))
            .and_then(|raw| raw.parse().ok())
            .unwrap_or_else(|| panic!(
                "⚠ `[routed] {name}_cmd` is recorded and `{name}_kb` is not, so the command was \
                 named and its cost was not. `{site}` divides free RAM by that figure.",
            ));
        assert!(
            kb > KB_PER_GIB / 4,
            "⚠ `[routed] {name}_kb = {kb}` is under a quarter of a gigabyte, which no compilation \
             in this workspace has ever measured. A reading that low would hand the call site a \
             parallelism budget larger than the machine, which is the failure the reading exists \
             to prevent.",
        );

        let hook = std::fs::read_to_string(workspace_root().join(site.split(':').next().unwrap()))
            .expect("the hook this site was found in");
        // ⚠ The key AND its `=` — not the bare name. A hook that only MENTIONS the reading in a
        // message (this one does, in its own refusal text) would satisfy a bare-name check while
        // dividing by nothing. Measured: the mutation that repointed the `sed` at another key left
        // that message untouched and this assertion still caught it.
        assert!(
            hook.contains(&format!("{name}_kb = ")),
            "⛔ ITEM 932: `{site}` routes `{name}`, and its hook never reads `{name}_kb` out of \
             `{DECL}`. The reading is then a number in a file: the wrapper divides by \
             `peak_gb_per_task` instead, and the branch that runs without the wrapper divides by \
             nothing at all. Derive the bound from the reading at the call site.",
        );
    }
}

/// ⚠⚠ **A MACHINE THAT CLEARS THE FLOOR CAN RUN ONE TASK.**
///
/// `min_ram_gb` is a host requirement and `peak_gb_per_task` is what one task needs; they are
/// different quantities — `bx` has a scar from merging them — but they are not independent. A floor
/// below the per-task ceiling would admit a machine that cannot run a single task of this
/// repository, and the placement would then be refused for RAM on a host that passed the RAM check.
#[test]
fn a_host_that_clears_the_ram_floor_can_run_one_task() {
    let decl = read_decl();
    let floor: u64 = decl
        .top
        .get("min_ram_gb")
        .unwrap_or_else(|| panic!("⚠ `{DECL}` declares no `min_ram_gb`"))
        .parse()
        .expect("`min_ram_gb` is a whole number of GiB");
    let peak: u64 = declared_ceiling(&decl);

    assert!(
        floor >= peak,
        "⚠ `min_ram_gb = {floor}` admits a machine that cannot hold one task of \
         `peak_gb_per_task = {peak}`.",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Reading the declaration
// ─────────────────────────────────────────────────────────────────────────────

/// The declaration, split into its top-level scalars and its named tables.
///
/// ⚠ A hand-written reader of a small TOML subset, because this crate takes no dependencies on
/// purpose — see its manifest. It understands exactly what this file uses: comments, `key = value`
/// with a quoted or bare value, and `[table]` headers. Anything richer arriving in the file shows
/// up as a MISSING key and therefore as a red, which is the direction a partial parser is allowed
/// to be wrong in.
struct Decl {
    top: BTreeMap<String, String>,
    tables: BTreeMap<String, BTreeMap<String, String>>,
}

impl Decl {
    fn table(&self, name: &str) -> BTreeMap<String, String> {
        self.tables.get(name).cloned().unwrap_or_else(|| {
            panic!("⚠ `{DECL}` has no `[{name}]` table — register item 456 asks for one.")
        })
    }
}

/// The declared ceiling, for the two gates whose subject is something else.
///
/// Its SHAPE is not their claim — [`the_ram_ceiling_is_declared_as_a_whole_number_of_gigabytes`]
/// owns that — so a malformed field here is reported as "go read that gate" rather than as a
/// second, quieter opinion about the same defect.
fn declared_ceiling(decl: &Decl) -> u64 {
    decl.top
        .get("peak_gb_per_task")
        .and_then(|raw| raw.parse().ok())
        .unwrap_or_else(|| {
            panic!(
                "⚠ `{DECL}` has no usable `peak_gb_per_task`. That is \
             `the_ram_ceiling_is_declared_as_a_whole_number_of_gigabytes`' claim, not this one — \
             read its failure, which says what the field has to look like and why.",
            )
        })
}

fn read_decl() -> Decl {
    let path = workspace_root().join(DECL);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("⚠ cannot read `{}`: {e}", path.display()));

    let mut top = BTreeMap::new();
    let mut tables: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    let mut current: Option<String> = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = Some(name.trim().to_string());
            tables.entry(name.trim().to_string()).or_default();
            continue;
        }
        let Some((key, rest)) = line.split_once('=') else {
            continue;
        };
        let (key, rest) = (key.trim().to_string(), rest.trim());
        let value = match rest.strip_prefix('"') {
            // Basic strings only. This file has no escaped quote in any value, and one arriving
            // would truncate the value and so show up as a mismatch rather than as silence.
            Some(inner) => inner
                .split_once('"')
                .map(|(v, _)| v)
                .unwrap_or(inner)
                .to_string(),
            None => rest.split('#').next().unwrap_or(rest).trim().to_string(),
        };
        match &current {
            Some(t) => {
                tables.entry(t.clone()).or_default().insert(key, value);
            }
            None => {
                top.insert(key, value);
            }
        }
    }

    Decl { top, tables }
}

/// The hook directory, WALKED rather than listed — register item 445's rule, applied to the one
/// place this repository hands work to the wrapper.
const HOOKS: &str = ".githooks";

/// Every `"${BX}"` invocation under [`HOOKS`], as `(path:line, the command it hands over)`.
fn wrapper_call_sites() -> Vec<(String, String)> {
    let dir = workspace_root().join(HOOKS);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|why| panic!("{} must be readable: {why}", dir.display()))
        .map(|entry| entry.expect("a hook entry").path())
        .collect();
    files.sort();

    let mut found = Vec::new();
    for path in files {
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|why| panic!("{} must be text: {why}", path.display()));
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        for (index, line) in text.lines().enumerate() {
            let code = line.trim();
            if code.starts_with('#') || !code.contains("\"${BX}\"") {
                continue;
            }
            let site = format!("{HOOKS}/{name}:{}", index + 1);
            // ⛔⛔⛔ A CALL SITE IS THE WRAPPER IN A **COMMAND POSITION**, and getting to that rule
            // took two wrong ones — both of which a mutation caught.
            //
            //   1. *skip every `"${BX}"` line without ` -- `* — reads as "skip the tests" and is
            //      not: it makes a shape this cannot compose a SILENT PASS, the one thing this
            //      clause's own doc says it must never be.
            //   2. *skip lines starting `[` or `if [`* — a mutation of the form
            //      `[ -n "${BX:-}" ] && "${BX}" --explain-declaration` walked straight through it,
            //      because that line both tests AND invokes.
            //
            // So the question is asked of the OCCURRENCE, not the line: what comes before it. At
            // the start of the line, or after `&&`, `||`, `;`, `then`, `else`, `do`, `(` or `{`,
            // the wrapper is being RUN. Inside `[ -x … ]` it is an argument to a test and hands
            // over nothing. The wrapper refuses an argv that does not follow `--` (`die "unknown
            // flag $1 (commands go after --)"`), so a run without one is a line somebody reads.
            let Some(at) = code.find("\"${BX}\"") else {
                continue;
            };
            let before = code[..at].trim_end();
            let runs = before.is_empty()
                || ["&&", "||", ";", "then", "else", "do", "(", "{"]
                    .iter()
                    .any(|lead| before.ends_with(lead));
            if !runs {
                continue;
            }
            let argv = code
                .split_once(" -- ")
                .map(|(_, tail)| tail.trim())
                .unwrap_or_else(|| panic!(
                    "⚠ {site} names the wrapper and separates no argv with ` -- `, and it is not a \
                     test expression either. The wrapper takes its command after `--` and refuses \
                     anything else, so this clause cannot say what would be handed over — and a \
                     command it cannot name is one nobody measured (rule 6). The line:\n{code}",
                ));
            found.push((site.clone(), compose(&text, argv, &site)));
        }
    }
    found
}

/// What a `bash -c "…"` argv actually runs, with the hook's own shell variables resolved.
fn compose(text: &str, argv: &str, site: &str) -> String {
    let inner = argv
        .strip_prefix("bash -c ")
        .or_else(|| argv.strip_prefix("sh -c "))
        .unwrap_or_else(|| {
            panic!(
                "⚠ {site} hands the wrapper an argv this clause cannot read: `{argv}`. It knows \
             `bash -c \"…\"` and `sh -c \"…\"`, which is every shape this repository has used. A \
             shape it does not know is a RED rather than a skip — rule 6 — because the claim here \
             is that nothing reaches the wrapper unmeasured.",
            )
        });
    let inner = inner
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_else(|| {
            panic!(
                "⚠ {site} hands `bash -c` something that is not one double-quoted word: `{inner}`. \
             Composing it would be guessing at the caller's quoting.",
            )
        });
    expand(text, inner, site)
}

/// `$name` and `${name}` replaced by what the same file assigns them.
///
/// Pure over `(text, spelled)`, so [`the_composer_answers_both_ways`] can drive every arm from a
/// literal instead of hoping the hooks happen to hold one of each.
fn expand(text: &str, spelled: &str, site: &str) -> String {
    let mut out = String::new();
    let mut rest = spelled;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, tail) = match after.strip_prefix('{') {
            Some(braced) => {
                let close = braced
                    .find('}')
                    .unwrap_or_else(|| panic!("⚠ {site}: `${{` with no `}}` in `{spelled}`"));
                (&braced[..close], &braced[close + 1..])
            }
            None => {
                let end = after
                    .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                    .unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        out.push_str(&assignment(text, name).unwrap_or_else(|| {
            panic!(
                "⚠ {site} hands over `${name}`, and nothing in that file assigns it as one quoted \
             word. The clause cannot say what the wrapper will be given, so it says so — a command \
             it cannot name is one nobody measured.",
            )
        }));
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The value of `name='…'` or `name="…"` where that is one quoted word on one line.
fn assignment(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        let Some(rest) = line
            .trim()
            .strip_prefix(name)
            .and_then(|rest| rest.strip_prefix('='))
        else {
            continue;
        };
        for quote in ['\'', '"'] {
            if let Some(inner) = rest.strip_prefix(quote).and_then(|r| r.strip_suffix(quote)) {
                return Some(inner.to_string());
            }
        }
    }
    None
}

/// ⛔⛔⛔ **AND THE COMPOSER ANSWERS BOTH WAYS** — register item 908's lesson: a clause that passes
/// by composing one command it happens to recognise is a green about nothing.
#[test]
fn the_composer_answers_both_ways() {
    let text = "a='one two'\nb=\"three\"\n";
    assert_eq!(expand(text, "$a && $b", "fixture"), "one two && three");
    assert_eq!(expand(text, "${a}x", "fixture"), "one twox");
    assert_eq!(
        expand(text, "no variables here", "fixture"),
        "no variables here"
    );
    assert_eq!(assignment(text, "missing"), None);
    assert_eq!(
        assignment("c=bare\n", "c"),
        None,
        "an unquoted word is not one this reads"
    );
}
