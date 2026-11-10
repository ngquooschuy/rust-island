//! Thông báo tạm thời trên island: nội dung vừa sao chép và thông báo Windows mới.
//!
//! Một luồng nền nhận lần sao chép từ luồng giao diện (đọc clipboard phải ở luồng có
//! cửa sổ), hỏi thông báo Windows mỗi giây, tìm tên và biểu tượng 64px của ứng dụng
//! (có bộ nhớ đệm, mỗi app chỉ trích một lần), rồi xếp vào hàng đợi và báo cửa sổ
//! bằng `WM_ALERT`. Giao diện lấy ra bằng [`take`].

use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    System::Com::{COINIT_MULTITHREADED, CoInitializeEx},
    UI::WindowsAndMessaging::{PostMessageW, WM_APP},
};

use crate::clipboard::{self, Clip, Copied};
use crate::icon::{self, Icon};
use crate::notifications::{Listener, Toast};

/// Luồng nền gửi thông điệp này khi có thông báo mới để hiện
pub const WM_ALERT: u32 = WM_APP + 5;

/// Hỏi thông báo Windows bao lâu một lần
const POLL: Duration = Duration::from_secs(1);
/// Cùng một nội dung được báo sao chép lại trong khoảng này thì coi là một lần
/// (nhiều app ghi clipboard vài đợt liền nhau, hoặc ghi lại sau khi "chốt" dữ liệu)
const SAME_COPY_WINDOW: Duration = Duration::from_secs(3);
/// Hàng đợi chưa kịp hiện dài tối đa bao nhiêu (đầy thì bỏ cái cũ nhất)
const MAX_QUEUED: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Clipboard,
    Notification,
}

/// Một thông báo để hiện trên island
#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    pub kind: Kind,
    /// Tên ứng dụng ("Google Chrome")
    pub app: String,
    pub title: String,
    pub body: String,
    /// Biểu tượng 64px của app, hoặc ảnh thu nhỏ của ảnh vừa sao chép
    pub icon: Option<Arc<Icon>>,
}

static QUEUE: Mutex<VecDeque<Alert>> = Mutex::new(VecDeque::new());

/// Lấy hết thông báo đang chờ (cũ trước mới sau)
pub fn take() -> Vec<Alert> {
    QUEUE.lock().unwrap().drain(..).collect()
}

/// Thêm vào hàng đợi; lần sao chép mới thay hẳn lần sao chép cũ chưa kịp hiện
pub fn push(queue: &mut VecDeque<Alert>, alert: Alert) {
    if alert.kind == Kind::Clipboard {
        queue.retain(|a| a.kind != Kind::Clipboard);
    }
    queue.push_back(alert);
    while queue.len() > MAX_QUEUED {
        queue.pop_front();
    }
}

/// Chạy luồng nền (`notifications`: có đọc thông báo Windows không).
/// Trả về đầu gửi lần sao chép; bỏ đầu gửi đi thì luồng thôi nhận lần sao chép
pub fn start(hwnd_raw: isize, notifications: bool) -> Sender<Clip> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || run(hwnd_raw, rx, notifications));
    tx
}

/// Biểu tượng theo khóa, mỗi khóa chỉ trích một lần
struct Icons(HashMap<String, Option<Arc<Icon>>>);

impl Icons {
    fn get(&mut self, key: String, load: impl FnOnce() -> Option<Icon>) -> Option<Arc<Icon>> {
        self.0
            .entry(key)
            .or_insert_with(|| load().map(Arc::new))
            .clone()
    }
}

fn from_clip(clip: Clip, icons: &mut Icons) -> Alert {
    let (title, body) = clipboard::describe(&clip.content);
    let (app, app_icon) = match &clip.source {
        Some(exe) => {
            let key = format!("exe:{}", exe.display());
            (icon::app_name(exe), icons.get(key, || icon::from_exe(exe)))
        }
        None => ("Clipboard".to_string(), None),
    };
    // Ảnh vừa sao chép: hiện chính ảnh đó thay cho biểu tượng app
    let icon = match clip.content {
        Copied::Image {
            thumbnail: Some(t), ..
        } => Some(Arc::new(t)),
        _ => app_icon,
    };
    Alert {
        kind: Kind::Clipboard,
        app,
        title,
        body,
        icon,
    }
}

fn from_toast(t: Toast, icons: &mut Icons) -> Alert {
    let app_id = t.app_id.clone();
    let icon = (!app_id.is_empty())
        .then(|| icons.get(format!("app:{app_id}"), || icon::from_app_id(&app_id)))
        .flatten();
    Alert {
        kind: Kind::Notification,
        app: t.app,
        title: t.title,
        body: t.body,
        icon,
    }
}

fn post(hwnd_raw: isize, alerts: Vec<Alert>) {
    if alerts.is_empty() {
        return;
    }
    {
        let mut queue = QUEUE.lock().unwrap();
        for alert in alerts {
            push(&mut queue, alert);
        }
    }
    let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_ALERT, WPARAM(0), LPARAM(0));
    }
}

fn run(hwnd_raw: isize, clips: Receiver<Clip>, notifications: bool) {
    // Lấy biểu tượng qua Shell cần COM trên luồng này
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut listener = if notifications {
        Listener::open()
    } else {
        None
    };
    let mut icons = Icons(HashMap::new());
    let mut clips_open = true;
    let mut last_copy: Option<(Alert, Instant)> = None;
    loop {
        if clips_open {
            match clips.recv_timeout(POLL) {
                Ok(clip) => {
                    let alert = from_clip(clip, &mut icons);
                    let now = Instant::now();
                    if !is_repeat(last_copy.as_ref(), &alert, now) {
                        post(hwnd_raw, vec![alert.clone()]);
                    }
                    last_copy = Some((alert, now));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => clips_open = false,
            }
        } else if listener.is_some() {
            std::thread::sleep(POLL);
        } else {
            return; // không còn nguồn nào
        }
        if let Some(l) = &mut listener {
            let fresh = l
                .poll()
                .into_iter()
                .map(|t| from_toast(t, &mut icons))
                .collect();
            post(hwnd_raw, fresh);
        }
    }
}

/// Lần sao chép này giống hệt lần ngay trước (cùng app, cùng nội dung, cách nhau rất ngắn)?
fn is_repeat(last: Option<&(Alert, Instant)>, alert: &Alert, now: Instant) -> bool {
    last.is_some_and(|(prev, at)| {
        prev == alert && now.saturating_duration_since(*at) < SAME_COPY_WINDOW
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(kind: Kind, title: &str) -> Alert {
        Alert {
            kind,
            app: String::new(),
            title: title.into(),
            body: String::new(),
            icon: None,
        }
    }

    #[test]
    fn newer_copy_replaces_older_copy_but_not_notifications() {
        let mut q = VecDeque::new();
        push(&mut q, alert(Kind::Clipboard, "1"));
        push(&mut q, alert(Kind::Notification, "n"));
        push(&mut q, alert(Kind::Clipboard, "2"));
        let titles: Vec<_> = q.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["n", "2"]);
    }

    #[test]
    fn quick_identical_copies_count_once() {
        let a = alert(Kind::Clipboard, "x");
        let t0 = Instant::now();
        let last = (a.clone(), t0);
        assert!(is_repeat(Some(&last), &a, t0 + Duration::from_millis(400)));
        assert!(!is_repeat(Some(&last), &a, t0 + SAME_COPY_WINDOW));
        assert!(!is_repeat(Some(&last), &alert(Kind::Clipboard, "y"), t0));
        assert!(!is_repeat(None, &a, t0));
    }

    #[test]
    fn queue_is_bounded() {
        let mut q = VecDeque::new();
        for i in 0..10 {
            push(&mut q, alert(Kind::Notification, &i.to_string()));
        }
        assert_eq!(q.len(), MAX_QUEUED);
        assert_eq!(q.front().unwrap().title, "5");
    }
}
