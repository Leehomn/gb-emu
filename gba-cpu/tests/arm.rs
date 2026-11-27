use gba_cpu::{Bus, Cpu, Fault, RAM_START, ROM_START};

#[test]
fn loads_adds_and_stores_real_arm_words() {
    let mut bus = Bus::new(&[0xE5930000, 0xE5931004, 0xE0802001, 0xE5832008]);
    bus.write_word(RAM_START, 7).unwrap();
    bus.write_word(RAM_START + 4, 5).unwrap();
    let mut cpu = Cpu::new();
    cpu.regs[3] = RAM_START;

    for offset in [0, 4, 8, 12] {
        assert_eq!(cpu.pc(), ROM_START + offset);
        cpu.step(&mut bus).unwrap();
    }

    assert_eq!(cpu.regs[..3], [7, 5, 12]);
    assert_eq!(cpu.pc(), ROM_START + 16);
    assert_eq!(bus.read_word(RAM_START + 8), Ok(12));
}

#[test]
fn unsupported_instruction_leaves_pc_unchanged() {
    let mut bus = Bus::new(&[0xEF000000]); // SWI：这一阶段尚未实现。
    let mut cpu = Cpu::new();
    assert_eq!(cpu.step(&mut bus), Err(Fault::Unsupported(0xEF000000)));
    assert_eq!(cpu.pc(), ROM_START);
}

#[test]
fn bus_rejects_unaligned_and_out_of_range_words() {
    let mut bus = Bus::new(&[0xE0802001]);
    assert_eq!(bus.read_word(ROM_START), Ok(0xE0802001));
    assert_eq!(bus.read_word(ROM_START + 4), Err(Fault::Unmapped(ROM_START + 4)));
    assert_eq!(bus.read_word(RAM_START + 1), Err(Fault::Unaligned(RAM_START + 1)));
    assert_eq!(bus.write_word(ROM_START, 0), Err(Fault::ReadOnly(ROM_START)));
}


#[test]
fn cartridge_entry_has_a_stack_and_arm_can_push_pop_multiple_registers() {
    let mut cpu = Cpu::new();
    let sp = cpu.regs[13];
    assert_eq!(sp, 0x0300_7F00);
    cpu.regs[0] = 0x11223344;
    cpu.regs[1] = 0x55667788;
    cpu.regs[14] = ROM_START + 12;
    let mut bus = Bus::new(&[
        0xE92D_4003, // STMDB sp!, {r0,r1,lr}
        0xE3A0_0000, // MOV r0,#0
        0xE8BD_8003, // LDMIA sp!, {r0,r1,pc}
    ]);
    cpu.step(&mut bus).unwrap();
    assert_eq!(cpu.regs[13], sp - 12);
    assert_eq!(bus.read_word(sp - 12), Ok(0x11223344));
    assert_eq!(bus.read_word(sp - 8), Ok(0x55667788));
    cpu.step(&mut bus).unwrap();
    cpu.step(&mut bus).unwrap();
    assert_eq!((cpu.regs[0], cpu.regs[1], cpu.regs[13], cpu.pc()),
        (0x11223344, 0x55667788, sp, ROM_START + 12));
}
