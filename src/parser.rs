//! 文件名解析模块
//!
//! 该模块负责解析符合特定格式的动漫文件名，并提取关键信息。
#![allow(non_snake_case)]
//!
//! # 支持的文件名格式
//!
//! ```text
//! [发布组] 动漫名称（可含季度） - 集数 [标签信息].扩展名
//! ```
//!
//! # 示例
//!
//! ```
//! use anime_organizer::parser::FilenameParser;
//! use std::path::Path;
//!
//! let path = Path::new("[ANi] 妖怪旅館營業中 貳 - 07 [1080P].mp4");
//! if let Some(info) = FilenameParser::parse(path) {
//!     assert_eq!(info.publisher, "ANi");
//!     assert_eq!(info.anime_name, "妖怪旅館營業中 貳");
//!     assert_eq!(info.episode, "07");
//! }
//! ```
//!
//! # 测试模块
//!
//! 测试位于 `tests/parser/` 目录，按发布组分类组织。

use regex::Regex;
use std::path::Path;
use std::sync::LazyLock;

static ANIME_FILE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[(?P<publisher>[^\]]+)\]").expect("正则表达式编译失败"));
static SEASON_EPISODE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?P<title>.+?)\s*-\s*S(?P<season>\d{1,2})E(?P<episode>\d{1,4})(?P<rest>\s.*)$",
    )
    .expect("季集信息正则表达式编译失败")
});

static SEASON_SUFFIX_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)^(?P<title>.+?)\s+season\s*(?P<num>\d{1,2})$")
            .expect("季信息正则表达式编译失败"),
        Regex::new(r"(?i)^(?P<title>.+?)\s+s(?P<num>\d{1,2})$").expect("季信息正则表达式编译失败"),
        Regex::new(r"(?i)^(?P<title>.+?)\s+(?P<num>\d{1,2})(?:st|nd|rd|th)\s+season$")
            .expect("季信息正则表达式编译失败"),
        Regex::new(
            r"^(?P<title>.+?)\s*第\s*(?P<num>\d{1,2}|[一二三四五六七八九十]+)\s*季(?:\s*[~～].+)?$",
        )
        .expect("季信息正则表达式编译失败"),
        Regex::new(r"^(?P<title>.+?)\s*(?P<num>\d{1,2}|[一二三四五六七八九十]+)期$")
            .expect("季信息正则表达式编译失败"),
        Regex::new(
            r"^(?P<title>.+?)\s+(?P<num>II|III|IV|V|VI|VII|VIII|IX|X|貳|贰|弐|二期|三期|四期)$",
        )
        .expect("季信息正则表达式编译失败"),
        // 仅匹配数字 >= 2 的季号，如 "Anime 2"、"Anime 12"
        Regex::new(r"^(?P<title>.+?)\s+(?P<num>[2-9]\d*)$").expect("季信息正则表达式编译失败"),
    ]
});

/// 动漫文件信息结构体
///
/// 包含从文件名中解析出的所有关键信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnimeFileInfo {
    /// 发布组名称
    pub publisher: String,
    /// 动漫名称
    pub anime_name: String,
    /// 集数（补齐为两位数）
    pub episode: String,
    /// 标签信息（如分辨率、编码格式等）
    pub tags: String,
    /// 文件扩展名（小写）
    pub extension: String,
    /// 原始文件路径
    pub original_path: String,
}

impl AnimeFileInfo {
    /// 生成目标文件名
    ///
    /// 返回格式为 `{episode} {tags}{extension}` 的文件名。
    #[must_use]
    pub fn target_filename(&self) -> String {
        format!("{} {}{}", self.episode, self.tags, self.extension)
    }

    /// 返回系列主标题，不包含可识别的季后缀。
    #[must_use]
    pub fn series_name(&self) -> String {
        split_series_and_season(&self.anime_name).0
    }

    /// 返回从文件名中识别出的季号。
    #[must_use]
    pub fn season_number(&self) -> Option<u32> {
        split_series_and_season(&self.anime_name).1
    }

    /// 返回用于季目录的名称，无法识别时回落到 `Season 1`。
    #[must_use]
    pub fn season_dir_name(&self) -> String {
        format!("Season {}", self.season_number().unwrap_or(1))
    }
}

/// 将带季信息的动画标题拆分为系列名与季号。
#[must_use]
pub fn split_series_and_season(name: &str) -> (String, Option<u32>) {
    let trimmed = name.trim();

    for pattern in SEASON_SUFFIX_PATTERNS.iter() {
        if let Some(caps) = pattern.captures(trimmed) {
            let Some(title) = caps.name("title") else {
                continue;
            };
            let Some(num_match) = caps.name("num") else {
                continue;
            };

            let season = parse_season_number(num_match.as_str());
            if season.is_some() {
                return (title.as_str().trim().to_string(), season);
            }
        }
    }

    (trimmed.to_string(), None)
}

fn parse_season_number(raw: &str) -> Option<u32> {
    let normalized = raw.trim();

    if let Ok(num) = normalized.parse::<u32>() {
        return Some(num);
    }

    match normalized.to_ascii_uppercase().as_str() {
        "II" => return Some(2),
        "III" => return Some(3),
        "IV" => return Some(4),
        "V" => return Some(5),
        "VI" => return Some(6),
        "VII" => return Some(7),
        "VIII" => return Some(8),
        "IX" => return Some(9),
        "X" => return Some(10),
        _ => {}
    }

    match normalized {
        "貳" | "贰" | "弐" | "二期" => Some(2),
        "三期" => Some(3),
        "四期" => Some(4),
        _ => parse_cjk_number(normalized),
    }
}

fn parse_cjk_number(raw: &str) -> Option<u32> {
    let mut total = 0;
    let mut current = 0;

    for ch in raw.chars() {
        match ch {
            '一' => current += 1,
            '二' => current += 2,
            '三' => current += 3,
            '四' => current += 4,
            '五' => current += 5,
            '六' => current += 6,
            '七' => current += 7,
            '八' => current += 8,
            '九' => current += 9,
            '十' => {
                total += if current == 0 { 10 } else { current * 10 };
                current = 0;
            }
            _ => return None,
        }
    }

    Some(total + current)
}

/// 文件名解析器
///
/// 使用正则表达式解析符合特定格式的动漫文件名。
pub struct FilenameParser;

impl FilenameParser {
    /// 解析文件路径，提取动漫文件信息
    #[must_use]
    pub fn parse<P: AsRef<Path>>(file_path: P) -> Option<AnimeFileInfo> {
        let path = file_path.as_ref();
        let filename = path.to_str()?;

        let filename = if filename.starts_with('[') {
            filename.to_string()
        } else if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            name.to_string()
        } else {
            filename.to_string()
        };

        let caps = ANIME_FILE_REGEX.captures(&filename)?;
        let publisher = caps.name("publisher")?.as_str().trim().to_string();
        let publisher_end = caps.get(0)?.end();

        let after_publisher = &filename[publisher_end..];

        let (anime_name, episode, after_episode) = Self::parse_anime_episode(after_publisher)?;

        let (tags, extension) = Self::parse_tags_and_ext(after_episode)?;

        Some(AnimeFileInfo {
            publisher,
            anime_name,
            episode,
            tags,
            extension,
            original_path: path.to_string_lossy().to_string(),
        })
    }

    fn parse_anime_episode(input: &str) -> Option<(String, String, &str)> {
        let input = input.trim_start();
        if let Some(captures) = SEASON_EPISODE_REGEX.captures(input) {
            let title = captures.name("title")?.as_str().trim();
            let season = captures.name("season")?.as_str().parse::<u32>().ok()?;
            let episode = captures.name("episode")?.as_str().parse::<u32>().ok()?;
            let rest = captures.name("rest")?.as_str().trim_start();
            return Some((
                format!("{title} Season {season}"),
                format!("{episode:02}"),
                rest,
            ));
        }
        let bytes = input.as_bytes();

        let mut episode_info: Option<(usize, usize, usize)> = None;

        for i in 0..bytes.len() {
            if bytes[i] == b'-' && i > 0 {
                let before = bytes[i - 1];
                if before == b' ' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    continue;
                }
                if before == b' ' && i + 2 < bytes.len() && bytes[i + 1] == b' ' {
                    let mut num_start = i + 2;
                    while num_start < bytes.len() && bytes[num_start] == b' ' {
                        num_start += 1;
                    }
                    if num_start < bytes.len()
                        && (bytes[num_start].is_ascii_digit() || bytes[num_start] == b'.')
                    {
                        let mut num_end = num_start;
                        while num_end < bytes.len() {
                            if bytes[num_end].is_ascii_digit() {
                                num_end += 1;
                                continue;
                            }

                            if bytes[num_end] == b'.'
                                && num_end + 1 < bytes.len()
                                && bytes[num_end + 1].is_ascii_digit()
                            {
                                num_end += 1;
                                continue;
                            }

                            break;
                        }
                        let mut suffix_end = num_end;
                        if suffix_end + 1 < bytes.len()
                            && matches!(bytes[suffix_end], b'v' | b'V')
                            && bytes[suffix_end + 1].is_ascii_digit()
                        {
                            suffix_end += 2;
                            while suffix_end < bytes.len() && bytes[suffix_end].is_ascii_digit() {
                                suffix_end += 1;
                            }
                        }
                        let after_digits = if suffix_end < bytes.len() {
                            bytes[suffix_end]
                        } else {
                            b' '
                        };

                        if after_digits == b' '
                            || after_digits == b'['
                            || after_digits == b'-'
                            || after_digits == b'.'
                            || after_digits == b'('
                            || suffix_end >= bytes.len()
                        {
                            episode_info = Some((i, num_start, num_end));
                        }
                    }
                }
            }
        }

        if let Some((dash_pos, digit_start, digit_end)) = episode_info {
            let episode_raw = std::str::from_utf8(&bytes[digit_start..digit_end]).ok()?;
            let episode = if episode_raw.contains('.') {
                episode_raw.to_string()
            } else {
                format!("{:0>2}", episode_raw)
            };

            let anime_name = input[..dash_pos].trim().to_string();
            let after_episode = std::str::from_utf8(&bytes[digit_end..]).ok()?.trim_start();

            return Some((anime_name, episode, after_episode));
        }

        for marker in [" - 電影", " - 电影", " - Movie"] {
            if let Some(marker_pos) = input.rfind(marker) {
                let after_marker = input[marker_pos + marker.len()..].trim_start();
                if after_marker.starts_with('[') || after_marker.starts_with('.') {
                    let anime_name = input[..marker_pos].trim().to_string();
                    if !anime_name.is_empty() {
                        return Some((anime_name, "01".to_string(), after_marker));
                    }
                }
            }
        }

        if let Some(tags_pos) = input.find(" [") {
            let title = input[..tags_pos].trim();
            let normalized = title.to_ascii_lowercase();
            let is_explicit_unnumbered_release = normalized.starts_with("gekijouban ")
                || normalized.contains(" the movie")
                || normalized.ends_with(" heroines")
                || title.contains("劇場版")
                || Self::strip_release_year(title).is_some();
            if is_explicit_unnumbered_release && !title.is_empty() {
                return Some((
                    Self::strip_release_year(title).unwrap_or(title).to_string(),
                    "01".to_string(),
                    input[tags_pos..].trim_start(),
                ));
            }
        }

        // Try to find "[XX]" pattern (dmhy.org format with episode in brackets)
        // Look for last occurrence of "[" followed by digits and "]"
        for i in (0..bytes.len()).rev() {
            if bytes[i] == b']' {
                let mut j = i;
                while j > 0 && bytes[j - 1] != b'[' {
                    j -= 1;
                }
                if j > 0 && bytes[j - 1] == b'[' {
                    let content = &bytes[j..i];
                    let version_separator =
                        content.iter().position(|byte| matches!(byte, b'v' | b'V'));
                    let episode_content =
                        version_separator.map_or(content, |position| &content[..position]);
                    let version_is_valid = version_separator.is_none_or(|position| {
                        position > 0
                            && position + 1 < content.len()
                            && content[position + 1..]
                                .iter()
                                .all(|byte| byte.is_ascii_digit())
                    });
                    if !episode_content.is_empty()
                        && episode_content.iter().all(|byte| byte.is_ascii_digit())
                        && version_is_valid
                    {
                        let episode_str = std::str::from_utf8(episode_content).ok()?;
                        if let Ok(ep_num) = episode_str.parse::<u32>() {
                            if (1..=9999).contains(&ep_num) {
                                let episode = format!("{:0>2}", episode_str);
                                let anime_name = input[..j - 1].trim().to_string();
                                let after_episode =
                                    std::str::from_utf8(&bytes[i + 1..]).ok()?.trim_start();
                                return Some((anime_name, episode, after_episode));
                            }
                        }
                    }
                }
            }
        }

        if let Some((title, rest)) = Self::parse_bracketed_unnumbered_release(input) {
            return Some((title, "01".to_string(), rest));
        }

        None
    }

    fn parse_bracketed_unnumbered_release(input: &str) -> Option<(String, &str)> {
        let title_end = input.strip_prefix('[')?.find(']')? + 1;
        let title = input[1..title_end].trim();
        let rest = input[title_end + 1..].trim_start();
        let normalized = rest.to_ascii_lowercase();
        if title.is_empty()
            || !rest.starts_with('[')
            || !normalized.contains("[web-dl]")
            || !(normalized.contains("[baha]") || normalized.contains("[uvod]"))
        {
            return None;
        }
        Some((
            Self::strip_release_year(title).unwrap_or(title).to_string(),
            rest,
        ))
    }

    fn strip_release_year(title: &str) -> Option<&str> {
        let (prefix, year) = title.rsplit_once(' ')?;
        let year = year.parse::<u16>().ok()?;
        (1900..=2099).contains(&year).then_some(prefix.trim_end())
    }

    fn parse_tags_and_ext(input: &str) -> Option<(String, String)> {
        let input = input.trim();
        if input.is_empty() {
            return None;
        }

        if input.ends_with(']') {
            let mut open_stack: Vec<usize> = Vec::new();
            for (i, c) in input.char_indices() {
                if c == '[' {
                    open_stack.push(i);
                } else if c == ']' {
                    if let Some(open_pos) = open_stack.pop() {
                        let bracket_content = &input[open_pos + 1..i];
                        if Self::looks_like_extension(bracket_content) {
                            let tags = input[..open_pos].trim().to_string();
                            return Some((tags, format!(".{}", bracket_content.to_lowercase())));
                        }
                    }
                }
            }
        }

        if let Some(dot_pos) = input.rfind('.') {
            let ext = input[dot_pos + 1..].to_lowercase();
            if Self::looks_like_extension(&ext) {
                let tags = input[..dot_pos].trim().to_string();
                return Some((tags, format!(".{}", ext)));
            }
        }

        if input.ends_with(']') && input.contains('.') {
            if let Some(dot_pos) = input.rfind('.') {
                let ext = &input[dot_pos + 1..];
                if Self::looks_like_extension(ext) {
                    let before_dot = &input[..dot_pos];
                    if before_dot.ends_with(']') {
                        let mut bracket_positions: Vec<usize> = Vec::new();
                        for (i, c) in input.char_indices() {
                            if c == '[' {
                                bracket_positions.push(i);
                            }
                        }
                        for &bracket_start in bracket_positions.iter().rev() {
                            let bracket_end = input[bracket_start..]
                                .find(']')
                                .map(|p| bracket_start + p)
                                .unwrap_or(bracket_start);
                            let content = &input[bracket_start + 1..bracket_end];
                            if Self::looks_like_extension(content) {
                                let tags = input[..bracket_start].trim().to_string();
                                return Some((tags, format!(".{}", content.to_lowercase())));
                            }
                        }
                    }
                    let tags = before_dot.trim().to_string();
                    return Some((tags, format!(".{}", ext.to_lowercase())));
                }
            }
        }

        if input.ends_with(']') && !input.contains('.') {
            let mut bracket_positions: Vec<usize> = Vec::new();
            for (i, c) in input.char_indices() {
                if c == '[' {
                    bracket_positions.push(i);
                }
            }
            for &bracket_start in bracket_positions.iter().rev() {
                let bracket_end = input[bracket_start..]
                    .find(']')
                    .map(|p| bracket_start + p)
                    .unwrap_or(bracket_start);
                let content = &input[bracket_start + 1..bracket_end];
                if Self::looks_like_extension(content) {
                    let tags = input[..bracket_start].trim().to_string();
                    return Some((tags, format!(".{}", content.to_lowercase())));
                }
            }
        }

        None
    }

    fn looks_like_extension(s: &str) -> bool {
        let s = s.trim();
        if s.is_empty() {
            return false;
        }
        let upper = s.to_uppercase();
        matches!(
            upper.as_str(),
            "MP4"
                | "MKV"
                | "AVI"
                | "MOV"
                | "WMV"
                | "FLV"
                | "WEBM"
                | "MPG"
                | "MPEG"
                | "3GP"
                | "OGG"
                | "TS"
                | "M2TS"
        )
    }
}

/// 剥掉番名末尾残留的发布标签，例如 `[中文配音]`、`[1080p]`、`[Baha][WEB-DL]`。
///
/// 规则解析器遇到 `[组] 番名 [标签] - 01` 这类写法时，会把标签留在番名末尾。
/// 这种名字去做 Bangumi 匹配会整体失配（例如 `假面騎士 MY-TH [中文配音]` 谁都搜不到）。
/// 只处理**末尾**的方括号组，且内容必须像发布标签才删除，
/// 避免误伤 `[我推的孩子]` 这种正式标题。
#[must_use]
pub fn strip_residual_tags(name: &str) -> String {
    const SEPARATORS: [char; 6] = [' ', '-', '/', '／', '|', '　'];
    let mut value = name.trim().to_string();
    loop {
        let trimmed = value.trim_end_matches(SEPARATORS);
        if trimmed.len() != value.len() {
            value = trimmed.to_string();
        }
        let Some((start, inner)) = trailing_bracket_group(&value) else {
            break;
        };
        if !looks_like_release_tag(inner) {
            break;
        }
        let head = value[..start].trim_end();
        if head.is_empty() {
            break;
        }
        value = head.to_string();
    }
    value
}

fn trailing_bracket_group(value: &str) -> Option<(usize, &str)> {
    let trimmed = value.trim_end();
    let close = trimmed.chars().next_back()?;
    let open = match close {
        ']' => '[',
        '］' => '［',
        _ => return None,
    };
    let start = trimmed.rfind(open)?;
    let inner_start = start + open.len_utf8();
    let inner_end = trimmed.len() - close.len_utf8();
    if inner_start > inner_end {
        return None;
    }
    let inner = &trimmed[inner_start..inner_end];
    if inner.contains(['[', ']', '［', '］']) {
        return None;
    }
    Some((start, inner))
}

fn looks_like_release_tag(inner: &str) -> bool {
    let value = inner.trim();
    if value.is_empty() || value.chars().count() > 40 {
        return false;
    }
    const CJK_TAGS: &[&str] = &[
        "简",
        "繁",
        "内嵌",
        "内封",
        "外挂",
        "字幕",
        "中文",
        "配音",
        "日语",
        "双语",
        "多语",
        "合集",
        "无修",
        "完全版",
        "修正",
        "招募",
    ];
    if CJK_TAGS.iter().any(|tag| value.contains(tag)) {
        return true;
    }
    const ASCII_TAGS: &[&str] = &[
        "1080", "720", "2160", "480", "576", "1080p", "720p", "2160p", "480p", "576p", "hevc",
        "avc", "x264", "x265", "h264", "h265", "aac", "flac", "opus", "ac3", "dts", "web-dl",
        "webdl", "webrip", "web", "bluray", "bdrip", "bd", "dvdrip", "hdtv", "10bit", "8bit",
        "10-bit", "8-bit", "chs", "cht", "gb", "big5", "repack", "v2", "v3", "fin", "baha",
        "b-global", "amzn", "nf", "hulu", "dmm", "abema", "vcb", "ma10p", "hi10p", "utf-8", "sub",
    ];
    value
        .to_lowercase()
        .split(|ch: char| !ch.is_alphanumeric() && ch != '-')
        .any(|token| !token.is_empty() && ASCII_TAGS.contains(&token))
}

#[cfg(test)]
mod residual_tag_tests {
    use super::strip_residual_tags;

    #[test]
    fn strips_release_tags_at_the_end() {
        assert_eq!(
            strip_residual_tags("假面騎士 MY-TH [中文配音]"),
            "假面騎士 MY-TH"
        );
        assert_eq!(strip_residual_tags("番名 [1080p][Baha][WEB-DL]"), "番名");
        assert_eq!(strip_residual_tags("番名 [简繁日内嵌]"), "番名");
        assert_eq!(strip_residual_tags("番名 [10bit] - "), "番名");
    }

    #[test]
    fn keeps_bracketed_real_titles() {
        assert_eq!(strip_residual_tags("[我推的孩子]"), "[我推的孩子]");
        assert_eq!(strip_residual_tags("葬送的芙莉莲"), "葬送的芙莉莲");
        assert_eq!(
            strip_residual_tags("Re:Zero kara Hajimeru Isekai Seikatsu"),
            "Re:Zero kara Hajimeru Isekai Seikatsu"
        );
    }
}
