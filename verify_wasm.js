// 验证 wasm 版模拟器：加载 gb_emu.wasm，喂 2048 ROM，跑若干帧，读屏幕统计非零像素
const fs = require('fs');

async function main() {
  const wasmBytes = fs.readFileSync('target/wasm32-unknown-unknown/release/gb_emu.wasm');
  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const { gb_new, gb_alloc, gb_load, gb_run_frame, gb_screen_ptr, gb_free, memory } = instance.exports;

  const rom = fs.readFileSync('roms_homebrew/2048.gb');
  const SCREEN = 160 * 144;

  // 1. 分配 ROM 缓冲（现取视图写，写完即弃）
  const romPtr = gb_alloc(rom.length);
  new Uint8Array(memory.buffer, romPtr, rom.length).set(rom);

  // 2. 建实例 + 装载
  const ptr = gb_new();
  gb_load(ptr, romPtr, rom.length);

  // 3. 跑 300 帧
  for (let i = 0; i < 300; i++) gb_run_frame(ptr);

  // 4. 每帧现取视图读屏幕
  const screenPtr = gb_screen_ptr(ptr);
  const screen = new Uint8Array(memory.buffer, screenPtr, SCREEN);

  let nz = 0;
  const hist = [0, 0, 0, 0];
  for (let i = 0; i < SCREEN; i++) {
    const c = screen[i];
    hist[c]++;
    if (c !== 0) nz++;
  }
  console.log('非零像素', nz, '/', SCREEN);
  console.log('色号分布 0/1/2/3 =', hist.join('/'));

  // ASCII 预览（80×36，每字符 2×4 像素）
  const W = 80, H = 36;
  const cx = Math.floor(160 / W), cy = Math.floor(144 / H);
  const ramp = ' .:+*#';
  let lines = [];
  for (let gy = 0; gy < H; gy++) {
    let line = '';
    for (let gx = 0; gx < W; gx++) {
      let sum = 0, cnt = 0;
      for (let dy = 0; dy < cy; dy++) for (let dx = 0; dx < cx; dx++) {
        sum += screen[(gy * cy + dy) * 160 + gx * cx + dx]; cnt++;
      }
      const avg = sum / cnt;
      line += ramp[Math.round((avg / 3) * (ramp.length - 1))];
    }
    lines.push(line);
  }
  console.log(lines.join('\n'));

  gb_free(ptr);
  console.log(nz > 1000 ? '✓ 画面有内容，wasm 版跑通' : '✗ 画面几乎全黑，可能没跑起来');
}

main().catch(e => { console.error(e); process.exit(1); });
