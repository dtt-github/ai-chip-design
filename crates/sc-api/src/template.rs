//! Files a new project starts with: a small clocked design and a LibreLane
//! config targeting the IHP SG13G2 open PDK.

/// Keywords a project slug could plausibly collide with.
const RESERVED: &[&str] = &[
    "always",
    "and",
    "assign",
    "begin",
    "bit",
    "buf",
    "byte",
    "case",
    "class",
    "default",
    "edge",
    "else",
    "end",
    "enum",
    "event",
    "for",
    "function",
    "generate",
    "if",
    "initial",
    "inout",
    "input",
    "int",
    "integer",
    "interface",
    "logic",
    "longint",
    "module",
    "nand",
    "nor",
    "not",
    "or",
    "output",
    "package",
    "parameter",
    "real",
    "reg",
    "shortint",
    "signed",
    "string",
    "struct",
    "task",
    "time",
    "type",
    "unsigned",
    "var",
    "void",
    "wire",
    "xor",
];

pub fn starter_files(name: &str, slug: &str, licence: &str) -> Vec<(String, Vec<u8>)> {
    let top = slug.replace('-', "_");
    // Module names must be identifiers, and not Verilog/SystemVerilog keywords.
    let top = if top.starts_with(|c: char| c.is_ascii_digit()) || RESERVED.contains(&top.as_str()) {
        format!("top_{top}")
    } else {
        top
    };

    let readme = format!(
        "# {name}\n\n\
         A SiliconCommons project targeting the IHP SG13G2 130 nm open PDK.\n\n\
         - `src/{top}.v` - top-level RTL\n\
         - `tb/tb_{top}.v` - self-checking testbench (Simulate runs it in your browser)\n\
         - `config.json` - LibreLane flow configuration (RTL only; the testbench is not synthesized)\n"
    );

    let rtl = format!(
        "// SPDX-License-Identifier: {licence}\n\
         `default_nettype none\n\
         \n\
         module {top} #(\n    parameter WIDTH = 8\n) (\n    input  wire             clk,\n    input  wire             rst_n,\n    input  wire             en,\n    output reg  [WIDTH-1:0] count\n);\n\
         \n    always @(posedge clk or negedge rst_n) begin\n        if (!rst_n)\n            count <= {{WIDTH{{1'b0}}}};\n        else if (en)\n            count <= count + 1'b1;\n    end\n\
         \nendmodule\n\
         \n`default_nettype wire\n"
    );

    let config = serde_json::to_string_pretty(&serde_json::json!({
        "DESIGN_NAME": top,
        "VERILOG_FILES": format!("dir::src/{top}.v"),
        "CLOCK_PORT": "clk",
        "CLOCK_PERIOD": 20,
        // Leave room for hold buffers; tiny designs fail detailed placement when packed tighter.
        "FP_CORE_UTIL": 40,
    }))
    .expect("static json")
        + "\n";

    let tb = format!(
        "// SPDX-License-Identifier: {licence}\n\
         `timescale 1ns/1ps\n\
         \n\
         module tb_{top};\n\
         \x20   reg clk = 0, rst_n = 0, en = 0;\n\
         \x20   wire [7:0] count;\n\
         \n\
         \x20   {top} dut (.clk(clk), .rst_n(rst_n), .en(en), .count(count));\n\
         \n\
         \x20   always #10 clk = ~clk;  // 50 MHz, matching CLOCK_PERIOD in config.json\n\
         \n\
         \x20   initial begin\n\
         \x20       #25 rst_n = 1;\n\
         \x20       en = 1;\n\
         \x20       repeat (20) @(posedge clk);\n\
         \x20       #1;\n\
         \x20       if (count == 8'd20) $display(\"PASS: count = %0d after 20 cycles\", count);\n\
         \x20       else $display(\"FAIL: count = %0d, expected 20\", count);\n\
         \x20       $finish;\n\
         \x20   end\n\
         endmodule\n"
    );

    vec![
        ("README.md".into(), readme.into_bytes()),
        (format!("src/{top}.v"), rtl.into_bytes()),
        (format!("tb/tb_{top}.v"), tb.into_bytes()),
        ("config.json".into(), config.into_bytes()),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn starter_testbench_passes() {
        let files = super::starter_files("Demo", "demo", "Apache-2.0");
        let srcs: Vec<(String, String)> = files
            .iter()
            .filter(|(p, _)| p.ends_with(".v"))
            .map(|(p, c)| (p.clone(), String::from_utf8(c.clone()).unwrap()))
            .collect();
        let refs: Vec<(&str, &str)> = srcs.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let r = sc_sim::simulate(&refs, None, sc_sim::Limits::default()).unwrap();
        assert!(r.finished);
        assert_eq!(r.log, vec!["PASS: count = 20 after 20 cycles"]);
    }

    #[test]
    fn module_name_is_a_valid_identifier() {
        let files = super::starter_files("My Chip", "2-bit-alu", "Apache-2.0");
        assert_eq!(files[1].0, "src/top_2_bit_alu.v");
        assert_eq!(super::starter_files("Logic", "logic", "Apache-2.0")[1].0, "src/top_logic.v");
        let rtl = String::from_utf8(files[1].1.clone()).unwrap();
        assert!(rtl.contains("module top_2_bit_alu #("));
        assert!(rtl.contains("count <= {WIDTH{1'b0}};"));
    }
}
