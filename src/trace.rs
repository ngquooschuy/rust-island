//! Nhật ký gỡ lỗi, chỉ có trong bản debug (`cargo run`): ghi lúc island ẩn/hiện vì app
//! toàn màn hình, lúc thu thành notch, và cả thông báo lỗi khi app crash, vào
//! `%APPDATA%\DynamicIsland\debug.log`. Bản release không ghi gì.

use std::fmt::Display;
use std::fs::OpenOptions;
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

fn path() -> Option<std::path::PathBuf> {
    crate::config::path().map(|p| p.with_file_name("debug.log"))
}

/// Ghi một dòng (kèm giờ:phút:giây.mili giây UTC)
pub fn log(message: impl Display) {
    if !cfg!(debug_assertions) {
        return;
    }
    let Some(path) = path() else { return };
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let (h, m, s, milli) = (
        ms / 3_600_000 % 24,
        ms / 60_000 % 60,
        ms / 1000 % 60,
        ms % 1000,
    );
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{h:02}:{m:02}:{s:02}.{milli:03} {message}");
    }
}

/// Bắt đầu phiên mới: xóa nhật ký cũ, và ghi lại lỗi nếu app crash
pub fn start() {
    if !cfg!(debug_assertions) {
        return;
    }
    if let Some(path) = path() {
        let _ = std::fs::remove_file(path);
    }
    log(format!(
        "khởi động: {}",
        std::env::current_exe().map_or_else(|_| "?".into(), |p| p.display().to_string())
    ));
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        log(format!("CRASH: {info}"));
        default_hook(info);
    }));
}
