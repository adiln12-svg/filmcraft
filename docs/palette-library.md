# Colour Palette Library

Named, reusable colour palettes kept outside projects, so the same palette serves every project
and (from Milestone 2C) client brand kits. **Window ▸ Palette Library** manages them (also reachable
from **Project Colors ▸ Palette Library…**); agents use the same functions as `palettes.*` commands.

## Model

| | |
|---|---|
| Palette | `pal_…` id, name, description, archived flag, history of changes, and its **versions** |
| Version | a numbered, saved state of the palette's colours and usage rules: who saved it (`by`), when, a note, and a SHA-256 of its content. **A saved version never changes.** Every save of a change to colours or rules is a new version; renaming, describing and archiving are metadata (recorded in the history, not versioned) |
| Colour | `col_…` id (stable across versions and renames), name (unique in the palette), exact sRGB value (`#RRGGBB`, also returned as `[r, g, b]`), optional **role** (`primary`, `secondary`, `accent`, `background`, `text`, `neutral` or any short label of your own), optional **shade** of a base colour with a label (`Deep`, `700`…) |
| Usage rule | `rule_…` id. **Permitted** or **prohibited** combinations (a foreground colour on a background colour, for `text`, `graphics` or `any`) and written **guidelines** (text, optionally about some colours). Only rules someone states: FilmCraft adds none, and refuses a combination that is both permitted and prohibited |
| Storage | `<data dir>/Palette Library/palettes.json`, next to the Font Library (the same data directory: `FILMCRAFT_DATA_DIR`, a portable install's `data/`, or the per-user folder; `filmcraft-cli --data-dir` for the CLI). Written atomically; a damaged file is reported and never overwritten |

**Why versions.** A brand kit approved in 2C will reference `pal_…@N` (and its fingerprint). Later
edits to the palette create version N+1 and leave N exactly as approved, so editing a shared palette
can never silently change an approved kit. Two safeguards for edits:

- An edit names the version it was made to (`baseVersion`). If the palette has moved on (someone,
  person or agent, saved another version), the edit is refused instead of overwriting it.
- Each version's SHA-256 is checked on every read (`intact`). A version altered outside FilmCraft is
  reported, and no new version is built on it.

## The window

- **Left**: search (palette name, description, colour name, `#hex` or role), *Show archived*,
  **New Palette…**, and every palette with its swatches and latest version.
- **Palette actions**: **Rename…**, **Duplicate** (a new palette from the version shown), **Archive** /
  **Restore**, **Add to Project Colors** (puts the colours into the project's swatches, the existing
  Project Colors list), **Delete…** (asks first; archive keeps a palette instead).
- **Version** picker: every version with its date, author and note. An earlier version opens read
  only; **Back to Latest** returns to editing.
- **Colours**: a preview of every colour with name, hex, RGB, role and shade. Click a swatch to edit
  it in FilmCraft's colour picker (visual picker, hex field, **R/G/B** fields, or the system picker
  where the host has one), with name, role (standard roles or your own), *Shade of* and shade label.
  **Add Colour**, **Duplicate**, move ◀ ▶ and **Remove** (refused while a rule or a shade uses the
  colour).
- **Usage rules**: each rule with a live preview of the combination ("Aa" in the foreground colour on
  the background colour). **Add a rule**: kind, foreground, background, what it applies to, and the
  rule in your words; guidelines take text and the colours they concern.
- **Save Version** (with an optional note of what changed) or **Discard**. Unsaved changes are
  marked, and switching palettes asks to save or discard first.

## Commands

| Command | Does |
|---|---|
| `palettes.list {query?, includeArchived?}` | palettes with id, name, latest version, colour and rule counts, swatches |
| `palettes.get {id, version?, history?}` | a palette and one version (default the latest): colours with exact `hex` and `rgb`, roles, shades; rules with colour names; every version's summary and fingerprint |
| `palettes.resolve {ref}` | one colour by reference: `pal_…@3/col_…` (version 3), `pal_…/col_…` (latest), or `{palette, version?, color}`; the colour may be named. Returns its exact value and the rules that concern it |
| `palettes.create {name, description?, colors?, rules?, by?, note?}` | a new palette (version 1). Colours take `hex` (`#RRGGBB`) or `rgb` (`[r, g, b]`); rules and shades may name colours |
| `palettes.update {id, baseVersion, changes? \| colors? \| rules?, by?, note?}` | saves a new version. `changes`: `addColor`, `updateColor`, `duplicateColor`, `removeColor`, `moveColor`, `addRule`, `updateRule`, `removeRule`. No change, no new version |
| `palettes.setInfo {id, name?, description?}` | rename / describe |
| `palettes.duplicate {id, version?, name?}` | a new palette from a version |
| `palettes.archive {id, archived?}` | archive (default) or restore |
| `palettes.delete {id, confirm: true}` | delete a palette and every version |
| `palettes.addToProjectColors {id, version?}` | add the colours to the project's swatches (`colors.addCustom`) |
| `window.paletteLibrary {select?}` | open the window (on a palette) |

Agents should pass `by` (who is saving) and a `note`; both are stored with the version.

```sh
filmcraft-cli exec palettes.create '{"name": "Client X", "by": "claude", "colors": [{"name": "Navy", "hex": "#0B1F3A", "role": "primary"}, {"name": "Paper", "rgb": [250, 248, 240], "role": "background"}], "rules": [{"kind": "permitted", "foreground": "Navy", "background": "Paper", "appliesTo": "text"}]}'
filmcraft-cli exec palettes.get '{"id": "pal_…"}'
filmcraft-cli exec palettes.resolve '{"ref": "pal_…@1/col_…"}'
filmcraft-cli exec palettes.update '{"id": "pal_…", "baseVersion": 1, "by": "claude", "note": "add accent", "changes": [{"op": "addColor", "color": {"name": "Coral", "hex": "#FF6F59", "role": "accent"}}]}'
```

## Integration

- **Font Library (2A)** and palettes share one data directory and one storage layer
  (`crates/engine/src/user_library.rs`).
- **Brand kits (2C)** will reference palette versions by `pal_…@N` plus fingerprint, and roles and
  rules by id. Deleting a palette is not yet blocked by references; 2C adds that.
- **Brand application and QC (2D)** read exact values and the stated rules through the same model
  (`palettes.resolve`); no rule is evaluated against a picture yet.

## Limits

- Colours are opaque sRGB (`#RRGGBB`): no alpha, no other colour spaces.
- Name, description and archived state are not versioned (their changes are in the history).
- One library per data directory; there is no sharing between machines yet (the ARCA app interface
  is planned to read and write through the same commands).
