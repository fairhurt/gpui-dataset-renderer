# gpui-dataset-renderer

Requirements Explorer POC: render a large dataset (up to 150k) of XML requirement
documents in a GPUI (Zed's Rust GPU UI framework) desktop app with a shadcn/ui-style dark theme.

Dependencies: GPUI comes from `gpui-pre` 0.3.11 (renamed to `gpui` in Cargo.toml) plus
`gpui-pre-platform` (`gpui_platform::application()`), because `gpui-component` 0.9 (Longbridge's
component library: code editor, diff, menus, dialogs, notifications, resizable panels) is built on
it. Icons come from `gpui-kit-assets` (registered with `.with_assets(Assets)`). The window's root
view must be `gpui_component::Root` for menus/dialogs/notifications to work.

## The data

- One XML file per requirement, e.g. `<requirement id="R121" type=… status=… priority=…
  owner=… version=… updated=… children="R122,R123">`, with `<title>` and
  `<description><![CDATA[ …HTML… ]]></description>`.
- The HTML can contain anything: tables (incl. colspan), lists, entities, `<pre>`,
  inline formatting, stray or unclosed tags, and literal `]]>` (split across CDATA sections).
- Child links live in the `children` attribute; the document forms a tree.
- POC data is synthetic and generated deterministically in memory (`src/data/synth.rs`).
  `--write <dir>` dumps it as real files; `--dir <dir>` loads a folder of .xml files.

## Architecture

- `src/data/xml.rs`: quick-xml 0.37 parser for one requirement document.
- `src/data/mod.rs`: `Source` (synthetic or folder), parallel indexer (rayon) that parses
  every file once, resolves child IDs into a CSR tree, then computes DFS order, depth,
  section numbers and search keys. Bodies are NOT kept: they're re-read and parsed lazily on expand.
- `src/html.rs`: forgiving HTML → DOM → `Block` model (paragraphs with styled runs,
  headings, lists, tables, quotes, code, rules). Has unit tests.
- `src/html_render.rs`: `Block` → GPUI elements (`StyledText::with_runs` for inline styles).
- `src/explorer.rs`: UI-independent state (tree open flags, visible tree rows, filtered table
  rows, expanded rows, selection, LRU body cache).
- `src/app.rs`: the view. Sidebar tree = `uniform_list`; main table = `gpui::list` (variable
  height, so a row can expand inline to show the rendered HTML). Clicking a tree item
  selects it, expands its row and scrolls the table to it. Search box is a focusable div
  that captures key events (GPUI 0.2.2 has no built-in text input).
- `src/editor.rs`: XML editor panel (gpui-component `Editor` + tree-sitter-xml registered as
  language "xml", with HTML injected into CDATA), live validation (quick-xml well-formedness +
  requirement schema checks → editor diagnostics + problems list), changes view (`similar` patch →
  gpui-component `Diff`, unified/split) and a rendered preview. Ctrl+S saves. Has unit tests.
- Edits are kept in memory only: `Explorer::edits` overlays saved XML on the source, and the
  table, search, status counts and preview read through `Explorer::req()` / `current_xml()`.
  Changing `children` does not relink the tree (the indexer owns it); the editor says so.
- Opening: right-click a table row (context menu), "Edit XML" in an expanded row, or
  `--edit <ID>` on the command line.
- `src/ui.rs`, `src/theme.rs`: shadcn-like primitives and zinc dark tokens.

## Building on Windows

GPUI's build script only looks for `fxc.exe` at Windows SDK 10.0.26100.0. This machine has
10.0.22000.0, so plain `cargo build` fails with "Failed to find fxc.exe". Either:

- use `run.cmd` (finds fxc via `scripts/find-fxc.ps1`, runs `cargo build --release`, then
  starts the exe detached), or
- set `GPUI_FXC_PATH` first, e.g. in PowerShell:
  `$env:GPUI_FXC_PATH = & .\scripts\find-fxc.ps1; cargo build --release`

`dev.cmd` / `scripts/dev.ps1` is a file-trigger rebuild loop made for a session that had no
Windows shell. Not needed when running Claude Code locally; feel free to delete it.

## Status / next steps

- Builds cleanly on Windows (release), `cargo test --release` passes (html parser, validator,
  diff and XML-highlighting tests), and the app launches with 150k synthetic requirements.
- Release builds use the Windows GUI subsystem (no console window).
- Pushed to https://github.com/fairhurt/gpui-dataset-renderer (branch `main`).
- Still to verify by hand in the UI: the tree, table scrolling, clicking a tree item
  (expands + scrolls), HTML tables rendering, search, status filter tabs.

The user wants all building and running done on this Windows PC, not in WSL or the cloud.
