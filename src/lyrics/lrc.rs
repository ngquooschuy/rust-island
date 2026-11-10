//! Đọc lời bài hát dạng LRC ("[01:23.45] lời") và tìm dòng đang hát.

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Mốc thời gian bắt đầu dòng, tính từ đầu bài (mili giây)
    pub time_ms: u64,
    pub text: String,
}

/// Lời đồng bộ, các dòng đã sắp theo thời gian
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lyrics {
    pub lines: Vec<Line>,
}

/// Ba dòng quanh vị trí đang phát: dòng trước, dòng đang hát, dòng sau
#[derive(Debug, Clone, PartialEq)]
pub struct Window<'a> {
    pub prev: Option<&'a str>,
    pub current: &'a str,
    pub next: Option<&'a str>,
}

/// Dòng trống trong LRC là đoạn nhạc dạo, hiện bằng nốt nhạc
const GAP: &str = "♪";

/// "01:23.45" → 83450 ms. Nhận cả "01:23", "01:23.4", "01:23.456", "01:23:45"
fn parse_timestamp(tag: &str) -> Option<u64> {
    let (min, rest) = tag.split_once(':')?;
    let (sec, frac) = rest.split_once(['.', ':']).unwrap_or((rest, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(min) || !digits(sec) || !(frac.is_empty() || digits(frac)) {
        return None; // thẻ thông tin như [ar:Ca sĩ], [offset:200]
    }
    let min: u64 = min.parse().ok()?;
    let sec: u64 = sec.parse().ok()?;
    // Phần lẻ: "4" = 400 ms, "45" = 450 ms, "456" = 456 ms
    let frac_ms = match frac.len() {
        0 => 0,
        1 => frac.parse::<u64>().ok()? * 100,
        2 => frac.parse::<u64>().ok()? * 10,
        _ => frac[..3].parse::<u64>().ok()?,
    };
    Some((min * 60 + sec) * 1000 + frac_ms)
}

/// Đọc toàn bộ file LRC. Dòng không có mốc thời gian (thẻ thông tin) bị bỏ qua
pub fn parse(text: &str) -> Lyrics {
    let mut offset_ms: i64 = 0;
    let mut raw: Vec<(u64, String)> = Vec::new();

    for line in text.lines() {
        let mut rest = line.trim();
        let mut times = Vec::new();
        // Một dòng có thể có nhiều mốc: "[00:12.00][01:30.00] Điệp khúc"
        while let Some(inner) = rest.strip_prefix('[') {
            let Some(end) = inner.find(']') else { break };
            let tag = &inner[..end];
            if let Some(t) = parse_timestamp(tag) {
                times.push(t);
            } else if let Some(v) = tag.strip_prefix("offset:") {
                offset_ms = v.trim().parse().unwrap_or(0);
            }
            rest = &inner[end + 1..];
        }
        let words = rest.trim();
        raw.extend(times.into_iter().map(|t| (t, words.to_string())));
    }

    // [offset:+500] nghĩa là lời hiện sớm hơn 500 ms
    let mut lines: Vec<Line> = raw
        .into_iter()
        .map(|(t, text)| Line {
            time_ms: (t as i64 - offset_ms).max(0) as u64,
            text,
        })
        .collect();
    lines.sort_by_key(|l| l.time_ms); // sort ổn định: giữ thứ tự gốc khi trùng mốc
    Lyrics { lines }
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Chỉ số dòng đang hát lúc `pos_ms`: dòng cuối cùng đã tới mốc. None nếu chưa tới dòng đầu
    pub fn index_at(&self, pos_ms: u64) -> Option<usize> {
        // Các dòng đã sắp xếp nên tìm nhị phân được
        self.lines
            .partition_point(|l| l.time_ms <= pos_ms)
            .checked_sub(1)
    }

    fn text(&self, i: usize) -> Option<&str> {
        self.lines.get(i).map(|l| match l.text.as_str() {
            "" => GAP,
            t => t,
        })
    }

    /// Dòng trước / đang hát / sau tại `pos_ms` để vẽ lên island
    pub fn window_at(&self, pos_ms: u64) -> Window<'_> {
        match self.index_at(pos_ms) {
            // Đang nhạc dạo trước dòng đầu tiên
            None => Window {
                prev: None,
                current: GAP,
                next: self.text(0),
            },
            Some(i) => Window {
                prev: i.checked_sub(1).and_then(|p| self.text(p)),
                current: self.text(i).unwrap_or(GAP),
                next: self.text(i + 1),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SONG: &str = "[ar:LXNGVX, Warriyo]\n\
        [ti:Mortals Funk Remix]\n\
        [00:01.95] Stranded in the open\n\
        [00:06.20] Dried out tears of sorrow\n\
        [00:10.50]\n\
        [00:14.56] Staring down the barrel\n";

    #[test]
    fn timestamps_in_all_common_forms() {
        assert_eq!(parse_timestamp("01:23.45"), Some(83_450));
        assert_eq!(parse_timestamp("01:23"), Some(83_000));
        assert_eq!(parse_timestamp("00:01.5"), Some(1_500));
        assert_eq!(parse_timestamp("00:01.234"), Some(1_234));
        assert_eq!(parse_timestamp("00:01:23"), Some(1_230));
        assert_eq!(parse_timestamp("ar:Someone"), None);
        assert_eq!(parse_timestamp("offset:200"), None);
    }

    #[test]
    fn parses_lines_and_skips_tags() {
        let l = parse(SONG);
        assert_eq!(l.lines.len(), 4);
        assert_eq!(l.lines[0].time_ms, 1_950);
        assert_eq!(l.lines[0].text, "Stranded in the open");
        assert_eq!(l.lines[2].text, ""); // dòng trống được giữ làm đoạn nhạc dạo
    }

    #[test]
    fn repeated_timestamps_become_separate_lines() {
        let l = parse("[00:30.00][00:10.00] Chorus\n[00:20.00] Verse");
        let times: Vec<_> = l.lines.iter().map(|x| x.time_ms).collect();
        assert_eq!(times, [10_000, 20_000, 30_000]);
        assert_eq!(l.lines[2].text, "Chorus");
    }

    #[test]
    fn positive_offset_shows_lines_earlier() {
        let l = parse("[offset:+500]\n[00:02.00] Hi\n[00:00.20] Intro");
        assert_eq!(l.lines[0].time_ms, 0); // không âm
        assert_eq!(l.lines[1].time_ms, 1_500);
    }

    #[test]
    fn index_at_picks_last_started_line() {
        let l = parse(SONG);
        assert_eq!(l.index_at(0), None);
        assert_eq!(l.index_at(1_949), None);
        assert_eq!(l.index_at(1_950), Some(0));
        assert_eq!(l.index_at(6_199), Some(0));
        assert_eq!(l.index_at(6_200), Some(1));
        assert_eq!(l.index_at(999_999), Some(3));
    }

    #[test]
    fn window_shows_neighbours_and_gaps() {
        let l = parse(SONG);
        let w = l.window_at(7_000);
        assert_eq!(w.prev, Some("Stranded in the open"));
        assert_eq!(w.current, "Dried out tears of sorrow");
        assert_eq!(w.next, Some("♪")); // dòng trống
        let intro = l.window_at(0);
        assert_eq!((intro.prev, intro.current), (None, "♪"));
        assert_eq!(intro.next, Some("Stranded in the open"));
        let end = l.window_at(999_999);
        assert_eq!(end.next, None);
    }

    #[test]
    fn plain_text_without_timestamps_is_empty() {
        assert!(parse("Just some words\nno timing").is_empty());
    }
}
