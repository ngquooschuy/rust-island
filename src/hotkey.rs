//! Phím tắt toàn cục mở Tra cứu nhanh, ví dụ "Ctrl+Alt+`".
//! Trong config.toml ghi dạng chữ; Windows nhận dạng (tổ hợp phím bổ trợ, mã phím ảo).
//!
//! Nhận phím bằng hook bàn phím cấp thấp (WH_KEYBOARD_LL), giống PowerToys, AutoHotkey.
//! Không dùng `RegisterHotKey` làm cách chính: bộ gõ tiếng Việt, overlay của card đồ họa...
//! cũng hook bàn phím và chạy trước phần xử lý phím tắt của Windows, nên có máy bấm phím
//! tắt mà app không bao giờ nhận được. Hook cài sau được Windows gọi trước, nên island thấy
//! tổ hợp phím trước các app đó; thấy đúng tổ hợp thì "nuốt" luôn để nó không gõ ra chữ ở
//! app khác. Hook không cài được thì mới quay về `RegisterHotKey`.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU8, AtomicU32, Ordering};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Input::KeyboardAndMouse::{
            GetAsyncKeyState, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, MOD_SHIFT,
            MOD_WIN, RegisterHotKey, UnregisterHotKey, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN,
            VK_SHIFT,
        },
        WindowsAndMessaging::{
            CallNextHookEx, HC_ACTION, HHOOK, KBDLLHOOKSTRUCT, PostMessageW, SetWindowsHookExW,
            UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_HOTKEY, WM_KEYDOWN, WM_SYSKEYDOWN,
        },
    },
};

/// Mã của phím tắt khi đăng ký với Windows (WM_HOTKEY mang mã này trong wparam)
pub const LOOKUP_HOTKEY_ID: i32 = 1;

/// Phím tắt mặc định
pub const DEFAULT: &str = "Ctrl+Alt+`";

/// Một tổ hợp phím
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hotkey {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub win: bool,
    /// Mã phím ảo của Windows (VK_*)
    pub vk: u32,
}

/// Tên phím (như ghi trong file) và mã phím ảo
const NAMED_KEYS: &[(&str, u32)] = &[
    ("`", 0xC0),
    ("-", 0xBD),
    ("=", 0xBB),
    ("[", 0xDB),
    ("]", 0xDD),
    ("\\", 0xDC),
    (";", 0xBA),
    ("'", 0xDE),
    (",", 0xBC),
    (".", 0xBE),
    ("/", 0xBF),
    ("Space", 0x20),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
    ("Home", 0x24),
    ("End", 0x23),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("Up", 0x26),
    ("Down", 0x28),
    ("Left", 0x25),
    ("Right", 0x27),
];

fn key_name(vk: u32) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from),
        0x70..=0x87 => Some(format!("F{}", vk - 0x70 + 1)),
        _ => NAMED_KEYS
            .iter()
            .find(|(_, v)| *v == vk)
            .map(|(n, _)| n.to_string()),
    }
}

fn key_code(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    let mut chars = upper.chars();
    if let (Some(c), None) = (chars.next(), chars.next())
        && c.is_ascii_alphanumeric()
    {
        return Some(c as u32);
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x70 + n - 1);
    }
    NAMED_KEYS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, v)| v)
}

impl Hotkey {
    /// Đọc "Ctrl+Alt+`" (không phân biệt hoa thường, bỏ khoảng trắng). Phải có Ctrl, Alt
    /// hoặc Win, để không chiếm mất một phím đang dùng để gõ chữ
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut key = Self {
            ctrl: false,
            alt: false,
            shift: false,
            win: false,
            vk: 0,
        };
        let parts: Vec<&str> = text.split('+').map(str::trim).collect();
        let (last, modifiers) = parts
            .split_last()
            .filter(|(l, _)| !l.is_empty())
            .ok_or("Phím tắt trống")?;
        for m in modifiers {
            match m.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => key.ctrl = true,
                "alt" => key.alt = true,
                "shift" => key.shift = true,
                "win" | "windows" => key.win = true,
                other => return Err(format!("Không hiểu phím bổ trợ \"{other}\"")),
            }
        }
        key.vk = key_code(last).ok_or_else(|| format!("Không hiểu phím \"{last}\""))?;
        if !(key.ctrl || key.alt || key.win) {
            return Err("Phím tắt cần có Ctrl, Alt hoặc Win".to_string());
        }
        Ok(key)
    }

    fn modifiers(self) -> HOT_KEY_MODIFIERS {
        let mut m = MOD_NOREPEAT; // giữ phím không lặp lại liên tục
        for (on, flag) in [
            (self.ctrl, MOD_CONTROL),
            (self.alt, MOD_ALT),
            (self.shift, MOD_SHIFT),
            (self.win, MOD_WIN),
        ] {
            if on {
                m |= flag;
            }
        }
        m
    }

    /// Từ ô chọn phím tắt của Windows (HKM_GETHOTKEY): byte thấp là mã phím,
    /// byte cao là HOTKEYF_SHIFT (1) | HOTKEYF_CONTROL (2) | HOTKEYF_ALT (4)
    pub fn from_hotkey_control(value: u16) -> Option<Self> {
        let vk = u32::from(value & 0xFF);
        let flags = value >> 8;
        let key = Self {
            shift: flags & 1 != 0,
            ctrl: flags & 2 != 0,
            alt: flags & 4 != 0,
            win: false,
            vk,
        };
        (vk != 0 && key_name(vk).is_some() && (key.ctrl || key.alt)).then_some(key)
    }

    /// Giá trị cho ô chọn phím tắt của Windows (HKM_SETHOTKEY)
    pub fn to_hotkey_control(self) -> u16 {
        let flags = u16::from(self.shift) | u16::from(self.ctrl) << 1 | u16::from(self.alt) << 2;
        (self.vk as u16 & 0xFF) | flags << 8
    }
}

impl fmt::Display for Hotkey {
    /// "Ctrl+Alt+`"
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // Thứ tự quen thuộc trên Windows: Win+Shift+S, Ctrl+Alt+Delete
        for (on, name) in [
            (self.win, "Win"),
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
        ] {
            if on {
                write!(f, "{name}+")?;
            }
        }
        write!(f, "{}", key_name(self.vk).unwrap_or_else(|| "?".into()))
    }
}

impl Hotkey {
    /// Gói tổ hợp vào một số (để hook đọc được mà không cần khóa): 8 bit thấp là mã phím,
    /// các bit sau là Ctrl, Alt, Shift, Win. 0 = không có phím tắt
    fn packed(self) -> u32 {
        (self.vk & 0xFF)
            | u32::from(self.ctrl) << 8
            | u32::from(self.alt) << 9
            | u32::from(self.shift) << 10
            | u32::from(self.win) << 11
    }
}

/// Phím bổ trợ đang giữ, cùng cách gói với `Hotkey::packed`
fn modifiers_down() -> u32 {
    let down = |vk: u16| unsafe { GetAsyncKeyState(i32::from(vk)) } < 0;
    u32::from(down(VK_CONTROL.0)) << 8
        | u32::from(down(VK_MENU.0)) << 9
        | u32::from(down(VK_SHIFT.0)) << 10
        | u32::from(down(VK_LWIN.0) || down(VK_RWIN.0)) << 11
}

/// Hook là hàm `extern "system"` không nhận tham số riêng: tổ hợp cần bắt, cửa sổ nhận,
/// handle của hook, và phím chính có đang bị giữ không (giữ phím thì Windows lặp lại
/// keydown liên tục: chỉ mở một lần, và nuốt cả lúc nhả)
static HOOK_KEY: AtomicU32 = AtomicU32::new(0);
static HOOK_TARGET: AtomicIsize = AtomicIsize::new(0);
static HOOK_HANDLE: AtomicIsize = AtomicIsize::new(0);
static HOOK_HELD: AtomicBool = AtomicBool::new(false);
/// Bản debug: đã ghi nhật ký "hook đang nhận phím" chưa (chỉ ghi một lần)
static HOOK_SEEN: AtomicBool = AtomicBool::new(false);

/// "Ctrl+Alt" từ phần phím bổ trợ của số gói
fn modifier_names(packed: u32) -> String {
    let names: Vec<&str> = [(8, "Ctrl"), (9, "Alt"), (10, "Shift"), (11, "Win")]
        .iter()
        .filter(|(bit, _)| packed >> bit & 1 == 1)
        .map(|&(_, n)| n)
        .collect();
    if names.is_empty() {
        "không có".into()
    } else {
        names.join("+")
    }
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let key = HOOK_KEY.load(Ordering::Relaxed);
    if code == HC_ACTION as i32 && key != 0 {
        let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if cfg!(debug_assertions) && !HOOK_SEEN.swap(true, Ordering::Relaxed) {
            crate::trace::log("hook bàn phím đang nhận phím");
        }
        if kb.vkCode == key & 0xFF {
            let msg = wparam.0 as u32;
            if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
                if HOOK_HELD.load(Ordering::Relaxed) {
                    return LRESULT(1);
                }
                // Bản debug: chỉ ghi đúng phím của phím tắt, kèm phím bổ trợ đang giữ
                crate::trace::log(format!(
                    "hook thấy phím {:#x}, đang giữ: {}, cần: {}",
                    kb.vkCode,
                    modifier_names(modifiers_down()),
                    modifier_names(key)
                ));
                if modifiers_down() == key & !0xFF {
                    HOOK_HELD.store(true, Ordering::Relaxed);
                    let target = HWND(HOOK_TARGET.load(Ordering::Relaxed) as *mut std::ffi::c_void);
                    unsafe {
                        let _ = PostMessageW(
                            Some(target),
                            WM_HOTKEY,
                            WPARAM(LOOKUP_HOTKEY_ID as usize),
                            LPARAM(0),
                        );
                    }
                    return LRESULT(1);
                }
            } else if HOOK_HELD.swap(false, Ordering::Relaxed) {
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Cài hook bàn phím (nếu chưa có) để bắt tổ hợp `key`, báo cho cửa sổ `hwnd`
fn install_hook(hwnd: HWND, key: Hotkey) -> bool {
    HOOK_TARGET.store(hwnd.0 as isize, Ordering::Relaxed);
    HOOK_KEY.store(key.packed(), Ordering::Relaxed);
    if HOOK_HANDLE.load(Ordering::Relaxed) != 0 {
        return true;
    }
    let hook = unsafe {
        let module = GetModuleHandleW(None).ok();
        SetWindowsHookExW(
            WH_KEYBOARD_LL,
            Some(keyboard_proc),
            module.map(Into::into),
            0,
        )
    };
    match hook {
        Ok(h) => {
            HOOK_HANDLE.store(h.0 as isize, Ordering::Relaxed);
            true
        }
        Err(_) => {
            HOOK_KEY.store(0, Ordering::Relaxed);
            false
        }
    }
}

/// Trạng thái đăng ký phím tắt, để cửa sổ Cài đặt báo cho người dùng
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Tra cứu nhanh đang tắt
    Off,
    Registered,
    /// Ứng dụng khác đã dùng tổ hợp này, hoặc chữ trong file không hợp lệ
    Failed,
}

static STATUS: AtomicU8 = AtomicU8::new(0);

pub fn status() -> Status {
    match STATUS.load(Ordering::Relaxed) {
        1 => Status::Registered,
        2 => Status::Failed,
        _ => Status::Off,
    }
}

/// Đăng ký phím tắt mở Tra cứu nhanh cho cửa sổ island: hook bàn phím, không được thì
/// `RegisterHotKey`. Cả hai cách đều gửi WM_HOTKEY (mã `LOOKUP_HOTKEY_ID`) về cửa sổ
pub fn register(hwnd: HWND, text: &str) -> Status {
    let status = match Hotkey::parse(text) {
        Ok(key) if install_hook(hwnd, key) => Status::Registered,
        Ok(key) => {
            let ok = unsafe {
                RegisterHotKey(Some(hwnd), LOOKUP_HOTKEY_ID, key.modifiers(), key.vk).is_ok()
            };
            if ok {
                Status::Registered
            } else {
                Status::Failed
            }
        }
        Err(_) => Status::Failed,
    };
    STATUS.store(
        match status {
            Status::Off => 0,
            Status::Registered => 1,
            Status::Failed => 2,
        },
        Ordering::Relaxed,
    );
    status
}

pub fn unregister(hwnd: HWND) {
    HOOK_KEY.store(0, Ordering::Relaxed);
    let hook = HOOK_HANDLE.swap(0, Ordering::Relaxed);
    unsafe {
        if hook != 0 {
            let _ = UnhookWindowsHookEx(HHOOK(hook as *mut std::ffi::c_void));
        }
        let _ = UnregisterHotKey(Some(hwnd), LOOKUP_HOTKEY_ID);
    }
    STATUS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_hotkey_is_ctrl_alt_backtick() {
        let k = Hotkey::parse(DEFAULT).unwrap();
        assert!(k.ctrl && k.alt && !k.shift && !k.win);
        assert_eq!(k.vk, 0xC0);
        assert_eq!(k.to_string(), "Ctrl+Alt+`");
    }

    #[test]
    fn parses_common_hotkeys_and_round_trips() {
        for text in [
            "Ctrl+Shift+Space",
            "Alt+F2",
            "Win+Shift+K",
            "Ctrl+Alt+1",
            "Ctrl+Alt+/",
        ] {
            assert_eq!(Hotkey::parse(text).unwrap().to_string(), text);
        }
        // Viết thường, có khoảng trắng
        assert_eq!(
            Hotkey::parse(" ctrl + alt + k ").unwrap().to_string(),
            "Ctrl+Alt+K"
        );
    }

    #[test]
    fn rejects_unusable_hotkeys() {
        assert!(Hotkey::parse("").is_err());
        assert!(Hotkey::parse("Ctrl+").is_err());
        assert!(Hotkey::parse("K").is_err()); // thiếu phím bổ trợ: sẽ chiếm phím gõ chữ
        assert!(Hotkey::parse("Shift+K").is_err());
        assert!(Hotkey::parse("Ctrl+Hyper+K").is_err());
        assert!(Hotkey::parse("Ctrl+F25").is_err());
    }

    #[test]
    fn packed_hotkey_matches_modifier_layout() {
        let k = Hotkey::parse("Ctrl+Alt+`").unwrap();
        assert_eq!(k.packed(), 0xC0 | 1 << 8 | 1 << 9);
        assert_eq!(k.packed() & 0xFF, 0xC0);
        assert_eq!(Hotkey::parse("Win+Shift+K").unwrap().packed() >> 8, 0b1100);
    }

    #[test]
    fn hotkey_control_values_round_trip() {
        let k = Hotkey::parse("Ctrl+Alt+`").unwrap();
        let v = k.to_hotkey_control();
        assert_eq!(v, 0xC0 | (6 << 8));
        assert_eq!(Hotkey::from_hotkey_control(v), Some(k));
        // Chưa chọn gì, hoặc không có Ctrl/Alt: không nhận
        assert_eq!(Hotkey::from_hotkey_control(0), None);
        assert_eq!(Hotkey::from_hotkey_control(0x4B | (1 << 8)), None);
    }
}
