# App fonts

Place font files in this directory before building the desktop App. No manifest entry is required.

The file name selects the typography slot:

- `ui.woff2` or `ui-400.woff2` supplies normal interface text.
- `display.woff2` supplies display headings.
- `mono.woff2` supplies code and technical identifiers.

Optional weight and style suffixes are supported, for example `ui-600.woff2` and `ui-400-italic.woff2`. Desktop supports WOFF2, WOFF, TTF, and OTF, with WOFF2 preferred. Android loads TTF and OTF through the platform `Typeface` API; it ignores WOFF and WOFF2 and falls back to the system font for that slot.

Keep at most 24 files in this directory. Each file must be no larger than 8 MiB, and the directory total must be no larger than 32 MiB. Only package fonts that you are licensed to distribute.
