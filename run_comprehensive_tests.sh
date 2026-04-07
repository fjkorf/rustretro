#!/bin/bash

# Build first
echo "Building RustRetro..."
cd ~/Playspaces/rustretro
cargo build --release 2>&1 | grep "Finished"

BINARY="./target/release/rustretro"
CORE_DIR="$HOME/Library/Application Support/RetroArch/cores"
TIMEOUT=10

# Test matrix
tests=(
    "nestopia|nes_test|$CORE_DIR/nestopia_libretro.dylib|/Users/frankkorf/games/roms/test.nes"
    "mame2003plus|sf2ce|$CORE_DIR/mame2003_plus_libretro.dylib|/Users/frankkorf/games/roms/sf2ce.zip"
    "mame2003plus|mvsc|$CORE_DIR/mame2003_plus_libretro.dylib|/Users/frankkorf/games/roms/mvsc.zip"
    "mame2003plus|mvscu|$CORE_DIR/mame2003_plus_libretro.dylib|/Users/frankkorf/games/roms/mvscu.zip"
    "mame2003plus|sf2yyc2|$CORE_DIR/mame2003_plus_libretro.dylib|/Users/frankkorf/games/roms/sf2yyc2.zip"
    "mame2003plus|asurabld|$CORE_DIR/mame2003_plus_libretro.dylib|/Users/frankkorf/games/roms/asurabld.zip"
    "mame2003|sf2ce|$CORE_DIR/mame2003_libretro.dylib|/Users/frankkorf/games/roms/sf2ce.zip"
    "mame2003|asurabld|$CORE_DIR/mame2003_libretro.dylib|/Users/frankkorf/games/roms/asurabld.zip"
)

echo ""
echo "=============================================="
echo "RustRetro Comprehensive Test Suite"
echo "=============================================="
echo "Total tests: ${#tests[@]}"
echo ""

passed=0
failed=0
crashed=0

for test_line in "${tests[@]}"; do
    IFS='|' read -r core rom core_path rom_path <<< "$test_line"
    
    echo "Testing: $core / $rom"
    echo "  Core: $(basename "$core_path")"
    echo "  ROM: $(basename "$rom_path")"
    echo -n "  Result: "
    
    timeout $TIMEOUT "$BINARY" --core "$core_path" --rom "$rom_path" --no-audio > /tmp/test_output.txt 2>&1
    exit_code=$?
    
    if [ $exit_code -eq 0 ]; then
        echo "✅ PASS"
        ((passed++))
    elif [ $exit_code -eq 139 ] || [ $exit_code -eq 138 ]; then
        echo "❌ CRASH (exit $exit_code)"
        ((crashed++))
    elif [ $exit_code -eq 124 ]; then
        echo "⏱️  TIMEOUT"
        ((failed++))
    else
        echo "❌ FAIL ($exit_code)"
        ((failed++))
    fi
    
    echo ""
done

echo "=============================================="
echo "SUMMARY"
echo "=============================================="
echo "✅ Passed:  $passed"
echo "⏱️  Timeout: $failed"
echo "❌ Crashed: $crashed"
echo "---"
echo "Total:     $((passed + failed + crashed))"
echo ""
