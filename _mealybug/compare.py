"""mealybug tearoom tests 一键跑分
对每个 DMG-blob 测试：跑 gb-emu.exe --mealybug 出 .raw，再与 expected 标准答案 PNG 逐像素比对。
色号映射（对齐 README 的 $00/$55/$AA/$FF）：0->255(白) 1->170 2->85 3->0(黑)
用法：python compare.py
"""
import os, subprocess, sys
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
EXE = os.path.join(BASE, "..", "target", "release", "gb-emu.exe")
ROM_DIR = os.path.join(BASE, "extracted")
EXPECTED = os.path.join(BASE, "repo", "expected", "DMG-blob")
RAW_DIR = os.path.join(BASE, "out")
os.makedirs(RAW_DIR, exist_ok=True)

SHADES = {0: 255, 1: 170, 2: 85, 3: 0}

# 只有 DMG-blob 有全套标准答案（24 个 PNG）。DMG-CPU B 只有 2 个，另说。
PNGS = sorted(os.listdir(EXPECTED))

def run_rom(rom_name):
    rom = os.path.join(ROM_DIR, rom_name)
    raw = os.path.join(RAW_DIR, rom_name.replace(".gb", ".raw"))
    r = subprocess.run([EXE, rom, "--mealybug", raw],
                       capture_output=True, text=True, cwd=BASE)
    if not os.path.exists(raw):
        return None, r.stdout + r.stderr
    return raw, None

def compare(raw_path, png_name):
    raw = open(raw_path, "rb").read()
    if len(raw) != 160 * 144:
        return None
    im = Image.open(os.path.join(EXPECTED, png_name)).convert("L")
    exp = list(im.getdata())
    diff = 0
    first_bad = None
    for i in range(160 * 144):
        got = SHADES.get(raw[i], raw[i])
        if got != exp[i]:
            diff += 1
            if first_bad is None:
                y, x = divmod(i, 160)
                first_bad = (x, y, got, exp[i])
    return diff, first_bad

def main():
    print(f"共 {len(PNGS)} 个 DMG 标准答案，逐个跑分……\n")
    total = 0
    passed = 0
    rows = []
    for png in PNGS:
        name = png.replace(".png", "")
        rom_name = name + ".gb"
        if not os.path.exists(os.path.join(ROM_DIR, rom_name)):
            print(f"[跳过] {name}: 无对应 ROM")
            continue
        total += 1
        raw, err = run_rom(rom_name)
        if raw is None:
            print(f"[错误] {name}: 断点未命中\n  {err.strip()[:200]}")
            rows.append((name, None))
            continue
        diff, first = compare(raw, png)
        ok = diff == 0
        if ok:
            passed += 1
        rows.append((name, diff))
        print(f"[{'PASS' if ok else 'FAIL'}] {name}: 差 {diff} 像素"
              + (f"  首坏 {first}" if first else ""))
    print(f"\n===== 结果：{passed}/{total} 通过 =====")
    fails = [(n, d) for n, d in rows if d is not None and d > 0]
    if fails:
        fails.sort(key=lambda x: -x[1])
        print("失败清单（按差像素降序）：")
        for n, d in fails:
            print(f"  {n}: {d}")

if __name__ == "__main__":
    main()
