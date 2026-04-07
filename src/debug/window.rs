use eframe::egui;
use std::sync::{Arc, Mutex};

use crate::debug::DebugState;
use crate::debug::panels::{
    frame_inspector::FrameInspector,
    hex_dump::HexDump,
    input_monitor::InputMonitor,
    tile_viewer::TileViewer,
    frame_log::FrameLog,
    triggers::Triggers,
};

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    FrameInspector,
    HexDump,
    TileViewer,
    InputMonitor,
    FrameLog,
    Triggers,
}

pub struct DebugApp {
    state: Arc<Mutex<DebugState>>,
    active_tab: Tab,
    frame_inspector: FrameInspector,
    hex_dump: HexDump,
    input_monitor: InputMonitor,
    tile_viewer: TileViewer,
    frame_log: FrameLog,
    triggers: Triggers,
}

impl DebugApp {
    pub fn new(state: Arc<Mutex<DebugState>>, _cc: &eframe::CreationContext<'_>) -> Self {
        DebugApp {
            state,
            active_tab: Tab::FrameInspector,
            frame_inspector: FrameInspector::new(),
            hex_dump: HexDump::new(),
            input_monitor: InputMonitor::new(),
            tile_viewer: TileViewer::new(),
            frame_log: FrameLog::new(),
            triggers: Triggers::new(),
        }
    }
}

impl eframe::App for DebugApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint();

        let state_snapshot = {
            match self.state.lock() {
                Ok(s) => Some((
                    s.frame_count, s.video_frames, s.video_real,
                    s.fps, s.fb_width, s.fb_height, s.fb_fmt, s.paused,
                )),
                Err(_) => None,
            }
        };

        egui::TopBottomPanel::top("tab_bar").show(&ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("🎮 RustRetro Debugger");
                ui.separator();
                ui.selectable_value(&mut self.active_tab, Tab::FrameInspector, "🖼 Frame");
                ui.selectable_value(&mut self.active_tab, Tab::HexDump,        "📋 Hex");
                ui.selectable_value(&mut self.active_tab, Tab::TileViewer,     "🧩 Tiles");
                ui.selectable_value(&mut self.active_tab, Tab::InputMonitor,   "🕹 Input");
                ui.selectable_value(&mut self.active_tab, Tab::FrameLog,       "📜 Log");
                ui.selectable_value(&mut self.active_tab, Tab::Triggers,       "⏸ Triggers");

                if let Some((fc, vf, vr, fps, w, h, fmt, paused)) = state_snapshot {
                    ui.separator();
                    let status = if paused { "⏸ PAUSED" } else { "▶ running" };
                    ui.label(egui::RichText::new(status).color(
                        if paused { egui::Color32::YELLOW } else { egui::Color32::GREEN }
                    ));
                    ui.separator();
                    ui.label(format!("run:{fc} vid:{vf} real:{vr} | {w}×{h} fmt={fmt} @ {fps:.1}fps"));
                }
            });
        });

        egui::CentralPanel::default().show(&ctx, |ui| {
            match self.active_tab {
                Tab::FrameInspector => self.frame_inspector.show(ui, &ctx, &self.state),
                Tab::HexDump       => self.hex_dump.show(ui, &self.state),
                Tab::TileViewer    => self.tile_viewer.show(ui, &ctx, &self.state),
                Tab::InputMonitor  => self.input_monitor.show(ui, &self.state),
                Tab::FrameLog      => self.frame_log.show(ui, &self.state),
                Tab::Triggers      => self.triggers.show(ui, &self.state),
            }
        });
    }
}

/// Spawn the debug window in a background thread. Returns immediately.
pub fn spawn(state: Arc<Mutex<DebugState>>) {
    std::thread::spawn(move || {
        let options = eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_title("RustRetro Debugger")
                .with_inner_size([1100.0, 700.0])
                .with_min_inner_size([800.0, 500.0]),
            ..Default::default()
        };
        let _ = eframe::run_native(
            "RustRetro Debugger",
            options,
            Box::new(|cc| Ok(Box::new(DebugApp::new(state, cc)))),
        );
    });
}
