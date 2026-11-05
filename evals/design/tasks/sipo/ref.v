// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "sipo".
module sipo (
    input wire clk,
    input wire rst_n,
    input wire din,
    input wire shift,
    output reg [7:0] q
);
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) q <= 8'd0;
        else if (shift) q <= {q[6:0], din};
    end
endmodule
