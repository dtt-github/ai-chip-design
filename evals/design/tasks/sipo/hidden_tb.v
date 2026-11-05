// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "sipo": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_sipo;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg din = 1'b0;
    reg shift = 1'b0;
    wire [7:0] q;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [7:0] m_q;

    sipo dut (.clk(clk), .rst_n(rst_n), .din(din), .shift(shift), .q(q));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_q <= 8'd0;
        end else begin
            if (shift) m_q <= {m_q[6:0], din};
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            shift = r[0];
            din = r[1];
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (q !== m_q) begin
                    if (errors < 5) $display("FAIL: cycle %0d: q = 0x%0h, expected 0x%0h", cyc, q, m_q);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for sipo");
        else $display("FAIL: hidden checks for sipo: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
