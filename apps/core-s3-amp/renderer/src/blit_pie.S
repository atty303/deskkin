/* SPDX-License-Identifier: GPL-3.0-only */
.text
.align 4
.global deskkin_copy_pie
.type deskkin_copy_pie,@function
/* 1..4 vectors, aligned pointers, disjoint slices. IRQ/CP3 owned by caller. */
deskkin_copy_pie:
    entry a1, 64
    mov a6, a1
    ee.vst.128.ip q0, a6, 16
    ee.vst.128.ip q1, a6, 0
1:
    ee.vld.128.ip q0, a3, 16
    beqz a5, 2f
    ee.orq q1, q0, q0
    ee.vunzip.8 q0, q1
    ee.vzip.8 q1, q0
    ee.vst.128.ip q1, a2, 16
    j 3f
2:
    ee.vst.128.ip q0, a2, 16
3:
    addi a4, a4, -1
    bnez a4, 1b
    mov a6, a1
    ee.vld.128.ip q0, a6, 16
    ee.vld.128.ip q1, a6, 0
    retw.n
.size deskkin_copy_pie, .-deskkin_copy_pie
