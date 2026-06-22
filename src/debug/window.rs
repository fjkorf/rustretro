use bevy_egui::egui;
use std::sync::{Arc, Mutex};

use crate::audio::AudioOutput;
use crate::debug::DebugState;
use crate::debug::panels::{
    frame_inspector::FrameInspector,
    hex_dump::HexDump,
    input_monitor::InputMonitor,
    tile_viewer::TileViewer,
    frame_log::FrameLog,
    triggers::Triggers,
    cpu_state::CpuState,
    audio_controls::AudioControls,
    disassembly::Disassembly,
    regions::RegionsPanel,
    watch::WatchPanel,
    ram_search::RamSearchPanel,
    vdp_registers::VdpRegisters,
    help::HelpPanel,
};

#[derive(PartialEq, Clone, Copy)]
enum Tab { FrameInspector, HexDump, TileViewer, InputMonitor, FrameLog, Triggers, CpuState, Audio, Disasm, Regions, Watch, RamSearch, VdpRegisters, Help }

pub struct DebugApp {
    state: Arc<Mutex<DebugState>>,
    audio: Option<Arc<Mutex<AudioOutput>>>,
    active_tab: Tab,
    frame_inspector: FrameInspector,
    hex_dump: HexDump,
    input_monitor: InputMonitor,
    tile_viewer: TileViewer,
    frame_log: FrameLog,
    triggers: Triggers,
    cpu_state: CpuState,
    audio_controls: AudioControls,
    disassembly: Disassembly,
    regions_panel: RegionsPanel,
    watch_panel: WatchPanel,
    ram_search_panel: RamSearchPanel,
    vdp_registers: VdpRegisters,
    help_panel: HelpPanel,
}

impl DebugApp {
    pub fn new(state: Arc<Mutex<DebugState>>) -> Self {
        DebugApp {
            state,
            audio: None,
            active_tab: Tab::FrameInspector,
            frame_inspector: FrameInspector::new(),
            hex_dump: HexDump::new(),
            input_monitor: InputMonitor::new(),
            tile_viewer: TileViewer::new(),
            frame_log: FrameLog::new(),
            triggers: Triggers::new(),
            cpu_state: CpuState::new(),
            audio_controls: AudioControls,
            disassembly: Disassembly::new(),
            regions_panel: RegionsPanel::new(),
            watch_panel: WatchPanel::new(),
            ram_search_panel: RamSearchPanel::new(),
            vdp_registers: VdpRegisters::new(),
            help_panel: HelpPanel::new(),
        }
    }

    pub fn set_audio(&mut self, audio: Arc<Mutex<AudioOutput>>) {
        self.audio = Some(audio);
    }

    /// Render the debug overlay into the given egui context.
    /// Call this from a Bevy system that holds `EguiContexts`.
    pub fn show(&mut self, ctx: &egui::Context) {
        let state_snapshot = self.state.lock().ok().map(|s| (
            s.frame_count, s.video_frames, s.video_real,
            s.fps, s.fb_width, s.fb_height, s.fb_fmt, s.paused,
        ));

        egui::TopBottomPanel::top("debug_tab_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🎮 RustRetro Debugger");
                ui.separator();
                ui.selectable_value(&mut self.active_tab, Tab::FrameInspector, "🖼 Frame");
                ui.selectable_value(&mut self.active_tab, Tab::HexDump,        "📋 Hex");
                ui.selectable_value(&mut self.active_tab, Tab::TileViewer,     "🧩 Tiles");
                ui.selectable_value(&mut self.active_tab, Tab::InputMonitor,   "🕹 Input");
                ui.selectable_value(&mut self.active_tab, Tab::CpuState,       "🔧 CPU");
                ui.selectable_value(&mut self.active_tab, Tab::Disasm,         "📜 Disasm");
                ui.selectable_value(&mut self.active_tab, Tab::Audio,          "🔊 Audio");
                ui.selectable_value(&mut self.active_tab, Tab::FrameLog,       "📜 Log");
                ui.selectable_value(&mut self.active_tab, Tab::Triggers,       "⏸ Triggers");
                ui.selectable_value(&mut self.active_tab, Tab::Regions,        "🗺 Regions");
                ui.selectable_value(&mut self.active_tab, Tab::Watch,          "👁 Watch");
                ui.selectable_value(&mut self.active_tab, Tab::RamSearch,      "🔍 Search");
                ui.selectable_value(&mut self.active_tab, Tab::VdpRegisters,   "📺 VDP");
                ui.selectable_value(&mut self.active_tab, Tab::Help,           "❓ Help");

                if let Some((fc, vf, vr, fps, w, h, fmt, paused)) = state_snapshot {
                    ui.separator();
                    ui.label(egui::RichText::new(if paused { "⏸ PAUSED" } else { "▶ running" })
                        .color(if paused { egui::Color32::YELLOW } else { egui::Color32::GREEN }));
                    ui.separator();
                    ui.label(format!("run:{fc} vid:{vf} real:{vr} | {w}×{h} fmt={fmt} @ {fps:.1}fps"));
                }
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            match self.active_tab {
                Tab::FrameInspector => self.frame_inspector.show(ui, ctx, &self.state),
                Tab::HexDump        => self.hex_dump.show(ui, &self.state),
                Tab::TileViewer     => self.tile_viewer.show(ui, ctx, &self.state),
                Tab::InputMonitor   => self.input_monitor.show(ui, &self.state),
                Tab::CpuState       => self.cpu_state.show(ui, &self.state),
                Tab::FrameLog       => self.frame_log.show(ui, &self.state),
                Tab::Triggers       => self.triggers.show(ui, &self.state),
                Tab::Disasm => {
                    if let Ok(mut ds) = self.state.lock() {
                        self.disassembly.show(ui, &mut ds);
                    } else {
                        ui.label("Error: Could not acquire debug state lock");
                    }
                }
                Tab::Regions => {
                    if let Ok(mut ds) = self.state.lock() {
                        self.regions_panel.show(ui, ctx, &mut ds);
                    } else {
                        ui.label("Error: Could not acquire debug state lock");
                    }
                }
                Tab::Watch => {
                    if let Ok(mut ds) = self.state.lock() {
                        self.watch_panel.show(ui, &mut ds);
                    } else {
                        ui.label("Error: Could not acquire debug state lock");
                    }
                }
                Tab::RamSearch => {
                    if let Ok(mut ds) = self.state.lock() {
                        self.ram_search_panel.show(ui, &mut ds);
                    } else {
                        ui.label("Error: Could not acquire debug state lock");
                    }
                }
                Tab::VdpRegisters => {
                    if let Ok(ds) = self.state.lock() {
                        self.vdp_registers.show(ui, &ds.vdp_regs);
                    } else {
                        ui.label("Error: Could not acquire debug state lock");
                    }
                }
                Tab::Help => self.help_panel.show(ui),
                Tab::Audio => {
                    if let Some(ref audio_ref) = self.audio {
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
        });
    }
}
