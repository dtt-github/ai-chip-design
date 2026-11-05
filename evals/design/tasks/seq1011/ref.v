// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "seq1011".
module seq1011 (
    input wire clk,
    input wire rst_n,
    input wire din,
    output reg found
);
    reg [2:0] last;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            last <= 3'd0;
            found <= 1'b0;
        end else begin
            found <= {last, din} == 4'b1011;
            last <= {last[1:0], din};
        end
    end
endmodule
