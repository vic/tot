import { init, Terminal, FitAddon } from "ghostty-web";

const msg = document.getElementById("msg");

// tot speaks the kitty keyboard protocol (key releases = held keys). Neither the terminal widget nor the
// browser gives it, so the page turns key events into CSI u sequences itself.
const PUNCT = { Minus: "-", Equal: "=", BracketLeft: "[", BracketRight: "]", Backslash: "\\", Semicolon: ";",
  Quote: "'", Comma: ",", Period: ".", Slash: "/", Backquote: "`", Space: " " };
const FUNC = { Enter: 13, Escape: 27, Tab: 9, Backspace: 127, ShiftLeft: 57441, ShiftRight: 57447 };
const ARROW = { ArrowUp: "A", ArrowDown: "B", ArrowRight: "C", ArrowLeft: "D" };

function kitty(e) {
  const kind = e.type === "keyup" ? 3 : e.repeat ? 2 : 1;
  const mods = 1 + (e.shiftKey ? 1 : 0) + (e.altKey ? 2 : 0) + (e.ctrlKey ? 4 : 0) + (e.metaKey ? 8 : 0);
  const m = kind === 1 && mods === 1 ? "" : `;${mods}${kind === 1 ? "" : ":" + kind}`;
  if (ARROW[e.code]) return `\x1b[1${m || ";1"}${ARROW[e.code]}`;
  let base = FUNC[e.code];
  let shifted = "";
  if (base === undefined) {
    const k = /^Key([A-Z])$/.exec(e.code)?.[1].toLowerCase() ?? /^Digit(\d)$/.exec(e.code)?.[1] ?? PUNCT[e.code];
    if (k === undefined) return null;
    base = k.codePointAt(0);
    if (e.shiftKey && e.key.length === 1) shifted = ":" + e.key.codePointAt(0);
  }
  return `\x1b[${base}${shifted}${m}u`;
}

if (!crossOriginIsolated) {
  msg.textContent = "starting… (this page reloads once to become cross-origin isolated, which SharedArrayBuffer needs)";
  throw new Error("not cross-origin isolated");
}

await init();
const term = new Terminal({ fontFamily: "monospace", fontSize: 14, theme: { background: "#05031a" } });
const fit = new FitAddon();
term.loadAddon(fit);
term.open(document.getElementById("term"));
fit.fit();

const ctl = new SharedArrayBuffer(16);
const ring = new SharedArrayBuffer(1 << 16);
const c = new Int32Array(ctl);
const buf = new Uint8Array(ring);
const size = () => { Atomics.store(c, 2, term.rows); Atomics.store(c, 3, term.cols); };
size();

const send = (s) => {
  const b = new TextEncoder().encode(s);
  const w = Atomics.load(c, 0);
  for (let i = 0; i < b.length; i++) buf[(w + i) & (buf.length - 1)] = b[i];
  Atomics.store(c, 0, w + b.length);
  Atomics.notify(c, 0);
};

const worker = new Worker(new URL("./worker.js", import.meta.url), { type: "module" });
worker.onmessage = ({ data }) => {
  if (data.out) term.write(data.out);
  else if (data.started) msg.remove();
  else if ("exit" in data) term.write("\r\n[tot exited — reload to play again]\r\n");
};
worker.onerror = (e) => { msg.textContent = "worker error: " + e.message + " " + e.filename + ":" + e.lineno; };
addEventListener("unhandledrejection", (e) => { msg.textContent = "error: " + e.reason; });
worker.postMessage({ ctl, ring });

term.attachCustomKeyEventHandler((e) => {
  const seq = kitty(e);
  if (seq === null) return true;
  e.preventDefault();
  send(seq);
  return false;
});
addEventListener("keyup", (e) => { const s = kitty(e); if (s !== null) { e.preventDefault(); send(s); } }); // ghostty-web gives no key releases
term.onData((d) => { if (d.startsWith("\x1b[<")) send(d); }); // mouse reports (the sponsor button)
addEventListener("resize", () => { fit.fit(); size(); send("\x1b[?2048;0;0;0;0t"); });
term.focus();
