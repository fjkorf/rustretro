# RustRetro Comprehensive Test Plan & Results

## Executive Summary

**All 8 test combinations crash with Segmentation Fault (exit code 139)**

This indicates a **systemic issue in RustRetro's libretro integration** rather than ROM or core-specific problems.

## Test Environment

- **Binary**: `./target/release/rustretro`
- **Build Status**: ✅ Successful
- **Test Timeout**: 10 seconds per test
- **Flags**: `--no-audio` (to eliminate audio subsystem as variable)

## Available Resources

### Cores (5 total)
1. **nestopia** - NES emulator (console)
2. **bsnes** - SNES emulator (console)
3. **mame2003_plus** - Arcade (MAME 0.78)
4. **mame2003** - Arcade (MAME 0.78)
5. **mame_current** - Arcade (Latest MAME)

### ROMs (6 total)
1. **test.nes** - 64 KB NES ROM (generic test ROM)
2. **sf2ce.zip** - 3.4 MB Street Fighter II Champion Edition (Arcade)
3. **mvsc.zip** - 22 MB Marvel vs Capcom (Arcade)
4. **mvscu.zip** - 859 KB Marvel vs Capcom USA (Arcade)
5. **sf2yyc2.zip** - 3.7 MB Street Fighter II Turbo (Arcade)
6. **asurabld.zip** - 17 MB Asura Blade (Arcade)

## Test Matrix

| Test ID | Core | ROM | Result | Exit Code | Notes |
|---------|------|-----|--------|-----------|-------|
| 1 | Nestopia | test.nes | ❌ CRASH | 139 | Generic NES ROM |
| 2 | MAME 2003-Plus | sf2ce.zip | ❌ CRASH | 139 | Street Fighter II |
| 3 | MAME 2003-Plus | mvsc.zip | ❌ CRASH | 139 | Marvel vs Capcom |
| 4 | MAME 2003-Plus | mvscu.zip | ❌ CRASH | 139 | MvC USA version |
| 5 | MAME 2003-Plus | sf2yyc2.zip | ❌ CRASH | 139 | SF II Turbo |
| 6 | MAME 2003-Plus | asurabld.zip | ❌ CRASH | 139 | Asura Blade |
| 7 | MAME 2003 | sf2ce.zip | ❌ CRASH | 139 | Street Fighter II |
| 8 | MAME 2003 | asurabld.zip | ❌ CRASH | 139 | Asura Blade |

**Summary**: 0/8 passed (0%), 8/8 crashed (100%)

## Failure Pattern Analysis

### Pattern Observations
1. **All crashes exit with 139** (Segmentation Fault)
2. **Crashes occur during load_game()** (consistent with previous investigation)
3. **Pattern independent of**:
   - Core type (console or arcade)
   - ROM size
   - ROM format (NES or ZIP)
   - ROM complexity

### What Works ✅
- ✅ Core loading (get_system_info succeeds)
- ✅ Core initialization (init() succeeds)
- ✅ Callback registration
- ✅ Environment callback handling
- ✅ Application starts and validates ROM file

### What Fails ❌
- ❌ load_game() call in ALL cores
- ❌ Consistent crash point suggests shared FFI issue
- ❌ Not ROM or core specific (crashes with ALL combinations)

## Root Cause Hypotheses

### Hypothesis 1: RetroGameInfoC Structure Alignment (HIGH CONFIDENCE)
**Evidence**:
- All cores crash at same point (load_game)
- Crash happens before callbacks during load_game
- Problem appears during struct passing, not callback execution

**Symptoms**:
- Wrong struct layout → C code reads garbage
- Wrong padding → offset calculations fail
- Wrong field order → pointer arithmetic fails

**Test Plan**:
- Print struct offsets and sizes
- Compare with libretro.h specification
- Verify #[repr(C)] alignment

### Hypothesis 2: Pointer Lifetime Issue (MEDIUM CONFIDENCE)
**Evidence**:
- We leak strings and data to keep them alive
- Complex pointer management in load_game()
- Box::leak() may have issues

**Symptoms**:
- Core dereferences pointer after it's freed
- Stack corruption from improper pointer handling
- Memory layout changed between builds

**Test Plan**:
- Simplify pointer handling
- Use static strings instead of Box::leak()
- Ensure all pointers remain valid

### Hypothesis 3: Missing/Wrong Callback Setup (LOW CONFIDENCE)
**Evidence**:
- Callbacks ARE being called successfully during init
- Return values from callbacks are correct
- Callback context is properly stored

**Against**:
- Crash happens before callbacks during load_game
- Same callbacks work in RetroArch
- We've added comprehensive environment handlers

**Test Plan**:
- Log all callback invocations during load_game
- Compare callback sequence with RetroArch

### Hypothesis 4: SDL/Graphics Initialization Issue (LOW CONFIDENCE)
**Evidence**:
- Graphics window created successfully
- --no-audio flag doesn't help
- Issue exists even with minimal setup

**Test Plan**:
- Test without creating SDL window
- Test without Graphics initialization

## Recommended Next Steps

### Priority 1: Verify RetroGameInfoC Structure
```rust
// Inspect actual struct layout
eprintln!("RetroGameInfoC layout:");
eprintln!("  sizeof = {}", std::mem::size_of::<RetroGameInfoC>());
eprintln!("  alignof = {}", std::mem::align_of::<RetroGameInfoC>());
eprintln!("  offset(path) = {}", offset_of!(RetroGameInfoC, path));
eprintln!("  offset(data) = {}", offset_of!(RetroGameInfoC, data));
eprintln!("  offset(size) = {}", offset_of!(RetroGameInfoC, size));
eprintln!("  offset(meta) = {}", offset_of!(RetroGameInfoC, meta));
```

Use the `memoffset` crate to compute offsets.

### Priority 2: Simplify Pointer Management
Replace Box::leak() pattern with:
- Global ONCE_CELL or lazy_static storage
- Or pre-allocate fixed-size buffers
- Or return success/failure without complex allocations

### Priority 3: Add Trace Logging
Add detailed logging before load_game():
- Print struct pointer address
- Print all field values before calling
- Add eprintln! right after func() call to see if it returns

### Priority 4: Verify Against RetroArch
- Compare calling sequence with RetroArch via strace
- Use dtrace to trace malloc/free patterns
- Verify callback invocation timing

## Test Infrastructure

### Scripts Created
1. `run_comprehensive_tests.sh` - Automated test runner
2. SQL test matrix - Tracks all test combinations
3. This document - Test plan and results

### How to Run Tests
```bash
cd ~/Playspaces/rustretro
./run_comprehensive_tests.sh
```

### How to Add New Tests
```sql
INSERT INTO test_matrix (id, core_id, rom_id)
VALUES ('new_test', 'core_id', 'rom_id');
```

## Conclusion

The evidence points to a **fundamental FFI issue in RustRetro's libretro integration**. The fact that ALL cores crash at the same point (load_game) suggests a problem with how we're constructing or passing the RetroGameInfoC struct, not a problem with specific cores or ROMs.

**This is fixable** - the structure and approach are sound, but there's likely a bug in the FFI layer that needs to be debugged systematically.

---

**Last Updated**: Apr 7, 2026
**Test Status**: All 8/8 tests failing
**Confidence**: HIGH that problem is in FFI layer
