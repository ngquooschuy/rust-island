//! Icon ở khay hệ thống và menu chuột phải của nó.

use windows::{
    Win32::{
        Foundation::{HWND, POINT},
        UI::{
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
                Shell_NotifyIconW,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, Result, w},
};

use crate::{autostart, config, lyrics};

/// Thông điệp Windows gửi về cửa sổ khi có sự kiện chuột trên icon khay
pub const WM_TRAY: u32 = WM_APP + 2;

const TRAY_ID: u32 = 1;
const ID_AUTOSTART: usize = 1;
const ID_EXIT: usize = 2;
const ID_OPEN_CONFIG: usize = 3;
const ID_RESTART: usize = 4;
const ID_CLEAR_LYRICS: usize = 5;
const ID_LYRICS_COLLAPSED: usize = 6;

/// Mục menu cần trạng thái của island; main.rs làm tiếp
pub enum Action {
    /// Bật/tắt lời bài hát trên island thu gọn (giá trị mới)
    SetLyricsCollapsed(bool),
}

/// Mở một bản mới của chương trình rồi đóng bản hiện tại,
/// để bản mới đọc lại file cấu hình
fn restart(hwnd: HWND) {
    let started = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn());
    if started.is_ok() {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

pub fn add(hwnd: HWND) -> Result<()> {
    unsafe {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: LoadIconW(None, IDI_APPLICATION)?, // icon mặc định của Windows
            ..Default::default()
        };
        // szTip là mảng 128 số u16 cố định: chép chuỗi UTF-16 vào đầu mảng
        let tip: Vec<u16> = "Dynamic Island".encode_utf16().collect();
        nid.szTip[..tip.len()].copy_from_slice(&tip);
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }
    Ok(())
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }
}

/// Hiện menu chuột phải và xử lý mục được chọn.
/// `lyrics_collapsed`: tùy chọn lời trên island thu gọn (None = đã tắt lời bài hát)
pub fn show_menu(hwnd: HWND, lyrics_collapsed: Option<bool>) -> Option<Action> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;

        let autostart_flags = if autostart::is_enabled() {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(
            menu,
            autostart_flags,
            ID_AUTOSTART,
            w!("Khởi động cùng Windows"),
        );
        let _ = AppendMenuW(menu, MF_STRING, ID_OPEN_CONFIG, w!("Mở file cấu hình"));
        let _ = AppendMenuW(menu, MF_STRING, ID_RESTART, w!("Khởi động lại để áp dụng"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let collapsed_flags = match lyrics_collapsed {
            Some(true) => MF_STRING | MF_CHECKED,
            Some(false) => MF_STRING,
            None => MF_STRING | MF_GRAYED,
        };
        let _ = AppendMenuW(
            menu,
            collapsed_flags,
            ID_LYRICS_COLLAPSED,
            w!("Hiện lời bài hát trên island thu gọn"),
        );
        // Bộ nhớ đệm lời bài hát: hiện số bài và dung lượng; trống hoặc đang tắt thì làm mờ
        let (label, flags) = match lyrics::cache_summary() {
            Some((count, summary)) => {
                let flags = if count == 0 {
                    MF_STRING | MF_GRAYED
                } else {
                    MF_STRING
                };
                (format!("Xóa bộ nhớ đệm lời bài hát ({summary})"), flags)
            }
            None => (
                "Bộ nhớ đệm lời bài hát: đang tắt".to_string(),
                MF_STRING | MF_GRAYED,
            ),
        };
        let _ = AppendMenuW(menu, flags, ID_CLEAR_LYRICS, &HSTRING::from(label));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, w!("Thoát"));

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        // Bắt buộc để menu tự đóng khi bấm ra ngoài
        let _ = SetForegroundWindow(hwnd);
        // TPM_RETURNCMD: trả về luôn mã mục được chọn (0 nếu hủy)
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);

        match cmd.0 as usize {
            ID_LYRICS_COLLAPSED => {
                let on = !lyrics_collapsed.unwrap_or(true);
                return Some(Action::SetLyricsCollapsed(on));
            }
            ID_AUTOSTART => autostart::set(!autostart::is_enabled()),
            ID_OPEN_CONFIG => config::open_in_editor(),
            ID_RESTART => restart(hwnd),
            ID_CLEAR_LYRICS => lyrics::clear_cache(),
            ID_EXIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
        None
    }
}
