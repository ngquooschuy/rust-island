//! Nhờ trợ lý AI đoán tên bài và ca sĩ thật khi tìm lời theo tên đang phát không ra
//! (tên video YouTube lộn xộn, kênh lyrics/vietsub đăng lại, tên bài kèm chữ thừa...).
//!
//! Chỉ gửi đi tên đang phát, tên kênh/nghệ sĩ, album và thời lượng. Câu trả lời (kể cả
//! "không biết") được nhớ trong `%APPDATA%\DynamicIsland\lyrics_ai_names.json`, nên mỗi bài
//! chỉ hỏi AI một lần. File này nằm ngoài thư mục cache lời để không bị tính là một bài.

use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::lrclib::{Track, name_key};
use crate::ai;

/// Nhớ tối đa chừng này bài; quá thì bỏ bài hỏi lâu nhất
const MEMO_LIMIT: usize = 1000;

/// Lời dặn cho AI: chỉ trả về JSON để đọc được bằng máy
const SYSTEM_PROMPT: &str = "You identify songs from what a music player shows: often a \
YouTube video title and channel name. Reply with ONLY one JSON object, no other text: \
{\"title\": \"<official song title>\", \"artist\": \"<main performing artist>\"}. \
Use the song's original published name and spelling, keeping Vietnamese diacritics. \
Drop extras that are not part of the official title: Official MV, Lyrics, Vietsub, Karaoke, \
4K, featured artists, hashtags, channel tags. The channel may be a fan, lyrics or re-upload \
channel instead of the artist. If it is not a song or you are not confident, reply \
{\"title\": null, \"artist\": null}.";

/// Tên bài và ca sĩ AI đoán được
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Names {
    pub title: String,
    pub artist: String,
}

/// Câu hỏi gửi AI cho một bài
fn question(t: &Track) -> String {
    let mut q = format!(
        "Title: {}\nChannel or artist: {}",
        t.title.trim(),
        t.artist.trim()
    );
    if !t.album.trim().is_empty() {
        q.push_str(&format!("\nAlbum: {}", t.album.trim()));
    }
    if t.duration_s > 0 {
        q.push_str(&format!(
            "\nDuration: {}:{:02}",
            t.duration_s / 60,
            t.duration_s % 60
        ));
    }
    q
}

/// Đọc câu trả lời của AI. None nếu AI không biết, trả lời lạ, hoặc đoán y như tên đang
/// phát (tìm lại cũng vô ích)
fn parse_answer(text: &str, t: &Track) -> Option<Names> {
    #[derive(Deserialize)]
    struct Answer {
        title: Option<String>,
        artist: Option<String>,
    }
    // Model hay bọc JSON trong ```json ... ``` hoặc nói thêm một câu: chỉ lấy phần {...}
    let json = &text[text.find('{')?..=text.rfind('}')?];
    let answer: Answer = serde_json::from_str(json).ok()?;
    let title = answer.title.unwrap_or_default().trim().to_string();
    let artist = answer.artist.unwrap_or_default().trim().to_string();
    if name_key(&title).is_empty() {
        return None;
    }
    let same = name_key(&title) == name_key(&t.title) && name_key(&artist) == name_key(&t.artist);
    (!same).then_some(Names { title, artist })
}

/// Các câu trả lời đã có, theo thứ tự hỏi (cũ nhất trước)
#[derive(Debug, Default, Serialize, Deserialize)]
struct Memo {
    entries: Vec<(String, Option<Names>)>,
}

impl Memo {
    fn key(t: &Track) -> String {
        format!("{}\u{1f}{}", name_key(&t.title), name_key(&t.artist))
    }

    fn get(&self, t: &Track) -> Option<&Option<Names>> {
        let key = Self::key(t);
        self.entries.iter().find(|(k, _)| *k == key).map(|(_, n)| n)
    }

    fn put(&mut self, t: &Track, names: Option<Names>) {
        let key = Self::key(t);
        self.entries.retain(|(k, _)| *k != key);
        self.entries.push((key, names));
        let extra = self.entries.len().saturating_sub(MEMO_LIMIT);
        self.entries.drain(..extra);
    }
}

static MEMO: Mutex<Option<Memo>> = Mutex::new(None);

fn memo_path() -> Option<PathBuf> {
    crate::config::path().map(|p| p.with_file_name("lyrics_ai_names.json"))
}

/// Chạy `f` với bộ nhớ câu trả lời (đọc từ đĩa ở lần đầu)
fn with_memo<R>(f: impl FnOnce(&mut Memo) -> R) -> R {
    let mut memo = MEMO.lock().unwrap();
    let memo = memo.get_or_insert_with(|| {
        memo_path()
            .and_then(|p| fs::read_to_string(p).ok())
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    });
    f(memo)
}

fn save(memo: &Memo) {
    if let (Some(path), Ok(text)) = (memo_path(), serde_json::to_string(memo)) {
        let _ = fs::write(path, text);
    }
}

/// Quên hết câu trả lời đã nhớ (khi người dùng xóa bộ nhớ đệm lời bài hát)
pub fn forget_all() {
    with_memo(|memo| {
        memo.entries.clear();
        save(memo);
    });
    if let Some(path) = memo_path() {
        let _ = fs::remove_file(path);
    }
}

/// Cấu hình AI để hỏi tên bài (token đọc từ Credential Manager mỗi lần hỏi)
pub struct Guesser {
    account_id: String,
    model: String,
}

impl Guesser {
    /// None nếu chưa nhập Account ID
    pub fn new(account_id: &str, model: &str) -> Option<Self> {
        (!account_id.trim().is_empty()).then(|| Self {
            account_id: account_id.trim().to_string(),
            model: model.to_string(),
        })
    }

    /// Tên bài và ca sĩ thật của `t`. Ok(None): AI không biết (đã nhớ, lần sau không hỏi
    /// lại). Err: chưa có token hoặc gọi AI lỗi (không nhớ, lần sau phát lại sẽ hỏi lại)
    pub fn names_for(&self, t: &Track) -> Result<Option<Names>, String> {
        if let Some(known) = with_memo(|m| m.get(t).cloned()) {
            return Ok(known);
        }
        let token = crate::secret::load_token().ok_or("chưa lưu API token")?;
        let settings = ai::Settings {
            account_id: self.account_id.clone(),
            token,
            model: self.model.clone(),
            system_prompt: SYSTEM_PROMPT.to_string(),
        };
        let answer = ai::ask_raw(&settings, &[ai::Message::user(&question(t))])?;
        let names = parse_answer(&answer, t);
        with_memo(|memo| {
            memo.put(t, names.clone());
            save(memo);
        });
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str) -> Track {
        Track {
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_s: 0,
        }
    }

    #[test]
    fn question_lists_what_the_player_shows() {
        let t = Track {
            title: "[Vietsub] Shape of You".into(),
            artist: "Nhạc Hay Mỗi Ngày".into(),
            album: String::new(),
            duration_s: 263,
        };
        assert_eq!(
            question(&t),
            "Title: [Vietsub] Shape of You\nChannel or artist: Nhạc Hay Mỗi Ngày\nDuration: 4:23"
        );
    }

    #[test]
    fn reads_json_even_inside_code_fences() {
        let t = track("CHÚNG TA CỦA HIỆN TẠI | OFFICIAL MV", "Lyrics Việt");
        let answer =
            "```json\n{\"title\": \"Chúng Ta Của Hiện Tại\", \"artist\": \"Sơn Tùng M-TP\"}\n```";
        assert_eq!(
            parse_answer(answer, &t),
            Some(Names {
                title: "Chúng Ta Của Hiện Tại".into(),
                artist: "Sơn Tùng M-TP".into(),
            })
        );
    }

    #[test]
    fn unknown_odd_or_unchanged_answers_give_nothing() {
        let t = track("Lạc Trôi", "Sơn Tùng M-TP");
        assert_eq!(
            parse_answer("{\"title\": null, \"artist\": null}", &t),
            None
        );
        assert_eq!(parse_answer("Mình không chắc bài này.", &t), None);
        assert_eq!(parse_answer("{\"title\": \"  \"}", &t), None);
        // Đoán y như tên đang phát (chỉ khác hoa thường, dấu): không có gì mới để tìm
        assert_eq!(
            parse_answer(
                "{\"title\": \"lạc trôi\", \"artist\": \"Son Tung M-TP\"}",
                &t
            ),
            None
        );
        // Đúng tên bài nhưng ra ca sĩ khác: vẫn đáng tìm lại
        assert!(parse_answer("{\"title\": \"Lạc Trôi\", \"artist\": \"Triple D\"}", &t).is_some());
    }

    #[test]
    fn memo_keeps_newest_answers() {
        let mut memo = Memo::default();
        for i in 0..MEMO_LIMIT + 5 {
            memo.put(&track(&format!("Bài {i}"), "A"), None);
        }
        assert_eq!(memo.entries.len(), MEMO_LIMIT);
        assert!(memo.get(&track("Bài 0", "A")).is_none());
        assert_eq!(memo.get(&track("bài 1004", "a")), Some(&None));
        // Hỏi lại một bài: thay câu trả lời cũ, không thêm dòng mới
        let names = Names {
            title: "X".into(),
            artist: "Y".into(),
        };
        memo.put(&track("Bài 1004", "A"), Some(names.clone()));
        assert_eq!(memo.entries.len(), MEMO_LIMIT);
        assert_eq!(memo.get(&track("Bài 1004", "A")), Some(&Some(names)));
    }
}
