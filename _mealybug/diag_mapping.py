# -*- coding: utf-8 -*-
"""诊断：1) expected PNG 尺寸与灰阶值分布 2) raw 值分布 3) 穷举色号映射找最优"""
import os, itertools
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
RAW = os.path.join(BASE, "out")
EXP = os.path.join(BASE, "repo", "expected", "DMG-blob")

pngs = sorted(f for f in os.listdir(EXP) if f.endswith(".png"))
img = Image.open(os.path.join(EXP, pngs[0]))
print("PNG 尺寸:", img.size, "模式:", img.mode)
g = img.convert("L")
vals = sorted(set(g.getdata()))
print("首个 PNG 灰阶值集合:", vals[:10], "…共", len(vals), "种")

r = open(os.path.join(RAW, pngs[0][:-4] + ".raw"), "rb").read()
from collections import Counter
print("raw 值分布:", dict(Counter(r)))

# 穷举映射：raw 的 0-3 -> 四个灰阶的某种排列
target_vals = [0, 85, 170, 255]
def score(pngname, mapping):
    exp = Image.open(os.path.join(EXP, pngname)).convert("L")
    if exp.size != (160, 144):
        exp = exp.resize((160, 144), Image.NEAREST)
    e = list(exp.getdata())
    raw = open(os.path.join(RAW, pngname[:-4] + ".raw"), "rb").read()
    return sum(1 for i in range(160 * 144) if mapping.get(raw[i], 999) != e[i])

best_overall = None
for perm in itertools.permutations(target_vals):
    m = {i: v for i, v in enumerate(perm)}
    total = sum(score(p, m) for p in pngs)
    if best_overall is None or total < best_overall[0]:
        best_overall = (total, m)
print("\n最优映射:", best_overall[1], " 24 张总差像素:", best_overall[0], "/", 24*23040)
for p in pngs[:6]:
    print(f"  {score(p, best_overall[1]):7d}  {p}")
