# 多标签页与多 pi 进程

调研时间:2026-11-19(v0.4.0 之后)
状态:**设计文档,未实施**
范围:让 pido 同时打开多个项目、多个会话,每个会话跑独立的 `pi` 进程

## 1. 结论

**可行**。协议层(`crates/pi-rpc`)天然支持多实例,不需要改动;重构量集中在 UI 层的"单状态假设"上。按四期推进,总量与 v0.3 一个量级。

硬约束两条:

1. **并行必须多进程** —— pi 的 RPC 模式一个进程只绑定一个活跃会话,没有并发 run 的概念(见 §2)。所以"多任务并行"只有多进程一条路。
2. **同一会话文件禁止双开** —— 两个 pi 进程 append 同一 `.jsonl` 会交错破坏 `parentId` 链。这是唯一的硬正确性红线,必须在切换/分叉/克隆入口按 `session_key` 查重。

## 2. 根本约束:为什么不能单进程多会话

从 pi 1.5.2 的 RPC 面核实(`dist/modes/rpc/rpc-mode.js` 与 `docs/rpc-commands.md`):

- `get_state` 只有单数标志:`isStreaming`、`isCompacting`,没有"多个 run"的概念;
- `switch_session` 是**重绑**:把 pi 的运行时换到另一个会话文件,原会话的 run 不会继续;
- `get_tree` 只读:能看出树,不能把 leaf 指到别的节点;
- 队列(`steer`/`follow_up`)属于当前活跃 run,不是跨会话调度。

因此一个 pi 进程 = 一个前台工作会话。要并行两个项目,就是两个进程 —— 这与用户在多个终端里各开一个 `pi` 的心智模型完全一致。

## 3. 协议层:已就绪,零改动

逐项核对过 `crates/pi-rpc`:

| 检查点 | 结论 | 依据 |
|---|---|---|
| 多实例共存 | 每实例独立的子进程、stdin/stdout 管道、排空线程;唯一的全局态是 `REQUEST_COUNTER: AtomicU64`,只用来生成跨实例唯一的请求 id,无共享状态 | `client.rs:39`、`client.rs:370` |
| 生命周期 | Drop 按**句柄计数**关闭(`handle_count.fetch_sub(..) == 1` 才 shutdown),每个标签页的 pi 在自己的 `AppState` 被释放时优雅退出,互不影响 | `client.rs:353` |
| 事件泵 | `start_pump(generation, client)` 只订阅**自己的** client;generation 校验在 `AppState` 内部,不同 tab 的 pump 互不串扰 | `state.rs` `start_pump` |
| 会话文件 | 各 tab 各自 `cwd`/会话文件,pi 侧无共享写;唯一冲突是两个进程指向同一文件(见 §7) | — |

**资源成本(本机实测)**:空闲 `pi --mode rpc` ≈ **100 MB**(Windows node 工作集 94.7–99.9 MB)。2–4 个标签页完全可接受。另有两点说明:

- 开内置终端时,每个 tab 再加一个 ConPTY shell(内存小得多);
- 每个 pi 子进程都会加载完整 Node 运行时 + 扩展,**冷启动**有时间成本,适合"打开一个 tab 就留着",不适合频繁开关。

## 4. UI 层:单实例假设盘点

好消息:**渲染函数早已参数化**——`chat`/`composer`/`sidebar`/`panel` 共 17 处签名是 `state: &Entity<AppState>`,不假设单一状态。需要动的分三类。

### ① 纯搬运:`PidoApp` 字段二分

`PidoApp`(`ui/app.rs:44`)现在是"窗口壳 + 一个会话"的混合体。按职责二分:

**留在壳(窗口级)**:

- `left_px` / `right_px` / `snap_since` / `snap_flip` / `pane_save_at`(窗格宽度)
- `focus`(壳的 FocusHandle)、`window_size`
- `search_field`(侧栏搜索框,全局一个侧栏)
- `right_open` 的持久化(`right_tab`/`diff_*`/`terminal` 搬到 tab,见下)

**搬进每个 `TabView`(会话级)**:

- `state: Entity<AppState>`、`draft`、`composer_focused`
- 对话框:`rename_open` / `rename_field` / `rename_target` / `rename_project` / `confirm_remove` / `dialog_field` / `dialog_field_for`
- 右侧栏内容:`right_tab`、`right_open`、`diff_selected`、`diff_layout`、`terminal`
- 转录滚动:`transcript_list`、`list_epoch`、`transcript_scroll`
- 查找/面板:`find`、`palette`、`palette_gen`、`palette_selected`、`session_seen`

搬完后 `PidoApp` 变成 `Shell`,持 `Vec<Tab>`。

### ② 共享资源上移(小重构)

| 资源 | 现状 | 问题 | 处理 |
|---|---|---|---|
| `save_setting`(`settings.rs:109`) | load → apply → save 全文件覆写,**无跨进程/跨实体锁**;Windows 路径还有 remove+rename 的非原子窗口 | 两个 tab 并发写会互相丢更新 | 加 `static Mutex<()>` 串行化;写入本身已是同目录临时文件 |
| `session_scan`(`state.rs:1150`) | 每个 `AppState` 一份 `Arc<Mutex<SessionScanner>>` | N 个 tab = N 份扫描缓存 + 重复扫描 | 提升为全局 `SessionIndex` 实体,壳持有,侧栏读它 |
| `environment`(`state.rs:1013`,探测见 `probe_environment` `state.rs:1264`) | 每 tab 探测一次 | 每次探测 = `pi --version` + 最多 6×`pi auth check` 个 Node 进程 | 上移到壳,启动探测一次,tab 读共享结果 |
| `update`(`state.rs:1044`,`check_for_updates` `state.rs:3624`) | `AppState` 字段 + 下载逻辑 | 两个 tab 会各自检查、各自下载同一文件 | 上移到壳;`UpdateStatus` 与下载进度都是窗口级 |
| 未读/流式状态 | `AppState.streaming` 等 | 壳需要读所有 tab 的画角标 | 壳 observe 各 tab 的轻量快照 |

**最大的一块**是 `SessionIndex` 抽取:`state.rs` 里侧栏/会话相关函数约 44 个(`sidebar_rows_now` `state.rs:1680`、`refresh_sessions` `state.rs:3037`、`merge_user_projects` `state.rs:5239`、`session_tree` `state.rs:419` 等)。好在它们基本都是**纯数据操作**且有测试护着,边界清晰。

### ③ 行为正确性(必须做对)

1. **同一会话文件单开**:按 `session_key`(大小写折叠的规范路径)查重。已打开的会话:侧栏点击 = 切到该 tab;右键「在新标签页打开」在已打开时不可用;`fork`/`clone`/`new_session` 完成后登记新文件。分叉产生的新文件天然不同,不冲突。
2. **动作路由**:`Stop`/`Find`/`Palette`/`PinSession`/`RenameSession`/`ForkSession`/`ArchiveSession`/`ctrl-f`/`ctrl-k` 现在注册在 `PidoApp` 的监听上(`ui/app.rs` 的 `on_action` 链,键位在 `main.rs:23`)。壳是 tab 内容的祖先,action 冒泡到壳后**路由给活跃 tab**:壳上的 handler 把动作转发给 `active_tab.update(..)`,tab 自己的 handler 保持现状(它们已是 `cx.listener`)。
3. **焦点**:每个 `TabView` 一个 `FocusHandle`;切 tab 时 `window.focus(&tab.focus, cx)`;composer 自动聚焦逻辑随之搬进 tab。
4. **后台 tab 的渲染成本**:壳只 `observe` **活跃** tab 的 state;切 tab 时重建订阅(或在观察者里按 `active_epoch` 早退,后者更简单,推荐)。后台 tab 的事件泵与转录折叠照常运行(数据不丢),但**绘制成本≈0**——gpui 不为不在渲染树里的视图出帧。注意事件泵里的 `cx.notify()` 仍会通知它自己的观察者,所以"不重绘"靠的是壳不订阅而非泵停止。
5. **Esc 与终端**:现有逻辑在 `Stop` 里判断终端聚焦并写 `ESC` 到 PTY。搬到 tab 后逻辑不变,只是查的是活跃 tab 的 `terminal`。

## 5. 目标架构

```
main.rs
 └─ Shell(瘦身后的 PidoApp)
     ├─ 窗口级:窗格宽度 / 焦点 / tab strip / 全局侧栏(SessionIndex)
     │          / Environment(探测一次) / 更新管理 / settings 锁
     └─ Vec<Tab>
         ├─ 视图状态:今天 PidoApp 的会话级字段(见 §4①)
         └─ Entity<AppState> → 独立 PiClient → 独立 pi 进程
```

- **侧栏保持全局一个**:列出所有项目的会话(现状不变);tab strip 放在聊天列顶部(或与 header 同一行,二选一,实现时看排布)。
- **"打开"语义**:侧栏点会话 = 在活跃 tab 打开(若无 tab = 新建);右键「在新标签页打开」= 新 tab;欢迎页选项目 = 新 tab。
- **空壳**:零 tab 时显示现在的欢迎页。
- **未读指示**:壳读各 tab 的 `streaming`/`turn_running`/notices 摘要,在 tab 上画 spinner/圆点。
- **tabs 持久化(M3)**:settings 增加 `open_tabs: Vec<String>`(会话文件路径)+ `active_tab`,启动恢复。

## 6. 分期与工作量

| 期 | 内容 | 规模 |
|---|---|---|
| **M0** | 抽 `TabView`、壳持 `Vec<Tab>` 恒为 1,**行为零变化**;作为后续重构的回归基线,可单独发 v0.4.x | ~1 次会话 |
| **M1** | `save_setting` 加锁、`SessionIndex` 全局化、`Environment`/`Update` 上移 | 1–2 次 |
| **M2** | tab strip UI、创建入口(侧栏「+」/右键「在新标签页打开」/欢迎页)、切换焦点、**同会话单开守卫**、未读标记、`Ctrl+Tab`/`Ctrl+W` | 2–3 次 |
| **M3** | tabs 持久化恢复、中键关闭、后台完成角标、(可选)拖拽排序 | ~1 次 |

比 v0.4.0 大一档,与 v0.3 相当。

## 7. 风险与注意事项

1. **`SessionIndex` 抽取**是最大 refactor。缓解:`sidebar_rows_now` 等已是纯函数 + 单测覆盖;M0 先建立回归基线再动。
2. **渲染重入规则延续**:render/paint 路径上不 `view.read(cx)`。Shell + Tab 两层后规则不变;`TranscriptScroll` 的"独立实体"模式照搬即可。切 tab 时 `observe` 的早退判断也属于这条。
3. **供应商限速**:多个并行 run 共享同一 provider 配额(deepseek 等),属用户预期内,文档提一句即可。
4. **同会话双开**:§4③ 的守卫必须与 `finish_fork`/`switch_session`/`clone` 的收尾点同步,否则会出现"两个 tab 都以为自己拥有该会话"。
5. **未签名发布/CI**:与本特性无关,但 CI 三平台矩阵会覆盖新代码;沿用"push 前本地 `cargo clippy --workspace --all-targets -- -D warnings`"的做法(`--all-targets` 不能省)。

## 8. 考虑过并放弃的替代方案

| 方案 | 为什么不选 |
|---|---|
| **多窗口**(每窗独立 `PidoApp`) | 改动更小(每窗天然独立),但任务栏杂乱、无统一侧栏、与"标签页"的诉求不符 |
| **单进程多会话** | pi 侧不支持并行 run(§2),根本不可行 |
| **多 tab 共用同一 pi 进程轮流 switch** | 切换会中断另一 tab 的 run,失去并行的全部意义;且状态(队列/思考级别)会互相污染 |

## 9. 关键代码位置索引

| 位置 | 内容 |
|---|---|
| `crates/pi-rpc/src/client.rs:353` | `PiClient` 的 Drop 句柄计数关闭 |
| `crates/pi-rpc/src/client.rs:39` | 唯一全局态 `REQUEST_COUNTER` |
| `crates/pido/src/ui/app.rs:44` | `PidoApp` 结构体(字段二分见 §4①) |
| `crates/pido/src/ui/app.rs:16` | `actions!` 声明 |
| `crates/pido/src/main.rs:23` | 全局键位绑定 |
| `crates/pido/src/settings.rs:109` | `save_setting`(需加锁) |
| `crates/pido/src/state.rs:1150` | `session_scan` 字段(需全局化) |
| `crates/pido/src/state.rs:1264` | `probe_environment`(需上移) |
| `crates/pido/src/state.rs:1044` | `update: Option<UpdateStatus>`(需上移) |
| `crates/pido/src/state.rs:1680` / `3037` / `5239` | 侧栏相关的纯函数,`SessionIndex` 的原料 |
