# Skill Lifecycle And Management Design

Date: 2026-07-11

## Status

This document defines the complete target model for skill authoring, packaging, installation,
runtime mutation, authorization, upgrades, recovery, and cross-platform behavior in
GeneralAgent.

It supersedes the following earlier product decisions where they conflict with this design:

- Packaged applications always have a completely immutable skill inventory.
- Removing `skill-creator` is sufficient to prevent users from managing skills.
- Development skill APIs can later be reused as packaged-application management APIs.
- A single `skills/` directory can represent source packages, bundled packages, and runtime
  user data.

The earlier documents remain useful as implementation history. Their development tooling and
hidden-skill decisions still apply where this document does not replace them.

## Summary

GeneralAgent will provide runtime skill management as a framework capability, but packaged
applications must explicitly enable it through application policy. The default remains disabled.

The effective skill inventory is composed from separate source layers:

```text
Effective Skill Set
  = Built-in Skills
  + Managed Skills
  + optional Session Skills
  + Host Policy
```

Built-in Skills are selected by the application developer and shipped as an immutable bundle.
Managed Skills live in application data and may be created, imported, edited, activated,
disabled, rolled back, exported, or removed only when the host application grants the required
permissions. Session Skills are optional temporary packages that never become persistent
capabilities without a separate activation flow.

Skill management is enforced by the runtime. Hiding UI, omitting instruction text, or deleting
`skill-creator` is not a security boundary.

## Product Decisions

### Framework Capability, Application Choice

The framework supplies:

- Skill package validation.
- Built-in and managed stores.
- Draft, staging, quarantine, activation, rollback, and removal workflows.
- Runtime snapshots and atomic reload.
- Owner-facing management services and optional Agent tools.
- Policy enforcement and audit records.

Each packaged application decides whether to expose any of these capabilities. The default
application policy disables skill mutation.

### Layered Store Model

Use an immutable built-in layer and an optional mutable managed layer. Do not mutate application
installation resources and do not treat a packaged bundle as user data.

### Dedicated Management Boundary

Runtime skill mutation must go through `SkillManager`. Generic filesystem or command tools must
not install or activate packages by writing directly into skill directories.

### Conservative Runtime Authoring

Runtime authoring initially supports instruction-only skills and skills composed from
host-approved tools. Runtime-generated native command packages are excluded by default because
the current command entry model can execute arbitrary host processes.

### Stable Per-Turn Snapshot

Each turn captures one immutable `SkillSnapshot`. Activating or removing a skill affects the next
turn, not the current turn. Tool definitions, instruction documents, and executable resources
therefore remain consistent throughout one turn.

## Why The Current Model Is Insufficient

The current repository correctly separates two payload types:

- `SkillCatalog` loads instruction skills from `SKILL.md`.
- `SkillRegistry` loads executable tools from `skill.json`.

It also correctly separates development discovery from frozen packaged loading. However, several
assumptions prevent packaged owner-managed skills:

- `SkillRegistry`, `SkillCatalog`, and `TurnRunner` are constructed at process startup.
- `/dev/skills/reload` refreshes diagnostics only and does not replace active runtime state.
- Deleting a package on disk does not remove it from the already-created runtime registry.
- One configured root represents both development and packaged loading.
- The development API operates on repository source directories.
- Skill management has no independent permission type.
- The current command-based runtime package is too powerful for unrestricted Agent-generated
  installation.
- The mobile runtime loads its app-data skill directory with development discovery semantics.

The new design keeps the useful existing loaders and validators but places them behind a runtime
manager with explicit sources, policy, transactions, and snapshots.

## Modeling Dimensions

Personas alone do not determine skill behavior. Every installed package is evaluated across these
dimensions:

| Dimension | Examples |
| --- | --- |
| Origin | `framework`, `application`, `owner`, `third_party`, `agent_generated` |
| Scope | `application`, `organization`, `account`, `device`, `workspace`, `session` |
| Trust | `built_in`, `signed`, `approved`, `untrusted`, `quarantined` |
| Lifecycle | `draft`, `validating`, `active`, `disabled`, `failed`, `removed` |
| Mutability | `immutable`, `owner_mutable`, `ephemeral` |
| Kind | `instruction_only`, `host_tools_only`, `native_runtime` |
| Authority | inspect, create, edit, validate, activate, disable, delete, override, export |

Origin and trust are installation state owned by the host. A package cannot declare itself trusted
or built-in.

## Supported Roles And Scenarios

### Framework Maintainers

Framework maintainers author source packages in the repository, validate them with development
tools, commit them to Git, and select official packages for distribution. The in-app developer
workbench is a diagnostic aid, not the primary authoring workflow.

Recommended source separation:

```text
skills/core/       framework-critical packages
skills/official/   supported optional packages
skills/examples/   examples not bundled by default
```

Multiple configured source roots are also acceptable if they produce the same package model.

### Fork And Application Developers

Application developers use the same source workflow without committing application-specific
packages upstream. Application packages should live in a separate overlay:

```text
skills/             framework-provided source packages
app/skills/         application-specific source packages
```

The build resolves both sources by stable package ID. This keeps upstream updates separate from
application customization.

### Packaged Application Owners

When the application enables owner management, an authenticated owner may create and modify
Managed Skills through conversation or an owner interface. Managed packages live in application
data, survive application upgrades, and can be exported as source packages for later inclusion in
an application build.

### Ordinary Users

When management is disabled, management tools and APIs are absent. Generic tools cannot access
skill stores. Built-in capabilities remain available to the Agent without exposing package
management concepts to the user.

### Additional Scenarios

The same model covers:

- Organization administrators who manage a shared organization layer.
- Team members who may invoke skills but cannot activate revisions.
- Third-party package authors whose packages enter quarantine before approval.
- Hosted multi-tenant runtimes with tenant-scoped stores and snapshots.
- Temporary elevated users whose grants expire.
- Multi-device users whose packages synchronize but are revalidated against each device.
- Session-only experiments that disappear without activation.
- Safe-mode recovery after a broken package or incompatible update.

## Skill Package Model

A Skill Package is the unit of identity, validation, installation, versioning, and activation.

```text
skill-package/
├── general-agent.json
├── SKILL.md
├── skill.json
├── references/
├── scripts/
├── assets/
└── agents/openai.yaml
```

Only files required by the package need to be present.

### Package Descriptor

`general-agent.json` becomes the package-level descriptor:

```json
{
  "schemaVersion": 1,
  "id": "com.example.calendar",
  "version": "1.2.0",
  "displayName": "Calendar",
  "package": {
    "includeInstructions": true,
    "includeRuntime": true
  },
  "compatibility": {
    "minimumRuntimeVersion": "0.3.0",
    "platforms": ["desktop", "android"]
  },
  "requires": {
    "packages": [],
    "capabilities": ["network.http"],
    "runtimeTools": [],
    "connectors": []
  }
}
```

Responsibilities remain separated:

- `general-agent.json` defines package identity, version, compatibility, dependencies, and bundle
  targets.
- `SKILL.md` defines model-visible instructions.
- `skill.json` defines runtime tool contracts and executable entry details.
- `agents/openai.yaml` defines optional presentation metadata.

Package descriptors do not grant management authority. Disable, override, activation, and
permission-escalation rules belong exclusively to application policy and installation state.

### Package Identity

Package IDs are stable and globally unique within an application. Folder names, instruction skill
names, display names, and runtime tool names are not package identity.

### Tool Identity

Runtime tools should migrate to namespaced identity:

```text
com.example.calendar/create_event
```

The model gateway may encode this identity for providers with stricter function-name formats, but
the internal identity remains namespaced. This removes global collisions between built-in and
managed tools.

### Package Kind

Packages are classified by executable risk:

| Kind | Meaning | Runtime authoring default |
| --- | --- | --- |
| `instruction_only` | Instructions and references only | Allowed in owner mode |
| `host_tools_only` | Instructions composed around existing approved host tools | Allowed in owner mode |
| `native_runtime` | Adds command, process, WASM, or equivalent executable code | Disabled unless explicitly enabled |

## Skill Stores

### Built-in Store

- Generated during application build.
- Shipped in application resources.
- Read-only at runtime.
- Verified against bundle hashes.
- Replaced only by an application update.

### Managed Store

- Stored under application data.
- Mutable only through `SkillManager`.
- Preserved across application updates.
- Revisioned and auditable.
- Scoped to device, account, organization, or tenant according to host configuration.

### Staging Store

- Contains drafts and imports before activation.
- Not exposed to the model as active skills.
- Supports validation, testing, review, and permission diff generation.

### Quarantine Store

- Contains invalid, incompatible, revoked, or repeatedly failing revisions.
- Never contributes instructions or tools to the active snapshot.
- Retains diagnostics and recovery information.

### Session Store

- Optional and ephemeral.
- Cleared when its session or configured lifetime ends.
- Cannot override Built-in or Managed packages.
- Must use a separate activation flow before persistence.

## SkillManager Architecture

```text
SkillManager
├── BuiltinSkillSource
├── ManagedSkillSource
├── StagingSkillSource
├── optional SessionSkillSource
├── SkillValidator
├── SkillResolver
├── SkillPolicy
├── SkillRevisionStore
├── SkillAuditStore
└── current_snapshot: Arc<SkillSnapshot>
```

### SkillSnapshot

An immutable snapshot contains:

- Snapshot generation.
- Effective package IDs and revision IDs.
- Package versions and content hashes.
- Resolved dependencies.
- Availability and capability results.
- `SkillRegistry` runtime tools.
- `SkillCatalog` instruction summaries and documents.
- Tool definitions and namespaces.
- Diagnostics needed to explain inactive packages.

### Turn Integration

Every turn captures the current snapshot before building model input. The turn uses that snapshot
until completion, cancellation, or failure. A newly activated revision becomes visible to the next
turn.

### Mutation Transaction

All mutations follow this sequence:

```text
write immutable revision to staging
→ validate descriptor and payloads
→ resolve dependencies and conflicts
→ evaluate policy and permissions
→ optionally execute isolated tests
→ build candidate snapshot
→ atomically move revision into Managed Store
→ atomically publish snapshot
→ record audit event
```

Failure before publication leaves the previous snapshot active.

### Revision Layout

Active revisions are immutable:

```text
managed-skills/
  com.example.calendar/
    revisions/
      rev-001/
      rev-002/
    current -> rev-002
```

Old revisions are retained while any live snapshot references them. Cleanup occurs only after the
last reference is released and rollback retention policy allows deletion.

## Application Skill Policy

Applications configure skill management explicitly:

```json
{
  "skills": {
    "builtinBundle": "resources/skills/skill-bundle.json",
    "management": {
      "mode": "owner_only",
      "agentAuthoring": true,
      "allowedKinds": ["instruction_only", "host_tools_only"],
      "allowedOperations": [
        "create",
        "edit",
        "validate",
        "activate",
        "disable",
        "delete",
        "import",
        "export",
        "rollback"
      ],
      "protectedPackages": [
        "generalagent.core",
        "generalagent.skill-manager"
      ],
      "allowOverrides": [],
      "activationApproval": "always",
      "permissionEscalationApproval": "always"
    }
  }
}
```

Supported policy modes:

- `disabled`: no management tools, API, or UI.
- `diagnostics_only`: status and compatibility diagnostics only.
- `owner_only`: authenticated application owners may manage allowed package kinds.
- `organization_managed`: future organization policy controls installation and activation.

## Actor Context And Authorization

Authorization is supplied by the host, not inferred by the model:

```text
ActorContext
├── actor_id
├── role
├── tenant_id
├── device_id
└── grants
```

Possible grants include:

```text
skills.inspect
skills.create_draft
skills.edit_draft
skills.validate
skills.test
skills.activate
skills.disable
skills.delete_managed
skills.import
skills.export
skills.override_builtin
skills.grant_permissions
```

If the actor lacks a grant, the corresponding tool and service operation are unavailable. A user
message claiming administrative authority has no effect.

## Permission And Approval Model

Skill management permissions are independent from workspace read, workspace write, and command
execution permissions.

Default approval behavior:

| Operation | Default |
| --- | --- |
| Create instruction draft | Allowed for an authorized owner |
| Edit inactive draft | Allowed for an authorized owner |
| Static validation | Allowed |
| Execute tests | Approval follows test capabilities |
| Activate a new skill | Always require approval |
| Update an active skill | Always require approval and show diff |
| Add a capability | Always require approval |
| Increase tool permissions | Always require approval |
| Delete an active skill | Always require approval |
| Override a built-in package | Denied unless explicitly allowlisted |
| Import a third-party package | Quarantine before testing or activation |

The Agent may request an operation but cannot approve its own request.

## Filesystem Isolation

Generic filesystem tools must not access the control-plane paths used by skill management:

```text
app://builtin-skills
app://managed-skills
app://skill-staging
app://skill-quarantine
app://skill-state
```

Only `SkillManager` receives handles to these stores. This rule applies even when the application
otherwise gives the Agent workspace write access.

## Services And APIs

### DevSkillAuthoringService

The development service remains responsible for repository source packages:

- Scan and validate configured source roots.
- Show package diagnostics.
- Generate authoring prompts.
- Support source deletion when development APIs are enabled.
- Perform real runtime reload after successful validation when requested.

It remains development-only and must not be reused as the packaged owner service.

### OwnerSkillManagementService

The owner service operates only on Staging, Managed, and Quarantine stores:

```text
list_effective_skills
list_managed_skills
create_draft
update_draft
import_draft
validate_draft
test_draft
request_activation
activate_revision
disable_managed_skill
remove_managed_skill
rollback_managed_skill
export_managed_skill
get_skill_audit_log
```

Desktop applications should prefer Electron IPC or another authenticated native bridge. Remote or
hosted applications may expose authenticated owner HTTP APIs with tenant isolation.

### Agent Management Tools

When both application policy and actor grants allow them, the Agent may receive:

```text
create_skill_draft
edit_skill_draft
validate_skill_draft
test_skill_draft
request_skill_activation
disable_managed_skill
request_skill_removal
rollback_managed_skill
```

Activation and removal may produce approval requests instead of completing immediately.

## Authoring And Activation Flow

### Create Through Conversation

```text
owner requests a new skill
→ Agent creates a draft through OwnerSkillManagementService
→ static validation runs
→ isolated tests run when required
→ owner receives package summary, diff, tools, dependencies, and permission changes
→ owner approves activation
→ SkillManager publishes a new snapshot
→ next turn can use the new skill
```

### Update An Active Skill

```text
active revision remains unchanged
→ create a new immutable draft revision
→ validate and test
→ display diff and permission changes
→ approve
→ atomically switch snapshot
→ retain previous revision for rollback
```

### Import A Third-Party Skill

```text
import into quarantine
→ verify descriptor and content hash
→ classify trust and risk
→ validate dependencies and capabilities
→ test under restricted permissions
→ owner review and approval
→ copy to Managed Store and publish snapshot
```

## Conflict And Dependency Resolution

Resolution order:

1. Validate package ID and descriptor schema.
2. Validate package version and runtime compatibility.
3. Resolve package dependencies.
4. Evaluate platform capabilities.
5. Apply protected-package rules.
6. Validate tool namespaces.
7. Apply explicit override allowlists.
8. Build the candidate snapshot.

Default rules:

- Built-in packages have highest precedence.
- Managed packages with an existing package ID conflict unless override is explicitly allowed.
- Session packages cannot override Built-in or Managed packages.
- Missing dependencies make a package inactive without failing application startup.
- Missing platform capabilities preserve installation state but exclude the package from the active
  snapshot.
- One invalid Managed package cannot block unrelated packages.

## Persistence Model

SQLite stores control-plane metadata while package files remain on disk.

### `skill_installations`

- `package_id`
- `source_layer`
- `active_revision_id`
- `enabled`
- `trust_level`
- `install_status`
- `installed_at`
- `updated_at`

### `skill_revisions`

- `revision_id`
- `package_id`
- `version`
- `content_hash`
- `storage_path`
- `descriptor_json`
- `validation_json`
- `created_by`
- `created_at`

### `skill_approvals`

- `approval_id`
- `package_id`
- `revision_id`
- `operation`
- `requested_by`
- `approved_by`
- `permission_diff`
- `created_at`

### `skill_snapshots`

- `generation`
- `status`
- `members_json`
- `created_at`
- `activated_at`

### `skill_audit_log`

- `actor_id`
- `operation`
- `package_id`
- `revision_id`
- `result`
- `metadata_json`
- `created_at`

## Application Upgrade Behavior

Application updates replace the Built-in Store and preserve the Managed Store:

```text
load new Built-in Bundle
→ load existing Managed Store
→ revalidate compatibility and dependencies
→ build candidate snapshot
→ publish on success
→ otherwise retain a last-known-good snapshot
```

Possible outcomes after an update:

- A Managed Skill continues normally.
- A Managed Skill is disabled because its runtime compatibility no longer matches.
- A Managed Skill becomes inactive because a Built-in dependency was removed.
- A previously permitted override stops applying because the new application protects that
  package.
- The owner sees a stable diagnostic and can update, export, disable, or remove the package.

## Startup And Failure Recovery

Startup sequence:

1. Verify and load the Built-in Bundle.
2. Restore the last-known-good snapshot when available.
3. Scan the Managed Store.
4. Move invalid or incomplete revisions to quarantine.
5. Attempt to build a new snapshot.
6. Publish the new snapshot or continue with the last-known-good snapshot.
7. Report diagnostics without making the entire Agent unavailable.

Failure rules:

- Activation failure leaves the previous snapshot active.
- Test timeout leaves the draft inactive.
- Repeated execution failures may trip a circuit breaker and disable the revision.
- Startup removes or repairs incomplete staging transactions.
- Concurrent edits use revision IDs or ETags for optimistic concurrency.
- Snapshot publication is serialized through a single writer lock.
- Old revisions remain until no active snapshot references them.

## Packaging

`check-skills` remains a validation command. A separate bundle command must build the actual
application artifact:

```text
discover configured source packages
→ validate descriptors and payloads
→ resolve dependencies
→ select target packages
→ copy the exact file set
→ calculate content hashes
→ generate skill-bundle.json
→ generate skill-bundle.lock
→ run packaged-mode integration tests
```

`skill-bundle.json` contains at least:

- Package ID.
- Version.
- Relative path.
- Content hash.
- Instruction and runtime inclusion flags.
- Runtime compatibility.
- Declared capabilities.

`skill-bundle.lock` records the complete resolved dependency set so development validation and the
packaged application use identical package contents.

## Cross-Platform Behavior

### Desktop And Server

- Built-in resources are read-only.
- Managed stores use application or tenant data directories.
- Native runtime packages remain subject to host sandbox and command policy.
- Desktop owner operations prefer authenticated IPC.
- Server owner operations require authentication and tenant isolation.

### Android

Android uses separate roots:

```text
APK assets or extracted verified bundle → Built-in Store
filesDir/managed-skills              → Managed Store
cacheDir/skill-staging               → Staging Store
filesDir/skill-quarantine            → Quarantine Store
```

Android must load the packaged built-in bundle with packaged semantics, not
`load_development()`. Managed packages are revalidated against Android capabilities before every
snapshot publication.

The Skills screen follows application policy:

- `disabled`: no skill management surface.
- `diagnostics_only`: availability diagnostics without mutation.
- `owner_only`: owner management for allowed package kinds.
- Ordinary users never modify Built-in packages.

### Synchronization

Future account or organization synchronization transfers package revisions and installation
intent. Each target device independently reevaluates platform compatibility, capabilities,
permissions, and trust before activation.

## UI Boundaries

The project has two distinct skill surfaces:

### Developer Tools

- Visible only in development mode.
- Operates on repository source packages.
- Shows source validation and bundle readiness.
- Supports development reload and authoring prompts.

### Owner Skill Management

- Present only when application policy enables it.
- Operates on drafts and Managed Skills in application data.
- Shows revision history, validation status, permission diff, activation state, and rollback.
- Never edits the application bundle.

Normal users continue to interact with the application through chat and application-specific
settings without needing to understand skill packages.

## Security Requirements

- Skill stores are outside generic workspace filesystem access.
- Application policy and actor grants are enforced before tools are exposed.
- The model cannot self-elevate or approve its own requests.
- Third-party imports enter quarantine.
- Content hashes are checked before activation and startup loading.
- Built-in packages are immutable and protected by default.
- Native runtime authoring is disabled by default.
- Permission escalation always requires explicit approval.
- Audit logs exclude secrets and sensitive file contents.
- Tenant-scoped stores and database rows are mandatory in hosted deployments.
- Network policy must become enforceable before network-capable untrusted packages are supported.

## Required Changes To The Current Implementation

The complete implementation scope includes:

- Add `SkillManager`, Skill Stores, revisions, snapshots, transactions, and audit persistence.
- Build `SkillRegistry` and `SkillCatalog` from one resolved package set.
- Change `TurnRunner` to capture a snapshot at turn start.
- Replace startup-frozen registry and catalog fields in `AppState` with `SkillManager` access.
- Make development reload publish an actual runtime snapshot.
- Separate DevSkillAuthoringService from OwnerSkillManagementService.
- Add skill-management permissions and approval events.
- Prevent generic filesystem tools from accessing skill control-plane paths.
- Extend `general-agent.json` with stable package identity, version, compatibility, and package
  dependencies.
- Introduce namespaced tool identity and compatibility encoding in the model gateway.
- Add SQLite migrations for installations, revisions, approvals, snapshots, and audit logs.
- Implement `bundle-skills`, content hashes, lockfiles, and packaged-mode tests.
- Support multiple source roots for framework and application packages.
- Add Desktop owner management separately from Developer Tools.
- Replace Android development loading with Built-in, Managed, Staging, and Quarantine stores.
- Make Android skill surfaces policy-dependent.
- Preserve compatibility loading for the current flat `skills/` format during migration.
- Update older product documents to point to this design where their immutable packaged-mode
  assumptions are superseded.

## Migration Strategy

The implementation plan must cover the complete target, not only an initial phase. The migration
order should preserve a working application after each milestone:

1. Introduce stable package descriptors and compatibility parsing while preserving legacy input.
2. Add configured source layers and a resolved package representation.
3. Add `SkillManager` and immutable snapshots with Built-in packages only.
4. Move `TurnRunner` and diagnostics to per-turn snapshots.
5. Make development reload atomic and real.
6. Add revision, snapshot, approval, and audit persistence.
7. Add Staging, Managed, and Quarantine stores.
8. Add application policy, ActorContext, management grants, and approval flow.
9. Add OwnerSkillManagementService and Agent draft-management tools.
10. Add instruction-only and host-tools-only runtime authoring and activation.
11. Add rollback, last-known-good recovery, circuit breaking, and delayed revision cleanup.
12. Add namespaced tool identity and conflict resolution.
13. Add complete bundle generation, hashes, lockfile, and packaged integration tests.
14. Add Desktop owner management and preserve separate Developer Tools.
15. Migrate Android to layered stores and policy-dependent skill surfaces.
16. Add application-upgrade compatibility tests and legacy-format migration diagnostics.
17. Remove legacy direct-mutation paths only after all supported hosts use `SkillManager`.

## Testing Strategy

### Unit Tests

- Descriptor parsing, schema migration, and legacy compatibility.
- Stable package and namespaced tool identities.
- Layer precedence and protected package behavior.
- Version, dependency, capability, and compatibility resolution.
- Policy grants and management tool visibility.
- Permission diff generation.
- Revision immutability and reference tracking.

### Runtime Integration Tests

- Build one snapshot from Built-in and Managed packages.
- Capture a stable snapshot for one turn.
- Activate a revision while an older turn is running.
- Reload instructions and tools together.
- Reject a conflicting or unauthorized package without affecting the active snapshot.
- Roll back to a previous revision.
- Recover the last-known-good snapshot after restart.
- Quarantine incomplete or invalid revisions.
- Trip and recover a package circuit breaker.
- Prevent generic filesystem access to skill stores.

### Packaging Tests

- Generate deterministic bundle and lock files.
- Verify content hashes.
- Detect undeclared files and missing resources.
- Ensure packaged loading uses only locked packages.
- Preserve Managed Skills across application bundle updates.
- Reevaluate compatibility when Built-in packages change.

### Authorization And Security Tests

- Management tools are absent when policy is disabled.
- A normal user cannot gain grants through prompt text.
- The Agent cannot approve its own activation request.
- Built-in packages cannot be overwritten by default.
- Third-party imports remain quarantined until approval.
- Native runtime drafts are rejected unless explicitly enabled.
- Tenant data and snapshots remain isolated.

### End-To-End Tests

- Framework developer authors, validates, bundles, and runs a source package.
- Fork developer adds an application package without modifying framework packages.
- Packaged owner creates an instruction skill through chat, reviews it, activates it, and uses it
  on the next turn.
- Packaged owner edits and rolls back a Managed Skill.
- Ordinary user receives no management tools or management UI.
- Application update replaces Built-in Skills and preserves compatible Managed Skills.
- Android revalidates synchronized packages and explains unavailable capabilities.
- A bad Managed Skill cannot prevent the application from starting.

## Acceptance Criteria

- Applications explicitly choose whether runtime skill management exists.
- Disabling management removes the tools, service surface, and filesystem path to mutation.
- Built-in and Managed Skills have separate storage and lifecycle.
- Source authoring and packaged owner management remain separate services.
- Package activation and rollback are atomic.
- Every turn uses one consistent snapshot.
- Managed Skills survive application upgrades when compatible.
- Invalid or incompatible packages do not make the whole application unavailable.
- Runtime-created native command packages are disabled by default.
- Desktop, server, and Android enforce the same package policy model.
- The build emits a deterministic verified bundle and lockfile.
- The full implementation plan covers all required changes and end-to-end verification.

## Non-Goals

- A public marketplace in the initial implementation.
- Automatic trust of third-party packages.
- Arbitrary native code generation in ordinary owner mode.
- Cross-device automatic activation without target-device validation.
- Treating UI visibility as authorization.
- Reusing repository development APIs as packaged owner APIs.
