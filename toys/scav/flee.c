// flee toy: player BFS + Brogue safety map (x -1.2, rescan) vs plain negated field.
// counts sink tiles (no neighbour improves) far from player; times build + 20k readers. static, integer.
#include <stdio.h>
#include <stdint.h>
#include <string.h>
#include <time.h>
#define W 192
#define N (W*W)
#define NB 8192
#define NE 20000
static uint8_t walk[N];
static int32_t dist[N], safe[N], neg[N];
static int32_t qa[N];
static int32_t head[NB]; static int32_t pt[4 * N], pn[4 * N]; static int np;
static int32_t ex[NE], ey[NE];
static uint64_t rs = 0x9E3779B97F4A7C15ull;
static uint32_t rnd(uint32_t n) { rs ^= rs << 13; rs ^= rs >> 7; rs ^= rs << 17; return (uint32_t)(rs % n); }
static uint64_t ns(void) { struct timespec t; clock_gettime(CLOCK_MONOTONIC, &t); return (uint64_t)t.tv_sec * 1000000000ull + t.tv_nsec; }
static void carve(int x0, int y0, int w, int h) { for (int y = y0; y < y0 + h; y++) for (int x = x0; x < x0 + w; x++) if (x >= 0 && y >= 0 && x < W && y < W) walk[y * W + x] = 1; }
static int nbr(int t, int j) { int x = t % W, y = t / W;
  switch (j) { case 0: return x > 0 ? t - 1 : -1; case 1: return x < W - 1 ? t + 1 : -1; case 2: return y > 0 ? t - W : -1; default: return y < W - 1 ? t + W : -1; } }
static int bfs(int src) { for (int i = 0; i < N; i++) dist[i] = -1; int h = 0, n = 0, maxd = 0; dist[src] = 0; qa[n++] = src;
  while (h < n) { int t = qa[h++]; for (int j = 0; j < 4; j++) { int u = nbr(t, j); if (u >= 0 && walk[u] && dist[u] < 0) { dist[u] = dist[t] + 1; if (dist[u] > maxd) maxd = dist[u]; qa[n++] = u; } } } return maxd; }
// safety: cost scaled x5; c0 = 6*(maxd - d) (= -1.2 d shifted), step 5; Dial rescan with every tile as source.
static void push(int b, int t) { if (np >= 4 * N) return; pt[np] = t; pn[np] = head[b]; head[b] = np++; }
static void safety(int maxd) { memset(head, 0xFF, sizeof head); np = 0;
  for (int t = 0; t < N; t++) { if (!walk[t] || dist[t] < 0) { safe[t] = 1 << 30; continue; } int c = 6 * (maxd - dist[t]); safe[t] = c; push(c, t); }
  for (int b = 0; b < NB; b++) { int p = head[b]; while (p >= 0) { int t = pt[p]; int pnx = pn[p]; if (safe[t] == b) for (int j = 0; j < 4; j++) { int u = nbr(t, j); if (u >= 0 && walk[u] && safe[u] > b + 5 && b + 5 < NB) { safe[u] = b + 5; push(b + 5, u); } } p = pnx; } } }
static int sinks(const int32_t *f, int mind) { int s = 0; for (int t = 0; t < N; t++) { if (!walk[t] || dist[t] < mind) continue; int best = f[t];
    for (int j = 0; j < 4; j++) { int u = nbr(t, j); if (u >= 0 && walk[u] && f[u] < best) best = f[u]; } if (best == f[t]) s++; } return s; }
int main(void) {
  for (int r = 0; r < 12; r++) { int cx = (r % 4) * 48 + 8, cy = (r / 4) * 64 + 8; carve(cx, cy, 22, 22); if (r % 4 < 3) carve(cx + 22, cy + 9, 26, 4); if (r < 8) carve(cx + 9, cy + 22, 4, 42); }
  carve(0, 120, 8, 4); /* dead-end spur */
  int ptile = (8 + 64 + 11) * W + (8 + 48 + 11); /* centre of room 5 */
  uint64_t t0 = ns(); int maxd = 0; for (int k = 0; k < 200; k++) maxd = bfs(ptile); uint64_t tb = (ns() - t0) / 200;
  uint64_t t1 = ns(); for (int k = 0; k < 200; k++) safety(maxd); uint64_t tsf = (ns() - t1) / 200;
  for (int t = 0; t < N; t++) neg[t] = (walk[t] && dist[t] >= 0) ? 6 * (maxd - dist[t]) : 1 << 30;
  int sn = sinks(neg, 8), ss = sinks(safe, 8);
  for (int e = 0; e < NE; e++) { int t; do t = rnd(N); while (!walk[t]); ex[e] = t % W; ey[e] = t / W; }
  uint64_t t2 = ns(); long moved = 0;
  for (int tick = 0; tick < 300; tick++) for (int e = 0; e < NE; e++) { int t = ey[e] * W + ex[e]; int best = t, bv = safe[t];
      for (int j = 0; j < 4; j++) { int u = nbr(t, j); if (u >= 0 && walk[u] && safe[u] < bv) { bv = safe[u]; best = u; } }
      if (best != t) { ex[e] = best % W; ey[e] = best / W; moved++; } }
  uint64_t tr = (ns() - t2) / 300;
  int walkc = 0; for (int t = 0; t < N; t++) walkc += walk[t];
  printf("walkable=%d maxd=%d player_bfs_ns=%llu safety_rescan_ns=%llu\n", walkc, maxd, (unsigned long long)tb, (unsigned long long)tsf);
  printf("sinks(d>=8): negated=%d rescanned=%d\n", sn, ss);
  printf("20k flee readers: ns/tick=%llu ns/ent=%llu moved=%ld\n", (unsigned long long)tr, (unsigned long long)(tr / NE), moved);
  return 0;
}
