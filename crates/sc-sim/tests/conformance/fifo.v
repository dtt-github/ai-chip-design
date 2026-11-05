// Synchronous FIFO with a memory, count and flags.
module fifo #(parameter W = 8, parameter DEPTH = 4, parameter AW = 2) (
    input  wire         clk,
    input  wire         rst,
    input  wire         push,
    input  wire         pop,
    input  wire [W-1:0] din,
    output wire [W-1:0] dout,
    output wire         full,
    output wire         empty,
    output reg  [AW:0]  count
);
    reg [W-1:0] mem [0:DEPTH-1];
    reg [AW-1:0] wp, rp;
    assign full = (count == DEPTH);
    assign empty = (count == 0);
    assign dout = mem[rp];
    always @(posedge clk) begin
        if (rst) begin
            wp <= 0; rp <= 0; count <= 0;
        end else begin
            if (push && !full) begin mem[wp] <= din; wp <= wp + 1'b1; end
            if (pop && !empty) rp <= rp + 1'b1;
            case ({push && !full, pop && !empty})
                2'b10: count <= count + 1'b1;
                2'b01: count <= count - 1'b1;
                default: count <= count;
            endcase
        end
    end
endmodule

module tb;
    reg clk = 0, rst = 1, push = 0, pop = 0;
    reg [7:0] din = 0;
    wire [7:0] dout;
    wire full, empty;
    wire [2:0] count;
    integer i;
    fifo #(8, 4, 2) dut (clk, rst, push, pop, din, dout, full, empty, count);
    always #5 clk = ~clk;
    always @(posedge clk) #1 $display("t=%0d push=%b pop=%b din=%0d dout=%0d count=%0d full=%b empty=%b",
                                      $time, push, pop, din, empty ? 8'd0 : dout, count, full, empty);
    initial begin
        @(negedge clk) rst = 0;
        for (i = 0; i < 6; i = i + 1) begin
            @(negedge clk) begin push = 1; din = 10 + i; end
        end
        @(negedge clk) push = 0;
        for (i = 0; i < 3; i = i + 1) @(negedge clk) pop = 1;
        @(negedge clk) begin pop = 1; push = 1; din = 99; end
        @(negedge clk) begin pop = 0; push = 0; end
        repeat (2) @(negedge clk) pop = 1;
        @(negedge clk) pop = 0;
        #10 $finish;
    end
endmodule
