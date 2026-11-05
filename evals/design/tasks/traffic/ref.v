// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "traffic".
module traffic #(parameter GREEN = 8, parameter YELLOW = 2, parameter RED = 6) (
    input wire clk,
    input wire rst_n,
    output wire red,
    output wire yellow,
    output wire green
);
    // 0 red, 1 green, 2 yellow.
    reg [1:0] state;
    reg [15:0] cnt;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            state <= 2'd0;
            cnt <= 16'd0;
        end else begin
            case (state)
                2'd0: if (cnt == RED - 1) begin state <= 2'd1; cnt <= 16'd0; end else cnt <= cnt + 16'd1;
                2'd1: if (cnt == GREEN - 1) begin state <= 2'd2; cnt <= 16'd0; end else cnt <= cnt + 16'd1;
                2'd2: if (cnt == YELLOW - 1) begin state <= 2'd0; cnt <= 16'd0; end else cnt <= cnt + 16'd1;
                default: begin state <= 2'd0; cnt <= 16'd0; end
            endcase
        end
    end
    assign red = state == 2'd0;
    assign green = state == 2'd1;
    assign yellow = state == 2'd2;
endmodule
