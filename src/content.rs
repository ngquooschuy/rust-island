//! Nội dung hiển thị trên island (đồng hồ, ngày, thời tiết, pin, bài đang phát, lời bài hát)
//! và các trang mà người dùng lướt qua bằng con lăn chuột.

use windows::Win32::{Foundation::SYSTEMTIME, System::SystemInformation::GetLocalTime};

use crate::battery::{self, Battery};
use crate::config::ClockConfig;
use crate::lyrics::{self, Status};
use crate::media::{self, MediaInfo};
use crate::weather::{self, Weather};

const WEEKDAYS: [&str; 7] = [
    "Chủ nhật",
    "Thứ hai",
    "Thứ ba",
    "Thứ tư",
    "Thứ năm",
    "Thứ sáu",
    "Thứ bảy",
];
const WEEKDAYS_SHORT: [&str; 7] = ["CN", "T2", "T3", "T4", "T5", "T6", "T7"];

#[derive(Clone, PartialEq, Debug)]
pub struct Content {
    /// Giờ:phút, dùng khi thu gọn
    pub clock: String,
    /// Giờ khi mở rộng (có giây nếu cấu hình bật)
    pub long: String,
    pub date: String,
    /// Thứ và ngày dạng ngắn "T2 9/11", thay chỗ pin trên máy bàn
    pub day_short: String,
    /// "♪ Tên bài" đã cắt ngắn, dùng khi thu gọn
    pub song: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub battery: Option<Battery>,
    /// None khi chưa tải được hoặc đã tắt trong cấu hình
    pub weather: Option<Weather>,
    /// Thanh tiến độ; None nếu trình phát không báo thời lượng
    pub progress: Option<Progress>,
    pub lyrics: LyricsView,
}

/// Vị trí phát hiện tại và thời lượng bài (mili giây)
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Progress {
    pub elapsed_ms: u64,
    pub duration_ms: u64,
}

impl Progress {
    /// Tỉ lệ đã phát, từ 0 đến 1
    pub fn fraction(&self) -> f32 {
        if self.duration_ms == 0 {
            0.0
        } else {
            (self.elapsed_ms as f32 / self.duration_ms as f32).clamp(0.0, 1.0)
        }
    }
}

/// Phần lời bài hát cần vẽ
#[derive(Clone, PartialEq, Debug)]
pub enum LyricsView {
    /// Tắt trong cấu hình, hoặc không có bài nào
    Off,
    /// Chưa có lời để hiện: "Đang tìm…", "Không tìm thấy…"
    Message(&'static str),
    /// Dòng trước, dòng đang hát, dòng sau
    Lines {
        prev: Option<String>,
        current: String,
        next: Option<String>,
    },
}

impl LyricsView {
    /// Dòng đang hát, nếu đang có lời chạy theo nhạc
    pub fn current_line(&self) -> Option<&str> {
        match self {
            Self::Lines { current, .. } => Some(current),
            _ => None,
        }
    }
}

/// "2:05" cho thanh tiến độ
pub fn mmss(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

impl Content {
    /// Hai nội dung có trông giống nhau khi island thu gọn không?
    /// (khi thu gọn không hiện giây, nên không cần vẽ lại mỗi giây)
    pub fn same_when_collapsed(&self, other: &Self) -> bool {
        self.clock == other.clock
            && self.day_short == other.day_short
            && self.song == other.song
            && self.playing == other.playing
            && self.battery == other.battery
            && self.weather == other.weather
            && self.lyrics.current_line() == other.lyrics.current_line()
    }

    /// Trang Media khi mở rộng có hiện khung lời bài hát (island cao hơn) không?
    pub fn shows_lyrics_panel(&self, page: Page, lyrics_on: bool) -> bool {
        page == Page::Media && lyrics_on && !self.title.is_empty()
    }

    /// So với `old`: vừa bắt đầu phát, hoặc vừa đổi sang bài khác?
    pub fn started_playback_since(&self, old: &Self) -> bool {
        self.playing && !self.title.is_empty() && (!old.playing || self.title != old.title)
    }

    /// Dòng phụ khi mở rộng trang Idle:
    /// "Thứ hai, 09/11/2026 · Hà Nội, nhiều mây · Pin 57%"
    pub fn detail_line(&self) -> String {
        let mut parts = vec![self.date.clone()];
        if let Some(w) = &self.weather {
            parts.push(format!("{}, {}", w.place, w.sky.describe().to_lowercase()));
        }
        if let Some(b) = self.battery {
            let label = if b.charging {
                "Đang sạc"
            } else if b.is_low() {
                "Pin yếu"
            } else {
                "Pin"
            };
            parts.push(format!("{label} {}%", b.percent));
        }
        parts.join(" · ")
    }
}

/// Các trang của island; chấm phân trang cho biết đang ở trang nào
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// Đồng hồ + thời tiết
    Idle,
    /// Bài đang phát + nút điều khiển
    Media,
}

impl Page {
    pub const ALL: [Page; 2] = [Page::Idle, Page::Media];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&p| p == self).unwrap()
    }

    /// Trang kế tiếp (`forward`) hoặc trước đó, quay vòng
    pub fn step(self, forward: bool) -> Self {
        let n = Self::ALL.len();
        let i = if forward {
            self.index() + 1
        } else {
            self.index() + n - 1
        };
        Self::ALL[i % n]
    }

    /// Trang lúc mới mở app
    pub fn initial(content: &Content) -> Self {
        if content.playing && !content.title.is_empty() {
            Self::Media
        } else {
            Self::Idle
        }
    }

    /// Tự chuyển trang khi media đổi: bắt đầu phát/đổi bài thì sang Media,
    /// hết bài thì về Idle, còn lại giữ trang người dùng đang chọn
    pub fn after_media_change(self, old: &Content, new: &Content) -> Self {
        if new.started_playback_since(old) {
            Self::Media
        } else if new.title.is_empty() && !old.title.is_empty() {
            Self::Idle
        } else {
            self
        }
    }
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

/// Nội dung theo giờ hệ thống, pin và bài đang phát ngay lúc này
pub fn current(clock: &ClockConfig, lyrics_enabled: bool) -> Content {
    let now = unsafe { GetLocalTime() };
    let media = media::snapshot();
    let position_ms = media::position_at(&media, media::now_ticks());
    let status = lyrics::status_for(&media.title, &media.artist);
    Content {
        weather: weather::snapshot(),
        progress: progress(&media, position_ms),
        lyrics: lyrics_view(lyrics_enabled, &media, status, position_ms),
        ..build(&now, &media, battery::status(), clock)
    }
}

fn progress(media: &MediaInfo, position_ms: u64) -> Option<Progress> {
    (media.duration_ms > 0 && !media.title.is_empty()).then_some(Progress {
        elapsed_ms: position_ms,
        duration_ms: media.duration_ms,
    })
}

/// Chọn lời để hiện cho bài đang phát tại `position_ms`
fn lyrics_view(
    enabled: bool,
    media: &MediaInfo,
    status: Option<Status>,
    position_ms: u64,
) -> LyricsView {
    if !enabled || media.title.is_empty() {
        return LyricsView::Off;
    }
    let message = match status {
        None | Some(Status::Loading) => "Đang tìm lời bài hát…",
        Some(Status::PlainOnly) => "Bài này chưa có lời chạy theo nhạc",
        Some(Status::Instrumental) => "♪ Nhạc không lời",
        Some(Status::NotFound) => "Không tìm thấy lời bài hát",
        Some(Status::Failed) => "Không tải được lời bài hát",
        // Có lời đồng bộ nhưng trình phát không báo vị trí thì không biết hát tới đâu
        Some(Status::Synced(_)) if media.duration_ms == 0 && media.updated_ticks == 0 => {
            "Trình phát nhạc không báo vị trí bài hát"
        }
        Some(Status::Synced(lyrics)) => {
            let w = lyrics.window_at(position_ms);
            return LyricsView::Lines {
                prev: w.prev.map(String::from),
                current: w.current.to_string(),
                next: w.next.map(String::from),
            };
        }
    };
    LyricsView::Message(message)
}

/// "09:05" / "09:05:07" (24 giờ) hoặc "9:05 SA" / "9:05:07 CH" (12 giờ)
fn format_time(t: &SYSTEMTIME, clock: &ClockConfig, seconds: bool) -> String {
    let mut s = if clock.hour24 {
        format!("{:02}:{:02}", t.wHour, t.wMinute)
    } else {
        // 0 giờ và 12 giờ đều hiện là 12
        let hour = match t.wHour % 12 {
            0 => 12,
            h => h,
        };
        format!("{hour}:{:02}", t.wMinute)
    };
    if seconds {
        s.push_str(&format!(":{:02}", t.wSecond));
    }
    if !clock.hour24 {
        s.push_str(if t.wHour < 12 { " SA" } else { " CH" });
    }
    s
}

// Tách riêng phần không gọi Win32 để test được với thời gian giả
fn build(
    t: &SYSTEMTIME,
    media: &MediaInfo,
    battery: Option<Battery>,
    clock: &ClockConfig,
) -> Content {
    let weekday = t.wDayOfWeek as usize;
    Content {
        clock: format_time(t, clock, false),
        long: format_time(t, clock, clock.show_seconds),
        date: format!(
            "{}, {:02}/{:02}/{}",
            WEEKDAYS[weekday], t.wDay, t.wMonth, t.wYear
        ),
        day_short: format!("{} {}/{}", WEEKDAYS_SHORT[weekday], t.wDay, t.wMonth),
        song: format!("♪ {}", truncate(&media.title, 18)),
        title: truncate(&media.title, 28),
        artist: truncate(&media.artist, 30),
        playing: media.playing,
        battery,
        // current() điền vào sau
        weather: None,
        progress: None,
        lyrics: LyricsView::Off,
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
            ..MediaInfo::default()
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

    fn at(hour: u16) -> SYSTEMTIME {
        SYSTEMTIME {
            wHour: hour,
            ..time()
        }
    }

    const H24: ClockConfig = ClockConfig {
        hour24: true,
        show_seconds: true,
    };
    const H12: ClockConfig = ClockConfig {
        hour24: false,
        show_seconds: true,
    };

    fn idle() -> Content {
        build(&time(), &MediaInfo::default(), None, &H24)
    }

    fn with_song(playing: bool) -> Content {
        build(&time(), &song(playing), None, &H24)
    }

    fn synced(lrc: &str) -> Option<Status> {
        let parsed = std::sync::Arc::new(crate::lyrics::parse_lrc(lrc));
        Some(Status::Synced(parsed))
    }

    fn timed_song() -> MediaInfo {
        MediaInfo {
            duration_ms: 146_000,
            updated_ticks: 1,
            ..song(true)
        }
    }

    #[test]
    fn lyrics_view_shows_three_lines_around_position() {
        let status = synced("[00:01.00] one\n[00:05.00] two\n[00:09.00] three");
        let view = lyrics_view(true, &timed_song(), status, 6_000);
        assert_eq!(
            view,
            LyricsView::Lines {
                prev: Some("one".into()),
                current: "two".into(),
                next: Some("three".into()),
            }
        );
        assert_eq!(view.current_line(), Some("two"));
    }

    #[test]
    fn lyrics_view_messages() {
        let media = timed_song();
        assert_eq!(lyrics_view(false, &media, None, 0), LyricsView::Off);
        assert_eq!(
            lyrics_view(true, &media, None, 0),
            LyricsView::Message("Đang tìm lời bài hát…")
        );
        assert_eq!(
            lyrics_view(true, &media, Some(Status::NotFound), 0),
            LyricsView::Message("Không tìm thấy lời bài hát")
        );
        // Có lời nhưng trình phát không báo vị trí
        let no_timeline = song(true);
        assert_eq!(
            lyrics_view(true, &no_timeline, synced("[00:01.00] x"), 0),
            LyricsView::Message("Trình phát nhạc không báo vị trí bài hát")
        );
        assert_eq!(
            lyrics_view(true, &MediaInfo::default(), None, 0),
            LyricsView::Off
        );
    }

    #[test]
    fn progress_needs_a_duration() {
        assert_eq!(progress(&song(true), 5_000), None);
        let p = progress(&timed_song(), 73_000).unwrap();
        assert_eq!(p.fraction(), 0.5);
        assert_eq!(
            (mmss(p.elapsed_ms), mmss(p.duration_ms - p.elapsed_ms)),
            ("1:13".into(), "1:13".into())
        );
    }

    #[test]
    fn mmss_formats_minutes_and_seconds() {
        assert_eq!(mmss(0), "0:00");
        assert_eq!(mmss(29_900), "0:29");
        assert_eq!(mmss(605_000), "10:05");
    }

    #[test]
    fn lyrics_panel_only_on_media_page_with_a_song() {
        let c = with_song(true);
        assert!(c.shows_lyrics_panel(Page::Media, true));
        assert!(!c.shows_lyrics_panel(Page::Media, false));
        assert!(!c.shows_lyrics_panel(Page::Idle, true));
        assert!(!idle().shows_lyrics_panel(Page::Media, true));
    }

    #[test]
    fn no_media_shows_clock_and_date() {
        let c = idle();
        assert_eq!(c.clock, "09:05");
        assert_eq!(c.long, "09:05:07");
        assert_eq!(c.date, "Thứ hai, 09/11/2026");
        assert_eq!(c.day_short, "T2 9/11");
        assert!(c.title.is_empty());
    }

    #[test]
    fn song_line_has_note_and_title() {
        let c = with_song(true);
        assert_eq!(c.song, "♪ Nơi này có anh");
        assert!(c.playing);
    }

    #[test]
    fn battery_is_passed_through() {
        let b = Battery {
            percent: 57,
            charging: false,
        };
        let c = build(&time(), &MediaInfo::default(), Some(b), &H24);
        assert_eq!(c.battery, Some(b));
    }

    #[test]
    fn detail_line_lists_date_weather_and_battery() {
        assert_eq!(idle().detail_line(), "Thứ hai, 09/11/2026");

        let c = Content {
            weather: Some(Weather {
                temperature: 31.0,
                sky: crate::weather::Sky::PartlyCloudy,
                is_day: true,
                place: "Hà Nội".into(),
            }),
            battery: Some(Battery {
                percent: 82,
                charging: true,
            }),
            ..idle()
        };
        assert_eq!(
            c.detail_line(),
            "Thứ hai, 09/11/2026 · Hà Nội, có mây · Đang sạc 82%"
        );
    }

    #[test]
    fn seconds_can_be_hidden() {
        let clock = ClockConfig {
            show_seconds: false,
            ..H24
        };
        let c = build(&time(), &MediaInfo::default(), None, &clock);
        assert_eq!(c.long, "09:05");
    }

    #[test]
    fn seconds_do_not_count_when_collapsed() {
        let mut later = idle();
        later.long = "09:05:08".into();
        assert!(idle().same_when_collapsed(&later));
        later.clock = "09:06".into();
        assert!(!idle().same_when_collapsed(&later));
    }

    #[test]
    fn page_step_wraps_around_both_ways() {
        assert_eq!(Page::Idle.step(true), Page::Media);
        assert_eq!(Page::Media.step(true), Page::Idle);
        assert_eq!(Page::Idle.step(false), Page::Media);
    }

    #[test]
    fn initial_page_follows_playback() {
        assert_eq!(Page::initial(&idle()), Page::Idle);
        assert_eq!(Page::initial(&with_song(false)), Page::Idle);
        assert_eq!(Page::initial(&with_song(true)), Page::Media);
    }

    #[test]
    fn page_switches_to_media_when_playback_starts() {
        let page = Page::Idle.after_media_change(&with_song(false), &with_song(true));
        assert_eq!(page, Page::Media);
    }

    #[test]
    fn started_playback_means_play_or_new_song() {
        let playing = with_song(true);
        assert!(playing.started_playback_since(&idle())); // có bài mới
        assert!(playing.started_playback_since(&with_song(false))); // bấm phát lại
        assert!(!playing.started_playback_since(&playing)); // vẫn bài cũ
        assert!(!with_song(false).started_playback_since(&playing)); // tạm dừng
    }

    #[test]
    fn page_returns_to_idle_when_media_disappears() {
        let page = Page::Media.after_media_change(&with_song(true), &idle());
        assert_eq!(page, Page::Idle);
    }

    #[test]
    fn user_choice_is_kept_when_only_pausing() {
        // Người dùng đang xem Idle khi nhạc phát; tạm dừng không được kéo sang trang khác
        let page = Page::Idle.after_media_change(&with_song(true), &with_song(false));
        assert_eq!(page, Page::Idle);
    }

    #[test]
    fn twelve_hour_clock_uses_sa_ch() {
        assert_eq!(format_time(&at(0), &H12, false), "12:05 SA");
        assert_eq!(format_time(&at(9), &H12, false), "9:05 SA");
        assert_eq!(format_time(&at(12), &H12, false), "12:05 CH");
        assert_eq!(format_time(&at(23), &H12, true), "11:05:07 CH");
    }
}
