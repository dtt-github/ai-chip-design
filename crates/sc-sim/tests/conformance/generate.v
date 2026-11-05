// Generate: loops creating instances and assignments, nested loops, if and case on parameters,
// localparams inside blocks, and %m inside generated instances.
`timescale 1ns/1ps
module full_adder (input wire a, input wire b, input wire cin, output wire s, output wire cout);
    assign s = a ^ b ^ cin;
    assign cout = (a & b) | (cin & (a ^ b));
endmodule

module ripple #(parameter W = 4) (input wire [W-1:0] a, input wire [W-1:0] b, output wire [W:0] sum);
    wire [W:0] c;
    assign c[0] = 1'b0;
    genvar i;
    generate
        for (i = 0; i < W; i = i + 1) begin : bit_
            full_adder fa (.a(a[i]), .b(b[i]), .cin(c[i]), .s(sum[i]), .cout(c[i+1]));
        end
    endgenerate
    assign sum[W] = c[W];
endmodule

module tag;
    initial #5 $display("instance %m");
endmodule

module pick #(parameter MODE = 0) (input wire [7:0] x, output wire [7:0] y);
    generate
        if (MODE == 0) begin : pass
            assign y = x;
        end else if (MODE == 1) begin : inv
            assign y = ~x;
        end else begin : swap
            localparam H = 4;
            assign y = {x[H-1:0], x[7:H]};
        end
    endgenerate
endmodule

module shape #(parameter KIND = 2) (output wire [3:0] code);
    generate
        case (KIND)
            0: begin : zero
                assign code = 4'h0;
            end
            1, 2: begin : mid
                localparam V = KIND * 3;
                assign code = V;
            end
            default: begin : big
                assign code = 4'hF;
            end
        endcase
    endgenerate
endmodule

module tb;
    reg [3:0] a, b;
    wire [4:0] sum;
    ripple #(.W(4)) add (.a(a), .b(b), .sum(sum));

    // Bit reversal and a 2-D grid of XORs from nested loops (SystemVerilog genvar in the header).
    reg [7:0] v = 8'b1101_0011;
    wire [7:0] rev;
    wire [8:0] grid;
    for (genvar j = 0; j < 8; j = j + 1) begin : rv
        assign rev[j] = v[7 - j];
    end
    for (genvar r = 0; r < 3; r++) begin : row
        for (genvar c = 0; c < 3; c = c + 1) begin : col
            localparam IDX = r * 3 + c;
            assign grid[IDX] = v[r] ^ v[c];
        end
    end

    wire [7:0] y0, y1, y2;
    pick #(.MODE(0)) p0 (.x(v), .y(y0));
    pick #(.MODE(1)) p1 (.x(v), .y(y1));
    pick #(.MODE(2)) p2 (.x(v), .y(y2));

    wire [3:0] k0, k2, k9;
    shape #(.KIND(0)) s0 (.code(k0));
    shape #(.KIND(2)) s2 (.code(k2));
    shape #(.KIND(9)) s9 (.code(k9));

    genvar t;
    generate
        for (t = 0; t < 2; t = t + 1) begin : g
            tag u ();
        end
    endgenerate

    integer i;
    initial begin
        for (i = 0; i < 4; i = i + 1) begin
            a = 4'd3 * i + 4'd5;
            b = 4'd15 - i;
            #1 $display("%0d + %0d = %0d", a, b, sum);
        end
        $display("rev=%b grid=%b", rev, grid);
        $display("pick: %h %h %h", y0, y1, y2);
        $display("shape: %h %h %h", k0, k2, k9);
        #10 $finish;
    end
endmodule
