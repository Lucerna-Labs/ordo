#!/usr/bin/env bash
# Shared build environment for the Servo desktop shell (feature `servo-engine`).
#
# Servo's `mozangle` crate generates ANGLE shader bindings with bindgen/libclang
# (mozangle build.rs ~line 377, invoked as `-x c++ -std=c++17`). On some Linux
# setups — seen on Pop!_OS 24.04 — libclang does not auto-locate the GCC libstdc++
# headers, so that step dies with:
#     gfx/angle/checkout/include/GLSLANG/ShaderLang.h: fatal error: 'array' file not found
# The ANGLE C++ itself compiles fine (it goes through gcc/g++); only the bindgen
# libclang invocation lacks the C++ stdlib search path. bindgen appends
# BINDGEN_EXTRA_CLANG_ARGS to that invocation, so we point it at the libstdc++
# headers that are actually installed.
#
# Stock Ubuntu clang does not need this, and adding the paths there is harmless
# (verified: a clean mozangle build still succeeds with them set) — so we set it
# whenever we can locate the headers, on Linux only. The Windows build never runs
# these .sh scripts, so this cannot affect it.
#
# Sourced by the Linux build scripts immediately before the `--features
# servo-engine` cargo build.

set_servo_bindgen_clang_args() {
  # Respect an operator-provided value.
  [ -n "${BINDGEN_EXTRA_CLANG_ARGS:-}" ] && return 0
  [ "$(uname -s 2>/dev/null)" = "Linux" ] || return 0

  # Find the highest-versioned libstdc++ include dir that actually contains
  # <array> (robust to boxes with several gcc versions installed, and to the
  # case where clang's own toolchain detection picks the wrong one).
  local hdr="" candidate
  for candidate in /usr/include/c++/*; do
    [ -e "$candidate/array" ] && hdr="$candidate"
  done
  [ -n "$hdr" ] || return 0
  local ver
  ver="$(basename "$hdr")"

  local args="-isystem $hdr"
  # The arch-specific dir (e.g. /usr/include/x86_64-linux-gnu/c++/13) holds
  # bits/c++config.h; match the same version.
  for candidate in /usr/include/*/c++/"$ver"; do
    [ -d "$candidate" ] && args="$args -isystem $candidate"
  done
  [ -d "$hdr/backward" ] && args="$args -isystem $hdr/backward"

  export BINDGEN_EXTRA_CLANG_ARGS="$args"
  echo "servo-build-env: BINDGEN_EXTRA_CLANG_ARGS=$BINDGEN_EXTRA_CLANG_ARGS"
}

set_servo_bindgen_clang_args
