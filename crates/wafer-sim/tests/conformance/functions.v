// Functions and tasks: old-style and ANSI declarations, locals and loops, calls in continuous
// assignments and in always @(*), nested calls, return, constant functions in parameters and
// ranges, and tasks with outputs and delays.
`timescale 1ns/1ps
module tb;
    // Constant function: sizes a counter from a parameter.
    function integer clog2;
        input integer value;
        integer v;
        begin
            v = value - 1;
            for (clog2 = 0; v > 0; clog2 = clog2 + 1)
                v = v >> 1;
        end
    endfunction

    localparam DEPTH = 12;
    localparam AW = clog2(DEPTH);
    reg [clog2(DEPTH)-1:0] addr;

    // Old style: return type, then input declarations.
    function [7:0] reverse;
        input [7:0] x;
        integer i;
        begin
            for (i = 0; i < 8; i = i + 1)
                reverse[i] = x[7 - i];
        end
    endfunction

    // ANSI arguments, early return.
    function [3:0] popcount(input [7:0] x);
        integer i;
        begin
            popcount = 0;
            if (x == 0) return 0;
            for (i = 0; i < 8; i = i + 1)
                popcount = popcount + x[i];
        end
    endfunction

    function [3:0] decode(input [1:0] sel);
        case (sel)
            2'd0: decode = 4'b0001;
            2'd1: decode = 4'b0010;
            2'd2: decode = 4'b0100;
            default: decode = 4'b1000;
        endcase
    endfunction

    // One CRC-8 (x^8 + x^2 + x + 1) step per input bit.
    function [7:0] crc8(input [7:0] crc, input [7:0] data);
        integer i;
        reg fb;
        begin
            crc8 = crc;
            for (i = 7; i >= 0; i = i - 1) begin
                fb = crc8[7] ^ data[i];
                crc8 = {crc8[6:0], 1'b0} ^ (fb ? 8'h07 : 8'h00);
            end
        end
    endfunction

    function signed [7:0] negate(input signed [7:0] x);
        negate = -x;
    endfunction

    // A function that calls another.
    function [3:0] ones_in_reverse(input [7:0] x);
        ones_in_reverse = popcount(reverse(x));
    endfunction

    reg [7:0] a;
    reg [7:0] b;
    wire [7:0] r = reverse(a);
    wire [3:0] n = popcount(a) + popcount(b);
    reg [3:0] onehot;
    always @(*) onehot = decode(a[1:0]);

    reg [7:0] crc;
    reg [7:0] sum;
    reg [7:0] seen;
    integer k;

    task add(input [7:0] x, input [7:0] y, output [7:0] z);
        begin
            #1 z = x + y;
        end
    endtask

    task show;
        $display("show: a=%h r=%h n=%0d onehot=%b", a, r, n, onehot);
    endtask

    initial begin
        addr = -1;
        a = 8'h0F;
        b = 8'h00;
        #1;
        $display("AW=%0d addr all ones=%0d clog2(1)=%0d clog2(17)=%0d", AW, addr, clog2(1), clog2(17));
        show;
        a = 8'b1010_0001;
        b = 8'hFF;
        #1;
        show;
        $display("popcount(0)=%0d decode(3)=%b negate(5)=%0d negate(-7)=%0d", popcount(8'h00), decode(2'd3),
                 negate(8'sd5), negate(-8'sd7));
        $display("nested=%0d twice=%0d", ones_in_reverse(8'hC3), popcount(8'h01) + popcount(8'h03));
        crc = 8'h00;
        for (k = 0; k < 4; k = k + 1)
            crc = crc8(crc, 8'h31 + k);
        $display("crc8 of 1234 = %h", crc);
        add(8'd40, 8'd2, sum);
        $display("t=%0d sum=%0d", $time, sum);
        add(sum, 8'd200, seen);
        $display("t=%0d seen=%0d", $time, seen);
        $finish;
    end
endmodule
