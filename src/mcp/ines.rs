//! Pure iNES / NES 2.0 header parsing for the `rom_file` source.
//!
//! The running core hides a NES cart's CHR-ROM graphics, but the `.nes` FILE
//! carries them at a known offset. This module locates the PRG-ROM and CHR-ROM
//! spans within the file so the MCP tools can address them by name
//! (`rom_file:header` / `:prg` / `:chr`) and report a `rom_info` summary —
//! reusing the existing 2bpp tile decoder on the CHR span.
//!
//! Facts verified against the NESdev Wiki (iNES / NES 2.0 / PPU pattern tables):
//! file layout is contiguous `header(16) [+ trainer 512] + PRG + CHR`; PRG size =
//! byte4 × 16 KiB, CHR size = byte5 × 8 KiB (0 ⇒ CHR-RAM, no CHR-ROM in the
//! file); mapper = (byte6>>4) | (byte7 & 0xF0). NES 2.0 (byte7 bits 2-3 == 0b10)
//! widens the sizes via byte 9 nibbles and adds an exponent-multiplier form for
//! huge ROMs. PURE and unit-tested.

/// Parsed iNES / NES 2.0 metadata, with byte offsets/sizes resolved to locate
/// PRG-ROM and CHR-ROM inside the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InesInfo {
    /// True when the header is NES 2.0 (byte 7 bits 2-3 == 0b10 and the implied
    /// size fits the file); otherwise plain iNES.
    pub is_nes2: bool,
    /// Full mapper number (iNES 8-bit; NES 2.0 extends to 12-bit via byte 8).
    pub mapper: u16,
    /// NES 2.0 submapper (byte 8 high nibble); 0 for plain iNES.
    pub submapper: u8,
    /// PRG-ROM size in bytes.
    pub prg_rom_size: usize,
    /// CHR-ROM size in bytes. 0 ⇒ the board uses CHR-RAM (no CHR-ROM in the file).
    pub chr_rom_size: usize,
    /// True when `chr_rom_size == 0` (CHR-RAM cart).
    pub chr_is_ram: bool,
    /// 512-byte trainer present (byte 6 bit 2).
    pub has_trainer: bool,
    /// File offset where PRG-ROM begins (16 + 512 if trainer).
    pub prg_offset: usize,
    /// File offset where CHR-ROM begins (`prg_offset + prg_rom_size`). Only
    /// meaningful when `chr_rom_size > 0`.
    pub chr_offset: usize,
    /// "horizontal" | "vertical" | "four-screen" (nominal; mapper-controlled
    /// mappers override at runtime).
    pub mirroring: &'static str,
    /// Battery-backed save RAM present (byte 6 bit 1).
    pub battery: bool,
    /// Best-effort CHR-RAM size in bytes (NES 2.0 byte 11 low nibble: 64 << n),
    /// when `chr_is_ram`; 0 if unknown / not NES 2.0.
    pub chr_ram_size: usize,
    /// Total file length, for bounds context.
    pub file_len: usize,
}

const MAGIC: [u8; 4] = [0x4E, 0x45, 0x53, 0x1A]; // "NES\x1A"

/// Decode a NES 2.0 size field where the size-MSB nibble may select the
/// exponent-multiplier form. `lsb` is the low byte (byte 4 / byte 5); `msb` is
/// the 4-bit high nibble (from byte 9). Unit is 16384 (PRG) or 8192 (CHR).
/// When `msb == 0xF`, `lsb` is `EEEEEEMM`: size = 2^E × (MM·2+1) bytes.
fn nes2_size(lsb: u8, msb: u8, unit: usize) -> usize {
    if msb == 0x0F {
        let exponent = (lsb >> 2) as u32;
        let multiplier = ((lsb & 0x03) as usize) * 2 + 1;
        // 2^exponent * multiplier bytes (cap exponent to avoid overflow on junk).
        if exponent >= usize::BITS {
            return 0;
        }
        (1usize << exponent).saturating_mul(multiplier)
    } else {
        let count = ((msb as usize) << 8) | lsb as usize;
        count * unit
    }
}

/// Parse a 16-byte iNES / NES 2.0 header. Returns `None` when the buffer is too
/// short or the magic is absent (i.e. not a `.nes` file).
pub fn parse_ines(bytes: &[u8]) -> Option<InesInfo> {
    if bytes.len() < 16 || bytes[0..4] != MAGIC {
        return None;
    }
    let h = &bytes[0..16];
    let file_len = bytes.len();

    let has_trainer = h[6] & 0x04 != 0;
    let battery = h[6] & 0x02 != 0;
    let mirroring = if h[6] & 0x08 != 0 {
        "four-screen"
    } else if h[6] & 0x01 != 0 {
        "vertical"
    } else {
        "horizontal"
    };

    // NES 2.0 iff byte7 bits 2-3 == 0b10 AND the implied total size fits the file
    // (the size guard rejects archaic iNES dumps that coincidentally set the bits,
    // e.g. trailing "DiskDude!" garbage in byte 7).
    let nes2_bits = (h[7] & 0x0C) == 0x08;

    let prg_offset = 16 + if has_trainer { 512 } else { 0 };

    // Compute sizes both ways; pick NES 2.0 only if the bits say so AND it fits.
    let (mut is_nes2, prg_rom_size, chr_rom_size, mapper, submapper, chr_ram_size) = if nes2_bits {
        let prg = nes2_size(h[4], h[9] & 0x0F, 16384);
        let chr = nes2_size(h[5], (h[9] >> 4) & 0x0F, 8192);
        let mapper = (h[6] >> 4) as u16 | (((h[7] >> 4) as u16) << 4) | (((h[8] & 0x0F) as u16) << 8);
        let submapper = h[8] >> 4;
        // byte 11 low nibble: CHR-RAM shift count (64 << n), 0 = none.
        let chr_ram = {
            let n = h[11] & 0x0F;
            if n == 0 { 0 } else { 64usize << n }
        };
        (true, prg, chr, mapper, submapper, chr_ram)
    } else {
        let prg = h[4] as usize * 16384;
        let chr = h[5] as usize * 8192;
        let mapper = (h[6] >> 4) as u16 | (((h[7] >> 4) as u16) << 4);
        (false, prg, chr, mapper, 0u8, 0usize)
    };

    // Size guard: if NES 2.0 sizing overruns the actual file, fall back to plain
    // iNES interpretation (more robust against mis-flagged dumps).
    let (prg_rom_size, chr_rom_size, mapper, submapper, chr_ram_size) =
        if is_nes2 && prg_offset + prg_rom_size + chr_rom_size > file_len {
            is_nes2 = false;
            let prg = h[4] as usize * 16384;
            let chr = h[5] as usize * 8192;
            let mapper = (h[6] >> 4) as u16 | (((h[7] >> 4) as u16) << 4);
            (prg, chr, mapper, 0u8, 0usize)
        } else {
            (prg_rom_size, chr_rom_size, mapper, submapper, chr_ram_size)
        };

    let chr_is_ram = chr_rom_size == 0;
    let chr_offset = prg_offset + prg_rom_size;

    Some(InesInfo {
        is_nes2,
        mapper,
        submapper,
        prg_rom_size,
        chr_rom_size,
        chr_is_ram,
        has_trainer,
        prg_offset,
        chr_offset,
        mirroring,
        battery,
        chr_ram_size,
        file_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal 16-byte iNES header with the given fields, padded to
    /// `file_len` bytes so size guards have a body to check against.
    fn hdr(prg: u8, chr: u8, b6: u8, b7: u8, extra: &[(usize, u8)], file_len: usize) -> Vec<u8> {
        let mut v = vec![0u8; file_len.max(16)];
        v[0..4].copy_from_slice(&MAGIC);
        v[4] = prg;
        v[5] = chr;
        v[6] = b6;
        v[7] = b7;
        for &(i, b) in extra {
            v[i] = b;
        }
        v
    }

    #[test]
    fn rejects_non_ines() {
        assert!(parse_ines(b"not a rom").is_none());
        assert!(parse_ines(&[0u8; 8]).is_none()); // too short
        let mut bad = vec![0u8; 16];
        bad[0] = b'N';
        assert!(parse_ines(&bad).is_none()); // wrong magic
    }

    #[test]
    fn parses_tmnt_tournament_fighters_layout() {
        // Real TMNT: TF values — PRG=8×16K, CHR=16×8K, mapper 4 (MMC3), no trainer.
        // b6 = 0x40 (low mapper nibble 4), b7 = 0x00.
        let rom = hdr(8, 16, 0x40, 0x00, &[], 16);
        let info = parse_ines(&rom).expect("valid iNES");
        assert!(!info.is_nes2);
        assert_eq!(info.mapper, 4);
        assert_eq!(info.prg_rom_size, 0x20000); // 128 KiB
        assert_eq!(info.chr_rom_size, 0x20000); // 128 KiB
        assert!(!info.chr_is_ram);
        assert_eq!(info.prg_offset, 16);
        assert_eq!(info.chr_offset, 0x20010); // 16 + 128 KiB — the verified CHR offset
        assert_eq!(info.mirroring, "horizontal");
    }

    #[test]
    fn trainer_shifts_offsets_by_512() {
        let rom = hdr(2, 1, 0x04, 0x00, &[], 16); // trainer bit set
        let info = parse_ines(&rom).unwrap();
        assert!(info.has_trainer);
        assert_eq!(info.prg_offset, 16 + 512);
        assert_eq!(info.chr_offset, 16 + 512 + 0x8000);
    }

    #[test]
    fn chr_size_zero_means_chr_ram() {
        let rom = hdr(2, 0, 0x00, 0x00, &[], 16);
        let info = parse_ines(&rom).unwrap();
        assert!(info.chr_is_ram);
        assert_eq!(info.chr_rom_size, 0);
    }

    #[test]
    fn mirroring_and_battery_flags() {
        assert_eq!(parse_ines(&hdr(1, 1, 0x01, 0, &[], 16)).unwrap().mirroring, "vertical");
        assert_eq!(parse_ines(&hdr(1, 1, 0x08, 0, &[], 16)).unwrap().mirroring, "four-screen");
        assert!(parse_ines(&hdr(1, 1, 0x02, 0, &[], 16)).unwrap().battery);
    }

    #[test]
    fn nes2_detection_and_extended_size() {
        // NES 2.0: byte7 bits2-3 = 0b10 (0x08). PRG high nibble in byte9 low,
        // CHR high nibble in byte9 high. Use PRG = (1<<8 | 0)=256 ×16K = 4 MiB,
        // CHR = (0<<8 | 2)=2 ×8K = 16 KiB. mapper low 4 from b6, +submapper.
        let file_len = 16 + 256 * 16384 + 2 * 8192;
        let rom = hdr(0, 2, 0x40, 0x08, &[(9, 0x01), (8, 0x10)], file_len);
        let info = parse_ines(&rom).expect("valid NES2");
        assert!(info.is_nes2);
        assert_eq!(info.prg_rom_size, 256 * 16384);
        assert_eq!(info.chr_rom_size, 2 * 8192);
        assert_eq!(info.mapper, 4); // low nibble 4, high bits 0
        assert_eq!(info.submapper, 1); // byte8 high nibble
    }

    #[test]
    fn nes2_exponent_form_size() {
        // CHR size MSB nibble == 0xF selects exponent form on byte5:
        // byte5 = EEEEEEMM. E=10, MM=0 -> 2^10 * (0*2+1) = 1024 bytes.
        // PRG normal small. Set byte9 high nibble = 0xF for CHR.
        let lsb = (10u8 << 2) | 0; // E=10, MM=0
        let file_len = 16 + 1 * 16384 + 1024 + 10;
        let rom = hdr(1, lsb, 0x00, 0x08, &[(9, 0xF0)], file_len);
        let info = parse_ines(&rom).expect("valid NES2 exponent");
        assert!(info.is_nes2);
        assert_eq!(info.chr_rom_size, 1024);
    }

    #[test]
    fn nes2_size_guard_falls_back_to_ines_when_overrunning_file() {
        // NES2 bits set, but byte9 claims a huge PRG that overruns a tiny file →
        // fall back to plain iNES sizing (byte4/byte5 only).
        let rom = hdr(2, 1, 0x40, 0x08, &[(9, 0x0F)], 64); // tiny file
        let info = parse_ines(&rom).unwrap();
        assert!(!info.is_nes2, "should have fallen back to iNES");
        assert_eq!(info.prg_rom_size, 2 * 16384);
        assert_eq!(info.chr_rom_size, 1 * 8192);
    }
}
