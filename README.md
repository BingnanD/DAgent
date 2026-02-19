# DAgent

DAgent 是一个 Rust 实现的终端多 Agent 协作客户端。
它通过本地 CLI 调起 `claude` / `codex`，在同一会话里支持单 Agent、多 Agent 并行协作、流式渲染、会话记忆与可恢复状态。

## 给后续 LLM 的 60 秒导览

- **主入口**：`src/main.rs`
  - 负责终端初始化（raw mode / bracketed paste / inline viewport）
  - 检测可用 Provider（`claude`、`codex` 是否在 `PATH`）
  - 启动 `app::run_app`
- **应用层状态机**：`src/app/`
  - `App` 保存 UI/会话/运行状态
  - `runtime.rs` 是事件循环（输入、渲染、worker 轮询）
  - `input.rs` 处理按键、历史、补全、审批交互
  - `commands.rs` 处理 `/primary` `/theme` `/mem` `/clear`
  - `worker.rs` 消费 `WorkerEvent` 并更新转录区与活动区
  - `render.rs` + `ui.rs` 完成日志渲染与 composer UI
- **调度层**：`src/orchestrator.rs`
  - 普通消息：根据 dispatch target 路由到一个或多个 Provider
  - slash 命令：`/help` `/commands` `/tool ...` `/skill ...`
- **Provider 适配层**：`src/providers/`
  - `claude.rs`：解析 `stream-json`，提取 chunk/tool/progress
  - `codex.rs`：解析 `--json` 事件流，提取 chunk/progress
- **记忆层**：`src/memory.rs`
  - SQLite + FTS5，按 `session_id` 保存 user/assistant 消息
  - 构建上下文时采用“最近消息 + 检索命中”混合策略
- **Skills 层**：`src/skills.rs`
  - 本地 JSON 持久化（`~/.dagent/skills/*.json`）
  - CRUD、显式引用解析（`@skill:<id>`）、关键词匹配

---

## 核心能力

- 单 Agent 对话（primary provider）
- 多 Agent 并行分发（`@claude @codex`）
- 实时流式输出与活动日志（tool/progress）
- 高风险工具调用审批（`/tool bash ...`）
- 会话记忆检索（`/mem show/find/prune/clear`）
- Skills 管理与 agent 驱动生成（`/skill create/update`）
- 对话时自动注入相关 Skills 上下文（显式引用优先）
- 会话快照持久化与恢复（provider/theme/history/transcript）

## 架构与数据流

```text
Keyboard/Paste
   |
   v
App(input.rs) -------------------------+
   | submit_current_line               |
   v                                   |
commands.rs (local commands)           |
   |                                   |
   +----> orchestrator::execute_line --+--> providers::{claude,codex}
                    |                          |
                    | WorkerEvent channel      | spawn child process
                    +--------------------------+
                                   |
                                   v
                              app/worker.rs
                                   |
                  +----------------+----------------+
                  |                                 |
            transcript entries                 activity log
           (User/Assistant/System)            (Tool/Progress)
                  |
                  v
            memory.rs (SQLite/FTS)
```

## 目录说明

```text
src/
  main.rs                # 进程入口、终端初始化、provider 检测
  orchestrator.rs        # 调度入口、slash 命令执行、并发 provider 运行
  memory.rs              # SQLite 记忆存储与上下文构建
  skills.rs              # Skills 存储、CRUD、检索与引用解析
  providers/
    mod.rs               # provider 分发 + primary 自动切换策略
    claude.rs            # Claude CLI 流式解析
    codex.rs             # Codex CLI 流式解析
  app/
    mod.rs               # App 状态定义与公共方法
    runtime.rs           # 主事件循环 + 绘制节流 + scrollback flush
    input.rs             # 键盘/粘贴/补全/审批模式处理
    commands.rs          # UI 层命令（/primary /theme /mem /clear）
    dispatch.rs          # @mention 路由解析
    worker.rs            # WorkerEvent -> UI 状态更新
    render.rs            # 转录内容渲染、markdown 样式、换行处理
    ui.rs                # Composer/Activity/Status/Modal 绘制
    session.rs           # session.json 读写、上下文拼接
    text.rs              # 运行时文本净化（控制字符/ANSI）
    types.rs             # Provider/Theme/EntryKind/WorkerEvent
    tests.rs             # UI/流式/分发/粘贴/滚动等回归测试
```

## 运行与安装

### 前置条件

- Rust 工具链（`cargo`）
- 至少一个 Provider CLI 可执行并在 `PATH` 中：
  - `claude`
  - `codex`

### 开发运行

```bash
cargo run
```

### 安装到本地

```bash
./build_install.sh
```

可选参数：

- `--debug`：debug 构建
- `--skip-tests`：跳过 `cargo test`
- `--bin-name <name>`：指定安装后的二进制名

## 命令与交互

### 会话命令

- `/help`：查看帮助
- `/commands`：列出命令示例
- `/clear`：清空当前转录并清空当前 session memory
- `/exit` 或 `/quit`：退出

### 路由与主题

- `/primary [claude|codex]`：切换主 Agent
- `/provider [claude|codex]`：`/primary` 的别名
- `/theme [fjord|graphite|solarized|aurora|ember]`

### 记忆

- `/mem`：显示摘要与用法
- `/mem show [n]`：查看最近 n 条
- `/mem find <query>`：全文检索
- `/mem prune [keep]`：只保留最近 keep 条
- `/mem clear`：仅清空 memory（不清 transcript）

### 工具

- `/tool echo <text>`
- `/tool time`
- `/tool bash <command>`

`/tool bash`、`/tool shell`、`/tool exec` 会触发高风险审批弹窗（一次允许或永久允许）。

### Skills

- `/skill`：显示 Skills 摘要与用法
- `/skill list`：列出所有 Skills
- `/skill show <id>`：查看 Skill 详情
- `/skill create <name> <intent>`：由 agent 生成并创建 Skill（双 agent 可协同）
- `/skill update <id> <intent>`：由 agent 更新 Skill
- `/skill delete <id>`：删除 Skill

在普通对话中可显式引用：`@skill:<id>`。  
未显式引用时，系统会按关键词自动匹配并注入最多 3 个相关 Skills。

### Dispatch Override（消息级路由）

- `@claude <task>`
- `@codex <task>`
- `@claude @codex <task>`

实现细节：

- `@mention` 可出现在句子中间，会被解析并从 prompt 文本中移除

### 键位

- `Enter`：发送
- `Shift+Enter`：换行
- `Tab` / `Shift+Tab`：补全候选循环
- `PgUp` / `PgDn`：转录滚动
- `Ctrl+R`：历史搜索
- `Esc`：取消当前运行任务
- `Ctrl+C`：中断并退出

## 事件模型（WorkerEvent）

`src/app/types.rs` 中定义：

- `AgentStart(provider)`
- `AgentChunk { provider, chunk }`
- `AgentDone(provider)`
- `Tool { provider, msg }`
- `Progress { provider, msg }`
- `PromotePrimary { to, reason }`
- `Done(final_text)`
- `Error(err)`

关键约束：

- `Tool` / `Progress` 默认只进活动区，不写入 transcript
  - 多 Agent 协同运行时，协调事件与去重后的进度会以 `[coord]` / `[progress]` 形式写入 transcript
- `AgentChunk` 才会持续填充 assistant 面板
- 运行结束后 assistant 文本才会写入 memory

## 持久化与本地文件

- `~/.dagent/session.json`
  - 保存：`primary_provider`、`theme`、`entries`、`history`、`session_id`
- `~/.dagent/memory.db`
  - 表：`messages`
  - FTS：`messages_fts`
- `~/.dagent/skills/*.json`
  - 每个 Skill 一个 JSON 文件（`id/name/description/content/timestamps`）

恢复逻辑：

- 有 memory 后端时，默认**不恢复 transcript**（避免大转录影响启动）；可用环境变量开启
- 无 memory 后端时，默认恢复 transcript

## 环境变量

- `DAGENT_INLINE_HEIGHT`
  - composer inline viewport 高度（默认约 12 行）
- `DAGENT_RESTORE_TRANSCRIPT`
  - `1/true/yes/on` 时启动恢复 transcript
- `DAGENT_DECOMPOSE`
  - 多 Agent 任务分解开关（默认开启）；设为 `0/false/no/off` 时跳过分解，直接并行分发原始任务
- `DAGENT_CLAUDE_PERMISSION_MODE`
  - Claude permission mode（默认 `bypassPermissions`，root 下自动回退 `acceptEdits`）
- `DAGENT_CLAUDE_ALLOWED_TOOLS`
  - Claude allowed tools（默认 `Bash`）
- `DAGENT_CODEX_APPROVAL_POLICY`
  - Codex `--ask-for-approval`（默认 `never`）
- `DAGENT_CODEX_SANDBOX`
  - Codex `exec -s` 模式（默认 `danger-full-access`）

## Provider 细节

### Claude

- 主调用：`claude --print --output-format stream-json --include-partial-messages ...`
- 解析：
  - `content_block_delta.text_delta` -> `AgentChunk`
  - `tool_use` / `tool_result` / `system` -> `Tool/Progress`
- 错误处理：
  - quota/rate limit 文本识别（`is_quota_error_text`）
  - 若 Claude 因 quota 失败且 Codex 可用，可触发 primary 自动切换

### Codex

- 主调用：`codex exec --json ...`
- 解析：
  - `item.completed(agent_message)` -> `AgentChunk`
  - `session.started/turn.started/item.started/item.completed` -> `Progress`
- 回退：JSON 流失败时，退化到非 `--json` 模式一次性输出

## 上下文构建策略

优先使用 `memory.rs::build_context`：

1. 取最近消息（`RECENT_LIMIT=2`）
2. 对当前 prompt 进行词项归一化后做 FTS 检索（`SEARCH_LIMIT=8`）
3. 合并、去重、裁剪到字符上限（`CONTEXT_CHAR_LIMIT=2000`）
4. 包装成：
   - `Shared session memory:`
   - `Current user request:`

memory 不可用时回退到 transcript 近邻拼接（`session.rs`）。

随后会尝试注入 Skills 上下文（`session.rs` + `skills.rs`）：

1. 优先解析显式引用（`@skill:<id>`）
2. 不足时按关键词匹配相关 Skills
3. 截断后附加到最终 prompt（最多 3 个 Skills）

## 渲染与 UI 约定

- assistant 行使用固定 label 列：`claude │ ...` / `codex │ ...`
- 内置轻量 markdown 渲染：标题、列表、粗体、斜体、行内代码、代码块
- 运行中的活动区展示：
  - 每个 Agent 的呼吸灯 + verb + elapsed + chars
  - 最近 tool/progress 日志（限长）

## 回归测试覆盖重点

`src/app/tests.rs` 覆盖了以下高风险行为：

- 滚动与 autoscroll 切换
- tool/progress 不污染 transcript
- 多 Agent chunk 定向写入
- dispatch override 解析（多 agent mention 组合）
- streaming flush 差分与渲染稳定性
- 大段粘贴折叠与派发前还原
- memory backend 不可用路径

## 扩展指南（新增 Provider）

1. 在 `src/app/types.rs` 的 `Provider` 增加枚举项、`as_str()`、`binary()`。
2. 在 `src/providers/<new>.rs` 实现 `run_stream(provider, prompt, tx, child_pids)`。
3. 在 `src/providers/mod.rs` 分发到新 Provider。
4. 如需 CLI 进度可视化，按 `WorkerEvent::Progress` 输出。
5. 在 `main.rs` 的 provider 发现与命令提示中加入新 Provider。
6. 增加对应测试，至少覆盖：可用性检测、chunk 解析、错误回退、取消中断。

## 设计边界与注意事项

- DAgent 依赖外部 CLI；若 `PATH` 无 `claude`/`codex`，仅本地 slash 命令可用。
- 当前 memory 仅做会话内结构化回忆，不做长期知识库治理。
- `/skill create|update` 需要至少一个可用 Provider（`claude` 或 `codex`）。
- `/tool bash` 具有执行风险；默认需要显式审批。
- transcript append 策略偏保守，优先避免“中间重排导致的错位写入”。
