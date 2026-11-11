//! Thời tiết hiện tại và dự báo ngày mai từ Open-Meteo (miễn phí, không cần API key).
//! Chạy trên luồng nền riêng: tìm tọa độ theo tên thành phố một lần,
//! rồi cứ 15 phút tải lại; báo cho cửa sổ bằng `WM_WEATHER_CHANGED`.

use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, WM_APP},
};

use crate::config::WeatherConfig;
use crate::http::{Session, percent_encode};

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

/// Dự báo cho một ngày
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DayForecast {
    pub sky: Sky,
    /// Khả năng có mưa cao nhất trong ngày, phần trăm (None nếu Open-Meteo không có số liệu)
    pub rain_chance: Option<u8>,
}

impl DayForecast {
    /// "60%", hoặc "--" nếu không có số liệu
    pub fn rain_label(&self) -> String {
        self.rain_chance
            .map_or_else(|| "--".to_string(), |p| format!("{p}%"))
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
    /// Nhiệt độ cảm nhận (tính cả gió, độ ẩm); None nếu Open-Meteo không trả về
    pub feels_like: Option<f32>,
    /// Độ ẩm, phần trăm
    pub humidity: Option<u8>,
    /// Dự báo ngày mai
    pub tomorrow: Option<DayForecast>,
}

/// "31°" (đơn vị °C hay °F tùy cấu hình lúc tải)
pub fn degrees(t: f32) -> String {
    // + 0.0 biến -0.0 thành 0.0, để không hiện "-0°"
    format!("{}°", t.round() + 0.0)
}

impl Weather {
    /// "31°"
    pub fn temperature_label(&self) -> String {
        degrees(self.temperature)
    }
}

// Dữ liệu dùng chung: luồng thời tiết ghi, luồng giao diện đọc
static WEATHER: Mutex<Option<Weather>> = Mutex::new(None);

/// Số liệu mới nhất, hoặc None nếu chưa tải được lần nào
pub fn snapshot() -> Option<Weather> {
    WEATHER.lock().unwrap().clone()
}

// ---------- Địa chỉ API và đọc JSON (thuần, có test) ----------

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
         &current=temperature_2m,weather_code,is_day,apparent_temperature,relative_humidity_2m\
         &daily=weather_code,precipitation_probability_max&forecast_days=2\
         &timezone=auto&temperature_unit={unit}",
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
    /// Mỗi trường là một mảng, phần tử 0 là hôm nay, 1 là ngày mai
    #[serde(default)]
    daily: Option<Daily>,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: f32,
    weather_code: u16,
    is_day: u8,
    #[serde(default)]
    apparent_temperature: Option<f32>,
    #[serde(default)]
    relative_humidity_2m: Option<f32>,
}

#[derive(Deserialize, Default)]
struct Daily {
    #[serde(default)]
    weather_code: Vec<Option<u16>>,
    #[serde(default)]
    precipitation_probability_max: Vec<Option<f32>>,
}

impl Daily {
    /// Dự báo của ngày thứ `i` (0 = hôm nay); không có kiểu trời thì None
    fn day(&self, i: usize) -> Option<DayForecast> {
        let rain = self.precipitation_probability_max.get(i).copied().flatten();
        Some(DayForecast {
            sky: Sky::from_wmo((*self.weather_code.get(i)?)?),
            rain_chance: rain.map(|p| p.round().clamp(0.0, 100.0) as u8),
        })
    }
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
        feels_like: c.apparent_temperature,
        humidity: c
            .relative_humidity_2m
            .map(|h| h.round().clamp(0.0, 100.0) as u8),
        tomorrow: response.daily.and_then(|d| d.day(1)),
    })
}

/// Một lần cập nhật; nhớ tọa độ vào `place` để lần sau khỏi tìm lại
fn update(
    session: &Session,
    config: &WeatherConfig,
    place: &mut Option<Place>,
) -> Result<Weather, String> {
    let found = match place {
        Some(p) => p.clone(),
        None => {
            let p = parse_place(&session.get(&geocode_url(&config.city))?.ok()?)?;
            *place = Some(p.clone());
            p
        }
    };
    parse_weather(
        &session
            .get(&forecast_url(&found, config.fahrenheit))?
            .ok()?,
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
        feels_like: Some(temperature + 3.0),
        humidity: Some(42),
        tomorrow: Some(DayForecast {
            sky: Sky::Rain,
            rain_chance: Some(60),
        }),
    })
}

/// Vòng lặp của luồng thời tiết
pub fn run(hwnd_raw: isize, config: WeatherConfig) {
    let Ok(session) = Session::open() else {
        return;
    };
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);

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
        "weather_code":3,"is_day":0,"apparent_temperature":15.2,"relative_humidity_2m":81},
        "daily":{"time":["2026-11-09","2026-11-10"],"weather_code":[3,61],
        "precipitation_probability_max":[15,72.4]}}"#;
    /// Bản trả về cũ (chỉ có thời tiết hiện tại) vẫn đọc được
    const CURRENT_ONLY_JSON: &str = r#"{"current":{"temperature_2m":30.0,"weather_code":0,
        "is_day":1}}"#;

    fn hanoi() -> Place {
        parse_place(GEO_JSON).unwrap()
    }

    /// Gọi API thật nên mặc định bỏ qua; chạy bằng `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn live_fetch_from_open_meteo() {
        let session = Session::open().unwrap();
        let mut place = None;
        let w = update(&session, &WeatherConfig::default(), &mut place).unwrap();
        println!("{w:?}");
        assert_eq!(w.place, "Hà Nội");
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
    fn parses_feels_like_humidity_and_tomorrow() {
        let w = parse_weather(FORECAST_JSON, &hanoi()).unwrap();
        assert_eq!(w.feels_like, Some(15.2));
        assert_eq!(w.humidity, Some(81));
        let tomorrow = w.tomorrow.unwrap();
        assert_eq!(tomorrow.sky, Sky::Rain);
        assert_eq!(tomorrow.rain_chance, Some(72));
        assert_eq!(tomorrow.rain_label(), "72%");

        let old = parse_weather(CURRENT_ONLY_JSON, &hanoi()).unwrap();
        assert_eq!(
            (old.feels_like, old.humidity, old.tomorrow),
            (None, None, None)
        );
    }

    #[test]
    fn missing_forecast_values() {
        // Không có kiểu trời ngày mai: không có dự báo
        let json = r#"{"current":{"temperature_2m":30.0,"weather_code":0,"is_day":1},
            "daily":{"weather_code":[0,null],"precipitation_probability_max":[10,40]}}"#;
        assert_eq!(parse_weather(json, &hanoi()).unwrap().tomorrow, None);
        // Có kiểu trời nhưng thiếu khả năng mưa: hiện "--"
        let json = r#"{"current":{"temperature_2m":30.0,"weather_code":0,"is_day":1},
            "daily":{"weather_code":[0,2],"precipitation_probability_max":[10,null]}}"#;
        let tomorrow = parse_weather(json, &hanoi()).unwrap().tomorrow.unwrap();
        assert_eq!(tomorrow.rain_chance, None);
        assert_eq!(tomorrow.rain_label(), "--");
    }

    #[test]
    fn forecast_url_has_coordinates_and_unit() {
        let url = forecast_url(&hanoi(), true);
        assert!(url.contains("latitude=21.0245&longitude=105.84117"));
        assert!(url.contains("temperature_unit=fahrenheit"));
        assert!(url.contains("apparent_temperature,relative_humidity_2m"));
        assert!(url.contains("&daily=weather_code,precipitation_probability_max"));
        assert!(url.contains("&forecast_days=2"));
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
