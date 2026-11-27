const $ = (id) => document.getElementById(id);
const screen = $("screen");
const canvas = screen.getContext("2d");
const image = canvas.createImageData(240, 160);
for (let i = 3; i < image.data.length; i += 4) image.data[i] = 255;

const keyMap = new Map([
  ["KeyZ", 0], ["KeyX", 1], ["ShiftRight", 2], ["Enter", 3],
  ["ArrowRight", 4], ["ArrowLeft", 5], ["ArrowUp", 6], ["ArrowDown", 7],
  ["KeyS", 8], ["KeyA", 9],
]);
const held = new Map();
const frameDuration = 1000 * 280896 / 16777216;
let core;
let game = null;
let paused = false;
let loading = false;
let lastTime = 0;
let elapsed = 0;
let audio;
let audioGain;
let audioTime = 0;
let audioRate = 1;
let soundEnabled = false;

function sendBytes(bytes) {
  const address = core.input_buffer(bytes.length);
  if (address === 0) throw new Error("文件太大");
  new Uint8Array(core.memory.buffer, address, bytes.length).set(bytes);
}

function readSave() {
  return new Uint8Array(core.memory.buffer, core.save_address(), core.save_size());
}

function encode(bytes) {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 8192) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 8192));
  }
  return btoa(binary);
}

function storeSave() {
  if (!game) return;
  const data = encode(readSave());
  if (data === game.previousSave) return;
  try {
    localStorage.setItem(game.key, data);
    game.previousSave = data;
    $("save-status").textContent = "进度已保存在当前浏览器。";
  } catch {
    $("save-status").textContent = "浏览器存档写入失败，可以手动导出。";
  }
}

async function loadCartridge(file) {
  if (file.size === 0 || file.size > 32 * 1024 * 1024) throw new Error("请选择不超过 32 MiB 的 GBA 卡带");
  const bytes = new Uint8Array(await file.arrayBuffer());
  const digest = new Uint8Array(await crypto.subtle.digest("SHA-256", bytes));
  const key = "gba-save-" + Array.from(digest, (byte) => byte.toString(16).padStart(2, "0")).join("");
  const bios = $("bios").files[0];
  if (bios) {
    if (bios.size !== 16384) throw new Error("BIOS 必须是 16 KiB");
    sendBytes(new Uint8Array(await bios.arrayBuffer()));
    if (!core.use_bios(true)) throw new Error("BIOS 加载失败");
  } else {
    core.use_bios(false);
  }
  flushSave();
  sendBytes(bytes);
  if (!core.load_rom()) throw new Error("卡带加载失败");

  game = { name: file.name, key, rom: bytes, previousSave: encode(readSave()) };
  $("save-status").textContent = "存档保留在当前浏览器。";
  try {
    const stored = localStorage.getItem(key);
    if (stored !== null) {
      sendBytes(Uint8Array.from(atob(stored), (character) => character.charCodeAt(0)));
      if (!core.load_save()) throw new Error("大小错误");
      game.previousSave = stored;
      $("save-status").textContent = "已恢复浏览器存档。";
    }
  } catch {
    $("save-status").textContent = "无法读取浏览器存档，可以手动导入。";
    game.protectSave = true;
  }
  resumeGame();
}

function resumeGame() {
  held.clear();
  paused = false;
  elapsed = 0;
  audioTime = 0;
  audioRate = 1;
  $("pause").textContent = "暂停";
  $("status").textContent = game.name;
  screen.focus({ preventScroll: true });
}

function playSamples() {
  if (!soundEnabled || audio.state !== "running") return;
  const count = core.sample_count();
  if (count === 0) return;
  const now = audio.currentTime;
  if (audioTime <= now) audioTime = now + 0.060;
  const queued = audioTime - now;
  if (queued > 0.15) return;
  // 余量不足才慢放，等模拟速度恢复后逐渐回到原播放速度。
  const targetRate = 1 - 0.25 * Math.max(0, 1 - queued / 0.050);
  audioRate += (targetRate - audioRate) * 0.2;
  const samples = new Int16Array(core.memory.buffer, core.samples(), count * 2);
  const output = audio.createBuffer(2, count, 32768);
  for (let channel = 0; channel < 2; channel++) {
    const data = output.getChannelData(channel);
    for (let i = 0; i < count; i++) data[i] = samples[2 * i + channel] / 32768;
  }
  const source = audio.createBufferSource();
  source.buffer = output;
  source.playbackRate.value = audioRate;
  source.connect(audioGain);
  source.start(audioTime);
  audioTime += output.duration / audioRate;
}

function render() {
  const rgb = new Uint8Array(core.memory.buffer, core.pixels(), 240 * 160 * 3);
  for (let pixel = 0; pixel < 240 * 160; pixel++) {
    image.data[4 * pixel] = rgb[3 * pixel];
    image.data[4 * pixel + 1] = rgb[3 * pixel + 1];
    image.data[4 * pixel + 2] = rgb[3 * pixel + 2];
  }
  canvas.putImageData(image, 0, 0);
}

function animate(now) {
  const delta = Math.min(now - lastTime, 80);
  lastTime = now;
  if (game && !paused && !loading) {
    elapsed += delta;
    const keys = Array.from(held.values()).reduce((mask, bit) => mask | (1 << bit), 0);
    const deadline = performance.now() + frameDuration;
    let frames = 0;
    while (elapsed >= frameDuration && frames < 4) {
      if (!core.frame(keys)) {
        paused = true;
        $("pause").textContent = "继续";
        $("status").textContent = `卡带暂停在 0x${core.pc().toString(16).padStart(8, "0")}`;
        break;
      }
      elapsed -= frameDuration;
      frames++;
      playSamples();
      // 单帧已经耗尽刷新时间时，先呈现画面，再在下一次刷新继续。
      if (performance.now() >= deadline) break;
    }
    if (frames > 0) render();
    // 后台标签或设备短时繁忙后，只保留少量待补帧。
    elapsed = Math.min(elapsed, frameDuration * 2);
  }
  requestAnimationFrame(animate);
}

function flushSave() {
  if (game && !game.protectSave) storeSave();
}

function updateButtons() {
  $("open").disabled = !core || loading;
  $("bios").disabled = !core || loading;
  for (const id of ["pause", "sound", "export", "import"]) $(id).disabled = !game || loading;
}

$("open").addEventListener("click", () => $("rom").click());
$("rom").addEventListener("change", async () => {
  const file = $("rom").files[0];
  $("rom").value = "";
  if (!file || loading) return;
  loading = true;
  updateButtons();
  try { await loadCartridge(file); } catch (error) { $("status").textContent = error.message; }
  finally { loading = false; updateButtons(); }
});

$("pause").addEventListener("click", () => {
  paused = !paused;
  elapsed = 0;
  $("pause").textContent = paused ? "继续" : "暂停";
  $("status").textContent = paused ? `${game.name} · 已暂停` : game.name;
  if (paused) flushSave();
  else screen.focus({ preventScroll: true });
});

$("sound").addEventListener("click", async () => {
  if (soundEnabled) {
    soundEnabled = false;
    audioGain.gain.value = 0;
  } else {
    try {
      if (!audio) {
        audio = new AudioContext();
        audioGain = audio.createGain();
        audioGain.connect(audio.destination);
      }
      await audio.resume();
      audioTime = 0;
      audioRate = 1;
      audioGain.gain.value = 1;
      soundEnabled = true;
    } catch {
      $("status").textContent = "当前浏览器无法播放声音。";
    }
  }
  $("sound").setAttribute("aria-pressed", String(soundEnabled));
  $("sound").textContent = soundEnabled ? "关闭声音" : "开启声音";
  screen.focus({ preventScroll: true });
});

$("import").addEventListener("click", () => $("save-file").click());
$("save-file").addEventListener("change", async () => {
  const file = $("save-file").files[0];
  $("save-file").value = "";
  if (!game || loading || !file) return;
  loading = true;
  updateButtons();
  try {
    if (file.size > 128 * 1024) throw new Error("存档容量与卡带不匹配。");
    const bytes = new Uint8Array(await file.arrayBuffer());
    sendBytes(bytes);
    if (!core.load_save()) throw new Error("存档容量与卡带不匹配。");
    // 游戏可能只在启动时读存档，重启后在第一帧前恢复。
    sendBytes(game.rom);
    if (!core.load_rom()) throw new Error("重启卡带失败。");
    sendBytes(bytes);
    if (!core.load_save()) throw new Error("恢复存档失败。");
    game.previousSave = null;
    game.protectSave = false;
    storeSave();
    resumeGame();
  } catch (error) { $("save-status").textContent = error.message; }
  finally { loading = false; updateButtons(); }
});

$("export").addEventListener("click", () => {
  const blob = new Blob([readSave()]);
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = game.name + ".sav";
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
});

for (const name of ["keydown", "keyup"]) {
  window.addEventListener(name, (event) => {
    if (!game || !keyMap.has(event.code)) return;
    if (event.type === "keydown" && (event.ctrlKey || event.altKey || event.metaKey ||
        event.target instanceof HTMLInputElement ||
        (event.code === "Enter" && event.target instanceof HTMLButtonElement))) return;
    event.preventDefault();
    if (event.type === "keydown") held.set(event.code, keyMap.get(event.code));
    else held.delete(event.code);
  });
}

for (const button of document.querySelectorAll("[data-key]")) {
  button.addEventListener("pointerdown", (event) => {
    button.setPointerCapture(event.pointerId);
    held.set(event.pointerId, Number(button.dataset.key));
  });
  for (const name of ["pointerup", "pointercancel", "lostpointercapture"]) {
    button.addEventListener(name, (event) => held.delete(event.pointerId));
  }
}

window.addEventListener("blur", () => held.clear());
window.addEventListener("pagehide", flushSave);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) { held.clear(); flushSave(); }
});
setInterval(flushSave, 1000);

try {
  const response = await fetch("./gba.wasm");
  if (!response.ok) throw new Error("找不到模拟器核心，请先编译网页版本。");
  const module = await WebAssembly.instantiate(await response.arrayBuffer());
  core = module.instance.exports;
  $("status").textContent = "请选择本地 .gba 卡带。";
  updateButtons();
  requestAnimationFrame(animate);
} catch (error) {
  $("status").textContent = error.message;
}
