//! Nội dung vừa sao chép: nghe Windows báo clipboard đổi, đọc chữ / ảnh / tệp,
//! và tìm ứng dụng đã sao chép (để hiện tên và biểu tượng của nó).
//!
//! Tôn trọng quyền riêng tư: trình quản lý mật khẩu đánh dấu nội dung nhạy cảm bằng
//! hai định dạng đặc biệt; khi gặp thì không hiện nội dung (hoặc bỏ qua hẳn).

use std::path::PathBuf;

use windows::{
    Win32::{
        Foundation::{CloseHandle, GlobalFree, HANDLE, HGLOBAL, HWND},
        Graphics::Gdi::*,
        System::{
            DataExchange::*,
            Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock},
            Ole::{CF_BITMAP, CF_HDROP, CF_UNICODETEXT},
            Threading::{
                GetCurrentProcessId, OpenProcess, PROCESS_NAME_WIN32,
                PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
            },
        },
        UI::{
            Shell::{DragQueryFileW, HDROP},
            WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId},
        },
    },
    core::{PWSTR, w},
};

use crate::icon::{self, Icon};

/// Windows gửi thông điệp này khi clipboard đổi (sau khi đã đăng ký bằng `listen`)
pub const WM_CLIPBOARDUPDATE: u32 = 0x031D;
/// Ảnh vừa sao chép được thu nhỏ để vừa khung này (điểm ảnh)
const THUMBNAIL: u32 = 128;
/// Đoạn chữ xem trước dài tối đa bao nhiêu ký tự (island còn cắt ngắn hơn khi vẽ)
const PREVIEW_CHARS: usize = 200;

/// Những gì vừa được sao chép
#[derive(Debug, Clone, PartialEq)]
pub enum Copied {
    Text(String),
    /// Nội dung nhạy cảm (mật khẩu...): chỉ báo là đã sao chép, không hiện gì
    Hidden,
    Image {
        width: u32,
        height: u32,
        thumbnail: Option<Icon>,
    },
    /// Tên các tệp/thư mục
    Files(Vec<String>),
}

/// Một lần sao chép, kèm đường dẫn file .exe của ứng dụng đã sao chép (nếu biết)
#[derive(Debug, Clone)]
pub struct Clip {
    pub content: Copied,
    pub source: Option<PathBuf>,
}

/// Đăng ký nhận `WM_CLIPBOARDUPDATE` cho cửa sổ
pub fn listen(hwnd: HWND) {
    unsafe {
        let _ = AddClipboardFormatListener(hwnd);
    }
}

/// (tiêu đề, nội dung) để hiện trên island cho một lần sao chép
pub fn describe(copied: &Copied) -> (String, String) {
    match copied {
        Copied::Text(text) => ("Đã sao chép".into(), text.clone()),
        Copied::Hidden => (
            "Đã sao chép".into(),
            "Nội dung riêng tư nên không hiện".into(),
        ),
        Copied::Image { width, height, .. } => {
            ("Đã sao chép ảnh".into(), format!("{width} × {height}"))
        }
        Copied::Files(names) => {
            let title = match names.len() {
                1 => "Đã sao chép 1 tệp".to_string(),
                n => format!("Đã sao chép {n} tệp"),
            };
            (title, names.join(", "))
        }
    }
}

/// Chữ xem trước: gộp mọi khoảng trắng/xuống dòng thành một dấu cách, cắt bớt nếu dài
pub fn preview(text: &str) -> String {
    let joined = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.chars().count() > PREVIEW_CHARS {
        let cut: String = joined.chars().take(PREVIEW_CHARS).collect();
        format!("{cut}…")
    } else {
        joined
    }
}

/// Định dạng clipboard có tên riêng (do ứng dụng khác đăng ký) đang có mặt không?
fn has_format(name: &windows::core::PCWSTR) -> bool {
    unsafe {
        let id = RegisterClipboardFormatW(*name);
        id != 0 && IsClipboardFormatAvailable(id).is_ok()
    }
}

/// Đọc khối nhớ của clipboard trong lúc khóa nó
unsafe fn with_global<T>(handle: HANDLE, read: impl FnOnce(*const u8, usize) -> T) -> Option<T> {
    let global = HGLOBAL(handle.0);
    unsafe {
        let ptr = GlobalLock(global) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let value = read(ptr, GlobalSize(global));
        let _ = GlobalUnlock(global);
        Some(value)
    }
}

fn read_text() -> Option<String> {
    unsafe {
        let handle = GetClipboardData(u32::from(CF_UNICODETEXT.0)).ok()?;
        with_global(handle, |ptr, bytes| {
            let all = std::slice::from_raw_parts(ptr as *const u16, bytes / 2);
            let end = all.iter().position(|&c| c == 0).unwrap_or(all.len());
            String::from_utf16_lossy(&all[..end])
        })
    }
}

fn read_files() -> Option<Vec<String>> {
    unsafe {
        let drop = HDROP(GetClipboardData(u32::from(CF_HDROP.0)).ok()?.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        let names = (0..count)
            .map(|i| {
                let mut buf = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                let n = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
                let path = PathBuf::from(String::from_utf16_lossy(&buf[..n]));
                path.file_name().map_or_else(
                    || path.display().to_string(),
                    |f| f.to_string_lossy().into(),
                )
            })
            .collect::<Vec<_>>();
        (!names.is_empty()).then_some(names)
    }
}

/// Thu nhỏ ảnh `bitmap` (rộng `w`, cao `h`) cho vừa khung THUMBNAIL, chất lượng cao
unsafe fn thumbnail(bitmap: HBITMAP, w: u32, h: u32) -> Option<Icon> {
    let scale = (THUMBNAIL as f32 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * scale).round() as u32).max(1);
    let th = ((h as f32 * scale).round() as u32).max(1);
    unsafe {
        let screen = GetDC(None);
        let (src, dst) = (
            CreateCompatibleDC(Some(screen)),
            CreateCompatibleDC(Some(screen)),
        );
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: tw as i32,
                biHeight: -(th as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let result = CreateDIBSection(Some(dst), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            .ok()
            .map(|dib| {
                let old_src = SelectObject(src, bitmap.into());
                let old_dst = SelectObject(dst, dib.into());
                // HALFTONE: lấy trung bình nhiều điểm ảnh khi thu nhỏ, ảnh mịn không bị răng cưa
                SetStretchBltMode(dst, HALFTONE);
                let _ = SetBrushOrgEx(dst, 0, 0, None);
                let _ = StretchBlt(
                    dst,
                    0,
                    0,
                    tw as i32,
                    th as i32,
                    Some(src),
                    0,
                    0,
                    w as i32,
                    h as i32,
                    SRCCOPY,
                );
                let _ = GdiFlush();
                let mut pixels =
                    std::slice::from_raw_parts(bits as *const u8, (tw * th * 4) as usize).to_vec();
                icon::make_opaque(&mut pixels);
                SelectObject(src, old_src);
                SelectObject(dst, old_dst);
                let _ = DeleteObject(dib.into());
                Icon {
                    width: tw,
                    height: th,
                    pixels,
                }
            });
        let _ = DeleteDC(src);
        let _ = DeleteDC(dst);
        ReleaseDC(None, screen);
        result
    }
}

fn read_image() -> Option<Copied> {
    unsafe {
        let bitmap = HBITMAP(GetClipboardData(u32::from(CF_BITMAP.0)).ok()?.0);
        let mut bm = BITMAP::default();
        let got = GetObjectW(
            bitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&raw mut bm).cast()),
        );
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (width, height) = (bm.bmWidth as u32, bm.bmHeight as u32);
        Some(Copied::Image {
            width,
            height,
            thumbnail: thumbnail(bitmap, width, height),
        })
    }
}

/// Nội dung nhạy cảm? (trình quản lý mật khẩu đặt CanIncludeInClipboardHistory = 0)
fn is_private() -> bool {
    unsafe {
        let id = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
        if id == 0 || IsClipboardFormatAvailable(id).is_err() {
            return false;
        }
        GetClipboardData(id)
            .ok()
            .and_then(|h| with_global(h, |ptr, size| size >= 4 && *(ptr as *const u32) == 0))
            .unwrap_or(false)
    }
}

/// Đường dẫn .exe của tiến trình `pid`
fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buf.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(process);
        ok.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}

/// Mã tiến trình đã sao chép: chủ clipboard, không rõ thì cửa sổ đang dùng
fn source_pid() -> Option<u32> {
    unsafe {
        let owner = GetClipboardOwner().ok().filter(|h| !h.is_invalid());
        let window = owner.unwrap_or_else(|| GetForegroundWindow());
        let mut pid = 0;
        GetWindowThreadProcessId(window, Some(&mut pid));
        (pid != 0).then_some(pid)
    }
}

/// Mở clipboard; ứng dụng khác có thể đang giữ nó trong tích tắc nên thử vài lần
fn open(hwnd: HWND) -> bool {
    for _ in 0..5 {
        if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

/// Chữ đang có trong clipboard, cho Tra cứu nhanh. None nếu không phải chữ, hoặc là nội
/// dung nhạy cảm (mật khẩu từ trình quản lý mật khẩu)
pub fn current_text(hwnd: HWND) -> Option<String> {
    if has_format(&w!("ExcludeClipboardContentFromMonitorProcessing")) || !open(hwnd) {
        return None;
    }
    let text = if is_private() { None } else { read_text() };
    unsafe {
        let _ = CloseClipboard();
    }
    text.map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// Ghi chữ vào clipboard (sao chép câu trả lời của AI). Island không tự hiện thông báo
/// cho lần sao chép này vì `read` bỏ qua nội dung do chính island sao chép
pub fn set_text(hwnd: HWND, text: &str) -> bool {
    let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let bytes = wide.len() * 2;
    if !open(hwnd) {
        return false;
    }
    let ok = unsafe {
        let _ = EmptyClipboard();
        match GlobalAlloc(GMEM_MOVEABLE, bytes) {
            Ok(global) => {
                let ptr = GlobalLock(global) as *mut u16;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(global));
                    false
                } else {
                    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                    let _ = GlobalUnlock(global);
                    // Thành công thì Windows giữ khối nhớ; thất bại thì phải tự giải phóng
                    let set = SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(global.0)));
                    if set.is_err() {
                        let _ = GlobalFree(Some(global));
                    }
                    set.is_ok()
                }
            }
            Err(_) => false,
        }
    };
    unsafe {
        let _ = CloseClipboard();
    }
    ok
}

/// Đọc lần sao chép mới nhất. None: không có gì đáng hiện, chính island sao chép,
/// hoặc ứng dụng yêu cầu không theo dõi nội dung này
pub fn read(hwnd: HWND) -> Option<Clip> {
    if has_format(&w!("ExcludeClipboardContentFromMonitorProcessing")) {
        return None;
    }
    let pid = source_pid();
    if pid == Some(unsafe { GetCurrentProcessId() }) {
        return None;
    }
    if !open(hwnd) {
        return None;
    }
    let content = if is_private() {
        Some(Copied::Hidden)
    } else if let Some(files) = read_files() {
        Some(Copied::Files(files))
    } else if let Some(text) = read_text().map(|t| preview(&t)).filter(|t| !t.is_empty()) {
        Some(Copied::Text(text))
    } else {
        read_image()
    };
    unsafe {
        let _ = CloseClipboard();
    }
    Some(Clip {
        content: content?,
        source: pid.and_then(process_path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_collapses_whitespace_and_truncates() {
        assert_eq!(preview("  xin\r\n\tchào   bạn \n"), "xin chào bạn");
        let long = "a ".repeat(300);
        let p = preview(&long);
        assert_eq!(p.chars().count(), PREVIEW_CHARS + 1);
        assert!(p.ends_with('…'));
    }

    #[test]
    fn descriptions() {
        assert_eq!(
            describe(&Copied::Files(vec!["a.txt".into(), "b".into()])),
            ("Đã sao chép 2 tệp".into(), "a.txt, b".into())
        );
        let image = Copied::Image {
            width: 1920,
            height: 1080,
            thumbnail: None,
        };
        assert_eq!(describe(&image).1, "1920 × 1080");
        assert!(!describe(&Copied::Hidden).1.contains("mật khẩu"));
    }
}
