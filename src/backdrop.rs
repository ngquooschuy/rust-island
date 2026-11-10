//! Hiệu ứng nền Blur / Acrylic thật của Windows phía sau island.
//!
//! Dùng một cửa sổ riêng nằm ngay dưới cửa sổ island. Cửa sổ này chỉ chứa một hình
//! viên thuốc tô bằng `HostBackdropBrush`: lớp kính mờ do chính Windows làm mờ từ
//! nội dung desktop phía sau (giống menu Start, thanh tác vụ). Cửa sổ island vẫn vẽ
//! như cũ; chỗ nào nền island trong suốt (#RRGGBBAA) thì nhìn xuyên xuống lớp kính.
//!
//! Vì sao không dùng `SetWindowCompositionAttribute` (cách cũ, phổ biến trên mạng)?
//! Trên Windows 11 nó làm mờ cả khung chữ nhật của cửa sổ, bỏ qua vùng cắt
//! (`SetWindowRgn`), nên quanh island lòi ra một mảng mờ hình chữ nhật.
//! Còn vì sao cần cửa sổ riêng: island dùng DirectComposition kiểu cũ, còn lớp kính
//! chỉ có ở Windows.UI.Composition; hai thứ không ghép chung một cây hình được.

use std::cell::Cell;

use windows::{
    System::DispatcherQueueController,
    UI::Composition::{
        CompositionRoundedRectangleGeometry, Compositor, Desktop::DesktopWindowTarget, SpriteVisual,
    },
    Win32::{
        Foundation::*,
        Graphics::Dwm::{DWMWA_USE_HOSTBACKDROPBRUSH, DwmSetWindowAttribute},
        System::{
            LibraryLoader::GetModuleHandleW,
            WinRT::{
                Composition::ICompositorDesktopInterop, CreateDispatcherQueueController,
                DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
            },
        },
        UI::WindowsAndMessaging::*,
    },
    core::{BOOL, Interface, Result, w},
};
use windows_numerics::Vector2;

use crate::config::BackdropKind;

/// Cửa sổ kính không tự xử lý gì, để Windows làm mặc định
extern "system" fn backdrop_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn create_window((x, y, w, h): (i32, i32, i32, i32)) -> Result<HWND> {
    unsafe {
        let instance = GetModuleHandleW(None)?;
        let class_name = w!("DynamicIslandBackdrop");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(backdrop_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        // WS_EX_NOREDIRECTIONBITMAP: không có bề mặt GDI, chỉ có hình của Composition
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class_name,
            w!("Island backdrop"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(instance.into()),
            None,
        )
    }
}

/// Cửa sổ kính mờ nằm dưới island
pub struct Backdrop {
    hwnd: HWND,
    /// Khung viên thuốc cắt lớp kính; đổi kích thước từng khung hình theo island
    geometry: CompositionRoundedRectangleGeometry,
    /// Hình đang cắt (trái, rộng, cao), để khỏi đặt lại khi không đổi
    shape: Cell<(f32, f32, f32)>,
    // Không dùng trực tiếp nhưng phải giữ sống, nếu không lớp kính biến mất
    _visual: SpriteVisual,
    _target: DesktopWindowTarget,
    _compositor: Compositor,
    _queue: DispatcherQueueController,
}

impl Backdrop {
    /// Tạo cửa sổ kính mờ (chưa hiện), cùng vị trí và kích thước với cửa sổ island.
    /// None nếu tắt trong cấu hình hoặc Windows không hỗ trợ (cần Windows 11)
    pub fn create(kind: BackdropKind, rect: (i32, i32, i32, i32)) -> Option<Self> {
        if kind == BackdropKind::None {
            return None;
        }
        let hwnd = create_window(rect).ok()?;
        match Self::attach(hwnd) {
            Ok(backdrop) => Some(backdrop),
            Err(_) => {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
                None
            }
        }
    }

    fn attach(hwnd: HWND) -> Result<Self> {
        unsafe {
            // Xin Windows cho cửa sổ này dùng lớp kính của hệ thống (Windows 11 trở lên)
            let on = BOOL::from(true);
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_USE_HOSTBACKDROPBRUSH,
                (&raw const on).cast(),
                std::mem::size_of::<BOOL>() as u32,
            )?;
            // Compositor cần một hàng đợi DispatcherQueue trên luồng này; vòng lặp thông
            // điệp của main.rs bơm luôn cho nó
            let queue = CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?;
            let compositor = Compositor::new()?;
            let interop: ICompositorDesktopInterop = compositor.cast()?;
            let target = interop.CreateDesktopWindowTarget(hwnd, false)?;

            let visual = compositor.CreateSpriteVisual()?;
            visual.SetRelativeSizeAdjustment(Vector2 { X: 1.0, Y: 1.0 })?; // phủ cả cửa sổ
            visual.SetBrush(&compositor.CreateHostBackdropBrush()?)?;
            // ...nhưng chỉ hiện trong khung viên thuốc (mép được khử răng cưa)
            let geometry = compositor.CreateRoundedRectangleGeometry()?;
            visual.SetClip(&compositor.CreateGeometricClipWithGeometry(&geometry)?)?;
            target.SetRoot(&visual)?;

            Ok(Self {
                hwnd,
                geometry,
                shape: Cell::new((0.0, 0.0, 0.0)), // chưa có hình: không thấy gì
                _visual: visual,
                _target: target,
                _compositor: compositor,
                _queue: queue,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    /// Cắt lớp kính theo viên thuốc mép trái `left`, rộng `w`, cao `h`, bo góc `radius`
    pub fn set_shape(&self, left: f32, w: f32, h: f32, radius: f32) {
        if (left, w, h) == self.shape.get() {
            return;
        }
        self.shape.set((left, w, h));
        let g = &self.geometry;
        let _ = g.SetOffset(Vector2 { X: left, Y: 0.0 });
        let _ = g.SetSize(Vector2 { X: w, Y: h });
        let _ = g.SetCornerRadius(Vector2 {
            X: radius,
            Y: radius,
        });
    }

    pub fn show(&self, visible: bool) {
        let cmd = if visible { SW_SHOWNOACTIVATE } else { SW_HIDE };
        unsafe {
            let _ = ShowWindow(self.hwnd, cmd);
        }
    }
}
