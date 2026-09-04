/* SPDX-License-Identifier: MIT */

/* The image end includes the named display-band input section in .dram0.noinit.
 * Static growth consumes this pool automatically; no framebuffer-sized reserve.
 */
__deskkin_internal_free_start = ALIGN(_end, 32);
__deskkin_internal_free_end = ORIGIN(dram0_0_seg) + LENGTH(dram0_0_seg);
ASSERT(__deskkin_internal_free_end - __deskkin_internal_free_start >= 1024,
       "PROCPU static allocations leave less than 1 KiB of internal runtime SRAM")
ASSERT(__deskkin_internal_free_start < __deskkin_internal_free_end,
       "PROCPU static allocations overlap the APPCPU reservation")
ASSERT(_ext_ram_heap_end <= 0x3c400000,
       "PROCPU external state and heap overlap the APPCPU high 4 MiB PSRAM region")
