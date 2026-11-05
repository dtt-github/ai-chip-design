// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "fifo4".
module fifo4 (
    input wire clk,
    input wire rst_n,
    input wire wr,
    input wire [7:0] din,
    input wire rd,
    output wire [7:0] dout,
    output wire empty,
    output wire full
);
    reg [7:0] mem [0:3];
    reg [1:0] wptr;
    reg [1:0] rptr;
    reg [2:0] count;
    wire do_wr = wr && !full;
    wire do_rd = rd && !empty;
    always @(posedge clk or negedge rst_n) begin
        if (!rst_n) begin
            wptr <= 2'd0;
            rptr <= 2'd0;
            count <= 3'd0;
        end else begin
            if (do_wr) begin
                mem[wptr] <= din;
                wptr <= wptr + 2'd1;
            end
            if (do_rd) rptr <= rptr + 2'd1;
            if (do_wr && !do_rd) count <= count + 3'd1;
            else if (do_rd && !do_wr) count <= count - 3'd1;
        end
    end
    assign dout = mem[rptr];
    assign empty = count == 3'd0;
    assign full = count == 3'd4;
endmodule
