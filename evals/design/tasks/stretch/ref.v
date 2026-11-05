// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "stretch".
module stretch #(parameter N = 4) (
    input wire clk,
    input wire rst_n,
    input wire din,
    output reg dout
);
    reg [15:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            cnt <= 16'd0;
            dout <= 1'b0;
        end else if (din) begin
            cnt <= N - 1;
            dout <= 1'b1;
        end else if (cnt != 16'd0) begin
            cnt <= cnt - 16'd1;
        end else begin
            dout <= 1'b0;
        end
    end
endmodule
