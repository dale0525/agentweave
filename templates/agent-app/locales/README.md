# Localization

Keep one UTF-8 JSON catalog per locale. Catalogs use flat, stable message keys and must contain the same keys and placeholders.

Declare every catalog in `agent-app.json`. During release packaging, use `--locales en,zh-CN` to choose which declared languages ship in the App.
