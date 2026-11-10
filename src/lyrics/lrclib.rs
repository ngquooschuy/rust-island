//! Tra lời bài hát trên LRCLIB (https://lrclib.net), cơ sở dữ liệu lời miễn phí, mã nguồn mở.
//!
//! Gửi đi: tên bài, nghệ sĩ, album và thời lượng của bài đang phát.
//! Cách tìm:
//! 1. `/api/get`: khớp chính xác cả bốn thông tin (nhanh, đúng nhất).
//! 2. `/api/search?q=tên bài + nghệ sĩ đầu tiên`: vì mỗi ứng dụng ghi nghệ sĩ một kiểu
//!    ("Warriyo, LXNGVX, Laura Brehm" so với "LXNGVX, Warriyo").
//! 3. Như bước 2 nhưng tên bài đã bỏ phần phụ ("(feat. X)", " - Remastered").
//! 4. Tên kiểu video YouTube ("SƠN TÙNG M-TP | CHÚNG TA CỦA HIỆN TẠI | OFFICIAL MUSIC VIDEO",
//!    kênh "Sơn Tùng M-TP Official"): tách ra tên bài và nghệ sĩ rồi tìm lại.
//!
//! Khi tìm, chọn bản có thời lượng lệch không quá 3 giây để lời khớp nhịp.

use serde::Deserialize;

use crate::http::{Session, percent_encode};

/// Bài cần tìm lời
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// 0 nếu trình phát không báo thời lượng
    pub duration_s: u32,
}

/// Kết quả tra cứu
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    Lyrics {
        synced: Option<String>,
        plain: Option<String>,
    },
    Instrumental,
    NotFound,
}

/// Lệch thời lượng tối đa giữa bài đang phát và bản trên LRCLIB
const MAX_DURATION_DIFF_S: f64 = 3.0;

/// Một bản ghi LRCLIB (chỉ lấy các trường cần dùng)
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
struct Record {
    #[serde(default)]
    duration: f64,
    #[serde(default)]
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

/// Chuỗi rỗng hoặc toàn khoảng trắng coi như không có
fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

impl Record {
    fn into_found(self) -> Lookup {
        let synced = non_empty(self.synced_lyrics);
        let plain = non_empty(self.plain_lyrics);
        if synced.is_some() || plain.is_some() {
            Lookup::Lyrics { synced, plain }
        } else if self.instrumental {
            Lookup::Instrumental
        } else {
            Lookup::NotFound
        }
    }

    /// Hạng để so sánh: có lời đồng bộ > chỉ có lời thường > nhạc không lời
    fn rank(&self) -> u8 {
        let has = |s: &Option<String>| s.as_deref().is_some_and(|s| !s.trim().is_empty());
        if has(&self.synced_lyrics) {
            0
        } else if has(&self.plain_lyrics) {
            1
        } else if self.instrumental {
            2
        } else {
            3
        }
    }
}

/// Nghệ sĩ đầu tiên: "Warriyo, LXNGVX & Laura Brehm" → "Warriyo"
fn first_artist(artist: &str) -> &str {
    artist
        .split([',', '&', ';', '/'])
        .next()
        .unwrap_or(artist)
        .trim()
}

/// Bỏ phần phụ của tên bài: "Song (feat. X) - Remastered 2011" → "Song"
fn clean_title(title: &str) -> String {
    let main = title.split(" - ").next().unwrap_or(title);
    let mut out = String::new();
    let mut depth = 0usize;
    for c in main.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    let cleaned = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() {
        title.trim().to_string()
    } else {
        cleaned
    }
}

/// Những từ chỉ mô tả video, không thuộc tên bài: "OFFICIAL MUSIC VIDEO", "Lyrics", "4K"...
const NOISE_WORDS: &[&str] = &[
    "official",
    "music",
    "video",
    "videos",
    "mv",
    "m/v",
    "lyric",
    "lyrics",
    "audio",
    "visualizer",
    "visualiser",
    "hd",
    "hq",
    "4k",
    "remaster",
    "remastered",
    "version",
    "ver",
    "clip",
    "teaser",
    "full",
    "song",
    "nhạc",
    "chính",
    "thức",
];

/// Đoạn chỉ gồm toàn từ mô tả video (hoặc số, như năm "2011")
fn is_noise(segment: &str) -> bool {
    segment.split_whitespace().all(|w| {
        let w = w.to_lowercase();
        let w = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
        w.is_empty() || w.chars().all(|c| c.is_ascii_digit()) || NOISE_WORDS.contains(&w)
    })
}

/// Tên kênh → tên nghệ sĩ: "Sơn Tùng M-TP Official" → "Sơn Tùng M-TP",
/// "Rick Astley - Topic" → "Rick Astley", "TaylorSwiftVEVO" → "TaylorSwift"
fn clean_artist(channel: &str) -> String {
    let mut s = channel.trim().to_string();
    loop {
        let lower = s.to_lowercase();
        let suffix = [" - topic", " official", "official", "vevo", " channel"]
            .into_iter()
            .find(|suf| lower.ends_with(suf) && lower.len() > suf.len());
        let Some(suf) = suffix else { break };
        // Hậu tố toàn chữ ASCII nên cắt theo số byte của bản viết thường là an toàn
        s.truncate(s.len() - suf.len());
        s = s.trim_end_matches([' ', '-', '|']).to_string();
    }
    s
}

/// Đoán (tên bài, nghệ sĩ) từ tên video. None nếu tên trông đã là tên bài bình thường
fn guess_from_video_title(title: &str, artist: &str) -> Option<(String, String)> {
    let separators = [" | ", "｜", " - ", " – ", " — "];
    let has_separator = separators.iter().any(|s| title.contains(s));
    let has_noise_brackets =
        title != clean_title(title) && is_noise(&title.replace(['(', ')', '[', ']'], " "));
    if !has_separator && !has_noise_brackets {
        return None;
    }

    // Tách theo mọi dấu ngăn cách, bỏ phần trong ngoặc và các đoạn chỉ mô tả video
    let mut parts = vec![title.to_string()];
    for sep in separators {
        parts = parts
            .iter()
            .flat_map(|p| p.split(sep).map(String::from).collect::<Vec<_>>())
            .collect();
    }
    let parts: Vec<String> = parts
        .iter()
        .map(|p| clean_title(p))
        .filter(|p| !p.is_empty() && !is_noise(p))
        .collect();

    let channel = clean_artist(artist);
    let same = |a: &str, b: &str| a.to_lowercase() == b.to_lowercase();
    // Đoạn trùng tên nghệ sĩ thì bỏ, đoạn còn lại là tên bài
    if let Some(i) = parts.iter().position(|p| same(p, &channel)) {
        let title = parts
            .iter()
            .enumerate()
            .find(|&(j, _)| j != i)
            .map(|(_, p)| p.clone())?;
        return Some((title, parts[i].clone()));
    }
    match parts.as_slice() {
        // Kiểu phổ biến nhất: "Nghệ sĩ - Tên bài"
        [first, second, ..] => Some((second.clone(), first.clone())),
        [only] => Some((only.clone(), channel)),
        [] => None,
    }
}

fn get_url(t: &Track) -> String {
    format!(
        "https://lrclib.net/api/get?track_name={}&artist_name={}&album_name={}&duration={}",
        percent_encode(&t.title),
        percent_encode(&t.artist),
        percent_encode(&t.album),
        t.duration_s
    )
}

fn search_url(query: &str) -> String {
    format!("https://lrclib.net/api/search?q={}", percent_encode(query))
}

/// Chọn bản tốt nhất trong kết quả tìm kiếm
fn pick_best(records: Vec<Record>, duration_s: u32) -> Lookup {
    let diff = |r: &Record| (r.duration - f64::from(duration_s)).abs();
    records
        .into_iter()
        // Không biết thời lượng thì nhận mọi bản; biết thì chỉ nhận bản gần đúng
        .filter(|r| duration_s == 0 || diff(r) <= MAX_DURATION_DIFF_S)
        .min_by(|a, b| a.rank().cmp(&b.rank()).then(diff(a).total_cmp(&diff(b))))
        .map_or(Lookup::NotFound, Record::into_found)
}

/// Các câu tìm kiếm, theo thứ tự thử, không trùng nhau
fn search_queries(t: &Track) -> Vec<String> {
    let artist = first_artist(&t.artist);
    let mut queries = vec![
        // 2) Tên bài + nghệ sĩ đầu tiên
        format!("{} {artist}", t.title.trim()),
        // 3) Tên bài đã bỏ phần phụ
        format!("{} {artist}", clean_title(&t.title)),
    ];
    // 4) Tên kiểu video YouTube: tên bài + nghệ sĩ đoán được, rồi tên bài một mình
    //    (phòng khi tên kênh khác hẳn tên nghệ sĩ trên LRCLIB)
    if let Some((title, artist)) = guess_from_video_title(&t.title, &t.artist) {
        queries.push(format!("{title} {artist}"));
        queries.push(title);
    }
    let mut seen = Vec::new();
    queries.retain(|q| {
        let key = q.to_lowercase();
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    queries
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// Tra lời cho một bài. Lỗi mạng trả về Err (không được lưu vào cache)
pub fn fetch(session: &Session, t: &Track) -> Result<Lookup, String> {
    // 1) Khớp chính xác (cần thời lượng)
    if t.duration_s > 0 {
        let response = session.get(&get_url(t))?;
        match response.status {
            200 => {
                let found = parse_json::<Record>(&response.body)?.into_found();
                if found != Lookup::NotFound {
                    return Ok(found);
                }
            }
            404 => {} // không khớp chính xác: chuyển sang tìm kiếm
            status => return Err(format!("LRCLIB trả về HTTP {status}")),
        }
    }

    for query in search_queries(t) {
        let body = session.get(&search_url(&query))?.ok()?;
        let found = pick_best(parse_json(&body)?, t.duration_s);
        if found != Lookup::NotFound {
            return Ok(found);
        }
    }
    Ok(Lookup::NotFound)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(duration: f64, synced: Option<&str>, plain: Option<&str>) -> Record {
        Record {
            duration,
            instrumental: false,
            synced_lyrics: synced.map(String::from),
            plain_lyrics: plain.map(String::from),
        }
    }

    /// Gọi LRCLIB thật nên mặc định bỏ qua; chạy bằng `cargo test -- --ignored`
    #[test]
    #[ignore]
    fn live_fetch_finds_synced_lyrics() {
        let session = Session::open().unwrap();
        // Spotify ghi nghệ sĩ khác LRCLIB ("LXNGVX, Warriyo"): phải tìm ra nhờ bước search
        let t = Track {
            title: "Mortals Funk Remix".into(),
            artist: "Warriyo, LXNGVX, Laura Brehm".into(),
            album: "Mortals Funk Remix".into(),
            duration_s: 146,
        };
        match fetch(&session, &t).unwrap() {
            Lookup::Lyrics { synced, .. } => {
                let synced = synced.expect("có lời đồng bộ");
                println!("{}", &synced[..synced.len().min(120)]);
                assert!(synced.contains("Movin' with the motion"));
            }
            other => panic!("không tìm thấy lời: {other:?}"),
        }
    }

    #[test]
    fn reads_lrclib_json_in_camel_case() {
        let json = r#"{"id":17958388,"trackName":"Mortals Funk Remix","artistName":"LXNGVX, Warriyo",
            "duration":146.0,"instrumental":false,"plainLyrics":"Stranded",
            "syncedLyrics":"[00:01.95] Stranded"}"#;
        let found = parse_json::<Record>(json).unwrap().into_found();
        assert_eq!(
            found,
            Lookup::Lyrics {
                synced: Some("[00:01.95] Stranded".into()),
                plain: Some("Stranded".into()),
            }
        );
    }

    #[test]
    fn instrumental_and_empty_records() {
        let json = r#"{"duration":200,"instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#;
        assert_eq!(
            parse_json::<Record>(json).unwrap().into_found(),
            Lookup::Instrumental
        );
        assert_eq!(record(1.0, Some("  "), None).into_found(), Lookup::NotFound);
    }

    #[test]
    fn first_artist_splits_common_separators() {
        assert_eq!(first_artist("Warriyo, LXNGVX, Laura Brehm"), "Warriyo");
        assert_eq!(first_artist("Simon & Garfunkel"), "Simon");
        assert_eq!(first_artist("Solo"), "Solo");
    }

    #[test]
    fn clean_title_drops_extras() {
        assert_eq!(clean_title("Song (feat. X) - Remastered 2011"), "Song");
        assert_eq!(clean_title("Mortals Funk Remix"), "Mortals Funk Remix");
        assert_eq!(clean_title("[Intro]"), "[Intro]"); // không để tên rỗng
    }

    fn guess(title: &str, artist: &str) -> Option<(String, String)> {
        guess_from_video_title(title, artist)
    }

    fn pair(title: &str, artist: &str) -> Option<(String, String)> {
        Some((title.into(), artist.into()))
    }

    #[test]
    fn youtube_title_with_artist_first() {
        // Chính video đang phát trên máy lúc tìm lỗi
        assert_eq!(
            guess(
                "SƠN TÙNG M-TP | CHÚNG TA CỦA HIỆN TẠI | OFFICIAL MUSIC VIDEO",
                "Sơn Tùng M-TP Official"
            ),
            pair("CHÚNG TA CỦA HIỆN TẠI", "SƠN TÙNG M-TP")
        );
        assert_eq!(
            guess(
                "Rick Astley - Never Gonna Give You Up (Official Music Video)",
                "Rick Astley"
            ),
            pair("Never Gonna Give You Up", "Rick Astley")
        );
    }

    #[test]
    fn youtube_title_with_title_first_or_unknown_channel() {
        assert_eq!(
            guess(
                "Nơi Này Có Anh | Official Music Video | Sơn Tùng M-TP",
                "Sơn Tùng M-TP Official"
            ),
            pair("Nơi Này Có Anh", "Sơn Tùng M-TP")
        );
        // Kênh khác tên nghệ sĩ: đoán theo kiểu "Nghệ sĩ - Tên bài"
        assert_eq!(
            guess("Lana Del Rey - Video Games", "LanaDelReyVEVO"),
            pair("Video Games", "Lana Del Rey")
        );
    }

    #[test]
    fn normal_song_titles_are_left_alone() {
        assert_eq!(guess("Mortals Funk Remix", "LXNGVX, Warriyo"), None);
        assert_eq!(guess("Video Games", "Lana Del Rey"), None);
    }

    #[test]
    fn noise_segments() {
        assert!(is_noise("OFFICIAL MUSIC VIDEO"));
        assert!(is_noise("Remastered 2011"));
        assert!(is_noise("M/V"));
        assert!(!is_noise("Video Games"));
        assert!(!is_noise("CHÚNG TA CỦA HIỆN TẠI"));
    }

    #[test]
    fn channel_names_become_artist_names() {
        assert_eq!(clean_artist("Sơn Tùng M-TP Official"), "Sơn Tùng M-TP");
        assert_eq!(clean_artist("Rick Astley - Topic"), "Rick Astley");
        assert_eq!(clean_artist("TaylorSwiftVEVO"), "TaylorSwift");
        assert_eq!(clean_artist("Official"), "Official"); // không xóa sạch
        assert_eq!(clean_artist("LXNGVX, Warriyo"), "LXNGVX, Warriyo");
    }

    #[test]
    fn search_queries_for_youtube_and_spotify() {
        let youtube = Track {
            title: "SƠN TÙNG M-TP | CHÚNG TA CỦA HIỆN TẠI | OFFICIAL MUSIC VIDEO".into(),
            artist: "Sơn Tùng M-TP Official".into(),
            album: String::new(),
            duration_s: 890,
        };
        let q = search_queries(&youtube);
        assert!(q.contains(&"CHÚNG TA CỦA HIỆN TẠI SƠN TÙNG M-TP".to_string()));
        assert!(q.contains(&"CHÚNG TA CỦA HIỆN TẠI".to_string()));

        let spotify = Track {
            title: "Mortals Funk Remix".into(),
            artist: "Warriyo, LXNGVX, Laura Brehm".into(),
            album: "Mortals Funk Remix".into(),
            duration_s: 146,
        };
        // Tên bài sạch sẵn: chỉ một câu tìm kiếm, không gửi trùng
        assert_eq!(search_queries(&spotify), ["Mortals Funk Remix Warriyo"]);
    }

    #[test]
    fn urls_are_encoded() {
        let t = Track {
            title: "Nơi này có anh".into(),
            artist: "Sơn Tùng M-TP".into(),
            album: "".into(),
            duration_s: 260,
        };
        assert!(get_url(&t).starts_with("https://lrclib.net/api/get?track_name=N%C6%A1i%20n"));
        assert!(get_url(&t).ends_with("&album_name=&duration=260"));
        assert_eq!(search_url("a b"), "https://lrclib.net/api/search?q=a%20b");
    }

    #[test]
    fn pick_best_prefers_synced_and_close_duration() {
        let records = vec![
            record(134.0, Some("[00:01.00] a"), None),
            record(146.0, None, Some("plain")),
            record(146.5, Some("[00:01.00] b"), None),
            record(145.0, Some("[00:01.00] c"), None),
        ];
        // Bài dài 146 giây: bản 134 giây bị loại; trong bản có lời đồng bộ, chọn bản lệch ít nhất
        let found = pick_best(records, 146);
        assert_eq!(
            found,
            Lookup::Lyrics {
                synced: Some("[00:01.00] b".into()),
                plain: None,
            }
        );
    }

    #[test]
    fn pick_best_rejects_far_durations_and_accepts_any_when_unknown() {
        let records = || vec![record(179.0, Some("[00:01.00] x"), None)];
        assert_eq!(pick_best(records(), 146), Lookup::NotFound);
        assert_ne!(pick_best(records(), 0), Lookup::NotFound);
        assert_eq!(pick_best(vec![], 0), Lookup::NotFound);
    }
}
