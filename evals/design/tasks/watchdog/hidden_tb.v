// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "watchdog": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_watchdog;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg kick = 1'b0;
    wire bark;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    integer m_cnt;
    reg m_bark;

    watchdog dut (.clk(clk), .rst_n(rst_n), .kick(kick), .bark(bark));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_cnt <= 0;
            m_bark <= 1'b0;
        end else begin
            if (kick) begin
                m_cnt <= 0;
                m_bark <= 1'b0;
            end else if (m_cnt == 7) begin
                m_bark <= 1'b1;
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
            // Short gaps in one half, long ones in the other.
            if (cyc % 400 < 200) kick = (r[2:0] == 3'd0);
            else kick = (r[4:0] == 5'd0);
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (bark !== m_bark) begin
                    if (errors < 5) $display("FAIL: cycle %0d: bark = 0x%0h, expected 0x%0h", cyc, bark, m_bark);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for watchdog");
        else $display("FAIL: hidden checks for watchdog: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
