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
                    ("projects", "项目"),
                    ("collapse_all", "全部折叠"),
                    ("reconnecting", "pi 已断开,正在自动重连(第 {n} 次)…"),
                    ("expand_all", "全部展开"),
                    ("add_project", "添加项目"),
                    ("rename_project", "重命名项目"),
                    ("remove_project", "删除工作区"),
                    ("remove_project_desc", "将从侧栏移出「{name}」。会话记录保留在磁盘上,不会删除。"),
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
                    ("context_compacted_range", "上下文已压缩({range})"),
                    ("auto_compact", "自动压缩"),
                    ("branch_tree", "分支…"),
                    ("branch_tree_hint", "会话树中的每条提问;点一条即从那里分出新会话"),
                    ("tree_active", "当前"),
                    ("tree_empty", "这个会话还没有分支"),
                    ("retrying", "遇到临时错误,自动重试中 …"),
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
                    ("extension_dialog_cancelled", "扩展请求确认,当前版本已自动取消"),
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
                    ("turn_done", "已完成，用时 {t}"),
                    ("text_copied", "已复制"),
                    ("copy_message", "复制内容"),
                    ("queued_steer", "转向"),
                    ("queued_follow_up", "追加"),
                    ("queue_mode", "运行中发送:转向立即介入,追加等本轮结束"),
                ],
            )
            .catalog(
                EN,
                &[
                    ("app_name", "pido"),
                    ("new_chat", "New chat"),
                    ("projects", "Projects"),
                    ("collapse_all", "Collapse all"),
                    ("reconnecting", "pi disconnected — reconnecting (attempt {n})…"),
                    ("expand_all", "Expand all"),
                    ("add_project", "Add project"),
                    ("rename_project", "Rename project"),
                    ("remove_project", "Remove workspace"),
                    ("remove_project_desc", "'{name}' will be removed from the sidebar. Its sessions stay on disk."),
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
                    ("context_compacted_range", "Context compacted ({range})"),
                    ("auto_compact", "Auto-compact"),
                    ("branch_tree", "Branches…"),
                    ("branch_tree_hint", "Every prompt in the session's tree; click one to fork a new session from there"),
                    ("tree_active", "Active"),
                    ("tree_empty", "No branches in this session"),
                    ("retrying", "Transient error, retrying …"),
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
                    ("extension_dialog_cancelled", "Extension asked for input; this build auto-cancelled it"),
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
                    ("turn_done", "Done in {t}"),
                    ("text_copied", "Copied"),
                    ("copy_message", "Copy text"),
                    ("queued_steer", "Steer"),
                    ("queued_follow_up", "Follow-up"),
                    ("queue_mode", "Mid-run sends: steer joins now, follow-up queues"),
                ],
            ),
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
