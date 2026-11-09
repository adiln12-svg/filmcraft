# Client Brand Kits

**Window ▸ Brand Kits** is the Client Brand Kit Builder: clients, their logos and brand graphics,
and versioned brand kits built from exact [Font Library](font-library.md) faces and exact
[Palette Library](palette-library.md) versions. Agents use the same functions as `brand.*` commands.
Everything is stored in `<data dir>/Brand Kits/` (`brand.json` and `assets/`), next to the Font and
Palette Libraries, independent of projects; a project records which approved kit version it uses.

Applying a kit to a video automatically and checking rendered output against it come in
Milestone 2D; this page covers managing kits.

## Model

| | |
|---|---|
| Client | `cli_…` id, name, notes, archived flag, history. Clients own their assets and kits; nothing is shared between clients |
| Brand asset | `ast_…` id: a logo or brand graphic of one client. PNG (with transparency), JPEG or WebP; decoded by FilmCraft's still decoder to validate it (up to 16 384 px a side, 64 MiB), stored by content hash with its size, format and SHA-256. SVG is refused with a message (not supported yet) |
| Brand kit | `kit_…` id, client, name, description, archived flag, versions, change record |
| Kit version | `draft` or `approved`. The latest version may be a draft: saving replaces it, and every save is logged with its fingerprint and note. **Approving freezes the version** (fingerprint, approver, reference, time, authority); approved versions never change. Editing an approved kit saves a new draft version based on it |
| Fonts | exact Font Library faces: library id + file SHA-256 + family + style + weight (variable-font instances such as Archivo Black 900 are faces of their own) |
| Palettes | exact versions: `pal_…@N` + fingerprint. A later palette version never changes the kit |
| Colour roles | named roles ("background", "focal", …) pointing at palette colours |
| Typography roles | headline, subtitle, caption, lower third, CTA, wordmark, kicker, body or any other name: font face and weight, size (`px`, `%w` or `%h` of the frame, optional min/max px like CSS `clamp()`), colour, background (box or pill), accent, alignment, placement anchor, case, tracking (em), line height, max lines, opacity, other styling as stated |
| Logo usage | per asset: variant, approved for use, placement anchor, clear space (% of width), minimum and maximum width, allowed backgrounds, forbidden alterations (recolor, stretch, rotate, crop, effects, outline, shadow, rearrange), notes |
| Rules | the palettes' own permitted / prohibited combinations and guidelines are kept exactly and shown with the kit; kit rules add to them (guidelines and combinations) and can never contradict them |
| Frame and safe area | the reference frame `px` sizes refer to, and the safe-area margins |

Nothing is invented: a kit holds only what someone entered or imported.

## Dependency check and approval

`brand.kits.check` (and the Overview tab) reports, for a kit version:

- a font that is not in the Font Library, whose file changed, whose style or **weight** is not
  available, or that does not render exactly (never silently substituted);
- a palette version that is missing or does not match its fingerprint; colour references that are
  not in the kit's palette versions;
- typography that uses a face the kit does not list, or a weight the face does not have;
- a logo that is missing, altered, or belongs to another client;
- rules that contradict the palette's rules or each other;
- an approved version that does not match its fingerprint.

**Approval** is explicit: the latest draft, saved, with no errors, an approver name, a reference to
the decision, and confirmation (`confirm: true`; in the window a tick box "I have the authority to
approve this client's branding"). Agents must not approve on their own: the command refuses without
the approval record, and the record names who approved.

## Protection

- A logo or graphic used by any kit version cannot be deleted (an approved version keeps its
  assets; a draft must drop it first).
- A Font Library font used by an approved kit cannot be removed (with or without `force`) or
  replaced by a different file.
- A palette used by an approved kit cannot be deleted (archive it instead); its versions never
  change anyway.

## Projects

`brand.project.pin` (Overview ▸ **Pin vN to the Current Project**) records the approved version in
the project file (`Project::brand`, schema v18): kit, version, fingerprint, client, who and when.

- Only approved, intact versions can be pinned.
- A newer approved version does not move the pin: `brand.project.get` reports it
  (`newerApprovedVersion`) and the project keeps its version until someone moves it with
  `authorizeUpdate: true` (window: "I authorise changing this project's branding").
- Pinning another client's kit needs `replaceClient: true` as well (window: "This project changes
  client"), so one client's branding never replaces another's by accident.
- `brand.project.get` checks the pin against the kit (missing, changed, ok) and lists the version's
  dependency issues. Pins and unpins are logged in the decision log.

## The window

Clients on the left (search, **New Client…**, rename, archive) with their kits (status per kit,
**New Brand Kit…**). The selected kit in tabs:

- **Overview**: versions (earlier ones open read-only), status, approval record, fingerprint,
  reference (`kit_…@N`), dependency check, **Approve Version…**, rename, archive, project pin.
- **Fonts**: the kit's faces with their status; every Font Library face as a tick box.
- **Colours**: palette versions (palette and exact version), colour roles with notes.
- **Typography**: reference frame and safe area; roles with every setting above.
- **Logos**: the client's assets with previews (**Import Logo…**, delete), and the kit's logo usage.
- **Rules**: the palettes' rules (read only) and the kit's rules.
- **Preview**: the kit drawn on a 9:16 or 16:9 frame with its background, safe area, logo and every
  typography role in its exact font, size, tracking, line height and colours (editable sample text).
- **History**: versions and the change record.

**Save Draft** (with a note) or **Discard** at the bottom; unsaved changes are marked and switching
kits asks to save or discard first. Kits managed by ARCA are read only.

## Commands

| Command | Does |
|---|---|
| `brand.clients.list {query?, includeArchived?}` / `get {id}` / `create {name, notes?}` / `update {id, name?, notes?, archived?}` | clients |
| `brand.assets.import {client, path, name?, kind?, variant?, source?, license?}` / `list {client}` / `remove {id}` | logos and brand graphics |
| `brand.kits.list {client?}` | kits with latest and approved versions |
| `brand.kits.get {id \| id@N, approved?, history?}` | a version with every reference resolved: font files and status, palette colours (hex, RGB) and rules, colour roles, typography (also the size in px at the reference frame), logos (file, pixels, rules), kit rules, safe area, approval, fingerprint, issues |
| `brand.kits.create {client, name, content?}` | draft version 1 |
| `brand.kits.saveDraft {id, baseVersion, content, by, note}` | save a draft (or start one from an approved version); `content` keys replace the version's; fonts may be given as `{family, style}`, palettes as `{palette, version}`, logos as `{asset}` — exact references are filled in and anything inexact is refused |
| `brand.kits.approve {id, version, approval: {by, reference}, confirm: true}` | approve, only with authorisation |
| `brand.kits.update {id, name?, description?, archived?}` / `check {id, version?}` | rename, archive; dependency check |
| `brand.resolve {kit: kit_…@N, what}` | one exact reference: `typography:<role>`, `color:<role>`, `logo:<variant or asset id>`, `font:<family> <style>`, `rules:` |
| `brand.project.get` / `pin {kit, by, authorizeUpdate?, replaceClient?}` / `unpin {authorizeUpdate}` | the project's brand kit |
| `brand.arca.validate {path \| document}` / `brand.arca.import {path \| document}` | ARCA brand documents ([arca-brand-0.1.md](arca-brand-0.1.md)) |
| `window.brandKits {client?, kit?, tab?, close?}` | the window |

## Limits

- SVG logos are not supported (PNG, JPEG, WebP only).
- Opaque sRGB colours only (no alpha); colour opacity (such as a muted 62 % white) is a typography
  setting, not a colour.
- The preview is a guide drawn in the interface (sizes, fonts, colours, wrapping); the authoritative
  rendering and rendered-output brand checks come with 2D.
- One library per data directory; sharing between machines and the ARCA app interface go through
  the same commands and the `arca.brand/0.1` contract.
