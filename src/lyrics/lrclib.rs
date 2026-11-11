//! Tra lời bài hát trên LRCLIB (https://lrclib.net), cơ sở dữ liệu lời miễn phí, mã nguồn mở.
//!
//! Gửi đi: tên bài, nghệ sĩ, album và thời lượng của bài đang phát.
//! Cách tìm:
//! 1. `/api/get`: khớp chính xác cả bốn thông tin (nhanh, đúng nhất).
//! 2. `/api/search?q=tên bài + nghệ sĩ đầu tiên`: vì mỗi ứng dụng ghi nghệ sĩ một kiểu
//!    ("Warriyo, LXNGVX, Laura Brehm" so với "LXNGVX, Warriyo").
//! 3. Như bước 2 nhưng tên bài đã bỏ phần phụ ("(feat. X)", " - Remastered").
//! 4. Tên kiểu video YouTube ("SƠN TÙNG M-TP | CHÚNG TA CỦA HIỆN TẠI | OFFICIAL MUSIC VIDEO",
//!    kênh "Sơn Tùng M-TP Official"): tách ra tên bài và nghệ sĩ, tìm theo tên đoán được trước.
//!
//! Khi tìm, chỉ nhận bản cùng tên bài và lệch thời lượng không quá 3 giây để lời khớp nhịp.
//! Bản chỉ có lời thường được giữ làm dự phòng, vẫn tìm tiếp bản có lời đồng bộ.

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
        /// Bản này dài khác bài đang phát (vd. video MV có đoạn mở đầu): lời có thể lệch
        approximate: bool,
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
    track_name: String,
    #[serde(default)]
    artist_name: String,
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

impl Lookup {
    /// Có lời đồng bộ (chạy theo nhạc): kết quả tốt nhất, dừng tìm được
    fn is_synced(&self) -> bool {
        matches!(
            self,
            Lookup::Lyrics {
                synced: Some(_),
                ..
            }
        )
    }
}

impl Record {
    fn into_found(self) -> Lookup {
        let synced = non_empty(self.synced_lyrics);
        let plain = non_empty(self.plain_lyrics);
        if synced.is_some() || plain.is_some() {
            Lookup::Lyrics {
                synced,
                plain,
                approximate: false,
            }
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

/// Từ mở đầu phần ca sĩ góp giọng: "Có Ai Thương Em Như Anh ft. Touliver"
const FEATURING: &[&str] = &["ft", "ft.", "feat", "feat.", "featuring"];

/// Bỏ phần phụ của tên bài: "Song (feat. X) - Remastered 2011" → "Song",
/// "Song #CATENA ft. Y" → "Song"
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
    let mut words: Vec<&str> = out
        .split_whitespace()
        .filter(|w| !w.starts_with('#'))
        .collect();
    // Cắt từ "ft./feat." trở đi (không cắt nếu nó đứng đầu, kẻo tên rỗng)
    if let Some(i) = words
        .iter()
        .position(|w| FEATURING.contains(&w.to_lowercase().as_str()))
        .filter(|&i| i > 0)
    {
        words.truncate(i);
    }
    let cleaned = words.join(" ");
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

/// Tên bài và nghệ sĩ đoán từ tên video
#[derive(Debug, Clone, PartialEq, Eq)]
struct Guess {
    title: String,
    artist: String,
    /// Có một đoạn trùng tên kênh nên biết chắc đâu là nghệ sĩ. Nếu không, chỉ đoán theo
    /// kiểu phổ biến "Nghệ sĩ - Tên bài" và có thể bị ngược
    sure: bool,
}

/// Đoạn này là tên nghệ sĩ của kênh: "MIN" với kênh "MIN", hoặc nằm trong tên kênh fan
/// ("Sơn Tùng M-TP" trong "We Love Sơn Tùng M-TP")
fn is_channel_artist(part: &str, channel: &str) -> bool {
    let (part, channel) = (name_key(part), name_key(channel));
    !part.is_empty() && (part == channel || (part.chars().count() >= 3 && channel.contains(&part)))
}

/// Đoán (tên bài, nghệ sĩ) từ tên video. None nếu tên trông đã là tên bài bình thường
fn guess_from_video_title(title: &str, artist: &str) -> Option<Guess> {
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
    // Đoạn trùng tên nghệ sĩ thì bỏ, đoạn còn lại là tên bài
    if let Some(i) = parts.iter().position(|p| is_channel_artist(p, &channel)) {
        let title = parts
            .iter()
            .enumerate()
            .find(|&(j, _)| j != i)
            .map(|(_, p)| p.clone())?;
        return Some(Guess {
            title,
            artist: parts[i].clone(),
            sure: true,
        });
    }
    match parts.as_slice() {
        // Kiểu phổ biến nhất: "Nghệ sĩ - Tên bài"
        [first, second, ..] => Some(Guess {
            title: second.clone(),
            artist: first.clone(),
            sure: false,
        }),
        [only] => Some(Guess {
            title: only.clone(),
            artist: channel,
            sure: true,
        }),
        [] => None,
    }
}

impl Guess {
    /// Các tên bài có thể đúng: chưa chắc thứ tự thì cả đoạn kia cũng có thể là tên bài
    fn titles(&self) -> Vec<String> {
        let mut v = vec![self.title.clone()];
        if !self.sure {
            v.push(self.artist.clone());
        }
        v
    }

    /// Các tên nghệ sĩ có thể đúng (ngược lại với `titles`)
    fn artists(&self) -> Vec<String> {
        let mut v = vec![self.artist.clone()];
        if !self.sure {
            v.push(self.title.clone());
        }
        v
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

/// Chữ cái tiếng Việt về chữ không dấu ("ơ" → "o", "đ" → "d"), để LRCLIB ghi
/// "Son Tung M-TP" vẫn khớp "Sơn Tùng M-TP"
fn base_letter(c: char) -> char {
    const GROUPS: &[(&str, char)] = &[
        ("àáảãạăằắẳẵặâầấẩẫậ", 'a'),
        ("èéẻẽẹêềếểễệ", 'e'),
        ("ìíỉĩị", 'i'),
        ("òóỏõọôồốổỗộơờớởỡợ", 'o'),
        ("ùúủũụưừứửữự", 'u'),
        ("ỳýỷỹỵ", 'y'),
        ("đ", 'd'),
    ];
    GROUPS
        .iter()
        .find(|(set, _)| set.contains(c))
        .map_or(c, |&(_, base)| base)
}

/// Chuỗi để so tên: chữ thường, bỏ dấu, chỉ giữ chữ và số ("Sơn Tùng M-TP" → "sontungmtp")
fn name_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .map(base_letter)
        .collect()
}

/// Các tên bài được chấp nhận, đã đổi sang dạng để so (cả bản gốc lẫn bản bỏ phần phụ)
struct WantedTitles(Vec<String>);

impl WantedTitles {
    fn new<S: AsRef<str>>(titles: &[S]) -> Self {
        let mut keys = Vec::new();
        for t in titles {
            for key in [name_key(t.as_ref()), name_key(&clean_title(t.as_ref()))] {
                if !key.is_empty() && !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
        WantedTitles(keys)
    }

    /// Bản ghi có đúng là bài này: "Em Mới Là Người Yêu Anh" khớp, "Stuck (Nhớ)" thì không.
    /// Bỏ phần phụ ("(feat. X)") vẫn khớp, trừ khi đó là bản khác ("(Remix)", "(Live)")
    fn matches(&self, track_name: &str) -> bool {
        let raw = name_key(track_name);
        if self.0.contains(&raw) {
            return true;
        }
        !is_other_version(track_name) && self.0.contains(&name_key(&clean_title(track_name)))
    }
}

/// Bản phối, bản thu khác của bài: lời chạy khác nhịp bản gốc
const VERSION_WORDS: &[&str] = &[
    "remix",
    "live",
    "acoustic",
    "cover",
    "karaoke",
    "beat",
    "instrumental",
    "lofi",
    "slowed",
    "sped",
    "speed",
    "edm",
    "version",
];

fn is_other_version(track_name: &str) -> bool {
    track_name
        .split(|c: char| !c.is_alphanumeric())
        .any(|w| VERSION_WORDS.contains(&w.to_lowercase().as_str()))
}

/// Chọn bản tốt nhất trong kết quả tìm kiếm: đúng tên bài, thời lượng gần đúng
fn pick_best(records: &[Record], duration_s: u32, titles: &WantedTitles) -> Lookup {
    let diff = |r: &Record| (r.duration - f64::from(duration_s)).abs();
    records
        .iter()
        .filter(|r| titles.matches(&r.track_name))
        // Không biết thời lượng thì nhận mọi bản; biết thì chỉ nhận bản gần đúng
        .filter(|r| duration_s == 0 || diff(r) <= MAX_DURATION_DIFF_S)
        .min_by(|a, b| a.rank().cmp(&b.rank()).then(diff(a).total_cmp(&diff(b))))
        .map_or(Lookup::NotFound, |r| r.clone().into_found())
}

/// Video ca nhạc (MV) thường dài hơn bài hát vì có đoạn mở đầu, kết thúc, cảnh phim,
/// nên không khớp được theo thời lượng. Khi tên bài đoán từ tên video, nhận bản có
/// cùng tên bài, cùng ca sĩ, và không dài hơn video; đánh dấu là lời có thể lệch
fn pick_for_video<S: AsRef<str>>(
    records: &[Record],
    titles: &WantedTitles,
    artists: &[S],
    video_s: u32,
) -> Lookup {
    let artists: Vec<String> = artists
        .iter()
        .map(|a| name_key(a.as_ref()))
        .filter(|a| !a.is_empty())
        .collect();
    let video = f64::from(video_s);
    // "Tóc Tiên ft. Touliver" vẫn là bài của "Tóc Tiên"
    let same_artist = |r: &Record| {
        let a = name_key(&r.artist_name);
        !a.is_empty()
            && artists
                .iter()
                .any(|want| a.contains(want.as_str()) || want.contains(&a))
    };
    let best = records
        .iter()
        .filter(|r| titles.matches(&r.track_name) && same_artist(r))
        .filter(|r| video_s == 0 || r.duration <= video + MAX_DURATION_DIFF_S)
        // Bản đồng bộ trước, rồi bản dài nhất (gần với video nhất)
        .min_by(|a, b| {
            a.rank()
                .cmp(&b.rank())
                .then(b.duration.total_cmp(&a.duration))
        });
    match best.map(|r| r.clone().into_found()) {
        Some(Lookup::Lyrics { synced, plain, .. }) => Lookup::Lyrics {
            synced,
            plain,
            approximate: true,
        },
        other => other.unwrap_or(Lookup::NotFound),
    }
}

/// Các câu tìm kiếm, theo thứ tự thử, không trùng nhau
fn search_queries(t: &Track) -> Vec<String> {
    let artist = first_artist(&t.artist);
    // Tên kênh YouTube có đuôi riêng ("Hari Won - Topic", "Sơn Tùng M-TP Official"),
    // LRCLIB thì ghi tên nghệ sĩ trơn: thử tên đã bỏ đuôi trước, rồi tới tên gốc
    let channel = clean_artist(artist);
    let plain = [
        // 2) Tên bài + nghệ sĩ đầu tiên
        format!("{} {channel}", t.title.trim()),
        // 3) Tên bài đã bỏ phần phụ
        format!("{} {channel}", clean_title(&t.title)),
        format!("{} {artist}", t.title.trim()),
        format!("{} {artist}", clean_title(&t.title)),
    ];
    let guess = guess_from_video_title(&t.title, &t.artist);
    let mut queries = Vec::new();
    match &guess {
        // 4) Tên kiểu video YouTube: tên bài + nghệ sĩ đoán được trước tiên. Biết chắc
        //    nghệ sĩ thì tên video gốc không cần tìm nữa (chỉ toàn chữ thừa)
        Some(g) => {
            queries.push(format!("{} {}", g.title, g.artist));
            if !g.sure {
                queries.extend(plain);
            }
            // Tên bài một mình, phòng khi tên kênh khác hẳn tên nghệ sĩ trên LRCLIB
            queries.push(g.title.clone());
        }
        None => queries.extend(plain),
    }

    // Câu chỉ gồm tên nghệ sĩ ("MIN MIN", "Tóc Tiên Tóc Tiên") ra bài bất kỳ của
    // nghệ sĩ đó: bỏ
    let mut artist_words: Vec<String> = Vec::new();
    let mut names = vec![t.artist.as_str(), artist, channel.as_str()];
    // Nghệ sĩ đoán chưa chắc thì có khi lại là tên bài: không tính
    if let Some(g) = guess.as_ref().filter(|g| g.sure) {
        names.push(&g.artist);
    }
    for name in names {
        artist_words.extend(name.split_whitespace().map(name_key));
    }
    let only_artist = |q: &str| {
        q.split_whitespace()
            .map(name_key)
            .filter(|w| !w.is_empty())
            .all(|w| artist_words.contains(&w))
    };

    let mut seen = Vec::new();
    queries.retain(|q| {
        let key = q.to_lowercase();
        let keep = !seen.contains(&key) && !only_artist(q);
        seen.push(key);
        keep
    });
    queries
}

fn parse_json<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, String> {
    serde_json::from_str(body).map_err(|e| e.to_string())
}

/// Tra lời cho một bài. Lỗi mạng trả về Err (không được lưu vào cache).
/// Gặp lời đồng bộ thì dừng; bản chỉ có lời thường (hoặc ghi là nhạc không lời) được giữ
/// làm dự phòng, chỉ dùng khi tìm hết mà không có bản đồng bộ nào
pub fn fetch(session: &Session, t: &Track) -> Result<Lookup, String> {
    let mut fallback = Lookup::NotFound;
    let mut consider = |found: Lookup| -> Option<Lookup> {
        if found.is_synced() {
            return Some(found);
        }
        if fallback == Lookup::NotFound {
            fallback = found;
        }
        None
    };

    // 1) Khớp chính xác (cần thời lượng)
    if t.duration_s > 0 {
        let response = session.get(&get_url(t))?;
        match response.status {
            200 => {
                let found = parse_json::<Record>(&response.body)?.into_found();
                if let Some(found) = consider(found) {
                    return Ok(found);
                }
            }
            404 => {} // không khớp chính xác: chuyển sang tìm kiếm
            status => return Err(format!("LRCLIB trả về HTTP {status}")),
        }
    }

    // 2) Tìm kiếm, nhận bản cùng tên bài và có thời lượng gần đúng
    let guess = guess_from_video_title(&t.title, &t.artist);
    let mut titles = vec![t.title.clone()];
    if let Some(g) = &guess {
        titles.extend(g.titles());
    }
    let titles = WantedTitles::new(&titles);
    let mut seen: Vec<Record> = Vec::new();
    for query in search_queries(t) {
        let body = session.get(&search_url(&query))?.ok()?;
        let records: Vec<Record> = parse_json(&body)?;
        if let Some(found) = consider(pick_best(&records, t.duration_s, &titles)) {
            return Ok(found);
        }
        seen.extend(records);
    }

    // 3) Video ca nhạc: thời lượng không khớp nhưng đúng tên bài và ca sĩ
    if let Some(g) = &guess {
        let found = pick_for_video(
            &seen,
            &WantedTitles::new(&g.titles()),
            &g.artists(),
            t.duration_s,
        );
        if let Some(found) = consider(found) {
            return Ok(found);
        }
    }
    Ok(fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(duration: f64, synced: Option<&str>, plain: Option<&str>) -> Record {
        Record {
            track_name: "Song".into(),
            artist_name: String::new(),
            duration,
            instrumental: false,
            synced_lyrics: synced.map(String::from),
            plain_lyrics: plain.map(String::from),
        }
    }

    /// Gọi LRCLIB thật nên mặc định bỏ qua; chạy bằng `cargo test -- --ignored`
    /// MV trên YouTube dài hơn bài hát: vẫn phải tìm ra lời nhờ khớp tên bài và ca sĩ
    #[test]
    #[ignore]
    fn live_fetch_finds_lyrics_for_music_video() {
        let session = Session::open().unwrap();
        let t = Track {
            title: "Buông Đôi Tay Nhau Ra | OFFICIAL MUSIC VIDEO | Sơn Tùng M-TP".into(),
            artist: "Sơn Tùng M-TP Official".into(),
            album: String::new(),
            duration_s: 289,
        };
        match fetch(&session, &t).unwrap() {
            Lookup::Lyrics {
                synced,
                approximate,
                ..
            } => {
                assert!(
                    approximate,
                    "bản lời ngắn hơn video phải được đánh dấu có thể lệch"
                );
                let synced = synced.expect("có lời đồng bộ");
                println!("{}", synced.lines().take(3).collect::<Vec<_>>().join("\n"));
            }
            other => panic!("không tìm thấy lời: {other:?}"),
        }
    }

    /// Kênh "Topic" của YouTube Music: phải bỏ đuôi " - Topic" mới tìm ra
    #[test]
    #[ignore]
    fn live_fetch_finds_lyrics_for_topic_channel() {
        let session = Session::open().unwrap();
        let t = Track {
            title: "Hương Đêm Bay Xa".into(),
            artist: "Hari Won - Topic".into(),
            album: String::new(),
            duration_s: 222,
        };
        match fetch(&session, &t).unwrap() {
            Lookup::Lyrics {
                synced,
                approximate,
                ..
            } => {
                assert!(
                    !approximate,
                    "đúng thời lượng thì không phải lời có thể lệch"
                );
                let synced = synced.expect("có lời đồng bộ");
                println!("{}", synced.lines().take(3).collect::<Vec<_>>().join("\n"));
            }
            other => panic!("không tìm thấy lời: {other:?}"),
        }
    }

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
                approximate: false,
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
        guess_from_video_title(title, artist).map(|g| (g.title, g.artist))
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
    fn topic_channel_suffix_is_dropped_from_artist() {
        // Kênh "Topic" của YouTube Music: tên bài gọn sẵn, chỉ tên kênh có đuôi
        let topic = Track {
            title: "Hương Đêm Bay Xa".into(),
            artist: "Hari Won - Topic".into(),
            album: String::new(),
            duration_s: 222,
        };
        assert_eq!(
            search_queries(&topic),
            [
                "Hương Đêm Bay Xa Hari Won",
                "Hương Đêm Bay Xa Hari Won - Topic"
            ]
        );
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
        let found = pick_best(&records, 146, &WantedTitles::new(&["Song"]));
        assert_eq!(
            found,
            Lookup::Lyrics {
                synced: Some("[00:01.00] b".into()),
                plain: None,
                approximate: false,
            }
        );
    }

    fn named(track: &str, artist: &str, duration: f64, synced: Option<&str>) -> Record {
        Record {
            track_name: track.into(),
            artist_name: artist.into(),
            ..record(duration, synced, None)
        }
    }

    #[test]
    fn music_video_matches_by_name_when_duration_differs() {
        // MV dài 289 giây, bài hát trên LRCLIB chỉ 226–234 giây
        let records = vec![
            named(
                "Như Ngày Hôm Qua",
                "Sơn Tùng M-TP",
                221.0,
                Some("[00:01.00] khác"),
            ),
            named("Buông Đôi Tay Nhau Ra", "Sơn Tùng M-TP", 226.0, None),
            named(
                "Buông Đôi Tay Nhau Ra",
                "Sơn Tùng M-TP Official",
                228.0,
                Some("[00:01.00] a"),
            ),
            named(
                "Buông Đôi Tay Nhau Ra",
                "Sơn Tùng M-TP",
                234.0,
                Some("[00:01.00] b"),
            ),
        ];
        let titles = WantedTitles::new(&["Buông Đôi Tay Nhau Ra"]);
        assert_eq!(pick_best(&records, 289, &titles), Lookup::NotFound);
        let found = pick_for_video(&records, &titles, &["Sơn Tùng M-TP"], 289);
        // Bản đồng bộ, dài nhất (gần video nhất), đánh dấu có thể lệch
        assert_eq!(
            found,
            Lookup::Lyrics {
                synced: Some("[00:01.00] b".into()),
                plain: None,
                approximate: true,
            }
        );
    }

    #[test]
    fn music_video_match_needs_same_title_artist_and_shorter_song() {
        let r = |track: &str, artist: &str, duration: f64| {
            vec![named(track, artist, duration, Some("[00:01.00] x"))]
        };
        let pick = |records: &[Record]| {
            pick_for_video(
                records,
                &WantedTitles::new(&["Lạc Trôi"]),
                &["Sơn Tùng M-TP"],
                280,
            )
        };
        assert_ne!(
            pick(&r("LẠC TRÔI", "Sơn Tùng M-TP", 233.0)),
            Lookup::NotFound
        );
        assert_eq!(
            pick(&r("Lạc Trôi (Remix)", "Sơn Tùng M-TP", 233.0)),
            Lookup::NotFound
        );
        assert_eq!(pick(&r("Lạc Trôi", "Người khác", 233.0)), Lookup::NotFound);
        // Bài dài hơn cả video thì không phải bản của video này
        assert_eq!(
            pick(&r("Lạc Trôi", "Sơn Tùng M-TP", 400.0)),
            Lookup::NotFound
        );
    }

    #[test]
    fn pick_best_rejects_far_durations_and_accepts_any_when_unknown() {
        let records = vec![record(179.0, Some("[00:01.00] x"), None)];
        let titles = WantedTitles::new(&["Song"]);
        assert_eq!(pick_best(&records, 146, &titles), Lookup::NotFound);
        assert_ne!(pick_best(&records, 0, &titles), Lookup::NotFound);
        assert_eq!(pick_best(&[], 0, &titles), Lookup::NotFound);
    }

    #[test]
    fn pick_best_rejects_other_songs_of_same_length() {
        // Câu tìm "MIN" ra bài khác của MIN dài gần bằng video: không được nhận
        let records = vec![
            named("MIN - Stuck (Nhớ)", "MIN", 248.9, None),
            named(
                "Em Mới Là Người Yêu Anh",
                "MIN",
                250.0,
                Some("[00:01.00] a"),
            ),
        ];
        let titles = WantedTitles::new(&["Em Mới Là Người Yêu Anh"]);
        assert!(pick_best(&records[..1], 250, &titles) == Lookup::NotFound);
        assert!(pick_best(&records, 250, &titles).is_synced());
    }

    #[test]
    fn titles_match_without_accents_and_extras() {
        let titles = WantedTitles::new(&["Khuôn Mặt Đáng Thương"]);
        assert!(titles.matches("Khuon Mat Dang Thuong"));
        assert!(titles.matches("KHUÔN MẶT ĐÁNG THƯƠNG (feat. X)"));
        assert!(!titles.matches("Khuôn Mặt Đáng Thương (Remix)"));
        assert!(!titles.matches("Anh Sai Rồi"));
        assert_eq!(name_key("Sơn Tùng M-TP"), "sontungmtp");
    }

    #[test]
    fn clean_title_drops_featuring_and_hashtags() {
        assert_eq!(
            clean_title("CÓ AI THƯƠNG EM NHƯ ANH (#CATENA) ft. Touliver (Official MV)"),
            "CÓ AI THƯƠNG EM NHƯ ANH"
        );
        assert_eq!(clean_title("Song #shorts feat. X"), "Song");
        assert_eq!(clean_title("Feat. Song"), "Feat. Song"); // không để tên rỗng
    }

    #[test]
    fn guesses_for_the_three_reported_videos() {
        // Tên kênh trùng nghệ sĩ, phần "ft." và hashtag bị bỏ
        assert_eq!(
            guess(
                "Tóc Tiên - CÓ AI THƯƠNG EM NHƯ ANH (#CATENA) ft. Touliver (Official MV)",
                "Tóc Tiên"
            ),
            pair("CÓ AI THƯƠNG EM NHƯ ANH", "Tóc Tiên")
        );
        // Kênh fan có chứa tên nghệ sĩ: biết đoạn nào là nghệ sĩ dù đứng sau
        assert_eq!(
            guess(
                "Khuôn Mặt Đáng Thương - Sơn Tùng M-TP [ Kara Lyric Effect ]",
                "We Love Sơn Tùng M-TP Channel"
            ),
            pair("Khuôn Mặt Đáng Thương", "Sơn Tùng M-TP")
        );
        assert_eq!(
            guess(
                "MIN - Em Mới Là Người Yêu Anh | Official Music Video",
                "MIN OFFICIAL"
            ),
            pair("Em Mới Là Người Yêu Anh", "MIN")
        );
    }

    #[test]
    fn no_artist_only_queries() {
        let t = Track {
            title: "MIN - Em Mới Là Người Yêu Anh | Official Music Video".into(),
            artist: "MIN OFFICIAL".into(),
            album: String::new(),
            duration_s: 250,
        };
        assert_eq!(
            search_queries(&t),
            ["Em Mới Là Người Yêu Anh MIN", "Em Mới Là Người Yêu Anh"]
        );
        let t = Track {
            title: "Tóc Tiên - CÓ AI THƯƠNG EM NHƯ ANH (#CATENA) ft. Touliver (Official MV)".into(),
            artist: "Tóc Tiên".into(),
            album: String::new(),
            duration_s: 269,
        };
        assert_eq!(
            search_queries(&t),
            [
                "CÓ AI THƯƠNG EM NHƯ ANH Tóc Tiên",
                "CÓ AI THƯƠNG EM NHƯ ANH"
            ]
        );
    }

    #[test]
    fn unsure_guess_keeps_original_queries_and_both_orders() {
        // Kênh lạ: chưa biết đoạn nào là nghệ sĩ nên thử cả tên gốc và chấp nhận cả hai thứ tự
        let g = guess_from_video_title("Hello - From the movie Sing", "Some Uploader").unwrap();
        assert!(!g.sure);
        assert!(WantedTitles::new(&g.titles()).matches("Hello"));
        let t = Track {
            title: "Hello - From the movie Sing".into(),
            artist: "Some Uploader".into(),
            album: String::new(),
            duration_s: 200,
        };
        assert!(search_queries(&t).contains(&"Hello Some Uploader".to_string()));
    }

    /// Ba video người dùng báo không ra lời (gọi LRCLIB thật)
    #[test]
    #[ignore]
    fn live_fetch_three_reported_videos() {
        let session = Session::open().unwrap();
        let cases = [
            (
                "Tóc Tiên - CÓ AI THƯƠNG EM NHƯ ANH (#CATENA) ft. Touliver (Official MV)",
                "Tóc Tiên",
                269,
                true,
            ),
            (
                "Khuôn Mặt Đáng Thương - Sơn Tùng M-TP [ Kara Lyric Effect ]",
                "We Love Sơn Tùng M-TP Channel",
                255,
                false,
            ),
            (
                "MIN - Em Mới Là Người Yêu Anh | Official Music Video",
                "MIN OFFICIAL",
                250,
                false,
            ),
        ];
        for (title, artist, duration_s, want_approximate) in cases {
            let t = Track {
                title: title.into(),
                artist: artist.into(),
                album: String::new(),
                duration_s,
            };
            match fetch(&session, &t).unwrap() {
                Lookup::Lyrics {
                    synced: Some(synced),
                    approximate,
                    ..
                } => {
                    println!(
                        "{title}\n  lệch={approximate}  {}",
                        synced.lines().find(|l| l.len() > 12).unwrap_or("")
                    );
                    assert_eq!(approximate, want_approximate, "{title}");
                }
                other => panic!("{title}: không có lời đồng bộ: {other:?}"),
            }
        }
    }
}
