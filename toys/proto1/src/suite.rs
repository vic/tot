//! Scene runner: build frame, one write, DSR ack, per-scene JSON line. Numbers = [M-parse].
use crate::grid::{Dims, emit_diff};
use crate::kitty::{self, Action, Image, Medium, Placement, ZIndex};
use crate::scene::{self, Gfx, Plan, Scene, Sink, World};
use crate::tty::{Event, Tty};
use crate::{Error, Result};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct Opts {
    pub dims: Dims,
    pub secs: Duration,
    pub seed: u64,
    pub run: u32,
    pub null: bool,
    pub term: String,
    pub rev: String,
    pub out: Option<PathBuf>,
}

const ACK_TIMEOUT: Duration = Duration::from_secs(2);
const GLOW_ID: u32 = 31;
const BOSS_ID: u32 = 200;
const BSU: &[u8] = b"\x1b[?2026h";
const ESU: &[u8] = b"\x1b[?2026l";
const DSR: &[u8] = b"\x1b[5n";

/// Nearest-rank percentile of a sorted slice.
pub fn pct(sorted: &[u64], p: u64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (p * sorted.len() as u64).div_ceil(100).max(1) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

#[derive(Default)]
struct Stats {
    bytes: Vec<u64>,
    ack_us: Vec<u64>,
    changed: Vec<u64>,
    max_writes: u32,
    timeouts: u32,
    gfx_ok: u32,
    gfx_err: u32,
    gfx_placed: u32,
    first_gfx: Option<String>,
}

impl Stats {
    fn gfx(&mut self, evs: &[Event], want_ids: std::ops::Range<u32>) {
        for e in evs {
            if let Event::Gfx { id, placement, ok, raw } = e {
                if self.first_gfx.is_none() {
                    self.first_gfx = Some(raw.clone());
                }
                match (*ok, id.is_some_and(|i| want_ids.contains(&i)), placement.is_some()) {
                    (true, true, true) => {
                        self.gfx_ok += 1;
                        self.gfx_placed += 1;
                    }
                    (true, _, _) => self.gfx_ok += 1,
                    (false, _, _) => self.gfx_err += 1,
                }
            }
        }
    }
}

/// Medium probe: 1x1 RGBA `a=q`; supported = OK reply for our id before the DSR.
fn probe_medium(tty: &mut Tty, m: Medium) -> Result<(bool, String)> {
    let px = [0u8, 0, 0, 255];
    let mut buf = Vec::new();
    kitty::image(&mut buf, &Image { id: 77, w: 1, h: 1, rgba: &px }, m, Action::Query, None, u64::MAX)?;
    buf.extend_from_slice(DSR);
    tty.send(&buf)?;
    let evs = tty.wait_dsr(ACK_TIMEOUT).unwrap_or_default();
    let reply = evs.iter().find_map(|e| match e {
        Event::Gfx { id: Some(77), ok, raw, .. } => Some((*ok, raw.clone())),
        _ => None,
    });
    cleanup(m, u64::MAX);
    Ok(reply.unwrap_or((false, "no reply".into())))
}

fn cleanup(m: Medium, tag: u64) {
    if let Some(p) = kitty::medium_path(m, tag) {
        // terminal normally unlinks it; NotFound is the expected case
        let _ = std::fs::remove_file(p);
    }
}

/// Animation probe: upload a 1x1 image, then add one frame (`a=f`). Returns the frame reply.
fn probe_anim(tty: &mut Tty) -> Result<String> {
    let px = [255u8, 0, 0, 255];
    let mut buf = Vec::new();
    kitty::image(&mut buf, &Image { id: 78, w: 1, h: 1, rgba: &px }, Medium::Direct, Action::TransmitOnly, None, 0)?;
    buf.extend_from_slice(b"\x1b_Ga=f,i=78,s=1,v=1,f=32,t=d;AAD//w==\x1b\\");
    buf.extend_from_slice(DSR);
    tty.send(&buf)?;
    let evs = tty.wait_dsr(ACK_TIMEOUT).unwrap_or_default();
    let replies: Vec<String> = evs.iter().filter_map(|e| if let Event::Gfx { raw, .. } = e { Some(raw.clone()) } else { None }).collect();
    Ok(replies.join(" | "))
}

fn probe_sync(tty: &mut Tty) -> Result<Option<u8>> {
    let mut b = b"\x1b[?2026$p".to_vec();
    b.extend_from_slice(DSR);
    tty.send(&b)?;
    let evs = tty.wait_dsr(ACK_TIMEOUT).unwrap_or_default();
    Ok(evs.iter().find_map(|e| if let Event::SyncMode(v) = e { Some(*v) } else { None }))
}

struct Frame {
    world: World,
    front: Vec<Option<crate::grid::Cell>>,
    rgba: Vec<u8>,
    buf: Vec<u8>,
}

impl Frame {
    fn new(o: &Opts) -> Self {
        Frame { world: World::new(o.dims, o.seed), front: vec![None; o.dims.cells()], rgba: Vec::new(), buf: Vec::with_capacity(1 << 21) }
    }

    /// Build frame `n` of `plan` into `buf`. Returns changed cells.
    fn build(&mut self, plan: Plan, dims: Dims, z: ZIndex) -> Result<usize> {
        let b = &mut self.buf;
        b.clear();
        if plan.sync {
            b.extend_from_slice(BSU);
        }
        self.world.step(plan.text);
        let start = b.len();
        let changed = emit_diff(&mut self.front, &self.world.grid, b);
        let end = b.len();
        for _ in 1..plan.repeat {
            b.extend_from_within(start..end);
        }
        let f = self.world.frame;
        let full = Placement { pid: 1, cols: dims.cols(), rows: dims.rows(), z: Some(z) };
        match plan.gfx {
            Gfx::None => {}
            Gfx::Glow { medium, placed } => {
                scene::glow_rgba(f, &mut self.rgba);
                b.extend_from_slice(b"\x1b[1;1H");
                let img = Image { id: GLOW_ID, w: scene::GLOW_W, h: scene::GLOW_H, rgba: &self.rgba };
                let (act, p) = if placed { (Action::TransmitPlace, Some(&full)) } else { (Action::TransmitOnly, None) };
                kitty::image(b, &img, medium, act, p, f)?;
            }
            Gfx::Boss => {
                let k = (f % u64::from(scene::BOSS_FRAMES)) as u32;
                let prev = (k + scene::BOSS_FRAMES - 1) % scene::BOSS_FRAMES;
                b.extend_from_slice(b"\x1b[20;80H");
                kitty::unplace(b, BOSS_ID + prev, 1);
                kitty::place(b, BOSS_ID + k, &Placement { pid: 1, cols: 32, rows: 8, z: None });
            }
        }
        if plan.sync {
            b.extend_from_slice(ESU);
        }
        Ok(changed)
    }
}

fn line(o: &Opts, scene: &str, frames: usize, secs: f64, st: &mut Stats, ok: bool, extra: Value) -> Value {
    st.bytes.sort_unstable();
    st.ack_us.sort_unstable();
    st.changed.sort_unstable();
    let ms = |us: u64| us as f64 / 1000.0;
    let mut v = json!({
        "term": o.term, "rev": o.rev, "run": o.run, "scene": scene, "cols": o.dims.cols(), "rows": o.dims.rows(),
        "seed": o.seed, "secs": secs, "frames": frames,
        "bytes_total": st.bytes.iter().sum::<u64>(), "bytes_p50": pct(&st.bytes, 50), "bytes_p99": pct(&st.bytes, 99),
        "changed_p50": pct(&st.changed, 50),
        "ack_ms_p50": (!st.ack_us.is_empty()).then(|| ms(pct(&st.ack_us, 50))),
        "ack_ms_p99": (!st.ack_us.is_empty()).then(|| ms(pct(&st.ack_us, 99))),
        "fps_acked": if secs > 0.0 { frames as f64 / secs } else { 0.0 },
        "writes_per_frame_max": st.max_writes, "timeouts": st.timeouts,
        "gfx_ok": st.gfx_ok, "gfx_err": st.gfx_err, "gfx_placed": st.gfx_placed, "first_gfx_reply": st.first_gfx,
        "ok": ok, "meter": "M-parse",
    });
    if let (Some(m), Value::Object(e)) = (v.as_object_mut(), extra) {
        m.extend(e);
    }
    v
}

/// S0 / --null: same frame bytes to /dev/null, no ack (emission cost without a terminal).
fn run_null(scene: Scene, o: &Opts) -> Result<Value> {
    let mut null = std::fs::OpenOptions::new().write(true).open("/dev/null")?;
    let mut fr = Frame::new(o);
    let mut st = Stats::default();
    let plan = Plan { sink: Sink::Null, ..scene.plan() };
    let plan = if scene == Scene::S0 { Scene::S1.plan() } else { plan };
    let z = ZIndex::new(-1)?;
    let t0 = Instant::now();
    while t0.elapsed() < o.secs {
        let changed = fr.build(plan, o.dims, z)?;
        null.write_all(&fr.buf)?;
        cleanup_all(plan, fr.world.frame, fr.world.frame);
        st.bytes.push(fr.buf.len() as u64);
        st.changed.push(changed as u64);
        st.max_writes = 1;
    }
    let secs = t0.elapsed().as_secs_f64();
    Ok(line(o, scene.name(), st.bytes.len(), secs, &mut st, true, json!({"sink": "null"})))
}

fn cleanup_all(plan: Plan, from: u64, to: u64) {
    if let Gfx::Glow { medium, .. } = plan.gfx {
        for f in from..=to {
            cleanup(medium, f);
        }
    }
}

fn run_tty(scene: Scene, o: &Opts, tty: &mut Tty, probes: &[(Medium, bool, String)]) -> Result<Value> {
    let plan = scene.plan();
    let mut b = b"\x1b[0m\x1b[2J\x1b[H".to_vec();
    b.extend_from_slice(kitty::DELETE_ALL);
    tty.send(&b)?;
    tty.drain(Duration::from_millis(200));
    let mut st = Stats::default();
    let supported = match plan.gfx {
        Gfx::Glow { medium, .. } => probes.iter().find(|p| p.0 == medium).is_some_and(|p| p.1),
        Gfx::Boss => probes.iter().find(|p| p.0 == Medium::Direct).is_some_and(|p| p.1),
        Gfx::None => true,
    };
    let mut boss_ok = true;
    if plan.gfx == Gfx::Boss && supported {
        // upload all frames once (direct), ack-checked
        let mut up = Vec::new();
        for k in 0..scene::BOSS_FRAMES {
            let px = scene::boss_rgba(k);
            kitty::image(&mut up, &Image { id: BOSS_ID + k, w: scene::BOSS_W, h: scene::BOSS_H, rgba: &px }, Medium::Direct, Action::TransmitOnly, None, 0)?;
        }
        up.extend_from_slice(DSR);
        tty.send(&up)?;
        let evs = tty.wait_dsr(ACK_TIMEOUT).unwrap_or_default();
        let oks = evs.iter().filter(|e| matches!(e, Event::Gfx { ok: true, .. })).count();
        boss_ok = oks == scene::BOSS_FRAMES as usize;
    }
    let mut fr = Frame::new(o);
    let z = ZIndex::new(-1)?;
    let t0 = Instant::now();
    let mut frames = 0usize;
    while t0.elapsed() < o.secs && supported {
        let changed = fr.build(plan, o.dims, z)?;
        st.bytes.push(fr.buf.len() as u64);
        st.changed.push(changed as u64);
        fr.buf.extend_from_slice(DSR);
        let ts = Instant::now();
        st.max_writes = st.max_writes.max(tty.send(&fr.buf)?);
        let Some(evs) = tty.wait_dsr(ACK_TIMEOUT) else {
            st.timeouts += 1;
            break;
        };
        st.ack_us.push(ts.elapsed().as_micros() as u64);
        let ids = match plan.gfx {
            Gfx::Boss => BOSS_ID..BOSS_ID + scene::BOSS_FRAMES,
            _ => GLOW_ID..GLOW_ID + 1,
        };
        st.gfx(&evs, ids);
        frames += 1;
    }
    let secs = t0.elapsed().as_secs_f64();
    cleanup_all(plan, 0, fr.world.frame);
    let clean = st.timeouts == 0 && st.gfx_err == 0 && frames > 0;
    let ok = match plan.gfx {
        Gfx::None => clean,
        Gfx::Glow { .. } => clean && supported && st.gfx_placed as usize == frames,
        Gfx::Boss => clean && supported && boss_ok,
    };
    Ok(line(o, scene.name(), frames, secs, &mut st, ok, json!({"sink": "tty", "supported": supported, "boss_upload_ok": boss_ok})))
}

pub fn run(scenes: &[Scene], o: &Opts) -> Result<()> {
    let mut lines = Vec::new();
    let mut tty = None;
    let mut probes = Vec::new();
    for &s in scenes {
        let v = if o.null || s.plan().sink == Sink::Null {
            run_null(s, o)?
        } else {
            let t = match tty.as_mut() {
                Some(t) => t,
                None => {
                    let mut t = Tty::open()?;
                    let have = t.size()?;
                    if have.cols() < o.dims.cols() || have.rows() < o.dims.rows() {
                        return Err(Error::TooSmall { need: (o.dims.cols(), o.dims.rows()), have: (have.cols(), have.rows()) });
                    }
                    let sync = probe_sync(&mut t)?;
                    let mut media = serde_json::Map::new();
                    for m in Medium::ALL {
                        let (ok, raw) = probe_medium(&mut t, m)?;
                        media.insert(m.name().into(), json!({"ok": ok, "reply": raw}));
                        probes.push((m, ok, raw));
                    }
                    let anim = probe_anim(&mut t)?;
                    lines.push(json!({"term": o.term, "rev": o.rev, "run": o.run, "scene": "PROBE", "sync_2026": sync, "anim_frame_reply": anim,
                        "gfx": media, "tty_cols": have.cols(), "tty_rows": have.rows()}));
                    tty.insert(t)
                }
            };
            run_tty(s, o, t, &probes)?
        };
        lines.push(v);
    }
    drop(tty);
    let mut text = String::new();
    for l in &lines {
        text.push_str(&serde_json::to_string(l)?);
        text.push('\n');
    }
    match &o.out {
        Some(p) => std::fs::OpenOptions::new().create(true).append(true).open(p)?.write_all(text.as_bytes())?,
        None => std::io::stdout().write_all(text.as_bytes())?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts() -> Opts {
        Opts { dims: Dims::new(200, 60).unwrap(), secs: Duration::from_millis(100), seed: 1, run: 1, null: true, term: "null".into(), rev: "t".into(), out: None }
    }

    #[test]
    fn pct_nearest_rank() {
        let v: Vec<u64> = (1..=100).collect();
        assert_eq!((pct(&v, 50), pct(&v, 99), pct(&v, 100)), (50, 99, 100));
        assert_eq!(pct(&[7], 99), 7);
        assert_eq!(pct(&[], 50), 0);
    }

    #[test]
    fn null_s0_runs_headless() {
        let v = run_null(Scene::S0, &opts()).unwrap();
        assert!(v["frames"].as_u64().unwrap() > 10);
        assert!(v["bytes_p50"].as_u64().unwrap() > 1000);
    }

    #[test]
    fn n1_frame_is_ten_s3_frames() {
        let o = opts();
        let z = ZIndex::new(-1).unwrap();
        let (mut a, mut b) = (Frame::new(&o), Frame::new(&o));
        for _ in 0..3 {
            a.build(Scene::S3.plan(), o.dims, z).unwrap();
            b.build(Scene::N1.plan(), o.dims, z).unwrap();
        }
        assert_eq!(b.buf.len(), 10 * a.buf.len());
        assert_eq!(&b.buf[..a.buf.len()], &a.buf[..]);
    }

    #[test]
    fn n2_frame_is_empty_and_s4_is_synced() {
        let o = opts();
        let z = ZIndex::new(-1).unwrap();
        let mut f = Frame::new(&o);
        f.build(Scene::N2.plan(), o.dims, z).unwrap();
        f.build(Scene::N2.plan(), o.dims, z).unwrap();
        assert!(f.buf.is_empty());
        let mut s = Frame::new(&o);
        s.build(Scene::S4.plan(), o.dims, z).unwrap();
        assert!(s.buf.starts_with(BSU) && s.buf.ends_with(ESU));
    }

    #[test]
    fn glow_ok_needs_placement_reply() {
        // N3 logic: OK without p= is not a placement
        let mut st = Stats::default();
        st.gfx(&[Event::Gfx { id: Some(GLOW_ID), placement: None, ok: true, raw: String::new() }], GLOW_ID..GLOW_ID + 1);
        assert_eq!((st.gfx_ok, st.gfx_placed), (1, 0));
        st.gfx(&[Event::Gfx { id: Some(GLOW_ID), placement: Some(1), ok: true, raw: String::new() }], GLOW_ID..GLOW_ID + 1);
        assert_eq!(st.gfx_placed, 1);
    }
}
