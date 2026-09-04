/* SPDX-License-Identifier: GPL-3.0-only */

.text
.align 4
.global deskkin_background_pie
.type deskkin_background_pie,@function
/* Aligned destination/pattern; 1..40 vectors. CP3 enabled and IRQs locked.
 * Preserve the only PIE register used. The ABI supplies a 16-byte aligned SP. */
deskkin_background_pie:
    /* Keep the upper 32 bytes available for windowed-ABI register spills. */
    entry a1, 48
    mov a5, a1
    ee.vst.128.ip q0, a5, 0
    ee.vld.128.ip q0, a3, 0
1:
    ee.vst.128.ip q0, a2, 16
    addi a4, a4, -1
    bnez a4, 1b
    ee.vld.128.ip q0, a5, 0
    retw.n
.size deskkin_background_pie, .-deskkin_background_pie
