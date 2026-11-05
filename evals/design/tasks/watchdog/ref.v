// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "watchdog".
module watchdog #(parameter TIMEOUT = 8) (
    input wire clk,
    input wire rst_n,
    input wire kick,
    output reg bark
);
    reg [15:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            cnt <= 16'd0;
            bark <= 1'b0;
        end else if (kick) begin
            cnt <= 16'd0;
            bark <= 1'b0;
        end else if (cnt == TIMEOUT - 1) begin
            bark <= 1'b1;
        end else begin
            cnt <= cnt + 16'd1;
        end
    end
endmodule
