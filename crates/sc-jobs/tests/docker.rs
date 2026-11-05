//! Runs the real LibreLane image. Needs Docker, the image and the PDK volume:
//!   docker pull ghcr.io/librelane/librelane:3.0.16
//!   scripts/fetch-pdk.sh
//! Then: cargo test -p sc-jobs --test docker -- --ignored

use std::time::Duration;

use sc_jobs::{Backend, CancellationToken, DockerBackend, DockerConfig, JobSpec, Outcome, final_dir, summarize};

const COUNTER: &str = "`default_nettype none
module counter (input wire clk, input wire rst_n, output reg [7:0] count);
    always @(posedge clk or negedge rst_n)
        if (!rst_n) count <= 8'd0; else count <= count + 8'd1;
endmodule
";

fn config() -> DockerConfig {
    DockerConfig {
        docker: "docker".into(),
        image: std::env::var("SC_LIBRELANE_IMAGE").unwrap_or_else(|_| "ghcr.io/librelane/librelane:3.0.16".into()),
        pdk_volume: std::env::var("SC_PDK_VOLUME").unwrap_or_else(|_| "siliconcommons_pdk".into()),
        cpus: 4.0,
        memory: "8g".into(),
        user: Some(format!("{}:{}", uid(), gid())),
    }
}

fn uid() -> String {
    String::from_utf8(std::process::Command::new("id").arg("-u").output().unwrap().stdout).unwrap().trim().into()
}

fn gid() -> String {
    String::from_utf8(std::process::Command::new("id").arg("-g").output().unwrap().stdout).unwrap().trim().into()
}

fn workdir(name: &str) -> (tempfile::TempDir, JobSpec) {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(work.join("src")).unwrap();
    std::fs::write(work.join("src/counter.v"), COUNTER).unwrap();
    std::fs::write(
        work.join("config.json"),
        r#"{"DESIGN_NAME": "counter", "VERILOG_FILES": "dir::src/counter.v", "CLOCK_PORT": "clk", "CLOCK_PERIOD": 20, "FP_CORE_UTIL": 40}"#,
    )
    .unwrap();
    let spec = JobSpec {
        id: format!("test-{name}-{}", std::process::id()),
        workdir: work,
        config: "config.json".into(),
        pdk: "ihp-sg13g2".into(),
        log_path: dir.path().join("flow.log"),
    };
    (dir, spec)
}

#[tokio::test]
#[ignore = "runs LibreLane in Docker (~1 min)"]
async fn counter_to_gds() {
    let backend = DockerBackend::new(config()).await.unwrap();
    assert!(!backend.toolchain().is_empty());
    let (_dir, spec) = workdir("ok");
    let outcome = backend.run(&spec, CancellationToken::new()).await.unwrap();
    let log = std::fs::read_to_string(&spec.log_path).unwrap();
    assert_eq!(outcome, Outcome::Exited(0), "{}", &log[log.len().saturating_sub(4000)..]);
    assert!(log.contains("Flow complete"));
    let fin = final_dir(&spec.workdir);
    assert!(fin.join("gds/counter.gds").is_file());
    let metrics = sc_jobs::parse_metrics(&std::fs::read(fin.join("metrics.json")).unwrap()).unwrap();
    let summary = summarize(&metrics);
    assert_eq!(summary["signoff_clean"], true, "{summary}");
}

#[tokio::test]
#[ignore = "runs LibreLane in Docker"]
async fn cancel_stops_the_container() {
    let backend = DockerBackend::new(config()).await.unwrap();
    let (_dir, spec) = workdir("cancel");
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(8)).await;
        trigger.cancel();
    });
    let outcome = backend.run(&spec, cancel).await.unwrap();
    assert_eq!(outcome, Outcome::Canceled);
    let ps = std::process::Command::new("docker")
        .args(["ps", "-q", "--filter", &format!("name=sc-job-{}", spec.id)])
        .output()
        .unwrap();
    assert!(ps.stdout.is_empty(), "container still running");
}

#[tokio::test]
async fn missing_image_is_a_setup_error() {
    let mut c = config();
    c.image = "siliconcommons/does-not-exist:0".into();
    match DockerBackend::new(c).await {
        Err(sc_jobs::JobError::Setup(_)) => {}
        Err(e) => panic!("unexpected error: {e}"),
        Ok(_) => panic!("expected an error"),
    }
}
