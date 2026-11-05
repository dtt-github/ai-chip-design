// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "uart_tx".
module uart_tx #(parameter CLKS_PER_BIT = 4) (
    input wire clk,
    input wire rst_n,
    input wire start,
    input wire [7:0] data,
    output reg tx,
    output reg busy
);
    reg [9:0] frame;
    reg [3:0] bitn;
    reg [15:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            tx <= 1'b1;
            busy <= 1'b0;
            frame <= 10'd0;
            bitn <= 4'd0;
            cnt <= 16'd0;
        end else if (!busy) begin
            if (start) begin
                // Stop bit, data (LSB first), start bit.
                frame <= {1'b1, data, 1'b0};
                busy <= 1'b1;
                tx <= 1'b0;
                bitn <= 4'd0;
                cnt <= 16'd0;
            end
        end else if (cnt == CLKS_PER_BIT - 1) begin
            cnt <= 16'd0;
            if (bitn == 4'd9) begin
                busy <= 1'b0;
                tx <= 1'b1;
            end else begin
                bitn <= bitn + 4'd1;
                tx <= frame[bitn + 4'd1];
            end
        end else begin
            cnt <= cnt + 16'd1;
        end
    end
endmodule
