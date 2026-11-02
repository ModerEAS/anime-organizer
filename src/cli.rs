use anime_organizer::OperationMode;
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

macro_rules! app_long_about {
    ($($extra:literal)*) => {
        concat!(
            "AnimeOrganizer v",
            env!("CARGO_PKG_VERSION"),
            " - 跨平台动漫文件整理工具\n\n",
            "默认模式用于批量整理动漫文件：\n",
            "    aniorg --source=\"D:\\Downloads\" --target=\"E:\\Anime\"\n\n",
            "启用元数据刮削：\n",
            "    aniorg --source=\"D:\\Downloads\" --scrape-metadata\n\n",
            "生成 MiruPlay MLIP 媒体库：\n",
            "    aniorg --source=\"D:\\Downloads\" --target=\"E:\\Anime\" --mlip\n\n",
            "启用 scraper 子命令（需以 --features scraper 编译）：\n",
            "    aniorg scrape --days 7 --format json\n",
            "    aniorg match --input scraped.json --format github\n",
            $($extra),*
        )
    };
}

#[cfg(feature = "self-update")]
const APP_LONG_ABOUT: &str = app_long_about!(
    "\n检查并安装新版本：\n"
    "    aniorg update\n"
    "    aniorg update --check\n"
);
#[cfg(not(feature = "self-update"))]
const APP_LONG_ABOUT: &str = app_long_about!();

/// 跨平台动漫文件整理工具
#[derive(Parser, Debug)]
#[command(name = "aniorg")]
#[command(version = env!("CARGO_PKG_VERSION"))]
#[command(about = "轻量级、跨平台动漫文件整理工具")]
#[command(long_about = APP_LONG_ABOUT)]
pub(crate) struct Cli {
    /// 启动本地 WebAPI 守护进程
    #[cfg(feature = "daemon")]
    #[arg(long)]
    pub(crate) daemon: bool,

    /// 替换进程使用：新程序文件。
    #[cfg(feature = "self-update")]
    #[arg(long, hide = true)]
    pub(crate) replace_from: Option<PathBuf>,

    /// 替换进程使用：当前程序路径。
    #[cfg(feature = "self-update")]
    #[arg(long, hide = true)]
    pub(crate) replace_to: Option<PathBuf>,

    /// 替换进程使用：等原进程退出后再替换。
    #[cfg(feature = "self-update")]
    #[arg(long, hide = true)]
    pub(crate) wait_for_parent: bool,

    /// 替换完成后重新启动时附带的原始参数。
    #[cfg(feature = "self-update")]
    #[arg(long, hide = true, allow_hyphen_values = true)]
    pub(crate) restart_arg: Vec<String>,

    /// 这次启动来自程序自己的替换。
    #[cfg(feature = "self-update")]
    #[arg(long, hide = true)]
    pub(crate) from_self_update: bool,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,

    #[command(flatten)]
    pub(crate) organize: OrganizeArgs,
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct OrganizeArgs {
    /// 源目录路径（整理模式必填）
    #[arg(short, long, value_name = "PATH")]
    pub(crate) source: Option<PathBuf>,

    /// 目标根目录（默认：与源目录相同）
    #[arg(short, long, value_name = "PATH")]
    pub(crate) target: Option<PathBuf>,

    /// 操作模式：move（移动）、copy（复制）、link（硬链接）
    #[arg(short, long, value_enum, default_value = "link")]
    pub(crate) mode: OperationMode,

    /// 硬链接失败时的回退模式：move 或 copy（默认不回退）
    #[arg(long, value_enum, value_name = "MODE")]
    pub(crate) fallback_on_link_failure: Option<FallbackMode>,

    /// 仅预览不执行
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// 包含的扩展名（逗号分隔，默认：mp4,mkv,avi,mov,wmv,flv,rmvb）
    #[arg(long, value_name = "EXT", value_delimiter = ',')]
    pub(crate) include_ext: Option<Vec<String>>,

    /// 显示详细日志
    #[arg(short, long)]
    pub(crate) verbose: bool,

    /// 启用元数据刮削（生成 NFO 文件和下载封面图片）
    #[arg(long = "scrape-metadata", visible_alias = "刮削")]
    pub(crate) scrape_metadata: bool,

    /// TMDB API Key（用于下载封面图片）
    #[arg(long, value_name = "KEY")]
    pub(crate) tmdb_api_key: Option<String>,

    /// 自定义别名文件（JSON），会覆盖本地别名库中的同名项
    #[arg(long, value_name = "PATH")]
    pub(crate) alias_file: Option<PathBuf>,

    /// 跳过 Kodi/MLIP 图片下载（仍写入可用的元数据）
    #[arg(long)]
    pub(crate) no_images: bool,

    /// 跳过 Bangumi 分集标题、简介和时长查询
    #[arg(long)]
    pub(crate) no_episode_metadata: bool,

    /// 覆盖已有的 NFO 和图片文件
    #[arg(long)]
    pub(crate) force_overwrite: bool,

    /// Bangumi/AnimeAtlas 缓存目录（默认使用系统缓存目录，不写入媒体库）
    #[arg(long, value_name = "PATH")]
    pub(crate) bangumi_cache: Option<PathBuf>,

    /// 本地 Bangumi 元数据源（subject.jsonlines 或其目录；启用后保持离线）
    #[arg(long, value_name = "PATH")]
    pub(crate) metadata_source: Option<PathBuf>,

    /// 启用分季模式：按 `番名/Season N/` 结构整理文件
    #[arg(long = "season-mode", visible_alias = "分季")]
    pub(crate) season_mode: bool,

    /// 生成/更新目标目录根部的 MLIP 媒体库索引 library.db
    #[arg(long)]
    pub(crate) library_index: bool,

    /// 生成 MiruPlay MLIP 媒体库（library.db、Bangumi 元数据和缺失海报）
    #[arg(long)]
    pub(crate) mlip: bool,

    /// 全量扫描目标目录并重建索引；与 --mlip 合用时补元数据和缺失图片
    #[arg(long)]
    pub(crate) rebuild_library_index: bool,

    /// 仅从现有 library.db 重新匹配并刷新元数据，不扫描媒体目录
    #[arg(long)]
    pub(crate) refresh_library_metadata: bool,

    /// 使用 ffprobe 探测视频时长并写入 MLIP episode.runtime（秒）
    #[arg(long)]
    pub(crate) probe_runtime: bool,

    /// 文件名解析器：rules（默认规则）、anifilebert（ONNX）、auto（规则失败后回退 ONNX）
    #[arg(long, value_enum, default_value = "rules")]
    pub(crate) filename_parser: FilenameParserMode,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FilenameParserMode {
    /// 使用内置规则解析器
    #[default]
    Rules,
    /// 使用嵌入的 AniFileBERT ONNX 模型解析器
    Anifilebert,
    /// 先用规则解析器，失败后回退 AniFileBERT
    Auto,
}

#[derive(Clone, Copy, Debug, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FallbackMode {
    /// 移动文件
    Move,
    /// 复制文件
    Copy,
}

impl FallbackMode {
    pub(crate) fn to_operation_mode(self) -> OperationMode {
        match self {
            Self::Move => OperationMode::Move,
            Self::Copy => OperationMode::Copy,
        }
    }
}

impl OrganizeArgs {
    pub(crate) fn writes_library_index(&self) -> bool {
        self.library_index || self.mlip
    }
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct NormalizeLayoutArgs {
    /// 媒体库根目录
    #[arg(long, value_name = "PATH")]
    pub(crate) target: PathBuf,

    /// 只读扫描并生成 plan
    #[arg(long, conflicts_with = "apply_plan")]
    pub(crate) dry_run: bool,

    /// dry-run 输出 JSON plan
    #[arg(long, value_name = "PATH", requires = "dry_run")]
    pub(crate) plan: Option<PathBuf>,

    /// 执行已经审核的固定 plan；daemon job 需要 confirmed=true
    #[arg(long, value_name = "PATH", conflicts_with = "dry_run")]
    pub(crate) apply_plan: Option<PathBuf>,
}

#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CompactArtworkPacksArgs {
    /// 媒体库根目录
    #[arg(long, value_name = "PATH")]
    pub(crate) target: PathBuf,

    /// 只读验证 catalog 并生成 compact plan
    #[arg(long, conflicts_with = "apply_plan")]
    pub(crate) dry_run: bool,

    /// dry-run 输出 JSON plan
    #[arg(long, value_name = "PATH", requires = "dry_run")]
    pub(crate) plan: Option<PathBuf>,

    /// 执行已经审核的 compact plan；daemon job 需要 confirmed=true
    #[arg(long, value_name = "PATH", conflicts_with = "dry_run")]
    pub(crate) apply_plan: Option<PathBuf>,
}

#[cfg(feature = "self-update")]
#[derive(Args, Debug, Clone)]
pub(crate) struct UpdateArgs {
    /// 只查询，不下载、不替换
    #[arg(long)]
    pub(crate) check: bool,

    /// 覆盖更新源。默认使用 GitHub 最新 Release；只允许 https，或本机 http
    #[arg(long, value_name = "URL")]
    pub(crate) feed: Option<String>,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Commands {
    /// 生成或执行媒体库布局统一计划
    NormalizeLayout(NormalizeLayoutArgs),
    /// 验证或压实 MLIP artwork packs
    CompactArtworkPacks(CompactArtworkPacksArgs),
    /// 检查 GitHub Release 并替换当前程序
    #[cfg(feature = "self-update")]
    Update(UpdateArgs),
    #[cfg(feature = "scraper")]
    Scrape(ScrapeArgs),
    #[cfg(feature = "scraper")]
    /// 根据刮削结果生成别名提案
    Match(MatchArgs),
    #[cfg(feature = "scraper")]
    /// 从 Bangumi Archive 构建 SQLite 数据库
    BuildDb(BuildDbArgs),
    #[cfg(feature = "scraper")]
    /// 从 Bangumi dump 中提取别名信息
    ExtractAliases(ExtractAliasesArgs),
    #[cfg(feature = "scraper")]
    /// Merge new aliases from input JSON into database
    MergeAliases(MergeAliasesArgs),
    #[cfg(feature = "scraper")]
    /// Apply confident match proposals to database
    ApplyMatches(ApplyMatchesArgs),
    #[cfg(feature = "scraper")]
    /// Create GitHub issues for uncertain alias match proposals
    CreateAliasIssues(CreateAliasIssuesArgs),
    #[cfg(feature = "clouddrive")]
    /// RSS 订阅管理
    Rss(RssArgs),
    #[cfg(feature = "clouddrive")]
    /// 直接提交 magnet/torrent URL 到 115 网盘离线下载
    AddOffline(AddOfflineArgs),
    #[cfg(feature = "clouddrive")]
    /// 列出云盘目录内容
    ListFolder(ListFolderArgs),
    #[cfg(feature = "torrent-scraper")]
    /// 爬取 DMHY/Nyaa 的番剧种子文件名
    TorrentScrape(TorrentScrapeArgs),
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ScrapeArgs {
    /// 向前回溯的天数
    #[arg(long, default_value_t = 7)]
    #[serde(default = "default_scrape_days")]
    pub(crate) days: u32,

    /// 输出格式
    #[arg(long, value_enum, default_value = "json")]
    pub(crate) format: ScrapeOutputFormat,

    /// TMDB API Key；未传时尝试读取环境变量 TMDB_API_KEY
    #[arg(long, value_name = "KEY")]
    pub(crate) tmdb_api_key: Option<String>,
}

#[cfg(feature = "scraper")]
#[derive(Clone, Copy, Debug, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ScrapeOutputFormat {
    #[default]
    Json,
    Pretty,
}

#[cfg(feature = "scraper")]
fn default_scrape_days() -> u32 {
    7
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MatchArgs {
    /// scrape 子命令生成的 JSON 文件
    #[arg(long, value_name = "PATH")]
    pub(crate) input: PathBuf,

    /// 输出格式
    #[arg(long, value_enum, default_value = "github")]
    pub(crate) format: MatchOutputFormat,
}

#[cfg(feature = "scraper")]
#[derive(Clone, Copy, Debug, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MatchOutputFormat {
    Json,
    #[default]
    Github,
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct BuildDbArgs {
    #[arg(long, value_name = "PATH")]
    pub(crate) output: PathBuf,

    #[arg(long, default_value = "false")]
    pub(crate) include_relations: bool,

    #[arg(long, short, default_value = "false")]
    pub(crate) verbose: bool,
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ExtractAliasesArgs {
    /// 本地 subject.jsonlines 文件路径
    #[arg(long, value_name = "PATH")]
    pub(crate) input: Option<PathBuf>,

    /// 从 Bangumi Archive 下载最新的 dump
    #[arg(long)]
    pub(crate) download: bool,

    /// 输出文件路径（默认stdout）
    #[arg(long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct MergeAliasesArgs {
    /// JSON file containing new aliases to merge
    #[arg(long, value_name = "PATH")]
    pub(crate) input: PathBuf,

    /// Target database file (default: bangumi.db in current directory)
    #[arg(long, value_name = "PATH")]
    pub(crate) target: Option<PathBuf>,
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApplyMatchesArgs {
    /// JSON file containing confident match proposals
    #[arg(long, value_name = "PATH")]
    pub(crate) input: PathBuf,

    /// Target database file (default: bangumi.db in current directory)
    #[arg(long, value_name = "PATH")]
    pub(crate) target: Option<PathBuf>,
}

#[cfg(feature = "scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct CreateAliasIssuesArgs {
    /// JSON file containing uncertain match proposals
    #[arg(long, value_name = "PATH")]
    pub(crate) input: PathBuf,

    /// Repository owner/name (e.g., ModerRAS/anime-organizer)
    #[arg(long, value_name = "REPO")]
    pub(crate) repo: Option<String>,
}

#[cfg(feature = "clouddrive")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct RssArgs {
    /// 持续运行的 Daemon 模式
    #[arg(long)]
    pub(crate) daemon: bool,

    /// 单次执行模式
    #[arg(long)]
    pub(crate) single_shot: bool,

    /// RSS 订阅 URL
    #[arg(long, value_name = "URL")]
    pub(crate) rss_url: Option<String>,

    /// 正则过滤表达式
    #[arg(long, value_name = "REGEX")]
    pub(crate) rss_filter: Option<String>,

    /// 轮询间隔（秒）
    #[arg(long, default_value_t = 300, value_name = "SECS")]
    #[serde(default = "default_rss_interval")]
    pub(crate) rss_interval: u64,

    /// 115网盘目标目录
    #[arg(long, value_name = "PATH")]
    pub(crate) rss_target: Option<String>,

    /// CloudDrive2 服务地址（如 http://localhost:19798）
    #[arg(long, value_name = "URL")]
    pub(crate) clouddrive_url: Option<String>,

    /// CloudDrive2 JWT 令牌（已有令牌时直接使用）
    #[arg(long, value_name = "TOKEN")]
    pub(crate) clouddrive_token: Option<String>,

    /// CloudDrive2 用户名（用于登录获取令牌）
    #[arg(long, value_name = "USER")]
    pub(crate) clouddrive_user: Option<String>,

    /// CloudDrive2 密码
    #[arg(long, value_name = "PASS")]
    pub(crate) clouddrive_pass: Option<String>,

    /// 添加 RSS 订阅到数据库
    #[arg(long)]
    pub(crate) add_subscription: bool,

    /// 列出所有已保存的订阅
    #[arg(long)]
    pub(crate) list_subscriptions: bool,
}

#[cfg(feature = "clouddrive")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct AddOfflineArgs {
    /// magnet 链接或 .torrent 文件 URL
    #[arg(value_name = "MAGNET_OR_URL")]
    pub(crate) url: String,

    /// 115网盘目标目录
    #[arg(long, short = 't', value_name = "PATH")]
    pub(crate) target: String,

    /// CloudDrive2 服务地址
    #[arg(long, value_name = "URL")]
    pub(crate) clouddrive_url: String,

    /// CloudDrive2 JWT 令牌
    #[arg(long, value_name = "TOKEN")]
    pub(crate) clouddrive_token: String,
}

#[cfg(feature = "clouddrive")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ListFolderArgs {
    /// CloudDrive2 服务地址
    #[arg(long, value_name = "URL")]
    pub(crate) clouddrive_url: String,

    /// CloudDrive2 JWT 令牌
    #[arg(long, value_name = "TOKEN")]
    pub(crate) clouddrive_token: String,

    /// 要浏览的目录路径（默认根目录）
    #[arg(value_name = "PATH", default_value = "/")]
    pub(crate) path: String,
}

#[cfg(feature = "torrent-scraper")]
#[derive(Args, Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TorrentScrapeArgs {
    /// 数据来源：dmhy、nyaa 或 all
    #[arg(long, default_value = "all")]
    pub(crate) source: TorrentSource,

    /// Nyaa 搜索关键词（仅用于 nyaa 源）
    #[arg(long, value_name = "KEYWORD")]
    pub(crate) query: Option<String>,

    /// 爬取页数（每页约 75 条）
    #[arg(long, default_value_t = 1)]
    #[serde(default = "default_torrent_pages")]
    pub(crate) pages: u32,

    /// 输出文件路径
    #[arg(long, short = 'o', value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// 兼容旧 Playwright 后端的参数；当前 HTTP 抓取模式会忽略
    #[arg(long)]
    pub(crate) headed: bool,
}

#[cfg(feature = "torrent-scraper")]
#[derive(Clone, Copy, Debug, Default, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TorrentSource {
    Dmhy,
    Nyaa,
    #[default]
    All,
}

#[cfg(feature = "clouddrive")]
fn default_rss_interval() -> u64 {
    300
}

#[cfg(feature = "torrent-scraper")]
fn default_torrent_pages() -> u32 {
    1
}

#[cfg(all(test, feature = "self-update"))]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn replacement_args_keep_daemon_flag() {
        let cli = Cli::try_parse_from([
            "aniorg",
            "--replace-from",
            "aniorg.new",
            "--replace-to",
            "aniorg",
            "--wait-for-parent",
            "--restart-arg",
            "--daemon",
        ])
        .expect("parse replacement args");
        assert!(cli.wait_for_parent);
        assert_eq!(cli.restart_arg, vec!["--daemon".to_string()]);
        assert!(cli.command.is_none());
    }

    #[test]
    fn update_check_is_a_subcommand() {
        let cli = Cli::try_parse_from(["aniorg", "update", "--check"]).expect("parse update");
        match cli.command {
            Some(Commands::Update(args)) => assert!(args.check),
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
