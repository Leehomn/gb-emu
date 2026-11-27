use crate::Fault;

const FLASH_BANK_SIZE: usize = 64 * 1024;
const SECTOR_SIZE: usize = 4096;
const EEPROM_WRITE_CYCLES: u64 = 108_368;

pub(crate) enum CartridgeSave {
    Sram(Box<[u8]>),
    Flash(Flash),
    Eeprom(Eeprom),
}

fn has_version_tag(bytes: &[u8], prefix: &[u8]) -> bool {
    bytes.strip_prefix(prefix)
        .and_then(|version| version.get(..3))
        .is_some_and(|version| version.iter().all(u8::is_ascii_digit))
}

impl CartridgeSave {
    pub(crate) fn from_rom(rom: &[u8]) -> Self {
        for (index, &byte) in rom.iter().enumerate() {
            let suffix = &rom[index..];
            match byte {
                b'F' if has_version_tag(suffix, b"FLASH1M_V") => return Self::Flash(Flash::new(2)),
                b'F' if has_version_tag(suffix, b"FLASH512_V") || has_version_tag(suffix, b"FLASH_V") => {
                    return Self::Flash(Flash::new(1));
                }
                b'E' if has_version_tag(suffix, b"EEPROM_V") => return Self::Eeprom(Eeprom::default()),
                _ => {}
            }
        }
        Self::Sram(vec![0xFF; 32 * 1024].into_boxed_slice())
    }

    pub(crate) fn data(&self) -> &[u8] {
        match self {
            Self::Sram(data) => data,
            Self::Flash(flash) => &flash.data,
            Self::Eeprom(eeprom) => &eeprom.data,
        }
    }

    pub(crate) fn restore(&mut self, data: &[u8]) -> Result<(), Fault> {
        let storage: &mut [u8] = match self {
            Self::Sram(storage) if storage.len() == data.len() => storage,
            Self::Flash(flash) if flash.data.len() == data.len() => {
                flash.reset();
                &mut flash.data
            }
            Self::Eeprom(eeprom) if matches!(data.len(), 512 | 8192) => {
                *eeprom = Eeprom::default();
                eeprom.data.resize(data.len(), 0xFF);
                &mut eeprom.data
            }
            _ => return Err(Fault::InvalidSaveSize(data.len())),
        };
        storage.copy_from_slice(data);
        Ok(())
    }

    pub(crate) fn read_byte(&self, addr: u32) -> u8 {
        match self {
            Self::Sram(data) => data[(addr & 0x7FFF) as usize],
            Self::Flash(flash) => flash.read(addr as u16),
            Self::Eeprom(_) => 0xFF,
        }
    }

    pub(crate) fn write_byte(&mut self, addr: u32, value: u8) {
        match self {
            Self::Sram(data) => data[(addr & 0x7FFF) as usize] = value,
            Self::Flash(flash) => flash.write(addr as u16, value),
            Self::Eeprom(_) => {}
        }
    }
}

pub(crate) struct Flash {
    data: Box<[u8]>,
    bank: usize,
    id_mode: bool,
    command: FlashCommand,
}

#[derive(Default)]
enum FlashCommand {
    #[default]
    Idle,
    UnlockSecond { erase: bool },
    Unlocked { erase: bool },
    EraseUnlock,
    Program,
    Bank,
}

impl Flash {
    fn new(banks: usize) -> Self {
        Self {
            data: vec![0xFF; banks * FLASH_BANK_SIZE].into_boxed_slice(),
            bank: 0,
            id_mode: false,
            command: FlashCommand::Idle,
        }
    }

    fn reset(&mut self) {
        self.bank = 0;
        self.id_mode = false;
        self.command = FlashCommand::Idle;
    }

    fn read(&self, addr: u16) -> u8 {
        match (self.id_mode, addr) {
            (true, 0) => 0xC2,
            (true, 1) if self.data.len() == FLASH_BANK_SIZE => 0x1C,
            (true, 1) => 0x09,
            _ => self.data[self.bank * FLASH_BANK_SIZE + usize::from(addr)],
        }
    }

    fn write(&mut self, addr: u16, value: u8) {
        use FlashCommand::*;

        let command = std::mem::take(&mut self.command);
        let index = self.bank * FLASH_BANK_SIZE + usize::from(addr);
        self.command = match (command, addr, value) {
            (Program, _, _) => {
                self.data[index] &= value;
                Idle
            }
            (Bank, 0, 0..=1) if self.data.len() == 2 * FLASH_BANK_SIZE => {
                self.bank = usize::from(value);
                Idle
            }
            (Idle, 0x5555, 0xAA) => UnlockSecond { erase: false },
            (EraseUnlock, 0x5555, 0xAA) => UnlockSecond { erase: true },
            (UnlockSecond { erase }, 0x2AAA, 0x55) => Unlocked { erase },
            (Unlocked { erase: true }, 0x5555, 0x10) => {
                self.data.fill(0xFF);
                Idle
            }
            (Unlocked { erase: true }, _, 0x30) if index % SECTOR_SIZE == 0 => {
                self.data[index..index + SECTOR_SIZE].fill(0xFF);
                Idle
            }
            (Unlocked { erase: false }, 0x5555, 0x90) => {
                self.id_mode = true;
                Idle
            }
            (_, 0x5555, 0xF0) => {
                self.id_mode = false;
                Idle
            }
            (Unlocked { erase: false }, 0x5555, 0x80) => EraseUnlock,
            (Unlocked { erase: false }, 0x5555, 0xA0) => Program,
            (Unlocked { erase: false }, 0x5555, 0xB0) => Bank,
            _ => Idle,
        };
    }
}

pub(crate) struct Eeprom {
    data: Vec<u8>,
    command: Vec<u8>,
    output: u64,
    output_bits: u8,
    busy_until: Option<u64>,
}

impl Default for Eeprom {
    fn default() -> Self {
        Self {
            data: vec![0xFF; 512],
            command: Vec::with_capacity(81),
            output: 0,
            output_bits: 0,
            busy_until: None,
        }
    }
}

impl Eeprom {
    pub(crate) fn addressed(addr: u32, rom_len: usize) -> bool {
        addr >> 24 == 0x0D && (rom_len <= 16 * 1024 * 1024 || addr >= 0x0DFF_FF00)
    }

    pub(crate) fn ready(&self, clock: u64) -> u16 {
        u16::from(self.busy_until.is_none_or(|until| clock.wrapping_sub(until) as i64 >= 0))
    }

    pub(crate) fn write_bit(&mut self, value: u16, remaining: u32, first: bool, clock: u64) {
        if first {
            self.command.clear();
            self.output_bits = 0;
        }
        if self.command.len() < 82 {
            self.command.push((value & 1) as u8);
        }
        if remaining != 1 || self.ready(clock) == 0 { return; }

        let (address_bits, writing) = match (self.command.as_slice(), self.command.len()) {
            ([1, 1, .., 0], 9) => (6, false),
            ([1, 1, .., 0], 17) => (14, false),
            ([1, 0, .., 0], 73) => (6, true),
            ([1, 0, .., 0], 81) => (14, true),
            _ => return,
        };
        if address_bits == 14 {
            self.data.resize(8192, 0xFF);
        }
        let data_start = 2 + address_bits;
        let block = usize::from(bits_to_u64(&self.command[2..data_start]) as u16 & 0x03FF);
        let page = &mut self.data[block * 8..block * 8 + 8];
        if writing {
            page.copy_from_slice(&bits_to_u64(&self.command[data_start..data_start + 64]).to_be_bytes());
            self.busy_until = Some(clock.wrapping_add(EEPROM_WRITE_CYCLES));
        } else {
            self.output = u64::from_be_bytes(page.try_into().unwrap());
            self.output_bits = 68;
        }
    }

    pub(crate) fn read_bit(&mut self, clock: u64) -> u16 {
        if self.output_bits == 0 { return self.ready(clock); }
        self.output_bits -= 1;
        if self.output_bits >= 64 { 0 } else { ((self.output >> self.output_bits) & 1) as u16 }
    }
}

fn bits_to_u64(bits: &[u8]) -> u64 {
    bits.iter().fold(0, |value, &bit| value << 1 | u64::from(bit))
}
