# Creator Mode

Creator Mode is FilmCraft's simple way to make short social videos. It is not a second editor: it
uses the same project, timeline, commands and undo history as the professional workspaces and as
agents (CLI, control channel, MCP). Switch between the two at any time with **Creator / Pro** in
the header; nothing about the project changes.

## Start screen

FilmCraft opens on the Start screen (Settings ▸ General ▸ At Startup ▸ Show Home, the default; `--demo`
opens the demo project, `--empty` an empty project in the editor).

- **New video in a shape**: Vertical 9:16 (TikTok, Reels, Shorts, Stories), Landscape 16:9
  (YouTube), Square 1:1, Portrait 4:5 (Instagram feed). Click one and choose your clips.
- **Drop clips** anywhere on the screen: a new video in the selected shape.
- Recent projects, Open project…, Import into the current project…, the demo project.

The clips go on the timeline back to back **in file-name order** (natural order: `clip 2` before
`clip 10`, whatever order the file manager hands them over in), each filling the frame. Audio-only
files go on their own track from the start, as music. The editor opens at once with the video
selected; there is no import page to leave.

## The Creator workspace

| Area | What it does |
|---|---|
| **Assets** | Media (Add media…, add or replace with a project clip), Music (Add music…), Text (add a title at the playhead), Captions (add a caption at the playhead; captions from speech when a speech-to-text model is installed), Transitions (a cross dissolve or dip to black at every cut), Looks (colour looks for the selected clips, or every clip) |
| **Program** | the video, with play, the playhead and the safe-area overlays of the Pro monitor |
| **Properties** | what the selected thing has: a clip's size, position, opacity, volume and speed; a title's font, size and colour; a caption's text |
| **Timeline** | drag clips to move them, drag a clip's edge to trim it, drop files to add them; the toolbar above it |

The toolbar: **Undo**, **Redo**, **Split** (cut the clip at the playhead), **Trim start** /
**Trim end** (remove the part of the clip before / after the playhead and close the gap),
**Delete** (remove the selected clips and close the gap; also the Delete key in Creator Mode),
**Replace…** (another file in the selected clip's place, same length), **+ Media**, **+ Text**,
**Pro**, and **Export video**.

Files dropped on the window in Creator Mode are added at the end of the video, in file-name order;
dropped on the Timeline they go where they are dropped.

## Export video

**Export video** (toolbar, or File ▸ Export ▸ Export Video…) asks only what matters:

| Choice | |
|---|---|
| Where will you post it? | TikTok, Instagram Reels, YouTube Shorts (1080×1920), YouTube (1920×1080), Instagram Square (1080×1080), Instagram Portrait (1080×1350), Same as my video. The one matching the video's shape is chosen |
| File name | without extension: `.mp4` is added once (a typed `.mp4` is not doubled); an existing file is never replaced: `My Video 2.mp4` |
| Save to | the project's folder, else Movies, else the home folder; **Change…** picks another |
| Show captions in the video | on when the video has captions |
| Fast export | hardware encoding and GPU rendering (Export ▸ Hardware encoding and GPU rendering set to Auto): on by default on macOS, off elsewhere; FilmCraft's own encoder and renderer are used whenever they are not available |

**Export** starts it (progress and the written path show in the dialog). **More settings…** opens
the full Export page. The social presets are FilmCraft's own (H.264 + AAC, the short-form ones
loudness-normalised to −14 LUFS).

## Commands

Every Creator action is a command, for agents and scripts as much as for the interface:

| Command | Does |
|---|---|
| `creator.newVideo {aspect: "9:16"\|"16:9"\|"1:1"\|"4:5", paths?, items?, name?, keepOrder?, fps?}` | a new sequence at the aspect (1080 px on the short side) with the files imported and placed in file-name order, each filling the frame; audio-only files on a new track from 0. One undo step |
| `creator.addMedia {paths?, items?, at: "end"\|"playhead", keepOrder?}` | import and append at the end of the video, in order. One undo step |
| `creator.trimToPlayhead {side: "start"\|"end"}` | ripple-trim the selected clip (or the one under the playhead) to the playhead |
| `creator.transitions {effect?, frames?}` | a transition at every cut of the main video track. One undo step |
| `creator.exportPresets` / `creator.export {preset?, name?, folder?, overwrite?, burnCaptions?, hardwareAcceleration?, wait?}` | social export with one extension and no overwriting |
| `view.creatorMode {on?}`, `creator.start`, `creator.chooseClips {aspect, paths?}`, `creator.exportDialog` | the interface (UI commands) |

Editing in Creator Mode uses the same commands as the Pro workspace (`sequence.addEdit`,
`edit.rippleDelete`, `clip.replaceFromBin`, `graphics.newText`, `captions.add`,
`lumetri.applyPreset`, …), so everything can be undone, inspected and continued in Pro.
