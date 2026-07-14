# Secretary Agent Reference App

这是一个用于证明 AgentWeave Framework 可组合性的参考应用，不是核心 Runtime 的特殊模式。应用身份、中文 system prompt、Memory、Mail 和 `secretary-routines` 自定义 Skill 都通过 App package 配置提供。

默认 Mail 实现是本地 Fake Connector，不需要账号或凭据；所有写操作仍然经过 Runtime 的审批和幂等边界。Memory 使用与会话数据库同 scope 的本地 SQLite provider。

需要连接真实邮箱时，按仓库根目录的 [IMAP/SMTP 配置指南](../../MAIL_CONNECTOR_SETUP.md) 创建账号配置并把密码写入 Credential Vault。示例配置见 `mail-account.example.json`；该文件只包含 opaque secret ID，不包含密码。

从仓库根目录验证：

```bash
pixi run scaffold-agent-app -- --validate examples/secretary-agent
pixi run check-skills
```

同时启动本地 Server 与 Desktop 开发页面：

```bash
AGENTWEAVE_APP_ROOT=examples/secretary-agent pixi run dev
```

只启动 Server：

```bash
AGENTWEAVE_APP_ROOT=examples/secretary-agent pixi run server
```

开发页面位于 <http://127.0.0.1:5173>，Server 健康检查位于 <http://127.0.0.1:49321/health>。Fake Mail 和本地 Memory 不需要账号，但实际对话仍需要配置可用的模型端点。

生成可发布的冻结产物：

```bash
pixi run package-agent-app -- \
  --input examples/secretary-agent \
  --output output/secretary-agent-release \
  --runtime-version 0.1.0
```

要开发另一个秘书类或垂直 Agent App，可以复制本目录，替换 `appId`、品牌和 prompts，并增删 Foundation 或应用本地 packages；不需要修改 `crates/agent-runtime` 的 turn loop。
