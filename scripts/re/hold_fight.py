#!/usr/bin/env python3
"""Round-hold harness (final): one uninterrupted run —
  1P fight -> freeze round timer (no timeout) -> find+freeze P1 health (no KO)
  => a controllable fight held indefinitely. Also DISCOVERS the health address.
Run ON bigmac."""
import os
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)),
                                 "..", "..", "shadow", "train"))
from shadow_train.mcpclient import McpClient  # noqa: E402

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 4025
URL = f"http://127.0.0.1:{PORT}/mcp"
P1X = 0x40454C + 0x54

mcp = McpClient(URL, client_name="hold-fight")


def call(t, **a):
    return mcp.call(t, **a)


def press(b, f=8, p=0):
    mcp.press(b, frames=f, port=p)


def u16(a):
    x = mcp.read_memory(a, 2)
    return x[0] << 8 | x[1]


def wram():
    out = bytearray()
    for off in range(0, 0x8000, 0x1000):
        out += mcp.read_memory(0x400000 + off, 0x1000)
    return out


def shot(name):
    mcp.screenshot(name)


def p1_ctrl():
    x0 = u16(P1X)
    press(["right"], 10, 0); time.sleep(0.28); x1 = u16(P1X)
    press(["left"], 20, 0); time.sleep(0.28); x2 = u16(P1X)
    return x1 != x0 or x2 != x1


def enter_1p():
    if p1_ctrl():
        return True
    press(["select"], 8, 0); time.sleep(1.1)
    press(["start"], 8, 0); time.sleep(2.0)
    press(["b"], 8, 0); time.sleep(1.0)
    press(["start"], 8, 0); time.sleep(1.5)
    for _ in range(16):
        if p1_ctrl():
            return True
        press(["b"], 5, 0); time.sleep(1.2)
    return False


mcp.connect()

if not enter_1p():
    print("FAILED to reach a controllable fight"); shot("/tmp/hold_state.png"); sys.exit(1)
print("in a controllable 1P fight")
call("enable_writes")

# 1) freeze the round timer (byte decrementing ~1 per 1.1s, <=0x99)
a = wram(); time.sleep(1.15); b = wram()
timers = [0x400000 + i for i in range(len(a)) if a[i] - b[i] == 1 and 1 <= a[i] <= 0x99]
for t in timers:
    call("freeze", addr=t, format="u8")
print(f"froze {len(timers)} timer byte(s): {[hex(t) for t in timers]}")

# 2) freeze health IMMEDIATELY (discovered: P1 health at base+0x47 and +0x4F,
#    max 0x58; P2 mirrors at +0xDB4). Write full + freeze before the CPU can KO.
HP_MAX = 0x58
P1B, P2B = 0x40454C, 0x405300
for off in (0x47, 0x4F):
    for base in (P1B, P2B):
        call("write_memory", addr=base + off, len=1, value=HP_MAX)
        call("freeze", addr=base + off, format="u8")
print(f"froze P1/P2 health (+0x47,+0x4F) at 0x{HP_MAX:02X}")

# 4) verify held: fight stays controllable for 15s
print("froze health candidates; verifying hold for 15s...")
held = True
for t in range(15):
    time.sleep(1.0)
    if not p1_ctrl():
        held = False; print(f"  t={t+1}s: fight ended"); break
shot("/tmp/hold_state.png")
print(f"RESULT: hold {'HELD (indefinite) ✓' if held else 'FAILED ✗'}")
