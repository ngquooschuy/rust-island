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
use windows_numerics::{Matrix3x2, Vector2};

use crate::alerts::{Alert, Kind};
use crate::battery::Battery;
use crate::config::{BackdropKind, Color, Config};
use crate::content::{self, Content, LyricsView, Page, Progress};
use crate::icon::Icon;
use crate::layout::{
    Expanded, LOOKUP_ROWS_MAX, LYRICS_BUTTON, Layout, PLAYER_COVER, PLAYER_COVER_TOP, PLAYER_PAD,
    WEATHER_CARD_GAP, WEATHER_CARD_H, WEATHER_CARDS_TOP,
};
use crate::lookup;
use crate::meter::{WAVE_BARS, Wave};
use crate::palette;
use crate::weather::{self, Sky, Weather};

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
    /// Trang Media mở rộng có khung lời (cao hơn, có 3 dòng lời)
    pub lyrics_panel: bool,
    /// Ảnh bìa bài đang phát (đã đưa lên card đồ họa)
    pub cover: Option<&'a ID2D1Bitmap1>,
    /// Màu nhấn lấy từ ảnh bìa, cho trang Media Player (None: dùng màu trong Cài đặt)
    pub cover_accent: Option<Color>,
    /// Sóng nhạc của trang Media Player
    pub wave: &'a Wave,
    /// Câu lời hiện trên island thu gọn (None: hiện tên bài như thường)
    pub compact_lyric: Option<CompactLyric<'a>>,
    /// Thông báo tạm thời đang hiện (hoặc đang mờ đi)
    pub alert: Option<AlertView<'a>>,
    /// Hộp Tra cứu nhanh đang mở (thay cho nội dung trang)
    pub lookup: Option<LookupView<'a>>,
}

/// Hộp Tra cứu nhanh cần vẽ
pub struct LookupView<'a> {
    pub session: &'a lookup::Session,
    pub suggestions: &'a [lookup::Suggestion],
    /// Con trỏ nhấp nháy: đang ở nhịp hiện
    pub caret_on: bool,
    /// Đã nhập Account ID và token chưa
    pub configured: bool,
    /// Chuột đang ở dòng gợi ý nào, có trên nút "Mới" không
    pub hover_row: Option<usize>,
    pub hover_new: bool,
}

/// Khung chat: khoảng cách giữa câu hỏi và câu trả lời, giữa hai lượt hỏi
const CHAT_ANSWER_GAP: f32 = 4.0;
const CHAT_TURN_GAP: f32 = 14.0;

/// Thông báo tạm thời cần vẽ
pub struct AlertView<'a> {
    pub alert: &'a Alert,
    /// Biểu tượng đã đưa lên card đồ họa (None: vẽ biểu tượng mặc định)
    pub icon: Option<&'a ID2D1Bitmap1>,
    /// 0 = chưa hiện, 1 = hiện rõ; nội dung thường của island mờ theo chiều ngược lại
    pub alpha: f32,
}

/// Biểu tượng trong thông báo tạm thời: cạnh ô vuông và lề trái
const ALERT_ICON: f32 = 44.0;
const ALERT_PAD: f32 = 20.0;

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
const LYRIC_DIM: f32 = 0.38;
/// Phần chưa hát của dòng đang hát (karaoke) mờ cỡ này
const KARAOKE_UNSUNG: f32 = 0.42;

// Trang Media mở rộng: lề hai bên và tâm dọc của từng phần (khung lời cao 248)
const SIDE_PAD: f32 = 24.0;
const HEADER_CY: f32 = 30.0;
const LYRIC_PREV_CY: f32 = 74.0;
const LYRIC_CURRENT_CY: f32 = 108.0;
const LYRIC_NEXT_CY: f32 = 142.0;
/// Thanh tiến độ cách đáy bao nhiêu (hàng nút ở dưới nó, xem layout.rs)
const PROGRESS_FROM_BOTTOM: f32 = 72.0;
/// Độ đậm nền của nút tròn (chữ màu trắng phủ mờ lên nền island)
const BUTTON_BG: f32 = 0.09;
const PLAY_BG: f32 = 0.14;

/// Trang Media Player: lớp màu nhấn phủ nhẹ lên nền (Accent Wash), đậm nhất ở góc trên trái
const WASH_OPACITY: f32 = 0.22;
/// Sóng nhạc: bề ngang cả cụm, cách mép phải island, bề rộng mỗi vạch, cao thấp nhất/nhất
const WAVE_W: f32 = 62.0;
const WAVE_FROM_RIGHT: f32 = 42.0;
const WAVE_BAR_W: f32 = 3.5;
const WAVE_MIN_H: f32 = 4.0;
const WAVE_MAX_H: f32 = 34.0;
/// Nút nhỏ trên góc ảnh bìa (cho biết đang phát hay dừng)
const BADGE_R: f32 = 12.0;
const BADGE_BG: Color = Color::rgb(0x16, 0x16, 0x18);

/// Trang Thời tiết: độ đậm nền thẻ (màu chữ phủ mờ lên nền island) và biểu tượng trong thẻ
const CARD_BG: f32 = 0.06;
const CARD_ICON_OPACITY: f32 = 0.8;

/// Hạt nhiễu của nền Acrylic: ô vuông cạnh bao nhiêu điểm ảnh (lát lặp lại), và độ đậm
const NOISE_SIZE: u32 = 128;
const NOISE_OPACITY: f32 = 0.035;

/// Chấm phân trang
const DOT: f32 = 4.0;
const DOT_ACTIVE_W: f32 = 12.0;
const DOT_GAP: f32 = 5.0;

pub struct Graphics {
    swap_chain: IDXGISwapChain1,
    /// Để tạo hình tam giác cho các nút
    factory: ID2D1Factory1,
    d2d_context: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    brush: ID2D1SolidColorBrush,
    /// Lớp hạt nhiễu phủ lên nền khi dùng Acrylic (None với nền khác)
    noise: Option<ID2D1BitmapBrush1>,
    fmt_small: IDWriteTextFormat,
    fmt_large: IDWriteTextFormat,
    fmt_title: IDWriteTextFormat,
    fmt_date: IDWriteTextFormat,
    fmt_lyric: IDWriteTextFormat,
    fmt_head: IDWriteTextFormat,
    fmt_tiny: IDWriteTextFormat,
    /// Chữ căn trái cho thông báo tạm thời: tên app, tiêu đề, nội dung
    fmt_alert_app: IDWriteTextFormat,
    /// Tên bài lớn, căn trái (trang Media không có khung lời)
    fmt_media_title: IDWriteTextFormat,
    /// Nhiệt độ lớn, căn trái (trang Thời tiết)
    fmt_temp: IDWriteTextFormat,
    fmt_alert_title: IDWriteTextFormat,
    fmt_alert_body: IDWriteTextFormat,
    /// Biểu tượng dạng chữ của Windows (Segoe MDL2 Assets) khi app không có biểu tượng
    fmt_symbol: IDWriteTextFormat,
    /// Biểu tượng nhỏ trong hộp Tra cứu nhanh
    fmt_symbol_small: IDWriteTextFormat,
    /// Ô nhập, câu hỏi, câu trả lời của Tra cứu nhanh
    fmt_input: IDWriteTextFormat,
    fmt_chat_q: IDWriteTextFormat,
    fmt_chat_a: IDWriteTextFormat,
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

/// Ảnh Direct2D từ điểm ảnh BGRA đã nhân độ đục
fn create_bitmap(
    dc: &ID2D1DeviceContext,
    width: u32,
    height: u32,
    pixels: &[u8],
) -> Result<ID2D1Bitmap1> {
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
        dc.CreateBitmap(
            D2D_SIZE_U { width, height },
            Some(pixels.as_ptr().cast()),
            width * 4,
            &props,
        )
    }
}

/// Định dạng chữ căn trái (các định dạng khác căn giữa)
/// Chữ căn trái, căn trên, tự xuống dòng (khung chat của Tra cứu nhanh)
fn wrapping(format: IDWriteTextFormat) -> Result<IDWriteTextFormat> {
    unsafe {
        format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
    }
    Ok(format)
}

fn left_aligned(format: IDWriteTextFormat) -> Result<IDWriteTextFormat> {
    unsafe { format.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING)? };
    Ok(format)
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
            let fmt_lyric = make_format(&dwrite, &family, 20.0, DWRITE_FONT_WEIGHT_BOLD)?;
            let fmt_head = make_format(&dwrite, &family, 14.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?;
            let fmt_tiny = make_format(&dwrite, &family, 11.0, DWRITE_FONT_WEIGHT_NORMAL)?;
            let fmt_alert_app = left_aligned(make_format(
                &dwrite,
                &family,
                12.5,
                DWRITE_FONT_WEIGHT_NORMAL,
            )?)?;
            let fmt_media_title = left_aligned(make_format(
                &dwrite,
                &family,
                18.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
            )?)?;
            let fmt_temp = left_aligned(make_format(
                &dwrite,
                &family,
                46.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
            )?)?;
            let fmt_alert_title = left_aligned(make_format(
                &dwrite,
                &family,
                15.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
            )?)?;
            let fmt_alert_body = left_aligned(make_format(
                &dwrite,
                &family,
                13.5,
                DWRITE_FONT_WEIGHT_NORMAL,
            )?)?;
            let fmt_symbol = make_format(
                &dwrite,
                h!("Segoe MDL2 Assets"),
                20.0,
                DWRITE_FONT_WEIGHT_NORMAL,
            )?;
            let fmt_symbol_small = make_format(
                &dwrite,
                h!("Segoe MDL2 Assets"),
                14.0,
                DWRITE_FONT_WEIGHT_NORMAL,
            )?;
            let fmt_input = left_aligned(make_format(
                &dwrite,
                &family,
                15.0,
                DWRITE_FONT_WEIGHT_NORMAL,
            )?)?;
            let fmt_chat_q = wrapping(make_format(
                &dwrite,
                &family,
                14.5,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
            )?)?;
            let fmt_chat_a = wrapping(make_format(
                &dwrite,
                &family,
                13.5,
                DWRITE_FONT_WEIGHT_NORMAL,
            )?)?;

            let dcomp_device: IDCompositionDevice = DCompositionCreateDevice(&dxgi_device)?;
            let dcomp_target = dcomp_device.CreateTargetForHwnd(hwnd, true)?;
            let dcomp_visual = dcomp_device.CreateVisual()?;
            dcomp_visual.SetContent(&swap_chain)?;
            dcomp_target.SetRoot(&dcomp_visual)?;
            dcomp_device.Commit()?;

            Ok(Self {
                swap_chain,
                factory: d2d_factory,
                d2d_context,
                dwrite,
                brush,
                noise,
                fmt_small,
                fmt_large,
                fmt_title,
                fmt_date,
                fmt_lyric,
                fmt_head,
                fmt_tiny,
                fmt_alert_app,
                fmt_media_title,
                fmt_temp,
                fmt_alert_title,
                fmt_alert_body,
                fmt_symbol,
                fmt_symbol_small,
                fmt_input,
                fmt_chat_q,
                fmt_chat_a,
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

    /// Đưa biểu tượng (64px) lên card đồ họa để vẽ được
    pub fn icon_bitmap(&self, icon: &Icon) -> Option<ID2D1Bitmap1> {
        create_bitmap(&self.d2d_context, icon.width, icon.height, &icon.pixels).ok()
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

        // Thông báo tạm thời hiện rõ dần, nội dung thường mờ đi tương ứng
        let alert_alpha = frame.alert.as_ref().map_or(0.0, |a| a.alpha);
        let rest = 1.0 - alert_alpha;
        let (alpha_small, alpha_big, reveal) =
            (alpha_small * rest, alpha_big * rest, reveal * rest);

        // Cắt theo khung viên thuốc để chữ không lòi ra ngoài lúc đang co giãn
        unsafe {
            self.d2d_context
                .PushAxisAlignedClip(&pill.rect, D2D1_ANTIALIAS_MODE_ALIASED)
        };
        if alpha_small > 0.0 {
            match page {
                Page::Idle | Page::Weather => {
                    self.draw_idle_compact(content, left, w, h, alpha_small)
                }
                Page::Media | Page::Player => self.draw_media_compact(frame, left, alpha_small),
            }
        }
        if let Some(view) = frame.lookup.as_ref().filter(|_| alpha_big > 0.0) {
            self.draw_lookup(view, alpha_big);
        } else if alpha_big > 0.0 {
            match page {
                Page::Idle => self.draw_idle_full(content, alpha_big),
                Page::Weather => self.draw_weather_full(content, alpha_big),
                Page::Media => self.draw_media_full(frame, alpha_big),
                Page::Player => self.draw_player_full(frame, alpha_big),
            }
        }
        if let Some(view) = frame.alert.as_ref().filter(|a| a.alpha > 0.0) {
            self.draw_alert(view);
        }
        unsafe { self.d2d_context.PopAxisAlignedClip() };
        // Hộp Tra cứu nhanh không thuộc trang nào: không vẽ chấm phân trang
        if reveal > 0.0 && frame.lookup.is_none() {
            self.draw_dots(page, w, h, reveal);
        }
    }

    // ---------- Thông báo tạm thời ----------

    /// [biểu tượng]  Tên app
    ///               Tiêu đề
    ///               Nội dung
    /// Bố cục theo kích thước cuối của thông báo, căn giữa cửa sổ; lúc island còn
    /// đang giãn thì khung island cắt bớt phần chưa đủ chỗ
    fn draw_alert(&self, view: &AlertView) {
        let (w, h) = self.layout.alert_size();
        let left = self.layout.island_left(w);
        let alpha = view.alpha;
        let cy = h / 2.0;

        let icon_box = rect(
            left + ALERT_PAD,
            cy - ALERT_ICON / 2.0,
            left + ALERT_PAD + ALERT_ICON,
            cy + ALERT_ICON / 2.0,
        );
        match (view.icon, &view.alert.icon) {
            (Some(bitmap), Some(icon)) => self.draw_icon(bitmap, icon, icon_box, alpha),
            _ => self.draw_fallback_icon(view.alert.kind, icon_box, alpha),
        }

        let x0 = icon_box.right + 14.0;
        let x1 = left + w - ALERT_PAD;
        let max_w = x1 - x0;
        let line = |text: &str, format: &IDWriteTextFormat, color: Color, top: f32, bottom: f32| {
            if text.is_empty() {
                return;
            }
            let (t, f) = self.fit_text(text, &[format], max_w);
            self.set_brush(color, alpha);
            self.draw_text(&t, f, rect(x0, top, x1, bottom));
        };
        let a = view.alert;
        // Không có nội dung thì tiêu đề nằm giữa hơn
        let shift = if a.body.is_empty() { 9.0 } else { 0.0 };
        line(
            &a.app,
            &self.fmt_alert_app,
            self.secondary,
            cy - 31.0 + shift,
            cy - 13.0 + shift,
        );
        line(
            &a.title,
            &self.fmt_alert_title,
            self.text,
            cy - 13.0 + shift,
            cy + 8.0 + shift,
        );
        line(
            &a.body,
            &self.fmt_alert_body,
            self.secondary,
            cy + 8.0,
            cy + 27.0,
        );
    }

    /// Vẽ ảnh vừa khít ô (giữ tỉ lệ), thu nhỏ chất lượng cao từ bản 64px
    fn draw_icon(&self, bitmap: &ID2D1Bitmap1, icon: &Icon, area: D2D_RECT_F, alpha: f32) {
        let (bw, bh) = (area.right - area.left, area.bottom - area.top);
        let scale = (bw / icon.width as f32).min(bh / icon.height as f32);
        let (w, h) = (icon.width as f32 * scale, icon.height as f32 * scale);
        let x = area.left + (bw - w) / 2.0;
        let y = area.top + (bh - h) / 2.0;
        unsafe {
            self.d2d_context.DrawBitmap(
                bitmap,
                Some(&rect(x, y, x + w, y + h)),
                alpha,
                D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                None,
                None,
            );
        }
    }

    /// Ô bo góc với biểu tượng clipboard hoặc cái chuông
    fn draw_fallback_icon(&self, kind: Kind, area: D2D_RECT_F, alpha: f32) {
        self.set_brush(self.secondary, alpha * 0.25);
        self.fill(rounded(area, 10.0));
        let glyph = match kind {
            Kind::Clipboard => "\u{E77F}",    // Paste
            Kind::Notification => "\u{EA8F}", // Ringer
        };
        self.set_brush(self.text, alpha);
        self.draw_text(glyph, &self.fmt_symbol, area);
    }

    /// Hàng chấm phân trang: chấm của trang hiện tại kéo dài thành viên thuốc nhỏ.
    /// Thu gọn thì nằm ngang ở đáy, mở rộng thì dựng dọc ở mép phải (đổi dần theo chiều cao)
    fn draw_dots(&self, page: Page, w: f32, h: f32, alpha: f32) {
        let (collapsed, expanded) = (self.layout.collapsed.1, self.layout.expanded.1);
        let v = ((h - collapsed) / (expanded - collapsed)).clamp(0.0, 1.0);
        if alpha * (1.0 - v) > 0.0 {
            self.dot_row(
                page,
                self.layout.dots_row_center(h),
                false,
                alpha * (1.0 - v),
            );
        }
        if alpha * v > 0.0 {
            self.dot_row(page, self.layout.dots_column_center(w, h), true, alpha * v);
        }
    }

    fn dot_row(&self, page: Page, (cx, cy): (f32, f32), vertical: bool, alpha: f32) {
        let pages = Page::list(self.weather_enabled);
        let n = pages.len() as f32;
        let total = DOT_ACTIVE_W + (n - 1.0) * (DOT + DOT_GAP);
        let mut pos = if vertical { cy } else { cx } - total / 2.0;
        for &p in pages {
            let (len, color, opacity) = if p == page {
                (DOT_ACTIVE_W, self.text, 0.9)
            } else {
                (DOT, self.secondary, 0.45)
            };
            self.set_brush(color, opacity * alpha);
            let r = if vertical {
                rect(cx - DOT / 2.0, pos, cx + DOT / 2.0, pos + len)
            } else {
                rect(pos, cy - DOT / 2.0, pos + len, cy + DOT / 2.0)
            };
            self.fill(rounded(r, DOT / 2.0));
            pos += len + DOT_GAP;
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
                feels_like: None,
                humidity: None,
                tomorrow: None,
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

    // ---------- Tra cứu nhanh ----------

    /// Đoạn chữ tự xuống dòng trong bề rộng `width`: (bố cục để vẽ, chiều cao)
    fn wrapped(
        &self,
        text: &str,
        format: &IDWriteTextFormat,
        width: f32,
    ) -> Option<(IDWriteTextLayout, f32)> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let layout = self
                .dwrite
                .CreateTextLayout(&wide, format, width.max(1.0), 100_000.0)
                .ok()?;
            let mut m = DWRITE_TEXT_METRICS::default();
            layout.GetMetrics(&mut m).ok()?;
            Some((layout, m.height))
        }
    }

    /// Các khối chữ của cuộc trò chuyện, từ trên xuống: (bố cục, chiều cao, màu, độ đậm,
    /// khoảng cách phía trên)
    fn chat_blocks(
        &self,
        session: &lookup::Session,
        width: f32,
    ) -> Vec<(IDWriteTextLayout, f32, Color, f32, f32)> {
        let mut blocks = Vec::new();
        for (i, turn) in session.turns.iter().enumerate() {
            let gap = if i == 0 { 0.0 } else { CHAT_TURN_GAP };
            if let Some((l, h)) = self.wrapped(&turn.question, &self.fmt_chat_q, width) {
                blocks.push((l, h, self.text, 1.0, gap));
            }
            let (text, color, opacity) = match &turn.answer {
                lookup::Answer::Pending => {
                    // Ba chấm chạy lần lượt cho biết đang chờ
                    let dots = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis())
                        / 400
                        % 4) as usize;
                    (
                        format!("Đang trả lời{}", ".".repeat(dots)),
                        self.secondary,
                        1.0,
                    )
                }
                lookup::Answer::Text(text) => (text.clone(), self.text, 0.86),
                lookup::Answer::Error(e) => (format!("⚠ {e}"), LOW_RED, 1.0),
            };
            if let Some((l, h)) = self.wrapped(&text, &self.fmt_chat_a, width) {
                blocks.push((l, h, color, opacity, CHAT_ANSWER_GAP));
            }
        }
        blocks
    }

    /// Tổng chiều cao cuộc trò chuyện (để app biết cuộn lên được bao xa)
    pub fn chat_height(&self, session: &lookup::Session) -> f32 {
        let body = self.layout.lookup_body();
        self.chat_blocks(session, body.right - body.left)
            .iter()
            .map(|b| b.1 + b.4)
            .sum()
    }

    /// Hộp Tra cứu nhanh:
    ///   ( 🔍 Hỏi AI…                      [Mới] )
    ///   Câu hỏi
    ///   Câu trả lời, tự xuống dòng, cuộn được
    /// Chưa hỏi gì thì dưới ô nhập là các dòng gợi ý: nội dung clipboard và lịch sử tìm kiếm
    fn draw_lookup(&self, view: &LookupView, alpha: f32) {
        let session = view.session;
        let input = self.layout.lookup_input();
        let has_chat = !session.turns.is_empty();

        // Ô nhập
        self.set_brush(self.text, alpha * 0.08);
        self.fill(rounded(input, 12.0));
        self.set_brush(self.secondary, alpha);
        self.draw_text(
            "\u{E721}", // kính lúp
            &self.fmt_symbol_small,
            rect(input.left + 8.0, input.top, input.left + 34.0, input.bottom),
        );
        let x0 = input.left + 38.0;
        let x1 = if has_chat {
            self.layout.lookup_new_button().left - 8.0
        } else {
            input.right - 12.0
        };
        let text = session.editor.text();
        let caret_x = self.text_width(&session.editor.before_caret(), &self.fmt_input);
        // Chữ dài hơn ô thì trượt sang trái cho con trỏ luôn nằm trong ô
        let offset = (caret_x - (x1 - x0) + 2.0).max(0.0);
        unsafe {
            self.d2d_context.PushAxisAlignedClip(
                &rect(x0 - 1.0, input.top, x1, input.bottom),
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
        }
        if text.is_empty() {
            self.set_brush(self.secondary, alpha * 0.8);
            let hint = if view.configured {
                "Hỏi AI bất cứ điều gì…"
            } else {
                "Chưa cấu hình AI"
            };
            self.draw_text(hint, &self.fmt_input, rect(x0, input.top, x1, input.bottom));
        } else {
            let full_w = self.text_width(&text, &self.fmt_input);
            self.set_brush(self.text, alpha);
            self.draw_text(
                &text,
                &self.fmt_input,
                rect(
                    x0 - offset,
                    input.top,
                    x0 - offset + full_w + 40.0,
                    input.bottom,
                ),
            );
        }
        if view.caret_on {
            let x = x0 - offset + caret_x;
            let cy = (input.top + input.bottom) / 2.0;
            self.set_brush(self.accent, alpha);
            self.fill(rounded(rect(x, cy - 10.0, x + 1.6, cy + 10.0), 0.8));
        }
        unsafe { self.d2d_context.PopAxisAlignedClip() };

        if has_chat {
            let b = self.layout.lookup_new_button();
            let hover = if view.hover_new { 0.08 } else { 0.0 };
            self.set_brush(self.text, alpha * (0.1 + hover));
            self.fill(rounded(b, 8.0));
            let label_w = self.text_width("Mới", &self.fmt_alert_app);
            let lx = (b.left + b.right - label_w) / 2.0;
            self.set_brush(self.text, alpha);
            self.draw_text(
                "Mới",
                &self.fmt_alert_app,
                rect(lx, b.top, b.right, b.bottom),
            );
            self.draw_chat(session, alpha);
        } else {
            self.draw_suggestions(view, alpha);
        }
    }

    /// Cuộc trò chuyện, dính đáy (câu mới nhất ở dưới cùng), cuộn lên để xem câu cũ
    fn draw_chat(&self, session: &lookup::Session, alpha: f32) {
        let body = self.layout.lookup_body();
        let width = body.right - body.left;
        let blocks = self.chat_blocks(session, width);
        let total: f32 = blocks.iter().map(|b| b.1 + b.4).sum();
        let visible = body.bottom - body.top;
        let max_scroll = (total - visible).max(0.0);
        let scroll = session.scroll.clamp(0.0, max_scroll);
        // Ngắn hơn khung thì bắt đầu từ trên; dài hơn thì đáy chữ chạm đáy khung
        let mut y = if total <= visible {
            body.top
        } else {
            body.bottom - total + scroll
        };
        unsafe {
            self.d2d_context
                .PushAxisAlignedClip(&body, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
        }
        for (layout, h, color, opacity, gap) in &blocks {
            y += gap;
            if y + h >= body.top && y <= body.bottom {
                self.set_brush(*color, alpha * opacity);
                unsafe {
                    self.d2d_context.DrawTextLayout(
                        Vector2 { X: body.left, Y: y },
                        layout,
                        &self.brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
            y += h;
        }
        unsafe { self.d2d_context.PopAxisAlignedClip() };
        // Còn chữ phía trên: mờ dần ở mép trên để biết cuộn lên được
        if scroll < max_scroll {
            let fade = rect(body.left, body.top - 2.0, body.right, body.top + 14.0);
            let stops = [(0.0, self.background, 1.0), (1.0, self.background, 0.0)];
            if let Some(brush) = self.gradient(&stops, (0.0, fade.top), (0.0, fade.bottom)) {
                unsafe {
                    brush.SetOpacity(alpha);
                    self.d2d_context.FillRectangle(&fade, &brush);
                }
            }
        }
    }

    /// Dòng gợi ý: nội dung clipboard (nếu là chữ) rồi tới các câu đã hỏi gần đây
    fn draw_suggestions(&self, view: &LookupView, alpha: f32) {
        let body = self.layout.lookup_body();
        if !view.configured {
            let msg = "Mở Cài đặt > Modules, nhập Account ID và API token của Cloudflare \
                       để bắt đầu hỏi AI.";
            if let Some((layout, _)) = self.wrapped(msg, &self.fmt_chat_a, body.right - body.left) {
                self.set_brush(self.secondary, alpha);
                unsafe {
                    self.d2d_context.DrawTextLayout(
                        Vector2 {
                            X: body.left,
                            Y: body.top + 4.0,
                        },
                        &layout,
                        &self.brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
            return;
        }
        if view.suggestions.is_empty() {
            self.set_brush(self.secondary, alpha * 0.8);
            self.draw_text(
                "Gõ câu hỏi rồi nhấn Enter  ·  Esc để đóng",
                &self.fmt_alert_app,
                rect(body.left + 4.0, body.top, body.right, body.top + 28.0),
            );
            return;
        }
        for (i, s) in view.suggestions.iter().take(LOOKUP_ROWS_MAX).enumerate() {
            let r = self.layout.lookup_row(i);
            let hover = if view.hover_row == Some(i) { 0.07 } else { 0.0 };
            self.set_brush(self.text, alpha * (0.045 + hover));
            self.fill(rounded(r, 8.0));
            let icon = match s.kind {
                lookup::SuggestionKind::Clipboard => "\u{E77F}", // dán
                lookup::SuggestionKind::History => "\u{E81C}",   // lịch sử
            };
            self.set_brush(self.secondary, alpha);
            self.draw_text(
                icon,
                &self.fmt_symbol_small,
                rect(r.left + 4.0, r.top, r.left + 30.0, r.bottom),
            );
            let x = r.left + 34.0;
            let (label, f) = self.fit_text(&s.text, &[&self.fmt_alert_body], r.right - 10.0 - x);
            self.set_brush(self.text, alpha * 0.9);
            self.draw_text(&label, f, rect(x, r.top, r.right - 10.0, r.bottom));
        }
    }

    // ---------- Trang Thời tiết ----------

    /// Trang Thời tiết mở rộng:
    ///   Hà Nội                │  [mây]  Ngày mai
    ///   [biểu tượng] 31°      │         60% khả năng có mưa
    ///                         │  [nhiệt kế] Cảm giác như
    ///   Có mây                │         34°
    ///                         │  [giọt nước] Độ ẩm
    ///                         │         42%
    fn draw_weather_full(&self, content: &Content, alpha: f32) {
        let (el, right) = self.expanded_edges();
        let Some(w) = &content.weather else {
            self.set_brush(self.secondary, alpha);
            self.draw_text(
                "Đang tải thời tiết…",
                &self.fmt_date,
                rect(el, 10.0, right, 92.0),
            );
            return;
        };
        let h = self.layout.expanded_size(Expanded::Weather).1;
        // Đường chia lệch trái một chút: thẻ bên phải đủ rộng cho "60% khả năng có mưa"
        let mid = el + (right - el) * 0.44;
        let x = el + 30.0;

        // Bên trái: thành phố, biểu tượng + nhiệt độ lớn, mô tả
        self.set_brush(self.secondary, alpha);
        self.draw_text(
            &w.place,
            &self.fmt_alert_body,
            rect(x, 24.0, mid - 12.0, 44.0),
        );
        let cy = 88.0;
        let scale = 1.9;
        let icon_w = WEATHER_ICON.0 * scale;
        self.draw_weather_icon(x, cy, scale, w.sky, w.is_day, self.accent, alpha);
        self.set_brush(self.text, alpha);
        let tx = x + icon_w + 6.0;
        self.draw_text(
            &w.temperature_label(),
            &self.fmt_temp,
            rect(tx, cy - 30.0, mid - 8.0, cy + 30.0),
        );
        let (sky, f) = self.fit_text(w.sky.describe(), &[&self.fmt_alert_title], mid - 12.0 - x);
        self.draw_text(&sky, f, rect(x, 128.0, mid - 12.0, 152.0));

        self.draw_divider(mid, h / 2.0, h / 2.0 - 26.0, alpha);

        // Bên phải: ba thẻ
        let (cx0, cx1) = (mid + 14.0, right - 30.0);
        let card = |i: usize| {
            let top = WEATHER_CARDS_TOP + i as f32 * (WEATHER_CARD_H + WEATHER_CARD_GAP);
            rect(cx0, top, cx1, top + WEATHER_CARD_H)
        };
        let dash = || "--".to_string();

        let rain = w.tomorrow.map_or_else(dash, |t| t.rain_label());
        // Có số liệu thì nối thêm chữ nhỏ; thẻ hẹp thì dùng bản ngắn
        let note: &[&str] = if w.tomorrow.is_some_and(|t| t.rain_chance.is_some()) {
            &[" khả năng có mưa", " có mưa"]
        } else {
            &[]
        };
        self.draw_weather_card(card(0), "Ngày mai", &rain, note, alpha, |ix, iy| {
            let sky = w.tomorrow.map_or(Sky::Cloudy, |t| t.sky);
            let s = 0.62;
            self.draw_weather_icon(
                ix - WEATHER_ICON.0 * s / 2.0,
                iy,
                s,
                sky,
                true,
                self.accent,
                alpha * CARD_ICON_OPACITY,
            );
        });

        let feels = w.feels_like.map_or_else(dash, weather::degrees);
        self.draw_weather_card(card(1), "Cảm giác như", &feels, &[], alpha, |ix, iy| {
            self.thermometer(ix, iy, alpha * CARD_ICON_OPACITY);
        });

        let humidity = w.humidity.map_or_else(dash, |h| format!("{h}%"));
        self.draw_weather_card(card(2), "Độ ẩm", &humidity, &[], alpha, |ix, iy| {
            self.droplet(ix, iy, alpha * CARD_ICON_OPACITY);
        });
    }

    /// Một thẻ: nền mờ bo góc, biểu tượng bên trái, nhãn nhỏ ở trên, số liệu đậm ở dưới.
    /// `note`: chữ nhỏ nối sau số liệu, thử lần lượt từng bản, lấy bản đầu tiên vừa chỗ
    /// (không bản nào vừa thì bỏ). `draw_icon(x, y)` vẽ biểu tượng có tâm (x, y)
    #[allow(clippy::too_many_arguments)]
    fn draw_weather_card(
        &self,
        r: D2D_RECT_F,
        label: &str,
        value: &str,
        note: &[&str],
        alpha: f32,
        draw_icon: impl Fn(f32, f32),
    ) {
        self.set_brush(self.text, alpha * CARD_BG);
        self.fill(rounded(r, 12.0));
        draw_icon(r.left + 20.0, r.top + 15.0);
        let x = r.left + 38.0;
        let max_w = r.right - 8.0 - x;
        let (label, f) = self.fit_text(label, &[&self.fmt_alert_app], max_w);
        self.set_brush(self.secondary, alpha);
        self.draw_text(&label, f, rect(x, r.top + 6.0, r.right - 8.0, r.top + 23.0));
        let (value, f) = self.fit_text(value, &[&self.fmt_alert_title], max_w);
        self.set_brush(self.text, alpha);
        let (top, bottom) = (r.top + 22.0, r.top + 42.0);
        self.draw_text(&value, f, rect(x, top, r.right - 8.0, bottom));
        let value_w = self.text_width(&value, f);
        let room = max_w - value_w;
        let fits = note
            .iter()
            .find(|n| self.text_width(n, &self.fmt_alert_app) <= room);
        if let Some(note) = fits {
            // Chữ nhỏ hơn nên nhích xuống một chút cho cùng đường chân chữ với số liệu
            let nx = x + value_w;
            self.set_brush(self.secondary, alpha);
            self.draw_text(
                note,
                &self.fmt_alert_app,
                rect(nx, top + 2.5, r.right - 8.0, bottom),
            );
        }
    }

    /// Nhiệt kế nhỏ, tâm (cx, cy)
    fn thermometer(&self, cx: f32, cy: f32, alpha: f32) {
        self.set_brush(self.accent, alpha);
        self.fill(rounded(rect(cx - 1.8, cy - 8.0, cx + 1.8, cy + 3.0), 1.8));
        self.circle(cx, cy + 4.5, 3.6);
    }

    /// Giọt nước nhỏ, tâm (cx, cy): tam giác nhọn ở trên, tròn ở dưới
    fn droplet(&self, cx: f32, cy: f32, alpha: f32) {
        self.set_brush(self.accent, alpha);
        self.triangle([(cx, cy - 8.0), (cx - 4.4, cy + 1.5), (cx + 4.4, cy + 1.5)]);
        self.circle(cx, cy + 2.6, 4.6);
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

    /// Trang Media mở rộng:
    ///   [ảnh bìa] Tên bài · Nghệ sĩ
    ///   (3 dòng lời, dòng giữa tô dần kiểu karaoke — chỉ khi có khung lời)
    ///   0:29 ━━━━●───────── -1:56
    ///        (⏪)  ( ▶ )  (⏩)          (💬)
    fn draw_media_full(&self, frame: &Frame, alpha: f32) {
        let content = frame.content;
        let (el, right) = self.expanded_edges();
        if content.title.is_empty() {
            self.set_brush(self.secondary, alpha);
            self.draw_text(
                "Không có nhạc đang phát",
                &self.fmt_date,
                rect(el, 10.0, right, 92.0),
            );
            return;
        }
        if frame.lyrics_panel {
            self.draw_header_line(frame, (el, right), alpha);
            self.draw_lyric_lines(frame, (el, right), alpha);
        } else {
            self.draw_header_big(frame, (el, right), alpha);
        }
        if let Some(progress) = content.progress {
            self.draw_progress(progress, (el, right), frame.h - PROGRESS_FROM_BOTTOM, alpha);
        }
        self.draw_controls(frame, alpha, self.accent, false);
    }

    /// Ảnh bìa bo góc trong ô `area`; chưa có ảnh thì vẽ ô xám với nốt nhạc
    fn draw_cover(&self, cover: Option<&ID2D1Bitmap1>, area: D2D_RECT_F, radius: f32, alpha: f32) {
        let shape = rounded(area, radius);
        let Some(bitmap) = cover else {
            self.set_brush(self.secondary, alpha * 0.3);
            self.fill(shape);
            self.set_brush(self.text, alpha * 0.8);
            self.draw_text("♪", &self.fmt_small, area);
            return;
        };
        // Tô hình bo góc bằng chính ảnh: phóng ảnh (96px) cho khít ô rồi dời vào chỗ
        let size = unsafe { bitmap.GetPixelSize() };
        let scale = (area.right - area.left) / size.width.max(1) as f32;
        let props = D2D1_BITMAP_BRUSH_PROPERTIES1 {
            extendModeX: D2D1_EXTEND_MODE_CLAMP,
            extendModeY: D2D1_EXTEND_MODE_CLAMP,
            interpolationMode: D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
        };
        unsafe {
            let Ok(brush) = self
                .d2d_context
                .CreateBitmapBrush(bitmap, Some(&props), None)
            else {
                return;
            };
            brush.SetTransform(&Matrix3x2 {
                M11: scale,
                M12: 0.0,
                M21: 0.0,
                M22: scale,
                M31: area.left,
                M32: area.top,
            });
            brush.SetOpacity(alpha);
            self.d2d_context.FillRoundedRectangle(&shape, &brush);
        }
    }

    /// Đầu khung lời: [ảnh bìa nhỏ] Tên bài · Nghệ sĩ, căn trái trên một dòng
    fn draw_header_line(&self, frame: &Frame, (el, right): (f32, f32), alpha: f32) {
        let content = frame.content;
        let cy = HEADER_CY;
        let x = el + SIDE_PAD;
        self.draw_cover(
            frame.cover,
            rect(x, cy - 15.0, x + 30.0, cy + 15.0),
            8.0,
            alpha,
        );

        let x0 = x + 30.0 + 12.0;
        let max_w = right - 40.0 - x0;
        let (title, title_f) = self.fit_text(&content.title, &[&self.fmt_alert_title], max_w);
        let title_w = self.text_width(&title, title_f) + 1.0;
        let (top, bottom) = (cy - 12.0, cy + 12.0);
        self.set_brush(self.text, alpha);
        self.draw_text(&title, title_f, rect(x0, top, x0 + title_w, bottom));
        // Nghệ sĩ nối sau tên bài; chật thì cắt nghệ sĩ trước
        let room = max_w - title_w;
        if !content.artist.is_empty() && room > 40.0 {
            let artist = format!("  ·  {}", content.artist);
            let (a, f) = self.fit_text(&artist, &[&self.fmt_alert_body], room);
            self.set_brush(self.secondary, alpha);
            let ax = x0 + title_w;
            self.draw_text(&a, f, rect(ax, top, ax + room, bottom));
        }
    }

    /// Đầu trang khi không có khung lời: ảnh bìa lớn, tên bài và nghệ sĩ hai dòng
    fn draw_header_big(&self, frame: &Frame, (el, right): (f32, f32), alpha: f32) {
        let content = frame.content;
        let x = el + SIDE_PAD;
        self.draw_cover(frame.cover, rect(x, 16.0, x + 48.0, 64.0), 10.0, alpha);
        let x0 = x + 48.0 + 14.0;
        let max_w = right - 36.0 - x0;
        let (title, f) = self.fit_text(&content.title, &[&self.fmt_media_title], max_w);
        self.set_brush(self.text, alpha);
        self.draw_text(&title, f, rect(x0, 16.0, x0 + max_w, 42.0));
        let (artist, f) = self.fit_text(&content.artist, &[&self.fmt_alert_body], max_w);
        self.set_brush(self.secondary, alpha);
        self.draw_text(&artist, f, rect(x0, 42.0, x0 + max_w, 62.0));
    }

    /// Ba dòng lời: dòng trước và sau mờ, dòng đang hát to và tô sáng dần từ trái sang
    fn draw_lyric_lines(&self, frame: &Frame, (el, right): (f32, f32), alpha: f32) {
        let off = LyricsView::Message("Lời bài hát đang tắt");
        let lyrics = if frame.lyrics_on {
            &frame.content.lyrics
        } else {
            &off
        };
        let max_w = right - el - 2.0 * (SIDE_PAD + 16.0);
        let band = |cy: f32| rect(el, cy - 14.0, right, cy + 14.0);
        match lyrics {
            LyricsView::Lines {
                prev,
                current,
                next,
                progress,
            } => {
                let dim_line = |text: &Option<String>, cy: f32| {
                    if let Some(text) = text {
                        let (t, f) = self.fit_text(text, &[&self.fmt_head], max_w);
                        self.set_brush(self.text, alpha * LYRIC_DIM);
                        self.draw_text(&t, f, band(cy));
                    }
                };
                dim_line(prev, LYRIC_PREV_CY);
                dim_line(next, LYRIC_NEXT_CY);
                self.draw_karaoke(current, *progress, (el + right) / 2.0, max_w, alpha);
            }
            LyricsView::Message(message) => {
                self.set_brush(self.secondary, alpha);
                self.draw_text(message, &self.fmt_date, band(LYRIC_CURRENT_CY));
            }
            LyricsView::Off => {}
        }
    }

    /// Dòng đang hát kiểu karaoke: cả dòng màu nhạt, phần đã hát (bên trái) phủ màu sáng.
    /// Chữ đậm 20px, dài quá thì xuống 16px rồi mới cắt bớt
    fn draw_karaoke(&self, line: &str, progress: f32, mid: f32, max_w: f32, alpha: f32) {
        let (text, format) = self.fit_text(line, &[&self.fmt_lyric, &self.fmt_small], max_w);
        let width = self.text_width(&text, format);
        let cy = LYRIC_CURRENT_CY;
        let area = rect(mid - max_w / 2.0, cy - 18.0, mid + max_w / 2.0, cy + 18.0);
        self.set_brush(self.text, alpha * KARAOKE_UNSUNG);
        self.draw_text(&text, format, area);
        if progress > 0.0 {
            // Chữ căn giữa nên mép trái của chữ ở giữa trừ nửa bề rộng
            let lit_right = mid - width / 2.0 + width * progress;
            unsafe {
                self.d2d_context.PushAxisAlignedClip(
                    &rect(area.left, area.top, lit_right, area.bottom),
                    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                );
            }
            self.set_brush(self.text, alpha);
            self.draw_text(&text, format, area);
            unsafe { self.d2d_context.PopAxisAlignedClip() };
        }
    }

    /// Tô một tam giác (biểu tượng nút phát, lùi, tới)
    fn triangle(&self, points: [(f32, f32); 3]) {
        let v = |(x, y): (f32, f32)| Vector2 { X: x, Y: y };
        unsafe {
            let Ok(geometry) = self.factory.CreatePathGeometry() else {
                return;
            };
            let Ok(sink) = geometry.Open() else { return };
            sink.BeginFigure(v(points[0]), D2D1_FIGURE_BEGIN_FILLED);
            sink.AddLine(v(points[1]));
            sink.AddLine(v(points[2]));
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            if sink.Close().is_ok() {
                self.d2d_context.FillGeometry(&geometry, &self.brush, None);
            }
        }
    }

    /// Hai tam giác nối nhau (⏪ khi `dir` = -1, ⏩ khi `dir` = 1), tâm (cx, cy)
    fn double_arrow(&self, cx: f32, cy: f32, dir: f32) {
        let (half_w, half_h) = (7.0, 6.0);
        for offset in [-half_w, 0.0] {
            // Mỗi tam giác rộng half_w; với dir = 1 mũi nhọn ở bên phải
            let base = cx + dir * offset;
            let tip = base + dir * half_w;
            self.triangle([(base, cy - half_h), (base, cy + half_h), (tip, cy)]);
        }
    }

    /// Vạch đứng + tam giác (⏮ khi `dir` = -1, ⏭ khi `dir` = 1), tâm (cx, cy)
    fn skip_icon(&self, cx: f32, cy: f32, dir: f32) {
        let (half_w, half_h) = (6.0, 7.0);
        let back = cx - dir * half_w; // đáy tam giác
        let tip = cx + dir * (half_w - 3.0);
        self.triangle([
            (back + dir * 1.5, cy - half_h),
            (back + dir * 1.5, cy + half_h),
            (tip, cy),
        ]);
        let bar_x = tip + dir * 0.5;
        let (l, r) = if dir > 0.0 {
            (bar_x, bar_x + 2.5)
        } else {
            (bar_x - 2.5, bar_x)
        };
        self.fill(rounded(rect(l, cy - half_h, r, cy + half_h), 1.0));
    }

    /// Hàng nút tròn ở đáy island: lùi, phát/dừng (to hơn), tới; biểu tượng tô màu `accent`.
    /// Trang Media: nút ⏪ ⏩ và nút lời bên phải. Trang Media Player (`player`): nút ⏮ ⏭
    fn draw_controls(&self, frame: &Frame, alpha: f32, accent: Color, player: bool) {
        let skip = |cx: f32, cy: f32, dir: f32| {
            if player {
                self.skip_icon(cx, cy, dir);
            } else {
                self.double_arrow(cx, cy, dir);
            }
        };
        let content = frame.content;
        let disc = |i: usize, base: f32| {
            let r = self.layout.button_rect(i, frame.h);
            let hover = if frame.hover_button == Some(i) {
                0.1
            } else {
                0.0
            };
            let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0);
            self.set_brush(self.text, alpha * (base + hover));
            self.circle(cx, cy, (r.right - r.left) / 2.0);
            (cx, cy)
        };

        let (cx, cy) = disc(0, BUTTON_BG);
        self.set_brush(accent, alpha);
        skip(cx, cy, -1.0);

        let (cx, cy) = disc(1, PLAY_BG);
        self.set_brush(accent, alpha);
        if content.playing {
            for x in [cx - 6.0, cx + 2.0] {
                self.fill(rounded(rect(x, cy - 8.0, x + 4.0, cy + 8.0), 1.5));
            }
        } else {
            // Tam giác lệch phải một chút để trông cân giữa vòng tròn
            self.triangle([(cx - 5.0, cy - 9.0), (cx - 5.0, cy + 9.0), (cx + 9.0, cy)]);
        }

        let (cx, cy) = disc(2, BUTTON_BG);
        self.set_brush(accent, alpha);
        skip(cx, cy, 1.0);

        if self.lyrics_enabled && !player {
            disc(LYRICS_BUTTON, BUTTON_BG);
            let r = self.layout.button_rect(LYRICS_BUTTON, frame.h);
            self.draw_lyrics_icon(r, frame.lyrics_on, alpha);
        }
    }

    /// Biểu tượng bong bóng lời thoại: màu nhấn khi đang bật lời, xám khi tắt
    fn draw_lyrics_icon(&self, r: D2D_RECT_F, on: bool, alpha: f32) {
        let (cx, cy) = ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0 - 1.0);
        let (color, opacity) = if on {
            (self.accent, 1.0)
        } else {
            (self.secondary, 0.7)
        };
        self.set_brush(color, alpha * opacity);
        self.fill(rounded(rect(cx - 8.0, cy - 6.5, cx + 8.0, cy + 4.5), 3.0));
        self.line((cx - 4.0, cy + 3.5), (cx - 6.0, cy + 7.5), 2.6); // đuôi bong bóng
        // Hai dòng chữ khoét bằng màu nền
        self.knockout(|| {
            self.fill(rounded(rect(cx - 5.0, cy - 3.5, cx + 5.0, cy - 2.2), 0.6));
            self.fill(rounded(rect(cx - 5.0, cy - 0.4, cx + 2.0, cy + 0.9), 0.6));
        });
    }

    /// "0:29 ━━━●──────── -1:56" tại tâm dọc `cy`
    fn draw_progress(&self, p: Progress, (el, right): (f32, f32), cy: f32, alpha: f32) {
        let (top, bottom) = (cy - 8.0, cy + 8.0);
        self.set_brush(self.secondary, alpha);
        let remaining = p.duration_ms.saturating_sub(p.elapsed_ms);
        self.draw_text(
            &content::mmss(p.elapsed_ms),
            &self.fmt_tiny,
            rect(el + SIDE_PAD - 4.0, top, el + SIDE_PAD + 36.0, bottom),
        );
        self.draw_text(
            &format!("-{}", content::mmss(remaining)),
            &self.fmt_tiny,
            rect(right - 72.0, top, right - 30.0, bottom),
        );

        let (x0, x1) = (el + SIDE_PAD + 42.0, right - 78.0);
        let x = x0 + (x1 - x0) * p.fraction();
        let bar = |from: f32, to: f32| rounded(rect(from, cy - 2.0, to, cy + 2.0), 2.0);
        self.set_brush(self.text, alpha * 0.18);
        self.fill(bar(x0, x1));
        self.set_brush(self.accent, alpha);
        self.fill(bar(x0, x.max(x0 + 4.0)));
        self.set_brush(self.text, alpha);
        self.circle(x, cy, 5.0);
    }

    // ---------- Trang Media Player ----------

    /// Cọ tô chuyển màu theo đường thẳng từ `from` tới `to`; mỗi mốc là (vị trí 0–1, màu, độ đục)
    fn gradient(
        &self,
        stops: &[(f32, Color, f32)],
        from: (f32, f32),
        to: (f32, f32),
    ) -> Option<ID2D1LinearGradientBrush> {
        let stops: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|&(position, color, opacity)| {
                let (r, g, b, a) = color.to_f32();
                D2D1_GRADIENT_STOP {
                    position,
                    color: D2D1_COLOR_F {
                        r,
                        g,
                        b,
                        a: a * opacity,
                    },
                }
            })
            .collect();
        unsafe {
            // Bản đơn giản của ID2D1RenderTarget (bản của DeviceContext cần thêm 3 tham số)
            let target: &ID2D1RenderTarget = &self.d2d_context;
            let collection = target
                .CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP)
                .ok()?;
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES {
                startPoint: Vector2 {
                    X: from.0,
                    Y: from.1,
                },
                endPoint: Vector2 { X: to.0, Y: to.1 },
            };
            self.d2d_context
                .CreateLinearGradientBrush(&props, None, &collection)
                .ok()
        }
    }

    /// Trang Media Player mở rộng (màu nhấn lấy từ ảnh bìa):
    ///   [ẢNH BÌA]  Tên bài            sóng nhạc
    ///              Nghệ sĩ
    ///   1:42 ━━━━━━━━●──────────── -2:19
    ///          (lùi)  (phát/dừng)  (tới)
    fn draw_player_full(&self, frame: &Frame, alpha: f32) {
        let content = frame.content;
        let (el, right) = self.expanded_edges();
        if content.title.is_empty() {
            self.set_brush(self.secondary, alpha);
            self.draw_text(
                "Không có nhạc đang phát",
                &self.fmt_date,
                rect(el, 10.0, right, 92.0),
            );
            return;
        }
        let accent = frame.cover_accent.unwrap_or(self.accent);
        self.draw_wash(frame, accent, alpha);

        let x = el + PLAYER_PAD;
        let cover = rect(
            x,
            PLAYER_COVER_TOP,
            x + PLAYER_COVER,
            PLAYER_COVER_TOP + PLAYER_COVER,
        );
        self.draw_cover(frame.cover, cover, 16.0, alpha);
        self.draw_badge(cover, content.playing, accent, alpha);

        let cy = PLAYER_COVER_TOP + PLAYER_COVER / 2.0;
        let wave_right = right - WAVE_FROM_RIGHT;
        let wave_left = wave_right - WAVE_W;
        self.draw_wave(frame.wave, wave_left, cy, accent, content.playing, alpha);

        let x0 = cover.right + 18.0;
        let max_w = wave_left - 14.0 - x0;
        let (title, f) = self.fit_text(&content.full_title, &[&self.fmt_media_title], max_w);
        self.set_brush(self.text, alpha);
        self.draw_text(&title, f, rect(x0, cy - 24.0, x0 + max_w, cy + 1.0));
        let (artist, f) = self.fit_text(&content.full_artist, &[&self.fmt_alert_body], max_w);
        self.set_brush(self.secondary, alpha);
        self.draw_text(&artist, f, rect(x0, cy + 3.0, x0 + max_w, cy + 23.0));

        if let Some(progress) = content.progress {
            self.draw_player_progress(progress, accent, alpha);
        }
        self.draw_controls(frame, alpha, accent, true);
    }

    /// Accent Wash: phủ màu nhấn rất nhạt lên nền, đậm ở góc trên trái, nhạt dần về góc dưới phải
    fn draw_wash(&self, frame: &Frame, accent: Color, alpha: f32) {
        let left = self.layout.island_left(frame.w);
        let pill = rounded(
            rect(left, 0.0, left + frame.w, frame.h),
            Layout::corner_radius(frame.h),
        );
        let stops = [
            (0.0, accent, WASH_OPACITY),
            (0.55, accent, WASH_OPACITY * 0.25),
            (1.0, accent, 0.0),
        ];
        let to = (left + frame.w * 0.75, frame.h);
        if let Some(brush) = self.gradient(&stops, (left, 0.0), to) {
            unsafe {
                brush.SetOpacity(alpha);
                self.d2d_context.FillRoundedRectangle(&pill, &brush);
            }
        }
    }

    /// Nút tròn nhỏ ở góc dưới phải ảnh bìa: tam giác khi đang dừng, hai vạch khi đang phát
    fn draw_badge(&self, cover: D2D_RECT_F, playing: bool, accent: Color, alpha: f32) {
        let (cx, cy) = (cover.right - BADGE_R + 2.0, cover.bottom - BADGE_R + 2.0);
        self.set_brush(BADGE_BG, alpha * 0.92);
        self.circle(cx, cy, BADGE_R);
        self.set_brush(accent, alpha);
        if playing {
            for x in [cx - 4.0, cx + 1.0] {
                self.fill(rounded(rect(x, cy - 5.0, x + 3.0, cy + 5.0), 1.0));
            }
        } else {
            self.triangle([(cx - 3.5, cy - 6.0), (cx - 3.5, cy + 6.0), (cx + 6.0, cy)]);
        }
    }

    /// Sóng nhạc: các vạch dọc căn giữa theo `cy`, cao thấp theo mức âm thanh vừa đo.
    /// Đang dừng thì chỉ còn các vạch thấp, nhạt
    fn draw_wave(&self, wave: &Wave, left: f32, cy: f32, accent: Color, playing: bool, alpha: f32) {
        let step = WAVE_W / WAVE_BARS as f32;
        let opacity = if playing { 1.0 } else { 0.45 };
        self.set_brush(accent, alpha * opacity);
        for (i, level) in wave.bars.iter().enumerate() {
            let h = WAVE_MIN_H + (WAVE_MAX_H - WAVE_MIN_H) * level.clamp(0.0, 1.0);
            let x = left + step * i as f32 + (step - WAVE_BAR_W) / 2.0;
            let bar = rect(x, cy - h / 2.0, x + WAVE_BAR_W, cy + h / 2.0);
            self.fill(rounded(bar, WAVE_BAR_W / 2.0));
        }
    }

    /// "1:42 ━━━━━●────── -2:19": phần đã phát tô chuyển màu từ màu nhấn nhạt sang màu nhấn,
    /// núm tròn trắng viền màu nhấn. Bấm lên thanh để tua (xem app.rs)
    fn draw_player_progress(&self, p: Progress, accent: Color, alpha: f32) {
        let (el, right) = self.expanded_edges();
        let (x0, x1, cy) = self.layout.player_bar();
        let (top, bottom) = (cy - 9.0, cy + 9.0);
        self.set_brush(self.secondary, alpha);
        self.draw_text(
            &content::mmss(p.elapsed_ms),
            &self.fmt_tiny,
            rect(el + PLAYER_PAD - 6.0, top, x0 - 6.0, bottom),
        );
        let remaining = p.duration_ms.saturating_sub(p.elapsed_ms);
        self.draw_text(
            &format!("-{}", content::mmss(remaining)),
            &self.fmt_tiny,
            rect(x1 + 6.0, top, right - 30.0, bottom),
        );

        let bar = |from: f32, to: f32| rounded(rect(from, cy - 2.5, to, cy + 2.5), 2.5);
        self.set_brush(self.text, alpha * 0.14);
        self.fill(bar(x0, x1));
        let x = x0 + (x1 - x0) * p.fraction();
        let stops = [
            (0.0, palette::lighten(accent, 0.45), 1.0),
            (1.0, accent, 1.0),
        ];
        // Chuyển màu trải trên phần đã phát, nên sát núm luôn là màu nhấn đậm nhất
        if let Some(brush) = self.gradient(&stops, (x0, cy), (x.max(x0 + 1.0), cy)) {
            unsafe {
                brush.SetOpacity(alpha);
                self.d2d_context
                    .FillRoundedRectangle(&bar(x0, x.max(x0 + 5.0)), &brush);
            }
        }
        self.set_brush(accent, alpha);
        self.circle(x, cy, 7.5);
        self.set_brush(Color::rgb(0xFF, 0xFF, 0xFF), alpha);
        self.circle(x, cy, 5.5);
    }
}
