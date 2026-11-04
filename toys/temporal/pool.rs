// toy B: sparse effect pool (swap-remove) + 1 slot per (entity, kind) + additive order-free time scale. std only.
// usage: pool <seed> <fill_permille> [rev]   rev = process pool in reverse order (order-independence control)
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

struct Count;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
unsafe impl GlobalAlloc for Count {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Ordering::Relaxed); unsafe { System.alloc(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static A: Count = Count;

const N: usize = 20_000;
const K: usize = 3; // kinds: 0 HASTE, 1 SLOW, 2 FREEZE
const TICKS: usize = 300;
const NONE: u32 = u32::MAX;
fn sm(s: &mut u64) -> u64 { *s = s.wrapping_add(0x9e3779b97f4a7c15); let mut z = *s; z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); z ^ (z >> 31) }
fn below(s: &mut u64, n: u64) -> u64 { sm(s) % n }

struct Pool { owner: Vec<u32>, kind: Vec<u8>, left: Vec<u16>, mag: Vec<i16>, len: usize, slot: Vec<u32> }
impl Pool {
    // stacking rule: one effect per (entity, kind); re-apply refreshes duration to max, magnitude to max
    fn apply(&mut self, i: usize, k: usize, d: u16, m: i16) {
        let s = self.slot[i * K + k];
        if s != NONE { let s = s as usize; self.left[s] = self.left[s].max(d); self.mag[s] = self.mag[s].max(m); return; }
        let j = self.len; self.len += 1;
        self.owner[j] = i as u32; self.kind[j] = k as u8; self.left[j] = d; self.mag[j] = m; self.slot[i * K + k] = j as u32;
    }
    fn remove(&mut self, j: usize) {
        let (o, k) = (self.owner[j] as usize, self.kind[j] as usize); self.slot[o * K + k] = NONE;
        let last = self.len - 1;
        if j != last { self.owner[j] = self.owner[last]; self.kind[j] = self.kind[last]; self.left[j] = self.left[last]; self.mag[j] = self.mag[last];
            let (o2, k2) = (self.owner[j] as usize, self.kind[j] as usize); self.slot[o2 * K + k2] = j as u32; }
        self.len = last;
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut rng: u64 = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(7);
    let fill: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(300);
    let rev = a.get(3).map(|s| s == "rev").unwrap_or(false);
    let (mut x, mut y, mut vx, mut vy) = (vec![0i32; N], vec![0i32; N], vec![0i32; N], vec![0i32; N]);
    let (mut clk, mut cd) = (vec![0u16; N], vec![0u16; N]);
    let (mut hsum, mut ssum, mut frz) = (vec![0i32; N], vec![0i32; N], vec![0u8; N]);
    let mut p = Pool { owner: vec![0; N * K], kind: vec![0; N * K], left: vec![0; N * K], mag: vec![0; N * K], len: 0, slot: vec![NONE; N * K] };
    for i in 0..N {
        x[i] = below(&mut rng, 200_000) as i32; y[i] = below(&mut rng, 200_000) as i32;
        vx[i] = below(&mut rng, 200) as i32 - 100; vy[i] = below(&mut rng, 200) as i32 - 100;
        cd[i] = below(&mut rng, 30) as u16;
        for k in 0..K { if below(&mut rng, 1000) < fill { let d = 30 + below(&mut rng, 270) as u16; let m = 300 + below(&mut rng, 400) as i16; p.apply(i, k, d, m); } }
    }
    let mut actions: u64 = 0;
    let mut per_tick = vec![0u128; TICKS];
    let a0 = ALLOCS.load(Ordering::Relaxed);
    for t in 0..TICKS {
        let t0 = Instant::now();
        for _ in 0..20 { let i = below(&mut rng, N as u64) as usize; let k = below(&mut rng, K as u64) as usize; let d = 30 + below(&mut rng, 270) as u16; p.apply(i, k, d, 500); }
        hsum.fill(0); ssum.fill(0); frz.fill(0);
        // pass 1: accumulate (sums + or-flag = order-independent), pass 2: expire
        let n = p.len;
        for jj in 0..n { let j = if rev { n - 1 - jj } else { jj }; let o = p.owner[j] as usize;
            match p.kind[j] { 0 => hsum[o] += p.mag[j] as i32 / 2, 1 => ssum[o] += p.mag[j] as i32, _ => frz[o] = 1 } }
        let mut j = 0; while j < p.len { p.left[j] -= 1; if p.left[j] == 0 { p.remove(j); } else { j += 1; } }
        for i in 0..N {
            let ts = if frz[i] != 0 { 0 } else { (1000 + hsum[i].min(500) - ssum[i]).clamp(250, 1500) }; // slow floor 25%, haste cap +50%
            x[i] += vx[i] * ts / 1000; y[i] += vy[i] * ts / 1000;
            let mut c = clk[i] as i32 + ts;
            while c >= 1000 { c -= 1000; if cd[i] > 0 { cd[i] -= 1 } else { cd[i] = 30; actions += 1; } }
            clk[i] = c as u16;
        }
        per_tick[t] = t0.elapsed().as_nanos();
    }
    let allocs = ALLOCS.load(Ordering::Relaxed) - a0;
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |v: u64| { h ^= v; h = h.wrapping_mul(0x100000001b3); };
    for i in 0..N { mix(x[i] as u64); mix(y[i] as u64); mix(clk[i] as u64); mix(cd[i] as u64); }
    mix(actions);
    let mut s = per_tick.clone(); s.sort();
    let mean = s.iter().sum::<u128>() / TICKS as u128;
    println!("pool fill={} rev={} mean_us={:.1} p99_us={:.1} ns_per_ent={:.1} allocs={} live_end={} actions={} hash={:016x}",
        fill, rev, mean as f64 / 1000.0, s[TICKS * 99 / 100] as f64 / 1000.0, mean as f64 / N as f64, allocs, p.len, actions, h);
}
