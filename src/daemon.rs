use anime_organizer::error::AppError;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Instant;
#[cfg(feature = "metadata")]
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) mod api;
#[cfg(feature = "self-update")]
pub(crate) mod app_update;
#[cfg(feature = "clouddrive")]
pub(crate) mod cloud;
pub(crate) mod model;
pub(crate) mod queue;
#[cfg(feature = "clouddrive")]
pub(crate) mod remote_organize;
#[cfg(feature = "clouddrive")]
pub(crate) mod rss_schedule;
pub(crate) mod web;
pub(crate) mod worker;

#[cfg(feature = "metadata")]
const ANIMEATLAS_REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

use queue::QueueRepository;
use worker::WorkerSnapshot;

pub(crate) struct DaemonState {
    pub(crate) queue: QueueRepository,
    #[cfg(feature = "clouddrive")]
    pub(crate) cloud: cloud::CloudDriveState,
    #[cfg(feature = "clouddrive")]
    pub(crate) rss_db_path: PathBuf,
    pub(crate) started_at: Instant,
    pub(crate) wake: mpsc::Sender<()>,
    pub(crate) worker: Arc<Mutex<WorkerSnapshot>>,
    pub(crate) cancel_requested: Arc<AtomicBool>,
    #[cfg(feature = "self-update")]
    pub(crate) app_update: Arc<Mutex<app_update::AppUpdateStatus>>,
    #[cfg(feature = "self-update")]
    pub(crate) app_job_running: Arc<AtomicBool>,
    #[cfg(feature = "self-update")]
    pub(crate) app_update_retry_at: Arc<Mutex<Option<Instant>>>,
}

pub(crate) fn run() -> Result<(), AppError> {
    let queue = QueueRepository::new(&daemon_db_path())
        .map_err(|error| AppError::MetadataFetchError(error.to_string()))?;
    queue
        .recover_running()
        .map_err(|error| AppError::MetadataFetchError(error.to_string()))?;
    #[cfg(feature = "clouddrive")]
    let cloud = cloud::CloudDriveState::new(&daemon_db_path())
        .map_err(|error| AppError::MetadataFetchError(error.to_string()))?;

    let (wake_tx, wake_rx) = mpsc::channel();
    let worker = Arc::new(Mutex::new(WorkerSnapshot::default()));
    let shutting_down = Arc::new(AtomicBool::new(false));
    let cancel_requested = Arc::new(AtomicBool::new(false));
    #[cfg(feature = "clouddrive")]
    let rss_db_path = anime_organizer::rss::db::default_db_path();
    let state = Arc::new(DaemonState {
        queue: queue.clone(),
        #[cfg(feature = "clouddrive")]
        cloud,
        #[cfg(feature = "clouddrive")]
        rss_db_path: rss_db_path.clone(),
        started_at: Instant::now(),
        wake: wake_tx.clone(),
        worker: worker.clone(),
        cancel_requested: cancel_requested.clone(),
        #[cfg(feature = "self-update")]
        app_update: Arc::new(Mutex::new(app_update::initial_status())),
        #[cfg(feature = "self-update")]
        app_job_running: Arc::new(AtomicBool::new(false)),
        #[cfg(feature = "self-update")]
        app_update_retry_at: Arc::new(Mutex::new(None)),
    });
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|error| AppError::MetadataFetchError(format!("创建异步运行时失败: {error}")))?;
    let worker_handle = worker::start(
        queue,
        wake_rx,
        worker,
        shutting_down.clone(),
        cancel_requested,
        #[cfg(feature = "clouddrive")]
        rss_schedule::RssRuntime {
            cloud: state.cloud.clone(),
            rss_db_path: rss_db_path.clone(),
        },
    );
    let result = runtime.block_on(run_http(state));
    shutting_down.store(true, Ordering::Release);
    let _ = wake_tx.send(());
    worker_handle.join();
    result
}

async fn run_http(state: Arc<DaemonState>) -> Result<(), AppError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:32145")
        .await
        .map_err(|error| AppError::MetadataFetchError(format!("绑定 daemon 地址失败: {error}")))?;
    println!("anime-organizer daemon listening at http://127.0.0.1:32145/");

    #[cfg(feature = "self-update")]
    app_update::start(state.clone());
    #[cfg(feature = "metadata")]
    start_animeatlas_refresh_scheduler();
    #[cfg(feature = "metadata")]
    start_bangumi_dump_refresh_scheduler(state.queue.clone(), state.wake.clone());

    #[cfg(feature = "clouddrive")]
    rss_schedule::start_scheduler(
        state.queue.clone(),
        state.rss_db_path.clone(),
        state.wake.clone(),
    );

    let server = axum::serve(listener, api::router(state));
    server
        .with_graceful_shutdown(shutdown_signal())
        .await
        .map_err(|error| AppError::MetadataFetchError(format!("daemon HTTP 服务失败: {error}")))
}

#[cfg(feature = "metadata")]
fn start_animeatlas_refresh_scheduler() {
    drop(tokio::spawn(async move {
        let mut interval = tokio::time::interval(ANIMEATLAS_REFRESH_CHECK_INTERVAL);
        loop {
            interval.tick().await;
            let bangumi = anime_organizer::metadata::BangumiClient::new(None);
            if let Err(error) =
                crate::download_animeatlas_alias_db(bangumi.cache_dir(), false).await
            {
                eprintln!("AnimeAtlas 别名库定期刷新失败，将继续使用现有缓存: {error}");
            }
        }
    }));
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(feature = "metadata")]
fn start_bangumi_dump_refresh_scheduler(queue: QueueRepository, wake: mpsc::Sender<()>) {
    drop(tokio::spawn(async move {
        // 启动先看一眼：补上停机期间错过的版本。
        check_bangumi_release(&queue, &wake).await;
        // Bangumi 固定每周二 21:03 UTC 发版：算好下一次的点睡到那，平时零轮询。
        let mut target = next_bangumi_release(SystemTime::now());
        loop {
            // 蹲点之后新版还没出来（发版延迟/网络失败）：改成每小时看一眼。
            let wait = target
                .duration_since(SystemTime::now())
                .unwrap_or(Duration::ZERO);
            tokio::time::sleep(if wait.is_zero() {
                BANGUMI_RELEASE_RETRY
            } else {
                wait
            })
            .await;
            // 只有本地跟上了远端，才把目标推到下周二；否则保持每小时重试。
            if check_bangumi_release(&queue, &wake).await.is_some() {
                target = next_bangumi_release(SystemTime::now());
            }
        }
    }));
}

/// Bangumi Archive 发版日：周二（周一=0）。
#[cfg(feature = "metadata")]
const BANGUMI_RELEASE_WDAY_MON0: u64 = 1;
/// 发版时刻 21:03 UTC + 10 分钟余量（等 asset 就绪）。
#[cfg(feature = "metadata")]
const BANGUMI_RELEASE_TOD_SECS: u64 = 21 * 3600 + 14 * 60;
/// 蹲点扑空后的重试间隔。
#[cfg(feature = "metadata")]
const BANGUMI_RELEASE_RETRY: Duration = Duration::from_secs(60 * 60);

/// 返回 `now` 之后最近的一个发版时刻（严格大于 `now`）。
#[cfg(feature = "metadata")]
fn next_bangumi_release(now: SystemTime) -> SystemTime {
    let secs = now.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let days = secs / 86400;
    let tod = secs % 86400;
    // 1970-01-01 是周四，换算成周一=0 的星期。
    let weekday_mon0 = (days + 3) % 7;
    let mut ahead = (BANGUMI_RELEASE_WDAY_MON0 + 7 - weekday_mon0) % 7;
    if ahead == 0 && tod >= BANGUMI_RELEASE_TOD_SECS {
        ahead = 7;
    }
    UNIX_EPOCH + Duration::from_secs((days + ahead) * 86400 + BANGUMI_RELEASE_TOD_SECS)
}

/// 拉一次 500 字节的 latest.json：有新版就入队（同版本幂等，失败自动重试）。
/// 本地已跟上远端时返回 `Some(版本名)`，其他情况返回 `None`。
#[cfg(feature = "metadata")]
async fn check_bangumi_release(queue: &QueueRepository, wake: &mpsc::Sender<()>) -> Option<String> {
    use crate::daemon::model::{EnqueueRequest, JobOrigin, JobSpec};

    let client = anime_organizer::metadata::BangumiClient::new(None);
    let latest = client.fetch_latest().await.ok()?;
    if latest.name.is_empty() {
        return None;
    }
    if client.dump_version().as_deref() != Some(latest.name.as_str()) {
        let idempotency_key = format!("bangumi-dump:{}", latest.name);
        if !refresh_job_for(queue, &idempotency_key) {
            let request = EnqueueRequest {
                idempotency_key: Some(idempotency_key),
                origin: JobOrigin::Scheduled,
                confirmed: false,
                job: JobSpec::RefreshBangumiDump,
            };
            if let Ok(outcome) = queue.enqueue(&request) {
                if !outcome.duplicate {
                    let _ = wake.send(());
                }
            }
        }
        return None;
    }
    Some(latest.name)
}

/// 同版本已有 queued/running/succeeded 任务则跳过；failed/canceled 则重试后跳过。
/// 返回 `true` 表示本轮无需入队。
#[cfg(feature = "metadata")]
fn refresh_job_for(queue: &QueueRepository, idempotency_key: &str) -> bool {
    let Ok(jobs) = queue.list(None, Some("refresh_bangumi_dump"), 10, None) else {
        return false;
    };
    let Some(job) = jobs
        .iter()
        .find(|job| job.idempotency_key.as_deref() == Some(idempotency_key))
    else {
        return false;
    };
    use crate::daemon::model::JobState;
    match job.state {
        JobState::Failed | JobState::Canceled => queue.retry(job.id).is_ok(),
        _ => true,
    }
}

pub(crate) fn daemon_db_path() -> PathBuf {
    crate::paths::data_dir().join("daemon.db")
}

#[cfg(all(test, feature = "metadata"))]
mod tests {
    use super::*;

    const TUE_00_00: u64 = 1791849600; // 2026-10-13T00:00:00Z，周二
    const TUE_21_14: u64 = TUE_00_00 + 21 * 3600 + 14 * 60;

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    fn secs(t: SystemTime) -> u64 {
        t.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    #[test]
    fn next_release_is_same_tuesday_before_cutoff() {
        // 周一 00:00 -> 本周二 21:14；周二 21:13:59 -> 当天 21:14。
        assert_eq!(secs(next_bangumi_release(at(TUE_00_00 - 86400))), TUE_21_14);
        assert_eq!(secs(next_bangumi_release(at(TUE_21_14 - 1))), TUE_21_14);
    }

    #[test]
    fn next_release_rolls_to_next_week_after_cutoff() {
        // 21:14 整点已过 -> 下周二；周三 -> 下周二。
        assert_eq!(
            secs(next_bangumi_release(at(TUE_21_14))),
            TUE_21_14 + 7 * 86400
        );
        assert_eq!(
            secs(next_bangumi_release(at(TUE_00_00 + 86400))),
            TUE_21_14 + 7 * 86400
        );
    }
}
