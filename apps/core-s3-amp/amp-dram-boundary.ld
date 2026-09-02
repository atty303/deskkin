/* SPDX-License-Identifier: MIT */

ASSERT(_end <= 0x3fce5c00,
       "PROCPU DRAM overlaps the APPCPU image beginning at 0x3fce5c00")
ASSERT(_ext_ram_heap_end <= 0x3c400000,
       "PROCPU external state and heap overlap the APPCPU high 4 MiB PSRAM region")
