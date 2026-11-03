#![allow(clippy::result_large_err)]

//! # anime-organizer
//!
//! 轻量级、跨平台命令行工具，专为动漫收藏者批量整理视频文件。
//!
//! ## 功能特性
//!
//! - **智能解析**: 自动识别 `[发布组] 动漫名 - 集数 [标签].ext` 格式
//! - **灵活整理**: 重构为 `动漫名/集数 [标签].ext` 结构
//! - **多种模式**: 支持移动、复制、硬链接三种操作模式
//! - **跨平台**: 支持 Windows、Linux、macOS
//! - **零依赖运行**: 单文件部署，无需外部配置
//!
//! ## 快速开始
//!
//! ```bash
//! # 硬链接模式（默认，零额外空间）
//! aniorg --source="/path/to/downloads"
//!
//! # 指定目标目录
//! aniorg --source="/path/to/downloads" --mode=link --target="/path/to/anime"
//!
//! # 启用硬链接失败时的回退
//! aniorg --source="/path/to/downloads" --fallback-on-link-failure=copy
//!
//! # 预览模式
//! aniorg --source="/path/to/downloads" --dry-run --verbose
//! ```
//!
//! ## 模块结构
//!
//! - [`parser`] - 文件名解析模块
//! - [`organizer`] - 文件整理模块
//! - [`error`] - 错误处理模块
/// - [`metadata`] - 元数据模块（Bangumi 元数据，TMDB/AniDB 可选图片补充）
/// - [`nfo`] - NFO 文件生成模块
/// - `scraper` - 数据源刮削模块（需 `scraper` feature，参考 `cargo doc --features scraper`）
#[cfg(feature = "anifilebert")]
pub mod anifilebert;
pub mod archive_assets;
mod artwork_pack;
pub mod error;
pub mod layout_normalizer;
pub mod library_index;
pub mod metadata;
pub mod nfo;
pub mod organize_plan;
pub mod organizer;
pub mod parser;
#[cfg(feature = "clouddrive")]
pub mod rss;
#[cfg(feature = "scraper")]
pub mod scraper;
#[cfg(feature = "torrent-scraper")]
pub mod torrent;

pub use artwork_pack::{
    apply_artwork_compact_plan, build_artwork_compact_plan, sha256_file,
    ArtworkCompactApplySummary, ArtworkCompactPlan, ArtworkCompactStats,
};
pub use error::{AppError, Result};
pub use layout_normalizer::{
    apply_layout_plan, build_layout_plan, build_layout_plan_with_cancel, LayoutApplySummary,
    LayoutPlan, LayoutPlanSummary,
};
pub use library_index::{ExtraKind, LibraryExtraRecord, LibraryIndex, LibraryIndexRecord};
pub use metadata::AnimeMetadata;
pub use nfo::{EpisodeNfo, NfoWriter, TvShowNfo};
pub use organize_plan::organize_directory_components;
pub use organizer::{FileOrganizer, OperationMode};
pub use parser::{AnimeFileInfo, FilenameParser};
