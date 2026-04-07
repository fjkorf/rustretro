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
            scale,
        })
    }

    pub fn resize_window(&mut self, width: u32, height: u32) {
        let _ = self
            .canvas
            .window_mut()
            .set_size(width * self.scale, height * self.scale);
    }

    fn ensure_texture(&mut self, width: u32, height: u32, sdl_fmt: PixelFormatEnum) -> Result<()> {
        if self.tex_width == width && self.tex_height == height {
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
        // libretro XRGB8888 (=1) is layout-compatible with SDL ARGB8888 on little-endian
        // libretro RGB565 (=2) matches SDL RGB565 directly
        let sdl_fmt = if pixel_format == 2 {
            PixelFormatEnum::RGB565
        } else {
            PixelFormatEnum::ARGB8888
        };

        self.ensure_texture(width, height, sdl_fmt)?;

        if let Some(ref mut tex) = self.texture {
            tex.update(None, data, pitch)
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
}

impl Input {
    pub fn new() -> Self {
        Input {
            joypad_state: [false; 12],
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
            Keycode::Z => self.joypad_state[0] = pressed,      // B
            Keycode::A => self.joypad_state[1] = pressed,      // Y
            Keycode::LShift | Keycode::RShift => self.joypad_state[2] = pressed, // Select
            Keycode::Return => self.joypad_state[3] = pressed, // Start
            Keycode::Up => self.joypad_state[4] = pressed,
            Keycode::Down => self.joypad_state[5] = pressed,
            Keycode::Left => self.joypad_state[6] = pressed,
            Keycode::Right => self.joypad_state[7] = pressed,
            Keycode::X => self.joypad_state[8] = pressed,      // A
            Keycode::S => self.joypad_state[9] = pressed,      // X
            Keycode::Q => self.joypad_state[10] = pressed,     // L
            Keycode::W => self.joypad_state[11] = pressed,     // R
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
