# pido

Pi Coding Agent 的原生桌面客户端：项目会话、流式输出与工具卡片、运行中转向、模型/思考级别切换、历史会话恢复；Windows/macOS/Linux，中英双语，亮暗主题。

A native desktop client for the [Pi coding agent](https://github.com/earendil-works/pi) — Rust + [GPUI](https://github.com/zed-industries/zed) + [Ely GPUI Components](https://github.com/ELY-Open-Source/Ely-GPUI-Components). Pick a project, stream replies with thinking and tool cards, steer mid-run, switch models and thinking levels, resume past sessions. Windows, macOS, Linux, bilingual 中文/English, light/dark.

![status](https://img.shields.io/badge/status-early-orange) ![platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-blue)

## 功能 / Features

- **完整聊天闭环** — 选择项目目录启动会话,流式输出、思考过程、工具调用卡片实时呈现
- **运行中转向** — agent 执行中继续输入即 steer(转向),也可切为「追加」(follow-up)排队等本轮结束;排队中的消息以标签显示在输入框上方,可一键清空;Esc 或停止按钮 = 清空队列 + 中止
- **模型与思考级别切换** — 输入框右下角的模型胶囊,两级菜单切换 provider 模型和 thinking level
- **会话管理** — 侧栏按项目分组列出 `~/.pi/agent/sessions` 的历史会话:分叉/克隆出的子会话按血统嵌套在父会话下(可折叠);「项目」标题栏可一键全部折叠、"+" 添加任意文件夹为项目(无会话也常驻列表),拖拽调宽且限定 200–480px;右键菜单可置顶、重命名、分叉、复制 Session ID、在文件夹显示、归档(归档项收进底部「已归档」折叠);顶部搜索框按标题或内容过滤
- **消息操作** — 悬停用户消息显示分叉(分出新会话,该条消息回填输入框供修改重发)、时间和复制;悬停回答显示复制、token 用量和时间
- **回合状态** — 提交后回答上方计时(「已工作 1分4秒」),结束定格为「已完成,用时 X」;完成后默认折叠工具调用和思考过程,点状态行可展开
- **上下文压缩** — 设置窗口「pi 行为」里一键立即压缩,或开/关 pi 的自动压缩;压缩完成的横幅带前后 token 数,历史会话在压缩点留摘要标记
- **导出 HTML** — 头部一键把当前会话导出为 HTML 并在文件管理器中显示
- **命令运行器** — 输入框输 `!cmd` 直接跑 shell 命令(`!!` 发送原文),输出流式进转录行、可中止,并进入模型上下文;命令面板带「运行命令」模板
- **状态同步** — pi 侧的改名/换思考级别即时反映到侧栏与选择器;重试显示「第 2/3 次(2s 后):错误详情」;压缩启动有提示
- **运行控制** — 队列投递方式可选(全部一次 / 每轮一条);每条消息悬停即可复制;流式中实时显示累计 token
- **富展示** — 工具返回的图片(read 截图等)直接在卡片下渲染;工具结果一键复制;回答里的代码块自带复制按钮与折叠
- **并行会话** — 侧栏就是任务管理器:切走的会话在后台继续跑(每个会话独立 pi 进程),列表行前有运行中的 spinner 与完成后的未读点;右键可停止后台运行或关闭后台会话;同时最多 4 个活进程,满员自动让位给最旧的空闲会话;退出时的会话编队在下次启动原样回归
- **会话分支树** — 右侧栏「会话树」标签画出当前会话的分叉结构:活跃分支高亮,离开的分支带摘要标记(`branch_summary`);活跃分支的提问可在节点上一键「从这里分叉」;未连接时直接读会话文件建树
- **自动重试控制** — 设置窗口「pi 行为」里的自动重试开关(写入 pi 的全局设置);重试等待中的横幅可一键停止
- **版本提示** — pi 低于 pido 支持的最低版本时,转录上方给出升级提醒
- **搜索/面板增强** — 面板与侧栏过滤支持子序列模糊匹配(`xs` 找到「新会话」);面板置顶工作区文件(@ 引用);查找条显示命中行类型与上下文摘录
- **会话搜索** — 转录区 Ctrl+F 即时过滤并循环跳转(高亮当前行);侧栏输入框按标题/内容过滤会话,内容命中带上下文摘录
- **命令面板** — Ctrl+K 呼出:跳会话、切模型/思考级别、执行 pi 的 / 命令、新会话/导出/压缩/切主题,方向键 + Enter 即达
- **改动审查** — agent 用 `edit`/`write` 改过的文件按项目路径汇总,在可调宽、可隐藏的右侧栏里看统一/并排 diff,打开的历史会话也能重建
- **内置终端** — 右侧栏终端页运行真实 shell(Windows 为 PowerShell,Linux/macOS 为 `$SHELL`,ConPTY/PTY 驱动),可新建、随会话目录启动;切换会话时自动在新目录重生
- **统计** — token 用量、成本、上下文占用,每轮结束后刷新
- **亮暗主题 + 中英双语** — 一键切换,偏好持久化(含窗口尺寸、最近项目)
- **系统设置** — 头部齿轮打开设置窗口:通用(外观深色/浅色/跟随系统、语言、pi 的自动压缩与自动重试、队列投递默认值、启动检查更新、思考等级)、模型(pi 启动默认值可直接编辑:默认提供商/模型/思考等级;provider 登录状态一目了然,未登录的一键在内置终端里跑 `pi auth login`,已登录的可登出——删除 pi 存的凭据,运行中会话立即生效)、Skill 与扩展(每个技能可开关、可跳到所在目录;内置扩展可停用,配置的扩展与包可移除,可按用户/项目级添加 npm:/git:/https 源或本地扩展路径)。这些改动直接写入 pi 的配置文件,运行中的会话在重启 pi 后生效、新会话立即生效
- **A complete chat loop** — pick a project directory, stream replies, thinking, and tool-call cards live
- **Steering** — typing while the agent runs steers it, or switches to queueing it as a follow-up; queued messages show as chips over the composer and can be cleared in one click; Esc or the stop button clears the queue and aborts
- **Model & thinking switching** — the model pill at the composer's right opens two levels: model and thinking level
- **Sessions** — the sidebar lists past sessions grouped by project, with forked/cloned children nested under the session they came from (foldable): the "Projects" header folds everything at once and a "+" button adds any folder as a project (it stays listed even with no sessions); the width drags between 200–480px. The right-click menu pins, renames, forks, copies the session ID, reveals the file, and archives (archived sessions collect under an "Archived" fold at the bottom); a search box filters by title or content
- **Message actions** — hover a prompt to fork (a new session without it, its text back in the composer for re-editing), or copy; hover a reply for copy, token usage and time
- **Turn status** — a clock above the reply while it works ("Working · 1m 4s"), frozen to "Done in X" on completion; tool calls and thinking fold away, click the line to re-open
- **Compaction** — compact now, or toggle pi's auto-compaction, in the settings window's "pi behavior" section; the finished banner reports the before/after token counts, and restored sessions keep a summary marker at each compaction seam
- **HTML export** — one click exports the open session to HTML and reveals the file
- **Shell runner** — type `!cmd` to run a shell command (`!!` sends literally); output streams into a transcript row with a stop button and joins the model's context; the palette carries a run-a-command entry
- **State sync** — renames and thinking-level picks made in pi reach the sidebar and picker live; retries name their attempt and delay; compaction start announces itself
- **Run controls** — queue delivery is all-at-once or one-per-turn; every message copies on hover; a streaming answer shows its cumulative token count as it grows
- **Rich results** — images a tool returns (screenshot reads) render under the card; results copy in one click; code blocks in answers carry their own copy button and folding
- **Parallel sessions** — the sidebar doubles as the task manager: a session you switch away from keeps running in the background (each with its own pi process), rows carry a running spinner and an after-settle unread dot, the session menu stops a background run or closes its slot, at most four processes live at once with the oldest idle one giving way, and the fleet that was open at quit comes back on the next start
- **Branch tree** — the right panel's "Session tree" tab draws the open session's fork structure: the active branch highlighted, abandoned ones marked with their summaries (`branch_summary`); a user prompt on the active branch forks from its node in one click; with no live pi the tree is built from the session file itself
- **Retry control** — the auto-retry switch in the settings window's "pi behavior" section (persisted into pi's own global settings, applied to every open session); a retry's backoff stops from the banner
- **Version notice** — when the pi CLI is older than the minimum pido expects, a banner above the transcript says so
- **Search/palette** — subsequence fuzzy matching (`xs` finds 新会话); workspace files lead the palette (@refs); the find bar names the matched row's kind and excerpt
- **Search** — Ctrl+F filters the transcript and cycles through matches (the current row tints); the sidebar filters sessions by title or content, with a snippet on content hits
- **Command palette** — Ctrl+K: jump to sessions, switch models and thinking levels, run pi's slash commands, new chat / export / compact / theme — arrows and Enter
- **Change review** — files the agent edited or wrote, in a resizable, hideable right panel with unified/split diffs; restored sessions too
- **Built-in terminal** — the right panel's terminal tab runs a real shell (PowerShell on Windows, `$SHELL` on Linux/macOS) via ConPTY/PTY; spawn a fresh one any time, and it respawns in the new directory when the session switches
- **Stats** — tokens, cost, and context usage refresh after every run
- **Dark/light + 中文/English** — one-click toggle, persisted along with window size and last project
- **Settings** — the header gear opens a settings window: General (appearance dark/light/system, language, pi's auto-compaction and auto-retry, queue delivery default, update checks, thinking level), Models (pi's startup defaults are editable — default provider, model, and thinking level — and provider sign-in status comes with actions: one-click sign-in runs `pi auth login` in the built-in terminal, sign-out deletes the stored credential and applies to running sessions immediately), and Skills & Extensions (toggle each skill on or off and reveal its folder; disable builtin extensions, remove configured extensions and packages, and add an npm:/git:/https source or a local path at user or project scope). These edits land in pi's config files; running sessions pick them up after pi restarts, new sessions immediately

## 前置条件 / Prerequisites

- **pi CLI**(0.85+):`powershell -c "irm https://pi.dev/install.ps1 | iex"`(Windows)或 `curl -fsSL https://pi.dev/install.sh | sh`(macOS/Linux),需要 Node.js 22.19+
- **认证**:在终端运行 `pi`,执行 `/login`(或 `pi auth login <provider>`)。pido 不经手凭据,pi 自己读取 `~/.pi/agent/auth.json`
- **Linux/macOS 从桌面图标或 AppImage 启动时**:桌面会话不继承终端里的 `PATH`,而安装器通常把 `pi` 写进 `~/.pi/agent/bin`、`~/.local/bin` 这类只在 shell 里可见的目录。pido 会先查 `PATH`,再查这些常见目录;仍找不到,或报「pi 存在但无法运行」(多半是 `node` 不在桌面会话的 `PATH` 里),可用 `PIDO_PI_PATH=/完整路径/pi` 启动 pido 指定可执行文件
- **Rust 1.98+**(仅构建时需要)

English: a desktop launch (a `.desktop` entry or the AppImage) does not inherit
your terminal's `PATH`, and pi's installer writes to directories like
`~/.pi/agent/bin` or `~/.local/bin` that only a shell sees. pido searches
`PATH` and then those well-known directories; if pi is still not found, or the
banner says it is installed but will not run (usually a `node` the session
cannot see), start pido with `PIDO_PI_PATH=/full/path/to/pi`.

## 下载 / Download

打 `v*` 标签会构建并发布 GitHub Release。**当前仅发布 Linux AppImage**(Windows/macOS 的打包在签名证书就绪后恢复):

- `pido-x.y.z-linux-x64.AppImage` — `chmod +x` 后直接运行

Windows 与 macOS 用户请暂从源码构建(`cargo run --release -p pido`)。

每个产物都附带同名 `.sha256`,应用内「立即更新」(仅 AppImage)下载后据此校验。

**发版检查单**:两个 crate 的 `Cargo.toml` `version` 必须与标签一致(应用据此判断自身版本),然后 `git tag vX.Y.Z && git push origin vX.Y.Z`。

English: a `v*` tag currently publishes a **Linux AppImage only** — the
Windows zip and the macOS `.app` bundle return once signing certificates
are in place; build from source meanwhile. Every artifact ships with a
`.sha256` sibling, which the in-app update verifies. **Before tagging**,
set `version` in both crates' `Cargo.toml` to the tag's number — the app
reads it to know what it is.

## 运行 / Run

```bash
cargo run --release -p pido
```

### Linux

需要 wayland/x11 开发库(见 CI 中的依赖列表),例如 Ubuntu:

```bash
sudo apt install libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev libxcb1-dev \
  libxrandr-dev libxi-dev libxinerama-dev libxcursor-dev libegl1-mesa-dev libfontconfig1-dev
```

（较新的 Debian/Ubuntu 上 fontconfig 的开发包可能叫 `libfontconfig-dev`。）

### 自动连接(测试用) / Autoconnect (for testing)

```bash
PIDO_AUTOCONNECT=/path/to/project cargo run -p pido
```

## 架构 / Architecture

```
crates/
├── pi-rpc/    Pi RPC 协议客户端库(无 UI 依赖)
│   ├── protocol.rs   命令/响应/事件/扩展UI 的 serde 类型(未知类型向前兼容)
│   ├── client.rs     子进程管理、严格 JSONL 分帧、按 id 关联请求、订阅者分发
│   ├── sessions.rs   ~/.pi/agent/sessions 扫描(按项目分组、名称、预览)
│   └── tests/        用内置 mock-pi 二进制做端到端协议测试
└── pido/      GPUI 应用
    ├── state.rs      AppState:RPC 事件折叠为会话转录;阻塞操作全部走后台执行器
    ├── ui/           app(根视图)/ sidebar / chat(转录+头部)/ composer / welcome
    ├── i18n.rs       中英文案(Ely I18n)
    └── settings.rs   %APPDATA%/pido/settings.json 持久化
```

**协议要点**:pido 以 `pi --mode rpc` 子进程方式集成,stdin/stdout 交换 JSONL(见 pi 文档 `rpc.md`)。响应按 `id` 关联;`prompt` 成功仅代表已接受,完成信号是 `agent_settled` 事件;stdout 由专职线程持续排空以避免管道背压卡死;关闭 stdin 即优雅退出。Windows 上自动解析 npm 安装的 `pi.cmd`(Rust 标准库不做 PATHEXT 解析)。

**Protocol notes**: pido integrates by spawning `pi --mode rpc` and exchanging JSONL over stdio. Responses correlate by `id`; a successful `prompt` only means *accepted* — completion is the `agent_settled` event; stdout is drained continuously to avoid pipe backpressure; closing stdin shuts pi down gracefully. On Windows the npm-installed `pi.cmd` is resolved explicitly (std does no PATHEXT lookup).

## 测试 / Tests

```bash
cargo test --workspace     # 单元 + mock-pi 端到端协议测试
cargo run -p pi-rpc --example real_pi -- /tmp   # 对真实 pi 的 get_state 冒烟
PI_SMOKE_PROMPT="hi" cargo run -p pi-rpc --example real_pi -- /tmp   # 完整流式往返
```

## 状态与路线 / Status

MVP(核心聊天客户端)已完成,扩展 UI 原生对话框(权限审批类扩展)、队列可视化、会话重命名、改动审查(右侧栏 diff)、内置终端、会话分支树、多会话并行(每会话独立 pi 进程)均已支持。checkpoint 回溯仍不可行(pi 的 RPC 无法移动 leaf)。

依赖固定:`ely-gpui-component` 锁定 rev `e0f355a`,`gpui-pre =0.3.11`;升级视为显式变更。

多会话并行已落地为「侧栏即任务管理器」:不设 tab 栏,切走的会话在后台继续跑,列表行带运行中 spinner 与未读点。早期 tab 栏方案的设计文档见 [docs/multi-tab.md](docs/multi-tab.md)(已被否,留作决策记录)。Parallel sessions shipped as a sidebar-driven slot model; the earlier tab-strip design doc is kept as a decision record.

## License

MIT
