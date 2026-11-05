// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "fifo4": 3000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_fifo4;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg wr = 1'b0;
    reg rd = 1'b0;
    reg [7:0] din = 8'd0;
    wire [7:0] dout;
    wire empty;
    wire full;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg [7:0] m_mem [0:3];
    reg [1:0] m_w;
    reg [1:0] m_r;
    reg [2:0] m_n;
    reg m_wr;
    reg m_rd;

    fifo4 dut (.clk(clk), .rst_n(rst_n), .wr(wr), .din(din), .rd(rd), .dout(dout), .empty(empty), .full(full));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_w <= 2'd0;
            m_r <= 2'd0;
            m_n <= 3'd0;
        end else begin
            m_wr = wr && (m_n != 3'd4);
            m_rd = rd && (m_n != 3'd0);
            if (m_wr) begin
                m_mem[m_w] <= din;
                m_w <= m_w + 2'd1;
            end
            if (m_rd) m_r <= m_r + 2'd1;
            if (m_wr && !m_rd) m_n <= m_n + 3'd1;
            else if (m_rd && !m_wr) m_n <= m_n - 3'd1;
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 3000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            // Alternate between filling and draining so both full and empty are exercised.
            if (cyc % 200 < 100) begin
                wr = (r[1:0] != 2'd0);
                rd = (r[3:2] == 2'd0);
            end else begin
                wr = (r[1:0] == 2'd0);
                rd = (r[3:2] != 2'd0);
            end
            din = r[15:8];
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (empty !== (m_n == 3'd0)) begin
                    if (errors < 5) $display("FAIL: cycle %0d: empty = 0x%0h, expected 0x%0h", cyc, empty, (m_n == 3'd0));
                    errors = errors + 1;
                end
                if (full !== (m_n == 3'd4)) begin
                    if (errors < 5) $display("FAIL: cycle %0d: full = 0x%0h, expected 0x%0h", cyc, full, (m_n == 3'd4));
                    errors = errors + 1;
                end
                if (m_n != 3'd0) begin
                    if (dout !== m_mem[m_r]) begin
                        if (errors < 5) $display("FAIL: cycle %0d: dout = 0x%0h, expected 0x%0h", cyc, dout, m_mem[m_r]);
                        errors = errors + 1;
                    end
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for fifo4");
        else $display("FAIL: hidden checks for fifo4: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
