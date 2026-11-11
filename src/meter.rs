//! Mức âm thanh máy đang phát ra loa/tai nghe, để vẽ sóng nhạc trên trang Media Player.
//!
//! Dùng đồng hồ đo mức của Windows (`IAudioMeterInformation`, giống thanh xanh nhảy trong
//! Cài đặt âm thanh): chỉ đọc mức to nhỏ, không thu hay ghi lại âm thanh.
//! Một luồng nền đọc mức khoảng 40 lần mỗi giây, chỉ khi trang Media Player đang mở.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::{
    Media::Audio::{
        Endpoints::IAudioMeterInformation, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole,
        eRender,
    },
    System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx},
};

/// Đọc mức mỗi bao lâu khi đang cần
const SAMPLE_EVERY: Duration = Duration::from_millis(25);
/// Lúc không cần thì chỉ thức dậy xem có ai cần chưa
const IDLE_CHECK: Duration = Duration::from_millis(200);
/// Lấy lại thiết bị phát mặc định sau mỗi chừng này (người dùng có thể vừa cắm tai nghe)
const REFRESH_DEVICE: Duration = Duration::from_secs(3);

/// Số vạch của sóng nhạc
pub const WAVE_BARS: usize = 9;
/// Mức "to nhất gần đây" giảm dần mỗi khung hình (40 ms) theo tỉ lệ này: nhạc nhỏ đi
/// thì sau vài giây sóng lại cao như cũ
const REFERENCE_DECAY: f32 = 0.995;
/// Dưới mức này coi như im lặng (không phóng to tiếng ồn nền thành sóng)
const SILENCE: f32 = 0.02;
/// Làm rõ chênh lệch: so với mức to nhất gần đây rồi lũy thừa, chỗ nhạc dồn thì vạch cao vọt,
/// chỗ lặng thì vạch thấp hẳn
const CONTRAST: f32 = 2.0;

/// Đầu đọc mức âm thanh; luồng nền chạy suốt đời app, gần như không tốn gì khi không cần
pub struct Meter {
    /// Mức gần nhất (0–1), lưu dạng bit của f32
    level: Arc<AtomicU32>,
    wanted: Arc<AtomicBool>,
}

impl Meter {
    pub fn start() -> Self {
        let level = Arc::new(AtomicU32::new(0));
        let wanted = Arc::new(AtomicBool::new(false));
        let (l, w) = (level.clone(), wanted.clone());
        let _ = thread::Builder::new()
            .name("meter".into())
            .spawn(move || run(&l, &w));
        Self { level, wanted }
    }

    /// Bật/tắt việc đọc mức (chỉ bật khi trang Media Player đang mở và đang phát)
    pub fn set_wanted(&self, on: bool) {
        self.wanted.store(on, Ordering::Relaxed);
        if !on {
            self.level.store(0, Ordering::Relaxed);
        }
    }

    /// Mức to nhỏ lúc này, 0 (im lặng) đến 1 (to nhất)
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

fn open_meter() -> windows::core::Result<IAudioMeterInformation> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        device.Activate(CLSCTX_ALL, None)
    }
}

fn run(level: &AtomicU32, wanted: &AtomicBool) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut meter: Option<(IAudioMeterInformation, Instant)> = None;
    loop {
        if !wanted.load(Ordering::Relaxed) {
            meter = None;
            thread::sleep(IDLE_CHECK);
            continue;
        }
        if meter
            .as_ref()
            .is_none_or(|(_, since)| since.elapsed() > REFRESH_DEVICE)
        {
            meter = open_meter().ok().map(|m| (m, Instant::now()));
        }
        let peak = meter
            .as_ref()
            .and_then(|(m, _)| unsafe { m.GetPeakValue() }.ok());
        match peak {
            Some(v) => level.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed),
            None => {
                // Không có thiết bị phát (vừa rút tai nghe...): coi như im lặng, thử lại sau
                meter = None;
                level.store(0, Ordering::Relaxed);
            }
        }
        thread::sleep(SAMPLE_EVERY);
    }
}

/// Các vạch sóng nhạc: mỗi khung hình đẩy mức mới vào bên phải, các vạch cũ trôi sang trái.
/// Chiều cao tính so với mức to nhất gần đây, vì nhạc đã mix to thì mức đo gần như lúc nào
/// cũng cao: vẽ thẳng mức đo thì các vạch cao bằng nhau, không thấy nhịp
#[derive(Clone, Debug, PartialEq)]
pub struct Wave {
    /// Chiều cao từng vạch, 0–1
    pub bars: [f32; WAVE_BARS],
    /// Mức to nhất gần đây (giảm dần theo thời gian)
    reference: f32,
}

impl Default for Wave {
    fn default() -> Self {
        Self {
            bars: [0.0; WAVE_BARS],
            reference: 0.0,
        }
    }
}

impl Wave {
    /// Thêm mức đo được (0–1)
    pub fn push(&mut self, level: f32) {
        let level = level.clamp(0.0, 1.0);
        self.reference = level.max(self.reference * REFERENCE_DECAY);
        let shown = if self.reference < SILENCE {
            0.0
        } else {
            (level / self.reference).powf(CONTRAST)
        };
        self.bars.rotate_left(1);
        self.bars[WAVE_BARS - 1] = shown;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_levels_enter_on_the_right_and_scroll_left() {
        let mut w = Wave::default();
        w.push(0.8);
        assert_eq!(w.bars[WAVE_BARS - 1], 1.0); // to nhất gần đây: cao hết cỡ
        w.push(0.4);
        assert_eq!(w.bars[WAVE_BARS - 2], 1.0);
        // Nhỏ hơn một nửa so với lúc to nhất: thấp hẳn xuống (lũy thừa 2)
        assert!((w.bars[WAVE_BARS - 1] - 0.25).abs() < 0.01);
    }

    #[test]
    fn loud_music_still_shows_its_beat() {
        // Nhạc to, mức đo dao động 0.7–0.9: các vạch vẫn phải chênh nhau rõ
        let mut w = Wave::default();
        for i in 0..40 {
            w.push(if i % 3 == 0 { 0.9 } else { 0.7 });
        }
        let (lo, hi) = w
            .bars
            .iter()
            .fold((1.0f32, 0.0f32), |(lo, hi), &b| (lo.min(b), hi.max(b)));
        assert!(hi - lo > 0.3, "{:?}", w.bars);
    }

    #[test]
    fn silence_stays_flat_and_quiet_music_grows_back() {
        let mut w = Wave::default();
        w.push(0.005);
        w.push(-1.0); // giá trị lạ thì coi như im lặng
        assert!(w.bars.iter().all(|&b| b == 0.0));
        // Sau đoạn rất to, nhạc nhỏ đi: vài giây sau sóng lại cao gần hết cỡ
        w.push(1.0);
        for _ in 0..400 {
            w.push(0.3);
        }
        assert!(w.bars[WAVE_BARS - 1] > 0.9);
    }
}
