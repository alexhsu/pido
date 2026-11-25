//! UI text in Chinese and English, on Ely's `I18n` global.
//!
//! `init` installs both catalogs with Chinese as the default locale (the
//! project's primary audience); `toggle` flips between the two. All user-facing
//! strings in the app go through [`t`].

use ely_gpui_component::i18n::I18n;
use gpui::{App, SharedString};

pub const ZH: &str = "zh-CN";
pub const EN: &str = "en-US";

/// The Chinese catalog. Every key here must also appear in [`EN_STRINGS`];
/// a duplicate key aborts Ely's `I18n::new` at startup, so the test below
/// checks the tables instead.
const ZH_STRINGS: &[(&str, &str)] = &[
    ("app_name", "pido"),
    ("new_chat", "新会话"),
    ("projects", "项目"),
    ("collapse_all", "全部折叠"),
    ("reconnecting", "pi 已断开,正在自动重连(第 {n} 次)…"),
    ("expand_all", "全部展开"),
    ("add_project", "添加项目"),
    ("rename_project", "重命名项目"),
    ("remove_project", "删除工作区"),
    (
        "remove_project_desc",
        "将从侧栏移出「{name}」。会话记录保留在磁盘上,不会删除。",
    ),
    ("remove_confirm", "删除"),
    ("sessions", "历史会话"),
    ("no_sessions", "还没有会话记录"),
    ("send", "发送"),
    ("stop", "停止"),
    ("steer_hint", "运行中:Enter 即转向 (steer)"),
    ("idle_hint", "Enter 发送,Shift+Enter 换行"),
    ("greeting_morning", "早上好"),
    ("greeting_afternoon", "下午好"),
    ("greeting_evening", "晚上好"),
    (
        "welcome_desc",
        "Pi Coding Agent 的桌面客户端。选择一个项目目录,新建会话即可开始。",
    ),
    ("project_dir", "项目目录"),
    ("start", "启动会话"),
    ("starting", "正在启动 pi …"),
    ("pi_missing", "未检测到 pi CLI"),
    ("pi_missing_desc", "请先安装 pi"),
    (
        "pi_missing_desc_win",
        "请先安装:powershell -c \"irm https://pi.dev/install.ps1 | iex\"",
    ),
    (
        "pi_missing_desc_unix",
        "请先安装:curl -fsSL https://pi.dev/install.sh | sh",
    ),
    ("pi_broken_desc", "pi 存在但无法运行:{e}"),
    (
        "pi_path_hint",
        "从桌面图标启动的 pido 不会继承终端里的 PATH。若 pi 装在 ~/.pi/agent/bin、~/.local/bin 等目录,请把该目录加进 PATH,或用 PIDO_PI_PATH 指向 pi 的完整路径。",
    ),
    ("project_placeholder", "选择项目目录"),
    ("image_unsupported", "不支持的图片格式"),
    ("image_too_large", "图片过大:{d}"),
    ("add_project_failed", "添加项目失败:{e}"),
    ("clear_queue_failed", "清空队列失败:{e}"),
    ("auth_missing", "pi 尚未登录任何模型服务"),
    (
        "auth_missing_desc",
        "请在终端运行 pi 并执行 /login,或 pi auth login <provider>。",
    ),
    ("model", "模型"),
    ("thinking", "思考"),
    ("update_available", "pido {v} 可以更新"),
    ("update_now", "立即更新"),
    ("update_release_page", "发布页"),
    ("update_downloading", "正在下载更新 {p}%"),
    ("update_downloading_bytes", "正在下载更新 {m} MB"),
    ("update_ready", "更新已就绪（{v}），重启后生效"),
    ("restart_now", "立即重启"),
    ("later", "稍后"),
    ("update_failed", "更新失败：{e}"),
    ("update_none", "已是最新版本（{v}）"),
    ("update_check_failed", "检查更新失败：{e}"),
    ("check_updates", "检查更新"),
    ("theme", "主题"),
    ("language", "语言"),
    ("connecting", "连接中 …"),
    ("disconnected", "已断开"),
    ("connection_failed", "连接失败"),
    ("reconnect", "返回首页"),
    ("session_restored", "已恢复会话"),
    ("context_compacted", "上下文已压缩"),
    ("context_compacted_range", "上下文已压缩({range})"),
    ("auto_compact", "自动压缩"),
    ("branch_left", "已切到新分支,原路径摘要"),
    ("find_placeholder", "搜索本会话"),
    ("find_none", "无匹配"),
    ("palette_placeholder", "搜索会话、模型、命令…"),
    ("shell_template", "运行命令(! 前缀)"),
    ("delivery", "投递方式"),
    ("tok_short", "tok"),
    ("delivery_steer", "转向投递"),
    ("delivery_follow_up", "后续投递"),
    ("deliver_all", "全部一次投递"),
    ("deliver_one", "每轮一条"),
    ("kind_command", "命令"),
    ("kind_model", "模型"),
    ("kind_thinking", "思考级别"),
    ("kind_session", "会话"),
    ("kind_action", "操作"),
    ("kind_file", "文件"),
    ("kind_user", "提问"),
    ("kind_pi", "回答"),
    ("kind_tool", "工具"),
    ("kind_notice", "通知"),
    ("kind_shell", "命令"),
    ("search_sessions", "搜索会话…"),
    ("retrying", "遇到临时错误,自动重试中 …"),
    ("retrying_detail", "重试 {n}({delay} 后):{e}"),
    ("compacting", "正在压缩上下文…"),
    ("you", "我"),
    ("pi", "Pi"),
    ("empty_transcript", "会话为空,发第一条消息吧"),
    ("tokens", "tokens"),
    ("cost", "成本"),
    ("messages", "条消息"),
    ("compact_now", "压缩上下文"),
    ("export_html", "导出 HTML"),
    ("export_html_done", "已导出:{path}"),
    ("export_html_no_path", "已导出,但未返回路径"),
    ("dir_invalid", "目录不存在,请重新选择"),
    ("prompt_failed", "发送失败"),
    ("new_session_started", "已开启新会话"),
    ("extension_notify", "扩展通知"),
    ("dismiss", "知道了"),
    ("time_now", "刚刚"),
    ("ext_select", "扩展选择"),
    ("ext_confirm", "扩展确认"),
    ("ext_input", "扩展输入"),
    ("ext_editor", "扩展编辑"),
    ("ext_cancel", "取消"),
    ("ext_submit", "提交"),
    ("status_turns", "{n} 轮"),
    ("status_messages", "{n} 条消息"),
    ("status_cost", "成本 {c}"),
    ("status_cache", "缓存命中 {p}%"),
    ("status_ctx", "上下文 {p}%"),
    ("time_hours", "{n}小时"),
    ("time_days", "{n}天"),
    ("time_months", "{n}个月"),
    ("time_years", "{n}年"),
    ("retry", "重试"),
    ("aborted", "已中止"),
    ("agent_busy", "正在运行,请先停止或等待完成"),
    ("session_cancelled", "操作已被扩展取消"),
    ("context_compacted_aborted", "上下文压缩已中止"),
    ("context_compacted_failed", "上下文压缩失败"),
    ("retry_succeeded", "重试成功"),
    ("retry_failed", "重试失败:{e}"),
    ("retry_failed_unknown", "未知错误"),
    (
        "extension_dialog_cancelled",
        "扩展请求确认,当前版本已自动取消",
    ),
    ("rename", "重命名会话"),
    ("rename_action", "重命名"),
    ("rename_hint", "会话名称"),
    ("save", "保存"),
    ("clear_queue", "清空队列"),
    ("changes", "改动"),
    ("changes_files", "{n} 个文件"),
    ("changes_empty", "这个会话还没有文件改动"),
    ("changes_close", "关闭改动面板"),
    ("pin_session", "置顶会话"),
    ("unpin_session", "取消置顶"),
    ("fork_session", "分叉会话"),
    ("fork_turn", "从这里分叉"),
    ("copy_session_id", "复制 Session ID"),
    ("reveal_in_folder", "在文件夹显示"),
    ("archive_session", "归档会话"),
    ("unarchive_session", "取消归档"),
    ("archived", "已归档"),
    ("session_id_copied", "已复制 Session ID"),
    ("session_forked", "已分叉会话"),
    ("session_archived", "已归档该会话"),
    ("session_unarchived", "已取消归档"),
    ("usage", "用量"),
    ("turn_working", "已工作 {t}"),
    ("terminal", "终端"),
    ("new_terminal", "新建终端"),
    ("close_panel", "收起侧栏"),
    ("terminal_gone", "终端已退出"),
    ("terminal_failed", "终端启动失败:{e}"),
    ("turn_done", "已完成，用时 {t}"),
    ("text_copied", "已复制"),
    ("copy_message", "复制内容"),
    ("queued_steer", "转向"),
    ("queued_follow_up", "追加"),
    ("queue_mode", "运行中发送:转向立即介入,追加等本轮结束"),
    (
        "pi_version_old",
        "pi {v} 低于 pido 支持的最低版本 {min},部分功能可能异常,建议升级 pi",
    ),
    ("auto_retry", "自动重试"),
    ("stop_retry", "停止重试"),
    ("tree", "会话树"),
    ("tree_refresh", "刷新分支树"),
    ("tree_fork_here", "从这里分叉"),
    ("tree_empty", "此会话还没有分支记录"),
    ("tree_no_session", "打开会话后可查看分支树"),
    ("stop_session_run", "停止后台运行"),
    ("close_session_slot", "关闭后台会话"),
    ("slot_cap", "后台会话已达上限(4),先关闭一个再继续"),
    // Settings window.
    ("settings", "设置"),
    ("open_config_file", "打开配置文件"),
    ("close", "关闭"),
    ("settings_general", "通用设置"),
    ("settings_models", "模型设置"),
    ("settings_skills", "Skill 设置"),
    ("settings_extensions", "扩展设置"),
    (
        "settings_general_detail",
        "界面外观、语言,以及 pi 的运行行为。",
    ),
    (
        "settings_models_detail",
        "当前模型、pi 启动默认值,以及各提供商的登录状态。",
    ),
    (
        "settings_skills_detail",
        "pi 已加载的技能;技能本体在技能目录里维护。",
    ),
    (
        "settings_extensions_detail",
        "pi 的内置扩展与已配置的扩展/包来源。",
    ),
    ("settings_appearance", "外观"),
    ("settings_appearance_detail", "深色、浅色,或跟随系统。"),
    ("appearance_dark", "深色"),
    ("appearance_light", "浅色"),
    ("appearance_system", "跟随系统"),
    ("settings_language_detail", "界面显示语言。"),
    ("settings_pi_behavior", "pi 行为"),
    ("auto_compact_detail", "上下文接近上限时自动压缩会话。"),
    ("compact_now_detail", "手动压缩当前会话的上下文。"),
    ("run_now", "立即执行"),
    ("auto_retry_detail", "请求失败时按退避策略自动重试。"),
    ("settings_queue_update", "队列与更新"),
    ("settings_queue_default", "运行中发送的默认投递"),
    (
        "settings_queue_default_detail",
        "新会话沿用;运行中可在输入框上方临时切换。",
    ),
    ("settings_update_check", "启动时检查更新"),
    (
        "settings_thinking_detail",
        "当前会话的思考等级,与输入框下拉一致。",
    ),
    ("settings_need_session", "需要已连接的会话才能修改"),
    ("settings_models_current", "当前会话模型"),
    ("settings_not_connected", "未连接"),
    ("settings_pi_defaults", "pi 启动默认值"),
    ("settings_default_provider", "默认提供商"),
    ("settings_default_model", "默认模型"),
    ("settings_default_thinking", "默认思考等级"),
    ("settings_providers", "提供商登录状态"),
    ("provider_ready", "已登录"),
    ("provider_not_ready", "未登录"),
    ("provider_login", "登录"),
    ("provider_logout", "登出"),
    (
        "provider_logout_confirm",
        "从 {name} 登出?pi 会删除它保存的凭据,正在运行的会话立即生效。",
    ),
    ("provider_logout_title", "登出提供商"),
    ("provider_logout_do", "登出"),
    ("provider_refresh", "刷新登录状态"),
    ("settings_save_defaults", "保存默认值"),
    ("settings_saved", "已写入 pi 配置"),
    ("settings_write_failed", "写入 pi 配置失败:{e}"),
    (
        "settings_restart_hint",
        "改动写入 pi 的配置文件;运行中的会话在重启 pi 后生效,新会话立即生效。",
    ),
    ("open_auth_config", "打开认证配置"),
    ("open_models_config", "打开模型配置"),
    ("settings_installed_skills", "已装技能"),
    ("settings_no_skills", "还没有装任何技能"),
    ("open_skills_dir", "打开技能目录"),
    ("scope_user", "用户"),
    ("scope_project", "项目"),
    ("settings_builtin_extensions", "内置扩展"),
    ("settings_builtin_tag", "内置"),
    ("settings_configured_extensions", "已配置扩展与包"),
    ("settings_no_extensions", "没有配置任何扩展或包"),
    ("open_extensions_dir", "打开扩展目录"),
    ("settings_ext_add", "添加扩展"),
    (
        "settings_ext_add_detail",
        "填 npm:/git:/https:// 源(进「包」)或本地扩展路径(进 extensions);用户级写入全局,项目级写入 .pi。",
    ),
    ("settings_add", "添加"),
    ("settings_ext_remove", "移除"),
    (
        "settings_ext_remove_detail",
        "从配置里删掉这一条;已安装的包文件保留在磁盘上。",
    ),
    ("settings_ext_invalid", "本地路径不存在,未添加"),
];

/// The English catalog.
const EN_STRINGS: &[(&str, &str)] = &[
    ("app_name", "pido"),
    ("new_chat", "New chat"),
    ("projects", "Projects"),
    ("collapse_all", "Collapse all"),
    (
        "reconnecting",
        "pi disconnected — reconnecting (attempt {n})…",
    ),
    ("expand_all", "Expand all"),
    ("add_project", "Add project"),
    ("rename_project", "Rename project"),
    ("remove_project", "Remove workspace"),
    (
        "remove_project_desc",
        "'{name}' will be removed from the sidebar. Its sessions stay on disk.",
    ),
    ("remove_confirm", "Remove"),
    ("sessions", "Sessions"),
    ("no_sessions", "No sessions yet"),
    ("send", "Send"),
    ("stop", "Stop"),
    ("steer_hint", "Running: Enter steers the agent"),
    ("idle_hint", "Enter to send, Shift+Enter for a new line"),
    ("greeting_morning", "Good morning"),
    ("greeting_afternoon", "Good afternoon"),
    ("greeting_evening", "Good evening"),
    (
        "welcome_desc",
        "A desktop client for the Pi coding agent. Pick a project directory and start a session.",
    ),
    ("project_dir", "Project directory"),
    ("start", "Start session"),
    ("starting", "Starting pi …"),
    ("pi_missing", "pi CLI not found"),
    ("pi_missing_desc", "Install pi first"),
    (
        "pi_missing_desc_win",
        "Install it first: powershell -c \"irm https://pi.dev/install.ps1 | iex\"",
    ),
    (
        "pi_missing_desc_unix",
        "Install it first: curl -fsSL https://pi.dev/install.sh | sh",
    ),
    ("pi_broken_desc", "pi is installed but will not run: {e}"),
    (
        "pi_path_hint",
        "pido started from the desktop does not inherit your terminal's PATH. If pi lives in ~/.pi/agent/bin or ~/.local/bin, add that directory to PATH, or point PIDO_PI_PATH at the binary.",
    ),
    ("project_placeholder", "Choose a project directory"),
    ("image_unsupported", "Unsupported image format"),
    ("image_too_large", "Image too large: {d}"),
    ("add_project_failed", "Could not add project: {e}"),
    ("clear_queue_failed", "Could not clear the queue: {e}"),
    ("auth_missing", "pi has no authenticated provider"),
    (
        "auth_missing_desc",
        "Run pi in a terminal and use /login, or pi auth login <provider>.",
    ),
    ("model", "Model"),
    ("theme", "Theme"),
    ("language", "Language"),
    ("connecting", "Connecting …"),
    ("disconnected", "Disconnected"),
    ("connection_failed", "Connection failed"),
    ("reconnect", "Back to start"),
    ("session_restored", "Session restored"),
    ("context_compacted", "Context compacted"),
    ("context_compacted_range", "Context compacted ({range})"),
    ("auto_compact", "Auto-compact"),
    ("branch_left", "New branch; summary of the path left behind"),
    ("find_placeholder", "Search this transcript"),
    ("find_none", "No matches"),
    ("palette_placeholder", "Search sessions, models, commands…"),
    ("shell_template", "Run a command (! prefix)"),
    ("delivery", "Delivery"),
    ("thinking", "Thinking"),
    ("update_available", "pido {v} is available"),
    ("update_now", "Update now"),
    ("update_release_page", "Release page"),
    ("update_downloading", "Downloading update {p}%"),
    ("update_downloading_bytes", "Downloading update {m} MB"),
    (
        "update_ready",
        "Update ready ({v}) — takes effect after a restart",
    ),
    ("restart_now", "Restart now"),
    ("later", "Later"),
    ("update_failed", "Update failed: {e}"),
    ("update_none", "pido {v} is up to date"),
    ("update_check_failed", "Update check failed: {e}"),
    ("check_updates", "Check for updates"),
    ("tok_short", "tok"),
    ("delivery_steer", "Steer delivery"),
    ("delivery_follow_up", "Follow-up delivery"),
    ("deliver_all", "Deliver all at once"),
    ("deliver_one", "One per turn"),
    ("kind_command", "Command"),
    ("kind_model", "Model"),
    ("kind_thinking", "Thinking"),
    ("kind_session", "Session"),
    ("kind_action", "Action"),
    ("kind_file", "File"),
    ("kind_user", "Prompt"),
    ("kind_pi", "Answer"),
    ("kind_tool", "Tool"),
    ("kind_notice", "Notice"),
    ("kind_shell", "Shell"),
    ("search_sessions", "Search sessions…"),
    ("retrying", "Transient error, retrying …"),
    ("retrying_detail", "Retry {n} (after {delay}): {e}"),
    ("compacting", "Compacting context…"),
    ("you", "You"),
    ("pi", "Pi"),
    ("empty_transcript", "Nothing here yet — say something"),
    ("tokens", "tokens"),
    ("cost", "cost"),
    ("messages", "messages"),
    ("compact_now", "Compact context"),
    ("export_html", "Export HTML"),
    ("export_html_done", "Exported: {path}"),
    ("export_html_no_path", "Exported, but pi returned no path"),
    ("dir_invalid", "Directory does not exist"),
    ("prompt_failed", "Send failed"),
    ("new_session_started", "New session started"),
    ("extension_notify", "Extension notice"),
    ("dismiss", "Dismiss"),
    ("time_now", "now"),
    ("ext_select", "Extension choice"),
    ("ext_confirm", "Extension confirmation"),
    ("ext_input", "Extension input"),
    ("ext_editor", "Extension editor"),
    ("ext_cancel", "Cancel"),
    ("ext_submit", "Submit"),
    ("status_turns", "{n} turns"),
    ("status_messages", "{n} messages"),
    ("status_cost", "cost {c}"),
    ("status_cache", "cache {p}%"),
    ("status_ctx", "context {p}%"),
    ("time_hours", "{n}h"),
    ("time_days", "{n}d"),
    ("time_months", "{n}mo"),
    ("time_years", "{n}y"),
    ("retry", "Retry"),
    ("aborted", "Stopped"),
    ("agent_busy", "The agent is running; stop or wait first"),
    ("session_cancelled", "Cancelled by an extension"),
    ("context_compacted_aborted", "Context compaction aborted"),
    ("context_compacted_failed", "Context compaction failed"),
    ("retry_succeeded", "Retry succeeded"),
    ("retry_failed", "Retry failed: {e}"),
    ("retry_failed_unknown", "unknown error"),
    (
        "extension_dialog_cancelled",
        "Extension asked for input; this build auto-cancelled it",
    ),
    ("rename", "Rename session"),
    ("rename_action", "Rename"),
    ("rename_hint", "Session name"),
    ("save", "Save"),
    ("clear_queue", "Clear queue"),
    ("changes", "Changes"),
    ("changes_files", "{n} files"),
    ("changes_empty", "No file changes in this session yet"),
    ("changes_close", "Close the changes panel"),
    ("pin_session", "Pin session"),
    ("unpin_session", "Unpin session"),
    ("fork_session", "Fork session"),
    ("fork_turn", "Fork from here"),
    ("copy_session_id", "Copy Session ID"),
    ("reveal_in_folder", "Show in folder"),
    ("archive_session", "Archive session"),
    ("unarchive_session", "Unarchive session"),
    ("archived", "Archived"),
    ("session_id_copied", "Session ID copied"),
    ("session_forked", "Session forked"),
    ("session_archived", "Session archived"),
    ("session_unarchived", "Session unarchived"),
    ("usage", "Usage"),
    ("turn_working", "Working · {t}"),
    ("terminal", "Terminal"),
    ("new_terminal", "New terminal"),
    ("close_panel", "Hide panel"),
    ("terminal_gone", "The shell exited"),
    ("terminal_failed", "Terminal failed to start: {e}"),
    ("turn_done", "Done in {t}"),
    ("text_copied", "Copied"),
    ("copy_message", "Copy text"),
    ("queued_steer", "Steer"),
    ("queued_follow_up", "Follow-up"),
    (
        "queue_mode",
        "Mid-run sends: steer joins now, follow-up queues",
    ),
    (
        "pi_version_old",
        "pi {v} is below the minimum pido expects ({min}); some features may misbehave — upgrading pi is recommended",
    ),
    ("auto_retry", "Auto retry"),
    ("stop_retry", "Stop retrying"),
    ("tree", "Session tree"),
    ("tree_refresh", "Refresh the branch tree"),
    ("tree_fork_here", "Fork from here"),
    ("tree_empty", "No branches recorded in this session yet"),
    ("tree_no_session", "Open a session to see its branch tree"),
    ("stop_session_run", "Stop the background run"),
    ("close_session_slot", "Close the background session"),
    (
        "slot_cap",
        "At the 4-session cap — close one before starting another",
    ),
    // Settings window.
    ("settings", "Settings"),
    ("open_config_file", "Open config file"),
    ("close", "Close"),
    ("settings_general", "General"),
    ("settings_models", "Models"),
    ("settings_skills", "Skills"),
    ("settings_extensions", "Extensions"),
    (
        "settings_general_detail",
        "Appearance, language, and pi's run behavior.",
    ),
    (
        "settings_models_detail",
        "Current model, pi's startup defaults, and provider sign-in status.",
    ),
    (
        "settings_skills_detail",
        "The skills pi has loaded; skills themselves live in the skills directory.",
    ),
    (
        "settings_extensions_detail",
        "pi's builtin extensions and configured extension/package sources.",
    ),
    ("settings_appearance", "Appearance"),
    (
        "settings_appearance_detail",
        "Dark, light, or follow the system.",
    ),
    ("appearance_dark", "Dark"),
    ("appearance_light", "Light"),
    ("appearance_system", "System"),
    (
        "settings_language_detail",
        "The language the interface displays in.",
    ),
    ("settings_pi_behavior", "pi behavior"),
    (
        "auto_compact_detail",
        "Compact the session automatically as the context nears its limit.",
    ),
    (
        "compact_now_detail",
        "Compact the current session's context by hand.",
    ),
    ("run_now", "Run now"),
    (
        "auto_retry_detail",
        "Retry failed requests automatically with backoff.",
    ),
    ("settings_queue_update", "Queue & updates"),
    (
        "settings_queue_default",
        "Default delivery for mid-run sends",
    ),
    (
        "settings_queue_default_detail",
        "New sessions start with this; the composer can switch per run.",
    ),
    ("settings_update_check", "Check for updates at startup"),
    (
        "settings_thinking_detail",
        "The current session's thinking level, as in the composer menu.",
    ),
    ("settings_need_session", "Connect a session to change these"),
    ("settings_models_current", "Current session model"),
    ("settings_not_connected", "Not connected"),
    ("settings_pi_defaults", "pi startup defaults"),
    ("settings_default_provider", "Default provider"),
    ("settings_default_model", "Default model"),
    ("settings_default_thinking", "Default thinking level"),
    ("settings_providers", "Provider sign-in status"),
    ("provider_ready", "Signed in"),
    ("provider_not_ready", "Not signed in"),
    ("provider_login", "Sign in"),
    ("provider_logout", "Sign out"),
    (
        "provider_logout_confirm",
        "Sign out of {name}? pi deletes its stored credentials; running sessions notice immediately.",
    ),
    ("provider_logout_title", "Sign out of provider"),
    ("provider_logout_do", "Sign out"),
    ("provider_refresh", "Refresh sign-in status"),
    ("settings_save_defaults", "Save defaults"),
    ("settings_saved", "Written to pi's config"),
    ("settings_write_failed", "Failed to write pi's config: {e}"),
    (
        "settings_restart_hint",
        "Changes land in pi's config files; running sessions pick them up after pi restarts, new sessions immediately.",
    ),
    ("open_auth_config", "Open auth config"),
    ("open_models_config", "Open models config"),
    ("settings_installed_skills", "Installed skills"),
    ("settings_no_skills", "No skills installed yet"),
    ("open_skills_dir", "Open skills directory"),
    ("scope_user", "User"),
    ("scope_project", "Project"),
    ("settings_builtin_extensions", "Builtin extensions"),
    ("settings_builtin_tag", "Builtin"),
    (
        "settings_configured_extensions",
        "Configured extensions & packages",
    ),
    (
        "settings_no_extensions",
        "No extensions or packages configured",
    ),
    ("open_extensions_dir", "Open extensions directory"),
    ("settings_ext_add", "Add extension"),
    (
        "settings_ext_add_detail",
        "An npm:/git:/https:// source (lands in packages) or a local extension path (lands in extensions); user scope writes globally, project scope writes .pi.",
    ),
    ("settings_add", "Add"),
    ("settings_ext_remove", "Remove"),
    (
        "settings_ext_remove_detail",
        "Deletes this entry from the config; installed package files stay on disk.",
    ),
    (
        "settings_ext_invalid",
        "Local path does not exist; nothing was added",
    ),
];

/// Install the catalogs with `default` as the starting locale (the persisted
/// user choice, or [`ZH`]). Call once at startup, before the first window.
pub fn init(cx: &mut App, default: &str) {
    let default = if default == EN { EN } else { ZH };
    cx.set_global(
        I18n::new(default)
            .catalog(ZH, ZH_STRINGS)
            .catalog(EN, EN_STRINGS),
    );
}

/// Look up `key` in the active locale.
pub fn t(cx: &App, key: &str) -> SharedString {
    cx.global::<I18n>().text(key, &[])
}

/// Look up `key` with one `{name}` argument filled.
pub fn t_arg(cx: &App, key: &str, name: &str, value: &str) -> SharedString {
    cx.global::<I18n>().text(key, &[(name, value)])
}

/// A wall-clock span ("2时7分4秒" / "2h 7m 4s"), skipping zero leading
/// units so a 64-second turn reads "1分4秒" / "1m 4s".
pub fn format_duration(cx: &App, span: std::time::Duration) -> String {
    let total = span.as_secs();
    let (h, m, s) = (total / 3_600, total % 3_600 / 60, total % 60);
    if is_zh(cx) {
        let mut out = String::new();
        if h > 0 {
            out.push_str(&format!("{h}小时"));
        }
        if h > 0 || m > 0 {
            out.push_str(&format!("{m}分"));
        }
        out.push_str(&format!("{s}秒"));
        out
    } else {
        let mut parts = Vec::new();
        if h > 0 {
            parts.push(format!("{h}h"));
        }
        if h > 0 || m > 0 {
            parts.push(format!("{m}m"));
        }
        parts.push(format!("{s}s"));
        parts.join(" ")
    }
}

/// Whether the active locale is Chinese, for formats a catalog cannot
/// express (dates assembled from parts).
pub fn is_zh(cx: &App) -> bool {
    cx.global::<I18n>().locale().tag == ZH
}

/// A wall-clock stamp ("11月10日 23:22" / "Nov 10, 23:22").
pub fn format_time(cx: &App, unix_secs: Option<i64>) -> SharedString {
    let Some(secs) = unix_secs else {
        return SharedString::default();
    };
    let (year, month, day) = civil_from_days(secs.div_euclid(86_400));
    let rest = secs.rem_euclid(86_400);
    let (hour, minute) = (rest / 3_600, rest % 3_600 / 60);
    let _ = year;
    let stamp = if is_zh(cx) {
        format!("{month}月{day}日 {hour:02}:{minute:02}")
    } else {
        format!(
            "{} {}, {hour:02}:{minute:02}",
            MONTH_EN[month as usize - 1],
            day
        )
    };
    stamp.into()
}

const MONTH_EN: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Relative age of `unix_secs`, in the active locale ("3天" / "3d").
pub fn relative_time(cx: &App, unix_secs: Option<i64>) -> SharedString {
    let Some(secs) = unix_secs else {
        return SharedString::default();
    };
    let Some(age) = chrono_age(secs) else {
        return SharedString::default();
    };
    let (key, n) = match age {
        Age::Now => return t(cx, "time_now"),
        Age::Hours(n) => ("time_hours", n),
        Age::Days(n) => ("time_days", n),
        Age::Months(n) => ("time_months", n),
        Age::Years(n) => ("time_years", n),
    };
    t_arg(cx, key, "n", &n.to_string())
}

enum Age {
    Now,
    Hours(i64),
    Days(i64),
    Months(i64),
    Years(i64),
}

fn chrono_age(unix_secs: i64) -> Option<Age> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let age = (now - unix_secs).max(0);
    let minutes = age / 60;
    let hours = age / 3600;
    let days = age / 86_400;
    Some(match age {
        _ if minutes < 60 => Age::Now,
        _ if hours < 24 => Age::Hours(hours),
        _ if days < 30 => Age::Days(days),
        _ if days < 365 => Age::Months(days / 30),
        _ => Age::Years(days / 365),
    })
}

/// The locale used next, toggling between Chinese and English.
pub fn other_locale(cx: &App) -> &'static str {
    if cx.global::<I18n>().locale().tag == ZH {
        EN
    } else {
        ZH
    }
}

#[cfg(test)]
mod tests {
    use super::{EN_STRINGS, ZH_STRINGS};
    use std::collections::HashSet;

    /// Ely asserts both properties at startup (a repeated key aborts the
    /// app, a key in one catalog only renders blank in the other); checking
    /// the tables here makes `cargo test` catch it first.
    #[test]
    fn catalogs_are_duplicate_free_and_agree_on_keys() {
        let mut seen = HashSet::new();
        for (key, _) in ZH_STRINGS {
            assert!(seen.insert(*key), "zh-CN repeats {key}");
        }
        let mut seen = HashSet::new();
        for (key, _) in EN_STRINGS {
            assert!(seen.insert(*key), "en-US repeats {key}");
        }
        let zh: HashSet<&str> = ZH_STRINGS.iter().map(|(key, _)| *key).collect();
        let en: HashSet<&str> = EN_STRINGS.iter().map(|(key, _)| *key).collect();
        let only_zh: Vec<&&str> = zh.difference(&en).collect();
        let only_en: Vec<&&str> = en.difference(&zh).collect();
        assert!(
            only_zh.is_empty() && only_en.is_empty(),
            "keys differ: zh-only {only_zh:?}, en-only {only_en:?}"
        );
    }
}
