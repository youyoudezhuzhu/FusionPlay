# Project-level configuration.
cmake_minimum_required(VERSION 3.14)
project(FusionPlay LANGUAGES CXX)

# The name of the executable created for the application. Change this to change
# the on-disk name of your application.
set(BINARY_NAME "FusionPlay")

# === FusionPlay version metadata ===
# VERSION holds the user-visible version (for example 1.3.0) shown by the UI and
# the VERSIONINFO strings, while INSTALLER_VERSION holds the 4-part numeric
# version (for example 1.3.0.0) required by Windows VERSIONINFO resources and
# installers. Both files live at the repository root; the defaults below keep the
# project buildable when the windows/ folder is vendored standalone.
set(FUSIONPLAY_REPOSITORY_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../..")
set(FUSIONPLAY_VERSION "1.3.0")
set(FUSIONPLAY_VERSION_MAJOR "1")
set(FUSIONPLAY_VERSION_MINOR "3")
set(FUSIONPLAY_VERSION_PATCH "0")
set(FUSIONPLAY_VERSION_BUILD "0")

if(EXISTS "${FUSIONPLAY_REPOSITORY_ROOT}/VERSION")
  file(READ "${FUSIONPLAY_REPOSITORY_ROOT}/VERSION" FUSIONPLAY_VERSION_FILE)
  string(STRIP "${FUSIONPLAY_VERSION_FILE}" FUSIONPLAY_VERSION)
  if(FUSIONPLAY_VERSION STREQUAL "")
    set(FUSIONPLAY_VERSION "1.3.0")
  endif()
endif()

if(EXISTS "${FUSIONPLAY_REPOSITORY_ROOT}/INSTALLER_VERSION")
  file(READ "${FUSIONPLAY_REPOSITORY_ROOT}/INSTALLER_VERSION" FUSIONPLAY_INSTALLER_VERSION_FILE)
  string(STRIP "${FUSIONPLAY_INSTALLER_VERSION_FILE}" FUSIONPLAY_INSTALLER_VERSION)
else()
  set(FUSIONPLAY_INSTALLER_VERSION "1.3.0.0")
endif()
if(FUSIONPLAY_INSTALLER_VERSION STREQUAL "")
  set(FUSIONPLAY_INSTALLER_VERSION "1.3.0.0")
endif()

# Normalise the installer version to exactly four numeric components: Windows
# resource scripts require a 4-part version and reject anything else.
string(REPLACE "." ";" FUSIONPLAY_VERSION_PARTS "${FUSIONPLAY_INSTALLER_VERSION}")
list(LENGTH FUSIONPLAY_VERSION_PARTS FUSIONPLAY_VERSION_PART_COUNT)
foreach(FUSIONPLAY_VERSION_INDEX RANGE 0 3)
  if(FUSIONPLAY_VERSION_PART_COUNT GREATER FUSIONPLAY_VERSION_INDEX)
    list(GET FUSIONPLAY_VERSION_PARTS ${FUSIONPLAY_VERSION_INDEX} FUSIONPLAY_VERSION_PART)
    if(NOT FUSIONPLAY_VERSION_PART MATCHES "^[0-9]+$")
      set(FUSIONPLAY_VERSION_PART "0")
    endif()
  else()
    set(FUSIONPLAY_VERSION_PART "0")
  endif()
  if(FUSIONPLAY_VERSION_INDEX EQUAL 0)
    set(FUSIONPLAY_VERSION_MAJOR "${FUSIONPLAY_VERSION_PART}")
  elseif(FUSIONPLAY_VERSION_INDEX EQUAL 1)
    set(FUSIONPLAY_VERSION_MINOR "${FUSIONPLAY_VERSION_PART}")
  elseif(FUSIONPLAY_VERSION_INDEX EQUAL 2)
    set(FUSIONPLAY_VERSION_PATCH "${FUSIONPLAY_VERSION_PART}")
  else()
    set(FUSIONPLAY_VERSION_BUILD "${FUSIONPLAY_VERSION_PART}")
  endif()
endforeach()

# Re-run CMake when the version files change so the embedded version resource
# never goes stale.
if(EXISTS "${FUSIONPLAY_REPOSITORY_ROOT}/VERSION")
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${FUSIONPLAY_REPOSITORY_ROOT}/VERSION")
endif()
if(EXISTS "${FUSIONPLAY_REPOSITORY_ROOT}/INSTALLER_VERSION")
  set_property(DIRECTORY APPEND PROPERTY CMAKE_CONFIGURE_DEPENDS "${FUSIONPLAY_REPOSITORY_ROOT}/INSTALLER_VERSION")
endif()

message(STATUS "FusionPlay version: ${FUSIONPLAY_VERSION} (VERSIONINFO ${FUSIONPLAY_VERSION_MAJOR}.${FUSIONPLAY_VERSION_MINOR}.${FUSIONPLAY_VERSION_PATCH}.${FUSIONPLAY_VERSION_BUILD})")

# Explicitly opt in to modern CMake behaviors to avoid warnings with recent
# versions of CMake.
cmake_policy(VERSION 3.14...3.25)

# Define build configuration option.
get_property(IS_MULTICONFIG GLOBAL PROPERTY GENERATOR_IS_MULTI_CONFIG)
if(IS_MULTICONFIG)
  set(CMAKE_CONFIGURATION_TYPES "Debug;Profile;Release"
    CACHE STRING "" FORCE)
else()
  if(NOT CMAKE_BUILD_TYPE AND NOT CMAKE_CONFIGURATION_TYPES)
    set(CMAKE_BUILD_TYPE "Debug" CACHE
      STRING "Flutter build mode" FORCE)
    set_property(CACHE CMAKE_BUILD_TYPE PROPERTY STRINGS
      "Debug" "Profile" "Release")
  endif()
endif()
# Define settings for the Profile build mode.
set(CMAKE_EXE_LINKER_FLAGS_PROFILE "${CMAKE_EXE_LINKER_FLAGS_RELEASE}")
set(CMAKE_SHARED_LINKER_FLAGS_PROFILE "${CMAKE_SHARED_LINKER_FLAGS_RELEASE}")
set(CMAKE_C_FLAGS_PROFILE "${CMAKE_C_FLAGS_RELEASE}")
set(CMAKE_CXX_FLAGS_PROFILE "${CMAKE_CXX_FLAGS_RELEASE}")

# Use Unicode for all projects.
add_definitions(-DUNICODE -D_UNICODE)

# Compilation settings that should be applied to most targets.
#
# Be cautious about adding new options here, as plugins use this function by
# default. In most cases, you should add new options to specific targets instead
# of modifying this function.
function(APPLY_STANDARD_SETTINGS TARGET)
  target_compile_features(${TARGET} PUBLIC cxx_std_17)
  target_compile_options(${TARGET} PRIVATE /W4 /WX /wd"4100")
  target_compile_options(${TARGET} PRIVATE /EHsc)
  target_compile_definitions(${TARGET} PRIVATE "_HAS_EXCEPTIONS=0")
  target_compile_definitions(${TARGET} PRIVATE "$<$<CONFIG:Debug>:_DEBUG>")
endfunction()

# Flutter library and tool build rules.
set(FLUTTER_MANAGED_DIR "${CMAKE_CURRENT_SOURCE_DIR}/flutter")
add_subdirectory(${FLUTTER_MANAGED_DIR})

# Application build; see runner/CMakeLists.txt.
add_subdirectory("runner")


# Generated plugin build rules, which manage building the plugins and adding
# them to the application.
include(flutter/generated_plugins.cmake)


# === Installation ===
# Support files are copied into place next to the executable, so that it can
# run in place. This is done instead of making a separate bundle (as on Linux)
# so that building and running from within Visual Studio will work.
set(BUILD_BUNDLE_DIR "$<TARGET_FILE_DIR:${BINARY_NAME}>")
# Make the "install" step default, as it's required to run.
set(CMAKE_VS_INCLUDE_INSTALL_TO_DEFAULT_BUILD 1)
if(CMAKE_INSTALL_PREFIX_INITIALIZED_TO_DEFAULT)
  set(CMAKE_INSTALL_PREFIX "${BUILD_BUNDLE_DIR}" CACHE PATH "..." FORCE)
endif()

set(INSTALL_BUNDLE_DATA_DIR "${CMAKE_INSTALL_PREFIX}/data")
set(INSTALL_BUNDLE_LIB_DIR "${CMAKE_INSTALL_PREFIX}")

install(TARGETS ${BINARY_NAME} RUNTIME DESTINATION "${CMAKE_INSTALL_PREFIX}"
  COMPONENT Runtime)

install(FILES "${FLUTTER_ICU_DATA_FILE}" DESTINATION "${INSTALL_BUNDLE_DATA_DIR}"
  COMPONENT Runtime)

install(FILES "${FLUTTER_LIBRARY}" DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
  COMPONENT Runtime)

if(PLUGIN_BUNDLED_LIBRARIES)
  install(FILES "${PLUGIN_BUNDLED_LIBRARIES}"
    DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
    COMPONENT Runtime)
endif()

# Copy the native assets provided by the build.dart from all packages.
set(NATIVE_ASSETS_DIR "${PROJECT_BUILD_DIR}native_assets/windows/")
install(DIRECTORY "${NATIVE_ASSETS_DIR}"
   DESTINATION "${INSTALL_BUNDLE_LIB_DIR}"
   COMPONENT Runtime)

# Fully re-copy the assets directory on each build to avoid having stale files
# from a previous install.
set(FLUTTER_ASSET_DIR_NAME "flutter_assets")
install(CODE "
  file(REMOVE_RECURSE \"${INSTALL_BUNDLE_DATA_DIR}/${FLUTTER_ASSET_DIR_NAME}\")
  " COMPONENT Runtime)
install(DIRECTORY "${PROJECT_BUILD_DIR}/${FLUTTER_ASSET_DIR_NAME}"
  DESTINATION "${INSTALL_BUNDLE_DATA_DIR}" COMPONENT Runtime)

# Install the AOT library on non-Debug builds only.
install(FILES "${AOT_LIBRARY}" DESTINATION "${INSTALL_BUNDLE_DATA_DIR}"
  CONFIGURATIONS Profile;Release
  COMPONENT Runtime)
