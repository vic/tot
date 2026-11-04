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
