//! 从 GitHub Release 替换当前 `aniorg`。
//!
//! 发布标签可以是 `vMAJOR.MINOR.PATCH`，也可以是 master 构建使用的
//! `vMAJOR.MINOR.PATCH+BUILD`。版本号相同且两边都能读到提交号时，提交号不同也算新构建。

use std::cmp::Ordering;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

pub(crate) const DEFAULT_RELEASE_API: &str =
    "https://api.github.com/repos/ModerRAS/anime-organizer/releases/latest";
const USER_AGENT: &str = concat!(
    "ModerRAS/anime-organizer/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/ModerRAS/anime-organizer)"
);

const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 320 * 1024 * 1024;
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
const MAX_ENTRIES: usize = 64;

pub(crate) fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub(crate) fn current_commit() -> Option<&'static str> {
    option_env!("ANIORG_GIT_SHA")
        .map(str::trim)
        .filter(|value| looks_like_commit(value))
}

pub(crate) fn current_release_target() -> Result<&'static str, String> {
    #[cfg(all(target_os = "linux", target_env = "musl", target_arch = "x86_64"))]
    return Ok("x86_64-unknown-linux-musl");
    #[cfg(all(target_os = "linux", target_env = "musl", target_arch = "aarch64"))]
    return Ok("aarch64-unknown-linux-musl");
    #[cfg(all(target_os = "linux", not(target_env = "musl"), target_arch = "x86_64"))]
    return Ok("x86_64-unknown-linux-gnu");
    #[cfg(all(target_os = "linux", not(target_env = "musl"), target_arch = "aarch64"))]
    return Ok("aarch64-unknown-linux-gnu");
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Ok("x86_64-pc-windows-msvc");
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return Ok("aarch64-pc-windows-msvc");
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok("x86_64-apple-darwin");
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok("aarch64-apple-darwin");
    #[allow(unreachable_code)]
    Err(format!(
        "没有 {}-{} 的发布包",
        std::env::consts::OS,
        std::env::consts::ARCH
    ))
}

pub(crate) fn running_from_cargo_target() -> bool {
    let Ok(path) = std::env::current_exe() else {
        return false;
    };
    let text = path
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    text.contains("/target/debug/") || text.contains("/target/release/")
}

pub(crate) fn current_restart_args() -> Vec<String> {
    let mut args = std::env::args().skip(1);
    let mut kept = Vec::new();
    while let Some(arg) = args.next() {
        if is_replace_flag(&arg) {
            if takes_replace_value(&arg) {
                let _ = args.next();
            }
            continue;
        }
        kept.push(arg);
    }
    kept
}

fn is_replace_flag(arg: &str) -> bool {
    matches!(
        arg,
        "--replace-from"
            | "--replace-to"
            | "--restart-arg"
            | "--wait-for-parent"
            | "--from-self-update"
    ) || arg.starts_with("--replace-from=")
        || arg.starts_with("--replace-to=")
        || arg.starts_with("--restart-arg=")
}

fn takes_replace_value(arg: &str) -> bool {
    matches!(arg, "--replace-from" | "--replace-to" | "--restart-arg")
}

pub(crate) fn ignore_terminal_hangup() {
    #[cfg(unix)]
    unsafe {
        // SAFETY: SIG_IGN 是进程级信号处置，不借用其他数据。替换后的新进程需要忽略挂起，
        // 否则原控制台关闭时会把刚拉起的进程一起带走。
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PackageVersion {
    pub(crate) major: u64,
    pub(crate) minor: u64,
    pub(crate) patch: u64,
    pub(crate) build: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteAsset {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteRelease {
    pub(crate) tag: String,
    pub(crate) version: String,
    pub(crate) commit: Option<String>,
    pub(crate) assets: Vec<RemoteAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlannedDownload {
    pub(crate) version: String,
    pub(crate) commit: Option<String>,
    pub(crate) archive: RemoteAsset,
    pub(crate) checksum: RemoteAsset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum UpdateDecision {
    Keep { message: String },
    Update(PlannedDownload),
}

#[derive(Debug, Clone)]
pub(crate) enum StageEvent {
    Downloading { bytes: u64, total: Option<u64> },
    Verifying,
    Extracting,
}

#[cfg(test)]
#[derive(Debug)]
enum StageOutcome {
    UpToDate { message: String },
    Staged { path: PathBuf, version: String },
}

pub(crate) struct StageRequest<'a> {
    pub(crate) manual_client: &'a reqwest::Client,
    pub(crate) feed: &'a str,
    pub(crate) install_exe: &'a Path,
    pub(crate) work_dir: &'a Path,
    pub(crate) cancel: &'a CancellationToken,
    pub(crate) events: Option<&'a tokio::sync::mpsc::UnboundedSender<StageEvent>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct UpdateSettings {
    #[serde(default = "default_interval")]
    pub(crate) app_update_interval: String,
    #[serde(default = "default_auto_apply")]
    pub(crate) auto_apply_app_update: bool,
    #[serde(default)]
    pub(crate) app_update_feed: Option<String>,
    #[serde(default)]
    pub(crate) proxy: Option<String>,
}

fn default_interval() -> String {
    "6h".to_string()
}

fn default_auto_apply() -> bool {
    true
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            app_update_interval: default_interval(),
            auto_apply_app_update: true,
            app_update_feed: None,
            proxy: None,
        }
    }
}

impl UpdateSettings {
    pub(crate) fn path() -> PathBuf {
        crate::paths::data_dir().join("update.json")
    }

    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path).map_err(|err| format!("读取更新配置失败：{err}"))?;
        let mut settings: Self =
            serde_json::from_str(&text).map_err(|err| format!("更新配置无法解析：{err}"))?;
        settings.normalize();
        settings.validate()?;
        Ok(settings)
    }

    pub(crate) fn save(path: &Path, settings: &Self) -> Result<(), String> {
        let mut settings = settings.clone();
        settings.normalize();
        settings.validate()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|err| format!("创建更新配置目录失败：{err}"))?;
        }
        let body = serde_json::to_vec_pretty(&settings)
            .map_err(|err| format!("序列化更新配置失败：{err}"))?;
        fs::write(path, body).map_err(|err| format!("写入更新配置失败：{err}"))
    }

    pub(crate) fn feed_url(&self) -> String {
        self.app_update_feed
            .clone()
            .unwrap_or_else(|| DEFAULT_RELEASE_API.to_string())
    }

    fn normalize(&mut self) {
        self.app_update_interval = self.app_update_interval.trim().to_string();
        if self.app_update_interval.is_empty() {
            self.app_update_interval = default_interval();
        }
        blank_to_none(&mut self.app_update_feed);
        blank_to_none(&mut self.proxy);
        if let Some(proxy) = &self.proxy {
            if proxy.eq_ignore_ascii_case("off") || proxy.eq_ignore_ascii_case("direct") {
                self.proxy = Some("off".to_string());
            }
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        parse_interval(&self.app_update_interval)?;
        if let Some(feed) = &self.app_update_feed {
            validate_feed_url(feed)?;
        }
        if let Some(proxy) = &self.proxy {
            if !proxy.eq_ignore_ascii_case("off") {
                validate_proxy_url(proxy)?;
            }
        }
        Ok(())
    }
}

fn blank_to_none(value: &mut Option<String>) {
    if value.as_deref().is_some_and(|text| text.trim().is_empty()) {
        *value = None;
    } else if let Some(text) = value.as_mut() {
        *text = text.trim().to_string();
    }
}

/// 解析 `6h` / `30m` / `1d` / `90s`。空字符串和 `off` 表示关闭。
pub(crate) fn parse_interval(raw: &str) -> Result<Option<Duration>, String> {
    let text = raw.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("off") || text == "0" {
        return Ok(None);
    }
    let split_at = text
        .find(|ch: char| !ch.is_ascii_digit())
        .unwrap_or(text.len());
    if split_at == 0 {
        return Err(format!("无法解析时间间隔「{raw}」，示例：6h、30m、1d"));
    }
    let number: u64 = text[..split_at]
        .parse()
        .map_err(|_| format!("无法解析时间间隔「{raw}」"))?;
    let unit = text[split_at..].trim();
    let seconds = match unit {
        "s" | "sec" | "secs" => number,
        "m" | "min" | "mins" => number.saturating_mul(60),
        "h" | "hr" | "hour" | "hours" => number.saturating_mul(3600),
        "d" | "day" | "days" => number.saturating_mul(86_400),
        _ => return Err(format!("无法解析时间间隔「{raw}」，单位用 s/m/h/d")),
    };
    if seconds == 0 {
        Ok(None)
    } else {
        Ok(Some(Duration::from_secs(seconds)))
    }
}

pub(crate) fn validate_feed_url(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|err| format!("程序更新源无效：{err}"))?;
    match url.scheme() {
        "https" => Ok(()),
        "http" if host_is_loopback(url.host_str()) => Ok(()),
        "http" => Err("程序更新源只允许 https，或本机 http".into()),
        other => Err(format!("程序更新源协议无效：{other}")),
    }
}

pub(crate) fn validate_proxy_url(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|err| format!("代理地址无效：{err}"))?;
    match url.scheme() {
        "http" | "https" | "socks5" | "socks5h" => {}
        other => {
            return Err(format!(
                "不支持的代理协议 {other}，可用 http、https、socks5、socks5h"
            ));
        }
    }
    if url.host_str().is_none() {
        return Err("代理地址缺少主机".into());
    }
    Ok(())
}

pub(crate) fn redact_proxy(value: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(value) else {
        return "已配置代理".into();
    };
    let has_secret = !url.username().is_empty() || url.password().is_some();
    if has_secret {
        let _ = url.set_username("***");
        let _ = url.set_password(None);
    }
    url.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProxyChoice {
    Direct,
    Url(String),
}

pub(crate) struct UpdateClients {
    pub(crate) version: reqwest::Client,
    pub(crate) download: reqwest::Client,
}

pub(crate) fn build_clients(config_proxy: Option<&str>) -> Result<UpdateClients, String> {
    let (choice, _) = resolve_proxy(config_proxy, env_proxy().as_deref())?;
    Ok(UpdateClients {
        version: version_client(&choice)?,
        download: manual_client(&choice)?,
    })
}

pub(crate) fn proxy_display_for(config_proxy: Option<&str>) -> String {
    resolve_proxy(config_proxy, env_proxy().as_deref())
        .map(|(_, display)| display)
        .unwrap_or_else(|err| format!("代理无效：{err}"))
}

fn resolve_proxy(
    config_proxy: Option<&str>,
    env_proxy: Option<&str>,
) -> Result<(ProxyChoice, String), String> {
    let configured = config_proxy
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match configured {
        Some(value) if is_direct_word(value) => Ok((ProxyChoice::Direct, "直连".to_string())),
        Some(value) => {
            validate_proxy_url(value)?;
            Ok((ProxyChoice::Url(value.to_string()), redact_proxy(value)))
        }
        None => match env_proxy.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) if is_direct_word(value) => Ok((ProxyChoice::Direct, "直连".to_string())),
            Some(value) => match validate_proxy_url(value) {
                Ok(()) => Ok((ProxyChoice::Url(value.to_string()), redact_proxy(value))),
                Err(err) => Ok((
                    ProxyChoice::Direct,
                    format!("环境变量里的代理无效，改为直连：{err}"),
                )),
            },
            None => Ok((ProxyChoice::Direct, "直连".to_string())),
        },
    }
}

fn env_proxy() -> Option<String> {
    for name in [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Some(value) = std::env::var(name)
            .ok()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
        {
            return Some(value);
        }
    }
    None
}

fn is_direct_word(value: &str) -> bool {
    value.eq_ignore_ascii_case("off") || value.eq_ignore_ascii_case("direct")
}

fn version_client(choice: &ProxyChoice) -> Result<reqwest::Client, String> {
    finish(
        base_builder(choice, Duration::from_secs(30))?.timeout(Duration::from_secs(30)),
        "创建版本查询客户端失败",
    )
}

fn manual_client(choice: &ProxyChoice) -> Result<reqwest::Client, String> {
    finish(
        base_builder(choice, Duration::from_secs(60))?,
        "创建程序更新下载客户端失败",
    )
}

fn base_builder(
    choice: &ProxyChoice,
    connect_timeout: Duration,
) -> Result<reqwest::ClientBuilder, String> {
    let builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .user_agent(USER_AGENT)
        .redirect(reqwest::redirect::Policy::none());
    match choice {
        ProxyChoice::Direct => Ok(builder.no_proxy()),
        ProxyChoice::Url(url) => {
            let proxy = reqwest::Proxy::all(url)
                .map_err(|err| format!("代理不可用：{err}"))?
                .no_proxy(reqwest::NoProxy::from_string("localhost,127.0.0.1,::1"));
            Ok(builder.proxy(proxy))
        }
    }
}

fn finish(builder: reqwest::ClientBuilder, context: &str) -> Result<reqwest::Client, String> {
    builder.build().map_err(|err| format!("{context}：{err}"))
}

pub(crate) fn decide_update(
    current_version: &str,
    current_commit: Option<&str>,
    target: &str,
    remote: &RemoteRelease,
) -> Result<UpdateDecision, String> {
    let local = parse_package_version(current_version)?;
    let remote_version = parse_package_version(&remote.version)?;
    let triple = (
        remote_version.major,
        remote_version.minor,
        remote_version.patch,
    )
        .cmp(&(local.major, local.minor, local.patch));
    if triple == Ordering::Less {
        return Ok(keep_older(&remote.version, current_version));
    }
    if triple == Ordering::Equal {
        match (local.build, remote_version.build) {
            (Some(local_build), Some(remote_build)) if remote_build > local_build => {}
            (Some(local_build), Some(remote_build)) if remote_build < local_build => {
                return Ok(UpdateDecision::Keep {
                    message: format!(
                        "远端构建 {remote_build} 比当前 {local_build} 旧，保持现在的程序。"
                    ),
                });
            }
            (Some(_), None) => {
                return Ok(UpdateDecision::Keep {
                    message: format!("远端 {} 没有更高的构建号，保持现在的程序。", remote.version),
                });
            }
            (None, Some(_)) => {}
            _ => {
                if let Some(message) = same_revision_message(
                    current_version,
                    current_commit,
                    &remote.version,
                    remote.commit.as_deref(),
                ) {
                    return Ok(UpdateDecision::Keep { message });
                }
            }
        }
    }
    let download = select_download(remote, target)?;
    Ok(UpdateDecision::Update(download))
}

fn keep_older(remote: &str, current: &str) -> UpdateDecision {
    UpdateDecision::Keep {
        message: format!("远端 {remote} 比当前 {current} 旧，保持现在的程序。"),
    }
}

fn same_revision_message(
    current_version: &str,
    current_commit: Option<&str>,
    remote_version: &str,
    remote_commit: Option<&str>,
) -> Option<String> {
    match (current_commit.map(str::trim), remote_commit) {
        (Some(local_commit), Some(remote_commit))
            if looks_like_commit(local_commit)
                && looks_like_commit(remote_commit)
                && !commits_differ(local_commit, remote_commit) =>
        {
            Some(format!("程序已是发布 {remote_version}。"))
        }
        (Some(_), Some(_)) => None,
        _ => Some(format!(
            "程序版本已经是 {current_version}。没有可对照的提交号，所以不重复下载。"
        )),
    }
}

pub(crate) async fn inspect_release(
    client: &reqwest::Client,
    feed: &str,
    cancel: &CancellationToken,
) -> Result<RemoteRelease, String> {
    fetch_release(client, feed, cancel).await
}

pub(crate) async fn stage_planned(
    request: &StageRequest<'_>,
    plan: &PlannedDownload,
) -> Result<PathBuf, String> {
    download_and_stage(request, plan).await
}

#[cfg(test)]
async fn stage_release(
    version_client: &reqwest::Client,
    request: StageRequest<'_>,
    current_version: &str,
    current_commit: Option<&str>,
    target: &str,
) -> Result<StageOutcome, String> {
    ensure_not_cancelled(request.cancel)?;
    let remote = fetch_release(version_client, request.feed, request.cancel).await?;
    match decide_update(current_version, current_commit, target, &remote)? {
        UpdateDecision::Keep { message } => Ok(StageOutcome::UpToDate { message }),
        UpdateDecision::Update(plan) => {
            let staged = download_and_stage(&request, &plan).await?;
            Ok(StageOutcome::Staged {
                path: staged,
                version: plan.version,
            })
        }
    }
}

pub(crate) async fn run_cli(check_only: bool, feed: Option<&str>) -> Result<(), String> {
    let settings = UpdateSettings::load(&UpdateSettings::path())?;
    let feed = match feed {
        Some(value) => {
            validate_feed_url(value)?;
            value.to_string()
        }
        None => settings.feed_url(),
    };
    let clients = build_clients(settings.proxy.as_deref())?;
    let cancel = CancellationToken::new();
    let target = current_release_target()?;
    let remote = inspect_release(&clients.version, &feed, &cancel).await?;
    match decide_update(current_version(), current_commit(), target, &remote)? {
        UpdateDecision::Keep { message } => {
            println!("{message}");
            Ok(())
        }
        UpdateDecision::Update(plan) => {
            if check_only {
                println!("发现程序 {}，还没有安装。", plan.version);
                return Ok(());
            }
            println!("正在下载程序 {}。", plan.version);
            let install_exe =
                std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))?;
            let staged = stage_planned(
                &StageRequest {
                    manual_client: &clients.download,
                    feed: &feed,
                    install_exe: &install_exe,
                    work_dir: &crate::paths::data_dir().join("updates"),
                    cancel: &cancel,
                    events: None,
                },
                &plan,
            )
            .await?;
            println!("正在换成程序 {} 并重启。", plan.version);
            relaunch_and_exit(&staged, &current_restart_args())
        }
    }
}

pub(crate) fn swap_executable(from: &Path, to: &Path) -> Result<(), String> {
    validate_swap_paths(from, to)?;
    swap_file(from, to)?;
    make_executable(to)?;
    if let Some(dir) = to.parent() {
        swap_staged_dlls(dir)?;
    }
    Ok(())
}

pub(crate) fn apply_replacement(
    from: &Path,
    to: &Path,
    wait_for_parent: bool,
    restart_args: &[String],
) -> Result<(), String> {
    if !wait_for_parent || from.as_os_str().is_empty() || to.as_os_str().is_empty() {
        return Err("替换参数不完整".into());
    }
    let current = std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))?;
    let current = current
        .canonicalize()
        .map_err(|err| format!("无法解析当前程序路径：{err}"))?;
    let to = to
        .canonicalize()
        .map_err(|err| format!("无法解析替换目标：{err}"))?;
    if current != to {
        return Err("只能替换当前正在运行的程序".into());
    }
    let from = from
        .canonicalize()
        .map_err(|err| format!("无法解析新程序：{err}"))?;
    validate_swap_paths(&from, &to)?;
    ignore_terminal_hangup();
    #[cfg(unix)]
    detach_session();
    wait_for_parent_exit()?;
    let mut last = "替换程序失败".to_string();
    for _ in 0..50 {
        match swap_executable(&from, &to) {
            Ok(()) => {
                restart_process(&to, restart_args)?;
                std::process::exit(0);
            }
            Err(err) => {
                last = err;
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    Err(last)
}

#[allow(clippy::mem_forget)]
pub(crate) fn relaunch_and_exit(staged: &Path, restart_args: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))?;
    let mut command = std::process::Command::new(&exe);
    command
        .arg("--replace-from")
        .arg(staged)
        .arg("--replace-to")
        .arg(&exe)
        .arg("--wait-for-parent");
    for arg in restart_args {
        command.arg("--restart-arg").arg(arg);
    }
    command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    match command.spawn() {
        Ok(child) => {
            // 保留子进程的 stdin 写端，直到本进程退出后内核关掉它。
            // 替换进程把管道结束当成“原进程已经退出”。
            std::mem::forget(child);
            eprintln!("正在退出以便完成程序更新");
            std::process::exit(0);
        }
        Err(err) => Err(format!("无法启动替换进程：{err}")),
    }
}

pub(crate) fn parse_package_version(text: &str) -> Result<PackageVersion, String> {
    let raw = text.trim().trim_start_matches('v');
    if raw.contains('-') {
        return Err(format!("版本号需要是 MAJOR.MINOR.PATCH：{text}"));
    }
    let (core, build) = match raw.split_once('+') {
        Some((core, build)) => (core, Some(parse_semver_part(Some(build), raw)?)),
        None => (raw, None),
    };
    let (major, minor, patch) = parse_semver(core)?;
    Ok(PackageVersion {
        major,
        minor,
        patch,
        build,
    })
}

pub(crate) fn parse_semver(text: &str) -> Result<(u64, u64, u64), String> {
    let raw = text.trim().trim_start_matches('v');
    let mut parts = raw.split('.');
    let major = parse_semver_part(parts.next(), raw)?;
    let minor = parse_semver_part(parts.next(), raw)?;
    let patch = parse_semver_part(parts.next(), raw)?;
    if parts.next().is_some() {
        return Err(format!("版本号需要是 MAJOR.MINOR.PATCH：{text}"));
    }
    Ok((major, minor, patch))
}

pub(crate) fn parse_sha256_file(text: &str) -> Result<String, String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let token = line.split_whitespace().next().unwrap_or("");
        if token.len() == 64 && token.chars().all(|ch| ch.is_ascii_hexdigit()) {
            return Ok(token.to_ascii_lowercase());
        }
    }
    Err("校验文件里没有 SHA256".into())
}

pub(crate) fn ensure_asset_url(feed: &str, asset: &str) -> Result<(), String> {
    let asset_url = reqwest::Url::parse(asset).map_err(|err| format!("更新包地址无效：{err}"))?;
    let feed_url = reqwest::Url::parse(feed).map_err(|err| format!("程序更新源无效：{err}"))?;
    if github_repo(feed).is_some() {
        let host = asset_url.host_str().unwrap_or("");
        let allowed = asset_url.scheme() == "https"
            && (host == "github.com"
                || host.ends_with(".github.com")
                || host.ends_with(".githubusercontent.com"));
        if !allowed {
            return Err(format!("程序更新包只能从 GitHub 下载，当前是 {asset}"));
        }
        return Ok(());
    }
    let same_host = asset_url.host_str() == feed_url.host_str();
    let loopback = host_is_loopback(asset_url.host_str());
    let scheme_ok = asset_url.scheme() == "https" || (asset_url.scheme() == "http" && loopback);
    if scheme_ok && (same_host || loopback && host_is_loopback(feed_url.host_str())) {
        Ok(())
    } else {
        Err(format!("程序更新包地址不在更新源允许的范围内：{asset}"))
    }
}

fn candidate_names(version: &str, tag: &str, target: &str) -> Vec<String> {
    let mut bases = vec![format!("aniorg-{version}-{target}")];
    let tagged = format!("aniorg-{tag}-{target}");
    if tagged != bases[0] {
        bases.push(tagged);
    }
    bases.push(format!("aniorg-{target}"));
    bases.dedup();
    let prefer_zip = target.contains("windows");
    let mut names = Vec::new();
    for base in bases {
        if prefer_zip {
            names.push(format!("{base}.zip"));
            names.push(format!("{base}.tar.gz"));
        } else {
            names.push(format!("{base}.tar.gz"));
            names.push(format!("{base}.zip"));
        }
    }
    names
}

fn select_download(remote: &RemoteRelease, target: &str) -> Result<PlannedDownload, String> {
    let archive = candidate_names(&remote.version, &remote.tag, target)
        .into_iter()
        .find_map(|name| {
            remote
                .assets
                .iter()
                .find(|asset| asset.name == name)
                .cloned()
        })
        .ok_or_else(|| format!("当前平台没有对应的发布包：{target}"))?;
    let checksum_name = format!("{}.sha256", archive.name);
    let checksum = remote
        .assets
        .iter()
        .find(|asset| asset.name == checksum_name)
        .cloned()
        .ok_or_else(|| format!("发布里没有 {checksum_name}"))?;
    Ok(PlannedDownload {
        version: remote.version.clone(),
        commit: remote.commit.clone(),
        archive,
        checksum,
    })
}

async fn fetch_release(
    client: &reqwest::Client,
    feed: &str,
    cancel: &CancellationToken,
) -> Result<RemoteRelease, String> {
    ensure_not_cancelled(cancel)?;
    validate_feed_url(feed)?;
    let text = get_text(client, feed, feed, cancel, MAX_TEXT_BYTES).await?;
    let document: ReleaseDocument =
        serde_json::from_str(&text).map_err(|err| format!("发布信息无法解析：{err}"))?;
    let version = document.tag_name.trim().trim_start_matches('v').to_string();
    parse_package_version(&version)?;
    let mut commit = document
        .commit
        .as_deref()
        .filter(|value| looks_like_commit(value))
        .map(ToOwned::to_owned)
        .or_else(|| {
            document
                .target_commitish
                .as_deref()
                .filter(|value| looks_like_commit(value))
                .map(ToOwned::to_owned)
        });
    if commit.is_none() {
        if let Some((owner, repo)) = github_repo(feed) {
            commit = fetch_tag_commit(client, &owner, &repo, &document.tag_name, cancel)
                .await
                .ok();
        }
    }
    let assets = document
        .assets
        .into_iter()
        .map(|asset| RemoteAsset {
            name: asset.name,
            url: asset.browser_download_url,
            size: asset.size,
        })
        .collect();
    Ok(RemoteRelease {
        tag: document.tag_name,
        version,
        commit,
        assets,
    })
}

async fn fetch_tag_commit(
    client: &reqwest::Client,
    owner: &str,
    repo: &str,
    tag: &str,
    cancel: &CancellationToken,
) -> Result<String, String> {
    if parse_package_version(tag.trim_start_matches('v')).is_err() {
        return Err("标签不是稳定版本".into());
    }
    let url = format!(
        "https://api.github.com/repos/{owner}/{repo}/git/refs/tags/{}",
        encode_ref_component(tag)
    );
    let text = get_text(client, &url, &url, cancel, MAX_TEXT_BYTES).await?;
    let reference: GitRef =
        serde_json::from_str(&text).map_err(|err| format!("标签信息无法解析：{err}"))?;
    if reference.object.kind == "commit" && looks_like_commit(&reference.object.sha) {
        return Ok(reference.object.sha);
    }
    if reference.object.kind == "tag" {
        let tag_url = format!(
            "https://api.github.com/repos/{owner}/{repo}/git/tags/{}",
            reference.object.sha
        );
        let text = get_text(client, &tag_url, &tag_url, cancel, MAX_TEXT_BYTES).await?;
        let annotated: GitTag =
            serde_json::from_str(&text).map_err(|err| format!("附注标签无法解析：{err}"))?;
        if annotated.object.kind == "commit" && looks_like_commit(&annotated.object.sha) {
            return Ok(annotated.object.sha);
        }
    }
    Err("标签没有指向提交".into())
}

async fn download_and_stage(
    request: &StageRequest<'_>,
    plan: &PlannedDownload,
) -> Result<PathBuf, String> {
    ensure_asset_url(request.feed, &plan.archive.url)?;
    ensure_asset_url(request.feed, &plan.checksum.url)?;
    fs::create_dir_all(request.work_dir).map_err(|err| format!("创建更新目录失败：{err}"))?;
    let want = plan
        .archive
        .size
        .unwrap_or(32 * 1024 * 1024)
        .saturating_add(MAX_BINARY_BYTES);
    ensure_space(request.work_dir, want)?;
    let checksum_text = get_text(
        request.manual_client,
        &plan.checksum.url,
        request.feed,
        request.cancel,
        MAX_TEXT_BYTES,
    )
    .await?;
    let expected = parse_sha256_file(&checksum_text)?;
    let archive_path = request.work_dir.join(safe_file_name(&plan.archive.name)?);
    let part_path = archive_path.with_extension("part");
    let extracted = request.work_dir.join("aniorg.next");
    let outcome = async {
        download_file(
            request.manual_client,
            &plan.archive.url,
            request.feed,
            &part_path,
            MAX_ARCHIVE_BYTES,
            request.cancel,
            request.events,
        )
        .await?;
        emit(request.events, StageEvent::Verifying);
        let actual = tokio::task::spawn_blocking({
            let part_path = part_path.clone();
            move || sha256_file(&part_path)
        })
        .await
        .map_err(|err| format!("校验任务中断：{err}"))??;
        if actual != expected {
            return Err("程序包 SHA256 不一致".into());
        }
        fs::rename(&part_path, &archive_path).map_err(|err| format!("保存程序包失败：{err}"))?;
        emit(request.events, StageEvent::Extracting);
        let archive_path = archive_path.clone();
        let extracted_for_task = extracted.clone();
        tokio::task::spawn_blocking(move || extract_payload(&archive_path, &extracted_for_task))
            .await
            .map_err(|err| format!("解压任务中断：{err}"))??;
        let staged = staged_path(request.install_exe);
        copy_tree(&extracted, &staged)?;
        Ok(staged)
    }
    .await;
    if outcome.is_err() {
        let _ = fs::remove_file(&part_path);
        let _ = fs::remove_file(&extracted);
        if let Some(dir) = extracted.parent() {
            let _ = remove_extracted_dlls(dir);
        }
    }
    outcome
}

async fn get_text(
    client: &reqwest::Client,
    url: &str,
    feed: &str,
    cancel: &CancellationToken,
    max_bytes: u64,
) -> Result<String, String> {
    let response = open_checked(client, url, feed, cancel).await?;
    let bytes = read_body(response, max_bytes, cancel).await?;
    String::from_utf8(bytes).map_err(|_| "响应不是 UTF-8".into())
}

async fn open_checked(
    client: &reqwest::Client,
    start: &str,
    feed: &str,
    cancel: &CancellationToken,
) -> Result<reqwest::Response, String> {
    let mut next = start.to_string();
    for _ in 0..6 {
        ensure_not_cancelled(cancel)?;
        ensure_asset_url(feed, &next)?;
        let response = with_github_auth(client.get(&next), &next)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .send()
            .await
            .map_err(|err| format!("请求更新信息失败：{err}"))?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or_else(|| "重定向缺少 Location".to_string())?;
            let location = location
                .to_str()
                .map_err(|_| "重定向地址无效".to_string())?;
            let base = reqwest::Url::parse(&next).map_err(|err| format!("更新地址无效：{err}"))?;
            next = base
                .join(location)
                .map_err(|err| format!("重定向地址无效：{err}"))?
                .to_string();
            continue;
        }
        let final_url = response.url().clone();
        ensure_asset_url(feed, final_url.as_str())?;
        if !response.status().is_success() {
            return Err(format!("更新请求返回 HTTP {}", response.status()));
        }
        return Ok(response);
    }
    Err("重定向过多".into())
}

fn with_github_auth(request: reqwest::RequestBuilder, url: &str) -> reqwest::RequestBuilder {
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return request;
    };
    if parsed.host_str() != Some("api.github.com") {
        return request;
    }
    let token = std::env::var("ANIORG_GITHUB_TOKEN")
        .or_else(|_| std::env::var("GITHUB_TOKEN"))
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if let Some(token) = token {
        request.bearer_auth(token)
    } else {
        request
    }
}

async fn read_body(
    mut response: reqwest::Response,
    max_bytes: u64,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, String> {
    if let Some(length) = response.content_length() {
        if length > max_bytes {
            return Err("更新响应过大".into());
        }
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("读取更新响应失败：{err}"))?
    {
        ensure_not_cancelled(cancel)?;
        if body.len() as u64 + chunk.len() as u64 > max_bytes {
            return Err("更新响应过大".into());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    feed: &str,
    dest: &Path,
    max_bytes: u64,
    cancel: &CancellationToken,
    events: Option<&tokio::sync::mpsc::UnboundedSender<StageEvent>>,
) -> Result<(), String> {
    let mut response = open_checked(client, url, feed, cancel).await?;
    if let Some(length) = response.content_length() {
        if length > max_bytes {
            return Err(format!("程序包超过 {}，已放弃", format_bytes(max_bytes)));
        }
    }
    let total = response.content_length();
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建更新目录失败：{err}"))?;
    }
    let mut file = tokio::fs::File::create(dest)
        .await
        .map_err(|err| format!("创建程序包失败：{err}"))?;
    let mut written = 0u64;
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|err| format!("下载程序包失败：{err}"))?;
        let Some(chunk) = chunk else {
            break;
        };
        ensure_not_cancelled(cancel)?;
        written = written.saturating_add(chunk.len() as u64);
        if written > max_bytes {
            return Err(format!("程序包超过 {}，已放弃", format_bytes(max_bytes)));
        }
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|err| format!("写入程序包失败：{err}"))?;
        emit(
            events,
            StageEvent::Downloading {
                bytes: written,
                total,
            },
        );
    }
    file.sync_all()
        .await
        .map_err(|err| format!("保存程序包失败：{err}"))?;
    Ok(())
}

fn extract_payload(archive: &Path, dest: &Path) -> Result<(), String> {
    let entries = list_entries(archive)?;
    let binary = entries
        .iter()
        .find(|path| is_binary_name(path))
        .cloned()
        .ok_or_else(|| "压缩包里没有 aniorg".to_string())?;
    let parent = binary.parent().map(Path::to_path_buf);
    let mut written = Vec::new();
    let result = (|| {
        extract_entry(archive, &binary, dest)?;
        written.push(dest.to_path_buf());
        verify_executable_magic(dest)?;
        let Some(out_dir) = dest.parent() else {
            return Ok(());
        };
        for entry in &entries {
            if entry.parent() != parent.as_deref() || !is_dll(entry) {
                continue;
            }
            let Some(name) = entry.file_name() else {
                continue;
            };
            let output = out_dir.join(name);
            extract_entry(archive, entry, &output)?;
            written.push(output);
        }
        Ok(())
    })();
    if result.is_err() {
        for path in written {
            let _ = fs::remove_file(path);
        }
    }
    result
}

fn list_entries(archive: &Path) -> Result<Vec<PathBuf>, String> {
    if is_zip(archive) {
        list_zip_entries(archive)
    } else {
        list_tar_entries(archive)
    }
}

fn extract_entry(archive: &Path, wanted: &Path, dest: &Path) -> Result<(), String> {
    if is_zip(archive) {
        extract_zip_entry(archive, wanted, dest)
    } else {
        extract_tar_entry(archive, wanted, dest)
    }
}

fn is_zip(archive: &Path) -> bool {
    archive
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("zip"))
}

fn list_zip_entries(archive: &Path) -> Result<Vec<PathBuf>, String> {
    let file = File::open(archive).map_err(|err| format!("打开压缩包失败：{err}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|err| format!("压缩包无法读取：{err}"))?;
    if zip.len() > MAX_ENTRIES {
        return Err("压缩包条目过多".into());
    }
    let mut names = Vec::new();
    for index in 0..zip.len() {
        let entry = zip
            .by_index(index)
            .map_err(|err| format!("读取压缩条目失败：{err}"))?;
        let Some(path) = entry.enclosed_name() else {
            return Err("压缩包包含非法路径".into());
        };
        names.push(path);
    }
    Ok(names)
}

fn extract_zip_entry(archive: &Path, wanted: &Path, dest: &Path) -> Result<(), String> {
    let file = File::open(archive).map_err(|err| format!("打开压缩包失败：{err}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|err| format!("压缩包无法读取：{err}"))?;
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|err| format!("读取压缩条目失败：{err}"))?;
        let Some(path) = entry.enclosed_name() else {
            return Err("压缩包包含非法路径".into());
        };
        if path == wanted {
            return copy_limited(&mut entry, dest);
        }
    }
    Err("压缩包里没有 aniorg".into())
}

fn list_tar_entries(archive: &Path) -> Result<Vec<PathBuf>, String> {
    let mut names = Vec::new();
    for_each_tar_entry(archive, |path, _| {
        names.push(path);
        Ok(())
    })?;
    Ok(names)
}

fn extract_tar_entry(archive: &Path, wanted: &Path, dest: &Path) -> Result<(), String> {
    let mut found = false;
    for_each_tar_entry(archive, |path, entry| {
        if path == wanted {
            copy_limited(entry, dest)?;
            found = true;
        }
        Ok(())
    })?;
    if found {
        Ok(())
    } else {
        Err("压缩包里没有 aniorg".into())
    }
}

fn for_each_tar_entry(
    archive: &Path,
    mut visit: impl FnMut(PathBuf, &mut tar::Entry<flate2::read::GzDecoder<File>>) -> Result<(), String>,
) -> Result<(), String> {
    let file = File::open(archive).map_err(|err| format!("打开压缩包失败：{err}"))?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    let mut seen = 0usize;
    for entry in tar
        .entries()
        .map_err(|err| format!("压缩包无法读取：{err}"))?
    {
        seen += 1;
        if seen > MAX_ENTRIES {
            return Err("压缩包条目过多".into());
        }
        let mut entry = entry.map_err(|err| format!("读取压缩条目失败：{err}"))?;
        let path = entry
            .path()
            .map_err(|err| format!("压缩包路径无效：{err}"))?
            .into_owned();
        if !path_is_safe(&path) {
            return Err("压缩包包含非法路径".into());
        }
        visit(path, &mut entry)?;
    }
    Ok(())
}

fn copy_limited(reader: &mut impl Read, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建更新目录失败：{err}"))?;
    }
    let mut out = File::create(dest).map_err(|err| format!("写出程序失败：{err}"))?;
    let mut buf = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|err| format!("解压程序失败：{err}"))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_BINARY_BYTES {
            let _ = fs::remove_file(dest);
            return Err(format!(
                "解压后的程序超过 {}",
                format_bytes(MAX_BINARY_BYTES)
            ));
        }
        out.write_all(&buf[..read])
            .map_err(|err| format!("写出程序失败：{err}"))?;
    }
    if total == 0 {
        let _ = fs::remove_file(dest);
        return Err("解压后的程序是空的".into());
    }
    out.sync_all()
        .map_err(|err| format!("保存程序失败：{err}"))?;
    Ok(())
}

fn verify_executable_magic(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|err| format!("读取新程序失败：{err}"))?;
    let mut magic = [0u8; 4];
    file.read_exact(&mut magic)
        .map_err(|_| "解出的文件不是可执行程序".to_string())?;
    let macho = matches!(
        magic,
        [0xFE, 0xED, 0xFA, 0xCE]
            | [0xFE, 0xED, 0xFA, 0xCF]
            | [0xCE, 0xFA, 0xED, 0xFE]
            | [0xCF, 0xFA, 0xED, 0xFE]
            | [0xCA, 0xFE, 0xBA, 0xBE]
            | [0xBE, 0xBA, 0xFE, 0xCA]
    );
    if magic.starts_with(b"\x7FELF") || magic.starts_with(b"MZ") || macho {
        Ok(())
    } else {
        let _ = fs::remove_file(path);
        Err("解出的文件不是可执行程序".into())
    }
}

fn copy_tree(source: &Path, staged: &Path) -> Result<(), String> {
    copy_staged(source, staged)?;
    let Some(source_dir) = source.parent() else {
        return Ok(());
    };
    let Some(staged_dir) = staged.parent() else {
        return Ok(());
    };
    for entry in fs::read_dir(source_dir).map_err(|err| format!("读取更新目录失败：{err}"))?
    {
        let entry = entry.map_err(|err| format!("读取更新目录失败：{err}"))?;
        let path = entry.path();
        if !is_dll(&path) {
            continue;
        }
        let Some(name) = path.file_name() else {
            continue;
        };
        let mut staged_name = name.to_os_string();
        staged_name.push(".new");
        copy_staged(&path, &staged_dir.join(staged_name))?;
    }
    Ok(())
}

fn copy_staged(source: &Path, staged: &Path) -> Result<(), String> {
    if let Some(parent) = staged.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("创建程序目录失败：{err}"))?;
    }
    fs::copy(source, staged).map_err(|err| {
        format!("程序所在目录不可写，无法替换自己（{err}）。可以把程序放到可写目录后再更新。")
    })?;
    if is_binary_name(staged) || staged_binary_name(staged) {
        make_executable(staged)?;
    }
    let file = File::options()
        .write(true)
        .open(staged)
        .map_err(|err| format!("打开新程序失败：{err}"))?;
    file.sync_all()
        .map_err(|err| format!("保存新程序失败：{err}"))?;
    Ok(())
}

fn staged_binary_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name == "aniorg.new" || name == "aniorg.exe.new")
}

fn staged_path(exe: &Path) -> PathBuf {
    let name = exe.file_name().unwrap_or(std::ffi::OsStr::new("aniorg"));
    let mut staged = name.to_os_string();
    staged.push(".new");
    exe.with_file_name(staged)
}

#[cfg(windows)]
fn backup_path(exe: &Path) -> PathBuf {
    let name = exe.file_name().unwrap_or(std::ffi::OsStr::new("aniorg"));
    let mut backup = name.to_os_string();
    backup.push(".old");
    exe.with_file_name(backup)
}

fn validate_swap_paths(from: &Path, to: &Path) -> Result<(), String> {
    let from_name = from.file_name().ok_or("替换来源缺少文件名")?;
    let to_name = to.file_name().ok_or("替换目标缺少文件名")?;
    let mut expected = to_name.to_os_string();
    expected.push(".new");
    if from_name != expected.as_os_str() {
        return Err("替换来源必须是目标旁边的 .new 文件".into());
    }
    if from.parent() != to.parent() {
        return Err("替换文件必须和当前程序在同一目录".into());
    }
    if !from.is_file() {
        return Err("找不到新程序文件".into());
    }
    Ok(())
}

fn swap_file(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        fs::rename(from, to).map_err(|err| format!("替换程序失败：{err}"))?;
        Ok(())
    }
    #[cfg(windows)]
    {
        if to.exists() {
            let backup = backup_path(to);
            let _ = fs::remove_file(&backup);
            fs::rename(to, &backup).map_err(|err| format!("移开正在运行的程序失败：{err}"))?;
            if let Err(err) = fs::rename(from, to) {
                let _ = fs::rename(&backup, to);
                return Err(format!("写入新程序失败：{err}"));
            }
            let _ = fs::remove_file(&backup);
        } else {
            fs::rename(from, to).map_err(|err| format!("写入新程序失败：{err}"))?;
        }
        Ok(())
    }
}

fn swap_staged_dlls(dir: &Path) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) => return Err(format!("读取程序目录失败：{err}")),
    };
    for entry in entries {
        let entry = entry.map_err(|err| format!("读取程序目录失败：{err}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(dll_name) = name.strip_suffix(".dll.new") else {
            continue;
        };
        if dll_name.is_empty() || dll_name.contains(['/', '\\']) || dll_name.contains("..") {
            continue;
        }
        swap_file(&path, &dir.join(format!("{dll_name}.dll")))?;
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(path)
            .map_err(|err| format!("读取程序权限失败：{err}"))?
            .permissions();
        permissions.set_mode(permissions.mode() | 0o755);
        fs::set_permissions(path, permissions).map_err(|err| format!("设置程序权限失败：{err}"))?;
    }
    let _ = path;
    Ok(())
}

fn restart_process(exe: &Path, args: &[String]) -> Result<(), String> {
    std::process::Command::new(exe)
        .arg("--from-self-update")
        .args(args)
        .stdin(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|err| format!("重新启动失败：{err}"))
}

fn wait_for_parent_exit() -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buf = [0u8; 32];
        let result = std::io::Read::read(&mut stdin, &mut buf);
        let _ = tx.send(result);
    });
    match rx.recv_timeout(Duration::from_secs(120)) {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(format!("等待原进程退出失败：{err}")),
        Err(_) => Err("等待原进程退出超时".into()),
    }
}

#[cfg(unix)]
fn detach_session() {
    unsafe {
        // SAFETY: setsid 只作用于当前进程，失败时继续替换。调用前没有别的线程依赖控制终端。
        let _ = libc::setsid();
    }
}

fn ensure_not_cancelled(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("已取消".into())
    } else {
        Ok(())
    }
}

fn emit(events: Option<&tokio::sync::mpsc::UnboundedSender<StageEvent>>, event: StageEvent) {
    if let Some(sender) = events {
        let _ = sender.send(event);
    }
}

fn ensure_space(dir: &Path, want: u64) -> Result<(), String> {
    let probe = if dir.exists() {
        dir.to_path_buf()
    } else {
        dir.parent().unwrap_or(dir).to_path_buf()
    };
    match free_bytes(&probe) {
        Some(free) if free < want => Err(format!(
            "磁盘剩余 {}，程序更新至少需要 {}",
            format_bytes(free),
            format_bytes(want)
        )),
        _ => Ok(()),
    }
}

#[cfg(unix)]
fn disk_count(value: impl Into<u128>) -> Option<u64> {
    u64::try_from(value.into()).ok()
}

fn free_bytes(path: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        // SAFETY: statvfs 在返回 0 时写满 `stat`；路径是以 NUL 结尾的 CString。
        let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
        if rc != 0 {
            return None;
        }
        // SAFETY: 上一步成功，statvfs 已初始化整块结构。
        let stat = unsafe { stat.assume_init() };
        let blocks = disk_count(stat.f_bavail)?;
        let fragment = disk_count(stat.f_frsize)?;
        Some(blocks.saturating_mul(fragment))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|err| format!("读取程序包失败：{err}"))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|err| format!("读取程序包失败：{err}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn safe_file_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.contains(['/', '\\']) || name.contains("..") || name.starts_with('.')
    {
        Err(format!("发布文件名无效：{name}"))
    } else {
        Ok(name)
    }
}

fn path_is_safe(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn is_binary_name(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("aniorg" | "aniorg.exe")
    )
}

fn is_dll(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
}

fn remove_extracted_dlls(dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|err| format!("读取更新目录失败：{err}"))? {
        let entry = entry.map_err(|err| format!("读取更新目录失败：{err}"))?;
        if is_dll(&entry.path()) {
            let _ = fs::remove_file(entry.path());
        }
    }
    Ok(())
}

fn looks_like_commit(value: &str) -> bool {
    let value = value.trim();
    (7..=40).contains(&value.len()) && value.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn commits_differ(left: &str, right: &str) -> bool {
    let left = left.trim().to_ascii_lowercase();
    let right = right.trim().to_ascii_lowercase();
    !(left.starts_with(&right) || right.starts_with(&left))
}

fn parse_semver_part(part: Option<&str>, raw: &str) -> Result<u64, String> {
    let Some(part) = part else {
        return Err(format!("版本号需要是 MAJOR.MINOR.PATCH：{raw}"));
    };
    if part.is_empty()
        || (part.len() > 1 && part.starts_with('0'))
        || !part.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("版本号需要是 MAJOR.MINOR.PATCH：{raw}"));
    }
    part.parse()
        .map_err(|_| format!("版本号需要是 MAJOR.MINOR.PATCH：{raw}"))
}

fn host_is_loopback(host: Option<&str>) -> bool {
    matches!(host, Some("localhost" | "127.0.0.1" | "::1"))
}

fn github_repo(feed: &str) -> Option<(String, String)> {
    let url = reqwest::Url::parse(feed).ok()?;
    if url.host_str() != Some("api.github.com") {
        return None;
    }
    let mut parts = url.path().split('/').filter(|part| !part.is_empty());
    if parts.next() != Some("repos") {
        return None;
    }
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    Some((owner, repo))
}

fn encode_ref_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[derive(Debug, Deserialize)]
struct ReleaseDocument {
    tag_name: String,
    #[serde(default)]
    target_commitish: Option<String>,
    #[serde(default)]
    commit: Option<String>,
    #[serde(default)]
    assets: Vec<ReleaseAssetDocument>,
}

#[derive(Debug, Deserialize)]
struct ReleaseAssetDocument {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct GitRef {
    object: GitObject,
}

#[derive(Debug, Deserialize)]
struct GitTag {
    object: GitObject,
}

#[derive(Debug, Deserialize)]
struct GitObject {
    sha: String,
    #[serde(rename = "type")]
    kind: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, url: &str) -> RemoteAsset {
        RemoteAsset {
            name: name.into(),
            url: url.into(),
            size: None,
        }
    }

    fn release(version: &str, commit: Option<&str>, target: &str) -> RemoteRelease {
        let tag = format!("v{version}");
        let archive = format!("aniorg-{version}-{target}.tar.gz");
        RemoteRelease {
            tag: tag.clone(),
            version: version.into(),
            commit: commit.map(str::to_string),
            assets: vec![
                asset(
                    &archive,
                    "https://github.com/ModerRAS/anime-organizer/releases/download/app.tar.gz",
                ),
                asset(
                    &format!("{archive}.sha256"),
                    "https://github.com/ModerRAS/anime-organizer/releases/download/app.tar.gz.sha256",
                ),
            ],
        }
    }

    #[test]
    fn newer_version_is_selected() {
        let remote = release("0.2.0", None, "x86_64-unknown-linux-gnu");
        match decide_update("0.1.0", None, "x86_64-unknown-linux-gnu", &remote).unwrap() {
            UpdateDecision::Update(plan) => assert_eq!(plan.version, "0.2.0"),
            UpdateDecision::Keep { message } => panic!("应更新：{message}"),
        }
    }

    #[test]
    fn higher_build_number_updates_without_commit() {
        let remote = release("1.0.0+12", None, "x86_64-unknown-linux-gnu");
        match decide_update("1.0.0+3", None, "x86_64-unknown-linux-gnu", &remote).unwrap() {
            UpdateDecision::Update(plan) => {
                assert!(plan.archive.name.contains("1.0.0+12"));
            }
            UpdateDecision::Keep { message } => panic!("更高构建号应更新：{message}"),
        }
    }

    #[test]
    fn lower_build_number_is_kept() {
        let remote = release("1.0.0+3", None, "x86_64-unknown-linux-gnu");
        match decide_update("1.0.0+12", None, "x86_64-unknown-linux-gnu", &remote).unwrap() {
            UpdateDecision::Keep { message } => assert!(message.contains("旧"), "{message}"),
            UpdateDecision::Update(_) => panic!("不能降到更低构建号"),
        }
    }

    #[test]
    fn plain_version_moves_to_numbered_build() {
        let remote = release("1.0.0+4", None, "x86_64-unknown-linux-gnu");
        match decide_update("1.0.0", None, "x86_64-unknown-linux-gnu", &remote).unwrap() {
            UpdateDecision::Update(plan) => assert_eq!(plan.version, "1.0.0+4"),
            UpdateDecision::Keep { message } => panic!("应更新到带构建号的发布：{message}"),
        }
    }

    #[test]
    fn unnumbered_remote_does_not_replace_numbered_build() {
        let remote = release(
            "1.0.0",
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "x86_64-unknown-linux-gnu",
        );
        match decide_update(
            "1.0.0+4",
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            "x86_64-unknown-linux-gnu",
            &remote,
        )
        .unwrap()
        {
            UpdateDecision::Keep { .. } => {}
            UpdateDecision::Update(_) => panic!("不能从带构建号的发布退回纯版本号"),
        }
    }

    #[test]
    fn same_version_without_commit_stays() {
        let remote = release(
            "0.1.0",
            Some("0123456789abcdef"),
            "x86_64-unknown-linux-gnu",
        );
        match decide_update("0.1.0", None, "x86_64-unknown-linux-gnu", &remote).unwrap() {
            UpdateDecision::Keep { message } => assert!(message.contains("提交号")),
            UpdateDecision::Update(_) => panic!("开发构建不应因同版本重复下载"),
        }
    }

    #[test]
    fn same_version_different_commit_updates() {
        let remote = release(
            "0.1.0",
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "x86_64-pc-windows-msvc",
        );
        let remote = RemoteRelease {
            assets: vec![
                asset(
                    "aniorg-v0.1.0-x86_64-pc-windows-msvc.zip",
                    "https://github.com/ModerRAS/anime-organizer/releases/download/app.zip",
                ),
                asset(
                    "aniorg-v0.1.0-x86_64-pc-windows-msvc.zip.sha256",
                    "https://github.com/ModerRAS/anime-organizer/releases/download/app.zip.sha256",
                ),
            ],
            ..remote
        };
        match decide_update(
            "0.1.0",
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            "x86_64-pc-windows-msvc",
            &remote,
        )
        .unwrap()
        {
            UpdateDecision::Update(plan) => assert!(plan.archive.name.ends_with(".zip")),
            UpdateDecision::Keep { message } => panic!("同版本新提交应更新：{message}"),
        }
    }

    #[test]
    fn older_release_is_kept() {
        let remote = release(
            "0.1.0",
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "x86_64-unknown-linux-gnu",
        );
        match decide_update(
            "0.2.0",
            Some("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
            "x86_64-unknown-linux-gnu",
            &remote,
        )
        .unwrap()
        {
            UpdateDecision::Keep { message } => assert!(message.contains("旧")),
            UpdateDecision::Update(_) => panic!("不能降级"),
        }
    }

    #[test]
    fn unversioned_asset_is_accepted() {
        let mut remote = release("1.2.0", None, "aarch64-apple-darwin");
        remote.assets = vec![
            asset(
                "aniorg-aarch64-apple-darwin.tar.gz",
                "https://github.com/ModerRAS/anime-organizer/releases/download/app.tar.gz",
            ),
            asset(
                "aniorg-aarch64-apple-darwin.tar.gz.sha256",
                "https://github.com/ModerRAS/anime-organizer/releases/download/app.tar.gz.sha256",
            ),
        ];
        match decide_update("1.0.0", None, "aarch64-apple-darwin", &remote).unwrap() {
            UpdateDecision::Update(plan) => {
                assert_eq!(plan.archive.name, "aniorg-aarch64-apple-darwin.tar.gz");
            }
            UpdateDecision::Keep { message } => panic!("应选中不带版本的包：{message}"),
        }
    }

    #[test]
    fn sha256_accepts_hash_or_sum_line() {
        assert_eq!(
            parse_sha256_file("abc\nabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789  app.tar.gz\n").unwrap(),
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
        );
    }

    #[test]
    fn github_assets_stay_on_github() {
        assert!(ensure_asset_url(
            DEFAULT_RELEASE_API,
            "https://github.com/ModerRAS/anime-organizer/releases/download/v1.0.0/app.tar.gz"
        )
        .is_ok());
        assert!(ensure_asset_url(
            DEFAULT_RELEASE_API,
            "https://release-assets.githubusercontent.com/app"
        )
        .is_ok());
        assert!(ensure_asset_url(DEFAULT_RELEASE_API, "https://evil.example/app").is_err());
        assert!(ensure_asset_url(DEFAULT_RELEASE_API, "http://127.0.0.1/app").is_err());
    }

    #[test]
    fn settings_roundtrip_and_proxy_rules() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("update.json");
        assert!(UpdateSettings::load(&path).unwrap().auto_apply_app_update);
        let settings = UpdateSettings {
            app_update_interval: " 30m ".into(),
            auto_apply_app_update: false,
            app_update_feed: Some(" https://example.com/latest ".into()),
            proxy: Some(" http://user:secret@127.0.0.1:7890 ".into()),
        };
        UpdateSettings::save(&path, &settings).unwrap();
        let loaded = UpdateSettings::load(&path).unwrap();
        assert_eq!(loaded.app_update_interval, "30m");
        assert!(!loaded.auto_apply_app_update);
        assert_eq!(
            loaded.app_update_feed.as_deref(),
            Some("https://example.com/latest")
        );
        assert!(!redact_proxy(loaded.proxy.as_deref().unwrap()).contains("secret"));
        assert!(UpdateSettings {
            proxy: Some("file:///tmp/proxy".into()),
            ..UpdateSettings::default()
        }
        .validate()
        .is_err());
        assert!(validate_feed_url("http://example.com/latest").is_err());
        assert!(validate_feed_url("http://127.0.0.1:9/latest").is_ok());
        assert_eq!(parse_interval("off").unwrap(), None);
        assert_eq!(
            parse_interval("6h").unwrap(),
            Some(Duration::from_secs(6 * 3600))
        );
    }

    #[test]
    fn configured_proxy_beats_environment_and_off_ignores_it() {
        let (choice, display) =
            resolve_proxy(Some("http://127.0.0.1:7890"), Some("http://10.0.0.1:1")).unwrap();
        assert_eq!(choice, ProxyChoice::Url("http://127.0.0.1:7890".into()));
        assert!(display.contains("127.0.0.1"));
        let (choice, display) = resolve_proxy(Some("off"), Some("http://10.0.0.1:1")).unwrap();
        assert_eq!(choice, ProxyChoice::Direct);
        assert_eq!(display, "直连");
        let (choice, _) = resolve_proxy(None, Some("ftp://127.0.0.1:1")).unwrap();
        assert_eq!(choice, ProxyChoice::Direct);
    }

    #[test]
    fn swap_replaces_destination_bytes_and_sidecar_dll() {
        let dir = tempfile::tempdir().expect("temp");
        let dest = dir.path().join("aniorg");
        let source = dir.path().join("aniorg.new");
        fs::write(&dest, b"old").unwrap();
        fs::write(&source, b"new").unwrap();
        fs::write(dir.path().join("DirectML.dll"), b"old-dll").unwrap();
        fs::write(dir.path().join("DirectML.dll.new"), b"new-dll").unwrap();
        swap_executable(&source, &dest).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"new");
        assert!(!source.exists());
        assert_eq!(
            fs::read(dir.path().join("DirectML.dll")).unwrap(),
            b"new-dll"
        );
        assert!(!dir.path().join("DirectML.dll.new").exists());
    }

    #[test]
    fn rejects_parent_directory_in_archive() {
        let dir = tempfile::tempdir().expect("temp");
        let archive = dir.path().join("bad.tar.gz");
        pack_malicious_parent_tar(&archive);
        let dest = dir.path().join("out");
        let err = extract_payload(&archive, &dest).unwrap_err();
        assert!(err.contains("非法"), "{err}");
        assert!(!dest.exists());
    }

    #[test]
    fn extracts_binary_and_directml_but_not_readme() {
        let dir = tempfile::tempdir().expect("temp");
        let archive = dir.path().join("aniorg.zip");
        pack_zip(
            &archive,
            &[
                ("README.md", b"notes"),
                ("aniorg.exe", b"MZ\x90\x00aniorg"),
                ("DirectML.dll", b"MZ\x90\x00dll"),
                ("other/skip.dll", b"MZ\x90\x00nope"),
            ],
        );
        let dest = dir.path().join("staged").join("aniorg.next");
        extract_payload(&archive, &dest).unwrap();
        assert!(fs::read(&dest).unwrap().starts_with(b"MZ"));
        assert_eq!(
            fs::read(dir.path().join("staged").join("DirectML.dll")).unwrap(),
            b"MZ\x90\x00dll"
        );
        assert!(!dir.path().join("staged").join("README.md").exists());
        assert!(!dir.path().join("staged").join("skip.dll").exists());
    }

    #[tokio::test]
    async fn downloads_archive_checks_hash_and_extracts_binary() {
        let dir = tempfile::tempdir().expect("temp");
        let target = current_release_target().expect("target");
        let payload = b"\x7FELFaniorg-test";
        let archive_name = format!("aniorg-0.2.0-{target}.tar.gz");
        let archive_path = dir.path().join(&archive_name);
        pack_tar(
            &archive_path,
            &[
                ("README.md", b"notes"),
                ("aniorg", payload),
                ("DirectML.dll", b"MZ\x90\x00dll"),
            ],
        );
        let archive_bytes = fs::read(&archive_path).unwrap();
        let digest = format!("{:x}", Sha256::digest(&archive_bytes));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let checksum_name = format!("{archive_name}.sha256");
        let body = archive_bytes.clone();
        let checksum = format!("{digest}  {archive_name}\n");
        let json = serde_json::json!({
            "tag_name": "v0.2.0",
            "commit": "0123456789abcdef0123456789abcdef01234567",
            "assets": [
                {
                    "name": archive_name,
                    "browser_download_url": format!("http://{addr}/{archive_name}"),
                    "size": body.len(),
                },
                {
                    "name": checksum_name,
                    "browser_download_url": format!("http://{addr}/{checksum_name}"),
                    "size": checksum.len(),
                }
            ]
        })
        .to_string();
        tokio::spawn(async move {
            serve_release(
                listener,
                &json,
                &archive_name,
                &body,
                &checksum_name,
                &checksum,
            )
            .await;
        });

        let clients = build_clients(Some("off")).unwrap();
        let install = dir.path().join("aniorg");
        fs::write(&install, b"old").unwrap();
        let outcome = stage_release(
            &clients.version,
            StageRequest {
                manual_client: &clients.download,
                feed: &format!("http://{addr}/latest"),
                install_exe: &install,
                work_dir: &dir.path().join("updates"),
                cancel: &CancellationToken::new(),
                events: None,
            },
            "0.1.0",
            None,
            target,
        )
        .await
        .expect("stage");
        match outcome {
            StageOutcome::Staged { path, version } => {
                assert_eq!(version, "0.2.0");
                assert_eq!(fs::read(&path).unwrap(), payload);
                assert_eq!(
                    fs::read(dir.path().join("DirectML.dll.new")).unwrap(),
                    b"MZ\x90\x00dll"
                );
            }
            StageOutcome::UpToDate { message } => panic!("应下载：{message}"),
        }
        assert!(!dir.path().join("updates").join("README.md").exists());
    }

    #[tokio::test]
    async fn checksum_mismatch_leaves_no_staged_binary() {
        let dir = tempfile::tempdir().expect("temp");
        let target = current_release_target().expect("target");
        let archive_name = format!("aniorg-0.2.0-{target}.tar.gz");
        let archive_path = dir.path().join(&archive_name);
        pack_tar(&archive_path, &[("aniorg", b"\x7FELFbad")]);
        let body = fs::read(&archive_path).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        let checksum_name = format!("{archive_name}.sha256");
        let checksum = format!("{}\n", "ab".repeat(32));
        let json = serde_json::json!({
            "tag_name": "v0.2.0",
            "assets": [
                {"name": archive_name, "browser_download_url": format!("http://{addr}/{archive_name}")},
                {"name": checksum_name, "browser_download_url": format!("http://{addr}/{checksum_name}")}
            ]
        })
        .to_string();
        let archive_bytes = body.clone();
        tokio::spawn(async move {
            serve_release(
                listener,
                &json,
                &archive_name,
                &archive_bytes,
                &checksum_name,
                &checksum,
            )
            .await;
        });
        let clients = build_clients(Some("off")).unwrap();
        let install = dir.path().join("aniorg");
        fs::write(&install, b"old").unwrap();
        let err = stage_release(
            &clients.version,
            StageRequest {
                manual_client: &clients.download,
                feed: &format!("http://{addr}/latest"),
                install_exe: &install,
                work_dir: &dir.path().join("updates"),
                cancel: &CancellationToken::new(),
                events: None,
            },
            "0.1.0",
            None,
            target,
        )
        .await
        .unwrap_err();
        assert!(err.contains("SHA256"), "{err}");
        assert!(!dir.path().join("aniorg.new").exists());
    }

    #[tokio::test]
    async fn configured_proxy_receives_the_request() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut buf = vec![0u8; 2048];
            let n = tokio::io::AsyncReadExt::read(&mut socket, &mut buf)
                .await
                .expect("read");
            let text = String::from_utf8_lossy(&buf[..n]);
            assert!(
                text.starts_with("GET http://example.test/ping"),
                "请求没有走代理：{text}"
            );
            let body = "proxied";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes())
                .await
                .expect("write");
        });

        let clients = build_clients(Some(&format!("http://{addr}"))).expect("client");
        let response = clients
            .download
            .get("http://example.test/ping")
            .send()
            .await
            .expect("send");
        let body = response.text().await.expect("body");
        assert_eq!(body, "proxied");
    }

    fn pack_malicious_parent_tar(path: &Path) {
        let name = b"../aniorg";
        let data = b"\x7FELFbad";
        let mut header = [0u8; 512];
        header[..name.len()].copy_from_slice(name);
        header[100..108].copy_from_slice(b"0000755\0");
        header[108..116].copy_from_slice(b"0000000\0");
        header[116..124].copy_from_slice(b"0000000\0");
        let size = format!("{:011o}\0", data.len());
        header[124..136].copy_from_slice(size.as_bytes());
        header[136..148].copy_from_slice(b"00000000000\0");
        header[148..156].copy_from_slice(b"        ");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        let sum: u32 = header.iter().map(|byte| u32::from(*byte)).sum();
        let checksum = format!("{sum:06o}\0 ");
        header[148..156].copy_from_slice(checksum.as_bytes());
        let mut raw = Vec::new();
        raw.extend_from_slice(&header);
        raw.extend_from_slice(data);
        let pad = (512 - (data.len() % 512)) % 512;
        raw.extend(std::iter::repeat_n(0, pad));
        raw.extend(std::iter::repeat_n(0, 1024));
        let file = File::create(path).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap();
    }

    fn pack_tar(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);
        for (name, data) in files {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append(&header, *data).unwrap();
        }
        builder.finish().unwrap();
    }

    fn pack_zip(path: &Path, files: &[(&str, &[u8])]) {
        let file = File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in files {
            zip.start_file(name, options).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    async fn serve_release(
        listener: tokio::net::TcpListener,
        json: &str,
        archive_name: &str,
        archive: &[u8],
        checksum_name: &str,
        checksum: &str,
    ) {
        let routes = [
            ("/latest", json.as_bytes().to_vec(), "application/json"),
            (
                &format!("/{archive_name}"),
                archive.to_vec(),
                "application/gzip",
            ),
            (
                &format!("/{checksum_name}"),
                checksum.as_bytes().to_vec(),
                "text/plain",
            ),
        ];
        let routes: Vec<(String, Vec<u8>, &'static str)> = routes
            .into_iter()
            .map(|(path, body, kind)| (path.to_string(), body, kind))
            .collect();
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let mut buf = vec![0u8; 4096];
            let Ok(n) = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await else {
                continue;
            };
            let text = String::from_utf8_lossy(&buf[..n]);
            let path = text.split_whitespace().nth(1).unwrap_or("");
            let found = routes.iter().find(|(route, _, _)| route == path);
            let (status, kind, body) = if let Some((_, body, kind)) = found {
                ("200 OK", *kind, body.clone())
            } else {
                ("404 Not Found", "text/plain", b"missing".to_vec())
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response.as_bytes()).await;
            let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, &body).await;
        }
    }
}
