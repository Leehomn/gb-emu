// GameBoy 模拟器 —— 桌面入口（薄壳）。
// 核心逻辑都在 src/lib.rs（crate 名 gb_emu），本文件只保留命令行入口 + 实时窗口。
// 运行：cargo run --release -- roms/<游戏>.gb [--screenshot out.ppm | --play | --wav out.wav | --tone]

use gb_emu::{apu, ppu, Apu, Bus, Cpu};
fn main() {
    // 带参数 = 跑考卷：cargo run --release -- roms/cpu_instrs.gb [--screenshot out.ppm]
    // --play = 实时窗口玩：cargo run --release -- roms/<游戏>.gb --play
    // 不带参数 = CALL/RET 小演示
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--tone") {
        // 试音：直接触发方波1，跑 1 秒写成 WAV，肉耳验证 APU 发声
        let mut apu = Apu::new();
        apu.write(0xFF26, 0x80);   // 电源开
        apu.write(0xFF24, 0x77);   // 主音量 7/7
        apu.write(0xFF25, 0x11);   // ch1 → 左右
        apu.write(0xFF12, 0xF1);   // 音量15 递减 周期1 DAC开
        apu.write(0xFF13, 0x00);   // 频率低8位=0（最高音）
        apu.write(0xFF14, 0x80);   // 触发
        apu.tick(4_194_304);
        apu::write_wav(&apu.samples, "tone.wav").expect("写 tone.wav 失败");
        println!("试音已存 tone.wav（{} 样本 ≈1 秒），用播放器听听", apu.samples.len());
        return;
    }
    if args.iter().any(|a| a == "--play") {
        let rom_path = args.iter().find(|a| a.ends_with(".gb")).expect("要指定 ROM 文件");
        play(rom_path);
        return;
    }
    // mealybug tearoom tests：跑到 LD B,B(0x40) 软件断点时把 framebuffer dump 成原始色号字节，
    // 供外部脚本与 expected 标准答案 PNG 逐像素比对。用法：--mealybug out.raw
    if args.iter().any(|a| a == "--mealybug") {
        let rom_path = args.iter().find(|a| a.ends_with(".gb")).expect("要指定 ROM 文件");
        let out = args.iter().position(|a| a == "--mealybug").and_then(|i| args.get(i + 1)).cloned();
        mealybug_shot(rom_path, out.as_deref());
        return;
    }
    if args.len() > 1 {
        let rom = std::fs::read(&args[1]).expect("读不到 ROM 文件");
        let mut bus = Bus::new();
        bus.load_rom(&rom);
        let mut cpu = Cpu::new();
        let screenshot = args.iter().position(|a| a == "--screenshot").map(|i| args[i + 1].clone());
        let wav_out = args.iter().position(|a| a == "--wav").map(|i| args[i + 1].clone());
        let frames_limit: u64 = if screenshot.is_some() { u64::MAX } else { u64::MAX };
        println!("考卷开跑：{}（{} 字节）", args[1], rom.len());
        let mut last_len = 0usize;
        let mut stuck = 0u32;
        let mut frames = 0u64;
        let mut prev_ly = 0u8;
        let press_start = args.iter().any(|a| a == "--press-start");   // 诊断：自动按Start
        let press_dir = args.iter().any(|a| a == "--press-dir");       // 诊断：周期性乱按方向键（触发游戏声音）
        for _i in 0..500_000_000u64 {
            // 诊断脚本：第30~40帧按住 Start（Enter 对应的键位）
            if press_start {
                if frames >= 30 && frames < 40 { bus.joypad_btn &= !0x08; } else { bus.joypad_btn |= 0x08; }
            }
            // 诊断：每 20 帧轮换按一个方向键（右/下/左/上循环），触发移动/合并声音
            if press_dir && frames >= 40 {
                bus.joypad_dir = 0x0F;
                match (frames / 20) % 4 {
                    0 => bus.joypad_dir &= !0x01,   // 右
                    1 => bus.joypad_dir &= !0x08,   // 下
                    2 => bus.joypad_dir &= !0x02,   // 左
                    _ => bus.joypad_dir &= !0x04,   // 上
                }
            }
            cpu.step(&mut bus);                                   // PPU 内嵌在 Bus 里，随 CPU 访存逐 M-cycle 推进
            if bus.ppu.ly == 0 && prev_ly != 0 { frames += 1; }   // 行号回绕 = 一帧过完
            prev_ly = bus.ppu.ly;
            if _i % 5_000_000 == 0 && _i > 0 {
                if bus.serial_out.len() == last_len { stuck += 1; } else { stuck = 0; }
                last_len = bus.serial_out.len();
                if stuck >= 2 && screenshot.is_none() { println!("成绩单已稳定，判定交卷"); break; }
            }
            if frames >= frames_limit { break; }
            if press_start && frames >= 80 { break; }   // 诊断：按完Start再跑40帧就停
            if press_dir && frames >= 40 + 80 { break; }   // 诊断：乱按方向键跑够 80 帧就停
            if bus.serial_out.contains("Passed") || bus.serial_out.contains("Failed") {
                if screenshot.is_none() { break; }
            }
            if cpu.unknown.len() > 20 { println!("未实现指令太多，提前交卷"); break; }
        }
        if let Some(path) = screenshot {
            // 截图模式：跑到「成绩单稳定」（全文打完）才拍，不然屏幕只有一行
            if stuck < 2 {
                println!("再跑一会儿让屏幕字打完……");
                let mut s = 0;
                let mut last = bus.serial_out.len();
                for i in 0..600_000_000u64 {
                    cpu.step(&mut bus);
                    if i % 5_000_000 == 0 && i > 0 {
                        if bus.serial_out.len() == last { s += 1; } else { s = 0; }
                        last = bus.serial_out.len();
                        if s >= 3 { println!("全文打完"); break; }
                    }
                }
            }
            save_ppm(&path, &bus.ppu.framebuffer);
            println!("截图已存：{}", path);
        }
        if let Some(path) = wav_out {
            let count = bus.apu.samples.len();
            apu::write_wav(&bus.apu.samples, &path).expect("写 WAV 失败");
            println!("音频已存：{}（{} 个样本，约 {:.1} 秒）", path, count, count as f64 / 44100.0);
            // 诊断：游戏写过的 APU 寄存器（去重后按地址排序）
            let mut seen = std::collections::BTreeSet::new();
            let mut writes: Vec<(u16, u8)> = Vec::new();
            for &(a, v) in &bus.apu_writes { if seen.insert(a) { writes.push((a, v)); } }
            writes.sort();
            println!("游戏写过的 APU 寄存器（{} 个）：", writes.len());
            for (a, v) in writes { println!("  {a:04X} ← {v:02X}"); }
            let nz = bus.apu.samples.iter().filter(|&&s| s != 0).count();
            println!("非零样本：{nz} / {count}");
        }
        let lines: Vec<&str> = bus.serial_out.lines().collect();
        let start = if lines.len() > 20 { lines.len() - 20 } else { 0 };
        for line in &lines[start..] { println!("{}", line); }
        if lines.len() > 20 { println!("……（前面省略 {} 行）", lines.len() - 20); }
        println!("===== 结束 =====");
        println!("共 {} 条指令，{} 周期", cpu.cycles, cpu.cycles);
        if press_start {
            println!("手柄被读了 {} 次", bus.joy_reads);
            println!("前40次读取: 选择位/返回值");
            for &(n, sel, r) in &bus.joy_log { println!("  #{n}: sel={sel:02X} → {r:02X}"); }
        }
        if !cpu.unknown.is_empty() {
            println!("未实现指令清单（{} 处）：", cpu.unknown.len());
            for &(op, at) in &cpu.unknown { println!("  {op:02X} @ {at:04X}"); }
        }
        return;
    }
    // 演示：CALL/RET 子程序 —— 你的 2nnn/00EE 在 GameBoy 上复活
    let mut bus = Bus::new();
    let mut cpu = Cpu::new();
    bus.cart = vec![0; 0x8000];   // 无 MBC 卡带：程序写进 cart
    // 0x0100: LD A,10 / LD B,5 / CALL 0x0150 / HALT
    let main_prog: [u8; 8] = [0x3E, 10, 0x06, 5, 0xCD, 0x50, 0x01, 0x76];
    bus.cart[0x0100..0x0108].copy_from_slice(&main_prog);
    // 0x0150: ADD A,B / RET（子程序：把 B 加进 A 再回家）
    bus.cart[0x0150] = 0x80;
    bus.cart[0x0151] = 0xC9;
    while !cpu.halted { cpu.step(&mut bus); }
    println!("A={}（应为15） SP={:04X}（应回FFFE） 周期={}", cpu.reg.a, cpu.sp, cpu.cycles);
}

// ---------- 实时窗口（minifb） ----------
// 真机节奏：一帧 = 70224 周期（456×154），每秒约 59.7 帧。
// 主循环：跑一帧的指令 → 把 framebuffer 涂色上屏 → 采样键盘 → 睡到下一帧。
fn play(rom_path: &str) {
    use minifb::{Key, Window, WindowOptions};
    use std::time::{Duration, Instant};

    let rom = std::fs::read(rom_path).expect("读不到 ROM");
    let mut bus = Bus::new();
    bus.load_rom(&rom);
    let mut cpu = Cpu::new();

    let scale = 4;
    let (w, h) = (ppu::SCREEN_W * scale, ppu::SCREEN_H * scale);
    println!("准备开窗: {}x{}", w, h);
    std::io::Write::flush(&mut std::io::stdout()).ok();
    let mut window = Window::new(
        &format!("GameBoy 模拟器 · {}", rom_path),
        w, h,
        WindowOptions { resize: false, ..WindowOptions::default() },
    ).expect("开窗失败");
    println!("窗口已创建，is_open={}", window.is_open());
    std::io::Write::flush(&mut std::io::stdout()).ok();

    // 调色板：经典 GameBoy 绿屏四档
    let palette: [u32; 4] = [0xE0F8D0, 0x88C070, 0x346856, 0x081820];
    let mut pixels = vec![0u32; w * h];
    let frame_time = Duration::from_micros(16_740);   // 59.7 fps

    while window.is_open() && !window.is_key_down(Key::Escape) {
        let t0 = Instant::now();

        // 诊断：打印按键检测
        let keys = window.get_keys();
        if !keys.is_empty() {
            println!("检测到按键: {:?}", keys);
            std::io::Write::flush(&mut std::io::stdout()).ok();
        }

        // 采样键盘 → Joypad 寄存器（0=按下）
        // 方向：方向键；功能：X=A Z=B Enter=Start 右Shift=Select
        bus.joypad_dir = 0x0F;
        bus.joypad_btn = 0x0F;
        if window.is_key_down(Key::Right) { bus.joypad_dir &= !0x01; }
        if window.is_key_down(Key::Left)  { bus.joypad_dir &= !0x02; }
        if window.is_key_down(Key::Up)    { bus.joypad_dir &= !0x04; }
        if window.is_key_down(Key::Down)  { bus.joypad_dir &= !0x08; }
        if window.is_key_down(Key::X)     { bus.joypad_btn &= !0x01; }
        if window.is_key_down(Key::Z)     { bus.joypad_btn &= !0x02; }
        if window.is_key_down(Key::RightShift) { bus.joypad_btn &= !0x04; }
        if window.is_key_down(Key::Enter) { bus.joypad_btn &= !0x08; }

        // 跑一帧：70224 周期
        let mut spent = 0u32;
        while spent < 70224 {
            spent += cpu.step(&mut bus);   // PPU 内嵌在 Bus 里
        }

        // 涂色上屏
        for y in 0..ppu::SCREEN_H {
            for x in 0..ppu::SCREEN_W {
                let c = palette[bus.ppu.framebuffer[y][x] as usize];
                for sy in 0..scale {
                    for sx in 0..scale {
                        pixels[(y * scale + sy) * w + x * scale + sx] = c;
                    }
                }
            }
        }
        window.update_with_buffer(&pixels, w, h).expect("上屏失败");

        // 睡到下一帧（限速）
        let elapsed = t0.elapsed();
        if elapsed < frame_time { std::thread::sleep(frame_time - elapsed); }
    }
}

// mealybug tearoom tests：跑到 LD B,B(0x40) 断点时，把 framebuffer 的 160×144 色号(0~3)
// 逐字节 dump 成 .raw，供外部 Python 脚本映射成灰度后与 expected 标准答案 PNG 逐像素比对。
fn mealybug_shot(rom_path: &str, out: Option<&str>) {
    if std::env::var("GB_TRACE").is_ok() { gb_emu::set_trace(true); }
    if let Ok(l) = std::env::var("GB_LINE") { gb_emu::set_dbg_line(l.parse().unwrap_or(255)); }
    let rom = std::fs::read(rom_path).expect("读不到 ROM 文件");
    let mut bus = Bus::new();
    bus.load_rom(&rom);
    let mut cpu = Cpu::new();
    println!("mealybug 开跑：{}（{} 字节），等 LD B,B 断点……", rom_path, rom.len());
    for _ in 0..2_000_000_000u64 {
        cpu.step(&mut bus);
        // 0x40(LD B,B) 首次执行 = 测试的软件断点（mooneye_result 只在 0x40 时被置位）
        if cpu.mooneye_result.is_some() {
            let path = out.unwrap_or("mealybug.raw");
            let mut raw = Vec::with_capacity(ppu::SCREEN_W * ppu::SCREEN_H);
            for y in 0..ppu::SCREEN_H {
                for x in 0..ppu::SCREEN_W {
                    raw.push(bus.ppu.framebuffer[y][x]);
                }
            }
            std::fs::write(path, &raw).expect("写 raw 失败");
            println!("断点命中，已 dump {} 字节色号到 {}", raw.len(), path);
            return;
        }
    }
    println!("跑满 20 亿条指令仍未见 LD B,B 断点，疑似卡死或测试异常");
}

// 把 framebuffer 存成 PPM 图片（GameBoy 四档灰度，放大4倍方便看）
fn save_ppm(path: &str, fb: &[[u8; ppu::SCREEN_W]; ppu::SCREEN_H]) {
    let scale = 4;
    let (w, h) = (ppu::SCREEN_W * scale, ppu::SCREEN_H * scale);
    let mut out = format!("P3\n{} {}\n255\n", w, h);
    let shades = [255, 170, 85, 0];   // 色号0=白 3=黑（真机灰度）
    for y in 0..ppu::SCREEN_H {
        for _sy in 0..scale {
            for x in 0..ppu::SCREEN_W {
                let c = shades[fb[y][x] as usize];
                for _sx in 0..scale { out.push_str(&format!("{c} {c} {c}\n")); }
            }
        }
    }
    std::fs::write(path, out).expect("写截图失败");
}

