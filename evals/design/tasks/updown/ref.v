// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "updown".
module updown (
    input wire clk,
    input wire rst_n,
    input wire up,
    input wire down,
    output reg [3:0] count
);
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) count <= 4'd0;
        else if (up && !down) count <= count + 4'd1;
        else if (down && !up) count <= count - 4'd1;
    end
endmodule
