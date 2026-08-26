fn main() {
    // Varargs bridge for the libretro log interface (see src/log_shim.c and
    // src/core_log.rs). One tiny C file, compiled once and cached — no
    // measurable impact on the incremental release-dev loop.
    println!("cargo:rerun-if-changed=src/log_shim.c");
    cc::Build::new().file("src/log_shim.c").compile("log_shim");
}
