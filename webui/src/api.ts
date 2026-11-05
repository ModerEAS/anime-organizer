export type JobState = 'queued' | 'running' | 'succeeded' | 'failed' | 'canceled'
export type JobArtifact = { id: number; name: string; content_type: string; size: number; download_url: string }
export type JobLog = { id: number; level: string; message: string; created_at: string }
export type Job = { id: number; idempotency_key: string | null; origin: string; kind: string; resource_key: string | null; request: unknown; state: JobState; priority: number; attempts: number; progress_current: number | null; progress_total: number | null; progress_message: string | null; cancel_requested: boolean; cancelable: boolean; result: unknown; error: string | null; created_at: string; started_at: string | null; finished_at: string | null }
export type AppUpdatePhase = 'idle' | 'checking' | 'downloading' | 'verifying' | 'applying' | 'up_to_date' | 'failed'
export type AppUpdateState = {
  current_version: string
  current_commit: string | null
  latest_version: string | null
  latest_commit: string | null
  phase: AppUpdatePhase
  percent: number
  bytes: number
  total: number | null
  message: string
  error: string | null
  update_available: boolean
  next_check_unix_ms: number | null
  interval: string
  auto_apply: boolean
  feed: string
  proxy: string
  proxy_display: string
  cargo_target: boolean
}
export type AppUpdateSettings = {
  app_update_interval: string
  auto_apply_app_update: boolean
  app_update_feed: string | null
  proxy: string | null
}
export type MetadataSettings = { auto_download_alias_db: boolean }
export type Status = { uptime_seconds: number; worker_state: string; current_job_id: number | null; queue_counts: Record<JobState, number>; database_path: string; app_update?: AppUpdateState }
export type Capabilities = { features: string[]; job_types: string[]; resources: string[] }
export type OrganizeArgs = {
  source: string | null
  target: string | null
  mode: 'move' | 'copy' | 'link'
  fallback_on_link_failure: 'move' | 'copy' | null
  dry_run: boolean
  include_ext: string[] | null
  verbose: boolean
  scrape_metadata: boolean
  tmdb_api_key: string | null
  alias_file: string | null
  no_images: boolean
  no_episode_metadata: boolean
  force_overwrite: boolean
  bangumi_cache: string | null
  metadata_source: string | null
  season_mode: boolean
  library_index: boolean
  mlip: boolean
  rebuild_library_index: boolean
  probe_runtime: boolean
  filename_parser: 'rules' | 'anifilebert' | 'auto'
}
export type NormalizeLayoutArgs = {
  target: string
  dry_run: boolean
  plan: string | null
  apply_plan: string | null
}
export type CompactArtworkPacksArgs = {
  target: string
  dry_run: boolean
  plan: string | null
  apply_plan: string | null
}

export type StorageEndpoint = { type: 'local'; path: string } | { type: 'connection'; connection_id: number; path: string }
export type StorageOrganizeArgs = { source: StorageEndpoint; target: StorageEndpoint; mode: 'move' | 'copy'; season_mode: boolean; mlip: boolean; remove_empty_dirs: boolean }

export class ApiError extends Error {
  constructor(public code: string, message: string, public status: number) { super(message) }
}
export const errorMessage = (error: unknown) => error instanceof Error ? error.message : String(error)

export type Subscription = { id: number; url: string; filter_regex: string | null; target_folder: string; interval_secs: number; enabled: boolean; last_checked_at: string | null; connection_id: number | null; auto_organize: boolean; organize_target_folder: string | null; organize_target_connection_id: number | null; organize_interval_secs: number; organize_season_mode: boolean; remove_empty_dirs: boolean; remote_mlip: boolean; remote_overwrite: boolean; organize_mode: 'offline' | 'original'; last_organize_checked_at: string | null }
export type SubscriptionInput = Pick<Subscription, 'url' | 'target_folder' | 'interval_secs' | 'connection_id'> & Partial<Pick<Subscription, 'filter_regex' | 'auto_organize' | 'organize_target_folder' | 'organize_target_connection_id' | 'organize_interval_secs' | 'organize_season_mode' | 'remove_empty_dirs' | 'remote_mlip' | 'remote_overwrite' | 'organize_mode'>>
export type SubscriptionUpdate = Required<SubscriptionInput>
export type ProcessedItem = { id: number; subscription_id: number; item_hash: string; title: string | null; processed_at: string | null }
export type DownloadTask = { id: number; subscription_id: number; item_hash: string; cloud_name: string | null; info_hash: string | null; remote_name: string | null; status: string | null; added_at: string | null; completed_at: string | null }
export type Connection = { id: number; kind: 'clouddrive' | 'webdav'; name: string; url: string; has_token: boolean; has_username: boolean; has_password: boolean; created_at: string; updated_at: string }
export type FolderEntry = { id: string; name: string; path: string; size: number; is_directory: boolean }

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(`/api/v1${path}`, { headers: { 'content-type': 'application/json' }, ...init })
  if (response.status === 204) return undefined as T
  const body = await response.json().catch(() => null)
  if (!response.ok) throw new ApiError(body?.error?.code || 'request_failed', body?.error?.message || `API request failed (${response.status})`, response.status)
  if (body === null) throw new ApiError('invalid_response', 'API returned an invalid response', response.status)
  return body as T
}

export const api = {
  health: () => request<{ status: string; version: string }>('/health'),
  status: () => request<Status>('/status'),
  capabilities: () => request<Capabilities>('/capabilities'),
  jobs: (query: { state?: JobState; kind?: string; limit?: number; before_id?: number } = {}) => request<{ jobs: Job[] }>(`/jobs?${new URLSearchParams(Object.entries(query).filter(([, value]) => value !== undefined).map(([key, value]) => [key, String(value)]))}`),
  job: (id: number) => request<Job>(`/jobs/${id}`),
  jobLogs: (id: number, afterId?: number) => request<{ logs: JobLog[] }>(`/jobs/${id}/logs?${new URLSearchParams(afterId === undefined ? {} : { after_id: String(afterId), limit: '5000' })}`),
  cancel: (id: number) => request<Job>(`/jobs/${id}`, { method: 'DELETE' }),
  retry: (id: number) => request<Job>(`/jobs/${id}/retry`, { method: 'POST' }),
  enqueueOrganize: (args: OrganizeArgs, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'organize', args } }) }),
  enqueueStorageOrganize: (args: StorageOrganizeArgs, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'storage_organize', args } }) }),
  enqueueNormalizeLayout: (args: NormalizeLayoutArgs, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'normalize_layout', args } }) }),
  enqueueCompactArtworkPacks: (args: CompactArtworkPacksArgs, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'compact_artwork_packs', args } }) }),
  enqueueScrape: (args: { days: number; format: 'json' | 'pretty'; tmdb_api_key?: string | null }) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', job: { type: 'scrape', args } }) }),
  enqueueMatchAliases: (args: { input: string; format: 'json' | 'github' }) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', job: { type: 'match_aliases', args } }) }),
  enqueueBuildBangumiDb: (args: { output: string; include_relations: boolean; verbose: boolean }, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'build_bangumi_db', args } }) }),
  enqueueExtractAliases: (args: { input: string | null; download: boolean; output: string | null }) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', job: { type: 'extract_aliases', args } }) }),
  enqueueMergeAliases: (args: { input: string; target: string | null }, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'merge_aliases', args } }) }),
  enqueueApplyMatches: (args: { input: string; target: string | null }, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'apply_matches', args } }) }),
  enqueueCreateAliasIssues: (args: { input: string; repo: string | null }, confirmed: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed, job: { type: 'create_alias_issues', args } }) }),
  enqueueTorrentScrape: (args: { source: 'dmhy' | 'nyaa' | 'all'; query: string | null; pages: number; output: string | null; headed: boolean }) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', job: { type: 'torrent_scrape', args } }) }),
  enqueueCloudAddOffline: (args: { connection_id: number; url: string; target: string }) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', job: { type: 'cloud_add_offline', args } }) }),
  subscriptions: () => request<{ subscriptions: Subscription[] }>('/rss/subscriptions'),
  createSubscription: (value: SubscriptionInput) => request<Subscription>('/rss/subscriptions', { method: 'POST', body: JSON.stringify({ auto_organize: false, organize_target_folder: null, organize_target_connection_id: null, organize_interval_secs: 300, organize_season_mode: true, remove_empty_dirs: true, remote_mlip: false, remote_overwrite: false, organize_mode: 'offline', ...value }) }),
  updateSubscription: (id: number, value: SubscriptionUpdate) => request<Subscription>(`/rss/subscriptions/${id}`, { method: 'PUT', body: JSON.stringify(value) }),
  deleteSubscription: (id: number) => request<void>(`/rss/subscriptions/${id}`, { method: 'DELETE' }),
  setEnabled: (id: number, enabled: boolean) => request<Subscription>(`/rss/subscriptions/${id}/${enabled ? 'enable' : 'disable'}`, { method: 'POST' }),
  runSubscription: (id: number) => request<{ job: { id: number } }>(`/rss/subscriptions/${id}/run`, { method: 'POST' }),
  runOrganization: (id: number) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', job: { type: 'remote_rss_organize', args: { subscription_id: id } } }) }),
  cleanupEmptyDirs: (id: number, dryRun: boolean) => request<{ job: Job; duplicate: boolean }>('/jobs', { method: 'POST', body: JSON.stringify({ origin: 'manual', confirmed: !dryRun, job: { type: 'cleanup_empty_dirs', args: { subscription_id: id, dry_run: dryRun } } }) }),
  runAll: () => request<{ job: { id: number } }>('/rss/run', { method: 'POST' }),
  processed: (id?: number) => request<{ items: ProcessedItem[] }>(`/rss/processed${id ? `?subscription_id=${id}` : ''}`),
  tasks: (id?: number) => request<{ tasks: DownloadTask[] }>(`/rss/download-tasks${id ? `?subscription_id=${id}` : ''}`),
  connections: () => request<{ connections: Connection[] }>('/cloud/connections'),
  saveConnection: (value: Record<string, unknown>, id?: number) => request<Connection>(id ? `/cloud/connections/${id}` : '/cloud/connections', { method: id ? 'PUT' : 'POST', body: JSON.stringify(value) }),
  patchCredentials: (id: number, value: Record<string, unknown>) => request<Connection>(`/cloud/connections/${id}`, { method: 'PATCH', body: JSON.stringify(value) }),
  deleteConnection: (id: number) => request<void>(`/cloud/connections/${id}`, { method: 'DELETE' }),
  testConnection: (id: number) => request<{ ok: boolean }>(`/cloud/connections/${id}/test`, { method: 'POST' }),
  listFolder: (id: number, path: string) => request<{ entries: FolderEntry[] }>(`/cloud/connections/${id}/list-folder`, { method: 'POST', body: JSON.stringify({ path }) }),
  metadataSettings: () => request<MetadataSettings>('/metadata/settings'),
  saveMetadataSettings: (value: MetadataSettings) => request<MetadataSettings>('/metadata/settings', { method: 'PUT', body: JSON.stringify(value) }),
  saveAppUpdateSettings: (value: AppUpdateSettings) => request<AppUpdateState>('/app-update/settings', { method: 'PUT', body: JSON.stringify(value) }),
  checkAppUpdate: () => request<{ ok: boolean }>('/app-update/check', { method: 'POST' }),
  applyAppUpdate: () => request<{ ok: boolean }>('/app-update/apply', { method: 'POST' }),
}
