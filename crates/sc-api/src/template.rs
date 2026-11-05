//! Files a new project starts with: a small clocked design and a LibreLane
//! config targeting the IHP SG13G2 open PDK.

pub fn starter_files(name: &str, slug: &str) -> Vec<(String, Vec<u8>)> {
    let top = slug.replace('-', "_");
    let top = if top.starts_with(|c: char| c.is_ascii_digit()) { format!("top_{top}") } else { top };

    let readme = format!(
        "# {name}\n\n\
         A SiliconCommons project targeting the IHP SG13G2 130 nm open PDK.\n\n\
         - `src/{top}.v` - top-level RTL\n\
         - `config.json` - LibreLane flow configuration\n"
    );

    let rtl = format!(
        "// SPDX-License-Identifier: Apache-2.0\n\
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

    vec![
        ("README.md".into(), readme.into_bytes()),
        (format!("src/{top}.v"), rtl.into_bytes()),
        ("config.json".into(), config.into_bytes()),
    ]
}

#[cfg(test)]
mod tests {
    #[test]
    fn module_name_is_a_valid_identifier() {
        let files = super::starter_files("My Chip", "2-bit-alu");
        assert_eq!(files[1].0, "src/top_2_bit_alu.v");
        let rtl = String::from_utf8(files[1].1.clone()).unwrap();
        assert!(rtl.contains("module top_2_bit_alu #("));
        assert!(rtl.contains("count <= {WIDTH{1'b0}};"));
    }
}
