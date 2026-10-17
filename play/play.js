// Concern: renders the example in the URL with sva-wasm, plays it, shows what the engine answered | Non-concern: the fragment (payload.mjs), the layout (index.html) | IO: (location.hash) -> the page

import init, { Composition, outline, readings, subscribe } from "@scalable-vector-audio/sva-wasm";
import { decode } from "./payload.mjs";

const $ = (id) => document.getElementById(id);
/** Each engine call by its id: what this page asked, when, and every report the hook made of it. */
const calls = new Map();
let asking = null;
let totals = null;
let boots = [];

function element(tag, text, className) {
  const made = document.createElement(tag);
  if (text !== undefined) made.textContent = text;
  if (className) made.className = className;
  return made;
}

function said(text, bad = false) {
  $("status").textContent = text;
  $("status").className = bad ? "bad" : "";
}

/** An sva-wasm refusal: `name` is the CLI's error code, `refusal` the envelope; a help the message lacks follows. */
function described(error) {
  const helps = error.refusal.data.diagnostics.items.map((d) => d.help).filter((help) => help && !error.message.includes(help));
  return [`${error.name}: ${error.message}`, ...helps].join("\n");
}

function failed(error) {
  console.error(error);
  said(error?.refusal ? described(error) : String(error), true);
  document.body.dataset.state = "failed";
}

window.addEventListener("error", (event) => failed(event.error ?? event.message));
window.addEventListener("unhandledrejection", (event) => failed(event.reason));
window.addEventListener("hashchange", () => location.reload());

/** The module's engine-stats subscriber: every call of every composition, and the boot. */
function heard(stats) {
  if (stats.boot) boots.push({ at: performance.now(), ...stats.boot });
  totals = stats.totals;
  const call = stats.call;
  if (!call) return;
  if (!calls.has(call.id)) calls.set(call.id, { asked: asking?.asked, started: asking?.started ?? performance.now(), reports: [] });
  const held = calls.get(call.id);
  held.reports.push({ at: performance.now() - held.started, call });
  if (call.phase !== "done") said(`Call ${call.id} (${held.asked}): ${call.phase}, ${call.samples} samples`);
}

/** One render asking one representation; the signal lets the page paint between blocks. */
async function rendered(composition, target, asked, options = {}) {
  asking = { asked, started: performance.now() };
  try {
    return await composition.render(target, [asked], { ...options, signal: new AbortController().signal });
  } finally {
    for (const held of calls.values()) if (held.asked === asked && held.ms === undefined) held.ms = performance.now() - held.started;
    asking = null;
  }
}

const fixed = (n) => (Number.isInteger(n) ? String(n) : Math.abs(n) >= 1 ? n.toFixed(1) : n.toPrecision(2));
const json = (value) => element("pre", JSON.stringify(value, null, 2));

function rows(table, head, body) {
  table.replaceChildren();
  const top = table.insertRow();
  head.forEach((name, at) => top.append(element("th", name, typeof body[0]?.[at] === "number" ? "n" : "")));
  for (const cells of body) {
    const row = table.insertRow();
    for (const cell of cells) {
      const numeric = typeof cell === "number";
      const td = element("td", undefined, numeric ? "n" : "");
      if (cell instanceof Node) td.append(cell);
      else td.textContent = cell === null || cell === undefined ? "" : numeric ? fixed(cell) : String(cell);
      row.append(td);
    }
  }
}

function nodesShown(example) {
  $("target").textContent = example.target;
  for (const [name, text] of Object.entries(example.nodes)) {
    $("nodes").append(element("h3", name), element("pre", text));
  }
}

/** Each outline kind's label and the nodes under it, as sva-wasm's `outline` writes them. */
const KINDS = {
  literal: [(n) => `${n.value} ${n.unit}`, () => []],
  name: [(n) => n.name, () => []],
  operator: [(n) => n.op, (n) => [n.left, n.right]],
  call: [(n) => n.name, (n) => n.args.items],
  ref: [(n) => `@${n.path}, read by ${n.read}`, (n) => [n.arg, ...n.binds.items]],
  self: [(n) => `read by ${n.read}`, (n) => [n.arg]],
  parameter: [(n) => `${n.name}, read by ${n.read}`, (n) => [n.arg]],
};

/** One line per outline node: its kind and label, then the text its span covers; a named argument prefixes its name. */
function tree(node, text, depth, out, named = "") {
  if (node.kind === "positional" || node.kind === "named") {
    tree(node.value, text, depth, out, node.kind === "named" ? `${node.name} = ` : "");
    return;
  }
  const [label, children] = KINDS[node.kind];
  out.push(`${"  ".repeat(depth)}${named}${node.kind} ${label(node)}    ${text.slice(node.span.start, node.span.end)}`);
  for (const child of children(node)) tree(child, text, depth + 1, out);
}

function outlinesShown(example) {
  for (const [name, text] of Object.entries(example.nodes)) {
    $("outlines").append(element("h3", name));
    try {
      const parsed = outline(text);
      const lines = [];
      tree(parsed.outline, parsed.text, 0, lines);
      $("outlines").append(element("pre", lines.join("\n")));
    } catch (error) {
      $("outlines").append(element("pre", described(error), "bad"));
    }
  }
}

function colors() {
  const style = getComputedStyle(document.body);
  const token = (name) => style.getPropertyValue(name).trim();
  return { mark: token("--mark"), ink: token("--muted"), line: token("--line"), surface: token("--surface") };
}

function rgb(color) {
  const probe = document.createElement("canvas").getContext("2d");
  probe.fillStyle = color;
  const hex = probe.fillStyle;
  return [1, 3, 5].map((at) => parseInt(hex.slice(at, at + 2), 16));
}

function sized(canvas) {
  const ratio = window.devicePixelRatio || 1;
  canvas.width = Math.round(canvas.clientWidth * ratio);
  canvas.height = Math.round(canvas.clientHeight * ratio);
  const g = canvas.getContext("2d");
  g.scale(ratio, ratio);
  g.font = "11px system-ui, sans-serif";
  return [g, canvas.clientWidth, canvas.clientHeight];
}

function seconds(g, width, height, duration, c) {
  const step = [0.01, 0.02, 0.05, 0.1, 0.2, 0.25, 0.5, 1, 2, 5, 10, 30, 60].find((s) => duration / s <= 8) ?? 120;
  g.fillStyle = c.ink;
  for (let s = step; s < duration; s += step) g.fillText(`${+s.toFixed(3)} s`, (s / duration) * width + 2, height - 4);
}

/** Min and max of the samples under each pixel column. */
function waveform(canvas, plane, rate, readout) {
  const [g, width, height] = sized(canvas);
  const c = colors();
  const middle = height / 2;
  const full = plane.reduce((most, v) => Math.max(most, Math.abs(v)), 1);
  g.strokeStyle = c.line;
  g.beginPath();
  g.moveTo(0, middle);
  g.lineTo(width, middle);
  g.stroke();
  g.fillStyle = c.mark;
  const per = plane.length / width;
  const span = [];
  for (let x = 0; x < width; x++) {
    let low = Infinity, high = -Infinity;
    const end = Math.min(plane.length, Math.max(Math.floor((x + 1) * per), Math.floor(x * per) + 1));
    for (let i = Math.floor(x * per); i < end; i++) {
      low = Math.min(low, plane[i]);
      high = Math.max(high, plane[i]);
    }
    span.push([low, high]);
    const reach = middle - 2;
    if (low <= high) g.fillRect(x, middle - (high / full) * reach, 1, Math.max(1, ((high - low) / full) * reach));
  }
  seconds(g, width, height, plane.length / rate, c);
  g.fillStyle = c.ink;
  g.fillText(`+${+full.toFixed(3)}`, 2, 11);
  g.fillText(`-${+full.toFixed(3)}`, 2, height - 16);
  canvas.onmousemove = (event) => {
    const x = Math.min(width - 1, Math.max(0, Math.floor(event.offsetX)));
    const [low, high] = span[x];
    readout.textContent = `t = ${((x * per) / rate).toFixed(4)} s, min ${low.toFixed(4)}, max ${high.toFixed(4)}`;
  };
}

/** The engine's `bands` reading as drawn: one row per auditory band, one column per frame, rms in dB. */
function spectrogram(canvas, answer, duration, readout) {
  const [g, width, height] = sized(canvas);
  const c = colors();
  const bands = answer.bands.items;
  const db = bands.map((band) => band.rms.items.map((v) => 20 * Math.log10(v + 1e-12)));
  const loudest = db.reduce((most, row) => row.reduce((m, v) => Math.max(m, v), most), -Infinity);
  const floor = loudest - 80;
  const [from, to] = [rgb(c.surface), rgb(c.mark)];
  const pixels = g.createImageData(width, height);
  const cell = (x, y) => {
    const band = Math.min(bands.length - 1, Math.floor(((height - 1 - y) / height) * bands.length));
    const frame = Math.min(db[band].length - 1, Math.floor((x / width) * duration * answer.rate_hz));
    return [band, frame];
  };
  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const [band, frame] = cell(x, y);
      const share = Math.min(1, Math.max(0, (db[band][frame] - floor) / (loudest - floor)));
      const at = (y * width + x) * 4;
      for (let k = 0; k < 3; k++) pixels.data[at + k] = from[k] + (to[k] - from[k]) * share;
      pixels.data[at + 3] = 255;
    }
  }
  const scaled = document.createElement("canvas");
  [scaled.width, scaled.height] = [width, height];
  scaled.getContext("2d").putImageData(pixels, 0, 0);
  g.drawImage(scaled, 0, 0, width, height);
  g.fillStyle = c.ink;
  const step = Math.max(1, Math.ceil(bands.length / 8));
  bands.forEach((band, at) => {
    if (at % step === 0) g.fillText(`${Math.round(band.centre_hz)} Hz`, 2, height - ((at + 0.5) / bands.length) * height + 4);
  });
  seconds(g, width, height, duration, c);
  canvas.onmousemove = (event) => {
    const [band, frame] = cell(Math.floor(event.offsetX), Math.floor(event.offsetY));
    const t = answer.start_secs + frame / answer.rate_hz;
    readout.textContent = `t = ${t.toFixed(3)} s, band at ${bands[band].centre_hz.toFixed(0)} Hz, ${(db[band][frame] - loudest).toFixed(1)} dB below the loudest cell`;
  };
}

function soundShown(rendering) {
  const rate = rendering.sample_rate;
  const planes = Array.from({ length: rendering.channels }, (_, c) => rendering.samples(c));
  const frames = planes[0]?.length ?? 0;
  const facts = element("table");
  rows(facts, ["sample_rate", "duration_secs", "channels", "frames"], [[rate, rendering.duration_secs, rendering.channels, frames]]);
  $("sound").append(facts);
  planes.forEach((plane, c) => {
    const wave = element("canvas");
    const readout = element("p", "Hover for values.", "muted");
    $("sound").append(element("h3", `Waveform${planes.length > 1 ? ` of channel ${c}` : ""}`), wave, readout);
    waveform(wave, plane, rate, readout);
  });
  $("play").disabled = frames === 0;
  $("play").onclick = () => {
    const context = new AudioContext();
    const buffer = context.createBuffer(planes.length, frames, rate);
    planes.forEach((plane, c) => buffer.copyToChannel(plane, c));
    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(context.destination);
    source.onended = () => {
      $("playback").textContent = "ended";
      context.close();
    };
    $("playback").textContent = "playing";
    source.start();
  };
  return rendering.duration_secs;
}

async function spectrogramShown(composition, example, duration) {
  $("sound").append(element("h3", "Spectrogram"), element("p", "The bands reading: each auditory band's rms over time, 80 dB below the loudest cell to it.", "muted"));
  try {
    const done = await rendered(composition, example.target, "bands", { out: null });
    const answer = Object.values(done.representations().representations)[0].value;
    done.free();
    const canvas = element("canvas", undefined, "tall");
    const readout = element("p", "Hover for values.", "muted");
    $("sound").append(canvas, readout);
    spectrogram(canvas, answer, duration, readout);
  } catch (error) {
    $("sound").append(element("pre", described(error), "bad"));
  }
}

const typing = (call) => (call.typing ? `${call.typing.typed}/${call.typing.nodes} of ${call.typing.instances}` : "");

function callsShown() {
  const body = [...calls.entries()].map(([id, held]) => {
    const last = held.reports.at(-1)?.call ?? {};
    const l = last.lookups ?? {};
    return [id, last.kind, held.asked, last.phase, held.ms ?? null, last.samples, last.total_samples, last.planned_samples, last.computed_samples, last.cached_samples, `${l.hit ?? 0}/${l.miss ?? 0}/${l.new ?? 0}/${l.reused ?? 0}/${l.prefix ?? 0}`, typing(last), held.reports.length];
  });
  rows($("calls"), ["id", "kind", "asked", "phase", "ms", "samples", "total_samples", "planned_samples", "computed_samples", "cached_samples", "hit/miss/new/reused/prefix lookups", "typed/nodes of instances", "reports"], body);
}

function timelineShown(id) {
  const reports = calls.get(id)?.reports ?? [];
  rows($("timeline"), ["ms", "phase", "samples", "total_samples", "planned_samples", "computed_samples", "cached_samples", "typed/nodes of instances"], reports.map(({ at, call }) => [at, call.phase, call.samples, call.total_samples, call.planned_samples, call.computed_samples, call.cached_samples, typing(call)]));
}

function engineShown(composition, version, loaded) {
  rows($("engine"), ["", ""], [
    ["@scalable-vector-audio/sva-wasm", version],
    ["init(), ms: fetch and compile the module", loaded],
    ["crossOriginIsolated", String(window.crossOriginIsolated)],
    ["boot reports", boots.length === 0 ? "none: new Composition() reports no boot" : json(boots)],
    ["cache_bytes", composition.cache_bytes],
    ["cache_max_bytes", composition.cache_max_bytes],
    ["cache_entries", composition.cache_entries],
    ["cache_evictions", composition.cache_evictions],
    ["counters()", json(composition.counters())],
    ["totals, every call so far", json(totals)],
  ]);
}

async function readingsShown(composition, example) {
  const body = [];
  for (const reading of readings()) {
    if (reading.name === "samples") continue;
    const asks = reading.takes.includes("node")
      ? Object.keys(example.nodes).map((name) => `${reading.name}(node=@${name})`)
      : [reading.name];
    for (const asked of asks) {
      said(`Reading ${asked}.`);
      const started = performance.now();
      let answer;
      try {
        const done = await rendered(composition, example.target, asked, { out: null });
        answer = json(Object.values(done.representations().representations)[0]);
        done.free();
      } catch (error) {
        answer = element("pre", described(error), "bad");
      }
      body.push([asked, reading.reads, reading.help, performance.now() - started, answer]);
    }
  }
  rows($("readings"), ["asked", "reads", "help", "ms", "answer"], body);
}

async function main() {
  const fragment = location.hash.slice(1);
  if (!fragment) {
    said("No example in this URL: open one from a [Play] link in the README.");
    return;
  }
  const example = await decode(fragment);
  nodesShown(example);

  said("Loading sva-wasm.");
  const started = performance.now();
  await init();
  const loaded = performance.now() - started;
  const manifest = new URL("package.json", import.meta.resolve("@scalable-vector-audio/sva-wasm"));
  const { version } = await (await fetch(manifest)).json();
  subscribe(heard);

  const composition = new Composition("readme");
  for (const [name, text] of Object.entries(example.nodes)) composition.insert(name, text);
  outlinesShown(example);

  said("Rendering the samples.");
  const main = await rendered(composition, example.target, "samples");
  const mainId = [...calls.keys()].at(-1);
  const duration = soundShown(main);
  const answer = main.representations();
  for (const reading of Object.values(answer.representations ?? {})) delete reading.value;
  $("answer").textContent = JSON.stringify({ answer, stats: main.stats(), work: main.work() }, null, 2);
  main.free();
  timelineShown(mainId);
  callsShown();

  await spectrogramShown(composition, example, duration);
  await readingsShown(composition, example);
  callsShown();
  engineShown(composition, version, loaded);
  said(`Rendered ${example.target}: ${calls.size} engine calls.`);
  document.body.dataset.state = "done";
}

main().catch(failed);
