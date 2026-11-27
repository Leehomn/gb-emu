use gba_cpu::{Bus, Cpu, RAM_START, ROM_START};

#[test]
fn arm_loop_writes_its_sum() {
    let mut bus = Bus::new(&[
        0xE3A00003, // MOV r0, #3
        0xE3A01000, // MOV r1, #0
        0xE0811000, // ADD r1, r1, r0
        0xE2500001, // SUBS r0, r0, #1
        0x1AFFFFFC, // BNE 回到 ADD
        0xE5831000, // STR r1, [r3]
    ]);
    let mut cpu = Cpu::new();
    cpu.regs[3] = RAM_START;
    for _ in 0..12 { cpu.step(&mut bus).unwrap(); }
    assert_eq!(cpu.pc(), ROM_START + 24);
    assert_eq!(bus.read_word(RAM_START).unwrap(), 6);
}

#[test]
fn bx_runs_thumb_then_returns_to_arm() {
    let mut bus = Bus::new(&[0xE12FFF10, 0xE5832000, 0x32052207, 0x00004708]);
    let mut cpu = Cpu::new();
    cpu.regs[0] = ROM_START + 9;
    cpu.regs[1] = ROM_START + 4;
    cpu.regs[3] = RAM_START;
    for _ in 0..5 { cpu.step(&mut bus).unwrap(); }
    assert_eq!(bus.read_word(RAM_START).unwrap(), 12);
}

#[test]
fn thumb_bx_pc_uses_the_pipeline_address_and_aligns_arm_fetch() {
    // 两次分别从同一个字的低半字和高半字进入 Thumb。
    for entry in [ROM_START + 4, ROM_START + 6] {
        let mut bus = Bus::new(&[
            0xE12F_FF10, // ARM BX r0。
            0x4778_4778, // 两条 Thumb BX PC。
            0xE3A0_202A, // ARM MOV r2, #42。
        ]);
        let mut cpu = Cpu::new();
        cpu.regs[0] = entry | 1;
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc(), entry);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.pc(), ROM_START + 8);
        cpu.step(&mut bus).unwrap();
        assert_eq!(cpu.regs[2], 42);
        assert_eq!(cpu.pc(), ROM_START + 12);
    }
}
#[test]
fn thumb_loads_a_literal_and_stores_bytes_and_halfwords() {
    let mut bus = Bus::new(&[0xE12FFF10, 0x480221AB, 0x80417001, 0x00000000, 0x06000000]);
    let mut cpu = Cpu::new();
    cpu.regs[0] = ROM_START + 5;
    for _ in 0..5 { cpu.step(&mut bus).unwrap(); }
    assert_eq!(cpu.regs[0], 0x0600_0000);
    assert_eq!(bus.read_half(0x0600_0002), Ok(0xAB));
    assert_eq!(bus.read_half(0x0600_0000), Ok(0xABAB));
}

#[test]
fn arm_shift_carry_and_adc_feed_conditional_execution() {
    let mut cpu = Cpu::new();
    cpu.regs[1] = 0x8000_0001;
    cpu.regs[3] = 32;
    let mut bus = Bus::new(&[
        0xE1B0_0311, // MOVS r0,r1,LSL r3
        0x03A0_4007, // MOVEQ r4,#7
        0xE2A4_5000, // ADC r5,r4,#0
        0xE1B0_6065, // MOVS r6,r5,RRX
        0x23A0_7009, // MOVCS r7,#9（RRX 移出的低位为 0）
    ]);
    for _ in 0..5 { cpu.step(&mut bus).unwrap(); }
    assert_eq!((cpu.regs[0],cpu.regs[4],cpu.regs[5],cpu.regs[6],cpu.regs[7]),
        (0,7,8,0x8000_0004,0));
}

#[test]
fn thumb_bl_and_stack_can_preserve_registers_and_return() {
    let mut cpu = Cpu::new();
    let mut bus = Bus::new(&[
        0xE12F_FF10, // BX r0
        0xF801_F000, // BL 到 ROM+10
        0xB501_227B, // MOV r2,#123；子程序入口：PUSH {r0,lr}
        0xBD01_202A, // MOV r0,#42；POP {r0,pc}
    ]);
    cpu.regs[0]=ROM_START+5;
    cpu.regs[13]=RAM_START+0x100;
    for _ in 0..7 { cpu.step(&mut bus).unwrap(); }
    assert_eq!((cpu.regs[0],cpu.regs[2],cpu.regs[13],cpu.pc()),
        (ROM_START+5,123,RAM_START+0x100,ROM_START+10));
}

#[test]
fn arm_pc_relative_add_uses_instruction_address_plus_eight() {
    let mut bus = Bus::new(&[0xE28F_0009]); // ADD r0,pc,#9
    let mut cpu = Cpu::new();
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[0], ROM_START + 17);
}
