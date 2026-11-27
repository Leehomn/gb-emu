"""用独立的 Keystone 汇编器构建一个 ARM/Thumb 位图验证程序。"""
from pathlib import Path
import sys

from keystone import KS_ARCH_ARM, KS_MODE_ARM, KS_MODE_THUMB, Ks

ROM = 0x08000000

# ARM 入口布置栈，用 BX 切换到 Thumb。
ARM = """
add r0, pc, #9
ldr sp, [pc, #0]
bx r0
.word 0x03007f00
"""

# Thumb 主程序开启 Mode 3，调用子程序绘制两个像素。
THUMB = """
ldr r0, display_reg
movs r1, #4
lsls r1, r1, #8
adds r1, #3
strh r1, [r0]
ldr r2, vram
bl draw
stop:
b stop
draw:
push {lr}
movs r0, #31
strh r0, [r2]
lsls r0, r0, #10
strh r0, [r2, #2]
pop {pc}
.balign 4
display_reg:
.word 0x04000000
vram:
.word 0x06000000
"""

def assemble(source, mode, address):
    code, _ = Ks(KS_ARCH_ARM, mode).asm(source, addr=address, as_bytes=True)
    return code

if __name__ == "__main__":
    arm = assemble(ARM, KS_MODE_ARM, ROM)
    assert len(arm) == 16
    thumb = assemble(THUMB, KS_MODE_THUMB, ROM + len(arm))
    with Path(sys.argv[1]).open("xb") as output:
        output.write(arm + thumb)
