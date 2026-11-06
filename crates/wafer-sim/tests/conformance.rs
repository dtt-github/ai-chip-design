//! Differential tests: each `tests/conformance/*.v` must print exactly what
//! Icarus Verilog printed for it (`*.out`, from scripts/gen-sim-expected.sh).

use std::path::Path;

#[test]
fn matches_icarus_verilog() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/conformance");
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "v"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    let mut failures = Vec::new();
    for f in &files {
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(f).unwrap();
        let expected = std::fs::read_to_string(f.with_extension("out")).unwrap();
        let expected: Vec<&str> = expected.lines().collect();
        match wafer_sim::simulate(&[(&name, &src)], None, wafer_sim::Limits::default()) {
            Ok(r) => {
                let got: Vec<&str> = r.log.iter().map(String::as_str).collect();
                if got != expected {
                    let first =
                        got.iter().zip(&expected).position(|(a, b)| a != b).unwrap_or(got.len().min(expected.len()));
                    failures.push(format!(
                        "{name}: first difference at line {}:\n  wafer-sim:  {:?}\n  icarus:  {:?}\n  ({} vs {} lines)",
                        first + 1,
                        got.get(first),
                        expected.get(first),
                        got.len(),
                        expected.len()
                    ));
                }
            }
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    assert!(failures.is_empty(), "{} of {} differ:\n{}", failures.len(), files.len(), failures.join("\n"));
}
