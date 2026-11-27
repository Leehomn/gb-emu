use gba_cpu::{Bus, Cpu, ROM_START};

#[test]
fn biosless_vblank_wait_advances_to_the_next_frame_boundary() {
    let mut bus = Bus::new(&[0xEF05_0000, 0xEF05_0000]);
    let mut cpu = Cpu::new();
    bus.advance(200).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.pc(), ROM_START + 4);
    assert_eq!(bus.read_half(0x0400_0006), Ok(160));
    bus.write_word(0x0400_0100, 0x0080_0000).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!(bus.read_half(0x0400_0006), Ok(160));
    assert_ne!(bus.read_half(0x0400_0100), Ok(0));
}
