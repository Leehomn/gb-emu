use crate::{Bus, Fault};

#[derive(Clone, Copy)]
pub(crate) struct Pipeline {
    pub pc: u32,
    pub width: u32,
    pub words: [Option<u32>; 2],
}

#[derive(Default)]
pub(crate) struct GamePakPrefetch {
    next: u32,
    ready: u32,
    partial: u32,
}

pub(crate) fn gamepak_rom(addr: u32) -> bool {
    matches!(addr >> 24, 0x08..=0x0D)
}

fn save_bus(addr: u32) -> bool {
    matches!(addr >> 24, 0x0E | 0x0F)
}

pub(crate) fn vacant_gamepak_halfword(addr: u32) -> u16 {
    (addr >> 1) as u16
}

impl Bus {
    pub(crate) fn access_cycles(&self, addr: u32, width: u32, sequential: bool) -> u32 {
        let wait = u32::from(self.hw.waitcnt);
        match addr >> 24 {
            2 => if width == 4 { 6 } else { 3 },
            5 => if width == 4 { 2 } else { 1 },
            6 => {
                let halfwords = if width == 4 { 2 } else { 1 };
                let video = self.display_control;
                let offset = addr & 0x1_FFFF;
                if video & 0x0080 != 0 {
                    return u32::from(halfwords);
                }
                let fetch_slots = match video & 7 {
                    0 | 1 if offset < 0x1_0000 => {
                        let layers = video & if video & 7 == 0 { 0x0F00 } else { 0x0700 };
                        if layers == 0 { return u32::from(halfwords); }
                        return self.tile_background_vram_cycles(halfwords, layers, video & 7 == 1);
                    }
                    2 if offset < 0x1_0000 => {
                        // BG3 取前两拍，BG2 取后两拍。
                        let bg2 = if video & 0x0400 != 0 { 0b1100 } else { 0 };
                        let bg3 = if video & 0x0800 != 0 { 0b0011 } else { 0 };
                        bg2 | bg3
                    }
                    3..=5 if offset < 0x1_4000 && video & 0x0400 != 0 => 0b1000,
                    _ => 0,
                };
                if fetch_slots == 0 {
                    u32::from(halfwords)
                } else {
                    self.hw.background_vram_cycles(halfwords, fetch_slots)
                }
            }
            0x08..=0x0D => {
                let region = (addr >> 25) - 4;
                let first = [5, 4, 3, 9][((wait >> (2 + region * 3)) & 3) as usize];
                let second = if wait & (1 << (4 + region * 3)) != 0 { 2 }
                    else { [3, 5, 9][region as usize] };
                let cost = if sequential && addr & 0x1_FFFF != 0 { second } else { first };
                cost + if width == 4 { second } else { 0 }
            }
            0x0E | 0x0F => [5, 4, 3, 9][(wait & 3) as usize],
            _ => 1,
        }
    }

    pub(crate) fn fill_gamepak(&mut self, mut cycles: u32) {
        if self.hw.waitcnt & 0x4000 == 0 || !gamepak_rom(self.gamepak_prefetch.next) {
            return;
        }
        while cycles != 0 && self.gamepak_prefetch.ready < 8 {
            let queue = &self.gamepak_prefetch;
            let addr = queue.next + queue.ready * 2;
            if !gamepak_rom(addr) { break; }
            let remaining = self.access_cycles(addr, 2, true) - queue.partial;
            let spent = remaining.min(cycles);
            cycles -= spent;
            self.gamepak_prefetch.partial += spent;
            if spent == remaining {
                self.gamepak_prefetch.partial = 0;
                self.gamepak_prefetch.ready += 1;
            }
        }
    }

    pub(crate) fn clock_fetch(&mut self, addr: u32, width: u32, sequential: bool) -> Result<Option<u32>, Fault> {
        self.wait_for_dma()?;
        let after_dma = self.hw.take_dma_bus_access();
        let cost = if gamepak_rom(addr) && self.hw.waitcnt & 0x4000 != 0 {
            let buffered = sequential && addr == self.gamepak_prefetch.next;
            if !buffered {
                self.gamepak_prefetch = GamePakPrefetch { next: addr, ..Default::default() };
            }
            // DMA 打断总线的顺序访问，已有的卡带预取仍可继续供给指令。
            let sequential = sequential && (!after_dma || buffered);
            let mut cost = 0;
            for offset in (0..width).step_by(2) {
                if self.gamepak_prefetch.ready == 0 {
                    cost += self.access_cycles(addr + offset, 2, sequential || offset != 0)
                        - self.gamepak_prefetch.partial;
                    self.gamepak_prefetch.partial = 0;
                } else {
                    self.gamepak_prefetch.ready -= 1;
                }
                self.gamepak_prefetch.next = self.gamepak_prefetch.next.wrapping_add(2);
            }
            // CPU 从缓冲取指的这一拍，卡带总线仍可继续预读下一个半字。
            if cost == 0 { self.fill_gamepak(1); }
            cost.max(1)
        } else {
            self.gamepak_prefetch = GamePakPrefetch::default();
            self.access_cycles(addr, width, sequential && !after_dma)
        };
        self.advance_cpu_access(cost);
        Ok(self.sample_opcode(addr, width))
    }

    fn sample_opcode(&mut self, addr: u32, width: u32) -> Option<u32> {
        let word = if width == 2 { self.fetch_thumb(addr).ok().map(u32::from) }
            else { self.read_word(addr).ok() };
        if addr < 0x4000 && word.is_some() {
            self.bios_latch = self.read_word(addr & !3).unwrap_or(self.bios_latch);
        }
        word
    }

    // 直启卡带和宿主设置 PC 时，从已预取两条指令的状态开始。
    pub(crate) fn seed_pipeline(&mut self, pc: u32, width: u32) -> Pipeline {
        self.gamepak_prefetch = GamePakPrefetch::default();
        let first = self.sample_opcode(pc, width);
        let second = self.sample_opcode(pc.wrapping_add(width), width);
        Pipeline { pc, width, words: [first, second] }
    }

    pub(crate) fn refill_pipeline(&mut self, pc: u32, width: u32) -> Result<Pipeline, Fault> {
        let first = self.clock_fetch(pc, width, false)?;
        let second = self.clock_fetch(pc.wrapping_add(width), width, true)?;
        Ok(Pipeline { pc, width, words: [first, second] })
    }
}

// 指令解释器的访存入口。宿主的 Bus 读写没有 CPU 等待周期和开放总线副作用。
pub(crate) struct CpuBus<'a> {
    bus: &'a mut Bus,
    pc: u32,
    width: u32,
    decoded: Option<u32>,
    fetched: Option<u32>,
    fetch_pending: bool,
    previous_data: Option<(u32, bool)>,
    read_data: bool,
    jumped: bool,
}

impl<'a> CpuBus<'a> {
    pub fn new(bus: &'a mut Bus, pc: u32, width: u32, decoded: Option<u32>) -> Self {
        Self { bus, pc, width, decoded, fetched: None, fetch_pending: true,
            previous_data: None, read_data: false, jumped: false }
    }

    pub fn has_bios(&self) -> bool { self.bus.bios.is_some() }

    pub fn jump(&mut self, target: u32) -> u32 {
        self.jumped = true;
        target
    }

    fn fetch(&mut self, sequential: bool) -> Result<(), Fault> {
        if self.fetch_pending {
            self.fetched = self.bus.clock_fetch(self.pc.wrapping_add(2 * self.width), self.width, sequential)?;
            self.fetch_pending = false;
        }
        Ok(())
    }

    pub fn idle(&mut self, cycles: u32) -> Result<(), Fault> {
        self.fetch(self.internal_fetch_is_sequential())?;
        self.bus.wait_for_dma()?;
        self.bus.fill_gamepak(cycles);
        self.bus.advance_cpu_access(cycles);
        Ok(())
    }

    fn internal_fetch_is_sequential(&self) -> bool {
        !gamepak_rom(self.pc) || self.jumped || self.bus.hw.waitcnt & 0x4000 != 0
    }

    pub fn biosless_swi(&mut self, service: u8) -> Result<bool, Fault> {
        match service {
            2 | 3 => {
                self.fetch(true)?;
                self.bus.hw.set_power(if service == 2 { 0 } else { 0x80 });
            }
            5 => self.bus.wait_for_vblank()?,
            _ => return Ok(false),
        }
        Ok(true)
    }

    pub fn finish(mut self, next: u32, width: u32) -> Result<Pipeline, Fault> {
        self.fetch(true)?;
        if self.read_data { self.idle(1)?; }
        if self.jumped || width != self.width || next != self.pc.wrapping_add(width) {
            self.bus.refill_pipeline(next, width)
        } else {
            Ok(Pipeline { pc: next, width, words: [self.decoded, self.fetched] })
        }
    }

    fn data_cycle(&mut self, addr: u32, width: u32, write: bool) -> Result<(), Fault> {
        let external = matches!(addr >> 24, 8..=15);
        let prefetch = gamepak_rom(self.pc) && self.bus.hw.waitcnt & 0x4000 != 0;
        let sequential_code = if prefetch { !external } else { !write && self.internal_fetch_is_sequential() };
        self.fetch(sequential_code)?;
        self.bus.wait_for_dma()?;
        let after_dma = self.bus.hw.take_dma_bus_access();
        let sequential = !after_dma && self.previous_data == Some((addr, write)) && addr & 0x00FF_FFFF != 0;
        self.previous_data = Some((addr.wrapping_add(width), write));
        self.read_data |= !write;
        let cycles = self.bus.access_cycles(addr, width, sequential);
        if external {
            self.bus.gamepak_prefetch = GamePakPrefetch::default();
        } else {
            self.bus.fill_gamepak(cycles);
        }
        // 读值在最后一拍外设更新之前锁存。
        self.bus.advance_cpu_access(cycles - u32::from(!write));
        Ok(())
    }

    fn open_bus(&self, addr: u32) -> u32 {
        if addr < 0x4000 && self.pc >= 0x4000 {
            return self.bus.bios_latch;
        }
        let fetched = self.fetched.unwrap_or(0);
        if self.width == 4 { return fetched; }
        let decoded = self.decoded.unwrap_or(fetched);
        match self.pc >> 24 {
            0 | 7 if self.pc & 2 == 0 => {
                let high = u32::from(self.bus.fetch_thumb(self.pc.wrapping_add(6)).unwrap_or(0));
                (high << 16) | fetched
            }
            0 | 7 => (fetched << 16) | decoded,
            3 if self.pc & 2 == 0 => (decoded << 16) | fetched,
            3 => (fetched << 16) | decoded,
            _ => fetched | (fetched << 16),
        }
    }

    fn protected_bios(&self, addr: u32) -> bool {
        addr < 0x4000 && self.pc >= 0x4000
    }

    pub fn read_byte(&mut self, addr: u32) -> Result<u8, Fault> {
        self.data_cycle(addr, 1, false)?;
        let value = self.sample_byte(addr);
        self.bus.advance_cpu_access(1);
        Ok(value)
    }

    fn sample_byte(&self, addr: u32) -> u8 {
        if !self.protected_bios(addr) {
            if let Ok(value) = self.bus.read_byte(addr) { return value; }
        }
        if gamepak_rom(addr) { return (vacant_gamepak_halfword(addr) >> (8 * (addr & 1))) as u8; }
        (self.open_bus(addr) >> (8 * (addr & 3))) as u8
    }

    pub fn read_half(&mut self, addr: u32) -> Result<u16, Fault> {
        self.data_cycle(addr, 2, false)?;
        let value = self.sample_half(addr);
        self.bus.advance_cpu_access(1);
        Ok(value)
    }

    fn sample_half(&self, addr: u32) -> u16 {
        if save_bus(addr) { return u16::from(self.sample_byte(addr)) * 0x0101; }
        if !self.protected_bios(addr) {
            if let Ok(value) = self.bus.read_half(addr) { return value; }
        }
        if gamepak_rom(addr) { return vacant_gamepak_halfword(addr); }
        (self.open_bus(addr) >> (8 * (addr & 2))) as u16
    }

    pub fn read_word(&mut self, addr: u32) -> Result<u32, Fault> {
        self.data_cycle(addr, 4, false)?;
        let value = self.sample_word(addr);
        self.bus.advance_cpu_access(1);
        Ok(value)
    }

    fn sample_word(&self, addr: u32) -> u32 {
        if save_bus(addr) { return u32::from(self.sample_byte(addr)) * 0x0101_0101; }
        if !self.protected_bios(addr) {
            if let Ok(value) = self.bus.read_word(addr) { return value; }
        }
        if gamepak_rom(addr) {
            return u32::from(vacant_gamepak_halfword(addr))
                | (u32::from(vacant_gamepak_halfword(addr + 2)) << 16);
        }
        self.open_bus(addr)
    }

    pub fn load_half(&mut self, addr: u32) -> Result<u32, Fault> {
        let aligned = if save_bus(addr) { addr } else { addr & !1 };
        Ok(u32::from(self.read_half(aligned)?).rotate_right((addr & 1) * 8))
    }

    pub fn load_word(&mut self, addr: u32) -> Result<u32, Fault> {
        let aligned = if save_bus(addr) { addr } else { addr & !3 };
        Ok(self.read_word(aligned)?.rotate_right((addr & 3) * 8))
    }

    pub fn write_byte(&mut self, addr: u32, value: u8) -> Result<(), Fault> {
        self.data_cycle(addr, 1, true)?;
        guest_write(self.bus.write_byte(addr, value))
    }

    pub fn write_half(&mut self, addr: u32, value: u16) -> Result<(), Fault> {
        if save_bus(addr) { return self.write_byte(addr, (value >> (8 * (addr & 1))) as u8); }
        let addr = addr & !1;
        self.data_cycle(addr, 2, true)?;
        guest_write(self.bus.write_half(addr, value))
    }

    pub fn write_word(&mut self, addr: u32, value: u32) -> Result<(), Fault> {
        if save_bus(addr) { return self.write_byte(addr, (value >> (8 * (addr & 3))) as u8); }
        let addr = addr & !3;
        self.data_cycle(addr, 4, true)?;
        guest_write(self.bus.write_word(addr, value))
    }
}

fn guest_write(result: Result<(), Fault>) -> Result<(), Fault> {
    match result {
        Err(Fault::ReadOnly(_) | Fault::Unmapped(_)) => Ok(()),
        _ => result,
    }
}
