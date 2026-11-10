//! UI text in Chinese and English, on Ely's `I18n` global.
//!
//! `init` installs both catalogs with Chinese as the default locale (the
//! project's primary audience); `toggle` flips between the two. All user-facing
//! strings in the app go through [`t`].

use ely_gpui_component::i18n::I18n;
use gpui::{App, SharedString};

pub const ZH: &str = "zh-CN";
pub const EN: &str = "en-US";

/// Install the catalogs with `default` as the starting locale (the persisted
/// user choice, or [`ZH`]). Call once at startup, before the first window.
pub fn init(cx: &mut App, default: &str) {
    let default = if default == EN { EN } else { ZH };
    cx.set_global(
        I18n::new(default)
            .catalog(
                ZH,
                &[
                    ("app_name", "pido"),
                    ("new_chat", "新会话"),
                    ("sessions", "历史会话"),
                    ("no_sessions", "还没有会话记录"),
                    ("send", "发送"),
                    ("stop", "停止"),
                    ("steer_hint", "运行中:Enter 即转向 (steer)"),
                    ("idle_hint", "Enter 发送,Shift+Enter 换行"),
                    ("welcome_title", "开始使用 pido"),
                    ("welcome_desc", "Pi Coding Agent 的桌面客户端。选择一个项目目录,新建会话即可开始。"),
                    ("project_dir", "项目目录"),
                    ("start", "启动会话"),
                    ("starting", "正在启动 pi …"),
                    ("pi_missing", "未检测到 pi CLI"),
                    ("pi_missing_desc", "请先安装:powershell -c \"irm https://pi.dev/install.ps1 | iex\""),
                    ("auth_missing", "pi 尚未登录任何模型服务"),
                    ("auth_missing_desc", "请在终端运行 pi 并执行 /login,或 pi auth login <provider>。"),
                    ("model", "模型"),
                    ("thinking", "思考"),
                    ("theme", "主题"),
                    ("language", "语言"),
                    ("connecting", "连接中 …"),
                    ("disconnected", "已断开"),
                    ("connection_failed", "连接失败"),
                    ("reconnect", "返回首页"),
                    ("session_restored", "已恢复会话"),
                    ("context_compacted", "上下文已压缩"),
                    ("retrying", "遇到临时错误,自动重试中 …"),
                    ("you", "我"),
                    ("pi", "Pi"),
                    ("empty_transcript", "会话为空,发第一条消息吧"),
                    ("tokens", "tokens"),
                    ("cost", "成本"),
                    ("messages", "条消息"),
                    ("compact_now", "压缩上下文"),
                    ("dir_invalid", "目录不存在,请重新选择"),
                    ("prompt_failed", "发送失败"),
                    ("new_session_started", "已开启新会话"),
                    ("extension_notify", "扩展通知"),
                    ("dismiss", "知道了"),
                ],
            )
            .catalog(
                EN,
                &[
                    ("app_name", "pido"),
                    ("new_chat", "New chat"),
                    ("sessions", "Sessions"),
                    ("no_sessions", "No sessions yet"),
                    ("send", "Send"),
                    ("stop", "Stop"),
                    ("steer_hint", "Running: Enter steers the agent"),
                    ("idle_hint", "Enter to send, Shift+Enter for a new line"),
                    ("welcome_title", "Welcome to pido"),
                    ("welcome_desc", "A desktop client for the Pi coding agent. Pick a project directory and start a session."),
                    ("project_dir", "Project directory"),
                    ("start", "Start session"),
                    ("starting", "Starting pi …"),
                    ("pi_missing", "pi CLI not found"),
                    ("pi_missing_desc", "Install it first: curl -fsSL https://pi.dev/install.sh | sh"),
                    ("auth_missing", "pi has no authenticated provider"),
                    ("auth_missing_desc", "Run pi in a terminal and use /login, or pi auth login <provider>."),
                    ("model", "Model"),
                    ("thinking", "Thinking"),
                    ("theme", "Theme"),
                    ("language", "Language"),
                    ("connecting", "Connecting …"),
                    ("disconnected", "Disconnected"),
                    ("connection_failed", "Connection failed"),
                    ("reconnect", "Back to start"),
                    ("session_restored", "Session restored"),
                    ("context_compacted", "Context compacted"),
                    ("retrying", "Transient error, retrying …"),
                    ("you", "You"),
                    ("pi", "Pi"),
                    ("empty_transcript", "Nothing here yet — say something"),
                    ("tokens", "tokens"),
                    ("cost", "cost"),
                    ("messages", "messages"),
                    ("compact_now", "Compact context"),
                    ("dir_invalid", "Directory does not exist"),
                    ("prompt_failed", "Send failed"),
                    ("new_session_started", "New session started"),
                    ("extension_notify", "Extension notice"),
                    ("dismiss", "Dismiss"),
                ],
            ),
    );
}

/// Look up `key` in the active locale.
pub fn t(cx: &App, key: &str) -> SharedString {
    cx.global::<I18n>().text(key, &[])
}

/// The locale used next, toggling between Chinese and English.
pub fn other_locale(cx: &App) -> &'static str {
    if cx.global::<I18n>().locale().tag == ZH {
        EN
    } else {
        ZH
    }
}
