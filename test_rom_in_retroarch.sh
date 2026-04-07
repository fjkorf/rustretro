#!/bin/bash

# Test Asura Blade with RetroArch and MAME 2003-Plus core
# This will:
# 1. Load the ROM with the core
# 2. Run for 300 frames (5 seconds at 60fps)
# 3. Exit cleanly
# 4. Return exit code 0 if successful

ROM="/Users/frankkorf/games/roms/asurabld.zip"
CORE_PATH="$HOME/games/cores/mame2003_plus_libretro.dylib"
RETROARCH="/Applications/RetroArch.app/Contents/MacOS/RetroArch"

echo "Testing Asura Blade with RetroArch + MAME 2003-Plus"
echo "========================================================"
echo "ROM: $ROM"
echo "Core: $CORE_PATH"
echo ""

if [ ! -f "$ROM" ]; then
    echo "❌ ROM not found: $ROM"
    exit 1
fi

if [ ! -f "$CORE_PATH" ]; then
    echo "❌ Core not found: $CORE_PATH"
    exit 1
fi

if [ ! -f "$RETROARCH" ]; then
    echo "❌ RetroArch not found: $RETROARCH"
    exit 1
fi

echo "✓ All files exist, launching RetroArch..."
echo ""

# Run RetroArch with the ROM and core
# --max-frames 300 = run for 5 seconds at 60fps then exit
# -L = specify libretro core
timeout 15 "$RETROARCH" \
    -L "$CORE_PATH" \
    --max-frames=300 \
    "$ROM" 2>&1

EXIT_CODE=$?

echo ""
echo "========================================================"
if [ $EXIT_CODE -eq 0 ]; then
    echo "✅ SUCCESS - ROM loaded and ran successfully in RetroArch"
    echo "   This proves the ROM works with MAME 2003-Plus core"
    echo "   Conclusion: Problem is in RustRetro's libretro FFI, not the ROM"
    exit 0
elif [ $EXIT_CODE -eq 124 ]; then
    echo "⏱️  TIMEOUT - RetroArch didn't exit within 15 seconds"
    echo "   This suggests the ROM loaded and is running (good sign!)"
    echo "   Conclusion: Problem is likely in RustRetro's libretro FFI"
    exit 0
else
    echo "❌ FAILED - RetroArch exited with code $EXIT_CODE"
    echo "   This suggests the ROM doesn't work with this core either"
    echo "   Conclusion: Problem may be ROM incompatibility"
    exit 1
fi
