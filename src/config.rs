//! Cấu hình người dùng, đọc từ `%APPDATA%\DynamicIsland\config.toml`.
//! Thiếu file thì tạo file mặc định; thiếu trường nào thì dùng giá trị mặc định của trường đó.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;

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
    pub autohide: AutoHideConfig,
    pub lyrics: LyricsConfig,
    pub alerts: AlertsConfig,
    pub ai: AiConfig,
    /// Mục [lookup] của tính năng Tra cứu nhanh đã bỏ. File cấu hình cũ còn mục này thì đọc
    /// (để chuyển Account ID, model, lời dặn sang [ai]) rồi bỏ qua; không khai báo thì
    /// `deny_unknown_fields` báo lỗi và người dùng mất hết cấu hình
    #[serde(rename = "lookup")]
    retired_lookup: Option<toml::Table>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub top: i32,
    pub collapsed_width: f32,
    pub expanded_width: f32,
    /// Hiệu ứng làm mờ nội dung phía sau island (cần nền `#RRGGBBAA` trong suốt mới thấy)
    pub backdrop: BackdropKind,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            top: 10,
            collapsed_width: 220.0,
            expanded_width: 400.0,
            backdrop: BackdropKind::None,
        }
    }
}

/// Nền phía sau island, trong file TOML viết "none", "blur" hoặc "acrylic"
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackdropKind {
    /// Không làm mờ: chỗ nền trong suốt thấy rõ desktop phía sau
    #[default]
    None,
    /// Làm mờ đơn thuần
    Blur,
    /// Kính mờ Acrylic của Windows: làm mờ thêm lớp hạt nhiễu và ánh màu nền
    Acrylic,
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

/// Lời bài hát chạy theo nhạc, lấy từ LRCLIB
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LyricsConfig {
    /// false: không gọi mạng, không hiện lời
    pub enabled: bool,
    /// Hiện khung lời khi mở rộng trang nhạc (nút bên phải bật/tắt được trong lúc dùng)
    pub show: bool,
    /// Hiện câu đang hát ngay trên island thu gọn (bật/tắt trong menu chuột phải)
    pub collapsed: bool,
    /// Giới hạn bộ nhớ đệm trên đĩa (MB); 0 = tắt, không đọc/ghi đĩa
    pub cache_mb: u32,
}

impl Default for LyricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            show: true,
            collapsed: false,
            cache_mb: 25,
        }
    }
}

/// Thông báo tạm thời trên island
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AlertsConfig {
    /// Hiện nội dung vừa sao chép
    pub clipboard: bool,
    /// Hiện thông báo Windows mới
    pub notifications: bool,
    /// Mỗi thông báo hiện bao nhiêu giây (rê chuột vào thì giữ lại)
    pub seconds: u32,
}

impl Default for AlertsConfig {
    fn default() -> Self {
        Self {
            clipboard: true,
            notifications: true,
            seconds: 4,
        }
    }
}

/// Trợ lý AI (Cloudflare Workers AI).
/// API token không nằm ở đây mà trong Windows Credential Manager (xem secret.rs)
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AiConfig {
    /// Model của Workers AI, ví dụ "@cf/openai/gpt-oss-20b"
    pub model: String,
    /// Account ID của Cloudflare
    pub account_id: String,
    /// Lời dặn cho trợ lý (vai trò, cách trả lời)
    pub system_prompt: String,
    /// Không tìm được lời bài hát theo tên đang phát thì nhờ AI đoán tên bài thật
    pub guess_song_names: bool,
}

impl AiConfig {
    /// Lấy Account ID, model, lời dặn từ mục [lookup] cũ (thiếu trường nào thì dùng mặc định)
    fn from_retired_lookup(old: &toml::Table) -> Self {
        let mut ai = Self::default();
        let text = |key: &str| {
            old.get(key)
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        };
        if let Some(model) = text("model") {
            ai.model = model;
        }
        if let Some(account_id) = text("account_id") {
            ai.account_id = account_id;
        }
        if let Some(prompt) = text("system_prompt") {
            ai.system_prompt = prompt;
        }
        ai
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            model: crate::ai::MODELS[0].0.to_string(),
            account_id: String::new(),
            system_prompt: crate::ai::DEFAULT_SYSTEM_PROMPT.to_string(),
            guess_song_names: true,
        }
    }
}

/// Notch Auto-Hide: thu nhỏ island thành một vạch nhỏ khi không dùng
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AutoHideConfig {
    pub enabled: bool,
    /// Số giây không dùng trước khi thu nhỏ
    pub delay_seconds: u32,
    /// Đang phát nhạc thì không thu nhỏ
    pub stay_while_playing: bool,
}

impl Default for AutoHideConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            delay_seconds: 5,
            stay_while_playing: true,
        }
    }
}

impl AutoHideConfig {
    /// Island có đang được dùng không? Đang dùng thì chưa đếm giờ tự ẩn
    pub fn in_use(&self, hovered: bool, playing: bool) -> bool {
        hovered || (self.stay_while_playing && playing)
    }

    /// Đã không dùng đủ lâu (`idle`) để thu nhỏ chưa?
    pub fn is_due(&self, idle: Duration) -> bool {
        self.enabled && idle >= Duration::from_secs(u64::from(self.delay_seconds))
    }
}

/// Màu RGBA, trong file TOML viết dạng chuỗi "#RRGGBB" (đục hẳn) hoặc "#RRGGBBAA"
/// (AA là độ đục: 00 = trong suốt, FF = đục hẳn).
/// `try_from = "String"`: serde đọc ra String trước, rồi đổi sang Color bằng `TryFrom`
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "String")]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self::rgba(r, g, b, 0xFF)
    }

    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// Bốn kênh (r, g, b, độ đục) trong khoảng 0.0–1.0, kiểu Direct2D cần
    pub fn to_f32(self) -> (f32, f32, f32, f32) {
        let f = |c: u8| f32::from(c) / 255.0;
        (f(self.r), f(self.g), f(self.b), f(self.a))
    }

    /// Có nhìn xuyên qua được không (độ đục dưới FF)?
    pub fn is_translucent(self) -> bool {
        self.a < 0xFF
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').unwrap_or(s);
        // Kiểm tra trước để cắt chuỗi theo byte bên dưới không bao giờ panic
        if !matches!(hex.len(), 6 | 8) || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "màu \"{s}\" không hợp lệ, cần dạng \"#RRGGBB\" hoặc \"#RRGGBBAA\""
            ));
        }
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        let alpha = if hex.len() == 8 { channel(6) } else { 0xFF };
        Ok(Self::rgba(channel(0), channel(2), channel(4), alpha))
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
        let mut config = toml::from_str::<Self>(text)?;
        if let Some(old) = config.retired_lookup.take() {
            // File từ bản còn Tra cứu nhanh, chưa có mục [ai]: lấy cấu hình AI từ mục cũ
            let has_ai = text
                .parse::<toml::Table>()
                .is_ok_and(|t| t.contains_key("ai"));
            if !has_ai {
                config.ai = AiConfig::from_retired_lookup(&old);
            }
        }
        Ok(config.sanitized())
    }

    // Số quá lớn/nhỏ sẽ làm vỡ giao diện, nên giới hạn lại
    fn sanitized(mut self) -> Self {
        let d = WindowConfig::default();
        let w = &mut self.window;
        w.top = w.top.clamp(0, 300);
        // Thu gọn luôn hẹp hơn mở rộng: câu lời trên island thu gọn giãn trong khoảng giữa hai số.
        // Dưới 180 thì đồng hồ và pin ở Idle View không đủ chỗ
        w.collapsed_width = clamp_or(w.collapsed_width, 180.0, 300.0, d.collapsed_width);
        w.expanded_width = clamp_or(w.expanded_width, 320.0, 700.0, d.expanded_width);
        self.autohide.delay_seconds = self.autohide.delay_seconds.clamp(1, 3600);
        self.lyrics.cache_mb = self.lyrics.cache_mb.min(1000);
        self.alerts.seconds = self.alerts.seconds.clamp(2, 30);
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

/// Đổi giá trị `key` trong mục `[section]` của nội dung TOML mà vẫn giữ nguyên chú thích,
/// khoảng trắng và mọi dòng khác (người dùng có thể đã tự sửa file).
/// Chưa có dòng đó thì thêm vào cuối mục; chưa có mục thì thêm mục vào cuối file.
/// Chỉ dùng cho giá trị đơn giản (true/false, số), không có dấu # bên trong
fn set_value(text: &str, section: &str, key: &str, value: &str) -> String {
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    // Bỏ chú thích cuối dòng trước khi so tên mục: "[lyrics] # ..." vẫn là mục lyrics
    let code = |line: &str| line.split('#').next().unwrap_or("").trim().to_string();
    let header = format!("[{section}]");
    let is_key = |line: &str| {
        code(line)
            .strip_prefix(key)
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };

    match lines.iter().position(|l| code(l) == header) {
        None => {
            if lines.last().is_some_and(|l| !l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push(header);
            lines.push(format!("{key} = {value}"));
        }
        Some(start) => {
            let end = (start + 1..lines.len())
                .find(|&i| code(&lines[i]).starts_with('['))
                .unwrap_or(lines.len());
            match (start + 1..end).find(|&i| is_key(&lines[i])) {
                Some(i) => {
                    // Chỉ thay chữ của giá trị, giữ khoảng trắng và chú thích phía sau
                    let line = &lines[i];
                    let eq = line.find('=').unwrap_or(0) + 1;
                    let from = eq + (line[eq..].len() - line[eq..].trim_start().len());
                    let to = value_end(line, from);
                    lines[i] = format!("{}{value}{}", &line[..from], &line[to..]);
                }
                None => {
                    // Thêm sau dòng có chữ cuối cùng của mục (bỏ qua dòng trống ở cuối mục)
                    let mut at = end;
                    while at > start + 1 && lines[at - 1].trim().is_empty() {
                        at -= 1;
                    }
                    lines.insert(at, format!("{key} = {value}"));
                }
            }
        }
    }
    let mut out = lines.join(newline);
    if text.is_empty() || text.ends_with('\n') {
        out.push_str(newline);
    }
    out
}

/// Vị trí ngay sau giá trị bắt đầu ở `from` trong một dòng TOML:
/// chuỗi "..." thì tới hết dấu ngoặc kép đóng, còn lại thì tới khoảng trắng hoặc #
fn value_end(line: &str, from: usize) -> usize {
    let rest = &line[from..];
    if let Some(inner) = rest.strip_prefix('"') {
        let mut escaped = false;
        for (i, c) in inner.char_indices() {
            match c {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => return from + 1 + i + 1,
                _ => escaped = false,
            }
        }
        return line.len();
    }
    rest.find(|c: char| c.is_whitespace() || c == '#')
        .map_or(line.len(), |n| from + n)
}

/// Chuỗi TOML trong ngoặc kép (thoát dấu \, ", và xuống dòng để chuỗi nằm trên một dòng)
fn quoted(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("\r\n", "\\n")
        .replace('\n', "\\n")
        .replace('\r', "")
        .replace('\t', "\\t");
    format!("\"{escaped}\"")
}

impl fmt::Display for Color {
    /// "#RRGGBB" khi đục hẳn, "#RRGGBBAA" khi trong suốt
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)?;
        if self.is_translucent() {
            write!(f, "{:02X}", self.a)?;
        }
        Ok(())
    }
}

impl BackdropKind {
    fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Blur => "blur",
            Self::Acrylic => "acrylic",
        }
    }
}

impl Config {
    /// Mọi giá trị dạng (mục, tên, giá trị TOML), theo đúng thứ tự trong file mẫu
    fn entries(&self) -> Vec<(&'static str, &'static str, String)> {
        let (w, c) = (&self.window, &self.colors);
        let color = |c: Color| quoted(&c.to_string());
        vec![
            ("window", "top", w.top.to_string()),
            ("window", "collapsed_width", w.collapsed_width.to_string()),
            ("window", "expanded_width", w.expanded_width.to_string()),
            ("window", "backdrop", quoted(w.backdrop.name())),
            ("colors", "background", color(c.background)),
            ("colors", "text", color(c.text)),
            ("colors", "secondary", color(c.secondary)),
            ("colors", "accent", color(c.accent)),
            ("font", "family", quoted(&self.font.family)),
            ("clock", "hour24", self.clock.hour24.to_string()),
            ("clock", "show_seconds", self.clock.show_seconds.to_string()),
            ("weather", "enabled", self.weather.enabled.to_string()),
            ("weather", "city", quoted(&self.weather.city)),
            ("weather", "fahrenheit", self.weather.fahrenheit.to_string()),
            ("autohide", "enabled", self.autohide.enabled.to_string()),
            (
                "autohide",
                "delay_seconds",
                self.autohide.delay_seconds.to_string(),
            ),
            (
                "autohide",
                "stay_while_playing",
                self.autohide.stay_while_playing.to_string(),
            ),
            ("lyrics", "enabled", self.lyrics.enabled.to_string()),
            ("lyrics", "show", self.lyrics.show.to_string()),
            ("lyrics", "collapsed", self.lyrics.collapsed.to_string()),
            ("lyrics", "cache_mb", self.lyrics.cache_mb.to_string()),
            ("alerts", "clipboard", self.alerts.clipboard.to_string()),
            (
                "alerts",
                "notifications",
                self.alerts.notifications.to_string(),
            ),
            ("alerts", "seconds", self.alerts.seconds.to_string()),
            ("ai", "model", quoted(&self.ai.model)),
            ("ai", "account_id", quoted(&self.ai.account_id)),
            ("ai", "system_prompt", quoted(&self.ai.system_prompt)),
            (
                "ai",
                "guess_song_names",
                self.ai.guess_song_names.to_string(),
            ),
        ]
    }
}

/// Nội dung file sau khi ghi `config` vào `text` (giữ chú thích của file cũ)
fn with_config(text: &str, config: &Config) -> String {
    config
        .entries()
        .into_iter()
        .fold(text.to_string(), |text, (section, key, value)| {
            set_value(&text, section, key, &value)
        })
}

/// Đọc nội dung file hiện tại (chưa có thì lấy file mẫu), sửa bằng `edit`, rồi ghi lại
fn edit_file(edit: impl FnOnce(&str) -> String) -> io::Result<()> {
    let Some(path) = path() else { return Ok(()) };
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => DEFAULT_TOML.to_string(),
        Err(e) => return Err(e),
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, edit(&text))
}

/// Lưu toàn bộ cấu hình (cửa sổ Cài đặt gọi khi bấm Lưu)
pub fn save(config: &Config) -> io::Result<()> {
    edit_file(|text| with_config(text, config))
}

/// Ghi lựa chọn "lời bài hát trên island thu gọn" vào file cấu hình,
/// để lần mở sau vẫn giữ (chỉ sửa đúng một dòng)
pub fn save_lyrics_collapsed(on: bool) -> io::Result<()> {
    edit_file(|text| set_value(text, "lyrics", "collapsed", &on.to_string()))
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
    fn autohide_delay_is_clamped() {
        let c = Config::parse(
            "[autohide]
delay_seconds = 0",
        )
        .unwrap();
        assert_eq!(c.autohide.delay_seconds, 1);
        assert!(c.autohide.enabled);
    }

    #[test]
    fn autohide_waits_for_the_delay() {
        let a = AutoHideConfig::default(); // 5 giây
        assert!(!a.is_due(Duration::from_secs(4)));
        assert!(a.is_due(Duration::from_secs(5)));
        let off = AutoHideConfig {
            enabled: false,
            ..a
        };
        assert!(!off.is_due(Duration::from_secs(600)));
    }

    #[test]
    fn hover_and_playback_count_as_in_use() {
        let a = AutoHideConfig::default();
        assert!(a.in_use(true, false));
        assert!(a.in_use(false, true));
        assert!(!a.in_use(false, false));
        let hide_anyway = AutoHideConfig {
            stay_while_playing: false,
            ..a
        };
        assert!(!hide_anyway.in_use(false, true));
    }

    #[test]
    fn lyrics_cache_can_be_turned_off_and_is_capped() {
        let off = Config::parse("[lyrics]\ncache_mb = 0").unwrap();
        assert_eq!(off.lyrics.cache_mb, 0);
        assert!(off.lyrics.enabled);
        let huge = Config::parse("[lyrics]\ncache_mb = 99999").unwrap();
        assert_eq!(huge.lyrics.cache_mb, 1000);
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
    fn set_value_replaces_only_the_value() {
        let text = "[window]\ncollapsed = 1\n\n[lyrics]\nshow = true    # chú thích\ncollapsed = false  # giữ nguyên\n";
        let out = set_value(text, "lyrics", "collapsed", "true");
        assert_eq!(
            out,
            "[window]\ncollapsed = 1\n\n[lyrics]\nshow = true    # chú thích\ncollapsed = true  # giữ nguyên\n"
        );
        let edited = set_value(DEFAULT_TOML, "lyrics", "collapsed", "true");
        assert!(Config::parse(&edited).unwrap().lyrics.collapsed);
    }

    #[test]
    fn set_value_adds_missing_key_or_section() {
        // Thiếu dòng: thêm vào cuối mục, trước dòng trống và mục kế tiếp
        let text = "[lyrics]\nshow = true\n\n[clock]\nhour24 = true\n";
        assert_eq!(
            set_value(text, "lyrics", "collapsed", "true"),
            "[lyrics]\nshow = true\ncollapsed = true\n\n[clock]\nhour24 = true\n"
        );
        // Thiếu cả mục
        assert_eq!(
            set_value("[clock]\nhour24 = true", "lyrics", "collapsed", "false"),
            "[clock]\nhour24 = true\n\n[lyrics]\ncollapsed = false"
        );
        assert_eq!(
            set_value("", "lyrics", "collapsed", "true"),
            "[lyrics]\ncollapsed = true\n"
        );
    }

    #[test]
    fn set_value_replaces_whole_quoted_strings() {
        let text = "[weather]\ncity = \"Hà Nội\"     # tên thành phố\n";
        assert_eq!(
            set_value(text, "weather", "city", &quoted("Hồ Chí Minh")),
            "[weather]\ncity = \"Hồ Chí Minh\"     # tên thành phố\n"
        );
        let text = "[font]\nfamily = \"a \\\" # b\" # chú thích\n";
        assert_eq!(
            set_value(text, "font", "family", "\"X\""),
            "[font]\nfamily = \"X\" # chú thích\n"
        );
    }

    #[test]
    fn multiline_system_prompt_survives_saving() {
        let mut c = Config::default();
        c.ai.account_id = "abc123".into();
        c.ai.system_prompt = "Dòng 1\nDòng \"2\"\r\n\tDòng 3 \\ hết".into();
        let text = with_config(DEFAULT_TOML, &c);
        let back = Config::parse(&text).unwrap();
        assert_eq!(back.ai.system_prompt, "Dòng 1\nDòng \"2\"\n\tDòng 3 \\ hết");
        assert_eq!(back.ai.account_id, "abc123");
    }

    #[test]
    fn retired_lookup_section_moves_to_ai() {
        // File từ bản còn Tra cứu nhanh: vẫn đọc được, cấu hình AI chuyển sang [ai]
        let old = "[clock]\nhour24 = false\n\n[lookup]\nenabled = true\nhotkey = \"Ctrl+Alt+`\"\n\
                   model = \"@cf/openai/gpt-oss-120b\"\naccount_id = \"abc\"\n";
        let c = Config::parse(old).unwrap();
        assert!(!c.clock.hour24);
        assert_eq!(c.ai.account_id, "abc");
        assert_eq!(c.ai.model, "@cf/openai/gpt-oss-120b");
        assert_eq!(c.ai.system_prompt, AiConfig::default().system_prompt);
        // Lưu lại thì có mục [ai]; từ đó [ai] là chuẩn, mục [lookup] cũ bị bỏ qua
        let saved = with_config(old, &c);
        assert!(saved.contains("[ai]"));
        let back = Config::parse(&saved).unwrap();
        assert_eq!(back, c);
        let only_ai = Config::parse(&format!("{old}\n[ai]\naccount_id = \"xyz\"\n")).unwrap();
        assert_eq!(only_ai.ai.account_id, "xyz");
        assert_eq!(only_ai.ai.model, AiConfig::default().model);
    }

    #[test]
    fn saved_config_reads_back_the_same() {
        let mut c = Config::default();
        c.window.top = 42;
        c.window.expanded_width = 512.0;
        c.window.backdrop = BackdropKind::Acrylic;
        c.colors.background = Color::rgba(1, 2, 3, 0x80);
        c.font.family = "Cascadia Code".into();
        c.weather.city = "Thành phố \"HCM\" \\ Sài Gòn".into();
        c.autohide.enabled = false;
        c.lyrics.collapsed = true;
        c.lyrics.cache_mb = 100;
        c.alerts.notifications = false;
        c.alerts.seconds = 9;
        for start in [DEFAULT_TOML, "", "[clock]\nhour24 = true\n"] {
            let text = with_config(start, &c);
            assert_eq!(
                Config::parse(&text).unwrap(),
                c,
                "từ:\n{start}\nra:\n{text}"
            );
        }
        // Lưu lại mặc định lên file mẫu thì không đổi gì
        assert_eq!(with_config(DEFAULT_TOML, &Config::default()), DEFAULT_TOML);
    }

    #[test]
    fn color_display_round_trips() {
        for s in ["#0F1112", "#0F111290", "#FFFFFF00"] {
            assert_eq!(s.parse::<Color>().unwrap().to_string(), s);
        }
    }

    #[test]
    fn set_value_keeps_windows_line_endings() {
        let out = set_value(
            "[lyrics]\r\ncollapsed = false\r\n",
            "lyrics",
            "collapsed",
            "true",
        );
        assert_eq!(out, "[lyrics]\r\ncollapsed = true\r\n");
    }

    #[test]
    fn set_value_ignores_similar_keys_and_comments() {
        let text = "[lyrics]\ncollapsed_x = 1\n# collapsed = false\n";
        assert_eq!(
            set_value(text, "lyrics", "collapsed", "true"),
            "[lyrics]\ncollapsed_x = 1\n# collapsed = false\ncollapsed = true\n"
        );
    }

    #[test]
    fn backdrop_kinds() {
        let c = Config::parse("[window]\nbackdrop = \"acrylic\"").unwrap();
        assert_eq!(c.window.backdrop, BackdropKind::Acrylic);
        assert!(Config::parse("[window]\nbackdrop = \"glass\"").is_err());
    }

    #[test]
    fn color_with_alpha_channel() {
        assert_eq!("#0F111280".parse(), Ok(Color::rgba(0x0F, 0x11, 0x12, 0x80)));
        assert!(!"#0F1112".parse::<Color>().unwrap().is_translucent());
        assert!("#0F1112FE".parse::<Color>().unwrap().is_translucent());
        let c = Config::parse("[colors]\nbackground = \"#00000000\"").unwrap();
        assert_eq!(c.colors.background.a, 0);
    }

    #[test]
    fn color_rejects_bad_input_without_panicking() {
        let bad_inputs = [
            "",
            "#12345",
            "#1234567",
            "#123456789",
            "#GG0000",
            "#ÀÀÀ",
            "#+12345",
        ];
        for bad in bad_inputs {
            assert!(bad.parse::<Color>().is_err(), "{bad} lẽ ra phải lỗi");
        }
    }

    #[test]
    fn color_to_f32_spans_zero_to_one() {
        assert_eq!(Color::rgb(0, 255, 0).to_f32(), (0.0, 1.0, 0.0, 1.0));
        assert_eq!(Color::rgba(0, 0, 0, 0).to_f32().3, 0.0);
    }
}
