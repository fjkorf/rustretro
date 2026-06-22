# RustRetro — Debugging Notes & libretro Gotchas

Hard-won lessons from bringing real cores (Nestopia, Genesis Plus GX, MAME 2003-Plus) up
against this frontend. These are the things that cost the most time; keep them here so the
next core doesn't cost the same.

## 1. libretro environment-callback constants are NOT sequential

The single biggest bug in this project's history: the FFI layer originally numbered the
`RETRO_ENVIRONMENT_*` command constants 1, 2, 3… sequentially. The real `libretro.h` values
are sparse and specific. Only `GET_SYSTEM_DIRECTORY = 9` was right by coincidence — which was
enough for the lenient Nestopia core to limp along, hiding the fact that the whole table was
wrong. MAME, which is stricter during init, crashed.

Correct values (verify any new one against `libretro.h`, never guess):

| Constant | Value |
|---|---|
| `SET_PIXEL_FORMAT` | 10 |
| `GET_VARIABLE` | 15 |
| `GET_LOG_INTERFACE` | 27 |
| `GET_SAVE_DIRECTORY` | 31 |
| `SET_SYSTEM_AV_INFO` | 32 |
| `SET_MEMORY_MAPS` | 36 |
| `GET_VFS_INTERFACE` | 65581 (`45 \| 0x10000`) |
| `RETRO_PIXEL_FORMAT_XRGB8888` | 1 (pixel-format enum, not 2) |

The `0x10000` (`RETRO_ENVIRONMENT_EXPERIMENTAL`) flag is why newer interfaces (VFS, LED) have
huge cmd values that look like bugs but aren't.

**Lesson:** a forgiving peer (Nestopia) can mask a broken protocol. Cross-check against the
authoritative header, and treat the strictest core (MAME) as your conformance test.

## 2. `GET_VFS_INTERFACE` returning `true` without a struct is a crash bomb

A core that gets `true` for VFS will immediately call function pointers in the struct you were
supposed to fill in. We return `false` so cores fall back to stdio file I/O. Only return `true`
once the interface is actually implemented.

## 3. ROM loading: `need_fullpath` decides the strategy

`RetroSystemInfo.need_fullpath` splits cores into two loading modes:

- **`need_fullpath = true`** (e.g. MAME): pass the ROM *path*; the core opens the file itself.
- **`need_fullpath = false`** (e.g. NES/Genesis): read the whole ROM into memory and pass the
  *data pointer*.

Get this wrong and `retro_load_game` fails or the core reads garbage.

## 4. Disassembly: where the code bytes come from

The Disasm panel decodes `DebugState.m68k_code_bytes` with Capstone. Those bytes are sourced,
in priority order:

1. **`SekFetchByte`** — MAME/FBAlpha export the symbol `_Z12SekFetchBytej`
   (`extern "C" fn(u32) -> u8`, a side-effect-free instruction fetch). When present, the
   frontend pulls 256 bytes at PC each frame directly from the core. This is the path that
   makes disassembly work for arcade cores.
2. **`SET_MEMORY_MAPS` regions** — if the core published a memory map, translate PC → host
   pointer via `region.ptr + offset + ((addr & ~disconnect) - addr_start)` and read there.

If a core exposes *neither* (some MAME/FBAlpha builds don't implement `SET_MEMORY_MAPS`, and
older cores lack `SekFetchByte`), the panel shows *"No code bytes — core does not expose memory
via SekFetchByte or SET_MEMORY_MAPS."* This is correct behavior, not a frontend bug: the code
is simply not reachable from the frontend. **Workaround:** read the bytes manually in the
📋 Hex tab at the PC shown in the 🔧 CPU tab.

## 5. Quiet your environment callback for MAME

MAME fires environment callbacks in a tight loop during init. Per-call `eprintln!` logging
floods stdout and slows boot to a crawl. Keep the env callback's match arms minimal and log
only the unhandled/interesting commands.
