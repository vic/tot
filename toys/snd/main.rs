// throwaway audio proto: offline synth cost, determinism, sim-hash oracle, earblast gate. std only.
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::time::Instant;

const SR: f32 = 48_000.0;
const BLOCK: usize = 256; // 5.33 ms @48k

// ---- typed patch: gain only buildable through parse (cap) ----
#[derive(Clone, Copy, Debug)]
struct Gain(f32); // linear, invariant: 0 < g <= 0.5 (-6 dBFS per voice)
impl Gain {
    fn parse(db: f32) -> Result<Gain, String> {
        if !(db <= -6.0 && db >= -60.0) { return Err(format!("gain {db} dB outside -60..=-6")); }
        Ok(Gain(10f32.powf(db / 20.0)))
    }
}
#[derive(Clone, Copy, Debug)]
enum Wave { Saw, Square, Noise }
#[derive(Clone, Copy, Debug)]
struct Patch { wave: Wave, hz: f32, a: f32, d: f32, gain: Gain, cutoff: f32, drive: f32 }

#[derive(Clone, Copy)]
struct Voice { p: Patch, ph: f32, t: f32, lp: f32, rng: u32, pan: f32 }
impl Voice {
    fn new(p: Patch, pan: f32, seed: u32) -> Voice { Voice { p, ph: 0.0, t: 0.0, lp: 0.0, rng: seed | 1, pan } }
    fn alive(&self) -> bool { self.t < self.p.a + self.p.d }
    #[inline]
    fn next(&mut self) -> f32 {
        let p = self.p;
        self.ph += p.hz / SR; if self.ph >= 1.0 { self.ph -= 1.0; }
        let osc = match p.wave {
            Wave::Saw => 2.0 * self.ph - 1.0,
            Wave::Square => if self.ph < 0.5 { 1.0 } else { -1.0 },
            Wave::Noise => { self.rng ^= self.rng << 13; self.rng ^= self.rng >> 17; self.rng ^= self.rng << 5; (self.rng as f32 / u32::MAX as f32) * 2.0 - 1.0 }
        };
        let k = (p.cutoff / SR * 6.28).min(1.0);
        self.lp += k * (osc - self.lp);
        let env = if self.t < p.a { self.t / p.a } else { 1.0 - (self.t - p.a) / p.d };
        self.t += 1.0 / SR;
        (self.lp * p.drive).tanh() * env.max(0.0) * p.gain.0 * p.drive.max(1.0)
    }
}

// mix bus: soft limiter (tanh) => output can never exceed 1.0 full scale
fn render_block(vs: &mut Vec<Voice>, out: &mut [f32; BLOCK * 2], master: f32) {
    out.iter_mut().for_each(|s| *s = 0.0);
    for v in vs.iter_mut() {
        let (l, r) = ((1.0 - v.pan) * 0.5, (1.0 + v.pan) * 0.5);
        for i in 0..BLOCK { let s = v.next(); out[2 * i] += s * l; out[2 * i + 1] += s * r; }
    }
    vs.retain(|v| v.alive());
    for s in out.iter_mut() { *s = (*s * master).tanh() * 0.891; } // ceiling -1 dBFS
}

fn fnv(h: &mut u64, b: &[u8]) { for x in b { *h ^= *x as u64; *h = h.wrapping_mul(0x100000001b3); } }

// ---- validator: offline render one voice, measure peak + rms; reject earblast ----
fn validate(p: Patch) -> Result<(f32, f32), String> {
    let mut v = Voice::new(p, 0.0, 7);
    let (mut pk, mut e, mut n) = (0f32, 0f64, 0u64);
    while v.alive() { let s = v.next(); pk = pk.max(s.abs()); e += (s as f64) * (s as f64); n += 1; }
    let pk_db = 20.0 * pk.max(1e-9).log10();
    let rms_db = 10.0 * ((e / n.max(1) as f64).max(1e-18)).log10() as f32;
    if p.a + p.d > 2.0 { return Err(format!("length {:.2}s > 2s", p.a + p.d)); }
    if pk_db > -6.0 { return Err(format!("peak {pk_db:.1} dBFS > -6")); }
    if p.a < 0.002 && pk_db > -12.0 { return Err(format!("click: attack {:.4}s with peak {pk_db:.1} dBFS", p.a)); }
    Ok((pk_db, rms_db))
}

// ---- toy integer sim emitting typed events; audio must never feed back ----
#[derive(Clone, Copy)]
enum Ev { Kill { kind: u8, x: i32 }, Hit { x: i32 } }
struct Sim { pos: Vec<i32>, hp: Vec<i32>, rng: u64 }
impl Sim {
    fn new(n: usize) -> Sim { Sim { pos: (0..n as i32).map(|i| i * 37 % 10_000).collect(), hp: vec![3; n], rng: 0x9e3779b97f4a7c15 } }
    fn tick(&mut self, out: &mut Vec<Ev>, leak: i32) {
        for i in 0..self.pos.len() {
            self.rng ^= self.rng << 7; self.rng ^= self.rng >> 9;
            let bump = leak; // LEAK path: nonzero only in negative control
            self.pos[i] = (self.pos[i] + (self.rng % 5) as i32 - 2 + bump).rem_euclid(10_000);
            if self.rng % 97 == 0 { self.hp[i] -= 1; out.push(Ev::Hit { x: self.pos[i] });
                if self.hp[i] <= 0 { self.hp[i] = 3; out.push(Ev::Kill { kind: (i % 5) as u8, x: self.pos[i] }); } }
        }
    }
    fn hash(&self) -> u64 { let mut h = 0xcbf29ce484222325; for (p, q) in self.pos.iter().zip(&self.hp) { fnv(&mut h, &p.to_le_bytes()); fnv(&mut h, &q.to_le_bytes()); } h }
}

// coalesce: per tick, per kind max 1 cue; cap 8 cues/tick; queue full => drop (lossy, never block sim)
fn coalesce(evs: &[Ev], tx: &SyncSender<(u8, i32, u16)>, dropped: &mut u64) {
    let mut seen = [0u16; 6];
    let mut xs = [0i32; 6];
    for e in evs { let (k, x) = match *e { Ev::Kill { kind, x } => (kind as usize, x), Ev::Hit { x } => (5, x) }; seen[k] += 1; xs[k] = x; }
    for k in 0..6 { if seen[k] > 0 { match tx.try_send((k as u8, xs[k], seen[k])) { Ok(()) => {}, Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => *dropped += 1 } } }
}

fn patch_for(k: u8, count: u16) -> Patch {
    let base = [110.0, 165.0, 220.0, 330.0, 440.0, 880.0][k as usize];
    let g = Gain::parse(-18.0 + (count as f32).log2().min(6.0)).unwrap_or(Gain(0.1)); // louder w/ count, capped
    Patch { wave: if k == 5 { Wave::Noise } else { Wave::Saw }, hz: base, a: 0.005, d: 0.25, gain: g, cutoff: 2000.0, drive: 1.0 }
}

fn run_game(ticks: u32, n: usize, audio: bool, leak: bool) -> (u64, u64, u64, u64, f64) {
    let mut sim = Sim::new(n);
    let (tx, rx): (SyncSender<(u8, i32, u16)>, Receiver<(u8, i32, u16)>) = sync_channel(64);
    let mut evs = Vec::with_capacity(4096);
    let (mut dropped, mut cues) = (0u64, 0u64);
    let mut vs: Vec<Voice> = Vec::with_capacity(64);
    let mut out = [0f32; BLOCK * 2];
    let mut ah = 0xcbf29ce484222325u64;
    let mut worst = 0f64;
    for _ in 0..ticks {
        evs.clear(); sim.tick(&mut evs, if leak { (vs.len() % 3) as i32 } else { 0 });
        if audio {
            coalesce(&evs, &tx, &mut dropped);
            // audio side: 48000/30 = 1600 samples/tick = 6.25 blocks; render 6 blocks/tick (approx)
            while let Ok((k, x, c)) = rx.try_recv() {
                if vs.len() < 48 { vs.push(Voice::new(patch_for(k, c), (x as f32 / 5000.0) - 1.0, x as u32)); cues += 1; }
            }
            for _ in 0..6 { let t = Instant::now(); render_block(&mut vs, &mut out, 1.0);
                worst = worst.max(t.elapsed().as_secs_f64() * 1e3);
                for s in out.iter() { fnv(&mut ah, &s.to_bits().to_le_bytes()); } }
        }
    }
    (sim.hash(), ah, cues, dropped, worst)
}

fn main() {
    // 1. CPU per N voices (all voices alive for whole measure; saw + LP + tanh)
    for &n in &[32usize, 64, 128, 256, 512] {
        let p = Patch { wave: Wave::Saw, hz: 220.0, a: 0.01, d: 1000.0, gain: Gain(0.05), cutoff: 3000.0, drive: 1.0 };
        let mut vs: Vec<Voice> = (0..n).map(|i| Voice::new(p, (i as f32 / n as f32) * 2.0 - 1.0, i as u32)).collect();
        let mut out = [0f32; BLOCK * 2];
        let blocks = 1875; // 10 s audio
        let mut worst = 0f64; let mut over = 0u32;
        let budget_ms = BLOCK as f64 / SR as f64 * 1e3;
        let t0 = Instant::now();
        for _ in 0..blocks { let t = Instant::now(); render_block(&mut vs, &mut out, 1.0); let ms = t.elapsed().as_secs_f64() * 1e3; worst = worst.max(ms); if ms > budget_ms { over += 1; } }
        let tot = t0.elapsed().as_secs_f64();
        println!("voices={n:4} cpu_of_1core={:.2}% worst_block={:.3}ms budget={budget_ms:.2}ms deadline_miss={over}/{blocks}", tot / 10.0 * 100.0, worst);
    }
    // 2. render determinism
    let a = run_game(300, 20_000, true, false); let b = run_game(300, 20_000, true, false);
    println!("audio_hash run1={:016x} run2={:016x} equal={}", a.1, b.1, a.1 == b.1);
    // 3. sim hash oracle: audio on vs off
    let off = run_game(300, 20_000, false, false);
    println!("sim_hash audio_on={:016x} audio_off={:016x} equal={} cues={} dropped={} worst_block_ms={:.3}", a.0, off.0, a.0 == off.0, a.2, a.3, a.4);
    // 3n. negative control: sim reads live voice count (audio state) => must differ
    let lk = run_game(300, 20_000, true, true); let lk_off = run_game(300, 20_000, false, true);
    println!("NEG leak sim_hash on={:016x} off={:016x} equal={} (must be false)", lk.0, lk_off.0, lk.0 == lk_off.0);
    // 4. earblast gate
    let ok = Patch { wave: Wave::Saw, hz: 440.0, a: 0.005, d: 0.3, gain: Gain::parse(-18.0).unwrap_or(Gain(0.0)), cutoff: 3000.0, drive: 1.0 };
    println!("valid patch => {:?}", validate(ok));
    println!("NEG gain 0 dB parse => {:?}", Gain::parse(0.0).map(|_| ()));
    let blast = Patch { gain: Gain::parse(-6.0).unwrap_or(Gain(0.0)), drive: 8.0, a: 0.0005, ..ok }; // drive bypasses gain cap
    println!("NEG drive x8 patch => {:?}", validate(blast));
    let long = Patch { d: 5.0, ..ok };
    println!("NEG 5 s patch => {:?}", validate(long));
    // 5. mix ceiling: 48 voices at max gain, full scale check
    let mx = Patch { gain: Gain::parse(-6.0).unwrap_or(Gain(0.0)), d: 1.0, ..ok };
    let mut vs: Vec<Voice> = (0..48).map(|i| Voice::new(mx, 0.0, i)).collect(); let mut out = [0f32; BLOCK * 2]; let mut pk = 0f32;
    for _ in 0..100 { render_block(&mut vs, &mut out, 1.0); for s in out.iter() { pk = pk.max(s.abs()); } }
    println!("mix 48 voices @-6 dB each: bus peak={:.2} dBFS (ceiling -1)", 20.0 * pk.log10());
}
