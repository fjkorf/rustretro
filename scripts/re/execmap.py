#!/usr/bin/env python3
"""Execution-loop mapping toolkit for Asura Blade (68EC020, fbalpha2012).

The frontend samples PC + D/A registers once per frame (at frame end, when the
core's cycle budget runs out — a quasi-random point in whatever code dominates
the frame, unless the game parks in a wait loop, where sampling concentrates).
Polling get_state across many frames therefore gives a cycle-weighted profile
of each game state's execution. Disassembly comes from the de-interleaved
program ROM files on disk (ROM_LOAD32_BYTE: pgm3.u1=byte0 .. pgm0.u4=byte3).

Subcommands:
  sample <label> <seconds>       poll get_state, save samples to <label>.samples.json
  report <label> [top]           PC histogram, clustered into loop candidates
  disasm <hexaddr> <len> [hexpc] disassemble ROM at addr (mark pc if given)
"""
import json
import os
import struct
import sys
import time

import capstone

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                 "..", "..", "shadow", "train"))
from shadow_train.mcpclient import McpClient  # noqa: E402

SCRATCH = "."
ROMDIR = "/Users/frankkorf/games/roms/asurabld"
PORT = 4011


# ---------------------------------------------------------------- MCP client
class Mcp(McpClient):
    """execmap's Mcp -- constructed by port, eager-handshakes like before."""

    def __init__(self, port=PORT):
        super().__init__(f"http://127.0.0.1:{port}/mcp", timeout=30.0,
                          client_name="execmap")
        self.connect()


def rom_bytes():
    parts = [open(f"{ROMDIR}/{n}", "rb").read()
             for n in ("pgm3.u1", "pgm2.u2", "pgm1.u3", "pgm0.u4")]
    rom = bytearray(len(parts[0]) * 4)
    for i, p in enumerate(parts):
        rom[i::4] = p
    return bytes(rom)


def disassemble(rom, addr, length, mark_pc=None):
    md = capstone.Cs(capstone.CS_ARCH_M68K, capstone.CS_MODE_M68K_020)
    out = []
    for ins in md.disasm(rom[addr:addr + length], addr):
        mark = ">>" if mark_pc is not None and ins.address == mark_pc else "  "
        out.append(f"{mark} {ins.address:06X}: {ins.bytes.hex():<20} {ins.mnemonic:<10} {ins.op_str}")
    return out


# ---------------------------------------------------------------- subcommands
def cmd_sample(label, seconds):
    mcp = Mcp()
    samples = []
    last_frame = -1
    deadline = time.time() + float(seconds)
    while time.time() < deadline:
        st = mcp.call("get_state")
        if st["frame_count"] != last_frame:
            last_frame = st["frame_count"]
            samples.append({"f": st["frame_count"], "pc": st["m68k"]["pc"],
                            "d": st["m68k"]["d"], "a": st["m68k"]["a"],
                            "sr": st["m68k"]["sr"]})
        time.sleep(0.004)
    path = f"{SCRATCH}/{label}.samples.json"
    json.dump(samples, open(path, "w"))
    pcs = {s["pc"] for s in samples}
    print(f"{label}: {len(samples)} frame samples, {len(pcs)} distinct PCs -> {path}")


def cluster(pcs, gap=64):
    """Group sorted PCs into address clusters separated by > gap bytes."""
    clusters = []
    for pc in sorted(pcs):
        if clusters and pc - clusters[-1][-1] <= gap:
            clusters[-1].append(pc)
        else:
            clusters.append([pc])
    return clusters


def cmd_report(label, top=10):
    samples = json.load(open(f"{SCRATCH}/{label}.samples.json"))
    from collections import Counter
    hist = Counter(s["pc"] for s in samples)
    n = len(samples)
    print(f"{label}: {n} samples, {len(hist)} distinct PCs")
    clusters = cluster(hist.keys())
    scored = []
    for cl in clusters:
        weight = sum(hist[pc] for pc in cl)
        scored.append((weight, cl))
    scored.sort(key=lambda t: -t[0])
    for weight, cl in scored[:int(top)]:
        pcs_s = " ".join(f"{pc:06X}({hist[pc]})" for pc in cl[:8])
        more = "" if len(cl) <= 8 else f" +{len(cl)-8} more"
        print(f"  {100.0*weight/n:5.1f}%  {cl[0]:06X}-{cl[-1]:06X}  {pcs_s}{more}")


def cmd_disasm(addr, length, mark=None):
    rom = rom_bytes()
    for line in disassemble(rom, int(addr, 16), int(length),
                            int(mark, 16) if mark else None):
        print(line)




# ---------------------------------------------------------------- backtrace
def is_call_return(rom, addr):
    """True if the two/three words before `addr` decode as jsr/bsr."""
    if addr < 8 or addr >= len(rom):
        return False
    b = rom[addr - 6:addr]
    if len(b) == 6 and b[0] == 0x4E and b[1] == 0xB9:      # jsr abs.l
        return True
    if b[2] == 0x4E and b[3] == 0xB8:                       # jsr abs.w
        return True
    if b[2] == 0x61 and b[3] == 0x00:                       # bsr.w
        return True
    if b[4] == 0x4E and (b[5] & 0xC0) == 0x90:              # jsr (An) family
        return True
    if b[4] == 0x61:                                        # bsr.b
        return True
    return False


def backtrace(rom, mcp, a7, depth_bytes=96):
    """Stacked exception PC + heuristic call-stack from the work-RAM snapshot."""
    raw = mcp.call("read_memory", addr=a7, len=8 + depth_bytes)
    stack = bytes.fromhex(raw["hex"].replace(" ", ""))
    sr = int.from_bytes(stack[0:2], "big")
    ret_pc = int.from_bytes(stack[2:6], "big")
    fmt = int.from_bytes(stack[6:8], "big")
    chain = []
    for off in range(8, len(stack) - 3, 2):
        v = int.from_bytes(stack[off:off + 4], "big")
        if 0x100 <= v < 0x200000 and v % 2 == 0 and is_call_return(rom, v):
            chain.append((a7 + off, v))
    return sr, ret_pc, fmt & 0xFFF, chain


def cmd_bt(label="bt", seconds="3"):
    rom = rom_bytes()
    mcp = Mcp()
    from collections import Counter
    rets = Counter()
    chains = Counter()
    last = -1
    deadline = time.time() + float(seconds)
    while time.time() < deadline:
        st = mcp.call("get_state")
        if st["frame_count"] == last:
            time.sleep(0.004)
            continue
        last = st["frame_count"]
        a7 = st["m68k"]["a"][7]
        sr, ret_pc, vec, chain = backtrace(rom, mcp, a7)
        rets[ret_pc] += 1
        chains[tuple(v for _, v in chain[:6])] += 1
    print(f"{label}: {sum(rets.values())} samples")
    for pc, n in rets.most_common(8):
        print(f"  interrupted PC {pc:06X}  x{n}")
    print("  call chains (return addrs, innermost first):")
    for ch, n in chains.most_common(8):
        print(f"    x{n:<4} " + " <- ".join(f"{v:06X}" for v in ch))


if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "sample":
        cmd_sample(sys.argv[2], sys.argv[3])
    elif cmd == "report":
        cmd_report(*sys.argv[2:])
    elif cmd == "disasm":
        cmd_disasm(*sys.argv[2:])
    elif cmd == "bt":
        cmd_bt(*sys.argv[2:])
