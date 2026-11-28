# RustBoy · 用 Rust 从零写 GB、GBC 和 GBA 模拟器

从 Python 写 CHIP-8 的取指循环开始，我用 Rust 实现了 Game Boy 的 CPU、画面、声音和卡带。接着给 Game Boy Color 加入彩色与双倍速，再从 ARM/Thumb 指令起步做 Game Boy Advance。现在三个机型都能在浏览器里运行，GBA 正在补时序精度和游戏兼容性。

[打开作品站](https://leehomn.github.io/gb-emu/) · [直接打开 GBA 播放器](https://leehomn.github.io/gb-emu/gba-cpu/web/) · [项目复盘](./POSTMORTEM.md)

## 在线游玩

| 机型 | 怎么开始 | 当前进度 |
| --- | --- | --- |
| **Game Boy** | 在[游玩区](https://leehomn.github.io/gb-emu/#play)启动模拟器，可玩 Adjustris、2048 和扫雷 | SM83 指令、逐周期画面、音频、MBC1/3/5 和卡带存档 |
| **Game Boy Color** | 用同一个播放器载入带 CGB 标记的卡带，自动切换彩色模式 | 彩色图层、显存与工作内存分区、双倍速、普通及 HBlank 显存 DMA |
| **Game Boy Advance** | 在 [GBA 播放页](https://leehomn.github.io/gb-emu/gba-cpu/web/)选择本地 `.gba` 文件 | 独立的 Rust/Wasm 核心，已接入 ARM/Thumb、Mode 0–5、定时器、中断、DMA、声音和 SRAM/Flash/EEPROM 存档 |

GBA 网页支持键盘和触屏，声音通过「开启声音」打开。载入同一份 ROM 可恢复浏览器存档，也可以手动导入或导出 `.sav`。需要 BIOS 的卡带可在载入前选择自备文件。

## 第三方测试与回归

| 机型 | 测试 | 已记录结果 |
| --- | --- | --- |
| GB | Blargg CPU 指令 | **75/75** |
| GB | Mooneye | MBC1 **13/13**，MBC5 **8/8**，OAM DMA **6/6** |
| GB | [Mealybug Tearoom](https://github.com/mattcurrie/mealybug-tearoom-tests) | DMG 画面逐像素对比 **24/24** |
| GBC | [cgb-acid2](https://github.com/mattcurrie/cgb-acid2) | **23,040 个像素**与参考图一致 |
| GBA | [jsmolka ARM / Thumb](https://github.com/jsmolka/gba-tests) | 两个指令测试 ROM 均显示全部通过 |
| GBA | [mGBA Test Suite](https://github.com/mgba-emu/suite) | 移位 **140/140**，进借位 **93/93**，长乘法 **72/72**，时序 **1834/2020**，内存 **1506/1552**，BIOS 数学 **603/615** |
| GBA | [NanoBoyAdvance 显存时序 ROM](https://github.com/nba-emu/hw-test/tree/master/ppu/ram-access-timing) | 背景显存 **11 组**、精灵 **14 组**、调色板 **3 组**、显示控制 **4 组**。逐拍访问测试每组测量 **1,231 个位置** |

GBA 最近一次整套回归通过 **115 项**。Tonc 示例和开源 Pong 已验证画面与输入。mGBA 套件使用开源替代 BIOS。Mode 1 混合背景通过了项目内的 CPU、DMA 和消隐边界用例，逐拍真机对照仍待完成。 [GBA 验证与复现步骤](./gba-cpu/README.md)

## 本地运行

在仓库根目录运行 GB/GBC，命令里的 ROM 路径可换成自己的文件。GBA 使用独立的 Cargo 清单。

```powershell
cargo test --locked
cargo run --release --locked -- path/to/game.gb
cargo test --locked --manifest-path gba-cpu/Cargo.toml
```

第三方测试 ROM 和参考图的准备方式见 [GBA 测试说明](./gba-cpu/README.md) 和 [GB 项目复盘](./POSTMORTEM.md)。

| 路径 | 内容 |
| --- | --- |
| [`src/`](./src/) | GB/GBC 的 Rust 核心和命令行入口 |
| [`web/`](./web/) 与 [`emulator/`](./emulator/) | GB/GBC 网页播放器和作品站使用的页面 |
| [`gba-cpu/`](./gba-cpu/) | GBA 核心、测试、示例和网页播放器 |
| [`index.html`](./index.html) | RustBoy 作品站首页 |

## 开发路线

1. **画面时序**。Mode 0–5 已接入背景显存等待，并验证了精灵、调色板争用和显示控制锁存。下一步对照真机的行内寄存器变化与取数边界。
2. **CPU 与内存**。已修正非对齐 SRAM 批量访问、CPU 与 DMA 重叠、卡带预取和 DMA 首次访问等待。继续排查卡带预取与 DMA 交接、镜像读数和 BIOS 相关差异。
3. **整机与手机**。细化音频混音和 Flash 写入耗时，再分别验证实际手机上的触控、声音，以及更多卡带的启动、持续游玩和读档。

[查看 GBA 整机路线图](./gba-cpu/ROADMAP.md)

## 关于 ROM

- GB 的 Blargg 测试使用单独准备的测试 ROM。[测试源码和构建产物](https://github.com/retrio/gb-test-roms)
- 网页版内置的游戏均为**开源 / homebrew 作品**：
  - **Adjustris**（tbsp/Adjustris）—— CC0-1.0
  - **2048**（Sanqui/2048-gb）—— Zlib License
  - **扫雷**（lancekindle/minesweepGB）—— GPL-3.0
  - **Tobu Tobu Girl**（tangramgames）—— MIT（代码）+ CC BY 4.0（素材），MBC1 换页
  - **Gigant Golf**（quinnyo/gbcompo2023）—— GPLv3（代码）+ CC BY-SA（素材），纯 32KB ROM
- 商业游戏 ROM 受版权保护，**本仓库不包含、不传播**。请自备合法取得的 ROM 文件，用「📁 加载 ROM」自行载入。
