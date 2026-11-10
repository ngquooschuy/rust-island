//! Kích thước cửa sổ, kích thước island và vị trí các nút, tính từ cấu hình.
//! Chỉ có tính toán thuần (không gọi Win32), nên dễ viết test.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

use crate::config::WindowConfig;

// Chiều cao cố định: vị trí chữ và nút trong graphics.rs được căn theo hai số này
const COLLAPSED_H: f32 = 44.0;
const EXPANDED_H: f32 = 116.0;
/// Trang Media khi mở rộng có khung lời bài hát: cao hơn để chứa 3 dòng lời và thanh tiến độ
const LYRICS_H: f32 = 200.0;
/// Thông báo tạm thời (vừa sao chép, thông báo Windows): đủ cao cho biểu tượng và 3 dòng chữ
const ALERT_H: f32 = 84.0;
/// Island khi tự ẩn: chỉ còn một vạch nhỏ (notch)
const NOTCH: (f32, f32) = (72.0, 8.0);
/// Vùng nhận chuột quanh notch rộng hơn hình vẽ (mỗi bên, và phía dưới) để dễ rê trúng
const NOTCH_HIT_PAD: (f32, f32) = (16.0, 8.0);
// Cửa sổ trong suốt rộng/cao hơn island mở rộng một chút
const MARGIN_X: f32 = 60.0;
const MARGIN_Y: f32 = 30.0;

const BTN_W: f32 = 44.0;
const BTN_H: f32 = 26.0;
/// Khoảng cách từ đáy island mở rộng lên đỉnh hàng nút (chừa chỗ cho chấm phân trang)
const BTN_FROM_BOTTOM: f32 = 46.0;

/// Tâm hàng chấm phân trang cách đáy island bao nhiêu
const DOTS_FROM_BOTTOM: f32 = 8.0;
/// Vùng bấm quanh hàng chấm (rộng hơn chấm để dễ bấm)
const DOTS_HIT_HALF_W: f32 = 24.0;
const DOTS_HIT_H: f32 = 16.0;
/// Khoảng cách giữa tâm hai nút liền nhau
const BTN_GAP: f32 = 64.0;
/// Ba nút điều khiển nhạc: lùi, phát/dừng, tới
pub const BUTTON_COUNT: usize = 3;
/// Nút thứ tư (bật/tắt lời bài hát) nằm sát mép phải của island
pub const LYRICS_BUTTON: usize = 3;
const LYRICS_BTN_W: f32 = 36.0;
const LYRICS_BTN_FROM_RIGHT: f32 = 16.0;

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
    /// (rộng, cao) khi tự ẩn thành notch
    pub notch: (f32, f32),
}

impl Layout {
    pub fn new(window: &WindowConfig) -> Self {
        Self {
            win_w: (window.expanded_width + MARGIN_X).ceil() as i32,
            win_h: (EXPANDED_H.max(LYRICS_H) + MARGIN_Y) as i32,
            top: window.top,
            collapsed: (window.collapsed_width, COLLAPSED_H),
            expanded: (window.expanded_width, EXPANDED_H),
            notch: NOTCH,
        }
    }

    /// (rộng, cao) của vùng nhận chuột khi island là notch
    pub fn notch_hit(&self) -> (f32, f32) {
        (
            self.notch.0 + 2.0 * NOTCH_HIT_PAD.0,
            self.notch.1 + NOTCH_HIT_PAD.1,
        )
    }

    /// Mép trái của island rộng `w` khi căn giữa cửa sổ
    pub fn island_left(&self, w: f32) -> f32 {
        (self.win_w as f32 - w) / 2.0
    }

    /// Bán kính bo góc của island cao `h`: thấp thì tròn hẳn hai đầu như viên thuốc
    pub fn corner_radius(h: f32) -> f32 {
        (h / 2.0).min(36.0)
    }

    /// Kích thước khi đang hiện thông báo tạm thời: rộng như lúc mở rộng, thấp hơn
    pub fn alert_size(&self) -> (f32, f32) {
        (self.expanded.0, ALERT_H)
    }

    /// Kích thước khi mở rộng; `lyrics_panel` = trang Media có khung lời bài hát
    pub fn expanded_size(&self, lyrics_panel: bool) -> (f32, f32) {
        if lyrics_panel {
            (self.expanded.0, LYRICS_H)
        } else {
            self.expanded
        }
    }

    /// Hình chữ nhật của nút thứ `i` trên island cao `h` (hàng nút neo ở đáy).
    /// 0 = lùi, 1 = phát/dừng, 2 = tới, `LYRICS_BUTTON` = bật/tắt lời
    pub fn button_rect(&self, i: usize, h: f32) -> D2D_RECT_F {
        let top = h - BTN_FROM_BOTTOM;
        let (cx, w) = if i == LYRICS_BUTTON {
            let right = self.island_left(self.expanded.0) + self.expanded.0;
            (
                right - LYRICS_BTN_FROM_RIGHT - LYRICS_BTN_W / 2.0,
                LYRICS_BTN_W,
            )
        } else {
            (self.win_w as f32 / 2.0 + (i as f32 - 1.0) * BTN_GAP, BTN_W)
        };
        D2D_RECT_F {
            left: cx - w / 2.0,
            top,
            right: cx + w / 2.0,
            bottom: top + BTN_H,
        }
    }

    /// Nút nằm dưới điểm (x, y) trên island cao `h`, xét `count` nút đầu tiên
    pub fn button_at(&self, x: f32, y: f32, h: f32, count: usize) -> Option<usize> {
        (0..count).find(|&i| {
            let r = self.button_rect(i, h);
            x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
        })
    }

    /// Điểm (x, y) có nằm trong island rộng `w`, cao `h` không?
    pub fn island_contains(&self, w: f32, h: f32, x: f32, y: f32) -> bool {
        let left = self.island_left(w);
        x >= left && x <= left + w && y >= 0.0 && y <= h
    }

    /// Tâm (x, y) của hàng chấm phân trang trên island cao `h`
    pub fn dots_center(&self, h: f32) -> (f32, f32) {
        (self.win_w as f32 / 2.0, h - DOTS_FROM_BOTTOM)
    }

    /// Điểm (x, y) có trúng hàng chấm phân trang không?
    pub fn dots_hit(&self, h: f32, x: f32, y: f32) -> bool {
        let (cx, _) = self.dots_center(h);
        (x - cx).abs() <= DOTS_HIT_HALF_W && y >= h - DOTS_HIT_H && y <= h
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
    fn default_config_window_size() {
        let l = layout();
        assert_eq!((l.win_w, l.win_h), (460, 230));
        assert_eq!(l.button_rect(1, 116.0).top, 70.0);
        assert_eq!(l.expanded_size(false), (400.0, 116.0));
        assert_eq!(l.expanded_size(true), (400.0, 200.0));
    }

    #[test]
    fn notch_is_smaller_than_the_narrowest_island() {
        let l = Layout::new(&WindowConfig {
            collapsed_width: 180.0, // nhỏ nhất mà cấu hình cho phép
            ..WindowConfig::default()
        });
        let (hit_w, hit_h) = l.notch_hit();
        assert!(l.notch.0 < hit_w && hit_w < l.collapsed.0);
        assert!(l.notch.1 < hit_h && hit_h < l.collapsed.1);
    }

    #[test]
    fn dots_sit_below_buttons() {
        let l = layout();
        for lyrics in [false, true] {
            let (_, h) = l.expanded_size(lyrics);
            let (_, dots_y) = l.dots_center(h);
            assert!(dots_y - 3.0 > l.button_rect(1, h).bottom, "chấm đè lên nút");
        }
    }

    #[test]
    fn dots_hit_only_near_bottom_center() {
        let l = layout();
        let h = l.collapsed.1;
        let (cx, cy) = l.dots_center(h);
        assert!(l.dots_hit(h, cx, cy));
        assert!(!l.dots_hit(h, cx, 5.0)); // phía trên
        assert!(!l.dots_hit(h, cx + 60.0, cy)); // lệch sang bên
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
        for i in 0..=LYRICS_BUTTON {
            let (x, y) = center(l.button_rect(i, 116.0));
            assert_eq!(l.button_at(x, y, 116.0, LYRICS_BUTTON + 1), Some(i));
        }
    }

    #[test]
    fn button_at_returns_none_between_and_outside_buttons() {
        let l = layout();
        let h = 116.0;
        let (x0, y) = center(l.button_rect(0, h));
        let (x1, _) = center(l.button_rect(1, h));
        assert_eq!(l.button_at((x0 + x1) / 2.0, y, h, 3), None); // khe giữa hai nút
        assert_eq!(l.button_at(x1, 0.0, h, 3), None); // phía trên hàng nút
        // Tắt lời bài hát thì nút thứ tư không bấm được
        let (xl, yl) = center(l.button_rect(LYRICS_BUTTON, h));
        assert_eq!(l.button_at(xl, yl, h, 3), None);
    }

    #[test]
    fn buttons_fit_inside_expanded_island() {
        // Kiểm tra cả island hẹp nhất cho phép
        for width in [300.0, 400.0, 700.0] {
            let l = Layout::new(&WindowConfig {
                expanded_width: width,
                ..WindowConfig::default()
            });
            for lyrics in [false, true] {
                let (w, h) = l.expanded_size(lyrics);
                for i in 0..=LYRICS_BUTTON {
                    let r = l.button_rect(i, h);
                    assert!(l.island_contains(w, h, r.left, r.top));
                    assert!(l.island_contains(w, h, r.right, r.bottom));
                }
                // Nút lời không đè lên nút "tới"
                let next = l.button_rect(2, h);
                assert!(l.button_rect(LYRICS_BUTTON, h).left > next.right);
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
