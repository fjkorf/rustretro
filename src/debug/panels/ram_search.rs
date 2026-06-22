use bevy_egui::egui::{self, Color32, RichText, ScrollArea};

use crate::debug::{
    DebugState, SearchCompare, SearchSize, SearchSource, Watch,
};

/// Cap on how many result rows we render; above this we show the count only.
const RESULT_RENDER_CAP: usize = 1000;

pub struct RamSearchPanel {
    compare: SearchCompare,
    /// Operator-picker selection independent of any embedded value.
    compare_kind: CompareKind,
    /// When true the search compares against a specific value, else previous snapshot.
    use_specific: bool,
    specific_value: String,
    diff_by: String,
}

#[derive(Clone, Copy, PartialEq)]
enum CompareKind {
    Equal,
    NotEqual,
    Less,
    Greater,
    Changed,
    Unchanged,
    Increased,
    Decreased,
    DifferentBy,
}

impl CompareKind {
    const ALL: [CompareKind; 9] = [
        CompareKind::Equal,
        CompareKind::NotEqual,
        CompareKind::Less,
        CompareKind::Greater,
        CompareKind::Changed,
        CompareKind::Unchanged,
        CompareKind::Increased,
        CompareKind::Decreased,
        CompareKind::DifferentBy,
    ];

    fn label(self) -> &'static str {
        match self {
            CompareKind::Equal => "= Equal",
            CompareKind::NotEqual => "≠ Not equal",
            CompareKind::Less => "< Less",
            CompareKind::Greater => "> Greater",
            CompareKind::Changed => "≈ Changed",
            CompareKind::Unchanged => "= Unchanged",
            CompareKind::Increased => "↑ Increased",
            CompareKind::Decreased => "↓ Decreased",
            CompareKind::DifferentBy => "± Different by",
        }
    }

    /// Whether this operator compares against the previous snapshot only
    /// (so the specific-value choice is irrelevant).
    fn is_relative(self) -> bool {
        matches!(
            self,
            CompareKind::Changed
                | CompareKind::Unchanged
                | CompareKind::Increased
                | CompareKind::Decreased
        )
    }

    /// Whether this operator can compare against a specific value.
    fn takes_value(self) -> bool {
        matches!(
            self,
            CompareKind::Equal
                | CompareKind::NotEqual
                | CompareKind::Less
                | CompareKind::Greater
        )
    }
}

impl RamSearchPanel {
    pub fn new() -> Self {
        RamSearchPanel {
            compare: SearchCompare::Equal,
            compare_kind: CompareKind::Equal,
            use_specific: true,
            specific_value: String::new(),
            diff_by: String::new(),
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        // Clamp region index.
        if state.ram_search.region_idx >= state.memory_regions.len() {
            state.ram_search.region_idx = 0;
        }

        // ── Top controls: region + size + interpretation ────────────────────
        ui.horizontal(|ui| {
            ui.label("Memory:");
            if state.memory_regions.is_empty() {
                ui.label("(no regions available)");
            } else {
                let names: Vec<String> =
                    state.memory_regions.iter().map(|r| r.name.clone()).collect();
                let mut idx = state.ram_search.region_idx;
                egui::ComboBox::from_id_salt("ram_search_region")
                    .selected_text(names[idx].clone())
                    .show_index(ui, &mut idx, names.len(), |i| {
                        egui::WidgetText::from(names[i].clone())
                    });
                state.ram_search.region_idx = idx;
            }

            ui.separator();
            ui.label("Size:");
            ui.radio_value(&mut state.ram_search.size, SearchSize::U8, "8");
            ui.radio_value(&mut state.ram_search.size, SearchSize::U16, "16");
            ui.radio_value(&mut state.ram_search.size, SearchSize::U32, "32");

            ui.separator();
            ui.checkbox(&mut state.ram_search.signed, "signed");
            ui.checkbox(&mut state.ram_search.hex, "hex");
        });

        ui.horizontal(|ui| {
            if ui.button("🔄 New Search / Reset").clicked() {
                state.reset_search();
            }
            ui.separator();
            let count = state.ram_search.candidates.len();
            let txt = if state.ram_search.started {
                RichText::new(format!("{} candidates", count))
                    .strong()
                    .color(Color32::from_rgb(150, 220, 150))
            } else {
                RichText::new("not started").color(Color32::DARK_GRAY)
            };
            ui.label(txt);
        });

        ui.separator();

        // ── Operator + source + value ───────────────────────────────────────
        ui.horizontal(|ui| {
            ui.label("Operator:");
            egui::ComboBox::from_id_salt("ram_search_op")
                .selected_text(self.compare_kind.label())
                .show_ui(ui, |ui| {
                    for k in CompareKind::ALL {
                        ui.selectable_value(&mut self.compare_kind, k, k.label());
                    }
                });

            if self.compare_kind == CompareKind::DifferentBy {
                ui.label("by:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.diff_by)
                        .desired_width(70.0)
                        .hint_text("delta"),
                );
            }
        });

        // Compare-to choice: only meaningful when the operator can take a value.
        if self.compare_kind.takes_value() {
            ui.horizontal(|ui| {
                ui.label("Compare to:");
                ui.radio_value(&mut self.use_specific, false, "previous value");
                ui.radio_value(&mut self.use_specific, true, "specific value");
                if self.use_specific {
                    let hint = if state.ram_search.hex { "0x.." } else { "value" };
                    ui.add(
                        egui::TextEdit::singleline(&mut self.specific_value)
                            .desired_width(90.0)
                            .hint_text(hint),
                    );
                }
            });
        } else if self.compare_kind.is_relative() {
            ui.label(
                RichText::new("compares against the previous checkpoint")
                    .italics()
                    .color(Color32::DARK_GRAY),
            );
        }

        ui.horizontal(|ui| {
            let label = if state.ram_search.started { "🔍 Next" } else { "🔍 Search" };
            let enabled = state.ram_search.started;
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                if let Some((compare, source)) = self.resolve(state.ram_search.hex) {
                    self.compare = compare;
                    state.step_search(compare, source);
                }
            }
            if !state.ram_search.started {
                ui.label(
                    RichText::new("press Reset to begin")
                        .color(Color32::DARK_GRAY),
                );
            }
        });

        ui.separator();

        // ── Results ─────────────────────────────────────────────────────────
        self.show_results(ui, state);
    }

    /// Build the (compare, source) pair from current UI state, parsing values.
    fn resolve(&self, hex: bool) -> Option<(SearchCompare, SearchSource)> {
        let parse = |s: &str| -> Option<u32> {
            let t = s.trim();
            if t.is_empty() {
                return None;
            }
            if let Some(rest) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                u32::from_str_radix(rest, 16).ok()
            } else if hex {
                u32::from_str_radix(t, 16).ok()
            } else {
                // Allow signed decimal too (stored as raw bits).
                t.parse::<u32>().ok().or_else(|| t.parse::<i32>().ok().map(|v| v as u32))
            }
        };

        let compare = match self.compare_kind {
            CompareKind::Equal => SearchCompare::Equal,
            CompareKind::NotEqual => SearchCompare::NotEqual,
            CompareKind::Less => SearchCompare::Less,
            CompareKind::Greater => SearchCompare::Greater,
            CompareKind::Changed => SearchCompare::Changed,
            CompareKind::Unchanged => SearchCompare::Unchanged,
            CompareKind::Increased => SearchCompare::Increased,
            CompareKind::Decreased => SearchCompare::Decreased,
            CompareKind::DifferentBy => {
                let d = self.diff_by.trim().parse::<i64>().ok()?;
                SearchCompare::DifferentBy(d)
            }
        };

        let source = if self.compare_kind.takes_value() && self.use_specific {
            SearchSource::SpecificValue(parse(&self.specific_value)?)
        } else {
            SearchSource::PreviousSnapshot
        };

        Some((compare, source))
    }

    fn show_results(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        if !state.ram_search.started {
            ui.label("No active search. Choose a region and press Reset.");
            return;
        }

        let count = state.ram_search.candidates.len();
        if count == 0 {
            ui.label(
                RichText::new("No candidates remain — try a different value or reset.")
                    .color(Color32::from_rgb(220, 150, 150)),
            );
            return;
        }
        if count > RESULT_RENDER_CAP {
            ui.label(format!(
                "{} candidates — narrow below {} to list them.",
                count, RESULT_RENDER_CAP
            ));
            return;
        }

        let size = state.ram_search.size;
        let signed = state.ram_search.signed;
        let hex = state.ram_search.hex;
        let len = size.byte_len();

        // Snapshot rows up front to avoid borrowing `state` during the closure
        // while we also push watches.
        let rows: Vec<(usize, Option<u32>)> = state
            .ram_search
            .candidates
            .iter()
            .map(|&addr| (addr, state.read_addr(addr, len)))
            .collect();

        let mut to_watch: Option<usize> = None;

        ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                egui::Grid::new("ram_search_results")
                    .num_columns(3)
                    .spacing([12.0, 2.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (addr, val) in &rows {
                            ui.label(
                                RichText::new(format!("{:06X}", addr))
                                    .monospace()
                                    .color(Color32::from_rgb(150, 150, 200)),
                            );

                            let vtext = match val {
                                Some(v) => format_value(*v, size, signed, hex),
                                None => "??".to_string(),
                            };
                            ui.label(RichText::new(vtext).monospace().color(Color32::WHITE));

                            if ui.button("+Watch").clicked() {
                                to_watch = Some(*addr);
                            }
                            ui.end_row();
                        }
                    });
            });

        if let Some(addr) = to_watch {
            let value = state.read_addr(addr, len);
            state.watches.push(Watch {
                label: format!("addr_{:X}", addr),
                addr,
                format: size.watch_format(hex),
                frozen: false,
                frozen_value: None,
                track_changes: false,
                current: value,
                prev_value: None,
            });
        }
    }
}

/// Format a raw little-endian value per the search's size/signed/hex settings.
fn format_value(raw: u32, size: SearchSize, signed: bool, hex: bool) -> String {
    let bits = (size.byte_len() * 8) as u32;
    if hex {
        match size {
            SearchSize::U8 => format!("0x{:02X}", raw & 0xFF),
            SearchSize::U16 => format!("0x{:04X}", raw & 0xFFFF),
            SearchSize::U32 => format!("0x{:08X}", raw),
        }
    } else if signed {
        let v = if bits < 32 {
            let shift = 32 - bits;
            ((raw << shift) as i32 >> shift) as i64
        } else {
            (raw as i32) as i64
        };
        format!("{}", v)
    } else {
        let v = match size {
            SearchSize::U8 => raw & 0xFF,
            SearchSize::U16 => raw & 0xFFFF,
            SearchSize::U32 => raw,
        };
        format!("{}", v)
    }
}
