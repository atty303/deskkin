# SPDX-License-Identifier: MIT

cmake_minimum_required(VERSION 3.28.0)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(deskkin_core_s3_amp)
target_sources(app PRIVATE src/main.c ../core-s3-service/src/adapter.c)
zephyr_linker_sources(SECTIONS amp-sram2.ld)
zephyr_linker_sources(SECTIONS amp-dram-boundary.ld)
rust_cargo_application()
