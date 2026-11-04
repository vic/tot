//! T3 toy (design v5 sec 22.11): integer horde sim on a generated floor (toys/gen layout rules).
//! Measures tick cost vs live count, hall jams, on-screen counts, contact pressure by player stance.
//! std only, integer sim (mm, ticks @30 Hz). modes: perf | stance
use std::time::Instant;

const G: usize = 6;
const C: usize = 32;
const W: usize = G * C; // 192 tiles, 1 tile = 1 m = 1000 mm
const R: i32 = 350; // trash radius mm
const PSPD: i32 = 167; // player 5.0 m/s @30 Hz

// ---------- rng (xoshiro256++ / splitmix64, same as toys/gen) ----------
struct X([u64; 4]);
fn sm(s: &mut u64) -> u64 {
    *s = s.wrapping_add(0x9e3779b97f4a7c15);
    let mut z = *s;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
    z ^ (z >> 31)
}
impl X {
    fn new(seed: u64) -> X {
        let mut s = seed;
        X([sm(&mut s), sm(&mut s), sm(&mut s), sm(&mut s)])
    }
    fn next(&mut self) -> u64 {
        let s = &mut self.0;
        let r = (s[0].wrapping_add(s[3])).rotate_left(23).wrapping_add(s[0]);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        r
    }
    fn below(&mut self, n: u32) -> u32 {
        let z = (u32::MAX - n + 1) % n;
        loop {
            let r = (self.next() >> 32) as u32;
            if r >= z {
                return r % n;
            }
        }
    }
}

// ---------- floor (toys/gen/gen.rs rules, condensed) ----------
fn carve(t: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize) {
    for y in y0..y1 {
        for x in x0..x1 {
            t[y * W + x] = 1;
        }
    }
}
fn nb(c: usize) -> Vec<usize> {
    let (x, y) = (c % G, c / G);
    let mut v = vec![];
    if x > 0 {
        v.push(c - 1)
    }
    if x + 1 < G {
        v.push(c + 1)
    }
    if y > 0 {
        v.push(c - G)
    }
    if y + 1 < G {
        v.push(c + G)
    }
    v
}
/// returns (tiles 1 = walkable, start tile)
fn floor(seed: u64) -> (Vec<u8>, usize) {
    let mut m = X::new(seed);
    let mut used = [false; G * G];
    let mut e = vec![];
    let start = [0usize, G - 1, G * (G - 1), G * G - 1][m.below(4) as usize];
    used[start] = true;
    let mut cells = vec![start];
    let n = 10 + m.below(5) as usize;
    while cells.len() < n {
        let mut fr = vec![];
        for &c in &cells {
            for d in nb(c) {
                if !used[d] {
                    fr.push((c, d));
                }
            }
        }
        let (a, b) = fr[m.below(fr.len() as u32) as usize];
        used[b] = true;
        cells.push(b);
        e.push((a, b));
    }
    let mut cand = vec![];
    for &c in &cells {
        for d in nb(c) {
            if d > c && used[d] && !e.contains(&(c, d)) && !e.contains(&(d, c)) {
                cand.push((c, d));
            }
        }
    }
    for _ in 0..2 {
        if cand.is_empty() {
            break;
        }
        let i = m.below(cand.len() as u32) as usize;
        e.push(cand.swap_remove(i));
    }
    let mut t = vec![0u8; W * W];
    for &c in &cells {
        let (ox, oy) = ((c % G) * C, (c / G) * C);
        let k = m.below(3);
        let w = 16 + m.below(13) as usize;
        let h = 16 + m.below(13) as usize;
        let (x0, y0) = (ox + (C - w) / 2, oy + (C - h) / 2);
        carve(&mut t, x0, y0, x0 + w, y0 + h);
        if k == 1 {
            for y in (y0 + 3..y0 + h - 3).step_by(5) {
                for x in (x0 + 3..x0 + w - 3).step_by(5) {
                    t[y * W + x] = 0;
                }
            }
        }
    }
    for &(a, b) in &e {
        let (ax, ay) = ((a % G) * C + C / 2, (a / G) * C + C / 2);
        let (bx, by) = ((b % G) * C + C / 2, (b / G) * C + C / 2);
        carve(&mut t, ax.min(bx) - 2, ay.min(by) - 2, ax.max(bx) + 2, ay.max(by) + 2);
    }
    let s = ((start % G) * C + C / 2) + ((start / G) * C + C / 2) * W;
    (t, s)
}
fn arena() -> (Vec<u8>, usize) {
    let mut t = vec![0u8; W * W];
    carve(&mut t, 64, 64, 128, 128);
    (t, 96 * W + 96)
}

// ---------- sim ----------
struct Sim {
    t: Vec<u8>,
    dist: Vec<u16>,
    q: Vec<u32>,
    ring: Vec<u32>,
    flow_tile: usize,
    // SoA, old + new positions (phase rule: read old, write new)
    x: Vec<i32>,
    y: Vec<i32>,
    nx: Vec<i32>,
    ny: Vec<i32>,
    spd: Vec<i32>,
    // uniform grid 1 m, counting sort
    cstart: Vec<u32>,
    occ: Vec<u32>,
    parked: Vec<bool>,
    order: Vec<u32>,
    px: i32,
    py: i32,
    rng: X,
    arrivals: u64,
    ph: [u128; 4], // ns: flow, grid, move, contact
    flows: u64,
    pairs: u64,
    maxocc: u32,
    cap: u32, // 0 = off (NEG control)
    fight: bool, // T5: contact = damage (no respawn), spawn ring = out of view + out of LOS
    touch: Vec<u32>,
    hold: Vec<bool>, // T5b: ranged enemy holds position (no seek), separation still applies
    pincer: bool,    // T5b: spawn = out of LOS only (fog hides it), path dist from 14 m
}
fn tile(x: i32, y: i32) -> usize {
    (y / 1000) as usize * W + (x / 1000) as usize
}
impl Sim {
    fn new(t: Vec<u8>, start: usize, n: usize, seed: u64) -> Sim {
        let mut s = Sim {
            dist: vec![u16::MAX; W * W],
            q: Vec::with_capacity(W * W),
            ring: Vec::with_capacity(W * W),
            flow_tile: usize::MAX,
            x: vec![0; n],
            y: vec![0; n],
            nx: vec![0; n],
            ny: vec![0; n],
            spd: vec![0; n],
            cstart: vec![0; W * W + 2],
            occ: vec![0; W * W],
            parked: vec![false; n],
            order: vec![0; n],
            px: (start % W) as i32 * 1000 + 500,
            py: (start / W) as i32 * 1000 + 500,
            rng: X::new(seed),
            arrivals: 0,
            ph: [0; 4],
            flows: 0,
            pairs: 0,
            maxocc: 0,
            cap: std::env::var("TOT_CAP").ok().and_then(|v| v.parse().ok()).unwrap_or(3),
            fight: false,
            hold: Vec::new(),
            pincer: false,
            touch: Vec::new(),
            t,
        };
        s.flow();
        for i in 0..n {
            s.spd[i] = 93 + s.rng.below(41) as i32; // 2.8-4.0 m/s
            s.respawn(i);
        }
        s
    }
    fn walk(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < (W * 1000) as i32 && y < (W * 1000) as i32 && self.t[tile(x, y)] == 1
    }
    fn flow(&mut self) {
        let s = tile(self.px, self.py);
        self.flow_tile = s;
        self.flows += 1;
        self.dist.fill(u16::MAX);
        self.q.clear();
        self.ring.clear();
        self.dist[s] = 0;
        self.q.push(s as u32);
        let mut i = 0;
        while i < self.q.len() {
            let c = self.q[i] as usize;
            i += 1;
            let d = self.dist[c];
            let dmin = if self.pincer { 14 } else { 26 };
            if (dmin..=32).contains(&d) && (!self.fight || (!self.los(s, c) && (self.pincer || !self.in_view(c)))) {
                self.ring.push(c as u32);
            }
            for o in [c.wrapping_sub(1), c + 1, c.wrapping_sub(W), c + W] {
                if o < W * W && self.t[o] == 1 && self.dist[o] == u16::MAX {
                    self.dist[o] = d + 1;
                    self.q.push(o as u32);
                }
            }
        }
    }
    /// spawn on a ring tile with room (density invariant at the spawn write site); none => parked, retried next tick
    fn respawn(&mut self, i: usize) {
        self.parked[i] = true;
        for _ in 0..8 {
            if self.ring.is_empty() {
                return;
            }
            let c = self.ring[self.rng.below(self.ring.len() as u32) as usize] as usize;
            if self.cap == 0 || self.occ[c] < self.cap {
                self.occ[c] += 1;
                self.parked[i] = false;
                self.x[i] = (c % W) as i32 * 1000 + 200 + self.rng.below(600) as i32;
                self.y[i] = (c / W) as i32 * 1000 + 200 + self.rng.below(600) as i32;
                return;
            }
        }
    }
    fn tl(&self, i: usize) -> usize {
        if self.parked[i] { W * W } else { tile(self.x[i], self.y[i]) }
    }
    fn tick(&mut self) {
        let n = self.x.len();
        let t0 = Instant::now();
        if tile(self.px, self.py) != self.flow_tile {
            self.flow();
        }
        let t1 = Instant::now();
        self.cstart.fill(0);
        for i in 0..n {
            let k = self.tl(i);
            self.cstart[k + 1] += 1;
        }
        for k in 0..W * W + 1 {
            self.cstart[k + 1] += self.cstart[k];
        }
        for k in 0..W * W {
            self.occ[k] = self.cstart[k + 1] - self.cstart[k];
        }
        let mut fill = self.cstart.clone(); // shortcut: 1 alloc/tick (toy), product keeps a 2nd prealloc array
        for i in 0..n {
            let c = self.tl(i);
            self.order[fill[c] as usize] = i as u32;
            fill[c] += 1;
        }
        self.maxocc = self.maxocc.max((0..W * W).map(|k| self.cstart[k + 1] - self.cstart[k]).max().unwrap_or(0));
        let t2 = Instant::now();
        for i in 0..n {
            let (x, y) = (self.x[i], self.y[i]);
            if self.parked[i] {
                (self.nx[i], self.ny[i]) = (x, y);
                continue;
            }
            let c = tile(x, y);
            // steer: steepest descent over 8 neighbours (diagonal only if both orthogonals open)
            let (mut best, mut bd) = (c, self.dist[c]);
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
                let o = c as i32 + dx + dy * W as i32;
                if o < 0 || o as usize >= W * W {
                    continue;
                }
                let o = o as usize;
                let diag_ok = dx == 0 || dy == 0 || (self.t[(c as i32 + dx) as usize] == 1 && self.t[(c as i32 + dy * W as i32) as usize] == 1);
                if diag_ok && self.dist[o] < bd {
                    (best, bd) = (o, self.dist[o]);
                }
            }
            let (tx, ty) = if best == c { (self.px, self.py) } else { ((best % W) as i32 * 1000 + 500, (best / W) as i32 * 1000 + 500) };
            let (dx, dy) = ((tx - x) as i64, (ty - y) as i64);
            let d = (dx * dx + dy * dy).isqrt().max(1);
            let s = self.spd[i] as i64;
            let (mut vx, mut vy) = ((dx * s / d) as i32, (dy * s / d) as i32);
            if self.hold.get(i).copied().unwrap_or(false) {
                (vx, vy) = (0, 0);
            }
            // separation: 3x3 cells, push apart when overlapping
            let (cx, cy) = (c % W, c / W);
            for yy in cy.saturating_sub(1)..=(cy + 1).min(W - 1) {
                for xx in cx.saturating_sub(1)..=(cx + 1).min(W - 1) {
                    let k = yy * W + xx;
                    for &j in &self.order[self.cstart[k] as usize..self.cstart[k + 1] as usize] {
                        let j = j as usize;
                        self.pairs += 1;
                        if j == i {
                            continue;
                        }
                        let (ex, ey) = ((x - self.x[j]) as i64, (y - self.y[j]) as i64);
                        let dd = ex * ex + ey * ey;
                        if dd < (4 * R * R) as i64 {
                            let dl = dd.isqrt().max(1);
                            let push = (2 * R as i64 - dl) / 2;
                            let (ex, ey) = if dd == 0 { ((i as i64 % 3) - 1, (i as i64 % 2) * 2 - 1) } else { (ex, ey) };
                            vx += (ex * push / dl.max(1)).clamp(-60, 60) as i32;
                            vy += (ey * push / dl.max(1)).clamp(-60, 60) as i32;
                        }
                    }
                }
            }
            let (mut nx, mut ny) = (x, y);
            // density invariant: never enter another tile holding >= cap bodies (old counts => order-free)
            let room = |s: &Sim, ax: i32, ay: i32| {
                let k = tile(ax, ay);
                k == c || s.cap == 0 || s.occ[k] < s.cap
            };
            if self.walk(x + vx + vx.signum() * R, y) && room(self, x + vx, y) {
                nx = x + vx;
            }
            if self.walk(nx, y + vy + vy.signum() * R) && room(self, nx, y + vy) {
                ny = y + vy;
            }
            let nt = tile(nx, ny);
            if nt != c {
                self.occ[c] -= 1;
                self.occ[nt] += 1;
            }
            self.nx[i] = nx;
            self.ny[i] = ny;
        }
        std::mem::swap(&mut self.x, &mut self.nx);
        std::mem::swap(&mut self.y, &mut self.ny);
        let t3 = Instant::now();
        // contact (serial): touching the player = 1 arrival, enemy respawns on the ring
        let reach = (R + 300) as i64;
        self.touch.clear();
        for i in 0..n {
            if self.parked[i] {
                if !self.fight {
                    self.respawn(i);
                }
                continue;
            }
            let (ex, ey) = ((self.x[i] - self.px) as i64, (self.y[i] - self.py) as i64);
            if ex * ex + ey * ey < reach * reach {
                if self.fight {
                    self.touch.push(i as u32);
                } else {
                    self.arrivals += 1;
                    self.respawn(i);
                }
            }
        }
        let t4 = Instant::now();
        for (k, (a, b)) in [(t0, t1), (t1, t2), (t2, t3), (t3, t4)].into_iter().enumerate() {
            self.ph[k] += b.duration_since(a).as_nanos();
        }
    }
    /// kiter: move away from mean of enemies within 8 m, wall slide
    fn kite(&mut self) {
        let (mut sx, mut sy, mut k) = (0i64, 0i64, 0i64);
        let c = tile(self.px, self.py);
        let (cx, cy) = (c % W, c / W);
        for yy in cy.saturating_sub(8)..=(cy + 8).min(W - 1) {
            for xx in cx.saturating_sub(8)..=(cx + 8).min(W - 1) {
                let t = yy * W + xx;
                for &j in &self.order[self.cstart[t] as usize..self.cstart[t + 1] as usize] {
                    sx += (self.px - self.x[j as usize]) as i64;
                    sy += (self.py - self.y[j as usize]) as i64;
                    k += 1;
                }
            }
        }
        if k == 0 {
            return;
        }
        let d = (sx * sx + sy * sy).isqrt().max(1);
        let (vx, vy) = ((sx * PSPD as i64 / d) as i32, (sy * PSPD as i64 / d) as i32);
        let r = 300;
        if self.walk(self.px + vx + vx.signum() * r, self.py) {
            self.px += vx;
        }
        if self.walk(self.px, self.py + vy + vy.signum() * r) {
            self.py += vy;
        }
    }
    /// enemies inside the T1 view rect (320x96, 40 m, aspect 2.1, player at 0.58 height)
    fn on_screen(&self) -> usize {
        let kc = 320.0 / (std::f64::consts::SQRT_2 * 40.0);
        (0..self.x.len())
            .filter(|&i| !self.parked[i])
            .filter(|&i| {
                let (dx, dy) = ((self.x[i] - self.px) as f64 / 1000.0, (self.y[i] - self.py) as f64 / 1000.0);
                let (a, b) = (dx - dy, dx + dy);
                let sub = b * kc / 2.1;
                (a * kc).abs() <= 160.0 && (-111.0..=81.0).contains(&sub)
            })
            .count()
    }
    fn hash(&self) -> u64 {
        self.x.iter().chain(self.y.iter()).fold(0xcbf29ce484222325u64, |h, &v| (h ^ v as u32 as u64).wrapping_mul(0x100000001b3))
    }
}
fn openness(t: &[u8], c: usize) -> u32 {
    let (x, y) = ((c % W) as i32, (c / W) as i32);
    let mut n = 0;
    for yy in y - 4..=y + 4 {
        for xx in x - 4..=x + 4 {
            if xx >= 0 && yy >= 0 && (xx as usize) < W && (yy as usize) < W && t[yy as usize * W + xx as usize] == 1 {
                n += 1;
            }
        }
    }
    n
}

fn perf() {
    for n in [600usize, 1500, 5000, 20000] {
        let (t, s) = floor(42);
        let mut sim = Sim::new(t, s, n, 7);
        let mut times = vec![];
        for k in 0..300 {
            if k % 2 == 0 {
                sim.kite();
            }
            let t0 = Instant::now();
            sim.tick();
            times.push(t0.elapsed().as_micros() as u64);
        }
        times.sort_unstable();
        let ph: Vec<u128> = sim.ph.iter().map(|v| v / 300 / 1000).collect();
        println!("{{\"n\":{n},\"tick_us_mean\":{},\"tick_us_p99\":{},\"ns_per_ent\":{},\"phase_us_mean_flow_grid_move_contact\":{ph:?},\"flows\":{},\"pairs_per_ent_tick\":{},\"max_cell_occ\":{}}}", times.iter().sum::<u64>() / 300, times[297], times.iter().sum::<u64>() * 1000 / 300 / n as u64, sim.flows, sim.pairs / 300 / n as u64, sim.maxocc);
        println!("  parked at end {} of {n}", sim.parked.iter().filter(|&&p| p).count());
    }
    // determinism: same seed => same hash; different seed differs (NEG)
    let run = |seed| {
        let (t, s) = floor(42);
        let mut sim = Sim::new(t, s, 1500, seed);
        for _ in 0..300 {
            sim.kite();
            sim.tick();
        }
        sim.hash()
    };
    let (a, b, c) = (run(7), run(7), run(8));
    println!("det same-seed equal {} ; NEG other-seed differs {}", a == b, a != c);
}

fn stance() {
    // live 600 (FP cap), 2 min @30 Hz, 8 floor seeds; arrivals/s = contact pressure on the player
    println!("stance | floors | arrivals/s mean (min..max) | on-screen mean / p95 | stuck share");
    for (name, open, kite, pick_hall) in [("STILL room", false, false, false), ("STILL hall", false, false, true), ("KITER dungeon", false, true, false), ("STILL arena", true, false, false), ("KITER arena", true, true, false)] {
        let (mut rates, mut scr, mut stuck_sum, mut stuck_n) = (vec![], vec![], 0u64, 0u64);
        for seed in 0..8u64 {
            let (t, mut s) = if open { arena() } else { floor(100 + seed) };
            if pick_hall {
                // hall tile: walkable, openness <= 40 of 81, far from room centres (first by scan order = deterministic)
                if let Some(c) = (0..W * W).find(|&c| t[c] == 1 && openness(&t, c) <= 40 && openness(&t, c) >= 30) {
                    s = c;
                }
            }
            let mut sim = Sim::new(t, s, 600, 7 + seed);
            let mut last = sim.x.clone();
            for k in 0..3600 {
                if kite && k % 2 == 0 {
                    sim.kite();
                }
                sim.tick();
                if k >= 600 && k % 30 == 0 {
                    scr.push(sim.on_screen() as u64);
                }
                if k % 90 == 89 {
                    // stuck = moved < 0.3 m in 3 s and not adjacent to player
                    for (i, l) in last.iter().enumerate() {
                        let far = ((sim.x[i] - sim.px) as i64).abs() + ((sim.y[i] - sim.py) as i64).abs() > 3000;
                        if far && (sim.x[i] - l).abs() < 300 {
                            stuck_sum += 1;
                        }
                        stuck_n += 1;
                    }
                    last.clone_from(&sim.x);
                }
            }
            rates.push(sim.arrivals as f64 / 120.0);
        }
        scr.sort_unstable();
        let mean = rates.iter().sum::<f64>() / rates.len() as f64;
        let (lo, hi) = rates.iter().fold((f64::MAX, f64::MIN), |(a, b), &r| (a.min(r), b.max(r)));
        println!(
            "{name} | 8 | {mean:.1} ({lo:.1}..{hi:.1}) | {} / {} | {:.1}%",
            scr.iter().sum::<u64>() / scr.len().max(1) as u64,
            scr[scr.len() * 95 / 100],
            stuck_sum as f64 * 100.0 / stuck_n.max(1) as f64
        );
    }
}

// ---------- T5: combat on the floor ----------
impl Sim {
    /// tile inside the T1 view rect (320x96, 40 m) around the player
    fn in_view(&self, c: usize) -> bool {
        let kc = 320.0 / (std::f64::consts::SQRT_2 * 40.0);
        let (dx, dy) = ((c % W) as f64 + 0.5 - self.px as f64 / 1000.0, (c / W) as f64 + 0.5 - self.py as f64 / 1000.0);
        let sub = (dx + dy) * kc / 2.1;
        ((dx - dy) * kc).abs() <= 160.0 && (-111.0..=81.0).contains(&sub)
    }
    /// tile line of sight (Bresenham over tile centres, all walkable)
    fn los(&self, a: usize, b: usize) -> bool {
        let (mut x, mut y) = ((a % W) as i32, (a / W) as i32);
        let (x1, y1) = ((b % W) as i32, (b / W) as i32);
        let (dx, dy) = ((x1 - x).abs(), -(y1 - y).abs());
        let (sx, sy) = ((x1 - x).signum(), (y1 - y).signum());
        let mut e = dx + dy;
        loop {
            if self.t[y as usize * W + x as usize] == 0 {
                return false;
            }
            if x == x1 && y == y1 {
                return true;
            }
            let e2 = 2 * e;
            if e2 >= dy {
                e += dy;
                x += sx;
            }
            if e2 <= dx {
                e += dx;
                y += sy;
            }
        }
    }
}
/// design sec 11.4 / 12.2 / 10.3 numbers [E]; f64 curves (toy), state integer milli-units
struct Fight {
    s: Sim,
    hp: Vec<i64>,
    hit_cd: Vec<u8>,
    php: i64,
    t: u32,
    budget: i64,
    xp: u32,
    level: u32,
    picks: bool,
    fire_cd: u32,
    kills: u32,
    taken_hits: u32,
    set: Set,
    fam: Vec<u8>, // 0 trash, 1 fast, 2 pinger
    shot_cd: Vec<u8>,
    shots: Vec<[i32; 5]>, // x, y, vx, vy, life
}
/// T5b anti-cheese set (OPEN 44), each part a knob
#[derive(Clone, Copy, Default)]
struct Set {
    ranged: bool,
    fast: bool,
    pincer: bool,
    purge: bool,
}
fn set_of(s: &str) -> Set {
    let has = |k: &str| s == "all" || s.split('+').any(|p| p == k);
    Set { ranged: has("ranged"), fast: has("fast"), pincer: has("pincer"), purge: has("purge") }
}
const PMAX: i64 = 320_000;
fn need(l: u32) -> u32 {
    match l {
        0..=5 => 14,
        6..=11 => 56,
        12..=19 => 123,
        20..=25 => 312,
        _ => 500,
    }
}
impl Fight {
    fn new(t: Vec<u8>, start: usize, live: usize, seed: u64, picks: bool, set: Set) -> Fight {
        let mut s = Sim::new(t, start, 0, seed);
        s.fight = true;
        s.pincer = set.pincer;
        s.hold = vec![false; live];
        s.x = vec![0; live];
        s.y = vec![0; live];
        s.nx = vec![0; live];
        s.ny = vec![0; live];
        s.order = vec![0; live];
        s.parked = vec![true; live];
        s.spd = (0..live).map(|_| 93 + s.rng.below(41) as i32).collect();
        s.flow_tile = usize::MAX;
        s.flow();
        Fight { s, hp: vec![0; live], hit_cd: vec![0; live], php: PMAX, t: 0, budget: 0, xp: 0, level: 1, picks, fire_cd: 0, kills: 0, taken_hits: 0, set, fam: vec![0; live], shot_cd: vec![0; live], shots: Vec::new() }
    }
    fn minute(&self) -> f64 {
        self.t as f64 / 1800.0
    }
    /// one 30 Hz tick; Some(death tick) when the player dies
    fn step(&mut self, kite: bool) -> Option<u32> {
        let m = self.minute();
        self.t += 1;
        // spawns: N(t) = 1 + 0.35 t per s, trash HP 30 x 1.12^t
        let purge = if self.set.purge && m > 6.0 { 1.0 + 0.5 * (m - 6.0) } else { 1.0 };
        self.budget += ((1.0 + 0.35 * m) * purge * 1000.0 / 30.0) as i64;
        let ehp = (30.0 * 1.12f64.powf(m) * 1000.0) as i64;
        let mut i = 0;
        while self.budget >= 1000 && i < self.hp.len() {
            if self.s.parked[i] {
                self.s.respawn(i);
                if self.s.parked[i] {
                    break; // no room / no out-of-view ring tile: wallet banks
                }
                self.hp[i] = ehp;
                let r = self.s.rng.below(100);
                self.fam[i] = if self.set.ranged && r < 20 { 2 } else if self.set.fast && r < 35 { 1 } else { 0 };
                self.s.spd[i] = if self.fam[i] == 1 { 140 } else { 93 + self.s.rng.below(41) as i32 };
                self.budget -= 1000;
            }
            i += 1;
        }
        self.budget = self.budget.min(60_000);
        if kite && self.t.is_multiple_of(2) {
            self.s.kite();
        }
        let pt = tile(self.s.px, self.s.py);
        for i in 0..self.hp.len() {
            let near = !self.s.parked[i] && self.fam[i] == 2 && {
                let (dx, dy) = ((self.s.x[i] - self.s.px) as i64, (self.s.y[i] - self.s.py) as i64);
                dx * dx + dy * dy < 64_000_000
            };
            self.s.hold[i] = near && self.s.los(pt, tile(self.s.x[i], self.s.y[i]));
        }
        self.s.tick();
        // ranged: PINGER fires every 2.5 s at 8 m/s, walls stop shots, hit radius 0.5 m
        let rdmg = (8.0 * (1.0 + 0.04 * m) * 1000.0) as i64;
        for i in 0..self.hp.len() {
            self.shot_cd[i] = self.shot_cd[i].saturating_sub(1);
            if self.s.hold[i] && !self.s.parked[i] && self.shot_cd[i] == 0 {
                let (dx, dy) = ((self.s.px - self.s.x[i]) as i64, (self.s.py - self.s.y[i]) as i64);
                let d = (dx * dx + dy * dy).isqrt().max(1);
                self.shots.push([self.s.x[i], self.s.y[i], (dx * 267 / d) as i32, (dy * 267 / d) as i32, 90]);
                self.shot_cd[i] = 75;
            }
        }
        let (px, py) = (self.s.px, self.s.py);
        let mut hit = 0;
        self.shots.retain_mut(|s| {
            s[0] += s[2];
            s[1] += s[3];
            s[4] -= 1;
            let (dx, dy) = ((s[0] - px) as i64, (s[1] - py) as i64);
            if dx * dx + dy * dy < 250_000 {
                hit += 1;
                return false;
            }
            s[4] > 0
        });
        let s = &self.s;
        self.shots.retain(|q| s.walk(q[0], q[1]));
        self.php -= hit * rdmg;
        self.taken_hits += hit as u32;
        // contact damage, once per 15 ticks per enemy
        let dmg = (6.0 * (1.0 + 0.04 * m) * 1000.0) as i64;
        for c in self.hit_cd.iter_mut() {
            *c = c.saturating_sub(1);
        }
        for k in 0..self.s.touch.len() {
            let i = self.s.touch[k] as usize;
            if self.hit_cd[i] == 0 {
                self.php -= dmg;
                self.hit_cd[i] = 15;
                self.taken_hits += 1;
            }
        }
        self.php = (self.php + 200 / 30).min(PMAX);
        if self.php <= 0 {
            return Some(self.t);
        }
        // autofire: PACKET BOLT 40 x 1.2/s x crit 1.1 x 1.142^picks; area abstraction = damage spills to
        // nearest enemies within 4 m of the target (no overkill waste), LOS + range 12 m
        if self.fire_cd > 0 {
            self.fire_cd -= 1;
            return None;
        }
        self.fire_cd = 25;
        let picks = if self.picks { self.level - 1 } else { 0 };
        let mut pool = (44.0 * 1.142f64.powi(picks as i32) * 1000.0) as i64;
        let pt = tile(self.s.px, self.s.py);
        let near = |f: &Fight, cx: i32, cy: i32, r: i64| -> Vec<(i64, usize)> {
            let mut v: Vec<(i64, usize)> = (0..f.hp.len())
                .filter(|&i| !f.s.parked[i])
                .map(|i| {
                    let (dx, dy) = ((f.s.x[i] - cx) as i64, (f.s.y[i] - cy) as i64);
                    (dx * dx + dy * dy, i)
                })
                .filter(|q| q.0 <= r * r)
                .collect();
            v.sort_unstable();
            v
        };
        let &(_, tgt) = near(self, self.s.px, self.s.py, 12_000).iter().find(|q| self.s.los(pt, tile(self.s.x[q.1], self.s.y[q.1])))?;
        for (_, i) in near(self, self.s.x[tgt], self.s.y[tgt], 4_000) {
            if pool <= 0 {
                break;
            }
            let d = pool.min(self.hp[i]);
            self.hp[i] -= d;
            pool -= d;
            if self.hp[i] <= 0 {
                self.s.parked[i] = true;
                self.kills += 1;
                self.xp += 1;
                if self.xp >= need(self.level) {
                    self.xp -= need(self.level);
                    self.level += 1;
                }
            }
        }
        None
    }
}
/// hall tile with evidence: straight run >= 12 along one axis, <= 5 across
fn hall(t: &[u8]) -> Option<(usize, i32, i32)> {
    let run = |c: usize, dx: i32, dy: i32| {
        let mut n = 0;
        for s in [-1, 1] {
            let (mut x, mut y) = ((c % W) as i32, (c / W) as i32);
            loop {
                x += dx * s;
                y += dy * s;
                if x < 0 || y < 0 || x >= W as i32 || y >= W as i32 || t[y as usize * W + x as usize] == 0 {
                    break;
                }
                n += 1;
            }
        }
        n + 1
    };
    (0..W * W).filter(|&c| t[c] == 1).map(|c| (c, run(c, 1, 0), run(c, 0, 1))).find(|&(_, h, v)| (h <= 5 && v >= 12) || (v <= 5 && h >= 12))
}
fn fight(sets: &[String]) {
    let ms = |t: u32| format!("{}:{:02}", t / 1800, t / 30 % 60);
    for name in sets {
        let set = set_of(name);
        println!("## set {name} | stance picks | death median (min..max), 15:00 = survived | kills median | level median | hits taken/s");
        let mut med = std::collections::HashMap::new();
        for (stance, hall_tile, kite) in [("STILL room", false, false), ("STILL hall", true, false), ("KITER dungeon", false, true)] {
            for picks in [true, false] {
                let (mut deaths, mut kills, mut lv, mut hits) = (vec![], vec![], vec![], 0f64);
                for seed in 0..6u64 {
                    let (t, mut s) = floor(200 + seed);
                    if hall_tile && let Some((c, _, _)) = hall(&t) {
                        s = c;
                    }
                    let mut f = Fight::new(t, s, 600, 11 + seed, picks, set);
                    let end = loop {
                        if let Some(d) = f.step(kite) {
                            break d;
                        }
                        if f.t >= 27_000 {
                            break 27_000;
                        }
                    };
                    deaths.push(end);
                    kills.push(f.kills);
                    lv.push(f.level);
                    hits += f.taken_hits as f64 / (end as f64 / 30.0);
                }
                deaths.sort_unstable();
                kills.sort_unstable();
                lv.sort_unstable();
                med.insert((stance, picks), deaths[3]);
                println!("{stance} {} | {} ({}..{}) | {} | {} | {:.1}", if picks { "picks" } else { "NONE " }, ms(deaths[3]), ms(deaths[0]), ms(deaths[5]), kills[3], lv[3], hits / 6.0);
            }
        }
        let g = |k: (&str, bool)| med.get(&k).copied().unwrap_or(0);
        let hall_ok = g(("STILL hall", true)) < 9000;
        let kite_ok = g(("KITER dungeon", false)) as f64 <= 0.6 * g(("KITER dungeon", true)) as f64;
        let n1_ok = g(("STILL room", true)) < 9000;
        println!("GATE {name}: hall-dies<5:00 {hall_ok} | kiter-NONE<=60%-of-picks {kite_ok} | N1 still-dies<5:00 {n1_ok}");
    }
}

fn main() {
    match std::env::args().nth(1).as_deref() {
        Some("fight") => fight(&std::env::args().skip(2).collect::<Vec<_>>()),
        Some("perf") => perf(),
        Some("stance") => stance(),
        _ => eprintln!("usage: horde perf | stance"),
    }
}
