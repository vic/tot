# Audit: proto2 smooth camera (P8), read-only

Auditor: opus-high-proto2-smooth (opus-5.5/high), 2026-11-03 ~21:10. Source: proto2 at rest (no src/data change for 227 s before the audit).
Method: read src/cam.rs, src/iso.rs, src/render.rs, src/play.rs, src/sim/step.rs. Probes ran in a scratch COPY
(/tmp/claude-1000/-home-vic-hk-tot/ebc7f377-f239-4526-93a6-2bc7eba28345/scratchpad/p2audit, tests `audit_*` appended to render.rs tests).
No file in proto2 changed except this one.

## test.sh (one run)

    tests pass=47 fail=0 over1s=0 lint=0
    slowest ms: 366 render::tests::scroll_and_turn_bytes_measured_against_budget

## Verdict per brief item

| item | verdict | evidence |
|---|---|---|
| sim hash 0x4e150460130ec301 | PASS | sim.rs:806 `iso_render_leaves_sim_hash_unchanged` asserts it; green in test.sh |
| no float in hashed state | PASS | `Frame::compose(&mut self, sim: &Sim, ..)` takes `&Sim` (immutable), so render cannot write sim. Cam/Proj f64 live in Frame only. Input keeps integer `Heading` (input.rs:419 `screen_dir`). rg `f64|Proj|Cam` in sim.rs/director.rs/floorgen.rs/input.rs: 0 hits. |
| O-interp alpha 0/1 exact | PASS (probe), NOT IN SUITE | probe: `moved=9 alpha0 exact=true alpha1 exact=true / NEG alpha0.5 as0=false as1=false`. The suite has only `interpolation_moves_actors_between_ticks` (lerp at 0.5 on constants). Add the alpha 0/1 + wrong-alpha test. |
| O-smooth max step | PASS, WEAK NEG | suite X = 1.5 cols incl. a dodge. Snap control (follow_ms 0) fails ONLY by the dodge (2.15 cols). Probe without dodge: snap `O-smooth=Ok(0.354)` and steady steps min 0.354 max 0.354, smooth min 0.326 max 0.361. Reason: snap follows the INTERPOLATED player, which is already smooth. The NEG proves follow lag, not walk smoothness. A judder control needs alpha pinned to 1 (no interp). |
| no reversal jitter | PASS, NARROW | checks only negative col steps on one screen-right walk. No direction-change script, no row axis. |
| converge after stop | PASS (probe), NOT IN SUITE | probe: lag at stop 1149 mm (~5 cols at 5 m/s, follow_ms 200), within deadzone+33 mm after 41 frames = 683 ms = 3.4 x follow_ms, final gap 200 mm = deadzone. |
| rotation no holes | PASS, NO NEG | iso.rs:99 cell round trip unproject->project at 32 angles (every screen cell has one world point); iso.rs:113 monotone world line, steps <= 1 cell. No naive-projection negative control. Face choice at intermediate angles (render.rs:398 nearest footprint edge) is not tested; only grid headings (`rotation_swaps_lit_side`). |
| turn ease + retarget | PASS | cam.rs:87: monotone, intermediate angles shown, ends exactly on 1.0 and 2.0, retarget keeps current angle. Retarget restarts smoothstep: speed drops to 0 at the retarget (not measured visually). |
| render decoupled 60 fps | PASS (code) | play.rs:94 `frame_dt` from view.fps; alpha from time to the next tick (play.rs:188). |

## BUG found: flicker while the sim is frozen (pick pause, death screen)

sim/step.rs:24 returns before `self.tick += 1` while a pick is open (pick_pauses true), and step.rs:18 on outcome.
render.rs:139 `shift` keeps prev = tick T-1, cur = T. play.rs:188 alpha still cycles 0..1 every tick period.
So actors and the camera target swing between the T-1 and T positions at 30 Hz while the game is frozen.

Probe (frozen tick 3284, 240 frames, alpha 0 / 0.5 alternating as in play.rs):

    alpha cycling: last bytes [145, 158, 145, 158, 145, 158, 145, 158, 145, 158]  cam dx alternates 0 / 0.0019 mm
    control alpha pinned 1.0: last bytes [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]

Fix (not done, owner decides): when the tick did not advance for one full tick period, render alpha 1 (or copy cur
into prev in `shift`). The second option is one place and needs no play.rs knowledge.

## Other findings

- Golden frame re-pinned: render.rs:893 GOLDEN is 0x4bb6_db87_c343_2452 (was 0x5c69_5c48_44ac_2c08 before P8).
  Expected (blank cells now keep one fg, world-fixed grid), but P8 in PROGRESS.md does not say so.
- Shots, drops, bolts, fx are not interpolated (P8 says so). Shots are the fastest actors; they still step at 30 Hz.
- Brief asked for ONE sub-cell technique with a reason: P8 chose camera at cell resolution + world-fixed grid. OK.

## Bytes per frame, 120x40, 60 fps

| case | source | p50 | p99 | max |
|---|---|---|---|---|
| idle | suite | - | 127 B | - |
| walk, pillars, no enemies | suite (P8) | 0.8 KB | 8.6 KB | - |
| walk, open, director on (live 11) | probe | 340 B | 5.4 KB | 9.5 KB |
| walk, pillars, director on (live 11) | probe | 439 B | 9.1 KB | 13.6 KB |
| turn, pillars, no enemies, 16 frames | suite (P8) | - | - | 8.1 KB (sum 92 KB) |
| turn, open / pillars, live 11, 16 frames | probe | - | - | 3.9 / 7.3 KB (sum 40 / 92 KB) |
| pick pause (bug above) | probe | - | - | 158 B every frame |

All far under budget (typical <= 64 KB, p99 <= 160 KB). NOT MEASURED: dense crowd (live 300-600) while walking
or turning, a dodge at 60 fps in a crowd, real terminal parse time, any live play (no key injection).

## Knobs untuned

fps 60, follow_ms 200 (gives ~1.1 m lag at 5 m/s), deadzone_m 0.2, turn_ms 250: all [E], no human has played them.
