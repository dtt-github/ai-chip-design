//! cargo run -p sc-sim --release --example bench
fn main() {
    let src = r#"
module counter (input clk, input rst, output reg [31:0] q);
    always @(posedge clk) if (rst) q <= 0; else q <= q + 1;
endmodule
module tb;
    reg clk = 0, rst = 1;
    wire [31:0] q;
    counter c (clk, rst, q);
    always #5 clk = ~clk;
    initial begin #12 rst = 0; #10000000 $display("q=%0d", q); $finish; end
endmodule"#;
    let t = std::time::Instant::now();
    let r = sc_sim::simulate(
        &[("bench.v", src)],
        None,
        sc_sim::Limits { max_time: u64::MAX, max_steps: u64::MAX, ..Default::default() },
    )
    .unwrap();
    let changes: usize = r.signals.iter().map(|s| s.times.len()).sum();
    println!("{:?} log={:?} steps={} changes={} in {:?}", r.finished, r.log, r.steps, changes, t.elapsed());
}
