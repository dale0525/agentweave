# Secretary Agent Reference App

English | [简体中文](./README.zh-CN.md)

This reference application demonstrates that the AgentWeave Framework is composable; it is not a special mode built into the core Runtime. Its application identity, Chinese system prompt, Memory, Mail, and custom `secretary-routines` Skill are all supplied through App package configuration.

The default Mail implementation is a local Fake Connector that requires no account or credentials. All write operations still pass through the Runtime's approval and idempotency boundaries. Memory uses a local SQLite provider with the same scope as the conversation database.

To connect a real mailbox, follow the repository's [IMAP/SMTP setup guide](../../MAIL_CONNECTOR_SETUP.md) to create an account configuration and store the password in the Credential Vault. See `mail-account.example.json` for an example; it contains only an opaque secret ID, never a password.

Validate from the repository root:

```bash
pixi run scaffold-agent-app -- --validate examples/secretary-agent
pixi run check-skills
```

Start the local Server and Desktop development page together:

```bash
AGENTWEAVE_APP_ROOT=examples/secretary-agent pixi run dev
```

Start only the Server:

```bash
AGENTWEAVE_APP_ROOT=examples/secretary-agent pixi run server
```

The development page is available at <http://127.0.0.1:5173>, and the Server health check is available at <http://127.0.0.1:49321/health>. Fake Mail and local Memory require no account, but an actual conversation still requires a reachable model endpoint.

Generate a frozen release artifact:

```bash
pixi run package-agent-app -- \
  --input examples/secretary-agent \
  --output output/secretary-agent-release \
  --runtime-version 0.1.0
```

To develop another secretary-style or vertical Agent App, copy this directory, replace its `appId`, brand, and prompts, and add or remove Foundation or app-local packages. You do not need to modify the turn loop in `crates/agent-runtime`.
