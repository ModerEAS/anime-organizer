# anime-organizer 项目知识库

**Generated:** 2026-04-13
**Commit:** a3530dc
**Branch:** master

## OVERVIEW
Rust CLI工具，批量整理动漫视频文件（硬链接/移动/复制），支持Kodi元数据刮削。核心栈：clap + regex + rusqlite + reqwest + tonic。

## STRUCTURE
```
./
├── src/                    # 源码（含lib.rs + main.rs双入口）
│   ├── parser.rs          # 文件名解析（正则）
│   ├── organizer.rs        # 文件操作（Move/Copy/Link）
│   ├── error.rs           # 统一错误类型
│   ├── nfo.rs             # Kodi NFO生成
│   ├── metadata/           # 元数据（Bangumi/TMDB/别名）
│   ├── scraper/           # 刮削子命令（LLM辅助）
│   ├── torrent/           # Torrent种子文件爬取（DMHY/Nyaa）
│   └── rss/               # RSS/CloudDrive同步
├── proto/                  # gRPC协议定义（clouddrive feature）
├── tests/                  # 集成测试
└── .github/workflows/      # CI/CD（5个workflow）
```

## WHERE TO LOOK
| Task | Location | Notes |
|------|----------|-------|
| 添加新CLI参数 | src/main.rs | clap derive，`OrganizeArgs`/`Commands` |
| 程序自身更新 | src/self_update.rs、src/daemon/app_update.rs | GitHub Release、SHA256、同目录替换；`aniorg update` |
| 修改文件名解析规则 | src/parser.rs | `ANIME_FILE_REGEX` |
| 文件操作逻辑 | src/organizer.rs | `FileOrganizer::organize_to_dir` |
| 新增错误类型 | src/error.rs | thiserror枚举 |
| NFO格式修改 | src/nfo.rs | Kodi规范兼容性 |
| API客户端 | src/metadata/*.rs | bangumi/tmdb客户端 |
| RSS功能 | src/rss/*.rs | CloudDrive2 gRPC |
| 刮削功能 | src/scraper/*.rs | LLM匹配逻辑 |
| Torrent爬虫 | src/torrent/*.rs | DMHY/Nyaa种子文件名爬取 |

## CI/CD RULES（强制）
```bash
cargo fmt --all -- --check  # 必须格式化
cargo clippy --all-features -- -D warnings  # 禁止警告
cargo doc --no-deps --document-private-items  # 文档完整
```
- PR需Ubuntu/Windows/macOS三平台测试通过
- 推送前检查GitHub Actions状态

## ANTI-PATTERNS (THIS PROJECT)
- ❌ 跨文件系统硬链接（会返回`CrossDeviceLink`错误）
- ❌ 忽略`#[must_use]`返回值（parser模块）
- ❌ 未启用`--fallback-on-link-failure`时硬链接失败不处理
- ❌ 使用Python或其他非Rust语言（项目强制Pure Rust）

## UNIQUE STYLES
- Feature-gated模块：`#[cfg(feature = "...")]`
- 静态正则：`LazyLock`预编译
- 批量SQL：1000条/批，事务提交
- 异步运行时：`tokio`（metadata/clouddrive features）

## COMMANDS
```bash
cargo build --release              # 默认（含metadata）
cargo build --release --features "scraper clouddrive torrent-scraper"  # 全功能
cargo test                         # 单元+集成测试
cargo doc --no-deps               # 文档生成
```

### Torrent爬虫命令（需 `--features torrent-scraper`）
```bash
# 爬取DMHY种子文件列表
aniorg torrent-scrape --source dmhy --output dmhy_titles.txt

# 爬取Nyaa种子文件列表
aniorg torrent-scrape --source nyaa --query "anime" --output nyaa_titles.txt

# 爬取所有来源
aniorg torrent-scrape --source all --pages 3 --output all_titles.txt
```

## NOTES
- 硬链接默认，需同文件系统；跨设备需`--fallback-on-link-failure=copy`
- Bangumi dump 会按需下载；若存在 `bangumi.db` 或传入 `--alias-file`，会优先用于别名匹配
- 临时文件用`tmp/`（已gitignore）
- `.sisyphus/`为任务管理数据，已gitignore

## SUBMODULE LOCATIONS
- `src/scraper/` → `./src/scraper/AGENTS.md` (LLM匹配、数据库构建)
- `src/rss/` → `./src/rss/AGENTS.md` (gRPC客户端、RSS调度)
