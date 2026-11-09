//! Đọc bài đang phát và gửi lệnh điều khiển qua API media của Windows.
//! Chạy trên một luồng nền riêng; báo cho cửa sổ bằng `WM_MEDIA_CHANGED`.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;
use windows::{
    Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    },
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize},
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
    core::Result,
};

/// Luồng media gửi thông điệp này về cửa sổ khi bài hát/trạng thái đổi
pub const WM_MEDIA_CHANGED: u32 = WM_APP + 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaCommand {
    Previous,
    TogglePlayPause,
    Next,
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
    pub playing: bool,
}

// Dữ liệu dùng chung: luồng media ghi, luồng giao diện đọc
static MEDIA: Mutex<MediaInfo> = Mutex::new(MediaInfo {
    title: String::new(),
    artist: String::new(),
    playing: false,
});

/// Bản sao thông tin media mới nhất
pub fn snapshot() -> MediaInfo {
    MEDIA.lock().unwrap().clone()
}

fn query(manager: &GlobalSystemMediaTransportControlsSessionManager) -> Result<MediaInfo> {
    let session = manager.GetCurrentSession()?;
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
    Ok(MediaInfo {
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        playing: status == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing,
    })
}

fn run_command(manager: &GlobalSystemMediaTransportControlsSessionManager, cmd: MediaCommand) {
    if let Ok(session) = manager.GetCurrentSession() {
        let op = match cmd {
            MediaCommand::Previous => session.TrySkipPreviousAsync(),
            MediaCommand::TogglePlayPause => session.TryTogglePlayPauseAsync(),
            MediaCommand::Next => session.TrySkipNextAsync(),
        };
        let _ = op.and_then(|op| op.join());
    }
}

/// Vòng lặp của luồng media: nhận lệnh từ giao diện, hỏi trạng thái mỗi giây.
/// `hwnd_raw` là HWND đổi sang số, vì HWND chứa con trỏ nên không gửi qua luồng được.
pub fn run(hwnd_raw: isize, commands: Receiver<MediaCommand>) {
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
    loop {
        match commands.recv_timeout(Duration::from_millis(1000)) {
            Ok(cmd) => {
                run_command(&manager, cmd);
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }

        let info = query(&manager).unwrap_or_default();
        if info != last {
            *MEDIA.lock().unwrap() = info.clone();
            unsafe {
                let _ = PostMessageW(Some(hwnd), WM_MEDIA_CHANGED, WPARAM(0), LPARAM(0));
            }
            last = info;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
