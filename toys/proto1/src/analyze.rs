//! Reads proto1 jsonl, medians over runs, scene x bar PASS/FAIL, flags invalid meters (N1-N3).
use crate::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write;

#[derive(Clone, Copy, Debug)]
enum Bar {
    FpsMin(f64),
    BytesP50Max(f64),
    BytesP99Max(f64),
    AckP99Max(f64),
    Ok,
    /// negative control N3: glow never placed must report ok=false (armed only if S5 ok)
    NotOk,
    /// negative control N1: fps <= frac x fps of the named scene
    FpsAtMost(&'static str, f64),
}

/// Bars from vic/notes/proto1-plan.md (estimates vic may change).
fn bars(scene: &str) -> &'static [Bar] {
    match scene {
        "S1" => &[Bar::BytesP50Max(65536.0), Bar::BytesP99Max(163840.0), Bar::FpsMin(60.0)],
        "S3" | "S4" => &[Bar::FpsMin(30.0)],
        "S5" | "S6" | "S7" => &[Bar::Ok, Bar::FpsMin(30.0)],
        "S8" | "K1" => &[Bar::Ok],
        "L1" => &[Bar::AckP99Max(16.7)],
        "N1" => &[Bar::FpsAtMost("S3", 0.2)],
        "N2" => &[Bar::FpsMin(1000.0)],
        "N3" => &[Bar::NotOk],
        _ => &[],
    }
}

#[derive(Default)]
struct Row {
    fps: Vec<f64>,
    b50: Vec<f64>,
    b99: Vec<f64>,
    a50: Vec<f64>,
    a99: Vec<f64>,
    ok: Vec<bool>,
}

fn median(v: &[f64]) -> Option<f64> {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(f64::total_cmp);
    s.get(s.len().wrapping_sub(1) / 2).copied().filter(|_| !s.is_empty())
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k).and_then(Value::as_f64)
}

type Key = (String, String, String);

fn verdict(bar: Bar, r: &Row, peer: impl Fn(&str) -> Option<(f64, bool)>) -> String {
    let all_ok = !r.ok.is_empty() && r.ok.iter().all(|&o| o);
    let cmp = |v: Option<f64>, pass: &dyn Fn(f64) -> bool, what: String| match v {
        Some(x) if pass(x) => format!("PASS {what}"),
        Some(_) => format!("FAIL {what}"),
        None => format!("NOT MEASURED {what}"),
    };
    match bar {
        Bar::FpsMin(m) => cmp(median(&r.fps), &|x| x >= m, format!("fps>={m}")),
        Bar::BytesP50Max(m) => cmp(median(&r.b50), &|x| x <= m, format!("p50<={}KB", m / 1024.0)),
        Bar::BytesP99Max(m) => cmp(median(&r.b99), &|x| x <= m, format!("p99<={}KB", m / 1024.0)),
        Bar::AckP99Max(m) => cmp(median(&r.a99), &|x| x <= m, format!("ack_p99<={m}ms")),
        Bar::Ok => (if all_ok { "PASS ok" } else { "FAIL ok" }).to_string(),
        Bar::NotOk => match peer("S5") {
            Some((_, true)) if !all_ok => "PASS ok=false (N3 armed)".into(),
            Some((_, true)) => "METER INVALID: N3 ok=true".into(),
            _ => "UNARMED: S5 not ok".into(),
        },
        Bar::FpsAtMost(s, k) => match (peer(s), median(&r.fps)) {
            (Some((p, _)), Some(x)) if p > 0.0 && x <= p * k => format!("PASS fps<={k}x{s}"),
            (Some((p, _)), Some(_)) if p > 0.0 => format!("METER INVALID: fps>{k}x{s}"),
            _ => format!("UNARMED: no {s} fps"),
        },
    }
}

pub fn report(files: &[String]) -> Result<String> {
    let mut rows: BTreeMap<Key, Row> = BTreeMap::new();
    let mut out = String::new();
    for path in files {
        for l in std::fs::read_to_string(path)?.lines().filter(|l| !l.trim().is_empty()) {
            let v: Value = serde_json::from_str(l)?;
            let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("?").to_string();
            if s("scene") == "PROBE" {
                let _ = writeln!(out, "PROBE {} {} run {}: sync_2026={} gfx={} anim={}", s("term"), s("rev"), v["run"], v["sync_2026"], v["gfx"], v["anim_frame_reply"]);
                continue;
            }
            let r = rows.entry((s("term"), s("rev"), s("scene"))).or_default();
            let push = |dst: &mut Vec<f64>, k| dst.extend(f(&v, k));
            push(&mut r.fps, "fps_acked");
            push(&mut r.b50, "bytes_p50");
            push(&mut r.b99, "bytes_p99");
            push(&mut r.a50, "ack_ms_p50");
            push(&mut r.a99, "ack_ms_p99");
            r.ok.push(v.get("ok").and_then(Value::as_bool).unwrap_or(false));
        }
    }
    let _ = writeln!(out, "{:<10} {:<8} {:<5} {:>4} {:>9} {:>9} {:>9} {:>8} {:>8} {:>5}  verdict (median of runs, [M-parse])",
        "term", "rev", "scene", "runs", "fps", "B_p50", "B_p99", "ack_p50", "ack_p99", "ok");
    let mut invalid = 0;
    for ((term, rev, scene), r) in &rows {
        let peer = |s: &str| {
            rows.get(&(term.clone(), rev.clone(), s.to_string()))
                .map(|p| (median(&p.fps).unwrap_or(0.0), !p.ok.is_empty() && p.ok.iter().all(|&o| o)))
        };
        let vs: Vec<String> = if term == "null" {
            vec!["INFO (no terminal)".into()]
        } else {
            let neg = scene.starts_with('N');
            bars(scene)
                .iter()
                .map(|&b| verdict(b, r, peer))
                .map(|v| if neg && v.starts_with("FAIL") { format!("METER INVALID: {v}") } else { v })
                .collect()
        };
        invalid += vs.iter().filter(|v| v.starts_with("METER INVALID")).count();
        let n = |v: &[f64], d: usize| median(v).map_or("-".into(), |x| format!("{x:.d$}"));
        let _ = writeln!(out, "{:<10} {:<8} {:<5} {:>4} {:>9} {:>9} {:>9} {:>8} {:>8} {:>5}  {}",
            term, rev, scene, r.ok.len(), n(&r.fps, 1), n(&r.b50, 0), n(&r.b99, 0), n(&r.a50, 2), n(&r.a99, 2),
            r.ok.iter().all(|&o| o), if vs.is_empty() { "INFO".into() } else { vs.join("; ") });
    }
    let _ = writeln!(out, "meter: {}", if invalid == 0 { "no negative control failed" } else { "INVALID (a negative control failed)" });
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(lines: &[&str]) -> String {
        let p = std::env::temp_dir().join(format!("proto1-an-{}-{}.jsonl", std::process::id(), lines.len()));
        std::fs::write(&p, lines.join("\n")).unwrap();
        p.to_string_lossy().into_owned()
    }

    const S3: &str = r#"{"term":"t","rev":"1","scene":"S3","fps_acked":100.0,"ok":true}"#;
    const S5: &str = r#"{"term":"t","rev":"1","scene":"S5","fps_acked":50.0,"ok":true}"#;

    #[test]
    fn median_odd_even() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), Some(2.0));
        assert_eq!(median(&[4.0, 1.0]), Some(1.0));
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn valid_negative_controls_pass() {
        let n1 = r#"{"term":"t","rev":"1","scene":"N1","fps_acked":15.0,"ok":true}"#;
        let n3 = r#"{"term":"t","rev":"1","scene":"N3","fps_acked":50.0,"ok":false}"#;
        let r = report(&[write(&[S3, S5, n1, n3])]).unwrap();
        assert!(r.contains("PASS fps<=0.2xS3") && r.contains("N3 armed") && r.contains("no negative control failed"), "{r}");
    }

    #[test]
    fn broken_meter_is_flagged() {
        // N1 not slower than S3, N3 reports ok: both must flag the meter
        let n1 = r#"{"term":"t","rev":"1","scene":"N1","fps_acked":90.0,"ok":true}"#;
        let n3 = r#"{"term":"t","rev":"1","scene":"N3","fps_acked":50.0,"ok":true}"#;
        let n2 = r#"{"term":"t","rev":"1","scene":"N2","fps_acked":200.0,"ok":true}"#;
        let r = report(&[write(&[S3, S5, n1, n2, n3, ""])]).unwrap();
        assert_eq!(r.matches("METER INVALID").count(), 3, "{r}");
        assert!(r.contains("METER INVALID: FAIL fps>=1000"), "{r}");
        assert!(r.contains("INVALID (a negative control failed)"));
    }
}
