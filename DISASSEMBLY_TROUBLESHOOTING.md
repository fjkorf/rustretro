# Disassembly Panel Troubleshooting Guide

## Error: "PC outside all memory regions"

If you see this message in the Disasm tab, it means the disassembly panel cannot find which memory region contains the current PC.

### Causes

#### 1. **Memory Regions Not Set** (Most Common)
The libretro core hasn't called `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` callback.

**Check:** Look in the panel - if it says "(No memory regions set)", this is the issue.

**Solution:**
- Not all cores implement SET_MEMORY_MAPS callback
- This is optional in libretro spec
- Workaround: Use CPU tab to verify PC is updating correctly
- Some cores may need to call it at specific time (after load_game, not before)

#### 2. **PC in Unmapped Address Space**
The core has set memory regions, but the PC is in an address that's not in any region.

**Check:** Look at the listed memory regions. Is the current PC outside all of them?

**Solutions:**
- Core may have unmapped address space (mirrors, gaps, etc.)
- PC might be executing from an address we don't know about
- This is a core-specific issue

#### 3. **Wrong CPU Selected**
You're looking at M68K disassembly but the core is using Z80 (or vice versa).

**Check:** Look at the Z80 PC line. If Z80 is active and M68K PC is 0x020010E, the Z80 might be the active CPU.

**Solution:**
- Multi-CPU systems need to show correct CPU disassembly
- Future enhancement: Auto-detect or user-select which CPU to disassemble

#### 4. **Memory Region Address Bug**
The region start/end values might be wrong (core or RustRetro bug).

**Check:** Do the listed regions look reasonable? (e.g., ROM should be 0x000000-0x0FFFFF, RAM should be larger range)

**Solution:**
- Report to core maintainers if regions look wrong
- Check libretro spec for correct format

---

## Debugging Steps

### Step 1: Check Available Regions
The panel now shows all available memory regions when disassembly fails:

```
Available Memory Regions:
  ROM: 0x000000—0x0FFFFF (ROM)
  RAM: 0x100000—0x10FFFF (RAM)
  VRAM: 0x110000—0x111FFF (VRAM)
```

**What to look for:**
- Are there ANY regions listed?
- Do they cover reasonable memory ranges?
- Does the PC value fit in any of them?

### Step 2: Check CPU PC Values
- **M68K PC:** 6-digit hex (e.g., 0x001234)
- **Z80 PC:** 4-digit hex (e.g., 0x1234) - only shows if > 0

**What to look for:**
- Is M68K PC changing as game runs?
- If Z80 PC is active, maybe that's where code is executing

### Step 3: Check CPU State Tab
Switch to the **🔧 CPU** tab to verify:
- M68K PC is updating (changing values each frame)
- Z80 PC if it's a multi-CPU game
- Registers are changing (not frozen)

If CPU state is frozen or all zeros, that's a different issue (CPU debug symbols not found).

### Step 4: Check Hex Dump
Switch to **📋 Hex** tab:
- Navigate to the PC address
- You should see machine code bytes
- If you see zeros or no data, that confirms PC is in unmapped space

---

## Common Scenarios

### Scenario 1: "No memory regions set"
**Cause:** Core doesn't implement SET_MEMORY_MAPS callback  
**Fix:** Contact core maintainer, or use CPU/Hex tabs instead  
**Workaround:** None (need core support)

### Scenario 2: PC listed but outside regions
Example: PC = 0x02010E but regions are 0x000000-0x0FFFFF (ROM) and 0x100000-0x10FFFF (RAM)

**Cause:** Core might have unmapped address space or memory mapped I/O  
**Fix:** Contact core maintainer with the PC value  
**Workaround:** Disassembly not available for this region (limitation)

### Scenario 3: Z80 PC is active, M68K showing error
**Cause:** Game uses Z80 for audio, M68K was idle  
**Fix:** Future enhancement to auto-detect active CPU or allow user selection  
**Workaround:** Wait for frame where M68K is active, or check Z80 PC

### Scenario 4: Regions shown but still "outside"
Example: PC = 0x001234, ROM region = 0x000000-0x0FFFFF, but still error

**Cause:** Bug in address translation formula or region bounds check  
**Fix:** Please report with screenshot to developer  
**Workaround:** Use Hex tab to manually verify address exists

---

## How to Report Issues

If you encounter persistent "PC outside" errors:

1. Take screenshot of the Disasm panel showing:
   - The PC value
   - The list of available memory regions
   - Whether any regions are shown

2. Include:
   - Core name (e.g., fbalpha2012)
   - ROM filename
   - Screenshot of CPU tab (showing PC is updating)
   - Screenshot of Hex tab at that PC address

3. Report to: [RustRetro GitHub issues]

---

## Future Improvements

Planned enhancements to disassembly panel:

- **Auto-detect active CPU** - Show M68K when M68K PC active, Z80 when Z80 active
- **Z80 disassembly** - When Capstone adds Z80 support or we add separate decoder
- **Breakpoint system** (Path B) - Set breakpoints at addresses in mapped regions
- **Step controls** (Path B) - Step over/into to follow code execution
- **Memory dump** - Show raw bytes at current PC alongside disassembly

---

## Technical Details

### Address Translation Formula

For each memory region, the formula to convert emulated address to host pointer is:

```
host_ptr = region.ptr + region.offset + ((emu_addr & ~region.disconnect) - region.addr_start)
```

The panel validates:
1. `emu_addr >= region.addr_start`
2. `emu_addr <= region.addr_end`

If both checks pass, address is valid for that region. If no region contains the address, we get "PC outside all memory regions".

### Why This Matters

- **Safety:** Bounds checking prevents dereferencing invalid pointers
- **Accuracy:** Correct translation ensures disassembly matches actual code
- **Debugging:** Error message helps identify core issues or unsupported features

### Memory Region Sources

Memory regions come from libretro core via `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` callback:

- Populated once at game start (usually in load_game)
- Typically includes: ROM, System RAM, VRAM, SRAM
- May include: I/O space, mirror regions, etc.
- Not all cores implement this callback (optional in libretro spec)

