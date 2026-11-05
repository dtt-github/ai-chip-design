// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "blinker".
module blinker #(parameter N = 4) (
    input wire clk,
    input wire rst_n,
    input wire en,
    output reg led
);
    reg [15:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            cnt <= 16'd0;
            led <= 1'b0;
        end else if (!en) begin
            cnt <= 16'd0;
            led <= 1'b0;
        end else if (cnt == N - 1) begin
            cnt <= 16'd0;
            led <= ~led;
        end else begin
            cnt <= cnt + 16'd1;
        end
    end
endmodule
