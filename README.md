# gb-emu —— 从零手写的 Game Boy 模拟器

> **在线试玩（作品展示站）**：<https://leehomn.github.io/gb-emu/>

一个用 Rust 写的 Game Boy（DMG）模拟器核心，配套一个纯 JS 的网页版，能跑 Adjustris、2048、扫雷等开源游戏。

这是我学习「模拟器到底怎么工作」的作品：从 CHIP-8（Python）→ Rust → Game Boy，CPU、中断、定时器、PPU 全部从零实现，每一行都能解释为什么这么写。

## 已实现

| 模块 | 内容 |
|---|---|
| CPU | SM83（类 Z80）主指令集 + CB 前缀位操作，取指/译码/执行循环 |
| 中断 | VBlank / LCD / Timer / Serial / Joypad 五个向量，IME + IE/IF 逻辑 |
| 定时器 | DIV + TIMA/TMA/TAC，按 TAC 分频累加、溢出回装 |
| 串口 | 0xFF01/0xFF02，把测试 ROM 的 `ok` 结果吐到终端 |
| PPU | 背景层（BG）+ Window + 精灵层（OBJ），含 SCX/SCY 滚动、图块映射、调色板、DMA、STAT 中断 |
| MBC | MBC1 换页 + SRAM 存档（.sav 落盘 / 网页 localStorage） |
| APU | 4 通道（方波×2 + 波形 + 噪声）+ 帧序列器，WAV 输出 / 网页 Web Audio |
| 验证 | Blargg CPU 指令考卷 **75/75 全过** + Mooneye MBC1 **13 卷全过** |

## 怎么跑

### Rust 版（真代码）

```powershell
# 环境：需要 Rust 工具链
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"

# 跑 Blargg 考卷（75 个指令测试逐个打印 ok）
cargo run --release -- roms/cpu_instrs.gb

# 出截图
cargo run --release -- roms/cpu_instrs.gb --screenshot exam.ppm

# 单元测试
cargo test
```

> 注意：`--play` 实时窗口用的是 minifb，在 Windows 上键盘事件收不到，**玩游戏请用网页版**。

### 网页版（推荐玩）

直接双击打开 `web/index.html`，点「▶玩 Adjustris」「▶玩 2048」或「▶玩扫雷」。

- 方向键：移动
- X：旋转 / 确认
- Z：快速下落 / 返回
- Enter：开始

网页版是单文件 HTML，JS 版 Bus/Cpu/Ppu 逐行移植自 Rust 版，内置游戏 ROM 以 base64 内嵌。

### 无头验证（Node）

```powershell
node verify_homebrew.js
```

跑 500 万条指令后检查 PPU 是否画出内容。

## 目录结构

```
gb-emu/
├── src/
│   ├── main.rs       # CPU、总线、中断、定时器、串口、单元测试
│   ├── ppu.rs        # 背景层 + 精灵层渲染
│   └── apu.rs        # 音频（4 通道 + 帧序列器，WAV 输出）
├── web/
│   └── index.html    # 网页版（单文件，可玩 Adjustris / 2048 / 扫雷）
├── roms/             # 测试 ROM（Blargg 考卷等）
├── roms_homebrew/    # 内置开源游戏 ROM（见「关于 ROM」）
├── verify_homebrew.js# 网页版无头验证脚本
└── assets/img/       # 成果截图（exam.png / screen2048.png / screenminesweep.png）
```

## 关于 ROM

- `roms/cpu_instrs.gb`、`roms/02-interrupts.gb` 等是 **Blargg 测试 ROM**（Shay Green 开源），可从 gbdev 官网下载。
- 网页版内置的游戏均为**开源 / homebrew 作品**：
  - **Adjustris**（tbsp/Adjustris）—— CC0-1.0
  - **2048**（Sanqui/2048-gb）—— Zlib License
  - **扫雷**（lancekindle/minesweepGB）—— GPL-3.0
- 商业游戏 ROM 受版权保护，**本仓库不包含、不传播**。请自备合法取得的 ROM 文件，用「📁 加载 ROM」自行载入。

## 学习路线

```
CHIP-8（35 条指令，Python）
  → Rust 语法
  → GameBoy CPU 全指令集
  → Blargg 考卷 75/75
  → PPU 显像（BG + Window + OBJ + STAT 中断）
  → MBC1 换页 + SRAM 存档（Mooneye 13 卷全过）
  → APU 音频（4 通道 + 帧序列器）
  → 网页版跑开源游戏（Adjustris / 2048 / 扫雷），能玩有声
```

## 里程碑

- [x] ① CPU 骨架：取指 / 译码 / 执行
- [x] ② CPU 全指令集 + CB 前缀
- [x] ③ Blargg 考卷 75/75
- [x] ④ 显像：BG + Window + OBJ + STAT 中断，开源游戏可玩
- [x] ⑤ 音频（APU 4 通道 + 帧序列器）
- [x] ⑥ MBC1 换页 + SRAM 存档（Mooneye 13 卷全过）
