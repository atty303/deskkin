# SPDX-License-Identifier: MIT

cmake_minimum_required(VERSION 3.28.0)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(deskkin_core_s3_amp)
target_sources(app PRIVATE src/main.c)
zephyr_linker_sources(SECTIONS amp-dram-boundary.ld)
target_link_options(app PUBLIC
  -Wl,--wrap=esp_init_psram
  -Wl,--wrap=esp_psram_smh_init
)
