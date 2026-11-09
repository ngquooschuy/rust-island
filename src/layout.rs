//! Kích thước cửa sổ, kích thước island và vị trí các nút, tính từ cấu hình.
//! Chỉ có tính toán thuần (không gọi Win32), nên dễ viết test.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

use crate::config::WindowConfig;

// Chiều cao cố định: vị trí chữ và nút trong graphics.rs được căn theo hai số này
const COLLAPSED_H: f32 = 36.0;
const EXPANDED_H: f32 = 100.0;
// Cửa sổ trong suốt rộng/cao hơn island mở rộng một chút
const MARGIN_X: f32 = 60.0;
const MARGIN_Y: f32 = 30.0;

const BTN_W: f32 = 44.0;
const BTN_H: f32 = 26.0;
/// Khoảng cách từ đáy island mở rộng lên đỉnh hàng nút
const BTN_FROM_BOTTOM: f32 = 30.0;
/// Khoảng cách giữa tâm hai nút liền nhau
const BTN_GAP: f32 = 64.0;
pub const BUTTON_COUNT: usize = 3;

/// `Copy` vì chỉ chứa vài con số: truyền đi đâu cũng sao chép, không cần mượn
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Kích thước cửa sổ trong suốt chứa island
    pub win_w: i32,
    pub win_h: i32,
    /// Khoảng cách từ mép trên màn hình
    pub top: i32,
    /// (rộng, cao) của island khi thu gọn và khi mở rộng
    pub collapsed: (f32, f32),
    pub expanded: (f32, f32),
}

impl Layout {
    pub fn new(window: &WindowConfig) -> Self {
        Self {
            win_w: (window.expanded_width + MARGIN_X).ceil() as i32,
            win_h: (EXPANDED_H + MARGIN_Y) as i32,
            top: window.top,
            collapsed: (window.collapsed_width, COLLAPSED_H),
            expanded: (window.expanded_width, EXPANDED_H),
        }
    }

    /// Mép trái của island rộng `w` khi căn giữa cửa sổ
    pub fn island_left(&self, w: f32) -> f32 {
        (self.win_w as f32 - w) / 2.0
    }

    /// Hình chữ nhật của nút thứ `i` (0 = lùi, 1 = phát/dừng, 2 = tới)
    pub fn button_rect(&self, i: usize) -> D2D_RECT_F {
        let cx = self.win_w as f32 / 2.0 + (i as f32 - 1.0) * BTN_GAP;
        let top = self.expanded.1 - BTN_FROM_BOTTOM;
        D2D_RECT_F {
            left: cx - BTN_W / 2.0,
            top,
            right: cx + BTN_W / 2.0,
            bottom: top + BTN_H,
        }
    }

    /// Nút nằm dưới điểm (x, y), nếu có
    pub fn button_at(&self, x: f32, y: f32) -> Option<usize> {
        (0..BUTTON_COUNT).find(|&i| {
            let r = self.button_rect(i);
            x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
        })
    }

    /// Điểm (x, y) có nằm trong island rộng `w`, cao `h` không?
    pub fn island_contains(&self, w: f32, h: f32, x: f32, y: f32) -> bool {
        let left = self.island_left(w);
        x >= left && x <= left + w && y >= 0.0 && y <= h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> Layout {
        Layout::new(&WindowConfig::default())
    }

    fn center(r: D2D_RECT_F) -> (f32, f32) {
        ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
    }

    #[test]
    fn default_config_gives_original_window_size() {
        let l = layout();
        assert_eq!((l.win_w, l.win_h), (460, 130));
        assert_eq!(l.button_rect(1).top, 70.0);
    }

    #[test]
    fn window_grows_with_expanded_width() {
        let wide = Layout::new(&WindowConfig {
            expanded_width: 600.0,
            ..WindowConfig::default()
        });
        assert_eq!(wide.win_w, 660);
    }

    #[test]
    fn button_at_finds_each_button_at_its_center() {
        let l = layout();
        for i in 0..BUTTON_COUNT {
            let (x, y) = center(l.button_rect(i));
            assert_eq!(l.button_at(x, y), Some(i));
        }
    }

    #[test]
    fn button_at_returns_none_between_and_outside_buttons() {
        let l = layout();
        let (x0, y) = center(l.button_rect(0));
        let (x1, _) = center(l.button_rect(1));
        assert_eq!(l.button_at((x0 + x1) / 2.0, y), None); // khe giữa hai nút
        assert_eq!(l.button_at(x1, 0.0), None); // phía trên hàng nút
    }

    #[test]
    fn buttons_fit_inside_expanded_island() {
        // Kiểm tra cả island hẹp nhất cho phép
        for width in [300.0, 400.0, 700.0] {
            let l = Layout::new(&WindowConfig {
                expanded_width: width,
                ..WindowConfig::default()
            });
            let (w, h) = l.expanded;
            for i in 0..BUTTON_COUNT {
                let r = l.button_rect(i);
                assert!(l.island_contains(w, h, r.left, r.top));
                assert!(l.island_contains(w, h, r.right, r.bottom));
            }
        }
    }

    #[test]
    fn island_contains_checks_centered_pill() {
        let l = layout();
        let (w, h) = l.collapsed;
        let mid = l.win_w as f32 / 2.0;
        assert!(l.island_contains(w, h, mid, h / 2.0));
        assert!(!l.island_contains(w, h, 5.0, h / 2.0)); // lệch trái
        assert!(!l.island_contains(w, h, mid, h + 1.0)); // dưới island
    }
}
