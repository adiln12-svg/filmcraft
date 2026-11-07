//! Script-faithful dialogue assembly: the take analysis shared by `script.align` and
//! `script.assemble`, and the assembly itself.
//!
//! For every script line, every spoken take found in the transcripts is judged on three kinds of
//! evidence: the word alignment (missing, misheard and extra words), the take's own audio
//! ([`super::takecheck`]: hidden speech, repetition, pauses, noise), and the cut points (can the
//! take be cut in silence?). Hard findings make a take unusable. `script.assemble` places the best
//! usable take of each line in script order and captions the result with the script's own words.
//! If any line has no usable take it changes nothing and reports `BLOCKED`, naming the lines.

use filmcraft_project::ItemId;
use serde_json::{Value, json};

use super::align::{self, Op, TWord, Take};
use super::script::ScriptLine;
use super::takecheck::{self, Envelope, Flag, Limits, Severity};
use crate::Session;

/// Options shared by `script.align` and `script.assemble`.
#[derive(Clone, Copy, Debug)]
pub struct Opts {
    pub min_score: f32,
    pub pad_in: f64,
    pub pad_out: f64,
    pub limits: Limits,
}

impl Default for Opts {
    fn default() -> Self {
        Self { min_score: 0.5, pad_in: 0.08, pad_out: 0.12, limits: Limits::default() }
    }
}

/// A transcribed media item.
pub struct Source {
    pub item: ItemId,
    pub name: String,
    pub words: Vec<TWord>,
    /// Mono 16 kHz audio of the whole item (`None` when it could not be read).
    pub audio: Option<Vec<f32>>,
    pub duration: f64,
}

/// The transcribed items (all of them, or those listed in `wanted`).
pub fn sources(s: &Session, wanted: Option<&[u64]>) -> Vec<Source> {
    s.project
        .transcripts
        .iter()
        .filter(|(id, _)| wanted.is_none_or(|w| w.contains(&id.0)))
        .map(|(id, t)| Source {
            item: *id,
            name: s.project.item(*id).map(|i| i.name.clone()).unwrap_or_default(),
            words: t.words.iter().map(|w| TWord { text: w.text.clone(), start: w.start.seconds(), end: w.end.seconds(), confidence: w.confidence }).collect(),
            audio: crate::transcript::item_audio(s, *id),
            duration: s.source(*id).map(|m| m.info().duration.seconds()).unwrap_or(0.0),
        })
        .collect()
}

/// One take of one line, judged.
#[derive(Clone, Debug)]
pub struct Judged {
    pub take: Take,
    pub item: ItemId,
    pub name: String,
    pub ops: Vec<Op>,
    pub flags: Vec<Flag>,
    /// Media seconds to cut at (in silence where possible).
    pub cut_in: f64,
    pub cut_out: f64,
    pub voiced: f64,
    pub snr_db: f32,
    pub usable: bool,
    pub rank: f32,
    /// Misheard stretches the alignment resolved: (script words, heard words).
    pub resolved: Vec<(String, String)>,
}

impl Judged {
    pub fn hard(&self) -> impl Iterator<Item = &Flag> {
        self.flags.iter().filter(|f| f.severity == Severity::Hard)
    }
}

fn flag(kind: &'static str, severity: Severity, at: Option<f64>, detail: String) -> Flag {
    Flag { kind, severity, at, detail }
}

/// What the word alignment says about a take: missing / extra words, repeats, fillers, and the
/// misheard stretches it could resolve.
fn word_flags(line: &[(usize, String)], display: &[&str], ops: &[Op], hyp: &[TWord]) -> (Vec<Flag>, Vec<(String, String)>) {
    let mut flags = Vec::new();
    let mut resolved = Vec::new();
    let n = line.len().max(1);
    let script_word = |i: usize| line.get(i).and_then(|(d, _)| display.get(*d)).copied().unwrap_or("");
    let at = |j: usize| hyp.get(j).map(|w| w.start);
    let matched = ops.iter().filter(|o| matches!(o, Op::Match(..))).count();
    let paired = matched + ops.iter().filter(|o| matches!(o, Op::Sub(..))).count();
    if (paired as f32) < 0.6 * n as f32 {
        flags.push(flag("unresolved", Severity::Hard, None, format!("only {paired} of {n} script words line up with what was said")));
    }
    // regions between exact matches
    let first_m = ops.iter().position(|o| matches!(o, Op::Match(..)));
    let last_m = ops.iter().rposition(|o| matches!(o, Op::Match(..)));
    let mut k = 0;
    while k < ops.len() {
        if matches!(ops.get(k), Some(Op::Match(..))) {
            k += 1;
            continue;
        }
        let a = k;
        while k < ops.len() && !matches!(ops.get(k), Some(Op::Match(..))) {
            k += 1;
        }
        let region = ops.get(a..k).unwrap_or(&[]);
        let dels: Vec<usize> = region.iter().filter_map(|o| if let Op::Del(i) = o { Some(*i) } else { None }).collect();
        let ins: Vec<usize> = region
            .iter()
            .filter_map(|o| if let Op::Ins(j) = o { Some(*j) } else { None })
            .filter(|j| hyp.get(*j).is_some_and(|w| !align::is_filler(&align::norm(&w.text))))
            .collect();
        let subs: Vec<(usize, usize)> = region.iter().filter_map(|o| if let Op::Sub(i, j) = o { Some((*i, *j)) } else { None }).collect();
        let edge_head = first_m.is_none_or(|m| a < m);
        let edge_tail = last_m.is_none_or(|m| a > m);
        let words = |v: &[usize]| v.iter().map(|i| script_word(*i)).collect::<Vec<_>>().join(" ");
        if (edge_head || edge_tail) && dels.len() >= 2 && subs.is_empty() {
            flags.push(flag(
                "incomplete",
                Severity::Hard,
                None,
                format!("not spoken at the {}: \"{}\"", if edge_head { "start" } else { "end" }, words(&dels)),
            ));
            continue;
        }
        let net = ins.len() as i64 - dels.len() as i64;
        let heard = |v: &[usize]| v.iter().filter_map(|j| hyp.get(*j)).map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        // words added with nothing of the script replaced are an ad-lib; a script word heard as a
        // few words (a misheard name) is a mishearing up to two extra words
        let replaced = !dels.is_empty() || !subs.is_empty();
        let added = ins.len() >= 2 && !replaced || net >= 3;
        let dropped = (dels.len() >= 2 && ins.is_empty() && subs.is_empty()) || net <= -3;
        if added {
            flags.push(flag(
                "extra_words",
                Severity::Hard,
                ins.first().and_then(|j| at(*j)),
                format!("words spoken that are not in the script: \"{}\"", heard(&ins)),
            ));
        } else if dropped {
            flags.push(flag("missing_words", Severity::Hard, None, format!("script words not heard: \"{}\"", words(&dels))));
        } else if !subs.is_empty() || !dels.is_empty() || !ins.is_empty() {
            let mut sw: Vec<usize> = subs.iter().map(|x| x.0).chain(dels.iter().copied()).collect();
            sw.sort_unstable();
            let mut hw: Vec<usize> = subs.iter().map(|x| x.1).chain(ins.iter().copied()).collect();
            hw.sort_unstable();
            resolved.push((words(&sw), heard(&hw)));
        }
    }
    // repeated words and fillers the recogniser did write down
    let spoken: Vec<String> = hyp.iter().map(|w| align::norm(&w.text)).filter(|w| !w.is_empty() && !align::is_filler(w)).collect();
    if let Some(p) = spoken.windows(2).position(|w| w[0] == w[1]) {
        flags.push(flag("repeated_words", Severity::Hard, None, format!("\"{0} {0}\": a stumble", spoken.get(p).map(String::as_str).unwrap_or(""))));
    }
    let fillers = hyp.iter().filter(|w| align::is_filler(&align::norm(&w.text))).count();
    if fillers > 0 {
        flags.push(flag("fillers", Severity::Soft, None, format!("{fillers} filler word(s)")));
    }
    if !resolved.is_empty() {
        let d = resolved.iter().map(|(a, b)| format!("\"{a}\" heard as \"{b}\"")).collect::<Vec<_>>().join("; ");
        flags.push(flag("asr_mismatch", Severity::Soft, None, format!("resolved by the alignment, script text kept: {d}")));
    }
    (flags, resolved)
}

/// The alignment starts a take at its first well-heard word, so a misheard first or last word
/// (often a name) is left outside it. Pull in transcript words that run on from the take without
/// a pause (< 0.35 s) and plausibly are the missing script words (one edit away, or the same first
/// letter), at most as many as are missing at that edge.
fn extend_edges(mut take: Take, words: &[TWord], script: &[String]) -> Take {
    let plausible = |w: &TWord, s: &str| {
        let h = align::norm(&w.text);
        !h.is_empty() && !align::is_filler(&h) && (align::similar(&h, s) || h.chars().next() == s.chars().next())
    };
    for _ in 0..2 {
        let Some(span) = words.get(take.word_first..=take.word_last) else { break };
        let ops = align::word_ops(script, span);
        let head: Vec<usize> = ops.iter().take_while(|o| matches!(o, Op::Del(_))).filter_map(|o| if let Op::Del(i) = o { Some(*i) } else { None }).collect();
        let tail: Vec<usize> =
            ops.iter().rev().take_while(|o| matches!(o, Op::Del(_))).filter_map(|o| if let Op::Del(i) = o { Some(*i) } else { None }).collect();
        let mut changed = false;
        // backwards: the word before pairs with the last missing head word
        for si in head.iter().rev() {
            let (Some(prev), Some(cur)) = (take.word_first.checked_sub(1).and_then(|i| words.get(i)), words.get(take.word_first)) else { break };
            if cur.start - prev.end > 0.35 || !plausible(prev, script.get(*si).map(String::as_str).unwrap_or("")) {
                break;
            }
            take.word_first -= 1;
            take.start = prev.start;
            changed = true;
        }
        // forwards: the word after pairs with the first missing tail word
        for si in tail.iter().rev() {
            let (Some(cur), Some(next)) = (words.get(take.word_last), words.get(take.word_last + 1)) else { break };
            if next.start - cur.end > 0.35 || !plausible(next, script.get(*si).map(String::as_str).unwrap_or("")) {
                break;
            }
            take.word_last += 1;
            take.end = next.end;
            changed = true;
        }
        if !changed {
            break;
        }
        take.text = words.get(take.word_first..=take.word_last).unwrap_or(&[]).iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    }
    take
}

const SOFT_PENALTY: &[(&str, f32)] = &[("noisy", 0.10), ("pause", 0.08), ("longer_than_other_takes", 0.10), ("distorted", 0.10), ("fillers", 0.05)];

/// Judge every take of every line. Returns per line the takes, best first (usable ones first).
pub fn analyze(lines: &[ScriptLine], src: &[Source], opts: &Opts) -> Vec<Vec<Judged>> {
    let texts: Vec<String> = lines.iter().map(|l| l.text.clone()).collect();
    let words: Vec<Vec<TWord>> = src.iter().map(|x| x.words.clone()).collect();
    let found = align::align(&texts, &words, 50, opts.min_score);
    let envs: Vec<Option<Envelope<'_>>> = src.iter().map(|x| x.audio.as_deref().map(|a| Envelope::new(a, filmcraft_speech::SAMPLE_RATE))).collect();
    lines
        .iter()
        .zip(found)
        .map(|(line, takes)| {
            let toks = align::script_words(&line.text);
            let display: Vec<&str> = line.text.split_whitespace().collect();
            let script_toks: Vec<String> = toks.iter().map(|t| t.1.clone()).collect();
            let mut judged: Vec<Judged> = takes
                .into_iter()
                .filter_map(|take| {
                    let source = src.get(take.source)?;
                    let take = extend_edges(take, &source.words, &script_toks);
                    let span = source.words.get(take.word_first..=take.word_last)?;
                    let ops = align::word_ops(&script_toks, span);
                    let (mut flags, resolved) = word_flags(&toks, &display, &ops, span);
                    let (mut cut_in, mut cut_out) = (take.start - opts.pad_in, take.end + opts.pad_out);
                    let (mut voiced, mut snr) = (take.end - take.start, 0.0);
                    match envs.get(take.source).and_then(Option::as_ref) {
                        Some(env) => {
                            let audio = takecheck::check(env, span, &opts.limits);
                            flags.extend(audio.flags);
                            voiced = audio.voiced;
                            snr = audio.levels.snr_db;
                            let prev = take.word_first.checked_sub(1).and_then(|i| source.words.get(i));
                            let next = source.words.get(take.word_last + 1);
                            let len = source.duration.max(take.end);
                            let n = takecheck::Neighbours {
                                prev_start: prev.map(|w| w.start + 0.05).unwrap_or(0.0),
                                prev_end: prev.map(|w| w.end).unwrap_or(0.0),
                                next_start: next.map(|w| w.start).unwrap_or(len),
                                next_end: next.map(|w| w.end - 0.05).unwrap_or(len),
                            };
                            let (i, o, f) = takecheck::cut_points(env, &audio.levels, span, opts.pad_in, opts.pad_out, n);
                            // words the recogniser missed at an edge, where the audio shows speech:
                            // not an incomplete read
                            let edge_speech = |a: f64, b: f64| takecheck::voiced_between(env, &audio.levels, a, b);
                            for fl in flags.iter_mut().filter(|fl| fl.kind == "incomplete") {
                                let tail = fl.detail.contains("at the end");
                                let missing = fl.detail.matches(' ').count() + 1;
                                let present = if tail { edge_speech(take.end, o) } else { edge_speech(i, take.start) };
                                if present >= 0.12 * missing as f64 {
                                    fl.kind = "unrecognised_edge_words";
                                    fl.severity = Severity::Soft;
                                    fl.detail = format!(
                                        "{} (but {present:.2} s of speech is there: the recogniser missed them; caption timing interpolated)",
                                        fl.detail
                                    );
                                }
                            }
                            (cut_in, cut_out) = (i, o);
                            flags.extend(f);
                        }
                        None => flags.push(flag("no_audio", Severity::Hard, None, "the take's audio could not be read, so it cannot be verified".into())),
                    }
                    if take.score < opts.min_score {
                        flags.push(flag("low_score", Severity::Hard, None, format!("alignment score {:.2} is under {:.2}", take.score, opts.min_score)));
                    }
                    Some(Judged {
                        item: source.item,
                        name: source.name.clone(),
                        ops,
                        flags,
                        cut_in: cut_in.max(0.0),
                        cut_out: cut_out.max(cut_in),
                        voiced,
                        snr_db: snr,
                        usable: false,
                        rank: 0.0,
                        resolved,
                        take,
                    })
                })
                .collect();
            // a take much longer than the line's other takes probably holds a restart
            let mut v: Vec<f64> = judged.iter().map(|j| j.voiced).collect();
            v.sort_by(f64::total_cmp);
            if v.len() >= 2 {
                let median = v.get(v.len() / 2).copied().unwrap_or(0.0);
                let shortest = v.first().copied().unwrap_or(0.0);
                for j in &mut judged {
                    if j.voiced > median.max(shortest) + (0.15 * shortest).max(0.45) {
                        j.flags.push(flag(
                            "longer_than_other_takes",
                            Severity::Soft,
                            None,
                            format!("{:.2} s of speech against {shortest:.2} s for the shortest take: possible restart", j.voiced),
                        ));
                    }
                }
            }
            for j in &mut judged {
                let usable = !j.flags.iter().any(|f| f.severity == Severity::Hard);
                j.usable = usable;
                let penalty: f32 = j.flags.iter().filter_map(|f| SOFT_PENALTY.iter().find(|p| p.0 == f.kind).map(|p| p.1)).sum();
                j.rank = j.take.score + 0.02 * j.take.confidence - penalty;
            }
            judged.sort_by(|a, b| b.usable.cmp(&a.usable).then(b.rank.total_cmp(&a.rank)));
            judged
        })
        .collect()
}

pub fn flag_json(f: &Flag) -> Value {
    json!({"kind": f.kind, "severity": f.severity.as_str(), "seconds": f.at.map(|t| (t * 1000.0).round() / 1000.0), "detail": f.detail})
}

pub fn judged_json(j: &Judged) -> Value {
    let r3 = |v: f64| (v * 1000.0).round() / 1000.0;
    json!({
        "item": j.item.0, "name": j.name,
        "start": r3(j.take.start), "end": r3(j.take.end),
        "startTicks": filmcraft_time::Tick::from_seconds_f64(j.take.start).0, "endTicks": filmcraft_time::Tick::from_seconds_f64(j.take.end).0,
        "cutIn": r3(j.cut_in), "cutOut": r3(j.cut_out),
        "score": r3(j.take.score as f64), "coverage": r3(j.take.coverage as f64), "rank": r3(j.rank as f64),
        "usable": j.usable,
        "flags": j.flags.iter().map(flag_json).collect::<Vec<_>>(),
        "fillers": j.take.fillers, "repeats": j.take.repeats,
        "confidence": r3(j.take.confidence as f64), "wordsPerSecond": r3(j.take.pace as f64),
        "voicedSeconds": r3(j.voiced), "snrDb": (j.snr_db as f64).round(),
        "text": j.take.text,
    })
}

/// The decision for one line: the take to use, or why none can be.
pub fn choose(line: &ScriptLine, takes: &[Judged]) -> std::result::Result<usize, Value> {
    match takes.iter().position(|t| t.usable) {
        Some(i) => Ok(i),
        None => Err(json!({
            "line": line.n,
            "text": line.text,
            "reason": if takes.is_empty() { "no take of this line was found in the transcripts".to_string() } else { format!("none of the {} take(s) is usable", takes.len()) },
            "rejected": takes.iter().map(|t| json!({"item": t.item.0, "name": t.name, "start": t.take.start, "end": t.take.end,
                "hard": t.hard().map(flag_json).collect::<Vec<_>>()})).collect::<Vec<_>>(),
        })),
    }
}

// ---------------------------------------------------------------------------------------- command

use crate::commands::{bad, bool_p, f64_p, str_p, u64_p};
use crate::{EngineError, Result};

fn finite(p: &Value, k: &str) -> Option<f64> {
    f64_p(p, k).filter(|v| v.is_finite())
}

pub fn opts_from(p: &Value) -> Opts {
    let d = Opts::default();
    Opts {
        min_score: finite(p, "minScore").unwrap_or(d.min_score as f64).clamp(0.0, 1.0) as f32,
        pad_in: finite(p, "padIn").unwrap_or(d.pad_in).clamp(0.0, 1.0),
        pad_out: finite(p, "padOut").unwrap_or(d.pad_out).clamp(0.0, 1.0),
        limits: Limits { max_pause: finite(p, "maxPause").unwrap_or(d.limits.max_pause).clamp(0.1, 10.0), ..d.limits },
    }
}

/// Append to the session's decision log (and return the entry).
pub fn log(s: &mut Session, stage: &str, event: &str, detail: Value) -> Value {
    let e = json!({"seq": s.autoedit_log.len() + 1, "stage": stage, "event": event, "detail": detail});
    if s.autoedit_log.len() < 100_000 {
        s.autoedit_log.push(e.clone());
    }
    e
}

/// Caption style for script captions: the default face, sized so a 9:16 frame fits about 22
/// characters per line, on a translucent box, kept above the bottom safe area.
pub fn default_caption_style(vertical: bool) -> Value {
    if vertical {
        json!({"size": 46, "color": "#ffffff", "background": true, "backgroundColor": "#000000b0", "anchor": "bottom", "margin": 0.24, "align": "center"})
    } else {
        json!({"size": 44, "color": "#ffffff", "background": true, "backgroundColor": "#000000b0", "anchor": "bottom", "margin": 0.08, "align": "center"})
    }
}

/// `script.assemble`.
pub fn assemble(s: &mut Session, p: &Value) -> Result<Value> {
    let cmd = "script.assemble";
    let script = str_p(p, "script").ok_or_else(|| bad(cmd, "need `script`: the approved script, as written"))?;
    let (lines, skipped) = super::script::parse(script);
    // reading-speed policy: `readability` object, or the same keys at the top level
    let mut rp = p.get("readability").filter(|v| v.is_object()).cloned().unwrap_or(json!({}));
    for k in ["maxCps", "targetCps", "maxCpsApproval", "leadIn"] {
        if rp.get(k).is_none()
            && let Some(v) = p.get(k)
        {
            rp[k] = v.clone();
        }
    }
    let captions_on = bool_p(p, "captions").unwrap_or(true);
    let policy = super::readability::policy(Some(&rp)).map_err(|e| bad(cmd, e))?;
    if captions_on {
        log(s, "captions", "policy", policy.json());
    }
    log(
        s,
        "script",
        "parsed",
        json!({"lines": lines.iter().map(|l| json!({"line": l.n, "beat": l.beat, "target": l.target, "text": l.text})).collect::<Vec<_>>(),
               "skipped": skipped.iter().map(|k| json!({"raw": k.raw, "reason": k.reason})).collect::<Vec<_>>()}),
    );
    if lines.is_empty() {
        let e = log(s, "assemble", "BLOCKED", json!({"reason": "the script has no dialogue lines"}));
        return Ok(json!({"state": "BLOCKED", "blocked": [e["detail"].clone()], "log": [e]}));
    }
    let wanted: Option<Vec<u64>> = p.get("items").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect());
    let src = sources(s, wanted.as_deref());
    if src.is_empty() {
        let e = log(s, "assemble", "BLOCKED", json!({"reason": "no transcribed footage: run transcript.generate on the takes first"}));
        return Ok(json!({"state": "BLOCKED", "blocked": [e["detail"].clone()]}));
    }
    let opts = opts_from(p);
    let mut judged = analyze(&lines, &src, &opts);
    // among usable takes, prefer those that can be captioned within the hard limit
    if captions_on {
        let hold = (opts.pad_in + opts.pad_out).min(0.7);
        for (line, takes) in lines.iter().zip(judged.iter_mut()) {
            let chars = line.text.chars().count() as f64;
            let feasible = |t: &Judged| chars / ((t.take.end - t.take.start) + policy.lead_in + hold).max(0.05) <= policy.max_cps;
            takes.sort_by(|a, b| b.usable.cmp(&a.usable).then(feasible(b).cmp(&feasible(a))).then(b.rank.total_cmp(&a.rank)));
        }
    }
    // decide every line before touching the timeline
    let mut picks = Vec::new();
    let mut blocked = Vec::new();
    for (line, takes) in lines.iter().zip(&judged) {
        for t in takes {
            log(s, "takes", "considered", json!({"line": line.n, "take": judged_json(t)}));
        }
        match choose(line, takes) {
            Ok(i) => {
                let t = takes.get(i).ok_or_else(|| EngineError::Other("take index".into()))?;
                let why = if i == 0 && takes.len() == 1 {
                    "the only usable take".to_string()
                } else {
                    let skipped_unusable = takes.iter().take(i).count();
                    format!(
                        "best usable take (rank {:.2}) of {}{}",
                        t.rank,
                        takes.len(),
                        if skipped_unusable > 0 { format!("; {skipped_unusable} better-scoring take(s) rejected") } else { String::new() }
                    )
                };
                log(
                    s,
                    "takes",
                    "selected",
                    json!({"line": line.n, "item": t.item.0, "name": t.name, "cutIn": t.cut_in, "cutOut": t.cut_out, "why": why,
                    "softFlags": t.flags.iter().map(flag_json).collect::<Vec<_>>()}),
                );
                for r in takes.iter().filter(|x| !x.usable) {
                    log(
                        s,
                        "takes",
                        "rejected",
                        json!({"line": line.n, "item": r.item.0, "start": r.take.start, "end": r.take.end, "hard": r.hard().map(flag_json).collect::<Vec<_>>()}),
                    );
                }
                picks.push((line.clone(), t.clone()));
            }
            Err(b) => {
                log(s, "takes", "BLOCKED", b.clone());
                blocked.push(b);
            }
        }
    }
    if !blocked.is_empty() {
        let e = log(
            s,
            "assemble",
            "BLOCKED",
            json!({"lines": blocked.iter().map(|b| b["line"].clone()).collect::<Vec<_>>(), "reason": "lines without a usable take; nothing was placed"}),
        );
        return Ok(json!({"state": "BLOCKED", "blocked": blocked, "summary": e["detail"]}));
    }
    // can the captions be read within the hard limit? Planned on the cut as it will be placed,
    // before anything is placed
    if captions_on {
        let size = match p.get("sequence").filter(|v| v.is_object()) {
            Some(spec) => {
                (spec.get("width").and_then(Value::as_u64).unwrap_or(1920) as usize, spec.get("height").and_then(Value::as_u64).unwrap_or(1080) as usize)
            }
            None => s.active_sequence().map(|q| (q.settings.width as usize, q.settings.height as usize)).unwrap_or((1920, 1080)),
        };
        let cfg = caption_cfg(p, size.0.clamp(16, 16_384), size.1.clamp(16, 16_384), &policy);
        let mut t0 = finite(p, "start").unwrap_or(0.0).max(0.0);
        let virt: Vec<(&ScriptLine, &Judged, f64, f64, f64)> = picks
            .iter()
            .map(|(l, t)| {
                let d = (t.cut_out - t.cut_in).max(0.04);
                let v = (l, t, t0, t.cut_in, t0 + d);
                t0 += d;
                v
            })
            .collect();
        let lw = line_words(s, &virt);
        let plan = super::capscript::plan(&lw, &cfg.timing, &|ws: &[String]| fit(&cfg, ws));
        let reading: Vec<Value> = plan.lines.iter().map(reading_json).collect();
        let over: Vec<Value> = plan.lines.iter().filter(|r| r.worst_cps > policy.max_cps + 1e-6).map(reading_json).collect();
        let above_target: Vec<Value> =
            plan.lines.iter().filter(|r| r.worst_cps > policy.target_cps + 1e-6 && r.worst_cps <= policy.max_cps + 1e-6).map(reading_json).collect();
        log(s, "captions", "readability", json!({"policy": policy.json(), "lines": reading, "aboveTarget": above_target, "overLimit": over}));
        if !over.is_empty() {
            let detail = json!({
                "kind": "caption_reading_speed",
                "lines": over.iter().map(|r| r["line"].clone()).collect::<Vec<_>>(),
                "reason": format!("the approved script cannot be captioned within the {} characters/s limit from the chosen takes: the delivery is faster than the captions can be read while staying in sync", policy.max_cps),
                "readingSpeed": over,
                "policy": policy.json(),
                "upstreamDecision": super::readability::upstream_options(&policy),
            });
            if str_p(p, "onInfeasible").unwrap_or("block") != "continue" {
                let e = log(s, "assemble", "BLOCKED", detail.clone());
                return Ok(json!({"state": "BLOCKED", "blocked": [e["detail"].clone()], "summary": e["detail"]["reason"]}));
            }
            // assemble anyway for review; the exported file cannot pass qc.render
            log(s, "captions", "over_limit_continue", detail);
        }
    }
    // the sequence: a new one, or the active one
    if let Some(spec) = p.get("sequence").filter(|v| v.is_object()) {
        let r = s.execute("file.newSequence", spec.clone())?;
        log(s, "assemble", "sequence", json!({"created": r, "settings": spec}));
    }
    let seq_id = s.state.active_sequence.ok_or_else(|| bad(cmd, "open a sequence or pass `sequence` settings"))?;
    let (vt, at) = (str_p(p, "videoTrack").unwrap_or("V1").to_string(), str_p(p, "audioTrack").unwrap_or("A1").to_string());
    let mut t = finite(p, "start").unwrap_or(0.0).max(0.0);
    let mut placed = Vec::new();
    for (line, take) in &picks {
        let dur = (take.cut_out - take.cut_in).max(0.04);
        let r = s.execute(
            "timeline.place",
            json!({"item": take.item.0, "track": vt, "audioTrack": at, "seconds": t, "sourceIn": {"seconds": take.cut_in}, "duration": {"seconds": dur}}),
        )?;
        let clips: Vec<u64> = r["clips"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
        // where the clip really landed (frame-snapped)
        let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
        let (start, source_in) = clips
            .first()
            .and_then(|c| q.find_item(filmcraft_project::ClipId(*c)))
            .map(|(_, it)| (it.start.seconds(), it.source_in.seconds()))
            .unwrap_or((t, take.cut_in));
        let end = start + dur;
        log(
            s,
            "edit",
            "placed",
            json!({"line": line.n, "item": take.item.0, "clips": clips, "sequenceStart": start, "sequenceEnd": end, "sourceIn": source_in, "sourceOut": source_in + dur}),
        );
        placed.push((line.clone(), take.clone(), start, source_in, end, clips));
        t = end;
    }
    // captions from the script
    let mut caption_out = Value::Null;
    if captions_on {
        caption_out = script_captions(s, &placed, p, &policy)?;
    }
    let total = t;
    let e = log(s, "assemble", "ASSEMBLED", json!({"lines": placed.len(), "seconds": total}));
    Ok(json!({
        "state": "ASSEMBLED",
        "sequence": seq_id.0,
        "seconds": total,
        "lines": placed.iter().map(|(l, tk, st, si, en, clips)| json!({"line": l.n, "item": tk.item.0, "name": tk.name, "sequenceStart": st, "sequenceEnd": en,
            "sourceIn": si, "clips": clips, "take": judged_json(tk)})).collect::<Vec<_>>(),
        "captions": caption_out,
        "summary": e["detail"],
    }))
}

type Placed = (ScriptLine, Judged, f64, f64, f64, Vec<u64>);

/// Everything that decides how script captions are cut and drawn.
struct CapCfg {
    style_v: Value,
    style: filmcraft_project::CaptionStyle,
    h: usize,
    wrap_w: usize,
    max_lines: usize,
    timing: super::capscript::Timing,
}

fn caption_cfg(p: &Value, w: usize, h: usize, policy: &super::readability::Policy) -> CapCfg {
    let mut style_v = default_caption_style(h > w);
    if let Some(o) = p.get("captionStyle").and_then(Value::as_object) {
        for (k, v) in o {
            style_v[k] = v.clone();
        }
    }
    let mut style = filmcraft_project::CaptionStyle::default();
    if let Some(v) = style_v.get("size").and_then(Value::as_f64) {
        style.size = v as f32;
    }
    if let Some(f) = style_v.get("font").and_then(Value::as_str) {
        style.font = f.into();
    }
    // wrap inside the platform safe area, box padding included: the renderer wraps at 90% of the
    // width it is given, and pads each line's box by about a third of the text size
    let (_, _, sl, sr) = super::render::safe_area(p.get("safeArea"));
    let px = style.size.clamp(4.0, 256.0) as f64 * h as f64 / 1080.0;
    let usable = (1.0 - sl - sr) * w as f64 - 2.0 * (0.34 * px + 2.0);
    let wrap_w = ((usable / 0.9).floor() as usize).clamp(1, w.max(1));
    let timing = super::capscript::Timing {
        max_cps: policy.max_cps,
        lead_in: policy.lead_in,
        min_seconds: finite(p, "minCaptionSeconds").unwrap_or(0.8).clamp(0.1, 10.0),
        ..Default::default()
    };
    CapCfg { style_v, style, h, wrap_w, max_lines: u64_p(p, "captionLines").unwrap_or(2).clamp(1, 3) as usize, timing }
}

fn rendered_lines(cfg: &CapCfg, text: &str) -> usize {
    let c = filmcraft_project::Caption {
        id: filmcraft_project::ClipId(0),
        start: filmcraft_time::Tick::ZERO,
        duration: filmcraft_time::Tick(1),
        text: text.into(),
        speaker: None,
        cue_id: None,
        settings: String::new(),
    };
    filmcraft_captions::burn::layout_lines(&c, &cfg.style, cfg.wrap_w, cfg.h).len()
}

/// The cue text for `ws` with its line breaks when it renders within the allowed lines.
fn fit(cfg: &CapCfg, ws: &[String]) -> Option<String> {
    let t = ws.join(" ");
    if rendered_lines(cfg, &t) == 1 {
        return Some(t);
    }
    if cfg.max_lines < 2 {
        return None;
    }
    // two balanced lines, each drawn as one line
    (1..ws.len())
        .filter_map(|k| {
            let (a, b) = (ws.get(..k)?.join(" "), ws.get(k..)?.join(" "));
            (rendered_lines(cfg, &a) == 1 && rendered_lines(cfg, &b) == 1).then(|| (a.chars().count().abs_diff(b.chars().count()), format!("{a}\n{b}")))
        })
        .min_by_key(|x| x.0)
        .map(|x| x.1)
}

/// Each line's script words with the times of the transcript words they were aligned with, in
/// sequence seconds for a take placed at `start` with media `source_in`.
fn line_words(s: &Session, placed: &[(&ScriptLine, &Judged, f64, f64, f64)]) -> Vec<super::capscript::LineWords> {
    let mut lw = Vec::new();
    for (line, take, start, source_in, end) in placed {
        let src_words: Vec<TWord> = s
            .project
            .transcripts
            .get(&take.item)
            .map(|t| t.words.iter().map(|w| TWord { text: w.text.clone(), start: w.start.seconds(), end: w.end.seconds(), confidence: w.confidence }).collect())
            .unwrap_or_default();
        let span = src_words.get(take.take.word_first..=take.take.word_last).unwrap_or(&[]);
        let toks = align::script_words(&line.text);
        let display: Vec<String> = line.text.split_whitespace().map(String::from).collect();
        let mut times: Vec<Option<(f64, f64)>> = vec![None; display.len()];
        for op in &take.ops {
            if let Op::Match(i, j) | Op::Sub(i, j) = op
                && let (Some((d, _)), Some(wd)) = (toks.get(*i), span.get(*j))
                && let Some(slot) = times.get_mut(*d)
            {
                let map = |m: f64| (start + (m - source_in)).clamp(*start, *end);
                *slot = Some((map(wd.start), map(wd.end)));
            }
        }
        lw.push(super::capscript::LineWords { line: line.n, words: display, times, window: (*start, *end) });
    }
    lw
}

fn reading_json(r: &super::capscript::LineReading) -> Value {
    let r1 = |v: f64| (v * 10.0).round() / 10.0;
    json!({"line": r.line, "chars": r.chars, "seconds": (r.seconds * 100.0).round() / 100.0, "lowerBoundCps": r1(r.bound_cps), "achievedCps": r1(r.worst_cps)})
}

/// Captions whose text is the script, timed by the aligned words of the placed takes.
fn script_captions(s: &mut Session, placed: &[Placed], p: &Value, policy: &super::readability::Policy) -> Result<Value> {
    let q = s.active_sequence().ok_or(EngineError::NoSequence)?;
    let cfg = caption_cfg(p, q.settings.width as usize, q.settings.height as usize, policy);
    let rate = q.settings.frame_rate;
    let lw = line_words(s, &placed.iter().map(|(l, t, a, b, c, _)| (l, t, *a, *b, *c)).collect::<Vec<_>>());
    let plan = super::capscript::plan(&lw, &cfg.timing, &|ws: &[String]| fit(&cfg, ws));
    let mut cues = plan.cues;
    // whole frames, never overlapping: captions.add floors the start and rounds the length
    let fd = rate.frame_duration();
    let mut prev_end = filmcraft_time::Tick::ZERO;
    let mut frames: Vec<(filmcraft_time::Tick, filmcraft_time::Tick)> = Vec::new();
    for c in &cues {
        let a = rate.snap_nearest(filmcraft_time::Tick::from_seconds_f64(c.start)).max(prev_end);
        let b = rate.snap_nearest(filmcraft_time::Tick::from_seconds_f64(c.end)).max(a + fd);
        frames.push((a, b));
        prev_end = b;
    }
    for (c, (a, b)) in cues.iter_mut().zip(&frames) {
        c.start = a.seconds();
        c.end = b.seconds();
    }
    let tr = s.execute("captions.newTrack", json!({"format": "Subtitle", "name": str_p(p, "captionTrackName").unwrap_or("Script captions")}))?;
    let track = tr["track"].clone();
    let mut out = Vec::new();
    for c in &cues {
        let (a, b) = frames.get(out.len()).copied().unwrap_or_default();
        s.execute("captions.add", json!({"track": track, "text": c.text, "time": a.0, "durationSeconds": (b - a).seconds()}))?;
        out.push(json!({"line": c.line, "start": c.start, "end": c.end, "text": c.text, "cps": (c.cps() * 10.0).round() / 10.0, "interpolatedWords": c.interpolated}));
    }
    let mut style_cmd = cfg.style_v.clone();
    style_cmd["track"] = track.clone();
    s.execute("captions.setStyle", style_cmd)?;
    let resolved: Vec<Value> =
        placed.iter().flat_map(|(l, t, ..)| t.resolved.iter().map(move |(a, b)| json!({"line": l.n, "script": a, "heard": b}))).collect();
    let reading: Vec<Value> = plan.lines.iter().map(reading_json).collect();
    let detail = json!({"track": track, "cues": out.len(), "source": "approved script text", "resolvedMismatches": resolved,
        "interpolatedWords": cues.iter().map(|c| c.interpolated).sum::<usize>(), "style": cfg.style_v, "readingSpeed": reading, "policy": policy.json()});
    log(s, "captions", "created", detail.clone());
    Ok(
        json!({"track": track, "cues": out, "resolvedMismatches": detail["resolvedMismatches"], "readingSpeed": detail["readingSpeed"], "policy": detail["policy"]}),
    )
}
