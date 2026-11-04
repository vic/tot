// throwaway: arena-graph floor gen, variety metric, determinism. integer only.
use std::time::Instant;
const G: usize = 6; // grid cells per side
const C: usize = 32; // tiles per cell (1 tile = 1 m)
const W: usize = G * C; // 192
struct X([u64; 4]);
fn sm(s: &mut u64) -> u64 { *s = s.wrapping_add(0x9e3779b97f4a7c15); let mut z = *s; z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9); z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb); z ^ (z >> 31) }
impl X {
    fn new(seed: u64) -> X { let mut s = seed; X([sm(&mut s), sm(&mut s), sm(&mut s), sm(&mut s)]) }
    fn next(&mut self) -> u64 { let s = &mut self.0; let r = (s[0].wrapping_add(s[3])).rotate_left(23).wrapping_add(s[0]); let t = s[1] << 17; s[2] ^= s[0]; s[3] ^= s[1]; s[1] ^= s[2]; s[0] ^= s[3]; s[2] ^= t; s[3] = s[3].rotate_left(45); r }
    fn jump(&mut self) { const J: [u64; 4] = [0x180ec6d33cfd0aba, 0xd5a61266f0c9392c, 0xa9582618e03fc9aa, 0x39abdc4529b1661c]; let mut t = [0u64; 4]; for j in J { for b in 0..64 { if j & (1 << b) != 0 { for i in 0..4 { t[i] ^= self.0[i]; } } self.next(); } } self.0 = t; }
    fn below(&mut self, n: u32) -> u32 { let z = (u32::MAX - n + 1) % n; loop { let r = (self.next() >> 32) as u32; if r >= z { return r % n; } } }
}
// stream per (floor, layer): floor key via splitmix, layer = k jumps
fn stream(run: u64, floor: u64, layer: u32) -> X { let mut s = run ^ floor.wrapping_mul(0xd1b54a32d192ed03); let mut x = X::new(sm(&mut s)); for _ in 0..layer { x.jump(); } x }
struct Floor { t: Vec<u8>, used: [bool; G * G], e: Vec<(usize, usize)>, start: usize, exit: usize }
fn carve(t: &mut [u8], x0: usize, y0: usize, x1: usize, y1: usize) { for y in y0..y1 { for x in x0..x1 { t[y * W + x] = 1; } } }
fn nb(c: usize) -> Vec<usize> { let (x, y) = (c % G, c / G); let mut v = vec![]; if x > 0 { v.push(c - 1) } if x + 1 < G { v.push(c + 1) } if y > 0 { v.push(c - G) } if y + 1 < G { v.push(c + G) } v }
fn gen(run: u64, floor: u64, layout_run: u64) -> Floor {
    let mut m = stream(layout_run, floor, 0); // mission+embed layer
    let mut r = stream(run, floor, 1); // room template layer
    let mut used = [false; G * G]; let mut e = vec![];
    let start = [0usize, G - 1, G * (G - 1), G * G - 1][m.below(4) as usize];
    used[start] = true; let mut cells = vec![start]; let n = 10 + m.below(5) as usize;
    while cells.len() < n { // random frontier growth = spanning tree
        let mut fr = vec![]; for &c in &cells { for d in nb(c) { if !used[d] { fr.push((c, d)); } } }
        let (a, b) = fr[m.below(fr.len() as u32) as usize]; used[b] = true; cells.push(b); e.push((a, b));
    }
    let mut cand = vec![]; for &c in &cells { for d in nb(c) { if d > c && used[d] && !e.contains(&(c, d)) && !e.contains(&(d, c)) { cand.push((c, d)); } } }
    for _ in 0..2 { if cand.is_empty() { break } let i = m.below(cand.len() as u32) as usize; e.push(cand.swap_remove(i)); } // 2 cycles
    let mut dist = [u32::MAX; G * G]; dist[start] = 0; let mut q = vec![start]; let mut qi = 0; // graph BFS -> exit = farthest
    while qi < q.len() { let c = q[qi]; qi += 1; for &(a, b) in &e { let o = if a == c { b } else if b == c { a } else { continue }; if dist[o] == u32::MAX { dist[o] = dist[c] + 1; q.push(o); } } }
    let exit = *cells.iter().max_by_key(|&&c| (dist[c], c)).unwrap_or(&start);
    let mut t = vec![0u8; W * W];
    for &c in &cells { let (ox, oy) = ((c % G) * C, (c / G) * C); let k = r.below(3);
        let w = 16 + r.below(13) as usize; let h = 16 + r.below(13) as usize; let x0 = ox + (C - w) / 2; let y0 = oy + (C - h) / 2;
        carve(&mut t, x0, y0, x0 + w, y0 + h);
        if k == 1 { for y in (y0 + 3..y0 + h - 3).step_by(5) { for x in (x0 + 3..x0 + w - 3).step_by(5) { t[y * W + x] = 0; } } } // pillars
        if k == 2 { for _ in 0..(w * h / 6) { let x = x0 + 1 + r.below(w as u32 - 2) as usize; let y = y0 + 1 + r.below(h as u32 - 2) as usize; t[y * W + x] = 0; } } // rubble
    }
    for &(a, b) in &e { let (ax, ay) = ((a % G) * C + C / 2, (a / G) * C + C / 2); let (bx, by) = ((b % G) * C + C / 2, (b / G) * C + C / 2); // halls width 4
        carve(&mut t, ax.min(bx) - 2, ay.min(by) - 2, ax.max(bx) + 2, ay.max(by) + 2); }
    Floor { t, used, e, start, exit }
}
fn h64(t: &[u8]) -> u64 { let mut h = 0xcbf29ce484222325u64; for &b in t { h = (h ^ b as u64).wrapping_mul(0x100000001b3); } h }
fn flow(t: &[u8], s: usize, d: &mut [u16], q: &mut Vec<u32>) -> usize { d.fill(u16::MAX); q.clear(); d[s] = 0; q.push(s as u32); let mut i = 0;
    while i < q.len() { let c = q[i] as usize; i += 1; let (x, y) = (c % W, c / W); for o in [if x > 0 { c - 1 } else { c }, if x + 1 < W { c + 1 } else { c }, if y > 0 { c - W } else { c }, if y + 1 < W { c + W } else { c }] { if t[o] == 1 && d[o] == u16::MAX { d[o] = d[c] + 1; q.push(o as u32); } } } q.len() }
// throwaway: symmetric shadowcasting (Albert Ford, integer rationals), explorer vs beeline, RLE size. integer only.
const R: i64 = 14; // LOS radius tiles (1 tile = 1 m)
fn wall(t: &[u8], x: i64, y: i64) -> bool { x < 0 || y < 0 || x >= W as i64 || y >= W as i64 || t[y as usize * W + x as usize] == 0 }
fn fdiv(a: i64, b: i64) -> i64 { a.div_euclid(b) } // b > 0
// slope = p/q, q > 0
fn fov(t: &[u8], ox: i64, oy: i64, sym: bool, seen: &mut [u8], vis: &mut Vec<u32>) {
    vis.clear();
    let mut mark = |x: i64, y: i64, seen: &mut [u8], vis: &mut Vec<u32>| { if x >= 0 && y >= 0 && x < W as i64 && y < W as i64 && (x - ox) * (x - ox) + (y - oy) * (y - oy) <= R * R { let i = y as usize * W + x as usize; if seen[i] == 0 { vis.push(i as u32); } seen[i] = 1; } };
    mark(ox, oy, seen, vis);
    for quad in 0..4 {
        let tr = |d: i64, c: i64| -> (i64, i64) { match quad { 0 => (ox + c, oy - d), 1 => (ox + c, oy + d), 2 => (ox + d, oy + c), _ => (ox - d, oy + c) } };
        let mut st: Vec<(i64, i64, i64, i64, i64)> = vec![(1, -1, 1, 1, 1)]; // depth, sp, sq, ep, eq
        while let Some((d, mut sp, mut sq, ep, eq)) = st.pop() {
            if d > R { continue; }
            let lo = fdiv(2 * d * sp + sq, 2 * sq); // round ties up of d*s
            let hi = -fdiv(sq_neg(2 * d * ep, eq), 2 * eq); // round ties down of d*e
            let mut prev: Option<bool> = None;
            for c in lo..=hi {
                let (x, y) = tr(d, c); let w = wall(t, x, y);
                let symok = c * sq >= d * sp && c * eq <= d * ep;
                if w || !sym || symok { mark(x, y, seen, vis); }
                if prev == Some(true) && !w { sp = 2 * c - 1; sq = 2 * d; }
                if prev == Some(false) && w { st.push((d + 1, sp, sq, 2 * c - 1, 2 * d)); }
                prev = Some(w);
            }
            if prev == Some(false) { st.push((d + 1, sp, sq, ep, eq)); }
        }
    }
}
fn sq_neg(a: i64, q: i64) -> i64 { q - a } // ceil((a - q)/(2q)) = -floor((q - a)/(2q))
fn bfs_to(t: &[u8], s: usize, goal: &dyn Fn(usize) -> bool, rev: bool, par: &mut [u32]) -> Option<usize> {
    par.fill(u32::MAX); par[s] = s as u32; let mut q = vec![s]; let mut i = 0;
    while i < q.len() { let c = q[i]; i += 1; if c != s && goal(c) { return Some(c); }
        let (x, y) = (c % W, c / W); let mut n = [if x > 0 { c - 1 } else { c }, if x + 1 < W { c + 1 } else { c }, if y > 0 { c - W } else { c }, if y + 1 < W { c + W } else { c }];
        if rev { n.reverse(); }
        for o in n { if t[o] == 1 && par[o] == u32::MAX { par[o] = c as u32; q.push(o); } } }
    None
}
fn path(par: &[u32], s: usize, g: usize) -> Vec<usize> { let mut p = vec![g]; let mut c = g; while c != s { c = par[c] as usize; p.push(c); } p.pop(); p.reverse(); p }
fn rle_bytes(seen: &[u8]) -> usize { let mut n = 0; let mut cur = 0u8; let mut run = 0usize; for &b in seen { if b == cur { run += 1; } else { n += run.to_string().len() + 1; cur = b; run = 1; } } n + run.to_string().len() }
struct Walk { steps: usize, seen_walk: usize, walk: usize, new_max: usize, new_mean_x100: usize, fov_ns: u128, fovs: usize, rle: usize, hash: u64 }
fn walk(f: &Floor, explorer: bool, rev: bool, stop_steps: usize) -> Walk {
    let c2t = |c: usize| ((c % G) * C + C / 2) + ((c / G) * C + C / 2) * W;
    let (s0, ex) = (c2t(f.start), c2t(f.exit));
    let mut seen = vec![0u8; W * W]; let mut vis = Vec::with_capacity(4096); let mut par = vec![0u32; W * W];
    let mut p = s0; let mut steps = 0; let (mut new_max, mut new_sum) = (0, 0); let mut fov_ns = 0u128; let mut fovs = 0;
    let mut doit = |p: usize, seen: &mut Vec<u8>, vis: &mut Vec<u32>| { let before: usize = 0; let _ = before; let t0 = Instant::now(); fov(&f.t, (p % W) as i64, (p / W) as i64, true, seen, vis); let ns = t0.elapsed().as_nanos(); (vis.len(), ns) };
    let (n, ns) = doit(p, &mut seen, &mut vis); fov_ns += ns; fovs += 1; new_max = new_max.max(n);
    let mut plan: Vec<usize> = vec![]; let mut pi = 0;
    loop {
        if steps >= stop_steps { break; }
        if pi >= plan.len() || (explorer && plan.last().map_or(true, |&g| seen[g] == 1)) {
            let g = if explorer { let sn = &seen; bfs_to(&f.t, p, &|c| sn[c] == 0, rev, &mut par) } else { if p == ex { None } else { bfs_to(&f.t, p, &|c| c == ex, rev, &mut par) } };
            match g { None => break, Some(g) => { plan = path(&par, p, g); pi = 0; } }
        }
        p = plan[pi]; pi += 1; steps += 1;
        let (n, ns) = doit(p, &mut seen, &mut vis); fov_ns += ns; fovs += 1; new_max = new_max.max(n); new_sum += n;
    }
    let walk = f.t.iter().filter(|&&b| b == 1).count(); let seen_walk = (0..W * W).filter(|&i| f.t[i] == 1 && seen[i] == 1).count();
    Walk { steps, seen_walk, walk, new_max, new_mean_x100: new_sum * 100 / steps.max(1), fov_ns, fovs, rle: rle_bytes(&seen), hash: h64(&seen) }
}
fn main() {
    let base = 0x5eed_0000u64; let n = 20u64;
    let fl: Vec<Floor> = (0..n).map(|i| gen(base + i, 0, base + i)).collect();
    // symmetry check: A sees B <=> B sees A, sampled
    for symflag in [true, false] {
        let mut asym = 0; let mut pairs = 0; let mut sa = vec![0u8; W * W]; let mut sb = vec![0u8; W * W]; let mut v = vec![];
        for f in fl.iter().take(5) { let fls: Vec<usize> = (0..W * W).filter(|&i| f.t[i] == 1).collect(); let mut r = stream(7, 7, 0);
            for _ in 0..400 { let a = fls[r.below(fls.len() as u32) as usize]; sa.fill(0); fov(&f.t, (a % W) as i64, (a / W) as i64, symflag, &mut sa, &mut v);
                for _ in 0..10 { let b = fls[r.below(fls.len() as u32) as usize]; let (dx, dy) = ((a % W) as i64 - (b % W) as i64, (a / W) as i64 - (b / W) as i64); if dx * dx + dy * dy > R * R { continue; } pairs += 1; sb.fill(0); fov(&f.t, (b % W) as i64, (b / W) as i64, symflag, &mut sb, &mut v); if (sa[b] == 1) != (sb[a] == 1) { asym += 1; } } } }
        println!("{} pairs={pairs} asymmetric={asym}", if symflag { "SYMMETRIC" } else { "NEG non-symmetric (reveal whole row)" });
    }
    let mut rows = vec![];
    for f in &fl { let e = walk(f, true, false, usize::MAX); let b = walk(f, false, false, usize::MAX); let e2 = walk(f, true, false, usize::MAX); let er = walk(f, true, true, usize::MAX); let half = walk(f, true, false, e.steps / 2);
        rows.push((e.steps, e.seen_walk * 1000 / e.walk, b.steps, b.seen_walk * 1000 / b.walk, e.new_max, e.new_mean_x100, (e.fov_ns / e.fovs as u128) as u64, e.rle, half.rle, b.rle, e.hash == e2.hash, e.hash != er.hash)); }
    let med = |k: &dyn Fn(&(usize, usize, usize, usize, usize, usize, u64, usize, usize, usize, bool, bool)) -> u64| { let mut v: Vec<u64> = rows.iter().map(|r| k(r)).collect(); v.sort(); (v[0], v[v.len() / 2], v[v.len() - 1]) };
    println!("floors={n} radius={R} (min, median, max):");
    println!("explorer steps(m) {:?}", med(&|r| r.0 as u64)); println!("explorer seen walkable permille {:?}", med(&|r| r.1 as u64));
    println!("beeline steps(m) {:?}", med(&|r| r.2 as u64)); println!("beeline seen walkable permille {:?}", med(&|r| r.3 as u64));
    println!("new tiles seen per 1 m step: max {:?} mean_x100 {:?}", med(&|r| r.4 as u64), med(&|r| r.5 as u64));
    println!("fov ns per call {:?}", med(&|r| r.6));
    println!("RLE bytes seen bitmap: full explore {:?} half explore {:?} beeline {:?} (raw bits base64 = {})", med(&|r| r.7 as u64), med(&|r| r.8 as u64), med(&|r| r.9 as u64), (W * W / 8 + 2) / 3 * 4);
    println!("det same-run hash equal: {}/{n} ; NEG reversed BFS tie-break hash differs: {}/{n}", rows.iter().filter(|r| r.10).count(), rows.iter().filter(|r| r.11).count());
}
