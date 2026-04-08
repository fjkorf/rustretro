use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::{Arc, Mutex};

/// Shared ring buffer: emulation fills it, cpal stream drains it.
type SampleBuf = Arc<Mutex<Vec<i16>>>;

/// Audio output resource — `Send + Sync` so it can be a Bevy `Resource`.
/// The cpal `Stream` lives on a background thread; we communicate via the buffer.
pub struct AudioOutput {
    buf: SampleBuf,
    pub sample_rate: f64,
    pub enabled: bool,
}

// The stream handle stays alive via the background thread; AudioOutput itself is safe to send.
unsafe impl Send for AudioOutput {}
unsafe impl Sync for AudioOutput {}

impl AudioOutput {
    pub fn new(enabled: bool) -> Self {
        let buf: SampleBuf = Arc::new(Mutex::new(Vec::with_capacity(8192)));
        let sample_rate;

        if enabled {
            sample_rate = Self::start_stream(Arc::clone(&buf));
        } else {
            sample_rate = 44100.0;
        }

        AudioOutput { buf, sample_rate, enabled }
    }

    /// Queue raw stereo i16 samples for playback.
    pub fn queue(&self, samples: &[i16]) {
        if !self.enabled || samples.is_empty() { return; }
        // Don't let the buffer grow unbounded — drop oldest if we're way behind.
        let mut b = self.buf.lock().unwrap();
        let blen = b.len();
        if blen > 48000 * 2 { // ~0.5 s of stereo @ 48 kHz
            b.drain(0..blen / 2);
        }
        b.extend_from_slice(samples);
    }

    fn start_stream(buf: SampleBuf) -> f64 {
        let host = cpal::default_host();
        let device = match host.default_output_device() {
            Some(d) => d,
            None => { eprintln!("[audio] No output device"); return 44100.0; }
        };

        let config = match device.default_output_config() {
            Ok(c) => c,
            Err(e) => { eprintln!("[audio] Config error: {e}"); return 44100.0; }
        };

        let sample_rate = config.sample_rate().0 as f64;
        let channels    = config.channels() as usize;

        let buf_clone = Arc::clone(&buf);
        let err_fn = |e| eprintln!("[audio] stream error: {e}");

        let stream = match config.sample_format() {
            cpal::SampleFormat::I16 => device.build_output_stream(
                &config.into(),
                move |out: &mut [i16], _| drain_i16(out, &buf_clone, channels),
                err_fn, None,
            ),
            cpal::SampleFormat::F32 => device.build_output_stream(
                &config.into(),
                move |out: &mut [f32], _| drain_f32(out, &buf_clone, channels),
                err_fn, None,
            ),
            _ => { eprintln!("[audio] Unsupported sample format"); return sample_rate; }
        };

        match stream {
            Ok(s) => {
                if let Err(e) = s.play() { eprintln!("[audio] play error: {e}"); }
                // cpal::Stream is not Send on macOS CoreAudio — use unsafe wrapper.
                struct SendStream(cpal::Stream);
                unsafe impl Send for SendStream {}
                let wrapped = SendStream(s);
                std::thread::spawn(move || {
                    let _s = wrapped; // stream stays alive until thread ends
                    loop { std::thread::sleep(std::time::Duration::from_secs(60)); }
                });
            }
            Err(e) => eprintln!("[audio] Build stream error: {e}"),
        }

        sample_rate
    }
}

fn drain_i16(out: &mut [i16], buf: &SampleBuf, channels: usize) {
    let mut b = buf.lock().unwrap();
    for frame in out.chunks_mut(channels) {
        if b.len() >= 2 {
            frame[0] = b.remove(0);
            if channels > 1 { frame[1] = b.remove(0); } else { b.remove(0); }
        } else {
            for s in frame.iter_mut() { *s = 0; }
        }
    }
}

fn drain_f32(out: &mut [f32], buf: &SampleBuf, channels: usize) {
    let mut b = buf.lock().unwrap();
    for frame in out.chunks_mut(channels) {
        if b.len() >= 2 {
            frame[0] = b.remove(0) as f32 / i16::MAX as f32;
            if channels > 1 {
                frame[1] = b.remove(0) as f32 / i16::MAX as f32;
            } else {
                b.remove(0);
            }
        } else {
            for s in frame.iter_mut() { *s = 0.0; }
        }
    }
}
