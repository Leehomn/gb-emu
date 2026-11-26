use gb_emu::{Bus, Cpu};

fn cgb_bus() -> Bus {
    let mut rom = vec![0; 32768];
    rom[0x143] = 0xC0;
    rom[0x100..0x102].copy_from_slice(&[0x10, 0]);
    let mut bus = Bus::new();
    bus.load_rom(&rom);
    bus.write(0xFF40, 0);
    bus
}

#[test]
fn cgb_rom_sees_separate_memory_banks_and_switches_speed() {
    let mut bus = cgb_bus();
    bus.write(0x8000, 0x12);
    bus.write(0xFF4F, 1);
    bus.write(0x8000, 0x34);
    bus.write(0xFF4F, 0);
    assert_eq!(bus.read(0x8000), 0x12);
    bus.write(0xFF4F, 1);
    assert_eq!(bus.read(0x8000), 0x34);
    bus.write(0xD000, 0x37);
    bus.write(0xFF70, 2);
    bus.write(0xF000, 0x49);
    assert_eq!(bus.read(0xD000), 0x49);
    bus.write(0xFF70, 0);
    assert_eq!(bus.read(0xD000), 0x37);

    bus.write(0xFF4D, 1);
    Cpu::new().step(&mut bus);
    let before = bus.video_dots;
    assert_eq!(bus.read(0xFF4D) & 0x81, 0x80);
    assert_eq!(bus.video_dots - before, 2);
}

fn color(bus: &mut Bus, obj: bool, palette: u8, index: u8, rgb: u16) {
    let register = if obj { 0xFF6A } else { 0xFF68 };
    bus.write(register, 0x80 | (palette * 8 + index * 2));
    for byte in rgb.to_le_bytes() { bus.write(register + 1, byte); }
}

#[test]
fn cgb_composites_background_window_and_sprite_colors() {
    let mut bus = cgb_bus();
    color(&mut bus, false, 0, 1, 0x001F);
    color(&mut bus, false, 1, 1, 0x7C00);
    color(&mut bus, false, 2, 1, 0x03E0);
    color(&mut bus, true, 3, 1, 0x03FF);
    for y in 0..8 { bus.write(0x8000 + 2 * y, 0xFF); }
    bus.write(0xFF4F, 1);
    for y in 0..8 { bus.write(0x8020 + 2 * y, 0xFF); }
    for row in 0..32 {
        bus.write(0x9801 + row * 32, 1);
        bus.write(0x9C00 + row * 32, 0x82);
    }
    bus.write(0xFF4F, 0);
    for (i, byte) in [24, 12, 2, 0x0B].into_iter().enumerate() {
        bus.write(0xFE00 + i as u16, byte);
    }
    bus.write(0xFF4A, 0);
    bus.write(0xFF4B, 31);
    bus.write(0xFF40, 0xF3);
    bus.tick(70224);
    let line = &bus.ppu.color_frame[8];
    assert_eq!((line[0], line[5], line[14], line[25]), (0x001F, 0x03FF, 0x7C00, 0x03E0));
}

#[test]
fn cgb_video_dma_can_copy_now_or_across_hblanks() {
    let mut bus = cgb_bus();
    for i in 0..48 { bus.write(0xC000 + i, i as u8 + 1); }
    bus.write(0xFF51, 0xC0);
    bus.write(0xFF52, 0);
    bus.write(0xFF53, 0);
    bus.write(0xFF54, 0);
    bus.write(0xFF4F, 1);
    bus.write(0xFF55, 0);
    assert_eq!(bus.read(0x800F), 16);
    assert_eq!(bus.read(0xFF55), 0xFF);
    bus.write(0xFF40, 0x91);
    bus.write(0xFF55, 0x81);
    assert_eq!(bus.read(0x8010), 0);
    bus.tick(0);
    bus.tick(300);
    assert_eq!(bus.read(0x801F), 32);
    assert_eq!(bus.read(0x8020), 0);
    bus.tick(0);
    bus.tick(456);
    assert_eq!(bus.read(0x802F), 48);
    assert_eq!(bus.read(0xFF55), 0xFF);
}
