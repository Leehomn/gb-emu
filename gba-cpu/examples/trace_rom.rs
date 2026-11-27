use gba_cpu::{Bus, Cpu};
use std::collections::VecDeque;
fn main() {
 for path in std::env::args().skip(1) {
    let mut bus = Bus::from_rom(std::fs::read(&path).unwrap()).unwrap();
    let mut cpu = Cpu::new();
    let mut trace = VecDeque::new();
    for i in 0..2_000_000 {
        let pc = cpu.pc();
        if trace.len()==12 { trace.pop_front(); }
        trace.push_back((pc,bus.read_half(pc),cpu.regs[7],cpu.regs[12]));
        if let Err(e) = cpu.step(&mut bus) {
           println!("{path} {i} {e:?} PC={pc:08x} r7={} r12={}",cpu.regs[7],cpu.regs[12]);
           for (p,w,r7,r12) in trace { println!("{p:08x} {:04x} r7={r7} r12={r12}",w.unwrap_or(0)); }
           break;
        }
        if cpu.pc()==pc {
            println!("{path} {i}: 自循环 PC={pc:08x}, r7={}, r12={}",cpu.regs[7],cpu.regs[12]);
            break;
        }
        if i == 1_999_999 {println!("{path}: 已执行 200 万条指令，PC={:08x}, r7={}, r12={}",cpu.pc(),cpu.regs[7],cpu.regs[12]);}
    }
 }
}
