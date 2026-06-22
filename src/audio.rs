use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Shared ring buffer: emulation fills it, cpal stream drains it.
type SampleBuf = Arc<Mutex<Vec<i16>>>;

/// Audio output resource — `Send + Sync` so it can be a Bevy `Resource`.
/// The cpal `Stream` lives on a background thread; we communicate via the buffer.
///
/// `volume` and `muted` are shared via `Arc<Atomic*>` so that *every* clone of
/// `AudioOutput` (the playing resource, the debug overlay, etc.) observes the same
/// value. They are applied at *drain* time in the cpal callback so that changes
/// affect already-buffered audio and unmute recovers immediately.
#[derive(Clone)]
pub struct AudioOutput {
    buf: SampleBuf,
    pub sample_rate: f64,
    pub enabled: bool,
    /// f32 volume stored as raw bits via `f32::to_bits` / `from_bits`.
    volume: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
}

// The stream handle stays alive via the background thread; AudioOutput itself is safe to send.
unsafe impl Send for AudioOutput {}
unsafe impl Sync for AudioOutput {}

impl AudioOutput {
    pub fn new(enabled: bool) -> Self {
        let buf: SampleBuf = Arc::new(Mutex::new(Vec::with_capacity(8192)));
        let volume = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let muted = Arc::new(AtomicBool::new(false));
        let sample_rate;

        if enabled {
            sample_rate = Self::start_stream(
                Arc::clone(&buf),
                Arc::clone(&volume),
                Arc::clone(&muted),
            );
        } else {
            sample_rate = 44100.0;
        }

        AudioOutput { buf, sample_rate, enabled, volume, muted }
    }

    pub fn set_volume(&mut self, vol: f32) {
        let clamped = vol.max(0.0).min(1.0);
        self.volume.store(clamped.to_bits(), Ordering::Relaxed);
    }

    pub fn set_mute(&mut self, mute: bool) {
        self.muted.store(mute, Ordering::Relaxed);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }

    pub fn get_volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    /// Queue raw stereo i16 samples for playback.
    ///
    /// Mute and volume are NOT applied here — they're applied per-sample at drain
    /// time in the cpal callback, so changes affect already-buffered audio.
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

    fn start_stream(buf: SampleBuf, volume: Arc<AtomicU32>, muted: Arc<AtomicBool>) -> f64 {
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

        let err_fn = |e| eprintln!("[audio] stream error: {e}");

        let stream = match config.sample_format() {
            cpal::SampleFormat::I16 => {
                let buf_clone = Arc::clone(&buf);
                let vol_clone = Arc::clone(&volume);
                let mute_clone = Arc::clone(&muted);
                device.build_output_stream(
                    &config.into(),
                    move |out: &mut [i16], _| drain_i16(out, &buf_clone, channels, &vol_clone, &mute_clone),
                    err_fn, None,
                )
            }
            cpal::SampleFormat::F32 => {
                let buf_clone = Arc::clone(&buf);
                let vol_clone = Arc::clone(&volume);
                let mute_clone = Arc::clone(&muted);
                device.build_output_stream(
                    &config.into(),
                    move |out: &mut [f32], _| drain_f32(out, &buf_clone, channels, &vol_clone, &mute_clone),
                    err_fn, None,
                )
            }
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

/// Load the current volume, treating "near 1.0" as no scaling.
fn current_volume(volume: &AtomicU32) -> f32 {
    f32::from_bits(volume.load(Ordering::Relaxed))
}

fn drain_i16(out: &mut [i16], buf: &SampleBuf, channels: usize, volume: &AtomicU32, muted: &AtomicBool) {
    let mut b = buf.lock().unwrap();
    let is_muted = muted.load(Ordering::Relaxed);
    let vol = current_volume(volume);
    for frame in out.chunks_mut(channels) {
        if b.len() >= 2 {
            // Always consume the buffer (even when muted) so unmute resumes at "now".
            let l = b.remove(0);
            let r = if channels > 1 { b.remove(0) } else { b.remove(0); 0 };
            if is_muted {
                for s in frame.iter_mut() { *s = 0; }
            } else {
                frame[0] = scale_i16(l, vol);
                if channels > 1 { frame[1] = scale_i16(r, vol); }
            }
        } else {
            for s in frame.iter_mut() { *s = 0; }
        }
    }
}

fn drain_f32(out: &mut [f32], buf: &SampleBuf, channels: usize, volume: &AtomicU32, muted: &AtomicBool) {
    let mut b = buf.lock().unwrap();
    let is_muted = muted.load(Ordering::Relaxed);
    let vol = current_volume(volume);
    for frame in out.chunks_mut(channels) {
        if b.len() >= 2 {
            // Always consume the buffer (even when muted) so unmute resumes at "now".
            let l = b.remove(0);
            let r = if channels > 1 { b.remove(0) } else { b.remove(0); 0 };
            if is_muted {
                for s in frame.iter_mut() { *s = 0.0; }
            } else {
                frame[0] = (l as f32 / i16::MAX as f32) * vol;
                if channels > 1 { frame[1] = (r as f32 / i16::MAX as f32) * vol; }
            }
        } else {
            for s in frame.iter_mut() { *s = 0.0; }
        }
    }
}

/// Scale a single i16 sample by `volume`, clamping to the i16 range.
fn scale_i16(sample: i16, volume: f32) -> i16 {
    if volume >= 0.99 { return sample; }
    let scaled = (sample as f32) * volume;
    scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_shared_across_clones() {
        let original = AudioOutput::new(false);
        let mut clone = original.clone();
        clone.set_volume(0.5);
        assert_eq!(original.get_volume(), 0.5, "volume Arc must be shared between clones");
    }

    #[test]
    fn mute_is_shared_across_clones() {
        let original = AudioOutput::new(false);
        let mut clone = original.clone();
        clone.set_mute(true);
        assert!(original.is_muted(), "mute Arc must be shared between clones");
    }
}
