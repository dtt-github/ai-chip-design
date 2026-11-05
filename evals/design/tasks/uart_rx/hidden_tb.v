// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "uart_rx": 150 frames of random bytes with random gaps, about
// one in eight with a bad stop bit. Every valid must last one cycle and carry the next good byte,
// arriving before the next frame starts. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_uart_rx;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg rx = 1'b1;
    wire [7:0] data;
    wire valid;
    reg [15:0] r = 16'hACE1;
    reg [7:0] expected [0:255];
    reg [7:0] tx_byte;
    reg good;
    reg last_valid = 1'b0;
    integer sent = 0;
    integer got = 0;
    integer errors = 0;
    integer f;
    integer b;

    uart_rx dut (.clk(clk), .rst_n(rst_n), .rx(rx), .data(data), .valid(valid));

    always #500 clk = ~clk;

    // Every valid lasts one cycle and carries the next byte that was sent with a good stop bit.
    always @(negedge clk) begin
        #1;
        if (rst_n) begin
            if (valid && last_valid) begin
                if (errors < 5) $display("FAIL: valid stayed 1 for more than one cycle");
                errors = errors + 1;
            end
            if (valid && !last_valid) begin
                if (got >= sent) begin
                    if (errors < 5) $display("FAIL: valid with no good frame (data = 0x%0h)", data);
                    errors = errors + 1;
                end else if (data !== expected[got]) begin
                    if (errors < 5) $display("FAIL: byte %0d: data = 0x%0h, expected 0x%0h", got, data, expected[got]);
                    errors = errors + 1;
                end
                got = got + 1;
            end
            last_valid = valid;
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        repeat (8) @(negedge clk);
        for (f = 0; f < 150; f = f + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            tx_byte = r[7:0];
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            good = (r[2:0] != 3'd0);
            rx = 1'b0;
            repeat (4) @(negedge clk);
            for (b = 0; b < 8; b = b + 1) begin
                rx = tx_byte[b];
                repeat (4) @(negedge clk);
            end
            if (good) begin
                expected[sent] = tx_byte;
                sent = sent + 1;
            end
            rx = good;
            repeat (4) @(negedge clk);
            rx = 1'b1;
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            repeat (4 * (1 + r[1:0])) @(negedge clk);
            if (got != sent) begin
                if (errors < 5) $display("FAIL: frame %0d: %0d byte(s) received, expected %0d", f, got, sent);
                errors = errors + 1;
                got = sent;
            end
        end
        if (errors == 0) $display("PASS: hidden checks for uart_rx (%0d bytes)", sent);
        else $display("FAIL: hidden checks for uart_rx: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
