// Bản release chạy như ứng dụng cửa sổ (không có cửa sổ console đen)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ai; // gọi trợ lý AI (Cloudflare Workers AI)
mod alerts; // thông báo tạm thời: vừa sao chép, thông báo Windows
mod animation; // lò xo cho hiệu ứng co giãn
mod app; // trạng thái island, xử lý sự kiện
mod autostart; // khởi động cùng Windows
mod backdrop; // nền Blur / Acrylic phía sau island
mod battery; // trạng thái pin
mod clipboard; // nội dung vừa sao chép
mod config; // đọc file cấu hình config.toml
mod content; // nội dung hiển thị và các trang (Idle, Media, Media Player)
mod graphics; // vẽ bằng Direct2D + DirectComposition
mod http; // HTTP bằng WinHTTP (thời tiết, lời bài hát, AI)
mod icon; // biểu tượng ứng dụng 64px và tên ứng dụng
mod layout; // kích thước, vị trí nút
mod lyrics; // lời bài hát chạy theo nhạc (LRCLIB + cache trên đĩa)
mod media; // luồng nền đọc/điều khiển media
mod meter; // mức âm thanh đang phát, cho sóng nhạc của trang Media Player
mod notifications; // thông báo Windows mới nhất
mod palette; // màu nhấn lấy từ ảnh bìa
mod secret; // cất API token của AI trong Windows Credential Manager
mod settings; // cửa sổ Cài đặt
mod trace; // nhật ký gỡ lỗi (chỉ bản debug)
mod tray; // icon khay hệ thống
mod weather; // luồng nền tải thời tiết từ Open-Meteo

use std::cell::RefCell;
use std::sync::mpsc;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            Shell::{QUNS_PRESENTATION_MODE, SHQueryUserNotificationState},
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, Result, w},
};

use app::App;
use backdrop::Backdrop;
use config::Config;
use graphics::Graphics;
use layout::Layout;
use lyrics::WM_LYRICS_CHANGED;
use media::{MediaCommand, WM_MEDIA_CHANGED};
use tray::WM_TRAY;
use weather::WM_WEATHER_CHANGED;

const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    // wnd_proc là hàm `extern "system"` nên không nhận được tham số riêng;
    // trạng thái app phải đặt ở biến toàn cục của luồng giao diện
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Chạy `f` với app nếu nó đã được tạo.
///
/// Một số hàm Windows (ShowWindow...) gửi ngay thông điệp về chính cửa sổ này trong lúc app
/// còn đang được dùng. Khi đó bỏ qua thông điệp gửi lồng đó thay vì crash vì mượn app hai lần
fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        if let Ok(mut app) = cell.try_borrow_mut()
            && let Some(app) = app.as_mut()
        {
            f(app);
        }
    });
}

/// Đọc một giá trị từ app (None nếu app chưa được tạo).
/// Không giữ app trong lúc làm việc khác, vì menu chạy vòng lặp riêng và gọi lại wnd_proc
fn read_app<R>(f: impl FnOnce(&App) -> R) -> Option<R> {
    APP.with(|cell| cell.borrow().as_ref().map(f))
}

/// Hiện menu chuột phải (từ icon khay hoặc từ chính island)
fn show_menu(hwnd: HWND) {
    let collapsed = read_app(App::lyrics_collapsed).flatten();
    match tray::show_menu(hwnd, collapsed) {
        Some(tray::Action::OpenSettings) => settings::open(hwnd),
        Some(tray::Action::SetLyricsCollapsed(on)) => {
            with_app(|app| app.set_lyrics_collapsed(hwnd, on));
            // Ghi vào file cấu hình để lần sau mở lên vẫn giữ; lỗi thì chỉ mất lựa chọn này
            let _ = config::save_lyrics_collapsed(on);
        }
        None => {}
    }
}

/// Mở một bản mới của chương trình rồi đóng bản hiện tại (`hwnd` là cửa sổ island),
/// để bản mới đọc lại file cấu hình
pub fn restart(hwnd: HWND) {
    let started = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).spawn());
    if started.is_ok() {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// Chuyển một thông điệp tới cửa sổ nhận nó (cửa sổ Cài đặt được xử lý phím Tab/Enter/Esc trước)
fn dispatch(msg: &MSG) {
    if !settings::pre_translate(msg) {
        unsafe {
            let _ = TranslateMessage(msg);
            DispatchMessageW(msg);
        }
    }
}

/// Tọa độ chuột (x, y) nằm trong 2 nửa thấp/cao của lparam, có thể âm
fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

// Có ứng dụng toàn màn hình / thuyết trình đang chạy không?
/// Có app toàn màn hình (game, phim) đang che màn hình của island không?
/// Trả về kèm lý do để ghi nhật ký.
///
/// Không tin trạng thái "bận" (QUNS_BUSY) của Windows: trên máy nhiều màn hình nó báo bận cả
/// khi chỉ có cửa sổ phóng to ở màn hình khác. Thay vào đó tự kiểm tra cửa sổ trước nhất có
/// phủ kín cả màn hình của island (kể cả chỗ thanh tác vụ) không. Cửa sổ phóng to bình
/// thường chừa thanh tác vụ nên không tính. Chế độ trình chiếu thì luôn ẩn
fn is_fullscreen_app_running(island: HWND) -> (bool, String) {
    unsafe {
        if SHQueryUserNotificationState() == Ok(QUNS_PRESENTATION_MODE) {
            return (true, "đang ở chế độ trình chiếu".into());
        }
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return (false, "không có cửa sổ trước nhất".into());
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        // Màn hình nền và thanh tác vụ phủ kín màn hình nhưng không phải app toàn màn hình
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return (false, format!("màn hình nền ({class})"));
        }
        let monitor = MonitorFromWindow(island, MONITOR_DEFAULTTONEAREST);
        if MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST) != monitor {
            return (false, format!("{class} ở màn hình khác"));
        }
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let mut r = RECT::default();
        if !GetMonitorInfoW(monitor, &mut info).as_bool() || GetWindowRect(fg, &mut r).is_err() {
            return (false, "không đọc được kích thước".into());
        }
        let m = info.rcMonitor;
        let covers =
            r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom;
        let reason = format!(
            "{class} ({},{})-({},{}) so với màn hình ({},{})-({},{})",
            r.left, r.top, r.right, r.bottom, m.left, m.top, m.right, m.bottom
        );
        (covers, reason)
    }
}

/// Tọa độ x để cửa sổ nằm giữa màn hình chính
fn centered_x(layout: &Layout) -> i32 {
    let screen_width = unsafe { GetSystemMetrics(SM_CXSCREEN) };
    (screen_width - layout.win_w) / 2
}

// Đặt cửa sổ ở giữa mép trên màn hình chính
fn position_window(hwnd: HWND, layout: &Layout) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            None,
            centered_x(layout),
            layout.top,
            0,
            0,
            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
        );
    }
}

/// Đọc cấu hình; file lỗi thì báo cho người dùng rồi chạy tiếp với mặc định
fn load_config() -> Config {
    config::load().unwrap_or_else(|err| {
        let text = HSTRING::from(format!("{err}\n\nĐang dùng cấu hình mặc định."));
        unsafe {
            MessageBoxW(None, &text, w!("Dynamic Island"), MB_OK | MB_ICONWARNING);
        }
        Config::default()
    })
}

/// Vị trí và kích thước (x, y, rộng, cao) của cửa sổ island trên màn hình
fn window_rect(layout: &Layout) -> (i32, i32, i32, i32) {
    (centered_x(layout), layout.top, layout.win_w, layout.win_h)
}

/// `owner`: cửa sổ kính mờ (nếu có). Cửa sổ được sở hữu luôn nằm trên cửa sổ sở hữu nó,
/// nhờ vậy island luôn nằm ngay trên lớp kính
fn create_window(layout: &Layout, owner: Option<HWND>) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("DynamicIslandWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            hIcon: crate::icon::app_icon(false),
            hIconSm: crate::icon::app_icon(true),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let (x, y, w, h) = window_rect(layout);
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Island"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            owner,
            None,
            Some(instance.into()),
            None,
        )
    }
}

/// Vòng lặp thông điệp: khi đang co giãn thì vẽ liên tục,
/// còn lại thì ngủ chờ thông điệp để không tốn CPU
fn run_message_loop(hwnd: HWND) {
    unsafe {
        let mut msg = MSG::default();
        'main: loop {
            let animating =
                APP.with(|cell| cell.borrow().as_ref().is_some_and(|app| app.is_animating()));

            if animating {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    if msg.message == WM_QUIT {
                        break 'main;
                    }
                    dispatch(&msg);
                }
                with_app(|app| {
                    let _ = app.tick(hwnd);
                });
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    break 'main;
                }
                dispatch(&msg);
            }
        }
    }
}

fn main() -> Result<()> {
    trace::start();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let config = load_config();
    let layout = Layout::new(&config.window);
    // Lớp kính mờ tạo trước, để cửa sổ island nhận nó làm cửa sổ sở hữu.
    // Nền đục hẳn thì không nhìn thấy lớp kính, khỏi tạo cho tốn tài nguyên
    let backdrop = config
        .colors
        .background
        .is_translucent()
        .then(|| Backdrop::create(config.window.backdrop, window_rect(&layout)))
        .flatten();
    let hwnd = create_window(&layout, backdrop.as_ref().map(Backdrop::hwnd))?;

    let (tx, rx) = mpsc::channel::<MediaCommand>();
    let graphics = Graphics::new(hwnd, layout, &config)?;
    // HWND chứa con trỏ nên không gửi qua luồng được: đổi sang số trước
    let hwnd_raw = hwnd.0 as isize;
    // Thông báo tạm thời: luồng nền đọc thông báo Windows, tìm tên và biểu tượng app
    let alerts_config = config.alerts;
    let clips = (alerts_config.clipboard || alerts_config.notifications)
        .then(|| alerts::start(hwnd_raw, alerts_config.notifications))
        .filter(|_| alerts_config.clipboard);
    if clips.is_some() {
        clipboard::listen(hwnd);
    }
    let app = App::new(graphics, layout, backdrop, &config, tx, clips);
    app.show_first_frame(hwnd)?;
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    tray::add(hwnd)?;
    unsafe {
        SetTimer(Some(hwnd), app::TIMER_CLOCK, 1000, None);
    }

    // Lời bài hát: luồng riêng, luồng media báo cho nó mỗi khi đổi bài
    let lyrics = config
        .lyrics
        .enabled
        .then(|| lyrics::start(hwnd_raw, config.lyrics.cache_mb));
    std::thread::spawn(move || media::run(hwnd_raw, rx, lyrics));
    if config.weather.enabled {
        let weather_config = config.weather.clone();
        std::thread::spawn(move || weather::run(hwnd_raw, weather_config));
    }

    with_app(|app| app.show(hwnd, true));
    run_message_loop(hwnd);
    Ok(())
}

extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEMOVE => {
            let (x, y) = mouse_pos(lparam);
            with_app(|app| app.on_mouse_move(hwnd, x, y));
        }
        WM_MOUSELEAVE => with_app(|app| app.on_mouse_leave(hwnd)),
        WM_LBUTTONDOWN => {
            let (x, y) = mouse_pos(lparam);
            with_app(|app| app.on_click(hwnd, x, y));
        }
        // Con lăn: nửa cao của wparam là độ lăn (có dấu, ±120 mỗi nấc)
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16;
            with_app(|app| app.on_wheel(hwnd, delta));
        }
        WM_TIMER if wparam.0 == app::TIMER_LYRICS => with_app(|app| app.on_lyrics_tick(hwnd)),
        WM_TIMER if wparam.0 == app::TIMER_ALERT => with_app(|app| app.on_alert_timer(hwnd)),
        WM_TIMER if wparam.0 == app::TIMER_CLIPBOARD => {
            with_app(|app| app.on_clipboard_timer(hwnd))
        }
        clipboard::WM_CLIPBOARDUPDATE => with_app(|app| app.on_clipboard_update(hwnd)),
        alerts::WM_ALERT => with_app(|app| app.on_alerts(hwnd)),
        WM_TIMER => {
            let (fullscreen, reason) = is_fullscreen_app_running(hwnd);
            if fullscreen != read_app(App::is_hidden).unwrap_or(false) {
                trace::log(format!("kiểm tra toàn màn hình: {fullscreen}, vì {reason}"));
            }
            with_app(|app| app.on_timer(hwnd, fullscreen));
        }
        WM_MEDIA_CHANGED => with_app(|app| app.on_media_changed(hwnd)),
        WM_WEATHER_CHANGED => with_app(|app| app.on_weather_changed(hwnd)),
        WM_LYRICS_CHANGED => with_app(|app| app.on_lyrics_changed(hwnd)),
        // Màn hình đổi (cắm/rút, đổi độ phân giải): đặt lại vị trí
        WM_DISPLAYCHANGE => with_app(|app| {
            position_window(hwnd, app.layout());
            if let Some(backdrop) = app.backdrop() {
                position_window(backdrop.hwnd(), app.layout());
            }
        }),
        // Icon khay: chuột phải thì hiện menu, nhấp đúp thì mở Cài đặt
        WM_TRAY => match (lparam.0 & 0xFFFF) as u32 {
            WM_RBUTTONUP => show_menu(hwnd),
            WM_LBUTTONDBLCLK => settings::open(hwnd),
            _ => {}
        },
        // Chuột phải lên island: cùng menu với icon khay
        WM_RBUTTONUP => show_menu(hwnd),
        WM_DESTROY => {
            tray::remove(hwnd);
            unsafe { PostQuitMessage(0) };
        }
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_pos_splits_lparam_into_signed_coordinates() {
        assert_eq!(mouse_pos(LPARAM((50 << 16) | 120)), (120, 50));
        // x = -1 được mã hóa là 0xFFFF (chuột bên trái màn hình khác)
        assert_eq!(mouse_pos(LPARAM((7 << 16) | 0xFFFF)), (-1, 7));
    }
}
