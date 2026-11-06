// Sequential shift-add multiplier with start/done handshake.
module mult (input clk, input rst, input start, input [7:0] a, input [7:0] b, output reg [15:0] p, output reg done);
    reg [15:0] acc, ma;
    reg [7:0] mb;
    reg [3:0] n;
    reg running;
    always @(posedge clk) begin
        if (rst) begin running <= 0; done <= 0; p <= 0; acc <= 0; ma <= 0; mb <= 0; n <= 0; end
        else if (start && !running) begin
            running <= 1; done <= 0; acc <= 0; ma <= {8'd0, a}; mb <= b; n <= 0;
        end else if (running) begin
            if (mb[0]) acc <= acc + ma;
            ma <= ma << 1;
            mb <= mb >> 1;
            n <= n + 1;
            if (n == 4'd7) begin running <= 0; done <= 1; p <= mb[0] ? acc + ma : acc; end
        end else done <= 0;
    end
endmodule

module tb;
    reg clk = 0, rst = 1, start = 0;
    reg [7:0] a = 0, b = 0;
    wire [15:0] p;
    wire done;
    integer i;
    mult m (.clk(clk), .rst(rst), .start(start), .a(a), .b(b), .p(p), .done(done));
    always #5 clk = ~clk;
    initial begin
        @(negedge clk) rst = 0;
        for (i = 0; i < 6; i = i + 1) begin
            @(negedge clk) begin a = 8'd13 * i + 7; b = 8'd255 - 8'd31 * i; start = 1; end
            @(negedge clk) start = 0;
            @(posedge done) #1 $display("%0d * %0d = %0d (%s)", a, b, p, (p == a * b) ? "ok" : "BAD");
        end
        $finish;
    end
endmodule
