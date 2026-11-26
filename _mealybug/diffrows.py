# -*- coding: utf-8 -*-
"""精确差分：python diffrows.py <测试名> [y0 y1]  —— 逐行给出期望/实际的游程（仅列出差的行，最多 40 行）"""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from analyze import load

name = sys.argv[1]
y0 = int(sys.argv[2]) if len(sys.argv) > 2 else 0
y1 = int(sys.argv[3]) if len(sys.argv) > 3 else 144
e = load("expected", name)
o = load("out", name)


def runs(row, base=0):
    out = []
    for x, v in enumerate(row):
        if x < base:
            continue
        if out and out[-1][0] == v:
            out[-1][2] += 1
        else:
            out.append([v, x, 1])
    return " ".join(f"[{s}:{c}x{n}]" for c, s, n in runs_(out))


def runs_(lst):
    res = []
    for c, s, n in lst:
        res.append([c, s, n])
    return res


shown = 0
bad_lines = []
for y in range(y0, y1):
    d = [x for x in range(160) if e[y][x] != o[y][x]]
    if not d:
        continue
    bad_lines.append((y, len(d), d[0], d[-1]))
    if shown < 40:
        lo, hi = max(0, d[0] - 6), min(160, d[-1] + 7)
        print(f"{y:3d} n={len(d):3d} x={d[0]}..{d[-1]}")
        print("    exp " + runs(e[y][lo:hi], lo))
        print("    out " + runs(o[y][lo:hi], lo))
        shown += 1
print(f"bad lines: {len(bad_lines)} / {y1 - y0};  total diff = {sum(b[1] for b in bad_lines)}")
print("first/last bad x per 8th bad line:", [(y, a, b) for y, n, a, b in bad_lines[::8]][:20])
