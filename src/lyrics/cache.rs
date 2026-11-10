//! Bộ nhớ đệm lời bài hát trên đĩa: `%APPDATA%\DynamicIsland\lyrics\`, mỗi bài một file JSON nhỏ.
//!
//! - Chỉ lưu bài có lời. Bài không có lời và request lỗi không bao giờ được lưu,
//!   nên lần sau phát lại sẽ được tra lại.
//! - Lời đã lưu quá 90 ngày thì tải lại (tải lại lỗi thì vẫn dùng bản cũ).
//! - Vượt giới hạn dung lượng thì xóa file lâu không dùng nhất trước (LRU).
//!   "Lần dùng gần nhất" chính là thời gian sửa file: mỗi lần đọc trúng thì cập nhật lại.
//! - Xóa tay lúc nào cũng được: app chỉ coi đây là bản sao, mất thì tải lại.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::lrclib::Track;

/// Lời đã lưu quá lâu thì tra lại LRCLIB (có thể đã được sửa/đồng bộ tốt hơn)
pub const MAX_AGE: Duration = Duration::from_secs(90 * 24 * 60 * 60);
/// Đổi định dạng file thì tăng số này; file cũ sẽ bị bỏ qua và tải lại
const FORMAT_VERSION: u32 = 1;
const EXTENSION: &str = "json";

/// Nội dung một file cache
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Entry {
    version: u32,
    /// Lúc tải về, tính bằng giây kể từ 1970
    fetched_at: u64,
    title: String,
    artist: String,
    album: String,
    duration_s: u32,
    pub synced: Option<String>,
    pub plain: Option<String>,
}

/// Thông tin một file trong thư mục cache
#[derive(Debug, Clone, PartialEq)]
struct CachedFile {
    path: PathBuf,
    size: u64,
    used: SystemTime,
}

pub struct Cache {
    dir: PathBuf,
    limit_bytes: u64,
}

/// Viết thường, gộp khoảng trắng: "  Mortals  FUNK " → "mortals funk"
fn normalize(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Mã băm FNV-1a 64 bit. Tự viết thay vì dùng `DefaultHasher` vì thuật toán đó
/// có thể đổi giữa các phiên bản Rust, làm tên file cache đổi theo
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Khóa của một bài: tên + nghệ sĩ + album (không tính thời lượng, vì mỗi trình phát báo lệch nhau)
fn key(t: &Track) -> u64 {
    let id = format!(
        "{}\u{1f}{}\u{1f}{}",
        normalize(&t.title),
        normalize(&t.artist),
        normalize(&t.album)
    );
    fnv1a(id.as_bytes())
}

fn unix_now(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Những file cần xóa để tổng dung lượng không vượt `limit`: file lâu không dùng nhất trước
fn files_to_evict(mut files: Vec<CachedFile>, limit: u64) -> Vec<PathBuf> {
    let mut total: u64 = files.iter().map(|f| f.size).sum();
    files.sort_by_key(|f| f.used);
    let mut evict = Vec::new();
    for f in files {
        if total <= limit {
            break;
        }
        total -= f.size;
        evict.push(f.path);
    }
    evict
}

/// "12 bài · 340 KB"
pub fn describe(count: usize, bytes: u64) -> String {
    let size = if bytes < 1024 * 1024 {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    };
    format!("{count} bài · {size}")
}

/// Đánh dấu file vừa được dùng (cập nhật thời gian sửa) cho LRU
fn touch(path: &Path, now: SystemTime) {
    if let Ok(file) = fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(now);
    }
}

impl Cache {
    /// `limit_mb = 0` nghĩa là tắt cache: không đọc, không ghi đĩa
    pub fn new(dir: PathBuf, limit_mb: u32) -> Option<Self> {
        (limit_mb > 0).then(|| Self {
            dir,
            limit_bytes: u64::from(limit_mb) * 1024 * 1024,
        })
    }

    fn file_for(&self, t: &Track) -> PathBuf {
        self.dir.join(format!("{:016x}.{EXTENSION}", key(t)))
    }

    /// Lời đã lưu của bài `t` và cho biết còn mới (chưa quá 90 ngày) không
    pub fn load(&self, t: &Track) -> Option<(Entry, bool)> {
        self.load_at(t, SystemTime::now())
    }

    fn load_at(&self, t: &Track, now: SystemTime) -> Option<(Entry, bool)> {
        let path = self.file_for(t);
        let entry: Entry = serde_json::from_str(&fs::read_to_string(&path).ok()?).ok()?;
        // Khác định dạng, hoặc trùng mã băm với bài khác: coi như không có
        if entry.version != FORMAT_VERSION
            || normalize(&entry.title) != normalize(&t.title)
            || normalize(&entry.artist) != normalize(&t.artist)
        {
            return None;
        }
        touch(&path, now);
        let age = unix_now(now).saturating_sub(entry.fetched_at);
        Some((entry, age < MAX_AGE.as_secs()))
    }

    /// Lưu lời của bài `t` (chỉ khi thật sự có lời), rồi dọn bớt nếu vượt giới hạn
    pub fn store(
        &self,
        t: &Track,
        synced: Option<String>,
        plain: Option<String>,
    ) -> io::Result<()> {
        self.store_at(t, synced, plain, SystemTime::now())
    }

    fn store_at(
        &self,
        t: &Track,
        synced: Option<String>,
        plain: Option<String>,
        now: SystemTime,
    ) -> io::Result<()> {
        if synced.is_none() && plain.is_none() {
            return Ok(());
        }
        let entry = Entry {
            version: FORMAT_VERSION,
            fetched_at: unix_now(now),
            title: t.title.clone(),
            artist: t.artist.clone(),
            album: t.album.clone(),
            duration_s: t.duration_s,
            synced,
            plain,
        };
        let json = serde_json::to_string(&entry).map_err(io::Error::other)?;
        fs::create_dir_all(&self.dir)?;
        // Ghi ra file tạm rồi đổi tên, để lỡ tắt máy giữa chừng cũng không có file hỏng
        let path = self.file_for(t);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        self.evict()
    }

    fn files(&self) -> Vec<CachedFile> {
        let Ok(dir) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        dir.flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == EXTENSION))
            .filter_map(|e| {
                let meta = e.metadata().ok()?;
                Some(CachedFile {
                    path: e.path(),
                    size: meta.len(),
                    used: meta.modified().ok()?,
                })
            })
            .collect()
    }

    fn evict(&self) -> io::Result<()> {
        for path in files_to_evict(self.files(), self.limit_bytes) {
            fs::remove_file(path)?;
        }
        Ok(())
    }

    /// (số bài, tổng dung lượng) đang lưu
    pub fn stats(&self) -> (usize, u64) {
        let files = self.files();
        (files.len(), files.iter().map(|f| f.size).sum())
    }

    /// Xóa toàn bộ cache, trả về số bài đã xóa
    pub fn clear(&self) -> usize {
        self.files()
            .into_iter()
            .filter(|f| fs::remove_file(&f.path).is_ok())
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str) -> Track {
        Track {
            title: title.into(),
            artist: "LXNGVX, Warriyo".into(),
            album: "Mortals Funk Remix".into(),
            duration_s: 146,
        }
    }

    /// Thư mục tạm riêng cho từng test, tự xóa khi xong
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("island-lyrics-test-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn key_ignores_case_and_spacing_but_not_title() {
        let a = track("Mortals  Funk Remix");
        let b = Track {
            title: "mortals funk remix ".into(),
            artist: "lxngvx,  warriyo".into(),
            ..track("")
        };
        assert_eq!(key(&a), key(&b));
        assert_ne!(key(&a), key(&track("Mortals")));
    }

    #[test]
    fn fnv1a_matches_reference_values() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
    }

    #[test]
    fn zero_limit_disables_cache() {
        assert!(Cache::new(PathBuf::from("x"), 0).is_none());
    }

    #[test]
    fn stores_and_loads_lyrics() {
        let tmp = TempDir::new("roundtrip");
        let cache = Cache::new(tmp.0.clone(), 25).unwrap();
        let t = track("Mortals Funk Remix");
        cache
            .store_at(&t, Some("[00:01.00] hi".into()), None, at(1_000))
            .unwrap();
        let (entry, fresh) = cache.load_at(&t, at(2_000)).unwrap();
        assert_eq!(entry.synced.as_deref(), Some("[00:01.00] hi"));
        assert!(fresh);
        assert_eq!(cache.stats().0, 1);
    }

    #[test]
    fn songs_without_lyrics_are_not_stored() {
        let tmp = TempDir::new("nolyrics");
        let cache = Cache::new(tmp.0.clone(), 25).unwrap();
        cache.store(&track("Instrumental"), None, None).unwrap();
        assert_eq!(cache.stats(), (0, 0));
    }

    #[test]
    fn old_entries_are_stale_after_90_days() {
        let tmp = TempDir::new("stale");
        let cache = Cache::new(tmp.0.clone(), 25).unwrap();
        let t = track("Old");
        cache
            .store_at(&t, None, Some("words".into()), at(0))
            .unwrap();
        let day = 24 * 60 * 60;
        assert!(cache.load_at(&t, at(89 * day)).unwrap().1);
        assert!(!cache.load_at(&t, at(91 * day)).unwrap().1);
    }

    #[test]
    fn evicts_least_recently_used_first() {
        let file = |name: &str, size: u64, used: u64| CachedFile {
            path: PathBuf::from(name),
            size,
            used: at(used),
        };
        let files = vec![
            file("new", 40, 300),
            file("old", 40, 100),
            file("mid", 40, 200),
        ];
        // 120 byte, giới hạn 80: chỉ cần xóa file cũ nhất
        assert_eq!(files_to_evict(files.clone(), 80), [PathBuf::from("old")]);
        assert_eq!(
            files_to_evict(files.clone(), 40),
            [PathBuf::from("old"), PathBuf::from("mid")]
        );
        assert!(files_to_evict(files, 1_000).is_empty());
    }

    #[test]
    fn clear_removes_everything() {
        let tmp = TempDir::new("clear");
        let cache = Cache::new(tmp.0.clone(), 25).unwrap();
        for title in ["A", "B", "C"] {
            cache.store(&track(title), None, Some("x".into())).unwrap();
        }
        assert_eq!(cache.clear(), 3);
        assert_eq!(cache.stats(), (0, 0));
    }

    #[test]
    fn describe_formats_size() {
        assert_eq!(describe(0, 0), "0 bài · 0 KB");
        assert_eq!(describe(12, 340 * 1024), "12 bài · 340 KB");
        assert_eq!(describe(500, 5 * 1024 * 1024 + 300_000), "500 bài · 5.3 MB");
    }
}
