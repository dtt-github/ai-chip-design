// Parameterised hierarchy: overrides by name and position, localparams, $clog2.
module sat_add #(parameter W = 4, parameter MAX = (1 << W) - 1) (input [W-1:0] a, b, output [W-1:0] y);
    wire [W:0] s = a + b;
    assign y = (s > MAX) ? MAX[W-1:0] : s[W-1:0];
endmodule

module adder_tree #(parameter W = 4) (input [W-1:0] a, b, c, d, output [W-1:0] y);
    localparam INNER = W;
    wire [INNER-1:0] ab, cd;
    sat_add #(.W(INNER)) u0 (.a(a), .b(b), .y(ab));
    sat_add #(INNER) u1 (c, d, cd);
    sat_add #(W, 9) u2 (.a(ab), .b(cd), .y(y));
endmodule

module tb;
    localparam N = 10;
    localparam AW = $clog2(N);
    reg [3:0] a, b, c, d;
    wire [3:0] y;
    adder_tree #(.W(4)) t (a, b, c, d, y);
    initial begin
        $display("AW=%0d", AW);
        a = 1; b = 2; c = 3; d = 0; #1 $display("%0d", y);
        a = 7; b = 7; c = 1; d = 1; #1 $display("%0d %0d %0d", t.ab, t.cd, y);
        a = 15; b = 15; c = 15; d = 15; #1 $display("%0d", y);
        a = 2; b = 2; c = 2; d = 2; #1 $display("%0d", y);
        a = 4; b = 3; c = 0; d = 1; #1 $display("%0d", y);
    end
endmodule
