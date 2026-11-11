#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
SDK_ROOT="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Android/Sdk}}"
NDK_ROOT="${ANDROID_NDK_HOME:-$SDK_ROOT/ndk/27.2.12479018}"
if [[ ! -d "$NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64" ]]; then
    echo "Android NDK 27.2.12479018 (Linux x86_64) が必要です。ANDROID_NDK_HOMEで指定できます。" >&2
    exit 1
fi
NDK_BIN="$NDK_ROOT/toolchains/llvm/prebuilt/linux-x86_64/bin"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$NDK_BIN/aarch64-linux-android29-clang"
export CC_aarch64_linux_android="$CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER"
mkdir -p target/android-toolchain
cat > target/android-toolchain/android.toolchain.cmake <<CMAKE
set(ANDROID_ABI arm64-v8a CACHE STRING "" FORCE)
set(ANDROID_PLATFORM android-29 CACHE STRING "" FORCE)
include("$NDK_ROOT/build/cmake/android.toolchain.cmake")
CMAKE
export CMAKE_TOOLCHAIN_FILE="$PWD/target/android-toolchain/android.toolchain.cmake"
export CMAKE_GENERATOR=Ninja
export CMAKE_POLICY_VERSION_MINIMUM=3.5
export ANDROID_ABI=arm64-v8a
export ANDROID_PLATFORM=android-29
export AR_aarch64_linux_android="$NDK_BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384"
cargo build -p riscord-core --locked --release --target aarch64-linux-android
mkdir -p android/app/src/main/jniLibs/arm64-v8a
cp target/aarch64-linux-android/release/libriscord_core.so android/app/src/main/jniLibs/arm64-v8a/
