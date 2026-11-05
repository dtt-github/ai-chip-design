// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "gray4".
module gray4 (
    input wire clk,
    input wire rst_n,
    input wire en,
    output wire [3:0] gray
);
    reg [3:0] bin;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) bin <= 4'd0;
        else if (en) bin <= bin + 4'd1;
    end
    assign gray = bin ^ (bin >> 1);
endmodule
