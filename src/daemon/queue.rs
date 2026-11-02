#[cfg(test)]
use super::model::JobSpec;
use super::model::{EnqueueRequest, JobOrigin, JobState, StoredJob};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::path::{Path, PathBuf};
use thiserror::Error;

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    idempotency_key TEXT UNIQUE,
    origin TEXT NOT NULL,
    kind TEXT NOT NULL,
    resource_key TEXT,
    request_json TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued', 'running', 'succeeded', 'failed', 'canceled')),
    priority INTEGER NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    progress_current INTEGER,
    progress_total INTEGER,
    progress_message TEXT,
    cancel_requested INTEGER NOT NULL DEFAULT 0,
    result_json TEXT,
    error TEXT,
    created_at TEXT NOT NULL,
    started_at TEXT,
    finished_at TEXT
);

CREATE INDEX IF NOT EXISTS jobs_queue_order ON jobs(state, priority DESC, id ASC);
CREATE UNIQUE INDEX IF NOT EXISTS jobs_one_active_resource
ON jobs(resource_key)
WHERE resource_key IS NOT NULL AND state IN ('queued', 'running');

PRAGMA user_version = 2;

CREATE TABLE IF NOT EXISTS job_artifacts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    name TEXT NOT NULL,
    content_type TEXT NOT NULL,
    path TEXT NOT NULL,
    size INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(job_id) REFERENCES jobs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS job_logs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id INTEGER NOT NULL,
    level TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at TEXT NOT NULL,
    FOREIGN KEY(job_id) REFERENCES jobs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS job_logs_job_id ON job_logs(job_id, id);
"#;

#[derive(Debug, Error)]
pub(crate) enum QueueError {
    #[error("queue database error: {0}")]
    Database(String),
    #[error("job {0} was not found")]
    NotFound(i64),
    #[error("job is in an invalid state")]
    InvalidState,
    #[error("job conflicts with an existing active resource or idempotency key")]
    Conflict,
}

pub(crate) type QueueResult<T> = std::result::Result<T, QueueError>;

#[derive(Debug, Clone)]
pub(crate) struct QueueRepository {
    path: PathBuf,
}

#[derive(Debug, Clone)]
pub(crate) struct EnqueueOutcome {
    pub(crate) job: StoredJob,
    pub(crate) duplicate: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ArtifactRecord {
    pub(crate) name: String,
    pub(crate) content_type: String,
    pub(crate) path: PathBuf,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct JobLogRecord {
    pub(crate) id: i64,
    pub(crate) level: String,
    pub(crate) message: String,
    pub(crate) created_at: String,
}

impl QueueRepository {
    pub(crate) fn new(path: &Path) -> QueueResult<Self> {
        let repository = Self {
            path: path.to_path_buf(),
        };
        repository.with_connection(|conn| {
            conn.execute_batch(SCHEMA)
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let has_cancel_requested = conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('jobs') WHERE name = 'cancel_requested')",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| QueueError::Database(error.to_string()))?
                != 0;
            if !has_cancel_requested {
                conn.execute_batch(
                    "ALTER TABLE jobs ADD COLUMN cancel_requested INTEGER NOT NULL DEFAULT 0;",
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            }
            conn.execute_batch("PRAGMA user_version = 2;")
                .map_err(|error| QueueError::Database(error.to_string()))
        })?;
        Ok(repository)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    fn with_connection<T>(
        &self,
        operation: impl FnOnce(&mut Connection) -> QueueResult<T>,
    ) -> QueueResult<T> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| QueueError::Database(error.to_string()))?;
        }
        let mut connection = Connection::open(&self.path)
            .map_err(|error| QueueError::Database(error.to_string()))?;
        connection
            .execute_batch("PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 5000;")
            .map_err(|error| QueueError::Database(error.to_string()))?;
        operation(&mut connection)
    }

    pub(crate) fn enqueue(&self, request: &EnqueueRequest) -> QueueResult<EnqueueOutcome> {
        let request_json = serde_json::to_string(&request.job)
            .map_err(|error| QueueError::Database(error.to_string()))?;
        let kind = request.job.kind();
        let resource_key = request.job.resource_key();
        let created_at = now_string();
        self.with_connection(|conn| {
            let transaction = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| QueueError::Database(error.to_string()))?;
            if let Some(idempotency_key) = request.idempotency_key.as_deref() {
                let existing = transaction
                    .query_row(
                        "SELECT id FROM jobs WHERE idempotency_key = ?1",
                        params![idempotency_key],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(|error| QueueError::Database(error.to_string()))?;
                if let Some(id) = existing {
                    transaction
                        .commit()
                        .map_err(|error| QueueError::Database(error.to_string()))?;
                    return Ok(EnqueueOutcome {
                        job: self.get(id)?,
                        duplicate: true,
                    });
                }
            }

            let rss_overlap = match kind {
                "rss_poll" | "remote_rss_organize" => transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM jobs WHERE resource_key = 'rss:all' AND state IN ('queued', 'running'))",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|value| value != 0),
                "rss_poll_all" => transaction
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind IN ('rss_poll', 'rss_poll_all', 'remote_rss_organize') AND state IN ('queued', 'running'))",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map(|value| value != 0),
                _ => Ok(false),
            }
            .map_err(|error| QueueError::Database(error.to_string()))?;
            if rss_overlap {
                return Err(QueueError::Conflict);
            }
            let insert = transaction.execute(
                "INSERT INTO jobs (idempotency_key, origin, kind, resource_key, request_json, state, priority, created_at) VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6, ?7)",
                params![
                    request.idempotency_key,
                    request.origin.as_str(),
                    kind,
                    resource_key,
                    request_json,
                    request.origin.priority(),
                    created_at,
                ],
            );
            match insert {
                Ok(_) => {
                    let id = transaction.last_insert_rowid();
                    transaction
                        .commit()
                        .map_err(|error| QueueError::Database(error.to_string()))?;
                    Ok(EnqueueOutcome {
                        job: self.get(id)?,
                        duplicate: false,
                    })
                }
                Err(_) if request.idempotency_key.is_some() => {
                    let existing = transaction
                        .query_row(
                            "SELECT id FROM jobs WHERE idempotency_key = ?1",
                            params![request.idempotency_key],
                            |row| row.get::<_, i64>(0),
                        )
                        .optional()
                        .map_err(|error| QueueError::Database(error.to_string()))?;
                    if let Some(id) = existing {
                        transaction
                            .commit()
                            .map_err(|error| QueueError::Database(error.to_string()))?;
                        return Ok(EnqueueOutcome {
                            job: self.get(id)?,
                            duplicate: true,
                        });
                    }
                    Err(QueueError::Conflict)
                }
                Err(_) => Err(QueueError::Conflict),
            }
        })
    }

    pub(crate) fn get(&self, id: i64) -> QueueResult<StoredJob> {
        self.with_connection(|conn| {
            conn.query_row(SELECT_JOB, params![id], row_to_job)
                .optional()
                .map_err(|error| QueueError::Database(error.to_string()))?
                .ok_or(QueueError::NotFound(id))
        })
    }

    pub(crate) fn append_log(&self, job_id: i64, level: &str, message: &str) -> QueueResult<()> {
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO job_logs (job_id, level, message, created_at) VALUES (?1, ?2, ?3, ?4)",
                params![job_id, level, message, now_string()],
            )
            .map_err(|error| QueueError::Database(error.to_string()))?;
            Ok(())
        })
    }

    pub(crate) fn logs(
        &self,
        job_id: i64,
        after_id: Option<i64>,
        limit: i64,
    ) -> QueueResult<Vec<JobLogRecord>> {
        self.get(job_id)?;
        self.with_connection(|conn| {
            let mut statement = conn
                .prepare("SELECT id, level, message, created_at FROM job_logs WHERE job_id = ?1 AND (?2 IS NULL OR id > ?2) ORDER BY id ASC LIMIT ?3")
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let rows = statement
                .query_map(params![job_id, after_id, limit.clamp(1, 10_000)], |row| {
                    Ok(JobLogRecord {
                        id: row.get(0)?,
                        level: row.get(1)?,
                        message: row.get(2)?,
                        created_at: row.get(3)?,
                    })
                })
                .map_err(|error| QueueError::Database(error.to_string()))?;
            rows.map(|row| row.map_err(|error| QueueError::Database(error.to_string())))
                .collect()
        })
    }

    #[cfg(any(test, feature = "scraper", feature = "torrent-scraper"))]
    pub(crate) fn store_artifact(
        &self,
        job_id: i64,
        name: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> QueueResult<(i64, i64)> {
        let safe_name = name
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>();
        let safe_name = if safe_name.is_empty() {
            "artifact.bin"
        } else {
            &safe_name
        };
        let directory = self.path.with_extension("artifacts");
        std::fs::create_dir_all(&directory)
            .map_err(|error| QueueError::Database(error.to_string()))?;
        let path = directory.join(format!("{job_id}-{safe_name}"));
        std::fs::write(&path, bytes).map_err(|error| QueueError::Database(error.to_string()))?;
        let size = i64::try_from(bytes.len())
            .map_err(|_| QueueError::Database("artifact is too large".to_string()))?;
        self.with_connection(|conn| {
            conn.execute(
                "INSERT INTO job_artifacts (job_id, name, content_type, path, size, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![job_id, name, content_type, path.to_string_lossy().to_string(), size, now_string()],
            )
            .map_err(|error| QueueError::Database(error.to_string()))?;
            Ok((conn.last_insert_rowid(), size))
        })
    }

    pub(crate) fn get_artifact(
        &self,
        job_id: i64,
        artifact_id: i64,
    ) -> QueueResult<ArtifactRecord> {
        self.with_connection(|conn| {
            conn.query_row(
                "SELECT name, content_type, path FROM job_artifacts WHERE id = ?1 AND job_id = ?2",
                params![artifact_id, job_id],
                |row| {
                    Ok(ArtifactRecord {
                        name: row.get(0)?,
                        content_type: row.get(1)?,
                        path: PathBuf::from(row.get::<_, String>(2)?),
                    })
                },
            )
            .optional()
            .map_err(|error| QueueError::Database(error.to_string()))?
            .ok_or(QueueError::NotFound(artifact_id))
        })
    }

    pub(crate) fn list(
        &self,
        state: Option<JobState>,
        kind: Option<&str>,
        limit: i64,
        before_id: Option<i64>,
    ) -> QueueResult<Vec<StoredJob>> {
        self.with_connection(|conn| {
            let sql = "SELECT ".to_string()
                + JOB_COLUMNS
                + " FROM jobs WHERE (?1 IS NULL OR state = ?1) AND (?2 IS NULL OR kind = ?2) AND (?3 IS NULL OR id < ?3) ORDER BY id DESC LIMIT ?4";
            let mut statement = conn
                .prepare(&sql)
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let rows = statement
                .query_map(
                    params![
                        state.map(JobState::as_str),
                        kind,
                        before_id,
                        limit.clamp(1, 100),
                    ],
                    row_to_job,
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            rows.map(|row| row.map_err(|error| QueueError::Database(error.to_string())))
                .collect()
        })
    }

    pub(crate) fn claim_next(&self) -> QueueResult<Option<StoredJob>> {
        self.with_connection(|conn| {
            let transaction = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let id = transaction
                .query_row(
                    "SELECT id FROM jobs WHERE state = 'queued' ORDER BY priority DESC, id ASC LIMIT 1",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let Some(id) = id else {
                transaction
                    .commit()
                    .map_err(|error| QueueError::Database(error.to_string()))?;
                return Ok(None);
            };
            let changed = transaction
                .execute(
                    "UPDATE jobs SET state = 'running', attempts = attempts + 1, started_at = ?1, progress_message = 'validating', cancel_requested = 0 WHERE id = ?2 AND state = 'queued'",
                    params![now_string(), id],
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            if changed != 1 {
                return Err(QueueError::Conflict);
            }
            let job = transaction
                .query_row(SELECT_JOB, params![id], row_to_job)
                .map_err(|error| QueueError::Database(error.to_string()))?;
            transaction
                .commit()
                .map_err(|error| QueueError::Database(error.to_string()))?;
            Ok(Some(job))
        })
    }

    pub(crate) fn set_progress(&self, id: i64, message: &str) -> QueueResult<()> {
        self.set_detailed_progress(id, None, None, message)
    }

    pub(crate) fn set_detailed_progress(
        &self,
        id: i64,
        current: Option<i64>,
        total: Option<i64>,
        message: &str,
    ) -> QueueResult<()> {
        self.with_connection(|conn| {
            let changed = conn
                .execute(
                    "UPDATE jobs SET progress_current = ?1, progress_total = ?2, progress_message = ?3 WHERE id = ?4 AND state = 'running'",
                    params![current, total, message, id],
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            if changed == 1 {
                Ok(())
            } else {
                Err(QueueError::InvalidState)
            }
        })
    }

    pub(crate) fn mark_succeeded(&self, id: i64, result_json: &str) -> QueueResult<()> {
        self.finish(id, "succeeded", Some(result_json), None)
    }

    pub(crate) fn mark_failed(&self, id: i64, error: &str) -> QueueResult<()> {
        self.finish(id, "failed", None, Some(error))
    }

    pub(crate) fn mark_canceled(&self, id: i64) -> QueueResult<()> {
        self.finish(id, "canceled", None, None)
    }

    fn finish(
        &self,
        id: i64,
        state: &str,
        result_json: Option<&str>,
        error: Option<&str>,
    ) -> QueueResult<()> {
        self.with_connection(|conn| {
            let changed = conn
                .execute(
                    "UPDATE jobs SET state = ?1, result_json = ?2, error = ?3, finished_at = ?4, progress_message = ?5 WHERE id = ?6 AND state = 'running'",
                    params![state, result_json, error, now_string(), state, id],
                )
                .map_err(|db_error| QueueError::Database(db_error.to_string()))?;
            if changed == 1 {
                Ok(())
            } else {
                Err(QueueError::InvalidState)
            }
        })
    }

    pub(crate) fn cancel(&self, id: i64) -> QueueResult<StoredJob> {
        let job = self.get(id)?;
        if job.state == JobState::Canceled && job.cancel_requested {
            return Ok(job);
        }
        if job.state == JobState::Running && !job.supports_running_cancel() {
            return Err(QueueError::InvalidState);
        }
        self.with_connection(|conn| {
            let changed = match job.state {
                JobState::Queued => conn.execute(
                    "UPDATE jobs SET state = 'canceled', finished_at = ?1, progress_message = 'canceled', cancel_requested = 1 WHERE id = ?2 AND state = 'queued'",
                    params![now_string(), id],
                ),
                JobState::Running => conn.execute(
                    "UPDATE jobs SET cancel_requested = 1, progress_message = 'canceling' WHERE id = ?1 AND state = 'running'",
                    params![id],
                ),
                _ => return Err(QueueError::InvalidState),
            }
            .map_err(|error| QueueError::Database(error.to_string()))?;
            if changed != 1 {
                return Err(self.invalid_transition(conn, id));
            }
            self.get(id)
        })
    }

    pub(crate) fn retry(&self, id: i64) -> QueueResult<StoredJob> {
        self.with_connection(|conn| {
            let changed = conn
                .execute(
                    "UPDATE jobs SET state = 'queued', attempts = attempts, progress_current = NULL, progress_total = NULL, result_json = NULL, error = NULL, started_at = NULL, finished_at = NULL, progress_message = 'queued for retry', cancel_requested = 0 WHERE id = ?1 AND state IN ('failed', 'canceled')",
                    params![id],
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            if changed != 1 {
                return Err(self.invalid_transition(conn, id));
            }
            self.get(id)
        })
    }

    pub(crate) fn delete_terminal(&self, id: i64) -> QueueResult<()> {
        let artifact_paths = self.with_connection(|conn| {
            let mut statement = conn
                .prepare("SELECT path FROM job_artifacts WHERE job_id = ?1")
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let artifact_paths = statement
                .query_map(params![id], |row| row.get::<_, String>(0))
                .map_err(|error| QueueError::Database(error.to_string()))?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(|error| QueueError::Database(error.to_string()))?;
            drop(statement);
            let changed = conn
                .execute(
                    "DELETE FROM jobs WHERE id = ?1 AND state IN ('succeeded', 'failed', 'canceled')",
                    params![id],
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            if changed == 1 {
                Ok(artifact_paths)
            } else {
                Err(self.invalid_transition(conn, id))
            }
        })?;
        for path in artifact_paths {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => eprintln!("failed to remove daemon artifact: {error}"),
            }
        }
        Ok(())
    }

    fn invalid_transition(&self, conn: &Connection, id: i64) -> QueueError {
        match conn
            .query_row("SELECT 1 FROM jobs WHERE id = ?1", params![id], |row| {
                row.get::<_, i64>(0)
            })
            .optional()
        {
            Ok(Some(_)) => QueueError::InvalidState,
            Ok(None) => QueueError::NotFound(id),
            Err(error) => QueueError::Database(error.to_string()),
        }
    }

    pub(crate) fn recover_running(&self) -> QueueResult<usize> {
        self.with_connection(|conn| {
            let canceled = conn
                .execute(
                    "UPDATE jobs SET state = 'canceled', progress_message = 'canceled', finished_at = ?1 WHERE state = 'running' AND cancel_requested = 1",
                    params![now_string()],
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let requeued = conn
                .execute(
                    "UPDATE jobs SET state = 'queued', progress_message = 'daemon interrupted; queued for retry', started_at = NULL WHERE state = 'running'",
                    [],
                )
                .map_err(|error| QueueError::Database(error.to_string()))?;
            Ok(canceled + requeued)
        })
    }

    pub(crate) fn counts(&self) -> QueueResult<[i64; 5]> {
        self.with_connection(|conn| {
            let mut counts = [0; 5];
            let mut statement = conn
                .prepare("SELECT state, COUNT(*) FROM jobs GROUP BY state")
                .map_err(|error| QueueError::Database(error.to_string()))?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })
                .map_err(|error| QueueError::Database(error.to_string()))?;
            for row in rows {
                let (state, count) =
                    row.map_err(|error| QueueError::Database(error.to_string()))?;
                if let Some(index) = JobState::parse(&state).map(state_index) {
                    counts[index] = count;
                }
            }
            Ok(counts)
        })
    }
}

fn state_index(state: JobState) -> usize {
    match state {
        JobState::Queued => 0,
        JobState::Running => 1,
        JobState::Succeeded => 2,
        JobState::Failed => 3,
        JobState::Canceled => 4,
    }
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredJob> {
    Ok(StoredJob {
        id: row.get(0)?,
        idempotency_key: row.get(1)?,
        origin: parse_origin(&row.get::<_, String>(2)?),
        kind: row.get(3)?,
        resource_key: row.get(4)?,
        request_json: row.get(5)?,
        state: JobState::parse(&row.get::<_, String>(6)?).unwrap_or(JobState::Failed),
        priority: row.get(7)?,
        attempts: row.get(8)?,
        progress_current: row.get(9)?,
        progress_total: row.get(10)?,
        progress_message: row.get(11)?,
        cancel_requested: row.get(12)?,
        result_json: row.get(13)?,
        error: row.get(14)?,
        created_at: row.get(15)?,
        started_at: row.get(16)?,
        finished_at: row.get(17)?,
    })
}

fn parse_origin(value: &str) -> JobOrigin {
    match value {
        "qbittorrent" => JobOrigin::Qbittorrent,
        "scheduled" => JobOrigin::Scheduled,
        _ => JobOrigin::Manual,
    }
}

const JOB_COLUMNS: &str = "id, idempotency_key, origin, kind, resource_key, request_json, state, priority, attempts, progress_current, progress_total, progress_message, cancel_requested, result_json, error, created_at, started_at, finished_at";
const SELECT_JOB: &str = "SELECT id, idempotency_key, origin, kind, resource_key, request_json, state, priority, attempts, progress_current, progress_total, progress_message, cancel_requested, result_json, error, created_at, started_at, finished_at FROM jobs WHERE id = ?1";

fn now_string() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(value) => value.as_secs().to_string(),
        Err(_) => "0".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{FilenameParserMode, OrganizeArgs};
    use anime_organizer::OperationMode;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn request(key: Option<&str>, origin: JobOrigin) -> EnqueueRequest {
        EnqueueRequest {
            idempotency_key: key.map(str::to_string),
            origin,
            confirmed: false,
            job: JobSpec::Organize(OrganizeArgs {
                source: Some(PathBuf::from("source")),
                target: Some(PathBuf::from("target")),
                mode: OperationMode::Copy,
                fallback_on_link_failure: None,
                dry_run: false,
                delete_extracted_archives: false,
                scan_extracted_archives: false,
                include_ext: None,
                verbose: false,
                scrape_metadata: false,
                tmdb_api_key: None,
                alias_file: None,
                no_images: false,
                no_episode_metadata: false,
                force_overwrite: false,
                bangumi_cache: None,
                metadata_source: None,
                season_mode: false,
                library_index: false,
                mlip: false,
                rebuild_library_index: false,
                refresh_library_metadata: false,
                probe_runtime: false,
                filename_parser: FilenameParserMode::Rules,
            }),
        }
    }

    #[test]
    fn priority_and_idempotency_are_stable() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let manual = queue
            .enqueue(&request(Some("manual"), JobOrigin::Manual))
            .unwrap();
        let qb = queue
            .enqueue(&request(Some("qb"), JobOrigin::Qbittorrent))
            .unwrap();
        assert_eq!(queue.claim_next().unwrap().unwrap().id, qb.job.id);
        let duplicate = queue
            .enqueue(&request(Some("manual"), JobOrigin::Manual))
            .unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.job.id, manual.job.id);
    }

    #[cfg(feature = "clouddrive")]
    #[test]
    fn rss_resource_keys_prevent_active_overlap() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let request = EnqueueRequest {
            idempotency_key: Some("rss:1:100".to_string()),
            origin: JobOrigin::Scheduled,
            confirmed: false,
            job: JobSpec::RssPoll { subscription_id: 1 },
        };
        let first = queue.enqueue(&request).unwrap();
        assert!(!first.duplicate);
        let duplicate = queue.enqueue(&request).unwrap();
        assert!(duplicate.duplicate);
        assert_eq!(duplicate.job.id, first.job.id);
        let next_window = EnqueueRequest {
            idempotency_key: Some("rss:1:101".to_string()),
            ..request.clone()
        };
        assert!(matches!(
            queue.enqueue(&next_window),
            Err(QueueError::Conflict)
        ));
        let remote = EnqueueRequest {
            idempotency_key: Some("rss-organize:1:100".to_string()),
            origin: JobOrigin::Scheduled,
            confirmed: false,
            job: JobSpec::RemoteRssOrganize { subscription_id: 1 },
        };
        assert!(matches!(queue.enqueue(&remote), Err(QueueError::Conflict)));
    }

    #[test]
    fn artifacts_are_written_and_scoped_to_the_job() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let job = queue
            .enqueue(&request(None, JobOrigin::Manual))
            .unwrap()
            .job;
        let (artifact_id, size) = queue
            .store_artifact(job.id, "result/fixture.json", "application/json", br#"{}"#)
            .unwrap();
        assert_eq!(size, 2);
        let artifact = queue.get_artifact(job.id, artifact_id).unwrap();
        assert_eq!(artifact.name, "result/fixture.json");
        assert_eq!(std::fs::read_to_string(&artifact.path).unwrap(), "{}");
        assert!(queue.get_artifact(job.id + 1, artifact_id).is_err());
        queue.cancel(job.id).unwrap();
        queue.delete_terminal(job.id).unwrap();
        assert!(!artifact.path.exists());
    }

    #[test]
    fn job_logs_are_persistent_and_support_incremental_reads() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("daemon.db");
        let queue = QueueRepository::new(&path).unwrap();
        let job = queue
            .enqueue(&request(None, JobOrigin::Manual))
            .unwrap()
            .job;
        queue.append_log(job.id, "info", "queued").unwrap();
        let first = queue.logs(job.id, None, 100).unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].message, "queued");
        queue.append_log(job.id, "info", "started").unwrap();
        drop(queue);

        let reopened = QueueRepository::new(&path).unwrap();
        let next = reopened.logs(job.id, Some(first[0].id), 100).unwrap();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].message, "started");
    }

    #[test]
    fn detailed_progress_persists_counts_and_message() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let job = queue
            .enqueue(&request(None, JobOrigin::Manual))
            .unwrap()
            .job;
        queue.claim_next().unwrap().unwrap();

        queue
            .set_detailed_progress(job.id, Some(7), Some(19), "Hashing media 7/19")
            .unwrap();

        let stored = queue.get(job.id).unwrap();
        assert_eq!(stored.progress_current, Some(7));
        assert_eq!(stored.progress_total, Some(19));
        assert_eq!(
            stored.progress_message.as_deref(),
            Some("Hashing media 7/19")
        );
    }

    #[test]
    fn legacy_queue_schema_adds_cancel_requested_column() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("daemon.db");
        let legacy_schema = SCHEMA
            .replace("    cancel_requested INTEGER NOT NULL DEFAULT 0,\n", "")
            .replace("PRAGMA user_version = 2;", "PRAGMA user_version = 1;");
        Connection::open(&path)
            .unwrap()
            .execute_batch(&legacy_schema)
            .unwrap();

        QueueRepository::new(&path).unwrap();
        let conn = Connection::open(path).unwrap();
        let has_column: i64 = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('jobs') WHERE name = 'cancel_requested')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(has_column, 1);
        assert_eq!(version, 2);
    }

    #[test]
    fn cancellation_is_idempotent() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let queued = queue
            .enqueue(&request(None, JobOrigin::Manual))
            .unwrap()
            .job;
        assert_eq!(queue.cancel(queued.id).unwrap().state, JobState::Canceled);
        assert_eq!(queue.cancel(queued.id).unwrap().state, JobState::Canceled);

        let running = queue
            .enqueue(&request(None, JobOrigin::Manual))
            .unwrap()
            .job;
        assert_eq!(queue.claim_next().unwrap().unwrap().id, running.id);
        assert_eq!(queue.cancel(running.id).unwrap().state, JobState::Running);
        assert_eq!(queue.cancel(running.id).unwrap().state, JobState::Running);
        queue.mark_canceled(running.id).unwrap();
    }

    #[test]
    fn running_unsafe_jobs_cannot_be_canceled() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let job = queue
            .enqueue(&EnqueueRequest {
                idempotency_key: None,
                origin: JobOrigin::Manual,
                confirmed: true,
                job: JobSpec::CompactArtworkPacks(crate::cli::CompactArtworkPacksArgs::default()),
            })
            .unwrap()
            .job;
        assert_eq!(queue.claim_next().unwrap().unwrap().id, job.id);
        assert!(matches!(
            queue.cancel(job.id),
            Err(QueueError::InvalidState)
        ));
        queue.mark_succeeded(job.id, "{}").unwrap();
    }

    #[test]
    fn recovery_and_transitions_are_restricted() {
        let directory = tempdir().unwrap();
        let queue = QueueRepository::new(&directory.path().join("daemon.db")).unwrap();
        let job = queue
            .enqueue(&request(None, JobOrigin::Manual))
            .unwrap()
            .job;
        assert_eq!(
            queue.claim_next().unwrap().unwrap().state,
            JobState::Running
        );
        let canceling = queue.cancel(job.id).unwrap();
        assert_eq!(canceling.state, JobState::Running);
        assert!(canceling.cancel_requested);
        assert_eq!(canceling.progress_message.as_deref(), Some("canceling"));
        assert_eq!(queue.recover_running().unwrap(), 1);
        assert_eq!(queue.get(job.id).unwrap().state, JobState::Canceled);

        assert!(queue.retry(job.id).is_ok());
        assert_eq!(queue.get(job.id).unwrap().state, JobState::Queued);
        assert!(!queue.get(job.id).unwrap().cancel_requested);
        assert_eq!(
            queue.claim_next().unwrap().unwrap().state,
            JobState::Running
        );
        assert_eq!(queue.recover_running().unwrap(), 1);
        assert_eq!(queue.get(job.id).unwrap().state, JobState::Queued);

        let claimed = queue.claim_next().unwrap().unwrap();
        queue.mark_failed(claimed.id, "bad input").unwrap();
        assert!(queue.retry(claimed.id).is_ok());
        assert_eq!(queue.get(claimed.id).unwrap().state, JobState::Queued);
    }
}
