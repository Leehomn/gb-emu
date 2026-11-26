# -*- coding: utf-8 -*-
"""只比对已生成的 out/*.raw 与 expected 标准答案 PNG，不重跑模拟器。
色号映射对齐 compare.py：0->255(白) 1->170 2->85 3->0(黑)。
用法：python compare_existing.py
"""
import os
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
RAW = os.path.join(BASE, "out")
EXP = os.path.join(BASE, "repo", "expected", "DMG-blob")
SHADES = {0: 255, 1: 170, 2: 85, 3: 0}

results = []
for png in sorted(os.listdir(EXP)):
    if not png.endswith(".png"):
        continue
    name = png[:-4]
    rawp = os.path.join(RAW, name + ".raw")
    if not os.path.exists(rawp):
        results.append((name, "NO_RAW", -1, "-"))
        continue
    raw = open(rawp, "rb").read()
    exp = Image.open(os.path.join(EXP, png)).convert("L")
    if exp.size != (160, 144):
        exp = exp.resize((160, 144), Image.NEAREST)
    expd = list(exp.getdata())
    diff = sum(1 for i in range(160 * 144) if SHADES.get(raw[i], raw[i]) != expd[i])
    results.append((name, "PASS" if diff == 0 else "FAIL", diff, f"{diff / 23040:.1%}"))

for n, s, d, pct in results:
    print(f"{s:6} {d:7d} ({pct})  {n}")
p = sum(1 for _, s, _, _ in results if s == "PASS")
print(f"\n合计 {p}/{len(results)} 通过")
