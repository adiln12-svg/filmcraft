# Co-editing with Claude

You and Claude edit the **same** FilmCraft project: one project, one timeline, one undo history and
one set of engine commands. Claude connects through the control channel
(`filmcraft --control <port>`, see [control-protocol.md](control-protocol.md)), the MCP bridge
(`filmcraft-cli mcp --bridge`) or the headless MCP server (`filmcraft-cli mcp --project …`). There
is no second editing engine and no copy of the project; media stays where it is.

## What you see

- **Claude's changes appear immediately** in the timeline, the monitor and every panel. The status
  bar says what Claude did ("Claude: Trim — 1 clip changed").
- **History.** Each change Claude makes is one undo step named `Claude: …` (Edit ▸ Undo, the
  History panel, the toolbar's Undo). A set of commands Claude sends as a transaction is a single
  step.
- **The Changes panel** (Creator Mode: next to Properties; Pro: Window ▸ Changes) lists Claude's
  changes, newest first: when, what, how much changed, which clips, and whether it succeeded, was
  blocked by a protection or was not applied because the project had changed. The latest one has
  **Undo**; a large one has **Restore the version before**. In Pro the panel also shows revision
  numbers and the commands.
- **Versions.** **Save a version now** keeps a copy of the whole project; FilmCraft also keeps one
  automatically before a large change by Claude (a transaction, removed clips or tracks, five or
  more clips changed) and before any restore. **Restore** brings a version back as one undoable
  step; the state it replaces is kept as a version first. Versions are project files only
  (`<data dir>/Project Versions/<project>/`), at most 100 per project.

## Protecting your work

Select clips and click **Protect** (Creator toolbar) or **Clip ▸ Protect from Claude**; for a whole
track, right-click its header ▸ **Protect Track from Claude**. Protected clips show a gold lock, as do
protected track headers. Protection is saved with the project.

- You can still edit protected work yourself.
- Claude cannot change, move, trim, delete, regrade or ripple anything protected. When a command would,
  nothing at all is changed, and Claude is told which elements were in the way.
- Claude cannot remove a protection. It can ask: a window appears ("Claude asks to change protected
  work", with its reason and the elements). **Allow once** lets it make that one change;
  **Keep protected** refuses. The request also shows in the Changes panel.

## Conflicts and safety

- **Revisions.** The project's content revision (`projectRevision`) changes with every edit (not
  with playback, selection or render previews). Claude sends the revision it last saw; if you
  changed the project since, its command is refused (`CONFLICT`) with what changed, nothing is
  applied, and Claude looks again before retrying. FilmCraft never merges uncertain edits.
- **Transactions** are all or nothing: if one step fails or touches protected work, none is
  applied.
- **Closing your work.** When Claude opens, reverts or replaces a project that has unsaved changes,
  those changes are kept first as a version (Changes ▸ Versions ▸ Restore brings them back).
- **Two programs, one file.** Saving over a project file another program changed since it was
  opened or saved here is refused ("Save As" keeps both; `file.save {force: true}` overwrites).
- **Crashes.** The recovery journal keeps unsaved changes, Claude's included; the next launch offers
  to recover them. A command that fails or is interrupted leaves the project as it was.
- **Reconnecting.** Claude's activity is stored next to the project's versions and comes back when
  the project is opened again; `agent.status` tells a newly connected agent where things stand.

## For agents

| Command | |
|---|---|
| `agent.status` | `projectRevision`, project and path, unsaved changes, sequences, protected ids, pending requests, last activity, latest version, undo/redo labels, recent changes |
| `agent.changes {since}` | changes after a revision: label, time, `by` (`user` / `agent`) |
| `agent.transaction {label, commands: [{command, params}], expectRevision?, snapshot?}` | atomic, one undo step `Claude: <label>`; a version first unless `snapshot: false` |
| `agent.activity {limit}` | Claude's actions as the person sees them |
| `protect.set {clips?, tracks?, on}` / `protect.toggle` / `protect.list` | protection (only the person can remove one) |
| `agent.requestAuthorization {elements, reason}` → `agent.authorizationStatus {request}` | ask; when allowed, pass the returned `authorization` with the command (control channel `engine.execute {…, authorization}` or the transaction's call); good for one successful edit |
| `versions.list` / `versions.create {label}` / `versions.restore {version}` | project versions |

Control channel: `engine.execute` and `ui.menu.invoke` take `expectRevision` and `authorization`
beside `command` and `params`, and every reply carries `projectRevision`. Errors start with a code:
`PROTECTED`, `CONFLICT`, `NOT_ALLOWED`. Clicks and keys the control channel injects
(`ui.click`, `ui.key`, …) into the desktop app count as Claude's too.

## Limits

- Protection covers clips, captions and tracks (video, audio, caption). Sequence settings,
  markers and project bins are not protectable yet.
- A protected clip blocks any change that would move it, including a ripple edit earlier on its
  track: Claude must ask, or edit after it.
- Conflicts are detected per project revision, not per clip: any change of yours since Claude's
  last look makes its next command stale, even an unrelated one.
- Claude's activity log is kept per project file on this computer (not inside the project file).
