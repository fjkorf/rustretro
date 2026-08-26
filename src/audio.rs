use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Shared ring buffer: emulation fills it, cpal stream drains it.
/// VecDeque so the real-time callback pops from the front in O(1) — the old
/// `Vec::remove(0)` was O(n) per SAMPLE inside the callback.
type SampleBuf = Arc<Mutex<VecDeque<i16>>>;

/// Audio output resource — `Send + Sync` so it can be a Bevy `Resource`.
/// The cpal `Stream` lives on a background thread; we communicate via the buffer.
///
/// `volume` and `muted` are shared via `Arc<Atomic*>` so that *every* clone of
/// `AudioOutput` (the playing resource, the debug overlay, etc.) observes the same
/// value. They are applied at *drain* time in the cpal callback so that changes
/// affect already-buffered audio and unmute recovers immediately.
///
/// The core's sample rate (`in_rate`, e.g. fbalpha2012's 32040 Hz) rarely equals
/// the device's (typically 48000): the drain callbacks LINEARLY RESAMPLE by the
/// live in/out ratio. Without this the audio played ~1.5x fast (shrill) and the
/// queue starved (gaps). `in_rate` is an atomic so a mid-game
/// SET_SYSTEM_AV_INFO rate change propagates.
#[derive(Clone)]
pub struct AudioOutput {
    buf: SampleBuf,
    /// Device output rate (display/reference).
    pub sample_rate: f64,
    pub enabled: bool,
    /// f32 volume stored as raw bits via `f32::to_bits` / `from_bits`.
    volume: Arc<AtomicU32>,
    muted: Arc<AtomicBool>,
    /// Core/input sample rate, f64 bits.
    in_rate: Arc<AtomicU64>,
}

// The stream handle stays alive via the background thread; AudioOutput itself is safe to send.
unsafe impl Send for AudioOutput {}
unsafe impl Sync for AudioOutput {}

/// Per-stream resampler cursor (lives inside the cpal callback closure).
struct ResampleState {
    phase: f64,
    prev: [i16; 2],
    next: [i16; 2],
    primed: bool,
}

impl ResampleState {
    fn new() -> Self {
        ResampleState { phase: 1.0, prev: [0; 2], next: [0; 2], primed: false }
    }

    /// Advance to the interpolation position for the next output frame.
    /// Returns None on underrun (caller writes silence).
    fn step(&mut self, b: &mut VecDeque<i16>, ratio: f64) -> Option<(f32, [i16; 2], [i16; 2])> {
        while self.phase >= 1.0 {
            if b.len() < 2 {
                return None;
            }
            self.prev = self.next;
            self.next = [b.pop_front().unwrap(), b.pop_front().unwrap()];
            self.phase -= 1.0;
            self.primed = true;
        }
        if !self.primed {
            return None;
        }
        let t = self.phase as f32;
        self.phase += ratio;
        Some((t, self.prev, self.next))
    }
}

impl AudioOutput {
    /// `core_rate` = the rate the loaded core synthesizes at
    /// (`retro_get_system_av_info`); update later via [`set_input_rate`].
    pub fn new(enabled: bool, core_rate: f64) -> Self {
        let buf: SampleBuf = Arc::new(Mutex::new(VecDeque::with_capacity(8192)));
        let volume = Arc::new(AtomicU32::new(1.0_f32.to_bits()));
        let muted = Arc::new(AtomicBool::new(false));
        let in_rate = Arc::new(AtomicU64::new(core_rate.max(1.0).to_bits()));
        let sample_rate;

        if enabled {
            sample_rate = Self::start_stream(
                Arc::clone(&buf),
                Arc::clone(&volume),
                Arc::clone(&muted),
                Arc::clone(&in_rate),
            );
            eprintln!(
                "[audio] resampling {core_rate:.0} Hz (core) -> {sample_rate:.0} Hz (device), \
                 ratio {:.4}",
                core_rate / sample_rate
            );
        } else {
            sample_rate = 44100.0;
        }

        AudioOutput { buf, sample_rate, enabled, volume, muted, in_rate }
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

    /// Keep the resampler's input rate in sync with the core (cheap; call
    /// per frame — handles mid-game SET_SYSTEM_AV_INFO changes).
    pub fn set_input_rate(&self, rate: f64) {
        self.in_rate.store(rate.max(1.0).to_bits(), Ordering::Relaxed);
    }

    /// Queue raw stereo i16 samples for playback.
    ///
    /// Mute and volume are NOT applied here — they're applied per-sample at drain
    /// time in the cpal callback, so changes affect already-buffered audio.
    pub fn queue(&self, samples: &[i16]) {
        if !self.enabled || samples.is_empty() { return; }
        let in_rate = f64::from_bits(self.in_rate.load(Ordering::Relaxed));
        // Cap ~200 ms of CORE-rate audio; when over, drop down to ~50 ms in
        // whole stereo frames (not "half the buffer" — that skipped 0.25 s).
        let cap = (in_rate * 0.2) as usize * 2;
        let target = (in_rate * 0.05) as usize * 2;
        let mut b = self.buf.lock().unwrap();
        if b.len() + samples.len() > cap {
            let excess = (b.len() + samples.len()).saturating_sub(target);
            let drop = excess.min(b.len()) & !1; // even = whole frames
            b.drain(0..drop);
        }
        b.extend(samples.iter().copied());
    }

    fn start_stream(
        buf: SampleBuf,
        volume: Arc<AtomicU32>,
        muted: Arc<AtomicBool>,
        in_rate: Arc<AtomicU64>,
    ) -> f64 {
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
                let rate_clone = Arc::clone(&in_rate);
                let mut st = ResampleState::new();
                device.build_output_stream(
                    &config.into(),
                    move |out: &mut [i16], _| {
                        drain_i16(out, &buf_clone, channels, &vol_clone, &mute_clone,
                                  &rate_clone, sample_rate, &mut st)
                    },
                    err_fn, None,
                )
            }
            cpal::SampleFormat::F32 => {
                let buf_clone = Arc::clone(&buf);
                let vol_clone = Arc::clone(&volume);
                let mute_clone = Arc::clone(&muted);
                let rate_clone = Arc::clone(&in_rate);
                let mut st = ResampleState::new();
                device.build_output_stream(
                    &config.into(),
                    move |out: &mut [f32], _| {
                        drain_f32(out, &buf_clone, channels, &vol_clone, &mute_clone,
                                  &rate_clone, sample_rate, &mut st)
                    },
                    err_fn, None,
                )
            }
            _ => { eprintln!("[audio] Unsupported sample format"); return sample_rate; }
        };

        match stream {
            Ok(s) => {
                if let Err(e) = s.play() { eprintln!("[audio] play error: {e}"); }
                // cpal::Stream is not Send on macOS CoreAudio — use unsafe wrapper.
                // The field is never read — it exists to keep the stream alive.
                struct SendStream(#[allow(dead_code)] cpal::Stream);
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

fn drain_i16(
    out: &mut [i16], buf: &SampleBuf, channels: usize,
    volume: &AtomicU32, muted: &AtomicBool,
    in_rate: &AtomicU64, out_rate: f64, st: &mut ResampleState,
) {
    let ratio = f64::from_bits(in_rate.load(Ordering::Relaxed)) / out_rate;
    let mut b = buf.lock().unwrap();
    let is_muted = muted.load(Ordering::Relaxed);
    let vol = current_volume(volume);
    for frame in out.chunks_mut(channels) {
        match st.step(&mut b, ratio) {
            // Consume even when muted so unmute resumes at "now".
            Some((t, prev, next)) if !is_muted => {
                let l = prev[0] as f32 + t * (next[0] - prev[0]) as f32;
                let r = prev[1] as f32 + t * (next[1] - prev[1]) as f32;
                frame[0] = scale_i16(l as i16, vol);
                if channels > 1 { frame[1] = scale_i16(r as i16, vol); }
                for s in frame.iter_mut().skip(2) { *s = 0; }
            }
            _ => {
                for s in frame.iter_mut() { *s = 0; }
            }
        }
    }
}

fn drain_f32(
    out: &mut [f32], buf: &SampleBuf, channels: usize,
    volume: &AtomicU32, muted: &AtomicBool,
    in_rate: &AtomicU64, out_rate: f64, st: &mut ResampleState,
) {
    let ratio = f64::from_bits(in_rate.load(Ordering::Relaxed)) / out_rate;
    let mut b = buf.lock().unwrap();
    let is_muted = muted.load(Ordering::Relaxed);
    let vol = current_volume(volume);
    for frame in out.chunks_mut(channels) {
        match st.step(&mut b, ratio) {
            // Consume even when muted so unmute resumes at "now".
            Some((t, prev, next)) if !is_muted => {
                let l = prev[0] as f32 + t * (next[0] - prev[0]) as f32;
                let r = prev[1] as f32 + t * (next[1] - prev[1]) as f32;
                frame[0] = (l / i16::MAX as f32) * vol;
                if channels > 1 { frame[1] = (r / i16::MAX as f32) * vol; }
                for s in frame.iter_mut().skip(2) { *s = 0.0; }
            }
            _ => {
                for s in frame.iter_mut() { *s = 0.0; }
            }
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
        let original = AudioOutput::new(false, 32040.0);
        let mut clone = original.clone();
        clone.set_volume(0.5);
        assert_eq!(original.get_volume(), 0.5, "volume Arc must be shared between clones");
    }

    #[test]
    fn mute_is_shared_across_clones() {
        let original = AudioOutput::new(false, 32040.0);
        let mut clone = original.clone();
        clone.set_mute(true);
        assert!(original.is_muted(), "mute Arc must be shared between clones");
    }

    #[test]
    fn resampler_upsamples_at_the_expected_ratio() {
        // 100 input frames at ratio 2/3 (32k -> 48k) should produce ~150
        // output frames before underrunning.
        let mut b: VecDeque<i16> = (0..200).map(|i| (i % 100) as i16).collect();
        let mut st = ResampleState::new();
        let mut produced = 0;
        while st.step(&mut b, 32040.0 / 48000.0).is_some() {
            produced += 1;
        }
        assert!((145..=152).contains(&produced), "produced {produced} frames");
    }

    #[test]
    fn queue_cap_drops_whole_frames_to_target() {
        let a = AudioOutput::new(false, 32040.0);
        // enabled=false short-circuits queue(); poke the buffer directly to
        // exercise the cap math via a fake enabled clone.
        let mut on = a.clone();
        on.enabled = true;
        let big = vec![1i16; (32040.0 * 0.2) as usize * 2];
        on.queue(&big);
        on.queue(&[2i16; 1068]); // one more core frame triggers the cap
        let len = on.buf.lock().unwrap().len();
        assert!(len % 2 == 0, "even sample count (whole stereo frames)");
        assert!(len <= (32040.0 * 0.06) as usize * 2, "dropped to ~target, got {len}");
    }
}
