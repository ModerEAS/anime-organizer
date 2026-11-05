# anime-organizer

[![CI](https://github.com/ModerRAS/anime-organizer/actions/workflows/ci.yml/badge.svg)](https://github.com/ModerRAS/anime-organizer/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/anime-organizer.svg)](https://crates.io/crates/anime-organizer)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)

轻量级、跨平台命令行工具，专为动漫收藏者批量整理视频文件，支持硬链接模式实现零额外空间占用。

[English](#english) | [中文](#中文)

---

## 中文

### 📖 项目描述

anime-organizer（简称 aniorg）是一款专为动漫收藏者设计的命令行工具，旨在自动化整理下载目录中的动漫视频文件。

**核心功能：**

- 自动解析文件名，识别动漫名称、集数、发布组等信息
- 支持硬链接模式，整理后不占用额外磁盘空间
- 自动生成 Kodi 兼容的 NFO 元数据文件
- 下载封面海报和背景图
- 生成 MiruPlay 可直接导入的 MLIP `library.db`，并补齐 Bangumi 元数据和海报
- 支持 RSS 订阅和 115 网盘同步（需启用 clouddrive feature）

**适用场景：**

- 整理下载目录中的动漫文件
- 自动化管理动漫媒体库
- 为 Kodi/Jellyfin/Plex 等媒体中心生成元数据

### 🚀 功能特性

- **智能解析**: 自动识别 `[发布组] 动漫名 - 集数 [标签].ext` 格式
- **灵活整理**: 重构为 `动漫名/Season N/集数 [标签].ext` 结构
- **多种模式**: 支持移动、复制、硬链接三种操作模式
- **元数据刮削**: 基于 Bangumi 生成 Kodi 兼容的 NFO 文件
- **封面下载**: 优先通过 Bangumi 下载海报，TMDB 作为备选并补充背景图，AniDB 作为最终海报兜底
- **别名匹配**: 支持本地 `bangumi.db` 别名库和自定义 JSON 覆盖，缺失时回退到 Bangumi 名称/搜索匹配
- **RSS 订阅**: 支持 RSS 自动订阅和 115 网盘同步（需 `clouddrive` feature）
- **跨平台**: 支持 Windows、Linux、macOS
- **便携部署**: 大多数平台为单文件；Windows x64 AniFileBERT 发布包需让 `DirectML.dll` 与 `aniorg.exe` 保持同目录
- **高性能**: Rust 原生实现，极速处理

### 📥 安装方式

#### 方式一：下载预编译二进制

从 [GitHub Releases](https://github.com/ModerRAS/anime-organizer/releases) 下载对应平台的二进制文件：

| 平台 | 文件 |
|------|------|
| Windows x64 | `aniorg-<version>-x86_64-pc-windows-msvc.zip` |
| Windows ARM64 | `aniorg-<version>-aarch64-pc-windows-msvc.zip` |
| Linux x64 | `aniorg-<version>-x86_64-unknown-linux-gnu.tar.gz` |
| Linux x64 (musl) | `aniorg-<version>-x86_64-unknown-linux-musl.tar.gz` |
| Linux ARM64 | `aniorg-<version>-aarch64-unknown-linux-gnu.tar.gz` |
| macOS x64 | `aniorg-<version>-x86_64-apple-darwin.tar.gz` |
| macOS ARM64 | `aniorg-<version>-aarch64-apple-darwin.tar.gz` |

Windows x64 压缩包中的 `aniorg.exe` 和 `DirectML.dll` 应一起解压。即使使用 CPU provider，也建议保留发布包的完整内容。

#### 方式二：从源码构建

```bash
git clone https://github.com/ModerRAS/anime-organizer.git
cd anime-organizer

# 默认构建（启用 metadata feature）
cargo build --release

# 全功能构建
cargo build --release --features "scraper clouddrive torrent-scraper"

# 启用嵌入式 AniFileBERT ONNX 文件名解析器（CPU）
cargo build --release --features "anifilebert"

# 启用 AniFileBERT + Windows DirectML（GPU；NPU 可显式选择）
cargo build --release --features "anifilebert-directml"
```

编译后的二进制文件位于 `target/release/aniorg`。`ANIORG_BERT_PROVIDER=cpu|auto|directml-gpu|directml-npu|directml-any` 控制 ONNX Runtime provider；`ANIORG_DIRECTML_DEVICE_ID=0` 可指定 DirectML 设备。未启用 `anifilebert-directml` 时默认 CPU；启用后默认 `auto`，会先尝试 DirectML GPU，失败则回退 CPU。CPU 线程数可用 `ANIORG_ORT_INTRA_THREADS` 和 `ANIORG_ORT_INTER_THREADS` 调整，默认均为 `1`。`anifilebert-amd-npu` 仍可用，是 `anifilebert-directml` 的兼容别名；VitisAI 需要额外的 ONNX Runtime provider 包，未放入默认构建。

如果本机默认是 GNU Rust 工具链，Windows 上验证 AniFileBERT/DirectML 时请显式指定 MSVC `rustc`。若 ONNX Runtime 在本地链接阶段报错，可将 `ORT_LIB_LOCATION` 指向解压后的 ORT 目录，并设置 `ORT_PREFER_DYNAMIC_LINK=1` 改走 import-lib/dll 路线。

### 程序自身更新

`aniorg update` 查询本仓库最新的 GitHub Release，只下载当前平台那一个压缩包和旁边的 `.sha256`，核对一致后再替换正在运行的程序，并按原来的参数重启。`aniorg update --check` 只查询，不下载。

比较时同时看 `MAJOR.MINOR.PATCH` 和 master 发布使用的 `+构建号`。两者都相同、并且本地和远端都能读到提交号时，提交号不同也算新构建。远端更旧时不会降级。校验、路径或可执行文件头不对时，不会覆盖当前程序。

`aniorg --daemon` 默认每 6 小时检查一次；间隔写成短于 5 分钟时按 5 分钟执行。发现新构建后会自动替换并重启。`target/debug` 和 `target/release` 里的开发构建只提示，不自动替换。关于页的「更新并重启」以及命令行的 `aniorg update` 仍然会替换。有整理任务在跑时，会等任务结束再重启。

设置写在数据目录的 `update.json`（和 `daemon.db` 同一层），关于页也可以改：

| 字段 | 默认 | 含义 |
| --- | --- | --- |
| `app_update_interval` | `6h` | `off` 关闭自动检查 |
| `auto_apply_app_update` | `true` | 发现新构建后自动替换 |
| `app_update_feed` | 空 | 空则使用 GitHub 最新 Release。只允许 https，或本机上的 http |
| `proxy` | 空 | 空则依次看 `ALL_PROXY`、`HTTPS_PROXY`、`HTTP_PROXY` 及对应小写变量；`off` 表示直连 |

访问本机回环地址不会送进代理。代理里的账号和密码不会出现在页面上。`ANIORG_GITHUB_TOKEN` 或 `GITHUB_TOKEN` 只用于请求 `api.github.com`，用来提高接口限额。

### Daemon 与 WebUI

Windows 使用以下唯一入口启动本地 daemon：

```powershell
aniorg.exe --daemon
```

WebUI 和 typed WebAPI 仅监听 `http://127.0.0.1:32145/`。有限任务由一个持久化 worker 串行执行；daemon 数据位于 `%LOCALAPPDATA%\anime-organizer\daemon.db`。CloudDrive token 以及 CloudDrive/WebDAV 用户名和密码依赖当前 Windows 用户对该目录的 ACL 保护，以明文存储在本地数据库中，但不会由 API 返回，也不会写入浏览器存储。v1 仅适用于受信任的本机单用户环境。

RSS 自动整理支持两种来源模式：`offline` 通过 BTIH 关联 CloudDrive 离线任务，`original` 直接扫描订阅配置的远端源目录，不依赖离线下载记录。RSS 下载连接必须是 CloudDrive；整理目标连接可独立选择 CloudDrive 或 WebDAV。启用“远端 MLIP”时，daemon 只在 hash 缓存未命中时把原始媒体下载到系统临时文件，校验大小并在本地计算完整 SHA-256；随后下载目标根目录的 `library.db` 执行 SQLite 增量事务，再通过远端临时文件、备份重命名和 SHA-256 校验发布，最后才移动媒体。关闭“远端 MLIP”的原始模式不会 hash 媒体；同连接使用原生 `CopyFile`，跨连接使用经过大小和 SHA-256 验证的流式传输。MLIP 发布失败时不会移动源 bundle、完成 RSS 任务或删除源目录。

通用 `storage_organize` typed job 支持本地目录、CloudDrive 连接和 WebDAV 连接之间的 `copy`/`move` 组合。相同远端连接优先使用后端原生 `COPY`/`MOVE`；跨端点先下载到 daemon 临时文件，再以隐藏临时名上传，按大小和 SHA-256 校验，重命名为最终文件，最后才删除源。WebDAV 连接支持标准 `PROPFIND/MKCOL/GET/PUT/COPY/MOVE/DELETE` 和可选 Basic Auth，不提供 magnet/offline 下载。硬链接仍由原本地 `organize` job 处理。

```json
{
  "origin": "manual",
  "confirmed": true,
  "job": {
    "type": "storage_organize",
    "args": {
      "source": { "type": "local", "path": "C:\\Downloads\\Anime" },
      "target": { "type": "connection", "connection_id": 2, "path": "/Anime" },
      "mode": "move",
      "season_mode": true,
      "mlip": true,
      "remove_empty_dirs": true
    }
  }
}
```

本地 `move` 成功后默认从最深层开始清理本次文件涉及的空源目录，保留源根目录及任何仍含文件的目录。新 RSS 订阅默认删除已完成离线任务留下的空根目录。订阅详情页还提供“预览空目录”和“清理空目录”：手动清理会递归处理该订阅源目录下所有空目录，但保留订阅根目录；只要 CloudDrive 仍报告 `init` 或 `downloading` 离线任务，daemon 就拒绝开始清理。

### 🎯 快速开始

#### 基本用法

```bash
# 硬链接模式（默认，零额外空间）
aniorg --source="/path/to/downloads"

# 复制模式
aniorg --source="/path/to/downloads" --mode=copy --target="/path/to/anime"

# 启用硬链接失败回退为复制
aniorg --source="/path/to/downloads" --fallback-on-link-failure=copy

# 启用分季目录结构
aniorg --source="/path/to/downloads" --season-mode --target="/path/to/anime"

# 生成 NFO 和海报
aniorg --source="/path/to/downloads" --scrape-metadata

# 生成/更新目标目录根部的 MLIP 媒体库索引
aniorg --source="/path/to/downloads" --target="/path/to/anime" --library-index

# 生成 MiruPlay 可直接导入的 MLIP 媒体库（含 Bangumi 元数据和海报）
aniorg --source="/path/to/downloads" --target="/path/to/anime" --mlip

# 只重建文件索引，不联网、不下载图片
aniorg --source="/path/to/empty" --target="/path/to/anime" --library-index --rebuild-library-index

# 重建 MiruPlay 成品库，并补齐 Bangumi 元数据和缺失海报
aniorg --source="/path/to/empty" --target="/path/to/anime" --mlip --rebuild-library-index

# 只读取现有 library.db 重新刮削元数据；不扫描媒体目录
aniorg --target="/path/to/anime" --mlip --refresh-library-metadata

# 只读生成目录统一/去重计划；不会移动或删除媒体
aniorg normalize-layout --target="/path/to/anime" --dry-run --plan="layout-plan.json"

# 只读验证 artwork catalog 并生成压实计划
aniorg compact-artwork-packs --target="/path/to/anime" --dry-run --plan="artwork-compact-plan.json"
```

`normalize-layout --apply-plan` 和 `compact-artwork-packs --apply-plan` 只能作为 daemon typed job 执行，并要求请求顶层 `confirmed=true`。直接 CLI apply 会拒绝执行。请先审核 JSON plan 中的 conflict、unresolved、删除数量和预计释放空间；生成 layout plan 只枚举路径、文件名、大小/时间和 `library.db` 缓存，不读取媒体或 sidecar 内容，也不计算新 hash。

#### 预览模式

在实际操作前先预览变更：

```bash
aniorg --source="/path/to/downloads" --dry-run --verbose
```

### 📋 参数说明

| 参数 | 缩写 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|------|--------|------|
| `--source` | `-s` | string | ✅ | - | 源目录路径 |
| `--target` | `-t` | string | ❌ | source | 目标根目录 |
| `--mode` | `-m` | enum | ❌ | link | 操作模式：move/copy/link |
| `--dry-run` | | bool | ❌ | false | 仅预览不执行 |
| `--include-ext` | | string | ❌ | mp4,mkv,... | 处理的扩展名（逗号分隔） |
| `--filename-parser` | | enum | ❌ | rules | 文件名解析器：`rules`、`anifilebert`、`auto`（规则失败后回退 AniFileBERT） |
| `--verbose` | `-v` | bool | ❌ | false | 显示详细日志 |
| `--fallback-on-link-failure` | | enum | ❌ | - | 硬链接失败时回退模式：move 或 copy（默认不回退） |
| `--season-mode` / `--分季` | | bool | ❌ | false | 按 `番名/Season N/` 结构整理多季作品 |
| `--scrape-metadata` / `--刮削` | | bool | ❌ | false | 启用 Bangumi 元数据刮削（TMDB 可选补图） |
| `--tmdb-api-key` | | string | ❌ | - | TMDB API Key，用于备选海报和背景图 |
| `--alias-file` | | string | ❌ | - | 自定义别名 JSON 文件，覆盖本地 `bangumi.db` 中的同名项 |
| `--no-images` | | bool | ❌ | false | 跳过 Kodi/MLIP 图片下载；仍写入可用的文本元数据 |
| `--no-episode-metadata` | | bool | ❌ | false | 跳过 Bangumi 分集标题、简介和时长查询 |
| `--force-overwrite` | | bool | ❌ | false | 覆盖已有的 NFO 和图片文件 |
| `--bangumi-cache` | | string | ❌ | 系统缓存目录 | Bangumi Archive 和 AnimeAtlas SQLite 缓存目录；默认不写入媒体 target |
| `--metadata-source` | | string | ❌ | - | 指定本地 `subject.jsonlines` 或其目录；显式指定后保持离线，不自动刷新 AnimeAtlas |
| `--library-index` | | bool | ❌ | false | 生成/更新目标目录根部的 `library.db` |
| `--mlip` | | bool | ❌ | false | 生成 MiruPlay 成品库：`library.db`、Bangumi 元数据和缺失海报；不生成 Kodi NFO |
| `--rebuild-library-index` | | bool | ❌ | false | 与 `--library-index` 或 `--mlip` 合用，全量重扫 target；后者还会补元数据和缺失图片 |
| `--refresh-library-metadata` | | bool | ❌ | false | 与 `--mlip` 合用，只读取现有 `library.db` 重新匹配元数据；不扫描媒体目录，新图片会写入 artwork tar |
| `--probe-runtime` | | bool | ❌ | false | 使用 `ffprobe` 探测视频时长并写入 `episode.runtime`（秒） |
| `--help` | `-h` | bool | ❌ | false | 显示帮助 |
| `--version` | `-V` | bool | ❌ | false | 显示版本 |

### 🧾 元数据刮削

启用 `--scrape-metadata` 后，程序会：

- 优先使用 `--alias-file` JSON 和本地别名库匹配 Bangumi 条目；若未提供本地 `animeatlas.sqlite`/`bangumi.db`，会从 AnimeAtlas 固定发布地址下载 `animeatlas.sqlite` 到缓存目录，失败时回退到现有缓存或 Bangumi 名称/搜索匹配
- 在动画根目录生成 `tvshow.nfo`
- 在 `Season N/` 目录下生成与视频同名的 `*.nfo`
- 优先从 Bangumi 下载 `poster.jpg` 和 `seasonXX-poster.jpg`；如果提供了 TMDB API Key，则 TMDB 作为海报备选并补充 `fanart.jpg`

默认缓存位于系统缓存目录：Windows 为 `%LOCALAPPDATA%\anime-organizer\bangumi-cache`（缺失时回退 `%APPDATA%`），Linux/macOS 优先 `$XDG_CACHE_HOME/anime-organizer/bangumi-cache`，否则使用 `~/.cache/anime-organizer/bangumi-cache`。`--bangumi-cache` 可显式覆盖。缓存不会默认放进媒体 target。

未指定 `--metadata-source` 时，元数据模式会按 24 小时 TTL 刷新固定发布地址的 AnimeAtlas `animeatlas.sqlite`；daemon 还会每小时检查一次缓存，过期后自动刷新。刷新失败会继续使用现有缓存。显式指定 `--metadata-source` 表示使用本地数据并保持离线，不会触发 AnimeAtlas 自动下载；`--alias-file` JSON 仍具有最高优先级。

别名匹配优先使用本仓库每周日构建的全量库 `bangumi.db`（3 万余部动画，含 infobox 的「中文名 / 别名 / 英文名」，约 120 MB），缓存 7 天后自动更新；AnimeAtlas 只在它不可用时兜底。不想自动下载可以在 WebUI 的「关于 → 元数据数据源」关闭（写入数据目录的 `metadata.json`），关闭后只使用本地已有的 `bangumi.db`。

`--alias-file` 可直接复用 `extract-aliases` 导出的 JSON 对象格式，例如：

```json
{
  "芙莉莲": {
    "bangumi_id": 444444,
    "name": "葬送のフリーレン",
    "tmdb_id": 209867,
    "anidb_id": 18597
  }
}
```

### 🗃️ MLIP 媒体库索引

启用 `--library-index` 后，程序会固定管理目标目录根部的 `library.db`：

- 如果 `library.db` 不存在：整理完成后扫描整个 target，初始化建库。
- 如果 `library.db` 已存在：默认只把本次整理成功的新文件增量更新进去。
- 如果同时传入 `--rebuild-library-index`：整理完成后重新扫描整个 target 并重建数据库。
- `--dry-run` 下只打印将执行的模式和统计，不创建或修改数据库。

`--library-index` 只写文件索引，不联网、不下载图片。给 MiruPlay 使用时推荐 `--mlip`：它会生成 `library.db`，写入 Bangumi 作品与分集元数据，并把缺失的 `poster.jpg` 和 `seasonXX-poster.jpg` 下载到对应媒体目录。已有图片默认保留；`--force-overwrite` 才会重新下载覆盖，`--no-images` 则完全跳过图片下载。Kodi NFO 仍只由 `--scrape-metadata` 生成。

如果提供 TMDB API Key，TMDB 会作为海报备选来源并补充 `fanart.jpg`。Bangumi 分集数据有 `duration_seconds` 时会写入 `episode.runtime`；如需以本地视频为准，可额外传入 `--probe-runtime` 使用 `ffprobe` 探测实际时长。默认不会探测视频，避免对 NAS、WebDAV 或云盘逐文件读取。

全量扫描已整理的 target 时，直接父目录 `Season N` 是季号的权威来源。根目录直放的 S1 文件、`Season 1` 子目录和后续 `Season N` 可以在同一媒体库中共存；目录季号与文件名季号冲突时以目录为准。图片路径始终相对实际媒体目录写入，不会按解析后的别名另建目录。

本数据库是只读协议。任何播放器不得修改本数据库；播放历史、收藏、继续播放等数据必须保存在播放器自己的数据库。

#### 使用方式

首次启用时，`library.db` 不存在，程序会先完成本次整理，再扫描整个 target 目录建立索引：

```bash
aniorg \
  --source="/path/to/downloads" \
  --target="/path/to/anime" \
  --library-index
```

生成结果固定放在目标目录根部：

```text
/path/to/anime/
├── library.db
├── Bocchi the Rock!/
│   └── 01 [1080P].mkv
└── Spy x Family/
    └── Season 2/
        └── 01 [1080P].mkv
```

以后继续整理新下载时，仍然使用同一个命令。只要 `library.db` 已存在，默认就是增量更新：只把本次整理成功的文件写入索引，不会重新扫描整个目标目录。为兼容 NAS、WebDAV 和 CloudDrive，程序会先把 `library.db` 复制到系统临时目录完成 SQLite 事务，再顺序上传并替换远程数据库，避免在远程文件上随机写入。

```bash
aniorg \
  --source="/path/to/new-downloads" \
  --target="/path/to/anime" \
  --library-index
```

如果你手动往目标目录添加、删除、移动了文件，或者想重新生成整个索引，使用 `--rebuild-library-index`。如果只想扫描现有 target 而不整理任何新文件，让 `--source` 指向一个存在的空目录：

```bash
# 裸索引：不联网、不下载图片
aniorg \
  --source="/path/to/empty" \
  --target="/path/to/anime" \
  --library-index \
  --rebuild-library-index

# MiruPlay 成品库：补元数据和缺失图片
aniorg \
  --source="/path/to/empty" \
  --target="/path/to/anime" \
  --mlip \
  --rebuild-library-index
```

`--source` 目录必须存在。空源不会移动、复制或硬链接媒体，只会扫描 target。`--rebuild-library-index` 必须和 `--library-index` 或 `--mlip` 一起使用；如果 source 不是空目录，本次成功整理的新文件也会包含在重建结果中。

只需要按新别名或供应商数据重新刮削现有索引时，使用 `--refresh-library-metadata`。它不需要 `--source`，只从 `library.db` 读取媒体路径、集号、大小、mtime、hash 和 artwork 绑定，不递归枚举 target，也不读取视频。该模式会强制刷新 AnimeAtlas 缓存，重新匹配 Bangumi/分集元数据；缺失或被 `--force-overwrite` 覆盖的图片按明确路径下载，并由现有 artwork pack builder 写入 tar/catalog。daemon `organize` job 需要顶层 `confirmed=true`。

```bash
aniorg \
  --target="/path/to/anime" \
  --mlip \
  --refresh-library-metadata
```

给 MiruPlay 增量生成成品库时，使用 `--mlip`；它默认下载缺失图片，但不会生成 Kodi NFO：

```bash
aniorg \
  --source="/path/to/downloads" \
  --target="/path/to/anime" \
  --mlip
```

如果还想补充 TMDB 背景图，可以额外传入 `--tmdb-api-key`。

如果同时想生成 Kodi NFO、图片和 MLIP 索引，可以一起开启元数据刮削：

```bash
aniorg \
  --source="/path/to/downloads" \
  --target="/path/to/anime" \
  --scrape-metadata \
  --tmdb-api-key="YOUR_TMDB_KEY" \
  --library-index
```

预览时可以组合 `--dry-run`。此时不会创建或修改 `library.db`，只会打印将执行初始化、增量更新还是重建：

```bash
aniorg \
  --source="/path/to/downloads" \
  --target="/path/to/anime" \
  --library-index \
  --dry-run \
  --verbose
```

如果不传 `--target`，目标目录沿用现有规则等于 source，所以索引会写到 `<source>/library.db`。播放器读取时应把 `media_file.path` 当作相对 `library.db` 所在目录的路径处理，路径分隔符统一为 `/`。

```sql
-- ============================================================
-- Media Library Index Protocol (MLIP)
--
-- Version : 1
-- Storage : SQLite 3
--
-- 设计原则：
-- 1. 不扫描目录即可构建媒体库
-- 2. 不依赖在线刮削
-- 3. 支持长期扩展
-- 4. 所有数据均可由整理器重新生成
-- 5. 不使用 polymorphic association / JSON / EAV
-- ============================================================

PRAGMA foreign_keys = ON;

CREATE TABLE meta
(
    key     TEXT PRIMARY KEY,
    value   TEXT NOT NULL
);

-- meta keys:
-- schema = 3
-- protocol = MLIP
-- generator = AnimeOrganizer
-- generator_version = <cargo package version>
-- library_uuid = <uuid>
-- library_root = <canonical target path>
-- generated_at = <RFC3339 timestamp>

CREATE TABLE series
(
    id              INTEGER PRIMARY KEY,
    uuid            TEXT UNIQUE NOT NULL,
    title           TEXT NOT NULL,
    original_title  TEXT,
    sort_title      TEXT,
    summary         TEXT,
    year            INTEGER,

    -- series_type:
    -- 1 TV
    -- 2 Movie
    -- 3 OVA
    -- 4 ONA
    -- 5 SP
    series_type     INTEGER NOT NULL DEFAULT 1
);

CREATE INDEX idx_series_title ON series(title);

CREATE TABLE series_release_date
(
    series_id   INTEGER PRIMARY KEY,
    air_date    TEXT NOT NULL, -- YYYY-MM-DD when known

    FOREIGN KEY(series_id)
        REFERENCES series(id)
        ON DELETE CASCADE
);

CREATE TABLE episode
(
    id          INTEGER PRIMARY KEY,
    uuid        TEXT UNIQUE NOT NULL,
    series_id   INTEGER NOT NULL,
    season      INTEGER NOT NULL DEFAULT 1,
    episode     REAL NOT NULL,
    sort_order  REAL NOT NULL,
    title       TEXT,
    summary     TEXT,
    runtime     INTEGER, -- seconds

    FOREIGN KEY(series_id)
        REFERENCES series(id)
        ON DELETE CASCADE,

    UNIQUE(series_id, season, episode)
);

CREATE INDEX idx_episode_series ON episode(series_id);

CREATE TABLE media_file
(
    id              INTEGER PRIMARY KEY,
    episode_id      INTEGER NOT NULL,
    path            TEXT NOT NULL UNIQUE,
    size            INTEGER,
    modified_time   INTEGER,

    FOREIGN KEY(episode_id)
        REFERENCES episode(id)
        ON DELETE CASCADE
);

CREATE INDEX idx_media_path ON media_file(path);
CREATE INDEX idx_media_episode ON media_file(episode_id);

-- Required in MLIP v2. Each subtitle belongs to one concrete media file.
CREATE TABLE media_subtitle
(
    id              INTEGER PRIMARY KEY,
    media_file_id   INTEGER NOT NULL,
    path            TEXT NOT NULL,
    language        TEXT,
    title           TEXT,
    sort_order      INTEGER NOT NULL DEFAULT 0,

    FOREIGN KEY(media_file_id)
        REFERENCES media_file(id)
        ON DELETE CASCADE,

    UNIQUE(media_file_id, path)
);

CREATE INDEX idx_media_subtitle_file ON media_subtitle(media_file_id);

-- Required in MLIP v3. Extras belong directly to a series, never to a fake episode.
CREATE TABLE media_extra
(
    id              INTEGER PRIMARY KEY,
    uuid            TEXT UNIQUE NOT NULL,
    series_id       INTEGER NOT NULL,
    extra_kind      INTEGER NOT NULL, -- 1 OVA, 2 special, 3 NCOP, 4 NCED, 5 gallery
    ordinal         INTEGER NOT NULL,
    sort_order      INTEGER NOT NULL,
    title           TEXT NOT NULL,
    path            TEXT NOT NULL UNIQUE,
    size            INTEGER,
    modified_time   INTEGER,
    runtime         INTEGER, -- seconds

    FOREIGN KEY(series_id)
        REFERENCES series(id)
        ON DELETE CASCADE
);

CREATE INDEX idx_media_extra_series ON media_extra(series_id, extra_kind, sort_order);

CREATE TABLE series_artwork
(
    id              INTEGER PRIMARY KEY,
    series_id       INTEGER NOT NULL,

    -- artwork_kind:
    -- 1 poster
    -- 2 fanart
    -- 3 banner
    -- 4 logo
    -- 5 thumb
    -- 6 clearart
    -- 7 season_poster
    artwork_kind    INTEGER NOT NULL,
    path            TEXT NOT NULL,

    FOREIGN KEY(series_id)
        REFERENCES series(id)
        ON DELETE CASCADE,

    UNIQUE(series_id, artwork_kind, path)
);

CREATE INDEX idx_series_artwork_series ON series_artwork(series_id);

CREATE TABLE episode_artwork
(
    id              INTEGER PRIMARY KEY,
    episode_id      INTEGER NOT NULL,
    artwork_kind    INTEGER NOT NULL,
    path            TEXT NOT NULL,

    FOREIGN KEY(episode_id)
        REFERENCES episode(id)
        ON DELETE CASCADE,

    UNIQUE(episode_id, artwork_kind, path)
);

CREATE INDEX idx_episode_artwork_episode ON episode_artwork(episode_id);

CREATE TABLE genre
(
    id      INTEGER PRIMARY KEY,
    name    TEXT UNIQUE NOT NULL
);

CREATE TABLE series_genre
(
    series_id   INTEGER NOT NULL,
    genre_id    INTEGER NOT NULL,

    PRIMARY KEY(series_id, genre_id),

    FOREIGN KEY(series_id)
        REFERENCES series(id)
        ON DELETE CASCADE,

    FOREIGN KEY(genre_id)
        REFERENCES genre(id)
        ON DELETE CASCADE
);

CREATE TABLE series_external_id
(
    series_id   INTEGER NOT NULL,

    -- provider:
    -- 1 bangumi
    -- 2 tmdb
    -- 3 anidb
    provider    INTEGER NOT NULL,
    value       TEXT NOT NULL,

    PRIMARY KEY(series_id, provider, value),

    FOREIGN KEY(series_id)
        REFERENCES series(id)
        ON DELETE CASCADE
);

CREATE TABLE episode_external_id
(
    episode_id  INTEGER NOT NULL,
    provider    INTEGER NOT NULL,
    value       TEXT NOT NULL,

    PRIMARY KEY(episode_id, provider, value),

    FOREIGN KEY(episode_id)
        REFERENCES episode(id)
        ON DELETE CASCADE
);

CREATE TABLE capability
(
    name        TEXT PRIMARY KEY,
    enabled     INTEGER NOT NULL
);

-- v3 capabilities:
-- artwork = 1
-- genre = 1
-- external_id = 1
-- release_date = 1
-- people = 0
-- subtitle = 1
-- extra = 1
-- media_technical = 0
-- multi_file = 1

PRAGMA user_version = 3;
```

v2 新增必需表 `media_subtitle`，并将 `subtitle` capability 设为 `1`。每条字幕关联到具体 `media_file`；整理器会同步处理与视频同目录、同 stem 或带语言后缀的 `.srt`、`.ass`、`.ssa`、`.vtt` 文件。`.sub` 因需要配对 `.idx`，暂不支持。

v3 新增必需表 `media_extra` 和 `extra` capability。完整目标扫描会把 OVA、`NCOP`、`NCED`、`Tokuten` 和 `Images` 文件作为所属作品的本地特典编目；BD `menu` 文件仍会忽略。特典不创建伪造的 episode，也不参与正片集数。对既有 v1/v2 数据库执行写入时会自动升级为 v3。

MLIP v4 是当前 writer 的默认输出。它新增 `artwork_pack` 与 `artwork_asset`，将可验证的原始图片字节保存到不可变、未压缩的 tar shards；`series_artwork` 和 `episode_artwork` 保留相对 `path` fallback，并可引用 `asset_id` 及 binding 级来源信息。writer 会先发布并校验 pack，再发布引用它们的 `library.db`。完整重建会生成 v4；已审核且不希望重新解析标题/元数据的 v3 库可用以下维护工具原样升级：

```bash
cargo run --example migrate_mlip_v3_to_v4 -- "/path/to/anime"
```

### 🎨 文件命名格式

#### 支持的源文件名格式

```
[发布组] 动漫名称（可含季度） - 集数 [标签信息].扩展名
```

示例：
- `[ANi] 妖怪旅館營業中 貳 - 07 [1080P][Baha][WEB-DL][AAC AVC][CHT].mp4`
- `[SubsPlease] 间谍过家家 - 12 [1080p].mkv`
- `[EMBER] 进击的巨人 The Final Season - 01 [1080p][Multiple Subtitle].avi`

#### 目标文件结构

```
动漫名称（含季度）/
├── Season 1/
│   ├── 01 [标签信息].扩展名
│   └── 02 [标签信息].扩展名
├── Season 2/
│   ├── 01 [标签信息].扩展名
│   └── ...
├── tvshow.nfo
├── poster.jpg
└── fanart.jpg
```

### 🔗 硬链接说明

硬链接是推荐的整理方式，具有以下优势：

- **零额外空间**: 不占用额外磁盘空间
- **快速操作**: 几乎瞬间完成
- **文件同步**: 源文件和目标文件内容完全同步

#### 使用条件

1. **同一文件系统**: 源文件和目标必须在同一分区/NAS卷
2. **文件系统支持**: ext4、NTFS、APFS 等均支持
3. **权限要求**: 需要对源和目标目录有写入权限

#### 跨设备错误

如果源文件和目标不在同一文件系统，会显示错误：
```
硬链接失败：源文件和目标必须在同一文件系统
```

此时可选择：
- 将目标目录改为与源文件同一文件系统
- 使用复制模式 (`--mode=copy`)，或通过 `--fallback-on-link-failure=copy` 自动回退
- 使用移动模式 (`--mode=move`)，或通过 `--fallback-on-link-failure=move` 自动回退

### 🔧 刮削子命令（需 `--features scraper`）

```bash
# 刮削近期更新
cargo run --features scraper -- scrape --days 7 --format json

# 匹配别名提案
cargo run --features scraper -- match --input scraped.json --format github

# 构建 SQLite 别名库
cargo run --features scraper -- build-db --output bangumi.db

# 从 dump 提取别名
cargo run --features scraper -- extract-aliases --download

# 合并新别名到数据库
cargo run --features scraper -- merge-aliases --input new_aliases.json

# 应用匹配的别名提案
cargo run --features scraper -- apply-matches --input proposals.json

# 创建别名请求 issue
cargo run --features scraper -- create-alias-issues --input uncertain.json --repo ModerRAS/anime-organizer
```

### 📡 RSS 订阅管理（需 `--features clouddrive`）

```bash
# 列出订阅
aniorg rss --list-subscriptions

# 添加订阅
aniorg rss --add-subscription \
  --rss-url "https://example.com/rss" \
  --rss-target "/anime" \
  --rss-filter "720p"

# 单次执行
aniorg rss --single-shot \
  --clouddrive-url http://localhost:19798 \
  --rss-url "https://example.com/rss" \
  --rss-target "/anime"

# Daemon 模式（持续监控）
aniorg rss --daemon \
  --clouddrive-url http://localhost:19798 \
  --rss-interval 300
```

认证支持两种方式：直接传 `--clouddrive-token`，或使用 `--clouddrive-user` + `--clouddrive-pass` 登录获取令牌。订阅和去重状态默认保存在 Windows 的 `%LOCALAPPDATA%\anime-organizer\rss.db`。

### ☁️ CloudDrive 辅助命令（需 `--features clouddrive`）

```bash
# 直接提交 magnet 或 torrent URL 到离线下载
aniorg add-offline "magnet:?xt=urn:btih:..." \
  --target "/anime" \
  --clouddrive-url http://localhost:19798 \
  --clouddrive-token "YOUR_TOKEN"

# 浏览云盘目录
aniorg list-folder "/anime" \
  --clouddrive-url http://localhost:19798 \
  --clouddrive-token "YOUR_TOKEN"
```

### 💡 使用示例

```bash
# 整理下载目录
aniorg --source="D:\Downloads\Anime"

# 整理到指定目录
aniorg -s "/home/user/Downloads" -t "/media/anime"

# 硬链接模式
aniorg -s "/home/user/Downloads" -m link -t "/mnt/anime"

# 多季作品按 Season 目录整理
aniorg -s "/path/to/downloads" -t "/path/to/anime" --season-mode

# 预览变更并显示详细日志
aniorg -s "/path/to/downloads" --dry-run -v

# 指定文件类型
aniorg -s "/path/to/downloads" --include-ext="mp4,mkv"

# 使用自定义别名库
aniorg -s "/path/to/downloads" --alias-file="./my-aliases.json"

# 强制覆盖已有 NFO
aniorg -s "/path/to/downloads" --scrape-metadata --force-overwrite
```

---

## English

### 📖 Project Description

anime-organizer (aniorg) is a command-line tool designed for anime collectors to automatically organize video files in download directories.

**Core Features:**

- Auto-parse filenames to extract anime name, episode number, publisher info
- Support hard link mode - organize files without using extra disk space
- Generate Kodi-compatible NFO metadata files
- Generate a MiruPlay-ready MLIP `library.db` with Bangumi metadata and artwork
- Download poster and fanart images
- RSS subscription and 115 cloud drive sync (requires clouddrive feature)

**Use Cases:**

- Organize anime files in download directories
- Automate anime media library management
- Generate metadata for Kodi/Jellyfin/Plex media centers

### 🚀 Features

- **Smart Parsing**: Auto-recognize `[Publisher] AnimeName - Episode [Tags].ext` format
- **Flexible Organization**: Restructure to `AnimeName/Season N/Episode [Tags].ext`
- **Multiple Modes**: Support move, copy, and hard link operations
- **Metadata Scraping**: Generate Kodi-compatible NFO files from Bangumi metadata
- **Artwork Download**: Download anime posters from Bangumi first, use TMDB as a backup and fanart source, then AniDB as the final poster fallback
- **Alias Matching**: Local `bangumi.db` aliases plus optional custom JSON overrides, with Bangumi title/search fallback
- **RSS Subscription**: RSS auto-subscription and 115 cloud drive sync (requires `clouddrive` feature)
- **Cross-Platform**: Support Windows, Linux, macOS
- **Portable Deployment**: Single-file on most platforms; keep `DirectML.dll` beside `aniorg.exe` from the Windows x64 AniFileBERT release
- **High Performance**: Native Rust implementation

### 📥 Installation

#### Option 1: Download Pre-built Binary

Download from [GitHub Releases](https://github.com/ModerRAS/anime-organizer/releases). Windows x64 users should extract both `aniorg.exe` and `DirectML.dll` from the versioned ZIP and keep them in the same directory.

#### Option 2: Build from Source

```bash
git clone https://github.com/ModerRAS/anime-organizer.git
cd anime-organizer

# Default build (metadata feature enabled)
cargo build --release

# Full features
cargo build --release --features "scraper clouddrive torrent-scraper"

# Embedded AniFileBERT ONNX filename parser (CPU)
cargo build --release --features "anifilebert"

# AniFileBERT plus Windows DirectML (GPU; NPU can be selected explicitly)
cargo build --release --features "anifilebert-directml"
```

The compiled binary is located at `target/release/aniorg`. `ANIORG_BERT_PROVIDER=cpu|auto|directml-gpu|directml-npu|directml-any` controls the ONNX Runtime provider; `ANIORG_DIRECTML_DEVICE_ID=0` pins a DirectML device. Without `anifilebert-directml`, the parser uses CPU. With it, the default is `auto`: try DirectML GPU first, then fall back to CPU. CPU threads can be tuned with `ANIORG_ORT_INTRA_THREADS` and `ANIORG_ORT_INTER_THREADS`; both default to `1`. `anifilebert-amd-npu` remains as a compatibility alias for `anifilebert-directml`; VitisAI requires an additional ONNX Runtime provider package and is not part of the default build.

If your local Windows machine defaults to the GNU Rust toolchain, validate AniFileBERT/DirectML builds with the MSVC `rustc` explicitly. If ONNX Runtime fails during the local link step, point `ORT_LIB_LOCATION` at the extracted ORT directory and set `ORT_PREFER_DYNAMIC_LINK=1` to use the import-lib/dll path.

### Self-update

`aniorg update` checks the latest GitHub Release, downloads only the archive for the current target plus its `.sha256`, verifies the hash, then replaces the running program and restarts it with the original arguments. `aniorg update --check` reports availability and does not download.

Comparison uses `MAJOR.MINOR.PATCH` and the `+BUILD` number published from `master`. When both the version and build number match, and both sides have a commit id, a different commit is also an update. Older releases are not installed. A failed checksum, path check, or executable header leaves the current program in place.

`aniorg --daemon` checks every 6 hours by default. Intervals shorter than 5 minutes run every 5 minutes. A newer build is downloaded and the daemon restarts itself. Binaries under `target/debug` or `target/release` only show a notice. The About page action “Update and restart” and `aniorg update` still replace those builds. A running organize job delays the restart until it finishes.

Settings live in `update.json` beside `daemon.db` and can also be edited on the About page:

| Field | Default | Meaning |
| --- | --- | --- |
| `app_update_interval` | `6h` | `off` disables the schedule |
| `auto_apply_app_update` | `true` | replace the program when a newer build is found |
| `app_update_feed` | empty | empty uses the GitHub latest release. Only `https`, or `http` on loopback, is accepted |
| `proxy` | empty | empty uses the first of `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` and their lowercase forms; `off` forces a direct connection |

Loopback requests bypass the proxy. Proxy usernames and passwords are not shown in the page. `ANIORG_GITHUB_TOKEN` or `GITHUB_TOKEN` is sent only to `api.github.com`.

### Daemon and WebUI

On Windows, start the local daemon through its single entry point:

```powershell
aniorg.exe --daemon
```

The WebUI and typed WebAPI listen only on `http://127.0.0.1:32145/`. One durable worker executes finite jobs serially, with daemon state stored in `%LOCALAPPDATA%\anime-organizer\daemon.db`. CloudDrive tokens and CloudDrive/WebDAV usernames and passwords rely on the current Windows user's ACL and are stored as plaintext in that local database; they are never returned by the API or stored in the browser. v1 is intended only for a trusted, single-user local machine.

RSS automatic organization keeps CloudDrive as the download/offline-task connection, while its organization target may independently use CloudDrive or WebDAV. Remote MLIP publication downloads uncached source media to a daemon temporary file for size and SHA-256 verification, updates `library.db` locally, then publishes it through a staged remote file, backup rename, and hash verification. Publication completes before any source mutation.

The `storage_organize` typed job supports `copy` and `move` between local directories, CloudDrive connections, and WebDAV connections. Same-connection operations prefer native `COPY`/`MOVE`; cross-endpoint moves stage, upload, verify size and SHA-256, publish final names, and only then delete source files. WebDAV supports standard file operations with optional Basic Auth but not magnet/offline download. Local hard links remain part of the existing `organize` job.

After a successful local `move`, empty source ancestors involved in that move are removed deepest-first while the source root and every non-empty directory are retained. New RSS subscriptions remove completed offline-task roots by default. The subscription detail page also provides preview and apply actions for recursively cleaning all empty directories below that subscription's source root. The root itself is retained, and cleanup is rejected while CloudDrive reports any `init` or `downloading` offline task.

### 🎯 Quick Start

```bash
# Hard link mode (default, zero extra space)
aniorg --source="/path/to/downloads"

# Preview mode
aniorg --source="/path/to/downloads" --dry-run --verbose

# Enable automatic fallback to copy when hard link fails
aniorg --source="/path/to/downloads" --fallback-on-link-failure=copy

# Organize multi-season releases into Season folders
aniorg --source="/path/to/downloads" --season-mode --target="/path/to/anime"

# Generate NFO files and artwork
aniorg --source="/path/to/downloads" --scrape-metadata

# Generate or update the MLIP library index in the target root
aniorg --source="/path/to/downloads" --target="/path/to/anime" --library-index

# Generate a MiruPlay-ready MLIP library with Bangumi metadata and posters
aniorg --source="/path/to/downloads" --target="/path/to/anime" --mlip

# Rebuild only the offline file index
aniorg --source="/path/to/empty" --target="/path/to/anime" --library-index --rebuild-library-index

# Rebuild a MiruPlay-ready library and download missing artwork
aniorg --source="/path/to/empty" --target="/path/to/anime" --mlip --rebuild-library-index

# Re-scrape metadata from the existing library.db without scanning media
aniorg --target="/path/to/anime" --mlip --refresh-library-metadata
```

### 📋 Arguments

| Argument | Short | Type | Required | Default | Description |
|----------|-------|------|----------|---------|-------------|
| `--source` | `-s` | string | ✅ | - | Source directory path |
| `--target` | `-t` | string | ❌ | source | Target root directory |
| `--mode` | `-m` | enum | ❌ | link | Operation mode: move/copy/link |
| `--dry-run` | | bool | ❌ | false | Preview only, no actual changes |
| `--include-ext` | | string | ❌ | mp4,mkv,... | File extensions to process |
| `--filename-parser` | | enum | ❌ | rules | Filename parser: `rules`, `anifilebert`, or `auto` (fall back to AniFileBERT after rules fail) |
| `--verbose` | `-v` | bool | ❌ | false | Show detailed logs |
| `--fallback-on-link-failure` | | enum | ❌ | - | Fallback when hard link fails: move or copy (disabled by default) |
| `--season-mode` / `--分季` | | bool | ❌ | false | Group multi-season titles as `Series/Season N/` |
| `--scrape-metadata` / `--刮削` | | bool | ❌ | false | Enable Bangumi metadata scraping with optional TMDB artwork |
| `--tmdb-api-key` | | string | ❌ | - | TMDB API key for backup posters and fanart |
| `--alias-file` | | string | ❌ | - | Custom alias JSON file overriding same-name entries from local `bangumi.db` |
| `--no-images` | | bool | ❌ | false | Skip Kodi/MLIP artwork downloads while retaining available text metadata |
| `--no-episode-metadata` | | bool | ❌ | false | Skip Bangumi episode title, summary, and duration lookup |
| `--force-overwrite` | | bool | ❌ | false | Overwrite existing NFO and image files |
| `--bangumi-cache` | | string | ❌ | system cache dir | Cache for Bangumi Archive and AnimeAtlas SQLite; kept outside the media target by default |
| `--metadata-source` | | string | ❌ | - | Local `subject.jsonlines` or its directory; explicit use stays offline and disables AnimeAtlas refresh |
| `--library-index` | | bool | ❌ | false | Generate/update `library.db` in the target root |
| `--mlip` | | bool | ❌ | false | Generate `library.db`, Bangumi metadata, and missing artwork without Kodi NFO |
| `--rebuild-library-index` | | bool | ❌ | false | Full target rescan; with `--mlip`, also refresh metadata and download missing artwork |
| `--refresh-library-metadata` | | bool | ❌ | false | With `--mlip`, re-match metadata using only the existing `library.db`; do not scan media, and pack new artwork into MLIP tar files |
| `--probe-runtime` | | bool | ❌ | false | Use `ffprobe` to probe video duration and write `episode.runtime` in seconds |
| `--help` | `-h` | bool | ❌ | false | Show help |
| `--version` | `-V` | bool | ❌ | false | Show version |

### 🧾 Metadata Scraping

When `--scrape-metadata` is enabled, the tool will:

- Prefer aliases from `--alias-file` JSON and a local alias database; when no local `animeatlas.sqlite`/`bangumi.db` is available, it downloads `animeatlas.sqlite` from the fixed AnimeAtlas release URL into the cache directory and falls back to the existing cache or Bangumi title/search matching if that download fails
- Generate `tvshow.nfo` in the series root
- Generate per-episode `*.nfo` files beside organized videos in `Season N/`
- Download `poster.jpg` and `seasonXX-poster.jpg` from Bangumi first; when a TMDB API key is provided, TMDB is used as a poster backup and for `fanart.jpg`

The default cache is `%LOCALAPPDATA%\anime-organizer\bangumi-cache` on Windows (falling back to `%APPDATA%`) and `$XDG_CACHE_HOME/anime-organizer/bangumi-cache` or `~/.cache/anime-organizer/bangumi-cache` on Linux/macOS. Override it with `--bangumi-cache`; the cache is not placed inside the media target by default.

Without `--metadata-source`, metadata modes refresh AnimeAtlas `animeatlas.sqlite` from its fixed release URL with a 24-hour TTL. The daemon also checks the cache hourly and refreshes it after expiry; failures keep the existing cache. An explicit `--metadata-source` selects local offline data and does not trigger AnimeAtlas downloads. A JSON `--alias-file` remains the highest-priority override.

Alias matching prefers the full weekly `bangumi.db` built by this repository every Sunday (about 31k anime with the infobox Chinese/localized titles and aliases, roughly 120 MB). It is cached for seven days and refreshed automatically; AnimeAtlas is only a fallback. Automatic download can be turned off in the WebUI under About → Metadata data source, which writes `metadata.json` in the data directory; when disabled only a `bangumi.db` already on disk is used.

The `--alias-file` JSON can reuse the object output format from `extract-aliases`, for example:

```json
{
  "Frieren": {
    "bangumi_id": 444444,
    "name": "葬送のフリーレン",
    "tmdb_id": 209867,
    "anidb_id": 18597
  }
}
```

### 🗃️ MLIP Library Index

`--library-index` manages a fixed `library.db` in the target root. It only writes the file index: it does not fetch metadata or artwork. For MiruPlay, prefer `--mlip`; it writes Bangumi series and episode metadata and downloads missing `poster.jpg` and `seasonXX-poster.jpg` files into the physical media directory. Existing artwork is preserved unless `--force-overwrite` is set; `--no-images` skips downloads. Kodi NFO files are still generated only by `--scrape-metadata`.

A TMDB API key is optional and only adds backup poster/fanart coverage. When Bangumi episodes include `duration_seconds`, it is written to `episode.runtime`; pass `--probe-runtime` to use `ffprobe` and prefer actual local video duration. Runtime probing stays disabled by default to avoid reading every file on NAS, WebDAV, or cloud-backed libraries.

During a full target scan, an immediate `Season N` parent is authoritative. Flat season-one files, a `Season 1` subdirectory, and later `Season N` directories may coexist; when a filename season conflicts with its directory, the directory wins. Artwork paths always follow the physical top-level media directory instead of creating a separate parser-derived directory.

The first run creates the database by scanning the full target directory after organization finishes:

```bash
aniorg \
  --source="/path/to/downloads" \
  --target="/path/to/anime" \
  --library-index
```

Later runs with the same command are incremental when `library.db` already exists: only files successfully organized in the current run are upserted. For NAS, WebDAV, and CloudDrive targets, `library.db` is copied to the system temp directory, updated locally, then uploaded and replaced sequentially instead of being randomly written in place.

```bash
aniorg \
  --source="/path/to/new-downloads" \
  --target="/path/to/anime" \
  --library-index
```

Use `--rebuild-library-index` when files were manually added, removed, or moved. To rebuild an existing target without organizing new media, point `--source` at an existing empty directory:

```bash
# Offline file index only
aniorg \
  --source="/path/to/empty" \
  --target="/path/to/anime" \
  --library-index \
  --rebuild-library-index

# MiruPlay-ready metadata and missing artwork
aniorg \
  --source="/path/to/empty" \
  --target="/path/to/anime" \
  --mlip \
  --rebuild-library-index
```

The source directory must exist. An empty source performs no move, copy, or hard-link operations; it only scans the target. `--dry-run --library-index` does not create or modify `library.db`; it only reports whether the command would initialize, incrementally update, or rebuild the index. `media_file.path` values are stored relative to the directory containing `library.db` and always use `/` separators.

Use `--refresh-library-metadata` when only aliases or provider metadata changed. It requires no `--source`: media paths, episode numbers, size, mtime, hashes, and artwork bindings come from the existing `library.db`; the target is not recursively enumerated and video bytes are never read. The mode force-refreshes the AnimeAtlas cache, re-resolves Bangumi and episode metadata, downloads missing or `--force-overwrite` artwork at explicit paths, and publishes those images through the existing artwork tar/catalog builder. A daemon `organize` job requires top-level `confirmed=true`.

```bash
aniorg \
  --target="/path/to/anime" \
  --mlip \
  --refresh-library-metadata
```

Matching `.srt`, `.ass`, `.ssa`, and `.vtt` sidecars are organized with their video and exported through the required MLIP v2 `media_subtitle` table. A sidecar must share the video stem or append a language-style suffix; `.sub` is excluded because VobSub requires a paired `.idx`.

MLIP v3 adds the required `media_extra` table. Full target scans classify OVA, NCOP, NCED, Tokuten, and Images videos as series-owned extras while ignoring Blu-ray menu videos. Extras never create fake episode rows or change the normal episode count. Existing v1/v2 databases migrate to v3 on the next write.

MLIP v4 is the current writer output. It adds `artwork_pack` and `artwork_asset`, storing verified original image bytes in immutable, uncompressed tar shards. `series_artwork` and `episode_artwork` retain relative `path` fallback and can reference an `asset_id` with binding-level provenance. Packs are published and verified before a `library.db` that references them. A full rebuild writes v4; an audited v3 library can be upgraded without re-resolving titles or metadata:

```bash
cargo run --example migrate_mlip_v3_to_v4 -- "/path/to/anime"
```

### 🔗 Hard Link Notes

Hard linking is the recommended mode:

- **Zero Extra Space**: No additional disk space used
- **Fast Operation**: Almost instant
- **File Sync**: Source and target share the same content

If hard linking fails due to cross-filesystem layouts or lack of support, you can opt in to automatic fallback via `--fallback-on-link-failure=copy` or `--fallback-on-link-failure=move`; otherwise, the failure is reported and the file is skipped.

**Requirements:**
1. Source and target must be on the same filesystem
2. Filesystem must support hard links (ext4, NTFS, APFS, etc.)
3. Write permission required for both directories

### 🔧 Scraper Subcommands (requires `--features scraper`)

```bash
# Scrape recent updates
cargo run --features scraper -- scrape --days 7 --format json

# Match alias proposals
cargo run --features scraper -- match --input scraped.json --format github

# Build SQLite alias database
cargo run --features scraper -- build-db --output bangumi.db

# Extract aliases from dump
cargo run --features scraper -- extract-aliases --download

# Merge new aliases into database
cargo run --features scraper -- merge-aliases --input new_aliases.json

# Apply confident match proposals
cargo run --features scraper -- apply-matches --input proposals.json

# Create alias request issues
cargo run --features scraper -- create-alias-issues --input uncertain.json --repo ModerRAS/anime-organizer
```

### 📡 RSS Subscription Management (requires `--features clouddrive`)

```bash
# List subscriptions
aniorg rss --list-subscriptions

# Add subscription
aniorg rss --add-subscription \
  --rss-url "https://example.com/rss" \
  --rss-target "/anime" \
  --rss-filter "720p"

# Single shot execution
aniorg rss --single-shot \
  --clouddrive-url http://localhost:19798 \
  --rss-url "https://example.com/rss" \
  --rss-target "/anime"

# Daemon mode (continuous monitoring)
aniorg rss --daemon \
  --clouddrive-url http://localhost:19798 \
  --rss-interval 300
```

Authentication supports either `--clouddrive-token` or `--clouddrive-user` + `--clouddrive-pass`. Subscription and dedupe state is stored in `%LOCALAPPDATA%\anime-organizer\rss.db` on Windows.

### ☁️ CloudDrive Helper Commands (requires `--features clouddrive`)

```bash
# Submit a magnet or torrent URL directly to offline download
aniorg add-offline "magnet:?xt=urn:btih:..." \
  --target "/anime" \
  --clouddrive-url http://localhost:19798 \
  --clouddrive-token "YOUR_TOKEN"

# Browse a cloud folder
aniorg list-folder "/anime" \
  --clouddrive-url http://localhost:19798 \
  --clouddrive-token "YOUR_TOKEN"
```

### 🧲 Torrent Scraping (requires `--features torrent-scraper`)

```bash
# Scrape DMHY torrent titles
aniorg torrent-scrape --source dmhy --output dmhy_titles.txt

# Scrape Nyaa torrent titles
aniorg torrent-scrape --source nyaa --query "anime" --output nyaa_titles.txt

# Scrape both sources
aniorg torrent-scrape --source all --pages 3 --output all_titles.txt
```

### 🛠️ Development

```bash
# Run tests
cargo test

# Run with verbose output
cargo run -- --source="/path/to/downloads" --verbose

# Build release binary
cargo build --release

# Build with all features
cargo build --release --features "scraper clouddrive torrent-scraper"
```

## License

AGPL-3.0 License - see [LICENSE](LICENSE) for details.
