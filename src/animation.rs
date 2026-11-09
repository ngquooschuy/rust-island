//! Lò xo giảm chấn dùng cho hiệu ứng co giãn của island.

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

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

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
