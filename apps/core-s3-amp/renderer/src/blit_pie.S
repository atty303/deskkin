/* SPDX-License-Identifier: GPL-3.0-only */
.text
.align 4
.global deskkin_copy_pie
.type deskkin_copy_pie,@function
/* Nonempty aligned destination, disjoint slices; CP3 enabled by renderer.
 * Unaligned source enclosing vectors must be readable. q0..q3/SAR_BYTE are
 * caller-clobbered; no other thread or ISR on APPCPU uses PIE. */
deskkin_copy_pie:
    entry a1, 32
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
    retw.n
.Lunaligned:
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
    retw.n
.size deskkin_copy_pie, .-deskkin_copy_pie

.align 4
.global deskkin_alpha_pie
.type deskkin_alpha_pie,@function
/* Nonempty aligned destination, enclosing source/A8 vectors readable.
 * q0..q7, SAR and SAR_BYTE are caller-clobbered; a7 points to masks. */
deskkin_alpha_pie:
    entry a1, 32
    extui a11, a3, 0, 4
    beqz a11, .Lalpha
    ee.ld.128.usar.ip q7, a3, 16
.Lalpha:
    beqz a11, .Lsource_aligned
    wur.sar_byte a11
    ee.vld.128.ip q4, a3, 16
    ee.src.q.qup q0, q7, q4
    j .Lsource_ready
.Lsource_aligned:
    ee.vld.128.ip q0, a3, 16
.Lsource_ready:
    mov a8, a4
    extui a9, a4, 0, 4
    ee.ld.128.usar.ip q4, a8, 16
    ee.zero.q q5
    movi a10, 8
    bgeu a10, a9, .Lalpha_loaded
    ee.vld.128.ip q5, a8, 0
.Lalpha_loaded:
    ee.src.q q2, q4, q5
    addi a4, a4, 8
    ee.movi.32.a q2, a8, 0
    ee.movi.32.a q2, a9, 1
    and a10, a8, a9
    or a8, a8, a9
    beqz a8, .Lalpha_transparent
    addi a10, a10, 1
    beqz a10, .Lalpha_opaque
    ee.zero.q q5
    ee.vzip.8 q2, q5
    movi a8, 7
    wsr.sar a8
    ee.vsr.32 q5, q2
    mov a8, a7
    ee.vld.128.ip q6, a8, 16
    ee.andq q5, q5, q6
    ee.vadds.s16 q2, q2, q5
    ee.vld.128.ip q1, a2, 0
    beqz a6, .Ldestination_native
    ee.orq q4, q1, q1
    ee.vunzip.8 q1, q4
    ee.vzip.8 q4, q1
    ee.orq q1, q4, q4
.Ldestination_native:
    movi a9, 8
    wsr.sar a9
    ee.vld.128.ip q6, a8, 16
    ee.andq q4, q0, q6
    ee.andq q5, q1, q6
    ee.vsubs.s16 q4, q4, q5
    ee.vmul.s16 q4, q4, q2
    ee.vadds.s16 q3, q4, q5
    ee.vld.128.ip q6, a8, 16
    ee.andq q4, q0, q6
    ee.andq q5, q1, q6
    ee.vsubs.s16 q4, q4, q5
    ee.vmul.s16 q4, q4, q2
    ee.vadds.s16 q4, q4, q5
    ee.andq q4, q4, q6
    ee.orq q3, q3, q4
    ee.vld.128.ip q6, a8, 0
    movi a9, 1
    wsr.sar a9
    ee.vsr.32 q4, q0
    ee.vsr.32 q5, q1
    ee.andq q4, q4, q6
    ee.andq q5, q5, q6
    ee.vsubs.s16 q4, q4, q5
    movi a9, 8
    wsr.sar a9
    ee.vmul.s16 q4, q4, q2
    ee.vadds.s16 q4, q4, q5
    ee.andq q4, q4, q6
    movi a9, 1
    wsr.sar a9
    ee.vsl.32 q4, q4
    ee.orq q3, q3, q4
.Lalpha_encode:
    beqz a6, .Lalpha_store
    ee.orq q4, q3, q3
    ee.vunzip.8 q3, q4
    ee.vzip.8 q4, q3
    ee.orq q3, q4, q4
.Lalpha_store:
    ee.vst.128.ip q3, a2, 16
    j .Lalpha_advance
.Lalpha_opaque:
    ee.orq q3, q0, q0
    j .Lalpha_encode
.Lalpha_transparent:
    addi a2, a2, 16
.Lalpha_advance:
    addi a5, a5, -1
    bnez a5, .Lalpha
    retw.n
.size deskkin_alpha_pie, .-deskkin_alpha_pie
