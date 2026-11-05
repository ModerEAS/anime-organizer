#![allow(clippy::result_large_err)]

//! anime-organizer 命令行入口
//!
//! 提供默认的文件整理模式，以及用于自动化工作流的 scraper 子命令。

mod cli;
mod commands;
#[cfg(feature = "daemon")]
mod daemon;
#[cfg(feature = "metadata")]
mod metadata_settings;
#[cfg(feature = "metadata")]
mod mlip;
#[cfg(any(feature = "daemon", feature = "self-update", feature = "metadata"))]
mod paths;
#[cfg(feature = "self-update")]
mod self_update;
#[cfg(feature = "metadata")]
mod title_resolver;

use crate::cli::*;
use crate::commands::run_command;
#[cfg(feature = "metadata")]
use crate::mlip::{
    anime_group_min_episode, apply_bangumi_episode_details, artwork_source, create_episode_nfo,
    download_images, fetch_anime_metadata, fetch_bangumi_episodes_cached, min_episode_by_series,
    MetadataLookup,
};
use anime_organizer::archive_assets::{
    join_relative, prepare_source_archives, subtitle_target_name, ArchiveLimits, IngestOptions,
    PreparedArchive,
};
#[cfg(feature = "metadata")]
use anime_organizer::library_index::{Artwork, ArtworkKind, ExternalProvider};
use anime_organizer::{
    error::AppError, organize_directory_components, parser::strip_residual_tags, sha256_file,
    AnimeFileInfo, FileOrganizer, FilenameParser, LibraryExtraRecord, LibraryIndex,
    LibraryIndexRecord, OperationMode,
};
#[cfg(feature = "metadata")]
use anime_organizer::{
    metadata::{bangumi::BangumiEpisode, AliasLookup, BangumiClient, TmdbClient},
    nfo::{NfoWriter, TvShowNfo},
    AnimeMetadata,
};
use clap::Parser;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(feature = "metadata")]
use std::sync::Arc;
#[cfg(feature = "metadata")]
use tokio::io::AsyncWriteExt;
use walkdir::WalkDir;

/// 默认支持的视频扩展名
const DEFAULT_EXTENSIONS: &[&str] = &[".mp4", ".mkv", ".avi", ".mov", ".wmv", ".flv", ".rmvb"];
#[cfg(feature = "metadata")]
const ANIMEATLAS_SQLITE_FILENAME: &str = "animeatlas.sqlite";
#[cfg(feature = "metadata")]
const ANIMEATLAS_SQLITE_URL: &str =
    "https://github.com/ModerRAS/AnimeAtlas/releases/download/download/animeatlas.sqlite";
#[cfg(feature = "metadata")]
const ANIMEATLAS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);
#[cfg(feature = "metadata")]
const BANGUMI_ALIAS_DB_FILENAME: &str = "bangumi.db";
#[cfg(feature = "metadata")]
const BANGUMI_ALIAS_DB_URL: &str =
    "https://github.com/ModerRAS/anime-organizer/releases/download/bangumi-db-latest/bangumi.db";
/// 全量别名库由 `bangumi-db.yml` 每周日重建，本地缓存同样按 7 天 TTL 复用。
#[cfg(feature = "metadata")]
const BANGUMI_ALIAS_DB_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);
/// 全量库约 120 MB，明显偏小视为下载不完整。
#[cfg(feature = "metadata")]
const BANGUMI_ALIAS_DB_MIN_BYTES: u64 = 16 * 1024 * 1024;
#[cfg(feature = "metadata")]
static BANGUMI_ALIAS_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(feature = "metadata")]
static ANIMEATLAS_REFRESH_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
#[cfg(feature = "metadata")]
const HTTP_USER_AGENT: &str = concat!(
    "ModerRAS/anime-organizer/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/ModerRAS/anime-organizer)"
);
static ANIFILEBERT_AUTO_WARNED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

fn main() {
    if let Err(error) = run() {
        eprintln!("错误: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), AppError> {
    let cli = Cli::parse();

    #[cfg(feature = "self-update")]
    {
        if cli.wait_for_parent || cli.replace_from.is_some() || cli.replace_to.is_some() {
            let from = cli
                .replace_from
                .as_deref()
                .ok_or_else(|| AppError::SelfUpdate("替换参数不完整".to_string()))?;
            let to = cli
                .replace_to
                .as_deref()
                .ok_or_else(|| AppError::SelfUpdate("替换参数不完整".to_string()))?;
            return self_update::apply_replacement(from, to, cli.wait_for_parent, &cli.restart_arg)
                .map_err(AppError::SelfUpdate);
        }
        if cli.from_self_update {
            self_update::ignore_terminal_hangup();
            eprintln!("程序已更新并重新启动。");
        }
    }

    #[cfg(feature = "daemon")]
    if cli.daemon {
        reject_daemon_conflicts(&cli)?;
        return daemon::run();
    }

    if let Some(command) = cli.command {
        return run_command(command);
    }

    run_organize_entry(cli.organize)
}

#[cfg(feature = "daemon")]
fn reject_daemon_conflicts(cli: &Cli) -> Result<(), AppError> {
    if cli.command.is_some() {
        return Err(AppError::ParseError(
            "--daemon cannot be combined with a subcommand".to_string(),
        ));
    }

    let args = &cli.organize;
    if args.source.is_some()
        || args.target.is_some()
        || args.fallback_on_link_failure.is_some()
        || args.dry_run
        || args.delete_extracted_archives
        || args.scan_extracted_archives
        || args.include_ext.is_some()
        || args.verbose
        || args.scrape_metadata
        || args.tmdb_api_key.is_some()
        || args.alias_file.is_some()
        || args.no_images
        || args.no_episode_metadata
        || args.force_overwrite
        || args.bangumi_cache.is_some()
        || args.metadata_source.is_some()
        || args.season_mode
        || args.library_index
        || args.mlip
        || args.rebuild_library_index
        || args.refresh_library_metadata
        || args.probe_runtime
        || args.mode != OperationMode::Link
        || args.filename_parser != FilenameParserMode::Rules
    {
        return Err(AppError::ParseError(
            "--daemon cannot be combined with organize arguments".to_string(),
        ));
    }
    Ok(())
}

#[cfg(feature = "metadata")]
pub(crate) fn run_organize_entry(args: OrganizeArgs) -> Result<(), AppError> {
    run_organize_entry_with_log(args, &|_| {})
}

#[cfg(feature = "metadata")]
pub(crate) fn run_organize_entry_with_log(
    args: OrganizeArgs,
    log: &dyn Fn(&str),
) -> Result<(), AppError> {
    run_organize_entry_with_cancel(args, log, &|| false)
}

#[cfg(feature = "metadata")]
pub(crate) fn run_organize_entry_with_cancel(
    args: OrganizeArgs,
    log: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<(), AppError> {
    ensure_not_canceled(cancel)?;
    if args.scrape_metadata || args.mlip {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;
        runtime.block_on(run_with_metadata(args, log, cancel))
    } else {
        run_organize(args, log, cancel)
    }
}

#[cfg(not(feature = "metadata"))]
pub(crate) fn run_organize_entry(args: OrganizeArgs) -> Result<(), AppError> {
    run_organize_entry_with_log(args, &|_| {})
}

#[cfg(not(feature = "metadata"))]
pub(crate) fn run_organize_entry_with_log(
    args: OrganizeArgs,
    log: &dyn Fn(&str),
) -> Result<(), AppError> {
    run_organize_entry_with_cancel(args, log, &|| false)
}

#[cfg(not(feature = "metadata"))]
pub(crate) fn run_organize_entry_with_cancel(
    args: OrganizeArgs,
    log: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<(), AppError> {
    ensure_not_canceled(cancel)?;
    if args.scrape_metadata || args.mlip {
        return Err(AppError::MetadataFetchError(
            "元数据功能未启用，请使用 --features metadata 编译".to_string(),
        ));
    }

    run_organize(args, log, cancel)
}

fn ensure_not_canceled(cancel: &dyn Fn() -> bool) -> Result<(), AppError> {
    if cancel() {
        Err(AppError::Canceled)
    } else {
        Ok(())
    }
}

/// 仅文件整理流程（无元数据）
fn run_organize(
    args: OrganizeArgs,
    log: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<(), AppError> {
    ensure_not_canceled(cancel)?;
    validate_library_index_args(&args)?;
    validate_filename_parser_args(&args)?;
    let (source, target) = resolve_source_and_target(&args)?;
    let fallback_mode = args
        .fallback_on_link_failure
        .map(FallbackMode::to_operation_mode);
    let extensions = build_extensions(&args.include_ext);
    let subtitle_candidates = FileOrganizer::collect_external_subtitle_candidates(&source);
    let archives = load_source_archives(&source, args.scan_extracted_archives, log)?;
    log(&format!("Scanning {}", source.display()));
    let probe_runtime = runtime_probe_enabled(&args);

    let mut processed = 0;
    let mut succeeded = 0;
    let mut failed = 0;
    let mut canceled = false;
    let mut library_records = Vec::new();

    for entry in WalkDir::new(&source)
        .into_iter()
        .filter_entry(|entry| !is_supplemental_video_path(entry.path()))
        .filter_map(|item| item.ok())
        .filter(|item| item.file_type().is_file())
    {
        if cancel() {
            if processed == 0 {
                return Err(AppError::Canceled);
            }
            canceled = true;
            break;
        }
        let path = entry.path();
        if !has_valid_extension(path, &extensions) {
            continue;
        }

        let anime_file = match parse_anime_file(path, args.filename_parser, args.verbose)? {
            Some(info) => info,
            None => {
                if args.verbose {
                    eprintln!(
                        "跳过：无法解析文件名 {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
                continue;
            }
        };

        processed += 1;
        let target_dir = organize_directory_components(&anime_file, args.season_mode)
            .iter()
            .fold(target.clone(), |path, component| path.join(component));
        match organize_file_to_dir(
            &anime_file,
            &target_dir,
            args.mode,
            args.dry_run,
            fallback_mode,
            args.verbose,
            &subtitle_candidates,
            args.writes_library_index() && !args.dry_run,
            &source,
            log,
        ) {
            Ok((target_path, sha256_full)) => {
                match place_archive_assets(
                    Path::new(&anime_file.original_path),
                    &target_path,
                    &archives,
                    args.mode,
                    args.dry_run,
                ) {
                    Ok(placed_subtitles) => {
                        succeeded += 1;
                        if args.writes_library_index() {
                            let mut record = LibraryIndexRecord::from_anime_file(
                                &target,
                                &target_path,
                                &anime_file,
                            )?;
                            record.sha256_full = sha256_full;
                            remember_placed_subtitles(&mut record, &target, &placed_subtitles);
                            apply_runtime_probe(&mut record, &target, probe_runtime, args.verbose);
                            library_records.push(record);
                        }
                    }
                    Err(error) => {
                        eprintln!("放入提取资产失败 {}: {error}", anime_file.original_path);
                        log(&format!(
                            "Failed to place extracted assets for {}: {error}",
                            anime_file.original_path
                        ));
                        failed += 1;
                    }
                }
            }
            Err(_) => failed += 1,
        }
    }

    println!("处理完成：总计{processed}个文件，成功{succeeded}个，失败{failed}个");
    log(&format!(
        "Processed {processed} files: {succeeded} succeeded, {failed} failed"
    ));
    canceled |= cancel();
    finish_library_index(&args, &target, &extensions, &library_records)?;
    if args.delete_extracted_archives && !args.dry_run {
        delete_consumed_archives(&archives, &source, log)?;
    }
    if canceled {
        return Err(AppError::Canceled);
    }
    Ok(())
}

/// 带元数据刮削的流程
#[cfg(feature = "metadata")]
async fn run_with_metadata(
    args: OrganizeArgs,
    log: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<(), AppError> {
    ensure_not_canceled(cancel)?;
    validate_library_index_args(&args)?;
    validate_filename_parser_args(&args)?;
    let (source, target) = if args.refresh_library_metadata {
        let target = args.target.clone().ok_or_else(|| {
            AppError::ParseError("--refresh-library-metadata 必须提供 --target".to_string())
        })?;
        if !target.exists() {
            return Err(AppError::TargetNotFound(target));
        }
        (None, target)
    } else {
        let (source, target) = resolve_source_and_target(&args)?;
        (Some(source), target)
    };
    let fallback_mode = args
        .fallback_on_link_failure
        .map(FallbackMode::to_operation_mode);
    let extensions = build_extensions(&args.include_ext);
    let bangumi = Arc::new(BangumiClient::with_source(
        args.bangumi_cache.clone(),
        args.metadata_source.clone(),
    ));
    let alias_db_path = resolve_alias_db_path(
        &bangumi,
        args.metadata_source.as_deref(),
        args.verbose,
        args.refresh_library_metadata,
    )
    .await;
    let alias_lookup = Arc::new(AliasLookup::load_from_sources(
        alias_db_path.as_deref(),
        args.alias_file.as_deref(),
    )?);
    if args.verbose {
        if alias_lookup.is_empty() {
            eprintln!("未找到可用别名库，将仅使用 Bangumi 名称和搜索匹配");
        } else {
            eprintln!("已加载 {} 条别名", alias_lookup.len());
        }
    }

    match bangumi.prepare_index().await {
        Ok(count) => {
            if args.verbose {
                eprintln!("Bangumi 索引已就绪，共 {} 条动画条目", count);
            }
        }
        Err(error) => {
            if args.verbose {
                eprintln!("Bangumi 本地索引准备失败，将回退到在线查询: {error}");
            }
        }
    }

    let allow_online_title_resolution = args.metadata_source.is_none();
    let existing_library_bangumi_ids =
        Arc::new(match LibraryIndex::bangumi_ids_by_series_root(&target) {
            Ok(ids) => {
                if args.verbose && !ids.is_empty() {
                    eprintln!("已从 library.db 加载 {} 条 Bangumi 元数据缓存", ids.len());
                }
                ids
            }
            Err(error) => {
                if args.verbose {
                    eprintln!("读取 library.db 元数据缓存失败，将使用常规匹配: {error}");
                }
                HashMap::new()
            }
        });
    let library_bangumi_ids = if args.refresh_library_metadata {
        Arc::new(HashMap::new())
    } else {
        Arc::clone(&existing_library_bangumi_ids)
    };
    let tmdb = args.tmdb_api_key.clone().map(TmdbClient::new).map(Arc::new);
    if !args.no_images && tmdb.is_none() && args.verbose {
        eprintln!("未提供 TMDB API Key，将跳过 TMDB 图片下载");
    }
    let probe_runtime = runtime_probe_enabled(&args);

    if args.refresh_library_metadata {
        let mut records = LibraryIndex::records_for_metadata_refresh(&target)?;
        log(&format!(
            "Loaded {} media records from library.db without scanning the target",
            records.len()
        ));
        if args.dry_run {
            println!(
                "[dry-run] MLIP 元数据刷新: {} ({} 条记录，不扫描媒体目录)",
                LibraryIndex::database_path(&target).display(),
                records.len()
            );
            return Ok(());
        }
        let mut metadata_cache = HashMap::new();
        let mut episode_cache = HashMap::new();
        enrich_library_index_records(
            &mut records,
            &target,
            &mut MetadataIndexContext {
                metadata_cache: &mut metadata_cache,
                episode_cache: &mut episode_cache,
                alias_lookup: alias_lookup.as_ref(),
                bangumi: bangumi.as_ref(),
                tmdb: tmdb.as_deref(),
                library_bangumi_ids: library_bangumi_ids.as_ref(),
                artwork_bangumi_ids: existing_library_bangumi_ids.as_ref(),
                refresh_changed_artwork: true,
                download_images: !args.no_images,
                scan_artwork: true,
                force_overwrite: args.force_overwrite,
                fetch_episode_metadata: !args.no_episode_metadata,
                allow_online_title_resolution,
                probe_runtime: false,
                verbose: args.verbose,
            },
        )
        .await;
        ensure_not_canceled(cancel)?;
        let stats = LibraryIndex::refresh_metadata(&target, &records)?;
        println!(
            "媒体库元数据刷新完成：{} 部作品，{} 集，{} 个文件，{} 个特典 ({})",
            stats.series,
            stats.episodes,
            stats.media_files,
            stats.extras,
            LibraryIndex::database_path(&target).display()
        );
        log("MLIP metadata refresh finished and library.db was published");
        return Ok(());
    }

    let source = source.expect("non-refresh organize requires source");
    let subtitle_candidates = FileOrganizer::collect_external_subtitle_candidates(&source);
    let archives = load_source_archives(&source, args.scan_extracted_archives, log)?;
    let anime_groups =
        collect_anime_groups(&source, &extensions, args.filename_parser, args.verbose)?;
    log(&format!("Discovered {} anime groups", anime_groups.len()));
    let mut processed = 0;
    let mut succeeded = 0;
    let mut failed = 0;
    let (mut metadata_cache, mut episode_cache) = prefetch_group_metadata(
        &anime_groups,
        Arc::clone(&alias_lookup),
        Arc::clone(&bangumi),
        tmdb.clone(),
        target.clone(),
        Arc::clone(&library_bangumi_ids),
        allow_online_title_resolution,
        !args.no_episode_metadata,
        !args.no_images && !args.dry_run,
        args.force_overwrite,
        args.verbose,
    )
    .await;
    log(&format!(
        "Metadata prefetch completed for {} groups",
        metadata_cache.len()
    ));
    ensure_not_canceled(cancel)?;
    let mut canceled = false;
    let mut library_records = Vec::new();

    'groups: for (anime_name, files) in anime_groups {
        if cancel() {
            if processed == 0 {
                return Err(AppError::Canceled);
            }
            canceled = true;
            break;
        }
        let Some(first_file) = files.first() else {
            continue;
        };

        let series_name = first_file.series_name();
        let season_number = first_file.season_number().unwrap_or(1);
        let anime_root = target.join(&series_name);

        let cache_key = (anime_name.clone(), season_number);
        let metadata = if let Some(cached) = metadata_cache.get(&cache_key) {
            cached.clone()
        } else {
            let fetched = fetch_anime_metadata(
                MetadataLookup {
                    anime_name: &anime_name,
                    series_name: &series_name,
                    publisher_hint: Some(&first_file.publisher),
                    season_hint: first_file.season_number(),
                    cached_bangumi_id: library_bangumi_ids
                        .get(&(series_name.clone(), season_number))
                        .copied(),
                    allow_online_title_resolution,
                },
                alias_lookup.as_ref(),
                bangumi.as_ref(),
                tmdb.as_deref(),
                args.verbose,
            )
            .await;
            metadata_cache.insert(cache_key, fetched.clone());
            fetched
        };

        let episodes = if !args.no_episode_metadata {
            if let Some(ref meta) = metadata {
                fetch_bangumi_episodes_cached(
                    meta.bangumi_id,
                    bangumi.as_ref(),
                    &mut episode_cache,
                    args.verbose,
                )
                .await
            } else {
                None
            }
        } else {
            None
        };
        let group_min_episode = anime_group_min_episode(&files);

        if let Some(ref meta) = metadata {
            if args.scrape_metadata {
                let tvshow_nfo_path = anime_root.join("tvshow.nfo");
                if args.force_overwrite || !tvshow_nfo_path.exists() {
                    let nfo = TvShowNfo::from(meta);
                    if args.dry_run {
                        if args.verbose {
                            eprintln!("[dry-run] 生成 tvshow.nfo: {}", tvshow_nfo_path.display());
                        }
                    } else {
                        NfoWriter::write_tvshow(&anime_root, &nfo)?;
                        if args.verbose {
                            eprintln!("已生成 tvshow.nfo: {}", tvshow_nfo_path.display());
                        }
                    }
                }
            }
        }

        for file in files {
            if cancel() {
                if processed == 0 {
                    return Err(AppError::Canceled);
                }
                canceled = true;
                break 'groups;
            }
            let season_dir = organize_directory_components(&file, true)
                .iter()
                .fold(target.clone(), |path, component| path.join(component));
            processed += 1;

            match organize_file_to_dir(
                &file,
                &season_dir,
                args.mode,
                args.dry_run,
                fallback_mode,
                args.verbose,
                &subtitle_candidates,
                args.writes_library_index() && !args.dry_run,
                &source,
                log,
            ) {
                Ok((target_path, sha256_full)) => {
                    let placed_subtitles = match place_archive_assets(
                        Path::new(&file.original_path),
                        &target_path,
                        &archives,
                        args.mode,
                        args.dry_run,
                    ) {
                        Ok(placed_subtitles) => placed_subtitles,
                        Err(error) => {
                            eprintln!("放入提取资产失败 {}: {error}", file.original_path);
                            log(&format!(
                                "Failed to place extracted assets for {}: {error}",
                                file.original_path
                            ));
                            failed += 1;
                            continue;
                        }
                    };
                    succeeded += 1;

                    if args.writes_library_index() {
                        let mut record =
                            LibraryIndexRecord::from_anime_file(&target, &target_path, &file)?;
                        record.sha256_full = sha256_full;
                        remember_placed_subtitles(&mut record, &target, &placed_subtitles);
                        if let Some(ref meta) = metadata {
                            record.apply_metadata(meta);
                            apply_bangumi_episode_details(
                                &mut record,
                                episodes.as_deref(),
                                group_min_episode,
                            );
                            add_metadata_artwork(&mut record, &target, &anime_root, season_number);
                        }
                        apply_runtime_probe(&mut record, &target, probe_runtime, args.verbose);
                        library_records.push(record);
                    }

                    if args.scrape_metadata {
                        if let Some(ref meta) = metadata {
                            let episode_nfo_path = target_path.with_extension("nfo");
                            if args.force_overwrite || !episode_nfo_path.exists() {
                                let episode_nfo = create_episode_nfo(&file, meta);
                                if args.dry_run {
                                    if args.verbose {
                                        eprintln!(
                                            "[dry-run] 生成 episode.nfo: {}",
                                            episode_nfo_path.display()
                                        );
                                    }
                                } else if let Err(error) =
                                    NfoWriter::write_episode(&episode_nfo_path, &episode_nfo)
                                {
                                    eprintln!("生成 episode.nfo 失败: {error}");
                                }
                            }
                        }
                    }
                }
                Err(_) => failed += 1,
            }
        }
    }

    println!("处理完成：总计{processed}个文件，成功{succeeded}个，失败{failed}个");
    log(&format!(
        "Processed {processed} files: {succeeded} succeeded, {failed} failed"
    ));
    if !metadata_cache.is_empty() {
        let matched = metadata_cache
            .values()
            .filter(|item| item.is_some())
            .count();
        println!("元数据匹配：{matched}/{} 部动画", metadata_cache.len());
    }

    canceled |= cancel();
    finish_library_index_with_metadata(
        &args,
        &target,
        &extensions,
        &library_records,
        MetadataIndexContext {
            metadata_cache: &mut metadata_cache,
            episode_cache: &mut episode_cache,
            alias_lookup: alias_lookup.as_ref(),
            bangumi: bangumi.as_ref(),
            tmdb: tmdb.as_deref(),
            library_bangumi_ids: library_bangumi_ids.as_ref(),
            artwork_bangumi_ids: library_bangumi_ids.as_ref(),
            refresh_changed_artwork: false,
            download_images: !args.no_images,
            scan_artwork: true,
            force_overwrite: args.force_overwrite,
            fetch_episode_metadata: !args.no_episode_metadata,
            allow_online_title_resolution,
            probe_runtime,
            verbose: args.verbose,
        },
    )
    .await?;
    if args.delete_extracted_archives && !args.dry_run {
        delete_consumed_archives(&archives, &source, log)?;
    }

    if canceled {
        return Err(AppError::Canceled);
    }
    Ok(())
}

#[cfg(feature = "metadata")]
#[allow(clippy::too_many_arguments)]
async fn prefetch_group_metadata(
    anime_groups: &HashMap<String, Vec<AnimeFileInfo>>,
    alias_lookup: Arc<AliasLookup>,
    bangumi: Arc<BangumiClient>,
    tmdb: Option<Arc<TmdbClient>>,
    target: PathBuf,
    library_bangumi_ids: Arc<HashMap<(String, u32), u32>>,
    allow_online_title_resolution: bool,
    fetch_episode_metadata: bool,
    download_images_enabled: bool,
    force_overwrite: bool,
    verbose: bool,
) -> (
    HashMap<(String, u32), Option<AnimeMetadata>>,
    HashMap<u32, Option<Vec<BangumiEpisode>>>,
) {
    const CONCURRENCY: usize = 8;

    let permits = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut tasks = tokio::task::JoinSet::new();

    for (anime_name, files) in anime_groups {
        let Some(first_file) = files.first() else {
            continue;
        };
        let anime_name = anime_name.clone();
        let series_name = first_file.series_name();
        let publisher = first_file.publisher.clone();
        let season_hint = first_file.season_number();
        let season_number = season_hint.unwrap_or(1);
        let cached_bangumi_id = library_bangumi_ids
            .get(&(series_name.clone(), season_number))
            .copied();
        let alias_lookup = Arc::clone(&alias_lookup);
        let bangumi = Arc::clone(&bangumi);
        let tmdb = tmdb.clone();
        let target = target.clone();
        let permits = Arc::clone(&permits);

        tasks.spawn(async move {
            let Ok(_permit) = permits.acquire_owned().await else {
                return ((anime_name, season_number), None, None);
            };
            let metadata = fetch_anime_metadata(
                MetadataLookup {
                    anime_name: &anime_name,
                    series_name: &series_name,
                    publisher_hint: Some(&publisher),
                    season_hint,
                    cached_bangumi_id,
                    allow_online_title_resolution,
                },
                alias_lookup.as_ref(),
                bangumi.as_ref(),
                tmdb.as_deref(),
                verbose,
            )
            .await;
            if download_images_enabled {
                if let Some(meta) = metadata.as_ref() {
                    download_images(
                        meta,
                        &target.join(&series_name),
                        season_number,
                        bangumi.as_ref(),
                        tmdb.as_deref(),
                        force_overwrite,
                        verbose,
                    )
                    .await;
                }
            }
            let episodes = if fetch_episode_metadata {
                if let Some(meta) = metadata.as_ref() {
                    let bangumi_id = meta.bangumi_id;
                    let fetched = match bangumi.fetch_episodes(bangumi_id).await {
                        Ok(episodes) => {
                            if verbose {
                                eprintln!(
                                    "Bangumi 分集加载: {bangumi_id} -> {} 集",
                                    episodes.len()
                                );
                            }
                            Some(episodes)
                        }
                        Err(error) => {
                            if verbose {
                                eprintln!("Bangumi 分集加载失败 {bangumi_id}: {error}");
                            }
                            None
                        }
                    };
                    Some((bangumi_id, fetched))
                } else {
                    None
                }
            } else {
                None
            };
            ((anime_name, season_number), metadata, episodes)
        });
    }

    let mut metadata_cache = HashMap::new();
    let mut episode_cache = HashMap::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((key, metadata, episodes)) => {
                metadata_cache.insert(key, metadata);
                if let Some((bangumi_id, episodes)) = episodes {
                    episode_cache.entry(bangumi_id).or_insert(episodes);
                }
            }
            Err(error) if verbose => eprintln!("元数据预取任务失败: {error}"),
            Err(_) => {}
        }
    }

    (metadata_cache, episode_cache)
}

#[allow(clippy::too_many_arguments)]
fn cleanup_moved_source_directories(
    source: &Path,
    subtitles: &[PathBuf],
    source_root: &Path,
    log: &dyn Fn(&str),
) -> Result<(), AppError> {
    let mut directories = subtitles
        .iter()
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .chain(source.parent().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
    directories.dedup();
    for directory in directories {
        match FileOrganizer::cleanup_empty_ancestors(&directory, source_root) {
            Ok(removed) => {
                for path in removed {
                    log(&format!(
                        "Removed empty source directory {}",
                        path.display()
                    ));
                }
            }
            Err(error) => log(&format!(
                "Failed to clean empty source directory {}: {error}",
                directory.display()
            )),
        }
    }
    Ok(())
}

fn archive_search_root(source: &Path) -> &Path {
    if source.is_dir() {
        source
    } else {
        source.parent().unwrap_or(source)
    }
}

fn load_source_archives(
    source: &Path,
    scan_with_defender: bool,
    log: &dyn Fn(&str),
) -> Result<Vec<PreparedArchive>, AppError> {
    let archives = prepare_source_archives(
        archive_search_root(source),
        is_supplemental_video_path,
        &IngestOptions {
            limits: ArchiveLimits::default(),
            scan_with_defender,
        },
    )?;
    for archive in &archives {
        for warning in &archive.warnings {
            eprintln!("压缩包 {}：{warning}", archive.source.display());
            log(&format!("Archive {}: {warning}", archive.source.display()));
        }
    }
    Ok(archives)
}

fn place_archive_assets(
    video_path: &Path,
    target_video: &Path,
    archives: &[PreparedArchive],
    mode: OperationMode,
    dry_run: bool,
) -> Result<Vec<PathBuf>, AppError> {
    let Some(video_name) = video_path.file_name().and_then(|name| name.to_str()) else {
        return Ok(Vec::new());
    };
    let Some(video_dir) = video_path.parent() else {
        return Ok(Vec::new());
    };
    let Some(target_dir) = target_video.parent() else {
        return Ok(Vec::new());
    };
    let mut placed_subtitles = Vec::new();
    for archive in archives.iter().filter(|archive| !archive.rejected) {
        if archive.directory != video_dir {
            continue;
        }
        let mut placed = false;
        for subtitle in archive.subtitles_for(video_name) {
            let target = target_dir.join(subtitle_target_name(video_name, subtitle.file_name()));
            FileOrganizer::place_extracted_asset(&subtitle.absolute_path, &target, mode, dry_run)?;
            placed_subtitles.push(target);
            placed = true;
        }
        for font in archive.fonts_for(video_name) {
            let target = join_relative(target_dir, &font.relative_path);
            FileOrganizer::place_extracted_asset(&font.absolute_path, &target, mode, dry_run)?;
            placed = true;
        }
        if placed && !dry_run {
            archive.mark_consumed();
        }
    }
    Ok(placed_subtitles)
}

fn remember_placed_subtitles(
    record: &mut LibraryIndexRecord,
    target: &Path,
    subtitles: &[PathBuf],
) {
    for subtitle in subtitles {
        let Ok(relative) = subtitle.strip_prefix(target) else {
            continue;
        };
        let relative = relative
            .components()
            .filter_map(|component| match component {
                std::path::Component::Normal(value) => value.to_str().map(str::to_string),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("/");
        if !relative.is_empty() && !record.subtitle_paths.iter().any(|path| path == &relative) {
            record.subtitle_paths.push(relative);
        }
    }
}

fn delete_consumed_archives(
    archives: &[PreparedArchive],
    source: &Path,
    log: &dyn Fn(&str),
) -> Result<(), AppError> {
    let root = archive_search_root(source);
    for archive in archives {
        if archive.rejected || !archive.consumed() {
            continue;
        }
        std::fs::remove_file(&archive.source)?;
        eprintln!("已删除已提取的压缩包 {}", archive.source.display());
        log(&format!(
            "Deleted extracted archive {}",
            archive.source.display()
        ));
        if let Some(parent) = archive.source.parent() {
            match FileOrganizer::cleanup_empty_ancestors(parent, root) {
                Ok(removed) => {
                    for path in removed {
                        log(&format!(
                            "Removed empty source directory {}",
                            path.display()
                        ));
                    }
                }
                Err(error) => log(&format!(
                    "Failed to clean empty source directory after archive deletion: {error}"
                )),
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn organize_file_to_dir(
    anime_file: &AnimeFileInfo,
    target_dir: &Path,
    mode: OperationMode,
    dry_run: bool,
    fallback_mode: Option<OperationMode>,
    verbose: bool,
    subtitle_candidates: &[PathBuf],
    hash_source: bool,
    source_root: &Path,
    log: &dyn Fn(&str),
) -> Result<(PathBuf, Option<String>), AppError> {
    let source_hash = if hash_source {
        match sha256_file(Path::new(&anime_file.original_path)) {
            Ok(hash) => Some(hash),
            Err(error) => {
                eprintln!(
                    "计算文件 SHA-256 失败 {}: {error}",
                    anime_file.original_path
                );
                log(&format!(
                    "Failed to hash {}: {error}",
                    anime_file.original_path
                ));
                return Err(error);
            }
        }
    } else {
        None
    };
    let subtitles = FileOrganizer::find_external_subtitles_from(
        Path::new(&anime_file.original_path),
        subtitle_candidates,
    );
    match FileOrganizer::organize_to_dir_with_subtitles(
        anime_file, target_dir, mode, dry_run, &subtitles,
    ) {
        Ok(target_path) => {
            if mode == OperationMode::Move && !dry_run {
                cleanup_moved_source_directories(
                    Path::new(&anime_file.original_path),
                    &subtitles,
                    source_root,
                    log,
                )?;
            }
            if verbose && !dry_run {
                println!(
                    "成功: {} -> {}",
                    anime_file.original_path,
                    target_path.display()
                );
            }
            log(&format!("Organized {}", target_path.display()));
            Ok((target_path, source_hash))
        }
        Err(error) => {
            if mode == OperationMode::Link {
                if let Some(fallback) = fallback_mode {
                    if matches!(
                        error,
                        AppError::CrossDeviceLink | AppError::HardLinkNotSupported
                    ) {
                        if verbose {
                            eprintln!(
                                "硬链接失败，回退为 {}: {}",
                                fallback, anime_file.original_path
                            );
                        }

                        return FileOrganizer::organize_to_dir_with_subtitles(
                            anime_file, target_dir, fallback, dry_run, &subtitles,
                        )
                        .and_then(|target_path| {
                            if fallback == OperationMode::Move && !dry_run {
                                cleanup_moved_source_directories(
                                    Path::new(&anime_file.original_path),
                                    &subtitles,
                                    source_root,
                                    log,
                                )?;
                            }
                            Ok((target_path, source_hash))
                        })
                        .map_err(|fallback_error| {
                            eprintln!(
                                "处理文件失败 {}: {fallback_error}",
                                anime_file.original_path
                            );
                            fallback_error
                        });
                    }
                }
            }

            eprintln!("处理文件失败 {}: {error}", anime_file.original_path);
            log(&format!("Failed {}: {error}", anime_file.original_path));
            Err(error)
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum LibraryIndexWriteMode {
    Initialize,
    Incremental,
    Rebuild,
}

impl LibraryIndexWriteMode {
    fn label(self) -> &'static str {
        match self {
            Self::Initialize => "初始化",
            Self::Incremental => "增量更新",
            Self::Rebuild => "重建",
        }
    }
}

fn validate_library_index_args(args: &OrganizeArgs) -> Result<(), AppError> {
    if args.rebuild_library_index && !args.writes_library_index() {
        return Err(AppError::ParseError(
            "--rebuild-library-index 必须与 --library-index 或 --mlip 一起使用".to_string(),
        ));
    }
    if args.refresh_library_metadata && !args.mlip {
        return Err(AppError::ParseError(
            "--refresh-library-metadata 必须与 --mlip 一起使用".to_string(),
        ));
    }
    if args.refresh_library_metadata && args.rebuild_library_index {
        return Err(AppError::ParseError(
            "--refresh-library-metadata 不能与 --rebuild-library-index 同时使用".to_string(),
        ));
    }
    if args.refresh_library_metadata && args.probe_runtime {
        return Err(AppError::ParseError(
            "--refresh-library-metadata 不读取媒体文件，不能与 --probe-runtime 同时使用"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_filename_parser_args(args: &OrganizeArgs) -> Result<(), AppError> {
    let _ = args;
    #[cfg(not(feature = "anifilebert"))]
    if args.filename_parser == FilenameParserMode::Anifilebert {
        return Err(AppError::ParseError(
            "--filename-parser anifilebert 需要使用 --features anifilebert 编译".to_string(),
        ));
    }

    Ok(())
}

fn resolve_library_index_mode(args: &OrganizeArgs, target: &Path) -> Option<LibraryIndexWriteMode> {
    if !args.writes_library_index() {
        return None;
    }

    if args.rebuild_library_index {
        return Some(LibraryIndexWriteMode::Rebuild);
    }

    if LibraryIndex::database_path(target).exists() {
        Some(LibraryIndexWriteMode::Incremental)
    } else {
        Some(LibraryIndexWriteMode::Initialize)
    }
}

fn finish_library_index(
    args: &OrganizeArgs,
    target: &Path,
    extensions: &HashSet<String>,
    current_records: &[LibraryIndexRecord],
) -> Result<(), AppError> {
    let Some(mode) = resolve_library_index_mode(args, target) else {
        return Ok(());
    };

    if args.dry_run {
        let scan_count = if matches!(
            mode,
            LibraryIndexWriteMode::Initialize | LibraryIndexWriteMode::Rebuild
        ) {
            let records = collect_target_library_records(
                target,
                extensions,
                args.filename_parser,
                args.verbose,
            )?;
            let extras = collect_target_library_extras(target, extensions, &records)?;
            records.len() + extras.len()
        } else {
            current_records.len()
        };
        eprintln!(
            "[dry-run] MLIP {}: {} ({} 条记录)",
            mode.label(),
            LibraryIndex::database_path(target).display(),
            scan_count
        );
        return Ok(());
    }

    let stats = match mode {
        LibraryIndexWriteMode::Initialize | LibraryIndexWriteMode::Rebuild => {
            let mut records = collect_target_library_records(
                target,
                extensions,
                args.filename_parser,
                args.verbose,
            )?;
            merge_current_media_hashes(&mut records, current_records);
            let probe_runtime = runtime_probe_enabled(args);
            apply_runtime_probe_to_records(&mut records, target, probe_runtime, args.verbose);
            let extras = collect_target_library_extras(target, extensions, &records)?;
            LibraryIndex::rebuild_with_extras(target, &records, &extras)?
        }
        LibraryIndexWriteMode::Incremental => LibraryIndex::update(target, current_records)?,
    };

    println!(
        "媒体库索引{}完成：{} 部作品，{} 集，{} 个文件，{} 个特典 ({})",
        mode.label(),
        stats.series,
        stats.episodes,
        stats.media_files,
        stats.extras,
        LibraryIndex::database_path(target).display()
    );
    Ok(())
}

fn merge_current_media_hashes(
    records: &mut [LibraryIndexRecord],
    current_records: &[LibraryIndexRecord],
) {
    let hashes = current_records
        .iter()
        .filter_map(|record| {
            record.sha256_full.as_ref().map(|hash| {
                (
                    (
                        record.relative_path.as_str(),
                        record.size,
                        record.modified_time,
                    ),
                    hash,
                )
            })
        })
        .collect::<HashMap<_, _>>();
    for record in records {
        if let Some(hash) = hashes.get(&(
            record.relative_path.as_str(),
            record.size,
            record.modified_time,
        )) {
            record.sha256_full = Some((*hash).clone());
        }
    }
}

fn collect_target_library_records(
    target: &Path,
    extensions: &HashSet<String>,
    filename_parser: FilenameParserMode,
    verbose: bool,
) -> Result<Vec<LibraryIndexRecord>, AppError> {
    let mut records = Vec::new();

    for entry in WalkDir::new(target)
        .into_iter()
        .filter_map(|item| item.ok())
        .filter(|item| item.file_type().is_file())
    {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("library.db"))
        {
            continue;
        }
        if !has_valid_extension(path, extensions) {
            continue;
        }

        let record = match LibraryIndexRecord::from_target_path(target, path)? {
            some @ Some(_) => some,
            None if filename_parser != FilenameParserMode::Rules
                && !path.ancestors().any(is_supplemental_video_path) =>
            {
                parse_anime_file(path, filename_parser, verbose)?
                    .map(|info| LibraryIndexRecord::from_anime_file(target, path, &info))
                    .transpose()?
            }
            None => None,
        };
        match record {
            Some(record) => records.push(record),
            None if verbose => eprintln!(
                "跳过：无法加入媒体库索引 {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            None => {}
        }
    }

    Ok(records)
}

fn collect_target_library_extras(
    target: &Path,
    extensions: &HashSet<String>,
    records: &[LibraryIndexRecord],
) -> Result<Vec<LibraryExtraRecord>, AppError> {
    let mut titles_by_root: HashMap<&str, HashSet<&str>> = HashMap::new();
    for record in records {
        let Some(root) = record.relative_path.split('/').next() else {
            continue;
        };
        titles_by_root
            .entry(root)
            .or_default()
            .insert(&record.series_title);
    }
    let owners: HashMap<&str, &str> = titles_by_root
        .into_iter()
        .filter_map(|(root, titles)| {
            (titles.len() == 1).then(|| (root, *titles.iter().next().expect("one title")))
        })
        .collect();
    let mut extras = Vec::new();
    for entry in WalkDir::new(target)
        .into_iter()
        .filter_map(|item| item.ok())
        .filter(|item| item.file_type().is_file())
    {
        let path = entry.path();
        if !has_valid_extension(path, extensions) {
            continue;
        }
        let Some(root) = path
            .strip_prefix(target)
            .ok()
            .and_then(|relative| relative.components().next())
            .and_then(|component| component.as_os_str().to_str())
        else {
            continue;
        };
        let Some(series_title) = owners.get(root) else {
            continue;
        };
        if let Some(extra) = LibraryExtraRecord::from_target_path(target, path, *series_title)? {
            extras.push(extra);
        }
    }
    Ok(extras)
}

fn runtime_probe_enabled(args: &OrganizeArgs) -> bool {
    if !args.probe_runtime || args.dry_run || !args.writes_library_index() {
        return false;
    }

    match Command::new("ffprobe").arg("-version").output() {
        Ok(output) if output.status.success() => true,
        _ => {
            eprintln!("--probe-runtime 需要 PATH 中存在 ffprobe，将跳过时长探测");
            false
        }
    }
}

fn apply_runtime_probe_to_records(
    records: &mut [LibraryIndexRecord],
    target: &Path,
    probe_runtime: bool,
    verbose: bool,
) {
    if !probe_runtime {
        return;
    }

    for record in records {
        apply_runtime_probe(record, target, true, verbose);
    }
}

fn apply_runtime_probe(
    record: &mut LibraryIndexRecord,
    target: &Path,
    probe_runtime: bool,
    verbose: bool,
) {
    if !probe_runtime {
        return;
    }

    let media_path = target.join(&record.relative_path);
    if let Some(seconds) = probe_media_runtime_seconds(&media_path, verbose) {
        record.runtime = Some(seconds);
    }
}

fn probe_media_runtime_seconds(path: &Path, verbose: bool) -> Option<i64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            if verbose {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("ffprobe 读取时长失败 {}: {}", path.display(), stderr.trim());
            }
            return None;
        }
        Err(error) => {
            if verbose {
                eprintln!("ffprobe 启动失败 {}: {error}", path.display());
            }
            return None;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let duration = stdout
        .lines()
        .find_map(|line| line.trim().parse::<f64>().ok())?;
    if !duration.is_finite() || duration <= 0.0 || duration > i64::MAX as f64 {
        return None;
    }

    Some(duration.round() as i64)
}

#[cfg(feature = "metadata")]
struct MetadataIndexContext<'a> {
    metadata_cache: &'a mut HashMap<(String, u32), Option<AnimeMetadata>>,
    episode_cache: &'a mut HashMap<u32, Option<Vec<BangumiEpisode>>>,
    alias_lookup: &'a AliasLookup,
    bangumi: &'a BangumiClient,
    tmdb: Option<&'a TmdbClient>,
    library_bangumi_ids: &'a HashMap<(String, u32), u32>,
    artwork_bangumi_ids: &'a HashMap<(String, u32), u32>,
    refresh_changed_artwork: bool,
    download_images: bool,
    scan_artwork: bool,
    force_overwrite: bool,
    fetch_episode_metadata: bool,
    allow_online_title_resolution: bool,
    probe_runtime: bool,
    verbose: bool,
}

#[cfg(feature = "metadata")]
fn force_artwork_refresh(
    explicit_force: bool,
    refresh_changed_artwork: bool,
    previous_ids: &HashMap<(String, u32), u32>,
    series_root: &str,
    season: u32,
    resolved_bangumi_id: u32,
) -> bool {
    explicit_force
        || (refresh_changed_artwork
            && previous_ids
                .get(&(series_root.to_string(), season))
                .is_some_and(|previous| *previous != resolved_bangumi_id))
}

#[cfg(feature = "metadata")]
async fn finish_library_index_with_metadata(
    args: &OrganizeArgs,
    target: &Path,
    extensions: &HashSet<String>,
    current_records: &[LibraryIndexRecord],
    mut context: MetadataIndexContext<'_>,
) -> Result<(), AppError> {
    let Some(mode) = resolve_library_index_mode(args, target) else {
        return Ok(());
    };

    if args.dry_run {
        let scan_count = if matches!(
            mode,
            LibraryIndexWriteMode::Initialize | LibraryIndexWriteMode::Rebuild
        ) {
            let records = collect_target_library_records(
                target,
                extensions,
                args.filename_parser,
                args.verbose,
            )?;
            let extras = collect_target_library_extras(target, extensions, &records)?;
            records.len() + extras.len()
        } else {
            current_records.len()
        };
        eprintln!(
            "[dry-run] MLIP {}: {} ({} 条记录)",
            mode.label(),
            LibraryIndex::database_path(target).display(),
            scan_count
        );
        return Ok(());
    }

    let stats = match mode {
        LibraryIndexWriteMode::Initialize | LibraryIndexWriteMode::Rebuild => {
            let mut records = collect_target_library_records(
                target,
                extensions,
                args.filename_parser,
                args.verbose,
            )?;
            merge_current_media_hashes(&mut records, current_records);
            enrich_library_index_records(&mut records, target, &mut context).await;
            let extras = collect_target_library_extras(target, extensions, &records)?;
            LibraryIndex::rebuild_with_extras(target, &records, &extras)?
        }
        LibraryIndexWriteMode::Incremental => LibraryIndex::update(target, current_records)?,
    };

    println!(
        "媒体库索引{}完成：{} 部作品，{} 集，{} 个文件，{} 个特典 ({})",
        mode.label(),
        stats.series,
        stats.episodes,
        stats.media_files,
        stats.extras,
        LibraryIndex::database_path(target).display()
    );
    Ok(())
}

#[cfg(feature = "metadata")]
async fn enrich_library_index_records(
    records: &mut [LibraryIndexRecord],
    target: &Path,
    context: &mut MetadataIndexContext<'_>,
) {
    let min_episodes = min_episode_by_series(records);
    let mut downloaded_artwork = HashSet::new();

    for record in records {
        let lookup_title = record.series_title.clone();
        let season = u32::try_from(record.season).unwrap_or(1);
        let season_hint = (season > 1).then_some(season);
        let cache_key = (lookup_title.clone(), season);
        let series_root = record
            .relative_path
            .split('/')
            .next()
            .unwrap_or(&lookup_title)
            .to_string();
        let metadata = if let Some(cached) = context.metadata_cache.get(&cache_key) {
            cached.clone()
        } else {
            let publisher = FilenameParser::parse(target.join(&record.relative_path))
                .map(|file| file.publisher);
            let fetched = fetch_anime_metadata(
                MetadataLookup {
                    anime_name: &lookup_title,
                    series_name: &lookup_title,
                    publisher_hint: publisher.as_deref(),
                    season_hint,
                    cached_bangumi_id: context
                        .library_bangumi_ids
                        .get(&(series_root.clone(), season))
                        .copied(),
                    allow_online_title_resolution: context.allow_online_title_resolution,
                },
                context.alias_lookup,
                context.bangumi,
                context.tmdb,
                context.verbose,
            )
            .await;
            context.metadata_cache.insert(cache_key, fetched.clone());
            fetched
        };

        if let Some(ref meta) = metadata {
            let episodes = if context.fetch_episode_metadata {
                fetch_bangumi_episodes_cached(
                    meta.bangumi_id,
                    context.bangumi,
                    context.episode_cache,
                    context.verbose,
                )
                .await
            } else {
                None
            };
            let anime_root = library_series_root(target, record, &lookup_title);
            if context.download_images
                && downloaded_artwork.insert((anime_root.clone(), season.max(1)))
            {
                download_images(
                    meta,
                    &anime_root,
                    season.max(1),
                    context.bangumi,
                    context.tmdb,
                    force_artwork_refresh(
                        context.force_overwrite,
                        context.refresh_changed_artwork,
                        context.artwork_bangumi_ids,
                        &series_root,
                        season,
                        meta.bangumi_id,
                    ),
                    context.verbose,
                )
                .await;
            }
            record.apply_metadata(meta);
            apply_bangumi_episode_details(
                record,
                episodes.as_deref(),
                min_episodes
                    .get(&(lookup_title.clone(), i64::from(season)))
                    .copied(),
            );
            if context.scan_artwork {
                add_metadata_artwork(record, target, &anime_root, season.max(1));
            }
        }
        apply_runtime_probe(record, target, context.probe_runtime, context.verbose);
    }
}

#[cfg(feature = "metadata")]
fn library_series_root(
    target: &Path,
    record: &LibraryIndexRecord,
    fallback_title: &str,
) -> PathBuf {
    let directory = record
        .relative_path
        .split('/')
        .next()
        .filter(|component| !component.is_empty())
        .unwrap_or(fallback_title);
    target.join(directory)
}

#[cfg(feature = "metadata")]
fn add_metadata_artwork(
    record: &mut LibraryIndexRecord,
    target: &Path,
    anime_root: &Path,
    season_number: u32,
) {
    add_series_artwork_if_exists(
        record,
        target,
        ArtworkKind::Poster,
        &anime_root.join("poster.jpg"),
    );
    add_series_artwork_if_exists(
        record,
        target,
        ArtworkKind::Fanart,
        &anime_root.join("fanart.jpg"),
    );
    add_series_artwork_if_exists(
        record,
        target,
        ArtworkKind::SeasonPoster,
        &anime_root.join(format!("season{season_number:02}-poster.jpg")),
    );
}

#[cfg(feature = "metadata")]
fn add_series_artwork_if_exists(
    record: &mut LibraryIndexRecord,
    target: &Path,
    kind: ArtworkKind,
    path: &Path,
) {
    if !path.exists() {
        return;
    }

    if let Some(relative) = path.strip_prefix(target).ok().map(normalized_relative_path) {
        let mut artwork = Artwork::new(kind, relative);
        if let Some(source) = artwork_source(path) {
            let provider = match source.source_provider {
                1 => Some(ExternalProvider::Bangumi),
                2 => Some(ExternalProvider::Tmdb),
                3 => Some(ExternalProvider::Anidb),
                _ => None,
            };
            if let Some(provider) = provider {
                artwork = artwork.with_source(
                    provider,
                    source.source_subject_id,
                    Some(source.source_url),
                    Some(source.downloaded_at),
                );
            }
        }
        if let Some(existing) = record
            .series_artwork
            .iter_mut()
            .find(|existing| existing.kind == artwork.kind && existing.path == artwork.path)
        {
            *existing = artwork;
        } else {
            record.series_artwork.push(artwork);
        }
    }
}

#[cfg(feature = "metadata")]
fn normalized_relative_path(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => value.to_str().map(ToOwned::to_owned),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn parse_anime_file(
    path: &Path,
    mode: FilenameParserMode,
    verbose: bool,
) -> Result<Option<AnimeFileInfo>, AppError> {
    match mode {
        FilenameParserMode::Rules => Ok(FilenameParser::parse(path).map(sanitize_anime_file)),
        FilenameParserMode::Anifilebert => {
            Ok(parse_anifilebert(path, true, verbose)?.map(sanitize_anime_file))
        }
        FilenameParserMode::Auto => {
            let rules = FilenameParser::parse(path);
            // 规则解析器"成功"不等于名字干净：末尾残留 `[中文配音]` 这类标签时，
            // 元数据匹配会整体失配，这种情况也交给模型重新解析。
            let rules_need_cleanup = rules
                .as_ref()
                .is_some_and(|info| strip_residual_tags(&info.anime_name) != info.anime_name);
            if let (Some(info), false) = (rules.as_ref(), rules_need_cleanup) {
                return Ok(Some(sanitize_anime_file(info.clone())));
            }
            match parse_anifilebert(path, false, verbose)? {
                Some(info) => Ok(Some(sanitize_anime_file(info))),
                None => Ok(rules.map(sanitize_anime_file)),
            }
        }
    }
}

/// 去掉番名末尾残留的发布标签，让整理出的目录名与元数据查询都保持干净。
fn sanitize_anime_file(info: AnimeFileInfo) -> AnimeFileInfo {
    let cleaned = strip_residual_tags(&info.anime_name);
    if cleaned.is_empty() || cleaned == info.anime_name {
        return info;
    }
    AnimeFileInfo {
        anime_name: cleaned,
        ..info
    }
}

#[cfg(feature = "anifilebert")]
fn parse_anifilebert(
    path: &Path,
    required: bool,
    verbose: bool,
) -> Result<Option<AnimeFileInfo>, AppError> {
    match anime_organizer::anifilebert::parse_path(path) {
        Ok(info) => Ok(info),
        Err(error) if required => Err(AppError::ParseError(format!(
            "AniFileBERT 解析器初始化或推理失败: {error}"
        ))),
        Err(error) => {
            if verbose && ANIFILEBERT_AUTO_WARNED.set(()).is_ok() {
                eprintln!("AniFileBERT 回退不可用，将继续使用规则解析器: {error}");
            }
            Ok(None)
        }
    }
}

#[cfg(not(feature = "anifilebert"))]
fn parse_anifilebert(
    _path: &Path,
    required: bool,
    verbose: bool,
) -> Result<Option<AnimeFileInfo>, AppError> {
    if required {
        return Err(AppError::ParseError(
            "AniFileBERT 解析器未启用，请使用 --features anifilebert 编译".to_string(),
        ));
    }
    if verbose && ANIFILEBERT_AUTO_WARNED.set(()).is_ok() {
        eprintln!("AniFileBERT 回退未启用，将继续使用规则解析器");
    }
    Ok(None)
}

fn collect_anime_groups(
    source: &Path,
    extensions: &HashSet<String>,
    filename_parser: FilenameParserMode,
    verbose: bool,
) -> Result<HashMap<String, Vec<AnimeFileInfo>>, AppError> {
    let mut groups: HashMap<String, Vec<AnimeFileInfo>> = HashMap::new();

    for entry in WalkDir::new(source)
        .into_iter()
        .filter_entry(|entry| !is_supplemental_video_path(entry.path()))
        .filter_map(|item| item.ok())
        .filter(|item| item.file_type().is_file())
    {
        let path = entry.path();
        if !has_valid_extension(path, extensions) {
            continue;
        }

        if let Some(info) = parse_anime_file(path, filename_parser, verbose)? {
            groups
                .entry(info.anime_name.clone())
                .or_default()
                .push(info);
        } else if verbose {
            eprintln!(
                "跳过：无法解析文件名 {}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
        }
    }

    Ok(groups)
}

fn resolve_source_and_target(args: &OrganizeArgs) -> Result<(PathBuf, PathBuf), AppError> {
    let source = args.source.clone().ok_or_else(|| {
        AppError::ParseError("整理模式下必须提供 --source；若要使用工作流子命令，请执行 aniorg scrape 或 aniorg match".to_string())
    })?;

    if !source.exists() {
        return Err(AppError::SourceNotFound(source));
    }

    let target = args.target.clone().unwrap_or_else(|| source.clone());
    if !target.exists() {
        return Err(AppError::TargetNotFound(target));
    }

    Ok((source, target))
}

fn build_extensions(include_ext: &Option<Vec<String>>) -> HashSet<String> {
    match include_ext {
        Some(exts) => exts
            .iter()
            .map(|ext| {
                if ext.starts_with('.') {
                    ext.to_lowercase()
                } else {
                    format!(".{}", ext.to_lowercase())
                }
            })
            .collect(),
        None => DEFAULT_EXTENSIONS
            .iter()
            .map(|ext| (*ext).to_string())
            .collect(),
    }
}

#[cfg(feature = "metadata")]
async fn resolve_alias_db_path(
    bangumi: &BangumiClient,
    metadata_source: Option<&Path>,
    verbose: bool,
    force_refresh: bool,
) -> Option<PathBuf> {
    if let Some(path) = find_metadata_source_alias_db(metadata_source) {
        return Some(path);
    }

    let animeatlas_path = bangumi.cache_dir().join(ANIMEATLAS_SQLITE_FILENAME);
    let bangumi_path = bangumi.cache_dir().join("bangumi.db");
    if metadata_source.is_some() {
        return animeatlas_path
            .is_file()
            .then_some(animeatlas_path)
            .or_else(|| bangumi_path.is_file().then_some(bangumi_path));
    }

    // 周更的全量库优先：它同时包含中文名与 infobox 别名。
    //（AnimeAtlas 只有 91 部手工作品，实测全量库是 3 万部 / 2 万条别名。）
    let allow_download = crate::metadata_settings::MetadataSettings::load(
        &crate::metadata_settings::MetadataSettings::path(),
    )
    .unwrap_or_default()
    .auto_download_alias_db;
    if let Some(path) =
        ensure_bangumi_alias_db(bangumi.cache_dir(), force_refresh, allow_download, verbose).await
    {
        if verbose {
            eprintln!("使用全量别名库: {}", path.display());
        }
        return Some(path);
    }

    match download_animeatlas_alias_db(bangumi.cache_dir(), force_refresh).await {
        Ok(path) => {
            if verbose {
                eprintln!("AnimeAtlas 别名库已就绪: {}", path.display());
            }
            Some(path)
        }
        Err(error) => {
            if verbose {
                eprintln!("AnimeAtlas 别名库下载失败，将回退到本地缓存、本地 bangumi.db 或在线搜索: {error}");
            }
            animeatlas_path
                .is_file()
                .then_some(animeatlas_path)
                .or_else(|| bangumi_path.is_file().then_some(bangumi_path))
        }
    }
}

/// 确保本地有周更的全量别名库 `bangumi.db`，必要时下载。
///
/// 这份库由 `.github/workflows/bangumi-db.yml` 每周日从 Bangumi Archive 重建，
/// 覆盖全部动画并带 infobox 别名（实测 31,258 部 / 20,196 条），
/// 比 91 部的手工 AnimeAtlas 高一到两个数量级。
/// 任何一步失败都返回既有缓存（可能过期），由调用方继续回退 AnimeAtlas。
/// 是否允许下载由 `metadata.json` 的 `auto_download_alias_db` 控制（WebUI 可改）。
#[cfg(feature = "metadata")]
pub(crate) async fn ensure_bangumi_alias_db(
    cache_dir: &Path,
    force_refresh: bool,
    allow_download: bool,
    verbose: bool,
) -> Option<PathBuf> {
    let db_path = cache_dir.join(BANGUMI_ALIAS_DB_FILENAME);
    if !force_refresh && bangumi_alias_db_is_fresh(&db_path) {
        return Some(db_path);
    }
    // 设置里关掉了自动下载时，只认本地已有文件（可能过期）。
    if !allow_download {
        return db_path.is_file().then_some(db_path);
    }

    match download_bangumi_alias_db(cache_dir).await {
        Ok(()) => {
            if verbose {
                eprintln!("全量别名库已更新: {}", db_path.display());
            }
            Some(db_path)
        }
        Err(error) => {
            if verbose {
                eprintln!("全量别名库下载失败，将继续使用现有缓存或回退 AnimeAtlas: {error}");
            }
            db_path.is_file().then_some(db_path)
        }
    }
}

#[cfg(feature = "metadata")]
fn bangumi_alias_db_is_fresh(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if metadata.len() < BANGUMI_ALIAS_DB_MIN_BYTES {
        return false;
    }
    match metadata.modified() {
        Ok(modified) => modified
            .elapsed()
            .map(|age| age < BANGUMI_ALIAS_DB_TTL)
            .unwrap_or(false),
        Err(_) => false,
    }
}

#[cfg(feature = "metadata")]
async fn download_bangumi_alias_db(cache_dir: &Path) -> Result<(), AppError> {
    let _refresh_guard = BANGUMI_ALIAS_DB_LOCK.lock().await;
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| AppError::MetadataFetchError(format!("创建缓存目录失败: {e}")))?;

    let db_path = cache_dir.join(BANGUMI_ALIAS_DB_FILENAME);
    let tmp_path = cache_dir.join(format!("{BANGUMI_ALIAS_DB_FILENAME}.part"));
    let client = reqwest::Client::builder()
        .user_agent(HTTP_USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| AppError::MetadataFetchError(format!("创建 HTTP 客户端失败: {e}")))?;
    let mut resp = client
        .get(BANGUMI_ALIAS_DB_URL)
        .send()
        .await
        .map_err(|e| AppError::MetadataFetchError(format!("下载全量别名库失败: {e}")))?;
    if !resp.status().is_success() {
        return Err(AppError::MetadataFetchError(format!(
            "下载全量别名库失败 (HTTP {})",
            resp.status()
        )));
    }

    let mut file = tokio::fs::File::create(&tmp_path)
        .await
        .map_err(|e| AppError::MetadataFetchError(format!("创建临时文件失败: {e}")))?;
    let mut written: u64 = 0;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| AppError::MetadataFetchError(format!("读取全量别名库失败: {e}")))?
    {
        file.write_all(&chunk)
            .await
            .map_err(|e| AppError::MetadataFetchError(format!("写入全量别名库失败: {e}")))?;
        written += chunk.len() as u64;
    }
    file.flush()
        .await
        .map_err(|e| AppError::MetadataFetchError(format!("写入全量别名库失败: {e}")))?;
    drop(file);

    if written < BANGUMI_ALIAS_DB_MIN_BYTES {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(AppError::MetadataFetchError(format!(
            "全量别名库大小异常: {written} 字节"
        )));
    }
    std::fs::rename(&tmp_path, &db_path)
        .map_err(|e| AppError::MetadataFetchError(format!("替换全量别名库失败: {e}")))?;
    Ok(())
}

#[cfg(feature = "metadata")]
fn find_metadata_source_alias_db(metadata_source: Option<&Path>) -> Option<PathBuf> {
    let source = metadata_source?;
    let parent = if source.is_dir() {
        source
    } else {
        source.parent()?
    };
    [ANIMEATLAS_SQLITE_FILENAME, "bangumi.db"]
        .into_iter()
        .map(|name| parent.join(name))
        .find(|path| path.is_file())
}

#[cfg(feature = "metadata")]
pub(crate) async fn download_animeatlas_alias_db(
    cache_dir: &Path,
    force_refresh: bool,
) -> Result<PathBuf, AppError> {
    let _refresh_guard = ANIMEATLAS_REFRESH_LOCK.lock().await;
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| AppError::MetadataFetchError(format!("创建缓存目录失败: {e}")))?;

    let db_path = cache_dir.join(ANIMEATLAS_SQLITE_FILENAME);
    if !force_refresh && animeatlas_cache_is_fresh(&db_path) {
        return Ok(db_path);
    }

    let client = reqwest::Client::builder()
        .user_agent(HTTP_USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| AppError::MetadataFetchError(format!("创建 HTTP 客户端失败: {e}")))?;
    let resp = client
        .get(ANIMEATLAS_SQLITE_URL)
        .send()
        .await
        .map_err(|e| AppError::MetadataFetchError(format!("下载 AnimeAtlas 别名库失败: {e}")))?;

    if !resp.status().is_success() {
        return Err(AppError::MetadataFetchError(format!(
            "下载 AnimeAtlas 别名库失败 (HTTP {})",
            resp.status()
        )));
    }

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| AppError::MetadataFetchError(format!("读取 AnimeAtlas 别名库失败: {e}")))?;
    let tmp_path = db_path.with_extension("sqlite.tmp");
    std::fs::write(&tmp_path, &bytes)
        .map_err(|e| AppError::MetadataFetchError(format!("写入 AnimeAtlas 临时文件失败: {e}")))?;
    let validation = AliasLookup::load(&tmp_path).and_then(|lookup| {
        if lookup.is_empty() {
            Err(AppError::AliasLoadError(
                "AnimeAtlas 别名库不包含可用别名".to_string(),
            ))
        } else {
            Ok(())
        }
    });
    if let Err(error) = validation {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(AppError::MetadataFetchError(format!(
            "校验 AnimeAtlas 别名库失败: {error}"
        )));
    }
    if db_path.is_file() {
        std::fs::remove_file(&db_path).map_err(|e| {
            AppError::MetadataFetchError(format!("替换旧 AnimeAtlas 别名库失败: {e}"))
        })?;
    }
    std::fs::rename(&tmp_path, &db_path)
        .map_err(|e| AppError::MetadataFetchError(format!("保存 AnimeAtlas 别名库失败: {e}")))?;

    Ok(db_path)
}

#[cfg(feature = "metadata")]
fn animeatlas_cache_is_fresh(path: &Path) -> bool {
    let Ok(modified) = path.metadata().and_then(|metadata| metadata.modified()) else {
        return false;
    };
    animeatlas_timestamp_is_fresh(modified, std::time::SystemTime::now())
}

#[cfg(feature = "metadata")]
fn animeatlas_timestamp_is_fresh(
    modified: std::time::SystemTime,
    now: std::time::SystemTime,
) -> bool {
    now.duration_since(modified)
        .map_or(true, |age| age < ANIMEATLAS_CACHE_TTL)
}

fn has_valid_extension(path: &Path, extensions: &HashSet<String>) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| extensions.contains(&format!(".{}", ext.to_lowercase())))
        .unwrap_or(false)
}

fn is_supplemental_video_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if matches!(
        name.trim().to_ascii_lowercase().as_str(),
        "menu" | "ncop" | "nced" | "ncop&nced" | "图集" | "圖集" | "特典映像"
    ) {
        return true;
    }

    name.split('[')
        .skip(1)
        .filter_map(|part| part.split_once(']'))
        .map(|(token, _)| token.trim().to_ascii_uppercase())
        .any(|token| {
            matches!(token.as_str(), "MENU" | "NCOP&NCED" | "TOKUTEN" | "IMAGES")
                || ["NCOP", "NCED"].iter().any(|prefix| {
                    token
                        .strip_prefix(prefix)
                        .is_some_and(|suffix| suffix.chars().all(|ch| ch.is_ascii_digit()))
                })
        })
}

#[cfg(all(test, feature = "metadata"))]
mod tests {
    use super::*;

    #[test]
    fn bangumi_alias_db_points_at_weekly_release() {
        assert_eq!(
            BANGUMI_ALIAS_DB_URL,
            "https://github.com/ModerRAS/anime-organizer/releases/download/bangumi-db-latest/bangumi.db"
        );
        assert_eq!(
            BANGUMI_ALIAS_DB_TTL,
            std::time::Duration::from_secs(7 * 24 * 60 * 60)
        );
    }

    #[test]
    fn bangumi_alias_db_freshness_checks_size_and_age() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bangumi.db");
        // 不存在 → 需要下载
        assert!(!bangumi_alias_db_is_fresh(&path));
        // 明显偏小 → 视为下载不完整
        std::fs::write(&path, vec![0u8; 1024]).unwrap();
        assert!(!bangumi_alias_db_is_fresh(&path));
        // 大小正常且刚写入 → 直接复用
        std::fs::write(&path, vec![0u8; BANGUMI_ALIAS_DB_MIN_BYTES as usize]).unwrap();
        assert!(bangumi_alias_db_is_fresh(&path));
    }

    #[test]
    fn animeatlas_download_uses_fixed_release_url() {
        assert_eq!(
            ANIMEATLAS_SQLITE_URL,
            "https://github.com/ModerRAS/AnimeAtlas/releases/download/download/animeatlas.sqlite"
        );
    }

    #[test]
    fn animeatlas_cache_refreshes_daily() {
        let now = std::time::UNIX_EPOCH + ANIMEATLAS_CACHE_TTL * 2;
        assert!(animeatlas_timestamp_is_fresh(
            now - ANIMEATLAS_CACHE_TTL + std::time::Duration::from_secs(1),
            now
        ));
        assert!(!animeatlas_timestamp_is_fresh(
            now - ANIMEATLAS_CACHE_TTL,
            now
        ));
        assert!(animeatlas_timestamp_is_fresh(
            now + std::time::Duration::from_secs(1),
            now
        ));
    }

    #[test]
    fn animeatlas_existing_cache_is_reused() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(ANIMEATLAS_SQLITE_FILENAME);
        assert!(!animeatlas_cache_is_fresh(&path));
        std::fs::write(&path, b"cache").unwrap();
        assert!(animeatlas_cache_is_fresh(&path));
    }

    #[test]
    fn metadata_refresh_forces_artwork_only_when_provider_identity_changes() {
        let ids = HashMap::from([
            (("Changed".to_string(), 1), 565411),
            (("Stable".to_string(), 1), 545008),
        ]);
        assert!(force_artwork_refresh(
            false, true, &ids, "Changed", 1, 545008
        ));
        assert!(!force_artwork_refresh(
            false, true, &ids, "Stable", 1, 545008
        ));
        assert!(!force_artwork_refresh(
            false, true, &ids, "Unknown", 1, 545008
        ));
        assert!(force_artwork_refresh(
            true, false, &ids, "Stable", 1, 545008
        ));
    }

    #[test]
    fn library_artwork_uses_physical_series_directory() {
        let record = LibraryIndexRecord::new(
            "Parsed Title".to_string(),
            1,
            1.0,
            "Physical Folder/[ANi] Parsed Title - 01.mkv".to_string(),
            Path::new("unused.mkv"),
        );

        assert_eq!(
            library_series_root(Path::new("library"), &record, "Parsed Title"),
            Path::new("library").join("Physical Folder")
        );
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn daemon_flag_parses_without_organize_arguments() {
        let cli = Cli::try_parse_from(["aniorg", "--daemon"]).unwrap();
        assert!(cli.daemon);
        assert!(reject_daemon_conflicts(&cli).is_ok());
    }

    #[cfg(feature = "daemon")]
    #[test]
    fn daemon_rejects_source_arguments() {
        let cli = Cli::try_parse_from(["aniorg", "--daemon", "--source", "downloads"]).unwrap();
        assert!(reject_daemon_conflicts(&cli).is_err());
    }

    #[test]
    fn organize_entry_file_source_copies_nested_subtitles_and_indexes_them() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let source_video = source.path().join("[ANi] Test Anime - 01 [1080P].mkv");
        std::fs::write(&source_video, b"video").unwrap();
        let subtitle_dir = source.path().join("Subs");
        std::fs::create_dir(&subtitle_dir).unwrap();
        std::fs::write(
            subtitle_dir.join("[ANi] Test Anime - 01 [1080P].zh.srt"),
            b"sub",
        )
        .unwrap();

        run_organize_entry(OrganizeArgs {
            source: Some(source_video),
            target: Some(target.path().to_path_buf()),
            mode: OperationMode::Copy,
            library_index: true,
            ..OrganizeArgs::default()
        })
        .unwrap();

        let video = target
            .path()
            .join("Test Anime")
            .join("[ANi] Test Anime - 01 [1080P].mkv");
        assert!(video.exists());
        assert!(video
            .with_file_name("[ANi] Test Anime - 01 [1080P].zh.srt")
            .exists());
        let conn = rusqlite::Connection::open(LibraryIndex::database_path(target.path())).unwrap();
        let subtitle_path: String = conn
            .query_row("SELECT path FROM media_subtitle", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            subtitle_path,
            "Test Anime/[ANi] Test Anime - 01 [1080P].zh.srt"
        );
    }

    #[test]
    fn organize_cancellation_indexes_completed_files() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        for episode in [1, 2] {
            std::fs::write(
                source
                    .path()
                    .join(format!("[ANi] Test Anime - {episode:02}.mkv")),
                b"video",
            )
            .unwrap();
        }
        let checks = std::cell::Cell::new(0);
        let cancel = || {
            let current = checks.get();
            checks.set(current + 1);
            current >= 2
        };

        let result = run_organize(
            OrganizeArgs {
                source: Some(source.path().to_path_buf()),
                target: Some(target.path().to_path_buf()),
                mode: OperationMode::Copy,
                library_index: true,
                ..OrganizeArgs::default()
            },
            &|_| {},
            &cancel,
        );
        assert!(matches!(result, Err(AppError::Canceled)));

        let media_count = std::fs::read_dir(target.path().join("Test Anime"))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "mkv"))
            .count();
        assert_eq!(media_count, 1);
        let conn = rusqlite::Connection::open(LibraryIndex::database_path(target.path())).unwrap();
        let indexed_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM media_file", [], |row| row.get(0))
            .unwrap();
        assert_eq!(indexed_count, 1);
    }

    #[test]
    fn organize_entry_dry_run_does_not_create_media() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("[ANi] Test Anime - 01.mkv"), b"video").unwrap();
        run_organize_entry(OrganizeArgs {
            source: Some(source.path().to_path_buf()),
            target: Some(target.path().to_path_buf()),
            mode: OperationMode::Copy,
            dry_run: true,
            ..OrganizeArgs::default()
        })
        .unwrap();
        assert!(!target.path().join("Test Anime").exists());
    }

    #[test]
    fn organize_entry_skips_supplemental_videos() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("[ANi] Test Anime - 01.mkv"), b"video").unwrap();

        for directory in [
            "menu",
            "NCOP",
            "NCED",
            "NCOP&NCED",
            "图集",
            "圖集",
            "特典映像",
        ] {
            let path = source.path().join(directory);
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("supplement.mkv"), b"supplement").unwrap();
        }
        for tag in ["MENU", "NCOP01", "NCED10", "Tokuten", "Images"] {
            std::fs::write(
                source
                    .path()
                    .join(format!("[DBD-Raws][Test Anime][{tag}][01].mkv")),
                b"supplement",
            )
            .unwrap();
        }

        run_organize_entry(OrganizeArgs {
            source: Some(source.path().to_path_buf()),
            target: Some(target.path().to_path_buf()),
            mode: OperationMode::Copy,
            ..OrganizeArgs::default()
        })
        .unwrap();

        assert_eq!(std::fs::read_dir(target.path()).unwrap().count(), 1);
        assert!(target.path().join("Test Anime").exists());
    }

    fn write_asset_zip(path: &std::path::Path, files: &[(&str, &[u8])]) {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        use zip::ZipWriter;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = ZipWriter::new(file);
        for (name, bytes) in files {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn organize_extracts_subtitle_archive_beside_the_episode() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        let video = "[SweetSub] Test Show - 01 [1080P].mkv";
        std::fs::write(source.path().join(video), b"video").unwrap();
        write_asset_zip(
            &source.path().join("[SweetSub] Test Show - Subtitles.zip"),
            &[
                ("[SweetSub] Test Show - 01.chs.ass", b"dialogue"),
                ("fonts/Show.ttf", b"font-bytes"),
                ("readme.txt", b"skip"),
            ],
        );

        run_organize_entry(OrganizeArgs {
            source: Some(source.path().to_path_buf()),
            target: Some(target.path().to_path_buf()),
            mode: OperationMode::Copy,
            ..OrganizeArgs::default()
        })
        .unwrap();

        let episode = target.path().join("Test Show");
        assert_eq!(
            std::fs::read(episode.join("[SweetSub] Test Show - 01.chs.ass")).unwrap(),
            b"dialogue"
        );
        assert_eq!(
            std::fs::read(episode.join("fonts").join("Show.ttf")).unwrap(),
            b"font-bytes"
        );
        assert!(!episode.join("readme.txt").exists());
        assert!(source
            .path()
            .join("[SweetSub] Test Show - Subtitles.zip")
            .exists());
        assert!(source.path().join(video).exists());
    }

    #[test]
    fn organize_deletes_consumed_archive_when_requested() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(
            source.path().join("[SweetSub] Test Show - 01 [1080P].mkv"),
            b"video",
        )
        .unwrap();
        let archive = source.path().join("[SweetSub] Test Show - Subtitles.zip");
        write_asset_zip(
            &archive,
            &[("[SweetSub] Test Show - 01.chs.ass", b"dialogue")],
        );

        run_organize_entry(OrganizeArgs {
            source: Some(source.path().to_path_buf()),
            target: Some(target.path().to_path_buf()),
            mode: OperationMode::Copy,
            delete_extracted_archives: true,
            ..OrganizeArgs::default()
        })
        .unwrap();

        assert!(!archive.exists());
        assert!(target
            .path()
            .join("Test Show")
            .join("[SweetSub] Test Show - 01.chs.ass")
            .exists());
        assert!(source
            .path()
            .join("[SweetSub] Test Show - 01 [1080P].mkv")
            .exists());
    }

    #[test]
    fn organize_rejects_zip_slip_and_keeps_the_video() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("[ANi] Test Show - 01.mkv"), b"video").unwrap();
        write_asset_zip(
            &source.path().join("Subtitles.zip"),
            &[("../../evil.exe", b"MZ"), ("payload.exe", b"MZ")],
        );

        run_organize_entry(OrganizeArgs {
            source: Some(source.path().to_path_buf()),
            target: Some(target.path().to_path_buf()),
            mode: OperationMode::Copy,
            ..OrganizeArgs::default()
        })
        .unwrap();

        assert!(target
            .path()
            .join("Test Show")
            .join("[ANi] Test Show - 01.mkv")
            .exists());
        assert!(!source.path().join("evil.exe").exists());
        assert!(!target.path().join("evil.exe").exists());
        assert!(!target.path().join("Test Show").join("payload.exe").exists());
        assert!(source.path().join("Subtitles.zip").exists());
    }
}
