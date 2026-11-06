//! Report extraction against a real KLayout DRC report (IHP SG13G2 deck run on a
//! layout with three deliberate violations: M1 width, M2 spacing, Via1 size).

use serde_json::json;
use wafer_jobs::reports::{drc_report, run_report};

#[test]
fn drc_report_from_run_directory() {
    let run = tempfile::tempdir().unwrap();
    let reports = run.path().join("65-klayout-drc/reports");
    std::fs::create_dir_all(&reports).unwrap();
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/drc.klayout.lyrdb"),
        reports.join("drc.klayout.lyrdb"),
    )
    .unwrap();
    // An older step with the same name must be ignored in favour of the newest one.
    std::fs::create_dir_all(run.path().join("12-klayout-drc/reports")).unwrap();
    std::fs::write(run.path().join("12-klayout-drc/reports/drc.klayout.lyrdb"), "<report-database/>").unwrap();

    let r = drc_report(run.path());
    assert_eq!(r.sources, vec!["65-klayout-drc/reports/drc.klayout.lyrdb"]);
    assert_eq!(r.violations.len(), 4);
    let m1 = r.violations.iter().find(|v| v.rule == "M1.a").unwrap();
    assert_eq!(m1.source, "klayout");
    assert!(m1.description.contains("Min. Metal1 width"));
    assert_eq!(m1.bbox, Some([2.0, 5.0, 2.1, 9.0]));
    let counts: Vec<(&str, usize)> = r.counts.iter().map(|c| (c.rule.as_str(), c.count)).collect();
    assert_eq!(counts, vec![("M1.a", 1), ("M2.b", 1), ("V1.a", 2)]);
    assert!(!r.truncated);
}

#[test]
fn empty_run_has_no_reports() {
    let run = tempfile::tempdir().unwrap();
    let r = drc_report(run.path());
    assert!(r.violations.is_empty() && r.sources.is_empty());
    let rep = run_report(run.path(), &json!({}));
    assert!(rep.corners.is_empty() && rep.paths.is_empty());
}

#[test]
fn corners_and_classes_from_metrics() {
    let run = tempfile::tempdir().unwrap();
    let metrics = json!({
        "timing__setup__ws__corner:slow": 13.7, "timing__setup__ws__corner:fast": 14.9,
        "timing__hold__ws__corner:slow": 0.77, "timing__hold__ws__corner:fast": 0.18,
        "timing__setup_vio__count__corner:slow": 0,
        "design__instance__count__class:sequential_cell": 8, "design__instance__area__class:sequential_cell": 391.9,
        "design__instance__count__class:inverter": 2, "design__instance__area__class:inverter": 10.9,
        "design__die__area": 2950.05, "design__instance__utilization": 0.91
    });
    let r = run_report(run.path(), &metrics);
    assert_eq!(r.corners.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), vec!["fast", "slow"]);
    assert_eq!(r.corners[1].setup_violations, Some(0.0));
    assert_eq!(r.classes[0].class, "sequential_cell");
    assert_eq!(r.die_area_um2, Some(2950.05));
}
