// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "parity8": every input value, checked against a model.
// The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_parity8;
    reg [7:0] d = 8'd0;
    wire odd;
    reg [8:0] i;
    reg [7:0] v;
    integer errors = 0;

    parity8 dut (.d(d), .odd(odd));

    initial begin
        for (i = 0; i < 256; i = i + 1) begin
            v = i[7:0];
            d = v;
            #10;
            if (odd !== ^v) begin
                if (errors < 5) $display("FAIL: d = 0x%0h: odd = %0d, expected %0d", v, odd, ^v);
                errors = errors + 1;
            end
        end
        if (errors == 0) $display("PASS: hidden checks for parity8");
        else $display("FAIL: hidden checks for parity8: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
