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
