use gba_cpu::{Bus, Cpu, RAM_START, WIDTH};

#[test]
fn writes_during_a_line_change_only_pixels_that_have_not_been_drawn() {
    let mut bus = Bus::new(&[]);
    let mut cpu = Cpu::new();
    cpu.regs[15] = 0x0300_0000;
    bus.write_word(cpu.pc(), 0xE1C1_00B0).unwrap(); // ARM STRH r0, [r1]。
    cpu.regs[0] = 16;
    cpu.regs[1] = 0x0400_0054;
    bus.write_half(0x0400_0000, 0x0404).unwrap();
    bus.write_half(0x0400_0050, 0x0084).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    for x in (0..WIDTH as u32).step_by(2) {
        bus.write_half(0x0600_0000 + x, 0x0101).unwrap();
    }

    bus.advance(320).unwrap();
    bus.write_byte(0x0500_0003, 0x7C).unwrap(); // 两字节复制，颜色此时变为紫色。
    bus.advance(158).unwrap(); // 片内取指和 I/O 写入各用一拍。
    cpu.step(&mut bus).unwrap(); // 第 120 像素开始提亮。
    bus.advance(522).unwrap(); // 可见像素已画完，仍未进入 HBlank。
    bus.write_byte(0x0400_0054, 0).unwrap();
    assert_eq!(&bus.scanout_rgb()[..3], &[0; 3]);
    bus.advance(4).unwrap();
    let frame = bus.scanout_rgb();
    assert!(frame[..80 * 3].chunks_exact(3).all(|rgb| rgb == [255, 0, 0]));
    assert!(frame[80 * 3..120 * 3].chunks_exact(3).all(|rgb| rgb == [231, 24, 255]));
    assert!(frame[120 * 3..WIDTH * 3].chunks_exact(3).all(|rgb| rgb == [255; 3]));
}

#[test]
fn hblank_dma_applies_scroll_to_the_next_line() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0100).unwrap();
    bus.write_half(0x0400_0008, 0x1800).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x03E0).unwrap();
    bus.write_half(0x0600_0004, 0x0022).unwrap();
    bus.write_half(0x0600_0020, 0x0011).unwrap();
    bus.write_half(0x0600_C002, 1).unwrap();
    bus.write_half(0x0400_0010, 8).unwrap();
    bus.write_half(RAM_START, 0).unwrap();
    bus.write_word(0x0400_00D4, RAM_START).unwrap();
    bus.write_word(0x0400_00D8, 0x0400_0010).unwrap();
    bus.write_word(0x0400_00DC, 0xA000_0001).unwrap();

    bus.advance(1006 + 1232).unwrap();
    let frame = bus.scanout_rgb();
    assert_eq!(&frame[..3], &[255, 0, 0]); // 第一行只用 DMA 之前的滚动值。
    assert_eq!(&frame[WIDTH * 3..(WIDTH + 1) * 3], &[0, 255, 0]);
}

#[test]
fn a_vram_word_write_retains_the_completed_pixel_of_the_pair() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0403).unwrap();
    bus.write_word(0x0600_0000, 0x001F_001F).unwrap();
    bus.advance(4).unwrap();
    bus.write_word(0x0600_0000, 0x03E0_03E0).unwrap();
    bus.advance(1002).unwrap();
    assert_eq!(&bus.scanout_rgb()[..6], &[255, 0, 0, 0, 255, 0]);
}

#[test]
fn midline_scroll_changes_the_tile_sample_without_repainting_the_prefix() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0100).unwrap();
    bus.write_half(0x0400_0008, 0x1800).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x03E0).unwrap();
    bus.write_word(0x0600_0000, 0x1111_1111).unwrap();
    bus.write_word(0x0600_0020, 0x2222_2222).unwrap();
    bus.write_half(0x0600_C002, 1).unwrap();
    bus.advance(8).unwrap();
    bus.write_half(0x0400_0010, 8).unwrap();
    bus.advance(998).unwrap();
    assert_eq!(&bus.scanout_rgb()[..12], &[255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0]);
}

#[test]
fn mode5_flips_pages_and_applies_the_bitmap_transform() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0405).unwrap();
    bus.write_half(0x0500_0000, 0x7C00).unwrap();
    bus.write_half(0x0600_0000, 0x001F).unwrap();
    bus.write_half(0x0600_A000, 0x03E0).unwrap();
    bus.write_half(0x0600_A140, 0x7FFF).unwrap(); // 第二页第 1 行第 0 列。
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..3], &[255, 0, 0]);
    assert_eq!(&frame[160 * 3..161 * 3], &[0, 0, 255]);
    assert_eq!(&frame[128 * WIDTH * 3..(128 * WIDTH + 1) * 3], &[0, 0, 255]);
    bus.write_byte(0x0400_0000, 0x15).unwrap(); // 字节切页保留高位的 BG2 开关。
    assert_eq!(&bus.mode3_rgb()[..3], &[0, 255, 0]);
    bus.write_word(0x0400_0024, 0x0000_0100).unwrap(); // x 前进时，改为沿位图纵向取样。
    bus.write_half(0x0400_0020, 0).unwrap();
    bus.advance(1006).unwrap();
    assert_eq!(&bus.scanout_rgb()[..6], &[0, 255, 0, 255, 255, 255]);
}

#[test]
fn green_swap_exchanges_only_the_green_channel_of_each_pixel_pair() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0400_0000, 0xFFFF_0403).unwrap();
    assert_eq!(bus.read_half(0x0400_0002).unwrap(), 1);
    bus.write_word(0x0600_0000, 0x7FE0_001F).unwrap(); // 红色、青色。
    assert_eq!(&bus.mode3_rgb()[..6], &[255, 255, 0, 0, 0, 255]); // 黄色、蓝色。
}

#[test]
fn segmented_green_swap_uses_both_pixels_of_a_split_pair() {
    let mut bus = Bus::new(&[]);
    bus.write_word(0x0400_0000, 0x0001_0403).unwrap();
    bus.write_word(0x0600_0000, 0x7FE0_001F).unwrap();
    bus.advance(4).unwrap();
    bus.write_word(0x0600_0000, 0x7C00_03E0).unwrap();
    bus.advance(1002).unwrap();
    // 每一段都按当时的颜色交换，再写入该段完成的像素。
    assert_eq!(&bus.scanout_rgb()[..6], &[255, 255, 0, 0, 255, 255]);
}

#[test]
fn obj_line_budget_shortens_the_last_sprite_when_hblank_is_reserved() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1060).unwrap();
    bus.write_half(0x0500_0202, 0x001F).unwrap();
    for sprite in 0..14 {
        bus.write_half(0x0700_0002 + sprite * 8, 0xC064).unwrap(); // 64 像素宽，首行透明。
    }
    bus.write_half(0x0700_0072, 0xC000).unwrap();
    bus.write_half(0x0700_0074, 8).unwrap();
    for tile in 8..16 {
        bus.write_word(0x0601_0000 + tile * 32, 0x1111_1111).unwrap();
    }
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[57 * 3..59 * 3], &[255, 0, 0, 0, 0, 0]);
    bus.advance(57 * 4).unwrap();
    bus.write_half(0x0500_0000, 0x03E0).unwrap();
    bus.advance(1006 - 57 * 4).unwrap();
    assert_eq!(&bus.scanout_rgb()[57 * 3..59 * 3], &[255, 0, 0, 0, 255, 0]);
    bus.write_half(0x0400_0000, 0x1040).unwrap();
    assert_eq!(&bus.mode3_rgb()[57 * 3..59 * 3], &[255, 0, 0, 255, 0, 0]);
}

#[test]
fn obj_8bpp_uses_the_full_palette_and_both_tile_strides() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1000).unwrap();
    bus.write_word(0x0700_0000, 0x4000_2080).unwrap(); // 16×16，放在第 128 行。
    bus.write_half(0x0700_0004, 0xF001).unwrap(); // 8bpp 忽略调色板组。
    bus.write_half(0x0500_0300, 0x001F).unwrap(); // 索引 128。
    bus.write_half(0x0500_0302, 0x03E0).unwrap();
    bus.write_half(0x0500_0304, 0x7C00).unwrap();
    bus.write_half(0x0601_0000, 128).unwrap();
    bus.write_half(0x0601_0040, 129).unwrap();
    bus.write_half(0x0601_0400, 130).unwrap();
    bus.write_half(0x0601_0020, 130).unwrap();
    bus.write_half(0x0601_0060, 128).unwrap();
    bus.write_half(0x0601_00A0, 129).unwrap();
    let positions = |bus: &Bus| {
        let frame = bus.mode3_rgb();
        [(0, 128), (8, 128), (0, 136)].map(|(x, y)| {
            <[u8; 3]>::try_from(&frame[(y * WIDTH + x) * 3..(y * WIDTH + x + 1) * 3]).unwrap()
        })
    };
    let (red, green, blue) = ([255, 0, 0], [0, 255, 0], [0, 0, 255]);
    assert_eq!(positions(&bus), [red, green, blue]);
    bus.write_half(0x0400_0000, 0x1040).unwrap();
    assert_eq!(positions(&bus), [blue, red, green]);
}

#[test]
fn affine_obj_window_rotates_and_supports_the_double_size_area() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x9403).unwrap();
    bus.write_half(0x0400_004A, 0x0400).unwrap();
    bus.write_word(0x0700_0000, 0x0200_2B00).unwrap(); // 8bpp 仿射窗口，倍宽倍高，矩阵编号 1。
    bus.write_half(0x0700_0004, 512).unwrap();
    bus.write_half(0x0700_002E, 0xFF00).unwrap(); // PB = -1。
    bus.write_half(0x0700_0036, 0x0100).unwrap(); // PC = +1。
    bus.write_half(0x0601_4028, 1).unwrap(); // 源图块 (0, 5)。
    bus.write_half(0x0600_0000 + (12 * WIDTH as u32 + 9) * 2, 0x001F).unwrap();
    let frame = bus.mode3_rgb();
    let offset = (12 * WIDTH + 9) * 3;
    assert_eq!(&frame[offset..offset + 3], &[255, 0, 0]);
    bus.write_half(0x0700_0000, 0x2900).unwrap(); // 普通大小的仿射区域裁掉该像素。
    assert_eq!(&bus.mode3_rgb()[offset..offset + 3], &[0; 3]);
}

#[test]
fn a_tall_double_size_sprite_wrapping_vertically_only_appears_at_the_top() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1000).unwrap();
    bus.write_half(0x0500_0202, 0x001F).unwrap();
    bus.write_word(0x0700_0000, 0xC000_038C).unwrap(); // 64×64，倍宽倍高，y=140。
    bus.write_half(0x0601_1080, 1).unwrap(); // 零矩阵固定取中心。
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..3], &[255, 0, 0]);
    assert_eq!(&frame[140 * WIDTH * 3..(140 * WIDTH + 1) * 3], &[0; 3]);
}

#[test]
fn mosaic_repeats_the_first_sample_of_each_background_and_sprite_block() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1403).unwrap();
    bus.write_half(0x0400_000C, 0x0040).unwrap();
    bus.write_byte(0x0400_004C, 0x11).unwrap();
    bus.write_byte(0x0400_004D, 0x11).unwrap();
    bus.write_half(0x0600_0000, 0x001F).unwrap();
    bus.write_half(0x0600_0002, 0x03E0).unwrap();
    bus.write_half(0x0600_01E0, 0x7FFF).unwrap();
    bus.write_word(0x0700_0000, 0x0004_1000).unwrap();
    bus.write_half(0x0700_0004, 512).unwrap();
    bus.write_half(0x0500_0202, 0x7C00).unwrap();
    bus.write_half(0x0601_4000, 1).unwrap();
    bus.advance(1006 + 1232).unwrap();
    for y in 0..2 {
        let row = &bus.scanout_rgb()[y * WIDTH * 3..(y + 1) * WIDTH * 3];
        assert_eq!(&row[..6], &[255, 0, 0, 255, 0, 0]);
        assert_eq!(&row[12..18], &[0, 0, 255, 0, 0, 255]);
    }
}

#[test]
#[ignore = "需从 Tonc 下载演示 ROM，见 README"]
fn tonc_obj_aff_rotates_the_sprite_with_the_r_button() {
    let rom = std::fs::read("target/bin/obj_aff.gba").expect("测试 ROM 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    let mut cpu = Cpu::new();
    for _ in 0..8 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let initial = bus.scanout_rgb().to_vec();
    let matrix_b = bus.read_half(0x0700_000E).unwrap();
    bus.set_keys(1 << 8);
    for _ in 0..32 {
        cpu.run_frame(&mut bus)
            .unwrap_or_else(|err| panic!("按 R 时 PC {:#010X}: {err:?}", cpu.pc()));
    }
    assert_ne!(matrix_b, bus.read_half(0x0700_000E).unwrap());
    let changed = changed_pixels_in(&initial, bus.scanout_rgb(), 96..160, 32..96);
    assert!(changed > 100, "旋转后的变化像素：{changed}");
    println!("按 R 后精灵区域的变化像素：{changed}");
}

#[test]
#[ignore = "需从 Tonc 下载演示 ROM，见 README"]
fn tonc_mosaic_demo_adjusts_background_and_sprite_independently() {
    let rom = std::fs::read("target/bin/mos_demo.gba").expect("测试 ROM 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    let mut cpu = Cpu::new();
    for _ in 0..8 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let initial = bus.scanout_rgb().to_vec();
    bus.set_keys(1 << 4);
    for _ in 0..40 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let sprite_only = bus.scanout_rgb().to_vec();
    let sprite_changed = changed_pixels_in(&initial, &sprite_only, 24..104, 24..96);
    assert!(sprite_changed > 100);
    assert_eq!(changed_pixels_in(&initial, &sprite_only, 136..216, 24..96), 0);
    bus.set_keys((1 << 4) | 1); // A + 右：调整背景。
    for _ in 0..40 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let final_frame = bus.scanout_rgb();
    let bg_changed = changed_pixels_in(&sprite_only, final_frame, 136..216, 24..96);
    assert!(bg_changed > 100);
    assert_eq!(changed_pixels_in(&sprite_only, final_frame, 24..104, 24..96), 0);
    println!("马赛克变化像素：精灵 {sprite_changed}，背景 {bg_changed}");
}

fn changed_pixels_in(
    before: &[u8],
    after: &[u8],
    columns: std::ops::Range<usize>,
    rows: std::ops::Range<usize>,
) -> usize {
    rows.map(|y| {
        let start = (y * WIDTH + columns.start) * 3;
        let end = (y * WIDTH + columns.end) * 3;
        before[start..end].chunks_exact(3).zip(after[start..end].chunks_exact(3))
            .filter(|(a, b)| a != b)
            .count()
    }).sum()
}

#[test]
fn obj_window_uses_the_flipped_tile_shape_and_rectangles_take_priority() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0xF403).unwrap();
    bus.write_half(0x0500_0000, 0x7C00).unwrap(); // 蓝色底色。
    bus.write_half(0x0500_0202, 0x03E0).unwrap(); // 窗口精灵的绿色调色板。
    for x in 0..8 {
        bus.write_half(0x0600_0000 + x * 2, 0x001F).unwrap();
    }
    bus.write_word(0x0601_4000, 0x0101_0100).unwrap(); // 翻转后 x=1、3、5 在窗口内。
    bus.write_word(0x0700_0000, 0x1000_0800).unwrap();
    bus.write_half(0x0700_0004, 0x0E00).unwrap(); // OBJ 窗口忽略精灵绘图优先级。
    bus.write_word(0x0400_0040, 0x0306_0506).unwrap(); // WIN0 遮住 x=5，WIN1 遮住 x=3..5。
    bus.write_word(0x0400_0044, 0x0001_0001).unwrap();
    bus.write_word(0x0400_0048, 0x2400_0004).unwrap();
    bus.write_half(0x0400_0050, 0x0084).unwrap();
    bus.write_half(0x0400_0054, 16).unwrap();
    bus.advance(1006).unwrap();
    let frame = bus.scanout_rgb();
    let (blue, white, red) = ([0, 0, 255], [255, 255, 255], [255, 0, 0]);
    assert_eq!(frame[..21], [blue, white, blue, blue, blue, red, blue].concat());
}

#[test]
fn obj_window_ignores_palette_color_and_uses_the_union_of_enabled_sprites() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x9100).unwrap();
    bus.write_half(0x0400_0008, 0x1800).unwrap();
    bus.write_half(0x0400_004A, 0x0100).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_word(0x0600_0000, 0x1111_1111).unwrap();
    bus.write_half(0x0601_0020, 0x0001).unwrap();
    bus.write_word(0x0700_0000, 0x0000_0800).unwrap();
    bus.write_half(0x0700_0004, 1).unwrap();
    bus.write_word(0x0700_0008, 0x0002_0800).unwrap();
    bus.write_half(0x0700_000C, 1).unwrap();
    bus.write_word(0x0700_0010, 0x0003_0A00).unwrap(); // 已禁用。
    bus.write_half(0x0700_0014, 1).unwrap();
    assert_eq!(&bus.mode3_rgb()[..12], &[255, 0, 0, 0, 0, 0, 255, 0, 0, 0, 0, 0]);
    bus.write_half(0x0400_0000, 0x8100).unwrap(); // OBJ 全局关闭。
    assert_eq!(&bus.mode3_rgb()[..12], &[0; 12]);
}

#[test]
fn alpha_blends_only_the_next_visible_layer_and_obeys_the_window() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x2700).unwrap();
    bus.write_word(0x0400_0008, 0x1901_1800).unwrap();
    bus.write_half(0x0400_000C, 0x1A02).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x7C00).unwrap();
    bus.write_half(0x0500_0006, 0x03E0).unwrap();
    bus.write_word(0x0600_0020, 0x1111_1111).unwrap();
    bus.write_half(0x0600_0040, 0x0020).unwrap(); // BG1 只挡住 x=1。
    bus.write_word(0x0600_0060, 0x3333_3333).unwrap();
    bus.write_half(0x0600_C000, 1).unwrap();
    bus.write_half(0x0600_C800, 2).unwrap();
    bus.write_half(0x0600_D000, 3).unwrap();
    bus.write_half(0x0400_0040, 2).unwrap();
    bus.write_half(0x0400_0044, 1).unwrap();
    bus.write_word(0x0400_0048, 0x0007_0027).unwrap();
    bus.write_word(0x0400_0050, 0x08E8_0441).unwrap();
    assert_eq!(bus.read_half(0x0400_0052).unwrap(), 0x0808);
    assert_eq!(&bus.mode3_rgb()[..9], &[123, 123, 0, 255, 0, 0, 255, 0, 0]);
    bus.write_byte(0x0400_0053, 31).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[123, 255, 0]);
}

#[test]
fn semi_transparent_obj_blends_with_the_background_and_not_another_obj() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1403).unwrap();
    bus.write_half(0x0500_0202, 0x001F).unwrap();
    bus.write_half(0x0500_0204, 0x7C00).unwrap();
    bus.write_half(0x0600_0000, 0x03E0).unwrap();
    bus.write_half(0x0601_4000, 1).unwrap();
    bus.write_half(0x0601_4020, 2).unwrap();
    bus.write_half(0x0700_0000, 0x0400).unwrap();
    bus.write_half(0x0700_0004, 512).unwrap();
    bus.write_half(0x0700_000C, 513).unwrap();
    bus.write_word(0x0400_0050, 0x0808_1400).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[123, 123, 0]);
    bus.write_half(0x0400_0050, 0x1000).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[255, 0, 0]);
}

#[test]
#[ignore = "需从 Tonc 下载演示 ROM，见 README"]
fn tonc_bld_demo_changes_sprite_pixels_when_the_blend_weights_change() {
    let rom = std::fs::read("target/bin/bld_demo.gba").expect("测试 ROM 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    let mut cpu = Cpu::new();
    let run = |cpu: &mut Cpu, bus: &mut Bus, frames| {
        for _ in 0..frames {
            cpu.run_frame(bus)
                .unwrap_or_else(|err| panic!("PC {:#010X}: {err:?}", cpu.pc()));
        }
    };
    run(&mut cpu, &mut bus, 8);
    bus.set_keys(1 << 8); // R：切换到混色模式。
    run(&mut cpu, &mut bus, 2);
    assert_eq!((bus.read_half(0x0400_0050).unwrap() >> 6) & 3, 1);
    assert_eq!(bus.read_half(0x0400_0052).unwrap(), 0x0010);
    let before = bus.scanout_rgb().to_vec();
    bus.set_keys((1 << 5) | (1 << 6)); // 左、上：降低前层权重，提高后层权重。
    run(&mut cpu, &mut bus, 64);
    let weights = bus.read_half(0x0400_0052).unwrap();
    assert!((7..=9).contains(&(weights & 31)) && (7..=9).contains(&(weights >> 8)));
    let after = bus.scanout_rgb();
    let changed = (24..88).map(|y| {
        let start = (y * WIDTH + 32) * 3;
        let end = start + 64 * 3;
        before[start..end].chunks_exact(3).zip(after[start..end].chunks_exact(3))
            .filter(|(a, b)| a != b)
            .count()
    }).sum::<usize>();
    assert!(changed > 100);
    println!("混色权重：前层 {}，后层 {}，精灵区域变化像素：{changed}", weights & 31, weights >> 8);
}

#[test]
#[ignore = "需从 Tonc 下载演示 ROM，见 README"]
fn tonc_sbb_aff_boots_and_rotates_with_the_r_button() {
    let rom = std::fs::read("target/bin/sbb_aff.gba").expect("测试 ROM 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    let mut cpu = Cpu::new();
    for _ in 0..8 {
        cpu.run_frame(&mut bus)
            .unwrap_or_else(|err| panic!("启动时 PC {:#010X}: {err:?}", cpu.pc()));
    }
    assert_eq!(bus.read_half(0x0400_0000).unwrap() & 0x0F07, 0x0501);

    // 取说明框上方的地图区域，排除说明文字的变化。
    let map_size = 100 * WIDTH * 3;
    let initial_map = bus.scanout_rgb()[..map_size].to_vec();
    for _ in 0..4 {
        cpu.run_frame(&mut bus).unwrap();
    }
    assert_eq!(bus.scanout_rgb()[..map_size], initial_map);
    bus.set_keys(1 << 8); // R 肩键。
    for _ in 0..32 {
        cpu.run_frame(&mut bus)
            .unwrap_or_else(|err| panic!("按 R 时 PC {:#010X}: {err:?}", cpu.pc()));
    }
    let rotated_map = &bus.scanout_rgb()[..map_size];
    let colored = |rgb: &[u8]| rgb[0] != rgb[1] || rgb[1] != rgb[2];
    assert!(initial_map.chunks_exact(3).filter(|rgb| colored(rgb)).count() > 1_000);
    assert!(rotated_map.chunks_exact(3).filter(|rgb| colored(rgb)).count() > 1_000);
    let changed_pixels = initial_map.chunks_exact(3).zip(rotated_map.chunks_exact(3))
        .filter(|(before, after)| before != after && (colored(before) || colored(after)))
        .count();
    assert!(changed_pixels > 1_000, "彩色地图像素变化：{changed_pixels}");
    println!("按 R 后变化的彩色地图像素：{changed_pixels}");
}

#[test]
fn brightness_respects_the_window_and_the_visible_layer() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x3403).unwrap(); // Mode 3、BG2、OBJ、WIN0。
    bus.write_half(0x0400_0040, 3).unwrap();
    bus.write_half(0x0400_0044, 1).unwrap();
    bus.write_word(0x0400_0048, 0x0014_0034).unwrap(); // 仅窗口内启用特效。
    bus.write_half(0x0500_0202, 0x7C00).unwrap();
    bus.write_half(0x0700_0002, 1).unwrap();
    bus.write_half(0x0700_0004, 512).unwrap();
    bus.write_half(0x0601_4000, 1).unwrap();
    for x in 0..4 {
        bus.write_half(0x0600_0000 + 2 * x, 0x001F).unwrap();
    }
    bus.write_half(0x0400_0050, 0xFFC4).unwrap(); // 仅对最上层 BG2 像素压暗。
    assert_eq!(bus.read_half(0x0400_0050).unwrap(), 0x3FC4);
    bus.write_byte(0x0400_0054, 8).unwrap();
    bus.advance(1006).unwrap();
    assert_eq!(&bus.scanout_rgb()[..12], &[132, 0, 0, 0, 0, 255, 132, 0, 0, 255, 0, 0]);
}

#[test]
fn brightness_adjusts_backdrop_and_clamps_the_coefficient() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0500_0000, 0x001F).unwrap();
    bus.write_half(0x0400_0050, 0x00A0).unwrap();
    bus.write_half(0x0400_0054, 8).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[255, 123, 123]);
    bus.write_word(0x0400_0054, 31).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[255, 255, 255]);
    bus.write_byte(0x0400_0050, 0xE0).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[0, 0, 0]);
    bus.write_half(0x0400_0054, 0).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[255, 0, 0]);
    assert!(bus.read_half(0x0400_0054).is_err());
}

#[test]
fn arm_program_paints_two_mode3_pixels() {
    let mut bus = Bus::new(&[0xE5830000, 0xE5841000]); // 两次 STR
    let mut cpu = Cpu::new();
    cpu.regs[0] = 0x0403; // Mode 3 + BG2
    cpu.regs[1] = 0x7C00_001F; // 蓝色和红色
    cpu.regs[3] = 0x0400_0000;
    cpu.regs[4] = 0x0600_0000;
    assert_eq!(bus.mode3_rgb()[..6], [0; 6]);
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    let frame = bus.mode3_rgb();
    assert_eq!(frame.len(), 240 * 160 * 3);
    assert_eq!(frame[..9], [255, 0, 0, 0, 0, 255, 0, 0, 0]);
}

#[test]
fn mode4_switches_framebuffer_pages_and_uses_palette_indices() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x7C00).unwrap();
    bus.write_half(0x0600_0000, 0x0201).unwrap();
    bus.write_half(0x0600_A000, 0x0102).unwrap();
    bus.write_half(0x0400_0000, 0x0404).unwrap();
    assert_eq!(&bus.mode3_rgb()[..6], &[255, 0, 0, 0, 0, 255]);
    bus.write_half(0x0400_0000, 0x0414).unwrap();
    assert_eq!(&bus.mode3_rgb()[..6], &[0, 0, 255, 255, 0, 0]);
}

#[test]
fn mode0_scrolls_across_four_screenblocks_with_wrapping_and_flips() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0008, 0xD800).unwrap(); // 512×512，地图从第 24 块开始。
    bus.write_half(0x0400_0000, 0x0100).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0022, 0x03E0).unwrap();
    bus.write_half(0x0500_0042, 0x7C00).unwrap();
    bus.write_half(0x0500_0062, 0x7FFF).unwrap();
    bus.write_half(0x0600_0020, 0x0001).unwrap(); // 仅图块左上角有颜色。
    bus.write_half(0x0600_C7FE, 0x0C01).unwrap();
    bus.write_half(0x0600_CFC0, 0x1801).unwrap();
    bus.write_half(0x0600_D03E, 0x2401).unwrap();
    bus.write_half(0x0600_D800, 0x3001).unwrap();
    bus.write_word(0x0400_0010, 0x00FF_00FF).unwrap();
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..6], &[255, 0, 0, 0, 255, 0]);
    assert_eq!(&frame[720..726], &[0, 0, 255, 255, 255, 255]);

    bus.write_byte(0x0400_0010, 255).unwrap();
    bus.write_byte(0x0400_0011, 1).unwrap();
    bus.write_half(0x0400_0012, 512).unwrap();
    bus.write_half(0x0600_C000, 0x0001).unwrap();
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..6], &[0, 0, 0, 255, 0, 0]);
}

#[test]
fn mode0_combines_backgrounds_backdrop_and_sprites_by_priority() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x1300).unwrap();
    bus.write_half(0x0400_0008, 0x1801).unwrap();
    bus.write_half(0x0400_000A, 0x1985).unwrap(); // 8bpp，图块从第 1 块读取。
    assert_eq!(bus.read_half(0x0400_000A).unwrap(), 0x1985);
    bus.write_half(0x0500_0000, 0x7FFF).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x03E0).unwrap();
    bus.write_half(0x0500_0202, 0x7C00).unwrap();
    bus.write_half(0x0600_0020, 0x0001).unwrap();
    bus.write_half(0x0600_4040, 0x0202).unwrap();
    bus.write_half(0x0600_C000, 1).unwrap();
    bus.write_half(0x0600_C800, 0xF001).unwrap(); // 8bpp 忽略调色板组。
    bus.write_half(0x0601_0020, 0x0011).unwrap();
    bus.write_half(0x0700_0004, 0x0801).unwrap();
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..9], &[255, 0, 0, 0, 255, 0, 255, 255, 255]);

    bus.write_half(0x0700_0004, 0x0401).unwrap();
    assert_eq!(&bus.mode3_rgb()[..6], &[0, 0, 255, 0, 0, 255]);
    bus.write_half(0x0400_0000, 0x0300).unwrap();
    bus.write_half(0x0400_000A, 0x1984).unwrap();
    assert_eq!(&bus.mode3_rgb()[..3], &[0, 255, 0]);
}

#[test]
fn mode1_rotates_and_scales_an_affine_map_and_wraps_negative_coordinates() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0501).unwrap(); // 普通 BG0 和仿射 BG2。
    bus.write_half(0x0400_0008, 0x1904).unwrap();
    bus.write_half(0x0400_000C, 0x1800).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap(); // 红。
    bus.write_half(0x0500_0004, 0x03E0).unwrap(); // 绿。
    bus.write_half(0x0500_0006, 0x7C00).unwrap(); // 蓝。
    bus.write_half(0x0600_C000, 1).unwrap();
    bus.write_half(0x0600_C00E, 0x0200).unwrap(); // 地图第 15 列为图块 2。
    bus.write_half(0x0600_4040, 0x3333).unwrap(); // BG0 左上角为蓝色。
    bus.write_half(0x0600_C800, 2).unwrap();
    bus.write_half(0x0600_0040, 3).unwrap();
    bus.write_half(0x0600_0042, 1).unwrap();
    bus.write_half(0x0600_0052, 2).unwrap();
    bus.write_half(0x0600_0086, 0x0100).unwrap();
    bus.write_word(0x0400_0020, 0xFE00_0000).unwrap(); // 每到下一行，图块横坐标减 2。
    bus.write_word(0x0400_0024, 0x0000_0200).unwrap(); // 每个屏幕像素，图块纵坐标加 2。
    bus.write_word(0x0400_0028, 0x200).unwrap();
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..9], &[0, 0, 255, 0, 0, 255, 0, 0, 255]);
    bus.write_half(0x0400_0000, 0x0401).unwrap();
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..6], &[255, 0, 0, 0, 255, 0]);
    assert_eq!(&frame[WIDTH * 3..WIDTH * 3 + 3], &[0, 0, 255]);

    bus.write_word(0x0400_0028, 0xFFFF_FF00).unwrap(); // -1，位 28–31 不参与坐标。
    let frame = bus.mode3_rgb();
    assert_eq!(&frame[..3], &[0, 0, 0]);
    bus.write_byte(0x0400_000D, 0x38).unwrap(); // 打开回绕。
    assert_eq!(&bus.mode3_rgb()[..3], &[255, 0, 0]);
}

#[test]
fn mode2_layers_two_affine_backgrounds_and_respects_transparency() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0C02).unwrap();
    bus.write_word(0x0400_000C, 0x1901_1801).unwrap();
    bus.write_half(0x0400_0020, 0x100).unwrap();
    bus.write_half(0x0400_0030, 0x100).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x03E0).unwrap();
    bus.write_half(0x0600_C000, 1).unwrap();
    bus.write_half(0x0600_C800, 2).unwrap();
    bus.write_half(0x0600_0040, 1).unwrap();
    bus.write_half(0x0600_0080, 0x0202).unwrap();
    assert_eq!(&bus.mode3_rgb()[..6], &[255, 0, 0, 0, 255, 0]);
    bus.write_half(0x0400_000E, 0x1900).unwrap();
    assert_eq!(&bus.mode3_rgb()[..6], &[0, 255, 0, 0, 255, 0]);
}

#[test]
fn affine_reference_writes_redirect_the_next_scanline_and_reload_each_frame() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x0402).unwrap();
    bus.write_half(0x0400_000C, 0x1800).unwrap();
    bus.write_half(0x0400_0026, 0x100).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x03E0).unwrap();
    bus.write_half(0x0500_0006, 0x7C00).unwrap();
    bus.write_half(0x0600_0000, 1).unwrap();
    bus.write_half(0x0600_0008, 2).unwrap();
    bus.write_half(0x0600_0010, 3).unwrap();
    bus.advance(1006 + 1232).unwrap();
    assert_eq!(&bus.scanout_rgb()[..3], &[255, 0, 0]);
    assert_eq!(&bus.scanout_rgb()[WIDTH * 3..WIDTH * 3 + 3], &[0, 255, 0]);
    bus.write_half(0x0400_002C, 0x200).unwrap();
    bus.advance(1232).unwrap();
    assert_eq!(&bus.scanout_rgb()[2 * WIDTH * 3..2 * WIDTH * 3 + 3], &[0, 0, 255]);
    bus.advance(226 * 1232).unwrap();
    assert_eq!(&bus.scanout_rgb()[..3], &[0, 0, 255]);
}

#[test]
#[ignore = "需从 Tonc 下载演示 ROM，见 README"]
fn tonc_sbb_reg_boots_and_scrolls_into_the_other_three_screenblocks() {
    let rom = std::fs::read("target/bin/sbb_reg.gba").expect("测试 ROM 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    let mut cpu = Cpu::new();
    let colors = [[255, 0, 0], [0, 255, 0], [0, 0, 255], [132, 132, 132]];
    let counts = |bus: &Bus| {
        let mut counts = [0; 4];
        for rgb in bus.scanout_rgb().chunks_exact(3) {
            if let Some(i) = colors.iter().position(|color| color == rgb) {
                counts[i] += 1;
            }
        }
        counts
    };
    for _ in 0..12 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let initial = counts(&bus);
    assert!(initial[0] > 10_000);
    assert_eq!(initial[1..], [0; 3]);

    // 连按左上，视野从地图原点绕到对角，四个地图块应同时可见。
    bus.set_keys((1 << 5) | (1 << 6));
    for _ in 0..40 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let scrolled = counts(&bus);
    assert!(scrolled.into_iter().all(|count| count > 300), "{scrolled:?}");
    println!("红、绿、蓝、灰像素数：静止 {initial:?}，按左上 {scrolled:?}");
}

#[test]
fn arm_cartridge_rotates_mode2_in_response_to_the_shoulder_button() {
    let mut bus = Bus::new(&[
        // r0、r1、r2 分别指向 I/O、调色板、显存。
        0xE3A00301, 0xE3A01405, 0xE3A02406,
        // 开启 Mode 2 BG2，地图放在显存第 24 个地图块。
        0xE3A03B01, 0xE3833002, 0xE1C030B0, 0xE3A03B06, 0xE1C030BC,
        // 颜色 1 为红，2 为绿。图块右侧像素为红，下方像素为绿。
        0xE3A0301F, 0xE1C130B2, 0xE3A03E3E, 0xE1C130B4,
        0xE3A03C01, 0xE1C230B0, 0xE3A03002, 0xE1C230B8,
        // 循环读取 KEYINPUT。有 R 键时写入旋转 90° 的矩阵，松开时写回单位矩阵。
        0xE2801E13, 0xE1D130B0, 0xE3130C01, 0x13A03C01, 0x03A034FF,
        0xE5803020, 0x13A03401, 0x03A03C01, 0xE5803024, 0xEAFFFFF6,
    ]);
    let mut cpu = Cpu::new();
    cpu.run_frame(&mut bus).unwrap();
    // 卡带在首行开始后才写入图块，下一帧才从 x=1 显示完整画面。
    assert_eq!(&bus.scanout_rgb()[3..6], &[0; 3]);
    cpu.run_frame(&mut bus).unwrap();
    assert_eq!(&bus.scanout_rgb()[3..6], &[255, 0, 0]);
    bus.set_keys(1 << 8);
    cpu.run_frame(&mut bus).unwrap();
    assert_eq!(&bus.scanout_rgb()[3..6], &[0, 255, 0]);
    bus.set_keys(0);
    cpu.run_frame(&mut bus).unwrap();
    assert_eq!(&bus.scanout_rgb()[3..6], &[255, 0, 0]);
}

#[test]
fn rectangular_windows_select_layers_and_win0_wins_the_overlap() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x7300).unwrap();
    bus.write_word(0x0400_0008, 0x1900_1800).unwrap();
    bus.write_half(0x0500_0002, 0x001F).unwrap();
    bus.write_half(0x0500_0004, 0x03E0).unwrap();
    bus.write_half(0x0500_0202, 0x7C00).unwrap();
    bus.write_word(0x0600_0020, 0x1111_1111).unwrap();
    bus.write_word(0x0600_0024, 0x1111_1111).unwrap();
    bus.write_word(0x0600_0040, 0x2222_2222).unwrap();
    bus.write_word(0x0600_C000, 0x0001_0001).unwrap();
    bus.write_word(0x0600_C800, 0x0002_0002).unwrap();
    bus.write_word(0x0601_0020, 0x1111_1111).unwrap();
    bus.write_half(0x0700_0004, 1).unwrap();
    bus.write_word(0x0400_0040, 0x0105_0307).unwrap(); // WIN0 x=3..7，WIN1 x=1..5。
    bus.write_word(0x0400_0044, 0x0002_0001).unwrap(); // WIN0 y=0，WIN1 y=0..2。
    bus.write_word(0x0400_0048, 0x0002_FF10).unwrap(); // WIN0 只显示 OBJ，WIN1 只显示 BG0。
    bus.write_byte(0x0400_0049, 1).unwrap();
    assert_eq!(bus.read_half(0x0400_0048).unwrap(), 0x0110);
    let frame = bus.mode3_rgb();
    let pixel = |x, y| &frame[(y * WIDTH + x) * 3..(y * WIDTH + x + 1) * 3];
    assert_eq!(pixel(0, 0), [0, 255, 0]);
    assert_eq!(pixel(1, 0), [255, 0, 0]);
    assert_eq!(pixel(3, 0), [0, 0, 255]); // 重叠时 WIN0 生效。
    assert_eq!(pixel(6, 0), [0, 0, 255]);
    assert_eq!(pixel(7, 0), [0, 255, 0]); // 右边界不包含在窗口内。
    assert_eq!(pixel(3, 1), [255, 0, 0]); // 下边界不包含在 WIN0 内。
}

#[test]
fn rectangular_window_wraps_and_masks_the_bitmap_on_a_captured_line() {
    let mut bus = Bus::new(&[]);
    bus.write_half(0x0400_0000, 0x2403).unwrap();
    bus.write_half(0x0500_0000, 0x001F).unwrap();
    for x in [0, 1, 2, 237, 238, 239] {
        bus.write_half(0x0600_0000 + x * 2, 0x03E0).unwrap();
    }
    bus.write_half(0x0400_0040, 0xEE02).unwrap(); // x=238 起，越过右边缘后包含 x=0、1。
    bus.write_half(0x0400_0044, 1).unwrap();
    bus.write_byte(0x0400_0048, 4).unwrap();
    bus.advance(1006).unwrap();
    let pixels = bus.scanout_rgb();
    assert_eq!(&pixels[..9], &[0, 255, 0, 0, 255, 0, 255, 0, 0]);
    assert_eq!(&pixels[711..720], &[255, 0, 0, 0, 255, 0, 0, 255, 0]);
}

#[test]
#[ignore = "需从 Tonc 下载演示 ROM，见 README"]
fn tonc_win_demo_moves_the_xray_window_with_the_dpad() {
    let rom = std::fs::read("target/bin/win_demo.gba").expect("测试 ROM 缺失，见 README");
    let mut bus = Bus::from_rom(rom).unwrap();
    let mut cpu = Cpu::new();
    for _ in 0..12 {
        cpu.run_frame(&mut bus).unwrap();
    }
    assert_eq!(bus.read_half(0x0400_0048).unwrap() & 0x1F1F, 0x0110);
    let before = bus.scanout_rgb().to_vec();
    let black_run = |frame: &[u8]| {
        let row = &frame[25 * WIDTH * 3..26 * WIDTH * 3];
        // 透视窗口只显示精灵。火箭外侧是一段连续的黑色底色。
        let start = (12..200).find(|&x| row[x * 3..(x + 24) * 3].iter().all(|&v| v == 0));
        start.expect("未找到透视窗口")
    };
    bus.set_keys(1 << 4);
    // 演示在等待 VBlank 前读键，第一帧可能已读过上一次输入。
    for _ in 0..17 {
        cpu.run_frame(&mut bus).unwrap();
    }
    let after = bus.scanout_rgb();
    let (left_before, left_after) = (black_run(&before), black_run(after));
    assert_eq!(left_before, 36);
    assert!((16..=17).contains(&(left_after - left_before)));
    assert_eq!(&before[..WIDTH * 3], &after[..WIDTH * 3]);
    println!("透视窗左边界：{left_before} → {left_after}");
}
