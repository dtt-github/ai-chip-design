// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "edges": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_edges;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg din = 1'b0;
    wire rise;
    wire fall;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg m_prev;
    reg m_rise;
    reg m_fall;

    edges dut (.clk(clk), .rst_n(rst_n), .din(din), .rise(rise), .fall(fall));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_prev <= 1'b0;
            m_rise <= 1'b0;
            m_fall <= 1'b0;
        end else begin
            m_prev <= din;
            m_rise <= din && !m_prev;
            m_fall <= !din && m_prev;
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            if (r[1:0] == 2'd0 || r[3:2] == 2'd0) din = ~din;
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (rise !== m_rise) begin
                    if (errors < 5) $display("FAIL: cycle %0d: rise = 0x%0h, expected 0x%0h", cyc, rise, m_rise);
                    errors = errors + 1;
                end
                if (fall !== m_fall) begin
                    if (errors < 5) $display("FAIL: cycle %0d: fall = 0x%0h, expected 0x%0h", cyc, fall, m_fall);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for edges");
        else $display("FAIL: hidden checks for edges: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
