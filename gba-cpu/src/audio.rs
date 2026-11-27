#[derive(Default)]
struct Voice {
    enabled: bool,
    length: u16,
    countdown: u32,
    phase: usize,
    volume: u8,
    envelope_timer: u8,
}

#[derive(Default)]
pub(crate) struct Psg {
    registers: [u16; 17],
    wave: [[u8; 16]; 2],
    voices: [Voice; 4],
    noise: u16,
    frame_cycles: u16,
    frame_step: u8,
    sweep_timer: u8,
}

const SHAPE: [usize; 4] = [1, 4, 9, 12];
const CONTROL: [usize; 4] = [2, 6, 10, 14];
const MASK: [u16; 17] = [
    0x007F, 0xFFFF, 0xC7FF, 0, 0xFFFF, 0, 0xC7FF, 0, 0x00E0,
    0xE0FF, 0xC7FF, 0, 0xFF3F, 0, 0xC0FF, 0, 0xFF77,
];
const READ_MASK: [u16; 17] = [
    0x007F, 0xFFC0, 0x4000, 0, 0xFFC0, 0, 0x4000, 0, 0x00E0,
    0xE000, 0x4000, 0, 0xFF00, 0, 0x40FF, 0, 0xFF77,
];

impl Psg {
    pub(crate) fn read(&self, addr: u32) -> u16 {
        if addr >= 0x0400_0090 {
            let offset = (addr - 0x0400_0090) as usize;
            return u16::from_le_bytes(self.wave[self.wave_cpu_bank()][offset..offset + 2].try_into().unwrap());
        }
        let index = ((addr - 0x0400_0060) / 2) as usize;
        self.registers[index] & READ_MASK[index]
    }

    pub(crate) fn write(&mut self, addr: u32, value: u16, mask: u16, enabled: bool) {
        if addr >= 0x0400_0090 {
            let offset = (addr - 0x0400_0090) as usize;
            let bank = self.wave_cpu_bank();
            let before = self.read(addr);
            self.wave[bank][offset..offset + 2].copy_from_slice(&merge(before, value, mask).to_le_bytes());
            return;
        }
        if !enabled { return; }
        let index = ((addr - 0x0400_0060) / 2) as usize;
        let value = merge(self.registers[index], value, mask) & MASK[index];
        self.registers[index] = value & if CONTROL.contains(&index) { 0x7FFF } else { 0xFFFF };

        if let Some(channel) = SHAPE.iter().position(|&shape| shape == index) {
            let limit = if channel == 2 { 256 } else { 64 };
            if mask & 0xFF != 0 {
                self.voices[channel].length = limit - (value & (limit - 1));
            }
            if channel != 2 && value & 0xF800 == 0 {
                self.voices[channel].enabled = false;
            }
        }
        if index == 8 && value & 0x80 == 0 {
            self.voices[2].enabled = false;
        }
        if value & 0x8000 != 0 {
            if let Some(channel) = CONTROL.iter().position(|&control| control == index) {
                self.trigger(channel);
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        let wave = self.wave;
        *self = Self::default();
        self.wave = wave;
    }

    pub(crate) fn status(&self) -> u16 {
        self.voices.iter().enumerate().fold(0, |bits, (i, voice)| bits | (u16::from(voice.enabled) << i))
    }

    fn wave_cpu_bank(&self) -> usize { usize::from((self.registers[8] >> 6 & 1) ^ 1) }

    fn period(&self, channel: usize) -> u32 {
        let setting = u32::from(self.registers[CONTROL[channel]]);
        match channel {
            0 | 1 => 16 * (2048 - (setting & 2047)),
            2 => 8 * (2048 - (setting & 2047)),
            _ => ((setting & 7) * 64).max(32) << ((setting >> 4) & 15),
        }
    }

    fn trigger(&mut self, channel: usize) {
        let shape = self.registers[SHAPE[channel]];
        let period = self.period(channel);
        let voice = &mut self.voices[channel];
        voice.enabled = if channel == 2 { self.registers[8] & 0x80 != 0 } else { shape & 0xF800 != 0 };
        if voice.length == 0 { voice.length = if channel == 2 { 256 } else { 64 }; }
        voice.countdown = period;
        voice.phase = 0;
        voice.volume = (shape >> 12) as u8;
        voice.envelope_timer = (shape >> 8 & 7) as u8;
        if channel == 0 {
            self.sweep_timer = (self.registers[0] >> 4 & 7) as u8;
            if self.registers[0] & 7 != 0 && self.sweep_target().is_none() {
                self.voices[0].enabled = false;
            }
        }
        if channel == 3 { self.noise = 0x7FFF; }
    }

    fn sweep_target(&self) -> Option<u16> {
        let sweep = self.registers[0];
        let frequency = self.registers[2] & 2047;
        let delta = frequency >> (sweep & 7);
        let target = if sweep & 8 == 0 { frequency + delta } else { frequency - delta };
        (target < 2048).then_some(target)
    }

    pub(crate) fn tick(&mut self) {
        for channel in 0..4 {
            let voice = &mut self.voices[channel];
            if !voice.enabled { continue; }
            voice.countdown -= 1;
            if voice.countdown != 0 { continue; }
            voice.phase = voice.phase.wrapping_add(1);
            if channel == 3 {
                let next = (self.noise ^ (self.noise >> 1)) & 1;
                self.noise = (self.noise >> 1) | (next << 14);
                if self.registers[14] & 8 != 0 {
                    self.noise = (self.noise & !(1 << 6)) | (next << 6);
                }
            }
            self.voices[channel].countdown = self.period(channel);
        }
        self.frame_cycles += 1;
        if self.frame_cycles != 32_768 { return; }
        self.frame_cycles = 0;
        for i in 0..4 {
            let voice = &mut self.voices[i];
            if self.frame_step & 1 == 0 && self.registers[CONTROL[i]] & 0x4000 != 0 && voice.length != 0 {
                voice.length -= 1;
                if voice.length == 0 { voice.enabled = false; }
            }
            if self.frame_step == 7 && i != 2 && voice.envelope_timer != 0 {
                voice.envelope_timer -= 1;
                if voice.envelope_timer == 0 {
                    let shape = self.registers[SHAPE[i]];
                    voice.envelope_timer = (shape >> 8 & 7) as u8;
                    voice.volume = if shape & 0x800 != 0 { (voice.volume + 1).min(15) }
                        else { voice.volume.saturating_sub(1) };
                }
            }
        }
        if self.frame_step & 3 == 2 && self.sweep_timer != 0 && self.voices[0].enabled {
            self.sweep_timer -= 1;
            if self.sweep_timer == 0 {
                let sweep = self.registers[0];
                self.sweep_timer = (sweep >> 4 & 7) as u8;
                if sweep & 7 != 0 {
                    if let Some(target) = self.sweep_target() {
                        self.registers[2] = (self.registers[2] & !2047) | target;
                        if self.sweep_target().is_none() { self.voices[0].enabled = false; }
                    } else { self.voices[0].enabled = false; }
                }
            }
        }
        self.frame_step = (self.frame_step + 1) & 7;
    }

    pub(crate) fn mix(&self, scale: u16) -> [i16; 2] {
        let routing = self.registers[16];
        let mut stereo = [0; 2];
        for (i, voice) in self.voices.iter().enumerate() {
            if !voice.enabled { continue; }
            let level = match i {
                0 | 1 => {
                    let duty = (self.registers[SHAPE[i]] >> 6 & 3) as usize;
                    let high = [1, 2, 4, 6][duty];
                    i16::from(voice.volume) * if voice.phase & 7 < high { 1 } else { -1 }
                }
                2 => self.wave_level(voice.phase),
                _ => i16::from(voice.volume) * if self.noise & 1 == 0 { 1 } else { -1 },
            };
            if routing & (0x1000 << i) != 0 { stereo[0] += level; }
            if routing & (0x0100 << i) != 0 { stereo[1] += level; }
        }
        let gain = [1, 2, 4, 4][usize::from(scale & 3)];
        [
            stereo[0] * (1 + ((routing >> 4) & 7) as i16) * gain / 32,
            stereo[1] * (1 + (routing & 7) as i16) * gain / 32,
        ]
    }

    fn wave_level(&self, phase: usize) -> i16 {
        let select = self.registers[8];
        let mut bank = usize::from(select >> 6 & 1);
        if select & 0x20 != 0 { bank ^= phase >> 5 & 1; }
        let raw = self.wave[bank][(phase >> 1) & 15];
        let sample = if phase & 1 == 0 { raw >> 4 } else { raw & 15 };
        let volume = self.registers[9];
        let gain = if volume & 0x8000 != 0 { 3 } else { [0, 4, 2, 1][usize::from(volume >> 13 & 3)] };
        (i16::from(sample) * 2 - 15) * gain / 4
    }
}

fn merge(before: u16, value: u16, mask: u16) -> u16 {
    (before & !mask) | (value & mask)
}
