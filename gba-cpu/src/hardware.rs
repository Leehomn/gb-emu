use crate::{Bus, Fault};
use crate::cpu_bus::{gamepak_rom, vacant_gamepak_halfword};
use std::collections::VecDeque;

const LINE_CYCLES: u16 = 1232;
const HBLANK_START: u16 = 1006;
const LAST_TEXT_TILE_START: u16 = HBLANK_START - 3;
const FIRST_BG_FETCH_GROUP: u16 = 32;
const VISIBLE_LINES: u16 = 160;
const FRAME_LINES: u16 = 228;
const DISPLAY_DMA_FIRST_LINE: u16 = 2;
const DISPLAY_DMA_END_LINE: u16 = VISIBLE_LINES + DISPLAY_DMA_FIRST_LINE;
const PRESCALERS: [u16; 4] = [1, 64, 256, 1024];
const IRQ_DELAY: u8 = 7;
const DMA_START_DELAY: u64 = 2;
const DMA_FINISH_DELAY: u32 = 2;

#[derive(Clone, Copy, Default)]
pub(crate) enum PowerMode {
    #[default]
    Running,
    Halt,
    Stop,
}

#[derive(Default)]
pub(crate) struct Timer {
    reload: u16,
    count: u16,
    control: u16,
    phase: u16,
    starting: bool,
}

#[derive(Clone, Copy, Default)]
struct Dma {
    source: u32,
    dest: u32,
    count: u16,
    control: u16,
    current_source: u32,
    current_dest: u32,
    remaining: u32,
    ready_at: u64,
    sequential: bool,
    latch: u32,
}

impl Dma {
    fn sound_destination(&self, channel: usize) -> Option<u32> {
        let dest = self.current_dest & !3;
        (matches!(channel, 1 | 2) && self.control & 0xB000 == 0xB000
            && matches!(dest, 0x0400_00A0 | 0x0400_00A4)).then_some(dest)
    }

    fn width(&self, channel: usize) -> u32 {
        if self.sound_destination(channel).is_some() || self.control & 0x0400 != 0 { 4 } else { 2 }
    }
}

enum DmaPhase {
    Read,
    Write(u32),
    Complete,
}

struct DmaUnit {
    channel: usize,
    cycles: u32,
    phase: DmaPhase,
    uses_gamepak: bool,
}

#[derive(Default)]
pub(crate) struct Hardware {
    line: u16,
    pub(crate) frame: u64,
    dot: u16,
    display_irq: u16,
    enable: u16,
    flags: u16,
    master: bool,
    irq_age: u8,
    pub(crate) waitcnt: u16,
    pub(crate) power: PowerMode,
    post_boot: u16,
    key_control: u16,
    keys_match: bool,
    timers: [Timer; 4],
    dma: [Dma; 4],
    dma_pending: u8,
    dma_unit: Option<DmaUnit>,
    dma_last_access: bool,
    clock: u64,
    sound_on: bool,
    sound_control: u16,
    psg: crate::audio::Psg,
    sound_bias: Option<u16>,
    fifo: [VecDeque<i8>; 2],
    pcm: [i8; 2],
    sample_phase: u16,
    samples: Vec<[i16; 2]>,
}

impl Hardware {
    pub(crate) fn background_vram_cycles(&self, halfwords: u16, fetch_slots: u8) -> u32 {
        if self.line >= VISIBLE_LINES { return u32::from(halfwords); }
        let mut dot = self.dot + 1;
        for _ in 0..halfwords {
            if (FIRST_BG_FETCH_GROUP..HBLANK_START).contains(&dot) {
                if fetch_slots == 0b1111 {
                    // 双仿射背景连续取数，直接跳到消隐的第一个空位。
                    dot = HBLANK_START;
                } else {
                    while dot < HBLANK_START && fetch_slots & (1 << (dot % 4)) != 0 {
                        dot += 1;
                    }
                }
            }
            dot += 1;
        }
        u32::from(dot - self.dot - 1)
    }

    pub(crate) fn visible_write_position(&self) -> Option<(usize, usize)> {
        if self.line >= VISIBLE_LINES || self.dot >= HBLANK_START {
            return None;
        }
        // 像素每四拍完成一个。可见区画完后，还要等到第 1006 拍才进入 HBlank。
        Some((usize::from(self.line), usize::from(self.dot / 4).min(crate::WIDTH)))
    }

    fn vblank(&self) -> bool {
        (VISIBLE_LINES..FRAME_LINES - 1).contains(&self.line)
    }

    fn matched(&self) -> bool {
        self.line == self.display_irq >> 8
    }

    pub(crate) fn irq_pending(&self) -> bool {
        self.master && self.enable & self.flags != 0 && self.irq_age >= IRQ_DELAY
    }

    pub(crate) fn set_power(&mut self, control: u8) {
        self.power = if control & 0x80 != 0 { PowerMode::Stop } else { PowerMode::Halt };
    }

    pub(crate) fn set_post_boot(&mut self, value: u8) {
        self.post_boot |= u16::from(value & 1);
    }

    pub(crate) fn update_keys(&mut self, input: u16) {
        let mask = self.key_control & 0x03FF;
        let pressed = !input & mask;
        let matched = if self.key_control & 0x8000 != 0 {
            mask != 0 && pressed == mask
        } else {
            pressed != 0
        };
        if !self.keys_match && matched && self.key_control & 0x4000 != 0 {
            self.flags |= 0x1000;
        }
        self.keys_match = matched && self.key_control & 0x4000 != 0;
    }

    pub(crate) fn wake_cpu(&mut self) {
        let enabled = self.enable & self.flags;
        let wake = match self.power {
            PowerMode::Running => false,
            PowerMode::Halt => enabled != 0,
            PowerMode::Stop => enabled & 0x3080 != 0,
        };
        if wake { self.power = PowerMode::Running; }
    }

    pub(crate) fn read(&self, addr: u32) -> Option<u16> {
        Some(match addr {
            0x0400_0004 => self.display_irq | u16::from(self.vblank())
                | (u16::from(self.dot >= HBLANK_START) << 1) | (u16::from(self.matched()) << 2),
            0x0400_0006 => self.line,
            0x0400_0100..=0x0400_010E => {
                let timer = &self.timers[((addr - 0x0400_0100) / 4) as usize];
                if addr & 2 == 0 { timer.count } else { timer.control }
            }
            0x0400_00B0..=0x0400_00DE => {
                let offset = addr - 0x0400_00B0;
                if offset % 12 == 10 { self.dma[(offset / 12) as usize].control } else { 0 }
            }
            0x0400_0060..=0x0400_0080 | 0x0400_0090..=0x0400_009E => self.psg.read(addr),
            0x0400_0082 => self.sound_control,
            0x0400_0084 => (u16::from(self.sound_on) << 7) | self.psg.status(),
            0x0400_0088 => self.sound_bias.unwrap_or(0x0200),
            0x0400_0132 => self.key_control,
            0x0400_0200 => self.enable,
            0x0400_0202 => self.flags,
            0x0400_0204 => self.waitcnt,
            0x0400_0208 => u16::from(self.master),
            0x0400_0300 => self.post_boot,
            _ => return None,
        })
    }

    pub(crate) fn write(&mut self, addr: u32, value: u16) -> bool {
        match addr {
            0x0400_0004 => {
                let before = self.matched();
                self.display_irq = value & 0xFF38;
                if !before && self.matched() && value & 0x20 != 0 { self.flags |= 4; }
            }
            0x0400_0100..=0x0400_010E => {
                let i = ((addr - 0x0400_0100) / 4) as usize;
                let t = &mut self.timers[i];
                if addr & 2 == 0 {
                    t.reload = value;
                    if t.control & 0x80 == 0 { t.count = value; }
                } else {
                    if t.control & 0x80 == 0 && value & 0x80 != 0 {
                        t.count = t.reload;
                        t.phase = 0;
                        t.starting = true;
                    }
                    t.control = value & if i == 0 { 0xC3 } else { 0xC7 };
                }
            }
            0x0400_00B0..=0x0400_00DE => {
                let offset = addr - 0x0400_00B0;
                let i = (offset / 12) as usize;
                let d = &mut self.dma[i];
                match offset % 12 {
                    0 => d.source = (d.source & 0xFFFF_0000) | u32::from(value),
                    2 => d.source = (d.source & 0xFFFF) | (u32::from(value & if i == 0 {0x07FF} else {0x0FFF}) << 16),
                    4 => d.dest = (d.dest & 0xFFFF_0000) | u32::from(value),
                    6 => d.dest = (d.dest & 0xFFFF) | (u32::from(value & if i == 3 {0x0FFF} else {0x07FF}) << 16),
                    8 => d.count = value & if i == 3 {0xFFFF} else {0x3FFF},
                    10 => {
                        let enabled = value & 0x8000 != 0;
                        if d.control & 0x8000 == 0 && enabled {
                            d.current_source = d.source;
                            d.current_dest = d.dest;
                            d.remaining = dma_count(d.count, i);
                        } else if !enabled {
                            self.dma_pending &= !(1 << i);
                            if self.dma_unit.as_ref().is_some_and(|unit| unit.channel == i) {
                                self.dma_unit = None;
                            }
                        }
                        let start_now = d.control & 0x8000 == 0 && enabled && value & 0x3000 == 0;
                        d.control = value & if i == 3 {0xFFE0} else {0xF7E0};
                        if start_now { self.queue_dma(i); }
                    }
                    _ => unreachable!(),
                }
            }
            0x0400_0060..=0x0400_0080 | 0x0400_0090..=0x0400_009E => {
                self.psg.write(addr, value, 0xFFFF, self.sound_on);
            }
            0x0400_0082 => {
                self.sound_control = value & 0x770F;
                for i in 0..2 {
                    if value & (1 << (11 + i * 4)) != 0 {
                        self.fifo[i].clear();
                        self.pcm[i] = 0;
                    }
                }
            }
            0x0400_0084 => {
                self.sound_on = value & 0x80 != 0;
                if !self.sound_on {
                    self.fifo.iter_mut().for_each(VecDeque::clear);
                    self.pcm = [0; 2];
                    self.psg.reset();
                }
            }
            0x0400_0088 => self.sound_bias = Some(value & 0xC3FE),
            0x0400_00A0..=0x0400_00A6 => {
                let fifo = &mut self.fifo[((addr - 0x0400_00A0) / 4) as usize];
                for byte in value.to_le_bytes() {
                    if fifo.len() < 32 { fifo.push_back(byte as i8); }
                }
            }
            0x0400_0132 => self.key_control = value & 0xC3FF,
            0x0400_0200 => self.enable = value & 0x3FFF,
            0x0400_0202 => self.flags &= !value,
            0x0400_0204 => self.waitcnt = value & 0x5FFF,
            0x0400_0208 => self.master = value & 1 != 0,
            0x0400_0300 => {
                self.post_boot |= value & 1;
                self.set_power((value >> 8) as u8);
            }
            _ => return false,
        }
        if self.enable & self.flags == 0 { self.irq_age = 0; }
        true
    }

    pub(crate) fn write_audio_byte(&mut self, addr: u32, value: u8) -> bool {
        let half = addr & !1;
        let shift = (addr & 1) * 8;
        let mask = 0xFF << shift;
        let half_value = u16::from(value) << shift;
        match half {
            0x0400_0060..=0x0400_0080 | 0x0400_0090..=0x0400_009E => {
                self.psg.write(half, half_value, mask, self.sound_on);
                true
            }
            0x0400_0082 | 0x0400_0084 | 0x0400_0088 => {
                let before = self.read(half).unwrap_or(0);
                self.write(half, (before & !mask) | half_value)
            }
            0x0400_00A0..=0x0400_00A6 => {
                let fifo = &mut self.fifo[((addr - 0x0400_00A0) / 4) as usize];
                if fifo.len() < 32 { fifo.push_back(value as i8); }
                true
            }
            _ => false,
        }
    }

    fn level(&self) -> [i16; 2] {
        if !self.sound_on { return [0; 2]; }
        let mut stereo = self.psg.mix(self.sound_control);
        for i in 0..2 {
            let value = i16::from(self.pcm[i]) / if self.sound_control & (4 << i) != 0 {1} else {2};
            if self.sound_control & (0x0200 << (4 * i)) != 0 { stereo[0] += value; }
            if self.sound_control & (0x0100 << (4 * i)) != 0 { stereo[1] += value; }
        }
        stereo
    }

    fn tick(&mut self) -> Option<u16> {
        self.clock = self.clock.wrapping_add(1);
        if self.enable & self.flags == 0 {
            self.irq_age = 0;
        } else if self.irq_age < IRQ_DELAY {
            self.irq_age += 1;
        }
        let mut overflow = false;
        let mut sound_requests = 0u8;
        for (i, t) in self.timers.iter_mut().enumerate() {
            if t.control & 0x80 == 0 { overflow = false; continue; }
            // 控制寄存器的启动效果在下一拍生效。
            if std::mem::take(&mut t.starting) { overflow = false; continue; }
            let clock = if t.control & 4 != 0 { overflow } else {
                t.phase += 1;
                let ready = t.phase >= PRESCALERS[(t.control & 3) as usize];
                if ready { t.phase = 0; }
                ready
            };
            overflow = clock && t.count == u16::MAX;
            if overflow {
                t.count = t.reload;
                if i < 2 && self.sound_on {
                    for j in 0..2 {
                        if usize::from(self.sound_control >> (10 + j * 4) & 1) == i {
                            self.pcm[j] = self.fifo[j].pop_front().unwrap_or(0);
                            if self.fifo[j].len() <= 16 { sound_requests |= 1 << j; }
                        }
                    }
                }
                if t.control & 0x40 != 0 { self.flags |= 1 << (3 + i); }
            } else if clock {
                t.count += 1;
            }
        }

        if sound_requests != 0 {
            self.trigger_sound_dma(sound_requests);
        }
        if self.sound_on { self.psg.tick(); }
        self.sample_phase += 1;
        if self.sample_phase == 512 {
            self.sample_phase = 0;
            if self.samples.len() < 32768 {
                self.samples.push(self.level().map(|v| v.saturating_mul(128)));
            }
        }
        self.dot += 1;
        if self.dot == HBLANK_START {
            if self.display_irq & 0x10 != 0 { self.flags |= 2; }
            if (DISPLAY_DMA_FIRST_LINE..DISPLAY_DMA_END_LINE).contains(&self.line) {
                self.trigger_display_dma();
            }
            if self.line < VISIBLE_LINES { return Some(2); }
        }
        if self.dot == LINE_CYCLES {
            self.dot = 0;
            self.line = (self.line + 1) % FRAME_LINES;
            if self.line == DISPLAY_DMA_END_LINE && self.dma[3].control & 0xB800 == 0xB000 {
                self.dma[3].control &= !0x8000;
            }
            if self.matched() && self.display_irq & 0x20 != 0 { self.flags |= 4; }
            if self.line == VISIBLE_LINES {
                self.frame = self.frame.wrapping_add(1);
                if self.display_irq & 8 != 0 { self.flags |= 1; }
                return Some(1);
            }
        }
        None
    }
}

impl Bus {
    pub(crate) fn tile_background_vram_cycles(&self, halfwords: u16, layers: u16, mixed: bool) -> u32 {
        if self.hw.line >= VISIBLE_LINES { return u32::from(halfwords); }
        let text_layers = if mixed { layers & 0x0300 } else { layers };
        let affine = mixed && layers & 0x0400 != 0;
        let mut dot = self.hw.dot + 1;
        for _ in 0..halfwords {
            // Mode 1 的普通背景占前两拍中的位置，仿射 BG2 取后两拍。
            while (affine && (FIRST_BG_FETCH_GROUP..HBLANK_START).contains(&dot) && dot % 4 >= 2)
                || self.text_background_fetches(dot, text_layers)
            {
                dot += 1;
            }
            dot += 1;
        }
        u32::from(dot - self.hw.dot - 1)
    }

    fn text_background_fetches(&self, dot: u16, layers: u16) -> bool {
        if dot > LAST_TEXT_TILE_START + 28 { return false; }
        let layer = usize::from(dot % 4);
        if layers & (0x0100 << layer) == 0 { return false; }
        let step = dot / 4 + (self.bg_scroll[layer * 2] & 7);
        if step < 8 { return false; }
        let phase = step % 8;
        if phase == 0 { return dot <= HBLANK_START; }

        // 晚到的地图读取只取编号。此前开始的图块可以读到消隐内。
        let map_dot = dot - phase * 4;
        let full_palette = self.bg_control[layer] & 0x0080 != 0;
        map_dot <= LAST_TEXT_TILE_START
            && (phase == 1 || phase == 5 || (full_palette && phase % 2 == 1))
    }

    /// 推进指定的整机时钟拍数，DMA 也在这些周期内占用总线。
    pub fn advance(&mut self, cycles: u32) -> Result<(), Fault> {
        for _ in 0..cycles {
            self.begin_dma_unit();
            // 内部 DMA 占用 CPU 的访存时隙，卡带总线仍可供预取使用。
            if !self.hw.dma_unit.as_ref().is_some_and(|unit| unit.uses_gamepak) {
                self.fill_gamepak(1);
            }
            self.tick_hardware();
            self.finish_dma_cycle()?;
        }
        Ok(())
    }

    // 一次 CPU 总线访问已开始时，等到访问边界再把总线交给 DMA。
    pub(crate) fn advance_cpu_access(&mut self, cycles: u32) {
        for _ in 0..cycles { self.tick_hardware(); }
    }

    fn tick_hardware(&mut self) {
        if let Some(trigger) = self.hw.tick() {
            if trigger == 2 {
                self.capture_line(usize::from(self.hw.line));
            } else {
                self.reload_affine_origins();
            }
            self.hw.trigger_dma(trigger);
        }
    }

    pub(crate) fn dma_owns_bus(&self) -> bool {
        self.hw.dma_unit.is_some() || self.hw.ready_dma().is_some()
    }

    pub(crate) fn wait_for_dma(&mut self) -> Result<(), Fault> {
        while self.dma_owns_bus() { self.advance(1)?; }
        Ok(())
    }

    pub fn sound_level(&self) -> [i16; 2] { self.hw.level() }

    /// 以 32,768 Hz 输出累积的左右声道采样。
    pub fn take_audio_samples(&mut self) -> Vec<[i16; 2]> {
        std::mem::take(&mut self.hw.samples)
    }

    /// 无 BIOS 时，将 VBlankIntrWait 近似为等待下一次可见区结束。
    pub(crate) fn wait_for_vblank(&mut self) -> Result<(), Fault> {
        let lines = (VISIBLE_LINES + FRAME_LINES - self.hw.line) % FRAME_LINES;
        let cycles = if lines == 0 { FRAME_LINES } else { lines };
        self.advance(u32::from(cycles) * u32::from(LINE_CYCLES) - u32::from(self.hw.dot))
    }

    fn begin_dma_unit(&mut self) {
        if self.hw.dma_unit.is_some() { return; }
        let Some(channel) = self.hw.ready_dma() else { return; };
        let d = self.hw.dma[channel];
        self.start_dma_access(channel, d.current_source, DmaPhase::Read);
    }

    fn start_dma_access(&mut self, channel: usize, addr: u32, phase: DmaPhase) {
        let d = self.hw.dma[channel];
        let uses_gamepak = matches!(addr >> 24, 8..=15);
        let cycles = self.access_cycles(addr, d.width(channel), d.sequential);
        if uses_gamepak {
            self.gamepak_prefetch = Default::default();
        }
        self.hw.dma_last_access = true;
        self.hw.dma_unit = Some(DmaUnit { channel, cycles, phase, uses_gamepak });
    }

    fn finish_dma_cycle(&mut self) -> Result<(), Fault> {
        let Some(unit) = self.hw.dma_unit.as_mut() else { return Ok(()); };
        unit.cycles -= 1;
        if unit.cycles != 0 { return Ok(()); }
        let DmaUnit { channel, phase, .. } = self.hw.dma_unit.take().unwrap();
        let dma = self.hw.dma[channel];
        let width = dma.width(channel);
        match phase {
            DmaPhase::Read => {
                if let Some(value) = self.read_dma_source(channel, dma.current_source, width) {
                    self.hw.dma[channel].latch = value;
                }
                self.start_dma_access(channel, dma.current_dest, DmaPhase::Write(self.hw.dma[channel].latch));
            }
            DmaPhase::Write(value) => {
                let dest = dma.current_dest;
                let eeprom = if channel == 3 && width == 2 && crate::Eeprom::addressed(dest, self.rom.len()) {
                    if let crate::CartridgeSave::Eeprom(eeprom) = &mut self.cartridge_save {
                        eeprom.write_bit(value as u16, dma.remaining, !dma.sequential, self.hw.clock);
                        true
                    } else { false }
                } else { false };
                let result = if channel == 3 && (0x0E00_0000..0x1000_0000).contains(&dest)
                    && matches!(self.cartridge_save, crate::CartridgeSave::Sram(_))
                {
                    // SRAM 总线只写入最低字节，地址已按 DMA 传输宽度对齐。
                    self.write_byte(dest, value as u8)
                } else if eeprom || dest >= 0x0E00_0000 { Ok(()) }
                    else if width == 4 { self.write_word(dest, value) }
                    else { self.write_half(dest, (value >> ((dest & 2) * 8)) as u16) };
                match result {
                    Ok(()) | Err(Fault::Unmapped(_) | Fault::ReadOnly(_)) => {}
                    Err(fault) => return Err(fault),
                }
                if self.hw.dma_pending & (1 << channel) != 0 {
                    if dma.remaining == 1 {
                        // 最后一笔写入后，总线结束传输并交还给 CPU。
                        self.hw.dma_unit = Some(DmaUnit {
                            channel, cycles: DMA_FINISH_DELAY, phase: DmaPhase::Complete, uses_gamepak: false,
                        });
                    } else {
                        self.hw.finish_dma_unit(channel, width);
                    }
                }
            }
            DmaPhase::Complete => self.hw.finish_dma_unit(channel, width),
        }
        Ok(())
    }

    fn read_dma_source(&mut self, channel: usize, addr: u32, width: u32) -> Option<u32> {
        if channel == 3 && width == 2 && crate::Eeprom::addressed(addr, self.rom.len()) {
            if let crate::CartridgeSave::Eeprom(eeprom) = &mut self.cartridge_save {
                return Some(u32::from(eeprom.read_bit(self.hw.clock)) * 0x1_0001);
            }
        }
        if channel != 0 && (0x0E00_0000..0x1000_0000).contains(&addr)
            && matches!(self.cartridge_save, crate::CartridgeSave::Sram(_))
        {
            // SRAM 的单字节在 DMA 数据锁存器中重复，半字传输取其中一半。
            return Some(u32::from(self.cartridge_save.read_byte(addr)) * 0x0101_0101);
        }
        if !(0x0200_0000..0x0E00_0000).contains(&addr) { return None; }
        if width == 4 {
            self.read_word(addr).ok().or_else(|| gamepak_rom(addr).then(|| {
                u32::from(vacant_gamepak_halfword(addr))
                    | (u32::from(vacant_gamepak_halfword(addr + 2)) << 16)
            }))
        } else {
            self.read_half(addr).ok().or_else(|| gamepak_rom(addr).then(|| vacant_gamepak_halfword(addr)))
                .map(|half| u32::from(half) * 0x1_0001)
        }
    }
}

impl Hardware {
    pub(crate) fn clock(&self) -> u64 { self.clock }

    pub(crate) fn take_dma_bus_access(&mut self) -> bool {
        std::mem::take(&mut self.dma_last_access)
    }

    fn trigger_dma(&mut self, trigger: u16) {
        for channel in 0..self.dma.len() {
            if self.dma[channel].control & 0xB000 == (0x8000 | (trigger << 12))
                && self.dma_pending & (1 << channel) == 0
            {
                self.queue_dma(channel);
            }
        }
    }

    fn trigger_sound_dma(&mut self, requests: u8) {
        for channel in 1..=2 {
            if self.dma_pending & (1 << channel) != 0 { continue; }
            if let Some(dest) = self.dma[channel].sound_destination(channel) {
                let bit = (dest - 0x0400_00A0) / 4;
                if requests & (1 << bit) != 0 {
                    self.dma[channel].remaining = 4;
                    self.queue_dma(channel);
                }
            }
        }
    }

    fn trigger_display_dma(&mut self) {
        if self.dma[3].control & 0xB800 == 0xB000 && self.dma_pending & (1 << 3) == 0 {
            self.queue_dma(3);
        }
    }

    fn queue_dma(&mut self, channel: usize) {
        let dma = &mut self.dma[channel];
        let mask = !(dma.width(channel) - 1);
        dma.current_source &= mask;
        dma.current_dest &= mask;
        dma.ready_at = self.clock.wrapping_add(DMA_START_DELAY);
        dma.sequential = false;
        self.dma_pending |= 1 << channel;
    }

    fn ready_dma(&self) -> Option<usize> {
        if self.dma_pending == 0 { return None; }
        self.dma.iter().enumerate().position(|(index, dma)| {
            self.dma_pending & (1 << index) != 0 && (self.clock.wrapping_sub(dma.ready_at) as i64) >= 0
        })
    }

    fn finish_dma_unit(&mut self, channel: usize, width: u32) {
        let dma = &mut self.dma[channel];
        let dest_mode = if dma.sound_destination(channel).is_some() { 2 } else { (dma.control >> 5) & 3 };
        dma.current_source = dma_next_address(dma.current_source, width, (dma.control >> 7) & 3);
        dma.current_dest = dma_next_address(dma.current_dest, width, dest_mode);
        dma.remaining -= 1;
        dma.sequential = true;
        if dma.remaining != 0 { return; }
        self.dma_pending &= !(1 << channel);
        if dma.control & 0x4000 != 0 { self.flags |= 1 << (8 + channel); }
        if dma.control & 0x3000 == 0 || dma.control & 0x0200 == 0 {
            dma.control &= !0x8000;
        } else {
            dma.remaining = dma_count(dma.count, channel);
            if dest_mode == 3 { dma.current_dest = dma.dest; }
        }
    }
}

fn dma_count(count: u16, channel: usize) -> u32 {
    if count != 0 { u32::from(count) } else if channel == 3 { 0x1_0000 } else { 0x4000 }
}

fn dma_next_address(addr: u32, width: u32, mode: u16) -> u32 {
    match mode {
        1 => addr.wrapping_sub(width),
        2 => addr,
        _ => addr.wrapping_add(width),
    }
}
