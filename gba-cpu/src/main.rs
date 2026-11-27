use gba_cpu::{Bus, Cpu, Fault, RAM_START};

fn demo() -> Result<(), Fault> {
    let program = [
        (0xE5930000, "LDR r0, [r3]"),
        (0xE5931004, "LDR r1, [r3, #4]"),
        (0xE0802001, "ADD r2, r0, r1"),
        (0xE5832008, "STR r2, [r3, #8]"),
    ];
    let mut bus = Bus::new(&program.map(|(word, _)| word));
    bus.write_word(RAM_START, 7)?;
    bus.write_word(RAM_START + 4, 5)?;
    let mut cpu = Cpu::new();
    cpu.regs[3] = RAM_START;

    println!("r3 = {RAM_START:#010X}，内存里先放入 7 和 5。");
    for (_, name) in program {
        let pc = cpu.pc();
        let word = bus.read_word(pc)?;
        cpu.step(&mut bus)?;
        println!(
            "PC {pc:#010X} → {:#010X} | {word:08X} {name:<19} | r0={} r1={} r2={}",
            cpu.pc(), cpu.regs[0], cpu.regs[1], cpu.regs[2]
        );
    }

    let address = RAM_START + 8;
    println!("内存 [{address:#010X}] = {}", bus.read_word(address)?);
    Ok(())
}

fn run_rom(path: &str, budget: usize, output: &str) -> Result<(), String> {
    use gba_cpu::{HEIGHT, WIDTH};
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut rom = Vec::new();
    use std::io::Read;
    file.take(32 * 1024 * 1024 + 1).read_to_end(&mut rom).map_err(|e| e.to_string())?;
    let mut bus = Bus::from_rom(rom).map_err(|e| format!("{e:?}"))?;
    let mut cpu = Cpu::new();
    for _ in 0..budget {
        cpu.step(&mut bus).map_err(|e| format!("PC {:#010X}: {e:?}", cpu.pc()))?;
    }
    let mut ppm = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    ppm.extend(bus.mode3_rgb());
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true)
        .open(output).map_err(|e| format!("{output}: {e}"))?;
    use std::io::Write;
    file.write_all(&ppm).map_err(|e| e.to_string())?;
    println!("执行 {budget} 条指令，画面已写到 {output}。");
    Ok(())
}

fn main() -> std::process::ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let result = match args.as_slice() {
        [] => demo().map_err(|e| format!("{e:?}")),
        [flag, path, budget, output] if flag == "--rom" => {
            budget.parse::<usize>()
                .map_err(|_| "指令数必须是非负整数。".to_owned())
                .and_then(|budget| run_rom(path, budget, output))
        }
        _ => Err("用法：gba-cpu --rom <文件.gba> <指令数> <输出.ppm>".to_owned()),
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            std::process::ExitCode::FAILURE
        }
    }
}
