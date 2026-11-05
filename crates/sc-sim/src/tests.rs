//! End-to-end behaviour on small programs.

use crate::*;

fn run(src: &str) -> SimResult {
    simulate(&[("t.v", src)], None, Limits::default()).unwrap_or_else(|e| panic!("{e}"))
}

fn log(src: &str) -> Vec<String> {
    run(src).log
}

#[test]
fn counter_with_testbench() {
    let src = r#"
`timescale 1ns/1ps
module counter #(parameter WIDTH = 4) (input wire clk, input wire rst_n, input wire en, output reg [WIDTH-1:0] count);
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) count <= {WIDTH{1'b0}};
        else if (en) count <= count + 1'b1;
    end
endmodule

module tb;
    reg clk = 0, rst_n = 0, en = 0;
    wire [3:0] count;
    counter dut (.clk(clk), .rst_n(rst_n), .en(en), .count(count));
    always #5 clk = ~clk;
    initial begin
        #12 rst_n = 1;
        en = 1;
        repeat (18) @(posedge clk);
        #1 $display("count=%0d time=%0t", count, $time);
        $finish;
    end
endmodule
"#;
    let r = run(src);
    assert!(r.finished);
    // 18 rising edges with en=1 from t=15 on: wraps at 16 → 2.
    assert_eq!(r.log, vec!["count=2 time=186"]);
    let c = r.signals.iter().find(|s| s.name == "tb.count").unwrap();
    assert_eq!(c.width, 4);
    assert_eq!(c.times[1], 15);
    assert_eq!(c.values[1], 1);
}

#[test]
fn blocking_vs_nonblocking_swap() {
    let l = log(r#"
module tb;
    reg [7:0] a = 1, b = 2, c = 1, d = 2;
    reg clk = 0;
    always @(posedge clk) begin a <= b; b <= a; end
    always @(posedge clk) begin c = d; d = c; end
    initial begin #1 clk = 1; #1 $display("%0d %0d %0d %0d", a, b, c, d); end
endmodule"#);
    assert_eq!(l, vec!["2 1 2 2"]);
}

#[test]
fn widths_signedness_and_operators() {
    let l = log(r#"
module tb;
    reg [7:0] a = 8'hF0, b = 8'h20;
    reg [8:0] s;
    reg signed [7:0] n = -8'sd3;
    integer i = -7;
    wire [3:0] nib = a[7:4];
    initial begin
        s = a + b;                       // context width 9: carry kept
        $display("%h %0d", s, s);
        $display("%b", ~a[3:0]);
        $display("%0d %0d", n >>> 1, n / 2);
        $display("%0d %0d %0d", i % 4, i / 2, -i);
        $display("%0d %0d %0d", &a, |a, ^a);
        $display("%h", {a[3:0], b[7:4]});
        $display("%h", {2{nib}});
        $display("%0d", (a > b) ? 7 : 9);
        $display("%0d", 2 ** 10);
        $display("%0d", a[5 +: 3]);
    end
endmodule"#);
    assert_eq!(l, vec!["110 272", "1111", "-2 -1", "-3 -3 7", "0 1 0", "02", "ff", "7", "1024", "7"]);
}

#[test]
fn case_casez_memory_and_loops() {
    let l = log(r#"
module tb;
    reg [3:0] x;
    reg [7:0] mem [0:7];
    integer i;
    reg [7:0] y;
    always @(*) begin
        casez (x)
            4'b1???: y = 8;
            4'b01??: y = 4;
            4'b0001, 4'b0010: y = 1;
            default: y = 0;
        endcase
    end
    initial begin
        for (i = 0; i < 8; i = i + 1) mem[i] = i * 3;
        x = 4'b1010; #1 $display("%0d", y);
        x = 4'b0110; #1 $display("%0d", y);
        x = 4'b0010; #1 $display("%0d", y);
        x = 4'b0000; #1 $display("%0d", y);
        $display("%0d %0d", mem[5], mem[8]);
        i = 0;
        while (i < 3) i = i + 1;
        $display("%0d", i);
    end
endmodule"#);
    assert_eq!(l, vec!["8", "4", "1", "0", "15 0", "3"]);
}

#[test]
fn display_formats_like_iverilog() {
    let l = log(r#"
module tb;
    reg [7:0] v = 5;
    integer k = 42;
    initial begin
        $display("[%d] [%h] [%b] [%o]", v, v, v, v);
        $display("[%d]", k);
        $display(v, " and ", k);
        $write("a"); $write("b"); $display("c %%");
        $display("%m");
    end
endmodule"#);
    assert_eq!(l, vec!["[  5] [05] [00000101] [005]", "[         42]", "  5 and          42", "abc %", "tb"]);
}

#[test]
fn monitor_strobe_wait_and_finish() {
    let r = run(r#"
module tb;
    reg [3:0] c = 0;
    reg go = 0;
    initial $monitor("t=%0t c=%0d", $time, c);
    always #10 c = c + 1;
    initial begin
        wait (c == 2);
        $strobe("strobe c=%0d", c);
        go = 1;
    end
    initial #35 $finish;
endmodule"#);
    assert!(r.finished);
    assert_eq!(r.time, 35);
    // Postponed region: $monitor before $strobe, as in Icarus Verilog.
    assert_eq!(r.log, vec!["t=0 c=0", "t=10 c=1", "t=20 c=2", "strobe c=2", "t=30 c=3"]);
}

#[test]
fn hierarchy_parameters_and_outputs() {
    let l = log(r#"
module add #(parameter W = 8) (input [W-1:0] a, b, output [W:0] s);
    assign s = a + b;
endmodule
module pair (input [3:0] x, output [4:0] y);
    wire [4:0] t;
    add #(.W(4)) u0 (.a(x), .b(4'd1), .s(t));
    assign y = t;
endmodule
module tb;
    reg [3:0] x = 15;
    wire [4:0] y;
    pair p (x, y);
    initial #1 $display("%0d %0d", y, tb.p.t);
endmodule"#);
    assert_eq!(l, vec!["16 16"]);
}

#[test]
fn errors_point_at_the_line() {
    let e = simulate(&[("bad.v", "module m;\n  reg a;\n  initial a = b;\nendmodule")], None, Limits::default())
        .unwrap_err();
    assert_eq!(e, Error::Elab { file: "bad.v".into(), line: 3, msg: "'b' is not declared in m".into() });
    let e = simulate(&[("bad.v", "module m;\n function f;\nendmodule")], None, Limits::default()).unwrap_err();
    assert!(matches!(e, Error::Syntax { line: 2, .. }), "{e}");
    let e = simulate(&[("bad.v", "module m;\n reg [127:0] w;\nendmodule")], None, Limits::default()).unwrap_err();
    assert!(e.to_string().contains("up to 64"), "{e}");
}

#[test]
fn zero_delay_loops_are_caught() {
    let e = simulate(
        &[("loop.v", "module m;\n wire a, b;\n assign a = ~b;\n assign b = a;\nendmodule")],
        None,
        Limits { max_steps_per_time: 10_000, ..Limits::default() },
    )
    .unwrap_err();
    assert!(matches!(e, Error::Runtime { .. }), "{e}");
}

#[test]
fn time_limit_without_finish() {
    let r = simulate(
        &[("c.v", "module m; reg c = 0; always #1 c = ~c; endmodule")],
        None,
        Limits { max_time: 100, ..Limits::default() },
    )
    .unwrap();
    assert!(!r.finished);
    assert!(r.stopped.unwrap().contains("time limit"));
}

#[test]
fn vcd_output() {
    let r = run("module tb; reg [3:0] a = 0; initial begin #5 a = 3; #5 $finish; end endmodule");
    let v = vcd::to_vcd(&r);
    assert!(v.contains("$timescale 1ns $end"));
    assert!(v.contains("$scope module tb $end"));
    assert!(v.contains("#5\nb11 !"));
}

#[test]
fn resolved_instance_parameters() {
    let src = r#"
module dut #(parameter W = 4, parameter P = 0) (input wire a);
    localparam L = W * 2;
endmodule
module tb;
    localparam CPB = 8;
    reg a = 0;
    dut #(.W(CPB)) d1 (.a(a));
    dut d2 (.a(a));
endmodule
"#;
    let (modules, _) = parse(&[("t.v", src)]).unwrap();
    let d = elab::elaborate(&modules, "tb").unwrap();
    let get = |path: &str| d.instances.iter().find(|i| i.path == path).unwrap().params.clone();
    assert_eq!(get("tb.d1"), [("P".to_string(), 0), ("W".to_string(), 8)].into());
    assert_eq!(get("tb.d2")["W"], 4);
    assert!(!get("tb.d1").contains_key("L"));

    let top = elab::elaborate_with(&modules, "dut", &[("P".to_string(), 1)].into()).unwrap();
    assert_eq!(top.instances[0].params["P"], 1);
    assert!(elab::elaborate_with(&modules, "dut", &[("L".to_string(), 1)].into()).is_err());
}

#[test]
fn hostile_nesting_is_refused_not_a_crash() {
    // The way servers run untrusted designs.
    let r = on_big_stack(|| {
        let cases = [
            format!("module tb; wire x; wire y = {}x; endmodule", "~".repeat(1_000_000)),
            format!("module tb; wire x; wire y = {}x{}; endmodule", "(".repeat(100_000), ")".repeat(100_000)),
            format!("module tb; initial {}$finish; endmodule", "begin ".repeat(100_000)),
            format!("module tb; wire x; wire y = x{}; endmodule", " + x".repeat(500_000)),
        ];
        cases.iter().map(|src| simulate(&[("t.v", src)], Some("tb"), Limits::default()).is_err()).collect::<Vec<_>>()
    });
    assert_eq!(r, vec![true; 4]);

    // Ordinary depth still works: 100 nested parentheses and a 900-term sum.
    let src = format!(
        "module tb; reg [15:0] y; initial begin y = {}1{} + {}; $display(\"%0d\", y); $finish; end endmodule",
        "(".repeat(100),
        ")".repeat(100),
        vec!["1"; 900].join(" + ")
    );
    assert_eq!(on_big_stack(|| log(&src)), vec!["901"]);
}
