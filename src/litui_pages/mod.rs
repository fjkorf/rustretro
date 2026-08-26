//! litui Wave C — a parallel preview surface that renders three debugger screens
//! (CPU, Log, Audio) as pure-litui Markdown pages, alongside the existing
//! egui_dock debugger.
//!
//! ## What this proves
//! (The Wave-C live-preview app (`LituiPages`, F9) was removed in the cleanup
//! pass — the dock panels are the real UI; tutorials below remain.)
//! Historical note: a Bevy `Resource` (wrapping
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

// ─── Wave D: tutorials as in-app litui pages (Help → Tutorials) ──────────────
//
// The 18 task-oriented tutorial pages in `docs/tutorials/` are already authored
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
        "docs/tutorials/input-and-triggers.md",
        "docs/tutorials/audio.md",
        "docs/tutorials/lua-scripting.md",
        "docs/tutorials/training-mode.md",
        "docs/tutorials/shadow-loop.md",
        "docs/tutorials/matchup-grid.md",
        "docs/tutorials/porting-a-game.md",
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
