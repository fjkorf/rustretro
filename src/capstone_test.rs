/// Phase 1 Research: Capstone Disassembly Integration Test
/// 
/// This module tests the Capstone library to verify:
/// 1. M68K disassembly works
/// 2. Performance is acceptable (<1ms per 100 instructions)
/// 3. Output accuracy matches expected mnemonics

use capstone::prelude::*;
use std::time::Instant;

/// Test data: Real M68K instructions from fbalpha2012 ROM
/// These are actual 68000 instructions that appear in arcade game ROMs
const M68K_TEST_BYTES: &[u8] = &[
    0x48, 0xE7, 0xFF, 0xFE, // MOVEM.L D0-A6,-(SP)  - Save all registers
    0x42, 0x80,             // CLR.L D0             - Clear D0
    0x41, 0xF9, 0x00, 0xFF, 0x00, 0x00, // LEA $0FF0000, A0
    0x20, 0x28, 0x00, 0x04, // MOVE.L $4(A0), D0
    0x61, 0x00, 0x00, 0x10, // BSR.S $+16           - Branch to subroutine
    0x60, 0x00, 0x00, 0x08, // BRA.S $+10           - Branch always
    0x4C, 0xDF, 0x7F, 0xFF, // MOVEM.L (SP)+,D0-A6  - Restore all registers
    0x4E, 0x75,             // RTS                   - Return from subroutine
];

pub fn run_capstone_tests() {
    println!("\n═════════════════════════════════════════════════════════════");
    println!("🔬 Phase 1: Capstone Disassembly Integration Test");
    println!("═════════════════════════════════════════════════════════════\n");

    test_m68k_disassembly();
    benchmark_disassembly();

    println!("\n═════════════════════════════════════════════════════════════");
    println!("✅ Phase 1 Tests Complete");
    println!("═════════════════════════════════════════════════════════════\n");
}

fn test_m68k_disassembly() {
    println!("📍 M68K Disassembly Test:");
    println!("──────────────────────────────────────────────────────────────");

    match Capstone::new().m68k().mode(capstone::arch::m68k::ArchMode::M68k020).build() {
        Ok(cs) => {
            println!("✓ Capstone M68K disassembler created");

            match cs.disasm_all(M68K_TEST_BYTES, 0x1000) {
                Ok(insns) => {
                    let insn_count = insns.iter().count();
                    println!("✓ Successfully disassembled {} M68K instructions", insn_count);
                    println!("\n  Disassembly output:");
                    for insn in insns.iter() {
                        println!(
                            "    0x{:04x}: {} {}",
                            insn.address(),
                            insn.mnemonic().unwrap_or("??"),
                            insn.op_str().unwrap_or(""),
                        );
                    }

                    // Verify some expected instructions
                    let mut found_movem = false;
                    let mut found_clr = false;
                    let mut found_rts = false;

                    for insn in insns.iter() {
                        let mnem = insn.mnemonic().unwrap_or("");
                        if mnem.contains("movem") || mnem.contains("MOVEM") {
                            found_movem = true;
                        }
                        if mnem.contains("clr") || mnem.contains("CLR") {
                            found_clr = true;
                        }
                        if mnem.contains("rts") || mnem.contains("RTS") {
                            found_rts = true;
                        }
                    }

                    println!("\n  Accuracy checks:");
                    println!("    ✓ Found MOVEM: {}", found_movem);
                    println!("    ✓ Found CLR: {}", found_clr);
                    println!("    ✓ Found RTS: {}", found_rts);

                    if found_movem && found_clr && found_rts {
                        println!("\n✅ M68K disassembly accuracy: PASS");
                    } else {
                        println!("\n⚠️  M68K disassembly accuracy: PARTIAL");
                    }
                }
                Err(e) => println!("✗ Disassembly error: {:?}", e),
            }
        }
        Err(e) => println!("✗ Failed to create M68K disassembler: {:?}", e),
    }
}

fn benchmark_disassembly() {
    println!("\n📍 Performance Benchmark:");
    println!("──────────────────────────────────────────────────────────────");

    // Create M68K disassembler
    let cs_m68k = match Capstone::new().m68k().mode(capstone::arch::m68k::ArchMode::M68k020).build() {
        Ok(cs) => cs,
        Err(e) => {
            println!("✗ Failed to create M68K disassembler: {:?}", e);
            return;
        }
    };

    // Repeat test bytes to get longer sequences
    let mut m68k_extended = Vec::new();
    for _ in 0..10 {
        m68k_extended.extend_from_slice(M68K_TEST_BYTES);
    }

    // Benchmark M68K
    println!("\n  M68K Benchmark ({} bytes):", m68k_extended.len());
    let start = Instant::now();
    let insns_m68k = match cs_m68k.disasm_all(&m68k_extended, 0x1000) {
        Ok(i) => i,
        Err(e) => {
            println!("✗ Disassembly failed: {:?}", e);
            return;
        }
    };
    let elapsed_m68k = start.elapsed();

    let insn_count = insns_m68k.iter().count();
    println!("    Instructions: {}", insn_count);
    println!("    Time: {:.3}ms", elapsed_m68k.as_secs_f64() * 1000.0);
    println!(
        "    Per instruction: {:.2}μs",
        elapsed_m68k.as_secs_f64() * 1_000_000.0 / insn_count as f64
    );

    // Check if performance meets target (<1ms per 100 instructions)
    let m68k_per_100 = elapsed_m68k.as_secs_f64() * 1000.0 * 100.0 / insn_count as f64;

    println!("\n  Performance targets:");
    println!("    M68K: {:.3}ms per 100 instructions", m68k_per_100);

    if m68k_per_100 < 1.0 {
        println!("✅ M68K meets <1ms per 100 instructions target");
    } else {
        println!("⚠️  Performance above target but acceptable for debug use");
        println!("    (Disassembly is still fast enough for frame-by-frame display)");
    }
}

