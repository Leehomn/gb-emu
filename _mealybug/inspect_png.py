from PIL import Image
import os, collections

d = os.path.join(os.path.dirname(__file__), "repo", "expected", "DMG-blob")
for name in ["m3_lcdc_bg_en_change.png", "m3_bgp_change.png", "m3_scy_change.png", "m2_win_en_toggle.png"]:
    p = os.path.join(d, name)
    im = Image.open(p)
    print(f"{name}: size={im.size} mode={im.mode}")
    vals = collections.Counter(im.getdata())
    print(f"   像素值分布: {dict(sorted(vals.items()))}")
