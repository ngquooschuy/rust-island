//! Đọc bài đang phát và gửi lệnh điều khiển qua API media của Windows.
//! Chạy trên một luồng nền riêng; báo cho cửa sổ bằng `WM_MEDIA_CHANGED`.

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use windows::{
    Graphics::Imaging::{
        BitmapAlphaMode, BitmapBounds, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat,
        BitmapTransform, ColorManagementMode, ExifOrientationMode,
    },
    Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    },
    Storage::Streams::IRandomAccessStream,
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::{
            SystemInformation::GetSystemTimePreciseAsFileTime,
            WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
        },
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
    core::{Interface, Result},
};

use crate::icon::Icon;
use crate::lyrics::{Requester, Track};

/// Luồng media gửi thông điệp này về cửa sổ khi bài hát/trạng thái đổi
pub const WM_MEDIA_CHANGED: u32 = WM_APP + 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaCommand {
    Previous,
    TogglePlayPause,
    Next,
    /// Tua tới vị trí này (ms tính từ đầu bài)
    Seek(u64),
}

impl MediaCommand {
    /// Lệnh ứng với nút thứ `i` trên island (0 = lùi, 1 = phát/dừng, 2 = tới)
    pub fn from_button(i: usize) -> Option<Self> {
        match i {
            0 => Some(Self::Previous),
            1 => Some(Self::TogglePlayPause),
            2 => Some(Self::Next),
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Default, Debug)]
pub struct MediaInfo {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub playing: bool,
    /// Vị trí phát (ms) tại thời điểm `updated_ticks`; 0 nếu trình phát không báo
    pub position_ms: u64,
    /// Thời lượng bài (ms); 0 nếu không rõ
    pub duration_ms: u64,
    /// Lúc trình phát báo vị trí, đơn vị 100 ns kể từ năm 1601 (giống FILETIME)
    pub updated_ticks: i64,
}

impl MediaInfo {
    /// Bài này dưới dạng yêu cầu tra lời
    fn as_track(&self) -> Track {
        Track {
            title: self.title.clone(),
            artist: self.artist.clone(),
            album: self.album.clone(),
            duration_s: (self.duration_ms / 1000) as u32,
        }
    }
}

// Dữ liệu dùng chung: luồng media ghi, luồng giao diện đọc
static MEDIA: Mutex<MediaInfo> = Mutex::new(MediaInfo {
    title: String::new(),
    artist: String::new(),
    album: String::new(),
    playing: false,
    position_ms: 0,
    duration_ms: 0,
    updated_ticks: 0,
});

/// Ảnh bìa của bài đang phát, kèm khóa của bài đó (để không hiện nhầm ảnh bài trước)
static COVER: Mutex<Option<(String, Arc<Icon>)>> = Mutex::new(None);
/// Ảnh bìa được cắt vuông và thu về cạnh này (điểm ảnh); vẽ ra nhỏ hơn nên vẫn nét
const COVER_SIZE: u32 = 96;
/// Trình phát thường báo ảnh bìa chậm hơn tên bài một chút: thử lại vài lần
const COVER_TRIES: u32 = 4;

fn track_key(title: &str, artist: &str) -> String {
    format!("{title}\u{1}{artist}")
}

/// Ảnh bìa nếu đúng là của bài (title, artist)
pub fn cover_for(title: &str, artist: &str) -> Option<Arc<Icon>> {
    let cover = COVER.lock().unwrap();
    let (key, icon) = cover.as_ref()?;
    (*key == track_key(title, artist)).then(|| icon.clone())
}

/// Kích thước sau khi thu nhỏ để cạnh ngắn bằng `size`, và vùng vuông ở giữa cần cắt
fn square_crop(w: u32, h: u32, size: u32) -> ((u32, u32), BitmapBounds) {
    let short = w.min(h).max(1) as f32;
    let scaled = |v: u32| ((v as f32 * size as f32 / short).round() as u32).max(size);
    let (sw, sh) = (scaled(w), scaled(h));
    let bounds = BitmapBounds {
        X: (sw - size) / 2,
        Y: (sh - size) / 2,
        Width: size,
        Height: size,
    };
    ((sw, sh), bounds)
}

/// Giải mã ảnh (JPEG/PNG...) thành ô vuông COVER_SIZE, cắt phần giữa như ảnh bìa
fn decode_cover(stream: &IRandomAccessStream) -> Result<Icon> {
    let decoder = BitmapDecoder::CreateAsync(stream)?.join()?;
    let ((sw, sh), bounds) = square_crop(decoder.PixelWidth()?, decoder.PixelHeight()?, COVER_SIZE);
    let transform = BitmapTransform::new()?;
    transform.SetScaledWidth(sw)?;
    transform.SetScaledHeight(sh)?;
    transform.SetBounds(bounds)?;
    transform.SetInterpolationMode(BitmapInterpolationMode::Fant)?; // thu nhỏ mịn
    let data = decoder
        .GetPixelDataTransformedAsync(
            BitmapPixelFormat::Bgra8,
            BitmapAlphaMode::Premultiplied,
            &transform,
            ExifOrientationMode::RespectExifOrientation,
            ColorManagementMode::DoNotColorManage,
        )?
        .join()?;
    Ok(Icon {
        width: COVER_SIZE,
        height: COVER_SIZE,
        pixels: data.DetachPixelData()?.to_vec(),
    })
}

/// Ảnh bìa trình phát báo cho bài hiện tại
fn load_cover(manager: &GlobalSystemMediaTransportControlsSessionManager) -> Option<Icon> {
    if let Some(path) = fake_cover_path() {
        let file = windows::Storage::StorageFile::GetFileFromPathAsync(&path.into())
            .ok()?
            .join()
            .ok()?;
        let stream = file.OpenReadAsync().ok()?.join().ok()?;
        return decode_cover(&stream.cast().ok()?).ok();
    }
    let session = manager.GetCurrentSession().ok()?;
    let props = session.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    decode_cover(&stream.cast().ok()?).ok()
}

/// Giờ hiện tại theo đồng hồ hệ thống, đơn vị 100 ns kể từ năm 1601
pub fn now_ticks() -> i64 {
    let ft = unsafe { GetSystemTimePreciseAsFileTime() };
    ((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)) as i64
}

/// Vị trí phát ngay lúc `now_ticks`. Trình phát chỉ báo vị trí thỉnh thoảng
/// (khi phát, dừng, tua), nên lúc đang phát phải tự cộng thêm thời gian đã trôi qua
pub fn position_at(info: &MediaInfo, now_ticks: i64) -> u64 {
    let mut pos = info.position_ms;
    if info.playing && info.updated_ticks > 0 && now_ticks > info.updated_ticks {
        pos += ((now_ticks - info.updated_ticks) / 10_000) as u64; // 100 ns → ms
    }
    if info.duration_ms > 0 {
        pos.min(info.duration_ms)
    } else {
        pos
    }
}

/// Bản sao thông tin media mới nhất
pub fn snapshot() -> MediaInfo {
    MEDIA.lock().unwrap().clone()
}

fn query(manager: &GlobalSystemMediaTransportControlsSessionManager) -> Result<MediaInfo> {
    let session = manager.GetCurrentSession()?;
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
    let mut info = MediaInfo {
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        album: props.AlbumTitle()?.to_string(),
        playing: status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing,
        ..MediaInfo::default()
    };
    // Vị trí phát: không phải trình phát nào cũng báo, nên lỗi thì bỏ qua
    if let Ok(timeline) = session.GetTimelineProperties() {
        let ticks = |t: Result<windows::Foundation::TimeSpan>| t.map_or(0, |t| t.Duration);
        let start = ticks(timeline.StartTime());
        let to_ms = |t: i64| ((t - start).max(0) / 10_000) as u64; // 100 ns → ms
        info.position_ms = to_ms(ticks(timeline.Position()));
        info.duration_ms = to_ms(ticks(timeline.EndTime()));
        info.updated_ticks = timeline.LastUpdatedTime().map_or(0, |d| d.UniversalTime);
    }
    Ok(info)
}

fn run_command(manager: &GlobalSystemMediaTransportControlsSessionManager, cmd: MediaCommand) {
    if let Ok(session) = manager.GetCurrentSession() {
        let op = match cmd {
            MediaCommand::Previous => session.TrySkipPreviousAsync(),
            MediaCommand::TogglePlayPause => session.TryTogglePlayPauseAsync(),
            MediaCommand::Next => session.TrySkipNextAsync(),
            // Windows tính vị trí theo đơn vị 100 ns
            MediaCommand::Seek(ms) => {
                session.TryChangePlaybackPositionAsync(ms.saturating_mul(10_000) as i64)
            }
        };
        let _ = op.and_then(|op| op.join());
    }
}

/// Vòng lặp của luồng media: nhận lệnh từ giao diện, hỏi trạng thái mỗi giây.
/// `hwnd_raw` là HWND đổi sang số, vì HWND chứa con trỏ nên không gửi qua luồng được.
/// `lyrics`: nếu bật lời bài hát, báo cho luồng lời mỗi khi đổi bài
pub fn run(hwnd_raw: isize, commands: Receiver<MediaCommand>, lyrics: Option<Requester>) {
    unsafe {
        let _ = RoInitialize(RO_INIT_MULTITHREADED);
    }
    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.join())
    {
        Ok(m) => m,
        Err(_) => return,
    };
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);

    let mut last = MediaInfo::default();
    let mut requested: Option<Track> = None;
    let started_ticks = now_ticks();
    // Bài đang lấy ảnh bìa, và còn thử được mấy lần
    let mut cover_track = String::new();
    let mut cover_tries = 0;
    loop {
        match commands.recv_timeout(Duration::from_millis(1000)) {
            Ok(cmd) => {
                run_command(&manager, cmd);
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let info = fake_media(started_ticks).unwrap_or_else(|| query(&manager).unwrap_or_default());
        if let Some(lyrics) = &lyrics {
            let track = info.as_track();
            if needs_lyrics_request(requested.as_ref(), &track) {
                lyrics.request(track.clone());
                requested = Some(track);
            }
        }
        // Ảnh bìa: đổi bài thì lấy ảnh mới; chưa có ảnh thì thử lại vài giây sau
        let mut cover_changed = false;
        let key = track_key(&info.title, &info.artist);
        if info.title.is_empty() {
            cover_track.clear();
        } else if key != cover_track {
            cover_track = key.clone();
            cover_tries = COVER_TRIES;
            *COVER.lock().unwrap() = None;
            cover_changed = true;
        }
        if cover_tries > 0 {
            cover_tries -= 1;
            if let Some(icon) = load_cover(&manager) {
                *COVER.lock().unwrap() = Some((key, Arc::new(icon)));
                cover_tries = 0;
                cover_changed = true;
            }
        }
        if info != last || cover_changed {
            *MEDIA.lock().unwrap() = info.clone();
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_MEDIA_CHANGED, WPARAM(0), LPARAM(0));
            }
            last = info;
        }
    }
}

/// Chỉ bản debug: ISLAND_FAKE_COVER="đường dẫn ảnh" làm ảnh bìa cho bài giả lập
fn fake_cover_path() -> Option<String> {
    if !cfg!(debug_assertions) || std::env::var_os("ISLAND_FAKE_MEDIA").is_none() {
        return None;
    }
    std::env::var("ISLAND_FAKE_COVER").ok()
}

/// Chỉ bản debug: ISLAND_FAKE_MEDIA="Tên bài|Nghệ sĩ|Album|thời lượng (giây)|bắt đầu từ giây"
/// giả lập một bài đang phát (không có tiếng) để thử lời bài hát và thanh tiến độ
fn fake_media(started_ticks: i64) -> Option<MediaInfo> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let value = std::env::var("ISLAND_FAKE_MEDIA").ok()?;
    let parts: Vec<&str> = value.split('|').map(str::trim).collect();
    let duration_s: u64 = parts.get(3)?.parse().ok()?;
    let start_s: u64 = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0);
    Some(MediaInfo {
        title: parts[0].to_string(),
        artist: parts.get(1)?.to_string(),
        album: parts.get(2)?.to_string(),
        playing: true,
        position_ms: start_s * 1000,
        duration_ms: duration_s * 1000,
        updated_ticks: started_ticks,
    })
}

/// Có cần tra lời lại không: đổi bài, hoặc lần trước chưa biết thời lượng mà giờ đã biết
/// (biết thời lượng thì LRCLIB khớp chính xác hơn)
fn needs_lyrics_request(requested: Option<&Track>, now: &Track) -> bool {
    if now.title.is_empty() {
        return false;
    }
    match requested {
        None => true,
        Some(r) => {
            r.title != now.title
                || r.artist != now.artist
                || r.album != now.album
                || (r.duration_s == 0 && now.duration_s > 0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECOND: i64 = 10_000_000; // 1 giây theo đơn vị 100 ns

    fn playing_at(position_ms: u64, updated_ticks: i64) -> MediaInfo {
        MediaInfo {
            title: "Song".into(),
            playing: true,
            position_ms,
            duration_ms: 146_000,
            updated_ticks,
            ..MediaInfo::default()
        }
    }

    /// Đọc bài đang phát thật trên máy rồi tra lời; chạy bằng
    /// `cargo test probe_current_media -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_current_media() {
        unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
        }
        let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
            .and_then(|op| op.join())
            .unwrap();
        let info = query(&manager).unwrap();
        println!("media: {info:#?}");
        println!("position now: {} ms", position_at(&info, now_ticks()));
        let track = info.as_track();
        let session = crate::http::Session::open().unwrap();
        println!("lookup: {:?}", crate::lyrics::probe_fetch(&session, &track));
    }

    #[test]
    fn position_advances_while_playing() {
        let info = playing_at(29_000, 100 * SECOND);
        assert_eq!(position_at(&info, 100 * SECOND), 29_000);
        assert_eq!(position_at(&info, 102 * SECOND + SECOND / 2), 31_500);
    }

    #[test]
    fn position_stays_when_paused_and_never_passes_the_end() {
        let paused = MediaInfo {
            playing: false,
            ..playing_at(29_000, 100 * SECOND)
        };
        assert_eq!(position_at(&paused, 500 * SECOND), 29_000);
        let near_end = playing_at(145_000, 100 * SECOND);
        assert_eq!(position_at(&near_end, 200 * SECOND), 146_000);
    }

    #[test]
    fn unknown_update_time_is_not_extrapolated() {
        let info = playing_at(5_000, 0);
        assert_eq!(position_at(&info, 999 * SECOND), 5_000);
    }

    #[test]
    fn lyrics_are_requested_once_per_song() {
        let song = |title: &str, duration_s| Track {
            title: title.into(),
            artist: "A".into(),
            album: String::new(),
            duration_s,
        };
        assert!(needs_lyrics_request(None, &song("X", 146)));
        assert!(!needs_lyrics_request(
            Some(&song("X", 146)),
            &song("X", 146)
        ));
        assert!(needs_lyrics_request(Some(&song("X", 146)), &song("Y", 146)));
        // Lần đầu chưa biết thời lượng, giờ đã biết: tra lại cho chính xác
        assert!(needs_lyrics_request(Some(&song("X", 0)), &song("X", 146)));
        assert!(!needs_lyrics_request(Some(&song("X", 146)), &song("X", 0)));
        // Không có bài nào đang phát
        assert!(!needs_lyrics_request(None, &song("", 0)));
    }

    #[test]
    fn from_button_maps_buttons_left_to_right() {
        assert_eq!(MediaCommand::from_button(0), Some(MediaCommand::Previous));
        assert_eq!(
            MediaCommand::from_button(1),
            Some(MediaCommand::TogglePlayPause)
        );
        assert_eq!(MediaCommand::from_button(2), Some(MediaCommand::Next));
        assert_eq!(MediaCommand::from_button(3), None);
    }
}
