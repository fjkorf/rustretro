# Disassembly Feature Workaround: Using Hex Tab

**Status:** ✅ Ready to use  
**Cores Affected:** MAME 2003+, FBAlpha 2012, and others without SET_MEMORY_MAPS  
**Alternative:** Native disassembly tab (works with cores that support memory regions)

---

## Quick Reference

### How to View Code at Current PC

1. **Get the PC address:**
   - Press `F12` to open debug window
   - Click "🔧 CPU" tab
   - Note the **M68K PC** value (e.g., `0x02010E`)

2. **Navigate to that address in Hex:**
   - Click "📋 Hex" tab
   - In the "Address:" field, enter the PC value: `0x02010E`
   - Press Enter

3. **Examine the code:**
   - Left column: Hex bytes (machine code)
   - Right column: ASCII representation
   - Compare with your disassembly reference

---

## Example Workflow

### Scenario: Debugging Asurabld (Fighting Game)

```
1. Game running, want to see code at current PC
   
2. Press F12 to show debug window
   
3. CPU Tab shows:
   M68K PC: 0x02010E
   D0-D7 registers: [values]
   A0-A7 registers: [values]
   
4. Switch to Hex tab
   
5. Clear "Address:" field and type: 0x02010E
   
6. See raw bytes:
   02010E: 48 E7 FF FE 48 9F ...
   
7. Interpret bytes manually or use M68K instruction reference
   Example: 48 E7 FF FE = MOVEM (save all registers)
```

### Why This Works

The hex bytes shown ARE the actual machine code being executed. Unlike the disassembly tab (which needs memory region info), the hex tab works with ANY core because it just reads and displays raw memory.

---

## Reading M68K Instructions Manually

### Common M68K Patterns

```
Instruction Bytes    | Meaning
==========================================
48 E7 FF FE         | MOVEM.L  (push registers)
48 9F               | MOVEM.L  (continuation)
4E 56 XX XX         | LINK.L A6,#XXXX
4E 5E               | UNLK A6
4E 75               | RTS (return from subroutine)
60 XX               | BRA (branch always)
61 XX               | BSR (branch to subroutine)
```

### For Exact Decoding

Use an M68K instruction decoder online:
- Search: "M68K instruction decoder"
- Paste hex bytes: `48E7FFFF`
- Get: `MOVEM.L A0-A7,-(A7)`

---

## Limitations of Hex Workaround

| Feature | Disassembly Tab | Hex Tab |
|---------|---|---|
| Automatic PC tracking | ✅ YES | ❌ Manual |
| Instruction context | ✅ Shows ±10 | ❌ Raw bytes only |
| Branch targets | ✅ Auto-resolved | ❌ Manual calculation |
| Search by mnemonic | ✅ In future | ❌ NO |
| Speed | ✅ Fast | ✅ Fast |
| Works with all cores | ❌ NO | ✅ YES |

---

## Better Long-Term Solution

If you want true disassembly support, we can:

1. **Add manual config flag:**
   ```bash
   rustretro --core core.so --rom rom.zip \
     --cpu-regions "ROM:0x000000-0x0FFFFF,RAM:0x100000-0x10FFFF"
   ```
   - No code changes to core needed
   - You provide the memory layout
   - Disassembly works immediately

2. **Build auto-detection:**
   - We pre-program layouts for MAME, FBAlpha, etc.
   - Disassembly works automatically with popular cores
   - Takes ~1 week to implement

Either way, the Hex tab workaround is always available as a fallback.

---

## Troubleshooting

### Q: Hex tab shows gibberish characters
**A:** That's expected! These aren't code—they're VRAM or RAM, not instructions. Try navigating to the actual code region (usually address 0x000000 or specific ROM range for your core).

### Q: How do I know what address range has code?
**A:** For MAME 2003+ arcade:
- ROM usually: 0x000000 - 0x0FFFFF
- RAM usually: 0x100000 - 0x10FFFF
- VRAM usually: 0x200000 - varies

Check your specific arcade board documentation or try common ranges.

### Q: Can I bookmark addresses?
**A:** Not yet, but you can:
1. Note the address in a text file
2. Copy/paste it into Hex tab when needed
3. Or use the CPU tab to watch PC in real-time

### Q: Does this work with other debug cores?
**A:** Yes! The Hex tab works with ALL cores in ALL situations. It's just raw memory view.

---

## Summary

**Today:** Use Hex tab to view code (works now)  
**Soon:** Consider requesting disassembly feature with memory regions  
**Always:** Hex tab remains as reliable fallback

The game runs perfectly regardless. This workaround just lets you inspect code when debugging.

