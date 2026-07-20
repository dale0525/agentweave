# AgentWeave

[English](./README.md) | 简体中文

**在可复用的 Runtime 上构建有自己品牌的 Agent App，不必从头重写 Agent 循环、权限、存储和宿主集成。**

AgentWeave 是面向产品与工程团队的开源 **Agent App Framework**。它提供 Agent 应用背后的共用基础设施：多轮对话、模型接入、Skills、Connectors、审批、凭据、持久化、后台运行，以及 Desktop、Android、Server 三类宿主。你的产品团队负责目标用户、用户旅程、Prompt、启用的能力、Provider 选择、策略、品牌和最终界面。

AgentWeave 不是一个现成助手，不是托管 SaaS，也不是无代码 App Builder。仓库中的秘书只是参考应用，不是框架的固定产品形态。

> [!IMPORTANT]
> AgentWeave 仍处于 `0.1.x` 阶段，适合原型、框架开发和受控试点。Manifest、Host API、Foundation Skill 契约、Provider 覆盖和跨平台行为仍可能变化。在没有完成应用级安全与数据评审前，不要使用生产凭据，也不要执行高风险外部操作。

## AgentWeave 是否适合你的产品？

### 值得评估的情况

- 你要构建个人助手、研究工具、内容工作流、企业内部 Agent 或其他主要由应用配置和扩展定义的产品。
- 你希望使用同一份 App 契约描述多个宿主上的 Prompt、能力、策略、语言、主题和私有 Skill。
- 你需要用确定性的权限、审批、凭据、持久化和幂等边界约束模型驱动的行为。
- 你的团队有工程师负责模型与工作空间 Provider 集成、完整用户旅程测试、发布和运维。
- 你可以先使用 fake/local Provider，再通过受控试点逐步引入真实账号。

### 以下情况暂不应把它作为默认选择

- 非技术产品负责人必须在没有工程师的情况下独立创建并发布 App。
- 你希望框架直接提供生产 SLA、托管云、计费、组织管理或客户支持。
- 产品上线依赖所有能力在所有宿主上稳定且完全一致；目前多数 Foundation 能力仍是 Preview。
- 你需要现成的 iOS、Windows 或面向最终用户的公开 Web App；当前没有这些发布目标。
- 第一版就会执行高风险外部操作或处理受监管数据，但团队还无法完成安全、隐私、Provider 和恢复评审。

### 框架提供什么，产品团队还要负责什么

| AgentWeave 提供 | 你的 App 团队负责 |
| --- | --- |
| Agent Runtime、模型协议适配、会话、事件与持久化 | 产品定义、用户旅程、验收标准和模型质量评估 |
| 版本化 App Manifest、Prompt、可选 packages、主题、字体和本地化 | 品牌、文案、信息架构、最终交互设计和无障碍评审 |
| Foundation Skill 契约、fake/local 实现、Connector 与 Host Tool 边界 | 实际发布哪些能力、接入哪些 Provider，以及真实账号如何连接 |
| 凭据、审批、权限、审计和幂等基础机制 | 威胁模型、隐私披露、数据保留、Provider 条款和生产安全评审 |
| Desktop、Android、Server 参考宿主与打包工具 | 分发、签名凭据、部署、监控、支持、升级和事故响应 |

## 先选择你的入口

| 你的目标 | 从这里开始 |
| --- | --- |
| 判断 AgentWeave 能否支撑一个产品想法 | 阅读[当前能力与平台覆盖](#当前能力与平台覆盖)和[从产品想法到交付](#从产品想法到交付) |
| 证明本地 Runtime 能运行 | 按照[技术快速开始](#技术快速开始)操作 |
| 创建独立的品牌化 Agent App | 阅读[开发 Agent App](./DEVELOPING_AGENT_APPS.zh-CN.md) |
| 体验一条完整的本地工作流 | 使用 Fake Mail 和本地 Memory 运行 [Secretary Agent](./examples/secretary-agent/README.zh-CN.md) |
| 连接 Google Workspace、Microsoft 365 或 IMAP/SMTP | 阅读 [Provider Adapters](./crates/provider-adapters/README.md) 与 [Mail Connector 配置](./MAIL_CONNECTOR_SETUP.zh-CN.md) |
| 构建并发布 macOS 应用 | 阅读 [macOS Desktop 打包](./DESKTOP_PACKAGING.zh-CN.md) |
| 构建受管模型与身份链路 | 参考 [Managed Gateway Agent](./examples/managed-gateway-agent/README.zh-CN.md) |
| 为框架本身贡献代码 | 阅读[贡献指南](./CONTRIBUTING.zh-CN.md) |

## 当前能力与平台覆盖

[`catalog/foundation-skills.json`](./catalog/foundation-skills.json) 是 package 状态的机器可读权威来源。下表把它转换成产品规划视角。

| 能力领域 | Catalog 状态 | 声明支持的宿主 | 当前可用实现或集成 |
| --- | --- | --- | --- |
| Filesystem | Stable | Desktop、Server | 经过批准的本地 Workspace 访问 |
| Memory | Stable | Android、Desktop、Server | 持久、可审计、按 App 隔离的 Memory |
| Skill Creator | Stable，仅开发者 | Desktop、Server | Skill 编写与校验，不是消费者 App 默认能力 |
| Mail | Preview | Android、Desktop、Server | 确定性 Fake Mail 与 IMAP/SMTP；Server 侧提供 Google Workspace 和 Microsoft 365 Adapter |
| Calendar | Preview | Android、Desktop、Server | fake/local 覆盖；Server 侧提供 Google Calendar 与 Microsoft Graph Adapter |
| Contacts | Preview | Android、Desktop、Server | fake/local 覆盖；Server 侧提供 Google 与 Microsoft Graph Adapter |
| Tasks and Reminders | Preview | Desktop、Server | 本地任务状态和受审批约束的修改 |
| Web Research、Documents、Scheduler、Notifications、Structured Content | Preview | Desktop、Server | 框架契约和确定性 fake/local 覆盖；仍可能需要产品或 Provider 集成 |
| Notes and Messaging | Preview | Android、Desktop、Server | Provider-neutral Connector 契约；你的 App 需要核实或提供真实 Provider Adapter |

“声明支持的宿主”表示 package 契约包含该平台，不代表所有宿主已经拥有相同 UI、Provider 覆盖或生产成熟度。Preview 表示 package 已实现并通过仓库校验，但 API、Provider 支持和跨平台行为仍可能变化。

### 宿主与发布目标

| 目标 | 当前角色 | 重要边界 |
| --- | --- | --- |
| macOS Desktop | Electron Host、受管本地 Rust sidecar、自包含打包、签名与公证工作流 | 正式分发仍需要你的 Developer ID、Apple 凭据、发布测试和更新运维 |
| Android | Kotlin Compose Host、Rust FFI 与冻结 App 资源 | 需要项目本地 Android SDK/NDK；必须针对所选 App 验证能力与体验一致性 |
| Server | 本地 HTTP Host、开发诊断、后台运行和自定义受管宿主集成 | 仓库自带的开发 Server 不是开箱即用的公开 SaaS 部署 |
| Linux | `pixi.toml` 声明支持的开发环境 | 尚未提供面向最终用户的 Linux Desktop 发布流程 |
| iOS、Windows、公开 Web App | 当前不属于发布目标 | 下游产品需要自行新增并维护这些 Host，或选择其他交付表面 |

## 从产品想法到交付

用下面的顺序把产品设想变成证据。不要把“仓库能启动”当成“产品已经可交付”。

| 阶段 | 决策或交付物 | 典型负责人 |
| --- | --- | --- |
| 1. 产品适配 | 目标用户、关键旅程、必需能力、目标 Host 和不可接受的操作 | 产品与设计 |
| 2. 技术验证 | Minimal App 能启动，一个真实模型能完成 turn，必需 packages 能解析 | App 工程 |
| 3. App 定义 | 通过校验的 Manifest、Prompt、语言、品牌、策略和私有 Skills | 产品、设计与 App 工程 |
| 4. Provider 方案 | 明确模型、身份、邮件/日历/联系人 Provider、scope、账号隔离与降级行为 | 集成与安全工程 |
| 5. 安全本地证明 | fake/local Provider 覆盖成功、拒绝、审批、重试、冲突、重启和重复操作路径 | 工程与 QA |
| 6. 受控试点 | 使用专用测试账号验证真实 Provider，不接触生产数据或宽泛权限 | 产品、QA、安全与法务 |
| 7. 发布候选 | 冻结 App lock、可安装 Host 包、签名、干净设备 smoke、恢复方案与已知限制 | 发布工程 |
| 8. 运营 | 监控、备份、升级、支持责任、事故响应、数据导出与删除 | 产品运营与安全 |

合理的第一次批准应是有时间边界、出口条件明确的技术原型。生产决策还需要 Provider 覆盖、模型质量、数据处理、故障恢复、平台行为、分发、维护成本和许可证证据。

## 系统如何协作

```text
用户
  |
  v
Desktop / Android / Server Host
  UI + 身份 + 凭据 + 审批 + 平台能力
  |
  v
Agent Runtime ----------------------> Model Gateway ------> 模型 Provider
  |                                         |
  |                                         v
  |                                      模型响应
  |
  +--> Skills 描述任务行为
  |
  +--> Runtime / Host Tools 执行确定性的本地工作
  |
  +--> 审批 --> Connector --> 外部账号或服务
  |
  +--> App-scoped 存储、事件、Memory、Tasks 与 Durable Runs
```

模型可以提出行动，但不能给自己授权。凭据保留在 Host 控制的存储中；外部副作用始终受 Runtime/Host 策略、审批和幂等检查约束。

### 用产品语言理解术语

| 框架术语 | 对 App 创建者意味着什么 |
| --- | --- |
| Agent App | 产品专属定义：身份、行为、能力、策略、品牌、语言和私有 packages |
| App Manifest（`agent-app.json`） | 告诉各 Host“App 需要什么、适用什么策略”的版本化契约 |
| Prompt | 产品团队编写的人格与行为指令；永远不是权限边界 |
| Skill | Agent 处理某类任务的可复用方法，可附带资源和受控工具 |
| Connector | 在认证、scope、审批和审计约束下确定性地访问外部账号或服务 |
| Host Tool | Desktop、Android、Server 或其他 Host 提供的可信能力 |
| Host | 拥有 UI、凭据、审批和平台集成的应用外壳 |
| Runtime | 运行 turn、解析工具与 packages、应用策略并持久化状态的共用引擎 |
| Foundation Skill | Memory、Mail 等面向通用能力的可选第一方 package |

## 技术快速开始

这一入口面向工程师。它先证明本地 Host 与 Runtime 链路，再接入模型和真实外部账号。

### 1. 准备环境

安装 Git 与 [Pixi](https://pixi.prefix.dev/latest/)。项目环境会管理 Rust、Node.js、Python、OpenJDK 和其他命令行依赖。当前声明的开发平台是 macOS Apple Silicon、macOS Intel 和 Linux x86_64。

```bash
git clone https://github.com/dale0525/agentweave.git
cd agentweave

pixi install
pixi run npm --prefix apps/desktop ci
```

Android 开发还需要[贡献指南](./CONTRIBUTING.zh-CN.md#android-environment)中说明的项目本地 SDK/NDK。

### 2. 校验仓库与示例 App

```bash
pixi run validate-agent-assets
pixi run test-dev-script
```

这些检查不需要模型 Key 或外部账号。

### 3. 启动 Minimal App

```bash
AGENTWEAVE_APP_ROOT=examples/minimal-agent pixi run dev
```

- Desktop 开发页面：<http://127.0.0.1:5173>
- Server 健康检查：<http://127.0.0.1:49321/health>

页面能够加载并且健康检查返回 `ok`，说明本地外壳已经工作；这**还不能**证明 AI turn 或外部 Connector 可用。按 `Ctrl+C` 停止两个进程。

### 4. 完成第一次真实对话

打开 **Settings → Model**，填写：

- **Base URL**：Provider 的 API 根路径。AgentWeave 会根据所选协议追加 `/responses`、`/chat/completions` 或 `/completions`。例如 Provider 文档给出的地址是 `https://model.example/v1/chat/completions`，这里填写 `https://model.example/v1`。
- **Endpoint type**：Responses、Chat Completions 或 Completions，必须与 Provider 实际支持的协议一致。
- **Model name**：该端点接受的准确模型标识。
- **API key**：本地端点可以不需要，托管 Provider 通常需要。Desktop 会先用 Electron safe storage 加密，再在本地持久化输入的 Key。

发送消息前先选择 **Test connection**。测试失败通常意味着 Base URL 路径多了一段或少了一段、Endpoint type 不匹配、模型名不存在或鉴权失败。请先使用开发凭据，不要把生产 Key 输入尚未评审的构建。

AgentWeave 还支持由 App 管理模型接入，并替换身份、权益和 Gateway Provider。当最终用户不应自行配置模型端点时，参考 [Managed Gateway Agent](./examples/managed-gateway-agent/README.zh-CN.md)。

### 5. 创建独立 App

```bash
pixi run scaffold-agent-app -- \
  --name "Research Agent" \
  --app-id com.example.research-agent \
  --output output/research-agent

pixi run scaffold-agent-app -- --validate output/research-agent
AGENTWEAVE_APP_ROOT=output/research-agent pixi run dev
```

生成的 App 包含：

```text
output/research-agent/
  agent-app.json          身份、兼容性、依赖需求和策略
  prompts/                system 与 developer 行为
  locales/                随包分发的界面语言
  themes/                 可选自定义主题
  fonts/                  可选打包字体
  packages/               App 私有 Skills 与 packages
```

Manifest 中不同部分回答不同问题：

| 部分 | 回答的问题 |
| --- | --- |
| `appId`、`package`、`compatibility` | 这是什么 App，接受哪个 Runtime 版本，哪些 Host 可以加载？ |
| `requires` | 必须存在哪些 packages、capabilities、Runtime Tools 和 Connectors？ |
| `policy` | 是否允许外部副作用、网络、后台运行、Memory 和 Skill 管理？ |
| `instructions` | 哪些 system、developer 和附加 Prompt 资源定义行为？ |
| `branding`、`appearance`、`localization` | 用户最终看到什么名称、主题、字体和语言？ |

从脚手架生成的 deny-by-default 策略开始，只添加已选 package 真正要求的声明。可对照完整的 [Minimal Agent Manifest](./examples/minimal-agent/agent-app.json)和[开发 Agent App 指南](./DEVELOPING_AGENT_APPS.zh-CN.md)。

## 模型、数据与安全边界

- App 与 package 请求能力，Host 与 Runtime 决定实际授予范围。
- Prompt 和外部文档不能授予权限，也不能绕过审批。
- 凭据保留在 Host 控制的存储中，通过有 scope 的 opaque ID 引用，不应复制进 Manifest、Prompt、package 或普通日志。
- 发送邮件、修改日历等外部写操作必须遵循契约声明的 Durable Approval 与幂等保护。
- 默认测试使用 fake service 或本地存储；真实 Provider 测试与试点必须使用专用账号显式启用。
- 加密本地备份保护导出的 SQLite backup envelope，不等于所有运行中数据库或任意 Workspace 文件都已加密。声明静态加密或恢复能力前，请阅读[本地数据保护与备份](./DATA_PROTECTION.zh-CN.md)。
- 模型与 Connector Provider 可能接收用户数据。Provider 条款、数据位置、保留、删除、用户同意、日志和事故响应仍由你的产品团队负责。

## 打包与发布分别意味着什么

这里有两类不同产物：

1. `package-agent-app` 生成经过哈希锁定的冻结 App 定义。它是 Host 构建的输入，本身不是最终用户可安装的应用。
2. Host 打包流水线把 App 定义、Runtime 和平台外壳组合成可安装或可部署的产品。

创建并校验冻结 App 定义：

```bash
pixi run package-agent-app -- \
  --input output/research-agent \
  --output output/research-agent-release \
  --runtime-version 0.1.0

pixi run package-agent-app -- --verify output/research-agent-release
```

构建自包含 macOS App：

```bash
pixi run package-macos \
  --input output/research-agent \
  --output dist/macos/research-agent \
  --overwrite
```

本地构建使用 ad-hoc 签名，只适合开发。正式 macOS 分发需要 Developer ID 签名、公证、干净设备测试、发布元数据，以及你自己的凭据和运维流程。Android 会在 `pixi run android-assemble` 时打包所选 App 定义。Server 部署仍属于自定义受管 Host 的责任，不是一个命令即可获得的托管服务。

## 仓库地图

```text
apps/                     Desktop 与 Android Hosts
crates/agent-runtime/     turn、会话、工具、策略、存储与 packages
crates/model-gateway/     模型端点与流式协议适配
crates/agent-server/      本地 HTTP Host、诊断与后台运行
crates/provider-adapters/ Google Workspace 与 Microsoft 365 Adapters
skills/                   内置、Foundation 与开发者 Skills
catalog/                  Foundation Skill 与主题的机器可读 Catalog
examples/                 可运行的 Agent App 参考实现
templates/agent-app/      deny-by-default App 脚手架
scripts/                  校验、打包与开发工具
```

特定产品行为应进入 App、Skill、Connector、Provider Adapter 或示例；只有可复用的协议、状态模型、安全边界和 Host 基础设施才应进入核心 Runtime。

## 文档导航

- [开发 Agent App](./DEVELOPING_AGENT_APPS.zh-CN.md)：Manifest、Prompt、Skill、主题、字体、本地测试与冻结发布。
- [Secretary Agent](./examples/secretary-agent/README.zh-CN.md)：使用本地实现组合 Mail、Memory、审批与 App 私有工作流。
- [Managed Gateway Agent](./examples/managed-gateway-agent/README.zh-CN.md)：App 管理的身份、权益、模型接入与 Cloudflare 部署。
- [Provider Adapters](./crates/provider-adapters/README.md)：Google Workspace 与 Microsoft 365 OAuth 和 Connector 覆盖。
- [Mail Connector 配置](./MAIL_CONNECTOR_SETUP.zh-CN.md)：IMAP/SMTP、TLS、Credential Vault 与 live smoke 边界。
- [macOS Desktop 打包](./DESKTOP_PACKAGING.zh-CN.md)：自包含 App 构建、签名、公证与发布验证。
- [本地数据保护与备份](./DATA_PROTECTION.zh-CN.md)：加密导出、恢复、密钥分离和明确不包含的内容。
- [对话生命周期](./CONVERSATION_LIFECYCLE.zh-CN.md)与[流式 Turn 生命周期](./STREAMING_TURN_LIFECYCLE.zh-CN.md)：持久历史、流式运行、停止与恢复契约。
- [贡献指南](./CONTRIBUTING.zh-CN.md)：架构归属、环境、测试与 Pull Request 要求。

## License

除单独标识的第三方材料外，AgentWeave 可由你选择按 [Apache License 2.0](./LICENSE-APACHE) 或 [MIT License](./LICENSE-MIT) 双重许可使用。[LICENSE](./LICENSE) 说明贡献条款，[NOTICE](./NOTICE) 记录仓库级归属信息。

第三方 Skills、脚本、主题、协议、Connectors、依赖和资产保留各自的许可证与版权声明；再分发时请保留 package 内的许可证与 notice 文件。
