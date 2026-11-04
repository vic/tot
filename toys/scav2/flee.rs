// rev2 flee toy: player BFS + safety map (-1.2 x dist, rescan; RogueBasin Dijkstra maps).
// Rescan = exact multi-source Dijkstra for unit steps via merge of 2 sorted streams
// (sources counting-sorted by start cost + FIFO of relaxed tiles), O(N), no heap.
// Prints: build ns, 20k reader ns/ent, sink tiles (local minima) negated vs rescanned, hash.
use std::time::Instant;
const W: usize = 192;
const N: usize = W * W;
const NE: usize = 20_000;
const INF: u32 = u32::MAX;
const STEP: u32 = 5; // 1 tile = 5; start = 6 x (maxd - d) => slope 1.2
struct F { walk: Vec<bool>, dist: Vec<u32>, safe: Vec<u32>, q: Vec<u32>, cnt: Vec<u32>, src: Vec<u32>, fifo: Vec<u32> }
fn nbr(t: usize, j: usize) -> Option<usize> {
    let (x, y) = (t % W, t / W);
    match j { 0 if x > 0 => Some(t - 1), 1 if x < W - 1 => Some(t + 1), 2 if y > 0 => Some(t - W), 3 if y < W - 1 => Some(t + W), _ => None }
}
impl F {
    fn bfs(&mut self, s: usize) -> u32 {
        self.dist.fill(INF);
        self.q.clear();
        self.dist[s] = 0;
        self.q.push(s as u32);
        let (mut h, mut m) = (0, 0);
        while h < self.q.len() {
            let t = self.q[h] as usize;
            h += 1;
            for j in 0..4 {
                if let Some(u) = nbr(t, j) {
                    if self.walk[u] && self.dist[u] == INF {
                        self.dist[u] = self.dist[t] + 1;
                        m = m.max(self.dist[u]);
                        self.q.push(u as u32);
                    }
                }
            }
        }
        m
    }
    fn safety(&mut self, maxd: u32) {
        let nb = (6 * maxd + 1) as usize;
        self.cnt.clear();
        self.cnt.resize(nb + 1, 0);
        for t in 0..N {
            self.safe[t] = INF;
            if self.walk[t] && self.dist[t] != INF {
                self.cnt[(6 * (maxd - self.dist[t])) as usize + 1] += 1;
            }
        }
        for b in 0..nb { self.cnt[b + 1] += self.cnt[b]; }
        self.src.clear();
        self.src.resize(self.cnt[nb] as usize, 0);
        for t in 0..N {
            if self.walk[t] && self.dist[t] != INF {
                let c = (6 * (maxd - self.dist[t])) as usize;
                self.src[self.cnt[c] as usize] = t as u32;
                self.cnt[c] += 1;
                self.safe[t] = c as u32;
            }
        }
        self.fifo.clear();
        let (mut i, mut h) = (0, 0);
        loop {
            // pop min of the two sorted streams; ties: source stream first (fixed order)
            let t = match (self.src.get(i), self.fifo.get(h)) {
                (Some(&a), Some(&b)) => if self.key(a) <= self.key(b) { i += 1; a } else { h += 1; b },
                (Some(&a), None) => { i += 1; a }
                (None, Some(&b)) => { h += 1; b }
                (None, None) => break,
            } as usize;
            let v = self.safe[t] + STEP;
            for j in 0..4 {
                if let Some(u) = nbr(t, j) {
                    if self.walk[u] && self.safe[u] > v {
                        self.safe[u] = v;
                        self.fifo.push(u as u32);
                    }
                }
            }
        }
    }
    fn key(&self, t: u32) -> u32 { self.safe[t as usize] }
    fn sinks(&self, f: &[u32], mind: u32) -> u32 {
        (0..N).filter(|&t| self.walk[t] && self.dist[t] != INF && self.dist[t] >= mind
            && (0..4).filter_map(|j| nbr(t, j)).all(|u| !self.walk[u] || f[u] >= f[t])).count() as u32
    }
}
fn carve(w: &mut [bool], x0: usize, y0: usize, cw: usize, ch: usize) {
    for y in y0..(y0 + ch).min(W) { for x in x0..(x0 + cw).min(W) { w[y * W + x] = true; } }
}
fn main() {
    let mut walk = vec![false; N];
    for r in 0..12 {
        let (cx, cy) = ((r % 4) * 48 + 8, (r / 4) * 64 + 8);
        carve(&mut walk, cx, cy, 22, 22);
        if r % 4 < 3 { carve(&mut walk, cx + 22, cy + 9, 26, 4); }
        if r < 8 { carve(&mut walk, cx + 9, cy + 22, 4, 42); }
    }
    carve(&mut walk, 0, 120, 8, 4);
    let mut f = F { walk, dist: vec![0; N], safe: vec![0; N], q: Vec::with_capacity(N), cnt: Vec::with_capacity(8192), src: Vec::with_capacity(N), fifo: Vec::with_capacity(4 * N) };
    let pt = (8 + 64 + 11) * W + (8 + 48 + 11);
    let mode = std::env::args().nth(1).unwrap_or_default();
    let reps = 200;
    let t0 = Instant::now();
    let mut maxd = 0;
    for _ in 0..reps { maxd = f.bfs(pt); }
    let tb = t0.elapsed().as_nanos() / reps as u128;
    let t1 = Instant::now();
    for _ in 0..reps { f.safety(maxd); }
    let ts = t1.elapsed().as_nanos() / reps as u128;
    let neg: Vec<u32> = (0..N).map(|t| if f.walk[t] && f.dist[t] != INF { 6 * (maxd - f.dist[t]) } else { INF }).collect();
    let field: &[u32] = if mode == "neg" { &neg } else { &f.safe };
    let (sn, ss) = (f.sinks(&neg, 8), f.sinks(&f.safe, 8));
    // 20k readers, xorshift placement, fixed neighbour order W,E,N,S, strict < (tie = stay / first)
    let mut rs: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut rnd = |n: usize| { rs ^= rs << 13; rs ^= rs >> 7; rs ^= rs << 17; (rs % n as u64) as usize };
    let mut ent: Vec<usize> = (0..NE).map(|_| loop { let t = rnd(N); if f.walk[t] { break t; } }).collect();
    let t2 = Instant::now();
    let mut moved = 0u64;
    for _ in 0..300 {
        for e in ent.iter_mut() {
            let mut best = (*e, field[*e]);
            for j in 0..4 { if let Some(u) = nbr(*e, j) { if f.walk[u] && field[u] < best.1 { best = (u, field[u]); } } }
            if best.0 != *e { *e = best.0; moved += 1; }
        }
    }
    let tr = t2.elapsed().as_nanos() / 300;
    let stuck_far = ent.iter().filter(|&&t| f.dist[t] < 20).count();
    let hash = ent.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &t| (h ^ t as u64).wrapping_mul(0x100_0000_01b3));
    let walkc = f.walk.iter().filter(|&&w| w).count();
    println!("mode={} walkable={} maxd={} bfs_ns={} safety_ns={} reader_ns_tick={} ns_ent={:.1} moved={} sinks_d8 neg={} rescan={} ents_within_20_tiles_after={} hash={:016x}",
        if mode == "neg" { "neg" } else { "rescan" }, walkc, maxd, tb, ts, tr, tr as f64 / NE as f64, moved, sn, ss, stuck_far, hash);
}
