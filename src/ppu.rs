// PPU —— GameBoy 的第二颗芯片：按扫描线画图
// 屏幕 160×144 像素，每帧 456 周期/线 × 154 线（144 可见 + 10 行 VBlank）。
// 本批只做背景图（BG），精灵（OBJ）和窗口（Window）下批再说。
// 输出 framebuffer：160×144，每像素 0~3 的色号，外壳负责上色。

pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;

pub struct Ppu {
    pub framebuffer: [[u8; SCREEN_W]; SCREEN_H],
    pub line_cycles: u32,   // 本扫描线已走周期
    pub ly: u8,             // 当前行号（0~153）
}

impl Ppu {
    pub fn new() -> Self {
        Ppu { framebuffer: [[0; SCREEN_W]; SCREEN_H], line_cycles: 0, ly: 0 }
    }

    // 每走 t 个周期调一次，推进扫描线状态机
    // 返回：是否刚进入 VBlank（让 CPU 发中断）
    pub fn tick(&mut self, t: u32, bus: &mut crate::Bus) -> bool {
        let mut vblank_started = false;
        self.line_cycles += t;
        if self.line_cycles >= 456 {
            self.line_cycles -= 456;
            self.ly += 1;
            if self.ly == 144 {
                vblank_started = true;
                bus.mem[0xFF0F] |= 0x01;        // 请求 VBlank 中断
            } else if self.ly > 153 {
                self.ly = 0;                     // 回绕到帧首
            }
            bus.mem[0xFF44] = self.ly;           // LY 寄存器同步
            // 可见行才画（VBlank 的 10 行不画）
            if self.ly < 144 {
                self.render_line(bus);
                self.render_sprites(bus);
            }
        }
        self.update_stat(bus);                    // 维护 0xFF41 模式位 + LYC 比较 + STAT 中断
        vblank_started
    }

    // STAT 寄存器（0xFF41）—— 扫雷这类游戏死等它做渲染同步，之前漏了！
    // 每行 456 周期里切换 4 种模式：
    //   mode 2 (OAM 扫描) = 0~80，mode 3 (数据传送) = 80~252，mode 0 (HBlank) = 252~456
    //   模式 1 (VBlank) = ly 144~153 整段。
    // 0xFF41 位：bit0-1 当前模式、bit2 LYC==LY 命中、bit3-6 对应使能位。
    // STAT 中断是「电平触发」：条件成立且使能位开 → IF 第 2 位持续置 1（和 VBlank 的边沿触发不同）。
    fn update_stat(&mut self, bus: &mut crate::Bus) {
        let lyc = bus.mem[0xFF45];
        let mode = if self.ly >= 144 {
            1                                                  // VBlank
        } else if self.line_cycles < 80 {
            2                                                  // OAM 扫描
        } else if self.line_cycles < 252 {
            3                                                  // 数据传送
        } else {
            0                                                  // HBlank
        };
        let stat = bus.mem[0xFF41];
        // bit0-1 = 模式，bit2 = LYC 命中（只读，由 PPU 维护）
        let mut new_stat = (stat & 0xF8) | mode;
        if lyc == self.ly { new_stat |= 0x04; } else { new_stat &= !0x04; }
        bus.mem[0xFF41] = new_stat;
        // 电平触发的中断线
        let lyc_line = lyc == self.ly && stat & 0x40 != 0;     // bit6 = LYC 使能
        let mode_line = match mode {
            0 => stat & 0x08 != 0,                             // bit3 = HBlank 使能
            1 => stat & 0x10 != 0,                             // bit4 = VBlank 使能
            2 => stat & 0x20 != 0,                             // bit5 = OAM 使能
            _ => false,
        };
        if lyc_line || mode_line {
            bus.mem[0xFF0F] |= 0x02;                           // 电平触发：条件成立 → 中断线拉高
        } else {
            bus.mem[0xFF0F] &= !0x02;                          // 条件消失 → 中断线拉低
        }
    }

    // 画一行背景：从 VRAM 取地图+图块，上色进 framebuffer
    fn render_line(&mut self, bus: &mut crate::Bus) {
        let lcdc = bus.mem[0xFF40];
        if lcdc & 0x80 == 0 {                    // LCD 关着 → 整行空白
            self.framebuffer[self.ly as usize] = [0; SCREEN_W];
            return;
        }
        let y = self.ly as usize;
        let scy = bus.mem[0xFF42] as usize;      // 垂直滚动
        let scx = bus.mem[0xFF43] as usize;      // 水平滚动
        // 图块地图基址：LCDC bit3（0=9800，1=9C00）
        let map_base = if lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 };
        // 图块数据基址：LCDC bit4（1=8000 无符号，0=8800 有符号）
        let signed_tiles = lcdc & 0x10 == 0;
        let bgp = bus.mem[0xFF47];               // 调色板：每2位一个色号映射

        for x in 0..SCREEN_W {
            let map_x = ((x + scx) & 255) / 8;   // 32×32 图块地图里的哪块
            let map_y = ((y + scy) & 255) / 8;
            let tile_idx_addr = map_base + map_y * 32 + map_x;
            let tile_num = bus.mem[tile_idx_addr] as usize;
            // 算图块在 VRAM 里的地址（有符号模式的偏移）
            let tile_addr = if signed_tiles {
                (0x9000i32 + (tile_num as i8 as i32) * 16) as usize
            } else {
                0x8000 + tile_num * 16
            };
            // 图块内偏移：每块 8×8，每像素 2 bit（两个位平面）
            let px_in_tile = (x + scx) & 7;
            let py_in_tile = (y + scy) & 7;
            let lo = bus.mem[tile_addr + py_in_tile * 2];
            let hi = bus.mem[tile_addr + py_in_tile * 2 + 1];
            let bit = 7 - px_in_tile;            // 最左像素在最高位
            let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            // 过调色板：真机是灰度屏，颜色其实是映射出来的
            self.framebuffer[y][x] = (bgp >> (color * 2)) & 3;
        }
    }

    // 画一行精灵（OBJ）：游戏里会动的东西——下落的方块、角色
    // 每个精灵 40 字节：第0字节=Y(+16)，第1字节=X(+8)，第2字节=图块号，第3字节=属性
    fn render_sprites(&mut self, bus: &mut crate::Bus) {
        let lcdc = bus.mem[0xFF40];
        if lcdc & 0x80 == 0 { return; }                    // LCD 关着
        if lcdc & 0x02 == 0 { return; }                    // bit1=OBJ 使能关着
        let y = self.ly as i32;
        let sprite_size = if lcdc & 0x04 != 0 { 16 } else { 8 };   // bit2=8×16
        // 逐精灵画：OAM 从 0xFE00 起，40 字节一个
        for sp in 0..40 {
            let base = 0xFE00 + sp * 4;
            // OAM 存的坐标 = 屏幕左上角 + 偏移（X+8、Y+16），要减回来才是真屏幕位置
            let screen_y = bus.mem[base] as i32 - 16;
            let screen_x = bus.mem[base + 1] as i32 - 8;
            let tile = bus.mem[base + 2] as usize;
            let attr = bus.mem[base + 3];
            // 精灵是否覆盖本行？（screen_y<0 时从屏幕上方滑入，自然裁掉）
            if y < screen_y || y >= screen_y + sprite_size { continue; }
            let mut py = (y - screen_y) as usize;
            if attr & 0x40 != 0 { py = (sprite_size as usize - 1) - py; }  // Y 翻转
            for x in 0..SCREEN_W as i32 {
                if x < screen_x || x >= screen_x + 8 { continue; }
                let px = (x - screen_x) as usize;
                let px = if attr & 0x20 != 0 { 7 - px } else { px };              // X 翻转
                let tile_addr = 0x8000 + tile * 16;
                let lo = bus.mem[tile_addr + py * 2];
                let hi = bus.mem[tile_addr + py * 2 + 1];
                let bit = 7 - px;
                let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                if color == 0 { continue; }                // 色号0=透明
                // 调色板：OBP0/OBP1，由属性 bit4 选
                let palette = if attr & 0x10 != 0 { bus.mem[0xFF49] } else { bus.mem[0xFF48] };
                self.framebuffer[y as usize][x as usize] = (palette >> (color * 2)) & 3;
            }
        }
    }
}
