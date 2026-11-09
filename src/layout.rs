//! Kích thước cửa sổ, kích thước island và vị trí các nút.
//! Chỉ có tính toán thuần (không gọi Win32), nên dễ viết test.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;

/// Kích thước cửa sổ trong suốt chứa island (lớn hơn island khi mở rộng)
pub const WIN_W: i32 = 460;
pub const WIN_H: i32 = 130;
/// Khoảng cách từ mép trên màn hình
pub const WIN_TOP: i32 = 10;

/// (rộng, cao) của island khi thu gọn và khi mở rộng
pub const COLLAPSED: (f32, f32) = (160.0, 36.0);
pub const EXPANDED: (f32, f32) = (400.0, 100.0);

const BTN_W: f32 = 44.0;
const BTN_H: f32 = 26.0;
const BTN_TOP: f32 = 70.0;
/// Khoảng cách giữa tâm hai nút liền nhau
const BTN_GAP: f32 = 64.0;
pub const BUTTON_COUNT: usize = 3;

/// Hình chữ nhật của nút thứ `i` (0 = lùi, 1 = phát/dừng, 2 = tới)
pub fn button_rect(i: usize) -> D2D_RECT_F {
    let cx = WIN_W as f32 / 2.0 + (i as f32 - 1.0) * BTN_GAP;
    D2D_RECT_F {
        left: cx - BTN_W / 2.0,
        top: BTN_TOP,
        right: cx + BTN_W / 2.0,
        bottom: BTN_TOP + BTN_H,
    }
}

/// Nút nằm dưới điểm (x, y), nếu có
pub fn button_at(x: f32, y: f32) -> Option<usize> {
    (0..BUTTON_COUNT).find(|&i| {
        let r = button_rect(i);
        x >= r.left && x <= r.right && y >= r.top && y <= r.bottom
    })
}

/// Điểm (x, y) có nằm trong island rộng `w`, cao `h` (căn giữa cửa sổ) không?
pub fn island_contains(w: f32, h: f32, x: f32, y: f32) -> bool {
    let left = (WIN_W as f32 - w) / 2.0;
    x >= left && x <= left + w && y >= 0.0 && y <= h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn center(r: D2D_RECT_F) -> (f32, f32) {
        ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
    }

    #[test]
    fn button_at_finds_each_button_at_its_center() {
        for i in 0..BUTTON_COUNT {
            let (x, y) = center(button_rect(i));
            assert_eq!(button_at(x, y), Some(i));
        }
    }

    #[test]
    fn button_at_returns_none_between_and_outside_buttons() {
        let (x0, y) = center(button_rect(0));
        let (x1, _) = center(button_rect(1));
        assert_eq!(button_at((x0 + x1) / 2.0, y), None); // khe giữa hai nút
        assert_eq!(button_at(x1, 0.0), None); // phía trên hàng nút
    }

    #[test]
    fn buttons_fit_inside_expanded_island() {
        for i in 0..BUTTON_COUNT {
            let r = button_rect(i);
            assert!(island_contains(EXPANDED.0, EXPANDED.1, r.left, r.top));
            assert!(island_contains(EXPANDED.0, EXPANDED.1, r.right, r.bottom));
        }
    }

    #[test]
    fn island_contains_checks_centered_pill() {
        let (w, h) = COLLAPSED;
        let mid = WIN_W as f32 / 2.0;
        assert!(island_contains(w, h, mid, h / 2.0));
        assert!(!island_contains(w, h, 5.0, h / 2.0)); // lệch trái
        assert!(!island_contains(w, h, mid, h + 1.0)); // dưới island
    }
}
