// toy: temporal effects as SoA tick counters + per-entity time-scale accumulator. std only.
// usage: tfx <mode> <seed> <fill_permille>   mode = fx | none | alloc
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
const S: usize = 4; // effect slots per entity
const TICKS: usize = 300;
// kinds: 0 none, 1 HASTE, 2 SLOW, 3 FREEZE
fn sm(s: &mut u64) -> u64 { *s = s.wrapping_add(0x9e3779b97f4a7c15); let mut z = *s; z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); z ^ (z >> 31) }
fn below(s: &mut u64, n: u64) -> u64 { sm(s) % n } // toy: modulo bias ok

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mode = a.get(1).map(|s| s.as_str()).unwrap_or("fx").to_string();
    let mut rng: u64 = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(7);
    let fill: u64 = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(300);
    let (mut x, mut y, mut vx, mut vy) = (vec![0i32; N], vec![0i32; N], vec![0i32; N], vec![0i32; N]);
    let (mut clk, mut cd) = (vec![0u16; N], vec![0u16; N]);
    let (mut kind, mut left, mut mag) = (vec![0u8; N * S], vec![0u16; N * S], vec![0i16; N * S]);
    for i in 0..N {
        x[i] = below(&mut rng, 200_000) as i32; y[i] = below(&mut rng, 200_000) as i32;
        vx[i] = below(&mut rng, 200) as i32 - 100; vy[i] = below(&mut rng, 200) as i32 - 100;
        cd[i] = below(&mut rng, 30) as u16;
        if mode != "none" { for s in 0..S { if below(&mut rng, 1000) < fill { let k = i * S + s;
            kind[k] = 1 + below(&mut rng, 3) as u8; left[k] = 30 + below(&mut rng, 270) as u16; mag[k] = 300 + below(&mut rng, 400) as i16; } } }
    }
    let mut sink: Vec<u32> = Vec::new(); // alloc control only
    let mut actions: u64 = 0;
    let mut per_tick = vec![0u128; TICKS];
    let a0 = ALLOCS.load(Ordering::Relaxed);
    for t in 0..TICKS {
        let t0 = Instant::now();
        // proc: 20 new effects per tick at random entities; stacking = first free slot, else refresh same kind to max
        if mode != "none" { for _ in 0..20 {
            let i = below(&mut rng, N as u64) as usize; let k = 1 + below(&mut rng, 3) as u8; let d = 30 + below(&mut rng, 270) as u16;
            let b = i * S; let mut put = false;
            for s in 0..S { if kind[b + s] == k { left[b + s] = left[b + s].max(d); put = true; break; } }
            if !put { for s in 0..S { if kind[b + s] == 0 { kind[b + s] = k; left[b + s] = d; mag[b + s] = 500; break; } } }
        } }
        for i in 0..N {
            let mut ts: i32 = 1000; // permille time scale
            let b = i * S;
            for s in 0..S { let k = b + s; match kind[k] {
                0 => {}
                kk => { left[k] -= 1; if left[k] == 0 { kind[k] = 0; } // effect clocks run in GLOBAL ticks
                    match kk { 1 => ts = ts * (1000 + mag[k] as i32 / 2) / 1000, 2 => ts = ts * (1000 - mag[k] as i32) / 1000, _ => ts = 0 }
                    if mode == "alloc" { sink.push(i as u32); } }
            } }
            let ts = ts.clamp(0, 1500);
            x[i] += vx[i] * ts / 1000; y[i] += vy[i] * ts / 1000; // continuous: scale velocity
            let mut c = clk[i] as i32 + ts; // discrete: local clock accumulator
            while c >= 1000 { c -= 1000; if cd[i] > 0 { cd[i] -= 1 } else { cd[i] = 30; actions += 1; } }
            clk[i] = c as u16;
        }
        if mode == "alloc" { sink = Vec::new(); }
        per_tick[t] = t0.elapsed().as_nanos();
    }
    let allocs = ALLOCS.load(Ordering::Relaxed) - a0;
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |v: u64| { h ^= v; h = h.wrapping_mul(0x100000001b3); };
    for i in 0..N { mix(x[i] as u64); mix(y[i] as u64); mix(clk[i] as u64); mix(cd[i] as u64); }
    for k in 0..N * S { mix(kind[k] as u64); mix(left[k] as u64); }
    mix(actions);
    let live = kind.iter().filter(|&&k| k != 0).count();
    let mut s = per_tick.clone(); s.sort();
    let mean = s.iter().sum::<u128>() / TICKS as u128;
    println!("mode={} fill={} mean_us={:.1} p99_us={:.1} ns_per_ent={:.1} allocs={} live_effects_end={} actions={} hash={:016x} sink={}",
        mode, fill, mean as f64 / 1000.0, s[TICKS * 99 / 100] as f64 / 1000.0, mean as f64 / N as f64, allocs, live, actions, h, sink.len());
}
