use gba_cpu::Bus;

const SOUND: u32 = 0x0400_0000;

#[test]
fn pulse_is_routed_to_the_left_and_stops_when_its_length_expires() {
    let mut bus = Bus::new(&[]);
    bus.write_half(SOUND + 0x84, 0x80).unwrap();
    bus.write_half(SOUND + 0x80, 0x1070).unwrap();
    bus.write_half(SOUND + 0x82, 2).unwrap();
    bus.write_half(SOUND + 0x62, 0xF0BF).unwrap();
    bus.write_half(SOUND + 0x64, 0xC700).unwrap();
    assert_eq!(bus.read_half(SOUND + 0x84), Ok(0x81));
    bus.advance(32_767).unwrap();
    let samples = bus.take_audio_samples();
    assert!(samples.iter().all(|sample| sample[1] == 0));
    assert!(samples.iter().any(|sample| sample[0] < 0));
    assert!(samples.iter().any(|sample| sample[0] > 0));
    bus.advance(1).unwrap();
    assert_eq!(bus.read_half(SOUND + 0x84), Ok(0x80));
    assert_eq!(bus.sound_level(), [0, 0]);
}

#[test]
fn second_pulse_envelope_increases_on_the_right_channel() {
    let mut bus = Bus::new(&[]);
    bus.write_byte(SOUND + 0x84, 0x80).unwrap();
    bus.write_half(SOUND + 0x80, 0x0207).unwrap();
    bus.write_half(SOUND + 0x82, 2).unwrap();
    bus.write_byte(SOUND + 0x68, 0x80).unwrap();
    bus.write_byte(SOUND + 0x69, 0x19).unwrap();
    bus.write_half(SOUND + 0x6C, 0x8700).unwrap();
    assert_eq!(bus.read_half(SOUND + 0x84), Ok(0x82));
    bus.advance(262_143).unwrap();
    let before = bus.take_audio_samples();
    assert_eq!(before.iter().map(|sample| sample[1].abs()).max(), Some(128));
    bus.advance(32_769).unwrap();
    let after = bus.take_audio_samples();
    assert!(after.iter().all(|sample| sample[0] == 0 && sample[1].abs() == 256));
}

#[test]
fn wave_ram_plays_the_selected_bank_and_master_disable_silences_noise() {
    let mut bus = Bus::new(&[]);
    bus.write_half(SOUND + 0x84, 0x80).unwrap();
    for offset in (0x90..0xA0).step_by(2) {
        bus.write_half(SOUND + offset, 0xFFFF).unwrap();
    }
    bus.write_half(SOUND + 0x70, 0xC0).unwrap();
    assert_eq!(bus.read_half(SOUND + 0x90), Ok(0));
    bus.write_half(SOUND + 0x72, 0x2000).unwrap();
    bus.write_half(SOUND + 0x74, 0x8000).unwrap();
    bus.write_half(SOUND + 0x80, 0x8477).unwrap();
    bus.write_half(SOUND + 0x82, 2).unwrap();
    bus.write_half(SOUND + 0x78, 0xF000).unwrap();
    bus.write_half(SOUND + 0x7C, 0x8033).unwrap();
    assert_eq!(bus.read_half(SOUND + 0x84), Ok(0x8C));
    bus.advance(32_768).unwrap();
    let samples = bus.take_audio_samples();
    assert!(samples.iter().all(|sample| sample[1] > 0));
    assert!(samples.iter().any(|sample| sample[0] < 0));
    assert!(samples.iter().any(|sample| sample[0] > 0));
    bus.write_half(SOUND + 0x84, 0).unwrap();
    assert_eq!(bus.read_half(SOUND + 0x84), Ok(0));
    assert_eq!(bus.sound_level(), [0, 0]);
}
