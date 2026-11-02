//! 安全解压字幕/字体压缩包。
//!
//! 只把白名单扩展名写到调用方提供的目录，拒绝路径穿越、符号链接和压缩炸弹。
//! 解压使用 `zip`、`sevenz-rust2` 和 `rar-codec`，不调用 shell。
//!
//! CloudDrive2 的 proto 只有未挂接 RPC 的 `UnmountArchiveResult`，没有服务端解压接口，
//! 远端整理因此先把压缩包下载到本地临时目录，再按同样的规则提取。

use crate::error::{AppError, Result};
use rar_codec::EntryKind;
use sevenz_rust2::{Password, SevenZReader};
use std::cell::Cell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use zip::ZipArchive;

const ARCHIVE_EXTENSIONS: &[&str] = &["7z", "zip", "rar"];
const SUBTITLE_EXTENSIONS: &[&str] = &["ass", "ssa", "srt", "sup", "sub", "idx", "vtt"];
const FONT_EXTENSIONS: &[&str] = &["ttf", "otf", "ttc"];
const BLOCKED_EXTENSIONS: &[&str] = &[
    "exe", "dll", "scr", "bat", "cmd", "ps1", "js", "vbs", "lnk", "url", "com", "msi", "jar", "sh",
    "hta", "pif", "cpl", "msc", "reg", "wsf", "jse", "vbe", "psd1", "psm1", "app", "command",
];

/// 压缩包本体超过该大小时直接拒绝，避免把整包读进内存。
pub const MAX_ARCHIVE_FILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// 解压上限。默认拒绝压缩比超过 100、解压总量超过 1 GiB、单文件超过 128 MiB
/// 或条目数超过 10_000 的压缩包。
#[derive(Debug, Clone, Copy)]
pub struct ArchiveLimits {
    pub max_entries: usize,
    pub max_entry_bytes: u64,
    pub max_total_bytes: u64,
    pub max_ratio: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            max_entries: 10_000,
            max_entry_bytes: 128 * 1024 * 1024,
            max_total_bytes: 1024 * 1024 * 1024,
            max_ratio: 100,
        }
    }
}

/// 从压缩包里保留下来的资产种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// 外挂字幕。
    Subtitle,
    /// 字体。
    Font,
}

/// 已经写入临时目录的白名单文件。
#[derive(Debug, Clone)]
pub struct ExtractedAsset {
    /// 压缩包内的安全相对路径，使用 `/` 分隔。
    pub relative_path: String,
    /// 临时目录中的绝对路径。
    pub absolute_path: PathBuf,
    /// 字幕或字体。
    pub kind: AssetKind,
}

impl ExtractedAsset {
    /// 返回相对路径的文件名。
    #[must_use]
    pub fn file_name(&self) -> &str {
        self.relative_path
            .rsplit(['/', '\\'])
            .find(|part| !part.is_empty())
            .unwrap_or(self.relative_path.as_str())
    }
}

/// 一次成功解压的结果。被拒绝的压缩包通过 [`Err`] 返回，不会留下已写文件。
#[derive(Debug)]
pub struct ExtractOutcome {
    /// 写入目标目录的白名单文件。
    pub assets: Vec<ExtractedAsset>,
    /// 跳过的可执行文件或脚本。
    pub warnings: Vec<String>,
}

/// 准备阶段的选项。
#[derive(Debug, Clone, Copy, Default)]
pub struct IngestOptions {
    /// 体积、数量和压缩比上限。
    pub limits: ArchiveLimits,
    /// 为 true 时，在 Windows 上调用 Defender 扫描临时目录。
    pub scan_with_defender: bool,
}

/// Defender 扫描结果。
#[derive(Debug)]
pub enum DefenderScan {
    /// 扫描通过。
    Clean,
    /// 当前系统没有 Windows Defender。
    Unsupported,
    /// 扫描器缺失、执行失败或报告威胁。
    Failed(String),
}

/// 一个源压缩包及其提取结果。临时目录在本结构丢弃前一直保留。
pub struct PreparedArchive {
    /// 压缩包原路径。
    pub source: PathBuf,
    /// 压缩包所在目录，用来和同目录剧集配对。
    pub directory: PathBuf,
    /// 提取出的资产。拒绝时为空。
    pub assets: Vec<ExtractedAsset>,
    /// 拒绝原因或被跳过的危险条目。
    pub warnings: Vec<String>,
    /// 压缩包被整体拒绝。
    pub rejected: bool,
    consumed: Cell<bool>,
    _temp: Option<tempfile::TempDir>,
}

impl PreparedArchive {
    /// 是否已经有资产被放进目标目录。
    #[must_use]
    pub fn consumed(&self) -> bool {
        self.consumed.get()
    }

    /// 记录这个压缩包的资产已经被使用，允许按选项删除压缩包本体。
    pub fn mark_consumed(&self) {
        self.consumed.set(true);
    }

    /// 属于这部视频的字幕。
    #[must_use]
    pub fn subtitles_for(&self, video_name: &str) -> Vec<&ExtractedAsset> {
        self.assets
            .iter()
            .filter(|asset| {
                asset.kind == AssetKind::Subtitle && subtitle_matches(video_name, asset.file_name())
            })
            .collect()
    }

    /// 应该随这部视频入库的字体。
    #[must_use]
    pub fn fonts_for(&self, video_name: &str) -> Vec<&ExtractedAsset> {
        let Some(archive_name) = self.source.file_name().and_then(|name| name.to_str()) else {
            return Vec::new();
        };
        if !font_applies(archive_name, video_name) {
            return Vec::new();
        }
        self.assets
            .iter()
            .filter(|asset| asset.kind == AssetKind::Font)
            .collect()
    }
}

#[derive(Clone)]
struct EntryMeta {
    name: String,
    uncompressed: u64,
    compressed: u64,
    link: bool,
    anti: bool,
}

/// 路径是不是字幕/字体压缩包。
#[must_use]
pub fn is_asset_archive(path: impl AsRef<Path>) -> bool {
    extension_of(path.as_ref())
        .is_some_and(|extension| ARCHIVE_EXTENSIONS.contains(&extension.as_str()))
}

/// 远程路径是不是字幕/字体压缩包。
#[must_use]
pub fn is_asset_archive_name(name: &str) -> bool {
    is_asset_archive(Path::new(name))
}

/// 字幕文件名是否属于这部视频。
///
/// 先用与外挂字幕相同的词干前缀规则，再用 ` - 集数` 加标题重叠兜底，
/// 这样 `Subtitles.7z` 里的 `[组] 片名 - 01.chs.ass` 能配上带分辨率标签的视频。
#[must_use]
pub fn subtitle_matches(video_name: &str, subtitle_name: &str) -> bool {
    if stem_prefix_matches(video_name, subtitle_name) {
        return true;
    }
    match (episode_token(video_name), episode_token(subtitle_name)) {
        (Some(video), Some(subtitle)) if video == subtitle => {
            titles_overlap(video_name, subtitle_name)
        }
        _ => false,
    }
}

/// 压缩包里的字体是否应跟着这部视频走。
///
/// 压缩包文件名自身带集数时只给那一集；`Fonts.7z` 这类整包字体给同目录全部剧集。
#[must_use]
pub fn font_applies(archive_name: &str, video_name: &str) -> bool {
    match episode_token(archive_name) {
        Some(episode) => episode_token(video_name).as_deref() == Some(episode.as_str()),
        None => true,
    }
}

/// 字幕入库后的文件名。词干能接上视频时保留语言后缀，否则保留压缩包内的文件名。
#[must_use]
pub fn subtitle_target_name(video_name: &str, subtitle_name: &str) -> String {
    let video_stem = file_stem(video_name);
    let subtitle_stem = file_stem(subtitle_name);
    let extension = extension_of(Path::new(subtitle_name)).unwrap_or_default();
    let video_lower = video_stem.to_lowercase();
    let subtitle_lower = subtitle_stem.to_lowercase();
    if let Some(suffix) = subtitle_lower.strip_prefix(&video_lower) {
        if suffix.is_empty() || starts_with_delimiter(suffix) {
            let suffix = subtitle_stem.get(video_stem.len()..).unwrap_or_default();
            return format!("{video_stem}{suffix}.{extension}");
        }
    }
    Path::new(subtitle_name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(subtitle_name)
        .to_string()
}

/// 把 `/` 相对路径接到目录上，忽略空段和 `.` / `..`。
#[must_use]
pub fn join_relative(root: &Path, relative: &str) -> PathBuf {
    let mut path = root.to_path_buf();
    for part in relative.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." {
            continue;
        }
        path.push(part);
    }
    path
}

/// 扫描源目录中的压缩包并提取白名单资产。
///
/// 单个压缩包被拒绝时只记录警告，不中断其余文件。`skip_directory` 为 true 的目录不会进入。
pub fn prepare_source_archives(
    root: &Path,
    skip_directory: impl Fn(&Path) -> bool,
    options: &IngestOptions,
) -> Result<Vec<PreparedArchive>> {
    let mut prepared = Vec::new();
    let mut warned_unsupported_scan = false;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|entry| !skip_directory(entry.path()))
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file() && is_asset_archive(entry.path()))
    {
        let source = entry.into_path();
        let directory = source.parent().unwrap_or(root).to_path_buf();
        match extract_archive_to_temp(&source, options) {
            Ok((temp, outcome)) => {
                if options.scan_with_defender {
                    match scan_extracted_directory(temp.path()) {
                        DefenderScan::Clean => {}
                        DefenderScan::Unsupported if !warned_unsupported_scan => {
                            warned_unsupported_scan = true;
                            prepared.push(retained(
                                source,
                                directory,
                                temp,
                                outcome.assets,
                                vec!["Windows Defender scan is unavailable on this platform; extracted assets were kept".to_string()],
                                false,
                            ));
                            continue;
                        }
                        DefenderScan::Unsupported => {}
                        DefenderScan::Failed(message) => {
                            prepared.push(rejected(source, directory, vec![message]));
                            continue;
                        }
                    }
                }
                prepared.push(retained(
                    source,
                    directory,
                    temp,
                    outcome.assets,
                    outcome.warnings,
                    false,
                ));
            }
            Err(error) => prepared.push(rejected(source, directory, vec![error.to_string()])),
        }
    }
    Ok(prepared)
}

/// 解压一个压缩包。拒绝时返回错误，且 `destination` 中不会留下本次写出的文件。
pub fn extract_asset_archive(
    archive: &Path,
    destination: &Path,
    limits: &ArchiveLimits,
) -> Result<ExtractOutcome> {
    let length = fs::metadata(archive)?.len();
    if length > MAX_ARCHIVE_FILE_BYTES {
        return Err(rejected_error(
            archive,
            format!("archive is {length} bytes, above the {MAX_ARCHIVE_FILE_BYTES} byte cap"),
        ));
    }
    if !destination.exists() {
        fs::create_dir_all(destination)?;
    }
    ensure_real_directory(destination)?;
    let extension = extension_of(archive).unwrap_or_default();
    let outcome = match extension.as_str() {
        "zip" => extract_zip(archive, destination, limits),
        "7z" => extract_sevenz(archive, destination, limits),
        "rar" => extract_rar(archive, destination, limits),
        _ => Err(rejected_error(archive, "unsupported archive extension")),
    };
    if outcome.is_err() {
        clear_extraction_directory(destination);
    }
    outcome
}

/// 在 Windows 上对临时目录做 Defender 自定义扫描。
pub fn scan_extracted_directory(path: &Path) -> DefenderScan {
    scan_extracted_directory_with(path, defender_executable())
}

fn scan_extracted_directory_with(path: &Path, executable: Option<PathBuf>) -> DefenderScan {
    let Some(executable) = executable else {
        return if cfg!(windows) {
            DefenderScan::Failed("MpCmdRun.exe was not found".to_string())
        } else {
            DefenderScan::Unsupported
        };
    };
    match Command::new(executable)
        .args(["-Scan", "-ScanType", "3", "-File"])
        .arg(path)
        .status()
    {
        Ok(status) if status.success() => DefenderScan::Clean,
        Ok(status) => DefenderScan::Failed(format!("Windows Defender scan failed: {status}")),
        Err(error) => DefenderScan::Failed(format!("failed to start Windows Defender: {error}")),
    }
}

fn defender_executable() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let mut candidates = Vec::new();
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(program_files)
                .join("Windows Defender")
                .join("MpCmdRun.exe"),
        );
    }
    candidates.push(PathBuf::from(
        r"C:\Program Files\Windows Defender\MpCmdRun.exe",
    ));
    candidates.into_iter().find(|candidate| candidate.is_file())
}

fn extract_archive_to_temp(
    archive: &Path,
    options: &IngestOptions,
) -> Result<(tempfile::TempDir, ExtractOutcome)> {
    let temp = tempfile::tempdir().map_err(|error| AppError::FileOperation {
        path: archive.to_path_buf(),
        message: format!("create archive temp dir failed: {error}"),
    })?;
    let outcome = extract_asset_archive(archive, temp.path(), &options.limits)?;
    Ok((temp, outcome))
}

fn retained(
    source: PathBuf,
    directory: PathBuf,
    temp: tempfile::TempDir,
    assets: Vec<ExtractedAsset>,
    warnings: Vec<String>,
    rejected: bool,
) -> PreparedArchive {
    PreparedArchive {
        source,
        directory,
        assets,
        warnings,
        rejected,
        consumed: Cell::new(false),
        _temp: Some(temp),
    }
}

fn rejected(source: PathBuf, directory: PathBuf, warnings: Vec<String>) -> PreparedArchive {
    PreparedArchive {
        source,
        directory,
        assets: Vec::new(),
        warnings,
        rejected: true,
        consumed: Cell::new(false),
        _temp: None,
    }
}

fn extract_zip(
    archive: &Path,
    destination: &Path,
    limits: &ArchiveLimits,
) -> Result<ExtractOutcome> {
    let file = File::open(archive)?;
    let mut zip =
        ZipArchive::new(file).map_err(|error| rejected_error(archive, error.to_string()))?;
    let mut entries = Vec::with_capacity(zip.len());
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|error| rejected_error(archive, error.to_string()))?;
        if entry.encrypted() {
            return Err(rejected_error(
                archive,
                format!("encrypted entry '{}'", entry.name()),
            ));
        }
        entries.push(EntryMeta {
            name: entry.name().to_string(),
            uncompressed: entry.size(),
            compressed: entry.compressed_size(),
            link: entry.is_symlink() || entry.unix_mode().is_some_and(unix_mode_is_link),
            anti: false,
        });
    }
    vet_entries(archive, &entries, limits)?;
    let mut outcome = ExtractOutcome {
        assets: Vec::new(),
        warnings: Vec::new(),
    };
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| rejected_error(archive, error.to_string()))?;
        if entry.is_dir() {
            continue;
        }
        let relative = vetted_relative(entry.name())
            .map_err(|error| rejected_error(archive, format!("{}: {error}", entry.name())))?;
        match asset_kind(&relative) {
            AssetDisposition::Asset(kind) => {
                let output = join_relative(destination, &relative_string(&relative));
                let declared = entry.size();
                copy_entry(archive, destination, &mut entry, &output, declared)?;
                outcome.assets.push(ExtractedAsset {
                    relative_path: relative_string(&relative),
                    absolute_path: output,
                    kind,
                });
            }
            AssetDisposition::Blocked => {
                outcome
                    .warnings
                    .push(format!("skipped blocked entry '{}'", entry.name()));
            }
            AssetDisposition::Ignore => {}
        }
    }
    Ok(outcome)
}

fn extract_sevenz(
    archive: &Path,
    destination: &Path,
    limits: &ArchiveLimits,
) -> Result<ExtractOutcome> {
    let mut reader = SevenZReader::open(archive, Password::empty())
        .map_err(|error| rejected_error(archive, error.to_string()))?;
    let mut entries = Vec::new();
    let mut failure: Option<String> = None;
    reader
        .for_each_entries(|entry, _reader| {
            if failure.is_some() {
                return Ok(false);
            }
            let meta = EntryMeta {
                name: entry.name().to_string(),
                uncompressed: entry.size(),
                compressed: entry.compressed_size,
                link: entry.has_windows_attributes
                    && windows_attributes_are_link(entry.windows_attributes()),
                anti: entry.is_anti_item,
            };
            if let Err(error) = consider_entry(archive, &meta, limits, entries.len(), &entries) {
                failure = Some(error.to_string());
                return Ok(false);
            }
            entries.push(meta);
            Ok(true)
        })
        .map_err(|error| rejected_error(archive, error.to_string()))?;
    if let Some(failure) = failure {
        return Err(rejected_error(archive, failure));
    }
    vet_entries(archive, &entries, limits)?;

    let mut reader = SevenZReader::open(archive, Password::empty())
        .map_err(|error| rejected_error(archive, error.to_string()))?;
    let mut outcome = ExtractOutcome {
        assets: Vec::new(),
        warnings: Vec::new(),
    };
    let mut write_error: Option<AppError> = None;
    reader
        .for_each_entries(|entry, reader| {
            if write_error.is_some() {
                return Ok(false);
            }
            if entry.is_directory() {
                return Ok(true);
            }
            let relative = match vetted_relative(entry.name()) {
                Ok(relative) => relative,
                Err(error) => {
                    write_error = Some(rejected_error(archive, error));
                    return Ok(false);
                }
            };
            match asset_kind(&relative) {
                AssetDisposition::Asset(kind) => {
                    let output = join_relative(destination, &relative_string(&relative));
                    if let Err(error) =
                        copy_entry(archive, destination, reader, &output, entry.size())
                    {
                        write_error = Some(error);
                        return Ok(false);
                    }
                    outcome.assets.push(ExtractedAsset {
                        relative_path: relative_string(&relative),
                        absolute_path: output,
                        kind,
                    });
                }
                AssetDisposition::Blocked => {
                    outcome
                        .warnings
                        .push(format!("skipped blocked entry '{}'", entry.name()));
                    drain(reader, entry.size()).map_err(sevenz_rust2::Error::io)?;
                }
                AssetDisposition::Ignore => {
                    drain(reader, entry.size()).map_err(sevenz_rust2::Error::io)?;
                }
            }
            Ok(true)
        })
        .map_err(|error| rejected_error(archive, error.to_string()))?;
    if let Some(error) = write_error {
        return Err(error);
    }
    Ok(outcome)
}

fn extract_rar(
    archive: &Path,
    destination: &Path,
    limits: &ArchiveLimits,
) -> Result<ExtractOutcome> {
    let bytes = fs::read(archive)?;
    let rar = rar_codec::Archive::open(&bytes)
        .map_err(|error| rejected_error(archive, error.to_string()))?;
    if rar.encrypted {
        return Err(rejected_error(archive, "encrypted RAR headers"));
    }
    let entries = rar
        .entries()
        .iter()
        .map(|entry| EntryMeta {
            name: entry.name.to_string(),
            uncompressed: entry.unpacked_size,
            compressed: entry.packed_size,
            link: matches!(entry.kind, EntryKind::Symlink | EntryKind::Hardlink),
            anti: false,
        })
        .collect::<Vec<_>>();
    vet_entries(archive, &entries, limits)?;
    let mut outcome = ExtractOutcome {
        assets: Vec::new(),
        warnings: Vec::new(),
    };
    for entry in rar.entries() {
        if entry.kind == EntryKind::Directory || entry.kind == EntryKind::Service {
            continue;
        }
        let relative = vetted_relative(&entry.name)
            .map_err(|error| rejected_error(archive, format!("{}: {error}", entry.name)))?;
        match asset_kind(&relative) {
            AssetDisposition::Asset(kind) => {
                let bytes = rar
                    .extract(entry)
                    .map_err(|error| rejected_error(archive, error.to_string()))?;
                if bytes.len() as u64 > entry.unpacked_size.max(limits.max_entry_bytes) {
                    return Err(rejected_error(
                        archive,
                        format!("entry '{}' exceeded its declared size", entry.name),
                    ));
                }
                let output = join_relative(destination, &relative_string(&relative));
                write_bytes(destination, &output, &bytes)?;
                outcome.assets.push(ExtractedAsset {
                    relative_path: relative_string(&relative),
                    absolute_path: output,
                    kind,
                });
            }
            AssetDisposition::Blocked => {
                outcome
                    .warnings
                    .push(format!("skipped blocked entry '{}'", entry.name));
            }
            AssetDisposition::Ignore => {}
        }
    }
    Ok(outcome)
}

fn consider_entry(
    archive: &Path,
    entry: &EntryMeta,
    limits: &ArchiveLimits,
    seen: usize,
    previous: &[EntryMeta],
) -> Result<()> {
    if seen + 1 > limits.max_entries {
        return Err(rejected_error(
            archive,
            format!("entry count exceeds {}", limits.max_entries),
        ));
    }
    if entry.anti {
        return Err(rejected_error(
            archive,
            format!("anti-item '{}'", entry.name),
        ));
    }
    if entry.link {
        return Err(rejected_error(
            archive,
            format!("refusing link entry '{}'", entry.name),
        ));
    }
    if vetted_relative(&entry.name).is_err() {
        return Err(rejected_error(
            archive,
            format!("unsafe path '{}'", entry.name),
        ));
    }
    if entry.uncompressed > limits.max_entry_bytes {
        return Err(rejected_error(
            archive,
            format!(
                "entry '{}' is {} bytes, above the {} byte cap",
                entry.name, entry.uncompressed, limits.max_entry_bytes
            ),
        ));
    }
    let total_uncompressed = previous.iter().fold(entry.uncompressed, |total, item| {
        total.saturating_add(item.uncompressed)
    });
    let total_compressed = previous.iter().fold(entry.compressed, |total, item| {
        total.saturating_add(item.compressed)
    });
    if total_uncompressed > limits.max_total_bytes {
        return Err(rejected_error(
            archive,
            format!(
                "uncompressed size {total_uncompressed} exceeds {}",
                limits.max_total_bytes
            ),
        ));
    }
    if ratio_exceeds(entry.uncompressed, entry.compressed, limits.max_ratio)
        || ratio_exceeds(total_uncompressed, total_compressed, limits.max_ratio)
    {
        return Err(rejected_error(
            archive,
            format!("compression ratio exceeds {}", limits.max_ratio),
        ));
    }
    Ok(())
}

fn vet_entries(archive: &Path, entries: &[EntryMeta], limits: &ArchiveLimits) -> Result<()> {
    let mut accepted = Vec::new();
    for entry in entries {
        consider_entry(archive, entry, limits, accepted.len(), &accepted)?;
        accepted.push(entry.clone());
    }
    Ok(())
}

enum AssetDisposition {
    Asset(AssetKind),
    Blocked,
    Ignore,
}

fn asset_kind(relative: &Path) -> AssetDisposition {
    let Some(name) = relative.file_name().and_then(|name| name.to_str()) else {
        return AssetDisposition::Ignore;
    };
    if blocked_file_name(name) {
        return AssetDisposition::Blocked;
    }
    match extension_of(relative).as_deref() {
        Some(extension) if SUBTITLE_EXTENSIONS.contains(&extension) => {
            AssetDisposition::Asset(AssetKind::Subtitle)
        }
        Some(extension) if FONT_EXTENSIONS.contains(&extension) => {
            AssetDisposition::Asset(AssetKind::Font)
        }
        _ => AssetDisposition::Ignore,
    }
}

fn blocked_file_name(name: &str) -> bool {
    name.rsplit('.')
        .skip(1)
        .any(|part| BLOCKED_EXTENSIONS.contains(&part.to_ascii_lowercase().as_str()))
        || extension_of(Path::new(name))
            .is_some_and(|extension| BLOCKED_EXTENSIONS.contains(&extension.as_str()))
}

fn vetted_relative(name: &str) -> std::result::Result<PathBuf, String> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains('\0') {
        return Err("absolute or NUL path".to_string());
    }
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err("absolute path".to_string());
    }
    let mut relative = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("path escapes the archive root".to_string());
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err("empty path".to_string());
    }
    Ok(relative)
}

fn relative_string(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn copy_entry(
    archive: &Path,
    destination: &Path,
    reader: &mut dyn Read,
    output: &Path,
    declared: u64,
) -> Result<()> {
    if let Some(parent) = output.parent() {
        create_directories_without_following(destination, parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let written = drain_into(reader, &mut file, declared)?;
    file.flush()?;
    if written > declared {
        let _ = fs::remove_file(output);
        return Err(rejected_error(archive, "entry grew past its declared size"));
    }
    ensure_regular_file_within(destination, output)?;
    Ok(())
}

fn write_bytes(destination: &Path, output: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = output.parent() {
        create_directories_without_following(destination, parent)?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    file.write_all(bytes)?;
    ensure_regular_file_within(destination, output).inspect_err(|_| {
        let _ = fs::remove_file(output);
    })?;
    Ok(())
}

fn drain(reader: &mut dyn Read, declared: u64) -> io::Result<()> {
    let mut sink = io::sink();
    drain_into(reader, &mut sink, declared)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
    Ok(())
}

fn drain_into(reader: &mut dyn Read, writer: &mut dyn Write, declared: u64) -> Result<u64> {
    let mut buffer = [0_u8; 64 * 1024];
    let mut written = 0_u64;
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        let next = written.saturating_add(read as u64);
        if next > declared {
            return Err(AppError::FileOperation {
                path: PathBuf::from("archive-entry"),
                message: "entry exceeded its declared size".to_string(),
            });
        }
        writer.write_all(&buffer[..read])?;
        written = next;
    }
    Ok(written)
}

fn ensure_real_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(AppError::FileOperation {
            path: path.to_path_buf(),
            message: "refusing to extract through a symlink".to_string(),
        }),
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(AppError::FileOperation {
            path: path.to_path_buf(),
            message: "extraction destination is not a directory".to_string(),
        }),
        Err(error) => Err(error.into()),
    }
}

fn clear_extraction_directory(destination: &Path) {
    let Ok(metadata) = fs::symlink_metadata(destination) else {
        return;
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return;
    }
    let _ = fs::remove_dir_all(destination);
    let _ = fs::create_dir_all(destination);
}

/// 只检查并创建目标目录内部的路径。
///
/// macOS 的 `/var` 和 `/tmp` 是指向 `/private` 的符号链接。调用方给出的临时目录
/// 可以经过这些系统链接，但压缩包条目不能在目标目录内部再穿越符号链接。
fn create_directories_without_following(root: &Path, path: &Path) -> Result<()> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AppError::FileOperation {
            path: path.to_path_buf(),
            message: "refusing to create a directory outside the extraction root".to_string(),
        })?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        match component {
            Component::Normal(part) => current.push(part),
            Component::CurDir => continue,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::FileOperation {
                    path: path.to_path_buf(),
                    message: "refusing to create a directory via '..'".to_string(),
                });
            }
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::FileOperation {
                    path: current,
                    message: "refusing to follow a symlink while extracting".to_string(),
                });
            }
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => {
                return Err(AppError::FileOperation {
                    path: current,
                    message: "extraction parent is not a directory".to_string(),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn ensure_regular_file_within(root: &Path, file: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(file)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        let _ = fs::remove_file(file);
        return Err(AppError::FileOperation {
            path: file.to_path_buf(),
            message: "extracted entry is not a regular file".to_string(),
        });
    }
    let root = root.canonicalize()?;
    let parent = file.parent().unwrap_or(file).canonicalize()?;
    if !parent.starts_with(&root) {
        let _ = fs::remove_file(file);
        return Err(rejected_error(
            file,
            "extracted path escaped the temp directory",
        ));
    }
    Ok(())
}

fn ratio_exceeds(uncompressed: u64, compressed: u64, max_ratio: u64) -> bool {
    compressed > 0 && uncompressed > compressed.saturating_mul(max_ratio)
}

fn unix_mode_is_link(mode: u32) -> bool {
    mode & 0o170_000 == 0o120_000
}

fn windows_attributes_are_link(attributes: u32) -> bool {
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    let unix_mode = attributes >> 16;
    unix_mode_is_link(unix_mode) || attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
}

fn file_stem(name: &str) -> &str {
    Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(name)
}

fn stem_prefix_matches(video_name: &str, subtitle_name: &str) -> bool {
    let video = file_stem(video_name).to_lowercase();
    let subtitle = file_stem(subtitle_name).to_lowercase();
    subtitle
        .strip_prefix(&video)
        .is_some_and(|suffix| suffix.is_empty() || starts_with_delimiter(suffix))
}

fn starts_with_delimiter(suffix: &str) -> bool {
    suffix.starts_with(['.', ' ', '_', '-', '[', '(', '（'])
}

/// 从文件名里取 ` - 12` 这种集数标记。
#[must_use]
pub fn episode_token(file_name: &str) -> Option<String> {
    let stem = file_stem(file_name);
    let bytes = stem.as_bytes();
    let mut found = None;
    for index in 0..bytes.len() {
        if bytes[index] != b'-'
            || index == 0
            || bytes[index - 1] != b' '
            || index + 2 >= bytes.len()
            || bytes[index + 1] != b' '
        {
            continue;
        }
        let mut start = index + 2;
        while start < bytes.len() && bytes[start] == b' ' {
            start += 1;
        }
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end < bytes.len()
            && bytes[end] == b'.'
            && end + 1 < bytes.len()
            && bytes[end + 1].is_ascii_digit()
        {
            end += 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        if end == start {
            continue;
        }
        let mut suffix = end;
        if suffix + 1 < bytes.len()
            && matches!(bytes[suffix], b'v' | b'V')
            && bytes[suffix + 1].is_ascii_digit()
        {
            suffix += 2;
            while suffix < bytes.len() && bytes[suffix].is_ascii_digit() {
                suffix += 1;
            }
        }
        let after = bytes.get(suffix).copied().unwrap_or(b' ');
        if matches!(after, b' ' | b'[' | b'-' | b'.' | b'(') || suffix >= bytes.len() {
            let raw = std::str::from_utf8(&bytes[start..end]).ok()?;
            found = Some(if raw.contains('.') {
                raw.to_string()
            } else {
                format!("{raw:0>2}")
            });
        }
    }
    found
}

fn titles_overlap(left: &str, right: &str) -> bool {
    let left = title_key(left);
    let right = title_key(right);
    !left.is_empty()
        && !right.is_empty()
        && (left == right || left.contains(&right) || right.contains(&left))
}

fn title_key(file_name: &str) -> String {
    let mut text = file_stem(file_name).to_string();
    while let Some(start) = text.find('[') {
        let Some(end) = text[start..].find(']') else {
            break;
        };
        text.replace_range(start..start + end + 1, " ");
    }
    if let Some(token) = episode_token(file_name) {
        let marker = format!(" - {token}");
        if let Some(position) = text.find(&marker) {
            text.truncate(position);
        } else if let Some(position) = text.to_lowercase().find(&marker) {
            text.truncate(position);
        }
    }
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn rejected_error(path: &Path, message: impl Into<String>) -> AppError {
    AppError::FileOperation {
        path: path.to_path_buf(),
        message: message.into(),
    }
}

/// 供测试构造 7z。属性位用于覆盖符号链接拒绝。
#[cfg(test)]
fn write_sevenz(path: &Path, files: &[(&str, &[u8], Option<u32>)]) {
    use sevenz_rust2::{SevenZArchiveEntry, SevenZWriter};
    let mut writer = SevenZWriter::create(path).unwrap();
    for (name, bytes, attributes) in files {
        let mut entry = SevenZArchiveEntry::new_file(name);
        if let Some(attributes) = attributes {
            entry.has_windows_attributes = true;
            entry.windows_attributes = *attributes;
        }
        writer
            .push_archive_entry(entry, Some(io::Cursor::new(*bytes)))
            .unwrap();
    }
    writer.finish().unwrap();
}

/// 供测试构造 zip。`mode` 为 Unix 权限，用来造符号链接条目。
#[cfg(test)]
fn write_zip(path: &Path, files: &[(&str, &[u8], Option<u32>)]) {
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;
    let file = File::create(path).unwrap();
    let mut zip = ZipWriter::new(file);
    for (name, bytes, mode) in files {
        if mode.is_some_and(unix_mode_is_link) {
            let target = std::str::from_utf8(bytes).expect("symlink target is utf-8");
            zip.add_symlink(*name, target, SimpleFileOptions::default())
                .unwrap();
            continue;
        }
        let mut options = SimpleFileOptions::default();
        if let Some(mode) = mode {
            options = options.unix_permissions(*mode);
        }
        zip.start_file(*name, options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
}

#[cfg(test)]
fn write_rar(path: &Path, files: &[(&str, &[u8])]) {
    let mut builder = rar_codec::ArchiveBuilder::new();
    for (name, bytes) in files {
        builder.add_stored(name, bytes).unwrap();
    }
    fs::write(path, builder.finish()).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> ArchiveLimits {
        ArchiveLimits::default()
    }

    fn extract_owned(
        archive: &Path,
        limits: &ArchiveLimits,
    ) -> Result<(tempfile::TempDir, ExtractOutcome)> {
        let destination = tempfile::tempdir().unwrap();
        let outcome = extract_asset_archive(archive, destination.path(), limits)?;
        Ok((destination, outcome))
    }

    #[test]
    fn zip_extracts_subtitle_and_font_only() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("subs.zip");
        write_zip(
            &archive,
            &[
                ("[ANi] Show - 01 [1080P].chs.ass", b"dialogue", None),
                ("fonts/Show.ttf", b"font", None),
                ("readme.txt", b"notes", None),
                ("setup.bat", b"@echo off", None),
            ],
        );

        let (destination, outcome) = extract_owned(&archive, &limits()).unwrap();
        assert_eq!(outcome.assets.len(), 2);
        assert!(outcome
            .warnings
            .iter()
            .any(|warning| warning.contains("setup.bat")));
        let subtitle = destination.path().join("[ANi] Show - 01 [1080P].chs.ass");
        assert_eq!(fs::read(subtitle).unwrap(), b"dialogue");
        assert_eq!(
            fs::read(destination.path().join("fonts").join("Show.ttf")).unwrap(),
            b"font"
        );
        assert!(!destination.path().join("readme.txt").exists());
        assert!(!destination.path().join("setup.bat").exists());
    }

    #[test]
    fn zip_slip_script_symlink_and_bomb_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let slip = dir.path().join("slip.zip");
        write_zip(&slip, &[("../../evil.exe", b"MZ", None)]);
        let error =
            extract_asset_archive(&slip, &dir.path().join("out-slip"), &limits()).unwrap_err();
        assert!(error.to_string().contains("unsafe") || error.to_string().contains("escapes"));
        assert!(dir
            .path()
            .join("out-slip")
            .read_dir()
            .map(|entries| entries.count() == 0)
            .unwrap_or(true));
        assert!(!dir.path().join("evil.exe").exists());

        let script = dir.path().join("script.zip");
        write_zip(
            &script,
            &[
                ("payload.exe", b"MZ", None),
                ("run.ps1", b"Write-Host", None),
            ],
        );
        let (destination, outcome) = extract_owned(&script, &limits()).unwrap();
        assert!(outcome.assets.is_empty());
        assert!(!walk_has_executable(destination.path()));

        let link = dir.path().join("link.zip");
        write_zip(&link, &[("fonts.ttf", b"/tmp/evil", Some(0o120_777))]);
        assert!(extract_asset_archive(&link, &dir.path().join("out-link"), &limits()).is_err());
        assert!(!dir.path().join("out-link").join("fonts.ttf").exists());

        let bomb = dir.path().join("bomb.zip");
        write_zip(&bomb, &[("bomb.ass", &vec![0_u8; 256 * 1024], None)]);
        let error =
            extract_asset_archive(&bomb, &dir.path().join("out-bomb"), &limits()).unwrap_err();
        assert!(error.to_string().contains("ratio"));
        assert!(!dir.path().join("out-bomb").join("bomb.ass").exists());
    }

    #[test]
    fn sevenz_and_rar_round_trip_whitelist() {
        let dir = tempfile::tempdir().unwrap();
        let seven = dir.path().join("subs.7z");
        write_sevenz(&seven, &[("[ANi] Show - 01.chs.ass", b"from-7z", None)]);
        let (destination, outcome) = extract_owned(&seven, &limits()).unwrap();
        assert_eq!(outcome.assets.len(), 1);
        assert_eq!(
            fs::read(&outcome.assets[0].absolute_path).unwrap(),
            b"from-7z"
        );
        drop(destination);

        let rar = dir.path().join("fonts.rar");
        write_rar(&rar, &[("Font.otf", b"otf-bytes")]);
        let (_destination, outcome) = extract_owned(&rar, &limits()).unwrap();
        assert_eq!(outcome.assets[0].kind, AssetKind::Font);
        assert_eq!(
            fs::read(&outcome.assets[0].absolute_path).unwrap(),
            b"otf-bytes"
        );
    }

    #[test]
    fn sevenz_symlink_attribute_rejects_the_archive() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("link.7z");
        write_sevenz(&archive, &[("link.ass", b"target", Some(0o120_777 << 16))]);
        assert!(extract_asset_archive(&archive, &dir.path().join("out"), &limits()).is_err());
    }

    #[test]
    fn entry_count_and_single_file_caps_reject() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("many.zip");
        write_zip(&archive, &[("a.ass", b"a", None), ("b.ass", b"b", None)]);
        let tight = ArchiveLimits {
            max_entries: 1,
            ..ArchiveLimits::default()
        };
        assert!(extract_asset_archive(&archive, &dir.path().join("out"), &tight).is_err());

        let large = dir.path().join("large.zip");
        write_zip(&large, &[("a.ass", b"abcdef", None)]);
        let tight = ArchiveLimits {
            max_entry_bytes: 4,
            ..ArchiveLimits::default()
        };
        assert!(extract_asset_archive(&large, &dir.path().join("out-large"), &tight).is_err());
    }

    #[test]
    fn subtitle_matching_accepts_release_tags_and_episode_token() {
        assert!(subtitle_matches(
            "[ANi] Test Show - 01 [1080P].mkv",
            "[ANi] Test Show - 01 [1080P].chs.ass"
        ));
        assert!(subtitle_matches(
            "[ANi] Test Show - 01 [1080P].mkv",
            "[ANi] Test Show - 01.chs.ass"
        ));
        assert!(!subtitle_matches(
            "[ANi] Test Show - 01 [1080P].mkv",
            "[ANi] Other Show - 01.chs.ass"
        ));
        assert!(font_applies(
            "[ANi] Test Show - 05.fonts.rar",
            "[ANi] Test Show - 05 [WebRip].mkv"
        ));
        assert!(!font_applies(
            "[ANi] Test Show - 05.fonts.rar",
            "[ANi] Test Show - 06 [WebRip].mkv"
        ));
        assert!(font_applies(
            "[ANi] Test Show - Fonts.7z",
            "[ANi] Test Show - 06 [WebRip].mkv"
        ));
    }

    #[cfg(unix)]
    #[test]
    fn extraction_allows_symlink_ancestors_outside_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        fs::create_dir(&real).unwrap();
        let linked = dir.path().join("linked");
        std::os::unix::fs::symlink(&real, &linked).unwrap();
        let destination = linked.join("out");
        fs::create_dir(&destination).unwrap();
        let archive = dir.path().join("subs.zip");
        write_zip(&archive, &[("fonts/Show.ttf", b"font", None)]);

        let outcome = extract_asset_archive(&archive, &destination, &limits()).unwrap();
        assert_eq!(outcome.assets.len(), 1);
        assert_eq!(
            fs::read(real.join("out").join("fonts").join("Show.ttf")).unwrap(),
            b"font"
        );
    }

    #[cfg(unix)]
    #[test]
    fn extraction_refuses_symlink_directory_inside_the_destination() {
        let destination = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), destination.path().join("fonts")).unwrap();
        let archive = destination.path().join("fonts.zip");
        write_zip(&archive, &[("fonts/Show.ttf", b"font", None)]);

        let error = extract_asset_archive(&archive, destination.path(), &limits()).unwrap_err();
        assert!(error.to_string().contains("symlink"));
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn defender_scan_reports_unsupported_without_an_executable() {
        assert!(matches!(
            scan_extracted_directory_with(Path::new("."), None),
            DefenderScan::Unsupported
        ));
    }

    fn walk_has_executable(root: &Path) -> bool {
        walkdir::WalkDir::new(root).into_iter().any(|entry| {
            entry.ok().is_some_and(|entry| {
                extension_of(entry.path())
                    .is_some_and(|extension| BLOCKED_EXTENSIONS.contains(&extension.as_str()))
            })
        })
    }
}
