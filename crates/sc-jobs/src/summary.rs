//! Condenses LibreLane's `metrics.json` (hundreds of keys) into the handful
//! a designer looks at first, plus a sign-off verdict.

use serde_json::{Map, Value, json};

/// Metric keys copied into the summary, with the name used there.
const KEYS: &[(&str, &str)] = &[
    ("design__die__area", "die_area_um2"),
    ("design__core__area", "core_area_um2"),
    ("design__instance__count__stdcell", "stdcells"),
    ("design__instance__utilization", "utilization"),
    ("timing__setup__ws", "setup_wns_ns"),
    ("timing__setup__tns", "setup_tns_ns"),
    ("timing__hold__ws", "hold_wns_ns"),
    ("timing__hold__tns", "hold_tns_ns"),
    ("power__total", "power_total_w"),
    ("ir__drop__worst", "ir_drop_worst_v"),
    ("ir__drop__avg", "ir_drop_avg_v"),
    ("route__wirelength", "wirelength_um"),
    ("magic__drc_error__count", "drc_magic"),
    ("klayout__drc_error__count", "drc_klayout"),
    ("route__drc_errors", "drc_route"),
    ("design__lvs_error__count", "lvs_errors"),
    ("antenna__violating__nets", "antenna_nets"),
    ("design__max_slew_violation__count", "max_slew_violations"),
    ("design__max_cap_violation__count", "max_cap_violations"),
    ("design__max_fanout_violation__count", "max_fanout_violations"),
];

/// Parses LibreLane's `metrics.json`, which is not strict JSON: unconstrained
/// timing paths are written as bare `Infinity`/`-Infinity`/`NaN`. Those become `null`.
pub fn parse_metrics(bytes: &[u8]) -> serde_json::Result<Value> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = String::with_capacity(text.len());
    let (mut in_string, mut escaped) = (false, false);
    let mut rest = text.as_ref();
    while let Some(c) = rest.chars().next() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
        } else if c == '"' {
            in_string = true;
        } else if let Some(token) = ["-Infinity", "Infinity", "NaN"].iter().find(|t| rest.starts_with(**t)) {
            out.push_str("null");
            rest = &rest[token.len()..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    serde_json::from_str(&out)
}

pub fn summarize(metrics: &Value) -> Value {
    let mut out = Map::new();
    for (key, name) in KEYS {
        if let Some(v) = metrics.get(*key).filter(|v| v.is_number()) {
            out.insert((*name).into(), v.clone());
        }
    }
    let num = |name: &str| out.get(name).and_then(Value::as_f64);
    let zero = |name: &str| num(name).map(|v| v == 0.0);
    let non_negative = |name: &str| num(name).map(|v| v >= 0.0);

    // Every check must be present and pass; a missing metric is not a pass.
    let checks = [
        ("drc", zero("drc_magic").zip(zero("drc_klayout")).map(|(a, b)| a && b)),
        ("lvs", zero("lvs_errors")),
        ("antenna", zero("antenna_nets")),
        ("setup", non_negative("setup_wns_ns")),
        ("hold", non_negative("hold_wns_ns")),
    ];
    let mut check_map = Map::new();
    for (name, result) in checks {
        check_map.insert(name.into(), result.map(Value::Bool).unwrap_or(Value::Null));
    }
    let signoff_clean = checks.iter().all(|(_, r)| *r == Some(true));
    out.insert("checks".into(), Value::Object(check_map));
    out.insert("signoff_clean".into(), json!(signoff_clean));
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean() -> Value {
        json!({
            "design__die__area": 2950.05, "design__instance__count__stdcell": 56,
            "timing__setup__ws": 13.7, "timing__hold__ws": 0.18,
            "magic__drc_error__count": 0, "klayout__drc_error__count": 0,
            "design__lvs_error__count": 0, "antenna__violating__nets": 0,
            "power__total": 9.5e-05, "unrelated": "x"
        })
    }

    #[test]
    fn parses_non_finite_numbers() {
        let raw = br#"{"a": Infinity, "b": -Infinity, "c": NaN, "d": "NaN Infinity \" x", "e": [1.5, NaN]}"#;
        let v = parse_metrics(raw).unwrap();
        assert_eq!(v, json!({"a": null, "b": null, "c": null, "d": "NaN Infinity \" x", "e": [1.5, null]}));
        assert!(parse_metrics(b"{nope").is_err());
    }

    #[test]
    fn clean_design_signs_off() {
        let s = summarize(&clean());
        assert_eq!(s["signoff_clean"], true);
        assert_eq!(s["stdcells"], 56);
        assert_eq!(s["checks"]["drc"], true);
        assert!(s.get("unrelated").is_none());
    }

    #[test]
    fn violations_and_missing_metrics_fail() {
        let mut m = clean();
        m["timing__hold__ws"] = json!(-0.05);
        let s = summarize(&m);
        assert_eq!(s["checks"]["hold"], false);
        assert_eq!(s["signoff_clean"], false);

        let mut m = clean();
        m.as_object_mut().unwrap().remove("design__lvs_error__count");
        let s = summarize(&m);
        assert_eq!(s["checks"]["lvs"], Value::Null);
        assert_eq!(s["signoff_clean"], false);
    }
}
