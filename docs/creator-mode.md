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
| **Assets** | Media (Add media…, a thumbnail per clip; drag a clip onto the Timeline, add it at the end or replace the selected clip with it), Music (Add music…, and Sound: fade in and out, lower the music when someone speaks, mark the beats, reduce background noise), Text (add a title at the playhead), Captions (Auto captions, caption looks and positions, add a caption, Import / Export SRT, every caption's text, start and end), Transitions (a cross dissolve or dip to black at every cut), Adjust (speed 0.5×–2×, steady shaky footage, colour looks for the selected clips, or every clip) |
| **Program** | the video, with play, the playhead and the safe-area overlays of the Pro monitor |
| **Properties** | what the selected thing has: a clip's size, position, opacity, volume and speed; a title's font, size and colour; a caption's text |
| **Timeline** | drag clips to move them, drag a clip's edge to trim it, drop files or Assets clips to add them; the toolbar above it. Snapping is on (clips, the playhead and markers, beat markers included). Thumbnails and waveforms on the clips. The tracks resize so every track with something on it stays in view: the music under the video is never scrolled out of sight |

The toolbar: **Undo**, **Redo**, **Split** (cut the selected clip, or the main-track clip under the playhead, and its own sound at the playhead; music and titles are not cut), **Trim start** /
**Trim end** (remove the part of the clip before / after the playhead and close the gap),
**Delete** (remove the selected clips and close the gap; also the Delete key in Creator Mode),
**Replace…** (another file in the selected clip's place, same length), **+ Media**, **+ Text**,
**Pro**, and **Export video**.

Files dropped on the window in Creator Mode are added at the end of the video, in file-name order;
dropped on the Timeline they go where they are dropped. Music goes on the first empty audio track
under the clips' sound (A2 in a new video).

**Shortcuts** (the same as Pro): Space play / pause, ← → one frame, Cmd/Ctrl+K split at the playhead,
Delete delete the selected clips and close the gap, Cmd/Ctrl+Z / Shift+Cmd/Ctrl+Z undo / redo, S
snapping on / off, = and − zoom, \ the whole video.

## Captions

**Auto captions** (Assets ▸ Captions) transcribes the video's speech with the installed
speech-to-text model (Settings ▸ Media Analysis & Transcription). Without one, the tab offers
**Download speech model** (about 150 MB, once, in the background with its progress; editing goes
on meanwhile). It makes short one-line captions (24 characters, at most 2.5 s each) with the time of every
word, and gives them the chosen look, all as one undo step.

| Look | |
|---|---|
| Clean | white text on a dark box |
| Bold Pop | big white capitals with a black outline; the spoken word in yellow |
| Karaoke | white text, the spoken word in green |
| Word by Word | words appear as they are spoken |
| Subtle | smaller text that fades in |

Positions: **Lower** (the default: above the bottom fifth of a 9:16 frame, where the apps put their
buttons and description), **Middle**, **Upper**. A look or position applies to the captions at once.
The spoken word's highlight and Word by Word use the word timings (or, for captions typed or
imported without them, the caption's time shared by word length). The monitor and the export draw
captions with the same code, so the preview is what is exported.

Each caption's text, start and end can be changed in the list (one undo step per change), on the
Timeline's caption track, or in Properties. **Import SRT…** / **Export SRT…** read and write SubRip
files (WebVTT too).

**Captions follow the footage.** Trim start / end, Delete, dragging a clip's edge, moving a clip and
changing its speed in Creator Mode move every caption with the words it belongs to: a caption over
a later clip moves with that clip; one whose footage is cut is shortened to what is left, or
removed when nothing is left; words keep their timing inside their caption. Captions over a gap
move with the next clip. It is part of the same undo step as the edit.

**4K footage.** When the video has clips larger than 1080p-class (more than 2.5 megapixels: 4K
phone video), Assets ▸ Media offers **Make editing copies**: small ProRes copies (¼ size) made in the
background and played in the monitors, so playback and scrubbing stay smooth on a laptop. Exports
always use the full-quality originals. **Play the originals** switches back
(`creator.smoothEditing {on}`, `creator.smoothEditingStatus`).

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
| `creator.exportPresets` / `creator.export {preset?, name?, folder?, overwrite?, burnCaptions?, hardwareAcceleration?, wait?, maxThreads?}` | social export with one extension and no overwriting (`maxThreads` caps its threads) |
| `creator.smoothEditing {on, wait?}` / `creator.smoothEditingStatus` | editing copies of 4K clips for the monitors |
| `creator.split` | cut the selected or main-track clip and its sound at the playhead (not the music) |
| `transcript.downloadModel {model?, background: true}` | the speech model downloaded as a job |
| `creator.trim {clip, edge, delta}`, `creator.delete {clips?}`, `creator.moveClip {clip, time}`, `creator.speed {clips?, speed}` | ripple edits of the main track; captions follow the footage. One undo step each |
| `creator.autoCaptions {preset?, position?, model?, language?, maxChars?}` | transcribe if needed, captions with word timings, a look. One undo step |
| `captions.presets` / `captions.applyPreset {preset, position?, track?}` | caption looks (`clean`, `boldPop`, `karaoke`, `wordByWord`, `subtle`) in a safe-area position (`lower`, `middle`, `upper`) |
| `captions.setStyle {…, highlight?, animation?, uppercase?}` | also the spoken word's colour, `none` / `fadeIn` / `wordByWord`, capitals |
| `creator.fade {clips?, edge?, seconds?}` | fade the music (or the selected sound) in and out (audio transitions) |
| `creator.duckMusic {clips?, reduceDb?}` | the music lowered while someone speaks (Essential Sound ducking keyframes) |
| `creator.beatMarkers {clip?}` | sequence markers on the music's beats (`analyze.beats`); snapping on |
| `creator.cleanVoice {clips?, amount?}` | noise reduction on the video's sound (Essential Sound ▸ Repair) |
| `view.creatorMode {on?}`, `creator.start`, `creator.chooseClips {aspect, paths?}`, `creator.exportDialog` | the interface (UI commands) |

Claude uses these commands too, through the same co-editing rules as everything else
([co-editing](co-editing.md)): one undo step labelled "Claude: …" per command or transaction, protected
clips and tracks refused (a ripple that would move protected captions is refused as a whole), the
activity log and the version snapshots.

Editing in Creator Mode uses the same commands as the Pro workspace (`sequence.addEdit`,
`edit.rippleDelete`, `clip.replaceFromBin`, `graphics.newText`, `captions.add`,
`lumetri.applyPreset`, …), so everything can be undone, inspected and continued in Pro.
