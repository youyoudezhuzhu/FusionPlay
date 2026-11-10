cmake_minimum_required(VERSION 3.14)
project(runner LANGUAGES CXX)

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME} WIN32
  "flutter_window.cpp"
  "main.cpp"
  "utils.cpp"
  "win32_window.cpp"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
  "Runner.rc"
  "runner.exe.manifest"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# Add preprocessor definitions for the FusionPlay build version. The display
# version comes from the repository root VERSION file, the 4-part numeric
# version (used by Runner.rc) comes from INSTALLER_VERSION; both are read by the
# top-level CMakeLists.txt.
target_compile_definitions(${BINARY_NAME} PRIVATE "FUSIONPLAY_VERSION=\"${FUSIONPLAY_VERSION}\"")
target_compile_definitions(${BINARY_NAME} PRIVATE "FUSIONPLAY_VERSION_MAJOR=${FUSIONPLAY_VERSION_MAJOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FUSIONPLAY_VERSION_MINOR=${FUSIONPLAY_VERSION_MINOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FUSIONPLAY_VERSION_PATCH=${FUSIONPLAY_VERSION_PATCH}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FUSIONPLAY_VERSION_BUILD=${FUSIONPLAY_VERSION_BUILD}")

# Disable Windows macros that collide with C++ standard library functions.
target_compile_definitions(${BINARY_NAME} PRIVATE "NOMINMAX")

# Add dependency libraries and include directories. Add any application-specific
# dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter flutter_wrapper_app)
target_link_libraries(${BINARY_NAME} PRIVATE "dwmapi.lib")
target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}")

# Run the Flutter tool portions of the build. This must not be removed.
add_dependencies(${BINARY_NAME} flutter_assemble)
