"""并排 ASCII 对比：我的 .raw vs expected PNG，用于定位纯 BG 测试差在哪。

关键：.raw 存的是「色号 0~3」（0=白 1=浅灰 2=深灰 3=黑），expected PNG 存的是
灰度值（255/170/85/0）。两套映射必须各自正确。
"""
import os, subprocess
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
EXE = os.path.join(BASE, "..", "target", "release", "gb-emu.exe")
ROM_DIR = os.path.join(BASE, "extracted")
EXPECTED = os.path.join(BASE, "repo", "expected", "DMG-blob")

# .raw 色号 0~3 → 字符
SHADE_CHARS = {0: " ", 1: ".", 2: "o", 3: "#"}
# expected PNG 灰度值 → 字符
GRAY_CHARS = {255: " ", 170: ".", 85: "o", 0: "#"}

def run(rom_name):
    rom = os.path.join(ROM_DIR, rom_name)
    raw = os.path.join(BASE, "out", rom_name.replace(".gb", ".raw"))
    subprocess.run([EXE, rom, "--mealybug", raw], capture_output=True)
    return open(raw, "rb").read()

for name in ["m3_scx_low_3_bits", "m3_scx_high_5_bits", "m3_scy_change", "m3_bgp_change"]:
    raw = run(name + ".gb")
    im = Image.open(os.path.join(EXPECTED, name + ".png")).convert("L")
    exp = list(im.get_flattened_data())
    print(f"\n========== {name} (第0行起 10 行：左=我的 右=期望) ==========")
    for y in range(10):
        mine = "".join(SHADE_CHARS.get(v, "?") for v in raw[y*160:(y+1)*160])
        want = "".join(GRAY_CHARS.get(v, "?") for v in exp[y*160:(y+1)*160])
        print(f"{mine} | {want}")
