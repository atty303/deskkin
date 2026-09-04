/* SPDX-License-Identifier: GPL-3.0-only */
.text
.align 4
.global deskkin_copy_pie
.type deskkin_copy_pie,@function
/* 1..4 vectors, aligned destination, disjoint slices. For unaligned source,
 * the caller guarantees both enclosing vectors are readable. IRQ/CP3 owned
 * by caller. The upper 32 frame bytes belong to windowed-ABI spills. */
deskkin_copy_pie:
    entry a1, 112
    mov a6, a1
    ee.vst.128.ip q0, a6, 16
    ee.vst.128.ip q1, a6, 16
    extui a7, a3, 0, 4
    bnez a7, .Lunaligned
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
.Lunaligned:
    ee.vst.128.ip q2, a6, 16
    ee.vst.128.ip q3, a6, 16
    rur.sar_byte a7
    s32i a7, a1, 64
    ee.ld.128.usar.ip q0, a3, 16
.Lfunnel:
    ee.vld.128.ip q1, a3, 16
    ee.src.q.qup q2, q0, q1
    beqz a5, .Lnative
    ee.orq q3, q2, q2
    ee.vunzip.8 q2, q3
    ee.vzip.8 q3, q2
    ee.vst.128.ip q3, a2, 16
    j .Lnext
.Lnative:
    ee.vst.128.ip q2, a2, 16
.Lnext:
    addi a4, a4, -1
    bnez a4, .Lfunnel
    l32i a7, a1, 64
    wur.sar_byte a7
    mov a6, a1
    ee.vld.128.ip q0, a6, 16
    ee.vld.128.ip q1, a6, 16
    ee.vld.128.ip q2, a6, 16
    ee.vld.128.ip q3, a6, 0
    retw.n
.size deskkin_copy_pie, .-deskkin_copy_pie
