use bevy_egui::egui;
use crate::debug::{DebugState, Watch, WatchFormat};

pub struct WatchPanel {
    add_addr: String,
    add_label: String,
    add_format: WatchFormat,
}

impl WatchPanel {
    pub fn new() -> Self {
        WatchPanel {
            add_addr: String::new(),
            add_label: String::new(),
            add_format: WatchFormat::Hex8,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.vertical(|ui| {
            ui.heading("👁 Watch");
            ui.separator();

            // ── Add form ──────────────────────────────────────────────
            ui.horizontal(|ui| {
                ui.label("Address:");
                ui.add(egui::TextEdit::singleline(&mut self.add_addr)
                    .desired_width(90.0)
                    .hint_text("0x..."));
                ui.label("Label:");
                ui.add(egui::TextEdit::singleline(&mut self.add_label)
                    .desired_width(140.0)
                    .hint_text("e.g. lives"));
                ui.label("Format:");
                egui::ComboBox::from_id_salt("watch_add_format")
                    .selected_text(format_name(self.add_format))
                    .show_ui(ui, |ui| {
                        for fmt in ALL_FORMATS {
                            ui.selectable_value(&mut self.add_format, fmt, format_name(fmt));
                        }
                    });
                if ui.button("➕ Add Watch").clicked() {
                    if let Ok(addr) = usize::from_str_radix(
                        self.add_addr.trim_start_matches("0x").trim(),
                        16,
                    ) {
                        let label = if self.add_label.trim().is_empty() {
                            format!("{:06X}", addr)
                        } else {
                            self.add_label.clone()
                        };
                        state.watches.push(Watch {
                            addr,
                            label,
                            format: self.add_format,
                            frozen: false,
                            frozen_value: None,
                            track_changes: false,
                            current: None,
                            prev_value: None,
                        });
                        self.add_addr.clear();
                        self.add_label.clear();
                    }
                }
            });

            ui.separator();

            if state.watches.is_empty() {
                ui.label(egui::RichText::new("No watches. Add one above.")
                    .color(egui::Color32::DARK_GRAY));
                return;
            }

            let mut remove_idx: Option<usize> = None;
            // Collect goto target into a local to avoid a second mut-borrow of state
            // while iterating over state.watches.
            let mut goto_addr: Option<u32> = None;

            egui::ScrollArea::vertical().show(ui, |ui| {
                egui::Grid::new("watch_grid")
                    .num_columns(7)
                    .spacing([12.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        // Header
                        ui.label(egui::RichText::new("Address").strong());
                        ui.label(egui::RichText::new("Label").strong());
                        ui.label(egui::RichText::new("Value").strong());
                        ui.label(egui::RichText::new("Freeze").strong());
                        ui.label(egui::RichText::new("🔍 Track").strong())
                            .on_hover_text(
                                "Log every frame this value changes, with the PC \
                                 running that frame. Frame-granular: the actual \
                                 write happened sometime during that frame, not \
                                 necessarily at this exact instruction.");
                        ui.label(egui::RichText::new("").strong()); // goto
                        ui.label(egui::RichText::new("").strong()); // remove
                        ui.end_row();

                        for (i, watch) in state.watches.iter_mut().enumerate() {
                            // Address
                            ui.label(egui::RichText::new(format!("{:06X}", watch.addr))
                                .monospace()
                                .color(egui::Color32::from_rgb(150, 150, 200)));

                            // Label (editable)
                            ui.add(egui::TextEdit::singleline(&mut watch.label)
                                .desired_width(140.0));

                            // Value
                            let value_text = format_value(watch.format, watch.current);
                            ui.label(egui::RichText::new(value_text)
                                .monospace()
                                .color(egui::Color32::from_rgb(150, 220, 150)));

                            // Freeze checkbox
                            let resp = ui.checkbox(&mut watch.frozen, "");
                            if resp.changed() && !watch.frozen {
                                watch.frozen_value = None;
                            }

                            // Track-changes checkbox
                            ui.checkbox(&mut watch.track_changes, "");

                            // Navigate — jump Disasm/Hex to this watch address.
                            if ui.small_button("→")
                                .on_hover_text("Navigate Disasm/Hex to this address")
                                .clicked()
                            {
                                goto_addr = Some(watch.addr as u32);
                            }

                            // Remove
                            if ui.small_button("✕").clicked() {
                                remove_idx = Some(i);
                            }

                            ui.end_row();
                        }
                    });
            });

            if let Some(i) = remove_idx {
                state.watches.remove(i);
            }
            // Apply goto after the iter_mut borrow of state.watches has ended.
            if let Some(addr) = goto_addr {
                state.goto(addr);
            }

            ui.separator();
            change_log_section(ui, state);
        });
    }
}

/// Collapsible "Change Log" showing recent tracked-watch value changes,
/// newest-first. Frame-granular: the change happened during the listed frame.
fn change_log_section(ui: &mut egui::Ui, state: &mut DebugState) {
    egui::CollapsingHeader::new(format!("🔍 Change Log ({})", state.change_log.len()))
        .default_open(false)
        .show(ui, |ui| {
            ui.label(egui::RichText::new(
                "What changed this address? — frame-granular: the write ran \
                 sometime during the listed frame, not necessarily at this PC.")
                .small()
                .color(egui::Color32::DARK_GRAY));

            ui.horizontal(|ui| {
                if ui.button("🗑 Clear").clicked() {
                    state.change_log.clear();
                }
            });

            if state.change_log.is_empty() {
                ui.label(egui::RichText::new(
                    "No changes recorded. Enable 🔍 Track on a watch.")
                    .color(egui::Color32::DARK_GRAY));
                return;
            }

            egui::ScrollArea::vertical()
                .max_height(200.0)
                .id_salt("change_log_scroll")
                .show(ui, |ui| {
                    egui::Grid::new("change_log_grid")
                        .num_columns(4)
                        .spacing([12.0, 4.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label(egui::RichText::new("Frame").strong());
                            ui.label(egui::RichText::new("Addr").strong());
                            ui.label(egui::RichText::new("old → new").strong());
                            ui.label(egui::RichText::new("PC").strong());
                            ui.end_row();

                            // Newest first.
                            for ev in state.change_log.iter().rev() {
                                ui.label(egui::RichText::new(format!("{}", ev.frame))
                                    .monospace());
                                ui.label(egui::RichText::new(format!("{:06X}", ev.addr))
                                    .monospace()
                                    .color(egui::Color32::from_rgb(150, 150, 200)));
                                ui.label(egui::RichText::new(
                                    format!("0x{:X} → 0x{:X}", ev.old, ev.new))
                                    .monospace()
                                    .color(egui::Color32::from_rgb(220, 180, 120)));
                                ui.label(egui::RichText::new(format!("${:06X}", ev.pc))
                                    .monospace()
                                    .color(egui::Color32::from_rgb(150, 220, 150)));
                                ui.end_row();
                            }
                        });
                });
        });
}

const ALL_FORMATS: [WatchFormat; 9] = [
    WatchFormat::U8,
    WatchFormat::S8,
    WatchFormat::U16LE,
    WatchFormat::U16BE,
    WatchFormat::U32LE,
    WatchFormat::U32BE,
    WatchFormat::Hex8,
    WatchFormat::Hex16,
    WatchFormat::Hex32,
];

fn format_name(fmt: WatchFormat) -> &'static str {
    match fmt {
        WatchFormat::U8 => "u8",
        WatchFormat::S8 => "s8",
        WatchFormat::U16LE => "u16 LE",
        WatchFormat::U16BE => "u16 BE",
        WatchFormat::U32LE => "u32 LE",
        WatchFormat::U32BE => "u32 BE",
        WatchFormat::Hex8 => "hex8",
        WatchFormat::Hex16 => "hex16",
        WatchFormat::Hex32 => "hex32",
    }
}

/// Format the raw little-endian value held in `current` per the watch format.
fn format_value(fmt: WatchFormat, current: Option<u32>) -> String {
    let raw = match current {
        Some(v) => v,
        None => return "—".to_string(),
    };
    match fmt {
        WatchFormat::U8 => format!("{}", raw & 0xFF),
        WatchFormat::S8 => format!("{}", (raw & 0xFF) as u8 as i8),
        WatchFormat::U16LE => format!("{}", raw & 0xFFFF),
        WatchFormat::U16BE => {
            let v = raw & 0xFFFF;
            format!("{}", ((v >> 8) | (v << 8)) & 0xFFFF)
        }
        WatchFormat::U32LE => format!("{}", raw),
        WatchFormat::U32BE => format!("{}", raw.swap_bytes()),
        WatchFormat::Hex8 => format!("0x{:02X}", raw & 0xFF),
        WatchFormat::Hex16 => format!("0x{:04X}", raw & 0xFFFF),
        WatchFormat::Hex32 => format!("0x{:08X}", raw),
    }
}
