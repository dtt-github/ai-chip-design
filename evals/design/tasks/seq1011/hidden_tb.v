// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "seq1011": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_seq1011;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg din = 1'b0;
    wire found;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [3:0] m_hist;
    reg m_found;

    seq1011 dut (.clk(clk), .rst_n(rst_n), .din(din), .found(found));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_hist <= 4'd0;
            m_found <= 1'b0;
        end else begin
            m_found <= ({m_hist[2:0], din} == 4'b1011);
            m_hist <= {m_hist[2:0], din};
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            din = r[0];
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (found !== m_found) begin
                    if (errors < 5) $display("FAIL: cycle %0d: found = 0x%0h, expected 0x%0h", cyc, found, m_found);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for seq1011");
        else $display("FAIL: hidden checks for seq1011: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
