//! Bangumi 元数据客户端
//!
//! 批量索引优先使用 Bangumi Archive dump；单条在线详情使用 Bangumi v0 API。
//!
//! ## 数据源
//!
//! - 单条查询：`https://api.bgm.tv/v0/subjects/{id}`
//! - 批量数据：Bangumi Archive Release 中的 `subject.jsonlines`（`type=2` 为动画）

use crate::error::{AppError, Result};
use crate::metadata::wiki::WikiParser;
use crate::metadata::AnimeMetadata;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(feature = "scraper")]
use rusqlite::Connection;

/// Bangumi API 基础 URL
const BANGUMI_API_BASE: &str = "https://api.bgm.tv/v0";

/// Bangumi Archive latest release metadata URL.
const BANGUMI_ARCHIVE_LATEST_JSON: &str =
    "https://raw.githubusercontent.com/bangumi/Archive/master/aux/latest.json";

/// 默认本地 dump 文件名
const SUBJECT_DUMP_FILENAME: &str = "subject.jsonlines";

/// 本地 dump 版本 sidecar 文件名（内容为 release asset 名，如 `dump-2026-10-06.210359Z.zip`）。
const DUMP_VERSION_FILENAME: &str = "subject.jsonlines.version";

/// 本地 dump 的刷新检查间隔：24 小时内不再请求远端版本信息。
const DUMP_REFRESH_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Bangumi Archive dump can be hundreds of MB.
const BANGUMI_ARCHIVE_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// 默认本地数据库文件名
#[cfg(feature = "scraper")]
const BANGUMI_DATABASE_FILENAME: &str = "bangumi.db";

/// Bangumi 要求非浏览器 API 调用带开发者 ID、应用名、版本号和项目主页。
const BANGUMI_USER_AGENT: &str = concat!(
    "ModerRAS/anime-organizer/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/ModerRAS/anime-organizer)"
);

/// Bangumi Subject 基本信息（从 Archive 获取）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BangumiSubject {
    /// Subject ID
    pub id: u32,
    /// 类型（2 = 动画）
    #[serde(rename = "type")]
    pub subject_type: u32,
    /// 标题
    pub name: String,
    /// 中文标题
    #[serde(default)]
    pub name_cn: Option<String>,
    /// 简介
    #[serde(default)]
    pub summary: Option<String>,
    /// 放送日期
    #[serde(default)]
    pub date: Option<String>,
    /// 评分
    #[serde(default)]
    pub score: Option<f32>,
    /// 话数
    #[serde(default)]
    pub eps: Option<u32>,
    /// Wiki Infobox 原始文本（Archive 为字符串；v0 API 为结构化数组时忽略）。
    #[serde(default, deserialize_with = "deserialize_optional_string")]
    pub infobox: Option<String>,
    /// Bangumi 用户标签，写入 MLIP genre。
    #[serde(default, deserialize_with = "deserialize_tag_names")]
    pub tags: Vec<String>,
    /// Bangumi 元标签，作为用户标签缺失时的 genre 后备。
    #[serde(default, deserialize_with = "deserialize_tag_names")]
    pub meta_tags: Vec<String>,
    /// Bangumi v0 API 返回的封面图集合；Archive dump 中通常不存在。
    #[serde(default)]
    pub images: Option<BangumiImages>,
    /// 从 `infobox` 抽取的别名（中文名 / 别名 / 英文名）。
    ///
    /// Archive 不提供独立的别名字段，别名都写在 infobox 的 wiki 文本里；
    /// 这里在载入 dump 时解析一次，供名称匹配使用。
    #[serde(skip)]
    pub aliases: Vec<String>,
}

/// 从 infobox 文本中抽取可用于匹配的别名。
#[must_use]
pub fn infobox_aliases(infobox: Option<&str>) -> Vec<String> {
    let Some(text) = infobox else {
        return Vec::new();
    };
    let Ok(fields) = WikiParser::new().parse(text) else {
        return Vec::new();
    };
    let mut aliases: Vec<String> = Vec::new();
    for key in ["中文名", "别名", "英文名"] {
        let Some(value) = fields.get(key) else {
            continue;
        };
        for item in value.split(" / ") {
            let item = item.trim();
            if !item.is_empty() && !aliases.iter().any(|existing| existing == item) {
                aliases.push(item.to_string());
            }
        }
    }
    aliases
}

fn deserialize_optional_string<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::String(value) => Ok(Some(value)),
        _ => Ok(None),
    }
}

fn deserialize_tag_names<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let serde_json::Value::Array(values) = serde_json::Value::deserialize(deserializer)? else {
        return Ok(Vec::new());
    };

    Ok(values
        .into_iter()
        .filter_map(|value| match value {
            serde_json::Value::String(name) => Some(name),
            serde_json::Value::Object(mut object) => object
                .remove("name")
                .and_then(|name| name.as_str().map(ToOwned::to_owned)),
            _ => None,
        })
        .filter(|name| !name.trim().is_empty())
        .collect())
}

impl BangumiSubject {
    fn best_poster_url(&self) -> Option<String> {
        self.images
            .as_ref()
            .and_then(BangumiImages::best_poster_url)
    }
}

#[derive(Debug, Serialize)]
struct BangumiSearchRequest<'a> {
    keyword: &'a str,
    filter: BangumiSearchFilter,
}

#[derive(Debug, Serialize)]
struct BangumiSearchFilter {
    #[serde(rename = "type")]
    subject_type: [u32; 1],
}

#[derive(Debug, Deserialize)]
struct BangumiSearchResponse {
    #[serde(default)]
    data: Vec<BangumiSubject>,
}

#[derive(Debug, Deserialize)]
struct BangumiEpisodeResponse {
    #[serde(default)]
    data: Vec<BangumiEpisode>,
    #[serde(default)]
    total: u32,
}

#[derive(Debug, Deserialize)]
pub struct BangumiArchiveLatest {
    #[serde(rename = "browser_download_url", alias = "download_url")]
    pub download_url: String,
    /// release asset 名（如 `dump-2026-10-06.210359Z.zip`），作为 dump 版本标识。
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub digest: String,
}

/// Bangumi 分集信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BangumiEpisode {
    /// Episode ID
    pub id: u32,
    /// 所属 Subject ID
    pub subject_id: u32,
    /// 单季内集数编号。
    #[serde(default)]
    pub ep: Option<f64>,
    /// Bangumi 排序编号，有些续季会使用全局集数。
    #[serde(default)]
    pub sort: Option<f64>,
    /// 原始分集标题。
    #[serde(default)]
    pub name: Option<String>,
    /// 中文分集标题。
    #[serde(default)]
    pub name_cn: Option<String>,
    /// 分集简介。
    #[serde(default, rename = "desc")]
    pub summary: Option<String>,
    /// 秒级时长。
    #[serde(default)]
    pub duration_seconds: Option<u32>,
    /// 0 = 正片。
    #[serde(default, rename = "type")]
    pub episode_type: u32,
}

impl BangumiEpisode {
    #[must_use]
    pub fn display_title(&self) -> Option<String> {
        self.name_cn
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                self.name
                    .as_deref()
                    .filter(|value| !value.trim().is_empty())
            })
            .map(|value| value.trim().to_string())
    }

    #[must_use]
    pub fn cleaned_summary(&self) -> Option<String> {
        self.summary
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    }
}

/// Bangumi 封面图 URL 集合。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BangumiImages {
    #[serde(default)]
    pub large: Option<String>,
    #[serde(default)]
    pub common: Option<String>,
    #[serde(default)]
    pub grid: Option<String>,
    #[serde(default)]
    pub medium: Option<String>,
    #[serde(default)]
    pub small: Option<String>,
}

impl BangumiImages {
    fn best_poster_url(&self) -> Option<String> {
        self.large
            .clone()
            .or_else(|| self.common.clone())
            .or_else(|| self.grid.clone())
    }
}

/// Bangumi 元数据客户端
///
/// 从 Bangumi Archive dump 和 Bangumi v0 API 获取动画元数据。
/// 支持单条在线查询和本地缓存的批量数据查询。
///
/// # 示例
///
/// ```no_run
/// # async fn example() -> anime_organizer::error::Result<()> {
/// use anime_organizer::metadata::BangumiClient;
///
/// let client = BangumiClient::new(None);
/// let metadata = client.fetch_metadata(328609).await?;
/// println!("{}: {}", metadata.title, metadata.summary);
/// # Ok(())
/// # }
/// ```
pub struct BangumiClient {
    #[cfg(feature = "metadata")]
    http: reqwest::Client,
    /// 本地缓存目录
    cache_dir: PathBuf,
    /// 显式指定的数据源（subject.jsonlines）
    source_path: Option<PathBuf>,
    /// 内存中的 Subject 索引（从 dump 加载后缓存）
    index: std::sync::Mutex<Option<HashMap<u32, BangumiSubject>>>,
    #[cfg(feature = "scraper")]
    /// 内存中的别名索引（从数据库加载后缓存）：alias -> (subject_id, canonical_name)
    aliases_index: std::sync::Mutex<Option<HashMap<String, (u32, String)>>>,
}

fn default_cache_dir() -> PathBuf {
    default_cache_dir_from_env(
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("APPDATA"),
        std::env::var_os("XDG_CACHE_HOME"),
        std::env::var_os("HOME"),
        std::env::temp_dir(),
    )
}

fn default_cache_dir_from_env(
    local_app_data: Option<OsString>,
    app_data: Option<OsString>,
    xdg_cache_home: Option<OsString>,
    home: Option<OsString>,
    temp_dir: PathBuf,
) -> PathBuf {
    let root = if cfg!(windows) {
        local_app_data.or(app_data).map(PathBuf::from)
    } else {
        xdg_cache_home
            .map(PathBuf::from)
            .or_else(|| home.map(|path| PathBuf::from(path).join(".cache")))
    }
    .unwrap_or(temp_dir);

    root.join("anime-organizer").join("bangumi-cache")
}

impl BangumiClient {
    /// 创建新的 Bangumi 客户端
    ///
    /// # 参数
    ///
    /// - `cache_dir` - 本地缓存目录，默认使用系统缓存目录下的 `anime-organizer/bangumi-cache`
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        Self::with_source(cache_dir, None)
    }

    /// 创建新的 Bangumi 客户端，并允许显式指定 dump 路径。
    pub fn with_source(cache_dir: Option<PathBuf>, source_path: Option<PathBuf>) -> Self {
        let cache_dir = cache_dir.unwrap_or_else(default_cache_dir);
        Self {
            #[cfg(feature = "metadata")]
            http: reqwest::Client::builder()
                .user_agent(BANGUMI_USER_AGENT)
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .build()
                .expect("创建 HTTP 客户端失败"),
            cache_dir,
            source_path,
            index: std::sync::Mutex::new(None),
            #[cfg(feature = "scraper")]
            aliases_index: std::sync::Mutex::new(None),
        }
    }

    /// 从 Bangumi v0 API 在线获取单个 Subject 信息。
    #[cfg(feature = "metadata")]
    pub async fn fetch_subject(&self, bangumi_id: u32) -> Result<BangumiSubject> {
        let url = format!("{BANGUMI_API_BASE}/subjects/{bangumi_id}");
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("请求 Bangumi API 失败: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::AnimeNotFoundError(format!(
                "Bangumi Subject {bangumi_id} 未找到 (HTTP {})",
                resp.status()
            )));
        }

        let subject: BangumiSubject = resp
            .json()
            .await
            .map_err(|e| AppError::BangumiParseError(format!("解析 Subject JSON 失败: {e}")))?;

        Ok(subject)
    }

    /// 获取完整的动画元数据
    ///
    /// 从 Bangumi 获取 Subject 信息并解析 Wiki Infobox。
    #[cfg(feature = "metadata")]
    pub async fn fetch_metadata(&self, bangumi_id: u32) -> Result<AnimeMetadata> {
        if self.prepare_index().await.is_ok() {
            if let Some(subject) = self.find_by_id(bangumi_id)? {
                let mut metadata = self.subject_to_metadata(&subject);
                if metadata.poster_url.is_none() {
                    if let Ok(online_subject) = self.fetch_subject(bangumi_id).await {
                        metadata.poster_url = online_subject.best_poster_url();
                    }
                }
                return Ok(metadata);
            }
        }

        let subject = self.fetch_subject(bangumi_id).await?;
        Ok(self.subject_to_metadata(&subject))
    }

    /// 使用 Bangumi v0 API 按标题搜索动画 Subject。
    #[cfg(feature = "metadata")]
    pub async fn search_subjects(&self, query: &str, limit: u32) -> Result<Vec<BangumiSubject>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!(
            "{BANGUMI_API_BASE}/search/subjects?limit={}",
            limit.clamp(1, 20)
        );
        let resp = self
            .http
            .post(&url)
            .json(&BangumiSearchRequest {
                keyword: query,
                filter: BangumiSearchFilter { subject_type: [2] },
            })
            .send()
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("搜索 Bangumi 失败: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::MetadataFetchError(format!(
                "搜索 Bangumi 失败 (HTTP {})",
                resp.status()
            )));
        }

        let response: BangumiSearchResponse = resp
            .json()
            .await
            .map_err(|e| AppError::BangumiParseError(format!("解析 Bangumi 搜索结果失败: {e}")))?;

        Ok(response
            .data
            .into_iter()
            .filter(|subject| subject.subject_type == 2)
            .collect())
    }

    /// 使用 Bangumi v0 API 获取正片分集信息。
    #[cfg(feature = "metadata")]
    pub async fn fetch_episodes(&self, bangumi_id: u32) -> Result<Vec<BangumiEpisode>> {
        let limit = 100u32;
        let mut offset = 0u32;
        let mut episodes = Vec::new();

        loop {
            let url = format!(
                "{BANGUMI_API_BASE}/episodes?subject_id={bangumi_id}&type=0&limit={limit}&offset={offset}"
            );
            let resp =
                self.http.get(&url).send().await.map_err(|e| {
                    AppError::MetadataFetchError(format!("获取 Bangumi 分集失败: {e}"))
                })?;

            if !resp.status().is_success() {
                return Err(AppError::MetadataFetchError(format!(
                    "获取 Bangumi 分集失败 (HTTP {})",
                    resp.status()
                )));
            }

            let response: BangumiEpisodeResponse = resp.json().await.map_err(|e| {
                AppError::BangumiParseError(format!("解析 Bangumi 分集结果失败: {e}"))
            })?;
            let count = response.data.len() as u32;
            episodes.extend(
                response
                    .data
                    .into_iter()
                    .filter(|episode| episode.episode_type == 0),
            );

            offset += limit;
            if count < limit || offset >= response.total {
                break;
            }
        }

        Ok(episodes)
    }

    /// 从本地 dump 文件中按 ID 查找
    pub fn find_by_id(&self, bangumi_id: u32) -> Result<Option<BangumiSubject>> {
        let index = self
            .index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定索引失败: {e}")))?;
        Ok(index.as_ref().and_then(|idx| idx.get(&bangumi_id).cloned()))
    }

    /// 从本地 dump 文件中按名称精确查找
    pub fn find_by_name(&self, name: &str) -> Result<Option<BangumiSubject>> {
        let index = self
            .index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定索引失败: {e}")))?;
        if let Some(subject) = index.as_ref().and_then(|idx| {
            idx.values()
                .find(|s| {
                    s.name == name
                        || s.name_cn.as_deref() == Some(name)
                        || s.aliases.iter().any(|alias| alias == name)
                })
                .cloned()
        }) {
            return Ok(Some(subject));
        }
        #[cfg(feature = "scraper")]
        {
            drop(index);
            if let Ok(Some((bangumi_id, _))) = self.find_by_alias(name) {
                return self.find_by_id(bangumi_id);
            }
        }
        Ok(None)
    }

    #[cfg(feature = "scraper")]
    pub fn find_by_alias(&self, alias: &str) -> Result<Option<(u32, String)>> {
        self.prepare_aliases_index()?;
        let aliases_index = self
            .aliases_index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定别名索引失败: {e}")))?;
        Ok(aliases_index
            .as_ref()
            .and_then(|idx| idx.get(alias).cloned()))
    }

    #[cfg(feature = "scraper")]
    pub fn get_aliases(&self, bangumi_id: u32) -> Result<Vec<String>> {
        self.prepare_aliases_index()?;
        let db_path = self.cache_dir.join(BANGUMI_DATABASE_FILENAME);
        if !db_path.is_file() {
            return Ok(Vec::new());
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| AppError::BangumiParseError(format!("打开数据库失败: {e}")))?;
        let mut stmt = conn
            .prepare("SELECT alias FROM aliases WHERE subject_id = ?1")
            .map_err(|e| AppError::BangumiParseError(format!("预处理 SQL 失败: {e}")))?;
        let aliases = stmt
            .query_map([bangumi_id], |row| row.get::<_, String>(0))
            .map_err(|e| AppError::BangumiParseError(format!("查询别名失败: {e}")))?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| AppError::BangumiParseError(format!("读取别名失败: {e}")))?;
        Ok(aliases)
    }

    #[cfg(feature = "scraper")]
    fn prepare_aliases_index(&self) -> Result<()> {
        {
            let aliases_index = self
                .aliases_index
                .lock()
                .map_err(|e| AppError::BangumiParseError(format!("锁定别名索引失败: {e}")))?;
            if aliases_index.is_some() {
                return Ok(());
            }
        }
        let db_path = self.cache_dir.join(BANGUMI_DATABASE_FILENAME);
        if !db_path.is_file() {
            let mut aliases_index = self
                .aliases_index
                .lock()
                .map_err(|e| AppError::BangumiParseError(format!("锁定别名索引失败: {e}")))?;
            *aliases_index = Some(HashMap::new());
            return Ok(());
        }
        let conn = Connection::open(&db_path)
            .map_err(|e| AppError::BangumiParseError(format!("打开数据库失败: {e}")))?;
        let mut stmt = conn
            .prepare(
                "SELECT a.alias, a.subject_id, s.name FROM aliases a JOIN subjects s ON a.subject_id = s.id",
            )
            .map_err(|e| AppError::BangumiParseError(format!("预处理 SQL 失败: {e}")))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, u32>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(|e| AppError::BangumiParseError(format!("查询别名失败: {e}")))?;
        let mut aliases: HashMap<String, (u32, String)> = HashMap::new();
        for row in rows {
            let (alias, subject_id, name) =
                row.map_err(|e| AppError::BangumiParseError(format!("读取别名失败: {e}")))?;
            aliases.insert(alias, (subject_id, name));
        }
        let mut aliases_index = self
            .aliases_index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定别名索引失败: {e}")))?;
        *aliases_index = Some(aliases);
        Ok(())
    }

    #[cfg(feature = "scraper")]
    #[allow(dead_code)]
    fn resolve_existing_db_path(&self) -> Option<PathBuf> {
        let db_path = self.cache_dir.join(BANGUMI_DATABASE_FILENAME);
        if db_path.is_file() {
            Some(db_path)
        } else {
            None
        }
    }

    /// 从本地 dump 文件中模糊搜索
    pub fn search(&self, query: &str) -> Result<Vec<BangumiSubject>> {
        let query_lower = query.to_lowercase();
        let index = self
            .index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定索引失败: {e}")))?;
        Ok(index
            .as_ref()
            .map(|idx| {
                idx.values()
                    .filter(|s| {
                        s.name.to_lowercase().contains(&query_lower)
                            || s.name_cn
                                .as_ref()
                                .is_some_and(|cn| cn.to_lowercase().contains(&query_lower))
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    /// 在已加载的本地 dump 中按调用方评分返回前几个候选。
    pub fn find_local_subjects<F>(
        &self,
        min_score: f32,
        limit: usize,
        score: F,
    ) -> Result<Vec<(BangumiSubject, f32)>>
    where
        F: Fn(&BangumiSubject) -> f32,
    {
        let index = self
            .index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定索引失败: {e}")))?;
        let mut matches = index
            .as_ref()
            .into_iter()
            .flat_map(|subjects| subjects.values())
            .filter_map(|subject| {
                let value = score(subject);
                (value >= min_score).then_some((subject.clone(), value))
            })
            .collect::<Vec<_>>();
        matches.sort_by(|(left_subject, left), (right_subject, right)| {
            right
                .total_cmp(left)
                .then_with(|| left_subject.id.cmp(&right_subject.id))
        });
        matches.truncate(limit);
        Ok(matches)
    }

    /// 加载本地 JSONL dump 文件
    ///
    /// 从缓存目录中加载 `subject.jsonlines`，仅保留 `type=2`（动画）的条目。
    pub fn load_dump(&self, dump_path: &Path) -> Result<usize> {
        let file = std::fs::File::open(dump_path)
            .map_err(|e| AppError::BangumiParseError(format!("打开 dump 文件失败: {e}")))?;

        let reader = std::io::BufReader::new(file);
        let mut subjects = HashMap::new();

        for line in reader.lines() {
            let line = line.map_err(|e| AppError::BangumiParseError(format!("读取行失败: {e}")))?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if let Ok(mut subject) = serde_json::from_str::<BangumiSubject>(line) {
                // 仅保留动画类型
                if subject.subject_type == 2 {
                    subject.aliases = infobox_aliases(subject.infobox.as_deref());
                    subjects.insert(subject.id, subject);
                }
            }
        }

        let count = subjects.len();
        let mut index = self
            .index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定索引失败: {e}")))?;
        *index = Some(subjects);

        Ok(count)
    }

    /// 准备本地索引：优先加载现有 dump；按 TTL 检查远端版本，过期就刷新。
    ///
    /// 以前这里只要有本地文件就直接用，CLI 用户永远停在第一次下载的那份快照上，
    /// 新番在本地库里根本不存在。现在与守护进程一致：先看 sidecar 版本是否过期，
    /// 过期才去拉 500 字节的 `latest.json` 比对 asset 名，真的变了才重新下载。
    /// 任何一步失败都继续使用现有缓存。
    #[cfg(feature = "metadata")]
    pub async fn prepare_index(&self) -> Result<usize> {
        if let Some(len) = self.index_len()? {
            return Ok(len);
        }

        if let Some(path) = self.resolve_existing_dump_path() {
            if self.dump_refresh_due().await {
                if let Ok(fresh) = self.download_dump().await {
                    return self.load_dump(&fresh);
                }
            }
            return self.load_dump(&path);
        }

        let dump_path = self.download_dump().await?;
        self.load_dump(&dump_path)
    }

    /// 本地 dump 是否该检查/执行刷新（TTL 内直接返回 false，避免每次启动都请求远端）。
    #[cfg(feature = "metadata")]
    async fn dump_refresh_due(&self) -> bool {
        let Some(version) = self.dump_version() else {
            return true;
        };
        let sidecar = self.cache_dir.join(DUMP_VERSION_FILENAME);
        let stale = match std::fs::metadata(&sidecar).and_then(|meta| meta.modified()) {
            Ok(modified) => modified
                .elapsed()
                .map(|age| age >= DUMP_REFRESH_TTL)
                .unwrap_or(true),
            Err(_) => true,
        };
        if !stale {
            return false;
        }
        match self.fetch_latest().await {
            Ok(latest) => !latest.name.is_empty() && latest.name != version,
            Err(_) => false,
        }
    }

    fn index_len(&self) -> Result<Option<usize>> {
        let index = self
            .index
            .lock()
            .map_err(|e| AppError::BangumiParseError(format!("锁定索引失败: {e}")))?;
        Ok(index.as_ref().map(HashMap::len))
    }

    pub fn resolve_existing_dump_path(&self) -> Option<PathBuf> {
        let candidates = [
            self.source_path.clone(),
            Some(self.cache_dir.join(SUBJECT_DUMP_FILENAME)),
        ];

        for candidate in candidates.into_iter().flatten() {
            if candidate.is_file() {
                return Some(candidate);
            }

            let nested = candidate.join(SUBJECT_DUMP_FILENAME);
            if nested.is_file() {
                return Some(nested);
            }
        }

        None
    }

    /// 获取 Bangumi Archive 最新 dump 的版本信息（约 500 字节，可高频轮询）。
    #[cfg(feature = "metadata")]
    pub async fn fetch_latest(&self) -> Result<BangumiArchiveLatest> {
        let latest_resp = self
            .http
            .get(BANGUMI_ARCHIVE_LATEST_JSON)
            .send()
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("获取 dump 版本信息失败: {e}")))?;
        if !latest_resp.status().is_success() {
            return Err(AppError::MetadataFetchError(format!(
                "获取 dump 版本信息失败 (HTTP {})",
                latest_resp.status()
            )));
        }

        latest_resp
            .json()
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("解析 dump 版本信息失败: {e}")))
    }

    /// 本地 dump 的版本标识（下载成功时写入的 sidecar 文件内容），缺失返回 `None`。
    pub fn dump_version(&self) -> Option<String> {
        std::fs::read_to_string(self.cache_dir.join(DUMP_VERSION_FILENAME))
            .ok()
            .map(|version| version.trim().to_string())
            .filter(|version| !version.is_empty())
    }

    /// 下载最新的 Bangumi Archive dump
    #[cfg(feature = "metadata")]
    pub async fn download_dump(&self) -> Result<PathBuf> {
        std::fs::create_dir_all(&self.cache_dir)
            .map_err(|e| AppError::MetadataFetchError(format!("创建缓存目录失败: {e}")))?;

        let latest = self.fetch_latest().await?;

        let dump_path = self.cache_dir.join(SUBJECT_DUMP_FILENAME);
        let zip_path = self.cache_dir.join("bangumi-archive.zip.tmp");
        let mut zip_output = std::fs::File::create(&zip_path)
            .map_err(|e| AppError::MetadataFetchError(format!("创建 dump 临时文件失败: {e}")))?;

        let mut resp = self
            .http
            .get(&latest.download_url)
            .timeout(BANGUMI_ARCHIVE_DOWNLOAD_TIMEOUT)
            .send()
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("下载 Bangumi dump 失败: {e}")))?;
        if !resp.status().is_success() {
            return Err(AppError::MetadataFetchError(format!(
                "下载 dump 失败 (HTTP {})",
                resp.status()
            )));
        }

        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("读取 dump 数据失败: {e}")))?
        {
            std::io::Write::write_all(&mut zip_output, &chunk).map_err(|e| {
                AppError::MetadataFetchError(format!("写入 dump 临时文件失败: {e}"))
            })?;
        }
        drop(zip_output);

        let zip_file = std::fs::File::open(&zip_path)
            .map_err(|e| AppError::MetadataFetchError(format!("打开 dump ZIP 失败: {e}")))?;
        let mut archive = zip::ZipArchive::new(zip_file)
            .map_err(|e| AppError::MetadataFetchError(format!("读取 dump ZIP 失败: {e}")))?;
        let mut subject_file = archive.by_name(SUBJECT_DUMP_FILENAME).map_err(|e| {
            AppError::MetadataFetchError(format!("在 dump ZIP 中找不到 subject.jsonlines: {e}"))
        })?;
        let mut output = std::fs::File::create(&dump_path)
            .map_err(|e| AppError::MetadataFetchError(format!("写入 dump 文件失败: {e}")))?;
        std::io::copy(&mut subject_file, &mut output)
            .map_err(|e| AppError::MetadataFetchError(format!("解压 dump 失败: {e}")))?;
        let _ = std::fs::remove_file(&zip_path);
        let _ = std::fs::write(self.cache_dir.join(DUMP_VERSION_FILENAME), &latest.name);

        Ok(dump_path)
    }

    /// 下载图片内容为字节。
    #[cfg(feature = "metadata")]
    pub async fn download_image_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| AppError::ImageDownloadError(format!("图片下载请求失败: {e}")))?;

        if !resp.status().is_success() {
            return Err(AppError::ImageDownloadError(format!(
                "图片下载失败 (HTTP {})",
                resp.status()
            )));
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| AppError::ImageDownloadError(format!("读取图片数据失败: {e}")))?;

        Ok(bytes.to_vec())
    }

    /// 获取缓存目录路径
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    /// 检查本地 dump 是否存在
    pub fn has_local_dump(&self) -> bool {
        self.resolve_existing_dump_path().is_some()
    }

    /// 将 BangumiSubject 转为 AnimeMetadata
    ///
    /// `title_cn` 只收真正的中文（含汉字且不含假名）：英文 `name_cn`
    /// （如 `Girls Band Cry`）会被丢弃，改为尝试 infobox `中文名`；
    /// 两者都无中文时为 `None`，调用方经 `preferred_title` 回退日文原名。
    fn subject_to_metadata(&self, subject: &BangumiSubject) -> AnimeMetadata {
        let mut metadata = AnimeMetadata::new(subject.id, subject.name.clone());

        let infobox = subject
            .infobox
            .as_deref()
            .and_then(|infobox| WikiParser::new().parse_anime_infobox(infobox).ok());
        metadata.title_cn = [
            subject.name_cn.as_deref(),
            infobox.as_ref().and_then(|info| info.name_cn.as_deref()),
        ]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .find(|name| super::is_chinese_title(name))
        .map(str::to_string);
        metadata.original_title = subject.name.clone();
        metadata.summary = subject.summary.clone().unwrap_or_default();
        metadata.air_date = subject.date.clone();
        metadata.rating = subject.score.unwrap_or(0.0);
        metadata.episode_count = subject.eps.unwrap_or(0);
        metadata.poster_url = subject.best_poster_url();
        metadata.genre = if subject.tags.is_empty() {
            subject.meta_tags.iter().take(8).cloned().collect()
        } else {
            subject.tags.iter().take(8).cloned().collect()
        };

        // 复用已解析的 Infobox 提取额外信息
        if let Some(info) = infobox {
            if info.studio.is_some() {
                metadata.studio = info.studio;
            }
            if info.director.is_some() {
                metadata.director = info.director;
            }
        }

        metadata
    }
}

impl Default for BangumiClient {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_subject(images: Option<BangumiImages>) -> BangumiSubject {
        BangumiSubject {
            id: 431767,
            subject_type: 2,
            name: "ガールズバンドクライ".to_string(),
            name_cn: Some("Girls Band Cry".to_string()),
            summary: Some("summary".to_string()),
            date: Some("2024-04-06".to_string()),
            score: Some(8.2),
            eps: Some(13),
            infobox: None,
            tags: vec!["原创".to_string()],
            meta_tags: Vec::new(),
            images,
            aliases: Vec::new(),
        }
    }

    #[test]
    fn infobox_aliases_include_multi_value_block() {
        let infobox = "{{Infobox animanga/TVAnime\n|中文名=黄泉使者\n|别名={\n[黃泉的使者]\n[Yomi no Tsugai]\n}\n|话数=24\n}}";
        let aliases = infobox_aliases(Some(infobox));
        assert!(aliases.contains(&"黄泉使者".to_string()));
        assert!(aliases.contains(&"黃泉的使者".to_string()));
        assert!(aliases.contains(&"Yomi no Tsugai".to_string()));
    }

    #[test]
    fn bangumi_images_prefer_large_common_then_grid() {
        let images = BangumiImages {
            large: None,
            common: Some("common.jpg".to_string()),
            grid: Some("grid.jpg".to_string()),
            medium: Some("medium.jpg".to_string()),
            small: Some("small.jpg".to_string()),
        };

        assert_eq!(images.best_poster_url().as_deref(), Some("common.jpg"));

        let images = BangumiImages {
            large: None,
            common: None,
            grid: Some("grid.jpg".to_string()),
            medium: Some("medium.jpg".to_string()),
            small: Some("small.jpg".to_string()),
        };

        assert_eq!(images.best_poster_url().as_deref(), Some("grid.jpg"));
    }

    #[test]
    fn search_response_deserializes_subjects() {
        let response: BangumiSearchResponse = serde_json::from_str(
            r#"{"data":[{"id":498947,"type":2,"name":"ガチアクタ","name_cn":"咔嗒咔嗒","infobox":[{"key":"中文名","value":"咔嗒咔嗒"}],"tags":[{"name":"战斗"}],"meta_tags":["2025年7月"],"images":{"large":"large.jpg"}}]}"#,
        )
        .unwrap();

        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].id, 498947);
        assert_eq!(response.data[0].tags, ["战斗"]);
        assert_eq!(
            response.data[0].best_poster_url().as_deref(),
            Some("large.jpg")
        );
        assert!(response.data[0].infobox.is_none());
    }

    #[test]
    fn episode_response_deserializes_titles_and_duration() {
        let response: BangumiEpisodeResponse = serde_json::from_str(
            r#"{"data":[{"id":1519541,"subject_id":498947,"ep":1,"sort":1,"name":"天界","name_cn":"天界","desc":"简介","duration_seconds":1420,"type":0}],"total":1}"#,
        )
        .unwrap();

        assert_eq!(response.data.len(), 1);
        assert_eq!(response.data[0].display_title().as_deref(), Some("天界"));
        assert_eq!(response.data[0].cleaned_summary().as_deref(), Some("简介"));
        assert_eq!(response.data[0].duration_seconds, Some(1420));
    }

    #[test]
    fn archive_latest_deserializes_download_url() {
        let latest: BangumiArchiveLatest = serde_json::from_str(
            r#"{"browser_download_url":"https://github.com/bangumi/Archive/releases/download/archive/dump.zip","name":"dump-2026-10-06.210359Z.zip","digest":"sha256:abc"}"#,
        )
        .unwrap();

        assert_eq!(
            latest.download_url,
            "https://github.com/bangumi/Archive/releases/download/archive/dump.zip"
        );
        assert_eq!(latest.name, "dump-2026-10-06.210359Z.zip");
        assert_eq!(latest.digest, "sha256:abc");
    }

    #[test]
    fn dump_version_round_trips_through_sidecar() {
        let directory = tempfile::tempdir().unwrap();
        let client = BangumiClient::new(Some(directory.path().to_path_buf()));
        assert_eq!(client.dump_version(), None);
        std::fs::write(
            directory.path().join("subject.jsonlines.version"),
            "dump-2026-10-06.210359Z.zip\n",
        )
        .unwrap();
        assert_eq!(
            client.dump_version().as_deref(),
            Some("dump-2026-10-06.210359Z.zip")
        );
    }

    #[test]
    fn default_cache_dir_uses_platform_cache_root() {
        let path = default_cache_dir_from_env(
            Some(OsString::from("local")),
            Some(OsString::from("roaming")),
            Some(OsString::from("xdg")),
            Some(OsString::from("home")),
            PathBuf::from("tmp"),
        );
        let expected_root = if cfg!(windows) {
            PathBuf::from("local")
        } else {
            PathBuf::from("xdg")
        };

        assert_eq!(
            path,
            expected_root.join("anime-organizer").join("bangumi-cache")
        );
    }

    #[test]
    fn default_cache_dir_falls_back_to_temp() {
        assert_eq!(
            default_cache_dir_from_env(None, None, None, None, PathBuf::from("tmp")),
            PathBuf::from("tmp")
                .join("anime-organizer")
                .join("bangumi-cache")
        );
    }

    #[test]
    fn subject_to_metadata_ignores_blank_chinese_title() {
        let client = BangumiClient::new(None);
        let mut subject = sample_subject(None);
        subject.name_cn = Some("  ".to_string());

        let metadata = client.subject_to_metadata(&subject);

        assert_eq!(metadata.title_cn, None);
    }

    #[test]
    fn subject_to_metadata_rejects_english_name_cn() {
        // Girls Band Cry 的 name_cn 本身就是英文，不能冒充中文。
        let client = BangumiClient::new(None);
        let subject = sample_subject(None);

        let metadata = client.subject_to_metadata(&subject);

        assert_eq!(metadata.title_cn, None);
        assert_eq!(metadata.preferred_title(), "ガールズバンドクライ");
    }

    #[test]
    fn subject_to_metadata_falls_back_to_infobox_chinese_name() {
        let client = BangumiClient::new(None);
        let mut subject = sample_subject(None);
        subject.infobox =
            Some("{{Infobox animanga/TVAnime\n|中文名= 少女乐队的呐喊\n}}".to_string());

        let metadata = client.subject_to_metadata(&subject);

        assert_eq!(metadata.title_cn.as_deref(), Some("少女乐队的呐喊"));
        assert_eq!(metadata.preferred_title(), "少女乐队的呐喊");
    }

    #[test]
    fn subject_to_metadata_keeps_bangumi_poster_url() {
        let client = BangumiClient::new(None);
        let subject = sample_subject(Some(BangumiImages {
            large: Some("large.jpg".to_string()),
            common: Some("common.jpg".to_string()),
            grid: Some("grid.jpg".to_string()),
            medium: None,
            small: None,
        }));

        let metadata = client.subject_to_metadata(&subject);

        assert_eq!(metadata.poster_url.as_deref(), Some("large.jpg"));
    }
}
