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

/// Thông điệp Windows gửi về cửa sổ khi có sự kiện chuột trên icon khay
pub const WM_TRAY: u32 = WM_APP + 2;

const TRAY_ID: u32 = 1;
const ID_SETTINGS: usize = 1;
const ID_EXIT: usize = 2;
const ID_LYRICS_COLLAPSED: usize = 3;
const ID_LOOKUP: usize = 4;

/// Mục menu cần tới trạng thái của app; main.rs làm tiếp
pub enum Action {
    OpenSettings,
    /// Bật/tắt lời bài hát trên island thu gọn (giá trị mới)
    SetLyricsCollapsed(bool),
    /// Mở hộp Tra cứu nhanh (cách mở thứ hai, khi phím tắt bị app khác chặn)
    OpenLookup,
}

pub fn add(hwnd: HWND) -> Result<()> {
    unsafe {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_TRAY,
            hIcon: crate::icon::app_icon(true),
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
/// `lyrics_collapsed`: tùy chọn lời trên island thu gọn (None = đã tắt lời bài hát).
/// `lookup_hotkey`: phím tắt của Tra cứu nhanh (None = đang tắt, không có mục này)
pub fn show_menu(
    hwnd: HWND,
    lyrics_collapsed: Option<bool>,
    lookup_hotkey: Option<&str>,
) -> Option<Action> {
    unsafe {
        let menu = CreatePopupMenu().ok()?;

        if let Some(keys) = lookup_hotkey {
            // Chữ sau dấu tab hiện căn phải, như phím tắt trong menu của Windows
            let label = HSTRING::from(format!("Tra cứu nhanh với AI\t{keys}"));
            let _ = AppendMenuW(menu, MF_STRING, ID_LOOKUP, &label);
        }

        let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, w!("Cài đặt…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        // Tùy chọn hay bật/tắt nhất được để ngay trong menu cho nhanh
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
            ID_SETTINGS => return Some(Action::OpenSettings),
            ID_LOOKUP => return Some(Action::OpenLookup),
            ID_EXIT => {
                let _ = DestroyWindow(hwnd);
            }
            _ => {}
        }
        None
    }
}
