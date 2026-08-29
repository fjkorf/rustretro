//! 🔍 Signal Hunt — the in-app face of `docs/signal-hunt.md`.
//!
//! Mark buttons with an editable label, live per-label mark counts, Analyze, a
//! candidate table with the per-mark transitions §4 requires, Export, Reset.
//!
//! The panel is deliberately opinionated about the two failure modes §6 names:
//! the control field is pre-filled and the Analyze button says so when it is
//! empty, and a zero-candidate outcome is rendered as a stated RESULT rather
//! than as an empty table a reader can mistake for "it didn't run".

use bevy_egui::egui;

use crate::debug::DebugState;
use crate::hunt;

pub struct HuntPanel {
    /// Label written by the "＋ Event" button (editable — §2 allows any label).
    event_label: String,
    /// Label written by the "＋ Control" button.
    control_label: String,
    /// Extra window fields (hex text, as typed).
    extra_start: String,
    extra_len: String,
    include_blocks: bool,
    ring_frames: usize,
    pre: u64,
    post: u64,
    include_idle: bool,
    /// Sticky one-liner from the last action.
    note: Option<String>,
    /// Rendered evidence-doc text from the last Export (shown inline; the
    /// button also puts it on the clipboard).
    export_text: Option<String>,
}

impl HuntPanel {
    pub fn new() -> Self {
        HuntPanel {
            event_label: "event".into(),
            control_label: "control".into(),
            extra_start: String::new(),
            extra_len: String::new(),
            include_blocks: true,
            ring_frames: hunt::DEFAULT_RING_FRAMES,
            pre: hunt::DEFAULT_PRE,
            post: hunt::DEFAULT_POST,
            include_idle: true,
            note: None,
            export_text: None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        ui.heading("🔍 Signal hunt");
        ui.label(
            egui::RichText::new(
                "Event-marked differential RAM analysis (docs/signal-hunt.md). Mark the moments \
                 the event happens, mark near-misses as controls, then Analyze. Judging \"that \
                 was a blocked hit\" is YOUR job — the tool only differences what you marked.",
            )
            .small()
            .color(egui::Color32::DARK_GRAY),
        );
        ui.separator();

        self.marking_section(ui, state);
        ui.separator();
        self.region_section(ui);
        ui.separator();
        self.analysis_section(ui);
    }

    // ── marking (§2) ───────────────────────────────────────────────────────
    fn marking_section(&mut self, ui: &mut egui::Ui, state: &mut DebugState) {
        let status = hunt::status();
        let sampling = status["enabled"].as_bool().unwrap_or(false);
        let filled = status["ring_filled"].as_u64().unwrap_or(0);
        let ring = status["ring_frames"].as_u64().unwrap_or(0);

        ui.horizontal(|ui| {
            ui.label("Event label:");
            ui.add(egui::TextEdit::singleline(&mut self.event_label).desired_width(90.0));
            if ui
                .button("＋ Event")
                .on_hover_text("Mark THIS frame as an occurrence of the thing you are hunting")
                .clicked()
            {
                self.note = Some(match hunt::mark_with(state, &self.event_label.clone()) {
                    Ok(m) => m,
                    Err(e) => format!("mark failed: {e}"),
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label("Control label:");
            ui.add(egui::TextEdit::singleline(&mut self.control_label).desired_width(90.0));
            if ui
                .button("＋ Control")
                .on_hover_text("Mark a NEAR-MISS — the same action without the event (a whiff)")
                .clicked()
            {
                self.note = Some(match hunt::mark_with(state, &self.control_label.clone()) {
                    Ok(m) => m,
                    Err(e) => format!("mark failed: {e}"),
                });
            }
        });

        // Live per-label counts (§8).
        let marks = status["marks"].as_array().cloned().unwrap_or_default();
        if marks.is_empty() {
            ui.label(
                egui::RichText::new("no marks yet")
                    .small()
                    .color(egui::Color32::DARK_GRAY),
            );
        } else {
            ui.horizontal_wrapped(|ui| {
                for m in &marks {
                    let label = m["label"].as_str().unwrap_or("?");
                    let n = m["marks"].as_u64().unwrap_or(0);
                    let usable = m["usable"].as_u64().unwrap_or(0);
                    let pending = n.saturating_sub(usable);
                    let text = if pending > 0 {
                        format!("{label}: {n} ({pending} awaiting +POST)")
                    } else {
                        format!("{label}: {n}")
                    };
                    ui.label(egui::RichText::new(text).monospace());
                }
            });
        }

        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(format!(
                    "sampling {} · ring {filled}/{ring} · idle churn {} B over {} quiet frames",
                    if sampling { "on" } else { "OFF" },
                    status["idle_churn_bytes"].as_u64().unwrap_or(0),
                    status["idle_frames"].as_u64().unwrap_or(0),
                ))
                .small()
                .color(if sampling { egui::Color32::DARK_GRAY } else { egui::Color32::RED }),
            );
            if ui.button("Reset").on_hover_text("Discard all marks and the ring").clicked() {
                self.note = Some(hunt::reset());
                self.export_text = None;
            }
        });
        if let Some(n) = status["note"].as_str() {
            ui.colored_label(egui::Color32::from_rgb(220, 140, 60), n);
        }

        // The §2 record per mark, so a gate-closed or evidence-less mark is
        // visible BEFORE you analyze rather than only in the report's warnings.
        let log = status["mark_log"].as_array().cloned().unwrap_or_default();
        if !log.is_empty() {
            egui::CollapsingHeader::new(format!("Marks ({})", log.len()))
                .id_salt("hunt_marks")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("hunt_mark_log")
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for m in log.iter().rev() {
                                let gate = m["gate_open"].as_bool().unwrap_or(false);
                                let usable = m["usable"].as_bool().unwrap_or(false);
                                let text = format!(
                                    "#{} {} @f{} pre f{} post {} {}{}",
                                    m["id"], m["label"].as_str().unwrap_or("?"), m["frame"],
                                    m["pre_frame"], m["post_frame"],
                                    if gate { "" } else { "GATE-CLOSED " },
                                    if usable { "" } else { "· awaiting +POST" },
                                );
                                let color = if !gate {
                                    egui::Color32::from_rgb(230, 120, 60)
                                } else {
                                    ui.visuals().text_color()
                                };
                                ui.label(egui::RichText::new(text).monospace().small().color(color));
                            }
                        });
                });
        }
    }

    // ── region scoping (§3) ────────────────────────────────────────────────
    fn region_section(&mut self, ui: &mut egui::Ui) {
        let status = hunt::status();
        egui::CollapsingHeader::new(format!(
            "Region — {} B, {} window(s)",
            status["region_bytes"].as_u64().unwrap_or(0),
            status["windows"].as_array().map(|a| a.len()).unwrap_or(0)
        ))
        .id_salt("hunt_region")
        .show(ui, |ui| {
            for w in status["windows"].as_array().unwrap_or(&vec![]) {
                ui.label(egui::RichText::new(w.as_str().unwrap_or("")).monospace().small());
            }
            ui.checkbox(&mut self.include_blocks, "Both fighter structs (profile blocks)");
            ui.horizontal(|ui| {
                ui.label("Extra window  start 0x");
                ui.add(egui::TextEdit::singleline(&mut self.extra_start).desired_width(70.0));
                ui.label("len 0x");
                ui.add(egui::TextEdit::singleline(&mut self.extra_len).desired_width(60.0));
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.ring_frames).range(2..=600).prefix("ring "));
                ui.add(egui::DragValue::new(&mut self.pre).range(0..=240).prefix("pre "));
                ui.add(egui::DragValue::new(&mut self.post).range(1..=240).prefix("post "));
            });
            ui.checkbox(&mut self.include_idle, "Subtract idle churn")
                .on_hover_text(
                    "§4: bytes that move between consecutive quiet frames are disqualified. \
                     The report shows the result both ways regardless.",
                );
            if ui
                .button("Apply")
                .on_hover_text(
                    "Changing the REGION discards marks captured under the old layout — their \
                     snapshots are not comparable.",
                )
                .clicked()
            {
                let parse = |s: &str| -> Option<u32> {
                    let t = s.trim().trim_start_matches("0x").trim_start_matches("0X");
                    (!t.is_empty()).then(|| u32::from_str_radix(t, 16).ok()).flatten()
                };
                let extra = match (parse(&self.extra_start), parse(&self.extra_len)) {
                    (Some(a), Some(l)) => Some((a, l)),
                    (None, None) => None,
                    _ => {
                        self.note =
                            Some("extra window needs BOTH start and len as hex".to_string());
                        return;
                    }
                };
                self.note = Some(
                    match hunt::configure(
                        self.include_blocks,
                        extra,
                        Some(self.ring_frames),
                        Some(self.pre),
                        Some(self.post),
                        Some(self.include_idle),
                        Some(true),
                    ) {
                        Ok(m) => m,
                        Err(e) => e,
                    },
                );
            }
        });
    }

    // ── analysis (§4-§6) ───────────────────────────────────────────────────
    fn analysis_section(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("▶ Analyze").clicked() {
                let control = (!self.control_label.trim().is_empty())
                    .then(|| self.control_label.trim().to_string());
                match hunt::run_analysis(&self.event_label.clone(), control.as_deref()) {
                    Ok(_) => self.note = None,
                    Err(e) => self.note = Some(e),
                }
                self.export_text = None;
            }
            if ui
                .button("⎘ Export")
                .on_hover_text("Evidence-doc markdown → clipboard (and shown below)")
                .clicked()
            {
                let md = hunt::with_state(|g| g.last_analysis.as_ref().map(hunt::export_markdown))
                    .flatten();
                match md {
                    Some(text) => {
                        ui.ctx().copy_text(text.clone());
                        self.export_text = Some(text);
                    }
                    None => self.note = Some("nothing to export — run Analyze first".into()),
                }
            }
        });

        if let Some(n) = &self.note {
            ui.label(egui::RichText::new(n).small());
        }

        let Some(a) = hunt::with_state(|g| g.last_analysis.clone()).flatten() else {
            return;
        };
        ui.separator();

        // §6 warnings, first and loud.
        for w in &a.warnings {
            ui.colored_label(egui::Color32::from_rgb(230, 120, 60), format!("⚠ {w}"));
        }
        // §6: the settings the run actually used.
        ui.label(
            egui::RichText::new(format!(
                "ring {} · pre {} · post {} · idle {} · region {} B · events {}/{} usable · \
                 controls {}/{} usable",
                a.ring_frames,
                a.pre,
                a.post,
                if a.include_idle { "subtracted" } else { "kept" },
                a.region_bytes,
                a.event_marks_usable,
                a.event_marks,
                a.control_marks_usable,
                a.control_marks,
            ))
            .small()
            .monospace()
            .color(egui::Color32::DARK_GRAY),
        );
        ui.label(&a.verdict);
        ui.label(
            egui::RichText::new(format!(
                "event set {} → −{} by controls → −{} by idle churn",
                a.event_set_size, a.eliminated_by_control_marks, a.eliminated_by_idle_churn
            ))
            .small()
            .monospace(),
        );

        if !a.candidates.is_empty() {
            egui::ScrollArea::vertical()
                .id_salt("hunt_candidates")
                .max_height(220.0)
                .show(ui, |ui| {
                    egui::Grid::new("hunt_cand_grid").striped(true).show(ui, |ui| {
                        ui.label("#");
                        ui.label("address");
                        ui.label("tags");
                        ui.label("per-event pre→post");
                        ui.end_row();
                        for (i, c) in a.candidates.iter().enumerate() {
                            ui.label(format!("{}", i + 1));
                            ui.label(
                                egui::RichText::new(format!("{} (0x{:X})", c.name, c.addr))
                                    .monospace(),
                            );
                            let mut tags = Vec::new();
                            if c.small_values {
                                tags.push("small");
                            }
                            if c.counter_like {
                                tags.push("counter");
                            }
                            if c.byte_like {
                                tags.push("byte");
                            } else {
                                tags.push("word-half");
                            }
                            if c.consistent {
                                tags.push("consistent");
                            }
                            ui.label(egui::RichText::new(tags.join(" ")).small());
                            ui.label(
                                egui::RichText::new(
                                    c.event_transitions
                                        .iter()
                                        .map(|(id, p, q)| format!("m{id}:{p}→{q}"))
                                        .collect::<Vec<_>>()
                                        .join("  "),
                                )
                                .monospace()
                                .small(),
                            );
                            ui.end_row();
                        }
                    });
                });
            ui.label(
                egui::RichText::new(
                    "Hypotheses only — the hunt NEVER writes a profile. Confirm with a write-test \
                     before promoting anything here.",
                )
                .small()
                .color(egui::Color32::DARK_GRAY),
            );
        }

        if let Some(text) = &self.export_text {
            egui::ScrollArea::vertical()
                .id_salt("hunt_export")
                .max_height(160.0)
                .show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut text.as_str())
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY),
                    );
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render the panel headlessly against a real `DebugState`, in both the
    /// no-analysis and post-analysis states. egui panics at RUNTIME on things a
    /// compile cannot catch — duplicate widget ids inside the nested
    /// scroll/collapsing/grid areas, a borrow held across a closure — and this
    /// panel is only reachable by clicking a tab, so a smoke render is the only
    /// cheap way to know it opens at all.
    #[test]
    fn panel_renders_headlessly_before_and_after_analysis() {
        crate::profile::init_for_tests();
        let ctx = egui::Context::default();
        let mut panel = HuntPanel::new();
        let mut ds = DebugState::new();

        let mut draw = |panel: &mut HuntPanel, ds: &mut DebugState| {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| panel.show(ui, ds));
            });
        };

        // Cold: no region resolved, no marks, no analysis.
        draw(&mut panel, &mut ds);

        // Warm: a completed hunt sitting in the shared state, including the
        // collapsing mark log and the candidate grid.
        let _ = hunt::configure(true, Some((0x1000, 0x40)), Some(8), None, None, None, Some(true));
        let _ = hunt::mark_with(&ds, "event");
        let _ = hunt::run_analysis("event", Some("control"));
        panel.export_text = Some("# export".into());
        draw(&mut panel, &mut ds);
        let _ = hunt::reset();
    }
}
