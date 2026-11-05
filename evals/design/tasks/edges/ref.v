// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "edges".
module edges (
    input wire clk,
    input wire rst_n,
    input wire din,
    output reg rise,
    output reg fall
);
    reg prev;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            prev <= 1'b0;
            rise <= 1'b0;
            fall <= 1'b0;
        end else begin
            prev <= din;
            rise <= din && !prev;
            fall <= !din && prev;
        end
    end
endmodule
