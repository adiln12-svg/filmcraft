//! `perf.stats`: performance counters an agent can read (headless engine part). The desktop UI
//! extends the same query with playback, frame-worker and UI timings (`ui-egui` `perf.rs`).

use serde_json::{Value, json};

use crate::Session;

/// How much memory FilmCraft's caches may use on this computer (M3C). Sized from the machine's
/// RAM less Settings ▸ Memory ▸ RAM reserved for other applications, so an 8 GB Mac keeps its
/// caches well inside what is left after the system and other applications: decoded source
/// frames, the monitors' rendered frames, GPU plans, recycled planes and float images, and how
/// many idle decoders stay alive. Caches in use (the frame on screen, the clips being played)
/// always keep what they need; these limits decide how much *idle* work is kept around.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPlan {
    /// Physical RAM (bytes); `None` when it could not be read (then 16 GB is assumed).
    pub total_ram: Option<u64>,
    /// RAM left to other applications.
    pub reserved: u64,
    /// RAM FilmCraft plans for.
    pub available: u64,
    pub decoded_frames: usize,
    pub frame_cache: usize,
    pub gpu_plans: usize,
    pub planes: usize,
    pub f32_images: usize,
    pub max_decoders: usize,
    pub hard_decoders: usize,
}

const GIB: u64 = 1 << 30;
const MIB: u64 = 1 << 20;

fn share(available: u64, percent: u64, lo: u64, hi: u64) -> usize {
    (available / 100 * percent).clamp(lo, hi) as usize
}

/// The plan for `total_ram` bytes of RAM with `reserved_gb` reserved for other applications and
/// the monitors' frame cache limited to `frame_cache_mb` (Settings ▸ Memory).
///
/// The reserve is at least 2 GB and at most half the RAM (an 8 GB Mac with the default 4 GB
/// reserve plans for 4 GB). Of what is left: 15 % for decoded source frames (256 MB–1 GB), 10 %
/// for the monitors' rendered frames (128 MB up to the setting), 10 % for GPU plans
/// (192 MB–1 GB), 2 % per plane type and 4 % for float images (up to the defaults), and 2–4
/// decoders kept by idle sources (3–8 by sources left a moment ago). With 16 GB or more every limit is at its previous default.
pub fn memory_plan(total_ram: Option<u64>, reserved_gb: u32, frame_cache_mb: u32) -> MemoryPlan {
    let total = total_ram.unwrap_or(16 * GIB).max(2 * GIB);
    let reserved = (u64::from(reserved_gb) * GIB).clamp(2 * GIB, total / 2);
    let available = total.saturating_sub(reserved);
    let (max_decoders, hard_decoders) = match available {
        a if a <= 3 * GIB => (2, 3),
        a if a <= 6 * GIB => (3, 4),
        _ => (4, 8),
    };
    MemoryPlan {
        total_ram,
        reserved,
        available,
        decoded_frames: share(available, 15, 256 * MIB, GIB),
        frame_cache: share(available, 10, 128 * MIB, (u64::from(frame_cache_mb) * MIB).max(128 * MIB)),
        gpu_plans: share(available, 10, 192 * MIB, GIB),
        planes: share(available, 2, 48 * MIB, 192 * MIB),
        f32_images: share(available, 4, 64 * MIB, 320 * MIB),
        max_decoders,
        hard_decoders,
    }
}

/// Physical RAM of this computer in bytes (Linux: `/proc/meminfo`, macOS: `sysctl hw.memsize`),
/// `None` where it cannot be read.
pub fn total_ram() -> Option<u64> {
    static RAM: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *RAM.get_or_init(read_total_ram)
}

fn read_total_ram() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/meminfo").ok()?;
        let kb: u64 = s.lines().find(|l| l.starts_with("MemTotal:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    }
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("/usr/sbin/sysctl").args(["-n", "hw.memsize"]).output().ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// Apply the decoder and buffer limits of `plan` (process-wide). The frontend applies the frame
/// cache and GPU plan limits to its frame server.
pub fn apply_memory_plan(plan: &MemoryPlan) {
    filmcraft_codecs::gop::set_limits(plan.decoded_frames, plan.max_decoders, plan.hard_decoders);
    filmcraft_frame::pool::set_limits(plan.planes, plan.f32_images);
}

/// `perf.memory`: the plan in force, as JSON.
pub fn memory_json(plan: &MemoryPlan) -> Value {
    let mb = |b: usize| b as f64 / MIB as f64;
    json!({
        "totalRamGB": plan.total_ram.map(|b| b as f64 / GIB as f64),
        "reservedGB": plan.reserved as f64 / GIB as f64,
        "availableGB": plan.available as f64 / GIB as f64,
        "decodedFramesMB": mb(plan.decoded_frames),
        "frameCacheMB": mb(plan.frame_cache),
        "gpuPlansMB": mb(plan.gpu_plans),
        "planesMB": mb(plan.planes),
        "floatImagesMB": mb(plan.f32_images),
        "maxDecoders": plan.max_decoders,
        "hardDecoders": plan.hard_decoders,
    })
}

/// Decoder / GOP-cache counters since the process started (they only grow: diff two readings to
/// measure an interval).
pub fn decode_json() -> Value {
    let g = filmcraft_codecs::gop_stats();
    let hw = filmcraft_codecs::hw::hw_stats();
    json!({
        "requests": g.hits + g.misses,
        "cacheHits": g.hits,
        "cacheMisses": g.misses,
        "cacheHitRate": g.hit_rate(),
        "seeks": g.seeks,
        "samplesDecoded": g.decoded,
        "samplesSkipped": g.skipped,
        "draftFrames": g.draft,
        "h264Threads": filmcraft_codecs::video::h264_threads(),
        "evicted": g.evicted,
        // sources that hold a decoder right now (idle ones give theirs up beyond a cap)
        "liveDecoders": filmcraft_codecs::live_decoders(),
        // decoded frames all sources hold, and the budget idle sources are trimmed to
        "cacheMB": filmcraft_codecs::cached_bytes() as f64 / 1e6,
        "cacheBudgetMB": filmcraft_codecs::gop::limits().0 as f64 / 1e6,
        // plane buffers of evicted frames waiting for the next decoded pictures, and how many
        // planes were decoded into a recycled buffer
        "planePoolMB": filmcraft_frame::pool::stats().idle_bytes as f64 / 1e6,
        "planesReused": filmcraft_frame::pool::stats().reused,
        "decodeMs": g.decode_ns as f64 / 1e6,
        "decodeMsPerSample": g.decode_ms_per_sample(),
        "framesDecoded": g.frames,
        // Settings ▸ Playback ▸ Hardware decoding: pictures from OS hardware decoders vs ours,
        // hardware decoders created, streams handed to software up front, mid-stream fallbacks.
        "hardware": {
            "enabled": filmcraft_codecs::hw::hardware_decoding(),
            // the OS decoder backend registered at startup (null where there is none)
            "backend": filmcraft_codecs::hw::hw_backend(),
            "frames": hw.frames,
            "softwareFrames": g.frames.saturating_sub(hw.frames),
            "sessions": hw.sessions,
            "declined": hw.declined,
            "fallbacks": hw.fallbacks,
        },
    })
}

/// Export counters: pictures encoded by hardware encoders, sessions, declined requests; and the wall
/// time of each export stage in milliseconds (`stages`: setup, loudness, render, encode with its
/// `convert` part, audio, mux, finish, wait), summed over the exports of this process (diff two readings).
/// Render and encode overlap (the next batch renders while this one is encoded), so their sum can exceed
/// the export's wall time; `waitMs` is the encoding side's idle wait for the render.
pub fn export_json() -> Value {
    let hw = filmcraft_export::hw_encode_stats();
    let stages: serde_json::Map<String, Value> =
        filmcraft_export::stage_times().into_iter().map(|(s, ns)| (format!("{}Ms", s.name()), json!(ns as f64 / 1e6))).collect();
    json!({"hardware": {"frames": hw.frames, "sessions": hw.sessions, "declined": hw.declined}, "stages": stages})
}

/// The engine's `perf.stats`.
pub fn stats(s: &Session) -> Value {
    let running = s.jobs.iter().filter(|j| j.result.lock().map(|r| r.is_none()).unwrap_or(false)).count();
    json!({
        "decode": decode_json(),
        "export": export_json(),
        "media": {"openSources": s.media.open_sources()},
        "jobs": {"total": s.jobs.len(), "running": running},
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::Session;

    #[test]
    fn perf_stats_query_reports_decode_counters() {
        let mut s = Session::default();
        assert!(s.execute("perf.stats", json!({})).is_ok(), "available without a project");
        s.execute("file.openDemoProject", json!({})).unwrap();
        let undo = s.history.undo.len();
        let v = s.execute("perf.stats", json!({})).unwrap();
        for k in [
            "requests",
            "cacheHitRate",
            "seeks",
            "samplesDecoded",
            "samplesSkipped",
            "draftFrames",
            "h264Threads",
            "decodeMs",
            "decodeMsPerSample",
            "framesDecoded",
            "liveDecoders",
            "cacheMB",
            "cacheBudgetMB",
            "planePoolMB",
            "planesReused",
        ] {
            assert!(v["decode"][k].is_number(), "decode.{k} in {v}");
        }
        for k in ["frames", "softwareFrames", "sessions", "declined", "fallbacks"] {
            assert!(v["decode"]["hardware"][k].is_number(), "decode.hardware.{k} in {v}");
        }
        assert!(v["decode"]["hardware"]["enabled"].is_boolean());
        assert!(v["decode"]["hardware"]["backend"].is_null() || v["decode"]["hardware"]["backend"].is_string());
        for k in ["frames", "sessions", "declined"] {
            assert!(v["export"]["hardware"][k].is_number(), "export.hardware.{k} in {v}");
        }
        for k in ["setupMs", "loudnessMs", "renderMs", "encodeMs", "convertMs", "audioMs", "muxMs", "finishMs", "waitMs"] {
            assert!(v["export"]["stages"][k].is_number(), "export.stages.{k} in {v}");
        }
        assert!(v["media"]["openSources"].is_number());
        assert_eq!(v["jobs"]["running"], json!(0));
        assert_eq!(s.history.undo.len(), undo, "a query adds no undo step");
    }
}

#[cfg(test)]
mod memory_plan_tests {
    use super::*;

    #[test]
    fn an_8_gb_mac_plans_well_inside_its_ram_and_16_gb_keeps_the_old_defaults() {
        let m1 = memory_plan(Some(8 * GIB), 4, 768);
        assert_eq!(m1.available, 4 * GIB);
        // everything the caches may hold idle, plus three decoders of ~300 MB, stays under 3 GB
        let caches = m1.decoded_frames + m1.frame_cache + m1.gpu_plans + 2 * m1.planes + m1.f32_images;
        assert!(caches + 3 * 300 * MIB as usize <= 3 * GIB as usize, "{m1:?}");
        assert_eq!((m1.max_decoders, m1.hard_decoders), (3, 4));
        assert!(m1.decoded_frames < filmcraft_codecs::FRAME_BUDGET);
        // the previous defaults where RAM allows
        let big = memory_plan(Some(32 * GIB), 4, 768);
        assert_eq!(big.decoded_frames, filmcraft_codecs::FRAME_BUDGET);
        assert_eq!(big.frame_cache, 768 * MIB as usize);
        assert_eq!(big.gpu_plans, GIB as usize);
        assert_eq!((big.max_decoders, big.hard_decoders), (4, 8));
        // a tiny machine still gets working minimums; the reserve never takes more than half
        let tiny = memory_plan(Some(4 * GIB), 8, 768);
        assert_eq!(tiny.available, 2 * GIB);
        assert_eq!((tiny.max_decoders, tiny.hard_decoders), (2, 3));
        assert!(tiny.decoded_frames >= 256 * MIB as usize);
        // unknown RAM plans as for 16 GB
        assert_eq!(memory_plan(None, 4, 768).available, 12 * GIB);
        // the user's frame cache setting is an upper bound
        assert_eq!(memory_plan(Some(32 * GIB), 4, 256).frame_cache, 256 * MIB as usize);
    }

    #[test]
    fn applying_a_plan_sets_the_decoder_limits() {
        let before = filmcraft_codecs::gop::limits();
        let p = memory_plan(Some(8 * GIB), 4, 768);
        apply_memory_plan(&p);
        assert_eq!(filmcraft_codecs::gop::limits(), (p.decoded_frames, 3, 4));
        filmcraft_codecs::gop::set_limits(before.0, before.1, before.2);
        assert!(total_ram().is_none_or(|r| r > 256 * MIB));
    }
}
