# Font Library

Fonts a person adds to FilmCraft stay installed: they are copied into the data directory, recorded
with their licence and registered with the text engine at every start, so titles, captions and
exports render with them after a restart. **Window ▸ Font Library** manages them; agents use the
same functions as commands.

## What is supported

| | |
|---|---|
| Formats | TrueType (`.ttf`), OpenType (`.otf`), collections (`.ttc`, `.otc`), WOFF (`.woff`), WOFF2 (`.woff2`) |
| Variable fonts | one style per named instance (Archivo: Thin 100 … Black 900). Metrics, outlines and shaping use the instance's design-space location, so Black renders at weight 900. Checked against Chromium drawing the same WOFF2: advance widths within 0.5 %, coverage within 2 % at 100, 400, 700 and 900 |
| Family names | variable builds that name the family after their default instance ("Archivo SemiBold" with a SemiBold default) are listed under the family ("Archivo") |
| Where | `<data dir>/Font Library/`: the font files (named by content hash) and `library.json`. The data directory is `FILMCRAFT_DATA_DIR`, a portable install's `data/`, or the per-user folder (`~/.local/share/filmcraft`, `~/Library/Application Support/FilmCraft`, `%APPDATA%\FilmCraft`); `filmcraft-cli --data-dir` sets it for the CLI |

WOFF and WOFF2 are decoded by [`wuff`](https://crates.io/crates/wuff) 0.2.9 (MIT, pure Rust, no
`unsafe` in the crate; Brotli by `brotli-decompressor`, BSD-3-Clause/MIT, built without its opt-in
`unsafe` feature). Decoding runs under `catch_unwind` and its output is validated, so a damaged or
hostile file is refused with a message, never a crash. Files over 64 MiB are refused.

## The window

- **Add Fonts…** chooses font files. Each is inspected first (format, styles, variable axes, the
  licence the font states in its name table) and waits for its **licence**: an SPDX identifier
  (`OFL-1.1`, `Apache-2.0`, or `LicenseRef-<name>` for a commercial licence), with the licence text
  or URL and the source. FilmCraft fills in the licence only when the font's own statement names a
  well-known licence (SIL OFL, Apache 2.0, Ubuntu Font Licence, CC0); otherwise the field is empty
  and **Add to Library** stays disabled until it is filled. `NOASSERTION` / `NONE` are refused.
- Every style is listed with its weight and a live preview drawn by the text engine that renders
  titles, captions and exports (preview text and size are editable). A style shadowed by another
  installed copy is marked.
- **Edit Licence…** changes the recorded licence.
- **Remove…** asks for confirmation. When the open project uses the font, the first attempt says so
  and offers **Remove Anyway**; the font stops resolving at once (its text is then reported
  missing) and its file is deleted.
- **Fonts in this sequence** lists every family and style the active sequence uses (enabled clips,
  nested sequences, source graphics, enabled caption tracks) as *exact*, *substituted* (the family is
  installed but not that style; another style, possibly synthetic bold or italic, would be drawn) or
  *missing* (another typeface would be drawn), with what would be drawn instead and where it is used.
- Load problems (a file missing from the library folder, or changed since it was added) are listed;
  such a font is not registered.

A second font that provides a family and style already in the library is refused unless
**Replace** is ticked, so a font name never means two different files.

## No silent fallback

The text engine still draws *something* for an unknown font (so editing never breaks), but FilmCraft
does not let that reach a deliverable unnoticed:

- **Export** (`file.exportMedia`, `export.queue.add`, `export.quick`) is refused when the exported
  sequence's picture uses a missing or substituted font, naming each font, what would be drawn
  instead and where it is used. Caption tracks count when captions are burned in; audio-only exports
  are not checked. `allowFontSubstitution: true` exports anyway; the fonts are returned as warnings
  and logged in the decision log (`autoedit.report`, kind `fontSubstitutionAllowed`).
- **`qc.render`** has a hard `fonts` check: every family and style of the edit must render exactly,
  else the state is `FAILED`. Together with its PSNR comparison (the file matches the edit as
  rendered now) this shows the file's text was drawn in the specified fonts.
- **`fonts.missing`** and the window report the same statuses at any time.

## Commands

| Command | Does |
|---|---|
| `fonts.library.list` | the library folder, every font (families, styles with weight and state, format, axes, licence, added date, size, SHA-256) and load problems |
| `fonts.library.inspect {path}` | what a font file holds before adding it: format, styles, axes, the licence it states, the suggested SPDX id |
| `fonts.library.import {path \| paths, license, licenseText?, source?, replace?}` | adds font files (licence required; `license` may be `{"spdx", "text", "source"}`) |
| `fonts.library.remove {id, force?}` | removes a font (`force` when the open project uses it) |
| `fonts.library.setLicense {id, license}` | changes the recorded licence |
| `fonts.library.preview {family, style?, text?, size?, path?}` | a PNG of every style (or one) drawn by the text engine |
| `fonts.missing {sequence?, all?, ignoreCaptions?}` | exact / substituted / missing for every font a sequence uses |
| `window.fontLibrary`, `fontLibrary.addFonts {paths?}` | opens the window; queues files for licence entry |

```sh
filmcraft-cli exec fonts.library.import '{"path": "Archivo.woff2", "license": {"spdx": "OFL-1.1", "source": "Omnibus-Type/Archivo"}}'
filmcraft-cli exec fonts.library.list
filmcraft-cli --project spot.fcproj exec fonts.missing
```

## Limits

- A removed font stays drawable for layouts already made in this session (face ids never change
  while FilmCraft runs); it no longer resolves for new text and is reported missing.
- Font files are not embedded in project files (graphics templates still embed theirs); a project
  opened on a machine without the font reports it missing and its exports are refused.
- Fonts are not part of the repository: FilmCraft ships none beyond its bundled OFL faces and the
  optional craft-fonts, and library fonts live only in the user's data directory.
