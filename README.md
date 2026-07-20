# AgentWeave

English | [简体中文](./README.zh-CN.md)

**Build a branded Agent App on a reusable runtime instead of rebuilding the Agent loop, permissions, storage, and host integration from scratch.**

AgentWeave is an open-source **Agent App Framework** for product and engineering teams. It provides the shared machinery behind an Agent application: multi-turn conversations, model access, Skills, Connectors, approvals, credentials, persistence, background work, and Desktop, Android, and Server hosts. Your product supplies the audience, user journey, prompts, enabled capabilities, provider choices, policies, branding, and final interface.

AgentWeave is not a finished assistant, a hosted SaaS, or a no-code App builder. The secretary in this repository is a reference application, not the framework's fixed product form.

> [!IMPORTANT]
> AgentWeave is still at `0.1.x`. It is a good fit for prototypes, framework development, and controlled pilots. Manifests, Host APIs, Foundation Skill contracts, provider coverage, and cross-platform behavior may change. Do not use production credentials or high-risk external actions without an application-specific security and data review.

## Is AgentWeave a fit for your product?

### Good reasons to evaluate it

- You are building a personal assistant, research tool, content workflow, internal enterprise Agent, or another product whose behavior should come mainly from App configuration and extensions.
- You want one App contract to describe prompts, capabilities, policies, languages, themes, and private Skills across more than one Host.
- You need deterministic permission, approval, credential, persistence, and idempotency boundaries around model-driven behavior.
- You have engineers who can integrate model and workspace providers, test the complete user journey, and own release and operations.
- You can begin with fake or local providers, then introduce real accounts in a controlled pilot.

### It is not yet the default choice when

- A non-technical product owner must create and publish the App without engineers.
- You need a production SLA, managed cloud hosting, billing, organization administration, or customer support from the framework itself.
- Your launch depends on stable parity across every capability and Host. Most Foundation capabilities are still Preview.
- You require a ready-made iOS, Windows, or public browser-hosted end-user application. Those release targets are not currently provided.
- Your first release will perform high-risk external actions or process regulated data before you can complete security, privacy, provider, and recovery reviews.

### What the framework provides—and what your team still owns

| AgentWeave provides | Your App team owns |
| --- | --- |
| Agent runtime, model protocol adapters, sessions, events, and persistence | Product definition, user journeys, acceptance criteria, and model quality evaluation |
| Versioned App Manifest, prompts, optional packages, themes, fonts, and localization | Brand, copy, information architecture, final interaction design, and accessibility review |
| Foundation Skill contracts, fake/local implementations, Connector and Host Tool boundaries | Which capabilities ship, which providers are connected, and how real accounts are onboarded |
| Credential, approval, permission, audit, and idempotency primitives | Threat model, privacy disclosures, retention policy, provider terms, and production security review |
| Desktop, Android, and Server reference Hosts plus packaging tools | Distribution, signing credentials, deployment, monitoring, support, upgrades, and incident response |

## Choose your path

| Your goal | Start here |
| --- | --- |
| Decide whether AgentWeave can support a product idea | Read [current capability and platform coverage](#current-capability-and-platform-coverage) and the [delivery path](#from-product-idea-to-delivery) |
| Prove the local runtime works | Follow the [technical quick start](#technical-quick-start) |
| Create a separate branded Agent App | Follow [Developing Agent Apps](./DEVELOPING_AGENT_APPS.md) |
| Study a complete local workflow | Run the [Secretary Agent](./examples/secretary-agent/README.md) with Fake Mail and local Memory |
| Connect Google Workspace, Microsoft 365, or IMAP/SMTP | Read [provider adapters](./crates/provider-adapters/README.md) and [Mail Connector Setup](./MAIL_CONNECTOR_SETUP.md) |
| Build and release a macOS application | Read [macOS Desktop Packaging](./DESKTOP_PACKAGING.md) |
| Build a managed model and identity path | Study the [Managed Gateway Agent](./examples/managed-gateway-agent/README.md) |
| Contribute to the framework itself | Read the [Contributing Guide](./CONTRIBUTING.md) |

## Current capability and platform coverage

The Foundation Catalog at [`catalog/foundation-skills.json`](./catalog/foundation-skills.json) is the machine-readable source of package status. The table below translates it into a product-planning view.

| Capability area | Catalog status | Declared Hosts | Available backing or integration |
| --- | --- | --- | --- |
| Filesystem | Stable | Desktop, Server | Approved local workspace access |
| Memory | Stable | Android, Desktop, Server | Persistent, auditable App-scoped Memory |
| Skill Creator | Stable, developer-only | Desktop, Server | Skill authoring and validation; not a consumer default |
| Mail | Preview | Android, Desktop, Server | Deterministic Fake Mail and IMAP/SMTP; Server adapters for Google Workspace and Microsoft 365 |
| Calendar | Preview | Android, Desktop, Server | Fake/local coverage; Server adapters for Google Calendar and Microsoft Graph |
| Contacts | Preview | Android, Desktop, Server | Fake/local coverage; Server adapters for Google and Microsoft Graph |
| Tasks and Reminders | Preview | Desktop, Server | Local task state and approval-aware mutations |
| Web Research, Documents, Scheduler, Notifications, Structured Content | Preview | Desktop, Server | Framework contracts and deterministic fake/local coverage; provider or product integration may still be required |
| Notes and Messaging | Preview | Android, Desktop, Server | Provider-neutral Connector contracts; verify or supply the real provider adapter needed by your App |

“Declared Host” means the package contract includes that platform; it does not promise identical UI, provider coverage, or production readiness on every Host. Preview means the package is implemented and validated in the repository, while its API, provider support, and cross-platform behavior may still change.

### Host and release targets

| Target | Current role | Important boundary |
| --- | --- | --- |
| macOS Desktop | Electron Host, managed local Rust sidecar, self-contained packaging, signing and notarization workflow | Production distribution still requires your Developer ID, Apple credentials, release testing, and update operations |
| Android | Kotlin Compose Host with Rust FFI and frozen App resources | Requires a project-local Android SDK/NDK; capability and UX parity must be validated for the selected App |
| Server | Local HTTP Host, development diagnostics, background execution, and custom managed-host integration | The stock development server is not a turnkey public SaaS deployment |
| Linux | Supported development environment for the declared Pixi platform | No end-user Linux Desktop release pipeline is documented yet |
| iOS, Windows, public web App | Not currently provided as release targets | A downstream product must add and maintain these Hosts or choose another delivery surface |

## From product idea to delivery

Use this sequence to turn an idea into evidence. Do not treat “the repository starts” as proof that a product is ready.

| Phase | Decision or output | Typical owner |
| --- | --- | --- |
| 1. Product fit | Target user, critical journey, required capabilities, target Hosts, and unacceptable actions | Product and design |
| 2. Technical spike | Minimal App starts, one real model completes a turn, and required packages resolve | App engineering |
| 3. App definition | Validated Manifest, prompts, languages, brand, policies, and private Skills | Product, design, and App engineering |
| 4. Provider plan | Exact model, identity, mail/calendar/contact providers, scopes, account isolation, and fallback behavior | Integration and security engineering |
| 5. Safe local proof | Fake/local providers cover success, denial, approval, retry, conflict, restart, and duplicate-action paths | Engineering and QA |
| 6. Controlled pilot | Dedicated test accounts prove real provider behavior without production data or broad access | Product, QA, security, and legal |
| 7. Release candidate | Frozen App lock, installable Host package, signing, clean-machine smoke, recovery plan, and known limitations | Release engineering |
| 8. Operation | Monitoring, backups, upgrades, support ownership, incident response, data export, and deletion | Product operations and security |

A reasonable first approval is a time-boxed technical prototype with explicit exit criteria. A production decision needs evidence for provider coverage, model quality, data handling, failure recovery, platform behavior, distribution, maintenance cost, and licensing.

## How the system fits together

```text
User
  |
  v
Desktop / Android / Server Host
  UI + identity + credentials + approvals + platform capabilities
  |
  v
Agent runtime -----------------------> Model gateway ------> Model provider
  |                                         |
  |                                         v
  |                                   model response
  |
  +--> Skills describe task behavior
  |
  +--> Runtime / Host Tools perform deterministic local work
  |
  +--> Approval --> Connector --> external account or service
  |
  +--> App-scoped storage, events, Memory, tasks, and durable runs
```

The model can propose work, but it does not grant itself permissions. Credentials stay in Host-controlled storage. External side effects remain subject to Runtime and Host policy, approval, and idempotency checks.

### Terms in product language

| Framework term | What it means for an App creator |
| --- | --- |
| Agent App | The product-specific definition: identity, behavior, capabilities, policy, brand, languages, and private packages |
| App Manifest (`agent-app.json`) | The versioned contract that tells Hosts what the App requires and what policies apply |
| Prompt | Product-authored instructions that shape persona and behavior; never a permission boundary |
| Skill | A reusable description of how the Agent handles a class of tasks, optionally with resources and controlled tools |
| Connector | Deterministic access to an external account or service under authentication, scope, approval, and audit rules |
| Host Tool | A trusted capability supplied by Desktop, Android, Server, or another Host |
| Host | The platform shell that owns UI, credentials, approvals, and platform integration |
| Runtime | The shared engine that runs turns, resolves tools and packages, applies policy, and persists state |
| Foundation Skill | An optional first-party package for a broadly useful capability such as Memory or Mail |

## Technical quick start

This path is for an engineer. It proves the local Host and Runtime chain before you add a model or real external account.

### 1. Prepare the environment

Install Git and [Pixi](https://pixi.prefix.dev/latest/). The project environment manages Rust, Node.js, Python, OpenJDK, and other command-line dependencies. The declared development platforms are macOS Apple Silicon, macOS Intel, and Linux x86_64.

```bash
git clone https://github.com/dale0525/agentweave.git
cd agentweave

pixi install
pixi run npm --prefix apps/desktop ci
```

Android work additionally requires the project-local SDK/NDK described in the [Contributing Guide](./CONTRIBUTING.md#android-environment).

### 2. Validate the repository and App examples

```bash
pixi run validate-agent-assets
pixi run test-dev-script
```

These checks require no model key or external account.

### 3. Start the minimal App

```bash
AGENTWEAVE_APP_ROOT=examples/minimal-agent pixi run dev
```

- Desktop development page: <http://127.0.0.1:5173>
- Server health check: <http://127.0.0.1:49321/health>

If the page loads and the health check returns `ok`, the local shell is working. This does **not** yet prove that an AI turn or external Connector works. Press `Ctrl+C` to stop both processes.

### 4. Complete the first real conversation

Open **Settings → Model** and enter:

- **Base URL**: the provider API root. AgentWeave appends `/responses`, `/chat/completions`, or `/completions` according to the selected protocol. For example, if the provider documents `https://model.example/v1/chat/completions`, enter `https://model.example/v1`.
- **Endpoint type**: Responses, Chat Completions, or Completions. It must match the provider's actual protocol.
- **Model name**: the exact model identifier accepted by that endpoint.
- **API key**: optional for a local endpoint, normally required by a hosted provider. Desktop encrypts a supplied key with Electron safe storage before local persistence.

Select **Test connection** before sending a message. A failed test usually means the Base URL includes too much or too little path, the endpoint type is wrong, the model name is unavailable, or authentication failed. Use a development credential first; do not paste a production key into an unreviewed build.

AgentWeave also supports an App-managed model path with replaceable identity, entitlement, and gateway providers. Use the [Managed Gateway Agent](./examples/managed-gateway-agent/README.md) when end users must not configure model endpoints themselves.

### 5. Create a separate App

```bash
pixi run scaffold-agent-app -- \
  --name "Research Agent" \
  --app-id com.example.research-agent \
  --output output/research-agent

pixi run scaffold-agent-app -- --validate output/research-agent
AGENTWEAVE_APP_ROOT=output/research-agent pixi run dev
```

The generated App contains:

```text
output/research-agent/
  agent-app.json          identity, compatibility, requirements, and policy
  prompts/                system and developer behavior
  locales/                packaged interface languages
  themes/                 optional custom color themes
  fonts/                  optional packaged fonts
  packages/               App-private Skills and packages
```

The Manifest sections answer different questions:

| Section | Question it answers |
| --- | --- |
| `appId`, `package`, `compatibility` | What is this App, which Runtime version does it accept, and which Hosts can load it? |
| `requires` | Which packages, capabilities, Runtime Tools, and Connectors must exist? |
| `policy` | Are external effects, networking, background work, Memory, and Skill management allowed? |
| `instructions` | Which system, developer, and additional Prompt resources define behavior? |
| `branding`, `appearance`, `localization` | What name, themes, fonts, and languages ship to users? |

Start with the generated deny-by-default policy. Add only the requirements demanded by packages you have chosen. See the exact [Minimal Agent Manifest](./examples/minimal-agent/agent-app.json) and the full [App development guide](./DEVELOPING_AGENT_APPS.md).

## Model, data, and security boundaries

- Apps and packages request capabilities; the Host and Runtime decide what is granted.
- Prompts and external documents cannot grant permissions or bypass approval.
- Credentials remain in Host-controlled stores and are referenced by scoped, opaque identifiers rather than copied into Manifests, prompts, packages, or ordinary logs.
- Writes such as sending mail or changing calendar data require durable approval and idempotency protection where the contract declares an external side effect.
- Default tests use fake services or local storage. Real-provider tests and pilots must be enabled deliberately with dedicated accounts.
- Encrypted local backup protects exported SQLite backup envelopes, not every live database or arbitrary workspace file. Read [Local Data Protection and Backup](./DATA_PROTECTION.md) before making at-rest or recovery claims.
- Your model and Connector providers may receive user data. Your product team remains responsible for provider terms, data location, retention, deletion, user consent, logging, and incident response.

## Packaging and release meaning

There are two different artifacts:

1. `package-agent-app` creates a frozen, hash-locked App definition. It is an input to a Host build, not an installable end-user application by itself.
2. A Host packaging pipeline combines that App definition with the Runtime and platform shell to create an installable or deployable product.

Create and verify a frozen App definition:

```bash
pixi run package-agent-app -- \
  --input output/research-agent \
  --output output/research-agent-release \
  --runtime-version 0.1.0

pixi run package-agent-app -- --verify output/research-agent-release
```

Build a self-contained macOS App:

```bash
pixi run package-macos \
  --input output/research-agent \
  --output dist/macos/research-agent \
  --overwrite
```

Local builds receive an ad-hoc signature and are for development. Formal macOS distribution requires Developer ID signing, notarization, clean-machine testing, release metadata, and your own credentials and operational process. Android uses the selected App definition during `pixi run android-assemble`. Server deployment remains a custom managed-Host responsibility rather than a one-command hosted service.

## Repository map

```text
apps/                     Desktop and Android Hosts
crates/agent-runtime/     turns, sessions, tools, policy, storage, and packages
crates/model-gateway/     model endpoint and streaming adapters
crates/agent-server/      local HTTP Host, diagnostics, and background execution
crates/provider-adapters/ Google Workspace and Microsoft 365 adapters
skills/                   built-in, Foundation, and developer Skills
catalog/                  machine-readable Foundation Skill and theme catalogs
examples/                 runnable Agent App references
templates/agent-app/      deny-by-default App scaffold
scripts/                  validation, packaging, and development tooling
```

Product-specific behavior belongs in an App, Skill, Connector, provider adapter, or example. Only reusable protocols, state models, security boundaries, and Host infrastructure should enter the core Runtime.

## Documentation

- [Developing Agent Apps](./DEVELOPING_AGENT_APPS.md): Manifest, prompts, Skills, themes, fonts, local testing, and frozen releases.
- [Secretary Agent](./examples/secretary-agent/README.md): Mail, Memory, approval, and an App-private workflow using local backing.
- [Managed Gateway Agent](./examples/managed-gateway-agent/README.md): App-managed identity, entitlement, model access, and Cloudflare deployment.
- [Provider adapters](./crates/provider-adapters/README.md): Google Workspace and Microsoft 365 OAuth and Connector coverage.
- [Mail Connector Setup](./MAIL_CONNECTOR_SETUP.md): IMAP/SMTP, TLS, Credential Vault, and live-smoke boundaries.
- [macOS Desktop Packaging](./DESKTOP_PACKAGING.md): self-contained App builds, signing, notarization, and release verification.
- [Local Data Protection and Backup](./DATA_PROTECTION.md): encrypted export, restore, key separation, and explicit exclusions.
- [Conversation Lifecycle](./CONVERSATION_LIFECYCLE.md) and [Streaming Turn Lifecycle](./STREAMING_TURN_LIFECYCLE.md): durable history, turn streaming, stop, and recovery contracts.
- [Contributing Guide](./CONTRIBUTING.md): architecture placement, environment, tests, and Pull Request requirements.

## License

Except for separately identified third-party material, AgentWeave is dual-licensed under the [Apache License 2.0](./LICENSE-APACHE) or the [MIT License](./LICENSE-MIT), at your option. See [LICENSE](./LICENSE) for contribution terms and [NOTICE](./NOTICE) for repository-level attributions.

Third-party Skills, scripts, themes, protocols, Connectors, dependencies, and assets retain their own licenses and copyright notices. Preserve package-local license and notice files when redistributing them.
