use gba_cpu::{Bus, Cpu, HEIGHT, WIDTH};

fn run(rom: &[u8], direction: u16) -> (usize, Vec<u8>) {
    let mut bus = Bus::from_rom(rom.to_vec()).unwrap();
    let mut cpu = Cpu::new();
    for frame in 0..230 {
        // 从菜单按 Start。等开球倒计时结束，再按方向键。
        let keys = match frame {
            12..=15 => 1 << 3,
            210.. => direction,
            _ => 0,
        };
        bus.set_keys(keys);
        cpu.run_frame(&mut bus)
            .unwrap_or_else(|err| panic!("第 {frame} 帧，PC {:#010X}: {err:?}", cpu.pc()));
    }
    let pixels = bus.scanout_rgb();
    assert!(pixels.chunks_exact(3).any(|rgb| rgb == [0, 255, 0]), "画面缺少绿球");
    // x=4 穿过玩家球拍，取该列蓝色像素的首行。
    let paddle: Vec<_> = (0..HEIGHT)
        .filter(|&y| {
            let i = (y * WIDTH + 4) * 3;
            pixels[i..i + 3] == [0, 0, 255]
        })
        .collect();
    assert_eq!(paddle.len(), 24);
    assert_eq!(paddle[23], paddle[0] + 23);
    (paddle[0], pixels.to_vec())
}

#[test]
#[ignore = "需从作者仓库下载测试 ROM，见 README"]
fn third_party_pong_boots_and_responds_to_input() {
    let rom = std::fs::read("target/pong-homebrew.gba").expect("测试 ROM 缺失，见 README");
    let (rest, screen) = run(&rom, 0);
    let (up, _) = run(&rom, 1 << 6);
    let (down, _) = run(&rom, 1 << 7);
    assert_eq!((rest, up, down), (68, 28, 108));
    println!("玩家球拍顶部：无输入 {rest}，按上 {up}，按下 {down}");
    let mut ppm = b"P6\n240 160\n255\n".to_vec();
    ppm.extend(screen);
    std::fs::write("target/pong-verified.ppm", ppm).unwrap();
}
