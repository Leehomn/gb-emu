use gba_cpu::{Bus, Cpu};
use std::cell::RefCell;

#[derive(Default)]
struct Player {
    cartridge: Option<(Bus, Cpu)>,
    incoming: Vec<u8>,
    bios: Option<Vec<u8>>,
    audio: Vec<[i16; 2]>,
}

thread_local! {
    static PLAYER: RefCell<Player> = RefCell::new(Player::default());
}

fn main() {}

#[unsafe(no_mangle)]
pub extern "C" fn input_buffer(size: usize) -> *mut u8 {
    PLAYER.with_borrow_mut(|player| {
        if size > 32 * 1024 * 1024 { return std::ptr::null_mut(); }
        player.incoming.resize(size, 0);
        player.incoming.as_mut_ptr()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn use_bios(enabled: bool) -> bool {
    PLAYER.with_borrow_mut(|player| {
        if enabled && player.incoming.len() != 0x4000 { return false; }
        player.bios = enabled.then(|| std::mem::take(&mut player.incoming));
        true
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn load_rom() -> bool {
    PLAYER.with_borrow_mut(|player| {
        if player.incoming.is_empty() { return false; }
        let Ok(mut bus) = Bus::from_rom(std::mem::take(&mut player.incoming)) else { return false; };
        let cpu = if let Some(bios) = &player.bios {
            if bus.load_bios(bios.clone()).is_err() { return false; }
            Cpu::from_reset()
        } else { Cpu::new() };
        player.cartridge = Some((bus, cpu));
        player.audio.clear();
        true
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn load_save() -> bool {
    PLAYER.with_borrow_mut(|player| {
        player.cartridge.as_mut().is_some_and(|(bus, _)| bus.restore_save(&player.incoming).is_ok())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn frame(keys: u16) -> bool {
    PLAYER.with_borrow_mut(|player| {
        let Some((bus, cpu)) = &mut player.cartridge else { return false; };
        bus.set_keys(keys);
        let result = cpu.run_frame(bus);
        player.audio = bus.take_audio_samples();
        result.is_ok()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn pc() -> u32 {
    PLAYER.with_borrow(|player| player.cartridge.as_ref().map_or(0, |(_, cpu)| cpu.pc()))
}

#[unsafe(no_mangle)]
pub extern "C" fn pixels() -> *const u8 {
    PLAYER.with_borrow(|player| player.cartridge.as_ref().map_or(std::ptr::null(), |(bus, _)| bus.scanout_rgb().as_ptr()))
}

#[unsafe(no_mangle)]
pub extern "C" fn samples() -> *const i16 {
    PLAYER.with_borrow(|player| player.audio.as_ptr().cast())
}

#[unsafe(no_mangle)]
pub extern "C" fn sample_count() -> usize {
    PLAYER.with_borrow(|player| player.audio.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn save_address() -> *const u8 {
    PLAYER.with_borrow(|player| player.cartridge.as_ref().map_or(std::ptr::null(), |(bus, _)| bus.save_data().as_ptr()))
}

#[unsafe(no_mangle)]
pub extern "C" fn save_size() -> usize {
    PLAYER.with_borrow(|player| player.cartridge.as_ref().map_or(0, |(bus, _)| bus.save_data().len()))
}
