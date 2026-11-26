import json, re, base64
t = open(r'C:\Users\pc\AppData\Local\Temp\dsh-spill-YyZc2F\session-28763a5c0857\3ba09b6e2439-web_fetch.txt', encoding='utf-8').read()
m = re.search(r'"content": "([^"]+)"', t)
b64 = m.group(1).replace('\\n', '')
data = base64.b64decode(b64)
out = r'C:\Users\pc\Documents\deepseek-harness\default-workspace\rust-learning\gb-emu\_mealybug\docboy_ppu.cpp'
open(out, 'wb').write(data)
print(len(data))
