# Minimal Agent

This directory contains the smallest consumer-facing Agent App example for `com.example.minimal-agent`. It is the recommended starting point for verifying a new checkout because it does not require an external account or persistent memory.

The app enables only the consumer-default Filesystem foundation package. Its manifest denies network and background execution, requires approval for external side effects, and excludes preview packages and the developer-only Skill Creator.

From the repository root, install and validate the required assets:

```bash
pixi install
pixi run npm --prefix apps/desktop ci
pixi run scaffold-agent-app -- --validate examples/minimal-agent
```

Start the Server and Desktop development page with this App definition:

```bash
GENERAL_AGENT_APP_ROOT=examples/minimal-agent pixi run dev
```

Open <http://127.0.0.1:5173>. The UI can load without a model; sending a message requires model settings that match an available Responses, Chat Completions, or Completion endpoint.

To create a separate App, use `pixi run scaffold-agent-app` instead of modifying this reference in place. See [Developing Agent Apps](../../DEVELOPING_AGENT_APPS.md) for the full workflow.
