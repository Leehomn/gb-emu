# gb-emu —— 从零手写的 Game Boy 模拟器

> **在线试玩（作品展示站）**：<https://leehomn.github.io/gb-emu/>
> **项目复盘（为什么这么走、踩了哪些坑）**：见 [POSTMORTEM.md](./POSTMORTEM.md)

一个用 Rust 写的 Game Boy（DMG）模拟器核心，配套一个纯 JS 的网页版，能跑 Adjustris、2048、扫雷等开源游戏。

这是我学习「模拟器到底怎么工作」的作品：从 CHIP-8（Python）→ Rust → Game Boy，CPU、中断、定时器、PPU 全部从零实现，每一行都能解释为什么这么写。

## 已实现

| 模块 | 内容 |
|---|---|
| CPU | SM83（类 Z80）主指令集 + CB 前缀位操作，取指/译码/执行循环 |
| 中断 | VBlank / LCD / Timer / Serial / Joypad 五个向量，IME + IE/IF 逻辑 |
| 定时器 | DIV + TIMA/TMA/TAC，按 TAC 分频累加、溢出回装 |
| 串口 | 0xFF01/0xFF02，把测试 ROM 的 `ok` 结果吐到终端 |
| PPU | **逐 dot（T-cycle）状态机**：37 个主状态 + 19 个取数状态 + 双 FIFO，BG/Window/OBJ 三层，含 SCX/SCY 滚动、图块映射、背景调色板切换毛刺、逐周期 OAM DMA、STAT 中断 |
| MBC | MBC1/MBC3/MBC5 换页 + SRAM 存档 + RTC 寄存器（.sav 落盘 / 网页 localStorage） |
| APU | 4 通道（方波×2 + 波形 + 噪声）+ 帧序列器，WAV 输出 / 网页 Web Audio |
| 验证 | Blargg CPU 指令考卷 **75/75** + Mooneye MBC1 **13/13** + MBC5 **8/8** + cargo test **58 项** + Mooneye OAM DMA **6/6** + Mealybug Tearoom **24/24** |

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

### Mealybug Tearoom Tests（PPU 时序考卷）

24 张标准答案逐像素比对，DMG 套件 **24/24 通过**。

```powershell
# 前置：把测试 ROM 放进 _mealybug/extracted/、标准答案 PNG 放进 _mealybug/repo/expected/DMG-blob/
# （这两处都被 .gitignore 挡着，需要自己从 https://github.com/mattcurrie/mealybug-tearoom-tests 取）

# 一次跑全部 24 张
python -X utf8 .\_mealybug\compare.py

# 单张细看
.\target\release\gb-emu.exe .\_mealybug\extracted\m3_scy_change.gb --mealybug .\_mealybug\out\m3_scy_change.raw
python -X utf8 .\_mealybug\diff_rows.py m3_scy_change
python -X utf8 .\_mealybug\ascii_diff.py m3_scy_change
```

这套测试比 Blargg 难得多：它专测「寄存器在一行渲染中途被改写」这类边界时序，需要 PPU 逐 dot 推进才能还原。旧版按扫描线整行渲染，一张都过不了。

## 目录结构

```
gb-emu/
├── src/
│   ├── lib.rs        # CPU（全指令集+CB）、总线、中断、定时器、串口、MBC、单元测试
│   ├── ppu.rs        # PPU：逐 dot 状态机 + 双 FIFO + 取数器
│   ├── apu.rs        # 音频（4 通道 + 帧序列器，WAV 输出）
│   ├── main.rs       # CLI 入口（--screenshot / --play / --wav / --mealybug）
│   └── wasm.rs       # C ABI 导出层（只在 wasm32 目标编译）
├── web/
│   └── index.html    # 网页版（单文件，可玩 5 款开源游戏）
├── emulator/         # web/ 的副本，供展示站 iframe 载入
├── index.html        # 作品展示站（RustBoy）
├── roms/             # 测试 ROM（Blargg 考卷等）
├── roms_mbc/         # Mooneye MBC1 考卷
├── roms_mbc5/        # Mooneye MBC5 考卷
├── roms_homebrew/    # 内置开源游戏 ROM（见「关于 ROM」）
├── _mealybug/        # Mealybug 攻坚台（脚本入库，ROM/标准答案/参照实现不入库）
├── verify_homebrew.js# 网页版无头验证脚本
└── assets/img/       # 成果截图（exam.png / screen2048.png / screenminesweep.png）
```

## 关于 ROM

- `roms/cpu_instrs.gb`、`roms/02-interrupts.gb` 等是 **Blargg 测试 ROM**（Shay Green 开源），可从 gbdev 官网下载。
- 网页版内置的游戏均为**开源 / homebrew 作品**：
  - **Adjustris**（tbsp/Adjustris）—— CC0-1.0
  - **2048**（Sanqui/2048-gb）—— Zlib License
  - **扫雷**（lancekindle/minesweepGB）—— GPL-3.0
  - **Tobu Tobu Girl**（tangramgames）—— MIT（代码）+ CC BY 4.0（素材），MBC1 换页
  - **Gigant Golf**（quinnyo/gbcompo2023）—— GPLv3（代码）+ CC BY-SA（素材），纯 32KB ROM
- 商业游戏 ROM 受版权保护，**本仓库不包含、不传播**。请自备合法取得的 ROM 文件，用「📁 加载 ROM」自行载入。

## 学习路线

```
CHIP-8（35 条指令，Python）
  → Rust 语法
  → GameBoy CPU 全指令集
  → Blargg 考卷 75/75
  → PPU 显像（BG + Window + OBJ + STAT 中断）
  → MBC1 换页 + SRAM 存档（Mooneye 13 卷全过）
  → MBC3/MBC5 换页 + RTC 寄存器（Mooneye MBC5 8 卷全过）
  → APU 音频（4 通道 + 帧序列器）
  → 网页版跑开源游戏（Adjustris / 2048 / 扫雷），能玩有声
  → PPU 逐 dot 重写（Mealybug 时序考卷 24/24）
```

## 里程碑

- [x] ① CPU 骨架：取指 / 译码 / 执行
- [x] ② CPU 全指令集 + CB 前缀
- [x] ③ Blargg 考卷 75/75
- [x] ④ 显像：BG + Window + OBJ + STAT 中断，开源游戏可玩
- [x] ⑤ 音频（APU 4 通道 + 帧序列器）
- [x] ⑥ MBC1 换页 + SRAM 存档（Mooneye 13 卷全过）
- [x] ⑦ MBC3/MBC5 换页 + RTC 寄存器（Mooneye MBC5 8 卷全过）
- [x] ⑧ PPU 逐 dot 时序：Mealybug Tearoom DMG 套件 **24/24**
