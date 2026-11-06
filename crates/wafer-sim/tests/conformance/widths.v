// Expression width, sign and select corner cases.
module tb;
    reg [7:0] u8 = 8'hC8;            // 200
    reg signed [7:0] s8 = -8'sd56;    // 0xC8
    reg [3:0] u4 = 4'hA;
    reg [0:7] asc = 8'b1000_0001;    // ascending range
    reg [15:0] w16;
    reg [4:0] r5;
    integer i = -5;
    reg [63:0] big = 64'hFEDC_BA98_7654_3210;
    initial begin
        w16 = u8 + u8;              $display("a %0d", w16);
        r5 = u4 + u4;               $display("b %0d", r5);
        $display("c %0d", u8 + u8);
        $display("d %b", u8 > s8);
        $display("e %b", s8 < 0);
        $display("f %0d", s8 >>> 2);
        $display("g %0d", s8 >> 2);
        $display("h %0d", $signed(u4));
        $display("i %0d", $unsigned(s8));
        $display("j %b %b %b", asc[0], asc[7], asc[0:3]);
        $display("k %h", {u4, 4'h5, u8[3:0]});
        $display("l %h", {3{u4[1:0]}});
        $display("m %0d", i * 3);
        $display("n %0d", i >>> 1);
        $display("o %h", big[47:16]);
        $display("p %h", big >> 60);
        $display("q %h", ~u4);
        $display("r %0d", -u4);
        $display("s %b", !u4);
        $display("t %0d", u8[6 -: 3]);
        $display("u %0d", 4'd9 - 4'd12);
        $display("v %0d", 10 - 12);
        $display("w %b", 3'b101 == 5'b00101);
        $display("x %0d %0d", 7 / 2, -7 / 2);
        $display("y %0d %0d", 7 % 3, -7 % 3);
        $display("z %h", 16'h00FF << 4);
    end
endmodule
