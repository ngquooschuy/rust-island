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
            WindowsAndMessaging::{SW_HIDE, SW_SHOWNOACTIVATE, ShowWindow},
        },
    },
    core::Result,
};

use crate::animation::Spring;
use crate::content::{self, Content};
use crate::graphics::Graphics;
use crate::layout::{self, COLLAPSED, EXPANDED, WIN_H, WIN_W};
use crate::media::MediaCommand;

pub struct App {
    graphics: Graphics,
    width: Spring,
    height: Spring,
    content: Content,
    hovered: bool,
    hover_button: Option<usize>,
    tracking: bool, // đã đăng ký nhận WM_MOUSELEAVE chưa?
    hidden: bool,   // đang ẩn vì có ứng dụng toàn màn hình?
    last_frame: Instant,
    commands: Sender<MediaCommand>,
}

/// Chỉ cho chuột/hình hiện trong vùng island, phần còn lại của cửa sổ bị cắt bỏ
fn set_region(hwnd: HWND, w: f32, h: f32) {
    unsafe {
        let left = ((WIN_W as f32 - w) / 2.0).round() as i32;
        let right = (left + w.round() as i32).min(WIN_W);
        let bottom = (h.round() as i32).min(WIN_H);
        let region = CreateRectRgn(left.max(0), 0, right, bottom);
        let _ = SetWindowRgn(hwnd, Some(region), true);
    }
}

impl App {
    pub fn new(graphics: Graphics, commands: Sender<MediaCommand>) -> Self {
        Self {
            graphics,
            width: Spring::new(COLLAPSED.0),
            height: Spring::new(COLLAPSED.1),
            content: content::current(),
            hovered: false,
            hover_button: None,
            tracking: false,
            hidden: false,
            last_frame: Instant::now(),
            commands,
        }
    }

    /// Vẽ khung hình đầu tiên khi vừa tạo cửa sổ
    pub fn show_first_frame(&self, hwnd: HWND) -> Result<()> {
        set_region(hwnd, self.width.value, self.height.value);
        self.redraw()
    }

    fn redraw(&self) -> Result<()> {
        self.graphics.render(
            self.width.value,
            self.height.value,
            &self.content,
            self.hover_button,
        )
    }

    pub fn is_animating(&self) -> bool {
        !(self.width.is_settled() && self.height.is_settled())
    }

    fn buttons_active(&self) -> bool {
        self.hovered && !self.content.title.is_empty()
    }

    fn set_hovered(&mut self, hwnd: HWND, hovered: bool) {
        if self.hovered == hovered {
            return;
        }
        self.hovered = hovered;
        let (w, h) = if hovered { EXPANDED } else { COLLAPSED };
        self.width.target = w;
        self.height.target = h;
        self.last_frame = Instant::now();
        // Mở vùng nhận chuột ra cả cửa sổ trong lúc co giãn
        set_region(hwnd, WIN_W as f32, WIN_H as f32);
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
            set_region(hwnd, self.width.value, self.height.value);
        }
        self.content = content::current();
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
        let inside = layout::island_contains(self.width.value, self.height.value, x, y);
        self.set_hovered(hwnd, inside);

        let hover_button = if self.buttons_active() {
            layout::button_at(x, y)
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

    pub fn on_click(&self, x: i32, y: i32) {
        if !self.buttons_active() {
            return;
        }
        if let Some(cmd) = layout::button_at(x as f32, y as f32).and_then(MediaCommand::from_button)
        {
            let _ = self.commands.send(cmd);
        }
    }

    /// Gọi mỗi giây: ẩn/hiện theo chế độ toàn màn hình, cập nhật đồng hồ
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
                self.content = content::current();
                let _ = self.redraw();
            }
        }
        if self.hidden || self.is_animating() {
            return; // đang ẩn thì không vẽ; đang co giãn thì tick() tự vẽ
        }
        let new = content::current();
        // Khi thu gọn chỉ thấy dòng ngắn, nên chỉ vẽ lại khi dòng đó đổi
        let changed = if self.hovered {
            new != self.content
        } else {
            new.short != self.content.short
        };
        if changed {
            self.content = new;
            let _ = self.redraw();
        }
    }

    pub fn on_media_changed(&mut self) {
        self.content = content::current();
        if !self.is_animating() && !self.hidden {
            let _ = self.redraw();
        }
    }
}
