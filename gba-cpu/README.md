# GBA 模拟器核心

这是用 Rust 自研的 GBA 学习项目。最小演示执行四条 ARM 指令，把工作内存里的 7 和 5 相加，再写回 12。

在本目录运行：

```powershell
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo run --offline --locked
cargo test --offline --locked
```

程序从卡带窗口 `0x08000000` 取指，`r3` 指向工作内存 `0x02000000`。两次 LDR 读入 r0 和 r1，ADD 写入 r2，STR 把 r2 写到 `0x02000008`。这个演示里的 ARM 指令顺序执行，取指地址每条前进 4 字节。

## 已实现的范围

- ARM 与 Thumb 指令覆盖算术、逻辑、移位、乘加、访存、块传输与跨模式调用，并处理 CPSR/SPSR 和各模式的独立寄存器。
- CPU 分别推进取指、访存和内部周期。CPU 内部运算可和 DMA 同拍推进，取指或访存时等待 DMA 交还总线。使用两级预取保存下一条和再下一条指令，并在跳转、模式切换和异常时从新地址重取。卡带访问区分顺序与非顺序等待，读取 `WAITCNT` 的配置并模拟八个半字的预取缓冲。
- 启动支持从地址 0 执行自备的 16 KiB BIOS，或初始化常用栈后直接运行卡带。已验证 SWI、未定义指令、IRQ 的向量进入与返回，以及 HALT 和按键唤醒 STOP。
- 视频：Mode 0–5 按扫描线生成画面。Mode 0–2 支持普通图块和仿射背景，Mode 3–5 支持位图变换，Mode 4、5 可切页。支持 4bpp 和 8bpp 精灵、1D 和 2D 图块编址、仿射和倍尺寸裁剪。支持背景和精灵马赛克、矩形窗口、精灵窗口、混色和亮度特效。
- 外设包括四个定时器、显示和定时器中断、四通道普通 DMA、DMA1/2 音频补给及 DMA3 显示同步传输。
- 音频支持 A/B 数字采样、两路方波、波表和噪声，以 32,768 Hz 输出立体声。网页可播放。
- 存档支持 32 KiB SRAM、64/128 KiB Flash 和 512 B/8 KiB EEPROM。桌面和网页可读取和保存原始存档。

后续阶段见 [整机开发路线](ROADMAP.md)。本原型已验证部分开源 GBA ROM，商业游戏兼容性仍待验证。

自制指令 ROM 可用 `cargo run --offline --locked -- --rom demo.gba 8 frame.ppm` 执行指定数量的指令，并输出 Mode 3 静态画面。输出文件需使用新路径。遇到未实现的指令会报告取指地址。

## 方向键方块演示

运行 `cargo run --offline --locked --example square`。点一下窗口获取焦点，用方向键移动红色方块，按 Esc 退出。

桌面窗口每次只传入按键状态并显示画面。自制 ARM 程序读取 `KEYINPUT`（地址 `0x04000130`），通过 `TST` 判断按键并决定新位置，再用 `STR` 擦掉旧方块、画出新方块。自动测试检查四个方向、松开按键和屏幕边界。每次窗口刷新对应一轮演示程序循环。普通卡带按硬件的扫描线事件推进整帧。

尚待完善的功能包括图块与精灵数据的逐拍锁存、卡带访存边界、音频 PWM 与精确混音、Flash 写入和擦除耗时，以及更多卡带的兼容性。位图模式下，映射到背景区域的部分精灵显存镜像读取为零，写入会被忽略。

## 本地网页播放器

从本目录构建并启动，浏览器访问本机的 8766 端口。

```powershell
rustup target add wasm32-unknown-unknown
./tools/build_web.ps1 --offline
python -X utf8 -m http.server 8766 --bind 127.0.0.1 --directory web
```

选择本地 `.gba` 文件开始运行，按「开启声音」播放。可以通过键盘或屏幕按键操作。需要 BIOS 的卡带可先选择自备的 16 KiB BIOS，再载入卡带。网页在本机执行 ROM，存档按 ROM 内容隔离，刷新后重新选择同一份卡带即可恢复。可手动导出 `.sav`，导入有效存档后会重新启动当前卡带。

桌面版在 ROM 旁读取 `游戏.gba.sav`。关窗、按 Esc 或模拟器报告运行错误时，若存档有变化，会先写临时文件再替换旧存档。Flash 和 EEPROM 根据 ROM 内的常见存档库标识识别。缺少标识时沿用 SRAM。Flash 当前模拟两种 Macronix 容量，编程和擦除即时完成。桌面版已接入画面、按键和存档，立体声播放由网页版提供。

安装 Playwright 及其 Chromium 后，可运行 `node tests/web_player.cjs`。测试使用自制 ROM 验证画面、按键、非静音采样、导出和刷新后恢复。

设备短时赶不上模拟速度时，播放器会先显示已完成的画面，再继续补帧。音频缓冲不足时会暂时降低播放速度，缓冲恢复后逐步回到原速。降速会降低音高。

## 独立汇编验证

使用 Keystone 0.9.2 汇编自制演示。测试程序从 ARM 切到 Thumb，调用函数，画出第一个红色像素和第二个蓝色像素。

```powershell
python -X utf8 tools/assemble_demo.py demo.gba
cargo run --offline --locked -- --rom demo.gba 24 frame.ppm
```

构建脚本需要可导入 Keystone。Rust 模拟器无需安装它即可运行 ROM。示例 ROM 由模拟器直接从卡带地址取指，用来验证核心执行路径。

## 第三方指令测试

使用 [jsmolka/gba-tests](https://github.com/jsmolka/gba-tests) 的 ARM 和 Thumb ROM 从卡带入口启动，两个结果页均显示 “All tests passed”。ROM 保存在构建目录，未纳入源码。该套件主要验证指令行为，整机游戏兼容性仍待验证。

```powershell
cargo run --release --offline --locked -- --rom target/jsmolka/arm.gba 204000 arm-result.ppm
cargo run --release --offline --locked -- --rom target/jsmolka/thumb.gba 204000 thumb-result.ppm
```

## CPU 时序和 BIOS 验收

从复位向量运行 [Cult-of-GBA 开源替代 BIOS](https://github.com/Cult-of-GBA/BIOS/tree/a30e9a96df083628b650724b7d4d7112b4070b98)，启动 [mGBA 测试套件](https://github.com/mgba-emu/suite)。移位 **140/140**、进借位 **93/93**、长乘法 **72/72**，时序 **1834/2020**，内存 **1506/1552**。剩余差异见 [路线图](ROADMAP.md)。

+ 开源替代 BIOS 保存为 `target/cult-gba-bios.bin`，SHA-256 为 `61af6e8c2db6cf24aa6924e8133f6a50833158fca33ff08ea5e11e1a06e132f2`。
+ [预编译测试 ROM](https://raw.githubusercontent.com/Asphaltian/sgba/main/Assets/roms/suite.gba) 保存为 `target/mgba-suite.gba`，本次运行的 SHA-256 为 `c8f9846058af9472c321000900ceb57e14fb42ca05d77b17eb7cbe8736c85512`。

```powershell
cargo test --release --offline --locked --test cpu --test cpu_roms -- --include-ignored --nocapture
cargo run --release --offline --locked --example play_rom -- game.gba bios.bin
```

桌面命令省略第二个文件时直接启动卡带。从复位到套件菜单已用开源替代 BIOS 验证，任天堂原版 BIOS 仍待使用自备副本验证。无 BIOS 的简化服务覆盖 Halt、Stop 和 VBlankIntrWait。

## 第三方 Pong 验收

从 [作者仓库](https://github.com/ZeroDayArcade/Pong-Homebrew-GBA) 下载 MIT 许可的 `Pong-Homebrew-GBA.gba`，保存为 `target/pong-homebrew.gba`。本次验收的 ROM SHA-256 是 `3c850319ddef6d237756dd64b85ddf3b75bf7c096feb2408ed1b2dd0a05a0158`。

```powershell
cargo run --release --offline --locked --example play_rom -- target/pong-homebrew.gba
cargo test --release --offline --locked --test pong -- --ignored --nocapture
```

桌面窗口按 Enter 开局，开球倒计时结束后用上下键移动球拍，Esc 退出。测试自动进入对局，比较无输入和上下按键三种情况下的球拍像素位置，并输出 `target/pong-verified.ppm`。

无 BIOS 时，`VBlankIntrWait` 的当前实现会直接推进到下一次 VBlank，期间继续更新扫描线、定时器和 DMA。这个近似省略了 BIOS 与游戏之间的中断处理流程。

## Tonc 图块背景验收

从 [Tonc 作者的下载页](https://www.coranac.com/projects/#tonc) 获取 v1.4.2 的演示包，把其中的 `bin/sbb_reg.gba` 解压到 `target/bin/sbb_reg.gba`。验收用 ROM 的 SHA-256 为 `1b429f9b0a5b91245925b8ae568ae8e2f419a3f1d1a56d16eb7be245f35c88be`。

```powershell
cargo run --release --offline --locked --example play_rom -- target/bin/sbb_reg.gba
cargo test --release --offline --locked --test video tonc_sbb_reg -- --ignored --nocapture
```

初始画面显示红色地图块。按住左上方向后，视野绕过地图原点，能同时看到红、绿、蓝、灰四个地图块。测试通过卡带启动、注入按键，并检查扫描输出中的四种颜色。

## 仿射背景验收

```powershell
cargo test --release --offline --locked --test video
```

自制 ARM 卡带在 Mode 2 循环读取 R 肩键。按下时把背景旋转 90°，松开时恢复，测试比较同一屏幕坐标的颜色。另有像素测试验证 Mode 1 的普通背景与仿射背景叠放、缩放和边缘回绕，Mode 2 的双层优先级，以及帧中改写参考点。

从 Tonc v1.4.2 演示包将 [`bin/sbb_aff.gba`](https://gbadev.net/tonc/affbg.html) 解压到 `target/bin/sbb_aff.gba`。验收用 ROM 的 SHA-256 为 `13cf10496eedd894aec56d7b0eba3cc9403c6de598bfe9a1432874f5f9fe5e80`。

```powershell
cargo run --release --offline --locked --example play_rom -- target/bin/sbb_aff.gba
cargo test --release --offline --locked --test video tonc_sbb_aff -- --ignored --nocapture
```

桌面窗口按 S 对应 GBA 的 R 肩键，旋转背景。自动测试对比说明框上方的地图像素，确认无输入时画面稳定，按住 R 后地图发生变化。

## Tonc 矩形窗口验收

从同一个 v1.4.2 演示包将 `bin/win_demo.gba` 解压到 `target/bin/win_demo.gba`。验收用 ROM 的 SHA-256 为 `1bb86a5af8c0653c941047194d21598c34892e86e4e0c63b2c4ec64150376e1a`。

```powershell
cargo run --release --offline --locked --example play_rom -- target/bin/win_demo.gba
cargo test --release --offline --locked --test video tonc_win_demo -- --ignored --nocapture
```

方向键移动透视窗口。自动测试读取第 25 行的像素，验证按住右方向后透视窗口向右移动，且第 0 行保持原样。

## 精灵窗口验收

4bpp 或 8bpp 精灵标记为 OBJ Window 后，其非零像素形成遮罩，透明像素保留空洞，仿射精灵的遮罩会一起变换。窗口内使用 `WINOUT` 的高字节设置图层和特效。多个精灵窗口的形状合并，重叠时依次由 WIN1、WIN0 覆盖。

```powershell
cargo test --release --offline --locked --test video obj_window
```

## 亮度特效验收

`BLDCNT` 选择特效及目标图层，`BLDY` 设置提亮或压暗的强度，`BLDALPHA` 设置透明混合的两个权重。扫描线合成保留最上层和紧挨着的下一层像素，窗口和图层标志决定是否启用普通混色。支持半透明 4bpp 精灵与背景或底色混合。Mode 4 的调色板索引 0 作为透明像素参与图层合成。

```powershell
cargo test --release --offline --locked --test video brightness
```

### Tonc 透明混色验收

从 Tonc v1.4.2 演示包解压 `bin/bld_demo.gba` 到 `target/bin/bld_demo.gba`。验收用 ROM 的 SHA-256 为 `2990fbe308b5d5b28117eecf8721b36847e3d561ff08fe6e32bb1ee0eb5bc472`。

```powershell
cargo run --release --offline --locked --example play_rom -- target/bin/bld_demo.gba
cargo test --release --offline --locked --test video tonc_bld_demo -- --ignored --nocapture
```

桌面窗口按 S（R 肩键）进入混色模式，按左、上调整前后层权重。自动测试验证权重变化及精灵区域的像素变化。

## Mode 5、仿射精灵和马赛克验收

从 Tonc v1.4.2 演示包将 `bin/obj_aff.gba` 和 `bin/mos_demo.gba` 解压到 `target/bin/`。桌面窗口运行 `obj_aff` 后按 S 旋转精灵。运行 `mos_demo` 后按右方向调整精灵马赛克，按 Z 加右方向调整背景马赛克。

```powershell
cargo test --release --offline --locked --test video
cargo test --release --offline --locked --test video -- --ignored --nocapture
```

像素测试验证 Mode 5 的双页、160×128 的边界和位图旋转，8bpp 精灵的调色板和两种寻址方式，仿射精灵窗口的矩阵和裁剪，以及背景与精灵的马赛克取样。还验证了相邻像素绿色通道交换，以及 HBlank 设置降低单行精灵取样预算时的裁剪。

### 扫描线内分段取样

写显示寄存器、调色板、显存或 OAM 前，模拟器以每 4 拍一个像素结算当前行已经画完的部分。可见的 240 个像素画完后，整行在第 1006 拍进入 HBlank 时发布，并在发布后请求 HBlank DMA。测试覆盖行中改亮度、调色板、滚动和显存，以及 DMA 改下一行的滚动。

### 背景显存争用

Mode 0 的 BG0–BG3 分用四拍中的不同位置。8 像素的图块先读编号，再按色深读取 2 个或 4 个半字的数据。横向滚动的低 3 位改变读取的相位。第 1004–1006 拍可以读图块编号，此前开始的图块数据读取可以延续到 HBlank。当前按访存时的横向滚动量和色深计算，行内改写寄存器时的锁存与取数过程待细化。 [NanoBoyAdvance 图块取数参考](https://github.com/nba-emu/NanoBoyAdvance/blob/master/src/nba/src/hw/ppu/background.inl#L3-L119)

Mode 1 的 BG0、BG1 沿用普通图块的取数位置，仿射 BG2 从第 34 拍起占用每四拍的后两拍。CPU 和 DMA 等到任一背景都不用显存的时刻再访问。普通背景可以在行消隐中读完已经开始的图块，仿射 BG2 在行消隐开始时释放总线。

Mode 2 从每行第 32 拍开始，四拍一组，BG3 读取前两拍，BG2 读取后两拍。两个背景同时开启时，CPU 或 DMA 与它们争用背景显存会等到第 1006 拍进入 HBlank。Mode 3–5 的 BG2 从第 35 拍起每四拍读取一次。32 位访问为两个半字分别找空位。强制空白、VBlank 及精灵显存区跳过背景等待。仿射取样地址变化和双半字写入分别生效的时刻仍待细化。 [NanoBoyAdvance 背景取数参考](https://github.com/nba-emu/NanoBoyAdvance/blob/master/src/nba/src/hw/ppu/background.cc#L79-L104)

从 [公开测试项目](https://github.com/nba-emu/hw-test/tree/master/ppu/ram-access-timing) 下载 `ram-access-timing.gba`，保存为 `target/ppu-ram-access-timing.gba`。验收文件的 SHA-256 为 `b7a7b370721c771c6ccf9e1a338b475e1881890f37d5c9a672bc67e92b89de12`。

```powershell
cargo test --release --offline --locked --test cpu_roms hardware_rom_measures -- --include-ignored --nocapture
```

## DMA 总线时序

从 SRAM 读取时，DMA1–3 将对齐地址处的单字节重复填满半字或整字。DMA3 写 SRAM 时只写入对齐地址处的最低字节。普通存档区 0x0E 与 0x0F 镜像使用相同规则。

立即、HBlank 和 VBlank 请求进入同一队列。启动后，每个 16 位或 32 位传输单位分别计入读取和写入耗时，卡带访问使用 `WAITCNT`。多个通道就绪时，编号最小的优先，下一个传输单位可以换通道。最后一次写入后，DMA 走完收尾周期，再清除非重复传输的启用位并按设置请求 IRQ。HBlank 重复传输可重载目标地址。

DMA 占用总线时，CPU 等到总线可用才继续，显示、定时器和音频时钟仍推进。纯内存 DMA 期间，空闲的卡带总线继续填充指令预取。DMA 读取或写入卡带时清空预取。交回总线后，CPU 可以消费已预取的指令。若首笔取指需要直接访问卡带，则使用非顺序等待。预取即将完成时是否需要额外等待，仍待逐拍验证。宿主可以用 `Bus::advance` 推进指定的硬件周期，再读取 DMA 传输结果。

### 精灵、调色板与显示控制时序

精灵引擎从每行第 40 拍开始准备下一条可见扫描线，按两拍一个时隙读取 OAM 属性、仿射矩阵和精灵瓦片。CPU 和 DMA 分别等待 OAM 或精灵显存释放。打开 HBlank 空闲位会缩短精灵的取数窗口。强制空白时，精灵引擎仍由当前精灵启用位控制。 [NanoBoyAdvance 精灵取数参考](https://github.com/nba-emu/NanoBoyAdvance/blob/master/src/nba/src/hw/ppu/sprite.cc)

调色板按画面合成需求抢占总线。底色和索引色占用主颜色时隙，透明混色才按需读取第二个颜色。Mode 3 和 Mode 5 的 RGB555 像素可以直接使用显存中的颜色。 [NanoBoyAdvance 画面合成参考](https://github.com/nba-emu/NanoBoyAdvance/blob/master/src/nba/src/hw/ppu/merge.cc)

显示控制寄存器每行第 40 拍采样。背景启用和退出强制空白要等采样值经过三段锁存，关闭背景或开启强制空白立即生效。实时输出会在锁存改变时结算已完成的画面。图块数据、精灵像素和调色板占用仍有部分依靠分段渲染的取样结果，寄存器在取数途中改写的极端情况待进一步对照真机。 [NanoBoyAdvance 锁存参考](https://github.com/nba-emu/NanoBoyAdvance/blob/master/src/nba/src/hw/ppu/ppu.cc#L234-L237)
