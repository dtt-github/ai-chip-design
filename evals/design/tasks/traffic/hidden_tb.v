// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "traffic": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_traffic;
    reg clk = 1'b0;
    reg rst_n = 1'b0;

    wire red;
    wire yellow;
    wire green;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [1:0] m_s;
    integer m_cnt;

    traffic dut (.clk(clk), .rst_n(rst_n), .red(red), .yellow(yellow), .green(green));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_s <= 2'd0;
            m_cnt <= 0;
        end else begin
            if ((m_s == 2'd0 && m_cnt == 5) || (m_s == 2'd1 && m_cnt == 7) || (m_s == 2'd2 && m_cnt == 1)) begin
                m_s <= (m_s == 2'd2) ? 2'd0 : m_s + 2'd1;
                m_cnt <= 0;
            end else begin
                m_cnt <= m_cnt + 1;
            end
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;

            @(negedge clk);
            #1;
            if (rst_n) begin
                if (red !== (m_s == 2'd0)) begin
                    if (errors < 5) $display("FAIL: cycle %0d: red = 0x%0h, expected 0x%0h", cyc, red, (m_s == 2'd0));
                    errors = errors + 1;
                end
                if (green !== (m_s == 2'd1)) begin
                    if (errors < 5) $display("FAIL: cycle %0d: green = 0x%0h, expected 0x%0h", cyc, green, (m_s == 2'd1));
                    errors = errors + 1;
                end
                if (yellow !== (m_s == 2'd2)) begin
                    if (errors < 5) $display("FAIL: cycle %0d: yellow = 0x%0h, expected 0x%0h", cyc, yellow, (m_s == 2'd2));
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for traffic");
        else $display("FAIL: hidden checks for traffic: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
