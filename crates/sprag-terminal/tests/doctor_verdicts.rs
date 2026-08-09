//! Every environment check's verdict, driven from a captured machine.
//!
//! The whole point of [`sprag_terminal::doctor`]'s split is that these run with no filesystem: no
//! test suite can arrange a box that is swapping, oversubscribed, missing a controller and losing
//! its CPU to a CI runner at once, and a check that could only be exercised on such a box would
//! never be exercised at all. The capture half — the file reads that produce a `Readings` — is
//! driven against a fake `/proc` and a fake cgroup tree in the module's own tests.
//!
//! Each check gets its DEGRADED case and its HEALTHY case from the same fixture with one field
//! moved, so what is being asserted is the criterion and not the fixture: if the two readings did
//! not disagree, both would pass whatever the code did.

use std::time::Duration;

use sprag_terminal::doctor::{
    Blind, Ccache, Check, Diagnosis, Evidence, Finding, Level, Load, Measurement, PaneReading,
    Readings, Sibling, SubtreeReading, Verdict,
};
use sprag_terminal::{Cpu, PaneId, Percent, Pressure, Waiting};

/// A machine with nothing wrong with it, for one field at a time to be moved off.
fn healthy() -> Readings {
    Readings {
        cpu: Some(pressure(1_00, 0)),
        io: Some(pressure(1_00, 0)),
        memory: Some(pressure(0, 0)),
        swappiness: Some(60),
        load: Some(Load {
            runnable: 3,
            threads: 2295,
            cores: 32,
            pane_procs: 12,
        }),
        panes: vec![
            pane(1, "/sprag.scope/pane-1", 300),
            // Not the same figure as its neighbour: with both equal, "the worst pane" would be
            // whichever the fold happened to keep and the assertion would discriminate nothing.
            pane(2, "/sprag.scope/pane-2", 1_200),
        ],
        subtree: Some(SubtreeReading {
            root: "/sys/fs/cgroup/user.slice/sprag.scope".to_owned(),
            available: controllers(&["cpu", "memory", "pids"]),
            enabled: controllers(&["cpu", "memory", "pids"]),
            above: vec![Level {
                at: "/sys/fs/cgroup/user.slice".to_owned(),
                ours: "sprag.scope".to_owned(),
                children: vec![
                    sibling("sprag.scope", Some(100), 4_000),
                    sibling("idle.scope", Some(100), 0),
                ],
            }],
        }),
        ccache: Some(Ccache {
            shims: Some(("/usr/lib/ccache".to_owned(), 33)),
            max_size: Some("50.0 GB".to_owned()),
            depend_mode: Some(true),
            hit_rate: Some(Percent::from_hundredths(7982)),
            cleanups: Some(0),
            occupancy: Some(Percent::from_hundredths(788)),
        }),
        linkers: vec!["mold".to_owned()],
        paths: 1,
        hierarchy: true,
    }
}

fn pressure(some_hundredths: u32, full_hundredths: u32) -> Pressure {
    Pressure {
        some: row(some_hundredths),
        full: row(full_hundredths),
    }
}

fn row(hundredths: u32) -> Waiting {
    Waiting::Measured {
        avg10: Percent::from_hundredths(hundredths),
        avg60: Percent::from_hundredths(hundredths),
        avg300: Percent::from_hundredths(hundredths),
    }
}

fn pane(id: u64, cgroup: &str, waiting_hundredths: u32) -> PaneReading {
    PaneReading {
        id: PaneId(id),
        cgroup: Some(cgroup.to_owned()),
        swapped: Some(0),
        waiting: row(waiting_hundredths),
        ccache_on_path: Some(true),
    }
}

fn sibling(name: &str, weight: Option<u32>, millicores: u64) -> Sibling {
    Sibling {
        name: name.to_owned(),
        weight,
        took: Cpu::Held {
            millicores,
            over_ms: 500,
        },
    }
}

fn controllers(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

/// One check's answer, and the words it printed — both, because a verdict without its evidence is
/// the thing this module exists to make impossible.
fn judged(check: Check, readings: &Readings) -> (Verdict, String) {
    let finding = check.judge(readings);
    assert_eq!(finding.check, check, "a check answers about itself");
    (finding.verdict, said(&finding))
}

/// Everything a finding measured, as one searchable line.
fn said(finding: &Finding) -> String {
    finding
        .evidence
        .rows()
        .map(|Measurement { of, is }| format!("{of}={is}"))
        .collect::<Vec<_>>()
        .join("; ")
}

// ── the report itself ───────────────────────────────────────────────────────────────────────────

/// Every check is answered, exactly once, in the closed set's own order.
///
/// The ratchet: a check added to the enum is in every report the day it compiles. A hand-written
/// list is the one a new thing gets left out of, which is the failure mode a ratchet exists to
/// prevent and which this project has met three times.
#[test]
fn a_diagnosis_answers_every_check_in_the_sets_own_order() {
    let report = Diagnosis::of(&healthy());
    assert_eq!(
        report
            .findings
            .iter()
            .map(|finding| finding.check)
            .collect::<Vec<_>>(),
        Check::ALL.to_vec(),
    );
    assert_eq!(Check::ALL.len(), 11, "the checks this round built");
}

/// A healthy machine has nothing degraded, and every row still carries what it measured.
#[test]
fn a_healthy_machine_reports_no_fault_and_still_shows_its_working() {
    let report = Diagnosis::of(&healthy());
    assert_eq!(
        report.degraded().map(|f| f.check).collect::<Vec<_>>(),
        Vec::<Check>::new(),
        "findings: {:?}",
        report.findings,
    );
    for finding in &report.findings {
        assert!(
            finding.evidence.rows().count() > 0,
            "{:?} printed no measurement",
            finding.check,
        );
    }
}

/// A machine with nothing on it at all is BLIND everywhere and clean nowhere.
///
/// The distinction the design turns on: a report that dropped the rows it could not read would be
/// indistinguishable from one where everything passed, and a person acting on it would stop
/// looking.
#[test]
fn an_unreadable_machine_is_blind_rather_than_clean() {
    let report = Diagnosis::of(&Readings::default());
    assert_eq!(
        report.findings.len(),
        Check::ALL.len(),
        "no row disappears when its source does",
    );
    assert_eq!(
        report.degraded().count(),
        0,
        "and nothing it could not read is called a fault",
    );
    for finding in &report.findings {
        assert!(
            matches!(finding.verdict, Verdict::Blind(_)),
            "{:?} answered {:?} about a machine it could not read",
            finding.check,
            finding.verdict,
        );
    }
}

/// Every check names its source, its criterion and a remedy — the design's rule that advice a
/// person cannot check is advice they have to take on faith.
#[test]
fn every_check_says_what_it_read_and_what_would_make_it_fail() {
    for check in Check::ALL {
        let entry = check.entry();
        for (field, text) in [
            ("name", entry.name),
            ("asks", entry.asks),
            ("source", entry.source),
            ("criterion", entry.criterion),
            ("remedy", entry.remedy),
        ] {
            assert!(!text.is_empty(), "{check:?} has an empty {field}");
        }
        assert_eq!(
            serde_json::to_value(check).expect("a check serialises"),
            serde_json::Value::String(entry.name.to_owned()),
            "the name a report prints and the name that crosses the wire are ONE spelling",
        );
    }
}

/// A verdict cannot be built, or received, with nothing measured behind it.
#[test]
fn a_finding_with_no_evidence_does_not_exist() {
    let wire = serde_json::json!({
        "check": "cpu-stall",
        "verdict": "degraded",
        "evidence": [],
    });
    let refused = serde_json::from_value::<Finding>(wire).unwrap_err();
    assert!(
        refused.to_string().contains("nothing measured"),
        "the refusal says why: {refused}",
    );
    let accepted = serde_json::from_value::<Finding>(serde_json::json!({
        "check": "cpu-stall",
        "verdict": "degraded",
        "evidence": [{"of": "machine waiting (some, 60s)", "is": "84.09%"}],
    }))
    .expect("one measurement is enough");
    assert_eq!(said(&accepted), "machine waiting (some, 60s)=84.09%");
}

/// A whole report survives the wire unchanged, evidence and all.
#[test]
fn a_diagnosis_round_trips() {
    let report = Diagnosis::of(&healthy());
    let text = serde_json::to_string(&report).expect("serialise");
    assert_eq!(
        serde_json::from_str::<Diagnosis>(&text).expect("deserialise"),
        report,
    );
}

// ── the checks ──────────────────────────────────────────────────────────────────────────────────

/// Two panes in one cgroup is the defect this whole layer was reported for: CPU is then divided per
/// PROCESS, so the pane running `make -j32` takes thirty-two times its neighbour's share and
/// nothing the person sets can change it.
#[test]
fn two_panes_in_one_cgroup_is_the_fault_the_layer_exists_for() {
    let mut shared = healthy();
    shared.panes[1].cgroup = shared.panes[0].cgroup.clone();
    let (verdict, said) = judged(Check::PaneIsolation, &shared);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("pane 1, pane 2") && said.contains("/sprag.scope/pane-1"),
        "it names which panes and which cgroup: {said}",
    );

    let (verdict, said) = judged(Check::PaneIsolation, &healthy());
    assert_eq!(verdict, Verdict::Healthy);
    assert!(
        said.contains("distinct cgroups=2"),
        "and counts them either way: {said}",
    );
}

/// A pane whose cgroup could not be read at all is BLIND, not isolated: nobody looked.
#[test]
fn panes_with_no_cgroup_path_are_not_reported_as_isolated() {
    let mut blind = healthy();
    for pane in &mut blind.panes {
        pane.cgroup = None;
    }
    assert_eq!(
        judged(Check::PaneIsolation, &blind).0,
        Verdict::Blind(Blind::NoHierarchy),
    );
}

/// Without the cpu controller no pane can carry a weight, so every share setting is inert — and the
/// io controller's absence is reported as a consequence rather than as a fault, because this daemon
/// does not use it.
#[test]
fn a_subtree_without_the_cpu_controller_can_enforce_nothing() {
    let mut inert = healthy();
    inert
        .subtree
        .as_mut()
        .expect("a subtree")
        .enabled
        .retain(|name| name != "cpu");
    let (verdict, said) = judged(Check::ControllerDelegation, &inert);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("enabled=memory pids"),
        "the list is printed, so the reader can see what IS there: {said}",
    );

    let (verdict, said) = judged(Check::ControllerDelegation, &healthy());
    assert_eq!(
        verdict,
        Verdict::Healthy,
        "io is absent here too and that is not a fault: {said}",
    );
    assert!(
        said.contains("io=not delegated"),
        "but it is said out loud, with what it costs: {said}",
    );
}

/// The design's first worked example, encoded: a weight compares only among SIBLINGS, so a level
/// where nothing else ran is not competition however the weights read, and a level where a
/// stranger is taking cores at equal weight is — whatever this daemon sets below it.
#[test]
fn a_neighbour_taking_cores_at_equal_weight_is_the_competition() {
    let mut contested = healthy();
    contested.subtree.as_mut().expect("a subtree").above[0].children = vec![
        sibling("sprag.scope", Some(100), 1_450),
        sibling("ci.scope", Some(100), 6_600),
    ];
    let (verdict, said) = judged(Check::CompetingWeight, &contested);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("competing=ci.scope weight 100, took 6.60 cores over 0.5s"),
        "the rival is named with its weight AND what it actually took: {said}",
    );
    assert!(
        said.contains("ours=sprag.scope weight 100, took 1.45 cores"),
        "beside ours, so the two are compared at one level: {said}",
    );

    let (verdict, said) = judged(Check::CompetingWeight, &healthy());
    assert_eq!(
        verdict,
        Verdict::Healthy,
        "an idle neighbour at the same weight is not competition: {said}",
    );
}

/// A weight that reads worse and takes nothing is still not competition. The number that decides is
/// what was TAKEN, because a nominal ratio is not a measured one — 10:100 measured 18:82.
#[test]
fn an_idle_neighbour_at_a_better_weight_is_still_not_competition() {
    let mut idle_rival = healthy();
    idle_rival.subtree.as_mut().expect("a subtree").above[0].children = vec![
        sibling("sprag.scope", Some(10), 8_000),
        sibling("batch.scope", Some(10_000), 0),
    ];
    assert_eq!(
        judged(Check::CompetingWeight, &idle_rival).0,
        Verdict::Healthy
    );
}

/// A level with no weights anywhere is one the kernel is not arbitrating at all, so a busy stranger
/// there is the worst case rather than the exempt one.
#[test]
fn an_unweighted_level_with_a_busy_stranger_is_degraded() {
    let mut unweighted = healthy();
    unweighted.subtree.as_mut().expect("a subtree").above[0].children = vec![
        sibling("sprag.scope", None, 1_000),
        sibling("ci.scope", None, 7_000),
    ];
    let (verdict, said) = judged(Check::CompetingWeight, &unweighted);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("competing=ci.scope no weight"),
        "and it says the weight is absent rather than printing a zero: {said}",
    );
}

/// The machine's own stall, with the worst pane's beside it — because a machine stalling while one
/// pane holds all of it is a different problem from one stalling evenly.
#[test]
fn the_machines_cpu_stall_is_judged_with_the_worst_panes_beside_it() {
    let mut stalled = healthy();
    // The design's own measured numbers: 88.69% over ten seconds against 49.96% over five
    // minutes. The two windows have to differ or the row cannot show that a minute was a burst.
    stalled.cpu = Some(Pressure {
        some: Waiting::Measured {
            avg10: Percent::from_hundredths(88_69),
            avg60: Percent::from_hundredths(84_09),
            avg300: Percent::from_hundredths(49_96),
        },
        full: row(0),
    });
    let (verdict, said) = judged(Check::CpuStall, &stalled);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("machine waiting (some, 60s)=84.09%")
            && said.contains("limit=50.00%")
            && said.contains("and over 5 minutes=49.96%")
            && said.contains("worst pane=pane 2 at 12.00%"),
        "the reading, the bar it was judged against, and the WORST pane — not the first: \
         {said}",
    );

    assert_eq!(judged(Check::CpuStall, &healthy()).0, Verdict::Healthy);
}

/// A kernel that keeps no pressure accounting reports no number, and a zero there would say *this
/// machine never waited* about a machine that may have waited for everything.
#[test]
fn a_kernel_without_pressure_accounting_is_blind_not_calm() {
    let mut no_psi = healthy();
    no_psi.cpu = Some(Pressure::NONE);
    assert_eq!(
        judged(Check::CpuStall, &no_psi).0,
        Verdict::Blind(Blind::NoAccounting),
    );
    no_psi.cpu = None;
    assert_eq!(
        judged(Check::CpuStall, &no_psi).0,
        Verdict::Blind(Blind::NoAccounting),
    );
}

/// `full` above zero on the machine is not a slow disk — it is every runnable task on the box
/// parked at once. `some` alone is not, which is why the row that decides is `full`.
#[test]
fn io_is_judged_on_the_full_row_and_not_on_some() {
    let mut stopped = healthy();
    stopped.io = Some(pressure(40_00, 7_55));
    let (verdict, said) = judged(Check::IoStall, &stopped);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("stopped (full, 60s)=7.55%") && said.contains("waiting (some, 60s)=40.00%"),
        "both rows are printed and only one decides: {said}",
    );
    assert!(
        said.contains("arbitrable between panes=no"),
        "and it says whether anything could be done about it here: {said}",
    );

    let mut busy = healthy();
    busy.io = Some(pressure(40_00, 0));
    assert_eq!(
        judged(Check::IoStall, &busy).0,
        Verdict::Healthy,
        "a busy disk that never stopped the machine is a busy disk",
    );
}

/// The same row, the same rule, for memory — and the delegation line flips with the controller.
#[test]
fn memory_is_judged_on_the_full_row_too() {
    let mut reclaiming = healthy();
    reclaiming.memory = Some(pressure(10_00, 2_00));
    let (verdict, said) = judged(Check::MemoryStall, &reclaiming);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("arbitrable between panes=yes"),
        "memory IS delegated here, unlike io: {said}",
    );
    assert_eq!(judged(Check::MemoryStall, &healthy()).0, Verdict::Healthy);
}

/// The reading that made the threshold exist: an ordinary idle box measured
/// `/proc/pressure/memory` `full avg60` at **0.09%**, and the design's own criterion of *above
/// zero* called that a fault. Fifty-four milliseconds across a minute is nobody's complaint, and a
/// row that is red on a healthy machine is a row nobody reads on the day it matters.
///
/// Both sides of the bar, from the numbers the shipped command actually printed: 0.09% is clean and
/// the same machine's 14.02% disk stall is not.
#[test]
fn a_stall_too_small_to_feel_is_not_a_fault_and_the_bar_is_printed() {
    let mut idle = healthy();
    idle.memory = Some(pressure(12, 9));
    let (verdict, said) = judged(Check::MemoryStall, &idle);
    assert_eq!(verdict, Verdict::Healthy);
    assert!(
        said.contains("stopped (full, 60s)=0.09%") && said.contains("limit=1.00%"),
        "the reading AND the bar it was judged against, so a reader can disagree: {said}",
    );

    let mut stopping = healthy();
    stopping.io = Some(pressure(16_20, 14_02));
    assert_eq!(judged(Check::IoStall, &stopping).0, Verdict::Degraded);

    let mut exactly_at_the_bar = healthy();
    exactly_at_the_bar.io = Some(pressure(16_20, 1_00));
    assert_eq!(
        judged(Check::IoStall, &exactly_at_the_bar).0,
        Verdict::Degraded,
        "at the limit, not merely past it — the same inclusive bound the cpu row uses",
    );
}

/// The swap setting is printed and the swapped PAGES are the verdict: a number with nothing behind
/// it is a number, not a fault.
#[test]
fn swapping_is_judged_on_the_pages_and_not_on_the_setting() {
    let mut swapped = healthy();
    swapped.panes[0].swapped = Some(104 * 1024 * 1024);
    swapped.panes[1].swapped = Some(128 * 1024 * 1024);
    let (verdict, said) = judged(Check::Swapping, &swapped);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("total swapped=232.0 MiB")
            && said.contains("vm.swappiness=60")
            && said.contains("which=pane 1, pane 2"),
        "the effect, the setting and which panes: {said}",
    );

    let mut eager = healthy();
    eager.swappiness = Some(100);
    let (verdict, said) = judged(Check::Swapping, &eager);
    assert_eq!(
        verdict,
        Verdict::Healthy,
        "a high swappiness with nothing swapped is not a fault: {said}",
    );
    assert!(
        said.contains("vm.swappiness=100"),
        "but it is printed: {said}"
    );
}

/// Twice as many runnable tasks as cores is where a scheduler stops being busy and starts queueing.
#[test]
fn oversubscription_is_runnable_against_cores() {
    let mut saturated = healthy();
    saturated.load = Some(Load {
        runnable: 151,
        threads: 2295,
        cores: 32,
        pane_procs: 140,
    });
    let (verdict, said) = judged(Check::BuildSaturation, &saturated);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("runnable=151")
            && said.contains("cores=32")
            && said.contains("processes in panes=140"),
        "the machine's numbers and the panes' share of them: {said}",
    );

    let mut busy = healthy();
    busy.load = Some(Load {
        runnable: 40,
        threads: 2295,
        cores: 32,
        pane_procs: 30,
    });
    assert_eq!(
        judged(Check::BuildSaturation, &busy).0,
        Verdict::Healthy,
        "more runnable than cores is a busy machine; twice as many is a queue",
    );
}

/// A host whose core count could not be read judges nothing, because zero cores would make every
/// machine oversubscribed.
#[test]
fn an_uncounted_machine_is_not_oversubscribed_by_arithmetic() {
    let mut uncounted = healthy();
    uncounted.load = Some(Load {
        runnable: 151,
        threads: 2295,
        cores: 0,
        pane_procs: 140,
    });
    assert_eq!(
        judged(Check::BuildSaturation, &uncounted).0,
        Verdict::Healthy,
    );
}

/// The compiler cache installed and walked past — the design's case, where 33 shims existed and no
/// shell's PATH named them.
#[test]
fn shims_that_no_pane_can_reach_are_a_bypassed_cache() {
    let mut bypassed = healthy();
    for pane in &mut bypassed.panes {
        pane.ccache_on_path = Some(false);
    }
    let (verdict, said) = judged(Check::CcacheOnPath, &bypassed);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("shims=33 in /usr/lib/ccache")
            && said.contains("panes started with it on PATH=0/2"),
        "how many shims, and how many panes reach them: {said}",
    );

    let (verdict, said) = judged(Check::CcacheOnPath, &healthy());
    assert_eq!(verdict, Verdict::Healthy);
    assert!(said.contains("2/2"), "{said}");
}

/// A pane whose PATH could not be read is not a pane that bypassed the cache.
#[test]
fn unreadable_paths_make_the_cache_check_blind() {
    let mut unread = healthy();
    for pane in &mut unread.panes {
        pane.ccache_on_path = None;
    }
    assert_eq!(
        judged(Check::CcacheOnPath, &unread).0,
        Verdict::Blind(Blind::NoPanes),
    );
}

/// A host with no ccache at all has nothing to judge — and saying *degraded* there would be
/// prescribing an install nobody asked for.
#[test]
fn a_host_without_ccache_is_blind_rather_than_faulty() {
    let mut none = healthy();
    none.ccache = None;
    assert_eq!(
        judged(Check::CcacheOnPath, &none).0,
        Verdict::Blind(Blind::NotInstalled),
    );
    assert_eq!(
        judged(Check::CcacheSizing, &none).0,
        Verdict::Blind(Blind::NotInstalled),
    );
}

/// A cleanup is the cache throwing away what was paid for, so any cleanup at all means the working
/// set does not fit.
#[test]
fn a_cache_that_has_ever_evicted_is_too_small_for_what_goes_through_it() {
    let mut evicting = healthy();
    evicting.ccache.as_mut().expect("ccache").cleanups = Some(388);
    let (verdict, said) = judged(Check::CcacheSizing, &evicting);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("cleanups=388")
            && said.contains("max_size=50.0 GB")
            && said.contains("hit rate=79.82%")
            && said.contains("cache is full=7.88%")
            && said.contains("depend_mode=on"),
        "the count, the ceiling it was evicting under, how full it is now, and what it is worth — \
         388 evictions against a cache at 8% is history, and against one at 99% is happening: \
         {said}",
    );

    assert_eq!(judged(Check::CcacheSizing, &healthy()).0, Verdict::Healthy);
}

/// A link is the one build step that cannot be parallelised away, so whether a fast linker is
/// reachable at all is a fact about every build in every pane.
#[test]
fn no_fast_linker_on_any_panes_path_is_reported() {
    let mut slow = healthy();
    slow.linkers.clear();
    let (verdict, said) = judged(Check::FastLinker, &slow);
    assert_eq!(verdict, Verdict::Degraded);
    assert!(
        said.contains("PATHs searched=1") && said.contains("fast linkers found=none"),
        "what was searched, and what was found: {said}",
    );

    let (verdict, said) = judged(Check::FastLinker, &healthy());
    assert_eq!(verdict, Verdict::Healthy);
    assert!(said.contains("fast linkers found=mold"), "{said}");
}

/// A daemon with no panes has no PATH to search, which is not a machine without a linker.
#[test]
fn no_panes_means_no_path_to_search() {
    let mut empty = healthy();
    empty.paths = 0;
    empty.panes.clear();
    assert_eq!(
        judged(Check::FastLinker, &empty).0,
        Verdict::Blind(Blind::NoPanes),
    );
    assert_eq!(
        judged(Check::PaneIsolation, &empty).0,
        Verdict::Blind(Blind::NoPanes),
    );
}

/// A hierarchy this daemon was never given a subtree in is a different absence from one that is not
/// there at all, and an operator responds to them differently.
#[test]
fn no_subtree_and_no_hierarchy_are_different_absences() {
    let mut unplaced = healthy();
    unplaced.subtree = None;
    assert_eq!(
        judged(Check::ControllerDelegation, &unplaced).0,
        Verdict::Blind(Blind::NoSubtree),
    );
    assert_eq!(
        judged(Check::CompetingWeight, &unplaced).0,
        Verdict::Blind(Blind::NoSubtree),
    );
    unplaced.hierarchy = false;
    assert_eq!(
        judged(Check::ControllerDelegation, &unplaced).0,
        Verdict::Blind(Blind::NoHierarchy),
    );
}

/// The evidence builder keeps every reading, in the order it was added, head first.
#[test]
fn evidence_keeps_what_it_was_given_in_order() {
    let evidence = Evidence::of("first", "1")
        .and("second", "2")
        .and("third", "3");
    assert_eq!(
        evidence
            .rows()
            .map(|row| row.of.as_str())
            .collect::<Vec<_>>(),
        vec!["first", "second", "third"],
    );
}

/// A capture on a machine with nothing on it produces a reading rather than a panic, and the whole
/// report comes back blind. The window is not slept through: a zero-length one is honest here
/// because there is nothing under the (absent) mount point to sample twice.
#[test]
fn a_capture_against_nothing_still_answers() {
    let sources = sprag_terminal::doctor::Sources {
        proc: "/nonexistent-proc".into(),
        cgroup: Some("/nonexistent-cgroup".into()),
        shims: "/nonexistent-shims".into(),
        ccache: None,
    };
    let readings = Readings::capture(
        &sprag_terminal::doctor::Subject::default(),
        &sources,
        Duration::ZERO,
    );
    assert_eq!(readings, Readings::default());
    assert_eq!(Diagnosis::of(&readings).degraded().count(), 0);
}
