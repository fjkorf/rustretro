# Debug Verification - Quick Start Guide

**Status:** Ready to execute  
**Time Required:** 1.5 hours  
**Goal:** Take screenshots at strategic frames to diagnose memory region issue

---

## Quick Setup

### Step 0: Build Latest
```bash
cd /Users/frankkorf/Playspaces/rustretro
cargo build --release
```

### Step 1: Prepare Core & ROM
Have ready:
- Libretro core (.so file)
- Game ROM file
- Know the full paths

Example:
```bash
CORE=/path/to/mycore_libretro.so
ROM=/path/to/game.rom
```

---

## Test Matrix (5 Tests)

### TEST 1: Initial Boot (5 min)
```bash
./target/release/rustretro --core $CORE --rom $ROM --debug
```
✅ Immediately take screenshot of Disasm tab  
✅ Note: Memory regions list, PC value, error message

### TEST 2: Gameplay Frames (10 min)
```bash
# Same command as TEST 1, but now run game
- Let game run
- Press Space to pause at frame 10
- Take screenshot (note frame number in UI)
- Press Space to resume
- Repeat at frames 20, 30
```
✅ Take 3 screenshots at different frames  
✅ Note: PC value at each frame

### TEST 3: CPU Tab (5 min)
```bash
# Same game running
- Click "🔧 CPU" tab
- Take screenshot
- Note: M68K PC value
- Compare with Disasm tab value (should match)
```
✅ Verify CPU state matches Disasm tab

### TEST 4: Hex Dump (5 min)
```bash
# Same game running
- Note PC value from Disasm (e.g., 0x02010E)
- Click "📋 Hex" tab
- Enter that PC address
- Take screenshot
```
✅ Verify address shows non-zero bytes  
✅ Proves address IS accessible

### TEST 5: Different Core (10 min)
```bash
# Stop current game, restart with fbalpha2012
CORE=/path/to/fbalpha2012_libretro.so
./target/release/rustretro --core $CORE --rom $ROM --debug

# Repeat TEST 1 + TEST 2 with this core
```
✅ Check if fbalpha2012 shows regions  
✅ Compare behavior

---

## Screenshot Checklist

For EACH screenshot, verify:
- [ ] Tab bar is visible (which tab selected)
- [ ] Frame counter visible (bottom right of window)
- [ ] PC value visible and readable
- [ ] Error messages fully shown
- [ ] Memory regions list visible (scroll if needed)
- [ ] File name includes: test number, frame, core name
  - Example: `screenshot_TEST2_FRAME20_mycore.png`

---

## Critical Questions to Answer

After all screenshots, answer these:

1. **Are memory regions ever set?**
   - Look at TEST 1 screenshot
   - Does it say "(No memory regions set)"?
   - YES = core doesn't call SET_MEMORY_MAPS
   - NO = regions are set, different issue

2. **Does PC fall within any region?**
   - Example PC: 0x02010E
   - Example regions: ROM 0x000000-0x0FFFFF, RAM 0x100000-0x10FFFF
   - Calculate: Is 0x02010E between 0x000000-0x0FFFFF? YES!
   - So why error? Bug in RustRetro's bounds check

3. **Is address actually readable?**
   - Look at TEST 4 (Hex dump)
   - Does it show non-zero bytes?
   - YES = address accessible, just not mapped
   - NO = address unmapped or invalid

4. **Is issue core-specific?**
   - Compare TEST 1 results: mycore vs fbalpha2012
   - mycore: No regions, fbalpha: Has regions?
   - YES = core-specific issue
   - NO = RustRetro-wide issue

5. **What's the root cause?**
   - Combine answers above
   - Narrow to: Core doesn't set regions, or RustRetro bug, or unmapped address

---

## Expected Outcomes

### Outcome A: Core Doesn't Set Regions
- TEST 1: "(No memory regions set)"
- TEST 5: fbalpha shows regions
- **Cause:** Your core missing callback
- **Solution:** Manual config or auto-detection

### Outcome B: Regions Set But PC Outside  
- TEST 1: Shows regions
- PC manually calculated outside all ranges
- **Cause:** Core bug or RustRetro bug
- **Solution:** Depends on investigation

### Outcome C: Address Exists But Unmapped
- TEST 1: No regions listed
- TEST 4: Hex shows code bytes at address
- **Cause:** Memory region list incomplete
- **Solution:** Workaround for sparse regions

### Outcome D: fbalpha Works, Your Core Fails
- TEST 5: fbalpha has regions & disassembly works
- Your core: No regions
- **Cause:** 100% core-specific
- **Solution:** Build auto-detection for your core

---

## File Organization

Create folder:
```bash
mkdir -p debug_verification_screenshots
cd debug_verification_screenshots
```

Save screenshots with names:
```
screenshot_TEST1_FRAME0_mycore.png
screenshot_TEST2_FRAME10_mycore.png
screenshot_TEST2_FRAME20_mycore.png
screenshot_TEST2_FRAME30_mycore.png
screenshot_TEST3_CPU_FRAME15_mycore.png
screenshot_TEST3_DISASM_FRAME15_mycore.png
screenshot_TEST4_HEX_FRAME15_mycore.png
screenshot_TEST5_FRAME0_fbalpha2012.png
```

---

## Analysis Template

Create `analysis.md`:
```markdown
# Debug Verification Analysis

## Test Results

| Test | Frame | Core | PC | Regions? | Error | Notes |
|------|-------|------|----|----|---|---|
| 1 | 0 | mycore | 0x001000 | NO | YES | No regions set |
| 2 | 10 | mycore | 0x001234 | NO | YES | PC changed |
| 2 | 20 | mycore | 0x001456 | NO | YES | PC changed again |
| 4 | - | mycore | 0x001000 | - | - | Hex shows: 48 E7 FF FE |
| 5 | 0 | fbalpha | 0x000100 | YES | NO | Works! Regions: 3 |

## Critical Answers

1. Are regions ever set? **NO** (TEST 1: no regions)
2. Does PC fall within regions? **N/A** (no regions to check)
3. Is address readable? **YES** (TEST 4: Hex shows code)
4. Is issue core-specific? **YES** (TEST 5: fbalpha works)
5. Root cause? **Core doesn't call SET_MEMORY_MAPS callback**

## Conclusion

The issue is **100% core-specific**. Your core does not implement the 
SET_MEMORY_MAPS callback (optional in libretro spec). This means:
- ✅ RustRetro code is correct
- ✅ Address translation works
- ❌ Core doesn't provide region info

## Recommended Solution

Option 1: Use fbalpha2012 or another core that supports regions
Option 2: Contact your core maintainer to add SET_MEMORY_MAPS
Option 3: We implement manual --cpu-regions config flag
```

---

## Keyboard Shortcuts in RustRetro

- **Space:** Pause/Resume emulation
- **F12:** Toggle debug window
- **Tab:** Switch between different parts of UI

---

## Common Issues & Fixes

**Issue:** Game doesn't load/crashes
- Check: Does core/ROM exist?
- Fix: Verify full paths are correct

**Issue:** Debug window doesn't open
- Check: Pass `--debug` flag
- Fix: Restart with `--debug`

**Issue:** Screenshots are blurry
- Check: Use native screenshot tool (Cmd+Shift+5 on macOS)
- Fix: Avoid third-party screenshot apps

**Issue:** PC always 0x000000
- Check: CPU state tab (is it updating?)
- Fix: Game might not be running, check window

---

## Success Criteria

After 1.5 hours, you should have:
- ✅ 8 organized screenshots
- ✅ Analysis table completed
- ✅ 5 critical questions answered
- ✅ Root cause identified
- ✅ Recommended solution clear

If you have all of above → **we can implement the fix**

---

## Next Steps

1. **Execute tests** (today, 1.5 hours)
2. **Take screenshots** (organized in folder)
3. **Fill analysis table** (compare results)
4. **Answer critical questions** (what did you learn?)
5. **Share findings** (we implement solution)

---

## Estimated Timeline

| Phase | Time | Tasks |
|-------|------|-------|
| Setup | 5 min | Build, prepare core/ROM |
| Tests 1-4 | 25 min | Capture 7 screenshots |
| Test 5 | 10 min | Test alternate core |
| Analysis | 15 min | Fill table, answer questions |
| **Total** | **~55 min** | **Ready for solution** |

---

**Ready? Start with TEST 1 above!**

