//! wasm 导出层 —— 手写 C ABI 函数，供 JS 用 WebAssembly.instantiate 调用。
//! 只编译进 wasm32 目标（桌面构建完全忽略本模块）。
//!
//! 内存协议（关键，别踩）：
//!   · wasm 线性内存会被 Rust allocator 在任意时刻 grow（Box/Vec 分配都可能触发），
//!     所以 JS 侧【绝不长期持有 Uint8Array 视图】——每次要读写都现取：
//!        new Uint8Array(memory.buffer, ptr, len)
//!   · 输入缓冲（ROM）由 Rust 的 gb_alloc 分配，JS 只往里写一次、写完全弃视图；
//!   · 屏幕帧是 Emu 的固定字段，gb_screen_ptr 返回稳定指针，JS 每帧现取视图读；
//!   · JS 只持有一个不透明指针（gb_new 返回），从不自己解引用。

use crate::{ppu, Bus, Cpu, Ppu};

// 一帧 = 456 周期/线 × 154 线（真机节奏，59.7 fps）
const FRAME_CYCLES: u32 = 70224;
const SCREEN_LEN: usize = ppu::SCREEN_W * ppu::SCREEN_H;

// 模拟器整机状态：CPU + PPU + APU + 固定屏幕帧，JS 拿到的是它的裸指针
pub struct Emu {
    bus: Bus,
    cpu: Cpu,
    ppu: Ppu,
    screen: [u8; SCREEN_LEN],
}

/// 分配 n 字节（不初始化），返回指针供 JS 写入输入数据（如 ROM）。
/// 用 Vec + forget 借 Rust 分配器，JS 写完传给 gb_load 后即可弃。
#[unsafe(no_mangle)]
pub extern "C" fn gb_alloc(n: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(n);
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// 建一台新模拟器，返回不透明指针（JS 存下来，每次调用传回）
#[unsafe(no_mangle)]
pub extern "C" fn gb_new() -> *mut Emu {
    Box::into_raw(Box::new(Emu {
        bus: Bus::new(),
        cpu: Cpu::new(),
        ppu: Ppu::new(),
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
            let t = emu.cpu.step(&mut emu.bus);
            emu.ppu.tick(t, &mut emu.bus);
            spent += t;
        }
        for y in 0..ppu::SCREEN_H {
            for x in 0..ppu::SCREEN_W {
                emu.screen[y * ppu::SCREEN_W + x] = emu.ppu.framebuffer[y][x];
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

/// 取走累计的音频样本（i16 小端），写满 out（最多 max 个），返回实际样本数。
/// 采样率 44100Hz；drain 掉已取走的样本，避免下次重复。
/// out 由 JS 用 gb_alloc 分好（至少 max*2 字节），每次调用后现取视图读。
#[unsafe(no_mangle)]
pub extern "C" fn gb_audio(ptr: *mut Emu, out: *mut i16, max: usize) -> usize {
    unsafe {
        let emu = &mut *ptr;
        let n = emu.bus.apu.samples.len().min(max);
        let out = std::slice::from_raw_parts_mut(out, n);
        for (i, s) in emu.bus.apu.samples.drain(..n).enumerate() {
            out[i] = s;
        }
        n
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

/// 释放模拟器（JS 侧 drop 掉实例时调，别重复调）
#[unsafe(no_mangle)]
pub extern "C" fn gb_free(ptr: *mut Emu) {
    unsafe {
        if !ptr.is_null() {
            drop(Box::from_raw(ptr));
        }
    }
}
