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
| `qc.check` | a review screening | the sequence judged against an **edit spec**; `pass` is false while any error remains |
| `qc.filmstrip` | looking at each cut | one PNG: per cut, the frame 0.5 s before, the last frame before, the first frame after and 0.5 s after |

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
  changes tempo.
- Every command runs synchronously with capped work (frames, samples, words): 2,000 frames for
  `analyze.takes`, 15 minutes of audio for `analyze.beats`, 1,200 program frames for black-frame
  sampling, 40 rows per filmstrip.
