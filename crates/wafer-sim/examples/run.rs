//! Simulates files from the command line, like `iverilog` + `vvp`:
//! cargo run -p wafer-sim --example run -- tb.v dut.v
fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    let srcs: Vec<(String, String)> = paths
        .iter()
        .map(|p| (p.rsplit('/').next().unwrap_or(p).to_string(), std::fs::read_to_string(p).expect("readable file")))
        .collect();
    let files: Vec<(&str, &str)> = srcs.iter().map(|(n, s)| (n.as_str(), s.as_str())).collect();
    match wafer_sim::simulate(&files, None, wafer_sim::Limits::default()) {
        Ok(r) => {
            for l in &r.log {
                println!("{l}");
            }
            eprintln!("[finished={} time={} steps={} stopped={:?}]", r.finished, r.time, r.steps, r.stopped);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}
