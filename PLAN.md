# Localref Win32 Native Layer + Leptos SSR UI 迁移计划

## Summary
可行。迁移分两阶段：先集中 Windows 原生能力到独立 C++ crate，再用浏览器打开的 Leptos SSR + Tailwind 替换 Iced UI。通知按你的最新决定采用 Windows App SDK 的 `AppNotificationManager`，这是 Microsoft 当前对 unpackaged Win32/WPF/WinForms 这类桌面应用推荐的 app notification API。

默认选择：
- UI 宿主：浏览器打开 `localhost` SSR UI。
- 原生层：新增 Windows-only C++ crate，Rust 暴露安全薄 API。
- 通知：`Microsoft.Windows.AppNotifications::AppNotificationManager`。
- 不使用 PowerShell、cmd、`windows-sys` 作为项目原生功能实现路径。

## Key Changes
- 新增 workspace crate：`crates/native-win32`
  - Windows 下用 `cc` 编译 C++；Rust 通过 `extern "C"` 调用。
  - 非 Windows 提供同名 stub，返回 `Unsupported` 或 no-op。
  - Public Rust API：
    - `open_path(path: &Path) -> Result<()>`
    - `open_uri(uri: &str) -> Result<()>`
    - `create_directory_junction(link: &Path, target: &Path) -> Result<()>`
    - `show_app_notification(title: &str, body: &str, kind: NotificationKind) -> Result<()>`
    - `register_app_notifications() -> Result<()>`
    - `unregister_app_notifications() -> Result<()>`
    - `detach_console() -> Result<()>`
- C++ 实现：
  - 打开文件/文件夹/URL：`ShellExecuteW`。
  - junction：`DeviceIoControl(FSCTL_SET_REPARSE_POINT)` 创建 `IO_REPARSE_TAG_MOUNT_POINT`，保持当前 junction 语义，不降级 symlink。
  - quiet start：包装 `FreeConsole`。
  - 通知：用 C++/WinRT 调 Windows App SDK `AppNotificationManager::Default()`；进程启动时调用 `Register()`，退出前可调用 `Unregister()`。
  - V1 notification 只支持标题、正文、severity 对应 XML payload，不处理点击激活、按钮、输入框、进度条。
- Windows App SDK 依赖策略：
  - `native-win32` 明确依赖 Windows App SDK headers/libs，构建环境要求 MSVC + Windows SDK + Windows App SDK NuGet/runtime。
  - 计划优先使用 Windows App SDK unpackaged 支持；如果 runtime 缺失，返回明确错误并写日志，不退回旧 Toast 或 tray balloon。
- 替换调用点：
  - `rest_files` 的 Windows open 改调 `native_win32::open_path`。
  - `platformfs` 的 Windows link 创建改调 `native_win32::create_directory_junction`。
  - `native_tray` 通知改调 `native_win32::show_app_notification`。
  - `main` quiet startup 改调 `native_win32::detach_console`。
  - tray host 启动时调用 `register_app_notifications()`；正常退出路径调用 `unregister_app_notifications()`。
- Leptos SSR UI：
  - 新增 `crates/ui-web`，使用 Leptos SSR + Axum + Tailwind。
  - `desktop` feature 改为启用 web UI，不再依赖 Iced。
  - root binary 合并 UI router 与现有 REST router。
  - tray `Open UI` 调 `native_win32::open_uri(config.rest_endpoint())` 打开浏览器。
  - 删除或停用 Iced app 入口。

## UI Behavior
- SSR 首屏包含 dashboard、搜索、类别筛选、文献列表、详情、类别管理、文件列表、事件视图。
- URL query 保存 `q`、`category`、`selected`、`active`、`tab`。
- 多选类别逻辑保持当前规则：可添加类别不显示所选文献共有类别。
- Scan、pause/resume、metadata save、category add/remove 通过 POST server functions 或 Axum form handlers 完成后 redirect。
- V1 不迁移 Iced 拖放文件、多窗口、嵌入 WebView。

## Test Plan
- `native-win32`：
  - Windows integration test：junction tempdir 创建和读取验证。
  - App notification smoke test：调用 `register_app_notifications()` + `show_app_notification()`，默认 `#[ignore]`，手动验证通知中心出现 Localref。
  - `open_uri`/`open_path` 做参数转换与错误路径测试。
  - 非 Windows stub 测试返回 `Unsupported`。
- 替换验证：
  - 源码搜索不得出现 `windows-sys`、`powershell`、`Start-Process`、`Command::new("cmd")`。
  - 现有 REST、storage、platformfs tests 全部保留。
- Leptos SSR：
  - Router test 请求 `/`，断言 SSR HTML 包含 dashboard、item list、category form。
  - Form/server-function tests 覆盖搜索、多选类别、metadata revision conflict。
  - Tailwind/Leptos 构建：`cargo leptos build --release` 或等价 CI 命令。
- Full verification：
  - `cargo fmt --check`
  - `cargo test --workspace`
  - Windows 上运行 native smoke tests 和 tray `Open UI` 手测。

## Assumptions
- 当前工作树很脏，迁移前先提交或清理当前改动；不要把迁移混入已有 diff。
- 通知采用 Windows App SDK `AppNotificationManager`，不采用 UWP `ToastNotificationManager`，也不采用 `Shell_NotifyIcon`。
- V1 仅发送本地通知，不做 notification click activation；如果后续要点击通知打开 UI，需要新增 activation argument 和 callback/CLI 路径。
- 参考依据：
  - Microsoft 对通知 API 的推荐：[Windows notifications overview](https://learn.microsoft.com/en-us/windows/apps/develop/notifications/)
  - `AppNotificationManager` API：[AppNotificationManager class](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/winrt/microsoft.windows.appnotifications.appnotificationmanager?view=windows-app-sdk-1.8)
  - Windows App SDK notifications quickstart：[Quickstart App notifications](https://learn.microsoft.com/da-dk/windows/apps/windows-app-sdk/notifications/app-notifications/app-notifications-quickstart)
  - Leptos SSR/Axum 与 cargo-leptos：[Leptos SSR](https://book.leptos.dev/ssr/index.html)、[cargo-leptos](https://book.leptos.dev/ssr/21_cargo_leptos.html)
  - Native build/API：[cc crate](https://docs.rs/cc)、[ShellExecuteW](https://learn.microsoft.com/zh-tw/windows/win32/api/shellapi/nf-shellapi-shellexecutew)、[Reparse Point Operations](https://learn.microsoft.com/nb-no/windows/win32/fileio/reparse-point-operations)、[FreeConsole](https://learn.microsoft.com/en-us/windows/console/freeconsole)
