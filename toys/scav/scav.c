// scavenger toy: loot field (multi-source Dial BFS w/ rarity offsets) + per-tick scavenger step + player pickup.
// static arrays only (0 malloc). integer only. deterministic (xorshift seed fixed).
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#define W 192
#define N (W*W)
#define MAXG 1024      /* ground loot slots */
#define MAXS 16        /* scavengers */
#define INF 0xFFFF
#define MAXOFF 12      /* rarity offset in tiles: unique 12, rare 6, magic 3, shard 0 */
static uint8_t walk[N];
static uint16_t field[N];
static int32_t q[MAXOFF + 400][N / 4]; /* bucket queue; bucket b holds tiles at dist b */
static int32_t qn[MAXOFF + 400];
static int16_t gx[MAXG], gy[MAXG]; static uint8_t grar[MAXG], galive[MAXG]; static uint16_t gage[MAXG];
static int16_t tile_head[N]; static int16_t gnext[MAXG];
static int32_t sx[MAXS], sy[MAXS]; static uint16_t shoard[MAXS]; static uint8_t schan[MAXS];
static uint64_t rs = 0x9E3779B97F4A7C15ull;
static uint32_t rnd(uint32_t n) { rs ^= rs << 13; rs ^= rs >> 7; rs ^= rs << 17; return (uint32_t)(rs % n); }
static uint64_t ns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec; }
static void carve(int x0, int y0, int w, int h) { for (int y = y0; y < y0 + h; y++) for (int x = x0; x < x0 + w; x++) if (x >= 0 && y >= 0 && x < W && y < W) walk[y * W + x] = 1; }
static const int off[4] = {0, 6, 3, 12}; /* by rarity: 0 shard,1 rare,2 magic,3 unique */
static int bfs(int stale_age) {
  memset(field, 0xFF, sizeof field); memset(qn, 0, sizeof qn);
  int src = 0;
  for (int i = 0; i < MAXG; i++) if (galive[i] && gage[i] >= stale_age) {
    int t = gy[i] * W + gx[i]; int b = MAXOFF - off[grar[i]];
    if (field[t] > b) { field[t] = b; q[b][qn[b]++] = t; src++; } }
  for (int b = 0; b < MAXOFF + 400; b++) for (int k = 0; k < qn[b]; k++) {
    int t = q[b][k]; if (field[t] != b) continue; int x = t % W, y = t / W;
    int nb[4] = {x > 0 ? t - 1 : -1, x < W - 1 ? t + 1 : -1, y > 0 ? t - W : -1, y < W - 1 ? t + W : -1};
    for (int j = 0; j < 4; j++) { int u = nb[j]; if (u < 0 || !walk[u] || field[u] <= b + 1 || b + 1 >= MAXOFF + 400) continue;
      if (qn[b + 1] >= N / 4) continue; field[u] = b + 1; q[b + 1][qn[b + 1]++] = u; } }
  return src;
}
static void rehash(void) { memset(tile_head, 0xFF, sizeof tile_head);
  for (int i = MAXG - 1; i >= 0; i--) if (galive[i]) { int t = gy[i] * W + gx[i]; gnext[i] = tile_head[t]; tile_head[t] = i; } }
static int picks, interrupts, stalls;
static void scav_step(int s, int hit) {
  int t = sy[s] * W + sx[s];
  if (schan[s]) { if (hit) { schan[s] = 0; interrupts++; return; }
    if (--schan[s] == 0) { int i = tile_head[t]; if (i >= 0) { shoard[s] += 1 + 10 * grar[i]; galive[i] = 0; tile_head[t] = gnext[i]; picks++; } } return; }
  if (tile_head[t] >= 0 && gage[tile_head[t]] >= 240) { schan[s] = 15; return; }
  int best = t, bv = field[t]; int x = sx[s], y = sy[s];
  int nb[4] = {x > 0 ? t - 1 : -1, x < W - 1 ? t + 1 : -1, y > 0 ? t - W : -1, y < W - 1 ? t + W : -1};
  for (int j = 0; j < 4; j++) { int u = nb[j]; if (u >= 0 && walk[u] && field[u] < bv) { bv = field[u]; best = u; } } /* tie-break = fixed neighbour order */
  if (bv == INF) { stalls++; return; } /* real game: fall back to player flow field */
  sx[s] = best % W; sy[s] = best / W;
}
static int player_pickup(int px, int py) { int got = 0;
  for (int dy = -3; dy <= 3; dy++) for (int dx = -3; dx <= 3; dx++) { int x = px + dx, y = py + dy; if (x < 0 || y < 0 || x >= W || y >= W) continue;
    int t = y * W + x; while (tile_head[t] >= 0) { int i = tile_head[t]; galive[i] = 0; tile_head[t] = gnext[i]; got++; } } return got; }
int main(void) {
  for (int r = 0; r < 12; r++) { int cx = (r % 4) * 48 + 8, cy = (r / 4) * 64 + 8; carve(cx, cy, 22, 22); if (r % 4 < 3) carve(cx + 22, cy + 9, 26, 4); if (r < 8) carve(cx + 9, cy + 22, 4, 42); }
  int wcount = 0; for (int i = 0; i < N; i++) wcount += walk[i];
  int nitems = 400;
  for (int i = 0; i < nitems; i++) { int t; do t = rnd(N); while (!walk[t]); gx[i] = t % W; gy[i] = t / W; galive[i] = 1; uint32_t r = rnd(100); grar[i] = r < 90 ? 0 : r < 96 ? 2 : r < 99 ? 1 : 3; gage[i] = 240 + rnd(300); }
  for (int s = 0; s < MAXS; s++) { int t; do t = rnd(N); while (!walk[t]); sx[s] = t % W; sy[s] = t / W; }
  rehash();
  uint64_t t0 = ns(); int srcs = 0; for (int k = 0; k < 200; k++) srcs = bfs(240); uint64_t tb = (ns() - t0) / 200;
  uint64_t t1 = ns(); for (int k = 0; k < 2000; k++) rehash(); uint64_t th = (ns() - t1) / 2000;
  uint64_t t2 = ns(); long steps = 0;
  for (int tick = 0; tick < 9000; tick++) { if (tick % 30 == 0) { bfs(240); } for (int s = 0; s < MAXS; s++) { scav_step(s, rnd(64) == 0); steps++; } }
  uint64_t ts = (ns() - t2); /* includes the BFS every 30 ticks */
  uint64_t t3 = ns(); int got = 0; for (int k = 0; k < 10000; k++) got += player_pickup(96, 96); uint64_t tp = (ns() - t3) / 10000;
  int alive = 0; for (int i = 0; i < MAXG; i++) alive += galive[i];
  printf("walkable=%d sources=%d bfs_ns=%llu rehash_ns=%llu\n", wcount, srcs, (unsigned long long)tb, (unsigned long long)th);
  printf("9000 ticks x %d scav: total_ns=%llu per_tick_ns=%llu picks=%d interrupts=%d stalls=%d alive_left=%d\n", MAXS, (unsigned long long)ts, (unsigned long long)(ts / 9000), picks, interrupts, stalls, alive);
  printf("player_pickup 7x7 tiles ns=%llu got=%d\n", (unsigned long long)tp, got);
  return 0;
}
