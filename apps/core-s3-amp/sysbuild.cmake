# SPDX-License-Identifier: MIT

string(REPLACE "procpu" "appcpu" REMOTE_CPU "${BOARD_QUALIFIERS}")
string(CONFIGURE "${BOARD}/${REMOTE_CPU}" DESKKIN_REMOTE_BOARD)
list(APPEND mcuboot_DTC_OVERLAY_FILE "${APP_DIR}/mcuboot.overlay")
list(REMOVE_DUPLICATES mcuboot_DTC_OVERLAY_FILE)
set(mcuboot_DTC_OVERLAY_FILE "${mcuboot_DTC_OVERLAY_FILE}" CACHE INTERNAL "")
list(APPEND mcuboot_EXTRA_CONF_FILE "${APP_DIR}/flash-qio.conf")
list(REMOVE_DUPLICATES mcuboot_EXTRA_CONF_FILE)
set(mcuboot_EXTRA_CONF_FILE "${mcuboot_EXTRA_CONF_FILE}" CACHE INTERNAL "")
if(${REMOTE_CPU} STREQUAL ${BOARD_QUALIFIERS})
  message(FATAL_ERROR "Deskkin AMP requires the PROCPU board target")
endif()

ExternalZephyrProject_Add(
  APPLICATION deskkin_core_s3_renderer
  SOURCE_DIR ${APP_DIR}/renderer
  BOARD ${DESKKIN_REMOTE_BOARD}
)

add_dependencies(core-s3-amp deskkin_core_s3_renderer)
sysbuild_add_dependencies(CONFIGURE core-s3-amp deskkin_core_s3_renderer)

if(SB_CONFIG_BOOTLOADER_MCUBOOT)
  sysbuild_add_dependencies(FLASH deskkin_core_s3_renderer mcuboot)
endif()
