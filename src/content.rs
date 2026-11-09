//! Chuỗi hiển thị trên island: đồng hồ, ngày tháng, bài đang phát.

use windows::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};

use crate::media::{self, MediaInfo};

const WEEKDAYS: [&str; 7] = [
    "Chủ nhật",
    "Thứ hai",
    "Thứ ba",
    "Thứ tư",
    "Thứ năm",
    "Thứ sáu",
    "Thứ bảy",
];

#[derive(Clone, PartialEq, Debug)]
pub struct Content {
    /// Dòng ngắn khi thu gọn: giờ hoặc tên bài
    pub short: String,
    /// Giờ:phút:giây khi mở rộng
    pub long: String,
    pub date: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

/// Cắt chuỗi còn tối đa `max` ký tự (không phải byte) và thêm "…"
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// Nội dung theo giờ hệ thống và bài đang phát ngay lúc này
pub fn current() -> Content {
    let now = unsafe { GetLocalTime() };
    build(&now, &media::snapshot())
}

// Tách riêng phần không gọi Win32 để test được với thời gian giả
fn build(t: &SYSTEMTIME, media: &MediaInfo) -> Content {
    let short = if media.playing && !media.title.is_empty() {
        format!("♪ {}", truncate(&media.title, 14))
    } else {
        format!("{:02}:{:02}", t.wHour, t.wMinute)
    };

    Content {
        short,
        long: format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond),
        date: format!(
            "{}, {:02}/{:02}/{}",
            WEEKDAYS[t.wDayOfWeek as usize], t.wDay, t.wMonth, t.wYear
        ),
        title: truncate(&media.title, 28),
        artist: truncate(&media.artist, 30),
        playing: media.playing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn time() -> SYSTEMTIME {
        SYSTEMTIME {
            wYear: 2026,
            wMonth: 11,
            wDayOfWeek: 1,
            wDay: 9,
            wHour: 9,
            wMinute: 5,
            wSecond: 7,
            wMilliseconds: 0,
        }
    }

    fn song(playing: bool) -> MediaInfo {
        MediaInfo {
            title: "Nơi này có anh".into(),
            artist: "Sơn Tùng M-TP".into(),
            playing,
        }
    }

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("abc", 3), "abc");
        assert_eq!(truncate("", 5), "");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        // "Thứ" có 3 ký tự nhưng 5 byte UTF-8
        assert_eq!(truncate("Thứ hai", 3), "Thứ…");
    }

    #[test]
    fn no_media_shows_clock_and_date() {
        let c = build(&time(), &MediaInfo::default());
        assert_eq!(c.short, "09:05");
        assert_eq!(c.long, "09:05:07");
        assert_eq!(c.date, "Thứ hai, 09/11/2026");
        assert!(c.title.is_empty());
    }

    #[test]
    fn playing_song_replaces_clock() {
        let c = build(&time(), &song(true));
        assert_eq!(c.short, "♪ Nơi này có anh");
        assert!(c.playing);
    }

    #[test]
    fn paused_song_keeps_clock_but_shows_title() {
        let c = build(&time(), &song(false));
        assert_eq!(c.short, "09:05");
        assert_eq!(c.title, "Nơi này có anh");
    }
}
