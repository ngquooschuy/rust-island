//! Thông báo Windows mới nhất, đọc qua `UserNotificationListener`.
//!
//! Windows không báo sự kiện "có thông báo mới" cho ứng dụng không đóng gói (chỉ app
//! Store mới nhận được), nên cứ mỗi giây hỏi danh sách một lần và so với lần trước.
//! Những thông báo đã có sẵn lúc mở app được coi là đã xem, không hiện lại.

use std::collections::HashSet;

use windows::UI::Notifications::Management::{
    UserNotificationListener, UserNotificationListenerAccessStatus,
};
use windows::UI::Notifications::{KnownNotificationBindings, NotificationKinds, UserNotification};

/// Một thông báo mới, đã rút ra những gì cần hiện
#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub id: u32,
    /// Tên ứng dụng gửi ("Phone Link")
    pub app: String,
    /// Mã ứng dụng AppUserModelID, dùng để lấy biểu tượng
    pub app_id: String,
    pub title: String,
    pub body: String,
}

/// Một lần cùng lúc có nhiều thông báo mới thì chỉ hiện tối đa chừng này cái mới nhất
const MAX_PER_POLL: usize = 3;

pub struct Listener {
    listener: UserNotificationListener,
    /// Mã các thông báo đã thấy ở lần hỏi trước
    seen: HashSet<u32>,
}

impl Listener {
    /// Xin quyền đọc thông báo; None nếu Windows không cho
    /// (Cài đặt > Quyền riêng tư > Thông báo > cho ứng dụng đọc thông báo)
    pub fn open() -> Option<Self> {
        let listener = UserNotificationListener::Current().ok()?;
        let allowed = UserNotificationListenerAccessStatus::Allowed;
        let status = match listener.GetAccessStatus() {
            Ok(s) if s == allowed => s,
            _ => listener
                .RequestAccessAsync()
                .and_then(|op| op.join())
                .ok()?,
        };
        if status != allowed {
            return None;
        }
        let mut me = Self {
            listener,
            seen: HashSet::new(),
        };
        me.poll(); // thông báo có sẵn từ trước: đánh dấu đã xem
        Some(me)
    }

    /// Các thông báo mới từ lần hỏi trước, cũ trước mới sau
    pub fn poll(&mut self) -> Vec<Toast> {
        let list = self
            .listener
            .GetNotificationsAsync(NotificationKinds::Toast)
            .and_then(|op| op.join());
        let Ok(list) = list else {
            return Vec::new(); // lỗi tạm thời: giữ nguyên danh sách đã thấy
        };
        let mut current = HashSet::new();
        let mut fresh = Vec::new();
        for n in list {
            let Ok(id) = n.Id() else { continue };
            current.insert(id);
            if !self.seen.contains(&id)
                && let Some(toast) = read_toast(id, &n)
            {
                fresh.push(toast);
            }
        }
        self.seen = current;
        newest(fresh)
    }
}

/// Giữ tối đa MAX_PER_POLL thông báo mới nhất, xếp cũ trước mới sau
fn newest(mut toasts: Vec<Toast>) -> Vec<Toast> {
    toasts.sort_by_key(|t| t.id); // mã thông báo tăng dần theo thời gian
    let skip = toasts.len().saturating_sub(MAX_PER_POLL);
    toasts.split_off(skip)
}

/// Dòng đầu là tiêu đề, các dòng sau gộp thành nội dung
pub fn split_texts(texts: &[String]) -> (String, String) {
    let mut lines = texts.iter().map(|t| t.trim()).filter(|t| !t.is_empty());
    let title = lines.next().unwrap_or_default().to_string();
    let body = lines.collect::<Vec<_>>().join(" · ");
    (title, body)
}

fn read_toast(id: u32, n: &UserNotification) -> Option<Toast> {
    let app = n.AppInfo().ok()?;
    let name = app
        .DisplayInfo()
        .and_then(|d| d.DisplayName())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let texts: Vec<String> = n
        .Notification()
        .and_then(|x| x.Visual())
        .and_then(|v| v.GetBinding(&KnownNotificationBindings::ToastGeneric()?))
        .and_then(|b| b.GetTextElements())
        .map(|list| {
            list.into_iter()
                .filter_map(|e| e.Text().ok())
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    let (title, body) = split_texts(&texts);
    (!title.is_empty()).then(|| Toast {
        id,
        app: name,
        app_id: app
            .AppUserModelId()
            .map(|s| s.to_string())
            .unwrap_or_default(),
        title,
        body,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(id: u32) -> Toast {
        Toast {
            id,
            app: String::new(),
            app_id: String::new(),
            title: id.to_string(),
            body: String::new(),
        }
    }

    #[test]
    fn texts_become_title_and_body() {
        let texts = [
            "Shopee ".into(),
            "".into(),
            "Đơn hàng".into(),
            "đã giao".into(),
        ];
        assert_eq!(
            split_texts(&texts),
            ("Shopee".into(), "Đơn hàng · đã giao".into())
        );
        assert_eq!(split_texts(&[]), (String::new(), String::new()));
    }

    #[test]
    fn only_the_newest_few_are_kept_in_order() {
        let ids: Vec<u32> = newest(vec![toast(5), toast(1), toast(9), toast(7)])
            .iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, [5, 7, 9]);
    }

    /// Thăm dò: app không đóng gói có đọc được thông báo không? (chỉ in số lượng và tên app)
    /// cargo test probe_notifications -- --ignored --nocapture
    #[test]
    #[ignore]
    fn probe_notifications() {
        let listener = UserNotificationListener::Current().unwrap();
        println!("quyền: {:?}", listener.GetAccessStatus());
        match Listener::open() {
            Some(l) => println!("đọc được, đang có {} thông báo", l.seen.len()),
            None => println!("Windows không cho đọc thông báo"),
        }
    }
}
