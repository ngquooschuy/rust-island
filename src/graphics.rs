//! Vẽ island bằng Direct2D lên swap chain, ghép vào cửa sổ qua DirectComposition
//! (nhờ vậy nền cửa sổ trong suốt thật sự).

use std::mem::ManuallyDrop;
// Kiểu điểm 2D mà Direct2D dùng (crate windows-numerics đi kèm crate windows)
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
use windows_numerics::Vector2;

use crate::battery::Battery;
use crate::config::{BackdropKind, Color, Config};
use crate::content::{self, Content, LyricsView, Page, Progress};
use crate::layout::{BUTTON_COUNT, LYRICS_BUTTON, Layout};
use crate::weather::{Sky, Weather};

/// Màu cố định cho trạng thái pin (theo màu hệ thống của Apple)
const CHARGING_GREEN: Color = Color::rgb(0x30, 0xD1, 0x58);
const LOW_RED: Color = Color::rgb(0xFF, 0x45, 0x3A);

/// Kích thước biểu tượng pin ở tỉ lệ 1.0
const BATTERY_BODY: (f32, f32) = (20.0, 10.0);
const BATTERY_NUB_W: f32 = 2.6;

/// Kích thước khung biểu tượng thời tiết ở tỉ lệ 1.0
const WEATHER_ICON: (f32, f32) = (27.0, 20.0);

/// Mọi thứ cần để vẽ một khung hình (gom lại cho gọn tham số)
pub struct Frame<'a> {
    /// Kích thước island lúc này (đang co giãn thì là giá trị trung gian)
    pub w: f32,
    pub h: f32,
    pub content: &'a Content,
    pub page: Page,
    pub hover_button: Option<usize>,
    /// Người dùng đang bật hiện lời bài hát
    pub lyrics_on: bool,
    /// Câu lời hiện trên island thu gọn (None: hiện tên bài như thường)
    pub compact_lyric: Option<CompactLyric<'a>>,
}

/// Câu lời trên island thu gọn đang trượt vào
pub struct CompactLyric<'a> {
    pub current: &'a str,
    /// Câu cũ đang trượt lên và mờ đi
    pub previous: Option<&'a str>,
    /// 0 = vừa đổi câu, 1 = đã trượt xong
    pub progress: f32,
}

/// Câu lời trên island thu gọn: lề hai bên, và quãng trượt lúc đổi câu
const COMPACT_PAD: f32 = 22.0;
const SLIDE_DISTANCE: f32 = 16.0;

/// Dòng lời trước/sau mờ hơn dòng đang hát
const LYRIC_DIM: f32 = 0.55;

/// Hạt nhiễu của nền Acrylic: ô vuông cạnh bao nhiêu điểm ảnh (lát lặp lại), và độ đậm
const NOISE_SIZE: u32 = 128;
const NOISE_OPACITY: f32 = 0.035;

/// Chấm phân trang
const DOT: f32 = 4.0;
const DOT_ACTIVE_W: f32 = 12.0;
const DOT_GAP: f32 = 5.0;

pub struct Graphics {
    swap_chain: IDXGISwapChain1,
    d2d_context: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    brush: ID2D1SolidColorBrush,
    /// Lớp hạt nhiễu phủ lên nền khi dùng Acrylic (None với nền khác)
    noise: Option<ID2D1BitmapBrush1>,
    fmt_small: IDWriteTextFormat,
    fmt_large: IDWriteTextFormat,
    fmt_title: IDWriteTextFormat,
    fmt_date: IDWriteTextFormat,
    fmt_icon: IDWriteTextFormat,
    fmt_lyric: IDWriteTextFormat,
    fmt_head: IDWriteTextFormat,
    fmt_tiny: IDWriteTextFormat,
    layout: Layout,
    background: Color,
    text: Color,
    secondary: Color,
    accent: Color,
    /// Ô bên phải của Idle View hiện thời tiết (true) hay pin (false)
    weather_enabled: bool,
    /// Có nút bật/tắt lời bài hát trên hàng nút không
    lyrics_enabled: bool,
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
        // Không bao giờ xuống dòng: chữ dài quá thì tràn chứ không vỡ thành 2 dòng
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

/// Ô hạt nhiễu xám ngẫu nhiên (BGRA), lát lặp lại kín nền island như kính mờ Acrylic
fn noise_pixels() -> Vec<u8> {
    // Bộ sinh số giả ngẫu nhiên xorshift: đủ "lộn xộn" cho mắt, không cần crate rand
    let mut state: u32 = 0x9E37_79B9;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 17;
        state ^= state << 5;
        state
    };
    let n = (NOISE_SIZE * NOISE_SIZE) as usize;
    (0..n)
        .flat_map(|_| {
            let v = (next() >> 24) as u8;
            [v, v, v, 0xFF]
        })
        .collect()
}

fn create_noise_brush(dc: &ID2D1DeviceContext) -> Result<ID2D1BitmapBrush1> {
    let pixels = noise_pixels();
    let props = D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
        colorContext: ManuallyDrop::new(None),
    };
    unsafe {
        let bitmap = dc.CreateBitmap(
            D2D_SIZE_U {
                width: NOISE_SIZE,
                height: NOISE_SIZE,
            },
            Some(pixels.as_ptr().cast()),
            NOISE_SIZE * 4,
            &props,
        )?;
        let brush = dc.CreateBitmapBrush(
            &bitmap,
            Some(&D2D1_BITMAP_BRUSH_PROPERTIES1 {
                extendModeX: D2D1_EXTEND_MODE_WRAP,
                extendModeY: D2D1_EXTEND_MODE_WRAP,
                interpolationMode: D2D1_INTERPOLATION_MODE_NEAREST_NEIGHBOR,
            }),
            None,
        )?;
        brush.SetOpacity(NOISE_OPACITY);
        Ok(brush)
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

fn rounded(r: D2D_RECT_F, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: r,
        radiusX: radius,
        radiusY: radius,
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

            let noise = if config.window.backdrop == BackdropKind::Acrylic {
                Some(create_noise_brush(&d2d_context)?)
            } else {
                None
            };

            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let family = HSTRING::from(config.font.family.as_str());
            let fmt_small = make_format(&dwrite, &family, 16.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_large = make_format(&dwrite, &family, 32.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_title = make_format(&dwrite, &family, 22.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_date = make_format(&dwrite, &family, 14.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            // Icon ⏮ ▶ ⏭ luôn dùng Segoe UI cho chắc có đủ ký tự
            let fmt_icon = make_format(&dwrite, h!("Segoe UI"), 18.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let fmt_lyric = make_format(&dwrite, &family, 20.0, DWRITE_FONT_WEIGHT_BOLD)?;
            let fmt_head = make_format(&dwrite, &family, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_tiny = make_format(&dwrite, &family, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?;

            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let dcomp_target = dcomp_device.CreateTargetForHwnd(hwnd, true)?;
            let dcomp_visual = dcomp_device.CreateVisual()?;
            dcomp_visual.SetContent(&swap_chain)?;
            dcomp_target.SetRoot(&dcomp_visual)?;
            dcomp_device.Commit()?;

            Ok(Self {
                swap_chain,
                d2d_context,
                dwrite,
                brush,
                noise,
                fmt_small,
                fmt_large,
                fmt_title,
                fmt_date,
                fmt_icon,
                fmt_lyric,
                fmt_head,
                fmt_tiny,
                layout,
                background: config.colors.background,
                text: config.colors.text,
                secondary: config.colors.secondary,
                accent: config.colors.accent,
                weather_enabled: config.weather.enabled,
                lyrics_enabled: config.lyrics.enabled,
                _dcomp_device: dcomp_device,
                _dcomp_target: dcomp_target,
                _dcomp_visual: dcomp_visual,
            })
        }
    }

    // ---------- Các hàm vẽ cơ bản ----------

    /// Đặt màu cọ; độ đục của màu (#RRGGBBAA) nhân với `opacity`
    fn set_brush(&self, color: Color, opacity: f32) {
        let (r, g, b, a) = color.to_f32();
        unsafe {
            self.brush.SetColor(&D2D1_COLOR_F { r, g, b, a });
            self.brush.SetOpacity(opacity);
        }
    }

    /// Vẽ "lỗ khoét" bằng màu nền: ghi đè thẳng điểm ảnh thay vì phủ lên, để khi nền
    /// trong suốt thì chỗ khoét trông y như nền island (phủ lên thì sẽ thấy hình bên dưới)
    fn knockout(&self, draw: impl FnOnce()) {
        self.set_brush(self.background, 1.0);
        unsafe {
            self.d2d_context
                .SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_COPY)
        };
        draw();
        unsafe {
            self.d2d_context
                .SetPrimitiveBlend(D2D1_PRIMITIVE_BLEND_SOURCE_OVER)
        };
    }

    fn fill(&self, shape: D2D1_ROUNDED_RECT) {
        unsafe { self.d2d_context.FillRoundedRectangle(&shape, &self.brush) };
    }

    fn stroke(&self, shape: D2D1_ROUNDED_RECT, width: f32) {
        unsafe {
            self.d2d_context
                .DrawRoundedRectangle(&shape, &self.brush, width, None)
        };
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

    /// Chiều rộng thật của chuỗi khi vẽ bằng `format` (để căn icon cạnh chữ)
    fn text_width(&self, text: &str, format: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut metrics = DWRITE_TEXT_METRICS::default();
        let measured = unsafe {
            self.dwrite
                .CreateTextLayout(&wide, format, 1000.0, 100.0)
                .and_then(|layout| layout.GetMetrics(&mut metrics))
        };
        if measured.is_ok() {
            metrics.widthIncludingTrailingWhitespace
        } else {
            0.0
        }
    }

    /// Chọn cỡ chữ đầu tiên trong `formats` vừa `max_w`; không cỡ nào vừa thì
    /// dùng cỡ cuối cùng và cắt bớt thêm "…" (tìm nhị phân độ dài vừa nhất)
    fn fit_text<'f>(
        &self,
        text: &str,
        formats: &[&'f IDWriteTextFormat],
        max_w: f32,
    ) -> (String, &'f IDWriteTextFormat) {
        for &f in formats {
            if self.text_width(text, f) <= max_w {
                return (text.to_string(), f);
            }
        }
        let f = formats[formats.len() - 1];
        let chars: Vec<char> = text.chars().collect();
        let cut = |n: usize| {
            let head: String = chars[..n].iter().collect();
            format!("{}…", head.trim_end())
        };
        // `lo` luôn là độ dài vừa (0 ký tự + "…" coi như vừa)
        let (mut lo, mut hi) = (0, chars.len());
        while lo < hi {
            let mid = (lo + hi).div_ceil(2);
            if self.text_width(&cut(mid), f) <= max_w {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        (cut(lo), f)
    }

    /// Island thu gọn cần rộng bao nhiêu để vừa câu lời `line`
    /// (không hẹp hơn lúc thu gọn thường, không rộng hơn lúc mở rộng)
    pub fn compact_width(&self, line: &str) -> f32 {
        let (collapsed, expanded) = (self.layout.collapsed.0, self.layout.expanded.0);
        let (text, format) = self.fit_text(line, &[&self.fmt_small], expanded - 2.0 * COMPACT_PAD);
        (self.text_width(&text, format) + 2.0 * COMPACT_PAD)
            .ceil()
            .clamp(collapsed, expanded)
    }

    // ---------- Khung hình ----------

    /// Vẽ một khung hình
    pub fn render(&self, frame: &Frame) -> Result<()> {
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
            self.draw_island(frame);
            self.d2d_context.EndDraw(None, None)?;
            self.d2d_context.SetTarget(None);

            self.swap_chain.Present(1, DXGI_PRESENT(0)).ok()?;
            Ok(())
        }
    }

    // Thân viên thuốc, nội dung của trang hiện tại, rồi chấm phân trang.
    // Khi là notch thì chỉ còn thân viên thuốc
    fn draw_island(&self, frame: &Frame) {
        let Frame {
            w,
            h,
            content,
            page,
            ..
        } = *frame;
        let left = self.layout.island_left(w);
        let pill = rounded(rect(left, 0.0, left + w, h), Layout::corner_radius(h));
        self.set_brush(self.background, 1.0);
        self.fill(pill);
        if let Some(noise) = &self.noise {
            unsafe { self.d2d_context.FillRoundedRectangle(&pill, noise) };
        }
        // Viền sáng rất mảnh cho island tách khỏi nền tối
        let inset = rounded(
            rect(left + 0.5, 0.5, left + w - 0.5, h - 0.5),
            pill.radiusX - 0.5,
        );
        self.set_brush(self.text, 0.08);
        self.stroke(inset, 1.0);

        // t: 0 = thu gọn hẳn, 1 = mở rộng hẳn. Chữ nhỏ mờ dần ở nửa đầu, chữ lớn hiện dần ở nửa sau.
        // Tính theo chiều cao: chiều rộng khi thu gọn thay đổi theo câu lời đang hiện
        let (collapsed, expanded) = (self.layout.collapsed.1, self.layout.expanded.1);
        let t = ((h - collapsed) / (expanded - collapsed)).clamp(0.0, 1.0);
        let alpha_big = (2.0 * t - 1.0).clamp(0.0, 1.0);

        // Notch Auto-Hide: `shown` 0 = notch, 1 = thu gọn bình thường.
        // Nội dung mờ dần và biến mất hẳn ở nửa đường, lúc viên thuốc còn đủ chỗ chứa
        let (notch_h, collapsed_h) = (self.layout.notch.1, self.layout.collapsed.1);
        let shown = ((h - notch_h) / (collapsed_h - notch_h)).clamp(0.0, 1.0);
        let reveal = (2.0 * shown - 1.0).clamp(0.0, 1.0);
        let alpha_small = (1.0 - 2.0 * t).clamp(0.0, 1.0) * reveal;

        // Cắt theo khung viên thuốc để chữ không lòi ra ngoài lúc đang co giãn
        unsafe {
            self.d2d_context
                .PushAxisAlignedClip(&pill.rect, D2D1_ANTIALIAS_MODE_ALIASED)
        };
        if alpha_small > 0.0 {
            match page {
                Page::Idle => self.draw_idle_compact(content, left, w, h, alpha_small),
                Page::Media => self.draw_media_compact(frame, left, alpha_small),
            }
        }
        if alpha_big > 0.0 {
            match page {
                Page::Idle => self.draw_idle_full(content, alpha_big),
                Page::Media => self.draw_media_full(frame, alpha_big),
            }
        }
        unsafe { self.d2d_context.PopAxisAlignedClip() };
        if reveal > 0.0 {
            self.draw_dots(page, h, reveal);
        }
    }

    /// Hàng chấm phân trang: chấm của trang hiện tại kéo dài thành viên thuốc nhỏ
    fn draw_dots(&self, page: Page, h: f32, alpha: f32) {
        let (cx, cy) = self.layout.dots_center(h);
        let n = Page::ALL.len() as f32;
        let total = DOT_ACTIVE_W + (n - 1.0) * (DOT + DOT_GAP);
        let mut x = cx - total / 2.0;
        for p in Page::ALL {
            let (width, color, opacity) = if p == page {
                (DOT_ACTIVE_W, self.text, 0.9)
            } else {
                (DOT, self.secondary, 0.45)
            };
            self.set_brush(color, opacity * alpha);
            self.fill(rounded(
                rect(x, cy - DOT / 2.0, x + width, cy + DOT / 2.0),
                DOT / 2.0,
            ));
            x += width + DOT_GAP;
        }
    }

    /// Vạch ngăn dọc mảnh giữa hai nửa
    fn draw_divider(&self, x: f32, cy: f32, half_h: f32, alpha: f32) {
        self.set_brush(self.secondary, alpha * 0.35);
        self.fill(rounded(
            rect(x - 0.75, cy - half_h, x + 0.75, cy + half_h),
            0.75,
        ));
    }

    // ---------- Pin ----------

    /// Biểu tượng pin có tâm dọc `cy`, mép trái `x`, phóng to `s` lần
    fn draw_battery_icon(&self, x: f32, cy: f32, s: f32, battery: Battery, alpha: f32) {
        let (bw, bh) = (BATTERY_BODY.0 * s, BATTERY_BODY.1 * s);
        let top = cy - bh / 2.0;

        // Vỏ và núm
        self.set_brush(self.secondary, alpha * 0.9);
        self.stroke(rounded(rect(x, top, x + bw, top + bh), 2.8 * s), 1.2 * s);
        let nub_x = x + bw + 1.0 * s;
        self.fill(rounded(
            rect(nub_x, cy - 2.0 * s, nub_x + 1.6 * s, cy + 2.0 * s),
            0.8 * s,
        ));

        // Ruột: dài theo phần trăm, màu theo trạng thái
        let color = if battery.charging {
            CHARGING_GREEN
        } else if battery.is_low() {
            LOW_RED
        } else {
            self.accent
        };
        let pad = 2.2 * s;
        let full = bw - 2.0 * pad;
        let level = (full * f32::from(battery.percent) / 100.0).max(1.5 * s);
        self.set_brush(color, alpha);
        self.fill(rounded(
            rect(x + pad, top + pad, x + pad + level, top + bh - pad),
            1.2 * s,
        ));
    }

    // ---------- Thời tiết ----------

    fn circle(&self, cx: f32, cy: f32, r: f32) {
        let ellipse = D2D1_ELLIPSE {
            point: Vector2 { X: cx, Y: cy },
            radiusX: r,
            radiusY: r,
        };
        unsafe { self.d2d_context.FillEllipse(&ellipse, &self.brush) };
    }

    fn line(&self, (x0, y0): (f32, f32), (x1, y1): (f32, f32), width: f32) {
        let (p0, p1) = (Vector2 { X: x0, Y: y0 }, Vector2 { X: x1, Y: y1 });
        unsafe { self.d2d_context.DrawLine(p0, p1, &self.brush, width, None) };
    }

    /// Đám mây gồm ba vòng tròn trên một đế bo tròn; khung 21.5×13 ở tỉ lệ 1.
    /// `at(dx, dy)` đổi tọa độ trong khung biểu tượng sang tọa độ màn hình
    fn cloud(&self, at: &impl Fn(f32, f32) -> (f32, f32), ox: f32, oy: f32, s: f32) {
        let (l, t) = at(ox + 2.0, oy + 7.5);
        let (r, b) = at(ox + 21.0, oy + 13.0);
        self.fill(rounded(rect(l, t, r, b), 2.75 * s));
        for (dx, dy, radius) in [(7.0, 8.0, 4.5), (13.0, 6.0, 5.5), (17.5, 8.5, 3.8)] {
            let (cx, cy) = at(ox + dx, oy + dy);
            self.circle(cx, cy, radius * s);
        }
    }

    /// Mặt trời (ngày) hoặc trăng khuyết (đêm) tâm (cx, cy) bán kính r
    fn sun_or_moon(&self, cx: f32, cy: f32, r: f32, is_day: bool) {
        self.circle(cx, cy, r);
        if !is_day {
            // Khoét một hình tròn màu nền lệch lên trên-phải để thành trăng khuyết
            self.knockout(|| self.circle(cx + 0.45 * r, cy - 0.35 * r, 0.85 * r));
        }
    }

    /// Biểu tượng thời tiết vẽ bằng hình khối như thiết kế (khung 27×20 ở tỉ lệ 1),
    /// mép trái `x`, tâm dọc `cy`
    #[allow(clippy::too_many_arguments)]
    fn draw_weather_icon(
        &self,
        x: f32,
        cy: f32,
        s: f32,
        sky: Sky,
        is_day: bool,
        color: Color,
        alpha: f32,
    ) {
        let top = cy - WEATHER_ICON.1 * s / 2.0;
        let at = |dx: f32, dy: f32| (x + dx * s, top + dy * s);
        let solid = |opacity: f32| self.set_brush(color, alpha * opacity);

        match sky {
            Sky::Clear => {
                let (cx, cy) = at(13.5, 10.0);
                solid(1.0);
                self.sun_or_moon(cx, cy, 7.0 * s, is_day);
            }
            Sky::PartlyCloudy => {
                // Mặt trời nhạt hơn một chút, đám mây đè lên phía trước như ảnh mẫu
                let (cx, cy) = at(9.0, 7.5);
                solid(0.85);
                self.sun_or_moon(cx, cy, 6.0 * s, is_day);
                solid(1.0);
                self.cloud(&at, 5.5, 6.5, s);
            }
            Sky::Cloudy => {
                solid(1.0);
                self.cloud(&at, 2.5, 3.5, s);
            }
            Sky::Fog => {
                solid(1.0);
                self.cloud(&at, 2.5, 1.0, s);
                solid(0.8);
                for (x0, x1, y) in [(4.0, 23.0, 16.2), (7.0, 21.0, 19.0)] {
                    self.line(at(x0, y), at(x1, y), 1.4 * s);
                }
            }
            Sky::Drizzle | Sky::Rain => {
                solid(1.0);
                self.cloud(&at, 2.5, 0.0, s);
                let (xs, len): (&[f32], f32) = if sky == Sky::Rain {
                    (&[8.5, 13.5, 18.5], 4.5)
                } else {
                    (&[10.0, 17.0], 2.5)
                };
                for &dx in xs {
                    // Giọt mưa hơi nghiêng
                    self.line(at(dx, 15.0), at(dx - 1.2, 15.0 + len), 1.5 * s);
                }
            }
            Sky::Snow => {
                solid(1.0);
                self.cloud(&at, 2.5, 0.0, s);
                for (dx, dy) in [(8.5, 17.0), (13.5, 18.5), (18.5, 17.0)] {
                    let (cx, cy) = at(dx, dy);
                    self.circle(cx, cy, 1.3 * s);
                }
            }
            Sky::Thunder => {
                solid(1.0);
                self.cloud(&at, 2.5, 0.0, s);
                let bolt = [(14.5, 13.5), (11.5, 17.0), (15.0, 17.0), (12.0, 20.5)];
                for pair in bolt.windows(2) {
                    self.line(at(pair[0].0, pair[0].1), at(pair[1].0, pair[1].1), 1.6 * s);
                }
            }
        }
    }

    // ---------- Ô bên phải của Idle View ----------

    /// Vẽ cụm "biểu tượng + chữ", cả cụm căn giữa trong [x0, x1]
    #[allow(clippy::too_many_arguments)]
    fn draw_icon_and_label(
        &self,
        (x0, x1): (f32, f32),
        (top, bottom): (f32, f32),
        icon_w: f32,
        gap: f32,
        label: &str,
        format: &IDWriteTextFormat,
        alpha: f32,
        draw_icon: impl Fn(f32, f32),
    ) {
        let cy = (top + bottom) / 2.0;
        let text_w = self.text_width(label, format) + 2.0;
        let start = (x0 + x1) / 2.0 - (icon_w + gap + text_w) / 2.0;
        draw_icon(start, cy);
        let text_x = start + icon_w + gap;
        self.set_brush(self.text, alpha);
        self.draw_text(label, format, rect(text_x, top, text_x + text_w, bottom));
    }

    /// Thời tiết (mặc định); nếu tắt thời tiết thì pin, máy không có pin thì thứ/ngày
    fn draw_status(
        &self,
        content: &Content,
        span: (f32, f32),
        rows: (f32, f32),
        scale: f32,
        format: &IDWriteTextFormat,
        alpha: f32,
    ) {
        if self.weather_enabled {
            // Chưa tải xong (hoặc mất mạng): đám mây xám và "--°"
            let placeholder = Weather {
                temperature: f32::NAN,
                sky: Sky::Cloudy,
                is_day: true,
                place: String::new(),
            };
            let (w, label, color, opacity) = match &content.weather {
                Some(w) => (w, w.temperature_label(), self.accent, 1.0),
                None => (&placeholder, "--°".to_string(), self.secondary, 0.6),
            };
            let icon_w = WEATHER_ICON.0 * scale;
            self.draw_icon_and_label(
                span,
                rows,
                icon_w,
                10.0 * scale,
                &label,
                format,
                alpha,
                |x, cy| {
                    self.draw_weather_icon(x, cy, scale, w.sky, w.is_day, color, alpha * opacity)
                },
            );
        } else if let Some(battery) = content.battery {
            let icon_w = (BATTERY_BODY.0 + BATTERY_NUB_W) * scale;
            self.draw_icon_and_label(
                span,
                rows,
                icon_w,
                7.0 * scale,
                &format!("{}%", battery.percent),
                format,
                alpha,
                |x, cy| self.draw_battery_icon(x, cy, scale, battery, alpha),
            );
        } else {
            self.set_brush(self.text, alpha);
            let ((x0, x1), (top, bottom)) = (span, rows);
            self.draw_text(&content.day_short, format, rect(x0, top, x1, bottom));
        }
    }

    // ---------- Trang Idle ----------

    /// Thu gọn: "20:24 │ ☁ 31°" như thiết kế Idle View
    fn draw_idle_compact(&self, content: &Content, left: f32, w: f32, h: f32, alpha: f32) {
        let (top, bottom) = (2.0, h - 12.0); // chừa đáy cho chấm phân trang
        let cy = (top + bottom) / 2.0;
        let mid = left + w / 2.0;

        self.set_brush(self.text, alpha);
        self.draw_text(
            &content.clock,
            &self.fmt_small,
            rect(left + 10.0, top, mid, bottom),
        );
        self.draw_divider(mid, cy, 9.0, alpha);
        self.draw_status(
            content,
            (mid, left + w - 10.0),
            (top, bottom),
            1.0,
            &self.fmt_small,
            alpha,
        );
    }

    /// Mép trái và phải của island khi mở rộng hẳn
    fn expanded_edges(&self) -> (f32, f32) {
        let w = self.layout.expanded.0;
        let left = self.layout.island_left(w);
        (left, left + w)
    }

    /// Mở rộng: đồng hồ lớn │ thời tiết lớn, dòng ngày · nơi · pin bên dưới
    fn draw_idle_full(&self, content: &Content, alpha: f32) {
        let (el, right) = self.expanded_edges();
        let mid = (el + right) / 2.0;
        let (top, bottom) = (10.0, 66.0);

        self.set_brush(self.text, alpha);
        self.draw_text(
            &content.long,
            &self.fmt_large,
            rect(el + 12.0, top, mid, bottom),
        );
        self.draw_divider(mid, (top + bottom) / 2.0, 16.0, alpha);
        self.draw_status(
            content,
            (mid, right - 12.0),
            (top, bottom),
            1.6,
            &self.fmt_title,
            alpha,
        );

        self.set_brush(self.secondary, alpha);
        self.draw_text(
            &content.detail_line(),
            &self.fmt_date,
            rect(el, 66.0, right, 92.0),
        );
    }

    // ---------- Trang Media ----------

    /// Thu gọn: câu lời đang hát (nếu bật trong menu), không thì "♪ Tên bài"
    fn draw_media_compact(&self, frame: &Frame, left: f32, alpha: f32) {
        let (content, w, h) = (frame.content, frame.w, frame.h);
        let area = rect(left + 14.0, 2.0, left + w - 14.0, h - 12.0);
        if content.title.is_empty() {
            self.set_brush(self.secondary, alpha);
            self.draw_text("Không có nhạc", &self.fmt_small, area);
        } else if let Some(lyric) = &frame.compact_lyric {
            self.draw_compact_lyric(lyric, left + w / 2.0, h, alpha);
        } else {
            // Tạm dừng thì chữ nhạt đi
            let color = if content.playing {
                self.text
            } else {
                self.secondary
            };
            self.set_brush(color, alpha);
            self.draw_text(&content.song, &self.fmt_small, area);
        }
    }

    /// Câu lời căn giữa tại `mid`: câu cũ trượt lên và mờ đi, câu mới trượt từ dưới lên.
    /// Phần chữ lòi ra ngoài island (lúc island đang giãn theo câu mới) bị khung cắt đi
    fn draw_compact_lyric(&self, lyric: &CompactLyric, mid: f32, h: f32, alpha: f32) {
        let max_w = self.layout.expanded.0 - 2.0 * COMPACT_PAD;
        // Khung chữ rộng hẳn ra hai bên: chữ luôn căn giữa island, không bị cắt sớm
        let line = |text: &str, dy: f32, opacity: f32| {
            let (text, format) = self.fit_text(text, &[&self.fmt_small], max_w);
            self.set_brush(self.text, alpha * opacity);
            let r = rect(mid - max_w, 2.0 + dy, mid + max_w, h - 12.0 + dy);
            self.draw_text(&text, format, r);
        };
        let p = lyric.progress;
        if let Some(previous) = lyric.previous.filter(|_| p < 1.0) {
            line(previous, -SLIDE_DISTANCE * p, 1.0 - p);
        }
        line(lyric.current, SLIDE_DISTANCE * (1.0 - p), p);
    }

    fn draw_media_full(&self, frame: &Frame, alpha: f32) {
        let content = frame.content;
        let (el, right) = self.expanded_edges();
        // Khung cao (có lời) khi đang bật lời, hoặc khi island vẫn đang cao (vừa tắt lời)
        let tall = frame.h > self.layout.expanded.1 + 0.5;
        if content.shows_lyrics_panel(frame.page, frame.lyrics_on)
            || (tall && content.shows_lyrics_panel(frame.page, true))
        {
            self.draw_media_lyrics(frame, alpha);
            return;
        }
        if content.title.is_empty() {
            self.set_brush(self.secondary, alpha);
            self.draw_text(
                "Không có nhạc đang phát",
                &self.fmt_date,
                rect(el, 10.0, right, 92.0),
            );
            return;
        }
        self.set_brush(self.text, alpha);
        self.draw_text(&content.title, &self.fmt_title, rect(el, 10.0, right, 46.0));
        self.set_brush(self.secondary, alpha);
        self.draw_text(&content.artist, &self.fmt_date, rect(el, 44.0, right, 68.0));
        self.draw_controls(frame, alpha);
    }

    /// Hàng nút ở đáy island: lùi, phát/dừng, tới, và nút bật/tắt lời bên phải
    fn draw_controls(&self, frame: &Frame, alpha: f32) {
        let content = frame.content;
        let glyphs = ["⏮", if content.playing { "⏸" } else { "▶" }, "⏭"];
        let hover_bg = |i: usize, r: D2D_RECT_F| {
            if frame.hover_button == Some(i) {
                self.set_brush(self.text, alpha * 0.18);
                self.fill(rounded(r, 10.0));
            }
        };
        for (i, glyph) in glyphs.iter().enumerate().take(BUTTON_COUNT) {
            let r = self.layout.button_rect(i, frame.h);
            hover_bg(i, r);
            self.set_brush(self.text, alpha);
            self.draw_text(glyph, &self.fmt_icon, r);
        }
        if self.lyrics_enabled {
            let r = self.layout.button_rect(LYRICS_BUTTON, frame.h);
            hover_bg(LYRICS_BUTTON, r);
            self.draw_lyrics_icon(r, frame.lyrics_on, alpha);
        }
    }

    /// Biểu tượng bong bóng lời thoại: màu nhấn khi đang bật lời, xám khi tắt
    fn draw_lyrics_icon(&self, r: D2D_RECT_F, on: bool, alpha: f32) {
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
        let (color, opacity) = if on {
            (self.accent, 1.0)
        } else {
            (self.secondary, 0.7)
        };
        self.set_brush(color, alpha * opacity);
        self.fill(rounded(rect(cx - 9.0, cy - 7.5, cx + 9.0, cy + 5.0), 3.5));
        self.line((cx - 4.5, cy + 4.0), (cx - 7.0, cy + 8.5), 3.0); // đuôi bong bóng
        // Hai dòng chữ khoét bằng màu nền
        self.knockout(|| {
            self.fill(rounded(rect(cx - 5.5, cy - 4.0, cx + 5.5, cy - 2.6), 0.7));
            self.fill(rounded(rect(cx - 5.5, cy - 0.6, cx + 2.5, cy + 0.8), 0.7));
        });
    }

    /// Trang Media có khung lời: tiêu đề · nghệ sĩ, 3 dòng lời, thanh tiến độ, hàng nút
    fn draw_media_lyrics(&self, frame: &Frame, alpha: f32) {
        let content = frame.content;
        let (el, right) = self.expanded_edges();
        self.draw_header(content, (el, right), alpha);

        let max_w = right - el - 40.0;
        let off = LyricsView::Message("Lời bài hát đang tắt");
        let lyrics = if frame.lyrics_on {
            &content.lyrics
        } else {
            &off
        };
        match lyrics {
            LyricsView::Lines {
                prev,
                current,
                next,
            } => {
                let dim_line = |text: &Option<String>, top: f32, bottom: f32| {
                    if let Some(text) = text {
                        let (t, f) = self.fit_text(text, &[&self.fmt_date], max_w);
                        self.set_brush(self.secondary, alpha * LYRIC_DIM);
                        self.draw_text(&t, f, rect(el, top, right, bottom));
                    }
                };
                dim_line(prev, 38.0, 58.0);
                // Dòng đang hát: chữ đậm 20px, dài quá thì xuống 16px rồi mới cắt bớt
                let (t, f) = self.fit_text(current, &[&self.fmt_lyric, &self.fmt_small], max_w);
                self.set_brush(self.text, alpha);
                self.draw_text(&t, f, rect(el, 58.0, right, 90.0));
                dim_line(next, 90.0, 110.0);
            }
            LyricsView::Message(message) => {
                self.set_brush(self.secondary, alpha);
                self.draw_text(message, &self.fmt_date, rect(el, 38.0, right, 110.0));
            }
            LyricsView::Off => {}
        }

        if let Some(progress) = content.progress {
            self.draw_progress(progress, (el, right), frame.h - 66.0, alpha);
        }
        self.draw_controls(frame, alpha);
    }

    /// "Tên bài · Nghệ sĩ" căn giữa, tên bài trắng, nghệ sĩ xám; chật thì cắt nghệ sĩ trước
    fn draw_header(&self, content: &Content, (el, right): (f32, f32), alpha: f32) {
        let max_w = right - el - 48.0;
        let (title, title_f) = self.fit_text(&content.title, &[&self.fmt_head], max_w);
        let title_w = self.text_width(&title, title_f) + 1.0;
        let room = max_w - title_w;
        let artist = (!content.artist.is_empty() && room > 50.0).then(|| {
            let (a, _) =
                self.fit_text(&format!("  ·  {}", content.artist), &[&self.fmt_date], room);
            let w = self.text_width(&a, &self.fmt_date) + 1.0;
            (a, w)
        });
        let artist_w = artist.as_ref().map_or(0.0, |(_, w)| *w);
        let start = (el + right) / 2.0 - (title_w + artist_w) / 2.0;
        let (top, bottom) = (10.0, 34.0);
        self.set_brush(self.text, alpha);
        self.draw_text(&title, title_f, rect(start, top, start + title_w, bottom));
        if let Some((a, w)) = artist {
            self.set_brush(self.secondary, alpha);
            let x = start + title_w;
            self.draw_text(&a, &self.fmt_date, rect(x, top, x + w, bottom));
        }
    }

    /// "0:29 ━━━●──────── -1:56" tại tâm dọc `cy`
    fn draw_progress(&self, p: Progress, (el, right): (f32, f32), cy: f32, alpha: f32) {
        let (top, bottom) = (cy - 8.0, cy + 8.0);
        self.set_brush(self.secondary, alpha);
        let remaining = p.duration_ms.saturating_sub(p.elapsed_ms);
        self.draw_text(
            &content::mmss(p.elapsed_ms),
            &self.fmt_tiny,
            rect(el + 14.0, top, el + 58.0, bottom),
        );
        self.draw_text(
            &format!("-{}", content::mmss(remaining)),
            &self.fmt_tiny,
            rect(right - 58.0, top, right - 14.0, bottom),
        );

        let (x0, x1) = (el + 64.0, right - 64.0);
        let x = x0 + (x1 - x0) * p.fraction();
        let bar = |from: f32, to: f32| rounded(rect(from, cy - 2.0, to, cy + 2.0), 2.0);
        self.set_brush(self.secondary, alpha * 0.3);
        self.fill(bar(x0, x1));
        self.set_brush(self.accent, alpha);
        self.fill(bar(x0, x.max(x0 + 4.0)));
        self.set_brush(self.text, alpha);
        self.circle(x, cy, 5.0);
    }
}
