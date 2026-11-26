# -*- coding: utf-8 -*-
"""分析标准答案 / 模拟器输出：按行打印色号游程，反推真机 dot 时序。
用法：python analyze.py expected <测试名> [行号...]        标准答案
      python analyze.py out <测试名> [行号...]            模拟器 dump（out/<名>.raw）
      python analyze.py ascii expected <测试名> [行号...] 整行画成字符（. 浅 o 中 X 深 空格=白）
不给行号 = 自动挑前 70 行里有变化的 12 行。
"""
import os, sys
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
EXP = os.path.join(BASE, "repo", "expected", "DMG-blob")
OUT = os.path.join(BASE, "out")
TOIDX = {255: 0, 170: 1, 85: 2, 0: 3}
CH = " .oX"


def load(kind, name):
    if kind == "expected":
        im = Image.open(os.path.join(EXP, name + ".png")).convert("L")
        if im.size != (160, 144):
            im = im.resize((160, 144), Image.NEAREST)
        px = list(im.getdata())
        return [[TOIDX.get(v, v) for v in px[y * 160:(y + 1) * 160]] for y in range(144)]
    raw = open(os.path.join(OUT, name + ".raw"), "rb").read()
    return [[v for v in raw[y * 160:(y + 1) * 160]] for y in range(144)]


def runs(row):
    out = []
    for x, v in enumerate(row):
        c = v
        if out and out[-1][0] == c:
            out[-1][2] += 1
        else:
            out.append([c, x, 1])
    return out


def main():
    kind, name = sys.argv[1], sys.argv[2]
    args = sys.argv[3:]
    rows = load(kind, name)
    if args and args[0] == "ascii":
        ys = [int(a) for a in args[1:]] or list(range(144))
        for y in ys:
            print(f"{y:3d} |" + "".join(CH[v] for v in rows[y]) + "|")
        return
    if args:
        ys = [int(a) for a in args]
    else:
        ys = [y for y in range(0, 72) if len(set(rows[y])) > 1][:12]
    for y in ys:
        print(f"{y:3d} " + " ".join(f"[{s}:{c}x{n}]" for c, s, n in runs(rows[y])))


if __name__ == "__main__":
    main()
