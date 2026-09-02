/* SPDX-License-Identifier: MIT */

/*
 * A 32 KiB data SRAM bank remains available above the ESP32-S3 32 KiB
 * data-cache window. Keep cache-disable-safe worker stacks here so the main
 * SRAM bank can retain the two DMA-capable full-screen framebuffers.
 */
.sram2.noinit 0x3fcf0000 (NOLOAD) :
{
	. = ALIGN(16);
	__deskkin_sram2_start = .;
	KEEP(*(.sram2.noinit))
	KEEP(*(.sram2.noinit.*))
	. = ALIGN(16);
	__deskkin_sram2_end = .;
}

ASSERT(SIZEOF(.sram2.noinit) <= 0x8000,
	"Deskkin SRAM2 allocations exceed the 32 KiB cache-independent bank")
