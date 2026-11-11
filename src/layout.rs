//! Kích thước cửa sổ, kích thước island và vị trí các nút, tính từ cấu hình.
//! Chỉ có tính toán thuần (không gọi Win32), nên dễ viết test.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

use crate::config::WindowConfig;

// Chiều cao cố định: vị trí chữ và nút trong graphics.rs được căn theo các số này
const COLLAPSED_H: f32 = 44.0;
/// Mở rộng thường (trang Idle, trang Media khi không có nhạc)
const EXPANDED_H: f32 = 116.0;
/// Trang Media có bài đang phát: ảnh bìa + tên bài, thanh tiến độ, hàng nút tròn
const MEDIA_H: f32 = 160.0;
/// Trang Media có khung lời bài hát: thêm 3 dòng lời
const LYRICS_H: f32 = 248.0;
/// Trang Media Player: ảnh bìa lớn, tên bài, sóng nhạc, thanh tiến độ, hàng nút
const PLAYER_H: f32 = 208.0;
/// Thông báo tạm thời (vừa sao chép, thông báo Windows): đủ cao cho biểu tượng và 3 dòng chữ
const ALERT_H: f32 = 84.0;
/// Island khi tự ẩn: chỉ còn một vạch nhỏ (notch)
const NOTCH: (f32, f32) = (72.0, 8.0);
/// Vùng nhận chuột quanh notch rộng hơn hình vẽ (mỗi bên, và phía dưới) để dễ rê trúng
const NOTCH_HIT_PAD: (f32, f32) = (16.0, 8.0);
// Cửa sổ trong suốt rộng/cao hơn island mở rộng một chút
const MARGIN_X: f32 = 60.0;
const MARGIN_Y: f32 = 30.0;

/// Hàng nút tròn: tâm hàng cách đáy island bao nhiêu
const BTN_ROW_FROM_BOTTOM: f32 = 38.0;
/// Đường kính nút phát/dừng (to hơn) và các nút còn lại
const PLAY_D: f32 = 46.0;
const BTN_D: f32 = 36.0;
/// Khoảng cách giữa tâm hai nút nhạc liền nhau
const BTN_GAP: f32 = 76.0;
/// Ba nút điều khiển nhạc: lùi, phát/dừng, tới
pub const BUTTON_COUNT: usize = 3;
/// Nút thứ tư (bật/tắt lời bài hát) nằm gần mép phải của island
pub const LYRICS_BUTTON: usize = 3;
/// Tâm nút lời cách mép phải bao nhiêu (chừa chỗ cho hàng chấm dọc ở mép)
const LYRICS_BTN_FROM_RIGHT: f32 = 44.0;

/// Trang Media Player: lề hai bên, ảnh bìa (cạnh, mép trên), tâm dọc thanh tiến độ
pub const PLAYER_PAD: f32 = 26.0;
pub const PLAYER_COVER: f32 = 80.0;
pub const PLAYER_COVER_TOP: f32 = 20.0;
const PLAYER_PROGRESS_CY: f32 = 128.0;
/// Thanh tiến độ: chừa chỗ bên trái cho giờ đã phát, bên phải cho giờ còn lại và hàng chấm
const PLAYER_BAR_FROM_LEFT: f32 = PLAYER_PAD + 46.0;
const PLAYER_BAR_FROM_RIGHT: f32 = 80.0;
/// Bấm cách thanh tiến độ chừng này (theo chiều dọc) vẫn tính là bấm trúng để tua
const SEEK_HIT_HALF_H: f32 = 10.0;

/// Chấm phân trang khi thu gọn: nằm ngang, tâm cách đáy bao nhiêu
const DOTS_FROM_BOTTOM: f32 = 8.0;
/// Khi mở rộng: xếp dọc sát mép phải, tâm cách mép bao nhiêu
const DOTS_FROM_RIGHT: f32 = 13.0;
/// Vùng bấm quanh hàng chấm (rộng hơn chấm để dễ bấm)
const DOTS_HIT_HALF_LONG: f32 = 24.0;
const DOTS_HIT_HALF_SHORT: f32 = 9.0;

/// Kiểu mở rộng, mỗi kiểu một chiều cao
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expanded {
    /// Trang Idle, hoặc trang Media chưa có nhạc
    Plain,
    /// Trang Media có bài đang phát, không có khung lời
    Media,
    /// Trang Media có khung lời bài hát
    Lyrics,
    /// Trang Media Player có bài đang phát
    Player,
}

/// `Copy` vì chỉ chứa vài con số: truyền đi đâu cũng sao chép, không cần mượn
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Layout {
    /// Kích thước cửa sổ trong suốt chứa island
    pub win_w: i32,
    pub win_h: i32,
    /// Khoảng cách từ mép trên màn hình
    pub top: i32,
    /// (rộng, cao) của island khi thu gọn và khi mở rộng thường
    pub collapsed: (f32, f32),
    pub expanded: (f32, f32),
    /// (rộng, cao) khi tự ẩn thành notch
    pub notch: (f32, f32),
}

impl Layout {
    pub fn new(window: &WindowConfig) -> Self {
        Self {
            win_w: (window.expanded_width + MARGIN_X).ceil() as i32,
            win_h: (EXPANDED_H.max(MEDIA_H).max(LYRICS_H).max(PLAYER_H) + MARGIN_Y) as i32,
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

    /// Mép trái và phải của island khi mở rộng hẳn
    pub fn expanded_edges(&self) -> (f32, f32) {
        let left = self.island_left(self.expanded.0);
        (left, left + self.expanded.0)
    }

    /// Bán kính bo góc của island cao `h`: thấp thì tròn hẳn hai đầu như viên thuốc
    pub fn corner_radius(h: f32) -> f32 {
        (h / 2.0).min(36.0)
    }

    /// Kích thước khi đang hiện thông báo tạm thời: rộng như lúc mở rộng, thấp hơn
    pub fn alert_size(&self) -> (f32, f32) {
        (self.expanded.0, ALERT_H)
    }

    /// Kích thước khi mở rộng theo kiểu `kind`
    pub fn expanded_size(&self, kind: Expanded) -> (f32, f32) {
        let h = match kind {
            Expanded::Plain => EXPANDED_H,
            Expanded::Media => MEDIA_H,
            Expanded::Lyrics => LYRICS_H,
            Expanded::Player => PLAYER_H,
        };
        (self.expanded.0, h)
    }

    /// Khung vuông của nút tròn thứ `i` trên island cao `h` (hàng nút neo ở đáy).
    /// 0 = lùi, 1 = phát/dừng, 2 = tới, `LYRICS_BUTTON` = bật/tắt lời
    pub fn button_rect(&self, i: usize, h: f32) -> D2D_RECT_F {
        let cy = h - BTN_ROW_FROM_BOTTOM;
        let (cx, d) = if i == LYRICS_BUTTON {
            (self.expanded_edges().1 - LYRICS_BTN_FROM_RIGHT, BTN_D)
        } else {
            let d = if i == 1 { PLAY_D } else { BTN_D };
            (self.win_w as f32 / 2.0 + (i as f32 - 1.0) * BTN_GAP, d)
        };
        D2D_RECT_F {
            left: cx - d / 2.0,
            top: cy - d / 2.0,
            right: cx + d / 2.0,
            bottom: cy + d / 2.0,
        }
    }

    /// Nút nằm dưới điểm (x, y) trên island cao `h`, xét `count` nút đầu tiên
    pub fn button_at(&self, x: f32, y: f32, h: f32, count: usize) -> Option<usize> {
        (0..count).find(|&i| {
            let r = self.button_rect(i, h);
            x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
        })
    }

    /// Thanh tiến độ của trang Media Player: (mép trái, mép phải, tâm dọc)
    pub fn player_bar(&self) -> (f32, f32, f32) {
        let (el, right) = self.expanded_edges();
        (
            el + PLAYER_BAR_FROM_LEFT,
            right - PLAYER_BAR_FROM_RIGHT,
            PLAYER_PROGRESS_CY,
        )
    }

    /// Bấm tại (x, y) trên trang Media Player: trúng thanh tiến độ thì trả về vị trí cần tua
    /// tới (0 = đầu bài, 1 = cuối bài)
    pub fn seek_at(&self, x: f32, y: f32) -> Option<f32> {
        let (x0, x1, cy) = self.player_bar();
        let near = (y - cy).abs() <= SEEK_HIT_HALF_H && x >= x0 - 6.0 && x <= x1 + 6.0;
        near.then(|| ((x - x0) / (x1 - x0)).clamp(0.0, 1.0))
    }

    /// Điểm (x, y) có nằm trong island rộng `w`, cao `h` không?
    pub fn island_contains(&self, w: f32, h: f32, x: f32, y: f32) -> bool {
        let left = self.island_left(w);
        x >= left && x <= left + w && y >= 0.0 && y <= h
    }

    /// Mở rộng thì hàng chấm xếp dọc ở mép phải, thu gọn thì nằm ngang ở đáy
    pub fn dots_vertical(h: f32) -> bool {
        h > COLLAPSED_H + 1.0
    }

    /// Tâm hàng chấm nằm ngang ở đáy island cao `h`
    pub fn dots_row_center(&self, h: f32) -> (f32, f32) {
        (self.win_w as f32 / 2.0, h - DOTS_FROM_BOTTOM)
    }

    /// Tâm hàng chấm dọc ở mép phải island rộng `w`, cao `h`
    pub fn dots_column_center(&self, w: f32, h: f32) -> (f32, f32) {
        (self.island_left(w) + w - DOTS_FROM_RIGHT, h / 2.0)
    }

    /// Tâm (x, y) của hàng chấm phân trang trên island rộng `w`, cao `h`
    pub fn dots_center(&self, w: f32, h: f32) -> (f32, f32) {
        if Self::dots_vertical(h) {
            self.dots_column_center(w, h)
        } else {
            self.dots_row_center(h)
        }
    }

    /// Điểm (x, y) có trúng hàng chấm phân trang không?
    pub fn dots_hit(&self, w: f32, h: f32, x: f32, y: f32) -> bool {
        let (cx, cy) = self.dots_center(w, h);
        let (dx, dy) = ((x - cx).abs(), (y - cy).abs());
        if Self::dots_vertical(h) {
            dx <= DOTS_HIT_HALF_SHORT && dy <= DOTS_HIT_HALF_LONG
        } else {
            dx <= DOTS_HIT_HALF_LONG && dy <= DOTS_HIT_HALF_SHORT
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KINDS: [Expanded; 4] = [
        Expanded::Plain,
        Expanded::Media,
        Expanded::Lyrics,
        Expanded::Player,
    ];

    fn layout() -> Layout {
        Layout::new(&WindowConfig::default())
    }

    fn center(r: D2D_RECT_F) -> (f32, f32) {
        ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
    }

    #[test]
    fn default_config_window_size() {
        let l = layout();
        assert_eq!((l.win_w, l.win_h), (460, 278));
        assert_eq!(l.expanded_size(Expanded::Plain), (400.0, 116.0));
        assert_eq!(l.expanded_size(Expanded::Media), (400.0, 160.0));
        assert_eq!(l.expanded_size(Expanded::Lyrics), (400.0, 248.0));
        // Nút phát/dừng to hơn hai nút bên cạnh, cùng một hàng
        let (play, prev) = (l.button_rect(1, 160.0), l.button_rect(0, 160.0));
        assert!(play.right - play.left > prev.right - prev.left);
        assert_eq!(center(play).1, center(prev).1);
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
    fn dots_lie_flat_when_collapsed_and_stand_up_when_expanded() {
        let l = layout();
        let (w, h) = l.collapsed;
        let (cx, cy) = l.dots_center(w, h);
        assert_eq!((cx, cy), (230.0, 36.0));
        assert!(l.dots_hit(w, h, cx, cy));
        assert!(!l.dots_hit(w, h, cx, 5.0)); // phía trên
        assert!(!l.dots_hit(w, h, cx + 60.0, cy)); // lệch sang bên

        let (w, h) = l.expanded_size(Expanded::Lyrics);
        let (cx, cy) = l.dots_center(w, h);
        assert_eq!((cx, cy), (l.expanded_edges().1 - DOTS_FROM_RIGHT, h / 2.0));
        assert!(l.dots_hit(w, h, cx, cy + 15.0)); // dọc theo hàng chấm
        assert!(!l.dots_hit(w, h, cx - 30.0, cy)); // lệch vào trong
    }

    #[test]
    fn vertical_dots_stay_clear_of_buttons() {
        let l = layout();
        for kind in KINDS {
            let (w, h) = l.expanded_size(kind);
            let (dx, _) = l.dots_center(w, h);
            let lyrics = l.button_rect(LYRICS_BUTTON, h);
            assert!(dx - 4.0 > lyrics.right, "chấm đè lên nút lời");
        }
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
            let (x, y) = center(l.button_rect(i, 248.0));
            assert_eq!(l.button_at(x, y, 248.0, LYRICS_BUTTON + 1), Some(i));
        }
    }

    #[test]
    fn button_at_returns_none_between_and_outside_buttons() {
        let l = layout();
        let h = 160.0;
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
        for width in [320.0, 400.0, 700.0] {
            let l = Layout::new(&WindowConfig {
                expanded_width: width,
                ..WindowConfig::default()
            });
            for kind in [Expanded::Media, Expanded::Lyrics, Expanded::Player] {
                let (w, h) = l.expanded_size(kind);
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
    fn player_page_fits_cover_bar_and_buttons() {
        for width in [320.0, 400.0, 700.0] {
            let l = Layout::new(&WindowConfig {
                expanded_width: width,
                ..WindowConfig::default()
            });
            let (w, h) = l.expanded_size(Expanded::Player);
            assert!(h <= l.win_h as f32);
            let (x0, x1, cy) = l.player_bar();
            assert!(x1 - x0 > 100.0, "thanh tiến độ quá ngắn");
            // Thanh tiến độ nằm dưới ảnh bìa và trên hàng nút
            assert!(cy - 8.0 > PLAYER_COVER_TOP + PLAYER_COVER);
            assert!(cy + 8.0 < l.button_rect(1, h).top);
            // Chừa chỗ cho hàng chấm dọc ở mép phải
            let (dots_x, _) = l.dots_center(w, h);
            assert!(x1 + 40.0 < dots_x);
        }
    }

    #[test]
    fn seek_at_maps_bar_to_fraction() {
        let l = layout();
        let (x0, x1, cy) = l.player_bar();
        assert_eq!(l.seek_at(x0, cy), Some(0.0));
        assert_eq!(l.seek_at(x1, cy + 5.0), Some(1.0));
        assert_eq!(l.seek_at((x0 + x1) / 2.0, cy), Some(0.5));
        assert_eq!(l.seek_at(x0 - 3.0, cy), Some(0.0)); // hơi lệch mép vẫn tính
        assert_eq!(l.seek_at((x0 + x1) / 2.0, cy + 30.0), None);
        assert_eq!(l.seek_at(x1 + 30.0, cy), None);
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
