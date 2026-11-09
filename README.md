# ToT proto2: FP1a playable (Netrunner, 15 min, KIOSK 7:30, SWEEPER-9 15:00)

Start: `./play.sh` inside Ghostty >= 1.4.0 (min 120x40). `./play.sh --seed 7` fixes the seed.
Clean window: `GHOSTTY=/nix/store/pda6j5833xwmiqh8w8bpsccs22hxqlk3-ghostty-1.4.0/bin/ghostty ./play.sh --window` (1.4.0 built for proto1).

Keys (data/bindings.jsonl, remappable):
- PLAY: `wasd` / `hjkl` / arrows = move (8 dir). Shift tap = dodge. `x` = hack (also auto with autofire).
- `q` / `e` = rotate camera 90 deg. `:` = command line. Space = leader; which-key shows after 250 ms.
- Leader: `spc a` autofire, `spc l` look, `spc c q/e` camera, `spc q` quit.
- PICK (sim pauses): `h`/`l` select, Enter take, `1`-`3` direct, `s` skip.
- LOOK: `hjkl` cursor, tooltip in the bottom strip, Esc back.
- CMD: `:autofire on|off`, `:cam left|right`, `:spawn <family> [n] [tier] [form]` (src human), `:director`, `:look`, `:q`.
- After death or win: `r` new run (new entropy seed), `q` quit.

Options (5 OPEN rulings, data/options.jsonl): `pick_pauses` (true), `tick_hz` 30|60 (30),
`rewind` "off" only, `rewind_charges` (1, unused in FP1a), `backend` "terminal", `hot_numbers` "F-only".
Also `whichkey_ms` (250), `autofire` (true). Every tuning knob is in data/tuning.jsonl, enemies.jsonl,
skills.jsonl, bosses.jsonl, waves.jsonl. Bad data fails at load with `file:line .path: reason`.

Results (one JSON per line in results/play-<unix>.jsonl):
- `head`: run_seed (hex) + seed source. `pick`: time, skill, rarity, rank, decide_s.
- `alive` every 30 s (partial runs survive a closed window). `runlog`: every Cmd with its src tag.
- `run`: end, run_s, cause, death_foreseen, time_to_first_pick_s, picks_per_min, picks per minute
  window (0-1, 1-4, 9-15), pick_decide_median_s, kills, level, rho samples, state_hash, options.
- Compare with design sec 14.1 bars: first pick 8-15 s, picks/min 0-1 >= 4, median PICK <= 5 s.

Harness: `target/release/proto2 bot --seeds 16 --move kiter --pick tag` (moves kiter|oracle|still|random,
picks tag|dps|random|none, `--mutant zero_tele|plus500:<skill>|double_hp`, `--cmds FILE --src mcp:NAME`).
`proto2 controls --seeds 16` runs the negative controls. `proto2 snapshot --secs 120 [--cam 0-3] [--turn MS] [--ansi FILE]` prints a frame (--cam = e presses; --turn = frame MS into an animated e turn).
Tests: `./test.sh` (clippy 0 warnings + every test isolated, 1 s cap; fails on 0 tests).
