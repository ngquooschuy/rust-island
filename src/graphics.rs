//! Vẽ island bằng Direct2D lên swap chain, ghép vào cửa sổ qua DirectComposition
//! (nhờ vậy nền cửa sổ trong suốt thật sự).

use std::mem::ManuallyDrop;
use windows::{
    Win32::{
        Foundation::*,
        Graphics::{
            Direct2D::{Common::*, *},
            Direct3D::*,
            Direct3D11::*,
            DirectComposition::*,
            DirectWrite::*,
            Dxgi::{Common::*, *},
        },
    },
    core::{HSTRING, Interface, Result, h, w},
};

use crate::config::{Color, Config};
use crate::content::Content;
use crate::layout::Layout;

pub struct Graphics {
    swap_chain: IDXGISwapChain1,
    d2d_context: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    fmt_small: IDWriteTextFormat,
    fmt_large: IDWriteTextFormat,
    fmt_title: IDWriteTextFormat,
    fmt_date: IDWriteTextFormat,
    fmt_icon: IDWriteTextFormat,
    layout: Layout,
    background: Color,
    text: Color,
    secondary: Color,
    // Không dùng trực tiếp nhưng phải giữ sống, nếu không hình sẽ biến mất
    _dcomp_device: IDCompositionDevice,
    _dcomp_target: IDCompositionTarget,
    _dcomp_visual: IDCompositionVisual,
}

fn make_format(
    factory: &IDWriteFactory,
    family: &HSTRING,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat> {
    unsafe {
        // Font không có trên máy thì DirectWrite tự dùng font thay thế
        let format = factory.CreateTextFormat(
            family,
            None,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("vi-vn"),
        )?;
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        Ok(format)
    }
}

fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left,
        top,
        right,
        bottom,
    }
}

impl Graphics {
    pub fn new(hwnd: HWND, layout: Layout, config: &Config) -> Result<Self> {
        let (width, height) = (layout.win_w as u32, layout.win_h as u32);
        unsafe {
            let mut d3d_device: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                None,
            )?;
            let d3d_device = d3d_device.expect("Không tạo được thiết bị D3D11");

            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let adapter = dxgi_device.GetAdapter()?;
            let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;

            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: width,
                Height: height,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                Stereo: false.into(),
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                Flags: 0,
            };
            let swap_chain =
                dxgi_factory.CreateSwapChainForComposition(&d3d_device, &desc, None)?;

            let d2d_factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let d2d_device = d2d_factory.CreateDevice(&dxgi_device)?;
            let d2d_context = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            d2d_context.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);

            let brush = d2d_context.CreateSolidColorBrush(
                &D2D1_COLOR_F {
                    r: 0.0,
                    g: 0.0,
                    b: 0.0,
                    a: 1.0,
                },
                None,
            )?;

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let family = HSTRING::from(config.font.family.as_str());
            let fmt_small = make_format(&dwrite, &family, 16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_large = make_format(&dwrite, &family, 38.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_title = make_format(&dwrite, &family, 22.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_date = make_format(&dwrite, &family, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            // Icon ⏮ ▶ ⏭ luôn dùng Segoe UI cho chắc có đủ ký tự
            let fmt_icon = make_format(&dwrite, h!("Segoe UI"), 18.0, DWRITE_FONT_WEIGHT_NORMAL)?;

            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let dcomp_target = dcomp_device.CreateTargetForHwnd(hwnd, true)?;
            let dcomp_visual = dcomp_device.CreateVisual()?;
            dcomp_visual.SetContent(&swap_chain)?;
            dcomp_target.SetRoot(&dcomp_visual)?;
            dcomp_device.Commit()?;

            Ok(Self {
                swap_chain,
                d2d_context,
                brush,
                fmt_small,
                fmt_large,
                fmt_title,
                fmt_date,
                fmt_icon,
                layout,
                background: config.colors.background,
                text: config.colors.text,
                secondary: config.colors.secondary,
                _dcomp_device: dcomp_device,
                _dcomp_target: dcomp_target,
                _dcomp_visual: dcomp_visual,
            })
        }
    }

    fn set_brush(&self, color: Color, opacity: f32) {
        let (r, g, b) = color.to_f32();
        unsafe {
            self.brush.SetColor(&D2D1_COLOR_F { r, g, b, a: 1.0 });
            self.brush.SetOpacity(opacity);
        }
    }

    fn draw_text(&self, text: &str, format: &IDWriteTextFormat, rect: D2D_RECT_F) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.d2d_context.DrawText(
                &wide,
                format,
                &rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    /// Vẽ một khung hình: island rộng `w`, cao `h`
    pub fn render(
        &self,
        w: f32,
        h: f32,
        content: &Content,
        hover_button: Option<usize>,
    ) -> Result<()> {
        unsafe {
            let surface: IDXGISurface = self.swap_chain.GetBuffer(0)?;
            let props = D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                colorContext: ManuallyDrop::new(None),
            };
            let bitmap = self
                .d2d_context
                .CreateBitmapFromDxgiSurface(&surface, Some(&props))?;
            self.d2d_context.SetTarget(&bitmap);

            self.d2d_context.BeginDraw();
            self.d2d_context.Clear(Some(&D2D1_COLOR_F {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            }));
            self.draw_island(w, h, content, hover_button);
            self.d2d_context.EndDraw(None, None)?;
            self.d2d_context.SetTarget(None);

            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
            Ok(())
        }
    }

    // Phần thân viên thuốc, rồi chữ của trạng thái thu gọn/mở rộng
    fn draw_island(&self, w: f32, h: f32, content: &Content, hover_button: Option<usize>) {
        let left = self.layout.island_left(w);
        let radius = (h / 2.0).min(36.0);
        let pill = D2D1_ROUNDED_RECT {
            rect: rect(left, 0.0, left + w, h),
            radiusX: radius,
            radiusY: radius,
        };
        self.set_brush(self.background, 1.0);
        unsafe { self.d2d_context.FillRoundedRectangle(&pill, &self.brush) };

        // t: 0 = thu gọn hẳn, 1 = mở rộng hẳn. Chữ nhỏ mờ dần ở nửa đầu, chữ lớn hiện dần ở nửa sau
        let (collapsed, expanded) = (self.layout.collapsed.0, self.layout.expanded.0);
        let t = ((w - collapsed) / (expanded - collapsed)).clamp(0.0, 1.0);
        let alpha_small = (1.0 - 2.0 * t).clamp(0.0, 1.0);
        let alpha_big = (2.0 * t - 1.0).clamp(0.0, 1.0);

        if alpha_small > 0.0 {
            self.set_brush(self.text, alpha_small);
            self.draw_text(
                &content.short,
                &self.fmt_small,
                rect(left, 0.0, left + w, h),
            );
        }
        if alpha_big > 0.0 {
            if content.title.is_empty() {
                self.draw_clock(content, alpha_big);
            } else {
                self.draw_media(content, alpha_big, hover_button);
            }
        }
    }

    /// Mép trái và phải của island khi mở rộng hẳn
    fn expanded_edges(&self) -> (f32, f32) {
        let w = self.layout.expanded.0;
        let left = self.layout.island_left(w);
        (left, left + w)
    }

    fn draw_clock(&self, content: &Content, alpha: f32) {
        let (el, right) = self.expanded_edges();
        self.set_brush(self.text, alpha);
        self.draw_text(&content.long, &self.fmt_large, rect(el, 10.0, right, 66.0));
        self.set_brush(self.secondary, alpha);
        self.draw_text(&content.date, &self.fmt_date, rect(el, 62.0, right, 90.0));
    }

    fn draw_media(&self, content: &Content, alpha: f32, hover_button: Option<usize>) {
        let (el, right) = self.expanded_edges();
        self.set_brush(self.text, alpha);
        self.draw_text(&content.title, &self.fmt_title, rect(el, 10.0, right, 46.0));
        self.set_brush(self.secondary, alpha);
        self.draw_text(&content.artist, &self.fmt_date, rect(el, 44.0, right, 68.0));

        let glyphs = ["⏮", if content.playing { "⏸" } else { "▶" }, "⏭"];
        for (i, glyph) in glyphs.iter().enumerate() {
            let rect = self.layout.button_rect(i);
            if hover_button == Some(i) {
                self.set_brush(self.text, alpha * 0.18);
                let bg = D2D1_ROUNDED_RECT {
                    rect,
                    radiusX: 10.0,
                    radiusY: 10.0,
                };
                unsafe { self.d2d_context.FillRoundedRectangle(&bg, &self.brush) };
            }
            self.set_brush(self.text, alpha);
            self.draw_text(glyph, &self.fmt_icon, rect);
        }
    }
}
