use bevy_egui::egui;
use crate::audio::AudioOutput;

/// Audio controls panel for the debug window.
pub struct AudioControls;

impl AudioControls {
    pub fn show(ui: &mut egui::Ui, audio: &mut AudioOutput) {
        ui.vertical(|ui| {
            ui.heading("🔊 Audio Controls");
            ui.separator();

            let mut is_muted = audio.is_muted();
            if ui.checkbox(&mut is_muted, "Mute").changed() {
                audio.set_mute(is_muted);
            }

            let mut volume_percent = (audio.get_volume() * 100.0) as i32;
            if ui
                .add(
                    egui::Slider::new(&mut volume_percent, 0..=100)
                        .text("Volume")
                        .step_by(1.0),
                )
                .changed()
            {
                audio.set_volume(volume_percent as f32 / 100.0);
            }
            ui.label(format!("{}%", volume_percent));

            ui.separator();
            ui.label(format!("Sample Rate: {} Hz", audio.sample_rate as u32));
            ui.label(format!(
                "Status: {}",
                if audio.enabled { "Enabled" } else { "Disabled" }
            ));
        });
    }
}
