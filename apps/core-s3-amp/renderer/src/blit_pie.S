/* SPDX-License-Identifier: GPL-3.0-only */
.text
.macro blend_lanes
    movi a8, 7
    wsr.sar a8
    ee.vsr.32 q5, q2
    mov a8, a7
    ee.vld.128.ip q6, a8, 16
    ee.andq q5, q5, q6
    ee.vadds.s16.ld.incp q6, a8, q2, q2, q5
    ee.vld.128.ip q1, a2, 0
    beqz a6, .Ldestination_native\@
    ee.orq q4, q1, q1
    ee.vunzip.8 q1, q4
    ee.vzip.8 q4, q1
    ee.orq q1, q4, q4
.Ldestination_native\@:
    movi a9, 8
    wsr.sar a9
    ee.andq q4, q0, q6
    ee.andq q5, q1, q6
    ee.vsubs.s16 q4, q4, q5
    ee.vmul.s16.ld.incp q6, a8, q4, q4, q2
    ee.vadds.s16 q3, q4, q5
    ee.andq q4, q0, q6
    ee.andq q5, q1, q6
    ee.vsubs.s16 q4, q4, q5
    ee.vmul.s16 q4, q4, q2
    ee.vadds.s16.ld.incp q5, a8, q4, q4, q5
    ee.andq q4, q4, q6
    ee.orq q3, q3, q4
    movi a9, 1
    wsr.sar a9
    ee.vsr.32 q4, q0
    ee.vsr.32 q6, q1
    ee.andq q4, q4, q5
    ee.andq q6, q6, q5
    ee.vsubs.s16 q4, q4, q6
    movi a9, 8
    wsr.sar a9
    ee.vmul.s16 q4, q4, q2
    ee.vadds.s16 q4, q4, q6
    ee.andq q4, q4, q5
    movi a9, 1
    wsr.sar a9
    ee.vsl.32 q4, q4
    ee.orq q3, q3, q4
.endm
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
    extui a12, a4, 0, 3
    movi a13, 1
.Lalpha:
    /* Only the eight required A8 values; unaligned spans use two 64-bit reads. */
    ee.vld.l.64.ip q2, a4, 8
    beqz a12, .Lalpha_loaded
    ee.vld.h.64.ip q2, a4, 0
    wur.sar_byte a12
    ee.src.q q2, q2, q2
.Lalpha_loaded:
    ee.movi.32.a q2, a8, 0
    ee.movi.32.a q2, a9, 1
    and a10, a8, a9
    or a8, a8, a9
    beqz a8, .Lalpha_transparent
    beqz a11, .Lsource_aligned
    /* A transparent run invalidates the previous enclosing source vector. */
    beqz a13, .Lsource_cached
    ee.vld.128.ip q7, a3, 0
    movi a13, 0
.Lsource_cached:
    addi a3, a3, 16
    wur.sar_byte a11
    ee.vld.128.ip q4, a3, 0
    ee.src.q.qup q0, q7, q4
    j .Lsource_ready
.Lsource_aligned:
    ee.vld.128.ip q0, a3, 16
.Lsource_ready:
    addi a10, a10, 1
    beqz a10, .Lalpha_opaque
    ee.zero.q q5
    ee.vzip.8 q2, q5
    blend_lanes
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
    addi a3, a3, 16
    movi a13, 1
    addi a2, a2, 16
.Lalpha_advance:
    addi a5, a5, -1
    bnez a5, .Lalpha
    retw.n
.size deskkin_alpha_pie, .-deskkin_alpha_pie

.align 4
.global deskkin_sample_nearest_pie
.type deskkin_sample_nearest_pie,@function
/* Validated columns are four-byte entries with the source x in the low half.
 * Only requested source texels are read; destination is aligned and disjoint.
 * q0..q6/SAR are renderer-owned caller-clobbers. Stack scratch is above spill area. */
deskkin_sample_nearest_pie:
    entry a1, 48
    s32i a5, a1, 16
    s32i a6, a1, 20
    s32i a7, a1, 24
    beqz a5, .Lsample_opaque
.Lsample_alpha:
    l16ui a8, a4, 0
    l16ui a9, a4, 4
    addx2 a10, a8, a3
    addx2 a11, a9, a3
    l16ui a10, a10, 0
    l16ui a11, a11, 0
    slli a11, a11, 16
    or a10, a10, a11
    ee.movi.32.q q0, a10, 0
    add a8, a8, a5
    add a9, a9, a5
    l8ui a8, a8, 0
    l8ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q2, a8, 0
    l16ui a8, a4, 8
    l16ui a9, a4, 12
    addx2 a10, a8, a3
    addx2 a11, a9, a3
    l16ui a10, a10, 0
    l16ui a11, a11, 0
    slli a11, a11, 16
    or a10, a10, a11
    ee.movi.32.q q0, a10, 1
    add a8, a8, a5
    add a9, a9, a5
    l8ui a8, a8, 0
    l8ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q2, a8, 1
    l16ui a8, a4, 16
    l16ui a9, a4, 20
    addx2 a10, a8, a3
    addx2 a11, a9, a3
    l16ui a10, a10, 0
    l16ui a11, a11, 0
    slli a11, a11, 16
    or a10, a10, a11
    ee.movi.32.q q0, a10, 2
    add a8, a8, a5
    add a9, a9, a5
    l8ui a8, a8, 0
    l8ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q2, a8, 2
    l16ui a8, a4, 24
    l16ui a9, a4, 28
    addx2 a10, a8, a3
    addx2 a11, a9, a3
    l16ui a10, a10, 0
    l16ui a11, a11, 0
    slli a11, a11, 16
    or a10, a10, a11
    ee.movi.32.q q0, a10, 3
    add a8, a8, a5
    add a9, a9, a5
    l8ui a8, a8, 0
    l8ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q2, a8, 3
    l32i a6, a1, 24
    movi a7, deskkin_alpha_pie_masks
    blend_lanes
    beqz a6, .Lsample_alpha_store
    ee.orq q4, q3, q3
    ee.vunzip.8 q3, q4
    ee.vzip.8 q4, q3
    ee.orq q3, q4, q4
.Lsample_alpha_store:
    ee.vst.128.ip q3, a2, 16
    addi a4, a4, 32
    l32i a6, a1, 20
    addi a6, a6, -1
    s32i a6, a1, 20
    bnez a6, .Lsample_alpha
    retw.n
.Lsample_opaque:
    l16ui a8, a4, 0
    l16ui a9, a4, 4
    addx2 a8, a8, a3
    addx2 a9, a9, a3
    l16ui a8, a8, 0
    l16ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q0, a8, 0
    l16ui a8, a4, 8
    l16ui a9, a4, 12
    addx2 a8, a8, a3
    addx2 a9, a9, a3
    l16ui a8, a8, 0
    l16ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q0, a8, 1
    l16ui a8, a4, 16
    l16ui a9, a4, 20
    addx2 a8, a8, a3
    addx2 a9, a9, a3
    l16ui a8, a8, 0
    l16ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q0, a8, 2
    l16ui a8, a4, 24
    l16ui a9, a4, 28
    addx2 a8, a8, a3
    addx2 a9, a9, a3
    l16ui a8, a8, 0
    l16ui a9, a9, 0
    slli a9, a9, 16
    or a8, a8, a9
    ee.movi.32.q q0, a8, 3
    beqz a7, .Lsample_opaque_store
    ee.orq q1, q0, q0
    ee.vunzip.8 q0, q1
    ee.vzip.8 q1, q0
    ee.orq q0, q1, q1
.Lsample_opaque_store:
    ee.vst.128.ip q0, a2, 16
    addi a4, a4, 32
    addi a6, a6, -1
    bnez a6, .Lsample_opaque
    retw.n
.size deskkin_sample_nearest_pie, .-deskkin_sample_nearest_pie

.align 4
.global deskkin_cutout_pie
.type deskkin_cutout_pie,@function
/* Binary coverage was classified at upload. Each iteration consumes eight bits.
 * Unaligned source enclosing vectors are readable; destination writes are aligned.
 * q0..q3/SAR/SAR_BYTE are renderer-owned caller-clobbers. */
deskkin_cutout_pie:
    entry a1, 32
    extui a11, a3, 0, 4
    extui a12, a6, 8, 3
    extui a6, a6, 0, 1
.Lcutout:
    l8ui a8, a4, 0
    beqz a12, .Lcutout_bits
    l8ui a9, a4, 1
    slli a9, a9, 8
    or a8, a8, a9
    ssr a12
    srl a8, a8
    extui a8, a8, 0, 8
.Lcutout_bits:
    addi a4, a4, 1
    beqz a8, .Lcutout_skip
    beqz a11, .Lcutout_aligned
    ee.vld.128.ip q0, a3, 0
    addi a3, a3, 16
    ee.vld.128.ip q1, a3, 0
    wur.sar_byte a11
    ee.src.q q0, q0, q1
    j .Lcutout_color
.Lcutout_aligned:
    ee.vld.128.ip q0, a3, 16
.Lcutout_color:
    beqz a6, .Lcutout_ordered
    ee.orq q1, q0, q0
    ee.vunzip.8 q0, q1
    ee.vzip.8 q1, q0
    ee.orq q0, q1, q1
.Lcutout_ordered:
    movi a9, 255
    beq a8, a9, .Lcutout_store
    extui a9, a8, 0, 4
    extui a8, a8, 4, 4
    addx8 a9, a9, a7
    addx8 a8, a8, a7
    ee.vld.l.64.ip q2, a9, 0
    ee.vld.h.64.ip q2, a8, 0
    ee.vld.128.ip q1, a2, 0
    ee.xorq q0, q0, q1
    ee.andq q0, q0, q2
    ee.xorq q0, q0, q1
.Lcutout_store:
    ee.vst.128.ip q0, a2, 16
    j .Lcutout_next
.Lcutout_skip:
    addi a3, a3, 16
    addi a2, a2, 16
.Lcutout_next:
    addi a5, a5, -1
    bnez a5, .Lcutout
    retw.n
.size deskkin_cutout_pie, .-deskkin_cutout_pie
