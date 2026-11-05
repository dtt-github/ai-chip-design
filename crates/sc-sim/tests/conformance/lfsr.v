// Galois LFSR and a shift register, checking periods and sequences.
module lfsr8 (input clk, input rst, output reg [7:0] q);
    always @(posedge clk)
        if (rst) q <= 8'h01;
        else q <= {q[6:0], 1'b0} ^ (q[7] ? 8'h1D : 8'h00);
endmodule

module tb;
    reg clk = 0, rst = 1;
    wire [7:0] q;
    reg [15:0] shreg = 16'h0000;
    integer cycles = 0;
    lfsr8 u (.clk(clk), .rst(rst));
    assign q = u.q;
    always #1 clk = ~clk;
    always @(posedge clk) if (!rst) shreg <= {shreg[14:0], q[0] ^ q[7]};
    initial begin
        #3 rst = 0;
        repeat (20) begin
            @(posedge clk); #0.1;
            $display("%b %h %h", q, q, shreg);
        end
        // Period of the maximal-length sequence.
        @(posedge clk); #0.1;
        while (q != 8'h01) begin
            @(posedge clk); #0.1;
            cycles = cycles + 1;
        end
        $display("period %0d", cycles + 22);
        $finish;
    end
endmodule
