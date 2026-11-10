//! Trạng thái của island và cách nó phản ứng với từng sự kiện.
//! `wnd_proc` trong main.rs chỉ đọc thông điệp Windows rồi gọi các hàm `on_*` ở đây.

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};
use windows::{
    Win32::Graphics::Direct2D::ID2D1Bitmap1,
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

use crate::alerts::{self, Alert};
use crate::animation::{LineSlide, Spring};
use crate::backdrop::Backdrop;
use crate::clipboard::{self, Clip};
use crate::config::{AutoHideConfig, ClockConfig, Config};
use crate::content::{self, Content, Page};
use crate::graphics::{AlertView, CompactLyric, Frame, Graphics};
use crate::layout::{BUTTON_COUNT, LYRICS_BUTTON, Layout};
use crate::media::MediaCommand;

/// Một nấc con lăn chuột (WHEEL_DELTA); touchpad gửi nhiều số nhỏ cộng dồn lại
const WHEEL_NOTCH: i32 = 120;

/// Timer mỗi giây: đồng hồ, toàn màn hình, tự ẩn, thanh tiến độ
pub const TIMER_CLOCK: usize = 1;
/// Timer nhanh, chỉ chạy khi đang hiện lời bài hát, để dòng lời đổi đúng nhịp
pub const TIMER_LYRICS: usize = 2;
const LYRICS_TICK_MS: u32 = 150;
/// Timer hết giờ hiện thông báo tạm thời
pub const TIMER_ALERT: usize = 3;
/// Timer chờ clipboard ổn định: một lần sao chép, app nguồn có thể ghi nhiều đợt liền nhau
pub const TIMER_CLIPBOARD: usize = 4;
const CLIPBOARD_SETTLE_MS: u32 = 150;
/// Rê chuột lên thông báo thì kéo dài thêm từng quãng này
const ALERT_HOLD_MS: u32 = 1500;
/// Thông báo hiện rõ dần / mờ đi trong bao lâu
const ALERT_FADE_IN: Duration = Duration::from_millis(260);
const ALERT_FADE_OUT: Duration = Duration::from_millis(200);

/// Thông báo tạm thời đang hiện trên island
struct ShownAlert {
    alert: Alert,
    /// Biểu tượng đã đưa lên card đồ họa
    icon: Option<ID2D1Bitmap1>,
    since: Instant,
    /// Lúc bắt đầu mờ đi (đã hết giờ, island đang thu về)
    closing: Option<Instant>,
}

impl ShownAlert {
    fn alpha(&self, now: Instant) -> f32 {
        let ratio = |from: Instant, span: Duration| {
            (now.saturating_duration_since(from).as_secs_f32() / span.as_secs_f32()).min(1.0)
        };
        match self.closing {
            Some(t) => 1.0 - ratio(t, ALERT_FADE_OUT),
            None => ratio(self.since, ALERT_FADE_IN),
        }
    }

    /// Còn đang hiện rõ dần hoặc mờ đi?
    fn fading(&self, now: Instant) -> bool {
        match self.closing {
            Some(t) => now.saturating_duration_since(t) < ALERT_FADE_OUT,
            None => now.saturating_duration_since(self.since) < ALERT_FADE_IN,
        }
    }
}

pub struct App {
    graphics: Graphics,
    layout: Layout,
    /// Lớp kính mờ phía sau island (None nếu tắt trong cấu hình)
    backdrop: Option<Backdrop>,
    clock: ClockConfig,
    autohide: AutoHideConfig,
    lyrics_enabled: bool,   // bật lời bài hát trong cấu hình?
    show_lyrics: bool,      // đang hiện lời? (nút bên phải hàng nút bật/tắt)
    lyrics_collapsed: bool, // hiện câu đang hát trên island thu gọn? (menu chuột phải)
    slide: LineSlide,       // câu đang hiện trên island thu gọn, có hiệu ứng trượt
    line_width: f32,        // island thu gọn rộng bao nhiêu để vừa câu đó
    fast_timer: bool,       // TIMER_LYRICS đang chạy?
    keep_tall: bool,        // vừa tắt lời khi đang rê chuột: giữ chiều cao tới lúc rời chuột
    width: Spring,
    height: Spring,
    content: Content,
    page: Page,
    wheel: i32, // con lăn cộng dồn, chưa đủ một nấc
    hovered: bool,
    notch: bool,          // đang tự ẩn thành notch? (không bao giờ đúng khi `hovered`)
    last_active: Instant, // lần cuối island được dùng, để đếm giờ tự ẩn
    hover_button: Option<usize>,
    tracking: bool,                // đã đăng ký nhận WM_MOUSELEAVE chưa?
    region: Cell<(i32, i32, i32)>, // vùng cửa sổ đang đặt (trái, phải, đáy), khỏi đặt lại
    hidden: bool,                  // đang ẩn vì có ứng dụng toàn màn hình?
    last_frame: Instant,
    commands: Sender<MediaCommand>,
    /// Gửi lần sao chép sang luồng nền (None: tắt hiện nội dung vừa sao chép)
    clips: Option<Sender<Clip>>,
    alert_ms: u32,                // mỗi thông báo tạm thời hiện bao lâu
    alert_queue: VecDeque<Alert>, // thông báo đang chờ tới lượt
    alert: Option<ShownAlert>,    // thông báo đang hiện (hoặc đang mờ đi)
}

impl App {
    pub fn new(
        graphics: Graphics,
        layout: Layout,
        backdrop: Option<Backdrop>,
        config: &Config,
        commands: Sender<MediaCommand>,
        clips: Option<Sender<Clip>>,
    ) -> Self {
        let lyrics_enabled = config.lyrics.enabled;
        let content = content::current(&config.clock, lyrics_enabled);
        Self {
            graphics,
            layout,
            backdrop,
            clock: config.clock,
            autohide: config.autohide,
            lyrics_enabled,
            show_lyrics: lyrics_enabled && config.lyrics.show,
            lyrics_collapsed: lyrics_enabled && config.lyrics.collapsed,
            slide: LineSlide::default(),
            line_width: layout.collapsed.0,
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
            region: Cell::new((0, 0, 0)),
            hidden: false,
            last_frame: Instant::now(),
            commands,
            clips,
            alert_ms: config.alerts.seconds * 1000,
            alert_queue: VecDeque::new(),
            alert: None,
        }
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn backdrop(&self) -> Option<&Backdrop> {
        self.backdrop.as_ref()
    }

    /// Hiện/ẩn island cùng lớp kính mờ phía sau
    pub fn show(&self, hwnd: HWND, visible: bool) {
        if let Some(b) = &self.backdrop {
            b.show(visible);
        }
        let cmd = if visible { SW_SHOWNOACTIVATE } else { SW_HIDE };
        unsafe {
            let _ = ShowWindow(hwnd, cmd);
        }
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
        // Hiệu ứng trượt chữ vẽ lại liên tục mà kích thước không đổi: khỏi đặt lại vùng
        if self.region.replace((left, right, bottom)) == (left, right, bottom) {
            return;
        }
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
        let (w, h) = (self.width.value, self.height.value);
        if let Some(b) = &self.backdrop {
            // Lớp kính đi theo hình island từng khung hình
            b.set_shape(self.layout.island_left(w), w, h, Layout::corner_radius(h));
        }
        self.graphics.render(&Frame {
            w: self.width.value,
            h: self.height.value,
            content: &self.content,
            page: self.page,
            hover_button: self.hover_button,
            lyrics_on: self.show_lyrics,
            compact_lyric: self.slide.current.as_deref().map(|current| CompactLyric {
                current,
                previous: self.slide.previous.as_deref(),
                progress: self.slide.progress(Instant::now()),
            }),
            alert: self.alert.as_ref().map(|a| AlertView {
                alert: &a.alert,
                icon: a.icon.as_ref(),
                alpha: a.alpha(Instant::now()),
            }),
        })
    }

    /// Vẽ lại ngay nếu không đang ẩn hoặc đang co giãn (khi đó tick() tự vẽ)
    fn redraw_if_idle(&self) {
        if !self.is_animating() && !self.hidden {
            let _ = self.redraw();
        }
    }

    /// Island đang co giãn?
    fn resizing(&self) -> bool {
        !(self.width.is_settled() && self.height.is_settled())
    }

    /// Đang có hiệu ứng (co giãn hoặc trượt chữ): vòng lặp chính vẽ liên tục
    pub fn is_animating(&self) -> bool {
        let now = Instant::now();
        self.resizing()
            || self.slide.is_running(now)
            || self.alert.as_ref().is_some_and(|a| a.fading(now))
    }

    /// Đang hiện thông báo tạm thời (chưa tới lúc mờ đi)?
    fn alert_active(&self) -> bool {
        self.alert.as_ref().is_some_and(|a| a.closing.is_none())
    }

    /// Câu lời nên hiện trên island thu gọn lúc này (trang Media, đang phát, có lời đồng bộ)
    fn collapsed_line(&self) -> Option<&str> {
        let wanted = self.lyrics_collapsed && self.page == Page::Media && self.content.playing;
        wanted.then(|| self.content.lyrics.current_line()).flatten()
    }

    /// Cập nhật câu trên island thu gọn: đổi câu thì trượt câu mới vào và giãn island cho vừa
    fn refresh_line(&mut self, hwnd: HWND) {
        let line = self.collapsed_line().map(str::to_owned);
        if self.slide.set(line.as_deref(), Instant::now()) {
            if let Some(line) = &line {
                self.line_width = self.graphics.compact_width(line);
            }
            self.sync_target(hwnd);
        }
    }

    /// Thay nội dung mới (mọi chỗ đổi nội dung đều đi qua đây để câu lời luôn khớp)
    fn set_content(&mut self, hwnd: HWND, content: Content) {
        self.content = content;
        self.refresh_line(hwnd);
    }

    fn set_page(&mut self, hwnd: HWND, page: Page) {
        if page != self.page {
            self.page = page;
            self.hover_button = None; // nút media không còn trên trang mới
            self.refresh_line(hwnd); // câu lời chỉ hiện ở trang Media
            self.sync_target(hwnd); // trang Media có lời thì cao hơn
            self.update_fast_timer(hwnd);
            self.redraw_if_idle();
        }
    }

    fn buttons_active(&self) -> bool {
        self.hovered
            && !self.alert_active()
            && self.page == Page::Media
            && !self.content.title.is_empty()
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
        self.update_fast_timer(hwnd); // lời trên khung mở rộng và trên island thu gọn bật riêng
    }

    /// Kích thước island nên có theo trạng thái hiện tại
    fn target_size(&self) -> (f32, f32) {
        if self.alert_active() {
            // Thông báo tạm thời được ưu tiên, kể cả khi đang rê chuột (để đọc cho kịp)
            self.layout.alert_size()
        } else if self.hovered {
            // Giữ khung cao sau khi vừa tắt lời, để island không co lại ngay dưới con trỏ
            let panel = self.content.shows_lyrics_panel(self.page, self.show_lyrics)
                || (self.keep_tall && self.content.shows_lyrics_panel(self.page, true));
            self.layout.expanded_size(panel)
        } else if self.notch {
            self.layout.notch
        } else if self.slide.current.is_some() {
            // Đang hiện câu lời: giãn ngang cho vừa câu
            (self.line_width, self.layout.collapsed.1)
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
    /// Mở rộng thì theo nút lời trên hàng nút, thu gọn thì theo tùy chọn trong menu
    fn lyrics_live(&self) -> bool {
        let shown = if self.hovered {
            self.show_lyrics
        } else {
            self.lyrics_collapsed
        };
        shown
            && !self.hidden
            && !self.notch
            && !self.alert_active()
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

        if !self.resizing() {
            self.width.snap();
            self.height.snap();
            let (w, h) = self.hit_size();
            self.set_region(hwnd, w, h);
        }
        // Thông báo đã mờ hẳn thì bỏ đi
        if self
            .alert
            .as_ref()
            .is_some_and(|a| a.closing.is_some() && !a.fading(now))
        {
            self.alert = None;
        }
        self.set_content(hwnd, self.fresh_content());
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
        // Bấm vào thông báo tạm thời: đóng nó (rồi hiện cái kế tiếp nếu có)
        if self.alert_active() {
            self.close_alert(hwnd);
            return;
        }
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
            if fullscreen {
                // Đang xem phim, chơi game: bỏ hết thông báo tạm thời, không làm phiền
                self.alert_queue.clear();
                self.alert = None;
                unsafe {
                    let _ = KillTimer(Some(hwnd), TIMER_ALERT);
                }
                self.sync_target(hwnd);
            }
            self.show(hwnd, !fullscreen);
            if !fullscreen {
                self.set_content(hwnd, self.fresh_content());
                self.redraw_if_idle();
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
            self.set_content(hwnd, new);
            self.redraw_if_idle();
        }
    }

    /// Timer nhanh: chỉ vẽ lại khi dòng lời đổi
    pub fn on_lyrics_tick(&mut self, hwnd: HWND) {
        if self.is_animating() || self.hidden || self.notch {
            return;
        }
        let new = self.fresh_content();
        if new.lyrics != self.content.lyrics {
            self.set_content(hwnd, new); // đổi câu thì bắt đầu trượt, tick() vẽ tiếp
            self.redraw_if_idle();
        }
    }

    /// Tùy chọn "lời bài hát trên island thu gọn"; None nếu đã tắt lời bài hát trong cấu hình
    pub fn lyrics_collapsed(&self) -> Option<bool> {
        self.lyrics_enabled.then_some(self.lyrics_collapsed)
    }

    pub fn set_lyrics_collapsed(&mut self, hwnd: HWND, on: bool) {
        self.lyrics_collapsed = on && self.lyrics_enabled;
        self.refresh_line(hwnd);
        self.update_fast_timer(hwnd);
        self.redraw_if_idle();
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
            self.refresh_line(hwnd);
            self.sync_target(hwnd); // hết bài thì khung lời biến mất
            self.update_fast_timer(hwnd);
            self.redraw_if_idle();
        }
    }

    pub fn on_lyrics_changed(&mut self, hwnd: HWND) {
        self.set_content(hwnd, self.fresh_content());
        self.update_fast_timer(hwnd);
        self.redraw_if_idle();
    }

    // ---------- Thông báo tạm thời ----------

    /// Windows báo clipboard đổi: chờ một chút cho app nguồn ghi xong rồi mới đọc
    pub fn on_clipboard_update(&mut self, hwnd: HWND) {
        if self.clips.is_some() {
            unsafe {
                SetTimer(Some(hwnd), TIMER_CLIPBOARD, CLIPBOARD_SETTLE_MS, None);
            }
        }
    }

    /// Đọc lần sao chép rồi gửi sang luồng nền tìm tên và biểu tượng app
    pub fn on_clipboard_timer(&mut self, hwnd: HWND) {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_CLIPBOARD);
        }
        if self.hidden {
            return;
        }
        if let (Some(clips), Some(clip)) = (&self.clips, clipboard::read(hwnd)) {
            let _ = clips.send(clip);
        }
    }

    /// Luồng nền báo có thông báo mới
    pub fn on_alerts(&mut self, hwnd: HWND) {
        let fresh = alerts::take();
        if self.hidden {
            return;
        }
        for alert in fresh {
            alerts::push(&mut self.alert_queue, alert);
        }
        if !self.alert_active() {
            self.show_next_alert(hwnd);
        }
    }

    /// Hiện thông báo kế tiếp trong hàng đợi; hết thì island thu về như cũ
    fn show_next_alert(&mut self, hwnd: HWND) {
        let Some(alert) = self.alert_queue.pop_front() else {
            return;
        };
        let icon = alert
            .icon
            .as_ref()
            .and_then(|i| self.graphics.icon_bitmap(i));
        self.alert = Some(ShownAlert {
            alert,
            icon,
            since: Instant::now(),
            closing: None,
        });
        unsafe {
            SetTimer(Some(hwnd), TIMER_ALERT, self.alert_ms, None);
        }
        self.wake(hwnd); // đang là notch thì hiện lại
        self.hover_button = None;
        self.sync_target(hwnd);
        self.update_fast_timer(hwnd);
        self.redraw_if_idle();
    }

    /// Đóng thông báo đang hiện: còn cái khác đang chờ thì hiện luôn, không thì mờ đi
    fn close_alert(&mut self, hwnd: HWND) {
        unsafe {
            let _ = KillTimer(Some(hwnd), TIMER_ALERT);
        }
        if !self.alert_queue.is_empty() {
            self.show_next_alert(hwnd);
            return;
        }
        if let Some(a) = &mut self.alert {
            a.closing = Some(Instant::now());
        }
        self.last_active = Instant::now(); // đếm lại giờ tự ẩn từ lúc thông báo xong
        self.sync_target(hwnd);
        self.update_fast_timer(hwnd);
        self.redraw_if_idle();
    }

    /// Hết giờ hiện thông báo; đang rê chuột lên thì giữ thêm một chút
    pub fn on_alert_timer(&mut self, hwnd: HWND) {
        if self.hovered && self.alert_active() {
            unsafe {
                SetTimer(Some(hwnd), TIMER_ALERT, ALERT_HOLD_MS, None);
            }
        } else {
            self.close_alert(hwnd);
        }
    }

    pub fn on_weather_changed(&mut self, hwnd: HWND) {
        self.set_content(hwnd, self.fresh_content());
        self.redraw_if_idle();
    }
}
