use gba_cpu::{Bus, Cpu};

fn suite_result(index: usize, name: &str) -> (u32, u32) {
    let rom = std::fs::read("target/mgba-suite.gba").expect("mGBA 套件缺失，见 README");
    let bios = std::fs::read("target/cult-gba-bios.bin").expect("开源替代 BIOS 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    bus.load_bios(bios).unwrap();
    let mut cpu = Cpu::from_reset();
    for _ in 0..80 {
        cpu.run_frame(&mut bus).unwrap();
    }
    for _ in 0..index {
        press(&mut cpu, &mut bus, 1 << 7);
    }
    press(&mut cpu, &mut bus, 1);

    // 该 ROM 把字符编号放在 BG1 的地图中，可直接读取屏幕上的分数。
    for _ in 0..900 {
        cpu.run_frame(&mut bus).unwrap();
        let title: String = (0..30).map(|column| {
            let tile = bus.read_half(0x0600_0840 + 2 * column).unwrap() & 1023;
            char::from_u32(u32::from(tile) + 32).unwrap()
        }).collect();
        if let Some((passes, total)) = title.strip_prefix(name).and_then(|s| s.trim().split_once('/')) {
            return (passes.trim().parse().unwrap(), total.trim().parse().unwrap());
        }
    }
    panic!("{name} 未在预算内显示分数，PC={:08X}", cpu.pc());
}

fn press(cpu: &mut Cpu, bus: &mut Bus, key: u16) {
    bus.set_keys(key);
    cpu.run_frame(bus).unwrap();
    bus.set_keys(0);
    for _ in 0..3 {
        cpu.run_frame(bus).unwrap();
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_bitmap_background_vram_stalls() {
    for (mode, menu_down) in [(3, 2), (4, 3), (5, 4)] {
        let mut keys = vec![1 << 7; menu_down];
        keys.push(1);
        let stalls = vram_stall_positions(&keys);
        println!("Mode {mode}：测量 1231 个位置，记录 {} 个等待点", stalls.len());
        assert_eq!(stalls, (171..=1139).step_by(4).collect::<Vec<_>>(), "Mode {mode} 的等待位置有变化");
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_affine_background_vram_stalls() {
    for (label, option, phases) in [("BG2", 0, 0b1100), ("BG3", 1, 0b0011), ("BG2 + BG3", 2, 0b1111)] {
        let mut keys = vec![1 << 7, 1];
        keys.extend(std::iter::repeat_n(1 << 7, option));
        keys.push(1);
        let stalls = vram_stall_positions(&keys);
        println!("Mode 2 {label}：测量 1231 个位置，记录 {} 个等待点", stalls.len());
        let expected: Vec<_> = (168..1142).filter(|index| phases & (1 << (index % 4)) != 0).collect();
        assert_eq!(stalls, expected, "Mode 2 {label} 的等待位置有变化");
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_text_background_vram_stalls() {
    for (label, option, scroll, first_map, last_map, data_offsets) in [
        ("BG0 4bpp", 0, 0, 32, 992, &[4, 20][..]),
        ("BG1 8bpp", 5, 0, 33, 993, &[4, 12, 20, 28][..]),
        ("BG2 4bpp", 2, 5, 14, 1006, &[4, 20][..]),
        ("BG3 8bpp", 7, 5, 15, 975, &[4, 12, 20, 28][..]),
        ("BG0 4bpp", 0, 8, 32, 992, &[4, 20][..]),
    ] {
        let mut keys = vec![1];
        keys.extend(std::iter::repeat_n(1 << 7, option));
        keys.push(1);
        keys.extend(std::iter::repeat_n(1 << 4, scroll));
        keys.push(1);
        let stalls = vram_stall_positions(&keys);
        println!("Mode 0 {label} scroll={scroll}：测量 1231 个位置，记录 {} 个等待点", stalls.len());
        let mut expected = Vec::new();
        for map in (first_map..=last_map).step_by(32) {
            expected.push(map + 136);
            if map < 1004 {
                expected.extend(data_offsets.iter().map(|offset| map + offset + 136));
            }
        }
        assert_eq!(stalls.len(), expected.len(), "Mode 0 {label} scroll={scroll}");
        for (actual, expected) in stalls.iter().zip(&expected) {
            assert_eq!(actual, expected, "Mode 0 {label} scroll={scroll} 的等待位置");
        }
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_sprite_bus_stalls() {
    let oam: Vec<_> = [40, 42, 44, 58, 60, 74, 76].into_iter()
        .chain((90..=336).step_by(2)).map(|dot| dot + 136).collect();
    let vram: Vec<_> = (44..=90).step_by(2).map(|dot| dot + 136).collect();
    for (option, expected) in [&oam, &vram, &oam, &vram].into_iter().enumerate() {
        let mut keys = vec![1 << 7; 6];
        keys.push(1);
        keys.extend(std::iter::repeat_n(1 << 7, option));
        keys.push(1);
        let stalls = vram_stall_positions(&keys);
        println!("精灵测试 {option}：测量 1231 个位置，记录 {} 个等待点", stalls.len());
        assert_eq!(&stalls, expected, "精灵测试 {option}");
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_affine_and_clipped_sprite_stalls() {
    for option in [4, 5, 6, 7, 8, 9, 10, 11, 12, 13] {
        let mut keys = vec![1 << 7; 6];
        keys.push(1);
        keys.extend(std::iter::repeat_n(1 << 7, option));
        keys.push(1);
        let stalls = vram_stall_positions(&keys);
        println!("精灵测试 {option}：{} 个等待点，首尾 {:?} / {:?}", stalls.len(), stalls.first(), stalls.last());
        assert!(!stalls.is_empty(), "精灵测试 {option}");
        // 全精灵扫描会跨过行边界，探针在前一行末尾也能遇到争用。
        assert!(stalls.iter().all(|dot| dot % 2 == 0), "精灵测试 {option}");
        if option == 4 || option == 6 {
            assert_eq!(&stalls[..6], &[176, 178, 180, 182, 184, 186]);
            assert_eq!(stalls.len(), 143);
        }
        if option == 5 || option == 7 {
            assert_eq!(stalls.len(), if option == 5 { 48 } else { 96 });
        }
        if option == 9 {
            assert!(stalls.last().unwrap() < &(1004 + 136));
        }
        if option == 11 {
            assert!(stalls.last().unwrap() >= &(1006 + 136));
        }
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_display_control_latch() {
    // ROM 读到的计时包含 CPU 轮询。内部周期与 DMA 重叠也会改变后续传输的起始点。
    // 这些数值是当前模拟器的回归基线，逐拍显存争用由其他 ROM 用例单独校验。
    for (option, expected) in [
        (0, [275, 275, 414, 275]),
        (1, [275, 275, 411, 278]),
        (2, [527, 526, 527, 275]),
        (3, [526, 526, 527, 530]),
    ] {
        let mut keys = vec![1 << 7; 7];
        keys.push(1);
        keys.extend(std::iter::repeat_n(1 << 7, option));
        keys.push(1);
        let (mut cpu, mut bus) = start_video_timing_rom(&keys);
        for _ in 0..10 { cpu.run_frame(&mut bus).unwrap(); }
        let screen_block = (u32::from(bus.read_half(0x0400_0008).unwrap()) >> 8) & 31;
        let screen_map = 0x0600_0000 + screen_block * 0x800;
        let text: String = (0..30).map(|index| {
            let tile = bus.read_half(screen_map + 2 * index).unwrap() & 1023;
            char::from_u32(u32::from(tile)).unwrap()
        }).collect();
        let result: Vec<u16> = text.split_whitespace().map(|value| value.parse().unwrap()).collect();
        println!("显示控制测试 {option}：{result:?}");
        assert_eq!(result, expected);
    }
}

#[test]
#[ignore = "需下载公开显存时序测试 ROM 和开源替代 BIOS，见 README"]
fn hardware_rom_measures_palette_bus_stalls() {
    for (option, expected) in [
        (0, (183..=1139).step_by(4).collect::<Vec<_>>()),
        (9, Vec::new()),
        (10, (185..=1141).step_by(4).collect::<Vec<_>>()),
    ] {
        let mut keys = vec![1 << 7; 5];
        keys.push(1);
        keys.extend(std::iter::repeat_n(1 << 7, option));
        keys.push(1);
        let stalls = vram_stall_positions(&keys);
        println!("调色板测试 {option}：{} 个等待点，首尾 {:?} / {:?}", stalls.len(), stalls.first(), stalls.last());
        assert_eq!(stalls.len(), expected.len());
        assert_eq!(stalls, expected, "调色板测试 {option}");
    }
}

fn vram_stall_positions(keys: &[u16]) -> Vec<usize> {
    let (mut cpu, mut bus) = start_video_timing_rom(keys);
    for _ in 0..1400 {
        if bus.sram()[1230] <= 1 { break; }
        cpu.run_frame(&mut bus).unwrap();
    }
    let result = &bus.sram()[..1231];
    assert!(result.iter().all(|&bit| bit <= 1), "显存测试未导出完整结果，PC={:08X}", cpu.pc());
    // ROM 的横坐标从 IRQ 探针起算，比显示扫描线的拍号晚 136 拍。
    result.iter().enumerate().filter_map(|(index, &bit)| (bit == 1).then_some(index)).collect()
}

fn start_video_timing_rom(keys: &[u16]) -> (Cpu, Bus) {
    let rom = std::fs::read("target/ppu-ram-access-timing.gba").expect("显存测试 ROM 缺失，见 README");
    let bios = std::fs::read("target/cult-gba-bios.bin").expect("开源替代 BIOS 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    bus.load_bios(bios).unwrap();
    let mut cpu = Cpu::from_reset();
    for _ in 0..80 {
        cpu.run_frame(&mut bus).unwrap();
    }
    for &key in keys {
        press(&mut cpu, &mut bus, key);
    }
    (cpu, bus)
}

#[test]
#[ignore = "需下载公开测试 ROM 和开源替代 BIOS，见 README"]
fn mgba_instruction_groups_boot_from_reset_and_pass() {
    for (index, name, expected) in [
        (5, "Shifter tests", 140),
        (6, "Carry tests", 93),
        (7, "Multiply long tests", 72),
    ] {
        let actual = suite_result(index, name);
        assert_eq!(actual, (expected, expected), "{name}");
        println!("{name} {}/{}", actual.0, actual.1);
    }
}

#[test]
#[ignore = "需下载公开测试 ROM 和开源替代 BIOS，见 README"]
fn mgba_timing_score() {
    let (passes, total) = suite_result(2, "Timing tests");
    assert_eq!(total, 2020);
    println!("mGBA 时序分数 {passes}/{total}");
    assert!(passes >= 1834, "mGBA 时序分数低于已验证基线");
}

#[test]
#[ignore = "需下载公开测试 ROM 和开源替代 BIOS，见 README"]
fn mgba_memory_and_bios_scores() {
    for (index, name, minimum, expected_total) in [
        (0, "Memory tests", 1506, 1552),
        (8, "BIOS math tests", 603, 615),
    ] {
        let (passes, total) = suite_result(index, name);
        assert_eq!(total, expected_total);
        println!("{name} {passes}/{total}");
        assert!(passes >= minimum, "{name} 分数低于已验证基线");
    }
}
