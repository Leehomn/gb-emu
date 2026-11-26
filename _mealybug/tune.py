# -*- coding: utf-8 -*-
"""时序标定扫描：对若干测试 ROM 按 GB_TUNE 参数网格跑 gb-emu --mealybug，报每张的差像素数。
用法：python tune.py m3_bgp_change,m3_scx_low_3_bits "fetch=72,74,76,78,80;lat=1,3,5;objshift=0,-4"
每个 key=value 里 value 可逗号分隔多值，网格为笛卡尔积。默认只打印每组的合计差与逐张差。
"""
import os, sys, subprocess, itertools
from PIL import Image

BASE = os.path.dirname(os.path.abspath(__file__))
EXE = os.path.join(BASE, "..", "target", "release", "gb-emu.exe")
SHADES = {0: 255, 1: 170, 2: 85, 3: 0}
_exp_cache = {}


def expected(name):
    if name not in _exp_cache:
        im = Image.open(os.path.join(BASE, "repo", "expected", "DMG-blob", name + ".png")).convert("L")
        if im.size != (160, 144):
            im = im.resize((160, 144), Image.NEAREST)
        _exp_cache[name] = list(im.getdata())
    return _exp_cache[name]


def run(name, tune=None):
    out = os.path.join(BASE, "out", name + ".raw")
    env = dict(os.environ)
    if tune:
        env["GB_TUNE"] = tune
    subprocess.run([EXE, os.path.join(BASE, "extracted", name + ".gb"), "--mealybug", out],
                   capture_output=True, env=env, cwd=BASE, timeout=120)
    raw = open(out, "rb").read()
    exp = expected(name)
    return sum(1 for i in range(160 * 144) if SHADES.get(raw[i], raw[i]) != exp[i])


def main():
    names = sys.argv[1].split(",")
    spec = sys.argv[2] if len(sys.argv) > 2 else ""
    keys, lists = [], []
    for part in spec.split(";"):
        if not part.strip():
            continue
        k, vs = part.split("=")
        keys.append(k)
        lists.append(vs.split(","))
    if not keys:
        print(" ".join(f"{n}={run(n)}" for n in names))
        return
    results = []
    for combo in itertools.product(*lists):
        tune = ",".join(f"{k}={v}" for k, v in zip(keys, combo))
        diffs = [run(n, tune) for n in names]
        results.append((sum(diffs), tune, diffs))
    results.sort()
    for total, tune, diffs in results[:25]:
        print(f"{total:7d}  {tune:45s} " + " ".join(f"{n.split('_', 1)[-1][:14]}={d}" for n, d in zip(names, diffs)))


if __name__ == "__main__":
    main()
