//! Lời bài hát chạy theo nhạc (live lyrics).
//!
//! Luồng media báo bài mới qua [`Requester::request`] → luồng lời bài hát tìm trong cache,
//! không có thì hỏi LRCLIB (không ra lời đồng bộ thì nhờ AI đoán tên bài thật rồi tìm lại)
//! → ghi kết quả vào biến dùng chung → báo cửa sổ bằng `WM_LYRICS_CHANGED`.
//! Giao diện đọc kết quả bằng [`status_for`].
//!
//! Một module có thư mục riêng: `lyrics/mod.rs` là cửa chính, các file con
//! (`lrc`, `lrclib`, `cache`, `ai_names`) là chi tiết bên trong, bên ngoài không cần biết.

mod ai_names;
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
use ai_names::Guesser;
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

/// Bài vừa được tra lời gần nhất, cho cửa sổ Cài đặt chỉnh lời sớm/muộn
#[derive(Debug, Clone, PartialEq)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    /// Có lời chạy theo nhạc không (chỉ lời này mới chỉnh sớm/muộn được)
    pub synced: bool,
    pub timing: Timing,
}

pub fn now_playing() -> Option<NowPlaying> {
    let state = STATE.lock().unwrap();
    let (track, status, timing) = state.as_ref()?;
    Some(NowPlaying {
        title: track.title.clone(),
        artist: track.artist.clone(),
        synced: matches!(status, Status::Synced(_)),
        timing: *timing,
    })
}

/// Người dùng chỉnh lời sớm/muộn cho bài (title, artist). `save`: ghi luôn xuống đĩa
/// (lúc đang kéo thanh trượt trong Cài đặt thì chưa ghi, thả tay mới ghi)
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

/// Bật tính năng: chuẩn bị cache (`cache_mb = 0` là tắt) và chạy luồng nền.
/// `ai`: (Account ID, model) để nhờ AI đoán tên bài khi không tìm được lời; None = không dùng
pub fn start(hwnd_raw: isize, cache_mb: u32, ai: Option<(&str, &str)>) -> Requester {
    let dir = crate::config::path().and_then(|p| Some(p.parent()?.join("lyrics")));
    let _ = CACHE.set(dir.and_then(|d| Cache::new(d, cache_mb)));
    let guesser = ai.and_then(|(account_id, model)| Guesser::new(account_id, model));
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(hwnd_raw, rx, guesser));
    Requester { tx }
}

/// "12 bài · 340 KB" cho menu khay; None nếu cache đang tắt
pub fn cache_summary() -> Option<(usize, String)> {
    let (count, bytes) = cache()?.stats();
    Some((count, cache::describe(count, bytes)))
}

/// Xóa cache trên đĩa (lời đang hiện trên màn hình vẫn giữ), kể cả tên bài AI đã đoán
pub fn clear_cache() {
    if let Some(c) = cache() {
        c.clear();
    }
    ai_names::forget_all();
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

/// LRCLIB không có lời đồng bộ cho tên đang phát: nhờ AI đoán tên bài và ca sĩ thật rồi tìm
/// lại. Chỉ thay kết quả cũ khi tìm được lời đồng bộ, hoặc trước đó chẳng có lời nào
fn retry_with_ai(session: &Session, guesser: &Guesser, t: &Track, found: Lookup) -> Lookup {
    if found.is_synced() || found == Lookup::Instrumental {
        return found;
    }
    let names = match guesser.names_for(t) {
        Ok(Some(names)) => names,
        Ok(None) => return found,
        Err(e) => {
            crate::trace::log(format!("AI đoán tên bài lỗi: {e}"));
            return found;
        }
    };
    crate::trace::log(format!(
        "AI đoán «{}» / «{}» là «{}» của «{}»",
        t.title, t.artist, names.title, names.artist
    ));
    match lrclib::fetch_named(session, t, &names.title, &names.artist) {
        Ok(better @ Lookup::Lyrics { .. }) if better.is_synced() || found == Lookup::NotFound => {
            better
        }
        _ => found,
    }
}

/// Tìm lời cho một bài: cache còn mới → dùng luôn; không thì hỏi LRCLIB (và AI nếu cần)
fn lookup(session: Option<&Session>, guesser: Option<&Guesser>, t: &Track) -> (Status, Timing) {
    let cached = cache().and_then(|c| c.load(t));
    if let Some((entry, true)) = &cached {
        let status = status_from(entry.synced.as_deref(), entry.plain.as_deref());
        return (status, entry.timing());
    }
    // Tải lại thì vẫn giữ độ lệch người dùng đã chỉnh trước đó
    let old_offset = cached.as_ref().map_or(0, |(e, _)| e.offset_ms);
    let fetched = session
        .ok_or_else(|| "không mở được kết nối".to_string())
        .and_then(|s| {
            let found = lrclib::fetch(s, t)?;
            Ok(match guesser {
                Some(g) => retry_with_ai(s, g, t, found),
                None => found,
            })
        });
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

fn run(hwnd_raw: isize, rx: Receiver<Track>, guesser: Option<Guesser>) {
    let session = Session::open().ok();
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);

    while let Ok(mut track) = rx.recv() {
        // Người dùng bấm "bài tiếp" liên tục: bỏ qua các bài ở giữa, chỉ tra bài mới nhất
        while let Ok(newer) = rx.try_recv() {
            track = newer;
        }
        let (status, timing) = lookup(session.as_ref(), guesser.as_ref(), &track);

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

    /// Thăm dò bằng tài khoản thật: cần Account ID trong config.toml và token trong Credential
    /// Manager. Câu trả lời AI nhớ vào thư mục tạm, không đụng dữ liệu thật.
    /// Chạy: cargo test live_ai_guess -- --ignored --nocapture
    #[test]
    #[ignore = "gọi Cloudflare Workers AI và LRCLIB thật"]
    fn live_ai_guess_for_tricky_titles() {
        let ai = crate::config::load().unwrap().ai;
        let temp = std::env::temp_dir().join("island_ai_guess_test");
        let _ = std::fs::create_dir_all(temp.join("DynamicIsland"));
        // SAFETY: chỉ test này đổi APPDATA, chạy một mình với --ignored
        unsafe { std::env::set_var("APPDATA", &temp) };
        println!("model: {}", ai.model);
        let guesser = Guesser::new(&ai.account_id, &ai.model).expect("chưa nhập Account ID");
        let session = Session::open().unwrap();
        let cases = [
            (
                "[Vietsub + Lyrics] Shape of You - Ed Sheeran",
                "Kênh Vietsub Hay",
                0,
            ),
            ("Chúng Ta Của Hiện Tại | Lyrics Video", "Lyrics Việt Nam", 0),
            (
                "Hãy Trao Cho Anh (Lyrics) ft. Snoop Dogg",
                "Nhạc Trẻ Hay Nhất",
                0,
            ),
            (
                "Nơi Này Có Anh | Lyrics | Nhạc Trẻ Hot",
                "Kênh Lyrics Hay",
                0,
            ),
            (
                "Ngày Mai Người Ta Lấy Chồng (Lyrics Video)",
                "Lofi Chill Việt",
                0,
            ),
            (
                "See Tình - Hoàng Thùy Linh | Nhạc Hot TikTok 2022",
                "TikTok Music VN",
                0,
            ),
            (
                "LẠC TRÔI | OFFICIAL MUSIC VIDEO | SƠN TÙNG M-TP",
                "Sơn Tùng M-TP Official",
                0,
            ),
        ];
        for (title, artist, duration_s) in cases {
            let t = Track {
                title: title.into(),
                artist: artist.into(),
                album: String::new(),
                duration_s,
            };
            let before = lrclib::fetch(&session, &t).unwrap();
            let names = if before.is_synced() {
                None
            } else {
                Some(guesser.names_for(&t))
            };
            let after = retry_with_ai(&session, &guesser, &t, before.clone());
            let kind = |l: &Lookup| match l {
                Lookup::Lyrics {
                    synced: Some(s), ..
                } => format!("lời đồng bộ: {}", s.lines().nth(4).unwrap_or_default()),
                Lookup::Lyrics { .. } => "chỉ lời thường".to_string(),
                Lookup::Instrumental => "không lời".to_string(),
                Lookup::NotFound => "không thấy".to_string(),
            };
            println!(
                "{title} / {artist}\n  trước: {}\n  AI: {names:?}\n  sau:   {}",
                kind(&before),
                kind(&after)
            );
        }
        let _ = std::fs::remove_dir_all(temp);
    }
}
