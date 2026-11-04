// throwaway: generative music rules + musicality/variety oracle w/ negative controls. std only.
const STEPS: usize = 64; // 4 bars x 16
const SR: u64 = 48_000;

struct Rng(u64);
impl Rng {
    fn new(s: u64) -> Rng { Rng(s ^ 0x9e3779b97f4a7c15) }
    fn next(&mut self) -> u64 { self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15); let mut z = self.0; z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); z ^ (z >> 31) }
    fn below(&mut self, n: u64) -> u64 { self.next() % n } // toy: modulo bias ok here
}

// typed mood (toy subset)
#[derive(Clone, Copy)]
struct Mood { root: i32, mode: [i32; 7], bpm: u32, energy: u32 }
const PHRYGIAN: [i32; 7] = [0, 1, 3, 5, 7, 8, 10];

fn euclid(k: usize, n: usize, rot: usize) -> Vec<bool> { (0..n).map(|i| ((i + rot) * k) % n < k).collect() }

// GEN: Euclidean onsets + scale-degree walk (step-biased, post-leap reversal) + phrase grammar A A' B A''
type Seq = [i32; STEPS]; // midi pitch or -1 = rest
fn gen(seed: u64, m: Mood) -> Seq {
    let mut r = Rng::new(seed);
    let k = 5 + m.energy as usize + r.below(4) as usize;
    let on = euclid(k.min(13), 16, r.below(16) as usize);
    let on_b = euclid((k + 2).min(13), 16, r.below(16) as usize);
    let bar = |r: &mut Rng, start: i32, on: &[bool]| -> ([i32; 16], i32) {
        let (mut deg, mut out, mut last_leap) = (start, [-1; 16], 0i32);
        for i in 0..16 { if !on[i] { continue; }
            let roll = r.below(100);
            let mv = if last_leap != 0 { -last_leap.signum() } else if roll < 60 { if r.below(2) == 0 { 1 } else { -1 } } else if roll < 85 { if r.below(2) == 0 { 2 } else { -2 } } else if roll < 95 { 0 } else { if r.below(2) == 0 { 4 } else { -4 } };
            last_leap = if mv.abs() >= 3 { mv } else { 0 };
            let mv = if deg + mv > 6 || deg + mv < -1 { -mv } else { mv }; // regression to mean: reflect at range edge
            deg += mv;
            out[i] = m.root + 12 * deg.div_euclid(7) + m.mode[deg.rem_euclid(7) as usize];
        }
        (out, deg)
    };
    let st = [0, 2, 4][r.below(3) as usize];
    let (a, d) = bar(&mut r, st, &on);
    let mut a1 = a; let j = r.below(16) as usize; if a1[j] >= 0 { a1[j] = a[(j + 1) % 16].max(a1[j]); } // A' = 1-cell mutation (in-scale: copies a neighbour pitch)
    let (b, _) = bar(&mut r, d, &on_b);
    let mut a2 = a; let j = r.below(16) as usize; if a2[j] >= 0 { a2[j] = -1; } // A'' = drop 1 note
    let mut s = [-1; STEPS];
    for (bi, x) in [a, a1, b, a2].iter().enumerate() { s[bi * 16..bi * 16 + 16].copy_from_slice(x); }
    s
}
// NEG generators
fn gen_chromatic(seed: u64, m: Mood) -> Seq { let mut r = Rng::new(seed); let mut s = [-1; STEPS]; for i in 0..STEPS { if r.below(2) == 0 { s[i] = m.root + r.below(24) as i32; } } s }
fn gen_inscale_random(seed: u64, m: Mood) -> Seq { let mut r = Rng::new(seed); let mut s = [-1; STEPS]; for i in 0..STEPS { if r.below(2) == 0 { let d = r.below(14) as i32; s[i] = m.root + 12 * (d / 7) + m.mode[(d % 7) as usize]; } } s }

// METRICS
fn in_scale(s: &Seq, m: Mood) -> f64 { let n: Vec<_> = s.iter().filter(|p| **p >= 0).collect(); n.iter().filter(|p| m.mode.contains(&(**p - m.root).rem_euclid(12))).count() as f64 / n.len().max(1) as f64 }
fn intervals(s: &Seq) -> Vec<i32> { let n: Vec<i32> = s.iter().copied().filter(|p| *p >= 0).collect(); n.windows(2).map(|w| (w[1] - w[0]).abs()).collect() }
fn step_share(s: &Seq) -> f64 { let iv = intervals(s); iv.iter().filter(|d| **d <= 2).count() as f64 / iv.len().max(1) as f64 }
fn max_leap(s: &Seq) -> i32 { intervals(s).into_iter().max().unwrap_or(0) }
fn bar_repeat(s: &Seq) -> bool { // some pair of bars differs in <= 4 of 16 cells
    (0..4).any(|i| (i + 1..4).any(|j| (0..16).filter(|c| s[i * 16 + c] != s[j * 16 + c]).count() <= 4)) }
fn ham(a: &Seq, b: &Seq) -> f64 { a.iter().zip(b).filter(|(x, y)| x != y).count() as f64 / STEPS as f64 }

struct Bars { scale: f64, step: f64, leap: i32, repeat: f64 }
fn musical(g: &dyn Fn(u64, Mood) -> Seq, m: Mood, n: u64) -> (Bars, bool) {
    let (mut sc, mut st, mut lp, mut rp) = (1.0f64, 0.0f64, 0i32, 0u64);
    for seed in 0..n { let s = g(seed, m); sc = sc.min(in_scale(&s, m)); st += step_share(&s); lp = lp.max(max_leap(&s)); if bar_repeat(&s) { rp += 1; } }
    let b = Bars { scale: sc, step: st / n as f64, leap: lp, repeat: rp as f64 / n as f64 };
    let pass = b.scale >= 1.0 && b.step >= 0.5 && b.leap <= 12 && b.repeat >= 0.95;
    (b, pass)
}
fn variety(g: &dyn Fn(u64) -> Seq, n: usize) -> (f64, f64, bool) {
    let seqs: Vec<Seq> = (0..n as u64).map(g).collect();
    let mut nn: Vec<f64> = (0..n).map(|i| (0..n).filter(|j| *j != i).map(|j| ham(&seqs[i], &seqs[j])).fold(1.0, f64::min)).collect();
    nn.sort_by(|a, b| a.total_cmp(b));
    let (min, med) = (nn[0], nn[n / 2]);
    (min, med, min > 0.0 && med >= 0.30)
}
// tempo lock: event sample time must equal step * samples_per_step exactly
fn tempo_lock(m: Mood, jitter: bool) -> bool {
    let sps = SR * 60 / (m.bpm as u64 * 4); let mut r = Rng::new(1);
    (0..STEPS as u64).all(|i| { let t = i * sps + if jitter { r.below(64) } else { 0 }; t % sps == 0 })
}
// render lead + bass to buffer: peak + rms; limiter on/off
fn render(s: &Seq, m: Mood, master_db: f32, limiter: bool) -> (f32, f32) {
    let sps = (SR * 60 / (m.bpm as u64 * 4)) as usize; let g = 10f32.powf(master_db / 20.0);
    let (mut pk, mut e, mut n) = (0f32, 0f64, 0usize); let (mut ph, mut bph) = (0f32, 0f32);
    for (i, p) in s.iter().enumerate() { for k in 0..sps {
        let env = if *p >= 0 { 1.0 - k as f32 / sps as f32 } else { 0.0 };
        let hz = if *p >= 0 { 440.0 * 2f32.powf((*p - 69) as f32 / 12.0) } else { 0.0 };
        ph = (ph + hz / SR as f32).fract(); bph = (bph + 440.0 * 2f32.powf((m.root - 24 - 69) as f32 / 12.0) / SR as f32).fract();
        let bass_env = if i % 4 == 0 { 1.0 - k as f32 / sps as f32 } else { 0.3 };
        let x = ((2.0 * ph - 1.0) * env * 0.25 + (2.0 * bph - 1.0) * bass_env * 0.3) * g;
        let y = if limiter { x.tanh() * 0.891 } else { x };
        pk = pk.max(y.abs()); e += (y * y) as f64; n += 1; } }
    (20.0 * pk.max(1e-9).log10(), 10.0 * (e / n as f64).log10() as f32)
}
fn fnv(s: &Seq) -> u64 { let mut h = 0xcbf29ce484222325u64; for p in s { for b in p.to_le_bytes() { h ^= b as u64; h = h.wrapping_mul(0x100000001b3); } } h }

fn main() {
    let m = Mood { root: 52, mode: PHRYGIAN, bpm: 120, energy: 1 };
    let pr = |name: &str, (b, ok): (Bars, bool)| println!("{name:22} in_scale_min={:.2} step_share={:.2} max_leap={} bar_repeat={:.2} => {}", b.scale, b.step, b.leap, b.repeat, if ok { "PASS" } else { "FAIL" });
    pr("GEN rules", musical(&gen, m, 1000));
    pr("NEG chromatic random", musical(&gen_chromatic, m, 1000));
    pr("NEG in-scale random", musical(&gen_inscale_random, m, 1000));
    let pv = |name: &str, (mn, md, ok): (f64, f64, bool)| println!("{name:22} nn_min={mn:.3} nn_median={md:.3} => {}", if ok { "PASS" } else { "FAIL" });
    pv("VAR GEN 1000 seeds", variety(&|s| gen(s, m), 1000));
    pv("NEG constant seed", variety(&|_| gen(42, m), 1000));
    println!("tempo_lock GEN={} NEG jitter={}", tempo_lock(m, false), tempo_lock(m, true));
    println!("determinism seed 7 twice: {:016x} {:016x} equal={}", fnv(&gen(7, m)), fnv(&gen(7, m)), fnv(&gen(7, m)) == fnv(&gen(7, m)));
    println!("seed 7 vs 8 equal={}", fnv(&gen(7, m)) == fnv(&gen(8, m)));
    let s = gen(7, m);
    let (p, r) = render(&s, m, 0.0, true); println!("render master 0 dB limiter: peak={p:.2} dBFS rms={r:.1} dBFS");
    let (p, r) = render(&s, m, 12.0, false); println!("NEG master +12 dB no limiter: peak={p:.2} dBFS rms={r:.1} dBFS (bar peak <= -1)");
    let t = std::time::Instant::now(); for sd in 0..1000 { std::hint::black_box(gen(sd, m)); } println!("gen cost 1000 phrases = {:.3} ms", t.elapsed().as_secs_f64() * 1e3);
}
