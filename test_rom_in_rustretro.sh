#!/bin/bash

if [ $# -lt 2 ]; then
    echo "Usage: $0 <core_path> <rom_path> [--no-audio]"
    echo ""
    echo "Example:"
    echo "  $0 ~/games/cores/mame2003_plus_libretro.dylib ~/games/roms/asurabld.zip --no-audio"
    exit 1
fi

CORE_PATH="$1"
ROM_PATH="$2"
EXTRA_ARGS="${3:-}"

echo "Testing ROM in RustRetro"
echo "======================="
echo "Core: $CORE_PATH"
echo "ROM: $ROM_PATH"
echo ""

if [ ! -f "$CORE_PATH" ]; then
    echo "❌ Core not found: $CORE_PATH"
    exit 1
fi

if [ ! -f "$ROM_PATH" ]; then
    echo "❌ ROM not found: $ROM_PATH"
    exit 1
fi

cargo build --release 2>&1 | grep "Finished"
echo ""
echo "Running RustRetro..."
timeout 10 ./target/release/rustretro --core "$CORE_PATH" --rom "$ROM_PATH" $EXTRA_ARGS 2>&1

EXIT_CODE=$?

echo ""
if [ $EXIT_CODE -eq 0 ]; then
    echo "✅ SUCCESS - ROM loaded successfully"
elif [ $EXIT_CODE -eq 139 ] || [ $EXIT_CODE -eq 138 ]; then
    echo "❌ FAILED - Segmentation fault or bus error in core"
else
    echo "❌ FAILED - Exit code: $EXIT_CODE"
fi

exit $EXIT_CODE
