//! litui Wave C — a parallel preview surface that renders three debugger screens
//! (CPU, Log, Audio) as pure-litui Markdown pages, alongside the existing
//! egui_dock debugger.
//!
//! ## What this proves
//! Live-resource binding in the Bevy path: a Bevy `Resource` (`LituiPages`, wrapping
//! the macro-generated `MdApp`) whose `AppState` fields are overwritten from the
//! shared `DebugState` every frame, and whose form widgets (Audio mute/volume) are
//! read back into the live `AudioOutput`. This is the "values down, widget outputs up"
//! projection contract — one `sync_litui_pages` system per frame.
//!
//! ## How it coexists with the dock
//! Each page declares `panel: window` in its frontmatter, so `MdApp::show_all` paints
//! a top nav bar plus one floating egui `Window` for the selected page. That composes
//! cleanly over the dock's CentralPanel instead of fighting it. The whole surface is
//! gated behind an `open` flag toggled by F9, so when it is closed nothing renders and
//! existing behaviour is unchanged.

use bevy::prelude::*;

/// The litui-generated app. `define_markdown_app!` resolves the `.md` paths relative
/// to `CARGO_MANIFEST_DIR` (the crate root) and expands to `Page`, `AppState`, the
/// per-page `render_*` functions and the `MdApp` struct with `show_all()`.
pub mod pages {
    use bevy_egui::egui;
    use litui::*;

    define_markdown_app! {
        parent: "src/litui_pages/content/_app.md",
        "src/litui_pages/content/cpu.md",
        "src/litui_pages/content/log.md",
        "src/litui_pages/content/audio.md",
    }
}

pub use pages::MdApp;

/// Bevy `Resource` wrapper around the generated `MdApp` plus the F9 visibility flag.
#[derive(Resource)]
pub struct LituiPages {
    pub md: MdApp,
    pub open: bool,
}

impl Default for LituiPages {
    fn default() -> Self {
        let mut md = MdApp::default();
        // Initialise the volume slider to AudioOutput's default (1.0) so the
        // first-frame "widget output up" sync doesn't silence playback.
        md.state.volume = 1.0;
        Self { md, open: false }
    }
}
