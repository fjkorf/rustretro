# DEBUG VERIFICATION - TEST 1 RESULTS

**Date:** 2026-04-08T20:48  
**Core:** fbalpha2012_libretro.dylib  
**ROM:** asurabld.zip  
**Status:** ✅ COMPLETED

---

## Console Output Analysis

### Critical Finding: PC Address is Correct
```
[CPU] ✓ CPU state captured (M68K PC=$02010E)
[CPU] ✓ CPU state captured (M68K PC=$02010E)
```

**Observation:** The CPU state capture shows PC = 0x02010E, which is exactly the PC that was causing the "outside regions" error in our previous sessions with mame2003_plus core.

---

## Startup Sequence
1. ✅ Core loaded: "FB Alpha 2012 vv0.2.97.29 15af60b"
2. ✅ ROM parsed successfully
3. ✅ AV info retrieved: 320x240 @ 60.00 FPS, 32040 Hz audio
4. ✅ Game window created
5. ✅ CPU state captured with valid PC

---

## Key Information from Console

### Game Info
- Base resolution: 320x240
- Frame rate: 60.00 FPS
- Audio sample rate: 32040 Hz
- CPU: M68K (M68 core detected from PC=$02010E format)

### System Info
- OS: macOS 26.2
- CPU: Apple M4 with 10 cores
- Memory: 16.0 GiB
- GPU: Apple M4 Metal

---

## Memory Region Check

**Status:** Cannot determine from console - need debug window

The console output does not show memory regions information. This could mean:
1. Debug window not opened yet (F12 pressed but may not have registered)
2. Disassembly panel may show regions when opened
3. Need to proceed to debug window screenshot

---

## What We Know So Far

✅ **Game loads successfully**
- Core initializes without errors
- ROM parses correctly
- Video/audio configured
- CPU running (PC captured)

✅ **PC address (0x02010E) is real and valid**
- CPU state system captures it
- Core returns it as current PC
- Not a garbage value

❓ **Memory regions status: UNKNOWN**
- Console output silent on memory regions
- Need debug window to check RETRO_ENVIRONMENT_SET_MEMORY_MAPS

---

## Next Steps

1. **TEST 2:** Let game run, check if PC changes over frames
2. **TEST 3:** Open debug window (F12), navigate to Disasm tab
3. **TEST 4:** Check if regions are listed in disassembly panel
4. **TEST 5:** Use Hex tab to verify address 0x02010E is accessible

---

## Conclusion (Preliminary)

The fbalpha2012 core loads successfully with same ROM and same problematic PC address.
The question remains: **Does fbalpha2012 set memory regions?**

We need the debug window to answer this question definitively.

