# 需求文档

## 介绍

anime-organizer 目前只能手动换二进制。这次按隔壁 `arknights-auto-update` 的方式，让 `aniorg` 自己检查 GitHub Release、核对压缩包，并在同目录替换后重启。发布标签除了 `vMAJOR.MINOR.PATCH`，还有 master 构建使用的 `vMAJOR.MINOR.PATCH+BUILD`。

## 需求

### 需求 1 - 程序自身更新

**用户故事：** 作为已经装好 aniorg 的人，我希望它自己换成 GitHub Release 上的新构建，不用每次手动下压缩包。

#### 验收标准

1. When 用户执行 `aniorg update --check`，the anime-organizer shall 查询最新稳定 Release，并只报告是否可更新，不下载、不替换。
2. When 用户执行 `aniorg update` 且远端构建更新，the anime-organizer shall 下载当前平台的压缩包，核对 SHA256，解出 `aniorg` 或 `aniorg.exe` 后替换并按原来的参数重启。
3. While 远端版本号或构建号高于当前程序，when 程序更新检查运行，the anime-organizer shall 把该版本视为可更新，即使用户本地没有编译进提交号。
4. While 远端版本号和构建号都等于当前程序、两端都能读到提交号且提交号不同，when 程序更新检查运行，the anime-organizer shall 把该构建视为可更新。
5. While 远端版本号或构建号低于当前程序，或版本与构建号相同且提交号一致，when 程序更新检查运行，the anime-organizer shall 不下载、不替换当前程序。
6. While 当前程序没有提交号且远端版本号和构建号并不更高，when 程序更新检查运行，the anime-organizer shall 不替换当前程序。
7. When 校验、路径检查或可执行文件头检查失败，the anime-organizer shall 放弃这次替换，并不覆盖正在运行的文件。
8. When 压缩包里还有和程序同目录的 DLL，the anime-organizer shall 一并替换这些 DLL，并且不把其他文件落盘。
9. While 自动应用程序更新已开启，且当前可执行文件不在 Cargo 的 `target` 目录里，when daemon 发现可更新构建，the anime-organizer shall 下载、替换并重启自己。
10. While 当前可执行文件在 Cargo 的 `target` 目录里，when 自动检查发现新构建，the anime-organizer shall 只提示，不自动替换。
11. When 用户在关于页确认「更新并重启」，the anime-organizer shall 即使处在 Cargo 的 `target` 目录里也执行替换和重启。
12. When 替换程序，the anime-organizer shall 保留原来的启动参数并重新拉起进程。
13. While 有整理任务正在运行，when 程序更新准备替换进程，the anime-organizer shall 先等任务结束，再重启。
14. When daemon 启动且程序更新间隔不是关闭，the anime-organizer shall 在短时间后查询最新稳定 Release，之后按该间隔重复。默认间隔是 `6h`，短于 5 分钟时按 5 分钟。

### 需求 2 - 走代理下载

**用户故事：** 作为需要代理才能访问 GitHub 的人，我希望程序更新走我配置的代理。

#### 验收标准

1. When 用户把 `proxy` 配成 `http`、`https`、`socks5` 或 `socks5h` 地址，the anime-organizer shall 让程序更新的查询和下载都通过该代理发出。
2. When `proxy` 为空，the anime-organizer shall 使用 `ALL_PROXY`、`HTTPS_PROXY`、`HTTP_PROXY` 及其小写形式中的第一个有效值；都没有则直连。
3. When `proxy` 为 `off`，the anime-organizer shall 直连，不读取上述环境变量。
4. When 代理地址带有账号或密码，the anime-organizer shall 在页面上隐藏这些凭据。
5. When 代理地址协议不受支持或缺少主机，the anime-organizer shall 拒绝保存该配置。
6. When 访问本机回环地址，the anime-organizer shall 不把该请求送进代理。

### 需求 3 - 页面、命令和旧数据

**用户故事：** 作为已经在用 daemon 的人，我希望打开关于页就能看到程序版本和更新状态，旧的数据库文件也不必手改。

#### 验收标准

1. When 用户打开关于页，the anime-organizer shall 显示当前程序版本、程序更新状态和生效的代理。
2. When 用户保存设置，the anime-organizer shall 把检查间隔、是否自动替换、更新源和代理写入 `update.json`。
3. When 数据目录里还没有 `update.json`，the anime-organizer shall 按默认开启程序更新（`6h`，自动替换），并保持直连，除非环境变量里已有代理。
4. When 用户点击「检查」，the anime-organizer shall 立刻查询一次程序更新。
5. When 用户点击「更新并重启」且确认，the anime-organizer shall 在有新构建时下载并重启，没有新构建时只说明已经是当前发布。
6. When 发布工作流上传压缩包，the anime-organizer shall 同时上传同名 `.sha256`，并把当前提交号编译进二进制。
