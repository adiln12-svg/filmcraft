# Autonomous editing

How an agent takes a brief, footage and assets to a finished export with no person watching.
FilmCraft already does the editing (more than 700 commands over MCP, the control channel and
`filmcraft-cli`). What an agent lacks is *perception* and a *definition of done*: it can't watch
footage play, and "moody, fast, warm" is not something it can verify. The commands below give it
both. They live in `crates/engine/src/autoedit.rs` and have no menu entries; they are agent tools.

| Command | Replaces | Returns |
|---|---|---|
| `analyze.takes` | watching every take | 0–100 score per take (focus, exposure, clipping, motion), flags such as `soft`, `clipped highlights`, `heavy motion`; per-sample detail on request |
| `script.align` | scrubbing for each line | every spoken take of each script line across all transcripts, ranked: clean reads first, then fillers, stumbles and partial reads; lines never spoken in `missing` |
| `analyze.beats` | tapping along to the music | tempo, beat and downbeat times, onsets; optional Beat / Downbeat markers on the item or sequence |
| `qc.check` | a review screening | the *timeline* judged against an **edit spec**; `pass` is false while any error remains (use `qc.render` for the exported file) |
| `qc.filmstrip` | looking at each cut | one PNG: per cut, the frame 0.5 s before, the last frame before, the first frame after and 0.5 s after |

## Script-faithful dialogue assembly and rendered-output QC (Milestone 1)

| Command | Does |
|---|---|
| `script.parse` | Reads an approved script as written: ARCA `- [0–3s Hook] "…"` beats (label and timing kept as metadata, re-hook notes dropped), plain or numbered lines. Headers and `Label: value` metadata are skipped and listed, never guessed into dialogue. |
| `script.align` | Every take of every line, judged: word alignment (missing, misheard, extra words), the take's own audio (below) and its cut points. Takes come back usable-first with `flags`, `usable`, `cutIn` / `cutOut`. |
| `script.assemble` | Picks the best usable take per line, cuts it in silence, places the lines in script order and captions them with the **script's** words timed to the dialogue. Any line without a usable take → `BLOCKED`, naming the lines, and nothing is placed. |
| `qc.render` | Checks the **exported file**: it decodes, matches the frame size, rate and length; sampled frames match the edit (PSNR); every caption and title is in the file, inside the frame (titles by their full laid-out text, so a word pushed off the edge counts), inside the safe area; captions are at most N lines (renderer layout and measured pixels), readable (characters/s, time on screen, no overlaps) and identical to the script; loudness and true peak of the decoded sound; the file's audio is transcribed and every line must be spoken once, in order, without stumbles or unscripted speech. `COMPLETE` only when every hard check passes, else `FAILED`. |
| `autoedit.note` | Logs an agent's correction or decision. |
| `autoedit.report` | The decision log and the final state: `BLOCKED` (last assembly blocked), `COMPLETE` / `FAILED` (last check of an exported file), `FAILED` when nothing exported was checked after the last assembly. |

**Take checks that do not trust the transcript.** Whisper tidies speech: a stumble or false start can
disappear from the text while the word around it gets longer. On a 10 ms level envelope of the
take's own audio FilmCraft flags *hidden speech* (a word holding far more voiced time than it
needs), *repeated audio* (the same 0.3 s of sound again within 1.5 s), pauses over `maxPause`,
noise and clipped samples, and finds cut points in the silence around the voiced speech (a hard
`clipped_*` flag when there is less than 30 ms of silence). Misheard words inside an otherwise
matching read are *resolved* (logged; captions keep the script text); words missing at an edge where
the audio shows speech are recogniser misses (timing interpolated); missing words with no speech, two
or more added words, repeats and unverifiable noise are hard.

**Hard vs soft.** Hard flags make a take unusable (`incomplete`, `missing_words`, `extra_words`,
`repeated_words`, `hidden_speech`, `repeated_audio`, `clipped_in` / `clipped_out`, `too_noisy`,
`unresolved`, `low_score`, `no_audio`); soft flags only lower its rank (`pause`, `noisy`, `fillers`,
`asr_mismatch`, `unrecognised_edge_words`, `longer_than_other_takes`, `distorted`). In `qc.render`
every check is hard except caption time on screen.

**Measured limits (whisper-base, LibriSpeech read speech).** On 69 clean, correct reads, 2 were
rejected (2.9 %, both multi-word proper names the recogniser mangled) and 4 short lines were not
found; rejections fail safe (the line blocks, nothing wrong is placed). Takes made from one
performance with edits (stumbles, cut lines, pauses, noise) were judged correctly; choosing between
genuinely different performances is untested. Noisy takes (under 15 dB) are flagged soft; in them
cut points are less reliable.

Existing commands they work with: `transcript.generate` (speech to text, word timings),
`transcript.createCaptions`, `clip.sceneEditDetection`, `scopes.read` (colour as numbers),
`lumetri.applyMatch` (shot matching), `essentialSound.autoMatch` and
`essentialSound.generateDucking` (loudness and ducking), `file.exportMedia`.

## The loop

1. **Turn the brief into a spec.** Everything checkable goes into the `qc.check` spec (below):
   runtime, pacing, loudness target for the platform, caption rules, whether cuts follow the
   music. Creative intent that is not checkable (mood, look) becomes reference frames and
   `scopes.read` targets.
2. **Ingest.** `file.import` the footage, music and effects; `transcript.generate` every clip
   with speech.
3. **Select.** `script.align` with the script finds the best read of every line; `analyze.takes`
   breaks ties and rejects soft, badly exposed or shaky takes; `clip.sceneEditDetection` splits
   long b-roll into shots.
4. **Assemble.** Place takes in script order (`sequence.insert` / `sequence.overwrite` with the
   take's `startTicks` / `endTicks` as source in and out); lay the music; `analyze.beats` on the
   music (or the sequence) and snap cuts to beats or downbeats where the brief asks for it.
5. **Finish.** Grade (`lumetri.*`, `lumetri.applyMatch`, `scopes.read` to confirm), transitions,
   titles and captions (`transcript.createCaptions`, `captions.setStyle`), sound
   (`essentialSound.*`, effects, crossfades).
6. **Check, fix, repeat.** `qc.check` with the spec. Fix every error (and the warnings that
   matter), then check again until `pass` is true. `qc.filmstrip` gives one image of every cut
   to look over in a single pass.
7. **Deliver.** `file.exportMedia` for each deliverable; `qc.check` on each aspect-ratio version.

## The edit spec

`qc.check {"spec": {...}, "sampleEvery": 1}`. Every field is optional: a missing field uses its
default, `null` turns that check off. Times are seconds.

| Field | Default | Checks |
|---|---|---|
| `duration` | off | `{"min", "max"}`: total runtime |
| `minShotFrames` | `3` | clips shorter than this many frames are flash frames (error) |
| `maxShotSeconds` | off | shots (between cuts) longer than this (warning) |
| `averageShotSeconds` | off | `{"min", "max"}`: pacing (warning) |
| `allowGaps` | `false` | stretches with no enabled video clip, i.e. black in the export (error) |
| `blackFrames` | `{"maxLuma": 0.02}` | program frames sampled every `sampleEvery` s that are black inside clips (warning) |
| `loudness` | `{"integratedLufs": -14, "tolerance": 1, "truePeakMax": -1}` | BS.1770 integrated loudness and true peak of the mix (errors). Use −23 for EBU R128 broadcast, −24 for ATSC A/85, −16 for podcasts |
| `audioClicks` | `{"thresholdDb": -30}` | audio clip edges louder than the threshold with no transition over them (warning) |
| `captions` | `{"maxCps": 17, "minSeconds": 0.8, "maxLineChars": 42, "maxLines": 2}` | reading speed, time on screen, line length and count, overlaps |
| `beatSync` | off | `{"beats": [...], "toleranceFrames": 2, "minFraction": 0.8}`: share of cuts within the tolerance of a beat (pass `beats` from `analyze.beats`, in sequence time) |
| `colorContinuity` | off | `{"maxLumaDelta": 0.25, "maxChromaDelta": 0.06}`: brightness and colour jumps across cuts (scene changes are often meant to jump, so it is opt-in) |

The reply:

```json
{"pass": false, "errors": 2, "warnings": 1,
 "issues": [{"check": "flashFrame", "severity": "error", "seconds": 5.0, "clip": 412, "message": "Desert_Dunes.mp4 on V1 is 2 frame(s) long (minimum 3)"}],
 "metrics": {"durationSeconds": 26.75, "cuts": 9, "shots": 10, "averageShotSeconds": 2.675, "integratedLufs": -13.8, "truePeakDb": -1.4, "captions": 12, "cutsOnBeat": 0.9}}
```

## Examples

```sh
filmcraft-cli exec analyze.beats item=17 markers=true downbeatsOnly=true
filmcraft-cli exec script.align '{"script": "Line one.\nLine two.", "top": 3}'
filmcraft-cli exec analyze.takes '{"items": [17, 18, 19], "every": 0.5}'
filmcraft-cli exec qc.check '{"spec": {"duration": {"min": 28, "max": 30}, "averageShotSeconds": {"max": 2.5}, "loudness": {"integratedLufs": -14}}}'
filmcraft-cli exec qc.filmstrip '{"path": "/tmp/cuts.png", "limit": 20}'
```

## Limits

- These are measures, not taste. They catch what is wrong, not what is dull. Keep a person's
  final look at the cut when the stakes are high, or export alternatives for them to pick from.
- `analyze.takes` compares takes with each other (sharpness is relative to the sharpest sample
  of the call), so analyse the takes of one scene together.
- `analyze.beats` assumes one steady tempo per call; analyse sections separately for music that
  changes tempo. When it finds no steady beat (speech, ambience, free-time music: `confidence`,
  the share of onsets on the beat grid above chance, under 0.3) it returns `steadyBeat: false`
  with no tempo or beats, only the onsets. Beats are
  placed on the bass and kick when a grid half a beat away is as regular.
- Every command runs synchronously with capped work (frames, samples, words): 2,000 frames for
  `analyze.takes`, 15 minutes of audio for `analyze.beats`, 1,200 program frames for black-frame
  sampling, 40 rows per filmstrip.
