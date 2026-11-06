// Mealy sequence detector for 1011 (overlapping) with casez decoding.
module det1011 (input clk, input rst_n, input x, output reg hit, output reg [2:0] st);
    localparam S0 = 3'd0, S1 = 3'd1, S10 = 3'd2, S101 = 3'd3;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin st <= S0; hit <= 0; end
        else begin
            hit <= 0;
            casez ({st, x})
                {S0, 1'b1}: st <= S1;
                {S0, 1'b0}: st <= S0;
                {S1, 1'b0}: st <= S10;
                {S1, 1'b1}: st <= S1;
                {S10, 1'b1}: st <= S101;
                {S10, 1'b0}: st <= S0;
                {S101, 1'b1}: begin st <= S1; hit <= 1; end
                4'b011?: st <= S10;
                default: st <= S0;
            endcase
        end
    end
endmodule

module tb;
    reg clk = 0, rst_n = 0, x = 0;
    wire hit;
    wire [2:0] st;
    reg [31:0] stim = 32'b1011_0110_1101_1011_1011_0001_0110_1100;
    integer i, hits = 0;
    det1011 d (clk, rst_n, x, hit, st);
    always #5 clk = ~clk;
    always @(posedge clk) if (hit) hits = hits + 1;
    initial begin
        #7 rst_n = 1;
        for (i = 31; i >= 0; i = i - 1) begin
            @(negedge clk) x = stim[i];
            @(posedge clk) #1 $display("i=%0d x=%b st=%0d hit=%b", i, x, st, hit);
        end
        @(posedge clk) #1;
        $display("hits=%0d", hits);
        $finish;
    end
endmodule
