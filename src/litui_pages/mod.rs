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

// ─── Wave D: tutorials as in-app litui pages (Help → Tutorials) ──────────────
//
// The 14 task-oriented tutorial pages in `docs/tutorials/` are already authored
// in litui dialect (YAML `page:` frontmatter). This mounts them as a SECOND,
// read-only litui app — no live binding needed, they are static document pages.
// Shares the `_tutorials.md` parent for common styles. Gated by F8.

/// The litui app generated from the tutorial Markdown pages. Excludes the index
/// `README.md` (the GitHub index, not a mountable page) and `_tutorials.md`
/// (the parent). Exactly one page (`getting-started`) is `default: true`.
pub mod tutorials {
    use bevy_egui::egui;
    use litui::*;

    define_markdown_app! {
        parent: "docs/tutorials/_tutorials.md",
        "docs/tutorials/getting-started.md",
        "docs/tutorials/docking-workspace.md",
        "docs/tutorials/watch-and-freeze.md",
        "docs/tutorials/ram-search.md",
        "docs/tutorials/tracking-changes.md",
        "docs/tutorials/hex-dump.md",
        "docs/tutorials/disassembly-and-breakpoints.md",
        "docs/tutorials/regions-heatmap-bookmarks.md",
        "docs/tutorials/cpu-registers.md",
        "docs/tutorials/tiles-and-frames.md",
        "docs/tutorials/vdp-registers.md",
        "docs/tutorials/input-and-triggers.md",
        "docs/tutorials/audio.md",
        "docs/tutorials/lua-scripting.md",
    }
}

/// Bevy `Resource` wrapping the tutorials litui app plus its F8 visibility flag.
#[derive(Resource)]
pub struct TutorialPages {
    pub md: tutorials::MdApp,
    pub open: bool,
}

impl Default for TutorialPages {
    fn default() -> Self {
        Self { md: tutorials::MdApp::default(), open: false }
    }
}
