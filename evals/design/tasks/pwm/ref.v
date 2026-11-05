// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "pwm".
module pwm (
    input wire clk,
    input wire rst_n,
    input wire [7:0] duty,
    output wire out
);
    reg [7:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) cnt <= 8'd0;
        else cnt <= cnt + 8'd1;
    end
    assign out = cnt < duty;
endmodule
