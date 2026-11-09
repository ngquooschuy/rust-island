// Bản release chạy như ứng dụng cửa sổ (không có cửa sổ console đen)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod animation; // lò xo cho hiệu ứng co giãn
mod app; // trạng thái island, xử lý sự kiện
mod autostart; // khởi động cùng Windows
mod battery; // trạng thái pin
mod config; // đọc file cấu hình config.toml
mod content; // nội dung hiển thị và các trang (Idle, Media)
mod graphics; // vẽ bằng Direct2D + DirectComposition
mod layout; // kích thước, vị trí nút
mod media; // luồng nền đọc/điều khiển media
mod tray; // icon khay hệ thống
mod weather; // luồng nền tải thời tiết từ Open-Meteo

use std::cell::RefCell;
use std::sync::mpsc;
use windows::{
    Win32::{
        Foundation::*,
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext},
            Shell::{
                QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
                SHQueryUserNotificationState,
            },
            WindowsAndMessaging::*,
        },
    },
    core::{HSTRING, Result, w},
};

use app::App;
use config::Config;
use graphics::Graphics;
use layout::Layout;
use media::{MediaCommand, WM_MEDIA_CHANGED};
use tray::WM_TRAY;
use weather::WM_WEATHER_CHANGED;

const WM_MOUSELEAVE: u32 = 0x02A3;

thread_local! {
    // wnd_proc là hàm `extern "system"` nên không nhận được tham số riêng;
    // trạng thái app phải đặt ở biến toàn cục của luồng giao diện
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Chạy `f` với app nếu nó đã được tạo
fn with_app(f: impl FnOnce(&mut App)) {
    APP.with(|cell| {
        if let Some(app) = cell.borrow_mut().as_mut() {
            f(app);
        }
    });
}

/// Tọa độ chuột (x, y) nằm trong 2 nửa thấp/cao của lparam, có thể âm
fn mouse_pos(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as i16 as i32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as i32;
    (x, y)
}

// Có ứng dụng toàn màn hình / thuyết trình đang chạy không?
fn is_fullscreen_app_running() -> bool {
    unsafe {
        SHQueryUserNotificationState()
            .map(|s| {
                s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE
            })
            .unwrap_or(false)
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

fn create_window(layout: &Layout) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("DynamicIslandWindow");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Island"),
            WS_POPUP,
            centered_x(layout),
            layout.top,
            layout.win_w,
            layout.win_h,
            None,
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
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                with_app(|app| {
                    let _ = app.tick(hwnd);
                });
            } else {
                if !GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    break 'main;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
}

fn main() -> Result<()> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let config = load_config();
    let layout = Layout::new(&config.window);
    let hwnd = create_window(&layout)?;

    let (tx, rx) = mpsc::channel::<MediaCommand>();
    let graphics = Graphics::new(hwnd, layout, &config)?;
    let app = App::new(graphics, layout, config.clock, tx);
    app.show_first_frame(hwnd)?;
    APP.with(|cell| *cell.borrow_mut() = Some(app));

    tray::add(hwnd)?;
    unsafe {
        SetTimer(Some(hwnd), 1, 1000, None);
    }

    // HWND chứa con trỏ nên không gửi qua luồng được: đổi sang số trước
    let hwnd_raw = hwnd.0 as isize;
    std::thread::spawn(move || media::run(hwnd_raw, rx));
    if config.weather.enabled {
        let weather_config = config.weather.clone();
        std::thread::spawn(move || weather::run(hwnd_raw, weather_config));
    }

    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
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
            with_app(|app| app.on_click(x, y));
        }
        // Con lăn: nửa cao của wparam là độ lăn (có dấu, ±120 mỗi nấc)
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16;
            with_app(|app| app.on_wheel(delta));
        }
        WM_TIMER => {
            let fullscreen = is_fullscreen_app_running();
            with_app(|app| app.on_timer(hwnd, fullscreen));
        }
        WM_MEDIA_CHANGED => with_app(|app| app.on_media_changed()),
        WM_WEATHER_CHANGED => with_app(|app| app.on_weather_changed()),
        // Màn hình đổi (cắm/rút, đổi độ phân giải): đặt lại vị trí
        WM_DISPLAYCHANGE => with_app(|app| position_window(hwnd, app.layout())),
        // Icon khay: chuột phải thì hiện menu
        WM_TRAY => {
            if (lparam.0 & 0xFFFF) as u32 == WM_RBUTTONUP {
                tray::show_menu(hwnd);
            }
        }
        // Chỉ bản debug: chuột phải lên island để thoát nhanh khi đang phát triển
        #[cfg(debug_assertions)]
        WM_RBUTTONDOWN => unsafe {
            let _ = DestroyWindow(hwnd);
        },
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
