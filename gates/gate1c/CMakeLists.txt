# SPDX-License-Identifier: MIT

cmake_minimum_required(VERSION 3.28.0)

find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})

project(deskkin_gate1c)
string(LENGTH "${DESKKIN_FIRMWARE_DIGEST}" DESKKIN_FIRMWARE_DIGEST_LENGTH)
if(NOT DEFINED DESKKIN_FIRMWARE_DIGEST OR
   NOT DESKKIN_FIRMWARE_DIGEST_LENGTH EQUAL 64 OR
   NOT DESKKIN_FIRMWARE_DIGEST MATCHES "^[0-9a-f]+$")
  message(FATAL_ERROR "DESKKIN_FIRMWARE_DIGEST must be a lowercase SHA-256 digest")
endif()
target_sources(app PRIVATE src/abi.c)
target_compile_definitions(app PRIVATE DESKKIN_FIRMWARE_DIGEST="${DESKKIN_FIRMWARE_DIGEST}")
rust_cargo_application()
