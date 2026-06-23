use bevy_egui::egui;

/// Panel: Sega Genesis VDP Hardware Register Decoder
///
/// Displays the 24 VDP registers ($00-$17) with bitfield decoding to human-readable
/// text, modelled after no$gba's I/O register window. The panel is fully self-contained
/// and does not hold mutable state — `show()` accepts the register bytes directly so
/// the decode logic can be unit-tested in isolation.
pub struct VdpRegisters;

impl VdpRegisters {
    pub fn new() -> Self {
        VdpRegisters
    }

    /// Render the VDP register table.
    ///
    /// `regs` — the 24 VDP registers $00–$17 read from DebugState.vdp_regs.
    pub fn show(&mut self, ui: &mut egui::Ui, regs: &[u8; 24]) {
        ui.heading("VDP Registers ($00-$17)");
        ui.small("Genesis Video Display Processor — bitfield decoder");
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::Grid::new("vdp_regs_grid")
                .num_columns(3)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    // Header row
                    ui.label(egui::RichText::new("Reg").strong().monospace());
                    ui.label(egui::RichText::new("Raw").strong().monospace());
                    ui.label(egui::RichText::new("Decoded").strong());
                    ui.end_row();

                    for index in 0..24usize {
                        let value = regs[index];
                        let decoded = decode_vdp_register(index, value);

                        ui.label(
                            egui::RichText::new(format!("${:02X}", index))
                                .monospace()
                                .color(egui::Color32::from_rgb(100, 180, 255)),
                        );
                        ui.label(
                            egui::RichText::new(format!("${:02X}", value))
                                .monospace()
                                .color(egui::Color32::LIGHT_GRAY),
                        );
                        ui.label(egui::RichText::new(&decoded).monospace());
                        ui.end_row();
                    }
                });
        });

        ui.separator();
        ui.small("Register values captured each frame  ·  Populate DebugState.vdp_regs from the libretro core's VDP peek interface");
    }
}

/// Decode a single VDP register byte into a human-readable bitfield description.
///
/// This is a pure function with no side effects — suitable for unit testing.
///
/// # Arguments
/// * `index` — register index (0–23, corresponding to VDP registers $00–$17)
/// * `value` — raw byte value of the register
///
/// # Returns
/// A `String` describing the active bitfields in plain English.
pub fn decode_vdp_register(index: usize, value: u8) -> String {
    match index {
        // $00 — Mode Register 1
        0 => {
            let mut parts = Vec::new();
            if value & (1 << 5) != 0 { parts.push("Freeze HV counter"); }
            if value & (1 << 4) != 0 { parts.push("HINT enable"); } else { parts.push("HINT disable"); }
            if value & (1 << 3) != 0 { parts.push("Palette: M5=0 palettes"); }
            if value & (1 << 2) != 0 { parts.push("HV latch enable"); }
            if value & (1 << 1) != 0 { parts.push("SMS/GG mode"); } else { parts.push("Megadrive mode"); }
            if value & (1 << 0) != 0 { parts.push("SMS display disable"); }
            format!("Mode 1: {}", if parts.is_empty() { "all disabled".to_string() } else { parts.join(", ") })
        }

        // $01 — Mode Register 2
        1 => {
            let mut parts = Vec::new();
            if value & (1 << 7) != 0 { parts.push("128KB VRAM"); }
            if value & (1 << 6) != 0 { parts.push("Display ON"); } else { parts.push("Display OFF"); }
            if value & (1 << 5) != 0 { parts.push("VINT enable"); } else { parts.push("VINT disable"); }
            if value & (1 << 4) != 0 { parts.push("DMA enable"); } else { parts.push("DMA disable"); }
            if value & (1 << 3) != 0 { parts.push("V30 (PAL 240 lines)"); } else { parts.push("V28 (NTSC 224 lines)"); }
            if value & (1 << 2) != 0 { parts.push("M2 set"); }
            if value & (1 << 1) != 0 { parts.push("M5/Genesis mode"); } else { parts.push("SMS mode"); }
            format!("Mode 2: {}", parts.join(", "))
        }

        // $02 — Plane A Name Table Address
        2 => {
            // Bits [5:3] select the base address; each unit = $2000
            let base = ((value >> 3) & 0x07) as u16 * 0x2000;
            format!("Plane A nametable: VRAM ${:04X}  (raw bits[5:3]={:03b})", base, (value >> 3) & 0x07)
        }

        // $03 — Window Plane Name Table Address
        3 => {
            // In H40 mode bits [6:1]; H32 mode bits [5:1].  Provide both interpretations.
            let h40_base = ((value >> 1) & 0x3F) as u16 * 0x0400;
            let h32_base = ((value >> 1) & 0x1F) as u16 * 0x0800;
            format!(
                "Window nametable: H40 VRAM ${:04X} / H32 VRAM ${:04X}",
                h40_base, h32_base
            )
        }

        // $04 — Plane B Name Table Address
        4 => {
            // Bits [2:0]; each unit = $2000
            let base = (value & 0x07) as u16 * 0x2000;
            format!("Plane B nametable: VRAM ${:04X}  (raw bits[2:0]={:03b})", base, value & 0x07)
        }

        // $05 — Sprite Attribute Table Address
        5 => {
            // Bits [6:0]; base = value[6:0] * $200  (H40 mode)
            // H32: bits [5:0] * $200
            let base_h40 = ((value & 0x7F) as u16) * 0x0200;
            let base_h32 = ((value & 0x3F) as u16) * 0x0200;
            format!(
                "Sprite table: H40 VRAM ${:04X} / H32 VRAM ${:04X}",
                base_h40, base_h32
            )
        }

        // $06 — Sprite Pattern Generator Base Address (usually 0)
        6 => {
            format!("Sprite tile base: ${:02X}  (reserved — typically $00)", value)
        }

        // $07 — Background Color
        7 => {
            let palette = (value >> 4) & 0x03;
            let index = value & 0x0F;
            format!("BG color: palette {} entry {}  (CRAM offset ${:02X})", palette, index, palette * 16 + index)
        }

        // $08 — Unused (SMS horizontal scroll)
        8 => {
            format!("Unused (SMS HScroll): ${:02X}", value)
        }

        // $09 — Unused (SMS vertical scroll)
        9 => {
            format!("Unused (SMS VScroll): ${:02X}", value)
        }

        // $0A — H Interrupt Counter (HINT counter)
        10 => {
            format!("HINT counter: {} (interrupt every {} scanlines)", value, value + 1)
        }

        // $0B — Mode Register 3 (external interrupt / scrolling)
        11 => {
            let mut parts = Vec::new();
            if value & (1 << 3) != 0 { parts.push("EXT int enable"); } else { parts.push("EXT int disable"); }
            let vscroll = match (value >> 2) & 0x01 {
                0 => "VScroll: full screen",
                _ => "VScroll: per 2-cell column",
            };
            parts.push(vscroll);
            let hscroll = match value & 0x03 {
                0 => "HScroll: full screen",
                2 => "HScroll: per cell (row)",
                3 => "HScroll: per line",
                _ => "HScroll: prohibited",
            };
            parts.push(hscroll);
            format!("Mode 3: {}", parts.join(", "))
        }

        // $0C — Mode Register 4 (H resolution, shadow/highlight, interlace)
        12 => {
            let mut parts = Vec::new();
            let h_res = if (value & (1 << 7) != 0) || (value & (1 << 0) != 0) {
                "H40 (320px)"
            } else {
                "H32 (256px)"
            };
            parts.push(h_res);
            let sh = match (value >> 3) & 0x03 {
                0 => "S/H: off",
                1 => "S/H: Shadow/Highlight ON",
                2 => "S/H: extended (split screen)",
                _ => "S/H: reserved",
            };
            parts.push(sh);
            let interlace = match (value >> 1) & 0x03 {
                0 => "Interlace: off",
                1 => "Interlace: mode 1 (double resolution V)",
                3 => "Interlace: mode 2 (double resolution sprites)",
                _ => "Interlace: reserved",
            };
            parts.push(interlace);
            format!("Mode 4: {}", parts.join(", "))
        }

        // $0D — H Scroll Table Address
        13 => {
            let base = ((value & 0x3F) as u16) * 0x0400;
            format!("HScroll table: VRAM ${:04X}  (bits[5:0]={:06b})", base, value & 0x3F)
        }

        // $0E — Nametable Pattern Generator Base (usually 0)
        14 => {
            format!("Nametable pattern base: ${:02X}  (usually $00)", value)
        }

        // $0F — Auto-Increment Value
        15 => {
            format!("Auto-increment: {} (${:02X})  — added to VRAM addr after each data port access", value, value)
        }

        // $10 — Plane Size (H×V scroll plane dimensions)
        16 => {
            let h_size = match value & 0x03 {
                0 => "32 cells",
                1 => "64 cells",
                3 => "128 cells",
                _ => "prohibited",
            };
            let v_size = match (value >> 4) & 0x03 {
                0 => "32 cells",
                1 => "64 cells",
                3 => "128 cells",
                _ => "prohibited",
            };
            format!("Plane size: H={} V={}", h_size, v_size)
        }

        // $11 — Window Plane H Position
        17 => {
            let dir = if value & (1 << 7) != 0 { "right" } else { "left" };
            let pos = (value & 0x1F) as u16 * 2; // in cells (each unit = 2 cells)
            format!("Window H: display from {} at cell {} (${:02X}*2)", dir, pos, value & 0x1F)
        }

        // $12 — Window Plane V Position
        18 => {
            let dir = if value & (1 << 7) != 0 { "below" } else { "above" };
            let pos = value & 0x1F;
            format!("Window V: display {} scanline-row {} (${:02X})", dir, pos, value & 0x1F)
        }

        // $13 — DMA Length Low byte
        19 => {
            format!("DMA length low:  ${:02X}  (combined with reg $14 for full 16-bit length)", value)
        }

        // $14 — DMA Length High byte
        20 => {
            format!("DMA length high: ${:02X}  (combined with reg $13: total words = reg14<<8 | reg13)", value)
        }

        // $15 — DMA Source Address Low byte
        21 => {
            format!("DMA src addr low:  ${:02X}  (bits [7:0] of source >> 1)", value)
        }

        // $16 — DMA Source Address Mid byte
        22 => {
            format!("DMA src addr mid:  ${:02X}  (bits [15:8] of source >> 1)", value)
        }

        // $17 — DMA Source Address High / DMA type
        23 => {
            let dma_type = match (value >> 6) & 0x03 {
                0 | 1 => "DMA: 68K->VDP memory transfer",
                2     => "DMA: VRAM fill",
                3     => "DMA: VRAM copy",
                _     => "DMA: unknown",
            };
            let src_high = value & 0x7F;
            format!(
                "DMA src high: ${:02X}  (bits [22:16] of source >> 1)  — {}",
                src_high, dma_type
            )
        }

        // Unknown / beyond $17
        _ => {
            format!("Unknown register ${:02X}: ${:02X}", index, value)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::decode_vdp_register;

    #[test]
    fn reg01_display_on() {
        // Bit 6 of register $01 enables display
        let result = decode_vdp_register(1, 0b0100_0000);
        assert!(
            result.contains("Display ON"),
            "Expected 'Display ON' in decoded output, got: {result}"
        );
    }

    #[test]
    fn reg01_display_off_and_dma_enable() {
        // Bit 6 clear = Display OFF, bit 4 set = DMA enable
        let result = decode_vdp_register(1, 0b0001_0100);
        assert!(
            result.contains("Display OFF"),
            "Expected 'Display OFF', got: {result}"
        );
        assert!(
            result.contains("DMA enable"),
            "Expected 'DMA enable', got: {result}"
        );
    }

    #[test]
    fn reg00_hint_enable() {
        // Bit 4 of register $00 enables HINT
        let result = decode_vdp_register(0, 0b0001_0000);
        assert!(
            result.contains("HINT enable"),
            "Expected 'HINT enable', got: {result}"
        );
    }

    #[test]
    fn reg07_background_color() {
        // Palette 2, color index 5 → value = (2 << 4) | 5 = 0x25
        let result = decode_vdp_register(7, 0x25);
        assert!(
            result.contains("palette 2"),
            "Expected 'palette 2', got: {result}"
        );
        assert!(
            result.contains("entry 5"),
            "Expected 'entry 5', got: {result}"
        );
    }

    #[test]
    fn reg0a_hint_counter() {
        // HINT counter value 7 means interrupt every 8 scanlines
        let result = decode_vdp_register(10, 7);
        assert!(
            result.contains("8 scanlines"),
            "Expected '8 scanlines', got: {result}"
        );
    }

    #[test]
    fn reg0c_h40_mode() {
        // Bit 7 set → H40
        let result = decode_vdp_register(12, 0b1000_0001);
        assert!(
            result.contains("H40"),
            "Expected 'H40' in decoded output, got: {result}"
        );
    }

    #[test]
    fn reg10_plane_size() {
        // H=64 cells (bits[1:0]=01), V=32 cells (bits[5:4]=00)
        let result = decode_vdp_register(16, 0b0000_0001);
        assert!(
            result.contains("H=64 cells"),
            "Expected 'H=64 cells', got: {result}"
        );
        assert!(
            result.contains("V=32 cells"),
            "Expected 'V=32 cells', got: {result}"
        );
    }
}
