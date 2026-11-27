use gba_cpu::{Bus, Cpu, HEIGHT, WIDTH};
use minifb::{Key, Scale, Window, WindowOptions};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

fn main() -> Result<(), String> {
    let mut paths = std::env::args_os().skip(1);
    let path = PathBuf::from(paths.next().ok_or("用法：play_rom <游戏.gba> [BIOS.bin]")?);
    let bios = paths.next();
    if paths.next().is_some() { return Err("用法：play_rom <游戏.gba> [BIOS.bin]".into()); }
    let mut rom = Vec::new();
    File::open(&path).map_err(|e| e.to_string())?
        .take(32 * 1024 * 1024 + 1).read_to_end(&mut rom).map_err(|e| e.to_string())?;
    let mut bus = Bus::from_rom(rom).map_err(|e| format!("{e:?}"))?;
    let save = save_path(&path);
    let initial_save = load_save(&mut bus, &save)?;
    let mut cpu = if let Some(path) = bios {
        let mut bytes = Vec::new();
        File::open(path).map_err(|e| e.to_string())?
            .take(0x4001).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        bus.load_bios(bytes).map_err(|e| format!("BIOS 加载失败：{e:?}"))?;
        Cpu::from_reset()
    } else {
        Cpu::new()
    };
    let mut window = Window::new("GBA 原型 | Esc 退出", WIDTH, HEIGHT,
        WindowOptions { scale: Scale::X2, ..WindowOptions::default() }).map_err(|e| e.to_string())?;
    window.set_target_fps(60);
    let buttons = [Key::Z, Key::X, Key::RightShift, Key::Enter, Key::Right, Key::Left,
        Key::Up, Key::Down, Key::S, Key::A];
    let mut pixels = vec![0; WIDTH * HEIGHT];
    let play_result = loop {
        if !window.is_open() || window.is_key_down(Key::Escape) { break Ok(()); }
        let keys = buttons.iter().enumerate().fold(0, |mask, (i, &key)| {
            mask | if window.is_key_down(key) { 1 << i } else { 0 }
        });
        bus.set_keys(keys);
        if let Err(error) = cpu.run_frame(&mut bus) {
            break Err(format!("PC {:#010X}: {error:?}", cpu.pc()));
        }
        for (out, rgb) in pixels.iter_mut().zip(bus.scanout_rgb().chunks_exact(3)) {
            *out = u32::from(rgb[0]) << 16 | u32::from(rgb[1]) << 8 | u32::from(rgb[2]);
        }
        if let Err(error) = window.update_with_buffer(&pixels, WIDTH, HEIGHT) {
            break Err(error.to_string());
        }
    };
    match (play_result, store_save(&bus, &save, &initial_save)) {
        (Err(play_error), Err(save_error)) => Err(format!("{play_error}\n{save_error}")),
        (result, Ok(())) => result,
        (Ok(()), Err(save_error)) => Err(save_error),
    }
}

fn save_path(rom: &Path) -> PathBuf {
    let mut path = rom.as_os_str().to_os_string();
    path.push(".sav");
    path.into()
}

fn load_save(bus: &mut Bus, path: &Path) -> Result<Vec<u8>, String> {
    match File::open(path) {
        Ok(file) => {
            let mut data = Vec::new();
            file.take(128 * 1024 + 1).read_to_end(&mut data)
                .map_err(|error| format!("读取存档 {} 失败：{error}", path.display()))?;
            bus.restore_save(&data)
                .map_err(|_| format!("存档 {} 的容量与卡带不匹配", path.display()))?;
            Ok(data)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(bus.save_data().to_vec()),
        Err(error) => Err(format!("打开存档 {} 失败：{error}", path.display())),
    }
}

fn store_save(bus: &Bus, path: &Path, initial: &[u8]) -> Result<(), String> {
    let data = bus.save_data();
    if data == initial { return Ok(()); }
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".tmp-{}", std::process::id()));
    let pending = PathBuf::from(name);
    let error_message = |error| format!("保存存档 {} 失败：{error}", path.display());
    let mut file = OpenOptions::new().write(true).create_new(true).open(&pending).map_err(error_message)?;
    let result = file.write_all(data).and_then(|()| file.sync_all());
    drop(file);
    match result.and_then(|()| fs::rename(&pending, path)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&pending);
            Err(error_message(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_can_reopen_and_replace_a_save_file() {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("target").join(format!(
            "gba-save-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = save_path(&directory.join("卡带.gba"));
        let mut bus = Bus::new(&[]);
        let initial = load_save(&mut bus, &path).unwrap();
        store_save(&bus, &path, &initial).unwrap();
        assert!(!path.exists());
        bus.write_byte(0x0E00_0001, 0x76).unwrap();
        store_save(&bus, &path, &initial).unwrap();

        let mut reopened = Bus::new(&[]);
        let previous = load_save(&mut reopened, &path).unwrap();
        assert_eq!(reopened.read_byte(0x0E00_0001), Ok(0x76));
        reopened.write_byte(0x0E00_0001, 0x21).unwrap();
        store_save(&reopened, &path, &previous).unwrap();
        assert_eq!(std::fs::read(&path).unwrap()[1], 0x21);

        std::fs::write(&path, [0xAB; 16]).unwrap();
        assert!(load_save(&mut reopened, &path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), [0xAB; 16]);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
