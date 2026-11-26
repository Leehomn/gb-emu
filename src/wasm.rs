//! wasm 导出层 —— 手写 C ABI 函数，供 JS 用 WebAssembly.instantiate 调用。
//! 只编译进 wasm32 目标（桌面构建完全忽略本模块）。
//!
//! 内存协议（关键，别踩）：
//!   · wasm 线性内存会被 Rust allocator 在任意时刻 grow（Box/Vec 分配都可能触发），
//!     所以 JS 侧【绝不长期持有 Uint8Array 视图】——每次要读写都现取：
//!        new Uint8Array(memory.buffer, ptr, len)
//!   · 输入缓冲（ROM）由 gb_alloc 分配，JS 写一次 → gb_load 拷贝进 cart → gb_free_alloc 释放；
//!   · 屏幕帧 / SRAM / 音频样本都是 Emu 的固定字段或 Vec，接口按「先取指针读，读完立刻
//!     不再依赖它」的节奏走，绝不在两次调用之间跨 grow 持视图。

use crate::{ppu, Bus, Cpu};

// 一帧 = 456 周期/线 × 154 线（真机节奏，59.7 fps）
const FRAME_CYCLES: u32 = 70224;
const SCREEN_LEN: usize = ppu::SCREEN_W * ppu::SCREEN_H;

// 模拟器整机状态：CPU + PPU + APU + 固定屏幕帧，JS 拿到的是它的裸指针
pub struct Emu {
    bus: Bus,
    cpu: Cpu,
    screen: [u8; SCREEN_LEN],
}

/// 分配 n 字节（不初始化），供 JS 写入输入数据（如 ROM）。用完调 gb_free_alloc 释放。
#[unsafe(no_mangle)]
pub extern "C" fn gb_alloc(n: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(n);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// 释放 gb_alloc 分的内存（长度必须和分配时一致，重新构回 Vec 交给 drop）。
#[unsafe(no_mangle)]
pub extern "C" fn gb_free_alloc(p: *mut u8, n: usize) {
    unsafe {
        if !p.is_null() {
            drop(Vec::from_raw_parts(p, 0, n));
        }
    }
}

/// 建一台新模拟器，返回不透明指针（JS 存下来，每次调用传回）
#[unsafe(no_mangle)]
pub extern "C" fn gb_new() -> *mut Emu {
    Box::into_raw(Box::new(Emu {
        bus: Bus::new(),
        cpu: Cpu::new(),
        screen: [0; SCREEN_LEN],
    }))
}

/// 装载卡带 ROM（rom_ptr 指向 gb_alloc 分到的内存，rom_len 字节）
#[unsafe(no_mangle)]
pub extern "C" fn gb_load(ptr: *mut Emu, rom_ptr: *const u8, rom_len: usize) {
    unsafe {
        let emu = &mut *ptr;
        let rom = std::slice::from_raw_parts(rom_ptr, rom_len);
        emu.bus.load_rom(rom);
    }
}

/// 跑满一整帧（70224 周期），并把 framebuffer 拷进固定 screen 字段
#[unsafe(no_mangle)]
pub extern "C" fn gb_run_frame(ptr: *mut Emu) {
    unsafe {
        let emu = &mut *ptr;
        let mut spent = 0u32;
        while spent < FRAME_CYCLES {
            spent += emu.cpu.step(&mut emu.bus);   // PPU 内嵌在 Bus 里，随访存逐 M-cycle 推进
        }
        for y in 0..ppu::SCREEN_H {
            for x in 0..ppu::SCREEN_W {
                emu.screen[y * ppu::SCREEN_W + x] = emu.bus.ppu.framebuffer[y][x];
            }
        }
    }
}

/// 屏幕帧指针（160×144 = 23040 字节，行优先，每字节色号 0~3）。
/// JS 每帧用 `new Uint8Array(memory.buffer, ptr, 23040)` 现取视图读。
#[unsafe(no_mangle)]
pub extern "C" fn gb_screen_ptr(ptr: *mut Emu) -> *const u8 {
    unsafe { (&*ptr).screen.as_ptr() }
}

// ---------- 音频：指针 + 长度 + 清空 三步协议 ----------
// 每帧节奏：gb_run_frame（产生样本）→ gb_audio_ptr/gb_audio_len 读 → gb_audio_clear 清空。
// 读样本期间不跑模拟，Vec 不会 realloc，指针安全。

/// 当前累积的音频样本数（44100Hz，i16）
#[unsafe(no_mangle)]
pub extern "C" fn gb_audio_len(ptr: *mut Emu) -> usize {
    unsafe { (&*ptr).bus.apu.samples.len() }
}

/// 音频样本指针（i16 小端）。读完立刻调 gb_audio_clear，别跨 gb_run_frame 持此指针。
#[unsafe(no_mangle)]
pub extern "C" fn gb_audio_ptr(ptr: *mut Emu) -> *const i16 {
    unsafe { (&*ptr).bus.apu.samples.as_ptr() }
}

/// 清空已读走的音频样本
#[unsafe(no_mangle)]
pub extern "C" fn gb_audio_clear(ptr: *mut Emu) {
    unsafe { (&mut *ptr).bus.apu.samples.clear(); }
}

// ---------- SRAM 存档：长度 + 指针 + 写回 三步 ----------

/// SRAM 大小（字节）。0 = 本卡带没存档空间。
#[unsafe(no_mangle)]
pub extern "C" fn gb_sram_len(ptr: *mut Emu) -> usize {
    unsafe { (&*ptr).bus.ram.len() }
}

/// SRAM 指针（读存档用）。load_rom 之后才稳定；重新 gb_load 后要重新取。
#[unsafe(no_mangle)]
pub extern "C" fn gb_sram_ptr(ptr: *mut Emu) -> *const u8 {
    unsafe { (&*ptr).bus.ram.as_ptr() }
}

/// 把存档写回 SRAM（src 来自 JS 的 Uint8Array，len 必须等于 gb_sram_len）
#[unsafe(no_mangle)]
pub extern "C" fn gb_sram_load(ptr: *mut Emu, src: *const u8, len: usize) {
    unsafe {
        let emu = &mut *ptr;
        if len == emu.bus.ram.len() {
            let data = std::slice::from_raw_parts(src, len);
            emu.bus.ram.copy_from_slice(data);
        }
    }
}

/// 设置按键状态（dir/btn 低 4 位，0=按下，和桌面版 joypad 位序一致）
#[unsafe(no_mangle)]
pub extern "C" fn gb_set_keys(ptr: *mut Emu, dir: u8, btn: u8) {
    unsafe {
        let emu = &mut *ptr;
        emu.bus.joypad_dir = dir;
        emu.bus.joypad_btn = btn;
    }
}

/// 推进 MBC3 RTC 走时：JS 每帧把真实墙钟秒数（Date.now()/1000 取整）灌进来。
/// 仅 MBC3+TIMER 卡带生效；首帧记基准、此后按差值推进，停止标志置位时冻结。
#[unsafe(no_mangle)]
pub extern "C" fn gb_tick_rtc(ptr: *mut Emu, wall: u64) {
    unsafe { (&mut *ptr).bus.tick_rtc(wall); }
}

/// 释放模拟器（JS 侧 drop 掉实例时调，别重复调）
#[unsafe(no_mangle)]
pub extern "C" fn gb_free(ptr: *mut Emu) {
    unsafe {
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
    }
}
