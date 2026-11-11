//! Biểu tượng ứng dụng độ phân giải cao (64px) và tên ứng dụng, lấy thẳng từ Windows:
//! - từ file .exe (app vừa sao chép): `SHDefExtractIconW` đọc đúng bản 64px trong file;
//! - từ mã ứng dụng AppUserModelID (app gửi thông báo): hỏi thư mục ảo "Apps" của
//!   Windows (cũng là nơi menu Start lấy biểu tượng), được cả app Store lẫn app thường.
//!
//! Kết quả là mảng điểm ảnh BGRA đã nhân sẵn độ đục (premultiplied), dạng Direct2D cần.

use std::ffi::c_void;
use std::path::Path;

use windows::{
    Win32::{
        Foundation::SIZE,
        Graphics::Gdi::*,
        Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW},
        UI::{
            Shell::*,
            WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO},
        },
    },
    core::{HSTRING, PCWSTR, w},
};

/// Kích thước biểu tượng lấy ra
pub const ICON_SIZE: u32 = 64;

/// Mã icon của app trong file .exe (assets/app.rc)
const APP_ICON_ID: usize = 1;

/// Icon của chính app (con mèo trong assets/icon.ico), đúng cỡ theo DPI màn hình:
/// `small` cho khay hệ thống và góc cửa sổ, không thì cỡ lớn (Alt+Tab, taskbar).
/// Bản build thiếu icon (máy không có rc.exe) thì dùng icon mặc định của Windows
pub fn app_icon(small: bool) -> HICON {
    use windows::Win32::{
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{LIM_LARGE, LIM_SMALL, LoadIconMetric},
            WindowsAndMessaging::{IDI_APPLICATION, LoadIconW},
        },
    };
    let metric = if small { LIM_SMALL } else { LIM_LARGE };
    unsafe {
        GetModuleHandleW(None)
            .and_then(|module| {
                // MAKEINTRESOURCE: mã số đặt vào chỗ con trỏ tên
                LoadIconMetric(
                    Some(module.into()),
                    PCWSTR(APP_ICON_ID as *const u16),
                    metric,
                )
            })
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

/// Ảnh nhỏ: biểu tượng app hoặc ảnh thu nhỏ của ảnh vừa sao chép
#[derive(Clone, Debug, PartialEq)]
pub struct Icon {
    pub width: u32,
    pub height: u32,
    /// BGRA, từ trên xuống, đã nhân độ đục
    pub pixels: Vec<u8>,
}

/// Biểu tượng 64px trong file .exe
pub fn from_exe(exe: &Path) -> Option<Icon> {
    let mut large = HICON::default();
    let size = ICON_SIZE | (16 << 16); // MAKELONG(cỡ lớn, cỡ nhỏ)
    unsafe {
        SHDefExtractIconW(&HSTRING::from(exe), 0, 0, Some(&mut large), None, size)
            .ok()
            .ok()?;
        if large.is_invalid() {
            return None;
        }
        let icon = from_hicon(large);
        let _ = DestroyIcon(large);
        icon
    }
}

/// Biểu tượng 64px của ứng dụng có mã AppUserModelID `app_id`
/// (vd. "Microsoft.YourPhone_8wekyb3d8bbwe!App", "Chrome")
pub fn from_app_id(app_id: &str) -> Option<Icon> {
    unsafe {
        let item: IShellItemImageFactory = SHCreateItemInKnownFolder(
            &FOLDERID_AppsFolder,
            KF_FLAG_DEFAULT,
            &HSTRING::from(app_id),
        )
        .ok()?;
        let size = SIZE {
            cx: ICON_SIZE as i32,
            cy: ICON_SIZE as i32,
        };
        let bitmap = item.GetImage(size, SIIGBF_ICONONLY).ok()?;
        let icon = bitmap_pixels(bitmap).map(|(width, height, mut pixels)| {
            // Ảnh từ Explorer thường đã nhân độ đục sẵn; chưa thì nhân
            if all_transparent(&pixels) {
                make_opaque(&mut pixels);
            } else if !is_premultiplied(&pixels) {
                premultiply(&mut pixels);
            }
            Icon {
                width,
                height,
                pixels,
            }
        });
        let _ = DeleteObject(bitmap.into());
        icon
    }
}

/// Tên hiển thị của ứng dụng: mô tả trong file .exe ("Google Chrome"),
/// không có thì tên file bỏ đuôi ("notepad")
pub fn app_name(exe: &Path) -> String {
    file_description(exe)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            exe.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

fn file_description(exe: &Path) -> Option<String> {
    let path = HSTRING::from(exe);
    unsafe {
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, None, size, data.as_mut_ptr().cast()).ok()?;
        // Ngôn ngữ và bảng mã đầu tiên của phần mô tả
        let mut ptr: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        let found = VerQueryValueW(
            data.as_ptr().cast(),
            w!("\\VarFileInfo\\Translation"),
            &mut ptr,
            &mut len,
        );
        if !found.as_bool() || len < 4 {
            return None;
        }
        let pair = ptr as *const u16;
        let (lang, codepage) = (*pair, *pair.add(1));
        let key = format!("\\StringFileInfo\\{lang:04x}{codepage:04x}\\FileDescription");
        let found = VerQueryValueW(
            data.as_ptr().cast(),
            &HSTRING::from(key),
            &mut ptr,
            &mut len,
        );
        if !found.as_bool() || len == 0 {
            return None;
        }
        let text = std::slice::from_raw_parts(ptr as *const u16, len as usize);
        let end = text.iter().position(|&c| c == 0).unwrap_or(text.len());
        Some(String::from_utf16_lossy(&text[..end]))
    }
}

/// Đổi HICON thành điểm ảnh (biểu tượng cũ không có kênh độ đục thì dùng mặt nạ đen/trắng)
unsafe fn from_hicon(icon: HICON) -> Option<Icon> {
    let mut info = ICONINFO::default();
    unsafe {
        GetIconInfo(icon, &mut info).ok()?;
        let color = bitmap_pixels(info.hbmColor);
        let mask = bitmap_pixels(info.hbmMask);
        let _ = DeleteObject(info.hbmColor.into());
        let _ = DeleteObject(info.hbmMask.into());
        let (width, height, mut pixels) = color?;
        if all_transparent(&pixels) {
            match mask {
                Some((_, _, mask)) if mask.len() == pixels.len() => apply_mask(&mut pixels, &mask),
                _ => make_opaque(&mut pixels),
            }
        }
        premultiply(&mut pixels);
        Some(Icon {
            width,
            height,
            pixels,
        })
    }
}

/// Điểm ảnh BGRA 32 bit (từ trên xuống) của một HBITMAP
pub unsafe fn bitmap_pixels(bitmap: HBITMAP) -> Option<(u32, u32, Vec<u8>)> {
    if bitmap.is_invalid() {
        return None;
    }
    unsafe {
        let mut bm = BITMAP::default();
        let got = GetObjectW(
            bitmap.into(),
            std::mem::size_of::<BITMAP>() as i32,
            Some((&raw mut bm).cast()),
        );
        if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32), // số âm: hàng đầu tiên là hàng trên cùng
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let hdc = GetDC(None);
        let lines = GetDIBits(
            hdc,
            bitmap,
            0,
            h,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        ReleaseDC(None, hdc);
        (lines == h as i32).then_some((w, h, pixels))
    }
}

/// Mọi điểm ảnh có độ đục 0 (ảnh không dùng kênh độ đục)
pub fn all_transparent(bgra: &[u8]) -> bool {
    bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 0)
}

pub fn make_opaque(bgra: &mut [u8]) {
    for p in bgra.as_chunks_mut::<4>().0 {
        p[3] = 0xFF;
    }
}

/// Mặt nạ của biểu tượng cũ: điểm trắng là trong suốt, điểm đen là có hình
fn apply_mask(bgra: &mut [u8], mask: &[u8]) {
    for (p, m) in bgra
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(mask.as_chunks::<4>().0)
    {
        p[3] = if m[0] == 0 { 0xFF } else { 0 };
    }
}

/// Đã nhân độ đục chưa? (ảnh đã nhân thì không kênh màu nào lớn hơn độ đục)
fn is_premultiplied(bgra: &[u8]) -> bool {
    bgra.as_chunks::<4>()
        .0
        .iter()
        .all(|p| p[..3].iter().all(|&c| c <= p[3]))
}

fn premultiply(bgra: &mut [u8]) {
    for p in bgra.as_chunks_mut::<4>().0 {
        let a = u16::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u16::from(*c) * a + 127) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply_scales_colors_by_alpha() {
        let mut p = vec![200, 100, 50, 128, 255, 255, 255, 255, 9, 9, 9, 0];
        premultiply(&mut p);
        assert_eq!(p, vec![100, 50, 25, 128, 255, 255, 255, 255, 0, 0, 0, 0]);
        assert!(is_premultiplied(&p));
        assert!(!is_premultiplied(&[200, 0, 0, 100]));
    }

    #[test]
    fn mask_marks_black_as_visible() {
        let mut p = vec![1, 2, 3, 0, 4, 5, 6, 0];
        assert!(all_transparent(&p));
        apply_mask(&mut p, &[0, 0, 0, 0, 255, 255, 255, 0]);
        assert_eq!((p[3], p[7]), (255, 0));
    }

    /// Thăm dò: lấy được biểu tượng 64px của Notepad và của một app Store không?
    /// cargo test probe_icons -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_icons() {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_MULTITHREADED,
            );
        }
        let exe = Path::new(r"C:\Windows\System32\notepad.exe");
        let icon = from_exe(exe).map(|i| (i.width, i.height));
        println!("{} {icon:?}", app_name(exe));
        let id = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App";
        println!("{id}: {:?}", from_app_id(id).map(|i| (i.width, i.height)));
        for id in [
            "Microsoft.YourPhone_8wekyb3d8bbwe!App",
            "Claude_pzs8sxrjxfjjc!Claude",
            "Chrome",
            "MSEdge",
        ] {
            println!("{id}: {:?}", from_app_id(id).map(|i| (i.width, i.height)));
        }
    }
}
