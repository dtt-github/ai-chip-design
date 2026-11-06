// Combinational ALU with signed operations; testbench sweeps vectors.
module alu (
    input  wire [3:0]  op,
    input  wire [7:0]  a,
    input  wire [7:0]  b,
    output reg  [7:0]  y,
    output wire        zero,
    output reg         carry
);
    wire signed [7:0] sa = a;
    wire signed [7:0] sb = b;
    always @(*) begin
        carry = 1'b0;
        case (op)
            4'd0: {carry, y} = a + b;
            4'd1: {carry, y} = {1'b0, a} - {1'b0, b};
            4'd2: y = a & b;
            4'd3: y = a | b;
            4'd4: y = a ^ b;
            4'd5: y = ~a;
            4'd6: y = a << b[2:0];
            4'd7: y = a >> b[2:0];
            4'd8: y = sa >>> b[2:0];
            4'd9: y = (sa < sb) ? 8'd1 : 8'd0;
            4'd10: y = (a < b) ? 8'd1 : 8'd0;
            4'd11: y = a * b;
            4'd12: y = {a[3:0], b[7:4]};
            4'd13: y = {4{a[1:0]}};
            default: y = 8'hEE;
        endcase
    end
    assign zero = (y == 8'd0);
endmodule

module tb;
    reg [3:0] op;
    reg [7:0] a, b;
    wire [7:0] y;
    wire zero, carry;
    integer i, j;
    reg [7:0] as [0:3];
    reg [7:0] bs [0:3];
    alu dut (.op(op), .a(a), .b(b), .y(y), .zero(zero), .carry(carry));
    initial begin
        as[0] = 8'h00; as[1] = 8'h7F; as[2] = 8'h80; as[3] = 8'hC5;
        bs[0] = 8'h01; bs[1] = 8'hFF; bs[2] = 8'h03; bs[3] = 8'h80;
        for (op = 0; op < 15; op = op + 1) begin
            for (i = 0; i < 4; i = i + 1) begin
                for (j = 0; j < 4; j = j + 1) begin
                    a = as[i]; b = bs[j];
                    #1 $display("op=%0d a=%h b=%h y=%h z=%b c=%b", op, a, b, y, zero, carry);
                end
            end
        end
        $finish;
    end
endmodule
