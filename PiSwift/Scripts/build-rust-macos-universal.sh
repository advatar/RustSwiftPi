#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd -- "${SCRIPT_DIR}/../.." && pwd)"

cd "${ROOT_DIR}"

echo "Building Rust static library (pi_swift_ffi) for macOS (arm64 + x86_64)..."

: "${MACOSX_DEPLOYMENT_TARGET:=13.0}"
export MACOSX_DEPLOYMENT_TARGET
echo "MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET}"

cargo build -p pi_swift_ffi --release --target aarch64-apple-darwin
cargo build -p pi_swift_ffi --release --target x86_64-apple-darwin

LIB_ARM64="${ROOT_DIR}/target/aarch64-apple-darwin/release/libpi_swift_ffi.a"
LIB_X86_64="${ROOT_DIR}/target/x86_64-apple-darwin/release/libpi_swift_ffi.a"

OUT_DIR="${ROOT_DIR}/PiSwift/Sources/PiRustFFI/lib"
OUT_LIB="${OUT_DIR}/libpi_swift_ffi.a"

mkdir -p "${OUT_DIR}"

echo "Creating universal library: ${OUT_LIB}"
lipo -create -output "${OUT_LIB}" "${LIB_ARM64}" "${LIB_X86_64}"

echo "OK"
