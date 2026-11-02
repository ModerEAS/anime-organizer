use crate::cli::*;
use anime_organizer::error::AppError;
#[cfg(feature = "scraper")]
use anime_organizer::metadata::AliasLookup;
#[cfg(feature = "scraper")]
use anime_organizer::scraper::{
    db_builder::build_bangumi_db,
    matcher::{format_github_output, match_aliases},
    MatchResult, ScrapedAnime, Scraper,
};
#[cfg(feature = "scraper")]
use serde::{Deserialize, Serialize};
#[cfg(feature = "scraper")]
use std::io::Write;
#[cfg(feature = "scraper")]
use std::path::PathBuf;

pub(crate) fn run_command(command: Commands) -> Result<(), AppError> {
    match command {
        Commands::NormalizeLayout(args) => {
            let result = run_normalize_layout(&args, false, &|message| eprintln!("{message}"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| {
                    AppError::LibraryIndexError(format!("序列化 layout 结果失败: {error}"))
                })?
            );
            Ok(())
        }
        Commands::CompactArtworkPacks(args) => {
            let result = run_compact_artwork(&args, false, &|message| eprintln!("{message}"))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|error| {
                    AppError::LibraryIndexError(format!("序列化 compact 结果失败: {error}"))
                })?
            );
            Ok(())
        }
        #[cfg(feature = "scraper")]
        Commands::Scrape(args) => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;
            runtime.block_on(run_scrape(args))
        }
        #[cfg(feature = "scraper")]
        Commands::Match(args) => run_match(args),
        #[cfg(feature = "scraper")]
        Commands::BuildDb(args) => run_build_db(args),
        #[cfg(feature = "scraper")]
        Commands::ExtractAliases(args) => run_extract_aliases(args),
        #[cfg(feature = "scraper")]
        Commands::MergeAliases(args) => run_merge_aliases(args),
        #[cfg(feature = "scraper")]
        Commands::ApplyMatches(args) => run_apply_matches(args),
        #[cfg(feature = "scraper")]
        Commands::CreateAliasIssues(args) => run_create_alias_issues(args),
        #[cfg(feature = "clouddrive")]
        Commands::Rss(args) => run_rss(args),
        #[cfg(feature = "clouddrive")]
        Commands::AddOffline(args) => run_add_offline(args),
        #[cfg(feature = "clouddrive")]
        Commands::ListFolder(args) => run_list_folder(args),
        #[cfg(feature = "torrent-scraper")]
        Commands::TorrentScrape(args) => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;
            runtime.block_on(run_torrent_scrape(args))
        }
        #[cfg(feature = "self-update")]
        Commands::Update(args) => {
            let runtime = tokio::runtime::Runtime::new()
                .map_err(|error| AppError::SelfUpdate(format!("创建异步运行时失败: {error}")))?;
            runtime
                .block_on(crate::self_update::run_cli(
                    args.check,
                    args.feed.as_deref(),
                ))
                .map_err(AppError::SelfUpdate)
        }
    }
}

pub(crate) fn run_compact_artwork(
    args: &CompactArtworkPacksArgs,
    confirmed: bool,
    log: &dyn Fn(&str),
) -> Result<serde_json::Value, AppError> {
    if args.dry_run {
        let output = args.plan.as_ref().ok_or_else(|| {
            AppError::ParseError("compact-artwork-packs --dry-run requires --plan".to_string())
        })?;
        if args.apply_plan.is_some() {
            return Err(AppError::ParseError(
                "--dry-run cannot be combined with --apply-plan".to_string(),
            ));
        }
        let plan = anime_organizer::build_artwork_compact_plan(&args.target, output, log)?;
        return serde_json::to_value(&plan.stats).map_err(|error| {
            AppError::LibraryIndexError(format!("序列化 compact plan 汇总失败: {error}"))
        });
    }
    let input = args.apply_plan.as_ref().ok_or_else(|| {
        AppError::ParseError(
            "compact-artwork-packs requires --dry-run --plan or --apply-plan".to_string(),
        )
    })?;
    let summary = anime_organizer::apply_artwork_compact_plan(&args.target, input, confirmed, log)?;
    serde_json::to_value(summary).map_err(|error| {
        AppError::LibraryIndexError(format!("序列化 compact apply 汇总失败: {error}"))
    })
}

pub(crate) fn run_normalize_layout(
    args: &NormalizeLayoutArgs,
    confirmed: bool,
    log: &dyn Fn(&str),
) -> Result<serde_json::Value, AppError> {
    run_normalize_layout_with_cancel(args, confirmed, log, &|| false)
}

pub(crate) fn run_normalize_layout_with_cancel(
    args: &NormalizeLayoutArgs,
    confirmed: bool,
    log: &dyn Fn(&str),
    cancel: &dyn Fn() -> bool,
) -> Result<serde_json::Value, AppError> {
    if cancel() {
        return Err(AppError::Canceled);
    }
    if args.dry_run {
        let output = args.plan.as_ref().ok_or_else(|| {
            AppError::ParseError("normalize-layout --dry-run requires --plan".to_string())
        })?;
        if args.apply_plan.is_some() {
            return Err(AppError::ParseError(
                "--dry-run cannot be combined with --apply-plan".to_string(),
            ));
        }
        let plan =
            anime_organizer::build_layout_plan_with_cancel(&args.target, output, log, cancel)?;
        return serde_json::to_value(&plan.summary).map_err(|error| {
            AppError::LibraryIndexError(format!("序列化 layout plan 汇总失败: {error}"))
        });
    }
    let input = args.apply_plan.as_ref().ok_or_else(|| {
        AppError::ParseError(
            "normalize-layout requires --dry-run --plan or --apply-plan".to_string(),
        )
    })?;
    if cancel() {
        return Err(AppError::Canceled);
    }
    let summary = anime_organizer::apply_layout_plan(&args.target, input, confirmed, log)?;
    serde_json::to_value(summary).map_err(|error| {
        AppError::LibraryIndexError(format!("序列化 layout apply 汇总失败: {error}"))
    })
}

#[cfg(feature = "scraper")]
pub(crate) async fn scrape_result(args: &ScrapeArgs) -> Result<Vec<ScrapedAnime>, AppError> {
    let scraper = Scraper::new();
    let tmdb_api_key = args
        .tmdb_api_key
        .clone()
        .or_else(|| std::env::var("TMDB_API_KEY").ok());
    scraper.scrape_all(args.days, tmdb_api_key.as_deref()).await
}

#[cfg(feature = "scraper")]
async fn run_scrape(args: ScrapeArgs) -> Result<(), AppError> {
    let scraped = scrape_result(&args).await?;

    match args.format {
        ScrapeOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&scraped).map_err(
                    |e| AppError::MetadataFetchError(format!("序列化刮削结果失败: {e}"))
                )?
            );
        }
        ScrapeOutputFormat::Pretty => {
            for anime in scraped {
                println!(
                    "{} | {} | {} | {}",
                    anime.source,
                    anime.date.unwrap_or_else(|| "N/A".to_string()),
                    anime.title,
                    anime.title_cn.unwrap_or_else(|| "-".to_string())
                );
            }
        }
    }

    Ok(())
}

#[cfg(feature = "scraper")]
pub(crate) fn match_result(args: &MatchArgs) -> Result<MatchResult, AppError> {
    let input = std::fs::read_to_string(&args.input)
        .map_err(|e| AppError::MetadataFetchError(format!("读取刮削输入失败: {e}")))?;
    let scraped: Vec<ScrapedAnime> = serde_json::from_str(&input)
        .map_err(|e| AppError::MetadataFetchError(format!("解析刮削输入失败: {e}")))?;

    let db_path = PathBuf::from("./bangumi.db");
    let aliases = AliasLookup::load(&db_path)?;
    Ok(match_aliases(&scraped, aliases.entries()))
}

#[cfg(feature = "scraper")]
fn run_match(args: MatchArgs) -> Result<(), AppError> {
    let result = match_result(&args)?;

    match args.format {
        MatchOutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&result).map_err(|e| AppError::MetadataFetchError(
                    format!("序列化匹配结果失败: {e}")
                ))?
            );
        }
        MatchOutputFormat::Github => {
            println!("{}", format_github_output(&result));
        }
    }

    Ok(())
}

#[cfg(feature = "scraper")]
fn run_build_db(args: BuildDbArgs) -> Result<(), AppError> {
    let stats = build_db_result(&args)?;

    println!("=== Database Build Stats ===");
    println!("Subjects: {}", stats.subjects_count);
    println!("Episodes: {}", stats.episodes_count);
    println!("Aliases: {}", stats.aliases_count);
    println!("Relations: {}", stats.relations_count);
    println!(
        "Database size: {} bytes ({} KB, {} MB)",
        stats.db_size,
        stats.db_size / 1024,
        stats.db_size / (1024 * 1024)
    );
    println!(
        "Processing time: {} ms ({} s)",
        stats.processing_time_ms,
        stats.processing_time_ms as f64 / 1000.0
    );

    Ok(())
}

#[cfg(feature = "scraper")]
pub(crate) fn build_db_result(
    args: &BuildDbArgs,
) -> Result<anime_organizer::scraper::BuildDbStats, AppError> {
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;
    runtime.block_on(build_bangumi_db(
        &args.output,
        args.include_relations,
        args.verbose,
    ))
}

#[cfg(feature = "scraper")]
pub(crate) fn extract_aliases_result(
    args: &ExtractAliasesArgs,
) -> Result<std::collections::HashMap<String, AliasEntry>, AppError> {
    use anime_organizer::metadata::wiki::WikiParser;
    use std::io::{BufRead, BufReader};

    let dump_path = if args.download {
        let runtime = tokio::runtime::Runtime::new()
            .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;
        runtime.block_on(async {
            let client = anime_organizer::metadata::BangumiClient::new(None);
            client.download_dump().await
        })?
    } else if let Some(ref path) = args.input {
        path.clone()
    } else {
        let client = anime_organizer::metadata::BangumiClient::new(None);
        client.resolve_existing_dump_path().ok_or_else(|| {
            AppError::MetadataFetchError(
                "未找到本地 dump，请使用 --download 下载或 --input 指定路径".to_string(),
            )
        })?
    };

    let file = std::fs::File::open(&dump_path)
        .map_err(|e| AppError::BangumiParseError(format!("打开 dump 文件失败: {e}")))?;
    let reader = BufReader::new(file);
    let mut aliases_map = std::collections::HashMap::new();
    let parser = WikiParser::new();

    for line in reader.lines() {
        let line = line.map_err(|e| AppError::BangumiParseError(format!("读取行失败: {e}")))?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let subject: anime_organizer::metadata::bangumi::BangumiSubject =
            match serde_json::from_str(line) {
                Ok(subject) => subject,
                Err(_) => continue,
            };
        if subject.subject_type != 2 {
            continue;
        }
        let Some(ref infobox) = subject.infobox else {
            continue;
        };
        let anime_info = match parser.parse_anime_infobox(infobox) {
            Ok(info) => info,
            Err(_) => continue,
        };
        if anime_info.aliases.is_empty() {
            continue;
        }
        let entry = AliasEntry {
            bangumi_id: subject.id,
            name: subject.name,
            name_cn: subject.name_cn,
        };
        for alias in anime_info.aliases {
            if !alias.is_empty() {
                aliases_map.insert(alias, entry.clone());
            }
        }
    }
    Ok(aliases_map)
}

#[cfg(feature = "scraper")]
fn run_extract_aliases(args: ExtractAliasesArgs) -> Result<(), AppError> {
    let aliases_map = extract_aliases_result(&args)?;
    let output: Box<dyn std::io::Write> = if let Some(ref path) = args.output {
        Box::new(
            std::fs::File::create(path)
                .map_err(|e| AppError::MetadataFetchError(format!("创建输出文件失败: {e}")))?,
        )
    } else {
        Box::new(std::io::stdout())
    };
    let mut writer = std::io::BufWriter::new(output);
    serde_json::to_writer(&mut writer, &aliases_map)
        .map_err(|e| AppError::MetadataFetchError(format!("序列化别名失败: {e}")))?;
    writer
        .flush()
        .map_err(|e| AppError::MetadataFetchError(format!("写入输出失败: {e}")))?;
    eprintln!("成功提取 {} 个别名", aliases_map.len());
    Ok(())
}

#[cfg(feature = "scraper")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AliasMutationItem {
    pub(crate) alias: String,
    pub(crate) subject_id: u32,
    pub(crate) added: bool,
}

#[cfg(feature = "scraper")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AliasMutationResult {
    pub(crate) added: usize,
    pub(crate) processed: usize,
    pub(crate) target: String,
    pub(crate) items: Vec<AliasMutationItem>,
}

#[cfg(feature = "scraper")]
pub(crate) fn merge_aliases_result(
    args: &MergeAliasesArgs,
) -> Result<AliasMutationResult, AppError> {
    use rusqlite::Connection;

    let db_path = args
        .target
        .clone()
        .unwrap_or_else(|| PathBuf::from("bangumi.db"));
    if !db_path.exists() {
        return Err(AppError::AliasLoadError(
            "数据库不存在，请先运行 build-db".to_string(),
        ));
    }
    let conn = Connection::open(&db_path)
        .map_err(|e| AppError::AliasLoadError(format!("打开数据库失败: {e}")))?;
    let content = std::fs::read_to_string(&args.input)
        .map_err(|e| AppError::AliasLoadError(format!("读取输入文件失败: {e}")))?;
    let aliases: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&content)
        .map_err(|e| AppError::AliasLoadError(format!("解析输入文件失败: {e}")))?;

    let mut added = 0;
    let mut processed = 0;
    let mut items = Vec::new();
    for (alias, value) in aliases {
        if alias.is_empty() {
            continue;
        }
        let bangumi_id = value
            .get("bangumi_id")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as u32;
        if bangumi_id == 0 {
            continue;
        }
        processed += 1;
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO aliases (subject_id, alias) VALUES (?1, ?2)",
                rusqlite::params![bangumi_id, alias],
            )
            .map_err(|e| AppError::AliasLoadError(format!("写入别名失败: {e}")))?
            == 1;
        if inserted {
            added += 1;
        }
        items.push(AliasMutationItem {
            alias,
            subject_id: bangumi_id,
            added: inserted,
        });
    }
    Ok(AliasMutationResult {
        added,
        processed,
        target: db_path.to_string_lossy().into_owned(),
        items,
    })
}

#[cfg(feature = "scraper")]
fn run_merge_aliases(args: MergeAliasesArgs) -> Result<(), AppError> {
    let result = merge_aliases_result(&args)?;
    println!("新增 {} 个别名到数据库", result.added);
    Ok(())
}

#[cfg(feature = "scraper")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AliasEntry {
    bangumi_id: u32,
    name: String,
    name_cn: Option<String>,
}

#[cfg(feature = "scraper")]
pub(crate) fn apply_matches_result(
    args: &ApplyMatchesArgs,
) -> Result<AliasMutationResult, AppError> {
    use anime_organizer::scraper::matcher::Proposal;
    use rusqlite::Connection;

    let db_path = args
        .target
        .clone()
        .unwrap_or_else(|| PathBuf::from("bangumi.db"));
    if !db_path.exists() {
        return Err(AppError::AliasLoadError(
            "数据库不存在，请先运行 build-db".to_string(),
        ));
    }
    let conn = Connection::open(&db_path)
        .map_err(|e| AppError::AliasLoadError(format!("打开数据库失败: {e}")))?;
    let content = std::fs::read_to_string(&args.input)
        .map_err(|e| AppError::AliasLoadError(format!("读取提案文件失败: {e}")))?;
    let proposals: Vec<Proposal> = serde_json::from_str(&content)
        .map_err(|e| AppError::AliasLoadError(format!("解析提案文件失败: {e}")))?;

    let mut added = 0;
    let mut processed = 0;
    let mut items = Vec::new();
    for proposal in proposals {
        let alias = proposal.fan_translation;
        if alias.is_empty() {
            continue;
        }
        processed += 1;
        let subject_id = proposal.alias_entry.bangumi_id;
        let inserted = conn
            .execute(
                "INSERT OR IGNORE INTO aliases (subject_id, alias) VALUES (?1, ?2)",
                rusqlite::params![subject_id, alias],
            )
            .map_err(|e| AppError::AliasLoadError(format!("写入匹配别名失败: {e}")))?
            == 1;
        if inserted {
            added += 1;
        }
        items.push(AliasMutationItem {
            alias,
            subject_id,
            added: inserted,
        });
    }
    Ok(AliasMutationResult {
        added,
        processed,
        target: db_path.to_string_lossy().into_owned(),
        items,
    })
}

#[cfg(feature = "scraper")]
fn run_apply_matches(args: ApplyMatchesArgs) -> Result<(), AppError> {
    let result = apply_matches_result(&args)?;
    println!("新增 {} 个别名到数据库", result.added);
    Ok(())
}

#[cfg(feature = "scraper")]
pub(crate) fn gh_preflight() -> Result<String, AppError> {
    use std::process::Command;

    let version = Command::new("gh")
        .arg("--version")
        .output()
        .map_err(|_| AppError::AliasLoadError("gh is not installed".to_string()))?;
    if !version.status.success() {
        return Err(AppError::AliasLoadError(
            "gh version check failed".to_string(),
        ));
    }
    let version_text = String::from_utf8_lossy(&version.stdout)
        .lines()
        .next()
        .unwrap_or("gh")
        .to_string();
    let auth = Command::new("gh")
        .args(["auth", "status"])
        .output()
        .map_err(|_| AppError::AliasLoadError("gh authentication check failed".to_string()))?;
    if !auth.status.success() {
        return Err(AppError::AliasLoadError(
            "gh is not authenticated".to_string(),
        ));
    }
    Ok(version_text)
}

#[cfg(feature = "scraper")]
pub(crate) fn gh_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| gh_preflight().is_ok())
}

#[cfg(feature = "scraper")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct AliasIssueResult {
    pub(crate) index: usize,
    pub(crate) fan_translation: String,
    pub(crate) success: bool,
    pub(crate) issue_url: Option<String>,
    pub(crate) error: Option<String>,
}

#[cfg(feature = "scraper")]
pub(crate) fn create_alias_issues_result(
    args: &CreateAliasIssuesArgs,
) -> Result<Vec<AliasIssueResult>, AppError> {
    use anime_organizer::scraper::matcher::Proposal;
    use std::process::Command;

    gh_preflight()?;
    let content = std::fs::read_to_string(&args.input)
        .map_err(|e| AppError::AliasLoadError(format!("读取提案文件失败: {e}")))?;
    let proposals: Vec<Proposal> = serde_json::from_str(&content)
        .map_err(|e| AppError::AliasLoadError(format!("解析提案文件失败: {e}")))?;
    let repo = args.repo.clone().unwrap_or_else(|| {
        std::env::var("GITHUB_REPOSITORY")
            .unwrap_or_else(|_| "ModerRAS/anime-organizer".to_string())
    });
    let mut results = Vec::new();

    for (index, proposal) in proposals.into_iter().enumerate() {
        if proposal.fan_translation.is_empty() {
            continue;
        }
        let fan = proposal.fan_translation;
        let title = format!(
            "[Alias Request] {} -> {} (bgm:{})",
            fan, proposal.alias_entry.name, proposal.alias_entry.bangumi_id
        );
        let body = format!(
            "## Anime Information\n\n- **Bangumi ID**: {}\n- **Bangumi Name**: {}\n- **Fan Translation**: {}\n\n## LLM Analysis\n\n- **Confidence**: {}\n- **Reasoning**: {}\n\n## User Action Required\n\nReply with:\n- `confirm` - approve as-is\n- `correct: {{...}}` - provide correction\n- `reject` - discard",
            proposal.alias_entry.bangumi_id,
            proposal.alias_entry.name,
            fan,
            proposal.confidence,
            proposal.reasoning
        );
        let result = Command::new("gh")
            .args([
                "issue",
                "create",
                "--repo",
                &repo,
                "--title",
                &title,
                "--body",
                &body,
                "--label",
                "alias-request",
            ])
            .output();
        match result {
            Ok(output) if output.status.success() => results.push(AliasIssueResult {
                index,
                fan_translation: fan,
                success: true,
                issue_url: Some(String::from_utf8_lossy(&output.stdout).trim().to_string()),
                error: None,
            }),
            Ok(_) => results.push(AliasIssueResult {
                index,
                fan_translation: fan,
                success: false,
                issue_url: None,
                error: Some("gh issue create failed".to_string()),
            }),
            Err(_) => results.push(AliasIssueResult {
                index,
                fan_translation: fan,
                success: false,
                issue_url: None,
                error: Some("failed to execute gh issue create".to_string()),
            }),
        }
    }
    Ok(results)
}

#[cfg(feature = "scraper")]
fn run_create_alias_issues(args: CreateAliasIssuesArgs) -> Result<(), AppError> {
    let results = create_alias_issues_result(&args)?;
    println!(
        "创建了 {} 个 issue",
        results.iter().filter(|result| result.success).count()
    );
    Ok(())
}

#[cfg(feature = "clouddrive")]
fn run_rss(args: RssArgs) -> Result<(), AppError> {
    use anime_organizer::rss::{
        client::{CloudDriveClient, CloudDriveClientTrait},
        db::{default_db_path, RssDatabase},
        http_client::HttpClient,
        proxy::{build_http_client, ProxyConfig},
        scheduler::RssScheduler,
    };
    use std::sync::Arc;

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;

    runtime.block_on(async {
        // 初始化 tracing
        tracing_subscriber::fmt::init();

        // 初始化数据库
        let db_path = default_db_path();
        let db = RssDatabase::new(&db_path)?;

        // --list-subscriptions：列出订阅后退出
        if args.list_subscriptions {
            let subs = db.list_all_subscriptions()?;
            if subs.is_empty() {
                println!("暂无 RSS 订阅");
            } else {
                println!(
                    "{:<4} {:<60} {:<20} {:<10} 目标目录",
                    "ID", "URL", "过滤器", "间隔(s)"
                );
                println!("{}", "-".repeat(120));
                for sub in &subs {
                    println!(
                        "{:<4} {:<60} {:<20} {:<10} {}",
                        sub.id,
                        sub.url,
                        sub.filter_regex.as_deref().unwrap_or("-"),
                        sub.interval_secs,
                        sub.target_folder
                    );
                }
            }
            return Ok(());
        }

        // --add-subscription：添加订阅后退出
        if args.add_subscription {
            let rss_url = args.rss_url.as_deref().ok_or_else(|| {
                AppError::MetadataFetchError("添加订阅需要 --rss-url 参数".to_string())
            })?;
            let target = args.rss_target.as_deref().ok_or_else(|| {
                AppError::MetadataFetchError("添加订阅需要 --rss-target 参数".to_string())
            })?;
            let id = db.add_subscription(
                rss_url,
                args.rss_filter.as_deref(),
                target,
                args.rss_interval as i64,
            )?;
            println!("已添加/更新订阅 (id={id}): {rss_url}");
            return Ok(());
        }

        // 构建 CloudDrive2 客户端
        let cd_url = args.clouddrive_url.as_deref().ok_or_else(|| {
            AppError::MetadataFetchError("需要 --clouddrive-url 参数".to_string())
        })?;
        let mut cd_client = CloudDriveClient::new(cd_url, args.clouddrive_token.clone())?;

        // 如果没有 token 但有用户名/密码，则登录
        if cd_client.get_token_value().is_none() {
            let user = args.clouddrive_user.as_deref().ok_or_else(|| {
                AppError::MetadataFetchError(
                    "需要 --clouddrive-token 或 --clouddrive-user + --clouddrive-pass".to_string(),
                )
            })?;
            let pass = args.clouddrive_pass.as_deref().ok_or_else(|| {
                AppError::MetadataFetchError("需要 --clouddrive-pass 参数".to_string())
            })?;
            let token = cd_client.login(user, pass).await?;
            tracing::info!("CloudDrive2 登录成功，获取到令牌");
            let _ = token;
        }

        let proxy_config = ProxyConfig::from_env();
        let http_client = build_http_client(&proxy_config)?;
        let scheduler = RssScheduler::new(
            db,
            Arc::new(HttpClient::new(http_client)),
            Arc::new(cd_client),
            args.daemon || args.single_shot,
        );

        // 根据模式选择执行路径
        if args.daemon {
            let interval = std::time::Duration::from_secs(args.rss_interval);

            if let Some(ref rss_url) = args.rss_url {
                let target = args.rss_target.as_deref().ok_or_else(|| {
                    AppError::MetadataFetchError("daemon 模式需要 --rss-target 参数".to_string())
                })?;
                scheduler
                    .run_daemon_url(rss_url, args.rss_filter.as_deref(), target, interval)
                    .await
            } else {
                scheduler.run_daemon(interval).await
            }
        } else {
            // 默认单次执行
            if let Some(ref rss_url) = args.rss_url {
                let target = args.rss_target.as_deref().ok_or_else(|| {
                    AppError::MetadataFetchError("单次执行需要 --rss-target 参数".to_string())
                })?;
                scheduler
                    .run_once_url(
                        rss_url,
                        args.rss_filter.as_deref(),
                        target,
                        args.rss_interval,
                    )
                    .await
            } else {
                scheduler.run_once().await
            }
        }
    })
}

#[cfg(feature = "clouddrive")]
fn run_add_offline(args: AddOfflineArgs) -> Result<(), AppError> {
    use anime_organizer::rss::client::{CloudDriveClient, CloudDriveClientTrait};

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;

    runtime.block_on(async {
        let client = CloudDriveClient::new(&args.clouddrive_url, Some(args.clouddrive_token))?;

        println!("提交离线下载...");
        println!("  目标: {}", args.target);
        println!("  URL: {}...", &args.url[..args.url.len().min(60)]);

        client
            .add_offline_files(vec![args.url], &args.target)
            .await?;

        println!("✅ 离线下载提交成功！");
        Ok(())
    })
}

#[cfg(feature = "clouddrive")]
fn run_list_folder(args: ListFolderArgs) -> Result<(), AppError> {
    use anime_organizer::rss::client::{CloudDriveClient, CloudDriveClientTrait};

    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| AppError::MetadataFetchError(format!("创建异步运行时失败: {e}")))?;

    runtime.block_on(async {
        let client = CloudDriveClient::new(&args.clouddrive_url, Some(args.clouddrive_token))?;

        println!("浏览目录: {}", args.path);
        println!("{}", "=".repeat(80));

        let files = client.list_folder(&args.path).await?;

        if files.is_empty() {
            println!("(空目录)");
            return Ok(());
        }

        println!("{:<6} {:<50} {:>12}  云盘", "类型", "名称", "大小");
        println!("{}", "-".repeat(80));

        for file in &files {
            let file_type = if file.is_directory {
                "[目录]"
            } else {
                "[文件]"
            };
            let size = if file.is_directory {
                "-".to_string()
            } else {
                format_size(file.size)
            };
            let name = if file.name.is_empty() {
                file.full_path_name.clone()
            } else {
                file.name.clone()
            };
            let cloud = file
                .cloud_api
                .as_ref()
                .map(|c| c.name.as_str())
                .unwrap_or("-");
            println!("{:<6} {:<50} {:>12}  {}", file_type, name, size, cloud);
        }

        println!("{}", "-".repeat(80));
        println!("共 {} 项", files.len());
        Ok(())
    })
}

#[allow(dead_code)]
fn format_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

#[cfg(feature = "torrent-scraper")]
async fn run_torrent_scrape(args: TorrentScrapeArgs) -> Result<(), AppError> {
    use anime_organizer::torrent::dmhy;
    use anime_organizer::torrent::nyaa;

    let pages = anime_organizer::torrent::clamp_pages(args.pages);
    if args.headed {
        eprintln!("--headed 仅适用于旧 Playwright 后端；当前使用 HTTP 抓取");
    }

    let all_titles = match args.source {
        TorrentSource::Dmhy => {
            println!("正在从 DMHY 爬取种子文件列表 ({} 页)...", pages);
            dmhy::scrape_dmhy(pages).await?
        }
        TorrentSource::Nyaa => {
            if let Some(query) = args.query.as_deref() {
                println!(
                    "正在从 Nyaa 搜索 '{}' 并爬取种子文件列表 ({} 页)...",
                    query, pages
                );
                nyaa::scrape_search(query, pages).await?
            } else {
                println!("正在从 Nyaa 首页爬取最新种子文件列表 ({} 页)...", pages);
                nyaa::scrape_recent(pages).await?
            }
        }
        TorrentSource::All => {
            println!("正在从 DMHY 爬取 ({} 页)...", pages);
            let mut titles = dmhy::scrape_dmhy(pages).await?;

            println!("正在从 Nyaa 首页爬取最新种子 ({} 页)...", pages);
            titles.extend(nyaa::scrape_recent(pages).await?);
            titles
        }
    };

    if all_titles.is_empty() {
        println!("未爬取到任何文件");
        return Ok(());
    }

    let unique_lines = anime_organizer::torrent::sorted_unique_title_lines(&all_titles);
    let text = unique_lines.join("\n");

    if let Some(ref output_path) = args.output {
        std::fs::write(output_path, &text)
            .map_err(|e| AppError::Io(std::io::Error::other(format!("写入文件失败: {e}"))))?;
        println!("已保存到: {}", output_path.display());
    } else {
        print!("{}", text);
    }

    println!("\n共爬取 {} 个文件名", unique_lines.len());
    Ok(())
}

#[cfg(all(test, feature = "scraper"))]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn alias_db(path: &std::path::Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE aliases (subject_id INTEGER NOT NULL, alias TEXT NOT NULL, UNIQUE(subject_id, alias));",
        )
        .unwrap();
    }

    #[test]
    fn merge_uses_explicit_target_database() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("selected.db");
        let input = directory.path().join("aliases.json");
        alias_db(&target);
        std::fs::write(
            &input,
            r#"{"New Alias":{"bangumi_id":1,"name":"Example","name_cn":null}}"#,
        )
        .unwrap();

        let result = merge_aliases_result(&crate::cli::MergeAliasesArgs {
            input,
            target: Some(target.clone()),
        })
        .unwrap();
        assert_eq!(result.added, 1);
        let count: i64 = Connection::open(target)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM aliases", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn merge_reports_database_write_errors() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("broken.db");
        let input = directory.path().join("aliases.json");
        Connection::open(&target)
            .unwrap()
            .execute_batch("CREATE TABLE aliases (unexpected INTEGER);")
            .unwrap();
        std::fs::write(
            &input,
            r#"{"New Alias":{"bangumi_id":1,"name":"Example","name_cn":null}}"#,
        )
        .unwrap();

        assert!(merge_aliases_result(&crate::cli::MergeAliasesArgs {
            input,
            target: Some(target),
        })
        .is_err());
    }

    #[test]
    fn apply_uses_explicit_target_database() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("selected.db");
        let input = directory.path().join("matches.json");
        alias_db(&target);
        std::fs::write(
            &input,
            r#"[{"fan_translation":"New Match","alias_entry":{"bangumi_id":1,"name":"Example","tmdb_id":null,"anidb_id":null},"confidence":"High","reasoning":"test","source":"test"}]"#,
        )
        .unwrap();

        let result = apply_matches_result(&crate::cli::ApplyMatchesArgs {
            input,
            target: Some(target.clone()),
        })
        .unwrap();
        assert_eq!(result.added, 1);
        let count: i64 = Connection::open(target)
            .unwrap()
            .query_row("SELECT COUNT(*) FROM aliases", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
