//! Minimal v1 Lua scripting layer for RustRetro.
//!
//! Exposes a tiny, sandboxed API to community scripts whose killer use case is
//! fighting-game HITBOX OVERLAYS: read big-endian object-RAM box lists and draw
//! translucent rectangles onto the framebuffer every frame.
//!
//! ## Threading
//! `mlua::Lua` and the internal `Rc<RefCell<…>>` draw buffer are `!Send`, so
//! `LuaEngine` MUST live as a Bevy `NonSend` resource on the main thread, exactly
//! like `Emu`. It is never wrapped in `Arc<Mutex<…>>`.
//!
//! ## API surface (installed into the VM)
//! ```text
//! memory.read_u8(addr)              -> integer
//! memory.read_u16_be(addr)          -> integer
//! memory.read_u32_be(addr)          -> integer
//! memory.read_s16_be(addr)          -> integer (signed)
//! memory.read_u16_le(addr)          -> integer  (little-endian)
//! memory.read_u32_le(addr)          -> integer  (little-endian)
//! gui.drawBox(x1,y1,x2,y2, fill, line)
//! gui.drawText(x,y, str [, color])
//! gui.drawLine(x1,y1,x2,y2, color)
//! gui.drawPixel(x,y, color)
//! event.onframeend(function)
//! console.log(str)
//! emu.framecount()                  -> integer
//! _RUSTRETRO_API                    = 1  (version sentinel)
//! ```
//! Colors are packed RGBA u32: `0xRRGGBBAA`.

use std::cell::RefCell;
use std::rc::Rc;

use mlua::{Lua, LuaOptions, RegistryKey, StdLib};

use crate::debug::SharedDebugState;

/// A single frame-local drawing command pushed by a script via the `gui` table.
#[derive(Clone, Debug)]
pub enum DrawCmd {
    /// Filled + outlined rectangle. `fill`/`line` are packed `0xRRGGBBAA`.
    Box {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        fill: u32,
        line: u32,
    },
    /// Text label. `color` is packed `0xRRGGBBAA`.
    Text {
        x: i32,
        y: i32,
        s: String,
        color: u32,
    },
    /// A straight line from (x1,y1) to (x2,y2). `color` is packed `0xRRGGBBAA`.
    Line {
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        color: u32,
    },
    /// A single pixel at (x,y). `color` is packed `0xRRGGBBAA`.
    Pixel {
        x: i32,
        y: i32,
        color: u32,
    },
}

/// Shared, single-threaded draw buffer. `Rc<RefCell<…>>` is fine because the VM
/// and all its closures run only on the main thread.
type DrawBuf = Rc<RefCell<Vec<DrawCmd>>>;

/// Lock the shared debug state briefly and read one byte. Out-of-map reads
/// return 0 (mirrors how community emulator scripts behave). The `MutexGuard`
/// is dropped before this returns, keeping the borrow out of the Lua closure.
fn read1(dbg: &SharedDebugState, addr: u32) -> mlua::Result<u8> {
    let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
    let b = ds.read_u8(addr).unwrap_or(0);
    drop(ds);
    Ok(b)
}

/// The Lua scripting engine. Owns the VM, the registered `event.onframeend`
/// callbacks, and the frame-local draw-command buffer.
pub struct LuaEngine {
    lua: Lua,
    /// Registered `event.onframeend` callbacks (in registration order).
    frame_callbacks: Rc<RefCell<Vec<RegistryKey>>>,
    /// Frame-local draw commands produced by `gui.*` during callback execution.
    draw_cmds: DrawBuf,
    /// Shared debug state, kept so error reporting can log to the event log.
    debug: SharedDebugState,
}

impl LuaEngine {
    /// Create a sandboxed VM and install the API tables.
    ///
    /// Sandbox: only base/table/string/math stdlibs are loaded. `io`, `os`, and
    /// `package` are deliberately excluded so scripts cannot touch the filesystem,
    /// spawn processes, or load native modules. The HOST reads script files; Lua
    /// never does.
    pub fn new(debug: SharedDebugState) -> mlua::Result<Self> {
        // Restricted stdlib set — no io/os/package/debug.
        let libs = StdLib::TABLE | StdLib::STRING | StdLib::MATH;
        let lua = Lua::new_with(libs, LuaOptions::default())?;

        let draw_cmds: DrawBuf = Rc::new(RefCell::new(Vec::new()));
        let frame_callbacks: Rc<RefCell<Vec<RegistryKey>>> = Rc::new(RefCell::new(Vec::new()));

        let engine = LuaEngine {
            lua,
            frame_callbacks: Rc::clone(&frame_callbacks),
            draw_cmds: Rc::clone(&draw_cmds),
            debug: SharedDebugState::clone(&debug),
        };

        engine.install_api(&debug, &draw_cmds, &frame_callbacks)?;
        Ok(engine)
    }

    /// Install the `memory`, `gui`, `event`, and `console` global tables.
    fn install_api(
        &self,
        debug: &SharedDebugState,
        draw_cmds: &DrawBuf,
        frame_callbacks: &Rc<RefCell<Vec<RegistryKey>>>,
    ) -> mlua::Result<()> {
        let lua = &self.lua;
        let globals = lua.globals();

        // ── memory.* ──────────────────────────────────────────────────────────
        let memory = lua.create_table()?;

        // read_u8(addr) -> integer
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let b = read1(&dbg, addr)?;
                Ok(b as u32)
            })?;
            memory.set("read_u8", f)?;
        }

        // read_u16_be(addr) -> integer  (big-endian: byte[addr] is high byte)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let hi = read1(&dbg, addr)? as u32;
                let lo = read1(&dbg, addr.wrapping_add(1))? as u32;
                Ok((hi << 8) | lo)
            })?;
            memory.set("read_u16_be", f)?;
        }

        // read_u32_be(addr) -> integer
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let b0 = read1(&dbg, addr)? as u32;
                let b1 = read1(&dbg, addr.wrapping_add(1))? as u32;
                let b2 = read1(&dbg, addr.wrapping_add(2))? as u32;
                let b3 = read1(&dbg, addr.wrapping_add(3))? as u32;
                Ok((b0 << 24) | (b1 << 16) | (b2 << 8) | b3)
            })?;
            memory.set("read_u32_be", f)?;
        }

        // read_s16_be(addr) -> integer  (sign-extended 16-bit big-endian)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<i32> {
                let hi = read1(&dbg, addr)? as u16;
                let lo = read1(&dbg, addr.wrapping_add(1))? as u16;
                let raw = (hi << 8) | lo;
                Ok(raw as i16 as i32)
            })?;
            memory.set("read_s16_be", f)?;
        }

        // read_u16_le(addr) -> integer  (little-endian: byte[addr] is low byte)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let lo = read1(&dbg, addr)? as u32;
                let hi = read1(&dbg, addr.wrapping_add(1))? as u32;
                Ok((hi << 8) | lo)
            })?;
            memory.set("read_u16_le", f)?;
        }

        // read_u32_le(addr) -> integer  (little-endian)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, addr: u32| -> mlua::Result<u32> {
                let b0 = read1(&dbg, addr)? as u32;
                let b1 = read1(&dbg, addr.wrapping_add(1))? as u32;
                let b2 = read1(&dbg, addr.wrapping_add(2))? as u32;
                let b3 = read1(&dbg, addr.wrapping_add(3))? as u32;
                Ok(b0 | (b1 << 8) | (b2 << 16) | (b3 << 24))
            })?;
            memory.set("read_u32_le", f)?;
        }

        globals.set("memory", memory)?;

        // ── gui.* ─────────────────────────────────────────────────────────────
        let gui = lua.create_table()?;

        // drawBox(x1,y1,x2,y2, fill_rgba, line_rgba)
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x1, y1, x2, y2, fill, line): (i32, i32, i32, i32, u32, u32)| {
                    buf.borrow_mut().push(DrawCmd::Box {
                        x1,
                        y1,
                        x2,
                        y2,
                        fill,
                        line,
                    });
                    Ok(())
                },
            )?;
            gui.set("drawBox", f)?;
        }

        // drawText(x, y, str [, color])
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x, y, s, color): (i32, i32, String, Option<u32>)| {
                    buf.borrow_mut().push(DrawCmd::Text {
                        x,
                        y,
                        s,
                        // Default: opaque white.
                        color: color.unwrap_or(0xFFFF_FFFF),
                    });
                    Ok(())
                },
            )?;
            gui.set("drawText", f)?;
        }

        // drawLine(x1,y1,x2,y2, color)
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x1, y1, x2, y2, color): (i32, i32, i32, i32, u32)| {
                    buf.borrow_mut().push(DrawCmd::Line { x1, y1, x2, y2, color });
                    Ok(())
                },
            )?;
            gui.set("drawLine", f)?;
        }

        // drawPixel(x, y, color)
        {
            let buf = Rc::clone(draw_cmds);
            let f = lua.create_function(
                move |_, (x, y, color): (i32, i32, u32)| {
                    buf.borrow_mut().push(DrawCmd::Pixel { x, y, color });
                    Ok(())
                },
            )?;
            gui.set("drawPixel", f)?;
        }

        globals.set("gui", gui)?;

        // ── event.* ───────────────────────────────────────────────────────────
        let event = lua.create_table()?;

        // onframeend(function) — register a per-frame callback.
        {
            let cbs = Rc::clone(frame_callbacks);
            let f = lua.create_function(move |lua, func: mlua::Function| {
                let key = lua.create_registry_value(func)?;
                cbs.borrow_mut().push(key);
                Ok(())
            })?;
            event.set("onframeend", f)?;
        }

        globals.set("event", event)?;

        // ── console.* ─────────────────────────────────────────────────────────
        let console = lua.create_table()?;
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, msg: String| {
                if let Ok(mut ds) = dbg.lock() {
                    ds.log(format!("[lua] {msg}"));
                }
                Ok(())
            })?;
            console.set("log", f)?;
        }
        globals.set("console", console)?;

        // ── emu.* ─────────────────────────────────────────────────────────────
        let emu = lua.create_table()?;

        // framecount() -> integer  (returns DebugState.frame_count)
        {
            let dbg = SharedDebugState::clone(debug);
            let f = lua.create_function(move |_, ()| -> mlua::Result<u64> {
                let ds = dbg.lock().map_err(|e| mlua::Error::external(e.to_string()))?;
                Ok(ds.frame_count)
            })?;
            emu.set("framecount", f)?;
        }

        globals.set("emu", emu)?;

        // ── version sentinel ──────────────────────────────────────────────────
        // Scripts can check `if _RUSTRETRO_API >= 1 then … end` to feature-detect.
        globals.set("_RUSTRETRO_API", 1u32)?;

        Ok(())
    }

    /// Load and execute a script. `src_or_path` may be either inline Lua source or
    /// a filesystem path; if it points to an existing file the HOST reads it (Lua
    /// has no io access). Running the chunk typically calls `event.onframeend(...)`
    /// to register callbacks.
    pub fn load_script(&mut self, src_or_path: &str) -> mlua::Result<()> {
        let (src, name) = match std::fs::read_to_string(src_or_path) {
            Ok(contents) => (contents, src_or_path.to_string()),
            Err(_) => (src_or_path.to_string(), "<inline>".to_string()),
        };
        self.lua.load(&src).set_name(name).exec()
    }

    /// Re-create a fresh VM, discarding all registered callbacks and draw state,
    /// then reload `src_or_path`. Use this to hot-reload a script.
    /// Called by the script panel's "Reload" and "Clear VM" buttons.
    pub fn reload(&mut self, src_or_path: &str) -> mlua::Result<()> {
        let fresh = LuaEngine::new(SharedDebugState::clone(&self.debug))?;
        *self = fresh;
        self.load_script(src_or_path)
    }

    /// Run every registered `event.onframeend` callback for this frame.
    ///
    /// Clears the draw buffer first, then invokes each callback. A Lua runtime
    /// error is caught (mlua returns `Err`), logged to the debug event log, and
    /// execution continues with the next callback — a buggy script never crashes
    /// the app. Returns `Ok(())` even when individual callbacks errored.
    pub fn run_frame_callbacks(&self) -> mlua::Result<()> {
        self.draw_cmds.borrow_mut().clear();

        // Snapshot the registry keys we need to call. We borrow the Vec only to
        // read the keys; calling back into Lua does not touch this Vec.
        let count = self.frame_callbacks.borrow().len();
        for i in 0..count {
            // Re-borrow per iteration to keep the borrow short-lived.
            let func: mlua::Function = {
                let cbs = self.frame_callbacks.borrow();
                match self.lua.registry_value(&cbs[i]) {
                    Ok(f) => f,
                    Err(e) => {
                        self.log_error(&format!("bad callback registry value: {e}"));
                        continue;
                    }
                }
            };
            if let Err(e) = func.call::<()>(()) {
                self.log_error(&format!("onframeend callback error: {e}"));
                // continue — isolate the failure.
            }
        }
        Ok(())
    }

    /// Drain the current frame's draw commands (clears the internal buffer).
    pub fn take_draw_cmds(&self) -> Vec<DrawCmd> {
        std::mem::take(&mut *self.draw_cmds.borrow_mut())
    }

    /// Clear the draw buffer without returning its contents.
    #[allow(dead_code)]
    pub fn clear_draw_cmds(&self) {
        self.draw_cmds.borrow_mut().clear();
    }

    /// Return the number of registered `event.onframeend` callbacks.
    /// Used by the script panel to show registration status.
    pub fn callback_count(&self) -> usize {
        self.frame_callbacks.borrow().len()
    }

    fn log_error(&self, msg: &str) {
        if let Ok(mut ds) = self.debug.lock() {
            ds.log(format!("[lua] ERROR: {msg}"));
        }
        eprintln!("[lua] ERROR: {msg}");
    }
}

// ─── Compositor ──────────────────────────────────────────────────────────────

/// Alpha-blend draw commands into an RGBA8888 framebuffer in GAME-PIXEL space.
///
/// `rgba` is `[R, G, B, A]` per pixel, `width × height`. Boxes get a translucent
/// fill (alpha from `fill`'s low byte) plus a solid 1px outline (`line`). Anything
/// outside the buffer is clipped. Text is currently a TODO no-op (see below).
pub fn composite_into_rgba(cmds: &[DrawCmd], rgba: &mut [u8], width: u32, height: u32) {
    let w = width as i32;
    let h = height as i32;
    if w <= 0 || h <= 0 {
        return;
    }

    for cmd in cmds {
        match *cmd {
            DrawCmd::Box {
                x1,
                y1,
                x2,
                y2,
                fill,
                line,
            } => {
                let (lx, rx) = (x1.min(x2), x1.max(x2));
                let (ty, by) = (y1.min(y2), y1.max(y2));

                let (fr, fg, fb, fa) = unpack(fill);
                // Filled interior (inclusive bounds), alpha-blended.
                if fa > 0 {
                    for py in ty..=by {
                        if py < 0 || py >= h {
                            continue;
                        }
                        for px in lx..=rx {
                            if px < 0 || px >= w {
                                continue;
                            }
                            blend_px(rgba, w, px, py, fr, fg, fb, fa);
                        }
                    }
                }

                // 1px outline (solid blend; alpha from `line`).
                let (lr, lg, lb, la) = unpack(line);
                if la > 0 {
                    // Top & bottom edges.
                    for px in lx..=rx {
                        blend_clamped(rgba, w, h, px, ty, lr, lg, lb, la);
                        blend_clamped(rgba, w, h, px, by, lr, lg, lb, la);
                    }
                    // Left & right edges.
                    for py in ty..=by {
                        blend_clamped(rgba, w, h, lx, py, lr, lg, lb, la);
                        blend_clamped(rgba, w, h, rx, py, lr, lg, lb, la);
                    }
                }
            }
            DrawCmd::Text { x, y, ref s, color } => {
                draw_text(rgba, w, h, x, y, s, color);
            }
            DrawCmd::Line { x1, y1, x2, y2, color } => {
                let (r, g, b, a) = unpack(color);
                if a > 0 {
                    draw_line(rgba, w, h, x1, y1, x2, y2, r, g, b, a);
                }
            }
            DrawCmd::Pixel { x, y, color } => {
                let (r, g, b, a) = unpack(color);
                if a > 0 {
                    blend_clamped(rgba, w, h, x, y, r, g, b, a);
                }
            }
        }
    }
}

/// Unpack a packed `0xRRGGBBAA` color into `(r, g, b, a)`.
#[inline]
fn unpack(c: u32) -> (u8, u8, u8, u8) {
    (
        ((c >> 24) & 0xFF) as u8,
        ((c >> 16) & 0xFF) as u8,
        ((c >> 8) & 0xFF) as u8,
        (c & 0xFF) as u8,
    )
}

/// Alpha-blend a single source pixel onto `rgba` at (px, py). Caller guarantees
/// (px, py) is in-bounds. Standard src-over: out = src*a + dst*(1-a).
#[inline]
fn blend_px(rgba: &mut [u8], w: i32, px: i32, py: i32, sr: u8, sg: u8, sb: u8, sa: u8) {
    let idx = ((py * w + px) as usize) * 4;
    if idx + 3 >= rgba.len() {
        return;
    }
    let a = sa as u32;
    let inv = 255 - a;
    rgba[idx] = ((sr as u32 * a + rgba[idx] as u32 * inv) / 255) as u8;
    rgba[idx + 1] = ((sg as u32 * a + rgba[idx + 1] as u32 * inv) / 255) as u8;
    rgba[idx + 2] = ((sb as u32 * a + rgba[idx + 2] as u32 * inv) / 255) as u8;
    // Keep framebuffer opaque.
    rgba[idx + 3] = 0xFF;
}

/// Bounds-checked variant of `blend_px`.
#[inline]
fn blend_clamped(rgba: &mut [u8], w: i32, h: i32, px: i32, py: i32, sr: u8, sg: u8, sb: u8, sa: u8) {
    if px < 0 || px >= w || py < 0 || py >= h {
        return;
    }
    blend_px(rgba, w, px, py, sr, sg, sb, sa);
}

/// Bresenham integer line rasteriser. Draws all pixels from (x1,y1) to (x2,y2)
/// inclusive, alpha-blending each. Out-of-bounds pixels are silently clipped.
fn draw_line(rgba: &mut [u8], w: i32, h: i32, x1: i32, y1: i32, x2: i32, y2: i32, r: u8, g: u8, b: u8, a: u8) {
    let mut x = x1;
    let mut y = y1;
    let dx = (x2 - x1).abs();
    let dy = (y2 - y1).abs();
    let sx: i32 = if x1 < x2 { 1 } else { -1 };
    let sy: i32 = if y1 < y2 { 1 } else { -1 };
    let mut err = dx - dy;

    loop {
        blend_clamped(rgba, w, h, x, y, r, g, b, a);
        if x == x2 && y == y2 {
            break;
        }
        let e2 = err * 2;
        if e2 > -dy {
            err -= dy;
            x += sx;
        }
        if e2 < dx {
            err += dx;
            y += sy;
        }
    }
}

/// Minimal blocky text renderer: draws each character as a small filled 3×5 dot
/// pattern using a tiny built-in font. Good enough to label hitboxes; not a full
/// typeface. Unsupported glyphs render as a solid block.
fn draw_text(rgba: &mut [u8], w: i32, h: i32, x: i32, y: i32, s: &str, color: u32) {
    let (r, g, b, a) = unpack(color);
    if a == 0 {
        return;
    }
    const CW: i32 = 4; // cell width (3px glyph + 1px gap)
    const GH: i32 = 5; // glyph height
    let mut cx = x;
    for ch in s.chars() {
        let rows = glyph(ch);
        for (ry, bits) in rows.iter().enumerate() {
            for rxi in 0..3i32 {
                if (bits >> (2 - rxi)) & 1 == 1 {
                    blend_clamped(rgba, w, h, cx + rxi, y + ry as i32, r, g, b, a);
                }
            }
        }
        cx += CW;
        if cx >= w {
            break;
        }
    }
    let _ = GH;
}

/// 3×5 bitmap font: each glyph is 5 rows, each row is 3 low bits (MSB = left).
/// Covers 0-9, A-Z (uppercased), space, and a few symbols; falls back to a solid
/// block for anything else.
fn glyph(ch: char) -> [u8; 5] {
    let c = ch.to_ascii_uppercase();
    match c {
        ' ' => [0b000, 0b000, 0b000, 0b000, 0b000],
        '0' => [0b111, 0b101, 0b101, 0b101, 0b111],
        '1' => [0b010, 0b110, 0b010, 0b010, 0b111],
        '2' => [0b111, 0b001, 0b111, 0b100, 0b111],
        '3' => [0b111, 0b001, 0b111, 0b001, 0b111],
        '4' => [0b101, 0b101, 0b111, 0b001, 0b001],
        '5' => [0b111, 0b100, 0b111, 0b001, 0b111],
        '6' => [0b111, 0b100, 0b111, 0b101, 0b111],
        '7' => [0b111, 0b001, 0b010, 0b010, 0b010],
        '8' => [0b111, 0b101, 0b111, 0b101, 0b111],
        '9' => [0b111, 0b101, 0b111, 0b001, 0b111],
        'A' => [0b111, 0b101, 0b111, 0b101, 0b101],
        'B' => [0b110, 0b101, 0b110, 0b101, 0b110],
        'C' => [0b111, 0b100, 0b100, 0b100, 0b111],
        'D' => [0b110, 0b101, 0b101, 0b101, 0b110],
        'E' => [0b111, 0b100, 0b110, 0b100, 0b111],
        'F' => [0b111, 0b100, 0b110, 0b100, 0b100],
        'G' => [0b111, 0b100, 0b101, 0b101, 0b111],
        'H' => [0b101, 0b101, 0b111, 0b101, 0b101],
        'I' => [0b111, 0b010, 0b010, 0b010, 0b111],
        'L' => [0b100, 0b100, 0b100, 0b100, 0b111],
        'O' => [0b111, 0b101, 0b101, 0b101, 0b111],
        'P' => [0b111, 0b101, 0b111, 0b100, 0b100],
        'R' => [0b110, 0b101, 0b110, 0b101, 0b101],
        'S' => [0b111, 0b100, 0b111, 0b001, 0b111],
        'T' => [0b111, 0b010, 0b010, 0b010, 0b010],
        'X' => [0b101, 0b101, 0b010, 0b101, 0b101],
        'Y' => [0b101, 0b101, 0b010, 0b010, 0b010],
        ':' => [0b000, 0b010, 0b000, 0b010, 0b000],
        '-' => [0b000, 0b000, 0b111, 0b000, 0b000],
        '.' => [0b000, 0b000, 0b000, 0b000, 0b010],
        _ => [0b111, 0b111, 0b111, 0b111, 0b111], // unknown → solid block
    }
}
