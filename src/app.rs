//! Trạng thái của island và cách nó phản ứng với từng sự kiện.
//! `wnd_proc` trong main.rs chỉ đọc thông điệp Windows rồi gọi các hàm `on_*` ở đây.

use std::sync::mpsc::Sender;
use std::time::Instant;
use windows::{
    Win32::{
        Foundation::HWND,
        Graphics::Gdi::{CreateRectRgn, SetWindowRgn},
        UI::{
            Input::KeyboardAndMouse::{TME_LEAVE, TRACKMOUSEEVENT, TrackMouseEvent},
            WindowsAndMessaging::{KillTimer, SW_HIDE, SW_SHOWNOACTIVATE, SetTimer, ShowWindow},
        },
    },
    core::Result,
};

use crate::animation::Spring;
use crate::config::{AutoHideConfig, ClockConfig, Config};
use crate::content::{self, Content, Page};
use crate::graphics::{Frame, Graphics};
use crate::layout::{BUTTON_COUNT, LYRICS_BUTTON, Layout};
use crate::media::MediaCommand;

/// Một nấc con lăn chuột (WHEEL_DELTA); touchpad gửi nhiều số nhỏ cộng dồn lại
const WHEEL_NOTCH: i32 = 120;

/// Timer mỗi giây: đồng hồ, toàn màn hình, tự ẩn, thanh tiến độ
pub const TIMER_CLOCK: usize = 1;
/// Timer nhanh, chỉ chạy khi đang hiện lời bài hát, để dòng lời đổi đúng nhịp
pub const TIMER_LYRICS: usize = 2;
const LYRICS_TICK_MS: u32 = 150;

pub struct App {
    graphics: Graphics,
    layout: Layout,
    clock: ClockConfig,
    autohide: AutoHideConfig,
    lyrics_enabled: bool, // bật lời bài hát trong cấu hình?
    show_lyrics: bool,    // đang hiện lời? (nút bên phải hàng nút bật/tắt)
    fast_timer: bool,     // TIMER_LYRICS đang chạy?
    keep_tall: bool,      // vừa tắt lời khi đang rê chuột: giữ chiều cao tới lúc rời chuột
    width: Spring,
    height: Spring,
    content: Content,
    page: Page,
    wheel: i32, // con lăn cộng dồn, chưa đủ một nấc
    hovered: bool,
    notch: bool,          // đang tự ẩn thành notch? (không bao giờ đúng khi `hovered`)
    last_active: Instant, // lần cuối island được dùng, để đếm giờ tự ẩn
    hover_button: Option<usize>,
    tracking: bool, // đã đăng ký nhận WM_MOUSELEAVE chưa?
    hidden: bool,   // đang ẩn vì có ứng dụng toàn màn hình?
    last_frame: Instant,
    commands: Sender<MediaCommand>,
}

impl App {
    pub fn new(
        graphics: Graphics,
        layout: Layout,
        config: &Config,
        commands: Sender<MediaCommand>,
    ) -> Self {
        let lyrics_enabled = config.lyrics.enabled;
        let content = content::current(&config.clock, lyrics_enabled);
        Self {
            graphics,
            layout,
            clock: config.clock,
            autohide: config.autohide,
            lyrics_enabled,
            show_lyrics: lyrics_enabled && config.lyrics.show,
            fast_timer: false,
            keep_tall: false,
            width: Spring::new(layout.collapsed.0),
            height: Spring::new(layout.collapsed.1),
            page: Page::initial(&content),
            content,
            wheel: 0,
            hovered: false,
            notch: false,
            last_active: Instant::now(),
            hover_button: None,
            tracking: false,
            hidden: false,
            last_frame: Instant::now(),
            commands,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    fn fresh_content(&self) -> Content {
        content::current(&self.clock, self.lyrics_enabled)
    }

    /// Chỉ cho chuột/hình hiện trong vùng island, phần còn lại của cửa sổ bị cắt bỏ
    fn set_region(&self, hwnd: HWND, w: f32, h: f32) {
        let (win_w, win_h) = (self.layout.win_w, self.layout.win_h);
        let left = self.layout.island_left(w).round() as i32;
        let right = (left + w.round() as i32).min(win_w);
        let bottom = (h.round() as i32).min(win_h);
        unsafe {
            let region = CreateRectRgn(left.max(0), 0, right, bottom);
            let _ = SetWindowRgn(hwnd, Some(region), true);
        }
    }

    /// Vẽ khung hình đầu tiên khi vừa tạo cửa sổ
    pub fn show_first_frame(&self, hwnd: HWND) -> Result<()> {
        self.set_region(hwnd, self.width.value, self.height.value);
        self.redraw()
    }

    fn redraw(&self) -> Result<()> {
        self.graphics.render(&Frame {
            w: self.width.value,
            h: self.height.value,
            content: &self.content,
            page: self.page,
            hover_button: self.hover_button,
            lyrics_on: self.show_lyrics,
        })
    }

    /// Vẽ lại ngay nếu không đang ẩn hoặc đang co giãn (khi đó tick() tự vẽ)
    fn redraw_if_idle(&self) {
        if !self.is_animating() && !self.hidden {
            let _ = self.redraw();
        }
    }

    pub fn is_animating(&self) -> bool {
        !(self.width.is_settled() && self.height.is_settled())
    }

    fn set_page(&mut self, hwnd: HWND, page: Page) {
        if page != self.page {
            self.page = page;
            self.hover_button = None; // nút media không còn trên trang mới
            self.sync_target(hwnd); // trang Media có lời thì cao hơn
            self.update_fast_timer(hwnd);
            self.redraw_if_idle();
        }
    }

    fn buttons_active(&self) -> bool {
        self.hovered && self.page == Page::Media && !self.content.title.is_empty()
    }

    /// Số nút bấm được trên hàng nút: 3 nút nhạc, thêm nút lời nếu bật lời bài hát
    fn button_count(&self) -> usize {
        BUTTON_COUNT + usize::from(self.lyrics_enabled)
    }

    fn set_hovered(&mut self, hwnd: HWND, hovered: bool) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        self.keep_tall = false;
        // Rê vào thì hiện lại; rời đi thì về dạng thu gọn rồi mới đếm giờ tự ẩn
        self.notch = false;
        self.last_active = Instant::now();
        self.retarget(hwnd);
    }

    /// Kích thước island nên có theo trạng thái hiện tại
    fn target_size(&self) -> (f32, f32) {
        if self.hovered {
            // Giữ khung cao sau khi vừa tắt lời, để island không co lại ngay dưới con trỏ
            let panel = self.content.shows_lyrics_panel(self.page, self.show_lyrics)
                || (self.keep_tall && self.content.shows_lyrics_panel(self.page, true));
            self.layout.expanded_size(panel)
        } else if self.notch {
            self.layout.notch
        } else {
            self.layout.collapsed
        }
    }

    /// Đặt đích mới cho lò xo và bắt đầu co giãn tới đó
    fn retarget(&mut self, hwnd: HWND) {
        let (w, h) = self.target_size();
        self.width.target = w;
        self.height.target = h;
        self.last_frame = Instant::now();
        // Mở vùng nhận chuột ra cả cửa sổ trong lúc co giãn
        let (win_w, win_h) = (self.layout.win_w as f32, self.layout.win_h as f32);
        self.set_region(hwnd, win_w, win_h);
    }

    /// Co giãn lại chỉ khi kích thước đích thật sự đổi (đổi trang, bật/tắt lời, hết bài...)
    fn sync_target(&mut self, hwnd: HWND) {
        let (w, h) = self.target_size();
        if w != self.width.target || h != self.height.target {
            self.retarget(hwnd);
        }
    }

    /// Kích thước vùng nhận chuột: bằng island, riêng notch thì rộng hơn hình vẽ một chút
    fn hit_size(&self) -> (f32, f32) {
        let (w, h) = (self.width.value, self.height.value);
        if self.notch {
            let (hit_w, hit_h) = self.layout.notch_hit();
            (w.max(hit_w), h.max(hit_h))
        } else {
            (w, h)
        }
    }

    /// Có hoạt động mới: đếm lại giờ tự ẩn, và hiện lại nếu đang là notch
    fn wake(&mut self, hwnd: HWND) {
        self.last_active = Instant::now();
        if self.notch {
            self.notch = false;
            self.retarget(hwnd);
        }
    }

    /// Notch Auto-Hide: không dùng đủ lâu thì thu nhỏ thành notch
    fn update_auto_hide(&mut self, hwnd: HWND) {
        let playing = self.content.playing && !self.content.title.is_empty();
        if self.autohide.in_use(self.hovered, playing) {
            self.last_active = Instant::now();
        } else if !self.notch && self.autohide.is_due(self.last_active.elapsed()) {
            self.notch = true;
            self.retarget(hwnd);
        }
    }

    /// Đang hiện lời chạy theo nhạc (trang Media, đang phát, có lời đồng bộ)?
    fn lyrics_live(&self) -> bool {
        self.show_lyrics
            && !self.hidden
            && !self.notch
            && self.page == Page::Media
            && self.content.playing
            && self.content.lyrics.current_line().is_some()
    }

    /// Bật timer nhanh khi đang hiện lời, tắt khi không cần để máy được nghỉ
    fn update_fast_timer(&mut self, hwnd: HWND) {
        let want = self.lyrics_live();
        if want == self.fast_timer {
            return;
        }
        self.fast_timer = want;
        unsafe {
            if want {
                SetTimer(Some(hwnd), TIMER_LYRICS, LYRICS_TICK_MS, None);
            } else {
                let _ = KillTimer(Some(hwnd), TIMER_LYRICS);
            }
        }
    }

    /// Một bước animation; vòng lặp chính gọi liên tục khi `is_animating()`
    pub fn tick(&mut self, hwnd: HWND) -> Result<()> {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(1.0 / 30.0);
        self.last_frame = now;

        self.width.update(dt);
        self.height.update(dt);

        if !self.is_animating() {
            self.width.snap();
            self.height.snap();
            let (w, h) = self.hit_size();
            self.set_region(hwnd, w, h);
        }
        self.content = self.fresh_content();
        self.redraw()
    }

    pub fn on_mouse_move(&mut self, hwnd: HWND, x: i32, y: i32) {
        if !self.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.tracking = true;
        }
        let (x, y) = (x as f32, y as f32);
        let (w, h) = self.hit_size();
        let inside = self.layout.island_contains(w, h, x, y);
        self.set_hovered(hwnd, inside);

        let hover_button = if self.buttons_active() {
            self.layout
                .button_at(x, y, self.height.value, self.button_count())
        } else {
            None
        };
        if hover_button != self.hover_button {
            self.hover_button = hover_button;
            if !self.is_animating() {
                let _ = self.redraw();
            }
        }
    }

    pub fn on_mouse_leave(&mut self, hwnd: HWND) {
        self.tracking = false;
        self.hover_button = None;
        self.set_hovered(hwnd, false);
    }

    pub fn on_click(&mut self, hwnd: HWND, x: i32, y: i32) {
        let (x, y) = (x as f32, y as f32);
        // Bấm vào hàng chấm phân trang: sang trang kế
        if self.layout.dots_hit(self.height.value, x, y) {
            self.set_page(hwnd, self.page.step(true));
            return;
        }
        if !self.buttons_active() {
            return;
        }
        match self
            .layout
            .button_at(x, y, self.height.value, self.button_count())
        {
            Some(LYRICS_BUTTON) => {
                self.show_lyrics = !self.show_lyrics;
                self.keep_tall = !self.show_lyrics;
                self.sync_target(hwnd);
                self.update_fast_timer(hwnd);
                self.redraw_if_idle();
            }
            Some(i) => {
                if let Some(cmd) = MediaCommand::from_button(i) {
                    let _ = self.commands.send(cmd);
                }
            }
            None => {}
        }
    }

    /// Lăn chuột trên island: lăn xuống sang trang sau, lăn lên về trang trước
    pub fn on_wheel(&mut self, hwnd: HWND, delta: i16) {
        self.wheel += i32::from(delta);
        if self.wheel.abs() >= WHEEL_NOTCH {
            let forward = self.wheel < 0;
            self.wheel = 0;
            self.set_page(hwnd, self.page.step(forward));
        }
    }

    /// Gọi mỗi giây: ẩn/hiện theo chế độ toàn màn hình, tự ẩn thành notch, cập nhật đồng hồ
    pub fn on_timer(&mut self, hwnd: HWND, fullscreen: bool) {
        if fullscreen != self.hidden {
            self.hidden = fullscreen;
            let cmd = if fullscreen {
                SW_HIDE
            } else {
                SW_SHOWNOACTIVATE
            };
            unsafe {
                let _ = ShowWindow(hwnd, cmd);
            }
            if !fullscreen {
                self.content = self.fresh_content();
                let _ = self.redraw();
            }
        }
        if self.hidden {
            self.update_fast_timer(hwnd);
            return; // đang ẩn vì toàn màn hình: không vẽ, không đếm giờ
        }
        self.update_auto_hide(hwnd);
        self.update_fast_timer(hwnd);
        if self.is_animating() || self.notch {
            return; // đang co giãn thì tick() tự vẽ; notch thì không có nội dung để vẽ
        }
        let new = self.fresh_content();
        // Khi thu gọn không hiện giây, nên chỉ vẽ lại khi phần nhìn thấy đổi
        let changed = if self.hovered {
            new != self.content
        } else {
            !new.same_when_collapsed(&self.content)
        };
        if changed {
            self.content = new;
            let _ = self.redraw();
        }
    }

    /// Timer nhanh: chỉ vẽ lại khi dòng lời đổi
    pub fn on_lyrics_tick(&mut self) {
        if self.is_animating() || self.hidden || self.notch {
            return;
        }
        let new = self.fresh_content();
        if new.lyrics != self.content.lyrics {
            self.content = new;
            let _ = self.redraw();
        }
    }

    pub fn on_media_changed(&mut self, hwnd: HWND) {
        let new = self.fresh_content();
        if new.started_playback_since(&self.content) {
            self.wake(hwnd); // bài mới: hiện lại nếu đang là notch
        }
        let page = self.page.after_media_change(&self.content, &new);
        self.content = new;
        if page != self.page {
            self.set_page(hwnd, page); // set_page tự vẽ lại
        } else {
            self.sync_target(hwnd); // hết bài thì khung lời biến mất
            self.update_fast_timer(hwnd);
            self.redraw_if_idle();
        }
    }

    pub fn on_lyrics_changed(&mut self, hwnd: HWND) {
        self.content = self.fresh_content();
        self.update_fast_timer(hwnd);
        self.redraw_if_idle();
    }

    pub fn on_weather_changed(&mut self) {
        self.content = self.fresh_content();
        self.redraw_if_idle();
    }
}
