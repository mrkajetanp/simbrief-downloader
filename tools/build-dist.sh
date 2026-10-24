#!/bin/sh
# Builds the plugin for X-Plane on Linux and lays it out as X-Plane expects:
#
#   dist/SimBriefDownloader/lin_x64/SimBriefDownloader.xpl
#
# Copy dist/SimBriefDownloader into <X-Plane>/Resources/plugins/.
#
# On hosts other than x86_64 Linux (or with USE_ZIG=1) the build cross-compiles with Zig,
# targeting glibc 2.28. Set ZIG if zig is not on PATH, e.g. ZIG="python -m ziglang".
set -eu

cd "$(dirname "$0")/.."
target=x86_64-unknown-linux-gnu

if [ "$(uname -s)-$(uname -m)" != "Linux-x86_64" ] || [ "${USE_ZIG:-0}" = 1 ]; then
    export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER="$PWD/tools/zigcc"
    export CC_x86_64_unknown_linux_gnu="$PWD/tools/zigcc"
    export AR_x86_64_unknown_linux_gnu="$PWD/tools/zigar"
fi

cargo build --release -p simbrief-plugin --target "$target"

out=dist/SimBriefDownloader/lin_x64
mkdir -p "$out"
cp "target/$target/release/libsimbrief_plugin.so" "$out/SimBriefDownloader.xpl"
echo "Built $out/SimBriefDownloader.xpl"
