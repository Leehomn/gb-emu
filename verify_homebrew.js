// 无头验证：网页版内置的两款开源游戏能否被跑出画面
// 直接从 web/index.html 抽取 JS 类 + ROMS_B64 解码，自洽不依赖外部 .gb
const fs = require("fs");
const html = fs.readFileSync("web/index.html", "utf8");

// 抽取模拟器核心类（Bus/Cpu/Ppu）
const s = html.indexOf("<script>") + 8, e = html.indexOf("</" + "script>");
const js = html.slice(s, e);
const classEnd = js.indexOf("/* ================= 网页外壳");
const classes = js.slice(0, classEnd) + "\nglobalThis.Bus=Bus; globalThis.Cpu=Cpu; globalThis.Ppu=Ppu;";
eval(classes);

// 抽取内嵌 ROM base64
const m = /const\s+ROMS_B64\s*=\s*\{[\s\S]*?"2048":\s*"([^"]*)",[\s\S]*?"minesweep":\s*"([^"]*)"[\s\S]*?\};/.exec(html);
if (!m) throw new Error("找不到 ROMS_B64");
const b64toRom = (b64) => {
  const bin = Buffer.from(b64, "base64");
  return new Uint8Array(bin);
};

for (const [name, b64] of [["2048", m[1]], ["minesweep", m[2]]]) {
  const rom = b64toRom(b64);
  const bus = new Bus(), cpu = new Cpu(), ppu = new Ppu();
  bus.loadRom(rom);
  for (let i = 0; i < 3000000; i++) {
    const t = cpu.step(bus);
    ppu.tick(t, bus);
  }
  let nonZero = 0;
  for (let i = 0; i < ppu.framebuffer.length; i++) if (ppu.framebuffer[i] !== 0) nonZero++;
  const lcdc = bus.mem[0xFF40], ly = bus.mem[0xFF44];
  console.log(`${name}: LCDC=0x${lcdc.toString(16)} LY=${ly} 非零像素=${nonZero} PC=0x${cpu.pc.toString(16)} ${nonZero > 100 ? "✓有画面" : "✗可能空白"}`);
  if (nonZero <= 100) process.exitCode = 1;
}
