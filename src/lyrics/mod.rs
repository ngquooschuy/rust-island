//! Lời bài hát chạy theo nhạc (live lyrics).
//!
//! Luồng media báo bài mới qua [`Requester::request`] → luồng lời bài hát tìm trong cache,
//! không có thì hỏi LRCLIB → ghi kết quả vào biến dùng chung → báo cửa sổ bằng
//! `WM_LYRICS_CHANGED`. Giao diện đọc kết quả bằng [`status_for`].
//!
//! Một module có thư mục riêng: `lyrics/mod.rs` là cửa chính, các file con
//! (`lrc`, `lrclib`, `cache`) là chi tiết bên trong, bên ngoài không cần biết.

mod cache;
mod lrc;
mod lrclib;

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, WM_APP},
};

use crate::http::Session;
use cache::Cache;
use lrc::Lyrics;
#[cfg(test)]
pub use lrc::parse as parse_lrc;
pub use lrclib::Lookup;
pub use lrclib::Track;

/// Luồng lời bài hát gửi thông điệp này về cửa sổ khi có kết quả
pub const WM_LYRICS_CHANGED: u32 = WM_APP + 4;

/// Trạng thái lời của bài đang phát
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Loading,
    /// `Arc`: giao diện sao chép trạng thái nhiều lần mỗi giây, Arc giúp chỉ tăng bộ đếm
    Synced(Arc<Lyrics>),
    /// Có lời nhưng không có mốc thời gian
    PlainOnly,
    Instrumental,
    NotFound,
    /// Mất mạng hoặc máy chủ lỗi (không lưu cache, lần sau phát lại sẽ thử lại)
    Failed,
}

/// Chỉnh thời gian cho lời của một bài
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    /// Lời sớm/muộn bao nhiêu ms so với nhạc (dương = lời hiện muộn hơn)
    pub offset_ms: i64,
    /// Bản lời dài khác bài đang phát (vd. video MV có đoạn mở đầu): lời có thể lệch
    pub approximate: bool,
}

/// Chỉnh lệch được tối đa ±1 phút (đủ cho đoạn mở đầu của hầu hết video MV)
pub const MAX_OFFSET_MS: i64 = 60_000;

// Bài đang được tra, kết quả và chỉnh thời gian; luồng lời ghi, luồng giao diện đọc
static STATE: Mutex<Option<(Track, Status, Timing)>> = Mutex::new(None);
// Cache dùng chung giữa luồng lời bài hát và menu khay (None = đang tắt)
static CACHE: OnceLock<Option<Cache>> = OnceLock::new();

fn cache() -> Option<&'static Cache> {
    CACHE.get().and_then(Option::as_ref)
}

/// Trạng thái lời nếu đúng là của bài (title, artist), để không hiện lời bài cũ cho bài mới
pub fn status_for(title: &str, artist: &str) -> Option<Status> {
    let state = STATE.lock().unwrap();
    let (track, status, _) = state.as_ref()?;
    (track.title == title && track.artist == artist).then(|| status.clone())
}

/// Chỉnh thời gian của bài (title, artist); bài khác bài đang tra thì không chỉnh gì
pub fn timing_for(title: &str, artist: &str) -> Timing {
    let state = STATE.lock().unwrap();
    match state.as_ref() {
        Some((track, _, timing)) if track.title == title && track.artist == artist => *timing,
        _ => Timing::default(),
    }
}

/// Người dùng chỉnh lời sớm/muộn cho bài (title, artist). `save`: ghi luôn xuống đĩa
/// (lúc đang kéo thanh trượt thì chưa ghi, thả tay mới ghi)
pub fn set_offset(title: &str, artist: &str, offset_ms: i64, save: bool) {
    let offset_ms = offset_ms.clamp(-MAX_OFFSET_MS, MAX_OFFSET_MS);
    let track = {
        let mut state = STATE.lock().unwrap();
        match state.as_mut() {
            Some((track, _, timing)) if track.title == title && track.artist == artist => {
                timing.offset_ms = offset_ms;
                track.clone()
            }
            _ => return,
        }
    };
    if save && let Some(c) = cache() {
        let _ = c.set_offset(&track, offset_ms);
    }
}

/// Đầu gửi yêu cầu, luồng media giữ để báo bài mới
pub struct Requester {
    tx: Sender<Track>,
}

impl Requester {
    pub fn request(&self, track: Track) {
        // Đặt "đang tìm" ngay để giao diện không hiện lời của bài trước
        *STATE.lock().unwrap() = Some((track.clone(), Status::Loading, Timing::default()));
        let _ = self.tx.send(track);
    }
}

/// Bật tính năng: chuẩn bị cache (`cache_mb = 0` là tắt) và chạy luồng nền
pub fn start(hwnd_raw: isize, cache_mb: u32) -> Requester {
    let dir = crate::config::path().and_then(|p| Some(p.parent()?.join("lyrics")));
    let _ = CACHE.set(dir.and_then(|d| Cache::new(d, cache_mb)));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(hwnd_raw, rx));
    Requester { tx }
}

/// "12 bài · 340 KB" cho menu khay; None nếu cache đang tắt
pub fn cache_summary() -> Option<(usize, String)> {
    let (count, bytes) = cache()?.stats();
    Some((count, cache::describe(count, bytes)))
}

/// Xóa cache trên đĩa (lời đang hiện trên màn hình vẫn giữ)
pub fn clear_cache() {
    if let Some(c) = cache() {
        c.clear();
    }
}

/// Chỉ dùng trong test thăm dò: tra LRCLIB trực tiếp, bỏ qua cache
#[cfg(test)]
pub fn probe_fetch(session: &Session, t: &Track) -> Result<Lookup, String> {
    lrclib::fetch(session, t)
}

/// Đổi kết quả tra cứu thành trạng thái hiển thị
fn status_from(synced: Option<&str>, plain: Option<&str>) -> Status {
    let lyrics = synced.map(lrc::parse).filter(|l| !l.is_empty());
    match (lyrics, plain) {
        (Some(l), _) => Status::Synced(Arc::new(l)),
        (None, Some(_)) => Status::PlainOnly,
        (None, None) => Status::NotFound,
    }
}

/// Tìm lời cho một bài: cache còn mới → dùng luôn; không thì hỏi LRCLIB
fn lookup(session: Option<&Session>, t: &Track) -> (Status, Timing) {
    let cached = cache().and_then(|c| c.load(t));
    if let Some((entry, true)) = &cached {
        let status = status_from(entry.synced.as_deref(), entry.plain.as_deref());
        return (status, entry.timing());
    }
    // Tải lại thì vẫn giữ độ lệch người dùng đã chỉnh trước đó
    let old_offset = cached.as_ref().map_or(0, |(e, _)| e.offset_ms);
    let fetched = session
        .ok_or_else(|| "không mở được kết nối".to_string())
        .and_then(|s| lrclib::fetch(s, t));
    // Bản cũ (quá 90 ngày) dùng làm phương án dự phòng khi tải lại thất bại
    let stale = |otherwise: Status| {
        cached
            .as_ref()
            .map_or((otherwise, Timing::default()), |(e, _)| {
                (
                    status_from(e.synced.as_deref(), e.plain.as_deref()),
                    e.timing(),
                )
            })
    };
    match fetched {
        Ok(Lookup::Lyrics {
            synced,
            plain,
            approximate,
        }) => {
            let status = status_from(synced.as_deref(), plain.as_deref());
            let timing = Timing {
                offset_ms: old_offset,
                approximate,
            };
            if let Some(c) = cache() {
                let _ = c.store(t, synced, plain, timing);
            }
            (status, timing)
        }
        Ok(Lookup::Instrumental) => (Status::Instrumental, Timing::default()),
        Ok(Lookup::NotFound) => stale(Status::NotFound),
        Err(_) => stale(Status::Failed),
    }
}

fn run(hwnd_raw: isize, rx: Receiver<Track>) {
    let session = Session::open().ok();
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);

    while let Ok(mut track) = rx.recv() {
        // Người dùng bấm "bài tiếp" liên tục: bỏ qua các bài ở giữa, chỉ tra bài mới nhất
        while let Ok(newer) = rx.try_recv() {
            track = newer;
        }
        let (status, timing) = lookup(session.as_ref(), &track);

        let mut state = STATE.lock().unwrap();
        // Trong lúc tra, bài có thể đã đổi: chỉ ghi nếu vẫn là bài này
        if state.as_ref().is_some_and(|(t, _, _)| *t == track) {
            *state = Some((track, status, timing));
            drop(state); // nhả khóa trước khi báo cửa sổ
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_LYRICS_CHANGED, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_prefers_synced_lyrics() {
        let s = status_from(Some("[00:01.00] hi"), Some("hi"));
        assert!(matches!(s, Status::Synced(l) if l.lines.len() == 1));
    }

    #[test]
    fn synced_text_without_timestamps_falls_back_to_plain() {
        assert_eq!(
            status_from(Some("no timing"), Some("hi")),
            Status::PlainOnly
        );
        assert_eq!(status_from(None, Some("hi")), Status::PlainOnly);
        assert_eq!(status_from(None, None), Status::NotFound);
    }

    #[test]
    fn status_is_only_shown_for_the_matching_song() {
        let t = Track {
            title: "Song A".into(),
            artist: "Someone".into(),
            album: String::new(),
            duration_s: 0,
        };
        *STATE.lock().unwrap() = Some((t, Status::NotFound, Timing::default()));
        assert_eq!(status_for("Song A", "Someone"), Some(Status::NotFound));
        assert_eq!(status_for("Song B", "Someone"), None);
        // Chỉnh lệch: chỉ áp cho đúng bài, và bị giới hạn ±1 phút
        set_offset("Song A", "Someone", 90_000, false);
        assert_eq!(timing_for("Song A", "Someone").offset_ms, MAX_OFFSET_MS);
        set_offset("Song B", "Someone", 5_000, false);
        assert_eq!(timing_for("Song B", "Someone"), Timing::default());
    }
}
