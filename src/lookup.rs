//! Tra cứu nhanh (Quick Lookup): hộp hỏi AI ngay trên island.
//!
//! - Ô nhập chữ tự làm (island vẽ bằng Direct2D nên không có ô nhập của Windows):
//!   gõ, xóa, di chuyển con trỏ, dán.
//! - Các lượt hỏi-đáp của cuộc trò chuyện hiện tại; câu hỏi gửi đi kèm vài lượt gần nhất.
//! - Lịch sử tìm kiếm lưu ở `%APPDATA%\DynamicIsland\lookup_history.json`.
//! - Gọi AI trên luồng nền, xong thì báo cửa sổ bằng `WM_LOOKUP_DONE`.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use windows::Win32::{
    Foundation::{HWND, LPARAM, WPARAM},
    UI::WindowsAndMessaging::{PostMessageW, WM_APP},
};

use crate::ai::{self, Message};

/// Luồng nền gửi thông điệp này về cửa sổ khi AI trả lời xong (hoặc lỗi)
pub const WM_LOOKUP_DONE: u32 = WM_APP + 6;

/// Số câu tìm kiếm giữ lại trong lịch sử
const HISTORY_MAX: usize = 20;
/// Câu hỏi dài tối đa (ký tự), tránh dán nhầm cả trang văn bản
pub const INPUT_MAX: usize = 2000;

// ---------- Ô nhập chữ ----------

/// Chữ trong ô nhập và vị trí con trỏ (tính theo ký tự)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Editor {
    chars: Vec<char>,
    caret: usize,
    /// Nửa đầu của ký tự UTF-16 dài (emoji) đang chờ nửa sau từ WM_CHAR
    high_surrogate: Option<u16>,
}

impl Editor {
    pub fn text(&self) -> String {
        self.chars.iter().collect()
    }

    /// Phần chữ trước con trỏ (để đo vị trí con trỏ khi vẽ)
    pub fn before_caret(&self) -> String {
        self.chars[..self.caret].iter().collect()
    }

    pub fn is_empty(&self) -> bool {
        self.chars.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        self.chars.clear();
        self.caret = 0;
        self.insert_str(text);
    }

    pub fn clear(&mut self) {
        self.set("");
    }

    /// Chèn chữ tại con trỏ; xuống dòng và tab thành dấu cách (ô nhập chỉ có một dòng)
    pub fn insert_str(&mut self, text: &str) {
        for c in text.chars() {
            let c = if c.is_whitespace() { ' ' } else { c };
            if c.is_control() || self.chars.len() >= INPUT_MAX {
                continue;
            }
            // Dán nhiều dấu cách liền nhau (từ xuống dòng) thì giữ một
            if c == ' ' && self.caret > 0 && self.chars[self.caret - 1] == ' ' {
                continue;
            }
            self.chars.insert(self.caret, c);
            self.caret += 1;
        }
    }

    /// Một đơn vị UTF-16 từ WM_CHAR (bộ gõ tiếng Việt gửi qua đây)
    pub fn push_utf16(&mut self, unit: u16) {
        match unit {
            0xD800..=0xDBFF => self.high_surrogate = Some(unit),
            0xDC00..=0xDFFF => {
                if let Some(high) = self.high_surrogate.take()
                    && let Some(Ok(c)) = char::decode_utf16([high, unit]).next()
                {
                    self.insert_str(&c.to_string());
                }
            }
            _ => {
                self.high_surrogate = None;
                if let Some(c) = char::from_u32(u32::from(unit)) {
                    self.insert_str(&c.to_string());
                }
            }
        }
    }

    pub fn backspace(&mut self) {
        if self.caret > 0 {
            self.caret -= 1;
            self.chars.remove(self.caret);
        }
    }

    /// Ctrl+Backspace: xóa cả từ đứng trước con trỏ
    pub fn backspace_word(&mut self) {
        while self.caret > 0 && self.chars[self.caret - 1] == ' ' {
            self.backspace();
        }
        while self.caret > 0 && self.chars[self.caret - 1] != ' ' {
            self.backspace();
        }
    }

    pub fn delete(&mut self) {
        if self.caret < self.chars.len() {
            self.chars.remove(self.caret);
        }
    }

    pub fn left(&mut self) {
        self.caret = self.caret.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.caret = (self.caret + 1).min(self.chars.len());
    }

    pub fn home(&mut self) {
        self.caret = 0;
    }

    pub fn end(&mut self) {
        self.caret = self.chars.len();
    }
}

// ---------- Cuộc trò chuyện ----------

#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    /// Đang chờ AI
    Pending,
    Text(String),
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub question: String,
    pub answer: Answer,
}

/// Trạng thái hộp Tra cứu nhanh (giữ cả khi đóng hộp, tới khi bấm "Mới")
#[derive(Default)]
pub struct Session {
    pub editor: Editor,
    pub turns: Vec<Turn>,
    /// Đã cuộn lên bao nhiêu (px) so với cuối cuộc trò chuyện
    pub scroll: f32,
    /// Mã của câu đang chờ trả lời; câu trả lời của cuộc trò chuyện cũ (đã bấm "Mới") bỏ qua
    pending: Option<u64>,
    next_id: u64,
}

impl Session {
    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Câu trả lời gần nhất (để sao chép bằng Ctrl+C)
    pub fn last_answer(&self) -> Option<&str> {
        self.turns.iter().rev().find_map(|t| match &t.answer {
            Answer::Text(text) => Some(text.as_str()),
            _ => None,
        })
    }

    /// Gửi chữ trong ô nhập: thêm lượt mới (đang chờ), xóa ô nhập.
    /// Trả về (mã, các lượt chat cần gửi cho AI); None nếu ô trống hoặc đang chờ câu trước
    pub fn submit(&mut self) -> Option<(u64, String, Vec<Message>)> {
        let question = self.editor.text().trim().to_string();
        if question.is_empty() || self.is_pending() {
            return None;
        }
        self.editor.clear();
        Some(self.ask(question))
    }

    /// Hỏi thẳng một câu (bấm vào lịch sử, clipboard)
    pub fn ask(&mut self, question: String) -> (u64, String, Vec<Message>) {
        // Gửi kèm vài lượt gần nhất đã có câu trả lời, để AI hiểu ngữ cảnh ("còn cái kia?")
        let mut chat: Vec<Message> = Vec::new();
        let answered = self
            .turns
            .iter()
            .filter_map(|t| match &t.answer {
                Answer::Text(a) => Some((t.question.as_str(), a.as_str())),
                _ => None,
            })
            .collect::<Vec<_>>();
        for (q, a) in answered.iter().rev().take(ai::CONTEXT_TURNS).rev() {
            chat.push(Message::user(q));
            chat.push(Message::assistant(a));
        }
        chat.push(Message::user(&question));

        self.next_id += 1;
        let id = self.next_id;
        self.pending = Some(id);
        self.turns.push(Turn {
            question: question.clone(),
            answer: Answer::Pending,
        });
        self.scroll = 0.0;
        (id, question, chat)
    }

    /// AI trả lời xong câu có mã `id`
    pub fn finish(&mut self, id: u64, result: Result<String, String>) {
        if self.pending != Some(id) {
            return; // câu của cuộc trò chuyện đã xóa
        }
        self.pending = None;
        if let Some(turn) = self.turns.last_mut() {
            turn.answer = match result {
                Ok(text) => Answer::Text(text),
                Err(e) => Answer::Error(e),
            };
        }
        self.scroll = 0.0;
    }

    /// Bắt đầu cuộc trò chuyện mới
    pub fn new_chat(&mut self) {
        self.turns.clear();
        self.pending = None;
        self.scroll = 0.0;
        self.editor.clear();
    }
}

// ---------- Gợi ý dưới ô nhập ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SuggestionKind {
    Clipboard,
    History,
}

/// Một dòng gợi ý: bấm vào là hỏi luôn câu đó
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub kind: SuggestionKind,
    pub text: String,
}

/// Các dòng gợi ý: nội dung clipboard (nếu là chữ, khác câu đầu lịch sử) rồi lịch sử
pub fn suggestions(clipboard: Option<&str>, history: &History, max: usize) -> Vec<Suggestion> {
    let mut rows = Vec::new();
    if let Some(clip) = clipboard {
        // Clipboard dài thì chỉ lấy đoạn đầu làm câu hỏi
        let text: String = clip.split_whitespace().collect::<Vec<_>>().join(" ");
        let text: String = text.chars().take(INPUT_MAX).collect();
        if !text.is_empty() {
            rows.push(Suggestion {
                kind: SuggestionKind::Clipboard,
                text,
            });
        }
    }
    for item in &history.items {
        if rows.len() >= max {
            break;
        }
        if rows.iter().any(|r| r.text == *item) {
            continue;
        }
        rows.push(Suggestion {
            kind: SuggestionKind::History,
            text: item.clone(),
        });
    }
    rows.truncate(max);
    rows
}

// ---------- Lịch sử tìm kiếm ----------

fn history_path() -> Option<PathBuf> {
    crate::config::path().map(|p| p.with_file_name("lookup_history.json"))
}

/// Câu đã hỏi, mới nhất trước
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    pub items: Vec<String>,
}

impl History {
    pub fn load() -> Self {
        let items = history_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
            .unwrap_or_default();
        Self { items }
    }

    fn save(&self) {
        if let Some(path) = history_path()
            && let Ok(json) = serde_json::to_string_pretty(&self.items)
        {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::write(path, json);
        }
    }

    /// Thêm câu vừa hỏi lên đầu (hỏi lại câu cũ thì đưa nó lên đầu, không lặp)
    pub fn push(&mut self, question: &str) {
        self.remember(question);
        self.save();
    }

    fn remember(&mut self, question: &str) {
        let q = question.trim();
        if q.is_empty() {
            return;
        }
        self.items
            .retain(|old| !old.eq_ignore_ascii_case(q) && old != q);
        self.items.insert(0, q.to_string());
        self.items.truncate(HISTORY_MAX);
    }
}

// ---------- Gọi AI trên luồng nền ----------

/// Câu trả lời đã xong, chờ luồng giao diện lấy
static RESULTS: Mutex<Vec<(u64, Result<String, String>)>> = Mutex::new(Vec::new());

/// Hỏi AI trên luồng riêng; xong thì báo cửa sổ `hwnd_raw`
pub fn ask_async(hwnd_raw: isize, id: u64, settings: ai::Settings, chat: Vec<Message>) {
    std::thread::spawn(move || {
        let result = ai::ask(&settings, &chat);
        RESULTS.lock().unwrap().push((id, result));
        let hwnd = HWND(hwnd_raw as *mut std::ffi::c_void);
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_LOOKUP_DONE, WPARAM(0), LPARAM(0));
        }
    });
}

/// Lấy hết các câu trả lời đã xong
pub fn take_results() -> Vec<(u64, Result<String, String>)> {
    std::mem::take(&mut *RESULTS.lock().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(text: &str) -> Editor {
        let mut e = Editor::default();
        for unit in text.encode_utf16() {
            e.push_utf16(unit);
        }
        e
    }

    #[test]
    fn typing_vietnamese_and_emoji() {
        let e = typed("Phở ngon 🍜");
        assert_eq!(e.text(), "Phở ngon 🍜");
        assert_eq!(e.before_caret(), "Phở ngon 🍜");
    }

    #[test]
    fn editing_at_the_caret() {
        let mut e = typed("pizza");
        e.home();
        e.insert_str("Vì sao ");
        assert_eq!(e.text(), "Vì sao pizza");
        e.end();
        e.backspace();
        e.left();
        e.delete();
        assert_eq!(e.text(), "Vì sao piz");
        e.backspace_word();
        assert_eq!(e.text(), "Vì sao ");
        e.backspace_word();
        assert_eq!(e.text(), "Vì ");
        // Ở đầu/cuối thì không làm gì, không lỗi
        e.home();
        e.backspace();
        e.left();
        e.end();
        e.delete();
        e.right();
        assert_eq!(e.text(), "Vì ");
    }

    #[test]
    fn pasting_turns_lines_into_one() {
        let mut e = Editor::default();
        e.insert_str("dòng 1\r\n\r\ndòng\t2\u{7}");
        assert_eq!(e.text(), "dòng 1 dòng 2");
        e.set(&"a".repeat(INPUT_MAX + 50));
        assert_eq!(e.text().len(), INPUT_MAX);
    }

    #[test]
    fn conversation_sends_recent_turns_as_context() {
        let mut s = Session::default();
        s.editor.set("  Pizza là gì  ");
        let (id, q, chat) = s.submit().unwrap();
        assert_eq!(q, "Pizza là gì");
        assert_eq!(chat, vec![Message::user("Pizza là gì")]);
        assert!(s.editor.is_empty() && s.is_pending());
        // Đang chờ thì không gửi thêm
        s.editor.set("câu khác");
        assert!(s.submit().is_none());
        s.finish(id, Ok("Món Ý".into()));
        assert_eq!(s.last_answer(), Some("Món Ý"));

        let (_, _, chat) = s.submit().unwrap();
        assert_eq!(
            chat,
            vec![
                Message::user("Pizza là gì"),
                Message::assistant("Món Ý"),
                Message::user("câu khác"),
            ]
        );
    }

    #[test]
    fn errors_are_shown_but_not_sent_as_context() {
        let mut s = Session::default();
        let (id, _, _) = s.ask("hỏi 1".into());
        s.finish(id, Err("Token sai".into()));
        assert_eq!(s.turns[0].answer, Answer::Error("Token sai".into()));
        let (_, _, chat) = s.ask("hỏi 2".into());
        assert_eq!(chat, vec![Message::user("hỏi 2")]);
    }

    #[test]
    fn answers_for_a_cleared_chat_are_ignored() {
        let mut s = Session::default();
        let (old, _, _) = s.ask("cũ".into());
        s.new_chat();
        s.finish(old, Ok("trễ".into()));
        assert!(s.turns.is_empty() && !s.is_pending());
    }

    #[test]
    fn context_is_limited_to_recent_turns() {
        let mut s = Session::default();
        for i in 0..10 {
            let (id, _, _) = s.ask(format!("q{i}"));
            s.finish(id, Ok(format!("a{i}")));
        }
        let (_, _, chat) = s.ask("cuối".into());
        assert_eq!(chat.len(), ai::CONTEXT_TURNS * 2 + 1);
        assert_eq!(chat[0], Message::user("q4"));
    }

    #[test]
    fn suggestions_put_clipboard_first_then_history() {
        let mut h = History::default();
        for q in ["c", "b", "a"] {
            h.remember(q);
        }
        let rows = suggestions(Some("  dòng 1\n dòng 2 "), &h, 3);
        assert_eq!(rows[0].kind, SuggestionKind::Clipboard);
        assert_eq!(rows[0].text, "dòng 1 dòng 2");
        assert_eq!(
            rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            ["dòng 1 dòng 2", "a", "b"]
        );
        // Clipboard trùng câu đã hỏi thì không lặp lại; không có clipboard thì chỉ lịch sử
        let rows = suggestions(Some("a"), &h, 5);
        assert_eq!(rows.len(), 3);
        assert_eq!(suggestions(None, &h, 2).len(), 2);
        assert!(suggestions(Some("   "), &History::default(), 5).is_empty());
    }

    #[test]
    fn history_keeps_latest_first_without_duplicates() {
        let mut h = History::default();
        for q in ["a", "b", "A", "  ", "c"] {
            h.remember(q);
        }
        assert_eq!(h.items, ["c", "A", "b"]);
        for i in 0..30 {
            h.remember(&i.to_string());
        }
        assert_eq!(h.items.len(), HISTORY_MAX);
        assert_eq!(h.items[0], "29");
    }
}
