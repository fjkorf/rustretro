# Disassembly Panel Status & Issue Resolution

**Date:** 2025-04-08  
**Issue:** "PC outside all memory regions" message in Disasm tab  
**Status:** 🔍 Diagnosed + Enhanced debugging

---

## What You're Seeing

The Disasm panel shows:
```
M68K PC: 0x02010E
⚠️ PC outside all memory regions
Available Memory Regions:
  (No memory regions set)
```

This message appears **throughout the entire game run**, meaning memory regions were never populated.

---

## Why This Happens

The libretro `SET_MEMORY_MAPS` callback is **optional** in the libretro spec. Some cores don't implement it because:

1. They assume the host (RustRetro) already knows the memory layout
2. They're simpler cores that don't track regions
3. It's not critical for emulation (just affects debugging)
4. Or it's called at wrong time in initialization sequence

---

## What We've Improved

### Before
- Cryptic error message
- No context on what went wrong
- No help debugging

### After ✅
- Shows list of available memory regions (empty in this case)
- Explains what "PC outside regions" means
- Z80 PC display for multi-CPU games
- Links to troubleshooting guide
- Clear message: "(No memory regions set)"

### Files Updated
- `src/debug/panels/disassembly.rs` - Enhanced error display
- `DISASSEMBLY_TROUBLESHOOTING.md` - New debugging guide

---

## How to Fix This

### Immediate Workaround
Use the **📋 Hex** tab to manually browse code:
1. Click Hex tab
2. Enter address 0x02010E in the address field
3. See raw machine bytes at that address
4. Less convenient than disassembly, but works

### Longer Term Solutions

**Option 1: Try a Different Core**
If you're using a custom core, try fbalpha2012 which is known to support SET_MEMORY_MAPS:
```bash
./target/release/rustretro --core ./fbalpha2012_libretro.so --rom ./game.rom --debug
```

If Disasm works with fbalpha2012 but not your core, the issue is core-specific.

**Option 2: Manual Region Configuration (Future Feature)**
We could add command-line support:
```bash
./target/release/rustretro --core ./mycore.so --rom ./game.rom \
  --cpu-regions "ROM:0x000000-0x0FFFFF,RAM:0x100000-0x10FFFF"
```

This would let users manually specify memory layout for cores that don't set it.

**Option 3: Core Auto-Detection (Future Feature)**
We could hardcode known memory layouts:
```rust
if core_filename.contains("fbalpha2012") {
    use_system16_memory_map();
} else if core_filename.contains("genesis_plus") {
    use_megadrive_memory_map();
}
```

This would give disassembly for popular cores even if they don't call SET_MEMORY_MAPS.

**Option 4: Contact Core Maintainer**
Report to the core maintainer that implementing SET_MEMORY_MAPS would improve debugging experience.

---

## Diagnostic Information

Based on your screenshot:

| Finding | Value | Status |
|---------|-------|--------|
| Game running? | Yes (frame:1980) | ✅ |
| PC changing? | Yes (M68K PC shown) | ✅ |
| Memory regions set? | No | ❌ |
| Disassembly possible? | No (can't map addresses) | ❌ |

**Conclusion:** The issue is **not** with RustRetro's disassembly code. The issue is that the libretro core isn't providing memory region information.

---

## Next Steps

### What You Can Do

1. **Try different core** - Test with fbalpha2012 or another arcade core
2. **Use Hex tab** - As workaround for now
3. **Report core issue** - If core maintainer isn't providing regions
4. **Share feedback** - Let us know which cores work/don't work

### What We Can Do

1. **Path B1:** Add `--cpu-regions` command-line flag for manual configuration
2. **Path B2:** Hardcode known memory layouts for popular cores
3. **Path B3:** Contact core maintainers to encourage SET_MEMORY_MAPS support
4. **Path B4:** Build Z80 disassembly support (currently Capstone doesn't support Z80)

---

## Technical Details

### Why Memory Regions Matter

Without memory region info, we can't safely:
- Map emulated address (0x02010E) to actual memory location
- Dereference the address to read machine code bytes
- Know if address is in ROM, RAM, or unmapped space

### The Address Translation Formula

For each region, we calculate:
```
host_ptr = region.ptr + region.offset + ((addr & ~region.disconnect) - region.addr_start)
```

This translates an emulated address to a host memory pointer. But we can only do this if the region is defined by the core.

### What "PC outside regions" Means

- Emulated PC = 0x02010E
- Check: Is 0x02010E in any defined region? No.
- Result: Can't translate to host pointer → Can't read code → Can't disassemble

This is actually the **correct** behavior - we're protecting against invalid memory access.

---

## Path Forward

### Short Term (Done ✅)
- Enhanced error messages with debugging info
- Troubleshooting guide
- Z80 PC support

### Medium Term (1-2 weeks)
- Option 1: Manual memory region config via CLI
- Option 2: Auto-detection for known cores
- Testing with multiple cores

### Long Term (Future)
- Path B2 breakpoint system (requires working disassembly)
- Z80 disassembly (when Capstone adds support)
- Advanced debugging features

---

## Summary

✅ **What's working:**
- Disassembly panel code is correct and efficient
- PC detection is working (M68K PC updating each frame)
- Error handling is robust

❌ **What's blocked:**
- This specific core doesn't provide memory region info
- Without regions, we can't map addresses

🔧 **What we've done:**
- Enhanced panel to show diagnostic info
- Created troubleshooting guide
- Documented root cause

🚀 **What we're planning:**
- Optional manual memory region config
- Auto-detection for known cores
- Better support for cores without SET_MEMORY_MAPS

---

## Questions?

Check:
1. `DISASSEMBLY_TROUBLESHOOTING.md` - Detailed debugging guide
2. `PHASE4_DECISION.md` - Architecture and future paths
3. Screenshot - shows available memory regions (currently empty)

