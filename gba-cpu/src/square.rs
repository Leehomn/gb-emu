//! 自制 ARM 演示：ROM 读 KEYINPUT，擦旧方块、移动，再写显存。

use crate::{Bus, Cpu, Fault, ROM_START};

pub const RIGHT: u16 = 1 << 4;
pub const LEFT: u16 = 1 << 5;
pub const UP: u16 = 1 << 6;
pub const DOWN: u16 = 1 << 7;

const INPUT_LOOP: u32 = ROM_START + 52;
const PROGRAM: &[u32] = &[
    0xE59F30B0, // LDR r3, =显存起始
    0xE59F40B0, // LDR r4, =按键地址
    0xE59F80B0, // LDR r8, =一行字节数
    0xE59F90B0, // LDR r9, =最右偏移
    0xE59FA0B0, // LDR r10, =最下偏移
    0xE59FB0B0, // LDR r11, =红色像素对
    0xE59F10B0, // LDR r1, =初始横向
    0xE59F20B0, // LDR r2, =初始纵向
    0xE59F00B0, // LDR r0, =显示配置
    0xE59FC0B0, // LDR r12, =显示地址
    0xE58C0000, // STR r0, [r12]
    0xE1A0000B, // MOV r0, r11
    0xEB000015, // BL 绘制
    0xE3A00000, // MOV r0, #0
    0xEB000013, // BL 绘制
    0xE5945000, // LDR r5, [r4]
    0xE3150010, // TST r5, #16
    0x1A000001, // BNE 跳过右
    0xE1510009, // CMP r1, r9
    0x12811004, // ADDNE r1, r1, #4
    0xE3150020, // TST r5, #32
    0x1A000001, // BNE 跳过左
    0xE3510000, // CMP r1, #0
    0x12411004, // SUBNE r1, r1, #4
    0xE3150040, // TST r5, #64
    0x1A000001, // BNE 跳过上
    0xE3520000, // CMP r2, #0
    0x12422D0F, // SUBNE r2, r2, #960
    0xE3150080, // TST r5, #128
    0x1A000001, // BNE 跳过下
    0xE152000A, // CMP r2, r10
    0x12822D0F, // ADDNE r2, r2, #960
    0xE1A0000B, // MOV r0, r11
    0xEB000000, // BL 绘制
    0xEAFFFFE9, // B 输入循环
    0xE0836001, // ADD r6, r3, r1
    0xE0866002, // ADD r6, r6, r2
    0xE3A07008, // MOV r7, #8
    0xE5860000, // STR r0, [r6, #0]
    0xE5860004, // STR r0, [r6, #4]
    0xE5860008, // STR r0, [r6, #8]
    0xE586000C, // STR r0, [r6, #12]
    0xE0866008, // ADD r6, r6, r8
    0xE2577001, // SUBS r7, r7, #1
    0x1AFFFFF8, // BNE 绘制一行
    0xE12FFF1E, // BX lr
    0x06000000, // 显存起始
    0x04000130, // 按键地址
    0x000001E0, // 一行字节数
    0x000001D0, // 最右偏移
    0x00011D00, // 最下偏移
    0x001F001F, // 红色像素对
    0x000000E8, // 初始横向
    0x00008E80, // 初始纵向
    0x00000403, // 显示配置
    0x04000000, // 显示地址
];

pub struct SquareDemo {
    bus: Bus,
    cpu: Cpu,
}

impl SquareDemo {
    pub fn new() -> Result<Self, Fault> {
        let mut demo = Self { bus: Bus::new(PROGRAM), cpu: Cpu::new() };
        demo.update(0)?;
        Ok(demo)
    }

    pub fn update(&mut self, keys: u16) -> Result<(), Fault> {
        self.bus.set_keys(keys);
        // 一次宿主更新跑完一轮 ROM 循环，不把指令条数当作硬件帧。
        for _ in 0..256 {
            self.cpu.step(&mut self.bus)?;
            if self.cpu.pc() == INPUT_LOOP { return Ok(()); }
        }
        Err(Fault::StepLimit)
    }

    pub fn frame(&self) -> Vec<u8> {
        self.bus.mode3_rgb()
    }
}
