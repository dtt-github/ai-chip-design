// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "debounce": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_debounce;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg din = 1'b0;
    wire dout;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [15:0] m_cnt;
    reg m_dout;

    debounce dut (.clk(clk), .rst_n(rst_n), .din(din), .dout(dout));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_cnt <= 0;
            m_dout <= 1'b0;
        end else begin
            if (din == m_dout) begin
                m_cnt <= 0;
            end else if (m_cnt == 3) begin
                m_cnt <= 0;
                m_dout <= din;
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
            if (r[2:0] == 3'd0) din = ~din;
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (dout !== m_dout) begin
                    if (errors < 5) $display("FAIL: cycle %0d: dout = 0x%0h, expected 0x%0h", cyc, dout, m_dout);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for debounce");
        else $display("FAIL: hidden checks for debounce: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
