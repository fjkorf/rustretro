/* log_shim.c — varargs bridge for the libretro log interface.
 *
 * The core is handed retro_log_printf_t, a C-variadic function. Stable Rust
 * cannot *define* a variadic extern fn (c_variadic is nightly-only), so this
 * tiny shim receives the varargs, vsnprintf's them into a fixed buffer
 * (truncating), and calls back into Rust with the already-formatted string.
 * The Rust side (rr_core_log_sink in src/core_log.rs) owns prefixing and
 * rate limiting.
 */

#include <stdarg.h>
#include <stdio.h>

/* Defined in Rust (src/core_log.rs). `truncated` is nonzero when the
 * formatted output did not fit in the buffer. */
extern void rr_core_log_sink(unsigned int level, const char *msg, int truncated);

#define RR_LOG_BUF_SIZE 1024

void rr_core_log(unsigned int level, const char *fmt, ...) {
    char buf[RR_LOG_BUF_SIZE];
    va_list ap;
    int n;

    if (!fmt) return;

    va_start(ap, fmt);
    n = vsnprintf(buf, sizeof(buf), fmt, ap);
    va_end(ap);

    if (n < 0) return; /* encoding error — nothing sane to print */

    rr_core_log_sink(level, buf, n >= (int)sizeof(buf));
}
