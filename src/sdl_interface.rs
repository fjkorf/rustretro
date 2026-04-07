use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use sdl2::audio::{AudioCallback, AudioDevice};
use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use std::sync::Arc;

pub struct AudioHandler {
    sample_rate: f64,
}

impl AudioCallback for AudioHandler {
    type Channel = i16;

    fn callback(&mut self, out: &mut [i16]) {
        // Samples are written via queue in the main thread
        for sample in out.iter_mut() {
            *sample = 0;
        }
    }
}

pub struct Graphics {
    is_initialized: bool,
}

impl Graphics {
    pub fn new(
        sdl: &sdl2::Sdl,
        width: u32,
        height: u32,
        scale: u32,
        fullscreen: bool,
    ) -> Result<Self> {
        let video_subsystem = sdl.video().map_err(|e| anyhow!(e))?;

        let window_result = if fullscreen {
            video_subsystem
                .window(
                    "RustRetro",
                    width * scale,
                    height * scale,
                )
                .position_centered()
                .fullscreen()
                .build()
        } else {
            video_subsystem
                .window(
                    "RustRetro",
                    width * scale,
                    height * scale,
                )
                .position_centered()
                .build()
        };

        let _window = window_result.map_err(|e| anyhow!(e.to_string()))?;

        Ok(Graphics {
            is_initialized: true,
        })
    }

    pub fn render_frame(&mut self, _data: &[u8], _pitch: usize) -> Result<()> {
        // Frame rendering would happen here
        Ok(())
    }

    pub fn set_dimensions(&mut self, _width: u32, _height: u32) -> Result<()> {
        Ok(())
    }
}

pub struct Audio {
    _device: AudioDevice<AudioHandler>,
    queue: Arc<Mutex<Vec<i16>>>,
}

impl Audio {
    pub fn new(_sdl: &sdl2::Sdl, sample_rate: f64) -> Result<Self> {
        let audio_subsystem = _sdl.audio().map_err(|e| anyhow!(e))?;

        let desired_spec = sdl2::audio::AudioSpecDesired {
            freq: Some(sample_rate as i32),
            channels: Some(2),
            samples: Some(2048),
        };

        let device = audio_subsystem
            .open_playback(None, &desired_spec, |_| AudioHandler {
                sample_rate,
            })
            .map_err(|e| anyhow!(e))?;

        device.resume();

        Ok(Audio {
            _device: device,
            queue: Arc::new(Mutex::new(Vec::new())),
        })
    }

    pub fn queue_sample(&self, left: i16, right: i16) {
        let mut queue = self.queue.lock();
        queue.push(left);
        queue.push(right);
    }

    pub fn process_queue(&self) {
        let mut queue = self.queue.lock();
        if !queue.is_empty() {
            let _samples = queue.drain(..).collect::<Vec<_>>();
        }
    }
}

pub struct Input {
    joypad_state: [bool; 12],
}

impl Input {
    pub fn new() -> Self {
        Input {
            joypad_state: [false; 12],
        }
    }

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
            Keycode::Up => self.joypad_state[4] = pressed,
            Keycode::Down => self.joypad_state[5] = pressed,
            Keycode::Left => self.joypad_state[6] = pressed,
            Keycode::Right => self.joypad_state[7] = pressed,
            Keycode::Z => self.joypad_state[0] = pressed,    // B
            Keycode::X => self.joypad_state[8] = pressed,    // A
            Keycode::A => self.joypad_state[1] = pressed,    // Y
            Keycode::S => self.joypad_state[9] = pressed,    // X
            Keycode::Return => self.joypad_state[3] = pressed, // Start
            Keycode::LShift | Keycode::RShift => self.joypad_state[2] = pressed, // Select
            Keycode::Q => self.joypad_state[10] = pressed,   // L
            Keycode::W => self.joypad_state[11] = pressed,   // R
            _ => {}
        }
    }

    pub fn get_button_state(&self, button: u32) -> i16 {
        if button < 12 && self.joypad_state[button as usize] {
            1
        } else {
            0
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
            assert_eq!(input.get_button_state(i), 0);
        }
    }
}
