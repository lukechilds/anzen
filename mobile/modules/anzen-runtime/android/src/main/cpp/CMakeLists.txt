cmake_minimum_required(VERSION 3.22)
project(anzen_jni)
if(NOT DEFINED ANZEN_ANDROID_PAGE_SIZE)
  message(FATAL_ERROR "Build Anzen through its Gradle module so native-targets.json is applied.")
endif()
set(ANZEN_RUST_LIBRARY "${CMAKE_CURRENT_SOURCE_DIR}/../jniLibs/${ANDROID_ABI}/libanzen_mobile.so")
if(NOT EXISTS "${ANZEN_RUST_LIBRARY}")
  message(FATAL_ERROR "Missing Anzen Rust library for ${ANDROID_ABI}. Run npm run build:rust:android in mobile/ first.")
endif()
add_library(anzen_mobile SHARED IMPORTED)
set_target_properties(anzen_mobile PROPERTIES IMPORTED_LOCATION "${ANZEN_RUST_LIBRARY}")
add_library(anzen_jni SHARED bridge.cpp)
target_link_libraries(anzen_jni anzen_mobile)
target_link_options(anzen_jni PRIVATE
  "-Wl,-z,max-page-size=${ANZEN_ANDROID_PAGE_SIZE}"
  "-Wl,-z,common-page-size=${ANZEN_ANDROID_PAGE_SIZE}"
)
