/* SPDX-License-Identifier: GPL-3.0-only */

.text
.align 4
.global deskkin_background_pie
.type deskkin_background_pie,@function
/* Aligned destination/pattern; 1..40 vectors. CP3 enabled by renderer.
 * q0 is caller-clobbered; no PIE value survives a kernel call. */
deskkin_background_pie:
    entry a1, 32
    ee.vld.128.ip q0, a3, 0
1:
    ee.vst.128.ip q0, a2, 16
    addi a4, a4, -1
    bnez a4, 1b
    retw.n
.size deskkin_background_pie, .-deskkin_background_pie
