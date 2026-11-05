// SPDX-License-Identifier: Apache-2.0
// Reference design for eval task "parity8".
module parity8 (
    input wire [7:0] d,
    output wire odd
);
    assign odd = ^d;
endmodule
