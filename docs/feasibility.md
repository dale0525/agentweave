# General App Agent 可行性与架构评估

日期：2026-06-25

## 结论

可行，但不建议把 `openai/codex`、`fathah/hermes-desktop` 和 `farion1231/cc-switch` 粗暴拼成一个应用。

更稳妥的路线是：

- 用 `openai/codex` 作为 agent runtime 的设计参考和局部代码来源，复用它的 turn loop、事件协议、工具调用循环、skill/plugin/MCP 思路。
- 用 `hermes-desktop` 作为桌面客户端和会话 UI 的主要参考，复用 Electron + React 客户端结构、IPC 边界、聊天流式渲染、多 session 管理方式。
- 用 `cc-switch` 作为模型网关和协议适配层的主要代码来源，复用 provider adapter、Responses 到 Chat Completions 的转换、Chat streaming 到 Responses SSE 的转换、provider routing/failover。

GeneralAgent 的产品定位调整为“为开发者服务的 agent 应用框架”。开发者在开发阶段通过本地 skill 包、Codex 内置 `skill-creator`、以及后续 SDK/脚手架扩展 agent 能力；打包后 skill inventory 被固定为应用内部能力，对终端用户不可见。用户只通过自然对话表达意图，由 runtime 自动选择和调用内置能力。

推荐架构可以概括为：

```text
Hermes-like Desktop Client
        |
        | IPC / local HTTP / WebSocket
        v
General App Agent Runtime
  - session / conversation
  - agent turn loop
  - skill registry
  - tool execution
        |
        v
Model Gateway / Adapter Layer
  - Responses adapter
  - Chat Completions adapter
  - Completion adapter
  - provider routing / failover
        |
        v
OpenAI-compatible / local / third-party providers
```

MVP 的关键不是“能不能做”，而是边界要收住：先做一个不操控宿主机的 app agent runtime，再把宿主机 shell、文件系统、sandbox、OpenAI 账号体系、Codex CLI/TUI、Hermes installer/SSH/office/wallet 等都排除在外。

## MVP 范围

MVP 只需要覆盖这四件事：

1. 用户可以配置 OpenAI-compatible 模型接口。
2. 支持 `responses`、`chat/completions`、基础 `completion` 三类上游形态。
3. 用户发送信息后，agent 可以循环思考、自动调用打包内置 skill/tool、继续请求模型，直到回复用户。
4. 支持多 session / conversation，并能恢复历史。

MVP 明确不做：

- 不做宿主机系统操控。
- 不做 shell command、patch、sandbox、approval workflow。
- 不绑定 OpenAI 登录、OpenAI 官方 API 或 Codex 服务端。
- 不做远程 SSH / gateway / installer / wallet / office 等 Hermes 扩展能力。
- 不追求完整兼容 Codex CLI、TUI 或现有 Codex App。

## 参考项目复用边界

许可证初步判断：

- `openai/codex`：Apache-2.0。
- `fathah/hermes-desktop`：MIT。
- `farion1231/cc-switch`：MIT。

这三者对商业化和二次开发都比较友好。后续如果直接复制源文件，建议在对应文件头或 `NOTICE`/第三方声明里保留来源、许可证和修改说明。

### openai/codex

当前本地参考版本：`.tool/openai-codex`，HEAD `51864b0`。

最有价值的部分：

- `codex-rs/core/src/session/turn.rs`
  - agent turn loop 的核心参考。
  - 模型返回 function call 时执行工具，再把结果送回模型继续采样。
  - 模型返回 assistant message 时记录历史并结束当前 turn。
- `codex-rs/core/src/tools`
  - tool registry、tool router、tool runtime 的结构值得参考。
- `codex-rs/core/src/session`
  - session 生命周期、turn context、事件发送方式值得借鉴。
- `codex-rs/protocol`
  - 前后端事件协议、message/item 类型、plan/reasoning/tool events 可作为协议蓝本。
- skill/plugin/MCP 注入机制
  - 值得复用理念，MVP 可做简化版。

需要谨慎或不建议复用的部分：

- 当前 Codex 的 provider wire API 已经基本只支持 Responses。
  - `codex-rs/model-provider-info/src/lib.rs` 中 `WireApi` 只有 `Responses`。
  - `wire_api = "chat"` 会被显式拒绝。
- CLI/TUI、OpenAI login、sandbox、shell tool、patch tool、unified exec 都和本项目目标相反。
- Codex core 和 OpenAI Responses API、Codex App 协议耦合较深，直接拿整个 core 会带来较高拆解成本。

建议复用方式：

- 先不要把 Codex core 当成依赖整体嵌入。
- 以“Codex-inspired runtime”的方式重建一个更小的 core。
- 复制或移植其中稳定、边界清晰的协议和循环结构。
- 如果后续要更深复用，再把 `turn.rs` 周边能力拆成可裁剪 crate。

### fathah/hermes-desktop

当前本地参考版本：`.tool/hermes-desktop`，HEAD `d677824`。

最有价值的部分：

- Electron + React + TypeScript 桌面客户端结构。
- `src/preload/index.ts` 和 `src/preload/index.d.ts`
  - preload API、IPC 暴露方式、前后端边界可参考。
- `src/renderer/src/screens/Chat`
  - `Chat.tsx`、`MessageList.tsx`、`MessageRow.tsx`、`ChatInput.tsx`、streaming/reasoning/tool events 的 UI 模型可参考。
- `src/renderer/src/screens/Sessions`
  - session 列表、恢复、搜索等交互可参考。
- `src/main/sessions.ts`、`session-cache.ts`、`session-continuation-store.ts`
  - 本地会话存储、恢复、continuation 逻辑可参考。
- `src/main/skills.ts`
  - skill 列表、安装、读取的客户端管理体验可参考。

需要谨慎或不建议复用的部分：

- Hermes 自身 installer、Hermes CLI/Python 环境管理。
- SSH/remote、office、wallet、messaging platform、cronjobs、gateway 中和 Hermes 产品强绑定的能力。
- 大量业务屏幕和配置项不适合直接迁入 MVP。

建议复用方式：

- 以 Hermes 的桌面壳和聊天体验为主要参考。
- 复制/移植 Chat、Sessions、Settings、Skills 相关 UI 和 IPC 模式。
- 后端 main process 不直接沿用 Hermes 业务逻辑，而是连接新的 General App Agent Runtime。

### farion1231/cc-switch

当前本地参考版本：`.tool/cc-switch`，HEAD `6fd4e6f`。

这是补齐 Codex 协议缺口的关键项目。

最有价值的部分：

- `src-tauri/src/proxy/providers/adapter.rs`
  - `ProviderAdapter` trait 提供 provider 统一接口。
  - 覆盖 base URL、auth、URL 构建、请求/响应转换。
- `src-tauri/src/proxy/providers/transform_codex_chat.rs`
  - OpenAI Responses request 到 Chat Completions request 的转换。
  - 已处理 tools、namespace tools、custom tools、tool_search、message/history、reasoning、usage 等细节。
- `src-tauri/src/proxy/providers/streaming_codex_chat.rs`
  - Chat Completions SSE 到 Responses SSE events 的状态机转换。
  - 对流式 text、reasoning、tool call、usage、finish reason 都有现成处理。
- `src-tauri/src/proxy/providers/codex_chat_common.rs`
  - Responses/Chat 桥接共享 helper。
- `src-tauri/src/proxy/provider_router.rs`
  - provider routing、failover、circuit breaker 思路可复用。
- `src-tauri/src/proxy/forwarder.rs`、`response_processor.rs`
  - HTTP forwarding 与响应处理可参考。

需要谨慎或不建议直接复用的部分：

- cc-switch 是面向多个现有 agent/client 的代理工具，不是 agent runtime。
- UI、Tauri app、特定 provider 账号体系、OAuth 反代等不一定属于 MVP。
- 代码中已经有很多适配复杂边界，MVP 可以先选最小 provider subset。

建议复用方式：

- 把 cc-switch 的 provider adapter 和 Codex Chat bridge 作为 Model Gateway 的起点。
- 保留 Rust 版本的转换逻辑优先级高于重写 TypeScript 版本，因为它已经覆盖很多边界。
- 如果客户端采用 Electron，也可以让 Model Gateway 作为 Rust sidecar，通过 local HTTP/WebSocket 暴露给 Electron main process。

## 推荐技术路线

推荐采用“Electron 客户端 + Rust runtime/gateway sidecar”的组合：

- 客户端：Electron + React + TypeScript，参考 Hermes Desktop。
- Agent runtime：Rust，参考 Codex core 的 session/turn/tool 模型。
- Model gateway：Rust，直接复用或裁剪 cc-switch 的 proxy/provider/transform 模块。
- 存储：SQLite。
- 前后端通讯：MVP 用 local HTTP + WebSocket；后续如需要更紧密桌面集成，再增加 Electron IPC wrapper。

这样做的原因：

- Codex 和 cc-switch 的关键代码都是 Rust，直接复用成本低。
- Hermes Desktop 是 Electron，适合快速得到成熟桌面客户端体验。
- runtime 作为 sidecar 可以保持客户端和 agent core 解耦。
- 后续如果要提供 Web 客户端、CLI、SDK，同一个 runtime 可以继续复用。

备选路线：

- 全 TypeScript：开发快，但 cc-switch 和 Codex 的关键能力需要重写，长期风险更高。
- Tauri + React：和 cc-switch 技术栈更近，但 Hermes Desktop 的 Electron 客户端代码复用会下降。
- 直接 fork Codex：短期看似省事，但剥离 OpenAI/Codex/shell/sandbox/CLI/TUI 的成本大。

## 核心模块设计

### Model Gateway

职责：

- 读取 provider 配置。
- 把 runtime 的统一模型请求转换到目标 provider 协议。
- 支持 native Responses、Chat Completions、基础 Completion。
- 支持 streaming event 标准化。
- 支持 provider routing、failover、健康检查。

建议内部统一事件先向 Responses-like item/event 靠拢，因为：

- Codex turn loop 原本就是围绕 Responses item/event 设计。
- Responses 的 tool call / function call / reasoning 语义更接近 agent runtime。
- Chat Completions 可以通过 cc-switch 转换桥接。

但对外配置不要强行要求用户理解 Responses。用户只需要配置：

- provider name
- endpoint type: `responses` / `chat_completions` / `completion`
- base URL
- API key 或自定义 headers
- model name
- streaming enabled
- optional routing/failover priority

### Agent Runtime

职责：

- 管理 session 和 conversation。
- 接收用户消息，构造模型输入。
- 注入 system instruction、session context、enabled skills/tools。
- 发起模型请求并消费 streaming events。
- 遇到 tool call 时执行 skill/tool。
- 把 tool result 加入上下文继续循环。
- 收到最终 assistant message 后结束 turn。
- 把消息、tool call、tool result、reasoning summary、usage 写入存储。

MVP loop 可以简化为：

```text
user message
  -> build prompt
  -> model stream
  -> if assistant text: append and finish
  -> if tool call: execute skill/tool
  -> append tool result
  -> continue loop
  -> stop on final response / max steps / cancellation / error
```

MVP 必须有这些保护：

- `max_agent_steps`
- `max_tool_calls_per_turn`
- `max_context_tokens` 或粗略字符预算
- cancellation
- tool timeout
- structured error event

### Skill 系统

MVP 里的 skill 不需要等同 Codex 插件系统，可以先做“即插即用的本地目录 skill”：

```text
skills/
  weather/
    skill.json
    README.md
    index.js 或 command
```

Skill 是开发者扩展点，不是终端用户配置项。开发模式可以扫描 `skills/` 目录并支持调试诊断；打包模式必须读取冻结的 skill bundle/index。生产 UI 和生产 API 默认不暴露 skill 列表、开关或 marketplace。

建议 manifest：

```json
{
  "name": "weather",
  "description": "Query weather for a city.",
  "version": "0.1.0",
  "entry": {
    "type": "command",
    "command": "node",
    "args": ["index.js"]
  },
  "tools": [
    {
      "name": "get_weather",
      "description": "Get weather by city.",
      "input_schema": {
        "type": "object",
        "properties": {
          "city": { "type": "string" }
        },
        "required": ["city"]
      }
    }
  ]
}
```

MVP skill registry 需要：

- 扫描 skill directory。
- 校验 manifest。
- 暴露 tool schema 给 agent runtime。
- 执行 tool call。
- 返回 JSON tool result。

后续可以扩展：

- MCP server skill。
- WebAssembly skill。
- HTTP skill。
- npm/pixi/python 环境隔离。
- 权限声明。
- marketplace/install/update。

### Session / Conversation

MVP 建议 SQLite 表：

- `sessions`
  - `id`
  - `title`
  - `created_at`
  - `updated_at`
  - `active_model_profile_id`
  - `metadata_json`
- `messages`
  - `id`
  - `session_id`
  - `role`
  - `content_json`
  - `created_at`
  - `turn_id`
- `turns`
  - `id`
  - `session_id`
  - `status`
  - `started_at`
  - `finished_at`
  - `usage_json`
  - `error_json`
- `tool_calls`
  - `id`
  - `turn_id`
  - `tool_name`
  - `arguments_json`
  - `result_json`
  - `status`
  - `started_at`
  - `finished_at`
- `model_profiles`
  - `id`
  - `name`
  - `endpoint_type`
  - `base_url`
  - `model`
  - `auth_ref`
  - `headers_json`

客户端只展示 session/message 抽象，不直接依赖 provider 协议。

## 主要风险与缓解

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| Codex core 与 OpenAI Responses/Codex App 耦合深 | 直接复用成本高 | 先复用设计和小模块，不整体嵌入 |
| Chat/Responses 工具语义不完全一致 | tool call loop 容易出边界 bug | 复用 cc-switch 已有转换与 streaming 状态机 |
| Skill 执行权限不清晰 | 插件生态风险高 | MVP 默认本地显式安装、禁用宿主机敏感能力、加 timeout |
| 多 session 恢复与 streaming 状态复杂 | UI 容易重复消息或丢消息 | 参考 Hermes session continuation 测试与事件模型 |
| Completion API 能力弱 | 很难支持可靠 tool calling | MVP 标为 text-only 或通过 prompt contract 做实验性支持 |
| 过早复制全部参考项目 | 代码体积膨胀，维护困难 | 只复制 Chat/Sessions/Skills UI、turn loop 骨架、gateway adapter |

## MVP 里程碑

### M0：文档和骨架

- 建立 repo 结构。
- 固化架构文档和 Superpowers 实施计划。
- 确定 Electron + React + Rust sidecar 路线。
- 加入 `.tool/` 到 `.gitignore`。

### M1：Model Gateway

- 支持 provider profile CRUD。
- 支持 native Chat Completions streaming。
- 支持 native Responses streaming。
- 从 cc-switch 移植 Responses↔Chat bridge。
- 输出统一 runtime event。

### M2：Agent Runtime

- 建立 session、turn、message 存储。
- 实现最小 agent loop。
- 支持 tool call 执行和继续采样。
- 支持 cancellation、max steps、tool timeout。

### M3：Skill MVP

- 本地目录 skill discovery。
- manifest schema validation。
- command-based tool execution。
- 示例 skill。

### M4：Desktop Client

- Hermes-like Chat UI。
- Session list / restore / delete / rename。
- Model profile settings。
- 不提供终端用户 skill management screen；后续只在 dev mode 增加 skill validation/diagnostics。
- Runtime event streaming display。

### M5：打磨与验证

- 端到端测试：chat provider、responses provider、tool call、session restore。
- 手动测试桌面客户端。
- 错误恢复和日志。
- 打包策略。

## 代码复用清单

优先复用：

- Codex:
  - `codex-rs/core/src/session/turn.rs` 的循环结构。
  - `codex-rs/core/src/tools` 的 registry/router 思路。
  - `codex-rs/protocol` 的事件和 item 命名。
- Hermes Desktop:
  - `src/renderer/src/screens/Chat/*`
  - `src/renderer/src/screens/Sessions/*`
  - `src/renderer/src/screens/Skills/*`
  - `src/preload/index.ts`
  - session store 相关 main process 文件的结构。
- cc-switch:
  - `src-tauri/src/proxy/providers/adapter.rs`
  - `src-tauri/src/proxy/providers/transform_codex_chat.rs`
  - `src-tauri/src/proxy/providers/streaming_codex_chat.rs`
  - `src-tauri/src/proxy/providers/codex_chat_common.rs`
  - `src-tauri/src/proxy/provider_router.rs`
  - `src-tauri/src/proxy/forwarder.rs`
  - `src-tauri/src/proxy/response_processor.rs`

暂不复用：

- Codex shell/sandbox/patch/approval/CLI/TUI/OpenAI login。
- Hermes installer/SSH/office/wallet/messaging/cronjobs。
- cc-switch Tauri UI、OAuth 反代、非 MVP provider 特性。

## 推荐的第一步

第一步不要马上写客户端 UI。

建议先实现一个 headless runtime/gateway 骨架：

1. `POST /sessions`
2. `POST /sessions/:id/messages`
3. `GET /sessions/:id/events` 或 WebSocket stream
4. `GET /sessions`
5. `POST /model-profiles`
6. Dev-only: `GET /dev/skills` / `POST /dev/skills/validate`，生产包默认关闭。

只要 headless runtime 能跑通“用户消息 -> 模型 -> tool call -> tool result -> 模型 -> assistant reply -> session restore”，客户端就可以按 Hermes Desktop 的方式接入。
