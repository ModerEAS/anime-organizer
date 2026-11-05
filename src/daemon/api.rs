#[cfg(feature = "clouddrive")]
use super::cloud::{
    CloudConnectionRequest, CloudConnectionView, CloudError, CloudFolderEntry,
    CLOUD_OPERATION_TIMEOUT_SECS, MAX_FOLDER_ENTRIES, MAX_FOLDER_PATH_BYTES,
};
use super::model::{EnqueueRequest, JobState, JobView};
#[cfg(feature = "clouddrive")]
use super::model::{JobOrigin, JobSpec};
use super::queue::{JobLogRecord, QueueError};
use super::{web, DaemonState};
#[cfg(feature = "clouddrive")]
use anime_organizer::rss::db::RssDatabase;
use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Json, Path, Query, State};
use axum::http::{header, HeaderValue, Request, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::Response;
#[cfg(any(feature = "clouddrive", feature = "self-update"))]
use axum::routing::put;
use axum::routing::{delete, get, post};
use axum::{response::IntoResponse, Router};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::Arc;
#[cfg(feature = "clouddrive")]
use std::time::Duration;

pub(crate) fn router(state: Arc<DaemonState>) -> Router {
    #[allow(unused_mut)]
    let mut router = Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/status", get(status))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/jobs", post(enqueue).get(list_jobs))
        .route("/api/v1/jobs/:id", get(get_job).delete(cancel_job))
        .route("/api/v1/jobs/:id/logs", get(get_job_logs))
        .route("/api/v1/jobs/:id/retry", post(retry_job))
        .route("/api/v1/jobs/:id/history", delete(delete_history))
        .route(
            "/api/v1/jobs/:id/artifacts/:artifact_id",
            get(download_artifact),
        );
    #[cfg(feature = "metadata")]
    {
        router = router.route(
            "/api/v1/metadata/settings",
            get(get_metadata_settings).put(update_metadata_settings),
        );
    }
    #[cfg(feature = "self-update")]
    {
        router = router
            .route(
                "/api/v1/app-update/settings",
                put(update_app_update_settings),
            )
            .route("/api/v1/app-update/check", post(check_app_update))
            .route("/api/v1/app-update/apply", post(apply_app_update));
    }
    #[cfg(feature = "clouddrive")]
    {
        router = router
            .route(
                "/api/v1/cloud/connections",
                get(list_cloud_connections).post(create_cloud_connection),
            )
            .route(
                "/api/v1/cloud/connections/:id",
                put(update_cloud_connection)
                    .patch(patch_cloud_connection)
                    .delete(delete_cloud_connection),
            )
            .route(
                "/api/v1/cloud/connections/:id/test",
                post(test_cloud_connection),
            )
            .route(
                "/api/v1/cloud/connections/:id/list-folder",
                post(list_cloud_folder),
            )
            .route(
                "/api/v1/rss/subscriptions",
                get(list_rss_subscriptions).post(create_rss_subscription),
            )
            .route(
                "/api/v1/rss/subscriptions/:id",
                put(update_rss_subscription).delete(delete_rss_subscription),
            )
            .route(
                "/api/v1/rss/subscriptions/:id/enable",
                post(enable_rss_subscription),
            )
            .route(
                "/api/v1/rss/subscriptions/:id/disable",
                post(disable_rss_subscription),
            )
            .route(
                "/api/v1/rss/subscriptions/:id/run",
                post(run_rss_subscription),
            )
            .route("/api/v1/rss/run", post(run_all_rss_subscriptions))
            .route("/api/v1/rss/processed", get(list_processed_items))
            .route("/api/v1/rss/download-tasks", get(list_download_tasks));
    }
    router
        .fallback(not_found_or_index)
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(restrict_local_requests))
        .with_state(state)
}

async fn restrict_local_requests(request: Request<Body>, next: Next) -> Response {
    let host_allowed = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(local_host_allowed);
    let origin_allowed = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_none_or(local_origin_allowed);
    if !host_allowed || !origin_allowed {
        return error(
            StatusCode::FORBIDDEN,
            "forbidden_origin",
            "daemon API is available only from the local WebUI",
        );
    }
    next.run(request).await
}

fn local_host_allowed(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "127.0.0.1:32145" | "localhost:32145" | "[::1]:32145"
    )
}

fn local_origin_allowed(value: &str) -> bool {
    url::Url::parse(value).ok().is_some_and(|url| {
        url.scheme() == "http"
            && url.username().is_empty()
            && url.password().is_none()
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"))
    })
}

async fn not_found_or_index(uri: Uri) -> Response {
    if uri.path().starts_with("/api/v1/") || uri.path() == "/api/v1" {
        error(StatusCode::NOT_FOUND, "not_found", "API endpoint not found")
    } else {
        web::index().await.into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ApiError,
}

#[derive(Debug, Serialize)]
struct ApiError {
    code: &'static str,
    message: String,
}

fn error(status: StatusCode, code: &'static str, message: impl Into<String>) -> Response {
    (
        status,
        Json(ErrorEnvelope {
            error: ApiError {
                code,
                message: message.into(),
            },
        }),
    )
        .into_response()
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
    })
}

#[derive(Debug, Serialize)]
struct StatusResponse {
    uptime_seconds: u64,
    worker_state: String,
    current_job_id: Option<i64>,
    queue_counts: QueueCounts,
    database_path: String,
    #[cfg(feature = "self-update")]
    app_update: crate::daemon::app_update::AppUpdateStatus,
}

#[derive(Debug, Serialize)]
struct QueueCounts {
    queued: i64,
    running: i64,
    succeeded: i64,
    failed: i64,
    canceled: i64,
}

async fn status(State(state): State<Arc<DaemonState>>) -> Response {
    let counts = match state.queue.counts() {
        Ok(counts) => counts,
        Err(queue_error) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                queue_error.to_string(),
            )
        }
    };
    let worker = state.worker.lock().ok().map(|value| value.clone());
    let Some(worker) = worker else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "worker state unavailable",
        );
    };
    Json(StatusResponse {
        uptime_seconds: state.started_at.elapsed().as_secs(),
        worker_state: worker.state,
        current_job_id: worker.current_job_id,
        queue_counts: QueueCounts {
            queued: counts[0],
            running: counts[1],
            succeeded: counts[2],
            failed: counts[3],
            canceled: counts[4],
        },
        database_path: state.queue.path().display().to_string(),
        #[cfg(feature = "self-update")]
        app_update: crate::daemon::app_update::snapshot(&state),
    })
    .into_response()
}

/// 读取 `metadata.json`。没有文件时返回默认值（自动下载全量别名库）。
#[cfg(feature = "metadata")]
async fn get_metadata_settings() -> Response {
    let path = crate::metadata_settings::MetadataSettings::path();
    match crate::metadata_settings::MetadataSettings::load(&path) {
        Ok(settings) => Json(settings).into_response(),
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal", message),
    }
}

/// 保存 `metadata.json`，返回落盘后的实际内容。
#[cfg(feature = "metadata")]
async fn update_metadata_settings(
    Json(settings): Json<crate::metadata_settings::MetadataSettings>,
) -> Response {
    let path = crate::metadata_settings::MetadataSettings::path();
    if let Err(message) = crate::metadata_settings::MetadataSettings::save(&path, &settings) {
        return error(StatusCode::BAD_REQUEST, "invalid_request", message);
    }
    match crate::metadata_settings::MetadataSettings::load(&path) {
        Ok(settings) => Json(settings).into_response(),
        Err(message) => error(StatusCode::INTERNAL_SERVER_ERROR, "internal", message),
    }
}

#[cfg(feature = "self-update")]
async fn update_app_update_settings(
    State(state): State<Arc<DaemonState>>,
    Json(settings): Json<crate::self_update::UpdateSettings>,
) -> Response {
    let path = crate::self_update::UpdateSettings::path();
    if let Err(message) = crate::self_update::UpdateSettings::save(&path, &settings) {
        return error(StatusCode::BAD_REQUEST, "invalid_request", message);
    }
    let settings = match crate::self_update::UpdateSettings::load(&path) {
        Ok(settings) => settings,
        Err(message) => {
            return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", message);
        }
    };
    crate::daemon::app_update::sync_settings(&state, &settings);
    Json(crate::daemon::app_update::snapshot(&state)).into_response()
}

#[cfg(feature = "self-update")]
async fn check_app_update(State(state): State<Arc<DaemonState>>) -> Response {
    start_app_update(state, crate::daemon::app_update::ApplyMode::CheckOnly)
}

#[cfg(feature = "self-update")]
async fn apply_app_update(State(state): State<Arc<DaemonState>>) -> Response {
    start_app_update(state, crate::daemon::app_update::ApplyMode::Manual)
}

#[cfg(feature = "self-update")]
fn start_app_update(
    state: Arc<DaemonState>,
    mode: crate::daemon::app_update::ApplyMode,
) -> Response {
    match crate::daemon::app_update::begin(&state, mode) {
        Ok(()) => Json(serde_json::json!({ "ok": true })).into_response(),
        Err(message) => error(StatusCode::CONFLICT, "update_busy", message),
    }
}

#[derive(Debug, Serialize)]
struct CapabilitiesResponse {
    features: Vec<&'static str>,
    job_types: Vec<&'static str>,
    resources: Vec<&'static str>,
}

async fn capabilities() -> Json<CapabilitiesResponse> {
    #[allow(unused_mut)]
    let mut features = vec!["daemon", "organize"];
    #[allow(unused_mut)]
    let mut resources = Vec::new();
    #[cfg(feature = "metadata")]
    features.push("metadata");
    #[cfg(feature = "self-update")]
    features.push("self-update");
    #[cfg(feature = "anifilebert")]
    features.push("anifilebert");
    #[cfg(feature = "clouddrive")]
    {
        features.push("clouddrive");
        resources.extend(["cloud_connections", "rss_subscriptions"]);
    }
    #[allow(unused_mut)]
    let mut job_types = vec!["organize", "normalize_layout", "compact_artwork_packs"];
    #[cfg(feature = "metadata")]
    job_types.push("refresh_bangumi_dump");
    #[cfg(feature = "scraper")]
    {
        features.push("scraper");
        job_types.extend([
            "scrape",
            "match_aliases",
            "build_bangumi_db",
            "extract_aliases",
            "merge_aliases",
            "apply_matches",
        ]);
        if crate::commands::gh_available() {
            job_types.push("create_alias_issues");
        }
    }
    #[cfg(feature = "torrent-scraper")]
    {
        features.push("torrent-scraper");
        job_types.push("torrent_scrape");
    }
    #[cfg(feature = "clouddrive")]
    job_types.extend([
        "rss_poll",
        "rss_poll_all",
        "remote_rss_organize",
        "cleanup_empty_dirs",
        "storage_organize",
        "cloud_add_offline",
    ]);
    Json(CapabilitiesResponse {
        features,
        job_types,
        resources,
    })
}

#[derive(Debug, Deserialize)]
struct JobsQuery {
    state: Option<String>,
    kind: Option<String>,
    limit: Option<i64>,
    before_id: Option<i64>,
}

#[derive(Debug, Serialize)]
struct JobsResponse {
    jobs: Vec<JobView>,
}

async fn list_jobs(
    State(state): State<Arc<DaemonState>>,
    Query(query): Query<JobsQuery>,
) -> Response {
    let parsed_state = match query.state.as_deref() {
        Some(value) => match JobState::parse(value) {
            Some(state) => Some(state),
            None => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "invalid job state",
                )
            }
        },
        None => None,
    };
    let jobs = match state.queue.list(
        parsed_state,
        query.kind.as_deref(),
        query.limit.unwrap_or(50),
        query.before_id,
    ) {
        Ok(jobs) => jobs.into_iter().map(JobView::from).collect(),
        Err(error) => return queue_error(error),
    };
    Json(JobsResponse { jobs }).into_response()
}

#[derive(Debug, Serialize)]
struct EnqueueResponse {
    job: JobView,
    duplicate: bool,
}

async fn enqueue(
    State(state): State<Arc<DaemonState>>,
    request: Result<Json<EnqueueRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    if let Err(message) = request.job.validate(
        request.confirmed,
        request.origin,
        request.idempotency_key.as_deref(),
    ) {
        return error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", message);
    }
    let outcome = match state.queue.enqueue(&request) {
        Ok(outcome) => outcome,
        Err(error) => return queue_error(error),
    };
    if !outcome.duplicate {
        let _ = state.queue.append_log(outcome.job.id, "info", "Job queued");
    }
    let response = EnqueueResponse {
        job: JobView::from(outcome.job),
        duplicate: outcome.duplicate,
    };
    let _ = state.wake.send(());
    (StatusCode::ACCEPTED, Json(response)).into_response()
}

async fn get_job(State(state): State<Arc<DaemonState>>, Path(id): Path<i64>) -> Response {
    match state.queue.get(id) {
        Ok(job) => Json(JobView::from(job)).into_response(),
        Err(error) => queue_error(error),
    }
}

#[derive(Debug, Deserialize)]
struct JobLogsQuery {
    after_id: Option<i64>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
struct JobLogsResponse {
    logs: Vec<JobLogRecord>,
}

async fn get_job_logs(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
    Query(query): Query<JobLogsQuery>,
) -> Response {
    match state
        .queue
        .logs(id, query.after_id, query.limit.unwrap_or(5_000))
    {
        Ok(logs) => Json(JobLogsResponse { logs }).into_response(),
        Err(error) => queue_error(error),
    }
}

async fn cancel_job(State(state): State<Arc<DaemonState>>, Path(id): Path<i64>) -> Response {
    match state.queue.cancel(id) {
        Ok(job) => {
            if job.cancel_requested && job.state == JobState::Running {
                state.cancel_requested.store(true, Ordering::Release);
                let _ = state
                    .queue
                    .append_log(id, "warning", "Cancellation requested");
                if let Ok(mut worker) = state.worker.lock() {
                    if worker.current_job_id == Some(id) {
                        worker.state = "canceling".to_string();
                    }
                }
            } else {
                let _ = state.queue.append_log(id, "warning", "Job canceled");
            }
            Json(JobView::from(job)).into_response()
        }
        Err(error) => queue_error(error),
    }
}

async fn retry_job(State(state): State<Arc<DaemonState>>, Path(id): Path<i64>) -> Response {
    match state.queue.retry(id) {
        Ok(job) => {
            let _ = state.queue.append_log(id, "info", "Job queued for retry");
            let _ = state.wake.send(());
            Json(JobView::from(job)).into_response()
        }
        Err(error) => queue_error(error),
    }
}

async fn delete_history(State(state): State<Arc<DaemonState>>, Path(id): Path<i64>) -> Response {
    match state.queue.delete_terminal(id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => queue_error(error),
    }
}

async fn download_artifact(
    State(state): State<Arc<DaemonState>>,
    Path((job_id, artifact_id)): Path<(i64, i64)>,
) -> Response {
    let artifact = match state.queue.get_artifact(job_id, artifact_id) {
        Ok(artifact) => artifact,
        Err(error) => return queue_error(error),
    };
    let bytes = match std::fs::read(&artifact.path) {
        Ok(bytes) => bytes,
        Err(io_error) => return error(StatusCode::NOT_FOUND, "not_found", io_error.to_string()),
    };
    let content_type = HeaderValue::from_str(&artifact.content_type)
        .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream"));
    let filename = artifact.name.replace(['"', '\r', '\n'], "_");
    let disposition = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"));
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, content_type);
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition);
    response
}

fn queue_error(queue_error: QueueError) -> Response {
    match queue_error {
        QueueError::NotFound(id) => error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("job {id} was not found"),
        ),
        QueueError::InvalidState => error(
            StatusCode::CONFLICT,
            "invalid_state",
            "job is not in a valid state for this operation",
        ),
        QueueError::Conflict => error(
            StatusCode::CONFLICT,
            "conflict",
            "job conflicts with an existing idempotency key or active resource",
        ),
        QueueError::Database(message) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
        }
    }
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Deserialize)]
struct RssSubscriptionRequest {
    url: String,
    #[serde(default)]
    filter_regex: Option<String>,
    target_folder: String,
    interval_secs: i64,
    #[serde(default)]
    connection_id: Option<i64>,
    #[serde(default)]
    auto_organize: Option<bool>,
    #[serde(default)]
    organize_target_folder: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_option")]
    organize_target_connection_id: Option<Option<i64>>,
    #[serde(default)]
    organize_season_mode: Option<bool>,
    #[serde(default)]
    remove_empty_dirs: Option<bool>,
    #[serde(default)]
    remote_mlip: Option<bool>,
    #[serde(default)]
    remote_overwrite: Option<bool>,
    #[serde(default)]
    organize_mode: Option<String>,
    #[serde(default)]
    organize_interval_secs: Option<i64>,
}

#[cfg(feature = "clouddrive")]
fn deserialize_present_option<'de, D, T>(
    deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Clone)]
struct RssOrganizationSettings {
    auto_organize: bool,
    organize_target_folder: Option<String>,
    organize_target_connection_id: Option<i64>,
    organize_season_mode: bool,
    remove_empty_dirs: bool,
    remote_mlip: bool,
    remote_overwrite: bool,
    organize_mode: String,
    organize_interval_secs: i64,
}

#[cfg(feature = "clouddrive")]
fn organization_settings(
    request: &RssSubscriptionRequest,
    existing: Option<&anime_organizer::rss::db::Subscription>,
) -> RssOrganizationSettings {
    RssOrganizationSettings {
        auto_organize: request
            .auto_organize
            .unwrap_or_else(|| existing.is_some_and(|subscription| subscription.auto_organize)),
        organize_target_folder: request.organize_target_folder.clone().or_else(|| {
            existing.and_then(|subscription| subscription.organize_target_folder.clone())
        }),
        organize_target_connection_id: request.organize_target_connection_id.unwrap_or_else(|| {
            existing.and_then(|subscription| subscription.organize_target_connection_id)
        }),
        organize_season_mode: request.organize_season_mode.unwrap_or_else(|| {
            existing.is_none_or(|subscription| subscription.organize_season_mode)
        }),
        remove_empty_dirs: request
            .remove_empty_dirs
            .unwrap_or_else(|| existing.is_none_or(|subscription| subscription.remove_empty_dirs)),
        remote_mlip: request
            .remote_mlip
            .unwrap_or_else(|| existing.is_some_and(|subscription| subscription.remote_mlip)),
        remote_overwrite: request
            .remote_overwrite
            .unwrap_or_else(|| existing.is_some_and(|subscription| subscription.remote_overwrite)),
        organize_mode: request
            .organize_mode
            .clone()
            .or_else(|| existing.map(|subscription| subscription.organize_mode.clone()))
            .unwrap_or_else(|| "offline".to_string()),
        organize_interval_secs: request.organize_interval_secs.unwrap_or_else(|| {
            existing.map_or(300, |subscription| subscription.organize_interval_secs)
        }),
    }
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Deserialize)]
struct RssHistoryQuery {
    subscription_id: Option<i64>,
    status: Option<String>,
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Serialize)]
struct RssSubscriptionsResponse {
    subscriptions: Vec<anime_organizer::rss::db::Subscription>,
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Serialize)]
struct RssProcessedResponse {
    items: Vec<anime_organizer::rss::db::ProcessedItem>,
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Serialize)]
struct RssDownloadTasksResponse {
    tasks: Vec<anime_organizer::rss::db::DownloadTask>,
}

#[cfg(feature = "clouddrive")]
#[allow(clippy::result_large_err)]
fn rss_db(state: &DaemonState) -> Result<RssDatabase, Response> {
    RssDatabase::new(&state.rss_db_path).map_err(|db_error| {
        error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            db_error.to_string(),
        )
    })
}

#[cfg(feature = "clouddrive")]
#[allow(clippy::result_large_err)]
fn validate_rss_request(
    state: &DaemonState,
    request: &RssSubscriptionRequest,
    organization: &RssOrganizationSettings,
) -> Result<(), Response> {
    let parsed_url = url::Url::parse(&request.url).ok();
    if request.url.trim() != request.url
        || request.url.len() > 8192
        || !parsed_url.as_ref().is_some_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
    {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "url must be an HTTP(S) URL of at most 8192 bytes without embedded credentials",
        ));
    }
    if request.target_folder.trim().is_empty() || request.target_folder.len() > 4096 {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "target_folder is required and must be at most 4096 bytes",
        ));
    }
    let minimum_interval =
        i64::try_from(super::rss_schedule::SCHEDULER_INTERVAL.as_secs()).unwrap_or(30);
    if !(minimum_interval..=86_400).contains(&request.interval_secs) {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            format!("interval_secs must be between {minimum_interval} and 86400"),
        ));
    }
    if let Some(pattern) = request.filter_regex.as_deref() {
        if pattern.len() > 4096 || anime_organizer::rss::filter::RssFilter::new(pattern).is_err() {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                "filter_regex is invalid",
            ));
        }
    }
    if !matches!(organization.organize_mode.as_str(), "offline" | "original") {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "organize_mode must be 'offline' or 'original'",
        ));
    }
    if !(60..=86_400).contains(&organization.organize_interval_secs) {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "organize_interval_secs must be between 60 and 86400",
        ));
    }
    if organization.auto_organize {
        let Some(target) = organization.organize_target_folder.as_deref() else {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                "organize_target_folder is required when auto_organize is enabled",
            ));
        };
        let same_connection = organization
            .organize_target_connection_id
            .is_none_or(|target_id| Some(target_id) == request.connection_id);
        if target.trim() != target
            || target.is_empty()
            || target.len() > 4096
            || (same_connection
                && super::remote_organize::validate_remote_organize_paths(
                    &request.target_folder,
                    target,
                )
                .is_err())
        {
            return Err(error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request",
                "organize_target_folder must be a nonempty remote path separate from target_folder",
            ));
        }
        validate_rss_connection(
            state,
            organization
                .organize_target_connection_id
                .or(request.connection_id),
        )?;
    }
    let source = validate_rss_connection(state, request.connection_id)?;
    if source.kind != "clouddrive" {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "RSS download connection must be a CloudDrive connection",
        ));
    }
    Ok(())
}

#[cfg(feature = "clouddrive")]
#[allow(clippy::result_large_err)]
fn validate_rss_connection(
    state: &DaemonState,
    connection_id: Option<i64>,
) -> Result<super::cloud::StoredCloudConnection, Response> {
    let Some(connection_id) = connection_id else {
        return Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "connection_id is required",
        ));
    };
    match state.cloud.repository.get(connection_id) {
        Ok(connection) => Ok(connection),
        Err(CloudError::NotFound(_)) => Err(error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "connection_id was not found",
        )),
        Err(cloud_failure) => Err(cloud_error(cloud_failure)),
    }
}

#[cfg(feature = "clouddrive")]
async fn list_rss_subscriptions(State(state): State<Arc<DaemonState>>) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    match db.list_all_subscriptions() {
        Ok(subscriptions) => Json(RssSubscriptionsResponse { subscriptions }).into_response(),
        Err(db_error) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            db_error.to_string(),
        ),
    }
}

#[cfg(feature = "clouddrive")]
async fn create_rss_subscription(
    State(state): State<Arc<DaemonState>>,
    request: Result<Json<RssSubscriptionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    let organization = organization_settings(&request, None);
    if let Err(response) = validate_rss_request(&state, &request, &organization) {
        return response;
    }
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    match db.add_subscription_with_connection(
        request.url.trim(),
        request.filter_regex.as_deref(),
        request.target_folder.trim(),
        request.interval_secs,
        request.connection_id,
    ) {
        Ok(id) => {
            if let Err(db_error) = db.update_subscription_organization_settings_with_mlip_and_mode(
                id,
                organization.auto_organize,
                if organization.auto_organize {
                    organization
                        .organize_target_folder
                        .as_deref()
                        .map(str::trim)
                } else {
                    None
                },
                organization.organize_season_mode,
                organization.remove_empty_dirs,
                organization.auto_organize && organization.remote_mlip,
                organization.auto_organize && organization.remote_overwrite,
                &organization.organize_mode,
            ) {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                );
            }
            if let Err(db_error) = db.set_subscription_organize_target_connection(
                id,
                organization.organize_target_connection_id,
            ) {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                );
            }
            if let Err(db_error) =
                db.set_subscription_organize_interval(id, organization.organize_interval_secs)
            {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                );
            }
            match db.get_subscription(id) {
                Ok(Some(subscription)) => (StatusCode::CREATED, Json(subscription)).into_response(),
                Ok(None) => error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "created subscription disappeared",
                ),
                Err(db_error) => error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                ),
            }
        }
        Err(db_error) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            db_error.to_string(),
        ),
    }
}

#[cfg(feature = "clouddrive")]
async fn update_rss_subscription(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
    request: Result<Json<RssSubscriptionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    let existing = match db.get_subscription(id) {
        Ok(Some(subscription)) => subscription,
        Ok(None) => {
            return error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("RSS subscription {id} was not found"),
            )
        }
        Err(db_error) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                db_error.to_string(),
            )
        }
    };
    let organization = organization_settings(&request, Some(&existing));
    if let Err(response) = validate_rss_request(&state, &request, &organization) {
        return response;
    }
    if let Err(db_error) = db.update_subscription(
        id,
        request.url.trim(),
        request.filter_regex.as_deref(),
        request.target_folder.trim(),
        request.interval_secs,
        request.connection_id,
    ) {
        return error(StatusCode::NOT_FOUND, "not_found", db_error.to_string());
    }
    if let Err(db_error) = db.update_subscription_organization_settings_with_mlip_and_mode(
        id,
        organization.auto_organize,
        if organization.auto_organize {
            organization
                .organize_target_folder
                .as_deref()
                .map(str::trim)
        } else {
            None
        },
        organization.organize_season_mode,
        organization.remove_empty_dirs,
        organization.auto_organize && organization.remote_mlip,
        organization.auto_organize && organization.remote_overwrite,
        &organization.organize_mode,
    ) {
        return error(StatusCode::NOT_FOUND, "not_found", db_error.to_string());
    }
    if let Err(db_error) = db
        .set_subscription_organize_target_connection(id, organization.organize_target_connection_id)
    {
        return error(StatusCode::NOT_FOUND, "not_found", db_error.to_string());
    }
    if let Err(db_error) =
        db.set_subscription_organize_interval(id, organization.organize_interval_secs)
    {
        return error(StatusCode::NOT_FOUND, "not_found", db_error.to_string());
    }
    match db.get_subscription(id) {
        Ok(Some(subscription)) => Json(subscription).into_response(),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("RSS subscription {id} was not found"),
        ),
        Err(db_error) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            db_error.to_string(),
        ),
    }
}

#[cfg(feature = "clouddrive")]
async fn delete_rss_subscription(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    match db.delete_subscription(id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(db_error) => error(StatusCode::NOT_FOUND, "not_found", db_error.to_string()),
    }
}

#[cfg(feature = "clouddrive")]
async fn set_rss_enabled(state: Arc<DaemonState>, id: i64, enabled: bool) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    if enabled {
        match db.get_subscription(id) {
            Ok(Some(subscription)) => {
                if let Err(response) = validate_rss_connection(&state, subscription.connection_id) {
                    return response;
                }
            }
            Ok(None) => {
                return error(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("RSS subscription {id} was not found"),
                )
            }
            Err(db_error) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                )
            }
        }
    }
    match db.set_subscription_enabled(id, enabled) {
        Ok(()) => match db.get_subscription(id) {
            Ok(Some(subscription)) => Json(subscription).into_response(),
            Ok(None) => error(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("RSS subscription {id} was not found"),
            ),
            Err(db_error) => error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                db_error.to_string(),
            ),
        },
        Err(db_error) => error(StatusCode::NOT_FOUND, "not_found", db_error.to_string()),
    }
}

#[cfg(feature = "clouddrive")]
async fn enable_rss_subscription(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
) -> Response {
    set_rss_enabled(state, id, true).await
}

#[cfg(feature = "clouddrive")]
async fn disable_rss_subscription(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
) -> Response {
    set_rss_enabled(state, id, false).await
}

#[cfg(feature = "clouddrive")]
async fn enqueue_rss(
    state: &DaemonState,
    job: JobSpec,
    idempotency_key: Option<String>,
) -> Response {
    let request = EnqueueRequest {
        idempotency_key,
        origin: JobOrigin::Manual,
        confirmed: false,
        job,
    };
    let outcome = match state.queue.enqueue(&request) {
        Ok(outcome) => outcome,
        Err(error) => return queue_error(error),
    };
    let _ = state.wake.send(());
    (
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "job": JobView::from(outcome.job),
            "duplicate": outcome.duplicate,
        })),
    )
        .into_response()
}

#[cfg(feature = "clouddrive")]
async fn run_rss_subscription(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    match db.get_subscription(id) {
        Ok(Some(subscription)) if subscription.enabled => {
            if let Err(response) = validate_rss_connection(&state, subscription.connection_id) {
                return response;
            }
            enqueue_rss(
                &state,
                JobSpec::RssPoll {
                    subscription_id: id,
                },
                None,
            )
            .await
        }
        Ok(Some(_)) => error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid_request",
            "RSS subscription is disabled",
        ),
        Ok(None) => error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("RSS subscription {id} was not found"),
        ),
        Err(db_error) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            db_error.to_string(),
        ),
    }
}

#[cfg(feature = "clouddrive")]
async fn run_all_rss_subscriptions(State(state): State<Arc<DaemonState>>) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    let subscriptions = match db.list_subscriptions() {
        Ok(subscriptions) => subscriptions,
        Err(db_error) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                db_error.to_string(),
            )
        }
    };
    for subscription in subscriptions {
        if let Err(response) = validate_rss_connection(&state, subscription.connection_id) {
            return response;
        }
    }
    enqueue_rss(&state, JobSpec::RssPollAll, None).await
}

#[cfg(feature = "clouddrive")]
async fn list_processed_items(
    State(state): State<Arc<DaemonState>>,
    Query(query): Query<RssHistoryQuery>,
) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    let mut items = Vec::new();
    let ids = match query.subscription_id {
        Some(id) => vec![id],
        None => match db.list_all_subscriptions() {
            Ok(subscriptions) => subscriptions
                .into_iter()
                .map(|subscription| subscription.id)
                .collect(),
            Err(db_error) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                )
            }
        },
    };
    for id in ids {
        match db.list_processed_items(id) {
            Ok(mut values) => items.append(&mut values),
            Err(db_error) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                )
            }
        }
    }
    Json(RssProcessedResponse { items }).into_response()
}

#[cfg(feature = "clouddrive")]
async fn list_download_tasks(
    State(state): State<Arc<DaemonState>>,
    Query(query): Query<RssHistoryQuery>,
) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    let ids = match query.subscription_id {
        Some(id) => vec![id],
        None => match db.list_all_subscriptions() {
            Ok(subscriptions) => subscriptions
                .into_iter()
                .map(|subscription| subscription.id)
                .collect(),
            Err(db_error) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                )
            }
        },
    };
    let mut tasks = Vec::new();
    for id in ids {
        match db.list_download_tasks(id, query.status.as_deref()) {
            Ok(mut values) => tasks.append(&mut values),
            Err(db_error) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    db_error.to_string(),
                )
            }
        }
    }
    Json(RssDownloadTasksResponse { tasks }).into_response()
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Deserialize)]
struct FolderRequest {
    path: String,
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Serialize)]
struct CloudConnectionsResponse {
    connections: Vec<CloudConnectionView>,
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Serialize)]
struct CloudFolderResponse {
    entries: Vec<CloudFolderEntry>,
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Serialize)]
struct CloudTestResponse {
    ok: bool,
    authenticated: bool,
}

#[cfg(feature = "clouddrive")]
async fn list_cloud_connections(State(state): State<Arc<DaemonState>>) -> Response {
    match state.cloud.repository.list() {
        Ok(connections) => Json(CloudConnectionsResponse {
            connections: connections.iter().map(CloudConnectionView::from).collect(),
        })
        .into_response(),
        Err(error) => cloud_error(error),
    }
}

#[cfg(feature = "clouddrive")]
async fn create_cloud_connection(
    State(state): State<Arc<DaemonState>>,
    request: Result<Json<CloudConnectionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    let request = match request.normalize() {
        Ok(request) => request,
        Err(error) => return cloud_error(error),
    };
    match state.cloud.repository.create(&request) {
        Ok(connection) => (
            StatusCode::CREATED,
            Json(CloudConnectionView::from(&connection)),
        )
            .into_response(),
        Err(error) => cloud_error(error),
    }
}

#[cfg(feature = "clouddrive")]
async fn update_cloud_connection(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
    request: Result<Json<CloudConnectionRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    let request = match request.normalize() {
        Ok(request) => request,
        Err(error) => return cloud_error(error),
    };
    match state.cloud.repository.update(id, &request) {
        Ok(connection) => Json(CloudConnectionView::from(&connection)).into_response(),
        Err(error) => cloud_error(error),
    }
}

#[cfg(feature = "clouddrive")]
#[derive(Debug, Deserialize)]
struct CloudCredentialsPatchRequest {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

#[cfg(feature = "clouddrive")]
async fn patch_cloud_connection(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
    request: Result<Json<CloudCredentialsPatchRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    match state.cloud.repository.patch_credentials(
        id,
        request.token,
        request.username,
        request.password,
    ) {
        Ok(connection) => Json(CloudConnectionView::from(&connection)).into_response(),
        Err(error) => cloud_error(error),
    }
}

#[cfg(feature = "clouddrive")]
async fn delete_cloud_connection(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
) -> Response {
    let db = match rss_db(&state) {
        Ok(db) => db,
        Err(response) => return response,
    };
    match db.list_all_subscriptions() {
        Ok(subscriptions)
            if subscriptions.iter().any(|subscription| {
                subscription.connection_id == Some(id)
                    || subscription.organize_target_connection_id == Some(id)
            }) =>
        {
            return error(
                StatusCode::CONFLICT,
                "connection_in_use",
                "CloudDrive connection is referenced by an RSS subscription",
            );
        }
        Ok(_) => {}
        Err(db_error) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                db_error.to_string(),
            );
        }
    }
    match state.cloud.repository.delete(id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => cloud_error(error),
    }
}

#[cfg(feature = "clouddrive")]
async fn test_cloud_connection(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
) -> Response {
    let connection = match state.cloud.repository.get(id) {
        Ok(connection) => connection,
        Err(error) => return cloud_error(error),
    };
    let client = match state.cloud.authenticated_client(&connection).await {
        Ok(client) => client,
        Err(error) => return cloud_error(error),
    };
    match tokio::time::timeout(
        Duration::from_secs(CLOUD_OPERATION_TIMEOUT_SECS),
        client.list_folder("/"),
    )
    .await
    {
        Ok(Ok(_)) => {}
        Ok(Err(_)) | Err(_) => return cloud_error(CloudError::Operation),
    }
    Json(CloudTestResponse {
        ok: true,
        authenticated: true,
    })
    .into_response()
}

#[cfg(feature = "clouddrive")]
async fn list_cloud_folder(
    State(state): State<Arc<DaemonState>>,
    Path(id): Path<i64>,
    request: Result<Json<FolderRequest>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Json(request) = match request {
        Ok(request) => request,
        Err(rejection) => {
            return error(
                StatusCode::BAD_REQUEST,
                "malformed_json",
                rejection.body_text(),
            )
        }
    };
    if request.path.is_empty()
        || request.path.len() > MAX_FOLDER_PATH_BYTES
        || request.path.contains('\0')
    {
        return cloud_error(CloudError::Invalid(
            "folder path must be 1-4096 bytes and contain no NUL".to_string(),
        ));
    }
    let connection = match state.cloud.repository.get(id) {
        Ok(connection) => connection,
        Err(error) => return cloud_error(error),
    };
    let client = match state.cloud.authenticated_client(&connection).await {
        Ok(client) => client,
        Err(error) => return cloud_error(error),
    };
    let files = match tokio::time::timeout(
        Duration::from_secs(CLOUD_OPERATION_TIMEOUT_SECS),
        client.list_folder(&request.path),
    )
    .await
    {
        Ok(Ok(files)) => files,
        Ok(Err(_)) | Err(_) => return cloud_error(CloudError::Operation),
    };
    if files.len() > MAX_FOLDER_ENTRIES {
        return cloud_error(CloudError::Invalid(format!(
            "folder contains more than {MAX_FOLDER_ENTRIES} entries"
        )));
    }
    Json(CloudFolderResponse {
        entries: files.into_iter().map(CloudFolderEntry::from).collect(),
    })
    .into_response()
}

#[cfg(feature = "clouddrive")]
fn cloud_error(cloud_error: CloudError) -> Response {
    match cloud_error {
        CloudError::NotFound(id) => error(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("cloud connection {id} was not found"),
        ),
        CloudError::Invalid(message) => {
            error(StatusCode::UNPROCESSABLE_ENTITY, "invalid_request", message)
        }
        CloudError::Database(message) => {
            error(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
        }
        CloudError::Operation => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "cloud_operation_failed",
            "CloudDrive operation failed",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_include_layout_maintenance_jobs() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let Json(response) = runtime.block_on(capabilities());

        assert!(response.job_types.contains(&"normalize_layout"));
        assert!(response.job_types.contains(&"compact_artwork_packs"));
    }

    #[test]
    fn error_envelope_is_stable() {
        let response = error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "target is required",
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn local_host_and_origin_validation_rejects_rebinding() {
        assert!(local_host_allowed("127.0.0.1:32145"));
        assert!(local_host_allowed("LOCALHOST:32145"));
        assert!(!local_host_allowed("attacker.example"));
        assert!(local_origin_allowed("http://127.0.0.1:4173"));
        assert!(local_origin_allowed("http://localhost:32145"));
        assert!(!local_origin_allowed("https://attacker.example"));
        assert!(!local_origin_allowed("null"));
    }

    #[tokio::test]
    async fn api_fallback_is_json_not_found() {
        let response = not_found_or_index("/api/v1/missing".parse().unwrap()).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = not_found_or_index("/jobs/1".parse().unwrap()).await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[cfg(feature = "clouddrive")]
    mod cloud_handlers {
        use super::*;
        use crate::daemon::cloud::{
            CloudConnectionRepository, CloudConnectionRequest, CloudDriveClientFactory,
            CloudDriveState,
        };
        use crate::daemon::queue::QueueRepository;
        use crate::daemon::worker::WorkerSnapshot;
        use anime_organizer::error::Result;
        use anime_organizer::rss::client::CloudDriveClientTrait;
        use async_trait::async_trait;
        use axum::body::to_bytes;
        use std::sync::{mpsc, Arc, Mutex};
        use tempfile::tempdir;

        type LoginCalls = Arc<Mutex<Vec<(String, String)>>>;

        #[derive(Clone)]
        struct MockCloudClient {
            login_calls: LoginCalls,
        }

        #[async_trait]
        impl CloudDriveClientTrait for MockCloudClient {
            async fn login(&mut self, username: &str, password: &str) -> Result<String> {
                self.login_calls
                    .lock()
                    .unwrap()
                    .push((username.to_string(), password.to_string()));
                Ok("mock-login-token".to_string())
            }

            async fn add_offline_files(&self, _urls: Vec<String>, _to_folder: &str) -> Result<()> {
                Ok(())
            }

            async fn list_folder(
                &self,
                _path: &str,
            ) -> Result<Vec<anime_organizer::rss::client::proto::CloudDriveFile>> {
                Ok(vec![anime_organizer::rss::client::proto::CloudDriveFile {
                    id: "1".to_string(),
                    name: "Anime".to_string(),
                    full_path_name: "/Anime".to_string(),
                    size: 42,
                    is_directory: true,
                    ..Default::default()
                }])
            }
        }

        fn test_state(directory: &std::path::Path) -> (Arc<DaemonState>, LoginCalls) {
            let repository = CloudConnectionRepository::new(&directory.join("daemon.db")).unwrap();
            let login_calls = Arc::new(Mutex::new(Vec::new()));
            let mock = MockCloudClient {
                login_calls: login_calls.clone(),
            };
            let factory: CloudDriveClientFactory =
                Arc::new(move |_| Ok(Box::new(mock.clone()) as Box<dyn CloudDriveClientTrait>));
            let queue = QueueRepository::new(&directory.join("daemon.db")).unwrap();
            let (wake, _) = mpsc::channel();
            (
                Arc::new(DaemonState {
                    queue,
                    cloud: CloudDriveState::with_factory(repository, factory),
                    rss_db_path: directory.join("rss.db"),
                    started_at: std::time::Instant::now(),
                    wake,
                    worker: Arc::new(Mutex::new(WorkerSnapshot::default())),
                    cancel_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    #[cfg(feature = "self-update")]
                    app_update: Arc::new(Mutex::new(
                        crate::daemon::app_update::AppUpdateStatus::starting(),
                    )),
                    #[cfg(feature = "self-update")]
                    app_job_running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                    #[cfg(feature = "self-update")]
                    app_update_retry_at: Arc::new(Mutex::new(None)),
                }),
                login_calls,
            )
        }

        async fn body_text(response: Response) -> String {
            let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
            String::from_utf8(body.to_vec()).unwrap()
        }

        fn create_test_connection(state: &Arc<DaemonState>) -> i64 {
            state
                .cloud
                .repository
                .create(
                    &CloudConnectionRequest {
                        kind: "clouddrive".to_string(),
                        name: "rss".to_string(),
                        url: "http://localhost:19798".to_string(),
                        token: Some("token".to_string()),
                        username: None,
                        password: None,
                    }
                    .normalize()
                    .unwrap(),
                )
                .unwrap()
                .id
        }

        #[tokio::test]
        async fn connection_handlers_redact_login_and_list_folders() {
            let directory = tempdir().unwrap();
            let (state, login_calls) = test_state(directory.path());
            let response = create_cloud_connection(
                State(state.clone()),
                Ok(Json(CloudConnectionRequest {
                    kind: "clouddrive".to_string(),
                    name: "primary".to_string(),
                    url: "http://localhost:19798".to_string(),
                    token: None,
                    username: Some("user".to_string()),
                    password: Some("secret-password".to_string()),
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::CREATED);
            let body = body_text(response).await;
            assert!(!body.contains("secret-password"));
            assert!(body.contains("has_password"));

            let response = list_cloud_folder(
                State(state.clone()),
                Path(1),
                Ok(Json(FolderRequest {
                    path: "/".to_string(),
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = body_text(response).await;
            assert!(body.contains("Anime"));
            assert!(body.contains("/Anime"));
            assert_eq!(
                state.cloud.repository.get(1).unwrap().token.as_deref(),
                Some("mock-login-token")
            );

            let response = test_cloud_connection(State(state.clone()), Path(1)).await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = body_text(response).await;
            assert!(body.contains("authenticated"));
            assert!(!body.contains("mock-login-token"));

            let response = list_cloud_folder(
                State(state),
                Path(1),
                Ok(Json(FolderRequest {
                    path: "/".to_string(),
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let body = body_text(response).await;
            assert!(body.contains("Anime"));
            assert!(body.contains("/Anime"));
            assert_eq!(login_calls.lock().unwrap().len(), 3);
        }

        #[tokio::test]
        async fn folder_handler_rejects_unbounded_paths() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let response = list_cloud_folder(
                State(state),
                Path(1),
                Ok(Json(FolderRequest {
                    path: "x".repeat(4097),
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }

        #[test]
        fn rss_target_connection_distinguishes_omitted_from_null() {
            let omitted: RssSubscriptionRequest = serde_json::from_value(serde_json::json!({
                "url": "https://example.test/rss.xml",
                "target_folder": "/source",
                "interval_secs": 300,
                "connection_id": 1
            }))
            .unwrap();
            assert_eq!(omitted.organize_target_connection_id, None);

            let cleared: RssSubscriptionRequest = serde_json::from_value(serde_json::json!({
                "url": "https://example.test/rss.xml",
                "target_folder": "/source",
                "interval_secs": 300,
                "connection_id": 1,
                "organize_target_connection_id": null
            }))
            .unwrap();
            assert_eq!(cleared.organize_target_connection_id, Some(None));
        }

        #[tokio::test]
        async fn rss_interval_matches_scheduler_granularity() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let malformed = create_rss_subscription(
                State(state.clone()),
                Ok(Json(RssSubscriptionRequest {
                    url: "https://".to_string(),
                    filter_regex: None,
                    target_folder: "/anime".to_string(),
                    interval_secs: 300,
                    connection_id: None,
                    auto_organize: Some(false),
                    organize_target_folder: None,
                    organize_target_connection_id: None,
                    organize_season_mode: Some(true),
                    remove_empty_dirs: Some(false),
                    remote_mlip: Some(false),
                    remote_overwrite: None,
                    organize_mode: None,
                    organize_interval_secs: Some(300),
                })),
            )
            .await;
            assert_eq!(malformed.status(), StatusCode::UNPROCESSABLE_ENTITY);

            let too_fast = create_rss_subscription(
                State(state.clone()),
                Ok(Json(RssSubscriptionRequest {
                    url: "https://example.test/fast.xml".to_string(),
                    filter_regex: None,
                    target_folder: "/anime".to_string(),
                    interval_secs: 29,
                    connection_id: None,
                    auto_organize: Some(false),
                    organize_target_folder: None,
                    organize_target_connection_id: None,
                    organize_season_mode: Some(true),
                    remove_empty_dirs: Some(false),
                    remote_mlip: Some(false),
                    remote_overwrite: None,
                    organize_mode: None,
                    organize_interval_secs: Some(300),
                })),
            )
            .await;
            assert_eq!(too_fast.status(), StatusCode::UNPROCESSABLE_ENTITY);

            let connection_id = create_test_connection(&state);
            let accepted = create_rss_subscription(
                State(state),
                Ok(Json(RssSubscriptionRequest {
                    url: "https://example.test/accepted.xml".to_string(),
                    filter_regex: None,
                    target_folder: "/anime".to_string(),
                    interval_secs: 30,
                    connection_id: Some(connection_id),
                    auto_organize: Some(false),
                    organize_target_folder: None,
                    organize_target_connection_id: None,
                    organize_season_mode: Some(true),
                    remove_empty_dirs: Some(false),
                    remote_mlip: Some(false),
                    remote_overwrite: None,
                    organize_mode: None,
                    organize_interval_secs: Some(300),
                })),
            )
            .await;
            assert_eq!(accepted.status(), StatusCode::CREATED);
        }

        #[tokio::test]
        async fn rejects_auto_organize_target_matching_source() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let connection_id = create_test_connection(&state);
            let response = create_rss_subscription(
                State(state),
                Ok(Json(RssSubscriptionRequest {
                    url: "https://example.test/invalid-organize.xml".to_string(),
                    filter_regex: None,
                    target_folder: "/anime".to_string(),
                    interval_secs: 30,
                    connection_id: Some(connection_id),
                    auto_organize: Some(true),
                    organize_target_folder: Some("/anime/".to_string()),
                    organize_target_connection_id: None,
                    organize_season_mode: Some(true),
                    remove_empty_dirs: Some(false),
                    remote_mlip: Some(false),
                    remote_overwrite: None,
                    organize_mode: None,
                    organize_interval_secs: Some(300),
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }

        #[tokio::test]
        async fn old_client_update_preserves_omitted_organization_settings() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let connection_id = create_test_connection(&state);
            let db = RssDatabase::new(&state.rss_db_path).unwrap();
            let id = db
                .add_subscription_with_connection(
                    "https://example.test/feed.xml",
                    None,
                    "/anime",
                    300,
                    Some(connection_id),
                )
                .unwrap();
            db.update_subscription_organization_settings_with_mlip(
                id,
                true,
                Some("/library"),
                false,
                true,
                true,
            )
            .unwrap();
            db.set_subscription_organize_interval(id, 600).unwrap();

            let response = update_rss_subscription(
                State(state.clone()),
                Path(id),
                Ok(Json(RssSubscriptionRequest {
                    url: "https://example.test/feed.xml".to_string(),
                    filter_regex: None,
                    target_folder: "/anime".to_string(),
                    interval_secs: 300,
                    connection_id: Some(connection_id),
                    auto_organize: None,
                    organize_target_folder: None,
                    organize_target_connection_id: None,
                    organize_season_mode: None,
                    remove_empty_dirs: None,
                    remote_mlip: None,
                    remote_overwrite: None,
                    organize_mode: None,
                    organize_interval_secs: None,
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK);
            let subscription = RssDatabase::new(&state.rss_db_path)
                .unwrap()
                .get_subscription(id)
                .unwrap()
                .unwrap();
            assert!(subscription.auto_organize);
            assert_eq!(
                subscription.organize_target_folder.as_deref(),
                Some("/library")
            );
            assert!(!subscription.organize_season_mode);
            assert!(subscription.remove_empty_dirs);
            assert!(subscription.remote_mlip);
            assert_eq!(subscription.organize_mode, "offline");
            assert_eq!(subscription.organize_interval_secs, 600);
        }

        #[tokio::test]
        async fn referenced_cloud_connection_cannot_be_deleted() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let connection = state
                .cloud
                .repository
                .create(
                    &CloudConnectionRequest {
                        kind: "clouddrive".to_string(),
                        name: "rss".to_string(),
                        url: "http://localhost:19798".to_string(),
                        token: Some("token".to_string()),
                        username: None,
                        password: None,
                    }
                    .normalize()
                    .unwrap(),
                )
                .unwrap();
            let db = RssDatabase::new(&state.rss_db_path).unwrap();
            let subscription_id = db
                .add_subscription_with_connection(
                    "https://example.test/linked.xml",
                    None,
                    "/anime",
                    300,
                    Some(connection.id),
                )
                .unwrap();

            let in_use = delete_cloud_connection(State(state.clone()), Path(connection.id)).await;
            assert_eq!(in_use.status(), StatusCode::CONFLICT);
            db.delete_subscription(subscription_id).unwrap();
            let deleted = delete_cloud_connection(State(state), Path(connection.id)).await;
            assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
        }

        #[tokio::test]
        async fn legacy_rss_without_connection_cannot_run_or_enable() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let db = RssDatabase::new(&state.rss_db_path).unwrap();
            let id = db
                .add_subscription_with_connection(
                    "https://example.test/legacy.xml",
                    None,
                    "/anime",
                    300,
                    None,
                )
                .unwrap();

            let response = run_rss_subscription(State(state.clone()), Path(id)).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            let response = run_all_rss_subscriptions(State(state.clone())).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
            db.set_subscription_enabled(id, false).unwrap();
            let response = enable_rss_subscription(State(state), Path(id)).await;
            assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        }

        #[tokio::test]
        async fn rss_run_now_uses_registered_typed_job_and_resource_conflict() {
            let directory = tempdir().unwrap();
            let (state, _) = test_state(directory.path());
            let connection_id = create_test_connection(&state);
            let response = create_rss_subscription(
                State(state.clone()),
                Ok(Json(RssSubscriptionRequest {
                    url: "https://example.test/feed.xml".to_string(),
                    filter_regex: None,
                    target_folder: "/anime".to_string(),
                    interval_secs: 300,
                    connection_id: Some(connection_id),
                    auto_organize: Some(false),
                    organize_target_folder: None,
                    organize_target_connection_id: None,
                    organize_season_mode: Some(true),
                    remove_empty_dirs: Some(false),
                    remote_mlip: Some(false),
                    remote_overwrite: None,
                    organize_mode: None,
                    organize_interval_secs: Some(300),
                })),
            )
            .await;
            assert_eq!(response.status(), StatusCode::CREATED);
            let first = run_rss_subscription(State(state.clone()), Path(1)).await;
            assert_eq!(first.status(), StatusCode::ACCEPTED);
            let second = run_rss_subscription(State(state), Path(1)).await;
            assert_eq!(second.status(), StatusCode::CONFLICT);
        }
    }
}
