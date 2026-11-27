use gba_cpu::{Bus, Cpu, Fault};

const BACKUP: u32 = 0x0E00_0000;
const EEPROM: u32 = 0x0DFF_FF00;
const DMA3: u32 = 0x0400_00D4;
const IWRAM: u32 = 0x0300_0000;

fn flash_command(bus: &mut Bus, command: u8) {
    bus.write_byte(BACKUP + 0x5555, 0xAA).unwrap();
    bus.write_byte(BACKUP + 0x2AAA, 0x55).unwrap();
    bus.write_byte(BACKUP + 0x5555, command).unwrap();
}

#[test]
fn flash_commands_select_identification_and_program_bytes() {
    let rom = b"FLASH512_V130".to_vec();
    let mut bus = Bus::from_rom(rom.clone()).unwrap();
    flash_command(&mut bus, 0x90);
    assert_eq!(bus.read_byte(BACKUP).unwrap(), 0xC2);
    assert_eq!(bus.read_byte(BACKUP + 1).unwrap(), 0x1C);
    flash_command(&mut bus, 0xF0);
    assert_eq!(bus.read_byte(BACKUP).unwrap(), 0xFF);
    bus.write_byte(BACKUP, 0).unwrap();
    assert_eq!(bus.read_byte(BACKUP).unwrap(), 0xFF);

    flash_command(&mut bus, 0xA0);
    bus.write_byte(BACKUP + 0x789, 0x45).unwrap();
    flash_command(&mut bus, 0xA0);
    bus.write_byte(BACKUP + 0x789, 0xC3).unwrap();
    assert_eq!(bus.read_byte(BACKUP + 0x789).unwrap(), 0x41);
    let mut restarted = Bus::from_rom(rom).unwrap();
    assert_eq!(restarted.restore_save(&[0; 10]), Err(Fault::InvalidSaveSize(10)));
    restarted.restore_save(bus.save_data()).unwrap();
    assert_eq!(restarted.read_byte(BACKUP + 0x789).unwrap(), 0x41);
}

#[test]
fn flash128k_erases_selected_sector_or_the_entire_chip() {
    let mut bus = Bus::from_rom(b"FLASH1M_V103".to_vec()).unwrap();
    for bank in 0..=1 {
        flash_command(&mut bus, 0xB0);
        bus.write_byte(BACKUP, bank).unwrap();
        for offset in [0x1200, 0x2200] {
            flash_command(&mut bus, 0xA0);
            bus.write_byte(BACKUP + offset, 0x10 + bank).unwrap();
        }
    }
    assert_eq!(bus.save_data().len(), 128 * 1024);
    flash_command(&mut bus, 0x80);
    bus.write_byte(BACKUP + 0x5555, 0xAA).unwrap();
    bus.write_byte(BACKUP + 0x2AAA, 0x55).unwrap();
    bus.write_byte(BACKUP + 0x1000, 0x30).unwrap();
    assert_eq!(bus.read_byte(BACKUP + 0x1200).unwrap(), 0xFF);
    assert_eq!(bus.read_byte(BACKUP + 0x2200).unwrap(), 0x11);
    flash_command(&mut bus, 0xB0);
    bus.write_byte(BACKUP, 0).unwrap();
    assert_eq!(bus.read_byte(BACKUP + 0x1200).unwrap(), 0x10);
    flash_command(&mut bus, 0x80);
    flash_command(&mut bus, 0x10);
    assert!(bus.save_data().iter().all(|&byte| byte == 0xFF));
}

fn dma_halfwords(bus: &mut Bus, source: u32, dest: u32, count: u16) {
    bus.write_word(DMA3, source).unwrap();
    bus.write_word(DMA3 + 4, dest).unwrap();
    bus.write_word(DMA3 + 8, 0x8000_0000 | u32::from(count)).unwrap();
    bus.advance(24 * u32::from(count) + 8).unwrap();
    assert_eq!(bus.read_half(DMA3 + 10).unwrap() & 0x8000, 0);
}

fn bits(value: u64, width: u32) -> impl Iterator<Item = u16> {
    (0..width).rev().map(move |shift| ((value >> shift) & 1) as u16)
}

fn eeprom_command(bus: &mut Bus, addr: u64, width: u32, data: Option<u64>) {
    let mut command = vec![1, u16::from(data.is_none())];
    command.extend(bits(addr, width));
    if let Some(value) = data { command.extend(bits(value, 64)); }
    command.push(0);
    for (index, &bit) in command.iter().enumerate() {
        bus.write_half(IWRAM + index as u32 * 2, bit).unwrap();
    }
    dma_halfwords(bus, IWRAM, EEPROM, command.len() as u16);
}

#[test]
fn dma3_round_trips_both_eeprom_sizes_across_restart() {
    for (width, addr, len) in [(6, 0x3F, 512), (14, 0x3FF, 8192)] {
        let rom = b"EEPROM_V126".to_vec();
        let mut bus = Bus::from_rom(rom.clone()).unwrap();
        let data = 0x0145_89AB_CDEF_2376;
        eeprom_command(&mut bus, addr, width, Some(data));
        assert_eq!(bus.save_data().len(), len);
        assert_eq!(bus.read_half(EEPROM), Ok(0));
        bus.advance(108_368).unwrap();
        assert_eq!(bus.read_half(EEPROM), Ok(1));

        let mut restarted = Bus::from_rom(rom).unwrap();
        restarted.restore_save(bus.save_data()).unwrap();
        eeprom_command(&mut restarted, addr, width, None);
        dma_halfwords(&mut restarted, EEPROM, IWRAM, 68);
        let result = (4..68).fold(0, |value, index| {
            value << 1 | u64::from(restarted.read_half(IWRAM + index * 2).unwrap() & 1)
        });
        assert_eq!(result, data);
        restarted.write_word(IWRAM, 0xE1D1_00B0).unwrap(); // LDRH r0,[r1]。
        let mut cpu = Cpu::new();
        cpu.regs[15] = IWRAM;
        cpu.regs[1] = EEPROM;
        cpu.step(&mut restarted).unwrap();
        assert_eq!(cpu.regs[0] & 1, 1);
    }
}
