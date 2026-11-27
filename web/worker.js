// Runs tot (wasm32-wasip1) in a worker. The game blocks in its own loop, so everything it waits on is a SharedArrayBuffer:
// ctl[0] = bytes the page wrote to stdin (monotonic), ctl[2] = rows, ctl[3] = cols; the ring holds the stdin bytes.
// A sleeping `poll_oneoff` waits on ctl[0], so a key press wakes the game at once.
import { WASI, File, Directory, PreopenDirectory, Fd, wasi } from "@bjorn3/browser_wasi_shim";
import totUrl from "./tot.wasm?url";

const files = import.meta.glob("../data/**/*", { query: "?raw", import: "default", eager: true });
const enc = new TextEncoder();

function tree() {
  const root = new Map();
  for (const [path, text] of Object.entries(files)) {
    const parts = path.replace("../data/", "").split("/");
    let dir = root;
    for (const d of parts.slice(0, -1)) {
      if (!dir.has(d)) dir.set(d, new Directory(new Map()));
      dir = dir.get(d).contents;
    }
    dir.set(parts.at(-1), new File(enc.encode(text)));
  }
  return root;
}

self.onmessage = async ({ data: { ctl, ring } }) => {
  const c = new Int32Array(ctl);
  const buf = new Uint8Array(ring);
  const mask = buf.length - 1;
  let rpos = 0;

  class Stdin extends Fd {
    fd_read(size) {
      const w = Atomics.load(c, 0);
      const n = Math.min(size, w - rpos);
      const data = new Uint8Array(n);
      for (let i = 0; i < n; i++) data[i] = buf[(rpos + i) & mask];
      rpos += n;
      return { ret: 0, data };
    }
    fd_filestat_get() { return { ret: wasi.ERRNO_SUCCESS, filestat: new wasi.Filestat(0n, wasi.FILETYPE_CHARACTER_DEVICE, 0n) }; }
  }
  class Stdout extends Fd {
    fd_write(data) {
      self.postMessage({ out: data.slice() });
      return { ret: 0, nwritten: data.byteLength };
    }
    fd_filestat_get() { return { ret: wasi.ERRNO_SUCCESS, filestat: new wasi.Filestat(0n, wasi.FILETYPE_CHARACTER_DEVICE, 0n) }; }
  }
  class Stderr extends Fd {
    fd_write(data) {
      console.error(new TextDecoder().decode(data));
      return { ret: 0, nwritten: data.byteLength };
    }
  }
  // "rows cols", live from the page's resizes
  class SizeFile extends File {
    get data() { return enc.encode(`${Atomics.load(c, 2)} ${Atomics.load(c, 3)}`); }
    set data(_) {}
  }

  const fds = [
    new Stdin(), new Stdout(), new Stderr(),
    new PreopenDirectory("/data", tree()),
    new PreopenDirectory("/out", new Map()),
    new PreopenDirectory("/term", new Map([["size", new SizeFile(new Uint8Array())]])),
  ];
  const w = new WASI(["tot", "play", "--data", "/data", "--out", "/out"], [], fds);

  // one relative or absolute clock subscription = a sleep that input interrupts
  w.wasiImport.poll_oneoff = (inp, out, n, nev) => {
    const m = new DataView(w.inst.exports.memory.buffer);
    let ns = m.getBigUint64(inp + 24, true);
    if (m.getUint16(inp + 40, true) & 1) ns -= BigInt(Math.round(performance.timeOrigin * 1e6 + performance.now() * 1e6));
    if (ns > 0n) Atomics.wait(c, 0, rpos, Number(ns) / 1e6);
    m.setBigUint64(out, m.getBigUint64(inp, true), true);
    m.setUint16(out + 8, 0, true);
    m.setUint8(out + 10, 0);
    m.setUint32(nev, 1, true);
    return 0;
  };

  const { instance } = await WebAssembly.instantiateStreaming(fetch(totUrl), { wasi_snapshot_preview1: w.wasiImport });
  self.postMessage({ started: true });
  const code = w.start(instance);
  self.postMessage({ exit: code });
};
