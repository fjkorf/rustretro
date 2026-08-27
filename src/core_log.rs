//! Core log sink: receives already-formatted lines from the C shim
//! (`src/log_shim.c`) and prints them with the `[CORE ...]` prefix,
//! rate-limited so a chatty core (mame2003_plus once wrote 953 MB in ten
//! minutes) cannot flood stderr.
//!
//! The libretro log interface hands the core a C-variadic `retro_log_printf_t`.
//! Stable Rust cannot define one, so `rr_core_log` (the variadic entry point
//! the core actually calls) lives in the C shim; it vsnprintf's into a
//! 1024-byte buffer and calls `rr_core_log_sink` here with the formatted text.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Lines admitted per one-second window before further lines are dropped.
const MAX_LINES_PER_SEC: u32 = 200;

extern "C" {
    /// The variadic entry point defined in `src/log_shim.c`. Its address is
    /// what we hand to the core via RETRO_ENVIRONMENT_GET_LOG_INTERFACE.
    pub fn rr_core_log(level: u32, fmt: *const c_char, ...);
}

/// Decision for one incoming log line.
#[derive(Debug, PartialEq, Eq)]
pub enum LogGate {
    /// Print the line.
    Emit,
    /// Print "[core log] suppressed N lines" first, then the line.
    EmitWithSummary(u64),
    /// Drop the line silently (counted for a later summary).
    Drop,
}

/// Pure per-second rate limiter: at most `MAX_LINES_PER_SEC` lines per
/// one-second window; excess lines are dropped and tallied, and the tally is
/// reported as a single summary line when the next window opens.
pub struct RateLimiter {
    window_start: Instant,
    count: u32,
    suppressed: u64,
}

impl RateLimiter {
    pub fn new(now: Instant) -> Self {
        RateLimiter { window_start: now, count: 0, suppressed: 0 }
    }

    pub fn admit(&mut self, now: Instant) -> LogGate {
        if now.duration_since(self.window_start) >= Duration::from_secs(1) {
            let suppressed = self.suppressed;
            self.window_start = now;
            self.count = 1;
            self.suppressed = 0;
            return if suppressed > 0 {
                LogGate::EmitWithSummary(suppressed)
            } else {
                LogGate::Emit
            };
        }
        self.count += 1;
        if self.count <= MAX_LINES_PER_SEC {
            LogGate::Emit
        } else {
            self.suppressed += 1;
            LogGate::Drop
        }
    }
}

static LIMITER: Mutex<Option<RateLimiter>> = Mutex::new(None);

/// Called by the C shim with the formatted line. `truncated` is nonzero when
/// the line exceeded the shim's buffer and was cut short.
#[no_mangle]
pub extern "C" fn rr_core_log_sink(level: u32, msg: *const c_char, truncated: c_int) {
    if msg.is_null() {
        return;
    }
    let now = Instant::now();
    let gate = {
        let mut guard = LIMITER.lock().unwrap();
        guard.get_or_insert_with(|| RateLimiter::new(now)).admit(now)
    };
    if gate == LogGate::Drop {
        return;
    }
    if let LogGate::EmitWithSummary(n) = gate {
        eprintln!("[core log] suppressed {} lines", n);
    }
    let prefix = match level {
        0 => "[CORE DBG]",
        1 => "[CORE INF]",
        2 => "[CORE WRN]",
        _ => "[CORE ERR]",
    };
    let s = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    let ellipsis = if truncated != 0 { "…" } else { "" };
    eprintln!("{} {}{}", prefix, s.trim_end(), ellipsis);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn quiet_core_all_lines_emit() {
        let base = Instant::now();
        let mut rl = RateLimiter::new(base);
        for i in 0..50 {
            assert_eq!(rl.admit(t(base, i * 10)), LogGate::Emit);
        }
    }

    #[test]
    fn drops_after_200_in_one_second() {
        let base = Instant::now();
        let mut rl = RateLimiter::new(base);
        for _ in 0..200 {
            assert_eq!(rl.admit(base), LogGate::Emit);
        }
        for _ in 0..500 {
            assert_eq!(rl.admit(base), LogGate::Drop);
        }
    }

    #[test]
    fn summary_emitted_when_window_rolls() {
        let base = Instant::now();
        let mut rl = RateLimiter::new(base);
        for _ in 0..200 {
            assert_eq!(rl.admit(base), LogGate::Emit);
        }
        for _ in 0..37 {
            assert_eq!(rl.admit(base), LogGate::Drop);
        }
        // Next second: summary of the 37 dropped lines, then normal flow.
        assert_eq!(rl.admit(t(base, 1000)), LogGate::EmitWithSummary(37));
        assert_eq!(rl.admit(t(base, 1001)), LogGate::Emit);
    }

    #[test]
    fn window_roll_without_suppression_is_plain_emit() {
        let base = Instant::now();
        let mut rl = RateLimiter::new(base);
        assert_eq!(rl.admit(base), LogGate::Emit);
        assert_eq!(rl.admit(t(base, 1500)), LogGate::Emit);
    }

    #[test]
    fn each_window_gets_fresh_budget_and_summary() {
        let base = Instant::now();
        let mut rl = RateLimiter::new(base);
        // Window 1: fill budget, suppress 10.
        for _ in 0..200 {
            rl.admit(base);
        }
        for _ in 0..10 {
            assert_eq!(rl.admit(base), LogGate::Drop);
        }
        // Window 2: summary, then a fresh 200 budget (1 used by the summary
        // line's own admit), then suppression counts anew.
        assert_eq!(rl.admit(t(base, 1000)), LogGate::EmitWithSummary(10));
        for _ in 0..199 {
            assert_eq!(rl.admit(t(base, 1000)), LogGate::Emit);
        }
        assert_eq!(rl.admit(t(base, 1000)), LogGate::Drop);
        assert_eq!(rl.admit(t(base, 2000)), LogGate::EmitWithSummary(1));
    }
}
