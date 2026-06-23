//! egui_dock-based docking workspace for the RustRetro debugger.
//!
//! This replaces the flat tab bar with a draggable, splittable dock so that
//! multiple panels (Disasm + CPU + Watch/Regions + a bottom strip) are visible
//! at once. The Lua script panel is intentionally NOT a tab here — it remains a
//! separate floating egui::Window driven by a Bevy system in main.rs.
//!
//! Design note — the `show()` signature bridge:
//! The existing panels have several distinct `show()` shapes:
//!   * `&mut self, ui, &Arc<Mutex<DebugState>>`        (cpu, hex, triggers, frame_log, input)
//!   * `&mut self, ui, ctx, &Arc<Mutex<DebugState>>`   (frame_inspector, tile_viewer)
//!   * `&mut self, ui, ctx, &mut DebugState`           (regions)
//!   * `&mut self, ui, &mut DebugState`                (watch, ram_search)
//!   * `&mut self, ui, &[u8; 24]`                      (vdp_registers, reads ds.vdp_regs)
//!   * `&mut self, ui`                                 (help)
//!   * assoc fn  `ui, &mut DebugState`                 (disassembly)
//!   * assoc fn  `ui, &mut AudioOutput`                (audio_controls)
//!
//! `egui_dock::TabViewer::ui` only hands us `&mut self` and `&mut Ui`. We get
//! `ctx` for free via `ui.ctx()` (cheap Arc clone). The panels and the shared
//! `DebugState` are borrowed mutably by a transient `DockViewer<'a>` built each
//! frame, so the borrow lives only for the single `DockArea::show` call.

use bevy_egui::egui;
use std::sync::{Arc, Mutex};

use crate::audio::AudioOutput;
use crate::debug::DebugState;
use crate::debug::panels::{
    audio_controls::AudioControls,
    cpu_state::CpuState,
    disassembly::Disassembly,
    frame_inspector::FrameInspector,
    frame_log::FrameLog,
    help::HelpPanel,
    hex_dump::HexDump,
    input_monitor::InputMonitor,
    ram_search::RamSearchPanel,
    regions::RegionsPanel,
    tile_viewer::TileViewer,
    triggers::Triggers,
    vdp_registers::VdpRegisters,
    watch::WatchPanel,
};

use egui_dock::{DockArea, DockState, NodeIndex, Style};

/// Identity of a dockable tab. Cheap `Copy`, matched inside the `TabViewer`.
/// `Serialize`/`Deserialize` let the whole `DockState<Tab>` round-trip to disk
/// for layout persistence.
#[derive(
    Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize,
)]
pub enum Tab {
    FrameInspector,
    HexDump,
    TileViewer,
    InputMonitor,
    FrameLog,
    Triggers,
    CpuState,
    Audio,
    Disasm,
    Regions,
    Watch,
    RamSearch,
    VdpRegisters,
    Help,
}

impl Tab {
    fn title(self) -> &'static str {
        match self {
            Tab::FrameInspector => "🖼 Frame",
            Tab::HexDump => "📋 Hex",
            Tab::TileViewer => "🧩 Tiles",
            Tab::InputMonitor => "🕹 Input",
            Tab::FrameLog => "🧾 Log",
            Tab::Triggers => "⏸ Triggers",
            Tab::CpuState => "🔧 CPU",
            Tab::Audio => "🔊 Audio",
            Tab::Disasm => "📜 Disasm",
            Tab::Regions => "🗺 Regions",
            Tab::Watch => "👁 Watch",
            Tab::RamSearch => "🔍 Search",
            Tab::VdpRegisters => "📺 VDP",
            Tab::Help => "❓ Help",
        }
    }
}

/// The panel structs that own per-panel UI state, grouped so the borrow of the
/// `DockState` (held elsewhere in `DebugApp`) can be split from the panels.
pub struct Panels {
    pub frame_inspector: FrameInspector,
    pub hex_dump: HexDump,
    pub input_monitor: InputMonitor,
    pub tile_viewer: TileViewer,
    pub frame_log: FrameLog,
    pub triggers: Triggers,
    pub cpu_state: CpuState,
    pub disassembly: Disassembly,
    pub regions_panel: RegionsPanel,
    pub watch_panel: WatchPanel,
    pub ram_search_panel: RamSearchPanel,
    pub vdp_registers: VdpRegisters,
    pub help_panel: HelpPanel,
}

impl Panels {
    pub fn new() -> Self {
        Panels {
            frame_inspector: FrameInspector::new(),
            hex_dump: HexDump::new(),
            input_monitor: InputMonitor::new(),
            tile_viewer: TileViewer::new(),
            frame_log: FrameLog::new(),
            triggers: Triggers::new(),
            cpu_state: CpuState::new(),
            disassembly: Disassembly::new(),
            regions_panel: RegionsPanel::new(),
            watch_panel: WatchPanel::new(),
            ram_search_panel: RamSearchPanel::new(),
            vdp_registers: VdpRegisters::new(),
            help_panel: HelpPanel::new(),
        }
    }
}

/// Transient bridge built each frame. Holds mutable refs to the panels and the
/// shared state for exactly the duration of one `DockArea::show` call.
struct DockViewer<'a> {
    panels: &'a mut Panels,
    state: &'a Arc<Mutex<DebugState>>,
    audio: &'a Option<Arc<Mutex<AudioOutput>>>,
}

impl<'a> egui_dock::TabViewer for DockViewer<'a> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Self::Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn id(&mut self, tab: &mut Self::Tab) -> egui::Id {
        egui::Id::new(*tab)
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Self::Tab) {
        // ctx for the texture-uploading panels comes free from the Ui.
        let ctx = ui.ctx().clone();
        match *tab {
            // shape: &mut self, ui, ctx, &Arc<Mutex<DebugState>>
            Tab::FrameInspector => self.panels.frame_inspector.show(ui, &ctx, self.state),
            Tab::TileViewer => self.panels.tile_viewer.show(ui, &ctx, self.state),

            // shape: &mut self, ui, &Arc<Mutex<DebugState>>
            Tab::HexDump => self.panels.hex_dump.show(ui, self.state),
            Tab::InputMonitor => self.panels.input_monitor.show(ui, self.state),
            Tab::FrameLog => self.panels.frame_log.show(ui, self.state),
            Tab::Triggers => self.panels.triggers.show(ui, self.state),
            Tab::CpuState => self.panels.cpu_state.show(ui, self.state),

            // shape: &mut self, ui, ctx, &mut DebugState  (needs the lock)
            Tab::Regions => {
                if let Ok(mut ds) = self.state.lock() {
                    self.panels.regions_panel.show(ui, &ctx, &mut ds);
                } else {
                    ui.label("Error: Could not acquire debug state lock");
                }
            }

            // shape: &mut self, ui, &mut DebugState  (needs the lock)
            Tab::Watch => {
                if let Ok(mut ds) = self.state.lock() {
                    self.panels.watch_panel.show(ui, &mut ds);
                } else {
                    ui.label("Error: Could not acquire debug state lock");
                }
            }
            Tab::RamSearch => {
                if let Ok(mut ds) = self.state.lock() {
                    self.panels.ram_search_panel.show(ui, &mut ds);
                } else {
                    ui.label("Error: Could not acquire debug state lock");
                }
            }

            // shape: &mut self, ui, &[u8; 24]  (lock only to read vdp_regs)
            Tab::VdpRegisters => {
                if let Ok(ds) = self.state.lock() {
                    self.panels.vdp_registers.show(ui, &ds.vdp_regs);
                } else {
                    ui.label("Error: Could not acquire debug state lock");
                }
            }

            // shape: &mut self, ui  (no state)
            Tab::Help => self.panels.help_panel.show(ui),

            // shape: &mut self, ui, &mut DebugState
            Tab::Disasm => {
                if let Ok(mut ds) = self.state.lock() {
                    self.panels.disassembly.show(ui, &mut ds);
                } else {
                    ui.label("Error: Could not acquire debug state lock");
                }
            }

            // shape: assoc fn, ui, &mut AudioOutput
            Tab::Audio => {
                if let Some(audio_ref) = self.audio {
                    if let Ok(mut audio) = audio_ref.lock() {
                        AudioControls::show(ui, &mut audio);
                    } else {
                        ui.label("Error: Could not acquire audio lock");
                    }
                } else {
                    ui.label("Audio not available");
                }
            }
        }
    }
}

/// Build the default workspace layout proving simultaneous multi-panel
/// visibility:
///
/// ```text
/// +-------------------------------------------------------+
/// |  toolbar (TopBottomPanel, rendered separately)        |
/// +----------------------------+--------------------------+
/// |                            |   CPU                    |
/// |        Disasm (central)    +--------------------------+
/// |                            |   Watch / Regions (tabs) |
/// +----------------------------+--------------------------+
/// |  Hex | Frame | Tiles | Input | Log | Trig | Audio |   |
/// |  RamSearch | VDP | Help            (bottom, tabbed)   |
/// +-------------------------------------------------------+
/// ```
pub fn default_layout() -> DockState<Tab> {
    // Central node starts with Disasm.
    let mut dock = DockState::new(vec![Tab::Disasm]);

    let surface = dock.main_surface_mut();

    // Split right: CPU above, Watch+Regions (tabbed) below.
    let [_central, right] =
        surface.split_right(NodeIndex::root(), 0.62, vec![Tab::CpuState]);
    let [_cpu, _watch_regions] =
        surface.split_below(right, 0.5, vec![Tab::Watch, Tab::Regions]);

    // Split the central area's bottom into a tabbed strip of the remaining panels.
    let [_central2, _bottom] = surface.split_below(
        NodeIndex::root(),
        0.66,
        vec![
            Tab::HexDump,
            Tab::FrameInspector,
            Tab::TileViewer,
            Tab::InputMonitor,
            Tab::FrameLog,
            Tab::Triggers,
            Tab::Audio,
            Tab::RamSearch,
            Tab::VdpRegisters,
            Tab::Help,
        ],
    );

    dock
}

/// Path of the layout sidecar file. Kept simple for v1: a fixed name in the
/// current working directory (next to where the regions sidecar lives by
/// default). Saved/loaded via the Save/Reset layout toolbar buttons.
pub const LAYOUT_PATH: &str = "rustretro_layout.json";

/// Load a previously saved dock layout from [`LAYOUT_PATH`], falling back to
/// [`default_layout`] if the file is absent or fails to parse.
pub fn load_layout() -> DockState<Tab> {
    match std::fs::read_to_string(LAYOUT_PATH) {
        Ok(json) => match serde_json::from_str::<DockState<Tab>>(&json) {
            Ok(state) => state,
            Err(e) => {
                eprintln!("[dock] failed to parse {LAYOUT_PATH}: {e}; using default layout");
                default_layout()
            }
        },
        Err(_) => default_layout(),
    }
}

/// Persist the current dock layout to [`LAYOUT_PATH`]. Errors are logged, not
/// fatal (a debugger that can't write its layout sidecar should keep running).
pub fn save_layout(dock_state: &DockState<Tab>) {
    match serde_json::to_string_pretty(dock_state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(LAYOUT_PATH, json) {
                eprintln!("[dock] failed to write {LAYOUT_PATH}: {e}");
            }
        }
        Err(e) => eprintln!("[dock] failed to serialize layout: {e}"),
    }
}

/// Render the dock workspace. Called from `DebugApp::show` *inside* a
/// `CentralPanel` (so it sits below the persistent toolbar).
pub fn show_dock(
    ui: &mut egui::Ui,
    dock_state: &mut DockState<Tab>,
    panels: &mut Panels,
    state: &Arc<Mutex<DebugState>>,
    audio: &Option<Arc<Mutex<AudioOutput>>>,
) {
    let mut viewer = DockViewer { panels, state, audio };
    DockArea::new(dock_state)
        .style(Style::from_egui(ui.style().as_ref()))
        .show_inside(ui, &mut viewer);
}
