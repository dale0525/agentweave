# {{APP_NAME}}

This directory is a GeneralAgent Agent App scaffold for `{{APP_ID}}`.

The application manifest is `agent-app.json`. Prompts live under `prompts/`, UTF-8 localization catalogs under `locales/`, optional app-local packages under `packages/`, VS Code-compatible themes under `themes/`, and packaged fonts under `fonts/`.

The scaffold exposes the same 19 initial color themes as the pinned VS Code release and starts with Dark 2026. Edit `appearance.themes.builtins` to control which choices ship in the App.

English and Simplified Chinese catalogs are included initially. Keep their message keys and placeholders aligned, then choose the release languages with `package-agent-app --locales en,zh-CN`.

The default security posture is deny-by-default. Credentials stay in the host vault, and external side effects require host approval.

Validate this scaffold from the GeneralAgent repository root:

```bash
pixi run scaffold-agent-app -- --validate <path-to-this-directory>
```

Run the App in the local Server and Desktop development hosts:

```bash
GENERAL_AGENT_APP_ROOT=<path-to-this-directory> pixi run dev
```

Before adding a package, declare its version, capabilities, runtime tools, and connectors in `agent-app.json`. Prompt instructions can shape behavior but cannot grant permissions or bypass host approval.

See the repository's `DEVELOPING_AGENT_APPS.md` for package authoring, appearance, validation, and release artifact instructions.
