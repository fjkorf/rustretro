---
page:
  name: Cpu
  label: CPU
  default: true
  panel: window
  width: 360
---

# M68K / Z80 ::title

Live register view bound to `DebugState` every frame. ::muted

## M68K Data Registers ::title

| Reg | Value | Reg | Value |
|-----|-------|-----|-------|
| **D0** | [display](d0) | **D4** | [display](d4) |
| **D1** | [display](d1) | **D5** | [display](d5) |
| **D2** | [display](d2) | **D6** | [display](d6) |
| **D3** | [display](d3) | **D7** | [display](d7) |

## M68K Address Registers ::title

| Reg | Value | Reg | Value |
|-----|-------|-----|-------|
| **A0** | [display](a0) | **A4** | [display](a4) |
| **A1** | [display](a1) | **A5** | [display](a5) |
| **A2** | [display](a2) | **A6** | [display](a6) |
| **A3** | [display](a3) | **A7** | [display](a7) |

## Control ::title

| Reg | Value |
|-----|-------|
| **PC** | [display](pc) |
| **SR** | [display](sr) |

## Z80 ::title

| Reg | Value |
|-----|-------|
| **PC** | [display](z80_pc) |
| **BC** | [display](z80_bc) |
| **DE** | [display](z80_de) |
| **HL** | [display](z80_hl) |
