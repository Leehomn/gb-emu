// 无头验证：网页版内置的三款开源游戏能否被跑出画面
// 直接从 web/index.html 抽取 JS 类 + ROMS_B64 解码，自洽不依赖外部 .gb
const fs = require("fs");
const html = fs.readFileSync("web/index.html", "utf8");

// 抽取模拟器核心类（Bus/Cpu/Ppu）
const s = html.indexOf("<script>") + 8, e = html.indexOf("</" + "script>");
const js = html.slice(s, e);
const classEnd = js.indexOf("/* ================= 网页外壳");
const classes = js.slice(0, classEnd) + "\nglobalThis.Bus=Bus; globalThis.Cpu=Cpu; globalThis.Ppu=Ppu;";
eval(classes);

// 抽取内嵌 ROM base64（三款，顺序无关）
const m = /const\s+ROMS_B64\s*=\s*\{[\s\S]*?"adjustris":\s*"([^"]*)",[\s\S]*?"2048":\s*"([^"]*)",[\s\S]*?"minesweep":\s*"([^"]*)"[\s\S]*?\};/.exec(html);
if (!m) throw new Error("找不到 ROMS_B64 三款条目");
const b64toRom = (b64) => new Uint8Array(Buffer.from(b64, "base64"));

const games = [["adjustris", m[1]], ["2048", m[2]], ["minesweep", m[3]]];
for (const [name, b64] of games) {
  const rom = b64toRom(b64);
  const bus = new Bus(), cpu = new Cpu(), ppu = new Ppu();
  bus.loadRom(rom);
  // hook apu.write 统计写次数（JS 版 Bus 没带 apu_writes 诊断字段）
  let apuWrites = 0;
  const origWrite = bus.apu.write.bind(bus.apu);
  bus.apu.write = (addr, val) => { apuWrites++; return origWrite(addr, val); };
  // 顺带模拟按 A + 方向键，看能否触发 APU 写
  for (let i = 0; i < 5000000; i++) {
    const t = cpu.step(bus);
    ppu.tick(t, bus);
    if (i % 200000 === 0) {
      bus.joypad_btn = 0xFF & ~0x01; // 按 A
      bus.joypad_dir = 0xFF;         // 方向不按
    } else if (i % 200000 === 100000) {
      bus.joypad_btn = 0xFF;
      bus.joypad_dir = 0xFF & ~0x08; // 按 下
    }
  }
  let nonZero = 0;
  for (let i = 0; i < ppu.framebuffer.length; i++) if (ppu.framebuffer[i] !== 0) nonZero++;
  const lcdc = bus.mem[0xFF40], ly = bus.mem[0xFF44];
  console.log(`${name}: LCDC=0x${lcdc.toString(16)} LY=${ly} 非零像素=${nonZero} PC=0x${cpu.pc.toString(16)} APU写=${apuWrites} ${nonZero > 100 ? "✓有画面" : "✗可能空白"}`);
  if (nonZero <= 100) process.exitCode = 1;
}
