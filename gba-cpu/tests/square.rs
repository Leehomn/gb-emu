use gba_cpu::square::{SquareDemo, DOWN, LEFT, RIGHT, UP};
use gba_cpu::WIDTH;

fn position(demo: &SquareDemo) -> (usize, usize) {
    let frame = demo.frame();
    let i = frame.chunks_exact(3).position(|p| p == [255, 0, 0]).unwrap();
    assert_eq!(frame.chunks_exact(3).filter(|p| *p == [255, 0, 0]).count(), 64);
    (i % WIDTH, i / WIDTH)
}

#[test]
fn rom_draws_and_moves_a_square_in_all_four_directions() {
    let mut demo = SquareDemo::new().unwrap();
    let (x, y) = position(&demo);
    for (keys, expected) in [
        (0, (x, y)), (RIGHT, (x + 2, y)), (DOWN, (x + 2, y + 2)),
        (LEFT, (x, y + 2)), (UP, (x, y)),
    ] {
        demo.update(keys).unwrap();
        assert_eq!(position(&demo), expected);
    }
}

#[test]
fn held_keys_stop_at_screen_edges_without_wrapping() {
    let mut demo = SquareDemo::new().unwrap();
    for _ in 0..130 { demo.update(LEFT | UP).unwrap(); }
    assert_eq!(position(&demo), (0, 0));
    for _ in 0..130 { demo.update(RIGHT | DOWN).unwrap(); }
    assert_eq!(position(&demo), (232, 152));
}
