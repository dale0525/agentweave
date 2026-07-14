# Custom themes

Place VS Code-compatible `.json` or `.jsonc` color themes in this directory. Themes may use `include` when the included file also remains inside this directory.

Declare each theme in `agent-app.json`:

```json
{
  "id": "com.example.brand-dark",
  "label": "Brand Dark",
  "path": "themes/brand-dark-color-theme.json"
}
```

Add the entry to `appearance.themes.custom`, then use its `id` as `appearance.defaultTheme` if it should be the initial theme. AgentWeave maps VS Code workbench colors to the App surface; syntax token colors remain valid theme data but do not affect chat typography.
