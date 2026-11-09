//! Trạng thái pin của máy (phần trăm, đang cắm sạc hay không).

use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Battery {
    pub percent: u8,
    /// Đang cắm nguồn điện
    pub charging: bool,
}

impl Battery {
    /// Pin yếu (≤ 20%) và không cắm sạc
    pub fn is_low(self) -> bool {
        self.percent <= 20 && !self.charging
    }
}

const NO_BATTERY: u8 = 128; // cờ BatteryFlag: máy không có pin
const UNKNOWN: u8 = 255;

// Tách riêng để test được với dữ liệu giả
fn from_status(s: &SYSTEM_POWER_STATUS) -> Option<Battery> {
    if s.BatteryFlag == UNKNOWN
        || s.BatteryFlag & NO_BATTERY != 0
        || s.BatteryLifePercent == UNKNOWN
    {
        return None;
    }
    Some(Battery {
        percent: s.BatteryLifePercent.min(100),
        charging: s.ACLineStatus == 1,
    })
}

/// Pin hiện tại, hoặc None nếu máy không có pin (máy bàn)
pub fn status() -> Option<Battery> {
    // Chỉ bản debug: ISLAND_FAKE_BATTERY=57 (hoặc 57c = đang sạc) để thử giao diện trên máy bàn
    #[cfg(debug_assertions)]
    if let Ok(fake) = std::env::var("ISLAND_FAKE_BATTERY") {
        let charging = fake.ends_with('c');
        if let Ok(percent) = fake.trim_end_matches('c').parse::<u8>() {
            return Some(Battery {
                percent: percent.min(100),
                charging,
            });
        }
    }

    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    from_status(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(flag: u8, percent: u8, ac: u8) -> SYSTEM_POWER_STATUS {
        SYSTEM_POWER_STATUS {
            ACLineStatus: ac,
            BatteryFlag: flag,
            BatteryLifePercent: percent,
            ..Default::default()
        }
    }

    fn bat(percent: u8, charging: bool) -> Battery {
        Battery { percent, charging }
    }

    #[test]
    fn reads_percent_and_charging() {
        assert_eq!(from_status(&status(1, 85, 1)), Some(bat(85, true)));
        assert_eq!(from_status(&status(1, 85, 0)), Some(bat(85, false)));
    }

    #[test]
    fn desktop_without_battery_gives_none() {
        assert_eq!(from_status(&status(128, 255, 1)), None);
        assert_eq!(from_status(&status(255, 255, 255)), None);
        assert_eq!(from_status(&status(1, 255, 0)), None);
    }

    #[test]
    fn low_only_when_not_charging() {
        assert!(bat(15, false).is_low());
        assert!(!bat(15, true).is_low());
        assert!(!bat(21, false).is_low());
    }
}
