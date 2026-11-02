//! 文件整理模块
//!
//! 该模块负责将解析后的动漫文件整理到目标目录。
//!
//! # 操作模式
//!
//! - **Move**: 移动文件到目标目录
//! - **Copy**: 复制文件到目标目录
//! - **Link**: 创建硬链接（零额外空间占用）
//!
//! # 示例
//!
//! ```no_run
//! use anime_organizer::organizer::{FileOrganizer, OperationMode};
//! use anime_organizer::parser::AnimeFileInfo;
//!
//! let info = AnimeFileInfo {
//!     publisher: "ANi".to_string(),
//!     anime_name: "测试".to_string(),
//!     episode: "01".to_string(),
//!     tags: "[1080P]".to_string(),
//!     extension: ".mp4".to_string(),
//!     original_path: "/downloads/[ANi] 测试 - 01 [1080P].mp4".to_string(),
//! };
//!
//! let result = FileOrganizer::organize(&info, "/anime", OperationMode::Copy, false, false);
//! ```

use crate::error::{AppError, Result};
use crate::parser::AnimeFileInfo;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const FILE_COMPARE_BUFFER_SIZE: usize = 1024 * 1024;

/// 文件操作模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationMode {
    /// 移动文件
    #[value(name = "move")]
    Move,
    /// 复制文件
    #[value(name = "copy")]
    Copy,
    /// 创建硬链接
    #[value(name = "link")]
    #[default]
    Link,
}

impl std::fmt::Display for OperationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Move => write!(f, "move"),
            Self::Copy => write!(f, "copy"),
            Self::Link => write!(f, "link"),
        }
    }
}

/// 文件整理器
///
/// 提供文件整理的静态方法。
pub struct FileOrganizer;

impl FileOrganizer {
    /// 整理单个动漫文件
    ///
    /// 根据指定的操作模式将文件整理到目标目录。
    ///
    /// # 参数
    ///
    /// * `anime_file` - 动漫文件信息
    /// * `target_root` - 目标根目录
    /// * `mode` - 操作模式
    /// * `dry_run` - 是否为预览模式
    /// * `season_mode` - 是否使用 Kodi 两层目录结构（`番名/Season N/`）
    ///
    /// # 返回值
    ///
    /// 成功返回 `Ok(())`，失败返回相应的错误。
    ///
    /// # 错误
    ///
    /// - `AppError::Io` - IO 操作失败
    /// - `AppError::CrossDeviceLink` - 硬链接跨设备
    /// - `AppError::HardLinkNotSupported` - 硬链接不支持
    ///
    /// # 示例
    ///
    /// ```no_run
    /// use anime_organizer::organizer::{FileOrganizer, OperationMode};
    /// use anime_organizer::parser::AnimeFileInfo;
    ///
    /// let info = AnimeFileInfo {
    ///     publisher: "ANi".to_string(),
    ///     anime_name: "测试".to_string(),
    ///     episode: "01".to_string(),
    ///     tags: "[1080P]".to_string(),
    ///     extension: ".mp4".to_string(),
    ///     original_path: "/downloads/[ANi] 测试 - 01 [1080P].mp4".to_string(),
    /// };
    ///
    /// // 常规模式：`/anime/测试/[ANi] 测试 - 01 [1080P].mp4`
    /// FileOrganizer::organize(&info, "/anime", OperationMode::Copy, false, false)?;
    ///
    /// // Kodi 两层目录模式：`/anime/测试/Season 1/[ANi] 测试 - 01 [1080P].mp4`
    /// FileOrganizer::organize(&info, "/anime", OperationMode::Copy, false, true)?;
    /// # Ok::<(), anime_organizer::error::AppError>(())
    /// ```
    pub fn organize<P: AsRef<Path>>(
        anime_file: &AnimeFileInfo,
        target_root: P,
        mode: OperationMode,
        dry_run: bool,
        season_mode: bool,
    ) -> Result<()> {
        let target_dir = if season_mode {
            target_root
                .as_ref()
                .join(anime_file.series_name())
                .join(anime_file.season_dir_name())
        } else {
            target_root.as_ref().join(&anime_file.anime_name)
        };
        Self::organize_to_dir(anime_file, target_dir, mode, dry_run).map(|_| ())
    }

    /// 将文件整理到指定目录，不额外附加动画名称目录。
    pub fn organize_to_dir<P: AsRef<Path>>(
        anime_file: &AnimeFileInfo,
        target_dir: P,
        mode: OperationMode,
        dry_run: bool,
    ) -> Result<PathBuf> {
        let subtitles = Self::find_external_subtitles(Path::new(&anime_file.original_path));
        Self::organize_to_dir_with_subtitles(anime_file, target_dir, mode, dry_run, &subtitles)
    }

    /// 使用已发现的字幕整理文件，供批处理避免重复扫描源目录。
    pub fn organize_to_dir_with_subtitles<P: AsRef<Path>>(
        anime_file: &AnimeFileInfo,
        target_dir: P,
        mode: OperationMode,
        dry_run: bool,
        subtitle_paths: &[PathBuf],
    ) -> Result<PathBuf> {
        let target_dir = target_dir.as_ref();
        let source_path = Path::new(&anime_file.original_path);
        let target_filename = source_path
            .file_name()
            .ok_or_else(|| AppError::FileOperation {
                path: source_path.to_path_buf(),
                message: "源路径缺少文件名".to_string(),
            })?;
        let target_path = target_dir.join(target_filename);

        if dry_run {
            println!(
                "[DRY-RUN] {} -> {}",
                anime_file.original_path,
                target_path.display()
            );
            for subtitle_path in subtitle_paths {
                println!(
                    "[DRY-RUN] {} -> {}",
                    subtitle_path.display(),
                    Self::subtitle_target_path(source_path, subtitle_path, &target_path).display(),
                );
            }
            return Ok(target_path);
        }

        fs::create_dir_all(target_dir)?;
        Self::organize_path(source_path, &target_path, mode)?;
        for subtitle_path in subtitle_paths {
            let subtitle_target =
                Self::subtitle_target_path(source_path, subtitle_path, &target_path);
            Self::organize_path(subtitle_path, &subtitle_target, mode)?;
        }

        Ok(target_path)
    }

    /// 把已经解压到临时目录的字幕或字体放进目标位置。
    ///
    /// 临时文件会被多集共用，因此这里只复制或硬链接，不删除源文件。
    /// 硬链接失败时改为复制。目标若是符号链接，先删除链接本身再写入。
    pub fn place_extracted_asset(
        source: &Path,
        target: &Path,
        mode: OperationMode,
        dry_run: bool,
    ) -> Result<()> {
        if dry_run {
            println!("[DRY-RUN] {} -> {}", source.display(), target.display());
            return Ok(());
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Ok(metadata) = fs::symlink_metadata(target) {
            if metadata.file_type().is_symlink() {
                fs::remove_file(target)?;
            } else if metadata.is_file() {
                if Self::files_match(source, target)? {
                    return Ok(());
                }
                fs::remove_file(target)?;
            } else {
                return Err(AppError::FileOperation {
                    path: target.to_path_buf(),
                    message: "提取资产的目标不是普通文件".to_string(),
                });
            }
        }
        match mode {
            OperationMode::Link => {
                if Self::create_hard_link(source, target).is_err() {
                    fs::copy(source, target)?;
                }
            }
            OperationMode::Copy | OperationMode::Move => {
                fs::copy(source, target)?;
            }
        }
        Ok(())
    }

    /// 从最深层开始删除根目录下的空目录，保留根目录本身。
    pub fn cleanup_empty_directories<P: AsRef<Path>>(
        root: P,
        dry_run: bool,
    ) -> Result<Vec<PathBuf>> {
        let root = root.as_ref();
        if !root.is_dir() {
            return Err(AppError::SourceNotFound(root.to_path_buf()));
        }
        let mut directories = WalkDir::new(root)
            .min_depth(1)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_dir())
            .map(|entry| entry.into_path())
            .collect::<Vec<_>>();
        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));

        let mut removed = Vec::new();
        for directory in directories {
            let is_empty = fs::read_dir(&directory)?.next().is_none();
            if is_empty {
                if !dry_run {
                    fs::remove_dir(&directory)?;
                }
                removed.push(directory);
            }
        }
        Ok(removed)
    }

    /// 删除指定目录及其空父目录，最多清理到根目录但不删除根目录本身。
    pub fn cleanup_empty_ancestors<P: AsRef<Path>, Q: AsRef<Path>>(
        directory: P,
        root: Q,
    ) -> Result<Vec<PathBuf>> {
        let root = root.as_ref();
        let mut current = directory.as_ref().to_path_buf();
        let mut removed = Vec::new();
        while current != root && current.starts_with(root) {
            if !current.is_dir() || fs::read_dir(&current)?.next().is_some() {
                break;
            }
            fs::remove_dir(&current)?;
            removed.push(current.clone());
            let Some(parent) = current.parent() else {
                break;
            };
            current = parent.to_path_buf();
        }
        Ok(removed)
    }

    /// 扫描一次批处理根目录中的所有外部字幕候选。
    ///
    /// 单文件任务（例如 qBittorrent 完成钩子）从文件的父目录扫描，
    /// 以便发现同级或字幕子目录中的外挂字幕。
    pub fn collect_external_subtitle_candidates(root: &Path) -> Vec<PathBuf> {
        let search_root = if root.is_dir() {
            root
        } else {
            root.parent().unwrap_or(root)
        };
        WalkDir::new(search_root)
            .min_depth(1)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .map(|entry| entry.into_path())
            .filter(|path| Self::is_external_subtitle(path))
            .collect()
    }

    /// 查找视频附近三层目录内同名或带语言后缀的外部字幕。
    pub fn find_external_subtitles(video_path: &Path) -> Vec<PathBuf> {
        let Some(directory) = video_path.parent() else {
            return Vec::new();
        };
        let candidates = Self::collect_external_subtitle_candidates(directory);
        Self::find_external_subtitles_from(video_path, &candidates)
    }

    /// 从批处理候选集中查找视频对应字幕。
    pub fn find_external_subtitles_from(video_path: &Path, candidates: &[PathBuf]) -> Vec<PathBuf> {
        let Some(directory) = video_path.parent() else {
            return Vec::new();
        };
        let Some(video_stem) = video_path.file_stem().and_then(|value| value.to_str()) else {
            return Vec::new();
        };
        let video_stem_lower = video_stem.to_lowercase();
        let mut paths = candidates
            .iter()
            .filter(|path| {
                path.strip_prefix(directory)
                    .is_ok_and(|relative| relative.components().count() <= 3)
            })
            .filter(|path| {
                let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                    return false;
                };
                let stem_lower = stem.to_lowercase();
                let Some(suffix) = stem_lower.strip_prefix(&video_stem_lower) else {
                    return false;
                };
                suffix.is_empty() || suffix.starts_with(['.', ' ', '_', '-', '[', '(', '（'])
            })
            .cloned()
            .collect::<Vec<_>>();
        paths.sort_by_key(|path| path.to_string_lossy().to_lowercase());
        paths
    }

    fn is_external_subtitle(path: &Path) -> bool {
        matches!(
            path.extension()
                .and_then(|value| value.to_str())
                .map(|value| value.to_ascii_lowercase())
                .as_deref(),
            Some("srt" | "ass" | "ssa" | "vtt")
        )
    }

    fn subtitle_target_path(
        source_video: &Path,
        source_subtitle: &Path,
        target_video: &Path,
    ) -> PathBuf {
        let source_stem = source_video
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let subtitle_stem = source_subtitle
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let suffix = subtitle_stem.get(source_stem.len()..).unwrap_or_default();
        let target_stem = target_video
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        let extension = source_subtitle
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        target_video.with_file_name(format!("{target_stem}{suffix}.{extension}"))
    }

    fn organize_path(source_path: &Path, target_path: &Path, mode: OperationMode) -> Result<()> {
        if source_path == target_path
            || target_path.exists()
                && fs::canonicalize(source_path)? == fs::canonicalize(target_path)?
        {
            return Ok(());
        }
        if target_path.exists() {
            if Self::files_match(source_path, target_path)? {
                if mode == OperationMode::Move {
                    fs::remove_file(source_path)?;
                }
                return Ok(());
            }
            fs::remove_file(target_path)?;
        }
        match mode {
            OperationMode::Move => {
                if fs::rename(source_path, target_path).is_err() {
                    fs::copy(source_path, target_path)?;
                    fs::remove_file(source_path)?;
                }
            }
            OperationMode::Copy => {
                fs::copy(source_path, target_path)?;
            }
            OperationMode::Link => Self::create_hard_link(source_path, target_path)?,
        }
        Ok(())
    }

    fn files_match(source_path: &Path, target_path: &Path) -> std::io::Result<bool> {
        let source_metadata = fs::metadata(source_path)?;
        let target_metadata = fs::metadata(target_path)?;
        let source_len = source_metadata.len();
        if source_len != target_metadata.len() {
            return Ok(false);
        }

        let mut remaining = source_len;
        let mut source = fs::File::open(source_path)?;
        let mut target = fs::File::open(target_path)?;
        let mut source_buffer = vec![0; FILE_COMPARE_BUFFER_SIZE];
        let mut target_buffer = vec![0; FILE_COMPARE_BUFFER_SIZE];
        while remaining > 0 {
            let chunk = remaining.min(FILE_COMPARE_BUFFER_SIZE as u64) as usize;
            source.read_exact(&mut source_buffer[..chunk])?;
            target.read_exact(&mut target_buffer[..chunk])?;
            if source_buffer[..chunk] != target_buffer[..chunk] {
                return Ok(false);
            }
            remaining -= chunk as u64;
        }
        Ok(true)
    }

    /// 创建硬链接
    ///
    /// 在不同平台上创建硬链接。
    ///
    /// # 参数
    ///
    /// * `source` - 源文件路径
    /// * `target` - 目标路径
    ///
    /// # 错误
    ///
    /// - `AppError::CrossDeviceLink` - 源和目标不在同一文件系统
    /// - `AppError::HardLinkNotSupported` - 当前系统不支持硬链接
    fn create_hard_link<P: AsRef<Path>, Q: AsRef<Path>>(source: P, target: Q) -> Result<()> {
        let result = fs::hard_link(source.as_ref(), target.as_ref());

        match result {
            Ok(()) => Ok(()),
            Err(e) => {
                // 检查是否为跨设备错误
                let error_code = e.raw_os_error();

                // Windows: ERROR_NOT_SAME_DEVICE = 17
                // Unix: EXDEV = 18
                if error_code == Some(17) || error_code == Some(18) {
                    Err(AppError::CrossDeviceLink)
                } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                    Err(AppError::HardLinkNotSupported)
                } else {
                    Err(AppError::Io(e))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, filename: &str, content: &str) -> std::path::PathBuf {
        let file_path = dir.join(filename);
        let mut file = fs::File::create(&file_path).unwrap();
        file.write_all(content.as_bytes()).unwrap();
        file_path
    }

    fn create_test_anime_info(source_path: &Path) -> AnimeFileInfo {
        AnimeFileInfo {
            publisher: "ANi".to_string(),
            anime_name: "测试".to_string(),
            episode: "01".to_string(),
            tags: "[1080P]".to_string(),
            extension: ".mp4".to_string(),
            original_path: source_path.to_string_lossy().to_string(),
        }
    }

    #[test]
    fn test_organize_move_mode() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = create_test_anime_info(&source_file);

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Move,
            false,
            false,
        );

        assert!(result.is_ok());
        let expected_path = target_dir.path().join("测试").join("test.mp4");
        assert!(expected_path.exists());
        assert!(!source_file.exists());
        assert_eq!(fs::read_to_string(&expected_path).unwrap(), "test content");
    }

    #[test]
    fn move_removes_source_when_matching_target_exists() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();
        let source_file = create_test_file(source_dir.path(), "test.mp4", "same content");
        let target_anime_dir = target_dir.path().join("测试");
        fs::create_dir(&target_anime_dir).unwrap();
        let target_file = create_test_file(&target_anime_dir, "test.mp4", "same content");
        let anime_info = create_test_anime_info(&source_file);

        FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Move,
            false,
            false,
        )
        .unwrap();

        assert!(!source_file.exists());
        assert_eq!(fs::read_to_string(target_file).unwrap(), "same content");
    }

    #[test]
    fn test_organize_copy_mode() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = create_test_anime_info(&source_file);

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Copy,
            false,
            false,
        );

        assert!(result.is_ok());
        let expected_path = target_dir.path().join("测试").join("test.mp4");
        assert!(expected_path.exists());
        assert!(source_file.exists());
        assert_eq!(fs::read_to_string(&expected_path).unwrap(), "test content");
        assert_eq!(fs::read_to_string(&source_file).unwrap(), "test content");
    }

    #[test]
    fn existing_file_match_checks_the_entire_file() {
        let directory = TempDir::new().unwrap();
        let source_path = directory.path().join("source.mkv");
        let target_path = directory.path().join("target.mkv");
        let file_len = FILE_COMPARE_BUFFER_SIZE as u64 + 1;
        let source = fs::File::create(&source_path).unwrap();
        let mut target = fs::File::create(&target_path).unwrap();
        source.set_len(file_len).unwrap();
        target.set_len(file_len).unwrap();

        assert!(FileOrganizer::files_match(&source_path, &target_path).unwrap());

        target.seek(SeekFrom::Start(file_len - 1)).unwrap();
        target.write_all(b"y").unwrap();
        assert!(!FileOrganizer::files_match(&source_path, &target_path).unwrap());

        let mut source = fs::OpenOptions::new()
            .write(true)
            .open(&source_path)
            .unwrap();
        source.seek(SeekFrom::Start(file_len - 1)).unwrap();
        source.write_all(b"y").unwrap();
        assert!(FileOrganizer::files_match(&source_path, &target_path).unwrap());
    }

    #[test]
    fn test_organize_copy_mode_includes_matching_external_subtitles() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();
        let source_file = create_test_file(source_dir.path(), "test.mp4", "video");
        create_test_file(source_dir.path(), "test.zh-CN.ass", "subtitle");
        let subtitle_dir = source_dir.path().join("Subs");
        fs::create_dir(&subtitle_dir).unwrap();
        create_test_file(&subtitle_dir, "test(zh-TW).srt", "nested subtitle");
        create_test_file(source_dir.path(), "test10.srt", "other episode");
        create_test_file(source_dir.path(), "test.sub", "unsupported vobsub");
        let anime_info = create_test_anime_info(&source_file);

        FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Copy,
            false,
            false,
        )
        .unwrap();

        let anime_dir = target_dir.path().join("测试");
        assert_eq!(
            fs::read_to_string(anime_dir.join("test.zh-CN.ass")).unwrap(),
            "subtitle",
        );
        assert_eq!(
            fs::read_to_string(anime_dir.join("test(zh-TW).srt")).unwrap(),
            "nested subtitle",
        );
        assert!(!anime_dir.join("test10.srt").exists());
        assert!(!anime_dir.join("test.sub").exists());
    }

    #[test]
    fn test_organize_dry_run_does_not_modify_files() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = create_test_anime_info(&source_file);

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Move,
            true,
            false,
        );

        assert!(result.is_ok());
        assert!(source_file.exists());
        assert!(!target_dir.path().join("测试").exists());
    }

    #[test]
    fn test_organize_creates_target_directory() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();
        let nested_target = target_dir.path().join("sub").join("dir");

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = create_test_anime_info(&source_file);

        let result = FileOrganizer::organize(
            &anime_info,
            &nested_target,
            OperationMode::Copy,
            false,
            false,
        );

        assert!(result.is_ok());
        assert!(nested_target.join("测试").exists());
    }

    #[test]
    fn test_organize_overwrites_existing_file() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "new content");
        let anime_info = create_test_anime_info(&source_file);

        // 创建已存在的目标文件
        let target_anime_dir = target_dir.path().join("测试");
        fs::create_dir_all(&target_anime_dir).unwrap();
        create_test_file(&target_anime_dir, "test.mp4", "old content");

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Copy,
            false,
            false,
        );

        assert!(result.is_ok());
        let expected_path = target_dir.path().join("测试").join("test.mp4");
        assert_eq!(fs::read_to_string(&expected_path).unwrap(), "new content");
    }

    #[test]
    fn test_organize_to_same_path_keeps_file() {
        let directory = TempDir::new().unwrap();
        let source_file = create_test_file(directory.path(), "test.mp4", "test content");
        let anime_info = create_test_anime_info(&source_file);

        let target_path = FileOrganizer::organize_to_dir(
            &anime_info,
            directory.path(),
            OperationMode::Copy,
            false,
        )
        .unwrap();

        assert_eq!(target_path, source_file);
        assert_eq!(fs::read_to_string(target_path).unwrap(), "test content");
    }

    #[test]
    fn cleanup_empty_directories_removes_only_empty_descendants() {
        let root = TempDir::new().unwrap();
        let empty_leaf = root.path().join("empty").join("leaf");
        let occupied = root.path().join("occupied");
        fs::create_dir_all(&empty_leaf).unwrap();
        fs::create_dir_all(&occupied).unwrap();
        create_test_file(&occupied, "keep.txt", "keep");

        let removed = FileOrganizer::cleanup_empty_directories(root.path(), false).unwrap();

        assert_eq!(removed.len(), 2);
        assert!(!root.path().join("empty").exists());
        assert!(occupied.exists());
        assert!(root.path().exists());
    }

    #[test]
    fn cleanup_empty_directories_dry_run_keeps_directories() {
        let root = TempDir::new().unwrap();
        let empty = root.path().join("empty");
        fs::create_dir(&empty).unwrap();

        let removed = FileOrganizer::cleanup_empty_directories(root.path(), true).unwrap();

        assert_eq!(removed, vec![empty.clone()]);
        assert!(empty.exists());
    }

    #[test]
    fn test_operation_mode_display() {
        assert_eq!(format!("{}", OperationMode::Move), "move");
        assert_eq!(format!("{}", OperationMode::Copy), "copy");
        assert_eq!(format!("{}", OperationMode::Link), "link");
    }

    #[test]
    fn test_operation_mode_default() {
        assert_eq!(OperationMode::default(), OperationMode::Link);
    }

    #[test]
    fn test_organize_to_dir_uses_exact_target_dir() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();
        let season_dir = target_dir.path().join("测试").join("Season 2");

        let filename = "[ANi] Test Anime Season 2 - 01 [1080P].mp4";
        let source_file = create_test_file(source_dir.path(), filename, "test content");
        let anime_info = create_test_anime_info(&source_file);

        let target_path =
            FileOrganizer::organize_to_dir(&anime_info, &season_dir, OperationMode::Copy, false)
                .unwrap();

        assert_eq!(target_path, season_dir.join(filename));
        assert!(target_path.exists());
    }

    #[test]
    fn test_organize_season_mode_true() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = AnimeFileInfo {
            publisher: "ANi".to_string(),
            anime_name: "Test Anime 第2季".to_string(),
            episode: "01".to_string(),
            tags: "[1080P]".to_string(),
            extension: ".mp4".to_string(),
            original_path: source_file.to_string_lossy().to_string(),
        };

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Copy,
            false,
            true,
        );

        assert!(result.is_ok());
        let expected_path = target_dir
            .path()
            .join("Test Anime")
            .join("Season 2")
            .join("test.mp4");
        assert!(expected_path.exists());
    }

    #[test]
    fn test_organize_season_mode_false_backward_compatible() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = AnimeFileInfo {
            publisher: "ANi".to_string(),
            anime_name: "Test Anime 第2季".to_string(),
            episode: "01".to_string(),
            tags: "[1080P]".to_string(),
            extension: ".mp4".to_string(),
            original_path: source_file.to_string_lossy().to_string(),
        };

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Copy,
            false,
            false,
        );

        assert!(result.is_ok());
        let expected_path = target_dir.path().join("Test Anime 第2季").join("test.mp4");
        assert!(expected_path.exists());
    }

    #[test]
    fn test_organize_season_mode_numeric_suffix() {
        let source_dir = TempDir::new().unwrap();
        let target_dir = TempDir::new().unwrap();

        let source_file = create_test_file(source_dir.path(), "test.mp4", "test content");
        let anime_info = AnimeFileInfo {
            publisher: "ANi".to_string(),
            anime_name: "異世界悠閒農家 2".to_string(),
            episode: "03".to_string(),
            tags: "[1080P]".to_string(),
            extension: ".mp4".to_string(),
            original_path: source_file.to_string_lossy().to_string(),
        };

        let result = FileOrganizer::organize(
            &anime_info,
            target_dir.path(),
            OperationMode::Copy,
            false,
            true,
        );

        assert!(result.is_ok());
        let expected_path = target_dir
            .path()
            .join("異世界悠閒農家")
            .join("Season 2")
            .join("test.mp4");
        assert!(expected_path.exists());
    }
}
