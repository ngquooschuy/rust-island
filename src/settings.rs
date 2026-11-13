//! Cửa sổ Cài đặt: chỉnh mọi tùy chọn bằng giao diện, không cần mở file config.toml.
//! Bấm "Lưu" thì ghi file rồi khởi động lại island để áp dụng.
//!
//! Dùng control có sẵn của Windows (ô đánh dấu, danh sách chọn, thanh trượt...).
//! Mọi control là con trực tiếp của cửa sổ và được tìm lại bằng mã số (`GetDlgItem`),
//! nên trạng thái riêng chỉ còn vài giá trị nhỏ, `Copy` được, cất trong một `Cell`.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Controls::{Dialogs::*, *},
            HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow},
            Input::KeyboardAndMouse::EnableWindow,
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::config::{self, BackdropKind, Color, Config};
use crate::{ai, autostart, lyrics, secret};

// Mã số control. Lưu = IDOK và Hủy = IDCANCEL để phím Enter / Esc tự bấm hai nút đó
const ID_SAVE: i32 = 1;
const ID_CANCEL: i32 = 2;
const ID_DEFAULTS: i32 = 3;
const ID_BACKDROP: i32 = 10;
const ID_OPACITY: i32 = 11;
const ID_OPACITY_VALUE: i32 = 12;
/// Bốn ô màu liền nhau: nền, chữ, chữ phụ, nhấn
const ID_COLOR: i32 = 13;
const ID_FONT: i32 = 20;
const ID_TOP: i32 = 21;
const ID_COLLAPSED_W: i32 = 22;
const ID_COLLAPSED_W_VALUE: i32 = 23;
const ID_EXPANDED_W: i32 = 24;
const ID_EXPANDED_W_VALUE: i32 = 25;
const ID_HOUR24: i32 = 30;
const ID_SECONDS: i32 = 31;
const ID_AUTOSTART: i32 = 40;
const ID_AUTOHIDE: i32 = 41;
const ID_AUTOHIDE_DELAY: i32 = 42;
const ID_STAY_PLAYING: i32 = 43;
const ID_WEATHER: i32 = 50;
const ID_CITY: i32 = 51;
const ID_FAHRENHEIT: i32 = 52;
const ID_LYRICS: i32 = 60;
const ID_LYRICS_SHOW: i32 = 61;
const ID_LYRICS_COLLAPSED: i32 = 62;
const ID_CACHE: i32 = 63;
const ID_CLEAR_CACHE: i32 = 64;
const ID_ALERT_CLIPBOARD: i32 = 70;
const ID_ALERT_NOTIFICATIONS: i32 = 71;
const ID_ALERT_SECONDS: i32 = 72;
const ID_OFFSET_SONG: i32 = 80;
const ID_OFFSET: i32 = 81;
const ID_OFFSET_VALUE: i32 = 82;
const ID_OFFSET_RESET: i32 = 83;
const ID_AI_MODEL: i32 = 93;
const ID_AI_ACCOUNT: i32 = 94;
const ID_AI_TOKEN: i32 = 95;
const ID_AI_TOKEN_CLEAR: i32 = 96;
const ID_AI_PROMPT: i32 = 97;
const ID_AI_TEST: i32 = 98;
const ID_AI_TEST_RESULT: i32 = 99;

/// Luồng "Kiểm tra kết nối" gửi thông điệp này về cửa sổ Cài đặt khi có kết quả
const WM_AI_TEST_DONE: u32 = WM_APP + 20;
/// Chữ mờ gợi ý trong ô nhập (EM_SETCUEBANNER)
const EM_SETCUEBANNER: u32 = 0x1501;

/// Thanh trượt chỉnh lời: mỗi nấc nửa giây, vạch chia mỗi 10 giây
const OFFSET_STEP_MS: i64 = 500;
const OFFSET_TICK_EVERY: isize = 20;
/// Timer kiểm tra bài đang phát đã đổi chưa (để thanh trượt luôn ứng với bài hiện tại)
const TIMER_NOW_PLAYING: usize = 1;
// Thông điệp của thanh trượt Windows (TBM_*) dùng tới mà crate windows chưa có tên sẵn
const TBM_SETRANGEMIN: u32 = WM_USER + 7;
const TBM_SETRANGEMAX: u32 = WM_USER + 8;
const TBM_SETTICFREQ: u32 = WM_USER + 20;
const TBM_SETPAGESIZE: u32 = WM_USER + 21;
/// Mã trong WM_HSCROLL báo người dùng đã kéo/bấm xong (thả chuột, nhả phím)
const TB_ENDTRACK: usize = 8;

/// Kích thước vùng trong cửa sổ, ở tỉ lệ 100% (96 DPI): ba cột, cột phải là Trợ lý AI
const CLIENT: (i32, i32) = (956, 650);

const BACKDROPS: [(BackdropKind, &str); 3] = [
    (BackdropKind::None, "Không"),
    (BackdropKind::Blur, "Làm mờ (Blur)"),
    (BackdropKind::Acrylic, "Kính mờ (Acrylic)"),
];
const CACHE_SIZES: [u32; 4] = [0, 10, 25, 100];
/// Chọn Blur/Acrylic khi nền đang đục hẳn thì tự giảm độ đục xuống mức này, để thấy kính
const OPACITY_FOR_BACKDROP: u32 = 60;

/// Vài giá trị cửa sổ cần nhớ giữa các thông điệp
#[derive(Clone, Copy)]
struct State {
    hwnd: HWND,
    /// Cửa sổ island, để khởi động lại sau khi lưu
    island: HWND,
    font: HFONT,
    /// Màu nền, chữ, chữ phụ, nhấn đang chọn (độ đục của nền lấy từ thanh trượt)
    colors: [Color; 4],
}

thread_local! {
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
    /// Bài mà thanh trượt chỉnh lời đang ứng với (tên + nghệ sĩ), để biết khi nào bài đổi
    static OFFSET_SONG: RefCell<String> = const { RefCell::new(String::new()) };
    /// 16 màu tự chọn trong hộp chọn màu của Windows, nhớ trong lúc app chạy
    static CUSTOM_COLORS: Cell<[COLORREF; 16]> = const { Cell::new([COLORREF(0xFFFFFF); 16]) };
}

/// Kết quả "Kiểm tra kết nối", chờ cửa sổ Cài đặt lấy
static TEST_RESULT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Mở cửa sổ Cài đặt (đang mở rồi thì đưa lên trước)
pub fn open(island: HWND) {
    if let Some(s) = STATE.get() {
        unsafe {
            let _ = ShowWindow(s.hwnd, SW_RESTORE);
            let _ = SetForegroundWindow(s.hwnd);
        }
        return;
    }
    let _ = create(island);
}

/// Cửa sổ Cài đặt đang mở? (khi đó island coi như đang được dùng, không tự thu nhỏ)
pub fn is_open() -> bool {
    STATE.get().is_some()
}

/// Vòng lặp thông điệp gọi trước khi xử lý mỗi thông điệp: cho phím Tab, Enter, Esc
/// hoạt động trong cửa sổ Cài đặt như một hộp thoại. true = đã xử lý xong
pub fn pre_translate(msg: &MSG) -> bool {
    STATE
        .get()
        .is_some_and(|s| unsafe { IsDialogMessageW(s.hwnd, msg).as_bool() })
}

// ---------- Đọc/ghi control ----------

fn item(hwnd: HWND, id: i32) -> HWND {
    unsafe { GetDlgItem(Some(hwnd), id).unwrap_or_default() }
}

fn send(hwnd: HWND, id: i32, msg: u32, wparam: usize, lparam: isize) -> isize {
    unsafe {
        SendMessageW(
            item(hwnd, id),
            msg,
            Some(WPARAM(wparam)),
            Some(LPARAM(lparam)),
        )
        .0
    }
}

fn checked(hwnd: HWND, id: i32) -> bool {
    send(hwnd, id, BM_GETCHECK, 0, 0) == 1
}

fn set_checked(hwnd: HWND, id: i32, on: bool) {
    send(hwnd, id, BM_SETCHECK, usize::from(on), 0);
}

fn text(hwnd: HWND, id: i32) -> String {
    let control = item(hwnd, id);
    unsafe {
        let mut buf = vec![0u16; GetWindowTextLengthW(control) as usize + 1];
        let n = GetWindowTextW(control, &mut buf);
        String::from_utf16_lossy(&buf[..n.max(0) as usize])
    }
}

fn set_text(hwnd: HWND, id: i32, s: &str) {
    unsafe {
        let _ = SetWindowTextW(item(hwnd, id), &HSTRING::from(s));
    }
}

fn enable(hwnd: HWND, id: i32, on: bool) {
    unsafe {
        let _ = EnableWindow(item(hwnd, id), on);
    }
}

fn slider(hwnd: HWND, id: i32) -> u32 {
    send(hwnd, id, WM_USER, 0, 0).max(0) as u32 // WM_USER = TBM_GETPOS
}

fn set_slider(hwnd: HWND, id: i32, value: u32) {
    send(hwnd, id, TBM_SETPOS, 1, value as isize);
}

/// Thêm một dòng vào danh sách chọn, kèm số `data` (vd. số MB); trả về vị trí dòng
fn combo_add(hwnd: HWND, id: i32, label: &str, data: usize) -> usize {
    let label = HSTRING::from(label);
    let index = send(hwnd, id, CB_ADDSTRING, 0, label.as_ptr() as isize).max(0) as usize;
    send(hwnd, id, CB_SETITEMDATA, index, data as isize);
    index
}

fn combo_selected(hwnd: HWND, id: i32) -> Option<usize> {
    usize::try_from(send(hwnd, id, CB_GETCURSEL, 0, 0)).ok()
}

fn combo_select(hwnd: HWND, id: i32, index: usize) {
    send(hwnd, id, CB_SETCURSEL, index, 0);
}

/// Chọn dòng có số `data`; không có thì thêm dòng mới với nhãn `label`
fn combo_select_data(hwnd: HWND, id: i32, data: usize, label: &str) {
    let count = send(hwnd, id, CB_GETCOUNT, 0, 0).max(0) as usize;
    let found = (0..count).find(|&i| send(hwnd, id, CB_GETITEMDATA, i, 0) as usize == data);
    let index = found.unwrap_or_else(|| combo_add(hwnd, id, label, data));
    combo_select(hwnd, id, index);
}

fn combo_data(hwnd: HWND, id: i32) -> Option<usize> {
    combo_selected(hwnd, id).map(|i| send(hwnd, id, CB_GETITEMDATA, i, 0) as usize)
}

// ---------- Màu ----------

fn colorref(c: Color) -> COLORREF {
    COLORREF(u32::from(c.r) | u32::from(c.g) << 8 | u32::from(c.b) << 16)
}

/// Độ đục 0–255 ↔ phần trăm 0–100 trên thanh trượt
fn alpha_to_percent(a: u8) -> u32 {
    (u32::from(a) * 100 + 127) / 255
}

fn percent_to_alpha(p: u32) -> u8 {
    ((p.min(100) * 255 + 50) / 100) as u8
}

/// Hộp chọn màu của Windows cho ô màu thứ `i`
fn pick_color(hwnd: HWND, i: usize) {
    let Some(state) = STATE.get() else { return };
    let mut custom = CUSTOM_COLORS.get();
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: hwnd,
        rgbResult: colorref(state.colors[i]),
        lpCustColors: custom.as_mut_ptr(),
        Flags: CC_RGBINIT | CC_FULLOPEN,
        ..Default::default()
    };
    // Hộp chọn màu chạy vòng lặp riêng; không giữ gì đang mượn trong lúc đó
    if !unsafe { ChooseColorW(&mut cc) }.as_bool() {
        return;
    }
    CUSTOM_COLORS.set(custom);
    let Some(mut state) = STATE.get() else { return };
    let rgb = cc.rgbResult.0;
    let old = state.colors[i];
    state.colors[i] = Color::rgba(rgb as u8, (rgb >> 8) as u8, (rgb >> 16) as u8, old.a);
    STATE.set(Some(state));
    unsafe {
        let _ = InvalidateRect(Some(item(hwnd, ID_COLOR + i as i32)), None, true);
    }
}

/// Vẽ ô màu (nút tự vẽ): khung xám, ruột là màu đang chọn
fn draw_swatch(d: &DRAWITEMSTRUCT) {
    let Some(state) = STATE.get() else { return };
    let i = (d.CtlID as i32 - ID_COLOR) as usize;
    let Some(&color) = state.colors.get(i) else {
        return;
    };
    let mut r = d.rcItem;
    unsafe {
        FillRect(d.hDC, &r, GetSysColorBrush(COLOR_BTNFACE));
        let _ = InflateRect(&mut r, -1, -1);
        let border = CreateSolidBrush(COLORREF(0x7A7A7A));
        FrameRect(d.hDC, &r, border);
        let _ = DeleteObject(border.into());
        let _ = InflateRect(&mut r, -2, -2);
        let fill = CreateSolidBrush(colorref(color));
        FillRect(d.hDC, &r, fill);
        let _ = DeleteObject(fill.into());
        if d.itemState.0 & ODS_FOCUS.0 != 0 {
            let mut focus = d.rcItem;
            let _ = InflateRect(&mut focus, -1, -1);
            let _ = DrawFocusRect(d.hDC, &focus);
        }
    }
}

// ---------- Font ----------

unsafe extern "system" fn collect_font(
    lf: *const LOGFONTW,
    _: *const TEXTMETRICW,
    _: u32,
    names: LPARAM,
) -> i32 {
    let (lf, names) = unsafe { (&*lf, &mut *(names.0 as *mut Vec<String>)) };
    let len = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
    let name = String::from_utf16_lossy(&lf.lfFaceName[..len]);
    // "@Tên" là bản chữ dọc của cùng font, không cần
    if !name.starts_with('@') {
        names.push(name);
    }
    1 // tiếp tục liệt kê
}

/// Tên các font đã cài, sắp theo bảng chữ cái
fn installed_fonts() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    unsafe {
        let hdc = GetDC(None);
        let filter = LOGFONTW {
            lfCharSet: DEFAULT_CHARSET,
            ..Default::default()
        };
        EnumFontFamiliesExW(
            hdc,
            &filter,
            Some(collect_font),
            LPARAM(&raw mut names as isize),
            0,
        );
        ReleaseDC(None, hdc);
    }
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

// ---------- Dựng cửa sổ ----------

/// Tạo control con ở tọa độ tỉ lệ 100%, tự nhân theo DPI và gán font
struct Builder {
    parent: HWND,
    instance: HINSTANCE,
    font: HFONT,
    scale: f32,
}

impl Builder {
    fn px(&self, v: i32) -> i32 {
        (v as f32 * self.scale).round() as i32
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &self,
        class: PCWSTR,
        text: &str,
        style: i32,
        ex: WINDOW_EX_STYLE,
        (x, y, w, h): (i32, i32, i32, i32),
        id: i32,
    ) -> HWND {
        unsafe {
            let hwnd = CreateWindowExW(
                ex,
                class,
                &HSTRING::from(text),
                WINDOW_STYLE(style as u32) | WS_CHILD | WS_VISIBLE,
                self.px(x),
                self.px(y),
                self.px(w),
                self.px(h),
                Some(self.parent),
                Some(HMENU(id as isize as *mut c_void)),
                Some(self.instance),
                None,
            )
            .unwrap_or_default();
            SendMessageW(
                hwnd,
                WM_SETFONT,
                Some(WPARAM(self.font.0 as usize)),
                Some(LPARAM(1)),
            );
            hwnd
        }
    }

    fn group(&self, text: &str, x: i32, y: i32, w: i32, h: i32) {
        self.add(
            w!("BUTTON"),
            text,
            BS_GROUPBOX,
            WINDOW_EX_STYLE(0),
            (x, y, w, h),
            -1,
        );
    }

    fn label(&self, text: &str, x: i32, y: i32, w: i32, id: i32) {
        const SS_CENTERIMAGE: i32 = 0x200; // canh giữa theo chiều dọc
        let ex = WINDOW_EX_STYLE(0);
        self.add(w!("STATIC"), text, SS_CENTERIMAGE, ex, (x, y, w, 23), id);
    }

    fn check(&self, text: &str, x: i32, y: i32, w: i32, id: i32) {
        let style = BS_AUTOCHECKBOX | WS_TABSTOP.0 as i32;
        self.add(
            w!("BUTTON"),
            text,
            style,
            WINDOW_EX_STYLE(0),
            (x, y, w, 23),
            id,
        );
    }

    fn combo(&self, x: i32, y: i32, w: i32, id: i32) {
        let style = CBS_DROPDOWNLIST | (WS_VSCROLL.0 | WS_TABSTOP.0) as i32;
        // Chiều cao ở đây là chiều cao danh sách khi thả xuống
        self.add(
            w!("COMBOBOX"),
            "",
            style,
            WINDOW_EX_STYLE(0),
            (x, y, w, 320),
            id,
        );
    }

    fn edit(&self, x: i32, y: i32, w: i32, id: i32, number: bool) {
        let style = ES_AUTOHSCROLL | WS_TABSTOP.0 as i32 | if number { ES_NUMBER } else { 0 };
        self.add(w!("EDIT"), "", style, WS_EX_CLIENTEDGE, (x, y, w, 23), id);
    }

    fn password(&self, x: i32, y: i32, w: i32, id: i32) {
        let style = ES_AUTOHSCROLL | ES_PASSWORD | WS_TABSTOP.0 as i32;
        self.add(w!("EDIT"), "", style, WS_EX_CLIENTEDGE, (x, y, w, 23), id);
    }

    fn multiline(&self, x: i32, y: i32, w: i32, h: i32, id: i32) {
        let style =
            ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | (WS_VSCROLL.0 | WS_TABSTOP.0) as i32;
        self.add(w!("EDIT"), "", style, WS_EX_CLIENTEDGE, (x, y, w, h), id);
    }

    /// Chữ nhiều dòng (tự xuống dòng), cao `h`
    fn note(&self, text: &str, x: i32, y: i32, w: i32, h: i32, id: i32) {
        self.add(w!("STATIC"), text, 0, WINDOW_EX_STYLE(0), (x, y, w, h), id);
    }

    fn slider(&self, x: i32, y: i32, w: i32, id: i32, (min, max): (u32, u32)) {
        let style = (TBS_HORZ | TBS_NOTICKS) as i32 | WS_TABSTOP.0 as i32;
        let hwnd = self.add(
            TRACKBAR_CLASSW,
            "",
            style,
            WINDOW_EX_STYLE(0),
            (x, y, w, 26),
            id,
        );
        let range = (min & 0xFFFF) | (max << 16); // MAKELONG(min, max)
        unsafe {
            SendMessageW(
                hwnd,
                TBM_SETRANGE,
                Some(WPARAM(1)),
                Some(LPARAM(range as isize)),
            );
        }
    }

    fn button(&self, text: &str, x: i32, y: i32, w: i32, id: i32, default: bool) {
        let kind = if default {
            BS_DEFPUSHBUTTON
        } else {
            BS_PUSHBUTTON
        };
        let style = kind | WS_TABSTOP.0 as i32;
        self.add(
            w!("BUTTON"),
            text,
            style,
            WINDOW_EX_STYLE(0),
            (x, y, w, 28),
            id,
        );
    }

    fn swatch(&self, x: i32, y: i32, id: i32) {
        let style = BS_OWNERDRAW | WS_TABSTOP.0 as i32;
        self.add(
            w!("BUTTON"),
            "",
            style,
            WINDOW_EX_STYLE(0),
            (x, y, 44, 23),
            id,
        );
    }
}

fn create(island: HWND) -> windows::core::Result<()> {
    unsafe {
        let instance: HINSTANCE = GetModuleHandleW(None)?.into();
        let class_name = w!("DynamicIslandSettings");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance,
            hIcon: crate::icon::app_icon(false),
            hIconSm: crate::icon::app_icon(true),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: GetSysColorBrush(COLOR_BTNFACE),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc); // lần mở thứ hai: lớp đã có, lỗi này bỏ qua được

        let dpi = GetDpiForWindow(island).max(96);
        let scale = dpi as f32 / 96.0;
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let ex = WS_EX_CONTROLPARENT | WS_EX_APPWINDOW;
        let mut r = RECT {
            left: 0,
            top: 0,
            right: (CLIENT.0 as f32 * scale) as i32,
            bottom: (CLIENT.1 as f32 * scale) as i32,
        };
        AdjustWindowRectExForDpi(&mut r, style, false, ex, dpi)?;
        let (w, h) = (r.right - r.left, r.bottom - r.top);
        let x = (GetSystemMetrics(SM_CXSCREEN) - w) / 2;
        let y = (GetSystemMetrics(SM_CYSCREEN) - h) / 2;
        let hwnd = CreateWindowExW(
            ex,
            class_name,
            w!("Cài đặt Dynamic Island"),
            style,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(instance),
            None,
        )?;

        // Font giao diện của Windows: Segoe UI 9pt theo DPI
        let mut lf = LOGFONTW {
            lfHeight: -(9.0 * dpi as f32 / 72.0).round() as i32,
            lfWeight: 400,
            lfQuality: CLEARTYPE_QUALITY,
            ..Default::default()
        };
        for (dst, src) in lf.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
            *dst = src;
        }
        let font = CreateFontIndirectW(&lf);

        let config = config::load().unwrap_or_default();
        let c = &config.colors;
        STATE.set(Some(State {
            hwnd,
            island,
            font,
            colors: [c.background, c.text, c.secondary, c.accent],
        }));

        build(&Builder {
            parent: hwnd,
            instance,
            font,
            scale,
        });
        fill(hwnd, &config);
        set_checked(hwnd, ID_AUTOSTART, autostart::is_enabled());
        update_enabled(hwnd);
        // Thanh trượt chỉnh lời: −60 … +60 giây tính theo nấc nửa giây
        let steps = (lyrics::MAX_OFFSET_MS / OFFSET_STEP_MS) as isize;
        send(hwnd, ID_OFFSET, TBM_SETRANGEMIN, 0, -steps);
        send(hwnd, ID_OFFSET, TBM_SETRANGEMAX, 1, steps);
        send(
            hwnd,
            ID_OFFSET,
            TBM_SETTICFREQ,
            OFFSET_TICK_EVERY as usize,
            0,
        );
        send(hwnd, ID_OFFSET, TBM_SETPAGESIZE, 0, 2); // bấm hai bên thanh: 1 giây
        OFFSET_SONG.with_borrow_mut(String::clear);
        refresh_offset(hwnd);
        SetTimer(Some(hwnd), TIMER_NOW_PLAYING, 1000, None);

        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        Ok(())
    }
}

/// Đặt các control, hai cột như bản vẽ dưới đây (tọa độ ở tỉ lệ 100%)
fn build(b: &Builder) {
    // ===== Cột trái =====
    b.group("Giao diện", 12, 8, 300, 202);
    b.label("Hiệu ứng nền", 24, 32, 110, -1);
    b.combo(140, 32, 160, ID_BACKDROP);
    b.label("Độ đục của nền", 24, 64, 110, -1);
    b.slider(134, 62, 130, ID_OPACITY, (0, 100));
    b.label("", 266, 64, 40, ID_OPACITY_VALUE);
    for (i, name) in ["Màu nền", "Màu chữ", "Màu chữ phụ", "Màu nhấn"]
        .iter()
        .enumerate()
    {
        let (col, row) = ((i % 2) as i32, (i / 2) as i32);
        let (x, y) = (24 + col * 146, 98 + row * 30);
        b.label(name, x, y, 82, -1);
        b.swatch(x + 84, y, ID_COLOR + i as i32);
    }
    b.label("Phông chữ", 24, 166, 110, -1);
    b.combo(140, 166, 160, ID_FONT);

    b.group("Kích thước và vị trí", 12, 218, 300, 126);
    b.label("Cách mép trên (px)", 24, 242, 130, -1);
    b.edit(160, 242, 60, ID_TOP, true);
    b.label("Rộng khi thu gọn", 24, 274, 120, -1);
    b.slider(144, 272, 116, ID_COLLAPSED_W, (180, 300));
    b.label("", 262, 274, 46, ID_COLLAPSED_W_VALUE);
    b.label("Rộng khi mở rộng", 24, 306, 120, -1);
    b.slider(144, 304, 116, ID_EXPANDED_W, (320, 700));
    b.label("", 262, 306, 46, ID_EXPANDED_W_VALUE);

    b.group("Đồng hồ", 12, 352, 300, 84);
    b.check("Kiểu 24 giờ", 24, 376, 270, ID_HOUR24);
    b.check("Hiện giây khi mở rộng", 24, 402, 270, ID_SECONDS);

    // ===== Cột phải =====
    b.group("Chung", 328, 8, 300, 134);
    b.check("Khởi động cùng Windows", 340, 32, 276, ID_AUTOSTART);
    b.check(
        "Tự thu nhỏ thành vạch khi không dùng",
        340,
        58,
        276,
        ID_AUTOHIDE,
    );
    b.label("Sau", 362, 86, 36, -1);
    b.edit(400, 86, 50, ID_AUTOHIDE_DELAY, true);
    b.label("giây", 456, 86, 60, -1);
    b.check(
        "Không thu nhỏ khi đang phát nhạc",
        362,
        112,
        254,
        ID_STAY_PLAYING,
    );

    b.group("Thời tiết", 328, 150, 300, 90);
    b.check("Hiện thời tiết", 340, 174, 150, ID_WEATHER);
    b.check("Độ F (°F)", 500, 174, 116, ID_FAHRENHEIT);
    b.label("Thành phố", 340, 204, 80, -1);
    b.edit(424, 204, 190, ID_CITY, false);

    b.group("Lời bài hát", 328, 248, 300, 188);
    b.check("Lời chạy theo nhạc (từ LRCLIB)", 340, 272, 276, ID_LYRICS);
    b.check("Hiện khung lời khi mở rộng", 362, 298, 254, ID_LYRICS_SHOW);
    b.check(
        "Hiện lời trên island thu gọn",
        362,
        324,
        254,
        ID_LYRICS_COLLAPSED,
    );
    b.label("Bộ nhớ đệm", 362, 356, 90, -1);
    b.combo(456, 356, 158, ID_CACHE);
    b.button("", 362, 394, 252, ID_CLEAR_CACHE, false);

    // ===== Các hàng dưới, trải hết chiều ngang =====
    // Chỉnh lời sớm/muộn cho bài đang phát: áp dụng ngay, không cần bấm Lưu
    b.group(
        "Lời bài đang phát (chỉnh là áp dụng ngay)",
        12,
        444,
        616,
        86,
    );
    // SS_ENDELLIPSIS: tên bài dài quá thì Windows tự cắt và thêm "…"
    const SS_CENTERIMAGE_ENDELLIPSIS: i32 = 0x200 | 0x4000;
    let ex = WINDOW_EX_STYLE(0);
    let song_label = (24, 466, 592, 23);
    b.add(
        w!("STATIC"),
        "",
        SS_CENTERIMAGE_ENDELLIPSIS,
        ex,
        song_label,
        ID_OFFSET_SONG,
    );
    b.label("Lời sớm/muộn", 24, 496, 96, -1);
    // Thanh trượt kiểu Windows: kéo, bấm hai bên, hoặc dùng phím mũi tên
    let style = (TBS_HORZ | TBS_AUTOTICKS) as i32 | WS_TABSTOP.0 as i32;
    b.add(
        TRACKBAR_CLASSW,
        "",
        style,
        ex,
        (120, 492, 352, 30),
        ID_OFFSET,
    );
    b.label("", 476, 496, 64, ID_OFFSET_VALUE);
    b.button("Về 0", 540, 494, 76, ID_OFFSET_RESET, false);

    b.group("Thông báo trên island", 12, 538, 616, 60);
    b.check("Nội dung vừa sao chép", 24, 562, 190, ID_ALERT_CLIPBOARD);
    b.check(
        "Thông báo của Windows",
        220,
        562,
        190,
        ID_ALERT_NOTIFICATIONS,
    );
    b.label("Hiện trong", 430, 562, 70, -1);
    b.edit(502, 562, 44, ID_ALERT_SECONDS, true);
    b.label("giây", 552, 562, 60, -1);

    // ===== Cột thứ ba: Trợ lý AI =====
    b.group("Trợ lý AI (Cloudflare Workers AI)", 644, 8, 300, 590);
    b.label("Model", 656, 34, 80, -1);
    b.combo(740, 32, 192, ID_AI_MODEL);
    b.label("Account ID", 656, 66, 80, -1);
    b.edit(740, 64, 192, ID_AI_ACCOUNT, false);
    b.label("API token", 656, 98, 80, -1);
    b.password(740, 96, 192, ID_AI_TOKEN);
    b.button("Xóa token đã lưu", 740, 126, 192, ID_AI_TOKEN_CLEAR, false);
    b.label(
        "Lời dặn cho trợ lý (vai trò, cách trả lời)",
        656,
        164,
        276,
        -1,
    );
    b.multiline(656, 188, 276, 180, ID_AI_PROMPT);
    b.button("Kiểm tra kết nối", 656, 380, 140, ID_AI_TEST, false);
    b.note("", 656, 414, 276, 92, ID_AI_TEST_RESULT);
    b.note(
        "Câu hỏi được gửi tới Workers AI của Cloudflare. Token cất trong Windows \
         Credential Manager, không ghi vào file cấu hình.",
        656,
        530,
        276,
        54,
        -1,
    );

    // ===== Hàng nút =====
    b.button("Mặc định", 12, 610, 100, ID_DEFAULTS, false);
    b.button("Hủy", 736, 610, 100, ID_CANCEL, false);
    b.button("Lưu", 844, 610, 100, ID_SAVE, true);

    let hwnd = b.parent;
    for (kind, label) in BACKDROPS {
        combo_add(hwnd, ID_BACKDROP, label, kind as usize);
    }
    for (i, name) in installed_fonts().iter().enumerate() {
        combo_add(hwnd, ID_FONT, name, i);
    }
    for mb in CACHE_SIZES {
        let label = if mb == 0 {
            "Tắt".to_string()
        } else {
            format!("{mb} MB")
        };
        combo_add(hwnd, ID_CACHE, &label, mb as usize);
    }
    send(hwnd, ID_FONT, CB_SETMINVISIBLE, 14, 0);
    for (i, (_, label)) in ai::MODELS.iter().enumerate() {
        combo_add(hwnd, ID_AI_MODEL, label, i);
    }
    let banner = HSTRING::from("Dán token mới vào đây");
    send(
        hwnd,
        ID_AI_TOKEN,
        EM_SETCUEBANNER,
        1,
        banner.as_ptr() as isize,
    );
}

/// Đưa giá trị của `config` lên các control (khi mở cửa sổ, và khi bấm Mặc định)
fn fill(hwnd: HWND, config: &Config) {
    let backdrop = BACKDROPS
        .iter()
        .position(|(k, _)| *k == config.window.backdrop)
        .unwrap_or(0);
    combo_select(hwnd, ID_BACKDROP, backdrop);
    set_slider(
        hwnd,
        ID_OPACITY,
        alpha_to_percent(config.colors.background.a),
    );
    if let Some(mut s) = STATE.get() {
        let c = &config.colors;
        s.colors = [c.background, c.text, c.secondary, c.accent];
        STATE.set(Some(s));
        for i in 0..4 {
            unsafe {
                let _ = InvalidateRect(Some(item(hwnd, ID_COLOR + i)), None, true);
            }
        }
    }
    // Font chưa cài (hoặc gõ sai trong file) vẫn giữ, thêm vào cuối danh sách
    let family = HSTRING::from(config.font.family.as_str());
    let found = send(
        hwnd,
        ID_FONT,
        CB_FINDSTRINGEXACT,
        usize::MAX,
        family.as_ptr() as isize,
    );
    let font = usize::try_from(found)
        .unwrap_or_else(|_| combo_add(hwnd, ID_FONT, &config.font.family, usize::MAX));
    combo_select(hwnd, ID_FONT, font);

    set_text(hwnd, ID_TOP, &config.window.top.to_string());
    set_slider(
        hwnd,
        ID_COLLAPSED_W,
        config.window.collapsed_width.round() as u32,
    );
    set_slider(
        hwnd,
        ID_EXPANDED_W,
        config.window.expanded_width.round() as u32,
    );
    set_checked(hwnd, ID_HOUR24, config.clock.hour24);
    set_checked(hwnd, ID_SECONDS, config.clock.show_seconds);
    set_checked(hwnd, ID_AUTOHIDE, config.autohide.enabled);
    set_text(
        hwnd,
        ID_AUTOHIDE_DELAY,
        &config.autohide.delay_seconds.to_string(),
    );
    set_checked(hwnd, ID_STAY_PLAYING, config.autohide.stay_while_playing);
    set_checked(hwnd, ID_WEATHER, config.weather.enabled);
    set_checked(hwnd, ID_FAHRENHEIT, config.weather.fahrenheit);
    set_text(hwnd, ID_CITY, &config.weather.city);
    set_checked(hwnd, ID_LYRICS, config.lyrics.enabled);
    set_checked(hwnd, ID_LYRICS_SHOW, config.lyrics.show);
    set_checked(hwnd, ID_LYRICS_COLLAPSED, config.lyrics.collapsed);
    let mb = config.lyrics.cache_mb;
    combo_select_data(hwnd, ID_CACHE, mb as usize, &format!("{mb} MB"));
    set_checked(hwnd, ID_ALERT_CLIPBOARD, config.alerts.clipboard);
    set_checked(hwnd, ID_ALERT_NOTIFICATIONS, config.alerts.notifications);
    set_text(hwnd, ID_ALERT_SECONDS, &config.alerts.seconds.to_string());

    let assistant = &config.ai;
    // Model lạ (tự sửa trong file) vẫn giữ, thêm vào cuối danh sách
    let model = ai::MODELS
        .iter()
        .position(|(id, _)| *id == assistant.model)
        .unwrap_or_else(|| combo_add(hwnd, ID_AI_MODEL, &assistant.model, usize::MAX));
    combo_select(hwnd, ID_AI_MODEL, model);
    set_text(hwnd, ID_AI_ACCOUNT, &assistant.account_id);
    set_text(hwnd, ID_AI_TOKEN, "");
    // Ô nhập nhiều dòng của Windows cần \r\n để xuống dòng
    set_text(
        hwnd,
        ID_AI_PROMPT,
        &assistant.system_prompt.replace('\n', "\r\n"),
    );
    update_values(hwnd);
    update_enabled(hwnd);
}

/// Model đang chọn
fn chosen_model(hwnd: HWND) -> String {
    match combo_data(hwnd, ID_AI_MODEL).and_then(|i| ai::MODELS.get(i)) {
        Some((id, _)) => id.to_string(),
        None => text(hwnd, ID_AI_MODEL), // model tự sửa trong file: nhãn chính là mã model
    }
}

/// Đọc các control thành cấu hình; Err là lời nhắc cho người dùng
fn read(hwnd: HWND) -> Result<Config, &'static str> {
    let state = STATE.get().ok_or("Cửa sổ đã đóng")?;
    let top = text(hwnd, ID_TOP)
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|t| (0..=300).contains(t))
        .ok_or("Khoảng cách tới mép trên phải từ 0 đến 300 px.")?;
    let delay = text(hwnd, ID_AUTOHIDE_DELAY)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|d| (1..=3600).contains(d))
        .ok_or("Thời gian chờ trước khi thu nhỏ phải từ 1 đến 3600 giây.")?;
    let alert_seconds = text(hwnd, ID_ALERT_SECONDS)
        .trim()
        .parse::<u32>()
        .ok()
        .filter(|s| (2..=30).contains(s))
        .ok_or("Thông báo trên island hiện trong 2 đến 30 giây.")?;
    let city = text(hwnd, ID_CITY).trim().to_string();
    let weather = checked(hwnd, ID_WEATHER);
    if weather && city.is_empty() {
        return Err("Hãy nhập tên thành phố để xem thời tiết.");
    }

    let [background, text_color, secondary, accent] = state.colors;
    let backdrop = combo_selected(hwnd, ID_BACKDROP)
        .and_then(|i| BACKDROPS.get(i))
        .map_or(BackdropKind::None, |(k, _)| *k);
    let mut c = Config::default();
    c.window.top = top;
    c.window.collapsed_width = slider(hwnd, ID_COLLAPSED_W) as f32;
    c.window.expanded_width = slider(hwnd, ID_EXPANDED_W) as f32;
    c.window.backdrop = backdrop;
    c.colors.background = Color {
        a: percent_to_alpha(slider(hwnd, ID_OPACITY)),
        ..background
    };
    c.colors.text = text_color;
    c.colors.secondary = secondary;
    c.colors.accent = accent;
    c.font.family = self::text(hwnd, ID_FONT);
    c.clock.hour24 = checked(hwnd, ID_HOUR24);
    c.clock.show_seconds = checked(hwnd, ID_SECONDS);
    c.weather.enabled = weather;
    c.weather.city = city;
    c.weather.fahrenheit = checked(hwnd, ID_FAHRENHEIT);
    c.autohide.enabled = checked(hwnd, ID_AUTOHIDE);
    c.autohide.delay_seconds = delay;
    c.autohide.stay_while_playing = checked(hwnd, ID_STAY_PLAYING);
    c.lyrics.enabled = checked(hwnd, ID_LYRICS);
    c.lyrics.show = checked(hwnd, ID_LYRICS_SHOW);
    c.lyrics.collapsed = checked(hwnd, ID_LYRICS_COLLAPSED);
    c.lyrics.cache_mb = combo_data(hwnd, ID_CACHE).unwrap_or(25) as u32;
    c.alerts.clipboard = checked(hwnd, ID_ALERT_CLIPBOARD);
    c.alerts.notifications = checked(hwnd, ID_ALERT_NOTIFICATIONS);
    c.alerts.seconds = alert_seconds;
    c.ai.model = chosen_model(hwnd);
    c.ai.account_id = text(hwnd, ID_AI_ACCOUNT).trim().to_string();
    let prompt = text(hwnd, ID_AI_PROMPT).replace("\r\n", "\n");
    c.ai.system_prompt = prompt.trim().to_string();
    Ok(c)
}

// ---------- Chỉnh lời sớm/muộn ----------

/// "+5.0 s", "−12.5 s", "0 s"
fn offset_label(offset_ms: i64) -> String {
    if offset_ms == 0 {
        "0 s".to_string()
    } else {
        format!("{:+.1} s", offset_ms as f64 / 1000.0)
    }
}

/// Dòng mô tả bài đang phát phía trên thanh trượt
fn now_playing_line(np: Option<&lyrics::NowPlaying>) -> String {
    match np {
        Some(np) if np.synced => {
            // Bản lời ngắn hơn video (MV có đoạn mở đầu...): lời có thể chạy sớm.
            // Ghi chú đặt đầu dòng để tên bài dài có bị cắt cũng không mất ghi chú
            let mut line = if np.timing.approximate {
                "Lời có thể lệch, kéo cho khớp  ·  ".to_string()
            } else {
                String::new()
            };
            line.push_str(&np.title);
            if !np.artist.is_empty() {
                line.push_str(" — ");
                line.push_str(&np.artist);
            }
            line
        }
        Some(np) => format!(
            "{} — chưa có lời chạy theo nhạc nên không chỉnh được",
            np.title
        ),
        None => "Chưa có bài nào đang phát".to_string(),
    }
}

/// Đưa bài đang phát và độ lệch của nó lên nhóm "Lời bài đang phát".
/// Chỉ làm lại khi bài đã đổi, để không giật thanh trượt khi người dùng đang kéo
fn refresh_offset(hwnd: HWND) {
    let np = lyrics::now_playing();
    let key = np
        .as_ref()
        .map(|n| format!("{}\u{1}{}\u{1}{}", n.title, n.artist, n.synced))
        .unwrap_or_default();
    let changed = OFFSET_SONG.with_borrow_mut(|old| {
        let changed = *old != key || old.is_empty();
        *old = key;
        changed
    });
    if !changed {
        return;
    }
    set_text(hwnd, ID_OFFSET_SONG, &now_playing_line(np.as_ref()));
    let synced = np.as_ref().is_some_and(|n| n.synced);
    let offset = np.as_ref().map_or(0, |n| n.timing.offset_ms);
    send(
        hwnd,
        ID_OFFSET,
        TBM_SETPOS,
        1,
        (offset / OFFSET_STEP_MS) as isize,
    );
    set_text(hwnd, ID_OFFSET_VALUE, &offset_label(offset));
    for id in [ID_OFFSET, ID_OFFSET_RESET] {
        enable(hwnd, id, synced);
    }
}

/// Áp độ lệch mới cho bài đang phát và báo island vẽ lại ngay
fn apply_offset(hwnd: HWND, offset_ms: i64, save: bool) {
    let Some(np) = lyrics::now_playing().filter(|n| n.synced) else {
        return;
    };
    lyrics::set_offset(&np.title, &np.artist, offset_ms, save);
    set_text(hwnd, ID_OFFSET_VALUE, &offset_label(offset_ms));
    if let Some(s) = STATE.get() {
        unsafe {
            let _ = PostMessageW(
                Some(s.island),
                lyrics::WM_LYRICS_CHANGED,
                WPARAM(0),
                LPARAM(0),
            );
        }
    }
}

/// Người dùng kéo thanh trượt chỉnh lời; thả tay (`TB_ENDTRACK`) mới ghi xuống đĩa
fn on_offset_scroll(hwnd: HWND, code: usize) {
    let pos = send(hwnd, ID_OFFSET, WM_USER, 0, 0) as i64; // WM_USER = TBM_GETPOS
    apply_offset(hwnd, pos * OFFSET_STEP_MS, code == TB_ENDTRACK);
}

/// Chữ số cạnh các thanh trượt
fn update_values(hwnd: HWND) {
    set_text(
        hwnd,
        ID_OPACITY_VALUE,
        &format!("{}%", slider(hwnd, ID_OPACITY)),
    );
    set_text(
        hwnd,
        ID_COLLAPSED_W_VALUE,
        &format!("{} px", slider(hwnd, ID_COLLAPSED_W)),
    );
    set_text(
        hwnd,
        ID_EXPANDED_W_VALUE,
        &format!("{} px", slider(hwnd, ID_EXPANDED_W)),
    );
}

/// Làm mờ các tùy chọn con khi tùy chọn cha đang tắt
fn update_enabled(hwnd: HWND) {
    let autohide = checked(hwnd, ID_AUTOHIDE);
    enable(hwnd, ID_AUTOHIDE_DELAY, autohide);
    enable(hwnd, ID_STAY_PLAYING, autohide);
    let weather = checked(hwnd, ID_WEATHER);
    enable(hwnd, ID_CITY, weather);
    enable(hwnd, ID_FAHRENHEIT, weather);
    let lyrics = checked(hwnd, ID_LYRICS);
    for id in [ID_LYRICS_SHOW, ID_LYRICS_COLLAPSED, ID_CACHE] {
        enable(hwnd, id, lyrics);
    }
    // Nút xóa bộ nhớ đệm ghi luôn số bài và dung lượng đang dùng
    let (label, has_files) = match lyrics::cache_summary() {
        Some((count, summary)) => (format!("Xóa bộ nhớ đệm ({summary})"), count > 0),
        None => ("Bộ nhớ đệm đang tắt".to_string(), false),
    };
    set_text(hwnd, ID_CLEAR_CACHE, &label);
    enable(hwnd, ID_CLEAR_CACHE, lyrics && has_files);

    let has_token = secret::has_token();
    enable(hwnd, ID_AI_TOKEN_CLEAR, has_token);
    let banner = HSTRING::from(if has_token {
        "Đã lưu (để trống để giữ nguyên)"
    } else {
        "Dán token vào đây"
    });
    send(
        hwnd,
        ID_AI_TOKEN,
        EM_SETCUEBANNER,
        1,
        banner.as_ptr() as isize,
    );
}

/// Gửi một câu hỏi thử bằng Account ID, token (ô nhập hoặc token đã lưu) và model đang chọn
fn test_connection(hwnd: HWND) {
    let typed = text(hwnd, ID_AI_TOKEN).trim().to_string();
    let settings = ai::Settings {
        account_id: text(hwnd, ID_AI_ACCOUNT).trim().to_string(),
        token: if typed.is_empty() {
            secret::load_token().unwrap_or_default()
        } else {
            typed
        },
        model: chosen_model(hwnd),
        system_prompt: "Trả lời đúng một câu ngắn.".to_string(),
    };
    set_text(hwnd, ID_AI_TEST_RESULT, "Đang hỏi thử AI…");
    enable(hwnd, ID_AI_TEST, false);
    let hwnd_raw = hwnd.0 as isize;
    std::thread::spawn(move || {
        let question = ai::Message::user("Chào bạn! Hãy xác nhận bạn đang hoạt động.");
        let line = match ai::ask(&settings, &[question]) {
            Ok(answer) => format!("✓ Kết nối được. AI trả lời: {answer}"),
            Err(e) => format!("✗ {e}"),
        };
        *TEST_RESULT.lock().unwrap() = Some(line);
        unsafe {
            let _ = PostMessageW(
                Some(HWND(hwnd_raw as *mut c_void)),
                WM_AI_TEST_DONE,
                WPARAM(0),
                LPARAM(0),
            );
        }
    });
}

fn message(hwnd: HWND, text: &str) {
    unsafe {
        MessageBoxW(
            Some(hwnd),
            &HSTRING::from(text),
            w!("Cài đặt Dynamic Island"),
            MB_OK | MB_ICONWARNING,
        );
    }
}

/// Lưu: ghi file, đổi "khởi động cùng Windows", rồi khởi động lại island nếu có gì đổi
fn save(hwnd: HWND) {
    let Some(state) = STATE.get() else { return };
    let config = match read(hwnd) {
        Ok(config) => config,
        Err(problem) => return message(hwnd, problem),
    };
    let autostart = checked(hwnd, ID_AUTOSTART);
    if autostart != autostart::is_enabled() {
        autostart::set(autostart);
    }
    // Token không nằm trong file cấu hình: cất vào Credential Manager (để trống = giữ cũ)
    let token = text(hwnd, ID_AI_TOKEN).trim().to_string();
    if !token.is_empty()
        && let Err(e) = secret::save_token(&token)
    {
        return message(hwnd, &format!("Không lưu được API token:\n{e}"));
    }
    let changed = config::load().ok().as_ref() != Some(&config);
    if changed && let Err(e) = config::save(&config) {
        return message(hwnd, &format!("Không lưu được cài đặt:\n{e}"));
    }
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    if changed {
        crate::restart(state.island);
    }
}

fn on_command(hwnd: HWND, id: i32, code: u32) {
    match id {
        ID_SAVE => save(hwnd),
        ID_CANCEL => unsafe {
            let _ = DestroyWindow(hwnd);
        },
        ID_DEFAULTS => fill(hwnd, &Config::default()),
        ID_OFFSET_RESET => {
            send(hwnd, ID_OFFSET, TBM_SETPOS, 1, 0);
            apply_offset(hwnd, 0, true);
        }
        ID_CLEAR_CACHE => {
            lyrics::clear_cache();
            update_enabled(hwnd);
        }
        ID_AI_TOKEN_CLEAR => {
            secret::delete_token();
            set_text(hwnd, ID_AI_TOKEN, "");
            update_enabled(hwnd);
        }
        ID_AI_TEST => test_connection(hwnd),
        id if (ID_COLOR..ID_COLOR + 4).contains(&id) => pick_color(hwnd, (id - ID_COLOR) as usize),
        ID_BACKDROP if code == CBN_SELCHANGE => {
            // Nền đục hẳn thì không thấy lớp kính: tự giảm độ đục cho người dùng
            let wants_glass = combo_selected(hwnd, ID_BACKDROP).is_some_and(|i| i > 0);
            if wants_glass && slider(hwnd, ID_OPACITY) == 100 {
                set_slider(hwnd, ID_OPACITY, OPACITY_FOR_BACKDROP);
                update_values(hwnd);
            }
        }
        _ if code == BN_CLICKED => update_enabled(hwnd),
        _ => {}
    }
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_COMMAND => {
            let id = (wparam.0 & 0xFFFF) as u16 as i32;
            let code = ((wparam.0 >> 16) & 0xFFFF) as u32;
            on_command(hwnd, id, code);
        }
        // Kéo thanh trượt: thanh chỉnh lời áp dụng ngay, các thanh khác chỉ đổi số hiển thị
        WM_HSCROLL if HWND(lparam.0 as *mut c_void) == item(hwnd, ID_OFFSET) => {
            on_offset_scroll(hwnd, wparam.0 & 0xFFFF);
        }
        WM_HSCROLL => update_values(hwnd),
        WM_TIMER if wparam.0 == TIMER_NOW_PLAYING => refresh_offset(hwnd),
        WM_AI_TEST_DONE => {
            if let Some(line) = TEST_RESULT.lock().unwrap().take() {
                set_text(hwnd, ID_AI_TEST_RESULT, &line);
            }
            enable(hwnd, ID_AI_TEST, true);
        }
        WM_DRAWITEM => {
            draw_swatch(unsafe { &*(lparam.0 as *const DRAWITEMSTRUCT) });
            return LRESULT(1);
        }
        WM_DESTROY => {
            if let Some(s) = STATE.take() {
                unsafe {
                    let _ = DeleteObject(s.font.into());
                }
            }
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opacity_percent_round_trips_common_values() {
        for p in [0, 25, 50, 60, 75, 100] {
            assert_eq!(alpha_to_percent(percent_to_alpha(p)), p);
        }
        assert_eq!(percent_to_alpha(100), 255);
        assert_eq!(alpha_to_percent(0x90), 56);
    }

    #[test]
    fn offset_labels() {
        assert_eq!(offset_label(0), "0 s");
        assert_eq!(offset_label(5_000), "+5.0 s");
        assert_eq!(offset_label(-12_500), "-12.5 s");
    }

    #[test]
    fn now_playing_line_explains_what_can_be_adjusted() {
        let np = |synced, approximate| lyrics::NowPlaying {
            title: "Buông Đôi Tay Nhau Ra".into(),
            artist: "Sơn Tùng M-TP Official".into(),
            synced,
            timing: lyrics::Timing {
                offset_ms: 0,
                approximate,
            },
        };
        assert_eq!(
            now_playing_line(Some(&np(true, false))),
            "Buông Đôi Tay Nhau Ra — Sơn Tùng M-TP Official"
        );
        assert!(now_playing_line(Some(&np(true, true))).starts_with("Lời có thể lệch"));
        assert!(now_playing_line(Some(&np(false, false))).contains("không chỉnh được"));
        assert_eq!(now_playing_line(None), "Chưa có bài nào đang phát");
    }

    #[test]
    fn colorref_is_bgr() {
        assert_eq!(colorref(Color::rgb(0x11, 0x22, 0x33)), COLORREF(0x332211));
    }
}
