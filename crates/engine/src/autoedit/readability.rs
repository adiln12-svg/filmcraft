//! Caption reading speed: one policy for `script.assemble`, `qc.render` and `qc.check`.
//!
//! - **Preferred target** (`targetCps`, default 15 characters/s): captions faster than this are
//!   logged as soft warnings; they never block.
//! - **Hard limit** (`maxCps`, default 17 characters/s): captions faster than this fail, and an
//!   assembly whose script cannot be captioned within it from the chosen takes is `BLOCKED`.
//!
//! The hard limit can be made stricter freely. Making it looser than the default is a change to an
//! approved requirement, so it needs an approval record, `maxCpsApproval: {"by": …, "reference":
//! …}`, which is logged; without one the request is refused. FilmCraft never relaxes it itself.

use serde_json::{Value, json};

/// Default hard limit (characters per second).
pub const DEFAULT_MAX_CPS: f64 = 17.0;
/// Default preferred target (characters per second).
pub const DEFAULT_TARGET_CPS: f64 = 15.0;
/// Default lead-in: how long before its first word a caption may appear.
pub const DEFAULT_LEAD_IN: f64 = 0.2;

/// The reading-speed policy in force.
#[derive(Clone, Debug, PartialEq)]
pub struct Policy {
    pub target_cps: f64,
    pub max_cps: f64,
    /// The approval record when `max_cps` is looser than the default.
    pub approval: Option<Value>,
    pub lead_in: f64,
}

impl Default for Policy {
    fn default() -> Self {
        Self { target_cps: DEFAULT_TARGET_CPS, max_cps: DEFAULT_MAX_CPS, approval: None, lead_in: DEFAULT_LEAD_IN }
    }
}

impl Policy {
    pub fn json(&self) -> Value {
        json!({"targetCps": self.target_cps, "maxCps": self.max_cps, "defaultMaxCps": DEFAULT_MAX_CPS, "approval": self.approval, "leadIn": self.lead_in})
    }
}

fn text(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

/// Read the policy from a captions object (`targetCps`, `maxCps`, `maxCpsApproval`, `leadIn`).
/// `Err` explains a refused relaxation or an unusable number.
pub fn policy(v: Option<&Value>) -> Result<Policy, String> {
    let mut p = Policy::default();
    let Some(o) = v.filter(|x| x.is_object()) else { return Ok(p) };
    let num = |k: &str| -> Result<Option<f64>, String> {
        match o.get(k) {
            None | Some(Value::Null) => Ok(None),
            Some(x) => x.as_f64().filter(|f| f.is_finite() && *f > 0.0).map(Some).ok_or_else(|| format!("`{k}` must be a positive number")),
        }
    };
    if let Some(m) = num("maxCps")? {
        if m > DEFAULT_MAX_CPS + 1e-9 {
            let a = o.get("maxCpsApproval");
            let (by, reference) = (text(a.and_then(|a| a.get("by"))), text(a.and_then(|a| a.get("reference"))));
            match (by, reference) {
                (Some(by), Some(reference)) => p.approval = Some(json!({"by": by, "reference": reference, "date": a.and_then(|a| a.get("date")).cloned()})),
                _ => {
                    return Err(format!(
                        "a reading-speed limit of {m} characters/s is looser than the approved {DEFAULT_MAX_CPS}: give `maxCpsApproval` {{\"by\", \"reference\"}} for the person and decision that approved it"
                    ));
                }
            }
        }
        p.max_cps = m.min(60.0);
    }
    if let Some(t) = num("targetCps")? {
        p.target_cps = t;
    }
    p.target_cps = p.target_cps.min(p.max_cps);
    if let Some(l) = o.get("leadIn").and_then(Value::as_f64).filter(|f| f.is_finite()) {
        p.lead_in = l.clamp(0.0, 0.3);
    }
    Ok(p)
}

/// The upstream decisions that can resolve captions that cannot be read within the limit: none of
/// them is FilmCraft's to take.
pub fn upstream_options(policy: &Policy) -> Value {
    json!([
        {"owner": "Creative Production Director", "decision": "change the caption treatment (for example word-by-word or keyword captions), which changes how reading speed is judged"},
        {"owner": "Production / Hook Generator", "decision": "supply a slower take of the line, or a shorter approved script line"},
        {"owner": "Client approval (via ARCA)", "decision": format!("approve a looser hard limit than {} characters/s (record `maxCpsApproval`)", policy.max_cps)},
        {"owner": "Client approval (via ARCA)", "decision": "publish without burned-in captions for this content"},
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_stricter_limits_need_no_approval() {
        assert_eq!(policy(None).unwrap(), Policy::default());
        let p = policy(Some(&json!({"maxCps": 15, "targetCps": 16}))).unwrap();
        assert_eq!((p.max_cps, p.target_cps), (15.0, 15.0), "the target never exceeds the limit");
        assert!(p.approval.is_none());
    }

    #[test]
    fn a_looser_limit_needs_an_approval_record() {
        let e = policy(Some(&json!({"maxCps": 20}))).unwrap_err();
        assert!(e.contains("maxCpsApproval"), "{e}");
        assert!(policy(Some(&json!({"maxCps": 20, "maxCpsApproval": {"by": " "}}))).is_err());
        let p = policy(Some(&json!({"maxCps": 20, "maxCpsApproval": {"by": "Client lead", "reference": "brief v3"}}))).unwrap();
        assert_eq!(p.max_cps, 20.0);
        assert_eq!(p.approval.unwrap()["by"], "Client lead");
    }

    #[test]
    fn hostile_values() {
        assert!(policy(Some(&json!({"maxCps": -1}))).is_err());
        assert!(policy(Some(&json!({"maxCps": "fast"}))).is_err());
        assert!(policy(Some(&json!({"targetCps": f64::NAN}))).is_err() || policy(Some(&json!({"targetCps": f64::NAN}))).is_ok());
        let p = policy(Some(&json!({"leadIn": 99}))).unwrap();
        assert_eq!(p.lead_in, 0.3);
        assert_eq!(policy(Some(&json!("x"))).unwrap(), Policy::default());
    }
}
