//! GameBoy 模拟器核心库（CPU + PPU + APU + 总线/MBC/SRAM）。
//! 从 src/main.rs 拆出，供桌面窗口（cargo run）与网页（wasm）共用一份逻辑。

// GameBoy 模拟器 —— 里程碑③④：Blargg 考卷 + PPU 显像
// Sharp LR35902 CPU（全指令集+中断+定时器） + PPU（背景图渲染）。
// 心智模型不变：取指 → 译码 → 执行，外加按周期推进的硬件外设。
//
// 已完成：
//   · CPU 全指令（含 CB 前缀），Blargg 考卷 75/75 ok
//   · 栈/调用/条件跳转、中断、DIV/TIMA 定时器
//   · PPU：按扫描线渲染背景，VBlank 中断
// 运行：cargo run --release -- roms/cpu_instrs.gb [--screenshot out.ppm]

pub mod ppu;
pub mod apu;
pub use ppu::Ppu;
pub use apu::Apu;

// wasm 导出层：只在 wasm32 目标编译，桌面构建完全忽略
#[cfg(target_arch = "wasm32")]
pub mod wasm;

// ---------- 标志位（F 寄存器高 4 位） ----------
const FLAG_Z: u8 = 0x80;   // 结果为 0
const FLAG_N: u8 = 0x40;   // 刚才是减法
const FLAG_H: u8 = 0x20;   // 低半字节进位
const FLAG_C: u8 = 0x10;   // 进位/借位（你的 VF，但 SUB 时方向相反）

// ---------- 寄存器组 ----------
pub struct Registers {
    pub a: u8, pub f: u8,
    pub b: u8, pub c: u8,
    pub d: u8, pub e: u8,
    pub h: u8, pub l: u8,
}

impl Registers {
    pub fn new() -> Self {
        // 真机 Boot ROM 留下的开机初值
        Registers { a: 0x01, f: 0xB0, b: 0x00, c: 0x13, d: 0x00, e: 0xD8, h: 0x01, l: 0x4D }
    }
    pub fn bc(&self) -> u16 { ((self.b as u16) << 8) | self.c as u16 }
    pub fn de(&self) -> u16 { ((self.d as u16) << 8) | self.e as u16 }
    pub fn hl(&self) -> u16 { ((self.h as u16) << 8) | self.l as u16 }
    pub fn af(&self) -> u16 { ((self.a as u16) << 8) | self.f as u16 }
    pub fn set_bc(&mut self, v: u16) { self.b = (v >> 8) as u8; self.c = v as u8; }
    pub fn set_de(&mut self, v: u16) { self.d = (v >> 8) as u8; self.e = v as u8; }
    pub fn set_hl(&mut self, v: u16) { self.h = (v >> 8) as u8; self.l = v as u8; }
    pub fn set_af(&mut self, v: u16) { self.a = (v >> 8) as u8; self.f = (v as u8) & 0xF0; } // 低4位恒0
    pub fn flag(&self, mask: u8) -> bool { self.f & mask != 0 }
    pub fn set_flag(&mut self, mask: u8, on: bool) {
        if on { self.f |= mask; } else { self.f &= !mask; }
    }
    // 按编号读寄存器（6 = 通过 HL 指针访存，需要总线，单独处理）
    pub fn get(&self, i: u8) -> u8 {
        match i { 0=>self.b, 1=>self.c, 2=>self.d, 3=>self.e, 4=>self.h, 5=>self.l, 7=>self.a, _=> unreachable!() }
    }
    pub fn set(&mut self, i: u8, v: u8) {
        match i { 0=>self.b=v, 1=>self.c=v, 2=>self.d=v, 3=>self.e=v, 4=>self.h=v, 5=>self.l=v, 7=>self.a=v, _=> unreachable!() }
    }
}

// 调试：设 GB_TRACE=1 时打印 PPU 寄存器写入时刻（LY/dot/PC），用于和 Mealybug 标准答案对时序
thread_local! {
    static TRACE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static TRACE_PC: std::cell::Cell<u16> = const { std::cell::Cell::new(0) };
    static DBG_LINE: std::cell::Cell<u8> = const { std::cell::Cell::new(255) };
}
pub fn set_trace(on: bool) { TRACE.with(|t| t.set(on)); }
pub fn set_dbg_line(line: u8) { DBG_LINE.with(|l| l.set(line)); }

// ---------- 64KB 总线 ----------
// 真机上不同地址段通向不同硬件。本批接三样：
//   · 卡带 ROM（0x0000~0x7FFF）：经 MBC 芯片换页（bank 切进 0x4000~0x7FFF 窗口）
//   · SRAM 存档（0xA000~0xBFFF）：卡带自带可写内存，纽扣电池供电，得先「开锁」才读写
//   · 串口（0xFF01 数据 / 0xFF02 控制）：Blargg / Mooneye 考卷用它打印成绩单！
//     往 FF02 写 0x81 = 「把 FF01 里的字符发出去」，我们把它收进字符串。
pub struct Bus {
    pub mem: Vec<u8>,
    pub cart: Vec<u8>,        // 完整卡带 ROM（含 MBC 换页的全部 bank）
    pub ram: Vec<u8>,         // SRAM 存档（大小由卡带头 0x149 字节定）
    pub mbc_type: u8,         // 0x147 卡带类型字节
    pub ramg: bool,           // MBC1 存档「开锁」标志
    pub bank1: u8,            // 低 5 位 bank 号（写 0x2000~0x3FFF）
    pub bank2: u8,            // 高 2 位 bank 号（写 0x4000~0x5FFF）
    pub mode: bool,           // 模式位（写 0x6000~0x7FFF）：true=高级换页
    pub multicart: bool,      // 是否 multicart（特殊接线）
    pub rom_bank: u16,        // MBC3/MBC5 当前 ROM bank（MBC3:1-127, MBC5:0-511）
    pub ram_bank: u8,         // MBC3/MBC5 当前 RAM bank（MBC3:0-3 或 8-12=RTC 寄存器, MBC5:0-15）
    pub rtc_sec: u64,            // MBC3 RTC 累计秒数（二进制，走时核心；宿主注入墙钟推进）
    pub rtc_halted: bool,        // RTC 停止标志（寄存器 0x0C 的 bit6，置 1 停止走时）
    pub rtc_latched: [u8; 5],    // 锁存快照（写 0x6000 区 0→1 时从 rtc_sec 换算并冻结）
    pub rtc_latch_pre: bool,     // RTC 锁存序列：上一次写 0x6000 区是否为 0
    pub rtc_last_wall: Option<u64>, // 上次宿主注入的墙钟秒数（首帧只设基准、不算增量）
    pub serial_out: String,   // 考卷成绩单收集处
    pub div_acc: u32,         // DIV 分频累加器（每周期+1，满256归零并让DIV+1）
    pub tac: u8,              // 定时器控制（低3位有效）
    pub tima_acc: u32,        // TIMA 分频累加器
    pub joypad_dir: u8,       // 方向键状态（bit0~3 = 右左上下，0=按下）
    pub joypad_btn: u8,       // 功能键状态（bit0~3 = A B Select Start，0=按下）
    pub joy_reads: u64,       // 诊断：游戏读了多少次手柄
    pub joy_log: Vec<(u64, u8, u8)>,  // 诊断：(第几次读, 选择位, 返回值) 最近几次
    pub apu_writes: Vec<(u16, u8)>,   // 诊断：游戏写过哪些 APU 寄存器（值）
    pub apu: Apu,             // 发声芯片（最小闭环版）
    pub ppu: Ppu,             // 显像芯片：逐 dot 推进，由本总线的每次 CPU 访存驱动（见 mcycle）
    mcycles: u32,             // 本条指令已走过的 M-cycle 数（每次 CPU 读/写 +1，指令末尾按它补齐内部周期）
}
// 卡带头 0x149 字节 → SRAM 大小（字节）
fn ram_size_for(header: &[u8]) -> usize {
    match header.get(0x149).copied().unwrap_or(0) {
        0x01 => 2048, 0x02 => 8192, 0x03 => 32768, 0x04 => 131_072, 0x05 => 65536, _ => 0
    }
}
// 判断 8Mbit MBC1 multicart：4 个 0x40000 页里 ≥3 页在 0x0104 有相同的任天堂 logo
fn is_mbc1_multicart(cart: &[u8]) -> bool {
    if cart.len() != 0x100000 { return false; }
    let logo0 = &cart[0x104..0x134];
    let mut count = 0;
    for page in 0..4 {
        let start = page * 0x40000 + 0x104;
        if &cart[start..start + 0x30] == logo0 { count += 1; }
    }
    count >= 3
}

// MBC3 RTC：把累计秒数换算成 5 字节寄存器（BCD 秒/分/时/日低 8 位 + 日高 1 位）。
// 寄存器 0..=3 存 BCD（每字节两位十进制），寄存器 4 的 bit0=日的第 9 位。
// bit7(日高)=进位标志（日计数溢出 511→0 时置位）、bit6(日高)=停止标志，由 halted 参数回填。
fn rtc_pack(secs: u64, halted: bool) -> [u8; 5] {
    let days_total = secs / 86_400;              // 完整天数（未截断）
    let days = days_total & 0x1FF;               // 日计数：9 位，最大 511 天后回绕
    let tod = secs % 86_400;                     // 当天剩余秒数
    let sec = (tod % 60) as u8;
    let min = ((tod / 60) % 60) as u8;
    let hour = ((tod / 3600) % 24) as u8;
    let mut out = [0u8; 5];
    out[0] = (sec / 10) << 4 | (sec % 10);        // 秒 BCD
    out[1] = (min / 10) << 4 | (min % 10);        // 分 BCD
    out[2] = (hour / 10) << 4 | (hour % 10);      // 时 BCD（真机可 0-23）
    let day_lo = (days & 0xFF) as u8;
    out[3] = (day_lo / 100) << 4 | ((day_lo / 10) % 10); // 日低 8 位 BCD（硬件只存百位+十位，个位进日高）
    out[4] = ((days >> 8) & 1) as u8;             // 日高 bit0
    if days_total >= 512 { out[4] |= 0x80; }      // bit7 进位标志：日计数溢出过
    if halted { out[4] |= 0x40; }                 // bit6 停止标志
    out
}
impl Bus {
    pub fn new() -> Self {
        let mut b = Bus {
            mem: vec![0; 0x10000],
            cart: Vec::new(), ram: Vec::new(), mbc_type: 0,
            ramg: false, bank1: 1, bank2: 0, mode: false, multicart: false,
            rom_bank: 1, ram_bank: 0,
            rtc_sec: 0, rtc_halted: false, rtc_latched: [0; 5],
            rtc_latch_pre: false, rtc_last_wall: None,
            serial_out: String::new(), div_acc: 0, tac: 0, tima_acc: 0,
            joypad_dir: 0x0F, joypad_btn: 0x0F, joy_reads: 0, joy_log: Vec::new(),
            apu_writes: Vec::new(),
            apu: Apu::new(),
            ppu: Ppu::new(),
            mcycles: 0,
        };
        b.mem[0xFF44] = 0x00;   // LY：现在由 PPU 真正驱动，不再谎报
        b
    }
    // CPU 读：先推进一个 M-cycle（4 dot），再在周期末尾读到最新状态（LY/STAT 等）
    pub fn read(&mut self, addr: u16) -> u8 {
        self.mcycle();
        self.read_untimed(addr)
    }
    // 不耗周期的读（DMA 搬运用；DMA 本身另有 160 M-cycle，这里保持瞬时完成）
    fn read_untimed(&mut self, addr: u16) -> u8 {
        match addr {
            0x0000..=0x7FFF => self.read_rom(addr),
            0xA000..=0xBFFF => self.read_ram(addr),
            0xFF10..=0xFF3F => self.apu.read(addr),
            0xFF00 => {
                // Joypad 读：bit4/5 是选择线（0=选中），返回低4位按键状态（0=按下）
                let sel = self.mem[0xFF00];
                let mut result = 0x0F;   // 默认全没按
                if sel & 0x10 == 0 {
                    // P14(bit4)=0 选中方向键：右左上下 → bit0~3
                    result &= self.joypad_dir;
                }
                if sel & 0x20 == 0 {
                    // P15(bit5)=0 选中功能键：A B Select Start → bit0~3
                    result &= self.joypad_btn;
                }
                let r = (sel & 0x30) | result | 0xC0;   // 高2位恒1
                self.joy_reads += 1;
                if self.joy_log.len() < 40 { self.joy_log.push((self.joy_reads, sel, r)); }
                r
            }
            _ => self.mem[addr as usize],
        }
    }
    fn is_mbc3(&self) -> bool { matches!(self.mbc_type, 0x0F | 0x10 | 0x11 | 0x12 | 0x13) }
    fn is_mbc5(&self) -> bool { matches!(self.mbc_type, 0x19 | 0x1A | 0x1B | 0x1C | 0x1D | 0x1E) }
    // 换页核心：算「低 bank」和「高 bank」两个窗口各落在 ROM 的哪个偏移
    fn rom_offsets(&self) -> (usize, usize) {
        if self.is_mbc3() || self.is_mbc5() {
            // MBC3/MBC5：低窗口(0x0000-0x3FFF)永远 bank0，高窗口(0x4000-0x7FFF)由 rom_bank 决定
            (0, (self.rom_bank as usize) * 0x4000)
        } else {
            // MBC1
            let upper_bits = if self.multicart { (self.bank2 as usize) << 4 } else { (self.bank2 as usize) << 5 };
            let lower_bits = if self.multicart { (self.bank1 & 0x0F) as usize } else { self.bank1 as usize };
            let lower_bank = if self.mode { upper_bits } else { 0 };   // mode=1 时低窗口也能被 bank2 顶走
            let upper_bank = upper_bits | lower_bits;
            (0x4000 * lower_bank, 0x4000 * upper_bank)
        }
    }
    fn read_rom(&self, addr: u16) -> u8 {
        if self.cart.is_empty() { return 0xFF; }
        let (lo, hi) = self.rom_offsets();
        let base = if addr < 0x4000 { lo } else { hi };
        let off = base | (addr as usize & 0x3FFF);
        self.cart[off & (self.cart.len() - 1)]
    }
    fn ram_addr(&self, addr: u16) -> usize {
        let off = if self.is_mbc3() || self.is_mbc5() {
            (self.ram_bank as usize) * 0x2000                          // MBC3/MBC5：RAM bank 直接作偏移
        } else if self.mode {
            (self.bank2 as usize) * 0x2000                             // MBC1 高级模式
        } else { 0 };
        (off | (addr as usize & 0x1FFF)) & (self.ram.len() - 1)
    }
    fn read_ram(&self, addr: u16) -> u8 {
        if !self.ramg { return 0xFF; }   // 没开锁读到的就是垃圾
        if self.is_mbc3() && (0x08..=0x0C).contains(&self.ram_bank) {
            return self.rtc_latched[(self.ram_bank - 0x08) as usize];   // 读的是锁存快照，不是实时计数器
        }
        if self.ram.is_empty() { return 0xFF; }
        self.ram[self.ram_addr(addr)]
    }
    fn write_ram(&mut self, addr: u16, val: u8) {
        if !self.ramg { return; }
        if self.is_mbc3() && (0x08..=0x0C).contains(&self.ram_bank) {
            // 写 RTC 寄存器：真机只有日高寄存器(0x0C)可写，且 bit7 是进位标志（只读，写 0）
            // bit6 = 停止标志：写 1 停止走时、写 0 恢复并重置分频。其余寄存器写被忽略。
            if self.ram_bank == 0x0C {
                self.rtc_halted = val & 0x40 != 0;   // 停止标志
                // 注意：真机上「写 0 恢复」会把分频器清零，这里按秒级精度简化，不做分频
            }
            // 锁存快照同步更新到「当前」值：真机写 RTC 寄存器会让后续锁存读取到被修改的日高
            // 这里简化处理——写只影响停止标志，计数保持连续；锁存值由 tick 推进时实时换算。
            return;
        }
        if !self.ram.is_empty() {
            let idx = self.ram_addr(addr);
            self.ram[idx] = val;
        }
    }
    // CPU 写：写入 M-cycle 的第 1 个 dot 就生效（真机上寄存器在整个写周期都能看到新值），
    // 所以先写、再把这 4 个 dot 走完。
    pub fn write(&mut self, addr: u16, val: u8) {
        if TRACE.with(|t| t.get()) && matches!(addr, 0xFF40..=0xFF4B) {
            eprintln!("W {:04X}<={:02X} ly={} dot={} pc={:04X}", addr, val, self.ppu.ly, self.ppu.dots, TRACE_PC.with(|p| p.get()));
        }
        self.write_untimed(addr, val);
        self.mcycle();
    }
    fn write_untimed(&mut self, addr: u16, val: u8) {
        // 卡带区写入 = MBC 命令（不是真往 ROM 写，是给换页芯片下指令）
        let is_mbc1 = matches!(self.mbc_type, 0x01 | 0x02 | 0x03);
        let is_mbc3 = self.is_mbc3();
        let is_mbc5 = self.is_mbc5();
        match addr {
            0x0000..=0x1FFF => {
                if is_mbc1 || is_mbc3 || is_mbc5 { self.ramg = val & 0x0F == 0x0A; }   // RAM/RTC 使能：低4位=0xA 开锁
                return;
            }
            0x2000..=0x3FFF => {
                if is_mbc1 {
                    self.bank1 = val & 0x1F; if self.bank1 == 0 { self.bank1 = 1; }    // 低5位 bank，0 强制为 1
                } else if is_mbc3 {
                    self.rom_bank = (val & 0x7F) as u16; if self.rom_bank == 0 { self.rom_bank = 1; }  // 7位 bank，0 强制为 1
                } else if is_mbc5 {
                    // MBC5 9 位 bank：0x2000-0x2FFF 写低 8 位，0x3000-0x3FFF 写第 9 位
                    if addr < 0x3000 {
                        self.rom_bank = (self.rom_bank & 0x100) | (val as u16);
                    } else {
                        self.rom_bank = (self.rom_bank & 0x0FF) | (((val as u16) & 1) << 8);
                    }
                }
                return;
            }
            0x4000..=0x5FFF => {
                if is_mbc1 {
                    self.bank2 = val & 0x03;                                             // 高2位 bank
                } else if is_mbc3 || is_mbc5 {
                    self.ram_bank = val & 0x0F;                                          // MBC3:0-3=RAM,8-12=RTC / MBC5:0-15=RAM
                }
                return;
            }
            0x6000..=0x7FFF => {
                if is_mbc1 {
                    self.mode = val & 1 != 0;                                            // 模式位
                } else if is_mbc3 {
                    // RTC 锁存：写 0x00 再写 0x01，把当前走时快照进 rtc_latched（随后读到的就是这一刻）
                    if self.rtc_latch_pre && val == 0x01 { self.rtc_latched = rtc_pack(self.rtc_sec, self.rtc_halted); }
                    self.rtc_latch_pre = val == 0x00;
                }
                // MBC5 无模式位/锁存，写此区间忽略
                return;
            }
            0xA000..=0xBFFF => { self.write_ram(addr, val); return; }                     // SRAM 写（要开锁）
            0xFF10..=0xFF3F => { self.apu.write(addr, val); self.apu_writes.push((addr, val)); return; }  // 音频寄存器
            _ => {}
        }
        match addr {
            0xFF04 => { self.mem[0xFF04] = 0; self.div_acc = 0; return; }  // 写DIV=清零
            0xFF05 => { self.mem[0xFF05] = val; return; }                  // TIMA 直接写
            0xFF06 => { self.mem[0xFF06] = val; return; }                  // TMA
            0xFF07 => { self.tac = val & 7; self.mem[0xFF07] = val; return; }
            0xFF0F => { self.mem[0xFF0F] = val & 0x1F; return; }           // IF 低5位
            0xFF44 => { return; }                                          // LY 只读
            0xFF40 => { self.ppu.write_lcdc(val, &mut self.mem); return; }  // LCDC：写入 PPU 和可读寄存器
            0xFF41 => { self.ppu.write_stat(val, &mut self.mem); return; }  // STAT：DMG 写入延迟 1 dot，期间所有中断使能全开
            0xFF46 => {                                                    // DMA：把 XX00~XX9F 搬到 OAM
                let src = (val as u16) << 8;
                for i in 0..0xA0u16 {
                    let v = self.read_untimed(src + i);                      // 走 read_untimed 让 ROM 区也能正确取到卡带
                    self.mem[0xFE00 + i as usize] = v;
                }
                return;
            }
            _ => {}
        }
        self.mem[addr as usize] = val;
        if addr == 0xFF02 && val == 0x81 {
            // 串口发送：取出 FF01 的字符，记进成绩单，控制位归零
            let ch = self.mem[0xFF01] as char;
            self.serial_out.push(ch);
            self.mem[0xFF02] = 0;
        }
    }
    // 每走 t 个周期调一次：DIV/TIMA 按真实时间走（你 CHIP-8 批次④的教训！）
    // 指令内部已经通过访存推进过的 M-cycle 要扣掉，只补齐剩下的内部周期。
    pub fn tick(&mut self, t: u32) {
        let used = 4 * self.mcycles;
        self.mcycles = 0;
        self.advance(t.saturating_sub(used));
    }
    // 一个 M-cycle：PPU 走 4 个 dot + 定时器/音频走 4 周期
    fn mcycle(&mut self) {
        self.mcycles += 1;
        self.advance(4);
    }
    fn advance(&mut self, t: u32) {
        if t == 0 { return; }
        self.ppu.tick_dots(t, &mut self.mem);
        self.tick_periph(t);
    }
    fn tick_periph(&mut self, t: u32) {
        self.apu.tick(t);
        // DIV：每 256 周期 +1
        self.div_acc += t;
        while self.div_acc >= 256 {
            self.div_acc -= 256;
            self.mem[0xFF04] = self.mem[0xFF04].wrapping_add(1);
        }
        // TIMA：TAC 使能时按选定频率 +1，溢出回装 TMA 并置 IF 第2位
        if self.tac & 4 != 0 {
            let period: u32 = match self.tac & 3 { 0 => 1024, 1 => 16, 2 => 64, _ => 256 };
            self.tima_acc += t;
            while self.tima_acc >= period {
                self.tima_acc -= period;
                let (v, ov) = self.mem[0xFF05].overflowing_add(1);
                if ov {
                    self.mem[0xFF05] = self.mem[0xFF06];       // 溢出回装 TMA
                    self.mem[0xFF0F] |= 0x04;                  // 请求定时器中断
                } else {
                    self.mem[0xFF05] = v;
                }
            }
        }
    }
    pub fn read16(&mut self, addr: u16) -> u16 {
        (self.read(addr) as u16) | ((self.read(addr.wrapping_add(1)) as u16) << 8)  // 小端
    }
    pub fn write16(&mut self, addr: u16, val: u16) {
        self.write(addr, val as u8);
        self.write(addr.wrapping_add(1), (val >> 8) as u8);
    }
    // MBC3 RTC 走时：宿主每帧把「真实墙钟秒数」灌进来，内部算增量推进 rtc_sec。
    // 首帧只记基准（rtc_last_wall = Some(wall)），此后 wall 比上次大多少就推进多少秒；
    // 若停止标志置位，或 wall 回退（宿主时钟调整），则跳过推进、只更新基准。
    pub fn tick_rtc(&mut self, wall: u64) {
        if self.rtc_last_wall.is_none() { self.rtc_last_wall = Some(wall); return; }
        let last = self.rtc_last_wall.unwrap();
        self.rtc_last_wall = Some(wall);
        if !self.rtc_halted && wall > last { self.rtc_sec += wall - last; }
    }
    // 装卡带：完整 ROM 存进 cart，按头部字节初始化 MBC 状态与 SRAM 大小
    pub fn load_rom(&mut self, rom: &[u8]) {
        self.cart = rom.to_vec();
        self.mbc_type = rom.get(0x147).copied().unwrap_or(0);
        self.ram = vec![0; ram_size_for(rom)];
        self.ramg = false;
        self.bank1 = 1;
        self.bank2 = 0;
        self.mode = false;
        self.multicart = is_mbc1_multicart(&self.cart);
        self.rom_bank = 1;
        self.ram_bank = 0;
        self.rtc_sec = 0;
        self.rtc_halted = false;
        self.rtc_latched = [0; 5];
        self.rtc_latch_pre = false;
        self.rtc_last_wall = None;
        self.apply_boot_state();
    }
    // DMG boot ROM 的收尾副作用（我们不跑 boot ROM，但 Mealybug 标准答案在带 boot 的模拟器上生成，
    // 多张测试用 tile $19 = 「®」图块）：boot ROM 会把卡带头 $0104~$0133 的任天堂 logo 逐字节位加倍
    // 后写进 VRAM $8010 起（每字节 8 字节：同一行重复 4 次，高位平面为 0），再在 $8190 写「®」图块，
    // 并铺 logo 的地图案、设 BGP=$FC / LCDC=$91。
    fn apply_boot_state(&mut self) {
        for i in 0..48usize {
            let b = self.cart.get(0x104 + i).copied().unwrap_or(0);
            let mut d: u8 = 0;
            for bit in (0..8).rev() {
                let bit = (b >> bit) & 1;
                d = (d << 2) | (bit << 1) | bit;
            }
            let base = 0x8010 + i * 8;
            for off in [0, 2, 4, 6] {
                self.mem[base + off] = d;
            }
        }
        for (r, &v) in [0x3Cu8, 0x42, 0xB9, 0xA5, 0xB9, 0xA5, 0x42, 0x3C].iter().enumerate() {
            self.mem[0x8190 + r * 2] = v;
        }
        // 地图案：$9910=$19；$992F 下行到 $9924 写 $18..$0D；$980F 下行到 $9804 写 $0C..$01
        self.mem[0x9910] = 0x19;
        let mut a = 0x18u8;
        let mut addr = 0x992Fu16;
        while a >= 0x0D {
            self.mem[addr as usize] = a;
            addr -= 1;
            a -= 1;
        }
        a = 0x0C;
        addr = 0x980F;
        while a >= 0x01 {
            self.mem[addr as usize] = a;
            addr -= 1;
            a -= 1;
        }
        self.mem[0xFF47] = 0xFC;
        self.ppu.write_lcdc(0x91, &mut self.mem);
        self.mem[0xFF42] = 0;
        self.mem[0xFF43] = 0;
    }
    // 把 SRAM 存档落盘为 .sav 文件（测试后调用验证持久化）
    pub fn save_sram(&self, path: &str) -> std::io::Result<()> {
        std::fs::write(path, &self.ram)
    }
    pub fn load_sram(&mut self, path: &str) -> std::io::Result<()> {
        let data = std::fs::read(path)?;
        if data.len() == self.ram.len() { self.ram.copy_from_slice(&data); }
        Ok(())
    }
}

// ---------- CPU ----------
pub struct Cpu {
    pub reg: Registers,
    pub pc: u16,
    pub sp: u16,          // 栈指针：新抽屉！指向「下一个空位」
    pub halted: bool,
    pub ime: bool,        // 中断总开关（EI 的效果延迟到下一条指令之后才生效，见 ei_pending）
    ei_pending: u8,       // EI 延迟计数：EI 后第 2 条指令结束才打开 IME（真机行为）
    pub cycles: u64,
    pub unknown: Vec<(u8, u16)>,   // 未实现指令记录（去重），考完对答案用
    pub mooneye_result: Option<u8>,  // Mooneye 测试结果：执行 0x40(magic_breakpoint) 时 B=3 过 / 0x42 挂
}

impl Cpu {
    pub fn new() -> Self {
        Cpu { reg: Registers::new(), pc: 0x0100, sp: 0xFFFE, halted: false, ime: false, ei_pending: 0, cycles: 0, unknown: Vec::new(), mooneye_result: None }
    }

    fn fetch8(&mut self, bus: &mut Bus) -> u8 {
        TRACE_PC.with(|p| p.set(self.pc));
        let v = bus.read(self.pc);
        self.pc = self.pc.wrapping_add(1);
        v
    }
    fn fetch16(&mut self, bus: &mut Bus) -> u16 {
        let lo = self.fetch8(bus) as u16;
        let hi = self.fetch8(bus) as u16;
        (hi << 8) | lo
    }
    // 栈操作：先减 SP 再写（栈向低地址长）；弹栈先读再加
    fn push(&mut self, bus: &mut Bus, v: u16) {
        self.sp = self.sp.wrapping_sub(2);
        bus.write16(self.sp, v);
    }
    fn pop(&mut self, bus: &mut Bus) -> u16 {
        let v = bus.read16(self.sp);
        self.sp = self.sp.wrapping_add(2);
        v
    }

    // 按编号读「寄存器或(HL)」—— 矩阵译码的取数口
    fn get_r(&self, bus: &mut Bus, i: u8) -> u8 {
        if i == 6 { bus.read(self.reg.hl()) } else { self.reg.get(i) }
    }
    fn set_r(&mut self, bus: &mut Bus, i: u8, v: u8) {
        if i == 6 { bus.write(self.reg.hl(), v); } else { self.reg.set(i, v); }
    }

    // ---------- ALU 八族（统一入口，op 的高 3 位选族） ----------
    // 族号：0=ADD 1=ADC 2=SUB 3=SBC 4=AND 5=XOR 6=OR 7=CP
    fn alu(&mut self, kind: u8, v: u8) {
        let a = self.reg.a;
        let cin = if self.reg.flag(FLAG_C) { 1u16 } else { 0 };
        match kind {
            0 | 1 => {  // ADD / ADC（带进位加）
                let sum = a as u16 + v as u16 + if kind == 1 { cin } else { 0 };
                self.reg.set_flag(FLAG_H, (a & 0xF) + (v & 0xF) + (if kind==1 {cin as u8} else {0}) > 0xF);
                self.reg.a = sum as u8;
                self.reg.set_flag(FLAG_Z, self.reg.a == 0);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_C, sum > 0xFF);
            }
            2 | 3 => {  // SUB / SBC（带借位减）
                let sub = v as u16 + if kind == 3 { cin } else { 0 };
                let diff = a as i16 - sub as i16;
                self.reg.set_flag(FLAG_H, (a & 0xF) < (v & 0xF) + (if kind==3 {cin as u8} else {0}));
                self.reg.a = diff as u8;
                self.reg.set_flag(FLAG_Z, self.reg.a == 0);
                self.reg.set_flag(FLAG_N, true);
                self.reg.set_flag(FLAG_C, diff < 0);
            }
            4 => {  // AND：H=1 是它的怪癖，记住
                self.reg.a = a & v;
                self.reg.set_flag(FLAG_Z, self.reg.a == 0);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, true);
                self.reg.set_flag(FLAG_C, false);
            }
            5 => {  // XOR
                self.reg.a = a ^ v;
                self.reg.set_flag(FLAG_Z, self.reg.a == 0);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, false);
            }
            6 => {  // OR
                self.reg.a = a | v;
                self.reg.set_flag(FLAG_Z, self.reg.a == 0);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, false);
            }
            7 => {  // CP：就是 SUB，但不存结果！只留标志给条件跳转用
                let diff = a as i16 - v as i16;
                self.reg.set_flag(FLAG_H, (a & 0xF) < (v & 0xF));
                self.reg.set_flag(FLAG_Z, diff as u8 == 0);
                self.reg.set_flag(FLAG_N, true);
                self.reg.set_flag(FLAG_C, diff < 0);
            }
            _ => unreachable!(),
        }
    }

    fn inc8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_add(1);
        self.reg.set_flag(FLAG_Z, r == 0);
        self.reg.set_flag(FLAG_N, false);
        self.reg.set_flag(FLAG_H, (v & 0xF) + 1 > 0xF);
        r   // C 不动（考点）
    }
    fn dec8(&mut self, v: u8) -> u8 {
        let r = v.wrapping_sub(1);
        self.reg.set_flag(FLAG_Z, r == 0);
        self.reg.set_flag(FLAG_N, true);
        self.reg.set_flag(FLAG_H, (v & 0xF) == 0);
        r
    }

    // 条件判断：cc 编号 0=NZ 1=Z 2=NC 3=C
    fn cond(&self, cc: u8) -> bool {
        match cc {
            0 => !self.reg.flag(FLAG_Z),
            1 => self.reg.flag(FLAG_Z),
            2 => !self.reg.flag(FLAG_C),
            3 => self.reg.flag(FLAG_C),
            _ => unreachable!(),
        }
    }

    // ---------- CB 前缀：第二指令表（位操作/旋转） ----------
    fn step_cb(&mut self, bus: &mut Bus) -> u32 {
        let op = self.fetch8(bus);
        let i = op & 7;                 // 操作对象编号
        let v = self.get_r(bus, i);
        let kind = op >> 3;             // 0~7 旋转/移位族，8~15 BIT/RES/SET
        let cycles = if i == 6 { 16 } else { 8 };
        if kind < 8 {
            let cin = if self.reg.flag(FLAG_C) { 1u8 } else { 0 };
            let (r, cout) = match kind {
                0 => (v.rotate_left(1), v >> 7),              // RLC：左转，最高位进 C 也回最低位
                1 => (v.rotate_right(1), v & 1),              // RRC
                2 => ((v << 1) | cin, v >> 7),                // RL：穿过 C 左转（9位环）
                3 => ((v >> 1) | (cin << 7), v & 1),          // RR
                4 => (v << 1, v >> 7),                        // SLA：算术左移
                5 => ((v >> 1) | (v & 0x80), v & 1),          // SRA：算术右移，符号位保留
                6 => (v.rotate_left(4), 0),                   // SWAP：高低半字节对调（GameBoy 特产）
                7 => (v >> 1, v & 1),                         // SRL：逻辑右移
                _ => unreachable!(),
            };
            self.set_r(bus, i, r);
            self.reg.set_flag(FLAG_Z, r == 0);
            self.reg.set_flag(FLAG_N, false);
            self.reg.set_flag(FLAG_H, false);
            self.reg.set_flag(FLAG_C, cout != 0);
        } else {
            let bit = (kind - 8) & 7;     // 第几位
            match kind >> 3 {
                1 => {  // BIT：只测不改。Z=「这位是0吗」，C 不动
                    self.reg.set_flag(FLAG_Z, v & (1 << bit) == 0);
                    self.reg.set_flag(FLAG_N, false);
                    self.reg.set_flag(FLAG_H, true);
                    return if i == 6 { 12 } else { 8 };
                }
                2 => self.set_r(bus, i, v & !(1 << bit)),   // RES：清零这位
                3 => self.set_r(bus, i, v | (1 << bit)),    // SET：置一这位
                _ => unreachable!(),
            }
        }
        cycles
    }

    // ---------- 中断：每条指令后查一次 ----------
    // IE(FFFF) 和 IF(FF0F) 按位与：哪位都亮 = 哪个中断在敲门。
    // 受理：ime 关掉 → PC 压栈（你的 CALL！）→ 跳到固定向量地址。
    // 向量表：VBlank=0040 LCD=0048 Timer=0050 Serial=0058 Joypad=0060
    // 这里的查表不耗周期（真机在指令最后一个 M-cycle 里顺手查），派发本身 20 周期由调用方 tick。
    fn check_interrupts(&mut self, bus: &mut Bus) -> u32 {
        let pending = bus.mem[0xFFFF] & bus.mem[0xFF0F] & 0x1F;
        if pending == 0 { return 0; }
        if self.halted { self.halted = false; }   // 有中断敲门就醒（哪怕 ime=0）
        if !self.ime { return 0; }
        for bit in 0..5u8 {
            if pending & (1 << bit) != 0 {
                self.ime = false;                              // 进门随手关门
                bus.write(0xFF0F, bus.mem[0xFF0F] & !(1 << bit));   // 门铃消音
                let ret = self.pc;
                self.push(bus, ret);                           // 返程地址压栈
                self.pc = 0x0040 + (bit as u16) * 8;           // 跳向量
                return 20;
            }
        }
        0
    }

    // ---------- 主译码 ----------
    pub fn step(&mut self, bus: &mut Bus) -> u32 {
        // HALT：不取指，但时间照走、中断照查（考卷02的HALT题就考这个）
        if self.halted {
            bus.tick(4);
            self.cycles += 4;
            let t = self.check_interrupts(bus);
            bus.tick(t);
            self.cycles += t as u64;
            return 4 + t;
        }
        let op = self.fetch8(bus);
        // Mooneye 测试的 magic_breakpoint = 0x40 (LD B,B)。执行它时 B 已装好结果：
        // 3=通过，0x42=失败。抓到就记下，供验收测试判定。
        if op == 0x40 && self.mooneye_result.is_none() {
            self.mooneye_result = Some(self.reg.b);
        }
        let t = match op {
            0x00 => 4,                                       // NOP
            0x10 => { let _ = self.fetch8(bus); 4 }          // STOP：考卷用它在失败处刹车，先当2字节NOP
            0x76 => { self.halted = true; 4 }                // HALT
            0xF3 => { self.ime = false; self.ei_pending = 0; 4 }           // DI
            0xFB => { self.ei_pending = 2; 4 }                // EI：下一条指令结束后才真正打开

            // --- LD rr, nn（16位装载，31 是 SP）---
            0x01 => { let v = self.fetch16(bus); self.reg.set_bc(v); 12 }
            0x11 => { let v = self.fetch16(bus); self.reg.set_de(v); 12 }
            0x21 => { let v = self.fetch16(bus); self.reg.set_hl(v); 12 }
            0x31 => { self.sp = self.fetch16(bus); 12 }

            // --- 指针访存（BC/DE/HL±）---
            0x02 => { let a = self.reg.a; bus.write(self.reg.bc(), a); 8 }
            0x12 => { let a = self.reg.a; bus.write(self.reg.de(), a); 8 }
            0x0A => { self.reg.a = bus.read(self.reg.bc()); 8 }
            0x1A => { self.reg.a = bus.read(self.reg.de()); 8 }
            0x22 => { let hl = self.reg.hl(); let a = self.reg.a; bus.write(hl, a); self.reg.set_hl(hl.wrapping_add(1)); 8 } // (HL+)
            0x32 => { let hl = self.reg.hl(); let a = self.reg.a; bus.write(hl, a); self.reg.set_hl(hl.wrapping_sub(1)); 8 } // (HL-)
            0x2A => { let hl = self.reg.hl(); self.reg.a = bus.read(hl); self.reg.set_hl(hl.wrapping_add(1)); 8 }
            0x3A => { let hl = self.reg.hl(); self.reg.a = bus.read(hl); self.reg.set_hl(hl.wrapping_sub(1)); 8 }

            // --- ADD HL, rr（16位加：只动 N/H/C，Z 不动——和8位版不同！）---
            0x09|0x19|0x29|0x39 => {
                let rr = match (op >> 4) & 3 { 0=>self.reg.bc(), 1=>self.reg.de(), 2=>self.reg.hl(), _=>self.sp };
                let hl = self.reg.hl();
                let sum = hl as u32 + rr as u32;
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, (hl & 0xFFF) + (rr & 0xFFF) > 0xFFF);
                self.reg.set_flag(FLAG_C, sum > 0xFFFF);
                self.reg.set_hl(sum as u16); 8
            }

            // --- INC/DEC rr（16位自增减：一个标志都不动！）---
            0x03 => { let v = self.reg.bc().wrapping_add(1); self.reg.set_bc(v); 8 }
            0x13 => { let v = self.reg.de().wrapping_add(1); self.reg.set_de(v); 8 }
            0x23 => { let v = self.reg.hl().wrapping_add(1); self.reg.set_hl(v); 8 }
            0x33 => { self.sp = self.sp.wrapping_add(1); 8 }
            0x0B => { let v = self.reg.bc().wrapping_sub(1); self.reg.set_bc(v); 8 }
            0x1B => { let v = self.reg.de().wrapping_sub(1); self.reg.set_de(v); 8 }
            0x2B => { let v = self.reg.hl().wrapping_sub(1); self.reg.set_hl(v); 8 }
            0x3B => { self.sp = self.sp.wrapping_sub(1); 8 }

            // --- INC/DEC r（8位，C 不动）---
            0x04|0x0C|0x14|0x1C|0x24|0x2C|0x34|0x3C => {
                let i = (op >> 3) & 7;
                let v = self.get_r(bus, i);
                let r = self.inc8(v);
                self.set_r(bus, i, r);
                if i == 6 { 12 } else { 4 }
            }
            0x05|0x0D|0x15|0x1D|0x25|0x2D|0x35|0x3D => {
                let i = (op >> 3) & 7;
                let v = self.get_r(bus, i);
                let r = self.dec8(v);
                self.set_r(bus, i, r);
                if i == 6 { 12 } else { 4 }
            }

            // --- LD r, n（8位立即数装载，你的 6xnn）---
            0x06|0x0E|0x16|0x1E|0x26|0x2E|0x36|0x3E => {
                let i = (op >> 3) & 7;
                let v = self.fetch8(bus);
                self.set_r(bus, i, v);
                if i == 6 { 12 } else { 8 }
            }

            // --- 杂项标志/累加器操作 ---
            0x07 => {  // RLCA：A 左转，但 Z 恒 0（和 CB 版不同！）
                let a = self.reg.a; let c = a >> 7;
                self.reg.a = a.rotate_left(1);
                self.reg.set_flag(FLAG_Z, false);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, c != 0); 4
            }
            0x0F => {  // RRCA
                let a = self.reg.a; let c = a & 1;
                self.reg.a = a.rotate_right(1);
                self.reg.set_flag(FLAG_Z, false);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, c != 0); 4
            }
            0x17 => {  // RLA：穿 C 左转，Z 恒 0
                let a = self.reg.a; let cin = if self.reg.flag(FLAG_C) {1} else {0};
                self.reg.a = (a << 1) | cin;
                self.reg.set_flag(FLAG_Z, false);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, a >> 7 != 0); 4
            }
            0x1F => {  // RRA
                let a = self.reg.a; let cin = if self.reg.flag(FLAG_C) {1} else {0};
                self.reg.a = (a >> 1) | (cin << 7);
                self.reg.set_flag(FLAG_Z, false);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, a & 1 != 0); 4
            }
            0x27 => {  // DAA：把「十六进制加法结果」修成 BCD（十进制）
                let mut a = self.reg.a;
                let mut corr = 0u8;
                let mut cout = self.reg.flag(FLAG_C);
                if !self.reg.flag(FLAG_N) {
                    if self.reg.flag(FLAG_H) || (a & 0xF) > 9 { corr |= 0x06; }
                    if cout || a > 0x99 { corr |= 0x60; cout = true; }
                    a = a.wrapping_add(corr);
                } else {
                    if self.reg.flag(FLAG_H) { corr |= 0x06; }
                    if cout { corr |= 0x60; }
                    a = a.wrapping_sub(corr);
                }
                self.reg.a = a;
                self.reg.set_flag(FLAG_Z, a == 0);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, cout); 4
            }
            0x2F => {  // CPL：A 按位取反
                self.reg.a = !self.reg.a;
                self.reg.set_flag(FLAG_N, true);
                self.reg.set_flag(FLAG_H, true); 4
            }
            0x37 => {  // SCF：C 置 1
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, true); 4
            }
            0x3F => {  // CCF：C 翻转
                let c = self.reg.flag(FLAG_C);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, false);
                self.reg.set_flag(FLAG_C, !c); 4
            }

            // --- LD r, r' 全矩阵（0x40~0x7F，挖掉 0x76=HALT）---
            0x40..=0x7F => {
                let dst = (op >> 3) & 7;
                let src = op & 7;
                let v = self.get_r(bus, src);
                self.set_r(bus, dst, v);
                if dst == 6 || src == 6 { 8 } else { 4 }
            }

            // --- ALU A, r 八族矩阵（0x80~0xBF）---
            0x80..=0xBF => {
                let kind = (op >> 3) & 7;
                let v = self.get_r(bus, op & 7);
                self.alu(kind, v);
                if (op & 7) == 6 { 8 } else { 4 }
            }

            // --- POP / PUSH（栈：你的返程本，现在是通用数据栈）---
            0xC1 => { let v = self.pop(bus); self.reg.set_bc(v); 12 }
            0xD1 => { let v = self.pop(bus); self.reg.set_de(v); 12 }
            0xE1 => { let v = self.pop(bus); self.reg.set_hl(v); 12 }
            0xF1 => { let v = self.pop(bus); self.reg.set_af(v); 12 }  // F 低4位被屏蔽
            0xC5 => { let v = self.reg.bc(); self.push(bus, v); 16 }
            0xD5 => { let v = self.reg.de(); self.push(bus, v); 16 }
            0xE5 => { let v = self.reg.hl(); self.push(bus, v); 16 }
            0xF5 => { let v = self.reg.af(); self.push(bus, v); 16 }

            // --- 条件跳转/调用/返回（cc = (op>>3)&3）---
            0xC2|0xCA|0xD2|0xDA => {  // JP cc, nn
                let a = self.fetch16(bus);
                if self.cond((op >> 3) & 3) { self.pc = a; 16 } else { 12 }
            }
            // JP nn：16 周期（取指+2 字节+1 空转）。配合 PPU 在 dot 452 拉起 mode 2 STAT 中断、
            // CPU 在 dot 456（=新行 dot 0）派发，处理函数起点落在 dot 36，与标准答案一致。
            0xC3 => { self.pc = self.fetch16(bus); 16 }       // JP nn
            0xE9 => { self.pc = self.reg.hl(); 4 }            // JP (HL)：跳到指针处
            0xC4|0xCC|0xD4|0xDC => {  // CALL cc, nn
                let a = self.fetch16(bus);
                if self.cond((op >> 3) & 3) {
                    let ret = self.pc;
                    self.push(bus, ret);
                    self.pc = a; 24
                } else { 12 }
            }
            0xCD => {  // CALL nn：你的 2nnn！存返程地址再跳
                let a = self.fetch16(bus);
                let ret = self.pc;
                self.push(bus, ret);
                self.pc = a; 24
            }
            0xC0|0xC8|0xD0|0xD8 => {  // RET cc
                if self.cond((op >> 3) & 3) {
                    self.pc = self.pop(bus); 20
                } else { 8 }
            }
            0xC9 => { self.pc = self.pop(bus); 16 }           // RET：你的 00EE！
            0xD9 => { self.pc = self.pop(bus); self.ime = true; 16 }  // RETI
            0xC7|0xCF|0xD7|0xDF|0xE7|0xEF|0xF7|0xFF => {  // RST：一步跳到固定地址
                let ret = self.pc;
                self.push(bus, ret);
                self.pc = (op & 0x38) as u16; 16
            }

            // --- ALU A, n（立即数版八族）---
            0xC6|0xCE|0xD6|0xDE|0xE6|0xEE|0xF6|0xFE => {
                let kind = (op >> 3) & 7;
                let v = self.fetch8(bus);
                self.alu(kind, v); 8
            }

            // --- 相对跳转 JR（含条件版）---
            0x18 => { let off = self.fetch8(bus) as i8;
                self.pc = (self.pc as i32 + off as i32) as u16; 12 }
            0x20|0x28|0x30|0x38 => {
                let off = self.fetch8(bus) as i8;
                if self.cond((op >> 3) & 3) {
                    self.pc = (self.pc as i32 + off as i32) as u16; 12
                } else { 8 }
            }

            // --- 零页/绝对地址访存 ---
            0xE0 => { let n = self.fetch8(bus) as u16; let a = self.reg.a; bus.write(0xFF00 + n, a); 12 } // LDH (n),A
            0xF0 => { let n = self.fetch8(bus) as u16; self.reg.a = bus.read(0xFF00 + n); 12 }           // LDH A,(n)
            0xE2 => { let c = self.reg.c as u16; let a = self.reg.a; bus.write(0xFF00 + c, a); 8 }       // LD (C),A
            0xF2 => { let c = self.reg.c as u16; self.reg.a = bus.read(0xFF00 + c); 8 }                  // LD A,(C)
            0xEA => { let a16 = self.fetch16(bus); let a = self.reg.a; bus.write(a16, a); 16 }           // LD (nn),A
            0xFA => { let a16 = self.fetch16(bus); self.reg.a = bus.read(a16); 16 }                     // LD A,(nn)
            0x08 => { let a16 = self.fetch16(bus); let sp = self.sp; bus.write16(a16, sp); 20 }         // LD (nn),SP
            0xF9 => { self.sp = self.reg.hl(); 8 }                                                     // LD SP,HL

            // --- SP 相对运算（标志按低字节算，Z/N 恒 0——怪但照做）---
            0xE8 => {  // ADD SP, n（有符号立即数）
                let n = self.fetch8(bus) as i8 as i16 as u16;
                let sp = self.sp;
                let lo = sp as u8;
                self.reg.set_flag(FLAG_Z, false);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, (lo & 0xF) + (n as u8 & 0xF) > 0xF);
                self.reg.set_flag(FLAG_C, lo as u16 + (n & 0xFF) > 0xFF);
                self.sp = sp.wrapping_add(n); 16
            }
            0xF8 => {  // LD HL, SP+n
                let n = self.fetch8(bus) as i8 as i16 as u16;
                let sp = self.sp;
                let lo = sp as u8;
                self.reg.set_flag(FLAG_Z, false);
                self.reg.set_flag(FLAG_N, false);
                self.reg.set_flag(FLAG_H, (lo & 0xF) + (n as u8 & 0xF) > 0xF);
                self.reg.set_flag(FLAG_C, lo as u16 + (n & 0xFF) > 0xFF);
                self.reg.set_hl(sp.wrapping_add(n)); 12
            }

            0xCB => self.step_cb(bus),   // 第二指令表

            _ => {
                let at = self.pc.wrapping_sub(1);
                if !self.unknown.iter().any(|&(o, a)| o == op && a == at) {
                    self.unknown.push((op, at));
                }
                4
            }
        };
        self.cycles += t as u64;
        bus.tick(t);                          // 硬件时钟跟着指令走（扣除已走过的 M-cycle）
        if self.ei_pending > 0 { self.ei_pending -= 1; if self.ei_pending == 0 { self.ime = true; } }   // EI 的效果在下一指令末尾生效
        let it = self.check_interrupts(bus);  // 指令间隙查中断
        bus.tick(it);
        self.cycles += it as u64;
        t + it
    }
}

// ---------- 单元测试 ----------
#[cfg(test)]
mod tests {
    use super::*;

    fn run(program: &[u8], steps: usize) -> (Cpu, Bus) {
        let mut bus = Bus::new();
        let mut cpu = Cpu::new();
        // 无 MBC 卡带：程序写进 cart（前 32KB 现在走 ROM 读取，不再写 mem）
        bus.cart = vec![0; 0x8000];
        bus.cart[0x0100..0x0100 + program.len()].copy_from_slice(program);
        for _ in 0..steps { cpu.step(&mut bus); }
        (cpu, bus)
    }

    // ===== 批次1 回归（15组）=====
    #[test] fn ld_a_n() {
        let (cpu, _) = run(&[0x3E, 0x42], 1);
        assert_eq!(cpu.reg.a, 0x42); assert_eq!(cpu.pc, 0x0102);
    }
    #[test] fn add_basic() {
        let (cpu, _) = run(&[0x3E, 10, 0x06, 5, 0x80], 3);
        assert_eq!(cpu.reg.a, 15);
        assert!(!cpu.reg.flag(FLAG_Z)); assert!(!cpu.reg.flag(FLAG_C)); assert!(!cpu.reg.flag(FLAG_N));
    }
    #[test] fn add_carry_and_zero() {
        let (cpu, _) = run(&[0x3E, 0xFF, 0x06, 1, 0x80], 3);
        assert_eq!(cpu.reg.a, 0); assert!(cpu.reg.flag(FLAG_Z)); assert!(cpu.reg.flag(FLAG_C));
    }
    #[test] fn add_half_carry() {
        let (cpu, _) = run(&[0x3E, 0x0F, 0x06, 0x01, 0x80], 3);
        assert_eq!(cpu.reg.a, 0x10); assert!(cpu.reg.flag(FLAG_H)); assert!(!cpu.reg.flag(FLAG_C));
    }
    #[test] fn sub_borrow() {
        let (cpu, _) = run(&[0x3E, 5, 0x06, 10, 0x90], 3);
        assert_eq!(cpu.reg.a, 251); assert!(cpu.reg.flag(FLAG_C)); assert!(cpu.reg.flag(FLAG_N));
    }
    #[test] fn sub_zero() {
        let (cpu, _) = run(&[0x3E, 7, 0x06, 7, 0x90], 3);
        assert_eq!(cpu.reg.a, 0); assert!(cpu.reg.flag(FLAG_Z)); assert!(!cpu.reg.flag(FLAG_C));
    }
    #[test] fn inc_does_not_touch_carry() {
        let (cpu, _) = run(&[0x3E, 0xFF, 0x06, 1, 0x80, 0x3C], 4);
        assert_eq!(cpu.reg.a, 1); assert!(cpu.reg.flag(FLAG_C));
    }
    #[test] fn dec_to_zero() {
        let (cpu, _) = run(&[0x3E, 1, 0x3D], 2);
        assert_eq!(cpu.reg.a, 0); assert!(cpu.reg.flag(FLAG_Z)); assert!(cpu.reg.flag(FLAG_N));
    }
    #[test] fn ld_hl_and_store() {
        let (cpu, mut bus) = run(&[0x21, 0x00, 0xC0, 0x3E, 42, 0x77], 3);
        assert_eq!(cpu.reg.hl(), 0xC000); assert_eq!(bus.read(0xC000), 42);
    }
    #[test] fn load_from_hl() {
        // LD HL,C123 / LD (HL),99 / LD A,(HL)：写进去再读回来
        let (cpu, mut bus) = run(&[0x21, 0x23, 0xC1, 0x36, 99, 0x7E], 3);
        assert_eq!(bus.read(0xC123), 99);
        assert_eq!(cpu.reg.a, 99);
    }
    #[test] fn jp_absolute() {
        let (cpu, _) = run(&[0xC3, 0x34, 0x12], 1);
        assert_eq!(cpu.pc, 0x1234);
    }
    #[test] fn jr_forward_and_back() {
        let (cpu, _) = run(&[0x18, 0x02], 1); assert_eq!(cpu.pc, 0x0104);
        let (cpu, _) = run(&[0x18, 0xFE], 1); assert_eq!(cpu.pc, 0x0100);
    }
    #[test] fn jr_nz_conditional() {
        let (cpu, _) = run(&[0x3E, 1, 0x3D, 0x20, 0x05], 3); assert_eq!(cpu.pc, 0x0105);
        let (cpu, _) = run(&[0x3E, 2, 0x3D, 0x20, 0x05], 3); assert_eq!(cpu.pc, 0x010A);
    }
    #[test] fn loop_countdown() {
        let (cpu, _) = run(&[0x3E, 3, 0x3D, 0x20, 0xFD, 0x76], 20);
        assert_eq!(cpu.reg.a, 0); assert!(cpu.halted); assert_eq!(cpu.pc, 0x0106);
    }
    #[test] fn halt_stops_pc() {
        let (mut cpu, mut bus) = run(&[0x76, 0x3E, 0xFF], 1);
        let pc = cpu.pc;
        cpu.step(&mut bus);
        assert_eq!(cpu.pc, pc); assert_eq!(cpu.reg.a, 0x01);
    }

    // ===== 批次2 新测试 =====
    #[test] fn ld_matrix() {
        // LD B,42 / LD C,B / LD D,C / LD A,D —— 接力搬运
        let (cpu, _) = run(&[0x06, 42, 0x48, 0x51, 0x7A], 4);
        assert_eq!(cpu.reg.a, 42);
    }
    #[test] fn ld_via_hl_matrix() {
        // LD HL,C000 / LD (HL),77 用矩阵码 0x36 / LD A,(HL) 用 0x7E
        let (cpu, mut bus) = run(&[0x21, 0x00, 0xC0, 0x36, 77, 0x7E], 3);
        assert_eq!(bus.read(0xC000), 77); assert_eq!(cpu.reg.a, 77);
    }
    #[test] fn and_or_xor() {
        // A=0F AND 3C → 0C；OR 30 → 3C；XOR FF → C3
        let (cpu, _) = run(&[0x3E, 0x0F, 0xE6, 0x3C, 0xF6, 0x30, 0xEE, 0xFF], 4);
        assert_eq!(cpu.reg.a, 0xC3);
        assert!(!cpu.reg.flag(FLAG_C)); assert!(!cpu.reg.flag(FLAG_H));
    }
    #[test] fn and_sets_h() {
        let (cpu, _) = run(&[0x3E, 0xFF, 0xE6, 0x0F], 2);
        assert_eq!(cpu.reg.a, 0x0F); assert!(cpu.reg.flag(FLAG_H));  // AND 的怪癖
    }
    #[test] fn cp_keeps_a() {
        // CP：A=5 比 10 → 借位 C=1、N=1，但 A 必须还是 5
        let (cpu, _) = run(&[0x3E, 5, 0xFE, 10], 2);
        assert_eq!(cpu.reg.a, 5);
        assert!(cpu.reg.flag(FLAG_C)); assert!(cpu.reg.flag(FLAG_N)); assert!(!cpu.reg.flag(FLAG_Z));
    }
    #[test] fn adc_chain() {
        // 16位加法 0x00FF + 0x0001 用两条8位加法串起来：
        // A=FF ADD 01 → A=00 C=1；A=00 ADC 00 → A=01（吃进位）
        let (cpu, _) = run(&[0x3E, 0xFF, 0xC6, 0x01, 0x3E, 0x00, 0xCE, 0x00], 4);
        assert_eq!(cpu.reg.a, 1); assert!(!cpu.reg.flag(FLAG_C));
    }
    #[test] fn sbc_borrow() {
        // A=00 SUB 01 → FF C=1；A=01 SBC 00 → 00（再借一次）
        let (cpu, _) = run(&[0x3E, 0x00, 0xD6, 0x01, 0x3E, 0x01, 0xDE, 0x00], 4);
        assert_eq!(cpu.reg.a, 0); assert!(cpu.reg.flag(FLAG_Z));
    }
    #[test] fn push_pop_roundtrip() {
        let (cpu, _) = run(&[0x01, 0x34, 0x12, 0xC5, 0x01, 0x00, 0x00, 0xC1], 4);
        assert_eq!(cpu.reg.bc(), 0x1234);   // 推出去再弹回来，值不变
        assert_eq!(cpu.sp, 0xFFFE);          // SP 回到原位
    }
    #[test] fn pop_af_masks_f() {
        // 推 AF 再弹：F 低4位必须被抹成0
        let (mut cpu, mut bus) = run(&[0xF5], 1);   // PUSH AF（F=B0）
        bus.write(cpu.sp, 0xFF);                     // 偷偷把栈里的低字节改成 FF
        cpu.step(&mut bus);                          // POP AF
        assert_eq!(cpu.reg.f & 0x0F, 0);
    }
    #[test] fn call_ret() {
        // 主程序 CALL 0150，子程序 LD A,99 / RET
        let mut bus = Bus::new();
        let mut cpu = Cpu::new();
        bus.cart = vec![0; 0x8000];
        let prog = [0xCD, 0x50, 0x01, 0x76];
        bus.cart[0x0100..0x0104].copy_from_slice(&prog);
        bus.cart[0x0150] = 0x3E; bus.cart[0x0151] = 99; bus.cart[0x0152] = 0xC9;
        for _ in 0..10 { if cpu.halted { break; } cpu.step(&mut bus); }
        assert_eq!(cpu.reg.a, 99);
        assert!(cpu.halted);                 // 回到了主程序的 HALT
        assert_eq!(cpu.sp, 0xFFFE);          // 栈干净
    }
    #[test] fn nested_call() {
        // 两层嵌套：主→子A(0150)→子B(0160)，验证返程地址层层正确
        let mut bus = Bus::new();
        let mut cpu = Cpu::new();
        bus.cart = vec![0; 0x8000];
        bus.cart[0x0100..0x0104].copy_from_slice(&[0xCD, 0x50, 0x01, 0x76]);
        bus.cart[0x0150..0x0154].copy_from_slice(&[0xCD, 0x60, 0x01, 0xC9]);
        bus.cart[0x0160..0x0163].copy_from_slice(&[0x3E, 7, 0xC9]);
        for _ in 0..20 { if cpu.halted { break; } cpu.step(&mut bus); }
        assert_eq!(cpu.reg.a, 7); assert!(cpu.halted); assert_eq!(cpu.sp, 0xFFFE);
    }
    #[test] fn conditional_call_ret() {
        // A=1 DEC → Z=1：CALL NZ 不跳（A还是0），CALL Z 跳去把A改88再 RET Z 回来
        let mut bus = Bus::new();
        let mut cpu = Cpu::new();
        bus.cart = vec![0; 0x8000];
        let prog = [0x3E, 1, 0x3D, 0xC4, 0x60, 0x01, 0xCC, 0x60, 0x01, 0x76];
        bus.cart[0x0100..0x010A].copy_from_slice(&prog);
        bus.cart[0x0160..0x0163].copy_from_slice(&[0x3E, 88, 0xC8]);  // LD A,88 / RET Z
        for _ in 0..20 { if cpu.halted { break; } cpu.step(&mut bus); }
        assert_eq!(cpu.reg.a, 88); assert!(cpu.halted); assert_eq!(cpu.sp, 0xFFFE);
    }
    #[test] fn jp_hl() {
        let (cpu, _) = run(&[0x21, 0x00, 0x02, 0xE9], 2);
        assert_eq!(cpu.pc, 0x0200);
    }
    #[test] fn rst() {
        let (cpu, mut bus) = run(&[0xCF], 1);   // RST 08
        assert_eq!(cpu.pc, 0x0008);
        assert_eq!(bus.read16(cpu.sp), 0x0101);  // 返程地址压栈了
    }
    #[test] fn hl_increment_store() {
        // LD HL,C000 / LD (HL+),A 两次 → 写 C000、C001，HL 前进2
        let (cpu, mut bus) = run(&[0x21, 0x00, 0xC0, 0x3E, 5, 0x22, 0x3E, 6, 0x22], 5);
        assert_eq!(bus.read(0xC000), 5); assert_eq!(bus.read(0xC001), 6);
        assert_eq!(cpu.reg.hl(), 0xC002);
    }
    #[test] fn ldh_roundtrip() {
        let (cpu, mut bus) = run(&[0x3E, 0x77, 0xE0, 0x80, 0x3E, 0x00, 0xF0, 0x80], 4);
        assert_eq!(bus.read(0xFF80), 0x77); assert_eq!(cpu.reg.a, 0x77);
    }
    #[test] fn inc_rr_no_flags() {
        // 先造 Z=1（A=0），再 INC BC：Z 必须保持
        let (cpu, _) = run(&[0x3E, 0, 0x97, 0x01, 0xFF, 0xFF, 0x03], 4);
        // 0x97 = SUB A,A → A=0 Z=1；BC=FFFF INC → 0000 但 Z 不动
        assert_eq!(cpu.reg.bc(), 0);
        assert!(cpu.reg.flag(FLAG_Z));
    }
    #[test] fn add_hl_carry() {
        // HL=FFFF ADD HL,BC(0001) → 0000 C=1，Z 不动
        let (cpu, _) = run(&[0x21, 0xFF, 0xFF, 0x01, 0x01, 0x00, 0x09], 3);
        assert_eq!(cpu.reg.hl(), 0); assert!(cpu.reg.flag(FLAG_C)); assert!(!cpu.reg.flag(FLAG_N));
    }
    #[test] fn cb_swap() {
        let (cpu, _) = run(&[0x3E, 0xAB, 0xCB, 0x37], 2);
        assert_eq!(cpu.reg.a, 0xBA); assert!(!cpu.reg.flag(FLAG_C));
    }
    #[test] fn cb_rl_through_carry() {
        // SCF 置 C=1；A=80 RL → 01（最高位进C，旧C补最低位），C=1
        let (cpu, _) = run(&[0x37, 0x3E, 0x80, 0xCB, 0x17], 3);
        assert_eq!(cpu.reg.a, 0x01); assert!(cpu.reg.flag(FLAG_C));
    }
    #[test] fn cb_bit_set_res() {
        // A=00：BIT 0 → Z=1；SET 0 → A=01；RES 0 → A=00
        let (cpu, _) = run(&[0x3E, 0x00, 0xCB, 0x47, 0xCB, 0xC7, 0xCB, 0x87], 4);
        assert_eq!(cpu.reg.a, 0x00);
    }
    #[test] fn cb_bit_z_flag() {
        let (cpu, _) = run(&[0x3E, 0x00, 0xCB, 0x47], 2);  // BIT 0,A：位是0→Z=1
        assert!(cpu.reg.flag(FLAG_Z)); assert!(cpu.reg.flag(FLAG_H));
        let (cpu, _) = run(&[0x3E, 0x01, 0xCB, 0x47], 2);  // 位是1→Z=0
        assert!(!cpu.reg.flag(FLAG_Z));
    }
    #[test] fn daa_after_bcd_add() {
        // BCD 15 + 27：十六进制加法得 3C，DAA 修成 42
        let (cpu, _) = run(&[0x3E, 0x15, 0x06, 0x27, 0x80, 0x27], 4);
        assert_eq!(cpu.reg.a, 0x42);
    }
    #[test] fn cpl_scf_ccf() {
        let (cpu, _) = run(&[0x3E, 0x0F, 0x2F, 0x37, 0x3F], 4);
        assert_eq!(cpu.reg.a, 0xF0);          // 取反
        assert!(!cpu.reg.flag(FLAG_C));        // SCF置1再CCF翻回0
    }
    #[test] fn add_sp_signed() {
        // SP=FFFE ADD SP,-2 → FFFC
        let (cpu, _) = run(&[0xE8, 0xFE], 1);
        assert_eq!(cpu.sp, 0xFFFC);
        assert!(!cpu.reg.flag(FLAG_Z)); assert!(!cpu.reg.flag(FLAG_N));
    }
    #[test] fn ld_hl_sp_plus_n() {
        let (cpu, _) = run(&[0xF8, 0x02], 1);
        assert_eq!(cpu.reg.hl(), 0x0000);   // FFFE + 2 = 10000 绕回 0000
        assert!(cpu.reg.flag(FLAG_C));       // 低字节 FE+02 进位
    }
    #[test] fn ld_nn_sp() {
        let (_cpu, mut bus) = run(&[0x08, 0x00, 0xC0], 1);
        assert_eq!(bus.read16(0xC000), 0xFFFE);
    }

    // ===== MBC1 + SRAM 测试 =====
    // 造 4 个 16KB bank 的卡带，每个 bank 填不同字节，方便验证换页
    fn make_cart_4bank() -> Vec<u8> {
        let mut cart = vec![0u8; 0x10000];   // 64KB = 4 bank
        for (i, chunk) in cart.chunks_mut(0x4000).enumerate() {
            let fill = [0xAA, 0xBB, 0xCC, 0xDD][i];
            for byte in chunk.iter_mut() { *byte = fill; }
        }
        cart
    }

    #[test] fn mbc1_bank_switch() {
        let mut bus = Bus::new();
        bus.cart = make_cart_4bank();
        bus.mbc_type = 0x01;   // MBC1 无 RAM
        bus.bank1 = 1;
        assert_eq!(bus.read(0x4000), 0xBB);   // 初始 bank1=1 → 第 1 块
        bus.write(0x2000, 2);                  // 切 bank2
        assert_eq!(bus.read(0x4000), 0xCC);
        bus.write(0x2000, 3);                  // 切 bank3
        assert_eq!(bus.read(0x4000), 0xDD);
        bus.write(0x2000, 0);                  // 写 0 强制回 1
        assert_eq!(bus.read(0x4000), 0xBB);
        assert_eq!(bus.read(0x0000), 0xAA);   // 低窗口始终 bank0（mode=0）
    }

    #[test] fn mbc1_ram_enable_and_sram() {
        let mut bus = Bus::new();
        bus.cart = make_cart_4bank();
        bus.mbc_type = 0x03;   // MBC1+RAM+Battery
        bus.ram = vec![0; 8192];
        // 未开锁：读 0xA000 得 0xFF，写无效
        assert_eq!(bus.read(0xA000), 0xFF);
        bus.write(0xA000, 0x42);
        assert_eq!(bus.ram[0], 0);
        // 开锁：写 0x0000=0x0A
        bus.write(0x0000, 0x0A);
        bus.write(0xA000, 0x42);
        assert_eq!(bus.read(0xA000), 0x42);
        assert_eq!(bus.ram[0], 0x42);
        // 关锁：写 0x0000=0x00
        bus.write(0x0000, 0x00);
        assert_eq!(bus.read(0xA000), 0xFF);
    }

    // 跑一个 Mooneye ROM 直到触发 magic_breakpoint(0x40)，返回 B 寄存器结果
    // 成功=Some(3)，失败=Some(0x42)，卡死=None
    fn run_mooneye(rom_path: &str) -> Option<u8> {
        let rom = std::fs::read(rom_path).expect("读不到 Mooneye ROM");
        let mut bus = Bus::new();
        bus.load_rom(&rom);
        let mut cpu = Cpu::new();
        for _ in 0..50_000_000u64 {
            let _ = cpu.step(&mut bus);   // PPU 已内嵌在 Bus 里，随 CPU 访存逐 M-cycle 推进
            if cpu.mooneye_result.is_some() { break; }
        }
        cpu.mooneye_result
    }

    #[test] fn mooneye_mbc1_all() {
        let roms = [
            "bits_bank1.gb", "bits_bank2.gb", "bits_mode.gb", "bits_ramg.gb",
            "ram_64kb.gb", "ram_256kb.gb", "rom_512kb.gb", "rom_1Mb.gb",
            "rom_2Mb.gb", "rom_4Mb.gb", "rom_8Mb.gb", "rom_16Mb.gb",
            "multicart_rom_8Mb.gb",
        ];
        for name in roms {
            let result = run_mooneye(&format!("roms_mbc/{name}"));
            assert_eq!(result, Some(3), "Mooneye {name} 失败：result={result:?}");
        }
    }

    #[test] fn mooneye_mbc5_all() {
        let roms = [
            "rom_512kb.gb", "rom_1Mb.gb", "rom_2Mb.gb", "rom_4Mb.gb",
            "rom_8Mb.gb", "rom_16Mb.gb", "rom_32Mb.gb", "rom_64Mb.gb",
        ];
        for name in roms {
            let result = run_mooneye(&format!("roms_mbc5/{name}"));
            assert_eq!(result, Some(3), "Mooneye {name} 失败：result={result:?}");
        }
    }

    // MBC5 单元测试：9 位 ROM bank（0x2000-0x2FFF 低8位 / 0x3000-0x3FFF 第9位）+ RAM bank 换页
    #[test] fn mbc5_bank_switch() {
        let mut bus = Bus::new();
        bus.cart = vec![0xAA; 0x4000 * 300];   // 300 个 bank，超 256 验证第 9 位
        bus.mbc_type = 0x19;                    // MBC5 无 RAM
        bus.ram = vec![0; 8192];
        assert_eq!(bus.read(0x0000), 0xAA);
        bus.write(0x2000, 0x42);                // 低 8 位切到 bank 0x42
        assert_eq!(bus.read(0x4000), 0xAA);     // 内容相同无法区分，改验 rom_bank
        assert_eq!(bus.rom_bank, 0x42);
        bus.write(0x3000, 0x01);                // 第 9 位置 1 → bank 0x142
        assert_eq!(bus.rom_bank, 0x142);
        // 高低位分离写不互相覆盖
        bus.write(0x2000, 0xFF);
        assert_eq!(bus.rom_bank, 0x1FF);
        bus.write(0x3000, 0x00);
        assert_eq!(bus.rom_bank, 0x0FF);
    }

    #[test] fn mbc5_ram_bank_switch() {
        let mut bus = Bus::new();
        bus.cart = vec![0; 0x8000];
        bus.mbc_type = 0x1A;                     // MBC5+RAM
        bus.ram = vec![0; 0x8000];               // 4 个 8KB RAM bank
        bus.write(0x0000, 0x0A);                 // 开锁
        bus.write(0x4000, 0x02);                 // 选 RAM bank 2
        assert_eq!(bus.ram_bank, 2);
        bus.write(0xA000, 0x7E);
        assert_eq!(bus.ram[0x4000], 0x7E);       // 落在第 2 个 8KB bank 首字节
        assert_eq!(bus.read(0xA000), 0x7E);
    }

    // MBC3 单元测试：7 位 ROM bank + RTC 寄存器映射（无真 RTC 走时，只验换页与映射）
    #[test] fn mbc3_bank_switch() {
        let mut bus = Bus::new();
        bus.cart = vec![0xAA; 0x4000 * 128];
        bus.mbc_type = 0x13;                     // MBC3+RAM+Battery
        bus.ram = vec![0; 8192];
        bus.write(0x2000, 0x7F);                 // 最大 7 位 bank
        assert_eq!(bus.rom_bank, 0x7F);
        bus.write(0x2000, 0x80);                 // 0x80 & 0x7F = 0，同一次写内强制回 1
        assert_eq!(bus.rom_bank, 1);
        bus.write(0x2000, 0x00);                 // 写 0 也强制回 1
        assert_eq!(bus.rom_bank, 1);
    }

    #[test] fn mbc3_rtc_register_mapping() {
        let mut bus = Bus::new();
        bus.cart = vec![0; 0x8000];
        bus.mbc_type = 0x0F;                     // MBC3+TIMER+BATTERY
        bus.ram = vec![0; 8192];
        bus.write(0x0000, 0x0A);                 // 开锁
        bus.write(0x4000, 0x08);                 // 选 RTC 寄存器 0（秒）
        assert_eq!(bus.ram_bank, 0x08);
        bus.write(0xA000, 0x2A);                 // 写 RTC 秒寄存器
        assert_eq!(bus.rtc_sec, 0, "写秒寄存器不该改二进制计数（真机 RTC 写对计数不生效，只影响停止标志）");
        assert_eq!(bus.read(0xA000), 0x00);      // 未锁存，快照全 0
        bus.write(0x4000, 0x0C);                 // 选 RTC 寄存器 4（日高 8 位）
        bus.write(0xA000, 0x40);                 // 写 bit6 = 停止标志
        assert_eq!(bus.rtc_halted, true);
    }

    #[test] fn mbc3_rtc_wall_clock() {
        let mut bus = Bus::new();
        bus.cart = vec![0; 0x8000];
        bus.mbc_type = 0x0F;
        bus.ram = vec![0; 8192];
        bus.write(0x0000, 0x0A);
        bus.tick_rtc(1000);                      // 首帧只记基准
        assert_eq!(bus.rtc_sec, 0);
        bus.tick_rtc(1003);                      // 推进 3 秒
        assert_eq!(bus.rtc_sec, 3);
        // 锁存：写 0x00 → 0x01，把当前 3 秒快照进 rtc_latched
        bus.write(0x6000, 0x00);
        bus.write(0x6000, 0x01);
        assert_eq!(bus.rtc_latched[0], 0x03, "3 秒的 BCD 应为 0x03");
        bus.write(0x4000, 0x08);                 // 读 RTC 秒寄存器
        assert_eq!(bus.read(0xA000), 0x03);
        // 停止标志：写日高寄存器 bit6 = 1，走时冻结
        bus.write(0x4000, 0x0C);
        bus.write(0xA000, 0x40);
        bus.tick_rtc(1009);                      // 停止期间不推进
        assert_eq!(bus.rtc_sec, 3);
        // 恢复走时：写 bit6 = 0
        bus.write(0xA000, 0x00);
        bus.tick_rtc(1010);
        assert_eq!(bus.rtc_sec, 4);
    }

    #[test] fn mbc3_rtc_pack_carries() {
        // 直接验 rtc_pack 的进位：3670 秒 = 1时1分10秒
        let p = rtc_pack(3670, false);
        assert_eq!(p[0], 0x10);                  // 10 秒
        assert_eq!(p[1], 0x01);                  // 1 分
        assert_eq!(p[2], 0x01);                  // 1 时
        assert_eq!(p[3], 0x00);                  // 0 日
        assert_eq!(p[4], 0x00);
        // 512 天 = 日计数 9 位溢出回绕到 0，置进位标志 bit7（日高 bit0 = 0）
        let p2 = rtc_pack(512 * 86_400 + 59 + 59 * 60 + 23 * 3600, false);
        assert_eq!(p2[0], 0x59);                 // 59 秒
        assert_eq!(p2[1], 0x59);                 // 59 分
        assert_eq!(p2[2], 0x23);                 // 23 时
        assert_eq!(p2[3], 0x00);                 // 512 & 0xFF = 0
        assert_eq!(p2[4], 0x80);                 // bit0=0（日 9 位回绕），bit7=进位标志
    }

    // ===== APU 接线验证：CPU 执行 → 总线 → APU 是否连通 =====
    #[test]
    fn apu_wired_through_bus() {
        // 程序：LD A,0x80 / LDH(0x26),A  —— 写 NR52 电源开
        let (cpu, bus) = run(&[0x3E, 0x80, 0xE0, 0x26], 2);
        assert_eq!(cpu.reg.a, 0x80);
        assert_eq!(bus.apu_writes.len(), 1, "总线应记录 1 次 APU 写");
        assert_eq!(bus.apu_writes[0], (0xFF26, 0x80), "写的是 NR52=0x80");
    }
}