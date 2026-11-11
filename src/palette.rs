//! Lấy màu nhấn từ ảnh bìa album (Accent Wash của trang Media Player).
//!
//! Cách làm: đếm các điểm ảnh có màu rõ (đủ đậm, đủ sáng) theo 24 dải màu, chọn dải
//! chiếm nhiều nhất, lấy màu trung bình của dải đó rồi chỉnh độ sáng, độ đậm cho dễ
//! đọc trên nền tối. Ảnh gần như trắng đen thì không có màu nhấn (dùng màu trong Cài đặt).

use crate::config::Color;
use crate::icon::Icon;

/// Số dải màu (mỗi dải 15°)
const HUE_BUCKETS: usize = 24;
/// Điểm ảnh nhạt hơn / tối hơn mức này không tính là "có màu"
const MIN_SATURATION: f32 = 0.22;
const MIN_VALUE: f32 = 0.18;
/// Phần ảnh tối thiểu phải có màu rõ, không thì coi như ảnh trắng đen
const MIN_COLORFUL_SHARE: f32 = 0.04;
/// Màu nhấn được đưa về khoảng này (HSL) để nổi trên nền tối mà không chói
const SATURATION_RANGE: (f32, f32) = (0.45, 0.85);
const LIGHTNESS_RANGE: (f32, f32) = (0.58, 0.70);

/// (sắc độ 0–360, độ đậm 0–1, độ sáng 0–1) theo HSV
fn hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

/// (sắc độ, độ đậm, độ sáng) theo HSL
fn to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let (hue, _, _) = hsv(r, g, b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let s = if max - min <= f32::EPSILON {
        0.0
    } else {
        (max - min) / (1.0 - (2.0 * l - 1.0).abs())
    };
    (hue, s, l)
}

fn from_hsl(hue: f32, s: f32, l: f32) -> Color {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (hue / 60.0) as u32 % 6 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    Color::rgb(byte(r), byte(g), byte(b))
}

/// Màu nhấn của ảnh bìa, hoặc None nếu ảnh gần như không có màu
pub fn accent_from(icon: &Icon) -> Option<Color> {
    // Tổng (trọng số, r, g, b) của từng dải màu
    let mut buckets = [(0.0f32, 0.0f32, 0.0f32, 0.0f32); HUE_BUCKETS];
    let mut total = 0usize;
    let mut colorful = 0usize;
    for px in icon.pixels.as_chunks::<4>().0 {
        let a = f32::from(px[3]) / 255.0;
        if a < 0.5 {
            continue;
        }
        total += 1;
        // BGRA đã nhân độ đục: chia lại cho độ đục để ra màu thật
        let (b, g, r) = (
            f32::from(px[0]) / 255.0 / a,
            f32::from(px[1]) / 255.0 / a,
            f32::from(px[2]) / 255.0 / a,
        );
        let (hue, s, v) = hsv(r, g, b);
        if s < MIN_SATURATION || v < MIN_VALUE {
            continue;
        }
        colorful += 1;
        // Màu càng đậm, càng sáng càng được tính nhiều (ưu tiên màu nổi bật)
        let weight = s * v;
        let bucket = &mut buckets[(hue / (360.0 / HUE_BUCKETS as f32)) as usize % HUE_BUCKETS];
        bucket.0 += weight;
        bucket.1 += r * weight;
        bucket.2 += g * weight;
        bucket.3 += b * weight;
    }
    if total == 0 || (colorful as f32) < total as f32 * MIN_COLORFUL_SHARE {
        return None;
    }
    // Dải mạnh nhất, cộng thêm hai dải kề bên (màu nằm giữa hai dải không bị chia đôi)
    let around = |i: usize| {
        let prev = buckets[(i + HUE_BUCKETS - 1) % HUE_BUCKETS];
        let next = buckets[(i + 1) % HUE_BUCKETS];
        let cur = buckets[i];
        (
            cur.0 + prev.0 + next.0,
            cur.1 + prev.1 + next.1,
            cur.2 + prev.2 + next.2,
            cur.3 + prev.3 + next.3,
        )
    };
    let best = (0..HUE_BUCKETS)
        .map(around)
        .max_by(|a, b| a.0.total_cmp(&b.0))?;
    if best.0 <= 0.0 {
        return None;
    }
    let (r, g, b) = (best.1 / best.0, best.2 / best.0, best.3 / best.0);
    let (hue, s, l) = to_hsl(r, g, b);
    Some(from_hsl(
        hue,
        s.clamp(SATURATION_RANGE.0, SATURATION_RANGE.1),
        l.clamp(LIGHTNESS_RANGE.0, LIGHTNESS_RANGE.1),
    ))
}

/// Pha `color` với trắng theo tỉ lệ `amount` (0 = giữ nguyên, 1 = trắng): đầu sáng của dải màu
pub fn lighten(color: Color, amount: f32) -> Color {
    let mix = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * amount).round() as u8;
    Color::rgba(mix(color.r), mix(color.g), mix(color.b), color.a)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ảnh vuông 8×8 tô theo hàm `f(x, y) -> (r, g, b)`
    fn image(f: impl Fn(u32, u32) -> (u8, u8, u8)) -> Icon {
        let mut pixels = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                let (r, g, b) = f(x, y);
                pixels.extend([b, g, r, 255]);
            }
        }
        Icon {
            width: 8,
            height: 8,
            pixels,
        }
    }

    fn hue_of(c: Color) -> f32 {
        hsv(
            f32::from(c.r) / 255.0,
            f32::from(c.g) / 255.0,
            f32::from(c.b) / 255.0,
        )
        .0
    }

    #[test]
    fn orange_cover_gives_orange_accent() {
        // Giống ảnh mẫu: cam đào, có vùng tối ở giữa
        let cover = image(|x, y| {
            if (2..6).contains(&x) && (2..6).contains(&y) {
                (60, 30, 20)
            } else {
                (240, 140, 90)
            }
        });
        let accent = accent_from(&cover).expect("ảnh có màu");
        let hue = hue_of(accent);
        assert!((10.0..35.0).contains(&hue), "sắc độ {hue}");
        let (_, _, l) = to_hsl(
            f32::from(accent.r) / 255.0,
            f32::from(accent.g) / 255.0,
            f32::from(accent.b) / 255.0,
        );
        assert!((0.57..0.71).contains(&l), "độ sáng {l}");
    }

    #[test]
    fn most_common_color_wins_over_small_spot() {
        // Phần lớn xanh dương, một chấm đỏ nhỏ
        let cover = image(|x, y| {
            if x == 0 && y == 0 {
                (230, 20, 20)
            } else {
                (30, 90, 220)
            }
        });
        let hue = hue_of(accent_from(&cover).unwrap());
        assert!((200.0..240.0).contains(&hue), "sắc độ {hue}");
    }

    #[test]
    fn grayscale_cover_has_no_accent() {
        let gray = image(|x, y| {
            let v = ((x + y) * 16) as u8;
            (v, v, v)
        });
        assert_eq!(accent_from(&gray), None);
        let black = image(|_, _| (5, 5, 8));
        assert_eq!(accent_from(&black), None);
    }

    #[test]
    fn dark_but_colorful_cover_is_brightened() {
        let navy = image(|_, _| (20, 30, 90));
        let accent = accent_from(&navy).unwrap();
        // Đủ sáng để đọc được trên nền tối
        assert!(accent.r.max(accent.g).max(accent.b) > 150, "{accent:?}");
    }

    #[test]
    fn lighten_mixes_toward_white() {
        let c = Color::rgb(200, 100, 0);
        assert_eq!(lighten(c, 0.0), c);
        assert_eq!(lighten(c, 1.0), Color::rgb(255, 255, 255));
        assert_eq!(lighten(c, 0.5), Color::rgb(228, 178, 128));
    }

    #[test]
    fn hsl_round_trip() {
        for (r, g, b) in [(240u8, 140u8, 90u8), (30, 90, 220), (10, 200, 60)] {
            let (h, s, l) = to_hsl(
                f32::from(r) / 255.0,
                f32::from(g) / 255.0,
                f32::from(b) / 255.0,
            );
            let back = from_hsl(h, s, l);
            assert!(back.r.abs_diff(r) <= 1 && back.g.abs_diff(g) <= 1 && back.b.abs_diff(b) <= 1);
        }
    }
}
