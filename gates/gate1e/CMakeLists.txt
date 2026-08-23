# SPDX-License-Identifier: GPL-3.0-only

cmake_minimum_required(VERSION 3.28.0)

find_package(Zephyr REQUIRED HINTS $ENV{ZEPHYR_BASE})

project(deskkin_gate1e)
foreach(name IN ITEMS DESKKIN_FIRMWARE_DIGEST DESKKIN_WORKLOAD_DIGEST)
  string(LENGTH "${${name}}" value_length)
  if(NOT DEFINED ${name} OR NOT value_length EQUAL 64 OR
     NOT ${name} MATCHES "^[0-9a-f]+$")
    message(FATAL_ERROR "${name} must be a lowercase SHA-256 digest")
  endif()
endforeach()
target_sources(app PRIVATE src/adapter.c)
target_compile_definitions(
  app PRIVATE
  DESKKIN_FIRMWARE_DIGEST="${DESKKIN_FIRMWARE_DIGEST}"
  DESKKIN_WORKLOAD_DIGEST="${DESKKIN_WORKLOAD_DIGEST}"
)
rust_cargo_application()
