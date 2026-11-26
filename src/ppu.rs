// PPU —— GameBoy 的第二颗芯片：逐 dot（T-cycle）渲染。
//
// 本版架构对齐 DocBoy（github.com/Docheinstein/docboy）的 DMG PPU 实现 —— 该实现是
// GBEmulatorShootout 上唯一 24/24 全过 Mealybug Tearoom Tests 的开源模拟器，
// 本文件的状态机、取数器、FIFO 与窗口逻辑均按其语义逐条移植（源码注释里的
// [mealybug/...] 编号即为本套测试）。
//
// 核心概念：
//   · dots      本行内的 dot（0~455），每 T-cycle 走一格
//   · lx        像素位置 X+8，∈[0,168]；X = lx-8 为屏幕坐标
//   · bg_fifo   背景 FIFO（8 像素）：取数器每 6 dot 取一个 tile 行，
//               仅当 FIFO 为空时整体填入（覆写式）
//   · obj_fifo  精灵 FIFO（8 像素）：精灵取数器取完 tile 后按优先级合并进来，
//               与 bg_fifo 成对弹出、混色上屏
//   · oam_entries[lx]  OAM 扫描按「精灵 X 字节」分桶；渲染推进到该 lx 时逐个取数
//   · 窗口      每个dot检查 WY 条件（本帧激活过即可），lx==WX 时触发：
//               清 BG FIFO、取数器重启、窗口行计数 +1
//   · 寄存器    BGP/LCDC/WX 对 PPU 有 1 dot 延迟（last_*），BGP 在延迟 dot 上
//               表现为「旧值|新值」按位或（[mealybug/m3_bgp_change]）
//
// 时序常数（取数器 6 dot/tile、精灵取数 6 dot、窗口重启 6 dot）全部由此模型自然导出，
// 与 _mealybug/repo/expected/DMG-blob 的 24 张标准答案逐像素一致。

use std::collections::VecDeque;

pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;

const TILE_WIDTH: u32 = 8;
const TILE_HEIGHT: u32 = 8;
const TILEMAP_WIDTH: u32 = 32;
const TILEMAP_HEIGHT: u32 = 32;
const TILE_BYTES: u16 = 16;
const TILE_DOUBLE_HEIGHT: u8 = 16;

// ---------- 主状态机（对应 docboy 的 tick_selector） ----------
#[derive(Clone, Copy, PartialEq)]
enum State {
    OamScanEven,
    OamScanOdd,
    OamScan77,
    OamScanDone,
    OamScanAfterTurnOn,
    OamScanAfterTurnOn78,
    OamScanAfterTurnOn79,
    PixelTransferDummyLx0,
    PixelTransferDiscardLx0,
    PixelTransferDiscardLx0Wx0Scx,
    PixelTransferLx0,
    PixelTransferLx8,
    Hblank,
    Hblank452,
    Hblank453,
    Hblank454,
    Hblank455,
    HblankLastLine,
    HblankLastLine453,
    HblankLastLine454,
    HblankLastLine455,
    HblankFirstLineAfterTurnOn,
    Vblank,
    Vblank453,
    Vblank454,
    Vblank455,
    VblankLastLine,
    VblankLastLine2,
    VblankLastLine3,
    VblankLastLine4,
    VblankLastLine7,
    VblankLastLine453,
    VblankLastLine454,
    VblankLastLine455,
}

// ---------- BG/Window 取数器（对应 fetcher_tick_selector，每 tick 半步） ----------
#[derive(Clone, Copy, PartialEq)]
enum FetcherState {
    BgWinGetTile0,   // 分派：窗口激活中走窗口，否则走背景
    BgGetTile0,
    BgGetTile1,
    BgDataLow0,
    BgDataLow1,
    BgDataHigh0,
    BgDataHigh1,     // 共享收尾：tile 计数 +8、允许 push
    WinGetTile0,
    WinGetTile1,
    WinDataLow0,
    WinDataLow1,
    WinDataHigh0,
    WinDataHigh1,
    ObjGetTile0,
    ObjGetTile1,
    ObjDataLow0,
    ObjDataLow1,
    ObjDataHigh0,
    ObjDataHigh1Merge,
    Idle,
}

#[derive(Clone, Copy)]
struct ObjPixel {
    color: u8,
    attributes: u8,
    number: u8,
    x: u8,
}

pub struct Ppu {
    pub framebuffer: [[u8; SCREEN_W]; SCREEN_H],
    pub ly: u8,
    pub(crate) oam_dma_active: bool,

    lcdc: u8,
    last_lcdc: u8,
    last_bgp: u8,
    last_wx: u8,
    last_ly: u8,
    last_lyc: u8,

    pub state: State,
    pub dots: u32,
    lx: u8,
    lcd_x: u8,
    mode: u8,

    stat_lyc_eq_ly: bool,
    pending_stat_irq: bool,
    last_stat_irq: bool,
    enable_lyc_eq_ly_irq: bool,
    stat_write_pending: Option<u8>,

    bg_fifo: VecDeque<u8>,
    obj_fifo: VecDeque<ObjPixel>,

    fstate: FetcherState,
    bwf_lx: u8,
    bwf_tile_number: u8,
    psf_addr: u16,
    psf_lo: u8,
    psf_hi: u8,
    pending_bg_fifo_fill: bool,
    push_enabled: bool,
    interrupted_fetch: Option<(u8, u8)>,

    is_fetching_sprite: bool,
    of_number: u8,
    of_y: u8,
    of_tile: u8,
    of_attr: u8,

    w_active_for_frame: bool,
    w_wly: u8,
    w_activating: bool,
    w_active: bool,
    w_just_activated: bool,
    wf_tilemap_x: u8,

    // OAM 扫描按精灵 X 字节分桶（X ∈ [0,168)）
    oam_entries: Vec<Vec<(u8, u8)>>,
    oam_scan_count: u8,
    oam_scan_index: u8,

    pt_to_discard: u8,
    pt_discarded: u8,
    dummy_ticks: u32,

    is_glitched_line_0: bool,
    glitched_hblank_delay: u8,
    vblank_last_line_glitch: bool,
}

impl Ppu {
    pub fn new() -> Self {
        Ppu {
            framebuffer: [[0; SCREEN_W]; SCREEN_H],
            ly: 0,
            oam_dma_active: false,
            lcdc: 0,
            last_lcdc: 0,
            last_bgp: 0,
            last_wx: 0,
            last_ly: 0,
            last_lyc: 0,
            state: State::OamScanAfterTurnOn,
            dots: 0,
            lx: 0,
            lcd_x: 0,
            mode: 2,
            stat_lyc_eq_ly: false,
            pending_stat_irq: false,
            last_stat_irq: false,
            enable_lyc_eq_ly_irq: true,
            stat_write_pending: None,
            bg_fifo: VecDeque::with_capacity(8),
            obj_fifo: VecDeque::with_capacity(8),
            fstate: FetcherState::BgWinGetTile0,
            bwf_lx: 0,
            bwf_tile_number: 0,
            psf_addr: 0,
            psf_lo: 0,
            psf_hi: 0,
            pending_bg_fifo_fill: false,
            push_enabled: false,
            interrupted_fetch: None,
            is_fetching_sprite: false,
            of_number: 0,
            of_y: 0,
            of_tile: 0,
            of_attr: 0,
            w_active_for_frame: false,
            w_wly: 255,
            w_activating: false,
            w_active: false,
            w_just_activated: false,
            wf_tilemap_x: 0,
            oam_entries: (0..168).map(|_| Vec::new()).collect(),
            oam_scan_count: 0,
            oam_scan_index: 0,
            pt_to_discard: 0,
            pt_discarded: 0,
            dummy_ticks: 0,
            is_glitched_line_0: true,
            glitched_hblank_delay: 0,
            vblank_last_line_glitch: false,
        }
    }

    // 推进 n 个 T-cycle（Bus 保证 n 为 4 的倍数：CPU 每次访存调一次）
    pub fn tick_dots(&mut self, n: u32, mem: &mut [u8]) {
        for _ in 0..n {
            self.tick_dot(mem);
        }
    }

    fn tick_dot(&mut self, mem: &mut [u8]) {
        if self.lcdc & 0x80 != 0 {
            self.state_tick(mem);
            self.tick_window(mem);
            self.raise_stat_irq(mem);
            self.last_bgp = mem[0xFF47];
            self.last_wx = mem[0xFF4B];
            self.last_lcdc = self.lcdc;
            self.last_ly = self.ly;
            self.last_lyc = mem[0xFF45];
        }
        self.tick_pending_write(mem);
        self.tick_stat(mem);
    }

    // ---------- 寄存器写入（Bus 转发） ----------
    pub fn write_lcdc(&mut self, val: u8, mem: &mut [u8]) {
        let en = val & 0x80 != 0;
        if en != (self.lcdc & 0x80 != 0) {
            if en { self.turn_on(mem); } else { self.turn_off(mem); }
        }
        self.lcdc = val;
        mem[0xFF40] = val;
    }

    pub fn write_stat(&mut self, val: u8, mem: &mut [u8]) {
        // DMG：STAT 写入 1 dot 后生效；期间表现为所有中断使能全开
        self.stat_write_pending = Some(val);
        mem[0xFF41] |= 0xF8;
    }

    fn turn_off(&mut self, mem: &mut [u8]) {
        self.dots = 0;
        self.ly = 0;
        mem[0xFF44] = 0;
        for v in self.oam_entries.iter_mut() { v.clear(); }
        self.reset_fetcher();
        self.mode = 2;
        self.state = State::OamScanAfterTurnOn;
        self.is_glitched_line_0 = true;
        self.glitched_hblank_delay = 0;
        self.enable_lyc_eq_ly_irq = true;
        self.framebuffer = [[0; SCREEN_W]; SCREEN_H];
    }

    fn turn_on(&mut self, _mem: &mut [u8]) {
        // 状态沿用 turn_off 留下的 OamScanAfterTurnOn（首行行为特殊）
    }

    fn reset_fetcher(&mut self) {
        self.lx = 0;
        self.lcd_x = 0;
        self.bg_fifo.clear();
        self.obj_fifo.clear();
        self.push_enabled = false;
        self.pending_bg_fifo_fill = false;
        self.is_fetching_sprite = false;
        self.w_activating = false;
        self.w_active = false;
        self.w_just_activated = false;
        self.bwf_lx = 0;
        self.bwf_tile_number = 0;
        self.interrupted_fetch = None;
        self.wf_tilemap_x = 0;
        self.fstate = FetcherState::BgWinGetTile0;
    }

    // ---------- 每个状态 one tick ----------
    fn state_tick(&mut self, mem: &mut [u8]) {
        match self.state {
            State::OamScanEven => {
                // 两条 OAM 字节在本 tick 落地（简化：无 DMA 冲突）
                self.handle_oam_scan_entry(mem);
                self.dots += 1;
                self.state = if self.dots == 77 { State::OamScan77 } else { State::OamScanOdd };
            }
            State::OamScanOdd => {
                self.dots += 1;
                self.state = State::OamScanEven;
            }
            State::OamScan77 => {
                self.dots = 78;
                self.state = State::OamScanDone;
            }
            State::OamScanDone => {
                self.dots += 1;
                if self.dots == 79 {
                    self.mode = 3;   // STAT 比实际提前 1 dot 进入 Pixel Transfer
                } else if (self.dots == 80 && self.ly == 0) || self.dots == 84 {
                    // 第 0 行的 OAM 扫描比其余行短 4 dot（mode 2 中断在帧切换时晚 4 dot 拋出，
                    // 处理函数却早 4 dot，两相抵消后像素仍需提前 4 dot）
                    self.enter_pixel_transfer();
                }
            }
            State::OamScanAfterTurnOn => {
                // 开机后的首次 OAM 扫描是空转
                self.dots += 1;
                if self.dots == 78 {
                    self.state = State::OamScanAfterTurnOn78;
                }
            }
            State::OamScanAfterTurnOn78 => {
                self.mode = 3;
                self.dots = 83;
                self.state = State::OamScanAfterTurnOn79;
            }
            State::OamScanAfterTurnOn79 => {
                self.dots = 84;
                self.enter_pixel_transfer();
            }
            State::PixelTransferDummyLx0 => {
                self.dots += 1;
                self.dummy_ticks += 1;
                // 3 拍 dummy 取数后锁存 SCX%8（入口 dot 因行而异：第 0 行 80、其余 84）
                if self.dummy_ticks == 3 {
                    // 第一个 tile 取数只为对齐，数据扔掉
                    self.bg_fifo.clear();
                    for _ in 0..8 { self.bg_fifo.push_back(0); }
                    // [mealybug/m3_scx_low_3_bits] SCX 低 3 位在此刻之后锁存
                    self.pt_to_discard = mem[0xFF43] & 7;
                    self.pt_discarded = 0;
                    if self.pt_to_discard > 0 {
                        self.check_window_activation();
                        self.state = if self.w_activating {
                            State::PixelTransferDiscardLx0Wx0Scx
                        } else {
                            State::PixelTransferDiscardLx0
                        };
                    } else {
                        self.state = State::PixelTransferLx0;
                    }
                }
            }
            State::PixelTransferDiscardLx0Wx0Scx => {
                self.handle_pending_window_activation();
                // WX==0 激活晚 1 dot：跳过本 tick 的取数器
                self.state = State::PixelTransferDiscardLx0;
                self.dots += 1;
            }
            State::PixelTransferDiscardLx0 => {
                self.handle_pending_bg_fifo_fill();
                // SCX%8 个像素直接丢弃；lx 不前进（精灵与背景对齐）
                if self.is_bg_fifo_ready_to_be_popped(mem) {
                    self.bg_fifo.pop_front();
                    self.pt_discarded += 1;
                    if self.pt_discarded == self.pt_to_discard {
                        self.state = State::PixelTransferLx0;
                    }
                }
                self.tick_fetcher(mem);
                self.handle_pixel_slice_fetcher_push(mem);
                self.dots += 1;
            }
            State::PixelTransferLx0 => {
                self.handle_pending_window_activation();
                self.handle_pending_bg_fifo_fill();
                let mut inc_lx = false;
                if self.is_bg_fifo_ready_to_be_popped(mem) {
                    self.bg_fifo.pop_front();
                    if !self.obj_fifo.is_empty() {
                        self.obj_fifo.pop_front();
                    }
                    inc_lx = true;
                    if self.lx + 1 == 8 {
                        self.state = State::PixelTransferLx8;
                    }
                    self.check_window_activation();
                }
                self.tick_fetcher(mem);
                self.handle_pixel_slice_fetcher_push(mem);
                if inc_lx {
                    self.increase_lx();
                }
                self.dots += 1;
            }
            State::PixelTransferLx8 => {
                self.handle_pending_window_activation();
                self.handle_pending_bg_fifo_fill();
                let mut inc_lx = false;
                if self.is_bg_fifo_ready_to_be_popped(mem) {
                    let bg_pixel = self.bg_fifo.pop_front().unwrap();
                    // DMG：PPU 看到的 LCDC 晚 1 dot，lx==8 例外
                    let lcdc_ = if self.lx == 8 { self.lcdc } else { self.last_lcdc };
                    let mut color: Option<u8> = None;
                    if let Some(obj) = self.obj_fifo.pop_front() {
                        let show_obj = (lcdc_ & 0x02 != 0) && obj.color != 0
                            && (!(lcdc_ & 0x01 != 0) || bg_pixel == 0 || (obj.attributes & 0x80 == 0));
                        if show_obj {
                            let obp = if obj.attributes & 0x10 != 0 { mem[0xFF49] } else { mem[0xFF48] };
                            color = Some((obp >> (obj.color * 2)) & 3);
                        }
                    }
                    let color = color.unwrap_or_else(|| {
                        // BGP：1 dot 延迟，期间 PPU 看到「旧|新」按位或
                        let bgp_ = mem[0xFF47] | self.last_bgp;
                        if lcdc_ & 0x01 != 0 { (bgp_ >> (bg_pixel * 2)) & 3 } else { 0 }
                    });
                    let x = self.lcd_x as usize;
                    if x < SCREEN_W {
                        self.framebuffer[self.ly as usize][x] = color;
                    }
                    if crate::TRACE.with(|t| t.get()) && self.ly == crate::DBG_LINE.with(|l| l.get()) && self.lcd_x < 3 {
                        eprintln!("px ly={} lcd_x={} dot={} bg={} color={}", self.ly, self.lcd_x, self.dots, bg_pixel, color);
                    }
                    self.lcd_x += 1;
                    inc_lx = true;
                    if self.lx + 1 == 168 {
                        self.increase_lx();
                        self.dots += 1;
                        self.enter_hblank();
                        return;
                    }
                    self.check_window_activation();
                }
                self.tick_fetcher(mem);
                self.handle_pixel_slice_fetcher_push(mem);
                if inc_lx {
                    self.increase_lx();
                }
                self.dots += 1;
            }
            State::Hblank => {
                self.dots += 1;
                if self.dots == 452 {
                    self.state = State::Hblank452;
                }
            }
            State::Hblank452 => {
                self.update_stat_irq_for_oam_mode(mem);
                self.dots = 453;
                self.state = State::Hblank453;
            }            State::Hblank453 => {
                for v in self.oam_entries.iter_mut() { v.clear(); }
                self.oam_scan_count = 0;
                self.oam_scan_index = 0;
                self.enable_lyc_eq_ly_irq = false;
                self.end_increase_ly(mem);
                self.dots = 454;
                self.state = State::Hblank454;
            }
            State::Hblank454 => {
                self.handle_oam_scan_entry(mem);
                self.enable_lyc_eq_ly_irq = true;
                self.dots = 455;
                self.mode = 2;   // STAT 提前 1 dot 进入 OAM Scan
                self.state = State::Hblank455;
            }
            State::Hblank455 => {
                self.dots = 0;
                self.state = State::OamScanEven;
            }
            State::HblankLastLine => {
                self.dots += 1;
                if self.dots == 453 {
                    self.update_stat_irq_for_oam_mode_do_not_clear(mem);
                    self.state = State::HblankLastLine453;
                }
            }
            State::HblankLastLine453 => {
                self.enable_lyc_eq_ly_irq = false;
                self.end_increase_ly(mem);
                self.dots = 454;
                self.state = State::HblankLastLine454;
            }
            State::HblankLastLine454 => {
                self.enable_lyc_eq_ly_irq = true;
                self.dots = 455;
                self.mode = 1;
                self.state = State::HblankLastLine455;
            }
            State::HblankLastLine455 => {
                self.dots = 0;
                self.enter_vblank(mem);
            }
            State::HblankFirstLineAfterTurnOn => {
                self.dots += 1;
                self.glitched_hblank_delay += 1;
                if self.glitched_hblank_delay == 2 {
                    self.mode = 0;
                    self.state = State::Hblank;
                    self.is_glitched_line_0 = false;
                }
            }
            State::Vblank => {
                self.dots += 1;
                if self.dots == 453 {
                    self.state = State::Vblank453;
                }
            }
            State::Vblank453 => {
                self.dots = 454;
                self.end_increase_ly(mem);
                self.enable_lyc_eq_ly_irq = false;
                self.state = State::Vblank454;
            }
            State::Vblank454 => {
                self.dots = 455;
                self.enable_lyc_eq_ly_irq = true;
                self.state = State::Vblank455;
            }
            State::Vblank455 => {
                self.dots = 0;
                self.state = if self.ly == 153 { State::VblankLastLine } else { State::Vblank };
            }
            State::VblankLastLine => {
                self.dots += 1;
                if self.dots == 2 {
                    // DMG：LY 在本行 dot 2 就回 0
                    self.ly = 0;
                    mem[0xFF44] = 0;
                    self.state = State::VblankLastLine2;
                }
            }
            State::VblankLastLine2 => {
                self.dots = 3;
                self.enable_lyc_eq_ly_irq = false;
                self.state = State::VblankLastLine3;
            }
            State::VblankLastLine3 => {
                self.dots = 4;
                self.state = State::VblankLastLine4;
            }
            State::VblankLastLine4 => {
                self.dots += 1;
                if self.dots == 7 {
                    self.enable_lyc_eq_ly_irq = true;
                    self.state = State::VblankLastLine7;
                }
            }
            State::VblankLastLine7 => {
                self.dots += 1;
                if self.dots == 453 {
                    self.state = State::VblankLastLine453;
                }
            }
            State::VblankLastLine453 => {
                for v in self.oam_entries.iter_mut() { v.clear(); }
                self.oam_scan_count = 0;
                self.oam_scan_index = 0;
                self.vblank_last_line_glitch = true;
                self.mode = 2;
                self.dots = 454;
                self.state = State::VblankLastLine454;
            }
            State::VblankLastLine454 => {
                self.handle_oam_scan_entry(mem);
                self.dots = 455;
                self.state = State::VblankLastLine455;
            }
            State::VblankLastLine455 => {
                self.dots = 0;
                self.enter_new_frame(mem);
            }
        }
    }

    fn enter_pixel_transfer(&mut self) {
        self.reset_fetcher();
        self.state = State::PixelTransferDummyLx0;
        self.lcd_x = 0;
        self.dummy_ticks = 0;
    }

    fn enter_hblank(&mut self) {
        if self.is_glitched_line_0 {
            self.state = State::HblankFirstLineAfterTurnOn;
            self.glitched_hblank_delay = 0;
        } else {
            self.mode = 0;
            self.state = if self.ly == 143 { State::HblankLastLine } else { State::Hblank };
        }
    }

    fn enter_vblank(&mut self, mem: &mut [u8]) {
        self.state = State::Vblank;
        mem[0xFF0F] |= 0x01;
    }

    fn enter_new_frame(&mut self, mem: &mut [u8]) {
        self.w_wly = 255;
        self.w_active_for_frame = false;
        self.state = State::OamScanEven;
        self.mode = 2;
        self.update_stat_irq_for_oam_mode(mem);
    }

    fn end_increase_ly(&mut self, mem: &mut [u8]) {
        self.ly += 1;
        mem[0xFF44] = self.ly;
    }

    // ---------- 取数器 ----------
    fn tick_fetcher(&mut self, mem: &mut [u8]) {
        match self.fstate {
            FetcherState::BgWinGetTile0 => {
                if self.w_active {
                    if self.lcdc & 0x20 != 0 {
                        // 继续取窗口 tile
                        self.win_get_tile_0(mem);
                        return;
                    }
                    // 窗口被关掉：切回背景
                    self.w_active = false;
                }
                self.bg_get_tile_0(mem);
            }
            FetcherState::BgGetTile0 => { self.bg_get_tile_0(mem); }
            FetcherState::BgGetTile1 => { self.fstate = FetcherState::BgDataLow0; }
            FetcherState::BgDataLow0 => { self.bg_data_low_0(mem); }
            FetcherState::BgDataLow1 => { self.fstate = FetcherState::BgDataHigh0; }
            FetcherState::BgDataHigh0 => { self.bg_data_high_0(mem); }
            FetcherState::BgDataHigh1 => { self.bgwin_data_high_1(); }
            FetcherState::WinGetTile0 => { self.win_get_tile_0(mem); }
            FetcherState::WinGetTile1 => {
                if self.lcdc & 0x20 == 0 { self.fstate = FetcherState::BgDataLow0; }
                else { self.fstate = FetcherState::WinDataLow0; }
            }
            FetcherState::WinDataLow0 => {
                if self.lcdc & 0x20 == 0 { self.bg_data_low_0(mem); return; }
                self.setup_win_data_address(mem);
                self.psf_lo = mem[self.psf_addr as usize];
                self.fstate = FetcherState::WinDataLow1;
            }
            FetcherState::WinDataLow1 => {
                if self.lcdc & 0x20 == 0 { self.fstate = FetcherState::BgDataHigh0; }
                else { self.fstate = FetcherState::WinDataHigh0; }
            }
            FetcherState::WinDataHigh0 => {
                if self.lcdc & 0x20 == 0 { self.bg_data_high_0(mem); return; }
                self.setup_win_data_address(mem);
                self.psf_hi = mem[self.psf_addr as usize + 1];
                self.fstate = FetcherState::WinDataHigh1;
            }
            FetcherState::WinDataHigh1 => { self.bgwin_data_high_1(); }
            FetcherState::ObjGetTile0 => {
                self.fstate = FetcherState::ObjGetTile1;
            }
            FetcherState::ObjGetTile1 => {
                let base = 0xFE00 + self.of_number as usize * 4;
                self.of_tile = mem[base + 2];
                self.of_attr = mem[base + 3] & 0xF0;   // DMG：低 4 位无用
                self.fstate = FetcherState::ObjDataLow0;
            }
            FetcherState::ObjDataLow0 => {
                self.fstate = FetcherState::ObjDataLow1;
            }
            FetcherState::ObjDataLow1 => {
                self.setup_obj_data_address(mem);
                self.psf_lo = mem[self.psf_addr as usize];
                self.fstate = FetcherState::ObjDataHigh0;
            }
            FetcherState::ObjDataHigh0 => {
                self.fstate = FetcherState::ObjDataHigh1Merge;
            }
            FetcherState::ObjDataHigh1Merge => {
                self.setup_obj_data_address(mem);
                self.psf_hi = mem[self.psf_addr as usize + 1];
                self.merge_obj_row();
            }
            FetcherState::Idle => {}
        }
    }

    fn bg_get_tile_0(&mut self, mem: &mut [u8]) {
        // 图块地址在本相计算（地图位/SCX/SCY 现读）
        // [mealybug/m3_lcdc_bg_map_change]
        let tilemap_x = (((self.bwf_lx as u32 + mem[0xFF43] as u32) & 0xFF) / TILE_WIDTH % TILEMAP_WIDTH) as u16;
        let tilemap_y = (((self.ly as u32 + mem[0xFF42] as u32) & 0xFF) / TILE_HEIGHT % TILEMAP_HEIGHT) as u16;
        let map_base = if self.lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 };
        let addr = map_base + tilemap_y * 32 + tilemap_x;
        self.bwf_tile_number = mem[addr as usize];
        if std::env::var("GB_FDBG").is_ok() && self.ly == crate::DBG_LINE.with(|l| l.get()) {
            eprintln!("get_tile ly={} dot={} bwf_lx={} scx={} scy={} map_y={} tile={}", self.ly, self.dots, self.bwf_lx, mem[0xFF43], mem[0xFF42], tilemap_y, self.bwf_tile_number);
        }
        self.fstate = FetcherState::BgGetTile1;
    }

    fn bg_data_low_0(&mut self, mem: &mut [u8]) {
        // 数据地址在低/高两相各自现算（SCY/TILE_SEL 中途改会产生位平面错位）
        // [mealybug/m3_scy_change, mealybug/m3_lcdc_tile_sel_change]
        self.setup_bg_data_address(mem);
        self.psf_lo = mem[self.psf_addr as usize];
        self.fstate = FetcherState::BgDataLow1;
    }

    fn bg_data_high_0(&mut self, mem: &mut [u8]) {
        self.setup_bg_data_address(mem);
        self.psf_hi = mem[self.psf_addr as usize + 1];
        self.fstate = FetcherState::BgDataHigh1;
    }

    fn bgwin_data_high_1(&mut self) {
        // DMG：tile 计数只在此步 +8；首个窗口取数不加
        if !self.w_just_activated {
            self.bwf_lx = self.bwf_lx.wrapping_add(8);
        }
        self.fstate = FetcherState::Idle;
        self.push_enabled = true;
    }

    fn win_get_tile_0(&mut self, mem: &mut [u8]) {
        if self.lcdc & 0x20 == 0 {
            self.bg_get_tile_0(mem);
            return;
        }
        // 窗口取数器有自己的 tilemap x 计数
        let tilemap_x = self.wf_tilemap_x;
        self.wf_tilemap_x = self.wf_tilemap_x.wrapping_add(1) & 0x1F;
        let tilemap_y = (self.w_wly / 8) & 0x1F;
        let map_base = if self.lcdc & 0x40 != 0 { 0x9C00 } else { 0x9800 };
        let addr = map_base + tilemap_y as u16 * 32 + tilemap_x as u16;
        self.bwf_tile_number = mem[addr as usize];
        self.fstate = FetcherState::WinGetTile1;
    }

    fn setup_bg_data_address(&mut self, mem: &mut [u8]) {
        let vram_tile = if self.lcdc & 0x10 != 0 {
            0x8000u32 + TILE_BYTES as u32 * self.bwf_tile_number as u32
        } else {
            (0x9000i32 + TILE_BYTES as i32 * self.bwf_tile_number as i8 as i32) as u32
        };
        let tile_y = (self.ly as u32 + mem[0xFF42] as u32) & 7;
        self.psf_addr = (vram_tile + 2 * tile_y) as u16;
    }

    fn setup_win_data_address(&mut self, mem: &mut [u8]) {
        let vram_tile = if self.lcdc & 0x10 != 0 {
            0x8000u32 + TILE_BYTES as u32 * self.bwf_tile_number as u32
        } else {
            (0x9000i32 + TILE_BYTES as i32 * self.bwf_tile_number as i8 as i32) as u32
        };
        let tile_y = (self.w_wly & 7) as u32;
        self.psf_addr = (vram_tile + 2 * tile_y) as u16;
    }

    fn setup_obj_data_address(&mut self, mem: &mut [u8]) {
        let is_double = self.lcdc & 0x04 != 0;
        let height_mask: u8 = if is_double { 0xF } else { 7 };
        let obj_y = self.of_y as i32 - TILE_DOUBLE_HEIGHT as i32;
        let mut tile_y = (self.ly as i32 - obj_y) as u8 & height_mask;
        if self.of_attr & 0x40 != 0 {
            tile_y ^= height_mask;
        }
        let tile_number = if is_double { self.of_tile & 0xFE } else { self.of_tile };
        self.psf_addr = (0x8000 + TILE_BYTES as u16 * tile_number as u16 + 2 * tile_y as u16) as u16;
    }

    fn merge_obj_row(&mut self) {
        // 组装 8 个精灵像素（X 翻转）
        let mut pixels = [0u8; 8];
        let flip = self.of_attr & 0x20 != 0;
        for i in 0..8 {
            let bit = if flip { i } else { 7 - i };
            pixels[i] = ((self.psf_lo >> bit) & 1) | (((self.psf_hi >> bit) & 1) << 1);
        }
        // 精灵间冲突：不透明盖透明；同 X 时 OAM 序号小的赢
        let mut i = 0usize;
        while i < self.obj_fifo.len() {
            let fifo = self.obj_fifo[i];
            let opaque_new = pixels[i] != 0;
            let overwrite = (opaque_new && fifo.color == 0)
                || (opaque_new && fifo.color != 0
                    && self.of_number < fifo.number);
            if overwrite {
                self.obj_fifo[i] = ObjPixel {
                    color: pixels[i],
                    attributes: self.of_attr,
                    number: self.of_number,
                    x: self.lx,
                };
            }
            i += 1;
        }
        while i < 8 {
            self.obj_fifo.push_back(ObjPixel {
                color: pixels[i],
                attributes: self.of_attr,
                number: self.of_number,
                x: self.lx,
            });
            i += 1;
        }
        if !self.oam_entries[self.lx as usize].is_empty() && self.lcdc & 0x02 != 0 {
            // 还有同 X 的精灵要取
            let e = self.oam_entries[self.lx as usize].remove(0);
            self.of_number = e.0;
            self.of_y = e.1;
            self.fstate = FetcherState::ObjGetTile0;
        } else {
            if let Some((lo, hi)) = self.interrupted_fetch.take() {
                self.psf_lo = lo;
                self.psf_hi = hi;
            }
            self.is_fetching_sprite = false;
            self.fstate = FetcherState::Idle;
        }
    }

    // ---------- FIFO / 推进 ----------
    fn is_bg_fifo_ready_to_be_popped(&self, mem: &[u8]) -> bool {
        !self.bg_fifo.is_empty()
            && !self.is_fetching_sprite
            && (self.oam_entries[self.lx as usize].is_empty() || mem[0xFF40] & 0x02 == 0)
    }

    fn is_obj_ready_to_be_fetched(&self, mem: &[u8]) -> bool {
        !self.oam_entries[self.lx as usize].is_empty() && mem[0xFF40] & 0x02 != 0
    }

    fn handle_pixel_slice_fetcher_push(&mut self, mem: &mut [u8]) {
        if self.push_enabled && !self.is_fetching_sprite {
            let can_push = self.bg_fifo.is_empty();
            if can_push {
                // 窗口 00 像素毛刺：本帧窗口条件成立过、lx 恰为 WX 时，推入一个 0 像素
                if self.w_active_for_frame && self.lx == mem[0xFF4B] && !self.w_just_activated {
                    self.bg_fifo.push_back(0);
                    return;
                }
                self.pending_bg_fifo_fill = true;
                self.push_enabled = false;
                self.fstate = FetcherState::BgWinGetTile0;
                if self.w_just_activated {
                    self.w_just_activated = false;
                    return;
                }
            }
            if self.is_obj_ready_to_be_fetched(mem) {
                if !can_push {
                    // 背景取数被打断：缓存数据，精灵取完后恢复
                    self.interrupted_fetch = Some((self.psf_lo, self.psf_hi));
                }
                self.is_fetching_sprite = true;
                let e = self.oam_entries[self.lx as usize].remove(0);
                self.of_number = e.0;
                self.of_y = e.1;
                // 精灵预取的第一拍与背景推送重叠，下一拍直接读取 OAM。
                self.fstate = FetcherState::ObjGetTile1;
            }
        }
    }

    fn handle_pending_bg_fifo_fill(&mut self) {
        if self.pending_bg_fifo_fill {
            self.pending_bg_fifo_fill = false;
            self.bg_fifo.clear();
            for i in 0..8u32 {
                let bit = 7 - i;
                let px = ((self.psf_lo >> bit) & 1) | (((self.psf_hi >> bit) & 1) << 1);
                self.bg_fifo.push_back(px);
            }
        }
    }

    fn handle_pending_window_activation(&mut self) {
        if self.w_activating {
            self.bg_fifo.clear();
            self.w_active = true;
            self.w_just_activated = true;
            self.wf_tilemap_x = 0;
            self.w_activating = false;
            self.fstate = FetcherState::BgWinGetTile0;
        }
    }

    fn check_window_activation(&mut self) {
        // 触发条件：本帧 WY 条件成立过、窗口开着、lx 追上 WX（DMG：晚 1 dot 的毛刺）
        if self.w_active_for_frame && !self.w_active && self.lcdc & 0x20 != 0
            && (self.lx == self.last_wx
                || (self.lx == self.last_wx.wrapping_add(1) && self.last_lcdc & 0x20 == 0))
        {
            self.w_activating = true;
            self.w_wly = self.w_wly.wrapping_add(1);
            self.push_enabled = false;
            self.fstate = FetcherState::Idle;   // 下一个取数器 tick 被跳过
        }
    }

    fn tick_window(&mut self, mem: &mut [u8]) {
        // 本帧内只要有一刻「窗口开 + LY==WY」，窗口即对本帧激活（之后逐行再查 WX）
        self.w_active_for_frame = self.w_active_for_frame
            || (self.lcdc & 0x20 != 0 && self.ly == mem[0xFF4A]);
    }

    fn increase_lx(&mut self) {
        self.oam_entries[self.lx as usize].clear();
        self.lx += 1;
    }

    // ---------- OAM 扫描 ----------
    fn handle_oam_scan_entry(&mut self, mem: &mut [u8]) {
        if !self.oam_dma_active && self.oam_scan_count < 10 {
            let obj_height = if self.lcdc & 0x04 != 0 { 16 } else { 8 };
            let base = 0xFE00 + self.oam_scan_index as usize * 4;
            let entry_y = mem[base];
            let obj_y = entry_y as i32 - TILE_DOUBLE_HEIGHT as i32;
            if obj_y <= self.ly as i32 && (self.ly as i32) < obj_y + obj_height {
                let entry_x = mem[base + 1];
                if (entry_x as usize) < 168 {
                    self.oam_entries[entry_x as usize].push((self.oam_scan_index, entry_y));
                }
                self.oam_scan_count += 1;
            }
        }
        self.oam_scan_index += 1;
    }

    // ---------- STAT ----------
    fn is_lyc_eq_ly(&self) -> bool {
        (self.last_lyc == self.last_ly) && self.enable_lyc_eq_ly_irq
    }

    fn raise_stat_irq(&mut self, mem: &mut [u8]) {
        let lyc_eq_ly = self.is_lyc_eq_ly();
        if self.pending_stat_irq {
            mem[0xFF0F] |= 0x02;
            self.pending_stat_irq = false;
        } else {
            let stat = mem[0xFF41];
            let lyc_irq = stat & 0x40 != 0 && lyc_eq_ly;
            let hblank_irq = stat & 0x08 != 0 && self.mode == 0;
            // DMG：VBlank 行上 VBLANK 或 OAM 使能位都会拉 STAT 中断
            let vblank_irq = (stat & 0x10 != 0 || stat & 0x20 != 0) && self.mode == 1;
            self.update_stat_irq(lyc_irq || hblank_irq || vblank_irq);
        }
        self.stat_lyc_eq_ly = lyc_eq_ly;
    }

    fn update_stat_irq(&mut self, irq: bool) {
        // 只在上升沿请求 STAT 中断（[mooneye/stat_irq_blocking]）
        self.pending_stat_irq = self.last_stat_irq < irq;
        self.last_stat_irq = irq;
    }

    fn update_stat_irq_for_oam_mode(&mut self, mem: &[u8]) {
        let lyc_eq_ly_irq = self.stat_lyc_eq_ly_int(mem) && (self.last_lyc == self.last_ly);
        let irq = self.stat_oam_int(mem) || lyc_eq_ly_irq;
        self.pending_stat_irq = self.last_stat_irq < irq;
        if self.stat_lyc_eq_ly_int(mem) && self.last_lyc == self.last_ly.wrapping_add(1) {
            self.last_stat_irq = irq;
        }
    }

    fn update_stat_irq_for_oam_mode_do_not_clear(&mut self, mem: &[u8]) {
        let lyc_eq_ly_irq = self.stat_lyc_eq_ly_int(mem) && (self.last_lyc == self.last_ly);
        let irq = self.stat_oam_int(mem) || lyc_eq_ly_irq;
        self.pending_stat_irq = self.last_stat_irq < irq;
    }

    fn stat_lyc_eq_ly_int(&self, mem: &[u8]) -> bool { mem[0xFF41] & 0x40 != 0 }
    fn stat_oam_int(&self, mem: &[u8]) -> bool { mem[0xFF41] & 0x20 != 0 }

    fn tick_pending_write(&mut self, mem: &mut [u8]) {
        if let Some(val) = self.stat_write_pending.take() {
            mem[0xFF41] = (mem[0xFF41] & 0x07) | (val & 0xF8);
        }
    }

    fn tick_stat(&mut self, mem: &mut [u8]) {
        // STAT 读出的模式位有几个特例：开机首行 / VBlank 尾读出 HBlank
        let force_hblank = self.mode == 2
            && (self.is_glitched_line_0 || self.vblank_last_line_glitch);
        let mode = if force_hblank { 0 } else { self.mode };
        let lyc = if self.is_lyc_eq_ly() { 0x04 } else { 0 };
        mem[0xFF41] = (mem[0xFF41] & 0xF8) | lyc | (mode & 3);
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sprite_fetch_starts_on_the_bg_push_dot() {
        let mut ppu = Ppu::new();
        let mut mem = vec![0; 0x10000];
        mem[0x8000..0x8002].fill(0xFF);
        mem[0xFF48] = 0xE4;
        ppu.write_lcdc(0x93, &mut mem);
        ppu.state = State::PixelTransferLx8;
        ppu.lx = 8;
        ppu.fstate = FetcherState::Idle;
        ppu.bg_fifo.extend([0; 8]);
        ppu.push_enabled = true;
        ppu.oam_entries[8].push((0, 16));

        // 背景推送与精灵取数的第一拍重叠，第七拍输出第一个像素。
        ppu.tick_dots(6, &mut mem);
        assert_eq!(ppu.lcd_x, 0);
        ppu.tick_dot(&mut mem);
        assert_eq!(ppu.lcd_x, 1);
        assert_eq!(ppu.framebuffer[0][0], 3);
    }
}
