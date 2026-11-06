// UART transmitter (8N1) with a receiver model in the testbench.
`timescale 1ns/1ps
module uart_tx #(parameter CLKS_PER_BIT = 4) (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       start,
    input  wire [7:0] data,
    output reg        tx,
    output reg        busy
);
    localparam IDLE = 2'd0, START = 2'd1, DATA = 2'd2, STOP = 2'd3;
    reg [1:0] state;
    reg [7:0] shreg;
    reg [2:0] bitn;
    reg [7:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state <= IDLE; tx <= 1'b1; busy <= 1'b0; shreg <= 8'd0; bitn <= 3'd0; cnt <= 8'd0;
        end else begin
            case (state)
                IDLE: if (start) begin state <= START; shreg <= data; busy <= 1'b1; cnt <= 0; tx <= 1'b0; end
                START: if (cnt == CLKS_PER_BIT - 1) begin cnt <= 0; state <= DATA; tx <= shreg[0]; bitn <= 0; end
                       else cnt <= cnt + 1;
                DATA: if (cnt == CLKS_PER_BIT - 1) begin
                          cnt <= 0;
                          if (bitn == 3'd7) begin state <= STOP; tx <= 1'b1; end
                          else begin bitn <= bitn + 1; shreg <= shreg >> 1; tx <= shreg[1]; end
                      end else cnt <= cnt + 1;
                STOP: if (cnt == CLKS_PER_BIT - 1) begin state <= IDLE; busy <= 1'b0; cnt <= 0; end
                      else cnt <= cnt + 1;
            endcase
        end
    end
endmodule

module tb;
    reg clk = 0, rst_n = 0, start = 0;
    reg [7:0] data = 0;
    wire tx, busy;
    reg [7:0] rx;
    integer k, n;
    uart_tx #(.CLKS_PER_BIT(4)) dut (.clk(clk), .rst_n(rst_n), .start(start), .data(data), .tx(tx), .busy(busy));
    always #5 clk = ~clk;
    initial begin
        #12 rst_n = 1;
        for (n = 0; n < 3; n = n + 1) begin
            @(negedge clk);
            data = 8'hA5 + n * 8'h11;
            start = 1;
            @(negedge clk);
            start = 0;
            // Sample the middle of each bit.
            @(negedge tx);
            #20;
            for (k = 0; k < 8; k = k + 1) begin
                #40 rx[k] = tx;
            end
            #40 $display("sent %h received %h stop=%b busy=%b t=%0d", data, rx, tx, busy, $time);
            wait (!busy);
        end
        $finish;
    end
endmodule
