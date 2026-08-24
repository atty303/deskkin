# SPDX-License-Identifier: GPL-3.0-only

cmake_minimum_required(VERSION 3.28.0)
find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})
project(deskkin_core_s3_device)
target_sources(app PRIVATE src/adapter.c)
rust_cargo_application()
