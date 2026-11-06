// Phase 0 gate design: UART 8N1 transmitter and its self-checking testbench.
// SPDX-License-Identifier: Apache-2.0
// UART transmitter, 8 data bits, no parity, 1 stop bit (8N1).
`default_nettype none

module uart_tx #(
    parameter CLKS_PER_BIT = 8
) (
    input  wire       clk,
    input  wire       rst_n,
    input  wire       start,
    input  wire [7:0] data,
    output reg        tx,
    output wire       busy
);
    localparam IDLE = 2'd0, START = 2'd1, DATA = 2'd2, STOP = 2'd3;

    reg [1:0] state;
    reg [7:0] shreg;
    reg [2:0] bit_idx;
    reg [7:0] clk_cnt;

    assign busy = (state != IDLE);
    wire bit_done = (clk_cnt == CLKS_PER_BIT - 1);

    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state   <= IDLE;
            tx      <= 1'b1;
            shreg   <= 8'd0;
            bit_idx <= 3'd0;
            clk_cnt <= 8'd0;
        end else begin
            clk_cnt <= bit_done ? 8'd0 : clk_cnt + 8'd1;
            case (state)
                IDLE: begin
                    clk_cnt <= 8'd0;
                    if (start) begin
                        shreg <= data;
                        tx    <= 1'b0;  // start bit
                        state <= START;
                    end
                end
                START: if (bit_done) begin
                    tx      <= shreg[0];
                    bit_idx <= 3'd0;
                    state   <= DATA;
                end
                DATA: if (bit_done) begin
                    if (bit_idx == 3'd7) begin
                        tx    <= 1'b1;  // stop bit
                        state <= STOP;
                    end else begin
                        bit_idx <= bit_idx + 3'd1;
                        shreg   <= {1'b0, shreg[7:1]};
                        tx      <= shreg[1];
                    end
                end
                STOP: if (bit_done) state <= IDLE;
            endcase
        end
    end
endmodule

`default_nettype wire

// SPDX-License-Identifier: Apache-2.0
// Sends bytes through uart_tx and decodes the serial line like a receiver would.
`timescale 1ns/1ps

module tb_uart_tx;
    localparam CPB = 8;          // clocks per bit
    localparam BIT = CPB * 20;   // ns per bit at 50 MHz

    reg clk = 0, rst_n = 0, start = 0;
    reg [7:0] data = 0;
    wire tx, busy;
    reg [7:0] rx;
    reg [7:0] expected [0:3];
    integer n, k, errors = 0;

    uart_tx #(.CLKS_PER_BIT(CPB)) dut (.clk(clk), .rst_n(rst_n), .start(start), .data(data), .tx(tx), .busy(busy));

    always #10 clk = ~clk;

    initial begin
        expected[0] = 8'h55; expected[1] = 8'hA5; expected[2] = 8'h00; expected[3] = 8'hFF;
        #35 rst_n = 1;
        for (n = 0; n < 4; n = n + 1) begin
            @(negedge clk);
            data = expected[n];
            start = 1;
            // Receiver: find the start bit, then sample each bit in its middle.
            @(negedge tx);
            start = 0;
            #(BIT / 2);
            if (tx !== 1'b0) begin $display("FAIL: no start bit"); errors = errors + 1; end
            for (k = 0; k < 8; k = k + 1) begin
                #(BIT);
                rx[k] = tx;
            end
            #(BIT);
            if (tx !== 1'b1) begin $display("FAIL: no stop bit"); errors = errors + 1; end
            if (rx == expected[n]) $display("byte %0d: sent %h, received %h  ok", n, expected[n], rx);
            else begin $display("byte %0d: sent %h, received %h  MISMATCH", n, expected[n], rx); errors = errors + 1; end
            wait (!busy);
        end
        if (errors == 0) $display("PASS: %0d bytes", n);
        else $display("FAIL: %0d errors", errors);
        $finish;
    end
endmodule
