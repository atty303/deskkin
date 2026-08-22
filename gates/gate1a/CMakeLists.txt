# SPDX-License-Identifier: MIT

cmake_minimum_required(VERSION 3.28.0)

find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})

project(deskkin_gate1a)
rust_cargo_application()
