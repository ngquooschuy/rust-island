//! Thời tiết hiện tại từ Open-Meteo (miễn phí, không cần API key).
//! Chạy trên luồng nền riêng: tìm tọa độ theo tên thành phố một lần,
//! rồi cứ 15 phút tải lại; báo cho cửa sổ bằng `WM_WEATHER_CHANGED`.

use std::ffi::c_void;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use windows::{
    Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Networking::WinHttp::*,
        UI::WindowsAndMessaging::{PostMessageW, WM_APP},
    },
    core::{HSTRING, PCWSTR, w},
};

use crate::config::WeatherConfig;

/// Luồng thời tiết gửi thông điệp này về cửa sổ khi có số liệu mới
pub const WM_WEATHER_CHANGED: u32 = WM_APP + 3;

const REFRESH: Duration = Duration::from_secs(15 * 60);
/// Lỗi mạng (ví dụ vừa bật máy chưa có mạng) thì thử lại sớm hơn
const RETRY: Duration = Duration::from_secs(60);

/// Kiểu bầu trời, gom từ mã thời tiết WMO mà Open-Meteo trả về
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sky {
    Clear,
    PartlyCloudy,
    Cloudy,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunder,
}

impl Sky {
    /// Bảng mã: https://open-meteo.com/en/docs (mục "WMO Weather interpretation codes")
    pub fn from_wmo(code: u16) -> Self {
        match code {
            0 => Self::Clear,
            1 | 2 => Self::PartlyCloudy,
            45 | 48 => Self::Fog,
            51..=57 => Self::Drizzle,
            61..=67 | 80..=82 => Self::Rain,
            71..=77 | 85 | 86 => Self::Snow,
            95..=99 => Self::Thunder,
            _ => Self::Cloudy, // 3 và các mã lạ
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Clear => "Trời quang",
            Self::PartlyCloudy => "Có mây",
            Self::Cloudy => "Nhiều mây",
            Self::Fog => "Sương mù",
            Self::Drizzle => "Mưa phùn",
            Self::Rain => "Mưa",
            Self::Snow => "Tuyết",
            Self::Thunder => "Dông",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub temperature: f32,
    pub sky: Sky,
    /// Ban ngày thì vẽ mặt trời, ban đêm vẽ mặt trăng
    pub is_day: bool,
    /// Tên thành phố do Open-Meteo trả về, ví dụ "Hà Nội"
    pub place: String,
}

impl Weather {
    /// "31°" (đơn vị °C hay °F tùy cấu hình lúc tải)
    pub fn temperature_label(&self) -> String {
        // + 0.0 biến -0.0 thành 0.0, để không hiện "-0°"
        format!("{}°", self.temperature.round() + 0.0)
    }
}

// Dữ liệu dùng chung: luồng thời tiết ghi, luồng giao diện đọc
static WEATHER: Mutex<Option<Weather>> = Mutex::new(None);

/// Số liệu mới nhất, hoặc None nếu chưa tải được lần nào
pub fn snapshot() -> Option<Weather> {
    WEATHER.lock().unwrap().clone()
}

// ---------- Địa chỉ API và đọc JSON (thuần, có test) ----------

/// Mã hóa chuỗi để đặt vào URL: "Hà Nội" → "H%C3%A0%20N%E1%BB%99i"
fn percent_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn geocode_url(city: &str) -> String {
    format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=vi&format=json",
        percent_encode(city)
    )
}

fn forecast_url(place: &Place, fahrenheit: bool) -> String {
    let unit = if fahrenheit { "fahrenheit" } else { "celsius" };
    format!(
        "https://api.open-meteo.com/v1/forecast?latitude={}&longitude={}\
         &current=temperature_2m,weather_code,is_day&temperature_unit={unit}",
        place.latitude, place.longitude
    )
}

#[derive(Deserialize)]
struct GeoResponse {
    #[serde(default)] // không tìm thấy thì Open-Meteo bỏ hẳn trường "results"
    results: Vec<Place>,
}

#[derive(Deserialize, Debug, Clone, PartialEq)]
struct Place {
    name: String,
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct ForecastResponse {
    current: Current,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: f32,
    weather_code: u16,
    is_day: u8,
}

fn parse_place(json: &str) -> Result<Place, String> {
    let response: GeoResponse = serde_json::from_str(json).map_err(|e| e.to_string())?;
    response
        .results
        .into_iter()
        .next()
        .ok_or_else(|| "không tìm thấy thành phố".to_string())
}

fn parse_weather(json: &str, place: &Place) -> Result<Weather, String> {
    let response: ForecastResponse = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let c = response.current;
    Ok(Weather {
        temperature: c.temperature_2m,
        sky: Sky::from_wmo(c.weather_code),
        is_day: c.is_day == 1,
        place: place.name.clone(),
    })
}

// ---------- Mạng (dùng WinHTTP có sẵn trong Windows) ----------
// Không dùng HttpClient của WinRT: với ứng dụng desktop không đóng gói nó báo
// lỗi chứng chỉ 0x80072F8F, trong khi WinHTTP dùng chung tầng TLS với cả hệ thống.

/// Handle của WinHTTP, tự đóng khi ra khỏi phạm vi (RAII, giống `File` của Rust)
struct Handle(*mut c_void);

impl Handle {
    /// WinHTTP trả về con trỏ null khi lỗi; lý do nằm ở GetLastError
    fn new(raw: *mut c_void) -> Result<Self, String> {
        if raw.is_null() {
            Err(windows::core::Error::from_thread().to_string())
        } else {
            Ok(Self(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn open_session() -> Result<Handle, String> {
    let session = Handle::new(unsafe {
        WinHttpOpen(
            w!("DynamicIsland/0.1"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, // tự dùng proxy của hệ thống nếu có
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        )
    })?;
    // Phân giải tên, kết nối, gửi, nhận (mili giây)
    unsafe {
        let _ = WinHttpSetTimeouts(session.0, 10_000, 10_000, 10_000, 15_000);
    }
    Ok(session)
}

/// "https://host/path?query" → ("host", "/path?query")
fn split_url(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some(rest.split_at(slash))
}

fn http_get(session: &Handle, url: &str) -> Result<String, String> {
    let (host, path) = split_url(url).ok_or("URL không hợp lệ")?;
    let err = |e: windows::core::Error| e.to_string();
    unsafe {
        // Thứ tự khai báo quan trọng: Rust drop ngược lại, nên request đóng trước connect
        let connect = Handle::new(WinHttpConnect(
            session.0,
            &HSTRING::from(host),
            INTERNET_DEFAULT_HTTPS_PORT,
            0,
        ))?;
        let request = Handle::new(WinHttpOpenRequest(
            connect.0,
            w!("GET"),
            &HSTRING::from(path),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        ))?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(err)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(err)?;

        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&raw mut status).cast()),
            &mut size,
            std::ptr::null_mut(),
        )
        .map_err(err)?;
        if status != 200 {
            return Err(format!("máy chủ trả về HTTP {status}"));
        }

        // Đọc thân phản hồi từng khúc cho tới khi hết
        let mut body = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).map_err(err)?;
            if available == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + available as usize, 0u8);
            let mut read = 0u32;
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast(),
                available,
                &mut read,
            )
            .map_err(err)?;
            body.truncate(start + read as usize);
        }
        String::from_utf8(body).map_err(|e| e.to_string())
    }
}

/// Một lần cập nhật; nhớ tọa độ vào `place` để lần sau khỏi tìm lại
fn update(
    session: &Handle,
    config: &WeatherConfig,
    place: &mut Option<Place>,
) -> Result<Weather, String> {
    let found = match place {
        Some(p) => p.clone(),
        None => {
            let p = parse_place(&http_get(session, &geocode_url(&config.city))?)?;
            *place = Some(p.clone());
            p
        }
    };
    parse_weather(
        &http_get(session, &forecast_url(&found, config.fahrenheit))?,
        &found,
    )
}

/// Chỉ bản debug: ISLAND_FAKE_WEATHER="mã WMO,nhiệt độ[,n]" (n = ban đêm),
/// ví dụ "2,31" hay "61,24,n", để thử giao diện mà không cần mạng
fn fake() -> Option<Weather> {
    if !cfg!(debug_assertions) {
        return None;
    }
    let value = std::env::var("ISLAND_FAKE_WEATHER").ok()?;
    let mut parts = value.split(',');
    let code = parts.next()?.trim().parse().ok()?;
    let temperature = parts.next()?.trim().parse().ok()?;
    Some(Weather {
        temperature,
        sky: Sky::from_wmo(code),
        is_day: parts.next().map(str::trim) != Some("n"),
        place: "Thử nghiệm".into(),
    })
}

/// Vòng lặp của luồng thời tiết
pub fn run(hwnd_raw: isize, config: WeatherConfig) {
    let Ok(session) = open_session() else {
        return;
    };
    let hwnd = HWND(hwnd_raw as *mut c_void);

    let mut place = None;
    loop {
        let result = fake().map_or_else(|| update(&session, &config, &mut place), Ok);
        let wait = match result {
            Ok(weather) => {
                *WEATHER.lock().unwrap() = Some(weather);
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_WEATHER_CHANGED, WPARAM(0), LPARAM(0));
                }
                REFRESH
            }
            Err(_) => RETRY,
        };
        std::thread::sleep(wait);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEO_JSON: &str = r#"{"results":[{"id":1581130,"name":"Hà Nội","latitude":21.0245,
        "longitude":105.84117,"country":"Việt Nam"}],"generationtime_ms":0.24}"#;
    const FORECAST_JSON: &str = r#"{"latitude":21.05,"current_units":{"temperature_2m":"°C"},
        "current":{"time":"2026-11-09T23:45","interval":900,"temperature_2m":16.9,
        "weather_code":3,"is_day":0}}"#;

    fn hanoi() -> Place {
        parse_place(GEO_JSON).unwrap()
    }

    /// Gọi API thật nên mặc định bỏ qua; chạy bằng `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn live_fetch_from_open_meteo() {
        let session = open_session().unwrap();
        let mut place = None;
        let w = update(&session, &WeatherConfig::default(), &mut place).unwrap();
        println!("{w:?}");
        assert_eq!(w.place, "Hà Nội");
    }

    #[test]
    fn split_url_separates_host_and_path() {
        assert_eq!(
            split_url("https://api.open-meteo.com/v1/forecast?a=1"),
            Some(("api.open-meteo.com", "/v1/forecast?a=1"))
        );
        assert_eq!(split_url("http://example.com/"), None); // chỉ nhận https
        assert_eq!(split_url("https://example.com"), None); // thiếu đường dẫn
    }

    #[test]
    fn percent_encode_handles_vietnamese_and_spaces() {
        assert_eq!(percent_encode("Hà Nội"), "H%C3%A0%20N%E1%BB%99i");
        assert_eq!(percent_encode("Da-Nang_1.~"), "Da-Nang_1.~");
    }

    #[test]
    fn parses_first_geocoding_result() {
        let p = hanoi();
        assert_eq!(p.name, "Hà Nội");
        assert!((p.latitude - 21.0245).abs() < 1e-9);
    }

    #[test]
    fn unknown_city_is_an_error() {
        assert!(parse_place(r#"{"generationtime_ms":0.1}"#).is_err());
    }

    #[test]
    fn parses_current_weather() {
        let w = parse_weather(FORECAST_JSON, &hanoi()).unwrap();
        assert_eq!(w.temperature, 16.9);
        assert_eq!(w.sky, Sky::Cloudy);
        assert!(!w.is_day);
        assert_eq!(w.place, "Hà Nội");
    }

    #[test]
    fn forecast_url_has_coordinates_and_unit() {
        let url = forecast_url(&hanoi(), true);
        assert!(url.contains("latitude=21.0245&longitude=105.84117"));
        assert!(url.contains("temperature_unit=fahrenheit"));
    }

    #[test]
    fn wmo_codes_map_to_sky() {
        assert_eq!(Sky::from_wmo(0), Sky::Clear);
        assert_eq!(Sky::from_wmo(2), Sky::PartlyCloudy);
        assert_eq!(Sky::from_wmo(3), Sky::Cloudy);
        assert_eq!(Sky::from_wmo(53), Sky::Drizzle);
        assert_eq!(Sky::from_wmo(81), Sky::Rain);
        assert_eq!(Sky::from_wmo(75), Sky::Snow);
        assert_eq!(Sky::from_wmo(95), Sky::Thunder);
        assert_eq!(Sky::from_wmo(1234), Sky::Cloudy);
    }

    #[test]
    fn temperature_is_rounded_without_negative_zero() {
        let mut w = parse_weather(FORECAST_JSON, &hanoi()).unwrap();
        assert_eq!(w.temperature_label(), "17°");
        w.temperature = -0.4;
        assert_eq!(w.temperature_label(), "0°");
        w.temperature = -2.6;
        assert_eq!(w.temperature_label(), "-3°");
    }
}
