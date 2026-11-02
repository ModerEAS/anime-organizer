//! Daemon 里的程序自身更新循环。

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::DaemonState;
use crate::self_update::{
    self, build_clients, current_commit, current_restart_args, current_version, proxy_display_for,
    running_from_cargo_target, stage_planned, StageEvent, StageRequest, UpdateDecision,
    UpdateSettings,
};

const APP_UPDATE_GRACE: Duration = Duration::from_secs(3);
const APP_UPDATE_MIN: Duration = Duration::from_secs(5 * 60);
const BUSY_RETRY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplyMode {
    Auto,
    Manual,
    CheckOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AppUpdatePhase {
    Idle,
    Checking,
    Downloading,
    Verifying,
    Applying,
    UpToDate,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AppUpdateStatus {
    pub(crate) current_version: String,
    pub(crate) current_commit: Option<String>,
    pub(crate) latest_version: Option<String>,
    pub(crate) latest_commit: Option<String>,
    pub(crate) phase: AppUpdatePhase,
    pub(crate) percent: f64,
    pub(crate) bytes: u64,
    pub(crate) total: Option<u64>,
    pub(crate) message: String,
    pub(crate) error: Option<String>,
    pub(crate) update_available: bool,
    pub(crate) next_check_unix_ms: Option<u64>,
    pub(crate) interval: String,
    pub(crate) auto_apply: bool,
    pub(crate) feed: String,
    pub(crate) proxy: String,
    pub(crate) proxy_display: String,
    pub(crate) cargo_target: bool,
}

impl AppUpdateStatus {
    pub(crate) fn starting() -> Self {
        Self {
            current_version: current_version().to_string(),
            current_commit: current_commit().map(str::to_string),
            latest_version: None,
            latest_commit: None,
            phase: AppUpdatePhase::Idle,
            percent: 0.0,
            bytes: 0,
            total: None,
            message: "还没检查过程序更新。".to_string(),
            error: None,
            update_available: false,
            next_check_unix_ms: None,
            interval: "6h".to_string(),
            auto_apply: true,
            feed: String::new(),
            proxy: String::new(),
            proxy_display: "直连".to_string(),
            cargo_target: running_from_cargo_target(),
        }
    }
}

pub(crate) fn initial_status() -> AppUpdateStatus {
    let mut status = AppUpdateStatus::starting();
    match UpdateSettings::load(&UpdateSettings::path()) {
        Ok(settings) => apply_settings(&mut status, &settings),
        Err(err) => {
            status.phase = AppUpdatePhase::Failed;
            status.error = Some(err.clone());
            status.message = err;
        }
    }
    status
}

pub(crate) fn start(state: Arc<DaemonState>) {
    tokio::spawn(app_update_loop(state));
}

pub(crate) fn snapshot(state: &DaemonState) -> AppUpdateStatus {
    lock_status(state).clone()
}

pub(crate) fn sync_settings(state: &DaemonState, settings: &UpdateSettings) {
    apply_settings(&mut lock_status(state), settings);
}

pub(crate) fn begin(state: &Arc<DaemonState>, mode: ApplyMode) -> Result<(), String> {
    if mode != ApplyMode::CheckOnly && worker_busy(state) {
        return Err("有整理任务正在运行，完成后再更新。".into());
    }
    if state
        .app_job_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("程序更新已经在进行".into());
    }
    let state = Arc::clone(state);
    tokio::spawn(async move {
        let _guard = JobGuard {
            state: Arc::clone(&state),
        };
        if let Err(err) = run_app_update(&state, mode).await {
            eprintln!("程序更新失败: {err}");
            note(&state, AppUpdatePhase::Failed, err.clone(), Some(err), None);
        }
    });
    Ok(())
}

async fn app_update_loop(state: Arc<DaemonState>) {
    let mut armed: Option<Duration> = None;
    let mut next_run: Option<Instant> = None;
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if state.app_job_running.load(Ordering::SeqCst) {
            continue;
        }
        honor_retry(&state, &mut next_run);
        let settings = match UpdateSettings::load(&UpdateSettings::path()) {
            Ok(settings) => settings,
            Err(err) => {
                note(&state, AppUpdatePhase::Failed, err.clone(), Some(err), None);
                armed = None;
                next_run = None;
                continue;
            }
        };
        sync_settings(&state, &settings);
        let interval = match self::parse_saved_interval(&settings.app_update_interval) {
            Ok(interval) => interval,
            Err(err) => {
                note(
                    &state,
                    AppUpdatePhase::Failed,
                    format!("程序更新间隔无效：{err}"),
                    Some(err),
                    None,
                );
                armed = None;
                next_run = None;
                continue;
            }
        };
        let Some(mut interval) = interval else {
            armed = None;
            next_run = None;
            note(
                &state,
                AppUpdatePhase::Idle,
                "程序自动更新已关闭。",
                None,
                None,
            );
            continue;
        };
        if interval < APP_UPDATE_MIN {
            interval = APP_UPDATE_MIN;
        }
        if armed != Some(interval) {
            let first = armed.is_none();
            armed = Some(interval);
            next_run = Some(Instant::now() + if first { APP_UPDATE_GRACE } else { interval });
            if first && lock_status(&state).phase == AppUpdatePhase::Idle {
                set_message(&state, "程序会检查自己的发布版本。有新构建时会下载并重启。");
            }
        }
        set_next(&state, next_run);
        if Instant::now() < next_run.unwrap_or_else(Instant::now) {
            continue;
        }
        if worker_busy(&state) || state.app_job_running.load(Ordering::SeqCst) {
            next_run = Some(Instant::now() + BUSY_RETRY);
            set_message(&state, "有整理任务正在运行，程序更新等它完成。");
            set_next(&state, next_run);
            continue;
        }
        next_run = Some(Instant::now() + interval);
        set_next(&state, next_run);
        if let Err(err) = begin(&state, ApplyMode::Auto) {
            note(
                &state,
                AppUpdatePhase::Failed,
                err.clone(),
                Some(err),
                next_run,
            );
        }
    }
}

fn parse_saved_interval(raw: &str) -> Result<Option<Duration>, String> {
    crate::self_update::parse_interval(raw)
}

async fn run_app_update(state: &Arc<DaemonState>, mode: ApplyMode) -> Result<(), String> {
    let settings = UpdateSettings::load(&UpdateSettings::path())?;
    sync_settings(state, &settings);
    let cancel = CancellationToken::new();
    note(
        state,
        AppUpdatePhase::Checking,
        "正在检查程序更新。",
        None,
        None,
    );
    let clients = build_clients(settings.proxy.as_deref())?;
    let feed = settings.feed_url();
    let remote = self_update::inspect_release(&clients.version, &feed, &cancel).await?;
    {
        let mut status = lock_status(state);
        status.latest_version = Some(remote.version.clone());
        status.latest_commit = remote.commit.clone();
    }
    let target = self_update::current_release_target()?;
    let decision =
        self_update::decide_update(current_version(), current_commit(), target, &remote)?;
    let plan = match decision {
        UpdateDecision::Keep { message } => {
            note(state, AppUpdatePhase::UpToDate, message, None, None);
            let mut status = lock_status(state);
            status.latest_version = Some(remote.version);
            status.latest_commit = remote.commit;
            status.update_available = false;
            status.percent = 100.0;
            return Ok(());
        }
        UpdateDecision::Update(plan) => plan,
    };
    {
        let mut status = lock_status(state);
        status.latest_version = Some(plan.version.clone());
        status.latest_commit = plan.commit.clone();
        status.update_available = true;
        status.phase = AppUpdatePhase::UpToDate;
        status.error = None;
    }
    if mode == ApplyMode::CheckOnly {
        set_message(state, format!("发现程序 {}，还没有安装。", plan.version));
        return Ok(());
    }
    if mode == ApplyMode::Auto && !settings.auto_apply_app_update {
        set_message(
            state,
            format!("发现程序 {}。自动替换是关的，所以没有安装。", plan.version),
        );
        return Ok(());
    }
    if mode == ApplyMode::Auto && running_from_cargo_target() {
        set_message(
            state,
            format!(
                "发现程序 {}。当前是开发构建，不会自动替换。安装版会自己更新。",
                plan.version
            ),
        );
        return Ok(());
    }
    if worker_busy(state) {
        set_message(state, "发现新的程序版本。整理任务还在进行，完成后再替换。");
        schedule_retry(state);
        return Ok(());
    }
    note(
        state,
        AppUpdatePhase::Downloading,
        format!("正在下载程序 {}。", plan.version),
        None,
        None,
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let progress_state = Arc::clone(state);
    let progress = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            apply_stage_event(&progress_state, event);
        }
    });
    let install_exe = std::env::current_exe().map_err(|err| format!("找不到当前程序：{err}"))?;
    let staged = stage_planned(
        &StageRequest {
            manual_client: &clients.download,
            feed: &feed,
            install_exe: &install_exe,
            work_dir: &crate::paths::data_dir().join("updates"),
            cancel: &cancel,
            events: Some(&tx),
        },
        &plan,
    )
    .await;
    drop(tx);
    let _ = progress.await;
    let staged = staged?;
    if worker_busy(state) {
        set_message(state, "程序包已经下好。整理任务还在进行，完成后再重启。");
        schedule_retry(state);
        return Ok(());
    }
    note(
        state,
        AppUpdatePhase::Applying,
        format!("正在换成程序 {} 并重启。", plan.version),
        None,
        None,
    );
    self_update::relaunch_and_exit(&staged, &current_restart_args())
}

fn apply_stage_event(state: &DaemonState, event: StageEvent) {
    let mut status = lock_status(state);
    match event {
        StageEvent::Downloading { bytes, total } => {
            status.phase = AppUpdatePhase::Downloading;
            status.bytes = bytes;
            status.total = total;
            status.percent = match total {
                Some(total) if total > 0 => (bytes as f64 / total as f64 * 100.0).clamp(0.0, 100.0),
                _ => 0.0,
            };
            status.message = match total {
                Some(total) => format!("正在下载程序 {bytes} / {total} 字节"),
                None => format!("正在下载程序 {bytes} 字节"),
            };
            status.error = None;
        }
        StageEvent::Verifying => {
            status.phase = AppUpdatePhase::Verifying;
            status.message = "正在核对程序包。".to_string();
        }
        StageEvent::Extracting => {
            status.phase = AppUpdatePhase::Verifying;
            status.message = "正在解出新程序。".to_string();
        }
    }
}

fn worker_busy(state: &DaemonState) -> bool {
    state
        .worker
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .current_job_id
        .is_some()
}

fn apply_settings(status: &mut AppUpdateStatus, settings: &UpdateSettings) {
    status.interval = settings.app_update_interval.clone();
    status.auto_apply = settings.auto_apply_app_update;
    status.feed = settings.app_update_feed.clone().unwrap_or_default();
    status.proxy = settings.proxy.clone().unwrap_or_default();
    status.proxy_display = proxy_display_for(settings.proxy.as_deref());
    status.cargo_target = running_from_cargo_target();
}

fn note(
    state: &DaemonState,
    phase: AppUpdatePhase,
    message: impl Into<String>,
    error: Option<String>,
    next_run: Option<Instant>,
) {
    let mut status = lock_status(state);
    status.phase = phase;
    status.message = message.into();
    status.error = error;
    if let Some(next_run) = next_run {
        status.next_check_unix_ms = Some(instant_to_unix_ms(next_run));
    }
}

fn set_message(state: &DaemonState, message: impl Into<String>) {
    let mut status = lock_status(state);
    status.message = message.into();
    status.error = None;
}

fn set_next(state: &DaemonState, next_run: Option<Instant>) {
    lock_status(state).next_check_unix_ms = next_run.map(instant_to_unix_ms);
}

fn schedule_retry(state: &DaemonState) {
    *lock_retry(state) = Some(Instant::now() + BUSY_RETRY);
}

fn honor_retry(state: &DaemonState, next_run: &mut Option<Instant>) {
    let mut retry_at = lock_retry(state);
    let Some(at) = *retry_at else {
        return;
    };
    if Instant::now() >= at {
        *retry_at = None;
        return;
    }
    if next_run.map(|next| at < next).unwrap_or(true) {
        *next_run = Some(at);
    }
}

fn lock_status(state: &DaemonState) -> std::sync::MutexGuard<'_, AppUpdateStatus> {
    state
        .app_update
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn lock_retry(state: &DaemonState) -> std::sync::MutexGuard<'_, Option<Instant>> {
    state
        .app_update_retry_at
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

fn instant_to_unix_ms(target: Instant) -> u64 {
    let now_instant = Instant::now();
    let now_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let millis = if target > now_instant {
        now_unix
            .saturating_add(target.saturating_duration_since(now_instant))
            .as_millis()
    } else {
        now_unix
            .saturating_sub(now_instant.saturating_duration_since(target))
            .as_millis()
    };
    u64::try_from(millis).unwrap_or(u64::MAX)
}

struct JobGuard {
    state: Arc<DaemonState>,
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        self.state.app_job_running.store(false, Ordering::SeqCst);
    }
}
