import os
from PIL import Image
BASE = os.path.dirname(os.path.abspath(__file__))
EXP = os.path.join(BASE, "repo", "expected", "DMG-blob")
OUT = os.path.join(BASE, "out")
CH = " .oX"  # 0白 1浅 2深 3黑

def load_expected(name):
    im = Image.open(os.path.join(EXP, name + ".png")).convert("L")
    if im.size != (160, 144):
        im = im.resize((160, 144), Image.NEAREST)
    px = list(im.getdata())
    mp = {255:0, 170:1, 85:2, 0:3}
    return [[mp.get(v, v) for v in px[y*160:(y+1)*160]] for y in range(144)]

def load_out(name):
    raw = open(os.path.join(OUT, name + ".raw"), "rb").read()
    return [[v for v in raw[y*160:(y+1)*160]] for y in range(144)]

import sys
name = sys.argv[1]
exp = load_expected(name)
out = load_out(name)
# 找出有差异的行
diff_rows = []
for y in range(144):
    if exp[y] != out[y]:
        diff_rows.append(y)
print(f"差异行: {diff_rows}")
for y in diff_rows:
    e = "".join(CH[v] for v in exp[y])
    o = "".join(CH[v] for v in out[y])
    print(f"\n--- 行 {y} (期望) ---")
    print(e)
    print(f"--- 行 {y} (我的) ---")
    print(o)
