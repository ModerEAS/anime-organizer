//! 错误处理模块
//!
//! 定义了 `anime-organizer` 中使用的错误类型。

use std::path::PathBuf;
use thiserror::Error;

/// 应用程序错误类型
///
/// 包含所有可能在文件解析和整理过程中发生的错误。
#[derive(Error, Debug)]
pub enum AppError {
    /// IO 操作错误
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 文件名解析失败
    #[error("无法解析文件名: {0}")]
    ParseError(String),

    /// 源目录不存在
    #[error("源目录不存在: {0}")]
    SourceNotFound(PathBuf),

    /// 目标目录不存在
    #[error("目标目录不存在: {0}")]
    TargetNotFound(PathBuf),

    /// 硬链接跨设备错误
    #[error("硬链接失败：源文件和目标必须在同一文件系统")]
    CrossDeviceLink,

    /// 硬链接不支持
    #[error("当前系统不支持硬链接")]
    HardLinkNotSupported,

    /// 文件操作失败
    #[error("文件操作失败 {path}: {message}")]
    FileOperation {
        /// 操作的文件路径
        path: PathBuf,
        /// 错误信息
        message: String,
    },

    /// 元数据获取失败
    #[error("元数据获取失败: {0}")]
    MetadataFetchError(String),

    /// NFO 生成失败
    #[error("NFO 生成失败: {0}")]
    NfoGenerationError(String),

    /// 媒体库索引生成失败
    #[error("媒体库索引生成失败: {0}")]
    LibraryIndexError(String),

    /// 图片下载失败
    #[error("图片下载失败: {0}")]
    ImageDownloadError(String),

    /// 别名库加载失败
    #[error("别名库加载失败: {0}")]
    AliasLoadError(String),

    /// 无法匹配动画
    #[error("无法匹配动画: {0}")]
    AnimeNotFoundError(String),

    /// Bangumi Dump 解析错误
    #[error("Bangumi Dump 解析错误: {0}")]
    BangumiParseError(String),

    /// Wiki Infobox 解析错误
    #[error("Wiki Infobox 解析错误: {0}")]
    WikiParseError(String),

    /// TMDB API 错误
    #[error("TMDB API 错误: {0}")]
    TmdbApiError(String),

    /// 操作由 daemon 用户请求取消
    #[error("操作已取消")]
    Canceled,

    /// 程序自身更新失败
    #[error("程序更新失败: {0}")]
    SelfUpdate(String),

    /// Torrent 页面获取失败
    #[error("Torrent 获取失败: {0}")]
    TorrentFetchError(String),

    /// Torrent 响应解析失败
    #[error("Torrent 解析失败: {0}")]
    TorrentParseError(String),
}

/// 应用程序结果类型别名
pub type Result<T> = std::result::Result<T, AppError>;
