use gba_cpu::square::{DOWN, LEFT, RIGHT, SquareDemo, UP};
use gba_cpu::{HEIGHT, WIDTH};
use minifb::{Key, Scale, Window, WindowOptions};

fn main() -> Result<(), String> {
    let mut demo = SquareDemo::new().map_err(|e| format!("{e:?}"))?;
    let mut window = Window::new(
        "GBA 方块演示 | 方向键移动 | Esc 退出",
        WIDTH,
        HEIGHT,
        WindowOptions { scale: Scale::X2, ..WindowOptions::default() },
    ).map_err(|e| e.to_string())?;
    window.set_target_fps(60);
    let mut pixels = vec![0; WIDTH * HEIGHT];
    while window.is_open() && !window.is_key_down(Key::Escape) {
        let mut keys = 0;
        for (key, bit) in [(Key::Right, RIGHT), (Key::Left, LEFT), (Key::Up, UP), (Key::Down, DOWN)] {
            if window.is_key_down(key) { keys |= bit; }
        }
        demo.update(keys).map_err(|e| format!("{e:?}"))?;
        for (pixel, rgb) in pixels.iter_mut().zip(demo.frame().chunks_exact(3)) {
            *pixel = (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
        }
        window.update_with_buffer(&pixels, WIDTH, HEIGHT).map_err(|e| e.to_string())?;
    }
    Ok(())
}
