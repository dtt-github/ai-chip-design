// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "uart_rx".
module uart_rx #(parameter CLKS_PER_BIT = 4) (
    input wire clk,
    input wire rst_n,
    input wire rx,
    output reg [7:0] data,
    output reg valid
);
    // 0 idle, 1 start bit, 2 data bits, 3 stop bit.
    reg [1:0] state;
    reg [7:0] shreg;
    reg [2:0] bitn;
    reg [15:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state <= 2'd0;
            shreg <= 8'd0;
            bitn <= 3'd0;
            cnt <= 16'd0;
            data <= 8'd0;
            valid <= 1'b0;
        end else begin
            valid <= 1'b0;
            case (state)
                2'd0: begin
                    cnt <= 16'd0;
                    if (!rx) state <= 2'd1;
                end
                2'd1: begin
                    // Middle of the start bit: still 0, or it was a glitch.
                    if (cnt == CLKS_PER_BIT / 2 - 1) begin
                        cnt <= 16'd0;
                        bitn <= 3'd0;
                        state <= rx ? 2'd0 : 2'd2;
                    end else begin
                        cnt <= cnt + 16'd1;
                    end
                end
                2'd2: begin
                    if (cnt == CLKS_PER_BIT - 1) begin
                        cnt <= 16'd0;
                        shreg <= {rx, shreg[7:1]};
                        if (bitn == 3'd7) state <= 2'd3;
                        bitn <= bitn + 3'd1;
                    end else begin
                        cnt <= cnt + 16'd1;
                    end
                end
                default: begin
                    if (cnt == CLKS_PER_BIT - 1) begin
                        cnt <= 16'd0;
                        state <= 2'd0;
                        if (rx) begin
                            data <= shreg;
                            valid <= 1'b1;
                        end
                    end else begin
                        cnt <= cnt + 16'd1;
                    end
                end
            endcase
        end
    end
endmodule
