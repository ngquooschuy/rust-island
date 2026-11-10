//! Lò xo giảm chấn dùng cho hiệu ứng co giãn của island, và hiệu ứng trượt dòng chữ.

use std::time::{Duration, Instant};

pub struct Spring {
    pub value: f32,
    pub target: f32,
    velocity: f32,
}

impl Spring {
    const STIFFNESS: f32 = 320.0;
    const DAMPING: f32 = 24.0;

    pub fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            velocity: 0.0,
        }
    }

    /// Tiến thêm một bước thời gian `dt` (giây)
    pub fn update(&mut self, dt: f32) {
        let force = -Self::STIFFNESS * (self.value - self.target) - Self::DAMPING * self.velocity;
        self.velocity += force * dt;
        self.value += self.velocity * dt;
    }

    /// Đã gần như đứng yên tại đích chưa?
    pub fn is_settled(&self) -> bool {
        (self.value - self.target).abs() < 0.2 && self.velocity.abs() < 1.0
    }

    /// Nhảy thẳng tới đích và dừng hẳn
    pub fn snap(&mut self) {
        self.value = self.target;
        self.velocity = 0.0;
    }
}

/// Đổi dòng chữ có hiệu ứng: dòng cũ trượt lên và mờ đi, dòng mới trượt từ dưới lên.
/// Dùng cho lời bài hát trên island thu gọn
#[derive(Default)]
pub struct LineSlide {
    pub current: Option<String>,
    /// Dòng vừa bị thay, còn hiện trong lúc trượt đi
    pub previous: Option<String>,
    started: Option<Instant>,
}

impl LineSlide {
    pub const DURATION: Duration = Duration::from_millis(380);

    /// Đặt dòng đang hiện; trả về true nếu dòng đổi.
    /// Có dòng mới thì trượt vào; không còn dòng nào (None) thì xóa ngay, không trượt
    pub fn set(&mut self, line: Option<&str>, now: Instant) -> bool {
        if self.current.as_deref() == line {
            return false;
        }
        match line {
            Some(line) => {
                self.previous = self.current.replace(line.to_string());
                self.started = Some(now);
            }
            None => *self = Self::default(),
        }
        true
    }

    /// Tiến độ trượt từ 0 đến 1, chậm dần về cuối (ease-out) cho tự nhiên
    pub fn progress(&self, now: Instant) -> f32 {
        let Some(started) = self.started else {
            return 1.0;
        };
        let t = now.saturating_duration_since(started).as_secs_f32() / Self::DURATION.as_secs_f32();
        1.0 - (1.0 - t.min(1.0)).powi(3)
    }

    pub fn is_running(&self, now: Instant) -> bool {
        self.started
            .is_some_and(|s| now.saturating_duration_since(s) < Self::DURATION)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn line_slide_runs_once_per_new_line() {
        let t0 = Instant::now();
        let mut s = LineSlide::default();
        assert!(!s.is_running(t0));
        assert!(s.set(Some("dòng 1"), t0));
        assert!(!s.set(Some("dòng 1"), t0), "cùng dòng thì không trượt lại");
        assert!(s.is_running(t0));
        assert_eq!(s.progress(t0), 0.0);

        let later = t0 + LineSlide::DURATION;
        assert!(!s.is_running(later));
        assert_eq!(s.progress(later), 1.0);

        assert!(s.set(Some("dòng 2"), later));
        assert_eq!(s.previous.as_deref(), Some("dòng 1"));
        assert_eq!(s.current.as_deref(), Some("dòng 2"));
    }

    #[test]
    fn line_slide_progress_eases_out() {
        let t0 = Instant::now();
        let mut s = LineSlide::default();
        s.set(Some("a"), t0);
        let half = s.progress(t0 + LineSlide::DURATION / 2);
        assert!(
            half > 0.5 && half < 1.0,
            "nửa thời gian đã đi quá nửa đường: {half}"
        );
    }

    #[test]
    fn clearing_the_line_stops_at_once() {
        let t0 = Instant::now();
        let mut s = LineSlide::default();
        s.set(Some("a"), t0);
        assert!(s.set(None, t0));
        assert!(!s.is_running(t0));
        assert_eq!((s.current, s.previous), (None, None));
    }

    #[test]
    fn new_spring_is_settled() {
        assert!(Spring::new(10.0).is_settled());
    }

    #[test]
    fn moves_toward_new_target() {
        let mut s = Spring::new(0.0);
        s.target = 100.0;
        assert!(!s.is_settled());
        s.update(DT);
        assert!(s.value > 0.0 && s.value < 100.0);
    }

    #[test]
    fn settles_within_one_second() {
        let mut s = Spring::new(160.0);
        s.target = 400.0;
        let frames = (0..60).take_while(|_| {
            s.update(DT);
            !s.is_settled()
        });
        assert!(frames.count() < 60, "lò xo chưa dừng sau 1 giây");
        assert!((s.value - 400.0).abs() < 0.2);
    }

    #[test]
    fn snap_jumps_to_target_and_stops() {
        let mut s = Spring::new(0.0);
        s.target = 50.0;
        s.update(DT);
        s.snap();
        assert_eq!(s.value, 50.0);
        assert!(s.is_settled());
    }
}
