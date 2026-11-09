# `arca.brand/0.1` — proposed import contract

**Status: proposal.** A versioned JSON document in which ARCA hands FilmCraft a client's approved
brand. ARCA is the authority for approved creative and brand decisions; FilmCraft stores the assets,
validates them and renders with them. FilmCraft never edits or approves ARCA branding on its own.

FilmCraft implements a validator (`brand.arca.validate`, changes nothing) and an importer
(`brand.arca.import`) for this version. Nothing in the ARCA repository depends on it yet.

## Principles

1. **One library.** An import writes into FilmCraft's existing libraries: fonts into the Font
   Library, colours into a Palette Library palette (a new version only when they changed), logos into
   the client's brand assets, the kit into Brand Kits. The ARCA application can later manage the same
   records through the same `brand.*`, `fonts.library.*` and `palettes.*` commands; there is no
   second copy to keep in sync.
2. **ARCA decides.** A kit created from a document is *managed by ARCA*: read only in FilmCraft
   (`saveDraft` and FilmCraft approval are refused). Changes come as a new document revision, which
   adds a new kit version; earlier versions are never touched.
3. **Approval travels with the document.** A version is approved only when the document carries
   `approval.status: "approved"` with `by` and `reference`; FilmCraft records it with authority
   `ARCA`. Without it the import is a draft that only a later ARCA document can approve.
4. **Exact and verifiable.** Files are referenced relative to the document and carry their SHA-256;
   a mismatch, a file outside the document's folder, an unknown colour key or a missing licence
   rejects the whole document before anything is written.
5. **Idempotent.** Importing the same document again changes nothing.

## Document

```json
{
  "format": "arca.brand/0.1",
  "arca": {"authority": "ARCA", "document": "Clients/BackAlive/brand", "revision": "2026-11-09"},
  "client": {"id": "backalive", "name": "Back Alive Barbershop"},
  "kit": {"name": "Back Alive Core", "description": "Story canvas 9:16", "notes": ""},
  "fonts": [
    {"family": "Archivo", "style": "Black", "weight": 900, "file": "fonts/archivo.woff2",
     "sha256": "…", "license": "OFL-1.1", "licenseText": "…", "source": "Omnibus-Type/Archivo"}
  ],
  "palette": {
    "name": "Back Alive (ARCA)",
    "colors": [
      {"key": "blue", "name": "Blue", "hex": "#0248AD", "role": "primary"},
      {"key": "blueDeep", "name": "Blue Deep", "hex": "#022A66", "role": "primary", "shadeOf": "blue", "shade": "Deep"},
      {"key": "yellow", "name": "Yellow", "hex": "#FFF455", "role": "accent"},
      {"key": "white", "name": "White", "hex": "#FFFFFF", "role": "text"}
    ],
    "rules": [{"kind": "prohibited", "foreground": "yellow", "background": "white", "appliesTo": "text", "text": "Never yellow text on white"}]
  },
  "colors": [{"role": "focal", "color": "yellow", "notes": "headlines / CTA only"}],
  "typography": [
    {"role": "headline", "family": "Archivo", "style": "Black", "weight": 900,
     "size": {"value": 14.5, "unit": "%w", "minPx": 34, "maxPx": 150},
     "color": "white", "accent": "yellow", "background": null, "align": "left", "placement": "bottom-left",
     "case": "lower", "tracking": -0.03, "lineHeight": 0.92, "maxLines": null, "opacity": null, "styling": ""}
  ],
  "logos": [
    {"file": "logos/wordmark-white.png", "sha256": "…", "name": "Wordmark (white)", "variant": "white",
     "approved": true, "placement": "top-left", "marginPct": 6.5,
     "minWidth": {"value": 89, "unit": "px"}, "maxWidth": {"value": 206, "unit": "px"},
     "backgrounds": ["blue", "blueDeep"], "forbidden": [], "license": "", "notes": ""}
  ],
  "rules": [{"kind": "guideline", "text": "Yellow is the one focal point: headlines and CTA only"}],
  "safeArea": {"top": 9, "right": 6.5, "bottom": 15, "left": 6.5, "unit": "%w"},
  "referenceFrame": [1080, 1920],
  "approval": {"status": "approved", "by": "…", "reference": "…", "date": "2026-11-09"}
}
```

| Field | Required | Meaning |
|---|---|---|
| `format` | yes | exactly `arca.brand/0.1`; other versions are refused |
| `arca.authority`, `arca.document`, `arca.revision` | yes | the authority (`ARCA`) and the source record; stored as the kit's source reference |
| `client.id`, `client.name` | yes | ARCA's client id (FilmCraft client `source: arca:<id>`; reused on later imports) |
| `kit.name` | yes | the kit (one ARCA kit per client and name) |
| `fonts[]` | — | faces the kit uses. With `file` (+ `sha256`, `license` SPDX) the file is added to the Font Library when missing; without `file` the face must already be installed. `weight` must match the face |
| `palette.colors[]` | yes | `key` (used by every colour reference in the document), `name`, `hex`, `role`, `shadeOf` (a key), `shade` |
| `palette.rules[]` | — | `permitted` / `prohibited` (`foreground`, `background` keys, `appliesTo`) and `guideline` (`text`, `colors`) |
| `colors[]` | — | colour roles: `role`, `color` (key), `notes` |
| `typography[]` | — | as in a FilmCraft kit; colours by key; `size.unit` is `px`, `%w` or `%h` |
| `logos[]` | — | `file` + `sha256` (PNG, JPEG or WebP), usage rules as in a FilmCraft kit; colours by key |
| `rules[]` | — | kit rules (they may not contradict the palette's rules) |
| `safeArea`, `referenceFrame` | — | as in a FilmCraft kit |
| `approval` | — | `status` (`approved` to approve), `by`, `reference`, `date` |

## Behaviour

| Situation | Result |
|---|---|
| invalid document | nothing is written; `brand.arca.validate` lists every error |
| new client / kit | client (source `arca:<id>`) and kit (managed by ARCA) created; version 1 |
| same document again | nothing changes (same version returned) |
| new revision, changed content | a new version; an unapproved imported draft is replaced by the newer import, approved versions stay |
| `approval.status: approved` | the version is approved with authority `ARCA` (if FilmCraft's dependency check passes; otherwise the import reports why) |
| FilmCraft user or agent edits or approves an ARCA kit | refused: change it in ARCA and import |
| project pinned to an ARCA kit version | as for any kit: pinned exactly until someone authorises a move |

## Open questions for ARCA

- Where ARCA keeps logo files and font files for a client (the BackAlive design system has fonts but
  its wordmark is typographic, with no logo file).
- How ARCA records approval (who, reference), so `approval` can carry it verbatim.
- Whether one ARCA client can have several kits (formats, sub-brands), and how they are named.
