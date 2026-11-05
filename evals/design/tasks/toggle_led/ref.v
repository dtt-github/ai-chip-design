// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "toggle_led".
module toggle_led (
    input wire clk,
    input wire rst_n,
    input wire button,
    output reg led
);
    reg prev;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            prev <= 1'b0;
            led <= 1'b0;
        end else begin
            prev <= button;
            if (button && !prev) led <= ~led;
        end
    end
endmodule
