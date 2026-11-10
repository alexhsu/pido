# pido

Pi Coding Agent 的原生桌面客户端：项目会话、流式输出与工具卡片、运行中转向、模型/思考级别切换、历史会话恢复；Windows/macOS/Linux，中英双语，亮暗主题。

A native desktop client for the [Pi coding agent](https://github.com/earendil-works/pi) — Rust + [GPUI](https://github.com/zed-industries/zed) + [Ely GPUI Components](https://github.com/ELY-Open-Source/Ely-GPUI-Components). Pick a project, stream replies with thinking and tool cards, steer mid-run, switch models and thinking levels, resume past sessions. Windows, macOS, Linux, bilingual 中文/English, light/dark.

![status](https://img.shields.io/badge/status-early-orange) ![platforms](https://img.shields.io/badge/platforms-Windows%20%7C%20macOS%20%7C%20Linux-blue)

## 功能 / Features

- **完整聊天闭环** — 选择项目目录启动会话,流式输出、思考过程、工具调用卡片实时呈现
- **运行中转向** — agent 执行中继续输入即 steer(转向);排队中的消息以标签显示在输入框上方,可一键清空;Esc 或停止按钮 = 清空队列 + 中止
- **模型与思考级别切换** — 输入框右下角的模型胶囊,两级菜单切换 provider 模型和 thinking level
- **会话管理** — 侧栏按项目分组列出 `~/.pi/agent/sessions` 的历史会话,支持折叠、悬浮新建、拖拽调宽;点击恢复继续,可重命名当前会话
- **统计** — token 用量、成本、上下文占用,每轮结束后刷新
- **亮暗主题 + 中英双语** — 一键切换,偏好持久化(含窗口尺寸、最近项目)
- **A complete chat loop** — pick a project directory, stream replies, thinking, and tool-call cards live
- **Steering** — typing while the agent runs steers it; queued messages show as chips over the composer and can be cleared in one click; Esc or the stop button clears the queue and aborts
- **Model & thinking switching** — the model pill at the composer's right opens two levels: model and thinking level
- **Sessions** — the sidebar lists past sessions grouped by project, with folding, a hover new-chat button and a draggable width; click to resume, rename the open session from the header
- **Stats** — tokens, cost, and context usage refresh after every run
- **Dark/light + 中文/English** — one-click toggle, persisted along with window size and last project

## 前置条件 / Prerequisites

- **pi CLI**(0.85+):`powershell -c "irm https://pi.dev/install.ps1 | iex"`(Windows)或 `curl -fsSL https://pi.dev/install.sh | sh`(macOS/Linux),需要 Node.js 22.19+
- **认证**:在终端运行 `pi`,执行 `/login`(或 `pi auth login <provider>`)。pido 不经手凭据,pi 自己读取 `~/.pi/agent/auth.json`
- **Rust 1.98+**(仅构建时需要)

## 运行 / Run

```bash
cargo run --release -p pido
```

### Linux

需要 wayland/x11 开发库(见 CI 中的依赖列表),例如 Ubuntu:

```bash
sudo apt install libwayland-dev libxkbcommon-dev libxcb1-dev libxrandr-dev libxi-dev libxinerama-dev libxcursor-dev libegl1-mesa-dev
```

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

MVP(核心聊天客户端)已完成,扩展 UI 原生对话框(权限审批类扩展)、队列可视化、会话重命名均已支持。明确不在当前范围:bash 面板、内置终端、diff 审查、checkpoint 回溯、多 pi 进程并发。

依赖固定:`ely-gpui-component` 锁定 rev `e0f355a`,`gpui-pre =0.3.11`;升级视为显式变更。

## License

MIT
