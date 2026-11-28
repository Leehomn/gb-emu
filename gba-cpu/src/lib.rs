mod hardware;
mod cpu_bus;
mod cartridge_save;
mod audio;
use std::ops::Range;
use cartridge_save::{CartridgeSave, Eeprom};
use cpu_bus::{CpuBus, Pipeline};
pub mod square;

pub const RAM_START: u32 = 0x0200_0000;
pub const ROM_START: u32 = 0x0800_0000;
const RAM_SIZE: usize = 256 * 1024;
pub const KEYINPUT: u32 = 0x0400_0130;
pub const DISPLAY_CONTROL: u32 = 0x0400_0000;
const GREEN_SWAP: u32 = 0x0400_0002;
const BG_CONTROL_START: u32 = 0x0400_0008;
const BG_CONTROL_END: u32 = 0x0400_000E;
const BG_SCROLL_START: u32 = 0x0400_0010;
const BG_SCROLL_END: u32 = 0x0400_001E;
const BG_AFFINE_START: u32 = 0x0400_0020;
const BG_AFFINE_END: u32 = 0x0400_003E;
const WIN_BOUNDS_START: u32 = 0x0400_0040;
const WIN_BOUNDS_END: u32 = 0x0400_0046;
const WIN_IN: u32 = 0x0400_0048;
const WIN_OUT: u32 = 0x0400_004A;
const MOSAIC: u32 = 0x0400_004C;
const BLDCNT: u32 = 0x0400_0050;
const BLDALPHA: u32 = 0x0400_0052;
const BLDY: u32 = 0x0400_0054;
pub const VRAM_START: u32 = 0x0600_0000;
pub const WIDTH: usize = 240;
pub const HEIGHT: usize = 160;
const VRAM_SIZE: usize = 96 * 1024;
const ROM_SIZE: u32 = 32 * 1024 * 1024;

fn vram_offset(addr: u32, display_mode: u16) -> Option<usize> {
    let offset = (addr & 0x1_FFFF) as usize;
    if offset < VRAM_SIZE { return Some(offset); }
    let mirrored = offset - 0x8000;
    // 位图占用到 0x13FFF，覆盖这段的精灵显存镜像会被屏蔽。
    (display_mode < 3 || mirrored >= 0x14000).then_some(mirrored)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Fault {
    Unsupported(u32),
    Unmapped(u32),
    Unaligned(u32),
    ReadOnly(u32),
    RomTooLarge(usize),
    StepLimit,
    InvalidSaveSize(usize),
    InvalidBiosSize(usize),
}

struct AffineBackground {
    registers: [u16; 8],
    origin: [i32; 2],
    mosaic_origin: [i32; 2],
}

impl Default for AffineBackground {
    fn default() -> Self {
        // 直启卡带时，从单位矩阵开始。
        Self {
            registers: [0x100, 0, 0, 0x100, 0, 0, 0, 0],
            origin: [0; 2],
            mosaic_origin: [0; 2],
        }
    }
}

impl AffineBackground {
    fn reference(&self, axis: usize) -> i32 {
        let index = 4 + axis * 2;
        let raw = i32::from(self.registers[index])
            | (i32::from(self.registers[index + 1]) << 16);
        // 参考点是有符号 28 位定点数，最低 8 位为小数。
        (raw << 4) >> 4
    }

    fn write(&mut self, index: usize, value: u16) {
        self.registers[index] = value;
        if index >= 4 {
            let axis = (index - 4) / 2;
            self.origin[axis] = self.reference(axis);
        }
    }

    fn line_step(&self, axis: usize) -> i32 {
        i32::from(self.registers[axis * 2 + 1] as i16)
    }

    fn sample(&self, [x, y]: [i32; 2], column: usize) -> (i32, i32) {
        let column = column as i32;
        (
            (x + column * i32::from(self.registers[0] as i16)) >> 8,
            (y + column * i32::from(self.registers[2] as i16)) >> 8,
        )
    }
}

#[derive(Clone, Copy)]
struct ScanlinePixel {
    color: u16,
    priority: u16,
    layer: u8,
    semi_transparent: bool,
}

impl ScanlinePixel {
    fn add_background(pair: &mut [Self; 2], pixel: Self) {
        // 背景按编号从小到大绘制，同优先级时保留先进入的图层。
        if pixel.priority < pair[0].priority {
            pair[1] = pair[0];
            pair[0] = pixel;
        } else if pixel.priority < pair[1].priority {
            pair[1] = pixel;
        }
    }
}

pub struct Bus {
    ram: Box<[u8]>,
    rom: Vec<u8>,
    vram: Box<[u8]>,
    display_control: u16,
    green_swap: u16,
    bg_control: [u16; 4],
    bg_scroll: [u16; 8],
    bg_affine: [AffineBackground; 2],
    window_bounds: [u16; 4],
    window_control: [u16; 2],
    mosaic: u16,
    blend_control: u16,
    blend_alpha: u16,
    brightness: u16,
    keyinput: u16,
    pub(crate) hw: hardware::Hardware,
    bios: Option<Vec<u8>>,
    bios_latch: u32,
    gamepak_prefetch: cpu_bus::GamePakPrefetch,
    iwram: Box<[u8]>,
    cartridge_save: CartridgeSave,
    palette: [u8; 1024],
    oam: [u8; 1024],
    scanout: Box<[u8]>,
    pending_line: [u8; WIDTH * 3],
    drawn_columns: usize,
}

impl Bus {
    pub fn new(program: &[u32]) -> Self {
        assert!(program.len() <= ROM_SIZE as usize / 4);
        let bytes = program.iter().flat_map(|word| word.to_le_bytes()).collect();
        Self::from_rom(bytes).unwrap()
    }

    pub fn from_rom(rom: Vec<u8>) -> Result<Self, Fault> {
        if rom.len() > ROM_SIZE as usize {
            return Err(Fault::RomTooLarge(rom.len()));
        }
        Ok(Self {
            ram: vec![0; RAM_SIZE].into_boxed_slice(),
            vram: vec![0; VRAM_SIZE].into_boxed_slice(),
            display_control: 0,
            green_swap: 0,
            bg_control: [0; 4],
            bg_scroll: [0; 8],
            bg_affine: Default::default(),
            window_bounds: [0; 4],
            window_control: [0; 2],
            mosaic: 0,
            blend_control: 0,
            blend_alpha: 0,
            brightness: 0,
            keyinput: 0x03FF,
            hw: hardware::Hardware::default(),
            bios: None,
            bios_latch: 0,
            gamepak_prefetch: cpu_bus::GamePakPrefetch::default(),
            iwram: vec![0; 32 * 1024].into_boxed_slice(),
            cartridge_save: CartridgeSave::from_rom(&rom),
            palette: [0; 1024],
            oam: [0; 1024],
            scanout: vec![0; WIDTH * HEIGHT * 3].into_boxed_slice(),
            pending_line: [0; WIDTH * 3],
            drawn_columns: 0,
            rom,
        })
    }

    fn read_bytes(&self, addr: u32, len: usize) -> Result<&[u8], Fault> {
        let bytes = if addr < 0x4000 {
            self.bios.as_ref().and_then(|b| b.get(addr as usize..addr as usize + len))
        } else if (0x0300_0000..0x0400_0000).contains(&addr) {
            let offset = (addr & 0x7FFF) as usize;
            self.iwram.get(offset..offset + len)
        } else if (0x0500_0000..0x0600_0000).contains(&addr) {
            let offset = (addr & 1023) as usize;
            self.palette.get(offset..offset + len)
        } else if (0x0700_0000..0x0800_0000).contains(&addr) {
            let offset = (addr & 1023) as usize;
            self.oam.get(offset..offset + len)
        } else if (VRAM_START..0x0700_0000).contains(&addr) {
            match vram_offset(addr, self.display_control & 7) {
                Some(offset) => self.vram.get(offset..offset + len),
                None => Some(&[0; 4][..len]),
            }
        } else if (RAM_START..0x0300_0000).contains(&addr) {
            let offset = (addr as usize) & (RAM_SIZE - 1);
            self.ram.get(offset..offset + len)
        } else if (ROM_START..0x0E00_0000).contains(&addr) {
            let offset = ((addr - ROM_START) & (ROM_SIZE - 1)) as usize;
            self.rom.get(offset..offset + len)
        } else {
            None
        };
        bytes.ok_or(Fault::Unmapped(addr))
    }

    pub fn read_word(&self, addr: u32) -> Result<u32, Fault> {
        if addr & 3 != 0 {
            return Err(Fault::Unaligned(addr));
        }
        if (0x0400_0000..0x0400_0400).contains(&addr) {
            let low = self.io_read(addr);
            let high = self.io_read(addr + 2);
            if low.is_none() && high.is_none() { return Err(Fault::Unmapped(addr)); }
            return Ok(u32::from(low.unwrap_or(0)) | u32::from(high.unwrap_or(0)) << 16);
        }
        Ok(u32::from_le_bytes(self.read_bytes(addr, 4)?.try_into().unwrap()))
    }

    fn fetch_thumb(&self, addr: u32) -> Result<u16, Fault> {
        if addr & 1 != 0 {
            return Err(Fault::Unaligned(addr));
        }
        Ok(u16::from_le_bytes(self.read_bytes(addr, 2)?.try_into().unwrap()))
    }

    pub fn write_word(&mut self, addr: u32, value: u32) -> Result<(), Fault> {
        if addr & 3 != 0 {
            return Err(Fault::Unaligned(addr));
        }
        if (ROM_START..0x0E00_0000).contains(&addr) {
            return Err(Fault::ReadOnly(addr));
        }
        if (0x0400_0000..0x0400_0400).contains(&addr) {
            let low = self.io_write(addr, value as u16);
            let high = self.io_write(addr + 2, (value >> 16) as u16);
            if !low && !high { return Err(Fault::Unmapped(addr)); }
            return Ok(());
        }
        if self.write_extra(addr, &value.to_le_bytes()) { return Ok(()); }
        if !(RAM_START..0x0300_0000).contains(&addr) {
            return Err(Fault::Unmapped(addr));
        }
        let offset = (addr as usize) & (RAM_SIZE - 1);
        self.ram[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        Ok(())
    }

    fn io_read(&self, addr: u32) -> Option<u16> {
        match addr {
            DISPLAY_CONTROL => Some(self.display_control),
            GREEN_SWAP => Some(self.green_swap),
            BG_CONTROL_START..=BG_CONTROL_END => {
                Some(self.bg_control[((addr - BG_CONTROL_START) / 2) as usize])
            }
            WIN_IN..=WIN_OUT => Some(self.window_control[((addr - WIN_IN) / 2) as usize]),
            BLDCNT => Some(self.blend_control),
            BLDALPHA => Some(self.blend_alpha),
            KEYINPUT => Some(self.keyinput),
            _ => self.hw.read(addr),
        }
    }

    fn io_write(&mut self, addr: u32, value: u16) -> bool {
        // 仿射参考点重置当前取样起点，即使写回同一个值也可能改变画面。
        let affine_reference = matches!(addr, 0x0400_0028..=0x0400_002E | 0x0400_0038..=0x0400_003E);
        if self.stored_video_register(addr).is_some_and(|old| old != value) || affine_reference {
            self.finish_pixels_before_video_write();
        }
        match addr {
            DISPLAY_CONTROL => {
                self.display_control = value;
                self.hw.initialize_display_latch(value);
            }
            GREEN_SWAP => self.green_swap = value & 1,
            BG_CONTROL_START..=BG_CONTROL_END => {
                self.bg_control[((addr - BG_CONTROL_START) / 2) as usize] = value;
            }
            BG_SCROLL_START..=BG_SCROLL_END => {
                self.bg_scroll[((addr - BG_SCROLL_START) / 2) as usize] = value & 0x1FF;
            }
            BG_AFFINE_START..=BG_AFFINE_END => {
                let index = ((addr - BG_AFFINE_START) / 2) as usize;
                self.bg_affine[index / 8].write(index % 8, value);
            }
            WIN_BOUNDS_START..=WIN_BOUNDS_END => {
                self.window_bounds[((addr - WIN_BOUNDS_START) / 2) as usize] = value;
            }
            WIN_IN..=WIN_OUT => {
                self.window_control[((addr - WIN_IN) / 2) as usize] = value & 0x3F3F;
            }
            MOSAIC => self.mosaic = value,
            BLDCNT => self.blend_control = value & 0x3FFF,
            BLDALPHA => self.blend_alpha = value & 0x1F1F,
            BLDY => self.brightness = value & 0x1F,
            0x0400_0132 => {
                self.hw.write(addr, value);
                self.hw.update_keys(self.keyinput);
            }
            0x0400_0204 => {
                if self.hw.waitcnt != value & 0x5FFF {
                    self.gamepak_prefetch = cpu_bus::GamePakPrefetch::default();
                }
                self.hw.write(addr, value);
            }
            _ => return self.hw.write(addr, value),
        }
        true
    }

    fn stored_video_register(&self, addr: u32) -> Option<u16> {
        Some(match addr {
            DISPLAY_CONTROL => self.display_control,
            GREEN_SWAP => self.green_swap,
            BG_CONTROL_START..=BG_CONTROL_END => {
                self.bg_control[((addr - BG_CONTROL_START) / 2) as usize]
            }
            BG_SCROLL_START..=BG_SCROLL_END => {
                self.bg_scroll[((addr - BG_SCROLL_START) / 2) as usize]
            }
            BG_AFFINE_START..=BG_AFFINE_END => {
                let index = ((addr - BG_AFFINE_START) / 2) as usize;
                self.bg_affine[index / 8].registers[index % 8]
            }
            WIN_BOUNDS_START..=WIN_BOUNDS_END => {
                self.window_bounds[((addr - WIN_BOUNDS_START) / 2) as usize]
            }
            WIN_IN..=WIN_OUT => self.window_control[((addr - WIN_IN) / 2) as usize],
            MOSAIC => self.mosaic,
            BLDCNT => self.blend_control,
            BLDALPHA => self.blend_alpha,
            BLDY => self.brightness,
            _ => return None,
        })
    }

    fn write_extra(&mut self, addr: u32, data: &[u8]) -> bool {
        if (0x0500_0000..0x0800_0000).contains(&addr) {
            self.finish_pixels_before_video_write();
        }
        let (buffer, offset): (&mut [u8], usize) = match addr {
            0x0300_0000..=0x03FF_FFFF => (&mut self.iwram, (addr & 0x7FFF) as usize),
            0x0500_0000..=0x05FF_FFFF => (&mut self.palette, (addr & 1023) as usize),
            VRAM_START..=0x06FF_FFFF => {
                let Some(offset) = vram_offset(addr, self.display_control & 7) else { return true; };
                (&mut self.vram, offset)
            }
            0x0700_0000..=0x07FF_FFFF => (&mut self.oam, (addr & 1023) as usize),
            _ => return false,
        };
        buffer[offset..offset + data.len()].copy_from_slice(data);
        true
    }

    pub fn read_half(&self, addr: u32) -> Result<u16, Fault> {
        if addr & 1 != 0 { return Err(Fault::Unaligned(addr)); }
        if let Some(value) = self.io_read(addr) { return Ok(value); }
        if let CartridgeSave::Eeprom(eeprom) = &self.cartridge_save {
            if Eeprom::addressed(addr, self.rom.len()) {
                return Ok(eeprom.ready(self.hw.clock()));
            }
        }
        self.fetch_thumb(addr)
    }

    pub fn write_half(&mut self, addr: u32, value: u16) -> Result<(), Fault> {
        if addr & 1 != 0 { return Err(Fault::Unaligned(addr)); }
        if matches!(addr, 0x0400_0006 | KEYINPUT) { return Err(Fault::ReadOnly(addr)); }
        if self.io_write(addr, value) { return Ok(()); }
        let data = value.to_le_bytes();
        if self.write_extra(addr, &data) { return Ok(()); }
        let (bytes, offset) = if (RAM_START..0x0300_0000).contains(&addr) {
            (&mut self.ram, (addr as usize) & (RAM_SIZE - 1))
        } else if (ROM_START..0x0E00_0000).contains(&addr) || addr < 0x4000 {
            return Err(Fault::ReadOnly(addr));
        } else { return Err(Fault::Unmapped(addr)); };
        bytes[offset..offset + 2].copy_from_slice(&data);
        Ok(())
    }

    pub fn read_byte(&self, addr: u32) -> Result<u8, Fault> {
        if (0x0E00_0000..0x1000_0000).contains(&addr) {
            return Ok(self.cartridge_save.read_byte(addr));
        }
        if let Some(value) = self.io_read(addr & !1) {
            return Ok((value >> ((addr & 1) * 8)) as u8);
        }
        Ok(self.read_bytes(addr, 1)?[0])
    }

    pub fn write_byte(&mut self, addr: u32, value: u8) -> Result<(), Fault> {
        match addr {
            RAM_START..=0x02FF_FFFF => self.ram[(addr as usize) & (RAM_SIZE - 1)] = value,
            0x0300_0000..=0x03FF_FFFF => self.iwram[(addr & 0x7FFF) as usize] = value,
            0x0E00_0000..=0x0FFF_FFFF => self.cartridge_save.write_byte(addr, value),
            DISPLAY_CONTROL..=0x0400_0003 | BG_CONTROL_START..=0x0400_004D
                | BLDCNT..=0x0400_0055 => {
                let half = addr & !1;
                let previous = self.stored_video_register(half).unwrap();
                let mut bytes = previous.to_le_bytes();
                bytes[(addr & 1) as usize] = value;
                self.io_write(half, u16::from_le_bytes(bytes));
            }
            0x0000_0000..=0x0000_3FFF | 0x0800_0000..=0x0DFF_FFFF => return Err(Fault::ReadOnly(addr)),
            // 显存和调色板是 16 位总线，CPU 字节写复制到同一半字两端。
            0x0500_0000..=0x06FF_FFFF => {
                let mode = self.display_control & 7;
                let obj_start = if mode >= 3 { 0x14000 } else { 0x10000 };
                if addr >= VRAM_START && vram_offset(addr, mode).is_none_or(|offset| offset >= obj_start) {
                    return Ok(());
                }
                return self.write_half(addr & !1, u16::from(value) * 0x0101);
            }
            0x0700_0000..=0x07FF_FFFF => {} // OAM 不接受字节写。
            0x0400_0060..=0x0400_00A7 if self.hw.write_audio_byte(addr, value) => {}
            0x0400_0300 => self.hw.set_post_boot(value),
            0x0400_0301 => self.hw.set_power(value),
            0x0400_0202..=0x0400_0203 => {
                self.hw.write(0x0400_0202, u16::from(value) << ((addr & 1) * 8));
            }
            0x0400_0132..=0x0400_0133 | 0x0400_0200..=0x0400_0201
                | 0x0400_0204..=0x0400_0205 | 0x0400_0208..=0x0400_0209 => {
                let half = addr & !1;
                let shift = (addr & 1) * 8;
                let old = self.io_read(half).unwrap_or(0);
                self.io_write(half, (old & !(0xFF << shift)) | (u16::from(value) << shift));
            }
            _ => return Err(Fault::Unmapped(addr)),
        }
        Ok(())
    }

    pub fn sram(&self) -> &[u8] { self.save_data() }

    pub fn restore_sram(&mut self, data: &[u8]) -> Result<(), Fault> {
        self.restore_save(data)
    }

    /// 导出卡带存档芯片的原始数据。
    pub fn save_data(&self) -> &[u8] { self.cartridge_save.data() }

    /// 恢复与卡带存档芯片大小匹配的原始数据。
    pub fn restore_save(&mut self, data: &[u8]) -> Result<(), Fault> {
        self.cartridge_save.restore(data)
    }

    /// 使用调用者自备的 16 KiB BIOS，不附带 BIOS 文件。
    pub fn load_bios(&mut self, bytes: Vec<u8>) -> Result<(), Fault> {
        if bytes.len() != 0x4000 { return Err(Fault::InvalidBiosSize(bytes.len())); }
        self.bios = Some(bytes);
        Ok(())
    }

    /// 宿主传入按下的按键。GBA 的 KEYINPUT 用 0 表示按下。
    pub fn set_keys(&mut self, pressed: u16) {
        self.keyinput = !pressed & 0x03FF;
        self.hw.update_keys(self.keyinput);
    }

    /// 最近扫描完成的画面，各行在进入 HBlank 时更新。
    pub fn scanout_rgb(&self) -> &[u8] { &self.scanout }

    fn capture_line(&mut self, y: usize) {
        self.render_line_until(y, WIDTH);
        let start = y * WIDTH * 3;
        self.scanout[start..start + WIDTH * 3].copy_from_slice(&self.pending_line);
        self.drawn_columns = 0;
        // HBlank DMA 写参考点时，下一行从写入的坐标开始。
        for bg in &mut self.bg_affine {
            bg.origin[0] += bg.line_step(0);
            bg.origin[1] += bg.line_step(1);
        }
    }

    fn finish_pixels_before_video_write(&mut self) {
        if let Some((line, columns)) = self.hw.visible_write_position() {
            self.render_line_until(line, columns);
        }
    }

    fn render_line_until(&mut self, y: usize, end: usize) {
        if end <= self.drawn_columns { return; }
        let origins = [0, 1].map(|index| {
            let first_line = self.bg_sample_row(index + 2, y) == y;
            let bg = &mut self.bg_affine[index];
            if first_line {
                bg.mosaic_origin = bg.origin;
            }
            bg.mosaic_origin
        });
        let display = self.hw.latched_display_control(self.display_control, 0x1F00);
        let pixels = self.render_line(y, origins, self.drawn_columns..end, display);
        let range = self.drawn_columns * 3..end * 3;
        self.pending_line[range.clone()].copy_from_slice(&pixels[range]);
        self.drawn_columns = end;
    }

    fn reload_affine_origins(&mut self) {
        for bg in &mut self.bg_affine {
            bg.origin = [bg.reference(0), bg.reference(1)];
        }
    }

    /// 即时预览显存，用于原有的方块演示与截图工具。
    pub fn mode3_rgb(&self) -> Vec<u8> {
        (0..HEIGHT).flat_map(|y| {
            let origins = [0, 1].map(|index| {
                let bg = &self.bg_affine[index];
                let row = self.bg_sample_row(index + 2, y) as i32;
                [0, 1].map(|axis| bg.reference(axis) + row * bg.line_step(axis))
            });
            self.render_line(y, origins, 0..WIDTH, self.display_control)
        }).collect()
    }

    fn render_line(&self, y: usize, affine_origins: [[i32; 2]; 2], mut columns: Range<usize>, display: u16) -> [u8; WIDTH * 3] {
        let mut frame = [0; WIDTH * 3];
        if self.green_swap != 0 {
            // 相邻像素互换绿色，分段边界落在奇数列时也要取到配对像素。
            columns.start &= !1;
            columns.end = columns.end.next_multiple_of(2);
        }
        let output = &mut frame[columns.start * 3..columns.end * 3];
        if display & 0x80 != 0 {
            output.fill(255);
            return frame;
        }
        let mode = display & 7;
        if mode > 5 { return frame; }
        let (pixels, layer_masks) = self.scanline_layers(y, affine_origins, &columns, display);
        let effect = (self.blend_control >> 6) & 3;
        let amount = self.brightness.min(16);
        let eva = (self.blend_alpha & 31).min(16);
        let evb = (self.blend_alpha >> 8).min(16);
        let layers = pixels[columns.clone()].iter().copied();
        for ((rgb, [top, below]), &window_mask) in output.chunks_exact_mut(3).zip(layers).zip(&layer_masks[columns]) {
            let enabled = self.effect_enabled(top, window_mask);
            let blend = self.blends_pixel(top, below, enabled);
            for (channel, shift) in rgb.iter_mut().zip([0, 5, 10]) {
                let value = (top.color >> shift) & 31;
                // 先计算五位颜色分量，再扩展为八位。
                let value = match (blend, enabled, effect) {
                    (true, _, _) => ((value * eva + ((below.color >> shift) & 31) * evb) / 16).min(31),
                    (false, true, 2) => value + (31 - value) * amount / 16,
                    (false, true, 3) => value - value * amount / 16,
                    _ => value,
                } as u8;
                *channel = (value << 3) | (value >> 2);
            }
        }
        if self.green_swap != 0 {
            for pair in output.chunks_exact_mut(6) {
                pair.swap(1, 4);
            }
        }
        frame
    }

    fn effect_enabled(&self, top: ScanlinePixel, window_mask: u8) -> bool {
        window_mask & 0x20 != 0 && self.blend_control & (1 << top.layer) != 0
    }

    fn blends_pixel(&self, top: ScanlinePixel, below: ScanlinePixel, enabled: bool) -> bool {
        (top.semi_transparent || (enabled && (self.blend_control >> 6) & 3 == 1))
            && top.layer != below.layer && self.blend_control & (1 << (8 + below.layer)) != 0
    }

    fn palette_fetches_pixel(&self, row: usize, column: usize, second_color: bool, display: u16) -> bool {
        let mode = display & 7;
        if display & 0x80 != 0 || mode > 5 { return false; }
        let direct_color = matches!(mode, 3 | 5);
        if !second_color && !direct_color { return true; }
        if second_color && (self.blend_control & 0x3F00 == 0
            || ((self.blend_control >> 6) & 3 != 1 && display & 0x1000 == 0))
        {
            return false;
        }
        let origins = [0, 1].map(|index| {
            let bg = &self.bg_affine[index];
            if self.bg_sample_row(index + 2, row) == row { bg.origin } else { bg.mosaic_origin }
        });
        let (pixels, masks) = self.scanline_layers(row, origins, &(column..column + 1), display);
        let [top, below] = pixels[column];
        if second_color {
            (!direct_color || below.layer != 2)
                && self.blends_pixel(top, below, self.effect_enabled(top, masks[column]))
        } else {
            top.layer != 2
        }
    }

    fn scanline_layers(
        &self, y: usize, affine_origins: [[i32; 2]; 2], columns: &Range<usize>, display: u16,
    ) -> ([[ScanlinePixel; 2]; WIDTH], [u8; WIDTH]) {
        let mode = display & 7;
        let layer_masks = self.window_layer_masks(y, mode, columns, display);
        let backdrop = ScanlinePixel {
            color: self.palette_color(0), priority: 4, layer: 5, semi_transparent: false,
        };
        let mut pixels = [[backdrop; 2]; WIDTH];
        if mode < 3 {
            let text_layers = [4, 2, 0][usize::from(mode)];
            for bg in 0..text_layers {
                if display & (0x100 << bg) != 0 {
                    self.draw_text_background(bg, y, &mut pixels, &layer_masks, columns);
                }
            }
            let affine_layers = usize::from(mode);
            for (index, origin) in affine_origins.into_iter().take(affine_layers).enumerate() {
                let bg = index + 2;
                if display & (0x100 << bg) != 0 {
                    self.draw_affine_background(bg, origin, &mut pixels, &layer_masks, columns);
                }
            }
        } else if display & 0x400 != 0 {
            self.draw_bitmap_background(mode, affine_origins[0], &mut pixels, &layer_masks, columns);
        }
        if display & 0x1000 != 0 {
            self.overlay_sprites(y as i32, mode, &mut pixels, &layer_masks, columns);
        }
        (pixels, layer_masks)
    }

    fn window_layer_masks(&self, y: usize, mode: u16, columns: &Range<usize>, display: u16) -> [u8; WIDTH] {
        if display & 0xE000 == 0 {
            return [0x3F; WIDTH];
        }
        let mut masks = [self.window_control[1] as u8; WIDTH];
        if display & 0x9000 == 0x9000 {
            let inside = (self.window_control[1] >> 8) as u8;
            self.for_each_sprite_pixel(y as i32, mode, true, columns, |x, _, _, _| {
                masks[x] = inside;
            });
        }
        // 从低到高覆盖：精灵窗口、WIN1、WIN0。
        for window in (0..2).rev() {
            if display & (0x2000 << window) == 0
                || !Self::window_contains(self.window_bounds[2 + window], y as u8)
            {
                continue;
            }
            let inside = (self.window_control[0] >> (window * 8)) as u8;
            for (x, mask) in columns.clone().zip(&mut masks[columns.clone()]) {
                if Self::window_contains(self.window_bounds[window], x as u8) {
                    *mask = inside;
                }
            }
        }
        masks
    }

    fn window_contains(bounds: u16, position: u8) -> bool {
        let [end, start] = bounds.to_le_bytes();
        if start <= end {
            (start..end).contains(&position)
        } else {
            position >= start || position < end
        }
    }

    fn palette_color(&self, index: usize) -> u16 {
        u16::from_le_bytes([self.palette[index * 2], self.palette[index * 2 + 1]])
    }

    fn bg_mosaic_size(&self, bg: usize, shift: u32) -> usize {
        if self.bg_control[bg] & 0x40 == 0 {
            return 1;
        }
        usize::from((self.mosaic >> shift) & 15) + 1
    }

    fn bg_sample_row(&self, bg: usize, y: usize) -> usize {
        y - y % self.bg_mosaic_size(bg, 4)
    }

    fn draw_bitmap_background(
        &self,
        mode: u16,
        origin: [i32; 2],
        pixels: &mut [[ScanlinePixel; 2]; WIDTH],
        layer_masks: &[u8; WIDTH],
        columns: &Range<usize>,
    ) {
        let (width, height) = if mode == 5 { (160, 128) } else { (240, 160) };
        let page = if mode != 3 && self.display_control & 0x10 != 0 { 0xA000 } else { 0 };
        let block_width = self.bg_mosaic_size(2, 0);
        for (x, pixel) in columns.clone().zip(&mut pixels[columns.clone()]) {
            if layer_masks[x] & (1 << 2) == 0 { continue; }
            let (map_x, map_y) = self.bg_affine[0].sample(origin, x - x % block_width);
            if !(0..width).contains(&map_x) || !(0..height).contains(&map_y) { continue; }
            let offset = (map_y * width + map_x) as usize;
            let color = if mode == 4 {
                let index = self.vram[page + offset];
                if index == 0 { continue; }
                self.palette_color(usize::from(index))
            } else {
                let i = page + offset * 2;
                u16::from_le_bytes([self.vram[i], self.vram[i + 1]])
            };
            pixel[0] = ScanlinePixel {
                color, priority: self.bg_control[2] & 3, layer: 2, semi_transparent: false,
            };
        }
    }

    fn draw_affine_background(
        &self,
        bg: usize,
        origin: [i32; 2],
        pixels: &mut [[ScanlinePixel; 2]; WIDTH],
        layer_masks: &[u8; WIDTH],
        columns: &Range<usize>,
    ) {
        let control = usize::from(self.bg_control[bg]);
        let priority = (control & 3) as u16;
        let tile_base = ((control >> 2) & 3) * 0x4000;
        let map_base = ((control >> 8) & 31) * 0x800;
        let size = 128 << (control >> 14);
        let wrap = control & 0x2000 != 0;
        let block_width = self.bg_mosaic_size(bg, 0);
        for (x, pixel) in columns.clone().zip(&mut pixels[columns.clone()]) {
            if layer_masks[x] & (1 << bg) == 0 || pixel[1].priority <= priority { continue; }
            let (mut map_x, mut map_y) = self.bg_affine[bg - 2].sample(origin, x - x % block_width);
            if wrap {
                map_x &= size - 1;
                map_y &= size - 1;
            } else if !(0..size).contains(&map_x) || !(0..size).contains(&map_y) {
                continue;
            }
            let (map_x, map_y) = (map_x as usize, map_y as usize);
            let map_index = map_base + (map_y / 8) * (size as usize / 8) + map_x / 8;
            let tile = usize::from(self.vram[map_index & 0xFFFF]);
            let index = self.vram[tile_base + tile * 64 + (map_y % 8) * 8 + map_x % 8];
            if index == 0 { continue; }
            ScanlinePixel::add_background(pixel, ScanlinePixel {
                color: self.palette_color(usize::from(index)), priority, layer: bg as u8,
                semi_transparent: false,
            });
        }
    }

    fn draw_text_background(
        &self,
        bg: usize,
        y: usize,
        pixels: &mut [[ScanlinePixel; 2]; WIDTH],
        layer_masks: &[u8; WIDTH],
        columns: &Range<usize>,
    ) {
        let control = usize::from(self.bg_control[bg]);
        let priority = (control & 3) as u16;
        let tile_base = ((control >> 2) & 3) * 0x4000;
        let map_base = ((control >> 8) & 31) * 0x800;
        let wide = 1 + ((control >> 14) & 1);
        let tall = 1 + ((control >> 15) & 1);
        let depth_8 = control & 0x80 != 0;
        let scroll_x = usize::from(self.bg_scroll[bg * 2]);
        let x_mask = wide * 256 - 1;
        let y_mask = tall * 256 - 1;
        let block_width = self.bg_mosaic_size(bg, 0);
        let map_y = (self.bg_sample_row(bg, y) + usize::from(self.bg_scroll[bg * 2 + 1])) & y_mask;
        for (x, pixel) in columns.clone().zip(&mut pixels[columns.clone()]) {
            if layer_masks[x] & (1 << bg) == 0 || pixel[1].priority <= priority { continue; }
            let map_x = (x - x % block_width + scroll_x) & x_mask;
            let block = map_x / 256 + (map_y / 256) * wide;
            let tile_in_block = (map_x / 8) % 32 + ((map_y / 8) % 32) * 32;
            let entry_address = (map_base + block * 0x800 + tile_in_block * 2) & 0xFFFF;
            let entry = usize::from(u16::from_le_bytes([
                self.vram[entry_address], self.vram[entry_address + 1],
            ]));
            let mut col = map_x % 8;
            let mut row = map_y % 8;
            if entry & 0x0400 != 0 { col = 7 - col; }
            if entry & 0x0800 != 0 { row = 7 - row; }
            let tile = entry & 1023;
            let address = if depth_8 {
                tile_base + tile * 64 + row * 8 + col
            } else {
                tile_base + tile * 32 + row * 4 + col / 2
            };
            // 普通背景的图块存储区止于精灵显存之前。
            if address >= 0x10000 { continue; }
            let byte = usize::from(self.vram[address]);
            let index = if depth_8 { byte } else { (byte >> ((col & 1) * 4)) & 15 };
            if index == 0 { continue; }
            let bank = if depth_8 { 0 } else { (entry >> 12) * 16 };
            ScanlinePixel::add_background(pixel, ScanlinePixel {
                color: self.palette_color(bank + index), priority, layer: bg as u8,
                semi_transparent: false,
            });
        }
    }

    fn overlay_sprites(
        &self,
        y: i32,
        mode: u16,
        pixels: &mut [[ScanlinePixel; 2]; WIDTH],
        layer_masks: &[u8; WIDTH],
        columns: &Range<usize>,
    ) {
        let mut sprites = [None::<ScanlinePixel>; WIDTH];
        self.for_each_sprite_pixel(y, mode, false, columns, |x, index, a0, a2| {
            let priority = (a2 >> 10) & 3;
            if layer_masks[x] & (1 << 4) == 0 || pixels[x][1].priority < priority
                || sprites[x].is_some_and(|sprite| sprite.priority <= priority)
            {
                return;
            }
            let bank = if a0 & 0x2000 != 0 { 0 } else { (a2 >> 12) * 16 };
            let p = 256 + usize::from(bank + u16::from(index));
            sprites[x] = Some(ScanlinePixel {
                color: self.palette_color(p), priority, layer: 4, semi_transparent: a0 & 0x0400 != 0,
            });
        });
        // 所有精灵共用一个图层，每个位置最多保留一个精灵像素。
        for (pair, &sprite) in pixels[columns.clone()].iter_mut().zip(&sprites[columns.clone()]) {
            let Some(sprite) = sprite else { continue };
            if sprite.priority <= pair[0].priority {
                pair[1] = pair[0];
                pair[0] = sprite;
            } else {
                pair[1] = sprite;
            }
        }
    }

    // 显示精灵和精灵窗口共用同一套图块取样。
    fn sprite_size(a0: u16, a1: u16) -> Option<(i32, i32)> {
        const SIZES: [[(i32, i32); 4]; 3] = [
            [(8, 8), (16, 16), (32, 32), (64, 64)],
            [(16, 8), (32, 8), (32, 16), (64, 32)],
            [(8, 16), (8, 32), (16, 32), (32, 64)],
        ];
        SIZES.get(usize::from(a0 >> 14)).map(|sizes| sizes[usize::from(a1 >> 14)])
    }

    fn for_each_sprite_pixel(
        &self,
        y: i32,
        mode: u16,
        window_only: bool,
        columns: &Range<usize>,
        mut visit: impl FnMut(usize, u8, u16, u16),
    ) {
        let one_dimensional = self.display_control & 0x40 != 0;
        let mut cycles: i32 = if self.display_control & 0x20 == 0 { 1210 } else { 954 };
        for entry in self.oam.chunks_exact(8) {
            let a = |i| u16::from_le_bytes([entry[i], entry[i + 1]]);
            let (a0, a1, a2) = (a(0), a(2), a(4));
            let Some((width, height)) = Self::sprite_size(a0, a1) else { continue; };
            let tile = usize::from(a2 & 1023);
            let object_mode = (a0 >> 10) & 3;
            let affine = a0 & 0x0100 != 0;
            let double_size = a0 & 0x0200 != 0;
            if (!affine && double_size) || (mode >= 3 && tile < 512)
                || object_mode == 3
            {
                continue;
            }
            let scale = if double_size { 2 } else { 1 };
            let (area_width, area_height) = (width * scale, height * scale);
            let mut sy = i32::from(a0 & 255);
            if sy + area_height > 256 { sy -= 256; }
            let row = y - sy;
            if !(0..area_height).contains(&row) { continue; }
            // 窗口精灵和可见精灵按 OAM 顺序共用每行的取样时间。
            let (setup, pixel_cost) = if affine { (10, 2) } else { (0, 1) };
            let sampled_width = ((cycles - setup).max(0) / pixel_cost).min(area_width);
            cycles -= setup + area_width * pixel_cost;
            if sampled_width == 0 { break; }
            if (object_mode == 2) != window_only { continue; }
            let sx = (i32::from(a1 & 511) << 23) >> 23;
            let first_col = (columns.start as i32 - sx).max(0);
            let end_col = (columns.end as i32 - sx).min(sampled_width);
            if first_col >= end_col { continue; }
            let mosaic = a0 & 0x1000 != 0;
            let block_width = if mosaic { i32::from((self.mosaic >> 8) & 15) + 1 } else { 1 };
            let block_height = if mosaic { i32::from(self.mosaic >> 12) + 1 } else { 1 };
            let sample_row = (row - y % block_height).max(0);
            let matrix = affine.then(|| self.sprite_matrix(a1));
            let depth_8 = a0 & 0x2000 != 0;
            let tile_width = if depth_8 { 2 } else { 1 };
            let stride = if one_dimensional { width as usize / 8 * tile_width } else { 32 };
            let tile = if depth_8 && !one_dimensional { tile & !1 } else { tile };
            for col in first_col..end_col {
                let x = sx + col;
                let sample_col = (col - x % block_width).max(0);
                let (u, v) = if let Some([pa, pb, pc, pd]) = matrix {
                    let dx = sample_col - area_width / 2;
                    let dy = sample_row - area_height / 2;
                    (((pa * dx + pb * dy) >> 8) + width / 2,
                     ((pc * dx + pd * dy) >> 8) + height / 2)
                } else {
                    let u = if a1 & 0x1000 != 0 { width - 1 - sample_col } else { sample_col };
                    let v = if a1 & 0x2000 != 0 { height - 1 - sample_row } else { sample_row };
                    (u, v)
                };
                if !(0..width).contains(&u) || !(0..height).contains(&v) { continue; }
                let (u, v) = (u as usize, v as usize);
                let tile_col = u / 8 * tile_width;
                let tile_row = v / 8 * stride;
                let offset = if one_dimensional {
                    tile + tile_row + tile_col
                } else {
                    (tile & !31) + ((tile + tile_col) & 31) + tile_row
                };
                let address = 0x10000 + (offset & 1023) * 32;
                let index = if depth_8 {
                    self.vram[address + (v % 8) * 8 + u % 8]
                } else {
                    (self.vram[address + (v % 8) * 4 + (u % 8) / 2] >> ((u & 1) * 4)) & 15
                };
                if index != 0 {
                    visit(x as usize, index, a0, a2);
                }
            }
        }
    }

    fn sprite_matrix(&self, attr: u16) -> [i32; 4] {
        let base = usize::from((attr >> 9) & 31) * 32 + 6;
        [0, 8, 16, 24].map(|offset| {
            let i = base + offset;
            i32::from(i16::from_le_bytes([self.oam[i], self.oam[i + 1]]))
        })
    }
}

#[derive(Clone, Copy, Default)]
struct Flags {
    n: bool,
    z: bool,
    c: bool,
    v: bool,
}

impl Flags {
    fn nz(&mut self, value: u32) {
        self.n = value >> 31 != 0;
        self.z = value == 0;
    }

    fn allows(&self, cond: u32) -> bool {
        let passed = match cond >> 1 {
            0 => self.z,
            1 => self.c,
            2 => self.n,
            3 => self.v,
            4 => self.c && !self.z,
            5 => self.n == self.v,
            6 => !self.z && self.n == self.v,
            7 => return cond == 14,
            _ => unreachable!(),
        };
        passed ^ (cond & 1 != 0)
    }
}

pub struct Cpu {
    pub regs: [u32; 16],
    flags: Flags,
    thumb: bool,
    irq_masked: bool,
    fiq_masked: bool,
    mode: u8,
    sp_lr: [[u32; 2]; 6],
    fiq_high: [u32; 5],
    spsr: [u32; 6],
    pipeline: Option<Pipeline>,
}

impl Default for Cpu {
    fn default() -> Self {
        let mut regs = [0; 16];
        regs[13] = 0x0300_7F00; // 跳过 BIOS 时为卡带提供工作栈。
        regs[15] = ROM_START;
        let mut sp_lr = [[0; 2]; 6];
        sp_lr[2][0] = 0x0300_7FA0;
        sp_lr[3][0] = 0x0300_7FE0;
        Self { regs, flags: Flags::default(), thumb: false, irq_masked: false,
            fiq_masked: false, mode: 0x1F, sp_lr, fiq_high: [0; 5], spsr: [0; 6], pipeline: None }
    }
}

impl Cpu {
    pub fn new() -> Self { Self::default() }

    pub fn from_reset() -> Self {
        let mut cpu = Self::new();
        cpu.regs[13] = 0;
        cpu.regs[15] = 0;
        cpu.sp_lr = [[0; 2]; 6];
        cpu.irq_masked = true;
        cpu.mode = 0x13;
        cpu.fiq_masked = true;
        cpu
    }

    pub fn pc(&self) -> u32 { self.regs[15] }

    pub fn step(&mut self, bus: &mut Bus) -> Result<(), Fault> {
        if !matches!(bus.hw.power, hardware::PowerMode::Stop) && bus.dma_owns_bus() {
            return bus.advance(1);
        }
        bus.hw.wake_cpu();
        match bus.hw.power {
            hardware::PowerMode::Running => {}
            hardware::PowerMode::Halt => return bus.advance(1),
            hardware::PowerMode::Stop => return Ok(()),
        }
        let pc = self.pc();
        let width = self.instruction_width();
        if !self.irq_masked && bus.bios.is_some() && bus.hw.irq_pending() {
            bus.clock_fetch(pc.wrapping_add(2 * width), width, true)?;
            self.regs[15] = self.enter_exception(0x12, 0x18, pc.wrapping_add(4));
            self.pipeline = Some(bus.refill_pipeline(self.pc(), 4)?);
            return Ok(());
        }
        let pipeline = match self.pipeline.filter(|pipe| pipe.pc == pc && pipe.width == width) {
            Some(pipeline) => pipeline,
            None if pc == 0 && bus.bios.is_some() => bus.refill_pipeline(pc, width)?,
            None => bus.seed_pipeline(pc, width),
        };
        let mut memory = CpuBus::new(bus, pc, width, pipeline.words[1]);
        let result = if let Some(word) = pipeline.words[0] {
            if self.thumb {
                self.step_thumb(&mut memory, pc, word as u16)
            } else {
                self.step_arm(&mut memory, pc, word)
            }
        } else if memory.has_bios() {
            Ok(memory.jump(self.enter_exception(0x17, 0x0C, pc.wrapping_add(4))))
        } else {
            Err(Fault::Unmapped(pc))
        };
        let next = match result {
            Err(Fault::Unsupported(_)) if memory.has_bios() => {
                memory.jump(self.enter_exception(0x1B, 4, pc.wrapping_add(width)))
            }
            result => result?,
        };
        self.pipeline = Some(memory.finish(next, self.instruction_width())?);
        self.regs[15] = next;
        Ok(())
    }

    fn instruction_width(&self) -> u32 {
        if self.thumb { 2 } else { 4 }
    }

    fn enter_exception(&mut self, mode: u8, vector: u32, return_address: u32) -> u32 {
        self.spsr[Self::bank(mode).unwrap()] = self.cpsr();
        self.switch_mode(mode);
        self.regs[14] = return_address;
        self.irq_masked = true;
        self.thumb = false;
        vector
    }

    /// 为宿主推进到下一次 VBlank，返回前可通过 Bus 读取完整画面。
    pub fn run_frame(&mut self, bus: &mut Bus) -> Result<(), Fault> {
        let frame = bus.hw.frame;
        for _ in 0..(1232 * 228) {
            self.step(bus)?;
            if bus.hw.frame != frame { return Ok(()); }
            if matches!(bus.hw.power, hardware::PowerMode::Stop) { return Ok(()); }
        }
        Err(Fault::StepLimit)
    }

    fn bank(mode: u8) -> Option<usize> {
        Some(match mode {
            0x10 | 0x1F => 0,
            0x11 => 1,
            0x12 => 2,
            0x13 => 3,
            0x17 => 4,
            0x1B => 5,
            _ => return None,
        })
    }

    fn cpsr(&self) -> u32 {
        (u32::from(self.flags.n) << 31) | (u32::from(self.flags.z) << 30)
            | (u32::from(self.flags.c) << 29) | (u32::from(self.flags.v) << 28)
            | (u32::from(self.irq_masked) << 7) | (u32::from(self.fiq_masked) << 6)
            | (u32::from(self.thumb) << 5) | u32::from(self.mode)
    }

    fn switch_mode(&mut self, mode: u8) {
        let old = Self::bank(self.mode).unwrap();
        let new = Self::bank(mode).unwrap();
        if old != new {
            self.sp_lr[old].copy_from_slice(&self.regs[13..15]);
            self.regs[13..15].copy_from_slice(&self.sp_lr[new]);
            if (old == 1) != (new == 1) {
                self.regs[8..13].swap_with_slice(&mut self.fiq_high);
            }
        }
        self.mode = mode;
    }

    fn apply_cpsr(&mut self, value: u32) {
        self.flags.n = value & (1 << 31) != 0;
        self.flags.z = value & (1 << 30) != 0;
        self.flags.c = value & (1 << 29) != 0;
        self.flags.v = value & (1 << 28) != 0;
        self.irq_masked = value & 0x80 != 0;
        self.fiq_masked = value & 0x40 != 0;
        self.thumb = value & 0x20 != 0;
        let mode = value as u8 & 31;
        if Self::bank(mode).is_some() { self.switch_mode(mode); }
    }

    fn exchange(&mut self, target: u32) -> u32 {
        self.thumb = target & 1 != 0;
        target & if self.thumb { !1 } else { !3 }
    }

    fn arithmetic(&mut self, a: u32, b: u32, subtract: bool, set: bool) -> u32 {
        let (value, carry, overflow) = if subtract {
            let (value, borrow) = a.overflowing_sub(b);
            (value, !borrow, ((a ^ b) & (a ^ value)) >> 31 != 0)
        } else {
            let (value, carry) = a.overflowing_add(b);
            (value, carry, (!(a ^ b) & (a ^ value)) >> 31 != 0)
        };
        if set {
            self.flags.nz(value);
            self.flags.c = carry;
            self.flags.v = overflow;
        }
        value
    }

    // 运算结果和移出的最后一位。寄存器指定移位量时，0 与 32 的行为不同。
    fn shift(&self, value: u32, kind: u32, amount: u32, register: bool) -> (u32, bool) {
        if amount == 0 && (register || kind == 0) { return (value, self.flags.c); }
        match (kind, amount) {
            (0, 1..=31) => (value << amount, value >> (32 - amount) & 1 != 0),
            (0, 32) => (0, value & 1 != 0),
            (0, _) => (0, false),
            (1, 0 | 32) => (0, value >> 31 != 0),
            (1, 1..=31) => (value >> amount, value >> (amount - 1) & 1 != 0),
            (1, _) => (0, false),
            (2, 1..=31) => (((value as i32) >> amount) as u32, value >> (amount - 1) & 1 != 0),
            (2, _) => {
                let sign = value >> 31 != 0;
                (if sign { u32::MAX } else { 0 }, sign)
            }
            (3, 0) => ((value >> 1) | (u32::from(self.flags.c) << 31), value & 1 != 0),
            (3, _) => {
                let result = value.rotate_right(amount);
                (result, result >> 31 != 0)
            }
            _ => unreachable!(),
        }
    }

    fn add_with_carry(&mut self, a: u32, b: u32, carry: bool, set: bool) -> u32 {
        let sum = u64::from(a) + u64::from(b) + u64::from(carry);
        let value = sum as u32;
        if set {
            let signed = i64::from(a as i32) + i64::from(b as i32) + i64::from(carry);
            self.flags.nz(value);
            self.flags.c = sum >> 32 != 0;
            self.flags.v = i32::try_from(signed).is_err();
        }
        value
    }

    fn data_op(&mut self, op: u32, a: u32, b: u32, shifter_carry: bool, set: bool) -> u32 {
        let value = match op {
            0 | 8 => a & b,
            1 | 9 => a ^ b,
            2 | 10 => return self.add_with_carry(a, !b, true, set),
            3 => return self.add_with_carry(b, !a, true, set),
            4 | 11 => return self.add_with_carry(a, b, false, set),
            5 => return self.add_with_carry(a, b, self.flags.c, set),
            6 => return self.add_with_carry(a, !b, self.flags.c, set),
            7 => return self.add_with_carry(b, !a, self.flags.c, set),
            12 => a | b,
            13 => b,
            14 => a & !b,
            15 => !b,
            _ => unreachable!(),
        };
        if set {
            self.flags.nz(value);
            self.flags.c = shifter_carry;
        }
        value
    }

    fn step_arm(&mut self, bus: &mut CpuBus<'_>, pc: u32, word: u32) -> Result<u32, Fault> {
        let cond = word >> 28;
        if cond == 15 { return Err(Fault::Unsupported(word)); }
        if !self.flags.allows(cond) { return Ok(pc.wrapping_add(4)); }
        if word & 0x0F00_0000 == 0x0F00_0000 {
            if bus.has_bios() {
                return Ok(bus.jump(self.enter_exception(0x13, 8, pc.wrapping_add(4))));
            }
            if word & 0x0000_FFFF == 0 && bus.biosless_swi(((word >> 16) & 0xFF) as u8)? {
                return Ok(pc.wrapping_add(4));
            }
            return Err(Fault::Unsupported(word));
        }
        let rn = ((word >> 16) & 15) as usize;
        let rd = ((word >> 12) & 15) as usize;
        let rm = (word & 15) as usize;
        let saved = word & (1 << 22) != 0;
        let bank = Self::bank(self.mode).unwrap();
        if word & 0x0FBF_0FFF == 0x010F_0000 && rd != 15 {
            if saved && bank == 0 { return Err(Fault::Unsupported(word)); }
            self.regs[rd] = if saved { self.spsr[bank] } else { self.cpsr() };
            return Ok(pc.wrapping_add(4));
        }
        if word & 0x0FB0_FFF0 == 0x0120_F000 || word & 0x0FB0_F000 == 0x0320_F000 {
            let immediate = word & (1 << 25) != 0;
            if (saved && bank == 0) || (!immediate && rm == 15) {
                return Err(Fault::Unsupported(word));
            }
            let value = if immediate { (word & 255).rotate_right(((word >> 8) & 15) * 2) }
                else { self.regs[rm] };
            let fields = (word >> 16) & 15;
            let mut mask = if fields & 8 != 0 { 0xF000_0000 } else { 0 };
            if fields & 1 != 0 && self.mode != 0x10 {
                mask |= if saved { 0xFF } else { 0xDF };
            }
            if saved {
                self.spsr[bank] = (self.spsr[bank] & !mask) | (value & mask);
            } else {
                self.apply_cpsr((self.cpsr() & !mask) | (value & mask));
            }
            return Ok(pc.wrapping_add(4));
        }
        if word & 0x0FFF_FFF0 == 0x012F_FF10 {
            let target = if rm == 15 { pc.wrapping_add(8) } else { self.regs[rm] };
            return Ok(bus.jump(self.exchange(target)));
        }
        if word & 0x0E00_0000 == 0x0A00_0000 {
            let offset = ((word << 8) as i32 >> 6) as u32;
            if word & (1 << 24) != 0 { self.regs[14] = pc.wrapping_add(4); }
            return Ok(bus.jump(pc.wrapping_add(8).wrapping_add(offset)));
        }
        if word & 0x0E00_0000 == 0x0800_0000 {
            let encoded = word & 0xFFFF;
            let mask = if encoded == 0 { 1 << 15 } else { encoded };
            let load = word & (1 << 20) != 0;
            let writeback = word & (1 << 21) != 0;
            let special = word & (1 << 22) != 0;
            let restore = special && load && mask & (1 << 15) != 0;
            let user = special && !restore;
            if rn == 15 || (restore && bank == 0) {
                return Err(Fault::Unsupported(word));
            }
            let bytes = if encoded == 0 { 0x40 } else { mask.count_ones() * 4 };
            let base = self.regs[rn];
            let up = word & (1 << 23) != 0;
            let end = if up { base.wrapping_add(bytes) } else { base.wrapping_sub(bytes) };
            let before = word & (1 << 24) != 0;
            let mut addr = match (up, before) {
                (true, false) => base,
                (true, true) => base.wrapping_add(4),
                (false, false) => base.wrapping_sub(bytes).wrapping_add(4),
                (false, true) => base.wrapping_sub(bytes),
            };
            let mut next = pc.wrapping_add(4);
            if load && mask & (1 << 15) != 0 { bus.jump(next); }
            // 寄存器从低编号到高编号，对应从低地址到高地址。
            for reg in 0..16 {
                if mask & (1 << reg) == 0 { continue; }
                if load {
                    let value = bus.read_word(addr)?;
                    if reg == 15 { next = value; }
                    else if user && bank == 1 && (8..13).contains(&reg) { self.fiq_high[reg - 8] = value; }
                    else if user && bank != 0 && (13..15).contains(&reg) { self.sp_lr[0][reg - 13] = value; }
                    else { self.regs[reg] = value; }
                } else {
                    let value = if reg == 15 { pc.wrapping_add(12) }
                        else if reg == rn && writeback && reg != mask.trailing_zeros() as usize { end }
                        else if user && bank == 1 && (8..13).contains(&reg) { self.fiq_high[reg - 8] }
                        else if user && bank != 0 && (13..15).contains(&reg) { self.sp_lr[0][reg - 13] }
                        else { self.regs[reg] };
                    bus.write_word(addr, value)?;
                }
                addr = addr.wrapping_add(4);
            }
            if (writeback || encoded == 0) && (!load || mask & (1 << rn) == 0) {
                self.regs[rn] = end;
            }
            if restore { self.apply_cpsr(self.spsr[bank]); }
            return Ok(next & if self.thumb { !1 } else { !3 });
        }
        if word & 0x0FB0_0FF0 == 0x0100_0090 {
            if [rn, rd, rm].contains(&15) { return Err(Fault::Unsupported(word)); }
            let addr = self.regs[rn];
            let source = self.regs[rm];
            self.regs[rd] = if word & (1 << 22) != 0 {
                let old = bus.read_byte(addr)?;
                bus.write_byte(addr, source as u8)?;
                u32::from(old)
            } else {
                let old = bus.load_word(addr)?;
                bus.write_word(addr, source)?;
                old
            };
            return Ok(pc.wrapping_add(4));
        }
        if word & 0x0F80_00F0 == 0x0080_0090 {
            let rs = ((word >> 8) & 15) as usize;
            if [rn, rd, rs, rm].contains(&15) || rn == rd || rn == rm || rd == rm {
                return Err(Fault::Unsupported(word));
            }
            let a = self.regs[rm];
            let b = self.regs[rs];
            let signed = word & (1 << 22) != 0;
            let accumulate = word & (1 << 21) != 0;
            let accumulator = if accumulate { (u64::from(self.regs[rn]) << 32) | u64::from(self.regs[rd]) } else { 0 };
            let cycles = multiply_cycles(b, signed);
            bus.idle(cycles + 1 + u32::from(accumulate))?;
            let product = if signed {
                (i64::from(a as i32) * i64::from(b as i32)) as u64
            } else { u64::from(a) * u64::from(b) };
            let value = product.wrapping_add(accumulator);
            self.regs[rn] = (value >> 32) as u32;
            self.regs[rd] = value as u32;
            if word & (1 << 20) != 0 {
                self.flags.n = value >> 63 != 0;
                self.flags.z = value == 0;
                self.flags.c = multiply_carry(a, b, accumulator, signed, true, cycles);
            }
            return Ok(pc.wrapping_add(4));
        }
        if word & 0x0FC0_00F0 == 0x0000_0090 {
            let dest = rn;
            let rs = ((word >> 8) & 15) as usize;
            let accumulate = word & (1 << 21) != 0;
            if dest == 15 || rs == 15 || rm == 15
                || (accumulate && rd == 15) || (!accumulate && rd != 0) {
                return Err(Fault::Unsupported(word));
            }
            let a = self.regs[rm];
            let b = self.regs[rs];
            let cycles = multiply_cycles(b, true);
            let accumulator = if accumulate { self.regs[rd] } else { 0 };
            bus.idle(cycles + u32::from(accumulate))?;
            let value = a.wrapping_mul(b).wrapping_add(accumulator);
            self.regs[dest] = value;
            if word & (1 << 20) != 0 {
                self.flags.nz(value);
                self.flags.c = multiply_carry(a, b, u64::from(accumulator), true, false, cycles);
            }
            return Ok(pc.wrapping_add(4));
        }
        if word & 0x0E00_0090 == 0x0000_0090 && word & 0x60 != 0 && rd != 15 {
            let load = word & (1 << 20) != 0;
            let immediate = word & (1 << 22) != 0;
            let pre = word & (1 << 24) != 0;
            let writeback = !pre || word & (1 << 21) != 0;
            if (!pre && word & (1 << 21) != 0)
                || (writeback && rn == 15)
                || (!immediate && (rm == 15 || word & 0xF00 != 0)) {
                return Err(Fault::Unsupported(word));
            }
            let base = if rn == 15 { pc.wrapping_add(8) } else { self.regs[rn] };
            let offset = if immediate { ((word >> 4) & 0xF0) | (word & 15) }
                else { self.regs[rm] };
            let adjusted = if word & (1 << 23) != 0 { base.wrapping_add(offset) }
                else { base.wrapping_sub(offset) };
            let addr = if pre { adjusted } else { base };
            match (load, (word >> 5) & 3) {
                (false, 1) => bus.write_half(addr, self.regs[rd] as u16)?,
                (true, 1) => self.regs[rd] = bus.load_half(addr)?,
                (true, 2) => self.regs[rd] = bus.read_byte(addr)? as i8 as i32 as u32,
                (true, 3) if addr & 1 != 0 => self.regs[rd] = bus.read_byte(addr)? as i8 as i32 as u32,
                (true, 3) => self.regs[rd] = bus.read_half(addr)? as i16 as i32 as u32,
                _ => return Err(Fault::Unsupported(word)),
            }
            if writeback && !(load && rd == rn) { self.regs[rn] = adjusted; }        } else if word & 0x0C00_0000 == 0x0400_0000 {
            let base = if rn == 15 { pc.wrapping_add(8) } else { self.regs[rn] };
            let load = word & (1 << 20) != 0;
            let byte = word & (1 << 22) != 0;
            let pre = word & (1 << 24) != 0;
            let writeback = !pre || word & (1 << 21) != 0;
            if (writeback && rn == 15) || (byte && rd == 15) {
                return Err(Fault::Unsupported(word));
            }
            let offset = if word & (1 << 25) == 0 { word & 0xFFF } else {
                if word & 16 != 0 || rm == 15 { return Err(Fault::Unsupported(word)); }
                self.shift(self.regs[rm], (word >> 5) & 3, (word >> 7) & 31, false).0
            };
            let adjusted = if word & (1 << 23) != 0 { base.wrapping_add(offset) }
                else { base.wrapping_sub(offset) };
            let addr = if pre { adjusted } else { base };
            let mut next = pc.wrapping_add(4);
            if load {
                if rd == 15 { bus.jump(next); }
                let value = if byte { u32::from(bus.read_byte(addr)?) }
                else { bus.load_word(addr)? };
                if rd == 15 { next = bus.jump(value & !3); } else { self.regs[rd] = value; }
            } else {
                let value = if rd == 15 { pc.wrapping_add(12) } else { self.regs[rd] };
                if byte { bus.write_byte(addr, value as u8)?; }
                else { bus.write_word(addr, value)?; }
            }
            if writeback && !(load && rd == rn) { self.regs[rn] = adjusted; }
            return Ok(next);
        } else if word & 0x0C00_0000 == 0 {
            let immediate = word & (1 << 25) != 0;
            let register_shift = !immediate && word & 0x10 != 0;
            let rotation = ((word >> 8) & 15) * 2;
            let op = (word >> 21) & 15;
            let set = word & (1 << 20) != 0;
            let test = (8..=11).contains(&op);
            if (!test && rd == 15 && set && bank == 0) || (test && !set) || ((op == 13 || op == 15) && rn != 0) {
                return Err(Fault::Unsupported(word));
            }
            let (b, carry) = if immediate {
                let value = (word & 255).rotate_right(rotation);
                (value, if rotation == 0 { self.flags.c } else { value >> 31 != 0 })
            } else {
                let rs = ((word >> 8) & 15) as usize;
                // bit 7 与 bit 4 同时为 1 属于其他编码空间。
                if register_shift && (word & 0x80 != 0 || rs == 15) {
                    return Err(Fault::Unsupported(word));
                }
                if register_shift { bus.idle(1)?; }
                let value = if rm == 15 { pc.wrapping_add(if register_shift { 12 } else { 8 }) }
                    else { self.regs[rm] };
                let amount = if register_shift { self.regs[rs] & 255 } else { (word >> 7) & 31 };
                self.shift(value, (word >> 5) & 3, amount, register_shift)
            };
            let a = if rn == 15 { pc.wrapping_add(if register_shift { 12 } else { 8 }) } else { self.regs[rn] };
            if test {
                if rd == 15 && bank != 0 { self.apply_cpsr(self.spsr[bank]); }
                else { self.data_op(op, a, b, carry, true); }
                return Ok(pc.wrapping_add(4));
            }
            let value = self.data_op(op, a, b, carry, set && rd != 15);
            if rd == 15 {
                if set { self.apply_cpsr(self.spsr[bank]); }
                return Ok(bus.jump(value & if self.thumb { !1 } else { !3 }));
            }
            self.regs[rd] = value;
        } else {
            return Err(Fault::Unsupported(word));
        }
        Ok(pc.wrapping_add(4))
    }

    fn step_thumb(&mut self, bus: &mut CpuBus<'_>, pc: u32, word: u16) -> Result<u32, Fault> {
        let rd = usize::from((word >> 8) & 7);
        let value = u32::from(word & 255);
        match word {
            _ if word & 0xFF87 == 0x4700 => {
                let source = usize::from((word >> 3) & 15);
                // Thumb 读取 PC 时看到的是当前指令地址加 4。
                let target = if source == 15 { pc.wrapping_add(4) } else { self.regs[source] };
                return Ok(bus.jump(self.exchange(target)));
            }
            0x0000..=0x17FF => {
                let source = self.regs[usize::from((word >> 3) & 7)];
                let (value, carry) = self.shift(source, u32::from(word >> 11), u32::from((word >> 6) & 31), false);
                self.regs[usize::from(word & 7)] = value;
                self.flags.nz(value);
                self.flags.c = carry;
            }
            0x1800..=0x1FFF => {
                let a = self.regs[usize::from((word >> 3) & 7)];
                let arg = usize::from((word >> 6) & 7);
                let b = if word & 0x0400 != 0 { arg as u32 } else { self.regs[arg] };
                self.regs[usize::from(word & 7)] = self.arithmetic(a, b, word & 0x0200 != 0, true);
            }
            0x2000..=0x27FF => { self.regs[rd] = value; self.flags.nz(value); }
            0x2800..=0x2FFF => { self.arithmetic(self.regs[rd], value, true, true); }
            0x3000..=0x3FFF => {
                self.regs[rd] = self.arithmetic(self.regs[rd], value, word & 0x0800 != 0, true);
            }
            0x4000..=0x43FF => {
                let rd = usize::from(word & 7);
                let a = self.regs[rd];
                let b = self.regs[usize::from((word >> 3) & 7)];
                let carry = self.flags.c;
                let op = (word >> 6) & 15;
                let result = match op {
                    0 => self.data_op(0, a, b, carry, true),
                    1 => self.data_op(1, a, b, carry, true),
                    2..=4 | 7 => {
                        bus.idle(1)?;
                        let kind = if op == 7 { 3 } else { u32::from(op - 2) };
                        let (value, carry) = self.shift(a, kind, b & 255, true);
                        self.flags.nz(value);
                        self.flags.c = carry;
                        value
                    }
                    5 | 6 => self.data_op(u32::from(op), a, b, carry, true),
                    8 => { self.flags.nz(a & b); return Ok(pc.wrapping_add(2)); }
                    9 => self.arithmetic(0, b, true, true),
                    10 | 11 => { self.arithmetic(a, b, op == 10, true); return Ok(pc.wrapping_add(2)); }
                    12 => self.data_op(12, a, b, carry, true),
                    13 => {
                        // Thumb MUL 把目标寄存器的旧值送入决定提前终止的乘数端。
                        let cycles = multiply_cycles(a, true);
                        bus.idle(cycles)?;
                        let value = a.wrapping_mul(b);
                        self.flags.nz(value);
                        self.flags.c = multiply_carry(b, a, 0, true, false, cycles);
                        value
                    }
                    14 => self.data_op(14, a, b, carry, true),
                    _ => self.data_op(15, a, b, carry, true),
                };
                self.regs[rd] = result;
            }
            0x4400..=0x46FF => {
                let rd = usize::from((word & 7) | ((word >> 4) & 8));
                let rs = usize::from((word >> 3) & 15);
                let read = |reg| if reg == 15 { pc.wrapping_add(4) } else { self.regs[reg] };
                let source = read(rs);
                let value = match (word >> 8) & 3 {
                    0 => read(rd).wrapping_add(source),
                    1 => {
                        self.arithmetic(read(rd), source, true, true);
                        return Ok(pc.wrapping_add(2));
                    }
                    _ => source,
                };
                if rd == 15 { return Ok(bus.jump(value & !1)); }
                self.regs[rd] = value;
            }
            0x4800..=0x4FFF => {
                self.regs[rd] = bus.read_word((pc.wrapping_add(4) & !3).wrapping_add(value * 4))?;
            }
            0x5000..=0x5FFF => {
                let dest = usize::from(word & 7);
                let base = self.regs[usize::from((word >> 3) & 7)];
                let offset = self.regs[usize::from((word >> 6) & 7)];
                let addr = base.wrapping_add(offset);
                match (word >> 9) & 7 {
                    0 => bus.write_word(addr, self.regs[dest])?,
                    1 => bus.write_half(addr, self.regs[dest] as u16)?,
                    2 => bus.write_byte(addr, self.regs[dest] as u8)?,
                    3 => self.regs[dest] = bus.read_byte(addr)? as i8 as i32 as u32,
                    4 => self.regs[dest] = bus.load_word(addr)?,
                    5 => self.regs[dest] = bus.load_half(addr)?,
                    6 => self.regs[dest] = u32::from(bus.read_byte(addr)?),
                    _ if addr & 1 != 0 => self.regs[dest] = bus.read_byte(addr)? as i8 as i32 as u32,
                    _ => self.regs[dest] = bus.read_half(addr)? as i16 as i32 as u32,
                }
            }
            0x6000..=0x8FFF => {
                let rd = usize::from(word & 7);
                let base = self.regs[usize::from((word >> 3) & 7)];
                let offset = u32::from((word >> 6) & 31);
                let size = match word >> 12 { 6 => 4, 7 => 1, _ => 2 };
                let addr = base.wrapping_add(offset * size);
                match (word & 0x0800 != 0, size) {
                    (true, 4) => self.regs[rd] = bus.load_word(addr)?,
                    (true, 2) => self.regs[rd] = bus.load_half(addr)?,
                    (true, _) => self.regs[rd] = u32::from(bus.read_byte(addr)?),
                    (false, 4) => bus.write_word(addr, self.regs[rd])?,
                    (false, 2) => bus.write_half(addr, self.regs[rd] as u16)?,
                    (false, _) => bus.write_byte(addr, self.regs[rd] as u8)?,
                }
            }
            0x9000..=0x9FFF => {
                let addr = self.regs[13].wrapping_add(value * 4);
                if word & 0x0800 != 0 {
                    self.regs[rd] = bus.load_word(addr)?;
                } else { bus.write_word(addr, self.regs[rd])?; }
            }
            0xA000..=0xAFFF => {
                let base = if word & 0x0800 != 0 { self.regs[13] } else { pc.wrapping_add(4) & !3 };
                self.regs[rd] = base.wrapping_add(value * 4);
            }
            0xB000..=0xB0FF => {
                let offset = (value & 127) * 4;
                self.regs[13] = if value & 128 != 0 { self.regs[13].wrapping_sub(offset) }
                    else { self.regs[13].wrapping_add(offset) };
            }
            _ if word & 0xF600 == 0xB400 => {
                let pop = word & 0x0800 != 0;
                let extra = word & 0x0100 != 0;
                let mask = word & 255;
                let count = mask.count_ones() + u32::from(extra);
                if count == 0 { return Err(Fault::Unsupported(u32::from(word))); }
                if pop && extra { bus.jump(0); }
                let sp = self.regs[13];
                let mut address = if pop { sp } else { sp.wrapping_sub(count * 4) };
                for reg in 0..8 {
                    if mask & (1 << reg) == 0 { continue; }
                    if pop { self.regs[reg] = bus.read_word(address)?; }
                    else { bus.write_word(address, self.regs[reg])?; }
                    address = address.wrapping_add(4);
                }
                let target = if extra && pop { Some(bus.read_word(address)? & !1) }
                    else {
                        if extra { bus.write_word(address, self.regs[14])?; }
                        None
                    };
                self.regs[13] = if pop { sp.wrapping_add(count * 4) } else { sp.wrapping_sub(count * 4) };
                if let Some(target) = target { return Ok(bus.jump(target)); }
            }
            0xC000..=0xCFFF => {
                let mask = word & 255;
                let load = word & 0x0800 != 0;
                let base = self.regs[rd];
                if mask == 0 {
                    if load { bus.jump(0); }
                    let target = if load { Some(bus.read_word(base)? & !1) }
                        else { bus.write_word(base, pc.wrapping_add(6))?; None };
                    self.regs[rd] = base.wrapping_add(0x40);
                    return Ok(target.map_or(pc.wrapping_add(2), |addr| bus.jump(addr)));
                }
                let end = base.wrapping_add(mask.count_ones() * 4);
                let mut addr = base;
                let first = mask.trailing_zeros() as usize;
                for reg in 0..8 {
                    if mask & (1 << reg) == 0 { continue; }
                    if load { self.regs[reg] = bus.read_word(addr)?; }
                    else {
                        let value = if reg == rd && reg != first { end } else { self.regs[reg] };
                        bus.write_word(addr, value)?;
                    }
                    addr = addr.wrapping_add(4);
                }
                if !load || mask & (1 << rd) == 0 { self.regs[rd] = end; }
            }
            0xF000..=0xF7FF => {
                let high = (((u32::from(word & 0x7FF) << 21) as i32) >> 9) as u32;
                self.regs[14] = pc.wrapping_add(4).wrapping_add(high);
            }
            0xF800..=0xFFFF => {
                let target = self.regs[14].wrapping_add(u32::from(word & 0x7FF) * 2);
                self.regs[14] = pc.wrapping_add(2) | 1;
                return Ok(bus.jump(target & !1));
            }
            0xD000..=0xDDFF => {
                if self.flags.allows(u32::from((word >> 8) & 15)) {
                    return Ok(bus.jump(pc.wrapping_add(4).wrapping_add(((value as i8 as i32) * 2) as u32)));
                }
            }
            0xDF00..=0xDFFF if bus.has_bios() => {
                return Ok(bus.jump(self.enter_exception(0x13, 8, pc.wrapping_add(2))));
            }
            0xDF00..=0xDFFF if bus.biosless_swi(word as u8)? => {}
            0xE000..=0xE7FF => {
                let offset = ((word << 5) as i16 >> 4) as i32 as u32;
                return Ok(bus.jump(pc.wrapping_add(4).wrapping_add(offset)));
            }
            _ => return Err(Fault::Unsupported(u32::from(word))),
        }
        Ok(pc.wrapping_add(2))
    }
}

fn multiply_cycles(multiplier: u32, signed: bool) -> u32 {
    for bytes in 1..=3 {
        let high = multiplier >> (8 * bytes);
        if high == 0 || (signed && high == (u32::MAX >> (8 * bytes))) {
            return bytes;
        }
    }
    4
}

// ARM7TDMI 的乘法 C 位来自 Booth 部分进位的移位输出。算法依据
// https://bmchtech.github.io/post/multiply/ ，此处只重建 C 位，数值结果由 Rust 计算。
fn multiply_carry(a: u32, b: u32, accumulator: u64, signed: bool, long: bool, cycles: u32) -> bool {
    let multiplicand = if signed { (i64::from(a as i32)) as u64 } else { u64::from(a) };
    let multiplier = if signed { (i64::from(b as i32)) as u64 } else { u64::from(b) };
    let mask = (1 << 33) - 1;
    let initial = if multiplier & 1 == 0 { 0 } else { !multiplicand };
    let mut sum = accumulator >> 1;
    let mut carry = initial >> 1;
    let mut upper_accumulator = accumulator >> 34;
    let mut shifted_carries = u128::from(initial & 1);
    for shift in (0..8 * cycles).step_by(2) {
        let digit = (multiplier >> shift) & 7;
        let negative = (4..=6).contains(&digit);
        let magnitude = match digit {
            0 | 7 => 0,
            3 | 4 => multiplicand << 1,
            _ => multiplicand,
        };
        let addend = if negative { !magnitude } else { magnitude };
        let x = sum & mask;
        let y = carry & mask;
        let z = addend & mask;
        let next_carry = (((x & y) | ((x | y) & z)) << 1) | u64::from(negative);
        shifted_carries |= u128::from(next_carry & 3) << (1 + shift);
        let sign_correction = (upper_accumulator & 1) + (1 ^ (y >> 32)) + (1 ^ (addend >> 33 & 1));
        sum = ((x ^ y ^ z) >> 2) | (sign_correction << 31);
        carry = (next_carry >> 2) | ((1 ^ (upper_accumulator >> 1 & 1)) << 32);
        upper_accumulator >>= 2;
    }
    shifted_carries |= u128::from(carry) << (1 + 8 * cycles);
    let flag_bit = if cycles < 4 { 23 } else if long { 63 } else { 31 };
    shifted_carries & (1 << flag_bit) != 0
}
