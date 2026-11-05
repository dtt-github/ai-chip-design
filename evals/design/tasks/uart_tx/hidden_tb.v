// SPDX-License-Identifier: Apache-2.0
// Hidden testbench for eval task "uart_tx": 4000 cycles of pseudo-random stimulus (with a
// reset in the middle), checked cycle by cycle against a behavioural model of the intended
// behaviour. The AI never sees this file.
`timescale 1ps/1ps
module tb_hidden_uart_tx;
    reg clk = 1'b0;
    reg rst_n = 1'b0;
    reg start = 1'b0;
    reg [7:0] data = 8'd0;
    wire tx;
    wire busy;
    reg [15:0] r = 16'hACE1;
    integer cyc;
    integer errors = 0;

    // Model state.
    reg m_tx;
    reg m_busy;
    reg [9:0] m_frame;
    integer m_bit;
    integer m_cnt;

    uart_tx dut (.clk(clk), .rst_n(rst_n), .start(start), .data(data), .tx(tx), .busy(busy));

    always #500 clk = ~clk;

    // Behavioural model, reset synchronously; compared only while reset is off.
    always @(posedge clk) begin
        if (!rst_n) begin
            m_tx <= 1'b1;
            m_busy <= 1'b0;
            m_bit <= 0;
            m_cnt <= 0;
        end else begin
            if (!m_busy) begin
                if (start) begin
                    m_frame <= {1'b1, data, 1'b0};
                    m_busy <= 1'b1;
                    m_tx <= 1'b0;
                    m_bit <= 0;
                    m_cnt <= 0;
                end
            end else if (m_cnt == 3) begin
                m_cnt <= 0;
                if (m_bit == 9) begin
                    m_busy <= 1'b0;
                    m_tx <= 1'b1;
                end else begin
                    m_bit <= m_bit + 1;
                    m_tx <= m_frame[m_bit + 1];
                end
            end else begin
                m_cnt <= m_cnt + 1;
            end
        end
    end

    initial begin
        repeat (2) @(negedge clk);
        rst_n = 1'b1;
        for (cyc = 0; cyc < 4000; cyc = cyc + 1) begin
            r = {r[14:0], r[15] ^ r[13] ^ r[12] ^ r[10]};
            if (cyc == 1000) rst_n = 1'b0;
            if (cyc == 1002) rst_n = 1'b1;
            start = (r[3:0] == 4'd0);
            data = r[15:8];
            @(negedge clk);
            #1;
            if (rst_n) begin
                if (tx !== m_tx) begin
                    if (errors < 5) $display("FAIL: cycle %0d: tx = 0x%0h, expected 0x%0h", cyc, tx, m_tx);
                    errors = errors + 1;
                end
                if (busy !== m_busy) begin
                    if (errors < 5) $display("FAIL: cycle %0d: busy = 0x%0h, expected 0x%0h", cyc, busy, m_busy);
                    errors = errors + 1;
                end
            end
        end
        if (errors == 0) $display("PASS: hidden checks for uart_tx");
        else $display("FAIL: hidden checks for uart_tx: %0d mismatch(es)", errors);
        $finish;
    end
endmodule
