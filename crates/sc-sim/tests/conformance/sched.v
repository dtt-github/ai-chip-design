// Scheduling: blocking vs nonblocking, delayed NBAs, events, $strobe and $monitor.
module tb;
    reg clk = 0;
    reg [7:0] a = 0, b = 0, c = 0, d = 0;
    reg [7:0] pipe1 = 0, pipe2 = 0, pipe3 = 0;
    always #10 clk = ~clk;
    always @(posedge clk) begin
        pipe1 <= a;
        pipe2 <= pipe1;
        pipe3 <= pipe2;
    end
    always @(posedge clk) b = a;
    always @(b) c = b + 1;
    initial begin
        $monitor("mon a=%0d p1=%0d p2=%0d p3=%0d c=%0d", a, pipe1, pipe2, pipe3, c);
        repeat (4) begin
            @(negedge clk) a = a + 3;
            $strobe("strobe t=%0d a=%0d d=%0d", $time, a, d);
        end
        d <= #5 8'd77;
        #2 $display("before d=%0d", d);
        #4 $display("after d=%0d", d);
        #40 $finish;
    end
endmodule
