// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "updown": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_updown;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg up = 1'b0;
    reg down = 1'b0;
    wire [3:0] count;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [3:0] m_count;

    updown dut (.clk(clk), .rst_n(rst_n), .up(up), .down(down), .count(count));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_count <= 4'd0;
        end else begin
            if (up && !down) m_count <= m_count + 4'd1;
            else if (down && !up) m_count <= m_count - 4'd1;
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            up = r[0];
            down = r[1];
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (count !== m_count) begin
                    if (errors < 5) $display("FAIL: cycle %0d: count = 0x%0h, expected 0x%0h", cyc, count, m_count);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for updown");
        else $display("FAIL: hidden checks for updown: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
