// APU —— GameBoy 的发声芯片（最小闭环版）
// 4 通道：方波1、方波2、波形、噪声，混成一路输出。
// 本批范围：频率/占空比/音量包络/长度计数器/混音；跳过方波1的扫频(sweep，下批补)。
//
// 心智模型：和 CPU/PPU 一样，每走 t 个周期 tick 一次。帧序列器(512Hz)给
// 音量包络、长度计数器打拍子；每通道各自产生数字电平(0~15)，最后混音输出。
// 输出按 44100Hz 下采样存进 samples，外壳拿去写 WAV 或喂 Web Audio。

pub const SAMPLE_RATE: u32 = 44100;
const CPU_HZ: u32 = 4_194_304;
const SAMPLE_EVERY: u32 = CPU_HZ / SAMPLE_RATE;   // ≈95 个周期出一个样本

// 把 samples 写成 16 位单声道 PCM WAV 文件（零依赖，方便肉耳验证）
pub fn write_wav(samples: &[i16], path: &str) -> std::io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());          // PCM
    out.extend_from_slice(&1u16.to_le_bytes());          // 单声道
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());  // 字节率
    out.extend_from_slice(&2u16.to_le_bytes());          // 块对齐
    out.extend_from_slice(&16u16.to_le_bytes());         // 位深
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for &s in samples { out.extend_from_slice(&s.to_le_bytes()); }
    std::fs::write(path, out)
}

// 4 档占空比：8 步波形 pattern，1=高电平（bit 从右往左是 step 0→7）
const DUTY: [u8; 4] = [0b0000_0001, 0b0000_0011, 0b0000_1111, 0b1111_1100];

// ---------- 方波通道（ch1/ch2 共用） ----------
struct Square {
    enabled: bool,
    dac_on: bool,      // 音量寄存器 bit3-7 非 0 时 DAC 才开
    duty: u8,          // 0~3，占空比档位
    period: u16,       // 11 位频率值（0~2047），频率 = 131072/(2048-period) Hz
    timer: u16,        // 当前波形步还剩下的周期
    step: u8,          // 0~7 波形步
    volume: u8,        // 当前音量（含包络效果，0~15）
    env_initial: u8,
    env_dir: bool,     // true=音量递增
    env_period: u8,    // 包络步长（0=关闭）
    env_timer: u8,
    length: u8,        // 长度计数器（0~64）
    length_enable: bool,
}
impl Square {
    fn new() -> Self {
        Square { enabled: false, dac_on: false, duty: 0, period: 0, timer: 0, step: 0,
                 volume: 0, env_initial: 0, env_dir: false, env_period: 0, env_timer: 0,
                 length: 0, length_enable: false }
    }
    // 每周期推进，返回当前数字电平（0~15）
    fn tick(&mut self) -> u8 {
        if !self.enabled || !self.dac_on { return 0; }
        if self.timer == 0 {
            self.timer = (2048 - self.period) * 4;   // 每步 4×(2048-period) 周期
            self.step = (self.step + 1) & 7;
        }
        self.timer -= 1;
        if (DUTY[self.duty as usize] >> self.step) & 1 != 0 { self.volume } else { 0 }
    }
    // 帧序列器一个 step：偶数步走长度计数器，step7 走音量包络
    fn frame_tick(&mut self, step: u8) {
        if step % 2 == 0 && self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 { self.enabled = false; }
        }
        if step == 7 && self.env_period > 0 {
            if self.env_timer == 0 {
                self.env_timer = self.env_period;
                if self.env_dir && self.volume < 15 { self.volume += 1; }
                else if !self.env_dir && self.volume > 0 { self.volume -= 1; }
            } else { self.env_timer -= 1; }
        }
    }
    // 触发（写 NRx4 bit7=1 且 DAC 开）
    fn trigger(&mut self) {
        self.enabled = self.dac_on;
        self.timer = (2048 - self.period) * 4;
        self.step = 0;
        self.volume = self.env_initial;
        self.env_timer = self.env_period;
        if self.length == 0 { self.length = 64; }
    }
}

// ---------- 波形通道（ch3） ----------
struct Wave {
    enabled: bool,
    dac_on: bool,
    period: u16,
    timer: u16,
    pos: u8,           // 0~31，32 采样表里的当前位置
    volume_shift: u8,  // NR32 bit5-6：0→4(静音) 1→0 2→1 3→2
    length: u8,
    length_enable: bool,
}
impl Wave {
    fn new() -> Self {
        Wave { enabled: false, dac_on: false, period: 0, timer: 0, pos: 0, volume_shift: 4, length: 0, length_enable: false }
    }
    // wave_ram 在 Apu 里，这里只推进位置，采样取值由 Apu::mix 做
    fn tick(&mut self) -> u8 {
        if !self.enabled || !self.dac_on || self.volume_shift == 4 { return 0; }
        if self.timer == 0 {
            self.timer = (2048 - self.period) * 2;   // 波形每步 2×(2048-period) 周期
            self.pos = (self.pos + 1) & 31;
        }
        self.timer -= 1;
        self.pos
    }
    fn frame_tick(&mut self, step: u8) {
        if step % 2 == 0 && self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 { self.enabled = false; }
        }
    }
    fn trigger(&mut self) {
        self.enabled = self.dac_on;
        self.timer = (2048 - self.period) * 2;
        self.pos = 0;
        if self.length == 0 { self.length = 64; }
    }
}

// ---------- 噪声通道（ch4） ----------
struct Noise {
    enabled: bool,
    dac_on: bool,
    shift: u16,        // 15 位 LFSR（只用低 15 位）
    width7: bool,      // true=7 位模式
    divisor: u8,       // NR43 bit0-2
    clock_shift: u8,   // NR43 bit4-7
    timer: u32,        // 噪声周期可达 8<<15，超出 u16
    volume: u8, env_initial: u8, env_dir: bool, env_period: u8, env_timer: u8,
    length: u8, length_enable: bool,
}
impl Noise {
    fn new() -> Self {
        Noise { enabled: false, dac_on: false, shift: 0x7FFF, width7: false, divisor: 0, clock_shift: 0,
                timer: 0, volume: 0, env_initial: 0, env_dir: false, env_period: 0, env_timer: 0,
                length: 0, length_enable: false }
    }
    fn tick(&mut self) -> u8 {
        if !self.enabled || !self.dac_on { return 0; }
        if self.timer == 0 {
            // 移位周期：divisor==0 → 8，否则 divisor×16；再按 clock_shift 左移
            let base: u32 = if self.divisor == 0 { 8 } else { (self.divisor as u32) * 16 };
            self.timer = base << self.clock_shift;
            // LFSR 反馈：15 位用 bit0^bit1，7 位用 bit0^bit6
            let tap = if self.width7 { (self.shift >> 6) & 1 } else { (self.shift >> 1) & 1 };
            let fb = (self.shift ^ tap) & 1;
            self.shift = (self.shift >> 1) | (fb << 14);
            if self.width7 { self.shift = (self.shift & 0x3F) | (fb << 6); }
        }
        self.timer -= 1;
        // 噪声输出 = bit0 取反（低电平响）
        if self.shift & 1 == 0 { self.volume } else { 0 }
    }
    fn frame_tick(&mut self, step: u8) {
        if step % 2 == 0 && self.length_enable && self.length > 0 {
            self.length -= 1;
            if self.length == 0 { self.enabled = false; }
        }
        if step == 7 && self.env_period > 0 {
            if self.env_timer == 0 {
                self.env_timer = self.env_period;
                if self.env_dir && self.volume < 15 { self.volume += 1; }
                else if !self.env_dir && self.volume > 0 { self.volume -= 1; }
            } else { self.env_timer -= 1; }
        }
    }
    fn trigger(&mut self) {
        self.enabled = self.dac_on;
        self.shift = 0x7FFF;
        self.volume = self.env_initial;
        self.env_timer = self.env_period;
        if self.length == 0 { self.length = 64; }
    }
}

// ---------- APU 本体 ----------
pub struct Apu {
    pub samples: Vec<i16>,   // 44100Hz 输出
    sample_accum: f64,
    sample_count: u32,
    // 帧序列器：512Hz，每 8192 周期一个 step，8 步循环
    frame_cycles: u32,
    frame_step: u8,
    // 通道
    ch1: Square, ch2: Square, ch3: Wave, ch4: Noise,
    wave_ram: [u8; 16],      // 0xFF30-0xFF3F，每字节 2 个 4 位样本
    nr50: u8, nr51: u8, nr52: u8,   // 主音量 / 声道选择 / 电源开关
}

impl Apu {
    pub fn new() -> Self {
        Apu {
            samples: Vec::new(), sample_accum: 0.0, sample_count: 0,
            frame_cycles: 0, frame_step: 0,
            ch1: Square::new(), ch2: Square::new(), ch3: Wave::new(), ch4: Noise::new(),
            wave_ram: [0; 16], nr50: 0, nr51: 0, nr52: 0xF1,   // 真机 boot ROM 后 NR52=0xF1（电源开）
        }
    }

    // 每走 t 个周期调一次：推进帧序列器 + 各通道，按需下采样存样本
    pub fn tick(&mut self, t: u32) {
        // 先按周期数逐周期推进（t 通常很小，几十以内）
        for _ in 0..t {
            // 帧序列器：每 8192 周期走一个 step
            self.frame_cycles += 1;
            if self.frame_cycles >= 8192 {
                self.frame_cycles = 0;
                let step = self.frame_step;
                self.frame_step = (self.frame_step + 1) & 7;
                self.ch1.frame_tick(step);
                self.ch2.frame_tick(step);
                self.ch3.frame_tick(step);
                self.ch4.frame_tick(step);
            }
            // 采样：累加当前混音电平，够 95 个周期出一个样本
            self.sample_accum += self.mix();
            self.sample_count += 1;
            if self.sample_count >= SAMPLE_EVERY {
                let avg = self.sample_accum / self.sample_count as f64;   // 有符号，约 -60~+60
                let v = (avg / 60.0).clamp(-1.0, 1.0);                     // 归一化到 -1~1，静音=0
                self.samples.push((v * 32767.0) as i16);
                self.sample_accum = 0.0;
                self.sample_count = 0;
            }
        }
    }

    // 当前混音电平（有符号，约 -60~+60）：4 通道各自「去直流」后相加。
    // 真机靠隔直电容去掉 DC 分量，让静音=0、发声在 ± 之间振荡。
    fn mix(&mut self) -> f64 {
        if self.nr52 & 0x80 == 0 { return 0.0; }   // 电源关 = 全静音
        let mut sum = 0.0f64;
        // 方波1/2：输出 0~volume，减去半幅 volume/2 后中心化
        let c1 = self.ch1.tick();
        if self.ch1.enabled && self.ch1.dac_on { sum += c1 as f64 - self.ch1.volume as f64 * 0.5; }
        let c2 = self.ch2.tick();
        if self.ch2.enabled && self.ch2.dac_on { sum += c2 as f64 - self.ch2.volume as f64 * 0.5; }
        // 波形：输出 nibble>>shift（0~7.5 量级），中心 7.5>>shift
        if self.ch3.enabled && self.ch3.dac_on && self.ch3.volume_shift != 4 {
            let pos = self.ch3.tick();
            let ws = self.wave_ram[pos as usize / 2];
            let nibble = if pos & 1 == 0 { ws >> 4 } else { ws & 0x0F };
            let lvl = nibble >> self.ch3.volume_shift;
            sum += lvl as f64 - 7.5 / (1u8 << self.ch3.volume_shift) as f64;
        }
        // 噪声：输出 0~volume，中心 volume/2
        let c4 = self.ch4.tick();
        if self.ch4.enabled && self.ch4.dac_on { sum += c4 as f64 - self.ch4.volume as f64 * 0.5; }
        sum
    }

    // ---------- 寄存器读写（0xFF10~0xFF3F） ----------
    pub fn read(&self, addr: u16) -> u8 {
        match addr {
            0xFF10 => 0x80,                        // 扫频未实现，bit7 恒 1
            0xFF11 => 0x80 | (self.ch1.duty << 6), // 长度已消耗，只留占空比
            0xFF12 => 0,
            0xFF13 => 0xFF,
            0xFF14 => 0x80 | 0x3F,                 // 频率低 3 位（读不到真实值，给掩码）
            0xFF16 => 0x80 | (self.ch2.duty << 6),
            0xFF17 => 0,
            0xFF18 => 0xFF,
            0xFF19 => 0x80 | 0x3F,
            0xFF1A => if self.ch3.dac_on { 0x80 } else { 0 },
            0xFF1B => 0xFF,
            0xFF1C => 0x9F,
            0xFF1D => 0xFF,
            0xFF1E => 0x80 | 0x3F,
            0xFF20 => 0xFF,
            0xFF21 => 0,
            0xFF22 => 0,
            0xFF23 => 0x80 | 0x3F,
            0xFF24 => self.nr50,
            0xFF25 => self.nr51,
            0xFF26 => self.nr52 & 0xF0 | self.status_bits(),
            0xFF30..=0xFF3F => self.wave_ram[(addr - 0xFF30) as usize],
            _ => 0xFF,
        }
    }
    fn status_bits(&self) -> u8 {
        (if self.ch1.enabled { 0x01 } else { 0 })
            | (if self.ch2.enabled { 0x02 } else { 0 })
            | (if self.ch3.enabled { 0x04 } else { 0 })
            | (if self.ch4.enabled { 0x08 } else { 0 })
    }

    pub fn write(&mut self, addr: u16, val: u8) {
        // 电源关时，除 NR52 和波形表外全部忽略
        if self.nr52 & 0x80 == 0 && !(addr == 0xFF26 || (0xFF30..=0xFF3F).contains(&addr)) { return; }
        match addr {
            0xFF10 => {}   // 扫频：本批跳过
            0xFF11 => { self.ch1.duty = val >> 6; self.ch1.length = 64 - (val & 0x3F); }
            0xFF12 => {
                self.ch1.env_initial = val >> 4;
                self.ch1.env_dir = val & 0x08 != 0;
                self.ch1.env_period = val & 0x07;
                self.ch1.dac_on = val & 0xF8 != 0;
                if !self.ch1.dac_on { self.ch1.enabled = false; }
            }
            0xFF13 => { self.ch1.period = (self.ch1.period & 0x700) | val as u16; }
            0xFF14 => {
                self.ch1.period = (self.ch1.period & 0xFF) | (((val & 0x07) as u16) << 8);
                self.ch1.length_enable = val & 0x40 != 0;
                if val & 0x80 != 0 { self.ch1.trigger(); }
            }
            0xFF16 => { self.ch2.duty = val >> 6; self.ch2.length = 64 - (val & 0x3F); }
            0xFF17 => {
                self.ch2.env_initial = val >> 4;
                self.ch2.env_dir = val & 0x08 != 0;
                self.ch2.env_period = val & 0x07;
                self.ch2.dac_on = val & 0xF8 != 0;
                if !self.ch2.dac_on { self.ch2.enabled = false; }
            }
            0xFF18 => { self.ch2.period = (self.ch2.period & 0x700) | val as u16; }
            0xFF19 => {
                self.ch2.period = (self.ch2.period & 0xFF) | (((val & 0x07) as u16) << 8);
                self.ch2.length_enable = val & 0x40 != 0;
                if val & 0x80 != 0 { self.ch2.trigger(); }
            }
            0xFF1A => { self.ch3.dac_on = val & 0x80 != 0; if !self.ch3.dac_on { self.ch3.enabled = false; } }
            0xFF1B => { self.ch3.length = 256u16.wrapping_sub(val as u16) as u8; if self.ch3.length > 64 { self.ch3.length = 64; } }
            0xFF1C => { self.ch3.volume_shift = (val >> 5) & 3; }
            0xFF1D => { self.ch3.period = (self.ch3.period & 0x700) | val as u16; }
            0xFF1E => {
                self.ch3.period = (self.ch3.period & 0xFF) | (((val & 0x07) as u16) << 8);
                self.ch3.length_enable = val & 0x40 != 0;
                if val & 0x80 != 0 { self.ch3.trigger(); }
            }
            0xFF20 => { self.ch4.length = 64 - (val & 0x3F); }
            0xFF21 => {
                self.ch4.env_initial = val >> 4;
                self.ch4.env_dir = val & 0x08 != 0;
                self.ch4.env_period = val & 0x07;
                self.ch4.dac_on = val & 0xF8 != 0;
                if !self.ch4.dac_on { self.ch4.enabled = false; }
            }
            0xFF22 => { self.ch4.divisor = val & 0x07; self.ch4.width7 = val & 0x08 != 0; self.ch4.clock_shift = val >> 4; }
            0xFF23 => { self.ch4.length_enable = val & 0x40 != 0; if val & 0x80 != 0 { self.ch4.trigger(); } }
            0xFF24 => { self.nr50 = val; }
            0xFF25 => { self.nr51 = val; }
            0xFF26 => {
                let was_on = self.nr52 & 0x80 != 0;
                self.nr52 = (self.nr52 & 0x7F) | (val & 0x80);
                if was_on && self.nr52 & 0x80 == 0 {
                    // 关机：清空所有通道状态
                    self.ch1 = Square::new(); self.ch2 = Square::new();
                    self.ch3 = Wave::new(); self.ch4 = Noise::new();
                    self.frame_step = 0; self.frame_cycles = 0;
                }
            }
            0xFF30..=0xFF3F => { self.wave_ram[(addr - 0xFF30) as usize] = val; }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 配置并触发方波1，跑 1 秒，验证有非零输出（且是方波=有正有负）
    #[test]
    fn square_wave_produces_sound() {
        let mut apu = Apu::new();
        apu.write(0xFF26, 0x80);      // 电源开
        apu.write(0xFF24, 0x77);      // 主音量：左右都 7
        apu.write(0xFF25, 0x11);      // 路由：ch1 → 左右两声道
        apu.write(0xFF12, 0xF1);      // ch1：音量15、包络递减、周期1、DAC 开
        apu.write(0xFF13, 0x00);      // 频率低 8 位
        apu.write(0xFF14, 0x80);      // 触发（频率高3位=0）
        apu.tick(4_194_304);          // 跑 1 秒
        let nz = apu.samples.iter().filter(|&&s| s != 0).count();
        let pos = apu.samples.iter().filter(|&&s| s > 0).count();
        let neg = apu.samples.iter().filter(|&&s| s < 0).count();
        assert!(nz > 0, "应该有非零样本");
        assert!(pos > 0 && neg > 0, "方波应该有正有负（pos={pos} neg={neg}）");
    }

    // 电源关时应该全静音
    #[test]
    fn power_off_is_silent() {
        let mut apu = Apu::new();
        apu.write(0xFF26, 0x00);      // 电源关
        apu.write(0xFF25, 0x11);
        apu.write(0xFF12, 0xF1);
        apu.write(0xFF13, 0x00);
        apu.write(0xFF14, 0x80);
        apu.tick(4_194_304);
        assert!(apu.samples.iter().all(|&s| s == 0), "电源关应全静音");
    }
}
