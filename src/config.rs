//! Cấu hình người dùng, đọc từ `%APPDATA%\DynamicIsland\config.toml`.
//! Thiếu file thì tạo file mặc định; thiếu trường nào thì dùng giá trị mặc định của trường đó.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use serde::Deserialize;

/// Nội dung file mặc định (có chú thích), nhúng vào file .exe lúc biên dịch
const DEFAULT_TOML: &str = include_str!("default_config.toml");

// `default`: trường nào thiếu thì lấy từ `Default::default()`
// `deny_unknown_fields`: gõ sai tên trường thì báo lỗi thay vì lặng lẽ bỏ qua
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub window: WindowConfig,
    pub colors: Colors,
    pub font: FontConfig,
    pub clock: ClockConfig,
    pub weather: WeatherConfig,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub top: i32,
    pub collapsed_width: f32,
    pub expanded_width: f32,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            top: 10,
            collapsed_width: 220.0,
            expanded_width: 400.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Colors {
    pub background: Color,
    pub text: Color,
    pub secondary: Color,
    /// Màu nhấn: thanh pin, chấm phân trang đang chọn
    pub accent: Color,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: Color::rgb(0x0F, 0x11, 0x12),
            text: Color::rgb(0xF5, 0xF5, 0xF7),
            secondary: Color::rgb(0x8E, 0x8E, 0x93),
            accent: Color::rgb(0x4C, 0xC2, 0xEA),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    pub family: String,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "Segoe UI".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ClockConfig {
    pub hour24: bool,
    pub show_seconds: bool,
}

impl Default for ClockConfig {
    fn default() -> Self {
        Self {
            hour24: true,
            show_seconds: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WeatherConfig {
    /// false: không gọi mạng, ô bên phải hiện pin (hoặc thứ/ngày)
    pub enabled: bool,
    pub city: String,
    pub fahrenheit: bool,
}

impl Default for WeatherConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            city: "Hà Nội".into(),
            fahrenheit: false,
        }
    }
}

/// Màu RGB, trong file TOML viết dạng chuỗi "#RRGGBB".
/// `try_from = "String"`: serde đọc ra String trước, rồi đổi sang Color bằng `TryFrom`
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Ba kênh màu trong khoảng 0.0–1.0, kiểu Direct2D cần
    pub fn to_f32(self) -> (f32, f32, f32) {
        let f = |c: u8| f32::from(c) / 255.0;
        (f(self.r), f(self.g), f(self.b))
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        // Kiểm tra trước để cắt chuỗi theo byte bên dưới không bao giờ panic
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("màu \"{s}\" không hợp lệ, cần dạng \"#RRGGBB\""));
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        Ok(Self::rgb(channel(0), channel(2), channel(4)))
    }
}

impl TryFrom<String> for Color {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// Giới hạn số thực vào [lo, hi]; NaN/vô cực thì dùng `fallback`
fn clamp_or(v: f32, lo: f32, hi: f32, fallback: f32) -> f32 {
    if v.is_finite() {
        v.clamp(lo, hi)
    } else {
        fallback
    }
}

impl Config {
    /// Đọc cấu hình từ nội dung TOML, rồi kéo các số về khoảng an toàn
    pub fn parse(text: &str) -> Result<Self, toml::de::Error> {
        // Notepad đôi khi lưu kèm BOM ở đầu file; TOML không chấp nhận ký tự đó
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        toml::from_str::<Self>(text).map(Self::sanitized)
    }

    // Số quá lớn/nhỏ sẽ làm vỡ giao diện, nên giới hạn lại
    fn sanitized(mut self) -> Self {
        let d = WindowConfig::default();
        let w = &mut self.window;
        w.top = w.top.clamp(0, 300);
        // Thu gọn luôn hẹp hơn mở rộng, để hiệu ứng chuyển chữ không chia cho 0.
        // Dưới 180 thì đồng hồ và pin ở Idle View không đủ chỗ
        w.collapsed_width = clamp_or(w.collapsed_width, 180.0, 300.0, d.collapsed_width);
        w.expanded_width = clamp_or(w.expanded_width, 320.0, 700.0, d.expanded_width);
        self
    }
}

#[derive(Debug)]
pub enum LoadError {
    Io(PathBuf, io::Error),
    Parse(PathBuf, toml::de::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Io(path, e) => write!(f, "Không đọc/ghi được {}:\n{e}", path.display()),
            Self::Parse(path, e) => write!(f, "File {} bị lỗi:\n{e}", path.display()),
        }
    }
}

impl std::error::Error for LoadError {}

/// Đường dẫn file cấu hình, hoặc None nếu không tìm thấy thư mục AppData
pub fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join("DynamicIsland")
            .join("config.toml"),
    )
}

fn write_default(path: &Path) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(path, DEFAULT_TOML)
}

/// Đọc cấu hình; chưa có file thì tạo file mặc định
pub fn load() -> Result<Config, LoadError> {
    let Some(path) = path() else {
        return Ok(Config::default());
    };
    match fs::read_to_string(&path) {
        Ok(text) => Config::parse(&text).map_err(|e| LoadError::Parse(path, e)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            write_default(&path).map_err(|e| LoadError::Io(path, e))?;
            Ok(Config::default())
        }
        Err(e) => Err(LoadError::Io(path, e)),
    }
}

/// Mở file cấu hình bằng Notepad (tạo file mặc định nếu chưa có)
pub fn open_in_editor() {
    let Some(path) = path() else { return };
    if !path.exists() {
        let _ = write_default(&path);
    }
    let _ = Command::new("notepad.exe").arg(&path).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_default_struct() {
        // Giữ file mẫu và Default::default() luôn khớp nhau
        assert_eq!(Config::parse(DEFAULT_TOML).unwrap(), Config::default());
    }

    #[test]
    fn empty_file_gives_defaults() {
        assert_eq!(Config::parse("").unwrap(), Config::default());
    }

    #[test]
    fn missing_fields_keep_their_defaults() {
        let c = Config::parse("[clock]\nhour24 = false").unwrap();
        assert!(!c.clock.hour24);
        assert!(c.clock.show_seconds);
        assert_eq!(c.window, WindowConfig::default());
    }

    #[test]
    fn integers_are_accepted_for_widths() {
        let c = Config::parse("[window]\nexpanded_width = 500").unwrap();
        assert_eq!(c.window.expanded_width, 500.0);
    }

    #[test]
    fn misspelled_field_is_an_error() {
        let err = Config::parse("[window]\ntpo = 5").unwrap_err();
        assert!(err.to_string().contains("tpo"));
    }

    #[test]
    fn invalid_color_is_an_error() {
        let err = Config::parse("[colors]\ntext = \"trắng\"").unwrap_err();
        assert!(err.to_string().contains("#RRGGBB"));
    }

    #[test]
    fn out_of_range_numbers_are_clamped() {
        let c = Config::parse("[window]\ntop = -50\ncollapsed_width = 999\nexpanded_width = nan")
            .unwrap();
        assert_eq!(c.window.top, 0);
        assert_eq!(c.window.collapsed_width, 300.0);
        assert_eq!(c.window.expanded_width, 400.0);
    }

    #[test]
    fn utf8_bom_is_ignored() {
        assert!(Config::parse("\u{feff}[clock]\nhour24 = true").is_ok());
    }

    #[test]
    fn color_parses_with_or_without_hash() {
        assert_eq!("#1E90FF".parse(), Ok(Color::rgb(0x1E, 0x90, 0xFF)));
        assert_eq!("1e90ff".parse(), Ok(Color::rgb(0x1E, 0x90, 0xFF)));
    }

    #[test]
    fn color_rejects_bad_input_without_panicking() {
        for bad in ["", "#12345", "#1234567", "#GG0000", "#ÀÀÀ", "#+12345"] {
            assert!(bad.parse::<Color>().is_err(), "{bad} lẽ ra phải lỗi");
        }
    }

    #[test]
    fn color_to_f32_spans_zero_to_one() {
        assert_eq!(Color::rgb(0, 255, 0).to_f32(), (0.0, 1.0, 0.0));
    }
}
