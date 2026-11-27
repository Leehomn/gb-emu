use gba_cpu::{Bus, Cpu, RAM_START, ROM_START};

#[test]
fn arm_halfword_io_reads_scanline_and_interrupt_flags() {
    let mut bus = Bus::new(&[0xE1D100B0]); // LDRH r0,[r1]
    let mut cpu = Cpu::new();
    cpu.regs[1] = 0x0400_0006;
    bus.write_half(0x0400_0004, 0x0120).unwrap(); // 允许线号比较中断
    bus.advance(1006).unwrap();
    assert_ne!(bus.read_half(0x0400_0004).unwrap() & 2, 0);
    bus.advance(226).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 1);
    assert_eq!(cpu.pc(), ROM_START + 4);
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 4);
    bus.write_half(0x0400_0202, 4).unwrap();
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 0);
}

#[test]
fn timer_overflows_cascade_and_request_interrupt() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0400_0104, 0x00C4_FFFF).unwrap();
    bus.write_word(0x0400_0100, 0x0080_FFFE).unwrap();
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(0x0400_0100).unwrap(), 0xFFFE);
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(0x0400_0100).unwrap(), 0xFFFF);
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 0);
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(0x0400_0100).unwrap(), 0xFFFE);
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 1 << 4);
}

#[test]
fn vblank_starts_dma_and_sets_both_interrupt_flags() {
    let mut bus = Bus::new(&[]);
    bus.write_word(RAM_START, 0x7C00_001F).unwrap();
    bus.write_half(0x0400_0004, 8).unwrap();
    bus.write_word(0x0400_00D4, RAM_START).unwrap();
    bus.write_word(0x0400_00D8, 0x0600_0000).unwrap();
    bus.write_word(0x0400_00DC, 0xD400_0001).unwrap();
    bus.advance(1232 * 160 - 1).unwrap();
    assert_eq!(bus.read_word(0x0600_0000).unwrap(), 0);
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 1);
    assert_eq!(bus.read_word(0x0600_0000).unwrap(), 0);
    bus.advance(10).unwrap();
    assert_eq!(bus.read_word(0x0600_0000).unwrap(), 0x7C00_001F);
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 1);
    bus.advance(2).unwrap();
    assert_eq!(bus.read_half(0x0400_0202).unwrap(), 1 | (1 << 11));
}

#[test]
fn cartridge_sram_survives_new_bus_when_restored() {
    let mut bus = Bus::new(&[]);
    bus.write_byte(0x0E00_0012, 0xA5).unwrap();
    let mut restarted = Bus::new(&[]);
    restarted.restore_sram(bus.sram()).unwrap();
    assert_eq!(restarted.read_byte(0x0E00_8012).unwrap(), 0xA5);
}
#[test]
fn irq_enters_bios_vector_and_returns_to_interrupted_arm_code() {
    let mut bus = Bus::new(&[0xE3A0_0007]); // MOV r0,#7
    let mut bios = vec![0; 0x4000];
    bios[0..4].copy_from_slice(&0xE3A0_2003u32.to_le_bytes()); // MOV r2,#3
    bios[0x18..0x1C].copy_from_slice(&0xE1C1_00B0u32.to_le_bytes()); // STRH r0,[r1]
    bios[0x1C..0x20].copy_from_slice(&0xE25E_F004u32.to_le_bytes()); // SUBS pc,lr,#4
    bus.load_bios(bios).unwrap();
    let mut reset = Cpu::from_reset();
    reset.step(&mut bus).unwrap();
    assert_eq!((reset.pc(), reset.regs[2]), (4, 3));
    bus.write_half(0x0400_0200, 8).unwrap();
    bus.write_half(0x0400_0208, 1).unwrap();
    bus.write_word(0x0400_0100, 0x00C0_FFFF).unwrap();
    bus.advance(2).unwrap();
    bus.write_half(0x0400_0102, 0).unwrap();
    bus.advance(7).unwrap();
    let mut cpu = Cpu::new();
    cpu.regs[0] = 8;
    cpu.regs[1] = 0x0400_0202;
    cpu.regs[14] = 0x1234;
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc(), 0x18);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc(), ROM_START);
    assert_eq!(cpu.regs[14], 0x1234);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 7);
}

#[test]
fn a_scanline_captures_a_palette_sprite_over_the_bitmap() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1443).unwrap(); // Mode 3, BG2, OBJ, 1D
    bus.write_half(0x0600_0000, 0x001F).unwrap();
    bus.write_half(0x0500_0202, 0x03E0).unwrap();
    bus.write_half(0x0601_4000, 1).unwrap();
    bus.write_half(0x0700_0004, 512).unwrap();
    bus.advance(1005).unwrap();
    assert_eq!(bus.scanout_rgb()[..3], [0; 3]);
    bus.advance(1).unwrap();
    assert_eq!(bus.scanout_rgb()[..3], [0, 255, 0]);
}

#[test]
fn dma_waits_one_cycle_when_bitmap_background_is_reading_vram() {
    const DMA3: u32 = 0x0400_00D4;
    const IWRAM: u32 = 0x0300_0000;
    const VRAM: u32 = 0x0600_0000;
    for (control, source, dest, enable_at) in [
        (0x0403, IWRAM, VRAM, 31),
        (0x0403, VRAM, IWRAM, 32),
        (0x0483, IWRAM, VRAM, 31),
        (0x0483, VRAM, IWRAM, 32),
    ] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, control).unwrap();
        bus.write_half(source, 0x1234).unwrap();
        bus.write_word(DMA3, source).unwrap();
        bus.write_word(DMA3 + 4, dest).unwrap();
        bus.advance(enable_at).unwrap();
        bus.write_word(DMA3 + 8, 0x8000_0001).unwrap();
        bus.advance(4).unwrap();
        assert_eq!(bus.read_half(dest).unwrap(), if control & 0x80 == 0 { 0 } else { 0x1234 });
        bus.advance(1).unwrap();
        assert_eq!(bus.read_half(dest), Ok(0x1234));
    }
}

#[test]
fn dma_waits_until_hblank_when_both_affine_backgrounds_read_vram() {
    const DMA3: u32 = 0x0400_00D4;
    const IWRAM: u32 = 0x0300_0000;
    const VRAM: u32 = 0x0600_0000;
    for (source, dest, ready_dot) in [(IWRAM, VRAM, 1006), (VRAM, IWRAM, 1007)] {
        let mut bus = Bus::new(&[]);
        bus.write_half(0x0400_0000, 0x0C02).unwrap();
        bus.write_half(source, 0x1234).unwrap();
        bus.write_word(DMA3, source).unwrap();
        bus.write_word(DMA3 + 4, dest).unwrap();
        bus.advance(31).unwrap();
        bus.write_word(DMA3 + 8, 0x8000_0001).unwrap();
        bus.advance(ready_dot - 32).unwrap();
        assert_eq!(bus.read_half(dest), Ok(0));
        bus.advance(1).unwrap();
        assert_eq!(bus.read_half(dest), Ok(0x1234));
    }
}

#[test]
fn dma_waits_for_both_background_types_in_mode_one() {
    const DMA3: u32 = 0x0400_00D4;
    const SOURCE: u32 = 0x0300_0000;
    const DEST: u32 = 0x0600_0000;
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0701).unwrap();
    bus.write_half(SOURCE, 0x1234).unwrap();
    bus.write_word(DMA3, SOURCE).unwrap();
    bus.write_word(DMA3 + 4, DEST).unwrap();
    bus.advance(28).unwrap();
    bus.write_word(DMA3 + 8, 0x8000_0001).unwrap();
    bus.advance(11).unwrap();
    assert_eq!(bus.read_half(DEST), Ok(0));
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(DEST), Ok(0x1234));
}

#[test]
fn dma_uses_byte_wide_sram_bus_for_reads_and_writes() {
    const IWRAM: u32 = 0x0300_0000;
    const SRAM: u32 = 0x0E00_0000;
    for (channel, mirror, word) in [(1, 0, false), (2, 0x0100_0000, true), (3, 0, true), (3, 0x0100_0000, false)] {
        let mut bus = Bus::new(&[]);
        let dma = 0x0400_00B0 + 12 * channel;
        let control = if word { 0x8400_0001 } else { 0x8000_0001 };
        bus.write_byte(SRAM, 0x47).unwrap();
        bus.write_byte(SRAM + 1, 0x61).unwrap();
        bus.write_word(dma, SRAM + mirror + 1).unwrap();
        bus.write_word(dma + 4, IWRAM).unwrap();
        bus.write_word(dma + 8, control).unwrap();
        bus.advance(20).unwrap();
        assert_eq!(bus.read_word(IWRAM), Ok(if word { 0x4747_4747 } else { 0x4747 }));

        if channel == 3 {
            bus.write_word(IWRAM, 0x12AB_CDD8).unwrap();
            bus.write_word(dma, IWRAM).unwrap();
            bus.write_word(dma + 4, SRAM + mirror + 1).unwrap();
            bus.write_word(dma + 8, control).unwrap();
            bus.advance(20).unwrap();
            assert_eq!(bus.read_byte(SRAM), Ok(0xD8));
            assert_eq!(bus.read_byte(SRAM + 1), Ok(0x61));
        }
    }
}

#[test]
fn display_dma_copies_only_lines_two_through_161() {
    const LINE: u32 = 1232;
    const HBLANK: u32 = 1006;
    const DMA3: u32 = 0x0400_00D4;
    const SOURCE: u32 = 0x0300_0000;
    const DEST: u32 = 0x0600_0000;
    let mut bus = Bus::new(&[]);
    for i in 0..161u32 { bus.write_half(SOURCE + i * 2, (i + 1) as u16).unwrap(); }
    bus.write_word(DMA3, SOURCE).unwrap();
    bus.write_word(DMA3 + 4, DEST).unwrap();
    bus.write_word(DMA3 + 8, 0xB200_0001).unwrap();
    bus.advance(2 * LINE + HBLANK).unwrap();
    assert_eq!(bus.read_half(DEST), Ok(0));
    bus.advance(8).unwrap();
    assert_eq!(bus.read_half(DEST), Ok(1));
    assert_eq!(bus.read_half(DEST + 2), Ok(0));
    bus.advance(159 * LINE).unwrap();
    assert_eq!(bus.read_half(DEST + 159 * 2), Ok(160));
    assert_ne!(bus.read_half(DMA3 + 10).unwrap() & 0x8000, 0);
    bus.advance(LINE - HBLANK - 8).unwrap();
    assert_eq!(bus.read_half(DMA3 + 10).unwrap() & 0x8000, 0);
    bus.advance(70 * LINE).unwrap();
    assert_eq!(bus.read_half(DEST + 160 * 2), Ok(0));
}

#[test]
fn sound_dma_refills_the_selected_fifo_and_repeats_from_the_next_word() {
    const FIFO_B: u32 = 0x0400_00A4;
    const DMA1: u32 = 0x0400_00BC;
    const SOURCE: u32 = 0x0300_0000;
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0084, 0x80).unwrap();
    bus.write_half(0x0400_0082, 0x7008).unwrap(); // A 不输出，B 用定时器 1。
    for _ in 0..8 { bus.write_word(FIFO_B, 0x0707_0707).unwrap(); }
    for i in 0..32u32 { bus.write_byte(SOURCE + i, (40 + i) as u8).unwrap(); }
    bus.write_word(DMA1, SOURCE).unwrap();
    bus.write_word(DMA1 + 4, FIFO_B).unwrap();
    bus.write_word(DMA1 + 8, 0xF200_0001).unwrap(); // FIFO 模式强制四个字，忽略计数与宽度。
    bus.write_word(0x0400_0100, 0x0080_FFC0).unwrap();
    bus.advance(500).unwrap(); // 定时器 0 不能触发 B。
    assert_eq!(bus.read_half(0x0400_0202).unwrap() & 0x200, 0);
    assert_eq!(bus.sound_level(), [0, 0]);
    bus.write_word(0x0400_0104, 0x0080_FFC0).unwrap();
    bus.advance(1 + 15 * 64).unwrap();
    assert_eq!(bus.sound_level(), [7, 7]);
    assert_eq!(bus.read_half(0x0400_0202).unwrap() & 0x200, 0);
    bus.advance(64 + 12).unwrap();
    assert_ne!(bus.read_half(0x0400_0202).unwrap() & 0x200, 0);
    assert_ne!(bus.read_half(DMA1 + 10).unwrap() & 0x8000, 0);
    bus.write_half(0x0400_0202, 0x200).unwrap();
    bus.advance(16 * 64).unwrap();
    assert_eq!(bus.sound_level(), [7, 7]);
    assert_ne!(bus.read_half(0x0400_0202).unwrap() & 0x200, 0);
    for byte in 40..=71 {
        bus.advance(64).unwrap();
        assert_eq!(bus.sound_level(), [byte, byte]);
    }
}

#[test]
fn timer_consumes_signed_sound_fifo_samples_in_byte_order() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0084, 0x0080).unwrap();
    bus.write_half(0x0400_0082, 0x0304).unwrap(); // A 左右声道，音量 100%
    bus.write_word(0x0400_00A0, 0x007F_8010).unwrap();
    bus.write_word(0x0400_0100, 0x0080_FC00).unwrap();
    bus.advance(1).unwrap();
    for value in [16, -128, 127, 0] {
        bus.advance(1024).unwrap();
        assert_eq!(bus.sound_level(), [value, value]);
    }
    assert!(!bus.take_audio_samples().is_empty());
}

#[test]
fn hblank_dma_reloads_destination_and_keeps_source_progress() {
    let mut bus = Bus::new(&[]);
    bus.write_word(RAM_START, 0x5678_1234).unwrap();
    bus.write_word(0x0400_00D4, RAM_START).unwrap();
    bus.write_word(0x0400_00D8, 0x0600_0000).unwrap();
    bus.write_word(0x0400_00DC, 0xA260_0001).unwrap();
    bus.advance(1006).unwrap();
    assert_eq!(bus.read_half(0x0600_0000), Ok(0));
    bus.advance(6).unwrap();
    assert_eq!(bus.read_half(0x0600_0000), Ok(0x1234));
    bus.advance(1232).unwrap();
    assert_eq!(bus.read_half(0x0600_0000), Ok(0x5678));
    assert_eq!(bus.read_half(0x0600_0002), Ok(0));
}

#[test]
fn immediate_dma_delays_each_write_and_only_interrupts_at_the_end() {
    let mut bus = Bus::new(&[]);
    bus.write_word(RAM_START, 0xBEEF_5AA5).unwrap();
    bus.write_word(0x0400_00D4, RAM_START).unwrap();
    bus.write_word(0x0400_00D8, 0x0600_0000).unwrap();
    bus.write_word(0x0400_00DC, 0xC000_0002).unwrap();
    bus.advance(5).unwrap();
    assert_eq!(bus.read_word(0x0600_0000), Ok(0));
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(0x0600_0000), Ok(0x5AA5));
    assert_eq!(bus.read_half(0x0600_0002), Ok(0));
    assert_eq!(bus.read_half(0x0400_0202), Ok(0));
    assert_ne!(bus.read_half(0x0400_00DE).unwrap() & 0x8000, 0);
    bus.advance(4).unwrap();
    assert_eq!(bus.read_half(0x0600_0002), Ok(0xBEEF));
    assert_eq!(bus.read_half(0x0400_0202), Ok(0));
    bus.advance(2).unwrap();
    assert_eq!(bus.read_half(0x0400_00DE).unwrap() & 0x8000, 0);
    assert_eq!(bus.read_half(0x0400_0202), Ok(1 << 11));
}

#[test]
fn cpu_waits_for_dma_bus_but_timer_keeps_counting() {
    const IWRAM: u32 = 0x0300_0000;
    let mut bus = Bus::new(&[]);
    let mut cpu = Cpu::new();
    cpu.regs[15] = IWRAM;
    for offset in [0, 4, 8] {
        bus.write_word(IWRAM + offset, 0xE280_0001).unwrap(); // ADD r0,r0,#1。
    }
    bus.write_half(IWRAM + 0x100, 0x12AB).unwrap();
    bus.write_half(0x0400_0102, 0x80).unwrap();
    bus.advance(1).unwrap();
    bus.write_word(0x0400_00D4, IWRAM + 0x100).unwrap();
    bus.write_word(0x0400_00D8, 0x0600_0000).unwrap();
    bus.write_word(0x0400_00DC, 0x8000_0001).unwrap();

    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 2);
    for _ in 0..4 {
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc(), IWRAM + 8);
    }
    assert_eq!(bus.read_half(0x0600_0000), Ok(0x12AB));
    assert_eq!(bus.read_half(0x0400_0100), Ok(6));
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], 3);
}

#[test]
fn hblank_dma_grants_channel_zero_the_bus_first() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0300_0100, 0xBEEF_1234).unwrap();
    bus.write_word(0x0400_00D4, 0x0300_0102).unwrap();
    bus.write_word(0x0400_00D8, 0x0600_0002).unwrap();
    bus.write_word(0x0400_00DC, 0xA000_0001).unwrap();
    bus.write_word(0x0400_00B0, 0x0300_0100).unwrap();
    bus.write_word(0x0400_00B4, 0x0600_0000).unwrap();
    bus.write_word(0x0400_00B8, 0xA000_0001).unwrap();

    bus.advance(1006 + 4).unwrap();
    assert_eq!(bus.read_word(0x0600_0000), Ok(0x0000_1234));
    bus.advance(4).unwrap();
    assert_eq!(bus.read_word(0x0600_0000), Ok(0xBEEF_1234));
}

#[test]
fn gamepak_dma_uses_waitcnt_for_first_and_sequential_reads() {
    let mut bus = Bus::new(&[0xBEEF_1234]);
    bus.write_half(0x0400_0204, 0x0010).unwrap(); // ROM 0 首次五拍，连续访问两拍。
    bus.write_word(0x0400_00D4, ROM_START).unwrap();
    bus.write_word(0x0400_00D8, 0x0300_0100).unwrap();
    bus.write_word(0x0400_00DC, 0x8000_0002).unwrap();

    bus.advance(7).unwrap();
    assert_eq!(bus.read_word(0x0300_0100), Ok(0));
    bus.advance(1).unwrap();
    assert_eq!(bus.read_word(0x0300_0100), Ok(0x0000_1234));
    bus.advance(2).unwrap();
    assert_eq!(bus.read_word(0x0300_0100), Ok(0x0000_1234));
    bus.advance(1).unwrap();
    assert_eq!(bus.read_word(0x0300_0100), Ok(0xBEEF_1234));
}

#[test]
fn higher_priority_dma_interrupts_a_transfer_between_words() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0300_0100, 0x2222_1111).unwrap();
    bus.write_word(0x0300_0104, 0x4444_3333).unwrap();
    bus.write_half(0x0300_0108, 0xABCD).unwrap();
    bus.write_word(0x0400_00D4, 0x0300_0100).unwrap();
    bus.write_word(0x0400_00D8, 0x0600_0000).unwrap();
    bus.write_word(0x0400_00DC, 0xC000_0004).unwrap();
    bus.advance(3).unwrap();
    bus.write_word(0x0400_00B0, 0x0300_0108).unwrap();
    bus.write_word(0x0400_00B4, 0x0600_0008).unwrap();
    bus.write_word(0x0400_00B8, 0xC000_0001).unwrap();

    bus.advance(5).unwrap();
    assert_eq!(bus.read_word(0x0600_0000), Ok(0x2222_1111));
    assert_eq!(bus.read_word(0x0600_0004), Ok(0));
    assert_eq!(bus.read_half(0x0600_0008), Ok(0xABCD));
    bus.advance(2).unwrap();
    assert_eq!(bus.read_half(0x0400_0202), Ok(1 << 8));
    bus.advance(6).unwrap();
    assert_eq!(bus.read_word(0x0600_0004), Ok(0x4444_3333));
    assert_eq!(bus.read_half(0x0400_0202), Ok((1 << 8) | (1 << 11)));
}
