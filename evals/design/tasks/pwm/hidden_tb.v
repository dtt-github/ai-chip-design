// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "pwm": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_pwm;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg [7:0] duty = 8'd0;
    wire out;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [7:0] m_cnt;

    pwm dut (.clk(clk), .rst_n(rst_n), .duty(duty), .out(out));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_cnt <= 8'd0;
        end else begin
            m_cnt <= m_cnt + 8'd1;
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            if (cyc == 0) duty = 8'd0;
            else if (cyc == 600) duty = 8'd255;
            else if (cyc % 300 == 0) duty = r[7:0];
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (out !== (m_cnt < duty)) begin
                    if (errors < 5) $display("FAIL: cycle %0d: out = 0x%0h, expected 0x%0h", cyc, out, (m_cnt < duty));
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for pwm");
        else $display("FAIL: hidden checks for pwm: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
