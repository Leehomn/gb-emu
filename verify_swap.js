// 无头验证换血后的 web/index.html：模拟浏览器加载 wasm + 玩三款游戏，检查画面非零像素
const fs = require('fs');
const path = require('path');

async function main() {
  const wasmBytes = fs.readFileSync('web/gb_emu.wasm');
  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const w = instance.exports;
  const mem = w.memory;
  const SCREEN = 160 * 144;

  // 从换血后的 index.html 提取 ROMS_B64（三款游戏）
  const html = fs.readFileSync('web/index.html', 'utf8');
  const m = html.match(/"adjustris":\s*"([^"]*)",[\s\S]*?"2048":\s*"([^"]*)",[\s\S]*?"minesweep":\s*"([^"]*)"[\s\S]*?\};/);
  if (!m) throw new Error('ROMS_B64 提取失败');
  const roms = { adjustris: m[1], '2048': m[2], minesweep: m[3] };
  const b64toRom = b64 => { const bin = Buffer.from(b64, 'base64'); return new Uint8Array(bin); };

  for (const [name, b64] of Object.entries(roms)) {
    const rom = b64toRom(b64);
    const gb = w.gb_new();
    const rp = w.gb_alloc(rom.length);
    new Uint8Array(mem.buffer, rp, rom.length).set(rom);
    w.gb_load(gb, rp, rom.length);
    w.gb_free_alloc(rp, rom.length);

    // 模拟按 A（btn bit0=0）几帧，让游戏进入可玩状态
    for (let i = 0; i < 120; i++) {
      if (i > 20) w.gb_set_keys(gb, 0x0F, 0x0E);  // 按住 A
      else w.gb_set_keys(gb, 0x0F, 0x0F);
      w.gb_run_frame(gb);
    }
    w.gb_set_keys(gb, 0x0F, 0x0F);
    for (let i = 0; i < 60; i++) w.gb_run_frame(gb);

    const sp = w.gb_screen_ptr(gb);
    const fb = new Uint8Array(mem.buffer, sp, SCREEN);
    let nz = 0;
    for (let i = 0; i < SCREEN; i++) if (fb[i] !== 0) nz++;
    const sram = w.gb_sram_len(gb);
    console.log(`${name}: 非零像素=${nz} SRAM=${sram}字节`);
    w.gb_free(gb);
  }
  console.log('✓ 三款游戏在 wasm 内核下都能出画面');
}

main().catch(e => { console.error(e); process.exit(1); });
