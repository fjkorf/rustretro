use anyhow::{anyhow, Result};
use sdl2::audio::{AudioQueue, AudioSpecDesired};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::pixels::PixelFormatEnum;
use sdl2::render::{Canvas, Texture, TextureCreator};
use sdl2::video::{Window, WindowContext};

pub struct Graphics {
    // Drop order matters: texture must be dropped before canvas/texture_creator
    // (Rust drops fields in declaration order)
    texture: Option<Texture<'static>>,
    canvas: Canvas<Window>,
    texture_creator: TextureCreator<WindowContext>,
    tex_width: u32,
    tex_height: u32,
    tex_fmt: PixelFormatEnum,
    pub scale: u32,
}

impl Graphics {
    pub fn new(
        sdl: &sdl2::Sdl,
        width: u32,
        height: u32,
        scale: u32,
        fullscreen: bool,
    ) -> Result<Self> {
        let video = sdl.video().map_err(|e| anyhow!(e))?;

        let mut wb = video.window("RustRetro", width * scale, height * scale);
        wb.position_centered();
        if fullscreen {
            wb.fullscreen_desktop();
        }
        let window = wb.build().map_err(|e| anyhow!(e.to_string()))?;

        let canvas = window
            .into_canvas()
            .accelerated()
            .build()
            .map_err(|e| anyhow!(e.to_string()))?;

        let texture_creator = canvas.texture_creator();

        Ok(Graphics {
            texture: None,
            canvas,
            texture_creator,
            tex_width: 0,
            tex_height: 0,
            tex_fmt: PixelFormatEnum::Unknown,
            scale,
        })
    }

    pub fn resize_window(&mut self, width: u32, height: u32) {
        let _ = self
            .canvas
            .window_mut()
            .set_size(width * self.scale, height * self.scale);
    }

    pub fn set_title(&mut self, title: &str) {
        let _ = self.canvas.window_mut().set_title(title);
    }

    fn ensure_texture(&mut self, width: u32, height: u32, sdl_fmt: PixelFormatEnum) -> Result<()> {
        if self.tex_width == width && self.tex_height == height && self.tex_fmt == sdl_fmt {
            return Ok(());
        }
        // Drop old texture before creating new one
        self.texture = None;
        let tex = self
            .texture_creator
            .create_texture_streaming(sdl_fmt, width, height)
            .map_err(|e| anyhow!(e.to_string()))?;
        // Safety: texture_creator lives in the same struct as texture,
        // and texture is declared before canvas/texture_creator so it's
        // dropped first. The SDL renderer (held via Rc) stays alive.
        self.texture = Some(unsafe { std::mem::transmute(tex) });
        self.tex_width = width;
        self.tex_height = height;
        self.tex_fmt = sdl_fmt;
        Ok(())
    }

    pub fn render_frame(
        &mut self,
        data: &[u8],
        width: u32,
        height: u32,
        pitch: usize,
        pixel_format: u32, // libretro pixel format constant
    ) -> Result<()> {
        // Always convert to ARGB8888 before uploading to SDL.
        // SDL2's Metal renderer on macOS only reliably supports ARGB8888
        // as a streaming texture format; RGB565 and ARGB1555 silently fail.
        let argb_pitch = width as usize * 4;
        let argb = to_argb8888(data, width, height, pitch, pixel_format);

        self.ensure_texture(width, height, PixelFormatEnum::ARGB8888)?;
        if let Some(ref mut tex) = self.texture {
            tex.update(None, &argb, argb_pitch)
                .map_err(|e| anyhow!(e.to_string()))?;
        }

        self.canvas.clear();
        if let Some(ref tex) = self.texture {
            self.canvas
                .copy(tex, None, None)
                .map_err(|e| anyhow!(e.to_string()))?;
        }
        self.canvas.present();
        Ok(())
    }
}

/// Convert any libretro pixel format to packed ARGB8888 bytes.
///
/// libretro formats:
///   0 = 0RGB1555  — 2 bytes/pixel, bits: `0RRRRRGGGGGBBBBB`
///   1 = XRGB8888  — 4 bytes/pixel, little-endian: `[B, G, R, X]`
///   2 = RGB565    — 2 bytes/pixel, bits: `RRRRRGGGGGGBBBBB`
///
/// Output: packed ARGB8888, little-endian: `[B, G, R, A]` per pixel.
fn to_argb8888(src: &[u8], width: u32, height: u32, src_pitch: usize, fmt: u32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut out = vec![0u8; w * h * 4];

    for y in 0..h {
        let in_row  = &src[y * src_pitch..];
        let out_row = &mut out[y * w * 4..];

        match fmt {
            2 => {
                // RGB565: RRRRRGGGGGGBBBBB
                for x in 0..w {
                    let lo = in_row[x * 2] as u16;
                    let hi = in_row[x * 2 + 1] as u16;
                    let p = lo | (hi << 8);
                    let b = ((p & 0x001F) << 3) as u8;
                    let g = (((p >> 5) & 0x003F) << 2) as u8;
                    let r = (((p >> 11) & 0x001F) << 3) as u8;
                    out_row[x * 4]     = b;
                    out_row[x * 4 + 1] = g;
                    out_row[x * 4 + 2] = r;
                    out_row[x * 4 + 3] = 0xFF;
                }
            }
            1 => {
                // XRGB8888: already [B, G, R, X] in memory — copy with A=FF
                for x in 0..w {
                    out_row[x * 4]     = in_row[x * 4];     // B
                    out_row[x * 4 + 1] = in_row[x * 4 + 1]; // G
                    out_row[x * 4 + 2] = in_row[x * 4 + 2]; // R
                    out_row[x * 4 + 3] = 0xFF;               // A
                }
            }
            _ => {
                // 0RGB1555: 0RRRRRGGGGGBBBBB
                for x in 0..w {
                    let lo = in_row[x * 2] as u16;
                    let hi = in_row[x * 2 + 1] as u16;
                    let p = lo | (hi << 8);
                    let b = ((p & 0x001F) << 3) as u8;
                    let g = (((p >> 5) & 0x001F) << 3) as u8;
                    let r = (((p >> 10) & 0x001F) << 3) as u8;
                    out_row[x * 4]     = b;
                    out_row[x * 4 + 1] = g;
                    out_row[x * 4 + 2] = r;
                    out_row[x * 4 + 3] = 0xFF;
                }
            }
        }
    }
    out
}

pub struct Audio {
    queue: AudioQueue<i16>,
    pub sample_rate: f64,
}

impl Audio {
    pub fn new(sdl: &sdl2::Sdl, sample_rate: f64) -> Result<Self> {
        let audio_subsystem = sdl.audio().map_err(|e| anyhow!(e))?;

        let spec = AudioSpecDesired {
            freq: Some(sample_rate as i32),
            channels: Some(2),
            samples: Some(2048),
        };

        let queue: AudioQueue<i16> = audio_subsystem
            .open_queue(None, &spec)
            .map_err(|e| anyhow!(e))?;
        queue.resume();

        Ok(Audio { queue, sample_rate })
    }

    pub fn queue_audio(&self, samples: &[i16]) {
        let _ = self.queue.queue_audio(samples);
    }
}

pub struct Input {
    pub joypad_state: [bool; 12],
    pub f12_pressed: bool,
}

impl Input {
    pub fn new() -> Self {
        Input {
            joypad_state: [false; 12],
            f12_pressed: false,
        }
    }

    /// Returns true if the quit event was received.
    pub fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::Quit { .. } => true,
            Event::KeyDown {
                keycode: Some(code),
                ..
            } => {
                self.update_key(*code, true);
                false
            }
            Event::KeyUp {
                keycode: Some(code),
                ..
            } => {
                self.update_key(*code, false);
                false
            }
            _ => false,
        }
    }

    fn update_key(&mut self, keycode: Keycode, pressed: bool) {
        match keycode {
            Keycode::Z => self.joypad_state[0] = pressed,
            Keycode::A => self.joypad_state[1] = pressed,
            Keycode::LShift | Keycode::RShift => self.joypad_state[2] = pressed,
            Keycode::Return => self.joypad_state[3] = pressed,
            Keycode::Up => self.joypad_state[4] = pressed,
            Keycode::Down => self.joypad_state[5] = pressed,
            Keycode::Left => self.joypad_state[6] = pressed,
            Keycode::Right => self.joypad_state[7] = pressed,
            Keycode::X => self.joypad_state[8] = pressed,
            Keycode::S => self.joypad_state[9] = pressed,
            Keycode::Q => self.joypad_state[10] = pressed,
            Keycode::W => self.joypad_state[11] = pressed,
            Keycode::F12 => { if pressed { self.f12_pressed = true; } }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_initialization() {
        let input = Input::new();
        for i in 0..12 {
            assert!(!input.joypad_state[i]);
        }
    }
}
