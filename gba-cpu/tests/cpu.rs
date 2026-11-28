use gba_cpu::{Bus, Cpu, ROM_START};

const IWRAM: u32 = 0x0300_0000;
const TIMER: u32 = 0x0400_0100;

fn timer(bus: &mut Bus) -> u16 {
    bus.read_half(TIMER).unwrap()
}

fn start_cycle_counter(bus: &mut Bus) {
    bus.write_half(TIMER + 2, 0x80).unwrap();
    bus.advance(1).unwrap();
}

#[test]
fn arm_swi_and_undefined_enter_distinct_vectors_and_resume() {
    let mut bus = Bus::new(&[0xEF12_0000, 0xE7F0_00F0, 0xE3A0_0042]);
    let mut bios = vec![0; 0x4000];
    for offset in [4, 8] {
        bios[offset..offset + 4].copy_from_slice(&0xE1B0_F00Eu32.to_le_bytes());
    }
    bus.load_bios(bios).unwrap();
    let mut cpu = Cpu::new();
    cpu.step(&mut bus).unwrap();
    assert_eq!((cpu.pc(), cpu.regs[14]), (8, ROM_START + 4));
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!((cpu.pc(), cpu.regs[14]), (4, ROM_START + 8));
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 0x42);
}

#[test]
fn thumb_swi_saves_thumb_state_and_halfword_return_address() {
    let mut bus = Bus::new(&[0xE12F_FF10, 0x2142_DF01]);
    let mut bios = vec![0; 0x4000];
    bios[8..12].copy_from_slice(&0xE1B0_F00Eu32.to_le_bytes());
    bus.load_bios(bios).unwrap();
    let mut cpu = Cpu::new();
    cpu.regs[0] = ROM_START + 5;
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!((cpu.pc(), cpu.regs[14]), (8, ROM_START + 6));
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[1], 0x42);
}

#[test]
fn iwram_arm_branch_to_fallthrough_still_refills_and_multiply_varies() {
    let mut bus = Bus::new(&[]);
    for (index, op) in [0xE1A0_0000, 0x0AFF_FFFF, 0xEAFF_FFFF, 0xE000_0291, 0xE000_0291].into_iter().enumerate() {
        bus.write_word(IWRAM + index as u32 * 4, op).unwrap();
    }
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    cpu.regs[1] = 2;
    cpu.regs[2] = 0xFF;
    start_cycle_counter(&mut bus);
    for expected in [1, 2, 5, 7] {
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus), expected);
    }
    cpu.regs[2] = 0x1234_5678;
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 12);
}

#[test]
fn dma_overlaps_multiply_internal_cycles_but_blocks_next_fetch() {
    const EWRAM: u32 = 0x0200_0000;
    const DMA3: u32 = 0x0400_00D4;
    const DEST: u32 = IWRAM + 0x100;
    const VALUE: u32 = 0xA1B2_C3D4;

    for delay in [1, 0] {
        let mut bus = Bus::new(&[]);
        bus.write_word(IWRAM, 0xE000_0291).unwrap(); // MUL r0, r1, r2。
        bus.write_word(IWRAM + 4, 0xE3A0_3001).unwrap(); // MOV r3, #1。
        bus.write_word(EWRAM, VALUE).unwrap();
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[1] = 2;
        cpu.regs[2] = 0x1234_5678;
        start_cycle_counter(&mut bus);
        let start = timer(&mut bus);
        bus.write_word(DMA3, EWRAM).unwrap();
        bus.write_word(DMA3 + 4, DEST).unwrap();
        bus.write_word(DMA3 + 8, 0x8400_0001).unwrap();
        bus.advance(delay).unwrap();

        // DMA 在第一拍内部运算开始时或下一拍取得总线。
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus) - start, delay as u16 + 5);
        assert_eq!((cpu.pc(), cpu.regs[0]), (IWRAM + 4, 0x2468_ACF0));
        assert_eq!(bus.read_word(DEST), Ok(0));

        // 下一条指令等待 DMA 的两拍启动、六拍读、一拍写及两拍收尾。
        for _ in (delay as u16 + 5)..11 {
            cpu.step(&mut bus).unwrap();
            assert_eq!((cpu.pc(), cpu.regs[3]), (IWRAM + 4, 0));
        }
        assert_eq!(timer(&mut bus) - start, 11);
        assert_eq!(bus.read_word(DEST), Ok(VALUE));
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus) - start, 12);
        assert_eq!((cpu.pc(), cpu.regs[3]), (IWRAM + 8, 1));
    }
}

#[test]
fn cpu_store_and_load_charge_each_bus_transfer() {
    let mut bus = Bus::new(&[]);
    bus.write_word(IWRAM, 0xE581_0000).unwrap(); // STR r0, [r1]。
    bus.write_word(IWRAM + 4, 0xE591_2000).unwrap(); // LDR r2, [r1]。
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = 0x0200_0000;
    start_cycle_counter(&mut bus);
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 7);
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 15);
    assert_eq!(cpu.regs[2], 0x1234_5678);
}

#[test]
fn gamepak_mirrors_and_waitcnt_apply_to_word_fetches() {
    let mut bus = Bus::new(&[0xE1A0_0000; 5]);
    let mut cpu = Cpu::new();
    cpu.regs[15] = 0x0A00_0000;
    start_cycle_counter(&mut bus);
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 10);
    bus.write_half(0x0400_0204, 0x0080).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 14);
}

#[test]
fn halt_waits_for_enabled_interrupt_even_with_ime_clear() {
    let mut bus = Bus::new(&[0xEF02_0000, 0xE3A0_0042]);
    bus.write_half(0x0400_0200, 8).unwrap();
    bus.write_word(TIMER, 0x00C0_FFC0).unwrap();
    let mut cpu = Cpu::new();
    cpu.step(&mut bus).unwrap();
    for _ in 0..80 {
        if bus.read_half(0x0400_0202).unwrap() & 8 != 0 { break; }
        assert_eq!(cpu.pc(), ROM_START + 4);
        cpu.step(&mut bus).unwrap();
    }
    assert_ne!(bus.read_half(0x0400_0202).unwrap() & 8, 0);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 0x42);
}

#[test]
fn stop_pauses_peripheral_clock_until_keypad_interrupt() {
    let mut bus = Bus::new(&[0xEF03_0000, 0xE3A0_0042]);
    bus.write_half(TIMER + 2, 0x80).unwrap();
    bus.write_half(0x0400_0200, 0x1000).unwrap();
    bus.write_half(0x0400_0132, 0x4001).unwrap();
    let mut cpu = Cpu::new();
    cpu.step(&mut bus).unwrap();
    let stopped_at = timer(&mut bus);
    cpu.run_frame(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), stopped_at);
    bus.set_keys(1);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 0x42);
    assert!(timer(&mut bus) > stopped_at);
}

#[test]
fn enabled_gamepak_prefetch_runs_during_multiplier_internal_cycles() {
    fn run(waitcnt: u16) -> (u16, u16) {
        let mut bus = Bus::new(&[0xE000_0291, 0xE1A0_0000, 0xE1A0_0000, 0xE1A0_0000]);
        bus.write_half(0x0400_0204, waitcnt).unwrap();
        start_cycle_counter(&mut bus);
        let mut cpu = Cpu::new();
        cpu.regs[1] = 3;
        cpu.regs[2] = 0x1234_5678;
        cpu.step(&mut bus).unwrap();
        let multiply = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        (multiply, timer(&mut bus) - multiply)
    }
    assert_eq!(run(0), (12, 6));
    assert_eq!(run(0x4000), (10, 2));
}

#[test]
fn gamepak_data_read_keeps_started_opcode_fetch_progress() {
    for (waitcnt, arm_expected, thumb_expected) in [
        (0x0000, 24, 18), (0x4000, 21, 15), (0x4004, 19, 13),
        (0x4010, 17, 13), (0x4014, 15, 11),
    ] {
        // 如 mGBA 时序 ROM，在卡带代码中启动定时器，读卡带数据，再采样定时器。
        // 期望值包含套件报分时扣除的校准周期。
        let rom = [
            0xE580_1000, // STR r1, [r0]。
            0xE593_2000, // LDR r2, [r3]。
            0xE1D0_20B0, // LDRH r2, [r0]。
            0xE12F_FF14, // BX r4，进入 Thumb 用例。
            0x681A_6001, // STR r1, [r0]；LDR r2, [r3]。
            0x46C0_8802, // LDRH r2, [r0]；NOP。
            0x46C0_46C0,
        ];
        for (thumb, expected) in [(false, arm_expected), (true, thumb_expected)] {
            let mut bus = Bus::new(&rom);
            bus.write_half(0x0400_0204, waitcnt).unwrap();
            let mut cpu = Cpu::new();
            cpu.regs[0] = TIMER;
            cpu.regs[1] = 0x0080_0000;
            cpu.regs[3] = ROM_START;
            if thumb {
                cpu.regs[15] = ROM_START + 12;
                cpu.regs[4] = ROM_START + 17;
                cpu.step(&mut bus).unwrap();
            }
            cpu.step(&mut bus).unwrap();
            cpu.step(&mut bus).unwrap();
            assert_eq!(cpu.regs[2], rom[0]);
            cpu.step(&mut bus).unwrap();
            assert_eq!(cpu.regs[2], expected, "Thumb={thumb}，WAITCNT={waitcnt:04X}");
        }
    }
}

#[test]
fn dma_and_gamepak_prefetch_share_the_cartridge_bus() {
    const EWRAM: u32 = 0x0200_0000;
    const DMA3: u32 = 0x0400_00D4;

    for (rom, waitcnt, source, dest, dma_cycles, first_fetch, next_fetch) in [
        (ROM_START, 0x4000, IWRAM, IWRAM + 4, 6, 1, 5),
        (0x0C00_0000, 0x4000, IWRAM, IWRAM + 4, 6, 12, 18),
        (ROM_START, 0x4000, ROM_START, IWRAM + 4, 10, 8, 6),
        (ROM_START, 0x4000, EWRAM, ROM_START, 12, 8, 6),
        (ROM_START, 0x0000, IWRAM, IWRAM + 4, 6, 8, 6),
    ] {
        let mut bus = Bus::new(&[0xE1A0_0000; 5]);
        let mut cpu = Cpu::new();
        cpu.regs[15] = rom;
        bus.write_half(0x0400_0204, waitcnt).unwrap();
        start_cycle_counter(&mut bus);
        cpu.step(&mut bus).unwrap();
        let before_dma = timer(&mut bus);

        bus.write_word(DMA3, source).unwrap();
        bus.write_word(DMA3 + 4, dest).unwrap();
        bus.write_word(DMA3 + 8, 0x8000_0001).unwrap();
        while bus.read_half(DMA3 + 10).unwrap() & 0x8000 != 0 {
            assert!(timer(&mut bus) - before_dma < 20, "DMA3 未结束");
            bus.advance(1).unwrap();
        }
        let after_dma = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        let after_first = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(
            (after_dma - before_dma, after_first - after_dma, timer(&mut bus) - after_first),
            (dma_cycles, first_fetch, next_fetch),
            "WAITCNT={waitcnt:04X}, DMA {source:08X} → {dest:08X}",
        );
    }
}

#[test]
fn arm_pipeline_keeps_prefetched_opcode_until_branch_refill() {
    let mut bus = Bus::new(&[]);
    bus.write_word(IWRAM, 0xE581_0000).unwrap(); // STR r0, [r1]。
    bus.write_word(IWRAM + 4, 0xE3A0_2001).unwrap(); // MOV r2,#1。
    bus.write_word(IWRAM + 8, 0xEAFF_FFFD).unwrap(); // B 到第二条指令。
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    cpu.regs[0] = 0xE3A0_2002; // MOV r2,#2。
    cpu.regs[1] = IWRAM + 4;
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[2], 1);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[2], 2);
}

#[test]
fn multiply_long_exposes_arm7_hardware_carry() {
    for (instruction, a, b, flags) in [
        (0xE0D1_0392, 0, 0x8000_0000, 6), // SMULLS：零结果，Z 和 C 同时置位。
        (0xE091_0392, u32::MAX, u32::MAX, 10), // UMULLS：最高位为 1，N 和 C 同时置位。
    ] {
        let mut bus = Bus::new(&[instruction, 0xE10F_4000]); // MRS r4,CPSR。
        let mut cpu = Cpu::new();
        cpu.regs[2] = a;
        cpu.regs[3] = b;
        cpu.step(&mut bus).unwrap();
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.regs[4] >> 28, flags);
    }
}

#[test]
fn timer_irq_reaches_cpu_after_seven_cycles() {
    let mut bus = Bus::new(&[]);
    bus.load_bios(vec![0; 0x4000]).unwrap();
    for pc in (IWRAM..IWRAM + 40).step_by(4) {
        bus.write_word(pc, 0xE1A0_0000).unwrap();
    }
    bus.write_half(0x0400_0200, 8).unwrap();
    bus.write_byte(0x0400_0208, 1).unwrap();
    bus.write_word(TIMER, 0x00C0_FFFF).unwrap();
    bus.advance(2).unwrap();
    bus.write_half(TIMER + 2, 0).unwrap();
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    for index in 1..=7 {
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc(), IWRAM + index * 4);
    }
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc(), 0x18);
    assert_eq!(cpu.regs[14], IWRAM + 32);
}

#[test]
fn thumb_multiply_timing_depends_on_old_destination() {
    let mut bus = Bus::new(&[]);
    bus.write_word(IWRAM, 0xE12F_FF12).unwrap(); // BX r2。
    bus.write_half(IWRAM + 4, 0x4348).unwrap(); // MULS r0,r1。
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    cpu.regs[0] = 0x1234_5678;
    cpu.regs[1] = 1;
    cpu.regs[2] = IWRAM + 5;
    start_cycle_counter(&mut bus);
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 3);
    cpu.step(&mut bus).unwrap();
    assert_eq!(timer(&mut bus), 8);
    assert_eq!(cpu.regs[0], 0x1234_5678);
}

#[test]
fn cartridge_load_reads_bios_latch_and_unpopulated_rom_pattern() {
    let mut bios = vec![0; 0x4000];
    bios[0..4].copy_from_slice(&0xE12F_FF10u32.to_le_bytes()); // BX r0。
    bios[8..12].copy_from_slice(&0xAABB_CCDDu32.to_le_bytes());
    let mut bus = Bus::new(&[0xE591_2000, 0xE593_4000, 0xE595_6000]);
    bus.load_bios(bios).unwrap();
    bus.write_byte(0x0E00_0003, 0xA7).unwrap();
    let mut cpu = Cpu::from_reset();
    cpu.regs[0] = ROM_START;
    cpu.regs[3] = ROM_START + 0x100;
    cpu.regs[5] = 0x0E00_0003;
    for _ in 0..4 { cpu.step(&mut bus).unwrap(); }
    assert_eq!((cpu.regs[2], cpu.regs[4], cpu.regs[6]), (0xAABB_CCDD, 0x0081_0080, 0xA7A7_A7A7));
}

#[test]
fn guest_timer_read_samples_before_the_last_bus_tick() {
    let mut bus = Bus::new(&[]);
    bus.write_word(IWRAM, 0xE581_0000).unwrap(); // STR r0,[r1]。
    bus.write_word(IWRAM + 4, 0xE1D1_20B0).unwrap(); // LDRH r2,[r1]。
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    cpu.regs[0] = 0x0080_0000;
    cpu.regs[1] = TIMER;
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[2], 0);
    assert_eq!(timer(&mut bus), 2);
}

#[test]
fn video_memory_mirrors_share_storage_and_keep_obj_byte_write_rules() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0601_8000, 0xCAFE_BABE).unwrap();
    assert_eq!(bus.read_word(0x0601_0000), Ok(0xCAFE_BABE));
    assert_eq!(bus.read_half(0x0603_8002), Ok(0xCAFE));
    bus.write_byte(0x0603_8002, 0xAA).unwrap();
    assert_eq!(bus.read_half(0x0601_0002), Ok(0xCAFE));
}

#[test]
fn bitmap_vram_access_waits_for_background_fetch_to_finish() {
    const VRAM: u32 = 0x0600_0000;
    for (control, address, dot, store, cycles) in [
        (0x0403, VRAM, 33, 0xE1C1_00B0, 3), // STRH 的访问落在第 35 拍。
        (0x0403, VRAM, 34, 0xE1C1_00B0, 2),
        (0x0403, VRAM, 32, 0xE581_0000, 4), // STR 的第二个半字落在第 35 拍。
        (0x0403, VRAM, 33, 0xE1D1_00B0, 4), // LDRH 额外有一拍内部周期。
        (0x0404, VRAM, 33, 0xE1C1_00B0, 3),
        (0x0405, VRAM, 33, 0xE1C1_00B0, 3),
        (0x0483, VRAM, 33, 0xE1C1_00B0, 2), // 强制空白。
        (0x0003, VRAM, 33, 0xE1C1_00B0, 2), // BG2 关闭。
        (0x0403, VRAM + 0x14000, 33, 0xE1C1_00B0, 2), // 精灵区。
        (0x0403, VRAM, 1009, 0xE1C1_00B0, 2), // HBlank。
        (0x0403, VRAM, 160 * 1232 + 33, 0xE1C1_00B0, 2), // VBlank。
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, control).unwrap();
        bus.write_word(IWRAM, store).unwrap();
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[0] = 0xA1B2_C3D4;
        cpu.regs[1] = address;
        start_cycle_counter(&mut bus);
        bus.advance(dot - 1).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), cycles, "DISPCNT={control:04X}, 点={dot}");
    }
}

#[test]
fn affine_vram_access_waits_for_free_halfword_slots() {
    const VRAM: u32 = 0x0600_0000;
    for (control, address, dot, store, cycles) in [
        (0x0802, VRAM, 30, 0xE1C1_00B0, 4), // BG3 在第 32、33 拍读取。
        (0x0402, VRAM, 30, 0xE1C1_00B0, 2),
        (0x0402, VRAM, 32, 0xE1C1_00B0, 4), // BG2 在第 34、35 拍读取。
        (0x0802, VRAM, 32, 0xE1C1_00B0, 2),
        (0x0402, VRAM, 31, 0xE581_0000, 5), // 第一个半字在第 33 拍，第二个要等到第 36 拍。
        (0x0C02, VRAM, 30, 0xE1C1_00B0, 976), // 只有到第 1006 拍才取得总线。
        (0x0C02, VRAM, 1003, 0xE581_0000, 4),
        (0x0C82, VRAM, 30, 0xE1C1_00B0, 2),
        (0x0C02, VRAM + 0x10000, 30, 0xE1C1_00B0, 2),
        (0x0C02, VRAM, 160 * 1232 + 30, 0xE1C1_00B0, 2),
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, control).unwrap();
        bus.write_word(IWRAM, store).unwrap();
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[0] = 0xA1B2_C3D4;
        cpu.regs[1] = address;
        start_cycle_counter(&mut bus);
        bus.advance(dot - 1).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), cycles, "DISPCNT={control:04X}, 点={dot}");
        assert_eq!(bus.read_half(address), Ok(0xC3D4));
    }
}

#[test]
fn text_vram_access_accounts_for_color_depth_scroll_and_fetch_tail() {
    const VRAM: u32 = 0x0600_0000;
    for (display, layer, bpp8, scroll, dot, address, cycles) in [
        (0x0100, 0, false, 0, 30, VRAM, 3), // 第 32 拍读图块编号。
        (0x0100, 0, false, 0, 42, VRAM, 2), // 4bpp 在第 44 拍空闲。
        (0x0100, 0, true, 0, 42, VRAM, 3),  // 8bpp 在第 44 拍还要读数据。
        (0x0400, 2, false, 7, 4, VRAM, 3),  // 滚动让 BG2 的首次读取提前到第 6 拍。
        (0x0100, 0, false, 8, 30, VRAM, 3), // 滚动整块后取数相位恢复。
        (0x0200, 1, true, 0, 1019, VRAM, 3), // 已开始的图块数据读到消隐内。
        (0x0400, 2, false, 5, 1004, VRAM, 3), // 第 1006 拍只读图块编号。
        (0x0400, 2, false, 5, 1008, VRAM, 2),
        (0x0F00, 0, false, 0, 30, VRAM, 10), // 四层先取编号再取数据，占用第 32–39 拍。
        (0x0180, 0, false, 0, 30, VRAM, 2),
        (0x0100, 0, false, 0, 30, VRAM + 0x18000, 2),
        (0x0100, 0, false, 0, 160 * 1232 + 30, VRAM, 2),
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, display).unwrap();
        bus.write_half(0x0400_0008 + layer * 2, if bpp8 { 0x80 } else { 0 }).unwrap();
        bus.write_half(0x0400_0010 + layer * 4, scroll).unwrap();
        bus.write_word(IWRAM, 0xE1C1_00B0).unwrap(); // STRH r0, [r1]
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[0] = 0x1234;
        cpu.regs[1] = address;
        start_cycle_counter(&mut bus);
        bus.advance(dot - 1).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), cycles, "DISPCNT={display:04X}, 点={dot}");
        assert_eq!(bus.read_half(address), Ok(0x1234));
    }
}

#[test]
fn mixed_backgrounds_share_vram_slots_in_mode_one() {
    const VRAM: u32 = 0x0600_0000;
    for (mode, dot, opcode, cycles) in [
        (0x0101, 30, 0xE1C1_00B0, 3), // BG0 占用第 32 拍。
        (0x0201, 31, 0xE1C1_00B0, 3), // BG1 占用第 33 拍。
        (0x0401, 32, 0xE1C1_00B0, 4), // BG2 占用第 34、35 拍。
        (0x0701, 30, 0xE1C1_00B0, 10), // 三层连续占用第 32–39 拍。
        (0x0701, 30, 0xE581_0000, 11), // 整字使用第 40、41 拍。
        (0x0801, 32, 0xE1C1_00B0, 2), // Mode 1 不显示 BG3。
        (0x0781, 30, 0xE1C1_00B0, 2), // 强制空白。
        (0x0401, 1004, 0xE1C1_00B0, 2), // 仿射背景进入消隐后释放总线。
        (0x0201, 1019, 0xE1C1_00B0, 3), // 普通背景可继续完成图块读取。
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, mode).unwrap();
        bus.write_half(0x0400_000A, if dot == 1019 { 0x80 } else { 0 }).unwrap();
        bus.write_word(IWRAM, opcode).unwrap();
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[0] = 0xABCD_1234;
        cpu.regs[1] = VRAM;
        start_cycle_counter(&mut bus);
        bus.advance(dot - 1).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), cycles, "DISPCNT={mode:04X}, 点={dot}");
        assert_eq!(bus.read_half(VRAM), Ok(0x1234));
    }
}

#[test]
fn sprite_fetches_arbitrate_oam_and_vram_separately() {
    const OAM: u32 = 0x0700_0000;
    const OBJ: u32 = 0x0601_0000;
    for (control, address, dot, attr, expected) in [
        (0x1000, OAM, 38, 0, 3),
        (0x1000, OBJ, 38, 0, 2),
        (0x1000, OAM, 44, 0, 2),
        (0x1000, OBJ, 42, 0, 3),
        (0x1000, OBJ, 42, 0x0200, 2),
        (0x1000, OAM, 40, 0x0200, 3),
        (0x1080, OAM, 38, 0, 3), // 强制空白期间精灵取数仍由 OBJ 启用位控制。
        (0x1080, OBJ, 42, 0, 3),
        (0x1000, OAM, 160 * 1232 + 38, 0, 2),
        (0x1000, OAM, 227 * 1232 + 38, 0, 3),
        (0x1000, OAM, 1002, 0, 3),
        (0x1020, OAM, 1002, 0, 2),
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, control).unwrap();
        bus.write_half(OAM, attr).unwrap();
        bus.write_word(IWRAM, 0xE1C1_00B0).unwrap(); // STRH r0, [r1]
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[0] = 0xABCD;
        cpu.regs[1] = address;
        start_cycle_counter(&mut bus);
        bus.advance(dot - 1).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), expected, "地址 {address:08X}，第 {dot} 拍");
        assert_eq!(bus.read_half(address), Ok(0xABCD));
    }
}

#[test]
fn background_fetch_enable_is_delayed_and_disable_is_immediate() {
    const DISPLAY: u32 = 0x0400_0000;
    const VRAM: u32 = 0x0600_0000;
    for (initial, change_at, toggle, access_at, expected) in [
        (0x0003, 20, 0x0403, 33, 2),
        (0x0003, 20, 0x0403, 2 * 1232 + 33, 2),
        (0x0003, 20, 0x0403, 2 * 1232 + 41, 3),
        (0x0003, 41, 0x0403, 2 * 1232 + 41, 2),
        (0x0003, 41, 0x0403, 3 * 1232 + 41, 3),
        (0x0403, 20, 0x0003, 33, 2),
        (0x0483, 20, 0x0403, 33, 2),
        (0x0483, 20, 0x0403, 2 * 1232 + 41, 3),
        (0x0403, 20, 0x0483, 33, 2),
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(DISPLAY, initial).unwrap();
        bus.write_word(IWRAM, 0xE1C1_00B0).unwrap();
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[1] = VRAM;
        start_cycle_counter(&mut bus);
        bus.advance(change_at - 1).unwrap();
        bus.write_half(DISPLAY, toggle).unwrap();
        bus.advance(access_at - change_at).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), expected, "写入第 {change_at} 拍，测试第 {access_at} 拍");
    }
}

#[test]
fn palette_bus_waits_only_for_colors_used_by_the_mixer() {
    const PALETTE: u32 = 0x0500_0000;
    for (display, blend, dot, opcode, expected) in [
        (0, 0, 45, 0xE1C1_00B0, 3), // 第 47 拍读取底色。
        (0, 0, 46, 0xE1C1_00B0, 2),
        (0x80, 0, 45, 0xE1C1_00B0, 2),
        (0x0403, 0, 45, 0xE1C1_00B0, 2), // Mode 3 的颜色直接来自显存。
        (0x0403, 0x2044, 47, 0xE1C1_00B0, 3), // 混合时才读取下层底色。
        (0x0403, 0x2044, 46, 0xE581_0000, 4), // 32 位访问的第二个半字撞上读取。
        (0, 0, 160 * 1232 + 45, 0xE1C1_00B0, 2),
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, display).unwrap();
        bus.write_half(0x0400_0050, blend).unwrap();
        bus.write_word(IWRAM, opcode).unwrap();
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[1] = PALETTE;
        start_cycle_counter(&mut bus);
        bus.advance(dot - 1).unwrap();
        let before = timer(&mut bus);
        cpu.step(&mut bus).unwrap();
        assert_eq!(timer(&mut bus).wrapping_sub(before), expected, "DISPCNT={display:04X}，第 {dot} 拍");
    }
}

#[test]
fn block_transfers_keep_sram_byte_lanes_but_align_work_ram() {
    const SRAM: u32 = 0x0E00_0000;
    const RAM: u32 = 0x0200_0000;
    for thumb in [false, true] {
        for region in [SRAM, SRAM + 0x0100_0000, RAM] {
            for lane in 1..=3 {
                let mut bus = Bus::new(&[]);
                for (offset, value) in [0x47, 0x61, 0x6D, 0x65, 0x12, 0x34, 0x56, 0x78].into_iter().enumerate() {
                    bus.write_byte(region + offset as u32, value).unwrap();
                }
                let mut cpu = Cpu::new();
                cpu.regs[15] = IWRAM;
                if thumb {
                    bus.write_word(IWRAM, 0xE12F_FF13).unwrap(); // BX r3
                    bus.write_half(IWRAM + 4, 0xC905).unwrap(); // LDMIA r1!, {r0,r2}
                    bus.write_half(IWRAM + 6, 0xC105).unwrap(); // STMIA r1!, {r0,r2}
                    cpu.regs[3] = IWRAM + 5;
                    cpu.step(&mut bus).unwrap();
                } else {
                    bus.write_word(IWRAM, 0xE8B1_0005).unwrap(); // LDMIA r1!, {r0,r2}
                    bus.write_word(IWRAM + 4, 0xE8A1_0005).unwrap(); // STMIA r1!, {r0,r2}
                }
                cpu.regs[1] = region + lane;
                cpu.step(&mut bus).unwrap();
                let expected = if region == RAM {
                    [0x656D_6147, 0x7856_3412]
                } else {
                    [
                        u32::from(bus.read_byte(region + lane).unwrap()) * 0x0101_0101,
                        u32::from(bus.read_byte(region + 4 + lane).unwrap()) * 0x0101_0101,
                    ]
                };
                assert_eq!([cpu.regs[0], cpu.regs[2]], expected, "Thumb={thumb}，地址 {region:08x}+{lane}");
                assert_eq!(cpu.regs[1], region + lane + 8);
                cpu.regs[1] = region + lane;
                cpu.regs[0] = 0xDDCC_BBAA;
                cpu.regs[2] = 0x8877_6655;
                cpu.step(&mut bus).unwrap();
                assert_eq!(cpu.regs[1], region + lane + 8);
                if region == RAM {
                    assert_eq!(bus.read_word(region), Ok(0xDDCC_BBAA));
                    assert_eq!(bus.read_word(region + 4), Ok(0x8877_6655));
                } else {
                    assert_eq!(bus.read_byte(region), Ok(0x47));
                    assert_eq!(bus.read_byte(region + 4), Ok(0x12));
                    assert_eq!(bus.read_byte(region + lane), Ok((0xDDCC_BBAAu32 >> (lane * 8)) as u8));
                    assert_eq!(bus.read_byte(region + 4 + lane), Ok((0x8877_6655u32 >> (lane * 8)) as u8));
                }
            }
        }
    }
}

#[test]
fn bitmap_mode_blocks_mirrors_into_bitmap_vram() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0601_0000, 0xAABB_CCDD).unwrap();
    bus.write_word(0x0601_4000, 0x1122_3344).unwrap();
    for mode in [3, 4, 5] {
        bus.write_half(0x0400_0000, mode).unwrap();
        assert_eq!(bus.read_byte(0x0601_8000), Ok(0));
        assert_eq!(bus.read_half(0x0603_8002), Ok(0));
        assert_eq!(bus.read_word(0x0601_8000), Ok(0));
        assert_eq!(bus.read_word(0x0601_C000), Ok(0x1122_3344));
        bus.write_byte(0x0603_8001, 0x55).unwrap();
        bus.write_half(0x0601_8002, 0x7788).unwrap();
        bus.write_word(0x0601_8000, 0).unwrap();
        assert_eq!(bus.read_word(0x0601_0000), Ok(0xAABB_CCDD));
    }
    bus.write_half(0x0400_0000, 0).unwrap();
    assert_eq!(bus.read_word(0x0601_8000), Ok(0xAABB_CCDD));
}
