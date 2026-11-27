const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { chromium } = require(process.env.GBA_PLAYWRIGHT || "playwright");

const webRoot = path.resolve(__dirname, "../web");
const assets = new Map([
  ["/", ["index.html", "text/html; charset=utf-8"]],
  ["/player.css", ["player.css", "text/css"]],
  ["/player.js", ["player.js", "text/javascript"]],
  ["/gba.wasm", ["gba.wasm", "application/wasm"]],
]);

function testRom() {
  const words = [];
  const literals = [];
  function load(register, value) {
    literals.push({ instruction: words.length, register, value });
    words.push(0);
  }
  function write(address, value) {
    load(0, address);
    load(1, value);
    words.push(0xE1C010B0); // STRH r1,[r0]。
  }
  write(0x04000000, 0x0403);
  write(0x04000084, 0x0080);
  write(0x04000080, 0x1177);
  write(0x04000082, 0x0002);
  write(0x04000062, 0xF080);
  write(0x04000064, 0x8700);
  load(0, 0x0E000000);
  load(2, 0x06000002);
  words.push(0xE5D01000, 0xE1C210B0); // 启动时把存档首字节写到第二个像素。
  load(0, 0x04000130);
  load(1, 0x06000000);
  load(4, 0x0E000000);
  words.push(
    0xE1D020B0, // LDRH r2,[r0]。
    0xE3120010, // TST r2,#16。
    0x03A0301F, // MOVEQ r3,#31。
    0x13A03000, // MOVNE r3,#0。
    0x01C130B0, // STRHEQ r3,[r1]。
    0x05C43000, // STRBEQ r3,[r4]。
    0xEAFFFFF8, // B 到按键读取。
  );
  for (const { instruction, register, value } of literals) {
    const offset = (words.length - instruction - 2) * 4;
    words[instruction] = 0xE59F0000 | (register << 12) | offset;
    words.push(value);
  }
  const rom = Buffer.alloc(words.length * 4);
  words.forEach((word, i) => rom.writeUInt32LE(word >>> 0, 4 * i));
  return { name: "整机测试.gba", mimeType: "application/octet-stream", buffer: rom };
}

async function run() {
  const server = http.createServer(async (request, response) => {
    const asset = assets.get(request.url);
    if (!asset) { response.writeHead(404).end(); return; }
    try {
      const contents = await fs.readFile(path.join(webRoot, asset[0]));
      response.writeHead(200, { "Content-Type": asset[1] }).end(contents);
    } catch { response.writeHead(500).end(); }
  });
  await new Promise((resolve, reject) => {
    server.on("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  let browser;
  try {
    browser = await chromium.launch({ executablePath: process.env.GBA_CHROME || undefined });
    const page = await browser.newPage({ viewport: { width: 400, height: 800 }, hasTouch: true });
    const failures = [];
    page.on("pageerror", error => failures.push(error.message));
    await page.addInitScript(() => {
      window.audibleBuffers = 0;
      window.framesPerRefresh = [];
      window.slowAudioRates = [];
      let frames = 0;
      const animationFrame = window.requestAnimationFrame.bind(window);
      window.requestAnimationFrame = callback => animationFrame(time => {
        frames = 0;
        callback(time);
        if (window.simulateSlowCore) window.framesPerRefresh.push(frames);
      });
      const instantiate = WebAssembly.instantiate;
      WebAssembly.instantiate = async function (...args) {
        const built = await instantiate.apply(this, args);
        const core = built.instance.exports;
        return { instance: { exports: { ...core, frame(keys) {
          if (window.simulateSlowCore) {
            frames++;
            const deadline = performance.now() + 25;
            while (performance.now() < deadline) {}
          }
          return core.frame(keys);
        } } } };
      };
      const createBufferSource = AudioContext.prototype.createBufferSource;
      AudioContext.prototype.createBufferSource = function () {
        const source = createBufferSource.call(this);
        const start = source.start.bind(source);
        source.start = (...args) => {
          if (source.buffer.getChannelData(0).some(value => value !== 0)) window.audibleBuffers++;
          if (window.simulateSlowCore) window.slowAudioRates.push(source.playbackRate.value);
          start(...args);
        };
        return source;
      };
    });
    const url = "http://127.0.0.1:" + server.address().port;
    await page.goto(url);
    await page.waitForFunction(() => !document.querySelector("#open").disabled);
    await page.locator("#rom").setInputFiles(testRom());
    await page.waitForFunction(() => document.querySelector("#status").textContent === "整机测试.gba");
    assert.equal(await page.evaluate(() => document.activeElement.id), "screen");
    await page.keyboard.down("ArrowRight");
    await page.waitForFunction(() => document.querySelector("canvas").getContext("2d").getImageData(0, 0, 1, 1).data[0] === 255);
    await page.keyboard.up("ArrowRight");
    await page.getByRole("button", { name: "开启声音" }).click();
    await page.waitForFunction(() => window.audibleBuffers > 0);
    await page.getByRole("button", { name: "暂停", exact: true }).click();
    assert.match(await page.locator("#save-status").textContent(), /已保存在/);
    const download = page.waitForEvent("download");
    await page.getByRole("button", { name: "导出存档" }).click();
    const save = await fs.readFile(await (await download).path());
    assert.equal(save[0], 31);

    await page.reload();
    await page.waitForFunction(() => !document.querySelector("#open").disabled);
    await page.locator("#rom").setInputFiles(testRom());
    await page.waitForFunction(() => document.querySelector("#save-status").textContent.includes("已恢复"));
    const restored = page.waitForEvent("download");
    await page.getByRole("button", { name: "导出存档" }).click();
    assert.deepEqual(await fs.readFile(await (await restored).path()), save);
    const right = page.getByRole("button", { name: "右", exact: true });
    await right.hover();
    await page.mouse.down();
    await page.waitForFunction(() => document.querySelector("canvas").getContext("2d").getImageData(0, 0, 1, 1).data[0] === 255);
    await page.mouse.up();
    const imported = Buffer.from(save);
    imported[0] = 0xE0;
    await page.locator("#save-file").setInputFiles({
      name: "导入.sav", mimeType: "application/octet-stream", buffer: imported,
    });
    await page.waitForFunction(() => {
      const pixel = document.querySelector("canvas").getContext("2d").getImageData(1, 0, 1, 1).data;
      return pixel[0] === 0 && pixel[1] > 0;
    });
    await page.getByRole("button", { name: "开启声音" }).click();
    await page.evaluate(() => { window.simulateSlowCore = true; });
    await page.waitForFunction(() => window.framesPerRefresh.filter(n => n > 0).length >= 6);
    assert.equal(await page.evaluate(() => Math.max(...window.framesPerRefresh)), 1);
    await page.waitForFunction(() => window.slowAudioRates.some(rate => rate < 0.99));
    assert.ok(await page.evaluate(() => window.slowAudioRates.every(rate => rate >= 0.75 && rate <= 1)));
    assert.deepEqual(failures, []);
    console.log("浏览器通过：启动、两种按键、存档恢复、慢帧时优先刷新与音频缓冲。");
  } finally {
    if (browser) await browser.close();
    server.close();
  }
}

run().catch(error => { console.error(error); process.exitCode = 1; });
